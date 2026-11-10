//! Fleet Command: one shell command on several nodes, like pdsh. Each run is a task; output
//! streams to the UI per node (`fleet://output`) along with each node's state (`fleet://node`).
//! Nodes run all at once, one at a time, or canary first (one node, then the rest if it
//! worked), optionally stopping at the first failure.
//!
//! On the node every command is wrapped as
//! `LC_ALL=C timeout --signal=TERM --kill-after=5s <N>s sh -c '<cmd>'` (inside sudo when it
//! runs as root), so a time limit really stops it there, not just on the Mac.
//!
//! A destructive command never runs on several nodes at once. One that may cut SparkBox's
//! access (stopping SSH, the network, the firewall, a power verb) runs one node at a time,
//! and after each SparkBox logs in to that node again over a new SSH connection (for a
//! restart: once it has gone away and come back) before it touches the next; it stops at the
//! first node it can't reach. A shutdown is through once its node has gone (there's nothing to
//! log in to). A power verb is planned work (P1.15): each node gets a maintenance window and its
//! agent the note before the command, and after a restart the window closes once the node is
//! back and its models answer again (Verify; one that doesn't is the run's warning). A node
//! that's now off stays planned until it's woken. So nothing pages. Before a restart reaches a
//! node, its checks before the restart run (P1.26, prereboot.rs); a node that fails them isn't
//! touched, and the run stops there. A typed restart must be the restart alone (`restart_alone`):
//! the checks read the node just before it, so nothing may change the node in between. Each node
//! is held for its power verb as the node window holds it (`Ops::power`): one power command at a
//! time per node, never while an upgrade SparkBox started runs there.
//! A run of a read-only command is a read-only task (anyone may stop it); everything it
//! records is in the Changes view.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::classify::Power;
use super::Ops;
use crate::errors::{ErrorKind, SbError};
use crate::live::audit::{self, OpMeta};
use crate::live::{sh_quote as q, CancelToken, StreamKind};
use crate::models::verify;
use crate::tasks::{TaskHandle, TaskSpec};

/// Output kept per node and stream; the rest is dropped (and the result says so).
pub const OUTPUT_CAP: usize = 1024 * 1024;
const KILL_AFTER_S: u64 = 5;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FleetResult {
    pub node: String,
    pub code: Option<u32>,
    pub stdout: String,
    pub stderr: String,
    pub ms: u64,
    pub error: Option<String>,
    /// The time limit stopped it.
    pub timed_out: bool,
    /// More than 1 MiB of output; the start is kept.
    pub truncated: bool,
    /// ok | failed | timedOut | skipped | cancelled
    pub state: String,
    /// Ok, but its check found a problem: a model on a node it restarted didn't answer again.
    pub warning: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Strategy {
    #[default]
    Parallel,
    OneByOne,
    Canary,
}

#[derive(Clone, Debug, Default)]
pub struct FleetSpec {
    pub nodes: Vec<String>,
    pub command: String,
    pub sudo: bool,
    pub timeout_s: u64,
    pub strategy: Strategy,
    pub stop_on_failure: bool,
    /// A snippet's name ("Fabric links"): the task is "Fleet: Fabric links", the command its subtitle.
    pub title: Option<String>,
    /// The window's plan cuts access (a power verb meant for several nodes, run as a canary and
    /// then the rest). Only adds the reach-again check; the command itself can only add it too.
    pub cuts_access: bool,
}

/// How long a node that was told to restart gets to go away, and then to come back.
const POWER_DOWN_S: u64 = 120;
const POWER_BACK_S: u64 = 360;
/// How long SparkBox keeps trying to log in again after a command that may cut access.
const REACH_S: u64 = 60;

/// What actually runs on the node. `Live::run` adds `LC_ALL=C` in front; inside sudo the
/// environment is reset, so `env` sets it there (sudo may refuse VAR=value on its own line).
pub fn wrap(command: &str, timeout_s: u64, sudo: bool) -> String {
    let inner = format!("timeout --signal=TERM --kill-after={KILL_AFTER_S}s {timeout_s}s sh -c {}", q(command));
    if sudo {
        format!("env LC_ALL=C {inner}")
    } else {
        inner
    }
}

/// Turns output chunks into text without splitting a multi-byte character between two.
#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
    all: Vec<u8>,
}

impl Utf8Stream {
    /// The text that's complete so far (what isn't is kept for the next chunk).
    fn push(&mut self, data: &[u8]) -> String {
        self.all.extend_from_slice(data);
        self.pending.extend_from_slice(data);
        let upto = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => self.pending.len(),
        };
        let text = String::from_utf8_lossy(&self.pending[..upto]).into_owned();
        self.pending.drain(..upto);
        text
    }

    fn finish(self) -> String {
        String::from_utf8_lossy(&self.all).into_owned()
    }
}

