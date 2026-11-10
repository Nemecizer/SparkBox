//! The alert rules: pure functions from (what was seen before, the new snapshot,
//! now, the rule set) to the conditions that hold right now. The engine turns
//! conditions into alerts with pending/firing/resolved states; rules only say
//! what is wrong and how long it must last before anyone is told.
//!
//! Every threshold and duration comes from the rule set (rule.rs), so the
//! owner can tune them in Settings › Alerts. Rules with logic of their own are
//! below; any other threshold or rate rule on a metric `reading` knows is
//! judged generically.

use std::collections::{HashMap, HashSet, VecDeque};

use super::rule::{Rule, RuleKind, RuleSet, Th};
use super::{Category, Entity, Severity};
use crate::fabric::rates::Rates;
use crate::model::{ClusterSnapshot, Container, LinkStatus, MacReachState, NetRole, NetStats, NodeSnapshot, SwitchPort, SwitchSnapshot};

/// models/plan.rs warns about a load that would cross the default memory line.
pub use super::rule::MEM_WARN_PCT;

// ---- Mechanics (not thresholds: those are in the rule set)

/// A fill may predict this many times as far ahead as it has been watched: ten minutes of trend
/// can speak for the next hour, and the 24-hour warning line needs all 4 hours.
pub const DISK_PREDICT_RATIO: u64 = 6;
/// The look-backs a fill is judged over, in minutes, shortest first.
const DISK_SPANS_MIN: [u64; 7] = [5, 10, 20, 30, 60, 120, 240];
/// A look-back this short must also have seen a quarter of what's left disappear, so a fast
/// copy that's nowhere near filling the disk can't borrow a runaway log's speed.
pub const DISK_SHORT_SPAN_MS: u64 = 60 * 60_000;
/// How far back a trend must still show, at a quarter of its rate or better: a slice of its
/// look-back, and never less than this.
pub const DISK_STILL_FILLING_MS: u64 = 3 * 60_000;
/// One disk-usage sample per this long feeds the fill-rate.
pub const DISK_SAMPLE_EVERY_MS: u64 = 10_000;
/// How much of it the fill-rate keeps, when the rule doesn't say.
const DISK_WINDOW_MS: u64 = 4 * 3_600_000;
/// Interface and switch-port errors count as rising while they grew this recently; the
/// message still says how many came in the rule's window.
pub const ERRORS_RISING_MS: u64 = 15_000;

// ---- GPU throttling (fixtures/gpu-throttle-cases.json; the node agent and the UI's
// throttleVerdict read the same file)

/// nvidia-smi clock-event reasons that are the hardware protecting itself: they always count.
pub const THROTTLE_ALWAYS: [&str; 3] = ["hw slowdown", "hw thermal slowdown", "hw power brake slowdown"];
/// Reasons that are only the driver's own policy: they count while the GPU is busy (as does any
/// reason not listed here). An idle GB10 at P8 and 4 W reports "SW power cap" every other reading.
#[cfg_attr(not(test), allow(dead_code))]
pub const THROTTLE_IF_BUSY: [&str; 3] = ["sw power cap", "sw thermal slowdown", "sync boost"];
/// Normal operation, never throttling.
pub const THROTTLE_NEVER: [&str; 4] = ["gpu idle", "applications clocks setting", "display clock setting", "none"];
/// Busy: at least this utilisation…
pub const THROTTLE_BUSY_UTIL_PCT: f32 = 50.0;
/// …with the SM clock under this share of its top clock…
pub const THROTTLE_CLOCK_BELOW: f64 = 0.9;
/// …or drawing this share of a reported power limit.
pub const THROTTLE_POWER_AT: f64 = 0.8;

// ---- The one crash-loop definition (models::endpoints reads these too).

/// Restarts within the window that make a crash loop (critical).
pub const CRASH_RESTARTS: u32 = 2;
/// The window restarts are counted over.
pub const CRASH_WINDOW_S: u64 = 600;
/// Running this long without a restart, it has recovered (whatever the window still counts).
pub const CRASH_RECENT_S: u64 = 60;

/// Rules with logic of their own in this file. Any other rule is judged generically.
pub const CODED: [&str; 37] = [
    "node-offline", "node-reboot", "node-stale", "mem-high", "ntp-unsynced", "kernel-oom", "kernel-hung", "agent-silent", "disk-full",
    "disk-eta", "gpu-hot", "gpu-throttle", "kernel-xid", "fabric-down", "fabric-flap", "net-errors", "fabric-mtu", "kernel-mlx5",
    "switch-unreachable", "port-down", "port-errors", "optic-rx-low", "crash-loop", "container-exited", "unit-failed", "endpoint-down",
    "endpoint-idle", "roce-iface", "gpu-unreadable", "agent-config-stale", "switch-health", "mac-reach", "node-needs-you",
    "node-rebooted", "security-updates-pending", "restart-pending", "package-lists-stale",
];

// ---- Updates (P1.21), judged on the update check's facts (ops/update_check.rs)

/// Security updates pending this long are a warning, not only information.
pub const SECURITY_WAIT_MS: u64 = 7 * DAY_MS;
/// A restart waiting this long is a warning.
pub const RESTART_WAIT_MS: u64 = 3 * DAY_MS;
/// Package lists older than this: apt-daily isn't refreshing them.
pub const LISTS_STALE_MS: u64 = 48 * 3_600_000;
const DAY_MS: u64 = 86_400_000;

/// The SSH connect limit (live/ssh.rs): a timeout is "No SSH reply in 10 s".
pub const SSH_REPLY_S: u64 = 10;
/// Failure kinds where the node answered and turned SparkBox away: it needs the owner, it isn't down.
pub const NEEDS_YOU_KINDS: [&str; 2] = ["hostKeyUnknown", "authFailed"];

/// Rules whose alert resolves ("No data") when its readings stop coming, instead of being
/// held while the node isn't reporting: a rate nobody can measure isn't a rate that's high.
pub const RESOLVE_WITHOUT_DATA: [&str; 1] = ["roce-iface"];

/// Fatal Xids: the GPU needs a reset, so the alert stays until the node's boot_id changes.
pub fn xid_latches(code: u32) -> bool {
    super::kernel::xid(code).is_some_and(|x| x.latch)
}

/// One thing that's wrong right now.
#[derive(Clone, Debug)]
pub struct Condition {
    pub key: String,
    pub rule: String,
    pub category: Category,
    pub severity: Severity,
    pub entity: Entity,
    /// Reads after the entity, e.g. "Root disk 86% full".
    pub message: String,
    /// Stands alone, e.g. "gx10-0000 disk 86%".
    pub summary: String,
    pub detail: Option<String>,
    pub value: Option<f64>,
    /// How long it must hold before the alert fires.
    pub for_ms: u64,
    /// How long it must be gone before the alert resolves.
    pub clear_ms: u64,
    /// False for information that's shown but never notified.
    pub notify: bool,
    /// What it means in a few words ("check the optic or cable").
    pub cause: Option<String>,
    /// The short label when it isn't the rule's own ("Restarting" for a planned node-offline).
    pub short: Option<String>,
}

/// What the rules remember between snapshots: counters, when they last moved, and a
/// short disk-usage history for the fill-rate.
#[derive(Default)]
pub struct Trackers {
    disk: HashMap<String, VecDeque<(u64, f64, f64)>>,
    ifaces: HashMap<String, IfaceTrack>,
    containers: HashMap<String, ContainerTrack>,
    ports: HashMap<String, PortTrack>,
    /// Counters behind generic rate rules: "<rule>|<entity key>" → (ms, value).
    counters: HashMap<String, VecDeque<(u64, f64)>>,
    /// When each endpoint last generated tokens or held a request (or was first seen).
    endpoint_active: HashMap<String, u64>,
    /// Fatal Xids seen on each node, held until its boot_id changes: node → (boot_id, events).
    xid_latch: HashMap<String, (Option<String>, Vec<crate::model::KernelEvent>)>,
    // ── r1-h ──
    /// Each node's last boot_id, and when a new one was first seen (it rebooted). In memory.
    boots: HashMap<String, (String, Option<u64>)>,
}

#[derive(Default)]
struct IfaceTrack {
    carrier: Option<u64>,
    /// Carrier changes seen and not yet counted as a flap: a drop is two (down, then up).
    carrier_open: u64,
    flaps: VecDeque<u64>,
    errors: Option<u64>,
    errors_rose_ms: Option<u64>,
    errors_delta: u64,
    known_ip: Option<String>,
}

#[derive(Default)]
struct ContainerTrack {
    restarts: Option<u32>,
    restarted: VecDeque<u64>,
}

#[derive(Default)]
struct PortTrack {
    errors: Option<u64>,
    errors_rose_ms: Option<u64>,
    errors_delta: u64,
}

/// A node whose numbers are current (online and not stale).
pub fn reporting(n: &NodeSnapshot) -> bool {
    n.status == LinkStatus::Online && !n.stale
}

pub fn root_disk(n: &NodeSnapshot) -> Option<&crate::model::DiskStats> {
    n.disks.iter().find(|d| d.mount == "/").or(n.disks.first())
}

fn pct(part: f64, whole: f64) -> f64 {
    if whole > 0.0 {
        part / whole * 100.0
    } else {
        0.0
    }
}

/// Percent thresholds are judged on the whole percent the UI shows, so a disk that reads
/// "85%" is at the 85% line even when it's 84.9%.
fn shown(p: f64) -> f64 {
    p.round()
}

/// "~40 min", "~6 h".
pub fn eta_text(s: f64) -> String {
    if s < 3600.0 {
        format!("~{} min", ((s / 60.0).round() as u64).max(1))
    } else {
        format!("~{} h", (s / 3600.0).round() as u64)
    }
}

/// Disk usage at time `t`: the last sample at or before it (the first, for an earlier `t`).
fn used_at(samples: &VecDeque<(u64, f64, f64)>, t: u64) -> Option<f64> {
    samples.get(samples.partition_point(|s| s.0 <= t).saturating_sub(1)).map(|s| s.1)
}

/// The rate (bytes per ms) the disk held through the last `span`: the third-lowest of six equal
/// slices, so it filled at least this fast for four of the six and one burst can't set it.
/// None until the samples reach that far back.
fn held_rate(samples: &VecDeque<(u64, f64, f64)>, span: u64) -> Option<f64> {
    let start = samples.back()?.0.checked_sub(span)?;
    if samples.front()?.0 > start + 2 * DISK_SAMPLE_EVERY_MS {
        return None;
    }
    let slice = span / 6;
    let mut rates = (0..6).map(|i| Some((used_at(samples, start + (i + 1) * slice)? - used_at(samples, start + i * slice)?) / slice as f64)).collect::<Option<Vec<f64>>>()?;
    rates.sort_by(f64::total_cmp);
    Some(rates[2])
}

/// The disk gained, over the last `look` ms, at a quarter of `rate` or better.
fn still_filling(samples: &VecDeque<(u64, f64, f64)>, rate: f64, look: u64) -> bool {
    let Some(last) = samples.back() else { return false };
    let since = last.0.saturating_sub(look);
    used_at(samples, since).is_some_and(|before| (last.1 - before) / look as f64 >= rate / 4.0)
}

