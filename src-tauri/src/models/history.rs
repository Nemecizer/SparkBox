//! Guard the authoritative deployment list, and retain inert recovery copies. Copies are
//! never fed to resume/claim: old ownership or a saved Ready state isn't current permission.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::Deployment;

pub(super) struct History {
    path: PathBuf,
    expected: Option<Vec<u8>>,
    blocked: Option<String>,
    seeded: bool,
}

impl History {
    pub fn open(path: PathBuf) -> (Self, Vec<Deployment>) {
        let mut store = Self { path, expected: None, blocked: None, seeded: false };
        let loaded = read(&store.path).and_then(|bytes| {
            let records = decode(bytes.as_deref())?;
            store.expected = bytes;
            Ok(records)
        });
        let records = match loaded {
            Ok(records) => records,
            Err(error) => {
                store.blocked = Some(error.clone());
                store.warn(&error);
                Vec::new()
            }
        };
        (store, records)
    }

    pub(super) fn copies(&self) -> PathBuf {
        self.path.with_file_name("deployment-history")
    }

    fn warn(&self, error: &str) {
        let backup = if self.copies().is_dir() { self.copies() } else { self.path.clone() };
        crate::util::persistence_problem(&self.path, &backup, &format!(
            "Deployment history could not be saved or read: {error}. Existing files were kept. New loads and Forget require a successful save. Review the files before reopening SparkBox; recovery copies are never restored automatically."
        ));
    }

    /// Called under Models' deployment mutex. The file lock covers other *new* app instances;
    /// the byte comparison also detects older apps/external edits made before this check.
    /// An older, non-cooperating writer can still race the rename; copies retain our evidence.
    pub fn save(&mut self, records: &[Deployment]) -> Result<(), String> {
        let result = self.save_inner(records);
        match &result {
            Ok(()) => crate::util::clear_warning(&self.path),
            Err(error) => self.warn(error),
        }
        result.map_err(|e| format!("Couldn't save deployment history: {e}. Existing files were kept; see the storage warning."))
    }

