//! Evidence capture (Settings › Diagnostics › Capture evidence): what every connected node looks
//! like at one moment, before and after a supervised step, filed with the activity log and the
//! SSH log. It works the same in the release build and a debug build.
//!
//! Reads only. Each of `STEPS` is one `Live::probe` (SparkBox's own read-only checks, so not in
//! the activity log) with 20 s to answer. Nodes run in parallel, each node's steps one after the
//! other, and a step that fails is written down and the rest carry on. One activity row says it
//! happened ("Evidence captured", a read on the Mac: under Everything, not Changes).
//!
//! ```text
//! ~/Library/Logs/SparkBox/evidence/<YYYY-MM-DD>-<phase>/<HHMMSS>-<label>/
//!   <node>/<step>.txt          the command, its exit code, stdout and stderr
//!   manifest.json              instance, build, commands, codes, durations, nodes not connected, problems
//!   diff-from-<previous>.txt   diff -u of every step against the phase's capture before it (any day)
//!   activity.jsonl             the activity log since the previous capture (the hour before, for the first)
//!   ssh.log | ssh-dev.log      a copy of this build's SSH connection log, if there is one
//! ```
//!
//! The manifest is written first, marked not finished, and again at the end. Once the node steps
//! are on disk, a part that fails (the diff, the activity export, the SSH log copy, a file) goes
//! into the manifest's `problems` and the capture still files.
//!
//! Everything written passes through `audit::redact` first. JSON (the manifest, the activity
//! rows, docker's `{{json .}}` lines) is redacted value by value, so it still parses.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::identity;
use crate::live::audit::{redact, Entry, REDACTED};
use crate::live::ssh::Output;
use crate::live::Live;

/// Each step's name (its file) and the command it runs on the node.
pub const STEPS: &[(&str, &str)] = &[
    ("docker-ps", "docker ps -a --no-trunc --format '{{json .}}'"),
    ("docker-images", "docker images --digests --no-trunc --format '{{json .}}'"),
    ("systemctl-failed", "systemctl --failed --no-legend --plain"),
    ("df", "df -B1 /"),
    ("models", "ls -la ~/models ~/.cache/huggingface/hub"),
    ("apt-upgradable", "apt list --upgradable 2>/dev/null"),
    ("apt-holds", "apt-mark showhold"),
    ("dpkg", "dpkg-query -W"),
    ("nvidia-smi", "nvidia-smi --query-gpu=driver_version,pstate,power.draw --format=csv"),
    ("ip-addr", "ip -br addr"),
    ("listening", "ss -Hltn"),
    ("mem-available", "grep MemAvailable /proc/meminfo"),
    ("dev-shm", "ls -la /dev/shm"),
    ("shell", r#"ls -l /bin/sh /usr/bin/sh; readlink -f /bin/sh /usr/bin/sh; type sh; dpkg-query -W -f='${Package} ${Version}\n' dash bash; getent passwd "$USER""#),
];

/// How long one step may take.
pub const LIMIT: Duration = Duration::from_secs(20);

/// How many captures Settings › Diagnostics can list.
const LISTED: usize = 20;

/// The activity a first capture in a phase folder takes along.
const FIRST_ACTIVITY_MS: u64 = 60 * 60 * 1000;

const DEMO: &str = "Available with Live data.";

/// The problem a manifest carries until the capture finishes.
const UNFINISHED: &str = "Not finished: still running, or SparkBox quit while it ran.";

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// ~/Library/Logs/SparkBox/evidence.
pub fn root(logs: &Path) -> PathBuf {
    logs.join("evidence")
}

/// This build's SSH log in `logs`, named as Live writes it (`live::sshlog::path`): ssh.log for
/// release, ssh-dev.log for a debug build.
fn ssh_log(logs: &Path) -> PathBuf {
    logs.join(crate::live::sshlog::path().file_name().unwrap_or_default())
}

/// A folder-safe name: lower case letters, digits and dashes, at most 40 characters.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(40).collect();
    out.trim_end_matches('-').to_string()
}

/// One step on one node, as the manifest records it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StepRecord {
    pub step: String,
    /// The exit code; None when the command didn't run or was stopped.
    pub code: Option<u32>,
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeRecord {
    pub node: String,
    pub steps: Vec<StepRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub step: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub instance: String,
    pub build: String,
    pub phase: String,
    pub label: String,
    pub started_ms: u64,
    /// 0 until the capture finishes.
    pub finished_ms: u64,
    pub commands: Vec<Command>,
    pub nodes: Vec<NodeRecord>,
    pub not_connected: Vec<String>,
    /// The capture this one is compared with, as `<date>-<phase>/<time>-<label>` under the
    /// evidence folder (captures before P1 round 2 name only the `<time>-<label>` folder).
    pub previous: Option<String>,
    pub activity_from_ms: u64,
    /// The SSH log file copied along, if there was one.
    pub ssh_log: Option<String>,
    /// What couldn't be filed (the diff, the activity, a file), or UNFINISHED.
    #[serde(default)]
    pub problems: Vec<String>,
}

/// What a capture made, for the Diagnostics list and the button's result.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceFolder {
    pub path: String,
    pub phase: String,
    pub label: String,
    pub started_ms: u64,
    pub nodes: usize,
    /// Steps that didn't exit 0.
    pub failed_steps: usize,
    pub not_connected: Vec<String>,
    /// diff-from-<previous>.txt, when there was a capture to compare with.
    pub diff: Option<String>,
    pub problems: Vec<String>,
}

/// Runs one command on one node; Live's is `Live::probe`.
pub type Runner = Box<dyn Fn(String, &'static str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Output, String>> + Send>> + Send + Sync>;

/// What to capture and where.
pub struct Request<'a> {
    pub logs: &'a Path,
    pub phase: &'a str,
    pub label: &'a str,
    pub nodes: Vec<String>,
    pub not_connected: Vec<String>,
    pub at: jiff::Zoned,
}

