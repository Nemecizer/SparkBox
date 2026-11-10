//! Planned work: a restart or shutdown SparkBox started. While a node is in a maintenance
//! window, the alerts rooted at it (offline, its fabric and switch ports, its agent, the
//! models with a stage there, its containers) are Planned: shown as "Planned · cell3
//! restarting (SparkBox, 10:42)", counted nowhere and paged to nobody.
//!
//! The power commands call `begin` (and `end` after Verify). A power action the snapshot shows
//! (`power_state`) without a window gets one by itself, so planned work is never paged even
//! if a caller forgets. A window ends with `end`, or by itself once the node is back and every
//! model with a stage on it answers (or 15 minutes after it came back). A restart that runs
//! past its expected time plus the node-reboot grace stops being planned: the node's alerts
//! count again and "didn't come back" is critical.
//!
//! An ordered restart of a model (`begin_endpoint`, models' "Then restart …") is planned work
//! too: the model's alerts and its containers' are Planned ("glm-5.3-flash restarting
//! (SparkBox, 10:42)") until it answers again, or until its expected time plus the grace.
//!
//! Open windows are saved per data source (`maintenance.live.json`, `maintenance.demo.json`
//! next to the alert log, `keep_in`), so SparkBox quitting in the middle of a restart it started
//! doesn't page the restart when it opens again.
//!
//! Maintenance mode (P1.16) is a window the owner opens on a node by hand (`begin_manual`): for
//! an hour, or until they end it. Its alerts are Planned the same way ("cell2 in maintenance
//! (you, 10:42)"), but it doesn't follow the node: it ends with `end_manual` or at its deadline,
//! whatever the node does meanwhile. A restart SparkBox starts inside it leaves it open.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::rules::{reporting, EndpointInfo};
use crate::model::{ClusterSnapshot, MaintInfo};

/// How long a reboot SparkBox started (with no expected time given) should take.
pub const REBOOT_EXPECT_MS: u64 = 8 * 60_000;
/// A window whose node has been back this long ends even if its models never answer.
pub const SETTLE_MS: u64 = 15 * 60_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Maint {
    /// When the node should be back; None until it's woken (a shutdown).
    pub until_ms: Option<u64>,
    /// "restarting" or "shut down".
    pub reason: String,
    pub started_ms: u64,
    pub demo: bool,
    /// Opened from the snapshot's power state rather than by a power command.
    pub implicit: bool,
    /// When the node was first seen reporting again.
    pub back_ms: Option<u64>,
    // ── r1-c ──
    /// The node has been seen down (or in a power action) since the window opened.
    #[serde(default)]
    pub went_down: bool,
    // ── R2-OPS ──
    /// The owner opened it (maintenance mode), not a power action.
    #[serde(default)]
    pub manual: bool,
}

/// A manual window's reason, which the UI reads it by: "cell2 in maintenance".
pub const MANUAL_REASON: &str = "in maintenance";
/// The longest a timed manual window may run (a longer one is "until I end it").
pub const MANUAL_MAX_MINUTES: u32 = 24 * 60;

// ── r1-c ──
/// A shutdown that hasn't taken its node down this long after its window opened didn't happen
/// (node_power waits at most 60 min before sending it, then gives up a minute later).
const SHUTDOWN_BY_MS: u64 = 62 * 60_000;

static WINDOWS: Mutex<BTreeMap<(bool, String), Maint>> = Mutex::new(BTreeMap::new());
/// Ordered restarts of a model, by (demo, endpoint id).
static ENDPOINTS: Mutex<BTreeMap<(bool, String), EndpointMaint>> = Mutex::new(BTreeMap::new());

/// A model SparkBox is restarting in order (its stages on `nodes`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointMaint {
    pub nodes: Vec<String>,
    /// When it should answer again; None: no deadline but the grace after it's back.
    pub until_ms: Option<u64>,
    /// "restarting", "reloading".
    pub reason: String,
    pub started_ms: u64,
}

/// Seen answering this long after the restart began, the model is back (the "ready" before the
/// restart reached the containers doesn't count).
const ENDPOINT_SETTLE_MS: u64 = 15_000;

fn now_ms() -> u64 {
    crate::history::now_ms()
}

/// Where the windows are saved (the app's data folder); unset in tests, which save nothing.
static DIR: OnceLock<PathBuf> = OnceLock::new();

/// One data source's open windows, as saved.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    #[serde(default)]
    nodes: BTreeMap<String, Maint>,
    #[serde(default)]
    endpoints: BTreeMap<String, EndpointMaint>,
}

fn saved_path(dir: &Path, demo: bool) -> PathBuf {
    dir.join(format!("maintenance.{}.json", if demo { "demo" } else { "live" }))
}

