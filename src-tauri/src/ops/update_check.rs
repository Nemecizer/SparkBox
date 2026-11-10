//! The update check (P1.20–P1.23): what can be updated on each node (apt, DGX OS, firmware,
//! Ollama, snaps), read in the background and kept per node, so the sidebar badge and the update
//! alert rules know it without the Updates window open. The last results are kept in
//! <data>/updates-cache.json (Live only), so a restart shows them with their age and keeps when
//! security updates were first seen pending.
//!
//! Every 60 s each connected node gets one `STAMPS` read: the package lists' success stamp,
//! dpkg's status file and /var/run/reboot-required. The whole `CHECK_SCRIPT` runs when a node has
//! no result yet or has just (re)connected, when a stamp moved (apt-daily refreshed the lists,
//! someone installed something) and 6 h after its last check. Both only read, and look-only
//! doesn't stop them; each round is one poll op in the activity log (C11).
//!
//! "Check all nodes" (`refresh_all`) refreshes the package lists on every node at once (sudo,
//! fence class apt-refresh), tries again when apt-daily holds the lock, keeps every W: and E: line
//! apt printed, reads firmware in that sudo session where polkit turned the cluster user away,
//! and then checks those nodes again.
//!
//! The latest Ollama release is asked of GitHub from this Mac, never from a node.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::future::join_all;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use super::updates::{self, NodeUpdates, PendingPackage};
use super::Ops;
use crate::errors::SbError;
use crate::live::audit::{self, OpMeta};
use crate::live::exit::as_class;
use crate::live::fence_class::FenceClass;
use crate::live::ssh;
use crate::model::{ClusterSnapshot, LinkStatus, UpdateFacts};
use crate::tasks::{TaskHandle, TaskSpec};
use crate::{is_live, AppState};

/// The cheap read every minute: when the package lists were last refreshed, when dpkg last wrote
/// its status, and since when a restart has been waiting (C5).
/// It ends with `true`: stat exits 1 whenever /var/run/reboot-required is absent (the usual
/// case), and the reply is read from what it printed, so the row says ok (C5 amendment).
pub const STAMPS: &str = "stat -c '%n %Y' /var/lib/apt/periodic/update-success-stamp /var/lib/dpkg/status /var/run/reboot-required 2>/dev/null; true";

const CHECK_TAIL: &str = "; echo '--- dgx'; grep -E '^DGX_(SWBUILD_VERSION|OTA_VERSION|OTA_DATE)=' /etc/dgx-release 2>/dev/null; \
echo '--- fwupd'; fwupdmgr get-updates --json --no-unreported-check --no-metadata-check </dev/null 2>&1; echo \"fwupd-exit=$?\"; \
echo '--- snap'; snap refresh --list 2>&1; \
echo '--- ollama'; ollama --version 2>/dev/null; \
echo '--- docker'; docker info --format '{{.LiveRestoreEnabled}}' 2>/dev/null; \
echo '--- stamps'; stat -c '%n %Y' /var/lib/apt/periodic/update-success-stamp /var/lib/dpkg/status /var/run/reboot-required 2>/dev/null; true";

/// The whole check, one read with no sudo (C5): the Updates window's status script, then DGX OS
/// (/etc/dgx-release), firmware (fwupd), snaps, Ollama, Docker's live-restore and the stamps.
pub const CHECK_SCRIPT: &str = match std::str::from_utf8(&CHECK_BYTES) {
    Ok(s) => s,
    Err(_) => panic!("CHECK_SCRIPT isn't UTF-8"),
};
const CHECK_BYTES: [u8; updates::STATUS_SCRIPT.len() + CHECK_TAIL.len()] = concat(updates::STATUS_SCRIPT, CHECK_TAIL);

/// `a` then `b`, at compile time (`concat!` takes only literals, and STATUS_SCRIPT is updates.rs's).
const fn concat<const N: usize>(a: &str, b: &str) -> [u8; N] {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        out[i] = if i < a.len() { a[i] } else { b[i - a.len()] };
        i += 1;
    }
    out
}

/// "Check all nodes": refreshes the package lists, waiting up to 5 min for dpkg's lock (C3).
pub const REFRESH: &str = "apt-get -o DPkg::Lock::Timeout=300 update";
/// Firmware read as root, only during "Check all nodes" and only on a node whose polkit turned
/// the cluster user away (C5).
pub const FWUPD_SUDO: &str = "fwupdmgr get-updates --json --no-unreported-check --no-metadata-check";
/// The latest Ollama release, asked from this Mac.
pub const OLLAMA_RELEASES: &str = "https://api.github.com/repos/ollama/ollama/releases/latest";
/// Sent with the nodes whose summary changed: `{ nodes: [..] }`.
pub const CHANGED_EVENT: &str = "updates://changed";

/// How often each connected node's stamps are read.
const ROUND_EVERY: Duration = Duration::from_secs(60);
/// How often the poller looks for nodes that have just (re)connected (no command is sent).
const LOOK_EVERY: Duration = Duration::from_secs(5);
/// A node is checked in full at least this often.
const FULL_EVERY_MS: u64 = 6 * 3600 * 1000;
/// A full check that failed is tried again after this long (sooner when a stamp moves or the
/// node reconnects).
const RETRY_FAILED_MS: u64 = 10 * 60 * 1000;
/// How long the latest Ollama release is kept; a failed ask is tried again sooner.
const LATEST_FOR_MS: u64 = 6 * 3600 * 1000;
const LATEST_FAILED_FOR_MS: u64 = 30 * 60 * 1000;
const CHECK_LIMIT: Duration = Duration::from_secs(90);
const STAMPS_LIMIT: Duration = Duration::from_secs(20);
/// apt waits up to 300 s for dpkg's lock; the command gets a little more.
const REFRESH_LIMIT: Duration = Duration::from_secs(330);
const FWUPD_LIMIT: Duration = Duration::from_secs(60);
/// The waits before each new try while apt-daily holds the lists' lock.
const LOCK_WAITS: [Duration; 3] = [Duration::from_secs(15), Duration::from_secs(30), Duration::from_secs(60)];

// ---- What a node reports

/// One node's update status, as the last check read it (C10).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSummary {
    pub node: String,
    /// When the last full check finished (None: not checked yet).
    #[serde(default)]
    pub checked_ms: Option<u64>,
    /// Why the last check failed; what it read before is kept.
    #[serde(default)]
    pub error: Option<String>,
    /// Demo: only `apt` is filled in.
    #[serde(default)]
    pub demo: bool,
    #[serde(default, deserialize_with = "saved_apt")]
    pub apt: Option<NodeUpdates>,
    #[serde(default)]
    pub dgx: Option<Dgx>,
    #[serde(default)]
    pub firmware: Option<Firmware>,
    /// None on a node without Ollama.
    #[serde(default)]
    pub ollama: Option<Ollama>,
    /// None when snap couldn't say (not installed, snapd not answering).
    #[serde(default)]
    pub snaps: Option<Vec<SnapUpdate>>,
    #[serde(default)]
    pub engine_images: Option<Vec<super::image_registry::ImageUpdate>>,
    #[serde(default)]
    pub docker_live_restore: Option<bool>,
    #[serde(default)]
    pub stamps: Stamps,
    /// When SparkBox first saw security updates pending; cleared once there are none.
    #[serde(default)]
    pub security_since_ms: Option<u64>,
    /// The last "Check all nodes" on this node.
    #[serde(default)]
    pub refresh: Option<Refresh>,
}