/// Keeps the first `OUTPUT_CAP` bytes (on a character boundary); true when it cut.
pub fn cap(text: &mut String) -> bool {
    if text.len() <= OUTPUT_CAP {
        return false;
    }
    let mut at = OUTPUT_CAP;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text.truncate(at);
    true
}

/// What a node's result says when its run was stopped (`state_of` reads it).
const CANCELED: &str = "Canceled";

fn state_of(r: &FleetResult) -> &'static str {
    if r.error.as_deref() == Some(CANCELED) {
        "cancelled"
    } else if r.timed_out {
        "timedOut"
    } else if r.error.is_some() || r.code != Some(0) {
        "failed"
    } else {
        "ok"
    }
}

impl Ops {
    /// Runs one command on several nodes and waits for all of them (the original API).
    pub async fn fleet(self: &Arc<Self>, demo: bool, nodes: &[String], command: &str, sudo: bool, timeout_s: u64) -> Vec<FleetResult> {
        let spec = FleetSpec { nodes: nodes.to_vec(), command: command.into(), sudo, timeout_s, ..Default::default() };
        match self.fleet_start(demo, spec) {
            Ok(id) => {
                // The run enforces its own limit; this only guards against it never reporting.
                let give_up = Instant::now() + Duration::from_secs(timeout_s.clamp(1, 3600) + KILL_AFTER_S + 30);
                while Instant::now() < give_up {
                    if let Some(results) = self.fleet_results(&id) {
                        return results;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                let error = Some("The run didn't report back.".to_string());
                nodes.iter().map(|n| FleetResult { node: n.clone(), error: error.clone(), state: "failed".into(), ..Default::default() }).collect()
            }
            Err(e) => nodes.iter().map(|n| FleetResult { node: n.clone(), error: Some(e.message.clone()), state: "failed".into(), ..Default::default() }).collect(),
        }
    }

    /// Starts a run and returns its id (also its task's id) at once.
    pub fn fleet_start(self: &Arc<Self>, demo: bool, spec: FleetSpec) -> Result<String, SbError> {
        let command = spec.command.trim().to_string();
        if command.is_empty() {
            return Err(SbError::invalid("Type a command first."));
        }
        if spec.nodes.is_empty() {
            return Err(SbError::invalid("Pick at least one node."));
        }
        let timeout_s = spec.timeout_s.clamp(1, 3600);
        let verdict = super::classify::verdict(&command);
        // P1.26 reads the node just before its restart: a command that changes something first
        // (`apt-get install … && reboot`) would boot what the checks never saw.
        if verdict.power == Some(Power::Restart) && !restart_alone(&command) {
            return Err(SbError::invalid("Run the restart on its own: SparkBox checks the kernel the node boots next just before it restarts."));
        }
        // A node SparkBox is restarting, shutting down or upgrading already: nothing is sent.
        if let Some(p) = verdict.power {
            let again = if p == Power::Restart { "restart it" } else { "shut it down" };
            if let Some(why) = spec.nodes.iter().find_map(|n| self.power_busy(n, again)) {
                return Err(SbError::invalid(why));
            }
        }
        // A command that can destroy data or take nodes down never runs on several nodes at
        // once, whatever the window asked for: one at a time, stopping at the first failure.
        let destructive = verdict.kind == super::classify::Kind::Destructive;
        // A power verb, even on one node: SparkBox follows the node away and back (and keeps its
        // maintenance window open meanwhile).
        let cuts_access = verdict.cuts_access || verdict.power.is_some() || spec.cuts_access;
        // Docker is root-equivalent (P1.42): a root docker command runs one node at a time too,
        // so a bad one doesn't hit the whole fleet at once (the UI runs a canary first).
        let docker_root = verdict.root && spec.nodes.len() > 1;
        let (strategy, stop_on_failure) = if (destructive && spec.nodes.len() > 1) || cuts_access || docker_root { (Strategy::OneByOne, true) } else { (spec.strategy, spec.stop_on_failure) };
        let read_only = verdict.kind == super::classify::Kind::Read && !spec.sudo && !super::classify::escalates(&command);
        let root = if spec.sudo { " (as root)" } else { "" };
        let (title, subtitle) = match spec.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            Some(name) => (format!("Fleet: {name}{root}"), Some(command.clone())),
            None => {
                let short: String = command.chars().take(60).collect();
                (format!("Fleet: {short}{}{root}", if command.chars().count() > 60 { "…" } else { "" }), None)
            }
        };
        let mut task_spec = TaskSpec::new("fleet", title.clone(), &spec.nodes).cancellable();
        if let Some(sub) = subtitle {
            task_spec = task_spec.subtitle(sub);
        }
        if read_only {
            task_spec = task_spec.read_only();
        }
        let task = self.tasks.start(task_spec);
        task.set_retry(
            "fleet_start",
            json!({ "nodes": spec.nodes, "command": command, "sudo": spec.sudo, "timeoutS": timeout_s, "strategy": strategy, "stopOnFailure": stop_on_failure, "title": spec.title, "cutsAccess": cuts_access }),
        );
        let id = task.id().to_string();
        let me = self.clone();
        let power = verdict.power;
        let spec = FleetSpec { command, timeout_s, strategy, stop_on_failure, cuts_access, ..spec };
        let run = audit::with_op_id(id.clone(), OpMeta { title: title.clone(), source: "ui".into() }, async move {
            let head = me.live.audit.begin_op(&spec.nodes.join(", "), &title, &spec.command, demo);
            for n in &spec.nodes {
                me.node_event(task.id(), n, "queued", None, None);
            }
            let results = me.run_strategy(demo, &task, &spec, power).await;
            let ok = results.iter().filter(|r| r.state == "ok").count();
            let timed = results.iter().filter(|r| r.timed_out).count();
            let mut summary = format!("{ok} of {} ok", results.len());
            if timed > 0 {
                summary += &format!(" · {timed} timed out");
            }
            let skipped = results.iter().filter(|r| r.state == "skipped").count();
            if skipped > 0 {
                summary += &format!(" · {skipped} skipped");
            }
            me.live.audit.end_op(&head, ok == results.len(), Some(summary.clone()), None);
            {
                let mut runs = me.fleet_runs.lock().unwrap();
                runs.push((task.id().to_string(), results.clone()));
                let extra = runs.len().saturating_sub(20);
                runs.drain(..extra);
            }
            let warnings: Vec<String> = results.iter().filter_map(|r| r.warning.clone()).collect();
            if task.is_cancelled() {
                task.finish_cancelled(summary);
            } else if ok == results.len() && !warnings.is_empty() {
                task.finish_warn(summary, warnings.join("\n"));
            } else if ok == results.len() {
                task.finish_ok(summary);
            } else {
                task.finish_err(summary);
            }
        });
        tauri::async_runtime::spawn(audit::as_typed(run));
        Ok(id)
    }

    /// The results of a finished run (None while it's running or unknown).
    pub fn fleet_results(&self, run_id: &str) -> Option<Vec<FleetResult>> {
        self.fleet_runs.lock().unwrap().iter().find(|(id, _)| id == run_id).map(|(_, r)| r.clone())
    }

    async fn run_strategy(&self, demo: bool, task: &TaskHandle, spec: &FleetSpec, power: Option<Power>) -> Vec<FleetResult> {
        let nodes = &spec.nodes;
        let skip = |node: &str, why: &str| {
            self.node_event(task.id(), node, "skipped", None, None);
            task.step_skipped(self.name_of(node), why);
            FleetResult { node: node.into(), state: "skipped".into(), error: Some(why.into()), ..Default::default() }
        };
        if spec.cuts_access {
            return self.run_cutting_access(demo, task, spec, power).await;
        }
        match spec.strategy {
            Strategy::Parallel => {
                task.step(format!("Running on {} nodes at once", nodes.len()));
                join_all(nodes.iter().map(|n| self.run_one(demo, task, n, spec))).await
            }
            Strategy::OneByOne => {
                let mut out = Vec::new();
                let mut failed = false;
                for (i, n) in nodes.iter().enumerate() {
                    if task.is_cancelled() {
                        out.push(skip(n, "Stopped before this node"));
                        continue;
                    }
                    if failed && spec.stop_on_failure {
                        out.push(skip(n, "Not run after an earlier node failed"));
                        continue;
                    }
                    task.step(format!("{} ({} of {})", self.name_of(n), i + 1, nodes.len()));
                    let r = self.run_one(demo, task, n, spec).await;
                    if r.state != "ok" {
                        failed = true;
                        task.step_failed(r.error.clone().unwrap_or_else(|| format!("exit {}", r.code.map(|c| c.to_string()).unwrap_or("?".into()))));
                    }
                    out.push(r);
                    task.progress((i + 1) as f32 / nodes.len() as f32, None);
                }
                out
            }
            Strategy::Canary => {
                let (first, rest) = nodes.split_first().expect("at least one node");
                task.step(format!("Canary: {} first", self.name_of(first)));
                let canary = self.run_one(demo, task, first, spec).await;
                let ok = canary.state == "ok";
                let mut out = vec![canary];
                if !ok {
                    task.step_failed("The canary failed, so the other nodes weren't touched");
                    out.extend(rest.iter().map(|n| skip(n, "The canary failed")));
                    return out;
                }
                if task.is_cancelled() {
                    out.extend(rest.iter().map(|n| skip(n, "Stopped after the canary")));
                    return out;
                }
                if !rest.is_empty() {
                    task.step(format!("The other {} node{}", rest.len(), if rest.len() == 1 { "" } else { "s" }));
                    out.extend(join_all(rest.iter().map(|n| self.run_one(demo, task, n, spec))).await);
                }
                out
            }
        }
    }

    /// One node at a time; after each, SparkBox must log in to it again (or, after a shutdown,
    /// see it gone) before the next one, and it stops at the first failure or the first node it
    /// can't reach. A power verb opens each node's maintenance window first and closes it once the
    /// node is through: after a restart, once its models answer again.
    async fn run_cutting_access(&self, demo: bool, task: &TaskHandle, spec: &FleetSpec, power: Option<Power>) -> Vec<FleetResult> {
        let mut out = Vec::new();
        let mut stopped: Option<String> = None;
        let planned = power.and(self.planned());
        for (i, n) in spec.nodes.iter().enumerate() {
            if task.is_cancelled() {
                self.node_event(task.id(), n, "skipped", None, None);
                task.step_skipped(self.name_of(n), "Stopped before this node");
                out.push(FleetResult { node: n.clone(), state: "skipped".into(), error: Some("Stopped before this node".into()), ..Default::default() });
                continue;
            }
            if let Some(why) = &stopped {
                self.node_event(task.id(), n, "skipped", None, None);
                task.step_skipped(self.name_of(n), why.clone());
                out.push(FleetResult { node: n.clone(), state: "skipped".into(), error: Some(why.clone()), ..Default::default() });
                continue;
            }
            // Held for its power verb as the node window holds it (`Ops::power`): a restart as
            // restart-checking until its checks pass, then rebooting; a shutdown as shutting-down.
            // Held already, or upgrading, it isn't sent anything and the run stops here.
            if let Some(pw) = power {
                let name = self.name_of(n);
                let again = if pw == Power::Restart { "restart it" } else { "shut it down" };
                let held = match self.claim(demo, n, if pw == Power::Restart { super::RESTART_CHECKING } else { "shutting-down" }) {
                    Err(state) => Some(super::held_words(&name, state)),
                    Ok(()) => self.installing(n, &name, again).inspect(|_| self.undeclare(n)),
                };
                if let Some(why) = held {
                    task.step(format!("{name} ({} of {})", i + 1, spec.nodes.len()));
                    task.step_failed(why.clone());
                    out.push(self.not_sent(task, n, why));
                    stopped = Some(format!("Not run: {name} didn't come through"));
                    task.progress((i + 1) as f32 / spec.nodes.len() as f32, None);
                    continue;
                }
            }
            // P1.26: a node restarts only once it proves the kernel it boots next can come up with
            // its GPU (prereboot.rs). Refused, nothing is sent, no window opens, and the run stops
            // here like at any node that fails. Cancel meanwhile: nothing is sent either.
            if power == Some(Power::Restart) {
                let label = format!("{} of {}", super::prereboot::STEP, self.name_of(n));
                let checked = self.checks_before_restart(demo, task, n, &label).await;
                if task.is_cancelled() {
                    self.undeclare(n);
                    self.node_event(task.id(), n, "cancelled", None, None);
                    out.push(FleetResult { node: n.clone(), error: Some(CANCELED.into()), state: "cancelled".into(), ..Default::default() });
                    continue;
                }
                // Refused by its checks, or an upgrade task started just as the node was claimed.
                if let Some(why) = checked.err().or_else(|| self.installing(n, &self.name_of(n), "restart it")) {
                    self.undeclare(n);
                    out.push(self.not_sent(task, n, why));
                    stopped = Some(format!("Not run: {} didn't come through", self.name_of(n)));
                    task.progress((i + 1) as f32 / spec.nodes.len() as f32, None);
                    continue;
                }
                self.declare(demo, n, "rebooting");
            }
            if let (Some(p), Some(pw)) = (&planned, power) {
                self.begin_planned(task, p, n, pw).await;
            }
            task.step(format!("{} ({} of {})", self.name_of(n), i + 1, spec.nodes.len()));
            let boot = self.node_view(n).and_then(|v| v.boot_id);
            let mut r = self.run_one(demo, task, n, spec).await;
            if power.is_some() && went_with_the_node(&r) {
                // Its session closed before the exit status came back: the power verb went out.
                // Whether it took is what follows (the node going away and coming back).
                task.log(format!("{}: the connection closed as the command ran", self.name_of(n)));
                r.state = "ok".into();
                r.error = None;
                self.emit("fleet://node", json!({ "runId": task.id(), "node": n, "state": r.state, "code": r.code, "ms": r.ms, "error": r.error, "truncated": r.truncated }));
            }
            if r.state == "ok" {
                match self.reach_again(demo, task, n, power, boot).await {
                    Err(why) => {
                        task.step_failed(why.clone());
                        r.state = "failed".into();
                        r.error = Some(why);
                        self.emit("fleet://node", json!({ "runId": task.id(), "node": n, "state": r.state, "code": r.code, "ms": r.ms, "error": r.error, "truncated": r.truncated }));
                    }
                    // Back: its models answer again before its window closes, as after the node
                    // window's Restart (one still loading would page the moment it closed). One
                    // that doesn't is a warning; the node itself came through. On its new boot
                    // nothing is declared any more (the models' watcher waits for that).
                    Ok(()) if power == Some(Power::Restart) => {
                        self.undeclare(n);
                        r.warning = self.checks_after_restart(demo, task, n).await;
                        if let Err(warning) = self.verify(task, verify::Scope::Nodes(vec![n.clone()]), &format!("{} restarted", self.name_of(n))).await {
                            r.warning = Some(r.warning.map(|old| format!("{old}; {warning}")).unwrap_or(warning));
                        }
                    }
                    Ok(()) => {}
                }
            } else {
                task.step_failed(r.error.clone().unwrap_or_else(|| format!("exit {}", r.code.map(|c| c.to_string()).unwrap_or("?".into()))));
            }
            if let (Some(p), Some(pw)) = (&planned, power) {
                // Through (or failed): the window closes. A node a shutdown took off stays planned until it's woken.
                let off = pw == Power::Off && self.node_view(n).is_none_or(|v| v.status != crate::model::LinkStatus::Online);
                if !off {
                    p.end(n);
                }
            }
            // Only a node a shutdown took off keeps a declared state (off, until it's woken).
            match power {
                Some(Power::Off) if r.state == "ok" => self.declare(demo, n, "off"),
                Some(_) => self.undeclare(n),
                None => {}
            }
            if r.state != "ok" || r.warning.is_some() {
                stopped = Some(format!("Not run: {} didn't come through", self.name_of(n)));
            }
            out.push(r);
            task.progress((i + 1) as f32 / spec.nodes.len() as f32, None);
        }
        out
    }

    /// Before a power verb reaches `node`: its maintenance window opens (a restart should be back
    /// within the expected time; a shutdown stays open until it's woken), and every node's agent
    /// hears first, as for the node window's Restart (ops/mod.rs `power`).
    async fn begin_planned(&self, task: &TaskHandle, p: &Arc<dyn super::Planned>, node: &str, power: Power) {
        let (until, reason) = match power {
            Power::Restart => (Some(super::now_ms() + super::RESTART_EXPECT_MS), "restarting"),
            Power::Off => (None, "shut down"),
        };
        p.begin(node, until, reason);
        task.step("Telling the nodes' agents about the planned work");
        // In a task of its own, outside this run's typed scope: the note is the agents' own
        // request, not part of the command that was typed.
        let (agents, op, target) = (p.clone(), task.id().to_string(), node.to_string());
        let told = tauri::async_runtime::spawn(async move { agents.notify_agents(&op, std::slice::from_ref(&target), until, reason).await });
        let missing = told.await.unwrap_or_else(|e| vec![("the agents".to_string(), e.to_string())]);
        if missing.is_empty() {
            task.step_ok(None);
        } else {
            task.step_ok(Some(super::maintenance_missing(&missing, &|n: &str| self.name_of(n))));
        }
    }

    /// A node the run sends nothing to (held, upgrading, or refused by its checks): failed, with why.
    fn not_sent(&self, task: &TaskHandle, node: &str, why: String) -> FleetResult {
        task.log(format!("{}: not sent", self.name_of(node)));
        let r = FleetResult { node: node.into(), state: "failed".into(), error: Some(why), ..Default::default() };
        self.emit("fleet://node", json!({ "runId": task.id(), "node": node, "state": r.state, "code": r.code, "ms": r.ms, "error": r.error, "truncated": r.truncated }));
        r
    }

    /// After a command that may cut access: SparkBox logs in to the node again over a new SSH
    /// connection (the open one can outlive a stopped sshd). After a restart the node must go
    /// away first and then come back; after a shutdown, gone is through. Demo nodes answer when
    /// their snapshot says they're back. Gone and back are the node's link and boot id, not its
    /// power state: the run declares that itself (`run_cutting_access`).
    async fn reach_again(&self, demo: bool, task: &TaskHandle, node: &str, power: Option<Power>, boot_before: Option<String>) -> Result<(), String> {
        let name = self.name_of(node);
        let not_touched = "the other nodes weren't touched";
        let polls = |secs: u64| (Duration::from_secs(secs).as_millis() / self.poll.as_millis().max(1)) as u32;
        if let Some(p) = power {
            task.step(format!("Waiting for {name} to go away"));
            let gone = self.wait_for(task, node, polls(POWER_DOWN_S), |n| n.map(|n| n.status != crate::model::LinkStatus::Online).unwrap_or(true)).await;
            if !gone {
                return Err(format!("{name} didn't go down within {} minutes, so {not_touched}.", POWER_DOWN_S / 60));
            }
            // Off is what was asked: there's nothing to log in to. Its window stays open.
            if p == Power::Off {
                task.step_ok(Some(format!("{name} is off (planned until it's woken)")));
                return Ok(());
            }
            task.step(format!("Waiting for {name} to come back (up to {} min)", POWER_BACK_S / 60));
            // Back on a new boot (the collector's boot id is the proof, when the node reports one).
            let new_boot = |n: &crate::model::NodeSnapshot| boot_before.as_ref().is_none_or(|b| n.boot_id.as_ref().is_some_and(|now| now != b));
            let back = self.wait_for(task, node, polls(POWER_BACK_S), |n| n.is_some_and(|n| n.status == crate::model::LinkStatus::Online && !n.stale && new_boot(n))).await;
            if !back {
                if self.node_view(node).is_some_and(|n| n.status == crate::model::LinkStatus::Online && !n.stale) {
                    return Err(format!("{name} answers again on the boot it had before, so it didn't restart; {not_touched}."));
                }
                return Err(format!("{name} didn't come back within {} minutes, so {not_touched}.", POWER_BACK_S / 60));
            }
        }
        task.step(format!("Logging in to {name} again over SSH"));
        let started = Instant::now();
        if demo {
            let up = self.wait_for(task, node, polls(REACH_S), |n| n.is_some_and(|n| n.status == crate::model::LinkStatus::Online)).await;
            return if up {
                task.step_ok(Some("Logged in again (simulated)".into()));
                Ok(())
            } else {
                Err(format!("SparkBox can't reach {name} any more, so {not_touched}."))
            };
        }
        // 60 tries a poll apart, at most a minute (the tests poll every few milliseconds).
        let limit = (self.poll * 60).min(Duration::from_secs(REACH_S));
        let mut last = String::new();
        while started.elapsed() < limit {
            match self.live.reach_fresh(node).await {
                Ok(()) => {
                    task.step_ok(Some(format!("Logged in again after {}", super::fmt_minutes(started.elapsed().as_secs()))));
                    return Ok(());
                }
                Err(e) => last = e,
            }
            if task.is_cancelled() {
                break;
            }
            tokio::time::sleep(self.poll).await;
        }
        Err(format!("SparkBox can't log in to {name} over SSH any more ({last}), so {not_touched}."))
    }

    async fn run_one(&self, demo: bool, task: &TaskHandle, node: &str, spec: &FleetSpec) -> FleetResult {
        let started = Instant::now();
        if task.is_cancelled() {
            self.node_event(task.id(), node, "cancelled", None, None);
            return FleetResult { node: node.into(), error: Some(CANCELED.into()), state: "cancelled".into(), ..Default::default() };
        }
        self.node_event(task.id(), node, "running", None, None);
        let cmd = wrap(&spec.command, spec.timeout_s, spec.sudo);
        let mut r = if demo { self.run_demo(task, node, spec, &cmd).await } else { self.run_live(task, node, spec, &cmd).await };
        r.ms = started.elapsed().as_millis() as u64;
        r.truncated |= cap(&mut r.stdout) | cap(&mut r.stderr);
        r.state = state_of(&r).into();
        let line = match (&r.error, r.code) {
            (Some(e), _) => format!("{}: {e}", self.name_of(node)),
            (None, Some(c)) => format!("{}: exit {c}{}", self.name_of(node), if r.timed_out { " (time limit)" } else { "" }),
            (None, None) => format!("{}: no exit code", self.name_of(node)),
        };
        task.log(line);
        self.emit(
            "fleet://node",
            json!({ "runId": task.id(), "node": node, "state": r.state, "code": r.code, "ms": r.ms, "error": r.error, "truncated": r.truncated }),
        );
        r
    }

    async fn run_live(&self, task: &TaskHandle, node: &str, spec: &FleetSpec, cmd: &str) -> FleetResult {
        // The node's own timeout stops it first; this is the backstop if the node hangs.
        let limit = Duration::from_secs(spec.timeout_s + KILL_AFTER_S + 10);
        let cancel = CancelToken::new();
        let relay = {
            let (token, cancelled) = (cancel.clone(), task.cancelled());
            tokio::spawn(async move {
                cancelled.await;
                token.cancel();
            })
        };
        let started = Instant::now();
        let (mut out_text, mut err_text) = (Utf8Stream::default(), Utf8Stream::default());
        let result = if spec.sudo {
            // sudo answers once it's done; the output arrives in one piece.
            tokio::select! {
                r = self.live.sudo(node, cmd, limit) => r.inspect(|o| {
                    self.chunk(task.id(), node, "stdout", &out_text.push(o.stdout.as_bytes()));
                    self.chunk(task.id(), node, "stderr", &err_text.push(o.stderr.as_bytes()));
                }),
                _ = cancel.cancelled() => Err(CANCELED.to_string()),
            }
        } else {
            let run_id = task.id().to_string();
            self.live
                .stream_cmd(node, cmd, limit, cancel.clone(), |kind, data| match kind {
                    StreamKind::Stdout => self.chunk(&run_id, node, "stdout", &out_text.push(data)),
                    StreamKind::Stderr => self.chunk(&run_id, node, "stderr", &err_text.push(data)),
                })
                .await
        };
        relay.abort();
        let (stdout, stderr) = (out_text.finish(), err_text.finish());
        match result {
            Ok(out) if out.cancelled => FleetResult { node: node.into(), stdout, stderr, error: Some(CANCELED.into()), ..Default::default() },
            Ok(out) => {
                self.live.audit.attach_output(node, cmd, &stdout, &stderr);
                let timed_out = out.timed_out || out.code == Some(124) || (out.code == Some(137) && started.elapsed().as_secs() >= spec.timeout_s);
                FleetResult { node: node.into(), code: out.code, stdout, stderr, timed_out, truncated: out.truncated, ..Default::default() }
            }
            Err(e) => {
                let timed_out = e.contains("took longer") || e.contains("timed out");
                FleetResult { node: node.into(), stdout, stderr, error: Some(e), timed_out, ..Default::default() }
            }
        }
    }

    /// Demo: the simulator's reply, streamed line by line over 100–800 ms. `sleep N` really
    /// waits, so the time limit and Cancel can be tried.
    async fn run_demo(&self, task: &TaskHandle, node: &str, spec: &FleetSpec, cmd: &str) -> FleetResult {
        let command = spec.command.trim();
        // What would have run, logged before and completed with the simulated outcome (the run's
        // own title comes from its operation).
        let entry = self.live.audit.begin(audit::Entry {
            node: node.into(),
            kind: if spec.sudo { "sudo".into() } else { "command".into() },
            command: if spec.sudo { format!("sudo {cmd}") } else { cmd.to_string() },
            simulated: true,
            ..Default::default()
        });
        let cancelled = |me: &Self| {
            me.live.audit.end(&entry, audit::Ending::failed(CANCELED));
            FleetResult { node: node.into(), error: Some(CANCELED.into()), ..Default::default() }
        };
        if let Some(secs) = demo_sleep(command) {
            let wait = Duration::from_secs_f64(secs.min(spec.timeout_s as f64));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = task.cancelled() => return cancelled(self),
            }
            let timed_out = secs > spec.timeout_s as f64;
            let code = if timed_out { 124 } else { 0 };
            self.live.audit.end(&entry, audit::Ending { code: Some(code), error: timed_out.then(|| "Took longer than the time limit; stopped".to_string()), ..Default::default() });
            return FleetResult { node: node.into(), code: Some(code), timed_out, ..Default::default() };
        }
        let sent = if spec.sudo { format!("sudo {command}") } else { command.to_string() };
        let (mut code, mut stdout, mut stderr) = self.demo.lock().unwrap().fleet_reply(node, &sent);
        // The simulator has no sshd, firewall or power button of its own: a command that would
        // cut access or power the node down just works there, and a power verb takes the
        // simulated node down (and, for a restart, back up).
        let verdict = super::classify::verdict(command);
        if code == 127 && (verdict.cuts_access || verdict.power.is_some()) {
            (code, stdout, stderr) = (0, String::new(), String::new());
        }
        if code == 0 {
            if let Some(p) = verdict.power {
                let _ = self.demo.lock().unwrap().power(node, if p == Power::Restart { "reboot" } else { "poweroff" });
            }
        }
        let lines: Vec<&str> = stdout.split_inclusive('\n').collect();
        let total = 100 + (fastrand::u64(0..700));
        let per = Duration::from_millis(total / (lines.len() as u64 + 1));
        for line in &lines {
            tokio::select! {
                _ = tokio::time::sleep(per) => {}
                _ = task.cancelled() => return cancelled(self),
            }
            self.chunk(task.id(), node, "stdout", line);
        }
        tokio::time::sleep(per).await;
        self.chunk(task.id(), node, "stderr", &stderr);
        self.live.audit.end(&entry, audit::Ending { code: Some(code), stdout_tail: Some(stdout.clone()), stderr_tail: Some(stderr.clone()), ..Default::default() });
        FleetResult { node: node.into(), code: Some(code), stdout, stderr, ..Default::default() }
    }