/// Saves windows in `dir` from now on, and takes back the ones saved there before (Health::new).
/// `stamp` then closes any whose node is back, or past its deadline and grace.
pub fn keep_in(dir: &Path) {
    // Tests share these windows process-wide and run at once: they save and load explicitly.
    if cfg!(test) {
        return;
    }
    load_from(dir);
    let _ = DIR.set(dir.to_path_buf());
}

fn load_from(dir: &Path) {
    for demo in [false, true] {
        let Ok(Some(saved)) = crate::util::load_json_or_quarantine::<Saved>(&saved_path(dir, demo)) else { continue };
        let mut windows = WINDOWS.lock().unwrap();
        for (node, m) in saved.nodes {
            windows.entry((demo, node)).or_insert(m);
        }
        drop(windows);
        let mut endpoints = ENDPOINTS.lock().unwrap();
        for (id, m) in saved.endpoints {
            endpoints.entry((demo, id)).or_insert(m);
        }
    }
}

/// Saves one data source's windows (when a folder was given), if they changed since the last save.
fn save(demo: bool) {
    if let Some(dir) = DIR.get() {
        save_to(dir, demo);
    }
}

fn save_to(dir: &Path, demo: bool) {
    static LAST: Mutex<[Option<String>; 2]> = Mutex::new([None, None]);
    let nodes = WINDOWS.lock().unwrap().iter().filter(|((d, _), _)| *d == demo).map(|((_, n), m)| (n.clone(), m.clone())).collect();
    let saved = Saved { nodes, endpoints: endpoint_windows(demo) };
    let text = serde_json::to_string_pretty(&saved).unwrap_or_default();
    let mut last = LAST.lock().unwrap();
    let path = saved_path(dir, demo);
    // Nothing open and nothing saved before: no file to write (at rest SparkBox writes none).
    if last[demo as usize].as_deref() == Some(text.as_str()) || (saved.nodes.is_empty() && saved.endpoints.is_empty() && !path.exists()) {
        return;
    }
    if let Err(e) = crate::util::write_atomic(&path, text.as_bytes()) {
        eprintln!("maintenance: couldn't save the windows: {e}");
        return;
    }
    last[demo as usize] = Some(text);
}

fn is_demo(app: &AppHandle) -> bool {
    app.try_state::<crate::AppState>().map(|s| !crate::is_live(&s)).unwrap_or(true)
}

/// Opens a maintenance window on `node`: alerts rooted there are Planned until `end`, or until
/// `until_ms` (None: until the node is woken) plus the node-reboot grace.
pub fn begin(app: &AppHandle, node: &str, until_ms: Option<u64>, reason: &str) {
    begin_at(is_demo(app), node, until_ms, reason, now_ms());
    if let Some(h) = app.try_state::<std::sync::Arc<super::Health>>() {
        h.touch();
    }
}

pub(crate) fn begin_at(demo: bool, node: &str, until_ms: Option<u64>, reason: &str, now: u64) {
    let m = Maint { until_ms, reason: reason.into(), started_ms: now, demo, implicit: false, back_ms: None, went_down: false, manual: false };
    let mut map = WINDOWS.lock().unwrap();
    // The owner's maintenance mode stays as it is: its alerts are Planned already, and only the
    // owner (or its deadline) ends it.
    if map.get(&(demo, node.to_string())).is_some_and(|m| m.manual) {
        return;
    }
    map.insert((demo, node.to_string()), m);
    drop(map);
    save(demo);
}

/// Closes the window (after Verify): its Planned alerts resolve without paging anyone.
pub fn end(app: &AppHandle, node: &str) {
    let demo = is_demo(app);
    if !end_at(demo, node) {
        return;
    }
    if let Some(h) = app.try_state::<std::sync::Arc<super::Health>>() {
        h.end_maintenance(node, demo);
    }
}

/// Closes SparkBox's own window on `node`, never the owner's maintenance mode. True when one closed.
pub(crate) fn end_at(demo: bool, node: &str) -> bool {
    let mut map = WINDOWS.lock().unwrap();
    let key = (demo, node.to_string());
    if map.get(&key).is_none_or(|m| m.manual) {
        return false;
    }
    map.remove(&key);
    drop(map);
    save(demo);
    true
}

// ── R4-UPD ── an upgrade's window lasts while apt runs (decision d)

/// The work SparkBox started on `node` runs on: its window now lasts until `until_ms`.
pub fn extend(app: &AppHandle, node: &str, until_ms: u64) {
    if extend_at(is_demo(app), node, until_ms) {
        if let Some(h) = app.try_state::<std::sync::Arc<super::Health>>() {
            h.touch();
        }
    }
}

