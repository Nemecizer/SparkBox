//! Live data from the Sparks over SSH, made directly, through the VPN tunnel or through
//! a jump host. Login uses ssh-agent keys, then key files, then the cluster password
//! (sshconfig.rs). Each node keeps two connections:
//!
//! - *control*: the collector stream, probes, sudo and short commands. SparkBox copies a
//!   small read-only Python collector to the node (~/.sparkbox/collector.py, only when it
//!   changed) and streams one JSON frame per second from it.
//! - *bulk*: terminals, log followers, file transfers and tails, made on first use. Their
//!   output can be large or unread for a while; on their own connection a slow reader
//!   can't hold up the collector or a short command. One still being made when its node is
//!   stopped (Live → Demo) is closed as it comes up, never used.
//!
//! Every command runs exactly as written: it goes to the node as
//! `env LC_ALL=C sh -c '<command>'`, so compound commands work whatever the login shell
//! is, in the C locale. One that runs ~/.sparkbox/sparkjob.py first checks that the file is
//! SparkBox's own copy (another SparkBox on the same node may have put its own there) and
//! exits COPY_STALE, having run nothing, when it isn't.
//!
//! The control connection never shows old numbers as current: a node whose frames stop
//! is marked stale after 3 s. After 10 s only the collector's channel is closed and `true`
//! runs on the same connection: when it answers, a fresh collector starts there; when it
//! doesn't (or after more than 3 stalls in 5 minutes) the connection is made again. A
//! collector that ends (killed under memory pressure, say) starts again the same way, on the
//! same connection; one that keeps ending, or a node without python3, shows as an Error while
//! its connection stays up for commands, and is tried again a minute later. That watchdog
//! leaves the bulk connection alone. Waking the Mac from sleep, or the VPN switching between
//! direct and tunnel, reconnects every node (a bulk connection only when nothing is using it);
//! commands still running on a connection that's closed are marked interrupted in the activity
//! log first. Saving the inventory reconnects only the nodes whose settings changed.
//!
//! Each connection keeps within sshd's 10 channels (ssh.rs). Long-lived channels go to the
//! bulk connection, and to an extra one (at most three per node) when it's full or the node
//! refuses a channel there anyway. Every connect, stall, drop and reconnect goes into the SSH
//! log (sshlog.rs).
//!
//! sudo is prompt-safe: SparkBox waits for sudo's own randomised prompt before
//! sending the password, so a node where sudo doesn't ask (NOPASSWD) never
//! receives it, and a password a node rejects stops further attempts there until it
//! changes. Rejections on two nodes stop sudo everywhere.
//!
//! Reads are shared (`Reads`): the same read on the same node, with the same time limit, asked
//! again while it's still running waits for that run (one exec, one activity row) instead of
//! sending it twice, and a background probe that finished under 2 s ago answers the same probe
//! from another caller (`probe_fresh` never takes one: whether a process has exited yet).
//! No caller gets the same reading twice: a loop asking again gets a fresh one. A finished
//! `run` is never reused, nothing that changes a node is ever shared, and a change sent to a
//! node starts its reads afresh. A command SparkBox stops waiting for ends its activity row as
//! cancelled (`Row`), never "running".

pub mod audit;
// ── p1r1-x ── the write exit: fence, lease, identity pin
pub mod exit;
pub mod fence;
pub mod fence_class;
pub mod identity_pin;
pub mod lease;
pub(crate) mod ssh;
pub mod sshconfig;
pub mod sshlog;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)] #[path = "session_a_rehearsal.rs"] mod session_a_rehearsal;
#[cfg(test)] #[path = "session_b_rehearsal.rs"] mod session_b_rehearsal;
#[cfg(test)] mod staging_tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use russh::ChannelMsg;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::{watch, Notify};

use crate::inventory::{Inventory, NodeConfig, SwitchConfig};
use crate::model::*;
use crate::vpn::Vpn;
pub use ssh::{CancelToken, SshError, StreamKind, OUTPUT_CAP};

pub const COLLECTOR: &str = include_str!("collector.py");
pub const SPARKJOB: &str = include_str!("sparkjob.py");
pub const ROCE: &str = include_str!("roce.py");

/// No frame for this long: the numbers shown are marked stale.
pub const STALE_AFTER: Duration = Duration::from_secs(3);
/// No frame for this long: the collector is restarted, or the connection made again when it
/// doesn't answer.
pub const DROP_AFTER: Duration = Duration::from_secs(10);
/// How long `true` gets to show that a connection still answers.
const ANSWER_LIMIT: Duration = Duration::from_secs(10);
/// More collector restarts than this within RESTART_WINDOW: the connection counts as dead
/// (stalls), or the collector waits COLLECTOR_RETRY before it's tried again (exits).
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(300);
const COLLECTOR_RETRY: Duration = Duration::from_secs(60);
/// Bulk connections per node at most: the first one and two extra.
const MAX_BULK: usize = 3;
/// An extra bulk connection that carried nothing this long is closed.
const BULK_IDLE: Duration = Duration::from_secs(60);
/// How often each node's channel use goes into the SSH log.
const STATS_EVERY: Duration = Duration::from_secs(600);
/// How long sudo gets to show its prompt before SparkBox assumes it won't ask.
const SUDO_PROMPT_WAIT: Duration = Duration::from_secs(5);
/// The Mac's clock jumping ahead of the monotonic one by more than this means it slept.
const SLEEP_JUMP: Duration = Duration::from_secs(5);
/// A probe's result is reused for this long.
const PROBE_REUSE: Duration = Duration::from_secs(2);

/// Single-quotes an argument for the node's shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// What a command looks like on the wire: run by `sh -c` exactly as written, in the C
/// locale, whatever the cluster user's login shell is. A sparkjob.py command first checks the
/// file is SparkBox's copy (`sparkjob_checked`).
pub fn wire(cmd: &str) -> String {
    format!("env LC_ALL=C sh -c {}", sh_quote(&sparkjob_checked(cmd)))
}

/// How SparkBox runs its own helper in the cluster user's home.
const SPARKJOB_RUN: &str = "python3 ~/.sparkbox/sparkjob.py ";

/// The exit status of a sparkjob.py run that found the file isn't SparkBox's copy (missing, or
/// another SparkBox's): nothing ran. Its connection checks its helpers again (ssh.rs).
pub const COPY_STALE: u32 = ROOT_COPY_STALE;

/// `cmd`, as written, after a check that ~/.sparkbox/sparkjob.py is SparkBox's copy, when it
/// runs that file. Two SparkBoxes (a release and a dev build) share the folder on a node, and
/// `ensure_file` checks once per connection: without this, one could run the other's copy.
fn sparkjob_checked(cmd: &str) -> String {
    static SHA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if !cmd.starts_with(SPARKJOB_RUN) {
        return cmd.to_string();
    }
    let sha = SHA.get_or_init(|| sha256_hex(SPARKJOB.as_bytes()));
    format!(
        "[ \"$(sha256sum < ~/.sparkbox/sparkjob.py 2>/dev/null | cut -c1-64)\" = {sha} ] \
         || {{ echo \"~/.sparkbox/sparkjob.py isn't this SparkBox's copy, so it didn't run.\" >&2; exit {COPY_STALE}; }}; {cmd}"
    )
}

/// A command under sudo, with sudo's one-off `prompt`.
pub fn sudo_wire(cmd: &str, prompt: &str) -> String {
    format!("sudo -k -S -p {} -- {}", sh_quote(prompt), wire(cmd))
}

fn last_line(s: &str) -> String {
    s.trim().lines().last().unwrap_or("").trim().to_string()
}
const COLLECTOR_PATH: &str = "~/.sparkbox/collector.py";

/// Pins a host key for "address:port" in SparkBox's known hosts (after the user compared it).
pub fn trust_host_key(known_hosts: &Path, address: &str, fingerprint: &str) -> Result<(), String> {
    ssh::trust(known_hosts, address, fingerprint)
}

/// Forgets every pinned host key, including a known_hosts.json that couldn't be read.
pub fn forget_host_keys(known_hosts: &Path) -> Result<(), String> {
    ssh::forget(known_hosts)
}

// Only Live comes here, so it never says to switch to Live: the node is starting, offline or still connecting.
fn not_connected(node_id: &str) -> String {
    format!("{node_id} isn't connected. SparkBox keeps reconnecting; check that the node is on and on your network.")
}

fn breaker_message(node: &str) -> String {
    format!("The saved cluster password was rejected by sudo on {node}. Save the correct password in Settings.")
}

fn breaker_message_all(nodes: &[String]) -> String {
    let list = match nodes {
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
        [] => String::new(),
    };
    format!("The saved cluster password was rejected by sudo on {list}, so SparkBox has stopped using it everywhere. Save the correct password in Settings.")
}

/// Diagnostics gathered by "Run doctor": everything the collector depends on, then what the
/// SSH transport, the image store, a kernel rollback, the firewall and apt depend on (P1.13).
/// Each section only reads (`doctor_script` runs them as one command through the write exit,
/// which must judge every part of it a read), as the cluster user, without sudo.
const DOCTOR_SECTIONS: &[(&str, &str)] = &[
    ("host", "hostname; uname -a; cat /etc/os-release"),
    ("model", "cat /sys/class/dmi/id/product_name 2>/dev/null; tr -d '\\0' < /proc/device-tree/model 2>/dev/null; echo"),
    ("user", "id"),
    ("python", "which python3; python3 --version"),
    ("nvidia-smi", "nvidia-smi 2>&1"),
    ("gpu-query", "nvidia-smi --query-gpu=utilization.gpu,temperature.gpu,power.draw,clocks.sm,driver_version --format=csv,noheader,nounits 2>&1"),
    ("gpu-apps", "nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits 2>&1"),
    ("memory", "head -25 /proc/meminfo; free -b"),
    ("cpu", "nproc; head -3 /proc/stat; cat /proc/loadavg; lscpu 2>/dev/null | head -25"),
    ("thermal", "for z in /sys/class/thermal/thermal_zone*; do echo \"$(cat $z/type 2>/dev/null) $(cat $z/temp 2>/dev/null)\"; done"),
    ("disks", "grep '^/dev' /proc/mounts; df -B1 -x tmpfs -x devtmpfs -x overlay 2>/dev/null; lsblk -b -o NAME,SIZE,TYPE,MOUNTPOINT 2>/dev/null"),
    (
        "network",
        "ip -br link; ip -br -4 addr; for i in /sys/class/net/*; do echo \"$(basename $i) state=$(cat $i/operstate 2>/dev/null) speed=$(cat $i/speed 2>/dev/null) \
         driver=$(basename \"$(readlink $i/device/driver 2>/dev/null)\") wireless=$([ -e \"$i/wireless\" ] && echo yes || echo no)\"; done",
    ),
    ("rdma", "ls /sys/class/infiniband 2>/dev/null"),
    ("docker", "ls -l /var/run/docker.sock; docker version --format '{{.Server.Version}}' 2>&1; docker ps -a --no-trunc --format '{{.ID}} {{.Names}} {{.Image}} {{.State}} {{.Status}}' 2>&1 | head -40"),
    ("cgroups", "cat /proc/self/cgroup; ls /sys/fs/cgroup/system.slice 2>/dev/null | grep -m5 docker"),
    ("listening", "ss -tlnp 2>/dev/null | head -40"),
    // sshd's channel limit, where the cluster user can read its configuration.
    ("sshd-config", "grep -ri maxsessions /etc/ssh/sshd_config /etc/ssh/sshd_config.d 2>&1"),
    ("machine-id", "cat /etc/machine-id 2>&1"),
    ("image-store", "docker info -f '{{.Driver}} {{json .DriverStatus}}' 2>&1"),
    ("grub", "grep GRUB_DEFAULT /etc/default/grub 2>&1"),
    ("ufw", "grep ENABLED /etc/ufw/ufw.conf 2>&1"),
    ("apt", "apt-config dump APT::Periodic 2>&1; apt-mark showhold 2>&1"),
    ("dgx-release", "cat /etc/dgx-release 2>&1"),
];

/// The doctor's one command: every section under its "----- <name>" line. A section's own
/// failure is in its output; the command itself ends well once every section has run.
fn doctor_script() -> String {
    let sections: Vec<String> = DOCTOR_SECTIONS.iter().map(|(name, cmds)| format!("echo; echo '----- {name}'; {cmds}")).collect();
    format!("{}\ntrue", sections.join("\n"))
}

/// How long the doctor's command may run on each node.
const DOCTOR_LIMIT: Duration = Duration::from_secs(90);

/// The line the report gets when the doctor's command didn't run to its end, so the sections
/// after the last one shown are missing: stopped at the time limit, killed, the connection
/// lost (no exit status), an exit other than 0 (the script ends with `true`), or output past
/// OUTPUT_CAP. None when it ran through.
fn doctor_cut_short(out: &ssh::Output, limit: Duration) -> Option<String> {
    let why = if out.timed_out {
        format!("Diagnostics stopped after {} s", limit.as_secs())
    } else {
        match (out.code, &out.signal) {
            (_, Some(sig)) => format!("Diagnostics cut short: the doctor's command was killed (SIG{sig})"),
            (None, None) => "Diagnostics cut short: the connection ended with no exit status".to_string(),
            (Some(c), None) if c != 0 => format!("The doctor's command ended with exit {c}"),
            _ if out.truncated => format!("The doctor's output went past {} MiB", OUTPUT_CAP / (1024 * 1024)),
            _ => return None,
        }
    };
    Some(format!("{why}: the sections after this point are missing."))
}

/// The settings sshd decides from its configuration, read only with sudo (`sshd -T`): the
/// doctor's sudo-only line, in an owner's session (`Live::doctor_with`).
const DOCTOR_SSHD_T: &str = "sshd -T | grep -Ei 'maxsessions|maxstartups|allowtcpforwarding|disableforwarding'";

/// The GPU part of a frame: a number the collector couldn't read is null.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GpuFrame {
    util_pct: Option<f32>,
    temp_c: Option<f32>,
    power_w: Option<f32>,
    sm_clock_mhz: Option<u32>,
    #[serde(default)]
    procs: Vec<GpuProc>,
    #[serde(default)]
    clock_max_mhz: Option<u32>,
    #[serde(default)]
    power_limit_w: Option<f32>,
    #[serde(default)]
    pstate: Option<String>,
    #[serde(default)]
    throttle: Vec<String>,
    #[serde(default)]
    xid_errors: u32,
    #[serde(default)]
    error: Option<String>,
}

