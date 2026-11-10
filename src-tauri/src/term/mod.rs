//! Interactive terminals: a PTY on a node over SSH (a login shell, a shell inside a
//! container, or a follow-the-log command), streamed to xterm.js in the UI. In Demo
//! mode a simulated shell answers instead, backed by the demo filesystem and cluster.
//!
//! Output is batched (one event per 16 ms or 64 KB) and paced to xterm: the UI acknowledges
//! what xterm has drawn (`terminal_ack`), and once 256 KB is waiting unacknowledged SparkBox
//! stops sending. Reading from the node never stops: an SSH channel that isn't read holds up
//! every other channel on its connection. What the window can't take yet (a paused log tab,
//! a flood faster than xterm draws) waits here in a 2 MiB ring per tab that drops its oldest
//! bytes, and the tab is told how much it skipped ("… 3.2 MB skipped while paused …").
//! Keystrokes go to the node from a task of their own, so a node that stops reading input
//! can't stop the output either.
//!
//! A shell is authorised once, when it opens in Admin mode, and then takes keystrokes until
//! the owner locks (the top bar, the screen, sleep, quitting) or nobody types in it for the
//! idle time set in Settings › Terminal (60 minutes by default). Admin mode idling out
//! doesn't end it. A keystroke only stores the time in an atomic; the once-a-second `tick`
//! warns a minute before the idle lock and then locks.

mod demo;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use base64::Engine;
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

use crate::admin::{Clock, RelockReason, SystemClock};
use crate::demo::DemoCluster;
use crate::files::demo::DemoFs;
use crate::live::{sh_quote, Live};

/// One event carries at most this much output...
pub const BATCH_MAX: usize = 64 * 1024;
/// ...and waits at most this long for more.
pub const BATCH_WAIT: Duration = Duration::from_millis(16);
/// Unacknowledged bytes in the UI before SparkBox stops sending.
pub const HIGH_WATER: u64 = 256 * 1024;
/// Output waiting in SparkBox for the window, per tab; beyond it the oldest bytes are dropped.
pub const RING_MAX: usize = 2 * 1024 * 1024;

/// What input to a tab that ended gets.
pub const CLOSED: &str = "This tab has closed.";
/// What input to a locked shell gets.
pub const LOCKED_INPUT: &str = "Locked — authenticate to keep typing. Keystrokes typed while locked aren't sent.";
/// A shell that connected while Admin mode relocked waits for the owner.
pub const LOCKED_WHILE_OPENING: &str = "Admin mode locked while this shell was connecting. Authenticate to start typing.";

/// What runs in the terminal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(from = "WireTarget", into = "WireTarget")]
pub enum Target {
    /// A login shell on the node.
    Shell,
    /// A login shell starting in a folder (Files › Open terminal here): `{kind: "shell", cwd}` to the UI.
    ShellIn { cwd: String },
    /// bash (or sh) inside a running container.
    ContainerShell { container: String },
    /// `docker logs -f` for a container, from `since` (see SINCE) or its last 500 lines.
    ContainerLogs { container: String, since: Option<String> },
    /// `journalctl -f` for a systemd unit, from `since` or its last 300 lines.
    Journal { unit: String, since: Option<String> },
}

/// How far back a log tab starts: (UI value, docker --since, journalctl argument).
const SINCE: &[(&str, &str, &str)] = &[
    ("10m", "10m", "--since -10min"),
    ("1h", "1h", "--since -1h"),
    ("6h", "6h", "--since -6h"),
    ("24h", "24h", "--since -24h"),
    ("boot", "", "-b"),
];

