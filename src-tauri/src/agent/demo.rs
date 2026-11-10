//! Demo agents: installed on every simulated node, beating every few seconds, running
//! the current settings, probing the endpoints their node heads, and pinging a
//! simulated dead-man's switch. They page through a simulated "Phone (ntfy · Demo)"
//! channel, are watched by a simulated "Dead-man's switch (Demo)" (both are rows in the
//! channel list), and passed a simulated test page from every node 2 hours before the
//! session started, so the Demo baseline reads "24/7" truthfully within the simulation.
//! Those channels and that test exist only in the Demo watch state, never in notify.json.
//! Their deliveries are recorded as "Demo — not sent". Only a change to the channels makes
//! the test old, as in Live: a silence or a rule edit doesn't.
//!
//! Scenarios:
//! - `agent-offline` stops cell2's agent: its heartbeat freezes and the unit reads
//!   failed while SSH still answers ("Agent on cell2 not answering"), so 24/7 covers 3 of 4.
//! - `paging-drift`: pushing settings to cell3 fails ("sudo: a password is required"),
//!   so cell3 keeps old settings, the badge turns amber and this Mac pages for cell3.
//! - A node that goes offline is reported missing by its peers' agents.
//! - `as-found` (the cluster as SparkBox found it on Oct 29) has no agent on any node, as
//!   the real one: "This Mac only".
//!
//! The kernel events of `gpu-xid` and `oom-kill` come from the demo world itself
//! (demo/scenarios.rs). History for Demo is r3-charts' (history/); the agents here only
//! add their alert log from while SparkBox was closed (cell2's restart), tagged Demo.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tauri::{AppHandle, Manager};

use super::{AgentStatus, Deadman, EndToEnd, EndToEndCell, PushError, TestCell, TestRun, WatchChannel};
use crate::delivery::{Channel, Delivery, Outcome};
use crate::health::{AlertEvent, EventKind, Severity};
use crate::inventory::Inventory;
use crate::model::{AgentBeat, ClusterSnapshot, LinkStatus};
use crate::notify::Notifier;

/// How long the simulated agents have been running and recording.
const UPTIME_S: u64 = 6 * 86_400 + 4 * 3600;
const HISTORY_DAYS: u64 = 14;
/// The node whose agent the agent-offline scenario stops (its description names it).
const OFFLINE_NODE: &str = "cell2";
/// The node whose settings push the paging-drift scenario breaks.
const DRIFT_NODE: &str = "cell3";
pub const DRIFT_ERROR: &str = "sudo: a password is required";
/// The simulated end-to-end test, this long before the session started.
const TESTED_BEFORE_MS: u64 = 2 * 3_600_000;
/// The settings hash the simulated agents run (and the one cell3 is stuck on).
const HASH: &str = "5d0c9a7f3e21b8a4c6d2e0f1a3b5c7d9e1f2a4b6c8d0e2f4a6b8c0d2e4f6a8b0";
const OLD_HASH: &str = "9b1e4d7a2c5f8e0b3d6a9c2f5e8b1d4a7c0f3e6b9d2a5c8f1e4b7d0a3c6f9e2b";

pub struct DemoAgents {
    /// Nodes the agent was removed from this session.
    removed: BTreeSet<String>,
    /// Installed (again) during this session, and when.
    installed_at: BTreeMap<String, u64>,
    /// The scenario playing and when it started.
    scenario: Option<(String, u64)>,
    seeded: bool,
    nodes: Vec<String>,
    /// Endpoint ids by head node, from the last snapshot.
    endpoints: BTreeMap<String, Vec<String>>,
    session_start: u64,
    /// Settings changed during this Demo session.
    changed_ms: u64,
    /// The latest simulated test page: when, and each node's results.
    tested: Option<(u64, BTreeMap<String, TestRun>)>,
    push_errors: BTreeMap<String, PushError>,
    /// Offline nodes, which the running agents report missing, and since when (their agents'
    /// heartbeats stop there).
    missing: Vec<String>,
    offline_since: BTreeMap<String, u64>,
    /// The channels as last seen (what a test page proves), to tell a channel change from a
    /// silence or a rule edit.
    channels_seen: Option<String>,
}