/// Moves the end of SparkBox's own window on `node` to `until_ms`, keeping when it opened and
/// why; never the owner's maintenance mode, and no window is opened. True when one moved.
pub(crate) fn extend_at(demo: bool, node: &str, until_ms: u64) -> bool {
    let mut map = WINDOWS.lock().unwrap();
    let Some(m) = map.get_mut(&(demo, node.to_string())).filter(|m| !m.manual) else { return false };
    m.until_ms = Some(until_ms);
    drop(map);
    save(demo);
    true
}

// ── R2-OPS ── maintenance mode (P1.16)

/// Puts `node` in maintenance mode: its alerts are Planned until `end_manual`, or until
/// `until_ms` (None: until the owner ends it). Nothing runs on the node. A restart SparkBox has
/// in progress there keeps its own window (it's refused).
pub fn begin_manual(app: &AppHandle, node: &str, until_ms: Option<u64>) -> Result<MaintInfo, String> {
    let m = begin_manual_at(is_demo(app), node, until_ms, now_ms())?;
    if let Some(h) = app.try_state::<std::sync::Arc<super::Health>>() {
        h.touch();
    }
    Ok(m)
}

pub(crate) fn begin_manual_at(demo: bool, node: &str, until_ms: Option<u64>, now: u64) -> Result<MaintInfo, String> {
    if until_ms.is_some_and(|u| u <= now) {
        return Err("Pick a time in the future.".into());
    }
    let mut map = WINDOWS.lock().unwrap();
    let key = (demo, node.to_string());
    if let Some(m) = map.get(&key).filter(|m| !m.manual) {
        return Err(format!("SparkBox is in the middle of its own planned work there ({}); its alerts are already Planned until it's back.", m.reason));
    }
    let m = Maint { until_ms, reason: MANUAL_REASON.into(), started_ms: now, demo, implicit: false, back_ms: None, went_down: false, manual: true };
    let info = MaintInfo { until_ms, reason: m.reason.clone(), started_ms: now };
    map.insert(key, m);
    drop(map);
    save(demo);
    Ok(info)
}

/// Ends `node`'s maintenance mode: its Planned alerts resolve without paging anyone, and any
/// that still hold are raised again by the next evaluation.
pub fn end_manual(app: &AppHandle, node: &str) -> Result<(), String> {
    let demo = is_demo(app);
    end_manual_at(demo, node)?;
    if let Some(h) = app.try_state::<std::sync::Arc<super::Health>>() {
        h.end_maintenance(node, demo);
    }
    Ok(())
}

pub(crate) fn end_manual_at(demo: bool, node: &str) -> Result<(), String> {
    let mut map = WINDOWS.lock().unwrap();
    let key = (demo, node.to_string());
    match map.get(&key) {
        None => return Err("It isn't in maintenance.".into()),
        Some(m) if !m.manual => return Err("That's SparkBox's own window for a restart; it ends by itself once the node is back.".into()),
        Some(_) => {}
    }
    map.remove(&key);
    drop(map);
    save(demo);
    Ok(())
}

/// The activity row's title: "Maintenance on cell2 for 1 h", "Maintenance on cell2 until you end it".
pub fn manual_title(node_name: &str, minutes: Option<u32>) -> String {
    match minutes {
        None => format!("Maintenance on {node_name} until you end it"),
        Some(m) if m % 60 == 0 => format!("Maintenance on {node_name} for {} h", m / 60),
        Some(m) => format!("Maintenance on {node_name} for {m} min"),
    }
}

/// Opens a window on a model SparkBox restarts in order (models' "Then restart …"): its alerts
/// and its containers' on `nodes` are Planned until it answers again, or until `until_ms` (None:
/// 15 minutes) plus the node-reboot grace. `reason` reads after its name: "restarting".
#[allow(dead_code)] // The r4 contract for models' ordered restart ("Then restart …"), which calls it.
pub fn begin_endpoint(app: &AppHandle, endpoint_id: &str, nodes: &[String], until_ms: Option<u64>, reason: &str) {
    begin_endpoint_at(is_demo(app), endpoint_id, nodes, until_ms, reason, now_ms());
    if let Some(h) = app.try_state::<std::sync::Arc<super::Health>>() {
        h.touch();
    }
}

pub(crate) fn begin_endpoint_at(demo: bool, endpoint_id: &str, nodes: &[String], until_ms: Option<u64>, reason: &str, now: u64) {
    let m = EndpointMaint { nodes: nodes.to_vec(), until_ms: until_ms.or(Some(now + SETTLE_MS)), reason: reason.into(), started_ms: now };
    ENDPOINTS.lock().unwrap().insert((demo, endpoint_id.to_string()), m);
    save(demo);
}