/// How the UI writes a target: a shell has an optional starting folder.
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum WireTarget {
    Shell {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    ContainerShell { container: String },
    ContainerLogs {
        container: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    Journal {
        unit: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
}

impl From<WireTarget> for Target {
    fn from(w: WireTarget) -> Target {
        match w {
            WireTarget::Shell { cwd: None } => Target::Shell,
            WireTarget::Shell { cwd: Some(cwd) } => Target::ShellIn { cwd },
            WireTarget::ContainerShell { container } => Target::ContainerShell { container },
            WireTarget::ContainerLogs { container, since } => Target::ContainerLogs { container, since },
            WireTarget::Journal { unit, since } => Target::Journal { unit, since },
        }
    }
}

impl From<Target> for WireTarget {
    fn from(t: Target) -> WireTarget {
        match t {
            Target::Shell => WireTarget::Shell { cwd: None },
            Target::ShellIn { cwd } => WireTarget::Shell { cwd: Some(cwd) },
            Target::ContainerShell { container } => WireTarget::ContainerShell { container },
            Target::ContainerLogs { container, since } => WireTarget::ContainerLogs { container, since },
            Target::Journal { unit, since } => WireTarget::Journal { unit, since },
        }
    }
}

impl Target {
    /// Shells can change anything on the node: they need Admin mode, to open and to type in.
    pub fn is_shell(&self) -> bool {
        matches!(self, Target::Shell | Target::ShellIn { .. } | Target::ContainerShell { .. })
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TermEvent {
    /// Terminal output, base64 (it's raw bytes; a UTF-8 character can span two chunks).
    Data { data: String },
    Exit { code: Option<u32>, message: Option<String> },
    /// The shell was locked (the owner locked, or nothing was typed for the idle time): it
    /// takes no input until it is authorized again.
    Locked { reason: String },
    /// This many bytes of output were dropped (oldest first) while the window wasn't taking any.
    Skipped { bytes: u64 },
    /// Authorized again: input goes through.
    Unlocked,
    /// The shell locks in a minute unless something is typed.
    IdleWarning { text: String },
}

impl TermEvent {
    pub fn data(bytes: &[u8]) -> TermEvent {
        TermEvent::Data { data: base64::engine::general_purpose::STANDARD.encode(bytes) }
    }
}

pub type Emit = Box<dyn Fn(TermEvent) + Send + Sync>;

pub(crate) enum Input {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}

/// What the session sends towards the UI, in order.
pub(crate) enum Out {
    Data(Vec<u8>),
    Skipped(u64),
    Exit { code: Option<u32>, message: Option<String> },
    Locked(String),
    Unlocked,
    IdleWarning(String),
}

/// Bytes sent to the UI and not yet acknowledged.
#[derive(Default)]
pub(crate) struct Flow {
    unacked: AtomicU64,
    acked: Notify,
}

impl Flow {
    pub fn ack(&self, bytes: u64) {
        let mut cur = self.unacked.load(Ordering::Acquire);
        while let Err(now) = self.unacked.compare_exchange_weak(cur, cur.saturating_sub(bytes), Ordering::AcqRel, Ordering::Acquire) {
            cur = now;
        }
        self.acked.notify_one();
    }

    pub fn unacked(&self) -> u64 {
        self.unacked.load(Ordering::Acquire)
    }

    fn sent(&self, bytes: usize) {
        self.unacked.fetch_add(bytes as u64, Ordering::AcqRel);
    }
}

/// Container names/ids and systemd unit names, with nothing a shell would interpret.
fn name_ok(name: &str, extra: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c) || extra.contains(c))
}

/// The docker and journalctl arguments for a log tab's starting point (only the fixed choices).
fn since_args(since: &Option<String>) -> Result<Option<(&'static str, &'static str)>, String> {
    match since.as_deref() {
        None | Some("") => Ok(None),
        Some(v) => SINCE.iter().find(|(k, _, _)| *k == v).map(|(_, d, j)| Some((*d, *j))).ok_or_else(|| format!("“{v}” isn't a log starting point SparkBox knows.")),
    }
}

/// The command a target runs under the PTY (None: the login shell).
pub fn command_for(target: &Target) -> Result<Option<String>, String> {
    Ok(match target {
        Target::Shell => None,
        Target::ShellIn { cwd: dir } => {
            if dir.contains('\0') || dir.len() > 4096 || !(dir.starts_with('/') || dir == "~" || dir.starts_with("~/")) {
                return Err("That folder can't be used as a starting folder.".into());
            }
            let cd = match dir.strip_prefix("~") {
                Some("") => "cd".to_string(),
                Some(rest) => format!("cd -- \"$HOME\"{}", sh_quote(rest)),
                None => format!("cd -- {}", sh_quote(dir)),
            };
            Some(format!("{cd} && exec \"${{SHELL:-/bin/bash}}\" -l"))
        }
        Target::ContainerShell { container } if name_ok(container, "") => Some(format!(
            "docker exec -it -e TERM=xterm-256color {container} sh -c 'if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi'"
        )),
        Target::ContainerLogs { container, since } if name_ok(container, "") => match since_args(since)? {
            Some((docker, _)) if !docker.is_empty() => Some(format!("docker logs -f --since {docker} {container}")),
            _ => Some(format!("docker logs -f --tail 500 {container}")),
        },
        Target::Journal { unit, since } if name_ok(unit, "@:") => match since_args(since)? {
            Some((_, journal)) => Some(format!("journalctl -f {journal} --no-hostname -u {unit}")),
            None => Some(format!("journalctl -f -n 300 --no-hostname -u {unit}")),
        },
        _ => return Err("That container or service name has characters SparkBox won't pass to a shell.".into()),
    })
}

struct Session {
    tx: mpsc::Sender<Input>,
    flow: Arc<Flow>,
    shell: bool,
    /// A shell takes keystrokes only while authorized: from `authorize_shell` (its open in
    /// Admin mode, checked against relocks meanwhile) until it's locked (`revoke_shells`, the
    /// idle timer).
    authorized: bool,
    /// When something was last typed (or it was authorized), by the terminals' clock.
    last_input_ms: AtomicU64,
    /// The one-minute warning went out since the last keystroke.
    warned: AtomicBool,
    /// Lock and unlock notices for the tab.
    notices: mpsc::Sender<Out>,
    node: String,
    /// A shell on the real cluster: "Never" in Settings › Terminal means 8 hours for it.
    live: bool,
    /// A shell in a container (docker exec runs as root there, and Docker is root-equivalent
    /// on the node): it locks with Admin mode, whatever the reason.
    root: bool,
}

/// The longest a Live shell stays open with nothing typed, whatever Settings › Terminal says.
pub const LIVE_SHELL_IDLE_CAP_MIN: u32 = 8 * 60;

/// The idle limit that applies to a shell, in minutes (0: none): "Never" (0) only in Demo; a
/// Live shell is capped at 8 hours.
pub fn idle_limit_minutes(setting: u32, live: bool) -> u32 {
    match (setting, live) {
        (0, true) => LIVE_SHELL_IDLE_CAP_MIN,
        (m, true) => m.min(LIVE_SHELL_IDLE_CAP_MIN),
        (m, false) => m,
    }
}

impl Target {
    /// What the tab is, for the activity log: "a shell", "a shell in vllm-head", "vllm-head's logs".
    fn describe(&self) -> String {
        match self {
            Target::Shell | Target::ShellIn { .. } => "a shell".into(),
            Target::ContainerShell { container } => format!("a shell in {container}"),
            Target::ContainerLogs { container, .. } => format!("{container}'s logs"),
            Target::Journal { unit, .. } => format!("the journal of {unit}"),
        }
    }
}

pub struct Terminals {
    pub(crate) live: Arc<Live>,
    cluster: Arc<Mutex<DemoCluster>>,
    fs: Arc<Mutex<DemoFs>>,
    open: Mutex<HashMap<String, Session>>,
    next: AtomicU64,
    /// The idle timer's clock (the Admin gate's, so tests share one fake clock).
    clock: RwLock<Arc<dyn Clock>>,
    /// Lock a shell after this many minutes with nothing typed; 0 never.
    idle_minutes: AtomicU32,
    /// Tests: how long `open` takes, to relock while a shell is still connecting.
    #[cfg(test)]
    pub(crate) slow_open: Mutex<Duration>,
}

/// Forwards the session's output to the UI, in order.
fn spawn_emitter(mut rx: mpsc::Receiver<Out>, emit: Emit) {
    tauri::async_runtime::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Out::Data(bytes) => emit(TermEvent::data(&bytes)),
                Out::Skipped(bytes) => emit(TermEvent::Skipped { bytes }),
                Out::Locked(reason) => emit(TermEvent::Locked { reason }),
                Out::Unlocked => emit(TermEvent::Unlocked),
                Out::IdleWarning(text) => emit(TermEvent::IdleWarning { text }),
                Out::Exit { code, message } => {
                    emit(TermEvent::Exit { code, message });
                    break;
                }
            }
        }
    });
}

