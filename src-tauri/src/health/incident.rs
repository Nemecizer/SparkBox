//! Incidents: alerts that share a root cause, counted and paged as one (PagerDuty's
//! incidents, Datadog's root and affected). One bad optic is one incident with the RoCE
//! alert, the switch port's errors and its optic as children; a node going away is one
//! incident with its fabric links, its switch port and the model that had a stage there.
//!
//! Roots (fixtures/incident-roots.json has a case for each; the node agent mirrors them):
//! - `link:<node>/<iface>`: RoCE and link counters on a fabric interface (its flaps too), plus
//!   the errors and optic of the switch port it's cabled to;
//! - `node:<id>` for both halves at once: link alerts on two of a node's fabric interfaces are
//!   one incident, "cell2 fabric (both halves)" (one cable, optic or port 2 behind both);
//! - `node:<id>`: offline, not updating, its agent, its fabric links going down, its switch
//!   port going down, and a model down because a stage there is missing;
//! - `gpu:<node>`: Xid errors and an unreadable GPU, and a model down because of it;
//! - `endpoint:<id>`: a model's own alerts and the crash loops and exits of its containers;
//! - `switch:<id>`: a switch that doesn't answer and its other ports;
//! - `mac:reach`: this Mac can't reach the cluster (health/reach.rs), and under it whatever would
//!   root at a node it can't reach (offline, not updating, a model down there) and a switch that
//!   doesn't answer: one incident, not one per node (Mac only: the agents don't see the Mac);
//! - otherwise the alert's own entity.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::rules::EndpointInfo;
use super::{Alert, AlertState, Severity};
use crate::model::{ClusterSnapshot, NetRole};

/// Rules about a node being reachable and reporting: they root at `node:<id>`.
const NODE_RULES: [&str; 7] = ["node-offline", "node-reboot", "node-stale", "agent-silent", "agent-config-stale", "peer-unreachable", "node-needs-you"];
/// This Mac can't reach the cluster: the one incident of a Mac-side outage.
pub const MAC_ROOT: &str = "mac:reach";
/// Interface rules that root at the link they're about.
const LINK_RULES: [&str; 5] = ["roce-iface", "fabric-flap", "net-errors", "kernel-mlx5", "fabric-mtu"];
/// Switch-port rules that root at the link cabled to the port.
const PORT_LINK_RULES: [&str; 3] = ["port-errors", "optic-rx-low", "fabric-cabling"];

/// How an incident reads in the Alerts window and counts everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IncidentState {
    /// At least one child counts.
    Firing,
    /// Nothing breaches any more; it clears when the hold is over.
    Recovering,
    /// Work SparkBox started.
    Planned,
    Silenced,
    Acknowledged,
    /// Information only (an idle model).
    Info,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Incident {
    /// The root key.
    pub id: String,
    /// "cell2 half 2: RoCE retransmitting · CRC and symbol errors rising · check the optic or cable"
    pub title: String,
    /// The worst counted child's (or the worst child's when none counts).
    pub severity: Severity,
    pub state: IncidentState,
    /// Alert ids, the headline first.
    pub children: Vec<String>,
    pub cause: Option<String>,
    pub since_ms: u64,
    /// "cell2 half 2", "cell3", "llama-3.3-70b on cell1–cell3".
    pub affects: String,
    /// The headline alert's id (its entity is what Open shows).
    pub headline: String,
    /// Counts toward badges and the top bar.
    pub counted: bool,
    /// While recovering: when the last child clears.
    pub recovers_at_ms: Option<u64>,
    /// While planned: "cell3 restarting (SparkBox, 10:42)".
    pub planned: Option<String>,
    // ── r4-alerts ──
    /// It keeps coming back: the most flaps of any alert in it ("Flapping · 4 times in 30 min").
    #[serde(default)]
    pub flapping: Option<super::FlapInfo>,
}

/// What a root depends on besides the alert: cabling, models and what else is active.
#[derive(Default)]
pub struct RootCtx {
    /// "crs804/qsfp56-dd-1-2" → the fabric interfaces cabled there, (node, iface, half).
    port_ifaces: HashMap<String, Vec<(String, String, u8)>>,
    /// "node/iface" with an active link alert.
    links_active: HashSet<String>,
    /// Each node's fabric interfaces with an active link alert (two: both halves).
    links_by_node: HashMap<String, HashSet<String>>,
    /// Nodes with an active alert about the node itself being gone (offline, stale, fabric down).
    nodes_down: HashSet<String>,
    /// "node/iface" whose fabric link is down: its flapping and errors are part of that.
    links_down: HashSet<String>,
    /// Nodes whose GPU has a fatal fault.
    gpu_fault: HashSet<String>,
    endpoints: Vec<EndpointInfo>,
    node_names: HashMap<String, String>,
    /// Each node's addressed fabric interfaces, for "lost the fabric".
    fabric_ifaces: HashMap<String, Vec<String>>,
    // ── r1-h ──
    /// This Mac can't reach the cluster (the snapshot's mac_reach is a problem)…
    mac: bool,
    /// …and these nodes are the ones it can't reach: their incidents are the Mac's.
    unconfirmed: HashSet<String>,
}

