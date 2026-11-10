//! Alert rules as data. Every threshold and duration the health engine uses lives
//! in a `Rule`; `RuleSet::defaults()` is the table SparkBox ships with, and the
//! owner's edits are saved in rules.json and pushed to the node agents, which
//! evaluate the same JSON (src-tauri/src/agent/sparkbox_agent.py).
//!
//! Rules of kind `coded` keep their logic in rules.rs and take only their
//! durations (and the expected value, where there is one) from here. `threshold`
//! and `rate` rules on a metric the engine knows are evaluated generically, so a
//! new one needs no code. A rule that watches several counters at once (RoCE on a
//! fabric interface) carries one line per counter in `counters`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Category;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    Node,
    Net,
    Endpoint,
    Container,
    Port,
    Cluster,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleKind {
    /// A value crosses a line.
    Threshold,
    /// A counter's growth over `window_s` crosses a line.
    Rate,
    /// Something happened (a kernel event, a failed unit, an exit).
    Event,
    /// Logic of its own in rules.rs; only the durations come from here.
    Coded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "abs>")]
    AbsGt,
}

impl Op {
    /// Whether `v` is past `line` in this rule's direction.
    pub fn past(self, v: f64, line: f64) -> bool {
        match self {
            Op::Gt => v > line,
            Op::Lt => v < line,
            Op::AbsGt => v.abs() > line,
        }
    }

    /// Whether `v` has reached `line` (inclusive), for "stays raised until it's back under".
    pub fn reached(self, v: f64, line: f64) -> bool {
        match self {
            Op::Gt => v >= line,
            Op::Lt => v <= line,
            Op::AbsGt => v.abs() >= line,
        }
    }
}

/// One counter's lines inside a rule that watches several ("RoCE on a fabric interface"):
/// per second, raised past `warn`, critical past `crit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterTh {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub warn: Option<f64>,
    #[serde(default)]
    pub crit: Option<f64>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// Per-node changes to one rule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Override {
    pub warn: Option<f64>,
    pub crit: Option<f64>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    /// node | disk | gpu | fabric | containers | serving (Settings › Alerts groups by it).
    pub area: String,
    pub title: String,
    /// At most 24 characters, for where a sentence doesn't fit: the Dashboard, node rows, the
    /// menu bar ("GPU throttled", "Crash loop"). Every alert carries it as `Alert.short`.
    #[serde(default)]
    pub short: String,
    pub entity: EntityKind,
    pub kind: RuleKind,
    pub metric: String,
    pub op: Op,
    #[serde(default)]
    pub warn: Option<f64>,
    #[serde(default)]
    pub crit: Option<f64>,
    /// Hysteresis: once raised it stays until the value is back past this line.
    #[serde(default)]
    pub clear: Option<f64>,
    /// How long the condition must hold before the alert fires.
    #[serde(default)]
    pub for_s: u64,
    /// A shorter wait once it's critical (memory: two readings in a row).
    #[serde(default)]
    pub crit_for_s: Option<u64>,
    /// How long it must be gone before the alert resolves.
    #[serde(default)]
    pub clear_s: u64,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub overrides: BTreeMap<String, Override>,
    /// Reads after the entity's name; `{value}` and `{threshold}` are filled in.
    #[serde(default)]
    pub message: String,
    // Additions to the C-monitor contract.
    /// The unit of warn/crit/clear and of `{value}`: "%", "°C", "ms", "s", "h", "dBm", "/s", "".
    #[serde(default)]
    pub unit: String,
    /// Rate rules: the window the growth is counted over.
    #[serde(default)]
    pub window_s: Option<u64>,
    /// False: shown in the Alerts window, never notified (and severity info).
    #[serde(default = "yes")]
    pub notify: bool,
    /// One sentence for the ⓘ popover in Settings › Alerts.
    #[serde(default)]
    pub help: String,
    /// Per-counter lines, for a rule that watches several counters (roce-iface).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counters: Vec<CounterTh>,
    // ── r1-c ──
    /// Judged on the Mac only; the node agents don't evaluate it (node-rebooted).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mac_only: bool,
}