    fn chunk(&self, run_id: &str, node: &str, stream: &str, text: &str) {
        if !text.is_empty() {
            self.emit("fleet://output", json!({ "runId": run_id, "node": node, "stream": stream, "chunk": text }));
        }
    }

    fn node_event(&self, run_id: &str, node: &str, state: &str, code: Option<u32>, ms: Option<u64>) {
        self.emit("fleet://node", json!({ "runId": run_id, "node": node, "state": state, "code": code, "ms": ms }));
    }
}

/// The node's session closed under the command (no exit status, no time limit, not refused by
/// the gate, the write exit or sudo): what a power verb's own run looks like when the node goes
/// down before its answer is sent.
fn went_with_the_node(r: &FleetResult) -> bool {
    r.state == "failed" && r.code.is_none() && !r.timed_out && r.error.as_deref().is_none_or(|e| SbError::classify(e).kind == ErrorKind::Failed)
}

/// A typed restart that is the restart and nothing else: one simple command (no `;`, `&&`, `||`,
/// pipe, `&`, substitution or redirection) whose program is a power verb (`reboot`, `shutdown`,
/// `systemctl`, `init`, `telinit`, a bus call), after `sudo`, `env`, `nohup`, `nice`, `timeout` or
/// `chroot`, or as the whole script of `sh -c`/`bash -c`. Anything else that restarts is refused
/// by Fleet: the checks before a restart read the node just before the command is sent.
pub fn restart_alone(cmd: &str) -> bool {
    restart_alone_at(cmd, 0)
}

