//! Boots: which boot each node is on, how the boots before it ended, and the node-rebooted
//! rule. A GB10 has no BMC, so the node's own journal is the only record of a restart.
//!
//! Each Live snapshot carries the node's bootId (/proc/sys/kernel/random/boot_id). `observe`
//! keeps the last one per node in boots.json (the app's data folder), so a SparkBox restart
//! loses nothing. A bootId change outside a maintenance window SparkBox opened (no
//! `maintenance`, no `power_state`, no window in maintenance.rs) is a restart nobody asked
//! SparkBox for: rule node-rebooted, a warning, critical at a second one within an hour on that
//! node or another node's within 5 minutes. It fires for an hour after the boot, then clears.
//! A first sighting never raises: a node with no saved bootId, and boots found in the journal.
//! A restart counts as SparkBox's only when SparkBox saw the node go down while its window was
//! open (status not Online), and the new boot showed while the window was still open or within
//! the restart's expected time and grace of it: a window whose restart never happened (the node
//! kept answering) doesn't make a crash soon after it planned. A restart SparkBox didn't watch
//! (it wasn't running) counts when the window is open and the new boot began after it opened.
//!
//! How a boot ended is read from the node's journal, as the cluster user (in adm, no sudo),
//! as SparkBox's own probe: `--list-boots`, then each boot's last warnings, PID 1 lines and
//! journald lines. It ended cleanly when systemd reached a reboot, power-off or shutdown
//! target ("Shutting down.", "Journal stopped" too) within 2 minutes of the boot's last entry;
//! otherwise it ended without a shutdown (power loss, hang or watchdog). A "Journal stopped"
//! that a "Journal started" follows was journald restarting (an upgrade), not a shutdown.
//! That's read on a node's first connection in each run of SparkBox (every earlier boot of the
//! last 14 days not yet judged by today's rule, at most 10) and whenever its bootId changes
//! (the boot before, `-b -1`).
//!
//! Live only: Demo has no journal ("Available with Live data").

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::kernel::BootMark;
use super::rule::{EntityKind, Op, Rule, RuleKind};
use super::rules::{cond, first_lower, reporting, Condition, Ctx};
use super::{Category, Entity, Severity};
use crate::live::{audit, Live};
use crate::model::{ClusterSnapshot, LinkStatus, NodeSnapshot};

pub const RULE: &str = "node-rebooted";
/// The alert fires this long after the boot (the rule's window), then clears.
pub const FIRES_MS: u64 = 60 * 60_000;
/// A second unplanned restart of one node within this long is critical.
pub const AGAIN_MS: u64 = 60 * 60_000;
/// Two nodes restarting on their own within this long of each other is critical.
pub const TOGETHER_MS: u64 = 5 * 60_000;
/// The header says "restarted on its own" for a boot this recent.
pub const RECENT_MS: u64 = 24 * 3_600_000;
/// Boots kept per node.
const KEEP: usize = 30;
/// Earlier boots read from the journal on a first connection: at most this many, this recent.
const JOURNAL_BOOTS: usize = 10;
const JOURNAL_MS: u64 = 14 * 86_400_000;
/// A journal read that failed is tried again this much later.
const RETRY_MS: u64 = 5 * 60_000;
const LIMIT: Duration = Duration::from_secs(20);
/// A boot that ends this soon after a window SparkBox opened was last seen on it is SparkBox's
/// restart: the restart's expected time and node-reboot's shipped grace (2 min).
const PLANNED_FOR_MS: u64 = super::maintenance::REBOOT_EXPECT_MS + 2 * 60_000;
/// Still answering on that boot this long after the window went: the restart never happened
/// (node_power gives up a minute after sending it).
const HELD_MS: u64 = 2 * 60_000;
/// A shutdown line counts only this close to the boot's last entry.
const ENDED_NEAR_MS: u64 = 120_000;
/// `judge`'s rule, saved with each verdict: 2 counts a shutdown line only near the boot's end and
/// not a journald restart. Round 1's took any of them; its verdicts were saved without one (0).
const JUDGE_V: u8 = 2;

/// The journal reads, exactly as they run (`which` is `-1` or a 32-digit boot id).
pub const LIST_BOOTS: &str = "journalctl --list-boots -o json --no-pager";
/// Between the three answers of `boot_command`.
const SPLIT: &str = "@@sparkbox-boot@@";

pub fn boot_command(which: &str) -> String {
    let b = crate::live::sh_quote(which);
    [
        format!("journalctl -b {b} -n 80 -p warning -o json --no-pager --output-fields=MESSAGE,PRIORITY,SYSLOG_IDENTIFIER,__REALTIME_TIMESTAMP"),
        format!("journalctl -b {b} -n 50 -o json --no-pager _PID=1 --output-fields=MESSAGE,__REALTIME_TIMESTAMP"),
        format!("journalctl -b {b} -n 20 -o json --no-pager -u systemd-journald --output-fields=MESSAGE,__REALTIME_TIMESTAMP"),
    ]
    .join(&format!("; echo {SPLIT}; "))
}

/// How a boot ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ended {
    /// systemd reached a reboot, power-off or shutdown target.
    Clean,
    /// No shutdown in its journal: power loss, a hang or the watchdog.
    Unclean,
    /// Its journal is gone.
    Unknown,
}

/// How SparkBox knows of a boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// SparkBox saw the bootId change to it.
    Live,
    /// Found in the node's journal.
    Journal,
    /// Already running when SparkBox first connected, and not (yet) found in the journal.
    First,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Boot {
    /// 32 hex digits, the journal's form (/proc writes it with dashes).
    pub boot_id: String,
    pub start_ms: u64,
    /// Its journal's last entry; None while it's running.
    #[serde(default)]
    pub end_ms: Option<u64>,
    /// None while it's running, or until its journal has been read.
    #[serde(default)]
    pub ended: Option<Ended>,
    /// Its last warnings (unclean) or shutdown lines (clean), oldest first.
    #[serde(default)]
    pub last_lines: Vec<String>,
    pub source: Source,
    /// SparkBox restarted the node into this boot (a maintenance window or power action).
    #[serde(default)]
    pub planned: bool,
    /// Which rule of `judge` said how it ended (JUDGE_V); one judged by an older rule is read again.
    #[serde(default)]
    pub judge_v: u8,
}

impl Boot {
    /// The node restarted into it without SparkBox, as SparkBox saw.
    pub fn unplanned(&self) -> bool {
        self.source == Source::Live && !self.planned
    }
}

/// One node's boots, as saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeBoots {
    #[serde(default)]
    pub last_boot_id: Option<String>,
    /// Newest first.
    #[serde(default)]
    pub boots: Vec<Boot>,
    /// `--list-boots` listed only the running boot: the journal isn't kept across boots.
    #[serde(default)]
    pub current_only: bool,
    #[serde(default)]
    pub read_ms: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

impl NodeBoots {
    fn boot_mut(&mut self, id: &str) -> Option<&mut Boot> {
        self.boots.iter_mut().find(|b| b.boot_id == id)
    }

    fn sort(&mut self) {
        self.boots.sort_by_key(|b| std::cmp::Reverse(b.start_ms));
        self.boots.truncate(KEEP);
    }
}

/// What to read from a node's journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Its first connection in this run: every earlier boot not read yet.
    First,
    /// Its bootId changed: the boot before.
    Changed,
}

#[derive(Default)]
struct Store {
    nodes: BTreeMap<String, NodeBoots>,
    /// Unplanned boots seen in this run: (node, boot id). Only these raise node-rebooted; one
    /// from an earlier run only keeps the alert it already has.
    fresh: BTreeSet<(String, String)>,
    /// Nodes read (or being read) in this run, and those being read now.
    read_run: BTreeSet<String>,
    reading: BTreeSet<String>,
    retry_at: BTreeMap<String, u64>,
    /// Per node, the boot it ran while a window SparkBox opened was open, when that was last
    /// seen, and whether the node was then down (not Online, on the latest frame under the
    /// window): a restart away from that boot is SparkBox's only once it was seen go down under
    /// the window, even if the window closed before the new boot showed.
    windowed: BTreeMap<String, Windowed>,
    /// Nodes that called for a read while one was under way: read again once it's done (it
    /// began before the restart, so it can't have judged the boot before).
    again: BTreeSet<String>,
}

/// A node's boot under a window (`Store::windowed`).
#[derive(Clone, Debug, PartialEq)]
struct Windowed {
    boot: String,
    seen_ms: u64,
    down: bool,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    nodes: BTreeMap::new(),
    fresh: BTreeSet::new(),
    read_run: BTreeSet::new(),
    reading: BTreeSet::new(),
    retry_at: BTreeMap::new(),
    windowed: BTreeMap::new(),
    again: BTreeSet::new(),
});
/// Where boots.json is kept; unset in tests and before `start`.
static DIR: OnceLock<PathBuf> = OnceLock::new();

fn now_ms() -> u64 {
    crate::history::now_ms()
}

/// "bb600f35a1aa4286a65e6b2f30f33b9c" from either form.
pub fn norm_id(id: &str) -> String {
    id.trim().chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase()
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

// ---- The journal

/// One line of `journalctl --list-boots -o json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    /// 0 is the running boot, -1 the one before…
    pub index: i64,
    pub boot_id: String,
    pub first_ms: u64,
    pub last_ms: u64,
}

fn usec(v: &serde_json::Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// systemd prints the list as one JSON array (json-pretty too); one object per line is read as well.
pub fn parse_list_boots(text: &str) -> Option<Vec<Listed>> {
    let t = text.trim();
    let items: Vec<serde_json::Value> = match serde_json::from_str::<serde_json::Value>(t) {
        Ok(serde_json::Value::Array(a)) => a,
        Ok(v @ serde_json::Value::Object(_)) => vec![v],
        _ => t.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l.trim()).ok()).collect::<Option<Vec<_>>>()?,
    };
    let mut out: Vec<Listed> = items
        .iter()
        .filter_map(|v| {
            let id = norm_id(v.get("boot_id")?.as_str()?);
            valid_id(&id).then_some(())?;
            Some(Listed { index: v.get("index")?.as_i64()?, boot_id: id, first_ms: usec(v.get("first_entry")?)? / 1000, last_ms: usec(v.get("last_entry")?)? / 1000 })
        })
        .collect();
    if out.is_empty() {
        return None;
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.index));
    Some(out)
}

/// One `journalctl -o json` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub ts_ms: u64,
    pub message: String,
    pub priority: Option<u8>,
    pub ident: Option<String>,
}

