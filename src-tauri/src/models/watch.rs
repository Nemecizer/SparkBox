//! The endpoint round, every few seconds: find every server on the cluster, merge
//! in SparkBox's own loads and the ones that vanished in the last day, probe them on
//! their head node (Live) or run their simulators (Demo), decide each one's health
//! from that evidence (endpoints::assess), keep its metrics, let the supervisor
//! restart multi-node loads that went down, and register the healthy ones as
//! providers so Chat and the Server can use them (and reconcile the saved ones with
//! what's still there). A probe that finds the node's SSH channels busy says
//! nothing about the model: however long the busy spell lasts, its health is judged as at
//! the spell's start, with a note, until a probe gets through. In Demo it also keeps the
//! simulated cluster in step with Demo loads (a load that's ready shows up as a
//! container with its GPU memory and port; an unloaded one disappears), drives the
//! simulated GPUs with what each engine is doing, and plays the serving scenarios.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use serde_json::Value;

use super::endpoints::{self, Canary, CanaryReply, CanaryRequest, Endpoint, Engine, Health, ManagedBy, Probe, Source, Track};
use super::metrics::{self, Scrape};
use super::sim::{self, SimModel, VllmSim};
use super::supervisor::{self, Action, Supervisor};
use super::{demo, fit, now_ms, plan, Ctx, Deployment, DeploymentState, Models};
use crate::demo::{DemoContainerSpec, EngineLoad};
use crate::live::{audit, ssh};
use crate::llm::{DemoSpeed, Provider, ProviderKind};
use crate::model::{ClusterSnapshot, LinkStatus, NodeSnapshot};

/// How often each endpoint is asked for its model list (two missed answers and it's Down).
const PROBE_EVERY_MS: u64 = 15_000;
/// How often an Ollama with nothing in memory and no traffic since it was last asked is asked:
/// every probe is a line in its ollama.service journal (P1.47). An ask that finds the channels
/// busy or gets no answer puts it back on PROBE_EVERY_MS; endpoints::IDLE_READY_WITHIN_MS keeps
/// it Idle until that retry.
const IDLE_PROBE_EVERY_MS: u64 = 60_000;
/// vLLM log lines kept per demo load.
const LOG_LINES: usize = 14;
/// What a dying Ray head logs when it loses a pipeline worker.
pub const RAY_DIED: &str = "RayActorError: The actor died unexpectedly before finishing this task.";

/// The simulated model behind a Demo endpoint, for benchmarks (no background load).
pub(crate) fn sim_model_for(e: &Endpoint) -> SimModel {
    SimModel { max_seqs: 64.0, ..sim_model(e, 0.0) }
}

/// The model a simulated server runs, sized by the same fit math as the Models window.
fn sim_model(e: &Endpoint, busyness: f64) -> SimModel {
    let path = e.model_path.clone().unwrap_or_default();
    let info = demo::model_info(&path);
    let cfg = &info["config"];
    let text = if cfg["text_config"].is_object() { cfg["text_config"].clone() } else { cfg.clone() };
    let arch = fit::arch_facts(&text);
    let weights = info["bytes"].as_u64().filter(|b| *b > 0).unwrap_or(30_000_000_000) as f64;
    let p = e.parallel.unwrap_or(endpoints::Parallel { tp: 1, pp: 1 });
    // Discovery reads untrusted flags. Bound the simulator's fit-table allocation by
    // observed membership, even when a huge rank count happens not to overflow.
    let n = p.ranks().unwrap_or(1).min(e.nodes.len().max(1)) as u64;
    let rows = fit::fit_table(weights as u64, &arch, fit::DEFAULT_NODE_MEMORY, n);
    let kv_tokens = rows.last().and_then(|r| r.kv_capacity_tokens).unwrap_or(200_000) as f64;
    SimModel {
        served: e.models.first().cloned().unwrap_or_default(),
        weights,
        tp: p.tp.clamp(1, n),
        pp: p.pp.clamp(1, n),
        kv_tokens,
        max_seqs: 16.0,
        busyness,
    }
}

/// `curl` on the head node: body, then the HTTP status on its own line.
fn curl(host: &str, port: u16, path: &str, secs: u32) -> String {
    format!("curl -s -m {secs} -w '\\n%{{http_code}}' http://{host}:{port}{path} 2>/dev/null || printf '\\n000'")
}

fn split_status(out: &str) -> (String, u16) {
    let t = out.trim_end();
    match t.rsplit_once('\n') {
        Some((body, code)) => (body.to_string(), code.trim().parse().unwrap_or(0)),
        None => (String::new(), t.trim().parse().unwrap_or(0)),
    }
}

/// curl's word for a failure, from its exit code (the probe runs `curl … || printf 000`).
fn curl_error(code: u16) -> String {
    match code {
        0 => "connection refused".into(),
        c => format!("HTTP {c}"),
    }
}

/// A container name that's safe to put in a command on the node.
fn safe_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

/// A probe that couldn't run. Its reason reaches "Not answering yet (…)", so a refusal from the
/// exit shows its words, not its JSON.
fn unasked(err: &str, now: u64) -> Probe {
    Probe { ok: false, error: Some(crate::errors::SbError::words(err)), at_ms: now, ..Default::default() }
}

/// The last line of an engine log worth quoting: an error, else the last line.
pub fn last_error_line(logs: &str) -> Option<String> {
    let lines: Vec<&str> = logs.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let pick = lines.iter().rev().find(|l| l.contains("Error") || l.contains("error:") || l.contains("Killed")).or(lines.last())?;
    let line = pick.rsplit_once("] ").map(|(_, rest)| rest).unwrap_or(pick);
    Some(line.chars().take(160).collect())
}

/// What in a log counts as new progress: for a load of SparkBox's own its last line, whatever it
/// says, and for a server it didn't start its last compile or CUDA graph line. Digits are kept:
/// "12/50" and "13/50" are the point. (A progress bar rewrites its line with \r: only the last of
/// it counts.)
fn progress_line(logs: &str, own: bool) -> Option<String> {
    let mut lines = logs.lines().rev().filter_map(|l| l.rsplit('\r').map(str::trim).find(|l| !l.is_empty()));
    let line = if own { lines.next() } else { lines.find(|l| matches!(plan::stage_from_logs(l, false), plan::Stage::Compiling { .. } | plan::Stage::Warming)) };
    line.map(String::from)
}

/// What one round learned about an endpoint from its node (Live) or the simulator (Demo).
#[derive(Default)]
struct Look {
    probe: Option<Probe>,
    scrape: Option<Scrape>,
    /// Loading progress from the log, and any stage change.
    progress: Option<plan::Stage>,
    /// The log line that counts as progress this round (`progress_line`).
    progress_line: Option<String>,
    last_log: Option<String>,
    /// A test token, when one was due.
    canary: Option<Canary>,
    /// /v1/completions isn't served: test tokens go to chat from now on.
    chat_only: bool,
    /// Ollama has nothing in memory worth testing (Some), or it does (Some(false)).
    idle: Option<bool>,
    /// It wants an API key SparkBox doesn't have.
    untestable: bool,
    /// A test token was due and look-only held it.
    canary_held: bool,
    /// Its probe was due and found the head node's SSH channels busy.
    busy: bool,
}

/// What a test token for `e` asks for: its served model (Ollama: the model in memory with the
/// most time left), or None when there's nothing to test.
fn canary_request(e: &Endpoint, t: &Track, probe: Option<&Probe>, now: u64) -> Option<CanaryRequest> {
    if e.engine == Engine::Ollama {
        let loaded = probe.filter(|p| p.ok).map(|p| p.ollama.clone()).unwrap_or_else(|| e.ollama.clone());
        let (model, left) = ollama_target(&loaded, now / 1000)?;
        return Some(CanaryRequest { model, chat: false, keep_alive_s: left });
    }
    let model = probe.and_then(|p| p.models.first().cloned()).or_else(|| e.models.first().cloned())?;
    Some(CanaryRequest { model, chat: t.chat_only, keep_alive_s: None })
}

/// The simulator behind a Demo endpoint, made on first sight. The cluster's shared Llama has
/// regular users (Open WebUI); a fresh load of yours is quieter.
fn ensure_sim(st: &mut super::EndpointState, e: &Endpoint) {
    match e.engine {
        Engine::Vllm | Engine::Sglang | Engine::Trtllm | Engine::Nim if !st.vllm_sims.contains_key(&e.id) => {
            let busy = if e.managed_by == ManagedBy::External { 2.0 } else { 0.6 };
            let seed = e.id.bytes().map(u64::from).sum::<u64>();
            st.vllm_sims.insert(e.id.clone(), VllmSim::new(sim_model(e, busy), seed));
        }
        Engine::Ollama => {
            st.ollama_sims.entry(e.id.clone()).or_default();
        }
        _ => {}
    }
}

/// What a simulated engine is doing, whatever SparkBox concluded about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SimState {
    /// Up and generating.
    Serving,
    /// Up, with requests in flight that produce nothing (the endpoint-stall scenario: NCCL waits
    /// on a peer that never answers).
    Hung,
    /// Not running, still loading, a stage gone (DemoCluster stops the pipeline's traffic then),
    /// or its GPU is gone (CUDA calls fail, the engine stops).
    Off,
}

fn sim_state(e: &Endpoint, snap: &ClusterSnapshot, stall: bool) -> SimState {
    let head_ok = e.members.iter().find(|m| m.head).is_none_or(|h| h.running());
    let stages_up = e.nodes.iter().all(|n| reporting(snap, n).is_some());
    if !e.listening || !head_ok || e.gone || !stages_up || gpu_fault_on(e, snap).is_some() {
        return SimState::Off;
    }
    if stall && e.engine != Engine::Ollama {
        return SimState::Hung;
    }
    SimState::Serving
}

/// A member's GPU fault, as the health engine stamped it on the snapshot.
fn gpu_fault_on<'a>(e: &Endpoint, snap: &'a ClusterSnapshot) -> Option<&'a crate::model::GpuFault> {
    e.nodes.iter().filter_map(|n| snap.nodes.iter().find(|x| &x.id == n)).find_map(|n| n.gpu_fault.as_ref())
}

/// Demo: where an external pipeline is in its load (demo::load_seconds) after its last stage
/// started, or None once it's loaded.
fn demo_phase(e: &Endpoint, now_s: u64) -> Option<plan::Stage> {
    if e.deployment.is_some() || e.engine == Engine::Ollama || e.members.is_empty() || !e.members.iter().all(|m| m.running()) {
        return None;
    }
    let since = e.members.iter().map(|m| m.started_at).max()?;
    let weights = demo::model_info(e.model_path.as_deref().unwrap_or_default())["bytes"].as_u64().filter(|b| *b > 0).unwrap_or(30_000_000_000) as f64;
    let n = e.parallel.and_then(endpoints::Parallel::ranks).unwrap_or(1) as u64;
    let (load_s, warm_s) = demo::load_seconds(weights, n);
    let t = now_s.saturating_sub(since) as f64;
    if t < load_s {
        Some(plan::Stage::Loading { percent: ((t / load_s) * 100.0).min(99.0) as u64 })
    } else if t < load_s + warm_s {
        Some(plan::Stage::Warming)
    } else {
        None
    }
}

/// One round of Live probes in the activity log, by the round's clock (`now`): the probes run
/// under it, and test tokens, which run beside the round, join it by this id.
fn check_models(now: u64) -> (String, audit::OpMeta) {
    (format!("poll-models-{now}"), audit::OpMeta { title: "Check models".into(), source: "poll".into() })
}

/// The test token in Demo: what the simulated engine answers, the same evidence Live reads.
/// None before its simulator has run a round (nothing to answer with yet).
fn demo_canary(e: &Endpoint, snap: &ClusterSnapshot, st: &super::EndpointState, stage_silent: bool, now: u64) -> Option<Canary> {
    let fail = |error: String| Some(Canary { at_ms: now, ok: false, latency_ms: None, error: Some(error), after_round: None });
    if let Some(f) = gpu_fault_on(e, snap) {
        // CUDA calls fail on a GPU that's gone: the engine answers 500 with the driver's words.
        let words = if f.code == Some(79) { "GPU has fallen off the bus".to_string() } else { f.text.clone() };
        return fail(format!("HTTP 500: {words}"));
    }
    // A stage that stopped answering, or a hung engine: the request waits until curl gives up.
    let stall = snap.scenario.as_deref() == Some("endpoint-stall") && e.managed_by == ManagedBy::External;
    if stage_silent || sim_state(e, snap, stall) == SimState::Hung {
        return fail(format!("no reply in {} s", endpoints::CANARY_TIMEOUT_S));
    }
    let jitter = ((now / 1000) % 17) as f64 / 17.0;
    let ms = match e.engine {
        Engine::Ollama => st.ollama_sims.get(&e.id).map(|o| o.canary_ms(sim::OLLAMA_MODELS[0].0, jitter)),
        _ => st.vllm_sims.get(&e.id).map(|s| s.canary_ms() * (0.95 + 0.1 * jitter)),
    }?;
    Some(Canary { at_ms: now, ok: true, latency_ms: Some(ms.round() as u64), error: None, after_round: None })
}

/// vLLM's 10 s averages from an endpoint's recent history (the last `secs` of it); the prefix
/// hit rate over what the engine has served, when the last scrape saw no new queries.
fn engine_stats(m: &metrics::EndpointMetrics, secs: u64, prefix_hit_pct: Option<f64>) -> sim::EngineStats {
    let h = &m.history;
    let from = h.ts.last().copied().unwrap_or(0).saturating_sub(secs.saturating_sub(1));
    let idx: Vec<usize> = (0..h.ts.len()).filter(|&i| h.ts[i] >= from).collect();
    let avg = |v: &[f64], fallback: f64| if idx.is_empty() { fallback } else { idx.iter().map(|&i| v[i]).sum::<f64>() / idx.len() as f64 };
    sim::EngineStats {
        prompt_tps: avg(&h.prompt_tps, m.prompt_tps),
        gen_tps: avg(&h.gen_tps, m.gen_tps),
        running: m.running,
        waiting: m.waiting,
        kv_pct: m.kv_pct,
        prefix_hit_pct: m.prefix_hit_pct.or(prefix_hit_pct).unwrap_or(0.0),
    }
}

/// Demo engines' stats lines by container ("cell1/vllm-head"), the last day of them, for the
/// simulated `docker logs` (demo_engine_log).
static ENGINE_LOG: std::sync::Mutex<Option<EngineLog>> = std::sync::Mutex::new(None);

pub(super) type EngineLog = HashMap<String, VecDeque<(u64, String)>>;

fn push_engine_log(all: &mut EngineLog, container: &str, at: u64, line: String) {
    let lines = all.entry(container.to_string()).or_default();
    lines.push_back((at, line));
    while lines.front().is_some_and(|(t, _)| at.saturating_sub(*t) > 24 * 3600) {
        lines.pop_front();
    }
}

fn between(all: Option<&EngineLog>, node: &str, container: &str, from_s: u64, to_s: u64) -> Vec<(u64, String)> {
    all.and_then(|m| m.get(&format!("{node}/{container}"))).map(|l| l.iter().filter(|(t, _)| *t >= from_s && *t <= to_s).cloned().collect()).unwrap_or_default()
}

/// The engine-stats lines a Demo container logged between two Unix times, oldest first: the
/// same numbers its /metrics (and so its endpoint card) showed at those moments. For the
/// simulated terminal's `docker logs` of a serving container, which has no Models at hand.
pub fn demo_engine_log(node: &str, container: &str, from_s: u64, to_s: u64) -> Vec<(u64, String)> {
    between(ENGINE_LOG.lock().unwrap().as_ref(), node, container, from_s, to_s)
}

