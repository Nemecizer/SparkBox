//! Actions that change a node: signal or re-prioritise a process, manage systemd services,
//! reboot or shut down, run a command on several nodes (fleet.rs), act on containers
//! (containers.rs) and install updates (updates.rs). Anything that needs root goes through
//! `Live::sudo` (password from the Keychain, on stdin).
//!
//! Every action runs as a task (tasks.rs), and its commands are logged under the task's id
//! so the activity log groups them. In Demo mode the simulator (`DemoCluster`) plays the
//! part of the node, and the log records the command that would have run, marked simulated.

pub mod classify;
pub mod commands;
pub mod containers;
pub mod fleet;
pub mod gpu_guard;
pub mod images;
pub mod prereboot;
pub mod postboot;
pub mod rolling;
pub mod services;
#[cfg(test)]
mod tests;
pub mod updates;
pub mod protected_updates;
pub mod rdp;
pub mod update_check;
pub mod image_registry;
#[cfg(test)]
mod node_ops_tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::demo::DemoCluster;
use crate::models::verify;
use crate::errors::{ErrorKind, SbError};
use crate::live::audit::{self, OpMeta};
use crate::live::exit::as_class;
use crate::live::fence_class::FenceClass;
use crate::live::{sh_quote as q, Live};
use crate::model::{ClusterSnapshot, LinkStatus, Listener, NodeSnapshot};
use crate::tasks::{TaskHandle, TaskSpec, Tasks};

pub use fleet::FleetResult;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub unit: String,
    pub description: String,
    /// loaded | not-found | masked…
    pub load: String,
    /// active | inactive | failed | activating…
    pub active: String,
    /// running | exited | dead…
    pub sub: String,
    /// enabled | disabled | static | masked… (empty when unknown)
    pub enabled: String,
}

pub(crate) fn unit_ok(unit: &str) -> bool {
    !unit.is_empty() && unit.len() <= 200 && !unit.starts_with('-') && unit.chars().all(|c| c.is_ascii_alphanumeric() || "_.-@:\\".contains(c))
}

/// Parses `systemctl list-units --output=json` and `list-unit-files --output=json`.
pub fn parse_services(units: &str, files: &str) -> Result<Vec<Service>, String> {
    let units: Vec<Value> = serde_json::from_str(units).map_err(|e| format!("unexpected systemctl output: {e}"))?;
    let files: Vec<Value> = serde_json::from_str(files).unwrap_or_default();
    let enabled: HashMap<&str, &str> = files.iter().filter_map(|f| Some((f["unit_file"].as_str()?, f["state"].as_str()?))).collect();
    let mut out: Vec<Service> = units
        .iter()
        .filter_map(|u| {
            let unit = u["unit"].as_str()?.to_string();
            Some(Service {
                enabled: enabled.get(unit.as_str()).copied().unwrap_or("").into(),
                description: u["description"].as_str().unwrap_or_default().into(),
                load: u["load"].as_str().unwrap_or_default().into(),
                active: u["active"].as_str().unwrap_or_default().into(),
                sub: u["sub"].as_str().unwrap_or_default().into(),
                unit,
            })
        })
        .collect();
    // Installed but never loaded (disabled and not running) units appear only in the files list.
    for f in &files {
        let Some(unit) = f["unit_file"].as_str() else { continue };
        if unit.ends_with(".service") && !unit.contains('@') && !out.iter().any(|s| s.unit == unit) {
            out.push(Service {
                unit: unit.into(),
                description: String::new(),
                load: "not-loaded".into(),
                active: "inactive".into(),
                sub: "dead".into(),
                enabled: f["state"].as_str().unwrap_or_default().into(),
            });
        }
    }
    out.sort_by(|a, b| a.unit.cmp(&b.unit));
    Ok(out)
}

/// The command that sends `signal` to `pid` only if it's still the same process: its start
/// time (/proc/<pid>/stat field 22, clock ticks after boot) must match, so a PID the kernel
/// has since given to another program is left alone. Exits 3 with "gone" otherwise.
/// Field 2 (the name, in parentheses) can contain spaces and ")", so fields are counted after
/// the last ") " (sed's match is greedy, and no later field has one): field 22 is the 20th there.
pub fn pid_guard_cmd(pid: u32, signal: &str, start_time: Option<u64>, proc_root: &str) -> String {
    match start_time {
        Some(start) => format!(
            "s=$(sed 's/^.*) //' {proc_root}/{pid}/stat 2>/dev/null | cut -d' ' -f20); [ \"$s\" = \"{start}\" ] || {{ echo gone >&2; exit 3; }}; kill -s {signal} {pid}"
        ),
        None => format!("kill -s {signal} {pid}"),
    }
}

type SnapshotFn = Box<dyn Fn() -> Option<ClusterSnapshot> + Send + Sync>;
type EmitFn = Box<dyn Fn(&str, Value) + Send + Sync>;
type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// How long a Verify step waits for a model to answer again (a 70B model loads in minutes).
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What an operation's Verify step asks: which endpoints it touched, and whether they answer
/// again. The app uses the model watcher's rounds (`ModelsVerifier`); tests script it.
pub trait Verifier: Send + Sync {
    fn affected(&self, scope: &verify::Scope) -> Vec<verify::EndpointRef>;
    /// Waits for each to answer (or stay down); `progress` gets "Loading 42%"-style words.
    fn wait<'a>(&'a self, refs: &'a [verify::EndpointRef], timeout: Duration, progress: Box<dyn Fn(&verify::EndpointRef, String) + Send + Sync + 'a>) -> BoxFuture<'a, Vec<(verify::EndpointRef, verify::VerifyOutcome)>>;
}

/// What power and container operations tell the alerting: planned work starts and ends (no
/// page for a restart SparkBox started), and a crash-looping container is verified working
/// again. The app wires it to `health::maintenance` and the agents (lib.rs); tests record it.
pub trait Planned: Send + Sync {
    /// About to restart `node` (`until_ms`: when it should be back) or shut it down (None).
    fn begin(&self, node: &str, until_ms: Option<u64>, reason: &str);
    /// Tells every node's agent about the planned work before anything is sent, and waits for
    /// them (5 s at most): the agents that didn't get the note, each with why. Its commands
    /// nest under the operation `op_id`.
    fn notify_agents<'a>(&'a self, op_id: &'a str, nodes: &'a [String], until_ms: Option<u64>, reason: &'a str) -> BoxFuture<'a, Vec<(String, String)>> {
        let _ = (op_id, nodes, until_ms, reason);
        Box::pin(async { Vec::new() })
    }
    /// The work runs on: the window on `node` now lasts until `until_ms` (an upgrade whose unit
    /// is still active, decision d). The agents aren't told again.
    fn extend(&self, node: &str, until_ms: u64) {
        let _ = (node, until_ms);
    }
    /// The operation is over: Verify ran, or it failed or was called off.
    fn end(&self, node: &str);
    /// `alert` (a crash-loop on a container just restarted) is resolved, with why.
    fn resolved(&self, rule: &str, alert: &str, reason: &str);
}

/// Planned work as the app records it: a maintenance window in the alerting
/// (`health::maintenance`), the same note to every reachable node's agent, and resolutions
/// through `Health::resolve_now`.
pub struct AppPlanned(pub tauri::AppHandle);

impl Planned for AppPlanned {
    fn begin(&self, node: &str, until_ms: Option<u64>, reason: &str) {
        crate::health::maintenance::begin(&self.0, node, until_ms, reason);
    }

    fn notify_agents<'a>(&'a self, op_id: &'a str, nodes: &'a [String], until_ms: Option<u64>, reason: &'a str) -> BoxFuture<'a, Vec<(String, String)>> {
        Box::pin(async move {
            // agent::maintenance_push keeps to 5 s itself; this is the backstop.
            match tokio::time::timeout(Duration::from_secs(6), crate::agent::maintenance_push(&self.0, op_id, nodes, until_ms, reason)).await {
                Ok(report) => report.missing,
                Err(_) => vec![("the agents".to_string(), "no answer within 5 s".to_string())],
            }
        })
    }

