//! Tasks: every operation that changes the cluster (a reboot, a service restart, a Fleet
//! run, an upgrade) is tracked as a task with steps, progress, a log and an outcome, like
//! Proxmox's task list. The UI shows running tasks in the status bar and keeps the last
//! 200 in tasks.jsonl (mode 0600, titles and retry arguments redacted), so what happened is
//! still there after a restart.
//!
//! A finished task carries an `Outcome` (ok, warning, failed, cancelled), the same words its
//! activity-log head line and its toast use: `on_finish` hooks end the head with it.
//!
//! Updates reach the UI coalesced: at most one `tasks://update` (without the log) per task
//! every 200 ms, and new log lines as `tasks://log {id, lines}` deltas. The file is written at
//! most once a second, off the async workers.
//!
//! Every task carries the data source it ran under (`demo`), and the list the UI reads is the
//! current source's only: a simulated "Shut down cell3" never shows while the real cluster is.
//! Tasks saved before sources were kept apart are placed once, by their activity-log rows when
//! those say, else as Demo (Live was first used on Oct 29).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub use crate::live::audit::Outcome;

pub const UPDATE_EVENT: &str = "tasks://update";
pub const LOG_EVENT: &str = "tasks://log";
const KEEP: usize = 200;
const LOG_LINES: usize = 400;
const FLUSH_EVERY: Duration = Duration::from_millis(200);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    #[default]
    Running,
    Ok,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    #[default]
    Running,
    Ok,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Step {
    pub label: String,
    pub state: StepState,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub detail: Option<String>,
}

/// How the UI can run the same thing again: a command name and its arguments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Retry {
    pub command: String,
    pub args: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Task {
    pub id: String,
    /// power | service | container | process | fleet | updates | model | transfer…
    pub kind: String,
    pub title: String,
    pub nodes: Vec<String>,
    /// Where it was started: ui | tray | palette | system.
    pub source: String,
    pub cancellable: bool,
    pub state: TaskState,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    /// 0..1 when known.
    pub progress: Option<f32>,
    pub detail: Option<String>,
    pub rate_bps: Option<f64>,
    pub eta_s: Option<f64>,
    pub steps: Vec<Step>,
    pub log: Vec<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
    /// Finished, but something didn't come back as it should ("Restarted, but llama-3.3-70b
    /// didn't come back: …").
    pub warning: Option<String>,
    pub cancel_requested: bool,
    pub retry: Option<Retry>,
    /// Ties the task to its activity-log entries.
    pub op_id: Option<String>,
    /// A second line under the title (a Fleet snippet's command).
    pub subtitle: Option<String>,
    /// Only looks (a benchmark, a check): cancelling it needs no Admin mode.
    pub read_only: bool,
    /// How it ended, once it has (ok, warning, failed, cancelled).
    pub outcome: Option<Outcome>,
    /// It ran against the simulator (Demo), not the real cluster.
    pub demo: bool,
    /// Look-only came on while it ran: "Look-only turned on: stopped before its next step" (it
    /// was cancelled), "…: its next change is refused" (it can't be stopped part-way, and the
    /// write exit refuses its next change), or "…: what it started on cell2 keeps running…"
    /// (`on_node`). A cancelled task ends with these words.
    pub held: Option<String>,
    /// What it started runs on the node by itself now (an apt upgrade in its own unit, a load's
    /// containers starting): cancelling the task wouldn't stop that, and in look-only SparkBox
    /// can't, so look-only says so instead of claiming a stop (`Tasks::hold_non_read`).
    pub on_node: bool,
}

/// New log lines for one task (`tasks://log`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogDelta {
    pub id: String,
    pub lines: Vec<String>,
}

impl Task {
    pub fn finished(&self) -> bool {
        self.state != TaskState::Running
    }

    /// The outcome its state and warning add up to (None while running).
    pub fn outcome_of(&self) -> Option<Outcome> {
        match self.state {
            TaskState::Running => None,
            TaskState::Ok if self.warning.is_some() => Some(Outcome::Warning),
            TaskState::Ok => Some(Outcome::Ok),
            TaskState::Failed => Some(Outcome::Failed),
            TaskState::Cancelled => Some(Outcome::Cancelled),
        }
    }
}

pub struct TaskSpec {
    pub kind: String,
    pub title: String,
    pub nodes: Vec<String>,
    pub source: String,
    pub cancellable: bool,
    pub subtitle: Option<String>,
    pub read_only: bool,
    pub on_node: bool,
}

impl TaskSpec {
    pub fn new(kind: &str, title: impl Into<String>, nodes: &[String]) -> TaskSpec {
        TaskSpec { kind: kind.into(), title: title.into(), nodes: nodes.to_vec(), source: "ui".into(), cancellable: false, subtitle: None, read_only: false, on_node: false }
    }

    pub fn cancellable(mut self) -> TaskSpec {
        self.cancellable = true;
        self
    }

    /// Where it was started, when not the window: "tray", "palette", "system".
    pub fn source(mut self, source: &str) -> TaskSpec {
        self.source = source.into();
        self
    }

    /// A second line under the title.
    pub fn subtitle(mut self, text: impl Into<String>) -> TaskSpec {
        self.subtitle = Some(text.into());
        self
    }

    /// Only looks (Fleet read-only runs and checks, benchmarks, the doctor): anyone can stop
    /// it, Admin mode or not.
    pub fn read_only(mut self) -> TaskSpec {
        self.read_only = true;
        self
    }

