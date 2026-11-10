//! Test-only fake Spark that behaves like a real one: an SSH server (russh) that runs
//! the exact bytes of each exec request with `/bin/dash -c` in a temporary home, as sshd
//! does with an Ubuntu login shell's `sh` (so the `env LC_ALL=C sh -c '…'` wrapper,
//! sparkjob.py, tar, cat… really run, and a bashism fails here as it would on a Spark),
//! streams collector frames once a second, and lets a script answer anything that needs a
//! GB10 (docker, nvidia-smi). `sh` on the node's PATH is dash too, and so is every shell shim,
//! every `Node::bin` script (`#!/bin/dash`) and a `systemd-run` unit's `/bin/sh`: never macOS's
//! /bin/sh, which is bash 3.2.
//!
//! What the nodes' shell is (a look-only evidence capture of evidence.rs's `shell` step,
//! 2026-10-30 12:49 EDT, `~/Library/Logs/SparkBox/evidence/2026-10-30-r4-agent/124924-shell-facts`):
//! on cell1, cell2, cell3 and gx10-0000 alike, /bin/sh and /usr/bin/sh are symlinks to dash
//! (both resolve to /usr/bin/dash), `type sh` finds /usr/bin/sh, dash is 0.5.12-6ubuntu5, bash
//! is 5.2.21-2ubuntu4, and sparkuser's login shell is /bin/bash. So sshd hands each command to
//! `/bin/bash -c`, which only unwraps SparkBox's `env LC_ALL=C sh -c '<command>'`, and dash
//! parses `<command>` itself. This Mac's /bin/dash is Apple's dash-16 build (it carries no
//! upstream version), which reads the same: no `[[`, `$'…'`, brace expansion or arrays.
//! Where the fake differs: it hands the exec request's bytes to dash, not to bash first (they
//! read SparkBox's one-quoted-word wrapper the same); its terminal's login shell is `dash -i +m`
//! where a node's is `bash -i` with the user's .bashrc; and the Mac has no bash 5 (a script
//! that runs `bash` explicitly gets macOS's bash 3.2 here).
//!
//! Like sshd, it starts the command at exec time, streams stdin to it as it arrives,
//! forwards stdout and stderr in chunks as they're produced (waiting for the client's
//! window), and ends with `exit-status` for a normal exit or `exit-signal` (name,
//! core dumped, message) for a death by signal. A channel with a PTY runs its command
//! (or, for a shell request, `sh -i`) on a real pseudo-terminal, so echo, Return and
//! window size behave as they do over ssh.
//!
//! The node's PATH starts with shims (`bin_dir`): a `sudo` that parses
//! `-k -S -p <prompt> --`, prints the prompt to stderr, reads one line of stdin and
//! checks it against the configured password (or runs at once with NOPASSWD), then
//! execs the rest with no real privilege; and `systemctl`, `systemd-run`,
//! `journalctl`, `apt-get`, `dpkg`, `chown` and `ethtool` stand-ins that keep their
//! state under `state_dir` (a plain node also has `dpkg-query`, `needrestart` and
//! `apt-mark`). `Node::bin` adds or replaces any executable.
//!
//! Scripts, `log` and `stdin` see the logical command: the inner command of the
//! `env LC_ALL=C sh -c` wrapper, and sudo commands as `sudo -k -S -p '' <command>`
//! with the password line first on stdin. `raw` keeps exactly what the client sent.
//! A script that answers a sudo command still gets it through the sudo shim, so the
//! password exchange is the real one.
//!
//! Fault knobs (`Knobs`): drop every connection, stop frames without closing, freeze one
//! collector after N frames, a node that stops answering altogether, slow replies, NOPASSWD
//! sudo, a different sudo password, a child killed by a signal, a channel closed without an
//! exit status, refused channel opens, and counters of open connections, collector channels
//! and session channels. `systemctl stop ssh` (the shim) stops "sshd": new connections are
//! closed at once while open ones keep working, as on a real node.
//!
//! Like OpenSSH's MaxSessions (10 by default), a connection may have at most `max_sessions`
//! session channels open; the next is refused as "administratively prohibited". A channel's
//! place is given back when its command has ended and the channel is closed, so a command
//! that keeps running after the client closed its channel still holds one, as on sshd. Port
//! forwards don't count.
//!
//! A channel the client closes is handled as sshd handles it: a PTY session is hung up (HUP to
//! its process group, as closing the terminal does), and a plain command is left running. Its
//! input ends and its output is closed, so it stops only when it reads end of input or writes
//! (SIGPIPE), or when the client sends a signal first.
//!
//! `Profile::spark("cell1")` is one of the owner's real Sparks as the Live tour found it
//! (live/fixtures/doctor, built by spark_tree.py): the real collector.py runs against that
//! node's /proc, /sys and /etc, the node's own commands answer as they did for the cluster
//! user (nvidia-smi at P8 and 4 W, ethtool refusing Wake-on without root, nmcli, iw, `ss`
//! without owners, docker with the exited glm53, the 92 systemd units, apt's pending
//! packages), its Docker socket serves its containers, and it presents its own host key
//! (cell1, cell2 and cell3 the same one, as the cloned image does).

use std::collections::{HashMap, HashSet};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::PublicKey;
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, ChannelOpenFailure, ChannelWriteHalf, Disconnect, Sig};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::inventory::{Inventory, NodeConfig};
use crate::live::tests::{frame, HOST_KEY, HOST_KEY_GX10};
use crate::live::{CancelToken, Live};
use crate::model::LinkStatus;
use crate::vpn::Vpn;

/// Answers a command instead of the shell: (stdout, exit code), or None to run it for real.
pub type Script = Arc<dyn Fn(&str, &[u8]) -> Option<(String, u32)> + Send + Sync>;

/// Ways to make the fake misbehave, and what it saw.
pub struct Knobs {
    /// Collector channels keep their connection but stop sending frames.
    pub stall_frames: AtomicBool,
    /// Each command waits this long before answering.
    pub slow_ms: AtomicU64,
    /// sudo runs without asking for a password.
    pub nopasswd: AtomicBool,
    pub sudo_password: Mutex<String>,
    /// Interval between collector frames, in ms.
    pub frame_every_ms: AtomicU64,
    /// A frame to send instead of `live::tests::frame()` (e.g. a node with free memory, or garbage).
    pub frame: Mutex<Option<String>>,
    pub open_connections: AtomicUsize,
    pub total_connections: AtomicUsize,
    /// Collector channels open right now.
    pub collectors: AtomicUsize,
    /// Signals received (e.g. "TERM"), in order.
    pub signals: Mutex<Vec<String>>,
    /// The password login accepts this password (None: password login is off, keys only).
    pub password: Mutex<Option<String>>,
    /// Public keys `publickey` login accepts (authorized_keys).
    pub authorized_keys: Mutex<Vec<PublicKey>>,
    /// How each login succeeded ("password" or "publickey"), in order.
    pub logins: Mutex<Vec<String>>,
    /// Kill the next real command whose logical text contains the pattern: (pattern, signal).
    kill: Mutex<Option<(String, String)>>,
    /// Close the next matching command's channel without an exit status.
    no_status: Mutex<Option<String>>,
    handles: Mutex<Vec<server::Handle>>,
    /// The collector really runs (the Spark profile) instead of sending canned frames.
    pub real_collector: AtomicBool,
    /// The next collector channel sends this many frames, then stays open and quiet (a
    /// collector stuck reading /proc or waiting on nvidia-smi). One stall: later ones run.
    freeze_after: Mutex<Option<usize>>,
    /// The node stops answering (frozen under memory pressure): no output on any channel, and
    /// new channels and new logins wait, until `thaw`.
    hung: AtomicBool,
    thawed: tokio::sync::Notify,
    /// sshd's MaxSessions: session channels one connection may have open (None: no limit).
    pub max_sessions: Mutex<Option<usize>>,
    /// Refuse this many of the next session channel opens, as a node at its limit would.
    pub refuse_next: AtomicUsize,
    /// Session channel opens refused (over MaxSessions, or `refuse_next`).
    pub refused: AtomicUsize,
    /// The most session channels one connection had open at once.
    pub peak_sessions: AtomicUsize,
    /// The next login request (a password, or a key offered) closes the connection instead of
    /// being answered, as a node that goes away mid-login does (sshd killed, the link down).
    pub drop_at_login: AtomicBool,
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            stall_frames: AtomicBool::new(false),
            slow_ms: AtomicU64::new(0),
            nopasswd: AtomicBool::new(false),
            sudo_password: Mutex::new("pw".into()),
            frame_every_ms: AtomicU64::new(1000),
            frame: Mutex::new(None),
            open_connections: AtomicUsize::new(0),
            total_connections: AtomicUsize::new(0),
            collectors: AtomicUsize::new(0),
            signals: Mutex::new(Vec::new()),
            password: Mutex::new(Some("pw".into())),
            authorized_keys: Mutex::new(Vec::new()),
            logins: Mutex::new(Vec::new()),
            kill: Mutex::new(None),
            no_status: Mutex::new(None),
            handles: Mutex::new(Vec::new()),
            real_collector: AtomicBool::new(false),
            freeze_after: Mutex::new(None),
            hung: AtomicBool::new(false),
            thawed: tokio::sync::Notify::new(),
            max_sessions: Mutex::new(Some(10)),
            refuse_next: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            peak_sessions: AtomicUsize::new(0),
            drop_at_login: AtomicBool::new(false),
        }
    }
}

impl Knobs {
    /// Returns once the node answers again (at once when it isn't hung).
    async fn until_thawed(&self) {
        loop {
            let thawed = self.thawed.notified();
            if !self.hung.load(Ordering::SeqCst) {
                return;
            }
            thawed.await;
        }
    }

    /// The frames a new collector channel may send before it freezes (None: all of them).
    fn take_freeze(&self) -> Option<usize> {
        self.freeze_after.lock().unwrap().take()
    }
}

#[derive(Clone)]
pub struct Node {
    pub home: PathBuf,
    /// Commands as they ran (see the module notes).
    pub log: Arc<Mutex<Vec<String>>>,
    /// Commands exactly as the client sent them.
    pub raw: Arc<Mutex<Vec<String>>>,
    /// Job specs received on stdin by `sparkjob.py start`, in order.
    pub specs: Arc<Mutex<Vec<Value>>>,
    /// stdin of every command, by command, for checking what was sent (e.g. the sudo password).
    pub stdin: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    pub knobs: Arc<Knobs>,
    /// First on the node's PATH: the shims, and anything `bin` adds.
    pub bin_dir: PathBuf,
    /// Where the shims keep state (units, apt, journal).
    pub state_dir: PathBuf,
    /// Variables every command gets, and folders put before `bin_dir` on PATH (the Spark profile).
    env: Arc<Vec<(String, String)>>,
    path_first: Arc<Vec<PathBuf>>,
    script: Script,
}

impl Node {
    /// Drops every SSH connection, as if the node went away for a moment.
    pub async fn drop_connections(&self) {
        let handles: Vec<server::Handle> = self.knobs.handles.lock().unwrap().drain(..).collect();
        for h in handles {
            let _ = h.disconnect(Disconnect::ByApplication, "fake node going away".into(), "en".into()).await;
        }
    }

    pub fn open_connections(&self) -> usize {
        self.knobs.open_connections.load(Ordering::SeqCst)
    }

    pub fn collectors(&self) -> usize {
        self.knobs.collectors.load(Ordering::SeqCst)
    }

    pub fn total_connections(&self) -> usize {
        self.knobs.total_connections.load(Ordering::SeqCst)
    }

    /// The next collector channel sends `n` frames, then keeps its channel open and sends nothing.
    pub fn freeze_frames_after(&self, n: usize) {
        *self.knobs.freeze_after.lock().unwrap() = Some(n);
    }

