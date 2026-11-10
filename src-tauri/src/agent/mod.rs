//! The always-on node agents: SparkBox keeps watching while the lid is closed.
//!
//! sparkbox_agent.py runs on every Spark as a systemd service (it imports the
//! collector, so it sees the node exactly as the live stream does), keeps its
//! own history, evaluates the same rules.json with the same notification gate,
//! watches its peers and pages the owner directly. This module installs, upgrades
//! and removes it over SSH (sudo, node scope, every command in the activity log),
//! pushes the settings to it and checks it runs them, reads its status, and merges
//! what it recorded while SparkBox was closed into History and the alert log.
//!
//! Settings an agent runs: agent.json, rules.json and notify.json (schema below).
//! `configHash` = sha256(agent.json ‖ 0x00 ‖ rules.json ‖ 0x00 ‖ notify.json) over the
//! exact bytes (fixtures/config-hash). SparkBox records the hash it pushed per node in
//! agents.json and compares it with the one the agent reports: only then is the node's
//! agent "current". A failed push is kept, shown, and retried at once when the node is back,
//! else 5, 15 and 60 minutes after the last try and then every 6 hours, and on Push again. A
//! retry, like any push SparkBox starts by itself, waits for Admin mode (`auto_push_due`).
//! Each push is one operation in the activity log with its nodes as children; a retry that
//! fails the same way again updates the row of the first failure ("failed · retried 7 times,
//! last 10:42") instead of adding one. A change to agent.json or rules.json only reloads the
//! agent (SIGHUP); a change to notify.json restarts it, because systemd hands it the
//! channels as a credential at start.
//!
//! agent.json (schema 2): `node`, `name`, `index`, `port`; `peers`: every node in
//! inventory order, itself included, `{id, name, index, lan, fabric: [ip], addr, port}`;
//! `switchMgmt`: the fabric switch's management address or null; `endpoints`: the
//! inference endpoints whose head is this node, as models::endpoints::AgentEndpoint
//! `{id, name, engine, port, model, headNode, needsKey, keyRef, host, nodes, chat}`;
//! `crash`: `{recentS, windowS}` (health::rules CRASH_RECENT_S and CRASH_WINDOW_S); `tz`: this
//! Mac's IANA zone, for times in messages;
//! `silences`: the Live silences `{id, rule|null, entity|null, incident|null,
//! untilMs|null, untilClears, reason}` (entity as health::silence::entity_key writes it,
//! "disk:gx10-0000:/"); `quiet`: `{start, end, tz}` or null (tz is IANA, never the
//! node's clock); `repeatMinutes`; `criticalOnly`; `resolved`; `categories`: `{node,
//! disk, gpu, fabric, containers}`; `metrics`: `{enabled, port}`.
//! notify.json: `channels` (those not marked "this Mac only", each with `secrets`),
//! `endpointKeys` (endpoint id → API key, for the endpoints headed on this node),
//! `metrics.token`, `requestKey` (signs acks and maintenance windows, below).
//!
//! Requests: acknowledgements (`acks_changed`) and maintenance windows (`maintenance_push`,
//! awaited, 5 s at most, before a restart or shutdown) are files in requests/, written as the
//! login user: `{v: 1, kind, body, mac}` with `mac` = HMAC-SHA256(requestKey, kind ‖ "\n" ‖
//! body). The agent takes them only when signed, under 24 h old and not ahead of its clock,
//! once each, and answers in `<name>.result.json`. A window lasts at most 4 hours. An ack or a
//! window's end that didn't reach an agent is kept and sent again when that node is back.
//!
//! Who sends (`agent_sends`): a node's agent pages for its node's alerts only while it's
//! healthy (heartbeat under 60 s, its cycle not stuck) and current, for the rules it
//! evaluates (fixtures/agent-rules.json). node-offline is left to the agents only when a
//! healthy, current agent reports that node missing; an endpoint's alerts only when its
//! head node's agent probes it. Everything else goes out from this Mac.
//!
//! Install layout: /opt/sparkbox-agent (the Python files, root 0644), /etc/sparkbox-agent
//! (agent.json and rules.json root 0644, notify.json root 0600), the unit in
//! /etc/systemd/system, /var/lib/sparkbox-agent (history and status, owned by the
//! sparkbox-agent system user; requests/ is writable by the login user's group, for
//! tests, acks and maintenance notes). The service runs as sparkbox-agent; systemd gives it
//! notify.json as a credential. On the node the channels' secrets are readable by the
//! sparkbox-agent service, root and anyone in the docker group, which includes the login
//! user SparkBox connects as (Docker is root-equivalent): use scoped credentials.
//!
//! Every install and push is one sudo command whose text embeds each file's size and
//! SHA-256; the files themselves follow the password line on its stdin. As root it
//! reads them into a fresh folder under /root, refuses on any mismatch, and only then
//! installs them. Nothing passes through a folder the login user can write.

pub mod commands;
pub mod demo;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::admin::AdminGate;
use crate::health::rule::{EntityKind, RuleSet};
use crate::inventory::{Inventory, NodeConfig, SwitchKind};
use crate::live::{sh_quote, Live};
use crate::model::{ClusterSnapshot, LinkStatus, NetRole};
use crate::notify::{NotifySettings, Scope};

pub const AGENT_PY: &str = include_str!("sparkbox_agent.py");
pub const UNIT: &str = include_str!("sparkbox-agent.service");
const AGENT_RULES: &str = include_str!("fixtures/agent-rules.json");
/// The system user the service runs as.
pub const SERVICE_USER: &str = "sparkbox-agent";
pub const STATE_EVENT: &str = "agent://state";
/// A heartbeat younger than this: the agent is watching.
pub const HEALTHY_MS: u64 = 60_000;
pub const DEFAULT_PORT: u16 = 9469;
/// How often the agents' history and alerts are merged in (also on connect).
pub const BACKFILL_EVERY_MS: u64 = 10 * 60_000;
/// A pending push is tried again this long after the last try: 5, 15 and 60 minutes, then
/// every 6 hours (and at once when its node comes back).
pub const PUSH_BACKOFF_MS: [u64; 4] = [5 * 60_000, 15 * 60_000, 60 * 60_000, 6 * 3_600_000];
/// Settings that changed on their own (endpoints, addresses) are pushed at most this often.
const PUSH_SETTLE_MS: u64 = 60_000;
/// A maintenance note an agent accepts is at most this big.
pub const MAINT_MAX: usize = 4096;
/// The agents end every maintenance window this long after it started, shutdowns included.
pub const MAINT_MAX_MS: u64 = 4 * 3_600_000;
/// `maintenance_push` waits at most this long for the agents to take the note.
pub const NOTE_TIMEOUT: Duration = Duration::from_millis(5_000);
/// An ack that didn't reach an agent is tried again this often (and when its node is back).
const ACK_RETRY_MS: u64 = 15 * 60_000;
/// Signs the requests SparkBox drops for the agents; made once, kept in the Keychain, sent
/// to the agents inside notify.json.
const REQUEST_KEY: &str = "agent.request.key";
/// "Send a test page from every node" at most once in this long.
pub const TEST_PAGE_EVERY_MS: u64 = 60_000;
const METRICS_TOKEN: &str = "agent.metrics.token";

/// sparkbox_agent.py's VERSION.
pub fn version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| AGENT_PY.lines().find_map(|l| l.strip_prefix("VERSION = \"").and_then(|r| r.split('"').next()).map(String::from)).unwrap_or_default())
}

/// The agent's Python files, installed side by side in /opt/sparkbox-agent: the agent and
/// the modules it imports (its gate and peer watch, r3-serving's endpoint prober, the
/// collector and its RoCE reader).
pub fn program_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("sparkbox_agent.py", AGENT_PY),
        ("gate.py", include_str!("gate.py")),
        ("peers.py", include_str!("peers.py")),
        ("serving_probe.py", include_str!("serving_probe.py")),
        ("collector.py", crate::live::COLLECTOR),
        ("roce.py", crate::live::ROCE),
    ]
}

/// "0.8 s", "12 s".
pub fn fmt_secs(ms: u64) -> String {
    if ms < 10_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} s", ms / 1000)
    }
}

struct AgentRules {
    coded: HashSet<String>,
    generic: HashSet<String>,
    endpoint: HashSet<String>,
}

fn agent_rules() -> &'static AgentRules {
    static RULES: OnceLock<AgentRules> = OnceLock::new();
    RULES.get_or_init(|| {
        let v: Value = serde_json::from_str(AGENT_RULES).unwrap_or_default();
        let list = |k: &str| v.get(k).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        AgentRules { coded: list("coded"), generic: list("genericEntities"), endpoint: list("endpointRules") }
    })
}

// ---- Who sends

/// One node's agent, as paging needs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentView {
    /// Heartbeat under 60 s and its cycle not stuck.
    pub healthy: bool,
    /// It reports the settings hash SparkBox last pushed.
    pub current: bool,
    /// The bundled agent evaluates available memory, PSI and swap-in readings.
    pub serving_memory: bool,
    pub peers_missing: Vec<String>,
    pub endpoints_probed: Vec<String>,
    /// It leads and checks the fabric switch.
    pub switch: bool,
}

impl AgentView {
    fn ready(&self) -> bool {
        self.healthy && self.current
    }
}

/// What `agent_sends` decides from: each node's agent, and the entity kind of the rules
/// that aren't coded (so a generic threshold rule on a node is the agent's too).
#[derive(Clone, Debug, Default)]
pub struct PagingCtx {
    pub agents: HashMap<String, AgentView>,
    pub generic_rules: HashMap<String, String>,
    /// The switch whose management address the agents check (the RouterOS one).
    pub fabric_switch: Option<String>,
}

fn entity_word(k: EntityKind) -> String {
    serde_json::to_value(k).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

impl PagingCtx {
    pub fn new(snap: &ClusterSnapshot, rules: &RuleSet) -> PagingCtx {
        let generic_rules = rules
            .rules
            .iter()
            .filter(|r| !crate::health::rules::CODED.contains(&r.id.as_str()))
            .map(|r| (r.id.clone(), entity_word(r.entity)))
            .collect();
        let agents = snap
            .nodes
            .iter()
            .filter_map(|n| {
                let a = n.agent.as_ref()?;
                let fresh = |t: u64| snap.ts_ms.saturating_sub(t) < HEALTHY_MS;
                let view = AgentView {
                    healthy: fresh(a.last_beat_ms) && a.last_cycle_ms.is_none_or(fresh),
                    current: a.config_current == Some(true),
                    serving_memory: a.version == version(),
                    peers_missing: a.peers_missing.clone(),
                    endpoints_probed: a.endpoints_probed.clone(),
                    switch: a.switch_watched,
                };
                Some((n.id.clone(), view))
            })
            .collect();
        let fabric_switch = snap.switches.iter().find(|s| s.kind == SwitchKind::RouterOs).map(|s| s.id.clone());
        PagingCtx { agents, generic_rules, fabric_switch }
    }
}

/// The node whose agent pages for this alert, or None when this Mac must.
///
/// - Switch ports and optics: seen from the Mac only. The fabric switch not answering: the
///   leader agent while it checks the switch, healthy and current; other switches: the Mac.
/// - node-offline, and node-reboot (a restart that ran late): a healthy, current agent that
///   lists the node in `peersMissing` (the leader raises it from the maintenance note).
/// - Endpoint rules: the head node's agent, healthy, current and probing that endpoint.
/// - Anything else: the node's own agent, healthy and current, for a rule it evaluates.
pub fn agent_sends(s: &Scope, ctx: &PagingCtx) -> Option<String> {
    if let Some(sw) = s.switch.as_deref() {
        if s.rule != "switch-unreachable" || ctx.fabric_switch.as_deref() != Some(sw) {
            return None;
        }
        let mut by: Vec<&String> = ctx.agents.iter().filter(|(_, a)| a.ready() && a.switch).map(|(n, _)| n).collect();
        by.sort();
        return by.first().map(|n| n.to_string());
    }
    let node = s.node.as_deref()?;
    let rules = agent_rules();
    if s.rule == "node-offline" || s.rule == "node-reboot" {
        let mut by: Vec<&String> = ctx.agents.iter().filter(|(n, a)| n.as_str() != node && a.ready() && a.peers_missing.iter().any(|m| m == node)).map(|(n, _)| n).collect();
        by.sort();
        return by.first().map(|n| n.to_string());
    }
    let me = ctx.agents.get(node).filter(|a| a.ready())?;
    // A settings push does not upgrade Python. Older agents cannot evaluate these metrics;
    // retain Mac paging until the known, bundled implementation reports its version.
    if matches!(s.rule.as_str(), "mem-available" | "mem-pressure" | "mem-swap-in") && !me.serving_memory {
        return None;
    }
    if s.kind == "endpoint" || rules.endpoint.contains(&s.rule) {
        return me.endpoints_probed.iter().any(|e| *e == s.id).then(|| node.to_string());
    }
    let generic = ctx.generic_rules.get(&s.rule).is_some_and(|k| rules.generic.contains(k));
    (rules.coded.contains(&s.rule) || generic).then(|| node.to_string())
}

/// The context for what's being sent now: the snapshot the rules last saw.
pub fn paging_ctx(app: &AppHandle) -> PagingCtx {
    let Some(health) = app.try_state::<Arc<crate::health::Health>>() else { return PagingCtx::default() };
    let snap = health.last_snapshot().unwrap_or_default();
    PagingCtx::new(&snap, &health.rules())
}

/// The nodes in a snapshot whose agent has a fresh heartbeat.
pub fn healthy_agents(snap: &ClusterSnapshot) -> HashSet<String> {
    snap.nodes.iter().filter(|n| n.agent.as_ref().is_some_and(|a| snap.ts_ms.saturating_sub(a.last_beat_ms) < HEALTHY_MS)).map(|n| n.id.clone()).collect()
}

// ---- What the UI sees

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub node: String,
    pub name: String,
    pub installed: bool,
    /// running | stopped | failed | not-installed | unknown
    pub state: String,
    pub version: Option<String>,
    pub last_beat_ms: Option<u64>,
    pub history_from_ms: Option<u64>,
    pub firing: u32,
    pub last_delivery_ms: Option<u64>,
    pub last_delivery_ok: Option<bool>,
    pub uptime_s: Option<u64>,
    pub metrics_port: Option<u16>,
    pub error: Option<String>,
    pub simulated: bool,
    /// The settings hash the agent reports, and the one SparkBox pushed.
    #[serde(default)]
    pub config_hash: Option<String>,
    #[serde(default)]
    pub expected_hash: Option<String>,
    #[serde(default)]
    pub config_current: Option<bool>,
    #[serde(default)]
    pub push_error: Option<PushError>,
    #[serde(default)]
    pub last_test: Option<TestRun>,
    #[serde(default)]
    pub last_cycle_ms: Option<u64>,
    #[serde(default)]
    pub peers_missing: Vec<String>,
    #[serde(default)]
    pub isolated: Option<String>,
    #[serde(default)]
    pub leader: Option<bool>,
    /// It leads and checks the fabric switch's management address.
    #[serde(default)]
    pub switch_watched: bool,
    #[serde(default)]
    pub endpoints_probed: Vec<String>,
    #[serde(default)]
    pub heartbeat_ok_ms: Option<u64>,
    #[serde(default)]
    pub outbox_pending: u32,
    #[serde(default)]
    pub last_delivery_channel: Option<String>,
    /// The zone its quiet hours follow, or "missing" when it has none.
    #[serde(default)]
    pub quiet_tz: Option<String>,
    /// The paging hash of the channels it runs (from this Mac's last push that went through).
    #[serde(default, skip_serializing)]
    pub paging: Option<String>,
}