impl DemoAgents {
    pub fn new(now: u64) -> DemoAgents {
        DemoAgents {
            removed: BTreeSet::new(),
            installed_at: BTreeMap::new(),
            scenario: None,
            seeded: false,
            nodes: Vec::new(),
            endpoints: BTreeMap::new(),
            session_start: now,
            changed_ms: 0,
            tested: None,
            push_errors: BTreeMap::new(),
            missing: Vec::new(),
            offline_since: BTreeMap::new(),
            channels_seen: None,
        }
    }

    pub fn install(&mut self, node: &str, now: u64) {
        self.removed.remove(node);
        self.installed_at.insert(node.to_string(), now);
        self.push_errors.remove(node);
    }

    pub fn remove(&mut self, node: &str) {
        self.removed.insert(node.to_string());
        self.installed_at.remove(node);
        self.push_errors.remove(node);
    }

    pub fn installed(&self) -> Vec<String> {
        if self.absent() {
            return Vec::new();
        }
        self.nodes.iter().filter(|n| !self.removed.contains(*n)).cloned().collect()
    }

    /// The as-found world: no node has an agent, as on the real cluster.
    fn absent(&self) -> bool {
        self.scenario_is("as-found").is_some()
    }

    fn scenario_is(&self, id: &str) -> Option<u64> {
        self.scenario.as_ref().filter(|(s, _)| s == id).map(|(_, at)| *at)
    }

    /// A settings push to `node`: Err with the reason when the scenario breaks it.
    pub fn push(&mut self, node: &str, now: u64) -> Result<(), String> {
        if self.scenario_is("paging-drift").is_some() && node == DRIFT_NODE {
            let attempts = self.push_errors.get(node).map(|e| e.attempts).unwrap_or(0) + 1;
            self.push_errors.insert(node.into(), PushError { at_ms: now, message: DRIFT_ERROR.into(), attempts });
            return Err(DRIFT_ERROR.into());
        }
        self.push_errors.remove(node);
        Ok(())
    }

    /// The settings changed: pushed to every simulated agent (cell3's fails in paging-drift).
    /// Each node's result, for the activity log.
    pub fn pushed(&mut self, now: u64) -> Vec<(String, Result<(), String>)> {
        self.installed().into_iter().map(|n| {
            let r = self.push(&n, now);
            (n, r)
        }).collect()
    }

    /// The channels the Mac has now: a change to them (not to silences, quiet hours or rules)
    /// makes the simulated test page old.
    pub fn note_channels(&mut self, s: &crate::notify::NotifySettings, now: u64) {
        let seen: Vec<_> = s.channels.iter().filter(|c| c.kind.pages()).map(|c| (c.id.clone(), c.enabled, c.mac_only, c.target(), c.saved.clone())).collect();
        let fp = format!("{seen:?}");
        if self.channels_seen.as_ref().is_some_and(|f| *f != fp) {
            self.changed_ms = now;
        }
        self.channels_seen = Some(fp);
    }

    /// The simulated heartbeat: a beat every 2 s, so it reads "1 s ago" or "2 s ago";
    /// frozen where the agent-offline scenario stopped it.
    fn beat(&self, node: &str, index: usize, now: u64) -> u64 {
        if let Some(since) = self.offline_since.get(node) {
            return *since;
        }
        match self.scenario_is("agent-offline") {
            Some(since) if node == OFFLINE_NODE => since,
            _ => now - (now / 1000 + index as u64) % 2 * 1000 - now % 1000,
        }
    }

    fn stopped(&self, node: &str) -> bool {
        self.scenario_is("agent-offline").is_some() && node == OFFLINE_NODE
    }

    fn current(&self, node: &str) -> bool {
        !self.push_errors.contains_key(node)
    }

    /// The lowest node whose agent is running: it pings the dead-man's switch.
    fn leader(&self) -> Option<String> {
        self.installed().into_iter().find(|n| !self.stopped(n))
    }

