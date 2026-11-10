//! Saving and loading SparkBox's own files without ever leaving half of one behind.
//!
//! `write_atomic` writes a temporary file next to the target, flushes it to disk and
//! renames it over the original, so a crash or power cut leaves either the old file
//! or the new one. `Batch` does the same for many files at once with one barrier instead of
//! a full flush per file (history's minute save). `load_json_or_quarantine` moves a file it can't parse out of the
//! way (keeping it for the user) instead of silently replacing it with defaults, and
//! remembers the problem for `persistence_warnings`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// A settings file that couldn't be read and was set aside.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quarantined {
    /// Where the unreadable original now is.
    pub backup: PathBuf,
    pub error: String,
}

/// One line for the "some settings couldn't be read" banner.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistenceWarning {
    /// The file's name, e.g. "inventory.json".
    pub file: String,
    pub path: String,
    pub backup: String,
    pub error: String,
    pub ts_ms: u64,
}

static WARNINGS: Mutex<Vec<PersistenceWarning>> = Mutex::new(Vec::new());

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Replaces `path` with `bytes` in one step: temporary file in the same folder, fsync, rename, fsync the folder.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let mut nonce = [0u8; 4];
    let _ = getrandom::fill(&mut nonce);
    let tmp = dir.join(format!(".{name}.tmp-{}-{:08x}", std::process::id(), u32::from_le_bytes(nonce)));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        // The rename itself is only durable once the folder is flushed too.
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok::<(), std::io::Error>(())
    })();
    result.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Couldn't save {}: {e}", path.display())
    })
}

/// How hard a save pushes a file to disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flush {
    /// fsync(2): the data leaves for the drive; on macOS it may still wait in the drive's own cache.
    Plain,
    /// F_BARRIERFSYNC on macOS: fsync, and the drive writes everything sent before it ahead of
    /// anything sent after. Plain fsync elsewhere.
    Barrier,
    /// F_FULLFSYNC on macOS (what Rust's `sync_all` does there): the drive's cache is flushed to
    /// flash too, which takes milliseconds each time.
    Full,
}

/// Puts a file's data on disk. The seam tests count syncs (or hold a save up) through.
pub trait Syncer: Send + Sync {
    fn sync(&self, file: &fs::File, how: Flush) -> std::io::Result<()>;
}

/// The real syncs.
pub struct OsSync;

impl Syncer for OsSync {
    fn sync(&self, file: &fs::File, how: Flush) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let fd = file.as_raw_fd();
        let fsync = || if unsafe { libc::fsync(fd) } == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) };
        match how {
            Flush::Plain => fsync(),
            // A volume that can't do it (not APFS, a network share) gets a plain fsync, as SQLite does.
            Flush::Barrier => if barrier(fd) { Ok(()) } else { fsync() },
            Flush::Full => file.sync_all().or_else(|_| fsync()),
        }
    }
}

#[cfg(target_os = "macos")]
fn barrier(fd: std::os::fd::RawFd) -> bool {
    unsafe { libc::fcntl(fd, libc::F_BARRIERFSYNC) != -1 }
}

#[cfg(not(target_os = "macos"))]
fn barrier(_fd: std::os::fd::RawFd) -> bool {
    false
}

/// Several files replaced in one folder with one barrier for all of them, where `write_atomic`
/// flushes fully twice per file. Each file is written under a temporary name and fsynced; at
/// `commit` one barrier on the folder makes those writes reach the drive before the renames
/// that follow, so a power cut leaves every file either old or new. With `full` (Quit,
/// SIGTERM) one F_FULLFSYNC on the folder then puts everything on flash before SparkBox goes.
/// Files written but never committed are removed.
pub struct Batch<'a> {
    dir: PathBuf,
    syncer: &'a dyn Syncer,
    /// (temporary file, where it goes)
    pending: Vec<(PathBuf, PathBuf)>,
}

impl<'a> Batch<'a> {
    pub fn new(dir: &Path, syncer: &'a dyn Syncer) -> Result<Batch<'a>, String> {
        fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
        Ok(Batch { dir: dir.to_path_buf(), syncer, pending: Vec::new() })
    }

    /// Writes `bytes` for `name` (a file in the folder) under a temporary name, fsynced but not yet in place.
    pub fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.dir.join(name);
        let mut nonce = [0u8; 4];
        let _ = getrandom::fill(&mut nonce);
        let tmp = self.dir.join(format!(".{name}.tmp-{}-{:08x}", std::process::id(), u32::from_le_bytes(nonce)));
        let result = (|| {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            self.syncer.sync(&f, Flush::Plain)
        })();
        match result {
            Ok(()) => {
                self.pending.push((tmp, path));
                Ok(())
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(format!("Couldn't save {}: {e}", path.display()))
            }
        }
    }

