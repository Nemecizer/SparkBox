//! The health engine: turns each snapshot into alerts that every status dot,
//! the Alerts window, the menu bar and notifications agree on.
//!
//! Each alert moves ok → pending → firing → resolved. A condition has to hold
//! for its rule's minimum duration before it fires (a one-second blip isn't an
//! incident) and has to stay gone for the rule's clear time before it resolves
//! (a flapping link doesn't page twice a minute); while that hold runs it is
//! Recovering, which counts nowhere. Thresholds have hysteresis: memory raised at
//! 90% stays raised until it's back under 88%, and a critical alert only eases
//! to a warning once it has been below the critical line for its clear time.
//!
//! An alert raised again within 10 minutes of its resolve twice in 30 minutes is flapping: it
//! keeps its one incident, reads "Flapping · raised 3 times in 10 min", and clears only after 10
//! minutes clear (notify.rs pages it once, gate G16). The count runs until a raise comes more
//! than 10 minutes after the last resolve. An alert restored at launch isn't a re-raise, and
//! doesn't get worse in the minute its data takes to arrive. Nothing holds a node's "back
//! online": its clear is never stretched, and the gate sends it at once (and damps the node's
//! returns once it's flapping, G16b).
//!
//! This Mac's own path to the cluster (reach.rs) comes first: while it's the problem, one
//! mac-reach warning stands for it and the nodes it can't reach read "Offline · not confirmed
//! down" under it, counted and paged nowhere. Such a row stays that through the reconnects after
//! a sleep or a restart of SparkBox. A node that answers but won't let SparkBox in
//! (an untrusted host key, a refused password) is "needs you" information, not an outage.
//!
//! Alerts that share a root cause are one incident (incident.rs), and counts
//! everywhere are incidents. Work SparkBox started is Planned (maintenance.rs):
//! counted nowhere, paged to nobody. Acknowledged and silenced alerts
//! (silence.rs) are known issues: listed, never counted.
//!
//! Everything is kept per data source: the alert log (alerts-live.jsonl,
//! alerts-demo.jsonl, the last 1000 events each), silences, and the active
//! alerts (alerts-active.live.json, …), which come back at launch without being
//! raised (or paged) again. Demo never writes into a Live file.
//!
//! The thresholds and durations are data (rule.rs): the owner edits them in
//! Settings › Alerts, they're saved in rules.json and pushed to the node agents.

pub mod boots;
pub mod incident;
pub mod kernel;
mod memory;
pub mod maintenance;
pub mod reach;
pub mod roce_rules;
pub mod rule;
pub mod rules;
pub mod silence;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::fabric::rates::Rates;
use crate::model::ClusterSnapshot;
use incident::{Incident, IncidentState, RootCtx};
use rule::RuleSet;
pub use rules::EndpointInfo;
use rules::{Condition, Ctx, Trackers};
use silence::Silence;

pub const STATE_EVENT: &str = "health://state";
/// The Live silences changed (the agents get them in agent.json).
pub const SILENCES_EVENT: &str = "health://silences";
/// Alert events kept in memory and in each source's log.
pub const LOG_KEEP: usize = 1000;
/// Alerts restored at launch are held (neither confirmed nor cleared) this long while data arrives.
const RESTORE_GRACE_MS: u64 = 60_000;
/// After `resolve_now`, the same alert isn't raised again until its condition has gone once, or this long.
const SUPPRESS_MS: u64 = 120_000;
/// An alert raised again this soon after it resolved is flapping.
pub const FLAP_WINDOW_MS: u64 = 10 * 60_000;
/// A flapping alert clears only after this long clear (whatever its rule's clear time).
pub const FLAP_CLEAR_MS: u64 = 10 * 60_000;
/// "raised 3 times in 10 min": how far back a flap is counted.
pub const FLAP_COUNT_MS: u64 = 30 * 60_000;
/// Re-raises (each within 10 minutes of a resolve) in that window before it's flapping.
pub const FLAP_RERAISES: usize = 2;
/// A node being back is said at once: its clear is never held for flapping (here or in the gate).
pub const BACK_ONLINE_RULES: [&str; 2] = ["node-offline", "peer-unreachable"];
/// The reason a node-offline resolve carries when the node came back with a new boot_id.
pub const REBOOTED: &str = "It rebooted";
/// Rules whose alerts are Planned while a node they're about is in a maintenance window.
const PLANNED_RULES: [&str; 15] = [
    "node-offline", "node-stale", "fabric-down", "fabric-flap", "net-errors", "port-down", "port-errors", "optic-rx-low", "roce-iface",
    "kernel-mlx5", "agent-silent", "agent-config-stale", "peer-unreachable", "crash-loop", "container-exited",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Shown in the Alerts window, never counted on a node or notified.
    Info,
    Warn,
    Crit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertState {
    Pending,
    Firing,
    Resolved,
}

/// Matches the switches in Settings › Notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// Offline, not updating, memory pressure.
    Node,
    Disk,
    Gpu,
    /// Fabric links, switches, ports and optics.
    Fabric,
    /// Containers, systemd services, inference endpoints.
    Containers,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    /// node | container | service | interface | disk | gpu | switch | port | endpoint
    pub kind: String,
    pub id: String,
    pub node: Option<String>,
    pub switch: Option<String>,
    pub label: String,
}

/// An alert that keeps coming back: how often it was raised in the last 30 minutes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlapInfo {
    pub count: u32,
    /// When the first of those was.
    pub since_ms: u64,
}

impl FlapInfo {
    /// "Flapping · raised 3 times in 10 min".
    pub fn words(&self, now: u64) -> String {
        let mins = (now.saturating_sub(self.since_ms) / 60_000).max(1);
        format!("Flapping · raised {} times in {mins} min", self.count)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub id: String,
    pub rule: String,
    pub category: Category,
    pub severity: Severity,
    pub state: AlertState,
    pub entity: Entity,
    /// Reads after the entity: "Root disk 86% full".
    pub message: String,
    /// Stands alone: "gx10-0000 disk 86%".
    pub summary: String,
    pub detail: Option<String>,
    pub value: Option<f64>,
    pub since_ms: u64,
    pub firing_since_ms: Option<u64>,
    pub resolved_ms: Option<u64>,
    pub acked_ms: Option<u64>,
    /// When the silence covering it ends (9999 for open-ended ones); kept for older readers.
    pub muted_until_ms: Option<u64>,
    /// False for information that's never notified (an idle model).
    #[serde(default = "yes")]
    pub notify: bool,
    // ── r3-incidents ──
    /// The incident it belongs to (its root key).
    #[serde(default)]
    pub incident_id: Option<String>,
    #[serde(default)]
    pub root_key: Option<String>,
    /// Recovering: nothing breaches any more, and it clears at this time.
    #[serde(default)]
    pub recovers_at_ms: Option<u64>,
    /// Planned work: "cell3 restarting (SparkBox, 10:42)".
    #[serde(default)]
    pub planned: Option<String>,
    #[serde(default)]
    pub silence_id: Option<String>,
    /// Counts toward badges, rows, the sidebar, the status bar and the top bar: firing, not
    /// recovering, planned, silenced or acknowledged, and more than information.
    #[serde(default)]
    pub counted: bool,
    /// What it means, in a few words ("check the optic or cable").
    #[serde(default)]
    pub cause: Option<String>,
    /// Why it resolved, when not simply because the condition went: "No data", "Verified: …".
    #[serde(default)]
    pub reason: Option<String>,
    // ── r4-alerts ──
    /// At most 24 characters, for where a sentence doesn't fit (its rule's `short`): "GPU throttled".
    #[serde(default)]
    pub short: String,
    /// It keeps coming back (raised again within 10 minutes of its resolve).
    #[serde(default)]
    pub flapping: Option<FlapInfo>,
    #[serde(skip)]
    for_ms: u64,
    #[serde(skip)]
    clear_ms: u64,
    #[serde(skip)]
    absent_since: Option<u64>,
    /// Below its severity's line since then (hysteresis before easing).
    #[serde(skip)]
    eased_since: Option<u64>,
    /// Restored from disk at launch and not yet seen again.
    #[serde(skip)]
    restored: bool,
}

impl Alert {
    fn from(c: &Condition, now: u64) -> Alert {
        Alert {
            id: c.key.clone(),
            rule: c.rule.clone(),
            category: c.category,
            severity: c.severity,
            state: AlertState::Pending,
            entity: c.entity.clone(),
            message: c.message.clone(),
            summary: c.summary.clone(),
            detail: c.detail.clone(),
            value: c.value,
            since_ms: now,
            firing_since_ms: None,
            resolved_ms: None,
            acked_ms: None,
            muted_until_ms: None,
            notify: c.notify,
            incident_id: None,
            root_key: None,
            recovers_at_ms: None,
            planned: None,
            silence_id: None,
            counted: false,
            cause: c.cause.clone(),
            reason: None,
            short: c.short.clone().unwrap_or_default(),
            flapping: None,
            for_ms: c.for_ms,
            clear_ms: c.clear_ms,
            absent_since: None,
            eased_since: None,
            restored: false,
        }
    }

    /// Not to be paged: silenced, or planned work.
    pub fn muted(&self, now: u64) -> bool {
        self.silence_id.is_some() || self.planned.is_some() || self.muted_until_ms.is_some_and(|t| t > now)
    }

