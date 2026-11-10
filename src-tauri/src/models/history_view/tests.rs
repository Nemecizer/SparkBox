use super::*;

const REPORTS: &[u8] = include_bytes!("../../../../src/test/fixtures/deployment-history-reports.json");
const RECOVERY: &[u8] = include_bytes!("../../../../src/test/fixtures/deployment-history-recovery.json");
fn copy(dir: &Path, bytes: &[u8]) -> std::path::PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{}.json", crate::live::sha256_hex(bytes)));
    fs::write(&path, bytes).unwrap(); path
}

#[test]
fn history_view_keeps_ten_older_reports_and_both_original_warnings() {
    let source = preview("older.json".into(), REPORTS).unwrap();
    assert_eq!(source.records.len(), 10);
    assert!(source.issues.is_empty());
    assert_eq!(source.records.iter().filter(|r| !r.unload_report.as_ref().unwrap().ok).count(), 2);
    assert!(source.records.iter().all(|r| r.report_only && r.model.is_none() && r.started_at.is_none()));
    assert_eq!(source.records[4].unload_report.as_ref().unwrap().nodes[0].new_shm, ["example-semaphore"]);
    assert_eq!(source.sha256, crate::live::sha256_hex(REPORTS));
}

#[test]
fn history_view_never_restores_ready_or_forgotten_records_or_changes_files() {
    let root = crate::fake_node::temp("history-view-inert");
    let active = root.join("deployments.json"); fs::write(&active, b"[]").unwrap();
    let full = crate::models::Deployment {
        spec: crate::models::plan::DeploySpec { id: "stale-ready".into(), served_name: "Historical model".into(), nodes: vec!["cell1".into()], ..Default::default() },
        state: crate::models::DeploymentState::Ready, ..Default::default()
    };
    let bytes = serde_json::to_vec(&vec![full]).unwrap();
    let dir = root.join("deployment-history"); let path = copy(&dir, &bytes);
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    for _ in 0..2 {
        let view = read(&dir);
        assert!(view.issues.is_empty());
        assert_eq!(view.sources[0].records[0].saved_state, "ready");
        let (_, live) = crate::models::history::History::open(active.clone());
        assert!(live.is_empty());
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), before);
    assert_eq!(fs::read(active).unwrap(), b"[]");
    assert_eq!(fs::read_dir(root).unwrap().count(), 2, "no locks, repair files or hidden imports");
}

#[test]
fn history_view_bad_files_are_reported_without_repair_or_loss_of_good_copies() {
    let root = crate::fake_node::temp("history-view-corrupt");
    copy(&root, RECOVERY); let bad = copy(&root, b"{broken");
    let changed = copy(&root, REPORTS); fs::write(&changed, b"[]").unwrap();
    let view = read(&root);
    assert_eq!(view.sources.len(), 3);
    assert_eq!(view.sources.iter().map(|s| s.records.len()).sum::<usize>(), 2);
    assert_eq!(view.sources.iter().filter(|s| !s.issues.is_empty()).count(), 2);
    assert!(view.sources.iter().any(|s| s.issues.iter().any(|e| e.contains("SHA-256"))));
    assert_eq!(fs::read(bad).unwrap(), b"{broken"); assert_eq!(fs::read(changed).unwrap(), b"[]");
    assert_eq!(fs::read_dir(root).unwrap().count(), 3);
}

#[test]
fn history_view_links_special_files_missing_and_unreadable_folders_are_not_empty_successes() {
    use std::os::unix::fs::symlink;
    let root = crate::fake_node::temp("history-view-paths"); let dir = root.join("copies");
    let missing = read(&dir); assert!(missing.sources.is_empty() && missing.issues.is_empty()); assert!(!dir.exists());
    fs::write(&dir, "blocked").unwrap(); assert!(!read(&dir).issues.is_empty()); fs::remove_file(&dir).unwrap();
    let target = root.join("elsewhere"); copy(&target, RECOVERY); symlink(&target, &dir).unwrap();
    assert!(!read(&dir).issues.is_empty()); fs::remove_file(&dir).unwrap(); fs::create_dir(&dir).unwrap();
    let link = dir.join(format!("{}.json", crate::live::sha256_hex(RECOVERY)));
    symlink(target.join(link.file_name().unwrap()), &link).unwrap();
    assert!(read(&dir).sources[0].records.is_empty()); assert!(!read(&dir).sources[0].issues.is_empty());
    fs::remove_file(&link).unwrap(); fs::create_dir(&link).unwrap();
    assert!(read(&dir).sources[0].issues[0].contains("regular file"));
}

#[test]
fn history_view_size_count_and_identity_limits_fail_visibly() {
    assert!(preview("large.json".into(), &vec![b' '; MAX_BYTES + 1]).is_err());
    assert!(preview("../secret.json".into(), REPORTS).is_err());
    let root = crate::fake_node::temp("history-view-limits");
    for i in 0..=MAX_FILES { fs::write(root.join(format!("{i}.json")), b"[]").unwrap(); }
    let view = read(&root); assert!(view.sources.is_empty()); assert!(view.issues[0].contains("128-entry"));
    let row: serde_json::Value = serde_json::from_slice::<Vec<serde_json::Value>>(REPORTS).unwrap().remove(0);
    let too_many = serde_json::to_vec(&vec![row.clone(); MAX_RECORDS + 1]).unwrap();
    let source = preview("many.json".into(), &too_many).unwrap(); assert!(source.records.is_empty()); assert!(!source.issues.is_empty());
    let mut demo = row.clone(); demo["demo"] = true.into();
    let mut malformed = row.clone(); malformed["unloadReport"]["ok"] = serde_json::Value::Null;
    let source = preview("mixed.json".into(), &serde_json::to_vec(&vec![row.clone(), row, demo, malformed]).unwrap()).unwrap();
    assert_eq!(source.records.len(), 2, "duplicate versions kept, invalid rows not invented");
    assert_eq!(source.issues.len(), 3);
}

#[test]
fn history_view_projection_omits_executable_settings_and_unknown_fields() {
    let mut rows: Vec<serde_json::Value> = serde_json::from_slice(RECOVERY).unwrap();
    rows[0]["providerId"] = "secret-provider".into(); rows[0]["spec"]["apiKey"] = "do-not-return".into();
    rows[0]["spec"]["command"] = "never-execute-this".into(); rows[0]["recovery"] = serde_json::json!({"kind":"resume"});
    let source = preview("projection.json".into(), &serde_json::to_vec(&rows).unwrap()).unwrap();
    let output = serde_json::to_string(&source).unwrap();
    for forbidden in ["secret-provider", "do-not-return", "never-execute-this", "\"spec\"", "\"recovery\""] { assert!(!output.contains(forbidden)); }
    let view = demo(); assert!(view.demo && view.folder.is_none()); assert_eq!(view.sources[1].records.len(), 10);
}
