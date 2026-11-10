//! Which SparkBox this is, and what it keeps apart from the other one.
//!
//! The release app is `com.nemecj.sparkbox`. While `SEPARATE_DEV_IDENTITY` is false, a debug
//! build shares that identifier, and so its config, data and Keychain vault, with the release
//! app, as it always has. Turned on, a debug build becomes `com.nemecj.sparkbox.dev`: Tauri
//! derives the config and data folders from the identifier, and the vault is the Keychain item
//! of the same name. Its first run copies the cluster's inventory and trusted host keys from the
//! release config (`import_from_release_once`), never its settings or secrets, so the debug build
//! reaches the cluster only once the password is saved into its own vault.
//!
//! The window state is kept apart either way (`window_state_file`): a test run never moves the
//! release window.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Flipped to true by the integrator as the last step of P1 round 1. While false, debug builds
/// share com.nemecj.sparkbox (config, data, Keychain vault) with the release app, as before.
pub const SEPARATE_DEV_IDENTITY: bool = true;
pub const RELEASE_ID: &str = "com.nemecj.sparkbox";
pub const DEV_ID: &str = "com.nemecj.sparkbox.dev";

/// The debug build's window state; the release app keeps the plugin's default name.
pub const DEV_WINDOW_STATE: &str = ".window-state.dev.json";

/// What a dev build copies from the release config on its first run: the cluster (nodes, how to
/// log in, which host keys and machines were trusted). `node_pins.json` only exists once node
/// identity pins land, so a missing file is skipped.
pub const IMPORTED: &[&str] = &["inventory.json", "known_hosts.json", "ssh-login.json", "node_pins.json"];

/// What was imported and when, written into the dev config next to the copies.
pub const IMPORT_RECORD: &str = "imported-from-release.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instance {
    Release,
    Dev,
}

/// A debug build is the dev instance; a release build is the release one.
pub fn instance() -> Instance {
    if cfg!(debug_assertions) {
        Instance::Dev
    } else {
        Instance::Release
    }
}

/// "release" or "dev", as the evidence manifest and the logs name it.
pub fn instance_name() -> &'static str {
    match instance() {
        Instance::Release => "release",
        Instance::Dev => "dev",
    }
}

/// The dev identifier only for a debug build with the separation turned on.
pub fn identifier_for(debug: bool, separate: bool) -> &'static str {
    if debug && separate {
        DEV_ID
    } else {
        RELEASE_ID
    }
}

/// This build's bundle identifier: Tauri's config and data folders are named after it.
pub fn identifier() -> &'static str {
    identifier_for(cfg!(debug_assertions), SEPARATE_DEV_IDENTITY)
}

/// The Keychain service of this build's vault.
pub fn keychain_service() -> &'static str {
    identifier()
}

/// The window-state file: separate for debug builds whatever the flag says.
pub fn window_state_file_for(debug: bool) -> &'static str {
    if debug {
        DEV_WINDOW_STATE
    } else {
        tauri_plugin_window_state::DEFAULT_FILENAME
    }
}

pub fn window_state_file() -> &'static str {
    window_state_file_for(cfg!(debug_assertions))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportRecord<'a> {
    from: String,
    files: &'a [String],
    at_ms: u64,
}

/// A dev build's first run with its own identifier: copies `IMPORTED` from the release config
/// next to `dev_config` (`<dev_config>/../com.nemecj.sparkbox/`). Returns the files copied, or
/// None when there was nothing to do (release build, separation off, already imported).
pub fn import_from_release_once(dev_config: &Path) -> Result<Option<Vec<String>>, String> {
    import_for(identifier(), dev_config)
}