impl From<GpuFrame> for GpuStats {
    /// Null numbers mean the GPU couldn't be read this frame: `readable` false, the numbers 0.
    fn from(g: GpuFrame) -> GpuStats {
        let readable = g.util_pct.is_some() && g.temp_c.is_some() && g.power_w.is_some() && g.sm_clock_mhz.is_some();
        GpuStats {
            util_pct: g.util_pct.unwrap_or_default(),
            temp_c: g.temp_c.unwrap_or_default(),
            power_w: g.power_w.unwrap_or_default(),
            sm_clock_mhz: g.sm_clock_mhz.unwrap_or_default(),
            procs: g.procs,
            clock_max_mhz: g.clock_max_mhz,
            power_limit_w: g.power_limit_w,
            pstate: g.pstate,
            throttle: g.throttle,
            xid_errors: g.xid_errors,
            error: if readable { None } else { Some(g.error.unwrap_or_else(|| "nvidia-smi gave no numbers".into())) },
            readable,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Frame {
    cpu: CpuStats,
    mem: MemStats,
    gpu: GpuFrame,
    disks: Vec<DiskStats>,
    nets: Vec<NetStats>,
    system: SystemInfo,
    containers: Vec<Container>,
    #[serde(default)]
    processes: Vec<Process>,
    #[serde(default)]
    listeners: Vec<Listener>,
    #[serde(default)]
    failed_units: Vec<String>,
    #[serde(default)]
    kernel_events: Vec<KernelEvent>,
    #[serde(default)]
    errors: Vec<String>,
    /// /proc/sys/kernel/random/boot_id (r3 collectors): a new value means the node rebooted.
    #[serde(default)]
    boot_id: Option<String>,
    /// Every process on the node (r4 collectors).
    #[serde(default)]
    proc_total: u32,
}

struct NodeState {
    /// Which node loop this state belongs to (a stopped loop can't write into its successor's).
    generation: u64,
    status: LinkStatus,
    error: Option<String>,
    latency_ms: Option<f32>,
    frame: Option<Frame>,
    session: Option<Arc<ssh::Session>>,
    /// Terminals, logs, transfers and tails (made on first use; outlives control reconnects).
    bulk: Option<Arc<ssh::Session>>,
    /// Extra bulk connections, made when every channel on `bulk` is taken, with the time each
    /// was last in use.
    extra: Vec<(Arc<ssh::Session>, Instant)>,
    /// Logged commands still running: activity entry id → the connection they run on.
    open: HashMap<String, usize>,
    /// Wakes the loop from its wait between attempts.
    retry: Arc<Notify>,
    /// How the control connection logged in.
    login: Option<ssh::LoginInfo>,
    /// When the last frame arrived, and its Unix ms.
    last_frame_at: Option<Instant>,
    last_seen_ms: Option<u64>,
    /// The last frame couldn't be parsed (kept apart from connection errors).
    frame_error: Option<String>,
    /// Why the last connection failed, as a kind (NodeSnapshot.error_kind).
    error_kind: Option<&'static str>,
    /// The host key the node presented: accepted, or offered on first contact or after a change.
    host_key: Option<HostKeyInfo>,
    /// Asks the node's connection to reconnect at once, saying why ("the Mac woke from sleep").
    /// A connection answers only kicks sent after it started connecting; several close
    /// together are one.
    kick: watch::Sender<String>,
}

impl NodeState {
    fn new(generation: u64) -> Self {
        NodeState {
            generation,
            status: LinkStatus::Connecting,
            error: None,
            latency_ms: None,
            frame: None,
            session: None,
            bulk: None,
            extra: Vec::new(),
            open: HashMap::new(),
            retry: Arc::new(Notify::new()),
            login: None,
            last_frame_at: None,
            last_seen_ms: None,
            frame_error: None,
            error_kind: None,
            host_key: None,
            kick: watch::channel(String::new()).0,
        }
    }
}

type PasswordFn = Box<dyn Fn() -> Result<Option<String>, String> + Send + Sync>;

pub struct Live {
    pub audit: audit::Log,
    vpn: Arc<Vpn>,
    known_hosts: PathBuf,
    password: PasswordFn,
    nodes: Mutex<HashMap<String, NodeState>>,
    configs: Mutex<HashMap<String, NodeConfig>>,
    /// Each node's loop, with the config it was started with.
    tasks: Mutex<HashMap<String, (NodeConfig, tauri::async_runtime::JoinHandle<()>)>>,
    /// Watches for sleep, closes idle extra connections and logs channel use (one for all nodes).
    watcher: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    generations: AtomicU64,
    /// Activity entries already ended as interrupted: their command's own ending is dropped.
    interrupted: Mutex<HashSet<String>>,
    /// The SSH connection log.
    pub sshlog: sshlog::SshLog,
    /// $HOME on each node, asked once per connection.
    homes: Mutex<HashMap<String, String>>,
    /// Nodes whose sudo rejected the saved password, with the hash of the password rejected.
    breaker: Mutex<HashMap<String, String>>,
    /// Login settings (Settings › Cluster login), kept next to the known hosts.
    login_path: PathBuf,
    /// ~/.ssh/config, or another file (tests); None reads none.
    ssh_config: Mutex<Option<PathBuf>>,
    /// The agent socket to use instead of the one the settings choose (tests).
    agent_override: Mutex<Option<Option<PathBuf>>>,
    /// One bulk connection is made at a time per node.
    bulk_lock: tokio::sync::Mutex<()>,
    root_lib: Mutex<String>,
    /// Every command that could change a node asks it first (exit.rs).
    exit: exit::Exit,
    /// Reads in flight and probes just answered, shared by identical asks.
    reads: Reads,
}

type Shared = Result<ssh::Output, String>;

/// One read in flight: the node, how it's run ("run" or "probe"), the command, its time limit
/// (a caller that allowed longer never gets another's "took longer than"), and the node's change
/// count when it started.
type ReadKey = (String, &'static str, String, Duration, u64);

/// Identical reads shared (see the module notes).
#[derive(Default)]
struct Reads {
    /// The result of each read in flight, once it has one.
    flights: Mutex<HashMap<ReadKey, watch::Receiver<Option<Shared>>>>,
    /// (node, command) → the last probe: when it finished, the node's change count it started at,
    /// the tasks that have had it, its output.
    recent: Mutex<HashMap<(String, String), Recent>>,
    /// Changes sent to each node so far: a read from before one isn't shared after it.
    changes: Mutex<HashMap<String, u64>>,
}

/// A probe's result, kept PROBE_REUSE for other callers.
struct Recent {
    at: Instant,
    epoch: u64,
    /// The tasks it was given to: each gets a fresh reading when it asks again (a loop's next round).
    served: Vec<tokio::task::Id>,
    out: ssh::Output,
}

impl Reads {
    fn epoch(&self, node: &str) -> u64 {
        self.changes.lock().unwrap().get(node).copied().unwrap_or(0)
    }

    /// A change goes to `node` (or just went): its reads start afresh. The count decides (reads
    /// and results from before it are keyed by the old one); its results are dropped to free them.
    fn changed(&self, node: &str) {
        *self.changes.lock().unwrap().entry(node.to_string()).or_default() += 1;
        self.recent.lock().unwrap().retain(|(n, _), _| n != node);
    }

    /// The last result of this probe, when it finished under PROBE_REUSE ago, no change went to
    /// the node since it started, and this task hasn't had it yet (a caller outside any task
    /// can't be told apart: it always gets a fresh one).
    fn recent(&self, node: &str, cmd: &str) -> Option<ssh::Output> {
        let (epoch, me) = (self.epoch(node), tokio::task::try_id()?);
        let mut recent = self.recent.lock().unwrap();
        let r = recent.get_mut(&(node.to_string(), cmd.to_string()))?;
        if r.at.elapsed() >= PROBE_REUSE || r.epoch != epoch || r.served.contains(&me) {
            return None;
        }
        r.served.push(me);
        Some(r.out.clone())
    }

    /// This task has the probe's last result (it waited on the run that made it).
    fn had(&self, node: &str, cmd: &str) {
        let Some(me) = tokio::task::try_id() else { return };
        if let Some(r) = self.recent.lock().unwrap().get_mut(&(node.to_string(), cmd.to_string())) {
            if !r.served.contains(&me) {
                r.served.push(me);
            }
        }
    }

    fn remember(&self, node: &str, cmd: &str, epoch: u64, out: &ssh::Output) {
        let mut recent = self.recent.lock().unwrap();
        recent.retain(|_, r| r.at.elapsed() < PROBE_REUSE);
        recent.insert((node.to_string(), cmd.to_string()), Recent { at: Instant::now(), epoch, served: tokio::task::try_id().into_iter().collect(), out: out.clone() });
    }
}

/// Takes a read out of the flights when it ends, or when the caller running it stops waiting
/// (the callers waiting on it then run it themselves).
struct Flight<'a> {
    reads: &'a Reads,
    key: ReadKey,
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.reads.flights.lock().unwrap().remove(&self.key);
    }
}

/// Whether identical asks of `cmd` may share one run: the exit let it through as a read (no
/// fence class let it through, and its shape only reads), nobody typed it, and it gets nothing
/// on stdin.
fn shareable(kind: exit::ExecKind, cmd: &str, admitted: &exit::Admitted, stdin: Option<&[u8]>) -> bool {
    stdin.is_none() && admitted.fence.is_none() && !audit::in_typed() && fence_class::class_of(kind, cmd, &fence_class::Ctx::default()) == fence_class::Shape::Read
}

/// A logged command's activity row while it runs (`log_begin`): `log_end` completes it. Dropped
/// first (SparkBox stopped waiting), it stops being tracked on its connection and ends as
/// cancelled (`audit::Begun`), never left "running".
struct Row<'a> {
    live: &'a Live,
    begun: Option<audit::Begun<'a>>,
}

impl Row<'_> {
    fn id(&self) -> Option<&str> {
        self.begun.as_ref().map(|b| b.id())
    }

    /// The command is on its way to the node: its channel's close sends it TERM from now on.
    fn sent(&self) {
        if let Some(b) = &self.begun {
            b.sent();
        }
    }
}

impl Drop for Row<'_> {
    fn drop(&mut self) {
        let Some(begun) = self.begun.take() else { return };
        let id = begun.id().to_string();
        self.live.interrupted.lock().unwrap().remove(&id);
        for s in self.live.nodes.lock().unwrap().values_mut() {
            s.open.remove(&id);
        }
        drop(begun);
    }
}

/// `session.exec_with` (no output callback), telling `row` once the command is on its way.
async fn exec_marked(session: &ssh::Session, cmd: &str, stdin: Option<&[u8]>, limit: Duration, cancel: &CancelToken, row: &Row<'_>) -> Result<ssh::Output, String> {
    let slot = session.slot().await?;
    let deadline = tokio::time::Instant::now() + limit;
    let ch = session.open_on(&slot, limit).await?;
    if ch.is_some() {
        row.sent();
    }
    session.exec_in(ch, slot, cmd, stdin, deadline, cancel, &mut |_, _| {}).await
}

/// A running command's channel, for streaming; holds its connection open and its place there.
/// Dropped (a cancelled copy, an error on the other side), it ends the command on the node and
/// gives its place back once the node has closed the channel (`ssh::Reader::abandon`).
pub struct Exec {
    pub read: ssh::Reader,
    pub write: ssh::CloseOnDrop,
    hold: Option<Hold>,
}

impl Exec {
    /// Both halves of the channel and what holds its place, for a caller that ends and closes
    /// the channel itself (a terminal's pump, in tests).
    #[cfg(test)]
    pub fn into_parts(mut self) -> (russh::ChannelReadHalf, russh::ChannelWriteHalf<russh::client::Msg>, Hold) {
        let parts = (self.read.take().into_inner(), self.write.take(), self.hold.take());
        match parts {
            (Some(read), Some(write), Some(hold)) => (read, write, hold),
            _ => unreachable!("an Exec has its channel until it's dropped"),
        }
    }
}

impl Drop for Exec {
    fn drop(&mut self) {
        if let Some(write) = self.write.take() {
            self.read.take().abandon(write, self.hold.take());
        }
    }
}

/// An interactive terminal channel. It holds its SSH connection, so an extra one stays open.
/// Dropped, it hangs the terminal up on the node (TERM to what runs there, then the close, as
/// closing an ssh window does) and gives its place back once the node has closed the channel,
/// as `Exec` does. A caller that pumps it and closes it itself takes it apart (`into_parts`).
pub struct Pty {
    pub read: ssh::Reader,
    write: Option<russh::ChannelWriteHalf<russh::client::Msg>>,
    hold: Option<Hold>,
}

impl Pty {
    /// Where keystrokes go.
    #[cfg(test)]
    pub fn writer(&self) -> &russh::ChannelWriteHalf<russh::client::Msg> {
        self.write.as_ref().expect("a Pty has its channel until it's dropped")
    }