    /// Follows work that already runs on the node by itself (an upgrade picked up again): see
    /// `TaskHandle::on_node`.
    pub fn on_node(mut self) -> TaskSpec {
        self.on_node = true;
        self
    }
}

type EmitFn = Box<dyn Fn(&Task) + Send + Sync>;
type SourceFn = Box<dyn Fn() -> bool + Send + Sync>;
type LogFn = Box<dyn Fn(&LogDelta) + Send + Sync>;
type FinishFn = std::sync::Arc<dyn Fn(&Task) + Send + Sync>;

/// What the next flush sends: tasks that changed (in order) and their new log lines.
#[derive(Default)]
struct Pending {
    changed: Vec<String>,
    lines: Vec<(String, Vec<String>)>,
}

pub struct Tasks {
    list: Mutex<VecDeque<Task>>,
    cancels: Mutex<HashMap<String, watch::Sender<bool>>>,
    path: Option<PathBuf>,
    emit: Mutex<Option<EmitFn>>,
    emit_log: Mutex<Option<LogFn>>,
    pending: Mutex<Pending>,
    /// The list changed since it was last written.
    dirty: AtomicBool,
    /// Progress updates are sent at most this often; steps and the outcome always are.
    last_progress: Mutex<HashMap<String, Instant>>,
    /// Run once with each task as it finishes (its activity-log head line takes the outcome).
    finish_hooks: Mutex<Vec<FinishFn>>,
    /// Whether the app shows Demo right now (set once the app runs; Demo until then).
    source: Mutex<Option<SourceFn>>,
    /// Tasks read from a file written before tasks carried their source, still to be placed.
    legacy: Mutex<HashSet<String>>,
}