    /// The node stops answering on every connection (and to new ones) until `thaw`.
    pub fn hang(&self) {
        self.knobs.hung.store(true, Ordering::SeqCst);
    }

    pub fn thaw(&self) {
        self.knobs.hung.store(false, Ordering::SeqCst);
        self.knobs.thawed.notify_waiters();
    }

    /// The most session channels one connection had open at once.
    pub fn peak_sessions(&self) -> usize {
        self.knobs.peak_sessions.load(Ordering::SeqCst)
    }

    /// Puts an executable called `name` first on the node's PATH (a dash script unless it starts
    /// with `#!`), replacing a shim of that name. Part of C-live.
    pub fn bin(&self, name: &str, script: &str) {
        write_exec(&self.bin_dir.join(name), script);
    }

    /// The next real command whose logical text contains `matching` is killed with `signal`
    /// ("KILL", "TERM"…) once it has received its first stdin (or after 200 ms without any).
    pub fn kill_child_with(&self, signal: &str, matching: &str) {
        *self.knobs.kill.lock().unwrap() = Some((matching.to_string(), signal.to_string()));
    }

    /// The next command whose logical text contains `matching` ends with its channel closed
    /// and no exit status (an sshd child that died, a connection cut mid-reply).
    pub fn close_without_status(&self, matching: &str) {
        *self.knobs.no_status.lock().unwrap() = Some(matching.to_string());
    }

    /// Lets `key` log in (a line in authorized_keys).
    pub fn authorize_key(&self, key: PublicKey) {
        self.knobs.authorized_keys.lock().unwrap().push(key);
    }

    /// Turns password login off (None) or sets the password it accepts.
    pub fn set_login_password(&self, password: Option<&str>) {
        *self.knobs.password.lock().unwrap() = password.map(String::from);
    }

    /// The next login request closes its connection unanswered (see `Knobs::drop_at_login`).
    pub fn drop_next_login(&self) {
        self.knobs.drop_at_login.store(true, Ordering::SeqCst);
    }

    /// A key the knobs allow, or one listed in the home's ~/.ssh/authorized_keys (as sshd reads it).
    fn key_allowed(&self, key: &PublicKey) -> bool {
        if self.knobs.authorized_keys.lock().unwrap().iter().any(|k| k.key_data() == key.key_data()) {
            return true;
        }
        let listed = std::fs::read_to_string(self.home.join(".ssh/authorized_keys")).unwrap_or_default();
        listed.lines().filter_map(|l| PublicKey::from_openssh(l.trim()).ok()).any(|k| k.key_data() == key.key_data())
    }

    /// The logical commands run so far that contain `pattern`.
    pub fn ran(&self, pattern: &str) -> Vec<String> {
        self.log.lock().unwrap().iter().filter(|c| c.contains(pattern)).cloned().collect()
    }

    fn take_once(slot: &Mutex<Option<String>>, cmd: &str) -> bool {
        let mut s = slot.lock().unwrap();
        if s.as_deref().is_some_and(|m| cmd.contains(m)) {
            *s = None;
            true
        } else {
            false
        }
    }
}

/// An executable at `path`: a dash script unless `script` names its own interpreter (`#!`).
fn write_exec(path: &Path, script: &str) {
    let body = if script.starts_with("#!") { script.to_string() } else { format!("#!{DASH}\n{script}") };
    std::fs::write(path, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Undoes `live::sh_quote` (one quoted word, nothing else).
fn sh_unquote(s: &str) -> Option<String> {
    let inner = s.trim().strip_prefix('\'')?.strip_suffix('\'')?;
    let text = inner.replace("'\\''", "\u{0}");
    if text.contains('\'') {
        return None;
    }
    Some(text.replace('\u{0}', "'"))
}

/// What a command means, under SparkBox's wrappers: (logical command, sudo prompt).
fn logical(raw: &str) -> (String, Option<String>) {
    let unwrap = |s: &str| s.strip_prefix("env LC_ALL=C sh -c ").and_then(sh_unquote);
    if let Some(rest) = raw.strip_prefix("sudo -k -S -p '") {
        if let Some((prompt, after)) = rest.split_once("' -- ") {
            let inner = unwrap(after).unwrap_or_else(|| after.to_string());
            return (format!("sudo -k -S -p '' {inner}"), Some(prompt.to_string()));
        }
    }
    (unwrap(raw).unwrap_or_else(|| raw.to_string()), None)
}

fn signal_number(name: &str) -> i32 {
    match name.trim_start_matches("SIG") {
        "KILL" => libc::SIGKILL,
        "INT" => libc::SIGINT,
        "HUP" => libc::SIGHUP,
        "QUIT" => libc::SIGQUIT,
        "ABRT" => libc::SIGABRT,
        "SEGV" => libc::SIGSEGV,
        "PIPE" => libc::SIGPIPE,
        "USR1" => libc::SIGUSR1,
        "ALRM" => libc::SIGALRM,
        _ => libc::SIGTERM,
    }
}

fn sig_of(n: i32) -> Sig {
    match n {
        libc::SIGKILL => Sig::KILL,
        libc::SIGTERM => Sig::TERM,
        libc::SIGINT => Sig::INT,
        libc::SIGHUP => Sig::HUP,
        libc::SIGQUIT => Sig::QUIT,
        libc::SIGABRT => Sig::ABRT,
        libc::SIGSEGV => Sig::SEGV,
        libc::SIGPIPE => Sig::PIPE,
        libc::SIGUSR1 => Sig::USR1,
        libc::SIGALRM => Sig::ALRM,
        libc::SIGFPE => Sig::FPE,
        libc::SIGILL => Sig::ILL,
        other => Sig::Custom(format!("{other}")),
    }
}

fn kill_group(pgid: &Mutex<Option<u32>>, signal: i32) {
    if let Some(pid) = *pgid.lock().unwrap() {
        unsafe { libc::killpg(pid as libc::pid_t, signal) };
    }
}

/// How a command ended.
enum Ended {
    Status(u32),
    Signal { sig: Sig, core: bool },
}

/// A channel's terminal: its size, and the PTY's master side once something runs on it.
#[derive(Default)]
struct PtyState {
    cols: u32,
    rows: u32,
    master: Option<i32>,
}

fn set_size(fd: i32, cols: u32, rows: u32) {
    let ws = libc::winsize { ws_row: rows as u16, ws_col: cols as u16, ws_xpixel: 0, ws_ypixel: 0 };
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
}

/// One command on one channel.
struct Job {
    node: Node,
    channel: ChannelId,
    raw: String,
    logical: String,
    sudo_prompt: Option<String>,
    pty: Option<Arc<Mutex<PtyState>>>,
    shell: bool,
    write: Arc<ChannelWriteHalf<Msg>>,
    handle: server::Handle,
    pgid: Arc<Mutex<Option<u32>>>,
    /// The client closed the channel: the command's output pipes are closed from here on.
    closed: CancelToken,
}

impl Job {
    fn command(&self, text: &str) -> tokio::process::Command {
        let node = &self.node;
        let mut c = tokio::process::Command::new(DASH);
        if self.shell {
            // No job control: on this Mac dash's `tcsetpgrp` for a new foreground job sometimes
            // fails under load, "Cannot set tty process group (Invalid argument)", and the job dies.
            c.arg("-i").arg("+m");
        } else {
            c.arg("-c").arg(text);
        }
        // A login environment, not the test runner's: locale-sensitive commands show it.
        c.env_clear()
            .env("HOME", &node.home)
            .env("PATH", format!("{}:{}", node.bin_dir.display(), std::env::var("PATH").unwrap_or_default()))
            .env("USER", "sparkuser")
            .env("LOGNAME", "sparkuser")
            .env("SHELL", "/bin/sh")
            .env("LANG", "en_US.UTF-8")
            .env("COPYFILE_DISABLE", "1")
            .env("BASH_SILENCE_DEPRECATION_WARNING", "1")
            .env("SB_FAKE_STATE", &node.state_dir)
            .env("SB_FAKE_SUDO_PASSWORD", node.knobs.sudo_password.lock().unwrap().clone())
            .env("SB_FAKE_NOPASSWD", if node.knobs.nopasswd.load(Ordering::SeqCst) { "1" } else { "0" });
        if !node.path_first.is_empty() {
            let first: Vec<String> = node.path_first.iter().map(|p| p.display().to_string()).collect();
            c.env("PATH", format!("{}:{}:{}", first.join(":"), node.bin_dir.display(), std::env::var("PATH").unwrap_or_default()));
        }
        for (k, v) in node.env.iter() {
            c.env(k, v);
        }
        if let Ok(t) = std::env::var("TMPDIR") {
            c.env("TMPDIR", t);
        }
        c.current_dir(&node.home).kill_on_drop(true);
        c
    }

    /// Runs on a real pseudo-terminal: the child is a session leader with the PTY as its
    /// controlling terminal; its output (stdout and stderr alike) and the echo come from the master.
    async fn run_pty(&self, text: &str, mut input: mpsc::UnboundedReceiver<Vec<u8>>, record: Arc<Mutex<Vec<u8>>>) -> Ended {
        use std::os::fd::{FromRawFd, OwnedFd};
        let state = self.pty.clone().unwrap();
        let (cols, rows) = {
            let s = state.lock().unwrap();
            (s.cols.max(2), s.rows.max(2))
        };
        let (mut master, mut slave) = (0, 0);
        let mut ws = libc::winsize { ws_row: rows as u16, ws_col: cols as u16, ws_xpixel: 0, ws_ypixel: 0 };
        if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) } != 0 {
            let _ = self.write.data(&b"openpty failed\r\n"[..]).await;
            return Ended::Status(1);
        }
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        let mut cmd = self.command(text);
        cmd.env("TERM", "xterm-256color")
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
        let spawned = cmd.spawn();
        drop(cmd); // our copies of the slave side: the master reads EOF once the child's are gone
        let Ok(mut child) = spawned else { return Ended::Status(127) };
        *self.pgid.lock().unwrap() = child.id();
        use std::os::fd::AsRawFd;
        state.lock().unwrap().master = Some(master.as_raw_fd());
        let mut reader = tokio::fs::File::from_std(std::fs::File::from(master.try_clone().unwrap()));
        let mut writer = tokio::fs::File::from_std(std::fs::File::from(master.try_clone().unwrap()));
        let feeder = tokio::spawn(async move {
            while let Some(chunk) = input.recv().await {
                record.lock().unwrap().extend_from_slice(&chunk);
                if writer.write_all(&chunk).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
            }
        });
        let (write, knobs) = (self.write.clone(), self.node.knobs.clone());
        let out = tokio::spawn(async move {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break, // EIO once the terminal has no process left
                    Ok(n) => {
                        knobs.until_thawed().await;
                        if write.data(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let status = child.wait().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), out).await;
        feeder.abort();
        state.lock().unwrap().master = None;
        *self.pgid.lock().unwrap() = None;
        drop(master);
        match status {
            Ok(s) => match (s.code(), s.signal()) {
                (Some(code), _) => Ended::Status(code as u32),
                (None, Some(n)) => Ended::Signal { sig: sig_of(n), core: s.core_dumped() },
                _ => Ended::Status(1),
            },
            Err(_) => Ended::Status(1),
        }
    }

    /// Runs `text` for real: stdin streams in from `input`, output streams out as produced.
    /// With `capture`, stdout is kept instead of sent (the sudo relay). Returns how it ended
    /// and the captured stdout.
    async fn run_real(&self, text: &str, mut input: mpsc::UnboundedReceiver<Vec<u8>>, record: Arc<Mutex<Vec<u8>>>, capture: bool) -> (Ended, Vec<u8>) {
        let mut cmd = self.command(text);
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = self.write.extended_data(1, format!("sh: {e}\n").as_bytes()).await;
                return (Ended::Status(127), Vec::new());
            }
        };
        *self.pgid.lock().unwrap() = child.id();
        let kill = {
            let mut k = self.node.knobs.kill.lock().unwrap();
            match k.as_ref() {
                Some((m, _)) if self.logical.contains(m.as_str()) => k.take().map(|(_, s)| signal_number(&s)),
                _ => None,
            }
        };
        let fed = Arc::new(tokio::sync::Notify::new());
        let mut stdin = child.stdin.take();
        let (rec, fed2) = (record.clone(), fed.clone());
        let feeder = tokio::spawn(async move {
            while let Some(chunk) = input.recv().await {
                rec.lock().unwrap().extend_from_slice(&chunk);
                if let Some(pipe) = stdin.as_mut() {
                    if pipe.write_all(&chunk).await.is_err() {
                        stdin = None; // the command stopped reading; keep recording what arrives
                    }
                }
                fed2.notify_one();
            }
            // The client's EOF: the command's stdin ends too.
        });
        if let Some(signal) = kill {
            let (pgid, fed) = (self.pgid.clone(), fed.clone());
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_millis(200), fed.notified()).await;
                kill_group(&pgid, signal);
            });
        }
        // Output goes out while the command runs, as it's produced.
        let frames = if is_collector(&self.logical) { self.node.knobs.take_freeze() } else { None };
        let gate = Gate { knobs: self.node.knobs.clone(), frames_left: frames, closed: self.closed.clone() };
        let out = tokio::spawn(forward(child.stdout.take().unwrap(), self.write.clone(), None, capture, gate.clone()));
        let err = tokio::spawn(forward(child.stderr.take().unwrap(), self.write.clone(), Some(1), false, Gate { frames_left: None, ..gate }));
        let status = child.wait().await;
        let (captured, _) = tokio::join!(out, err);
        let captured = captured.unwrap_or_default();
        *self.pgid.lock().unwrap() = None;
        // Give stdin that was already on its way a moment to land in the record, then stop.
        tokio::time::sleep(Duration::from_millis(5)).await;
        feeder.abort();
        let ended = match status {
            Ok(s) => match (s.code(), s.signal()) {
                (Some(code), _) => Ended::Status(code as u32),
                (None, Some(n)) => Ended::Signal { sig: sig_of(n), core: s.core_dumped() },
                _ => Ended::Status(1),
            },
            Err(_) => Ended::Status(1),
        };
        (ended, captured)
    }

    async fn send_answer(&self, out: &str) {
        // A script reply starting with "!stderr:" goes to stderr instead.
        match out.strip_prefix("!stderr:") {
            Some(err) => {
                let _ = self.write.extended_data(1, err.as_bytes()).await;
            }
            None => {
                for chunk in out.as_bytes().chunks(16 * 1024) {
                    let _ = self.write.data(chunk).await;
                }
            }
        }
    }

    async fn run(self, input: mpsc::UnboundedReceiver<Vec<u8>>) {
        let slow = self.node.knobs.slow_ms.load(Ordering::SeqCst);
        if slow > 0 {
            tokio::time::sleep(Duration::from_millis(slow)).await;
        }
        let record: Arc<Mutex<Vec<u8>>> = Arc::default();
        let scripted = if self.pty.is_some() || self.shell { None } else { (self.node.script)(&self.logical, b"") };
        let ended = match (scripted, &self.sudo_prompt) {
            (None, _) if self.pty.is_some() => self.run_pty(&self.raw, input, record.clone()).await,
            (None, _) => self.run_real(&self.raw, input, record.clone(), false).await.0,
            (Some((answer, code)), None) => {
                // A stand-in: it reads stdin to the end like the real command would, then answers.
                let mut input = input;
                while let Some(chunk) = input.recv().await {
                    record.lock().unwrap().extend_from_slice(&chunk);
                }
                self.send_answer(&answer).await;
                Ended::Status(code)
            }
            (Some(_), Some(prompt)) => {
                // sudo's password exchange is the shim's; what it passes on is the command's stdin.
                let relay = format!("sudo -k -S -p {} -- cat", crate::live::sh_quote(prompt));
                let (ended, rest) = self.run_real(&relay, input, record.clone(), true).await;
                match ended {
                    Ended::Status(0) => {
                        let stdin = record.lock().unwrap().clone();
                        match (self.node.script)(&self.logical, &stdin) {
                            Some((answer, code)) => {
                                self.send_answer(&answer).await;
                                Ended::Status(code)
                            }
                            None => {
                                let inner = self.logical.trim_start_matches("sudo -k -S -p '' ").to_string();
                                let (tx, rx) = mpsc::unbounded_channel();
                                let _ = tx.send(rest);
                                drop(tx);
                                self.run_real(&inner, rx, Arc::default(), false).await.0
                            }
                        }
                    }
                    other => other,
                }
            }
        };
        let stdin = record.lock().unwrap().clone();
        if self.logical.contains("sparkjob.py start") {
            let body = stdin.splitn(2, |&b| b == b'\n').last().filter(|_| self.sudo_prompt.is_some()).unwrap_or(&stdin).to_vec();
            self.node.specs.lock().unwrap().push(serde_json::from_slice(&body).unwrap_or(Value::Null));
        }
        self.node.stdin.lock().unwrap().push((self.logical.clone(), stdin));
        if Node::take_once(&self.node.knobs.no_status, &self.logical) {
            let _ = self.handle.close(self.channel).await;
            return;
        }
        match ended {
            Ended::Status(code) => {
                let _ = self.handle.exit_status_request(self.channel, code).await;
            }
            Ended::Signal { sig, core } => {
                let _ = self.handle.exit_signal_request(self.channel, sig, core, String::new(), "en".into()).await;
            }
        }
        let _ = self.write.eof().await;
        let _ = self.write.close().await;
    }
}

