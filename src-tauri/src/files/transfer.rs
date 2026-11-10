//! Copies and moves: one tar stream per batch (`tar -cv` on the source, piped through SparkBox
//! into `tar -x` on the destination), so names, folders and permissions come across intact and
//! `-v` names the file in flight. Everything lands in a hidden staging folder inside the
//! destination (`.sparkbox-incoming-<id>`). It's put in place, following each item's conflict
//! policy, only once both tars reported success (an exit status of 0: a tar killed by a signal,
//! or a channel that closed without a status, is a failure) and the staged entries, total bytes
//! and every file's size match what the sending side listed before it started. A move removes
//! the originals only after that. Stopping or failing removes the staging folder; when that
//! can't be done (the node went away) it's retried with the node back and Admin unlocked, and the window says
//! what was left where. Before each copy into a folder, staging folders there that no running
//! copy owns are swept. Within one node a copy runs on the node itself (sparkjob.py cp) and a
//! move is a rename.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::ChannelMsg;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;

use super::demo::Item;
use super::mac::Landing;
use super::{mac, paths, valid_name, Emit, Files, Leftover, TransferEvent, DEMO_TO_MAC, MAC};
use crate::live::{sh_quote as q, ssh::sig_name, Exec, SPARKJOB};

/// What to do with an item whose name is already taken at the destination.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Policy {
    /// Swap the old item out whole (a folder loses files the new one doesn't have).
    Replace,
    /// Folders merge; files with the same name are replaced.
    Merge,
    Skip,
    /// Keep the old one and add the new one as "name 2".
    KeepBoth,
    /// Merge, replacing a file only when the incoming copy is newer.
    ReplaceIfNewer,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    #[default]
    Copy,
    Move,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSpec {
    pub from: String,
    /// The folder the items are in.
    pub dir: String,
    pub names: Vec<String>,
    pub to: String,
    pub to_dir: String,
    #[serde(default)]
    pub op: Op,
    /// By name, for the names that were taken when checked. Missing: no conflict expected.
    #[serde(default)]
    pub policies: HashMap<String, Policy>,
}

/// One side of an item: what it is, how big, and when it last changed.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub kind: String,
    pub bytes: u64,
    pub entries: u64,
    pub mtime: u64,
    /// Bytes in the tar stream for it (headers and padding included), for honest progress.
    pub tar_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub name: String,
    pub source: Side,
    pub dest: Side,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub bytes: u64,
    pub entries: u64,
    pub conflicts: Vec<Conflict>,
    /// Free bytes where the items are going.
    pub free: Option<u64>,
    /// "local" (within one place), "direct" (Mac and a node) or "viaMac" (node to node, through this Mac).
    pub route: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Placed {
    pub name: String,
    /// copied | replaced | merged | keptBoth | skipped
    pub action: String,
    /// The name it ended up with ("notes 2.txt" for keep both).
    #[serde(rename = "as")]
    pub as_name: String,
}

pub(super) enum Stop {
    Cancelled,
    Failed(String),
}

impl From<String> for Stop {
    fn from(s: String) -> Stop {
        Stop::Failed(s)
    }
}

type Outcome = Result<(u64, u64, Vec<Placed>), Stop>;

async fn cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// `tar -v` output from the source: which file is in flight, how many have gone, and warnings.
pub(super) struct Verbose {
    names: Vec<String>,
    /// bsdtar prints "a name"; GNU tar prints the name alone. Decided by the first line.
    bsd: Option<bool>,
    partial: String,
    pub files: u64,
    pub current: Option<String>,
    pub errors: String,
}

impl Verbose {
    pub fn new(names: &[String]) -> Verbose {
        Verbose { names: names.to_vec(), bsd: None, partial: String::new(), files: 0, current: None, errors: String::new() }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.partial.push_str(&String::from_utf8_lossy(bytes));
        while let Some(i) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=i).collect();
            self.line(line.trim_end());
        }
    }

    fn line(&mut self, l: &str) {
        if l.is_empty() {
            return;
        }
        if l.starts_with("tar: ") || l.starts_with("bsdtar: ") {
            if self.errors.len() < 4000 {
                self.errors.push_str(l);
                self.errors.push('\n');
            }
            return;
        }
        let ours = |s: &str| self.names.iter().any(|n| s == n || s.starts_with(&format!("{n}/")));
        let bsd = *self.bsd.get_or_insert_with(|| !ours(l) && l.strip_prefix("a ").is_some_and(ours));
        let name = if bsd { l.strip_prefix("a ").unwrap_or(l) } else { l };
        self.files += 1;
        self.current = Some(name.trim_end_matches('/').to_string());
    }
}

/// Progress for one transfer, reported about four times a second.
struct Meter {
    done: u64,
    total: u64,
    files: u64,
    files_total: u64,
    current: Option<String>,
    last: Instant,
}

impl Meter {
    fn new(total: u64, files_total: u64) -> Meter {
        Meter { done: 0, total, files: 0, files_total, current: None, last: Instant::now() - Duration::from_secs(1) }
    }

    fn event(&self) -> TransferEvent {
        TransferEvent::Progress {
            done: self.done,
            total: self.total.max(self.done),
            files: self.files,
            files_total: self.files_total.max(self.files),
            current: self.current.clone(),
        }
    }

    fn report(&mut self, emit: &Emit, force: bool) {
        if force || self.last.elapsed() >= Duration::from_millis(250) {
            emit(self.event());
            self.last = Instant::now();
        }
    }
}