    /// A known issue: acknowledged or silenced.
    pub fn known(&self) -> bool {
        self.silence_id.is_some() || self.acked_ms.is_some()
    }

}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub alert_id: String,
    pub severity: Severity,
    /// The message (reads after the entity's name).
    pub summary: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Crit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityHealth {
    pub level: Level,
    pub issues: Vec<Issue>,
}

/// An acknowledged or silenced alert: listed in one quiet line, never counted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownIssue {
    pub alert_id: String,
    pub incident_id: String,
    pub title: String,
    pub severity: Severity,
    /// silenced | acknowledged
    pub why: String,
    pub until_ms: Option<u64>,
    pub reason: Option<String>,
}

/// An alert waiting out its hold before it fires ("cell3 offline · confirming…").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAlert {
    pub alert_id: String,
    pub title: String,
    pub severity: Severity,
    pub since_ms: u64,
    pub fires_at_ms: u64,
}

/// The counts every surface shows. `warn`, `crit` and `info` count incidents.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    #[serde(default)]
    pub info: u32,
    pub warn: u32,
    pub crit: u32,
    /// Alerts still confirming (see `pending_alerts`).
    pub pending: u32,
    /// Known issues (the round-2 name).
    pub muted: u32,
    // ── r3-incidents ──
    /// Incidents recovering: nothing breaches, the clear hold is running.
    #[serde(default)]
    pub recovering: u32,
    /// Incidents that are planned work.
    #[serde(default)]
    pub planned: u32,
    #[serde(default)]
    pub known: Vec<KnownIssue>,
    #[serde(default)]
    pub pending_alerts: Vec<PendingAlert>,
    /// Counted alerts behind the incidents, for tooltips ("1 critical · 3 alerts").
    #[serde(default)]
    pub alerts: u32,
}

/// What the UI gets (`health_get`, `health://state`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    pub ts_ms: u64,
    pub alerts: Vec<Alert>,
    pub summary: Summary,
    pub nodes: BTreeMap<String, EntityHealth>,
    pub switches: BTreeMap<String, EntityHealth>,
    pub last_alert_ms: Option<u64>,
    // ── r3-incidents ──
    #[serde(default)]
    pub incidents: Vec<Incident>,
    /// The silences of the data source in use.
    #[serde(default)]
    pub silences: Vec<Silence>,
    #[serde(default)]
    pub demo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    Raised,
    Escalated,
    Eased,
    Resolved,
    Acked,
    /// Round-2 mutes, still read from old logs.
    Muted,
    Unmuted,
    Silenced,
    Unsilenced,
    /// A silence ran out while its alert was still firing: it counts (and pages) again.
    SilenceEnded,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertEvent {
    pub ts_ms: u64,
    pub kind: EventKind,
    pub alert: Alert,
    /// Recorded by a node agent while SparkBox was closed ("Fired 03:12 while SparkBox was closed").
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub away: bool,
    /// Who evaluated it: None for this Mac, "agent:<node>" for a node agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    // ── r3-incidents ──
    /// From the Demo simulator: kept in alerts-demo.jsonl, never sent.
    #[serde(default)]
    pub demo: bool,
    #[serde(default)]
    pub incident_id: Option<String>,
    #[serde(default)]
    pub incident_title: Option<String>,
    /// For silences: what was silenced and why ("for 1 week: expanding disk Friday").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl AlertEvent {
    pub fn new(ts_ms: u64, kind: EventKind, alert: Alert) -> AlertEvent {
        let incident_id = alert.incident_id.clone();
        AlertEvent { ts_ms, kind, alert, away: false, source: None, demo: false, incident_id, incident_title: None, note: None }
    }
}

/// "120 GiB"
pub(crate) fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 || v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// "3 days ago", "2 h ago", "5 min ago", "just now".
pub(crate) fn fmt_ago(now_s: u64, then_s: u64) -> String {
    let s = now_s.saturating_sub(then_s);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// An alert as it's saved between launches: with the holds the UI never needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    alert: Alert,
    for_ms: u64,
    clear_ms: u64,
}

/// The pure part: alerts and trackers, advanced one snapshot at a time.
#[derive(Default)]
pub struct Engine {
    active: BTreeMap<String, Alert>,
    trackers: Trackers,
    silences: Vec<Silence>,
    demo: Option<bool>,
    last_alert_ms: Option<u64>,
    rules: Arc<RuleSet>,
    /// Readings for coded rules another module judges (the fabric monitor's cabling and
    /// NCCL transport): metric → (entity, value, message).
    external: rules::External,
    /// The fabric monitor's counter rates for the snapshot being judged.
    rates: Option<Arc<Rates>>,
    incidents: Vec<Incident>,
    /// Alerts `resolve_now` closed: not raised again before this time unless their condition went first.
    suppressed: HashMap<String, u64>,
    /// Alerts restored at launch are held until then.
    restored_until: u64,
    /// Which alerts those are: none of them gets worse before then (`restored_until`).
    restored_keys: HashSet<String>,
    /// The last snapshot and endpoints judged, for views between snapshots (an ack, a silence).
    last_snap: Option<ClusterSnapshot>,
    last_endpoints: Vec<EndpointInfo>,
    /// When each alert key went bad and last resolved, for flapping.
    flaps: HashMap<String, Flaps>,
    /// An escalation ended an acknowledgement since the last evaluation.
    acks_dirty: bool,
}

/// One alert key's comings and goings.
#[derive(Default)]
struct Flaps {
    /// When it was raised (or came back during a flapping alert's clear hold), the last 30 minutes.
    raised: VecDeque<u64>,
    /// Which of those were re-raises: within 10 minutes of its resolve.
    again: VecDeque<u64>,
    resolved_ms: Option<u64>,
    /// Re-raised twice in the window, and not clear for a whole hold since.
    flapping: bool,
}

impl Flaps {
    /// It was raised at `now`; `again`: within 10 minutes of its last resolve. A raise that isn't
    /// starts a new count (a resolve doesn't: the next return may still be part of the same run).
    fn raise(&mut self, now: u64, again: bool) {
        if !again {
            self.raised.clear();
            self.again.clear();
        }
        self.raised.push_back(now);
        if again {
            self.again.push_back(now);
        }
        self.prune(now);
        self.flapping = self.again.len() >= FLAP_RERAISES;
    }

    fn prune(&mut self, now: u64) {
        for q in [&mut self.raised, &mut self.again] {
            while q.front().is_some_and(|t| now.saturating_sub(*t) > FLAP_COUNT_MS) {
                q.pop_front();
            }
        }
    }

    /// "raised 3 times in 10 min", once it's flapping.
    fn info(&self) -> Option<FlapInfo> {
        self.flapping.then_some(())?;
        Some(FlapInfo { count: self.raised.len() as u32, since_ms: *self.raised.front()? })
    }
}

/// Which node windows make an alert Planned: its own node, and for a model (or a container
/// that serves one) every node with a stage of it.
fn touched_nodes(a: &Alert, endpoints: &[EndpointInfo]) -> Vec<String> {
    let mut out: Vec<String> = a.entity.node.iter().cloned().collect();
    let node = a.entity.node.as_deref().unwrap_or("");
    for e in endpoints {
        let mine = (a.entity.kind == "endpoint" && e.id == a.entity.id) || (a.entity.kind == "container" && e.has_member(node, &a.entity.id));
        if mine {
            out.extend(e.nodes.iter().cloned());
            out.push(e.head_node.clone());
        }
    }
    out
}

impl Engine {
    /// Advances to `snap`. Returns what fired, changed severity or resolved.
    #[cfg(test)]
    pub fn step(&mut self, snap: &ClusterSnapshot, now: u64) -> Vec<AlertEvent> {
        self.step_with(snap, &[], now)
    }