impl Terminals {
    /// Terminals whose demo shell reads the app's demo cluster and filesystem (shared with Files).
    pub fn with_demo(live: Arc<Live>, cluster: Arc<Mutex<DemoCluster>>, fs: Arc<Mutex<DemoFs>>) -> Arc<Terminals> {
        Arc::new(Terminals {
            live,
            cluster,
            fs,
            open: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            clock: RwLock::new(Arc::new(SystemClock)),
            idle_minutes: AtomicU32::new(crate::admin::SecuritySettings::default().shell_idle_minutes),
            #[cfg(test)]
            slow_open: Mutex::new(Duration::ZERO),
        })
    }

    /// Uses the Admin gate's clock for the idle timer.
    pub fn set_clock(&self, clock: Arc<dyn Clock>) {
        *self.clock.write().unwrap() = clock;
    }

    /// Settings › Terminal › Lock idle shells after (minutes; 0 never).
    pub fn set_idle_minutes(&self, minutes: u32) {
        self.idle_minutes.store(minutes, Ordering::SeqCst);
    }

    fn now_ms(&self) -> u64 {
        self.clock.read().unwrap().now_ms()
    }

    /// Terminals with a demo world of their own (tests of the live path).
    #[cfg(test)]
    pub fn new(live: Arc<Live>) -> Arc<Terminals> {
        let cluster = Arc::new(Mutex::new(DemoCluster::new(&crate::inventory::Inventory::example())));
        Self::with_demo(live, cluster, Arc::new(Mutex::new(DemoFs::new())))
    }