/// A stats line as a Demo engine writes it (tests of the simulated terminal).
#[cfg(test)]
pub fn demo_engine_log_push(node: &str, container: &str, at: u64, line: &str) {
    push_engine_log(ENGINE_LOG.lock().unwrap().get_or_insert_with(HashMap::new), &format!("{node}/{container}"), at, line.into());
}

/// The node an endpoint member runs on, when it's online and reporting.
fn reporting<'a>(snap: &'a ClusterSnapshot, node: &str) -> Option<&'a NodeSnapshot> {
    snap.nodes.iter().find(|n| n.id == node && endpoints::reporting(n))
}

/// What a service Ollama in the Demo keeps on disk, as the real ones did (Oct 29): nothing on
/// cell1, gemma4 on cell3.
pub(crate) fn demo_service_tags(node: &str) -> Vec<(String, u64)> {
    match node {
        "cell3" => vec![("gemma4:latest".to_string(), 9_556_302_029)],
        _ => Vec::new(),
    }
}

/// The model an Ollama test token goes to: the one in memory with the most time left, if it has
/// at least OLLAMA_MIN_LEFT_S; and how long that is (the test's keep_alive, so it isn't kept longer).
fn ollama_target(loaded: &[metrics::OllamaLoaded], now_s: u64) -> Option<(String, Option<u64>)> {
    let left = |m: &metrics::OllamaLoaded| m.expires_at.map(|x| x.saturating_sub(now_s));
    loaded
        .iter()
        .filter(|m| !m.name.is_empty() && left(m).is_none_or(|l| l >= endpoints::OLLAMA_MIN_LEFT_S))
        .max_by_key(|m| left(m).unwrap_or(u64::MAX))
        .map(|m| (m.name.clone(), left(m)))
}

impl Models {
    /// The latest list, without probing (for `endpoints_list`).
    pub fn endpoints(&self) -> Vec<Endpoint> {
        self.eps.lock().unwrap().list.clone()
    }

    /// How many rounds have run (verify waits for one that started after its operation).
    pub fn round(&self) -> u64 {
        self.eps.lock().unwrap().round
    }

    /// Endpoints whose test tokens go to /v1/chat/completions (for the agents' probes).
    pub fn chat_only(&self) -> BTreeMap<String, bool> {
        self.eps.lock().unwrap().tracks.iter().filter(|(_, t)| t.chat_only).map(|(id, _)| (id.clone(), true)).collect()
    }

    /// How long requests may run with no tokens before it's Stalled (the endpoint-stalled rule's for_s).
    pub fn set_stall_after(&self, secs: u64) {
        self.eps.lock().unwrap().stall_after_s = secs;
    }

    /// This Models' Demo engine-stats lines (demo_engine_log, without the other instances tests run).
    #[cfg(test)]
    pub(crate) fn engine_log(&self, node: &str, container: &str) -> Vec<(u64, String)> {
        between(Some(&self.eps.lock().unwrap().engine_log), node, container, 0, u64::MAX)
    }

    /// Test tokens that came back since the last call: (endpoint, unix ms, latency ms), for history.
    pub fn take_new_canaries(&self) -> Vec<(String, u64, u64)> {
        let mut st = self.eps.lock().unwrap();
        let fresh: Vec<(String, u64, u64)> = st
            .tracks
            .iter()
            .filter_map(|(id, t)| t.canary.as_ref().filter(|c| c.ok).and_then(|c| c.latency_ms.map(|ms| (id.clone(), c.at_ms, ms))))
            .filter(|(id, at, _)| st.canaries_recorded.get(id) != Some(at))
            .collect();
        for (id, at, _) in &fresh {
            st.canaries_recorded.insert(id.clone(), *at);
        }
        fresh
    }

    /// Drops an endpoint SparkBox remembers but that isn't running (an exited head, or one that
    /// vanished). It comes back if it starts again.
    pub fn forget_endpoint(&self, id: &str) -> Result<String, String> {
        let mut st = self.eps.lock().unwrap();
        let e = st.list.iter().find(|e| e.id == id).cloned().ok_or("That endpoint isn't listed any more.")?;
        if e.managed_by == ManagedBy::Sparkbox {
            return Err("SparkBox loaded this one: unload it from Models instead.".into());
        }
        if !e.forgettable {
            return Err("It's still there: only a server that exited or vanished can be forgotten.".into());
        }
        let since = e.members.iter().find(|m| m.head).map(|m| m.finished_at).unwrap_or(0);
        st.forgotten.insert(id.to_string(), since);
        st.seen.remove(id);
        st.list.retain(|x| x.id != id);
        Ok(endpoints::label_of(&e))
    }

    /// A restart starts the crash-loop count over: the restarts before it were the problem it fixes.
    pub(crate) fn clear_restarts(&self, endpoint: &str) {
        let mut st = self.eps.lock().unwrap();
        let keys: Vec<String> = st.list.iter().filter(|e| e.id == endpoint).flat_map(|e| e.members.iter().map(|m| format!("{}/{}", m.node, m.id))).collect();
        for k in keys {
            st.restart_log.remove(&k);
        }
        if let Some(t) = st.tracks.get_mut(endpoint) {
            t.restarts.clear();
        }
    }

    /// The supervisor's state for a load, after a restart by hand (which starts it over).
    pub(crate) fn reset_supervisor(&self, deployment: &str) {
        if let Some(s) = self.eps.lock().unwrap().supervisors.get_mut(deployment) {
            s.reset();
        }
    }

    /// One round. `force` probes model lists now rather than on their schedule.
    pub async fn endpoints_tick(self: &Arc<Self>, ctx: &Ctx, force: bool) -> Vec<Endpoint> {
        let now = now_ms();
        {
            let mut st = self.eps.lock().unwrap();
            if st.was_live != Some(ctx.live) {
                // Switching data source: nothing simulated carries over into Live, or back.
                let round = st.round;
                *st = Default::default();
                st.was_live = Some(ctx.live);
                st.round = round;
            }
        }
        let snap = ctx.snapshot.clone().unwrap_or_default();
        if ctx.live {
            // Downloads and copies SparkBox quit while following get their tasks again (once), and
            // copies' file servers their watcher.
            self.pick_up_jobs(ctx);
            self.pick_up_shares(ctx);
        } else {
            self.demo.lock().unwrap().set_scenario(snap.scenario.clone());
        }
        let deployments = self.deployments(ctx);
        if !ctx.live {
            self.sync_demo_cluster(ctx, &deployments);
        }
        let found = endpoints::discover(&snap, &ctx.inventory);
        let manual: Vec<Provider> = self.llm.all().await.into_iter().filter(|p| !self.llm.is_discovered(&p.id)).collect();
        let mut list = endpoints::merge(found, &deployments, &manual, &ctx.inventory);
        self.recall(&mut list, &snap, now);
        self.count_restarts(&list, now);

        // Live, a round is one poll in the activity log ("Check models"). Its probes aren't logged
        // (Live::probe); what is, a test token with a key, a refused probe, is that one row.
        let looks = if ctx.live {
            let (op, poll) = check_models(now);
            audit::with_op_id(op, poll, audit::as_probe(self.look_live(&list, &snap, now, force))).await
        } else {
            self.look_demo(&mut list, &snap, now)
        };
        self.probe_manual(&list, now, force).await;
        {
            let mut st = self.eps.lock().unwrap();
            let stall_after = st.stall_after_s;
            for e in list.iter_mut() {
                let t = st.tracks.entry(e.id.clone()).or_default();
                t.stall_after_s = stall_after;
                if let Some(look) = looks.get(&e.id) {
                    if look.busy {
                        endpoints::record_busy(t, now);
                    }
                    if let Some(p) = &look.probe {
                        endpoints::record_probe(t, p);
                        if let (Some(code), Some(path)) = (p.health, e.engine.health_path()) {
                            endpoints::record_health(t, code, path, now);
                        }
                    }
                    // Live reads /metrics before deciding; Demo's simulator records its own after.
                    if let Some(sc) = &look.scrape {
                        endpoints::record_scrape(t, sc.generation_tokens, sc.running, sc.waiting, e.engine.health_path().is_some(), now);
                    }
                    if let Some(c) = &look.canary {
                        endpoints::record_canary(t, c.clone());
                    }
                    t.chat_only |= look.chat_only;
                    if look.untestable {
                        t.untestable = true;
                    }
                    if look.canary_held {
                        t.canary_held = true;
                    }
                    if let Some(idle) = look.idle {
                        t.idle = idle;
                        // At rest there's nothing to test: no test a busy spell held back is owed.
                        if idle {
                            t.untested_since_ms = None;
                        }
                    }
                    // A starting load's stage is its own (below): two sources would move the clock every round.
                    if let Some(stage) = look.progress.as_ref().filter(|_| !matches!(&e.deployment, Some((DeploymentState::Starting, Some(_), ..)))) {
                        note_progress(t, stage, look.progress_line.as_deref(), now);
                    }
                    if look.last_log.is_some() {
                        t.last_log = look.last_log.clone();
                    }
                }
                if let Some((DeploymentState::Starting, Some(stage), ..)) = &e.deployment {
                    note_progress(t, stage, looks.get(&e.id).and_then(|l| l.progress_line.as_deref()), now);
                    // Look-only holding its readiness check is waiting: the stuck clock starts once it ends.
                    if e.readiness.as_ref().is_some_and(|r| r.why.as_deref() == Some(super::readiness::HELD)) {
                        t.progress_ms = Some(now);
                    }
                }
                if let Some(look) = looks.get(&e.id).and_then(|l| l.probe.as_ref()) {
                    st.probes.insert(e.id.clone(), look.clone());
                }
            }
            for e in list.iter_mut() {
                if let Some(p) = st.probes.get(&e.id) {
                    endpoints::apply_probe(e, p);
                }
                let keep = e.deployment_id.as_ref().filter(|_| e.managed_by == ManagedBy::Sparkbox).and_then(|d| {
                    let multi = e.nodes.len() > 1;
                    deployments.iter().find(|x| &x.spec.id == d).filter(|x| x.spec.keep_running && multi).map(|x| x.spec.id.clone())
                });
                if let Some(d) = keep {
                    e.supervisor = Some(st.supervisors.entry(d).or_default().view());
                }
                // A remembered one its probe still finds (it answers, or its unit is active) is found.
                if e.recalled && looks.get(&e.id).and_then(|l| l.probe.as_ref()).is_some_and(endpoints::probe_finds) {
                    e.recalled = false;
                    let t = st.tracks.entry(e.id.clone()).or_default();
                    t.missed = 0;
                    t.missed_since_ms = None;
                }
                let t = st.tracks.get(&e.id).cloned().unwrap_or_default();
                endpoints::assess(e, &t, &snap, now);
                if let Some(since) = st.planned.get(&e.id).copied() {
                    endpoints::pause_for_restart(e, since);
                }
                let t = st.tracks.entry(e.id.clone()).or_default();
                if !e.gone && !e.recalled {
                    t.seen_ms = now;
                }
                let want = e.parallel.and_then(endpoints::Parallel::ranks).unwrap_or(1);
                if e.missing_nodes.is_empty() && e.nodes.len() == want && !matches!(e.health, Health::Down | Health::Unknown | Health::Paused) {
                    t.members = e.nodes.clone();
                }
                if e.health == Health::Ready {
                    t.pct = None;
                    t.stage = None;
                    t.progress_ms = None;
                    t.progress_line.clear();
                } else {
                    // Proof is about the run that gave it: once it isn't Ready, it has to generate again.
                    t.canary_ok_ms = None;
                    t.passive_ok_ms = None;
                    t.canary_held = false;
                    // Test tokens that failed because a stage was gone don't count against it once it's back.
                    let tokens = e.health_reason.as_deref().is_some_and(|r| r.starts_with("Didn't generate") || r.starts_with("Answers, but no token") || r.starts_with("No token for"));
                    if !tokens {
                        t.canary_fails = 0;
                    }
                }
                // A busy spell holds what it found answering; one that starts while it's down ages as usual.
                if !matches!(e.health, Health::Ready | Health::Idle) {
                    t.busy_since_ms = None;
                    t.untested_since_ms = None;
                }
                if e.source == Source::Discovered && e.managed_by == ManagedBy::External && !e.gone {
                    st.seen.insert(e.id.clone(), e.clone());
                }
            }
        }

        if ctx.live {
            self.metrics_live(&mut list, &looks, now);
        } else {
            self.simulate(&mut list, &snap, now);
        }
        self.supervise(ctx, &mut list, &snap, now);

        // Healthy servers become providers for Chat and the Server; Demo loads answer as demo models.
        let mut providers: Vec<Provider> = list.iter().filter_map(|e| endpoints::provider_for(e, !ctx.live)).collect();
        if !ctx.live {
            for d in deployments.iter().filter(|d| d.state == DeploymentState::Ready && d.spec.listen != "fabric") {
                let healthy = list.iter().any(|e| e.deployment_id.as_deref() == Some(d.spec.id.as_str()) && e.health == Health::Ready);
                if !healthy {
                    continue;
                }
                providers.push(Provider {
                    id: format!("vllm-{}", d.spec.id),
                    name: format!("{} on {}", if d.spec.engine == "sglang" { "SGLang" } else { "vLLM" }, d.spec.nodes.join("+")),
                    kind: ProviderKind::Demo,
                    base_url: d.endpoint.clone(),
                    models: vec![d.spec.served_name.clone()],
                    default_model: Some(d.spec.served_name.clone()),
                    ..Default::default()
                });
            }
        }
        for e in list.iter_mut() {
            if e.provider_id.is_none() {
                e.provider_id = providers.iter().find(|p| p.id == e.id).map(|p| p.id.clone());
            }
            if e.managed_by == ManagedBy::Sparkbox && !ctx.live {
                e.provider_id = e.deployment_id.as_ref().map(|d| format!("vllm-{d}")).filter(|id| providers.iter().any(|p| &p.id == id));
            }
        }
        self.llm.set_discovered(providers);
        self.reconcile_providers(ctx, &list, &deployments).await;
        let mut st = self.eps.lock().unwrap();
        // A load that just became Ready is verified: its endpoint-down alert (from an earlier run)
        // closes now, with the reason, instead of waiting for the next evaluation.
        let loaded: Vec<(String, String)> = list
            .iter()
            .filter(|e| e.health == Health::Ready && e.deployment.is_some())
            .filter(|e| st.list.iter().any(|old| old.id == e.id && old.health != Health::Ready))
            .map(|e| {
                let started = e.deployment.as_ref().map(|d| d.3).unwrap_or(0);
                let took = super::verify::duration((now / 1000).saturating_sub(started));
                (e.id.clone(), format!("Verified: {} answered after {took}", e.label))
            })
            .collect();
        st.list = list.clone();
        st.round += 1;
        drop(st);
        if let Some(h) = self.health.lock().unwrap().clone() {
            for (id, reason) in loaded {
                h.resolve_now("endpoint-down", &id, &reason);
            }
        }
        list
    }