fn yes() -> bool {
    true
}

impl Rule {
    /// Which notification switch (Settings › Notifications) covers it.
    pub fn category(&self) -> Category {
        match self.area.as_str() {
            "disk" => Category::Disk,
            "gpu" => Category::Gpu,
            "fabric" | "roce" => Category::Fabric,
            "containers" | "serving" => Category::Containers,
            _ => Category::Node,
        }
    }
}

/// One rule's settings for one node, with overrides applied and durations in ms.
#[derive(Clone, Debug, PartialEq)]
pub struct Th {
    pub enabled: bool,
    pub op: Op,
    pub warn: Option<f64>,
    pub crit: Option<f64>,
    pub clear: Option<f64>,
    pub for_ms: u64,
    pub crit_for_ms: u64,
    pub clear_ms: u64,
    pub window_ms: u64,
    pub notify: bool,
}

impl Th {
    const OFF: Th = Th { enabled: false, op: Op::Gt, warn: None, crit: None, clear: None, for_ms: 0, crit_for_ms: 0, clear_ms: 0, window_ms: 0, notify: false };
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSet {
    #[serde(default = "version")]
    pub version: u32,
    pub rules: Vec<Rule>,
}

fn version() -> u32 {
    1
}

impl Default for RuleSet {
    fn default() -> Self {
        RuleSet::defaults()
    }
}

impl RuleSet {
    /// The shipped table, RoCE rules included.
    pub fn defaults() -> RuleSet {
        let mut rules = default_rules();
        for r in extra_defaults() {
            if !rules.iter().any(|x| x.id == r.id) {
                rules.push(r);
            }
        }
        RuleSet { version: 1, rules }
    }

    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// The settings `id` has on `node` (None: cluster-wide). An unknown rule is off.
    pub fn th(&self, id: &str, node: Option<&str>) -> Th {
        let Some(r) = self.get(id) else { return Th::OFF };
        let o = node.and_then(|n| r.overrides.get(n));
        let s = |v: u64| v * 1000;
        Th {
            enabled: o.and_then(|o| o.enabled).unwrap_or(r.enabled),
            op: r.op,
            warn: o.and_then(|o| o.warn).or(r.warn),
            crit: o.and_then(|o| o.crit).or(r.crit),
            clear: r.clear,
            for_ms: s(r.for_s),
            crit_for_ms: s(r.crit_for_s.unwrap_or(r.for_s)),
            clear_ms: s(r.clear_s),
            window_ms: s(r.window_s.unwrap_or(0)),
            notify: r.notify,
        }
    }

    /// Keeps the owner's edits (thresholds, durations, switches, overrides) on top of the
    /// current defaults, so wording and new rules from a newer SparkBox still arrive.
    /// Rules the defaults no longer have are dropped; the round-2 per-counter RoCE rules
    /// become the counter lines of "RoCE on a fabric interface" (roce_rules::migrate).
    pub fn merged_onto_defaults(saved: &RuleSet) -> RuleSet {
        let mut out = RuleSet::defaults();
        for r in &mut out.rules {
            if let Some(s) = saved.get(&r.id) {
                r.warn = s.warn;
                r.crit = s.crit;
                r.clear = s.clear;
                r.for_s = s.for_s;
                r.crit_for_s = s.crit_for_s;
                r.clear_s = s.clear_s;
                r.window_s = s.window_s.or(r.window_s);
                r.enabled = s.enabled;
                // mac-reach isn't paged unless the owner turns its paging on.
                if PAGING_OPTIONAL.contains(&r.id.as_str()) {
                    r.notify = s.notify;
                }
                // Round 4 lengthened gpu-throttle's holds (an idle GPU at its power cap flips
                // every reading): a rules.json saved with the old shipped 30/15 s takes the new ones.
                if r.id == "gpu-throttle" && (s.for_s, s.clear_s) == (30, 15) {
                    (r.for_s, r.clear_s) = (60, 120);
                }
                // The disk forecast's look-back grew from 1 to 4 hours and it gained a clear wait:
                // a rules.json saved with the old shipped values takes the new ones.
                if r.id == "disk-eta" {
                    if s.window_s == Some(3600) {
                        r.window_s = Some(14_400);
                    }
                    if s.clear_s == 0 {
                        r.clear_s = 300;
                    }
                }
                r.overrides = s.overrides.clone();
                // Counter lines by key: new counters keep their defaults, unknown ones are dropped.
                for c in &mut r.counters {
                    if let Some(sc) = s.counters.iter().find(|x| x.key == c.key) {
                        (c.warn, c.crit, c.enabled) = (sc.warn, sc.crit, sc.enabled);
                    }
                }
            }
            if r.id == super::roce_rules::RULE && saved.get(&r.id).is_none() {
                super::roce_rules::migrate(saved, &mut r.counters);
            }
        }
        out
    }