/// Redacts every string in a JSON value on its own, so the JSON stays valid: redacting its text
/// can take the backslash of an escaped quote (`\"API_KEY=…\"`) and end the string early. A
/// string under a key `redact` treats as secret in JSON text (`"api_key": …`) is masked whole.
fn redact_json(v: &mut Value) {
    match v {
        Value::String(s) => *s = redact(s),
        Value::Array(items) => items.iter_mut().for_each(redact_json),
        Value::Object(fields) => {
            *fields = std::mem::take(fields)
                .into_iter()
                .map(|(key, mut field)| {
                    match &mut field {
                        Value::String(s) if secret_key(&key) => *s = REDACTED.into(),
                        _ => redact_json(&mut field),
                    }
                    (redact(&key), field)
                })
                .collect();
        }
        _ => {}
    }
}

/// Whether `redact` masks a JSON string under this key.
fn secret_key(key: &str) -> bool {
    let probe = format!("{{{}:\"x\"}}", Value::String(key.into()));
    redact(&probe) != probe
}

/// A step's output or a diff of them, redacted. A line that is JSON (docker's `{{json .}}`, also
/// after a diff's `+`, `-` or space) is redacted value by value, and stays as the node wrote it
/// when there's nothing to hide; the lines between are redacted together as text. Text with a
/// private key block is redacted whole, since the block spans lines.
fn redact_output(text: &str) -> String {
    if text.contains("PRIVATE KEY") {
        return redact(text);
    }
    let mut out = String::new();
    let mut between = String::new();
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let (mark, rest) = match body.as_bytes().first() {
            Some(b'+' | b'-' | b' ') => body.split_at(1),
            _ => ("", body),
        };
        match serde_json::from_str::<Value>(rest).ok().filter(|v| v.is_object() || v.is_array()) {
            Some(mut v) => {
                out.push_str(&redact(&std::mem::take(&mut between)));
                let before = v.clone();
                redact_json(&mut v);
                out.push_str(mark);
                out.push_str(&if v == before { rest.to_string() } else { v.to_string() });
                out.push_str(&line[body.len()..]);
            }
            None => between.push_str(line),
        }
    }
    out.push_str(&redact(&between));
    out
}

/// A step's file: the command, how it ended, then its output, redacted.
fn step_text(cmd: &str, result: &Result<Output, String>) -> String {
    match result {
        Ok(o) => {
            let ended = match (o.code, &o.signal, o.timed_out) {
                (_, _, true) => format!("timed out after {} s", LIMIT.as_secs()),
                (Some(c), _, _) => format!("exit {c}"),
                (None, Some(sig), _) => format!("killed by SIG{sig}"),
                (None, None, _) => "no exit status".to_string(),
            };
            let cut = if o.truncated { " (output cut short)" } else { "" };
            format!("$ {cmd}\n{ended}{cut}\n--- stdout\n{}\n--- stderr\n{}\n", redact_output(o.stdout.trim_end()), redact(o.stderr.trim_end()))
        }
        Err(e) => format!("$ {cmd}\nnot run: {}\n", redact(e)),
    }
}

/// The capture folders in a phase folder, oldest first.
fn captures_in(phase_dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(phase_dir)
        .map(|it| it.flatten().map(|e| e.path()).filter(|p| p.join("manifest.json").is_file()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).ok()?).ok()
}

/// The phase's last finished capture, whatever day it was in (a session can run past midnight),
/// with its manifest and its name under the evidence folder (`<date>-<phase>/<time>-<label>`).
fn last_capture(evidence: &Path, phase: &str) -> Option<(PathBuf, Manifest, String)> {
    let days = std::fs::read_dir(evidence).ok()?;
    let mut all: Vec<(PathBuf, Manifest)> = days
        .flatten()
        .map(|d| d.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // <YYYY-MM-DD>-<phase>, the phase exactly (p1 isn't p1-gate).
            name.len() > 11 && name.is_char_boundary(10) && name[..10].parse::<jiff::civil::Date>().is_ok() && name[10..] == format!("-{phase}")
        })
        .flat_map(|p| captures_in(&p))
        .filter_map(|dir| Some((dir.clone(), read_manifest(&dir)?)))
        .filter(|(_, m)| m.finished_ms > 0)
        .collect();
    all.sort_by(|a, b| (a.1.started_ms, &a.0).cmp(&(b.1.started_ms, &b.0)));
    let (dir, m) = all.pop()?;
    let name = dir.strip_prefix(evidence).ok()?.to_string_lossy().into_owned();
    Some((dir, m, name))
}

/// The capture folder's name in a manifest's `previous` (the last part of the path).
fn folder_name(previous: &str) -> &str {
    previous.rsplit('/').next().unwrap_or(previous)
}

/// Writes the manifest, redacted value by value, and gives back what was written.
fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<Manifest, String> {
    let mut v = serde_json::to_value(manifest).map_err(|e| e.to_string())?;
    redact_json(&mut v);
    // Back through the struct, so the file keeps the struct's field order.
    let manifest: Manifest = serde_json::from_value(v).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("manifest.json"), json).map_err(|e| format!("Couldn't write the manifest in {}: {e}", dir.display()))?;
    Ok(manifest)
}

/// The activity export (a JSON array) as one redacted row a line.
fn activity_lines(export: &str) -> Result<String, String> {
    let rows: Vec<Value> = serde_json::from_str(export).map_err(|e| format!("the export isn't a JSON array: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|mut r| {
            redact_json(&mut r);
            format!("{r}\n")
        })
        .collect())
}