    /// Keeps Chat's cluster providers in step with the endpoints. One that stopped answering
    /// keeps its grace (llm::GRACE_MS) only while its endpoint is still listed. With Live data,
    /// a load's saved `vllm-<id>` whose load is unloaded (left by a crash part-way through) is
    /// taken out, and a scan's saved entry that the cluster now finds by itself becomes a
    /// discovered one. A `vllm-…` with no load on record is left alone: one added by hand in
    /// AI › Providers ("vLLM box") gets such an id too. Look-only changes neither: providers.json
    /// and the activity log stay as they are until the first round after it ends.
    async fn reconcile_providers(&self, ctx: &Ctx, list: &[Endpoint], deployments: &[Deployment]) {
        self.llm.set_nodes(ctx.inventory.nodes.iter().map(|n| (n.id.clone(), [Some(n.host.clone()), n.ip.clone()].into_iter().flatten().collect())).collect());
        let listed: HashSet<String> = list
            .iter()
            .filter(|e| !e.gone && !matches!(e.health, Health::Gone | Health::Stopped))
            .flat_map(|e| [Some(e.id.clone()), e.provider_id.clone(), e.deployment_id.as_ref().map(|d| format!("vllm-{d}"))])
            .flatten()
            .collect();
        self.llm.forget_quiet(|id| listed.contains(id));
        if !ctx.live {
            return; // providers.json holds Live's registrations; Demo leaves them alone
        }
        if self.hold_view().look_only {
            return;
        }
        for id in self.llm.saved_ids().await {
            let stale = id.strip_prefix("vllm-").is_some_and(|d| deployments.iter().any(|x| x.spec.id == d && x.state == DeploymentState::Stopped));
            if stale && self.llm.unsave(&id).await.unwrap_or(false) {
                self.live.audit.record_app("Chat provider removed", &format!("{id}: its load isn't on the cluster any more"));
            }
            let found = list.iter().any(|e| e.id == id && e.source == Source::Discovered);
            if found && self.llm.adopt_scanned(&id).await.unwrap_or(false) {
                self.live.audit.record_app("Chat provider now found on the cluster", &format!("{id}: offered while it answers, with the settings it had"));
            }
        }
    }

    /// Drops forgotten endpoints, and lists external ones that vanished in the last day. One that
    /// no source finds (process, listener, unit) stays listed as last seen, and probed, until it
    /// has been missing for GONE_AFTER_ROUNDS rounds and GONE_AFTER_MS; then it's Gone.
    fn recall(&self, list: &mut Vec<Endpoint>, snap: &ClusterSnapshot, now: u64) {
        let mut st = self.eps.lock().unwrap();
        let forgotten = st.forgotten.clone();
        list.retain(|e| match forgotten.get(&e.id) {
            // Forgotten until it starts again (or exits again, later).
            Some(since) => e.members.iter().find(|m| m.head).is_some_and(|m| m.running() || m.finished_at > *since),
            None => true,
        });
        st.forgotten.retain(|id, _| !list.iter().any(|e| &e.id == id && e.health != Health::Down));
        for e in list.iter() {
            let t = st.tracks.entry(e.id.clone()).or_default();
            t.missed = 0;
            t.missed_since_ms = None;
        }
        let keep = endpoints::KEEP_GONE_S * 1000;
        let tracks = st.tracks.clone();
        st.seen.retain(|id, e| e.nodes.iter().any(|id| !snap.nodes.iter().any(|n| &n.id == id && endpoints::reporting(n)))
            || tracks.get(id).is_some_and(|t| now.saturating_sub(t.seen_ms) < keep));
        let missing: Vec<Endpoint> = st.seen.iter().filter(|(id, _)| !list.iter().any(|x| &x.id == *id) && !forgotten.contains_key(*id)).map(|(_, e)| e.clone()).collect();
        for mut e in missing {
            let t = st.tracks.entry(e.id.clone()).or_default();
            t.missed += 1;
            let since = *t.missed_since_ms.get_or_insert(now);
            let online = snap.nodes.iter().any(|n| n.id == e.head_node && endpoints::reporting(n));
            if online && (t.missed < endpoints::GONE_AFTER_ROUNDS || now.saturating_sub(since) < endpoints::GONE_AFTER_MS) {
                // Missed by this sample only, so far: as last seen.
                e.recalled = true;
                e.gone = false;
                list.push(e);
                continue;
            }
            // Gone from the snapshot. Its node may be offline (assess says so), or it was removed.
            e.recalled = false;
            e.gone = online;
            e.members.clear();
            e.listening = false;
            e.container_ids.clear();
            list.push(e);
        }
    }

    /// Restart counts per member node over the last 10 minutes, from Docker's restart counter.
    fn count_restarts(&self, list: &[Endpoint], now: u64) {
        let mut st = self.eps.lock().unwrap();
        let now_s = now / 1000;
        for e in list {
            let mut per_node: BTreeMap<String, (u32, u64)> = BTreeMap::new();
            for m in &e.members {
                let log = st.restart_log.entry(format!("{}/{}", m.node, m.id)).or_default();
                if log.back().is_none_or(|(_, c)| *c != m.restart_count) {
                    log.push_back((now_s, m.restart_count));
                }
                while log.len() > 1 && log.front().is_some_and(|(t, _)| now_s.saturating_sub(*t) > endpoints::CRASH_WINDOW_S) {
                    log.pop_front();
                }
                let first = log.front().map(|(_, c)| *c).unwrap_or(0);
                let last_change = log.back().map(|(t, _)| *t).unwrap_or(0);
                let n = m.restart_count.saturating_sub(first);
                if n > 0 {
                    per_node.insert(m.node.clone(), (n, last_change));
                }
            }
            st.tracks.entry(e.id.clone()).or_default().restarts = per_node;
        }
    }

    // ---- Live: probes over SSH on each head node

    /// Whether `e` is asked for its model list this round: every PROBE_EVERY_MS, or every
    /// IDLE_PROBE_EVERY_MS for an Ollama whose last answer had nothing in memory, with no request
    /// from SparkBox since. vLLM and the other engines keep the 15 s.
    fn probe_due(st: &super::EndpointState, e: &Endpoint, now: u64, force: bool) -> bool {
        let asked = st.asked_ms.get(&e.id).copied().unwrap_or(0);
        let idle = e.engine == Engine::Ollama && st.empty_ms.get(&e.id).is_some_and(|at| st.traffic_ms.get(&e.id).is_none_or(|t| t <= at));
        force || now.saturating_sub(asked) >= if idle { IDLE_PROBE_EVERY_MS } else { PROBE_EVERY_MS }
    }

    async fn look_live(self: &Arc<Self>, list: &[Endpoint], snap: &ClusterSnapshot, now: u64, force: bool) -> HashMap<String, Look> {
        let due: HashSet<String> = {
            let st = self.eps.lock().unwrap();
            list.iter().filter(|e| Self::probe_due(&st, e, now, force)).map(|e| e.id.clone()).collect()
        };
        // Look-only holds test tokens (the exit would refuse them): none is sent, none fails.
        let held = self.hold_view().look_only;
        let tracks: HashMap<String, Track> = self.eps.lock().unwrap().tracks.clone();
        let due = &due;
        let canaries: std::sync::Mutex<Vec<(Endpoint, CanaryRequest)>> = Default::default();
        let canaries = &canaries;
        let jobs = list.iter().filter(|e| e.source != Source::Manual && !e.head_node.is_empty() && !e.gone
            && e.nodes.iter().chain(tracks.get(&e.id).into_iter().flat_map(|t| &t.members))
                .all(|id| snap.nodes.iter().any(|n| &n.id == id && endpoints::reporting(n)))).map(|e| {
            let e = e.clone();
            let t = tracks.get(&e.id).cloned().unwrap_or_default();
            let logged_at = t.last_log_at;
            async move {
                let mut look = Look::default();
                let head = e.members.iter().find(|m| m.head).cloned();
                let running = head.as_ref().is_none_or(|h| h.running());
                let mut asked = false;
                if running && e.listening {
                    if due.contains(&e.id) {
                        look.probe = self.probe_models(&e, now).await;
                        look.busy = look.probe.is_none();
                        asked = true;
                    }
                    let mut t = t.clone();
                    if let Some(p) = &look.probe {
                        endpoints::record_probe(&mut t, p);
                        if let (Some(code), Some(path)) = (p.health, e.engine.health_path()) {
                            endpoints::record_health(&mut t, code, path, now);
                        }
                    }
                    if e.engine.has_metrics() {
                        look.scrape = self
                            .poll(&e.head_node, &curl(&e.probe_host, e.port, "/metrics", 2), Duration::from_secs(8))
                            .await
                            .ok()
                            .map(|out| split_status(&out))
                            .filter(|(_, code)| *code == 200)
                            .map(|(body, _)| Scrape::from_text(&body, now))
                            .filter(|s| s.is_engine());
                    }
                    // A test token, when it answers and one is due (live traffic counts instead).
                    let answering = look.probe.as_ref().map(|p| p.ok).unwrap_or_else(|| t.last_ok_ms.is_some_and(|ok| now.saturating_sub(ok) <= endpoints::READY_WITHIN_MS));
                    if let Some(sc) = &look.scrape {
                        endpoints::record_scrape(&mut t, sc.generation_tokens, sc.running, sc.waiting, e.engine.health_path().is_some(), now);
                    }
                    let req = canary_request(&e, &t, look.probe.as_ref(), now);
                    if e.engine == Engine::Ollama && look.probe.as_ref().is_some_and(|p| p.ok) {
                        look.idle = Some(req.is_none());
                    }
                    if let Some(req) = req.filter(|_| answering && endpoints::canary_due(&t, now)) {
                        if held {
                            look.canary_held = true;
                        } else {
                            canaries.lock().unwrap().push((e.clone(), req));
                        }
                    }
                }
                // Loading: how far the weights are, from the engine's log.
                if let Some(h) = head.as_ref().filter(|h| h.running() && !e.listening && safe_name(&h.name)) {
                    let cmd = format!("docker logs --tail 40 {} 2>&1 | tail -n 40", h.name);
                    if let Ok(logs) = self.poll(&e.head_node, &cmd, Duration::from_secs(10)).await {
                        look.progress = Some(plan::stage_from_logs(&logs, false));
                        look.progress_line = progress_line(&logs, e.managed_by == ManagedBy::Sparkbox);
                    }
                }
                // Exited with an error: the line that says why, once per exit.
                if let Some(h) = head.as_ref().filter(|h| !h.running() && h.exit_code != 0 && h.finished_at != logged_at && safe_name(&h.name)) {
                    let cmd = format!("docker logs --tail 30 {} 2>&1 | tail -n 30", h.name);
                    if let Ok(logs) = self.poll(&e.head_node, &cmd, Duration::from_secs(10)).await {
                        look.last_log = last_error_line(&logs);
                    }
                }
                (e.id.clone(), look, head.map(|h| h.finished_at), asked, e.engine == Engine::Ollama)
            }
        });
        let results = join_all(jobs).await;
        for (e, req) in std::mem::take(&mut *canaries.lock().unwrap()) {
            self.spawn_canary(e, req, now);
        }
        let mut st = self.eps.lock().unwrap();
        let mut out = HashMap::new();
        for (id, look, finished, asked, ollama) in results {
            if look.last_log.is_some() {
                st.tracks.entry(id.clone()).or_default().last_log_at = finished.unwrap_or(0);
            }
            if asked {
                st.asked_ms.insert(id.clone(), now);
            }
            match &look.probe {
                Some(p) if ollama && p.ok && p.ollama.is_empty() => {
                    st.empty_ms.insert(id.clone(), now);
                }
                // Something in memory, no answer, or no SSH channel free to ask: asked on the
                // usual schedule, so a missed ask is tried again within 15 s.
                _ if asked => {
                    st.empty_ms.remove(&id);
                }
                _ => {}
            }
            out.insert(id, look);
        }
        out
    }

    /// `/v1/models` and the engine's /health (for Ollama, `/api/ps` for what's in memory) on the head node.
    /// An Ollama that isn't a container also says whether its systemd unit is active (one more
    /// source that it's there, when the process sample leaves it out). None when the node's SSH
    /// channels were all busy: back-pressure, no evidence about the model either way.
    async fn probe_models(&self, e: &Endpoint, now: u64) -> Option<Probe> {
        let path = e.engine.models_path();
        let mut cmd = curl(&e.probe_host, e.port, path, 3);
        let unit = e.engine == Engine::Ollama && e.members.is_empty();
        if e.engine == Engine::Ollama {
            cmd = format!("{cmd}; printf '\\n--sparkbox--\\n'; {}", curl(&e.probe_host, e.port, "/api/ps", 3));
            if unit {
                cmd = format!("{cmd}; printf '\\n--sparkbox--\\n'; systemctl show -p ActiveState --value ollama.service 2>/dev/null");
            }
        } else if let Some(health) = e.engine.health_path() {
            cmd = format!("{cmd}; printf '\\n--sparkbox--\\n'; {}", curl(&e.probe_host, e.port, health, 3));
        }
        let out = match self.poll(&e.head_node, &cmd, Duration::from_secs(10)).await {
            Ok(o) => o,
            Err(err) if ssh::is_busy(&err) => return None,
            Err(err) => return Some(unasked(&err, now)),
        };
        let mut parts = out.split("\n--sparkbox--\n").map(String::from);
        let first = parts.next().unwrap_or_default();
        let ps = parts.next();
        let unit_active = parts.next().filter(|_| unit).map(|s| s.trim() == "active");
        let (body, code) = split_status(&first);
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let health = e.engine.health_path().and(ps.as_deref()).map(|h| split_status(h).1);
        let ps = ps.filter(|_| e.engine == Engine::Ollama);
        let probe = |p: Probe| Some(Probe { health, unit_active, ..p });
        probe(match code {
            200 => {
                let (models, max_context) =
                    if e.engine == Engine::Ollama { (metrics::parse_ollama_tags(&v), None) } else { endpoints::parse_models(&v) };
                let ollama = ps
                    .map(|p| split_status(&p).0)
                    .and_then(|b| serde_json::from_str::<Value>(&b).ok())
                    .map(|v| metrics::parse_ollama_ps(&v))
                    .unwrap_or_default();
                let on_disk = if e.engine == Engine::Ollama { metrics::parse_ollama_sizes(&v) } else { Vec::new() };
                Probe { ok: true, models, max_context, at_ms: now, ollama, on_disk, ..Default::default() }
            }
            // It answered, it just wants a key SparkBox doesn't have: up, with the models its command line names.
            401 | 403 => Probe { ok: true, at_ms: now, error: None, ..Default::default() },
            c => Probe { ok: false, error: Some(curl_error(c)), at_ms: now, ..Default::default() },
        })
    }