    /// Checks a candidate before it's saved: lines in the right order, durations sane.
    pub fn validate(&self) -> Result<(), String> {
        for r in &self.rules {
            let finite = |v: Option<f64>| v.is_none_or(f64::is_finite);
            if !finite(r.warn) || !finite(r.crit) || !finite(r.clear) {
                return Err(format!("{}: thresholds must be numbers.", r.title));
            }
            let worse = |a: f64, b: f64| match r.op {
                Op::Gt => a >= b,
                Op::Lt => a <= b,
                Op::AbsGt => a.abs() >= b.abs(),
            };
            if let (Some(w), Some(c)) = (r.warn, r.crit) {
                if !worse(c, w) {
                    return Err(format!("{}: the critical line has to be past the warning line.", r.title));
                }
            }
            for (node, o) in &r.overrides {
                if let (Some(w), Some(c)) = (o.warn.or(r.warn), o.crit.or(r.crit)) {
                    if !worse(c, w) {
                        return Err(format!("{} on {node}: the critical line has to be past the warning line.", r.title));
                    }
                }
            }
            for c in &r.counters {
                if !finite(c.warn) || !finite(c.crit) {
                    return Err(format!("{}, {}: thresholds must be numbers.", r.title, c.label));
                }
                if let (Some(w), Some(cr)) = (c.warn, c.crit) {
                    if cr < w {
                        return Err(format!("{}, {}: the critical line has to be past the warning line.", r.title, c.label));
                    }
                }
                if c.warn.is_some_and(|w| w < 0.0) || c.crit.is_some_and(|w| w < 0.0) {
                    return Err(format!("{}, {}: a rate can't be below 0.", r.title, c.label));
                }
            }
            if r.short.chars().count() > SHORT_MAX {
                return Err(format!("{}: its short label is over {SHORT_MAX} characters.", r.title));
            }
            const WEEK: u64 = 7 * 86_400;
            if r.for_s > WEEK || r.clear_s > WEEK || r.crit_for_s.is_some_and(|v| v > WEEK) || r.window_s.is_some_and(|v| v > WEEK) {
                return Err(format!("{}: durations can be a week at most.", r.title));
            }
        }
        Ok(())
    }