impl RootCtx {
    pub fn new(snap: &ClusterSnapshot, endpoints: &[EndpointInfo], active: &[&Alert]) -> RootCtx {
        let mut c = RootCtx { endpoints: endpoints.to_vec(), mac: super::reach::mac_problem(snap), ..Default::default() };
        for n in &snap.nodes {
            c.node_names.insert(n.id.clone(), n.name.clone());
            if super::reach::unconfirmed(snap, n) {
                c.unconfirmed.insert(n.id.clone());
            }
            if n.gpu_fault.is_some() {
                c.gpu_fault.insert(n.id.clone());
            }
            let fabric: Vec<&crate::model::NetStats> = n.nets.iter().filter(|x| x.role == NetRole::Fabric && (x.ip.is_some() || x.switch_port.is_some())).collect();
            c.fabric_ifaces.insert(n.id.clone(), fabric.iter().map(|x| x.iface.clone()).collect());
            for x in &fabric {
                if let Some(p) = &x.switch_port {
                    let half = crate::fabric::half_of(x, &fabric);
                    c.port_ifaces.entry(p.clone()).or_default().push((n.id.clone(), x.iface.clone(), half));
                }
            }
        }
        for list in c.port_ifaces.values_mut() {
            list.sort_by_key(|(_, _, h)| *h);
        }
        for a in active.iter().filter(|a| a.state != AlertState::Resolved) {
            let node = a.entity.node.clone().unwrap_or_default();
            // A GPU fault the engine itself holds (before the snapshot is stamped with it).
            if a.state == AlertState::Firing && (a.rule == "gpu-unreadable" || (a.rule == "kernel-xid" && a.severity == Severity::Crit)) {
                c.gpu_fault.insert(node.clone());
            }
            if LINK_RULES.contains(&a.rule.as_str()) && a.entity.kind == "interface" {
                c.links_active.insert(format!("{node}/{}", a.entity.id));
                c.links_by_node.entry(node.clone()).or_default().insert(a.entity.id.clone());
            }
            let gone = matches!(a.rule.as_str(), "node-offline" | "node-reboot" | "node-stale" | "fabric-down") || (a.rule == "port-down" && a.entity.node.is_some());
            if a.rule == "fabric-down" {
                c.links_down.insert(format!("{node}/{}", a.entity.id));
            }
            if gone && a.state == AlertState::Firing {
                c.nodes_down.insert(node);
            }
        }
        c
    }

    /// Whether `node` has link alerts on another of its fabric interfaces than `iface`: both halves.
    fn both_halves(&self, node: &str, iface: &str) -> bool {
        self.links_by_node.get(node).is_some_and(|s| s.iter().any(|i| i != iface))
    }

    fn endpoint(&self, id: &str) -> Option<&EndpointInfo> {
        self.endpoints.iter().find(|e| e.id == id)
    }

    /// An endpoint's root: the node whose absence took it down, its GPU fault, or itself.
    fn endpoint_root(&self, e: &EndpointInfo) -> String {
        let members = e.missing_nodes.iter().chain(e.nodes.iter()).chain(std::iter::once(&e.head_node));
        if let Some(n) = members.clone().find(|n| self.nodes_down.contains(*n)) {
            return format!("node:{n}");
        }
        if let Some(n) = members.clone().find(|n| self.gpu_fault.contains(*n)) {
            return format!("gpu:{n}");
        }
        format!("endpoint:{}", e.id)
    }

    /// The incident `a` belongs to.
    pub fn root_of(&self, a: &Alert) -> String {
        if a.rule == "mac-reach" {
            return MAC_ROOT.into();
        }
        let root = self.own_root(a);
        if self.mac {
            // While this Mac can't reach the cluster, a node it can't reach and a switch that
            // doesn't answer are part of that, not incidents of their own.
            let node_gone = root.strip_prefix("node:").is_some_and(|n| self.unconfirmed.contains(n));
            if node_gone || a.rule == "switch-unreachable" {
                return MAC_ROOT.into();
            }
        }
        root
    }