/// The ordered restarts open for one data source, by endpoint id.
pub fn endpoint_windows(demo: bool) -> BTreeMap<String, EndpointMaint> {
    ENDPOINTS.lock().unwrap().iter().filter(|((d, _), _)| *d == demo).map(|((_, id), m)| (id.clone(), m.clone())).collect()
}

/// "glm-5.3-flash restarting (SparkBox, 10:42)".
pub fn endpoint_text(label: &str, m: &EndpointMaint) -> String {
    format!("{label} {} (SparkBox, {})", m.reason, local_hhmm(m.started_ms))
}

/// When the window SparkBox has open on `node` (a restart or shutdown it started) opened, if
/// one is. boots.rs asks before `stamp` runs, since the first frame back can close it.
pub fn opened_ms(demo: bool, node: &str) -> Option<u64> {
    WINDOWS.lock().unwrap().get(&(demo, node.to_string())).map(|m| m.started_ms)
}

/// The node's running boot began at or after `t`, by its uptime (0: no reading, so not known).
fn booted_since(n: &crate::model::NodeSnapshot, now: u64, t: u64) -> bool {
    n.system.uptime_s > 0 && now.saturating_sub(n.system.uptime_s * 1000) >= t
}

/// Every open window for one data source, by node.
#[cfg(test)]
pub fn windows(demo: bool) -> BTreeMap<String, Maint> {
    WINDOWS.lock().unwrap().iter().filter(|((d, _), _)| *d == demo).map(|((_, n), m)| (n.clone(), m.clone())).collect()
}

// ── r1-c ──
/// Held by the tests that open Live windows: `stamp` on a Live snapshot closes the Live windows
/// of nodes it doesn't have, so they take turns.
#[cfg(test)]
pub(crate) fn live_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Fills in `NodeSnapshot.maintenance` before the rules run: opens a window for a power action
/// that has none, notes when a node came back, and closes windows whose node has settled.
/// `grace_ms` is the node-reboot rule's grace. Returns the nodes whose window just closed.
pub fn stamp(snap: &mut ClusterSnapshot, endpoints: &[EndpointInfo], grace_ms: u64) -> Vec<String> {
    let now = snap.ts_ms;
    // A model restarted in order is back once it answers (some seconds in), or past its deadline.
    ENDPOINTS.lock().unwrap().retain(|(d, id), m| {
        if *d != snap.demo {
            return true;
        }
        let answers = endpoints.iter().any(|e| &e.id == id && e.health == "ready") && now.saturating_sub(m.started_ms) >= ENDPOINT_SETTLE_MS;
        let overdue = m.until_ms.is_some_and(|u| now > u + grace_ms);
        !answers && !overdue
    });
    let mut closed = Vec::new();
    let mut map = WINDOWS.lock().unwrap();
    for n in snap.nodes.iter_mut() {
        let key = (snap.demo, n.id.clone());
        if let (None, Some(power)) = (map.get(&key), n.power_state.as_deref()) {
            let (until, reason) = match power {
                "off" | "shutting-down" => (None, "shut down"),
                _ => (Some(now + REBOOT_EXPECT_MS), "restarting"),
            };
            map.insert(key.clone(), Maint { until_ms: until, reason: reason.into(), started_ms: now, demo: snap.demo, implicit: true, back_ms: None, went_down: false, manual: false });
        }
        let Some(m) = map.get_mut(&key) else {
            n.maintenance = None;
            continue;
        };
        // Maintenance mode doesn't follow the node: it's over at its deadline (or when the owner ends it).
        if m.manual {
            if m.until_ms.is_some_and(|u| now >= u) {
                map.remove(&key);
                n.maintenance = None;
                closed.push(n.id.clone());
            } else {
                n.maintenance = Some(MaintInfo { until_ms: m.until_ms, reason: m.reason.clone(), started_ms: m.started_ms });
            }
            continue;
        }
        // A shutdown that's being woken: from now on it's a restart with a deadline.
        if m.until_ms.is_none() && n.power_state.as_deref() == Some("waking") {
            m.until_ms = Some(now + REBOOT_EXPECT_MS);
            m.reason = "restarting".into();
        }
        // ── r1-c ── Until the restart takes, the node answers on the boot it had for some seconds
        // (the agents hear first, then the command goes): it's back only once it has been seen
        // down, or on a boot that began after the window opened (SparkBox was closed meanwhile).
        let answering = reporting(n) && n.power_state.is_none();
        if n.status != crate::model::LinkStatus::Online || n.power_state.is_some() {
            m.went_down = true;
        }
        let back = answering && (m.went_down || booted_since(n, now, m.started_ms));
        if back {
            m.back_ms.get_or_insert(now);
        } else {
            m.back_ms = None;
        }
        let models_answer = endpoints.iter().filter(|e| e.nodes.iter().any(|x| x == &n.id) || e.head_node == n.id).all(|e| e.health == "ready");
        let settled = m.back_ms.is_some_and(|b| models_answer || now.saturating_sub(b) >= SETTLE_MS);
        // Answering past the deadline and grace, on whichever boot: the restart is over, or never happened.
        let overdue_and_back = answering && m.until_ms.is_some_and(|u| now > u + grace_ms);
        let never_went_down = answering && !m.went_down && m.until_ms.is_none() && now.saturating_sub(m.started_ms) > SHUTDOWN_BY_MS + grace_ms;
        if settled || overdue_and_back || never_went_down {
            map.remove(&key);
            n.maintenance = None;
            closed.push(n.id.clone());
            continue;
        }
        n.maintenance = Some(MaintInfo { until_ms: m.until_ms, reason: m.reason.clone(), started_ms: m.started_ms });
    }
    // Windows on nodes that left the inventory.
    let ids: Vec<String> = snap.nodes.iter().map(|n| n.id.clone()).collect();
    map.retain(|(d, n), _| *d != snap.demo || ids.contains(n));
    drop(map);
    save(snap.demo);
    closed
}