    /// Like `step`, also judging the inference servers.
    pub fn step_with(&mut self, snap: &ClusterSnapshot, endpoints: &[EndpointInfo], now: u64) -> Vec<AlertEvent> {
        if self.demo != Some(snap.demo) {
            // Demo and Live are different clusters: start over without announcing anything.
            // (Health swaps in the other source's saved alerts and silences first.)
            if self.demo.is_some() {
                self.set_source(snap.demo, Vec::new(), Vec::new(), now);
            }
            self.demo = Some(snap.demo);
        }
        self.trackers.observe(snap, endpoints, &self.rules, now);
        let active: HashSet<String> = self.active.keys().cloned().collect();
        let firing: HashSet<String> = self.active.values().filter(|a| a.state == AlertState::Firing).map(|a| a.id.clone()).collect();
        let held = self.held();
        let ctx = Ctx { snap, endpoints, now, trackers: &self.trackers, active: &active, firing: &firing, held: &held, rules: &self.rules, external: &self.external, rates: self.rates.as_deref() };
        let (conds, frozen) = rules::evaluate(&ctx);

        let mut raw: Vec<(EventKind, Alert)> = Vec::new();
        let mut seen = HashSet::new();
        for c in &conds {
            seen.insert(c.key.clone());
            if self.suppressed.get(&c.key).is_some_and(|until| *until > now) {
                continue; // closed by a verified fix a moment ago
            }
            let short = c.short.clone().or_else(|| self.rules.get(&c.rule).map(|r| if r.short.is_empty() { r.title.clone() } else { r.short.clone() })).unwrap_or_default();
            let flaps = self.flaps.entry(c.key.clone()).or_default();
            let alert = self.active.entry(c.key.clone()).or_insert_with(|| Alert::from(c, now));
            if alert.state == AlertState::Firing && c.severity > alert.severity && now < self.restored_until && self.restored_keys.contains(&c.key) {
                // Back from the last launch and worse now, before this Mac's view of the cluster
                // has settled (the path monitor's first probes, the first connects): it stays as
                // it was. A node held as not confirmed down isn't made critical by a restart.
                alert.absent_since = None;
                continue;
            }
            if alert.state == AlertState::Firing && alert.absent_since.is_some() && alert.flapping.is_some() {
                // Back while it was waiting out its clear: one more flap on the same alert.
                flaps.raise(now, true);
                alert.flapping = flaps.info();
            }
            if alert.restored {
                // Back from the last launch and confirmed: it was raised once (not re-raised).
                let at = alert.firing_since_ms.unwrap_or(alert.since_ms);
                if now.saturating_sub(at) <= FLAP_COUNT_MS && flaps.raised.is_empty() {
                    flaps.raised.push_back(at);
                }
            }
            let damped = !BACK_ONLINE_RULES.contains(&c.rule.as_str());
            alert.absent_since = None;
            alert.restored = false;
            alert.short = short;
            alert.message.clone_from(&c.message);
            alert.summary.clone_from(&c.summary);
            alert.detail.clone_from(&c.detail);
            alert.entity.label.clone_from(&c.entity.label);
            alert.value = c.value;
            alert.for_ms = c.for_ms;
            alert.clear_ms = c.clear_ms;
            alert.notify = c.notify;
            alert.cause.clone_from(&c.cause);
            if alert.flapping.is_some() && damped {
                alert.clear_ms = alert.clear_ms.max(FLAP_CLEAR_MS);
            }
            match alert.state {
                AlertState::Pending => {
                    alert.severity = c.severity;
                    if now.saturating_sub(alert.since_ms) >= c.for_ms {
                        alert.state = AlertState::Firing;
                        alert.firing_since_ms = Some(now);
                        // Back within 10 minutes of its resolve, the second time in 30 minutes:
                        // flapping, on the same incident.
                        let again = flaps.resolved_ms.is_some_and(|r| now.saturating_sub(r) < FLAP_WINDOW_MS);
                        flaps.raise(now, again);
                        if flaps.flapping {
                            alert.flapping = flaps.info();
                            if damped {
                                alert.clear_ms = alert.clear_ms.max(FLAP_CLEAR_MS);
                            }
                        }
                        raw.push((EventKind::Raised, alert.clone()));
                    }
                }
                AlertState::Firing if c.severity > alert.severity => {
                    alert.severity = c.severity;
                    alert.eased_since = None;
                    // Worse than what was acknowledged: the agents hear the acknowledgement ended.
                    self.acks_dirty |= alert.acked_ms.take().is_some();
                    raw.push((EventKind::Escalated, alert.clone()));
                }
                AlertState::Firing if c.severity < alert.severity => {
                    // Below the line, but it only eases once it has stayed there for the clear time.
                    let since = *alert.eased_since.get_or_insert(now);
                    if now.saturating_sub(since) >= c.clear_ms {
                        alert.severity = c.severity;
                        alert.eased_since = None;
                        raw.push((EventKind::Eased, alert.clone()));
                    }
                }
                AlertState::Firing => alert.eased_since = None,
                AlertState::Resolved => {}
            }
        }
        // A suppressed alert whose condition has gone may come back like any other.
        self.suppressed.retain(|k, until| *until > now && seen.contains(k));
        // Flap memory outlives the alert by the window it's counted over, no longer.
        let active_keys: HashSet<&String> = self.active.keys().collect();
        self.flaps.retain(|k, f| {
            f.prune(now);
            active_keys.contains(k) || f.resolved_ms.is_some_and(|r| now.saturating_sub(r) < FLAP_COUNT_MS)
        });

        let mut resolved = Vec::new();
        for (key, alert) in self.active.iter_mut() {
            if seen.contains(key) {
                continue;
            }
            let scope = match (&alert.entity.switch, &alert.entity.node) {
                (Some(sw), _) => format!("switch:{sw}"),
                (None, Some(node)) => format!("node:{node}"),
                _ => String::new(),
            };
            let own_link = matches!(alert.rule.as_str(), "node-offline" | "node-reboot" | "node-stale" | "switch-unreachable");
            let no_data = rules::RESOLVE_WITHOUT_DATA.contains(&alert.rule.as_str());
            if (frozen.contains(&scope) || frozen.contains(key)) && !own_link && !no_data {
                if matches!(alert.rule.as_str(), "mem-high" | "mem-available" | "mem-pressure" | "mem-swap-in") {
                    if alert.state == AlertState::Pending {
                        resolved.push((key.clone(), false)); // missing readings cannot complete the hold
                    }
                    alert.absent_since = None;
                    alert.eased_since = None;
                }
                continue; // no current data for it: neither confirmed nor cleared
            }
            if alert.restored && now < self.restored_until && !no_data {
                continue; // back from the last launch; its data hasn't arrived yet
            }
            if alert.state == AlertState::Pending {
                resolved.push((key.clone(), false));
                continue;
            }
            let since = *alert.absent_since.get_or_insert(now);
            if now.saturating_sub(since) >= alert.clear_ms {
                resolved.push((key.clone(), true));
            }
        }
        let mut gone = Vec::new();
        for (key, was_firing) in resolved {
            if let Some(mut alert) = self.active.remove(&key) {
                if was_firing {
                    let f = self.flaps.entry(key.clone()).or_default();
                    // Restored at launch and never seen since: its resolve isn't one a re-raise follows.
                    if !alert.restored {
                        f.resolved_ms = Some(now);
                    }
                    if alert.rule == "node-offline" && alert.entity.node.as_deref().is_some_and(|n| self.trackers.rebooted_since(n, alert.since_ms)) {
                        // It came back with a new boot_id: "cell3 is back · it rebooted".
                        alert.reason = Some(REBOOTED.into());
                    }
                    alert.state = AlertState::Resolved;
                    alert.resolved_ms = Some(now);
                    alert.recovers_at_ms = None;
                    let node_quiet = alert.entity.node.as_ref().is_some_and(|n| frozen.contains(&format!("node:{n}")));
                    let no_rates = self.rates.as_ref().is_none_or(|r| r.ts_ms + 15_000 < now);
                    if rules::RESOLVE_WITHOUT_DATA.contains(&alert.rule.as_str()) && (node_quiet || no_rates) {
                        alert.reason = Some("No data".into());
                    }
                    gone.push(alert);
                }
            }
        }

        self.last_snap = Some(snap.clone());
        self.last_endpoints = endpoints.to_vec();
        // Silences that ran out: the alerts they covered count (and page) again.
        let ended: Vec<Silence> = self.silences.iter().filter(|s| s.expired(now)).cloned().collect();
        self.silences.retain(|s| !s.expired(now));
        self.derive(now, &mut gone);
        for s in &ended {
            for a in self.active.values().filter(|a| a.state == AlertState::Firing && a.silence_id.is_none()) {
                if s.covers(a, a.incident_id.as_deref().unwrap_or("")) {
                    raw.push((EventKind::SilenceEnded, a.clone()));
                }
            }
        }
        // "Until it clears": done once nothing it covers is active.
        let before = self.silences.len();
        let active_now: Vec<&Alert> = self.active.values().collect();
        self.silences.retain(|s| s.until != silence::Until::Resolved || active_now.iter().any(|a| s.covers(a, a.incident_id.as_deref().unwrap_or(""))));
        let cleared_silences = self.silences.len() != before || !ended.is_empty();

        let mut events: Vec<AlertEvent> = Vec::new();
        for (kind, a) in raw {
            // The event carries what the alert is now (planned, silenced, its incident).
            let a = self.active.get(&a.id).cloned().map(|cur| Alert { severity: a.severity, ..cur }).unwrap_or(a);
            events.push(self.event(now, kind, a, snap.demo));
        }
        for a in gone {
            events.push(self.event(now, EventKind::Resolved, a, snap.demo));
        }
        if !events.is_empty() || cleared_silences {
            self.last_alert_ms = Some(now);
        }
        events
    }

    /// An event stamped with its incident and data source.
    fn event(&self, now: u64, kind: EventKind, a: Alert, demo: bool) -> AlertEvent {
        let mut e = AlertEvent::new(now, kind, a);
        e.demo = demo;
        e.incident_title = e.incident_id.as_ref().and_then(|id| self.incidents.iter().find(|i| &i.id == id)).map(|i| i.title.clone()).or_else(|| Some(e.alert.summary.clone()));
        e
    }