/// One entry per line; MESSAGE is a string, or an array of bytes when it isn't UTF-8.
pub fn parse_entries(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .filter_map(|v| {
            let message = match v.get("MESSAGE")? {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(bytes) => String::from_utf8_lossy(&bytes.iter().filter_map(|b| b.as_u64().map(|b| b as u8)).collect::<Vec<u8>>()).into_owned(),
                _ => return None,
            };
            Some(Entry {
                ts_ms: v.get("__REALTIME_TIMESTAMP").and_then(usec).unwrap_or(0) / 1000,
                message,
                priority: v.get("PRIORITY").and_then(usec).map(|p| p as u8),
                ident: v.get("SYSLOG_IDENTIFIER").and_then(|s| s.as_str()).map(String::from),
            })
        })
        .collect()
}

/// A line systemd or journald writes only while shutting down.
pub fn is_shutdown_line(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    (m.contains("reached target") && ["reboot", "power-off", "power off", "poweroff", "shutdown", "halt", "kexec"].iter().any(|w| m.contains(w)))
        || m.starts_with("shutting down")
        || m.contains("journal stopped")
}

/// The words a journal sends when it no longer has that boot.
fn journal_gone(stderr: &str) -> bool {
    let e = stderr.to_ascii_lowercase();
    ["is not available", "no such boot", "no journal boot entry", "failed to look up boot"].iter().any(|w| e.contains(w))
}

/// How a boot ended, from `boot_command`'s answer, with its last lines. `last_ms`: the boot's
/// last entry as `--list-boots` gave it (None: the newest line read). A shutdown line counts
/// only near that end: journald writes "Journal stopped" whenever it's restarted too (an
/// upgrade of systemd), maybe days before a crash.
pub fn judge(stdout: &str, stderr: &str, last_ms: Option<u64>) -> (Ended, Vec<String>) {
    let parts: Vec<&str> = stdout.split(&format!("{SPLIT}\n")).collect();
    if parts.len() < 3 {
        return (Ended::Unknown, Vec::new());
    }
    // PID 1's and journald's lines don't carry SYSLOG_IDENTIFIER (not asked for): they're named here.
    let named = |text: &str, who: &str| -> Vec<Entry> { parse_entries(text).into_iter().map(|e| Entry { ident: e.ident.or_else(|| Some(who.to_string())), ..e }).collect() };
    let (warnings, pid1, journald) = (parse_entries(parts[0]), named(parts[1], "systemd"), named(parts[2], "systemd-journald"));
    if pid1.is_empty() && journald.is_empty() && (warnings.is_empty() || journal_gone(stderr)) {
        return (Ended::Unknown, Vec::new());
    }
    let mut ends: Vec<&Entry> = pid1.iter().chain(journald.iter()).collect();
    ends.sort_by_key(|e| e.ts_ms);
    let last = last_ms.or_else(|| warnings.iter().chain(ends.iter().copied()).map(|e| e.ts_ms).max()).unwrap_or(0);
    let said = |e: &Entry, words: &str| e.message.to_ascii_lowercase().contains(words);
    let shut_down = ends.iter().enumerate().any(|(i, e)| {
        let restarted = said(e, "journal stopped") && ends[i + 1..].iter().any(|later| said(later, "journal started"));
        is_shutdown_line(&e.message) && !restarted && last.saturating_sub(e.ts_ms) <= ENDED_NEAR_MS
    });
    if shut_down {
        let lines = ends.iter().rev().take(5).rev().map(|e| line(e)).collect();
        return (Ended::Clean, lines);
    }
    let lines: Vec<String> = if warnings.is_empty() { ends.iter().rev().take(5).rev().map(|e| line(e)).collect() } else { warnings.iter().rev().take(12).rev().map(line).collect() };
    (Ended::Unclean, lines)
}

/// "17:44:58 kernel: watchdog: BUG: soft lockup".
fn line(e: &Entry) -> String {
    format!("{} {}: {}", clock_s(e.ts_ms), e.ident.as_deref().unwrap_or("kernel"), e.message.trim())
}