    /// Both halves of the channel and what holds its place, for a caller that ends and closes
    /// the channel itself (a terminal's pump, term/mod.rs).
    pub fn into_parts(mut self) -> (russh::ChannelReadHalf, russh::ChannelWriteHalf<russh::client::Msg>, Hold) {
        match (self.read.take().into_inner(), self.write.take(), self.hold.take()) {
            (Some(read), Some(write), Some(hold)) => (read, write, hold),
            _ => unreachable!("a Pty has its channel until it's dropped"),
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if let Some(write) = self.write.take() {
            self.read.take().hang_up(write, self.hold.take());
        }
    }
}

/// A connection and a channel's place on it, both kept until this is dropped.
pub struct Hold {
    _session: Arc<ssh::Session>,
    _slot: ssh::Slot,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn nonce() -> Result<String, String> {
    let mut buf = [0u8; 12];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn address(cfg: &NodeConfig) -> String {
    cfg.ip.clone().filter(|ip| !ip.trim().is_empty()).unwrap_or_else(|| cfg.host.clone())
}

/// A switch before (or without) telemetry: configured only once it has an address, and never Online here.
pub fn placeholder_switch(s: &SwitchConfig) -> SwitchSnapshot {
    let configured = !s.host.trim().is_empty();
    SwitchSnapshot {
        id: s.id.clone(),
        name: s.name.clone(),
        kind: s.kind,
        host: s.host.clone(),
        status: if configured { LinkStatus::Connecting } else { LinkStatus::Offline },
        configured,
        ..Default::default()
    }
}

fn node_snapshot(cfg: &NodeConfig, st: Option<&NodeState>) -> NodeSnapshot {
    let status = st.map(|s| s.status).unwrap_or(LinkStatus::Connecting);
    let frame = st.and_then(|s| s.frame.clone());
    let error = match (&frame, st.and_then(|s| s.error.clone().or_else(|| s.frame_error.clone()))) {
        (_, Some(e)) => Some(e),
        (Some(f), None) if !f.errors.is_empty() => Some(f.errors.join("; ")),
        _ => None,
    };
    let age = st.and_then(|s| s.last_frame_at).map(|t| t.elapsed());
    let mut snap = NodeSnapshot {
        id: cfg.id.clone(),
        name: cfg.name.clone(),
        host: address(cfg),
        status,
        latency_ms: st.and_then(|s| s.latency_ms),
        error,
        error_kind: st.and_then(|s| s.error_kind).filter(|_| status != LinkStatus::Online).map(String::from),
        host_key: st.and_then(|s| s.host_key.clone()),
        system: SystemInfo { hostname: cfg.host.trim_end_matches(".local").into(), model: cfg.model.clone(), ..Default::default() },
        ..Default::default()
    };
    if let Some(f) = frame {
        snap.cpu = f.cpu;
        snap.mem = f.mem;
        snap.gpu = f.gpu.into();
        snap.proc_total = f.proc_total;
        snap.disks = f.disks;
        snap.nets = f.nets;
        snap.containers = f.containers;
        snap.processes = f.processes;
        snap.listeners = f.listeners;
        snap.failed_units = f.failed_units;
        snap.kernel_events = f.kernel_events;
        snap.boot_id = f.boot_id;
        let model = if f.system.model.trim().is_empty() { cfg.model.clone() } else { format!("{} ({})", cfg.model, f.system.model) };
        snap.system = SystemInfo { model, ..f.system };
        snap.frame_age_ms = age.map(|a| a.as_millis() as u64);
        snap.last_seen_ms = st.and_then(|s| s.last_seen_ms);
        snap.stale = age.is_some_and(|a| a > STALE_AFTER);
        // R1-K (P1.46): mask secrets in command lines before the snapshot leaves the backend.
        crate::live::audit::redact_processes(&mut snap.processes);
        crate::live::audit::redact_containers(&mut snap.containers);
    }
    snap
}

/// Names, on each node's host key, the other nodes that present the same one (a cloned image:
/// cell1, cell2 and cell3 all have SHA256:example…).
fn share_host_keys(nodes: &mut [NodeSnapshot]) {
    let keys: Vec<(String, Option<String>)> = nodes.iter().map(|n| (n.name.clone(), n.host_key.as_ref().map(|k| k.fingerprint.clone()))).collect();
    for n in nodes.iter_mut() {
        if let Some(k) = n.host_key.as_mut() {
            k.shared_with = keys.iter().filter(|(name, fp)| *name != n.name && fp.as_deref() == Some(k.fingerprint.as_str())).map(|(name, _)| name.clone()).collect();
        }
    }
}

/// Why a node couldn't be reached, as a kind the UI words (NodeSnapshot.error_kind).
pub fn error_kind_of(message: &str) -> Option<&'static str> {
    let m = message.to_lowercase();
    if ssh::is_busy(message) {
        Some("busy")
    } else if m.contains("hasn't connected to") {
        Some("hostKeyUnknown")
    } else if m.contains("host key for") && m.contains("changed") {
        Some("hostKeyChanged")
    } else if m.contains("timeout") || m.contains("timed out") || m.contains("didn't answer") {
        Some("timeout")
    } else if m.contains("refused") {
        Some("refused")
    } else if m.contains("no route") || m.contains("unreachable") || m.contains("host is down") {
        Some("unreachable")
    } else if m.contains("nodename nor servname") || m.contains("failed to lookup") || m.contains("not known") || m.contains("resolve") {
        Some("dns")
    } else if m.contains("rejected") || m.contains("didn't accept") || m.contains("no ssh key was found") || m.contains("authentication") {
        Some("authFailed")
    } else {
        None
    }
}

/// How a node's connection failed: the message, its kind, and the key it offered.
struct NodeFailure {
    message: String,
    kind: Option<&'static str>,
    host_key: Option<HostKeyInfo>,
}

impl From<String> for NodeFailure {
    fn from(message: String) -> Self {
        NodeFailure { kind: error_kind_of(&message), message, host_key: None }
    }
}

impl From<&str> for NodeFailure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl From<SshError> for NodeFailure {
    fn from(e: SshError) -> Self {
        let message = e.to_string();
        match e {
            SshError::HostKeyUnknown { address, fingerprint } => {
                NodeFailure { message, kind: Some("hostKeyUnknown"), host_key: Some(HostKeyInfo { address, fingerprint, shared_with: vec![] }) }
            }
            SshError::HostKeyChanged { address, fingerprint, .. } => {
                NodeFailure { message, kind: Some("hostKeyChanged"), host_key: Some(HostKeyInfo { address, fingerprint, shared_with: vec![] }) }
            }
            SshError::Failed(m) => m.into(),
        }
    }
}

/// How a node whose connection failed shows. One that didn't answer, refused, isn't on this
/// Mac's network or whose name doesn't resolve is Offline (a timeout reads "No SSH reply in
/// 10 s"); a rejected login, a host key to compare or a changed one, and anything unexpected,
/// is an Error to look at.
fn link_status(mut f: NodeFailure) -> (LinkStatus, NodeFailure) {
    match f.kind {
        Some("timeout") => {
            f.message = "No SSH reply in 10 s".into();
            (LinkStatus::Offline, f)
        }
        Some("unreachable" | "refused" | "dns") => (LinkStatus::Offline, f),
        _ => (LinkStatus::Error, f),
    }
}

/// Two configs for the same node that connect the same way (nothing in them changed).
fn same_config(a: &NodeConfig, b: &NodeConfig) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// Copies a script to ~/.sparkbox/<name> when its checksum differs from ours, and says if it did.
/// It runs on the session itself, outside the write exit (exit.rs): the approved helper refresh,
/// look-only included.
async fn install(session: &ssh::Session, name: &str, content: &str, node: &str) -> Result<bool, String> {
    let path = format!("~/.sparkbox/{name}");
    let have = session.exec(&wire(&format!("sha256sum {path} 2>/dev/null")), None, Duration::from_secs(10)).await?;
    if !have.stdout.starts_with(&sha256_hex(content.as_bytes())) {
        // A per-process temporary name, so two uploads at once can't trip over each other.
        let cmd = format!("mkdir -p ~/.sparkbox && cat > {path}.new.$$ && mv {path}.new.$$ {path}");
        let out = session.exec(&wire(&cmd), Some(content.as_bytes()), Duration::from_secs(20)).await?;
        if out.code != Some(0) {
            return Err(format!("Couldn't copy {name} to {node}: {}", out.stderr.trim()));
        }
        return Ok(true);
    }
    Ok(false)
}

/// Where root's verified copy of sparkjob.py lives on a node.
pub const ROOT_LIB: &str = "/usr/local/lib/sparkbox";
/// The exit status of a root run that found its copy of sparkjob.py missing or different.
pub const ROOT_COPY_STALE: u32 = 97;

/// Root runs sparkjob.py only from `lib` (root-owned), only when its SHA-256 is `sha`, and
/// with `python3 -I` (no user site-packages, no PYTHON* variables, not the current folder).
pub fn root_sparkjob_command(lib: &str, sha: &str, args: &str) -> String {
    let f = sh_quote(&format!("{lib}/sparkjob.py"));
    format!("[ \"$(sha256sum < {f} 2>/dev/null | cut -c1-64)\" = {sha} ] || exit {ROOT_COPY_STALE}; exec python3 -I {f} {args}")
}

/// Puts SparkBox's sparkjob.py (on stdin) in `lib` as root: owned by root, 0644, checked
/// against `sha` before it replaces the old copy.
pub fn root_install_command(lib: &str, sha: &str) -> String {
    let d = sh_quote(lib);
    format!(
        "umask 022 && mkdir -p {d} && chown 0:0 {d} && chmod 0755 {d} && t=$(mktemp {d}/.sparkjob.XXXXXX) && cat > \"$t\" \
         && [ \"$(sha256sum < \"$t\" | cut -c1-64)\" = {sha} ] && chown 0:0 \"$t\" && chmod 0644 \"$t\" && mv -f \"$t\" {d}/sparkjob.py \
         || {{ rm -f \"$t\"; echo 'The copy of sparkjob.py arrived damaged; nothing was replaced.' >&2; exit 1; }}"
    )
}

/// sparkjob prints one JSON value; a failure says why on its last stderr line.
fn parse_sparkjob(node_id: &str, out: &ssh::Output) -> Result<serde_json::Value, String> {
    if out.code != Some(0) {
        let why = last_line(&out.stderr);
        let why = if why.is_empty() { last_line(&out.stdout) } else { why };
        return Err(match &out.signal {
            Some(sig) => format!("{node_id}: sparkjob was killed (SIG{sig})"),
            None if out.code.is_none() => format!("{node_id}: sparkjob ended without reporting success"),
            None => format!("{node_id}: {why}"),
        });
    }
    serde_json::from_str(&last_line(&out.stdout)).map_err(|e| format!("{node_id}: unexpected reply from sparkjob ({e})"))
}

/// Runs `cmd` under sudo on a session, sending the password only to sudo's own one-off prompt.
/// A second prompt means the password was refused. (`sudo_exchange_on` without its row: how the
/// staging and exit tests drive the exchange on a session of their own.)
#[cfg(test)]
pub(crate) async fn sudo_exchange(session: &ssh::Session, node_id: &str, cmd: &str, password: &str, input: Option<&[u8]>, limit: Duration) -> Result<ssh::Output, String> {
    sudo_exchange_on(session, node_id, cmd, password, input, limit, &|| {}).await
}

/// Runs `cmd` under sudo on a session, sending the password only to sudo's own one-off prompt,
/// and calls `sent` once sudo runs on the node (a drop from then on sends it TERM). A second
/// prompt means the password was refused.
async fn sudo_exchange_on(session: &ssh::Session, node_id: &str, cmd: &str, password: &str, input: Option<&[u8]>, limit: Duration, sent: &(dyn Fn() + Sync)) -> Result<ssh::Output, String> {
    let prompt = format!("SPARKBOX-SUDO-{}:", nonce()?);
    let (mut read, write, _slot) = session.exec_io(&sudo_wire(cmd, &prompt)).await?;
    // Dropped part-way (its caller stopped waiting), sudo and its command get TERM.
    let write = ssh::CloseOnDrop::term(write);
    sent();
    let (mut stdout, mut stderr, mut code) = (Vec::new(), String::new(), None);
    let mut signal = None;
    let mut prompted = false;
    let mut ended = false;

    // 1. Wait for sudo's own prompt. Anything else first (output, an exit) means it isn't asking.
    let deadline = tokio::time::Instant::now() + SUDO_PROMPT_WAIT;
    loop {
        match tokio::time::timeout_at(deadline, read.wait()).await {
            Err(_) => break,
            Ok(None) | Ok(Some(ChannelMsg::Close)) => {
                ended = true;
                break;
            }
            Ok(Some(ChannelMsg::ExtendedData { data, .. })) => {
                stderr.push_str(&String::from_utf8_lossy(&data));
                if let Some(i) = stderr.find(&prompt) {
                    stderr.replace_range(i..i + prompt.len(), "");
                    prompted = true;
                    break;
                }
            }
            Ok(Some(ChannelMsg::Data { data })) => {
                stdout.extend_from_slice(&data);
                break;
            }
            Ok(Some(ChannelMsg::ExitStatus { exit_status })) => code = Some(exit_status),
            Ok(Some(ChannelMsg::ExitSignal { signal_name, .. })) => signal = Some(ssh::sig_name(&signal_name)),
            Ok(Some(_)) => {}
        }
    }
    if prompted {
        write.get().data(format!("{password}\n").as_bytes()).await.map_err(|e| e.to_string())?;
    }
    // The command's own input goes after the password line, never before sudo has asked.
    if let (Some(data), false) = (input, ended) {
        write.get().data(data).await.map_err(|e| e.to_string())?;
    }
    let _ = write.get().eof().await;

    // 2. The rest of the run.
    let rest = if ended { ssh::Output::default() } else { ssh::collect(&mut read, write, limit, &CancelToken::new(), &mut |_, _| {}).await? };
    stdout.extend_from_slice(rest.stdout.as_bytes());
    stderr.push_str(&rest.stderr);
    let out = ssh::Output {
        code: rest.code.or(code),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: stderr.replace(&prompt, "").replace("Sorry, try again.\n", ""),
        timed_out: rest.timed_out,
        truncated: rest.truncated,
        cancelled: false,
        signal: rest.signal.or(signal),
    };
    // A second prompt means the first password was refused.
    if prompted && (stderr.contains(&prompt) || stderr.contains("incorrect password")) {
        return Err(breaker_message(node_id));
    }
    if stderr.contains("is not in the sudoers file") {
        return Err(format!("The cluster user can't use sudo on {node_id}."));
    }
    if out.timed_out {
        return Err(ssh::took_longer(cmd, limit));
    }
    Ok(out)
}

/// How one connection ended, and why in a few words (the SSH log, interrupted commands).
enum Ended {
    /// The node closed it, keepalive was lost, or it stopped answering the 15-second check.
    Closed { why: String },
    /// SparkBox dropped it to reconnect at once (no reply, sleep, VPN path change): `shown` on the node.
    Reconnect { shown: String, why: String },
}

/// Why one collector run is over while its connection is still up.
enum Gone {
    /// No frame for DROP_AFTER.
    Stall(Duration),
    /// It ended: exited, or was killed.
    Exit(ssh::StreamEnd),
    /// No channel came free for it.
    Busy,
}

/// "exit 1: <its last stderr line>", "killed (SIGKILL)" or "no exit status".
fn exit_reason(end: &ssh::StreamEnd) -> String {
    match (end.code, &end.signal) {
        (_, Some(sig)) => format!("killed (SIG{sig})"),
        (Some(c), None) => match last_line(&end.stderr) {
            l if l.is_empty() => format!("exit {c}"),
            l => format!("exit {c}: {l}"),
        },
        (None, None) => "no exit status".into(),
    }
}

impl Live {
    pub fn new(
        vpn: Arc<Vpn>,
        known_hosts: PathBuf,
        password: impl Fn() -> Result<Option<String>, String> + Send + Sync + 'static,
    ) -> Arc<Live> {
        Arc::new(Live {
            audit: audit::Log::new(),
            vpn,
            password: Box::new(password),
            nodes: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            watcher: Mutex::new(None),
            generations: AtomicU64::new(0),
            interrupted: Mutex::new(HashSet::new()),
            sshlog: sshlog::SshLog::default(),
            homes: Mutex::new(HashMap::new()),
            breaker: Mutex::new(HashMap::new()),
            login_path: known_hosts.with_file_name("ssh-login.json"),
            // Tests never read the Mac's own ~/.ssh/config or agent.
            ssh_config: Mutex::new(if cfg!(test) { None } else { Some(sshconfig::home().join(".ssh/config")) }),
            agent_override: Mutex::new(if cfg!(test) { Some(None) } else { None }),
            bulk_lock: tokio::sync::Mutex::new(()),
            root_lib: Mutex::new(if cfg!(test) { std::env::temp_dir().join(format!("sparkbox-rootlib-{}", std::process::id())).display().to_string() } else { ROOT_LIB.into() }),
            exit: exit::Exit::default(),
            reads: Reads::default(),
            known_hosts,
        })
    }