    /// Works out each alert's incident, Planned, silence, Recovering and whether it counts,
    /// then the incidents. `gone` are alerts resolving this step (they get their incident too).
    fn derive(&mut self, now: u64, gone: &mut [Alert]) {
        let empty = ClusterSnapshot::default();
        let snap = self.last_snap.as_ref().unwrap_or(&empty);
        let endpoints = &self.last_endpoints;
        let grace = self.rules.th("node-reboot", None).for_ms;
        let roots = {
            let refs: Vec<&Alert> = self.active.values().collect();
            RootCtx::new(snap, endpoints, &refs)
        };
        let silences = &self.silences;
        let restarts = maintenance::endpoint_windows(self.demo.unwrap_or(true));
        let derive_one = |a: &mut Alert| {
            let fresh = roots.root_of(a);
            // An alert that joined a node's incident stays in it until it clears, even when the
            // node is back first: its clear goes with that incident's and never as one of its own
            // ("Resolved: gemma4:latest is down" beside "cell3 is back online", seen in Live on Oct 29).
            // (This Mac losing the cluster takes a node's alerts into its incident, and gives them back.)
            let root = match a.incident_id.as_deref() {
                Some(old) if old != fresh && old.starts_with("node:") && !fresh.starts_with("mac:") => old.to_string(),
                _ => fresh,
            };
            a.root_key = Some(root.clone());
            a.incident_id = Some(root.clone());
            let plannable = PLANNED_RULES.contains(&a.rule.as_str()) || a.rule.starts_with("endpoint-");
            a.planned = plannable
                .then(|| {
                    touched_nodes(a, endpoints).iter().find_map(|id| {
                        let n = snap.nodes.iter().find(|n| &n.id == id)?;
                        let m = n.maintenance.as_ref().filter(|m| maintenance::planned_now(m, now, grace))?;
                        Some(maintenance::planned_text(&n.name, m))
                    })
                })
                .flatten()
                .or_else(|| {
                    // A model SparkBox restarts in order: its own alerts and its containers'.
                    let node = a.entity.node.as_deref().unwrap_or("");
                    restarts.iter().find_map(|(id, m)| {
                        let e = endpoints.iter().find(|e| &e.id == id);
                        let mine = (a.entity.kind == "endpoint" && &a.entity.id == id) || (a.entity.kind == "container" && m.nodes.iter().any(|n| n == node) && e.is_some_and(|e| e.has_member(node, &a.entity.id)));
                        mine.then(|| maintenance::endpoint_text(e.map(|e| if e.model.is_empty() { e.label.as_str() } else { e.model.as_str() }).unwrap_or(id), m))
                    })
                });
            if a.planned.is_none() {
                let node = a.entity.node.as_deref().unwrap_or("");
                a.planned = endpoints.iter().find(|e| e.maintenance_hold && ((a.entity.kind == "endpoint" && a.entity.id == e.id) || (a.entity.kind == "container" && e.has_member(node, &a.entity.id))))
                    .map(|e| format!("{} paused for rolling maintenance", e.name));
            }
            let s = silences.iter().find(|s| s.covers(a, &root));
            a.silence_id = s.map(|s| s.id.clone());
            a.muted_until_ms = s.map(|s| s.until_ms());
            a.recovers_at_ms = match (a.state, a.absent_since) {
                (AlertState::Firing, Some(since)) => Some(since + a.clear_ms),
                _ => None,
            };
            a.counted = a.state == AlertState::Firing
                && a.recovers_at_ms.is_none()
                && a.planned.is_none()
                && a.silence_id.is_none()
                && a.acked_ms.is_none()
                && a.severity != Severity::Info;
        };
        for a in self.active.values_mut() {
            derive_one(a);
        }
        for a in gone.iter_mut() {
            derive_one(a);
        }
        let list: Vec<Alert> = self.active.values().cloned().collect();
        self.incidents = incident::build(&list, &roots);
    }

    pub fn view(&self, now: u64, snap: Option<&ClusterSnapshot>) -> HealthView {
        let mut alerts: Vec<Alert> = self.active.values().cloned().collect();
        alerts.sort_by(|a, b| {
            let rank = |x: &Alert| (x.counted, x.state == AlertState::Firing, x.severity);
            rank(b).cmp(&rank(a)).then(a.since_ms.cmp(&b.since_ms)).then(a.id.cmp(&b.id))
        });
        let mut summary = Summary::default();
        let mut nodes: BTreeMap<String, EntityHealth> = BTreeMap::new();
        let mut switches: BTreeMap<String, EntityHealth> = BTreeMap::new();
        if let Some(s) = snap.or(self.last_snap.as_ref()) {
            for n in &s.nodes {
                nodes.insert(n.id.clone(), EntityHealth { level: Level::Ok, issues: Vec::new() });
            }
            for sw in &s.switches {
                switches.insert(sw.id.clone(), EntityHealth { level: Level::Ok, issues: Vec::new() });
            }
        }
        for i in &self.incidents {
            match i.state {
                IncidentState::Firing => match i.severity {
                    Severity::Crit => summary.crit += 1,
                    Severity::Warn => summary.warn += 1,
                    Severity::Info => summary.info += 1,
                },
                IncidentState::Recovering => summary.recovering += 1,
                IncidentState::Planned => summary.planned += 1,
                IncidentState::Info => summary.info += 1,
                IncidentState::Silenced | IncidentState::Acknowledged => {}
            }
        }
        for a in &alerts {
            if a.state == AlertState::Pending && a.planned.is_none() && a.silence_id.is_none() {
                summary.pending += 1;
                summary.pending_alerts.push(PendingAlert { alert_id: a.id.clone(), title: a.summary.clone(), severity: a.severity, since_ms: a.since_ms, fires_at_ms: a.since_ms + a.for_ms });
            }
            if a.state == AlertState::Firing && a.known() && a.planned.is_none() {
                let silence = a.silence_id.as_ref().and_then(|id| self.silences.iter().find(|s| &s.id == id));
                summary.known.push(KnownIssue {
                    alert_id: a.id.clone(),
                    incident_id: a.incident_id.clone().unwrap_or_default(),
                    title: a.summary.clone(),
                    severity: a.severity,
                    why: if silence.is_some() { "silenced" } else { "acknowledged" }.into(),
                    until_ms: silence.and_then(|s| matches!(s.until, silence::Until::At { .. }).then(|| s.until_ms())),
                    reason: silence.map(|s| s.reason.clone()).filter(|r| !r.is_empty()),
                });
            }
            if !a.counted {
                continue;
            }
            summary.alerts += 1;
            let issue = Issue { alert_id: a.id.clone(), severity: a.severity, summary: a.message.clone(), label: a.entity.label.clone() };
            let lv = if a.severity == Severity::Crit { Level::Crit } else { Level::Warn };
            let add = |map: &mut BTreeMap<String, EntityHealth>, id: &str, issue: Issue| {
                let e = map.entry(id.to_string()).or_insert(EntityHealth { level: Level::Ok, issues: Vec::new() });
                if lv == Level::Crit || e.level == Level::Ok {
                    e.level = lv;
                }
                e.issues.push(issue);
            };
            // A node's colour describes the node: a model or a container on it doesn't paint it.
            if let (Some(node), false) = (&a.entity.node, matches!(a.entity.kind.as_str(), "endpoint" | "container")) {
                add(&mut nodes, node, issue.clone());
            }
            if let Some(sw) = &a.entity.switch {
                add(&mut switches, sw, issue);
            }
        }
        summary.muted = summary.known.len() as u32;
        HealthView {
            ts_ms: now,
            alerts,
            summary,
            nodes,
            switches,
            last_alert_ms: self.last_alert_ms,
            incidents: self.incidents.clone(),
            silences: self.silences.clone(),
            demo: self.demo.unwrap_or(true),
        }
    }

    /// Acknowledges an alert, or every alert of an incident.
    fn ack(&mut self, id: &str, now: u64) -> Vec<Alert> {
        let ids: Vec<String> = match self.incidents.iter().find(|i| i.id == id) {
            Some(i) => i.children.clone(),
            None => vec![id.to_string()],
        };
        let mut out = Vec::new();
        for k in ids {
            if let Some(a) = self.active.get_mut(&k).filter(|a| a.state == AlertState::Firing && a.acked_ms.is_none()) {
                a.acked_ms = Some(now);
                out.push(a.clone());
            }
        }
        self.derive(now, &mut []);
        out.into_iter().map(|a| self.active.get(&a.id).cloned().unwrap_or(a)).collect()
    }

    /// The alerts a silence covers right now.
    fn covered(&self, s: &Silence) -> Vec<Alert> {
        self.active.values().filter(|a| s.covers(a, a.incident_id.as_deref().unwrap_or(""))).cloned().collect()
    }

    /// Adds a silence, or changes the one that already silences the same thing (keeping its id).
    fn add_silence(&mut self, s: Silence, now: u64) -> (Silence, Vec<Alert>) {
        let s = match self.silences.iter_mut().find(|x| x.matcher.same_as(&s.matcher)) {
            Some(x) => {
                (x.until, x.reason, x.duration, x.label, x.max_ms) = (s.until, s.reason, s.duration, s.label, s.max_ms);
                x.clone()
            }
            None => {
                self.silences.push(s.clone());
                s
            }
        };
        self.derive(now, &mut []);
        let covered = self.covered(&s);
        (s, covered)
    }

    /// Ends a silence and every other one on the same alerts (the same matcher, its incident,
    /// or any alert it covers), so nothing it hid stays hidden.
    fn remove_silence(&mut self, id: &str, now: u64) -> Option<(Silence, Vec<Alert>)> {
        let s = self.silences.iter().find(|s| s.id == id)?.clone();
        let covered = self.covered(&s);
        let incidents: HashSet<String> = covered.iter().filter_map(|a| a.incident_id.clone()).chain(s.matcher.incident.clone()).collect();
        self.silences.retain(|x| {
            let same = x.id == s.id || x.matcher.same_as(&s.matcher) || x.matcher.incident.as_ref().is_some_and(|i| incidents.contains(i));
            !(same || covered.iter().any(|a| x.covers(a, a.incident_id.as_deref().unwrap_or(""))))
        });
        self.derive(now, &mut []);
        let covered = covered.into_iter().map(|a| self.active.get(&a.id).cloned().unwrap_or(a)).collect();
        Some((s, covered))
    }

    /// Closes matching alerts at once ("Verified: llama-3.3-70b answered after 26 s"). The
    /// same condition can't raise them again until it has gone, or for two minutes.
    fn resolve_now(&mut self, rule: &str, entity_id: &str, reason: &str, now: u64) -> Vec<Alert> {
        let keys: Vec<String> = self.active.values().filter(|a| a.rule == rule && (a.entity.id == entity_id || a.id == entity_id)).map(|a| a.id.clone()).collect();
        let mut out = Vec::new();
        for k in keys {
            let Some(mut a) = self.active.remove(&k) else { continue };
            if rule == "crash-loop" {
                if let Some(node) = &a.entity.node {
                    self.trackers.forget_restarts(node, &a.entity.id);
                }
            }
            self.suppressed.insert(k, now + SUPPRESS_MS);
            if a.state == AlertState::Firing {
                a.state = AlertState::Resolved;
                a.resolved_ms = Some(now);
                a.recovers_at_ms = None;
                a.counted = false;
                a.reason = Some(reason.to_string());
                out.push(a);
            }
        }
        self.derive(now, &mut out);
        out
    }