    /// Starts a terminal and returns its id; output and the exit arrive through `emit`.
    #[allow(clippy::too_many_arguments)]
    pub async fn open(self: &Arc<Self>, demo: bool, node: &str, target: Target, cols: u32, rows: u32, emit: Emit) -> Result<String, String> {
        let command = command_for(&target)?;
        let root = matches!(target, Target::ContainerShell { .. });
        let id = format!("t{}", self.next.fetch_add(1, Ordering::Relaxed));
        let what = target.describe();
        let shown = command.clone().unwrap_or_else(|| "login shell".into());
        let (tx, mut rx) = mpsc::channel(1024);
        let (out_tx, out_rx) = mpsc::channel(4);
        let notices = out_tx.clone();
        let flow = Arc::new(Flow::default());
        let shell = target.is_shell();
        let me = self.clone();
        let done = id.clone();
        #[cfg(test)]
        {
            let wait = *self.slow_open.lock().unwrap();
            tokio::time::sleep(wait).await;
        }
        // One activity-log entry per tab, from open to close (never keystrokes): a shell is a
        // session, a log or journal follower a read.
        let entry = |simulated: bool| crate::live::audit::Entry {
            node: node.to_string(),
            kind: "terminal".into(),
            command: format!("ssh {node} {shown}"),
            title: format!("Open {what} on {node}"),
            class: if shell { "session" } else { "read" }.into(),
            simulated,
            ..Default::default()
        };
        if demo {
            let head = self.live.audit.begin(entry(true));
            let node = node.to_string();
            let (cluster, fs) = (self.cluster.clone(), self.fs.clone());
            spawn_emitter(out_rx, emit);
            let f = flow.clone();
            tauri::async_runtime::spawn(async move {
                let (demo_tx, demo_rx) = mpsc::channel(64);
                let paced = tokio::spawn(relay(demo_rx, f, out_tx.clone()));
                let (code, message) = demo::run(&node, &target, cols, &mut rx, &demo_tx, cluster, fs).await;
                drop(demo_tx);
                let _ = paced.await;
                me.live.audit.end(&head, session_ending(code, &message));
                let _ = out_tx.send(Out::Exit { code, message }).await;
                me.open.lock().unwrap().remove(&done);
            });
        } else {
            // A shell (a container's too, whose container the exit checks) leaves as `shell`; a
            // log or journal follower stays a read by its shape.
            let pty = self.live.pty(node, cols.max(2), rows.max(2), command.as_deref());
            let opened = if shell { crate::live::exit::as_class(crate::live::fence_class::FenceClass::Shell, pty).await } else { pty.await };
            let (read, write, session) = match opened {
                Ok(pty) => pty.into_parts(),
                Err(e) => {
                    // A tab that couldn't open is in the log too, as what it would have been.
                    self.live.audit.record(crate::live::audit::Entry { error: Some(e.clone()), state: "failed".into(), ..entry(false) });
                    return Err(e);
                }
            };
            let head = self.live.audit.begin(entry(false));
            spawn_emitter(out_rx, emit);
            let f = flow.clone();
            tauri::async_runtime::spawn(async move {
                let (code, message) = pump(read, write, rx, f, out_tx.clone()).await;
                me.live.audit.end(&head, session_ending(code, &message));
                drop(session);
                let _ = out_tx.send(Out::Exit { code, message }).await;
                me.open.lock().unwrap().remove(&done);
            });
        }
        // Unauthorised until `authorize_opened` (or `authorize_shell`) says otherwise.
        let session = Session { tx, flow, shell, authorized: false, last_input_ms: AtomicU64::new(self.now_ms()), warned: AtomicBool::new(false), notices, node: node.to_string(), live: !demo, root };
        self.open.lock().unwrap().insert(id.clone(), session);
        Ok(id)
    }

