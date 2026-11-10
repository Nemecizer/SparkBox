//! SparkBox's own log, for when something goes wrong in the app rather than on a node:
//! ~/Library/Logs/SparkBox/sparkbox.log (rotated at 5 MB, three kept), with window crashes
//! from the UI and failed tasks. Everything is redacted before it's written. "Export
//! diagnostics" bundles it with the settings, recent tasks and activity, each node's SSH
//! channel use and this build's SSH connection log (live/sshlog.rs) into one text file.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};

use crate::live::audit::{iso_time, redact};

const ROTATE_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: usize = 3;

static PATH: OnceLock<PathBuf> = OnceLock::new();
static WRITE: Mutex<()> = Mutex::new(());

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Where the log goes: ~/Library/Logs/SparkBox.
pub fn init(home: &Path) {
    let dir = home.join("Library/Logs/SparkBox");
    let _ = std::fs::create_dir_all(&dir);
    let _ = PATH.set(dir.join("sparkbox.log"));
}

pub fn path() -> Option<PathBuf> {
    PATH.get().cloned()
}

/// Appends one line (or a few); `what` says where it came from ("ui", "task"…).
pub fn write(what: &str, text: &str) {
    let Some(path) = PATH.get() else { return };
    let _one = WRITE.lock().unwrap_or_else(|p| p.into_inner());
    if std::fs::metadata(path).map(|m| m.len() >= ROTATE_BYTES).unwrap_or(false) {
        for i in (1..KEEP).rev() {
            let _ = std::fs::rename(path.with_extension(format!("{i}.log")), path.with_extension(format!("{}.log", i + 1)));
        }
        let _ = std::fs::rename(path, path.with_extension("1.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} [{what}] {}", iso_time(now_ms()), redact(text).replace('\n', "\n    "));
    }
}

/// This build's SSH log and the rotation before it, oldest first: (title, text).
fn ssh_log_sections(log: &Path) -> Vec<(String, String)> {
    [1, 0]
        .iter()
        .filter_map(|&n| {
            let p = crate::live::sshlog::rotation(log, n);
            let text = std::fs::read_to_string(&p).ok()?;
            Some((format!("SSH log ({})", p.file_name()?.to_string_lossy()), text))
        })
        .collect()
}

/// "cell1: 3 channels in use, 0 queued, 0 refused, 1 bulk connection"
fn channel_line(node: &str, s: &crate::live::ssh::ChannelStats) -> String {
    let plural = |n: usize, one: &str| if n == 1 { format!("{n} {one}") } else { format!("{n} {one}s") };
    format!("{node}: {} in use, {} queued, {} refused, {}", plural(s.in_use, "channel"), s.queued, s.refusals, plural(s.bulk_connections, "bulk connection"))
}

fn tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// A window crashed in the UI (WindowErrorBoundary).
#[tauri::command]
pub fn log_ui_error(window: Option<String>, message: String, stack: Option<String>) {
    let stack: String = stack.unwrap_or_default().chars().take(8000).collect();
    write("ui", &format!("window {} crashed: {message}\n{stack}", window.unwrap_or_default()));
}

/// Saves a diagnostics report to Downloads (and shows it in Finder): versions, settings,
/// the inventory, recent tasks and activity, and the app log. No passwords or keys: they
/// live only in the Keychain, and the rest is redacted.
#[tauri::command]
pub async fn diagnostics_export(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt;
    let state = app.state::<crate::AppState>();
    let mut out = String::new();
    let section = |out: &mut String, title: &str, body: &str| {
        out.push_str(&format!("\n===== {title}\n{}\n", body.trim_end()));
    };
    section(&mut out, "SparkBox", &format!("version {} · {}", app.package_info().version, iso_time(now_ms())));
    let prefs = state.prefs.lock().unwrap().clone();
    section(&mut out, "Preferences", &serde_json::to_string_pretty(&prefs).unwrap_or_default());
    let inventory = state.inventory.lock().unwrap().clone();
    section(&mut out, "Inventory", &serde_json::to_string_pretty(&inventory).unwrap_or_default());
    if let Some(gate) = app.try_state::<std::sync::Arc<crate::admin::AdminGate>>() {
        section(&mut out, "Security", &format!("{:?}\n{:?}", gate.settings(), gate.state()));
    }
    let vpn = state.vpn.view().await.status;
    section(&mut out, "VPN", &format!("{:?}{}", vpn.state, vpn.message.map(|m| format!(" ({m})")).unwrap_or_default()));
    let snap = state.latest.read().unwrap().clone();
    if let Some(s) = snap {
        let nodes: Vec<String> = s.nodes.iter().map(|n| format!("{} {:?} {}", n.name, n.status, n.error.clone().unwrap_or_default())).collect();
        section(&mut out, "Nodes", &nodes.join("\n"));
    }
    let warnings = crate::util::persistence_warnings();
    if !warnings.is_empty() {
        section(&mut out, "Settings files set aside", &serde_json::to_string_pretty(&warnings).unwrap_or_default());
    }
    if let Some(tasks) = app.try_state::<std::sync::Arc<crate::tasks::Tasks>>() {
        let lines: Vec<String> = tasks
            .list()
            .iter()
            .take(100)
            .map(|t| format!("{} {:?} {} — {}", iso_time(t.started_ms), t.state, t.title, t.error.clone().or(t.summary.clone()).unwrap_or_default()))
            .collect();
        section(&mut out, "Recent tasks", &lines.join("\n"));
    }
    let activity: Vec<String> = state.live.audit.entries().iter().take(500).filter_map(|e| serde_json::to_string(e).ok()).collect();
    section(&mut out, "Recent activity (JSON lines)", &activity.join("\n"));
    if let Some(log) = path() {
        section(&mut out, "App log (last 300 lines)", &tail(&log, 300));
    }
    let channels: Vec<String> = state.live.all_channel_stats().iter().map(|(id, s)| channel_line(id, s)).collect();
    section(&mut out, "SSH channels", &channels.join("\n"));
    if let Some(log) = state.live.sshlog.file() {
        for (title, text) in ssh_log_sections(&log) {
            section(&mut out, &title, &text);
        }
    }
    let stamp = iso_time(now_ms()).replace([':', 'T'], "-").trim_end_matches('Z').to_string();
    let file = app.path().download_dir().map_err(|e| e.to_string())?.join(format!("SparkBox-diagnostics-{stamp}.txt"));
    crate::util::write_atomic(&file, redact(&out).as_bytes())?;
    let _ = app.opener().reveal_item_in_dir(&file);
    Ok(file.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_redacted_and_the_file_rotates() {
        let dir = std::env::temp_dir().join(format!("sparkbox-logging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init(&dir);
        let log = path().unwrap();
        log_ui_error(Some("fleet".into()), "boom with HF_TOKEN=hf_abcdefghijklmnopqrstuvwxyz0123".into(), Some("at x".into()));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("[ui] window fleet crashed: boom with HF_TOKEN=‹redacted›"));
        std::fs::OpenOptions::new().append(true).open(&log).unwrap().set_len(ROTATE_BYTES).unwrap();
        write("task", "after the limit");
        assert!(log.with_extension("1.log").exists());
        assert!(std::fs::read_to_string(&log).unwrap().contains("after the limit"));
    }

    #[test]
    fn the_export_carries_the_ssh_log_and_its_last_rotation() {
        let dir = std::env::temp_dir().join(format!("sparkbox-logging-ssh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("ssh-dev.log");
        assert!(ssh_log_sections(&log).is_empty(), "no log yet: no section");
        std::fs::write(&log, "now\n").unwrap();
        std::fs::write(dir.join("ssh-dev.1.log"), "before\n").unwrap();
        std::fs::write(dir.join("ssh-dev.2.log"), "long ago\n").unwrap();
        let sections = ssh_log_sections(&log);
        let titles: Vec<&str> = sections.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(titles, ["SSH log (ssh-dev.1.log)", "SSH log (ssh-dev.log)"]);
        assert_eq!((sections[0].1.as_str(), sections[1].1.as_str()), ("before\n", "now\n"));
        let s = crate::live::ssh::ChannelStats { in_use: 1, queued: 2, refusals: 3, bulk_connections: 2 };
        assert_eq!(channel_line("cell2", &s), "cell2: 1 channel in use, 2 queued, 3 refused, 2 bulk connections");
    }
}