impl Tasks {
    /// Loads earlier tasks from `path`. Any that were still running when SparkBox last
    /// quit are marked as interrupted.
    pub fn new(path: Option<PathBuf>) -> Arc<Tasks> {
        let mut list = VecDeque::new();
        let mut legacy = HashSet::new();
        if let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                let unplaced = v.get("demo").is_none();
                if let Ok(mut t) = serde_json::from_value::<Task>(v) {
                    if unplaced {
                        // Demo unless its activity-log rows say otherwise (classify_legacy).
                        t.demo = true;
                        legacy.insert(t.id.clone());
                    }
                    if t.state == TaskState::Running {
                        t.state = TaskState::Failed;
                        // Work on a node (a download, a copy, an upgrade) runs on without SparkBox;
                        // what SparkBox picks up again shows as its own "(picked up again)" task.
                        t.error = Some(if t.on_node {
                            format!("SparkBox quit while following this; it may still be running on {}.", t.nodes.join(", "))
                        } else {
                            "SparkBox quit while this was running.".into()
                        });
                        t.finished_ms = Some(t.started_ms);
                        for s in t.steps.iter_mut().filter(|s| s.state == StepState::Running) {
                            s.state = StepState::Failed;
                        }
                    }
                    t.outcome = t.outcome_of();
                    list.push_back(t);
                }
            }
        }
        while list.len() > KEEP {
            list.pop_front();
        }
        Arc::new(Tasks {
            list: Mutex::new(list),
            cancels: Mutex::new(HashMap::new()),
            path,
            emit: Mutex::new(None),
            emit_log: Mutex::new(None),
            pending: Mutex::new(Pending::default()),
            dirty: AtomicBool::new(false),
            last_progress: Mutex::new(HashMap::new()),
            finish_hooks: Mutex::new(Vec::new()),
            source: Mutex::new(None),
            legacy: Mutex::new(legacy),
        })
    }

    /// Where new tasks learn which source they run under: true while the app shows Demo.
    pub fn set_source(&self, demo_now: impl Fn() -> bool + Send + Sync + 'static) {
        *self.source.lock().unwrap() = Some(Box::new(demo_now));
    }

    fn demo_now(&self) -> bool {
        self.source.lock().unwrap().as_ref().is_none_or(|f| f())
    }

    /// Places tasks saved before they carried their source: `ran_live(op_id)` says Some(true)
    /// when the activity log shows that operation reaching a real node, Some(false) when it was
    /// simulated, None when it can't tell (the task stays Demo). Once is enough.
    pub fn classify_legacy(&self, ran_live: impl Fn(&str) -> Option<bool>) {
        let ids: Vec<String> = self.legacy.lock().unwrap().drain().collect();
        if ids.is_empty() {
            return;
        }
        let mut list = self.list.lock().unwrap();
        for t in list.iter_mut().filter(|t| ids.contains(&t.id)) {
            t.demo = !t.op_id.as_deref().and_then(&ran_live).unwrap_or(false);
        }
        drop(list);
        self.save();
    }

    /// Called with each task once it has finished, with its outcome set.
    pub fn on_finish(&self, f: impl Fn(&Task) + Send + Sync + 'static) {
        self.finish_hooks.lock().unwrap().push(std::sync::Arc::new(f));
    }

    /// Where updates go (the `tasks://update` event, the task without its log); set once the
    /// app is running.
    pub fn on_update(&self, f: impl Fn(&Task) + Send + Sync + 'static) {
        *self.emit.lock().unwrap() = Some(Box::new(f));
    }

    /// Where new log lines go (the `tasks://log` event).
    pub fn on_log(&self, f: impl Fn(&LogDelta) + Send + Sync + 'static) {
        *self.emit_log.lock().unwrap() = Some(Box::new(f));
    }

    /// Sends what changed since the last flush: one update per task, then the log lines.
    pub fn flush(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        if !pending.changed.is_empty() {
            let tasks: Vec<Task> = pending.changed.iter().filter_map(|id| self.get(id)).collect();
            if let Some(emit) = self.emit.lock().unwrap().as_ref() {
                for mut t in tasks {
                    t.log.clear();
                    emit(&t);
                }
            }
        }
        if let Some(emit) = self.emit_log.lock().unwrap().as_ref() {
            for (id, lines) in pending.lines {
                emit(&LogDelta { id, lines });
            }
        }
    }

    /// Flushes every 200 ms and writes the file at most once a second (off the async workers).
    pub fn start_background(self: &Arc<Self>) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut every = tokio::time::interval(FLUSH_EVERY);
            let mut ticks = 0u32;
            loop {
                every.tick().await;
                me.flush();
                ticks += 1;
                if ticks % 5 == 0 && me.dirty.load(Ordering::SeqCst) {
                    let store = me.clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || store.save_now()).await;
                }
            }
        });
    }

    pub fn start(self: &Arc<Self>, spec: TaskSpec) -> TaskHandle {
        let mut id = [0u8; 6];
        let _ = getrandom::fill(&mut id);
        let id = format!("t{}", id.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let task = Task {
            id: id.clone(),
            kind: spec.kind,
            title: crate::live::audit::redact(&spec.title),
            subtitle: spec.subtitle.map(|s| crate::live::audit::redact(&s)),
            nodes: spec.nodes,
            source: spec.source,
            cancellable: spec.cancellable,
            read_only: spec.read_only,
            on_node: spec.on_node,
            started_ms: now_ms(),
            // Operations log their commands under the task's id.
            op_id: Some(id.clone()),
            demo: self.demo_now(),
            ..Default::default()
        };
        let (tx, rx) = watch::channel(false);
        self.cancels.lock().unwrap().insert(id.clone(), tx);
        {
            let mut list = self.list.lock().unwrap();
            list.push_back(task.clone());
            while list.len() > KEEP {
                list.pop_front();
            }
        }
        self.send(&task);
        self.save();
        TaskHandle { id, tasks: self.clone(), cancel: rx, life: Arc::new(()) }
    }

    /// Newest first.
    pub fn list(&self) -> Vec<Task> {
        self.list.lock().unwrap().iter().rev().cloned().collect()
    }

    /// One data source's tasks, newest first.
    pub fn list_for(&self, demo: bool) -> Vec<Task> {
        self.list.lock().unwrap().iter().rev().filter(|t| t.demo == demo).cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.list.lock().unwrap().iter().find(|t| t.id == id).cloned()
    }

    /// The nodes with Live work running that isn't a read: a task that changes them, or that
    /// follows what it started there (`on_node`). The release build keeps its controller lease on
    /// these whatever Admin mode's state (`live::exit`).
    pub fn working_nodes(&self) -> Vec<String> {
        let list = self.list.lock().unwrap();
        let mut nodes: Vec<String> = list.iter().filter(|t| !t.finished() && !t.read_only && !t.demo).flat_map(|t| t.nodes.iter().cloned()).collect();
        nodes.sort();
        nodes.dedup();
        nodes
    }

    /// Look-only came on (`AdminGate::on_hold`, with `why` = "Look-only turned on"): every running
    /// task that isn't read-only is held, and says how. One that can be cancelled is, and stops
    /// before its next step. One that can't be stopped part-way is marked "its next change is
    /// refused" (the write exit refuses it in look-only; reads after it still run). One whose work
    /// runs on the node by itself (`on_node`) isn't cancelled, since stopping that work is a change
    /// look-only refuses: it says what it started keeps running there. Returns how many of each.
    pub fn hold_non_read(&self, why: &str) -> crate::admin::HeldTasks {
        let running: Vec<(String, bool, bool, Vec<String>)> =
            self.list.lock().unwrap().iter().filter(|t| !t.finished() && !t.read_only).map(|t| (t.id.clone(), t.cancellable, t.on_node, t.nodes.clone())).collect();
        let mut held = crate::admin::HeldTasks::default();
        for (id, cancellable, on_node, nodes) in running {
            let cancel = cancellable && !on_node;
            let words = if on_node {
                let place = if nodes.is_empty() { "its node".to_string() } else { nodes.join(", ") };
                format!("{why}: what it started on {place} keeps running; SparkBox can't stop it in look-only")
            } else if cancel {
                format!("{why}: stopped before its next step")
            } else {
                format!("{why}: its next change is refused")
            };
            self.change(&id, |t| {
                t.held = Some(words.clone());
                t.detail = Some(words.clone());
                t.cancel_requested |= cancel;
                t.log.push(words.clone());
                let extra = t.log.len().saturating_sub(LOG_LINES);
                t.log.drain(..extra);
            });
            self.queue_lines(&id, vec![words]);
            if cancel {
                if let Some(tx) = self.cancels.lock().unwrap().get(&id) {
                    let _ = tx.send(true);
                }
                held.stopped += 1;
            } else if on_node {
                held.on_node += 1;
            } else {
                held.stopping += 1;
            }
        }
        held
    }

    /// Asks a running task to stop. It decides when it's safe to (a step in flight finishes).
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let task = self.update(id, |t| {
            if !t.finished() {
                t.cancel_requested = true;
            }
        });
        let task = task.ok_or("That task is no longer listed.")?;
        if task.finished() {
            return Ok(());
        }
        if !task.cancellable {
            return Err(format!("“{}” can't be stopped part-way.", task.title));
        }
        if let Some(tx) = self.cancels.lock().unwrap().get(id) {
            let _ = tx.send(true);
        }
        Ok(())
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Task)) -> Option<Task> {
        let task = {
            let mut list = self.list.lock().unwrap();
            let t = list.iter_mut().find(|t| t.id == id)?;
            f(t);
            t.clone()
        };
        Some(task)
    }

    fn change(&self, id: &str, f: impl FnOnce(&mut Task)) {
        if let Some(task) = self.update(id, f) {
            self.send(&task);
            self.save();
        }
    }

    /// Queues an update for the next flush.
    fn send(&self, task: &Task) {
        let mut p = self.pending.lock().unwrap();
        if !p.changed.contains(&task.id) {
            p.changed.push(task.id.clone());
        }
    }

    fn queue_lines(&self, id: &str, lines: Vec<String>) {
        let mut p = self.pending.lock().unwrap();
        match p.lines.iter_mut().find(|(i, _)| i == id) {
            Some((_, l)) => l.extend(lines),
            None => p.lines.push((id.to_string(), lines)),
        }
    }

    /// Marks the list for the next write (at most once a second).
    fn save(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// Writes tasks.jsonl now (0600), retry arguments redacted. Called by the background
    /// writer and when SparkBox quits.
    pub fn save_now(&self) {
        let Some(path) = &self.path else { return };
        self.dirty.store(false, Ordering::SeqCst);
        let text: String = {
            let list = self.list.lock().unwrap();
            list.iter()
                .map(|t| {
                    let mut t = t.clone();
                    if let Some(r) = t.retry.as_mut() {
                        redact_json(&mut r.args);
                    }
                    t
                })
                .filter_map(|t| serde_json::to_string(&t).ok())
                .map(|l| l + "\n")
                .collect()
        };
        if let Err(e) = write_private(path, text.as_bytes()) {
            eprintln!("tasks: {e}");
        }
    }
}