    /// Opens a tab as `terminal_open` does: a shell is authorised once it's there, and only if
    /// Admin mode stayed unlocked all through its open (a relock meanwhile, even an idle one,
    /// leaves it waiting locked for the owner).
    #[allow(clippy::too_many_arguments)]
    pub async fn open_gated(self: &Arc<Self>, gate: &crate::admin::AdminGate, demo: bool, node: &str, target: Target, cols: u32, rows: u32, emit: Emit) -> Result<String, String> {
        let shell = target.is_shell();
        let generation = gate.generation();
        if shell && !gate.state().unlocked {
            return Err(crate::errors::SbError::new(crate::errors::ErrorKind::Locked, crate::admin::LOCKED).to_json());
        }
        let id = self.open(demo, node, target, cols, rows, emit).await?;
        if shell {
            self.authorize_opened(&id, gate.generation() == generation && gate.state().unlocked)?;
        }
        Ok(id)
    }

    fn send(&self, id: &str, input: Input) -> Result<(), String> {
        let open = self.open.lock().unwrap();
        let s = open.get(id).ok_or(CLOSED)?;
        s.tx.try_send(input).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => "The terminal is busy; try again in a moment.".to_string(),
            mpsc::error::TrySendError::Closed(_) => CLOSED.to_string(),
        })
    }

    /// True for shells (typing needs Admin mode); false for log followers and closed tabs.
    pub fn is_shell(&self, id: &str) -> bool {
        self.open.lock().unwrap().get(id).is_some_and(|s| s.shell)
    }

    /// True while a shell may take keystrokes (opened in Admin mode and not revoked since).
    pub fn shell_authorized(&self, id: &str) -> bool {
        self.open.lock().unwrap().get(id).is_some_and(|s| s.shell && s.authorized)
    }

    /// Lets a shell take keystrokes again (after the user authenticated). Part of C-live.
    pub fn authorize_shell(&self, id: &str) -> Result<(), String> {
        let now = self.now_ms();
        let mut open = self.open.lock().unwrap();
        let s = open.get_mut(id).filter(|s| s.shell).ok_or(CLOSED)?;
        s.last_input_ms.store(now, Ordering::Relaxed);
        s.warned.store(false, Ordering::Relaxed);
        if !s.authorized {
            s.authorized = true;
            let _ = s.notices.try_send(Out::Unlocked);
        }
        Ok(())
    }

    /// A shell that just opened: authorised when Admin mode stayed unlocked all through its
    /// open (`still_unlocked`, the gate's generation unchanged); otherwise it waits locked.
    pub fn authorize_opened(&self, id: &str, still_unlocked: bool) -> Result<(), String> {
        let now = self.now_ms();
        let mut open = self.open.lock().unwrap();
        let s = open.get_mut(id).filter(|s| s.shell).ok_or(CLOSED)?;
        if still_unlocked {
            // It was never locked, so there's nothing to tell the tab.
            s.authorized = true;
            s.last_input_ms.store(now, Ordering::Relaxed);
        } else {
            let _ = s.notices.try_send(Out::Locked(LOCKED_WHILE_OPENING.into()));
        }
        Ok(())
    }

    /// Admin mode relocked: shells stop taking keystrokes only when the owner or macOS locked
    /// (`RelockReason::ends_shells`). Returns how many it stopped.
    pub fn on_relock(&self, why: RelockReason, text: &str) -> usize {
        if why.ends_shells() {
            self.revoke_shells(text)
        } else {
            // Root shells (in containers) and, on the real cluster, login-user shells lock with
            // Admin mode for any reason, idle relock included: the login user is in the docker
            // group, which is root-equivalent on the node (P1.42).
            let mut n = 0;
            // The reason ends in one period whichever relock sent it (the idle one has its own).
            let text = text.trim_end().trim_end_matches('.');
            for s in self.open.lock().unwrap().values_mut().filter(|s| s.shell && s.authorized && (s.root || s.live)) {
                s.authorized = false;
                let why = if s.root {
                    format!("{text}. A shell in a container runs as root, so it locks with Admin mode.")
                } else {
                    format!("{text}. A shell on the cluster can change the node (the docker group is root-equivalent), so it locks with Admin mode.")
                };
                let _ = s.notices.try_send(Out::Locked(why));
                n += 1;
            }
            n
        }
    }

    /// The idle timer, once a second: a minute before a shell's idle time is up its tab warns;
    /// at the time it locks. Typing resets both.
    pub fn tick(&self) {
        let setting = self.idle_minutes.load(Ordering::SeqCst);
        let now = self.now_ms();
        let mut open = self.open.lock().unwrap();
        for s in open.values_mut().filter(|s| s.shell && s.authorized) {
            let minutes = idle_limit_minutes(setting, s.live);
            if minutes == 0 {
                continue;
            }
            let limit = crate::admin::scaled(u64::from(minutes) * 60_000);
            let warn = crate::admin::WARN_MS.min(limit / 2);
            let idle = now.saturating_sub(s.last_input_ms.load(Ordering::Relaxed));
            if idle >= limit {
                s.authorized = false;
                let _ = s.notices.try_send(Out::Locked(format!("Locked: nothing typed for {}.", idle_words(minutes))));
            } else if idle + warn >= limit && !s.warned.swap(true, Ordering::Relaxed) {
                let text = format!("This shell locks in 60 s: nothing typed for {}. Type anything to keep it open.", idle_words_less_a_minute(minutes));
                let _ = s.notices.try_send(Out::IdleWarning(text));
            }
        }
    }

    /// Runs `tick` every second for the app's lifetime.
    pub fn start_idle_timer(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(1));
            loop {
                every.tick().await;
                let Some(t) = me.upgrade() else { return };
                t.tick();
            }
        });
    }

    /// Notes that broadcast typing started or stopped and which shells it reaches (never what is typed).
    pub fn note_broadcast(&self, demo: bool, on: bool, ids: &[String]) {
        let open = self.open.lock().unwrap();
        let reached: Vec<String> = ids.iter().filter_map(|i| open.get(i)).filter(|s| s.shell).map(|s| s.node.clone()).collect();
        drop(open);
        if reached.is_empty() && on {
            return;
        }
        let title = if on { format!("Broadcast typing to {} shell{}", reached.len(), if reached.len() == 1 { "" } else { "s" }) } else { "Stop broadcast typing".into() };
        let detail = if reached.is_empty() { String::new() } else { reached.join(", ") };
        // Part of the shells' sessions: shown under Everything, not as a change.
        self.live.audit.record(crate::live::audit::Entry {
            node: "mac".into(),
            kind: "app".into(),
            command: detail,
            title,
            class: "session".into(),
            simulated: demo,
            state: "ok".into(),
            ..Default::default()
        });
    }

    /// Suspends input to every shell (Admin mode relocked); each tab shows why. Part of C-live.
    pub fn revoke_shells(&self, reason: &str) -> usize {
        let mut n = 0;
        for s in self.open.lock().unwrap().values_mut().filter(|s| s.shell && s.authorized) {
            s.authorized = false;
            let _ = s.notices.try_send(Out::Locked(reason.to_string()));
            n += 1;
        }
        n
    }

    /// Keystrokes for a shell. Log and journal followers are read-only, so Ctrl-C can't stop
    /// them. Each keystroke resets the shell's idle timer (an atomic store, nothing more).
    pub fn input(&self, id: &str, data: Vec<u8>) -> Result<(), String> {
        let now = self.now_ms();
        match self.open.lock().unwrap().get(id) {
            None => return Err(CLOSED.into()),
            Some(s) if !s.shell => return Err("This tab is read-only.".into()),
            Some(s) if !s.authorized => return Err(LOCKED_INPUT.into()),
            Some(s) => {
                s.last_input_ms.store(now, Ordering::Relaxed);
                s.warned.store(false, Ordering::Relaxed);
            }
        }
        self.send(id, Input::Data(data))
    }

    pub fn resize(&self, id: &str, cols: u32, rows: u32) -> Result<(), String> {
        self.send(id, Input::Resize(cols.max(2), rows.max(2)))
    }

    /// The UI has drawn `bytes` more of this terminal's output.
    pub fn ack(&self, id: &str, bytes: u64) {
        if let Some(s) = self.open.lock().unwrap().get(id) {
            s.flow.ack(bytes);
        }
    }

    pub fn close(&self, id: &str) {
        let tx = self.open.lock().unwrap().get(id).map(|s| s.tx.clone());
        if let Some(tx) = tx {
            // Close even when the input queue is full.
            tauri::async_runtime::spawn(async move {
                let _ = tx.send(Input::Close).await;
            });
        }
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.open.lock().unwrap().len()
    }
}

