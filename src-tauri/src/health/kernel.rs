//! What kernel events mean. The collector and the agent follow the kernel log
//! and send GPU Xid errors, mlx5_core (ConnectX-7) lines, OOM kills and hung
//! tasks (NodeSnapshot.kernel_events); this decodes them for alerts and the
//! Alerts › Events tab. The Xid table is xid.json, which collector.py carries a
//! copy of (collector_test.py checks the two agree).
//!
//! Most mlx5_core lines are links changing state, not faults: a cable unplugged or plugged,
//! a link or RoCE port going down or up. Recabling the fabric logs dozens of them (the real
//! cluster logged 58 on Oct 25–26, fixtures/kernel-mlx5.jsonl). `stamp` makes those kind
//! `link`, severity `info`; `rows` groups the ones within 15 minutes of each other across
//! nodes into one Events row. They never raise an alert. Only real driver faults (a command
//! timeout, "health compromised", a firmware fatal or syndrome, an mlx5 ERR, a page
//! allocation failure) stay kind `mlx5` and are "ConnectX-7 driver error" warnings.
//!
//! A boot logs them too: the uncabled ConnectX-7 halves report "cable unplugged" and the links
//! come up while the fabric starts. `with_boots` adds a row per boot (boots.rs) and puts the
//! link and information events of its first 2 minutes under it ("cell3 booted · 8 link events
//! while the fabric came up"), so a restart never reads as recabling.

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::model::{ClusterSnapshot, KernelEvent};

/// Link events closer than this to the one before are the same recabling.
pub const LINK_GROUP_GAP_MS: u64 = 15 * 60_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XidInfo {
    pub code: u32,
    /// warn | crit
    pub severity: String,
    pub name: String,
    /// Reads after "Xid 79: ".
    pub meaning: String,
    /// What the operator does next.
    pub action: String,
    /// Fatal: the GPU needs a reset, so the alert stays until the node boots again.
    #[serde(default)]
    pub latch: bool,
}

const XID_JSON: &str = include_str!("xid.json");

pub fn xid_table() -> &'static HashMap<u32, XidInfo> {
    static TABLE: OnceLock<HashMap<u32, XidInfo>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let list: Vec<XidInfo> = serde_json::from_str(XID_JSON).unwrap_or_default();
        list.into_iter().map(|x| (x.code, x)).collect()
    })
}

pub fn xid(code: u32) -> Option<&'static XidInfo> {
    xid_table().get(&code)
}

/// A kernel event with its meaning and the next step, for the Events tab.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedEvent {
    pub node: String,
    pub ts_ms: u64,
    pub kind: String,
    pub code: Option<u32>,
    pub text: String,
    pub severity: String,
    pub process: Option<String>,
    /// "GPU fallen off the bus", "Out-of-memory kill"…
    pub title: String,
    pub meaning: String,
    pub action: String,
    // ── r4-alerts ──
    /// The recabling it's part of (link events within 15 minutes of each other, any node).
    pub group: Option<String>,
    /// A group's row: the events in it, newest first; empty for a single event.
    pub events: Vec<DecodedEvent>,
    /// A group's row: the nodes it touched.
    pub nodes: Vec<String>,
    /// A group's row: its last event (`ts_ms` is its first).
    pub to_ms: Option<u64>,
}