/// The Mac's local "17:44:58".
fn clock_s(ms: u64) -> String {
    let s = (ms / 1000) as i64 + super::maintenance::local_offset_s();
    let day = s.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

/// The Mac's local "6:08 PM".
pub fn clock(ms: u64) -> String {
    let s = (ms / 1000) as i64 + super::maintenance::local_offset_s();
    let day = s.rem_euclid(86_400);
    let (h, m) = (day / 3600, (day % 3600) / 60);
    format!("{}:{m:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
}

/// What one read of a node's journal found.
#[derive(Clone, Debug, Default)]
pub struct JournalRead {
    pub listed: Vec<Listed>,
    /// (boot id, how it ended, its last lines).
    pub ended: Vec<(String, Ended, Vec<String>)>,
    pub error: Option<String>,
}

fn why(o: &crate::live::ssh::Output, what: &str) -> String {
    let last = o.stderr.lines().rev().find(|l| !l.trim().is_empty()).map(str::trim).unwrap_or("");
    if last.is_empty() {
        what.to_string()
    } else {
        format!("{what}: {last}")
    }
}

/// Reads `node`'s journal: the list of boots, then how the ones `mode` asks for ended (one
/// command per boot, three reads in it). `known` are boots already judged. Reads only.
pub async fn read_journal(live: &Live, node: &str, mode: Mode, known: &BTreeSet<String>, now: u64) -> JournalRead {
    let mut out = JournalRead::default();
    let list = match audit::as_probe(live.run(node, LIST_BOOTS, None, LIMIT)).await {
        Ok(o) => o,
        Err(e) => {
            out.error = Some(e);
            return out;
        }
    };
    let Some(listed) = parse_list_boots(&list.stdout) else {
        out.error = Some(why(&list, "The journal's list of boots couldn't be read"));
        return out;
    };
    out.listed = listed;
    let targets: Vec<(String, String)> = match mode {
        Mode::Changed => out.listed.iter().filter(|b| b.index == -1).map(|b| ("-1".to_string(), b.boot_id.clone())).collect(),
        Mode::First => out
            .listed
            .iter()
            .filter(|b| b.index < 0 && now.saturating_sub(b.last_ms) <= JOURNAL_MS && !known.contains(&b.boot_id))
            .take(JOURNAL_BOOTS)
            .map(|b| (b.boot_id.clone(), b.boot_id.clone()))
            .collect(),
    };
    for (which, id) in targets {
        let last_ms = out.listed.iter().find(|b| b.boot_id == id).map(|b| b.last_ms);
        match audit::as_probe(live.run(node, &boot_command(&which), None, LIMIT)).await {
            Ok(o) => {
                let (ended, lines) = judge(&o.stdout, &o.stderr, last_ms);
                out.ended.push((id, ended, lines));
            }
            Err(e) => {
                out.error = Some(e);
                break;
            }
        }
    }
    out
}

// ---- The store

impl Store {
    /// Takes in one snapshot; returns the journal reads it calls for, and whether anything to
    /// save changed. `window`: when the maintenance window SparkBox has open on the node opened.
    fn observe(&mut self, snap: &ClusterSnapshot, now: u64, window: &dyn Fn(&str) -> Option<u64>) -> (Vec<(String, Mode)>, bool) {
        let mut reads = Vec::new();
        let mut dirty = false;
        if snap.demo {
            return (reads, dirty);
        }
        for n in &snap.nodes {
            // When SparkBox's window on the node opened (a power action without one yet: now).
            let opened = n.maintenance.as_ref().map(|m| m.started_ms).or_else(|| window(&n.id)).or(n.power_state.as_ref().map(|_| now));
            let frame_id = n.boot_id.as_deref().map(norm_id).filter(|i| valid_id(i));
            // Under the window, on the boot it had (a frame already on a new one isn't noted).
            let new_boot = reporting(n) && frame_id.as_ref().is_some_and(|i| self.nodes.get(&n.id).and_then(|nb| nb.last_boot_id.as_ref()).is_some_and(|l| l != i));
            if let (Some(at), false) = (opened, new_boot) {
                self.note_window(&n.id, at, now, n.status != LinkStatus::Online);
            }
            if !reporting(n) {
                continue;
            }
            let Some(id) = frame_id else { continue };
            let start = now.saturating_sub(n.system.uptime_s * 1000);
            let nb = self.nodes.entry(n.id.clone()).or_default();
            let mut mode = None;
            match nb.last_boot_id.clone() {
                None => {
                    // First sighting: where SparkBox came in, never a restart.
                    nb.last_boot_id = Some(id.clone());
                    if nb.boot_mut(&id).is_none() {
                        nb.boots.push(Boot { boot_id: id.clone(), start_ms: start, end_ms: None, ended: None, last_lines: Vec::new(), source: Source::First, planned: false, judge_v: 0 });
                        nb.sort();
                    }
                    dirty = true;
                }
                Some(last) if last == id => {
                    if opened.is_none() && self.windowed.get(&n.id).is_some_and(|w| w.boot == id && now.saturating_sub(w.seen_ms) > HELD_MS) {
                        self.windowed.remove(&n.id);
                    }
                }
                Some(last) => {
                    // A restart. The boot before ends when its journal says (the read this starts).
                    let planned = match self.windowed.remove(&n.id).filter(|w| w.boot == last) {
                        // Watched on that boot under the window: SparkBox's once it saw the node
                        // go down there, while the window is open or soon after it closed.
                        Some(w) => w.down && (opened.is_some() || now.saturating_sub(w.seen_ms) <= PLANNED_FOR_MS),
                        // Not watched under it (SparkBox wasn't running): the window open now, and
                        // a boot that began after it opened, or the power state SparkBox set.
                        None => n.power_state.is_some() || opened.is_some_and(|at| start >= at),
                    };
                    match nb.boot_mut(&id) {
                        Some(b) => b.planned |= planned,
                        None => nb.boots.push(Boot { boot_id: id.clone(), start_ms: start, end_ms: None, ended: None, last_lines: Vec::new(), source: Source::Live, planned, judge_v: 0 }),
                    }
                    nb.sort();
                    nb.last_boot_id = Some(id.clone());
                    if !planned {
                        self.fresh.insert((n.id.clone(), id.clone()));
                    }
                    mode = Some(Mode::Changed);
                    dirty = true;
                }
            }
            let retry = self.retry_at.get(&n.id).is_some_and(|t| now >= *t);
            if !self.read_run.contains(&n.id) || retry || self.again.contains(&n.id) {
                mode = Some(Mode::First);
            }
            if let Some(m) = mode {
                if self.reading.insert(n.id.clone()) {
                    self.read_run.insert(n.id.clone());
                    self.retry_at.remove(&n.id);
                    self.again.remove(&n.id);
                    reads.push((n.id.clone(), m));
                } else {
                    self.again.insert(n.id.clone());
                }
            }
        }
        (reads, dirty)
    }

    /// Notes the boot `node` runs while a window opened at `opened` is open on it, and whether
    /// this frame has it down: only a boot that began before the window (the one being
    /// restarted, not the one it restarted into).
    fn note_window(&mut self, node: &str, opened: u64, now: u64, down: bool) {
        let Some(nb) = self.nodes.get(node) else { return };
        let Some(cur) = nb.last_boot_id.as_ref() else { return };
        if nb.boots.iter().any(|b| &b.boot_id == cur && b.start_ms <= opened) {
            self.windowed.insert(node.to_string(), Windowed { boot: cur.clone(), seen_ms: now, down });
        }
    }

    /// Takes in what a journal read found.
    fn apply(&mut self, node: &str, read: JournalRead, now: u64) {
        self.reading.remove(node);
        let nb = self.nodes.entry(node.to_string()).or_default();
        if read.listed.is_empty() {
            nb.error = read.error.or_else(|| Some("The journal didn't answer".into()));
            self.retry_at.insert(node.to_string(), now + RETRY_MS);
            return;
        }
        nb.error = read.error;
        if nb.error.is_some() {
            self.retry_at.insert(node.to_string(), now + RETRY_MS);
        }
        nb.read_ms = Some(now);
        nb.current_only = read.listed.len() == 1;
        for l in &read.listed {
            let end = (l.index < 0).then_some(l.last_ms);
            match nb.boot_mut(&l.boot_id) {
                Some(b) => {
                    b.start_ms = l.first_ms;
                    if end.is_some() {
                        b.end_ms = end;
                    }
                    if b.source == Source::First {
                        b.source = Source::Journal;
                    }
                }
                None => nb.boots.push(Boot { boot_id: l.boot_id.clone(), start_ms: l.first_ms, end_ms: end, ended: None, last_lines: Vec::new(), source: Source::Journal, planned: false, judge_v: 0 }),
            }
        }
        // A finished boot the journal no longer lists: how it ended is gone with it.
        let listed: BTreeSet<&str> = read.listed.iter().map(|l| l.boot_id.as_str()).collect();
        let current = nb.last_boot_id.clone();
        for b in nb.boots.iter_mut().filter(|b| b.ended.is_none() && Some(&b.boot_id) != current.as_ref() && !listed.contains(b.boot_id.as_str())) {
            b.ended = Some(Ended::Unknown);
        }
        for (id, ended, lines) in read.ended {
            if let Some(b) = nb.boot_mut(&id) {
                b.ended = Some(ended);
                b.last_lines = lines;
                b.judge_v = JUDGE_V;
            }
        }
        nb.sort();
    }

    /// Boots already judged on `node`, by today's rule: not read again.
    fn known(&self, node: &str) -> BTreeSet<String> {
        self.nodes.get(node).map(|nb| nb.boots.iter().filter(|b| b.ended.is_some() && b.judge_v == JUDGE_V).map(|b| b.boot_id.clone()).collect()).unwrap_or_default()
    }
}

/// Takes in a snapshot without starting any read (tests; `observe` starts them).
pub fn observe_at(snap: &ClusterSnapshot, now: u64) -> Vec<(String, Mode)> {
    let window = |node: &str| super::maintenance::opened_ms(snap.demo, node);
    let (reads, dirty) = STORE.lock().unwrap().observe(snap, now, &window);
    if dirty {
        save();
    }
    reads
}

/// Called from health::on_publish for every snapshot (after kernel::stamp, before the
/// maintenance windows close): notes bootIds and starts the journal reads they call for.
pub fn observe(app: &AppHandle, snap: &ClusterSnapshot) {
    if DIR.get().is_none() {
        return; // not started (boots.json not loaded yet)
    }
    let reads = observe_at(snap, snap.ts_ms);
    if reads.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<crate::AppState>() else { return };
    let live = state.live.clone();
    for (node, mode) in reads {
        let live = live.clone();
        tauri::async_runtime::spawn(async move { read_and_apply(&live, &node, mode).await });
    }
}

async fn read_and_apply(live: &Live, node: &str, mode: Mode) {
    let known = STORE.lock().unwrap().known(node);
    let read = read_journal(live, node, mode, &known, now_ms()).await;
    STORE.lock().unwrap().apply(node, read, now_ms());
    save();
}

/// Loads boots.json from the app's data folder and saves there from now on (lib.rs setup).
pub fn start(app: &AppHandle) {
    let Ok(dir) = app.path().app_data_dir() else { return };
    load_from(&dir);
    let _ = DIR.set(dir);
}

fn path_in(dir: &Path) -> PathBuf {
    dir.join("boots.json")
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    #[serde(default)]
    nodes: BTreeMap<String, NodeBoots>,
}

fn load_from(dir: &Path) {
    let Ok(Some(saved)) = crate::util::load_json_or_quarantine::<Saved>(&path_in(dir)) else { return };
    let mut store = STORE.lock().unwrap();
    for (node, nb) in saved.nodes {
        store.nodes.entry(node).or_insert(nb);
    }
}

fn save() {
    if let Some(dir) = DIR.get() {
        save_to(dir);
    }
}

fn save_to(dir: &Path) {
    static LAST: Mutex<String> = Mutex::new(String::new());
    let text = serde_json::to_string_pretty(&Saved { nodes: STORE.lock().unwrap().nodes.clone() }).unwrap_or_default();
    let mut last = LAST.lock().unwrap();
    if *last == text {
        return;
    }
    if let Err(e) = crate::util::write_atomic(&path_in(dir), text.as_bytes()) {
        eprintln!("boots: couldn't save boots.json: {e}");
        return;
    }
    *last = text;
}

// ---- The rule

/// node-rebooted, in the shipped rules (rule.rs extra_defaults). The Mac judges it; the
/// agents don't (yet: P7.16).
pub fn default_rule() -> Rule {
    Rule {
        id: RULE.into(),
        area: "node".into(),
        title: "Restarted on its own".into(),
        short: "Restarted on its own".into(),
        entity: EntityKind::Node,
        kind: RuleKind::Coded,
        metric: "bootId".into(),
        op: Op::Gt,
        warn: None,
        crit: None,
        clear: None,
        for_s: 0,
        crit_for_s: None,
        clear_s: 0,
        enabled: true,
        overrides: BTreeMap::new(),
        message: "Restarted on its own".into(),
        unit: String::new(),
        window_s: Some(FIRES_MS / 1000),
        notify: true,
        help: "The node's boot changed while SparkBox hadn't restarted or shut it down. A warning for an hour; critical at a second restart within an hour, or two nodes within 5 minutes. The detail says how the boot before ended, from the node's journal.".into(),
        counters: Vec::new(),
        mac_only: true,
    }
}

/// "2nd", "3rd", "11th".
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// How the boot before `boot` ended, in a sentence, with its last lines below.
pub fn previous_words(nb: &NodeBoots, boot: &Boot) -> String {
    let before = nb.boots.iter().filter(|b| b.start_ms < boot.start_ms).max_by_key(|b| b.start_ms);
    let Some(b) = before else {
        return if nb.current_only { "This node's journal keeps only the current boot, so how the last one ended isn't known.".into() } else { "Reading how the boot before ended from the node's journal…".into() };
    };
    let at = b.end_ms.map(|e| format!(" at {}", clock(e))).unwrap_or_default();
    let lines = if b.last_lines.is_empty() { String::new() } else { format!("\n{}", b.last_lines.join("\n")) };
    match b.ended {
        Some(Ended::Clean) => format!("The boot before ended with a clean shutdown{at}.{lines}"),
        Some(Ended::Unclean) => format!("The boot before ended without a shutdown{at} (power loss, hang or watchdog). Its last warnings:{lines}"),
        Some(Ended::Unknown) => "How the boot before ended isn't known: the node's journal no longer has it.".into(),
        None if nb.error.is_some() => format!("How the boot before ended couldn't be read: {}", nb.error.as_deref().unwrap_or("")),
        None => "Reading how the boot before ended from the node's journal…".into(),
    }
}

/// node-rebooted for each node that restarted on its own in the last hour (rules.rs evaluate).
pub fn conditions(ctx: &Ctx) -> Vec<Condition> {
    let mut out = Vec::new();
    if ctx.snap.demo {
        return out;
    }
    let store = STORE.lock().unwrap();
    // Every node's unplanned restarts (newest first), for "2 nodes within 5 minutes".
    let restarts = |id: &str| -> Vec<&Boot> { store.nodes.get(id).map(|nb| nb.boots.iter().filter(|b| b.unplanned()).collect()).unwrap_or_default() };
    for n in &ctx.snap.nodes {
        let th = ctx.rules.th(RULE, Some(&n.id));
        if !th.enabled {
            continue;
        }
        let Some(nb) = store.nodes.get(&n.id) else { continue };
        let mine = restarts(&n.id);
        let Some(last) = mine.first() else { continue };
        let window = if th.window_ms > 0 { th.window_ms } else { FIRES_MS };
        if ctx.now.saturating_sub(last.start_ms) >= window {
            continue;
        }
        let key = format!("{RULE}:node:{}", n.id);
        // Seen in an earlier run: it keeps the alert it has (restored at launch), never raises one.
        if !store.fresh.contains(&(n.id.clone(), last.boot_id.clone())) && !ctx.active.contains(&key) {
            continue;
        }
        let hour: Vec<&&Boot> = mine.iter().filter(|b| last.start_ms.saturating_sub(b.start_ms) <= AGAIN_MS).collect();
        let others: Vec<&NodeSnapshot> = ctx
            .snap
            .nodes
            .iter()
            .filter(|o| o.id != n.id && restarts(&o.id).iter().any(|b| b.start_ms.abs_diff(last.start_ms) <= TOGETHER_MS))
            .collect();
        let mut message = format!("Restarted on its own at {}", clock(last.start_ms));
        if hour.len() >= 2 {
            let first = hour.iter().map(|b| b.start_ms).min().unwrap_or(last.start_ms);
            message.push_str(&format!(" · {} time in {} min", ordinal(hour.len()), (last.start_ms - first).div_ceil(60_000).max(1)));
        }
        if !others.is_empty() {
            message.push_str(&format!(" · {} too", others.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", ")));
        }
        let severity = if hour.len() >= 2 || !others.is_empty() { Severity::Crit } else { Severity::Warn };
        let entity = Entity { kind: "node".into(), id: n.id.clone(), node: Some(n.id.clone()), switch: None, label: n.name.clone() };
        let summary = format!("{} {}", n.name, first_lower(&message));
        let mut c = cond(RULE, Category::Node, severity, entity, message, summary, th.for_ms, th.clear_ms);
        c.detail = Some(previous_words(nb, last));
        c.value = Some(hour.len() as f64);
        c.notify = th.notify;
        c.cause = Some("restarted without SparkBox".into());
        out.push(c);
    }
    out
}

// ---- What the UI reads

/// A node's boots for System › Boots and the node header (`boots_get`).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootsView {
    pub node: String,
    /// Newest first; the running boot has no end.
    pub boots: Vec<Boot>,
    pub current_only: bool,
    pub read_ms: Option<u64>,
    /// A journal read is under way.
    pub reading: bool,
    pub error: Option<String>,
    /// The running boot followed a restart SparkBox didn't make, in the last day: when it started.
    pub restarted_ms: Option<u64>,
    /// With `restarted_ms`: how the boot before it ended, once the journal has said.
    pub restarted_end: Option<Ended>,
}