    fn own_root(&self, a: &Alert) -> String {
        let e = &a.entity;
        let node = e.node.clone().unwrap_or_default();
        let rule = a.rule.as_str();
        if NODE_RULES.contains(&rule) && e.kind == "node" {
            return format!("node:{}", e.id);
        }
        match (e.kind.as_str(), rule) {
            ("interface", "fabric-down") => return format!("node:{node}"),
            ("interface", r) if LINK_RULES.contains(&r) && self.links_down.contains(&format!("{node}/{}", e.id)) => return format!("node:{node}"),
            ("interface", r) if LINK_RULES.contains(&r) && !node.is_empty() && self.both_halves(&node, &e.id) => return format!("node:{node}"),
            ("interface", r) if LINK_RULES.contains(&r) && !node.is_empty() => return format!("link:{node}/{}", e.id),
            ("port", "port-down") if !node.is_empty() => return format!("node:{node}"),
            ("port", r) if PORT_LINK_RULES.contains(&r) => {
                let place = format!("{}/{}", e.switch.as_deref().unwrap_or(""), e.id);
                let ifaces: Vec<&(String, String, u8)> = self.port_ifaces.get(&place).map(|v| v.iter().filter(|(n, _, _)| node.is_empty() || *n == node).collect()).unwrap_or_default();
                // The half with a RoCE alert, else the first half cabled there.
                let pick = ifaces.iter().find(|(n, i, _)| self.links_active.contains(&format!("{n}/{i}"))).or(ifaces.first());
                if let Some((n, i, _)) = pick {
                    if self.links_by_node.get(n).is_some_and(|s| s.len() > 1) {
                        return format!("node:{n}");
                    }
                    return format!("link:{n}/{i}");
                }
                return format!("switch:{}", e.switch.as_deref().unwrap_or(""));
            }
            ("port", _) | ("switch", _) | ("sensor", _) => return format!("switch:{}", e.switch.as_deref().unwrap_or(&e.id)),
            ("gpu", _) => return format!("gpu:{node}"),
            ("endpoint", _) => {
                return match self.endpoint(&e.id) {
                    Some(ep) => self.endpoint_root(ep),
                    // No longer listed (its node stopped answering, so nothing finds it): the alert,
                    // held while its node is away, is that node's.
                    None if !node.is_empty() && self.nodes_down.contains(&node) => format!("node:{node}"),
                    None => format!("endpoint:{}", e.id),
                }
            }
            ("container", "crash-loop" | "container-exited") => {
                if let Some(ep) = self.endpoints.iter().find(|ep| ep.has_member(&node, &e.id)) {
                    return self.endpoint_root(ep);
                }
            }
            // mem-high, kernel-oom…: the node's own incident, as the agents root it.
            ("node", _) => return format!("node:{}", e.id),
            _ => {}
        }
        let place = e.switch.as_deref().or(e.node.as_deref()).unwrap_or("");
        format!("{}:{place}/{}", e.kind, e.id)
    }
}

/// Which child leads an incident's title: the worst, then the most telling rule.
fn rank(a: &Alert) -> (u8, u8, u8) {
    let state = match a.state {
        AlertState::Firing => 2,
        AlertState::Pending => 0,
        AlertState::Resolved => 0,
    };
    let order = ["mac-reach", "node-reboot", "node-offline", "node-stale", "node-needs-you", "roce-iface", "switch-unreachable", "kernel-xid", "gpu-unreadable", "endpoint-down", "fabric-down", "port-down"];
    let pos = order.iter().position(|r| *r == a.rule).map(|p| (order.len() - p) as u8).unwrap_or(0);
    let sev = match a.severity {
        Severity::Crit => 2,
        Severity::Warn => 1,
        Severity::Info => 0,
    };
    (state, sev, pos)
}