    /// One test token on the head node (endpoints::canary_command), beside the rounds: a hung
    /// engine takes the whole 30 s to time out. The key, when SparkBox holds one, goes in on
    /// stdin as a header line, and its row is the round's (`round`: its clock); without one the
    /// request runs as a plain probe.
    fn spawn_canary(self: &Arc<Self>, e: Endpoint, req: CanaryRequest, round: u64) {
        self.eps.lock().unwrap().tracks.entry(e.id.clone()).or_default().canary_running = true;
        let me = self.clone();
        let (op, poll) = check_models(round);
        tauri::async_runtime::spawn(audit::with_op_id(op, poll, async move {
            let key = crate::secrets::get(&endpoints::key_account(&e)).ok().flatten().filter(|k| !k.is_empty());
            let limit = Duration::from_secs(u64::from(endpoints::CANARY_TIMEOUT_S) + 10);
            let mut req = req;
            let (mut chat_only, mut untestable, mut held, mut busy) = (false, false, false, false);
            let mut canary = None;
            for _ in 0..2 {
                let cmd = endpoints::canary_command(e.engine, &e.probe_host, e.port, &req, key.is_some());
                let sent = Some(me.eps.lock().unwrap().round);
                // A test token is inference on the node: the exit holds it in look-only, and its
                // refusal reads as its sentence, not its JSON.
                let out = match &key {
                    // stdin needs a full exec; as_probe keeps it out of the activity log's default view.
                    Some(k) => crate::live::exit::as_use(audit::as_probe(me.live.run(&e.head_node, &cmd, Some(format!("Authorization: Bearer {k}\n").as_bytes()), limit))).await.map(|o| o.stdout),
                    None => crate::live::exit::as_use(me.poll(&e.head_node, &cmd, limit)).await,
                }
                .map_err(|e| crate::errors::SbError::classify(&e));
                let at = now_ms();
                match out.map(|o| endpoints::parse_canary(e.engine, &o, at)) {
                    Ok(CanaryReply::Done(c)) => canary = Some(Canary { after_round: sent, ..c }),
                    // Look-only came on since the round asked: held, not failed.
                    Err(err) if err.kind == crate::errors::ErrorKind::LookOnly => held = true,
                    // No SSH channel came free: nothing was asked of the model, so nothing counts.
                    Err(err) if ssh::is_busy(&err.message) => busy = true,
                    Err(err) => canary = Some(Canary { at_ms: at, ok: false, latency_ms: None, error: Some(err.message), after_round: sent }),
                    Ok(CanaryReply::ChatOnly) if !req.chat => {
                        chat_only = true;
                        req.chat = true;
                        continue;
                    }
                    Ok(CanaryReply::ChatOnly) => canary = Some(Canary { at_ms: at, ok: false, latency_ms: None, error: Some("it serves neither completions nor chat".into()), after_round: sent }),
                    Ok(CanaryReply::NeedsKey) => untestable = true,
                }
                break;
            }
            let mut st = me.eps.lock().unwrap();
            let t = st.tracks.entry(e.id.clone()).or_default();
            t.canary_running = false;
            t.canary_tried_ms = Some(now_ms());
            t.chat_only |= chat_only;
            if let Some(c) = canary {
                endpoints::record_canary(t, c);
            }
            if untestable {
                t.untestable = true;
            }
            if held {
                t.canary_held = true;
            }
            if busy {
                endpoints::record_canary_busy(t, now_ms());
            }
        }));
    }

    /// Hand-added endpoints are asked from this Mac (their model list, with their key).
    async fn probe_manual(&self, list: &[Endpoint], now: u64, force: bool) {
        let due: Vec<String> = {
            let st = self.eps.lock().unwrap();
            list.iter()
                .filter(|e| e.source == Source::Manual)
                .filter(|e| force || st.probes.get(&e.id).is_none_or(|p| now.saturating_sub(p.at_ms) >= PROBE_EVERY_MS))
                .map(|e| e.id.clone())
                .collect()
        };
        let results = join_all(due.iter().map(|id| self.llm.refresh_models(id))).await;
        let mut st = self.eps.lock().unwrap();
        for (id, r) in due.into_iter().zip(results) {
            let p = match r {
                Ok(models) => Probe { ok: true, models, at_ms: now, ..Default::default() },
                Err(e) => Probe { ok: false, error: Some(e), at_ms: now, ..Default::default() },
            };
            endpoints::record_probe(st.tracks.entry(id.clone()).or_default(), &p);
            st.probes.insert(id, p);
        }
    }

    fn metrics_live(&self, list: &mut [Endpoint], looks: &HashMap<String, Look>, now: u64) {
        let mut st = self.eps.lock().unwrap();
        for e in list.iter_mut() {
            let traffic = self.llm.take_traffic(e.provider_id.as_deref().unwrap_or(&e.id));
            if traffic.active > 0 || traffic.finished > 0 || traffic.output_tokens > 0 {
                st.traffic_ms.insert(e.id.clone(), now);
            }
            let scrape = match looks.get(&e.id).and_then(|l| l.scrape.clone()) {
                Some(s) if e.health == Health::Ready => {
                    e.metrics_source = Some("server".into());
                    Some(s)
                }
                // No /metrics (Ollama, llama.cpp without --metrics): count what SparkBox itself sends.
                None if e.health == Health::Ready => {
                    e.metrics_source = Some("sparkbox".into());
                    Some(Scrape { ts_ms: now, ..st.observed.entry(e.id.clone()).or_default().step(&traffic) })
                }
                _ => None,
            };
            e.metrics = match scrape {
                Some(s) => Some(st.rings.entry(e.id.clone()).or_default().push(s).clone()),
                None if e.health == Health::Unknown => None,
                None => st.rings.get(&e.id).and_then(|r| r.metrics().cloned()),
            };
            e.corrupted_requests = e.metrics.as_ref().and_then(|m| m.corrupted_requests);
        }
    }

    // ---- Demo: simulated probes, servers and GPUs

    /// What the simulated cluster shows: a server answers when its head runs and listens; one
    /// that is starting up shows how far its weights are. An external pipeline whose stages just
    /// started reads its share of the weights at 1.6 GB/s a node, then captures CUDA graphs
    /// (demo::load_seconds), and listens only after that. Test tokens go to the simulators.
    fn look_demo(&self, list: &mut [Endpoint], snap: &ClusterSnapshot, now: u64) -> HashMap<String, Look> {
        let mut out = HashMap::new();
        let now_s = now / 1000;
        // What the simulated Ollama container has pulled (its /api/tags), copied before the
        // endpoints lock is taken: a card never reads "In memory: gpt-oss:20b" over "No models on disk".
        let pulled: Vec<(String, u64)> = self.demo.lock().unwrap().ollama_models().into_iter().map(|m| (m.name, m.size_bytes)).collect();
        let mut st = self.eps.lock().unwrap();
        // One the simulated cluster no longer lists (recalled) has no simulated server behind it:
        // nothing answers for it, so it's Gone once every source has missed it long enough.
        for e in list.iter_mut().filter(|e| e.source != Source::Manual && !e.gone && !e.recalled) {
            ensure_sim(&mut st, e);
            let mut look = Look::default();
            let head = e.members.iter().find(|m| m.head).cloned();
            let head_ok = head.as_ref().is_none_or(|h| h.running());
            let online = snap.nodes.iter().any(|n| n.id == e.head_node && endpoints::reporting(n));
            if head_ok && online && e.deployment.is_none() && e.engine != Engine::Ollama {
                if let Some(phase) = demo_phase(e, now_s) {
                    e.listening = false;
                    look.progress = Some(phase);
                }
            }
            let t = st.tracks.get(&e.id).cloned().unwrap_or_default();
            // Its stages as last seen complete: a stage whose node went silent isn't in the snapshot.
            let stages: Vec<String> = e.nodes.iter().chain(&t.members).fold(Vec::new(), |mut v, n| {
                if !v.contains(n) {
                    v.push(n.clone());
                }
                v
            });
            let hung = stages.iter().any(|n| reporting(snap, n).is_none());
            // Match Live: unavailable ranks cannot supply a new successful check.
            if hung { continue; }
            if e.listening && head_ok && online {
                let broken = gpu_fault_on(e, snap).is_some();
                let health = e.engine.health_path().map(|_| if broken { 503 } else { 200 });
                // An Ollama run as a systemd service, as found on cell1 and cell3: its tags on
                // disk and nothing in memory, so it's idle and never tested.
                if e.engine == Engine::Ollama && e.members.is_empty() {
                    let tags = demo_service_tags(&e.head_node);
                    let names: Vec<String> = tags.iter().map(|(n, _)| n.clone()).collect();
                    look.probe = Some(Probe { ok: true, models: names, at_ms: now, on_disk: tags, unit_active: Some(true), ..Default::default() });
                    look.idle = Some(true);
                    out.insert(e.id.clone(), look);
                    continue;
                }
                let on_disk = if e.engine == Engine::Ollama { pulled.clone() } else { Vec::new() };
                look.probe = Some(Probe { ok: true, models: e.models.clone(), at_ms: now, health, on_disk, ..Default::default() });
                if endpoints::canary_due(&t, now) {
                    look.canary = demo_canary(e, snap, &st, hung, now).map(|c| Canary { after_round: Some(st.round), ..c });
                }
                if e.engine == Engine::Ollama {
                    look.idle = Some(false);
                }
            } else if head_ok && online && hung && endpoints::canary_due(&t, now) {
                // A stage stopped answering: requests to the head hang until curl gives up.
                look.canary = demo_canary(e, snap, &st, hung, now).map(|c| Canary { after_round: Some(st.round), ..c });
            }
            // A pipeline head that exited 1 lost a stage: what the simulated vLLM logs then.
            if head.is_some_and(|h| !h.running() && h.exit_code == 1) && e.parallel.is_some_and(|p| p.pp > 1) {
                look.last_log = Some(RAY_DIED.into());
            }
            out.insert(e.id.clone(), look);
        }
        out
    }

    fn simulate(&self, list: &mut [Endpoint], snap: &ClusterSnapshot, now: u64) {
        let slow = snap.scenario.as_deref() == Some("endpoint-slow");
        let stall = snap.scenario.as_deref() == Some("endpoint-stall");
        let mut speeds: HashMap<String, DemoSpeed> = HashMap::new();
        let mut logs: Vec<(String, String)> = Vec::new();
        let mut loads: Vec<(String, EngineLoad)> = Vec::new();
        let mut st = self.eps.lock().unwrap();
        // Simulated time moves at least a round per round, so rates never divide by a few milliseconds.
        let dt = if st.last_tick_ms == 0 { 3.0 } else { (now.saturating_sub(st.last_tick_ms) as f64 / 1000.0).clamp(3.0, 15.0) };
        st.last_tick_ms = now;
        st.sim_clock_ms = if st.sim_clock_ms == 0 { now } else { st.sim_clock_ms + (dt * 1000.0) as u64 };
        let clock = st.sim_clock_ms;
        let mut rng = fastrand::Rng::with_seed(clock);
        for e in list.iter_mut().filter(|e| !e.recalled) {
            if e.health != Health::Unknown { e.probed_ms = Some(now); }
            let ready = e.health == Health::Ready;
            // What the engine itself is doing, whatever SparkBox concluded from it.
            let state = sim_state(e, snap, stall && e.managed_by == ManagedBy::External);
            let provider = if e.managed_by == ManagedBy::Sparkbox {
                e.deployment_id.as_ref().map(|d| format!("vllm-{d}")).unwrap_or_else(|| e.id.clone())
            } else {
                e.id.clone()
            };
            let traffic = self.llm.take_traffic(&provider);
            let (scrape, util) = match e.engine {
                Engine::Vllm | Engine::Sglang | Engine::Trtllm | Engine::Nim => {
                    ensure_sim(&mut st, e);
                    let Some(sim) = st.vllm_sims.get_mut(&e.id) else { continue };
                    sim.slow = slow && e.managed_by == ManagedBy::External;
                    match state {
                        SimState::Serving => sim.step(dt, &traffic),
                        SimState::Hung => sim.stall(dt),
                        SimState::Off => sim.idle(),
                    }
                    if ready {
                        speeds.insert(provider.clone(), sim.speed());
                    }
                    e.metrics_source = Some("server".into());
                    if e.max_context.is_none() {
                        e.max_context = Some(32_768);
                    }
                    // Decode keeps every stage busy in step: about 70% with three requests, 90% with a full batch.
                    // A hung pipeline spins in NCCL: busy GPUs, no tokens.
                    let util = match (state, sim.running) {
                        (SimState::Hung, _) => 0.97 + (rng.f64() - 0.5) * 0.02,
                        (_, r) if r <= 0.0 => 0.02,
                        (_, r) => (0.45 + 0.08 * r).min(0.92) + (rng.f64() - 0.5) * 0.06,
                    };
                    (Scrape::from_text(&sim.prometheus(), clock), Some(util))
                }
                // A service Ollama, as found on cell1 and cell3: its own node's tags on disk, nothing
                // in memory and no tokens (the everyday world's models aren't its). Its node's idle
                // GPU readings are the world's own, so it sets no engine load.
                Engine::Ollama if e.members.is_empty() => {
                    e.models = demo_service_tags(&e.head_node).into_iter().map(|(n, _)| n).collect();
                    e.ollama.clear();
                    e.metrics_source = Some("sparkbox".into());
                    let o = st.ollama_sims.entry(e.id.clone()).or_default();
                    (Scrape { ts_ms: clock, ..o.idle() }, None)
                }
                Engine::Ollama => {
                    let o = st.ollama_sims.entry(e.id.clone()).or_default();
                    // Its chat sessions are the node's own (demo::trace), so the card and the GPU agree.
                    o.busy = e.nodes.first().map(|n| crate::demo::trace::util_at(n, now) > 0.0);
                    let s = if state == SimState::Serving { Scrape { ts_ms: clock, ..o.step(dt, &traffic) } } else { Scrape { ts_ms: clock, ..o.idle() } };
                    if ready {
                        for (name, _, _) in sim::OLLAMA_MODELS {
                            speeds.insert(format!("{provider}/{name}"), o.speed(name));
                        }
                        speeds.insert(provider.clone(), o.speed(sim::OLLAMA_MODELS[0].0));
                    }
                    // Ollama's GPU follows its tokens per second.
                    let util = if o.tps > 0.0 { (o.tps / sim::OLLAMA_MODELS[0].2 * 0.8).clamp(0.05, 0.9) } else { 0.02 };
                    if state == SimState::Serving {
                        e.models = self.demo.lock().unwrap().ollama_models().into_iter().map(|m| m.name).collect();
                        let expires = now / 1000 + 240;
                        e.ollama = sim::OLLAMA_MODELS
                            .iter()
                            .map(|(name, bytes, _)| metrics::OllamaLoaded {
                                name: name.to_string(),
                                size_bytes: *bytes,
                                vram_bytes: *bytes,
                                expires_at: Some(expires),
                                context_length: Some(8192),
                            })
                            .collect();
                    }
                    e.metrics_source = Some("sparkbox".into());
                    (s, Some(util))
                }
                _ => continue,
            };
            // The token counter and the requests in flight are the evidence for the next verdict.
            let has_health = e.engine.health_path().is_some();
            if e.health != Health::Unknown {
                endpoints::record_scrape(st.tracks.entry(e.id.clone()).or_default(), scrape.generation_tokens, scrape.running, scrape.waiting, has_health, now);
            }
            let m = st.rings.entry(e.id.clone()).or_default().push(scrape).clone();
            // vLLM's engine-stats line, every 10 s, from the same numbers /metrics gave.
            if state != SimState::Off && e.engine != Engine::Ollama {
                let bucket = clock / 10_000;
                if st.logged_bucket.get(&e.id) != Some(&bucket) {
                    st.logged_bucket.insert(e.id.clone(), bucket);
                    let hit = st.vllm_sims.get(&e.id).map(|s| s.prefix_hit_pct());
                    let line = sim::stats_line(clock / 1000, &engine_stats(&m, 10, hit));
                    let key = e.members.iter().find(|x| x.head).map(|h| format!("{}/{}", h.node, h.name)).unwrap_or_else(|| e.id.clone());
                    push_engine_log(&mut st.engine_log, &key, clock / 1000, line.clone());
                    push_engine_log(ENGINE_LOG.lock().unwrap().get_or_insert_with(HashMap::new), &key, clock / 1000, line.clone());
                    if let Some(d) = e.deployment_id.clone() {
                        logs.push((d, line));
                    }
                }
            }
            e.metrics = (e.health != Health::Unknown).then_some(m);
            if e.health == Health::Unknown { e.metrics_source = None; }
            // The GPUs behind it: busy while it serves (or spins), working while it loads, idle (about
            // 15 W) when nothing runs, off when it's down.
            let loading = match &e.deployment {
                Some((DeploymentState::Starting, stage, ..)) => stage.clone(),
                Some(_) => None,
                None => demo_phase(e, now / 1000),
            };
            if let Some(util) = util {
                for node in e.nodes.iter().filter(|n| !e.missing_nodes.contains(n)) {
                    let load = match (&loading, state) {
                        (Some(plan::Stage::Warming), _) => EngineLoad { util_pct: 55.0, power_w: Some(56.0), active: true, steady: false },
                        (Some(plan::Stage::Loading { .. }), _) => EngineLoad { util_pct: 3.0, power_w: Some(19.0), active: true, steady: false },
                        (_, SimState::Serving | SimState::Hung) if ready || state == SimState::Hung => EngineLoad {
                            util_pct: (util * 100.0) as f32,
                            power_w: Some((15.0 + 75.0 * util) as f32),
                            active: true,
                            // Generating as usual (the idle 2% isn't): the node's trace takes over.
                            steady: state == SimState::Serving && util > 0.04,
                        },
                        _ => EngineLoad { util_pct: 0.0, power_w: None, active: false, steady: false },
                    };
                    loads.push((node.clone(), load));
                }
            }
        }
        drop(st);
        // Loads still starting stream at nothing yet; everything else as simulated.
        self.llm.set_demo_speeds(speeds);
        if let Some(cluster) = &self.cluster {
            let mut c = cluster.lock().unwrap();
            for (node, load) in loads {
                c.set_engine_load(&node, load);
            }
        }
        if !logs.is_empty() {
            let mut d = self.demo.lock().unwrap();
            for (id, line) in logs {
                d.append_log(&id, &line, LOG_LINES);
            }
        }
    }