/// The collector: the real one (the Spark profile) or the canned frames.
fn is_collector(logical: &str) -> bool {
    logical.contains("python3 -u") && logical.contains("collector.py")
}

/// What holds a channel's output back: a hung node (until it thaws), a collector frozen
/// after so many frames (it keeps running; what it prints after that is dropped), and the
/// client closing the channel (the pipe is closed then, as sshd closes it).
#[derive(Clone)]
struct Gate {
    knobs: Arc<Knobs>,
    frames_left: Option<usize>,
    closed: CancelToken,
}

/// Copies a child's pipe to the channel as it's produced, waiting for the client's window.
/// With `keep`, the bytes are returned instead of sent. Once the client has closed the channel
/// the pipe is let go of, so the command's next write fails (SIGPIPE) as it does under sshd.
async fn forward(mut pipe: impl tokio::io::AsyncRead + Unpin, write: Arc<ChannelWriteHalf<Msg>>, ext: Option<u32>, keep: bool, mut gate: Gate) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let n = tokio::select! {
            read = pipe.read(&mut buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            },
            _ = gate.closed.cancelled() => break,
        };
        gate.knobs.until_thawed().await;
        if keep {
            kept.extend_from_slice(&buf[..n]);
            continue;
        }
        let mut chunk = &buf[..n];
        if let Some(left) = gate.frames_left.as_mut() {
            if *left == 0 {
                continue; // frozen
            }
            // Up to the end of the last frame it may still send.
            let mut end = 0;
            while *left > 0 {
                match chunk[end..].iter().position(|&b| b == b'\n') {
                    Some(i) => {
                        end += i + 1;
                        *left -= 1;
                    }
                    None => {
                        end = chunk.len();
                        break;
                    }
                }
            }
            chunk = &chunk[..end];
        }
        let sent = match ext {
            Some(e) => write.extended_data(e, chunk).await,
            None => write.data(chunk).await,
        };
        if sent.is_err() {
            break;
        }
    }
    kept
}

struct Running {
    task: tokio::task::JoinHandle<()>,
    pgid: Arc<Mutex<Option<u32>>>,
    /// It runs on a terminal (a PTY session).
    pty: bool,
    closed: CancelToken,
}

struct Conn {
    node: Node,
    /// Session channels waiting for their exec or shell request.
    channels: HashMap<ChannelId, Channel<Msg>>,
    /// Channels with a PTY.
    ptys: HashMap<ChannelId, Arc<Mutex<PtyState>>>,
    /// stdin of each running command; dropping the sender is the client's EOF.
    inputs: HashMap<ChannelId, mpsc::UnboundedSender<Vec<u8>>>,
    running: Arc<Mutex<HashMap<ChannelId, Running>>>,
    collectors: Arc<Mutex<HashSet<ChannelId>>>,
    /// Session channels open on this connection: their places under MaxSessions.
    slots: Arc<Mutex<HashSet<ChannelId>>>,
    registered: bool,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.node.knobs.open_connections.fetch_sub(1, Ordering::SeqCst);
        let n = self.collectors.lock().unwrap().drain().count();
        self.node.knobs.collectors.fetch_sub(n, Ordering::SeqCst);
        // The connection is gone: like sshd, hang up on what it was running.
        for (_, r) in self.running.lock().unwrap().drain() {
            kill_group(&r.pgid, libc::SIGHUP);
            kill_group(&r.pgid, libc::SIGTERM);
            r.task.abort();
        }
    }
}

impl Conn {
    fn register(&mut self, session: &Session) {
        if !self.registered {
            self.registered = true;
            self.node.knobs.handles.lock().unwrap().push(session.handle());
        }
    }

    /// Starts a command (or a shell) on a channel, in the background.
    fn start(&mut self, channel: ChannelId, raw: String, shell: bool, session: &Session) {
        let Some(chan) = self.channels.remove(&channel) else { return };
        let (_read, write) = chan.split();
        let (logical, sudo_prompt) = if shell { ("login shell".to_string(), None) } else { logical(&raw) };
        let (tx, rx) = mpsc::unbounded_channel();
        self.inputs.insert(channel, tx);
        let pgid: Arc<Mutex<Option<u32>>> = Arc::default();
        let closed = CancelToken::new();
        let pty = self.ptys.get(&channel).cloned();
        let on_pty = pty.is_some();
        let job = Job {
            node: self.node.clone(),
            channel,
            raw,
            logical,
            sudo_prompt,
            pty,
            shell,
            write: Arc::new(write),
            handle: session.handle(),
            pgid: pgid.clone(),
            closed: closed.clone(),
        };
        let (running, slots) = (self.running.clone(), self.slots.clone());
        let task = tokio::spawn(async move {
            job.run(rx).await;
            running.lock().unwrap().remove(&channel);
            // The command has ended and the channel is closed: its place is free again.
            slots.lock().unwrap().remove(&channel);
        });
        self.running.lock().unwrap().insert(channel, Running { task, pgid, pty: on_pty, closed });
    }

    /// The client signalled or closed the channel: the command gets the signal (it then reports it).
    fn signal_job(&self, channel: ChannelId, signal: i32) {
        if let Some(r) = self.running.lock().unwrap().get(&channel) {
            kill_group(&r.pgid, signal);
        }
    }
}