/// DGX OS from /etc/dgx-release: the build the node was imaged with and the last OTA applied.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dgx {
    pub build: Option<String>,
    pub ota: Option<String>,
    pub ota_date: Option<String>,
    /// The OTA apt offers: the `to` version of dgx-spark-ota-update-meta.
    pub ota_pending: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FirmwareState {
    /// fwupd has nothing to offer.
    #[serde(rename = "none")]
    UpToDate,
    Available,
    /// polkit turned the cluster user away: read in a sudo session ("Check all nodes").
    NeedsSudo,
    /// fwupd isn't there, or said something else (`detail`).
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Firmware {
    pub state: FirmwareState,
    pub count: u32,
    pub devices: Vec<FirmwareDevice>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareDevice {
    pub name: String,
    pub current: String,
    pub available: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ollama {
    pub installed: Option<String>,
    /// The latest release on GitHub; None when it couldn't be asked.
    pub latest: Option<String>,
    pub newer: bool,
}

/// A snap with a refresh waiting. `snap refresh --list` names only the new version.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapUpdate {
    pub name: String,
    pub current: Option<String>,
    pub available: String,
}

/// The files `STAMPS` reads, by mtime (unix ms).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamps {
    pub lists_ms: Option<u64>,
    pub dpkg_ms: Option<u64>,
    pub reboot_required_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refresh {
    pub at_ms: u64,
    pub ok: bool,
    /// Every W: line apt printed, and every E: line.
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl UpdateSummary {
    /// What the snapshot carries for the sidebar badge and the alert rules; None until apt has
    /// been read.
    pub fn facts(&self) -> Option<UpdateFacts> {
        let (apt, checked_ms) = (self.apt.as_ref()?, self.checked_ms?);
        Some(UpdateFacts {
            pending: apt.pending.len() as u32,
            security: apt.pending.iter().filter(|p| p.security).count() as u32,
            security_since_ms: self.security_since_ms,
            reboot_required: apt.reboot_required || self.stamps.reboot_required_ms.is_some(),
            reboot_since_ms: self.stamps.reboot_required_ms,
            lists_ms: self.stamps.lists_ms.or(apt.checked_ms),
            checked_ms,
            firmware: self.firmware.as_ref().and_then(|f| match f.state {
                FirmwareState::UpToDate => Some(0),
                FirmwareState::Available => Some(f.count),
                _ => None,
            }),
            ollama_newer: self.ollama.as_ref().is_some_and(|o| o.newer),
            engine_images: self.engine_images.as_ref().map(|items| items.iter().filter(|i| i.available()).count() as u32),
            ota_pending: self.dgx.as_ref().and_then(|d| d.ota_pending.clone()),
            unconfirmed: false,
            security_gpu: Some(apt.pending.iter().filter(|p| p.security && p.sensitive).count() as u32),
        })
    }

    fn security(&self) -> usize {
        self.apt.as_ref().map(|a| a.pending.iter().filter(|p| p.security).count()).unwrap_or(0)
    }
}

/// NodeUpdates only serializes (updates.rs): read back from updates-cache.json field by field.
fn saved_apt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<NodeUpdates>, D::Error> {
    let Some(v) = Option::<Value>::deserialize(d)?.filter(|v| v.is_object()) else { return Ok(None) };
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let texts = |v: &Value| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let flag = |p: &Value, k: &str| p[k].as_bool().unwrap_or(false);
    let pending = v["pending"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| PendingPackage { name: text(&p["name"]), from: text(&p["from"]), to: text(&p["to"]), origin: text(&p["origin"]), security: flag(p, "security"), sensitive: flag(p, "sensitive"), held: flag(p, "held") })
        .collect();
    Ok(Some(NodeUpdates {
        node: text(&v["node"]),
        pending,
        reboot_required: flag(&v, "rebootRequired"),
        reboot_packages: texts(&v["rebootPackages"]),
        unattended: v["unattended"].as_bool(),
        held: texts(&v["held"]),
        driver: v["driver"].as_str().map(String::from),
        kernel: v["kernel"].as_str().map(String::from),
        checked_ms: v["checkedMs"].as_u64(),
    }))
}

// ---- Reading the replies

/// A reply of `CHECK_SCRIPT` (or `STAMPS`, as one section) by section.
fn sections(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current = String::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("--- ") {
            current = name.trim().to_string();
            out.entry(current.clone()).or_default();
        } else if !current.is_empty() {
            out.entry(current.clone()).or_default().push(line.to_string());
        }
    }
    out
}

/// `stat -c '%n %Y' …`: "/var/lib/dpkg/status 1793270000" lines.
pub fn parse_stamps<S: AsRef<str>>(lines: &[S]) -> Stamps {
    let mut s = Stamps::default();
    for line in lines {
        let Some((path, secs)) = line.as_ref().trim().rsplit_once(' ') else { continue };
        let Ok(secs) = secs.parse::<u64>() else { continue };
        let ms = Some(secs * 1000);
        match path {
            "/var/lib/apt/periodic/update-success-stamp" => s.lists_ms = ms,
            "/var/lib/dpkg/status" => s.dpkg_ms = ms,
            "/var/run/reboot-required" => s.reboot_required_ms = ms,
            _ => {}
        }
    }
    s
}

/// /etc/dgx-release's DGX_SWBUILD_VERSION and its last DGX_OTA_VERSION with that OTA's date (each
/// OTA adds a block: 7.5.0, then 7.6.0). None when the node has no such file.
pub fn parse_dgx<S: AsRef<str>>(lines: &[S]) -> Option<Dgx> {
    let mut d = Dgx::default();
    let mut any = false;
    for line in lines {
        let Some((key, value)) = line.as_ref().split_once('=') else { continue };
        let value = Some(value.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty());
        match key.trim() {
            "DGX_SWBUILD_VERSION" => d.build = value,
            "DGX_OTA_VERSION" => (d.ota, d.ota_date) = (value, None),
            "DGX_OTA_DATE" => d.ota_date = value,
            _ => continue,
        }
        any = true;
    }
    any.then_some(d)
}

/// `fwupdmgr get-updates --json …` with its output and `fwupd-exit=<code>`.
pub fn parse_firmware<S: AsRef<str>>(lines: &[S]) -> Firmware {
    let exit = lines.iter().rev().find_map(|l| l.as_ref().trim().strip_prefix("fwupd-exit=").and_then(|c| c.parse::<i32>().ok()));
    let text = lines.iter().map(AsRef::as_ref).filter(|l| !l.trim().starts_with("fwupd-exit=")).collect::<Vec<_>>().join("\n");
    let text = text.trim();
    let lower = text.to_lowercase();
    if let Some(devices) = json_devices(text) {
        let state = if devices.is_empty() { FirmwareState::UpToDate } else { FirmwareState::Available };
        return Firmware { state, count: devices.len() as u32, devices, detail: None };
    }
    if lower.contains("polkit") || lower.contains("not authorized") || lower.contains("authentication") {
        return Firmware { state: FirmwareState::NeedsSudo, detail: Some(short(text)), ..Default::default() };
    }
    // fwupdmgr's "nothing to do" is exit 2.
    if exit == Some(2) || text.contains("No updatable devices") || text.contains("No updates available") {
        return Firmware { state: FirmwareState::UpToDate, ..Default::default() };
    }
    let detail = match exit {
        Some(127) => "fwupdmgr isn't installed".to_string(),
        _ if text.is_empty() => format!("fwupdmgr said nothing (exit {})", exit.map(|c| c.to_string()).unwrap_or_else(|| "unknown".into())),
        _ => short(text),
    };
    Firmware { detail: Some(detail), ..Default::default() }
}

/// The devices with a release waiting, from fwupd's JSON (`{"Devices": [...]}`), wherever it
/// sits in the output; None when there's no such JSON.
fn json_devices(text: &str) -> Option<Vec<FirmwareDevice>> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let v: Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let devices = v.get("Devices")?.as_array()?;
    Some(
        devices
            .iter()
            .filter_map(|d| {
                // fwupd lists only newer releases, newest first.
                let newest = d["Releases"].as_array()?.first()?;
                Some(FirmwareDevice {
                    name: d["Name"].as_str().unwrap_or("Device").to_string(),
                    current: d["Version"].as_str().unwrap_or_default().to_string(),
                    available: newest["Version"].as_str().unwrap_or_default().to_string(),
                })
            })
            .collect(),
    )
}

/// At most one line of 200 characters, for a detail.
fn short(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    line.chars().take(200).collect()
}

/// `snap refresh --list`: "All snaps up to date." or a table (Name Version Rev Size Publisher
/// Notes). None when snap couldn't say.
pub fn parse_snaps<S: AsRef<str>>(lines: &[S]) -> Option<Vec<SnapUpdate>> {
    if lines.iter().any(|l| l.as_ref().contains("All snaps up to date")) {
        return Some(Vec::new());
    }
    let header = lines.iter().position(|l| {
        let mut w = l.as_ref().split_whitespace();
        (w.next(), w.next()) == (Some("Name"), Some("Version"))
    })?;
    Some(
        lines[header + 1..]
            .iter()
            .filter_map(|l| {
                let mut w = l.as_ref().split_whitespace();
                Some(SnapUpdate { name: w.next()?.into(), current: None, available: w.next()?.into() })
            })
            .collect(),
    )
}

/// `ollama --version`: "ollama version is 0.21.0", or "Warning: client version is 0.21.0" when
/// the server isn't running.
pub fn parse_ollama<S: AsRef<str>>(lines: &[S]) -> Option<String> {
    lines.iter().find_map(|l| {
        let l = l.as_ref();
        let at = l.find("ollama version is ").map(|i| i + 18).or_else(|| l.find("client version is ").map(|i| i + 18))?;
        l[at..].split_whitespace().next().map(String::from)
    })
}

/// `docker info --format '{{.LiveRestoreEnabled}}'`.
fn parse_live_restore<S: AsRef<str>>(lines: &[S]) -> Option<bool> {
    match lines.iter().map(|l| l.as_ref().trim()).find(|l| !l.is_empty())? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// One `CHECK_SCRIPT` reply, read. The Ollama latest and the times SparkBox keeps are added by
/// the cache.
pub fn parse_check(node: &str, text: &str) -> Result<UpdateSummary, String> {
    let s = sections(text);
    if !s.contains_key("upgradable") || !s.contains_key("stamps") {
        return Err(format!("The update check on {node} gave an unexpected reply: {}", short(text)));
    }
    let empty = Vec::new();
    let get = |k: &str| s.get(k).unwrap_or(&empty);
    let apt = updates::parse_status(node, text);
    let mut dgx = parse_dgx(get("dgx"));
    if let Some(p) = apt.pending.iter().find(|p| p.name == "dgx-spark-ota-update-meta") {
        dgx.get_or_insert_with(Dgx::default).ota_pending = Some(p.to.clone());
    }
    Ok(UpdateSummary {
        node: node.into(),
        dgx,
        firmware: s.contains_key("fwupd").then(|| parse_firmware(get("fwupd"))),
        ollama: parse_ollama(get("ollama")).map(|v| Ollama { installed: Some(v), ..Default::default() }),
        snaps: parse_snaps(get("snap")),
        docker_live_restore: parse_live_restore(get("docker")),
        stamps: parse_stamps(get("stamps")),
        apt: Some(apt),
        ..Default::default()
    })
}

/// Whether `latest` is a newer version than `installed`, part by part ("0.21.0" < "0.21.1" <
/// "0.22.0"; "0.21.0-rc1" counts as 0.21.0).
pub fn newer(latest: &str, installed: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim().trim_start_matches('v').split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
    };
    let (a, b) = (parts(latest), parts(installed));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// The W: and E: lines of apt's output, stdout's first.
fn apt_lines(out: &str, err: &str) -> Vec<String> {
    out.lines().chain(err.lines()).map(str::trim).filter(|l| l.starts_with("W:") || l.starts_with("E:")).map(String::from).collect()
}

/// apt-daily (or someone) holds the package lists' lock.
fn locked(out: &str, err: &str) -> bool {
    [out, err].iter().any(|t| t.contains("Could not get lock") || t.contains("Unable to lock directory"))
}

/// Asks GitHub for Ollama's latest release ("v0.21.0" → "0.21.0").
async fn fetch_latest(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).user_agent("SparkBox").build().map_err(|e| e.to_string())?;
    let reply = client.get(url).header("Accept", "application/vnd.github+json").send().await.map_err(|e| e.to_string())?;
    let v: Value = reply.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("the reply has no tag_name")?;
    Ok(tag.trim_start_matches('v').to_string())
}

// ---- The cache and the poller

/// What the poller saw of a node.
#[derive(Default)]
struct Seen {
    /// Connected at the last look.
    up: bool,
    /// Connected since the last look and not checked in full since.
    fresh: bool,
    /// When the poller last saw it connect; a check from before doesn't stand for now.
    up_since_ms: Option<u64>,
    /// When its last full check failed.
    failed_ms: Option<u64>,
}

struct Latest {
    at_ms: u64,
    version: Option<String>,
}

/// Each node's last update check (one per app: `start`; tests make their own).
pub struct UpdateCache {
    summaries: Mutex<BTreeMap<String, UpdateSummary>>,
    /// <data>/updates-cache.json; None keeps them in memory.
    path: Option<PathBuf>,
    seen: Mutex<HashMap<String, Seen>>,
    /// Held while GitHub is asked, so nodes checked together ask once.
    latest: tokio::sync::Mutex<Option<Latest>>,
    registry: tokio::sync::Mutex<HashMap<String, super::image_registry::ImageUpdate>>,
    releases_url: String,
    lock_waits: Vec<Duration>,
    /// A test's clock (unix ms); None: the real one.
    clock: Option<Arc<AtomicU64>>,
}

static APP: OnceLock<Arc<UpdateCache>> = OnceLock::new();

fn now_ms() -> u64 {
    crate::history::now_ms()
}

impl UpdateCache {
    /// A cache holding what `path` kept (nothing when None or unreadable).
    pub fn new(path: Option<PathBuf>) -> UpdateCache {
        let summaries = path.as_deref().map(load).unwrap_or_default();
        UpdateCache {
            summaries: Mutex::new(summaries),
            path,
            seen: Mutex::new(HashMap::new()),
            latest: tokio::sync::Mutex::new(None),
            registry: tokio::sync::Mutex::new(HashMap::new()),
            // Tests never ask the real GitHub: nothing listens on the discard port.
            releases_url: if cfg!(test) { "http://127.0.0.1:9/repos/ollama/ollama/releases/latest".into() } else { OLLAMA_RELEASES.into() },
            lock_waits: LOCK_WAITS.to_vec(),
            clock: None,
        }
    }

    fn now(&self) -> u64 {
        self.clock.as_ref().map(|c| c.load(Ordering::SeqCst)).unwrap_or_else(now_ms)
    }

    /// Every node's last summary, by id.
    pub fn summary(&self, node: &str) -> Option<UpdateSummary> {
        self.summaries.lock().unwrap().get(node).cloned()
    }

    /// The summaries of `nodes`, in order; a node not checked yet has an empty one.
    pub fn summaries_for(&self, nodes: &[String]) -> Vec<UpdateSummary> {
        let all = self.summaries.lock().unwrap();
        nodes.iter().map(|n| all.get(n).cloned().unwrap_or_else(|| UpdateSummary { node: n.clone(), ..Default::default() })).collect()
    }

    /// Writes each node's facts into a Live snapshot; a Demo one gets none. Facts from before the
    /// node last connected (the last launch's, or from before it went away) go in unconfirmed
    /// until its first check since, so the alert rules don't judge what may no longer be so.
    pub fn overlay_onto(&self, snap: &mut ClusterSnapshot) {
        if snap.demo {
            return;
        }
        let all = self.summaries.lock().unwrap();
        let seen = self.seen.lock().unwrap();
        for n in snap.nodes.iter_mut() {
            n.updates = all.get(&n.id).and_then(UpdateSummary::facts).map(|mut f| {
                let checked = seen.get(&n.id).is_some_and(|s| s.up && !s.fresh && s.up_since_ms.is_some_and(|u| f.checked_ms >= u));
                f.unconfirmed = !checked;
                f
            });
        }
    }

    /// The poller (`start`): a look for new connections every 5 s, a round every 60 s.
    async fn poll(self: Arc<Self>, ops: Arc<Ops>) {
        let mut since = ROUND_EVERY;
        loop {
            tokio::time::sleep(LOOK_EVERY).await;
            since += LOOK_EVERY;
            let round = since >= ROUND_EVERY;
            if round {
                since = Duration::ZERO;
            }
            self.tick(&ops, !round).await;
        }
    }

    /// One round: each connected node's stamps, then a full check wherever one is due (no result
    /// yet, just (re)connected, a stamp moved, 6 h since the last). `only_new`: only the nodes
    /// that have (re)connected since the last look, without the stamps. Returns the nodes whose
    /// summary changed.
    pub async fn tick(&self, ops: &Arc<Ops>, only_new: bool) -> Vec<String> {
        let Some(snap) = ops.cluster().filter(|s| !s.demo) else { return Vec::new() };
        let now = self.now();
        let (mut due, stamps) = {
            // Always summaries, then seen (as `store`).
            let all = self.summaries.lock().unwrap();
            let mut seen = self.seen.lock().unwrap();
            seen.retain(|id, _| snap.nodes.iter().any(|n| n.id == *id));
            let (mut due, mut stamps) = (Vec::new(), Vec::new());
            for n in &snap.nodes {
                let s = seen.entry(n.id.clone()).or_default();
                let up = n.status == LinkStatus::Online;
                if up && !s.up {
                    s.fresh = true;
                    s.up_since_ms = Some(now);
                }
                s.up = up;
                if !up {
                    continue;
                }
                let old = all.get(&n.id).and_then(|x| x.checked_ms).is_none_or(|c| now.saturating_sub(c) >= FULL_EVERY_MS);
                let may_retry = s.failed_ms.is_none_or(|f| now.saturating_sub(f) >= RETRY_FAILED_MS);
                // A check that failed is tried again once RETRY_FAILED_MS is over, and nothing is
                // read before; stamps are compared with the last check that worked.
                if s.fresh || (!only_new && may_retry && (old || s.failed_ms.is_some())) {
                    due.push(n.id.clone());
                } else if !only_new && may_retry && all.get(&n.id).is_some_and(|x| x.checked_ms.is_some()) {
                    stamps.push(n.id.clone());
                }
            }
            (due, stamps)
        };
        if !stamps.is_empty() {
            let op = OpMeta { title: "Check for update changes".into(), source: "poll".into() };
            let moved = audit::with_op_id(format!("poll-stamps-{now}"), op, audit::as_probe(join_all(stamps.iter().map(|n| self.stamps_moved(ops, n))))).await;
            due.extend(moved.into_iter().flatten());
        }
        if due.is_empty() {
            return Vec::new();
        }
        let op = OpMeta { title: "Check updates".into(), source: "poll".into() };
        let changed = audit::with_op_id(format!("poll-updates-{now}"), op, audit::as_probe(self.check(ops, &due))).await;
        self.changed(ops, &changed);
        changed
    }

    /// Reads `node`'s stamps; Some(node) when one moved since its last full check. A read cut
    /// short (no exit status: `STAMPS` ends in `true`) says nothing.
    async fn stamps_moved(&self, ops: &Ops, node: &str) -> Option<String> {
        let out = ops.live.run(node, STAMPS, None, STAMPS_LIMIT).await.ok().filter(|o| o.code == Some(0))?;
        let lines: Vec<&str> = out.stdout.lines().collect();
        let now = parse_stamps(&lines);
        let was = self.summaries.lock().unwrap().get(node).map(|s| s.stamps.clone())?;
        (now != was).then(|| node.to_string())
    }

    /// Checks `nodes` in full at once and keeps what each says; the nodes whose summary changed.
    async fn check(&self, ops: &Ops, nodes: &[String]) -> Vec<String> {
        let results = join_all(nodes.iter().map(|n| self.check_one(ops, n))).await;
        let now = self.now();
        nodes.iter().zip(results).filter_map(|(n, r)| self.store(n, r, now).then(|| n.clone())).collect()
    }

    async fn check_one(&self, ops: &Ops, node: &str) -> Result<UpdateSummary, String> {
        let out = ops.live.run(node, CHECK_SCRIPT, None, CHECK_LIMIT).await.map_err(|e| SbError::classify(&e).on(node).message)?;
        // CHECK_SCRIPT ends in `true`: anything but exit 0 is a reply cut short.
        match (out.code, &out.signal) {
            (Some(0), _) => {}
            (None, None) => return Err(format!("The connection to {node} dropped during the update check")),
            _ => return Err(out.failure("The update check", node).unwrap_or_default()),
        }
        let mut s = parse_check(node, &out.stdout)?;
        // Node update summaries must not wait through dozens of unreachable registry tags.
        // Completed per-image checks remain cached, so the next pass can make progress.
        s.engine_images = tokio::time::timeout(Duration::from_secs(90), self.image_updates(ops, node)).await.ok().flatten();
        if let Some(o) = s.ollama.as_mut() {
            o.latest = self.ollama_latest().await;
            o.newer = matches!((&o.latest, &o.installed), (Some(l), Some(i)) if newer(l, i));
        }
        Ok(s)
    }

    async fn image_updates(&self, ops: &Ops, node: &str) -> Option<Vec<super::image_registry::ImageUpdate>> {
        use super::image_registry;
        let ls = ops.live.run(node, "docker image ls --format '{{json .}}'", None, Duration::from_secs(20)).await.ok()?;
        if ls.code != Some(0) { return None; }
        let mut reports = Vec::new();
        for (repo, tag) in image_registry::installed(&ls.stdout) {
            let image = format!("{repo}:{tag}");
            let cmd = format!("docker image inspect -f '{{{{json .RepoDigests}}}}' {}", crate::live::sh_quote(&image));
            let local = ops.live.run(node, &cmd, None, Duration::from_secs(20)).await.ok().filter(|r| r.code == Some(0))
                .map(|r| image_registry::repo_digests(&r.stdout, &repo)).unwrap_or_default();
            let key = format!("{image}@{}", local.join(","));
            let mut cache = self.registry.lock().await;
            let now = self.now();
            let cached = cache.get(&key).filter(|r| now.saturating_sub(r.checked_ms) < if r.error.is_some() { LATEST_FAILED_FOR_MS } else { LATEST_FOR_MS }).cloned();
            let report = match cached {
                Some(r) => r,
                None => {
                    let (hub, ngc) = if cfg!(test) { ("http://127.0.0.1:9", "http://127.0.0.1:9") } else { ("https://hub.docker.com", "https://nvcr.io") };
                    // The total check is bounded even if several NGC candidate manifests time out.
                    let r = tokio::time::timeout(Duration::from_secs(45), image_registry::check(&repo, &tag, &local, now, hub, ngc)).await
                        .unwrap_or_else(|_| image_registry::ImageUpdate { image: image.clone(), checked_ms: now, error: Some("Registry check timed out".into()), ..Default::default() });
                    cache.retain(|_, r| now.saturating_sub(r.checked_ms) < LATEST_FOR_MS);
                    if cache.len() >= 128 { cache.clear(); }
                    cache.insert(key, r.clone());
                    r
                }
            };
            reports.push(report);
        }
        Some(reports)
    }

    /// Keeps a check's result: the times SparkBox tracks itself carry over, and a failed check
    /// keeps what was read before with the error. True when the summary changed.
    fn store(&self, node: &str, result: Result<UpdateSummary, String>, now: u64) -> bool {
        let mut all = self.summaries.lock().unwrap();
        let before = all.get(node).cloned();
        let mut seen = self.seen.lock().unwrap();
        let s = seen.entry(node.to_string()).or_default();
        s.fresh = false;
        let next = match result {
            Ok(mut new) => {
                s.failed_ms = None;
                let old = before.clone().unwrap_or_default();
                new.checked_ms = Some(now);
                new.security_since_ms = if new.security() > 0 { old.security_since_ms.or(Some(now)) } else { None };
                new.refresh = old.refresh;
                // A sudo read (Check all nodes) stands until the next one: the cluster user's own
                // read can only say it needs sudo.
                if new.firmware.as_ref().is_some_and(|f| f.state == FirmwareState::NeedsSudo)
                    && old.firmware.as_ref().is_some_and(|f| matches!(f.state, FirmwareState::UpToDate | FirmwareState::Available))
                {
                    new.firmware = old.firmware;
                }
                new
            }
            Err(e) => {
                s.failed_ms = Some(now);
                UpdateSummary { error: Some(e), ..before.clone().unwrap_or_else(|| UpdateSummary { node: node.into(), ..Default::default() }) }
            }
        };
        let changed = before.as_ref() != Some(&next);
        all.insert(node.to_string(), next);
        changed
    }

    /// Saves and tells the windows, when anything changed.
    fn changed(&self, ops: &Ops, nodes: &[String]) {
        if nodes.is_empty() {
            return;
        }
        self.save();
        ops.emit(CHANGED_EVENT, json!({ "nodes": nodes }));
    }

    /// Writes updates-cache.json (outside the lock).
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let all: Vec<UpdateSummary> = self.summaries.lock().unwrap().values().cloned().collect();
        if let Err(e) = crate::util::save_json(path, &json!({ "version": 1, "summaries": all })) {
            crate::logging::write("updates", &e);
        }
    }

    /// The latest Ollama release (kept 6 h; a failed ask is tried again after 30 min).
    async fn ollama_latest(&self) -> Option<String> {
        let mut latest = self.latest.lock().await;
        let now = self.now();
        if let Some(l) = latest.as_ref() {
            let keep = if l.version.is_some() { LATEST_FOR_MS } else { LATEST_FAILED_FOR_MS };
            if now.saturating_sub(l.at_ms) < keep {
                return l.version.clone();
            }
        }
        let version = fetch_latest(&self.releases_url).await.map_err(|e| crate::logging::write("updates", &format!("Couldn't ask GitHub for Ollama's latest release: {e}"))).ok();
        *latest = Some(Latest { at_ms: now, version: version.clone() });
        version
    }

    /// Checks the connected ones of `nodes` now (Updates' "fresh"), as reads the owner asked for.
    pub async fn check_now(&self, ops: &Arc<Ops>, nodes: &[String]) {
        let up: Vec<String> = match ops.cluster() {
            Some(snap) => nodes.iter().filter(|id| snap.nodes.iter().any(|n| n.id == **id && n.status == LinkStatus::Online)).cloned().collect(),
            None => Vec::new(),
        };
        if up.is_empty() {
            return;
        }
        let now = self.now();
        let changed = audit::with_op_id(format!("updates-check-{now}"), OpMeta { title: "Check updates".into(), source: "ui".into() }, self.check(ops, &up)).await;
        self.changed(ops, &changed);
    }

    // ---- Check all nodes

    /// `apt-get update` on every node at once, as one task (P1.23). Returns the task's id.
    pub fn refresh_all(self: &Arc<Self>, ops: &Arc<Ops>, nodes: Vec<String>) -> Result<String, SbError> {
        if nodes.is_empty() {
            return Err(SbError::invalid("Pick at least one node."));
        }
        let title = format!("Check for updates on {} node{}", nodes.len(), if nodes.len() == 1 { "" } else { "s" });
        let task = ops.tasks.start(TaskSpec::new("updates", title.clone(), &nodes).cancellable());
        task.set_retry("updates_refresh", json!({ "nodes": nodes }));
        let id = task.id().to_string();
        let (me, ops) = (self.clone(), ops.clone());
        tauri::async_runtime::spawn(audit::with_op_id(id.clone(), OpMeta { title, source: "ui".into() }, async move { me.run_refresh(&ops, &task, &nodes).await }));
        Ok(id)
    }

    async fn run_refresh(&self, ops: &Arc<Ops>, task: &TaskHandle, nodes: &[String]) {
        let names: Vec<String> = nodes.iter().map(|n| ops.name_of(n)).collect();
        task.detail(format!("apt-get update on {}", names.join(", ")));
        let done = &AtomicUsize::new(0);
        let records = join_all(nodes.iter().zip(&names).map(|(node, name)| async move {
            let (record, failure) = self.refresh_one(ops, task, node, name).await;
            // The nodes run at once; each one's step is written as it ends.
            task.step(format!("apt-get update on {name}"));
            match &failure {
                None => task.step_ok(Some(match record.warnings.len() {
                    0 => "package lists refreshed".to_string(),
                    1 => "package lists refreshed, 1 warning".to_string(),
                    n => format!("package lists refreshed, {n} warnings"),
                })),
                Some(why) => task.step_failed(why.clone()),
            }
            task.progress((done.fetch_add(1, Ordering::SeqCst) + 1) as f32 / nodes.len() as f32, None);
            (record, failure)
        }))
        .await;
        {
            let mut all = self.summaries.lock().unwrap();
            for (node, (record, _)) in nodes.iter().zip(&records) {
                all.entry(node.clone()).or_insert_with(|| UpdateSummary { node: node.clone(), ..Default::default() }).refresh = Some(record.clone());
            }
        }
        if task.is_cancelled() {
            self.changed(ops, nodes);
            task.finish_cancelled("Stopped; the package lists that were refreshed stay so");
            return;
        }
        task.step("Checking the nodes again");
        self.check(ops, nodes).await;
        task.step_ok(None);
        // Firmware polkit wouldn't show the cluster user, read in this sudo session.
        let sudo: Vec<(String, String)> = nodes
            .iter()
            .zip(&names)
            .filter(|(n, _)| self.summary(n).and_then(|s| s.firmware).is_some_and(|f| f.state == FirmwareState::NeedsSudo))
            .map(|(n, name)| (n.clone(), name.clone()))
            .collect();
        if !sudo.is_empty() {
            task.step(format!("Reading firmware updates with sudo on {}", sudo.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>().join(", ")));
            let read = join_all(sudo.iter().map(|(n, _)| self.sudo_firmware(ops, n))).await;
            let mut all = self.summaries.lock().unwrap();
            for ((node, name), f) in sudo.iter().zip(read) {
                if f.state == FirmwareState::NeedsSudo {
                    task.log(format!("{name}: firmware: {}", f.detail.clone().unwrap_or_default()));
                }
                if let Some(s) = all.get_mut(node) {
                    s.firmware = Some(f);
                }
            }
            drop(all);
            task.step_ok(None);
        }
        self.changed(ops, nodes);
        let failed: Vec<&str> = names.iter().zip(&records).filter(|(_, (_, f))| f.is_some()).map(|(n, _)| n.as_str()).collect();
        let warned: Vec<String> = names.iter().zip(&records).filter(|(_, (r, _))| !r.warnings.is_empty()).map(|(n, (r, _))| format!("{n} ({})", r.warnings.len())).collect();
        if !failed.is_empty() {
            task.finish_err(format!("Couldn't refresh the package lists on {}", failed.join(", ")));
        } else if !warned.is_empty() {
            task.finish_warn("Package lists are up to date", format!("apt printed warnings on {}; they're in the log", warned.join(", ")));
        } else {
            task.finish_ok("Package lists are up to date");
        }
    }

    /// One node's `apt-get update`, tried again while the lists are locked (15, 30, 60 s; not
    /// after Cancel). Every W: and E: line goes to the task log and the record; the second part is
    /// why it failed, when it did.
    async fn refresh_one(&self, ops: &Ops, task: &TaskHandle, node: &str, name: &str) -> (Refresh, Option<String>) {
        let mut record = Refresh { at_ms: self.now(), ..Default::default() };
        let mut waits = self.lock_waits.iter();
        let failure = loop {
            let reply = match as_class(FenceClass::AptRefresh, sudo_reply(ops, node, REFRESH, REFRESH_LIMIT)).await {
                Ok(r) => r,
                Err(e) => {
                    task.log(format!("{name}: {}", e.message));
                    record.errors.push(e.message.clone());
                    break Some(e.message);
                }
            };
            let (code, out, err) = (reply.code, reply.stdout.as_str(), reply.stderr.as_str());
            for line in apt_lines(out, err) {
                task.log(format!("{name}: {line}"));
                let kept = if line.starts_with("W:") { &mut record.warnings } else { &mut record.errors };
                if !kept.contains(&line) {
                    kept.push(line);
                }
            }
            if code == Some(0) {
                record.ok = true;
                break None;
            }
            let Some(code) = code else {
                // No exit status: apt was cut off part-way, so this isn't the lock to wait out.
                let why = match &reply.signal {
                    Some(sig) => format!("apt-get update on {name} was killed (SIG{sig}); the lists may be only partly refreshed"),
                    None => format!("The connection to {name} dropped before apt-get update finished; the lists may be only partly refreshed"),
                };
                task.log(why.clone());
                record.errors.push(why.clone());
                break Some(why);
            };
            let why = apt_lines("", err).into_iter().rfind(|l| l.starts_with("E:")).unwrap_or_else(|| format!("exit {code}"));
            match waits.next().filter(|_| locked(out, err) && !task.is_cancelled()) {
                Some(wait) => {
                    task.log(format!("{name}: the package lists are locked (apt-daily may be refreshing them); trying again in {} s", wait.as_secs()));
                    tokio::select! {
                        _ = tokio::time::sleep(*wait) => {}
                        _ = task.cancelled() => break Some(format!("Stopped while the package lists were locked: {why}")),
                    }
                }
                None => break Some(format!("apt-get update failed on {name}: {why}")),
            }
        };
        (record, failure)
    }

    /// fwupd's updates as root (`FWUPD_SUDO`, a sudo check). Refused or failed, it still needs
    /// sudo, and says why.
    async fn sudo_firmware(&self, ops: &Ops, node: &str) -> Firmware {
        match as_class(FenceClass::SudoProbe, ops.exec(node, FWUPD_SUDO, true, FWUPD_LIMIT)).await {
            Ok((code, out, err)) => {
                let mut lines: Vec<String> = out.lines().chain(err.lines()).map(String::from).collect();
                lines.push(format!("fwupd-exit={}", code.map(|c| c as i32).unwrap_or(1)));
                let mut f = parse_firmware(&lines);
                if matches!(f.state, FirmwareState::UpToDate | FirmwareState::Available) {
                    f.detail = Some("Read with sudo (Check all nodes)".into());
                }
                f
            }
            Err(e) => Firmware { state: FirmwareState::NeedsSudo, detail: Some(format!("Couldn't read it with sudo: {}", e.message)), ..Default::default() },
        }
    }
}