impl AgentStatus {
    pub fn healthy(&self, now: u64) -> bool {
        self.installed && self.state == "running" && self.last_beat_ms.is_some_and(|b| now.saturating_sub(b) < HEALTHY_MS)
    }
}

/// A settings push that failed and is still to be done.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushError {
    pub at_ms: u64,
    pub message: String,
    pub attempts: u32,
}

/// One node's last "Send a test page from every node": a result per channel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRun {
    pub at_ms: u64,
    pub results: BTreeMap<String, TestCell>,
    /// What it proved: the paging hash of the channels the node's agent ran then.
    #[serde(default)]
    pub paging: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCell {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// How long the channel took to accept it.
    #[serde(default)]
    pub ms: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
}

/// A channel, as the watch state needs it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchChannel {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// A phone or team channel (not this Mac, not the dead-man's switch).
    pub remote: bool,
    pub mac_only: bool,
    pub enabled: bool,
    pub last_ok_ms: Option<u64>,
    pub last_error: Option<String>,
    pub last_error_ms: Option<u64>,
    /// Demo: simulated, never saved.
    pub simulated: bool,
}

/// The dead-man's switch, if one is set up.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deadman {
    pub configured: bool,
    /// "healthchecks.io".
    pub service: Option<String>,
    pub last_ping_ms: Option<u64>,
    /// The node whose agent pinged it last.
    pub by: Option<String>,
}

/// What the agents cover while this Mac sleeps.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Watched {
    pub nodes: bool,
    pub gpus: bool,
    pub fabric: bool,
    pub containers: bool,
    /// Endpoint names a healthy, current agent probes with a one-token check.
    pub endpoints: Vec<String>,
    pub switch: bool,
    /// Switches and routers nothing watches while this Mac sleeps.
    pub unwatched: Vec<String>,
}

/// A node whose agent doesn't run the current settings, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drift {
    pub node: String,
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushFailure {
    pub node: String,
    pub name: String,
    pub at_ms: u64,
    pub message: String,
    pub attempts: u32,
}

/// The latest end-to-end test: a test page sent by every node's agent on every channel.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndToEnd {
    /// The oldest of the nodes' last tests (all of them have tested since then).
    pub tested_at_ms: Option<u64>,
    /// Every node's test reached every phone channel, after the last settings change.
    pub ok: bool,
    pub cells: Vec<EndToEndCell>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndToEndCell {
    pub node: String,
    pub channel: String,
    pub channel_name: String,
    pub ok: bool,
    pub error: Option<String>,
    pub ms: Option<u64>,
    pub at_ms: u64,
}

/// A delivery that failed recently, or an outbox that isn't draining.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PagingFailure {
    pub at_ms: u64,
    pub channel: String,
    pub message: String,
    /// "mac" or the node whose agent it was.
    pub by: String,
}

/// Which secrets an install or push copies to a node.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretCopy {
    pub node: String,
    pub name: String,
    /// "Phone (ntfy): token".
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchState {
    /// agents | partial | mac-only
    pub mode: String,
    pub agents: Vec<AgentStatus>,
    /// agents | mac | none: who pages the phone channels.
    pub delivery_owner: String,
    pub channels: Vec<WatchChannel>,
    pub end_to_end: EndToEnd,
    /// When the channels or notification settings last changed.
    pub changed_ms: u64,
    pub drift: Vec<Drift>,
    pub push_failures: Vec<PushFailure>,
    pub deadman: Deadman,
    pub watched: Watched,
    /// Silences in effect.
    pub silences: u32,
    pub paging_failure: Option<PagingFailure>,
    pub secret_copies: Vec<SecretCopy>,
    pub demo: bool,
    /// The settings push this call started or joined (agent_push_config), for its one toast.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_task: Option<String>,
}

/// The parts of the watch state that come from the agents alone (the rest is filled in by `current`).
pub fn watch_state(agents: Vec<AgentStatus>, remote_channels: bool, now: u64) -> WatchState {
    let healthy = agents.iter().filter(|a| a.healthy(now)).count();
    let mode = if healthy == 0 { "mac-only" } else if healthy == agents.len() { "agents" } else { "partial" };
    let current = agents.iter().filter(|a| a.installed).all(|a| a.config_current == Some(true));
    let owner = if !remote_channels { "none" } else if mode == "agents" && current { "agents" } else { "mac" };
    let drift = agents
        .iter()
        .filter(|a| a.installed && a.config_current != Some(true))
        .map(|a| Drift { node: a.node.clone(), name: a.name.clone(), reason: drift_reason(a) })
        .collect();
    let push_failures = agents
        .iter()
        .filter_map(|a| a.push_error.as_ref().map(|e| PushFailure { node: a.node.clone(), name: a.name.clone(), at_ms: e.at_ms, message: e.message.clone(), attempts: e.attempts }))
        .collect();
    WatchState { mode: mode.into(), delivery_owner: owner.into(), drift, push_failures, agents, ..Default::default() }
}

/// "push failed: sudo: a password is required", "agent 1.0.0 can't report its settings".
fn drift_reason(a: &AgentStatus) -> String {
    if let Some(e) = &a.push_error {
        return format!("push failed: {}", e.message);
    }
    if a.config_hash.is_none() {
        return format!("agent {} doesn't report its settings: reinstall it", a.version.as_deref().unwrap_or("?"));
    }
    if a.expected_hash.is_none() {
        return "set up before this version of SparkBox: push the settings".into();
    }
    "has older settings than this Mac".into()
}

/// "Expose metrics for Prometheus": off by default; the token is in the Keychain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentSettings {
    pub metrics_enabled: bool,
    pub metrics_port: u16,
    pub port: u16,
    /// A /metrics bearer token is in the Keychain (so reading settings never reads it).
    pub has_token: bool,
}

impl Default for AgentSettings {
    fn default() -> Self {
        AgentSettings { metrics_enabled: false, metrics_port: DEFAULT_PORT + 1, port: DEFAULT_PORT, has_token: false }
    }
}

/// The SHA-256 of each settings file, to tell a reload from a restart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parts {
    pub agent: String,
    pub rules: String,
    pub notify: String,
}

/// What this Mac knows about one node's agent (Live), kept in agents.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeRecord {
    pub installed: bool,
    /// The settings hash this Mac last pushed (or tried to).
    pub expected_hash: Option<String>,
    pub reported_hash: Option<String>,
    pub pushed_at_ms: Option<u64>,
    /// The files of the last push that went through.
    pub pushed_parts: Option<Parts>,
    /// The hash of the settings without their secrets at the last push: polling compares
    /// it to notice endpoints or addresses that changed, without reading the Keychain.
    pub fingerprint: Option<String>,
    pub push_error: Option<PushError>,
    pub last_test: Option<TestRun>,
    /// Its last known fabric addresses, so agent.json doesn't change while it's away.
    pub fabric: Vec<String>,
    /// The paging hash of the last push that went through (`paging_hash`): what a test page proves.
    pub paging: Option<String>,
    /// The activity-log row of an automatic push that keeps failing the same way.
    pub retry_row: Option<RetryRow>,
    /// Ends of maintenance windows that didn't reach this node's agent: the nodes they were about.
    pub pending_clears: Vec<PendingClear>,
}

/// The row a failing automatic push is on: retries that fail the same way update it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetryRow {
    pub op_id: String,
    pub head_id: String,
    pub message: String,
}

/// A window's end still to tell an agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PendingClear {
    pub node: String,
    pub since_ms: u64,
}

/// What `maintenance_push` got done: the nodes whose agent took the note, and those whose
/// didn't, with why ("not connected", "didn't answer in 5 s", "its signature doesn't match").
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushReport {
    pub reached: Vec<String>,
    pub missing: Vec<(String, String)>,
}

/// agents.json: the metrics settings and what's known about each node's agent.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AgentsFile {
    #[serde(flatten)]
    settings: AgentSettings,
    nodes: BTreeMap<String, NodeRecord>,
}

// ---- Files for a node

/// Where the agent's files go. Tests use a temporary root on a fake node.
#[derive(Clone, Debug)]
pub struct Layout {
    pub opt: String,
    pub etc: String,
    pub unit: String,
    pub state: String,
    /// Root's own folder, where the install reads what it was sent.
    pub scratch: String,
}

impl Layout {
    pub fn system() -> Layout {
        Layout {
            opt: "/opt/sparkbox-agent".into(),
            etc: "/etc/sparkbox-agent".into(),
            unit: "/etc/systemd/system/sparkbox-agent.service".into(),
            state: "/var/lib/sparkbox-agent".into(),
            scratch: "/root".into(),
        }
    }

    pub fn requests(&self) -> String {
        format!("{}/requests", self.state)
    }

    /// The agent's own command, as the cluster user.
    pub fn cli(&self, args: &str) -> String {
        format!("SPARKBOX_AGENT_STATE={} SPARKBOX_AGENT_CONFIG={} python3 -I {}/sparkbox_agent.py {args}", sh_quote(&self.state), sh_quote(&self.etc), sh_quote(&self.opt))
    }
}