/// "60 min", "4 h" (the setting's own words).
fn idle_words(minutes: u32) -> String {
    if minutes >= 120 && minutes % 60 == 0 {
        format!("{} h", minutes / 60)
    } else {
        format!("{minutes} min")
    }
}

/// How long nothing has been typed when the warning shows: "59 min", "3 h 59 min".
fn idle_words_less_a_minute(minutes: u32) -> String {
    let m = minutes.saturating_sub(1);
    if m >= 60 && minutes > 60 {
        format!("{} h {} min", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

/// How a terminal tab's activity-log entry ends: how long it was open and how it closed.
fn session_ending(code: Option<u32>, message: &Option<String>) -> crate::live::audit::Ending {
    use crate::live::audit::{Ending, Outcome};
    let how = match (code, message) {
        (_, Some(m)) => m.clone(),
        (Some(c), None) => format!("exit code {c}"),
        (None, None) => "closed".into(),
    };
    // A shell's last exit code isn't the session failing; a dropped connection is worth a look.
    let outcome = if message.is_some() && code.is_none() { Outcome::Warning } else { Outcome::Ok };
    Ending { code, stdout_tail: Some(format!("Closed: {how}")), error: (outcome == Outcome::Warning).then(|| how.clone()), outcome: Some(outcome), ..Default::default() }
}

/// Output the window hasn't taken yet: at most RING_MAX bytes, the oldest dropped first.
#[derive(Default)]
pub(crate) struct Ring {
    buf: std::collections::VecDeque<u8>,
    /// Dropped since the window last took output.
    skipped: u64,
    /// The node closed the channel: what's left is all there is.
    eof: bool,
    code: Option<u32>,
    message: Option<String>,
}

impl Ring {
    fn push(&mut self, data: &[u8]) {
        self.buf.extend(data);
        let over = self.buf.len().saturating_sub(RING_MAX);
        if over > 0 {
            // What's kept starts at a line (within 4 KiB), not halfway through one.
            let to_line = self.buf.iter().skip(over).take(4096).position(|&b| b == b'\n').map_or(0, |i| i + 1);
            self.buf.drain(..over + to_line);
            self.skipped += (over + to_line) as u64;
        }
    }
}

/// Reads the channel to its end, whatever the window is doing: everything goes into `ring`.
async fn drain(mut read: ChannelReadHalf, ring: Arc<Mutex<Ring>>, more: Arc<Notify>) {
    loop {
        let msg = read.wait().await;
        let mut r = ring.lock().unwrap();
        match msg {
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => r.push(&data),
            Some(ChannelMsg::ExitStatus { exit_status }) => r.code = Some(exit_status),
            Some(ChannelMsg::ExitSignal { signal_name, .. }) => r.message = Some(format!("ended by signal {}", crate::live::ssh::sig_name(&signal_name))),
            Some(ChannelMsg::Close) | None => r.eof = true,
            _ => continue,
        }
        let eof = r.eof;
        drop(r);
        more.notify_one();
        if eof {
            return;
        }
    }
}

/// Sends keystrokes, resizes and the close to the node, in order.
async fn feed(write: Arc<ChannelWriteHalf<russh::client::Msg>>, mut rx: mpsc::Receiver<Input>, ring: Arc<Mutex<Ring>>, more: Arc<Notify>) {
    while let Some(input) = rx.recv().await {
        match input {
            Input::Data(bytes) => {
                if write.data(&bytes[..]).await.is_err() {
                    ring.lock().unwrap().message.get_or_insert_with(|| "The connection to the node closed.".into());
                    break;
                }
            }
            Input::Resize(cols, rows) => {
                let _ = write.window_change(cols, rows, 0, 0).await;
            }
            Input::Close => break,
        }
    }
    // Closed by the window (or it went away): end the session on the node.
    let _ = write.eof().await;
    let _ = write.close().await;
    let mut r = ring.lock().unwrap();
    r.eof |= r.buf.is_empty();
    drop(r);
    more.notify_one();
}

/// Sends what's in `ring` to the window: in batches, while it acknowledges, with a Skipped
/// notice before output that follows a drop. Returns when everything is sent after the end,
/// or when the window has gone away.
async fn deliver(ring: &Mutex<Ring>, more: &Notify, flow: &Flow, out: &mpsc::Sender<Out>) {
    let mut first_at: Option<Instant> = None;
    let mut idle_since: Option<Instant> = None;
    loop {
        let can_send = flow.unacked() < HIGH_WATER;
        let (pending, skipped, eof) = {
            let r = ring.lock().unwrap();
            (r.buf.len(), r.skipped, r.eof)
        };
        if pending > 0 && first_at.is_none() {
            first_at = Some(Instant::now());
        }
        let due = pending > 0 && (pending >= BATCH_MAX || eof || first_at.is_some_and(|t| t.elapsed() >= BATCH_WAIT));
        if can_send && (due || skipped > 0) {
            idle_since = None;
            if skipped > 0 {
                ring.lock().unwrap().skipped = 0;
                if out.send(Out::Skipped(skipped)).await.is_err() {
                    break;
                }
            }
            if due {
                let chunk: Vec<u8> = {
                    let mut r = ring.lock().unwrap();
                    let n = r.buf.len().min(BATCH_MAX);
                    r.buf.drain(..n).collect()
                };
                flow.sent(chunk.len());
                if out.send(Out::Data(chunk)).await.is_err() {
                    break; // the UI went away
                }
                first_at = (pending > BATCH_MAX).then(Instant::now);
            }
            continue;
        }
        if eof && pending == 0 {
            break;
        }
        if eof && !can_send {
            // The node has finished but the window stopped drawing (it was closed or reloaded).
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_secs(10) {
                break;
            }
        }
        let wait = match first_at {
            Some(t) if can_send => BATCH_WAIT.saturating_sub(t.elapsed()),
            _ => Duration::from_millis(250),
        };
        tokio::select! {
            _ = more.notified() => {}
            _ = flow.acked.notified(), if !can_send => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// Demo sessions take the same road to the window: the simulated shell's output goes into a
/// ring and out at the window's pace, with the same skips.
async fn relay(mut rx: mpsc::Receiver<Out>, flow: Arc<Flow>, out: mpsc::Sender<Out>) {
    let ring: Arc<Mutex<Ring>> = Arc::default();
    let more = Arc::new(Notify::new());
    let (r, m) = (ring.clone(), more.clone());
    let notices = out.clone();
    let reader = tokio::spawn(async move {
        while let Some(o) = rx.recv().await {
            match o {
                Out::Data(d) => r.lock().unwrap().push(&d),
                other => {
                    let _ = notices.send(other).await;
                }
            }
            m.notify_one();
        }
        r.lock().unwrap().eof = true;
        m.notify_one();
    });
    deliver(&ring, &more, &flow, &out).await;
    reader.abort();
}

/// Copies node output to the UI (batched, paced by acknowledgements, never holding up the
/// channel) and keystrokes to the node, until either side closes. Returns the exit code and
/// why it ended.
pub(crate) async fn pump(
    read: ChannelReadHalf,
    write: ChannelWriteHalf<russh::client::Msg>,
    rx: mpsc::Receiver<Input>,
    flow: Arc<Flow>,
    out: mpsc::Sender<Out>,
) -> (Option<u32>, Option<String>) {
    let ring: Arc<Mutex<Ring>> = Arc::default();
    let more = Arc::new(Notify::new());
    let write = Arc::new(write);
    let reader = tokio::spawn(drain(read, ring.clone(), more.clone()));
    let writer = tokio::spawn(feed(write.clone(), rx, ring.clone(), more.clone()));
    deliver(&ring, &more, &flow, &out).await;
    reader.abort();
    writer.abort();
    let (code, message, eof) = {
        let r = ring.lock().unwrap();
        (r.code, r.message.clone(), r.eof)
    };
    if !eof {
        // The window went away first: don't leave the session running on the node.
        let _ = write.eof().await;
        let _ = write.close().await;
    }
    (code, message)
}

/// A file name made of what a tab is called, safe on any filesystem.
fn file_stem(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || " ._-@".contains(c) { c } else { '-' }).take(80).collect();
    let s = s.trim_matches(|c: char| c == '.' || c == ' ' || c == '-').to_string();
    if s.is_empty() { "terminal".into() } else { s }
}

/// "2026-10-28 14.02" in local time.
fn local_stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!("{:04}-{:02}-{:02} {:02}.{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// Saves a terminal tab's text into `dir` under a new name (never replacing a file).
pub fn save_text(dir: &std::path::Path, name: &str, text: &str) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("can't open {}: {e}", dir.display()))?;
    let stem = format!("{} {}", file_stem(name), local_stamp());
    let path = (1..)
        .map(|n| dir.join(if n == 1 { format!("{stem}.log") } else { format!("{stem} {n}.log") }))
        .find(|p| p.symlink_metadata().is_err())
        .unwrap();
    crate::util::write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

/// The Locked overlay's button, once the user has authenticated: the shell takes keystrokes again.
#[tauri::command]
pub fn terminal_authorize(app: tauri::AppHandle, state: tauri::State<crate::AppState>, id: String) -> Result<(), String> {
    crate::admin::require(&app)?;
    state.terminals.authorize_shell(&id)
}

/// Broadcast typing turned on or off: noted in the activity log with the shells it reaches.
#[tauri::command]
pub fn terminal_broadcast(state: tauri::State<crate::AppState>, on: bool, ids: Vec<String>) {
    state.terminals.note_broadcast(!crate::is_live(&state), on, &ids);
}

/// Terminal › Save: a log tab's text into ~/Downloads. Only ever creates a new file on this
/// Mac (nothing on a node changes), like a browser download.
#[tauri::command]
pub fn terminal_save_text(name: String, text: String) -> Result<String, String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).ok_or("No home folder")?;
    save_text(&home.join("Downloads"), &name, &text).map(|p| p.display().to_string())
}