/// `Ops::exec` with sudo, keeping the whole reply: a death by signal and a reply with no exit
/// status at all (the connection went) are told apart.
async fn sudo_reply(ops: &Ops, node: &str, cmd: &str, limit: Duration) -> Result<ssh::Output, SbError> {
    let out = ops.live.sudo(node, cmd, limit).await.map_err(|e| SbError::classify(&e).on(node))?;
    ops.live.audit.attach_output(node, cmd, &out.stdout, &out.stderr);
    Ok(out)
}

fn load(path: &Path) -> BTreeMap<String, UpdateSummary> {
    #[derive(Deserialize)]
    struct Saved {
        #[serde(default)]
        summaries: Vec<UpdateSummary>,
    }
    match crate::util::load_json_or_quarantine::<Saved>(path) {
        Ok(Some(saved)) => saved.summaries.into_iter().filter(|s| !s.demo).map(|s| (s.node.clone(), s)).collect(),
        _ => BTreeMap::new(),
    }
}

// ---- The app's

/// Starts the app's update check: the last results from <data>/updates-cache.json, and the poller
/// (lib.rs setup, after exit::start).
pub fn start(app: &AppHandle) {
    let path = app.path().app_data_dir().ok().map(|d| d.join("updates-cache.json"));
    let cache = APP.get_or_init(|| Arc::new(UpdateCache::new(path))).clone();
    let ops = app.state::<AppState>().ops.clone();
    tauri::async_runtime::spawn(cache.poll(ops));
}