/// One file to put on a node.
#[derive(Clone, Debug)]
pub struct File {
    pub name: &'static str,
    pub content: String,
    /// Where it goes, with its mode.
    pub dest: Dest,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dest {
    /// /opt/sparkbox-agent, root 0644.
    Program,
    /// /etc/sparkbox-agent, root 0644.
    Config,
    /// /etc/sparkbox-agent, root 0600 (the channels' secrets).
    Secret,
    /// The systemd unit.
    Unit,
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256 (RFC 2104), hex: what signs a request (sparkbox_agent.py sign).
pub fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.iter().map(|x| x ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A request file for the agents: `{v, kind, body, mac}`, signed with the request key.
pub fn signed_request(key: &str, kind: &str, body: &str) -> String {
    let mac = hmac_sha256_hex(key.as_bytes(), format!("{kind}\n{body}").as_bytes());
    serde_json::to_string(&json!({ "v": 1, "kind": kind, "body": body, "mac": mac })).unwrap_or_default()
}

/// What a test page from a node proves: sha256 of the channels in its notify.json (secrets
/// included). Silences, acks, quiet hours, repeats and rules live in agent.json and
/// rules.json, so changing them leaves a node's test standing; a channel, a channel's secret
/// or a "this Mac only" switch changes it.
pub fn paging_hash(notify: &str) -> String {
    let v: Value = serde_json::from_str(notify).unwrap_or_default();
    sha256_hex(serde_json::to_string(v.get("channels").unwrap_or(&Value::Null)).unwrap_or_default().as_bytes())
}

/// sha256(agent.json ‖ 0x00 ‖ rules.json ‖ 0x00 ‖ notify.json), hex.
pub fn config_hash(agent: &[u8], rules: &[u8], notify: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(agent);
    h.update([0u8]);
    h.update(rules);
    h.update([0u8]);
    h.update(notify);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The three settings files for one node.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    pub agent: String,
    pub rules: String,
    pub notify: String,
}

impl Bundle {
    pub fn hash(&self) -> String {
        config_hash(self.agent.as_bytes(), self.rules.as_bytes(), self.notify.as_bytes())
    }

    pub fn parts(&self) -> Parts {
        Parts { agent: sha256_hex(self.agent.as_bytes()), rules: sha256_hex(self.rules.as_bytes()), notify: sha256_hex(self.notify.as_bytes()) }
    }

    /// The files of an install (everything) or of a push (`apply`).
    pub fn files(&self, apply: Apply, program: bool) -> Vec<File> {
        let mut files = vec![File { name: "agent.json", content: self.agent.clone(), dest: Dest::Config }, File { name: "rules.json", content: self.rules.clone(), dest: Dest::Config }];
        if program || apply == Apply::Restart {
            files.push(File { name: "notify.json", content: self.notify.clone(), dest: Dest::Secret });
        }
        if program {
            for (name, content) in program_files() {
                files.push(File { name, content: content.into(), dest: Dest::Program });
            }
            files.push(File { name: "sparkbox-agent.service", content: UNIT.into(), dest: Dest::Unit });
        }
        files
    }
}

/// A node's addresses, for its peers' agent.json.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Addrs {
    pub lan: Option<String>,
    pub fabric: Vec<String>,
}

/// What agent.json carries besides the nodes.
#[derive(Clone, Debug, Default)]
pub struct Extras {
    /// The fabric switch the leader checks: `{id, name, host}`.
    pub switch_mgmt: Option<Value>,
    pub endpoints: Vec<Value>,
    pub silences: Vec<Value>,
}

/// agent.json for one node (schema in the module docs).
pub fn agent_json(inv: &Inventory, node: &str, addrs: &BTreeMap<String, Addrs>, extras: &Extras, n: &NotifySettings, s: &AgentSettings) -> String {
    let index = inv.nodes.iter().position(|x| x.id == node).unwrap_or(0);
    let me = inv.nodes.iter().find(|x| x.id == node);
    let peers: Vec<Value> = inv
        .nodes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let a = addrs.get(&p.id).cloned().unwrap_or_default();
            let lan = a.lan.clone().or_else(|| p.ip.clone()).unwrap_or_else(|| p.host.clone());
            let mut fabric = a.fabric.clone();
            fabric.sort();
            // r2 agents listen on `addr`: the lowest fabric address, else the LAN one.
            let addr = fabric.first().cloned().unwrap_or_else(|| lan.clone());
            json!({ "id": p.id, "name": p.name, "index": i, "lan": lan, "fabric": fabric, "addr": addr, "port": s.port })
        })
        .collect();
    let v = json!({
        "schema": 2,
        "node": node,
        "name": me.map(|x| x.name.clone()).unwrap_or_else(|| node.to_string()),
        "index": index,
        "port": s.port,
        "peers": peers,
        "switchMgmt": extras.switch_mgmt,
        "endpoints": extras.endpoints,
        "silences": extras.silences,
        "quiet": n.quiet(),
        "repeatMinutes": n.repeat_minutes,
        "criticalOnly": n.critical_only,
        "resolved": n.resolved,
        "categories": { "node": n.node, "disk": n.disk, "gpu": n.gpu, "fabric": n.fabric, "containers": n.containers },
        "metrics": { "enabled": s.metrics_enabled, "port": s.metrics_port },
        "crash": { "recentS": crate::health::rules::CRASH_RECENT_S, "windowS": crate::health::rules::CRASH_WINDOW_S },
        "tz": (!n.quiet_tz.is_empty()).then(|| n.quiet_tz.clone()).unwrap_or_else(crate::notify::mac_zone),
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// notify.json: the enabled channels that aren't "this Mac only", with their secrets, the API
/// keys of the endpoints headed on this node, and the request key. Root 0600 on the node,
/// handed to the service by systemd; it never touches the Mac's disk.
pub fn notify_json(s: &NotifySettings, secret: impl Fn(&str, &str) -> Option<String>, endpoint_keys: &BTreeMap<String, String>, metrics_token: Option<String>, request_key: Option<String>) -> String {
    let channels: Vec<Value> = s
        .channels
        .iter()
        .filter(|c| c.enabled && !c.mac_only && c.kind != crate::delivery::Kind::Mac)
        .map(|c| {
            let mut v = serde_json::to_value(c).unwrap_or_default();
            let secrets: serde_json::Map<String, Value> = c.saved.iter().filter_map(|f| secret(&c.id, f).map(|x| (f.clone(), json!(x)))).collect();
            v["secrets"] = json!(secrets);
            v
        })
        .collect();
    let mut v = json!({ "channels": channels, "endpointKeys": endpoint_keys, "metrics": { "token": metrics_token } });
    if let Some(k) = request_key {
        v["requestKey"] = json!(k);
    }
    serde_json::to_string(&v).unwrap_or_default()
}

/// Whether notify.json would copy one of the owner's secrets to a node: a channel secret,
/// an endpoint API key or the metrics token (the request key is SparkBox's own, made for the
/// agents). Read without the Keychain.
pub fn copies_secrets(s: &NotifySettings, endpoint_keys: bool, metrics_token: bool) -> bool {
    endpoint_keys || metrics_token || s.channels.iter().any(|c| c.enabled && !c.mac_only && c.kind != crate::delivery::Kind::Mac && !c.saved.is_empty())
}

pub fn rules_json(rules: &RuleSet) -> String {
    serde_json::to_string_pretty(rules).unwrap_or_default()
}

/// What an install or a push copies to `node`, for the install dialog: channel secrets and
/// endpoint keys, by name only.
pub fn secret_items(s: &NotifySettings, endpoint_keys: &[String]) -> Vec<String> {
    let mut out: Vec<String> = s
        .channels
        .iter()
        .filter(|c| c.enabled && !c.mac_only)
        .flat_map(|c| c.saved.iter().map(move |f| format!("{}: {}", c.name, field_word(f))))
        .collect();
    out.extend(endpoint_keys.iter().map(|e| format!("{e}: API key")));
    out
}

fn field_word(f: &str) -> &str {
    match f {
        "appToken" => "app token",
        "userKey" => "user key",
        "secret" => "signing secret",
        f => f,
    }
}

// ---- The root step

/// How a push makes the agent take new settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    /// agent.json and rules.json changed: `systemctl reload` (SIGHUP).
    Reload,
    /// notify.json changed (or what's on the node is unknown): restart, so systemd hands it the new credential.
    Restart,
}

/// Reload only when this Mac's last successful push is what the node runs and notify.json
/// hasn't changed since; anything else restarts with every file.
pub fn apply_for(rec: &NodeRecord, bundle: &Bundle) -> Apply {
    let parts = bundle.parts();
    let on_node = rec.reported_hash.is_some() && rec.reported_hash == rec.expected_hash && rec.push_error.is_none();
    match &rec.pushed_parts {
        Some(p) if on_node && p.notify == parts.notify => Apply::Reload,
        _ => Apply::Restart,
    }
}

/// The stdin of an install or push: every file, back to back.
pub fn stream(files: &[File]) -> Vec<u8> {
    files.iter().flat_map(|f| f.content.as_bytes().to_vec()).collect()
}

fn dest_path(layout: &Layout, f: &File) -> (String, &'static str) {
    match f.dest {
        Dest::Program => (format!("{}/{}", layout.opt, f.name), "0644"),
        Dest::Config => (format!("{}/{}", layout.etc, f.name), "0644"),
        Dest::Secret => (format!("{}/{}", layout.etc, f.name), "0600"),
        Dest::Unit => (layout.unit.clone(), "0644"),
    }
}

/// The root script of an install (`program`) or a push, given the files that follow the
/// password line on stdin. Its text embeds each file's size and SHA-256: it reads the
/// stream into a fresh 0700 folder under root's home, checks every file, refuses on any
/// mismatch, and only then installs them, root-owned, with fixed modes. An install also
/// creates the sparkbox-agent system user and its state folder (requests/ writable by
/// `login`'s group), then (re)starts the service; a push reloads or restarts it.
pub fn root_script(layout: &Layout, files: &[File], login: &str, program: bool, apply: Apply) -> String {
    let q = sh_quote;
    let total: usize = files.iter().map(|f| f.content.len()).sum();
    let mut s = String::from("set -eu\numask 077\n");
    s.push_str(&format!("w=$(mktemp -d {})\ntrap 'rm -rf \"$w\"' EXIT\n", q(&format!("{}/sparkbox-agent.XXXXXXXX", layout.scratch))));
    s.push_str("cat > \"$w/stream\"\n");
    s.push_str(&format!(
        "[ \"$(wc -c < \"$w/stream\" | tr -d ' ')\" = {total} ] || {{ echo 'What SparkBox sent arrived incomplete; nothing was installed.' >&2; exit 3; }}\n"
    ));
    let mut offset = 0usize;
    let mut sums = String::new();
    for (i, f) in files.iter().enumerate() {
        s.push_str(&format!("tail -c +{} \"$w/stream\" | head -c {} > \"$w/f{i}\"\n", offset + 1, f.content.len()));
        offset += f.content.len();
        sums.push_str(&format!("{}  f{i}\n", sha256_hex(f.content.as_bytes())));
    }
    s.push_str(&format!("printf '%s' {} > \"$w/sums\"\n", q(&sums)));
    s.push_str("( cd \"$w\" && sha256sum -c --quiet sums >&2 ) || { echo 'A file SparkBox sent arrived damaged; nothing was installed.' >&2; exit 3; }\n");
    if program {
        s.push_str(&format!(
            "id -u {u} >/dev/null 2>&1 || useradd --system --user-group --no-create-home --home-dir {home} --shell /usr/sbin/nologin {u}\n",
            u = SERVICE_USER,
            home = q(&layout.state)
        ));
    }
    let mut dirs = vec![layout.etc.clone()];
    if program {
        dirs.insert(0, layout.opt.clone());
        if let Some(d) = std::path::Path::new(&layout.unit).parent() {
            s.push_str(&format!("mkdir -p {}\n", q(&d.display().to_string())));
        }
    }
    s.push_str(&format!("install -d -o root -g root -m 0755 {}\n", dirs.iter().map(|d| q(d)).collect::<Vec<_>>().join(" ")));
    for (i, f) in files.iter().enumerate() {
        let (to, mode) = dest_path(layout, f);
        s.push_str(&format!("install -o root -g root -m {mode} \"$w/f{i}\" {}\n", q(&to)));
    }
    if program {
        // The state folder: taken from whoever had it (an older agent ran as the login user),
        // so nothing in it is followed or changed by anyone else while root works on it.
        let d = q(&layout.state);
        s.push_str(&format!("if [ -L {d} ]; then rm -f {d}; fi\nmkdir -p {d}\nchown -h root:root {d}\nchmod 0755 {d}\n"));
        s.push_str(&format!("chown -R -h {u}:{u} {d}\n", u = SERVICE_USER));
        let r = q(&layout.requests());
        s.push_str(&format!("rm -rf {r}\nmkdir {r}\nchown {u}:\"$(id -gn {})\" {r}\nchmod 2770 {r}\nchown {u}:{u} {d}\n", q(login), u = SERVICE_USER));
        s.push_str("systemctl daemon-reload\nsystemctl enable sparkbox-agent\nsystemctl restart sparkbox-agent\n");
    } else {
        s.push_str(match apply {
            Apply::Reload => "systemctl reload sparkbox-agent\n",
            Apply::Restart => "systemctl restart sparkbox-agent\n",
        });
    }
    s
}

pub fn uninstall_script(layout: &Layout) -> String {
    let q = sh_quote;
    format!(
        "systemctl disable --now sparkbox-agent || true\nrm -f {}\nrm -rf {} {} {}\nsystemctl daemon-reload\n",
        q(&layout.unit),
        q(&layout.opt),
        q(&layout.etc),
        q(&layout.state)
    )
}

/// Removes the staging folder older versions copied the agent through (never following it
/// if it's a link), as the login user: what's in it is never used.
pub const LEGACY_STAGE: &str = "d=\"$HOME/.sparkbox/agent-stage\"; if [ -L \"$d\" ]; then rm -f \"$d\"; elif [ -e \"$d\" ]; then rm -rf \"$d\"; fi";

/// What `systemctl is-active` and the agent's `status` say.
pub fn parse_status(node: &NodeConfig, out: &str) -> AgentStatus {
    let mut lines = out.lines();
    let active = lines.next().unwrap_or("").trim().to_string();
    let rest: String = lines.collect::<Vec<_>>().join("\n");
    let st: Value = serde_json::from_str(rest.trim()).unwrap_or_default();
    let installed = st.is_object() && !rest.trim().is_empty();
    let state = match active.as_str() {
        "active" | "reloading" | "activating" => "running",
        "failed" => "failed",
        "inactive" | "deactivating" if installed => "stopped",
        _ if !installed => "not-installed",
        _ => "unknown",
    };
    let u = |k: &str| st.get(k).and_then(|v| v.as_u64());
    let s = |k: &str| st.get(k).and_then(|v| v.as_str()).map(String::from);
    let list = |k: &str| st.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let delivery = st.get("lastDelivery").filter(|d| d.is_object());
    AgentStatus {
        node: node.id.clone(),
        name: node.name.clone(),
        installed,
        state: state.into(),
        version: s("version"),
        last_beat_ms: u("lastBeatMs"),
        history_from_ms: u("historyFromMs"),
        firing: st.get("firing").and_then(|f| f.as_array()).map(|a| a.len() as u32).unwrap_or(0),
        last_delivery_ms: delivery.and_then(|d| d.get("ms").or_else(|| d.get("tsMs"))).and_then(|v| v.as_u64()),
        last_delivery_ok: delivery.and_then(|d| d.get("ok")).and_then(|v| v.as_bool()),
        last_delivery_channel: delivery.and_then(|d| d.get("channel")).and_then(|v| v.as_str()).map(String::from),
        uptime_s: u("uptimeS"),
        metrics_port: st.get("metrics").filter(|m| m.get("enabled").and_then(|e| e.as_bool()) == Some(true)).and_then(|m| m.get("port")).and_then(|p| p.as_u64()).map(|p| p as u16),
        error: (state == "failed").then(|| "systemd reports the service failed; it restarts it every 5 s".to_string()),
        simulated: false,
        config_hash: s("configHash"),
        last_cycle_ms: u("lastCycleMs"),
        peers_missing: list("missing"),
        isolated: s("isolated"),
        leader: st.get("leader").and_then(|v| v.as_bool().or_else(|| v.as_str().map(|l| l == node.id))),
        switch_watched: st.get("switchWatched").and_then(|v| v.as_bool()).unwrap_or(false),
        endpoints_probed: list("endpointsProbed"),
        heartbeat_ok_ms: u("heartbeatOkMs"),
        outbox_pending: u("outboxPending").unwrap_or(0) as u32,
        quiet_tz: s("quietTz"),
        ..Default::default()
    }
}

pub fn status_command(layout: &Layout) -> String {
    format!("systemctl is-active sparkbox-agent; test -f {}/sparkbox_agent.py && {}", sh_quote(&layout.opt), layout.cli("status"))
}

/// Reads one node's agent: unit state plus its `status` JSON. Not in the activity log (polling).
pub async fn read_status(live: &Live, node: &NodeConfig, layout: &Layout) -> AgentStatus {
    match live.probe(&node.id, &status_command(layout), Duration::from_secs(15)).await {
        Ok(out) => parse_status(node, &out.stdout),
        Err(e) => AgentStatus { node: node.id.clone(), name: node.name.clone(), state: "unknown".into(), error: Some(e), ..Default::default() },
    }
}

/// Runs a root script with its files on stdin. Returns the last line of what went wrong.
async fn run_root(live: &Live, node: &NodeConfig, script: &str, files: &[File], limit: Duration) -> Result<(), String> {
    let out = live.sudo_with_input(&node.id, script, &stream(files), limit).await?;
    if out.code != Some(0) {
        let why = out.stderr.trim().lines().last().unwrap_or("the install script failed").trim().to_string();
        return Err(why);
    }
    Ok(())
}

/// Installs (or upgrades) the agent on one node, as task steps. Returns a summary.
pub async fn install_node(live: &Live, node: &NodeConfig, layout: &Layout, files: &[File], task: &crate::tasks::TaskHandle) -> Result<String, String> {
    task.step(format!("Install the agent on {}", node.name));
    let _ = live.run(&node.id, LEGACY_STAGE, None, Duration::from_secs(20)).await;
    let script = root_script(layout, files, &node.user, true, Apply::Restart);
    if let Err(why) = run_root(live, node, &script, files, Duration::from_secs(120)).await {
        task.step_failed(why.clone());
        return Err(format!("{}: {why}", node.name));
    }
    task.step_ok(Some(format!("{} files, each checked with SHA-256 as root", files.len())));
    task.step(format!("Check it's running on {}", node.name));
    let started = crate::history::now_ms();
    for _ in 0..20 {
        let st = read_status(live, node, layout).await;
        if st.state == "running" && st.last_beat_ms.is_some_and(|b| b + 5_000 >= started) {
            let v = st.version.clone().unwrap_or_default();
            task.step_ok(Some(format!("agent {v} running, heartbeat {} s ago", crate::history::now_ms().saturating_sub(st.last_beat_ms.unwrap_or(0)) / 1000)));
            return Ok(format!("agent {v} running on {}", node.name));
        }
        if st.state == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let why = format!("The service didn't report a heartbeat on {}. See `journalctl -u sparkbox-agent` there.", node.name);
    task.step_failed(why.clone());
    Err(why)
}

/// Writes new settings and reloads or restarts the agent.
pub async fn push_node(live: &Live, node: &NodeConfig, layout: &Layout, bundle: &Bundle, apply: Apply) -> Result<(), String> {
    let files = bundle.files(apply, false);
    run_root(live, node, &root_script(layout, &files, &node.user, false, apply), &files, Duration::from_secs(60)).await.map_err(|e| format!("{}: {e}", node.name))
}

pub async fn uninstall_node(live: &Live, node: &NodeConfig, layout: &Layout) -> Result<(), String> {
    let out = live.sudo(&node.id, &uninstall_script(layout), Duration::from_secs(60)).await?;
    if out.code != Some(0) {
        return Err(format!("{}: {}", node.name, out.stderr.trim().lines().last().unwrap_or("removing the agent failed")));
    }
    Ok(())
}

/// Why a push runs, for the activity log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushReason {
    /// The owner changed settings, or pressed Push again.
    Requested,
    /// Endpoints, addresses or silences changed by themselves.
    Changed,
    /// A push that failed, tried again.
    Retry,
}

/// Pushes `bundle` to one node and records the outcome in `agents` (agents.json): the hash
/// expected from now on, and a failure kept until a push goes through. Its commands go into
/// the operation it runs in (a task's, or `auto_push`'s).
pub async fn push_one(live: &Live, agents: &Agents, node: &NodeConfig, layout: &Layout, bundle: &Bundle, fingerprint: &str, now: u64) -> Result<Apply, String> {
    if !agents.begin_push(&node.id) {
        return Err(format!("A push to {} is already running.", node.name));
    }
    let rec = agents.record(&node.id);
    let apply = apply_for(&rec, bundle);
    let hash = bundle.hash();
    let result = push_node(live, node, layout, bundle, apply).await;
    agents.update(&node.id, |r| {
        r.expected_hash = Some(hash.clone());
        r.fingerprint = Some(fingerprint.to_string());
        r.pushed_at_ms = Some(now);
        match &result {
            Ok(_) => {
                r.pushed_parts = Some(bundle.parts());
                r.paging = Some(paging_hash(&bundle.notify));
                r.push_error = None;
            }
            Err(e) => {
                let attempts = r.push_error.as_ref().map(|p| p.attempts).unwrap_or(0) + 1;
                r.push_error = Some(PushError { at_ms: now, message: e.trim_start_matches(&format!("{}: ", node.name)).to_string(), attempts });
            }
        }
    });
    agents.end_push(&node.id);
    result.map(|_| apply)
}

/// How long after a failed try the next one waits: 5, 15, 60 minutes, then 6 hours.
pub fn backoff_ms(attempts: u32) -> u64 {
    PUSH_BACKOFF_MS[(attempts.max(1) as usize - 1).min(PUSH_BACKOFF_MS.len() - 1)]
}

/// Whether a node's pending push should be tried again now.
pub fn retry_due(rec: &NodeRecord, came_online: bool, now: u64) -> bool {
    rec.push_error.as_ref().is_some_and(|e| came_online || now.saturating_sub(e.at_ms) >= backoff_ms(e.attempts))
}

/// A push SparkBox starts by itself (a retry, or endpoints and addresses that changed), as one
/// operation of source SparkBox. A retry that fails the same way as the last one goes on that
/// failure's row ("failed · retried 7 times, last 10:42: sudo: a password is required"), so a
/// node that keeps refusing is one row in Changes, not one a try.
pub async fn auto_push(live: &Live, agents: &Agents, node: &NodeConfig, layout: &Layout, bundle: &Bundle, fingerprint: &str, reason: PushReason, now: u64) -> Result<Apply, String> {
    use crate::live::audit::{with_op_id, OpMeta};
    let rec = agents.record(&node.id);
    let row = rec.retry_row.clone().filter(|_| reason == PushReason::Retry);
    let title = match reason {
        PushReason::Changed => format!("Push alert settings to {} (endpoints or addresses changed)", node.name),
        _ => format!("Push alert settings to {}", node.name),
    };
    let op_id = row.as_ref().map(|r| r.op_id.clone()).unwrap_or_else(|| format!("agent-push-{}-{now}", node.id));
    let (head, result) = with_op_id(op_id.clone(), OpMeta { title: title.clone(), source: "system".into() }, async {
        let head = row.as_ref().map(|r| r.head_id.clone()).unwrap_or_else(|| live.audit.begin_op(&node.id, &title, "", false));
        (head, push_one(live, agents, node, layout, bundle, fingerprint, now).await)
    })
    .await;
    let attempts = agents.record(&node.id).push_error.map(|e| e.attempts).unwrap_or(0);
    match &result {
        Ok(apply) => {
            let done = if apply == &Apply::Reload { "reloaded" } else { "restarted" };
            let summary = if row.is_some() { format!("{done} · went through on try {}", rec.push_error.map(|e| e.attempts + 1).unwrap_or(1)) } else { done.to_string() };
            live.audit.end_op(&head, true, Some(summary), None);
            agents.update(&node.id, |r| r.retry_row = None);
        }
        Err(e) => {
            let message = e.trim_start_matches(&format!("{}: ", node.name)).to_string();
            if row.as_ref().is_some_and(|r| r.message == message) {
                let words = format!("failed · retried {} time{}, last {}: {message}", attempts - 1, if attempts == 2 { "" } else { "s" }, crate::notify::clock_in("", now));
                live.audit.update(&head, |x| {
                    x.state = "failed".into();
                    x.error = Some(words);
                });
            } else {
                live.audit.end_op(&head, false, None, Some(message.clone()));
            }
            agents.update(&node.id, |r| r.retry_row = Some(RetryRow { op_id, head_id: head, message }));
        }
    }
    result
}

/// Whether a node's settings changed by themselves since the last push (its secretless
/// `fingerprint` differs) and are due again.
pub fn change_due(rec: &NodeRecord, fingerprint: &str, now: u64) -> bool {
    rec.installed && rec.push_error.is_none() && rec.fingerprint.as_deref().is_some_and(|f| f != fingerprint) && rec.pushed_at_ms.is_none_or(|t| now.saturating_sub(t) >= PUSH_SETTLE_MS)
}

/// The push this round starts by itself, if any. Either kind waits for Admin mode: it's a root
/// change on the node that copies the owner's secrets as they are at that moment, and the release
/// build has no fence to hold it back (live/exit.rs). A retry is no exception: the push it repeats
/// may be one nobody asked for (a failed `Changed` push sets `push_error` too), and what it sends
/// is `bundle_for` now, not what that push sent. It stays due, and goes out once Admin mode is
/// unlocked.
pub fn auto_push_due(rec: &NodeRecord, fingerprint: &str, came_online: bool, unlocked: bool, now: u64) -> Option<PushReason> {
    if !unlocked {
        None
    } else if retry_due(rec, came_online, now) {
        Some(PushReason::Retry)
    } else if change_due(rec, fingerprint, now) {
        Some(PushReason::Changed)
    } else {
        None
    }
}

/// `auto_push_due` for `node` this round. `back` holds the nodes that came online and haven't had
/// a round with Admin unlocked since, so a retry due because the node is back stays due while Admin
/// is locked, and doesn't fall back to the end of its backoff.
pub fn push_due_now(back: &mut HashSet<String>, node: &str, rec: &NodeRecord, fingerprint: &str, unlocked: bool, now: u64) -> Option<PushReason> {
    let due = auto_push_due(rec, fingerprint, back.contains(node), unlocked, now);
    if unlocked {
        back.remove(node);
    }
    due
}

// ---- Maintenance notes for the agents

/// The body of a maintenance request: planned work on `nodes` until `untilMs` (null: a
/// shutdown, which the agents still end after 4 hours), so nothing pages for them; an end
/// record has `end: true`. `op` is the operation it's part of, `startedMs` now.
pub fn maintenance_note(nodes: &[String], until_ms: Option<u64>, reason: &str, by: &str, now: u64, op: &str, end: bool) -> Result<String, String> {
    if nodes.is_empty() {
        return Err("No node to note.".into());
    }
    for node in nodes {
        let ok = !node.is_empty() && node.len() <= 64 && node.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
        if !ok {
            return Err(format!("\"{node}\" isn't a node name."));
        }
    }
    let reason: String = reason.chars().filter(|c| !c.is_control()).take(300).collect();
    let op: String = op.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect();
    let mut v = json!({ "nodes": nodes, "untilMs": if end { Some(now) } else { until_ms.map(|u| u.min(now + MAINT_MAX_MS)) }, "reason": reason, "by": by, "startedMs": now, "op": op });
    if end {
        v["end"] = json!(true);
    }
    let text = serde_json::to_string(&v).map_err(|e| e.to_string())?;
    if text.len() > MAINT_MAX / 2 {
        return Err("That maintenance note is too long.".into());
    }
    Ok(text)
}

/// The shell that drops `content` into requests/<name>.json as the login user (no sudo),
/// atomically, then waits up to `wait` for the agent's answer (<name>.result.json), prints
/// it and removes it.
pub fn request_command(layout: &Layout, name: &str, wait: Duration) -> String {
    let d = sh_quote(&layout.requests());
    let tenths = (wait.as_millis() / 100).max(1);
    format!(
        "umask 022 && d={d} && cat > \"$d/{name}.json.tmp\" && mv -f \"$d/{name}.json.tmp\" \"$d/{name}.json\" && i=0; \
         while [ $i -lt {tenths} ]; do if [ -f \"$d/{name}.result.json\" ]; then cat \"$d/{name}.result.json\"; rm -f \"$d/{name}.result.json\"; exit 0; fi; sleep 0.1; i=$((i+1)); done; \
         echo '{{\"ok\":false,\"waiting\":true}}'"
    )
}

/// Why a request wasn't sent in look-only mode. Nothing was written on the node; the request
/// is kept and goes out once look-only is off.
pub const HELD_LOOK_ONLY: &str = "held while SparkBox is in look-only mode";

/// Drops a signed request on one node and reads the agent's answer. Ok when the agent took
/// it (or had already: "used before"), else why not. In look-only mode it sends nothing and
/// writes no file (`HELD_LOOK_ONLY`).
pub async fn deliver_request(live: &Live, gate: &AdminGate, node: &NodeConfig, layout: &Layout, name: &str, content: &str, wait: Duration) -> Result<(), String> {
    if gate.look_only() {
        return Err(HELD_LOOK_ONLY.into());
    }
    let out = live.run(&node.id, &request_command(layout, name, wait), Some(content.as_bytes()), wait + Duration::from_secs(3)).await?;
    if out.code != Some(0) {
        return Err(out.stderr.trim().lines().last().unwrap_or("couldn't write the request").to_string());
    }
    let v: Value = serde_json::from_str(out.stdout.trim().lines().last().unwrap_or("")).unwrap_or_default();
    if v.get("ok").and_then(|o| o.as_bool()) == Some(true) {
        return Ok(());
    }
    match v.get("error").and_then(|e| e.as_str()) {
        Some("it was used before") => Ok(()),
        Some(e) => Err(format!("the agent refused it: {e}")),
        None if v.get("waiting").is_some() => Err(format!("its agent didn't answer in {} s", wait.as_secs().max(1))),
        None => Err("its agent's answer wasn't readable".into()),
    }
}

/// The request key, made (and kept in the Keychain) the first time an install or push needs it.
fn request_key(create: bool) -> Option<String> {
    if let Ok(Some(k)) = crate::secrets::get(REQUEST_KEY) {
        if !k.is_empty() {
            return Some(k);
        }
    }
    if !create {
        return None;
    }
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).ok()?;
    let k: String = b.iter().map(|x| format!("{x:02x}")).collect();
    crate::secrets::set(REQUEST_KEY, &k).ok()?;
    Some(k)
}

/// The nodes whose agent this Mac knows is installed (Demo: the simulated ones).
fn installed_nodes(app: &AppHandle) -> Vec<NodeConfig> {
    let Some(state) = app_state(app) else { return Vec::new() };
    let Some(agents) = app.try_state::<Arc<Agents>>() else { return Vec::new() };
    let inv = state.inventory.lock().unwrap().clone();
    let demo = !crate::is_live(&state);
    let installed: HashSet<String> = agents.statuses(demo, &inv, crate::history::now_ms()).into_iter().filter(|s| s.installed).map(|s| s.node).collect();
    inv.nodes.into_iter().filter(|n| installed.contains(&n.id)).collect()
}

/// Sends one signed request to every node in `to`, all at once, each waiting at most `wait`
/// for its agent; returns each node's outcome. Unconnected nodes fail without a try.
async fn deliver_all(live: &Live, gate: &AdminGate, to: &[NodeConfig], layout: &Layout, name: &str, content: &str, wait: Duration) -> Vec<(String, Result<(), String>)> {
    let tries = to.iter().map(|n| async move {
        if !live.is_connected(&n.id) {
            return (n.id.clone(), Err("not connected".to_string()));
        }
        (n.id.clone(), deliver_request(live, gate, n, layout, name, content, wait).await)
    });
    futures_util::future::join_all(tries).await
}

/// Tells every installed agent that SparkBox is about to restart or shut down `nodes`
/// (`until_ms`: when they should be back; None for a shutdown, which the agents end after 4
/// hours), so nothing pages for them, and waits for each agent to take it: 5 s at most in
/// all. The commands nest under `op_id`. In Demo the simulated agents note it.
pub async fn maintenance_push(app: &AppHandle, op_id: &str, nodes: &[String], until_ms: Option<u64>, reason: &str) -> PushReport {
    let now = crate::history::now_ms();
    let names = nodes.join(", ");
    let body = match maintenance_note(nodes, until_ms, reason, "SparkBox", now, op_id, false) {
        Ok(b) => b,
        Err(e) => {
            crate::logging::write("agent", &format!("maintenance note for {names} not sent: {e}"));
            return PushReport { reached: Vec::new(), missing: installed_nodes(app).into_iter().map(|n| (n.id, e.clone())).collect() };
        }
    };
    let title = format!("Tell the agents about the planned work on {names}");
    send_note(app, op_id, &title, &body, now).await
}

/// Ends the maintenance window on `node` on every installed agent. One that isn't reached
/// keeps the end pending and gets it when its node is back. Returns how many took it.
pub async fn maintenance_clear(app: &AppHandle, node: &str) -> usize {
    let now = crate::history::now_ms();
    let op_id = crate::live::audit::current_op().unwrap_or_else(|| format!("agent-maint-end-{now}"));
    let Ok(body) = maintenance_note(&[node.to_string()], None, "ended", "SparkBox", now, &op_id, true) else { return 0 };
    let report = send_note(app, &op_id, &format!("Tell the agents the planned work on {node} is over"), &body, now).await;
    if let Some(agents) = app.try_state::<Arc<Agents>>() {
        for (target, why) in &report.missing {
            crate::logging::write("agent", &format!("the end of the window on {node} didn't reach {target}: {why}; kept to send again"));
            agents.update(target, |r| {
                r.pending_clears.retain(|p| p.node != node);
                r.pending_clears.push(PendingClear { node: node.to_string(), since_ms: now });
            });
        }
    }
    report.reached.len()
}

async fn send_note(app: &AppHandle, op_id: &str, title: &str, body: &str, now: u64) -> PushReport {
    use crate::live::audit::{with_op_id, OpMeta};
    let to = installed_nodes(app);
    let Some(state) = app_state(app) else { return PushReport::default() };
    let name = format!("maintenance-{now}-{:06x}", fastrand::u32(..0xff_ffff));
    if !crate::is_live(&state) {
        let live = state.live.clone();
        drop(state);
        let ids: Vec<String> = to.iter().map(|n| n.id.clone()).collect();
        with_op_id(op_id.to_string(), OpMeta { title: title.into(), source: String::new() }, async move {
            for n in &ids {
                live.audit.record_simulated(n, title, &format!("cat > /var/lib/sparkbox-agent/requests/{name}.json  # signed; the agent answers in {name}.result.json"));
            }
        })
        .await;
        return PushReport { reached: to.into_iter().map(|n| n.id).collect(), missing: Vec::new() };
    }
    let live = state.live.clone();
    drop(state);
    let Some(gate) = app.try_state::<Arc<AdminGate>>().map(|g| g.inner().clone()) else { return PushReport::default() };
    let Some(key) = request_key(false) else {
        return PushReport { reached: Vec::new(), missing: to.into_iter().map(|n| (n.id, "no request key yet: push the alert settings first".to_string())).collect() };
    };
    let content = signed_request(&key, "maintenance", body);
    let layout = Layout::system();
    let wait = NOTE_TIMEOUT - Duration::from_millis(500);
    let tries = with_op_id(op_id.to_string(), OpMeta { title: title.into(), source: String::new() }, deliver_all(&live, &gate, &to, &layout, &name, &content, wait));
    let results = tokio::time::timeout(NOTE_TIMEOUT, tries).await.unwrap_or_else(|_| to.iter().map(|n| (n.id.clone(), Err("its agent didn't answer in 5 s".to_string()))).collect());
    let mut report = PushReport::default();
    for (n, r) in results {
        match r {
            Ok(()) => report.reached.push(n),
            Err(e) => report.missing.push((n, e)),
        }
    }
    report
}

// ---- Acknowledgements for the agents

/// An alert's entity as the agents' gate spells it (gate.py entity_key): node:cell3,
/// endpoint:llama, disk:cell1:/, gpu:cell1:gpu0, net:cell2/enP2p1s0f1np1, container:cell1/vllm-head.
pub fn gate_entity(e: &crate::health::Entity) -> String {
    let scope = e.node.as_deref().or(e.switch.as_deref()).unwrap_or("");
    match e.kind.as_str() {
        "node" | "endpoint" | "switch" => format!("{}:{}", e.kind, e.id),
        "port" => format!("port:{}/{}", e.switch.as_deref().unwrap_or(scope), e.id),
        "disk" | "gpu" => format!("{}:{scope}:{}", e.kind, e.id),
        "interface" => format!("net:{scope}/{}", e.id),
        k => format!("{k}:{scope}/{}", e.id),
    }
}

/// The acknowledgements to hand the agents: every firing alert acknowledged on this Mac, as
/// (request name, body). Until it gets worse, as on the Mac.
pub fn ack_bodies(alerts: &[crate::health::Alert]) -> Vec<(String, String)> {
    alerts
        .iter()
        .filter(|a| a.state == crate::health::AlertState::Firing)
        .filter_map(|a| {
            let acked = a.acked_ms?;
            let body = serde_json::to_string(&json!({
                "rule": a.rule, "entity": gate_entity(&a.entity), "incident": a.incident_id.clone().or_else(|| a.root_key.clone()),
                "untilEscalated": true, "ackedMs": acked, "by": "SparkBox on this Mac",
            }))
            .ok()?;
            Some((format!("ack-{}", &sha256_hex(body.as_bytes())[..16]), body))
        })
        .collect()
}

/// An alert was acknowledged on this Mac (or the acknowledgement ended): tell every installed
/// agent, so the one paging for it stops repeating "Still firing". One that isn't reached
/// gets it when its node is back, and every 15 minutes until it takes it.
pub fn acks_changed(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let to = installed_nodes(&app);
        send_acks(&app, &to, "ui").await;
    });
}