/// Whether a window still makes its node's alerts Planned at `now` (not past its deadline and grace).
pub fn planned_now(m: &MaintInfo, now: u64, grace_ms: u64) -> bool {
    m.until_ms.is_none_or(|u| now <= u + grace_ms)
}

/// "cell3 restarting (SparkBox, 10:42)", "cell3 off · shut down from SparkBox (10:42)",
/// "cell2 in maintenance (you, 10:42)".
pub fn planned_text(node_name: &str, m: &MaintInfo) -> String {
    let at = local_hhmm(m.started_ms);
    if m.reason == MANUAL_REASON {
        format!("{node_name} {MANUAL_REASON} (you, {at})")
    } else if m.reason == "shut down" {
        format!("{node_name} off · shut down from SparkBox ({at})")
    } else {
        format!("{node_name} {} (SparkBox, {at})", m.reason)
    }
}

/// The Mac's offset from UTC in seconds ("10:42" means the owner's 10:42).
pub(crate) fn local_offset_s() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let (sign, digits) = match text.split_at_checked(1) {
            Some(("-", d)) => (-1, d),
            Some(("+", d)) => (1, d),
            _ => return 0,
        };
        let (h, m) = (digits.get(..2).and_then(|s| s.parse::<i64>().ok()), digits.get(2..4).and_then(|s| s.parse::<i64>().ok()));
        sign * (h.unwrap_or(0) * 3600 + m.unwrap_or(0) * 60)
    })
}