/// The `tar -c` side of a copy.
enum Source {
    Node(Exec, String, Option<u32>),
    Mac(Child),
}

/// The `tar -x` side of a copy.
enum Sink {
    Node(Exec, String),
    Mac(Child),
}

fn mac_tar(args: &[&str]) -> Command {
    let mut c = Command::new("/usr/bin/tar");
    // No AppleDouble ._ files or macOS extended attributes in the stream.
    c.env("COPYFILE_DISABLE", "1").args(["--no-mac-metadata", "--no-xattrs"]).args(args);
    c.kill_on_drop(true);
    c
}

impl Source {
    async fn next(&mut self, buf: &mut [u8], verbose: &Mutex<Verbose>) -> Result<Option<Vec<u8>>, String> {
        match self {
            Source::Node(exec, node, code) => loop {
                match exec.read.wait().await {
                    Some(ChannelMsg::Data { data }) => return Ok(Some(data.to_vec())),
                    Some(ChannelMsg::ExtendedData { data, .. }) => verbose.lock().unwrap().feed(&data),
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        *code = Some(exit_status);
                        if exit_status != 0 {
                            return Err(format!("tar failed on {node}: {}", verbose.lock().unwrap().errors.trim()));
                        }
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => return Err(format!("tar on {node} was killed (SIG{})", sig_name(&signal_name))),
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => return Ok(None),
                    _ => {}
                }
            },
            Source::Mac(child) => {
                let out = child.stdout.as_mut().ok_or("tar has no output")?;
                let n = out.read(buf).await.map_err(|e| e.to_string())?;
                Ok((n > 0).then(|| buf[..n].to_vec()))
            }
        }
    }

    async fn finish(self, verbose: &Mutex<Verbose>) -> Result<(), String> {
        match self {
            Source::Node(mut exec, node, mut code) => {
                let mut signal = None;
                while code.is_none() && signal.is_none() {
                    match exec.read.wait().await {
                        Some(ChannelMsg::ExitStatus { exit_status }) => code = Some(exit_status),
                        Some(ChannelMsg::ExitSignal { signal_name, .. }) => signal = Some(sig_name(&signal_name)),
                        Some(ChannelMsg::ExtendedData { data, .. }) => verbose.lock().unwrap().feed(&data),
                        Some(ChannelMsg::Close) | None => break,
                        _ => {}
                    }
                }
                match (code, signal) {
                    (Some(0), _) => Ok(()),
                    (_, Some(sig)) => Err(format!("tar on {node} was killed (SIG{sig})")),
                    (Some(_), None) => Err(format!("tar failed on {node}: {}", verbose.lock().unwrap().errors.trim())),
                    (None, None) => Err(format!("The sending side ({node}) ended without reporting success")),
                }
            }
            Source::Mac(mut child) => {
                let status = child.wait().await.map_err(|e| e.to_string())?;
                // Let the stderr reader catch up with the last lines.
                tokio::time::sleep(Duration::from_millis(20)).await;
                use std::os::unix::process::ExitStatusExt;
                match (status.success(), status.signal()) {
                    (true, _) => Ok(()),
                    (_, Some(n)) => Err(format!("tar on this Mac was killed (signal {n})")),
                    _ => Err(format!("tar failed on this Mac: {}", verbose.lock().unwrap().errors.trim())),
                }
            }
        }
    }
}

impl Sink {
    async fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
        match self {
            Sink::Node(exec, node) => {
                // Watch the channel while writing: a receiver that died stops taking data, and its
                // window never opens again, so the write alone would wait forever.
                let Exec { read, write, .. } = exec;
                let sending = write.data(chunk);
                tokio::pin!(sending);
                loop {
                    tokio::select! {
                        r = &mut sending => return r.map_err(|_| format!("The receiving side ({node}) closed the connection")),
                        msg = read.wait() => match msg {
                            Some(ChannelMsg::ExitSignal { signal_name, .. }) => return Err(format!("tar on {node} was killed (SIG{})", sig_name(&signal_name))),
                            Some(ChannelMsg::ExitStatus { exit_status }) => return Err(format!("unpacking on {node} stopped early (exit {exit_status})")),
                            Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => return Err(format!("The receiving side ({node}) ended without reporting success")),
                            _ => {}
                        },
                    }
                }
            }
            Sink::Mac(child) => {
                child.stdin.as_mut().ok_or("tar isn't reading")?.write_all(chunk).await.map_err(|e| format!("tar on this Mac stopped: {e}"))
            }
        }
    }

    async fn finish(self) -> Result<(), String> {
        match self {
            Sink::Node(mut exec, node) => {
                exec.write.eof().await.map_err(|e| e.to_string())?;
                let mut stderr = String::new();
                let (mut code, mut signal) = (None, None);
                while let Some(msg) = exec.read.wait().await {
                    match msg {
                        ChannelMsg::ExtendedData { data, .. } | ChannelMsg::Data { data } => stderr.push_str(&String::from_utf8_lossy(&data)),
                        ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
                        ChannelMsg::ExitSignal { signal_name, .. } => signal = Some(sig_name(&signal_name)),
                        ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                // Only an exit status of 0 is success: no status at all means it didn't finish.
                match (code, signal) {
                    (Some(0), _) => Ok(()),
                    (_, Some(sig)) => Err(format!("tar on {node} was killed (SIG{sig})")),
                    (Some(_), None) => Err(format!("unpacking on {node} failed: {}", stderr.trim())),
                    (None, None) => Err(format!("The receiving side ({node}) ended without reporting success")),
                }
            }
            Sink::Mac(mut child) => {
                drop(child.stdin.take());
                let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
                if out.status.success() {
                    Ok(())
                } else {
                    Err(format!("unpacking on this Mac failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
                }
            }
        }
    }
}

/// Pipes source to sink until it ends or the transfer is stopped.
async fn pump(mut src: Source, mut sink: Sink, meter: &mut Meter, verbose: &Mutex<Verbose>, emit: &Emit, cancel: &mut watch::Receiver<bool>) -> Result<(), Stop> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancelled(cancel) => return Err(Stop::Cancelled),
            c = src.next(&mut buf, verbose) => c?,
        };
        let Some(chunk) = chunk else { break };
        tokio::select! {
            biased;
            _ = cancelled(cancel) => return Err(Stop::Cancelled),
            r = sink.write(&chunk) => r?,
        }
        meter.done += chunk.len() as u64;
        {
            let v = verbose.lock().unwrap();
            meter.files = v.files;
            meter.current = v.current.clone();
        }
        meter.report(emit, false);
    }
    // The destination may still be unpacking; stopping now still removes everything.
    tokio::select! {
        biased;
        _ = cancelled(cancel) => return Err(Stop::Cancelled),
        r = async {
            src.finish(verbose).await?;
            sink.finish().await
        } => r?,
    }
    let v = verbose.lock().unwrap();
    meter.files = v.files.max(meter.files);
    Ok(())
}

