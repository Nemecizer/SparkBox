//! Files window: browse, edit, copy and trash files on this Mac and on every node.
//!
//! - Saves are atomic and conflict-aware: the editor sends the SHA-256 of the text it opened,
//!   and a file that changed since is reported instead of overwritten. On a node the new text
//!   is staged next to the file, given its mode and owner, checked (visudo for sudoers) and
//!   renamed over it, keeping the previous five versions in ~/.sparkbox/backups.
//! - Delete moves to the Trash (NSFileManager on the Mac, the freedesktop Trash on a node) so
//!   Undo can put it back; deleting for good is a separate, confirmed action. System folders
//!   and the home folder are refused after their paths are normalised.
//! - Copies and moves are tar streams through SparkBox with conflict policies, verified before
//!   anything is put in place (transfer.rs).
//! - A save as root runs SparkBox's own root-owned, checksum-verified sparkjob.py with
//!   `python3 -I` (Live::sparkjob_root), never the copy in the cluster user's home.
//! - Every change on this Mac (save, rename, delete, Trash, permissions, new folder, new file, a
//!   copy put in place) goes in the activity log with the path, the size and the SHA-256; in
//!   Demo mode, every simulated change on a node does too. A change on a node runs as one titled
//!   operation ("Save notes.txt on cell2"), so its rows, a refusal included, say what it was.
//! - New file makes an empty file only where nothing is (a link counts), never over one: on a
//!   node it's sparkjob's commit with `--expect absent`.
//! - A copy onto this Mac needs Mac scope, except into ~/Downloads, where it arrives like a
//!   browser download: quarantined, without execute bits. Nothing is copied into login items,
//!   shell startup files or ~/.ssh, and only into the home folder or /Volumes. Reading ~/.ssh,
//!   the Keychains, ~/.gnupg, ~/.aws, ~/.config/gh or ~/.kube needs Mac scope.
//!
//! In Demo mode the nodes have a simulated filesystem (demo.rs); the Mac side is always real,
//! so a Demo node's files are never copied onto it.

pub mod commands;
pub(crate) mod demo;
pub(crate) mod mac;
pub mod paths;
pub mod transfer;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::live::{audit, sh_quote as q, Live};
pub use transfer::{Placed, Preflight, TransferSpec};

pub const MAC: &str = "mac";
pub const TEXT_LIMIT: u64 = 2 * 1024 * 1024;
/// What a copy from a Demo node onto this Mac is refused with.
pub const DEMO_TO_MAC: &str = "Demo nodes are simulated: their files can't be copied to this Mac.";
/// What opening a Demo node's copy of a Mac file says: Demo never reads files on this Mac.
pub const DEMO_MAC_COPY: &str = "Demo copies don't carry the contents of files on your Mac.";
/// What New file says on a Demo node (it has no simulation).
pub const LIVE_ONLY: &str = "Available with Live data";
/// The SHA-256 of nothing: a new file's content.
const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    /// file | dir | link
    pub kind: String,
    pub size: u64,
    pub mtime: u64,
    /// `ls -l` style, e.g. "-rw-r--r--".
    pub mode: String,
    pub target: Option<String>,
    pub link_dir: bool,
    #[serde(default)]
    pub owner: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub place: String,
    pub path: String,
    pub entries: Vec<Entry>,
    /// Free and total bytes of the filesystem the folder is on.
    #[serde(default)]
    pub free: Option<u64>,
    #[serde(default)]
    pub capacity: Option<u64>,
}

/// A text file opened for editing, with what a save needs to notice changes made meanwhile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFile {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub mtime: u64,
    pub sha256: String,
    pub mode: String,
    /// "user:group"
    pub owner: String,
    /// The cluster user can save it without sudo.
    pub writable: bool,
    /// Over 2 MB: `content` is empty and `head` / `tail` hold its first and last 64 KB.
    pub large: bool,
    pub head: Option<String>,
    pub tail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub path: String,
    pub sha256: String,
    pub mtime: u64,
    pub size: u64,
    /// Where the previous version was kept.
    pub backup: Option<String>,
}

/// Why a save didn't happen, typed so the editor can offer Reload / Show diff / Overwrite.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SaveError {
    /// The file changed (or was deleted: no sha256) after it was opened.
    ChangedOnDisk { message: String, sha256: Option<String>, mtime: Option<u64>, size: Option<u64> },
    /// The cluster user can't write it (or it belongs to someone else); sudo can.
    NeedsSudo { message: String },
    Failed { message: String },
}