pub fn decode(node: &str, e: &KernelEvent) -> DecodedEvent {
    let process = e.process.clone();
    let who = process.clone().unwrap_or_else(|| "a process".into());
    let (title, meaning, action) = match (e.kind.as_str(), e.code.and_then(xid)) {
        ("xid", Some(x)) => (format!("Xid {} · {}", x.code, x.name), x.meaning.clone(), x.action.clone()),
        ("xid", None) => (
            format!("Xid {}", e.code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())),
            "an Xid this table doesn't know".into(),
            "Look it up in NVIDIA's Xid catalog; restart the node if the GPU stops responding.".into(),
        ),
        ("oom", _) => (
            "Out-of-memory kill".into(),
            format!("the kernel killed {who} to free memory"),
            if e.severity == "crit" {
                "A model server was killed: lower its memory use (smaller context or batch, gpu-memory-utilization) or unload another model, then restart it.".into()
            } else {
                "Find what used the memory (Workloads, sorted by memory) and stop or limit it.".into()
            },
        ),
        ("hung", _) => (
            "Hung task".into(),
            format!("{who} was stuck in uninterruptible sleep for over 2 minutes"),
            "Usually storage or a driver waiting on hardware: check the disks and the kernel log around this time.".into(),
        ),
        ("link", _) => (
            link_text(&e.text).unwrap_or_else(|| "ConnectX-7 link event".into()),
            "a fabric link or cable changed state (recabling, or the other end restarting)".into(),
            "Nothing to do if you were recabling or restarting. A link that stays down shows in Fabric.".into(),
        ),
        ("mlx5", _) if e.severity == "info" => (
            "ConnectX-7 event".into(),
            "the mlx5_core driver logged something that isn't a fault".into(),
            String::new(),
        ),
        ("mlx5", _) => (
            "ConnectX-7 driver error".into(),
            "the mlx5_core driver reported a problem with the fabric adapter".into(),
            "Check the cable and transceiver on that port (Fabric), then restart the node if the port stays down.".into(),
        ),
        _ => ("Kernel event".into(), e.text.clone(), String::new()),
    };
    DecodedEvent {
        node: node.into(),
        ts_ms: e.ts_ms,
        kind: e.kind.clone(),
        code: e.code,
        text: e.text.clone(),
        severity: e.severity.clone(),
        process,
        title,
        meaning,
        action,
        group: None,
        events: Vec::new(),
        nodes: Vec::new(),
        to_ms: None,
    }
}

/// What an mlx5_core line is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mlx5 {
    /// A cable, a link or a RoCE port changing state: information.
    Link,
    /// A driver fault: a command timeout, "health compromised", a firmware fatal or syndrome,
    /// an mlx5 ERR, a page allocation failure.
    Fault,
    /// Neither (the collector forwards only the lines above; anything else is logged quietly).
    Other,
}

pub fn classify_mlx5(text: &str) -> Mlx5 {
    let t = text.to_ascii_lowercase();
    let fault = ["timeout", "no done completion", "health compromised", "fatal", "syndrome", "page allocation failure", "assert"].iter().any(|w| t.contains(w))
        || text.contains(" ERR") || text.contains("ERR:") || t.contains("mlx5_cmd_out_err");
    if fault {
        return Mlx5::Fault;
    }
    if link_text(text).is_some() {
        Mlx5::Link
    } else {
        Mlx5::Other
    }
}

/// "ConnectX-7 cable unplugged (0000:01:00.0)", "Link down on enp1s0f1np1", "RoCE port down on
/// rocep1s0f1": a link event in a few words, or None when the line isn't one.
pub fn link_text(text: &str) -> Option<String> {
    let t = text.to_ascii_lowercase();
    // "mlx5_core 0000:01:00.0: …" or "mlx5_core 0000:01:00.1 enp1s0f1np1: …"
    let mut words = text.split_once("mlx5_core").map(|(_, r)| r).unwrap_or("").split_whitespace();
    let pci = words.next().map(|w| w.trim_end_matches(':').to_string()).filter(|w| w.contains(':'));
    let iface = words.next().filter(|w| w.ends_with(':')).map(|w| w.trim_end_matches(':').to_string());
    let at_pci = |what: &str| match &pci {
        Some(p) => format!("ConnectX-7 {what} ({p})"),
        None => format!("ConnectX-7 {what}"),
    };
    if t.contains("cable unplugged") || t.contains("module unplugged") || t.contains("module absent") || t.contains("module is absent") {
        return Some(at_pci("cable unplugged"));
    }
    if t.contains("cable plugged") || t.contains("module plugged") {
        return Some(at_pci("cable plugged in"));
    }
    let on = |what: &str| match &iface {
        Some(i) => format!("{what} on {i}"),
        None => what.to_string(),
    };
    if t.contains("port: ") && (t.ends_with("link down") || t.ends_with("link active") || t.ends_with("link up")) {
        let up = !t.ends_with("link down");
        return Some(on(if up { "RoCE port up" } else { "RoCE port down" }));
    }
    if t.trim_end().ends_with("link down") {
        return Some(on("Link down"));
    }
    if t.trim_end().ends_with("link up") || t.contains("link up,") {
        return Some(on("Link up"));
    }
    None
}