/// Sends the acknowledgements `to` hasn't taken yet, as one operation.
async fn send_acks(app: &AppHandle, to: &[NodeConfig], source: &str) {
    let Some(health) = app.try_state::<Arc<crate::health::Health>>() else { return };
    let Some(agents) = app.try_state::<Arc<Agents>>() else { return };
    let Some(gate) = app.try_state::<Arc<AdminGate>>() else { return };
    let Some(state) = app_state(app) else { return };
    let demo = !crate::is_live(&state);
    let wanted = ack_bodies(&health.view().alerts);
    let live = state.live.clone();
    drop(state);
    let key = if demo { Some(String::new()) } else { request_key(false) };
    let Some(key) = key else { return };
    deliver_acks(&live, &gate, &agents, to, &wanted, &key, AckRun { demo, source, layout: &Layout::system(), now: crate::history::now_ms() }).await;
}

/// How one round of acknowledgements runs: Demo or Live, who started it, where requests go.
pub struct AckRun<'a> {
    pub demo: bool,
    /// ui | system.
    pub source: &'a str,
    pub layout: &'a Layout,
    pub now: u64,
}

/// Sends each node in `to` the acknowledgements in `wanted` it hasn't taken yet, as one
/// operation; returns how many it took. In look-only mode it sends nothing and logs nothing:
/// every acknowledgement stays not-taken and its node is due again (`Agents::acks_due`), so
/// they go out on the agents' first round after look-only ends.
pub async fn deliver_acks(live: &Live, gate: &AdminGate, agents: &Agents, to: &[NodeConfig], wanted: &[(String, String)], key: &str, run: AckRun<'_>) -> usize {
    use crate::live::audit::{with_op_id, OpMeta};
    let AckRun { demo, source, layout, now } = run;
    let due: Vec<(NodeConfig, Vec<(String, String)>)> = to
        .iter()
        .filter_map(|n| {
            let left: Vec<(String, String)> = wanted.iter().filter(|(name, _)| !agents.ack_taken(&n.id, name)).cloned().collect();
            (!left.is_empty()).then(|| (n.clone(), left))
        })
        .collect();
    if due.is_empty() {
        return 0;
    }
    if gate.look_only() {
        for (node, _) in &due {
            agents.ack_held(&node.id);
        }
        return 0;
    }
    let count: usize = due.iter().map(|(_, l)| l.len()).max().unwrap_or(0);
    let title = format!("Tell the agents about {} acknowledged alert{}", count, if count == 1 { "" } else { "s" });
    let op_id = format!("agent-acks-{now}");
    with_op_id(op_id.clone(), OpMeta { title: title.clone(), source: source.into() }, async {
        let head = live.audit.begin_op(&due.iter().map(|(n, _)| n.id.clone()).collect::<Vec<_>>().join(", "), &title, "", demo);
        let (mut ok, mut failed) = (0, Vec::new());
        for (node, left) in &due {
            agents.ack_tried(&node.id, now);
            for (name, body) in left {
                let r = if demo {
                    live.audit.record_simulated(&node.id, &title, &format!("cat > /var/lib/sparkbox-agent/requests/{name}.json  # signed"));
                    Ok(())
                } else if !live.is_connected(&node.id) {
                    Err("not connected".to_string())
                } else {
                    deliver_request(live, gate, node, layout, name, &signed_request(key, "ack", body), Duration::from_secs(4)).await
                };
                match r {
                    Ok(()) => {
                        agents.ack_took(&node.id, name);
                        ok += 1;
                    }
                    Err(e) => failed.push(format!("{}: {e}", node.name)),
                }
            }
        }
        let summary = format!("{ok} of {} taken", ok + failed.len());
        live.audit.end_op(&head, failed.is_empty(), Some(summary), (!failed.is_empty()).then(|| failed.join("; ")));
        ok
    })
    .await
}