    /// Puts every written file in place: one barrier, the renames, and with `full` one full flush.
    /// With nothing written only `full` syncs (what earlier saves left behind a barrier still
    /// reaches flash on Quit). Returns the names that couldn't be put in place, each with why.
    pub fn commit(mut self, full: bool) -> Vec<(String, String)> {
        let pending = std::mem::take(&mut self.pending);
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let folder = match fs::File::open(&self.dir) {
            Ok(f) => f,
            Err(e) => {
                let why = format!("Couldn't open {}: {e}", self.dir.display());
                return pending.into_iter().map(|(tmp, path)| {
                    let _ = fs::remove_file(tmp);
                    (name(&path), why.clone())
                }).collect();
            }
        };
        let mut failed = Vec::new();
        if !pending.is_empty() {
            if let Err(e) = self.syncer.sync(&folder, Flush::Barrier) {
                // Without the barrier a rename could reach the drive before its data: keep the old files.
                let why = format!("Couldn't save in {}: {e}", self.dir.display());
                for (tmp, path) in pending {
                    let _ = fs::remove_file(tmp);
                    failed.push((name(&path), why.clone()));
                }
                return failed;
            }
        }
        for (tmp, path) in pending {
            if let Err(e) = fs::rename(&tmp, &path) {
                let _ = fs::remove_file(&tmp);
                failed.push((name(&path), format!("Couldn't save {}: {e}", path.display())));
            }
        }
        if full {
            if let Err(e) = self.syncer.sync(&folder, Flush::Full) {
                eprintln!("{} wasn't flushed to disk: {e}", self.dir.display());
            }
        }
        failed
    }
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        for (tmp, _) in self.pending.drain(..) {
            let _ = fs::remove_file(tmp);
        }
    }
}

/// A short, non-reversible tag for a secret (to notice it changed without keeping it).
pub fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Serialises `value` as pretty JSON and saves it atomically.
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    write_atomic(path, &json)
}

/// Reads JSON from `path`. A missing file is `Ok(None)`. A file that can't be parsed is
/// moved to `<name>.corrupt-<unix ms>` and reported (also through `warnings()`), so the
/// caller can start from defaults without destroying what was there.
pub fn load_json_or_quarantine<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, Quarantined> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            // Unreadable (permissions…): leave it where it is, but don't pretend it's fine.
            let q = Quarantined { backup: path.to_path_buf(), error: e.to_string() };
            remember(path, &q);
            return Err(q);
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let backup = path.with_file_name(format!("{name}.corrupt-{}", now_ms()));
            let backup = match fs::rename(path, &backup) {
                Ok(()) => backup,
                Err(_) => path.to_path_buf(),
            };
            let q = Quarantined { backup, error: e.to_string() };
            remember(path, &q);
            Err(q)
        }
    }
}

/// Like `load_json_or_quarantine`, falling back to the type's default (and logging why).
pub fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    match load_json_or_quarantine(path) {
        Ok(v) => v.unwrap_or_default(),
        Err(q) => {
            eprintln!("{} couldn't be read ({}); kept it at {} and started fresh", path.display(), q.error, q.backup.display());
            T::default()
        }
    }
}

fn remember(path: &Path, q: &Quarantined) {
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let full = path.display().to_string();
    let mut w = WARNINGS.lock().unwrap();
    w.retain(|x| x.path != full);
    w.push(PersistenceWarning { file, path: full, backup: q.backup.display().to_string(), error: q.error.clone(), ts_ms: now_ms() });
}

/// A guarded store refused a read/write without moving or replacing the original.
pub(crate) fn persistence_problem(path: &Path, backup: &Path, error: &str) {
    remember(path, &Quarantined { backup: backup.to_path_buf(), error: error.into() });
}

/// The warning for one file this session, if it was set aside.
pub fn warning_for(path: &Path) -> Option<PersistenceWarning> {
    let full = path.display().to_string();
    WARNINGS.lock().unwrap().iter().find(|w| w.path == full).cloned()
}

/// Every settings file set aside since launch, for the banner.
pub fn warnings() -> Vec<PersistenceWarning> {
    WARNINGS.lock().unwrap().clone()
}

/// Forgets the warning for one file (after the user dealt with it, e.g. forgot host keys).
pub fn clear_warning(path: &Path) {
    let full = path.display().to_string();
    WARNINGS.lock().unwrap().retain(|w| w.path != full);
}