    fn save_inner(&mut self, records: &[Deployment]) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(records).map_err(|e| e.to_string())?;
        let dir = self.path.parent().ok_or("Missing history folder")?;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true)
            .open(self.path.with_file_name("deployments.lock")).map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|e| format!("Another save holds the history lock ({e})"))?;
        // Drop releases the OS lock, including after an error or process exit.
        if let Some(error) = &self.blocked {
            if !records.is_empty() { preserve(&self.copies(), &bytes)?; }
            return Err(error.clone());
        }
        let current = read(&self.path)?;
        if current != self.expected {
            if let Some(previous) = &self.expected { preserve(&self.copies(), previous)?; }
            if !records.is_empty() { preserve(&self.copies(), &bytes)?; }
            let error = "The saved list changed outside this SparkBox session; refusing to overwrite it".to_string();
            self.blocked = Some(error.clone());
            return Err(error);
        }
        let before = decode(current.as_deref())?;
        // Preserve the original bytes on migration, before Forget/reusing an id, and when
        // a failure or unload report changes. Routine serving/log polling makes no copies.
        let previous_by_id: std::collections::HashMap<_, _> = before.iter().map(|d| (&d.spec.id, d)).collect();
        let next_by_id: std::collections::HashMap<_, _> = records.iter().map(|d| (&d.spec.id, d)).collect();
        let removed = before.iter().any(|old| next_by_id.get(&old.spec.id).is_none_or(|d|
            d.generation != old.generation || d.started_at != old.started_at));
        let ended_changed = records.iter().filter(|d| !d.state.active()).any(|d|
            previous_by_id.get(&d.spec.id).is_none_or(|old| serde_json::to_value(old).ok() != serde_json::to_value(d).ok()));
        if !self.seeded || removed || ended_changed {
            if let Some(previous) = &current { preserve(&self.copies(), previous)?; }
        }
        if ended_changed { preserve(&self.copies(), &bytes)?; }
        if current.as_ref() != Some(&bytes) {
            crate::util::write_atomic(&self.path, &bytes)?;
        }
        self.expected = Some(bytes);
        self.seeded = true;
        Ok(())
    }
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn decode(bytes: Option<&[u8]>) -> Result<Vec<Deployment>, String> {
    let records: Vec<Deployment> = match bytes {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    let mut ids = std::collections::HashSet::new();
    if records.iter().any(|d| d.demo || d.spec.id.is_empty() || !ids.insert(&d.spec.id)) {
        return Err("The saved list contains Demo, missing or duplicate deployment identities".into());
    }
    Ok(records)
}

/// Content addressed, create-once snapshots. Publish only a fully flushed file; never
/// truncate an existing copy (including a damaged copy). No automatic retention pruning.
fn preserve(dir: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let digest: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("{digest}.json"));
    if path.exists() {
        return if fs::read(&path).map_err(|e| e.to_string())? == bytes { Ok(()) }
            else { Err("A recovery copy has changed; refusing to replace it".into()) };
    }
    let tmp = dir.join(format!(".snapshot-{}-{:016x}", std::process::id(), fastrand::u64(..)));
    let result = (|| -> std::io::Result<()> {
        let mut file = File::create_new(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::hard_link(&tmp, &path)?;
        File::open(dir)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    result.map_err(|e| format!("Couldn't keep a recovery copy: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{plan::DeploySpec, DeploymentState, UnloadReport};

    fn stopped() -> Deployment {
        Deployment { spec: DeploySpec { id: "history-test".into(), ..Default::default() },
            state: DeploymentState::Stopped, unload_report: Some(UnloadReport {
                at_ms: 123, ok: false, summary: "Shared memory remains; do not erase this warning".into(), ..Default::default()
            }), ..Default::default() }
    }

    fn setup(name: &str) -> (History, PathBuf, Vec<Deployment>) {
        let path = crate::fake_node::temp(name).join("deployments.json");
        let records = vec![stopped()];
        crate::util::save_json(&path, &records).unwrap();
        let (store, loaded) = History::open(path.clone());
        assert_eq!(loaded[0].unload_report, records[0].unload_report);
        (store, path, records)
    }

    fn copies(store: &History) -> Vec<Vec<u8>> {
        fs::read_dir(store.copies()).unwrap().map(|p| fs::read(p.unwrap().path()).unwrap()).collect()
    }

    #[test]
    fn deployment_history_forget_keeps_exact_bytes_without_restoring_a_load() {
        let (mut store, path, _) = setup("history-forget");
        let original = fs::read(&path).unwrap();
        store.save(&[]).unwrap();
        assert!(copies(&store).contains(&original));
        let (_, reopened) = History::open(path);
        assert!(reopened.is_empty(), "copies never reenter recovery or claim");
        store.save(&[]).unwrap();
        assert_eq!(copies(&store).len(), 1, "no duplicates on unchanged saves");
    }

    #[test]
    fn deployment_history_old_instance_cannot_overwrite_a_new_unload() {
        let (mut first, path, mut records) = setup("history-stale");
        let (mut stale, _) = History::open(path.clone());
        records[0].unload_report.as_mut().unwrap().at_ms = 456;
        first.save(&records).unwrap();
        let newer = fs::read(&path).unwrap();
        assert!(stale.save(&[]).unwrap_err().contains("changed outside"));
        assert_eq!(fs::read(&path).unwrap(), newer);
        assert!(copies(&first).contains(&newer), "new unload is also recoverable");
        assert!(crate::util::warning_for(&path).unwrap().error.contains("refusing to overwrite"));
    }

    #[test]
    fn deployment_history_corrupt_read_is_not_an_empty_writable_list() {
        let path = crate::fake_node::temp("history-corrupt").join("deployments.json");
        let original = b"[{\"spec\": broken and irreplaceable";
        fs::write(&path, original).unwrap();
        let (mut store, records) = History::open(path.clone());
        assert!(records.is_empty());
        assert!(store.save(&[]).is_err());
        assert!(store.save(&[stopped()]).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(crate::util::warning_for(&path).is_some());
    }

    #[test]
    fn deployment_history_read_failure_and_deleted_file_refuse_replacement() {
        let dir = crate::fake_node::temp("history-unreadable");
        let path = dir.join("deployments.json");
        fs::create_dir(&path).unwrap();
        let (mut store, _) = History::open(path.clone());
        assert!(store.save(&[stopped()]).is_err());
        assert!(path.is_dir());
        let (mut store, path, records) = setup("history-deleted");
        fs::remove_file(&path).unwrap();
        assert!(store.save(&records).is_err());
        assert!(!path.exists());
        assert!(!copies(&store).is_empty());
    }

    #[test]
    fn deployment_history_backup_failure_prevents_forget() {
        let (mut store, path, _) = setup("history-copy-failure");
        let original = fs::read(&path).unwrap();
        fs::write(store.copies(), "not a folder").unwrap();
        assert!(store.save(&[]).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn deployment_history_damaged_copy_is_never_overwritten() {
        let (mut store, path, _) = setup("history-copy-damaged");
        let original = fs::read(&path).unwrap();
        fs::create_dir(store.copies()).unwrap();
        let digest: String = Sha256::digest(&original).iter().map(|b| format!("{b:02x}")).collect();
        let copy = store.copies().join(format!("{digest}.json"));
        fs::write(&copy, "damaged").unwrap();
        assert!(store.save(&[]).unwrap_err().contains("recovery copy has changed"));
        assert_eq!(fs::read_to_string(copy).unwrap(), "damaged");
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn deployment_history_file_lock_serializes_writers_and_releases() {
        let (mut store, path, records) = setup("history-lock");
        let lock = File::create(path.with_file_name("deployments.lock")).unwrap();
        lock.lock().unwrap();
        assert!(store.save(&records).unwrap_err().contains("history lock"));
        drop(lock);
        store.save(&records).unwrap();
        assert!(crate::util::warning_for(&path).is_none());
    }

    #[test]
    fn deployment_history_serving_polls_do_not_create_unbounded_copies() {
        let (mut store, _, mut records) = setup("history-polls");
        records[0].state = DeploymentState::Ready;
        records[0].generation += 1;
        records[0].unload_report = None;
        store.save(&records).unwrap();
        let n = copies(&store).len();
        for i in 0..20 {
            records[0].log_tail = format!("Polling {i}");
            store.save(&records).unwrap();
        }
        assert_eq!(copies(&store).len(), n);
        records[0].state = DeploymentState::Stopped;
        records[0].unload_report = stopped().unload_report;
        store.save(&records).unwrap();
        let (_, reopened) = History::open(store.path.clone());
        assert_eq!(reopened[0].unload_report, records[0].unload_report);
        assert!(copies(&store).iter().any(|b| decode(Some(b)).unwrap().iter().any(|d|
            d.generation == 1 && d.unload_report == records[0].unload_report)));
    }
}