// ---- Backfill: what the agents recorded while SparkBox was closed

/// One line of `sparkbox_agent.py export`.
#[derive(Clone, Debug, Deserialize)]
pub struct ExportRow {
    pub e: String,
    pub t: u64,
    #[allow(dead_code)]
    pub s: u64,
    pub v: BTreeMap<String, f64>,
}

/// Points of one entity: (unix ms, metric → value).
type Points = Vec<(u64, BTreeMap<String, f64>)>;

/// Merges export lines into History, entity by entity. Returns the points taken.
pub fn merge_export(history: &crate::history::History, demo: bool, text: &str) -> usize {
    let mut by_entity: BTreeMap<String, Points> = BTreeMap::new();
    for line in text.lines() {
        if let Ok(r) = serde_json::from_str::<ExportRow>(line) {
            by_entity.entry(r.e).or_default().push((r.t, r.v));
        }
    }
    by_entity.iter().map(|(e, points)| history.merge_agent(demo, e, points)).sum()
}

/// The agent's alert events that happened before `session_start` (while SparkBox was
/// closed), marked as such. Later ones SparkBox saw itself.
pub fn away_events(text: &str, node: &str, session_start: u64) -> Vec<crate::health::AlertEvent> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<crate::health::AlertEvent>(l).ok())
        .filter(|e| e.ts_ms < session_start)
        .map(|mut e| {
            e.away = true;
            e.source = Some(format!("agent:{node}"));
            e
        })
        .collect()
}