/// How fast (bytes per ms) a disk with `free` bytes left is filling, when that's a trend worth
/// projecting. A download that ends isn't one, so this needs a rate the disk held (`held_rate`),
/// watched for at least a sixth of the time it predicts, and still showing now. There's no free
/// pass for the longest look-back: at 4 hours it can speak for the 24-hour warning line and no
/// further. A look-back of an hour or less must also have seen a quarter of the free space go.
/// The shortest that qualifies wins, so a runaway log is caught in minutes, while a download or a
/// copy that's over in an hour says nothing on a disk with terabytes free. A writer that pauses
/// for much of a slice can drop out and come back, so a checkpoint every 40 to 60 minutes may flap.
#[cfg(test)]
pub fn fill_rate(samples: &VecDeque<(u64, f64, f64)>, free: f64, window_ms: u64) -> Option<f64> {
    fill_rate_to(samples, free, window_ms, 0)
}

/// `fill_rate`, with the longest look-back reaching as far as `lines_ms`, the rule's own warning
/// and clear lines: the owner can move those past 24 hours, and "clears above 30 h" must hold.
pub fn fill_rate_to(samples: &VecDeque<(u64, f64, f64)>, free: f64, window_ms: u64, lines_ms: u64) -> Option<f64> {
    let last = samples.back()?;
    let longest = DISK_SPANS_MIN.map(|m| m * 60_000).into_iter().filter(|s| *s <= window_ms).max()?;
    for span in DISK_SPANS_MIN.map(|m| m * 60_000).into_iter().filter(|s| *s <= window_ms) {
        let Some(rate) = held_rate(samples, span).filter(|r| *r > 0.0) else { continue };
        let reach = if span == longest { (span * DISK_PREDICT_RATIO).max(lines_ms) } else { span * DISK_PREDICT_RATIO };
        if (reach as f64) < free / rate {
            continue;
        }
        if span <= DISK_SHORT_SPAN_MS && last.1 - used_at(samples, last.0.saturating_sub(span))? < free / 4.0 {
            continue;
        }
        return still_filling(samples, rate, (span / 6).max(DISK_STILL_FILLING_MS)).then_some(rate);
    }
    None
}

fn prune(q: &mut VecDeque<u64>, now: u64, window: u64) {
    while q.front().is_some_and(|f| now.saturating_sub(*f) > window) {
        q.pop_front();
    }
}

impl Trackers {
    /// Whether `node` came back with a new boot_id at or after `since` (it rebooted while away).
    pub fn rebooted_since(&self, node: &str, since: u64) -> bool {
        self.boots.get(node).and_then(|b| b.1).is_some_and(|t| t >= since)
    }

    /// Forgets a container's restarts (a verified restart: only new ones count from here).
    pub fn forget_restarts(&mut self, node: &str, container: &str) {
        if let Some(t) = self.containers.get_mut(&format!("{node}/{container}")) {
            t.restarted.clear();
        }
    }

    /// Takes in the counters of a new snapshot. Only reporting nodes and reachable
    /// switches update, so a stale frame can't look like a counter reset.
    pub fn observe(&mut self, snap: &ClusterSnapshot, endpoints: &[EndpointInfo], rules: &RuleSet, now: u64) {
        let window = |id: &str, fallback: u64| Some(rules.th(id, None).window_ms).filter(|w| *w > 0).unwrap_or(fallback);
        let (disk_window, flap_window) = (window("disk-eta", DISK_WINDOW_MS), window("fabric-flap", 300_000));
        let (errors_window, restart_window, port_window) = (window("net-errors", 120_000), window("crash-loop", 600_000), window("port-errors", 120_000));
        for n in snap.nodes.iter().filter(|n| reporting(n)) {
            // A fatal Xid stays latched until the node boots again (a new boot_id).
            let latch = self.xid_latch.entry(n.id.clone()).or_default();
            if n.boot_id.is_some() && latch.0.is_some() && latch.0 != n.boot_id {
                latch.1.clear();
            }
            if n.boot_id.is_some() {
                latch.0.clone_from(&n.boot_id);
            }
            // A new boot_id is a reboot; the first one seen is only where SparkBox came in.
            if let Some(b) = &n.boot_id {
                match self.boots.get_mut(&n.id) {
                    Some((last, changed)) if last != b => (*last, *changed) = (b.clone(), Some(now)),
                    Some(_) => {}
                    None => {
                        self.boots.insert(n.id.clone(), (b.clone(), None));
                    }
                }
            }
            for e in n.kernel_events.iter().filter(|e| e.kind == "xid" && e.code.is_some_and(xid_latches)) {
                if !latch.1.iter().any(|x| x.ts_ms == e.ts_ms && x.code == e.code) {
                    latch.1.push(e.clone());
                }
            }
            if let Some(d) = root_disk(n) {
                let q = self.disk.entry(n.id.clone()).or_default();
                if q.back().is_none_or(|b| now.saturating_sub(b.0) >= DISK_SAMPLE_EVERY_MS) {
                    q.push_back((now, d.used as f64, d.total as f64));
                }
                while q.front().is_some_and(|f| now.saturating_sub(f.0) > disk_window) {
                    q.pop_front();
                }
            }
            for x in &n.nets {
                let t = self.ifaces.entry(format!("{}/{}", n.id, x.iface)).or_default();
                if x.ip.is_some() {
                    t.known_ip = x.ip.clone();
                }
                if let Some(c) = x.carrier_changes {
                    // One drop is one flap: the kernel counts its going down and its coming back.
                    // Counted once the link is up again, on this interface only.
                    if let Some(prev) = t.carrier.filter(|p| c > *p) {
                        t.carrier_open += c - prev;
                    }
                    if x.up && t.carrier_open > 0 {
                        for _ in 0..t.carrier_open.div_ceil(2) {
                            t.flaps.push_back(now);
                        }
                        t.carrier_open = 0;
                    }
                    t.carrier = Some(c);
                }
                prune(&mut t.flaps, now, flap_window);
                let errors = x.rx_errors + x.tx_errors;
                if let Some(prev) = t.errors {
                    if errors > prev {
                        t.errors_rose_ms = Some(now);
                        t.errors_delta += errors - prev;
                    }
                }
                if t.errors_rose_ms.is_none_or(|r| now.saturating_sub(r) > errors_window) {
                    t.errors_delta = 0;
                }
                t.errors = Some(errors);
            }
            let names: HashSet<&str> = n.containers.iter().map(|c| c.name.as_str()).collect();
            self.containers.retain(|k, _| !k.starts_with(&format!("{}/", n.id)) || names.contains(&k[n.id.len() + 1..]));
            for c in &n.containers {
                let t = self.containers.entry(format!("{}/{}", n.id, c.name)).or_default();
                if t.restarts.is_some_and(|prev| c.restart_count > prev) {
                    t.restarted.push_back(now);
                }
                t.restarts = Some(c.restart_count);
                prune(&mut t.restarted, now, restart_window);
            }
        }
        for s in snap.switches.iter().filter(|s| s.status == LinkStatus::Online) {
            for p in &s.ports {
                let t = self.ports.entry(format!("{}/{}", s.id, p.name)).or_default();
                let errors = p.rx_errors + p.tx_errors + p.fcs_errors;
                if let Some(prev) = t.errors {
                    if errors > prev {
                        t.errors_rose_ms = Some(now);
                        t.errors_delta += errors - prev;
                    }
                }
                if t.errors_rose_ms.is_none_or(|r| now.saturating_sub(r) > port_window) {
                    t.errors_delta = 0;
                }
                t.errors = Some(errors);
            }
        }
        for e in endpoints {
            let busy = e.metric("genTps").is_some_and(|v| v > 0.0) || e.metric("running").is_some_and(|v| v > 0.0);
            let seen = self.endpoint_active.entry(e.id.clone()).or_insert(now);
            if busy {
                *seen = now;
            }
        }
        self.endpoint_active.retain(|id, _| endpoints.iter().any(|e| &e.id == id));
        // Counters behind generic rate rules.
        let mut live = HashSet::new();
        for r in rules.rules.iter().filter(|r| r.enabled && r.kind == RuleKind::Rate && !CODED.contains(&r.id.as_str())) {
            let win = rules.th(&r.id, None).window_ms.max(1000);
            for rd in readings(snap, endpoints, r) {
                let key = format!("{}|{}", r.id, rd.entity_key());
                live.insert(key.clone());
                let q = self.counters.entry(key).or_default();
                if q.back().is_some_and(|b| rd.value < b.1) {
                    q.clear(); // the counter went back: a reset, not negative traffic
                }
                q.push_back((now, rd.value));
                while q.len() > 2 && q.front().is_some_and(|f| now.saturating_sub(f.0) > win) {
                    q.pop_front();
                }
            }
        }
        self.counters.retain(|k, _| live.contains(k));
    }

    /// Growth of a generic rate rule's counter over its window: per second when the
    /// rule's unit is "/s", otherwise the count.
    fn growth(&self, rule: &Rule, key: &str) -> Option<f64> {
        let q = self.counters.get(&format!("{}|{key}", rule.id))?;
        let (first, last) = (q.front()?, q.back()?);
        if last.0 <= first.0 {
            return None;
        }
        let delta = last.1 - first.1;
        Some(if rule.unit == "/s" { delta / ((last.0 - first.0) as f64 / 1000.0) } else { delta })
    }
}

/// An inference server as the endpoint list sees it (src-tauri/src/models/endpoints.rs).
#[derive(Clone, Debug, Default)]
pub struct EndpointInfo {
    /// A durable owner-started rolling maintenance hold.
    pub maintenance_hold: bool,
    pub id: String,
    /// "llama-3.3-70b on cell1–cell3"
    pub label: String,
    /// What the UI calls it (models/endpoints.rs label_of): "glm-5.3-flash", "Ollama on cell3".
    pub name: String,
    /// "llama-3.3-70b"
    pub model: String,
    pub head_node: String,
    /// ready | loading | down | stopped
    pub health: String,
    /// Why it's down: "Pipeline stage 3 of 3 on cell3 is offline".
    pub reason: Option<String>,
    pub message: Option<String>,
    /// Serving metrics by rule metric name: ttftP95 (ms), waiting, kvPct, preemptions…
    pub metrics: HashMap<String, f64>,
    /// Every node it runs on, head first.
    pub nodes: Vec<String>,
    /// Expected nodes that are offline, not reporting or not running their part.
    pub missing_nodes: Vec<String>,
    /// The containers behind it (their names).
    pub containers: Vec<String>,
}

impl EndpointInfo {
    pub fn metric(&self, name: &str) -> Option<f64> {
        self.metrics.get(name).copied().filter(|v| v.is_finite())
    }

    /// Whether a container on a node is one of its members.
    pub fn has_member(&self, node: &str, container: &str) -> bool {
        self.containers.iter().any(|c| c == container) && (self.nodes.is_empty() || self.nodes.iter().any(|n| n == node))
    }
}

/// Why a node's GPU readings are unavailable, when they are (`GpuStats.readable` is false: the
/// collector got no numbers from nvidia-smi, or the simulator plays a GPU fault). The words are
/// the GPU's own error, else the node's.
pub fn gpu_error(n: &NodeSnapshot) -> Option<String> {
    if n.gpu.readable {
        return None;
    }
    Some(n.gpu.error.clone().or_else(|| n.error.clone()).filter(|e| !e.is_empty()).unwrap_or_else(|| "nvidia-smi gave no readings".into()))
}