pub fn view(node: &str, now: u64) -> BootsView {
    let store = STORE.lock().unwrap();
    let Some(nb) = store.nodes.get(node) else { return BootsView { node: node.into(), reading: store.reading.contains(node), ..Default::default() } };
    let current = nb.boots.iter().find(|b| Some(&b.boot_id) == nb.last_boot_id.as_ref());
    let restarted = current.filter(|c| !c.planned && nb.boots.iter().any(|b| b.start_ms < c.start_ms) && now.saturating_sub(c.start_ms) < RECENT_MS);
    let restarted_ms = restarted.map(|c| c.start_ms);
    let restarted_end = restarted.and_then(|c| nb.boots.iter().filter(|b| b.start_ms < c.start_ms).max_by_key(|b| b.start_ms)).and_then(|b| b.ended);
    BootsView {
        node: node.into(),
        boots: nb.boots.clone(),
        current_only: nb.current_only,
        read_ms: nb.read_ms,
        reading: store.reading.contains(node),
        error: nb.error.clone(),
        restarted_ms,
        restarted_end,
    }
}

/// Boot rows for Alerts › Events (kernel::with_boots): each boot of the last 14 days SparkBox
/// knows of on the nodes in `snap` (and the running one, however old: its start holds the link
/// events of its boot) that started at or after `since_ms`, with how the boot before it ended.
pub fn marks(snap: &ClusterSnapshot, node: Option<&str>, since_ms: u64) -> Vec<BootMark> {
    if snap.demo {
        return Vec::new();
    }
    let store = STORE.lock().unwrap();
    let mut out = Vec::new();
    for n in snap.nodes.iter().filter(|n| node.is_none_or(|id| id == n.id)) {
        let Some(nb) = store.nodes.get(&n.id) else { continue };
        let recent = |b: &&Boot| snap.ts_ms.saturating_sub(b.start_ms) <= JOURNAL_MS || Some(&b.boot_id) == nb.last_boot_id.as_ref();
        for b in nb.boots.iter().filter(|b| b.start_ms >= since_ms).filter(recent) {
            let before = nb.boots.iter().filter(|x| x.start_ms < b.start_ms).max_by_key(|x| x.start_ms);
            let (severity, meaning, action) = match (b.planned, before.and_then(|x| x.ended)) {
                (true, _) => ("info", "SparkBox restarted it".to_string(), String::new()),
                (false, Some(Ended::Unclean)) => (
                    "warn",
                    "the boot before ended without a shutdown (power loss, hang or watchdog)".to_string(),
                    format!("If it keeps happening, check {}'s power and cooling; its last warnings are in System › Boots.", n.name),
                ),
                (false, Some(Ended::Clean)) => ("info", "the boot before ended with a clean shutdown: someone or something restarted it".to_string(), "Nothing to do if you or an update restarted it.".into()),
                (false, Some(Ended::Unknown)) => ("info", "how the boot before ended isn't known: the journal no longer has it".to_string(), String::new()),
                (false, None) if before.is_none() && nb.current_only => ("info", "this node's journal keeps only the current boot".to_string(), String::new()),
                (false, None) => ("info", "how the boot before ended hasn't been read from the journal".to_string(), String::new()),
            };
            let seen = match b.source {
                Source::Live => "SparkBox saw it start",
                Source::Journal => "found in the journal",
                Source::First => "running when SparkBox connected",
            };
            let lines = before.map(|x| x.last_lines.join("\n")).unwrap_or_default();
            out.push(BootMark {
                node: n.id.clone(),
                start_ms: b.start_ms,
                severity: severity.into(),
                meaning,
                action,
                text: if lines.is_empty() { format!("Booted · {seen}") } else { format!("Booted · {seen}\n{lines}") },
            });
        }
    }
    out
}

pub mod commands {
    use tauri::AppHandle;

    use super::{now_ms, read_and_apply, BootsView, Mode, STORE};

    /// A node's boots: started, ended, how each ended and how SparkBox knows (System › Boots).
    #[tauri::command]
    pub fn boots_get(node: String) -> BootsView {
        super::view(&node, now_ms())
    }