/// `history`: also its recorded history (on connect, history::backfill pulls that, the last
/// hour first, so only the alert log is read here).
pub async fn backfill_node(app: &AppHandle, live: &Live, node: &NodeConfig, layout: &Layout, session_start: u64, history: bool) -> Result<(usize, usize), String> {
    let mut points = 0;
    if history {
        let history = app.state::<Arc<crate::history::History>>().inner().clone();
        let since = history.last_point_ms(false, &format!("node:{}", node.id)).unwrap_or(0).max(crate::history::now_ms().saturating_sub(31 * 86_400_000));
        let out = live.probe(&node.id, &layout.cli(&format!("export --since {since}")), Duration::from_secs(120)).await?;
        points = merge_export(&history, false, &out.stdout);
    }
    let health = app.state::<Arc<crate::health::Health>>().inner().clone();
    let last_away = health.history(crate::health::LOG_KEEP).iter().filter(|e| e.source.as_deref() == Some(&format!("agent:{}", node.id))).map(|e| e.ts_ms).max().unwrap_or(0);
    let out = live.probe(&node.id, &layout.cli(&format!("alerts --since {}", last_away + 1)), Duration::from_secs(60)).await?;
    let alerts = health.import(away_events(&out.stdout, &node.id, session_start));
    Ok((points, alerts))
}

// ---- The managed state

pub struct Agents {
    path: Option<PathBuf>,
    file: Mutex<AgentsFile>,
    /// Live: the last status read from each node.
    live: Mutex<BTreeMap<String, AgentStatus>>,
    last_backfill: Mutex<HashMap<String, u64>>,
    /// Nodes a push is running to.
    pushing: Mutex<HashSet<String>>,
    /// Since when each node's outbox has had something waiting.
    outbox_since: Mutex<HashMap<String, u64>>,
    last_test_page: Mutex<Option<u64>>,
    pub demo: Mutex<demo::DemoAgents>,
    session_start: u64,
    last_emitted: Mutex<Option<String>>,
    /// Acknowledgements each node's agent has taken (request names), and when one was last tried.
    acks_taken: Mutex<HashMap<String, HashSet<String>>>,
    acks_tried: Mutex<HashMap<String, u64>>,
    /// The settings push task running now, so Push again joins it instead of starting another.
    push_task: Mutex<Option<String>>,
}

impl Agents {
    pub fn new(path: Option<PathBuf>) -> Arc<Agents> {
        let file = path.as_deref().map(crate::util::load_json_or_default::<AgentsFile>).unwrap_or_default();
        let now = crate::history::now_ms();
        Arc::new(Agents {
            path,
            file: Mutex::new(file),
            live: Mutex::new(BTreeMap::new()),
            last_backfill: Mutex::new(HashMap::new()),
            pushing: Mutex::new(HashSet::new()),
            outbox_since: Mutex::new(HashMap::new()),
            last_test_page: Mutex::new(None),
            demo: Mutex::new(demo::DemoAgents::new(now)),
            session_start: now,
            last_emitted: Mutex::new(None),
            acks_taken: Mutex::new(HashMap::new()),
            acks_tried: Mutex::new(HashMap::new()),
            push_task: Mutex::new(None),
        })
    }

    fn save(&self, f: &AgentsFile) -> Result<(), String> {
        match &self.path {
            Some(p) => crate::util::save_json(p, f),
            None => Ok(()),
        }
    }

    pub fn settings(&self) -> AgentSettings {
        self.file.lock().unwrap().settings.clone()
    }

    pub fn save_settings(&self, s: AgentSettings) -> Result<AgentSettings, String> {
        if s.metrics_port < 1024 || s.port < 1024 || s.metrics_port == s.port {
            return Err("Use two different ports from 1024 up.".into());
        }
        let mut f = self.file.lock().unwrap();
        let next = AgentsFile { settings: s.clone(), nodes: f.nodes.clone() };
        self.save(&next)?;
        *f = next;
        Ok(s)
    }

    pub fn record(&self, node: &str) -> NodeRecord {
        self.file.lock().unwrap().nodes.get(node).cloned().unwrap_or_default()
    }

    /// Changes one node's record and saves agents.json.
    pub fn update(&self, node: &str, f: impl FnOnce(&mut NodeRecord)) {
        let mut file = self.file.lock().unwrap();
        f(file.nodes.entry(node.to_string()).or_default());
        if let Err(e) = self.save(&file) {
            crate::logging::write("agent", &format!("agents.json wasn't saved: {e}"));
        }
    }

    fn begin_push(&self, node: &str) -> bool {
        self.pushing.lock().unwrap().insert(node.to_string())
    }

    fn end_push(&self, node: &str) {
        self.pushing.lock().unwrap().remove(node);
    }

    /// The statuses with what this Mac recorded (expected hash, push errors, tests).
    pub fn statuses(&self, demo: bool, inv: &Inventory, now: u64) -> Vec<AgentStatus> {
        if demo {
            return self.demo.lock().unwrap().statuses(inv, now);
        }
        let live = self.live.lock().unwrap();
        let file = self.file.lock().unwrap();
        inv.nodes
            .iter()
            .map(|n| {
                let mut st = live.get(&n.id).cloned().unwrap_or(AgentStatus { node: n.id.clone(), name: n.name.clone(), state: "unknown".into(), ..Default::default() });
                let rec = file.nodes.get(&n.id).cloned().unwrap_or_default();
                if st.state == "unknown" && rec.installed {
                    // Not read yet (or unreachable): it's installed as far as this Mac knows.
                    st.installed = true;
                }
                st.expected_hash = rec.expected_hash.clone();
                st.config_current = match (&st.config_hash, &rec.expected_hash) {
                    (Some(r), Some(e)) => Some(r == e && rec.push_error.is_none()),
                    _ if st.installed => Some(false),
                    _ => None,
                };
                st.push_error = rec.push_error.clone();
                st.last_test = rec.last_test.clone();
                st.paging = rec.paging.clone();
                st
            })
            .collect()
    }

    /// Takes a status just read, keeps what it says about the node in agents.json.
    pub fn set_live(&self, st: AgentStatus) {
        let now = crate::history::now_ms();
        {
            let mut since = self.outbox_since.lock().unwrap();
            if st.outbox_pending > 0 {
                since.entry(st.node.clone()).or_insert(now);
            } else {
                since.remove(&st.node);
            }
        }
        let rec = self.record(&st.node);
        if st.state != "unknown" && (rec.installed != st.installed || rec.reported_hash != st.config_hash) {
            let (installed, hash) = (st.installed, st.config_hash.clone());
            self.update(&st.node, |r| {
                r.installed = installed;
                r.reported_hash = hash;
            });
        }
        self.live.lock().unwrap().insert(st.node.clone(), st);
    }

    pub fn outbox_since(&self, node: &str) -> Option<u64> {
        self.outbox_since.lock().unwrap().get(node).copied()
    }

    /// Remembers a node's fabric addresses while it's online.
    pub fn note_fabric(&self, node: &str, fabric: Vec<String>) {
        if !fabric.is_empty() && self.record(node).fabric != fabric {
            self.update(node, |r| r.fabric = fabric);
        }
    }

    pub fn session_start(&self) -> u64 {
        self.session_start
    }

    pub fn ack_taken(&self, node: &str, name: &str) -> bool {
        self.acks_taken.lock().unwrap().get(node).is_some_and(|s| s.contains(name))
    }

    pub fn ack_took(&self, node: &str, name: &str) {
        self.acks_taken.lock().unwrap().entry(node.to_string()).or_default().insert(name.to_string());
    }

    pub fn ack_tried(&self, node: &str, now: u64) {
        self.acks_tried.lock().unwrap().insert(node.to_string(), now);
    }

    /// Look-only held this node's acknowledgements: they're due again as soon as it ends.
    pub fn ack_held(&self, node: &str) {
        self.acks_tried.lock().unwrap().remove(node);
    }

    /// Whether acknowledgements a node hasn't taken are due again: when it's back, or every 15 minutes.
    pub fn acks_due(&self, node: &str, came_online: bool, now: u64) -> bool {
        came_online || self.acks_tried.lock().unwrap().get(node).is_none_or(|t| now.saturating_sub(*t) >= ACK_RETRY_MS)
    }

    /// The push task running now, if any; `tasks` says whether it still is.
    pub fn running_push(&self, tasks: &crate::tasks::Tasks) -> Option<String> {
        let mut cur = self.push_task.lock().unwrap();
        if cur.as_ref().is_some_and(|id| tasks.get(id).is_none_or(|t| t.finished())) {
            *cur = None;
        }
        cur.clone()
    }

    pub fn set_push_task(&self, id: Option<String>) {
        *self.push_task.lock().unwrap() = id;
    }

    /// Allows "Send a test page from every node" once a minute.
    pub fn may_test_page(&self, now: u64) -> bool {
        let mut last = self.last_test_page.lock().unwrap();
        if last.is_some_and(|t| now.saturating_sub(t) < TEST_PAGE_EVERY_MS) {
            return false;
        }
        *last = Some(now);
        true
    }
}

fn app_state(app: &AppHandle) -> Option<tauri::State<'_, crate::AppState>> {
    app.try_state::<crate::AppState>()
}

/// The endpoints whose head is `node` (r3-serving's agent_targets), and the API keys they
/// need. Without `secrets` the Keychain isn't read: keys are left out and none counts as saved.
fn endpoints_for(app: &AppHandle, node: &str, secrets: bool) -> (Vec<Value>, BTreeMap<String, String>) {
    let Some(state) = app_state(app) else { return (Vec::new(), BTreeMap::new()) };
    let has_key = |account: &str| secrets && crate::secrets::get(account).ok().flatten().is_some_and(|k| !k.is_empty());
    let targets = crate::models::endpoints::agent_targets_from(&state.models.endpoints(), &state.models.chat_only(), has_key);
    let mine: Vec<_> = targets.into_iter().filter(|t| t.head_node == node).collect();
    let keys = mine
        .iter()
        .filter_map(|t| Some((t.id.clone(), crate::secrets::get(t.key_ref.as_deref().filter(|_| secrets)?).ok().flatten()?)))
        .collect();
    (mine.iter().filter_map(|t| serde_json::to_value(t).ok()).collect(), keys)
}

/// The Live silences, as agent.json lists them.
fn silences_json(app: &AppHandle) -> Vec<Value> {
    let Some(health) = app.try_state::<Arc<crate::health::Health>>() else { return Vec::new() };
    health
        .silences()
        .iter()
        .map(|x| {
            let until = x.ends_ms();
            json!({
                "id": x.id, "rule": x.matcher.rule, "entity": x.matcher.entity, "incident": x.matcher.incident,
                "untilMs": until, "untilClears": x.until == crate::health::silence::Until::Resolved, "reason": x.reason,
            })
        })
        .collect()
}

/// Everything a node's agent is given (Live). Without `secrets` the channels' secrets and
/// the metrics token are left out (for the fingerprint), so the Keychain isn't read.
pub fn bundle_for(app: &AppHandle, inv: &Inventory, node: &NodeConfig, secrets: bool) -> Bundle {
    let agents = app.state::<Arc<Agents>>();
    let settings = agents.settings();
    let snap = app_state(app).and_then(|s| s.latest.read().unwrap().clone());
    let mut addrs: BTreeMap<String, Addrs> = BTreeMap::new();
    for n in &inv.nodes {
        let live = snap.as_ref().and_then(|s| s.nodes.iter().find(|x| x.id == n.id));
        let mut fabric: Vec<String> = live.map(|x| x.nets.iter().filter(|i| i.role == NetRole::Fabric).filter_map(|i| i.ip.clone()).collect()).unwrap_or_default();
        if fabric.is_empty() {
            fabric = agents.record(&n.id).fabric;
        } else {
            agents.note_fabric(&n.id, fabric.clone());
        }
        let lan = live.and_then(|x| x.nets.iter().find(|i| i.role == NetRole::Lan).and_then(|i| i.ip.clone()));
        addrs.insert(n.id.clone(), Addrs { lan, fabric });
    }
    let switch_mgmt = inv.switches.iter().find(|s| s.kind == SwitchKind::RouterOs && !s.host.is_empty()).map(|s| json!({ "id": s.id, "name": s.name, "host": s.host }));
    let (endpoints, keys) = endpoints_for(app, &node.id, secrets);
    let extras = Extras { switch_mgmt, endpoints, silences: silences_json(app) };
    let rules = app.try_state::<Arc<crate::health::Health>>().map(|h| (*h.rules()).clone()).unwrap_or_default();
    let notify = app.try_state::<Arc<crate::notify::Notifier>>().map(|n| n.settings()).unwrap_or_default();
    let secret = |id: &str, f: &str| if secrets { crate::secrets::get(&crate::notify::secret_name(id, f)).ok().flatten() } else { Some(format!("<{id}.{f}>")) };
    let token = settings.has_token.then(|| if secrets { crate::secrets::get(METRICS_TOKEN).ok().flatten() } else { Some("<metrics>".into()) }).flatten();
    // Made once, on the first install or push; the fingerprint stands in a placeholder.
    let key = if secrets { request_key(true) } else { Some("<request-key>".into()) };
    Bundle {
        agent: agent_json(inv, &node.id, &addrs, &extras, &notify, &settings),
        rules: rules_json(&rules),
        notify: notify_json(&notify, secret, &keys, token, key),
    }
}