    fn extend(&self, node: &str, until_ms: u64) {
        crate::health::maintenance::extend(&self.0, node, until_ms);
    }

    fn end(&self, node: &str) {
        crate::health::maintenance::end(&self.0, node);
        let (app, node) = (self.0.clone(), node.to_string());
        tauri::async_runtime::spawn(async move {
            crate::agent::maintenance_clear(&app, &node).await;
        });
    }

    fn resolved(&self, rule: &str, alert: &str, reason: &str) {
        use tauri::Manager;
        if let Some(h) = self.0.try_state::<Arc<crate::health::Health>>() {
            h.resolve_now(rule, alert, reason);
        }
    }
}

/// "cell2's agent didn't get the maintenance note: it may page." (one line per agent missed).
pub fn maintenance_missing(missing: &[(String, String)], name: &dyn Fn(&str) -> String) -> String {
    missing
        .iter()
        .map(|(n, why)| {
            let who = if n == "the agents" { "The agents".to_string() } else { format!("{}'s agent", name(n)) };
            format!("{who} didn't get the maintenance note: it may page ({why}).")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How long a restart SparkBox started may take before its node's alerts count again.
const RESTART_EXPECT_MS: u64 = crate::health::maintenance::REBOOT_EXPECT_MS;

/// Verify against the endpoint rounds the model watcher runs (Demo and Live).
pub struct ModelsVerifier(pub Arc<crate::models::Models>);

impl Verifier for ModelsVerifier {
    fn affected(&self, scope: &verify::Scope) -> Vec<verify::EndpointRef> {
        verify::affected_in(&self.0.endpoints(), scope)
    }

    fn wait<'a>(&'a self, refs: &'a [verify::EndpointRef], timeout: Duration, progress: Box<dyn Fn(&verify::EndpointRef, String) + Send + Sync + 'a>) -> BoxFuture<'a, Vec<(verify::EndpointRef, verify::VerifyOutcome)>> {
        Box::pin(verify::wait_on(&self.0, refs, timeout, Duration::from_secs(2), move |r, e| {
            let word = match (e.loading_pct, e.health_reason.as_deref()) {
                (Some(p), _) => format!("{}: loading {p}%", r.model),
                (None, Some(why)) => format!("{}: {why}", r.model),
                (None, None) => format!("{}: waiting", r.model),
            };
            progress(r, word)
        }))
    }
}

pub struct Ops {
    live: Arc<Live>,
    demo: Arc<Mutex<DemoCluster>>,
    tasks: Arc<Tasks>,
    /// The latest cluster snapshot (power waits on it, impacts read it).
    snapshot: RwLock<Option<SnapshotFn>>,
    /// Sends an event to the UI (Fleet's per-node output and states).
    emit: RwLock<Option<EmitFn>>,
    /// Finished Fleet runs' results, by run id (the last 20).
    fleet_runs: Mutex<Vec<(String, Vec<FleetResult>)>>,
    /// Demo holds and installs made through Updates, layered over the simulator's lists.
    demo_updates: Mutex<HashMap<String, updates::DemoOverlay>>,
    rdp_demo: Mutex<HashSet<String>>,
    /// How often the power lifecycle looks at the snapshot.
    poll: Duration,
    /// A minute of a scheduled restart or shutdown (tests shorten it; the node's `shutdown +N` doesn't care).
    minute: Duration,
    /// Checks that models answer again after an operation (the Verify step).
    verifier: RwLock<Option<Arc<dyn Verifier>>>,
    /// Planned work and recoveries, for the alerting.
    planned: RwLock<Option<Arc<dyn Planned>>>,
    /// Power states SparkBox declared, by node (P1.14): shown in every snapshot from the moment
    /// the restart or shutdown is sent (`power_overlay`), not only once the node has gone.
    declared: Mutex<HashMap<String, Declared>>,
    /// The upgrade running (or last run) on each node (updates.rs): only this SparkBox's, and
    /// the only ones it saves to its upgrades.json. Lock order: `declared`, `runs`, `following`.
    runs: Mutex<BTreeMap<String, updates::UpgradeRun>>,
    /// Nodes whose upgrade a task of this SparkBox follows, or waits to follow again (updates.rs):
    /// one follower per node.
    following: Mutex<HashSet<String>>,
    /// How long an upgrade's follower waits for its node before the run counts as lost.
    upgrade_wait: Mutex<Duration>,
    /// An upgrade's maintenance window, from the start and from each sighting of its unit active.
    upgrade_window: Mutex<Duration>,
    postboot: Mutex<HashMap<String, postboot::Expected>>,
    rolling_busy: std::sync::atomic::AtomicBool,
}

/// A power state SparkBox declared for a node: rebooting until it's seen on a new boot,
/// shutting-down then off until it's woken (or seen answering again), waking while Wake runs.
/// restart-scheduled and shutdown-scheduled hold the node while a delayed one waits for its time;
/// restart-checking while a restart's checks run (prereboot.rs), before anything is sent.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Declared {
    state: &'static str,
    demo: bool,
}

const RESTART_SCHEDULED: &str = "restart-scheduled";
const SHUTDOWN_SCHEDULED: &str = "shutdown-scheduled";
const RESTART_CHECKING: &str = "restart-checking";

impl Declared {
    /// In the snapshots. A scheduled one isn't (the node is up until its time), nor one whose
    /// checks are still running (it may not restart at all); they only keep a second power
    /// command out.
    fn shown(&self) -> bool {
        self.state != RESTART_SCHEDULED && self.state != SHUTDOWN_SCHEDULED && self.state != RESTART_CHECKING
    }
}