impl From<String> for SaveError {
    fn from(message: String) -> SaveError {
        SaveError::Failed { message }
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::ChangedOnDisk { message, .. } | SaveError::NeedsSudo { message } | SaveError::Failed { message } => f.write_str(message),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub path: String,
    pub mtime: u64,
    pub size: u64,
}

/// An item moved to the Trash: where it was, and where it went (None when only Finder knows).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Trashed {
    pub path: String,
    pub trashed: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub path: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TransferEvent {
    /// `done` / `total` bytes; `files` items gone so far; `current` the one in flight (from `tar -v`).
    Progress { done: u64, total: u64, files: u64, files_total: u64, current: Option<String> },
    Done { bytes: u64, files: u64, placed: Vec<Placed> },
    Failed { message: String },
    /// Stopped by the user; nothing was left at the destination.
    Cancelled,
    /// The staging folder couldn't be removed (the node went away); SparkBox tries again once
    /// it's back. Comes before the transfer's last event.
    Leftover { place: String, path: String, bytes: Option<u64> },
}

/// A staging folder a transfer couldn't remove.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Leftover {
    pub place: String,
    /// The folder it's in, and the transfer's id (what sparkjob cleanup takes).
    pub dir: String,
    pub nonce: String,
    pub path: String,
    pub bytes: Option<u64>,
    pub why: String,
}

/// "12.0 GiB", "1.5 MiB", "312 B": binary units, one decimal.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

pub type Emit = Box<dyn Fn(TransferEvent) + Send + Sync>;

/// The cluster user on a node, looked up once.
#[derive(Clone, Debug)]
pub(crate) struct NodeUser {
    pub home: String,
    pub uid: u32,
    pub gid: u32,
}

/// The first 12 hex digits of a SHA-256, for the activity log.
fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

fn count_items(paths: &[String]) -> String {
    match paths {
        [one] => base(one).to_string(),
        many => format!("{} items", many.len()),
    }
}

/// "notes.txt" for "/home/sparkuser/sparkbox-test/notes.txt".
fn base(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// What New file says when something is already at the name.
pub fn already_there(name: &str) -> String {
    format!("There's already something called {name} there.")
}

/// `mv` that refuses to replace: every name quoted, so quotes, `$(…)`, backticks, spaces and a
/// leading `-` stay literal.
pub fn rename_command(path: &str, to: &str) -> String {
    format!("if [ -e {t} ] || [ -L {t} ]; then printf '%s already exists\\n' {t} >&2; exit 1; fi; mv -- {} {t}", q(path), t = q(to))
}

pub(crate) fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(format!("“{name}” isn't a usable file name."));
    }
    Ok(())
}

/// A read refused because only root (or the owner) can read the file, as one JSON string the
/// editor turns into "Only root can read 40-cx7.yaml (root, 600)" and Read with sudo….
pub fn permission_refusal(place: &str, path: &str, mode: &str, owner: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let facts = match (owner.is_empty(), mode.is_empty()) {
        (false, false) => format!(" ({owner}, {mode})"),
        (false, true) => format!(" ({owner})"),
        _ => String::new(),
    };
    let who = if owner.is_empty() || owner == "root" { "root".to_string() } else { owner.to_string() };
    // In this order, kind first, as every other refusal sent as a string (errors.rs).
    #[derive(serde::Serialize)]
    struct Refusal<'a> {
        kind: &'a str,
        message: String,
        node: &'a str,
        detail: String,
    }
    serde_json::to_string(&Refusal {
        kind: "permission",
        message: format!("Only {who} can read {name}{facts}. Reading it needs sudo, which needs Admin."),
        node: place,
        detail: serde_json::json!({ "path": path, "mode": mode, "owner": owner }).to_string(),
    })
    .unwrap_or_default()
}

/// The activity-log title of removing a staging folder a copy left behind.
fn leftover_title(place: &str) -> String {
    format!("Remove the staging folder left on {}", transfer::place_name(place))
}

fn parent_and_name(path: &str) -> Result<(String, String), String> {
    let p = std::path::Path::new(path.trim_end_matches('/'));
    let name = p.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("{path} has no name"))?;
    let parent = p.parent().and_then(|p| p.to_str()).filter(|p| !p.is_empty()).unwrap_or("/");
    Ok((parent.to_string(), name.to_string()))
}

fn sha_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Free and total bytes of a demo node's root disk, from the simulated cluster (so the Files
/// footer agrees with the Dashboard, disk-filling scenario included).
pub type SpaceFn = Box<dyn Fn(&str) -> Option<(u64, u64)> + Send + Sync>;

pub struct Files {
    live: Arc<Live>,
    demo: Arc<Mutex<demo::DemoFs>>,
    demo_space: Mutex<Option<SpaceFn>>,
    next: AtomicU64,
    transfers: Mutex<HashMap<String, watch::Sender<bool>>>,
    users: Mutex<HashMap<String, NodeUser>>,
    /// Previous versions of Mac files saved in the editor.
    backups: PathBuf,
    /// Staging ids of the transfers running now (a sweep leaves them alone).
    running: Mutex<HashSet<String>>,
    /// Staging folders that couldn't be removed yet.
    leftovers: Mutex<Vec<Leftover>>,
    retrying: AtomicBool,
    /// How often the removal of a leftover is tried again.
    retry_every: Mutex<Duration>,
    /// The Admin gate: the leftover sweep waits for Admin mode, and pauses in look-only mode.
    gate: Mutex<Option<Arc<crate::admin::AdminGate>>>,
    /// The home folder copies onto this Mac are checked against (a temp folder in tests).
    mac_home: Mutex<PathBuf>,
    /// Where Delete on this Mac puts things (the real Trash; a folder in tests).
    mac_trash: Mutex<Arc<dyn mac::Trash>>,
    /// Demo copies of Mac files, by (node, path): they carry names and sizes, never contents.
    mac_copies: Mutex<Vec<(String, String)>>,
}