fn app_cache() -> Result<&'static Arc<UpdateCache>, SbError> {
    APP.get().ok_or_else(|| SbError::failed("The update check hasn't started yet."))
}

/// Writes each node's update facts into a Live snapshot (lib.rs publish, before health::on_publish).
pub fn overlay(snap: &mut ClusterSnapshot) {
    if let Some(cache) = APP.get() {
        cache.overlay_onto(snap);
    }
}

/// "Check all nodes" in Live (updates_refresh): see `UpdateCache::refresh_all`.
pub fn refresh_all(ops: &Arc<Ops>, nodes: Vec<String>) -> Result<String, SbError> {
    app_cache()?.refresh_all(ops, nodes)
}

/// Demo: each node's apt from the simulator; the rest is for Live data.
async fn demo_summaries(ops: &Ops, nodes: &[String]) -> Vec<UpdateSummary> {
    let mut out = Vec::new();
    for node in nodes {
        let s = match ops.updates_status(true, node).await {
            Ok(apt) => UpdateSummary { node: node.clone(), checked_ms: Some(now_ms()), demo: true, apt: Some(apt), ..Default::default() },
            Err(e) => UpdateSummary { node: node.clone(), demo: true, error: Some(e.message), ..Default::default() },
        };
        out.push(s);
    }
    out
}