    /// Connects to the nodes in the list, keeping what's already right: a node whose config
    /// didn't change keeps its loop, its connections, terminals and transfers; a changed node
    /// is connected again, a removed one is stopped and an added one started.
    pub fn start(self: &Arc<Self>, nodes: &[NodeConfig]) {
        let mut tasks = self.tasks.lock().unwrap();
        let mut watcher = self.watcher.lock().unwrap();
        if watcher.is_none() {
            // Live data starts (SparkBox launched, or Demo → Live): logged before any node's
            // loop runs, so the connects follow it.
            self.sshlog.event("mac", "live_start", &[("nodes", &nodes.len())]);
        }
        let stale: Vec<String> = tasks.iter().filter(|(id, (cfg, _))| !nodes.iter().any(|n| n.id == **id && same_config(n, cfg))).map(|(id, _)| id.clone()).collect();
        for id in stale {
            if let Some((_, task)) = tasks.remove(&id) {
                task.abort();
            }
            let why = if nodes.iter().any(|n| n.id == id) { format!("{id}'s settings changed") } else { format!("{id} was removed from the inventory") };
            self.stop_node(&id, &why);
        }
        *self.configs.lock().unwrap() = nodes.iter().map(|n| (n.id.clone(), n.clone())).collect();
        for cfg in nodes {
            if tasks.contains_key(&cfg.id) {
                // Unchanged: one waiting between attempts tries again now.
                if let Some(s) = self.nodes.lock().unwrap().get(&cfg.id).filter(|s| s.session.is_none()) {
                    s.retry.notify_one();
                }
                continue;
            }
            let generation = self.generations.fetch_add(1, Ordering::SeqCst) + 1;
            self.nodes.lock().unwrap().insert(cfg.id.clone(), NodeState::new(generation));
            let me = self.clone();
            let c = cfg.clone();
            tasks.insert(cfg.id.clone(), (cfg.clone(), tauri::async_runtime::spawn(async move { me.run_node(c, generation).await })));
        }
        if watcher.is_none() {
            let me = self.clone();
            *watcher = Some(tauri::async_runtime::spawn(async move { me.watch_sleep_and_path().await }));
        }
    }

    /// Stops every node's loop and closes its connections (Live → Demo).
    pub fn stop(&self) {
        for (_, (_, task)) in self.tasks.lock().unwrap().drain() {
            task.abort();
        }
        if let Some(w) = self.watcher.lock().unwrap().take() {
            w.abort();
            self.sshlog.event("mac", "live_stop", &[]);
        }
        let ids: Vec<String> = self.nodes.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop_node(&id, "Live data stopped");
        }
    }

    /// Forgets one node (its loop is already stopped): ends its running commands' activity
    /// entries as interrupted and closes all its connections.
    fn stop_node(&self, id: &str, why: &str) {
        let Some(st) = self.nodes.lock().unwrap().remove(id) else { return };
        self.homes.lock().unwrap().remove(id);
        self.interrupt_entries(st.open.into_keys().collect(), why);
        let sessions: Vec<(Arc<ssh::Session>, &str)> =
            st.session.map(|s| (s, "control")).into_iter().chain(st.bulk.map(|s| (s, "bulk"))).chain(st.extra.into_iter().map(|(s, _)| (s, "bulk-extra"))).collect();
        for (_, conn) in &sessions {
            self.sshlog.event(id, "session_closed", &[("conn", conn), ("reason", &why)]);
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            for (s, _) in sessions {
                rt.spawn(async move { s.close().await });
            }
        }
    }

    /// Ends these running commands' activity entries: their connection is being closed.
    fn interrupt_entries(&self, ids: Vec<String>, why: &str) {
        for id in ids {
            self.interrupted.lock().unwrap().insert(id.clone());
            self.audit.end(&id, audit::Ending { error: Some(format!("Interrupted: the connection was reset ({why})")), ..Default::default() });
        }
    }

    /// Ends the activity entries of the commands running on one connection.
    fn interrupt(&self, node_id: &str, session: &Arc<ssh::Session>, why: &str) {
        let conn = Arc::as_ptr(session) as usize;
        let ids: Vec<String> = match self.nodes.lock().unwrap().get_mut(node_id) {
            Some(s) => {
                let ids: Vec<String> = s.open.iter().filter(|(_, c)| **c == conn).map(|(id, _)| id.clone()).collect();
                for id in &ids {
                    s.open.remove(id);
                }
                ids
            }
            None => Vec::new(),
        };
        self.interrupt_entries(ids, why);
    }

    /// Notes that a logged command runs on `session`, so a reconnect can mark it interrupted.
    fn track(&self, node_id: &str, session: &Arc<ssh::Session>, row: &Row<'_>) {
        if let (Some(id), Some(s)) = (row.id(), self.nodes.lock().unwrap().get_mut(node_id)) {
            s.open.insert(id.to_string(), Arc::as_ptr(session) as usize);
        }
    }

    /// How a node's connections use their SSH channels: control and every bulk one.
    pub fn channel_stats(&self, node_id: &str) -> ssh::ChannelStats {
        let nodes = self.nodes.lock().unwrap();
        let Some(s) = nodes.get(node_id) else { return ssh::ChannelStats::default() };
        let mut total = ssh::ChannelStats::default();
        for c in s.session.iter().chain(s.bulk.iter()).chain(s.extra.iter().map(|(c, _)| c)) {
            total += c.stats();
        }
        total.bulk_connections = usize::from(s.bulk.is_some()) + s.extra.len();
        total
    }

    /// `channel_stats` for every node, by id.
    pub fn all_channel_stats(&self) -> Vec<(String, ssh::ChannelStats)> {
        let mut ids: Vec<String> = self.nodes.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids.into_iter().map(|id| {
            let s = self.channel_stats(&id);
            (id, s)
        }).collect()
    }

    pub fn snapshot(&self, inv: &Inventory) -> ClusterSnapshot {
        let nodes = self.nodes.lock().unwrap();
        let mut out: Vec<NodeSnapshot> = inv.nodes.iter().map(|cfg| node_snapshot(cfg, nodes.get(&cfg.id))).collect();
        share_host_keys(&mut out);
        ClusterSnapshot {
            ts_ms: now_ms(),
            demo: false,
            nodes: out,
            switches: inv.switches.iter().map(placeholder_switch).collect(),
            ..Default::default()
        }
    }

    /// Changes the node's state, only while it still belongs to loop `generation`.
    fn update_as(&self, id: &str, generation: u64, f: impl FnOnce(&mut NodeState)) {
        if let Some(s) = self.nodes.lock().unwrap().get_mut(id).filter(|s| s.generation == generation) {
            f(s);
        }
    }

    /// The node's control connection is up.
    pub fn is_connected(&self, id: &str) -> bool {
        self.session(id).is_some()
    }

    fn session(&self, id: &str) -> Option<Arc<ssh::Session>> {
        self.nodes.lock().unwrap().get(id).and_then(|s| s.session.clone()).filter(|s| !s.is_closed())
    }

    /// Reconnects every node now (the login settings changed).
    pub fn reconnect_all(&self) {
        self.reconnect_every("the login settings changed");
    }

    /// Reconnects every node's connection (the one up, or being made) now, saying why, and
    /// has a node waiting between attempts try again now.
    fn reconnect_every(&self, why: &str) {
        for s in self.nodes.lock().unwrap().values() {
            s.kick.send_replace(why.to_string());
            if s.session.is_none() {
                s.retry.notify_one();
            }
        }
    }

    /// Runs a command on a node over its control connection, exactly as written (`wire`),
    /// and logs it: a line before it starts, completed when it ends. The same read asked again
    /// while it runs shares that run and its line (`Reads`).
    pub async fn run(&self, node_id: &str, cmd: &str, stdin: Option<&[u8]>, limit: Duration) -> Result<ssh::Output, String> {
        self.run_on_admitted(node_id, cmd, stdin, limit, || {}).await
    }

    /// Run a command, changing local operation state only after the write exit admits it.
    /// The callback runs before any SSH execution, never on a refusal. A restart uses this
    /// boundary to retire its old generation without changing a load that was never stopped.
    pub(crate) async fn run_on_admitted(&self, node_id: &str, cmd: &str, stdin: Option<&[u8]>, limit: Duration, on_admitted: impl FnOnce()) -> Result<ssh::Output, String> {
        let admitted = self.exit.admit(self, node_id, exit::ExecKind::Run, cmd).await.map_err(|e| e.to_json())?;
        on_admitted();
        if shareable(exit::ExecKind::Run, cmd, &admitted, stdin) {
            return self.shared(node_id, "run", cmd, limit, || self.run_logged(node_id, cmd, None, limit, &admitted)).await;
        }
        self.reads.changed(node_id);
        let result = self.run_logged(node_id, cmd, stdin, limit, &admitted).await;
        self.reads.changed(node_id);
        result
    }

    /// `run` once admitted: its activity line, the exec on the control connection.
    async fn run_logged(&self, node_id: &str, cmd: &str, stdin: Option<&[u8]>, limit: Duration, admitted: &exit::Admitted) -> Result<ssh::Output, String> {
        let (kind, shown) = match cmd.strip_prefix("sudo -k -S -p '' ") {
            Some(rest) => ("sudo", rest),
            None => ("command", cmd),
        };
        let session = match self.session(node_id) {
            Some(s) => s,
            None => {
                if audit::in_probe() {
                    // A background check of a node that isn't connected is a failed check, not a failed change.
                    self.audit.record(audit::Entry { node: node_id.into(), kind: kind.into(), command: shown.into(), error: Some(not_connected(node_id)), state: "failed".into(), ..Default::default() });
                } else {
                    self.audit.record_failed(node_id, "", shown, &not_connected(node_id));
                }
                return Err(not_connected(node_id));
            }
        };
        let started = Instant::now();
        let row = self.log_begin(node_id, kind, shown, admitted.fence.as_deref());
        self.track(node_id, &session, &row);
        let wired = wire(cmd);
        let result = exec_marked(&session, &wired, stdin, limit, &CancelToken::new(), &row).await.and_then(|o| if o.timed_out { Err(ssh::took_longer(&wired, limit)) } else { Ok(o) });
        self.log_end(row, started, &result);
        result
    }

    /// Like `run`, for SparkBox's own read-only polling: not in the activity log. The same probe
    /// asked by another caller within 2 s of the last one finishing gets its result.
    pub async fn probe(&self, node_id: &str, cmd: &str, limit: Duration) -> Result<ssh::Output, String> {
        self.probe_as(node_id, cmd, limit, true).await
    }

    /// `probe` for a check whose answer must be new (is the process still there, a moment after
    /// it was sent TERM?): never answered from a finished probe's result. The same probe still
    /// running is shared.
    pub async fn probe_fresh(&self, node_id: &str, cmd: &str, limit: Duration) -> Result<ssh::Output, String> {
        self.probe_as(node_id, cmd, limit, false).await
    }

    async fn probe_as(&self, node_id: &str, cmd: &str, limit: Duration, reuse: bool) -> Result<ssh::Output, String> {
        let admitted = self.exit.admit(self, node_id, exit::ExecKind::Probe, cmd).await.map_err(|e| e.to_json())?;
        let exec = || async move {
            let session = self.session(node_id).ok_or_else(|| not_connected(node_id))?;
            session.exec(&wire(cmd), None, limit).await
        };
        if !shareable(exit::ExecKind::Probe, cmd, &admitted, None) {
            return exec().await;
        }
        if let Some(out) = self.reads.recent(node_id, cmd).filter(|_| reuse) {
            return Ok(out);
        }
        let epoch = self.reads.epoch(node_id);
        let result = self
            .shared(node_id, "probe", cmd, limit, || async move {
                let result = exec().await;
                if let Ok(out) = &result {
                    self.reads.remember(node_id, cmd, epoch, out);
                }
                result
            })
            .await;
        self.reads.had(node_id, cmd);
        result
    }