impl Files {
    pub fn new(live: Arc<Live>, backups: PathBuf) -> Arc<Files> {
        Arc::new(Files {
            live,
            demo: Arc::new(Mutex::new(demo::DemoFs::new())),
            demo_space: Mutex::new(None),
            next: AtomicU64::new(1),
            transfers: Mutex::new(HashMap::new()),
            users: Mutex::new(HashMap::new()),
            backups,
            running: Mutex::new(HashSet::new()),
            leftovers: Mutex::new(Vec::new()),
            retrying: AtomicBool::new(false),
            retry_every: Mutex::new(Duration::from_secs(15)),
            gate: Mutex::new(None),
            mac_home: Mutex::new(mac::home()),
            mac_trash: Mutex::new(Arc::new(mac::SystemTrash)),
            mac_copies: Mutex::new(Vec::new()),
        })
    }

    #[cfg(test)]
    pub fn set_mac_trash(&self, bin: Arc<dyn mac::Trash>) {
        *self.mac_trash.lock().unwrap() = bin;
    }

    /// The home folder that copies onto this Mac must land in (tests point it at a temp folder).
    pub fn mac_home(&self) -> PathBuf {
        self.mac_home.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub fn set_mac_home(&self, home: PathBuf) {
        *self.mac_home.lock().unwrap() = home;
    }

    /// Whether `path` on `place` holds keys or credentials: a key folder or file on this Mac, a
    /// node secret (~/.ssh, ~/hf_token.txt, ~/glm53/api_key…) on a node. Listing and reading it
    /// needs Mac scope (P1.46).
    pub fn is_secret(&self, place: &str, path: &str) -> bool {
        mac::is_secret_at(place, path, &self.mac_home())
    }

    /// Staging folders transfers couldn't remove (Files shows them with Remove).
    pub fn leftovers(&self) -> Vec<Leftover> {
        self.leftovers.lock().unwrap().clone()
    }

    pub(super) fn live_nonces(&self) -> Vec<String> {
        self.running.lock().unwrap().iter().cloned().collect()
    }

    pub(super) fn keep_leftover(&self, left: Leftover) {
        let mut l = self.leftovers.lock().unwrap();
        if !l.iter().any(|x| x.path == left.path) {
            l.push(left);
        }
    }

    /// Removes a leftover staging folder now (Files' Remove button).
    pub async fn remove_leftover(&self, place: &str, path: &str) -> Result<(), String> {
        let left = self.leftovers.lock().unwrap().iter().find(|l| l.place == place && l.path == path).cloned().ok_or("SparkBox doesn't know that staging folder.")?;
        self.titled(leftover_title(place), self.remove_stage(&left.place, &left.dir, &left.path, &left.nonce)).await?;
        self.leftovers.lock().unwrap().retain(|l| l.path != path);
        Ok(())
    }

    pub fn set_gate(&self, gate: Arc<crate::admin::AdminGate>) {
        *self.gate.lock().unwrap() = Some(gate);
    }

    /// SparkBox may delete on a node by itself now: Admin mode is unlocked and look-only is off.
    /// This reads the gate and leaves it as it is: `check` would hold Admin open for as long as a
    /// leftover waited. No gate yet (the app hands it over at startup): no.
    fn may_tidy(&self) -> bool {
        self.gate.lock().unwrap().as_ref().is_some_and(|g| !g.look_only() && g.state().unlocked)
    }

    /// Keeps trying to remove leftovers whose node is connected again, until none are left.
    /// It waits for Admin mode, as a push of the alert settings does (`agent::auto_push_due`): the
    /// copy that left the folder ended long ago, nobody is there to confirm the delete, and the
    /// sweep reaches every idle staging folder in that directory, not only this copy's. It pauses
    /// while look-only is on, and goes on once Admin mode is unlocked again. The Remove button
    /// is there meanwhile, and the next copy into that folder sweeps it too.
    pub(super) fn retry_leftovers(self: &Arc<Self>) {
        if self.leftovers.lock().unwrap().is_empty() || self.retrying.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let every = *me.retry_every.lock().unwrap();
                tokio::time::sleep(every).await;
                let list = me.leftovers();
                if list.is_empty() {
                    break;
                }
                for l in list {
                    if !me.may_tidy() {
                        break;
                    }
                    if l.place != MAC && !me.live.is_connected(&l.place) {
                        continue;
                    }
                    let removed = me.titled_by("system", leftover_title(&l.place), async {
                        let removed = me.remove_stage(&l.place, &l.dir, &l.path, &l.nonce).await.is_ok();
                        // The node is back: sweep the rest of that folder too, unless Admin mode
                        // locked meanwhile. Then the next copy into that folder sweeps it.
                        if removed && me.may_tidy() {
                            me.sweep(&l.place, &l.dir).await;
                        }
                        removed
                    });
                    if removed.await {
                        me.leftovers.lock().unwrap().retain(|x| x.path != l.path);
                    }
                }
            }
            me.retrying.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    #[cfg(test)]
    pub fn set_retry_every(&self, every: Duration) {
        *self.retry_every.lock().unwrap() = every;
    }

    /// A change on this Mac, in the activity log: what, where, how much, and the checksums.
    fn audit_mac(&self, title: &str, detail: String) {
        self.live.audit.record_app(title, &detail);
    }

    /// A simulated change on a demo node, in the activity log as what would have run.
    fn audit_demo(&self, node: &str, title: &str, command: &str) {
        self.live.audit.record_simulated(node, title, command);
    }

    /// Runs a change on a node as one titled operation ("Save notes.txt on cell2"), so every row
    /// it writes, a refusal included, says what it was for. Work already inside an operation (a
    /// copy's steps) stays in that one.
    async fn titled<T>(&self, title: String, work: impl std::future::Future<Output = T>) -> T {
        self.titled_by("ui", title, work).await
    }

    /// `titled`, saying who started it: "ui", or "system" for SparkBox's own work.
    async fn titled_by<T>(&self, source: &str, title: String, work: impl std::future::Future<Output = T>) -> T {
        if audit::current_op().is_some() {
            return work.await;
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut salt = [0u8; 4];
        let _ = getrandom::fill(&mut salt);
        audit::with_op_id(format!("files-{n}{:08x}", u32::from_le_bytes(salt)), audit::OpMeta { title, source: source.into() }, work).await
    }

    /// The demo filesystems, shared with the demo Terminal so `ls` and the Files window agree.
    pub fn demo_fs(&self) -> Arc<Mutex<demo::DemoFs>> {
        self.demo.clone()
    }

    pub fn set_demo_space(&self, f: SpaceFn) {
        *self.demo_space.lock().unwrap() = Some(f);
    }

    /// A demo node's free and total bytes: the simulated cluster's, else the demo filesystem's own.
    pub(super) fn demo_free(&self, node: &str) -> (u64, u64) {
        if let Some((free, total)) = self.demo_space.lock().unwrap().as_ref().and_then(|f| f(node)) {
            return (free, total);
        }
        self.demo.lock().unwrap().space(node)
    }

    fn simulated(demo: bool, place: &str) -> bool {
        demo && place != MAC
    }

    /// The cluster user's home (Live::home_dir), uid and gid on a node: never assumed, asked once and kept.
    pub(crate) async fn node_user(&self, node: &str) -> Result<NodeUser, String> {
        if let Some(u) = self.users.lock().unwrap().get(node) {
            return Ok(u.clone());
        }
        let home = self.live.home_dir(node).await.ok_or_else(|| format!("{node} didn't say where the home folder is."))?;
        let out = self.live.probe(node, "id -u; id -g", Duration::from_secs(20)).await?;
        let ids: Vec<u32> = out.stdout.lines().filter_map(|l| l.trim().parse().ok()).collect();
        let user = NodeUser { home, uid: ids.first().copied().unwrap_or(1000), gid: ids.get(1).copied().unwrap_or(1000) };
        self.users.lock().unwrap().insert(node.to_string(), user.clone());
        Ok(user)
    }

    pub async fn list(&self, demo: bool, place: &str, path: &str) -> Result<Listing, String> {
        if place == MAC {
            return mac::list(path);
        }
        if demo {
            let mut listing = self.demo.lock().unwrap().list(place, path)?;
            let (free, total) = self.demo_free(place);
            (listing.free, listing.capacity) = (Some(free), Some(total));
            return Ok(listing);
        }
        let v = self.live.sparkjob(place, &format!("ls {}", q(if path.is_empty() { "~" } else { path })), None, Duration::from_secs(30)).await?;
        Ok(Listing {
            place: place.into(),
            path: v["path"].as_str().unwrap_or(path).into(),
            entries: serde_json::from_value(v["entries"].clone()).map_err(|e| e.to_string())?,
            free: v["free"].as_u64(),
            capacity: v["capacity"].as_u64(),
        })
    }

    /// Reads a text file. A file only root can read comes back as `permission_refusal` (the
    /// editor explains it and offers Read with sudo…), in Demo and Live alike.
    pub async fn read(&self, demo: bool, place: &str, path: &str) -> Result<TextFile, String> {
        if place == MAC {
            return mac::read(path);
        }
        if demo {
            let mut fs = self.demo.lock().unwrap();
            let real = fs.normalize(path);
            if self.mac_copies.lock().unwrap().iter().any(|(n, p)| n == place && (real == *p || real.starts_with(&format!("{p}/")))) {
                return Err(DEMO_MAC_COPY.into());
            }
            return fs.read(place, path).map_err(|e| {
                if !e.contains("Permission denied") {
                    return e;
                }
                let (dir, name) = parent_and_name(&real).unwrap_or_default();
                let item = fs.export(place, &dir, &name).ok().and_then(|items| items.into_iter().next()).map(|(_, i)| i);
                permission_refusal(place, &real, &item.as_ref().map(|i| format!("{:o}", i.mode & 0o7777)).unwrap_or_default(), item.as_ref().map_or("root", |i| i.owner.as_str()))
            });
        }
        let v = self.live.sparkjob(place, &format!("read {}", q(path)), None, Duration::from_secs(60)).await?;
        if v["kind"] == "permission" {
            let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            return Err(permission_refusal(place, &s("path"), &s("mode"), &s("owner")));
        }
        serde_json::from_value(v).map_err(|e| format!("{place}: unexpected reply from sparkjob ({e})"))
    }

    /// Reads a file as root (the editor's Read with sudo…): SparkBox's own root-owned, verified
    /// sparkjob.py with the password on stdin, as a save with sudo does. Node scope (lib.rs).
    pub async fn read_sudo(&self, demo: bool, place: &str, path: &str) -> Result<TextFile, String> {
        if place == MAC {
            return Err("Files on this Mac open as you; there's nothing to read with sudo.".into());
        }
        if demo {
            let mut fs = self.demo.lock().unwrap();
            let real = fs.normalize(path);
            let (dir, name) = parent_and_name(&real)?;
            let (_, item) = fs.export(place, &dir, &name)?.into_iter().next().ok_or_else(|| format!("{real}: No such file or directory"))?;
            let content = item.content.clone().ok_or_else(|| format!("{real} looks like a binary file"))?;
            drop(fs);
            self.live.audit.record_simulated(place, &format!("Read {real} as root"), &format!("sudo python3 -I /usr/local/lib/sparkbox/sparkjob.py read {}", q(&real)));
            let sha = sha_hex(content.as_bytes());
            return Ok(TextFile {
                path: real,
                size: item.size,
                mtime: item.mtime,
                sha256: sha,
                mode: demo::mode_string(&item),
                owner: format!("{0}:{0}", item.owner),
                writable: false,
                large: false,
                head: None,
                tail: None,
                content,
            });
        }
        let v = self.live.sparkjob_root(place, &format!("read {}", q(path)), Duration::from_secs(60)).await?;
        if v["kind"] == "permission" {
            let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            return Err(permission_refusal(place, &s("path"), &s("mode"), &s("owner")));
        }
        let mut file: TextFile = serde_json::from_value(v).map_err(|e| format!("{place}: unexpected reply from sparkjob ({e})"))?;
        // Saving it goes through Save with sudo, never as the cluster user.
        file.writable = false;
        Ok(file)
    }

    /// Saves a text file atomically. `expect` is the SHA-256 of the text the editor opened (None
    /// overwrites whatever is there). With `sudo`, a node file is saved as root and keeps its
    /// owner and permissions; the password and the text never share stdin.
    pub async fn write(&self, demo: bool, place: &str, path: &str, content: &str, sudo: bool, expect: Option<&str>) -> Result<Saved, SaveError> {
        if place == MAC {
            let before = mac::read(path).ok().map(|f| f.sha256);
            let saved = mac::commit(path, content, expect, &self.backups)?;
            self.audit_mac(
                &format!("Save {path}"),
                format!("{path}: {} bytes, sha256 {} → {}", content.len(), before.as_deref().map(short_sha).unwrap_or("(new)"), short_sha(&saved.sha256)),
            );
            return Ok(saved);
        }
        if demo {
            let saved = self.demo.lock().unwrap().commit(place, path, content, expect, sudo)?;
            let how = if sudo { "sudo python3 -I /usr/local/lib/sparkbox/sparkjob.py" } else { "python3 ~/.sparkbox/sparkjob.py" };
            self.audit_demo(
                place,
                &format!("Save {path}{}", if sudo { " as root" } else { "" }),
                &format!("{how} commit {} --sha256 {} ({} bytes)", q(path), short_sha(&saved.sha256), content.len()),
            );
            return Ok(saved);
        }
        let title = format!("Save {} on {place}{}", base(path), if sudo { " as root" } else { "" });
        self.titled(title, self.write_node(place, path, content, sudo, expect)).await
    }

    /// `write` on a node.
    async fn write_node(&self, place: &str, path: &str, content: &str, sudo: bool, expect: Option<&str>) -> Result<Saved, SaveError> {
        let user = self.node_user(place).await?;
        let sha = sha_hex(content.as_bytes());
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut salt = [0u8; 4];
        let _ = getrandom::fill(&mut salt);
        let nonce = format!("{n}{:08x}", u32::from_le_bytes(salt));
        let backups = format!("{}/.sparkbox/backups", user.home);
        let common = format!("--sha256 {sha} --expect {} --backups {} --nonce {nonce}", q(expect.unwrap_or("-")), q(&backups));
        let reply = if !sudo {
            self.live.sparkjob(place, &format!("commit {} {common}", q(path)), Some(content.as_bytes()), Duration::from_secs(120)).await.map_err(|e| {
                if e.contains("Permission denied") || e.contains("belongs to someone else") {
                    SaveError::NeedsSudo { message: format!("The cluster user can't write {path} on {place}. Save it with sudo instead.") }
                } else {
                    SaveError::Failed { message: e }
                }
            })?
        } else {
            // Stage the text as the cluster user (only they can read it), then let root's
            // sparkjob commit it from there: sudo's stdin carries only the password.
            let staged = format!("{}/.sparkbox/tmp/edit-{nonce}", user.home);
            let out = self.live.run(place, &format!("umask 077 && mkdir -p ~/.sparkbox/tmp && cat > {}", q(&staged)), Some(content.as_bytes()), Duration::from_secs(120)).await?;
            if out.code != Some(0) {
                return Err(SaveError::Failed { message: format!("couldn't stage the file on {place}: {}", out.stderr.trim()) });
            }
            // Root runs SparkBox's own root-owned, verified copy, never the one in the user's home.
            let args = format!("commit {} {common} --from {} --owner {}:{}", q(path), q(&staged), user.uid, user.gid);
            let reply = self.live.sparkjob_root(place, &args, Duration::from_secs(120)).await.map_err(|e| format!("sudo couldn't save {path} on {place}: {}", e.trim_start_matches(&format!("{place}: "))));
            let _ = self.live.run(place, &format!("rm -f {}", q(&staged)), None, Duration::from_secs(20)).await;
            reply?
        };
        if reply["conflict"].as_bool() == Some(true) {
            return Err(SaveError::ChangedOnDisk {
                message: format!("{path} on {place} changed on disk after you opened it."),
                sha256: reply["sha256"].as_str().map(String::from),
                mtime: reply["mtime"].as_u64(),
                size: reply["size"].as_u64(),
            });
        }
        serde_json::from_value(reply).map_err(|e| SaveError::Failed { message: format!("{place}: unexpected reply from sparkjob ({e})") })
    }

    /// Previous versions of a file the editor saved, newest first.
    pub async fn backups(&self, demo: bool, place: &str, path: &str) -> Result<Vec<Backup>, String> {
        if place == MAC {
            return Ok(mac::backups(&self.backups, path));
        }
        if demo {
            return Ok(self.demo.lock().unwrap().backups(place, path));
        }
        let v = self.live.sparkjob(place, &format!("backups ~/.sparkbox/backups {}", q(path)), None, Duration::from_secs(30)).await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    async fn node_cmd(&self, place: &str, cmd: &str, what: &str) -> Result<(), String> {
        let out = self.live.run(place, cmd, None, Duration::from_secs(120)).await?;
        if out.code == Some(0) {
            Ok(())
        } else {
            Err(format!("couldn't {what} on {place}: {}", out.stderr.trim().lines().last().unwrap_or("failed")))
        }
    }

    pub async fn mkdir(&self, demo: bool, place: &str, dir: &str, name: &str) -> Result<(), String> {
        valid_name(name)?;
        let path = format!("{}/{name}", dir.trim_end_matches('/'));
        if place == MAC {
            std::fs::create_dir(mac::path(&path)).map_err(|e| format!("can't create {path}: {e}"))?;
            self.audit_mac(&format!("New folder {path}"), path.clone());
            return Ok(());
        }
        if Self::simulated(demo, place) {
            self.demo.lock().unwrap().mkdir(place, &path)?;
            self.audit_demo(place, &format!("New folder {path}"), &format!("mkdir -- {}", q(&path)));
            return Ok(());
        }
        self.titled(format!("New folder {name} on {place}"), self.node_cmd(place, &format!("mkdir -- {}", q(&path)), "create the folder")).await
    }

    /// Makes an empty file `name` in `dir`, only if nothing is there (a link counts): on this Mac
    /// created new, never opened over one; on a node sparkjob's commit with `--expect absent`
    /// (C8). Returns its path, for the editor. A Demo node has no simulation of it.
    pub async fn new_file(&self, demo: bool, place: &str, dir: &str, name: &str) -> Result<String, String> {
        valid_name(name)?;
        let path = format!("{}/{name}", dir.trim_end_matches('/'));
        if place == MAC {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(mac::path(&path)) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(already_there(name)),
                Err(e) => return Err(format!("can't create {path}: {e}")),
            }
            self.audit_mac(&format!("New file {path}"), format!("{path}: 0 bytes, sha256 {}", short_sha(EMPTY_SHA)));
            return Ok(path);
        }
        if Self::simulated(demo, place) {
            return Err(LIVE_ONLY.into());
        }
        let args = format!("commit {} --sha256 {EMPTY_SHA} --expect absent", q(&path));
        let reply = self.titled(format!("New file {name} on {place}"), self.live.sparkjob(place, &args, Some(b""), Duration::from_secs(60))).await?;
        if reply["conflict"].as_bool() == Some(true) {
            return Err(already_there(name));
        }
        Ok(reply["path"].as_str().unwrap_or(&path).to_string())
    }

    pub async fn rename(&self, demo: bool, place: &str, path: &str, new_name: &str) -> Result<(), String> {
        valid_name(new_name)?;
        let (parent, _) = parent_and_name(path)?;
        let to = format!("{}/{new_name}", parent.trim_end_matches('/'));
        if place == MAC {
            if mac::path(&to).symlink_metadata().is_ok() {
                return Err(format!("{to} already exists."));
            }
            std::fs::rename(mac::path(path), mac::path(&to)).map_err(|e| format!("can't rename {path}: {e}"))?;
            self.audit_mac(&format!("Rename {path}"), format!("{path} → {to}"));
            return Ok(());
        }
        if Self::simulated(demo, place) {
            self.demo.lock().unwrap().rename(place, path, &to)?;
            self.audit_demo(place, &format!("Rename {path}"), &rename_command(path, &to));
            return Ok(());
        }
        self.titled(format!("Rename {} to {new_name} on {place}", base(path)), self.node_cmd(place, &rename_command(path, &to), "rename")).await
    }

    /// Moves items to the Trash. Undo hands the result back to `untrash`.
    pub async fn trash(&self, demo: bool, place: &str, paths: &[String]) -> Result<Vec<Trashed>, String> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        if place == MAC {
            // Refused paths (/, the home) are refused before anything walks them for a size.
            for p in paths {
                paths::mac_removable(&mac::path(p), &mac::home())?;
            }
            let sizes: u64 = paths.iter().map(|p| mac::size(&mac::path(p))).sum();
            let bin = self.mac_trash.lock().unwrap().clone();
            let done: Result<Vec<Trashed>, String> = paths.iter().map(|p| mac::trash(p, bin.as_ref())).collect();
            if done.is_ok() {
                self.audit_mac(&format!("Move {} to the Trash", count_items(paths)), format!("{} ({})", paths.join(", "), human_bytes(sizes)));
            }
            return done;
        }
        if Self::simulated(demo, place) {
            let done: Result<Vec<Trashed>, String> = {
                let mut fs = self.demo.lock().unwrap();
                paths.iter().map(|p| fs.trash(place, p)).collect()
            };
            if done.is_ok() {
                let args: Vec<String> = paths.iter().map(|p| q(p)).collect();
                self.audit_demo(place, &format!("Move {} to the Trash", count_items(paths)), &format!("python3 ~/.sparkbox/sparkjob.py trash {}", args.join(" ")));
            }
            return done;
        }
        self.titled(format!("Move {} to the Trash on {place}", count_items(paths)), async {
            let home = self.node_user(place).await?.home;
            let checked: Vec<String> = paths.iter().map(|p| paths::node_removable(p, Some(&home))).collect::<Result<_, _>>()?;
            let args: Vec<String> = checked.iter().map(|p| q(p)).collect();
            let v = self.live.sparkjob(place, &format!("trash {}", args.join(" ")), None, Duration::from_secs(300)).await?;
            serde_json::from_value(v).map_err(|e| e.to_string())
        })
        .await
    }

    /// Puts trashed items back (Undo of Move to Trash).
    pub async fn untrash(&self, demo: bool, place: &str, items: &[Trashed]) -> Result<(), String> {
        if place == MAC {
            return items.iter().try_for_each(mac::untrash);
        }
        if Self::simulated(demo, place) {
            let mut fs = self.demo.lock().unwrap();
            for t in items {
                fs.untrash(place, t.trashed.as_deref().ok_or("unknown Trash location")?)?;
            }
            return Ok(());
        }
        let args: Vec<String> = items.iter().filter_map(|t| t.trashed.as_deref()).map(q).collect();
        if args.is_empty() {
            return Ok(());
        }
        let shown: Vec<String> = items.iter().map(|t| t.path.clone()).collect();
        let cmd = format!("untrash {}", args.join(" "));
        let put_back = self.live.sparkjob(place, &cmd, None, Duration::from_secs(300));
        self.titled(format!("Put back {} on {place}", count_items(&shown)), put_back).await.map(|_| ())
    }

    /// Deletes items for good (the window asks first). System folders and the home are refused.
    pub async fn delete(&self, demo: bool, place: &str, paths: &[String]) -> Result<(), String> {
        self.remove_paths(demo, place, paths).await
    }

    pub(super) async fn remove_paths(&self, demo: bool, place: &str, paths: &[String]) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        if place == MAC {
            for p in paths {
                paths::mac_removable(&mac::path(p), &mac::home())?;
            }
            let sizes: u64 = paths.iter().map(|p| mac::size(&mac::path(p))).sum();
            paths.iter().try_for_each(|p| mac::remove(p))?;
            self.audit_mac(&format!("Delete {}", count_items(paths)), format!("{} ({})", paths.join(", "), human_bytes(sizes)));
            return Ok(());
        }
        if Self::simulated(demo, place) {
            {
                let mut fs = self.demo.lock().unwrap();
                paths.iter().try_for_each(|p| fs.remove(place, p))?;
            }
            let args: Vec<String> = paths.iter().map(|p| q(p)).collect();
            self.audit_demo(place, &format!("Delete {}", count_items(paths)), &format!("python3 ~/.sparkbox/sparkjob.py rm {}", args.join(" ")));
            return Ok(());
        }
        self.titled(format!("Delete {} on {place}", count_items(paths)), async {
            let home = self.node_user(place).await?.home;
            let checked: Vec<String> = paths.iter().map(|p| paths::node_removable(p, Some(&home))).collect::<Result<_, _>>()?;
            let args: Vec<String> = checked.iter().map(|p| q(p)).collect();
            self.live.sparkjob(place, &format!("rm {}", args.join(" ")), None, Duration::from_secs(600)).await.map(|_| ())
        })
        .await
    }