    /// Planned work on `node` is over (after Verify): its Planned alerts resolve without paging.
    fn end_planned(&mut self, node: &str, now: u64) -> Vec<Alert> {
        let endpoints = self.last_endpoints.clone();
        let keys: Vec<String> = self.active.values().filter(|a| a.planned.is_some() && touched_nodes(a, &endpoints).iter().any(|n| n == node)).map(|a| a.id.clone()).collect();
        let mut out = Vec::new();
        for k in keys {
            let Some(mut a) = self.active.remove(&k) else { continue };
            if a.state == AlertState::Firing {
                a.state = AlertState::Resolved;
                a.resolved_ms = Some(now);
                a.counted = false;
                a.reason = Some("The planned work is over".into());
                out.push(a);
            }
        }
        if let Some(n) = self.last_snap.as_mut().and_then(|s| s.nodes.iter_mut().find(|n| n.id == node)) {
            n.maintenance = None;
        }
        self.derive(now, &mut []);
        out
    }

    pub fn set_rules(&mut self, rules: Arc<RuleSet>) {
        self.rules = rules;
    }

    pub fn set_external(&mut self, external: rules::External) {
        self.external = external;
    }

    pub fn set_rates(&mut self, rates: Option<Arc<Rates>>) {
        self.rates = rates;
    }

    /// Switches to a data source with its own saved alerts and silences (nothing is announced).
    fn set_source(&mut self, demo: bool, saved: Vec<Saved>, silences: Vec<Silence>, now: u64) {
        self.active.clear();
        self.trackers = Trackers::default();
        self.suppressed.clear();
        self.incidents.clear();
        self.flaps.clear();
        for s in saved {
            let mut a = s.alert;
            (a.for_ms, a.clear_ms, a.restored) = (s.for_ms, s.clear_ms, true);
            a.counted = false;
            // Its flaps were counted in the last run: in this one it hasn't come back yet.
            a.flapping = None;
            self.active.insert(a.id.clone(), a);
        }
        self.restored_keys = self.active.keys().cloned().collect();
        self.restored_until = now + RESTORE_GRACE_MS;
        self.silences = silences;
        self.demo = Some(demo);
    }

    /// The node rows shown as "not confirmed down" (information under this Mac's reach problem):
    /// what a reconnect shows them never makes them critical (rules.rs node_link).
    fn held(&self) -> HashSet<String> {
        self.active.values().filter(|a| a.severity == Severity::Info && matches!(a.rule.as_str(), "node-offline" | "node-stale")).map(|a| a.id.clone()).collect()
    }

    /// The active alerts as they're saved for the next launch.
    fn saved(&self) -> Vec<Saved> {
        self.active.values().filter(|a| a.state == AlertState::Firing).map(|a| Saved { alert: a.clone(), for_ms: a.for_ms, clear_ms: a.clear_ms }).collect()
    }

    /// What `candidate` would raise right now: the current snapshot judged with the
    /// candidate's lines, using the trackers' windows and the alerts already pending.
    pub fn preview(&self, snap: &ClusterSnapshot, endpoints: &[EndpointInfo], candidate: &RuleSet, now: u64) -> Vec<PreviewRow> {
        let active: HashSet<String> = self.active.keys().cloned().collect();
        let firing: HashSet<String> = self.active.values().filter(|a| a.state == AlertState::Firing).map(|a| a.id.clone()).collect();
        let held = self.held();
        let ctx = Ctx { snap, endpoints, now, trackers: &self.trackers, active: &active, firing: &firing, held: &held, rules: candidate, external: &self.external, rates: self.rates.as_deref() };
        let (conds, _) = rules::evaluate(&ctx);
        candidate
            .rules
            .iter()
            .map(|r| {
                let matches: Vec<PreviewMatch> = conds
                    .iter()
                    .filter(|c| c.rule == r.id)
                    .map(|c| {
                        let held = self.active.get(&c.key).map(|a| now.saturating_sub(a.since_ms)).unwrap_or(0);
                        PreviewMatch {
                            label: c.entity.label.clone(),
                            severity: c.severity,
                            message: c.message.clone(),
                            value: c.value,
                            held_s: held / 1000,
                            fires_in_s: c.for_ms.saturating_sub(held).div_ceil(1000),
                        }
                    })
                    .collect();
                let known = rules::understood(r, snap, endpoints, &self.external) || (r.id == roce_rules::RULE && self.rates.as_ref().is_some_and(|x| x.ifaces.values().any(|i| !i.counters.is_empty())));
                PreviewRow { id: r.id.clone(), known, matches, unreported: rules::unreported(r, snap) }
            })
            .collect()
    }
}

/// One rule's "would fire now" in Settings › Alerts.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub id: String,
    /// False when nothing in the current data has this rule's metric.
    pub known: bool,
    pub matches: Vec<PreviewMatch>,
    /// Nodes whose data lacks this node rule's metric, by name ("Not reported by this node").
    pub unreported: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewMatch {
    pub label: String,
    pub severity: Severity,
    pub message: String,
    pub value: Option<f64>,
    /// How long the condition has held already.
    pub held_s: u64,
    /// 0: it would be firing now; otherwise how much longer it must hold.
    pub fires_in_s: u64,
}

/// One data source's alert log: the last 1000 events in memory, appended to its file.
struct Log {
    path: Option<PathBuf>,
    events: VecDeque<AlertEvent>,
    lines_on_disk: usize,
}

impl Log {
    fn open(path: Option<PathBuf>) -> Log {
        let mut events = VecDeque::new();
        let mut lines = 0;
        if let Some(text) = path.as_ref().and_then(|p| fs::read_to_string(p).ok()) {
            for line in text.lines() {
                lines += 1;
                if let Ok(e) = serde_json::from_str::<AlertEvent>(line) {
                    events.push_back(e);
                    if events.len() > LOG_KEEP {
                        events.pop_front();
                    }
                }
            }
        }
        let mut log = Log { path, events, lines_on_disk: lines };
        if lines > LOG_KEEP {
            log.rewrite();
        }
        log
    }

    fn rewrite(&mut self) {
        let Some(path) = &self.path else { return };
        let text: String = self.events.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
        if crate::util::write_atomic(path, text.as_bytes()).is_ok() {
            self.lines_on_disk = self.events.len();
        }
    }

    fn append(&mut self, e: AlertEvent) {
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let (Ok(line), Ok(mut f)) = (serde_json::to_string(&e), fs::OpenOptions::new().create(true).append(true).open(path)) {
                if writeln!(f, "{line}").is_ok() {
                    self.lines_on_disk += 1;
                }
            }
        }
        self.events.push_back(e);
        if self.events.len() > LOG_KEEP {
            self.events.pop_front();
        }
        // Let the file grow to twice the limit, then compact it in one write.
        if self.lines_on_disk > 2 * LOG_KEEP {
            self.rewrite();
        }
    }

    fn last_change(&self) -> Option<u64> {
        self.events.iter().filter(|e| matches!(e.kind, EventKind::Raised | EventKind::Resolved)).map(|e| e.ts_ms).max()
    }
}

/// The two logs: Live's and Demo's.
struct Logs {
    live: Log,
    demo: Log,
}

impl Logs {
    fn of(&mut self, demo: bool) -> &mut Log {
        if demo {
            &mut self.demo
        } else {
            &mut self.live
        }
    }
}

/// The managed health engine.
pub struct Health {
    engine: Mutex<Engine>,
    logs: Mutex<Logs>,
    dir: Option<PathBuf>,
    silences: silence::Store,
    rules_path: Option<PathBuf>,
    rules: Mutex<Arc<RuleSet>>,
    last: Mutex<(String, Option<ClusterSnapshot>)>,
    endpoints: Mutex<Vec<EndpointInfo>>,
    /// Events from outside an evaluation (a verified fix, the end of planned work), handed to the
    /// next evaluation so they're notified like any other.
    queued: Mutex<Vec<AlertEvent>>,
    /// What was last saved as the active alerts, to skip identical writes.
    saved_key: Mutex<String>,
    /// Pages a node's agent sent (from its imported events), for the notifier's gate: the Mac
    /// coalesces its own warnings into an incident an agent paged (G17).
    agent_pages: Mutex<Vec<AgentPage>>,
    // ── r1-h ──
    /// This Mac's path to the cluster (reach.rs): its probes and verdict.
    pub reach: reach::Monitor,
}

/// An incident a node's agent paged.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentPage {
    pub demo: bool,
    pub incident: String,
    pub severity: Severity,
    pub title: String,
    pub ts_ms: u64,
}

/// What one evaluation produced.
pub struct Evaluation {
    /// The new view, when anything visible changed.
    pub view: Option<HealthView>,
    pub events: Vec<AlertEvent>,
    /// An escalation ended an acknowledgement (the agents must hear it).
    pub acks_changed: bool,
}

fn now_ms() -> u64 {
    crate::history::now_ms()
}

fn source_word(demo: bool) -> &'static str {
    if demo {
        "demo"
    } else {
        "live"
    }
}