/// Redacts every string in a JSON value (retry arguments: a Fleet command with a token in it).
pub fn redact_json(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => *s = crate::live::audit::redact(s),
        serde_json::Value::Array(a) => a.iter_mut().for_each(redact_json),
        serde_json::Value::Object(o) => o.values_mut().for_each(redact_json),
        _ => {}
    }
}

/// Like `util::write_atomic`, but the file is only ever readable by you (0600 from creation).
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".tasks.jsonl.tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't save {}: {e}", path.display())
    })
}

/// The running side of a task. Cheap to clone; every method updates the UI.
#[derive(Clone)]
pub struct TaskHandle {
    id: String,
    tasks: Arc<Tasks>,
    cancel: watch::Receiver<bool>,
    /// Shared by every clone, so the last one to go can tell.
    life: Arc<()>,
}

impl TaskHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Starts a new step; the previous one (if still running) is marked done.
    pub fn step(&self, label: impl Into<String>) {
        let label = label.into();
        let now = now_ms();
        self.tasks.change(&self.id, |t| {
            close_running(t, StepState::Ok, now);
            t.steps.push(Step { label: label.clone(), state: StepState::Running, started_ms: now, ..Default::default() });
            t.detail = Some(label);
        });
    }

    /// Marks the current step done (with an optional note) without starting another.
    pub fn step_ok(&self, detail: Option<String>) {
        let now = now_ms();
        self.tasks.change(&self.id, |t| {
            if let Some(s) = t.steps.iter_mut().rev().find(|s| s.state == StepState::Running) {
                s.state = StepState::Ok;
                s.finished_ms = Some(now);
                if detail.is_some() {
                    s.detail = detail;
                }
            }
        });
    }

    /// Marks the current step failed; the task keeps going unless the caller stops it.
    pub fn step_failed(&self, detail: impl Into<String>) {
        let detail = detail.into();
        let now = now_ms();
        self.tasks.change(&self.id, |t| {
            if let Some(s) = t.steps.iter_mut().rev().find(|s| s.state == StepState::Running) {
                s.state = StepState::Failed;
                s.finished_ms = Some(now);
                s.detail = Some(detail);
            }
        });
    }

    /// Adds a step that didn't run (e.g. after an earlier node failed).
    pub fn step_skipped(&self, label: impl Into<String>, why: impl Into<String>) {
        let (label, why) = (label.into(), why.into());
        let now = now_ms();
        self.tasks.change(&self.id, |t| {
            t.steps.push(Step { label, state: StepState::Skipped, started_ms: now, finished_ms: Some(now), detail: Some(why) });
        });
    }

    pub fn progress(&self, frac: f32, detail: Option<String>) {
        self.progress_rate(frac, detail, None, None)
    }

    /// Progress with a transfer rate and time left, for copies and downloads.
    pub fn progress_rate(&self, frac: f32, detail: Option<String>, rate_bps: Option<f64>, eta_s: Option<f64>) {
        let quiet = {
            let mut last = self.tasks.last_progress.lock().unwrap();
            let now = Instant::now();
            let quiet = last.get(&self.id).is_some_and(|t| now.duration_since(*t) < Duration::from_millis(200)) && frac < 1.0;
            if !quiet {
                last.insert(self.id.clone(), now);
            }
            quiet
        };
        let apply = |t: &mut Task| {
            t.progress = Some(frac.clamp(0.0, 1.0));
            if detail.is_some() {
                t.detail = detail.clone();
            }
            t.rate_bps = rate_bps;
            t.eta_s = eta_s;
        };
        if quiet {
            self.tasks.update(&self.id, apply);
        } else {
            self.tasks.change(&self.id, apply);
        }
    }

    /// Adds lines to the task's log; the UI gets them as a delta, not the whole task.
    pub fn log(&self, line: impl Into<String>) {
        let line = crate::live::audit::redact(&line.into());
        let lines: Vec<String> = line.lines().map(String::from).collect();
        self.tasks.update(&self.id, |t| {
            t.log.extend(lines.iter().cloned());
            let extra = t.log.len().saturating_sub(LOG_LINES);
            t.log.drain(..extra);
        });
        self.tasks.queue_lines(&self.id, lines);
        self.tasks.save();
    }

    /// From now on what it started runs on the node by itself (the upgrade's unit is running, the
    /// load's head container is starting): turning look-only on no longer cancels it, and says
    /// that work keeps running (`Tasks::hold_non_read`).
    pub fn on_node(&self) {
        self.tasks.update(&self.id, |t| t.on_node = true);
        self.tasks.save();
    }

    /// How the UI can start the same thing again. The UI never replays it as is: it reopens the
    /// original confirmation with these arguments (src/lib/tasks.ts `retryTask`).
    pub fn set_retry(&self, command: &str, args: serde_json::Value) {
        self.tasks.update(&self.id, |t| t.retry = Some(Retry { command: command.into(), args }));
        self.tasks.save();
    }

    pub fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    /// Resolves when someone presses Cancel.
    pub fn cancelled(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = self.cancel.clone();
        async move {
            let _ = rx.wait_for(|c| *c).await;
            if !*rx.borrow() {
                // The sender is gone without a cancel: never resolve.
                std::future::pending::<()>().await;
            }
        }
    }

    pub fn finish_ok(&self, summary: impl Into<String>) {
        self.finish(TaskState::Ok, Some(summary.into()), None);
    }

    pub fn finish_err(&self, error: impl Into<String>) {
        let state = if self.is_cancelled() { TaskState::Cancelled } else { TaskState::Failed };
        self.finish(state, None, Some(error.into()));
    }

    pub fn finish_cancelled(&self, summary: impl Into<String>) {
        self.finish(TaskState::Cancelled, Some(summary.into()), None);
    }

    // ── R2-OPS ──
    /// Failed, even when Cancel was pressed: what Cancel asked for didn't happen (a scheduled
    /// restart the node wouldn't call off still went ahead), so it isn't "cancelled".
    pub fn finish_failed(&self, error: impl Into<String>) {
        self.finish(TaskState::Failed, None, Some(error.into()));
    }

    /// Cancel was pressed and didn't happen, and the task goes on (the node couldn't stop what it
    /// started): it isn't being cancelled any more, and Cancel can be pressed again.
    pub fn cancel_failed(&self) {
        self.tasks.change(&self.id, |t| t.cancel_requested = false);
        if let Some(tx) = self.tasks.cancels.lock().unwrap().get(&self.id) {
            let _ = tx.send(false);
        }
    }

    /// Done, but its outcome check found a problem: the task is Ok with a warning.
    pub fn finish_warn(&self, summary: impl Into<String>, warning: impl Into<String>) {
        let warning = crate::live::audit::redact(&warning.into());
        self.tasks.update(&self.id, |t| t.warning = Some(warning));
        self.finish(TaskState::Ok, Some(summary.into()), None);
    }

    /// Sets the line under the progress bar without moving it (a Verify step's "Loading 42%").
    pub fn detail(&self, text: impl Into<String>) {
        let text = text.into();
        self.tasks.change(&self.id, |t| t.detail = Some(text));
    }

    fn finish(&self, state: TaskState, summary: Option<String>, error: Option<String>) {
        let now = now_ms();
        let mut already = false;
        self.tasks.change(&self.id, |t| {
            if t.finished() {
                already = true;
                return;
            }
            let step = match state {
                TaskState::Ok => StepState::Ok,
                TaskState::Cancelled => StepState::Skipped,
                _ => StepState::Failed,
            };
            close_running(t, step, now);
            t.state = state;
            t.finished_ms = Some(now);
            if state == TaskState::Ok {
                t.progress = Some(1.0);
            }
            t.rate_bps = None;
            t.eta_s = None;
            t.summary = summary.map(|s| crate::live::audit::redact(&s));
            t.error = error.map(|e| crate::live::audit::redact(&e));
            // Stopped because look-only came on: it ends with that, and its own words go to the log.
            if let Some(held) = t.held.clone().filter(|_| state == TaskState::Cancelled) {
                let own = if t.error.is_some() { &mut t.error } else { &mut t.summary };
                if let Some(words) = own.replace(held.clone()).filter(|w| *w != held) {
                    t.log.push(words);
                }
            }
            t.outcome = t.outcome_of();
        });
        if !already {
            self.tasks.cancels.lock().unwrap().remove(&self.id);
            self.tasks.last_progress.lock().unwrap().remove(&self.id);
            self.tasks.save();
            if let Some(task) = self.tasks.get(&self.id) {
                let hooks: Vec<FinishFn> = self.tasks.finish_hooks.lock().unwrap().clone();
                for hook in hooks {
                    hook(&task);
                }
            }
        }
    }
}