impl server::Handler for Conn {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if self.node.knobs.drop_at_login.swap(false, Ordering::SeqCst) {
            return Err(russh::Error::Disconnect);
        }
        let ok = user == "sparkuser" && self.node.knobs.password.lock().unwrap().as_deref() == Some(password);
        if ok {
            self.node.knobs.logins.lock().unwrap().push("password".into());
        }
        Ok(if ok { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey_offered(&mut self, user: &str, offer: server::PublicKeyOffer<'_>) -> Result<Auth, Self::Error> {
        if self.node.knobs.drop_at_login.swap(false, Ordering::SeqCst) {
            return Err(russh::Error::Disconnect);
        }
        Ok(if user == "sparkuser" && self.node.key_allowed(offer.public_key) { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey(&mut self, user: &str, key: server::VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        let known = self.node.key_allowed(key.public_key);
        if user == "sparkuser" && known {
            self.node.knobs.logins.lock().unwrap().push("publickey".into());
            return Ok(Auth::Accept);
        }
        Ok(Auth::reject())
    }

    async fn channel_open_session(&mut self, channel: Channel<Msg>, reply: server::ChannelOpenHandle, session: &mut Session) -> Result<(), Self::Error> {
        self.register(session);
        // A frozen node answers nothing, this included (and nothing else on the connection meanwhile).
        self.node.knobs.until_thawed().await;
        let knobs = &self.node.knobs;
        let full = knobs.max_sessions.lock().unwrap().is_some_and(|max| self.slots.lock().unwrap().len() >= max);
        let forced = !full && knobs.refuse_next.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
        if full || forced {
            // What sshd answers past MaxSessions (OpenSSH 9.6: SSH_OPEN_CONNECT_FAILED, "no more
            // sessions"; live/staging_tests.rs checks it against the real one).
            knobs.refused.fetch_add(1, Ordering::SeqCst);
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        }
        let open = {
            let mut slots = self.slots.lock().unwrap();
            slots.insert(channel.id());
            slots.len()
        };
        knobs.peak_sessions.fetch_max(open, Ordering::SeqCst);
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    /// Port forwards really connect, from "the node" (this machine) to the target. As sshd does,
    /// the channel opens once the connection is made, and a target that refuses it gets an
    /// open failure ("connect failed"), not an open channel that closes.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _originator: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.register(session);
        self.node.log.lock().unwrap().push(format!("direct-tcpip {host}:{port}"));
        let target = format!("{host}:{port}");
        tokio::spawn(async move {
            match tokio::net::TcpStream::connect(&target).await {
                Ok(mut tcp) => {
                    reply.accept().await;
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
                }
                Err(_) => reply.reject(russh::ChannelOpenFailure::ConnectFailed).await,
            }
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.node.log.lock().unwrap().push(format!("pty {term} {cols}x{rows}"));
        self.ptys.insert(channel, Arc::new(Mutex::new(PtyState { cols, rows, master: None })));
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(&mut self, channel: ChannelId, cols: u32, rows: u32, _pw: u32, _ph: u32, _session: &mut Session) -> Result<(), Self::Error> {
        self.node.log.lock().unwrap().push(format!("resize {cols}x{rows}"));
        if let Some(p) = self.ptys.get(&channel) {
            let mut p = p.lock().unwrap();
            (p.cols, p.rows) = (cols, rows);
            if let Some(fd) = p.master {
                set_size(fd, cols, rows);
            }
        }
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        self.node.log.lock().unwrap().push("shell".into());
        session.channel_success(channel)?;
        self.start(channel, String::new(), true, session);
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        self.node.knobs.until_thawed().await;
        let raw = String::from_utf8_lossy(data).to_string();
        let (cmd, _) = logical(&raw);
        self.node.raw.lock().unwrap().push(raw.clone());
        self.node.log.lock().unwrap().push(cmd.clone());
        session.channel_success(channel)?;
        if is_collector(&cmd) && !self.node.knobs.real_collector.load(Ordering::SeqCst) {
            // The collector: frames until the channel closes; none while stalled or frozen.
            self.channels.remove(&channel);
            self.collectors.lock().unwrap().insert(channel);
            self.node.knobs.collectors.fetch_add(1, Ordering::SeqCst);
            let (knobs, h, open) = (self.node.knobs.clone(), session.handle(), self.collectors.clone());
            let mut left = knobs.take_freeze();
            tokio::spawn(async move {
                loop {
                    if !open.lock().unwrap().contains(&channel) {
                        return;
                    }
                    knobs.until_thawed().await;
                    let line = knobs.frame.lock().unwrap().clone().unwrap_or_else(frame);
                    if left != Some(0) && !knobs.stall_frames.load(Ordering::SeqCst) {
                        if h.data(channel, format!("{line}\n")).await.is_err() {
                            return;
                        }
                        left = left.map(|n| n - 1);
                    }
                    tokio::time::sleep(Duration::from_millis(knobs.frame_every_ms.load(Ordering::SeqCst).max(20))).await;
                }
            });
            return Ok(());
        }
        self.start(channel, raw, false, session);
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _session: &mut Session) -> Result<(), Self::Error> {
        if let Some(tx) = self.inputs.get(&channel) {
            let _ = tx.send(data.to_vec());
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        self.inputs.remove(&channel);
        Ok(())
    }

    async fn signal(&mut self, channel: ChannelId, signal: Sig, _session: &mut Session) -> Result<(), Self::Error> {
        let name = format!("{signal:?}");
        self.node.knobs.signals.lock().unwrap().push(name.clone());
        self.signal_job(channel, signal_number(&name));
        if self.collectors.lock().unwrap().remove(&channel) {
            self.node.knobs.collectors.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }

    async fn channel_close(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        self.inputs.remove(&channel);
        self.channels.remove(&channel);
        self.ptys.remove(&channel);
        // A channel with nothing running is done; one whose command still runs keeps its place until it ends.
        if !self.running.lock().unwrap().contains_key(&channel) {
            self.slots.lock().unwrap().remove(&channel);
        }
        // As sshd does: a PTY session is hung up (its terminal goes away); a plain command runs on,
        // its input ended above and its output closed, until it reads EOF or writes to the closed
        // pipe. SparkBox sends TERM itself before it lets go of one still running.
        if let Some(r) = self.running.lock().unwrap().get(&channel) {
            if r.pty {
                kill_group(&r.pgid, libc::SIGHUP);
            }
            r.closed.cancel();
        }
        if self.collectors.lock().unwrap().remove(&channel) {
            self.node.knobs.collectors.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

// ---- Shims on the node's PATH

const SUDO: &str = r#"#!/bin/dash
# sudo -k -S -p <prompt> [--] command...: asks on stderr, reads one line of stdin, then runs
# the command (as the same user: the fake has no root). As real sudo 1.9 does: root (a sudo
# inside a command that already runs as root) is never asked; without -S and with no terminal
# (an SSH command) there's nowhere to ask, so it refuses (-n: it never asks).
prompt="Password: "
stdin=0
never=0
while [ $# -gt 0 ]; do
  case "$1" in
    -S) stdin=1; shift ;;
    -n) never=1; shift ;;
    -k|-E|-H) shift ;;
    -p) prompt="$2"; shift 2 ;;
    -u) shift 2 ;;
    --) shift; break ;;
    -*) echo "sudo: unknown option -- ${1#-}" >&2; exit 1 ;;
    *) break ;;
  esac
done
if [ "$SB_FAKE_ROOT" = 1 ]; then exec "$@"; fi
if [ "$SB_FAKE_NOPASSWD" != 1 ] && [ "$never" = 1 ]; then echo "sudo: a password is required" >&2; exit 1; fi
if [ "$SB_FAKE_NOPASSWD" != 1 ] && [ "$stdin" = 0 ] && [ ! -t 0 ]; then
  echo "sudo: a terminal is required to read the password; either use the -S option to read from standard input or configure an askpass helper" >&2
  echo "sudo: a password is required" >&2
  exit 1
fi
if [ "$SB_FAKE_NOPASSWD" != 1 ]; then
  tries=0
  while :; do
    printf '%s' "$prompt" >&2
    if ! IFS= read -r line; then echo "sudo: no password was provided" >&2; exit 1; fi
    [ "$line" = "$SB_FAKE_SUDO_PASSWORD" ] && break
    tries=$((tries + 1))
    if [ "$tries" -ge 3 ]; then echo "sudo: 3 incorrect password attempts" >&2; exit 1; fi
    echo "Sorry, try again." >&2
  done
fi
SB_FAKE_ROOT=1 exec "$@"
"#;

const SYSTEMD_RUN: &str = r#"#!/usr/bin/env python3
# systemd-run --unit NAME [--collect] [-p StandardOutput=append:F] [-p StandardError=append:F] cmd...
# Runs cmd detached from the SSH session, like a transient service, keeping its state where the
# systemctl shim reads it. --collect forgets the unit once it ends, as systemd does.
import os, sys
args = sys.argv[1:]
unit, out, err, collect = None, os.devnull, os.devnull, False
while args:
    a = args[0]
    if a == "--unit": unit, args = args[1], args[2:]
    elif a.startswith("--unit="): unit, args = a[7:], args[1:]
    elif a in ("-p", "--property"):
        k, _, v = args[1].partition("=")
        if k == "StandardOutput" and v.startswith("append:"): out = v[7:]
        if k == "StandardError" and v.startswith("append:"): err = v[7:]
        args = args[2:]
    elif a == "--collect": collect, args = True, args[1:]
    elif a == "--": args = args[1:]; break
    elif a.startswith("-"): args = args[1:]
    else: break
if not args:
    sys.exit("systemd-run: no command given")
# The unit's /bin/sh is the node's: dash (this Mac's /bin/sh is bash 3.2).
if args[0] == "/bin/sh":
    args[0] = "/bin/dash"
unit = unit or "run-u%d" % os.getpid()
if not unit.endswith(".service"): unit += ".service"
units = os.path.join(os.environ["SB_FAKE_STATE"], "units")
os.makedirs(units, exist_ok=True)
path = os.path.join(units, unit)
def write(**kv):
    with open(path + ".tmp", "w") as f:
        f.write("".join("%s=%s\n" % kv_ for kv_ in kv.items()))
    os.replace(path + ".tmp", path)
try:
    if "ActiveState=active" in open(path).read():
        sys.exit("Failed to start transient service unit: Unit %s was already loaded or has a fragment file." % unit)
except OSError:
    pass
for f in (out, err):
    if f != os.devnull:
        os.makedirs(os.path.dirname(f), exist_ok=True)
write(ActiveState="activating", Result="success", MainPID=0)
if os.fork() == 0:
    os.setsid()
    if os.fork():
        os._exit(0)
    fd_in = os.open(os.devnull, os.O_RDONLY)
    fd_out = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    fd_err = os.open(err, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    pid = os.fork()
    if pid == 0:
        # The unit's processes form one group, so `systemctl kill` reaches all of them.
        os.setpgid(0, 0)
        os.dup2(fd_in, 0); os.dup2(fd_out, 1); os.dup2(fd_err, 2)
        os.execvp(args[0], args)
    for fd in (0, 1, 2):
        os.dup2(fd_in, fd)
    write(ActiveState="active", Result="success", MainPID=pid)
    _, status = os.waitpid(pid, 0)
    if collect:
        os.remove(path)
    elif os.WIFSIGNALED(status):
        write(ActiveState="failed", Result="signal", MainPID=0, ExecMainStatus=os.WTERMSIG(status))
    else:
        code = os.WEXITSTATUS(status)
        write(ActiveState="inactive" if code == 0 else "failed", Result="success" if code == 0 else "exit-code", MainPID=0, ExecMainStatus=code)
    os._exit(0)
import time
for _ in range(200):
    try:
        if "ActiveState=activating" not in open(path).read():
            break
    except OSError:
        break
    time.sleep(0.01)
print("Running as unit: %s; invocation ID: 6f1b0c2d9e8a4f7b8c3d2e1f0a9b8c7d" % unit, file=sys.stderr)
"#;

const SYSTEMCTL: &str = r#"#!/usr/bin/env python3
# systemctl show -p A,B [--value] UNIT | kill [--signal=S] UNIT | stop UNIT | is-active UNIT;
# anything else is noted and succeeds.
import os, signal, sys
args = [a for a in sys.argv[1:] if a not in ("--no-pager", "--quiet", "-q")]
units = os.path.join(os.environ["SB_FAKE_STATE"], "units")
def unit_name(u): return u if "." in u else u + ".service"
def load(u):
    try:
        return dict(l.rstrip("\n").split("=", 1) for l in open(os.path.join(units, unit_name(u))) if "=" in l)
    except OSError:
        return {"ActiveState": "inactive", "Result": "success", "LoadState": "not-found", "MainPID": "0"}
if not args:
    sys.exit(0)
# `--` ends the options, as for the real one (SparkBox writes `systemctl show -p … -- UNIT`).
verb, rest = args[0], [a for a in args[1:] if a != "--"]
if any(a.split(".")[0] in ("ssh", "sshd") for a in rest if not a.startswith("-")):
    # sshd stopped (stop, kill, or disable/mask --now): the fake refuses new connections and
    # open ones carry on, as on a real node; start or restart brings it back.
    flag = os.path.join(os.environ["SB_FAKE_STATE"], "sshd-stopped")
    if verb in ("stop", "kill") or (verb in ("disable", "mask") and "--now" in rest):
        open(flag, "w").close()
    elif verb in ("start", "restart") and os.path.exists(flag):
        os.remove(flag)
if verb == "show":
    props, value, names = [], False, []
    i = 0
    while i < len(rest):
        a = rest[i]
        if a in ("-p", "--property"): props += rest[i + 1].split(","); i += 2; continue
        if a.startswith("--property="): props += a[11:].split(",")
        elif a == "--value": value = True
        else: names.append(a)
        i += 1
    st = load(names[0] if names else "")
    st.setdefault("LoadState", "loaded")
    for p in props or sorted(st):
        print(st.get(p, "") if value else "%s=%s" % (p, st.get(p, "")))
elif verb in ("kill", "stop"):
    sig, names = signal.SIGTERM, []
    for a in rest:
        if a.startswith("--signal="): sig = getattr(signal, "SIG" + a[9:].upper().replace("SIG", ""))
        elif a.startswith("-s"): pass
        elif not a.startswith("-"): names.append(a)
    for n in names:
        pid = int(load(n).get("MainPID", "0") or 0)
        if pid > 0:
            # Every process in the unit, as systemd's default --kill-whom=all does.
            try: os.killpg(pid, sig)
            except OSError: pass
elif verb == "is-active":
    st = load(rest[0] if rest else "")
    print(st["ActiveState"])
    sys.exit(0 if st["ActiveState"] == "active" else 3)
else:
    with open(os.path.join(os.environ["SB_FAKE_STATE"], "systemctl.log"), "a") as f:
        f.write(" ".join(sys.argv[1:]) + "\n")
"#;

const APT_GET: &str = r#"#!/bin/dash
# apt-get [-o …] [-s] [-y] install --only-upgrade PKG… | dist-upgrade | update | check: prints
# what the real one prints, one package at a time ($SB_FAKE_STATE/apt/delay seconds apart, 0.3
# by default), and records the upgrade. dist-upgrade upgrades what $SB_FAKE_STATE/apt/pending
# lists (one a line; libfoo and libbar when there's no such file), less what apt-mark holds
# ($SB_FAKE_STATE/apt/holds), which it keeps back as the real one does; what's upgraded leaves
# that list. With -s it only prints the plan, as the real dry run does, and changes nothing.
# Stops on TERM.
st="$SB_FAKE_STATE/apt"; mkdir -p "$st"
delay=$(cat "$st/delay" 2>/dev/null || echo 0.3)
cmd=""; pkgs=""; skip=0; sim=0
for a in "$@"; do
  if [ $skip = 1 ]; then skip=0; continue; fi
  case "$a" in
    -o) skip=1 ;;
    -s|--simulate|--dry-run|--just-print|--no-act) sim=1 ;;
    -*) ;;
    install|update|upgrade|dist-upgrade|check) [ -z "$cmd" ] && cmd="$a" ;;
    *) pkgs="$pkgs $a" ;;
  esac
done
trap 'echo "E: Sub-process /usr/bin/dpkg received a termination signal" >&2; exit 100' TERM
# A waited background sleep, so TERM ends it at once, as it does the real apt-get (a plain
# `sleep` would hold the trap until it finished).
pause() { sleep "$delay" & wait $!; }
case "$cmd" in
  update) echo "Hit:1 http://ports.ubuntu.com/ubuntu-ports noble InRelease"; echo "Reading package lists..."; exit 0 ;;
  check) echo "Reading package lists... Done"; echo "Building dependency tree... Done"; echo "Reading state information... Done"; exit 0 ;;
  dist-upgrade) if [ -f "$st/pending" ]; then pkgs=" $(tr '\n' ' ' < "$st/pending")"; else pkgs=" libfoo libbar"; fi ;;
  install) ;;
  *) echo "E: Invalid operation $cmd" >&2; exit 100 ;;