impl Health {
    /// `dir` holds the alert logs, silences, active alerts and rules.json; None keeps everything
    /// in memory (tests).
    pub fn new(dir: Option<PathBuf>) -> Health {
        let rules_path = dir.as_ref().map(|d| d.join("rules.json"));
        let rules = Arc::new(rules_path.as_deref().map(RuleSet::load).unwrap_or_default());
        let mut engine = Engine { rules: rules.clone(), ..Engine::default() };
        if let Some(d) = &dir {
            // Round 2 kept one log for both sources; it can't be split now, so it's set aside.
            let (old, live) = (d.join("alerts.jsonl"), d.join("alerts-live.jsonl"));
            if old.exists() && !live.exists() {
                let kept = d.join("alerts-before-r3.jsonl");
                if fs::rename(&old, &kept).is_ok() {
                    eprintln!("health: set {} aside as {} (it mixed Demo and Live events)", old.display(), kept.display());
                }
            }
        }
        let logs = Logs { live: Log::open(dir.as_ref().map(|d| d.join("alerts-live.jsonl"))), demo: Log::open(dir.as_ref().map(|d| d.join("alerts-demo.jsonl"))) };
        let silences = silence::Store::new(dir.clone());
        if let Some(d) = &dir {
            maintenance::keep_in(d);
        }
        engine.last_alert_ms = logs.demo.last_change();
        Health {
            engine: Mutex::new(engine),
            logs: Mutex::new(logs),
            dir,
            silences,
            rules_path,
            rules: Mutex::new(rules),
            last: Mutex::new((String::new(), None)),
            endpoints: Mutex::new(Vec::new()),
            queued: Mutex::new(Vec::new()),
            saved_key: Mutex::new(String::new()),
            agent_pages: Mutex::new(Vec::new()),
            reach: reach::Monitor::default(),
        }
    }

    fn active_path(&self, demo: bool) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("alerts-active.{}.json", source_word(demo))))
    }

    fn load_active(&self, demo: bool) -> Vec<Saved> {
        self.active_path(demo).and_then(|p| crate::util::load_json_or_quarantine::<Vec<Saved>>(&p).ok().flatten()).unwrap_or_default()
    }

    /// Saves the active alerts of the source in use, when they changed.
    fn save_active(&self, engine: &Engine) {
        let Some(demo) = engine.demo else { return };
        let saved = engine.saved();
        let key = format!("{demo}|{}", saved.iter().map(|s| format!("{}:{:?}:{:?}:{:?}", s.alert.id, s.alert.severity, s.alert.acked_ms, s.alert.state)).collect::<Vec<_>>().join(";"));
        let mut last = self.saved_key.lock().unwrap();
        if *last == key {
            return;
        }
        *last = key;
        if let Some(p) = self.active_path(demo) {
            if let Err(e) = crate::util::write_atomic(&p, &serde_json::to_vec(&saved).unwrap_or_default()) {
                eprintln!("health: couldn't save {}: {e}", p.display());
            }
        }
    }

    /// Runs the rules over a snapshot; logs what fired or resolved.
    #[cfg(test)]
    pub fn evaluate(&self, snap: &ClusterSnapshot) -> Evaluation {
        self.evaluate_with(snap, &[])
    }

    /// Readings for coded rules judged elsewhere (the fabric monitor), used from the next evaluation.
    pub fn set_external(&self, external: rules::External) {
        self.engine.lock().unwrap().set_external(external);
    }

    /// The fabric monitor's counter rates for the snapshot about to be evaluated.
    pub fn set_rates(&self, rates: Option<Arc<Rates>>) {
        self.engine.lock().unwrap().set_rates(rates);
    }

    /// Runs the rules over a snapshot and the inference servers; logs what fired or resolved.
    pub fn evaluate_with(&self, snap: &ClusterSnapshot, endpoints: &[EndpointInfo]) -> Evaluation {
        let now = snap.ts_ms;
        let mut engine = self.engine.lock().unwrap();
        if engine.demo.is_some_and(|d| d != snap.demo) {
            // The other data source: put this one's alerts away and take that one's out.
            self.save_active(&engine);
            let (saved, silences) = (self.load_active(snap.demo), self.silences.load(snap.demo));
            engine.set_source(snap.demo, saved, silences, now);
            engine.last_alert_ms = self.logs.lock().unwrap().of(snap.demo).last_change();
        } else if engine.demo.is_none() {
            let (saved, silences) = (self.load_active(snap.demo), self.silences.load(snap.demo));
            engine.set_source(snap.demo, saved, silences, now);
            engine.last_alert_ms = self.logs.lock().unwrap().of(snap.demo).last_change();
        }
        let silences_before = engine.silences.len();
        let mut events = std::mem::take(&mut *self.queued.lock().unwrap());
        events.retain(|e| e.demo == snap.demo);
        events.extend(engine.step_with(snap, endpoints, now));
        let acks_changed = std::mem::take(&mut engine.acks_dirty);
        if engine.silences.len() != silences_before {
            self.silences.save(snap.demo, &engine.silences);
        }
        self.save_active(&engine);
        let view = engine.view(now, Some(snap));
        drop(engine);
        if !events.is_empty() {
            let mut logs = self.logs.lock().unwrap();
            for e in events.iter().filter(|e| !matches!(e.kind, EventKind::Acked | EventKind::Silenced | EventKind::Unsilenced)) {
                logs.of(e.demo).append(e.clone());
            }
        }
        let key = view_key(&view);
        let mut last = self.last.lock().unwrap();
        let changed = last.0 != key;
        *last = (key, Some(snap.clone()));
        *self.endpoints.lock().unwrap() = endpoints.to_vec();
        Evaluation { view: changed.then_some(view), events, acks_changed }
    }

    pub fn view(&self) -> HealthView {
        let last = self.last.lock().unwrap();
        let now = last.1.as_ref().map(|s| s.ts_ms).unwrap_or_else(now_ms);
        self.engine.lock().unwrap().view(now.max(now_ms()), last.1.as_ref())
    }

    /// The last snapshot the rules saw.
    pub fn last_snapshot(&self) -> Option<ClusterSnapshot> {
        self.last.lock().unwrap().1.clone()
    }

    fn demo(&self) -> bool {
        self.engine.lock().unwrap().demo.unwrap_or(true)
    }

    /// The most recent `limit` events of the data source in use, newest first.
    pub fn history(&self, limit: usize) -> Vec<AlertEvent> {
        let demo = self.demo();
        self.logs.lock().unwrap().of(demo).events.iter().rev().take(limit).cloned().collect()
    }

    /// Records events made outside an evaluation in the log of their source.
    fn record(&self, events: &[AlertEvent]) {
        let mut logs = self.logs.lock().unwrap();
        for e in events {
            logs.of(e.demo).append(e.clone());
        }
    }

    /// Acknowledges an alert, or every alert in an incident: no more repeats, and it no longer
    /// counts (an escalation un-acknowledges it).
    pub fn ack(&self, id: &str) -> Result<(HealthView, Vec<Alert>), String> {
        let now = now_ms();
        let mut engine = self.engine.lock().unwrap();
        let demo = engine.demo.unwrap_or(true);
        let alerts = engine.ack(id, now);
        if alerts.is_empty() {
            return Err("That alert has already cleared or is acknowledged.".into());
        }
        self.save_active(&engine);
        drop(engine);
        let events: Vec<AlertEvent> = alerts.iter().map(|a| AlertEvent { demo, ..AlertEvent::new(now, EventKind::Acked, a.clone()) }).collect();
        self.record(&events);
        self.touch();
        Ok((self.view(), alerts))
    }

    /// Adds a silence (validated: only the five durations, a reason for the long ones).
    pub fn silence(&self, matcher: silence::Matcher, duration: &str, reason: &str, label: &str) -> Result<(HealthView, Silence), String> {
        let now = now_ms();
        let mut engine = self.engine.lock().unwrap();
        let demo = engine.demo.unwrap_or(true);
        let s = silence::make(matcher, duration, reason, label, now, demo)?;
        let (s, covered) = engine.add_silence(s, now);
        self.silences.save(demo, &engine.silences);
        drop(engine);
        let note = format!("{}{}", silence::duration_words(duration), if s.reason.is_empty() { String::new() } else { format!(": {}", s.reason) });
        let events: Vec<AlertEvent> = covered.into_iter().map(|a| AlertEvent { demo, note: Some(note.clone()), ..AlertEvent::new(now, EventKind::Silenced, a) }).collect();
        self.record(&events);
        self.touch();
        Ok((self.view(), s))
    }

    /// Ends a silence early. Alerts it covered that still fire count (and page) again.
    pub fn unsilence(&self, id: &str) -> Result<(HealthView, Silence), String> {
        let now = now_ms();
        let mut engine = self.engine.lock().unwrap();
        let demo = engine.demo.unwrap_or(true);
        let (s, covered) = engine.remove_silence(id, now).ok_or("That silence has already ended.")?;
        self.silences.save(demo, &engine.silences);
        drop(engine);
        let events: Vec<AlertEvent> = covered.into_iter().map(|a| AlertEvent { demo, ..AlertEvent::new(now, EventKind::Unsilenced, a) }).collect();
        self.record(&events);
        self.touch();
        Ok((self.view(), s))
    }

    /// The Live silences (what the node agents get in agent.json).
    pub fn silences(&self) -> Vec<Silence> {
        let engine = self.engine.lock().unwrap();
        if engine.demo == Some(false) {
            return engine.silences.clone();
        }
        drop(engine);
        let now = now_ms();
        self.silences.load(false).into_iter().filter(|s| !s.expired(now)).collect()
    }

    /// Closes the matching alerts at once with `reason` (r3-serving after Verify, the container
    /// verify after a restart): "Verified: llama-3.3-70b answered after 26 s". The resolutions
    /// are logged and notified with the next evaluation.
    pub fn resolve_now(&self, rule: &str, entity_id: &str, reason: &str) {
        let now = now_ms();
        let mut engine = self.engine.lock().unwrap();
        let demo = engine.demo.unwrap_or(true);
        let alerts = engine.resolve_now(rule, entity_id, reason, now);
        if alerts.is_empty() {
            return;
        }
        engine.last_alert_ms = Some(now);
        self.save_active(&engine);
        let events: Vec<AlertEvent> = alerts.into_iter().map(|a| engine.event(now, EventKind::Resolved, a, demo)).collect();
        drop(engine);
        // The next evaluation logs and notifies them with its own.
        self.queued.lock().unwrap().extend(events);
        self.touch();
    }

    /// The maintenance window on `node` was closed: its Planned alerts resolve silently.
    pub fn end_maintenance(&self, node: &str, demo: bool) {
        let now = now_ms();
        let mut engine = self.engine.lock().unwrap();
        if engine.demo != Some(demo) {
            return;
        }
        let alerts = engine.end_planned(node, now);
        self.save_active(&engine);
        let events: Vec<AlertEvent> = alerts.into_iter().map(|a| engine.event(now, EventKind::Resolved, a, demo)).collect();
        drop(engine);
        self.queued.lock().unwrap().extend(events);
        self.touch();
    }

    pub fn rules(&self) -> Arc<RuleSet> {
        self.rules.lock().unwrap().clone()
    }

    /// Validates, saves (rules.json) and applies a new rule set.
    pub fn set_rules(&self, candidate: RuleSet) -> Result<Arc<RuleSet>, String> {
        candidate.validate()?;
        let merged = Arc::new(RuleSet::merged_onto_defaults(&candidate));
        if let Some(path) = &self.rules_path {
            merged.save(path)?;
        }
        *self.rules.lock().unwrap() = merged.clone();
        self.engine.lock().unwrap().set_rules(merged.clone());
        self.touch();
        Ok(merged)
    }

    /// "Would fire now" for a candidate rule set, against the last snapshot.
    pub fn preview(&self, candidate: &RuleSet) -> Vec<PreviewRow> {
        let snap = self.last.lock().unwrap().1.clone().unwrap_or_default();
        let endpoints = self.endpoints.lock().unwrap().clone();
        let now = if snap.ts_ms > 0 { snap.ts_ms } else { now_ms() };
        self.engine.lock().unwrap().preview(&snap, &endpoints, candidate, now)
    }

    /// Adds events a node agent recorded (while SparkBox was closed, or since the last
    /// import) to the alert log of the source in use, once each. Returns how many were new.
    pub fn import(&self, events: Vec<AlertEvent>) -> usize {
        let demo = self.demo();
        self.import_from(events, demo)
    }

    /// Like `import`, into one source's log: Demo's simulated agents never write into Live's.
    pub fn import_from(&self, mut events: Vec<AlertEvent>, demo: bool) -> usize {
        let mut logs = self.logs.lock().unwrap();
        let log = logs.of(demo);
        let seen: HashSet<(u64, String, String)> = log.events.iter().map(|e| (e.ts_ms, e.alert.id.clone(), format!("{:?}", e.kind))).collect();
        events.sort_by_key(|e| e.ts_ms);
        let mut added = 0;
        let mut pages = Vec::new();
        for mut e in events {
            if seen.contains(&(e.ts_ms, e.alert.id.clone(), format!("{:?}", e.kind))) {
                continue;
            }
            e.demo = demo;
            if matches!(e.kind, EventKind::Raised | EventKind::Escalated) && e.source.as_deref().is_some_and(|s| s.starts_with("agent:")) && e.alert.notify {
                let incident = e.incident_id.clone().or_else(|| e.alert.incident_id.clone()).unwrap_or_else(|| e.alert.id.clone());
                let title = e.incident_title.clone().unwrap_or_else(|| e.alert.summary.clone());
                pages.push(AgentPage { demo, incident, severity: e.alert.severity, title, ts_ms: e.ts_ms });
            }
            log.append(e);
            added += 1;
        }
        if added > 0 {
            // Keep the log in time order for the History tab and the next restart.
            let mut sorted: Vec<AlertEvent> = log.events.drain(..).collect();
            sorted.sort_by_key(|e| e.ts_ms);
            log.events = sorted.into();
            log.rewrite();
        }
        drop(logs);
        self.agent_pages.lock().unwrap().extend(pages);
        if added > 0 {
            self.touch();
        }
        added
    }

    /// The incidents node agents paged since the last call, for one data source's gate.
    pub fn take_agent_pages(&self, demo: bool) -> Vec<AgentPage> {
        let mut all = self.agent_pages.lock().unwrap();
        let (mine, rest): (Vec<AgentPage>, Vec<AgentPage>) = all.drain(..).partition(|p| p.demo == demo);
        *all = rest.into_iter().filter(|p| now_ms().saturating_sub(p.ts_ms) < 3_600_000).collect();
        mine
    }

    /// Forces the next evaluation to emit (after an ack, a silence or the end of planned work).
    fn touch(&self) {
        self.last.lock().unwrap().0.clear();
    }
}