    // ---- The supervisor: Keep running for multi-node loads

    fn supervise(self: &Arc<Self>, ctx: &Ctx, list: &mut [Endpoint], snap: &ClusterSnapshot, now: u64) {
        let online = |n: &str| snap.nodes.iter().any(|x| x.id == n && endpoints::reporting(x));
        let mut due: Vec<(String, usize)> = Vec::new();
        // Look-only and relocks hold it (supervisor.rs); with Live data it restarts only its own loads.
        let hold = self.hold_view();
        {
            let mut st = self.eps.lock().unwrap();
            for e in list.iter_mut().filter(|e| e.supervisor.is_some()) {
                let Some(id) = e.deployment_id.clone() else { continue };
                let s = st.supervisors.entry(id.clone()).or_default();
                let action = s.round(e, snap, ctx.live, crate::identity::instance_name(), online, hold, now);
                if let Action::Restart(n) = action {
                    due.push((id, n));
                }
                e.supervisor = Some(s.view());
                if s.gave_up {
                    // The give-up is the reason now, not the last failure.
                    let t = Track::default();
                    endpoints::assess(e, &t, snap, now);
                }
            }
            let known: Vec<String> = list.iter().filter_map(|e| e.deployment_id.clone()).collect();
            st.supervisors.retain(|id, _| known.contains(id));
        }
        for (id, n) in due {
            let me = self.clone();
            let ctx = Ctx { live: ctx.live, settings: ctx.settings.clone(), inventory: ctx.inventory.clone(), snapshot: ctx.snapshot.clone() };
            tauri::async_runtime::spawn(async move {
                let verb = format!("Keep running: restart|({n} of {})", supervisor::GIVE_UP_AFTER);
                if let Err(e) = me.restart_ordered(&ctx, &id, &verb, true).await {
                    eprintln!("supervisor: couldn't restart {id}: {e}");
                }
            });
        }
    }

    /// Demo loads appear in (and leave) the simulated cluster, and copies load its fabric.
    fn sync_demo_cluster(&self, ctx: &Ctx, deployments: &[Deployment]) {
        let Some(cluster) = &self.cluster else { return };
        let snap = ctx.snapshot.clone().unwrap_or_default();
        let present = |id: &str| snap.nodes.iter().any(|n| n.containers.iter().any(|c| c.labels.get("sparkbox.deployment").map(String::as_str) == Some(id)));
        let mut injected = self.eps.lock().unwrap().injected.clone();
        // Loads whose containers this snapshot already shows (added in an earlier round).
        let shown: Vec<String> = injected.iter().filter(|id| present(id)).cloned().collect();
        let mut lost: Vec<(String, String)> = Vec::new();
        let mut c = cluster.lock().unwrap();
        for d in deployments {
            let id = &d.spec.id;
            let ready = d.state == DeploymentState::Ready;
            if ready && (!injected.contains(id) || (!present(id) && ctx.snapshot.is_some())) {
                let plan_nodes: Vec<&crate::model::NodeSnapshot> = snap.nodes.iter().filter(|n| d.spec.nodes.contains(&n.id)).collect();
                let needs = demo_needs(&d.spec.model_path);
                let per_node = plan::memory_plan(&d.spec, &needs, &plan_nodes, &[]);
                let _ = c.remove_containers("sparkbox.deployment", id);
                for (i, node) in d.spec.nodes.iter().enumerate() {
                    let role = if d.spec.nodes.len() == 1 { "single" } else if i == 0 { "head" } else { "worker" };
                    let total = per_node.get(i).map(|p| p.total).unwrap_or(fit::DEFAULT_NODE_MEMORY);
                    // vLLM takes its whole share up front: weights, then KV cache in what's left.
                    let gpu_mem = (total as f64 * d.spec.gpu_memory_utilization) as u64;
                    let mut labels = BTreeMap::new();
                    labels.insert("sparkbox.deployment".to_string(), id.clone());
                    labels.insert("sparkbox.role".to_string(), role.to_string());
                    labels.insert("sparkbox.rank".to_string(), i.to_string());
                    labels.insert("sparkbox.model".to_string(), d.spec.served_name.clone());
                    let head_ip = crate::demo::world::fabric_ip(ctx.inventory.nodes.iter().position(|n| n.id == d.spec.nodes[0]).unwrap_or(0), 0);
                    let command = if d.spec.engine == "sglang" {
                        plan::sglang_args(&d.spec, i, &head_ip).join(" ")
                    } else if role == "worker" {
                        format!("ray start --address={head_ip}:{} --block", plan::RAY_PORT)
                    } else {
                        format!("vllm {}", plan::vllm_args(&d.spec).join(" "))
                    };
                    let env = vec![format!("NCCL_SOCKET_IFNAME={}", crate::demo::world::NCCL_IFACE)];
                    let spec = DemoContainerSpec {
                        name: plan::container_name(id, node, role),
                        image: if d.spec.engine == "sglang" { d.spec.image.clone() } else { plan::ray_image(&d.spec.image) },
                        ports: if role == "worker" { vec![] } else { vec![d.spec.port] },
                        gpu_mem,
                        rss: super::plan::VLLM_HOST_MEMORY,
                        labels,
                        command,
                        env,
                        restart_policy: Some(if d.spec.keep_running && role == "single" { "unless-stopped" } else { "no" }.into()),
                    };
                    if let Err(e) = c.add_container(node, spec) {
                        eprintln!("demo: couldn't add {id} on {node}: {e}");
                    }
                }
                if !injected.contains(id) {
                    injected.push(id.clone());
                }
            }
            // A ready multi-node load whose worker container stopped: the head dies with it, as Ray's does.
            if ready && shown.contains(id) && d.spec.nodes.len() > 1 && d.spec.engine != "sglang" {
                for (i, node) in d.spec.nodes.iter().enumerate().skip(1) {
                    let name = plan::container_name(id, node, "worker");
                    let n = snap.nodes.iter().find(|n| &n.id == node);
                    let running = n.is_none_or(|n| n.status != LinkStatus::Online || n.containers.iter().any(|c| c.name == name && c.state == "running"));
                    if !running {
                        lost.push((id.clone(), format!("Head exited (code 1): pipeline stage {} of {} on {node} stopped · {RAY_DIED}", i + 1, d.spec.nodes.len())));
                        break;
                    }
                }
            }
            // Unloaded, failed, or restarting (it shows up again once it's ready).
            if (!d.state.active() || d.state == DeploymentState::Starting || lost.iter().any(|(x, _)| x == id)) && injected.contains(id) {
                c.remove_containers("sparkbox.deployment", id);
                injected.retain(|x| x != id);
            }
        }
        // Forgotten loads.
        injected.retain(|id| {
            let known = deployments.iter().any(|d| &d.spec.id == id);
            if !known {
                c.remove_containers("sparkbox.deployment", id);
            }
            known
        });
        // Copies over the fabric: each running pull loads its source's send and its target's receive.
        let mut load: HashMap<String, (f64, f64)> = ctx.inventory.nodes.iter().map(|n| (n.id.clone(), (0.0, 0.0))).collect();
        for (source, target, bytes_per_s) in self.demo.lock().unwrap().fabric_flows() {
            if let Some(t) = load.get_mut(&target) {
                t.0 += bytes_per_s * 8.0;
            }
            if let Some(s) = load.get_mut(&source) {
                s.1 += bytes_per_s * 8.0;
            }
        }
        for (node, (rx, tx)) in load {
            let _ = c.set_external_fabric_load(&node, rx, tx);
        }
        drop(c);
        if !lost.is_empty() {
            let mut d = self.demo.lock().unwrap();
            for (id, why) in lost {
                d.fail(&id, &why);
            }
        }
        self.eps.lock().unwrap().injected = injected;
    }
}

/// Loading progress moves the stuck clock: a new percentage, a new stage, or a new `line` from its
/// log (`progress_line`). No line this round says nothing new, so the last one stands.
fn note_progress(t: &mut Track, stage: &plan::Stage, line: Option<&str>, now: u64) {
    if let Some(l) = line {
        t.progress_line = l.to_string();
    }
    let key = format!("{stage:?}|{}", t.progress_line);
    if key != t.progress_key || t.progress_ms.is_none() {
        t.progress_ms = Some(now);
        t.progress_key = key;
    }
    t.stage = Some(stage.clone());
    t.pct = match stage {
        plan::Stage::Loading { percent } => Some(*percent),
        _ => None,
    };
}

/// The demo library's models, for the injected containers' memory.
fn demo_needs(path: &str) -> plan::ModelNeeds {
    let info = demo::model_info(path);
    let cfg = &info["config"];
    let text = if cfg["text_config"].is_object() { cfg["text_config"].clone() } else { cfg.clone() };
    let arch = fit::arch_facts(&text);
    plan::ModelNeeds { weight_bytes: info["bytes"].as_u64().unwrap_or(0), kv_bytes_per_token: fit::kv_bytes_per_token(&arch), arch }
}

#[cfg(test)]
impl Models {
    /// Tests: as if `ms` had passed since every probe, scrape and test token.
    pub(crate) fn age(&self, ms: u64) {
        let mut st = self.eps.lock().unwrap();
        let eps = &mut *st;
        for at in eps.asked_ms.values_mut().chain(eps.empty_ms.values_mut()).chain(eps.traffic_ms.values_mut()) {
            *at = at.saturating_sub(ms);
        }
        for t in st.tracks.values_mut() {
            for v in [&mut t.last_ok_ms, &mut t.health_ok_ms, &mut t.canary_ok_ms, &mut t.passive_ok_ms, &mut t.flat_since_ms, &mut t.scrape_ms, &mut t.gen_rose_ms, &mut t.missed_since_ms, &mut t.busy_since_ms, &mut t.untested_since_ms] {
                if let Some(x) = v {
                    *x = x.saturating_sub(ms);
                }
            }
            if let Some(c) = &mut t.canary {
                c.at_ms = c.at_ms.saturating_sub(ms);
            }
        }
    }
}

/// Restart history kept per container, for crash-loop detection.
pub(super) type RestartLog = HashMap<String, VecDeque<(u64, u32)>>;

