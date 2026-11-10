//! Fabric insight: is the RoCE fabric lossless, is NCCL really on RDMA, and is every
//! node cabled where it was?
//!
//! `FabricMonitor` sees every published snapshot. It keeps a minute of each fabric
//! interface's cumulative RoCE and PHY counters (from roce.py in Live, the simulator
//! in Demo) to turn them into per-second rates, and an hour of loss events, then
//! gives each interface and the whole fabric a verdict: Lossless, Congested
//! (DCQCN at work, informative), Lossy (retransmitting), PHY errors, Pause storm,
//! Dropping, or Not lossless-configured (only when mlnx_qos data exists).
//!
//! It also places each node's fabric interfaces on switch ports (by MAC, LLDP, then
//! comment; see routeros::mapping), compares that with the learned cabling
//! baseline (fabric_cabling.json, set on the first complete mapping or by "Accept
//! current cabling") and checks each multi-node serving group's NCCL transport.

pub mod commands;
pub mod doctor;
pub mod parse;
pub mod rates;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::inventory::SwitchKind;
use crate::model::{ClusterSnapshot, LinkStatus, NetRole, NetStats, NodeSnapshot, RoceCounters};
use crate::routeros::mapping::{self, PortTables};

pub const STATE_EVENT: &str = "fabric://state";