/// What the gpu-throttle rule makes of one reading: the reasons that count right now and the
/// message, or None when nothing counts ("GPU throttled: SW power cap at 98% busy, 1.9 of 3.0 GHz").
pub fn throttle_verdict(g: &crate::model::GpuStats) -> Option<(Vec<String>, String)> {
    if !g.readable {
        return None;
    }
    let clock_low = g.clock_max_mhz.filter(|m| *m > 0).is_none_or(|m| (g.sm_clock_mhz as f64) < THROTTLE_CLOCK_BELOW * m as f64);
    let at_limit = g.power_limit_w.filter(|l| *l > 0.0).is_some_and(|l| g.power_w as f64 >= THROTTLE_POWER_AT * l as f64);
    let busy = (g.util_pct >= THROTTLE_BUSY_UTIL_PCT && clock_low) || at_limit;
    let reasons: Vec<String> = g
        .throttle
        .iter()
        .filter(|r| {
            let r = r.trim().to_ascii_lowercase();
            if THROTTLE_NEVER.contains(&r.as_str()) {
                false
            } else if THROTTLE_ALWAYS.contains(&r.as_str()) {
                true
            } else {
                busy // SW power cap, SW thermal, sync boost, and anything newer
            }
        })
        .cloned()
        .collect();
    if reasons.is_empty() {
        return None;
    }
    let ghz = |mhz: u32| format!("{:.1}", mhz as f64 / 1000.0);
    let clocks = match g.clock_max_mhz.filter(|m| *m > 0) {
        Some(m) => format!("{} of {} GHz", ghz(g.sm_clock_mhz), ghz(m)),
        None => format!("{} GHz", ghz(g.sm_clock_mhz)),
    };
    let message = format!("GPU throttled: {} at {:.0}% busy, {clocks}", reasons.join(", "), g.util_pct);
    Some((reasons, message))
}

/// Readings for coded rules judged by another module: metric → (entity, value, message).
/// The entity is "cluster" or "<switch>/<port>".
pub type External = HashMap<String, Vec<(String, f64, String)>>;

/// What every rule sees.
pub struct Ctx<'a> {
    pub snap: &'a ClusterSnapshot,
    pub endpoints: &'a [EndpointInfo],
    pub now: u64,
    pub trackers: &'a Trackers,
    /// Ids of alerts currently pending or firing, for hysteresis (stay raised until clearly better).
    pub active: &'a HashSet<String>,
    /// Ids of the alerts among them that are firing.
    pub firing: &'a HashSet<String>,
    /// Ids of the node rows among them shown as "not confirmed down" (information).
    pub held: &'a HashSet<String>,
    pub rules: &'a RuleSet,
    pub external: &'a External,
    /// The fabric monitor's counter rates for this snapshot (fabric::rates).
    pub rates: Option<&'a Rates>,
}

impl Ctx<'_> {
    fn is_active(&self, key: &str) -> bool {
        self.active.contains(key)
    }

    fn is_firing(&self, key: &str) -> bool {
        self.firing.contains(key)
    }

    fn th(&self, id: &str, node: &str) -> Th {
        self.rules.th(id, Some(node))
    }

    fn category(&self, id: &str, fallback: Category) -> Category {
        self.rules.get(id).map(Rule::category).unwrap_or(fallback)
    }
}

/// The severity a value earns: critical past the critical line, warning past the warning
/// line, and still a warning while an active alert hasn't got back past its own clear line.
/// `inclusive` counts reaching a line (whole-percent rules: 85% is at the 85% line).
///
/// Without a clear line of its own an alert holds exactly as long as it would raise. (Round 2
/// held it while the value merely *reached* the warning line, so a rate rule warning above
/// 0/s stayed raised at 0.0/s forever once the counter stopped: the 23-minute RoCE alerts.)
fn judge(th: &Th, v: f64, raw: f64, inclusive: bool, active: bool) -> Option<Severity> {
    let hit = |line: f64| if inclusive { th.op.reached(v, line) } else { th.op.past(v, line) };
    if th.crit.is_some_and(hit) {
        return Some(Severity::Crit);
    }
    if th.warn.is_some_and(hit) {
        return Some(Severity::Warn);
    }
    if active && th.clear.is_some_and(|c| th.op.reached(raw, c)) {
        return Some(if th.warn.is_some() { Severity::Warn } else { Severity::Crit });
    }
    None
}

fn node_entity(n: &NodeSnapshot) -> Entity {
    Entity { kind: "node".into(), id: n.id.clone(), node: Some(n.id.clone()), switch: None, label: n.name.clone() }
}

fn on_node(kind: &str, id: &str, n: &NodeSnapshot) -> Entity {
    Entity { kind: kind.into(), id: id.into(), node: Some(n.id.clone()), switch: None, label: format!("{id} on {}", n.name) }
}