/// Supervisors by deployment id.
pub(super) type Supervisors = HashMap<String, Supervisor>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_rank_counts_cannot_overflow_or_allocate_an_unbounded_demo_fit_table() {
        for pp in [1, 2] {
            let e = Endpoint { nodes: vec!["cell1".into()], parallel: Some(endpoints::Parallel { tp: u64::MAX, pp }), ..Default::default() };
            let model = sim_model(&e, 0.0);
            assert_eq!((model.tp, model.pp), (1, 1));
            assert!(model.prefill_tps().is_finite());
        }
    }

    #[test]
    fn curl_output_splits_into_body_and_status() {
        assert_eq!(split_status("{\"data\":[]}\n200"), ("{\"data\":[]}".to_string(), 200));
        assert_eq!(split_status("\n000"), (String::new(), 0));
        assert_eq!(split_status("Unauthorized\n401\n"), ("Unauthorized".to_string(), 401));
        assert!(curl("127.0.0.1", 8000, "/metrics", 2).starts_with("curl -s -m 2 -w '\\n%{http_code}' http://127.0.0.1:8000/metrics"));
        assert_eq!(curl_error(0), "connection refused");
    }

    #[test]
    fn the_line_that_says_why_it_exited() {
        let logs = "INFO 10-28 12:01:02 [loader.py:44] Loading weights took 41.2 seconds\n\
                    ERROR 10-28 12:04:10 [core.py:812] ray.exceptions.RayActorError: The actor died unexpectedly before finishing this task.\n\
                    INFO 10-28 12:04:11 [launcher.py:80] Shutting down FastAPI HTTP server.\n";
        assert_eq!(last_error_line(logs).as_deref(), Some("ray.exceptions.RayActorError: The actor died unexpectedly before finishing this task."));
        assert_eq!(last_error_line("just one line").as_deref(), Some("just one line"));
        let refused = crate::errors::SbError::new(crate::errors::ErrorKind::Fenced, "No fence is open.").to_json();
        assert_eq!(unasked(&refused, 7).error.as_deref(), Some("No fence is open."), "a probe's reason reads as words");
        assert_eq!(unasked(&format!("cell2: {refused}"), 7).error.as_deref(), Some("cell2: No fence is open."));
        assert_eq!(last_error_line(""), None);
        assert!(safe_name("sparkbox-m1a2b-head") && !safe_name("x; rm -rf /") && !safe_name(""));
    }

    #[test]
    fn progress_moves_the_stuck_clock_only_when_it_changes() {
        let mut t = Track::default();
        note_progress(&mut t, &plan::Stage::Loading { percent: 10 }, None, 1_000);
        assert_eq!((t.pct, t.progress_ms), (Some(10), Some(1_000)));
        note_progress(&mut t, &plan::Stage::Loading { percent: 10 }, None, 9_000);
        assert_eq!(t.progress_ms, Some(1_000), "the same percentage isn't progress");
        note_progress(&mut t, &plan::Stage::Loading { percent: 42 }, None, 12_000);
        assert_eq!((t.pct, t.progress_ms), (Some(42), Some(12_000)));
        note_progress(&mut t, &plan::Stage::Warming, None, 20_000);
        assert_eq!(t.progress_ms, Some(20_000), "warming up after loading is progress");
        note_progress(&mut t, &plan::Stage::Warming, None, 30_000);
        assert_eq!(t.progress_ms, Some(20_000));
    }

    /// Anything new in a load's log is progress, digit by digit; the same line isn't. The stage
    /// words come from research.md, not a capture, which is why a stage alone can't be the test.
    #[test]
    fn a_new_log_line_moves_a_loads_clock_and_the_same_one_doesnt() {
        let (quiet, line) = (plan::Stage::Starting, |n: u32| format!("(Worker pid=251) INFO [setup.py:12] building {n}/50"));
        let mut t = Track::default();
        note_progress(&mut t, &quiet, Some(&line(12)), 1_000);
        assert_eq!(t.progress_ms, Some(1_000));
        note_progress(&mut t, &quiet, Some(&line(12)), 9_000);
        assert_eq!(t.progress_ms, Some(1_000), "the same last line isn't progress");
        note_progress(&mut t, &quiet, Some(&line(13)), 12_000);
        assert_eq!(t.progress_ms, Some(12_000), "digits are kept: 12/50 and 13/50 are different lines");
        // A round that couldn't read the log says nothing new.
        note_progress(&mut t, &quiet, None, 20_000);
        assert_eq!(t.progress_ms, Some(12_000));
        note_progress(&mut t, &quiet, Some(&line(13)), 30_000);
        assert_eq!(t.progress_ms, Some(12_000), "and the line it had still stands");
    }

    #[test]
    fn what_counts_as_new_in_a_log() {
        let compile = "INFO 10-30 09:01:14 [flashinfer.py:88] flashinfer: compiling kernel 12";
        let noise = "(EngineCore pid=178) WARNING 10-30 09:01:15 [core.py:44] nothing to report";
        let logs = format!("{compile}\n{noise}\n\n");
        // A load of SparkBox's own: its last line, whatever it says.
        assert_eq!(progress_line(&logs, true).as_deref(), Some(noise));
        // A server it didn't start: only a compile line counts, however old the noise after it.
        assert_eq!(progress_line(&logs, false).as_deref(), Some(compile));
        assert_eq!(progress_line(noise, false), None);
        // A CUDA graph capture counts too.
        let capture = "Capturing CUDA graphs (mixed prefill-decode, compile):  50%";
        assert_eq!(progress_line(&format!("{capture}\n{noise}\n"), false).as_deref(), Some(capture));
        // A progress bar rewrites its line: what it says now is the end of it.
        assert_eq!(progress_line("Loading:  10%\rLoading:  50%\r", true).as_deref(), Some("Loading:  50%"));
        assert_eq!(progress_line("", true), None);
    }

    /// A load in the fake node's log, its stage its own (Joining, from `Models::follow`): only a
    /// new line moves its clock. The log's own stage disagrees with Joining all along, as it does
    /// on a real start, and a clock fed from both would move every round.
    #[tokio::test]
    async fn a_quiet_load_stays_loading_and_only_new_log_lines_move_its_clock() {
        use crate::fake_node::{fake_node, temp};
        use std::sync::Mutex;
        let log = Arc::new(Mutex::new("sparkbox: no stage in this line\n".to_string()));
        let l = log.clone();
        let script: crate::fake_node::Script = Arc::new(move |cmd: &str, _: &[u8]| cmd.contains("docker logs --tail 40 sparkbox-quiet-r0").then(|| (l.lock().unwrap().clone(), 0)));
        let (node, port) = fake_node(&temp("quiet-node"), script).await;
        let dir = temp("quiet-app");
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell1", port)]).await;
        let spec = plan::DeploySpec { id: "quiet".into(), served_name: "gemma-3-27b-it".into(), model_path: "/m/gemma".into(), nodes: vec!["cell1".into()], port: 8000, require_key: false, ..Default::default() };
        let d = Deployment { spec, state: DeploymentState::Starting, stage: Some(plan::Stage::Joining { have: 2, need: 2 }), started_at: now_ms() / 1000 - 40 * 60, ..Default::default() };
        std::fs::write(dir.join("deployments.json"), serde_json::to_string(&[d]).unwrap()).unwrap();
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm, None);
        // Its head in the snapshot, not listening yet: only launch.py on its Command.
        let mut snap = live.snapshot(&inventory);
        let labels = [("sparkbox.deployment", "quiet"), ("sparkbox.role", "head"), ("sparkbox.rank", "0"), ("sparkbox.model", "gemma-3-27b-it"), ("sparkbox.path", "/m/gemma"), ("sparkbox.port", "8000"), ("sparkbox.parallel", "1x1")];
        let head = crate::model::Container { id: "0a0000000001".into(), name: "sparkbox-quiet-r0".into(), state: "running".into(), started_at: now_ms() / 1000 - 40 * 60, command: "python3 /run/sparkbox/launch.py".into(), labels: labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), ..Default::default() };
        snap.nodes[0].containers = vec![head];
        snap.nodes[0].listeners = vec![crate::model::Listener { port: 22, ..Default::default() }];
        let ctx = Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(snap) };
        let id = "cell1-vllm-8000";
        let long_ago = now_ms() - 25 * 60_000;
        let moved = || models.eps.lock().unwrap().tracks.get(id).and_then(|t| t.progress_ms).unwrap();
        let tick = || async {
            // Past Live's 2 s reuse of the last answer: each round reads the log again.
            tokio::time::sleep(Duration::from_millis(2_100)).await;
            models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).expect("listed")
        };
        tick().await;
        let reads = || node.ran("docker logs --tail 40 sparkbox-quiet-r0").len();
        assert!(reads() >= 1, "its head's log was read");
        models.eps.lock().unwrap().tracks.get_mut(id).unwrap().progress_ms = Some(long_ago);

        // Nothing new: quiet, and the card says so.
        for _ in 0..2 {
            let e = tick().await;
            assert_eq!(moved(), long_ago, "no clock moved: {:?}", e.health_reason);
            assert_eq!(e.health, Health::Loading);
            assert_eq!(e.health_reason.as_deref(), Some("2 of 2 nodes up · no new progress for 25 min (limit 90 min)"));
        }
        // A new line, stage unchanged: progress.
        log.lock().unwrap().push_str("still working 1/50\n");
        let e = tick().await;
        assert!(moved() > long_ago, "a new line moved the clock");
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Loading, Some("2 of 2 nodes up")));
        live.stop();
    }

    /// A server SparkBox didn't start: its own stage is the log's, and only compile lines count.
    #[test]
    fn a_new_compile_line_moves_the_clock_of_a_server_sparkbox_didnt_start() {
        let lines = |n: u32| format!("INFO [setup.py:12] flashinfer: compiling kernel {n}\nINFO [x.py:1] heartbeat {n}\n");
        let read = |t: &mut Track, logs: &str, now: u64| note_progress(t, &plan::stage_from_logs(logs, false), progress_line(logs, false).as_deref(), now);
        let mut t = Track::default();
        read(&mut t, &lines(1), 1_000);
        assert_eq!(t.progress_ms, Some(1_000));
        assert!(matches!(t.stage, Some(plan::Stage::Compiling { .. })), "{:?}", t.stage);
        read(&mut t, &lines(2), 9_000);
        assert_eq!(t.progress_ms, Some(9_000), "a new compile line is progress");
        // Noise after it isn't: the same compile line, a new heartbeat.
        read(&mut t, "INFO [setup.py:12] flashinfer: compiling kernel 2\nINFO [x.py:1] heartbeat 3\n", 12_000);
        assert_eq!(t.progress_ms, Some(9_000));
    }

    /// P2.8 counts CUDA graph capture as progress: a bar that moves is a server that's alive.
    #[test]
    fn a_new_graph_capture_line_moves_the_clock_of_a_server_sparkbox_didnt_start() {
        let bar = |pct: u32| format!("Capturing CUDA graphs (mixed prefill-decode, compile):   0%|  | 0/49\rCapturing CUDA graphs (mixed prefill-decode, compile):  {pct}%|# | {}/49\n", pct * 49 / 100);
        let read = |t: &mut Track, logs: &str, now: u64| note_progress(t, &plan::stage_from_logs(logs, false), progress_line(logs, false).as_deref(), now);
        let mut t = Track::default();
        read(&mut t, &bar(10), 1_000);
        assert_eq!((t.progress_ms, t.stage.clone()), (Some(1_000), Some(plan::Stage::Warming)));
        read(&mut t, &bar(50), 9_000);
        assert_eq!(t.progress_ms, Some(9_000), "a new capture line is progress");
        read(&mut t, &bar(50), 12_000);
        assert_eq!(t.progress_ms, Some(9_000), "the same one isn't");
    }

    /// Through the round on fake nodes. The head's log says it's compiling and a worker is dead:
    /// that's Down, and Keep running schedules its restart rather than waiting out a compile that
    /// can't finish. With every rank up the same log is a compile, and nothing is scheduled.
    #[tokio::test]
    async fn a_worker_lost_mid_compile_goes_to_keep_running_at_once() {
        use crate::fake_node::{fake_node, temp};
        let script: crate::fake_node::Script = Arc::new(|cmd: &str, _: &[u8]| cmd.contains("docker logs --tail 40 sparkbox-lost-r0").then(|| ("INFO [flashinfer.py:88] flashinfer: compiling kernel 12\n".to_string(), 0)));
        let (_head, p1) = fake_node(&temp("lost-node-1"), script).await;
        let (_worker, p2) = fake_node(&temp("lost-node-2"), Arc::new(|_: &str, _: &[u8]| None)).await;
        let dir = temp("lost-app");
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell1", p1), ("cell2", p2)]).await;
        let spec = plan::DeploySpec { id: "lost".into(), served_name: "gemma-3-27b-it".into(), model_path: "/m/gemma".into(), nodes: vec!["cell1".into(), "cell2".into()], tensor_parallel: 2, pipeline_parallel: 1, port: 8000, keep_running: true, ..Default::default() };
        let d = Deployment { spec, state: DeploymentState::Ready, stage: Some(plan::Stage::Ready), recoverable: true, started_at: now_ms() / 1000 - 3600, ..Default::default() };
        std::fs::write(dir.join("deployments.json"), serde_json::to_string(&[d]).unwrap()).unwrap();
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm, None);
        // Its own gate: the app-wide look-only flag is other tests' to set.
        models.set_gate(crate::admin::testing::gate().0);

        // Both ranks in the snapshot, labelled as this build labels them, the head not listening.
        let at = |worker: &str, code: i32| {
            let rank = |id: &str, role: &str, n: u32, state: &str, code: i32| {
                let mut labels: BTreeMap<String, String> = [("sparkbox.deployment", "lost"), ("sparkbox.instance", crate::identity::instance_name()), ("sparkbox.role", role), ("sparkbox.model", "gemma-3-27b-it"), ("sparkbox.path", "/m/gemma"), ("sparkbox.parallel", "2x1")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                labels.insert("sparkbox.rank".into(), n.to_string());
                if n == 0 {
                    labels.insert("sparkbox.port".into(), "8000".into());
                }
                crate::model::Container { id: id.into(), name: format!("sparkbox-lost-r{n}"), state: state.into(), exit_code: code, started_at: now_ms() / 1000 - 600, finished_at: if state == "exited" { now_ms() / 1000 - 32 } else { 0 }, command: "python3 /run/sparkbox/launch.py".into(), labels, ..Default::default() }
            };
            let mut snap = live.snapshot(&inventory);
            snap.nodes[0].containers = vec![rank("0a0000000001", "head", 0, "running", 0)];
            snap.nodes[0].listeners = vec![crate::model::Listener { port: 22, ..Default::default() }];
            snap.nodes[1].containers = vec![rank("0b0000000001", "worker", 1, worker, code)];
            Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(snap) }
        };
        let id = "cell1-vllm-8000";
        let tick = |ctx: Ctx| {
            let models = &models;
            async move { models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).expect("listed") }
        };

        let e = tick(at("exited", 137)).await;
        assert_eq!(e.health, Health::Down, "{:?}", e.health_reason);
        assert!(e.health_reason.as_deref().unwrap_or_default().starts_with("Worker on cell2 isn't running (killed, exit 137, "), "{:?}", e.health_reason);
        let remembered = models.eps.lock().unwrap().tracks.get(id).and_then(|t| t.stage.clone());
        assert!(matches!(remembered, Some(plan::Stage::Compiling { .. })), "its log still says so: {remembered:?}");
        assert!(!e.compiling);
        let s = e.supervisor.clone().expect("Keep running is on");
        assert!(s.next_retry_ms.is_some() && s.held.is_none(), "{s:?}");

        // The worker's back: compiling, and the retry is dropped rather than left to throw it away.
        tokio::time::sleep(Duration::from_millis(2_100)).await;
        let e = tick(at("running", 0)).await;
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Loading, Some("Compiling: FlashInfer kernels")));
        assert!(e.compiling);
        assert_eq!(e.supervisor.and_then(|s| s.next_retry_ms), None);
        live.stop();
    }

    /// A GB10's vLLM that answers its model list, /health and test tokens.
    fn engine() -> crate::fake_node::Script {
        Arc::new(|cmd: &str, _: &[u8]| {
            if cmd.contains("/v1/models") {
                return Some(("{\"data\":[{\"id\":\"llama-3.3-70b\"}]}\n200\n--sparkbox--\n\n200".into(), 0));
            }
            if cmd.contains("/v1/completions") || cmd.contains("/v1/chat/completions") {
                return Some((format!("{}\n200 0.09 0\n", include_str!("fixtures/probe/completion.json").trim()), 0));
            }
            None
        })
    }

    #[tokio::test]
    async fn a_probe_that_finds_the_ssh_channels_busy_leaves_the_endpoint_as_it_was() {
        use crate::fake_node::{fake_node, temp};
        use std::sync::atomic::Ordering;
        let (node, port) = fake_node(&temp("busy-probe"), engine()).await;
        let dir = temp("busy-probe-app");
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell1", port)]).await;
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        let ctx = || Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let id = "cell1-vllm-8000";
        let track = || models.eps.lock().unwrap().tracks.get(id).cloned().unwrap_or_default();
        let mut ready = None;
        for _ in 0..200 {
            let eps = models.endpoints_tick(&ctx(), false).await;
            if let Some(e) = eps.iter().find(|e| e.id == id && e.health == Health::Ready) {
                ready = Some(e.clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(ready.is_some(), "{:?}", models.endpoints().iter().find(|e| e.id == id));
        assert!(llm.get(id).await.is_some());
        let probes = || node.log.lock().unwrap().iter().filter(|c| c.contains("/v1/models")).count();
        let before = (track().fails, track().canary_fails, probes());

        // Every channel the node is asked for is refused, as sshd does past MaxSessions.
        node.knobs.refuse_next.store(100_000, Ordering::SeqCst);
        for _ in 0..3 {
            // A test token is due (the probe is forced): its last one was a minute ago.
            if let Some(c) = models.eps.lock().unwrap().tracks.get_mut(id).and_then(|t| t.canary.as_mut()) {
                c.at_ms = c.at_ms.saturating_sub(endpoints::CANARY_EVERY_MS);
            }
            let eps = models.endpoints_tick(&ctx(), true).await;
            let e = eps.iter().find(|e| e.id == id).unwrap();
            assert_eq!(e.health, Health::Ready, "busy is back-pressure, not a dead model: {:?}", e.health_reason);
            for _ in 0..100 {
                if !track().canary_running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        assert!(node.knobs.refused.load(Ordering::SeqCst) > 0, "the node did refuse");
        assert_eq!((track().fails, track().canary_fails, probes()), before, "nothing reached the engine, and nothing counted against it");
        assert!(llm.get(id).await.is_some(), "still offered in Chat");

        // A test token sent while the channels are busy (its round's probe got through) isn't a
        // failed token either.
        let e = models.endpoints().into_iter().find(|e| e.id == id).unwrap();
        let tried = track().canary_tried_ms;
        models.spawn_canary(e, endpoints::CanaryRequest { model: "llama-3.3-70b".into(), chat: false, keep_alive_s: None }, now_ms());
        for _ in 0..200 {
            if !track().canary_running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!track().canary_running && track().canary_tried_ms > tried, "it was tried");
        assert_eq!(track().canary_fails, 0);
        assert!(track().canary.as_ref().is_some_and(|c| c.ok), "the last real token still stands");

        // Channels free again: the next round asks the engine.
        node.knobs.refuse_next.store(0, Ordering::SeqCst);
        models.endpoints_tick(&ctx(), true).await;
        assert!(probes() > before.2);
        assert_eq!(track().fails, 0);
        live.stop();
    }

    #[tokio::test]
    async fn a_busy_spell_longer_than_a_minute_keeps_a_ready_model_ready_and_raises_no_alert() {
        use crate::fake_node::{fake_node, temp};
        use std::sync::atomic::Ordering;
        let (node, port) = fake_node(&temp("busy-spell"), engine()).await;
        let dir = temp("busy-spell-app");
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell1", port)]).await;
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        let ctx = || Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let id = "cell1-vllm-8000";
        let mut ready = false;
        for _ in 0..200 {
            let eps = models.endpoints_tick(&ctx(), false).await;
            if eps.iter().any(|e| e.id == id && e.health == Health::Ready && e.canary.as_ref().is_some_and(|c| c.ok)) {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(ready, "{:?}", models.endpoints().iter().find(|e| e.id == id));

        // The node's channels stay busy (sshd past MaxSessions) from the next probe on, and the
        // rounds go on: 70 s of them, then 40 more through the rule engine (endpoint-down waits 30 s).
        node.knobs.refuse_next.store(100_000, Ordering::SeqCst);
        let e = models.endpoints_tick(&ctx(), true).await.into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.health, Health::Ready, "{:?}", e.health_reason);
        models.age(70_000);
        let mut health = crate::health::Engine::default();
        let start = now_ms();
        for i in 0..9u64 {
            let eps = models.endpoints_tick(&ctx(), true).await;
            let e = eps.iter().find(|e| e.id == id).unwrap();
            assert_eq!(e.health, Health::Ready, "busy is no evidence, round {i}: {:?}", e.health_reason);
            let note = e.health_note.clone().unwrap_or_default();
            assert!(note.starts_with("Not checked since ") && note.ends_with(": SSH channels busy on cell1"), "{note}");
            assert!(llm.get(id).await.is_some(), "still offered in Chat");
            let mut snap = ctx().snapshot.unwrap();
            snap.ts_ms = start + i * 5_000;
            let events = health.step_with(&snap, &crate::health::endpoint_infos(&eps), snap.ts_ms);
            assert!(!events.iter().any(|ev| ev.alert.rule == "endpoint-down"), "{events:?}");
            models.age(5_000);
        }
        assert!(node.knobs.refused.load(Ordering::SeqCst) > 0, "the node did refuse");

        // Channels free again: it is asked, answers, and the note goes.
        node.knobs.refuse_next.store(0, Ordering::SeqCst);
        let mut checked = None;
        for _ in 0..200 {
            let eps = models.endpoints_tick(&ctx(), true).await;
            if let Some(e) = eps.iter().find(|e| e.id == id && e.health_note.is_none()) {
                checked = Some(e.clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let e = checked.expect("checked again");
        assert_eq!(e.health, Health::Ready, "{:?}", e.health_reason);
        live.stop();
    }

    #[tokio::test]
    async fn a_model_that_stops_answering_during_a_busy_spell_still_goes_down_once_it_is_asked() {
        use crate::fake_node::{fake_node, temp};
        use std::sync::atomic::{AtomicBool, Ordering};
        let up = Arc::new(AtomicBool::new(true));
        let u = up.clone();
        let answers = engine();
        let script: crate::fake_node::Script = Arc::new(move |cmd: &str, stdin: &[u8]| {
            if !u.load(Ordering::SeqCst) && (cmd.contains("/v1/") || cmd.contains("/health")) {
                return Some(("\n000".into(), 0));
            }
            answers(cmd, stdin)
        });
        let (node, port) = fake_node(&temp("busy-then-dead"), script).await;
        let dir = temp("busy-then-dead-app");
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell1", port)]).await;
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        let ctx = || Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let id = "cell1-vllm-8000";
        let health = |eps: Vec<Endpoint>| eps.into_iter().find(|e| e.id == id).map(|e| (e.health, e.health_reason));
        let mut ready = false;
        for _ in 0..200 {
            if health(models.endpoints_tick(&ctx(), false).await).is_some_and(|(h, _)| h == Health::Ready) {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(ready);
        node.knobs.refuse_next.store(100_000, Ordering::SeqCst);
        models.endpoints_tick(&ctx(), true).await;
        models.age(90_000);
        assert_eq!(health(models.endpoints_tick(&ctx(), true).await).map(|(h, _)| h), Some(Health::Ready));

        // The spell ends and the engine is gone: the probes that get through count as ever.
        up.store(false, Ordering::SeqCst);
        node.knobs.refuse_next.store(0, Ordering::SeqCst);
        let mut down = None;
        for _ in 0..200 {
            if let Some((Health::Down, why)) = health(models.endpoints_tick(&ctx(), true).await) {
                down = why;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // Its last answer is from before the spell: one probe that gets no answer is enough.
        assert!(down.as_deref().is_some_and(|w| w.starts_with("No answer for ") || w.contains("connection refused")), "{down:?}");
        live.stop();
    }

    #[tokio::test]
    async fn a_model_that_stops_answering_stays_in_chat_as_not_answering_and_stale_registrations_go() {
        use crate::fake_node::{fake_node, temp};
        use std::sync::atomic::{AtomicBool, Ordering};
        let up = Arc::new(AtomicBool::new(true));
        let u = up.clone();
        let answers = engine();
        let script: crate::fake_node::Script = Arc::new(move |cmd: &str, stdin: &[u8]| {
            if !u.load(Ordering::SeqCst) && (cmd.contains("/v1/") || cmd.contains("/health")) {
                return Some(("\n000".into(), 0)); // nothing listens any more
            }
            answers(cmd, stdin)
        });
        let (_node, port) = fake_node(&temp("flap-node"), script).await;
        let dir = temp("flap-app");
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell1", port)]).await;
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let id = "cell1-vllm-8000";
        // Left in providers.json: the registration of a load that was unloaded (a crash came
        // before it was taken out), what a scan saved for the engine discovery finds by itself,
        // and one added by hand whose name made its id start with vllm-.
        let saved = |id: &str, hidden: Vec<String>| Provider { id: id.into(), name: id.into(), base_url: "http://192.168.50.51:8000/v1".into(), hidden, ..Default::default() };
        llm.upsert(saved("vllm-old", vec![])).await.unwrap();
        llm.upsert(saved(id, vec!["old-model".into()])).await.unwrap();
        llm.upsert(saved("vllm-box", vec![])).await.unwrap();
        let unloaded = Deployment {
            spec: plan::DeploySpec { id: "old".into(), served_name: "qwen3-8b".into(), nodes: vec!["cell1".into()], ..Default::default() },
            endpoint: "http://192.168.50.51:8001/v1".into(),
            state: DeploymentState::Stopped,
            stage: None,
            message: "Unloaded".into(),
            log_tail: String::new(),
            started_at: 1,
            provider_id: "vllm-old".into(),
            demo: false,
            generation: 0,
            ..Default::default()
        };
        std::fs::write(dir.join("deployments.json"), serde_json::to_string(&[unloaded]).unwrap()).unwrap();
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        let ctx = || Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let until = |want: fn(&Endpoint) -> bool| {
            let models = models.clone();
            async move {
                for _ in 0..200 {
                    let eps = models.endpoints_tick(&ctx(), true).await;
                    if let Some(e) = eps.iter().find(|e| e.id == id).filter(|e| want(e)) {
                        return e.clone();
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                panic!("{id} never got there: {:?}", models.endpoints().iter().find(|e| e.id == id));
            }
        };
        until(|e| e.health == Health::Ready).await;
        assert_eq!(llm.saved_ids().await, ["anthropic", "openai", "demo", "vllm-box"], "the unloaded load's and the scan's went; the hand-added one stays");
        let titles: Vec<String> = live.audit.entries().into_iter().map(|e| e.title).collect();
        assert!(titles.iter().any(|t| t == "Chat provider removed") && titles.iter().any(|t| t == "Chat provider now found on the cluster"), "{titles:?}");
        assert_eq!(llm.get(id).await.unwrap().hidden, ["old-model"], "the scan's settings carried over");
        assert_eq!(llm.node_of("http://127.0.0.1:8000/v1").as_deref(), Some("cell1"), "Chat knows which node it's on");

        // It stops answering: Down, and Chat keeps it, marked, for the grace period.
        up.store(false, Ordering::SeqCst);
        until(|e| e.health == Health::Down).await;
        let v = llm.views().await.into_iter().find(|v| v.provider.id == id).expect("still listed");
        assert!(v.not_answering);
        assert!(llm.get(id).await.is_none() && llm.chat_provider(id).await.is_some());

        // Back: offered as before.
        up.store(true, Ordering::SeqCst);
        until(|e| e.health == Health::Ready).await;
        assert!(!llm.views().await.into_iter().find(|v| v.provider.id == id).unwrap().not_answering);
        live.stop();
    }

    /// An Ollama with nothing in memory and no request since it was last asked is asked every
    /// 60 s, not every 15 s (each probe is a line in its ollama.service journal): on a clock that
    /// runs an hour in 5 s rounds, at most 61 probes (it was 240). A request through SparkBox
    /// has it asked within 15 s, and something in memory keeps it at 15 s. (Counted as the probes
    /// the rounds send: Live shares an identical read within 2 s, and this clock runs faster.)
    #[tokio::test]
    async fn an_idle_ollama_is_asked_once_a_minute() {
        use crate::fake_node::{fake_node, temp};
        use std::sync::atomic::{AtomicBool, Ordering};
        let loaded = Arc::new(AtomicBool::new(false));
        let l = loaded.clone();
        let script: crate::fake_node::Script = Arc::new(move |cmd: &str, _: &[u8]| {
            if !cmd.contains(":11434/api/tags") {
                return None;
            }
            let ps = if l.load(Ordering::SeqCst) { r#"{"models":[{"name":"gemma3:27b","size":17000000000,"expires_at":"2099-01-01T00:00:00Z"}]}"# } else { r#"{"models":[]}"# };
            Some((format!("{{\"models\":[{{\"name\":\"gemma3:27b\",\"size\":17000000000}}]}}\n200\n--sparkbox--\n{ps}\n200\n--sparkbox--\nactive\n"), 0))
        });
        let (node, port) = fake_node(&temp("idle-ollama"), script).await;
        let dir = temp("idle-ollama-app");
        let (live, _) = crate::fake_node::connect(&dir, &[("cell1", port)]).await;
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        let id = "cell1-ollama-11434";
        let mut list = vec![Endpoint {
            id: id.into(),
            engine: Engine::Ollama,
            head_node: "cell1".into(),
            port: 11434,
            probe_host: "127.0.0.1".into(),
            listening: true,
            source: Source::Discovered,
            ..Default::default()
        }];
        // One round every 5 s from `from` for `secs`, as the app's 3 s rounds would see them:
        // how many asked the Ollama.
        let rounds = |models: Arc<Models>, list: Vec<Endpoint>, from: u64, secs: u64| async move {
            let mut asked = 0;
            for s in (0..=secs).step_by(5) {
                let looks = models.look_live(&list, &ClusterSnapshot::default(), from + s * 1000, false).await;
                asked += looks.get(id).is_some_and(|l| l.probe.as_ref().is_some_and(|p| p.ok)) as usize;
            }
            asked
        };
        let mut clock = now_ms();
        let idle_hour = rounds(models.clone(), list.clone(), clock, 3600).await;
        assert!((60..=61).contains(&idle_hour), "{idle_hour} probes in an hour idle");
        assert!(!node.ran("/api/ps").is_empty(), "the probes reached the node");
        clock += 3_600_000;

        // A request through SparkBox: the next round within 15 s asks again; it's still empty, so
        // back to once a minute.
        drop(llm.track(id, 10));
        clock += 5_000;
        models.metrics_live(&mut list, &HashMap::new(), clock);
        assert_eq!(rounds(models.clone(), list.clone(), clock, 15).await, 1, "asked within 15 s of the request");
        clock += 20_000;
        assert_eq!(rounds(models.clone(), list.clone(), clock, 35).await, 0, "idle again: once a minute");
        clock += 40_000;

        // A model in memory: every 15 s once a probe has seen it (past Live's 2 s reuse of the
        // last answer, which this clock outruns).
        loaded.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(2_100)).await;
        let seen = |m: &Models| !m.eps.lock().unwrap().empty_ms.contains_key(id);
        let from = clock;
        while !seen(&models) {
            assert!(clock - from <= 60_000, "a probe sees it within a minute");
            models.look_live(&list, &ClusterSnapshot::default(), clock, false).await;
            clock += 5_000;
        }
        assert_eq!(rounds(models.clone(), list.clone(), clock, 115).await, 8, "every 15 s with something in memory");
        live.stop();
    }

    /// Only an empty Ollama gets the slower schedule; vLLM (and any engine) keeps 15 s.
    #[test]
    fn only_an_empty_ollama_without_traffic_waits_a_minute() {
        let mut st = super::super::EndpointState::default();
        let ep = |id: &str, engine| Endpoint { id: id.into(), engine, ..Default::default() };
        let (ollama, vllm) = (ep("o", Engine::Ollama), ep("v", Engine::Vllm));
        for e in [&ollama, &vllm] {
            st.asked_ms.insert(e.id.clone(), 100_000);
            st.empty_ms.insert(e.id.clone(), 100_000);
        }
        let at = |s: u64| 100_000 + s * 1000;
        assert!(!Models::probe_due(&st, &ollama, at(59), false) && Models::probe_due(&st, &ollama, at(60), false));
        assert!(Models::probe_due(&st, &ollama, at(1), true), "forced");
        assert!(!Models::probe_due(&st, &vllm, at(14), false) && Models::probe_due(&st, &vllm, at(15), false));
        st.traffic_ms.insert("o".into(), at(3));
        assert!(Models::probe_due(&st, &ollama, at(15), false), "traffic since the last probe: 15 s");
        st.empty_ms.remove("o");
        st.traffic_ms.clear();
        assert!(Models::probe_due(&st, &ollama, at(15), false), "something in memory: 15 s");
    }

    /// Look-only that never asks for Touch ID; turning it off answers yes.
    struct Yes;

    impl crate::admin::Authenticator for Yes {
        fn authenticate(&self, _reason: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// In look-only the rounds go on, but the saved providers are left as they are and no row is
    /// written: the unloaded load's registration and the scan's entry stay. The first round
    /// after look-only ends reconciles them, with its rows.
    #[tokio::test]
    async fn look_only_leaves_the_saved_providers_and_the_activity_log_alone() {
        use crate::fake_node::{fake_node, temp};
        let (_node, port) = fake_node(&temp("reconcile-held"), engine()).await;
        let dir = temp("reconcile-held-app");
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell1", port)]).await;
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let id = "cell1-vllm-8000";
        let saved = |id: &str| Provider { id: id.into(), name: id.into(), base_url: "http://192.168.50.51:8000/v1".into(), ..Default::default() };
        llm.upsert(saved("vllm-old")).await.unwrap();
        llm.upsert(saved(id)).await.unwrap();
        let unloaded = Deployment {
            spec: plan::DeploySpec { id: "old".into(), served_name: "qwen3-8b".into(), nodes: vec!["cell1".into()], ..Default::default() },
            endpoint: "http://192.168.50.51:8001/v1".into(),
            state: DeploymentState::Stopped,
            stage: None,
            message: "Unloaded".into(),
            log_tail: String::new(),
            started_at: 1,
            provider_id: "vllm-old".into(),
            demo: false,
            generation: 0,
            ..Default::default()
        };
        std::fs::write(dir.join("deployments.json"), serde_json::to_string(&[unloaded]).unwrap()).unwrap();
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
        gate.set_look_only(true).unwrap();
        models.set_gate(gate.clone());
        let ctx = || Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let before = std::fs::read_to_string(dir.join("providers.json")).unwrap();
        let rows = || live.audit.entries().into_iter().filter(|e| e.title.starts_with("Chat provider")).count();
        let mut found = false;
        for _ in 0..200 {
            let eps = models.endpoints_tick(&ctx(), true).await;
            if eps.iter().any(|e| e.id == id && e.health == Health::Ready && e.source == Source::Discovered) {
                found = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(found, "{:?}", models.endpoints().iter().find(|e| e.id == id));
        for _ in 0..3 {
            models.endpoints_tick(&ctx(), true).await;
        }
        assert_eq!(llm.saved_ids().await, ["anthropic", "openai", "demo", "vllm-old", id], "nothing taken out or adopted");
        assert_eq!(std::fs::read_to_string(dir.join("providers.json")).unwrap(), before, "providers.json untouched");
        assert_eq!(rows(), 0, "no rows in look-only");

        // Look-only off: the next round reconciles as before.
        gate.set_look_only(false).unwrap();
        models.endpoints_tick(&ctx(), true).await;
        assert_eq!(llm.saved_ids().await, ["anthropic", "openai", "demo"]);
        assert_eq!(rows(), 2);
        live.stop();
    }

    // ---- p1r4: a SparkBox load is Ready only once it answers (P1.35), and gone from Chat after unload

    /// C-L2 for `spec` on the fake cell2, no key, with what a start lays down first, until it
    /// answers /health.
    async fn start_rank(live: &crate::live::Live, spec: &plan::DeploySpec) {
        use super::super::fake_vllm;
        fake_vllm::launch_files(live, "cell2", spec, 0, &[], None).await;
        let out = live.run("cell2", &plan::single_command(spec), None, Duration::from_secs(60)).await.unwrap();
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        fake_vllm::until_healthy(live, spec.port).await;
    }

    /// A fake cell2 with the vLLM stand-in serving `spec` (started with C-L2, no key), and Models
    /// with that load saved in `state`.
    async fn serving(name: &str, state: DeploymentState, readiness: Option<super::super::readiness::Readiness>) -> (crate::fake_node::Node, super::super::fake_vllm::Standin, Arc<crate::live::Live>, crate::inventory::Inventory, Arc<crate::llm::Llm>, Arc<Models>, plan::DeploySpec) {
        use super::super::fake_vllm;
        use crate::fake_node::temp;
        let home = temp(&format!("{name}-node"));
        let (node, port, standin) = fake_vllm::node(&home, fake_vllm::Knobs::default()).await;
        let dir = temp(&format!("{name}-app"));
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell2", port)]).await;
        let spec = fake_vllm::gemma(&home, name, false);
        start_rank(&live, &spec).await;
        let d = Deployment {
            spec: spec.clone(),
            endpoint: format!("http://127.0.0.1:{}/v1", spec.port),
            state,
            stage: Some(plan::Stage::Ready),
            message: String::new(),
            log_tail: String::new(),
            started_at: now_ms() / 1000,
            provider_id: String::new(),
            demo: false,
            generation: 0,
            readiness,
            ..Default::default()
        };
        std::fs::write(dir.join("deployments.json"), serde_json::to_string(&[d]).unwrap()).unwrap();
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        (node, standin, live, inventory, llm, models, spec)
    }

    fn passed() -> super::super::readiness::Readiness {
        super::super::readiness::Readiness {
            health: true,
            models_listed: true,
            token: Some(true),
            ok: true,
            token_ms: Some(1),
            summary: "Ready: /health 200, gemma-3-27b-it listed, a test token in 0.0 s (17+25 was checked on an earlier load of this recipe and image)".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_sparkbox_load_reads_loading_until_its_readiness_check_passes_never_on_health_alone() {
        let not_yet = super::super::readiness::Readiness { health: true, ..Default::default() };
        let (node, _s, live, inventory, _llm, models, spec) = serving("ready-gate", DeploymentState::Ready, Some(not_yet)).await;
        let ctx = Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let id = format!("cell2-vllm-{}", spec.port);
        // It answers /health and its model list, and test tokens come back: still not Ready.
        for round in 0..12 {
            let e = models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).expect("listed");
            assert_eq!((e.health, e.health_reason.as_deref()), (Health::Loading, Some("Up; checking it answers…")), "round {round}");
            assert_eq!(e.image.as_deref(), Some(super::super::fake_vllm::GLM53_IMAGE));
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(!node.ran(&format!("127.0.0.1:{}/health", spec.port)).is_empty(), "/health answered 200 all along");
        // A check that's held says why.
        let held = super::super::readiness::Readiness { health: true, models_listed: true, why: Some(super::super::readiness::HELD.into()), ..Default::default() };
        models.update(&spec.id, |d| d.readiness = Some(held));
        let e = models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.health_reason.as_deref(), Some("Up; checking it answers… (Look-only mode holds the test token; it's checked once look-only ends.)"));

        // The check passed: Ready once a test token proves it, with the readiness and protocols on the card.
        let protocols = super::super::readiness::Protocols { messages: true, count_tokens: true, responses: false, image_id: spec.image_id.clone(), checked_ms: 1 };
        models.update(&spec.id, |d| {
            d.readiness = Some(passed());
            d.protocols = Some(protocols.clone());
        });
        let mut ready = None;
        for _ in 0..200 {
            let e = models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).unwrap();
            if e.health == Health::Ready {
                ready = Some(e);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let e = ready.expect("Ready once readiness passed");
        assert!(e.canary.as_ref().is_some_and(|c| c.ok && c.after_round.is_some()), "{:?}", e.canary);
        assert_eq!(e.protocols, Some(protocols));
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["protocols"]["countTokens"], true);
        assert!(json["readiness"]["summary"].as_str().unwrap().starts_with("Ready: /health 200, gemma-3-27b-it listed"));
        live.stop();
    }

    /// A load an older SparkBox made Ready saved no readiness check, and this one's gate only runs
    /// while a load starts: after a restart it's Ready as any server is (it answers and a test
    /// token comes back), with a note, not "checking it answers" for ever.
    #[tokio::test]
    async fn a_load_an_older_sparkbox_made_ready_is_ready_with_a_note_after_a_restart() {
        let (node, _s, live, inventory, _llm, models, spec) = serving("older-ready", DeploymentState::Ready, None).await;
        let ctx = Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let id = format!("cell2-vllm-{}", spec.port);
        // What an earlier run's log said while it compiled, which Ready has to end. (After a first
        // round: that one sets the data source, which starts everything kept over.)
        models.endpoints_tick(&ctx, true).await;
        let compiled = Track { stage: Some(plan::Stage::Compiling { what: "FlashInfer kernels".into() }), progress_line: "compiling".into(), progress_ms: Some(now_ms()), ..Default::default() };
        models.eps.lock().unwrap().tracks.insert(id.clone(), compiled);
        let mut ready = None;
        for _ in 0..200 {
            let e = models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).expect("listed");
            assert_ne!(e.health_reason.as_deref(), Some("Up; checking it answers…"));
            if e.health == Health::Ready {
                ready = Some(e);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let e = ready.expect("Ready once it answers and generates");
        let t = models.eps.lock().unwrap().tracks.get(&id).cloned().unwrap();
        assert_eq!((t.stage, t.progress_line.as_str(), t.progress_ms), (None, "", None), "Ready ends what its log said, or it'd hold Keep running off a later crash");
        assert_eq!(e.health_note.as_deref(), Some(endpoints::BEFORE_READINESS));
        assert!(e.canary.as_ref().is_some_and(|c| c.ok && c.after_round.is_some()), "a test token came back: {:?}", e.canary);
        assert!(!node.ran("/v1/completions").is_empty());
        assert!(node.ran("17+25").is_empty(), "the rounds don't ask 17+25");
        let d = models.deployments(&ctx).into_iter().find(|d| d.spec.id == spec.id).unwrap();
        assert_eq!((d.state, d.readiness), (DeploymentState::Ready, None), "nothing claims a check that never ran");
        live.stop();
    }

    /// Look-only holding a load's readiness check is waiting: the stuck clock doesn't run, and it
    /// starts again from where the hold ends. Anything else quiet for 20 minutes says so, and the
    /// load stays Loading until its limit.
    #[tokio::test]
    async fn a_held_readiness_check_doesnt_run_the_stuck_clock() {
        use super::super::readiness::{Readiness, HELD};
        let (_node, _s, live, inventory, _llm, models, spec) = serving("held-clock", DeploymentState::Starting, None).await;
        let ctx = Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let id = format!("cell2-vllm-{}", spec.port);
        models.update(&spec.id, |d| d.started_at -= 30 * 60);
        let tick = || async {
            // Its stage became Ready 25 minutes ago.
            if let Some(t) = models.eps.lock().unwrap().tracks.get_mut(&id) {
                t.progress_ms = Some(now_ms() - 25 * 60_000);
            }
            let e = models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).expect("listed");
            let moved = models.eps.lock().unwrap().tracks.get(&id).and_then(|t| t.progress_ms).unwrap();
            (e.health, e.health_reason.unwrap_or_default(), now_ms() - moved)
        };
        models.endpoints_tick(&ctx, true).await;
        let held = Readiness { health: true, models_listed: true, why: Some(HELD.into()), ..Default::default() };
        models.update(&spec.id, |d| {
            d.readiness = Some(held);
            d.message = format!("Up; checking it answers… ({HELD})");
        });
        let (health, why, since) = tick().await;
        assert_eq!((health, why.as_str()), (Health::Loading, format!("Up; checking it answers… ({HELD})").as_str()));
        assert!(since < 60_000, "the clock starts again while it's held: {since} ms");
        // The same load waiting on something else for 25 minutes is quiet, not Down.
        let other = Readiness { health: true, why: Some("It doesn't answer /health yet (HTTP 503).".into()), ..Default::default() };
        models.update(&spec.id, |d| {
            d.readiness = Some(other);
            d.message = "Up; checking it answers… (It doesn't answer /health yet (HTTP 503).)".into();
        });
        let (health, why, _) = tick().await;
        assert_eq!((health, why.as_str()), (Health::Loading, "Up; checking it answers… (It doesn't answer /health yet (HTTP 503).) · no new progress for 25 min (limit 90 min)"));
        live.stop();
    }

    /// A round of probes reads as one row in the activity log. The probes themselves aren't logged
    /// (Live::probe); a load's test tokens are, because they carry its key on stdin, and every
    /// token a round sends is that round's one "Check models" row (ActivityLog.tsx folds a poll by
    /// its op id). The next round is the next row.
    #[tokio::test]
    async fn one_round_of_models_probes_is_one_check_models_poll_row() {
        use super::super::fake_vllm;
        use crate::fake_node::temp;
        let home = temp("check-row-node");
        let (_node, port, _standin) = fake_vllm::node(&home, fake_vllm::Knobs::default()).await;
        let dir = temp("check-row-app");
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell2", port)]).await;
        let mut saved = Vec::new();
        for id in ["check-row-a", "check-row-b"] {
            let spec = fake_vllm::gemma(&home, id, false);
            start_rank(&live, &spec).await;
            crate::secrets::set(&plan::key_account(id), "sbk-check-row").unwrap();
            saved.push(Deployment { endpoint: format!("http://127.0.0.1:{}/v1", spec.port), spec, state: DeploymentState::Ready, stage: Some(plan::Stage::Ready), started_at: now_ms() / 1000, readiness: Some(passed()), ..Default::default() });
        }
        std::fs::write(dir.join("deployments.json"), serde_json::to_string(&saved).unwrap()).unwrap();
        let llm = crate::llm::Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm, None);
        let ctx = Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let tokens = || live.audit.entries().into_iter().filter(|e| e.command.contains("/v1/completions")).collect::<Vec<_>>();
        let ended = |n: usize| {
            let tokens = &tokens;
            let models = &models;
            async move {
                for _ in 0..400 {
                    let running = models.eps.lock().unwrap().tracks.values().any(|t| t.canary_running);
                    if tokens().len() >= n && !running {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                panic!("{n} test tokens never ended: {:?}", tokens());
            }
        };
        models.endpoints_tick(&ctx, true).await;
        ended(2).await;
        let round = tokens();
        assert_eq!(round.len(), 2, "one for each load: {round:?}");
        let op = round[0].op_id.clone().expect("in the round's operation");
        assert!(op.starts_with("poll-models-"), "{op}");
        for e in &round {
            assert_eq!(e.op_id.as_ref(), Some(&op), "{round:?}");
            assert_eq!((e.title.as_str(), e.source.as_str(), e.class.as_str(), e.node.as_str()), ("Check models", "poll", "probe", "cell2"), "{e:?}");
        }
        assert!(!live.audit.entries().iter().any(|e| e.command.contains("sbk-check-row")), "the key stays on stdin");
        assert!(!live.audit.entries().iter().any(|e| e.command.contains("/v1/models")), "the probes aren't logged: one row isn't thousands");

        // The next round's tokens are the next row. (A round first: its /metrics counts those two
        // tokens as live traffic, and no test is due while that's fresh.)
        models.endpoints_tick(&ctx, true).await;
        models.age(endpoints::CANARY_EVERY_MS + 1_000);
        models.endpoints_tick(&ctx, true).await;
        ended(4).await;
        let ops: HashSet<String> = tokens().into_iter().filter_map(|e| e.op_id).collect();
        assert_eq!(ops.len(), 2, "{ops:?}");
        for id in ["check-row-a", "check-row-b"] {
            crate::secrets::clear(&plan::key_account(id)).unwrap();
        }
        live.stop();
    }

    #[tokio::test]
    async fn after_unload_chat_no_longer_offers_the_load_and_the_rounds_dont_bring_it_back() {
        let (_node, standin, live, inventory, llm, models, spec) = serving("gone-from-chat", DeploymentState::Ready, Some(passed())).await;
        let ctx = || Ctx { live: true, settings: Default::default(), inventory: inventory.clone(), snapshot: Some(live.snapshot(&inventory)) };
        let provider = format!("vllm-{}", spec.id);
        models.register(&spec.id).await;
        let offered = || {
            let llm = llm.clone();
            let provider = provider.clone();
            async move { llm.views().await.into_iter().any(|v| v.provider.id == provider) }
        };
        assert!(offered().await, "registered for Chat");
        let id = format!("cell2-vllm-{}", spec.port);
        for _ in 0..200 {
            if models.endpoints_tick(&ctx(), true).await.iter().any(|e| e.id == id && e.health == Health::Ready) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(offered().await);

        models.unload(&ctx(), &spec.id).await.unwrap();
        let mut stopped = false;
        for _ in 0..400 {
            if models.deployments(&ctx()).iter().any(|d| d.spec.id == spec.id && d.state == DeploymentState::Stopped) {
                stopped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(stopped, "{:?}", models.deployments(&ctx()));
        assert!(standin.running().is_empty(), "docker stop, then rm");
        assert!(standin.shm().is_empty(), "stopped with TERM: nothing left in /dev/shm");
        for round in 0..6 {
            let eps = models.endpoints_tick(&ctx(), true).await;
            assert!(!eps.iter().any(|e| e.deployment_id.as_deref() == Some(spec.id.as_str()) && e.health != Health::Gone), "round {round}: {eps:?}");
            assert!(!offered().await, "round {round}: {provider} offered again");
            assert!(llm.get(&provider).await.is_none());
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        live.stop();
    }
}