fn import_for(id: &str, dev_config: &Path) -> Result<Option<Vec<String>>, String> {
    if id != DEV_ID || dev_config.join("inventory.json").exists() {
        return Ok(None);
    }
    let Some(release) = dev_config.parent().map(|p| p.join(RELEASE_ID)) else { return Ok(None) };
    // Read the release file, then write the copy: the source is never renamed, locked or touched.
    let mut files = Vec::new();
    for name in IMPORTED {
        let bytes = match std::fs::read(release.join(name)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("Couldn't read {name} from the release app's settings: {e}")),
        };
        crate::util::write_atomic(&dev_config.join(name), &bytes)?;
        files.push(name.to_string());
    }
    if files.is_empty() {
        return Ok(None);
    }
    let at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let record = ImportRecord { from: release.display().to_string(), files: &files, at_ms };
    crate::util::save_json(&dev_config.join(IMPORT_RECORD), &record)?;
    Ok(Some(files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn only_a_debug_build_with_the_flag_on_is_dev() {
        assert_eq!(identifier_for(false, false), RELEASE_ID);
        assert_eq!(identifier_for(false, true), RELEASE_ID);
        assert_eq!(identifier_for(true, false), RELEASE_ID);
        assert_eq!(identifier_for(true, true), DEV_ID);
        // Tests are a debug build: the release identifier and vault until the flag is turned on.
        assert_eq!(identifier(), if SEPARATE_DEV_IDENTITY { DEV_ID } else { RELEASE_ID });
        assert_eq!(keychain_service(), identifier());
        assert_eq!(instance(), Instance::Dev);
        assert_eq!(instance_name(), "dev");
    }

    #[test]
    fn the_window_state_is_apart_whatever_the_flag() {
        assert_eq!(window_state_file_for(true), ".window-state.dev.json");
        assert_eq!(window_state_file_for(false), ".window-state.json");
        assert_eq!(window_state_file_for(false), tauri_plugin_window_state::DEFAULT_FILENAME);
        assert_eq!(window_state_file(), DEV_WINDOW_STATE, "tests are a debug build");
    }

    /// Application Support with a release config holding everything a real one does.
    fn support() -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("sb-identity-{}-{}", std::process::id(), fastrand::u64(..)));
        let release = root.join(RELEASE_ID);
        let dev = root.join(DEV_ID);
        std::fs::create_dir_all(&release).unwrap();
        for name in [
            "inventory.json",
            "known_hosts.json",
            "ssh-login.json",
            "node_pins.json",
            "prefs.json",
            "security.json",
            "vpn.json",
            "providers.json",
            "server.json",
            "notify.json",
            "fence-trust.json",
            ".window-state.json",
            "tasks.jsonl",
            "activity.jsonl",
            "deployments.json",
        ] {
            std::fs::write(release.join(name), format!("{{\"file\":\"{name}\"}}")).unwrap();
        }
        for dir in ["chats", "history", "backups"] {
            std::fs::create_dir_all(release.join(dir)).unwrap();
            std::fs::write(release.join(dir).join("one.json"), "{}").unwrap();
        }
        (root, release, dev)
    }

    /// Every file under `dir`: its bytes and modification time.
    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        let mut out = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                let meta = std::fs::metadata(&p).unwrap();
                if meta.is_dir() {
                    out.insert(p.clone(), (Vec::new(), meta.modified().unwrap()));
                    stack.push(p);
                } else {
                    out.insert(p.clone(), (std::fs::read(&p).unwrap(), meta.modified().unwrap()));
                }
            }
        }
        out
    }

    #[test]
    fn the_first_dev_run_copies_the_cluster_and_nothing_else() {
        let (root, release, dev) = support();
        let before = snapshot(&release);
        let copied = import_for(DEV_ID, &dev).unwrap().expect("imported");
        assert_eq!(copied, ["inventory.json", "known_hosts.json", "ssh-login.json", "node_pins.json"]);
        for name in IMPORTED {
            assert_eq!(std::fs::read(dev.join(name)).unwrap(), std::fs::read(release.join(name)).unwrap(), "{name}");
        }
        let mut names: Vec<String> = std::fs::read_dir(&dev).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["imported-from-release.json", "inventory.json", "known_hosts.json", "node_pins.json", "ssh-login.json"]);
        // Never prefs (the dev build starts in Demo), settings, secrets' metadata, window state or data.
        for never in ["prefs.json", "security.json", "vpn.json", "providers.json", "server.json", "notify.json", "fence-trust.json", ".window-state.json", "tasks.jsonl", "activity.jsonl", "deployments.json", "chats", "history", "backups"] {
            assert!(!dev.join(never).exists(), "{never} was copied");
        }
        let record: serde_json::Value = serde_json::from_slice(&std::fs::read(dev.join(IMPORT_RECORD)).unwrap()).unwrap();
        assert_eq!(record["files"].as_array().unwrap().len(), 4);
        assert_eq!(record["from"], release.display().to_string());
        assert!(record["atMs"].as_u64().unwrap() > 0);
        // The source is byte- and mtime-identical, and nothing was added to or removed from it.
        assert_eq!(snapshot(&release), before);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_node_pins_file_is_skipped() {
        let (root, release, dev) = support();
        std::fs::remove_file(release.join("node_pins.json")).unwrap();
        let copied = import_for(DEV_ID, &dev).unwrap().unwrap();
        assert_eq!(copied, ["inventory.json", "known_hosts.json", "ssh-login.json"]);
        assert!(!dev.join("node_pins.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nothing_happens_once_imported_or_with_the_flag_off() {
        let (root, release, dev) = support();
        let before = snapshot(&release);
        // The release identifier (a release build, or the flag off): nothing is copied.
        assert_eq!(import_for(RELEASE_ID, &dev).unwrap(), None);
        assert!(!dev.exists());
        // A dev config that already has an inventory is left as it is.
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("inventory.json"), "{\"mine\":true}").unwrap();
        assert_eq!(import_for(DEV_ID, &dev).unwrap(), None);
        assert_eq!(std::fs::read_to_string(dev.join("inventory.json")).unwrap(), "{\"mine\":true}");
        assert!(!dev.join("known_hosts.json").exists());
        assert!(!dev.join(IMPORT_RECORD).exists());
        assert_eq!(snapshot(&release), before);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_entry_point_imports_only_with_the_flag_on() {
        let (root, release, dev) = support();
        let before = snapshot(&release);
        let copied = import_from_release_once(&dev).unwrap();
        assert_eq!(copied.is_some(), SEPARATE_DEV_IDENTITY);
        assert_eq!(dev.join("inventory.json").exists(), SEPARATE_DEV_IDENTITY);
        assert_eq!(snapshot(&release), before);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_release_config_imports_nothing() {
        let root = std::env::temp_dir().join(format!("sb-identity-empty-{}-{}", std::process::id(), fastrand::u64(..)));
        let dev = root.join(DEV_ID);
        assert_eq!(import_for(DEV_ID, &dev).unwrap(), None);
        assert!(!dev.join(IMPORT_RECORD).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