/// What has to change for the UI to need a new state.
fn view_key(v: &HealthView) -> String {
    let mut key = String::new();
    for a in &v.alerts {
        key.push_str(&format!(
            "{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{}|{:?};",
            a.id, a.state, a.severity, a.message, a.acked_ms, a.silence_id, a.planned, a.recovers_at_ms, a.counted, a.flapping.as_ref().map(|f| f.count)
        ));
    }
    for i in &v.incidents {
        key.push_str(&format!("{}|{:?}|{};", i.id, i.state, i.title));
    }
    key.push_str(&format!("{:?}|{}", v.nodes.keys().collect::<Vec<_>>(), v.silences.len()));
    key
}

/// The rule engine's view of an endpoint, read from its JSON. A new metric has to be added to
/// the list below, or no rule ever sees it.
pub fn endpoint_info(v: &serde_json::Value) -> EndpointInfo {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
    let list = |k: &str| -> Vec<String> { v.get(k).and_then(|n| n.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
    let model = v.get("models").and_then(|m| m.get(0)).and_then(|m| m.as_str()).map(String::from).or_else(|| s("engine")).unwrap_or_default();
    let nodes = list("nodes");
    let head = s("headNode").unwrap_or_default();
    let span = if nodes.len() > 1 { format!("{}–{}", nodes[0], nodes[nodes.len() - 1]) } else { head.clone() };
    // What the UI calls it (models/endpoints.rs label_of): "Ollama on cell3", "glm-5.3-flash".
    let name = s("label").unwrap_or_default();
    let label = match name.as_str() {
        "" => format!("{model} on {span}"),
        n if n.contains(" on ") => n.to_string(),
        n => format!("{n} on {span}"),
    };
    let mut metrics = HashMap::new();
    if let Some(m) = v.get("metrics").filter(|m| m.is_object()) {
        for (from, to) in [
            ("genTps", "genTps"),
            ("promptTps", "promptTps"),
            ("running", "running"),
            ("waiting", "waiting"),
            ("kvPct", "kvPct"),
            ("ttftP50Ms", "ttftP50"),
            ("ttftP95Ms", "ttftP95"),
            ("itlP50Ms", "itlP50"),
            ("reqPerS", "reqPerS"),
            ("preemptions", "preemptions"),
            ("preemptionsTotal", "preemptions"),
            ("corruptedRequests", "corrupted"),
        ] {
            if let Some(x) = m.get(from).and_then(|x| x.as_f64()) {
                metrics.insert(to.to_string(), x);
            }
        }
    }
    EndpointInfo {
        maintenance_hold: v["maintenanceHold"].as_bool().unwrap_or(false),
        id: s("id").unwrap_or_default(),
        label,
        name,
        model,
        head_node: head,
        health: s("health").unwrap_or_default(),
        reason: s("healthReason"),
        message: s("message"),
        metrics,
        nodes,
        missing_nodes: list("missingNodes"),
        containers: list("containerNames"),
    }
}

pub fn endpoint_infos(list: &[crate::models::endpoints::Endpoint]) -> Vec<EndpointInfo> {
    list.iter().filter_map(|e| serde_json::to_value(e).ok()).map(|v| endpoint_info(&v)).collect()
}

/// Fills in each node's latched GPU fault (a fatal Xid until the node boots again, or GPU
/// readings that stopped), from the alerts in force. models::endpoints marks the node's
/// models Down on it.
fn stamp_gpu_faults(view: &HealthView, snap: &mut ClusterSnapshot) {
    for n in snap.nodes.iter_mut() {
        let fault = view.alerts.iter().filter(|a| a.state == AlertState::Firing && a.entity.node.as_deref() == Some(&n.id)).find_map(|a| match a.rule.as_str() {
            "kernel-xid" if a.severity == Severity::Crit && a.value.is_some_and(|c| rules::xid_latches(c as u32)) => Some(crate::model::GpuFault {
                code: a.value.map(|c| c as u32),
                text: a.message.clone(),
                since_ms: a.firing_since_ms.unwrap_or(a.since_ms),
                source: "xid".into(),
            }),
            "gpu-unreadable" => Some(crate::model::GpuFault { code: None, text: a.message.clone(), since_ms: a.firing_since_ms.unwrap_or(a.since_ms), source: "unreadable".into() }),
            _ => None,
        });
        n.gpu_fault = fault;
    }
}

/// Called from publish() for every snapshot, before it's sent to the UI: fills in the agents'
/// heartbeats (and Demo's kernel events), maintenance windows and GPU faults, records history,
/// runs the health rules, tells the UI when something changed, sends notifications and
/// updates the Dock badge.
pub fn on_publish(app: &AppHandle, snap: &mut ClusterSnapshot) {
    crate::agent::overlay(app, snap);
    // Recabling is information: mlx5 link events become kind "link" before anything reads them.
    kernel::stamp(snap);
    // A new bootId (before maintenance::stamp closes the window of a restart SparkBox made).
    boots::observe(app, snap);
    let endpoint_list = app.try_state::<crate::AppState>().map(|s| s.models.endpoints()).unwrap_or_default();
    let endpoints = endpoint_infos(&endpoint_list);
    if let Some(history) = app.try_state::<Arc<crate::history::History>>() {
        history.record(snap);
        history.record_entities(snap, &endpoints);
    }
    let Some(health) = app.try_state::<Arc<Health>>() else { return };
    let grace = health.rules().th("node-reboot", None).for_ms;
    for node in maintenance::stamp(snap, &endpoints, grace) {
        // The window closed by itself (the node and its models are back): its Planned alerts go quietly.
        health.end_maintenance(&node, snap.demo);
    }
    // This Mac's own path (nodes in planned work aren't evidence): the rules hold the nodes it
    // can't reach as not confirmed down.
    health.reach.stamp(snap);
    if let Some(mon) = app.try_state::<Arc<crate::fabric::FabricMonitor>>() {
        if let Some(state) = mon.state() {
            let rules = health.rules();
            let metrics: std::collections::BTreeSet<&str> = rules.rules.iter().filter(|r| r.kind == rule::RuleKind::Coded).map(|r| r.metric.as_str()).collect();
            health.set_external(metrics.into_iter().map(|m| (m.to_string(), crate::fabric::coded_readings(&state, m))).filter(|(_, v)| !v.is_empty()).collect());
        }
        health.set_rates(mon.rates());
    }
    let eval = health.evaluate_with(snap, &endpoints);
    let view = eval.view.clone().unwrap_or_else(|| health.view());
    stamp_gpu_faults(&view, snap);
    if let Some(view) = &eval.view {
        let _ = app.emit(STATE_EVENT, view);
        let crit = view.summary.crit;
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_badge_count(if crit > 0 { Some(crit as i64) } else { None });
        }
    }
    if !eval.events.is_empty() {
        crate::notify::on_alerts(app, &eval.events, &crate::agent::healthy_agents(snap), snap.demo);
    }
    if eval.acks_changed {
        crate::agent::acks_changed(app);
    }
}

pub mod commands {
    use std::sync::Arc;

    use tauri::{AppHandle, Emitter, State};

    use super::kernel::{decode, DecodedEvent};
    use super::rule::RuleSet;
    use super::silence::{self, Matcher, Silence};
    use super::{AlertEvent, Health, HealthView, PreviewRow, SILENCES_EVENT, STATE_EVENT};

    #[tauri::command]
    pub fn health_get(health: State<'_, Arc<Health>>) -> HealthView {
        health.view()
    }

    #[tauri::command]
    pub fn alerts_history(health: State<'_, Arc<Health>>, limit: usize) -> Vec<AlertEvent> {
        health.history(limit.clamp(1, super::LOG_KEEP))
    }

    /// Records a change to alerting in the activity log (Live only: Demo changes nothing real).
    fn audit(app: &AppHandle, demo: bool, title: &str, detail: &str) {
        if demo {
            return;
        }
        if let Some(state) = tauri::Manager::try_state::<crate::AppState>(app) {
            state.live.audit.record_app(title, detail);
        }
    }

    /// The Live silences changed: tell the UI and push them to the agents.
    fn silences_changed(app: &AppHandle, health: &Health, demo: bool) {
        if !demo {
            let _ = app.emit(SILENCES_EVENT, health.silences());
        }
        crate::agent::config_changed(app);
    }

    /// Acknowledging an alert (or a whole incident) makes it a known issue: no repeats, not
    /// counted, until it gets worse. Node scope, as in PagerDuty one click at 3 AM: it ends by
    /// itself when the alert gets worse, and it's logged.
    #[tauri::command]
    pub fn alert_ack(app: AppHandle, health: State<'_, Arc<Health>>, id: String) -> Result<HealthView, String> {
        crate::admin::require(&app)?;
        let (view, alerts) = health.ack(&id)?;
        let what = if alerts.len() == 1 { format!("'{}'", alerts[0].summary) } else { format!("{} alerts", alerts.len()) };
        audit(&app, view.demo, "Alert acknowledged", &format!("Acknowledge {what}"));
        let _ = app.emit(STATE_EVENT, &view);
        // The agent paging for it stops repeating too.
        crate::agent::acks_changed(&app);
        Ok(view)
    }

    /// Silences an alert or an incident for 1 hour, 1 day, 1 week, until it clears (at most 7
    /// days) or indefinitely (`duration`: 1h, 1d, 1w, resolved, never). `label` names it the way
    /// the Alerts window does. One alert (its rule and entity) or one incident for an hour or a
    /// day is node scope, like an acknowledgement. A rule or an entity everywhere, until it
    /// clears, and a week or longer need Mac scope with Live data and a reason.
    #[tauri::command]
    pub fn alert_silence(app: AppHandle, health: State<'_, Arc<Health>>, matcher: Matcher, duration: String, reason: String, label: String) -> Result<HealthView, String> {
        // (A blank entity or incident names nothing: needs_mac judges it as the wider silence it
        // is, and silence::make saves it that way.)
        if silence::needs_mac(&matcher, &duration) {
            crate::admin::require_mac_live(&app)?;
        } else {
            crate::admin::require(&app)?;
        }
        if silence::needs_mac(&matcher, &duration) && reason.trim().is_empty() {
            return Err(silence::reason_needed(&matcher, &duration));
        }
        let (view, s) = health.silence(matcher, &duration, &reason, &label)?;
        let why = if s.reason.is_empty() { String::new() } else { format!(": {}", s.reason) };
        audit(&app, view.demo, "Alert silenced", &format!("Silence '{}' {}{why}", s.label, silence::duration_words(&s.duration)));
        let _ = app.emit(STATE_EVENT, &view);
        silences_changed(&app, &health, view.demo);
        Ok(view)
    }

    /// Ends a silence early (and every other one on the same alerts). Node scope: it only makes
    /// SparkBox louder again.
    #[tauri::command]
    pub fn alert_unsilence(app: AppHandle, health: State<'_, Arc<Health>>, id: String) -> Result<HealthView, String> {
        crate::admin::require(&app)?;
        let (view, s): (HealthView, Silence) = health.unsilence(&id)?;
        audit(&app, view.demo, "Alert silence ended", &format!("Unsilence '{}'", s.label));
        let _ = app.emit(STATE_EVENT, &view);
        silences_changed(&app, &health, view.demo);
        Ok(view)
    }

    /// The rules in force (rules.json over the defaults).
    #[tauri::command]
    pub fn rules_get(health: State<'_, Arc<Health>>) -> RuleSet {
        (*health.rules()).clone()
    }

    /// The shipped rules, for "Reset to defaults" (nothing is saved until rules_set).
    #[tauri::command]
    pub fn rules_defaults() -> RuleSet {
        RuleSet::defaults()
    }

    /// Saves new rules (Mac scope: they change what pages the owner) and pushes them to
    /// the node agents.
    #[tauri::command]
    pub fn rules_set(app: AppHandle, health: State<'_, Arc<Health>>, rules: RuleSet) -> Result<RuleSet, String> {
        crate::admin::require_mac(&app)?;
        let before = health.rules();
        let saved = health.set_rules(rules)?;
        let changed: Vec<&str> = saved.rules.iter().filter(|r| before.get(&r.id) != Some(*r)).map(|r| r.title.as_str()).collect();
        if let Some(state) = tauri::Manager::try_state::<crate::AppState>(&app) {
            let what = if changed.is_empty() { "nothing changed".to_string() } else { changed.join(", ") };
            state.live.audit.record_app("Alert rules changed", &what);
        }
        crate::agent::config_changed(&app);
        Ok((*saved).clone())
    }

    /// What a candidate rule set would raise right now (Settings › Alerts, "Would fire now").
    #[tauri::command]
    pub fn rules_preview(health: State<'_, Arc<Health>>, rules: RuleSet) -> Vec<PreviewRow> {
        health.preview(&rules)
    }

    /// Kernel events (Xid, mlx5 faults, links, OOM kills, hung tasks) since `since_ms`, newest
    /// first, with what they mean and what to do. Link events of one recabling are one row.
    #[tauri::command]
    pub fn kernel_events(health: State<'_, Arc<Health>>, node: Option<String>, since_ms: u64) -> Vec<DecodedEvent> {
        let Some(mut snap) = health.last_snapshot() else { return Vec::new() };
        super::kernel::stamp(&mut snap);
        let names = snap.nodes.iter().map(|n| (n.id.clone(), n.name.clone())).collect();
        let out: Vec<DecodedEvent> = snap
            .nodes
            .iter()
            .filter(|n| node.as_ref().is_none_or(|id| &n.id == id))
            .flat_map(|n| n.kernel_events.iter().filter(|e| e.ts_ms >= since_ms).map(move |e| decode(&n.id, e)))
            .collect();
        super::kernel::with_boots(out, &super::boots::marks(&snap, node.as_deref(), since_ms), &names)
    }
}