/// Groups the active (pending and firing) alerts into incidents, worst first. Pending alerts
/// join an incident that has others but never make one of their own.
pub fn build(alerts: &[Alert], roots: &RootCtx) -> Vec<Incident> {
    let mut groups: BTreeMap<String, Vec<&Alert>> = BTreeMap::new();
    for a in alerts.iter().filter(|a| a.state != AlertState::Resolved) {
        let root = a.incident_id.clone().unwrap_or_else(|| roots.root_of(a));
        groups.entry(root).or_default().push(a);
    }
    let mut out = Vec::new();
    for (id, mut kids) in groups {
        if kids.iter().all(|a| a.state == AlertState::Pending) {
            continue;
        }
        kids.sort_by(|a, b| rank(b).cmp(&rank(a)).then(a.since_ms.cmp(&b.since_ms)).then(a.id.cmp(&b.id)));
        let head = kids[0];
        let firing: Vec<&&Alert> = kids.iter().filter(|a| a.state == AlertState::Firing).collect();
        let counted: Vec<&&Alert> = firing.iter().copied().filter(|a| a.counted).collect();
        let state = if !counted.is_empty() {
            IncidentState::Firing
        } else if firing.iter().any(|a| a.recovers_at_ms.is_some() && a.planned.is_none() && a.silence_id.is_none()) {
            IncidentState::Recovering
        } else if firing.iter().any(|a| a.planned.is_some()) {
            IncidentState::Planned
        } else if firing.iter().any(|a| a.silence_id.is_some()) {
            IncidentState::Silenced
        } else if firing.iter().any(|a| a.acked_ms.is_some()) {
            IncidentState::Acknowledged
        } else {
            IncidentState::Info
        };
        let pool: &Vec<&&Alert> = if counted.is_empty() { &firing } else { &counted };
        let severity = pool.iter().map(|a| a.severity).max().unwrap_or(head.severity);
        let (title, affects, cause) = words(&id, &kids, roots);
        out.push(Incident {
            title,
            severity,
            state,
            children: kids.iter().map(|a| a.id.clone()).collect(),
            cause,
            since_ms: kids.iter().map(|a| a.firing_since_ms.unwrap_or(a.since_ms)).min().unwrap_or(head.since_ms),
            affects,
            headline: head.id.clone(),
            counted: !counted.is_empty(),
            recovers_at_ms: (state == IncidentState::Recovering).then(|| firing.iter().filter_map(|a| a.recovers_at_ms).max()).flatten(),
            planned: firing.iter().find_map(|a| a.planned.clone()),
            flapping: firing.iter().filter_map(|a| a.flapping.clone()).max_by_key(|f| f.count),
            id,
        });
    }
    out.sort_by(|a, b| {
        let key = |i: &Incident| (i.counted, i.severity, std::cmp::Reverse(i.since_ms));
        key(b).cmp(&key(a)).then(a.id.cmp(&b.id))
    });
    out
}

/// An incident's title, what it affects, and its cause.
fn words(id: &str, kids: &[&Alert], roots: &RootCtx) -> (String, String, Option<String>) {
    let head = kids[0];
    let has = |rule: &str| kids.iter().find(|a| a.rule == rule && a.state == AlertState::Firing);
    let (kind, rest) = id.split_once(':').unwrap_or(("", id));
    let name = |n: &str| roots.node_names.get(n).cloned().unwrap_or_else(|| n.to_string());
    let cause = kids.iter().find_map(|a| a.cause.clone());
    match kind {
        "node" => {
            let node_name = name(rest);
            if let Some(a) = has("node-reboot").or_else(|| has("node-offline")).or_else(|| has("node-stale")).or_else(|| has("node-needs-you")) {
                return (a.summary.clone(), node_name, cause);
            }
            let down: HashSet<&str> = kids.iter().filter(|a| a.rule == "fabric-down").map(|a| a.entity.id.as_str()).collect();
            let all = roots.fabric_ifaces.get(rest).map(|v| v.len()).unwrap_or(0);
            if !down.is_empty() && down.len() >= all {
                return (format!("{node_name} lost the fabric"), node_name, Some("Every fabric link is down; the LAN still answers.".into()));
            }
            let halves: HashSet<&str> = kids.iter().filter(|a| LINK_RULES.contains(&a.rule.as_str()) && a.entity.kind == "interface").map(|a| a.entity.id.as_str()).collect();
            if halves.len() > 1 && down.is_empty() {
                let what = cause.clone().or_else(|| Some("both halves share port 2: check its optic or cable".into()));
                return (format!("{node_name} fabric (both halves)"), format!("{node_name} fabric"), what);
            }
            (head.summary.clone(), node_name, cause)
        }
        "link" => {
            let affects = kids.iter().find(|a| a.rule == "roce-iface").map(|a| a.entity.label.clone()).unwrap_or_else(|| head.entity.label.clone());
            (head.summary.clone(), affects, cause)
        }
        "gpu" => (head.summary.clone(), format!("GPU on {}", name(rest)), cause),
        "mac" => {
            let n = kids.iter().filter(|a| a.entity.kind == "node").map(|a| a.entity.id.as_str()).collect::<HashSet<_>>().len();
            let affects = match n {
                0 => "This Mac".to_string(),
                1 => "This Mac · 1 node not confirmed down".to_string(),
                n => format!("This Mac · {n} nodes not confirmed down"),
            };
            (head.summary.clone(), affects, kids.iter().find(|a| a.rule == "mac-reach").and_then(|a| a.detail.clone()).or(cause))
        }
        "endpoint" => (head.summary.clone(), head.entity.label.clone(), cause),
        "switch" => {
            let affects = kids.iter().find(|a| a.entity.kind == "switch").map(|a| a.entity.label.clone()).unwrap_or_else(|| head.entity.label.split(" on ").last().unwrap_or("").to_string());
            (head.summary.clone(), affects, cause)
        }
        _ => (head.summary.clone(), head.entity.label.clone(), cause),
    }
}