/// Samples kept per interface: the long rate window, and a little slack.
const KEEP_MS: u64 = rates::LONG_MS + 2_000;
/// Loss events are remembered this long ("in the last hour").
const LOSS_MEMORY_MS: u64 = 3_600_000;
/// Pause frames per second that count as a storm once sustained…
pub const PAUSE_STORM_PER_S: f64 = 10_000.0;
/// …for this long.
const PAUSE_STORM_MS: u64 = 30_000;
/// ECN marks per second above which the fabric reads Congested rather than quietly lossless.
const CONGESTED_ECN_PER_S: f64 = 1_000.0;
/// RDMA share of a serving group's fabric bytes below which NCCL is on TCP sockets.
const TCP_FALLBACK_PCT: f64 = 50.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    Loss,
    Phy,
    Congestion,
    Pause,
    Drop,
    Link,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    Crit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerdictKind {
    Lossless,
    Congested,
    Lossy,
    Phy,
    PauseStorm,
    NotLossless,
    WasLossy,
    Dropping,
    Down,
    NoData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub kind: VerdictKind,
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub action: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterReading {
    pub key: String,
    pub label: String,
    pub total: Option<u64>,
    pub per_s: Option<f64>,
    pub group: Group,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IfaceHealth {
    pub node: String,
    pub node_name: String,
    pub iface: String,
    pub half: u8,
    pub up: bool,
    pub speed_mbps: u32,
    pub mtu: Option<u32>,
    pub ip: Option<String>,
    pub rdma_dev: Option<String>,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rdma_rx_bps: Option<f64>,
    pub rdma_tx_bps: Option<f64>,
    pub rdma_share: Option<f64>,
    pub switch_port: Option<String>,
    pub switch_id: Option<String>,
    pub mapped_by: Option<String>,
    pub cabling_ok: Option<bool>,
    pub expected_port: Option<String>,
    pub verdict: Verdict,
    pub counters: Vec<CounterReading>,
    pub loss_last_hour: u64,
    pub last_loss_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportCheck {
    pub name: String,
    pub nodes: Vec<String>,
    pub rdma_pct: Option<f64>,
    pub level: Level,
    pub message: String,
    pub env_issues: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CablingIssue {
    pub node: String,
    pub iface: String,
    pub half: u8,
    pub switch_id: String,
    pub port: String,
    pub expected_port: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricState {
    pub ts_ms: u64,
    pub verdict: Verdict,
    pub summary: String,
    pub ifaces: Vec<IfaceHealth>,
    pub transport: Vec<TransportCheck>,
    pub cabling: Vec<CablingIssue>,
    pub baseline_set: bool,
    // ── r4-alerts ──
    /// Known configuration worth knowing, never a fault and never counted: PFC off.
    #[serde(default)]
    pub advisory: Option<Advisory>,
}

/// An advisory: a grey ⓘ banner in Fabric, neutral cells and links, nothing counted or paged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Advisory {
    /// "pfcOff"
    pub kind: String,
    /// "The fabric isn't lossless."
    pub title: String,
    /// What it is, what it costs (from the counters), and how to change it.
    pub body: String,
    /// "PFC off", for a sidebar row or a cell.
    pub short: String,
}

/// Where each node fabric interface ("node/iface") was cabled, per switch.
pub type Cabling = BTreeMap<String, BTreeMap<String, String>>;

use rates::Sample;

#[derive(Default)]
struct IfaceTrack {
    samples: VecDeque<Sample>,
    /// (when, how many) loss or PHY errors were counted, for the last hour.
    losses: VecDeque<(u64, u64)>,
    /// Since when pause frames have run above the storm rate.
    pause_high_since: Option<u64>,
}

#[derive(Default)]
struct GroupTrack {
    /// (ts, each member's (node, RDMA bytes, IP bytes)) per second while the group was busy, last 5 minutes.
    busy: VecDeque<(u64, Vec<(String, f64, f64)>)>,
}

#[derive(Default)]
struct Inner {
    ifaces: HashMap<String, IfaceTrack>,
    groups: HashMap<String, GroupTrack>,
    first_ts: Option<u64>,
    last_ts: Option<u64>,
    /// Live baseline (saved), and Demo's (in memory, back to the world's own cabling on every scenario change).
    live_cabling: Cabling,
    demo_cabling: Option<(Option<String>, Cabling)>,
    /// What the tables showed last time, for "Accept current cabling".
    last_seen: BTreeMap<String, BTreeMap<String, String>>,
    /// Each switch's last tables, for while it doesn't answer.
    last_tables: BTreeMap<String, PortTables>,
    /// The Demo scenario last assessed: a new one starts a new history (its faults aren't the real past).
    last_scenario: Option<Option<String>>,
    /// Who was last seen on each port ("switch/port" → node, how), kept while its link is down
    /// (the switch forgets the MACs behind a port that has no link).
    last_on_port: HashMap<String, (String, String)>,
    state: Option<FabricState>,
    /// The counter rates the last assessment worked out (the health engine reads the same table).
    rates: Option<Arc<rates::Rates>>,
}

pub struct FabricMonitor {
    cabling_path: Option<PathBuf>,
    inner: Mutex<Inner>,
}

/// Every counter the Fabric window shows, in display order.
pub fn readings(c: &RoceCounters) -> Vec<(String, &'static str, Group, Option<u64>)> {
    let mut v: Vec<(String, &'static str, Group, Option<u64>)> = vec![
        ("outOfSequence".into(), "Out of sequence", Group::Loss, c.out_of_sequence),
        ("packetSeqErr".into(), "Sequence errors", Group::Loss, c.packet_seq_err),
        ("localAckTimeoutErr".into(), "ACK timeouts", Group::Loss, c.local_ack_timeout_err),
        ("impliedNakSeqErr".into(), "Implied NAKs", Group::Loss, c.implied_nak_seq_err),
        ("duplicateRequest".into(), "Duplicate requests", Group::Loss, c.duplicate_request),
        ("rnrNakRetryErr".into(), "RNR NAK retries", Group::Loss, c.rnr_nak_retry_err),
        ("reqCqeErr".into(), "Requester CQE errors", Group::Loss, c.req_cqe_err),
        ("respCqeErr".into(), "Responder CQE errors", Group::Loss, c.resp_cqe_err),
        ("rxCrcErrorsPhy".into(), "CRC errors", Group::Phy, c.rx_crc_errors_phy),
        ("rxSymbolErrPhy".into(), "Symbol errors (PHY)", Group::Phy, c.rx_symbol_err_phy),
        ("symbolError".into(), "Symbol errors (port)", Group::Phy, c.symbol_error),
        ("rxIcrcErr".into(), "ICRC errors", Group::Phy, c.rx_icrc_err),
        ("portRcvErrors".into(), "Receive errors", Group::Phy, c.port_rcv_errors),
        ("rxDiscardsPhy".into(), "Discards (PHY)", Group::Drop, c.rx_discards_phy),
        ("rxOutOfBuffer".into(), "Out of buffer", Group::Drop, c.rx_out_of_buffer),
        ("portXmitDiscards".into(), "Transmit discards", Group::Drop, c.port_xmit_discards),
        ("ecnMarked".into(), "ECN marked", Group::Congestion, c.ecn_marked),
        ("cnpSent".into(), "CNPs sent", Group::Congestion, c.cnp_sent),
        ("cnpHandled".into(), "CNPs handled", Group::Congestion, c.cnp_handled),
        ("cnpIgnored".into(), "CNPs ignored", Group::Congestion, c.cnp_ignored),
        ("txPause".into(), "Pause sent", Group::Pause, sum(&c.tx_pause).or(c.tx_pause_ctrl)),
        ("rxPause".into(), "Pause received", Group::Pause, sum(&c.rx_pause).or(c.rx_pause_ctrl)),
        ("linkDowned".into(), "Link downs", Group::Link, c.link_downed),
    ];
    const BYTES: [&str; 8] = ["Traffic, prio 0", "Traffic, prio 1", "Traffic, prio 2", "Traffic, prio 3", "Traffic, prio 4", "Traffic, prio 5", "Traffic, prio 6", "Traffic, prio 7"];
    for (p, bytes) in c.rx_prio_bytes.iter().enumerate() {
        v.push((format!("rxPrioBytes{p}"), BYTES[p], Group::Link, *bytes));
    }
    for (p, (tx, rx)) in c.tx_pause.iter().zip(&c.rx_pause).enumerate() {
        if tx.is_some_and(|v| v > 0) {
            v.push((format!("txPause{p}"), PRIO_TX[p], Group::Pause, *tx));
        }
        if rx.is_some_and(|v| v > 0) {
            v.push((format!("rxPause{p}"), PRIO_RX[p], Group::Pause, *rx));
        }
    }
    v
}

const PRIO_TX: [&str; 8] = ["Pause sent, prio 0", "Pause sent, prio 1", "Pause sent, prio 2", "Pause sent, prio 3", "Pause sent, prio 4", "Pause sent, prio 5", "Pause sent, prio 6", "Pause sent, prio 7"];
const PRIO_RX: [&str; 8] = [
    "Pause received, prio 0",
    "Pause received, prio 1",
    "Pause received, prio 2",
    "Pause received, prio 3",
    "Pause received, prio 4",
    "Pause received, prio 5",
    "Pause received, prio 6",
    "Pause received, prio 7",
];

fn sum(v: &[Option<u64>; 8]) -> Option<u64> {
    v.iter().flatten().copied().reduce(|a, b| a + b)
}

/// "qsfp56-dd-2-1" → "2-1".
pub fn short_port(name: &str) -> String {
    name.strip_prefix("qsfp56-dd-").unwrap_or(name).to_string()
}

/// "Half 1" for an interface on 192.168.100.x, "Half 2" on 192.168.101.x.
pub fn half_of(n: &NetStats, fabric: &[&NetStats]) -> u8 {
    if let Some(ip) = &n.ip {
        if ip.starts_with("192.168.101.") {
            return 1;
        }
        if ip.starts_with("192.168.100.") {
            return 0;
        }
    }
    // Otherwise by order among the node's addressed fabric interfaces.
    fabric.iter().position(|x| x.iface == n.iface).map(|i| (i % 2) as u8).unwrap_or(0)
}

/// A rate for display: "112/s", "50k/s", "0.004/s".
pub fn fmt_rate(v: f64) -> String {
    rates::fmt_per_s(v)
}

fn fmt_ago(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

fn fmt_span(ms: u64) -> String {
    let m = ms / 60_000;
    if m >= 60 {
        "the last hour".into()
    } else if m >= 1 {
        format!("the last {m} min")
    } else {
        format!("the last {} s", (ms / 1000).max(1))
    }
}

impl FabricMonitor {
    pub fn new(cabling_path: Option<PathBuf>) -> FabricMonitor {
        let live_cabling = cabling_path.as_deref().map(crate::util::load_json_or_default::<Cabling>).unwrap_or_default();
        FabricMonitor { cabling_path, inner: Mutex::new(Inner { live_cabling, ..Default::default() }) }
    }

    pub fn state(&self) -> Option<FabricState> {
        self.inner.lock().unwrap().state.clone()
    }

    /// The per-interface counter rates of the last assessment.
    pub fn rates(&self) -> Option<Arc<rates::Rates>> {
        self.inner.lock().unwrap().rates.clone()
    }

    /// Takes what every switch shows now as the cabling baseline (Demo: until the scenario changes).
    pub fn accept_cabling(&self, demo: bool, scenario: Option<String>) -> Result<usize, String> {
        let mut g = self.inner.lock().unwrap();
        let seen = g.last_seen.clone();
        let count = seen.values().map(|m| m.len()).sum();
        if count == 0 {
            return Err("No node is placed on a switch port yet: nothing to accept.".into());
        }
        if demo {
            g.demo_cabling = Some((scenario, seen));
        } else {
            g.live_cabling = seen;
            if let Some(p) = &self.cabling_path {
                crate::util::save_json(p, &g.live_cabling)?;
            }
        }
        Ok(count)
    }

    /// Assesses one snapshot: fills in the switch-port mapping and cabling on `snap`,
    /// updates the counter history and returns the fabric's state. `tables` are each
    /// RouterOS switch's host and neighbor tables; `world` is Demo's own cabling.
    pub fn assess(&self, snap: &mut ClusterSnapshot, tables: &BTreeMap<String, PortTables>, world: Option<&Cabling>) -> FabricState {
        let now = snap.ts_ms;
        let mut g = self.inner.lock().unwrap();
        let g = &mut *g;
        // Time went backwards (a new Demo world, a clock change), or another Demo scenario: start over.
        let scenario_changed = snap.demo && g.last_scenario.as_ref().is_some_and(|s| *s != snap.scenario);
        g.last_scenario = snap.demo.then(|| snap.scenario.clone());
        if g.last_ts.is_some_and(|t| now + 5_000 < t) || scenario_changed {
            g.ifaces.clear();
            g.groups.clear();
            g.first_ts = None;
        }
        let first = *g.first_ts.get_or_insert(now);
        g.last_ts = Some(now);

        // 1. Who is on which port, by MAC, LLDP or comment.
        let mut placed: BTreeMap<(String, String), (String, String, String)> = BTreeMap::new();
        g.last_seen.clear();
        for sw in snap.switches.iter_mut().filter(|s| s.kind == SwitchKind::RouterOs && s.status == LinkStatus::Online) {
            let Some(t) = tables.get(&sw.id) else { continue };
            let seen = mapping::apply(&mut sw.ports, t, &snap.nodes);
            for p in sw.ports.iter_mut() {
                let key = format!("{}/{}", sw.id, p.name);
                match (&p.node, p.running) {
                    (Some(n), true) => {
                        g.last_on_port.insert(key, (n.clone(), p.mapped_by.clone().unwrap_or_default()));
                    }
                    (None, false) => {
                        if let Some((n, by)) = g.last_on_port.get(&key) {
                            p.node = Some(n.clone());
                            p.mapped_by = Some(by.clone()).filter(|b| !b.is_empty());
                        }
                    }
                    _ => {}
                }
            }
            let entry = g.last_seen.entry(sw.id.clone()).or_default();
            for s in seen {
                entry.insert(format!("{}/{}", s.node, s.iface), s.port.clone());
                placed.insert((s.node.clone(), s.iface.clone()), (sw.id.clone(), s.port.clone(), s.by.to_string()));
            }
        }
        // The ports SparkBox's own path to each switch runs through.
        let path_ips: Vec<String> =
            snap.switches.iter().filter(|s| matches!(s.kind, SwitchKind::Gateway | SwitchKind::Firewalla)).map(|s| s.host.trim().to_string()).filter(|h| !h.is_empty()).collect();
        let none = PortTables::default();
        for sw in snap.switches.iter_mut().filter(|s| s.kind == SwitchKind::RouterOs && s.configured) {
            if sw.status == LinkStatus::Online {
                if let Some(t) = tables.get(&sw.id) {
                    g.last_tables.insert(sw.id.clone(), t.clone());
                }
            }
            let t = g.last_tables.get(&sw.id).unwrap_or(&none);
            let mgmt = mapping::management_ports(&sw.ports, t, &sw.host, &path_ips);
            for p in sw.ports.iter_mut() {
                p.management = mgmt.iter().find(|(n, _)| *n == p.name).map(|(_, why)| why.clone());
            }
        }
        for n in snap.nodes.iter_mut() {
            for x in n.nets.iter_mut().filter(|x| x.role == NetRole::Fabric) {
                if let Some((sw, port, _)) = placed.get(&(n.id.clone(), x.iface.clone())) {
                    x.switch_port = Some(format!("{sw}/{port}"));
                }
            }
        }

        // 2. Cabling against the baseline.
        let baseline: Cabling = if snap.demo {
            let scenario = snap.scenario.clone();
            match &g.demo_cabling {
                Some((s, c)) if *s == scenario => c.clone(),
                _ => {
                    let c = world.cloned().unwrap_or_default();
                    g.demo_cabling = Some((scenario, c.clone()));
                    c
                }
            }
        } else {
            // Learn on the first complete mapping: every online node's fabric interfaces that are up are placed.
            for (sw, seen) in &g.last_seen {
                if g.live_cabling.contains_key(sw) || seen.is_empty() {
                    continue;
                }
                let complete = snap.nodes.iter().filter(|n| n.status == LinkStatus::Online).all(|n| {
                    n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.up && x.ip.is_some()).all(|x| placed.contains_key(&(n.id.clone(), x.iface.clone())))
                });
                if complete {
                    g.live_cabling.insert(sw.clone(), seen.clone());
                    if let Some(p) = &self.cabling_path {
                        let _ = crate::util::save_json(p, &g.live_cabling);
                    }
                }
            }
            g.live_cabling.clone()
        };
        let mut cabling = Vec::new();
        for sw in snap.switches.iter_mut().filter(|s| s.kind == SwitchKind::RouterOs) {
            let Some(base) = baseline.get(&sw.id) else { continue };
            for p in sw.ports.iter_mut() {
                let Some(node) = p.node.clone() else {
                    p.cabling_ok = None;
                    p.expected_node = None;
                    continue;
                };
                // Who the baseline puts on this port.
                let expected: Option<String> = base.iter().find(|(_, port)| **port == p.name).and_then(|(k, _)| k.split('/').next().map(String::from));
                let mine_here = base.iter().any(|(k, port)| k.starts_with(&format!("{node}/")) && *port == p.name);
                let known = base.keys().any(|k| k.starts_with(&format!("{node}/")));
                p.cabling_ok = if mine_here { Some(true) } else if known || expected.is_some() { Some(false) } else { None };
                p.expected_node = if p.cabling_ok == Some(false) { expected } else { None };
            }
        }
        let name_of = |id: &str| snap.nodes.iter().find(|n| n.id == id).map(|n| n.name.clone()).unwrap_or_else(|| id.to_string());
        for ((node, iface), (sw, port, _)) in &placed {
            let Some(want) = baseline.get(sw).and_then(|b| b.get(&format!("{node}/{iface}"))) else { continue };
            if want != port {
                let n = snap.nodes.iter().find(|n| &n.id == node);
                let fabric: Vec<&NetStats> = n.map(|n| n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.ip.is_some()).collect()).unwrap_or_default();
                let half = n.and_then(|n| n.nets.iter().find(|x| &x.iface == iface)).map(|x| half_of(x, &fabric)).unwrap_or(0);
                cabling.push(CablingIssue {
                    node: node.clone(),
                    iface: iface.clone(),
                    half,
                    switch_id: sw.clone(),
                    port: port.clone(),
                    expected_port: want.clone(),
                    message: format!("{} half {} is on {}; expected {}", name_of(node), half + 1, short_port(port), short_port(want)),
                });
            }
        }

        // Half 1 before half 2 for each node (iface names don't sort that way: enP2… < enp1…).
        cabling.sort_by(|a, b| (&a.node, a.half).cmp(&(&b.node, b.half)));

        // 3. Counters, rates and a verdict per cabled fabric interface.
        let mut ifaces = Vec::new();
        let mut table = rates::Rates { ts_ms: now, ifaces: BTreeMap::new() };
        for n in &snap.nodes {
            let online = n.status == LinkStatus::Online && !n.stale;
            let fabric: Vec<&NetStats> = n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.ip.is_some()).collect();
            for x in &fabric {
                let key = format!("{}/{}", n.id, x.iface);
                let track = g.ifaces.entry(key.clone()).or_default();
                if online {
                    if let Some(c) = &x.roce {
                        record(track, now, c.clone());
                    }
                }
                let half = half_of(x, &fabric);
                let (sw, port, by) = placed.get(&(n.id.clone(), x.iface.clone())).cloned().map(|(a, b, c)| (Some(a), Some(b), Some(c))).unwrap_or((None, None, None));
                let expected = sw.as_ref().and_then(|s| baseline.get(s)).and_then(|b| b.get(&key)).cloned();
                let cabling_ok = match (&port, &expected) {
                    (Some(p), Some(e)) => Some(p == e),
                    _ => None,
                };
                let rdma = x.rdma_rx_bps.unwrap_or(0.0) + x.rdma_tx_bps.unwrap_or(0.0);
                let ip = x.rx_bps + x.tx_bps;
                let share = (x.rdma_rx_bps.is_some() && rdma + ip > 1e6).then(|| rdma / (rdma + ip));
                let per_counter = match (online, x.roce.as_ref()) {
                    (true, Some(_)) => rates::counters(&track.samples, now),
                    _ => BTreeMap::new(),
                };
                let counters: Vec<CounterReading> = match (online, x.roce.as_ref()) {
                    (true, Some(c)) => readings(c)
                        .into_iter()
                        .map(|(key, label, group, total)| {
                            let per_s = per_counter.get(&key).and_then(|r| r.r10);
                            CounterReading { per_s, key, label: label.into(), total, group }
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                table.ifaces.insert(
                    key.clone(),
                    rates::IfaceRates {
                        node: n.id.clone(),
                        node_name: n.name.clone(),
                        iface: x.iface.clone(),
                        half,
                        up: x.up,
                        switch_port: sw.as_ref().zip(port.as_ref()).map(|(s, p)| format!("{s}/{p}")).or_else(|| x.switch_port.clone()),
                        counters: per_counter,
                        pause_storm_s: track.pause_high_since.map(|since| now.saturating_sub(since) / 1000),
                    },
                );
                let verdict = verdict(n, x, half, port.as_deref(), &counters, track, now, first, online);
                ifaces.push(IfaceHealth {
                    node: n.id.clone(),
                    node_name: n.name.clone(),
                    iface: x.iface.clone(),
                    half,
                    up: x.up,
                    speed_mbps: x.speed_mbps,
                    mtu: x.mtu,
                    ip: x.ip.clone(),
                    rdma_dev: x.rdma_dev.clone(),
                    rx_bps: x.rx_bps,
                    tx_bps: x.tx_bps,
                    rdma_rx_bps: x.rdma_rx_bps,
                    rdma_tx_bps: x.rdma_tx_bps,
                    rdma_share: share,
                    switch_port: port,
                    switch_id: sw,
                    mapped_by: by,
                    cabling_ok,
                    expected_port: expected.filter(|_| cabling_ok == Some(false)),
                    verdict,
                    counters,
                    loss_last_hour: track.losses.iter().map(|(_, n)| n).sum(),
                    last_loss_ms: track.losses.back().map(|(t, _)| *t),
                });
            }
        }
        // Forget interfaces that left the inventory.
        let live: std::collections::HashSet<String> = ifaces.iter().map(|i| format!("{}/{}", i.node, i.iface)).collect();
        g.ifaces.retain(|k, _| live.contains(k));

        // 4. NCCL transport per multi-node serving group.
        let transport = transport_checks(&snap.nodes, &mut g.groups, now);

        let fabric_switch = snap.switches.iter().find(|s| s.kind == SwitchKind::RouterOs).map(|s| s.name.clone());
        let state = summarize(now, ifaces, transport, cabling, !baseline.is_empty(), &snap.nodes, fabric_switch.as_deref());
        g.state = Some(state.clone());
        g.rates = Some(Arc::new(table));
        state
    }
}

fn record(t: &mut IfaceTrack, now: u64, c: RoceCounters) {
    // A counter that went down means the driver reloaded or the node rebooted: start over.
    if let Some(last) = t.samples.back() {
        let went_down = readings(&last.c).iter().zip(readings(&c)).any(|(a, b)| matches!((a.3, b.3), (Some(x), Some(y)) if y < x));
        if went_down {
            t.samples.clear();
            t.pause_high_since = None;
        }
    }
    if let Some(last) = t.samples.back() {
        let counted: Vec<&str> = crate::health::roce_rules::group_keys("loss").into_iter().chain(crate::health::roce_rules::group_keys("phy")).collect();
        let loss = |c: &RoceCounters| readings(c).into_iter().filter(|r| counted.contains(&r.0.as_str())).filter_map(|r| r.3).sum::<u64>();
        let d = loss(&c).saturating_sub(loss(&last.c));
        if d > 0 {
            t.losses.push_back((now, d));
        }
        let dt = now.saturating_sub(last.ts) as f64 / 1000.0;
        if dt > 0.0 {
            let p = |c: &RoceCounters| sum(&c.tx_pause).or(c.tx_pause_ctrl).unwrap_or(0) + sum(&c.rx_pause).or(c.rx_pause_ctrl).unwrap_or(0);
            let per_s = p(&c).saturating_sub(p(&last.c)) as f64 / dt;
            if per_s > PAUSE_STORM_PER_S {
                t.pause_high_since.get_or_insert(last.ts);
            } else {
                t.pause_high_since = None;
            }
        }
    }
    t.samples.push_back(Sample { ts: now, c });
    while t.samples.front().is_some_and(|s| now.saturating_sub(s.ts) > KEEP_MS) {
        t.samples.pop_front();
    }
    while t.losses.front().is_some_and(|(ts, _)| now.saturating_sub(*ts) > LOSS_MEMORY_MS) {
        t.losses.pop_front();
    }
}

/// The loss, PHY or drop counters that a verdict and the roce-iface alert both count: the
/// counters in health/fixtures/roce-iface.json for that group.
fn counted_rate(counters: &[CounterReading], group: &str) -> f64 {
    let keys = crate::health::roce_rules::group_keys(group);
    counters.iter().filter(|c| keys.contains(&c.key.as_str())).filter_map(|c| c.per_s).sum()
}

fn rate_of(counters: &[CounterReading], key: &str) -> f64 {
    counters.iter().find(|c| c.key == key).and_then(|c| c.per_s).unwrap_or(0.0)
}

/// "CRC errors 12/s, symbol errors 4/s": the rising counters of a group, biggest first.
fn rising(counters: &[CounterReading], g: Group, skip: &[&str]) -> String {
    let mut v: Vec<&CounterReading> = counters.iter().filter(|c| c.group == g && !skip.contains(&c.key.as_str()) && c.per_s.is_some_and(|r| r > 0.0)).collect();
    v.sort_by(|a, b| b.per_s.partial_cmp(&a.per_s).unwrap_or(std::cmp::Ordering::Equal));
    v.iter().take(3).map(|c| format!("{} {}", lower_first(&c.label), fmt_rate(c.per_s.unwrap_or(0.0)))).collect::<Vec<_>>().join(", ")
}

/// "Out of sequence" → "out of sequence", but "CRC errors" and "ACK timeouts" keep their capitals.
fn lower_first(label: &str) -> String {
    let first = label.split_whitespace().next().unwrap_or("");
    if first.chars().skip(1).any(|c| c.is_ascii_uppercase()) {
        return label.to_string();
    }
    let mut c = label.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn verdict(n: &NodeSnapshot, x: &NetStats, half: u8, port: Option<&str>, counters: &[CounterReading], t: &IfaceTrack, now: u64, first: u64, online: bool) -> Verdict {
    let v = |kind, level, title: &str, detail: String, action: Option<String>| Verdict { kind, level, title: title.into(), detail, action };
    let where_ = format!("{} half {}", n.name, half + 1);
    let cable = match port {
        Some(p) => format!("Check the optic or cable between {}'s port 2 and switch port {}.", n.name, short_port(p)),
        None => format!("Check the optic or cable on {}'s port 2.", n.name),
    };
    if !online {
        return v(VerdictKind::NoData, Level::Info, "No data", format!("{} isn't reporting.", n.name), None);
    }
    if !x.up {
        return v(VerdictKind::Down, Level::Crit, "Link down", format!("{where_} has no carrier."), Some(cable));
    }
    if counters.is_empty() {
        return v(VerdictKind::NoData, Level::Info, "No RoCE counters", format!("{where_} reports bytes only (no RDMA counters from this node)."), None);
    }
    let phy = counted_rate(counters, "phy");
    let loss = counted_rate(counters, "loss");
    if loss > 0.0 {
        // Retransmits are the headline; rising CRC or symbol errors say why.
        let (detail, action) = if phy > 0.0 {
            (format!("{}; {} point at the optic or cable.", capitalize(&rising(counters, Group::Loss, &[])), rising(counters, Group::Phy, &[])), cable)
        } else {
            (
                format!("{}.", capitalize(&rising(counters, Group::Loss, &[]))),
                "Packets are being lost on this path: look for PHY errors on the switch port, a pause storm elsewhere, or PFC not enabled for RoCE's priority.".into(),
            )
        };
        return v(VerdictKind::Lossy, Level::Crit, "RoCE is retransmitting", detail, Some(action));
    }
    if phy > 0.0 {
        return v(VerdictKind::Phy, Level::Crit, "PHY errors", format!("{}.", capitalize(&rising(counters, Group::Phy, &[]))), Some(cable));
    }
    if let Some(since) = t.pause_high_since {
        if now.saturating_sub(since) >= PAUSE_STORM_MS {
            let tx = rate_of(counters, "txPause");
            let (dir, r) = if tx >= rate_of(counters, "rxPause") { ("sends", tx) } else { ("receives", rate_of(counters, "rxPause")) };
            return v(
                VerdictKind::PauseStorm,
                Level::Crit,
                "Pause storm",
                format!("{where_} {dir} {} pause frames for {} s.", fmt_rate(r), now.saturating_sub(since) / 1000),
                Some(if dir == "sends" {
                    format!("{} can't drain its receive queue and is stopping the whole priority. Look for a stuck process or out-of-buffer drops on {}.", n.name, n.name)
                } else {
                    "Something downstream is stopping this port. Find the node that sends the pause frames.".into()
                }),
            );
        }
    }
    let drop = counted_rate(counters, "drop");
    if drop > 0.0 {
        return v(
            VerdictKind::Dropping,
            Level::Warn,
            "Dropping packets",
            format!("{}.", capitalize(&rising(counters, Group::Drop, &[]))),
            Some("The receive buffer overflows. With PFC on RoCE's priority this shouldn't happen; check mlnx_qos.".into()),
        );
    }
    let ecn = rate_of(counters, "ecnMarked").max(rate_of(counters, "cnpHandled"));
    if ecn > CONGESTED_ECN_PER_S {
        return v(
            VerdictKind::Congested,
            Level::Info,
            "Congested",
            "DCQCN is pacing senders; expected during all-reduce bursts.".into(),
            Some(format!("ECN marks {}, nothing lost.", fmt_rate(rate_of(counters, "ecnMarked")))),
        );
    }
    let protection = x.roce.as_ref().map(|r| pfc_protection(r, counters));
    if protection.as_ref().is_some_and(|(protected, _)| *protected == Some(false)) {
        return v(VerdictKind::NotLossless, Level::Info, "RoCE priority unprotected",
            format!("{where_}: {} Node side only; switch QoS has not been verified.", protection.as_ref().unwrap().1), Some(PFC_GUIDANCE.into()));
    }
    let lost: u64 = t.losses.iter().map(|(_, n)| n).sum();
    if let Some((last, _)) = t.losses.back() {
        // History, not a fault: nothing is being lost now, so it's information (and no alert).
        return v(
            VerdictKind::WasLossy,
            Level::Info,
            "Lost packets earlier",
            format!("{lost} loss or PHY errors in the last hour, the last {}.", fmt_ago(now.saturating_sub(*last))),
            None,
        );
    }
    if !protection.as_ref().is_some_and(|(protected, _)| *protected == Some(true)) {
        return v(VerdictKind::NotLossless, Level::Info, "PFC protection unknown",
            format!("No current loss detected. {} Node side only; switch QoS has not been verified.", protection.map(|(_, why)| why).unwrap_or_else(|| "Priority readings are unavailable.".into())), Some(PFC_GUIDANCE.into()));
    }
    v(VerdictKind::Lossless, Level::Ok, "Node-side PFC protected", format!("No drops, retransmits or pause storms in {}. PFC covers the observed traffic priority. Switch QoS has not been verified.", fmt_span(now.saturating_sub(first))), None)
}

const PFC_GUIDANCE: &str = "For the planned DSCP configuration, verify all three together: node trust dscp with PFC on priority 3; RoCE ToS 106 (NCCL_IB_TC=106); and switch DSCP 26 mapped to priority 3 with PFC. Global pause is separate. SparkBox does not apply these settings.";

fn pfc_protection(r: &RoceCounters, counters: &[CounterReading]) -> (Option<bool>, String) {
    let Some(enabled) = &r.pfc_prios else { return (None, "PFC configuration was not read.".into()) };
    if enabled.is_empty() { return (Some(false), "No PFC priority is enabled.".into()) }
    if !matches!(r.trust.as_deref(), Some("pcp" | "dscp")) { return (None, "The priority trust mode is unknown.".into()) }
    // Missing/reset counters cannot prove which priority carries traffic. Several active
    // priorities do not identify RoCE unambiguously, so keep the verdict unknown.
    let rates: Option<Vec<f64>> = (0..8).map(|p| counters.iter().find(|c| c.key == format!("rxPrioBytes{p}")).and_then(|c| c.per_s)).collect();
    let Some(rates) = rates else { return (None, "Traffic priority has not been measured.".into()) };
    let active: Vec<u8> = rates.iter().enumerate().filter(|(_, r)| **r > 0.0).map(|(p, _)| p as u8).collect();
    if active.len() != 1 { return (None, "No single active fabric traffic priority was identified.".into()) }
    let p = active[0];
    let protected = enabled.contains(&p);
    (Some(protected), format!("Traffic uses priority {p}; PFC is {} for it (trust {}).", if protected { "enabled" } else { "disabled" }, r.trust.as_deref().unwrap_or("unknown")))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Multi-node serving groups: engine containers (or any carrying NCCL settings), grouped
/// by SparkBox deployment label or Compose project, on two or more nodes. A SparkBox load has no
/// `-e` and runs its image by id, so its sparkbox.path label is what says it's an engine.
/// Each group's members: (node id, the NCCL settings its container carries).
type Groups = BTreeMap<String, Vec<(String, BTreeMap<String, String>)>>;

fn serving_groups(nodes: &[NodeSnapshot]) -> Groups {
    let engine = |c: &crate::model::Container| {
        let i = c.image.to_ascii_lowercase();
        c.labels.contains_key("sparkbox.path") || ["vllm", "sglang", "tensorrt", "trtllm", "nim", "ray"].iter().any(|e| i.contains(e))
    };
    let mut groups = Groups::new();
    for n in nodes {
        for c in n.containers.iter().filter(|c| c.state == "running" && (!c.env_flags.is_empty() || engine(c))) {
            let key = c.labels.get("sparkbox.deployment").or_else(|| c.labels.get("com.docker.compose.project")).cloned().unwrap_or_else(|| c.name.clone());
            let g = groups.entry(key).or_default();
            if !g.iter().any(|(id, _)| *id == n.id) {
                g.push((n.id.clone(), c.env_flags.clone()));
            }
        }
    }
    groups.retain(|_, members| members.len() >= 2);
    groups
}

fn transport_checks(nodes: &[NodeSnapshot], tracks: &mut HashMap<String, GroupTrack>, now: u64) -> Vec<TransportCheck> {
    let groups = serving_groups(nodes);
    tracks.retain(|k, _| groups.contains_key(k));
    let mut out = Vec::new();
    for (name, members) in groups {
        let t = tracks.entry(name.clone()).or_default();
        let ns: Vec<&NodeSnapshot> = members.iter().filter_map(|(id, _)| nodes.iter().find(|n| &n.id == id)).collect();
        // Busy: generating on every member right now (GPU working, the fabric carrying something).
        let busy = ns.iter().all(|n| n.status == LinkStatus::Online && n.gpu.util_pct > 10.0);
        if busy {
            let per = ns
                .iter()
                .map(|n| {
                    let fab = n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.up);
                    let (r, i) = fab.fold((0.0, 0.0), |(r, i), x| (r + x.rdma_rx_bps.unwrap_or(0.0) + x.rdma_tx_bps.unwrap_or(0.0), i + x.rx_bps + x.tx_bps));
                    (n.id.clone(), r, i)
                })
                .collect();
            t.busy.push_back((now, per));
        }
        while t.busy.front().is_some_and(|(ts, _)| now.saturating_sub(*ts) > 300_000) {
            t.busy.pop_front();
        }
        // Each member's RDMA share over the window; the group's is their median, so a model copied
        // over TCP from one member (Models' fabric copy) isn't taken for NCCL falling back to sockets.
        let mut totals: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
        for (_, per) in &t.busy {
            for (id, r, i) in per {
                let e = totals.entry(id.as_str()).or_default();
                (e.0, e.1) = (e.0 + r, e.1 + i);
            }
        }
        let mut shares: Vec<f64> = totals.values().filter(|(r, i)| r + i > 0.0).map(|(r, i)| r / (r + i) * 100.0).collect();
        shares.sort_by(f64::total_cmp);
        let median = match shares.len() {
            0 => None,
            k if k % 2 == 1 => Some(shares[k / 2]),
            k => Some((shares[k / 2 - 1] + shares[k / 2]) / 2.0),
        };
        let pct = median.filter(|_| t.busy.len() >= 3);

        let mut env_issues = Vec::new();
        for (id, env) in &members {
            let n = nodes.iter().find(|n| &n.id == id);
            let name = n.map(|n| n.name.as_str()).unwrap_or(id);
            if env.get("NCCL_IB_DISABLE").is_some_and(|v| v.trim() == "1") {
                env_issues.push(format!("NCCL_IB_DISABLE=1 on {name}: NCCL won't use RDMA at all."));
            }
            // A node that doesn't report its containers' NCCL settings isn't judged on them.
            if env.is_empty() {
                continue;
            }
            if !env.contains_key("NCCL_IB_HCA") {
                env_issues.push(format!("NCCL_IB_HCA isn't set on {name}, so NCCL picks the RDMA devices itself (it may pick the uncabled port)."));
            }
            if let (Some(ifname), Some(n)) = (env.get("NCCL_SOCKET_IFNAME"), n) {
                let lan = n.nets.iter().filter(|x| x.role != NetRole::Fabric).any(|x| ifname.split(',').any(|w| w.trim().trim_start_matches('=').trim_start_matches('^') == x.iface));
                if lan {
                    env_issues.push(format!("NCCL_SOCKET_IFNAME={ifname} on {name} is the LAN, not the fabric."));
                }
            }
        }
        let (level, message) = match pct {
            Some(p) if p < TCP_FALLBACK_PCT => (Level::Warn, format!("NCCL is using TCP sockets (RDMA {p:.0}%)")),
            Some(p) => (if env_issues.is_empty() { Level::Ok } else { Level::Warn }, format!("NCCL over RoCE: {p:.0}% of fabric bytes are RDMA")),
            None => (if env_issues.is_empty() { Level::Ok } else { Level::Warn }, "Not generating yet: the RDMA share is measured while it serves".into()),
        };
        let mut nodes_sorted: Vec<String> = members.into_iter().map(|(id, _)| id).collect();
        nodes_sorted.sort();
        out.push(TransportCheck { name, nodes: nodes_sorted, rdma_pct: pct, level, message, env_issues });
    }
    out
}

/// "The fabric isn't lossless": the PFC-off advisory, with what the counters say it has cost.
/// `switch` names the fabric switch ("CRS804 DDQ").
pub fn pfc_advisory(ifaces: &[IfaceHealth], nodes: &[NodeSnapshot], switch: Option<&str>) -> Option<Advisory> {
    let up: Vec<&IfaceHealth> = ifaces.iter().filter(|i| i.up && i.verdict.kind != VerdictKind::NoData && i.verdict.kind != VerdictKind::Down).collect();
    let off: Vec<&&IfaceHealth> = up.iter().filter(|i| i.verdict.kind == VerdictKind::NotLossless).collect();
    if off.is_empty() {
        return None;
    }
    let roce = |i: &IfaceHealth| nodes.iter().find(|n| n.id == i.node).and_then(|n| n.nets.iter().find(|x| x.iface == i.iface)).and_then(|x| x.roce.clone());
    let trusts: std::collections::BTreeSet<String> = off.iter().filter_map(|i| roce(i).and_then(|r| r.trust)).collect();
    let trust = if trusts.len() == 1 { format!("trust {}, ", trusts.iter().next().unwrap()) } else { String::new() };
    let ports = if off.len() == up.len() { format!("all {} fabric ports", off.len()) } else { format!("{} of {} fabric ports", off.len(), up.len()) };
    let oos: Vec<u64> = off.iter().filter_map(|i| roce(i).and_then(|r| r.out_of_sequence)).collect();
    let cost = match (oos.iter().min(), oos.iter().max()) {
        (Some(&lo), Some(&hi)) if hi > 0 && lo == hi => format!("RoCE still works by resending lost packets: since they came up, each port counted {hi} out-of-sequence packets."),
        (Some(&lo), Some(&hi)) if hi > 0 => format!("RoCE still works by resending lost packets: since they came up, the ports counted {lo}–{hi} out-of-sequence packets each."),
        _ => "RoCE still works by resending lost packets; none have been lost since the ports came up.".to_string(),
    };
    let on_switch = switch.map(|s| format!(" and on the {s}")).unwrap_or_else(|| " and on the switch".into());
    Some(Advisory {
        kind: "pfcOff".into(),
        title: "Fabric PFC needs verification.".into(),
        body: format!("PFC protection is absent or unknown on {ports} ({trust}node-side evidence only). {cost} Verify the traffic priority{on_switch}. {PFC_GUIDANCE}"),
        short: "PFC protection unverified".into(),
    })
}

#[allow(clippy::too_many_arguments)]
fn summarize(now: u64, ifaces: Vec<IfaceHealth>, transport: Vec<TransportCheck>, cabling: Vec<CablingIssue>, baseline_set: bool, nodes: &[NodeSnapshot], switch: Option<&str>) -> FabricState {
    let worst = ifaces.iter().max_by_key(|i| (i.verdict.level, matches!(i.verdict.kind, VerdictKind::Lossy | VerdictKind::Phy | VerdictKind::PauseStorm | VerdictKind::Congested))).cloned();
    let reporting = ifaces.iter().filter(|i| i.verdict.kind != VerdictKind::NoData).count();
    let tcp = transport.iter().find(|t| t.level == Level::Warn);
    let verdict = match &worst {
        _ if ifaces.is_empty() => Verdict { kind: VerdictKind::NoData, level: Level::Info, title: "No fabric data".into(), detail: "No node reports a fabric interface yet.".into(), action: None },
        Some(w) if w.verdict.level >= Level::Warn => {
            let same = ifaces.iter().filter(|i| i.verdict.kind == w.verdict.kind).count();
            let title = if same > 1 { format!("{} on {same} interfaces", w.verdict.title) } else { format!("{} on {} half {}", w.verdict.title, w.node_name, w.half + 1) };
            Verdict { title, ..w.verdict.clone() }
        }
        _ if !cabling.is_empty() => Verdict {
            kind: VerdictKind::Lossless,
            level: Level::Warn,
            title: "Cabling changed".into(),
            detail: cabling.iter().map(|c| c.message.clone()).collect::<Vec<_>>().join(". ") + ".",
            action: Some("Move the cables back, or accept the current cabling if it was on purpose.".into()),
        },
        _ if tcp.is_some() => {
            let t = tcp.unwrap();
            Verdict { kind: VerdictKind::Lossless, level: Level::Warn, title: t.message.clone(), detail: t.env_issues.first().cloned().unwrap_or_else(|| format!("{} isn't using RDMA.", t.name)), action: None }
        }
        Some(w) if w.verdict.kind == VerdictKind::Congested => {
            Verdict { title: format!("Congested on {} half {}", w.node_name, w.half + 1), ..w.verdict.clone() }
        }
        Some(w) if w.verdict.kind == VerdictKind::NotLossless => w.verdict.clone(),
        Some(w) => Verdict { title: "Fabric healthy".into(), ..w.verdict.clone() },
        None => Verdict { kind: VerdictKind::NoData, level: Level::Info, title: "No fabric data".into(), detail: String::new(), action: None },
    };
    let summary = if verdict.level == Level::Ok {
        let mut parts = vec!["Fabric healthy".to_string(), "node-side PFC protected".to_string()];
        // Links per node: distinct switch ports (or interfaces) with their speed.
        let up: Vec<&IfaceHealth> = ifaces.iter().filter(|i| i.up).collect();
        let speed = up.iter().map(|i| i.speed_mbps).max().unwrap_or(0);
        let per_node = nodes
            .iter()
            .map(|n| {
                let mine: Vec<&&IfaceHealth> = up.iter().filter(|i| i.node == n.id).collect();
                let ports: std::collections::BTreeSet<&str> = mine.iter().filter_map(|i| i.switch_port.as_deref()).collect();
                if ports.is_empty() { mine.len().min(1) } else { ports.len() }
            })
            .max()
            .unwrap_or(0);
        if speed > 0 && per_node > 0 {
            parts.push(if per_node == 1 { format!("{}G per node", speed / 1000) } else { format!("{per_node} × {}G per node", speed / 1000) });
        }
        if transport.iter().any(|t| t.rdma_pct.is_some()) {
            parts.push("NCCL on RoCE".into());
        }
        if reporting < ifaces.len() {
            parts.push(format!("{reporting} of {} interfaces reporting", ifaces.len()));
        }
        parts.join(" · ")
    } else if verdict.kind == VerdictKind::NotLossless && verdict.level == Level::Info {
        let up = ifaces.iter().filter(|i| i.up).count();
        format!("Fabric up · PFC protection unverified · {up} fabric ports")
    } else {
        verdict.title.clone()
    };
    let advisory = pfc_advisory(&ifaces, nodes, switch);
    FabricState { ts_ms: now, verdict, summary, ifaces, transport, cabling, baseline_set, advisory }
}

/// Values for the two coded fabric rules (health::roce_rules): "ncclTcp" per serving
/// group (entity "cluster"), "cablingMismatch" per switch port ("<switch>/<port>").
/// 1.0 is firing; each comes with the sentence the alert shows.
/// Read by the health engine for its coded fabric rules (health::mod).
pub fn coded_readings(state: &FabricState, metric: &str) -> Vec<(String, f64, String)> {
    match metric {
        "ncclTcp" => state
            .transport
            .iter()
            .map(|t| {
                let bad = t.rdma_pct.is_some_and(|p| p < TCP_FALLBACK_PCT) || !t.env_issues.is_empty();
                let why = if t.rdma_pct.is_some_and(|p| p < TCP_FALLBACK_PCT) { t.message.clone() } else { t.env_issues.first().cloned().unwrap_or_else(|| t.message.clone()) };
                ("cluster".to_string(), if bad { 1.0 } else { 0.0 }, format!("{}: {why}", t.name))
            })
            .collect(),
        "cablingMismatch" => state.cabling.iter().map(|c| (format!("{}/{}", c.switch_id, c.port), 1.0, c.message.clone())).collect(),
        _ => Vec::new(),
    }
}

/// The world's own cabling in Demo: each node's fabric halves on the port the simulator cabled it to.
pub fn demo_world_cabling(snap: &ClusterSnapshot) -> Cabling {
    let mut out: Cabling = BTreeMap::new();
    for (i, n) in snap.nodes.iter().enumerate() {
        for x in n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.ip.is_some()) {
            out.entry("crs804".into()).or_default().insert(format!("{}/{}", n.id, x.iface), crate::demo::world::crs_port(i));
        }
    }
    out
}

/// Called from `publish()`: fills in mapping and cabling on the snapshot, assesses the
/// fabric and tells the UI.
pub fn on_publish(app: &tauri::AppHandle, snap: &mut ClusterSnapshot) {
    use tauri::{Emitter, Manager};
    let Some(mon) = app.try_state::<std::sync::Arc<FabricMonitor>>() else { return };
    let mut tables = BTreeMap::new();
    for sw in snap.switches.iter().filter(|s| s.kind == SwitchKind::RouterOs) {
        let t = if snap.demo {
            app.try_state::<std::sync::Arc<Mutex<crate::demo::DemoCluster>>>().map(|d| d.lock().unwrap().switch_tables(&sw.id))
        } else {
            app.try_state::<std::sync::Arc<crate::routeros::Switches>>().and_then(|s| s.tables(&sw.id))
        };
        if let Some(t) = t {
            tables.insert(sw.id.clone(), t);
        }
    }
    let world = snap.demo.then(|| demo_world_cabling(snap));
    let state = mon.assess(snap, &tables, world.as_ref());
    let _ = app.emit(STATE_EVENT, &state);
}

#[cfg(test)]
mod priority_tests {
    use super::*;
    #[test]
    fn pfc_must_protect_the_measured_priority_and_unknown_is_not_lossless() {
        let mut r = RoceCounters { trust: Some("pcp".into()), pfc_prios: Some(vec![3]), ..Default::default() };
        let readings = |priority| (0..8).map(|p| CounterReading { key: format!("rxPrioBytes{p}"), label: String::new(), total: None, per_s: Some(if p == priority { 1_000.0 } else { 0.0 }), group: Group::Link }).collect::<Vec<_>>();
        assert_eq!(pfc_protection(&r, &readings(0)).0, Some(false));
        r.trust = Some("dscp".into());
        assert_eq!(pfc_protection(&r, &readings(3)).0, Some(true));
        assert_eq!(pfc_protection(&r, &[]).0, None);
        let mut mixed = readings(3); mixed[0].per_s = Some(1.0);
        assert_eq!(pfc_protection(&r, &mixed).0, None);
        r.pfc_prios = Some(vec![]);
        assert_eq!(pfc_protection(&r, &[]).0, Some(false));
    }
}