/// Before the rules run and the UI sees a snapshot: mlx5 link events become kind `link`,
/// severity `info`. Faults keep kind `mlx5` and their severity.
pub fn stamp(snap: &mut ClusterSnapshot) {
    for e in snap.nodes.iter_mut().flat_map(|n| n.kernel_events.iter_mut()).filter(|e| e.kind == "mlx5") {
        match classify_mlx5(&e.text) {
            Mlx5::Link => {
                e.kind = "link".into();
                e.severity = "info".into();
            }
            Mlx5::Other => e.severity = "info".into(),
            Mlx5::Fault => {}
        }
    }
}

/// Link events within 15 minutes of the one before (on any node) share a group,
/// "link-<first ms>": one recabling.
pub fn group_links(events: &mut [DecodedEvent]) {
    let mut order: Vec<usize> = (0..events.len()).filter(|i| events[*i].kind == "link").collect();
    order.sort_by_key(|i| events[*i].ts_ms);
    let (mut group, mut last) = (String::new(), 0u64);
    for i in order {
        let ts = events[i].ts_ms;
        if group.is_empty() || ts.saturating_sub(last) > LINK_GROUP_GAP_MS {
            group = format!("link-{ts}");
        }
        last = ts;
        events[i].group = Some(group.clone());
    }
}

/// The Events tab's rows: every event, with each group of two or more link events collapsed
/// into one row ("58 link events on 4 nodes (cables unplugged and links down)") that holds
/// them in `events`. Newest first.
pub fn rows(mut events: Vec<DecodedEvent>, names: &HashMap<String, String>) -> Vec<DecodedEvent> {
    group_links(&mut events);
    events.sort_by_key(|e| std::cmp::Reverse(e.ts_ms));
    let mut groups: Vec<(String, Vec<DecodedEvent>)> = Vec::new();
    let mut out: Vec<DecodedEvent> = Vec::new();
    for e in events {
        match e.group.clone().filter(|_| e.kind == "link") {
            Some(g) => match groups.iter_mut().find(|(id, _)| *id == g) {
                Some((_, list)) => list.push(e),
                None => groups.push((g, vec![e])),
            },
            None => out.push(e),
        }
    }
    for (id, list) in groups {
        if list.len() == 1 {
            out.extend(list);
            continue;
        }
        let nodes: BTreeSet<&str> = list.iter().map(|e| e.node.as_str()).collect();
        let lower: Vec<String> = list.iter().map(|e| e.title.to_ascii_lowercase()).collect();
        let mut what = Vec::new();
        if lower.iter().any(|t| t.contains("unplugged")) {
            what.push("cables unplugged");
        }
        if lower.iter().any(|t| t.contains("plugged in")) {
            what.push("cables plugged in");
        }
        if lower.iter().any(|t| t.contains(" down")) {
            what.push("links down");
        }
        if lower.iter().any(|t| t.contains(" up")) {
            what.push("links up");
        }
        let what = match what.len() {
            0 => String::new(),
            1 => format!(" ({})", what[0]),
            _ => format!(" ({} and {})", what[..what.len() - 1].join(", "), what[what.len() - 1]),
        };
        let count = list.len();
        let node_word = if nodes.len() == 1 { names.get(*nodes.first().unwrap()).cloned().unwrap_or_else(|| nodes.first().unwrap().to_string()) } else { format!("{} nodes", nodes.len()) };
        let first = list.iter().map(|e| e.ts_ms).min().unwrap_or(0);
        let latest = list.iter().map(|e| e.ts_ms).max().unwrap_or(0);
        out.push(DecodedEvent {
            node: if nodes.len() == 1 { nodes.first().unwrap().to_string() } else { String::new() },
            ts_ms: first,
            kind: "link".into(),
            code: None,
            text: format!("{count} link events on {node_word}{what}"),
            severity: "info".into(),
            process: None,
            title: format!("{count} link events on {node_word}{what}"),
            meaning: "fabric cables or links changed state together: recabling, or nodes restarting".into(),
            action: "Nothing to do if you were recabling or restarting. A link that stays down shows in Fabric.".into(),
            group: Some(id),
            nodes: nodes.iter().map(|n| n.to_string()).collect(),
            to_ms: Some(latest),
            events: list,
        });
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.to_ms.unwrap_or(e.ts_ms)));
    out
}