esac
# dist-upgrade keeps held packages back.
kept=""; k=0
if [ "$cmd" = dist-upgrade ] && [ -s "$st/holds" ]; then
  free=""
  for p in $pkgs; do
    if grep -qx -- "$p" "$st/holds"; then kept="$kept $p"; k=$((k + 1)); else free="$free $p"; fi
  done
  pkgs="$free"
fi
n=0; for p in $pkgs; do n=$((n + 1)); done
kept_back() { if [ -n "$kept" ]; then echo "The following packages have been kept back:"; echo " $kept"; fi; }
if [ $sim = 1 ]; then
  echo "NOTE: This is only a simulation!"
  echo "      apt-get needs root privileges for real execution."
  echo "      Keep also in mind that locking is deactivated,"
  echo "      so don't depend on the relevance to the real current situation!"
  echo "Reading package lists..."
  echo "Building dependency tree..."
  echo "Reading state information..."
  echo "Calculating upgrade..."
  kept_back
  echo "The following packages will be upgraded:"
  echo " $pkgs"
  echo "$n upgraded, 0 newly installed, 0 to remove and $k not upgraded."
  for p in $pkgs; do echo "Inst $p [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])"; done
  for p in $pkgs; do echo "Conf $p (2.0-1 Ubuntu:24.04/noble-updates [arm64])"; done
  exit 0
fi
echo "Reading package lists..."
echo "Building dependency tree..."
echo "Reading state information..."
kept_back
echo "The following packages will be upgraded:"
echo " $pkgs"
echo "$n upgraded, 0 newly installed, 0 to remove and $k not upgraded."
i=0
for p in $pkgs; do i=$((i + 1)); echo "Get:$i http://ports.ubuntu.com/ubuntu-ports noble-updates/main arm64 $p arm64 2.0-1 [118 kB]"; done
echo "Fetched $((n * 118)) kB in 1s (236 kB/s)"
# A test holds apt in its download, as a slow mirror would, until it removes this file.
while [ -f "$st/hold-download" ]; do sleep 0.02 & wait $!; done
pause
echo "(Reading database ... 214337 files and directories currently installed.)"
for p in $pkgs; do
  echo "Preparing to unpack .../${p}_2.0-1_arm64.deb ..."
  pause
  echo "Unpacking $p (2.0-1) over (1.0-1) ..."
  echo "$p" >> "$st/unpacked"
done
for p in $pkgs; do
  pause
  echo "Setting up $p (2.0-1) ..."
  echo "$p" >> "$st/upgraded"
done
if [ -f "$st/fail" ]; then cat "$st/fail" >&2; exit 100; fi
echo "Processing triggers for libc-bin (2.39-0ubuntu8.6) ..."
# What's upgraded is no longer pending.
if [ -f "$st/pending" ]; then
  for p in $pkgs; do grep -vx -- "$p" "$st/pending" > "$st/pending.tmp"; mv "$st/pending.tmp" "$st/pending"; done
fi
"#;

const DPKG: &str = r#"#!/bin/dash
# dpkg --audit: what $SB_FAKE_STATE/apt/audit says (nothing when all is well). As the real one
# does on dpkg 1.22 (staging finding 4), only root gets the list: the cluster user gets the lock
# error, exit 2 and nothing on stdout while a package isn't fully installed.
case "$1" in
  --audit|-C)
    if [ -s "$SB_FAKE_STATE/apt/audit" ] && [ "$SB_FAKE_ROOT" != 1 ]; then
      echo "dpkg: error: unable to check lock file for dpkg database directory /var/lib/dpkg: Permission denied" >&2
      exit 2
    fi
    cat "$SB_FAKE_STATE/apt/audit" 2>/dev/null; exit 0 ;;
  *) echo "dpkg: fake supports --audit only" >&2; exit 2 ;;
esac
"#;

const DPKG_QUERY: &str = r#"#!/usr/bin/env python3
# dpkg-query -W [-f FORMAT] [PKG…]: the package database in $SB_FAKE_STATE/apt/installed
# ("name<TAB>version" a line; libbar and libfoo at 1.0-1 when there's no such file). Without a
# format each package is "name<TAB>version", as the real one lists it; a format (-f, -fFMT,
# -f=FMT or --showformat=FMT, as dpkg's own option parser takes them) gets ${Package},
# ${Version} and the \n and \t escapes. A name that matches nothing: the real one's error.
import fnmatch, os, sys
path = os.path.join(os.environ["SB_FAKE_STATE"], "apt", "installed")
try:
    rows = [l.rstrip("\n").split("\t") for l in open(path) if l.strip()]
except OSError:
    rows = [["libbar", "1.0-1"], ["libfoo", "1.0-1"]]
args, fmt, names, show = sys.argv[1:], None, [], False
i = 0
while i < len(args):
    a = args[i]
    if a in ("-W", "--show"): show = True
    elif a == "-f": fmt, i = args[i + 1], i + 1
    elif a.startswith("--showformat="): fmt = a[len("--showformat="):]
    elif a.startswith("-f"): fmt = a[2:].removeprefix("=")
    elif a.startswith("-"): sys.exit("dpkg-query: error: unknown option %s" % a)
    else: names.append(a)
    i += 1
if not show:
    sys.exit("dpkg-query: the fake supports -W only")
fmt = (fmt if fmt is not None else "${Package}\t${Version}\n").replace("\\n", "\n").replace("\\t", "\t")
code = 0
for want in names or ["*"]:
    hit = [r for r in rows if fnmatch.fnmatchcase(r[0], want)]
    if not hit:
        print("dpkg-query: no packages found matching %s" % want, file=sys.stderr)
        code = 1
    for r in hit:
        sys.stdout.write(fmt.replace("${Package}", r[0]).replace("${Version}", r[1] if len(r) > 1 else ""))
sys.exit(code)
"#;

const PGREP: &str = r#"#!/bin/dash
# pgrep -x NAME, as on a machine of its own: the fake nodes and every test process share this
# Mac's process table, so only a process this node runs counts, one whose program is in the
# node's home (a test's stand-in dpkg); another test's, or another cargo run's, never does.
# Anything else is the real pgrep.
if [ $# = 2 ] && [ "$1" = -x ]; then
  exec /usr/bin/pgrep -f "^$HOME/$2( |\$)"
fi
exec /usr/bin/pgrep "$@"
"#;

const NEEDRESTART: &str = r#"#!/bin/dash
# needrestart -b -r l: the batch listing in $SB_FAKE_STATE/needrestart (the version and "the
# running kernel is the newest" when there's no such file). The fake only lists: any other
# restart mode, or without -b, it refuses.
batch=0; mode=""
while [ $# -gt 0 ]; do
  case "$1" in
    -b) batch=1; shift ;;
    -r) mode="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [ $batch != 1 ] || [ "$mode" != l ]; then echo "needrestart: the fake only lists (-b -r l)" >&2; exit 1; fi
if [ -f "$SB_FAKE_STATE/needrestart" ]; then cat "$SB_FAKE_STATE/needrestart"; else printf 'NEEDRESTART-VER: 3.6\nNEEDRESTART-KSTA: 1\n'; fi
"#;