    pub async fn chmod(&self, demo: bool, place: &str, path: &str, mode: u32) -> Result<(), String> {
        if mode > 0o7777 {
            return Err("Permissions are three or four octal digits, e.g. 644.".into());
        }
        if place == MAC {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(mac::path(path), std::fs::Permissions::from_mode(mode)).map_err(|e| format!("can't change {path}: {e}"))?;
            self.audit_mac(&format!("Change permissions of {path}"), format!("{path}: {mode:o}"));
            return Ok(());
        }
        if Self::simulated(demo, place) {
            self.demo.lock().unwrap().chmod(place, path, mode)?;
            self.audit_demo(place, &format!("Change permissions of {path}"), &format!("chmod -- {mode:o} {}", q(path)));
            return Ok(());
        }
        let title = format!("Change permissions of {} on {place} to {mode:o}", base(path));
        self.titled(title, self.node_cmd(place, &format!("chmod -- {mode:o} {}", q(path)), "change permissions")).await
    }

    /// Bytes under each path (Calculate size).
    pub async fn du(&self, demo: bool, place: &str, paths: &[String]) -> Result<Vec<Usage>, String> {
        if place == MAC {
            return Ok(paths.iter().map(|p| Usage { path: p.clone(), bytes: mac::size(&mac::path(p)) }).collect());
        }
        if Self::simulated(demo, place) {
            let mut fs = self.demo.lock().unwrap();
            return paths.iter().map(|p| fs.size(place, p).map(|bytes| Usage { path: p.clone(), bytes })).collect();
        }
        let args: Vec<String> = paths.iter().map(|p| q(p)).collect();
        let v = self.live.sparkjob(place, &format!("du {}", args.join(" ")), None, Duration::from_secs(600)).await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
}

/// The leftover sweep against a fake node (real shell, SparkBox's sparkjob.py): look-only
/// pauses it, and it goes on once look-only is off.
#[cfg(test)]
mod hold_tests {
    use super::*;
    use crate::fake_node::{connect, fake_node, real_shell, temp};