/// Events this soon after a boot started are the boot's: ConnectX-7 halves without a cable
/// report "cable unplugged" and links come up while the fabric starts.
pub const BOOT_GROUP_MS: u64 = 120_000;
/// A node's clock and the uptime the boot start comes from can differ by a moment.
const BOOT_SLACK_MS: u64 = 5_000;

/// A boot, as boots::marks words it for the Events tab.
#[derive(Clone, Debug, PartialEq)]
pub struct BootMark {
    pub node: String,
    pub start_ms: u64,
    /// warn when the boot before ended without a shutdown, else info.
    pub severity: String,
    /// "the boot before ended without a shutdown (power loss, hang or watchdog)"
    pub meaning: String,
    pub action: String,
    /// How SparkBox knows of it, and the last lines of the boot before.
    pub text: String,
}

/// Whether an event is the boot's own: a link change or information, never a fault (an Xid, an
/// OOM kill or a driver error keeps its own row).
fn boot_noise(e: &DecodedEvent) -> bool {
    e.kind == "link" || e.severity == "info"
}

/// The Events tab's rows with boot rows (kind "boot") added: each boot holds the link and
/// information events of its node in the first 2 minutes after it started ("cell3 booted · 8 link
/// events while the fabric came up"), so they never read as recabling. The rest go through
/// `rows` as before. Newest first.
pub fn with_boots(events: Vec<DecodedEvent>, boots: &[BootMark], names: &HashMap<String, String>) -> Vec<DecodedEvent> {
    let mut held: Vec<Vec<DecodedEvent>> = vec![Vec::new(); boots.len()];
    let mut rest = Vec::new();
    for e in events {
        let at = boots.iter().position(|b| b.node == e.node && boot_noise(&e) && e.ts_ms + BOOT_SLACK_MS >= b.start_ms && e.ts_ms <= b.start_ms + BOOT_GROUP_MS);
        match at {
            Some(i) => held[i].push(e),
            None => rest.push(e),
        }
    }
    let mut out = rows(rest, names);
    for (b, mut list) in boots.iter().zip(held) {
        list.sort_by_key(|e| std::cmp::Reverse(e.ts_ms));
        let name = names.get(&b.node).cloned().unwrap_or_else(|| b.node.clone());
        let links = list.iter().filter(|e| e.kind == "link").count();
        let other = list.len() - links;
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let what = match (links, other) {
            (0, 0) => String::new(),
            (l, 0) => format!(" · {} while the fabric came up", plural(l, "link event", "link events")),
            (0, o) => format!(" · {} while it came up", plural(o, "kernel event", "kernel events")),
            (l, o) => format!(" · {} while the fabric came up, {}", plural(l, "link event", "link events"), plural(o, "other kernel event", "other kernel events")),
        };
        let to_ms = list.iter().map(|e| e.ts_ms).max();
        let title = format!("{name} booted{what}");
        out.push(DecodedEvent {
            node: b.node.clone(),
            ts_ms: b.start_ms,
            kind: "boot".into(),
            code: None,
            text: b.text.clone(),
            severity: b.severity.clone(),
            process: None,
            title,
            meaning: b.meaning.clone(),
            action: b.action.clone(),
            group: Some(format!("boot-{}-{}", b.node, b.start_ms)),
            nodes: vec![b.node.clone()],
            to_ms: to_ms.filter(|_| !list.is_empty()),
            events: list,
        });
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.to_ms.unwrap_or(e.ts_ms)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_xid_decodes_with_a_next_step() {
        let codes = [13, 31, 43, 45, 48, 61, 62, 63, 64, 68, 69, 74, 79, 92, 94, 95, 109, 119, 120, 154];
        for c in codes {
            let x = xid(c).unwrap_or_else(|| panic!("Xid {c} is missing"));
            assert!(!x.meaning.is_empty() && !x.action.is_empty(), "{c}");
        }
        for fatal in [48, 79, 94, 95, 119, 120, 154] {
            assert_eq!(xid(fatal).unwrap().severity, "crit", "{fatal}");
            assert!(xid(fatal).unwrap().latch, "Xid {fatal} latches until the node restarts");
        }
        let latched: Vec<u32> = xid_table().values().filter(|x| x.latch).map(|x| x.code).collect();
        assert_eq!(latched.len(), 7, "only the fatal ones latch: {latched:?}");
        assert_eq!(xid(13).unwrap().severity, "warn");
        let e = KernelEvent { ts_ms: 1, kind: "xid".into(), code: Some(79), text: "NVRM: Xid (PCI:000f:01:00): 79, GPU has fallen off the bus.".into(), severity: "crit".into(), process: None };
        let d = decode("gx10-0000", &e);
        assert_eq!(d.title, "Xid 79 · GPU fallen off the bus");
        assert!(d.action.starts_with("Restart the node"));
    }

    #[test]
    fn link_events_within_2_minutes_of_a_boot_are_its_own() {
        // cell3's boot at 18:08: the uncabled halves report "cable unplugged" and the links come
        // up; a link going down 20 minutes later is recabling, and an Xid at boot stays its own row.
        let start = 1_793_311_710_000; // Oct 29 18:08:30 EDT
        let link = |dt_s: u64, text: &str| KernelEvent { ts_ms: start + dt_s * 1000, kind: "link".into(), code: None, text: text.into(), severity: "info".into(), process: None };
        let mut events = Vec::new();
        for (i, t) in ["mlx5_core 0000:01:00.0: Port module event: module 0, Cage 0, Cable unplugged", "mlx5_core 0002:01:00.0: Port module event: module 0, Cage 0, Cable unplugged"].iter().enumerate() {
            events.push(decode("cell3", &link(21 + i as u64, t)));
        }
        for i in 0..6u64 {
            events.push(decode("cell3", &link(30 + i * 5, &format!("mlx5_core 0000:01:00.{} enp1s0f{}np{}: Link up", i % 2, i % 2, i % 2))));
        }
        events.push(decode("cell3", &link(20 * 60, "mlx5_core 0000:01:00.1 enp1s0f1np1: Link down")));
        events.push(decode("cell3", &KernelEvent { ts_ms: start + 40_000, kind: "xid".into(), code: Some(79), text: "NVRM: Xid (PCI:000f:01:00): 79".into(), severity: "crit".into(), process: None }));
        let mark = BootMark { node: "cell3".into(), start_ms: start, severity: "info".into(), meaning: "the boot before ended with a clean shutdown".into(), action: String::new(), text: "Booted".into() };
        let names = [("cell3".to_string(), "cell3".to_string())].into();
        let rows = with_boots(events, &[mark], &names);
        let boot = rows.iter().find(|r| r.kind == "boot").unwrap();
        assert_eq!(boot.title, "cell3 booted · 8 link events while the fabric came up");
        assert_eq!((boot.events.len(), boot.nodes.clone(), boot.ts_ms, boot.to_ms), (8, vec!["cell3".to_string()], start, Some(start + 55_000)));
        assert!(!rows.iter().any(|r| r.title.contains("cables unplugged")), "a boot is never recabling: {rows:?}");
        assert!(rows.iter().any(|r| r.kind == "link" && r.title == "Link down on enp1s0f1np1"), "{rows:?}");
        assert!(rows.iter().any(|r| r.kind == "xid"), "a fault at boot keeps its own row");
        // A boot with nothing logged is a plain row.
        let quiet = with_boots(Vec::new(), &[BootMark { node: "cell3".into(), start_ms: start, severity: "warn".into(), meaning: String::new(), action: String::new(), text: String::new() }], &names);
        assert_eq!((quiet[0].title.as_str(), quiet[0].events.len()), ("cell3 booted", 0));
    }
}