/// Whether installing on or pushing to `nodes` copies one of the owner's secrets there (a
/// channel secret, an endpoint key, the metrics token): then it needs Mac scope. Demo copies
/// nothing. Read without the Keychain.
pub fn push_copies_secrets(app: &AppHandle, nodes: &[String]) -> bool {
    let Some(state) = app_state(app) else { return false };
    if !crate::is_live(&state) {
        return false;
    }
    // No nodes named: a push to every node.
    let all: Vec<String> = if nodes.is_empty() { state.inventory.lock().unwrap().nodes.iter().map(|n| n.id.clone()).collect() } else { nodes.to_vec() };
    drop(state);
    let settings = app.try_state::<Arc<crate::notify::Notifier>>().map(|n| n.settings()).unwrap_or_default();
    let token = app.try_state::<Arc<Agents>>().is_some_and(|a| a.settings().has_token);
    let keys = all.iter().any(|n| endpoints_for(app, n, false).0.iter().any(|e| e.get("keyRef").is_some_and(|k| !k.is_null())));
    copies_secrets(&settings, keys, token)
}

/// The current watch state.
pub fn current(app: &AppHandle) -> WatchState {
    let now = crate::history::now_ms();
    let Some(agents) = app.try_state::<Arc<Agents>>() else { return watch_state(Vec::new(), false, now) };
    let Some(state) = app_state(app) else { return watch_state(Vec::new(), false, now) };
    let demo = !crate::is_live(&state);
    let inv = state.inventory.lock().unwrap().clone();
    let notifier = app.try_state::<Arc<crate::notify::Notifier>>();
    let settings = notifier.as_ref().map(|n| n.settings()).unwrap_or_default();
    let last = notifier.as_ref().map(|n| n.deliveries.last_by_channel()).unwrap_or_default();
    let mut channels: Vec<WatchChannel> = settings
        .channels
        .iter()
        .map(|c| {
            let d = last.get(&c.id);
            WatchChannel {
                id: c.id.clone(),
                name: c.name.clone(),
                kind: c.kind.label().into(),
                remote: c.kind.pages(),
                mac_only: c.mac_only,
                enabled: c.enabled,
                last_ok_ms: d.filter(|d| d.ok).map(|d| d.ts_ms),
                last_error: d.filter(|d| !d.ok).and_then(|d| d.error.clone()),
                last_error_ms: d.filter(|d| !d.ok).map(|d| d.ts_ms),
                simulated: false,
            }
        })
        .collect();
    let statuses = agents.statuses(demo, &inv, now);
    let mut ws = watch_state(statuses, channels.iter().any(|c| c.remote && c.enabled), now);
    ws.demo = demo;
    ws.changed_ms = settings.changed_ms;
    let endpoint_names: HashMap<String, String> = state
        .models
        .endpoints()
        .iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .filter_map(|v| Some((v.get("id")?.as_str()?.to_string(), v.get("models").and_then(|m| m.get(0)).and_then(|m| m.as_str()).unwrap_or("").to_string())))
        .collect();
    let silences = app.try_state::<Arc<crate::health::Health>>().map(|h| h.view().silences.iter().filter(|x| !x.expired(now)).count() as u32).unwrap_or(0);
    if demo {
        // The simulation's own phone channel and dead-man's switch (rows in the channel list,
        // marked Demo) and its test page; only channel changes made during this Demo session
        // count against that test.
        let mut sim = agents.demo.lock().unwrap();
        sim.note_channels(&settings, now);
        channels.push(sim.channel(now));
        channels.push(sim.deadman_channel(now));
        ws.end_to_end = sim.end_to_end(&inv, now);
        ws.changed_ms = sim.changed_ms();
        ws.deadman = deadman(&channels, &settings, sim.deadman(now).last_ping_ms, sim.deadman(now).by);
    } else {
        ws.end_to_end = end_to_end(&ws.agents, &channels, settings.changed_ms);
        let best = ws.agents.iter().filter_map(|a| a.heartbeat_ok_ms.map(|t| (t, a.node.clone()))).max();
        ws.deadman = deadman(&channels, &settings, best.as_ref().map(|b| b.0), best.map(|b| b.1));
    }
    // Who pages, now the channels are known (Demo's simulated one included).
    let all_current = ws.agents.iter().filter(|a| a.installed).all(|a| a.config_current == Some(true));
    ws.delivery_owner = if !channels.iter().any(|c| c.remote && c.enabled && !c.mac_only) {
        "none".into()
    } else if ws.mode == "agents" && all_current {
        "agents".into()
    } else {
        "mac".into()
    };
    ws.watched = watched(&ws.agents, &inv, &endpoint_names, now);
    ws.silences = silences;
    ws.paging_failure = paging_failure(notifier.as_ref().map(|n| n.inner().as_ref()), &ws.agents, &agents, &channels, now);
    let items = secret_items(&settings, &[]);
    ws.secret_copies = ws.agents.iter().filter(|a| a.installed || !demo).map(|a| SecretCopy { node: a.node.clone(), name: a.name.clone(), items: items.clone() }).collect();
    ws.channels = channels;
    ws
}

/// Every installed node tested every phone channel the agents send, through the channels it
/// runs now: its test's paging hash is the one of the settings it runs (`paging_hash`), so a
/// silence, an ack, quiet hours or a rule edit leave the test standing and settings put back
/// as they were restore it. A test from before the hash was kept counts when it's newer than
/// the last change.
fn end_to_end(agents: &[AgentStatus], channels: &[WatchChannel], changed_ms: u64) -> EndToEnd {
    let remote: Vec<&WatchChannel> = channels.iter().filter(|c| c.remote && c.enabled && !c.mac_only).collect();
    let installed: Vec<&AgentStatus> = agents.iter().filter(|a| a.installed).collect();
    let mut cells = Vec::new();
    for a in &installed {
        if let Some(t) = &a.last_test {
            for (id, r) in &t.results {
                let name = channels.iter().find(|c| &c.id == id).map(|c| c.name.clone()).or_else(|| r.name.clone()).unwrap_or_else(|| id.clone());
                cells.push(EndToEndCell { node: a.node.clone(), channel: id.clone(), channel_name: name, ok: r.ok, error: r.error.clone(), ms: r.ms, at_ms: t.at_ms });
            }
        }
    }
    let tested_at_ms = installed.iter().map(|a| a.last_test.as_ref().map(|t| t.at_ms)).collect::<Option<Vec<u64>>>().and_then(|v| v.into_iter().min());
    let proven = |a: &AgentStatus| {
        let Some(t) = &a.last_test else { return false };
        match (&t.paging, &a.paging) {
            (Some(tested), Some(runs)) => tested == runs,
            _ => t.at_ms >= changed_ms,
        }
    };
    let ok = !installed.is_empty()
        && !remote.is_empty()
        && installed.iter().all(|a| proven(a))
        && installed.iter().all(|a| remote.iter().all(|c| a.last_test.as_ref().and_then(|t| t.results.get(&c.id)).is_some_and(|r| r.ok)));
    EndToEnd { tested_at_ms, ok, cells }
}

/// The dead-man's switch: set up when the channel list has one, enabled (Demo's own row
/// included); pinged by the agents' last heartbeat.
fn deadman(channels: &[WatchChannel], s: &NotifySettings, last_ping_ms: Option<u64>, by: Option<String>) -> Deadman {
    let kind = crate::delivery::Kind::Heartbeat.label();
    let row = channels.iter().find(|c| c.enabled && c.kind == kind);
    let service = row.and_then(|r| s.channels.iter().find(|c| c.id == r.id).map(|c| c.target()).or_else(|| r.simulated.then(|| "healthchecks.io".to_string())));
    Deadman { configured: row.is_some(), service, last_ping_ms: row.and(last_ping_ms), by: row.and(by) }
}

fn watched(agents: &[AgentStatus], inv: &Inventory, names: &HashMap<String, String>, now: u64) -> Watched {
    let ready: Vec<&AgentStatus> = agents.iter().filter(|a| a.healthy(now) && a.config_current == Some(true)).collect();
    let any = !ready.is_empty();
    let mut endpoints: Vec<String> = ready.iter().flat_map(|a| a.endpoints_probed.iter()).map(|id| names.get(id).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| id.clone())).collect();
    endpoints.sort();
    endpoints.dedup();
    let switch = ready.iter().any(|a| a.switch_watched);
    Watched {
        nodes: any,
        gpus: any,
        fabric: any,
        containers: any,
        endpoints,
        switch,
        // The leader checks the RouterOS switch; the Firewallas only this Mac does.
        unwatched: inv.switches.iter().filter(|s| !(switch && s.kind == SwitchKind::RouterOs)).map(|s| s.name.clone()).collect(),
    }
}

/// A delivery that failed in the last hour (this Mac's, or an agent's), or an agent's
/// outbox waiting for more than 5 minutes.
fn paging_failure(notifier: Option<&crate::notify::Notifier>, agents: &[AgentStatus], state: &Agents, channels: &[WatchChannel], now: u64) -> Option<PagingFailure> {
    let hour = now.saturating_sub(3_600_000);
    let name = |id: &str| channels.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_else(|| id.to_string());
    if let Some(d) = notifier.and_then(|n| n.deliveries.last_failure(hour)) {
        return Some(PagingFailure { at_ms: d.ts_ms, channel: d.channel_name.clone(), message: d.error.unwrap_or_default(), by: d.via });
    }
    for a in agents {
        if a.last_delivery_ok == Some(false) && a.last_delivery_ms.is_some_and(|t| t >= hour) {
            let ch = a.last_delivery_channel.as_deref().map(name).unwrap_or_else(|| "a channel".into());
            return Some(PagingFailure { at_ms: a.last_delivery_ms.unwrap_or(now), channel: ch, message: "the agent's last delivery failed".into(), by: a.node.clone() });
        }
        let since = if a.simulated { None } else { state.outbox_since(&a.node) };
        if let Some(since) = since.filter(|s| now.saturating_sub(*s) > 5 * 60_000) {
            return Some(PagingFailure { at_ms: since, channel: String::new(), message: format!("{} message(s) waiting in its outbox since {}", a.outbox_pending, crate::notify::clock_in("", since)), by: a.node.clone() });
        }
    }
    None
}

fn emit(app: &AppHandle) -> WatchState {
    let ws = current(app);
    if let Some(agents) = app.try_state::<Arc<Agents>>() {
        let mut last = agents.last_emitted.lock().unwrap();
        // Heartbeat ages move every second; emit on what the UI shows changing state.
        let mut copy = ws.clone();
        for a in &mut copy.agents {
            a.last_beat_ms = a.last_beat_ms.map(|b| b / 15_000);
            a.last_cycle_ms = None;
            a.uptime_s = None;
        }
        copy.deadman.last_ping_ms = copy.deadman.last_ping_ms.map(|b| b / 15_000);
        let key = serde_json::to_string(&copy).unwrap_or_default();
        if last.as_deref() != Some(&key) {
            *last = Some(key);
            let _ = app.emit(STATE_EVENT, &ws);
        }
    }
    ws
}

