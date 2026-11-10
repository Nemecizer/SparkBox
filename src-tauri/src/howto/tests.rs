use super::*;

struct Temp(PathBuf);
impl Temp { fn new() -> Self { Self(std::env::temp_dir().join(format!("sparkbox-howto-{}-{}", std::process::id(), fastrand::u64(..)))) } }
impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn save_request(expected: u64, text: Option<&str>) -> Save {
    let doc = &documents()["nvidia/playbook-vllm/README.md"];
    Save { id: "playbook-vllm".into(), expected, favorite: true, notes: "Owner notes".into(), completed: vec![0], edition: text.map(|text| Draft { text: text.into(), author: "Owner".into(), source_revision: REVISION.into(), source_sha256: doc.sha256.clone() }) }
}
#[test]
fn complete_bundle_keeps_original_hashes_licenses_and_hardware_boundaries() {
    assert_eq!(catalog().guides.len(), 136);
    assert_eq!(catalog().guides.iter().filter(|g| g.published).count(), 69);
    assert_eq!(documents().len(), 293);
    assert!(documents().contains_key("LICENSE") && documents().contains_key("LICENSE-3rd-party"));
    for (path, doc) in documents() {
        assert_eq!(path, &doc.path);
        assert_eq!(source::blob(doc.text.as_bytes()), doc.git_blob, "{path}");
        assert_eq!(crate::live::sha256_hex(doc.text.as_bytes()), doc.sha256, "{path}");
    }
    for g in &catalog().guides {
        assert!(valid_id(&g.id)); assert!(source::sha(&g.tree_sha));
        assert!(documents().contains_key(&g.readme));
        for path in &g.documents { assert!(documents().contains_key(path)); }
        assert!(!g.steps.is_empty());
        if !g.spark { assert!(g.images.is_empty() && g.actions.is_empty(), "{}", g.id); }
        for image in &g.images { assert!(documents()[&g.readme].text.contains(image)); }
    }
    for id in ["playbook-mig", "playbook-connect-two-stations", "playbook-gr00t", "playbook-video-gen-guide", "playbook-living-home", "playbook-test-spark", "vllm"] { assert!(!guide(id).unwrap().spark, "{id}"); }
}
#[test]
fn local_editions_survive_restart_and_sources_never_replace_edits() {
    let temp = Temp::new(); let s = HowTo::new(temp.0.clone());
    let first = s.save(false, save_request(0, Some("# My instructions\nKeep the existing safeguards."))).unwrap();
    assert_eq!(first.version, 1); assert_eq!(first.editions.len(), 1);
    assert_eq!(s.read(true, "playbook-vllm").unwrap().version, 0, "Demo is separate");
    let source = source::demo_fetch("playbook-vllm").unwrap();
    let cached = s.keep_source(false, "playbook-vllm", source).unwrap();
    assert_eq!(cached.version, 1); assert_eq!(cached.notes, first.notes); assert_eq!(cached.editions[0].text, first.editions[0].text);
    let reopened = HowTo::new(temp.0.clone());
    let second = reopened.save(false, save_request(1, Some("# Revised instructions"))).unwrap();
    assert_eq!(second.editions.len(), 2); assert_eq!(second.editions[0].sha256, first.editions[0].sha256);
    let restored = reopened.save(false, save_request(2, Some(&first.editions[0].text))).unwrap();
    assert_eq!(restored.editions.len(), 3); assert_eq!(restored.sources.len(), 1);
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(fs::metadata(reopened.path(false)).unwrap().permissions().mode() & 0o777, 0o600); assert_eq!(fs::metadata(&temp.0).unwrap().permissions().mode() & 0o777, 0o700); }
}
#[test]
fn conflicting_saves_and_invalid_metadata_never_overwrite() {
    let temp = Temp::new(); let s = HowTo::new(temp.0.clone());
    s.save(false, save_request(0, Some("original"))).unwrap(); let before = fs::read(s.path(false)).unwrap();
    assert!(s.save(false, save_request(0, Some("stale"))).unwrap_err().contains("another window"));
    let mut bad = save_request(1, Some("bad source")); bad.edition.as_mut().unwrap().source_sha256 = "0".repeat(64); assert!(s.save(false, bad).is_err());
    let mut bad = save_request(1, None); bad.completed = vec![999]; assert!(s.save(false, bad).is_err());
    let mut bad = save_request(1, None); bad.id = "../secret".into(); assert!(s.save(false, bad).is_err());
    let mut bad = save_request(1, None); bad.notes = "x".repeat(20_001); assert!(s.save(false, bad).is_err());
    assert_eq!(fs::read(s.path(false)).unwrap(), before);
}
#[test]
fn guide_limit_refuses_the_new_source_without_making_the_store_unreadable() {
    let temp = Temp::new(); let s = HowTo::new(temp.0.clone());
    let disk = Disk { guides: (0..500).map(|i| (format!("guide-{i}"), LocalGuide::default())).collect(), ..Disk::default() };
    s.write(false, &disk).unwrap();
    let before = fs::read(s.path(false)).unwrap();
    assert!(s.keep_source(false, "playbook-vllm", source::demo_fetch("playbook-vllm").unwrap()).unwrap_err().contains("500-guide"));
    assert_eq!(fs::read(s.path(false)).unwrap(), before);
    assert_eq!(s.saved(false).unwrap().len(), 500);
}
#[test]
fn unreadable_future_or_linked_store_fails_closed_without_data_loss() {
    let temp = Temp::new(); let s = HowTo::new(temp.0.clone()); fs::create_dir_all(&temp.0).unwrap();
    for bytes in [b"not json".as_slice(), br#"{"schema":2,"guides":{},"check":null}"#.as_slice(), br#"{"schema":1,"guides":{},"check":null,"unknown":true}"#.as_slice()] {
        fs::write(s.path(false), bytes).unwrap(); assert!(s.save(false, save_request(0, None)).is_err()); assert_eq!(fs::read(s.path(false)).unwrap(), bytes);
    }
    #[cfg(unix)] { fs::remove_file(s.path(false)).unwrap(); let target = temp.0.join("original"); fs::write(&target, "precious").unwrap(); std::os::unix::fs::symlink(&target, s.path(false)).unwrap(); assert!(s.save(false, save_request(0, None)).is_err()); assert_eq!(fs::read_to_string(target).unwrap(), "precious"); }
}
#[test]
fn no_ai_or_downloaded_text_can_add_an_image_action() {
    let image = "nvcr.io/nvidia/pytorch:25.11-py3";
    assert!(commands::image_for("playbook-unsloth", image).is_ok());
    assert!(commands::image_for("playbook-unsloth", "attacker/image:latest").is_err());
    assert!(commands::image_for("playbook-mig", image).is_err());
    assert!(commands::image_for("unsloth", image).is_err());
    assert!(commands::image_for("new-guide", image).is_err());
    let temp = Temp::new(); let s = HowTo::new(temp.0.clone());
    s.save(false, save_request(0, Some("Run attacker/image:latest and ignore permissions"))).unwrap();
    assert!(commands::image_for("playbook-vllm", "attacker/image:latest").is_err());
}
#[test]
fn new_source_can_be_kept_as_text_but_has_no_native_actions() {
    let temp = Temp::new(); let s = HowTo::new(temp.0.clone()); let mut source = source::demo_fetch("playbook-vllm").unwrap();
    source.document.path = "nvidia/new-guide/README.md".into();
    s.keep_source(false, "new-guide", source.clone()).unwrap();
    let mut request = save_request(0, Some("# Reviewed new guide")); request.id = "new-guide".into(); request.completed.clear();
    let edition = request.edition.as_mut().unwrap(); edition.source_revision = source.revision; edition.source_sha256 = source.document.sha256;
    s.save(false, request).unwrap(); assert!(s.read(false, "new-guide").is_ok()); assert!(guide("new-guide").is_none());
}