const APT_MARK: &str = r#"#!/bin/dash
# apt-mark hold|unhold PKG… | showhold: the holds in $SB_FAKE_STATE/apt/holds, with the real
# one's words. Changing them needs root (the sudo shim), as dpkg's lock does; a package the
# database ($SB_FAKE_STATE/apt/installed, when there is one) doesn't have is the real error.
st="$SB_FAKE_STATE/apt"; mkdir -p "$st"; f="$st/holds"; touch "$f"
verb="$1"; [ $# -gt 0 ] && shift
case "$verb" in
  showhold) sort "$f"; exit 0 ;;
  hold|unhold) ;;
  *) echo "E: Invalid operation $verb" >&2; exit 100 ;;
esac
if [ "$SB_FAKE_ROOT" != 1 ]; then
  echo "E: Could not open lock file /var/lib/dpkg/lock-frontend - open (13: Permission denied)" >&2
  echo "E: Unable to acquire the dpkg frontend lock (/var/lib/dpkg/lock-frontend), are you root?" >&2
  exit 100
fi
for p in "$@"; do
  if [ -f "$st/installed" ] && ! cut -f1 "$st/installed" | grep -qx -- "$p"; then echo "E: Unable to locate package $p" >&2; exit 100; fi
done
for p in "$@"; do
  if [ "$verb" = hold ]; then
    if grep -qx -- "$p" "$f"; then echo "$p was already set on hold."; else echo "$p" >> "$f"; echo "$p set on hold."; fi
  elif grep -qx -- "$p" "$f"; then
    grep -vx -- "$p" "$f" > "$f.tmp"; mv "$f.tmp" "$f"; echo "Canceled hold on $p."
  else
    echo "$p was already not on hold."
  fi
done
"#;

const JOURNALCTL: &str = r#"#!/bin/dash
# journalctl [-u UNIT] [-f] [-n N] …: the lines in $SB_FAKE_STATE/journal/<unit> (or all.log).
unit=all; follow=0; n=""
while [ $# -gt 0 ]; do
  case "$1" in
    -u) unit="$2"; shift 2 ;;
    -n) n="$2"; shift 2 ;;
    -f) follow=1; shift ;;
    *) shift ;;
  esac
done
f="$SB_FAKE_STATE/journal/$unit"
mkdir -p "$SB_FAKE_STATE/journal"; touch "$f"
if [ $follow = 1 ]; then exec tail -n "${n:-10}" -f "$f"; fi
if [ -n "$n" ]; then tail -n "$n" "$f"; else cat "$f"; fi
"#;

const CHOWN: &str = r#"#!/bin/dash
# chown under the sudo shim: root's ownership can't be given without root, so it's noted only.
if [ "$SB_FAKE_ROOT" = 1 ]; then echo "chown $*" >> "$SB_FAKE_STATE/chown.log"; exit 0; fi
exec /usr/sbin/chown "$@"
"#;

