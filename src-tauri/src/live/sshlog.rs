//! SparkBox's SSH connection log: one line for every connect, drop and reconnect, so a
//! drop-out can be explained afterwards (round 4's 0/4-online moment couldn't be). A release
//! build writes ~/Library/Logs/SparkBox/ssh.log and a debug build ssh-dev.log, so two
//! SparkBoxes never rotate one file. Each is rotated at 5 MB, three kept (ssh.1.log, ssh.2.log).
//!
//! ```text
//! 2026-10-30T09:12:01.123+01:00 release cell2 connect_ok path=direct addr=192.168.50.154:22 auth=password ms=84 conn=control
//! ```
//!
//! Lines name events, reasons and counts. Passwords, stdin and command text never go in, and
//! every line passes through `audit::redact` on its way out.

use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::audit::redact;

const ROTATE_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: usize = 3;

/// One writer at a time, so rotation and appends from several connections don't interleave.
static WRITE: Mutex<()> = Mutex::new(());

/// This build's log: ssh.log for release, ssh-dev.log for a debug build.
pub fn path() -> PathBuf {
    let name = if cfg!(debug_assertions) { "ssh-dev.log" } else { "ssh.log" };
    super::sshconfig::home().join("Library/Logs/SparkBox").join(name)
}

/// The log (`n` 0) or one of its rotations: ssh.1.log is the one before it.
pub fn rotation(path: &Path, n: usize) -> PathBuf {
    if n == 0 {
        path.to_path_buf()
    } else {
        path.with_extension(format!("{n}.log"))
    }
}

/// A value as it goes in a line: bare when it's one word, quoted when it has spaces, quotes or
/// an equals sign; newlines become spaces.
fn field_value(v: &str) -> String {
    let v = v.trim().replace(['\r', '\n'], " ");
    if !v.is_empty() && !v.contains([' ', '\t', '"', '=']) {
        v
    } else {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// One line, without its newline: time, instance, node, event, then `key=value` fields.
pub fn line(time: &str, instance: &str, node: &str, event: &str, fields: &[(&str, String)]) -> String {
    let mut out = format!("{time} {instance} {} {event}", field_value(node));
    for (k, v) in fields {
        out.push_str(&format!(" {k}={}", field_value(v)));
    }
    redact(&out)
}

/// Now, in the Mac's time zone with its offset, to the millisecond.
fn now() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string()
}

fn append(path: &Path, text: &str) {
    let _one = WRITE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(path).map(|m| m.len() >= ROTATE_BYTES).unwrap_or(false) {
        for i in (1..KEEP).rev() {
            let _ = std::fs::rename(rotation(path, i), rotation(path, i + 1));
        }
        let _ = std::fs::rename(path, rotation(path, 1));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{text}");
    }
}

/// Where Live's connection events go. Tests write nowhere until they name a file.
#[derive(Clone)]
pub struct SshLog(Arc<Mutex<Option<PathBuf>>>);

impl Default for SshLog {
    fn default() -> Self {
        SshLog(Arc::new(Mutex::new(if cfg!(test) { None } else { Some(path()) })))
    }
}

impl SshLog {
    /// A log that writes nothing (connections made outside Live: the Connect wizard's checks).
    pub fn off() -> Self {
        SshLog(Arc::new(Mutex::new(None)))
    }

    #[cfg(test)]
    pub fn set_path(&self, path: PathBuf) {
        *self.0.lock().unwrap() = Some(path);
    }

    pub fn file(&self) -> Option<PathBuf> {
        self.0.lock().unwrap().clone()
    }

    /// Appends one event for `node` ("connect_ok", "frame_stall"…) with its fields.
    pub fn event(&self, node: &str, event: &str, fields: &[(&str, &dyn Display)]) {
        let Some(path) = self.file() else { return };
        let fields: Vec<(&str, String)> = fields.iter().map(|(k, v)| (*k, v.to_string())).collect();
        append(&path, &line(&now(), crate::identity::instance_name(), node, event, &fields));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_reads_time_instance_node_event_then_fields() {
        let l = line("2026-10-30T09:12:01.123+01:00", "release", "cell2", "connect_ok", &[("path", "direct".into()), ("addr", "192.168.50.154:22".into()), ("auth", "password".into()), ("ms", "84".into())]);
        assert_eq!(l, "2026-10-30T09:12:01.123+01:00 release cell2 connect_ok path=direct addr=192.168.50.154:22 auth=password ms=84");
        let l = line("t", "dev", "cell3", "connect_fail", &[("kind", "timeout".into()), ("msg", "SSH timeout: no reply\nfrom \"x\"".into())]);
        assert_eq!(l, r#"t dev cell3 connect_fail kind=timeout msg="SSH timeout: no reply from \"x\"""#);
        assert_eq!(line("t", "dev", "cell1", "reconnect", &[("reason", String::new())]), "t dev cell1 reconnect reason=\"\"");
    }

    #[test]
    fn the_time_is_local_with_its_offset_to_the_millisecond() {
        let t = now();
        let re = regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}[+-]\d\d:\d\d$").unwrap();
        assert!(re.is_match(&t), "{t}");
    }

    #[test]
    fn secrets_in_a_message_are_masked() {
        let l = line("t", "dev", "cell1", "connect_fail", &[("msg", "rejected password=hunter2 for HF_TOKEN=hf_abcdefghijklmnopqrstuvwxyz0123 and hf_AbCdEfGh12345678".into())]);
        assert!(!l.contains("hunter2") && !l.contains("hf_abcdefghij") && !l.contains("hf_AbCdEfGh"), "{l}");
        assert!(l.contains("password=‹redacted›"), "{l}");
    }

    #[test]
    fn the_file_rotates_at_5_mb_keeping_three() {
        let dir = std::env::temp_dir().join(format!("sparkbox-sshlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = SshLog::off();
        assert!(log.file().is_none());
        log.event("cell1", "connect_start", &[]); // writes nowhere
        let path = dir.join("ssh-dev.log");
        log.set_path(path.clone());
        for round in 0..4 {
            log.event("cell1", "connect_ok", &[("round", &round)]);
            std::fs::OpenOptions::new().append(true).open(&path).unwrap().set_len(ROTATE_BYTES).unwrap();
        }
        log.event("cell1", "connect_ok", &[("round", &"last")]);
        let current = std::fs::read_to_string(&path).unwrap();
        assert_eq!(current.lines().count(), 1, "a fresh file after the rotation");
        assert!(current.contains(" dev cell1 connect_ok round=last"), "{current}");
        assert!(rotation(&path, 1).exists() && rotation(&path, 2).exists() && rotation(&path, 3).exists());
        assert!(!rotation(&path, 4).exists(), "three kept");
        assert_eq!(rotation(&path, 1).file_name().unwrap(), "ssh-dev.1.log");
        assert!(std::fs::read_to_string(rotation(&path, 1)).unwrap().contains("round=3"), "the newest rotation is .1");
    }

    #[test]
    fn release_and_dev_builds_write_different_files() {
        let p = path();
        assert_eq!(p.parent().unwrap(), super::super::sshconfig::home().join("Library/Logs/SparkBox"));
        assert_eq!(p.file_name().unwrap(), if cfg!(debug_assertions) { "ssh-dev.log" } else { "ssh.log" });
        assert_eq!(crate::identity::instance_name(), if cfg!(debug_assertions) { "dev" } else { "release" });
        assert!(SshLog::default().file().is_none(), "tests never write the real log");
    }
}