    pub fn statuses(&self, inv: &Inventory, now: u64) -> Vec<AgentStatus> {
        let leader = self.leader();
        inv.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if self.removed.contains(&n.id) || self.absent() {
                    return AgentStatus { node: n.id.clone(), name: n.name.clone(), state: "not-installed".into(), simulated: true, ..Default::default() };
                }
                let since = self.installed_at.get(&n.id).copied();
                let stopped = self.stopped(&n.id);
                let current = self.current(&n.id);
                let beat = self.beat(&n.id, i, now);
                let offline = self.offline_since.contains_key(&n.id);
                AgentStatus {
                    node: n.id.clone(),
                    name: n.name.clone(),
                    installed: true,
                    state: if stopped { "failed".into() } else if offline { "unknown".into() } else { "running".into() },
                    version: Some(super::version().into()),
                    last_beat_ms: Some(beat),
                    history_from_ms: Some(since.unwrap_or(now - HISTORY_DAYS * 86_400_000)),
                    firing: 0,
                    last_delivery_ms: self.tested.as_ref().map(|t| t.0).or(Some(self.tested_at(now))),
                    last_delivery_ok: Some(true),
                    uptime_s: Some(since.map(|s| now.saturating_sub(s) / 1000).unwrap_or(UPTIME_S + now / 1000 % 60)),
                    metrics_port: None,
                    error: if stopped {
                        Some("Agent on cell2 not answering: sparkbox-agent exited (status 1) and systemd keeps restarting it; SSH still answers".to_string())
                    } else {
                        offline.then(|| format!("{} is offline", n.name))
                    },
                    simulated: true,
                    config_hash: Some(if current { HASH } else { OLD_HASH }.into()),
                    expected_hash: Some(HASH.into()),
                    config_current: Some(current),
                    push_error: self.push_errors.get(&n.id).cloned(),
                    last_test: self.test_of(&n.id, inv, now),
                    last_cycle_ms: Some(beat),
                    peers_missing: if stopped { Vec::new() } else { self.missing.clone() },
                    isolated: None,
                    leader: Some(leader.as_deref() == Some(n.id.as_str())),
                    switch_watched: leader.as_deref() == Some(n.id.as_str()) && !offline,
                    endpoints_probed: if stopped { Vec::new() } else { self.endpoints.get(&n.id).cloned().unwrap_or_default() },
                    heartbeat_ok_ms: (leader.as_deref() == Some(n.id.as_str())).then(|| self.ping(now)),
                    outbox_pending: 0,
                    last_delivery_channel: Some("demo-phone".into()),
                    quiet_tz: None,
                    paging: None,
                }
            })
            .collect()
    }

    /// The simulated dead-man's switch answered the leader's last ping (every 60 s).
    fn ping(&self, now: u64) -> u64 {
        now - (now / 1000 % 60) * 1000
    }

    /// When the simulated test page last went out.
    pub fn tested_at(&self, _now: u64) -> u64 {
        self.tested.as_ref().map(|t| t.0).unwrap_or(self.session_start.saturating_sub(TESTED_BEFORE_MS))
    }

    fn test_of(&self, node: &str, _inv: &Inventory, now: u64) -> Option<TestRun> {
        if let Some((_, runs)) = &self.tested {
            return runs.get(node).cloned();
        }
        let index = self.nodes.iter().position(|n| n == node).unwrap_or(0) as u64;
        let ms = 640 + (index * 233) % 560;
        let results = [("demo-phone".to_string(), TestCell { ok: true, error: None, ms: Some(ms), name: Some("Phone (ntfy · Demo)".into()) })].into();
        Some(TestRun { at_ms: self.tested_at(now), results, paging: None })
    }

    /// A simulated test page from every node through `channels` (and the Demo phone).
    pub fn test_page(&mut self, channels: &[Channel], now: u64) -> BTreeMap<String, TestRun> {
        let mut runs = BTreeMap::new();
        for (i, n) in self.installed().into_iter().enumerate() {
            let results: BTreeMap<String, TestCell> = channels
                .iter()
                .map(|c| {
                    let ok = !self.stopped(&n);
                    let error = (!ok).then(|| "The agent didn't answer in 20 s. Is sparkbox-agent running?".to_string());
                    (c.id.clone(), TestCell { ok, error, ms: ok.then_some(620 + (i as u64 * 173 + c.id.len() as u64 * 41) % 600), name: Some(c.name.clone()) })
                })
                .collect();
            runs.insert(n, TestRun { at_ms: now, results, paging: None });
        }
        self.tested = Some((now, runs.clone()));
        runs
    }

    /// The simulated phone channel, for the Demo watch state.
    pub fn channel(&self, now: u64) -> WatchChannel {
        let c = crate::notify::demo_channel();
        WatchChannel { id: c.id, name: c.name, kind: c.kind.label().into(), remote: true, mac_only: false, enabled: true, last_ok_ms: Some(self.tested_at(now)), last_error: None, last_error_ms: None, simulated: true }
    }

    pub fn end_to_end(&self, inv: &Inventory, now: u64) -> EndToEnd {
        let installed = self.installed();
        let mut cells = Vec::new();
        for n in &installed {
            if let Some(t) = self.test_of(n, inv, now) {
                for (id, r) in &t.results {
                    cells.push(EndToEndCell { node: n.clone(), channel: id.clone(), channel_name: r.name.clone().unwrap_or_else(|| id.clone()), ok: r.ok, error: r.error.clone(), ms: r.ms, at_ms: t.at_ms });
                }
            }
        }
        let tested = self.tested_at(now);
        let ok = !installed.is_empty() && tested >= self.changed_ms && cells.iter().all(|c| c.ok) && installed.iter().all(|n| cells.iter().any(|c| &c.node == n));
        EndToEnd { tested_at_ms: (!installed.is_empty()).then_some(tested), ok, cells }
    }

    /// Settings changed in this Demo session (0: none since it started).
    pub fn changed_ms(&self) -> u64 {
        self.changed_ms
    }

    pub fn deadman(&self, now: u64) -> Deadman {
        let by = self.leader();
        Deadman { configured: true, service: Some("healthchecks.io".into()), last_ping_ms: by.as_ref().map(|_| self.ping(now)), by }
    }

    /// The simulated dead-man's switch, as a row in the Demo channel list.
    pub fn deadman_channel(&self, now: u64) -> WatchChannel {
        let kind = crate::delivery::Kind::Heartbeat;
        WatchChannel {
            id: "demo-deadman".into(),
            name: "Dead-man's switch (Demo)".into(),
            kind: kind.label().into(),
            remote: false,
            mac_only: false,
            enabled: true,
            last_ok_ms: self.leader().map(|_| self.ping(now)),
            last_error: None,
            last_error_ms: None,
            simulated: true,
        }
    }

    /// Heartbeats and settings state on every node with an agent; `endpoints` are
    /// (endpoint id, head node) of the Demo endpoints. True when the scenario changed what
    /// the agents report (so the watch state is sent at once, not with the next poll).
    pub fn overlay(&mut self, snap: &mut ClusterSnapshot, endpoints: &[(String, String)]) -> bool {
        let now = snap.ts_ms;
        self.nodes = snap.nodes.iter().map(|n| n.id.clone()).collect();
        let was = self.scenario.as_ref().map(|(s, _)| s.clone());
        match (&snap.scenario, &self.scenario) {
            (Some(s), Some((cur, _))) if s == cur => {}
            (Some(s), _) => self.scenario = Some((s.clone(), now)),
            (None, _) => self.scenario = None,
        }
        let now_drift = self.scenario_is("paging-drift").is_some();
        if now_drift && was.as_deref() != Some("paging-drift") {
            // The scenario's push, as if the owner had just changed a setting.
            let _ = self.push(DRIFT_NODE, now);
        } else if !now_drift && was.as_deref() == Some("paging-drift") {
            // The automatic retry goes through once sudo works again.
            self.push_errors.remove(DRIFT_NODE);
        }
        self.endpoints.clear();
        for (id, head) in endpoints {
            self.endpoints.entry(head.clone()).or_default().push(id.clone());
        }
        let missing_before = std::mem::take(&mut self.missing);
        self.missing = snap.nodes.iter().filter(|n| n.status == LinkStatus::Offline).map(|n| n.id.clone()).collect();
        let missing = self.missing.clone();
        self.offline_since.retain(|n, _| missing.contains(n));
        for n in &missing {
            self.offline_since.entry(n.clone()).or_insert(now);
        }
        let leader = self.leader();
        let changed = was != self.scenario.as_ref().map(|(s, _)| s.clone());
        for (i, n) in snap.nodes.iter_mut().enumerate() {
            if self.absent() {
                n.agent = None;
                continue;
            }
            if self.removed.contains(&n.id) || n.status == LinkStatus::Offline {
                continue;
            }
            let since = self.installed_at.get(&n.id).copied();
            let stopped = self.stopped(&n.id);
            let beat = self.beat(&n.id, i, now);
            let current = !self.push_errors.contains_key(&n.id);
            n.agent = Some(AgentBeat {
                version: super::version().into(),
                last_beat_ms: beat,
                history_from_ms: Some(since.unwrap_or(now - HISTORY_DAYS * 86_400_000)),
                firing: 0,
                config_hash: Some(if current { HASH } else { OLD_HASH }.into()),
                config_current: Some(current),
                push_error: self.push_errors.get(&n.id).map(|e| e.message.clone()),
                push_error_ms: self.push_errors.get(&n.id).map(|e| e.at_ms),
                last_cycle_ms: Some(beat),
                peers_missing: if stopped { Vec::new() } else { self.missing.clone() },
                isolated: None,
                endpoints_probed: if stopped { Vec::new() } else { self.endpoints.get(&n.id).cloned().unwrap_or_default() },
                last_delivery: None,
                heartbeat_ok_ms: (leader.as_deref() == Some(n.id.as_str())).then(|| self.ping(now)),
                outbox_pending: 0,
                switch_watched: leader.as_deref() == Some(n.id.as_str()),
            });
        }
        changed || missing_before != self.missing
    }

    /// The agents' alert log from while SparkBox was closed, once per session (and at most
    /// once a day, so relaunching doesn't repeat the story). Returns the events added.
    pub fn seed_away_once(&mut self, app: &AppHandle, snap: &ClusterSnapshot) -> usize {
        if self.seeded {
            return 0;
        }
        self.seeded = true;
        let Some(health) = app.try_state::<Arc<crate::health::Health>>() else { return 0 };
        let recent = health.history(crate::health::LOG_KEEP).iter().any(|e| e.away && snap.ts_ms.saturating_sub(e.ts_ms) < 86_400_000);
        if recent {
            return 0;
        }
        health.import(away_events(snap))
    }
}