impl Drop for TaskHandle {
    /// A handle dropped without an outcome (an early return, a panic) still ends the task.
    fn drop(&mut self) {
        if Arc::strong_count(&self.life) == 1 && self.tasks.get(&self.id).is_some_and(|t| !t.finished()) {
            self.finish(TaskState::Failed, None, Some("Stopped unexpectedly.".into()));
        }
    }
}

fn close_running(t: &mut Task, state: StepState, now: u64) {
    for s in t.steps.iter_mut().filter(|s| s.state == StepState::Running) {
        s.state = state;
        s.finished_ms = Some(now);
    }
}

// ---- Commands

/// The tasks of the data source the app shows (Demo's never appear with Live data).
#[tauri::command]
pub fn tasks_list(app: tauri::AppHandle, tasks: tauri::State<'_, Arc<Tasks>>) -> Vec<Task> {
    use tauri::Manager;
    let demo = app.try_state::<crate::AppState>().map(|s| !crate::is_live(&s)).unwrap_or(true);
    tasks.list_for(demo)
}

#[tauri::command]
pub fn task_get(tasks: tauri::State<'_, Arc<Tasks>>, id: String) -> Option<Task> {
    tasks.get(&id)
}

/// Stops a task. Cancelling needs Admin mode like starting it did (node scope); read-only tasks
/// (Fleet runs that only read, checks, benchmarks) stop without it.
#[tauri::command]
pub fn task_cancel(app: tauri::AppHandle, tasks: tauri::State<'_, Arc<Tasks>>, id: String) -> Result<(), crate::errors::SbError> {
    if !tasks.get(&id).is_some_and(|t| t.read_only) {
        crate::admin::require_sb(&app)?;
    }
    tasks.cancel(&id).map_err(crate::errors::SbError::invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sparkbox-tasks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tasks.jsonl")
    }

    #[tokio::test]
    async fn a_cancel_that_failed_can_be_pressed_again() {
        let tasks = Tasks::new(None);
        let t = tasks.start(TaskSpec::new("updates", "Upgrade cell2", &["cell2".to_string()]).cancellable());
        tasks.cancel(t.id()).unwrap();
        t.cancelled().await;
        assert!(t.is_cancelled() && tasks.get(t.id()).unwrap().cancel_requested);

        t.cancel_failed();
        assert!(!t.is_cancelled() && !tasks.get(t.id()).unwrap().cancel_requested);
        let waiting = t.cancelled();
        tokio::pin!(waiting);
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut waiting).await.is_err(), "no press left over");

        tasks.cancel(t.id()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), waiting).await.expect("the second press is seen");
        assert!(tasks.get(t.id()).unwrap().cancel_requested);
        t.finish_failed("apt-get failed on cell2");
        assert_eq!(tasks.get(t.id()).unwrap().state, TaskState::Failed, "Failed, not Cancelled");
        t.cancel_failed();
    }

    #[tokio::test]
    async fn a_task_runs_steps_and_is_kept_after_a_restart() {
        let path = temp_path("persist");
        let tasks = Tasks::new(Some(path.clone()));
        let seen = Arc::new(Mutex::new(Vec::<Task>::new()));
        let s = seen.clone();
        tasks.on_update(move |t| s.lock().unwrap().push(t.clone()));

        let t = tasks.start(TaskSpec::new("power", "Reboot cell3", &["cell3".into()]));
        t.set_retry("fleet_start", serde_json::json!({ "command": "curl -H 'Authorization: Bearer abc123' http://x", "nodes": ["cell3"] }));
        t.step("Sending reboot");
        t.step("Waiting for cell3 to go offline");
        t.log("password=hunter2 should never show");
        t.step_failed("still answering");
        t.finish_err("The reboot didn't happen.");
        tasks.flush();

        let done = tasks.get(t.id()).unwrap();
        assert_eq!(done.state, TaskState::Failed);
        assert_eq!(done.steps.iter().map(|s| s.state).collect::<Vec<_>>(), [StepState::Ok, StepState::Failed]);
        assert_eq!(done.log, ["password=‹redacted› should never show"]);
        assert_eq!(seen.lock().unwrap().last().unwrap().state, TaskState::Failed, "the outcome reaches the UI");

        let running = tasks.start(TaskSpec::new("fleet", "uptime on 4 nodes", &[]));
        running.step("cell1");
        std::mem::forget(running); // still running when the app "quits"
        let download = tasks.start(TaskSpec::new("download", "Download glm-4.5-air", &["cell1".into()]).on_node());
        std::mem::forget(download); // runs on cell1 by itself
        tasks.save_now();
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains("abc123") && !text.contains("hunter2"), "retry arguments are redacted on disk");
        }

        let again = Tasks::new(Some(path));
        let list = again.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].title, "Download glm-4.5-air");
        assert_eq!((list[0].state, list[0].error.as_deref()), (TaskState::Failed, Some("SparkBox quit while following this; it may still be running on cell1.")));
        assert_eq!(list[1].title, "uptime on 4 nodes");
        assert_eq!((list[1].state, list[1].error.as_deref()), (TaskState::Failed, Some("SparkBox quit while this was running.")), "interrupted by the quit");
        assert_eq!(list[2].error.as_deref(), Some("The reboot didn't happen."));
    }

    #[tokio::test]
    async fn cancel_reaches_the_task_only_when_it_can_stop() {
        let tasks = Tasks::new(None);
        let fixed = tasks.start(TaskSpec::new("updates", "Upgrade cell1", &[]));
        assert!(tasks.cancel(fixed.id()).unwrap_err().contains("can't be stopped"));
        assert!(!fixed.is_cancelled());

        let t = tasks.start(TaskSpec::new("fleet", "sleep 30", &[]).cancellable());
        let waiter = tokio::spawn(t.cancelled());
        tasks.cancel(t.id()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap();
        assert!(t.is_cancelled());
        t.finish_err("Canceled");
        assert_eq!(tasks.get(t.id()).unwrap().state, TaskState::Cancelled);
        fixed.finish_ok("done");
    }

    #[test]
    fn only_the_last_200_are_kept() {
        let tasks = Tasks::new(None);
        for i in 0..210 {
            tasks.start(TaskSpec::new("x", format!("task {i}"), &[])).finish_ok("ok");
        }
        let list = tasks.list();
        assert_eq!(list.len(), 200);
        assert_eq!(list[0].title, "task 209");
    }

    #[test]
    fn a_dropped_handle_still_ends_its_task() {
        let tasks = Tasks::new(None);
        let id = {
            let t = tasks.start(TaskSpec::new("x", "early return", &[]));
            let copy = t.clone();
            drop(copy);
            assert!(!tasks.get(t.id()).unwrap().finished(), "a clone going away doesn't end it");
            t.id().to_string()
        };
        let t = tasks.get(&id).unwrap();
        assert_eq!(t.state, TaskState::Failed);
        assert_eq!(t.error.as_deref(), Some("Stopped unexpectedly."));
    }

    #[test]
    fn updates_are_coalesced_and_logs_go_as_deltas() {
        let tasks = Tasks::new(None);
        let updates = Arc::new(Mutex::new(Vec::<Task>::new()));
        let deltas = Arc::new(Mutex::new(Vec::<LogDelta>::new()));
        let (u, d) = (updates.clone(), deltas.clone());
        tasks.on_update(move |t| u.lock().unwrap().push(t.clone()));
        tasks.on_log(move |l| d.lock().unwrap().push(l.clone()));

        let t = tasks.start(TaskSpec::new("fleet", "Fleet: uptime", &[]));
        for i in 0..500 {
            t.log(format!("line {i}"));
            t.progress(i as f32 / 500.0, None);
        }
        t.step("cell1");
        assert!(updates.lock().unwrap().is_empty(), "nothing goes out between flushes");
        tasks.flush();
        let sent = updates.lock().unwrap().clone();
        assert_eq!(sent.len(), 1, "500 log lines and progress steps became one update");
        assert!(sent[0].log.is_empty(), "updates carry no log");
        let d = deltas.lock().unwrap().clone();
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].id.as_str(), d[0].lines.len(), d[0].lines[499].as_str()), (t.id(), 500, "line 499"));

        tasks.flush();
        assert_eq!(updates.lock().unwrap().len(), 1, "nothing new, nothing sent");
        t.log("one more");
        tasks.flush();
        assert_eq!(updates.lock().unwrap().len(), 1, "a log line alone sends no update");
        assert_eq!(deltas.lock().unwrap().last().unwrap().lines, ["one more"]);
        t.finish_ok("done");
    }

    #[test]
    fn each_task_keeps_its_source_and_the_list_shows_one_source() {
        let tasks = Tasks::new(None);
        let live = Arc::new(AtomicBool::new(false));
        let l = live.clone();
        tasks.set_source(move || !l.load(Ordering::SeqCst));
        tasks.start(TaskSpec::new("power", "Shut down cell3", &["cell3".into()])).finish_ok("cell3 is off");
        live.store(true, Ordering::SeqCst);
        let t = tasks.start(TaskSpec::new("fleet", "uptime on 4 nodes", &[]).read_only());
        t.finish_ok("4 answered");
        assert_eq!(tasks.list_for(false).iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["uptime on 4 nodes"]);
        assert_eq!(tasks.list_for(true).iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Shut down cell3"]);
        assert_eq!(tasks.list().len(), 2, "both are kept");
    }

    /// What the release build keeps its lease for: what runs and isn't a read, with Live data.
    #[test]
    fn working_nodes_are_where_a_running_task_changes_something() {
        let tasks = Tasks::new(None);
        tasks.set_source(|| false);
        let change = tasks.start(TaskSpec::new("service", "Restart earlyoom on 2 nodes", &["cell2".into(), "cell1".into()]));
        let follow = tasks.start(TaskSpec::new("updates", "Upgrade cell3 (picked up again)", &["cell3".into()]).on_node());
        let _read = tasks.start(TaskSpec::new("benchmark", "Benchmark gemma", &["gx10-0000".into()]).read_only());
        assert_eq!(tasks.working_nodes(), ["cell1", "cell2", "cell3"]);
        change.finish_ok("Restarted");
        assert_eq!(tasks.working_nodes(), ["cell3"]);
        follow.finish_cancelled("Stopped");
        assert!(tasks.working_nodes().is_empty());
        tasks.set_source(|| true);
        let _demo = tasks.start(TaskSpec::new("service", "Restart earlyoom", &["cell1".into()]));
        assert!(tasks.working_nodes().is_empty(), "Demo's nodes aren't real");
    }

    #[test]
    fn tasks_saved_before_sources_are_placed_by_the_activity_log_else_demo() {
        let path = temp_path("legacy");
        // Three tasks from round 3's file, without a source.
        let row = |id: &str, title: &str| serde_json::json!({ "id": id, "kind": "fleet", "title": title, "nodes": [], "source": "ui", "state": "ok", "startedMs": 1, "opId": id }).to_string();
        std::fs::write(&path, [row("t1", "Restart llama-3.3-70b"), row("t2", "uptime on 4 nodes"), row("t3", "Wake cell3")].join("\n") + "\n").unwrap();
        let tasks = Tasks::new(Some(path.clone()));
        assert!(tasks.list().iter().all(|t| t.demo), "Demo until placed");
        // The log saw t2's command reach a real node, t1 simulated; t3 left nothing.
        tasks.classify_legacy(|op| match op {
            "t1" => Some(false),
            "t2" => Some(true),
            _ => None,
        });
        assert_eq!(tasks.list_for(false).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2"]);
        assert_eq!(tasks.list_for(true).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t3", "t1"]);
        tasks.save_now();
        let again = Tasks::new(Some(path));
        again.classify_legacy(|_| Some(false));
        assert_eq!(again.list_for(false).len(), 1, "placed once, then saved with its source");
    }

    /// Turning look-only on stops what's running: a cancellable task finishes the step in flight
    /// and stops before the next, ending with the hold's words; one that can't be stopped part-way
    /// is marked; one whose work runs on the node by itself isn't cancelled and says that work
    /// keeps running; a read-only one runs on.
    #[tokio::test]
    async fn a_task_started_before_look_only_stops_at_its_next_step_and_says_so() {
        let tasks = Tasks::new(None);
        let (gate, _, _) = crate::admin::testing::gate();
        let hook = tasks.clone();
        gate.on_hold(move |why| hook.hold_non_read(why));
        let task = tasks.start(TaskSpec::new("container", "Restart vllm on 4 nodes", &[]).cancellable());
        let fixed = tasks.start(TaskSpec::new("updates", "Refresh the package lists", &["cell1".into()]));
        let read = tasks.start(TaskSpec::new("fleet", "uptime on 4 nodes", &[]).cancellable().read_only());
        // Cancellable until its unit starts on the node; from then on apt runs there by itself.
        let upgrade = tasks.start(TaskSpec::new("updates", "Upgrade cell2", &["cell2".into()]).cancellable());
        upgrade.on_node();
        let picked_up = tasks.start(TaskSpec::new("updates", "Upgrade cell3 (picked up again)", &["cell3".into()]).cancellable().on_node());
        let (go, mut step_done) = tokio::sync::mpsc::channel::<()>(1);
        let ran = Arc::new(Mutex::new(Vec::<String>::new()));
        let runner = {
            let (task, ran) = (task.clone(), ran.clone());
            tokio::spawn(async move {
                // One node at a time, checking for Cancel between nodes, as ops/containers.rs does.
                for node in ["cell1", "cell2", "cell3", "gx10-0000"] {
                    if task.is_cancelled() {
                        return task.finish_err("Canceled");
                    }
                    task.step(format!("Restart on {node}"));
                    ran.lock().unwrap().push(node.to_string());
                    step_done.recv().await;
                }
                task.finish_ok("Restarted on 4 nodes");
            })
        };
        while ran.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let v = gate.set_look_only(true).unwrap();
        assert_eq!(v.held, crate::admin::HeldTasks { stopped: 1, stopping: 1, on_node: 2 });
        go.send(()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), runner).await.unwrap().unwrap();

        let t = tasks.get(task.id()).unwrap();
        assert_eq!(t.state, TaskState::Cancelled);
        assert_eq!(t.error.as_deref(), Some("Look-only turned on: stopped before its next step"));
        assert_eq!(*ran.lock().unwrap(), ["cell1"], "the step in flight finished; nothing ran after it");
        assert!(t.log.iter().any(|l| l == "Canceled"), "its own words are kept in the log: {:?}", t.log);
        let f = tasks.get(fixed.id()).unwrap();
        assert!(!f.finished() && !fixed.is_cancelled(), "it can't be stopped part-way");
        assert_eq!(f.held.as_deref(), Some("Look-only turned on: its next change is refused"));
        assert_eq!(f.detail, f.held);
        let r = tasks.get(read.id()).unwrap();
        assert!(r.held.is_none() && !read.is_cancelled(), "a read-only task runs on");
        for (h, node) in [(&upgrade, "cell2"), (&picked_up, "cell3")] {
            let u = tasks.get(h.id()).unwrap();
            assert!(!u.finished() && !h.is_cancelled() && !u.cancel_requested, "not cancelled: stopping apt is a change look-only refuses");
            assert_eq!(u.held, Some(format!("Look-only turned on: what it started on {node} keeps running; SparkBox can't stop it in look-only")));
            assert!(u.on_node);
        }
        upgrade.finish_ok("3 packages installed on cell2");
        assert_eq!(tasks.get(upgrade.id()).unwrap().summary.as_deref(), Some("3 packages installed on cell2"), "it ends with its own words, not a stop");
        fixed.finish_ok("done");
        read.finish_ok("4 answered");
        picked_up.finish_ok("done");
    }

    #[test]
    fn titles_are_redacted_and_read_only_tasks_are_marked() {
        let tasks = Tasks::new(None);
        let t = tasks.start(TaskSpec::new("fleet", "Fleet: HF_TOKEN=hf_abcdefghijklmnop1234 hf download x", &[]).subtitle("export API_KEY=abc").read_only());
        let got = tasks.get(t.id()).unwrap();
        assert_eq!(got.title, "Fleet: HF_TOKEN=‹redacted› hf download x");
        assert_eq!(got.subtitle.as_deref(), Some("export API_KEY=‹redacted›"));
        assert!(got.read_only);
        t.finish_ok("done");
    }
}