fn key_of(rule: &str, entity: &Entity) -> String {
    match (&entity.node, &entity.switch) {
        (_, Some(sw)) if entity.kind != "switch" => format!("{rule}:{}:{sw}:{}", entity.kind, entity.id),
        (Some(node), _) if entity.kind != "node" => format!("{rule}:{}:{node}:{}", entity.kind, entity.id),
        _ => format!("{rule}:{}:{}", entity.kind, entity.id),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cond(rule: &str, category: Category, severity: Severity, entity: Entity, message: String, summary: String, for_ms: u64, clear_ms: u64) -> Condition {
    Condition { key: key_of(rule, &entity), rule: rule.into(), category, severity, entity, message, summary, detail: None, value: None, for_ms, clear_ms, notify: true, cause: None, short: None }
}

trait With {
    fn detail(self, d: Option<String>) -> Self;
    fn value(self, v: f64) -> Self;
    fn short(self, s: &str) -> Self;
}

impl With for Condition {
    fn short(mut self, s: &str) -> Self {
        self.short = Some(s.into());
        self
    }
    fn detail(mut self, d: Option<String>) -> Self {
        self.detail = d;
        self
    }
    fn value(mut self, v: f64) -> Self {
        self.value = Some(v);
        self
    }
}

/// Every rule over one snapshot. Also returns the nodes and switches whose other alerts
/// should be left as they are (no current data to confirm or clear them), and the ids of
/// single alerts left so (a node's update alerts while its facts are unconfirmed).
pub fn evaluate(ctx: &Ctx) -> (Vec<Condition>, HashSet<String>) {
    let mut out = Vec::new();
    let mut frozen = HashSet::new();
    for n in &ctx.snap.nodes {
        node_link(ctx, n, &mut out);
        if !reporting(n) {
            frozen.insert(format!("node:{}", n.id));
            continue;
        }
        for (id, metric) in [("mem-available", "memAvailableGiB"), ("mem-pressure", "memPsiFull"), ("mem-swap-in", "swapInMiBs")] {
            if ctx.th(id, &n.id).enabled && node_metric(n, metric).is_none() {
                frozen.insert(format!("{id}:node:{}", n.id));
            }
        }
        memory(ctx, n, &mut out);
        disk(ctx, n, &mut out);
        // A restart or a sleep loses the samples, not the alert: while they build up again a real
        // fill can't be measured, so its warning holds as long as the disk is still gaining.
        if let Some(d) = root_disk(n) {
            let key = format!("disk-eta:disk:{}:{}", n.id, d.mount);
            let window = Some(ctx.th("disk-eta", &n.id).window_ms).filter(|w| *w > 0).unwrap_or(DISK_WINDOW_MS);
            let young = ctx.trackers.disk.get(&n.id).is_none_or(|q| q.back().zip(q.front()).is_none_or(|(b, f)| b.0.saturating_sub(f.0) < window && (b.0.saturating_sub(f.0) < DISK_STILL_FILLING_MS || b.1 > f.1)));
            if young && ctx.is_active(&key) {
                frozen.insert(key);
            }
        }
        gpu(ctx, n, &mut out);
        fabric(ctx, n, &mut out);
        containers(ctx, n, &mut out);
        clock(ctx, n, &mut out);
        kernel(ctx, n, &mut out);
        agent(ctx, n, &mut out);
        roce(ctx, n, &mut out);
        updates(ctx, n, &mut out, &mut frozen);
        if ctx.th("unit-failed", &n.id).enabled {
            for unit in &n.failed_units {
                let short = unit.trim_end_matches(".service");
                out.push(cond(
                    "unit-failed",
                    Category::Containers,
                    Severity::Warn,
                    on_node("service", unit, n),
                    format!("{unit} failed"),
                    format!("{short} failed on {}", n.name),
                    0,
                    0,
                ));
            }
        }
    }
    mac_reach(ctx, &mut out);
    fabric_mtu(ctx, &mut out);
    endpoints(ctx, &mut out);
    // Nodes that restarted on their own (boots.rs keeps their bootIds).
    out.extend(super::boots::conditions(ctx));
    for s in &ctx.snap.switches {
        if !s.configured {
            continue; // "Not set up" is a setting, not a fault
        }
        if s.status != LinkStatus::Online {
            frozen.insert(format!("switch:{}", s.id));
            let th = ctx.rules.th("switch-unreachable", None);
            if !th.enabled {
                continue;
            }
            let why = s.error.clone().unwrap_or_else(|| "No answer".into());
            if super::reach::mac_problem(ctx.snap) {
                // This Mac can't reach anything: not evidence the switch is down.
                let mut c = cond("switch-unreachable", Category::Fabric, Severity::Info, switch_entity(s), "Unreachable · not confirmed down".into(), format!("{} unreachable · not confirmed down", s.name), th.for_ms, th.clear_ms)
                    .detail(s.error.clone())
                    .short("Not confirmed down");
                c.notify = false;
                out.push(c);
                continue;
            }
            out.push(
                cond(
                    "switch-unreachable",
                    Category::Fabric,
                    Severity::Crit,
                    switch_entity(s),
                    format!("Unreachable · {why}"),
                    format!("{} unreachable", s.name),
                    th.for_ms,
                    th.clear_ms,
                )
                .detail(s.error.clone()),
            );
            continue;
        }
        ports(ctx, s, &mut out);
        switch_health(ctx, s, &mut out);
    }
    for r in ctx.rules.rules.iter().filter(|r| !CODED.contains(&r.id.as_str())) {
        generic(ctx, r, &frozen, &mut out);
        external(ctx, r, &mut out);
    }
    (out, frozen)
}

/// A coded rule whose readings another module supplies (the fabric monitor's cabling and
/// NCCL transport checks): each reading past the line is a condition, worded by the reading.
fn external(ctx: &Ctx, rule: &Rule, out: &mut Vec<Condition>) {
    if rule.kind != RuleKind::Coded {
        return;
    }
    let Some(readings) = ctx.external.get(&rule.metric) else { return };
    let th = ctx.rules.th(&rule.id, None);
    if !th.enabled {
        return;
    }
    for (id, value, message) in readings {
        let Some(sev) = judge(&th, *value, *value, false, false) else { continue };
        let entity = match id.split_once('/') {
            Some((switch, port)) => Entity { kind: "port".into(), id: port.into(), node: None, switch: Some(switch.into()), label: format!("{port} on {switch}") },
            None => Entity { kind: "fabric".into(), id: id.clone(), node: None, switch: None, label: "Fabric".into() },
        };
        let summary = format!("{}: {}", rule.title, first_lower(message));
        let mut c = cond(&rule.id, rule.category(), sev, entity, message.clone(), summary, th.for_ms, th.clear_ms).value(*value);
        if !rule.help.is_empty() {
            c.detail = Some(rule.help.clone());
        }
        out.push(c);
    }
}

fn node_link(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let power = n.power_state.as_deref();
    let th = ctx.th("node-offline", &n.id);
    // Work SparkBox started: the node being away is Planned (the engine marks it), until the
    // restart runs past its expected time plus the grace, which is critical.
    if let (Some(m), false) = (&n.maintenance, reporting(n)) {
        let grace = ctx.th("node-reboot", &n.id);
        let at = super::maintenance::local_hhmm(m.started_ms);
        if let Some(until) = m.until_ms.filter(|u| grace.enabled && ctx.now > u + grace.for_ms) {
            out.push(
                cond(
                    "node-reboot",
                    Category::Node,
                    Severity::Crit,
                    node_entity(n),
                    format!("Didn't come back after the restart SparkBox started at {at}"),
                    format!("{} didn't come back after the restart SparkBox started at {at}", n.name),
                    0,
                    0,
                )
                .detail(Some(format!("It was expected back by {}. Check its power and console, then Wake or power-cycle it.", super::maintenance::local_hhmm(until)))),
            );
            return;
        }
        if th.enabled {
            // ── R2-OPS ── maintenance mode (P1.16): the owner's window, not a restart SparkBox started.
            let (message, summary, short) = if m.reason == super::maintenance::MANUAL_REASON {
                (format!("Offline · in maintenance (you, {at})"), format!("{} offline in maintenance", n.name), "Offline (maintenance)")
            } else if m.until_ms.is_none() || matches!(power, Some("off") | Some("shutting-down")) {
                ("Off · shut down from SparkBox".to_string(), format!("{} shut down from SparkBox", n.name), "Shut down (planned)")
            } else {
                (format!("Restarting · started from SparkBox at {at}"), format!("{} restarting", n.name), "Restarting (planned)")
            };
            out.push(cond("node-offline", Category::Node, Severity::Crit, node_entity(n), message, summary, th.for_ms, 0).short(short));
        }
        return;
    }
    let key = format!("node-offline:node:{}", n.id);
    let kind = super::reach::kind_of(n);
    // After a sleep the nodes get a minute to reconnect: nothing new is raised about them.
    let settling = super::reach::settling(ctx.snap, n);
    // This Mac can't reach the cluster: what this node shows isn't evidence it's down. A row
    // already held as not confirmed down stays that while the node shows nothing new (still
    // connecting, an error with no kind, or settling after a sleep): the reconnect after a wake
    // or a restart of SparkBox never makes it critical.
    let held = ctx.held.contains(&key) && (settling || n.status == LinkStatus::Connecting || kind.is_none());
    let unconfirmed = super::reach::unconfirmed(ctx.snap, n) || held;
    let why = unconfirmed.then(|| unconfirmed_why(ctx, n));
    match n.status {
        LinkStatus::Offline | LinkStatus::Error | LinkStatus::Connecting if power.is_some() => {
            // A power action SparkBox started, before the engine has its maintenance window.
            if th.enabled {
                out.push(cond("node-offline", Category::Node, Severity::Crit, node_entity(n), "Restarting · started from SparkBox".into(), format!("{} restarting", n.name), th.for_ms, 0).short("Restarting (planned)"));
            }
        }
        // It answers but won't let SparkBox in: the owner's to fix, not an outage (unless it was
        // already offline: then it stays that, with the new reason).
        LinkStatus::Offline | LinkStatus::Error | LinkStatus::Connecting if kind.as_deref().is_some_and(|k| NEEDS_YOU_KINDS.contains(&k)) && !ctx.is_active(&key) => needs_you(ctx, n, out),
        LinkStatus::Offline | LinkStatus::Error => {
            if th.enabled && !(settling && !ctx.is_firing(&key)) {
                out.push(offline(n, &th, why, false));
            }
        }
        LinkStatus::Connecting => {
            // Still trying: keep an existing offline alert raised instead of resolving it.
            if th.enabled && ctx.is_active(&key) && !(settling && !ctx.is_firing(&key)) {
                out.push(offline(n, &th, why, true));
            }
        }
        LinkStatus::Online if n.stale => {
            let stale = ctx.th("node-stale", &n.id);
            let stale_key = format!("node-stale:node:{}", n.id);
            if !stale.enabled || (settling && !ctx.is_firing(&stale_key)) {
                return;
            }
            let unconfirmed = unconfirmed || (settling && ctx.held.contains(&stale_key));
            let age = n.frame_age_ms.unwrap_or(0) / 1000;
            let mut c = cond(
                "node-stale",
                Category::Node,
                Severity::Warn,
                node_entity(n),
                format!("No new readings for {age} s"),
                format!("{} not updating", n.name),
                stale.for_ms,
                stale.clear_ms,
            );
            if unconfirmed {
                (c.severity, c.message, c.summary, c.notify) = (Severity::Info, "Not updating · not confirmed down".into(), format!("{} not updating · not confirmed down", n.name), false);
                c.short = Some("Not confirmed down".into());
            }
            out.push(c);
        }
        LinkStatus::Online => {}
    }
}

/// What a failure kind means, short: "No SSH reply in 10 s", "Host key changed".
fn link_reason(n: &NodeSnapshot, kind: Option<&str>) -> String {
    match kind {
        Some("timeout") => format!("No SSH reply in {SSH_REPLY_S} s"),
        Some("refused") => "SSH refused the connection".into(),
        Some("unreachable") => "Unreachable on the LAN".into(),
        Some("dns") => "Hostname not found".into(),
        Some("hostKeyChanged") => "Host key changed".into(),
        Some("hostKeyUnknown") => "Host key not trusted yet".into(),
        Some("authFailed") => "Login failed".into(),
        _ => n.error.as_deref().map(plain_error).filter(|e| !e.is_empty()).unwrap_or_else(|| "No reply".into()),
    }
}

/// An error in an alert's words: no "reconnecting…" (an alert never says Reconnecting; the
/// node row says "Offline · trying again") and no addresses (those are in the detail).
pub(super) fn plain_error(e: &str) -> String {
    let address = |w: &str| w.trim_matches(|c: char| !c.is_ascii_alphanumeric()).split(':').next().is_some_and(|h| h.parse::<std::net::Ipv4Addr>().is_ok());
    let text = plain_detail(e).split_whitespace().filter(|w| !address(w)).collect::<Vec<_>>().join(" ");
    text.trim_end_matches(['.', ' ']).to_string()
}

/// A detail with no "reconnecting…" in it (the full error, addresses kept).
fn plain_detail(e: &str) -> String {
    for sep in ["; reconnecting", "; Reconnecting", ", reconnecting", ", Reconnecting"] {
        if let Some(at) = e.find(sep) {
            return format!("{}.", e[..at].trim_end_matches(['.', ' ']));
        }
    }
    if e.trim().trim_end_matches(['…', '.']).eq_ignore_ascii_case("reconnecting") {
        return String::new();
    }
    e.to_string()
}

/// Why a node row is "not confirmed down", for its detail.
fn unconfirmed_why(ctx: &Ctx, n: &NodeSnapshot) -> String {
    if super::reach::mac_problem(ctx.snap) {
        format!("This Mac can't reach the cluster right now, so it can't tell whether {} is down.", n.name)
    } else {
        format!("SparkBox is still reconnecting to {}, so it can't tell yet whether it's down.", n.name)
    }
}

/// node-offline for a node that isn't reporting. `unconfirmed`: why what it shows isn't evidence
/// it's down, when it isn't. `trying`: it's connecting again.
fn offline(n: &NodeSnapshot, th: &Th, unconfirmed: Option<String>, trying: bool) -> Condition {
    let detail = n.error.as_deref().map(plain_detail).filter(|d| !d.is_empty());
    if let Some(why) = unconfirmed {
        // One of the mac-reach incident's rows: shown, never counted or paged.
        let mut c = cond("node-offline", Category::Node, Severity::Info, node_entity(n), "Offline · not confirmed down".into(), format!("{} offline · not confirmed down", n.name), th.for_ms, th.clear_ms)
            .detail(Some(match detail {
                Some(d) => format!("{d} {why}"),
                None => why,
            }))
            .short("Not confirmed down");
        c.notify = false;
        return c;
    }
    let kind = super::reach::kind_of(n);
    let reason = link_reason(n, kind.as_deref());
    // "Error" only for a node that answered and turned SparkBox away (or an error with no kind).
    let error = n.status == LinkStatus::Error && !kind.as_deref().is_some_and(|k| super::reach::REACH_KINDS.contains(&k));
    let summary = match kind.as_deref() {
        Some("hostKeyChanged") => format!("{}'s host key changed", n.name),
        Some("authFailed") => format!("{} refused the saved password", n.name),
        _ if error => format!("{} can't connect · {}", n.name, first_lower(&reason)),
        _ => format!("{} offline · {}", n.name, first_lower(&reason)),
    };
    let message = match (trying, error) {
        (true, _) => "Offline · trying again".to_string(),
        (false, true) => format!("Error · {reason}"),
        (false, false) => format!("Offline · {reason}"),
    };
    cond("node-offline", Category::Node, Severity::Crit, node_entity(n), message, summary, th.for_ms, th.clear_ms).detail(detail)
}

/// node-needs-you: the node answers, but its host key isn't trusted yet or it refused the saved
/// password. Information in Alerts, never paged.
fn needs_you(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let th = ctx.th("node-needs-you", &n.id);
    if !th.enabled {
        return;
    }
    let no_secret = n.error.as_deref().is_some_and(|e| e.to_lowercase().contains("no ssh key was found"));
    let (message, summary, short) = match super::reach::kind_of(n).as_deref() {
        Some("hostKeyUnknown") => ("Host key isn't trusted yet · Trust it".to_string(), format!("{}'s host key isn't trusted yet · Trust it", n.name), "Host key not trusted"),
        _ if no_secret => ("No saved password · Cluster login".to_string(), format!("{} has no saved password · Cluster login", n.name), "No saved password"),
        _ => ("Refused the saved password".to_string(), format!("{} refused the saved password", n.name), "Password refused"),
    };
    let mut c = cond("node-needs-you", Category::Node, Severity::Info, node_entity(n), message, summary, th.for_ms, th.clear_ms).detail(n.error.as_deref().map(plain_detail)).short(short);
    c.notify = false;
    out.push(c);
}

/// mac-reach: this Mac can't reach the cluster (health/reach.rs). One warning, not paged unless
/// the owner turns its paging on; the node rows under it read "Offline · not confirmed down".
fn mac_reach(ctx: &Ctx, out: &mut Vec<Condition>) {
    let Some(r) = ctx.snap.mac_reach.as_ref().filter(|r| super::reach::problem(r.state)) else { return };
    let th = ctx.rules.th("mac-reach", None);
    if !th.enabled {
        return;
    }
    let entity = Entity { kind: "mac".into(), id: "reach".into(), node: None, switch: None, label: "This Mac".into() };
    let title = if r.title.is_empty() { "This Mac can't reach the cluster".to_string() } else { r.title.clone() };
    let message = match title.strip_prefix("This Mac ") {
        Some(rest) => {
            let mut c = rest.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
        None => title.clone(),
    };
    let short = match r.state {
        MacReachState::NoRoute | MacReachState::LostHomeNetwork => "Mac lost its network",
        MacReachState::VpnProblem => "VPN problem",
        MacReachState::WiredSwitch => "Wired LAN switch?",
        _ => "Mac can't reach cluster",
    };
    let mut c = cond("mac-reach", Category::Node, Severity::Warn, entity, message, title, th.for_ms, th.clear_ms).detail(Some(r.detail.clone()).filter(|d| !d.is_empty())).short(short);
    c.notify = th.notify;
    c.cause = Some("Nodes this Mac can't reach aren't confirmed down".into());
    out.push(c);
}

fn memory(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let th = ctx.th("mem-high", &n.id);
    if !th.enabled {
        return;
    }
    let (p, pool) = super::memory::used_pct(n, ctx.th("mem-available", &n.id).enabled);
    let key = format!("mem-high:node:{}", n.id);
    let Some(sev) = judge(&th, shown(p), p, true, ctx.is_active(&key)) else { return };
    let for_ms = if sev == Severity::Crit { th.crit_for_ms } else { th.for_ms };
    let message = if pool > 0 { format!("Unified memory {p:.0}% used outside GPU reservation") } else { format!("Unified memory {p:.0}% used") };
    out.push(
        cond("mem-high", Category::Node, sev, node_entity(n), message, format!("{} memory {p:.0}%", n.name), for_ms, th.clear_ms)
            .value(p)
            .detail(Some("A GB10 stops responding when unified memory runs out. Unload a model or stop a container.".into())),
    );
}

fn disk(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let Some(d) = root_disk(n) else { return };
    // As df reports it: used / (used + avail), and avail free (total − used from an older collector).
    let p = d.used_pct();
    let mut entity = on_node("disk", &d.mount, n);
    if d.mount == "/" {
        entity.label = format!("Root disk on {}", n.name);
    }
    let th = ctx.th("disk-full", &n.id);
    let key = format!("disk-full:disk:{}:{}", n.id, d.mount);
    let free = super::fmt_bytes(d.free());
    let full = if th.enabled { judge(&th, shown(p), p, true, ctx.is_active(&key)) } else { None };
    if let Some(sev) = full {
        out.push(
            cond("disk-full", Category::Disk, sev, entity.clone(), format!("Root disk {p:.0}% full ({free} free)"), format!("{} disk {p:.0}%", n.name), th.for_ms, th.clear_ms)
                .value(p),
        );
    }
    if full == Some(Severity::Crit) {
        return; // already critical; a fill-rate warning would only repeat it
    }
    let eta = ctx.th("disk-eta", &n.id);
    if !eta.enabled {
        return;
    }
    let window = Some(eta.window_ms).filter(|w| *w > 0).unwrap_or(DISK_WINDOW_MS);
    let lines_ms = (eta.warn.unwrap_or(0.0).max(eta.clear.unwrap_or(0.0)) * 3_600_000.0) as u64;
    let Some(rate) = ctx.trackers.disk.get(&n.id).and_then(|q| fill_rate_to(q, d.free() as f64, window, lines_ms)) else { return };
    let eta_s = d.free() as f64 / rate / 1000.0;
    let key = format!("disk-eta:disk:{}:{}", n.id, d.mount);
    if judge(&eta, eta_s / 3600.0, eta_s / 3600.0, false, ctx.is_active(&key)).is_some() {
        let text = eta_text(eta_s);
        out.push(
            cond("disk-eta", Category::Disk, Severity::Warn, entity, format!("Root disk full in {text} if it keeps filling at this rate"), format!("{} disk full in {text}", n.name), eta.for_ms, eta.clear_ms)
                .value(eta_s),
        );
    }
}

fn gpu(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let g = &n.gpu;
    let entity = Entity { kind: "gpu".into(), id: "gpu0".into(), node: Some(n.id.clone()), switch: None, label: format!("GPU on {}", n.name) };
    let hot = ctx.th("gpu-hot", &n.id);
    let hot_key = format!("gpu-hot:gpu:{}:gpu0", n.id);
    if hot.enabled && g.readable {
        if let Some(sev) = judge(&hot, g.temp_c as f64, g.temp_c as f64, false, ctx.is_active(&hot_key)) {
            out.push(
                cond("gpu-hot", Category::Gpu, sev, entity.clone(), format!("GPU at {:.0} °C", g.temp_c), format!("{} GPU {:.0} °C", n.name, g.temp_c), hot.for_ms, hot.clear_ms)
                    .value(g.temp_c as f64),
            );
        }
    }
    let throttle = ctx.th("gpu-throttle", &n.id);
    if let (true, Some((_, message))) = (throttle.enabled, throttle_verdict(g)) {
        out.push(cond("gpu-throttle", Category::Gpu, Severity::Warn, entity.clone(), message, format!("{} GPU throttled", n.name), throttle.for_ms, throttle.clear_ms).value(g.util_pct as f64));
    }
    // The GPU stopped answering nvidia-smi / NVML: its models can't run.
    let unreadable = ctx.th("gpu-unreadable", &n.id);
    if let (true, Some(why)) = (unreadable.enabled, gpu_error(n)) {
        out.push(
            cond("gpu-unreadable", Category::Gpu, Severity::Crit, entity, format!("GPU readings unavailable · {why}"), format!("{} GPU not answering", n.name), unreadable.for_ms, unreadable.clear_ms)
                .detail(Some("Its models are Down until the GPU answers again. Restart the node if it doesn't come back by itself.".into())),
        );
    }
}

fn fabric(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let (down, flap, errs) = (ctx.th("fabric-down", &n.id), ctx.th("fabric-flap", &n.id), ctx.th("net-errors", &n.id));
    for x in &n.nets {
        let track = ctx.trackers.ifaces.get(&format!("{}/{}", n.id, x.iface));
        let ip = x.ip.clone().or_else(|| track.and_then(|t| t.known_ip.clone()));
        let entity = on_node("interface", &x.iface, n);
        if down.enabled && x.role == NetRole::Fabric && !x.up && ip.is_some() {
            let ip = ip.unwrap_or_default();
            out.push(cond("fabric-down", Category::Fabric, Severity::Crit, entity.clone(), format!("Fabric link {} down ({ip})", x.iface), format!("{} down on {}", x.iface, n.name), down.for_ms, down.clear_ms));
        }
        let Some(t) = track else { continue };
        let flaps = t.flaps.len() as f64;
        if flap.enabled && x.role == NetRole::Fabric && judge(&flap, flaps, flaps, false, false).is_some() {
            let times = t.flaps.len();
            let mins = (flap.window_ms / 60_000).max(1);
            let message = if times == 1 { format!("{} dropped and came back in the last {mins} min", x.iface) } else { format!("{} flapped {times} times in {mins} min", x.iface) };
            out.push(cond("fabric-flap", Category::Fabric, Severity::Warn, entity.clone(), message, format!("{} flapping on {}", x.iface, n.name), flap.for_ms, flap.clear_ms).value(flaps));
        }
        let delta = t.errors_delta as f64;
        let rising = t.errors_rose_ms.is_some_and(|r| ctx.now.saturating_sub(r) <= ERRORS_RISING_MS);
        if errs.enabled && rising && x.role != NetRole::Wifi && judge(&errs, delta, delta, false, false).is_some() {
            let mins = (errs.window_ms / 60_000).max(1);
            out.push(
                cond(
                    "net-errors",
                    Category::Fabric,
                    Severity::Warn,
                    entity,
                    format!("{} errors rising (+{} in the last {mins} min)", x.iface, t.errors_delta),
                    format!("{} errors on {}", x.iface, n.name),
                    errs.for_ms,
                    errs.clear_ms,
                )
                .value(delta),
            );
        }
    }
}

/// Fabric MTU: each addressed fabric port should carry the expected MTU (9000), and
/// the ports talking to each other should agree.
fn fabric_mtu(ctx: &Ctx, out: &mut Vec<Condition>) {
    let ports: Vec<(&NodeSnapshot, &NetStats, u32)> = ctx
        .snap
        .nodes
        .iter()
        .filter(|n| reporting(n))
        .flat_map(|n| n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.up && x.ip.is_some()).filter_map(move |x| x.mtu.map(|m| (n, x, m))))
        .collect();
    // The MTU most ports agree on.
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for (_, _, m) in &ports {
        *counts.entry(*m).or_default() += 1;
    }
    let common = counts.iter().max_by_key(|(m, c)| (**c, **m)).map(|(m, _)| *m);
    for (n, x, mtu) in ports {
        let th = ctx.th("fabric-mtu", &n.id);
        if !th.enabled {
            continue;
        }
        let expected = th.warn.map(|w| w as u32);
        let message = match (expected, common) {
            (Some(e), _) if mtu != e => format!("{} MTU {mtu}, expected {e}", x.iface),
            (_, Some(c)) if mtu != c => format!("{} MTU {mtu}, its peers use {c}", x.iface),
            _ => continue,
        };
        let detail = Some("RoCE drops or fragments frames when the two ends of a link disagree. Set the same MTU on every fabric port (and the switch).".to_string());
        out.push(
            cond("fabric-mtu", Category::Fabric, Severity::Warn, on_node("interface", &x.iface, n), message, format!("{} MTU {mtu} on {}", x.iface, n.name), th.for_ms, th.clear_ms)
                .value(mtu as f64)
                .detail(detail),
        );
    }
}

fn containers(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let (looping, exited) = (ctx.th("crash-loop", &n.id), ctx.th("container-exited", &n.id));
    let now_s = ctx.now / 1000;
    for c in &n.containers {
        let entity = on_node("container", &c.name, n);
        let restarts = ctx.trackers.containers.get(&format!("{}/{}", n.id, c.name)).map(|t| t.restarted.len()).unwrap_or(0) as f64;
        // Recovered: running a minute without a restart (whatever the window still counts), or the
        // model it serves answers again.
        let up_a_minute = c.state == "running" && c.started_at > 0 && now_s.saturating_sub(c.started_at) >= CRASH_RECENT_S;
        let serving = c.state == "running" && ctx.endpoints.iter().any(|e| e.health == "ready" && e.has_member(&n.id, &c.name));
        let settled = up_a_minute || serving;
        if looping.enabled && !settled {
            let sev = if c.state == "restarting" { Some(Severity::Crit) } else { judge(&looping, restarts, restarts, false, false) };
            // The seconds it runs between restarts are still the loop: an active crash loop holds
            // (a warning at least, so critical eases only after its clear time) until it settles.
            let sev = sev.or_else(|| ctx.is_active(&key_of("crash-loop", &entity)).then_some(Severity::Warn));
            match sev {
                Some(Severity::Crit) => {
                    let message = if restarts >= 2.0 {
                        format!("{} crash loop: Docker restarted it {restarts} times in {} min", c.name, looping.window_ms / 60_000)
                    } else {
                        format!("{} crash loop: Docker keeps restarting it", c.name)
                    };
                    out.push(cond("crash-loop", Category::Containers, Severity::Crit, entity.clone(), message, format!("{} crash-looping on {}", c.name, n.name), looping.for_ms, looping.clear_ms).value(restarts));
                }
                Some(_) => {
                    out.push(cond("crash-loop", Category::Containers, Severity::Warn, entity.clone(), format!("{} crashed and Docker restarted it", c.name), format!("{} restarted on {}", c.name, n.name), looping.for_ms, looping.clear_ms));
                }
                None => {}
            }
        }
        if exited.enabled && failed_recently(c, now_s, exited.window_ms / 1000) {
            let (sev, message) = if c.oom_killed {
                (Severity::Crit, format!("{} exited {} · OOM-killed", c.name, c.exit_code))
            } else {
                (Severity::Warn, format!("{} exited {}", c.name, c.exit_code))
            };
            let when = (c.finished_at > 0).then(|| super::fmt_ago(now_s, c.finished_at));
            let message = match &when {
                Some(w) => format!("{message} · {w}"),
                None => message,
            };
            out.push(cond("container-exited", Category::Containers, sev, entity, message, format!("{} exited on {}", c.name, n.name), exited.for_ms, exited.clear_ms).value(c.exit_code as f64));
        }
    }
}

/// Exited with an error (or OOM-killed) within the window; an unknown exit time counts as recent.
fn failed_recently(c: &Container, now_s: u64, window_s: u64) -> bool {
    let recent = c.finished_at == 0 || now_s.saturating_sub(c.finished_at) < window_s;
    matches!(c.state.as_str(), "exited" | "dead") && (c.exit_code != 0 || c.oom_killed) && recent
}

fn clock(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let th = ctx.th("ntp-unsynced", &n.id);
    if th.enabled && n.system.ntp_synced == Some(false) {
        out.push(
            cond("ntp-unsynced", Category::Node, Severity::Warn, node_entity(n), "Clock not synchronized with NTP".into(), format!("{} clock not synchronized", n.name), th.for_ms, th.clear_ms)
                .detail(Some("timedatectl says NTP isn't synchronized. Check chrony or systemd-timesyncd and that the node can reach its time server.".into())),
        );
    }
}

/// Pending updates, from the update check's facts on the node (`NodeSnapshot.updates`; none until
/// a node has been checked, and none in Demo). Each clears at the first check that no longer
/// finds it. Facts from before the node last connected (`unconfirmed`) judge nothing: its update
/// alerts stay as they are, neither raised, worsened nor cleared, until a check confirms them.
fn updates(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>, frozen: &mut HashSet<String>) {
    let Some(u) = &n.updates else { return };
    if u.unconfirmed {
        for rule in ["security-updates-pending", "restart-pending", "package-lists-stale"] {
            frozen.insert(key_of(rule, &node_entity(n)));
        }
        return;
    }
    let days = |since: u64| ctx.now.saturating_sub(since) / DAY_MS;
    let th = ctx.th("security-updates-pending", &n.id);
    if th.enabled && u.security > 0 {
        let waited = u.security_since_ms.filter(|s| ctx.now.saturating_sub(*s) >= SECURITY_WAIT_MS);
        let (message, detail) = security_words(u.security, u.security_gpu, waited.map(days));
        let sev = if waited.is_some() { Severity::Warn } else { Severity::Info };
        let mut c = cond("security-updates-pending", Category::Node, sev, node_entity(n), message.clone(), format!("{}: {message}", n.name), th.for_ms, th.clear_ms)
            .detail(Some(detail))
            .value(u.security as f64);
        c.notify = waited.is_some() && th.notify;
        out.push(c);
    }
    let th = ctx.th("restart-pending", &n.id);
    if let Some(since) = u.reboot_since_ms.filter(|s| th.enabled && u.reboot_required && ctx.now.saturating_sub(*s) >= RESTART_WAIT_MS) {
        let message = format!("Restart pending for {}", plural(days(since), "day"));
        out.push(
            cond("restart-pending", Category::Node, Severity::Warn, node_entity(n), message.clone(), format!("{}: {}", n.name, first_lower(&message)), th.for_ms, th.clear_ms)
                .detail(Some("/var/run/reboot-required: installed updates (a kernel or a library) take effect after a restart.".into())),
        );
    }
    let th = ctx.th("package-lists-stale", &n.id);
    if let Some(lists) = u.lists_ms.filter(|l| th.enabled && ctx.now.saturating_sub(*l) >= LISTS_STALE_MS) {
        let ago = format!("Package lists last refreshed {} ago", plural(days(lists), "day"));
        out.push(
            cond("package-lists-stale", Category::Node, Severity::Warn, node_entity(n), format!("{ago}; apt-daily may not be running"), format!("{}: {}", n.name, first_lower(&ago)), th.for_ms, th.clear_ms)
                .detail(Some("apt-daily.timer refreshes them twice a day, so what Updates shows may be out of date. Check the timer, or use Check all nodes in Updates.".into())),
        );
    }
}

/// The security alert's (message, detail) for `security` pending fixes that have waited
/// `waited_days` (None: under a week), `gpu_stack` of them (when known) in the GPU stack. SparkBox
/// can't upgrade those yet, so the alert never says to install them from Updates: "4 security
/// updates pending (1 is in the GPU stack: SparkBox can't upgrade it yet)".
pub fn security_words(security: u32, gpu_stack: Option<u32>, waited_days: Option<u64>) -> (String, String) {
    let mut message = format!("{security} security update{} pending", if security == 1 { "" } else { "s" });
    if let Some(days) = waited_days {
        message += &format!(" for {}", plural(days, "day"));
    }
    let detail = match gpu_stack {
        None => "Updates lists them. Upgrade security only installs the ones outside the GPU stack; SparkBox can't upgrade GPU-stack ones (kernel, NVIDIA, CUDA, Docker, network) yet.".to_string(),
        Some(0) => "Updates lists them; install them from there (Upgrade security only).".to_string(),
        Some(gpu) => {
            message += &if gpu == 1 { " (1 is in the GPU stack: SparkBox can't upgrade it yet)".to_string() } else { format!(" ({gpu} are in the GPU stack: SparkBox can't upgrade them yet)") };
            match security.saturating_sub(gpu) {
                0 => "Updates lists them. SparkBox can't upgrade the GPU stack (kernel, NVIDIA, CUDA, Docker, network) yet.".to_string(),
                rest => format!("Updates lists them; Upgrade security only installs the {rest} outside the GPU stack. SparkBox can't upgrade GPU-stack ones yet."),
            }
        }
    };
    (message, detail)
}

/// "1 day", "3 days".
fn plural(n: u64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// The update rules' defaults (rule.rs `extra_defaults`): coded here, judged on this Mac only.
pub fn update_rules() -> Vec<Rule> {
    let rule = |id: &str, title: &str, short: &str, metric: &str, message: &str, help: &str| Rule {
        id: id.into(),
        area: "node".into(),
        title: title.into(),
        short: short.into(),
        entity: super::rule::EntityKind::Node,
        kind: RuleKind::Coded,
        metric: metric.into(),
        op: super::rule::Op::Gt,
        warn: None,
        crit: None,
        clear: None,
        for_s: 0,
        crit_for_s: None,
        clear_s: 0,
        enabled: true,
        overrides: Default::default(),
        message: message.into(),
        unit: String::new(),
        window_s: None,
        notify: true,
        help: help.into(),
        counters: Vec::new(),
        mac_only: true,
    };
    vec![
        rule(
            "security-updates-pending",
            "Security updates pending",
            "Security updates",
            "updatesSecurity",
            "Security updates pending",
            "Security updates are waiting on the node. Information at first; a warning once they've waited 7 days.",
        ),
        rule("restart-pending", "Restart pending", "Restart pending", "updatesReboot", "Restart pending", "The node has needed a restart to finish installing updates for 3 days."),
        rule(
            "package-lists-stale",
            "Package lists stale",
            "Package lists stale",
            "updatesLists",
            "Package lists not refreshed in 48 h",
            "The node's package lists are over 48 h old: apt-daily may not be running, so the updates SparkBox shows may be out of date.",
        ),
    ]
}

/// GPU Xid errors, mlx5_core errors, OOM kills and hung tasks from the kernel log. Each
/// stays an alert for its rule's window after the latest such event, then clears; a fatal
/// Xid stays until the node boots again (DCGM's "needs reset").
fn kernel(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let mut latest: HashMap<String, (Condition, u64)> = HashMap::new();
    let latched: Vec<crate::model::KernelEvent> = ctx.trackers.xid_latch.get(&n.id).map(|l| l.1.clone()).unwrap_or_default();
    let events = n.kernel_events.iter().chain(latched.iter().filter(|l| !n.kernel_events.contains(l)));
    for e in events {
        let rule = match e.kind.as_str() {
            "xid" => "kernel-xid",
            // A cable or link changing state is information (kernel::stamp makes it kind "link").
            "mlx5" if super::kernel::classify_mlx5(&e.text) == super::kernel::Mlx5::Fault => "kernel-mlx5",
            "oom" => "kernel-oom",
            "hung" => "kernel-hung",
            _ => continue,
        };
        let th = ctx.th(rule, &n.id);
        let latch = e.kind == "xid" && e.code.is_some_and(xid_latches);
        if !th.enabled || (!latch && ctx.now.saturating_sub(e.ts_ms) > th.window_ms.max(60_000)) {
            continue;
        }
        let d = super::kernel::decode(&n.id, e);
        let sev = if e.severity == "crit" { Severity::Crit } else { Severity::Warn };
        let entity = match e.kind.as_str() {
            "xid" => Entity { kind: "gpu".into(), id: format!("xid-{}", e.code.unwrap_or(0)), node: Some(n.id.clone()), switch: None, label: format!("GPU on {}", n.name) },
            "mlx5" => {
                let iface = mlx5_iface(&e.text, n).unwrap_or_else(|| "mlx5".into());
                Entity { kind: "interface".into(), id: iface.clone(), node: Some(n.id.clone()), switch: None, label: format!("{iface} on {}", n.name) }
            }
            _ => node_entity(n),
        };
        let who = e.process.clone().unwrap_or_else(|| "a process".into());
        let (message, summary) = match e.kind.as_str() {
            "xid" => (format!("Xid {} · {}", e.code.unwrap_or(0), d.meaning), format!("{} GPU Xid {}", n.name, e.code.unwrap_or(0))),
            "oom" => (format!("Out of memory: the kernel killed {who}"), format!("{who} OOM-killed on {}", n.name)),
            "hung" => (format!("{who} blocked for more than 2 minutes"), format!("Hung task on {}", n.name)),
            _ => (format!("mlx5_core: {}", short_text(&e.text)), format!("ConnectX-7 error on {}", n.name)),
        };
        let action = if latch { format!("{} It stays open until the node restarts.", d.action) } else { d.action };
        let c = cond(rule, ctx.category(rule, Category::Node), sev, entity, message, summary, 0, 0).detail(Some(action)).value(e.code.unwrap_or(0) as f64);
        match latest.get(&c.key) {
            Some((prev, at)) if *at > e.ts_ms && prev.severity >= c.severity => {}
            _ => {
                latest.insert(c.key.clone(), (c, e.ts_ms));
            }
        }
    }
    let mut list: Vec<_> = latest.into_values().collect();
    list.sort_by(|a, b| a.0.key.cmp(&b.0.key));
    out.extend(list.into_iter().map(|(c, _)| c));
}

/// "mlx5_core 0000:01:00.1 enp1s0f1np1: Link down" → the interface named in it.
fn mlx5_iface(text: &str, n: &NodeSnapshot) -> Option<String> {
    n.nets.iter().find(|x| text.contains(&x.iface)).map(|x| x.iface.clone())
}

fn short_text(text: &str) -> String {
    let t = text.split_once("mlx5_core").map(|(_, r)| r).unwrap_or(text);
    // Drop the PCI address and interface, keep what happened.
    let t = t.rsplit(": ").next().unwrap_or(t).trim();
    t.chars().take(120).collect()
}

/// The node answers SparkBox but its always-on agent has stopped beating.
fn agent(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let th = ctx.th("agent-silent", &n.id);
    let Some(beat) = &n.agent else { return };
    if th.enabled && ctx.now.saturating_sub(beat.last_beat_ms) > 60_000 {
        let ago = super::fmt_ago(ctx.now / 1000, beat.last_beat_ms / 1000);
        out.push(
            cond("agent-silent", Category::Node, Severity::Warn, node_entity(n), format!("The always-on agent stopped reporting · last heartbeat {ago}"), format!("Agent stopped on {}", n.name), th.for_ms, th.clear_ms)
                .detail(Some("SparkBox still watches the node while it's open, but nothing will page you while it's closed. Reinstall the agent from Settings › Notifications.".into())),
        );
    }
    // The agent runs, but with settings other than the ones this Mac pushed (agent::overlay fills
    // config_current and push_error from the agent's status and the last push).
    let stale = ctx.th("agent-config-stale", &n.id);
    if stale.enabled && beat.config_current == Some(false) {
        let why = beat.push_error.clone().filter(|e| !e.is_empty());
        let at = beat.push_error_ms.map(super::maintenance::local_hhmm);
        let message = match (&why, &at) {
            (Some(e), Some(at)) => format!("Alert settings didn't reach {} (push failed {at}: {e})", n.name),
            (Some(e), None) => format!("Alert settings didn't reach {} (push failed: {e})", n.name),
            _ => format!("Alert settings didn't reach {} (it runs older settings)", n.name),
        };
        out.push(
            cond("agent-config-stale", Category::Node, Severity::Warn, node_entity(n), message.clone(), message, stale.for_ms, stale.clear_ms)
                .detail(Some("Its agent would page with older rules, channels or silences. Push again from Settings › Notifications.".into())),
        );
    }
}

/// One RoCE alert per fabric interface, from the fabric monitor's rate table: severity from the
/// worst counter, the message from what the rising counters mean (roce_rules, roce-iface.json).
fn roce(ctx: &Ctx, n: &NodeSnapshot, out: &mut Vec<Condition>) {
    let Some(rates) = ctx.rates.filter(|r| r.ts_ms + 15_000 >= ctx.now) else { return };
    let th = ctx.th(super::roce_rules::RULE, &n.id);
    let Some(rule) = ctx.rules.get(super::roce_rules::RULE).filter(|_| th.enabled) else { return };
    for x in n.nets.iter().filter(|x| x.role == NetRole::Fabric) {
        let Some(ir) = rates.ifaces.get(&format!("{}/{}", n.id, x.iface)) else { continue };
        let entity = Entity { kind: "interface".into(), id: x.iface.clone(), node: Some(n.id.clone()), switch: None, label: ir.label() };
        let key = key_of(super::roce_rules::RULE, &entity);
        let Some(j) = super::roce_rules::judge(&rule.counters, ir, ctx.is_active(&key)) else { continue };
        let summary = format!("{}: {}", ir.label(), first_lower(&j.message));
        let detail = format!("{}. {}", j.detail, rule.help);
        let mut c = cond(super::roce_rules::RULE, Category::Fabric, j.severity, entity, j.message, summary, th.for_ms, th.clear_ms).value(j.value).detail(Some(detail));
        c.cause = Some(j.cause);
        out.push(c);
    }
}

/// What a summary line starts with: the label up to " on ", led by the node when that's where it
/// is ("enP2p1s0f1np1 on cell2" → "cell2 enP2p1s0f1np1"; every node has an enP2p1s0f1np1).
/// The node agent's `short_label` does the same.
pub(super) fn short_label(e: &Entity) -> String {
    match e.label.split_once(" on ") {
        Some((what, place)) if e.node.as_deref() == Some(place) => format!("{place} {what}"),
        Some((what, _)) => what.to_string(),
        None => e.label.clone(),
    }
}

/// "Unified memory" → "unified memory", but a first word with capitals after its first letter
/// ("RoCE", "CRC", "GPU") keeps them. The node agent's `first_lower` does the same.
pub(super) fn first_lower(s: &str) -> String {
    let word = s.split_whitespace().next().unwrap_or("");
    if word.chars().skip(1).any(char::is_uppercase) {
        return s.to_string();
    }
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

fn endpoints(ctx: &Ctx, out: &mut Vec<Condition>) {
    for e in ctx.endpoints {
        let entity = Entity { kind: "endpoint".into(), id: e.id.clone(), node: Some(e.head_node.clone()), switch: None, label: e.label.clone() };
        // What the UI calls it ("Ollama on gx10-0000", "glm-5.3-flash"), never the engine's slug.
        let name = if !e.name.is_empty() {
            e.name.clone()
        } else if e.model.is_empty() {
            e.label.clone()
        } else {
            e.model.clone()
        };
        let down = ctx.th("endpoint-down", &e.head_node);
        let key = key_of("endpoint-down", &entity);
        // Its nodes are ones this Mac can't reach (or that are still reconnecting after a sleep).
        let members: Vec<&NodeSnapshot> = ctx.snap.nodes.iter().filter(|n| n.id == e.head_node || e.missing_nodes.contains(&n.id) || e.nodes.contains(&n.id)).collect();
        let unreached: Vec<&NodeSnapshot> = members.iter().copied().filter(|n| super::reach::unconfirmed(ctx.snap, n)).collect();
        let settling = members.iter().any(|n| super::reach::settling(ctx.snap, n)) && !ctx.is_firing(&key);
        if down.enabled && e.health == "down" && !settling {
            let reason = e.reason.as_deref().or(e.message.as_deref()).map(crate::errors::SbError::words).filter(|m| !m.is_empty());
            if let Some(n) = unreached.first() {
                let mut c = cond("endpoint-down", Category::Containers, Severity::Info, entity.clone(), "Down · not confirmed (this Mac can't reach its node)".into(), format!("{name} not answering · not confirmed down"), down.for_ms, down.clear_ms)
                    .detail(Some(format!("This Mac can't reach {}, so it can't tell whether {name} is down.", n.name)))
                    .short("Not confirmed down");
                c.notify = false;
                out.push(c);
            } else {
                let (message, summary) = match &reason {
                    Some(r) => (format!("Down · {r}"), format!("{name} is down: {}", first_lower(r))),
                    None => ("Down · not answering requests".to_string(), format!("{name} is down: it isn't answering requests")),
                };
                out.push(cond("endpoint-down", Category::Containers, Severity::Crit, entity.clone(), message, summary, down.for_ms, down.clear_ms));
            }
        }
        let idle = ctx.th("endpoint-idle", &e.head_node);
        if idle.enabled && e.health == "ready" {
            let since = ctx.trackers.endpoint_active.get(&e.id).copied().unwrap_or(ctx.now);
            let hours = ctx.now.saturating_sub(since) as f64 / 3_600_000.0;
            if idle.warn.is_some_and(|w| hours >= w) {
                let mut c = cond("endpoint-idle", Category::Containers, Severity::Info, entity, format!("Loaded but idle for {}", eta_text(hours * 3600.0).trim_start_matches('~')), format!("{name} idle"), 0, 0)
                    .detail(Some("It holds unified memory nobody is using. Unload it in Models if it isn't needed.".into()))
                    .value(hours);
                c.notify = false;
                out.push(c);
            }
        }
    }
}

fn switch_entity(s: &SwitchSnapshot) -> Entity {
    Entity { kind: "switch".into(), id: s.id.clone(), node: None, switch: Some(s.id.clone()), label: s.name.clone() }
}

/// The node on a port: the one seen there (MAC, LLDP, comment), else the one the cabling
/// baseline expects (a port that's down has no MAC to see).
fn port_node(p: &SwitchPort) -> Option<String> {
    p.node.clone().or_else(|| p.expected_node.clone())
}

fn port_entity(s: &SwitchSnapshot, p: &SwitchPort) -> Entity {
    let node = port_node(p);
    let label = match &node {
        Some(node) => format!("{} ({node}) on {}", p.name, s.name),
        None => format!("{} on {}", p.name, s.name),
    };
    Entity { kind: "port".into(), id: p.name.clone(), node, switch: Some(s.id.clone()), label }
}

/// The switch's own health sensors: a failed power supply, a stopped fan, a sensor too hot.
fn switch_health(ctx: &Ctx, s: &SwitchSnapshot, out: &mut Vec<Condition>) {
    let th = ctx.rules.th("switch-health", None);
    if !th.enabled {
        return;
    }
    for x in s.sensors.iter().filter(|x| !x.ok) {
        let entity = Entity { kind: "sensor".into(), id: x.name.clone(), node: None, switch: Some(s.id.clone()), label: format!("{} on {}", x.name, s.name) };
        let (sev, message) = match x.unit.as_str() {
            "" => (Severity::Crit, format!("{} failed: the switch runs on its other supply", x.name)),
            "RPM" => (Severity::Warn, format!("{} at {:.0} RPM: it has stopped or is failing", x.name, x.value)),
            "°C" => (Severity::Warn, format!("{} at {:.0} °C", x.name, x.value)),
            _ => (Severity::Warn, format!("{} out of range ({:.1} {})", x.name, x.value, x.unit)),
        };
        let summary = format!("{} {}", s.name, first_lower(&message));
        out.push(cond("switch-health", Category::Fabric, sev, entity, message, summary, th.for_ms, th.clear_ms).value(x.value));
    }
}

fn ports(ctx: &Ctx, s: &SwitchSnapshot, out: &mut Vec<Condition>) {
    let (down, errs) = (ctx.rules.th("port-down", None), ctx.rules.th("port-errors", None));
    for p in &s.ports {
        let entity = port_entity(s, p);
        let optic = ctx.rules.th("optic-rx-low", p.node.as_deref());
        if let (Some(node), false, true) = (&port_node(p), p.running, down.enabled) {
            let message = if p.disabled { format!("Disabled · {node} is cut off from the fabric") } else { format!("No link · {node} is cut off from the fabric") };
            out.push(cond("port-down", Category::Fabric, Severity::Crit, entity.clone(), message, format!("{} down ({node})", p.name), down.for_ms, down.clear_ms));
        }
        if let Some(t) = ctx.trackers.ports.get(&format!("{}/{}", s.id, p.name)) {
            let delta = t.errors_delta as f64;
            let rising = t.errors_rose_ms.is_some_and(|r| ctx.now.saturating_sub(r) <= ERRORS_RISING_MS);
            if errs.enabled && rising && judge(&errs, delta, delta, false, false).is_some() {
                let mins = (errs.window_ms / 60_000).max(1);
                out.push(
                    cond("port-errors", Category::Fabric, Severity::Warn, entity.clone(), format!("Errors rising (+{} in the last {mins} min)", t.errors_delta), format!("{} errors on {}", p.name, s.name), errs.for_ms, errs.clear_ms)
                        .value(delta),
                );
            }
        }
        if !p.running || !optic.enabled {
            continue;
        }
        let worst = p.lane_rx_power_dbm.iter().copied().enumerate().min_by(|a, b| a.1.total_cmp(&b.1));
        let (lane, dbm) = match (worst, p.rx_power_dbm) {
            (Some((i, v)), _) => (Some(i + 1), v),
            (None, Some(v)) => (None, v),
            (None, None) => continue,
        };
        let key = format!("optic-rx-low:port:{}:{}", s.id, p.name);
        if let Some(sev) = judge(&optic, dbm as f64, dbm as f64, false, ctx.is_active(&key)) {
            let which = lane.map(|l| format!("Lane {l} receive")).unwrap_or_else(|| "Receive".into());
            let weak = optic.warn.unwrap_or(-7.0);
            out.push(
                cond("optic-rx-low", Category::Fabric, sev, entity, format!("{which} power {dbm:.1} dBm (weak below {weak:.0} dBm)"), format!("{} optic weak ({dbm:.1} dBm)", p.name), optic.for_ms, optic.clear_ms)
                    .value(dbm as f64),
            );
        }
    }
}

// ---- Generic rules: any threshold or rate rule on a metric `readings` knows.

/// One value of a rule's metric, for one entity.
pub struct Reading {
    pub entity: Entity,
    /// The node whose overrides apply.
    pub node: Option<String>,
    pub value: f64,
}

impl Reading {
    fn entity_key(&self) -> String {
        format!("{}:{}:{}:{}", self.entity.kind, self.entity.node.as_deref().unwrap_or(""), self.entity.switch.as_deref().unwrap_or(""), self.entity.id)
    }
}

/// A node-level metric by rule name.
pub fn node_metric(n: &NodeSnapshot, metric: &str) -> Option<f64> {
    Some(match metric {
        "memPct" => pct(n.mem.used as f64, n.mem.total as f64),
        "diskPct" => root_disk(n).map(|d| d.used_pct())?,
        // An unreadable GPU has no readings: its zeros are not an idle, cool GPU.
        "gpuTemp" | "gpuUtil" | "gpuPowerW" if !n.gpu.readable => return None,
        "gpuTemp" => n.gpu.temp_c as f64,
        "gpuUtil" => n.gpu.util_pct as f64,
        "gpuPowerW" => n.gpu.power_w as f64,
        "cpuPct" => n.cpu.total_pct as f64,
        "cpuTempC" => n.cpu.temp_c as f64,
        "clockOffsetMs" => n.system.clock_offset_ms? as f64,
        // /proc/pressure/memory full avg10, when the kernel exposes it.
        "memPsiFull" => n.mem.psi_full10.filter(|v| v.is_finite() && *v >= 0.0)? as f64,
        "memAvailableGiB" => super::memory::available_gib(n)?,
        "swapInMiBs" => n.mem.swap_in_bps.filter(|v| v.is_finite() && *v >= 0.0)? / 1_048_576.0,
        "swapPct" => (n.mem.swap_total > 0).then(|| pct(n.mem.swap_used as f64, n.mem.swap_total as f64))?,
        "load1" => *n.cpu.load.first()? as f64,
        _ => return None,
    })
}

/// A fabric interface's counter or gauge by rule name. RoCE counters go by their
/// snapshot names (ecnMarked, outOfSequence…) or the history names (ecn, oos…).
pub fn net_metric(x: &NetStats, metric: &str) -> Option<f64> {
    let roce = || x.roce.as_ref().and_then(|r| serde_json::to_value(r).ok());
    let field = |v: &serde_json::Value, k: &str| v.get(k).and_then(|f| f.as_f64());
    let sum = |v: &serde_json::Value, keys: &[&str]| {
        let vals: Vec<f64> = keys.iter().filter_map(|k| field(v, k)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum())
    };
    let pause = |v: &serde_json::Value, ctrl: &str, prios: &str| {
        field(v, ctrl).or_else(|| {
            let list: Vec<f64> = v.get(prios)?.as_array()?.iter().filter_map(|p| p.as_f64()).collect();
            (!list.is_empty()).then(|| list.iter().sum())
        })
    };
    Some(match metric {
        "rxBps" => x.rx_bps,
        "txBps" => x.tx_bps,
        "ifErrors" => (x.rx_errors + x.tx_errors) as f64,
        "rxDrops" => x.rx_drops as f64,
        "ecn" => field(&roce()?, "ecnMarked")?,
        "cnpSent" => field(&roce()?, "cnpSent")?,
        "cnpHandled" => field(&roce()?, "cnpHandled")?,
        "oos" => field(&roce()?, "outOfSequence")?,
        "seqErr" => field(&roce()?, "packetSeqErr")?,
        "timeouts" => field(&roce()?, "localAckTimeoutErr")?,
        "pauseRx" => pause(&roce()?, "rxPauseCtrl", "rxPause")?,
        "pauseTx" => pause(&roce()?, "txPauseCtrl", "txPause")?,
        "discards" => sum(&roce()?, &["rxDiscardsPhy", "portXmitDiscards"])?,
        "crc" => sum(&roce()?, &["rxCrcErrorsPhy", "rxIcrcErr"])?,
        other => field(&roce()?, other)?,
    })
}

fn port_metric(p: &SwitchPort, metric: &str) -> Option<f64> {
    Some(match metric {
        "rxBps" => p.rx_bps,
        "txBps" => p.tx_bps,
        "portErrors" | "errors" => (p.rx_errors + p.tx_errors + p.fcs_errors) as f64,
        "rxPowerDbm" => p.lane_rx_power_dbm.iter().copied().reduce(f32::min).or(p.rx_power_dbm)? as f64,
        "moduleTempC" => p.module_temp_c? as f64,
        "rxPause" => p.rx_pause as f64,
        _ => return None,
    })
}

fn container_metric(c: &Container, metric: &str) -> Option<f64> {
    Some(match metric {
        "cpu" => c.cpu_pct as f64,
        "mem" => c.mem as f64,
        "restarts" => c.restart_count as f64,
        _ => return None,
    })
}

/// Every current value of `rule`'s metric, one per entity of its kind.
pub fn readings(snap: &ClusterSnapshot, endpoints: &[EndpointInfo], rule: &Rule) -> Vec<Reading> {
    use super::rule::EntityKind as K;
    let mut out = Vec::new();
    match rule.entity {
        K::Node => {
            for n in snap.nodes.iter().filter(|n| reporting(n)) {
                if let Some(value) = node_metric(n, &rule.metric) {
                    out.push(Reading { entity: node_entity(n), node: Some(n.id.clone()), value });
                }
            }
        }
        K::Net => {
            for n in snap.nodes.iter().filter(|n| reporting(n)) {
                for x in n.nets.iter().filter(|x| x.role == NetRole::Fabric) {
                    if let Some(value) = net_metric(x, &rule.metric) {
                        out.push(Reading { entity: on_node("interface", &x.iface, n), node: Some(n.id.clone()), value });
                    }
                }
            }
        }
        K::Container => {
            for n in snap.nodes.iter().filter(|n| reporting(n)) {
                for c in &n.containers {
                    if let Some(value) = container_metric(c, &rule.metric) {
                        out.push(Reading { entity: on_node("container", &c.name, n), node: Some(n.id.clone()), value });
                    }
                }
            }
        }
        K::Port => {
            for s in snap.switches.iter().filter(|s| s.status == LinkStatus::Online) {
                for p in &s.ports {
                    if let Some(value) = port_metric(p, &rule.metric) {
                        out.push(Reading { entity: port_entity(s, p), node: port_node(p), value });
                    }
                }
            }
        }
        K::Endpoint => {
            for e in endpoints.iter().filter(|e| e.health == "ready" || e.health == "loading") {
                if let Some(value) = e.metric(&rule.metric) {
                    let entity = Entity { kind: "endpoint".into(), id: e.id.clone(), node: Some(e.head_node.clone()), switch: None, label: e.label.clone() };
                    out.push(Reading { entity, node: Some(e.head_node.clone()), value });
                }
            }
        }
        K::Cluster => {}
    }
    out
}

/// "91%", "2.4 s", "230 ms", "-7.5 dBm", "12/s", "0.004/s", "3".
pub fn fmt_value(v: f64, unit: &str) -> String {
    match unit {
        "%" => format!("{v:.0}%"),
        "°C" => format!("{v:.0} °C"),
        "ms" if v.abs() >= 1000.0 => format!("{:.1} s", v / 1000.0),
        "ms" => format!("{v:.0} ms"),
        "s" => format!("{v:.1} s"),
        "h" => eta_text(v * 3600.0).trim_start_matches('~').to_string(),
        "dBm" => format!("{v:.1} dBm"),
        "GiB" | "MiB/s" => format!("{v:.2} {unit}"),
        "/s" => crate::fabric::rates::fmt_per_s(v),
        _ if v.fract() == 0.0 => format!("{v:.0}"),
        _ => format!("{v:.1}"),
    }
}

fn fill(template: &str, value: &str, threshold: &str) -> String {
    template.replace("{value}", value).replace("{threshold}", threshold)
}

fn generic(ctx: &Ctx, rule: &Rule, frozen: &HashSet<String>, out: &mut Vec<Condition>) {
    if !matches!(rule.kind, RuleKind::Threshold | RuleKind::Rate) {
        return;
    }
    for r in readings(ctx.snap, ctx.endpoints, rule) {
        if r.node.as_ref().is_some_and(|n| frozen.contains(&format!("node:{n}"))) && rule.entity != super::rule::EntityKind::Endpoint {
            continue;
        }
        let th = ctx.rules.th(&rule.id, r.node.as_deref());
        if !th.enabled {
            continue;
        }
        let value = match rule.kind {
            RuleKind::Rate => match ctx.trackers.growth(rule, &r.entity_key()) {
                Some(v) => v,
                None => continue,
            },
            _ => r.value,
        };
        let key = key_of(&rule.id, &r.entity);
        let inclusive = rule.unit == "%";
        let judged = if inclusive { shown(value) } else { value };
        let Some(sev) = judge(&th, judged, value, inclusive, ctx.is_active(&key)) else { continue };
        let sev = if rule.notify { sev } else { Severity::Info };
        let line = if sev == Severity::Crit { th.crit } else { th.warn };
        let (v, t) = (fmt_value(value, &rule.unit), line.map(|l| fmt_value(l, &rule.unit)).unwrap_or_default());
        let message = if rule.message.is_empty() { format!("{} {v}", rule.title) } else { fill(&rule.message, &v, &t) };
        let summary = format!("{} {} {v}", short_label(&r.entity), first_lower(&rule.title));
        let for_ms = if sev == Severity::Crit { th.crit_for_ms } else { th.for_ms };
        let mut c = cond(&rule.id, rule.category(), sev, r.entity, message, summary, for_ms, th.clear_ms).value(value);
        if !rule.help.is_empty() {
            c.detail = Some(rule.help.clone());
        }
        c.notify = rule.notify && th.notify;
        out.push(c);
    }
}

/// The reporting nodes a generic node rule has no reading from (the metric isn't in their data:
/// cell1's clock offset when timedatectl doesn't say), by name, for "Not reported by this node".
pub fn unreported(rule: &Rule, snap: &ClusterSnapshot) -> Vec<String> {
    if rule.entity != super::rule::EntityKind::Node || CODED.contains(&rule.id.as_str()) {
        return Vec::new();
    }
    snap.nodes.iter().filter(|n| reporting(n) && node_metric(n, &rule.metric).is_none()).map(|n| n.name.clone()).collect()
}

/// Whether a rule is one the engine can judge: coded, or generic on a known metric.
pub fn understood(rule: &Rule, snap: &ClusterSnapshot, endpoints: &[EndpointInfo], external: &External) -> bool {
    CODED.contains(&rule.id.as_str()) || external.contains_key(&rule.metric) || !readings(snap, endpoints, rule).is_empty()
}