    #[tokio::test]
    async fn the_leftover_sweep_pauses_in_look_only_and_goes_on_after() {
        let home = temp("files-hold-home");
        let (node, port) = fake_node(&home, real_shell()).await;
        let app = temp("files-hold-app");
        let (live, _) = connect(&app, &[("cell1", port)]).await;
        let files = Files::new(live.clone(), app.join("backups"));
        let (gate, _, _) = crate::admin::testing::gate();
        files.set_gate(gate.clone());
        files.set_retry_every(Duration::from_millis(50));
        let stage = home.join(".sparkbox-incoming-7-abc");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("half.bin"), "partial").unwrap();
        let home_s = home.canonicalize().unwrap().display().to_string();
        files.keep_leftover(Leftover { place: "cell1".into(), dir: home_s.clone(), nonce: "7-abc".into(), path: format!("{home_s}/.sparkbox-incoming-7-abc"), bytes: None, why: "connection lost".into() });

        gate.set_look_only(true).unwrap();
        files.retry_leftovers();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(node.ran("cleanup").is_empty(), "nothing sent to the node in look-only: {:?}", node.ran("sparkjob"));
        assert!(stage.exists() && files.leftovers().len() == 1, "the leftover is still there, and still listed");

        gate.set_look_only(false).unwrap();
        gate.unlock(None).unwrap();
        for _ in 0..100 {
            if files.leftovers().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(files.leftovers().is_empty(), "removed once look-only was off");
        assert!(!stage.exists());
        assert_eq!(node.ran("cleanup").len(), 1);
        live.stop();
    }