    /// Loads rules.json over the defaults. A file that can't be read is set aside
    /// (`rules.json.corrupt-<ms>`) and the defaults apply; the warning says so.
    pub fn load(path: &Path) -> RuleSet {
        match crate::util::load_json_or_quarantine::<RuleSet>(path) {
            Ok(Some(saved)) => RuleSet::merged_onto_defaults(&saved),
            _ => RuleSet::defaults(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        crate::util::write_atomic(path, &bytes)
    }
}

/// Rules other modules contribute: the RoCE and fabric rules from health::roce_rules.
pub fn extra_defaults() -> Vec<Rule> {
    let mut out = super::roce_rules::default_rules();
    out.push(super::boots::default_rule());
    out.extend(super::rules::update_rules());
    out
}

/// A rule in the defaults table.
#[allow(clippy::too_many_arguments)]
fn rule(id: &str, area: &str, title: &str, entity: EntityKind, kind: RuleKind, metric: &str, op: Op, unit: &str) -> Rule {
    Rule {
        id: id.into(),
        area: area.into(),
        title: title.into(),
        short: String::new(),
        entity,
        kind,
        metric: metric.into(),
        op,
        warn: None,
        crit: None,
        clear: None,
        for_s: 0,
        crit_for_s: None,
        clear_s: 0,
        enabled: true,
        overrides: BTreeMap::new(),
        message: String::new(),
        unit: unit.into(),
        window_s: None,
        notify: true,
        help: String::new(),
        counters: Vec::new(),
        mac_only: false,
    }
}

impl Rule {
    fn lines(mut self, warn: Option<f64>, crit: Option<f64>, clear: Option<f64>) -> Rule {
        (self.warn, self.crit, self.clear) = (warn, crit, clear);
        self
    }
    fn wait(mut self, for_s: u64, clear_s: u64) -> Rule {
        (self.for_s, self.clear_s) = (for_s, clear_s);
        self
    }
    fn crit_after(mut self, s: u64) -> Rule {
        self.crit_for_s = Some(s);
        self
    }
    fn window(mut self, s: u64) -> Rule {
        self.window_s = Some(s);
        self
    }
    fn says(mut self, message: &str, help: &str) -> Rule {
        self.message = message.into();
        self.help = help.into();
        self
    }
    fn quiet(mut self) -> Rule {
        self.notify = false;
        self
    }
    fn short(mut self, short: &str) -> Rule {
        self.short = short.into();
        self
    }
    /// About this Mac's own view of the cluster: the node agents can't judge it.
    fn seen_from_mac(mut self) -> Rule {
        self.mac_only = true;
        self
    }
}

/// The longest `Rule.short` may be.
pub const SHORT_MAX: usize = 24;

/// Rules shipped not paged whose paging the owner may turn on (rules.json keeps `notify`).
pub const PAGING_OPTIONAL: [&str; 1] = ["mac-reach"];

/// The default unified-memory warning line (models/plan.rs warns before a load crosses it).
pub const MEM_WARN_PCT: f64 = 90.0;

/// The shipped thresholds. These are the numbers the README lists.
pub fn default_rules() -> Vec<Rule> {
    use EntityKind::*;
    use Op::*;
    use RuleKind::*;
    vec![
        // ---- Nodes
        rule("node-offline", "node", "Node offline", Node, Coded, "status", Gt, "")
            .short("Offline")
            .wait(30, 0)
            .says("Offline", "Offline or in error this long before it's an alert; a quick reconnect isn't one."),
        rule("node-reboot", "node", "Not back after a restart", Node, Coded, "status", Gt, "")
            .short("Not back after restart")
            .wait(120, 0)
            .says(
                "Didn't come back after the restart SparkBox started",
                "While SparkBox restarts or shuts down a node its alerts are Planned and page nobody. A restart that runs this long past its expected time is critical.",
            ),
        rule("node-stale", "node", "Not updating", Node, Coded, "frameAge", Gt, "")
            .short("Not updating")
            .wait(5, 0)
            .says("No new readings", "Online, but no fresh numbers for this long."),
        rule("mac-reach", "node", "This Mac can't reach the cluster", Cluster, Coded, "macReach", Gt, "")
            .short("Mac can't reach cluster")
            .wait(0, 10)
            .says(
                "Can't reach the cluster",
                "This Mac lost its network, its router or VPN has a problem, or no node answers over the LAN. One warning instead of a critical per node: nodes it can't reach aren't confirmed down.",
            )
            .quiet()
            .seen_from_mac(),
        rule("node-needs-you", "node", "Node needs you", Node, Coded, "status", Gt, "")
            .short("Needs you")
            .says("Needs you", "The node answers but SparkBox can't log in: its host key isn't trusted yet, or it refused the saved password. It isn't an outage.")
            .quiet()
            .seen_from_mac(),
        rule("mem-high", "node", "Unified memory", Node, Threshold, "memPct", Gt, "%")
            .short("Memory high")
            .lines(Some(MEM_WARN_PCT), Some(95.0), Some(88.0))
            .wait(60, 0)
            .crit_after(2)
            .says("Unified memory {value} used", "Memory used outside a known GPU reservation, as a share of total memory. The reservation is excluded only while Available memory is enabled and reported. Otherwise this uses total usage."),
        rule("mem-available", "node", "Available memory", Node, Threshold, "memAvailableGiB", Lt, "GiB")
            .short("Memory low")
            .lines(Some(4.0), Some(2.0), Some(5.0))
            .wait(60, 30)
            .says("Only {value} available", "Memory Linux can give to programs, outside the engine's reserved pool. A reservation is never added back to this reading. Unload a model or stop other work if it stays low."),
        rule("mem-swap-in", "node", "Swap-in activity", Node, Threshold, "swapInMiBs", Gt, "MiB/s")
            .short("Reading swap")
            .lines(Some(1.0), None, Some(0.1))
            .wait(60, 30)
            .says("Reading swap at {value}", "Sustained reads from swap can slow model requests even when the reserved pool is expected. Existing swap occupancy alone does not fire this rule."),
        rule("mem-pressure", "node", "Memory pressure", Node, Threshold, "memPsiFull", Gt, "%")
            .short("Memory pressure")
            .lines(Some(5.0), Some(20.0), Some(3.0))
            .wait(30, 30)
            .says(
                "Every task waited on memory {value} of the last 10 s",
                "The kernel's pressure stall information (/proc/pressure/memory, full avg10): the share of time all running tasks were stalled on memory. It rises before the out-of-memory killer acts.",
            ),
        rule("ntp-offset", "node", "Clock offset", Node, Threshold, "clockOffsetMs", AbsGt, "ms")
            .short("Clock offset")
            .lines(Some(100.0), Some(1000.0), None)
            .wait(60, 60)
            .says("Clock {value} off NTP time", "NCCL timeouts and log timestamps go wrong when the clocks drift apart."),
        rule("ntp-unsynced", "node", "Clock not synchronized", Node, Coded, "ntpSynced", Lt, "")
            .short("Clock not synced")
            .wait(600, 60)
            .says("Clock not synchronized with NTP", "timedatectl says NTP isn't synchronized."),
        rule("kernel-oom", "node", "Out-of-memory kill", Node, Event, "oom", Gt, "")
            .short("OOM kill")
            .window(1800)
            .says("The kernel killed {process} for memory", "Critical when it kills a model server (vLLM, Ray, Ollama)."),
        rule("kernel-hung", "node", "Hung task", Node, Event, "hung", Gt, "")
            .short("Hung task")
            .window(1800)
            .says("{process} blocked for more than 2 minutes", "The kernel reported a task stuck in uninterruptible sleep."),
        rule("agent-silent", "node", "Agent stopped reporting", Node, Coded, "agentBeat", Gt, "")
            .short("Agent silent")
            .wait(60, 30)
            .says("The always-on agent stopped reporting", "The node answers SparkBox but its agent's heartbeat is older than a minute."),
        rule("agent-config-stale", "node", "Agent settings out of date", Node, Coded, "agentConfig", Gt, "")
            .short("Agent settings stale")
            .wait(60, 0)
            .says(
                "Alert settings didn't reach the agent",
                "The agent on the node reports a different settings hash than this Mac pushed, so it would page with old rules, channels or silences. Push again from Settings › Notifications.",
            ),
        // ---- Disk
        rule("disk-full", "disk", "Root disk full", Node, Threshold, "diskPct", Gt, "%")
            .short("Disk full")
            .lines(Some(85.0), Some(95.0), Some(83.0))
            .says("Root disk {value} full", "Judged on the whole percent the app shows."),
        rule("disk-eta", "disk", "Root disk filling", Node, Rate, "diskFullInH", Lt, "h")
            .short("Disk filling")
            .lines(Some(24.0), None, Some(30.0))
            .window(14_400)
            .wait(0, 300)
            .says(
                "Root disk full in {value} if it keeps filling at this rate",
                "Needs a rate the disk held and is still holding, watched for at least a sixth of the time it predicts, so a 24-hour estimate takes 4 hours to appear. Under an hour, the disk must also have lost a quarter of its free space. A download or copy that's over in an hour says nothing on a disk with room to spare.",
            ),
        // ---- GPU
        rule("gpu-hot", "gpu", "GPU temperature", Node, Threshold, "gpuTemp", Gt, "°C")
            .short("GPU hot")
            .lines(Some(85.0), None, Some(82.0))
            .wait(120, 30)
            .says("GPU at {value}", ""),
        rule("gpu-throttle", "gpu", "GPU throttling", Node, Coded, "throttle", Gt, "")
            .short("GPU throttled")
            .wait(60, 120)
            .says(
                "GPU throttled",
                "Hardware slowdowns (HW slowdown, HW thermal, HW power brake) always count. SW power cap, SW thermal and sync boost count only while the GPU is busy: 50% or more and below 90% of its top clock, or at 80% of a reported power limit. GPU idle and application clocks never count.",
            ),
        rule("kernel-xid", "gpu", "GPU Xid error", Node, Event, "xid", Gt, "")
            .short("GPU Xid error")
            .window(3600)
            .says(
                "Xid {code}: {meaning}",
                "Fatal Xids (48, 79, 94, 95, 119, 120, 154) stay open until the node restarts, however long ago they happened; other Xids clear an hour after the last one.",
            ),
        rule("gpu-unreadable", "gpu", "GPU not answering", Node, Coded, "gpuReadable", Gt, "")
            .short("GPU not answering")
            .wait(30, 0)
            .says("GPU readings unavailable", "nvidia-smi or NVML stopped answering. The node's models are Down until the GPU is back."),
        // ---- Fabric
        rule("fabric-down", "fabric", "Fabric link down", Net, Coded, "up", Lt, "")
            .short("Fabric link down")
            .wait(3, 30)
            .says("Fabric link down", "Only ports with an address count, so an uncabled half stays quiet."),
        rule("fabric-flap", "fabric", "Link flapping", Net, Rate, "carrierChanges", Gt, "")
            .short("Link flapping")
            .lines(Some(0.0), None, None)
            .window(300)
            .says("Flapped {value} times", ""),
        rule("net-errors", "fabric", "Interface errors", Net, Rate, "ifErrors", Gt, "")
            .short("Interface errors")
            .lines(Some(0.0), None, None)
            .window(120)
            .says("Errors rising (+{value})", ""),
        rule("fabric-mtu", "fabric", "Fabric MTU", Net, Coded, "mtu", Lt, "")
            .short("MTU mismatch")
            .lines(Some(9000.0), None, None)
            .wait(30, 30)
            .says("MTU {value}, expected {threshold}", "Warns when a fabric port's MTU isn't the expected one, or differs from its peers."),
        rule("kernel-mlx5", "fabric", "ConnectX-7 driver error", Net, Event, "mlx5", Gt, "")
            .short("ConnectX-7 error")
            .window(1800)
            .says("mlx5_core: {text}", "Module unplugged, command timeout, health compromised or link down, from the kernel log."),
        rule("switch-unreachable", "fabric", "Switch unreachable", Cluster, Coded, "status", Gt, "")
            .short("Switch unreachable")
            .wait(10, 30)
            .says("Unreachable", ""),
        rule("switch-health", "fabric", "Switch power, fans and temperature", Cluster, Coded, "sensors", Gt, "")
            .short("Switch hardware")
            .wait(30, 60)
            .says(
                "{sensor} failing",
                "From the switch's /system/health: a power supply that fails is critical; a fan under 500 RPM or a sensor over 85 °C warns.",
            ),
        rule("port-down", "fabric", "Switch port down", Port, Coded, "running", Lt, "")
            .short("Port down")
            .wait(3, 30)
            .says("No link", "Ports cabled to a node only."),
        rule("port-errors", "fabric", "Switch port errors", Port, Rate, "portErrors", Gt, "")
            .short("Port errors")
            .lines(Some(0.0), None, None)
            .window(120)
            .says("Errors rising (+{value})", "RX, TX and FCS errors together."),
        rule("optic-rx-low", "fabric", "Optic receive power", Port, Threshold, "rxPowerDbm", Lt, "dBm")
            .short("Optic weak")
            .lines(Some(-7.0), Some(-10.0), Some(-6.5))
            .wait(5, 15)
            .says("Receive power {value}", "The weakest lane counts."),
        // ---- Containers
        rule("crash-loop", "containers", "Crash loop", Container, Rate, "restarts", Gt, "")
            .short("Crash loop")
            .lines(Some(0.0), Some(f64::from(super::rules::CRASH_RESTARTS - 1)), None)
            .window(super::rules::CRASH_WINDOW_S)
            .wait(0, 300)
            .says(
                "Docker restarted it {value} times",
                "Critical at two restarts in 10 minutes, and it stays critical through the seconds it runs between restarts: it eases only after 5 minutes under the line. It ends once the container has run a minute without restarting, or its model answers again, and clears 5 minutes after that.",
            ),
        rule("container-exited", "containers", "Container exited with an error", Container, Event, "exitCode", Gt, "")
            .short("Exited with an error")
            .window(86_400)
            .says("Exited {value}", "Critical when it was OOM-killed. Exits older than the window are history, not incidents."),
        rule("unit-failed", "containers", "systemd unit failed", Node, Event, "failedUnits", Gt, "")
            .short("Unit failed")
            .says("Failed", ""),
        // ---- Serving
        rule("endpoint-down", "serving", "Model down", Endpoint, Coded, "health", Gt, "")
            .short("Model down")
            .wait(30, 0)
            .says(
                "Down",
                "From the endpoint's health: every stage online, its containers running, and an answer to a probe. Clears the moment it answers again.",
            ),
        rule("endpoint-ttft", "serving", "Time to first token", Endpoint, Threshold, "ttftP95", Gt, "ms")
            .short("Slow first token")
            .lines(Some(2000.0), None, None)
            .wait(300, 60)
            .says("TTFT p95 {value}", "95th percentile time to first token, from the engine's own metrics."),
        rule("endpoint-waiting", "serving", "Requests waiting", Endpoint, Threshold, "waiting", Gt, "")
            .short("Requests waiting")
            .lines(Some(0.0), None, None)
            .wait(60, 30)
            .says("{value} requests waiting", "Requests queued because the engine is full."),
        rule("endpoint-kv", "serving", "KV cache", Endpoint, Threshold, "kvPct", Gt, "%")
            .short("KV cache full")
            .lines(Some(90.0), None, Some(85.0))
            .wait(120, 60)
            .says("KV cache {value} full", ""),
        rule("endpoint-preempt", "serving", "Preemptions", Endpoint, Rate, "preemptions", Gt, "")
            .short("Preemptions")
            .lines(Some(0.0), None, None)
            .window(300)
            .says("{value} requests preempted in 5 min", "The engine evicted running requests to make room."),
        rule("endpoint-corrupted", "serving", "Corrupted requests", Endpoint, Rate, "corrupted", Gt, "")
            .short("Corrupted requests")
            .lines(Some(0.0), None, None)
            .window(300)
            .says(
                "{value} corrupted requests in 5 min",
                "Requests that came back with NaNs in their logits, from the engine's own counter. vLLM only keeps that counter when it runs with VLLM_COMPUTE_NANS_IN_LOGITS.",
            ),
        rule("endpoint-idle", "serving", "Loaded but idle", Endpoint, Coded, "genTps", Lt, "h")
            .short("Loaded but idle")
            .lines(Some(24.0), None, None)
            .says("Loaded but idle for {value}", "Information only, never notified: the model holds memory nobody is using.")
            .quiet(),
    ]
}