/// A Unix ms time as the Mac's local "10:42".
pub fn local_hhmm(ms: u64) -> String {
    let s = (ms / 1000) as i64 + local_offset_s();
    let day = s.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, (day % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LinkStatus, NodeSnapshot};

    fn node(id: &str, status: LinkStatus, power: Option<&str>) -> NodeSnapshot {
        NodeSnapshot { id: id.into(), name: id.into(), status, power_state: power.map(String::from), ..Default::default() }
    }

    #[test]
    fn a_power_action_opens_a_window_and_a_settled_node_closes_it() {
        // Ids of their own: the windows are process-wide.
        let mut snap = ClusterSnapshot { ts_ms: 1_000_000, demo: true, nodes: vec![node("mt-a", LinkStatus::Offline, Some("rebooting"))], ..Default::default() };
        stamp(&mut snap, &[], 120_000);
        let m = snap.nodes[0].maintenance.clone().expect("a reboot is planned work");
        assert_eq!((m.reason.as_str(), m.until_ms), ("restarting", Some(1_000_000 + REBOOT_EXPECT_MS)));
        // Live never sees a Demo window.
        assert!(windows(false).get("mt-a").is_none());
        // Back, and its model answers: the window closes.
        snap.ts_ms += 60_000;
        snap.nodes[0] = node("mt-a", LinkStatus::Online, None);
        let ep = EndpointInfo { id: "e".into(), head_node: "mt-b".into(), nodes: vec!["mt-b".into(), "mt-a".into()], health: "ready".into(), ..Default::default() };
        stamp(&mut snap, &[ep], 120_000);
        assert!(snap.nodes[0].maintenance.is_none());
        assert!(windows(true).get("mt-a").is_none());
    }

    #[test]
    fn open_windows_are_saved_per_data_source_and_come_back() {
        // Live windows only: the other tests stamp Demo snapshots, which close Demo windows on
        // nodes they don't have, at the same time as this one runs.
        let _live = live_test_lock();
        let dir = std::env::temp_dir().join(format!("sb-maint-{}-{}", std::process::id(), fastrand::u32(..)));
        std::fs::create_dir_all(&dir).unwrap();
        begin_at(false, "mt-live", Some(9_000_000), "restarting", 8_000_000);
        begin_endpoint_at(false, "mt-live-ep", &["mt-live".into()], None, "restarting", 8_000_000);
        begin_at(true, "mt-demo", None, "shut down", 8_000_000);
        save_to(&dir, false);
        let live: Saved = serde_json::from_str(&std::fs::read_to_string(saved_path(&dir, false)).unwrap()).unwrap();
        assert!(live.nodes.contains_key("mt-live") && live.endpoints.contains_key("mt-live-ep"));
        assert!(!live.nodes.contains_key("mt-demo"), "Live's file holds Live's windows only");
        // SparkBox quits mid-restart and opens again: the windows are back, in their own source.
        WINDOWS.lock().unwrap().retain(|(_, n), _| n != "mt-live" && n != "mt-demo");
        ENDPOINTS.lock().unwrap().remove(&(false, "mt-live-ep".to_string()));
        load_from(&dir);
        assert_eq!(windows(false).get("mt-live").map(|m| (m.until_ms, m.reason.as_str())), Some((Some(9_000_000), "restarting")));
        assert!(endpoint_windows(false).contains_key("mt-live-ep"));
        assert!(!windows(true).contains_key("mt-live"), "a Live window never comes back as Demo's");
        WINDOWS.lock().unwrap().retain(|(_, n), _| n != "mt-live");
        ENDPOINTS.lock().unwrap().remove(&(false, "mt-live-ep".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shutdown_is_planned_until_woken() {
        let mut snap = ClusterSnapshot { ts_ms: 5_000_000, demo: true, nodes: vec![node("mt-c", LinkStatus::Offline, Some("off"))], ..Default::default() };
        stamp(&mut snap, &[], 120_000);
        let m = snap.nodes[0].maintenance.clone().unwrap();
        assert_eq!((m.reason.as_str(), m.until_ms), ("shut down", None));
        assert!(planned_now(&m, 5_000_000 + 86_400_000, 120_000), "off is planned for as long as it's off");
        assert!(planned_text("cell3", &m).starts_with("cell3 off · shut down from SparkBox ("));
        // Wake: now it has a deadline.
        snap.ts_ms += 1_000;
        snap.nodes[0] = node("mt-c", LinkStatus::Offline, Some("waking"));
        stamp(&mut snap, &[], 120_000);
        assert_eq!(snap.nodes[0].maintenance.as_ref().unwrap().until_ms, Some(5_001_000 + REBOOT_EXPECT_MS));
    }

    #[test]
    fn an_ordered_model_restart_is_planned_until_it_answers() {
        // Far-future times: the windows are process-wide, and other tests' snapshots must not find it overdue.
        const T: u64 = 4_000_000_000_000;
        let ep = |health: &str| EndpointInfo { id: "mt-ep".into(), head_node: "mt-h".into(), nodes: vec!["mt-h".into(), "mt-w".into()], health: health.into(), ..Default::default() };
        begin_endpoint_at(true, "mt-ep", &["mt-h".into(), "mt-w".into()], None, "restarting", T);
        let m = endpoint_windows(true).get("mt-ep").cloned().expect("open");
        assert_eq!((m.until_ms, m.reason.as_str()), (Some(T + SETTLE_MS), "restarting"));
        assert!(endpoint_text("glm-5.3-flash", &m).starts_with("glm-5.3-flash restarting (SparkBox, "));
        assert!(endpoint_windows(false).get("mt-ep").is_none(), "Live never sees a Demo window");
        // Still ready from before the restart reached it: not over yet.
        let mut snap = ClusterSnapshot { ts_ms: T + 5_000, demo: true, ..Default::default() };
        stamp(&mut snap, &[ep("ready")], 120_000);
        assert!(endpoint_windows(true).contains_key("mt-ep"));
        snap.ts_ms = T + 30_000;
        stamp(&mut snap, &[ep("loading")], 120_000);
        assert!(endpoint_windows(true).contains_key("mt-ep"));
        snap.ts_ms = T + 90_000;
        stamp(&mut snap, &[ep("ready")], 120_000);
        assert!(!endpoint_windows(true).contains_key("mt-ep"), "it answers: the window closes");
    }

    // ── r1-c ──
    #[test]
    fn a_window_outlives_the_seconds_the_node_still_answers_on_its_old_boot() {
        let _live = live_test_lock();
        const T: u64 = 4_200_000_000_000;
        let up = |status: LinkStatus, uptime_s: u64| NodeSnapshot { system: crate::model::SystemInfo { uptime_s, ..Default::default() }, ..node("mt-old", status, None) };
        let frame = |ts: u64, n: NodeSnapshot| ClusterSnapshot { ts_ms: ts, demo: false, nodes: vec![n], ..Default::default() };
        // node_power opens the window, tells the agents (up to 6 s), then sends the reboot: the
        // node answers on its old boot meanwhile. No model has a stage on it, so it would settle at once.
        begin_at(false, "mt-old", Some(T + REBOOT_EXPECT_MS), "restarting", T);
        for i in 1..=6 {
            let mut s = frame(T + i * 1000, up(LinkStatus::Online, 50_000 + i));
            assert!(stamp(&mut s, &[], 120_000).is_empty() && s.nodes[0].maintenance.is_some(), "still the restart SparkBox started, {i} s in");
        }
        let mut s = frame(T + 7_000, up(LinkStatus::Offline, 0));
        stamp(&mut s, &[], 120_000);
        assert!(s.nodes[0].maintenance.is_some(), "down: planned");
        let mut s = frame(T + 60_000, up(LinkStatus::Online, 20));
        assert_eq!(stamp(&mut s, &[], 120_000), ["mt-old"], "back on a new boot: over");

        // SparkBox was closed while it went down: the first frame on a boot that began after the window ends it.
        let t = T + 10 * 60_000;
        begin_at(false, "mt-old", Some(t + REBOOT_EXPECT_MS), "restarting", t);
        assert_eq!(stamp(&mut frame(t + 5 * 60_000, up(LinkStatus::Online, 120)), &[], 120_000), ["mt-old"]);

        // A restart that never happened: planned until its deadline and grace, then over.
        let t = T + 20 * 60_000;
        begin_at(false, "mt-old", Some(t + REBOOT_EXPECT_MS), "restarting", t);
        assert!(stamp(&mut frame(t + REBOOT_EXPECT_MS + 120_000, up(LinkStatus::Online, 90_000)), &[], 120_000).is_empty());
        assert_eq!(stamp(&mut frame(t + REBOOT_EXPECT_MS + 121_000, up(LinkStatus::Online, 90_001)), &[], 120_000), ["mt-old"]);

        // A shutdown can be scheduled up to 60 min ahead; one that never took the node down ends after that.
        let t = T + 40 * 60_000;
        begin_at(false, "mt-old", None, "shut down", t);
        let mut s = frame(t + SHUTDOWN_BY_MS, up(LinkStatus::Online, 90_000));
        assert!(stamp(&mut s, &[], 120_000).is_empty() && s.nodes[0].maintenance.is_some());
        assert_eq!(stamp(&mut frame(t + SHUTDOWN_BY_MS + 121_000, up(LinkStatus::Online, 90_100)), &[], 120_000), ["mt-old"]);
        assert!(opened_ms(false, "mt-old").is_none());
    }

    #[test]
    fn local_times_read_as_hours_and_minutes() {
        let t = local_hhmm(1_800_000_000_000);
        assert_eq!(t.len(), 5);
        assert_eq!(&t[2..3], ":");
    }

    // ── R2-OPS ── maintenance mode (P1.16)

    /// One Live frame of `id` (shown as cell2), stamped as on_publish does before the rules.
    fn manual_frame(id: &str, ts: u64, status: LinkStatus, uptime_s: u64) -> ClusterSnapshot {
        let n = NodeSnapshot { system: crate::model::SystemInfo { uptime_s, ..Default::default() }, ..node(id, status, None) };
        let mut s = ClusterSnapshot { ts_ms: ts, demo: false, nodes: vec![NodeSnapshot { name: "cell2".into(), ..n }], ..Default::default() };
        stamp(&mut s, &[], 120_000);
        s
    }

    #[test]
    fn maintenance_mode_makes_the_offline_alert_planned_until_the_owner_ends_it() {
        let _live = live_test_lock();
        const T: u64 = 4_400_000_000_000;
        let id = "mt-man";
        let info = begin_manual_at(false, id, None, T).unwrap();
        assert_eq!((info.reason.as_str(), info.until_ms), (MANUAL_REASON, None));
        assert!(windows(false).get(id).is_some_and(|m| m.manual && m.until_ms.is_none()), "open in Live's windows");
        assert!(!windows(true).contains_key(id), "Demo never sees it");

        // The owner restarts it by hand: two minutes offline, then back on a new boot.
        let mut e = super::super::Engine::default();
        let mut events = Vec::new();
        for i in 0..180 {
            let s = if i < 150 { manual_frame(id, T + i * 1000, LinkStatus::Offline, 0) } else { manual_frame(id, T + i * 1000, LinkStatus::Online, i - 150) };
            assert!(s.nodes[0].maintenance.as_ref().is_some_and(|m| m.reason == MANUAL_REASON), "{i} s in");
            events.extend(e.step(&s, s.ts_ms));
            if i == 149 {
                let v = e.view(s.ts_ms, Some(&s));
                let off = v.alerts.iter().find(|a| a.rule == "node-offline").expect("offline is raised");
                assert!(off.planned.as_deref().is_some_and(|p| p.starts_with("cell2 in maintenance (you, ")), "{:?}", off.planned);
                assert!(!off.counted && v.summary.crit == 0, "counted nowhere");
                assert!(off.message.starts_with("Offline · in maintenance (you, "), "{}", off.message);
            }
        }
        assert!(events.iter().any(|ev| ev.alert.rule == "node-offline"), "{events:?}");
        assert!(events.iter().all(|ev| ev.alert.muted(ev.ts_ms)), "paged to nobody: {events:?}");
        // Back and settled: it's still in maintenance (it doesn't follow the node), an hour later too.
        let s = manual_frame(id, T + SHUTDOWN_BY_MS + 10 * 60_000, LinkStatus::Online, 4000);
        assert!(s.nodes[0].maintenance.is_some() && windows(false).contains_key(id));

        // Ended: gone from the windows; away again, the node's alert counts and pages.
        end_manual_at(false, id).unwrap();
        assert!(!windows(false).contains_key(id));
        let _ = e.end_planned(id, T + 4_000_000);
        let mut events = Vec::new();
        for i in 0..150 {
            let s = manual_frame(id, T + 4_000_000 + i * 1000, LinkStatus::Offline, 0);
            assert!(s.nodes[0].maintenance.is_none());
            events.extend(e.step(&s, s.ts_ms));
        }
        let raised: Vec<_> = events.iter().filter(|ev| ev.alert.rule == "node-offline" && ev.kind == super::super::EventKind::Raised).collect();
        assert!(!raised.is_empty() && raised.iter().all(|ev| ev.alert.planned.is_none() && !ev.alert.muted(ev.ts_ms)), "{events:?}");
        assert!(end_manual_at(false, id).is_err(), "nothing left to end");
    }

    #[test]
    fn maintenance_mode_ends_at_its_deadline_and_outlasts_sparkboxs_own_restart() {
        let _live = live_test_lock();
        const T: u64 = 4_500_000_000_000;
        let id = "mt-man-timed";
        assert!(begin_manual_at(false, id, Some(T - 1), T).is_err(), "a deadline in the past");
        begin_manual_at(false, id, Some(T + 3_600_000), T).unwrap();
        // A restart SparkBox starts inside it: the owner's window stays, and SparkBox's end leaves it.
        begin_at(false, id, Some(T + 60_000 + REBOOT_EXPECT_MS), "restarting", T + 60_000);
        assert!(windows(false).get(id).is_some_and(|m| m.manual && m.until_ms == Some(T + 3_600_000)));
        assert!(!end_at(false, id), "not SparkBox's to close");
        assert!(windows(false).contains_key(id));
        assert!(manual_frame(id, T + 3_599_000, LinkStatus::Online, 100).nodes[0].maintenance.is_some(), "up and settled: still in it");
        let mut s = ClusterSnapshot { ts_ms: T + 3_600_000, demo: false, nodes: vec![node(id, LinkStatus::Online, None)], ..Default::default() };
        assert_eq!(stamp(&mut s, &[], 120_000), [id], "over at its deadline");
        assert!(s.nodes[0].maintenance.is_none() && !windows(false).contains_key(id));

        // SparkBox's own window is never taken over or ended as maintenance mode.
        begin_at(false, id, Some(T + 9_000_000), "restarting", T + 8_000_000);
        assert!(begin_manual_at(false, id, None, T + 8_000_100).unwrap_err().contains("its own planned work there (restarting)"));
        assert!(end_manual_at(false, id).unwrap_err().contains("SparkBox's own window"));
        assert!(end_at(false, id));
        assert_eq!(manual_title("cell2", Some(60)), "Maintenance on cell2 for 1 h");
        assert_eq!(manual_title("cell2", Some(90)), "Maintenance on cell2 for 90 min");
        assert_eq!(manual_title("cell2", None), "Maintenance on cell2 until you end it");
    }
}