/// Captures every node in `req` with `run`, then files it all. `activity(from_ms, to_ms)` gives
/// the activity log as JSON (an array of entries). Once the node steps have run it returns the
/// folder, with what couldn't be filed in `problems`; only a manifest that can't be written fails.
pub async fn capture(req: Request<'_>, run: &Runner, activity: impl FnOnce(u64, u64) -> Result<String, String>) -> Result<EvidenceFolder, String> {
    let started_ms = now_ms();
    let phase = Some(slug(req.phase)).filter(|s| !s.is_empty()).unwrap_or_else(|| "p1".into());
    let label = Some(slug(req.label)).filter(|s| !s.is_empty()).unwrap_or_else(|| "capture".into());
    let evidence = root(req.logs);
    let phase_dir = evidence.join(format!("{}-{phase}", req.at.strftime("%Y-%m-%d")));
    let previous = last_capture(&evidence, &phase);
    let base = format!("{}-{label}", req.at.strftime("%H%M%S"));
    let mut dir = phase_dir.join(&base);
    for n in 2.. {
        if !dir.exists() {
            break;
        }
        dir = phase_dir.join(format!("{base}-{n}"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;

    let activity_from_ms = previous.as_ref().map(|(_, m, _)| m.started_ms).unwrap_or(started_ms.saturating_sub(FIRST_ACTIVITY_MS));
    let mut manifest = Manifest {
        instance: identity::instance_name().into(),
        build: option_env!("SPARKBOX_BUILD_TAG").unwrap_or("untagged").into(),
        phase: phase.clone(),
        label: label.clone(),
        started_ms,
        finished_ms: 0,
        commands: STEPS.iter().map(|(s, c)| Command { step: s.to_string(), command: c.to_string() }).collect(),
        nodes: req.nodes.iter().map(|n| NodeRecord { node: n.clone(), steps: Vec::new() }).collect(),
        not_connected: req.not_connected.clone(),
        previous: previous.as_ref().map(|(_, _, name)| name.clone()),
        activity_from_ms,
        ssh_log: None,
        problems: vec![UNFINISHED.into()],
    };
    // Listed (as not finished) even if SparkBox quits before the end.
    write_manifest(&dir, &manifest)?;

    // Every node at once; each node's steps in turn, so a node's SSH sessions stay few.
    let per_node = req.nodes.iter().map(|node| async move {
        let mut done = Vec::new();
        for &(step, cmd) in STEPS {
            let t = Instant::now();
            let result = run(node.clone(), cmd).await;
            done.push((step, cmd, result, t.elapsed().as_millis() as u64));
        }
        (node.clone(), done)
    });
    let results = futures_util::future::join_all(per_node).await;

    // From here on what fails is written down and the rest carries on.
    let mut problems = Vec::new();
    let mut nodes = Vec::new();
    for (node, steps) in results {
        let node_dir = dir.join(slug(&node));
        let can_write = match std::fs::create_dir_all(&node_dir) {
            Ok(()) => true,
            Err(e) => {
                problems.push(format!("Couldn't create {}: {e}", node_dir.display()));
                false
            }
        };
        let mut records = Vec::new();
        for (step, cmd, result, ms) in steps {
            let file = node_dir.join(format!("{step}.txt"));
            if can_write {
                if let Err(e) = std::fs::write(&file, step_text(cmd, &result)) {
                    problems.push(format!("Couldn't write {}: {e}", file.display()));
                }
            }
            let (code, error) = match &result {
                Ok(o) if o.timed_out => (None, Some(format!("timed out after {} s", LIMIT.as_secs()))),
                Ok(o) => (o.code, o.signal.as_ref().map(|s| format!("killed by SIG{s}"))),
                Err(e) => (None, Some(e.clone())),
            };
            records.push(StepRecord { step: step.to_string(), code, ms, error });
        }
        nodes.push(NodeRecord { node, steps: records });
    }

    let diff = match &previous {
        // About fifty short diff processes: on a blocking thread, not an async worker.
        Some((prev, prev_manifest, name)) => {
            let (prev, prev_manifest, name, dir, nodes) = (prev.clone(), prev_manifest.clone(), name.clone(), dir.clone(), nodes.clone());
            match tokio::task::spawn_blocking(move || write_diff(&prev, &name, &prev_manifest, &dir, &nodes)).await.map_err(|e| e.to_string()).and_then(|r| r) {
                Ok(path) => Some(path),
                Err(e) => {
                    problems.push(format!("No diff against {}: {e}", manifest.previous.as_deref().unwrap_or_default()));
                    None
                }
            }
        }
        None => None,
    };

    let finished_ms = now_ms();
    match activity(activity_from_ms, finished_ms).and_then(|export| activity_lines(&export)) {
        Ok(jsonl) => {
            if let Err(e) = std::fs::write(dir.join("activity.jsonl"), jsonl) {
                problems.push(format!("Couldn't write activity.jsonl: {e}"));
            }
        }
        Err(e) => problems.push(format!("No activity log: {e}")),
    }

    let ssh = ssh_log(req.logs);
    let name = ssh.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ssh_copied = match std::fs::read_to_string(&ssh) {
        Ok(text) => match std::fs::write(dir.join(&name), redact(&text)) {
            Ok(()) => Some(name),
            Err(e) => {
                problems.push(format!("Couldn't copy {name}: {e}"));
                None
            }
        },
        // No SSH log yet is nothing to report.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            problems.push(format!("Couldn't read {}: {e}", ssh.display()));
            None
        }
    };

    manifest.finished_ms = finished_ms;
    manifest.nodes = nodes;
    manifest.ssh_log = ssh_copied;
    manifest.problems = problems;
    let manifest = write_manifest(&dir, &manifest)?;
    Ok(folder(&dir, &manifest, diff))
}

/// `/usr/bin/diff -u` of every node's every step against the previous capture (`name` under the
/// evidence folder), in one file.
fn write_diff(prev: &Path, name: &str, prev_manifest: &Manifest, dir: &Path, nodes: &[NodeRecord]) -> Result<String, String> {
    let mut out = format!("Compared with {name}\n");
    let mut changed = 0;
    for gone in prev_manifest.nodes.iter().filter(|p| !nodes.iter().any(|n| n.node == p.node)) {
        changed += 1;
        out.push_str(&format!("{} was captured then, not this time.\n", gone.node));
    }
    for n in nodes {
        for s in &n.steps {
            let rel = Path::new(&slug(&n.node)).join(format!("{}.txt", s.step));
            // -N: a node or step the other capture doesn't have reads as an empty file.
            let d = std::process::Command::new("/usr/bin/diff")
                .arg("-u")
                .arg("-N")
                .arg(prev.join(&rel))
                .arg(dir.join(&rel))
                .output()
                .map_err(|e| format!("Couldn't run diff: {e}"))?;
            match d.status.code() {
                Some(0) => {}
                Some(1) => {
                    changed += 1;
                    out.push_str(&String::from_utf8_lossy(&d.stdout));
                }
                _ => out.push_str(&format!("diff {}: {}\n", rel.display(), String::from_utf8_lossy(&d.stderr).trim())),
            }
        }
    }
    if changed == 0 {
        out.push_str("No differences.\n");
    }
    let path = dir.join(format!("diff-from-{}.txt", folder_name(name)));
    // Again, in case the capture before was an older build's.
    std::fs::write(&path, redact_output(&out)).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

fn folder(dir: &Path, m: &Manifest, diff: Option<String>) -> EvidenceFolder {
    EvidenceFolder {
        path: dir.display().to_string(),
        phase: m.phase.clone(),
        label: m.label.clone(),
        started_ms: m.started_ms,
        nodes: m.nodes.len(),
        failed_steps: m.nodes.iter().flat_map(|n| &n.steps).filter(|s| s.code != Some(0)).count(),
        not_connected: m.not_connected.clone(),
        diff,
        problems: m.problems.clone(),
    }
}

/// The last `LISTED` captures under `logs`, newest first.
pub fn list(logs: &Path) -> Vec<EvidenceFolder> {
    let Ok(phases) = std::fs::read_dir(root(logs)) else { return Vec::new() };
    let mut all: Vec<EvidenceFolder> = phases
        .flatten()
        .flat_map(|p| captures_in(&p.path()))
        .filter_map(|dir| {
            let m = read_manifest(&dir)?;
            let diff = m.previous.as_ref().map(|p| dir.join(format!("diff-from-{}.txt", folder_name(p)))).filter(|p| p.is_file()).map(|p| p.display().to_string());
            Some(folder(&dir, &m, diff))
        })
        .collect();
    all.sort_by_key(|f| std::cmp::Reverse(f.started_ms));
    all.truncate(LISTED);
    all
}

/// Captures every connected node through `live` and adds the one activity row.
pub async fn capture_live(live: &std::sync::Arc<Live>, node_ids: &[String], logs: &Path, phase: &str, label: &str) -> Result<EvidenceFolder, String> {
    let (nodes, not_connected): (Vec<String>, Vec<String>) = node_ids.iter().cloned().partition(|id| live.is_connected(id));
    if nodes.is_empty() {
        return Err("No node is connected, so there's nothing to capture.".into());
    }
    let probe = live.clone();
    let run: Runner = Box::new(move |node, cmd| {
        let live = probe.clone();
        Box::pin(async move { live.probe(&node, cmd, LIMIT).await })
    });
    let at = jiff::Zoned::now();
    let req = Request { logs, phase, label, nodes, not_connected, at };
    let made = capture(req, &run, |from, to| live.audit.export("json", from, to, None)).await?;
    let nodes = if made.nodes == 1 { "1 node".to_string() } else { format!("{} nodes", made.nodes) };
    // A read you asked for, so under Everything and not under Changes; its own operation, so the
    // window keeps it with what you started rather than with SparkBox's checks.
    live.audit.record(Entry {
        node: "mac".into(),
        kind: "app".into(),
        command: format!("{} · {nodes} · {}", made.label, made.path),
        title: "Evidence captured".into(),
        class: "read".into(),
        source: "ui".into(),
        op_id: Some(format!("evidence-{}", made.started_ms)),
        state: "ok".into(),
        ..Default::default()
    });
    Ok(made)
}

fn logs_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    Ok(app.path().home_dir().map_err(|e| e.to_string())?.join("Library/Logs/SparkBox"))
}

/// Settings › Diagnostics › Capture evidence. Reads the nodes; writes new files only under
/// ~/Library/Logs/SparkBox/evidence, like the app's own logs.
#[tauri::command]
pub async fn evidence_capture(app: tauri::AppHandle, phase: String, label: String) -> Result<EvidenceFolder, String> {
    use tauri::Manager;
    let state = app.state::<crate::AppState>();
    if !crate::is_live(&state) {
        return Err(DEMO.into());
    }
    let ids: Vec<String> = state.inventory.lock().unwrap().nodes.iter().map(|n| n.id.clone()).collect();
    let live = state.live.clone();
    capture_live(&live, &ids, &logs_dir(&app)?, &phase, &label).await
}

/// The last 20 captures, newest first.
#[tauri::command]
pub fn evidence_list(app: tauri::AppHandle) -> Result<Vec<EvidenceFolder>, String> {
    Ok(list(&logs_dir(&app)?))
}

/// One text file an operation files as evidence (an upgrade's package list from before it, the
/// packages a pin held): `<logs>/evidence/<YYYY-MM-DD>-<phase>/<name>`, the phase and the name
/// slugged (the name keeps its extension), redacted, readable by you only (0600). Gives back
/// where it went.
pub fn file_text(logs: &Path, phase: &str, name: &str, text: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let phase = Some(slug(phase)).filter(|s| !s.is_empty()).unwrap_or_else(|| "p1".into());
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let file = match (slug(stem), slug(ext)) {
        (s, _) if s.is_empty() => return Err(format!("{name:?} isn't a file name SparkBox can use.")),
        (s, e) if e.is_empty() => s,
        (s, e) => format!("{s}.{e}"),
    };
    let dir = root(logs).join(format!("{}-{phase}", jiff::Zoned::now().strftime("%Y-%m-%d")));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let path = dir.join(file);
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    // A file already there keeps its mode on open: made private again.
    f.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    f.write_all(redact(text).as_bytes()).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_HF: &str = "hf_AbCdEfGhIjKlMnOpQrStUvWxYz012345";

    fn out(code: u32, stdout: &str, stderr: &str) -> Output {
        Output { code: Some(code), stdout: stdout.into(), stderr: stderr.into(), ..Default::default() }
    }

    /// Answers like a node would; `free` is what df reports free, so two captures differ there.
    fn runner(free: u64) -> Runner {
        Box::new(move |node, cmd| {
            Box::pin(async move {
                if node == "gx10-0000" && cmd.starts_with("dpkg-query") {
                    return Err("Lost the connection to gx10-0000".into());
                }
                Ok(match cmd {
                    c if c.starts_with("docker ps") => out(0, &format!("{{\"Names\":\"vllm\",\"Command\":\"vllm serve --hf-token {FAKE_HF}\"}}"), ""),
                    c if c.starts_with("df ") => out(0, &format!("Filesystem 1B-blocks Used Available Use% Mounted on\n/dev/nvme0n1p2 3936501538816 1000 {free} 26% /"), ""),
                    c if c.starts_with("grep MemAvailable") => out(0, "MemAvailable:   118000000 kB", ""),
                    c if c.starts_with("ls -la ~/models") => out(2, "", "ls: cannot access '/home/sparkuser/models': No such file or directory"),
                    _ => out(0, "", ""),
                })
            })
        })
    }

    fn at(h: i8, m: i8, s: i8) -> jiff::Zoned {
        at_on(29, h, m, s)
    }

    fn at_on(day: i8, h: i8, m: i8, s: i8) -> jiff::Zoned {
        jiff::civil::date(2026, 10, day).at(h, m, s, 0).to_zoned(jiff::tz::TimeZone::UTC).unwrap()
    }

    fn no_activity(_: u64, _: u64) -> Result<String, String> {
        Ok("[]".into())
    }

    fn manifest_of(path: &str) -> Manifest {
        serde_json::from_slice(&std::fs::read(Path::new(path).join("manifest.json")).unwrap()).unwrap()
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                } else {
                    out.push(e.path());
                }
            }
        }
        out
    }

    #[test]
    fn every_step_is_a_read() {
        use crate::ops::classify::{classify, Kind};
        for (step, cmd) in STEPS {
            assert!(matches!(classify(cmd), Kind::Probe | Kind::Read), "{step}: {cmd} classifies as {:?}", classify(cmd));
        }
    }

    #[test]
    fn slugs_are_folder_safe() {
        assert_eq!(slug("Before: restart vLLM on cell2!"), "before-restart-vllm-on-cell2");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("  "), "");
        assert_eq!(slug(&"x".repeat(60)).len(), 40);
    }

    #[tokio::test]
    async fn a_capture_files_every_step_redacted_and_diffs_the_one_before() {
        let logs = crate::fake_node::temp("evidence-unit");
        std::fs::write(logs.join(if cfg!(debug_assertions) { "ssh-dev.log" } else { "ssh.log" }), format!("connect_ok cell1 token={FAKE_HF}\n")).unwrap();
        let nodes = vec!["cell1".to_string(), "gx10-0000".to_string()];
        let activity = |_: u64, _: u64| Ok(serde_json::json!([{ "title": "Restart vllm", "command": "docker restart vllm" }, { "title": "Key saved", "command": "OPENAI_API_KEY=sk-proj-AbCdEf0123456789xyz" }]).to_string());

        let req = Request { logs: &logs, phase: "P1", label: "Before restart", nodes: nodes.clone(), not_connected: vec!["cell3".into()], at: at(10, 15, 0) };
        let first = capture(req, &runner(2_900_000_000_000), activity).await.unwrap();
        let dir = logs.join("evidence/2026-10-29-p1/101500-before-restart");
        assert_eq!(PathBuf::from(&first.path), dir);
        assert_eq!((first.nodes, first.not_connected.clone(), first.diff.clone()), (2, vec!["cell3".to_string()], None));
        assert_eq!(first.problems, Vec::<String>::new(), "finished, nothing missing");
        // One file per node per step, the manifest, the activity and the SSH log.
        for node in &nodes {
            for (step, _) in STEPS {
                assert!(dir.join(node).join(format!("{step}.txt")).is_file(), "{node}/{step}");
            }
        }
        assert_eq!(files_under(&dir).len(), 2 * STEPS.len() + 3);
        let df = std::fs::read_to_string(dir.join("cell1/df.txt")).unwrap();
        assert!(df.starts_with("$ df -B1 /\nexit 0\n--- stdout\nFilesystem"), "{df}");
        let missing = std::fs::read_to_string(dir.join("cell1/models.txt")).unwrap();
        assert!(missing.contains("exit 2") && missing.contains("No such file or directory"), "{missing}");
        let lost = std::fs::read_to_string(dir.join("gx10-0000/dpkg.txt")).unwrap();
        assert_eq!(lost, "$ dpkg-query -W\nnot run: Lost the connection to gx10-0000\n");

        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m.instance, identity::instance_name());
        assert_eq!((m.phase.as_str(), m.label.as_str()), ("p1", "before-restart"));
        assert_eq!(m.commands.len(), STEPS.len());
        assert_eq!(m.commands[3].command, "df -B1 /");
        assert_eq!(m.not_connected, ["cell3"]);
        assert_eq!(m.previous, None);
        let gx = m.nodes.iter().find(|n| n.node == "gx10-0000").unwrap();
        let dpkg = gx.steps.iter().find(|s| s.step == "dpkg").unwrap();
        assert_eq!((dpkg.code, dpkg.error.as_deref()), (None, Some("Lost the connection to gx10-0000")));
        // The failed listing on both nodes and gx10's lost step: the rest went on.
        assert_eq!(first.failed_steps, 3);
        assert!(m.ssh_log.is_some());

        // The activity, one entry a line; every file redacted (the hf_ token, the sk- key).
        let activity_text = std::fs::read_to_string(dir.join("activity.jsonl")).unwrap();
        assert_eq!(activity_text.lines().count(), 2);
        for f in files_under(&dir) {
            let text = std::fs::read_to_string(&f).unwrap();
            assert!(!text.contains(FAKE_HF) && !text.contains("sk-proj-AbCdEf"), "{} keeps a secret", f.display());
        }
        assert!(std::fs::read_to_string(dir.join("cell1/docker-ps.txt")).unwrap().contains("--hf-token"), "the rest of the line stays");

        // A minute later, df reads less free: the diff says so, and only that.
        let req = Request { logs: &logs, phase: "p1", label: "after restart", nodes: nodes.clone(), not_connected: vec![], at: at(10, 16, 0) };
        let second = capture(req, &runner(2_800_000_000_000), activity).await.unwrap();
        let diff_path = second.diff.clone().expect("a diff against the first");
        assert!(diff_path.ends_with("101600-after-restart/diff-from-101500-before-restart.txt"), "{diff_path}");
        let diff = std::fs::read_to_string(&diff_path).unwrap();
        assert!(diff.starts_with("Compared with 2026-10-29-p1/101500-before-restart\n"), "{diff}");
        assert!(diff.contains("-/dev/nvme0n1p2 3936501538816 1000 2900000000000 26% /"), "{diff}");
        assert!(diff.contains("+/dev/nvme0n1p2 3936501538816 1000 2800000000000 26% /"), "{diff}");
        assert_eq!(diff.matches("\n+++ ").count(), 2, "df changed on both nodes, nothing else: {diff}");
        let m2: Manifest = serde_json::from_slice(&std::fs::read(logs.join("evidence/2026-10-29-p1/101600-after-restart/manifest.json")).unwrap()).unwrap();
        assert_eq!(m2.previous.as_deref(), Some("2026-10-29-p1/101500-before-restart"));
        assert_eq!(m2.activity_from_ms, m.started_ms, "the activity since the capture before");

        // A third with one node gone says so; the same second twice gets its own folder.
        let req = Request { logs: &logs, phase: "p1", label: "after restart", nodes: vec!["cell1".into()], not_connected: vec!["gx10-0000".into()], at: at(10, 16, 0) };
        let third = capture(req, &runner(2_800_000_000_000), activity).await.unwrap();
        assert!(third.path.ends_with("101600-after-restart-2"), "{}", third.path);
        let diff = std::fs::read_to_string(third.diff.clone().unwrap()).unwrap();
        assert!(diff.contains("gx10-0000 was captured then, not this time."), "{diff}");
        assert!(!diff.contains("No differences."));

        // Listed newest first.
        let listed = list(&logs);
        assert_eq!(listed.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), [third.path.clone(), second.path.clone(), first.path.clone()]);
        assert_eq!(listed[1].diff, second.diff);
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[tokio::test]
    async fn json_stays_json_after_redaction() {
        let logs = crate::fake_node::temp("evidence-json");
        // What docker inspect's env looks like in a row's stdout tail: quoted, so escaped in JSON.
        let tail = "[\n \"API_KEY=sk-proj-AbCdEf0123456789xyz\",\n \"UCX_NET_DEVICES=enP7s7\"\n]";
        let rows = serde_json::json!([{ "title": "Inspect vllm", "command": "docker inspect vllm", "stdoutTail": tail }, { "title": "Saved", "command": "x", "api_key": "plain-secret-value" }]).to_string();
        // Redacting the text, as captures before this one did, ends the JSON string early.
        let line = format!("{}", serde_json::from_str::<Vec<Value>>(&rows).unwrap()[0]);
        assert!(serde_json::from_str::<Value>(&redact(&line)).is_err(), "{}", redact(&line));
        let run: Runner = Box::new(|_, cmd| {
            Box::pin(async move {
                Ok(match cmd {
                    // docker ps quotes the command, so its quotes are escaped too.
                    c if c.starts_with("docker ps") => out(0, "{\"Command\":\"\\\"/entrypoint.sh API_KEY=sk-proj-AbCdEf0123456789xyz\\\"\",\"ID\":\"7da4\",\"Names\":\"vllm\"}\n{\"Command\":\"\\\"/bin/bash /glm53/inner.sh 0\\\"\",\"ID\":\"8eb5\",\"Labels\":\"maintainer=NVIDIA \\u003cx\\u003e\",\"Names\":\"glm\"}", ""),
                    _ => out(0, "", ""),
                })
            })
        });
        let req = Request { logs: &logs, phase: "p1", label: "json", nodes: vec!["cell1".into()], not_connected: vec![], at: at(9, 0, 0) };
        let made = capture(req, &run, |_, _| Ok(rows.clone())).await.unwrap();
        let dir = PathBuf::from(&made.path);

        let activity = std::fs::read_to_string(dir.join("activity.jsonl")).unwrap();
        let parsed: Vec<Value> = activity.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}"))).collect();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["stdoutTail"], format!("[\n \"API_KEY={REDACTED}\",\n \"UCX_NET_DEVICES=enP7s7\"\n]"));
        assert_eq!(parsed[1]["api_key"], REDACTED, "a secret-named field, masked as in text");
        assert!(!activity.contains("sk-proj-AbCdEf") && !activity.contains("plain-secret-value"));

        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m.label, "json");

        // docker's lines: the one with a secret still parses; the other is byte for byte the node's.
        let ps = std::fs::read_to_string(dir.join("cell1/docker-ps.txt")).unwrap();
        let stdout: Vec<&str> = ps.split("--- stdout\n").nth(1).unwrap().split("\n--- stderr").next().unwrap().lines().collect();
        assert_eq!(stdout.len(), 2);
        let first: Value = serde_json::from_str(stdout[0]).unwrap_or_else(|e| panic!("{e}: {}", stdout[0]));
        assert_eq!(first["Command"], format!("\"/entrypoint.sh API_KEY={REDACTED}\""));
        assert_eq!(stdout[1], "{\"Command\":\"\\\"/bin/bash /glm53/inner.sh 0\\\"\",\"ID\":\"8eb5\",\"Labels\":\"maintainer=NVIDIA \\u003cx\\u003e\",\"Names\":\"glm\"}");
        assert!(!ps.contains("sk-proj-AbCdEf"));

        // The next capture's diff shows that line went: still JSON after the mark.
        let gone: Runner = Box::new(|_, _| Box::pin(async { Ok(out(0, "", "")) }));
        let req = Request { logs: &logs, phase: "p1", label: "json-after", nodes: vec!["cell1".into()], not_connected: vec![], at: at(9, 1, 0) };
        let after = capture(req, &gone, |_, _| Ok("[]".into())).await.unwrap();
        let diff = std::fs::read_to_string(after.diff.unwrap()).unwrap();
        let removed: Vec<&str> = diff.lines().filter(|l| l.starts_with("-{")).collect();
        assert_eq!(removed.len(), 2, "{diff}");
        let v: Value = serde_json::from_str(&removed[0][1..]).unwrap_or_else(|e| panic!("{e}: {}", removed[0]));
        assert_eq!(v["Command"], format!("\"/entrypoint.sh API_KEY={REDACTED}\""));
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn output_is_redacted_json_line_by_json_line_and_the_rest_as_text() {
        let line = "{\"Command\":\"\\\"run API_KEY=sk-proj-AbCdEf0123456789xyz\\\"\"}";
        let text = format!("Compared with x\n--- a/cell1/docker-ps.txt\n+++ b/cell1/docker-ps.txt\n@@ -1 +1 @@\n-{line}\n+{line}\n token=hf_AbCdEfGhIjKlMnOpQrStUvWxYz012345 \n");
        let out = redact_output(&text);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(&lines[..4], ["Compared with x", "--- a/cell1/docker-ps.txt", "+++ b/cell1/docker-ps.txt", "@@ -1 +1 @@"]);
        for l in &lines[4..6] {
            let v: Value = serde_json::from_str(&l[1..]).unwrap_or_else(|e| panic!("{e}: {l}"));
            assert_eq!(v["Command"], format!("\"run API_KEY={REDACTED}\""));
        }
        assert_eq!((&lines[4][..1], &lines[5][..1]), ("-", "+"));
        assert_eq!(lines[6], format!(" token={REDACTED} "), "the text between, as text");
        assert!(out.ends_with('\n'));
        // Redacting again changes nothing, unlike the text pass that ate the backslash.
        assert_eq!(redact_output(&out), out);
        // A key block across lines is caught whole, a JSON line in it or not.
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU\n{\"a\":1}\nAAAAB3NzaC1yc2EAAAADAQABAAAB\n-----END OPENSSH PRIVATE KEY-----";
        let out = redact_output(key);
        assert!(!out.contains("b3BlbnNzaC1rZXktdjEAAAAABG5vbmU") && !out.contains("AAAAB3NzaC1yc2EAAAADAQABAAAB"), "{out}");
        assert_eq!(redact_output("Filesystem 1B-blocks\n/dev/nvme0n1p2 1 2 3"), "Filesystem 1B-blocks\n/dev/nvme0n1p2 1 2 3");
        assert_eq!(redact_output(""), "");
    }

    #[tokio::test]
    async fn a_failure_after_the_node_reads_still_files_the_capture() {
        let logs = crate::fake_node::temp("evidence-problems");
        // The SSH log can't be read (a folder where the file should be) and the export fails.
        std::fs::create_dir_all(ssh_log(&logs)).unwrap();
        let req = Request { logs: &logs, phase: "p1", label: "before", nodes: vec!["cell1".into()], not_connected: vec![], at: at(11, 0, 0) };
        let made = capture(req, &runner(2_900_000_000_000), |_, _| Err("export failed".into())).await.unwrap();
        let dir = PathBuf::from(&made.path);
        assert!(dir.join("manifest.json").is_file());
        assert!(!dir.join("activity.jsonl").exists());
        assert_eq!(made.problems.len(), 2, "{:?}", made.problems);
        assert_eq!(made.problems[0], "No activity log: export failed");
        assert!(made.problems[1].starts_with("Couldn't read ") && made.problems[1].contains(ssh_log(&logs).to_str().unwrap()), "{:?}", made.problems);
        let m = manifest_of(&made.path);
        assert_eq!((m.problems.clone(), m.nodes[0].steps.len()), (made.problems.clone(), STEPS.len()));
        assert!(m.finished_ms >= m.started_ms && m.finished_ms > 0);
        // An export that isn't JSON is a problem too, not an error.
        let req = Request { logs: &logs, phase: "p1", label: "after", nodes: vec!["cell1".into()], not_connected: vec![], at: at(11, 5, 0) };
        let after = capture(req, &runner(2_800_000_000_000), |_, _| Ok("not json".into())).await.unwrap();
        assert!(after.problems[0].starts_with("No activity log: the export isn't a JSON array"), "{:?}", after.problems);
        // Listed, and the capture after it is compared with it.
        assert_eq!(list(&logs).iter().map(|f| f.path.clone()).collect::<Vec<_>>(), [after.path.clone(), made.path.clone()]);
        assert_eq!(manifest_of(&after.path).previous.as_deref(), Some("2026-10-29-p1/110000-before"));
        assert!(after.diff.is_some());
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[tokio::test]
    async fn a_capture_cut_off_is_listed_as_not_finished_and_never_compared_with() {
        let logs = crate::fake_node::temp("evidence-cut");
        let req = Request { logs: &logs, phase: "p1", label: "done", nodes: vec!["cell1".into()], not_connected: vec![], at: at(12, 0, 0) };
        let done = capture(req, &runner(2_900_000_000_000), no_activity).await.unwrap();
        // A node that never answers, and the capture dropped part way (as when SparkBox quits).
        let hang: Runner = Box::new(|_, _| Box::pin(std::future::pending()));
        let req = Request { logs: &logs, phase: "p1", label: "cut", nodes: vec!["cell1".into(), "cell2".into()], not_connected: vec![], at: at(12, 1, 0) };
        assert!(tokio::time::timeout(Duration::from_millis(200), capture(req, &hang, no_activity)).await.is_err());
        let listed = list(&logs);
        assert_eq!(listed.len(), 2);
        assert!(listed[0].path.ends_with("120100-cut"), "{}", listed[0].path);
        assert_eq!((listed[0].nodes, listed[0].problems.clone()), (2, vec![UNFINISHED.to_string()]));
        // The next capture is compared with the last one that finished.
        let req = Request { logs: &logs, phase: "p1", label: "again", nodes: vec!["cell1".into()], not_connected: vec![], at: at(12, 2, 0) };
        let again = capture(req, &runner(2_900_000_000_000), no_activity).await.unwrap();
        let m = manifest_of(&again.path);
        assert_eq!(m.previous.as_deref(), Some("2026-10-29-p1/120000-done"));
        assert_eq!(m.activity_from_ms, manifest_of(&done.path).started_ms);
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[tokio::test]
    async fn the_capture_before_midnight_is_the_one_compared_with() {
        let logs = crate::fake_node::temp("evidence-midnight");
        let nodes = vec!["cell1".to_string()];
        let req = Request { logs: &logs, phase: "p1", label: "before", nodes: nodes.clone(), not_connected: vec![], at: at_on(29, 23, 59, 0) };
        let before = capture(req, &runner(2_900_000_000_000), no_activity).await.unwrap();
        // Another phase in between, whose name starts the same way, isn't this phase.
        let req = Request { logs: &logs, phase: "p1-gate", label: "other", nodes: nodes.clone(), not_connected: vec![], at: at_on(29, 23, 59, 30) };
        capture(req, &runner(2_000_000_000_000), no_activity).await.unwrap();
        let req = Request { logs: &logs, phase: "p1", label: "after", nodes: nodes.clone(), not_connected: vec![], at: at_on(30, 0, 1, 0) };
        let after = capture(req, &runner(2_800_000_000_000), no_activity).await.unwrap();
        assert!(after.path.ends_with("evidence/2026-10-30-p1/000100-after"), "{}", after.path);
        let m = manifest_of(&after.path);
        assert_eq!(m.previous.as_deref(), Some("2026-10-29-p1/235900-before"));
        assert_eq!(m.activity_from_ms, manifest_of(&before.path).started_ms, "the activity since the capture before, not the hour before");
        let diff_path = after.diff.clone().expect("a diff across midnight");
        assert!(diff_path.ends_with("2026-10-30-p1/000100-after/diff-from-235900-before.txt"), "{diff_path}");
        let diff = std::fs::read_to_string(&diff_path).unwrap();
        assert!(diff.starts_with("Compared with 2026-10-29-p1/235900-before\n"), "{diff}");
        assert!(diff.contains("+/dev/nvme0n1p2 3936501538816 1000 2800000000000 26% /"), "{diff}");
        // The list finds the diff by the folder's name.
        assert_eq!(list(&logs).iter().find(|f| f.path == after.path).unwrap().diff, after.diff);
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[tokio::test]
    async fn a_real_cell2_answers_what_it_can_and_the_capture_still_finishes() {
        use crate::fake_node::{connect_sparks, fake_spark, temp};
        let (node, port) = fake_spark(&temp("evidence-cell2"), "cell2").await;
        let (live, _) = connect_sparks(&temp("evidence-cell2-app"), &[("cell2", port)]).await;
        let logs = temp("evidence-cell2-logs");
        let before = live.audit.entries().len();
        let made = capture_live(&live, &["cell2".to_string(), "cell3".to_string()], &logs, "p1", "fake cell2").await.unwrap();
        assert_eq!((made.nodes, made.not_connected.clone()), (1, vec!["cell3".to_string()]));
        let dir = PathBuf::from(&made.path);
        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        let steps = &m.nodes[0].steps;
        assert_eq!(steps.len(), STEPS.len(), "every step ran");
        let code = |s: &str| steps.iter().find(|x| x.step == s).unwrap().code;
        // What the node's own commands answer…
        assert_eq!(code("docker-ps"), Some(0));
        assert_eq!(code("systemctl-failed"), Some(0));
        assert_eq!(code("apt-upgradable"), Some(0));
        assert_eq!(code("apt-holds"), Some(0));
        assert_eq!(code("nvidia-smi"), Some(0));
        assert_eq!(code("ip-addr"), Some(0));
        assert_eq!(code("listening"), Some(0));
        assert!(std::fs::read_to_string(dir.join("cell2/nvidia-smi.txt")).unwrap().contains("580.178.04"));
        assert!(std::fs::read_to_string(dir.join("cell2/ip-addr.txt")).unwrap().contains("enP7s7"));
        // …and what the fake has no stand-in for fails on its own step, the rest carrying on.
        assert_eq!(code("docker-images"), Some(1));
        assert_ne!(code("dpkg"), Some(0));
        assert_ne!(code("mem-available"), Some(0));
        assert_ne!(code("models"), Some(0), "no ~/models in the fake home");
        assert!(made.failed_steps >= 4);
        // Each step ran as written, through the read-only wrapper (once each: `ran` matches by
        // substring, and the shell step's dpkg-query contains the dpkg step's).
        for (_, cmd) in STEPS {
            let containing = STEPS.iter().filter(|(_, c)| c.contains(cmd)).count();
            assert_eq!(node.ran(cmd).len(), containing, "{cmd}");
        }
        // The shell step files what the fake's own /bin/sh is; the fake has no getent, so the
        // step's exit code is the missing command's, as on any node without it.
        let shell = std::fs::read_to_string(dir.join("cell2/shell.txt")).unwrap();
        assert!(shell.starts_with(&format!("$ {}\nexit 127\n", STEPS.iter().find(|s| s.0 == "shell").unwrap().1)), "{shell}");
        assert!(shell.contains("--- stdout\n") && shell.contains("/bin/sh"), "{shell}");
        // The probes aren't in the activity log; one row on the Mac says a capture was made.
        let rows = live.audit.entries();
        assert_eq!(rows.len(), before + 1, "{:?}", rows.iter().map(|e| &e.title).collect::<Vec<_>>());
        let row = rows[0].clone();
        assert_eq!((row.node.as_str(), row.kind.as_str(), row.title.as_str()), ("mac", "app", "Evidence captured"));
        assert_eq!(row.command, format!("fake-cell2 · 1 node · {}", made.path));
        // A read you asked for: under Everything as yours, never under Changes (L27).
        assert_eq!((row.class.as_str(), row.changes, row.source.as_str(), row.actor.as_str()), ("read", false, "ui", "ui"));
        assert!(!rows.iter().any(|e| e.node == "cell2"), "no row for the node at all");
        // Its own operation, not one of the reads a window makes: more of those than memory
        // keeps (2000) don't push it out of the window.
        for _ in 0..2_001 {
            live.audit.record(Entry { node: "cell2".into(), kind: "command".into(), command: "docker ps".into(), ..Default::default() });
        }
        assert!(live.audit.entries().iter().any(|e| e.id == row.id), "the capture's row is still in the window");
        live.stop();
        let _ = std::fs::remove_dir_all(&logs);
    }
}