/// cell2's restart in the Demo day (9 h 20 min ago, gone for 4 minutes), as the other
/// agents logged it while SparkBox was closed. Tagged Demo.
pub fn away_events(snap: &ClusterSnapshot) -> Vec<AlertEvent> {
    let Some(second) = snap.nodes.get(1) else { return Vec::new() };
    let Some(first) = snap.nodes.first() else { return Vec::new() };
    let gone = snap.ts_ms - crate::history::REBOOT_AGO_MS;
    let back = gone + crate::history::REBOOT_GAP_MS;
    let alert = |state: &str, ts: u64| -> Option<crate::health::Alert> {
        serde_json::from_value(serde_json::json!({
            "id": format!("peer-unreachable:node:{}", second.id), "rule": "peer-unreachable", "category": "node", "severity": "crit", "state": state,
            "entity": { "kind": "node", "id": second.id, "node": second.id, "switch": null, "label": second.name },
            "message": "Unreachable from the other agents for 60 s",
            "summary": format!("{} is unreachable from the other agents", second.name),
            "detail": null, "value": null, "sinceMs": gone, "firingSinceMs": gone + 60_000,
            "resolvedMs": if state == "resolved" { Some(ts) } else { None }, "ackedMs": null, "mutedUntilMs": null
        }))
        .ok()
    };
    let mk = |kind: EventKind, ts: u64, state: &str| {
        alert(state, ts).map(|a| {
            let mut e = AlertEvent::new(ts, kind, a);
            e.away = true;
            e.demo = true;
            e.source = Some(format!("agent:{}", first.id));
            e
        })
    };
    [mk(EventKind::Raised, gone + 60_000, "firing"), mk(EventKind::Resolved, back + 15_000, "resolved")].into_iter().flatten().collect()
}

/// A test "sent" by a simulated agent: recorded, never sent.
pub fn record_test(notifier: &Notifier, node: &str, ch: &Channel) -> Delivery {
    let d = Delivery {
        body: format!("This is how alerts from {node} reach you while SparkBox is closed."),
        via: format!("agent:{node}"),
        demo: true,
        test: true,
        latency_ms: Some(700),
        ..Delivery::row(Some(ch), &format!("SparkBox test from the agent on {node}"), Severity::Warn, Outcome::Demo)
    };
    notifier.deliveries.record(d.clone());
    d
}

/// The nodes a Demo snapshot shows with a fresh heartbeat (for tests).
#[cfg(test)]
pub fn beating(snap: &ClusterSnapshot) -> std::collections::HashSet<String> {
    super::healthy_agents(snap)
}