/// The update status of `nodes` (every node when None) as the check last read it (C10). With
/// `fresh`, the connected ones are checked again first; it only reads.
#[tauri::command]
pub async fn updates_summary(state: State<'_, AppState>, nodes: Option<Vec<String>>, fresh: Option<bool>) -> Result<Vec<UpdateSummary>, SbError> {
    let nodes = nodes.unwrap_or_else(|| state.inventory.lock().unwrap().nodes.iter().map(|n| n.id.clone()).collect());
    if !is_live(&state) {
        return Ok(demo_summaries(&state.ops, &nodes).await);
    }
    let cache = app_cache()?;
    if fresh.unwrap_or(false) {
        cache.check_now(&state.ops, &nodes).await;
    }
    Ok(cache.summaries_for(&nodes))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Instant, UNIX_EPOCH};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::demo::DemoCluster;
    use crate::fake_node::{connect, connect_sparks, fake_node, fake_spark, real_shell, temp, Node};
    use crate::health::{AlertState, Engine, EventKind, Severity};
    use crate::inventory::Inventory;
    use crate::live::Live;
    use crate::tasks::{StepState, Task, TaskState, Tasks};

    const HOUR: u64 = 3_600_000;
    const DAY: u64 = 24 * HOUR;
    /// The tests' clock starts here (2026-10-26).
    const T0: u64 = 1_793_000_000_000;

    // ---- What the nodes print, in the real formats

    /// cell1's pending packages as the Live tour listed them (fixtures/doctor/cell1), with
    /// Chrome's origin twice, as the Updates window showed it on the cluster ("stable, stable").
    fn cell1_upgradable() -> String {
        include_str!("../live/fixtures/doctor/cell1/apt-upgradable.txt").replace("google-chrome-stable/stable ", "google-chrome-stable/stable,stable ")
    }

    /// /etc/dgx-release on cell1 (doctor.txt lines 26-38: the 7.2.3 image, then OTA 7.5.0 and 7.6.0).
    fn dgx_release(node: &str) -> String {
        let host = match node {
            "gx10-0000" => include_str!("../live/fixtures/doctor/gx10-0000/host.txt"),
            _ => include_str!("../live/fixtures/doctor/cell1/host.txt"),
        };
        host[host.find("DGX_NAME").unwrap()..].to_string()
    }

    /// What `CHECK_SCRIPT` prints on cell1 (the dgx section is grep's: no blank lines).
    fn cell1_reply() -> String {
        let release = dgx_release("cell1");
        let dgx: Vec<&str> = release.lines().filter(|l| l.starts_with("DGX_SWBUILD_VERSION=") || l.starts_with("DGX_OTA_")).collect();
        format!(
            "--- upgradable\n{}--- held\n--- reboot\n--- unattended\nAPT::Periodic::Unattended-Upgrade \"0\";\n--- checked\n1793269800\n--- dgx\n{}\n--- fwupd\nNo updatable devices\nfwupd-exit=2\n--- snap\nAll snaps up to date.\n--- ollama\nollama version is 0.21.0\n--- docker\nfalse\n--- stamps\n/var/lib/apt/periodic/update-success-stamp 1793269800\n/var/lib/dpkg/status 1793100000\n",
            cell1_upgradable(),
            dgx.join("\n")
        )
    }

    #[test]
    fn the_check_and_the_stamps_are_the_c5_literals() {
        let tail = "; echo '--- dgx'; grep -E '^DGX_(SWBUILD_VERSION|OTA_VERSION|OTA_DATE)=' /etc/dgx-release 2>/dev/null; echo '--- fwupd'; fwupdmgr get-updates --json --no-unreported-check --no-metadata-check </dev/null 2>&1; echo \"fwupd-exit=$?\"; echo '--- snap'; snap refresh --list 2>&1; echo '--- ollama'; ollama --version 2>/dev/null; echo '--- docker'; docker info --format '{{.LiveRestoreEnabled}}' 2>/dev/null; echo '--- stamps'; stat -c '%n %Y' /var/lib/apt/periodic/update-success-stamp /var/lib/dpkg/status /var/run/reboot-required 2>/dev/null; true";
        assert_eq!(CHECK_SCRIPT, format!("{}{tail}", updates::STATUS_SCRIPT));
        assert_eq!(STAMPS, "stat -c '%n %Y' /var/lib/apt/periodic/update-success-stamp /var/lib/dpkg/status /var/run/reboot-required 2>/dev/null; true");
        assert!(CHECK_SCRIPT.ends_with(STAMPS) && !CHECK_SCRIPT.contains("sudo"));
        // The copies the classifier's and the exit's tests read as reads.
        assert_eq!((CHECK_SCRIPT.to_string(), STAMPS), (crate::ops::classify::contracts::check_script(), crate::ops::classify::contracts::STAMPS));
        assert_eq!(REFRESH, "apt-get -o DPkg::Lock::Timeout=300 update");
        assert_eq!(FWUPD_SUDO, "fwupdmgr get-updates --json --no-unreported-check --no-metadata-check");
    }

    #[test]
    fn a_whole_reply_reads_as_cell1() {
        let s = parse_check("cell1", &cell1_reply()).unwrap();
        let apt = s.apt.as_ref().unwrap();
        let names: Vec<(&str, bool)> = apt.pending.iter().map(|p| (p.name.as_str(), p.security)).collect();
        assert_eq!(names.len(), 4);
        for (name, security) in [("google-chrome-stable", false), ("libbrotli1", true), ("nginx", true), ("nginx-common", true)] {
            assert!(names.contains(&(name, security)), "{name}: {names:?}");
        }
        assert_eq!(s.dgx, Some(Dgx { build: Some("7.2.3".into()), ota: Some("7.6.0".into()), ota_date: Some("Sun Oct  4 11:41:51 EDT 2026".into()), ota_pending: None }));
        assert_eq!(s.firmware.as_ref().map(|f| f.state), Some(FirmwareState::UpToDate));
        assert_eq!(s.snaps, Some(Vec::new()));
        assert_eq!(s.ollama.as_ref().and_then(|o| o.installed.as_deref()), Some("0.21.0"));
        assert_eq!(s.docker_live_restore, Some(false));
        assert_eq!(s.stamps, Stamps { lists_ms: Some(1_793_269_800_000), dpkg_ms: Some(1_793_100_000_000), reboot_required_ms: None });
        // Facts need a finished check.
        assert_eq!(s.facts(), None);
        let s = UpdateSummary { checked_ms: Some(T0), security_since_ms: Some(T0 - DAY), ..s };
        let f = s.facts().unwrap();
        assert_eq!((f.pending, f.security, f.security_since_ms, f.reboot_required, f.lists_ms, f.firmware, f.ollama_newer), (4, 3, Some(T0 - DAY), false, Some(1_793_269_800_000), Some(0), false));
        // An OTA apt offers is DGX OS's pending update.
        let offered = cell1_reply().replace("--- held", "dgx-spark-ota-update-meta/noble-updates 7.7.0 arm64 [upgradable from: 7.6.0]\n--- held");
        let s = parse_check("cell1", &offered).unwrap();
        assert_eq!(s.dgx.unwrap().ota_pending.as_deref(), Some("7.7.0"));
        // No reply to speak of: an error, not an empty summary.
        assert!(parse_check("cell1", "sh: 1: apt: not found").is_err());
    }

    #[test]
    fn dgx_release_keeps_the_last_ota() {
        let lines: Vec<String> = dgx_release("gx10-0000").lines().map(String::from).collect();
        assert_eq!(parse_dgx(&lines), Some(Dgx { build: Some("7.2.3".into()), ota: Some("7.6.0".into()), ota_date: Some("Thu Oct 15 10:29:19 EDT 2026".into()), ota_pending: None }));
        assert_eq!(parse_dgx(&["DGX_SWBUILD_VERSION=\"7.2.3\""]), Some(Dgx { build: Some("7.2.3".into()), ..Default::default() }));
        assert_eq!(parse_dgx::<&str>(&[]), None, "not a DGX OS node");
    }

    /// `fwupdmgr get-updates --json` as fwupd 1.9 prints it (the shape, not a cluster capture:
    /// the Live tour never ran fwupdmgr).
    const FWUPD_JSON: &str = r#"{
  "Devices" : [
    {
      "Name" : "System Firmware",
      "DeviceId" : "a45df35ac0e948ee180fe216a5f703f32dda163f",
      "Guid" : [ "230c8b18-8d9b-53ec-838b-6cfc0383493a" ],
      "Plugin" : "uefi_capsule",
      "Vendor" : "Micro-Star International Co., Ltd.",
      "Version" : "5.36_1.8.0",
      "Releases" : [ { "AppstreamId" : "com.msi.c931.firmware", "Version" : "5.36_1.9.0", "Description" : "<p>Fixes.</p>" } ]
    },
    {
      "Name" : "Embedded Controller",
      "Version" : "0x00000001",
      "Releases" : [ ]
    }
  ]
}"#;

    #[test]
    fn firmware_reads_as_none_available_needs_sudo_or_unknown() {
        let read = |text: &str, code: i32| {
            let mut lines: Vec<String> = text.lines().map(String::from).collect();
            lines.push(format!("fwupd-exit={code}"));
            parse_firmware(&lines)
        };
        assert_eq!(read("No updatable devices", 2).state, FirmwareState::UpToDate);
        assert_eq!(read("No updates available", 0).state, FirmwareState::UpToDate);
        assert_eq!(read("", 2).state, FirmwareState::UpToDate, "exit 2 is fwupdmgr's nothing to do");
        let f = read(FWUPD_JSON, 0);
        assert_eq!((f.state, f.count), (FirmwareState::Available, 1));
        assert_eq!(f.devices, vec![FirmwareDevice { name: "System Firmware".into(), current: "5.36_1.8.0".into(), available: "5.36_1.9.0".into() }]);
        assert_eq!(read(r#"{ "Devices" : [ ] }"#, 2).state, FirmwareState::UpToDate);
        // The JSON among other lines (stderr is merged).
        assert_eq!(read(&format!("WARNING: UEFI capsule updates not available or enabled in firmware setup\n{FWUPD_JSON}"), 0).count, 1);
        let f = read("Failed to get updates: Not authorized", 1);
        assert_eq!((f.state, f.detail.as_deref()), (FirmwareState::NeedsSudo, Some("Failed to get updates: Not authorized")));
        assert_eq!(read("Error: polkit denied the request", 1).state, FirmwareState::NeedsSudo);
        assert_eq!(read("Authentication is required to get the list of updates", 1).state, FirmwareState::NeedsSudo);
        let f = read("sh: 1: fwupdmgr: not found", 127);
        assert_eq!((f.state, f.detail.as_deref()), (FirmwareState::Unknown, Some("fwupdmgr isn't installed")));
        let f = read("Failed to connect to daemon: Error calling StartServiceByName", 1);
        assert_eq!((f.state, f.detail.as_deref()), (FirmwareState::Unknown, Some("Failed to connect to daemon: Error calling StartServiceByName")));
    }

    #[test]
    fn snaps_ollama_and_live_restore() {
        assert_eq!(parse_snaps(&["All snaps up to date."]), Some(Vec::new()));
        let table = ["Name                      Version          Rev    Size   Publisher     Notes", "firmware-updater          0+git.8cbca2e    167    9MB    canonical✓    -", "snapd                     2.73             25935  38MB   canonical✓    snapd"];
        assert_eq!(
            parse_snaps(&table),
            Some(vec![
                SnapUpdate { name: "firmware-updater".into(), current: None, available: "0+git.8cbca2e".into() },
                SnapUpdate { name: "snapd".into(), current: None, available: "2.73".into() },
            ])
        );
        assert_eq!(parse_snaps(&["error: cannot communicate with server: connection refused"]), None);
        assert_eq!(parse_snaps::<&str>(&[]), None, "snap isn't installed (its error went to /dev/null? no: 2>&1, and the shell's)");
        assert_eq!(parse_ollama(&["ollama version is 0.21.0"]).as_deref(), Some("0.21.0"));
        assert_eq!(parse_ollama(&["Warning: could not connect to a running Ollama instance", "Warning: client version is 0.21.0"]).as_deref(), Some("0.21.0"));
        assert_eq!(parse_ollama::<&str>(&[]), None);
        assert_eq!((parse_live_restore(&["true"]), parse_live_restore(&["false"]), parse_live_restore::<&str>(&[])), (Some(true), Some(false), None));
    }

    #[test]
    fn versions_compare_part_by_part() {
        for (latest, installed, newer_) in [("0.22.1", "0.21.0", true), ("0.21.0", "0.21.0", false), ("0.21.10", "0.21.9", true), ("0.9.0", "0.21.0", false), ("1.0", "0.99.99", true), ("0.21.0", "0.21.0-rc1", false), ("v0.21.1", "0.21.0", true)] {
            assert_eq!(newer(latest, installed), newer_, "{latest} vs {installed}");
        }
    }

    #[test]
    fn apt_lines_and_the_lock() {
        let err = "E: Could not get lock /var/lib/apt/lists/lock. It is held by process 2134 (apt-get)\nN: Be aware that removing the lock file is not a solution and may break your system.\nE: Unable to lock directory /var/lib/apt/lists/\n";
        assert!(locked("", err));
        assert_eq!(apt_lines("Hit:1 http://ports.ubuntu.com/ubuntu-ports noble InRelease\n", err), vec![
            "E: Could not get lock /var/lib/apt/lists/lock. It is held by process 2134 (apt-get)".to_string(),
            "E: Unable to lock directory /var/lib/apt/lists/".to_string()
        ]);
        assert!(!locked("Reading package lists...", "W: Some index files failed to download."));
    }

    #[test]
    fn the_wire_shape_and_the_saved_file() {
        let mut s = parse_check("cell1", &cell1_reply()).unwrap();
        s.checked_ms = Some(T0);
        s.security_since_ms = Some(T0 - 3 * DAY);
        s.firmware = Some(Firmware { state: FirmwareState::NeedsSudo, detail: Some("Not authorized".into()), ..Default::default() });
        s.refresh = Some(Refresh { at_ms: T0, ok: true, warnings: vec!["W: x".into()], errors: Vec::new() });
        let v = serde_json::to_value(&s).unwrap();
        for key in ["node", "checkedMs", "error", "demo", "apt", "dgx", "firmware", "ollama", "snaps", "dockerLiveRestore", "stamps", "securitySinceMs", "refresh"] {
            assert!(v.get(key).is_some(), "{key}: {v}");
        }
        assert_eq!((v["firmware"]["state"].as_str(), v["dgx"]["otaDate"].as_str(), v["stamps"]["listsMs"].as_u64()), (Some("needsSudo"), Some("Sun Oct  4 11:41:51 EDT 2026"), Some(1_793_269_800_000)));
        assert_eq!(serde_json::to_value(FirmwareState::UpToDate).unwrap(), "none");
        // updates-cache.json reads back as it was written, apt included.
        let back: UpdateSummary = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    // ---- Fake nodes that answer the update check through their shell

    /// Stand-ins for what the check runs that this Mac can't answer as a Spark would (GNU stat,
    /// the node's /etc, apt, fwupd, snap, ollama, docker): the node's files live under
    /// $SB_FAKE_STATE/root, and each command prints the real one's format.
    const STANDINS: &[(&str, &str)] = &[
        ("stat", r#"#!/usr/bin/env python3
# stat -c FORMAT FILE...: GNU stat's -c (%n, %Y) for the node's files under $SB_FAKE_STATE/root.
import os, sys
args = sys.argv[1:]
if len(args) < 3 or args[0] != "-c":
    sys.exit("stat: the fake node answers stat -c FORMAT FILE... only")
code = 0
for path in args[2:]:
    try:
        st = os.stat(os.path.join(os.environ["SB_FAKE_STATE"], "root") + path)
    except OSError:
        print("stat: cannot statx '%s': No such file or directory" % path, file=sys.stderr)
        code = 1
        continue
    print(args[1].replace("%n", path).replace("%Y", str(int(st.st_mtime))))
sys.exit(code)
"#),
        ("grep", r#"#!/usr/bin/env python3
# grep ... /etc/FILE: the node's /etc is $SB_FAKE_STATE/root/etc.
import os, sys
root = os.path.join(os.environ["SB_FAKE_STATE"], "root")
os.execv("/usr/bin/grep", ["grep"] + [root + a if a.startswith("/etc/") else a for a in sys.argv[1:]])
"#),
        ("apt", r#"st="$SB_FAKE_STATE/apt"
[ "$1 $2" = "list --upgradable" ] || { echo "E: the fake node answers apt list --upgradable only" >&2; exit 100; }
echo "WARNING: apt does not have a stable CLI interface. Use with caution in scripts." >&2
echo "Listing..."
cat "$st/upgradable" 2>/dev/null
exit 0
"#),
        ("apt-mark", r#"[ "$1" = showhold ] && cat "$SB_FAKE_STATE/apt/held" 2>/dev/null; exit 0
"#),
        ("apt-config", r#"echo 'APT::Periodic::Unattended-Upgrade "0";'
"#),
        ("fwupdmgr", r#"# fwupdmgr get-updates ...: $SB_FAKE_STATE/fwupd/out and exit; as root, root-out and root-exit when there.
d="$SB_FAKE_STATE/fwupd"; o=out; x=exit
if [ "$SB_FAKE_ROOT" = 1 ] && [ -f "$d/root-out" ]; then o=root-out; x=root-exit; fi
cat "$d/$o" 2>/dev/null || echo "No updatable devices"
exit "$(cat "$d/$x" 2>/dev/null || echo 2)"
"#),
        ("snap", r#"[ "$1 $2" = "refresh --list" ] || { echo "error: the fake node answers snap refresh --list only" >&2; exit 1; }
cat "$SB_FAKE_STATE/snap" 2>/dev/null || echo "All snaps up to date."
"#),
        ("ollama", r#"[ -f "$SB_FAKE_STATE/ollama" ] || { echo "sh: 1: ollama: not found" >&2; exit 127; }
echo "ollama version is $(cat "$SB_FAKE_STATE/ollama")"
"#),
        ("docker", r#"[ "$1" = info ] && [ -f "$SB_FAKE_STATE/live-restore" ] && { cat "$SB_FAKE_STATE/live-restore"; exit 0; }
echo "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?" >&2; exit 1
"#),
        ("apt-get", r#"# apt-get [-o ...] update: Hit lines, then $SB_FAKE_STATE/apt/warn on stderr, after apt/update-delay
# seconds; while apt/locked counts down, the lock error apt prints when apt-daily holds the lists.
# On success it touches the lists' stamp, as APT::Update::Post-Invoke-Success does on Ubuntu.
# With apt/gate (a folder's path) it first waits for four updates to have started, and fails
# if they never do: one node after another can't get past the first.
st="$SB_FAKE_STATE/apt"; mkdir -p "$st"
verb=""; skip=0
for a in "$@"; do
  if [ $skip = 1 ]; then skip=0; continue; fi
  case "$a" in -o) skip=1 ;; -*) ;; *) [ -z "$verb" ] && verb="$a" ;; esac
done
[ "$verb" = update ] || { echo "E: the fake node answers apt-get update only" >&2; exit 100; }
echo "$*" >> "$st/updates.log"
if [ -f "$st/gate" ]; then
  g=$(cat "$st/gate"); : > "$g/$$"
  i=0
  while [ "$(ls "$g" | wc -l)" -lt 4 ]; do
    i=$((i + 1))
    [ "$i" -lt 1200 ] || { echo "E: the other nodes' apt-get update never started" >&2; exit 100; }
    sleep 0.05
  done
fi
sleep "$(cat "$st/update-delay" 2>/dev/null || echo 0)"
n=$(cat "$st/locked" 2>/dev/null || echo 0)
if [ "$n" -gt 0 ]; then
  echo $((n - 1)) > "$st/locked"
  echo "E: Could not get lock /var/lib/apt/lists/lock. It is held by process 2134 (apt-get)" >&2
  echo "N: Be aware that removing the lock file is not a solution and may break your system." >&2
  echo "E: Unable to lock directory /var/lib/apt/lists/" >&2
  exit 100
fi
echo "Hit:1 http://ports.ubuntu.com/ubuntu-ports noble InRelease"
echo "Get:2 http://ports.ubuntu.com/ubuntu-ports noble-security InRelease [126 kB]"
echo "Reading package lists..."
[ -f "$st/warn" ] && cat "$st/warn" >&2
touch "$SB_FAKE_STATE/root/var/lib/apt/periodic/update-success-stamp"
exit 0
"#),
    ];

    struct Fake {
        node: Node,
        port: u16,
    }

    impl Fake {
        /// A fake node that has cell1's pending packages and DGX OS, no firmware or snap updates,
        /// no Ollama, live-restore off, its lists refreshed and dpkg last written at T0.
        async fn new(name: &str) -> Fake {
            let (node, port) = fake_node(&temp(&format!("uc-{name}")), real_shell()).await;
            for (bin, body) in STANDINS {
                node.bin(bin, body);
            }
            let f = Fake { node, port };
            f.write("apt/upgradable", &cell1_upgradable());
            f.write("root/etc/dgx-release", &dgx_release("cell1"));
            f.write("live-restore", "false\n");
            f.stamp("/var/lib/apt/periodic/update-success-stamp", T0);
            f.stamp("/var/lib/dpkg/status", T0);
            f
        }

        fn write(&self, rel: &str, text: &str) {
            let p = self.node.state_dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }

        /// The node's `path` (/var/…), last written at `ms`.
        fn stamp(&self, path: &str, ms: u64) {
            let p = self.node.state_dir.join("root").join(path.trim_start_matches('/'));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let f = std::fs::OpenOptions::new().create(true).append(true).open(&p).unwrap();
            f.set_modified(UNIX_EPOCH + Duration::from_millis(ms)).unwrap();
        }

        /// The packages an install took off the list, and dpkg's status written at `ms`.
        fn installed(&self, names: &[&str], ms: u64) {
            let left: Vec<String> = std::fs::read_to_string(self.node.state_dir.join("apt/upgradable")).unwrap().lines().filter(|l| !names.iter().any(|n| l.starts_with(&format!("{n}/")))).map(|l| format!("{l}\n")).collect();
            self.write("apt/upgradable", &left.concat());
            self.stamp("/var/lib/dpkg/status", ms);
        }

        fn checks(&self) -> usize {
            self.node.ran(CHECK_SCRIPT).len()
        }

        fn stamp_reads(&self) -> usize {
            self.node.log.lock().unwrap().iter().filter(|c| *c == STAMPS).count()
        }
    }

    struct Rig {
        ops: Arc<Ops>,
        live: Arc<Live>,
        fakes: Vec<Fake>,
        events: Arc<Mutex<Vec<Value>>>,
        /// A node the snapshot reports offline (the reconnect test).
        down: Arc<Mutex<Option<String>>>,
    }

    async fn rig(name: &str, ids: &[&str]) -> Rig {
        let mut fakes = Vec::new();
        for id in ids {
            fakes.push(Fake::new(&format!("{name}-{id}")).await);
        }
        let ports: Vec<(&str, u16)> = ids.iter().zip(&fakes).map(|(id, f)| (*id, f.port)).collect();
        let (live, inventory) = connect(&temp(&format!("uc-{name}-app")), &ports).await;
        rig_on(live, inventory, fakes)
    }

    fn rig_on(live: Arc<Live>, inventory: Inventory, fakes: Vec<Fake>) -> Rig {
        let ops = Ops::with_poll(live.clone(), Arc::new(Mutex::new(DemoCluster::new(&Inventory::example()))), Tasks::new(None), Duration::from_millis(5));
        let down: Arc<Mutex<Option<String>>> = Arc::default();
        let (l, d) = (live.clone(), down.clone());
        ops.set_snapshot_source(move || {
            let mut snap = l.snapshot(&inventory);
            if let Some(n) = snap.nodes.iter_mut().find(|n| Some(&n.id) == d.lock().unwrap().as_ref()) {
                n.status = LinkStatus::Offline;
            }
            Some(snap)
        });
        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        let e = events.clone();
        ops.set_emitter(move |event, payload| {
            if event == CHANGED_EVENT {
                e.lock().unwrap().push(payload);
            }
        });
        Rig { ops, live, fakes, events, down }
    }

    /// A port nothing listens on: asking GitHub fails at once (tests never ask the real one).
    async fn closed_url() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}/repos/ollama/ollama/releases/latest", l.local_addr().unwrap())
    }

    /// GitHub's releases API, standing in: each request gets `status` and `body`; the request
    /// heads are kept.
    async fn github(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/repos/ollama/ollama/releases/latest", l.local_addr().unwrap());
        let heads: Arc<Mutex<Vec<String>>> = Arc::default();
        let h = heads.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                h.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        (url, heads)
    }

    fn cache(path: Option<PathBuf>, clock: &Arc<AtomicU64>, releases: String) -> Arc<UpdateCache> {
        let mut c = UpdateCache::new(path);
        c.clock = Some(clock.clone());
        c.releases_url = releases;
        c.lock_waits = vec![Duration::from_millis(50); 3];
        Arc::new(c)
    }

    async fn finished(ops: &Ops, id: &str) -> Task {
        for _ in 0..600 {
            if let Some(t) = ops.tasks.get(id).filter(|t| t.state != TaskState::Running) {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("task {id} never finished: {:?}", ops.tasks.get(id));
    }

    #[tokio::test]
    async fn a_round_reads_a_node_through_its_shell() {
        let r = rig("shell", &["cell1"]).await;
        let f = &r.fakes[0];
        f.write("ollama", "0.21.0\n");
        f.write("snap", "Name              Version          Rev    Size   Publisher     Notes\nfirmware-updater  0+git.8cbca2e    167    9MB    canonical✓    -\n");
        f.write("live-restore", "true\n");
        f.write("fwupd/out", FWUPD_JSON);
        f.write("fwupd/exit", "0");
        f.stamp("/var/run/reboot-required", T0 - DAY);
        let (url, heads) = github(200, r#"{"tag_name":"v0.22.1","name":"v0.22.1"}"#).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, url);
        assert_eq!(c.tick(&r.ops, false).await, vec!["cell1".to_string()]);
        let s = c.summary("cell1").unwrap();
        assert_eq!((s.checked_ms, s.error.as_deref(), s.demo), (Some(T0), None, false));
        assert_eq!(s.apt.as_ref().map(|a| a.pending.len()), Some(4));
        assert_eq!(s.dgx.as_ref().and_then(|d| d.ota.as_deref()), Some("7.6.0"));
        assert_eq!(s.firmware.as_ref().map(|f| (f.state, f.count)), Some((FirmwareState::Available, 1)));
        assert_eq!(s.ollama, Some(Ollama { installed: Some("0.21.0".into()), latest: Some("0.22.1".into()), newer: true }));
        assert_eq!(s.snaps.as_ref().map(|v| v.len()), Some(1));
        assert_eq!(s.docker_live_restore, Some(true));
        assert_eq!(s.stamps, Stamps { lists_ms: Some(T0), dpkg_ms: Some(T0), reboot_required_ms: Some(T0 - DAY) });
        assert_eq!(s.security_since_ms, Some(T0));
        let facts = s.facts().unwrap();
        assert_eq!((facts.pending, facts.security, facts.reboot_required, facts.reboot_since_ms, facts.firmware, facts.ollama_newer), (4, 3, true, Some(T0 - DAY), Some(1), true));
        // GitHub was asked from this Mac, as SparkBox.
        let heads = heads.lock().unwrap().clone();
        assert_eq!(heads.len(), 1);
        assert!(heads[0].starts_with("GET /repos/ollama/ollama/releases/latest ") && heads[0].to_lowercase().contains("user-agent: sparkbox"), "{}", heads[0]);
        // One poll op, its row a probe; no sudo on the node.
        let rows: Vec<_> = r.live.audit.entries().into_iter().filter(|e| e.command == CHECK_SCRIPT).collect();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert!(row.op_id.as_deref().is_some_and(|o| o.starts_with("poll-")), "{row:?}");
        assert_eq!((row.title.as_str(), row.source.as_str(), row.class.as_str(), row.kind.as_str(), row.state.as_str()), ("Check updates", "poll", "probe", "command", "ok"));
        assert!(f.node.ran("sudo").is_empty());
        assert_eq!(r.events.lock().unwrap().clone(), vec![json!({ "nodes": ["cell1"] })]);
        r.live.stop();
    }

    #[tokio::test]
    async fn stamps_every_round_and_the_whole_check_when_one_moves_or_six_hours_pass() {
        let r = rig("rounds", &["cell1", "cell2"]).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        // First round: neither has a result.
        c.tick(&r.ops, false).await;
        assert_eq!(r.fakes.iter().map(|f| (f.checks(), f.stamp_reads())).collect::<Vec<_>>(), [(1, 0), (1, 0)]);
        // A quiet round: stamps only.
        clock.fetch_add(60_000, Ordering::SeqCst);
        assert!(c.tick(&r.ops, false).await.is_empty());
        assert_eq!(r.fakes.iter().map(|f| (f.checks(), f.stamp_reads())).collect::<Vec<_>>(), [(1, 1), (1, 1)]);
        let stamp_rows: Vec<_> = r.live.audit.entries().into_iter().filter(|e| e.command == STAMPS).collect();
        // No /var/run/reboot-required here, so stat exits 1: the row still says ok (C5 amendment).
        assert!(stamp_rows.iter().all(|e| e.title == "Check for update changes" && e.source == "poll" && e.class == "probe" && e.state == "ok"), "{stamp_rows:?}");
        assert_eq!(stamp_rows.iter().map(|e| e.op_id.clone()).collect::<std::collections::BTreeSet<_>>().len(), 1, "one op for the round");
        // Someone installs libbrotli1 on cell2 (dpkg writes its status): the next round checks it.
        clock.fetch_add(60_000, Ordering::SeqCst);
        r.fakes[1].installed(&["libbrotli1"], T0 + 90_000);
        assert_eq!(c.tick(&r.ops, false).await, vec!["cell2".to_string()]);
        assert_eq!(r.fakes.iter().map(|f| (f.checks(), f.stamp_reads())).collect::<Vec<_>>(), [(1, 2), (2, 2)]);
        assert_eq!(c.summary("cell2").unwrap().facts().unwrap().security, 2);
        assert_eq!(r.events.lock().unwrap().last().cloned(), Some(json!({ "nodes": ["cell2"] })));
        // The lists refreshed by apt-daily move the other stamp.
        clock.fetch_add(60_000, Ordering::SeqCst);
        r.fakes[0].stamp("/var/lib/apt/periodic/update-success-stamp", T0 + 150_000);
        c.tick(&r.ops, false).await;
        assert_eq!(r.fakes[0].checks(), 2);
        // 6 h after its last check a node is checked again, stamps or not.
        clock.store(T0 + 180_000 + 6 * HOUR, Ordering::SeqCst);
        c.tick(&r.ops, false).await;
        assert_eq!((r.fakes[0].checks(), r.fakes[1].checks()), (3, 3));
        // No background read was ever a sudo command.
        for f in &r.fakes {
            assert!(f.node.ran("sudo").is_empty(), "{:?}", f.node.ran("sudo"));
        }
        r.live.stop();
    }

    #[tokio::test]
    async fn a_node_that_reconnects_is_checked_again_at_the_next_look() {
        let r = rig("reconnect", &["cell1"]).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        c.tick(&r.ops, false).await;
        assert_eq!(r.fakes[0].checks(), 1);
        // Between rounds, the 5 s look runs nothing while nothing changed.
        c.tick(&r.ops, true).await;
        assert_eq!((r.fakes[0].checks(), r.fakes[0].stamp_reads()), (1, 0));
        // The node is seen gone, then back: the look checks it in full, stamps unchanged.
        *r.down.lock().unwrap() = Some("cell1".into());
        c.tick(&r.ops, true).await;
        *r.down.lock().unwrap() = None;
        c.tick(&r.ops, true).await;
        assert_eq!((r.fakes[0].checks(), r.fakes[0].stamp_reads()), (2, 0));
        // A real dropped connection, as the 5 s look sees it.
        let before = r.fakes[0].node.total_connections();
        r.fakes[0].node.drop_connections().await;
        let deadline = Instant::now() + Duration::from_secs(15);
        while r.fakes[0].checks() < 3 && Instant::now() < deadline {
            c.tick(&r.ops, true).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(r.fakes[0].node.total_connections() > before);
        assert_eq!(r.fakes[0].checks(), 3, "checked again after the reconnect");
        r.live.stop();
    }

    #[tokio::test]
    async fn a_failed_check_keeps_what_was_read_and_is_tried_again_after_ten_minutes() {
        let r = rig("failed", &["cell1"]).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        c.tick(&r.ops, false).await;
        let before = c.summary("cell1").unwrap();
        // The next check fails (the connection went in the middle of it).
        clock.fetch_add(60_000, Ordering::SeqCst);
        assert!(c.store("cell1", Err("The connection to cell1 dropped".into()), T0 + 60_000));
        let s = c.summary("cell1").unwrap();
        assert_eq!((s.error.as_deref(), s.checked_ms, s.apt.clone()), (Some("The connection to cell1 dropped"), Some(T0), before.apt.clone()));
        assert!(s.facts().is_some(), "the badge keeps the last counts");
        // For 10 minutes nothing is read, stamps included.
        let ran = |f: &Fake| (f.checks(), f.stamp_reads());
        clock.fetch_add(9 * 60_000, Ordering::SeqCst);
        c.tick(&r.ops, false).await;
        assert_eq!(ran(&r.fakes[0]), (1, 0));
        // Then it's checked again, and the error goes.
        clock.fetch_add(60_000, Ordering::SeqCst);
        c.tick(&r.ops, false).await;
        assert_eq!(ran(&r.fakes[0]), (2, 0));
        assert_eq!(c.summary("cell1").unwrap().error, None);
        // Back to stamps.
        clock.fetch_add(60_000, Ordering::SeqCst);
        c.tick(&r.ops, false).await;
        assert_eq!(ran(&r.fakes[0]), (2, 1));
        r.live.stop();
    }

    #[tokio::test]
    async fn when_security_updates_were_first_seen_survives_a_restart() {
        let r = rig("since", &["cell1"]).await;
        let dir = temp("uc-since-data");
        let path = dir.join("updates-cache.json");
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(Some(path.clone()), &clock, closed_url().await);
        c.tick(&r.ops, false).await;
        assert_eq!(c.summary("cell1").unwrap().security_since_ms, Some(T0));
        assert!(path.exists());
        // SparkBox restarts 3 days later: the last result is there with its time, and the date.
        clock.store(T0 + 3 * DAY, Ordering::SeqCst);
        let c = cache(Some(path.clone()), &clock, closed_url().await);
        let s = c.summary("cell1").unwrap();
        assert_eq!((s.checked_ms, s.security_since_ms, s.apt.as_ref().map(|a| a.pending.len())), (Some(T0), Some(T0), Some(4)));
        // Checked again (the 6 h rule): still pending, still since T0.
        c.tick(&r.ops, false).await;
        let s = c.summary("cell1").unwrap();
        assert_eq!((s.checked_ms, s.security_since_ms), (Some(T0 + 3 * DAY), Some(T0)));
        // The security updates installed: cleared, and saved so.
        clock.fetch_add(60_000, Ordering::SeqCst);
        r.fakes[0].installed(&["libbrotli1", "nginx", "nginx-common"], T0 + 3 * DAY + 30_000);
        c.tick(&r.ops, false).await;
        assert_eq!(c.summary("cell1").unwrap().security_since_ms, None);
        assert_eq!(cache(Some(path), &clock, closed_url().await).summary("cell1").unwrap().security_since_ms, None);
        r.live.stop();
    }

    /// The four nodes as the update check reads them, through the overlay and the alert rules,
    /// on a fake clock: no Updates window anywhere.
    #[tokio::test]
    async fn the_snapshot_carries_the_facts_and_the_rules_judge_them() {
        let r = rig("overlay", &["cell1", "cell2", "cell3", "gx10-0000"]).await;
        // cell2, cell3 and gx10-0000 have Chrome and libbrotli1 pending, as on the cluster.
        let two: String = cell1_upgradable().lines().filter(|l| !l.starts_with("nginx")).map(|l| format!("{l}\n")).collect();
        for f in &r.fakes[1..] {
            f.write("apt/upgradable", &two);
        }
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        let snap = |c: &UpdateCache| {
            let mut s = r.ops.cluster().unwrap();
            c.overlay_onto(&mut s);
            s
        };
        // Before any check: no facts, no update alerts.
        let mut e = Engine::default();
        let s = snap(&c);
        assert!(s.nodes.iter().all(|n| n.updates.is_none()));
        e.step(&s, T0);
        c.tick(&r.ops, false).await;
        let s = snap(&c);
        let counts: Vec<(u32, u32)> = s.nodes.iter().map(|n| n.updates.as_ref().map(|u| (u.pending, u.security)).unwrap()).collect();
        assert_eq!(counts, [(4, 3), (2, 1), (2, 1), (2, 1)]);
        let rows = |e: &Engine, s: &ClusterSnapshot, now: u64| -> Vec<(String, Severity, String)> {
            let mut v: Vec<_> = e.view(now, Some(s)).alerts.into_iter().filter(|a| a.rule == "security-updates-pending" && a.state == AlertState::Firing).map(|a| (a.entity.id, a.severity, a.message)).collect();
            v.sort();
            v
        };
        e.step(&s, T0 + 1000);
        assert_eq!(rows(&e, &s, T0 + 1000), vec![
            ("cell1".into(), Severity::Info, "3 security updates pending".into()),
            ("cell2".into(), Severity::Info, "1 security update pending".into()),
            ("cell3".into(), Severity::Info, "1 security update pending".into()),
            ("gx10-0000".into(), Severity::Info, "1 security update pending".into()),
        ]);
        // A week on (apt-daily kept the lists fresh): warnings.
        let week = T0 + 7 * DAY;
        clock.store(week, Ordering::SeqCst);
        for f in &r.fakes {
            f.stamp("/var/lib/apt/periodic/update-success-stamp", week);
        }
        c.tick(&r.ops, false).await;
        let s = snap(&c);
        e.step(&s, week);
        let now = rows(&e, &s, week);
        assert!(now.iter().all(|(_, sev, m)| *sev == Severity::Warn && m.ends_with("for 7 days")), "{now:?}");
        assert!(e.view(week, Some(&s)).alerts.iter().all(|a| a.rule != "package-lists-stale"));
        // Someone upgrades libbrotli1 on cell2 from a terminal: the next round sees dpkg's stamp
        // move, checks it, and the alert clears with that snapshot.
        clock.fetch_add(60_000, Ordering::SeqCst);
        r.fakes[1].installed(&["libbrotli1"], week + 30_000);
        assert_eq!(c.tick(&r.ops, false).await, vec!["cell2".to_string()]);
        let s = snap(&c);
        assert_eq!(s.nodes[1].updates.as_ref().map(|u| (u.pending, u.security, u.security_since_ms)), Some((1, 0, None)));
        let ev = e.step(&s, week + 60_000);
        assert!(ev.iter().any(|x| x.kind == EventKind::Resolved && x.alert.rule == "security-updates-pending" && x.alert.entity.id == "cell2"), "{ev:?}");
        assert_eq!(rows(&e, &s, week + 60_000).len(), 3);
        // Two days without the lists refreshed: stale on every node.
        let later = week + 60_000 + 2 * DAY;
        clock.store(later, Ordering::SeqCst);
        c.tick(&r.ops, false).await;
        let s = snap(&c);
        e.step(&s, later);
        assert_eq!(e.view(later, Some(&s)).alerts.iter().filter(|a| a.rule == "package-lists-stale" && a.state == AlertState::Firing).count(), 4);
        // The overlay writes nothing into a Demo snapshot, though the cache has every node.
        let mut demo = r.ops.cluster().unwrap();
        demo.demo = true;
        c.overlay_onto(&mut demo);
        assert!(demo.nodes.iter().all(|n| n.updates.is_none()));
        r.live.stop();
    }

    const UPDATE_RULES: [&str; 3] = ["security-updates-pending", "restart-pending", "package-lists-stale"];

    /// SparkBox opens 3 days after its last check. What updates-cache.json kept is in the
    /// snapshot (the badge keeps its counts) but raises no alert before the first check, which
    /// comes about 5 s later; the alerts follow that check.
    #[tokio::test]
    async fn kept_facts_raise_no_alert_until_the_first_check_after_launch() {
        let r = rig("launch", &["cell1"]).await;
        let f = &r.fakes[0];
        f.write("root/var/run/reboot-required", "*** System restart required ***\n");
        f.stamp("/var/run/reboot-required", T0 - 4 * DAY);
        let path = temp("uc-launch-data").join("updates-cache.json");
        let clock = Arc::new(AtomicU64::new(T0));
        cache(Some(path.clone()), &clock, closed_url().await).tick(&r.ops, false).await;
        // While SparkBox was closed, apt-daily refreshed the lists and the node restarted.
        let later = T0 + 3 * DAY;
        f.stamp("/var/lib/apt/periodic/update-success-stamp", later - HOUR);
        std::fs::remove_file(f.node.state_dir.join("root/var/run/reboot-required")).unwrap();
        clock.store(later, Ordering::SeqCst);
        let c = cache(Some(path), &clock, closed_url().await);
        let snap = || {
            let mut s = r.ops.cluster().unwrap();
            c.overlay_onto(&mut s);
            s
        };
        let mut e = Engine::default();
        // The first publishes, before the poller's first look: the kept facts, unconfirmed.
        let s = snap();
        assert_eq!(s.nodes[0].updates.as_ref().map(|u| (u.pending, u.security, u.reboot_required, u.unconfirmed)), Some((4, 3, true, true)));
        let ev = e.step(&s, later);
        assert!(ev.iter().all(|x| !UPDATE_RULES.contains(&x.alert.rule.as_str())), "{ev:?}");
        assert!(e.view(later, Some(&s)).alerts.iter().all(|a| !UPDATE_RULES.contains(&a.rule.as_str())));
        // The first check: what the node says now. Its security updates still wait (since T0).
        c.tick(&r.ops, false).await;
        let s = snap();
        assert_eq!(s.nodes[0].updates.as_ref().map(|u| (u.reboot_required, u.lists_ms, u.unconfirmed)), Some((false, Some(later - HOUR), false)));
        let ev = e.step(&s, later + 10_000);
        let raised: Vec<(&str, Severity, &str)> = ev.iter().filter(|x| x.kind == EventKind::Raised && UPDATE_RULES.contains(&x.alert.rule.as_str())).map(|x| (x.alert.rule.as_str(), x.alert.severity, x.alert.message.as_str())).collect();
        assert_eq!(raised, [("security-updates-pending", Severity::Info, "3 security updates pending")]);
        r.live.stop();
    }

    /// A node that goes away and comes back keeps its update alerts as they were until its first
    /// check since: none clears and comes back (a second notice) in between.
    #[tokio::test]
    async fn update_alerts_hold_while_a_node_reconnects() {
        let r = rig("hold", &["cell1"]).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        let snap = || {
            let mut s = r.ops.cluster().unwrap();
            c.overlay_onto(&mut s);
            s
        };
        let security = |e: &Engine, s: &ClusterSnapshot, now: u64| e.view(now, Some(s)).alerts.into_iter().find(|a| a.rule == "security-updates-pending" && a.state == AlertState::Firing).map(|a| (a.severity, a.firing_since_ms));
        let mut e = Engine::default();
        c.tick(&r.ops, false).await;
        e.step(&snap(), T0);
        // A week on (apt-daily keeping the lists fresh): a warning.
        let week = T0 + 7 * DAY;
        clock.store(week, Ordering::SeqCst);
        r.fakes[0].stamp("/var/lib/apt/periodic/update-success-stamp", week);
        c.tick(&r.ops, false).await;
        let s = snap();
        e.step(&s, week);
        let before = security(&e, &s, week);
        assert_eq!(before.map(|b| b.0), Some(Severity::Warn));
        // The 5 s look sees it gone.
        *r.down.lock().unwrap() = Some("cell1".into());
        clock.store(week + 5_000, Ordering::SeqCst);
        c.tick(&r.ops, true).await;
        e.step(&snap(), week + 5_000);
        // Back, before the next look: the facts from before, unconfirmed; the alert as it was.
        *r.down.lock().unwrap() = None;
        let s = snap();
        assert_eq!(s.nodes[0].updates.as_ref().map(|u| (u.security, u.unconfirmed)), Some((3, true)));
        let ev = e.step(&s, week + 7_000);
        assert!(ev.iter().all(|x| x.alert.rule != "security-updates-pending"), "{ev:?}");
        assert_eq!(security(&e, &s, week + 7_000), before);
        // The look checks it again: confirmed, the same alert carries on.
        clock.store(week + 10_000, Ordering::SeqCst);
        c.tick(&r.ops, true).await;
        assert_eq!(r.fakes[0].checks(), 3);
        let s = snap();
        assert_eq!(s.nodes[0].updates.as_ref().map(|u| u.unconfirmed), Some(false));
        let ev = e.step(&s, week + 10_000);
        assert!(ev.iter().all(|x| x.alert.rule != "security-updates-pending"), "{ev:?}");
        assert_eq!(security(&e, &s, week + 10_000), before);
        r.live.stop();
    }

    /// The connection drops in the middle of a full check: the reply is cut short with no exit
    /// status. The summary says the connection dropped and keeps what was read before.
    #[tokio::test]
    async fn a_check_cut_short_says_the_connection_dropped() {
        let r = rig("dropcheck", &["cell1"]).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        c.tick(&r.ops, false).await;
        let before = c.summary("cell1").unwrap();
        // apt list now takes 3 s; the connection goes 1 s into the check.
        r.fakes[0].node.bin("apt", "sleep 3; echo \"Listing...\"; cat \"$SB_FAKE_STATE/apt/upgradable\"\n");
        clock.store(T0 + 7 * HOUR, Ordering::SeqCst);
        let (c2, ops) = (c.clone(), r.ops.clone());
        let round = tokio::spawn(async move { c2.tick(&ops, false).await });
        tokio::time::sleep(Duration::from_millis(1000)).await;
        r.fakes[0].node.drop_connections().await;
        let changed = tokio::time::timeout(Duration::from_secs(60), round).await.expect("the round ends").unwrap();
        assert_eq!(changed, ["cell1".to_string()]);
        let s = c.summary("cell1").unwrap();
        assert_eq!(s.error.as_deref(), Some("The connection to cell1 dropped during the update check"));
        assert_eq!((s.checked_ms, &s.apt, &s.stamps), (before.checked_ms, &before.apt, &before.stamps));
        // Its activity row ended failed too.
        assert!(r.live.audit.entries().iter().any(|e| e.command == CHECK_SCRIPT && e.state == "failed" && e.code.is_none()));
        r.live.stop();
    }

    /// A check killed by a signal says so.
    #[tokio::test]
    async fn a_check_killed_says_so() {
        let r = rig("killcheck", &["cell1"]).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        c.tick(&r.ops, false).await;
        r.fakes[0].node.bin("apt", "sleep 3; echo \"Listing...\"; cat \"$SB_FAKE_STATE/apt/upgradable\"\n");
        r.fakes[0].node.kill_child_with("KILL", "--- upgradable");
        clock.store(T0 + 7 * HOUR, Ordering::SeqCst);
        c.tick(&r.ops, false).await;
        let s = c.summary("cell1").unwrap();
        assert_eq!((s.error.as_deref(), s.checked_ms), (Some("The update check on cell1 was killed (SIGKILL)"), Some(T0)));
        r.live.stop();
    }

    /// The connection drops during the minute's stamps read: a read cut short isn't a stamp
    /// that moved, so no full check follows it.
    #[tokio::test]
    async fn a_stamps_read_cut_short_is_not_a_change() {
        let r = rig("dropstamps", &["cell1"]).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        c.tick(&r.ops, false).await;
        r.fakes[0].node.bin("stat", "sleep 3\n");
        clock.fetch_add(60_000, Ordering::SeqCst);
        let (c2, ops) = (c.clone(), r.ops.clone());
        let round = tokio::spawn(async move { c2.tick(&ops, false).await });
        tokio::time::sleep(Duration::from_millis(1000)).await;
        r.fakes[0].node.drop_connections().await;
        let changed = tokio::time::timeout(Duration::from_secs(60), round).await.expect("the round ends").unwrap();
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!((r.fakes[0].checks(), r.fakes[0].stamp_reads()), (1, 1));
        assert_eq!(c.summary("cell1").unwrap().error, None);
        r.live.stop();
    }

    #[tokio::test]
    async fn check_all_nodes_runs_the_four_at_once_and_keeps_every_warning() {
        let ids = ["cell1", "cell2", "cell3", "gx10-0000"];
        let r = rig("refresh", &ids).await;
        // Each node's apt-get update waits for all four to have started: a refresh that takes the
        // nodes one at a time hangs on the first and fails, however busy this machine is.
        let gate = temp("uc-refresh-gate");
        for f in &r.fakes {
            f.write("apt/gate", gate.to_str().unwrap());
        }
        // A W: line on cell2 (a source listed twice), and apt-daily holding cell3's lock once.
        r.fakes[1].write("apt/warn", "W: Target Packages (main/binary-arm64/Packages) is configured multiple times in /etc/apt/sources.list.d/google-chrome.list:3 and /etc/apt/sources.list.d/google-chrome.sources:1\n");
        r.fakes[2].write("apt/locked", "1");
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        let id = c.refresh_all(&r.ops, ids.iter().map(|s| s.to_string()).collect()).unwrap();
        let t = finished(&r.ops, &id).await;
        assert_eq!((t.state, t.title.as_str(), t.summary.as_deref()), (TaskState::Ok, "Check for updates on 4 nodes", Some("Package lists are up to date")));
        assert!(t.warning.as_deref().is_some_and(|w| w.contains("cell2 (1)")), "{:?}", t.warning);
        let w = "W: Target Packages (main/binary-arm64/Packages) is configured multiple times in /etc/apt/sources.list.d/google-chrome.list:3 and /etc/apt/sources.list.d/google-chrome.sources:1";
        assert!(t.log.contains(&format!("cell2: {w}")), "{:#?}", t.log);
        assert!(t.log.contains(&"cell3: E: Could not get lock /var/lib/apt/lists/lock. It is held by process 2134 (apt-get)".to_string()), "{:#?}", t.log);
        assert!(t.log.iter().any(|l| l.starts_with("cell3: the package lists are locked") && l.ends_with("trying again in 0 s")), "{:#?}", t.log);
        for id in ids {
            let step = t.steps.iter().find(|s| s.label == format!("apt-get update on {id}")).unwrap_or_else(|| panic!("{id}: {:?}", t.steps));
            assert_eq!(step.state, StepState::Ok, "{id}");
        }
        // Each node's record: every line apt printed.
        let rec = |n: &str| c.summary(n).and_then(|s| s.refresh).unwrap();
        assert_eq!((rec("cell2").ok, rec("cell2").warnings.clone()), (true, vec![w.to_string()]));
        assert_eq!(rec("cell3").errors, vec!["E: Could not get lock /var/lib/apt/lists/lock. It is held by process 2134 (apt-get)".to_string(), "E: Unable to lock directory /var/lib/apt/lists/".to_string()]);
        assert!(rec("cell3").ok && rec("cell1").warnings.is_empty());
        // One sudo row per try: the exact command, as root, with the password on stdin only.
        for (f, id) in r.fakes.iter().zip(ids) {
            let tries = if id == "cell3" { 2 } else { 1 };
            assert_eq!(f.node.ran("sudo -k -S -p '' apt-get -o DPkg::Lock::Timeout=300 update").len(), tries, "{id}");
            let rows: Vec<_> = r.live.audit.entries().into_iter().filter(|e| e.node == id && e.command == REFRESH).collect();
            assert_eq!(rows.len(), tries, "{id}");
            assert!(rows.iter().all(|e| e.kind == "sudo" && e.op_id.as_deref() == Some(t.id.as_str())), "{rows:?}");
            assert!(f.node.raw.lock().unwrap().iter().all(|c| !c.contains("'pw'") && !c.contains(" pw ")));
            // Checked again afterwards (its lists' stamp moved), in the same op.
            assert_eq!(f.checks(), 1, "{id}");
        }
        assert_eq!(r.events.lock().unwrap().last().cloned(), Some(json!({ "nodes": ids })));
        r.live.stop();
    }

    #[tokio::test]
    async fn a_lock_held_past_the_last_try_fails_that_node_only() {
        let r = rig("locked", &["cell1", "cell2"]).await;
        r.fakes[1].write("apt/locked", "9");
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        let id = c.refresh_all(&r.ops, vec!["cell1".into(), "cell2".into()]).unwrap();
        let t = finished(&r.ops, &id).await;
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("Couldn't refresh the package lists on cell2")));
        assert_eq!(r.fakes[1].node.ran("apt-get -o DPkg::Lock::Timeout=300 update").len(), 4, "the first try and three more");
        let step = t.steps.iter().find(|s| s.label == "apt-get update on cell2").unwrap();
        assert_eq!((step.state, step.detail.as_deref()), (StepState::Failed, Some("apt-get update failed on cell2: E: Unable to lock directory /var/lib/apt/lists/")));
        assert_eq!(t.steps.iter().find(|s| s.label == "apt-get update on cell1").unwrap().state, StepState::Ok);
        assert!(!c.summary("cell2").unwrap().refresh.unwrap().ok);
        r.live.stop();
    }

    /// apt-get update cut off part-way: cell2's connection drops, cell3's apt is killed. Each
    /// says so in its step, the log and its record, and neither is tried again as a lock.
    #[tokio::test]
    async fn a_refresh_cut_off_part_way_says_why() {
        let ids = ["cell1", "cell2", "cell3"];
        let r = rig("cutoff", &ids).await;
        for f in &r.fakes[1..] {
            f.write("apt/update-delay", "4");
        }
        r.fakes[2].node.kill_child_with("KILL", "apt-get -o DPkg::Lock::Timeout=300 update");
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        let id = c.refresh_all(&r.ops, ids.iter().map(|s| s.to_string()).collect()).unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        r.fakes[1].node.drop_connections().await;
        let t = finished(&r.ops, &id).await;
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("Couldn't refresh the package lists on cell2, cell3")));
        let dropped = "The connection to cell2 dropped before apt-get update finished; the lists may be only partly refreshed";
        let killed = "apt-get update on cell3 was killed (SIGKILL); the lists may be only partly refreshed";
        for (id, why) in [("cell2", dropped), ("cell3", killed)] {
            let step = t.steps.iter().find(|s| s.label == format!("apt-get update on {id}")).unwrap();
            assert_eq!((step.state, step.detail.as_deref()), (StepState::Failed, Some(why)), "{id}");
            assert!(t.log.contains(&why.to_string()), "{:#?}", t.log);
            let rec = c.summary(id).and_then(|s| s.refresh).unwrap();
            assert_eq!((rec.ok, rec.errors), (false, vec![why.to_string()]), "{id}");
        }
        assert_eq!(t.steps.iter().find(|s| s.label == "apt-get update on cell1").unwrap().state, StepState::Ok);
        for f in &r.fakes {
            assert_eq!(f.node.ran("apt-get -o DPkg::Lock::Timeout=300 update").len(), 1, "tried once");
        }
        r.live.stop();
    }

    #[tokio::test]
    async fn cancel_stops_the_retries() {
        let r = rig("cancel", &["cell1"]).await;
        r.fakes[0].write("apt/locked", "9");
        let clock = Arc::new(AtomicU64::new(T0));
        let mut c = UpdateCache::new(None);
        c.clock = Some(clock.clone());
        c.releases_url = closed_url().await;
        c.lock_waits = vec![Duration::from_secs(30); 3];
        let c = Arc::new(c);
        let id = c.refresh_all(&r.ops, vec!["cell1".into()]).unwrap();
        for _ in 0..100 {
            if r.fakes[0].node.ran("apt-get").len() == 1 && r.ops.tasks.get(&id).is_some_and(|t| t.log.iter().any(|l| l.contains("trying again in 30 s"))) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        r.ops.tasks.cancel(&id).unwrap();
        let t = finished(&r.ops, &id).await;
        assert_eq!(t.state, TaskState::Cancelled);
        assert_eq!(r.fakes[0].node.ran("apt-get").len(), 1, "no try after Cancel");
        r.live.stop();
    }

    /// Firmware polkit won't show the cluster user is read as root only in "Check all nodes",
    /// never by the poller; the read stands until the next one.
    #[tokio::test]
    async fn firmware_that_needs_sudo_is_read_in_check_all_nodes_only() {
        let r = rig("fwsudo", &["cell1"]).await;
        let f = &r.fakes[0];
        f.write("fwupd/out", "Failed to get updates: Not authorized\n");
        f.write("fwupd/exit", "1");
        f.write("fwupd/root-out", FWUPD_JSON);
        f.write("fwupd/root-exit", "0");
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        c.tick(&r.ops, false).await;
        assert_eq!(c.summary("cell1").unwrap().firmware.map(|f| f.state), Some(FirmwareState::NeedsSudo));
        assert!(f.node.ran("sudo").is_empty(), "the poller never asks for sudo");
        let t = finished(&r.ops, &c.refresh_all(&r.ops, vec!["cell1".into()]).unwrap()).await;
        assert_eq!(t.state, TaskState::Ok, "{t:?}");
        assert_eq!(f.node.ran("sudo -k -S -p '' fwupdmgr get-updates --json --no-unreported-check --no-metadata-check").len(), 1);
        let fw = c.summary("cell1").unwrap().firmware.unwrap();
        assert_eq!((fw.state, fw.count, fw.detail.as_deref()), (FirmwareState::Available, 1, Some("Read with sudo (Check all nodes)")));
        // The next background check says "needs sudo" again; the sudo read stands.
        clock.store(T0 + 7 * HOUR, Ordering::SeqCst);
        c.tick(&r.ops, false).await;
        assert_eq!(c.summary("cell1").unwrap().firmware.map(|f| f.state), Some(FirmwareState::Available));
        assert_eq!(f.node.ran("sudo").len(), 2, "apt-get update and the one fwupd read");
        r.live.stop();
    }

    /// Under an enforced fence: the refresh's sudo rows carry apt-refresh, the fwupd read
    /// sudo-probe, and each is refused (nothing reaches the node) by a fence without its class.
    #[tokio::test]
    async fn check_all_nodes_under_a_fence() {
        use crate::live::fence_class::FenceClass::{AptRefresh, Service, SudoProbe};
        let r = rig("fence", &["cell1"]).await;
        let f = &r.fakes[0];
        f.write("fwupd/out", "Failed to get updates: Not authorized\n");
        f.write("fwupd/exit", "1");
        f.write("fwupd/root-out", FWUPD_JSON);
        f.write("fwupd/root-exit", "0");
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        // A fence without apt-refresh: refused before anything runs on the node.
        crate::live::tests::open_fence(&r.live, &temp("uc-fence-a"), &["cell1"], &[Service]);
        let t = finished(&r.ops, &c.refresh_all(&r.ops, vec!["cell1".into()]).unwrap()).await;
        assert_eq!(t.state, TaskState::Failed);
        assert!(f.node.ran("apt-get").is_empty() && f.node.ran("sudo").is_empty());
        let refused: Vec<_> = r.live.audit.entries().into_iter().filter(|e| e.title == "Refused" && e.command.contains("apt-get -o DPkg::Lock::Timeout=300 update")).collect();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(c.summary("cell1").unwrap().refresh.is_some_and(|r| !r.ok && r.errors.len() == 1));
        // apt-refresh only: the lists refresh; the fwupd read is refused and still needs sudo, saying why.
        crate::live::tests::open_fence(&r.live, &temp("uc-fence-b"), &["cell1"], &[AptRefresh]);
        let t = finished(&r.ops, &c.refresh_all(&r.ops, vec!["cell1".into()]).unwrap()).await;
        assert_eq!(t.state, TaskState::Ok, "{t:?}");
        let row = r.live.audit.entries().into_iter().find(|e| e.command == REFRESH && e.kind == "sudo").unwrap();
        assert_eq!((row.fence.as_deref(), row.state.as_str()), (Some("apt-refresh"), "ok"));
        assert!(f.node.ran("fwupdmgr get-updates --json --no-unreported-check --no-metadata-check").iter().all(|c| !c.starts_with("sudo")));
        let fw = c.summary("cell1").unwrap().firmware.unwrap();
        assert_eq!(fw.state, FirmwareState::NeedsSudo);
        assert!(fw.detail.as_deref().is_some_and(|d| d.starts_with("Couldn't read it with sudo: ")), "{fw:?}");
        // With sudo-probe as well, the read goes through as a sudo check.
        crate::live::tests::open_fence(&r.live, &temp("uc-fence-c"), &["cell1"], &[AptRefresh, SudoProbe]);
        let t = finished(&r.ops, &c.refresh_all(&r.ops, vec!["cell1".into()]).unwrap()).await;
        assert_eq!(t.state, TaskState::Ok, "{t:?}");
        let row = r.live.audit.entries().into_iter().find(|e| e.command == FWUPD_SUDO && e.kind == "sudo").unwrap();
        assert_eq!(row.fence.as_deref(), Some("sudo-probe"));
        assert_eq!(c.summary("cell1").unwrap().firmware.map(|f| f.state), Some(FirmwareState::Available));
        r.live.stop();
    }

    /// Look-only holds changes, not reads: the poller's rounds go through, and Check all nodes doesn't.
    #[tokio::test]
    async fn look_only_keeps_the_reads() {
        let r = rig("look", &["cell1"]).await;
        r.live.set_write_policy(crate::live::tests::TestPolicy::look_only());
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, closed_url().await);
        c.tick(&r.ops, false).await;
        clock.fetch_add(60_000, Ordering::SeqCst);
        c.tick(&r.ops, false).await;
        let s = c.summary("cell1").unwrap();
        assert_eq!((s.error.as_deref(), s.checked_ms), (None, Some(T0)));
        assert_eq!((r.fakes[0].checks(), r.fakes[0].stamp_reads()), (1, 1));
        assert!(r.live.audit.entries().iter().all(|e| e.title != "Refused"), "{:?}", r.live.audit.entries());
        let t = finished(&r.ops, &c.refresh_all(&r.ops, vec!["cell1".into()]).unwrap()).await;
        assert_eq!(t.state, TaskState::Failed);
        assert!(r.fakes[0].node.ran("apt-get").is_empty());
        r.live.stop();
    }

    #[tokio::test]
    async fn ollamas_latest_comes_from_github_once_per_six_hours() {
        let (url, heads) = github(200, r#"{"url":"https://api.github.com/repos/ollama/ollama/releases/1","tag_name":"v0.22.1","prerelease":false}"#).await;
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(None, &clock, url);
        assert_eq!(c.ollama_latest().await.as_deref(), Some("0.22.1"));
        clock.fetch_add(5 * HOUR, Ordering::SeqCst);
        assert_eq!(c.ollama_latest().await.as_deref(), Some("0.22.1"));
        assert_eq!(heads.lock().unwrap().len(), 1, "kept 6 h");
        clock.fetch_add(HOUR, Ordering::SeqCst);
        c.ollama_latest().await;
        assert_eq!(heads.lock().unwrap().len(), 2);
        // GitHub down, refusing, or answering something else: unknown, asked again after 30 min.
        let (url, heads) = github(403, r#"{"message":"API rate limit exceeded"}"#).await;
        let c = cache(None, &clock, url);
        assert_eq!(c.ollama_latest().await, None);
        clock.fetch_add(20 * 60_000, Ordering::SeqCst);
        assert_eq!(c.ollama_latest().await, None);
        assert_eq!(heads.lock().unwrap().len(), 1);
        clock.fetch_add(10 * 60_000, Ordering::SeqCst);
        c.ollama_latest().await;
        assert_eq!(heads.lock().unwrap().len(), 2);
        assert_eq!(cache(None, &clock, closed_url().await).ollama_latest().await, None);
        let (url, _) = github(200, r#"{"name":"no tag"}"#).await;
        assert_eq!(cache(None, &clock, url).ollama_latest().await, None);
    }

    /// The check on fake Sparks built from the Live tour's captures (fixtures/doctor): their own
    /// apt list and /etc/dgx-release, with spark_tree.py's marked stand-ins for fwupd, snap and
    /// Ollama (cell1 and cell3 only).
    #[tokio::test]
    async fn the_check_on_fake_sparks_from_the_captures() {
        let ids = ["cell1", "gx10-0000"];
        let mut fakes = Vec::new();
        let mut ports = Vec::new();
        for id in ids {
            let (node, port) = fake_spark(&temp(&format!("uc-spark-{id}")), id).await;
            ports.push((id, port));
            fakes.push(Fake { node, port });
        }
        let (live, inventory) = connect_sparks(&temp("uc-spark-app"), &ports).await;
        let r = rig_on(live, inventory, fakes);
        let clock = Arc::new(AtomicU64::new(T0));
        let (url, _) = github(200, r#"{"tag_name":"v0.21.0"}"#).await;
        let c = cache(None, &clock, url);
        c.tick(&r.ops, false).await;
        let cell1 = c.summary("cell1").unwrap();
        assert_eq!(cell1.error, None, "{cell1:?}");
        let f = cell1.facts().unwrap();
        assert_eq!((f.pending, f.security, f.firmware, f.ollama_newer), (4, 3, Some(0), false));
        assert_eq!(cell1.dgx.as_ref().map(|d| (d.build.as_deref(), d.ota.as_deref(), d.ota_date.as_deref())), Some((Some("7.2.3"), Some("7.6.0"), Some("Sun Oct  4 11:41:51 EDT 2026"))));
        assert_eq!(cell1.ollama.and_then(|o| o.installed), Some("0.21.0".into()));
        assert_eq!(cell1.snaps, Some(Vec::new()));
        let gx10 = c.summary("gx10-0000").unwrap();
        assert_eq!(gx10.facts().map(|f| (f.pending, f.security)), Some((2, 1)));
        assert_eq!(gx10.dgx.and_then(|d| d.ota_date), Some("Thu Oct 15 10:29:19 EDT 2026".into()));
        assert_eq!(gx10.ollama, None, "no Ollama on gx10-0000");
        r.live.stop();
    }

    #[tokio::test]
    async fn demo_has_apt_only_and_nothing_is_saved() {
        let dir = temp("uc-demo");
        let d = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
        let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
        let ops = Ops::with_poll(Live::new(vpn, dir.join("kh.json"), || Ok(None)), d.clone(), Tasks::new(None), Duration::from_millis(1));
        let src = d.clone();
        ops.set_snapshot_source(move || Some(src.lock().unwrap().snapshot()));
        let s = demo_summaries(&ops, &["cell1".into()]).await;
        assert_eq!(s.len(), 1);
        assert!(s[0].demo && s[0].apt.is_some() && s[0].error.is_none());
        assert!(s[0].dgx.is_none() && s[0].firmware.is_none() && s[0].ollama.is_none() && s[0].snaps.is_none());
        // The poller does nothing in Demo, and writes no file.
        let path = dir.join("updates-cache.json");
        let clock = Arc::new(AtomicU64::new(T0));
        let c = cache(Some(path.clone()), &clock, closed_url().await);
        assert!(c.tick(&ops, false).await.is_empty());
        assert!(!Path::new(&path).exists());
        let mut snap = ops.cluster().unwrap();
        assert!(snap.demo);
        c.overlay_onto(&mut snap);
        assert!(snap.nodes.iter().all(|n| n.updates.is_none()));
    }
}