impl Ops {
    pub fn new(live: Arc<Live>, demo: Arc<Mutex<DemoCluster>>, tasks: Arc<Tasks>) -> Arc<Ops> {
        // Every task's head line in the activity log ends with the task's own outcome.
        let log = live.clone();
        tasks.on_finish(move |t| {
            if let (Some(op), Some(outcome)) = (&t.op_id, t.outcome) {
                let error = if outcome == audit::Outcome::Warning { t.warning.clone() } else { t.error.clone() };
                log.audit.end_heads(op, outcome, t.summary.clone(), error);
            }
        });
        Arc::new(Ops {
            live,
            demo,
            tasks,
            snapshot: RwLock::new(None),
            emit: RwLock::new(None),
            fleet_runs: Mutex::new(Vec::new()),
            demo_updates: Mutex::new(HashMap::new()),
            rdp_demo: Mutex::new(HashSet::new()),
            poll: Duration::from_secs(1),
            minute: Duration::from_secs(60),
            verifier: RwLock::new(None),
            planned: RwLock::new(None),
            declared: Mutex::new(HashMap::new()),
            runs: Mutex::new(BTreeMap::new()),
            following: Mutex::new(HashSet::new()),
            upgrade_wait: Mutex::new(updates::UPGRADE_WAIT),
            upgrade_window: Mutex::new(updates::UPGRADE_WINDOW),
            postboot: Mutex::new(HashMap::new()),
            rolling_busy: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Writes each declared power state into its node's `power_state` where the frame shows none,
    /// so maintenance and boots treat a restart SparkBox sent as planned from the first snapshot
    /// (lib.rs publish calls it before health::on_publish). A node declared off that answers again
    /// (someone pressed its power button) is on: its "off" goes.
    pub fn power_overlay(&self, snap: &mut ClusterSnapshot) {
        let mut declared = self.declared.lock().unwrap();
        for n in snap.nodes.iter_mut() {
            let Some(d) = declared.get(&n.id).copied().filter(|d| d.demo == snap.demo && d.shown()) else { continue };
            if d.state == "off" && n.status == LinkStatus::Online && !n.stale {
                declared.remove(&n.id);
                continue;
            }
            if n.power_state.is_none() {
                n.power_state = Some(d.state.to_string());
            }
        }
    }

    fn declare(&self, demo: bool, node: &str, state: &'static str) {
        self.declared.lock().unwrap().insert(node.to_string(), Declared { state, demo });
    }

    fn undeclare(&self, node: &str) {
        self.declared.lock().unwrap().remove(node);
    }

    /// Declares `state` for `node` unless SparkBox has a power state there already (other than
    /// off): the check and the claim are one step, so two power commands can't both pass while the
    /// first is still telling the agents. Err with the state it has.
    fn claim(&self, demo: bool, node: &str, state: &'static str) -> Result<(), &'static str> {
        let mut declared = self.declared.lock().unwrap();
        if let Some(d) = declared.get(node).filter(|d| d.state != "off") {
            return Err(d.state);
        }
        declared.insert(node.to_string(), Declared { state, demo });
        Ok(())
    }

    /// Why `node` can't be restarted or shut down now, in the words the windows show: SparkBox
    /// holds a power state there (the node window's or a Fleet run's), or an upgrade it started
    /// runs there (`installing`; `verb` is "restart it" or "shut it down"). None when nothing does.
    /// Only a claim decides for sure; this is the look before one.
    fn power_busy(&self, node: &str, verb: &str) -> Option<String> {
        let name = self.name_of(node);
        let held = self.declared.lock().unwrap().get(node).filter(|d| d.state != "off").map(|d| d.state);
        held.map(|state| held_words(&name, state)).or_else(|| self.installing(node, &name, verb))
    }

    /// "cell2 is installing packages (Upgrade libbrotli1 on cell2); restart it when that
    /// finishes." while an upgrade SparkBox started runs on `node` (updates.rs): a restart or a
    /// shutdown would stop apt or dpkg part-way, and the upgrade's window would close the
    /// restart's. Its task counts from its dry run on; a picked-up one by its run's record (set as
    /// its task starts). A run SparkBox lost the node during counts too, until it's followed to its
    /// end.
    fn installing(&self, node: &str, name: &str, verb: &str) -> Option<String> {
        let running = |id: &str| self.tasks.get(id).is_some_and(|t| !t.finished());
        let task = self.tasks.list().into_iter().find(|t| !t.finished() && t.kind == "updates" && t.nodes == [node] && t.retry.as_ref().is_some_and(|r| r.command == "updates_upgrade"));
        let title = task.map(|t| t.title).or_else(|| {
            self.runs().into_iter().find(|r| r.node == node && !matches!(r.phase.as_str(), "done" | "failed" | "cancelled" | "lost") && running(&r.task_id)).map(|r| r.title)
        });
        if let Some(title) = title {
            return Some(format!("{name} is installing packages ({title}); {verb} when that finishes."));
        }
        let lost = self.runs().into_iter().find(|r| r.node == node && r.phase == "lost" && !r.simulated)?;
        Some(format!("{name} may still be installing packages ({}): SparkBox lost it while it was upgrading, and follows it again once it's back; {verb} when that finishes.", lost.title))
    }

    pub fn set_verifier(&self, v: Arc<dyn Verifier>) {
        *self.verifier.write().unwrap() = Some(v);
    }

    pub fn set_planned(&self, p: Arc<dyn Planned>) {
        *self.planned.write().unwrap() = Some(p);
    }

    fn planned(&self) -> Option<Arc<dyn Planned>> {
        self.planned.read().unwrap().clone()
    }

    /// The Verify step: waits for the endpoints `scope` touched to answer again. Ok with
    /// "llama-3.3-70b answered after 2m 10s" when they all do (None when it touched none);
    /// Err with "<done>, but llama-3.3-70b didn't come back: <reason>" otherwise.
    pub(crate) async fn verify(&self, task: &TaskHandle, scope: verify::Scope, done: &str) -> Result<Option<String>, String> {
        let Some(v) = self.verifier.read().unwrap().clone() else { return Ok(None) };
        let refs = v.affected(&scope);
        if refs.is_empty() {
            return Ok(None);
        }
        let names: Vec<&str> = refs.iter().map(|r| r.model.as_str()).collect();
        task.step(format!("Waiting for {} to answer…", names.join(", ")));
        let t = task.clone();
        let outcomes = v.wait(&refs, VERIFY_TIMEOUT, Box::new(move |_, word| t.detail(word))).await;
        let down: Vec<String> = outcomes
            .iter()
            .filter_map(|(r, o)| match o {
                verify::VerifyOutcome::Down { reason } => Some(format!("{} didn't come back: {reason}", r.model)),
                verify::VerifyOutcome::Ready { .. } => None,
            })
            .collect();
        if down.is_empty() {
            let slowest = outcomes.iter().filter_map(|(_, o)| match o {
                verify::VerifyOutcome::Ready { after_s, .. } => Some(*after_s),
                _ => None,
            });
            task.step_ok(Some(format!("ready after {}", verify::duration(slowest.max().unwrap_or(0)))));
            Ok(Some(outcomes.iter().map(|(r, o)| o.sentence(r)).collect::<Vec<_>>().join("; ")))
        } else {
            task.step_failed(down.join("; "));
            Err(format!("{done}, but {}", down.join("; ")))
        }
    }

    #[cfg(test)]
    pub(crate) fn with_poll(live: Arc<Live>, demo: Arc<Mutex<DemoCluster>>, tasks: Arc<Tasks>, poll: Duration) -> Arc<Ops> {
        Ops::with_timing(live, demo, tasks, poll, Duration::from_secs(60))
    }

    /// How long an upgrade's follower waits for a lost node, and its maintenance window (tests).
    #[cfg(test)]
    pub(crate) fn set_upgrade_timing(&self, wait: Duration, window: Duration) {
        *self.upgrade_wait.lock().unwrap() = wait;
        *self.upgrade_window.lock().unwrap() = window;
    }

    /// `with_poll`, with a scheduled restart's minute this long.
    #[cfg(test)]
    pub(crate) fn with_timing(live: Arc<Live>, demo: Arc<Mutex<DemoCluster>>, tasks: Arc<Tasks>, poll: Duration, minute: Duration) -> Arc<Ops> {
        let ops = Ops::new(live, demo, tasks);
        let mut ops = Arc::try_unwrap(ops).ok().expect("fresh");
        ops.poll = poll;
        ops.minute = minute;
        Arc::new(ops)
    }

    pub fn set_snapshot_source(&self, f: impl Fn() -> Option<ClusterSnapshot> + Send + Sync + 'static) {
        *self.snapshot.write().unwrap() = Some(Box::new(f));
    }

    pub fn set_emitter(&self, f: impl Fn(&str, Value) + Send + Sync + 'static) {
        *self.emit.write().unwrap() = Some(Box::new(f));
    }

    pub fn tasks(&self) -> &Arc<Tasks> {
        &self.tasks
    }

    fn emit(&self, event: &str, payload: Value) {
        if let Some(f) = self.emit.read().unwrap().as_ref() {
            f(event, payload);
        }
    }

    fn cluster(&self) -> Option<ClusterSnapshot> {
        self.snapshot.read().unwrap().as_ref().and_then(|f| f())
    }

    fn node_view(&self, node: &str) -> Option<NodeSnapshot> {
        self.cluster()?.nodes.into_iter().find(|n| n.id == node)
    }

    /// The node's display name ("cell3"), or its id when unknown.
    fn name_of(&self, node: &str) -> String {
        self.node_view(node).map(|n| n.name).unwrap_or_else(|| node.to_string())
    }

    /// Runs a command on a node (sudo when asked) and attaches its output to its log entry.
    async fn exec(&self, node: &str, cmd: &str, sudo: bool, limit: Duration) -> Result<(Option<u32>, String, String), SbError> {
        let out = if sudo { self.live.sudo(node, cmd, limit).await } else { self.live.run(node, cmd, None, limit).await }.map_err(|e| SbError::classify(&e).on(node))?;
        self.live.audit.attach_output(node, cmd, &out.stdout, &out.stderr);
        Ok((out.code, out.stdout, out.stderr))
    }

    /// Runs `cmd` as a change of `class`; when the node says it isn't allowed, runs it again with
    /// sudo. Only the user-level attempt carries the class: the sudo retry goes untagged, so its
    /// own shape decides (a root kill or renice is `other`, which only an `any` fence admits).
    async fn with_sudo_fallback(&self, class: FenceClass, node: &str, cmd: &str, what: &str) -> Result<(Option<u32>, String, String), SbError> {
        let (code, out, err) = as_class(class, self.exec(node, cmd, false, Duration::from_secs(30))).await?;
        if code == Some(0) {
            return Ok((code, out, err));
        }
        let refused = ["not permitted", "Permission denied", "permission denied"].iter().any(|m| err.contains(m));
        if !refused {
            return Ok((code, out, err));
        }
        let (code, out, err) = self.exec(node, cmd, true, Duration::from_secs(30)).await?;
        if code != Some(0) && err.contains("not permitted") {
            return Err(SbError::failed(format!("couldn't {what} on {node}: {}", err.trim())).on(node));
        }
        Ok((code, out, err))
    }

    /// Notes a simulated command in the activity log (Demo mode).
    fn simulated(&self, node: &str, title: &str, command: &str) {
        self.live.audit.record_simulated(node, title, command);
    }

    /// Demo mode: the simulated command failed, as the node would have said on stderr.
    fn simulated_failed(&self, node: &str, title: &str, command: &str, stderr: &str) -> SbError {
        self.live.audit.record(audit::Entry {
            node: node.into(),
            kind: if command.starts_with("sudo ") { "sudo".into() } else { "command".into() },
            command: command.into(),
            title: title.into(),
            code: Some(1),
            stderr_tail: Some(stderr.into()),
            simulated: true,
            ..Default::default()
        });
        demo_error(stderr, node)
    }

    /// Starts a task for an action on one node and runs `work` inside its activity-log op. The
    /// op's own line is written before anything runs and gets the outcome, so a failure that
    /// stops it before any command (a refused name, a node that isn't connected) is logged too.
    async fn one<T>(
        &self,
        kind: &str,
        title: String,
        node: &str,
        work: impl std::future::Future<Output = Result<(T, String), SbError>>,
    ) -> Result<T, SbError> {
        let task = self.tasks.start(TaskSpec::new(kind, title.clone(), &[node.to_string()]));
        task.step(title.clone());
        let op = OpMeta { title: title.clone(), source: "ui".into() };
        let result = audit::with_op_id(task.id().to_string(), op, async {
            let head = self.live.audit.begin_op(node, &title, "", false);
            let result = work.await;
            self.end_head(&head, &result.as_ref().map(|(_, s)| s.clone()).map_err(Clone::clone));
            result
        })
        .await;
        match &result {
            Ok((_, summary)) => task.finish_ok(summary.clone()),
            Err(e) => task.finish_err(e.message.clone()),
        }
        result.map(|(v, _)| v)
    }

    /// Writes an op's outcome to its head line: the summary, or the error with its stderr.
    fn end_head(&self, head: &str, result: &Result<String, SbError>) {
        match result {
            Ok(summary) => self.live.audit.end(head, audit::Ending::ok(Some(summary.clone()))),
            Err(e) => self.live.audit.end(head, audit::Ending { error: Some(e.message.clone()), stderr_tail: e.detail.clone(), ..Default::default() }),
        }
    }

    // ---- Processes

    /// Sends a signal. With `start_time` (from the snapshot) the node checks first that the
    /// PID still belongs to the same process.
    pub async fn signal(&self, demo: bool, node: &str, pid: u32, signal: &str, start_time: Option<u64>) -> Result<(), SbError> {
        if !matches!(signal, "TERM" | "INT" | "HUP" | "KILL" | "STOP" | "CONT") {
            return Err(SbError::invalid(format!("unsupported signal {signal}")));
        }
        if pid <= 1 {
            return Err(SbError::invalid("SparkBox won't signal process 1."));
        }
        let name = self.node_view(node).and_then(|n| n.processes.into_iter().find(|p| p.pid == pid).map(|p| p.name)).unwrap_or_else(|| format!("process {pid}"));
        let title = format!("Send SIG{signal} to {name} ({pid}) on {}", self.name_of(node));
        let cmd = pid_guard_cmd(pid, signal, start_time, "/proc");
        self.one("process", title.clone(), node, async {
            if demo {
                if let Err(e) = self.demo.lock().unwrap().signal(node, pid, signal) {
                    return Err(self.simulated_failed(node, &title, &cmd, &e));
                }
                self.simulated(node, &title, &cmd);
                return Ok(((), format!("SIG{signal} sent")));
            }
            // The guarded kill is several commands (its shape is `other`): as the cluster user it's
            // a process signal. Its root retry is not (process-test is user-level).
            let (code, _, err) = self.with_sudo_fallback(FenceClass::ProcessTest, node, &cmd, "signal the process").await?;
            match code {
                Some(0) => Ok(((), format!("SIG{signal} sent"))),
                Some(3) if err.contains("gone") => Err(SbError::new(ErrorKind::Gone, "Process already exited (its PID is gone or now belongs to another program).").on(node)),
                _ => Err(SbError::failed(format!("couldn't signal {name} on {node}: {}", err.trim())).on(node).with_detail(err)),
            }
        })
        .await
    }

    pub async fn renice(&self, demo: bool, node: &str, pid: u32, nice: i32) -> Result<(), SbError> {
        if !(-20..=19).contains(&nice) {
            return Err(SbError::invalid("Priority (nice) goes from -20 (highest) to 19 (lowest)."));
        }
        if pid <= 1 {
            return Err(SbError::invalid("SparkBox won't change process 1."));
        }
        let title = format!("Set the priority of process {pid} on {} to {nice}", self.name_of(node));
        let cmd = format!("renice -n {nice} -p {pid}");
        self.one("process", title.clone(), node, async {
            if demo {
                if let Err(e) = self.demo.lock().unwrap().renice(node, pid, nice) {
                    return Err(self.simulated_failed(node, &title, &cmd, &e));
                }
                self.simulated(node, &title, &cmd);
                return Ok(((), format!("Now nice {nice}")));
            }
            let (code, _, err) = self.with_sudo_fallback(FenceClass::ProcessTest, node, &cmd, "change the priority").await?;
            if code == Some(0) {
                Ok(((), format!("Now nice {nice}")))
            } else {
                Err(SbError::failed(format!("couldn't change the priority on {node}: {}", err.trim())).on(node))
            }
        })
        .await
    }

    /// Every listening TCP port with its owner, read with sudo (`ss -tlnpH`).
    pub async fn reveal_ports(&self, demo: bool, node: &str) -> Result<Vec<Listener>, SbError> {
        if demo {
            let title = format!("List listening ports on {} with sudo", self.name_of(node));
            let Some(view) = self.node_view(node) else {
                let e = SbError::new(ErrorKind::Unreachable, format!("{node} isn't connected."));
                self.live.audit.record_failed(node, &title, "sudo ss -tlnpH", &e.message);
                return Err(e);
            };
            if let Err(e) = self.demo.lock().unwrap().sudo_check(node) {
                return Err(self.simulated_failed(node, &title, "sudo ss -tlnpH", &e));
            }
            self.simulated(node, &title, "sudo ss -tlnpH");
            return Ok(services::demo_reveal(&view));
        }
        let (code, out, err) = as_class(FenceClass::SudoProbe, self.exec(node, "ss -tlnpH", true, Duration::from_secs(20))).await?;
        if code != Some(0) {
            return Err(SbError::failed(format!("ss failed on {node}: {}", err.trim())).on(node));
        }
        Ok(services::parse_ss(&out))
    }

    // ---- Services

    pub async fn services(&self, demo: bool, node: &str) -> Result<Vec<Service>, String> {
        if demo {
            let units = self.demo.lock().unwrap().services(node)?;
            return Ok(units
                .into_iter()
                .map(|u| Service { unit: u.unit, description: u.description, load: u.load, active: u.active, sub: u.sub, enabled: u.enabled })
                .collect());
        }
        let units = self
            .live
            .run(node, "systemctl list-units --type=service --all --no-pager --output=json", None, Duration::from_secs(30))
            .await?;
        if units.code != Some(0) {
            return Err(format!("systemctl failed on {node}: {}", units.stderr.trim()));
        }
        let files = self
            .live
            .run(node, "systemctl list-unit-files --type=service --no-pager --output=json", None, Duration::from_secs(30))
            .await?;
        parse_services(&units.stdout, &files.stdout)
    }

    pub async fn service_action(&self, demo: bool, node: &str, unit: &str, action: &str) -> Result<(), SbError> {
        services::check_action(unit, action)?;
        let title = services::action_title(action, unit, &self.name_of(node));
        self.one("service", title.clone(), node, self.service_action_in_op(demo, node, unit, action, &title)).await
    }

    /// The action itself, inside whatever task and op the caller runs. A failure is in the
    /// activity log with what systemctl said, in Demo mode too.
    async fn service_action_in_op(&self, demo: bool, node: &str, unit: &str, action: &str, title: &str) -> Result<((), String), SbError> {
        let cmd = services::action_command(unit, action);
        if demo && services::NO_UNIT.contains(&action) {
            return Err(SbError::new(ErrorKind::DemoOnly, "Available with Live data.").on(node));
        }
        if demo {
            let result = self.demo.lock().unwrap().service_action(node, unit, action);
            if let Err(e) = result {
                return Err(self.simulated_failed(node, title, &format!("sudo {cmd}"), &e));
            }
            self.simulated(node, title, &format!("sudo {cmd}"));
            return Ok(((), services::done(action).into()));
        }
        // Tagged only where its shape is a service change already (the tag keeps its unit); mask,
        // unmask and reset-failed go untagged, so their shape (`other`) decides.
        let run = self.exec(node, &cmd, true, Duration::from_secs(120));
        let (code, _, err) = if services::SERVICE_CLASS.contains(&action) { as_class(FenceClass::Service, run).await? } else { run.await? };
        if code == Some(0) {
            Ok(((), services::done(action).into()))
        } else {
            let what = if unit.is_empty() { format!("systemctl {action}") } else { format!("systemctl {action} {unit}") };
            Err(SbError::failed(format!("{what} failed on {node}: {}", err.trim())).on(node).with_detail(err))
        }
    }

    /// Applies one action to the unit on several nodes, one at a time, stopping at the first
    /// failure. Returns the task's id at once.
    pub fn services_apply(self: &Arc<Self>, demo: bool, nodes: Vec<String>, unit: String, action: String) -> Result<String, SbError> {
        services::check_action(&unit, &action)?;
        if nodes.is_empty() {
            return Err(SbError::invalid("Pick at least one node."));
        }
        let title = services::action_title(&action, &unit, &format!("{} node{}", nodes.len(), if nodes.len() == 1 { "" } else { "s" }));
        let task = self.tasks.start(TaskSpec::new("service", title.clone(), &nodes).cancellable());
        task.set_retry("services_apply", serde_json::json!({ "nodes": nodes, "unit": unit, "action": action }));
        let id = task.id().to_string();
        let me = self.clone();
        tauri::async_runtime::spawn(audit::with_op_id(id.clone(), OpMeta { title: title.clone(), source: "ui".into() }, async move {
            let head = me.live.audit.begin_op(&nodes.join(", "), &title, "", demo);
            let finish = |result: Result<String, SbError>| me.end_head(&head, &result);
            let mut done = 0;
            for (i, node) in nodes.iter().enumerate() {
                let name = me.name_of(node);
                if task.is_cancelled() {
                    for rest in &nodes[i..] {
                        task.step_skipped(me.name_of(rest), "Stopped before this node");
                    }
                    let summary = format!("{done} of {} done before you stopped it", nodes.len());
                    finish(Ok(summary.clone()));
                    return task.finish_cancelled(summary);
                }
                task.step(services::action_title(&action, &unit, &name));
                let line = services::action_title(&action, &unit, &name);
                match me.service_action_in_op(demo, node, &unit, &action, &line).await {
                    Ok(_) => {
                        done += 1;
                        task.step_ok(None);
                    }
                    Err(e) => {
                        task.step_failed(e.message.clone());
                        for rest in &nodes[i + 1..] {
                            task.step_skipped(me.name_of(rest), "Not tried after the failure");
                        }
                        finish(Err(SbError { message: format!("Stopped at {name}: {}", e.message), ..e.clone() }));
                        return task.finish_err(format!("Stopped at {name}: {}", e.message));
                    }
                }
                task.progress(done as f32 / nodes.len() as f32, None);
            }
            let summary = format!("Done on {done} node{}", if done == 1 { "" } else { "s" });
            finish(Ok(summary.clone()));
            task.finish_ok(summary);
        }));
        Ok(id)
    }

    /// `systemctl status` text for one unit.
    pub async fn service_status(&self, demo: bool, node: &str, unit: &str) -> Result<String, String> {
        if !unit_ok(unit) {
            return Err("bad service name".into());
        }
        if demo {
            return self.demo.lock().unwrap().service_status(node, unit);
        }
        // status exits 3 for stopped units; the text is what matters.
        let out = self.live.run(node, &format!("systemctl status --no-pager -n 0 -- {}", q(unit)), None, Duration::from_secs(30)).await?;
        Ok(if out.stdout.trim().is_empty() { out.stderr } else { out.stdout })
    }

    /// The status header parsed, plus the last 25 journal lines with their priority.
    pub async fn service_detail(&self, demo: bool, node: &str, unit: &str) -> Result<services::ServiceDetail, String> {
        let status = self.service_status(demo, node, unit).await?;
        let journal = if demo {
            services::parse_journal_text(&self.demo.lock().unwrap().journal(node, unit, 25)?)
        } else {
            let out = self.live.run(node, &format!("journalctl -u {} -n 25 --no-pager -o json", q(unit)), None, Duration::from_secs(30)).await?;
            services::parse_journal_json(&out.stdout)
        };
        Ok(services::parse_status(unit, &status, journal))
    }

    // ---- Power

    /// Reboots or shuts down a node, now or in a few minutes, and follows it through: the
    /// node going away and, for a restart, coming back on a new boot. Returns the task's id.
    pub fn power(self: &Arc<Self>, demo: bool, node: &str, action: &str, delay_min: u32) -> Result<String, SbError> {
        self.power_with_models(demo, node, action, delay_min, true)
    }

    fn power_with_models(self: &Arc<Self>, demo: bool, node: &str, action: &str, delay_min: u32, verify_models: bool) -> Result<String, SbError> {
        let verb = match action {
            "reboot" => "Restart",
            "poweroff" => "Shut down",
            _ => return Err(SbError::invalid(format!("unsupported power action {action}"))),
        };
        if delay_min > 60 {
            return Err(SbError::invalid("Schedule it within the next hour."));
        }
        let before = self.node_view(node).ok_or_else(|| SbError::new(ErrorKind::Unreachable, format!("{node} isn't in the cluster.")))?;
        if before.status != LinkStatus::Online {
            return Err(SbError::new(ErrorKind::Unreachable, format!("{} isn't connected, so it can't be told to {}.", before.name, verb.to_lowercase())).on(node));
        }
        if action == "reboot" && (before.stale || before.boot_id.as_ref().is_none_or(|b| b.trim().is_empty())) {
            return Err(SbError::invalid(format!("Wait for fresh readings and a boot id from {} before restarting it. Nothing was sent.", before.name)).on(node));
        }
        // Claimed here, before the task starts: one sent a moment ago may still be telling the
        // agents, or be scheduled, or the node still answers on the boot it's leaving. A restart
        // is held as restart-checking until its checks pass, then declared.
        let claim = match (action, delay_min) {
            ("reboot", _) => RESTART_CHECKING,
            (_, 0) => "shutting-down",
            _ => SHUTDOWN_SCHEDULED,
        };
        if let Err(state) = self.claim(demo, node, claim) {
            return Err(SbError::invalid(held_words(&before.name, state)).on(node));
        }
        // An upgrade SparkBox runs there: nothing is sent and no window opens. (Once claimed, no
        // new upgrade starts there: updates.rs looks at the claim.)
        if let Some(why) = self.installing(node, &before.name, if action == "reboot" { "restart it" } else { "shut it down" }) {
            self.undeclare(node);
            return Err(SbError::invalid(why).on(node));
        }
        let name = before.name.clone();
        let title = format!("{verb} {name}");
        let task = self.tasks.start(TaskSpec::new("power", title.clone(), &[node.to_string()]).cancellable());
        task.set_retry("node_power", serde_json::json!({ "node": node, "action": action }));
        let id = task.id().to_string();
        let me = self.clone();
        let (node, action) = (node.to_string(), action.to_string());
        let meta = OpMeta { title: title.clone(), source: "ui".into() };
        tauri::async_runtime::spawn(audit::with_op_id(id.clone(), meta, async move {
            let head = me.live.audit.begin_op(&node, &title, &power_command(&action, delay_min), demo);
            let what = if action == "reboot" { "restart" } else { "shutdown" };
            // Cancel (or look-only coming on) before anything is sent: nothing goes out, the claim
            // is let go, and the task is cancelled.
            let called_off = || {
                me.undeclare(&node);
                let summary = format!("The {what} of {name} was called off before anything was sent.");
                me.live.audit.end_op(&head, true, Some(summary.clone()), None);
                task.finish_cancelled(summary);
            };
            // P1.26: no restart until the node proves the kernel it boots next can come up with its
            // GPU (prereboot.rs); a scheduled one is checked now, as it's scheduled, and again a
            // minute before its time (`power_lifecycle`). Refused, nothing is sent and no window
            // opens. A shutdown isn't checked.
            if action == "reboot" {
                let label = if delay_min == 0 { prereboot::STEP.to_string() } else { format!("{} (run now, when it's scheduled)", prereboot::STEP) };
                let checked = me.checks_before_restart(demo, &task, &node, &label).await;
                if task.is_cancelled() {
                    return called_off();
                }
                if let Err(why) = checked {
                    me.undeclare(&node);
                    me.live.audit.end_op(&head, false, None, Some(why.clone()));
                    return task.finish_err(why);
                }
                // An upgrade task that started just as this one claimed the node (it looked before
                // the claim): the restart waits for it all the same.
                if let Some(why) = me.installing(&node, &name, "restart it") {
                    me.undeclare(&node);
                    me.live.audit.end_op(&head, false, None, Some(why.clone()));
                    return task.finish_err(why);
                }
                me.declare(demo, &node, if delay_min == 0 { "rebooting" } else { RESTART_SCHEDULED });
            }
            // Planned: the alerting (and every node's agent) knows before anything is sent. A
            // restart should be back within its expected time; a shutdown stays planned until
            // the node is woken.
            let planned = me.planned();
            if let Some(p) = &planned {
                let until = (action == "reboot").then(|| now_ms() + u64::from(delay_min) * 60_000 + RESTART_EXPECT_MS);
                let reason = if action == "reboot" { "restarting" } else { "shut down" };
                p.begin(&node, until, reason);
                // The agents hear first, and the task says which didn't (they may page).
                task.step("Telling the nodes' agents about the planned work");
                let missing = p.notify_agents(task.id(), std::slice::from_ref(&node), until, reason).await;
                if missing.is_empty() {
                    task.step_ok(None);
                } else {
                    task.step_ok(Some(maintenance_missing(&missing, &|n: &str| me.node_view(n).map(|v| v.name).unwrap_or_else(|| n.to_string()))));
                }
            }
            // Called off while the agents were told: still nothing sent, and the window closes.
            if task.is_cancelled() {
                if let Some(p) = &planned {
                    p.end(&node);
                }
                return called_off();
            }
            let mut not_called_off = None;
            let result = me.power_lifecycle(demo, &task, &node, &name, &action, delay_min, before.boot_id.clone(), &mut not_called_off).await;
            let off = action == "poweroff" && result.is_ok();
            let post_warning = if action == "reboot" && result.is_ok() { me.checks_after_restart(demo, &task, &node).await } else { None };
            // Only a node that's now off keeps its declared state (until it's woken).
            if !off {
                me.undeclare(&node);
            }
            match result {
                // Cancel was pressed, but the node kept the scheduled one: it went ahead, and the
                // task fails with why (its window stayed open until the node was through).
                result if not_called_off.is_some() => {
                    let outcome = match result {
                        Ok(summary) if action == "reboot" && verify_models => match me.verify(&task, verify::Scope::Nodes(vec![node.clone()]), "Restarted").await {
                            Ok(ready) => ready.map(|r| format!("{summary}. {r}")).unwrap_or(summary),
                            Err(warning) => format!("{summary}. {warning}"),
                        },
                        Ok(summary) => summary,
                        Err(e) => e.message,
                    };
                    let error = format!("{}. {outcome}{}", not_called_off.unwrap_or_default(), post_warning.map(|w| format!(". {w}")).unwrap_or_default());
                    me.live.audit.end_op(&head, false, None, Some(error.clone()));
                    task.finish_failed(error);
                }
                Ok(summary) if action == "reboot" => {
                    let verified = if verify_models { me.verify(&task, verify::Scope::Nodes(vec![node.clone()]), "Restarted").await } else { Ok(None) };
                    let (summary, model_warning) = match verified { Ok(Some(ready)) => (format!("{summary}. {ready}"), None), Ok(None) => (summary, None), Err(warning) => (summary, Some(warning)) };
                    let warnings = post_warning.into_iter().chain(model_warning).collect::<Vec<_>>();
                    me.live.audit.end_op(&head, true, Some(summary.clone()), (!warnings.is_empty()).then(|| warnings.join("; ")));
                    if warnings.is_empty() { task.finish_ok(summary); } else { task.finish_warn(summary, warnings.join("; ")); }
                }
                Ok(summary) => {
                    me.live.audit.end_op(&head, true, Some(summary.clone()), None);
                    task.finish_ok(summary);
                }
                Err(e) if e.kind == ErrorKind::Cancelled => {
                    me.live.audit.end_op(&head, true, Some(e.message.clone()), None);
                    task.finish_cancelled(e.message);
                }
                // Failed, even when Cancel was pressed once the command was on its way: Cancel
                // calls off nothing then, so the task isn't "cancelled".
                Err(e) => {
                    me.live.audit.end_op(&head, false, None, Some(e.message.clone()));
                    task.finish_failed(e.message);
                }
            }
            // A node that's now off stays planned (Wake ends it); anything else is over.
            if let (Some(p), false) = (&planned, off) {
                p.end(&node);
            }
        }));
        Ok(id)
    }

    /// The restart or shutdown itself. From the moment it's sent the node's power state is
    /// declared (`power_overlay`); the proof of a restart is the collector's boot id changing,
    /// not its uptime starting over. It must go offline or show a fresh new boot within a minute,
    /// and be back within five once offline. A scheduled one that Cancel
    /// couldn't call off on the node (`shutdown -c` failed or was refused) is followed through all
    /// the same, with why in `not_called_off`.
    #[allow(clippy::too_many_arguments)]
    async fn power_lifecycle(
        &self,
        demo: bool,
        task: &TaskHandle,
        node: &str,
        name: &str,
        action: &str,
        delay_min: u32,
        boot_before: Option<String>,
        not_called_off: &mut Option<String>,
    ) -> Result<String, SbError> {
        let reboot = action == "reboot";
        let going = if reboot { "rebooting" } else { "shutting-down" };
        let cmd = power_command(action, delay_min);
        if delay_min == 0 {
            self.declare(demo, node, going);
        }
        task.step(if delay_min == 0 { format!("Sending {} to {name}", if reboot { "the restart" } else { "the shutdown" }) } else { format!("Scheduling for {delay_min} min from now") });
        if demo {
            let title = format!("{} {name}", if reboot { "Restart" } else { "Shut down" });
            if let Err(e) = self.demo.lock().unwrap().sudo_check(node) {
                return Err(self.simulated_failed(node, &title, &format!("sudo {cmd}"), &e));
            }
            self.simulated(node, &title, &format!("sudo {cmd}"));
        } else {
            match as_class(FenceClass::PowerOne, self.exec(node, &cmd, true, Duration::from_secs(20))).await {
                Ok((Some(0) | None, _, _)) => {}
                Ok((_, _, err)) => return Err(SbError::failed(format!("{name} refused to {}: {}", if reboot { "restart" } else { "shut down" }, err.trim())).on(node)),
                // Refused before it left (the gate, the write exit) or by sudo: nothing was sent.
                Err(e)
                    if matches!(
                        e.kind,
                        ErrorKind::SudoRejected | ErrorKind::NoPassword | ErrorKind::Unreachable | ErrorKind::Locked | ErrorKind::LookOnly | ErrorKind::Fenced | ErrorKind::OtherController | ErrorKind::WrongNode | ErrorKind::Invalid | ErrorKind::AuthRejected
                    ) =>
                {
                    return Err(e)
                }
                // The session ended because the node went down: that's the command working.
                Err(_) => {}
            }
        }
        // It's on the node now: look-only can't call it off (that would be a change), so a
        // look-only switch says it keeps running there.
        task.on_node();
        if delay_min > 0 {
            let what = if reboot { "restart" } else { "shutdown" };
            let at = Instant::now() + self.minute * delay_min;
            let due = crate::health::maintenance::local_hhmm(now_ms() + (self.minute * delay_min).as_millis() as u64);
            task.step(format!("Waiting: {what} in {delay_min} min (Cancel to call it off)"));
            // P1.26 again a minute before its time (the node may have changed since it was
            // scheduled: an upgrade, a removed package). One scheduled a minute ahead was just
            // checked; Demo has no node to read. If SparkBox isn't running then, the node restarts
            // on its own schedule unchecked (the confirm says so).
            let mut rechecked = !reboot || delay_min <= 1 || demo;
            loop {
                let left = at.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                task.progress(1.0 - left.as_secs_f32() / (self.minute * delay_min).as_secs_f32(), Some(format!("{}s to go", left.as_secs())));
                // Cancel didn't take on the node: it's still due, so wait for it.
                if not_called_off.is_some() {
                    tokio::time::sleep(left.min(Duration::from_secs(1))).await;
                    continue;
                }
                if !rechecked && left <= self.minute {
                    rechecked = true;
                    let checked = self.checks_before_restart(demo, task, node, &format!("{} (again, a minute before it)", prereboot::STEP)).await;
                    // Cancel during the checks: the Cancel below calls it off.
                    if let (Err(reasons), false) = (checked, task.is_cancelled()) {
                        task.step(format!("Calling off the scheduled {what}: its checks failed"));
                        let why = match as_class(FenceClass::PowerOne, self.exec(node, "shutdown -c", true, Duration::from_secs(20))).await {
                            Ok((Some(0), _, _)) => {
                                task.step_ok(Some("shutdown -c".into()));
                                return Err(SbError::failed(reasons).on(node));
                            }
                            Ok((code, _, err)) if err.trim().is_empty() => format!("shutdown -c exited {}", code.map(|c| c.to_string()).unwrap_or("without a status".into())),
                            Ok((_, _, err)) => err.trim().to_string(),
                            Err(e) => e.message,
                        };
                        let verb = if reboot { "restarts" } else { "shuts down" };
                        task.step_failed(format!("Couldn't call off the {what}: {name} still {verb} at {due} ({why})"));
                        task.step(format!("Waiting: {what} at {due}"));
                        *not_called_off = Some(format!("Its checks failed a minute before it ({}), but SparkBox couldn't call off the {what} of {name} ({why}), so it stayed due at {due}", reasons.replace('\n', " ")));
                        continue;
                    }
                }
                tokio::select! {
                    _ = task.cancelled() => {
                        task.step(format!("Calling off the scheduled {what}"));
                        if demo {
                            self.simulated(node, &format!("Cancel the scheduled {what}"), "sudo shutdown -c");
                            return Err(SbError::new(ErrorKind::Cancelled, format!("The scheduled {what} of {name} was called off.")));
                        }
                        // Called off only when the node says so; refused or failed, it still happens.
                        let why = match as_class(FenceClass::PowerOne, self.exec(node, "shutdown -c", true, Duration::from_secs(20))).await {
                            Ok((Some(0), _, _)) => return Err(SbError::new(ErrorKind::Cancelled, format!("The scheduled {what} of {name} was called off."))),
                            Ok((code, _, err)) if err.trim().is_empty() => format!("shutdown -c exited {}", code.map(|c| c.to_string()).unwrap_or("without a status".into())),
                            Ok((_, _, err)) => err.trim().to_string(),
                            Err(e) => e.message,
                        };
                        let verb = if reboot { "restarts" } else { "shuts down" };
                        task.step_failed(format!("Couldn't call off the {what}: {name} still {verb} at {due} ({why})"));
                        task.step(format!("Waiting: {what} at {due}"));
                        *not_called_off = Some(format!("Couldn't call off the {what} of {name} ({why}), so it stayed due at {due}"));
                    }
                    _ = tokio::time::sleep(left.min(Duration::from_secs(1))) => {}
                }
            }
            self.declare(demo, node, going);
            if demo {
                self.demo.lock().unwrap().power(node, action).map_err(|e| demo_error(&e, node))?;
            }
        } else if demo {
            self.demo.lock().unwrap().power(node, action).map_err(|e| demo_error(&e, node))?;
        }

        task.step(format!("Waiting for {name} to go offline"));
        let sent = Instant::now();
        // The connection loop logs in again automatically with normal host-key/identity checks.
        // A fast reboot can finish between snapshots (or before the command channel returns).
        // Fresh readings on a different boot prove that restart even if we missed the outage.
        let new_boot = |n: &NodeSnapshot| {
            n.status == LinkStatus::Online && !n.stale && boot_before.as_ref().is_some_and(|before|
                n.boot_id.as_ref().is_some_and(|b| !b.trim().is_empty() && b != before))
        };
        let observed_offline = std::sync::atomic::AtomicBool::new(false);
        let changed = self.wait_for(task, node, 60, |n| {
            let offline = n.is_none_or(|n| n.status != LinkStatus::Online);
            observed_offline.store(offline, std::sync::atomic::Ordering::Relaxed);
            offline || (reboot && n.is_some_and(new_boot))
        }).await;
        if !changed {
            return Err(SbError::failed(format!(
                "The {} didn't happen: {name} kept answering for a minute. Check it with Fleet Command or a terminal.",
                if reboot { "restart" } else { "shutdown" }
            ))
            .on(node));
        }
        let down = sent.elapsed().as_secs();
        if !reboot {
            self.declare(demo, node, "off");
            return Ok(format!("{name} is off (offline {} after the shutdown was sent). It stays off until someone presses its power button (or Wake-on-LAN).", fmt_minutes(down)));
        }
        let missed_outage = !observed_offline.load(std::sync::atomic::Ordering::Relaxed);
        if missed_outage {
            task.step_ok(Some("Already reconnected on a new boot; the offline interval was not observed.".into()));
        }
        task.step(format!("Waiting for {name} to come back (up to 5 min)"));
        let gone_at = Instant::now();
        let back = self.wait_for(task, node, 300, |n| n.is_some_and(new_boot)).await;
        let now = self.node_view(node);
        if !back {
            if now.as_ref().is_some_and(|n| n.status == LinkStatus::Online && !n.stale) {
                return Err(SbError::failed(format!("{name} answers again on the boot it had before (its boot id didn't change), so it didn't restart.")).on(node));
            }
            return Err(SbError::new(ErrorKind::Timeout, format!("{name} hasn't come back after 5 minutes. It may be stuck in firmware or waiting at a console. SparkBox keeps trying to reconnect; it won't send another restart.")).on(node));
        }
        task.step("Checking that it restarted");
        let Some(boot) = now.and_then(|n| n.boot_id).filter(|_| boot_before.is_some()) else {
            return Err(SbError::failed(format!("{name} answers again, but SparkBox can't tell it restarted: it reports no boot id.")).on(node));
        };
        // On its new boot: the restart is over (the Verify step that follows is about its models).
        self.undeclare(node);
        task.step_ok(Some(format!("New boot {}", boot.chars().take(8).collect::<String>())));
        let total = sent.elapsed().as_secs();
        if missed_outage {
            return Ok(format!("{name} is back on a new boot after {} (the offline interval was not observed)", fmt_minutes(total)));
        }
        Ok(format!("{name} is back after {} (offline {} after the restart was sent, back on a new boot {} later)", fmt_minutes(total), fmt_minutes(down), fmt_minutes(gone_at.elapsed().as_secs())))
    }

    /// Wakes a switched-off node: sends the magic packet (`send`: r2-live's wol::send, which
    /// also powers on the simulated node in Demo), waits for it to come online, then checks
    /// the models it serves answer again. Returns the task's id.
    pub fn wake(self: &Arc<Self>, demo: bool, node: &str, send: BoxFuture<'static, Result<String, String>>) -> Result<String, SbError> {
        let before = self.node_view(node).ok_or_else(|| SbError::new(ErrorKind::Unreachable, format!("{node} isn't in the cluster.")))?;
        let name = before.name.clone();
        if before.status == LinkStatus::Online && before.power_state.is_none() {
            return Err(SbError::invalid(format!("{name} is already on.")).on(node));
        }
        let title = format!("Wake {name}");
        let task = self.tasks.start(TaskSpec::new("power", title.clone(), &[node.to_string()]));
        task.set_retry("node_power", serde_json::json!({ "node": node, "action": "wake" }));
        let id = task.id().to_string();
        let me = self.clone();
        let node = node.to_string();
        tauri::async_runtime::spawn(audit::with_op_id(id.clone(), OpMeta { title: title.clone(), source: "ui".into() }, async move {
            let head = me.live.audit.begin_op(&node, &title, "Wake-on-LAN magic packet", demo);
            let planned = me.planned();
            if let Some(p) = &planned {
                let until = Some(now_ms() + RESTART_EXPECT_MS);
                p.begin(&node, until, "starting up");
                let missing = p.notify_agents(task.id(), std::slice::from_ref(&node), until, "starting up").await;
                if !missing.is_empty() {
                    task.step(maintenance_missing(&missing, &|n: &str| me.node_view(n).map(|v| v.name).unwrap_or_else(|| n.to_string())));
                    task.step_ok(None);
                }
            }
            // Off no longer: waking, until it answers (or the wake fails).
            me.declare(demo, &node, "waking");
            let end_planned = || {
                me.undeclare(&node);
                if let Some(p) = &planned {
                    p.end(&node);
                }
            };
            task.step(format!("Sending the wake packet to {name}"));
            let route = match send.await {
                Ok(route) => route,
                Err(e) => {
                    me.live.audit.end(&head, audit::Ending::failed(e.clone()));
                    task.finish_err(format!("Couldn't wake {name}: {e}"));
                    return end_planned();
                }
            };
            task.step_ok(Some(format!("Sent {route}")));
            task.step(format!("Waiting for {name} to come online (up to 5 min)"));
            let sent = Instant::now();
            let back = me.wait_for(&task, &node, 300, |n| n.is_some_and(|n| n.status == LinkStatus::Online && !n.stale)).await;
            if !back {
                let e = format!("{name} didn't come online within 5 minutes. Wake-on-LAN may be off on its LAN port, or the packet didn't reach it.");
                me.live.audit.end_op(&head, false, None, Some(e.clone()));
                task.finish_err(e);
                return end_planned();
            }
            me.undeclare(&node);
            let summary = format!("{name} is on after {}", fmt_minutes(sent.elapsed().as_secs()));
            me.finish_verified(&task, &head, summary, verify::Scope::Nodes(vec![node.clone()]), "Woke").await;
            end_planned();
        }));
        Ok(id)
    }

    /// Ends a power task through its Verify step: Ok with what answered, or Ok with a warning.
    async fn finish_verified(&self, task: &TaskHandle, head: &str, summary: String, scope: verify::Scope, done: &str) {
        match self.verify(task, scope, done).await {
            Ok(ready) => {
                let summary = ready.map(|r| format!("{summary}. {r}")).unwrap_or(summary);
                self.live.audit.end_op(head, true, Some(summary.clone()), None);
                task.finish_ok(summary);
            }
            Err(warning) => {
                self.live.audit.end(head, audit::Ending { stdout_tail: Some(summary.clone()), error: Some(warning.clone()), ..Default::default() });
                task.finish_warn(summary, warning);
            }
        }
    }

    /// Polls the snapshot until `done` holds for the node; false after `polls` tries.
    async fn wait_for(&self, task: &TaskHandle, node: &str, polls: u32, done: impl Fn(Option<&NodeSnapshot>) -> bool) -> bool {
        for i in 0..polls {
            if done(self.node_view(node).as_ref()) {
                return true;
            }
            task.progress(i as f32 / polls as f32, None);
            tokio::time::sleep(self.poll).await;
        }
        false
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// "cell2 is already restarting." for the power state SparkBox holds on a node.
fn held_words(name: &str, state: &str) -> String {
    let doing = match state {
        "rebooting" => "is already restarting",
        "shutting-down" => "is already shutting down",
        RESTART_SCHEDULED => "already has a restart scheduled",
        SHUTDOWN_SCHEDULED => "already has a shutdown scheduled",
        RESTART_CHECKING => "is being checked before a restart",
        _ => "is already starting up",
    };
    format!("{name} {doing}.")
}

/// What runs on the node: now, or scheduled with a wall message.
pub fn power_command(action: &str, delay_min: u32) -> String {
    match (action, delay_min) {
        ("reboot", 0) => "systemctl reboot".into(),
        ("poweroff", 0) => "systemctl poweroff".into(),
        ("reboot", n) => format!("shutdown -r +{n} {}", q(&format!("SparkBox: restarting in {n} min"))),
        (_, n) => format!("shutdown -P +{n} {}", q(&format!("SparkBox: shutting down in {n} min"))),
    }
}

/// "2m14s", "48s"
pub fn fmt_minutes(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// The simulator's errors read like a node's; give them a kind.
fn demo_error(message: &str, node: &str) -> SbError {
    SbError::classify(message).on(node)
}