/// A copy of `dir/name` can't go inside itself.
fn into_itself(spec: &TransferSpec) -> Option<String> {
    if spec.from != spec.to {
        return None;
    }
    let to = paths::clean(&spec.to_dir);
    spec.names.iter().find_map(|n| {
        let item = paths::clean(&format!("{}/{n}", spec.dir));
        (to == item || to.starts_with(&format!("{item}/"))).then(|| format!("{n} can't be copied into itself."))
    })
}

fn validate(spec: &TransferSpec) -> Result<(), String> {
    if spec.names.is_empty() {
        return Err("Nothing to copy.".into());
    }
    for n in &spec.names {
        valid_name(n)?;
    }
    if spec.dir.trim().is_empty() || spec.to_dir.trim().is_empty() {
        return Err("Choose a folder first.".into());
    }
    if let Some(e) = into_itself(spec) {
        return Err(e);
    }
    Ok(())
}

fn route(spec: &TransferSpec) -> &'static str {
    if spec.from == spec.to {
        "local"
    } else if spec.from != MAC && spec.to != MAC {
        "viaMac"
    } else {
        "direct"
    }
}

/// The Mac items under `dir/name` as demo items: names, sizes, times and modes only. Demo never
/// reads what's in a file on this Mac (a Demo node's copy says so when opened).
fn mac_export(dir: &Path, name: &str) -> Vec<(String, Item)> {
    fn walk(p: &Path, rel: String, out: &mut Vec<(String, Item)>) {
        let Ok(m) = p.symlink_metadata() else { return };
        use std::os::unix::fs::PermissionsExt;
        let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let mode = m.permissions().mode() & 0o7777;
        let link = m.file_type().is_symlink().then(|| std::fs::read_link(p).map(|t| t.display().to_string()).unwrap_or_default());
        out.push((
            rel.clone(),
            Item { dir: m.is_dir(), link, size: if m.is_dir() { 4096 } else { m.len() }, mtime, mode, owner: String::new(), content: None, peek: None },
        ));
        if m.is_dir() {
            let mut kids: Vec<_> = std::fs::read_dir(p).into_iter().flatten().flatten().collect();
            kids.sort_by_key(|e| e.file_name());
            for e in kids {
                walk(&e.path(), format!("{rel}/{}", e.file_name().to_string_lossy()), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&dir.join(name), name.to_string(), &mut out);
    out
}


/// Checks the staged copy against the sender's listing: every entry there, of the same kind,
/// every file the same size.
fn verify(expected: &BTreeMap<String, (String, u64)>, got: &BTreeMap<String, (String, u64)>, place: &str) -> Result<(), String> {
    let missing: Vec<&String> = expected.keys().filter(|k| !got.contains_key(*k)).collect();
    if let Some(first) = missing.first() {
        return Err(format!(
            "{} of {} item{} didn't arrive on {} (first: {first}); nothing was put in place.",
            missing.len(),
            expected.len(),
            if expected.len() == 1 { "" } else { "s" },
            place_name(place)
        ));
    }
    for (rel, (kind, size)) in expected {
        let (gk, gs) = &got[rel];
        if gk != kind {
            return Err(format!("{rel} arrived as a different kind of item; nothing was put in place."));
        }
        if kind == "f" && gs != size {
            return Err(format!(
                "{rel} arrived as {}, expected {} (it may have changed while it was copied); nothing was put in place.",
                crate::files::human_bytes(*gs),
                crate::files::human_bytes(*size)
            ));
        }
    }
    let (sent, arrived): (u64, u64) = (expected.values().map(|e| e.1).sum(), got.iter().filter(|(k, _)| expected.contains_key(*k)).map(|(_, e)| e.1).sum());
    if sent != arrived {
        return Err(format!("{} arrived, {} were sent; nothing was put in place.", crate::files::human_bytes(arrived), crate::files::human_bytes(sent)));
    }
    Ok(())
}

fn place_label(place: &str) -> String {
    place_name(place)
}

pub(super) fn place_name(place: &str) -> String {
    if place == MAC { "this Mac".into() } else { place.to_string() }
}

/// `tree` on this Mac.
fn mac_tree(dir: &Path, names: &[String]) -> BTreeMap<String, (String, u64)> {
    fn walk(p: &Path, rel: String, out: &mut BTreeMap<String, (String, u64)>) {
        let Ok(m) = p.symlink_metadata() else { return };
        let kind = if m.file_type().is_symlink() { "l" } else if m.is_dir() { "d" } else { "f" };
        out.insert(rel.clone(), (kind.into(), if kind == "f" { m.len() } else { 0 }));
        if kind == "d" {
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                walk(&e.path(), format!("{rel}/{}", e.file_name().to_string_lossy()), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    for n in names {
        walk(&dir.join(n), n.clone(), &mut out);
    }
    out
}

/// `sweep` on this Mac: staging folders untouched for 24 hours, or for 10 minutes when no
/// running copy owns them.
fn mac_sweep(dir: &Path, keep: &[String]) -> Vec<PathBuf> {
    fn newest(p: &Path) -> std::time::SystemTime {
        let mut t = p.symlink_metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        if p.is_dir() && !p.is_symlink() {
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                t = t.max(newest(&e.path()));
            }
        }
        t
    }
    let mut removed = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(nonce) = name.strip_prefix(".sparkbox-incoming-") else { continue };
        let idle = newest(&e.path()).elapsed().unwrap_or_default();
        let mine = keep.iter().any(|k| k == nonce);
        let stale = idle >= Duration::from_secs(24 * 3600) || (!mine && idle >= Duration::from_secs(600));
        if stale && std::fs::remove_dir_all(e.path()).is_ok() {
            removed.push(e.path());
        }
    }
    removed
}

impl Files {
    /// An absolute path on a place ("~" expanded with that place's home).
    pub(super) async fn expand(&self, demo: bool, place: &str, path: &str) -> Result<String, String> {
        if place == MAC {
            return Ok(mac::path(path).display().to_string());
        }
        if demo {
            return Ok(self.demo.lock().unwrap().normalize(path));
        }
        match path.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => Ok(format!("{}{rest}", self.node_user(place).await?.home)),
            _ => Ok(path.to_string()),
        }
    }

    /// Each name in `dir` on a place, with its facts (None: not there), and the free space there.
    pub(super) async fn sides(&self, demo: bool, place: &str, dir: &str, names: &[String]) -> Result<(Vec<Option<Side>>, Option<u64>), String> {
        if place == MAC {
            let d = mac::path(dir);
            let free = mac::space(&d).0;
            return Ok((names.iter().map(|n| mac::side(&d, n)).collect(), free));
        }
        if demo {
            let free = self.demo_free(place).0;
            let mut fs = self.demo.lock().unwrap();
            return Ok((names.iter().map(|n| fs.side(place, dir, n)).collect(), Some(free)));
        }
        let args: Vec<String> = names.iter().map(|n| q(n)).collect();
        let v = self.live.sparkjob(place, &format!("summary {} {}", q(dir), args.join(" ")), None, Duration::from_secs(300)).await?;
        let items = v["items"].as_array().cloned().unwrap_or_default();
        let sides = names
            .iter()
            .map(|n| {
                let i = items.iter().find(|i| i["name"].as_str() == Some(n.as_str()))?;
                i["exists"].as_bool().filter(|e| *e)?;
                Some(Side {
                    kind: i["kind"].as_str().unwrap_or("file").into(),
                    bytes: i["bytes"].as_u64().unwrap_or(0),
                    entries: i["entries"].as_u64().unwrap_or(1),
                    mtime: i["mtime"].as_u64().unwrap_or(0),
                    tar_bytes: i["tarBytes"].as_u64().unwrap_or(0),
                })
            })
            .collect();
        Ok((sides, v["free"].as_u64()))
    }

    /// What a transfer will involve: sizes, the names already taken at the destination (with
    /// both sides' size and date), and whether it fits.
    pub async fn preflight(&self, demo: bool, spec: &TransferSpec) -> Result<Preflight, String> {
        validate(spec)?;
        self.check_mac_landing(demo, spec)?;
        let dir = self.expand(demo && spec.from != MAC, &spec.from, &spec.dir).await?;
        let to_dir = self.expand(demo && spec.to != MAC, &spec.to, &spec.to_dir).await?;
        let (src, _) = self.sides(demo && spec.from != MAC, &spec.from, &dir, &spec.names).await?;
        let (dst, free) = self.sides(demo && spec.to != MAC, &spec.to, &to_dir, &spec.names).await?;
        let mut conflicts = Vec::new();
        let (mut bytes, mut entries) = (0, 0);
        for ((name, s), d) in spec.names.iter().zip(src).zip(dst) {
            let s = s.ok_or_else(|| format!("{name} is no longer in {dir}."))?;
            bytes += s.bytes;
            entries += s.entries;
            if let Some(d) = d {
                conflicts.push(Conflict { name: name.clone(), source: s, dest: d });
            }
        }
        Ok(Preflight { bytes, entries, conflicts, free, route: route(spec).into() })
    }

    /// A copy onto this Mac: never from a Demo node, and only where `mac::landing` allows.
    /// Returns where it lands (None when it doesn't land on the Mac).
    pub fn check_mac_landing(&self, demo: bool, spec: &TransferSpec) -> Result<Option<Landing>, String> {
        if spec.to != MAC {
            return Ok(None);
        }
        if demo && spec.from != MAC {
            return Err(DEMO_TO_MAC.into());
        }
        mac::landing(&self.mac_home(), &spec.to_dir, &spec.names).map(Some)
    }

    pub async fn transfer(self: &Arc<Self>, demo: bool, spec: TransferSpec, emit: Emit) -> Result<String, String> {
        validate(&spec)?;
        self.check_mac_landing(demo, &spec)?;
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = format!("x{n}");
        let mut salt = [0u8; 4];
        let _ = getrandom::fill(&mut salt);
        let nonce = format!("{n}-{:08x}", u32::from_le_bytes(salt));
        let (tx, mut rx) = watch::channel(false);
        self.transfers.lock().unwrap().insert(id.clone(), tx);
        let simulated = demo && (spec.from != MAC || spec.to != MAC);
        let me = self.clone();
        let done_id = id.clone();
        self.running.lock().unwrap().insert(nonce.clone());
        tauri::async_runtime::spawn(async move {
            // What was put in place, kept even when a later step fails (a move's removal), so
            // the activity log lists whatever landed on this Mac, whatever the outcome.
            let placed_any = Mutex::new(Vec::new());
            let outcome = if simulated { me.run_demo(&spec, &emit, &mut rx).await } else { me.run_live(&spec, &nonce, &emit, &mut rx, &placed_any).await };
            me.running.lock().unwrap().remove(&nonce);
            let placed_on_mac = if spec.to == MAC { placed_any.into_inner().unwrap() } else { Vec::new() };
            if simulated {
                let (what, verb) = (spec.names.join(", "), if spec.op == Op::Move { "Move" } else { "Copy" });
                let node = if spec.to != MAC { spec.to.clone() } else { spec.from.clone() };
                let done = if outcome.is_ok() { "" } else { " (stopped)" };
                me.live.audit.record_simulated(&node, &format!("{verb} {what} from {} to {}{done}", place_label(&spec.from), place_label(&spec.to)), &format!("tar -C {} -cf - … | tar -C {} -xf -", q(&spec.dir), q(&spec.to_dir)));
            }
            me.retry_leftovers();
            emit(match outcome {
                Ok((bytes, files, placed)) => TransferEvent::Done { bytes, files, placed },
                Err(Stop::Cancelled) => TransferEvent::Cancelled,
                Err(Stop::Failed(message)) => TransferEvent::Failed { message },
            });
            me.transfers.lock().unwrap().remove(&done_id);
            if !placed_on_mac.is_empty() {
                let me2 = me.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || me2.audit_mac_placement(&spec, &placed_on_mac)).await;
            }
        });
        Ok(id)
    }

    /// One activity-log line per item put in place on this Mac: where it is now, its size and
    /// its SHA-256 (a folder's covers every file in it), so what arrived can be checked later.
    pub(super) fn audit_mac_placement(&self, spec: &TransferSpec, placed: &[Placed]) {
        let dir = mac::path(&spec.to_dir);
        let verb = if spec.op == Op::Move { "Moved" } else { "Copied" };
        let download = mac::landing(&self.mac_home(), &spec.to_dir, &spec.names).ok() == Some(Landing::Downloads);
        for p in placed.iter().filter(|p| p.action != "skipped") {
            let path = dir.join(&p.as_name);
            let (bytes, files, sha) = mac::fingerprint(&path);
            let what = if files > 1 || path.is_dir() { format!("{files} files, {bytes} bytes") } else { format!("{bytes} bytes") };
            let how = if download { " · quarantined, not executable" } else { "" };
            self.live.audit.record_app(
                &format!("{verb} {} from {} to this Mac", p.name, place_label(&spec.from)),
                &format!("{} ({}) · {what} · sha256 {sha}{how}", path.display(), p.action),
            );
        }
    }

    /// Stops a transfer; whatever had arrived is removed with its staging folder.
    pub fn cancel(&self, id: &str) {
        if let Some(tx) = self.transfers.lock().unwrap().get(id) {
            let _ = tx.send(true);
        }
    }

    async fn run_live(&self, spec: &TransferSpec, nonce: &str, emit: &Emit, cancel: &mut watch::Receiver<bool>, placed_any: &Mutex<Vec<Placed>>) -> Outcome {
        let items: Vec<(String, Option<Policy>)> = spec.names.iter().map(|n| (n.clone(), spec.policies.get(n).copied())).collect();
        let included: Vec<String> = items.iter().filter(|(_, p)| *p != Some(Policy::Skip)).map(|(n, _)| n.clone()).collect();
        let dir = self.expand(false, &spec.from, &spec.dir).await?;
        let to_dir = self.expand(false, &spec.to, &spec.to_dir).await?;
        let (sides, _) = self.sides(false, &spec.from, &dir, &included).await?;
        let bytes: u64 = sides.iter().flatten().map(|s| s.bytes).sum();
        let files_total: u64 = sides.iter().flatten().map(|s| s.entries).sum();
        let local = spec.from == spec.to;
        let mut meter = Meter::new(if local { bytes } else { sides.iter().flatten().map(|s| s.tar_bytes).sum::<u64>() + 1024 }, files_total);
        meter.report(emit, true);
        if included.is_empty() {
            let placed = items.iter().map(|(n, _)| Placed { name: n.clone(), action: "skipped".into(), as_name: n.clone() }).collect();
            return Ok((0, 0, placed));
        }
        if local && spec.op == Op::Move {
            let placed = self.place(&spec.to, &dir, &to_dir, &items, nonce, true).await?;
            return Ok((bytes, files_total, placed));
        }
        let stage = format!("{}/.sparkbox-incoming-{nonce}", to_dir.trim_end_matches('/'));
        // Leftovers of earlier copies into this folder go first.
        self.sweep(&spec.to, &to_dir).await;
        // What the receiving side must hold once the stream ends.
        let expected = self.tree(&spec.from, &dir, &included).await.map_err(Stop::Failed)?;
        let copied = if local && spec.from != MAC {
            self.node_local_copy(&spec.from, &dir, &to_dir, nonce, &included, &mut meter, emit, cancel).await
        } else {
            let verbose = Arc::new(Mutex::new(Verbose::new(&included)));
            match self.source(&spec.from, &dir, &included, &verbose).await {
                Ok(source) => match self.sink(&spec.to, &stage).await {
                    Ok(sink) => pump(source, sink, &mut meter, &verbose, emit, cancel).await,
                    Err(e) => Err(Stop::Failed(e)),
                },
                Err(e) => Err(Stop::Failed(e)),
            }
        };
        // In ~/Downloads a copy arrives like a browser download: quarantined and without execute
        // bits while still in staging, so nothing is ever in place without them.
        let download = spec.to == MAC && mac::landing(&self.mac_home(), &spec.to_dir, &spec.names).ok() == Some(Landing::Downloads);
        let placed = match copied {
            Ok(()) => match self.tree(&spec.to, &stage, &included).await.and_then(|got| verify(&expected, &got, &spec.to)) {
                Ok(()) => {
                    meter.done = meter.total;
                    meter.report(emit, true);
                    let staged: Result<(), String> = if download { included.iter().try_for_each(|n| mac::quarantine(&Path::new(&stage).join(n))) } else { Ok(()) };
                    match staged {
                        Ok(()) => self.place(&spec.to, &stage, &to_dir, &items, nonce, false).await.map_err(Stop::Failed),
                        Err(e) => Err(Stop::Failed(e)),
                    }
                }
                Err(e) => Err(Stop::Failed(e)),
            },
            Err(e) => Err(e),
        };
        self.cleanup(&spec.to, &to_dir, &stage, nonce, emit).await;
        let placed = placed?;
        *placed_any.lock().unwrap() = placed.clone();
        if spec.op == Op::Move {
            let moved: Vec<String> = placed.iter().filter(|p| p.action != "skipped").map(|p| format!("{}/{}", dir.trim_end_matches('/'), p.name)).collect();
            self.remove_paths(false, &spec.from, &moved).await?;
        }
        Ok((bytes, meter.files.max(files_total), placed))
    }

    async fn source(&self, from: &str, dir: &str, names: &[String], verbose: &Arc<Mutex<Verbose>>) -> Result<Source, String> {
        if from == MAC {
            let mut args = vec!["-C".to_string(), mac::path(dir).display().to_string(), "-cvf".into(), "-".into(), "--".into()];
            args.extend(names.iter().cloned());
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let mut child = mac_tar(&refs).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("can't run tar: {e}"))?;
            let stderr = child.stderr.take().ok_or("tar has no stderr")?;
            let v = verbose.clone();
            tauri::async_runtime::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    v.lock().unwrap().feed(format!("{line}\n").as_bytes());
                }
            });
            Ok(Source::Mac(child))
        } else {
            let quoted: Vec<String> = names.iter().map(|n| q(n)).collect();
            let exec = self.live.exec_io(from, &format!("tar -C {} -cvf - -- {}", q(dir), quoted.join(" "))).await?;
            exec.write.eof().await.map_err(|e| e.to_string())?; // tar -c reads nothing
            Ok(Source::Node(exec, from.to_string(), None))
        }
    }

    async fn sink(&self, to: &str, stage: &str) -> Result<Sink, String> {
        if to == MAC {
            let dir = PathBuf::from(stage);
            std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
            let child = mac_tar(&["-C", stage, "-xf", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("can't run tar: {e}"))?;
            Ok(Sink::Mac(child))
        } else {
            let exec = self.live.exec_io(to, &format!("mkdir -p -- {d} && tar -C {d} -xf -", d = q(stage))).await?;
            Ok(Sink::Node(exec, to.to_string()))
        }
    }

    /// Copies within one node, on the node (sparkjob.py cp), reading its progress lines.
    #[allow(clippy::too_many_arguments)]
    async fn node_local_copy(&self, node: &str, dir: &str, to_dir: &str, nonce: &str, names: &[String], meter: &mut Meter, emit: &Emit, cancel: &mut watch::Receiver<bool>) -> Result<(), Stop> {
        self.live.ensure_file(node, "sparkjob.py", SPARKJOB).await?;
        let quoted: Vec<String> = names.iter().map(|n| q(n)).collect();
        let mut exec = self.live.exec_io(node, &format!("python3 ~/.sparkbox/sparkjob.py cp {} {} {} {}", q(dir), q(to_dir), q(nonce), quoted.join(" "))).await?;
        exec.write.eof().await.map_err(|e| e.to_string())?;
        let (mut line, mut stderr, mut code) = (String::new(), String::new(), None);
        loop {
            let msg = tokio::select! {
                biased;
                _ = cancelled(cancel) => return Err(Stop::Cancelled),
                m = exec.read.wait() => m,
            };
            match msg {
                Some(ChannelMsg::Data { data }) => {
                    line.push_str(&String::from_utf8_lossy(&data));
                    while let Some(i) = line.find('\n') {
                        let l: String = line.drain(..=i).collect();
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(l.trim()) {
                            meter.done = v["done"].as_u64().unwrap_or(meter.done);
                            meter.files = v["files"].as_u64().unwrap_or(meter.files);
                            meter.current = v["current"].as_str().filter(|s| !s.is_empty()).map(String::from).or(meter.current.take());
                            meter.report(emit, false);
                        }
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => stderr.push_str(&String::from_utf8_lossy(&data)),
                Some(ChannelMsg::ExitStatus { exit_status }) => code = Some(exit_status),
                Some(ChannelMsg::ExitSignal { signal_name, .. }) => return Err(Stop::Failed(format!("the copy on {node} was killed (SIG{})", sig_name(&signal_name)))),
                Some(ChannelMsg::Close) | None => break,
                _ => {}
            }
        }
        match code {
            Some(0) => Ok(()),
            None => Err(Stop::Failed(format!("The copy on {node} ended without reporting success"))),
            _ => Err(Stop::Failed(format!("copying on {node} failed: {}", stderr.trim().lines().last().unwrap_or("it stopped")))),
        }
    }

    async fn place(&self, place: &str, source: &str, dest: &str, items: &[(String, Option<Policy>)], nonce: &str, moving: bool) -> Result<Vec<Placed>, String> {
        if place == MAC {
            return mac::place(Path::new(source), Path::new(dest), items, nonce, moving);
        }
        let spec = serde_json::json!({
            "items": items.iter().map(|(n, p)| serde_json::json!({"name": n, "policy": p})).collect::<Vec<_>>(),
            "nonce": nonce,
            "move": moving,
        });
        let v = self.live.sparkjob(place, &format!("place {} {}", q(source), q(dest)), Some(spec.to_string().as_bytes()), Duration::from_secs(600)).await?;
        serde_json::from_value(v).map_err(|e| format!("{place}: unexpected reply from sparkjob ({e})"))
    }

    /// Removes a transfer's staging folder. When that fails (the node went away), the window is
    /// told what was left, and the removal is tried again with the node back and Admin mode
    /// unlocked (`retry_leftovers`).
    async fn cleanup(&self, place: &str, to_dir: &str, stage: &str, nonce: &str, emit: &Emit) {
        let result = self.remove_stage(place, to_dir, stage, nonce).await;
        if let Err(why) = result {
            let left = Leftover { place: place.to_string(), dir: to_dir.to_string(), nonce: nonce.to_string(), path: stage.to_string(), bytes: None, why };
            emit(TransferEvent::Leftover { place: left.place.clone(), path: left.path.clone(), bytes: left.bytes });
            self.keep_leftover(left);
        }
    }

    pub(super) async fn remove_stage(&self, place: &str, to_dir: &str, stage: &str, nonce: &str) -> Result<(), String> {
        if place == MAC {
            match std::fs::remove_dir_all(stage) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("can't remove {stage}: {e}")),
            }
        } else {
            self.live.sparkjob(place, &format!("cleanup {} {}", q(to_dir), q(nonce)), None, Duration::from_secs(120)).await.map(|_| ())
        }
    }

    /// Every entry under the named items: (kind, size) by relative path.
    async fn tree(&self, place: &str, dir: &str, names: &[String]) -> Result<BTreeMap<String, (String, u64)>, String> {
        if place == MAC {
            return Ok(mac_tree(&mac::path(dir), names));
        }
        let args: Vec<String> = names.iter().map(|n| q(n)).collect();
        let v = self.live.sparkjob(place, &format!("tree {} {}", q(dir), args.join(" ")), None, Duration::from_secs(600)).await?;
        let map = v.as_object().ok_or_else(|| format!("{place}: unexpected reply from sparkjob tree"))?;
        Ok(map
            .iter()
            .map(|(k, e)| (k.clone(), (e[0].as_str().unwrap_or("f").to_string(), e[1].as_u64().unwrap_or(0))))
            .collect())
    }

    /// Removes staging folders in `dir` that no running copy owns (sparkjob sweep, or here on the Mac).
    pub(super) async fn sweep(&self, place: &str, dir: &str) {
        let keep: Vec<String> = self.live_nonces();
        if place == MAC {
            mac_sweep(&mac::path(dir), &keep);
            return;
        }
        let args: Vec<String> = keep.iter().map(|k| q(k)).collect();
        let _ = self.live.sparkjob(place, &format!("sweep {} {}", q(dir), args.join(" ")), None, Duration::from_secs(120)).await;
    }

    /// The simulated transfer: real sizes and file names from the demo world, at the speed the
    /// route would manage, then put in place by the same policies. Stopping places nothing.
    async fn run_demo(&self, spec: &TransferSpec, emit: &Emit, cancel: &mut watch::Receiver<bool>) -> Outcome {
        let items: Vec<(String, Option<Policy>)> = spec.names.iter().map(|n| (n.clone(), spec.policies.get(n).copied())).collect();
        let dir = self.expand(spec.from != MAC, &spec.from, &spec.dir).await?;
        let to_dir = self.expand(spec.to != MAC, &spec.to, &spec.to_dir).await?;
        let mut trees: Vec<(String, Option<Policy>, Vec<(String, Item)>)> = Vec::new();
        for (name, policy) in &items {
            if *policy == Some(Policy::Skip) {
                continue;
            }
            let tree = if spec.from == MAC {
                let t = mac_export(Path::new(&dir), name);
                if t.is_empty() {
                    return Err(Stop::Failed(format!("{name} is no longer in {dir}.")));
                }
                t
            } else {
                self.demo.lock().unwrap().export(&spec.from, &dir, name)?
            };
            trees.push((name.clone(), *policy, tree));
        }
        let files: Vec<(String, u64)> = trees.iter().flat_map(|(_, _, t)| t.iter().map(|(r, i)| (r.clone(), if i.dir || i.link.is_some() { 0 } else { i.size }))).collect();
        let total: u64 = files.iter().map(|(_, s)| s).sum();
        let mut meter = Meter::new(total, files.len() as u64);
        meter.report(emit, true);
        let local = spec.from == spec.to;
        if !(local && spec.op == Op::Move) {
            // 10 GbE through this Mac, or NVMe to NVMe within a node.
            let rate = if local { 2.6e9 } else { 1.15e9 };
            let started = Instant::now();
            let seconds = (total as f64 / rate).max(0.6);
            loop {
                tokio::select! {
                    biased;
                    _ = cancelled(cancel) => return Err(Stop::Cancelled),
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                }
                let frac = (started.elapsed().as_secs_f64() / seconds).min(1.0);
                meter.done = (total as f64 * frac) as u64;
                let mut acc = 0;
                let mut k = 0;
                for (i, (_, s)) in files.iter().enumerate() {
                    k = i;
                    acc += s;
                    if acc >= meter.done && *s > 0 {
                        break;
                    }
                }
                meter.files = if frac >= 1.0 { files.len() as u64 } else { k as u64 };
                meter.current = files.get(k).map(|(r, _)| r.clone());
                meter.report(emit, frac >= 1.0);
                if frac >= 1.0 {
                    break;
                }
            }
        }
        // Demo files never land on this Mac (check_mac_landing refuses it before this runs).
        if spec.to == MAC {
            return Err(Stop::Failed(DEMO_TO_MAC.into()));
        }
        let mut placed = Vec::new();
        {
            let mut fs = self.demo.lock().unwrap();
            for (name, policy) in &items {
                match trees.iter().find(|(n, _, _)| n == name) {
                    Some((_, _, tree)) => {
                        if local && spec.op == Op::Move && *policy != Some(Policy::Skip) && !fs.exists(&spec.to, &format!("{to_dir}/{name}")) {
                            fs.rename(&spec.to, &format!("{dir}/{name}"), &format!("{to_dir}/{name}"))?;
                            placed.push(Placed { name: name.clone(), action: "copied".into(), as_name: name.clone() });
                            continue;
                        }
                        let p = fs.place(&spec.to, &to_dir, name, tree.clone(), *policy)?;
                        if spec.from == MAC && p.action != "skipped" {
                            self.mac_copies.lock().unwrap().push((spec.to.clone(), format!("{}/{}", to_dir.trim_end_matches('/'), p.as_name)));
                        }
                        placed.push(p);
                    }
                    None => placed.push(Placed { name: name.clone(), action: "skipped".into(), as_name: name.clone() }),
                }
            }
        }
        if spec.op == Op::Move {
            let moved: Vec<String> = placed
                .iter()
                .filter(|p| p.action != "skipped")
                .map(|p| format!("{}/{}", dir.trim_end_matches('/'), p.name))
                .collect();
            for p in moved {
                if spec.from == MAC {
                    mac::remove(&p)?;
                } else {
                    let mut fs = self.demo.lock().unwrap();
                    if fs.exists(&spec.from, &p) {
                        fs.remove(&spec.from, &p)?;
                    }
                }
            }
        }
        Ok((total, files.len() as u64, placed))
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn tar_verbose_lines_name_the_file_in_flight() {
        let names = vec!["project".to_string(), "a b.txt".to_string()];
        let mut gnu = Verbose::new(&names);
        gnu.feed(b"project/\nproject/sub/da");
        gnu.feed(b"ta.bin\ntar: project/x: file changed as we read it\na b.txt\n");
        assert_eq!(gnu.files, 3);
        assert_eq!(gnu.current.as_deref(), Some("a b.txt"), "GNU tar: a name starting with 'a ' is kept whole");
        assert!(gnu.errors.contains("file changed"));
        let mut bsd = Verbose::new(&names);
        bsd.feed(b"a project\na project/run.sh\n");
        assert_eq!((bsd.files, bsd.current.as_deref()), (2, Some("project/run.sh")));
    }

    #[test]
    fn a_folder_cant_go_inside_itself() {
        let spec = |to_dir: &str| TransferSpec {
            from: "cell1".into(),
            dir: "/home/u".into(),
            names: vec!["models".into()],
            to: "cell1".into(),
            to_dir: to_dir.into(),
            op: Op::Move,
            policies: HashMap::new(),
        };
        assert!(validate(&spec("/home/u/models/sub")).is_err());
        assert!(validate(&spec("/home/u/models")).is_err());
        assert!(validate(&spec("/home/u/models-old")).is_ok());
        assert!(validate(&TransferSpec { names: vec!["../x".into()], ..spec("/tmp") }).is_err());
    }
}