const SSHD: &str = r#"#!/bin/dash
# sshd -t: checks the drop-in files under $SB_FAKE_STATE/sshd_config.d (an unknown keyword fails).
# sshd -T: the settings in force, as sshd decides them: drop-ins in name order, the first value
# of each keyword wins, passwords allowed when nothing says otherwise.
if [ "$1" = "-t" ]; then
  for f in "$SB_FAKE_STATE"/sshd_config.d/*.conf; do
    [ -e "$f" ] || continue
    if grep -qvE '^(PasswordAuthentication|KbdInteractiveAuthentication|PubkeyAuthentication) (yes|no)$' "$f"; then echo "$f: bad configuration option" >&2; exit 255; fi
  done
  exit 0
fi
if [ "$1" = "-T" ]; then
  pw=""
  for f in "$SB_FAKE_STATE"/sshd_config.d/*.conf; do
    [ -e "$f" ] || continue
    v=$(grep -i '^PasswordAuthentication ' "$f" | head -n 1 | cut -d' ' -f2)
    [ -z "$pw" ] && [ -n "$v" ] && pw="$v"
  done
  echo "port 22"
  echo "passwordauthentication ${pw:-yes}"
  echo "pubkeyauthentication yes"
  # OpenSSH's defaults for what the doctor asks (sshd -T prints every setting, lower case).
  echo "maxsessions 10"
  echo "maxstartups 10:30:100"
  echo "allowtcpforwarding yes"
  echo "disableforwarding no"
  exit 0
fi
echo "sshd: the fake only checks configs (-t, -T)" >&2; exit 1
"#;

const PING: &str = r#"#!/bin/dash
# ping on any fake node: nothing leaves this Mac, whose own ping must never see a fabric address.
# It answers as iputils does when the path carries the frame; models/fake_vllm.rs puts a knob on it.
addr=""
while [ $# -gt 0 ]; do
  case "$1" in
    -c|-W|-M|-s|-I) shift 2 ;;
    -*) echo "ping: invalid argument: '$1'" >&2; exit 2 ;;
    *) addr="$1"; shift ;;
  esac
done
[ -n "$addr" ] || { echo "ping: usage error: Destination address required" >&2; exit 1; }
printf 'PING %s (%s) 8972(9000) bytes of data.\n' "$addr" "$addr"
"#;

const ETHTOOL: &str = r#"#!/bin/dash
# ethtool IFACE: a Spark's LAN port, with Wake-on as $SB_FAKE_STATE/wol says (default d).
wol=$(cat "$SB_FAKE_STATE/wol" 2>/dev/null || echo d)
if [ "$1" = "-s" ]; then echo "${4:-d}" > "$SB_FAKE_STATE/wol"; exit 0; fi
printf 'Settings for %s:\n\tSpeed: 10000Mb/s\n\tSupports Wake-on: pumbg\n\tWake-on: %s\n\tLink detected: yes\n' "$1" "$wol"
"#;

/// The shell the nodes' `sh` is (dash 0.5.12, see the module notes); macOS ships a dash too. Every
/// shell shim, and every script `Node::bin` writes, runs under it.
pub const DASH: &str = "/bin/dash";

/// The shims; `spark`: one of the owner's Sparks, which answers only as the real node did, so
/// it gets no stand-in package database, needrestart or apt-mark (its own tree has apt-mark).
fn install_shims(bin: &Path, spark: bool) {
    // `sh` on the node's PATH is dash, so `sh -c` inside SparkBox's wrappers is dash as well.
    let _ = std::os::unix::fs::symlink(DASH, bin.join("sh"));
    for (name, body) in [
        ("sudo", SUDO),
        ("systemd-run", SYSTEMD_RUN),
        ("systemctl", SYSTEMCTL),
        ("apt-get", APT_GET),
        ("dpkg", DPKG),
        ("pgrep", PGREP),
        ("journalctl", JOURNALCTL),
        ("chown", CHOWN),
        ("ethtool", ETHTOOL),
        ("sshd", SSHD),
        ("ping", PING),
    ] {
        write_exec(&bin.join(name), body);
    }
    if !spark {
        for (name, body) in [("dpkg-query", DPKG_QUERY), ("needrestart", NEEDRESTART), ("apt-mark", APT_MARK)] {
            write_exec(&bin.join(name), body);
        }
    }
}

/// What a fake node is: a plain test node (scripts answer what needs a GB10), or one of the
/// owner's Sparks as the Live tour found it.
#[derive(Clone, Debug)]
pub struct Profile {
    /// cell1 | cell2 | cell3 | gx10-0000
    pub spark: Option<String>,
    /// sshd's MaxSessions: OpenSSH's default 10, as on the Sparks (None: no limit).
    pub max_sessions: Option<usize>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile { spark: None, max_sessions: Some(10) }
    }
}

impl Profile {
    pub fn plain() -> Profile {
        Profile::default()
    }

    /// The real node `name` (live/fixtures/doctor/<name>).
    pub fn spark(name: &str) -> Profile {
        Profile { spark: Some(name.to_string()), ..Profile::default() }
    }

    /// The host key the node presents: cell1, cell2 and cell3 share one (a cloned image).
    pub fn host_key(&self) -> &'static str {
        if self.spark.as_deref() == Some("gx10-0000") {
            &HOST_KEY_GX10
        } else {
            &HOST_KEY
        }
    }

    pub fn fingerprint(&self) -> String {
        use russh::keys::HashAlg;
        russh::keys::PrivateKey::from_openssh(self.host_key()).unwrap().public_key().fingerprint(HashAlg::Sha256).to_string()
    }
}

/// The Spark profile's fixtures.
pub fn doctor_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/live/fixtures/doctor")
}

pub async fn fake_node(home: &Path, script: Script) -> (Node, u16) {
    fake_node_as(home, script, Profile::plain()).await
}

/// One of the owner's Sparks (`Profile::spark`), answering only as the real node does.
pub async fn fake_spark(home: &Path, name: &str) -> (Node, u16) {
    fake_node_as(home, real_shell(), Profile::spark(name)).await
}

/// Builds the node from spark_tree.py's trees: (variables, folders first on PATH).
fn build_spark(name: &str, home: &Path, base_bin: &Path) -> (Vec<(String, String)>, Vec<PathBuf>, PathBuf) {
    let tree = PathBuf::from(format!("{}.spark", home.display()));
    unlock_tree(&tree);
    let _ = std::fs::remove_dir_all(&tree);
    let out = std::process::Command::new("python3").arg("-I").arg(doctor_dir().join("spark_tree.py")).arg(name).arg(&tree).output().unwrap();
    assert!(out.status.success(), "spark_tree.py {name}: {}", String::from_utf8_lossy(&out.stderr));
    let mut env: Vec<(String, String)> = std::fs::read_to_string(tree.join("env"))
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    env.push(("SB_FAKE_BASE_BIN".into(), base_bin.display().to_string()));
    (env, vec![tree.join("bin")], tree)
}

/// spark_tree.py makes other users' fd folders unreadable, as on a node: give them back to remove the tree.
fn unlock_tree(tree: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(entries) = std::fs::read_dir(tree) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
            unlock_tree(&p);
        }
    }
}

/// Docker's time format: "2026-10-26T22:20:24.000000000Z".
fn docker_time(t: u64) -> String {
    let (y, mo, d, h, mi, s, _) = crate::demo::ops::civil(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.000000000Z")
}

/// The node's Docker socket (collector.py reads containers from it, as on a Spark): its
/// containers, each container's inspect JSON and the engine version, from the fixtures.
async fn serve_docker(name: &str, sock: PathBuf) {
    let dir = doctor_dir().join(name);
    let facts: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("facts.json")).unwrap()).unwrap();
    let containers: Vec<Value> = facts["containers"].as_array().cloned().unwrap_or_default();
    let list: Vec<Value> = containers
        .iter()
        .map(|c| {
            serde_json::json!({
                "Id": c["fullId"].as_str().unwrap_or_default(), "Names": [format!("/{}", c["name"].as_str().unwrap_or_default())],
                "Image": c["image"], "State": c["state"], "Status": format!("Exited ({}) 2 days ago", c["exitCode"]),
                "Command": c["command"], "Labels": c["labels"], "Ports": [],
            })
        })
        .collect();
    let inspect = move |c: &Value| -> Value {
        let named = std::fs::read_to_string(dir.join(format!("inspect-{}.json", c["name"].as_str().unwrap_or_default())))
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
            .and_then(|mut v| v.pop());
        named.unwrap_or_else(|| {
            serde_json::json!({
                "Id": c["fullId"], "Name": format!("/{}", c["name"].as_str().unwrap_or_default()), "RestartCount": 0,
                "State": { "Status": c["state"], "ExitCode": c["exitCode"], "OOMKilled": false,
                           "StartedAt": docker_time(c["startedAt"].as_u64().unwrap_or(0)), "FinishedAt": docker_time(c["finishedAt"].as_u64().unwrap_or(0)) },
                "HostConfig": { "RestartPolicy": { "Name": "no" } }, "Config": { "Image": c["image"], "Env": [] },
            })
        })
    };
    let _ = std::fs::remove_file(&sock);
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    let version = facts["docker"].clone();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            let (list, containers, version) = (list.clone(), containers.clone(), version.clone());
            let inspect = inspect.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = conn.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = if path.starts_with("/containers/json") {
                    Some(Value::Array(list))
                } else if path == "/version" {
                    Some(serde_json::json!({ "Version": version }))
                } else if let Some(id) = path.strip_prefix("/containers/").and_then(|p| p.strip_suffix("/json")) {
                    containers.iter().find(|c| c["fullId"].as_str() == Some(id) || c["name"].as_str() == Some(id)).map(inspect)
                } else {
                    None
                };
                let (status, body) = match body {
                    Some(b) => ("200 OK", b.to_string()),
                    None => ("404 Not Found", r#"{"message":"page not found"}"#.to_string()),
                };
                let reply = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = conn.write_all(reply.as_bytes()).await;
            });
        }
    });
}

/// A fake node as `profile` says (see the module notes).
pub async fn fake_node_as(home: &Path, script: Script, profile: Profile) -> (Node, u16) {
    let bin_dir = PathBuf::from(format!("{}.fakebin", home.display()));
    let state_dir = PathBuf::from(format!("{}.fakestate", home.display()));
    for d in [&bin_dir, &state_dir] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }
    install_shims(&bin_dir, profile.spark.is_some());
    let (mut env, path_first) = match profile.spark.as_deref() {
        Some(name) => {
            let (env, first, _) = build_spark(name, home, &bin_dir);
            (env, first)
        }
        None => (Vec::new(), Vec::new()),
    };
    if let Some(name) = profile.spark.as_deref() {
        // A short path: a Unix socket's must fit in 104 bytes on macOS.
        let sock = std::env::temp_dir().join(format!("sbd-{}-{}.sock", std::process::id(), socket_number()));
        serve_docker(name, sock.clone()).await;
        env.push(("SPARKBOX_DOCKER_SOCK".into(), sock.display().to_string()));
    }
    let node = Node {
        home: home.into(),
        log: Arc::default(),
        raw: Arc::default(),
        specs: Arc::default(),
        stdin: Arc::default(),
        knobs: Arc::default(),
        bin_dir,
        state_dir,
        env: Arc::new(env),
        path_first: Arc::new(path_first),
        script,
    };
    node.knobs.real_collector.store(profile.spark.is_some(), Ordering::SeqCst);
    *node.knobs.max_sessions.lock().unwrap() = profile.max_sessions;
    let config = Arc::new(server::Config {
        keys: vec![russh::keys::PrivateKey::from_openssh(profile.host_key()).unwrap()],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let n = node.clone();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            // `systemctl stop ssh` ran: no new logins.
            if n.state_dir.join("sshd-stopped").exists() {
                drop(sock);
                continue;
            }
            n.knobs.open_connections.fetch_add(1, Ordering::SeqCst);
            n.knobs.total_connections.fetch_add(1, Ordering::SeqCst);
            let conn = Conn {
                node: n.clone(),
                channels: HashMap::new(),
                ptys: HashMap::new(),
                inputs: HashMap::new(),
                running: Arc::default(),
                collectors: Arc::default(),
                slots: Arc::default(),
                registered: false,
            };
            let (cfg, knobs) = (config.clone(), n.knobs.clone());
            tokio::spawn(async move {
                // A frozen node takes the TCP connection and says nothing until it thaws.
                knobs.until_thawed().await;
                if let Ok(session) = server::run_stream(cfg, sock, conn).await {
                    let _ = session.await;
                }
            });
        }
    });
    (node, port)
}

/// A different number for each Docker socket this process makes.
fn socket_number() -> usize {
    static N: AtomicUsize = AtomicUsize::new(0);
    N.fetch_add(1, Ordering::SeqCst)
}

pub fn real_shell() -> Script {
    Arc::new(|_, _| None)
}

/// Wait for an observable fixture transition, not an assumed process-start duration. The
/// deadline bounds a broken test; it is not a model readiness or production SSH timeout.
/// Keep diagnostics lazy so successful polls don't spawn more processes or dump audit logs.
pub async fn wait_for<T>(what: &str, mut observe: impl FnMut() -> Option<T>, diagnose: impl FnOnce() -> String) -> T {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(value) = observe() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.unwrap_or_else(|_| panic!("Timed out waiting for {what}: {}", diagnose()))
}

/// A fresh, real (symlink-free) temporary folder. The first one a test process asks for also
/// clears away what test processes that have ended left in $TMPDIR (`sweep_dead`).
pub fn temp(name: &str) -> PathBuf {
    static SWEEP: std::sync::Once = std::sync::Once::new();
    SWEEP.call_once(|| {
        sweep_dead(&std::env::temp_dir());
    });
    let dir = std::env::temp_dir().join(format!("sparkbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // macOS temp paths go through a /var -> /private/var symlink; use the real one like a Spark would.
    dir.canonicalize().unwrap()
}

/// Removes from `dir` what test processes that have ended left there: the folders `temp` and
/// `fake_node_as` make (`sparkbox-<name>-<pid>`, and the `.fakebin`, `.fakestate` and `.spark`
/// folders beside a node's home) and `fake_spark`'s Docker sockets (`sbd-<pid>-<n>.sock`), when
/// `kill(pid, 0)` says there's no such process (ESRCH). A running process's, another `cargo test`'s
/// included, stay. How many it removed.
pub fn sweep_dead(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(pid) = leftover_pid(&name) else { continue };
        if pid == std::process::id() || !process_gone(pid) {
            continue;
        }
        let (path, kind) = (e.path(), e.file_type());
        let gone = if name.starts_with("sbd-") {
            use std::os::unix::fs::FileTypeExt;
            kind.is_ok_and(|k| k.is_socket()) && std::fs::remove_file(&path).is_ok()
        } else {
            unlock_tree(&path);
            kind.is_ok_and(|k| k.is_dir()) && std::fs::remove_dir_all(&path).is_ok()
        };
        removed += usize::from(gone);
    }
    removed
}

/// The test process a leftover in $TMPDIR is from, by its name (see `sweep_dead`).
fn leftover_pid(name: &str) -> Option<u32> {
    let pid = |s: &str| if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) { s.parse().ok() } else { None };
    if let Some(rest) = name.strip_prefix("sbd-") {
        let (p, n) = rest.strip_suffix(".sock")?.split_once('-')?;
        return pid(n).and(pid(p));
    }
    let rest = name.strip_prefix("sparkbox-")?;
    let rest = [".fakebin", ".fakestate", ".spark"].iter().find_map(|s| rest.strip_suffix(s)).unwrap_or(rest);
    pid(rest.rsplit_once('-')?.1)
}

/// No process `pid` exists any more.
fn process_gone(pid: u32) -> bool {
    pid > 1 && unsafe { libc::kill(pid as libc::pid_t, 0) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// The fake nodes' host key fingerprint, as SparkBox pins it.
pub fn host_fingerprint() -> String {
    use russh::keys::HashAlg;
    russh::keys::PrivateKey::from_openssh(&*HOST_KEY).unwrap().public_key().fingerprint(HashAlg::Sha256).to_string()
}

/// Pins the fake host key for these local ports, as the Connect wizard's "Trust" would.
pub fn trust_ports(known_hosts: &Path, ports: &[u16]) {
    for p in ports {
        crate::live::trust_host_key(known_hosts, &format!("127.0.0.1:{p}"), &host_fingerprint()).unwrap();
    }
}

/// The inventory for fake nodes on these local ports.
pub fn inventory(ports: &[(&str, u16)]) -> Inventory {
    Inventory {
        nodes: ports
            .iter()
            .map(|(id, port)| NodeConfig {
                id: id.to_string(),
                name: id.to_string(),
                host: format!("{id}.local"),
                ip: Some("127.0.0.1".into()),
                model: "MSI EdgeXpert".into(),
                port: *port,
                user: "sparkuser".into(),
                fabric: None,
            })
            .collect(),
        switches: vec![],
    }
}

/// Connects a Live backend to fake nodes ("pw" is the password) and waits until they're online.
pub async fn connect(dir: &Path, ports: &[(&str, u16)]) -> (Arc<Live>, Inventory) {
    connect_with_keys(dir, ports, |_| host_fingerprint()).await
}

/// Like `connect`, for fake Sparks (`fake_spark`): each id is the Spark's name, and its own key is trusted.
pub async fn connect_sparks(dir: &Path, ports: &[(&str, u16)]) -> (Arc<Live>, Inventory) {
    connect_with_keys(dir, ports, |id| Profile::spark(id).fingerprint()).await
}

async fn connect_with_keys(dir: &Path, ports: &[(&str, u16)], key: impl Fn(&str) -> String) -> (Arc<Live>, Inventory) {
    let vpn = Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let known = dir.join("known_hosts.json");
    for (id, p) in ports {
        crate::live::trust_host_key(&known, &format!("127.0.0.1:{p}"), &key(id)).unwrap();
    }
    let live = Live::new(vpn, known, || Ok(Some("pw".into())));
    live.set_ssh_config_path(Some(dir.join("no-ssh-config")));
    live.set_agent_socket(None);
    live.sshlog.set_path(dir.join("ssh.log"));
    let inventory = inventory(ports);
    live.start(&inventory.nodes);
    // Authentication can finish before the real collector processes have started.
    // A four-node fixture must wait for their first reports, not assume ten seconds
    // is enough for the host to launch them. Production stall deadlines still apply.
    wait_for("all fake nodes' first collector reports", || {
        live.snapshot(&inventory).nodes.iter().all(|n| n.status == LinkStatus::Online).then_some(())
    }, || {
        let states: Vec<_> = live.snapshot(&inventory).nodes.into_iter()
            .map(|n| (n.id, n.status, n.error)).collect();
        format!("{states:?}\n{}", std::fs::read_to_string(dir.join("ssh.log")).unwrap_or_default())
    }).await;
    (live, inventory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_waits_for_the_first_report_after_an_initial_collector_stall() {
        let (node, port) = fake_node(&temp("delayed-first-report"), real_shell()).await;
        node.knobs.stall_frames.store(true, Ordering::SeqCst);
        let knobs = node.knobs.clone();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(11)).await;
            knobs.stall_frames.store(false, Ordering::SeqCst);
        });
        let began = std::time::Instant::now();
        let (live, inventory) = connect(&temp("delayed-first-report-app"), &[("cell2", port)]).await;
        assert!(began.elapsed() >= Duration::from_secs(11));
        assert_eq!(live.snapshot(&inventory).nodes[0].status, LinkStatus::Online);
        release.await.unwrap();
        live.stop();
    }

    /// A bare russh client that trusts any host key: what an OpenSSH client sees, without Live's budget.
    struct Raw;

    impl russh::client::Handler for Raw {
        type Error = russh::Error;

        async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    /// C-J on a plain fake node stops at the shim: the Mac's own ping refuses `-M do` with exit 64.
    #[tokio::test]
    async fn a_fake_nodes_ping_stays_on_the_fake_node() {
        let (_node, port) = fake_node(&temp("fake-ping"), real_shell()).await;
        let (live, _) = connect(&temp("fake-ping-app"), &[("cell2", port)]).await;
        let run = |cmd: &'static str| {
            let live = live.clone();
            async move { live.run("cell2", cmd, None, Duration::from_secs(10)).await.unwrap() }
        };
        let out = run("ping -c 2 -W 2 -M do -s 8972 -I enp1s0f1np1 192.168.100.1").await;
        assert_eq!((out.code, out.stdout.as_str()), (Some(0), "PING 192.168.100.1 (192.168.100.1) 8972(9000) bytes of data.\n"));
        assert_eq!(run("ping -c 2 -M want 192.168.100.1 -Q 1").await.code, Some(2), "a flag C-J doesn't send is iputils' usage error here");
        live.stop();
    }

    #[tokio::test]
    async fn the_eleventh_session_channel_is_refused_as_sshd_does() {
        let (node, port) = fake_node(&temp("maxsessions"), real_shell()).await;
        let mut h = russh::client::connect(Arc::new(russh::client::Config::default()), ("127.0.0.1", port), Raw).await.unwrap();
        assert!(h.authenticate_password("sparkuser", "pw").await.unwrap().success());
        let mut open = Vec::new();
        for _ in 0..10 {
            open.push(h.channel_open_session().await.expect("ten are allowed"));
        }
        match h.channel_open_session().await {
            Err(russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)) => {}
            other => panic!("the 11th should be refused, got {:?}", other.map(|c| c.id())),
        }
        assert_eq!((node.knobs.refused.load(Ordering::SeqCst), node.peak_sessions()), (1, 10));
        // Closing one frees its place.
        open.pop().unwrap().close().await.unwrap();
        let mut again = None;
        for _ in 0..50 {
            if let Ok(c) = h.channel_open_session().await {
                again = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(again.is_some(), "a closed channel's place is given back");

        // A command still running after its channel was closed keeps its place until it ends.
        let ch = open.pop().unwrap();
        ch.exec(true, "trap '' HUP TERM; sleep 1").await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        ch.close().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(h.channel_open_session().await.is_err(), "still held by the command that's ending");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(h.channel_open_session().await.is_ok(), "free once it ended");

        // Without the limit (max_sessions None), the fake takes any number.
        *node.knobs.max_sessions.lock().unwrap() = None;
        for _ in 0..4 {
            h.channel_open_session().await.unwrap();
        }
    }

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// The pid a command wrote to `path` (`echo $$ > …`).
    async fn pid_in(path: &Path) -> i32 {
        for _ in 0..100 {
            if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|t| t.trim().parse().ok()) {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        panic!("no pid in {}", path.display());
    }

    async fn ended(pid: i32, secs: u64) -> bool {
        for _ in 0..secs * 20 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// As sshd: closing a terminal's channel hangs its shell up; closing a plain command's
    /// leaves it running until it reads end of input or writes to its closed output.
    #[tokio::test]
    async fn a_closed_channel_hangs_up_a_terminal_but_not_a_plain_command() {
        let home = temp("fake-close");
        let (_node, port) = fake_node(&home, real_shell()).await;
        let mut h = russh::client::connect(Arc::new(russh::client::Config::default()), ("127.0.0.1", port), Raw).await.unwrap();
        assert!(h.authenticate_password("sparkuser", "pw").await.unwrap().success());

        // Quiet: it neither reads nor writes, so it's still running after its channel closed.
        let quiet = h.channel_open_session().await.unwrap();
        quiet.exec(true, "echo $$ > quiet.pid; exec sleep 30").await.unwrap();
        let q = pid_in(&home.join("quiet.pid")).await;
        quiet.close().await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(alive(q), "a plain command keeps running once its channel is closed");

        // One that keeps writing: its next write goes to a closed pipe, and SIGPIPE ends it.
        let chatty = h.channel_open_session().await.unwrap();
        chatty.exec(true, "echo $$ > chatty.pid; while :; do echo tick; sleep 0.1; done").await.unwrap();
        let c = pid_in(&home.join("chatty.pid")).await;
        chatty.close().await.unwrap();
        assert!(ended(c, 5).await, "a writer ends at its next write");

        // A terminal: the login shell is hung up with it.
        let term = h.channel_open_session().await.unwrap();
        term.request_pty(false, "xterm-256color", 80, 24, 0, 0, &[]).await.unwrap();
        term.request_shell(false).await.unwrap();
        term.data(&b"echo $$ > shell.pid\r"[..]).await.unwrap();
        let s = pid_in(&home.join("shell.pid")).await;
        assert!(alive(s));
        term.close().await.unwrap();
        assert!(ended(s, 5).await, "the login shell ends with its terminal");

        assert!(alive(q), "the quiet one still runs");
        unsafe { libc::kill(q, libc::SIGKILL) };
    }

    /// The fake reads commands as the nodes do (the module notes): dash, the nodes' `sh`, both for
    /// the exec request itself and for `sh` on the node's PATH, never macOS's /bin/sh (bash 3.2).
    /// Each line below is bash-only syntax that dash refuses or reads as plain text.
    #[tokio::test]
    async fn a_fake_node_parses_commands_as_ubuntu_dash_does() {
        let home = temp("fake-dash");
        let (node, port) = fake_node(&home, real_shell()).await;
        let (live, _) = connect(&temp("fake-dash-app"), &[("cell2", port)]).await;
        let run = |cmd: &'static str| {
            let live = live.clone();
            async move { live.run("cell2", cmd, None, Duration::from_secs(10)).await.unwrap() }
        };
        let o = run("[[ 1 ]]").await;
        assert_eq!(o.code, Some(127), "{o:?}");
        assert!(o.stderr.contains("[[: not found"), "{}", o.stderr);
        assert_eq!(run("echo $'x'").await.stdout, "$x\n");
        assert_eq!(run("echo {a,b}").await.stdout, "{a,b}\n");
        let o = run("a=(1)").await;
        assert_eq!(o.code, Some(2), "{o:?}");
        assert!(o.stderr.contains("Syntax error"), "{}", o.stderr);
        // `sh` on the PATH is dash: no bash variables, and it is /bin/dash.
        assert_eq!(run("sh -c 'echo \"$0 ${BASH_VERSION:-dash}\"'").await.stdout, "sh dash\n");
        assert_eq!(run("readlink \"$(command -v sh)\"").await.stdout, format!("{DASH}\n"));
        // So is the shell the exec request itself runs in (what sshd's login shell does here).
        let mut h = russh::client::connect(Arc::new(russh::client::Config::default()), ("127.0.0.1", port), Raw).await.unwrap();
        assert!(h.authenticate_password("sparkuser", "pw").await.unwrap().success());
        let mut ch = h.channel_open_session().await.unwrap();
        ch.exec(true, "echo ${BASH_VERSION:-dash}; [[ 1 ]] 2>/dev/null; echo $?").await.unwrap();
        let mut out = Vec::new();
        while let Some(msg) = ch.wait().await {
            if let russh::ChannelMsg::Data { data } = msg {
                out.extend_from_slice(&data);
            }
        }
        assert_eq!(String::from_utf8_lossy(&out), "dash\n127\n");
        // Every shell shim, and a script `bin` writes, starts #!/bin/dash; the rest are Python.
        node.bin("x-test", "echo hi");
        for e in std::fs::read_dir(&node.bin_dir).unwrap().flatten() {
            if e.file_name() == "sh" {
                assert_eq!(std::fs::read_link(e.path()).unwrap(), Path::new(DASH));
                continue;
            }
            let text = std::fs::read_to_string(e.path()).unwrap();
            let first = text.lines().next().unwrap_or("");
            assert!(first == "#!/bin/dash" || first == "#!/usr/bin/env python3", "{}: {first}", e.path().display());
        }
        assert!(std::fs::read_to_string(node.bin_dir.join("x-test")).unwrap().starts_with("#!/bin/dash\n"));
    }

    /// The sudo stand-in answers as sudo 1.9 does over SSH: without -S there's no terminal to ask
    /// on, -n never asks, and root (a sudo inside a command already run as root) isn't asked.
    #[tokio::test]
    async fn sudo_refuses_without_a_terminal_and_never_asks_root() {
        let (node, _port) = fake_node(&temp("fake-sudo"), real_shell()).await;
        let sudo = |args: &[&str], root: bool, input: &str| {
            let mut c = std::process::Command::new(node.bin_dir.join("sudo"));
            c.args(args).env("SB_FAKE_SUDO_PASSWORD", "pw").env("SB_FAKE_NOPASSWD", "0").env("SB_FAKE_ROOT", if root { "1" } else { "0" });
            c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            let mut child = c.spawn().unwrap();
            let mut stdin = child.stdin.take().unwrap();
            if !input.is_empty() {
                std::io::Write::write_all(&mut stdin, input.as_bytes()).unwrap();
            }
            drop(stdin);
            let o = child.wait_with_output().unwrap();
            (o.status.code(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
        };
        let no_terminal = "sudo: a terminal is required to read the password; either use the -S option to read from standard input or configure an askpass helper\nsudo: a password is required\n";
        assert_eq!(sudo(&["echo", "ran"], false, ""), (Some(1), String::new(), no_terminal.to_string()));
        assert_eq!(sudo(&["-n", "echo", "ran"], false, ""), (Some(1), String::new(), "sudo: a password is required\n".to_string()));
        assert_eq!(sudo(&["-k", "-S", "-p", "", "echo", "ran"], false, "pw\n"), (Some(0), "ran\n".to_string(), String::new()));
        assert_eq!(sudo(&["echo", "ran"], true, ""), (Some(0), "ran\n".to_string(), String::new()));
        assert_eq!(sudo(&["-n", "echo", "ran"], true, ""), (Some(0), "ran\n".to_string(), String::new()));
    }

    /// `temp`'s sweep removes what ended test processes left (folders and Docker sockets), and
    /// never a running process's.
    #[test]
    fn the_sweep_clears_only_what_ended_processes_left() {
        let dir = temp("sweep");
        // A pid no process has (the largest macOS hands out is 99998), and two that run.
        let (gone, live, me) = (99_999u32, 1u32, std::process::id());
        assert!(process_gone(gone) && !process_gone(live) && !process_gone(me));
        let made = |n: String, sock: bool| {
            let p = dir.join(n);
            if sock {
                std::os::unix::net::UnixListener::bind(&p).unwrap();
            } else {
                std::fs::create_dir_all(p.join("inside")).unwrap();
            }
            p
        };
        let old = [
            made(format!("sparkbox-ops-fleet-home-{gone}"), false),
            made(format!("sparkbox-ops-fleet-home-{gone}.fakebin"), false),
            made(format!("sparkbox-ops-fleet-home-{gone}.fakestate"), false),
            made(format!("sparkbox-x-{gone}.spark"), false),
            made(format!("sbd-{gone}-0.sock"), true),
        ];
        let kept = [
            made(format!("sparkbox-ops-fleet-home-{me}"), false),
            made(format!("sparkbox-a-{live}"), false),
            made(format!("sbd-{me}-0.sock"), true),
            made("sparkbox-staging-spark1.setup.lock".into(), false),
            made("sparkbox-verify-no-fence-config".into(), false),
            made(format!("sb-gate-{gone}-7"), false),
            made(format!("sparkbox-vpn-{gone}.json"), false),
        ];
        // A locked .spark tree, as spark_tree.py leaves one.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(old[3].join("inside"), std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(sweep_dead(&dir), old.len());
        assert!(old.iter().all(|p| !p.exists()), "{old:?}");
        assert!(kept.iter().all(|p| p.exists()), "{kept:?}");
        assert_eq!(sweep_dead(&dir), 0);
    }

    #[test]
    fn logical_commands_unwrap_sparkboxs_wrappers() {
        assert_eq!(logical("env LC_ALL=C sh -c 'echo '\\''hi'\\'''"), ("echo 'hi'".to_string(), None));
        let (cmd, prompt) = logical("sudo -k -S -p 'SPARKBOX-SUDO-ab:' -- env LC_ALL=C sh -c 'apt-get update'");
        assert_eq!((cmd.as_str(), prompt.as_deref()), ("sudo -k -S -p '' apt-get update", Some("SPARKBOX-SUDO-ab:")));
        assert_eq!(logical("true").0, "true", "anything else is taken as it is");
        assert_eq!(logical("env LC_ALL=C sh -c 'a' 'b'").0, "env LC_ALL=C sh -c 'a' 'b'", "only one quoted word unwraps");
    }
}