/// Tauri command: settings files that couldn't be read this session.
#[tauri::command]
pub fn persistence_warnings() -> Vec<PersistenceWarning> {
    warnings()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Thing {
        name: String,
        n: u32,
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sparkbox-util-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn atomic_writes_replace_the_file_and_leave_no_temporaries() {
        let d = dir("atomic");
        let p = d.join("sub").join("prefs.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "two");
        let names: Vec<String> = fs::read_dir(p.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        assert_eq!(names, vec!["prefs.json".to_string()], "temporary files are gone");
    }

    /// Counts the syncs; a barrier fails when `fail_barrier` is set.
    #[derive(Default)]
    struct Count {
        syncs: Mutex<Vec<Flush>>,
        fail_barrier: bool,
    }

    impl Syncer for Count {
        fn sync(&self, _file: &fs::File, how: Flush) -> std::io::Result<()> {
            self.syncs.lock().unwrap().push(how);
            if how == Flush::Barrier && self.fail_barrier {
                return Err(std::io::Error::other("no barrier"));
            }
            Ok(())
        }
    }

    fn names_in(d: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_batch_puts_its_files_in_place_behind_one_barrier() {
        let d = dir("batch");
        fs::write(d.join("b"), "old").unwrap();
        let count = Count::default();
        let mut batch = Batch::new(&d, &count).unwrap();
        batch.write("a", b"one").unwrap();
        batch.write("b", b"two").unwrap();
        assert_eq!(fs::read_to_string(d.join("b")).unwrap(), "old", "nothing is in place before the commit");
        assert!(batch.commit(false).is_empty());
        assert_eq!((fs::read_to_string(d.join("a")).unwrap(), fs::read_to_string(d.join("b")).unwrap()), ("one".into(), "two".into()));
        assert_eq!(*count.syncs.lock().unwrap(), vec![Flush::Plain, Flush::Plain, Flush::Barrier], "an fsync per file, one barrier, no full flush");
        assert_eq!(names_in(&d), vec!["a".to_string(), "b".to_string()], "no temporaries left");

        // Quit: a full flush at the end, even with nothing new to write.
        count.syncs.lock().unwrap().clear();
        assert!(Batch::new(&d, &count).unwrap().commit(true).is_empty());
        assert_eq!(*count.syncs.lock().unwrap(), vec![Flush::Full]);

        // A barrier that fails keeps every old file; a batch never committed leaves nothing behind.
        let failing = Count { fail_barrier: true, ..Default::default() };
        let mut batch = Batch::new(&d, &failing).unwrap();
        batch.write("a", b"three").unwrap();
        let failed: Vec<String> = batch.commit(false).into_iter().map(|(n, _)| n).collect();
        assert_eq!(failed, vec!["a".to_string()]);
        assert_eq!(fs::read_to_string(d.join("a")).unwrap(), "one");
        let mut batch = Batch::new(&d, &count).unwrap();
        batch.write("c", b"four").unwrap();
        drop(batch);
        assert_eq!(names_in(&d), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn the_real_syncs_work_on_this_disk() {
        let d = dir("os-sync");
        let f = fs::File::create(d.join("x")).unwrap();
        for how in [Flush::Plain, Flush::Barrier, Flush::Full] {
            OsSync.sync(&f, how).unwrap();
        }
        OsSync.sync(&fs::File::open(&d).unwrap(), Flush::Barrier).unwrap();
    }

    #[test]
    fn json_round_trips_and_missing_is_none() {
        let d = dir("json");
        let p = d.join("thing.json");
        assert_eq!(load_json_or_quarantine::<Thing>(&p), Ok(None));
        save_json(&p, &Thing { name: "cell1".into(), n: 4 }).unwrap();
        assert_eq!(load_json_or_quarantine::<Thing>(&p), Ok(Some(Thing { name: "cell1".into(), n: 4 })));
    }

    #[test]
    fn a_half_written_file_is_quarantined_and_reported() {
        let d = dir("corrupt");
        let p = d.join("thing-corrupt.json");
        fs::write(&p, br#"{"name": "cell1", "n": 4"#).unwrap();
        let q = load_json_or_quarantine::<Thing>(&p).unwrap_err();
        assert!(!p.exists(), "the broken file moved out of the way");
        assert!(q.backup.file_name().unwrap().to_string_lossy().starts_with("thing-corrupt.json.corrupt-"));
        assert_eq!(fs::read_to_string(&q.backup).unwrap(), r#"{"name": "cell1", "n": 4"#, "the original is kept");
        assert!(q.error.contains("EOF"), "{}", q.error);
        assert!(warnings().iter().any(|w| w.file == "thing-corrupt.json" && w.backup == q.backup.display().to_string()));
        assert!(warning_for(&p).is_some());

        // Starting fresh afterwards works, and the backup stays.
        let fresh: Thing = load_json_or_default(&p);
        assert_eq!(fresh, Thing::default());
        assert!(q.backup.exists());
        clear_warning(&p);
        assert!(warning_for(&p).is_none());
    }
}