    /// Before the app hands the gate over, nothing says Admin mode is unlocked: it waits.
    #[tokio::test]
    async fn the_leftover_sweep_waits_for_the_gate_at_startup() {
        let home = temp("files-nogate-home");
        let (node, port) = fake_node(&home, real_shell()).await;
        let app = temp("files-nogate-app");
        let (live, _) = connect(&app, &[("cell1", port)]).await;
        let files = Files::new(live.clone(), app.join("backups"));
        files.set_retry_every(Duration::from_millis(50));
        let stage = home.join(".sparkbox-incoming-7-abc");
        std::fs::create_dir_all(&stage).unwrap();
        let home_s = home.canonicalize().unwrap().display().to_string();
        files.keep_leftover(Leftover { place: "cell1".into(), dir: home_s.clone(), nonce: "7-abc".into(), path: format!("{home_s}/.sparkbox-incoming-7-abc"), bytes: None, why: "connection lost".into() });
        files.retry_leftovers();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(node.ran("cleanup").is_empty() && stage.exists());

        let (gate, _, _) = crate::admin::testing::gate();
        gate.unlock(None).unwrap();
        files.set_gate(gate);
        for _ in 0..100 {
            if files.leftovers().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(files.leftovers().is_empty() && !stage.exists());
        live.stop();
    }
}