fn restart_alone_at(cmd: &str, depth: usize) -> bool {
    let Some(segs) = crate::live::fence_class::segments(cmd.trim()) else { return false };
    let [words] = segs.as_slice() else { return false };
    if depth > 4 || words.iter().any(|w| w.contains("$(") || w.contains('`') || w.contains('>') || w.contains('<')) {
        return false;
    }
    let mut rest: Vec<&str> = words.iter().map(String::as_str).collect();
    loop {
        let Some((&first, args)) = rest.split_first() else { return false };
        let name = first.rsplit('/').next().unwrap_or(first);
        // Each wrapper's options that take a value, the ones that make it run something else
        // (a login shell, a string split into a command), and the operands before the command.
        let (valued, refused, operands): (&[&str], &[&str], usize) = match name {
            "reboot" | "poweroff" | "halt" | "shutdown" | "systemctl" | "init" | "telinit" | "busctl" | "gdbus" | "dbus-send" => return true,
            "sh" | "bash" | "dash" => return matches!(args, ["-c", script] if restart_alone_at(script, depth + 1)),
            "sudo" => (&["-u", "-g"], &["-i", "-s", "--login", "--shell", "-e", "--edit"], 0),
            "env" => (&["-u"], &["-S", "--split-string", "-C", "--chdir"], 0),
            "nohup" => (&[], &[], 0),
            "nice" => (&["-n"], &[], 0),
            // The duration; chroot's new root.
            "timeout" => (&["-s", "-k"], &[], 1),
            "chroot" => (&[], &[], 1),
            _ => return false,
        };
        let mut i = 0;
        while let Some(a) = args.get(i).copied().filter(|a| a.starts_with('-') && *a != "--") {
            // env's -S at the end of a cluster (`-iS`), and any long option of env's, shortened or not.
            if refused.contains(&a) || (name == "env" && (a.starts_with("--") || a.contains('S'))) {
                return false;
            }
            i += if valued.contains(&a) { 2 } else { 1 };
        }
        i += usize::from(args.get(i) == Some(&"--")) + operands;
        if name == "env" {
            i += args.iter().skip(i).take_while(|a| a.contains('=')).count();
        }
        let Some(next) = args.get(i..) else { return false };
        rest = next.to_vec();
    }
}

/// `sleep 30` (alone) in Demo: how long.
fn demo_sleep(command: &str) -> Option<f64> {
    let rest = command.strip_prefix("sleep ")?;
    rest.trim().trim_end_matches('s').parse::<f64>().ok().filter(|s| *s >= 0.0)
}