    /// Runs `exec` for a read, unless the same read on the same node, with the same time limit,
    /// is already running: then waits for that one's result. When the caller running it stops
    /// waiting, the others run it themselves.
    async fn shared<F, Fut>(&self, node_id: &str, how: &'static str, cmd: &str, limit: Duration, exec: F) -> Shared
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Shared>,
    {
        loop {
            let key: ReadKey = (node_id.to_string(), how, cmd.to_string(), limit, self.reads.epoch(node_id));
            let lead = {
                let mut flights = self.reads.flights.lock().unwrap();
                match flights.get(&key) {
                    Some(rx) => Err(rx.clone()),
                    None => {
                        let (tx, rx) = watch::channel(None);
                        flights.insert(key.clone(), rx);
                        Ok(tx)
                    }
                }
            };
            match lead {
                Ok(tx) => {
                    let flight = Flight { reads: &self.reads, key };
                    let result = exec().await;
                    // Out of the flights first: a read asked from now on runs afresh, only those
                    // already waiting get this result.
                    drop(flight);
                    tx.send_replace(Some(result.clone()));
                    return result;
                }
                Err(mut rx) => loop {
                    let got = rx.borrow_and_update().clone();
                    if let Some(result) = got {
                        return result;
                    }
                    if rx.changed().await.is_err() {
                        break; // it stopped without a result: run it here
                    }
                },
            }
        }
    }

    /// Runs a command that can be stopped: `cancel` (or the time limit) sends it TERM and
    /// closes the channel. Output is capped at 1 MiB per stream; the result says what happened.
    /// Fleet, the Fabric doctor and other long operations use it.
    pub async fn run_cancellable(
        &self,
        node_id: &str,
        cmd: &str,
        stdin: Option<&[u8]>,
        limit: Duration,
        cancel: CancelToken,
    ) -> Result<ssh::Output, String> {
        let admitted = self.exit.admit(self, node_id, exit::ExecKind::Cancellable, cmd).await.map_err(|e| e.to_json())?;
        let session = self.session(node_id).ok_or_else(|| not_connected(node_id))?;
        self.reads.changed(node_id);
        let started = Instant::now();
        let row = self.log_begin(node_id, "command", cmd, admitted.fence.as_deref());
        self.track(node_id, &session, &row);
        let result = exec_marked(&session, &wire(cmd), stdin, limit, &cancel, &row).await;
        let shown = result.as_ref().map_err(|e| e.clone()).and_then(|o| match (o.cancelled, o.timed_out) {
            (true, _) => Err("Stopped".into()),
            (_, true) => Err(format!("Took longer than {} s; stopped", limit.as_secs())),
            _ => Ok(o.clone()),
        });
        self.log_end(row, started, &shown);
        self.reads.changed(node_id);
        result
    }

    /// Streams a command's output as it arrives (Fleet, logs), over the bulk connection.
    /// Stops it (TERM) on cancel, at the time limit, or once 1 MiB has been delivered (then
    /// `truncated` is set).
    pub async fn stream_cmd(
        &self,
        node_id: &str,
        cmd: &str,
        limit: Duration,
        cancel: CancelToken,
        mut on_chunk: impl FnMut(StreamKind, &[u8]) + Send,
    ) -> Result<ssh::Output, String> {
        let admitted = self.exit.admit(self, node_id, exit::ExecKind::Stream, cmd).await.map_err(|e| e.to_json())?;
        let (session, slot, channel) = self.bulk_channel(node_id, limit).await?;
        self.reads.changed(node_id);
        let deadline = tokio::time::Instant::now() + limit;
        let started = Instant::now();
        let row = self.log_begin(node_id, "stream", cmd, admitted.fence.as_deref());
        self.track(node_id, &session, &row);
        if channel.is_some() {
            row.sent();
        }
        // Our own stop, fired by the caller's token or by the cap.
        let stop = CancelToken::new();
        let relay = {
            let (cancel, stop) = (cancel.clone(), stop.clone());
            tokio::spawn(async move {
                cancel.cancelled().await;
                stop.cancel();
            })
        };
        let mut sent = 0usize;
        let mut capped = false;
        let mut deliver = |kind: StreamKind, data: &[u8]| {
            if capped {
                return;
            }
            let room = OUTPUT_CAP.saturating_sub(sent);
            on_chunk(kind, &data[..data.len().min(room)]);
            sent += data.len().min(room);
            if data.len() > room {
                capped = true;
                stop.cancel();
            }
        };
        let result = session.exec_in(channel, slot, &wire(cmd), None, deadline, &stop, &mut deliver).await;
        relay.abort();
        let result = result.map(|mut o| {
            o.cancelled = cancel.is_cancelled();
            o.truncated |= capped;
            o
        });
        let shown = result.as_ref().map_err(|e| e.clone()).and_then(|o| if o.cancelled { Err("Stopped".into()) } else { Ok(o.clone()) });
        self.log_end(row, started, &shown);
        self.reads.changed(node_id);
        result.map(|mut o| {
            o.stdout.clear();
            o.stderr.clear();
            o
        })
    }

    /// The cluster user's home folder on a node, asked once per connection (Files' `~`).
    pub async fn home_dir(&self, node_id: &str) -> Option<String> {
        if let Some(h) = self.homes.lock().unwrap().get(node_id) {
            return Some(h.clone());
        }
        let out = self.probe(node_id, "printf %s \"$HOME\"", Duration::from_secs(10)).await.ok()?;
        let home = out.stdout.trim().to_string();
        if out.code != Some(0) || !home.starts_with('/') {
            return None;
        }
        self.homes.lock().unwrap().insert(node_id.to_string(), home.clone());
        Some(home)
    }

    /// Writes a command's activity-log line before it runs (so a crash or a lost connection
    /// still leaves it, marked unfinished, and a run SparkBox stops waiting for leaves it
    /// cancelled). The latency ping isn't logged.
    fn log_begin(&self, node_id: &str, kind: &str, command: &str, fence: Option<&str>) -> Row<'_> {
        if command == "true" {
            return Row { live: self, begun: None };
        }
        self.row(audit::Entry {
            node: node_id.into(),
            kind: kind.into(),
            command: command.into(),
            changes: kind == "terminal" || kind == "stream" || !audit::read_only(command),
            fence: fence.map(String::from),
            ..Default::default()
        })
    }

    /// A command's activity row, begun now.
    fn row(&self, entry: audit::Entry) -> Row<'_> {
        Row { live: self, begun: Some(self.audit.begin_guarded(entry)) }
    }

    /// Completes a line from `log_begin` with how the command ended.
    fn log_end(&self, mut row: Row<'_>, started: Instant, result: &Result<ssh::Output, String>) {
        let Some(id) = row.begun.take().map(audit::Begun::into_id) else { return };
        if self.interrupted.lock().unwrap().remove(&id) {
            return; // already ended: its connection was reset under it
        }
        for s in self.nodes.lock().unwrap().values_mut() {
            s.open.remove(&id);
        }
        let outcome = match result {
            Ok(o) => audit::Ending {
                code: o.code,
                stdout_tail: audit::tail(&o.stdout),
                stderr_tail: audit::tail(&o.stderr),
                duration_ms: Some(started.elapsed().as_millis() as u64),
                error: match (&o.signal, o.code, o.timed_out) {
                    (_, _, true) => Some("Timed out; stopped".into()),
                    (Some(sig), _, _) => Some(format!("Killed (SIG{sig})")),
                    (None, None, _) => Some("Ended without an exit status".into()),
                    _ => None,
                },
                outcome: None,
            },
            Err(e) => audit::Ending { error: Some(e.clone()), duration_ms: Some(started.elapsed().as_millis() as u64), ..Default::default() },
        };
        self.audit.end(&id, outcome);
    }

    /// Adds a finished line to the activity log.
    pub fn log(&self, node_id: &str, kind: &str, command: &str, started: Instant, result: Result<Option<u32>, String>) {
        if command == "true" {
            return; // the 15-second latency ping
        }
        let (code, error) = match result {
            Ok(code) => (code, None),
            Err(e) => (None, Some(e)),
        };
        self.audit.record(audit::Entry {
            ts_ms: now_ms(),
            node: node_id.into(),
            kind: kind.into(),
            command: command.into(),
            code,
            ms: started.elapsed().as_millis() as u64,
            changes: kind == "terminal" || kind == "stream" || !audit::read_only(command),
            error,
            ..Default::default()
        });
    }

    /// Logs in to a node over a new SSH connection, runs `true` and hangs up: proof SparkBox
    /// could connect from scratch now (an open connection outlives a stopped sshd or a firewall
    /// that drops new ones). Not logged: Fleet records it as a step of its task.
    pub async fn reach_fresh(&self, node_id: &str) -> Result<(), String> {
        let cfg = self.configs.lock().unwrap().get(node_id).cloned().ok_or_else(|| not_connected(node_id))?;
        let session = tokio::time::timeout(Duration::from_secs(15), self.connect(&cfg, "fresh")).await.map_err(|_| "no answer in 15 s".to_string())??;
        let out = session.exec(&wire("true"), None, Duration::from_secs(10)).await;
        session.close().await;
        self.sshlog.event(node_id, "session_closed", &[("conn", &"fresh"), ("reason", &"the check is done")]);
        match out {
            Ok(o) if o.code == Some(0) => Ok(()),
            Ok(o) => Err(format!("logged in, but a command ended with {}", o.code.map(|c| c.to_string()).unwrap_or_else(|| "no exit status".into()))),
            Err(e) => Err(e),
        }
    }

    /// The node's bulk connection (terminals, logs, transfers, tails), made on first use.
    pub(crate) async fn bulk(&self, node_id: &str) -> Result<Arc<ssh::Session>, String> {
        let current = || self.nodes.lock().unwrap().get(node_id).and_then(|s| s.bulk.clone()).filter(|s| !s.is_closed());
        if let Some(s) = current() {
            return Ok(s);
        }
        let _one = self.bulk_lock.lock().await;
        if let Some(s) = current() {
            return Ok(s);
        }
        // Only for a node whose control connection is up: the same checks have passed.
        let generation = self.nodes.lock().unwrap().get(node_id).filter(|s| s.session.as_ref().is_some_and(|c| !c.is_closed())).map(|s| s.generation);
        let generation = generation.ok_or_else(|| not_connected(node_id))?;
        let cfg = self.configs.lock().unwrap().get(node_id).cloned().ok_or_else(|| not_connected(node_id))?;
        let session = Arc::new(self.connect(&cfg, "bulk").await?);
        self.adopt(node_id, generation, "bulk", &session, |s| {
            s.bulk = Some(session.clone());
            self.sshlog.event(node_id, "bulk_open", &[]);
        })
        .await?;
        Ok(session)
    }

    /// Keeps a connection just made for a node (`keep` puts it in the node's state) while that
    /// state is still the one it was made for (`generation`). Stopped meanwhile (Live data
    /// stopped, the node removed or its settings changed), the node gets it closed instead:
    /// nothing runs on a connection that stopping it would have missed.
    async fn adopt(&self, node_id: &str, generation: u64, conn: &str, session: &ssh::Session, keep: impl FnOnce(&mut NodeState)) -> Result<(), String> {
        let kept = match self.nodes.lock().unwrap().get_mut(node_id).filter(|s| s.generation == generation) {
            Some(s) => {
                keep(s);
                true
            }
            None => false,
        };
        if kept {
            return Ok(());
        }
        self.sshlog.event(node_id, "session_closed", &[("conn", &conn), ("reason", &"the node was stopped while it connected")]);
        session.close().await;
        Err(not_connected(node_id))
    }

    /// A place for a long-lived channel (a terminal, a follower, a transfer) on one of the node's
    /// bulk connections, leaving out those in `full` (the node refused a channel there): the
    /// first with room; else a new extra connection, up to MAX_BULK per node; else the first
    /// place to come free within 20 s on the main bulk connection (or the first extra one left).
    async fn bulk_slot(&self, node_id: &str, full: &[Arc<ssh::Session>]) -> Result<(Arc<ssh::Session>, ssh::Slot), String> {
        let usable = |c: &Arc<ssh::Session>| !full.iter().any(|f| Arc::ptr_eq(f, c));
        let main = self.bulk(node_id).await?;
        if usable(&main) {
            if let Some(slot) = main.try_slot() {
                return Ok((main, slot));
            }
        }
        let extras = || -> Vec<Arc<ssh::Session>> {
            self.nodes.lock().unwrap().get(node_id).map(|s| s.extra.iter().map(|(c, _)| c.clone()).filter(|c| !c.is_closed()).collect()).unwrap_or_default()
        };
        let free = |list: Vec<Arc<ssh::Session>>| list.into_iter().filter(|c| usable(c)).find_map(|c| c.try_slot().map(|slot| (c, slot)));
        if let Some(found) = free(extras()) {
            return Ok(found);
        }
        {
            let _one = self.bulk_lock.lock().await;
            let list = extras();
            let count = list.len();
            if let Some(found) = free(list) {
                return Ok(found); // made meanwhile
            }
            if count + 1 < MAX_BULK {
                let cfg = self.configs.lock().unwrap().get(node_id).cloned().ok_or_else(|| not_connected(node_id))?;
                let generation = self.nodes.lock().unwrap().get(node_id).map(|s| s.generation).ok_or_else(|| not_connected(node_id))?;
                let extra = Arc::new(self.connect(&cfg, "bulk-extra").await?);
                self.adopt(node_id, generation, "bulk-extra", &extra, |s| {
                    s.extra.push((extra.clone(), Instant::now()));
                    self.sshlog.event(node_id, "bulk_extra_open", &[("bulk_connections", &(count + 2))]);
                })
                .await?;
                let slot = extra.try_slot().ok_or_else(|| ssh::busy_message(node_id))?;
                return Ok((extra, slot));
            }
        }
        let wait_on = std::iter::once(main).chain(extras()).find(|c| usable(c)).ok_or_else(|| ssh::busy_message(node_id))?;
        let slot = wait_on.slot().await?;
        Ok((wait_on, slot))
    }