/// Fills in each node's agent heartbeat and settings state (and, in Demo, the scripted
/// kernel events) before a snapshot is judged and shown.
pub fn overlay(app: &AppHandle, snap: &mut ClusterSnapshot) {
    let Some(agents) = app.try_state::<Arc<Agents>>() else { return };
    if snap.demo {
        let heads: Vec<(String, String)> = app_state(app)
            .map(|s| s.models.endpoints().iter().filter_map(|e| serde_json::to_value(e).ok()).filter_map(|v| Some((v.get("id")?.as_str()?.to_string(), v.get("headNode")?.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        let changed = agents.demo.lock().unwrap().overlay(snap, &heads);
        if changed {
            emit(app);
        }
        return;
    }
    let inv = app_state(app).map(|s| s.inventory.lock().unwrap().clone()).unwrap_or_default();
    let statuses: HashMap<String, AgentStatus> = agents.statuses(false, &inv, snap.ts_ms).into_iter().map(|s| (s.node.clone(), s)).collect();
    for n in &mut snap.nodes {
        if let Some(st) = statuses.get(&n.id).filter(|s| s.installed) {
            n.agent = st.last_beat_ms.map(|b| crate::model::AgentBeat {
                version: st.version.clone().unwrap_or_default(),
                last_beat_ms: b,
                history_from_ms: st.history_from_ms,
                firing: st.firing,
                config_hash: st.config_hash.clone(),
                config_current: st.config_current,
                push_error: st.push_error.as_ref().map(|e| e.message.clone()),
                push_error_ms: st.push_error.as_ref().map(|e| e.at_ms),
                last_cycle_ms: st.last_cycle_ms,
                peers_missing: st.peers_missing.clone(),
                isolated: st.isolated.clone(),
                endpoints_probed: st.endpoints_probed.clone(),
                last_delivery: st.last_delivery_ms.map(|ms| crate::model::DeliveryBeat { ms, ok: st.last_delivery_ok.unwrap_or(false), channel: st.last_delivery_channel.clone().unwrap_or_default() }),
                heartbeat_ok_ms: st.heartbeat_ok_ms,
                outbox_pending: st.outbox_pending,
                switch_watched: st.switch_watched,
            });
        }
    }
}

/// The rules, channels, notification settings or silences changed: push them to every
/// node whose agent is installed. A failure is kept and retried (`start`).
pub fn config_changed(app: &AppHandle) {
    let Some(state) = app_state(app) else { return };
    if !crate::is_live(&state) {
        // Demo agents are simulated; the push is noted as it would have run: one operation,
        // a line per node (cell3's fails in paging-drift).
        if let Some(agents) = app.try_state::<Arc<Agents>>() {
            let now = crate::history::now_ms();
            let results = agents.demo.lock().unwrap().pushed(now);
            let live = state.live.clone();
            drop(state);
            if !results.is_empty() {
                let title = format!("Push alert settings to {} node{}", results.len(), if results.len() == 1 { "" } else { "s" });
                let app = app.clone();
                tauri::async_runtime::spawn(crate::live::audit::with_op_id(format!("agent-push-demo-{now}"), crate::live::audit::OpMeta { title: title.clone(), source: "ui".into() }, async move {
                    let head = live.audit.begin_op(&results.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>().join(", "), &title, "", true);
                    for (node, r) in &results {
                        let id = live.audit.record_simulated(node, &title, "sudo sh -c '…sha256sum -c… && install -m 0600 notify.json /etc/sparkbox-agent/ && systemctl restart sparkbox-agent'");
                        if let Err(e) = r {
                            live.audit.update(&id, |x| {
                                x.state = "failed".into();
                                x.code = Some(1);
                                x.error = Some(e.clone());
                            });
                        }
                    }
                    let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
                    let failed: Vec<String> = results.iter().filter_map(|(n, r)| r.as_ref().err().map(|e| format!("Failed on {n}: {e}"))).collect();
                    live.audit.end_op(&head, failed.is_empty(), Some(format!("{ok}/{} ok", results.len())), (!failed.is_empty()).then(|| failed.join("; ")));
                    emit(&app);
                }));
                return;
            }
            emit(app);
            return;
        }
        drop(state);
        emit(app);
        return;
    }
    let installed: Vec<String> = app.state::<Arc<Agents>>().statuses(false, &state.inventory.lock().unwrap().clone(), crate::history::now_ms()).into_iter().filter(|s| s.installed).map(|s| s.node).collect();
    if installed.is_empty() {
        return;
    }
    if let Err(e) = commands::push(app, installed, PushReason::Requested) {
        crate::logging::write("agent", &format!("settings push didn't start: {e}"));
    }
}

/// Polls the agents every 15 s, retries pending pushes and pushes settings that changed by
/// themselves (both with Admin mode unlocked), and merges the agents' history on connect and every
/// 10 min. Also starts the notification gate's clock.
pub fn start(app: &AppHandle) {
    crate::notify::start(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(15));
        let mut online: HashSet<String> = HashSet::new();
        let mut back: HashSet<String> = HashSet::new();
        let mut was_held = false;
        loop {
            every.tick().await;
            let Some(gate) = app.try_state::<Arc<AdminGate>>().map(|g| g.inner().clone()) else { continue };
            // Pushes, maintenance notes and acks write on the node: none in look-only mode (asked
            // again for each node, as it can come on mid-round). On the first round after it, what
            // it held back is due at once.
            let held = gate.look_only();
            let released = std::mem::replace(&mut was_held, held) && !held;
            let Some(state) = app_state(&app) else { continue };
            if !crate::is_live(&state) {
                if let (Some(snap), Some(agents)) = (state.latest.read().unwrap().clone(), app.try_state::<Arc<Agents>>()) {
                    let added = agents.demo.lock().unwrap().seed_away_once(&app, &snap);
                    if added > 0 {
                        crate::logging::write("agent", &format!("Demo agents logged {added} alert events while SparkBox was closed"));
                    }
                }
                emit(&app);
                continue;
            }
            let inv = state.inventory.lock().unwrap().clone();
            let live = state.live.clone();
            let snap = state.latest.read().unwrap().clone();
            drop(state);
            let layout = Layout::system();
            let agents = app.state::<Arc<Agents>>().inner().clone();
            for node in &inv.nodes {
                let up = snap.as_ref().and_then(|s| s.nodes.iter().find(|n| n.id == node.id)).is_some_and(|n| n.status == LinkStatus::Online);
                if !up {
                    online.remove(&node.id);
                    back.remove(&node.id);
                    continue;
                }
                let came_online = online.insert(node.id.clone());
                if came_online {
                    back.insert(node.id.clone());
                }
                let st = read_status(&live, node, &layout).await;
                let installed = st.installed;
                agents.set_live(st);
                let now = crate::history::now_ms();
                if installed && !gate.look_only() {
                    let rec = agents.record(&node.id);
                    let fingerprint = bundle_for(&app, &inv, node, false).hash();
                    if let Some(reason) = push_due_now(&mut back, &node.id, &rec, &fingerprint, gate.state().unlocked, now) {
                        let bundle = bundle_for(&app, &inv, node, true);
                        if let Err(e) = auto_push(&live, &agents, node, &layout, &bundle, &fingerprint, reason, now).await {
                            crate::logging::write("agent", &format!("settings push to {} failed: {e}", node.name));
                        }
                        agents.set_live(read_status(&live, node, &layout).await);
                    }
                    // What didn't reach this agent while it was away: the ends of maintenance
                    // windows, and acknowledgements.
                    for p in rec.pending_clears.iter().filter(|p| now.saturating_sub(p.since_ms) < MAINT_MAX_MS) {
                        if let Ok(body) = maintenance_note(&[p.node.clone()], None, "ended", "SparkBox", now, &format!("agent-maint-end-{now}"), true) {
                            let r = match request_key(false) {
                                Some(key) => deliver_request(&live, &gate, node, &layout, &format!("maintenance-{now}-{:06x}", fastrand::u32(..0xff_ffff)), &signed_request(&key, "maintenance", &body), Duration::from_secs(4)).await,
                                None => Err("no request key".into()),
                            };
                            if r.is_ok() {
                                let done = p.node.clone();
                                agents.update(&node.id, |x| x.pending_clears.retain(|q| q.node != done));
                            }
                        }
                    }
                    if rec.pending_clears.iter().any(|p| now.saturating_sub(p.since_ms) >= MAINT_MAX_MS) {
                        // The agent has ended those windows by itself by now.
                        agents.update(&node.id, |x| x.pending_clears.retain(|q| now.saturating_sub(q.since_ms) < MAINT_MAX_MS));
                    }
                    if agents.acks_due(&node.id, came_online || released, now) {
                        send_acks(&app, std::slice::from_ref(node), "system").await;
                    }
                }
                let due = agents.last_backfill.lock().unwrap().get(&node.id).is_none_or(|t| now.saturating_sub(*t) >= BACKFILL_EVERY_MS);
                if installed && (came_online || due) {
                    agents.last_backfill.lock().unwrap().insert(node.id.clone(), now);
                    if let Err(e) = backfill_node(&app, &live, node, &layout, agents.session_start(), !came_online).await {
                        crate::logging::write("agent", &format!("backfill from {} failed: {e}", node.name));
                    }
                }
            }
            emit(&app);
        }
    });
}

/// Sends a test from a node's agent (`sparkbox_agent.py test-notify`), which holds the
/// channels' secrets; the Mac only asks.
pub async fn test_notify(app: &AppHandle, node: &str, ch: &crate::delivery::Channel) -> Result<crate::notify::commands::TestResult, String> {
    let state = app_state(app).ok_or("SparkBox is still starting.")?;
    let name = state.inventory.lock().unwrap().nodes.iter().find(|n| n.id == node).map(|n| n.name.clone()).ok_or("Unknown node.")?;
    if !crate::is_live(&state) {
        let notifier = app.state::<Arc<crate::notify::Notifier>>();
        let d = demo::record_test(&notifier, node, ch);
        return Ok(crate::notify::commands::TestResult { ok: true, message: format!("Demo — the agent on {name} is simulated, so nothing was sent."), delivery: Some(d) });
    }
    let live = state.live.clone();
    drop(state);
    let cmd = Layout::system().cli(&format!("test-notify {}", sh_quote(&ch.id)));
    let out = live.run(node, &cmd, None, Duration::from_secs(40)).await?;
    let v: Value = serde_json::from_str(out.stdout.trim().lines().last().unwrap_or("")).map_err(|_| format!("The agent on {name} didn't answer. Is it installed and running?"))?;
    let ok = v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false);
    let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let delivery = v.get("delivery").and_then(|d| serde_json::from_value::<crate::delivery::Delivery>(d.clone()).ok());
    if let Some(d) = &delivery {
        app.state::<Arc<crate::notify::Notifier>>().deliveries.record(d.clone());
    }
    Ok(crate::notify::commands::TestResult { ok, message, delivery })
}

/// What `sparkbox_agent.py test-notify --all --json` printed: a result per channel, as a
/// list `[{channel, name, ok, ms, error}]` or a map `{channel: {ok, ms, error}}`.
pub fn parse_test_all(out: &str, at_ms: u64) -> Result<TestRun, String> {
    let line = out.trim().lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or("The agent didn't report a result.")?;
    let v: Value = serde_json::from_str(line).map_err(|_| "The agent's answer wasn't readable.".to_string())?;
    let results = v.get("results").or_else(|| v.get("channels")).cloned().unwrap_or(Value::Null);
    let cell = |r: &Value| TestCell {
        ok: r.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
        error: r.get("error").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(String::from),
        ms: r.get("ms").or_else(|| r.get("latencyMs")).and_then(|x| x.as_u64()),
        name: r.get("name").and_then(|x| x.as_str()).map(String::from),
    };
    let failed_whole = v.get("ok").and_then(|o| o.as_bool()) == Some(false) && results.as_array().is_some_and(|a| a.is_empty());
    if failed_whole {
        return Err(v.get("message").and_then(|m| m.as_str()).unwrap_or("The agent didn't send any test.").to_string());
    }
    let mut map = BTreeMap::new();
    match &results {
        Value::Array(list) => {
            for r in list {
                if let Some(id) = r.get("channel").or_else(|| r.get("id")).and_then(|x| x.as_str()) {
                    map.insert(id.to_string(), cell(r));
                }
            }
        }
        Value::Object(m) => {
            for (id, r) in m {
                map.insert(id.clone(), cell(r));
            }
        }
        _ => {
            let why = v.get("message").and_then(|m| m.as_str()).unwrap_or("The agent didn't send any test.");
            return Err(why.to_string());
        }
    }
    Ok(TestRun { at_ms, results: map, paging: None })
}

/// Runs the end-to-end test on one node and records it (agents.json and Deliveries).
pub async fn test_page_node(live: &Live, agents: &Agents, notifier: Option<&crate::notify::Notifier>, node: &NodeConfig, layout: &Layout, channels: &[crate::delivery::Channel]) -> TestRun {
    let now = crate::history::now_ms();
    let run = match live.run(&node.id, &layout.cli("test-notify --all --json"), None, Duration::from_secs(60)).await {
        Ok(out) => parse_test_all(&out.stdout, now),
        Err(e) => Err(e),
    };
    let mut run = run.unwrap_or_else(|e| TestRun { at_ms: now, results: channels.iter().map(|c| (c.id.clone(), TestCell { ok: false, error: Some(e.clone()), ms: None, name: Some(c.name.clone()) })).collect(), paging: None });
    // What it proves: the channels the node's agent runs now.
    run.paging = agents.record(&node.id).paging;
    agents.update(&node.id, |r| r.last_test = Some(run.clone()));
    if let Some(n) = notifier {
        n.deliveries.record_all(test_rows(&node.id, &node.name, &run, channels, false));
    }
    run
}

/// Deliveries rows for a node's test: "Test from cell1 via Phone (ntfy): delivered in 0.8 s".
pub fn test_rows(node: &str, name: &str, run: &TestRun, channels: &[crate::delivery::Channel], demo: bool) -> Vec<crate::delivery::Delivery> {
    use crate::delivery::{Delivery, Outcome};
    run.results
        .iter()
        .map(|(id, r)| {
            let ch = channels.iter().find(|c| &c.id == id);
            let ch_name = ch.map(|c| c.name.clone()).or_else(|| r.name.clone()).unwrap_or_else(|| id.clone());
            let what = if r.ok { format!("delivered in {}", fmt_secs(r.ms.unwrap_or(0))) } else { format!("failed: {}", r.error.clone().unwrap_or_default()) };
            let outcome = if demo { Outcome::Demo } else if r.ok { Outcome::Sent } else { Outcome::Failed };
            Delivery {
                ts_ms: run.at_ms,
                channel: id.clone(),
                channel_name: ch_name.clone(),
                kind: ch.map(|c| c.kind).unwrap_or(crate::delivery::Kind::Ntfy),
                body: format!("Test from {name} via {ch_name}: {what}"),
                error: r.error.clone(),
                via: format!("agent:{node}"),
                demo,
                test: true,
                latency_ms: r.ms,
                ..Delivery::row(None, &format!("Test from {name}"), crate::health::Severity::Warn, outcome)
            }
        })
        .collect()
}