    /// Reads the node's journal again (every earlier boot of the last 14 days not read yet).
    /// Reads only, as the cluster user.
    #[tauri::command]
    pub async fn boots_refresh(app: AppHandle, node: String) -> Result<BootsView, String> {
        let state = tauri::Manager::try_state::<crate::AppState>(&app).ok_or("SparkBox is still starting")?;
        if !state.live.is_connected(&node) {
            return Err(format!("{node} isn't connected."));
        }
        let live = state.live.clone();
        if !STORE.lock().unwrap().reading.insert(node.clone()) {
            return Ok(super::view(&node, now_ms()));
        }
        read_and_apply(&live, &node, Mode::First).await;
        Ok(super::view(&node, now_ms()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AlertState, Engine, EventKind};
    use super::*;
    use crate::model::{LinkStatus, MaintInfo, SystemInfo};

    /// The Oct 29 journals (fixtures/boots): cell3's boot from Oct 25 17:08 ended about 17:44
    /// without a shutdown, the next one 17:45-18:07 cleanly, and the one running now began 18:08;
    /// gx10-0000's boot from Oct 25 ended about 18:07 without a shutdown. Times are the Mac's
    /// and the nodes' own zone (America/New_York, EDT on Oct 29: the plan's "UTC+1" was wrong).
    fn fixture(node: &str, file: &str) -> String {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/live/fixtures/boots").join(node).join(file)).unwrap()
    }

    fn ms(iso: &str) -> u64 {
        let out = std::process::Command::new("date").args(["-u", "-j", "-f", "%Y-%m-%dT%H:%M:%S", iso, "+%s"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap() * 1000
    }

    const CELL3_A: &str = "9cabf4f7e39d4c62b9d620e269501402";
    const CELL3_B: &str = "bbc047e45b564405bb6db33df16b2dcb";
    const CELL3_C: &str = "cc21cbd1d91b415583731e100ef7d5d1";
    const GX10_X: &str = "9e77d02b1916443e9b166476becb6a81";

    /// What `boot_command` prints for one boot of a fixture node.
    fn answer(node: &str, boot: &str) -> String {
        ["warning", "pid1", "journald"].iter().map(|p| fixture(node, &format!("{boot}.{p}.jsonl"))).collect::<Vec<_>>().join(&format!("{SPLIT}\n"))
    }

    /// How a fixture boot ended, judged with its last entry from the node's `--list-boots`.
    fn judged(node: &str, boot: &str) -> (Ended, Vec<String>) {
        let last = parse_list_boots(&fixture(node, "list-boots.json")).unwrap().into_iter().find(|b| b.boot_id == boot).map(|b| b.last_ms);
        judge(&answer(node, boot), "", last)
    }

    fn node(id: &str, boot: &str, up_s: u64) -> NodeSnapshot {
        NodeSnapshot {
            id: id.into(),
            name: id.trim_start_matches(|c: char| c != '/').trim_start_matches('/').into(),
            status: LinkStatus::Online,
            boot_id: Some(boot.into()),
            system: SystemInfo { uptime_s: up_s, ..Default::default() },
            ..Default::default()
        }
    }

    fn live(ts_ms: u64, nodes: Vec<NodeSnapshot>) -> ClusterSnapshot {
        ClusterSnapshot { ts_ms, demo: false, nodes, ..Default::default() }
    }

    /// Takes these nodes out of the process-wide store (tests share it and run at once, so each
    /// uses node ids of its own), as a SparkBox that never saw them.
    fn forget(ids: &[&str]) {
        let mut s = STORE.lock().unwrap();
        for id in ids {
            s.nodes.remove(*id);
            s.read_run.remove(*id);
            s.reading.remove(*id);
            s.retry_at.remove(*id);
            s.fresh.retain(|(n, _)| n != id);
            s.windowed.remove(*id);
            s.again.remove(*id);
        }
    }

    /// One snapshot through boots and the engine, as on_publish runs them (no journal reads).
    fn step(e: &mut Engine, snap: &ClusterSnapshot) -> Vec<super::super::AlertEvent> {
        let reads = observe_at(snap, snap.ts_ms);
        let mut s = STORE.lock().unwrap();
        for (node, _) in reads {
            s.reading.remove(&node);
        }
        drop(s);
        e.step(snap, snap.ts_ms)
    }

    fn rebooted(events: &[super::super::AlertEvent]) -> Vec<&super::super::AlertEvent> {
        events.iter().filter(|e| e.alert.rule == RULE).collect()
    }

    #[test]
    fn the_oct_29_boots_list_and_end_as_the_journal_says() {
        let cell3 = parse_list_boots(&fixture("cell3", "list-boots.json")).unwrap();
        assert_eq!(cell3.iter().map(|b| (b.index, b.boot_id.as_str())).collect::<Vec<_>>(), [(0, CELL3_C), (-1, CELL3_B), (-2, CELL3_A)]);
        assert_eq!((cell3[2].first_ms, cell3[2].last_ms), (ms("2026-10-25T21:08:16"), ms("2026-10-29T21:44:20")));
        assert_eq!((cell3[1].first_ms, cell3[1].last_ms), (ms("2026-10-29T21:45:40"), ms("2026-10-29T22:07:10")));
        let gx10 = parse_list_boots(&fixture("gx10-0000", "list-boots.json")).unwrap();
        assert_eq!((gx10.len(), gx10[1].boot_id.as_str(), gx10[1].last_ms), (2, GX10_X, ms("2026-10-29T22:07:40")));
        // One object per line, json-pretty and the dashed form read the same; anything else isn't a list.
        let lines = "{\"index\":0,\"boot_id\":\"CC21CBD1-D91B-4155-8373-1E100EF7D5D1\",\"first_entry\":\"1793311710000000\",\"last_entry\":1793318340000000}\n";
        assert_eq!(parse_list_boots(lines).unwrap()[0].boot_id, CELL3_C);
        assert!(parse_list_boots("journalctl: unrecognized option '--list-boots'").is_none());
        assert!(parse_list_boots("[]").is_none());

        let (a, a_lines) = judged("cell3", CELL3_A);
        assert_eq!(a, Ended::Unclean, "no shutdown target, no Journal stopped: power loss, hang or watchdog");
        assert_eq!(a_lines.len(), 3);
        assert!(a_lines[2].ends_with("wpa_supplicant: wlP9s9: CTRL-EVENT-BEACON-LOSS bssid=f0:9f:c2:d4:ff:b5"), "{a_lines:?}");
        assert!(a_lines[1].contains(" kernel: workqueue:"), "the last warnings, named by who logged them: {a_lines:?}");
        let (b, b_lines) = judged("cell3", CELL3_B);
        assert_eq!(b, Ended::Clean);
        assert!(b_lines.iter().any(|l| l.ends_with("Reached target reboot.target - System Reboot.")) && b_lines.last().unwrap().ends_with(" systemd-journald: Journal stopped"), "{b_lines:?}");
        assert!(b_lines.iter().any(|l| l.ends_with(" systemd: Shutting down.")), "PID 1 is systemd: {b_lines:?}");
        assert_eq!(judged("gx10-0000", GX10_X).0, Ended::Unclean);
        // Its journal gone (rotated away): unknown, never unclean.
        let gone = format!("-- No entries --\n{SPLIT}\n-- No entries --\n{SPLIT}\n-- No entries --\n");
        assert_eq!(judge(&gone, "Data from the specified boot (9cabf4f7e39d4c62b9d620e269501402) is not available: No such boot ID in journal\n", None).0, Ended::Unknown);
        assert_eq!(judge("", "", None).0, Ended::Unknown, "a cut-off answer proves nothing");
        for clean in ["Reached target poweroff.target - System Power Off.", "Reached target shutdown.target - System Shutdown.", "Shutting down.", "Journal stopped"] {
            assert!(is_shutdown_line(clean), "{clean}");
        }
        assert!(!is_shutdown_line("Reached target multi-user.target - Multi-User System."));
    }

    #[test]
    fn the_reads_are_journalctl_as_written_and_classed_as_reads() {
        assert_eq!(LIST_BOOTS, "journalctl --list-boots -o json --no-pager");
        assert_eq!(
            boot_command("-1"),
            "journalctl -b '-1' -n 80 -p warning -o json --no-pager --output-fields=MESSAGE,PRIORITY,SYSLOG_IDENTIFIER,__REALTIME_TIMESTAMP; echo @@sparkbox-boot@@; \
             journalctl -b '-1' -n 50 -o json --no-pager _PID=1 --output-fields=MESSAGE,__REALTIME_TIMESTAMP; echo @@sparkbox-boot@@; \
             journalctl -b '-1' -n 20 -o json --no-pager -u systemd-journald --output-fields=MESSAGE,__REALTIME_TIMESTAMP"
        );
        use crate::ops::classify::{classify, Kind};
        assert_eq!(classify(LIST_BOOTS), Kind::Read);
        assert_eq!(classify(&boot_command("-1")), Kind::Read);
        assert_eq!(classify(&boot_command(CELL3_A)), Kind::Read);
    }

    /// A journalctl that answers from fixtures/boots/<node>, as the real one does for the cluster
    /// user: the list as one JSON line, `-b -1`/`-b <id>` reads with -n, -p, _PID=1 and -u, "-- No
    /// entries --" for none, and an unknown boot refused on stderr with exit status 1.
    fn fake_journalctl(node: &str) -> String {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/live/fixtures/boots").join(node);
        format!(
            r#"#!/usr/bin/env python3
import json, os, sys
FIX = {dir:?}
args = sys.argv[1:]
boots = json.load(open(os.path.join(FIX, "list-boots.json")))
if "--list-boots" in args:
    sys.stdout.write(open(os.path.join(FIX, "list-boots.json")).read())
    sys.exit(0)
which = args[args.index("-b") + 1] if "-b" in args else "0"
if which.lstrip("-").isdigit() and len(which) < 4:
    found = [b["boot_id"] for b in boots if b["index"] == int(which)]
else:
    found = [b["boot_id"] for b in boots if b["boot_id"] == which]
if not found:
    sys.stderr.write("Data from the specified boot (%s) is not available: No such boot ID in journal\n" % which)
    sys.exit(1)
part = "warning" if "-p" in args else "pid1" if "_PID=1" in args else "journald" if "systemd-journald" in args else "all"
path = os.path.join(FIX, "%s.%s.jsonl" % (found[0], part))
lines = open(path).read().splitlines() if os.path.exists(path) else []
n = int(args[args.index("-n") + 1]) if "-n" in args else len(lines)
print("\n".join(lines[-n:]) if lines else "-- No entries --")
"#
        )
    }

    #[tokio::test]
    async fn a_first_connection_reads_every_earlier_boot_over_ssh() {
        use crate::fake_node::{connect, fake_node, real_shell, temp};
        let (node, port) = fake_node(&temp("boots-cell3"), real_shell()).await;
        node.bin("journalctl", &fake_journalctl("cell3"));
        let (live, _) = connect(&temp("boots-app"), &[("cell3", port)]).await;
        let now = ms("2026-10-29T23:59:00");
        let read = read_journal(&live, "cell3", Mode::First, &BTreeSet::new(), now).await;
        assert_eq!(read.error, None);
        assert_eq!(read.listed.len(), 3);
        let ended: Vec<(&str, Ended)> = read.ended.iter().map(|(id, e, _)| (id.as_str(), *e)).collect();
        assert_eq!(ended, [(CELL3_B, Ended::Clean), (CELL3_A, Ended::Unclean)], "newest first; the running boot isn't judged");
        // Exactly these commands, as the cluster user (no sudo), and classed as SparkBox's own probes.
        assert_eq!(node.ran("journalctl"), vec![LIST_BOOTS.to_string(), boot_command(CELL3_B), boot_command(CELL3_A)]);
        assert!(node.ran("sudo").is_empty());
        let logged: Vec<String> = live.audit.entries().iter().filter(|e| e.command.contains("journalctl")).map(|e| e.class.clone()).collect();
        assert!(!logged.is_empty() && logged.iter().all(|c| c == "probe"), "{logged:?}");
        // Already judged: not read again; a changed bootId reads only the boot before.
        let known: BTreeSet<String> = [CELL3_A.to_string(), CELL3_B.to_string()].into();
        assert!(read_journal(&live, "cell3", Mode::First, &known, now).await.ended.is_empty());
        let changed = read_journal(&live, "cell3", Mode::Changed, &known, now).await;
        assert_eq!(changed.ended.iter().map(|(id, e, _)| (id.as_str(), *e)).collect::<Vec<_>>(), [(CELL3_B, Ended::Clean)]);
        assert_eq!(node.ran("journalctl -b '-1'").len(), 1);
        // Boots older than 14 days aren't read.
        let later = read_journal(&live, "cell3", Mode::First, &BTreeSet::new(), now + 15 * 86_400_000).await;
        assert!(later.ended.is_empty());
    }

    #[test]
    fn the_oct_29_boots_show_on_a_first_sighting_and_raise_nothing() {
        let ids = ["oct29/cell3", "oct29/gx10-0000"];
        forget(&ids);
        let mut e = Engine::default();
        let now = ms("2026-10-29T23:59:00");
        let up = |first: &str| (now - ms(first)) / 1000;
        let snap = live(now, vec![node(ids[0], CELL3_C, up("2026-10-29T22:08:30")), node(ids[1], "762a3e73-1f64-43a8-9c1a-59ccf56e0047", up("2026-10-29T22:09:00"))]);
        let reads = observe_at(&snap, now);
        assert_eq!(reads, vec![(ids[0].to_string(), Mode::First), (ids[1].to_string(), Mode::First)], "first connection in this run: read the journal");
        for (id, fix, boots) in [(ids[0], "cell3", vec![CELL3_B, CELL3_A]), (ids[1], "gx10-0000", vec![GX10_X])] {
            let listed = parse_list_boots(&fixture(fix, "list-boots.json")).unwrap();
            let ended = boots.iter().map(|b| (b.to_string(), judged(fix, b).0, judged(fix, b).1)).collect();
            STORE.lock().unwrap().apply(id, JournalRead { listed, ended, error: None }, now);
        }
        let v = view(ids[0], now);
        let rows: Vec<(Source, Option<Ended>, Option<u64>)> = v.boots.iter().map(|b| (b.source, b.ended, b.end_ms)).collect();
        assert_eq!(
            rows,
            [
                (Source::Journal, None, None),
                (Source::Journal, Some(Ended::Clean), Some(ms("2026-10-29T22:07:10"))),
                (Source::Journal, Some(Ended::Unclean), Some(ms("2026-10-29T21:44:20"))),
            ]
        );
        assert_eq!(v.restarted_ms, Some(ms("2026-10-29T22:08:30")), "the header: restarted outside SparkBox, at the boot's start");
        assert_eq!(v.restarted_end, Some(Ended::Clean), "the boot before ended with a clean shutdown: not 'on its own'");
        assert_eq!(view(ids[1], now).boots[1].ended, Some(Ended::Unclean));
        // Seen only in the journal: in System and Events, never a firing alert.
        let events = step(&mut e, &snap);
        assert!(rebooted(&events).is_empty(), "{events:?}");
        let marks = marks(&snap, Some(ids[0]), 0);
        assert_eq!(marks.len(), 3);
        let latest = marks.iter().max_by_key(|m| m.start_ms).unwrap();
        assert_eq!(latest.meaning, "the boot before ended with a clean shutdown: someone or something restarted it");
        assert!(latest.text.starts_with("Booted · found in the journal\n"), "{}", latest.text);
        let middle = marks.iter().find(|m| m.start_ms == ms("2026-10-29T21:45:40")).unwrap();
        assert_eq!((middle.severity.as_str(), middle.meaning.as_str()), ("warn", "the boot before ended without a shutdown (power loss, hang or watchdog)"));
        forget(&ids);
    }

    #[test]
    fn a_restart_without_sparkbox_raises_one_node_rebooted_and_no_reconnecting() {
        let id = "nr1/cell3";
        forget(&[id]);
        let mut e = Engine::default();
        let t0 = ms("2026-10-29T21:30:00");
        let mut all = Vec::new();
        // Up for days, then gone (SSH times out), trying again, and back on a new boot.
        for i in 0..5 {
            all.extend(step(&mut e, &live(t0 + i * 1000, vec![node(id, CELL3_A, 360_000 + i)])));
        }
        for i in 5..90 {
            let mut n = node(id, CELL3_A, 0);
            n.status = if i < 85 { LinkStatus::Offline } else { LinkStatus::Connecting };
            n.error = Some("No SSH reply in 10 s".into());
            all.extend(step(&mut e, &live(t0 + i * 1000, vec![n])));
        }
        for i in 90..100 {
            all.extend(step(&mut e, &live(t0 + i * 1000, vec![node(id, CELL3_B, i - 85)])));
        }
        let raised: Vec<_> = rebooted(&all).into_iter().filter(|ev| ev.kind == EventKind::Raised).collect();
        assert_eq!(raised.len(), 1, "{all:?}");
        let a = &raised[0].alert;
        let start = t0 + 90_000 - 5_000;
        assert_eq!((a.severity, a.message.clone()), (Severity::Warn, format!("Restarted on its own at {}", clock(start))));
        assert_eq!(a.summary, format!("cell3 restarted on its own at {}", clock(start)));
        assert_eq!(a.detail.as_deref(), Some("Reading how the boot before ended from the node's journal…"));
        for ev in &all {
            for text in [&ev.alert.message, &ev.alert.summary, &ev.alert.short] {
                assert!(!text.contains("Reconnecting"), "{text}");
            }
        }
        // Once the journal answers, the detail says how the boot before ended.
        let listed = vec![
            Listed { index: 0, boot_id: CELL3_B.into(), first_ms: start, last_ms: start + 9_000 },
            Listed { index: -1, boot_id: CELL3_A.into(), first_ms: t0 - 360_000_000, last_ms: t0 + 4_000 },
        ];
        let (ended, lines) = judged("cell3", CELL3_A);
        STORE.lock().unwrap().apply(id, JournalRead { listed, ended: vec![(CELL3_A.into(), ended, lines)], error: None }, t0 + 100_000);
        step(&mut e, &live(t0 + 101_000, vec![node(id, CELL3_B, 16)]));
        let a = e.view(t0 + 101_000, None).alerts.into_iter().find(|a| a.rule == RULE).unwrap();
        let detail = a.detail.unwrap();
        assert!(detail.starts_with(&format!("The boot before ended without a shutdown at {} (power loss, hang or watchdog). Its last warnings:\n", clock(t0 + 4_000))), "{detail}");
        assert!(detail.ends_with("wpa_supplicant: wlP9s9: CTRL-EVENT-BEACON-LOSS bssid=f0:9f:c2:d4:ff:b5"), "{detail}");
        let v = view(id, t0 + 101_000);
        assert_eq!((v.restarted_ms, v.restarted_end), (Some(start), Some(Ended::Unclean)), "the header: on its own, no clean shutdown");
        // It fires for an hour after the boot, then clears.
        let late = step(&mut e, &live(start + FIRES_MS + 1_000, vec![node(id, CELL3_B, FIRES_MS / 1000 + 1)]));
        assert!(rebooted(&late).iter().any(|ev| ev.kind == EventKind::Resolved), "{late:?}");
        forget(&[id]);
    }

    #[test]
    fn a_restart_sparkbox_made_raises_none() {
        let _live = super::super::maintenance::live_test_lock();
        let (a, b) = ("sb1/cell2", "sb2/cell2");
        forget(&[a, b]);
        let mut e = Engine::default();
        let t0 = ms("2026-10-29T12:00:00");
        // The window is on the snapshot (maintenance::stamp ran), or only in maintenance.rs (the
        // first frame back closes it in stamp, after boots has looked).
        step(&mut e, &live(t0, vec![node(a, CELL3_A, 1_000), node(b, CELL3_A, 1_000)]));
        super::super::maintenance::begin_at(false, b, Some(t0 + 480_000), "restarting", t0);
        let mut back = node(a, CELL3_B, 20);
        back.maintenance = Some(MaintInfo { until_ms: Some(t0 + 480_000), reason: "restarting".into(), started_ms: t0 });
        let events = step(&mut e, &live(t0 + 120_000, vec![back, node(b, CELL3_B, 20)]));
        assert!(rebooted(&events).is_empty(), "{events:?}");
        let v = view(b, t0 + 120_000);
        assert!(v.boots[0].planned && v.restarted_ms.is_none(), "a SparkBox restart isn't 'on its own'");
        // Back and settled: maintenance.rs closes the window, as on_publish would right after.
        super::super::maintenance::stamp(&mut live(t0 + 121_000, vec![node(b, CELL3_B, 21)]), &[], 120_000);
        assert!(super::super::maintenance::opened_ms(false, b).is_none());
        forget(&[a, b]);
    }

    /// One snapshot as on_publish runs it: boots, then maintenance::stamp, then the rules.
    fn publish(e: &mut Engine, snap: &ClusterSnapshot) -> Vec<super::super::AlertEvent> {
        let mut snap = snap.clone();
        let reads = observe_at(&snap, snap.ts_ms);
        STORE.lock().unwrap().reading.retain(|n| !reads.iter().any(|(r, _)| r == n));
        super::super::maintenance::stamp(&mut snap, &[], 120_000);
        e.step(&snap, snap.ts_ms)
    }

    #[test]
    fn a_restart_sparkbox_made_while_the_node_still_answers_on_its_old_boot_raises_none() {
        let _live = super::super::maintenance::live_test_lock();
        let id = "sb3/cell2";
        forget(&[id]);
        let mut e = Engine::default();
        let t0 = ms("2026-10-29T12:00:00");
        publish(&mut e, &live(t0, vec![node(id, CELL3_A, 50_000)]));
        // node_power opens the window, tells the agents (up to 6 s), then sends the reboot: cell2
        // answers on its old boot meanwhile. Nothing serves on it, so no model holds the window open.
        super::super::maintenance::begin_at(false, id, Some(t0 + 1_000 + 480_000), "restarting", t0 + 1_000);
        let mut all = Vec::new();
        for i in 2..6 {
            all.extend(publish(&mut e, &live(t0 + i * 1000, vec![node(id, CELL3_A, 50_000 + i)])));
        }
        assert!(super::super::maintenance::opened_ms(false, id).is_some(), "the window outlives the old boot's last frames");
        for i in 6..60 {
            let mut n = node(id, CELL3_A, 0);
            n.status = LinkStatus::Offline;
            n.error = Some("No SSH reply in 10 s".into());
            all.extend(publish(&mut e, &live(t0 + i * 1000, vec![n])));
        }
        for i in 60..70 {
            all.extend(publish(&mut e, &live(t0 + i * 1000, vec![node(id, CELL3_B, i - 55)])));
        }
        assert!(rebooted(&all).is_empty(), "{all:?}");
        let offline: Vec<_> = all.iter().filter(|ev| ev.alert.rule == "node-offline" && ev.kind == EventKind::Raised).collect();
        assert!(!offline.is_empty() && offline.iter().all(|ev| ev.alert.muted(ev.ts_ms) && ev.alert.planned.is_some()), "away while planned, paged to nobody: {offline:?}");
        let v = view(id, t0 + 70_000);
        assert!(v.boots[0].planned && v.restarted_ms.is_none(), "a SparkBox restart isn't 'on its own'");
        assert!(super::super::maintenance::opened_ms(false, id).is_none(), "back on a new boot: the window closed");
        forget(&[id]);
    }

    fn frame(node_id: &str, ts: u64, boot: &str, up_s: u64, online: bool) -> ClusterSnapshot {
        let mut n = node(node_id, boot, up_s);
        if !online {
            n.status = LinkStatus::Offline;
        }
        live(ts, vec![n])
    }

    #[test]
    fn a_window_that_closed_before_the_new_boot_showed_still_makes_the_restart_sparkboxs() {
        let mut s = Store::default();
        let t = 1_793_300_000_000;
        let fresh = |s: &Store, boot: &str| s.fresh.contains(&("g1".to_string(), boot.to_string()));
        s.observe(&frame("g1", t, CELL3_A, 50_000, true), t, &|_| None);
        // The window (opened at t + 1 s) is seen for 3 frames on the old boot, the node goes down
        // under it, and the window is gone while it's still down (the power task gave up waiting).
        for i in 2..5 {
            s.observe(&frame("g1", t + i * 1000, CELL3_A, 50_000 + i, true), t + i * 1000, &|_| Some(t + 1000));
        }
        for i in 5..10 {
            s.observe(&frame("g1", t + i * 1000, CELL3_A, 0, false), t + i * 1000, &|_| Some(t + 1000));
        }
        for i in 10..60 {
            s.observe(&frame("g1", t + i * 1000, CELL3_A, 0, false), t + i * 1000, &|_| None);
        }
        s.observe(&frame("g1", t + 60_000, CELL3_B, 5, true), t + 60_000, &|_| None);
        assert!(s.nodes["g1"].boots[0].planned && !fresh(&s, CELL3_B), "the restart SparkBox started");
        // The window still open on the boot it restarted into, then closed: a crash 3 minutes on is the node's own.
        for i in 61..64 {
            s.observe(&frame("g1", t + i * 1000, CELL3_B, i - 55, true), t + i * 1000, &|_| Some(t + 1000));
        }
        let later = t + 64_000 + 3 * 60_000;
        s.observe(&frame("g1", later, CELL3_C, 30, true), later, &|_| None);
        assert!(fresh(&s, CELL3_C), "{:?}", s.nodes["g1"].boots);

        // A restart that never happened: the node answers on its boot for minutes after the window, then crashes.
        let mut s = Store::default();
        s.observe(&frame("g2", t, CELL3_A, 50_000, true), t, &|_| None);
        s.observe(&frame("g2", t + 2_000, CELL3_A, 50_002, true), t + 2_000, &|_| Some(t + 1000));
        for k in 0..4 {
            let ts = t + 3_000 + k * 60_000;
            s.observe(&frame("g2", ts, CELL3_A, 50_003 + k * 60, true), ts, &|_| None);
        }
        let ts = t + 4 * 60_000;
        s.observe(&frame("g2", ts, CELL3_B, 20, true), ts, &|_| None);
        assert!(s.fresh.contains(&("g2".to_string(), CELL3_B.to_string())), "{:?}", s.nodes["g2"].boots);
        // Away longer than the restart's expected time and grace after the window: not SparkBox's either.
        let mut s = Store::default();
        s.observe(&frame("g3", t, CELL3_A, 50_000, true), t, &|_| None);
        s.observe(&frame("g3", t + 2_000, CELL3_A, 0, false), t + 2_000, &|_| Some(t + 1000));
        let ts = t + 2_000 + PLANNED_FOR_MS + 1_000;
        s.observe(&frame("g3", ts, CELL3_B, 20, true), ts, &|_| None);
        assert!(s.fresh.contains(&("g3".to_string(), CELL3_B.to_string())));
    }

    /// The round-1 gap: a window whose restart never happened (the node kept answering on its
    /// boot under it) made a crash soon after it planned. Only a node seen down under the window
    /// is SparkBox's restart.
    #[test]
    fn a_window_whose_restart_never_happened_doesnt_make_a_crash_planned() {
        let t = 1_793_300_000_000;
        let fresh = |s: &Store, node: &str, boot: &str| s.fresh.contains(&(node.to_string(), boot.to_string()));
        // The window stays open, the node never goes down under it, and the next frame is on a new boot.
        let mut s = Store::default();
        s.observe(&frame("w1", t, CELL3_A, 50_000, true), t, &|_| None);
        for i in 2..30 {
            s.observe(&frame("w1", t + i * 1000, CELL3_A, 50_000 + i, true), t + i * 1000, &|_| Some(t + 1000));
        }
        s.observe(&frame("w1", t + 200_000, CELL3_B, 20, true), t + 200_000, &|_| Some(t + 1000));
        assert!(fresh(&s, "w1", CELL3_B) && !s.nodes["w1"].boots[0].planned, "{:?}", s.nodes["w1"].boots);
        // A blip under the window (down, then back on the same boot) is no restart either.
        let mut s = Store::default();
        s.observe(&frame("w2", t, CELL3_A, 50_000, true), t, &|_| None);
        s.observe(&frame("w2", t + 2_000, CELL3_A, 0, false), t + 2_000, &|_| Some(t + 1000));
        s.observe(&frame("w2", t + 3_000, CELL3_A, 50_003, true), t + 3_000, &|_| Some(t + 1000));
        for i in 4..40 {
            s.observe(&frame("w2", t + i * 1000, CELL3_A, 0, false), t + i * 1000, &|_| None);
        }
        s.observe(&frame("w2", t + 40_000, CELL3_B, 20, true), t + 40_000, &|_| None);
        assert!(fresh(&s, "w2", CELL3_B));
        // Down under the window, then back on a new boot: SparkBox's restart, window open or not.
        let mut s = Store::default();
        s.observe(&frame("w3", t, CELL3_A, 50_000, true), t, &|_| None);
        s.observe(&frame("w3", t + 2_000, CELL3_A, 50_002, true), t + 2_000, &|_| Some(t + 1000));
        s.observe(&frame("w3", t + 3_000, CELL3_A, 0, false), t + 3_000, &|_| Some(t + 1000));
        s.observe(&frame("w3", t + 90_000, CELL3_B, 20, true), t + 90_000, &|_| Some(t + 1000));
        assert!(!fresh(&s, "w3", CELL3_B) && s.nodes["w3"].boots[0].planned);
    }

    #[test]
    fn a_crash_right_after_a_restart_that_never_happened_raises_node_rebooted() {
        let id = "nh1/cell3";
        forget(&[id]);
        let mut e = Engine::default();
        let t0 = ms("2026-10-30T09:00:00");
        let mut all = Vec::new();
        all.extend(step(&mut e, &live(t0, vec![node(id, CELL3_A, 360_000)])));
        // SparkBox's window: the restart is sent, but the node keeps answering on its boot for a
        // minute, and the power task gives up ("didn't happen"), closing the window.
        let opened = MaintInfo { until_ms: Some(t0 + 1_000 + 480_000), reason: "restarting".into(), started_ms: t0 + 1_000 };
        for i in 2..62 {
            let mut n = node(id, CELL3_A, 360_000 + i);
            n.maintenance = Some(opened.clone());
            all.extend(step(&mut e, &live(t0 + i * 1000, vec![n])));
        }
        all.extend(step(&mut e, &live(t0 + 62_000, vec![node(id, CELL3_A, 360_062)])));
        // 30 s later it crashes on its own, and is back on a new boot 2 minutes on.
        for i in 92..212 {
            let mut n = node(id, CELL3_A, 0);
            n.status = LinkStatus::Offline;
            n.error = Some("No SSH reply in 10 s".into());
            all.extend(step(&mut e, &live(t0 + i * 1000, vec![n])));
        }
        for i in 212..220 {
            all.extend(step(&mut e, &live(t0 + i * 1000, vec![node(id, CELL3_B, i - 205)])));
        }
        let raised: Vec<_> = rebooted(&all).into_iter().filter(|ev| ev.kind == EventKind::Raised).collect();
        assert_eq!(raised.len(), 1, "{all:?}");
        assert!(raised[0].alert.summary.starts_with("cell3 restarted on its own at"), "{:?}", raised[0].alert.summary);
        let v = view(id, t0 + 220_000);
        assert!(!v.boots[0].planned && v.restarted_ms.is_some(), "{v:?}");
        forget(&[id]);
    }

    #[test]
    fn a_journald_restart_mid_boot_isnt_a_clean_shutdown() {
        let line = |s: u64, message: &str| serde_json::json!({ "__REALTIME_TIMESTAMP": (s * 1_000_000).to_string(), "MESSAGE": message }).to_string();
        let answer = |warnings: &[String], pid1: &[String], journald: &[String]| [warnings.join("\n"), pid1.join("\n"), journald.join("\n")].join(&format!("\n{SPLIT}\n"));
        let restart = |s: u64| -> (Vec<String>, Vec<String>) {
            (
                vec![line(s, "Stopping systemd-journald.service - Journal Service..."), line(s + 1, "Started systemd-journald.service - Journal Service.")],
                vec![line(s, "Journal stopped"), line(s + 1, "Journal started"), line(s + 1, "System Journal (/var/log/journal/x) is 1.2G, max 4.0G, 2.8G free.")],
            )
        };
        let lockup = vec![serde_json::json!({ "__REALTIME_TIMESTAMP": "1793300000000000", "PRIORITY": "3", "SYSLOG_IDENTIFIER": "kernel", "MESSAGE": "watchdog: BUG: soft lockup - CPU#3 stuck for 26s!" }).to_string()];
        // An upgrade of systemd restarted journald; 28 h later the node hung. With and without --list-boots's last entry.
        let (pid1, journald) = restart(1_793_200_000);
        for last in [Some(1_793_300_000_000), None] {
            let (ended, lines) = judge(&answer(&lockup, &pid1, &journald), "", last);
            assert_eq!(ended, Ended::Unclean, "{last:?}: {lines:?}");
        }
        // journald restarted a minute before the power went: near the end, but it started again.
        let (pid1, journald) = restart(1_793_299_940);
        assert_eq!(judge(&answer(&lockup, &pid1, &journald), "", Some(1_793_300_000_000)).0, Ended::Unclean);
        // A shutdown line a day before the boot's last entry isn't how it ended.
        let stale = vec![line(1_793_200_000, "Reached target reboot.target - System Reboot.")];
        for last in [Some(1_793_300_000_000), None] {
            assert_eq!(judge(&answer(&lockup, &stale, &[]), "", last).0, Ended::Unclean, "{last:?}");
        }
        // The same boot later shut down cleanly: clean, from the lines at its end.
        let (mut pid1, mut journald) = restart(1_793_200_000);
        pid1.extend([line(1_793_300_100, "Reached target reboot.target - System Reboot."), line(1_793_300_101, "Shutting down.")]);
        journald.push(line(1_793_300_101, "Journal stopped"));
        let (ended, lines) = judge(&answer(&lockup, &pid1, &journald), "", Some(1_793_300_101_000));
        assert_eq!(ended, Ended::Clean);
        assert!(lines.last().unwrap().ends_with(" systemd-journald: Journal stopped"), "{lines:?}");
    }

    #[test]
    fn a_first_sighting_raises_none() {
        let id = "fs1/cell1";
        forget(&[id]);
        let mut e = Engine::default();
        let t0 = ms("2026-10-29T12:00:00");
        // Never seen: SparkBox came in on this boot, minutes after it started.
        let events = step(&mut e, &live(t0, vec![node(id, CELL3_C, 90)]));
        assert!(rebooted(&events).is_empty());
        assert_eq!(STORE.lock().unwrap().nodes[id].boots[0].source, Source::First);
        forget(&[id]);
    }

    #[test]
    fn a_second_restart_within_an_hour_is_critical() {
        let id = "tw1/cell3";
        forget(&[id]);
        let mut e = Engine::default();
        let t0 = ms("2026-10-29T21:40:00");
        step(&mut e, &live(t0, vec![node(id, CELL3_A, 300_000)]));
        let first = step(&mut e, &live(t0 + 5 * 60_000, vec![node(id, CELL3_B, 60)]));
        assert_eq!(rebooted(&first).iter().map(|ev| (ev.kind, ev.alert.severity)).collect::<Vec<_>>(), [(EventKind::Raised, Severity::Warn)]);
        // 23 minutes later it restarts again (the Oct 29 pair on cell3).
        let again = step(&mut e, &live(t0 + 28 * 60_000, vec![node(id, CELL3_C, 60)]));
        let ev = rebooted(&again);
        assert_eq!(ev.iter().map(|ev| (ev.kind, ev.alert.severity)).collect::<Vec<_>>(), [(EventKind::Escalated, Severity::Crit)], "{again:?}");
        assert_eq!(ev[0].alert.summary, format!("cell3 restarted on its own at {} · 2nd time in 23 min", clock(t0 + 27 * 60_000)));
        forget(&[id]);
    }

    #[test]
    fn two_nodes_restarting_within_5_minutes_are_critical() {
        let (a, b) = ("tn1/cell3", "tn1/gx10-0000");
        forget(&[a, b]);
        let mut e = Engine::default();
        let t0 = ms("2026-10-29T22:00:00");
        step(&mut e, &live(t0, vec![node(a, CELL3_B, 1_000), node(b, GX10_X, 300_000)]));
        let events = step(&mut e, &live(t0 + 10 * 60_000, vec![node(a, CELL3_C, 90), node(b, "762a3e731f6443a89c1a59ccf56e0047", 30)]));
        let ev = rebooted(&events);
        assert_eq!(ev.len(), 2, "{events:?}");
        assert!(ev.iter().all(|ev| ev.alert.severity == Severity::Crit));
        assert!(ev.iter().any(|ev| ev.alert.summary == format!("cell3 restarted on its own at {} · gx10-0000 too", clock(t0 + 10 * 60_000 - 90_000))), "{ev:?}");
        forget(&[a, b]);
    }

    #[test]
    fn reloading_boots_json_raises_nothing_again() {
        let id = "rl1/cell3";
        forget(&[id]);
        let dir = std::env::temp_dir().join(format!("sparkbox-boots-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let t0 = ms("2026-10-29T22:00:00");
        step(&mut e, &live(t0, vec![node(id, CELL3_B, 1_000)]));
        let raised = step(&mut e, &live(t0 + 60_000, vec![node(id, CELL3_C, 30)]));
        assert_eq!(rebooted(&raised).len(), 1);
        save_to(&dir);
        let saved = e.saved();
        // SparkBox quits and opens again 10 minutes later: boots.json and the active alerts come back.
        forget(&[id]);
        load_from(&dir);
        assert_eq!(STORE.lock().unwrap().nodes[id].last_boot_id.as_deref(), Some(CELL3_C));
        let later = live(t0 + 11 * 60_000, vec![node(id, CELL3_C, 630)]);
        let mut restored = Engine::default();
        restored.set_source(false, saved, Vec::new(), later.ts_ms);
        let events = step(&mut restored, &later);
        assert!(rebooted(&events).is_empty(), "the same boot: nothing raised again, {events:?}");
        assert!(restored.view(later.ts_ms, None).alerts.iter().any(|a| a.rule == RULE && a.state == AlertState::Firing), "and the restored alert holds");
        // Without its saved alert (alerts-active lost), the reloaded restart raises nothing either.
        let mut fresh = Engine::default();
        assert!(rebooted(&step(&mut fresh, &later)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        forget(&[id]);
    }

    #[test]
    fn a_journal_that_keeps_only_the_current_boot_says_so() {
        let id = "co1/cell2";
        forget(&[id]);
        let now = ms("2026-10-29T22:00:00");
        observe_at(&live(now, vec![node(id, CELL3_C, 600)]), now);
        let listed = vec![Listed { index: 0, boot_id: CELL3_C.into(), first_ms: now - 600_000, last_ms: now }];
        STORE.lock().unwrap().apply(id, JournalRead { listed, ended: Vec::new(), error: None }, now);
        let v = view(id, now);
        assert!(v.current_only && v.restarted_ms.is_none());
        assert_eq!(marks(&live(now, vec![node(id, CELL3_C, 600)]), None, 0)[0].meaning, "this node's journal keeps only the current boot");
        let nb = STORE.lock().unwrap().nodes[id].clone();
        assert_eq!(previous_words(&nb, &nb.boots[0]), "This node's journal keeps only the current boot, so how the last one ended isn't known.");
        forget(&[id]);
    }

    #[test]
    fn events_list_the_boots_of_the_last_14_days_and_the_running_one() {
        let (old, up) = ("ev1/cell1", "ev1/gx10-0000");
        forget(&[old, up]);
        let now = ms("2026-10-29T22:00:00");
        let day = 86_400_000;
        // gx10-0000 has run for 20 days; cell1's journal goes back two months.
        observe_at(&live(now, vec![node(old, CELL3_C, 3_600), node(up, GX10_X, 20 * 86_400)]), now);
        let listed = |id: &str, first: u64, last: u64, index: i64| Listed { index, boot_id: id.into(), first_ms: first, last_ms: last };
        let reads = [
            (old, vec![listed(CELL3_C, now - 3_600_000, now, 0), listed(CELL3_B, now - 10 * day, now - 3_700_000, -1), listed(CELL3_A, now - 60 * day, now - 10 * day - 1_000, -2)]),
            (up, vec![listed(GX10_X, now - 20 * day, now, 0)]),
        ];
        for (id, l) in reads {
            STORE.lock().unwrap().apply(id, JournalRead { listed: l, ended: Vec::new(), error: None }, now);
        }
        let starts = |id: &str| marks(&live(now, vec![node(old, CELL3_C, 3_600), node(up, GX10_X, 20 * 86_400)]), Some(id), 0).iter().map(|m| (now - m.start_ms) / day).collect::<Vec<_>>();
        assert_eq!(starts(old), [0, 10], "the 60-day-old boot isn't listed");
        assert_eq!(starts(up), [20], "the running boot is, however old");
        forget(&[old, up]);
    }

    #[test]
    fn a_boot_judged_by_the_older_rule_is_read_again() {
        // boots.json as round 1 saved it: no judgeV, so judged by the rule that took any "Journal stopped".
        let old: Boot = serde_json::from_str(&format!(r#"{{"bootId":"{CELL3_A}","startMs":1,"endMs":2,"ended":"clean","lastLines":[],"source":"journal"}}"#)).unwrap();
        assert_eq!(old.judge_v, 0);
        let mut s = Store::default();
        let t = 1_793_300_000_000;
        s.observe(&live(t, vec![node("jv1", CELL3_C, 100)]), t, &|_| None);
        s.nodes.get_mut("jv1").unwrap().boots.push(old);
        let listed = vec![
            Listed { index: 0, boot_id: CELL3_C.into(), first_ms: t - 100_000, last_ms: t },
            Listed { index: -1, boot_id: CELL3_B.into(), first_ms: t - 200_000, last_ms: t - 150_000 },
            Listed { index: -2, boot_id: CELL3_A.into(), first_ms: 1, last_ms: 2 },
        ];
        s.apply("jv1", JournalRead { listed, ended: vec![(CELL3_B.into(), Ended::Clean, Vec::new())], error: None }, t);
        assert_eq!(s.known("jv1"), [CELL3_B.to_string()].into(), "CELL3_A was judged by the older rule: read again");
        let b = s.nodes["jv1"].boots.iter().find(|b| b.boot_id == CELL3_B).unwrap();
        assert_eq!((b.ended, b.judge_v), (Some(Ended::Clean), JUDGE_V));
    }

    #[test]
    fn a_restart_while_the_first_read_runs_reads_the_boot_before_after_it() {
        let mut s = Store::default();
        let t = 1_793_300_000_000;
        assert_eq!(s.observe(&live(t, vec![node("rr1", CELL3_A, 100)]), t, &|_| None).0, vec![("rr1".to_string(), Mode::First)]);
        // It restarts while that read runs: one read at a time, so this one waits.
        assert!(s.observe(&live(t + 60_000, vec![node("rr1", CELL3_B, 10)]), t + 60_000, &|_| None).0.is_empty());
        // The first read answers from before the restart: CELL3_A was still running then.
        s.apply("rr1", JournalRead { listed: vec![Listed { index: 0, boot_id: CELL3_A.into(), first_ms: t - 100_000, last_ms: t }], ended: vec![], error: None }, t + 61_000);
        assert_eq!(s.observe(&live(t + 62_000, vec![node("rr1", CELL3_B, 12)]), t + 62_000, &|_| None).0, vec![("rr1".to_string(), Mode::First)], "now the boot before");
        assert!(s.observe(&live(t + 63_000, vec![node("rr1", CELL3_B, 13)]), t + 63_000, &|_| None).0.is_empty(), "once");
    }

    #[test]
    fn a_failed_read_is_kept_and_tried_again() {
        let mut s = Store::default();
        let snap = live(1_000_000, vec![node("cell1", CELL3_A, 100)]);
        let (reads, _) = s.observe(&snap, 1_000_000, &|_| None);
        assert_eq!(reads, vec![("cell1".to_string(), Mode::First)]);
        assert!(s.observe(&snap, 1_001_000, &|_| None).0.is_empty(), "one read at a time, once per run");
        s.apply("cell1", JournalRead { error: Some("cell1 isn't connected.".into()), ..Default::default() }, 1_002_000);
        assert_eq!(s.nodes["cell1"].error.as_deref(), Some("cell1 isn't connected."));
        assert!(s.observe(&snap, 1_003_000, &|_| None).0.is_empty());
        assert_eq!(s.observe(&snap, 1_002_000 + RETRY_MS, &|_| None).0, vec![("cell1".to_string(), Mode::First)]);
        // Demo has no journal.
        let mut demo = snap.clone();
        demo.demo = true;
        let mut d = Store::default();
        assert!(d.observe(&demo, 1_000_000, &|_| None).0.is_empty() && d.nodes.is_empty());
    }

    #[test]
    fn local_times_read_like_the_mac_shows_them() {
        let off = super::super::maintenance::local_offset_s();
        let at = |h: i64, m: i64| ((h * 3600 + m * 60 - off).rem_euclid(86_400) as u64 + 1_793_232_000) * 1000;
        assert_eq!(clock(at(18, 8)), "6:08 PM");
        assert_eq!(clock(at(0, 5)), "12:05 AM");
        assert_eq!(clock(at(12, 0)), "12:00 PM");
        assert_eq!((ordinal(2), ordinal(3), ordinal(11), ordinal(21)), ("2nd".to_string(), "3rd".to_string(), "11th".to_string(), "21st".to_string()));
    }

    #[test]
    fn node_rebooted_is_shipped_mac_only() {
        let r = super::super::rule::RuleSet::defaults();
        let rule = r.get(RULE).expect("in the shipped rules");
        assert!(rule.mac_only && rule.enabled && rule.short.len() <= super::super::rule::SHORT_MAX);
        assert_eq!(r.th(RULE, None).window_ms, FIRES_MS);
        assert!(super::super::rules::CODED.contains(&RULE), "coded: the agents' paging treats it as the Mac's");
    }
}