    /// A channel for a long-lived command on one of the node's bulk connections, opened within
    /// `limit` (None: the node didn't answer in time), with its place there. A connection whose
    /// node refuses it although SparkBox counted a free place (channels still closing there, a
    /// lower MaxSessions) is full for now: another one is tried, an extra one made when there's
    /// room, before the open fails as busy.
    async fn bulk_channel(&self, node_id: &str, limit: Duration) -> Result<(Arc<ssh::Session>, ssh::Slot, Option<russh::Channel<russh::client::Msg>>), String> {
        let mut full: Vec<Arc<ssh::Session>> = Vec::new();
        loop {
            let (session, slot) = self.bulk_slot(node_id, &full).await?;
            match session.open_on(&slot, limit).await {
                Ok(channel) => return Ok((session, slot, channel)),
                Err(e) if ssh::is_busy(&e) && full.len() + 1 < MAX_BULK => {
                    self.sshlog.event(node_id, "bulk_full", &[("in_use", &session.stats().in_use)]);
                    full.push(session);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Whether a node's bulk connection is carrying anything (a terminal, a transfer, a tail).
    pub fn bulk_busy(&self, node_id: &str) -> bool {
        self.nodes.lock().unwrap().get(node_id).and_then(|s| s.bulk.as_ref()).is_some_and(|b| Arc::strong_count(b) > 1)
    }

    /// Closes the extra bulk connections that carried nothing for BULK_IDLE (and forgets dead ones).
    fn close_idle_bulk(&self) {
        let mut idle = Vec::new();
        for (id, s) in self.nodes.lock().unwrap().iter_mut() {
            s.extra.retain_mut(|(c, last_used)| {
                if c.is_closed() {
                    return false;
                }
                if Arc::strong_count(c) > 1 || c.stats().in_use > 0 {
                    *last_used = Instant::now();
                    return true;
                }
                if last_used.elapsed() < BULK_IDLE {
                    return true;
                }
                idle.push((id.clone(), c.clone()));
                false
            });
        }
        for (id, c) in idle {
            self.sshlog.event(&id, "session_closed", &[("conn", &"bulk-extra"), ("reason", &format!("unused for {} s", BULK_IDLE.as_secs()))]);
            tokio::spawn(async move { c.close().await });
        }
    }

    /// Opens a terminal on one of the node's bulk connections (an extra one when the others
    /// have no room: sshd allows 10 channels per connection by default).
    pub async fn pty(&self, node_id: &str, cols: u32, rows: u32, command: Option<&str>) -> Result<Pty, String> {
        // Terminals log the tab themselves (one entry from open to close, term/mod.rs).
        self.exit.admit(self, node_id, exit::ExecKind::Pty, command.unwrap_or("")).await.map_err(|e| e.to_json())?;
        self.reads.changed(node_id);
        let (session, slot, channel) = self.bulk_channel(node_id, ssh::OPEN_LIMIT).await?;
        let channel = channel.ok_or_else(|| session.not_opened())?;
        let (read, write) = session.pty_in(channel, cols, rows, command).await?;
        Ok(Pty { read, write: Some(write), hold: Some(Hold { _session: session, _slot: slot }) })
    }

    /// A TCP stream to `host:port` as seen from the node, over its bulk connection.
    pub async fn direct_tcpip(&self, node_id: &str, host: &str, port: u16, local_port: u16) -> Result<russh::ChannelStream<russh::client::Msg>, String> {
        self.exit.admit(self, node_id, exit::ExecKind::DirectTcpip, &format!("{host}:{port}")).await.map_err(|e| e.to_json())?;
        let session = self.bulk(node_id).await?;
        session.direct_tcpip(host, port, local_port).await
    }

    /// Starts `cmd` on a node for streaming its stdin/stdout (file copies), on the bulk connection.
    pub async fn exec_io(&self, node_id: &str, cmd: &str) -> Result<Exec, String> {
        self.exit.admit(self, node_id, exit::ExecKind::ExecIo, cmd).await.map_err(|e| e.to_json())?;
        self.reads.changed(node_id);
        let (session, slot, channel) = self.bulk_channel(node_id, ssh::OPEN_LIMIT).await?;
        let channel = channel.ok_or_else(|| session.not_opened())?;
        self.log(node_id, "stream", cmd, Instant::now(), Ok(None));
        let (read, write) = session.exec_io_in(channel, &wire(cmd)).await?;
        Ok(Exec { read, write, hold: Some(Hold { _session: session, _slot: slot }) })
    }

    /// Runs `sparkjob.py <args>` on a node (installing it first if needed) and parses its JSON
    /// reply. A copy found changed as it starts (COPY_STALE: nothing ran) is put back and the
    /// command run once more.
    pub async fn sparkjob(&self, node_id: &str, args: &str, stdin: Option<&[u8]>, limit: Duration) -> Result<serde_json::Value, String> {
        self.exit.admit(self, node_id, exit::ExecKind::Sparkjob, args).await.map_err(|e| e.to_json())?;
        let cmd = format!("{SPARKJOB_RUN}{args}");
        self.ensure_file(node_id, "sparkjob.py", SPARKJOB).await?;
        let mut out = self.run(node_id, &cmd, stdin, limit).await?;
        if out.code == Some(COPY_STALE) {
            self.ensure_file(node_id, "sparkjob.py", SPARKJOB).await?;
            out = self.run(node_id, &cmd, stdin, limit).await?;
        }
        parse_sparkjob(node_id, &out)
    }

    /// Runs `sparkjob.py <args>` as root. Root never runs Python from a folder the cluster user
    /// can write: it runs `python3 -I` on a root-owned copy in /usr/local/lib/sparkbox whose
    /// SHA-256 is checked in the same sudo shell, and puts SparkBox's copy there first when it
    /// differs (or is missing).
    pub async fn sparkjob_root(&self, node_id: &str, args: &str, limit: Duration) -> Result<serde_json::Value, String> {
        self.exit.admit(self, node_id, exit::ExecKind::SparkjobRoot, args).await.map_err(|e| e.to_json())?;
        let sha = sha256_hex(SPARKJOB.as_bytes());
        let run = root_sparkjob_command(&self.root_lib(), &sha, args);
        let mut out = self.sudo(node_id, &run, limit).await?;
        if out.code == Some(ROOT_COPY_STALE) {
            let install = root_install_command(&self.root_lib(), &sha);
            let installed = exit::as_class(fence_class::FenceClass::RootHelper, self.sudo_with_input(node_id, &install, SPARKJOB.as_bytes(), Duration::from_secs(60))).await;
            installed.and_then(|o| match o.failure("Installing sparkjob.py", node_id) {
                None => Ok(()),
                Some(why) => Err(why),
            })?;
            out = self.sudo(node_id, &run, limit).await?;
        }
        parse_sparkjob(node_id, &out)
    }

    /// Where root's verified copy of sparkjob.py lives (a temporary folder in tests).
    pub fn root_lib(&self) -> String {
        self.root_lib.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub fn set_root_lib(&self, dir: &str) {
        *self.root_lib.lock().unwrap() = dir.to_string();
    }

    /// Where detached upgrades write their logs on a node (a temporary folder in tests).
    pub fn upgrade_log_dir(&self) -> String {
        if cfg!(test) {
            format!("{}/logs", self.root_lib())
        } else {
            "/var/log/sparkbox".into()
        }
    }

    /// A file in SparkBox's own folder on this Mac (next to the known hosts).
    pub fn data_path(&self, name: &str) -> PathBuf {
        self.known_hosts.with_file_name(name)
    }

    /// Clears tripped sudo breakers (the password was changed in Settings).
    pub fn reset_sudo_breaker(&self) {
        self.breaker.lock().unwrap().clear();
    }

    /// Whether sudo may try `hash` on `node_id`: not if this node rejected it, nor once two nodes have.
    fn breaker_check(&self, node_id: &str, hash: &str) -> Result<(), String> {
        let mut breaker = self.breaker.lock().unwrap();
        // A password changed since it was rejected gets another chance.
        breaker.retain(|_, h| h == hash);
        let mut tripped: Vec<String> = breaker.keys().cloned().collect();
        tripped.sort();
        if tripped.len() >= 2 {
            return Err(breaker_message_all(&tripped));
        }
        if breaker.contains_key(node_id) {
            return Err(breaker_message(node_id));
        }
        Ok(())
    }

    /// Runs one command with sudo, giving it the cluster password (from the Keychain) only
    /// when sudo itself asks for it with SparkBox's one-off prompt.
    pub async fn sudo(&self, node_id: &str, cmd: &str, limit: Duration) -> Result<ssh::Output, String> {
        self.sudo_inner(node_id, cmd, None, limit).await
    }

    /// `sudo` with input for the command after the password line.
    pub async fn sudo_with_input(&self, node_id: &str, cmd: &str, input: &[u8], limit: Duration) -> Result<ssh::Output, String> {
        self.sudo_inner(node_id, cmd, Some(input), limit).await
    }

    async fn sudo_inner(&self, node_id: &str, cmd: &str, input: Option<&[u8]>, limit: Duration) -> Result<ssh::Output, String> {
        let kind = if input.is_some() { exit::ExecKind::SudoInput } else { exit::ExecKind::Sudo };
        let admitted = self.exit.admit(self, node_id, kind, cmd).await.map_err(|e| e.to_json())?;
        let password = match (self.password)()? {
            Some(p) => p,
            None => {
                let why = "Save the cluster password in Settings first.";
                self.audit.record_failed(node_id, "", &format!("sudo {cmd}"), why);
                return Err(why.into());
            }
        };
        let hash = sha256_hex(password.as_bytes());
        if let Err(e) = self.breaker_check(node_id, &hash) {
            self.audit.record_failed(node_id, "", &format!("sudo {cmd}"), &e);
            return Err(e);
        }
        let session = self.session(node_id).ok_or_else(|| not_connected(node_id))?;
        let row = self.log_begin(node_id, "sudo", cmd, admitted.fence.as_deref());
        self.sudo_logged(node_id, &session, cmd, &password, &hash, input, limit, row).await
    }

    /// The sudo exchange on `session` once the exit admitted it and the breaker let `hash`
    /// through, completing its activity-log line `row`; a rejected password trips the breaker.
    #[allow(clippy::too_many_arguments)]
    async fn sudo_logged(&self, node_id: &str, session: &Arc<ssh::Session>, cmd: &str, password: &str, hash: &str, input: Option<&[u8]>, limit: Duration, row: Row<'_>) -> Result<ssh::Output, String> {
        let started = Instant::now();
        self.reads.changed(node_id);
        self.track(node_id, session, &row);
        let result = sudo_exchange_on(session, node_id, cmd, password, input, limit, &|| row.sent()).await;
        if result.as_ref().err().is_some_and(|e| e.starts_with("The saved cluster password was rejected")) {
            self.breaker.lock().unwrap().insert(node_id.to_string(), hash.to_string());
        }
        self.log_end(row, started, &result);
        self.reads.changed(node_id);
        result
    }

    /// Copies `content` to ~/.sparkbox/<name> on the node unless the same file is already there.
    /// Checked once per connection: later calls on it skip the sha256sum round trip, and each
    /// sparkjob.py command checks the file itself as it starts (`wire`), so a copy another
    /// SparkBox put there since never runs, and has it checked here again.
    /// Outside the write exit, like `install`: copying SparkBox's own read-only helpers by
    /// checksum is the one write look-only allows.
    pub async fn ensure_file(&self, node_id: &str, name: &str, content: &str) -> Result<(), String> {
        let session = self.session(node_id).ok_or_else(|| format!("{node_id} isn't connected."))?;
        let fact = format!("{name} {}", sha256_hex(content.as_bytes()));
        if session.checked(&fact) {
            return Ok(());
        }
        install(&session, name, content, node_id).await?;
        session.mark_checked(&fact);
        Ok(())
    }

    /// `ensure_file` with no once-per-connection check, for launch.py: a container runs it as root,
    /// and nothing checks it as it starts, as `wire` checks sparkjob.py. Callers use a
    /// content-addressed filename; its checksum is still read every time. True when it copied the file. Only for a
    /// caller the exit has just admitted: it isn't a helper look-only lets through (exit.rs).
    pub async fn refresh_file(&self, node_id: &str, name: &str, content: &str) -> Result<bool, String> {
        let session = self.session(node_id).ok_or_else(|| format!("{node_id} isn't connected."))?;
        install(&session, name, content, node_id).await
    }

    /// How SparkBox reaches and logs in to a node: ~/.ssh/config, the login settings and the
    /// cluster password (only when there is one; keys alone are fine).
    fn plan(&self, cfg: &NodeConfig, password: Option<String>) -> (ssh::Target, ssh::Login, sshconfig::HostConfig) {
        let settings = self.login_settings();
        let blocks = match (&*self.ssh_config.lock().unwrap(), settings.use_ssh_config) {
            (Some(path), true) => sshconfig::load(path),
            _ => Vec::new(),
        };
        let addr = address(cfg);
        // ssh would be given the node's name; its address finds a block written for the IP.
        let mut host = sshconfig::resolve(&blocks, &cfg.host);
        if !host.explicit {
            let by_ip = sshconfig::resolve(&blocks, &addr);
            if by_ip.explicit {
                host = by_ip;
            }
        }
        let user = host.user.clone().filter(|_| host.explicit).unwrap_or_else(|| cfg.user.clone());
        let target_addr = host.hostname.clone().filter(|_| host.explicit).unwrap_or_else(|| addr.clone());
        let port = host.port.filter(|_| host.explicit).unwrap_or(cfg.port);
        let jump = host.proxy_jump.as_deref().map(|spec| {
            let (u, h, p) = sshconfig::parse_jump(spec);
            let jh = sshconfig::resolve(&blocks, &h);
            Box::new(ssh::Target {
                addr: jh.hostname.clone().unwrap_or(h),
                port: p.or(jh.port).unwrap_or(22),
                user: u.or(jh.user.clone()).unwrap_or_else(|| user.clone()),
                jump: None,
            })
        });
        let mut files: Vec<String> = host.identity_files.clone();
        if let Some(f) = settings.identity_file.clone().filter(|f| !f.trim().is_empty()) {
            files.insert(0, f);
        }
        if files.is_empty() && !host.identities_only && self.ssh_config.lock().unwrap().is_some() {
            // What ssh tries when nothing says otherwise.
            files = vec!["~/.ssh/id_ed25519".into(), "~/.ssh/id_ecdsa".into(), "~/.ssh/id_rsa".into()];
        }
        let keys = files
            .iter()
            .map(|f| {
                let full = sshconfig::expand(f, &target_addr, &user);
                let passphrase = crate::secrets::get(&sshconfig::passphrase_account(&full)).ok().flatten();
                ssh::KeyFile { shown: sshconfig::tilde(&full), path: PathBuf::from(full), passphrase }
            })
            .collect();
        let agent = match self.agent_override.lock().unwrap().clone() {
            Some(chosen) => chosen,
            None => sshconfig::agent_socket(&settings.agent),
        };
        let login = ssh::Login { agent, keys, identities_only: host.identities_only, password: password.filter(|_| settings.password_fallback) };
        (ssh::Target { addr: target_addr, port, user, jump }, login, host)
    }

    /// Settings › Cluster login.
    pub fn login_settings(&self) -> sshconfig::LoginSettings {
        sshconfig::load_settings(&self.login_path)
    }

    pub fn set_login_settings(&self, s: &sshconfig::LoginSettings) -> Result<(), String> {
        sshconfig::save_settings(&self.login_path, s)
    }

    /// Reads this file instead of ~/.ssh/config (None: none at all). Tests only.
    #[cfg(test)]
    pub fn set_ssh_config_path(&self, path: Option<PathBuf>) {
        *self.ssh_config.lock().unwrap() = path;
    }

    /// Uses this agent socket instead of the one the settings choose (None: no agent). Tests only.
    #[cfg(test)]
    pub fn set_agent_socket(&self, path: Option<PathBuf>) {
        *self.agent_override.lock().unwrap() = Some(path);
    }

    /// How each connected node's control connection logged in.
    pub fn logins(&self) -> HashMap<String, ssh::LoginInfo> {
        self.nodes.lock().unwrap().iter().filter_map(|(id, s)| s.login.clone().map(|l| (id.clone(), l))).collect()
    }

    /// What login SparkBox would try for a node, without connecting: (agent socket, key files,
    /// password fallback, notes).
    pub fn login_plan(&self, cfg: &NodeConfig) -> (Option<PathBuf>, Vec<String>, bool, Vec<String>, Option<String>) {
        let has_password = (self.password)().ok().flatten().is_some();
        let (target, login, host) = self.plan(cfg, has_password.then(String::new));
        let jump = target.jump.map(|j| format!("{}@{}:{}", j.user, j.addr, j.port));
        (login.agent, login.keys.into_iter().filter(|k| k.path.exists()).map(|k| k.shown).collect(), login.password.is_some(), host.notes, jump)
    }

    async fn connect(&self, cfg: &NodeConfig, conn: &str) -> Result<ssh::Session, String> {
        Ok(self.connect_logged(cfg, conn).await?.0)
    }

    /// Connects like the node loop does, writing the attempt and how it went to the SSH log
    /// (`conn` says which connection: control, bulk, bulk-extra, fresh, doctor). Also says the
    /// path it took (direct, vpn or jump), and keeps an unknown host key distinguishable.
    async fn connect_logged(&self, cfg: &NodeConfig, conn: &str) -> Result<(ssh::Session, &'static str), SshError> {
        let password = (self.password)()?;
        let (target, login, _) = self.plan(cfg, password);
        let socks = self.vpn.socks().await;
        let path = if target.jump.is_some() { "jump" } else if socks.is_some() { "vpn" } else { "direct" };
        let addr = format!("{}:{}", target.addr, target.port);
        self.sshlog.event(&cfg.id, "connect_start", &[("path", &path), ("addr", &addr), ("conn", &conn)]);
        let t = Instant::now();
        match ssh::connect_to(&target, &login, socks, &self.known_hosts, ssh::Trust::Normal).await {
            Ok(mut s) => {
                let auth = match s.login.method {
                    ssh::AuthMethod::Agent => "agent",
                    ssh::AuthMethod::Key => "key",
                    ssh::AuthMethod::Password => "password",
                };
                self.sshlog.event(&cfg.id, "connect_ok", &[("path", &path), ("addr", &addr), ("auth", &auth), ("ms", &t.elapsed().as_millis()), ("conn", &conn)]);
                s.label(&cfg.id, self.sshlog.clone());
                Ok((s, path))
            }
            Err(e) => {
                let kind = NodeFailure::from(e.clone()).kind.unwrap_or("unknown");
                self.sshlog.event(&cfg.id, "connect_fail", &[("path", &path), ("addr", &addr), ("kind", &kind), ("msg", &e), ("conn", &conn)]);
                Err(e)
            }
        }
    }

    /// Logs in to a node with one key file and nothing else (proving key login works), then hangs up.
    pub(crate) async fn connect_with_key(&self, cfg: &NodeConfig, key: &str) -> Result<(), String> {
        let (target, _, _) = self.plan(cfg, None);
        let passphrase = crate::secrets::get(&sshconfig::passphrase_account(key)).ok().flatten();
        let login = ssh::Login { keys: vec![ssh::KeyFile { shown: sshconfig::tilde(key), path: PathBuf::from(key), passphrase }], identities_only: true, ..Default::default() };
        let s = ssh::connect_to(&target, &login, self.vpn.socks().await, &self.known_hosts, ssh::Trust::Normal).await?;
        s.close().await;
        Ok(())
    }

    /// Connects for the Connect wizard's checks (keys first, then `password`). It never saves a
    /// host key: `trust` says which keys it accepts.
    pub(crate) async fn connect_with(&self, cfg: &NodeConfig, password: Option<&str>, trust: ssh::Trust) -> Result<ssh::Session, SshError> {
        let (target, login, _) = self.plan(cfg, password.map(String::from));
        ssh::connect_to(&target, &login, self.vpn.socks().await, &self.known_hosts, trust).await
    }

    /// Discovery uses the normal credential selection but never SSH config's target overrides,
    /// ProxyJump or VPN routing. Host-key verification precedes every credential offer.
    pub(crate) async fn connect_for_discovery(&self, cfg: &NodeConfig, stream: tokio::net::TcpStream) -> Result<ssh::Session, SshError> {
        let (_, login, _) = self.plan(cfg, crate::secrets::get("ssh").map_err(SshError::Failed)?);
        let target = ssh::Target { addr: cfg.ip.clone().unwrap_or_else(|| cfg.host.clone()), port: cfg.port, user: cfg.user.clone(), jump: None };
        ssh::connect_discovery(&target, &login, stream, &self.known_hosts).await
    }

    async fn run_node(self: Arc<Self>, cfg: NodeConfig, generation: u64) {
        let mut delay = 2;
        loop {
            self.update_as(&cfg.id, generation, |s| s.status = LinkStatus::Connecting);
            let started = Instant::now();
            let result = self.clone().run_session(&cfg, generation).await;
            let (status, failure, again_now) = match result {
                Ok(Ended::Closed { .. }) => (LinkStatus::Offline, NodeFailure::from("Connection closed; reconnecting…"), false),
                Ok(Ended::Reconnect { shown, .. }) => (LinkStatus::Connecting, NodeFailure::from(shown), true),
                Err(e) => {
                    let (status, f) = link_status(e);
                    (status, f, false)
                }
            };
            self.update_as(&cfg.id, generation, |s| {
                s.status = status;
                s.error = Some(failure.message);
                s.error_kind = failure.kind;
                if failure.host_key.is_some() {
                    s.host_key = failure.host_key;
                }
                s.session = None;
                s.login = None;
                s.frame = None;
                s.frame_error = None;
                s.latency_ms = None;
                s.last_frame_at = None;
            });
            self.homes.lock().unwrap().remove(&cfg.id);
            if again_now || started.elapsed() > Duration::from_secs(60) {
                delay = 2;
            }
            if again_now {
                continue;
            }
            // Until the next attempt, or sooner when asked (the inventory was saved again).
            let retry = self.nodes.lock().unwrap().get(&cfg.id).map(|s| s.retry.clone());
            match retry {
                Some(r) => {
                    let _ = tokio::time::timeout(Duration::from_secs(delay), r.notified()).await;
                }
                None => tokio::time::sleep(Duration::from_secs(delay)).await,
            }
            delay = (delay * 2).min(30);
        }
    }

    /// One connection's lifetime: connect, install the collector, stream frames. The pinger,
    /// the stall watch and the collector restarts run inside it, so they end with it. Whatever
    /// ended it, commands still running on it are marked interrupted before it's closed.
    async fn run_session(self: Arc<Self>, cfg: &NodeConfig, generation: u64) -> Result<Ended, NodeFailure> {
        // Kicks from here on are this connection's (a login change while it's being made remakes
        // it); those before were answered by the one before.
        let kick = self.nodes.lock().unwrap().get(&cfg.id).filter(|s| s.generation == generation).map(|s| s.kick.subscribe()).ok_or("stopped")?;
        let (session, path) = self.connect_logged(cfg, "control").await?;
        let session = Arc::new(session);
        let ended = self.clone().serve(cfg, generation, &session, path, kick).await;
        let why = match &ended {
            Ok(Ended::Closed { why }) | Ok(Ended::Reconnect { why, .. }) => why.clone(),
            Err(e) => e.message.clone(),
        };
        self.interrupt(&cfg.id, &session, &why);
        session.close().await;
        self.sshlog.event(&cfg.id, "session_closed", &[("conn", &"control"), ("reason", &why)]);
        self.sshlog.event(&cfg.id, "reconnect", &[("reason", &why)]);
        ended
    }

    /// The connected part of `run_session`.
    async fn serve(self: Arc<Self>, cfg: &NodeConfig, generation: u64, session: &Arc<ssh::Session>, via: &'static str, mut kick: watch::Receiver<String>) -> Result<Ended, NodeFailure> {
        let t = Instant::now();
        if let Err(e) = session.exec("true", None, ANSWER_LIMIT).await {
            let kind = if e.contains("took longer than") { Some("timeout") } else { error_kind_of(&e) };
            return Err(NodeFailure { message: e, kind, host_key: None });
        }
        let latency = t.elapsed().as_secs_f32() * 1000.0;
        self.update_as(&cfg.id, generation, |s| {
            s.session = Some(session.clone());
            s.login = Some(session.login.clone());
            s.error = None;
            s.error_kind = None;
            s.host_key = session.host_key.clone().map(|fingerprint| HostKeyInfo { address: format!("{}:{}", address(cfg), cfg.port), fingerprint, shared_with: vec![] });
            s.latency_ms = Some(latency);
        });

        install(session, "collector.py", COLLECTOR, &cfg.name).await?;
        // The collector reads RoCE and PHY counters through roce.py next to it.
        install(session, "roce.py", ROCE, &cfg.name).await?;
        let mut path = self.vpn.path_watch();
        let _ = path.borrow_and_update();

        // Latency from a tiny command every 15 s, on the same connection.
        let pinger = async {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let t = Instant::now();
                match session.exec("true", None, ANSWER_LIMIT).await {
                    Ok(_) => {
                        let ms = t.elapsed().as_secs_f32() * 1000.0;
                        self.update_as(&cfg.id, generation, |s| s.latency_ms = Some(ms));
                    }
                    // Every channel taken: a queue on this Mac, not a connection that stopped answering.
                    Err(e) if ssh::is_busy(&e) => {}
                    Err(e) => return e,
                }
            }
        };

        tokio::select! {
            ended = self.frames(cfg, generation, session) => ended,
            e = pinger => {
                self.sshlog.event(&cfg.id, "exec_true_failed", &[("by", &"pinger"), ("msg", &e)]);
                Ok(Ended::Closed { why: "no reply to the 15-second check".into() })
            }
            Ok(()) = kick.changed() => {
                let why = kick.borrow_and_update().clone();
                Ok(Ended::Reconnect { shown: "Reconnecting…".into(), why })
            }
            _ = path.changed() => {
                let to = if self.vpn.view().await.status.state == crate::vpn::VpnState::Connected { "vpn" } else { "direct" };
                self.sshlog.event(&cfg.id, "path_change", &[("from", &via), ("to", &to)]);
                // The old path may be gone: a bulk connection nothing is using is made again on next use.
                if !self.bulk_busy(&cfg.id) {
                    if let Some(b) = self.nodes.lock().unwrap().get_mut(&cfg.id).and_then(|s| s.bulk.take()) {
                        self.sshlog.event(&cfg.id, "session_closed", &[("conn", &"bulk"), ("reason", &"the network path changed")]);
                        tokio::spawn(async move { b.close().await });
                    }
                }
                Ok(Ended::Reconnect { shown: "The network path changed; reconnecting…".into(), why: "the network path changed".into() })
            }
        }
    }

    /// Streams the collector's frames. A collector quiet for DROP_AFTER is a stalled collector,
    /// not a dead connection, while `true` still answers on the connection: only its channel is
    /// closed and a fresh collector starts there. One that ended (killed, crashed) starts again
    /// the same way. The connection counts as dead when it closed, when `true` doesn't answer
    /// within 10 s, or after more than MAX_RESTARTS stalls in RESTART_WINDOW. A collector that
    /// ends that often, or a node without python3, is shown as an Error and tried again after
    /// COLLECTOR_RETRY, its connection kept for commands.
    async fn frames(&self, cfg: &NodeConfig, generation: u64, session: &ssh::Session) -> Result<Ended, NodeFailure> {
        let collector = wire(&format!("exec python3 -u {COLLECTOR_PATH}"));
        let mut restarts: VecDeque<Instant> = VecDeque::new();
        loop {
            let opened = Instant::now();
            let (id, name) = (cfg.id.clone(), cfg.name.clone());
            let stream = session.stream(&collector, |line| match serde_json::from_str::<Frame>(line) {
                Ok(frame) => self.update_as(&id, generation, |s| {
                    s.frame = Some(frame);
                    s.status = LinkStatus::Online;
                    s.error = None;
                    s.frame_error = None;
                    s.last_frame_at = Some(Instant::now());
                    s.last_seen_ms = Some(now_ms());
                }),
                Err(e) => self.update_as(&id, generation, |s| s.frame_error = Some(format!("{name} sent a collector frame SparkBox couldn't read ({e})."))),
            });
            // How long since this collector's last frame (or its start), once that's DROP_AFTER.
            let watch = async {
                loop {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let last = self.nodes.lock().unwrap().get(&cfg.id).and_then(|s| s.last_frame_at).filter(|t| *t > opened).unwrap_or(opened);
                    if last.elapsed() > DROP_AFTER {
                        return last.elapsed();
                    }
                }
            };
            let gone = tokio::select! {
                result = stream => match result {
                    // Its channel ended with the connection: the connection closed.
                    Ok(_) if session.ended().is_some() || session.is_closed() => return Ok(Ended::Closed { why: self.why_closed(&cfg.id, session) }),
                    Ok(end) => {
                        let code = end.code.map_or_else(|| "none".to_string(), |c| c.to_string());
                        self.sshlog.event(&cfg.id, "collector_exit", &[("code", &code), ("signal", &end.signal.as_deref().unwrap_or("none"))]);
                        Gone::Exit(end)
                    }
                    // No channel came free for it: tried again on this connection, like a stall.
                    Err(e) if ssh::is_busy(&e) => Gone::Busy,
                    Err(e) => return Err(e.into()),
                },
                quiet = watch => Gone::Stall(quiet),
            };
            // The stream is gone, and with it only the collector's channel.
            match &gone {
                Gone::Stall(q) => self.sshlog.event(&cfg.id, "frame_stall", &[("secs", &q.as_secs())]),
                Gone::Exit(_) | Gone::Busy => tokio::time::sleep(Duration::from_secs(1)).await,
            }
            match session.answers(ANSWER_LIMIT).await {
                Ok(()) => {}
                // The node refused even that channel: it answered, so the connection is alive.
                Err(e) if ssh::is_busy(&e) => {}
                Err(_) if session.ended().is_some() || session.is_closed() => return Ok(Ended::Closed { why: self.why_closed(&cfg.id, session) }),
                Err(e) => {
                    self.sshlog.event(&cfg.id, "exec_true_failed", &[("by", &"stall"), ("msg", &e)]);
                    let secs = ANSWER_LIMIT.as_secs();
                    return Ok(Ended::Reconnect { shown: format!("No reply from {} in {secs} s; reconnecting…", cfg.name), why: format!("no reply from {} in {secs} s", cfg.name) });
                }
            }
            if let Gone::Exit(ssh::StreamEnd { code: Some(127), .. }) = &gone {
                // No python3 on the node: nothing to restart until it's installed.
                self.hold_collector(cfg, generation, format!("python3 isn't installed on {}.", cfg.name)).await;
                continue;
            }
            let now = Instant::now();
            restarts.retain(|t| now.duration_since(*t) < RESTART_WINDOW);
            if restarts.len() >= MAX_RESTARTS {
                let n = MAX_RESTARTS + 1;
                match &gone {
                    // A collector that keeps ending is the collector's problem, not the connection's.
                    Gone::Exit(end) => {
                        let why = format!("The collector on {} stopped {n} times in 5 minutes ({}); SparkBox tries again in a minute.", cfg.name, exit_reason(end));
                        self.hold_collector(cfg, generation, why).await;
                    }
                    Gone::Stall(_) | Gone::Busy => {
                        return Ok(Ended::Reconnect { shown: format!("The collector on {} stalled {n} times in 5 minutes; reconnecting…", cfg.name), why: format!("the collector stalled {n} times in 5 minutes") })
                    }
                }
            }
            restarts.push_back(Instant::now());
            self.sshlog.event(&cfg.id, "collector_restart", &[("count", &restarts.len())]);
        }
    }

    /// Shows why a node's collector isn't running while its connection is up (an Error, the old
    /// numbers going stale), and waits COLLECTOR_RETRY before it's started again.
    async fn hold_collector(&self, cfg: &NodeConfig, generation: u64, why: String) {
        self.sshlog.event(&cfg.id, "collector_held", &[("secs", &COLLECTOR_RETRY.as_secs()), ("reason", &why)]);
        self.update_as(&cfg.id, generation, |s| {
            s.status = LinkStatus::Error;
            s.error = Some(why);
            s.error_kind = None;
        });
        tokio::time::sleep(COLLECTOR_RETRY).await;
    }

    /// Why a connection closed by itself, as far as SSH can tell: keepalive replies stopped
    /// (logged as keepalive_lost) or the node hung up.
    fn why_closed(&self, node_id: &str, session: &ssh::Session) -> String {
        match session.ended() {
            Some(k) if k == "keepalive" => {
                self.sshlog.event(node_id, "keepalive_lost", &[]);
                "keepalive lost".into()
            }
            Some(why) => why,
            None => "the connection ended".into(),
        }
    }

    /// Reconnects everything when the Mac wakes from sleep (its wall clock jumps ahead of the
    /// monotonic one) and asks the VPN to look at the network again. Also closes extra bulk
    /// connections nothing used for a minute, and logs each node's channel use every 10 minutes.
    async fn watch_sleep_and_path(self: Arc<Self>) {
        let mut last = (Instant::now(), SystemTime::now());
        let mut stats_at = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = (Instant::now(), SystemTime::now());
            let mono = now.0.duration_since(last.0);
            let wall = now.1.duration_since(last.1).unwrap_or_default();
            if wall > mono + SLEEP_JUMP {
                self.sshlog.event("mac", "sleep_wake", &[("slept_s", &(wall - mono).as_secs())]);
                self.vpn.nudge();
                self.reconnect_every("the Mac woke from sleep");
            }
            last = now;
            self.close_idle_bulk();
            if stats_at.elapsed() >= STATS_EVERY {
                stats_at = Instant::now();
                self.log_channel_stats();
            }
        }
    }

    /// One channel_stats line per node in the SSH log.
    fn log_channel_stats(&self) {
        for (id, s) in self.all_channel_stats() {
            self.sshlog.event(&id, "channel_stats", &[("in_use", &s.in_use), ("queued", &s.queued), ("refusals", &s.refusals), ("bulk_connections", &s.bulk_connections)]);
        }
    }

    pub async fn container_action(&self, node_id: &str, container_id: &str, action: &str) -> Result<(), String> {
        let verb = match action {
            "start" | "stop" | "restart" | "kill" | "pause" | "unpause" => action,
            "remove" => "rm",
            _ => return Err(format!("unsupported action {action}")),
        };
        if container_id.len() < 12 || !container_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("bad container id".into());
        }
        self.exit.admit(self, node_id, exit::ExecKind::ContainerAction, &format!("docker {verb} {container_id}")).await.map_err(|e| e.to_json())?;
        let out = self.run(node_id, &format!("docker {verb} {container_id}"), None, Duration::from_secs(90)).await?;
        if out.code == Some(0) {
            Ok(())
        } else {
            Err(format!("docker {verb} failed on {node_id}: {}", out.stderr.trim()))
        }
    }

    /// `docker inspect` for one container.
    pub async fn container_inspect(&self, node_id: &str, container_id: &str) -> Result<serde_json::Value, String> {
        if container_id.len() < 12 || !container_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("bad container id".into());
        }
        let out = self.run(node_id, &format!("docker inspect --type container {container_id}"), None, Duration::from_secs(30)).await?;
        if out.code != Some(0) {
            return Err(format!("docker inspect failed on {node_id}: {}", out.stderr.trim()));
        }
        let mut list: Vec<serde_json::Value> = serde_json::from_str(&out.stdout).map_err(|e| format!("unreadable docker inspect output: {e}"))?;
        list.pop().ok_or_else(|| "docker inspect returned nothing".into())
    }

    /// A plain-text report of everything the collector relies on, per node. Reads only: the
    /// sudo-only line says it needs sudo (`doctor_with`).
    #[cfg(test)]
    pub async fn doctor(&self, nodes: &[NodeConfig], header: &str) -> String {
        self.doctor_with(nodes, header, false).await
    }

    /// `doctor`, and with `sudo` (the owner's session: Admin unlocked, not look-only) sshd's own
    /// settings too, read with sudo as a sudo check (`sudo-probe`): the fence must allow it on
    /// the node, or the report says why it didn't run.
    pub async fn doctor_with(&self, nodes: &[NodeConfig], header: &str, sudo: bool) -> String {
        self.doctor_running(nodes, header, sudo, &doctor_script(), DOCTOR_LIMIT).await
    }

    /// `doctor_with`, running `script` on each node within `limit` (a test sends another one).
    async fn doctor_running(&self, nodes: &[NodeConfig], header: &str, sudo: bool, script: &str, limit: Duration) -> String {
        let mut report = format!("SparkBox doctor report\n{header}\n");
        for cfg in nodes {
            report.push_str(&format!("\n\n=================== {} ({}) ===================\n", cfg.name, address(cfg)));
            report.push_str(&self.doctor_node(cfg, sudo, script, limit).await);
        }
        report
    }

    /// One node's part of the report. The script leaves through the write exit like any command
    /// (it only reads, so look-only lets it through) and is in the activity log as one read; a
    /// node that isn't connected gets a connection of its own for it. A run that stopped
    /// part-way keeps what it printed and says why the rest is missing (`doctor_cut_short`).
    async fn doctor_node(&self, cfg: &NodeConfig, sudo: bool, script: &str, limit: Duration) -> String {
        let admitted = match self.exit.admit(self, &cfg.id, exit::ExecKind::Run, script).await {
            Ok(a) => a,
            Err(e) => return format!("Diagnostics refused: {}\n", e.message),
        };
        let (session, own) = match self.session(&cfg.id) {
            Some(s) => (s, false),
            None => match self.connect(cfg, "doctor").await {
                Ok(s) => (Arc::new(s), true),
                Err(e) => return format!("Couldn't connect: {e}\n"),
            },
        };
        let mut part = String::new();
        let mut stderr = String::new();
        match self.run_on(&cfg.id, &session, script, &admitted, "Run doctor", limit).await {
            Ok(out) => {
                part.push_str(&out.stdout);
                if let Some(why) = doctor_cut_short(&out, limit) {
                    part.push_str(&format!("\n{why}\n"));
                }
                stderr = out.stderr;
            }
            Err(e) => part.push_str(&format!("Diagnostics failed: {e}\n")),
        }
        if own {
            session.close().await;
            self.sshlog.event(&cfg.id, "session_closed", &[("conn", &"doctor"), ("reason", &"the doctor is done")]);
        }
        part.push_str(&self.doctor_sshd(cfg, sudo).await);
        part.push_str(&self.doctor_collector(cfg));
        if !stderr.trim().is_empty() {
            part.push_str(&format!("\n----- stderr\n{stderr}"));
        }
        part
    }

    /// sshd's settings as it decides them (`sshd -T`, root only): run in an owner's session as a
    /// sudo check, so the fence decides; otherwise the line says it needs sudo.
    async fn doctor_sshd(&self, cfg: &NodeConfig, sudo: bool) -> String {
        let head = "\n----- sshd-effective (sudo sshd -T)\n";
        if !sudo {
            return format!("{head}needs sudo: run the doctor with Admin unlocked to read it on {}\n", cfg.name);
        }
        match exit::as_class(fence_class::FenceClass::SudoProbe, self.sudo(&cfg.id, DOCTOR_SSHD_T, Duration::from_secs(30))).await {
            Ok(out) => format!("{head}{}{}", out.stdout, out.stderr),
            Err(e) => format!("{head}needs sudo: {}\n", crate::errors::SbError::classify(&e).message),
        }
    }

    /// The collector's side, from what SparkBox received rather than a second collector run on
    /// the node: its latest frame as SparkBox reads it (secrets masked), or why there's none.
    fn doctor_collector(&self, cfg: &NodeConfig) -> String {
        let head = "\n----- collector (the last frame SparkBox received)\n";
        let nodes = self.nodes.lock().unwrap();
        let st = nodes.get(&cfg.id);
        let mut snap = node_snapshot(cfg, st);
        if st.is_none_or(|s| s.frame.is_none()) {
            return format!("{head}No frame: {:?}{}\n", snap.status, snap.error.map(|e| format!(" ({e})")).unwrap_or_default());
        }
        let summary = format!(
            "{} · {:?} · frame {} ms old · {} processes, {} containers, {} listening ports{}\n",
            snap.system.hostname,
            snap.status,
            snap.frame_age_ms.unwrap_or_default(),
            snap.processes.len(),
            snap.containers.len(),
            snap.listeners.len(),
            snap.error.as_ref().map(|e| format!(" · {e}")).unwrap_or_default()
        );
        // The rest of the frame, without the process list (it's the bulk of it).
        snap.processes.clear();
        let text = serde_json::to_string(&snap).unwrap_or_default();
        let cut = (0..=text.len().min(6000)).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
        format!("{head}{summary}{}\n", &text[..cut])
    }

    /// Runs `cmd` on `session` (the node's control connection, or one the doctor or the Connect
    /// wizard made for it) once the exit has `admitted` it, logged like `run`. A run the time
    /// limit stopped comes back with what it printed and `timed_out` set.
    async fn run_on(&self, node_id: &str, session: &Arc<ssh::Session>, cmd: &str, admitted: &exit::Admitted, title: &str, limit: Duration) -> Result<ssh::Output, String> {
        let started = Instant::now();
        let row = self.row(audit::Entry {
            node: node_id.into(),
            kind: "command".into(),
            command: cmd.into(),
            title: title.into(),
            changes: !audit::read_only(cmd),
            fence: admitted.fence.clone(),
            ..Default::default()
        });
        self.track(node_id, session, &row);
        let result = exec_marked(session, &wire(cmd), None, limit, &CancelToken::new(), &row).await;
        self.log_end(row, started, &result);
        result
    }

    /// The Connect wizard's reads on its own connection to a node (one that may not be connected
    /// yet): through the write exit like `run`, and in the activity log as SparkBox's checks.
    pub(crate) async fn check_on(&self, node_id: &str, session: &Arc<ssh::Session>, cmd: &str, limit: Duration) -> Result<ssh::Output, String> {
        let admitted = self.exit.admit(self, node_id, exit::ExecKind::Run, cmd).await.map_err(|e| e.to_json())?;
        let out = audit::as_probe(self.run_on(node_id, session, cmd, &admitted, "", limit)).await?;
        if out.timed_out {
            return Err(ssh::took_longer(cmd, limit));
        }
        Ok(out)
    }

    /// The Connect wizard's sudo check on its own connection: `cmd` with sudo and `password`,
    /// through the write exit as `sudo` does (with its breaker), and always a sudo line in the
    /// activity log, `sudo true` included (the log otherwise leaves `true` out: the latency ping).
    pub(crate) async fn sudo_on(&self, node_id: &str, session: &Arc<ssh::Session>, cmd: &str, password: &str, limit: Duration) -> Result<ssh::Output, String> {
        let admitted = self.exit.admit(self, node_id, exit::ExecKind::Sudo, cmd).await.map_err(|e| e.to_json())?;
        let hash = sha256_hex(password.as_bytes());
        if let Err(e) = self.breaker_check(node_id, &hash) {
            self.audit.record_failed(node_id, "", &format!("sudo {cmd}"), &e);
            return Err(e);
        }
        let row = self.row(audit::Entry {
            node: node_id.into(),
            kind: "sudo".into(),
            command: cmd.into(),
            title: "Connect check: sudo".into(),
            fence: admitted.fence.clone(),
            ..Default::default()
        });
        self.sudo_logged(node_id, session, cmd, password, &hash, None, limit, row).await
    }

    /// The ports `node_id` listens on, from its latest frame (None: no frame from it yet).
    pub fn listeners(&self, node_id: &str) -> Option<Vec<Listener>> {
        self.nodes.lock().unwrap().get(node_id).and_then(|s| s.frame.as_ref().map(|f| f.listeners.clone()))
    }
}
