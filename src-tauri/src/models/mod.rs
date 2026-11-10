//! Models on the cluster: find them on Hugging Face, check whether they'll run
//! well (deterministic fit numbers plus an AI analysis), download to the chosen
//! node, copy to the others over the fabric, load them with vLLM on any set of
//! nodes, unload, and benchmark. In Demo mode the cluster side is simulated.
//!
//! A vLLM load runs native, with no Ray and no shell in its path (plan.rs, live/launch.py). The
//! chosen nodes are checked as one set first (`preflight_set`); then every node gets its launch
//! files, the workers start in parallel, and the head starts last (`start_ranks`). A start that
//! fails has each rank's log saved to this Mac, then its containers and files are removed, head
//! first (`roll_back`), and it's never retried by itself, by SparkBox or by Docker's own restart
//! policy (`restart_count`). A restart goes the same way, and looks at the nodes before it stops
//! anything (`restart_blocked`).

pub mod advisor;
pub mod bench;
pub mod commands;
mod demo;
mod download_recovery;
mod download_tools;
mod disk;
pub use disk::{DiskCheck, DiskRequest};
pub use download_tools::{DownloadTool, DownloadToolReview, DownloadTools};
mod history;
pub mod history_view;
pub mod endpoints;
pub mod fit;
pub mod hf;
pub mod metrics;
pub mod ollama;
pub mod plan;
pub mod postmortem;
pub(crate) mod recovery;
mod image_jobs;
pub mod cache_grant;
pub mod earlyoom;
mod sim;
pub mod supervisor;
#[cfg(test)]
mod tests;
pub mod verify;
mod watch;
pub mod readiness;
pub mod recipe;
#[cfg(test)]
pub(crate) mod fake_vllm;

pub use watch::demo_engine_log;
#[cfg(test)]
pub use watch::demo_engine_log_push;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::demo::DemoCluster;
use crate::inventory::Inventory;
use crate::live::exit::as_class;
use crate::live::fence_class::FenceClass;
use crate::live::{audit, Live};
use crate::llm::{Llm, Provider, ProviderKind, DEFAULT_CLAUDE_MODEL};
use crate::model::{ClusterSnapshot, LinkStatus};
use crate::secrets;
use advisor::{AnalysisResult, Facts, NodeFacts};
use endpoints::Endpoint;
use fit::{ArchFacts, FitRow};
use hf::HfModel;
use plan::{DeploySpec, ModelNeeds, NodeNet, NodePlan, Stage};
pub use plan::DeploymentState;
pub use plan::FabricFact as FabricIf;

#[cfg(test)]
use crate::live::SPARKJOB;
/// Keychain name of the Hugging Face token.
pub const HF_ACCOUNT: &str = "hf";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelSettings {
    /// Where models live on every node; "~" is each node's home.
    pub model_root: String,
    /// The vLLM image a load starts when none was picked.
    pub image: String,
    /// The SGLang container for SGLang loads.
    pub sglang_image: String,
    pub advisor_provider: String,
    pub advisor_model: String,
}

impl Default for ModelSettings {
    fn default() -> Self {
        ModelSettings {
            model_root: "~/models".into(),
            image: plan::DEFAULT_IMAGE.into(),
            sglang_image: "lmsysorg/sglang:spark".into(),
            advisor_provider: "anthropic".into(),
            advisor_model: DEFAULT_CLAUDE_MODEL.into(),
        }
    }
}

/// What each call needs to know about the app's current state.
#[derive(Clone)]
pub struct Ctx {
    pub live: bool,
    pub settings: ModelSettings,
    pub inventory: Inventory,
    pub snapshot: Option<ClusterSnapshot>,
}

impl Ctx {
    pub(crate) fn node_ids(&self) -> Vec<String> {
        self.inventory.nodes.iter().map(|n| n.id.clone()).collect()
    }

    fn lan_address(&self, node: &str) -> String {
        self.inventory
            .nodes
            .iter()
            .find(|n| n.id == node)
            .map(|n| n.ip.clone().filter(|ip| !ip.is_empty()).unwrap_or_else(|| n.host.clone()))
            .unwrap_or_else(|| node.to_string())
    }

    fn online(&self) -> Vec<String> {
        match &self.snapshot {
            Some(s) => s.nodes.iter().filter(|n| n.status == LinkStatus::Online).map(|n| n.id.clone()).collect(),
            None => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDetails {
    pub model: HfModel,
    pub arch: ArchFacts,
    pub kv_bytes_per_token: Option<u64>,
    pub fit: Vec<FitRow>,
    /// The same table with an fp8 KV cache (--kv-cache-dtype fp8).
    pub fit_fp8: Vec<FitRow>,
    pub node_memory: u64,
    pub nodes_available: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    pub path: String,
    pub size_bytes: u64,
    pub verified: bool,
    /// Why it isn't finished: "downloading" or "copying" (running), "stopped" (left partway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<String>,
    /// What a verified copy was compared by: Hugging Face "sha256", "source-sha256", or legacy "sizes".
    #[serde(default)]
    pub check: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntry {
    /// The folder's absolute path, the same on every node that has a copy.
    pub key: String,
    /// The endpoint serving it right now, if one is.
    #[serde(default)]
    pub in_use: Option<String>,
    pub repo: String,
    pub revision: String,
    pub format: String,
    pub size_bytes: u64,
    pub root: String,
    pub copies: BTreeMap<String, Copy>,
}

/// The Library's detail pane: what a folder's config.json says (r4-serving).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDetail {
    pub architectures: Vec<String>,
    pub model_type: Option<String>,
    /// "nvfp4", "fp8", "awq"…, from its quantization_config.
    pub quantization: Option<String>,
    pub weight_bytes: u64,
    pub kv_bytes_per_token: Option<u64>,
    pub arch: ArchFacts,
}

fn string_list(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Job {
    pub id: String,
    pub node: String,
    pub kind: String,
    pub state: String,
    pub step: String,
    pub message: String,
    pub label: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub rate_bps: f64,
    pub started_at: u64,
    pub updated_at: u64,
    /// The folder a download or a pull writes, as an absolute path on its node.
    pub dest: String,
    pub resume: Option<DownloadResume>,
    pub tool_used: String,
    pub tool_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResume {
    pub repo: String,
    pub revision: String,
    pub dest: String,
    pub plan_hash: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageParsers {
    pub image_id: String,
    pub tools: Option<Vec<String>>,
    pub reasoning: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeCheck {
    pub image_ref: String,
    pub parsers: Option<ImageParsers>,
    #[serde(default)]
    pub versions: BTreeMap<String, Option<String>>,
    pub node: String,
    pub error: Option<String>,
    pub home: String,
    pub root_path: String,
    pub root_exists: bool,
    pub free_bytes: u64,
    pub docker: bool,
    pub rsync: bool,
    pub python: String,
    pub image_present: bool,
    pub vllm_version: String,
    /// The GPU as a container sees it (nvidia-smi -L's first line, "" when it doesn't see one).
    /// None when it wasn't asked: the quick check, or no image to ask in.
    pub gpu_in_container: Option<String>,
    pub fabric: Vec<FabricIf>,
    /// When this was read: results are kept in checks.json and show their age.
    pub checked_at_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub spec: DeploySpec,
    pub endpoint: String,
    pub state: DeploymentState,
    pub stage: Option<Stage>,
    pub message: String,
    pub log_tail: String,
    pub started_at: u64,
    pub provider_id: String,
    pub demo: bool,
    /// Bumped by each restart, so the previous watcher stands down.
    #[serde(default)]
    pub generation: u64,
    // ── p1r4 load ──
    /// What each node had before the load (the pre-flight at deploy): Unload checks against it.
    #[serde(default)]
    pub baseline: Vec<Baseline>,
    /// The last readiness check (P1.35): Ready only once it's ok.
    #[serde(default)]
    pub readiness: Option<readiness::Readiness>,
    /// The native APIs its engine serves, read once it was Ready (P1.37).
    #[serde(default)]
    pub protocols: Option<readiness::Protocols>,
    /// What the last unload found.
    #[serde(default)]
    pub unload_report: Option<UnloadReport>,
    /// Keep running may restart it once it's Failed (supervisor.rs): it has been Ready, and no
    /// start since got a verdict (the wrong answer, the time limit). A load that fails while
    /// starting keeps what it had, so a failed restart of one that ran is tried again.
    #[serde(default)]
    pub recoverable: bool,
    // ── p2r1 ──
    /// Why its last start failed and what that left (C-X). None until one does.
    #[serde(default)]
    pub failure: Option<FailureRecord>,
    /// What the head's log says about its traffic (C-T), for a load on several nodes.
    #[serde(default)]
    pub transport: Option<plan::Transport>,
    /// "It isn't using the RDMA fabric: …", when that log says so.
    #[serde(default)]
    pub transport_warning: Option<String>,
    /// It hasn't been restarted since `deploy`: a failure now takes its key with it (C-X).
    #[serde(default)]
    pub first_start: bool,
    /// Restart reconciliation is pending, or needs an explicit owner action.
    #[serde(default)]
    pub recovery: Option<recovery::Recovery>,
}

/// A start that failed (C-X): its cause, how each rank ended, where its logs are on this Mac, and
/// the nodes that still hold something of it, which Unload removes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FailureRecord {
    pub at_ms: u64,
    pub cause: String,
    pub ranks: Vec<RankEnd>,
    pub log_dir: String,
    pub left_on: Vec<String>,
}

/// One rank of a failed start, as it was when the failure was seen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RankEnd {
    pub node: String,
    pub rank: usize,
    pub role: String,
    /// Its exit code. None while it runs, or if its container was never made or can't be asked.
    pub exit: Option<i32>,
    /// Its log's last 40 lines, redacted.
    pub tail: String,
}

/// A node's memory and /dev/shm right before a load started.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub node: String,
    pub mem_available: u64,
    pub shm: Vec<String>,
    pub at_ms: u64,
}

/// What an unload found once the containers were gone (P1.36).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnloadReport {
    pub at_ms: u64,
    /// Nothing was left behind: memory back, the port closed, nothing new in /dev/shm.
    pub ok: bool,
    /// "Unloaded from cell2 · memory back (121.1 GiB available, baseline 121.4) · port 8000 closed · /dev/shm: nothing new".
    pub summary: String,
    pub nodes: Vec<NodeUnload>,
}

/// One node of an unload.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeUnload {
    pub node: String,
    /// None of the load's containers is left on it.
    pub stopped: bool,
    pub mem_available: Option<u64>,
    /// MemAvailable came back to within 2 GiB of the baseline.
    pub mem_back: bool,
    pub port_closed: bool,
    /// /dev/shm entries the baseline didn't have.
    pub new_shm: Vec<String>,
}

/// One command a load will run, shown for review before anything starts.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStep {
    pub node: String,
    pub title: String,
    pub command: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchRow {
    pub concurrency: u64,
    pub completed: u64,
    pub ttft_ms_median: u64,
    pub tokens_per_sec_per_stream: f64,
    pub aggregate_tokens_per_sec: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Benchmark {
    pub deployment: String,
    pub served_name: String,
    pub nodes: Vec<String>,
    pub tensor_parallel: u64,
    pub pipeline_parallel: u64,
    pub at: u64,
    pub rows: Vec<BenchRow>,
    pub demo: bool,
    /// The sweep it ran (none for the older quick benchmark).
    #[serde(default)]
    pub spec: Option<bench::BenchSpec>,
    #[serde(default)]
    pub levels: Vec<bench::BenchLevel>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub external: bool,
}

/// What the Load page shows before anything runs: the memory plan per node and the port.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployPlan {
    /// Advisory comparison with owner-confirmed inventory; never launch inputs.
    pub inventory_warnings: Vec<String>,
    pub nodes: Vec<NodePlan>,
    /// Green, amber or red: the worst node.
    pub level: plan::Level,
    pub needs: ModelNeeds,
    /// What already listens on the chosen port on the head node.
    pub port_taken_by: Option<String>,
    /// The first free port from 8000 on the head node.
    pub suggested_port: u16,
    /// Where the API will listen and how apps reach it.
    pub host: String,
    pub url: String,
    /// The API key as the preview shows it (masked), when one will be made.
    pub key_preview: Option<String>,
    /// The model's maximum context from its config.
    pub max_context: Option<u64>,
    /// The one GPU memory share to offer (plan::suggested_share): fits, and every node stays
    /// under the memory alert line.
    pub suggested_share: Option<f64>,
    /// When the context asked for doesn't fit under the line: that share, and the longest context that does.
    pub suggested_context: Option<(f64, u64)>,
    /// The share sized to this model on these nodes (plan::sized_share), which the Load page
    /// follows until someone chooses one. None: stay at fit::FIRST_LOAD_SHARE.
    pub sized_share: Option<f64>,
    /// How many full-length requests the KV cache holds at the share chosen.
    pub full_requests: Option<u64>,
    /// The recipe this folder gets, with what it's been verified on (vLLM loads).
    pub recipe: Option<recipe::RecipeView>,
    /// The splits to offer on these nodes, each with its verdict.
    pub topologies: Vec<plan::TopologyOffer>,
}

pub struct Models {
    pub(crate) live: Arc<Live>,
    llm: Arc<Llm>,
    http: reqwest::Client,
    deployment_history: Mutex<history::History>,
    pub(crate) benchmarks_path: PathBuf,
    deployments: Mutex<Vec<Deployment>>,
    pub(crate) benchmarks: Mutex<Vec<Benchmark>>,
    /// The last check of each node, kept in checks.json: the preview's fabric addresses and the
    /// Prepare tab's results survive a restart.
    checks: Mutex<Vec<NodeCheck>>,
    checks_path: PathBuf,
    pub(crate) demo: Mutex<demo::DemoModels>,
    /// The simulated cluster, for Demo loads to appear in Containers, Workloads and the charts.
    cluster: Option<Arc<Mutex<DemoCluster>>>,
    /// Endpoint probes, metrics and simulators (endpoints_tick).
    eps: Mutex<EndpointState>,
    /// config.json of library models, by folder, for the memory plan.
    configs: Mutex<HashMap<String, Value>>,
    /// The app's task list, for restarts (set when the endpoint round starts).
    tasks: Mutex<Option<Arc<crate::tasks::Tasks>>>,
    /// The health engine, which a verified restart tells to close its alerts.
    health: Mutex<Option<Arc<crate::health::Health>>>,
    /// The Admin gate, whose look-only and hold generation hold Keep running (supervisor.rs).
    gate: Mutex<Option<Arc<crate::admin::AdminGate>>>,
    /// Endpoints (or loads) with a restart under way: a second one waits for the first to finish.
    restarting: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Hugging Face list rows sized from their files (hf::sizing), kept a day.
    hf_sizes: Mutex<HashMap<String, (Instant, hf::Sizing)>>,
    // ── r4-serving ──
    /// Failed deployments' postmortems, read once per head container and exit.
    postmortems: Mutex<postmortem::Cache>,
    observed_path: PathBuf,
    resuming: Mutex<std::collections::HashSet<(String, u64)>>,
    // ── p1r2 jobs picked up again ──
    /// Downloads and copies a task follows (follow_job), kept in jobs-followed.json until it
    /// ends: the ones SparkBox quit while following are followed again on the next start.
    followed_path: PathBuf,
    followed: Mutex<Vec<Followed>>,
    /// Jobs a task follows in this run, by node and id.
    following: Mutex<std::collections::HashSet<(String, String)>>,
    /// The saved ones were picked up (on the first Live round once the task list is set).
    picked_up: std::sync::atomic::AtomicBool,
    // ── p1r3 copies' file servers re-armed ──
    /// Each copy's temporary file server with the pulls it serves, kept in jobs-shares.json until
    /// it's stopped: one SparkBox quit while following is stopped once its pulls end.
    shares_path: PathBuf,
    shares: Mutex<Vec<Share>>,
    /// Serve jobs a watcher follows in this run.
    sharing: Mutex<std::collections::HashSet<String>>,
    /// The saved ones were picked up (on the first Live round).
    shares_picked_up: std::sync::atomic::AtomicBool,
    // ── p1r4 load ──
    /// Each image's local id as the last pre-flight read it, by (node, image name): the Load
    /// page's preview shows the id the load will start.
    image_ids: Mutex<HashMap<(String, String), String>>,
    /// How often an unload reads MemAvailable, and for how long at most (5 s, 60 s).
    unload_wait: Mutex<(Duration, Duration)>,
    /// 17+25 verdicts by recipe and image (sanity.json).
    sanity: readiness::SanityStore,
    /// Each image's native APIs (protocols.json), for P4.
    protocol_store: readiness::ProtocolStore,
    /// One lock per load, by id, that its start and its unload take: an unload waits for a
    /// `docker run` under way, so it always sees the container that run makes.
    starting: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// How long a load may take to answer before it fails (90 minutes). Time look-only holds
    /// its test token doesn't count.
    ready_within: Mutex<Duration>,
    /// How long a node must stay out of reach before that limit fails a load (`LOST_GRACE`).
    lost_grace: Mutex<Duration>,
    // ── p2r1 ──
    /// How long one `docker run` may take (`DOCKER_RUN_LIMIT`).
    run_limit: Mutex<Duration>,
    /// The last pre-flight of each set of nodes, image and folder, kept in preflight.json with its time.
    preflights: Mutex<BTreeMap<String, plan::PreflightSet>>,
    preflights_path: PathBuf,
    /// What each recipe has passed readiness on, kept in recipes-verified.json.
    verified: Mutex<BTreeMap<String, Vec<recipe::Verified>>>,
    verified_path: PathBuf,
    /// failed-loads/<id>-<ms>/r<rank>.log: each rank's log, saved before a rollback removes it.
    failed_logs: PathBuf,
    /// `gate` stops between its register and its update: it says so on the first and goes on at
    /// the second (tests land an unload there).
    #[cfg(test)]
    after_register: Mutex<Option<(tokio::sync::oneshot::Sender<()>, tokio::sync::oneshot::Receiver<()>)>>,
}

/// A copy's temporary file server (sparkjob's serve job) and the pulls it serves, as saved for
/// the next start.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Share {
    source: String,
    serve_id: String,
    /// Each pull's node and sparkjob id.
    pulls: Vec<(String, String)>,
    /// When the server started (unix s): it stops by itself SERVE_TTL_S later.
    started_at: u64,
    /// "Qwen/Qwen3-8B", for the row that stops it.
    repo: String,
}

/// A download or copy a task follows, as saved for the next start: its node, its sparkjob id and
/// the task's title.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Followed {
    node: String,
    id: String,
    title: String,
}

/// How long a list row's size is kept before the Hub is asked again.
const HF_SIZE_KEEP: Duration = Duration::from_secs(24 * 3600);
/// How long one `docker run -d` may take to answer.
const DOCKER_RUN_LIMIT: Duration = Duration::from_secs(60);
/// `follow`'s wait before each look: starting, checking that it answers, serving.
#[cfg(not(test))]
const FOLLOW_POLL: [Duration; 3] = [Duration::from_secs(3), Duration::from_secs(5), Duration::from_secs(20)];
#[cfg(test)]
const FOLLOW_POLL: [Duration; 3] = [Duration::from_millis(300), Duration::from_millis(500), Duration::from_secs(2)];
/// How often a starting load's workers are asked how they are.
#[cfg(not(test))]
const WORKER_POLL: Duration = Duration::from_secs(15);
#[cfg(test)]
const WORKER_POLL: Duration = Duration::from_millis(300);
/// How a load's key reads wherever it's shown.
const KEY_SHOWN: &str = "sbk-••••••••";
/// The pre-flights kept in preflight.json, newest first.
const PREFLIGHTS_KEPT: usize = 32;
/// What a load's log tail keeps of each rank.
const TAIL_LINES: usize = 40;
/// What the Hugging Face detail says instead of a fit table, a download or a load for MLX weights.
pub const MLX_NOTE: &str = "MLX weights run on Apple silicon, not on a GB10. Look for the same model as safetensors, GGUF or NVFP4.";

#[derive(Default)]
struct EndpointState {
    list: Vec<Endpoint>,
    probes: HashMap<String, endpoints::Probe>,
    rings: HashMap<String, metrics::Ring>,
    vllm_sims: HashMap<String, sim::VllmSim>,
    ollama_sims: HashMap<String, sim::OllamaSim>,
    observed: HashMap<String, sim::Observed>,
    last_tick_ms: u64,
    /// Demo time, advanced by each round's step, so rates are per simulated second.
    sim_clock_ms: u64,
    /// Demo loads whose container has been added to the simulated cluster.
    injected: Vec<String>,
    was_live: Option<bool>,
    /// What each endpoint's evidence was last round (endpoints::Track).
    tracks: HashMap<String, endpoints::Track>,
    restart_log: watch::RestartLog,
    /// External endpoints as last seen, listed (as Down) for a day after they vanish.
    seen: HashMap<String, Endpoint>,
    /// Endpoints the user forgot, and the exit they were forgotten at.
    forgotten: HashMap<String, u64>,
    supervisors: watch::Supervisors,
    /// Rounds run so far.
    round: u64,
    /// The endpoint-stalled rule's wait, set each round from the rules (0: the default).
    stall_after_s: u64,
    /// Ordered restarts under way, by endpoint id, and when each started: Paused until it ends.
    planned: HashMap<String, u64>,
    /// Demo: the 10 s bucket of each engine's last stats line.
    logged_bucket: HashMap<String, u64>,
    /// The last test token of each endpoint already put in history.
    canaries_recorded: HashMap<String, u64>,
    /// Demo: each serving container's engine-stats lines, the last day of them.
    engine_log: watch::EngineLog,
    // ── p1r3 idle Ollama ──
    /// Live: when each endpoint was last asked (a busy try included).
    asked_ms: HashMap<String, u64>,
    /// Live: the Ollamas whose last answer had nothing in memory (`/api/ps` empty), and when.
    empty_ms: HashMap<String, u64>,
    /// Live: when SparkBox last sent or relayed a request to each endpoint.
    traffic_ms: HashMap<String, u64>,
}

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn short_id() -> String {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn token() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// "Restart llama-3.3-70b in order" (several nodes), "Restart qwen3.8-27b" (one container);
/// `verb` may carry a count after the name ("Keep running: restart|(2 of 5)").
fn restart_title(verb: &str, name: &str, multi: bool) -> String {
    let (verb, after) = verb.split_once('|').map(|(v, a)| (v, format!(" {a}"))).unwrap_or((verb, String::new()));
    let order = if multi { " in order" } else { "" };
    format!("{verb} {name}{order}{after}")
}

/// Who a pre-flight is for: only a new load tests the jumbo path (C-J), and Keep running drops the
/// page cache only while Admin mode is unlocked.
#[derive(Clone, Copy, PartialEq)]
enum Starting {
    Deploy,
    Restart,
    KeepRunning,
}

/// A manual grant is scoped to one restart attempt; the supervisor never receives one.
#[derive(Default)]
struct RestartOptions {
    by_supervisor: bool,
    drop_caches_once: bool,
}

/// A restart's task steps (there's no task in some tests).
struct Steps<'a> {
    task: Option<&'a crate::tasks::TaskHandle>,
}

impl Steps<'_> {
    fn step(&self, label: String) {
        if let Some(t) = self.task {
            t.step(label);
        }
    }

    fn ok(&self, detail: Option<String>) {
        if let Some(t) = self.task {
            t.step_ok(detail);
        }
    }

    fn failed(&self, detail: String) {
        if let Some(t) = self.task {
            t.step_failed(detail);
        }
    }

    /// A container it started is loading on its node by itself now: look-only can't stop that,
    /// and the task says so (tasks.rs `on_node`).
    fn started(&self) {
        if let Some(t) = self.task {
            t.on_node();
        }
    }
}

/// Holds a restart's place: a second restart of the same thing is refused until it's dropped.
/// The longest an ordered restart counts as planned for the health engine.
const PLANNED_RESTART_MS: u64 = 10 * 60_000;
const PLANNED_REASON: &str = "restarting in order";

/// Ends an ordered restart's planned state (planned_restart): the Paused note goes, and the health
/// engine's window closes from now (alerts' own grace then applies).
pub(crate) struct PlannedGuard {
    models: Arc<Models>,
    endpoint: String,
    nodes: Vec<String>,
    demo: bool,
}

impl Drop for PlannedGuard {
    fn drop(&mut self) {
        self.models.eps.lock().unwrap().planned.remove(&self.endpoint);
        let now = now_ms();
        crate::health::maintenance::begin_endpoint_at(self.demo, &self.endpoint, &self.nodes, Some(now), PLANNED_REASON, now);
    }
}

struct RestartSlot(Arc<Mutex<std::collections::HashSet<String>>>, String);

impl RestartSlot {
    fn take(all: &Arc<Mutex<std::collections::HashSet<String>>>, key: &str, name: &str) -> Result<RestartSlot, String> {
        if !all.lock().unwrap().insert(key.to_string()) {
            return Err(format!("{name} is already restarting: Tasks follows it."));
        }
        Ok(RestartSlot(all.clone(), key.to_string()))
    }
}

impl Drop for RestartSlot {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
}

/// How long a simulated container takes to stop or start (vLLM shuts down in a few seconds).
const DEMO_STEP: Duration = Duration::from_millis(1200);

/// One simulated step of an operation: as long as docker would take, then its activity-log line
/// (under the operation running it).
async fn demo_step(models: &Models, node: &str, title: &str, command: &str) {
    tokio::time::sleep(DEMO_STEP).await;
    models.live.audit.record_simulated(node, title, command);
}

/// Single-quotes an argument for the remote shell.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn load<T: for<'de> Deserialize<'de> + Default>(path: &PathBuf) -> T {
    crate::util::load_json_or_default(path)
}

pub(crate) fn save<T: Serialize>(path: &PathBuf, value: &T) {
    if let Err(e) = crate::util::save_json(path, value) {
        eprintln!("{e}");
    }
}

fn last_line(s: &str) -> String {
    s.trim().lines().last().unwrap_or("").trim().to_string()
}

/// "Download Qwen/Qwen3-8B to cell1" → "download Qwen/Qwen3-8B to cell1", after a verb.
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The download job's spec: only the served files (hf.rs served_files). sparkjob.py passes each
/// one to `hf download` as an --include and verifies only those, so an `original/` checkpoint
/// or a `metal/` copy is never fetched.
pub(crate) fn download_spec(model: &HfModel, dest: &str, image: &str, token: String) -> Value {
    json!({
        "repo": model.id, "revision": model.revision, "dest": dest, "image": image,
        "files": model.files, "include": [], "token": token, "instance": crate::identity::instance_name(),
        "label": format!("Download {}", model.id),
    })
}

/// A load that ran past its limit (`ready_within`).
fn not_ready_within(spec: &DeploySpec, within: Duration) -> String {
    let limit = if within.as_secs() >= 60 { format!("{} minutes", within.as_secs() / 60) } else { format!("{} s", within.as_secs()) };
    format!("{} didn't become ready within {limit}.", engine_label(spec))
}

/// sparkjob's absolute `dest` is the folder `wanted`, which may start with ~ (the node's home).
fn same_folder(dest: &str, wanted: &str) -> bool {
    match wanted.strip_prefix('~') {
        Some(rest) => dest.ends_with(rest),
        None => dest == wanted,
    }
}

/// A folder of links into the Hugging Face cache (<hub>/models--<org>--<name>/snapshots/<rev>).
fn is_hf_cache(key: &str) -> bool {
    key.split_once("/models--").and_then(|(_, rest)| rest.split_once('/')).is_some_and(|(_, after)| after.starts_with("snapshots/"))
}

/// What a fabric copy leaves out: checkpoints vLLM doesn't serve (hf.rs served_files).
const COPY_EXCLUDE: [&str; 3] = ["original/", "metal/", "*.pth"];
/// How often a download's or a copy's task reads its job's progress.
#[cfg(not(test))]
const JOB_POLL: Duration = Duration::from_secs(5);
#[cfg(test)]
const JOB_POLL: Duration = Duration::from_millis(200);
/// How long a copy's temporary file server runs at most: it stops by itself after that.
const SERVE_TTL_S: u64 = 12 * 3600;
/// After a restart, how long SparkBox waits for the SSH connections before it watches its loads
/// again, and how often it asks one that's serving, or still starting.
#[cfg(not(test))]
const RESUME_AFTER: Duration = Duration::from_secs(20);
#[cfg(test)]
const RESUME_AFTER: Duration = Duration::from_millis(200);
#[cfg(not(test))]
const RESUME_POLL: Duration = Duration::from_secs(20);
#[cfg(test)]
const RESUME_POLL: Duration = Duration::from_millis(500);
#[cfg(not(test))]
const RESUME_POLL_STARTING: Duration = Duration::from_secs(5);
#[cfg(test)]
const RESUME_POLL_STARTING: Duration = Duration::from_millis(300);
/// A look that reached a node decides the limit at once. One that didn't may be a VPN coming up or
/// a switch rebooting, and a Failed load isn't watched again, so the limit waits this long for it.
const LOST_GRACE: Duration = Duration::from_secs(10 * 60);

/// GPU reservation a fresh load can safely take, using the Load planner's host and alert headroom.
fn free_now(ctx: &Ctx) -> Vec<(String, u64)> {
    let Some(snap) = &ctx.snapshot else { return Vec::new() };
    snap.nodes
        .iter()
        .filter(|n| n.status == LinkStatus::Online && n.mem.total > 0)
        .map(|n| {
            let spec = DeploySpec { nodes: vec![n.id.clone()], ..Default::default() };
            let plan = plan::memory_plan(&spec, &ModelNeeds::default(), &[n], &[]);
            (n.id.clone(), (n.mem.total as f64 * plan[0].safe_share) as u64)
        })
        .collect()
}

/// "vLLM" or "SGLang", for messages about a load.
fn engine_label(spec: &DeploySpec) -> &'static str {
    if spec.engine == "sglang" {
        "SGLang"
    } else {
        "vLLM"
    }
}

/// What a starting load is doing, in a sentence.
fn stage_message(stage: &Stage) -> String {
    match stage {
        Stage::Starting => "Starting…".into(),
        Stage::Joining { have, need } => format!("{have} of {need} nodes up"),
        Stage::Loading { percent } => format!("Loading weights {percent}%"),
        Stage::Compiling { what } => format!("Compiling: {what}"),
        Stage::Warming => "Capturing CUDA graphs".into(),
        Stage::Ready => String::new(),
    }
}

/// A new load may only take nodes no other active load holds, and a port nobody uses on its head.
fn claim(all: &[Deployment], spec: &DeploySpec) -> Result<(), String> {
    if let Some(d) = all.iter().find(|d| d.state != DeploymentState::Stopped && d.recovery.is_some()
        && (d.spec.id == spec.id || d.spec.nodes.iter().any(|n| spec.nodes.contains(n)))) {
        return Err(format!("{} needs recovery review. Wait for fresh reports, or Unload it before starting another load on these nodes.", d.spec.served_name));
    }
    if all.iter().any(|d| d.state.active() && d.spec.id == spec.id) {
        return Err(format!("{} is already loading or serving.", spec.served_name));
    }
    let active: Vec<&Deployment> = all.iter().filter(|d| d.state.active()).collect();
    if let Some(n) = spec.nodes.iter().find(|n| active.iter().any(|d| d.spec.nodes.contains(n))) {
        return Err(format!("{n} is already serving another model; unload it first."));
    }
    if let Some(d) = active.iter().find(|d| d.spec.nodes.first() == spec.nodes.first() && d.spec.port == spec.port) {
        let e = plan::PreflightError::PortInUse { node: spec.nodes[0].clone(), port: spec.port, by: format!("SparkBox load {}", d.spec.served_name) };
        return Err(e.to_string());
    }
    Ok(())
}

/// A vLLM load starts the image picked for it (P1.32): the one the Load page sent, or the saved
/// image; its `image` names it too. Its local id is read from the nodes at deploy, never taken
/// from the UI.
fn pick_image(spec: &mut DeploySpec, settings: &ModelSettings) {
    if !plan::is_native(spec) {
        return;
    }
    if spec.image_ref.is_empty() {
        spec.image_ref = if spec.image.is_empty() {
            let picked = spec.imported_recipe.as_ref().or_else(|| recipe::by_id(&spec.recipe)).unwrap_or_else(|| recipe::recipe_for(&spec.model_path, None));
            if picked.id != recipe::GENERIC && !picked.image.is_empty() { picked.image.clone() } else { settings.image.clone() }
        } else { spec.image.clone() };
    }
    spec.image = spec.image_ref.clone();
    spec.image_id.clear();
}

/// Every container a load makes, by rank.
fn containers_of(spec: &DeploySpec) -> Vec<String> {
    let n = spec.nodes.len();
    if plan::is_native(spec) {
        return (0..n).map(|r| plan::rank_container(&spec.id, r, n)).collect();
    }
    spec.nodes.iter().enumerate().map(|(r, node)| plan::container_name(&spec.id, node, plan::role_of(spec, r))).collect()
}

/// "worker (rank 2)", "head (rank 0)", or "vLLM" for a load on one node.
fn rank_words(spec: &DeploySpec, rank: usize) -> String {
    match plan::role_of(spec, rank) {
        "single" => engine_label(spec).to_string(),
        role => format!("{role} (rank {rank})"),
    }
}

/// How a container is, then what `docker logs` adds to the same answer (a read). Its state is
/// "missing 0" when there's no such container.
fn rank_state_command(container: &str, logs: Option<u32>) -> String {
    let state = format!("docker inspect -f '{{{{.State.Status}}}} {{{{.State.ExitCode}}}}' {container} 2>/dev/null || echo 'missing 0'");
    match logs {
        Some(n) => format!("{state}; docker logs --tail {n} {container} 2>&1"),
        None => state,
    }
}

/// "exited 1" as (running, exit code); the code only for a container that has ended. "created 128"
/// has one too: a `docker run` that couldn't start it leaves it created, with a code of its own.
fn rank_state(line: &str) -> (bool, Option<i32>) {
    let mut w = line.split_whitespace();
    let word = w.next();
    let code = w.next().and_then(|c| c.parse().ok());
    match (word, code) {
        (Some("created"), Some(c)) if c != 0 => (false, Some(c)),
        (Some("running" | "restarting" | "created"), _) => (true, None),
        (Some("missing") | None, _) => (false, None),
        (Some(_), code) => (false, code),
    }
}

/// How many times Docker started this container again by itself: the probe's state line ends with
/// it (`plan::probe_command`). A running container's exit code is 0 whatever the last run ended
/// with, so this is what says it stopped.
fn restart_count(state: &str) -> u64 {
    state.split_whitespace().nth(2).and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// A start Docker's restart policy has already started again: a load with Keep running on one node
/// has `--restart unless-stopped` from its first `docker run`, and a poll reads it as running. A
/// start that stops isn't retried (C-X), so this is a failure like any other.
fn started_again(spec: &DeploySpec, node: &str, count: u64, logs: &str) -> String {
    let times = if count == 1 { "once".to_string() } else { format!("{count} times") };
    let tail = last_lines(logs, 8);
    format!("{} stopped on {node} while it was starting, and Docker started it again ({times}). A start that stops isn't retried. Last log lines:\n{tail}", engine_label(spec))
}

/// `plan::probe_command`'s answer: the container's state, /health's status and its log. curl says
/// 000 itself when nothing answers, and `|| echo 000` says it again, which isn't a log line.
fn probe_parts(out: &str) -> (String, String, String) {
    let mut lines = out.lines();
    let state = lines.next().unwrap_or("missing 0").to_string();
    let http = lines.next().unwrap_or("000").trim().to_string();
    let rest: Vec<&str> = lines.collect();
    let logs = if http == "000" { rest.strip_prefix(&["000"]).unwrap_or(&rest) } else { &rest };
    (state, http, logs.join("\n"))
}

/// `sparkjob.py manifest`'s files as path → size.
fn manifest_sizes(manifest: &Value) -> BTreeMap<String, u64> {
    let files = manifest["files"].as_array().into_iter().flatten();
    files.filter_map(|f| Some((f["path"].as_str()?.to_string(), f["size"].as_u64()?))).collect()
}

/// How many files differ between two listings, and the first of them.
fn files_differ(a: &BTreeMap<String, u64>, b: &BTreeMap<String, u64>) -> Option<(usize, String)> {
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    let differ: Vec<&&String> = keys.iter().filter(|k| a.get(**k) != b.get(**k)).collect();
    differ.first().map(|first| (differ.len(), (**first).clone()))
}

/// The last `n` lines of a log.
fn last_lines(log: &str, n: usize) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// A pre-flight's key in preflight.json: which nodes, image and folder it read.
fn preflight_key(spec: &DeploySpec) -> String {
    format!("{} {} {}", spec.nodes.join(","), plan::image_ref(spec), spec.model_path)
}

/// Writes one rank's saved log: this Mac's own folder, the owner's alone.
fn save_rank_log(dir: &std::path::Path, rank: usize, log: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let file = dir.join(format!("r{rank}.log"));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(file)?;
    f.write_all(log.as_bytes())
}

/// A start that failed (C-X): why, the nodes it may have left something on, and what `docker run`
/// wrote on each rank it failed on. A rank like that has no log of its own to save.
struct StartFailed {
    why: String,
    touched: Vec<String>,
    said: Vec<(usize, String)>,
}

impl StartFailed {
    fn on(why: String, touched: &[String]) -> StartFailed {
        StartFailed { why, touched: touched.to_vec(), said: Vec::new() }
    }
}

/// The exit refused it (look-only, the fence, the lease, the identity pin): its words stand.
fn refused(e: &str) -> bool {
    use crate::errors::ErrorKind;
    matches!(crate::errors::SbError::classify(e).kind, ErrorKind::LookOnly | ErrorKind::Fenced | ErrorKind::OtherController | ErrorKind::WrongNode)
}

/// An error as a load's message or a task's line shows it: a refusal the exit wrote as JSON
/// reads as its own words.
fn words(e: String) -> String {
    crate::errors::SbError::words(&e)
}

/// The line of a failed `docker run`'s stderr that says why: Docker ends with a help hint, a blank
/// line before it, and its reason starts `docker:`.
fn docker_said(stderr: &str) -> String {
    let said: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("Run 'docker") && !l.starts_with("See 'docker")).collect();
    let why = said.iter().rev().find(|l| l.starts_with("docker:")).or(said.last());
    why.map(|l| l.to_string()).unwrap_or_else(|| last_line(stderr))
}

/// A `docker run` that didn't exit 0. With nothing on stderr the channel closed before Docker
/// answered, so whether it started is unknown.
fn docker_run_failed(node: &str, stderr: &str) -> String {
    match docker_said(stderr).as_str() {
        "" => format!("{node}: docker run didn't finish: the connection dropped before it answered. Check Containers on {node}, then unload or try again."),
        said => format!("{node}: docker run failed: {said}"),
    }
}

/// What the set's read says stops a start, with the consent action for that kind of start.
fn set_blocked(set: &plan::PreflightSet, spec: &DeploySpec, why: Starting) -> Result<(), String> {
    if set.problems.is_empty() {
        return Ok(());
    }
    let share = spec.gpu_memory_utilization;
    let restart_words = |x: &String| set.nodes.iter().find(|p| why != Starting::Deploy && plan::short_of_memory(p, share, false) == *x).map(|p| plan::restart_short_of_memory(p, share));
    Err(set.problems.iter().map(|x| restart_words(x).unwrap_or_else(|| x.clone())).collect::<Vec<_>>().join(" "))
}

/// After an unload: a node's MemAvailable (a read).
const MEM_AVAILABLE: &str = "grep MemAvailable /proc/meminfo";
/// How close MemAvailable must come back to the load's baseline.
const MEM_BACK_SLACK: u64 = 2 * 1024 * 1024 * 1024;

/// "MemAvailable:   122356776 kB" → bytes.
fn mem_available(out: &str) -> Option<u64> {
    let kb = out.lines().find_map(|l| l.strip_prefix("MemAvailable:"))?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()?;
    Some(kb * 1024)
}

impl Models {
    pub fn new(dir: PathBuf, live: Arc<Live>, llm: Arc<Llm>, cluster: Option<Arc<Mutex<DemoCluster>>>) -> Arc<Models> {
        let deployments_path = dir.join("deployments.json");
        let (deployment_history, deployments) = history::History::open(deployments_path);
        let benchmarks_path = dir.join("benchmarks.json");
        let followed_path = dir.join("jobs-followed.json");
        let shares_path = dir.join("jobs-shares.json");
        let checks_path = dir.join("checks.json");
        let preflights_path = dir.join("preflight.json");
        let verified_path = dir.join("recipes-verified.json");
        Arc::new(Models {
            live,
            llm,
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build().expect("HTTP client"),
            deployments: Mutex::new(deployments),
            benchmarks: Mutex::new(load(&benchmarks_path)),
            deployment_history: Mutex::new(deployment_history),
            benchmarks_path,
            checks: Mutex::new(load(&checks_path)),
            checks_path,
            demo: Mutex::new(demo::DemoModels::new()),
            cluster,
            eps: Mutex::new(EndpointState::default()),
            configs: Mutex::new(HashMap::new()),
            tasks: Mutex::new(None),
            health: Mutex::new(None),
            gate: Mutex::new(None),
            restarting: Arc::new(Mutex::new(std::collections::HashSet::new())),
            hf_sizes: Mutex::new(HashMap::new()),
            postmortems: Mutex::new(postmortem::Cache::default()),
            observed_path: dir.join("deployments-observed.json"),
            resuming: Mutex::new(Default::default()),
            followed: Mutex::new(load(&followed_path)),
            followed_path,
            following: Mutex::new(Default::default()),
            picked_up: Default::default(),
            shares: Mutex::new(load(&shares_path)),
            shares_path,
            sharing: Mutex::new(Default::default()),
            shares_picked_up: Default::default(),
            image_ids: Mutex::new(HashMap::new()),
            unload_wait: Mutex::new((Duration::from_secs(5), Duration::from_secs(60))),
            sanity: readiness::SanityStore::open(&dir),
            protocol_store: readiness::ProtocolStore::open(&dir),
            starting: Mutex::new(HashMap::new()),
            ready_within: Mutex::new(Duration::from_millis(endpoints::START_LIMIT_MS)),
            lost_grace: Mutex::new(LOST_GRACE),
            run_limit: Mutex::new(DOCKER_RUN_LIMIT),
            preflights: Mutex::new(load(&preflights_path)),
            preflights_path,
            verified: Mutex::new(load(&verified_path)),
            verified_path,
            failed_logs: dir.join("failed-loads"),
            #[cfg(test)]
            after_register: Mutex::new(None),
        })
    }

    /// How often an unload reads MemAvailable, and for how long at most (tests shorten it).
    #[cfg(test)]
    pub(crate) fn set_unload_wait(&self, every: Duration, within: Duration) {
        *self.unload_wait.lock().unwrap() = (every, within);
    }

    /// How long a load may take to answer (tests shorten it).
    #[cfg(test)]
    pub(crate) fn set_ready_within(&self, within: Duration) {
        *self.ready_within.lock().unwrap() = within;
    }

    /// How long the limit waits for a node out of reach (tests shorten it).
    #[cfg(test)]
    pub(crate) fn set_lost_grace(&self, grace: Duration) {
        *self.lost_grace.lock().unwrap() = grace;
    }

    /// How long one `docker run` may take (tests shorten it).
    #[cfg(test)]
    pub(crate) fn set_run_limit(&self, limit: Duration) {
        *self.run_limit.lock().unwrap() = limit;
    }

    /// The lock load `id`'s start and unload share (`starting`).
    fn start_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.starting.lock().unwrap().entry(id.to_string()).or_default().clone()
    }

    /// Load `id` is starting or serving, at `generation`: no unload or restart came since.
    fn current(&self, id: &str, generation: u64) -> bool {
        self.deployments.lock().unwrap().iter().any(|d| d.spec.id == id && d.state.active() && d.generation == generation)
    }

    /// Marks load `id` Failed with `why`, unless it was unloaded meanwhile: a watcher that finds
    /// its containers gone after an unload leaves Stopped and the unload's report as they are.
    /// The message outlives the failure (endpoints and their alerts read it), so a refusal the
    /// exit wrote as JSON is put into words here, whoever reports it.
    fn fail(&self, id: &str, why: String) {
        let why = audit::redact(&words(why));
        self.update(id, |d| {
            if d.state != DeploymentState::Stopped {
                // Serving when it failed, whoever made it Ready (an older SparkBox never said so).
                d.recoverable |= d.state == DeploymentState::Ready;
                d.state = DeploymentState::Failed;
                d.message = why;
            }
        });
    }

    pub fn set_tasks(&self, tasks: Arc<crate::tasks::Tasks>) {
        *self.tasks.lock().unwrap() = Some(tasks);
    }

    pub fn set_health(&self, health: Arc<crate::health::Health>) {
        *self.health.lock().unwrap() = Some(health);
    }

    pub fn set_gate(&self, gate: Arc<crate::admin::AdminGate>) {
        *self.gate.lock().unwrap() = Some(gate);
    }

    /// The gate's hold state now (the app-wide look-only flag until a gate is set).
    pub(crate) fn hold_view(&self) -> crate::admin::HoldView {
        self.gate.lock().unwrap().as_ref().map(|g| g.hold_view()).unwrap_or_else(crate::admin::HoldView::global)
    }

    // ---- Hugging Face and analysis (real in both modes)

    fn hf_token() -> Option<String> {
        secrets::get(HF_ACCOUNT).ok().flatten()
    }

    pub async fn search(&self, query: &str) -> Result<Vec<hf::SearchHit>, String> {
        hf::search(&self.http, Self::hf_token().as_deref(), query).await
    }

    /// A search row's size from the repo's files, as the detail reads it (cached a day).
    pub async fn hf_size(&self, repo: &str) -> Result<hf::Sizing, String> {
        if let Some((at, s)) = self.hf_sizes.lock().unwrap().get(repo) {
            if at.elapsed() < HF_SIZE_KEEP {
                return Ok(s.clone());
            }
        }
        let s = hf::sizing(&self.http, Self::hf_token().as_deref(), repo).await?;
        self.hf_sizes.lock().unwrap().insert(repo.to_string(), (Instant::now(), s.clone()));
        Ok(s)
    }

    pub async fn hf_refs(&self, repo: &str) -> Result<Vec<hf::HfRef>, String> {
        hf::refs(&self.http, Self::hf_token().as_deref(), repo).await
    }

    pub async fn reviewed_model(&self, selection: &hf::DownloadSelection) -> Result<HfModel, String> {
        selection.validate()?;
        let model = hf::model_at(&self.http, Self::hf_token().as_deref(), &selection.repo, Some(&selection.revision)).await?;
        selection.check(&model)?;
        Ok(model)
    }

    pub async fn details(&self, ctx: &Ctx, repo: &str, revision: Option<&str>) -> Result<ModelDetails, String> {
        let model = hf::model_at(&self.http, Self::hf_token().as_deref(), repo, revision).await?;
        // The list sizes this repo the same way from now on.
        let sizing = hf::Sizing {
            id: model.id.clone(),
            weight_bytes: model.weight_bytes,
            weights_format: model.weights_format.clone(),
            min_nodes: (model.weight_bytes > 0).then(|| hf::min_nodes(model.weight_bytes, 4)).flatten(),
            mlx: model.mlx,
            gguf: model.skipped.iter().any(|x| x.what == "GGUF files"),
        };
        if revision.is_none() { self.hf_sizes.lock().unwrap().insert(model.id.clone(), (Instant::now(), sizing)); }
        let mut arch = fit::arch_facts(&model.config);
        arch.vision_bytes = fit::vision_bytes(&model.vision_config);
        arch.sparse_mla = fit::sparse_mla(&model.config, &model.architectures);
        let node_memory = ctx
            .snapshot
            .as_ref()
            .and_then(|s| s.nodes.iter().filter(|n| n.mem.total > 0).map(|n| n.mem.total).min())
            .unwrap_or(fit::DEFAULT_NODE_MEMORY);
        let nodes_available = ctx.inventory.nodes.len() as u64;
        let free_now = free_now(ctx);
        // MLX weights don't run here: no fit table for them.
        let table = |kv: Option<&str>| {
            // Every Spark is a GB10 (compute 12.1).
            let opts = fit::FitOptions { kv_dtype: kv.map(String::from), free_now: free_now.clone(), gb10: true, blocked: recipe::view_for(&model.id, None).note.filter(|_| recipe::view_for(&model.id, None).placeholder) };
            if model.mlx || model.config_error.is_some() { Vec::new() } else { fit::fit_table_with(model.weight_bytes, &arch, node_memory, nodes_available, &opts) }
        };
        Ok(ModelDetails {
            kv_bytes_per_token: fit::kv_bytes_per_token(&arch),
            fit: table(None),
            fit_fp8: table(Some("fp8")),
            arch,
            node_memory,
            nodes_available,
            model,
        })
    }

    pub async fn analyze(&self, ctx: &Ctx, repo: &str, revision: Option<&str>) -> Result<AnalysisResult, String> {
        let d = self.details(ctx, repo, revision).await?;
        let m = &d.model;
        if let Some(error) = &m.config_error { return Err(error.clone()); }
        let checks = self.checks.lock().unwrap().clone();
        let nodes = ctx
            .snapshot
            .as_ref()
            .map(|s| {
                s.nodes
                    .iter()
                    .map(|n| NodeFacts {
                        id: n.id.clone(),
                        online: n.status == LinkStatus::Online,
                        memory_total_bytes: n.mem.total,
                        memory_available_bytes: n.mem.available,
                        gpu_processes: n.gpu.procs.iter().map(|p| p.container.clone().unwrap_or(p.name.clone())).collect(),
                        model_disk_free_bytes: checks.iter().find(|c| c.node == n.id).map(|c| c.free_bytes),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let vllm = checks.iter().map(|c| c.vllm_version.clone()).find(|v| !v.is_empty());
        let facts = Facts {
            model: json!({
                "id": m.id, "revision": m.revision, "parameters": m.params, "weightBytes": m.weight_bytes,
                "weightsFormat": m.weights_format, "quantization": m.quantization, "dtypes": m.dtypes,
                "architectures": m.architectures, "modelType": m.model_type, "pipeline": m.pipeline,
                "license": m.license, "gated": m.gated, "maxContext": d.arch.max_context,
            }),
            kv_cache_bytes_per_token: d.kv_bytes_per_token,
            architecture: d.arch.clone(),
            fit_table: d.fit.clone(),
            nodes,
            engine: format!("{} (vLLM {})", ctx.settings.image, vllm.unwrap_or_else(|| "version not checked yet".into())),
            already_loaded: self
                .endpoints()
                .iter()
                .filter(|e| e.health != endpoints::Health::Down)
                .map(|e| {
                    let who = if e.managed_by == endpoints::ManagedBy::Sparkbox { "loaded by SparkBox" } else { "started outside SparkBox" };
                    format!("{} ({} on {}, port {}, {who})", e.models.join(", "), e.engine.label(), e.nodes.join("+"), e.port)
                })
                .collect(),
            demo_cluster: !ctx.live,
        };
        let s = &ctx.settings;
        self.llm.analyze(&s.advisor_provider, &s.advisor_model, &facts, &m.readme_excerpt).await
    }

    // ---- Running things on a node

    /// Read-only polling (health, /v1/models, /metrics), which needs no activity-log line each time.
    /// With no exit status the connection went before the command finished: what came back says
    /// nothing, least of all that a container is missing.
    async fn poll(&self, node: &str, cmd: &str, limit: Duration) -> Result<String, String> {
        let out = self.live.probe(node, cmd, limit).await?;
        if out.code.is_none() {
            return Err(format!("{node} closed the connection before it answered."));
        }
        Ok(out.stdout)
    }

    async fn sparkjob(&self, node: &str, args: &str, stdin: Option<&[u8]>, limit: Duration) -> Result<Value, String> {
        self.live.sparkjob(node, args, stdin, limit).await
    }

    /// A node's jobs. `list` also marks a dead job failed and removes the secret files of jobs
    /// that are over; in look-only it reports the same list and changes nothing (`list --look`).
    async fn list_jobs(&self, node: &str) -> Result<Value, String> {
        let args = if self.hold_view().look_only { "list --look" } else { "list" };
        self.sparkjob(node, args, None, Duration::from_secs(20)).await
    }

    async fn start_job(&self, node: &str, kind: &str, id: &str, spec: &Value) -> Result<Job, String> {
        let mut spec = spec.clone();
        spec["node"] = json!(node);
        let v = self.sparkjob(node, &format!("start {kind} {id}"), Some(spec.to_string().as_bytes()), Duration::from_secs(30)).await?;
        let mut job: Job = serde_json::from_value(v).map_err(|e| e.to_string())?;
        job.node = node.into();
        Ok(job)
    }

    // ---- Library

    pub async fn library(&self, ctx: &Ctx) -> Result<Vec<LibraryEntry>, String> {
        let mut list = self.library_scan(ctx).await?;
        let serving = endpoints::in_use(&self.endpoints());
        for e in &mut list {
            e.in_use = serving.get(&e.key).or_else(|| serving.get(&e.repo)).cloned();
        }
        Ok(list)
    }

    async fn library_scan(&self, ctx: &Ctx) -> Result<Vec<LibraryEntry>, String> {
        if !ctx.live {
            return Ok(self.demo.lock().unwrap().library());
        }
        let roots = format!("{} {}", q(&ctx.settings.model_root), q("~/.cache/huggingface/hub"));
        let nodes = ctx.online();
        let cmd = format!("scan {roots}");
        let scans = join_all(nodes.iter().map(|n| self.sparkjob(n, &cmd, None, Duration::from_secs(120)))).await;
        let mut entries: BTreeMap<String, LibraryEntry> = BTreeMap::new();
        for (node, scan) in nodes.iter().zip(scans) {
            for item in scan?.as_array().cloned().unwrap_or_default() {
                let path = item["path"].as_str().unwrap_or_default().to_string();
                let size = item["sizeBytes"].as_u64().unwrap_or(0);
                let e = entries.entry(path.clone()).or_insert_with(|| LibraryEntry {
                    key: path.clone(),
                    in_use: None,
                    repo: item["repo"].as_str().unwrap_or_default().into(),
                    revision: item["revision"].as_str().unwrap_or_default().into(),
                    format: item["format"].as_str().unwrap_or("safetensors").into(),
                    size_bytes: 0,
                    root: item["root"].as_str().unwrap_or_default().into(),
                    copies: BTreeMap::new(),
                });
                e.size_bytes = e.size_bytes.max(size);
                if e.revision.is_empty() {
                    e.revision = item["revision"].as_str().unwrap_or_default().into();
                }
                let partial = item["partial"].as_str().map(String::from);
                let check = item["checked"].as_str().unwrap_or_default().to_string();
                e.copies.insert(node.clone(), Copy { path, size_bytes: size, verified: item["verified"].as_bool().unwrap_or(false), partial, check });
            }
        }
        Ok(entries.into_values().collect())
    }

    // ---- Jobs

    pub async fn jobs(&self, ctx: &Ctx) -> Result<Vec<Job>, String> {
        if !ctx.live {
            return Ok(self.demo.lock().unwrap().jobs());
        }
        let nodes = ctx.online();
        // The Jobs tab polls this: SparkBox's own read, each round one poll in the activity log.
        let poll = audit::OpMeta { title: "Check jobs".into(), source: "poll".into() };
        let reads = audit::as_probe(join_all(nodes.iter().map(|n| self.list_jobs(n))));
        let lists = audit::with_op_id(format!("poll-jobs-{}", now_ms()), poll, reads).await;
        let mut jobs = Vec::new();
        for (node, list) in nodes.iter().zip(lists) {
            for v in list.map(|l| l.as_array().cloned().unwrap_or_default()).unwrap_or_default() {
                let resume = download_recovery::summary(&v, crate::identity::instance_name());
                if let Ok(mut j) = serde_json::from_value::<Job>(v) {
                    j.node = node.clone();
                    j.resume = resume;
                    jobs.push(j);
                }
            }
        }
        jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(jobs)
    }

    pub async fn cancel_job(&self, ctx: &Ctx, node: &str, id: &str) -> Result<(), String> {
        if !ctx.live {
            self.demo.lock().unwrap().cancel(id);
            return Ok(());
        }
        if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("bad job id".into());
        }
        let job = self.sparkjob(node, &format!("cancel {id}"), None, Duration::from_secs(20)).await?;
        // Its only other word: the container may still be writing, which the toast has to say.
        match job["message"].as_str() {
            Some(words) if words.starts_with("Canceled,") => Err(words.to_string()),
            _ => Ok(()),
        }
    }

    /// Downloads to the explicitly reviewed inventory node; never falls back to another node.
    pub async fn download(self: &Arc<Self>, ctx: &Ctx, selection: &hf::DownloadSelection, review: &DownloadToolReview) -> Result<Job, String> {
        selection.validate()?;
        self.check_download_tool(ctx, &review.node, review).await?;
        let model = self.reviewed_model(selection).await?;
        self.download_reviewed_model(ctx, selection, &model, review).await
    }

    async fn download_reviewed_model(self: &Arc<Self>, ctx: &Ctx, selection: &hf::DownloadSelection, model: &HfModel, review: &DownloadToolReview) -> Result<Job, String> {
        selection.check(model)?;
        self.download_model(ctx, model, review).await
    }

    /// `download`, once the model is read from Hugging Face.
    async fn download_model(self: &Arc<Self>, ctx: &Ctx, model: &HfModel, review: &DownloadToolReview) -> Result<Job, String> {
        if model.mlx {
            return Err(MLX_NOTE.into());
        }
        if model.weights_format == "none" {
            return Err(format!("{} has no weight files SparkBox can serve.", model.id));
        }
        let dest = format!("{}/{}", ctx.settings.model_root.trim_end_matches('/'), model.id);
        let node = review.node.clone();
        self.check_download_tool(ctx, &node, review).await?;
        let bytes = disk::file_bytes(&json!(model.files))?;
        self.destination_disk(ctx, &node, &dest, bytes).await?.require()?;
        if !ctx.live {
            let mut demo = self.demo.lock().unwrap();
            let mut job = demo.download(&node, model, &dest);
            demo.note_download_tool(&mut job, &review.tool);
            self.live.audit.record_simulated(&node, &format!("Download {}", model.id), &format!("python3 ~/.sparkbox/sparkjob.py start download {}", job.id));
            return Ok(job);
        }
        let jobs = self.list_jobs(&node).await?;
        let running = |j: &&Value| j["kind"] == "download" && j["state"] == "running" && same_folder(j["dest"].as_str().unwrap_or_default(), &dest);
        if jobs.as_array().into_iter().flatten().any(|j| running(&j)) {
            return Err(format!("A download of {} is already running on {node}. Follow it in Jobs.", model.id));
        }
        let mut spec = download_spec(model, &dest, &ctx.settings.image, Self::hf_token().unwrap_or_default());
        spec["tool"] = json!(review.tool);
        spec["installConsent"] = json!(review.install_consent);
        let job = self.start_job(&node, "download", &format!("dl-{}", short_id()), &spec).await?;
        self.follow_job(&job, format!("Download {} to {node}", model.id));
        Ok(job)
    }

    /// A job sparkjob.py runs by itself on its node (a download, a copy), as a task that shows its
    /// progress and ends with it. The job keeps running there whatever this Mac does, so the task
    /// is on-node: turning look-only on says so rather than claiming to stop it (cancel it from
    /// the Jobs tab).
    fn follow_job(self: &Arc<Self>, job: &Job, title: String) {
        self.follow_saved(Followed { node: job.node.clone(), id: job.id.clone(), title }, false);
    }

    /// Follows again the downloads and copies SparkBox quit while following, once, on the first
    /// Live round: each gets a task "(picked up again)" that ends with its job, whether that is
    /// still running on its node or ended while SparkBox was closed. Only this SparkBox's own
    /// (its data folder's list): the jobs folder on a node is shared with any other SparkBox.
    pub(crate) fn pick_up_jobs(self: &Arc<Self>, ctx: &Ctx) {
        use std::sync::atomic::Ordering;
        if self.tasks.lock().unwrap().is_none() || self.picked_up.swap(true, Ordering::SeqCst) {
            return;
        }
        let nodes = ctx.node_ids();
        let saved: Vec<Followed> = {
            let mut saved = self.followed.lock().unwrap();
            // A node no longer in the cluster can't be asked.
            let before = saved.len();
            saved.retain(|f| nodes.contains(&f.node));
            if saved.len() != before {
                save(&self.followed_path, &*saved);
            }
            saved.clone()
        };
        for f in saved {
            self.follow_saved(f, true);
        }
    }

    /// follow_job's task, for a job just started or (`again`) one picked up again.
    fn follow_saved(self: &Arc<Self>, f: Followed, again: bool) {
        let Some(tasks) = self.tasks.lock().unwrap().clone() else { return };
        let key = (f.node.clone(), f.id.clone());
        if !self.following.lock().unwrap().insert(key.clone()) {
            return;
        }
        {
            let mut saved = self.followed.lock().unwrap();
            if !saved.iter().any(|x| x.node == f.node && x.id == f.id) {
                saved.push(f.clone());
                save(&self.followed_path, &*saved);
            }
        }
        let title = if again { format!("{} (picked up again)", f.title) } else { f.title.clone() };
        let task = tasks.start(crate::tasks::TaskSpec::new("model", title, std::slice::from_ref(&f.node)).on_node());
        if again {
            task.log(format!("Following {} on {} again", f.id, f.node));
        }
        let me = self.clone();
        let (node, id) = key.clone();
        // Its rounds are one poll in the activity log ("Follow download Qwen/Qwen3-8B to cell1").
        let op = format!("poll-follow-{id}-{}", now_ms());
        let poll = audit::OpMeta { title: format!("Follow {}", lower_first(&f.title)), source: "poll".into() };
        tauri::async_runtime::spawn(async move {
            async {
                let deadline = Instant::now() + Duration::from_secs(24 * 3600);
                let mut last_step = String::new();
                while Instant::now() < deadline {
                    tokio::time::sleep(JOB_POLL).await;
                    // Its progress is SparkBox's own read, logged as a probe; a node away for a moment is asked again.
                    let read = audit::as_probe(me.list_jobs(&node));
                    let Ok(list) = audit::with_op_id(op.clone(), poll.clone(), read).await else { continue };
                    let Some(j) = list.as_array().into_iter().flatten().find(|j| j["id"] == *id.as_str()).cloned() else {
                        return task.finish_err(format!("{id} isn't listed on {node} any more."));
                    };
                    let text = |k: &str| j[k].as_str().unwrap_or_default().to_string();
                    match j["state"].as_str() {
                        Some("done") => return task.finish_ok(format!("Finished on {node}")),
                        Some("cancelled") => return task.finish_cancelled("Canceled from the Jobs tab"),
                        Some("failed") => return task.finish_err(text("message")),
                        _ => {
                            let (done, total) = (j["bytesDone"].as_u64().unwrap_or(0), j["bytesTotal"].as_u64().unwrap_or(0));
                            let step = [text("step"), text("message")].into_iter().find(|t| !t.is_empty());
                            if total > 0 {
                                task.progress_rate(done as f32 / total as f32, step, j["rateBps"].as_f64(), None);
                            } else if let Some(step) = step.filter(|s| *s != last_step) {
                                last_step = step.clone();
                                task.step(step);
                            }
                        }
                    }
                }
                task.finish_err(format!("{id} was still running on {node} after a day; the Jobs tab follows it."));
            }
            .await;
            me.following.lock().unwrap().remove(&key);
            let mut saved = me.followed.lock().unwrap();
            saved.retain(|x| !(x.node == node && x.id == id));
            save(&me.followed_path, &*saved);
        });
    }

    /// Copies a model folder from a node that has it to `targets`, over the fabric.
    pub async fn share(self: &Arc<Self>, ctx: &Ctx, key: &str, targets: &[String]) -> Result<Vec<Job>, String> {
        let known = ctx.node_ids();
        if targets.iter().any(|n| !known.contains(n)) || targets.iter().collect::<std::collections::HashSet<_>>().len() != targets.len() {
            return Err("Pick distinct nodes from this inventory.".into());
        }
        let entry = self.library(ctx).await?.into_iter().find(|e| e.key == key).ok_or("That model isn't in the library any more.")?;
        // A folder left partway, or still being written, is never a source.
        let source = disk::copy_source(ctx, &entry)?;
        let targets: Vec<String> = targets.iter().filter(|t| **t != source).cloned().collect();
        if targets.is_empty() {
            return Err("Pick at least one node that doesn't have it yet.".into());
        }
        // One left partway is picked up where it stopped; one still running isn't started twice.
        let running = |t: &&String| entry.copies.get(*t).is_some_and(|c| matches!(c.partial.as_deref(), Some("copying" | "downloading")));
        if let Some(t) = targets.iter().find(running) {
            return Err(format!("A copy to {t} is already running. Follow it in Jobs."));
        }
        if !ctx.live {
            let jobs = self.demo.lock().unwrap().share(key, &source, &targets);
            for j in &jobs {
                self.live.audit.record_simulated(&j.node, &format!("Copy {} from {source}", entry.repo), &format!("rsync rsync://sparkbox@{source}/model {key}"));
            }
            return Ok(jobs);
        }
        // Protocol 2 can validate the snapshot's content-addressed blob links, but the
        // app's cache copy/delete integration remains P3. Keep this admission limit.
        if is_hf_cache(key) {
            return Err("Copying a Hugging Face cache model isn't supported yet.".into());
        }

        let manifest = self.sparkjob(&source, &format!("manifest {}", q(key)), None, Duration::from_secs(60)).await?;
        let bytes = disk::file_bytes(&manifest["files"])?;
        for target in &targets {
            self.destination_disk(ctx, target, key, bytes).await?.require()?;
        }
        let check = self.check_node(ctx, &source, true).await;
        let fabric = check
            .fabric
            .iter()
            .find(|f| f.ip.starts_with("192.168.100."))
            .or_else(|| check.fabric.first())
            .ok_or_else(|| format!("{source} has no fabric address; run Prepare cluster in Models."))?
            .clone();
        let group = short_id();
        let password = token();
        let port = 8730 + (u16::from_str_radix(&group[..2], 16).unwrap_or(0) % 200);
        let allow = match fabric.ip.rsplit_once('.') {
            Some((net, _)) if fabric.prefix == 24 => format!("{net}.0/24"),
            _ => "192.168.0.0/16".into(),
        };
        let serve_id = format!("share-{group}-serve");
        self.start_job(
            &source,
            "serve",
            &serve_id,
            &json!({ "path": key, "address": fabric.ip, "port": port, "password": password, "allow": allow,
                     "exclude": COPY_EXCLUDE, "ttl": SERVE_TTL_S, "protocol": 2,
                     "expected": targets.iter().map(|t| format!("share-{group}-{t}")).collect::<Vec<_>>(),
                     "repo": entry.repo, "revision": entry.revision, "label": format!("Share {} from {source}", entry.repo) }),
        )
        .await?;
        // Saved from now until it's stopped, each pull as it starts: a quit at any point leaves the
        // server for the next start to stop.
        let mut share = Share { source: source.clone(), serve_id, pulls: Vec::new(), started_at: now(), repo: entry.repo.clone() };
        self.save_share(&share);

        let mut jobs = Vec::new();
        let mut failed = None;
        for t in &targets {
            let spec = json!({
                "src": format!("rsync://sparkbox@{}:{port}/model", fabric.ip), "dest": key, "password": password,
                "files": manifest["files"], "exclude": COPY_EXCLUDE, "checksums": true, "protocol": 2, "label": format!("Copy {} to {t}", entry.repo),
                "repo": entry.repo, "revision": entry.revision, "instance": crate::identity::instance_name(),
            });
            match self.start_job(t, "pull", &format!("share-{group}-{t}"), &spec).await {
                Ok(job) => {
                    share.pulls.push((job.node.clone(), job.id.clone()));
                    self.save_share(&share);
                    self.follow_job(&job, format!("Copy {} from {source} to {t}", entry.repo));
                    jobs.push(job);
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }

        // Stop the temporary file server once every copy that started has finished, this run or
        // the next; one no copy uses is stopped now (or by its watcher, if that can't be done yet).
        if failed.is_some() && share.pulls.is_empty() && !self.hold_view().look_only && self.stop_share(&share).await {
            self.forget_share(&share.serve_id);
        } else {
            self.watch_share(share);
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(jobs),
        }
    }

    /// Saves `share` in jobs-shares.json, in place of what was saved for it before.
    fn save_share(&self, share: &Share) {
        let mut saved = self.shares.lock().unwrap();
        match saved.iter_mut().find(|x| x.serve_id == share.serve_id) {
            Some(x) => *x = share.clone(),
            None => saved.push(share.clone()),
        }
        save(&self.shares_path, &*saved);
    }

    /// The nodes running a copy's temporary file server now, past whose TTL none is left: work the
    /// release build keeps its controller lease for (`live::exit`), though no task follows it.
    pub fn serving_nodes(&self) -> Vec<String> {
        self.shares.lock().unwrap().iter().filter(|s| now() < s.started_at + SERVE_TTL_S).map(|s| s.source.clone()).collect()
    }

    fn forget_share(&self, serve_id: &str) {
        let mut saved = self.shares.lock().unwrap();
        saved.retain(|x| x.serve_id != serve_id);
        save(&self.shares_path, &*saved);
    }

    /// Stops a copy's file server (`cancel <serve id>` on its node), as its own titled row. False
    /// when it's to be asked again: the node away for a moment or busy, or look-only. Refused, or
    /// no such job: done, the server stops by itself at its TTL.
    async fn stop_share(&self, share: &Share) -> bool {
        let title = format!("Stop the file server for the copy of {} on {}", share.repo, share.source);
        let cancel = format!("cancel {}", share.serve_id);
        let stop = self.sparkjob(&share.source, &cancel, None, Duration::from_secs(20));
        match audit::with_op_id(format!("{}-stop-{}", share.serve_id, now_ms()), audit::OpMeta { title, source: "system".into() }, stop).await {
            Ok(_) => true,
            Err(e) => !(matches!(crate::errors::SbError::classify(&e).kind, crate::errors::ErrorKind::Unreachable | crate::errors::ErrorKind::Timeout | crate::errors::ErrorKind::LookOnly) || crate::live::ssh::is_busy(&e)),
        }
    }

    /// Stops a copy's file server (`cancel <serve id>` on its node) once none of its pulls is
    /// running, as saved in jobs-shares.json until then. A pull its node can't be asked about
    /// counts as running: a failed poll never stops anything. Look-only holds the stop until it
    /// ends; past SERVE_TTL_S the server has stopped by itself.
    fn watch_share(self: &Arc<Self>, share: Share) {
        if !self.sharing.lock().unwrap().insert(share.serve_id.clone()) {
            return;
        }
        {
            let mut saved = self.shares.lock().unwrap();
            if !saved.iter().any(|x| x.serve_id == share.serve_id) {
                saved.push(share.clone());
                save(&self.shares_path, &*saved);
            }
        }
        let me = self.clone();
        // Its checks are one poll in the activity log; the stop is a row of its own.
        let op = format!("poll-{}-{}", share.serve_id, now_ms());
        let poll = audit::OpMeta { title: format!("Watch the file server for the copy of {} on {}", share.repo, share.source), source: "poll".into() };
        tauri::async_runtime::spawn(async move {
            let mut ended = false;
            while now() < share.started_at + SERVE_TTL_S {
                tokio::time::sleep(JOB_POLL).await;
                if !ended {
                    let mut running = false;
                    for (node, id) in &share.pulls {
                        let read = audit::as_probe(me.list_jobs(node));
                        running |= match audit::with_op_id(op.clone(), poll.clone(), read).await {
                            Ok(list) => list.as_array().into_iter().flatten().any(|j| j["id"] == *id.as_str() && j["state"] == "running"),
                            Err(_) => true,
                        };
                    }
                    ended = !running;
                }
                if !ended || me.hold_view().look_only {
                    continue;
                }
                if me.stop_share(&share).await {
                    break;
                }
            }
            me.sharing.lock().unwrap().remove(&share.serve_id);
            me.forget_share(&share.serve_id);
        });
    }

    /// Watches again the file servers of copies SparkBox quit while following, once, on the first
    /// Live round (only this SparkBox's own: its data folder's list).
    pub(crate) fn pick_up_shares(self: &Arc<Self>, ctx: &Ctx) {
        if self.shares_picked_up.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let nodes = ctx.node_ids();
        let saved: Vec<Share> = {
            let mut saved = self.shares.lock().unwrap();
            // A node no longer in the cluster can't be asked, and an old server has stopped by itself.
            let before = saved.len();
            saved.retain(|s| nodes.contains(&s.source) && now() < s.started_at + SERVE_TTL_S);
            if saved.len() != before {
                save(&self.shares_path, &*saved);
            }
            saved.clone()
        };
        for s in saved {
            self.watch_share(s);
        }
    }

    pub async fn delete(&self, ctx: &Ctx, key: &str, nodes: &[String]) -> Result<(), String> {
        if !ctx.live {
            if self.deployments(ctx).iter().any(|d| (d.state.active() || d.recovery.is_some()) && d.state != DeploymentState::Stopped && d.spec.model_path == key) {
                return Err("That model is loaded; unload it first.".into());
            }
            self.demo.lock().unwrap().delete(key, nodes);
            for n in nodes {
                self.live.audit.record_simulated(n, &format!("Delete {key}"), &format!("python3 ~/.sparkbox/sparkjob.py delete {}", q(key)));
            }
            return Ok(());
        }
        if self.deployments(ctx).iter().any(|d| (d.state.active() || d.recovery.is_some()) && d.state != DeploymentState::Stopped && d.spec.model_path == key) {
            return Err("That model is loaded; unload it first.".into());
        }
        let entry = self.library(ctx).await?.into_iter().find(|e| e.key == key).ok_or("That model isn't in the library any more.")?;
        // A job is still writing there. Before any node is touched: sparkjob refuses too, but only its own.
        let running: Vec<&str> = nodes
            .iter()
            .filter(|n| entry.copies.get(*n).is_some_and(|c| matches!(c.partial.as_deref(), Some("downloading" | "copying"))))
            .map(String::as_str)
            .collect();
        if !running.is_empty() {
            return Err(format!("Still running on {}. Follow it in Jobs.", running.join(", ")));
        }
        for n in nodes {
            self.sparkjob(n, &format!("delete {} {}", q(&entry.root), q(key)), None, Duration::from_secs(300)).await?;
        }
        Ok(())
    }

    // ---- Readiness

    /// The readiness check on one node. The full one reads the vLLM version in a short test
    /// container (`--rm`) named and labelled for this build (`--instance dev`), which the exit
    /// counts as container-test; the quick one only reads.
    async fn check_node(&self, ctx: &Ctx, node: &str, quick: bool) -> NodeCheck {
        let s = &ctx.settings;
        let args = format!(
            "check {} {}{}",
            q(&s.model_root),
            q(&s.image),
            if quick { " quick".to_string() } else { format!(" --instance {}", q(crate::identity::instance_name())) }
        );
        match self.sparkjob(node, &args, None, Duration::from_secs(240)).await {
            Ok(v) => NodeCheck { node: node.into(), ..serde_json::from_value(v).unwrap_or_default() },
            Err(e) => NodeCheck { node: node.into(), error: Some(crate::errors::SbError::classify(&e).message), ..Default::default() },
        }
    }

    /// Prepare cluster: per-node facts SparkBox needs before loading anything. The full check
    /// starts a test container, so in the test build the exit lets it through where a fence allows
    /// container-test on that node. Look-only refuses it (one "Refused" line) and the quick facts
    /// are read instead; without such a fence the refusal is that node's answer, not facts read
    /// some other way.
    pub async fn check(&self, ctx: &Ctx) -> Vec<NodeCheck> {
        let checks = if ctx.live {
            let nodes = ctx.node_ids();
            join_all(nodes.iter().map(|n| async move {
                let full = self.check_node(ctx, n, false).await;
                match full.error.as_deref().map(|e| crate::errors::SbError::classify(e).kind) {
                    Some(crate::errors::ErrorKind::LookOnly) => self.check_node(ctx, n, true).await,
                    _ => full,
                }
            }))
            .await
        } else {
            demo::checks(&ctx.node_ids(), &ctx.settings, ctx.snapshot.as_ref())
        };
        if ctx.live {
            self.keep_checks(checks.clone());
        } else {
            *self.checks.lock().unwrap() = checks.clone();
        }
        checks
    }

    /// Keeps each node's latest check, stamped now, in checks.json.
    fn keep_checks(&self, fresh: Vec<NodeCheck>) {
        let mut kept = self.checks.lock().unwrap();
        for mut c in fresh {
            c.checked_at_ms = now_ms();
            match kept.iter_mut().find(|k| k.node == c.node) {
                Some(k) => {
                    if c.parsers.is_none() && c.image_ref == k.image_ref && k.parsers.as_ref().is_some_and(|p| c.versions.get("imageId").and_then(Option::as_ref) == Some(&p.image_id)) {
                        c.parsers = k.parsers.clone();
                    }
                    *k = c;
                }
                None => kept.push(c),
            }
        }
        save(&self.checks_path, &*kept);
    }

    /// A node's fabric identity from what a check read.
    fn net_of(node: &str, fabric: &[FabricIf]) -> Option<NodeNet> {
        let tuples: Vec<(String, String, String)> = fabric.iter().map(|f| (f.iface.clone(), f.ip.clone(), f.rdma.clone())).collect();
        plan::pick_fabric(node, &tuples)
    }

    // ---- Loading and unloading

    pub fn deployments(&self, ctx: &Ctx) -> Vec<Deployment> {
        if !ctx.live {
            return self.demo.lock().unwrap().deployments();
        }
        self.deployments.lock().unwrap().iter().filter(|d| !d.demo).cloned().collect()
    }

    pub fn history_view(&self, live: bool) -> history_view::View {
        if !live { return history_view::demo(); }
        let dir = self.deployment_history.lock().unwrap().copies();
        history_view::read(&dir)
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Deployment)) {
        // Runtime observations remain in memory even if saving fails; cleanup must still be
        // possible. History reports its failure in the persistent UI warning.
        let _ = self.update_saved(id, f);
    }

    fn update_saved(&self, id: &str, f: impl FnOnce(&mut Deployment)) -> Result<(), String> {
        let mut all = self.deployments.lock().unwrap();
        let Some(d) = all.iter_mut().find(|d| d.spec.id == id) else { return Ok(()) };
        f(d);
        self.deployment_history.lock().unwrap().save(&all)
    }

    /// The commands a load would run on each node, using the fabric addresses from the last check.
    pub fn preview(&self, ctx: &Ctx, spec: &DeploySpec) -> Result<Vec<PreviewStep>, String> {
        if spec.replicas > 1 {
            return plan::replica_specs(spec)?.iter().map(|s| self.preview(ctx, s)).collect::<Result<Vec<_>, _>>().map(|v| v.into_iter().flatten().collect());
        }
        let mut spec = spec.clone();
        pick_image(&mut spec, &ctx.settings);
        recipe::apply(&mut spec)?;
        plan::validate(&spec)?;
        spec.host = self.resolve_host(ctx, &spec);
        let checks = if ctx.live { self.checks.lock().unwrap().clone() } else { demo::checks(&ctx.node_ids(), &ctx.settings, ctx.snapshot.as_ref()) };
        let net = |node: &str| {
            checks.iter().find(|c| c.node == node).and_then(|c| Self::net_of(node, &c.fabric)).unwrap_or_else(|| NodeNet {
                node_id: node.into(),
                iface: "<fabric-iface>".into(),
                ip: format!("<{node}-fabric-ip>"),
                hcas: vec![],
            })
        };
        if plan::is_native(&spec) {
            let nets: Vec<NodeNet> = spec.nodes.iter().map(|n| net(n)).collect();
            return Ok(self.preview_ranks(&spec, &nets, &checks));
        }
        let head = net(&spec.nodes[0]);
        let mut steps = vec![
            PreviewStep { node: spec.nodes.join(", "), title: "Pull the SGLang image (skipped when it's already there)".into(), command: plan::pull_script(&spec.image) },
            PreviewStep {
                node: spec.nodes[0].clone(),
                title: format!("Check that nothing else took port {} (stops here if something did)", spec.port),
                command: plan::port_check_command(spec.port),
            },
        ];
        for (i, n) in spec.nodes.iter().enumerate() {
            let title = match (spec.nodes.len(), i) {
                (1, _) => "Start SGLang".to_string(),
                (_, 0) => "SGLang rank 0: serves the API".to_string(),
                _ => format!("SGLang rank {i}: joins {} on port {}", head.ip, plan::SGLANG_DIST_PORT),
            };
            let mine = net(n);
            steps.push(PreviewStep { node: n.clone(), title, command: plan::sglang_command(&spec, i, &head.ip, (spec.nodes.len() > 1).then_some(&mine)) });
        }
        Ok(steps)
    }

    /// A vLLM load's steps (C-O): the pre-flight read on every node, the page-cache drop when it's
    /// ticked, the jumbo path from each worker, then per rank, workers first and the head last, its
    /// launch files (the key masked) and its `docker run`, by the local id the last pre-flight read.
    fn preview_ranks(&self, spec: &DeploySpec, nets: &[NodeNet], checks: &[NodeCheck]) -> Vec<PreviewStep> {
        let head = spec.nodes[0].clone();
        let every = spec.nodes.join(", ");
        let mut spec = spec.clone();
        let known = self.image_ids.lock().unwrap().get(&(head.clone(), spec.image_ref.clone())).cloned();
        let id_note = match known {
            Some(id) => {
                spec.image_id = id;
                String::new()
            }
            None => {
                spec.image_id = format!("<local id of {} on {head}>", spec.image_ref);
                "# the image's local id is read from the node just before the start\n".to_string()
            }
        };
        let mut steps = vec![PreviewStep {
            node: every.clone(),
            title: "Check each node: memory, earlyoom, the image's id, Docker's image store, the model folder, the ports, /dev/shm, the cache folders, what earlier loads left, and Ollama (stops here if something is wrong)".into(),
            command: plan::preflight_script(&spec),
        }];
        if spec.drop_caches {
            steps.push(PreviewStep {
                node: every,
                title: "Drop the page cache on a node, only if vLLM would refuse to start without it (sudo)".into(),
                command: format!("# as root through sudo, the cluster password on its stdin\n{}", plan::DROP_CACHES),
            });
        }
        let fabric = |n: &str| checks.iter().find(|c| c.node == n).map(|c| c.fabric.as_slice()).unwrap_or(&[]);
        for worker in &spec.nodes[1..] {
            let pairs = plan::jumbo_pairs(fabric(worker), fabric(&head));
            if !pairs.is_empty() {
                steps.push(PreviewStep { node: worker.clone(), title: format!("Test the jumbo-frame path from {worker} to {head} (stops here if it fails)"), command: plan::jumbo_script(&pairs) });
            }
        }
        for rank in (1..spec.nodes.len()).chain(std::iter::once(0)) {
            let node = spec.nodes[rank].clone();
            let who = rank_words(&spec, rank);
            let single = spec.nodes.len() == 1;
            let shown = (spec.require_key && rank == 0).then_some(KEY_SHOWN);
            steps.push(PreviewStep {
                node: node.clone(),
                title: if single { "Write vLLM's launch files".into() } else { format!("Write the launch files for the {who}") },
                command: serde_json::to_string_pretty(&plan::launch_payload(&spec, rank, nets, shown)).unwrap_or_default(),
            });
            let title = if single { "Start vLLM".to_string() } else { format!("Start the {who}") };
            steps.push(PreviewStep { node, title, command: format!("{id_note}{}", plan::rank_command(&spec, rank)) });
        }
        steps
    }

    /// The address a load's API binds to, from its "Listen on" choice.
    pub fn resolve_host(&self, ctx: &Ctx, spec: &DeploySpec) -> String {
        let Some(head) = spec.nodes.first() else { return String::new() };
        match spec.listen.as_str() {
            "all" => String::new(),
            "fabric" => {
                let checks = if ctx.live { self.checks.lock().unwrap().clone() } else { demo::checks(&ctx.node_ids(), &ctx.settings, ctx.snapshot.as_ref()) };
                let from_check = checks.iter().find(|c| &c.node == head).and_then(|c| {
                    let t: Vec<(String, String, String)> = c.fabric.iter().map(|f| (f.iface.clone(), f.ip.clone(), f.rdma.clone())).collect();
                    plan::pick_fabric(head, &t).map(|n| n.ip)
                });
                from_check
                    .or_else(|| {
                        let snap = ctx.snapshot.as_ref()?;
                        let n = snap.nodes.iter().find(|n| &n.id == head)?;
                        n.nets.iter().filter(|x| x.role == crate::model::NetRole::Fabric).find_map(|x| x.ip.clone().filter(|ip| ip.starts_with("192.168.100.")))
                    })
                    .unwrap_or_default()
            }
            _ => {
                let lan = ctx.lan_address(head);
                if lan.chars().all(|c| c.is_ascii_digit() || c == '.') { lan } else { String::new() }
            }
        }
    }

    /// The memory plan and port check the Load page shows, and `deploy()` enforces.
    pub async fn plan(&self, ctx: &Ctx, spec: &DeploySpec) -> Result<DeployPlan, String> {
        let needs = self.needs_on(ctx, &spec.model_path, &spec.nodes).await;
        let mut spec = spec.clone();
        spec.host = self.resolve_host(ctx, &spec);
        let empty = ClusterSnapshot::default();
        let snap = ctx.snapshot.as_ref().unwrap_or(&empty);
        let nodes: Vec<&crate::model::NodeSnapshot> = snap.nodes.iter().filter(|n| spec.nodes.contains(&n.id)).collect();
        let own: Vec<String> = self.deployments(ctx).iter().filter(|d| d.spec.id == spec.id).flat_map(|d| containers_of(&d.spec)).collect();
        let per_node = plan::memory_plan(&spec, &needs, &nodes, &own);
        let level = per_node.iter().map(|p| p.level).max_by_key(|l| *l as u8).unwrap_or(plan::Level::Green);
        let head = spec.nodes.first().and_then(|h| snap.nodes.iter().find(|n| &n.id == h));
        let reserved: Vec<u16> = self
            .deployments(ctx)
            .iter()
            .filter(|d| d.state.active() && d.spec.nodes.first() == spec.nodes.first() && d.spec.id != spec.id)
            .map(|d| d.spec.port)
            .collect();
        // Edit & reload: the load being replaced (this id) serves on its own port until it's
        // unloaded, and deploy() checks the port again after that.
        let own_port = self.replaced(ctx, &spec).is_some_and(|d| d.spec.port == spec.port);
        let port_taken_by = head
            .and_then(|h| plan::port_owner(h, spec.port))
            .filter(|_| !own_port)
            .or_else(|| reserved.contains(&spec.port).then(|| "another SparkBox load".to_string()));
        let suggested_port = head.map(|h| plan::first_free_port(h, &reserved)).unwrap_or(plan::FIRST_PORT);
        let shown = if spec.host.is_empty() { spec.nodes.first().map(|h| ctx.lan_address(h)).unwrap_or_default() } else { spec.host.clone() };
        let recipe = self.recipe_view(&spec);
        let picked = spec.imported_recipe.as_ref().or_else(|| recipe.as_ref().and_then(|r| recipe::by_id(&r.id)));
        let mut topologies = picked.map(|r| r.offers(&needs.arch, spec.nodes.len())).unwrap_or_else(|| plan::topologies(&needs.arch, spec.nodes.len()));
        if spec.engine == "vllm" && needs.arch.sparse_mla && fit::kv_dtype_factor(spec.kv_cache_dtype.as_deref()) >= 1.0 {
            for offer in topologies.iter_mut().filter(|o| o.ok) { offer.ok = false; offer.reason = Some(fit::NO_KERNEL_GB10.into()); }
        }
        Ok(DeployPlan {
            inventory_warnings: crate::inventory::fabric::planning_warnings(&ctx.inventory, &snap.nodes, &spec.nodes, spec.listen == "fabric"),
            recipe,
            topologies,
            suggested_share: plan::suggested_share(&spec, &per_node),
            suggested_context: plan::suggested_context(&spec, &per_node),
            sized_share: plan::sized_share(&spec, &needs, &per_node),
            full_requests: plan::full_requests(&spec, &per_node),
            max_context: needs.arch.max_context,
            nodes: per_node,
            level,
            needs,
            port_taken_by,
            suggested_port,
            url: format!("http://{shown}:{}/v1", spec.port),
            host: if spec.host.is_empty() { "0.0.0.0".into() } else { spec.host.clone() },
            key_preview: spec.require_key.then(|| KEY_SHOWN.into()),
        })
    }

    /// The recipe a vLLM load gets: the one picked on the Load page, or the folder's, with what
    /// else fits and what it has been verified on.
    fn recipe_view(&self, spec: &DeploySpec) -> Option<recipe::RecipeView> {
        if !plan::is_native(spec) {
            return None;
        }
        let mut view = recipe::view_for(&spec.model_path, None);
        if let Some(imported) = &spec.imported_recipe {
            if recipe::validate(imported).is_ok() && !view.placeholder {
                view = imported.view(vec![recipe::RecipeChoice { id: imported.id.clone(), title: imported.title.clone() }]);
            }
        }
        if let Some(picked) = recipe::recipes().into_iter().find(|r| r.id == spec.recipe && r.id != view.id) {
            view = recipe::RecipeView { fits: view.fits, ..picked };
        }
        view.verified_on = self.verified.lock().unwrap().get(&view.id).cloned().unwrap_or_default();
        Some(view)
    }

    /// Every built-in recipe, with what each has been verified on (Settings › Models).
    pub fn recipes(&self) -> Vec<recipe::RecipeView> {
        let verified = self.verified.lock().unwrap();
        let mut all = recipe::recipes();
        for r in &mut all {
            r.verified_on = verified.get(&r.id).cloned().unwrap_or_default();
        }
        all
    }

    /// `recipes-verified.json`: this recipe, on this image and split, passed readiness.
    fn note_verified(&self, spec: &DeploySpec) {
        let id = if spec.recipe.is_empty() { recipe::GENERIC } else { &spec.recipe };
        let row = recipe::Verified { image_id: spec.image_id.clone(), topology: plan::topology_id(spec.tensor_parallel, spec.pipeline_parallel), node_count: spec.nodes.len() as u64, at_ms: now_ms() };
        let mut all = self.verified.lock().unwrap();
        let rows = all.entry(id.to_string()).or_default();
        rows.retain(|v| !(v.image_id == row.image_id && v.topology == row.topology && v.node_count == row.node_count));
        rows.push(row);
        save(&self.verified_path, &*all);
    }

    /// Weights and attention layout of a library folder (its config.json), for the memory plan.
    pub async fn needs(&self, ctx: &Ctx, path: &str) -> ModelNeeds {
        self.needs_on(ctx, path, &[]).await
    }

    /// `needs`, asking `nodes` first: the folder may be on none of the others.
    async fn needs_on(&self, ctx: &Ctx, path: &str, nodes: &[String]) -> ModelNeeds {
        let info = self.folder_info(ctx, path, nodes).await;
        let cfg = &info["config"];
        let text = if cfg["text_config"].is_object() { cfg["text_config"].clone() } else { cfg.clone() };
        let mut arch = fit::arch_facts(&text);
        arch.vision_bytes = fit::vision_bytes(&cfg["vision_config"]);
        arch.sparse_mla = arch.sparse_mla || fit::sparse_mla(&text, &string_list(&cfg["architectures"]));
        ModelNeeds { weight_bytes: info["bytes"].as_u64().unwrap_or(0), kv_bytes_per_token: fit::kv_bytes_per_token(&arch), arch }
    }

    /// What the Library's detail pane says about a folder: its config's architecture, quantisation
    /// and attention layout, and the weights' size. Reads config.json and du on a node holding it.
    pub async fn library_detail(&self, ctx: &Ctx, path: &str) -> LibraryDetail {
        let needs = self.needs(ctx, path).await;
        let info = self.folder_info(ctx, path, &[]).await;
        let cfg = &info["config"];
        let q = &cfg["quantization_config"];
        // NVIDIA's ModelOpt checkpoints say "modelopt" as the method and the format in quant_algo
        // ("NVFP4"), or only per group: 4-bit floats in groups of 16 is NVFP4.
        let w = &q["config_groups"]["group_0"]["weights"];
        let groups = (w["num_bits"].as_u64() == Some(4) && w["type"].as_str() == Some("float")).then(|| "nvfp4".to_string());
        let quantization = q["quant_algo"].as_str().map(str::to_ascii_lowercase).or(groups).or_else(|| q["quant_method"].as_str().map(String::from));
        LibraryDetail {
            architectures: string_list(&cfg["architectures"]),
            model_type: cfg["model_type"].as_str().map(String::from),
            quantization,
            weight_bytes: needs.weight_bytes,
            kv_bytes_per_token: needs.kv_bytes_per_token,
            arch: needs.arch,
        }
    }

    /// A folder's config.json and size ({config, bytes}), read once per folder, from the first of
    /// `nodes` and then the online nodes that has it.
    async fn folder_info(&self, ctx: &Ctx, path: &str, nodes: &[String]) -> Value {
        let cached = self.configs.lock().unwrap().get(path).cloned();
        match cached {
            Some(v) => v,
            None if !ctx.live => {
                let v = demo::model_info(path);
                // The as-found world's GLM-5.3 folder is the owner's real one: its config is the
                // real config.json from Hugging Face (nvidia/GLM-5.3-Flash-NVFP4).
                if v["config"].as_object().is_some_and(|c| c.is_empty()) && path.trim_end_matches('/').ends_with("/GLM-5.3-Flash-NVFP4") {
                    let config: Value = serde_json::from_str(include_str!("fixtures/hf/glm-5.3-flash-nvfp4.config.json")).unwrap_or(Value::Null);
                    let bytes = self.demo.lock().unwrap().library().into_iter().find(|e| e.key == path).map(|e| e.size_bytes).unwrap_or(0);
                    return json!({ "config": config, "bytes": bytes });
                }
                v
            }
            None => {
                let listed = self.library_cache(ctx).and_then(|l| l.into_iter().find(|e| e.key == path).and_then(|e| e.copies.keys().next().cloned()));
                let mut ask: Vec<String> = nodes.iter().cloned().chain(listed).collect();
                ask.extend(ctx.online());
                ask.dedup();
                let cmd = format!("cat {}/config.json 2>/dev/null; echo; du -sbL {} 2>/dev/null | cut -f1", q(path), q(path));
                let mut seen = std::collections::HashSet::new();
                for node in ask.iter().filter(|n| seen.insert(n.as_str())) {
                    let Ok(out) = self.live.run(node, &cmd, None, Duration::from_secs(30)).await else { continue };
                    let mut lines: Vec<&str> = out.stdout.trim().lines().collect();
                    // No size line when du has nothing to say: the last line is then the config's.
                    let size = lines.last().and_then(|l| l.trim().parse::<u64>().ok());
                    if size.is_some() {
                        lines.pop();
                    }
                    let size = size.unwrap_or(0);
                    let config: Value = serde_json::from_str(&lines.join("\n")).unwrap_or(Value::Null);
                    if config.is_null() && size == 0 {
                        continue;
                    }
                    let v = json!({ "config": config, "bytes": size });
                    if size > 0 {
                        self.configs.lock().unwrap().insert(path.to_string(), v.clone());
                    }
                    return v;
                }
                Value::Null
            }
        }
    }

    /// The library as last listed, without scanning the nodes again.
    fn library_cache(&self, ctx: &Ctx) -> Option<Vec<LibraryEntry>> {
        if !ctx.live {
            return Some(self.demo.lock().unwrap().library());
        }
        None
    }

    /// The pre-flight the Load page shows before a load (P2.7, C-P2): every chosen node's memory,
    /// earlyoom, image, Docker store, model folder, ports, /dev/shm, cache folders, leftovers,
    /// Ollama and fabric facts, read in parallel and judged as one set. No ping, and nothing
    /// changes. Demo reads nothing.
    pub async fn preflight(&self, ctx: &Ctx, spec: &DeploySpec) -> Result<plan::PreflightSet, String> {
        if spec.replicas > 1 {
            let mut result = plan::PreflightSet { verdict: if ctx.live { "ready" } else { "demo" }.into(), checked_at_ms: now_ms(), ..Default::default() };
            for child in plan::replica_specs(spec)? {
                let set = Box::pin(self.preflight(ctx, &child)).await?;
                result.nodes.extend(set.nodes);
                result.problems.extend(set.problems);
            }
            if !result.problems.is_empty() { result.verdict = "blocked".into(); }
            return Ok(result);
        }
        if spec.nodes.is_empty() {
            return Err("Pick a node.".into());
        }
        let mut spec = spec.clone();
        pick_image(&mut spec, &ctx.settings);
        if !ctx.live {
            let nodes = spec.nodes.iter().map(|n| plan::Preflight { node: n.clone(), demo: true, image_ref: plan::image_ref(&spec).to_string(), ..Default::default() }).collect();
            return Ok(plan::PreflightSet::judge(&spec, nodes, now_ms()));
        }
        if let Some(n) = spec.nodes.iter().find(|n| !ctx.node_ids().contains(n)) {
            return Err(format!("Unknown node {n}"));
        }
        // Only what the read uses, so a half-filled Load page still gets its answer.
        plan::validate(&DeploySpec { tensor_parallel: spec.nodes.len() as u64, pipeline_parallel: 1, extra_args: Vec::new(), ..spec.clone() })?;
        let replaced = self.replaced(ctx, &spec);
        Ok(self.preflight_set(&spec, &ctx.settings, replaced.as_ref()).await)
    }

    /// The load an Edit & reload replaces: starting or serving under this id with the same head
    /// node. deploy() refuses an id that's still active (claim), so this only changes what the
    /// Load page reads.
    fn replaced(&self, ctx: &Ctx, spec: &DeploySpec) -> Option<Deployment> {
        self.deployments(ctx).into_iter().find(|d| d.spec.id == spec.id && d.state.active() && d.spec.nodes.first() == spec.nodes.first())
    }

    /// C-P2 on `node`: one read, with its activity-log line.
    async fn preflight_on(&self, node: &str, spec: &DeploySpec) -> Result<plan::Preflight, String> {
        let out = self.live.run(node, &plan::preflight_script(spec), None, Duration::from_secs(30)).await?;
        let p = plan::read_preflight(node, spec, &out.stdout, out.code);
        let key = (node.to_string(), p.image_ref.clone());
        match &p.image_id {
            Some(id) => self.image_ids.lock().unwrap().insert(key, id.clone()),
            None => self.image_ids.lock().unwrap().remove(&key),
        };
        Ok(p)
    }

    /// The chosen nodes as one set, read in parallel (C-P2): each node's read, its quick check for
    /// the fabric facts and, with 2+ nodes, its file list. Whatever stops a node from loading goes
    /// in the set's problems: an unreachable node is a problem, not an error, so one bad node
    /// doesn't hide the others. `replaced`: the load an Edit & reload replaces, whose port and share
    /// of memory come back at the unload (deploy reads the nodes again after that).
    async fn preflight_set(&self, spec: &DeploySpec, settings: &ModelSettings, replaced: Option<&Deployment>) -> plan::PreflightSet {
        // Version declarations must belong to the selected image, not the saved default.
        let check_ctx = Ctx { live: true, settings: ModelSettings { image: plan::image_ref(spec).into(), ..settings.clone() }, inventory: Inventory::example(), snapshot: None };
        let multi = spec.nodes.len() > 1;
        let list = format!("manifest {}", q(&spec.model_path));
        let reads = join_all(spec.nodes.iter().map(|n| {
            let (ctx, list) = (&check_ctx, &list);
            async move {
                let files = async {
                    match multi {
                        true => Some(self.sparkjob(n, list, None, Duration::from_secs(60)).await),
                        false => None,
                    }
                };
                futures_util::future::join3(self.preflight_on(n, spec), self.check_node(ctx, n, true), files).await
            }
        }))
        .await;
        let active: Vec<String> = self.deployments.lock().unwrap().iter().filter(|d| d.state.active()).map(|d| d.spec.id.clone()).collect();
        let mut nodes = Vec::new();
        let mut checks = Vec::new();
        let mut listings = Vec::new();
        for (node, (read, check, files)) in spec.nodes.iter().zip(reads) {
            let mut p = read.unwrap_or_else(|e| plan::Preflight { node: node.clone(), image_ref: plan::image_ref(spec).to_string(), problems: vec![format!("Couldn't read {node}: {}", words(e))], cut_off: true, ..Default::default() });
            match &check.error {
                Some(e) => p.problems.push(e.clone()),
                None if !check.docker => p.problems.push(format!("{node}: SparkBox can't use Docker (is sparkuser in the docker group?)")),
                None if multi && Self::net_of(node, &check.fabric).is_none() => p.problems.push(format!("{node} has no fabric address (ConnectX-7 with an IPv4 address).")),
                None => {}
            }
            p.fabric = check.fabric.clone();
            // Capability evidence is useful only for the immutable image being admitted.
            if let Some(parsers) = self.checks.lock().unwrap().iter().find(|c| c.node == *node && c.image_ref == p.image_ref)
                .and_then(|c| c.parsers.as_ref()).filter(|r| Some(&r.image_id) == p.image_id.as_ref()) {
                for (kind, chosen, supported) in [("tool-call", &spec.tool_call_parser, &parsers.tools), ("reasoning", &spec.reasoning_parser, &parsers.reasoning)] {
                    if let (Some(chosen), Some(supported)) = (chosen, supported) {
                        if !supported.contains(chosen) { p.problems.push(format!("{node}: the selected image does not list the {kind} parser {chosen}.")); }
                    }
                }
            }
            if check.error.is_none() {
                p.versions = check.versions.clone();
                // A tag moving between reads cannot attach its declaration to the earlier ID.
                if p.image_id.is_none() || p.versions.get("imageId").and_then(Option::as_ref) != p.image_id.as_ref() {
                    p.versions.remove("nccl");
                }
            }
            // A download shares the label, but it's a job at work: reported, and no reason not to
            // load beside it.
            let (downloads, labelled): (Vec<_>, Vec<_>) = std::mem::take(&mut p.leftovers).into_iter().partition(|l| l.name.starts_with(plan::DOWNLOAD_CONTAINER));
            p.downloads = downloads;
            p.leftovers = labelled;
            // Only what still runs here comes back when the replaced load stops.
            let holding = replaced.filter(|d| p.leftovers.iter().any(|l| l.ours && l.deployment == d.spec.id && l.status.starts_with("Up")));
            // A live load's containers aren't left over, nor the replaced load's, which its restart
            // stops; this build's others are, and block. The other build's and unlabelled ones are
            // reported and left alone.
            p.leftovers.retain(|l| !(l.ours && (active.contains(&l.deployment) || replaced.is_some_and(|d| d.spec.id == l.deployment))));
            let ours: Vec<&str> = p.leftovers.iter().filter(|l| l.ours).map(|l| l.name.as_str()).collect();
            if !ours.is_empty() {
                let block = plan::leftovers_block(node, &ours.join(", "));
                p.problems.push(block);
            }
            if let Some(d) = holding {
                plan::count_replaced(&mut p, spec, &d.spec.served_name, d.spec.port, d.spec.gpu_memory_utilization);
            }
            if check.error.is_none() {
                checks.push(check);
            }
            listings.push(match files {
                Some(Ok(v)) => Some(manifest_sizes(&v)),
                Some(Err(e)) => {
                    if p.model_present {
                        p.problems.push(format!("Couldn't list the model's files on {node}: {}", words(e)));
                    }
                    None
                }
                None => None,
            });
            nodes.push(p);
        }
        // Same files and sizes as the first node's copy. A node with no finished copy has said so,
        // and an empty listing isn't a copy that differs.
        let copies: Vec<Option<BTreeMap<String, u64>>> = listings.into_iter().zip(&nodes).map(|(l, p)| l.filter(|_| p.model_present)).collect();
        if let Some(first) = copies.iter().flatten().next() {
            for (p, listed) in nodes.iter_mut().zip(&copies) {
                if let Some((count, name)) = listed.as_ref().and_then(|l| files_differ(first, l)) {
                    let differ = plan::files_differ(&p.node, count, &name);
                    p.problems.push(differ);
                }
            }
        }
        let set = plan::PreflightSet::judge(spec, nodes, now_ms());
        self.keep_checks(checks);
        let mut saved = self.preflights.lock().unwrap();
        saved.insert(preflight_key(spec), set.clone());
        while saved.len() > PREFLIGHTS_KEPT {
            let Some(oldest) = saved.iter().min_by_key(|(_, s)| s.checked_at_ms).map(|(k, _)| k.clone()) else { break };
            saved.remove(&oldest);
        }
        save(&self.preflights_path, &*saved);
        set
    }

    /// One node's page-cache drop (C-D, `drop-caches`), as its own row.
    async fn drop_page_cache(&self, node: &str, id: &str) -> Result<(), String> {
        let op = audit::OpMeta { title: format!("Drop the page cache on {node} (vLLM counts it as used)"), source: "ui".into() };
        let drop = as_class(FenceClass::DropCaches, self.live.sudo(node, plan::DROP_CACHES, Duration::from_secs(120)));
        let out = audit::with_op_id(format!("drop-{id}-{node}-{}", now_ms()), op, drop).await?;
        if out.code != Some(0) {
            return Err(format!("{node}: couldn't drop the page cache: {}", last_line(&out.stderr)));
        }
        Ok(())
    }

    /// C-J, from each worker to the head on every half it has. A failed half blocks the load:
    /// NCCL_IB_HCA lists both.
    async fn jumbo_path(&self, set: &plan::PreflightSet) -> Result<(), String> {
        let Some((head, workers)) = set.nodes.split_first() else { return Ok(()) };
        let pings = join_all(workers.iter().map(|w| async move {
            let pairs = plan::jumbo_pairs(&w.fabric, &head.fabric);
            if pairs.is_empty() {
                let iface = w.fabric.first().map(|f| f.iface.clone()).unwrap_or_else(|| "its fabric".into());
                return Err(plan::jumbo_failed(&w.node, &head.node, &iface));
            }
            let out = self.live.run(&w.node, &plan::jumbo_script(&pairs), None, Duration::from_secs(20)).await?;
            match plan::jumbo_failures(&out.stdout, pairs.len()).first() {
                Some(half) => Err(plan::jumbo_failed(&w.node, &head.node, &pairs[*half].0)),
                None => Ok(()),
            }
        }))
        .await;
        pings.into_iter().collect()
    }

    /// Admin mode is open (no gate: the exit decides).
    fn admin_open(&self) -> bool {
        self.gate.lock().unwrap().as_ref().is_none_or(|g| g.state().unlocked)
    }

    /// Keep running drops the page cache only while Admin mode is unlocked.
    fn may_drop(&self, set: &plan::PreflightSet, why: Starting) -> Result<(), String> {
        let short: Vec<&str> = set.nodes.iter().filter(|p| p.drop_needed).map(|p| p.node.as_str()).collect();
        if !short.is_empty() && why == Starting::KeepRunning && !self.admin_open() && !cache_grant::covers(short.iter().copied()) {
            return Err(format!("{} needs its page cache dropped before this load can start again. Unlock Admin or explicitly grant Keep running permission in Settings › Security.", short.join(", ")));
        }
        Ok(())
    }

    /// Before a restart stops anything: the nodes as one set, what the load holds counted as coming
    /// back (`preflight_set`). What would keep it from starting again refuses it here, with the
    /// load as it was. Its memory is an estimate (`count_replaced`), and the read after the stop
    /// (`ready_to_load`) has the last word.
    async fn restart_blocked(&self, d: &Deployment, settings: &ModelSettings, why: Starting) -> Result<(), String> {
        let mut spec = d.spec.clone();
        if why == Starting::KeepRunning && cache_grant::covers(spec.nodes.iter().map(String::as_str)) { spec.drop_caches = true; }
        let set = self.preflight_set(&spec, settings, Some(d)).await;
        set_blocked(&set, &spec, why).and_then(|()| self.may_drop(&set, why)).map_err(|e| format!("{e} Nothing was stopped."))
    }

    /// At a start, before anything changes (P1.32, P1.34, P2.7): the nodes as one set, refused on
    /// any problem. Where MemFree is short (vLLM counts the page cache as used) the cache is dropped
    /// on the owner's one consent for the set and the nodes read again. The image's local id, each
    /// node's baseline for Unload and every rank's fabric identity come from that read. Then, for
    /// a new load, the jumbo path; never in look-only, which refuses the load at its first change.
    async fn ready_to_load(&self, spec: &mut DeploySpec, settings: &ModelSettings, why: Starting) -> Result<(Vec<Baseline>, Vec<NodeNet>), String> {
        if why == Starting::KeepRunning && cache_grant::covers(spec.nodes.iter().map(String::as_str)) { spec.drop_caches = true; }
        let mut set = self.preflight_set(spec, settings, None).await;
        set_blocked(&set, spec, why)?;
        self.may_drop(&set, why)?;
        let short: Vec<String> = set.nodes.iter().filter(|p| p.drop_needed).map(|p| p.node.clone()).collect();
        if !short.is_empty() {
            join_all(short.iter().map(|n| self.drop_page_cache(n, &spec.id))).await.into_iter().collect::<Result<Vec<()>, String>>()?;
            set = self.preflight_set(spec, settings, None).await;
            set_blocked(&set, spec, why)?;
            if let Some(p) = set.nodes.iter().find(|p| p.drop_needed) {
                return Err(plan::short_of_memory(p, spec.gpu_memory_utilization, true));
            }
        }
        spec.image_id = set.image_id.clone().ok_or_else(|| format!("Couldn't read the image's id on {}.", spec.nodes.join(", ")))?;
        let nets: Vec<NodeNet> = set.nodes.iter().filter_map(|p| Self::net_of(&p.node, &p.fabric)).collect();
        if spec.nodes.len() > 1 && why == Starting::Deploy && !self.hold_view().look_only {
            self.jumbo_path(&set).await?;
        }
        let baseline = set.nodes.into_iter().map(|p| Baseline { mem_available: p.mem.as_ref().map(|m| m.available).unwrap_or(0), node: p.node, shm: p.shm, at_ms: now_ms() }).collect();
        Ok((baseline, nets))
    }

    pub async fn deploy(self: &Arc<Self>, ctx: &Ctx, mut spec: DeploySpec) -> Result<Deployment, String> {
        crate::ops::rolling::check_load(&self.live, !ctx.live, &spec.nodes)?;
        if spec.id.is_empty() {
            spec.id = format!("m{}", short_id());
        }
        if spec.replicas > 1 {
            let children = plan::replica_specs(&spec)?;
            let p = self.plan(ctx, &spec).await?;
            if !p.topologies.iter().any(|o| o.ok && o.tp == spec.tensor_parallel && o.pp == spec.pipeline_parallel && o.replicas == spec.replicas) {
                return Err("This model and recipe do not offer that replica layout.".into());
            }
            // Read every replica before starting any. Each child repeats admission and keeps its
            // own ownership, credentials, failure and unload boundary if a later child fails.
            for child in &children {
                claim(&self.deployments(ctx), child)?;
                let set = self.preflight(ctx, child).await?;
                set_blocked(&set, child, Starting::Deploy)?;
            }
            let mut first = None;
            for (i, child) in children.into_iter().enumerate() {
                match Box::pin(self.deploy(ctx, child)).await {
                    Ok(d) => { if first.is_none() { first = Some(d); } }
                    Err(e) => return Err(format!("Replica {} could not start: {e}. {i} earlier replicas were admitted independently; review them in Loaded.", i + 1)),
                }
            }
            return first.ok_or_else(|| "No replicas selected.".into());
        }
        pick_image(&mut spec, &ctx.settings);
        plan::validate(&spec)?;
        let unknown: Vec<&String> = spec.nodes.iter().filter(|n| !ctx.node_ids().contains(n)).collect();
        if !unknown.is_empty() {
            return Err(format!("Unknown node {}", unknown[0]));
        }
        // The same pre-flight the Load page shows: the port is free and the memory plan isn't red.
        spec.host = self.resolve_host(ctx, &spec);
        if spec.listen == "fabric" && spec.host.is_empty() {
            return Err(format!("{} has no fabric address; run Prepare cluster in Models.", spec.nodes[0]));
        }
        let p = self.plan(ctx, &spec).await?;
        // What the model can't take says more than what a recipe doesn't list, as the Load page's
        // offers do.
        if plan::is_native(&spec) && !recipe::recipe_for(&spec.model_path, None).placeholder() {
            plan::validate_topology(&spec, &p.needs.arch)?;
        }
        recipe::apply(&mut spec)?;
        // A vLLM load's port is read on the node itself (ready_to_load): the snapshot's listeners
        // can be 5 s old, and a reload's own port reads as taken right after its unload.
        let head = ctx.snapshot.as_ref().and_then(|s| s.nodes.iter().find(|n| n.id == spec.nodes[0])).filter(|_| !(ctx.live && plan::is_native(&spec)));
        plan::preflight(&spec, &p.nodes, head).map_err(|e| e.to_string())?;
        let op = audit::OpMeta { title: format!("Load {} on {}", spec.served_name, spec.nodes.join(", ")), source: "ui".into() };
        let op_id = format!("load-{}-{}", spec.id, now_ms());
        // The nodes themselves say what they have, before anything changes.
        let (baseline, nets) = if ctx.live && plan::is_native(&spec) {
            claim(&self.deployments.lock().unwrap(), &spec)?;
            audit::with_op_id(op_id.clone(), op.clone(), self.ready_to_load(&mut spec, &ctx.settings, Starting::Deploy)).await?
        } else {
            (Vec::new(), Vec::new())
        };
        let reach = if spec.host.is_empty() { ctx.lan_address(&spec.nodes[0]) } else { spec.host.clone() };
        let dep = Deployment {
            endpoint: format!("http://{reach}:{}/v1", spec.port),
            spec,
            state: DeploymentState::Starting,
            stage: Some(Stage::Starting),
            message: "Checking the nodes…".into(),
            log_tail: String::new(),
            started_at: now(),
            provider_id: String::new(),
            demo: !ctx.live,
            generation: 0,
            baseline,
            first_start: true,
            ..Default::default()
        };
        if !ctx.live {
            crate::ops::rolling::check_load(&self.live, true, &dep.spec.nodes)?;
            self.demo.lock().unwrap().claim(dep.clone())?;
            let s = &dep.spec;
            self.live.audit.record_simulated(&s.nodes.join(","), &format!("Load {} with vLLM", s.served_name), &plan::vllm_args(s).join(" "));
            return Ok(dep);
        }
        // Check and insert under one lock, so two loads can't take the same nodes or port. A load
        // reusing an id gets a newer generation: the old one's watcher stands down.
        let mut dep = dep;
        {
            let mut all = self.deployments.lock().unwrap();
            crate::ops::rolling::check_load(&self.live, false, &dep.spec.nodes)?;
            claim(&all, &dep.spec)?;
            dep.generation = all.iter().filter(|d| d.spec.id == dep.spec.id).map(|d| d.generation + 1).max().unwrap_or(0);
            let mut next = all.clone();
            next.retain(|d| d.spec.id != dep.spec.id);
            next.push(dep.clone());
            self.deployment_history.lock().unwrap().save(&next)?;
            *all = next;
        }
        let key = if dep.spec.require_key {
            let k = format!("sbk-{}", token());
            if let Err(e) = secrets::set(&plan::key_account(&dep.spec.id), &k) {
                self.update(&dep.spec.id, |d| {
                    d.state = DeploymentState::Failed;
                    d.message = format!("Couldn't save the API key in the Keychain: {e}");
                });
                return Err(e);
            }
            Some(k)
        } else {
            None
        };
        let me = self.clone();
        let settings = ctx.settings.clone();
        let spec = dep.spec.clone();
        tauri::async_runtime::spawn(audit::with_op_id(op_id, op, async move { me.run_deploy(&spec, &settings, &nets, key).await }));
        Ok(dep)
    }

    /// A new load's start, then following it. However it ends is on its deployment.
    async fn run_deploy(self: Arc<Self>, spec: &DeploySpec, settings: &ModelSettings, nets: &[NodeNet], key: Option<String>) {
        let id = &spec.id;
        let generation = self.deployments.lock().unwrap().iter().find(|d| d.spec.id == *id).map(|d| d.generation).unwrap_or(0);
        if !plan::is_native(spec) {
            if let Err(e) = self.clone().run_sglang(spec, settings, generation).await {
                self.fail(id, e);
            }
            return;
        }
        // Until its last docker run has answered, an Unload waits (start_lock): it then finds the
        // containers they made.
        let lock = self.start_lock(id);
        let starting = lock.lock().await;
        // Unloaded while it was being prepared: nothing more starts.
        if !self.current(id, generation) {
            return;
        }
        self.update(id, |d| d.message = format!("Starting {}…", engine_label(spec)));
        let started = self.start_ranks(spec, nets, key.as_deref(), &Steps { task: None }).await;
        drop(starting);
        if let Err(failed) = started {
            return self.fail_start(spec, failed, generation).await;
        }
        let head = plan::rank_container(id, 0, spec.nodes.len());
        if let Err(why) = self.clone().follow(spec, &head, generation).await {
            self.fail_start(spec, StartFailed::on(why, &spec.nodes), generation).await;
        }
    }

    /// A rank's launch files (C-F): `deploy-files` with its argv, env and, on rank 0, the key on
    /// stdin, then launch.py. What it's given goes in the activity log, the key never: the fence
    /// only sees `deploy-files <id> <role>`. launch.py isn't a read-only helper: root runs it in the
    /// container. So it goes only once the exit has admitted this load's files, its checksum is read
    /// for every load (the two builds share the file), and a copy gets its own row.
    async fn write_rank_files(&self, spec: &DeploySpec, rank: usize, nets: &[NodeNet], key: Option<&str>, class: FenceClass) -> Result<(), String> {
        let node = &spec.nodes[rank];
        let payload = plan::launch_payload(spec, rank, nets, key).to_string();
        let args = format!("deploy-files {} {}", spec.id, plan::role_of(spec, rank));
        as_class(class, self.sparkjob(node, &args, Some(payload.as_bytes()), Duration::from_secs(30))).await?;
        if self.live.refresh_file(node, &plan::launcher_name(), plan::LAUNCH).await? {
            self.live.audit.record(audit::Entry {
                node: node.clone(),
                kind: "app".into(),
                title: format!("Copy launch.py to {node}"),
                command: format!("~/.sparkbox/{} sha256 {}", plan::launcher_name(), crate::live::sha256_hex(plan::LAUNCH.as_bytes())),
                state: "ok".into(),
                ..Default::default()
            });
        }
        let files = plan::launch_payload(spec, rank, nets, None);
        self.live.audit.record(audit::Entry {
            node: node.clone(),
            kind: "app".into(),
            title: if spec.nodes.len() == 1 { format!("Launch files for vLLM on {node}") } else { format!("Launch files for the {} on {node}", rank_words(spec, rank)) },
            command: json!({ "argv": files["argv"], "env": files["env"] }).to_string(),
            state: "ok".into(),
            ..Default::default()
        });
        Ok(())
    }

    /// One rank's `docker run` (C-L2). Err is why, with what Docker wrote when it answered.
    async fn start_rank(&self, spec: &DeploySpec, rank: usize, class: FenceClass) -> Result<(), (String, Option<String>)> {
        let node = &spec.nodes[rank];
        let limit = *self.run_limit.lock().unwrap();
        let run = as_class(class, self.live.run(node, &plan::rank_command(spec, rank), None, limit)).await;
        // `live.run`'s words for a limit are the start of the command, cut off: no use to anyone.
        let out = run.map_err(|e| (if e.contains("took longer than") { format!("{node}: docker run didn't answer within {} s. Check Containers on {node}.", limit.as_secs()) } else { e }, None))?;
        if out.code != Some(0) {
            return Err((docker_run_failed(node, &out.stderr), Some(out.stderr)));
        }
        Ok(())
    }

    /// The start (C-O): every node's launch files in parallel, then all the workers' `docker run`
    /// in parallel, then the head, which waits for no one. Any failure stops there, so a worker that
    /// can't start never gets a head.
    async fn start_ranks(&self, spec: &DeploySpec, nets: &[NodeNet], key: Option<&str>, steps: &Steps<'_>) -> Result<(), StartFailed> {
        let n = spec.nodes.len();
        let class = if n == 1 { FenceClass::ModelLoadOne } else { FenceClass::ModelLoad };
        let written = join_all((0..n).map(|rank| self.write_rank_files(spec, rank, nets, key, class))).await;
        // A refusal changed nothing there; anything else may have.
        let touched: Vec<String> = spec.nodes.iter().zip(&written).filter(|(_, w)| !w.as_ref().is_err_and(|e| refused(e))).map(|(n, _)| n.clone()).collect();
        if let Some(e) = written.into_iter().find_map(Result::err) {
            return Err(StartFailed::on(e, &touched));
        }
        // What `docker run` said, on every rank it failed on.
        let failed = |runs: Vec<(usize, Result<(), (String, Option<String>)>)>| {
            let failures: Vec<(usize, (String, Option<String>))> = runs.into_iter().filter_map(|(rank, run)| run.err().map(|e| (rank, e))).collect();
            let why = failures.first().map(|(_, (why, _))| why.clone())?;
            let said = failures.into_iter().filter_map(|(rank, (_, said))| said.map(|s| (rank, s))).collect();
            Some(StartFailed { why, touched: touched.clone(), said })
        };
        if n > 1 {
            steps.step(format!("Start the workers on {}", spec.nodes[1..].join(", ")));
            let runs = join_all((1..n).map(|rank| self.start_rank(spec, rank, class))).await;
            steps.started();
            if let Some(f) = failed((1..n).zip(runs).collect()) {
                return Err(f);
            }
            steps.ok(None);
        }
        let head = &spec.nodes[0];
        steps.step(if n > 1 { format!("Start the head on {head}") } else { format!("Start the server on {head}") });
        if let Some(f) = failed(vec![(0, self.start_rank(spec, 0, class).await)]) {
            return Err(f);
        }
        steps.started();
        steps.ok(None);
        Ok(())
    }

    /// A start that failed (C-X), once anything may be on a node. A load that was Ready isn't a
    /// start failure and keeps its containers, and a start that touched nothing has nothing to
    /// undo. Nothing here retries.
    async fn fail_start(self: &Arc<Self>, spec: &DeploySpec, failed: StartFailed, generation: u64) {
        let id = &spec.id;
        let StartFailed { why, touched, said } = failed;
        let why = audit::redact(&words(why));
        // An Unload pressed now waits, and finds what's left.
        let lock = self.start_lock(id);
        let _starting = lock.lock().await;
        if !self.current(id, generation) {
            return;
        }
        let was_ready = self.deployments.lock().unwrap().iter().any(|d| d.spec.id == *id && d.state == DeploymentState::Ready);
        if was_ready || !plan::is_native(spec) {
            return self.fail(id, why);
        }
        if touched.is_empty() {
            self.fail(id, why);
            if self.first_start(id) {
                let _ = secrets::clear(&plan::key_account(id));
            }
            return;
        }
        let op = audit::OpMeta { title: format!("Clean up after {} failed to start", spec.served_name), source: "system".into() };
        audit::with_op_id(format!("rollback-{id}-{}", now_ms()), op, self.roll_back(spec, why, &touched, &said)).await;
    }

    fn first_start(&self, id: &str) -> bool {
        self.deployments.lock().unwrap().iter().any(|d| d.spec.id == id && d.first_start)
    }

    /// C-X: each rank's state and log, saved on this Mac before anything removes them; then the
    /// unload, head first; then /dev/shm against the baseline. What look-only or a node out of reach
    /// held back is named, and Unload removes it. The load ends Failed either way. `said`: what
    /// `docker run` wrote where it failed, which that rank's container has no log of.
    async fn roll_back(self: &Arc<Self>, spec: &DeploySpec, why: String, touched: &[String], said: &[(usize, String)]) {
        let id = &spec.id;
        let n = spec.nodes.len();
        let at_ms = now_ms();
        let dir = self.failed_logs.join(format!("{id}-{at_ms}"));
        let reads = join_all((0..n).map(|rank| async move {
            let node = &spec.nodes[rank];
            if !touched.contains(node) {
                return None;
            }
            let cmd = rank_state_command(&plan::rank_container(id, rank, n), Some(600));
            Some(audit::as_probe(self.live.run(node, &cmd, None, Duration::from_secs(30))).await)
        }))
        .await;
        let mut ranks = Vec::new();
        for (rank, read) in reads.into_iter().enumerate() {
            let (state, log) = match read {
                None => ("missing 0".to_string(), "This rank never started.".to_string()),
                Some(Err(e)) => ("missing 0".to_string(), format!("SparkBox couldn't read this rank's log: {}", words(e))),
                Some(Ok(o)) => match o.stdout.split_once('\n').unwrap_or((o.stdout.as_str(), "")) {
                    (state, _) if state.starts_with("missing") => (state.to_string(), "This rank's container was never made.".to_string()),
                    (state, log) => (state.to_string(), log.to_string()),
                },
            };
            let log = match said.iter().find(|(r, _)| *r == rank) {
                Some((_, stderr)) => format!("docker run said:\n{}\n{log}", stderr.trim_end()),
                None => log,
            };
            let log = audit::redact(&log);
            if let Err(e) = save_rank_log(&dir, rank, &log) {
                eprintln!("rollback {id}: couldn't save rank {rank}'s log: {e}");
            }
            let tail = last_lines(&log, TAIL_LINES);
            ranks.push(RankEnd { node: spec.nodes[rank].clone(), rank, role: plan::role_of(spec, rank).into(), exit: rank_state(&state).1, tail });
        }

        // Unload as C-O: the head first, then the others in parallel.
        let cmd = plan::unload_command(id);
        let unload = |node: String| {
            let cmd = &cmd;
            async move {
                let done = as_class(FenceClass::ModelUnload, self.live.run(&node, cmd, None, Duration::from_secs(120))).await;
                (node, done.is_ok_and(|o| o.code == Some(0)))
            }
        };
        let mut done = Vec::new();
        if touched.contains(&spec.nodes[0]) {
            done.push(unload(spec.nodes[0].clone()).await);
        }
        done.extend(join_all(touched.iter().filter(|n| **n != spec.nodes[0]).cloned().map(unload)).await);
        let cleaned: Vec<String> = done.iter().filter(|(_, ok)| *ok).map(|(n, _)| n.clone()).collect();
        let remaining = self.load_containers(&cleaned, id).await;
        let holds = |n: &String| remaining.iter().any(|(node, c)| node == n && c.as_ref().is_none_or(|c| !c.mine.is_empty()));
        let left_on: Vec<String> = done.iter().filter(|(n, ok)| !ok || holds(n)).map(|(n, _)| n.clone()).collect();
        let clear: Vec<String> = cleaned.iter().filter(|n| !left_on.contains(n)).cloned().collect();

        let baseline = self.deployments.lock().unwrap().iter().find(|d| d.spec.id == *id).map(|d| d.baseline.clone()).unwrap_or_default();
        let shm = self.new_in_shm(&clear, &baseline).await;

        let mut message = why.clone();
        if !left_on.is_empty() {
            message.push_str(&format!(" Its containers and files are still on {}; Unload or Forget removes them.", left_on.join(", ")));
        }
        if !shm.is_empty() {
            message.push_str(&format!(" New in /dev/shm since the load: {}.", shm.join("; ")));
        }
        // A worker that failed, and the head, which is all a load's log tail has room for.
        let failing = said.iter().map(|(r, _)| *r).find(|r| *r > 0).or(ranks.iter().find(|r| r.rank > 0 && r.exit.is_some_and(|c| c != 0)).map(|r| r.rank));
        let shown: Vec<&RankEnd> = ranks.iter().filter(|r| failing == Some(r.rank) || r.rank == 0).collect();
        let log_tail = shown.iter().map(|r| format!("── {}, {} ──\n{}", r.node, rank_words(spec, r.rank), r.tail)).collect::<Vec<_>>().join("\n");

        // Chat and the Server stop offering it. A restart's key stays for Keep running; a first
        // start's goes with it.
        let provider = self.deployments.lock().unwrap().iter().find(|d| d.spec.id == *id).map(|d| d.provider_id.clone()).unwrap_or_default();
        let account = plan::key_account(id);
        let first = self.first_start(id);
        let keep = if first { None } else { secrets::get(&account).ok().flatten() };
        if !provider.is_empty() {
            let _ = self.llm.remove(&provider).await;
        }
        match keep {
            Some(k) => {
                let _ = secrets::set(&account, &k);
            }
            None if first => {
                let _ = secrets::clear(&account);
            }
            None => {}
        }
        let log_dir = dir.display().to_string();
        self.update(id, |d| {
            d.state = DeploymentState::Failed;
            d.message = message;
            d.log_tail = log_tail;
            d.provider_id.clear();
            d.failure = Some(FailureRecord { at_ms, cause: why, ranks, log_dir, left_on });
        });
    }

    /// What's in /dev/shm on `nodes` that their baseline didn't have, as "cell2: vllm-1234".
    async fn new_in_shm(&self, nodes: &[String], baseline: &[Baseline]) -> Vec<String> {
        let lists = join_all(nodes.iter().map(|n| self.live.run(n, "ls -la /dev/shm", None, Duration::from_secs(15)))).await;
        let mut found = Vec::new();
        for (node, list) in nodes.iter().zip(lists) {
            let (Some(base), Ok(out)) = (baseline.iter().find(|b| b.node == *node), list) else { continue };
            let new: Vec<String> = plan::shm_listing(&out.stdout.lines().collect::<Vec<_>>()).into_iter().map(|(name, _)| name).filter(|name| !base.shm.contains(name)).collect();
            if !new.is_empty() {
                found.push(format!("{node}: {}", new.join(", ")));
            }
        }
        found
    }

    /// An SGLang load's start, which keeps its own path: head first, the others join it.
    async fn run_sglang(self: Arc<Self>, spec: &DeploySpec, settings: &ModelSettings, generation: u64) -> Result<(), String> {
        let id = spec.id.clone();
        let nets = self.prepare(spec, settings).await?;

        // 3. Clear leftovers (an unload of this id), make sure the port is still free, then start
        // the containers (head first).
        for n in &spec.nodes {
            let _ = as_class(FenceClass::ModelUnload, self.live.run(n, &plan::unload_command(&id), None, Duration::from_secs(120))).await;
        }
        let head = spec.nodes[0].clone();
        let taken = self.live.run(&head, &plan::port_check_command(spec.port), None, Duration::from_secs(15)).await?;
        if !taken.stdout.trim().is_empty() {
            let e = plan::PreflightError::PortInUse { node: head.clone(), port: spec.port, by: last_line(&taken.stdout) };
            return Err(e.to_string());
        }
        // From the check to the end of the docker runs, an Unload waits (start_lock): it then
        // finds the containers they made.
        let lock = self.start_lock(&id);
        let starting = lock.lock().await;
        // Unloaded while it was being prepared: nothing more starts.
        if !self.current(&id, generation) {
            return Ok(());
        }
        self.update(&id, |d| d.message = format!("Starting {}…", engine_label(spec)));
        let head_container = self.start_head(spec, &nets).await?;
        self.start_workers(spec, &nets).await?;
        drop(starting);
        self.follow(spec, &head_container, generation).await
    }

    /// 1–2 of an SGLang load: every node reachable, with the model and a fabric address; the image
    /// ready on each.
    async fn prepare(&self, spec: &DeploySpec, settings: &ModelSettings) -> Result<Vec<NodeNet>, String> {
        let id = spec.id.clone();
        let say = |msg: &str| self.update(&id, |d| d.message = msg.into());
        let check_ctx = Ctx { live: true, settings: settings.clone(), inventory: Inventory::example(), snapshot: None };
        let checks = join_all(spec.nodes.iter().map(|n| self.check_node(&check_ctx, n, true))).await;
        let mut nets = Vec::new();
        for c in &checks {
            if let Some(e) = &c.error {
                return Err(e.clone());
            }
            if !c.docker {
                return Err(format!("{}: SparkBox can't use Docker (is sparkuser in the docker group?)", c.node));
            }
            let has = self.live.run(&c.node, &plan::model_ready_test(&spec.model_path), None, Duration::from_secs(15)).await?;
            if has.code != Some(0) {
                return Err(plan::not_on_node(&c.node, &spec.model_path));
            }
            match Self::net_of(&c.node, &c.fabric) {
                Some(n) => nets.push(n),
                None if spec.nodes.len() == 1 => {}
                None => return Err(format!("{} has no fabric address (ConnectX-7 with an IPv4 address).", c.node)),
            }
        }
        // The same image on every node (the first time pulls ~20 GB).
        say(&format!("Preparing the {} image on each node (the first time downloads it)…", engine_label(spec)));
        let script = plan::pull_script(&spec.image);
        let builds = join_all(spec.nodes.iter().map(|n| self.live.run(n, &script, None, Duration::from_secs(90 * 60)))).await;
        for (n, b) in spec.nodes.iter().zip(builds) {
            let b = b?;
            if b.code != Some(0) {
                return Err(format!("{n}: couldn't prepare the {} image: {}", engine_label(spec), last_line(&b.stderr)));
            }
        }
        Ok(nets)
    }

    /// Starts SGLang's rank 0 (it takes no key: `validate` refuses one); returns its name.
    async fn start_head(&self, spec: &DeploySpec, nets: &[NodeNet]) -> Result<String, String> {
        let head = spec.nodes[0].clone();
        let single = spec.nodes.len() == 1;
        let cmd = plan::sglang_command(spec, 0, nets.first().map(|n| n.ip.as_str()).unwrap_or("127.0.0.1"), nets.first());
        let out = self.live.run(&head, &cmd, None, Duration::from_secs(60)).await?;
        if out.code != Some(0) {
            return Err(docker_run_failed(&head, &out.stderr));
        }
        Ok(plan::container_name(&spec.id, &head, if single { "single" } else { "head" }))
    }

    /// Starts SGLang's other ranks, which join rank 0 on port 5000.
    async fn start_workers(&self, spec: &DeploySpec, nets: &[NodeNet]) -> Result<(), String> {
        let Some(head_net) = nets.first() else { return Ok(()) };
        for (i, net) in nets.iter().enumerate().skip(1) {
            let cmd = plan::sglang_command(spec, i, &head_net.ip, Some(net));
            let out = self.live.run(&net.node_id, &cmd, None, Duration::from_secs(60)).await?;
            if out.code != Some(0) {
                return Err(docker_run_failed(&net.node_id, &out.stderr));
            }
        }
        Ok(())
    }

    /// How a starting load's workers are: how many ranks run, the head counted. A rank whose node
    /// can't be asked counts as still running; one that's gone is why the load failed (C-X).
    async fn workers_up(&self, spec: &DeploySpec) -> Result<u64, String> {
        let n = spec.nodes.len();
        let asks = join_all((1..n).map(|rank| async move {
            let cmd = rank_state_command(&plan::rank_container(&spec.id, rank, n), None);
            self.poll(&spec.nodes[rank], &cmd, Duration::from_secs(20)).await
        }))
        .await;
        let mut up = 1;
        for (rank, ask) in (1..n).zip(asks) {
            let Ok(out) = ask else {
                up += 1;
                continue;
            };
            let state = out.lines().next().unwrap_or("missing 0");
            match rank_state(state) {
                (true, _) => up += 1,
                (false, code) => {
                    let how = code.map(|c| format!("exit {c}")).unwrap_or_else(|| "its container is gone".into());
                    return Err(format!("The {} on {} stopped while the load was starting ({how}).", rank_words(spec, rank), spec.nodes[rank]));
                }
            }
        }
        Ok(up)
    }

    /// C-T: what the head's log says about its traffic, kept on the load and, as the read's own
    /// row, in the activity log. Only the log's words decide. A load off the fabric carries on,
    /// with the warning in its message.
    async fn read_transport(&self, spec: &DeploySpec) {
        let head = &spec.nodes[0];
        let container = plan::rank_container(&spec.id, 0, spec.nodes.len());
        let Ok(out) = self.live.run(head, &plan::transport_command(&container), None, Duration::from_secs(20)).await else { return };
        let t = plan::transport_from_logs(&out.stdout);
        let fabric: Vec<String> = self.checks.lock().unwrap().iter().find(|c| c.node == *head).map(|c| c.fabric.iter().map(|f| f.ip.clone()).collect()).unwrap_or_default();
        let warning = plan::transport_warning(&t, &fabric);
        self.update(&spec.id, |d| {
            if let Some(w) = warning.as_ref().filter(|w| !d.message.contains(w.as_str())) {
                d.message = format!("{} {w}", d.message);
            }
            d.transport = Some(t);
            d.transport_warning = warning;
        });
    }

    /// 4. Follows startup until it answers (the readiness gate), then keeps watching it.
    async fn follow(self: Arc<Self>, spec: &DeploySpec, head_container: &str, generation: u64) -> Result<(), String> {
        let id = spec.id.clone();
        let head = spec.nodes[0].clone();
        // Moved on by the time look-only holds the test token, which doesn't count.
        let mut started = Instant::now();
        let mut last = started;
        let mut away: Option<Instant> = None;
        let mut checking = false;
        let several = plan::is_native(spec) && spec.nodes.len() > 1;
        let need = spec.nodes.len() as u64;
        let mut workers_asked: Option<Instant> = None;
        let mut up = 1;
        let mut transport_read = false;
        let probe = plan::probe_command(head_container, plan::probe_host(spec), spec.port);
        loop {
            let now_state = self.deployments.lock().unwrap().iter().find(|d| d.spec.id == id).map(|d| (d.state, d.generation));
            let state = now_state.map(|(s, _)| s);
            let ready = state == Some(DeploymentState::Ready);
            if !state.is_some_and(|s| s.active()) || now_state.is_some_and(|(_, g)| g != generation) {
                return Ok(()); // unloaded, or restarted (a newer watcher has it)
            }
            tokio::time::sleep(FOLLOW_POLL[if ready { 2 } else if checking { 1 } else { 0 }]).await;
            let out = self.poll(&head, &probe, Duration::from_secs(20)).await;
            let since = last.elapsed();
            last = Instant::now();
            // Unloaded or restarted while it slept or asked: what it saw is that unload's doing.
            if !self.current(&id, generation) {
                return Ok(());
            }
            // A node out of reach ends nothing: the load goes on there, and it's asked again.
            let Ok(out) = out else {
                let within = *self.ready_within.lock().unwrap();
                let lost = away.get_or_insert_with(Instant::now).elapsed() >= *self.lost_grace.lock().unwrap();
                if !ready && lost && started.elapsed() > within {
                    self.update(&id, |d| d.recoverable = false);
                    return Err(not_ready_within(spec, within));
                }
                continue;
            };
            away = None;
            let (state, http, logs) = probe_parts(&out);
            let logs = audit::redact(&logs);
            let restarts = restart_count(&state);
            if !ready && restarts > 0 {
                return Err(started_again(spec, &head, restarts, &logs));
            }
            if !state.starts_with("running") {
                let code = state.split_whitespace().nth(1).unwrap_or("?").to_string();
                // launch.py said why, in one line.
                if code == plan::KEY_REFUSED.to_string() {
                    if let Some(line) = logs.lines().rev().find_map(|l| l.strip_prefix("sparkbox: ")) {
                        return Err(format!("{head} (exit {code}): {line}"));
                    }
                }
                let own = format!("{} stopped on {head} (exit {code}). Last log lines:\n{}", engine_label(spec), last_lines(&logs, 8));
                // A head exits when it loses a rank: what's wrong may be that rank.
                if several && !ready {
                    if let Err(worker) = self.workers_up(spec).await {
                        return Err(format!("{worker} {own}"));
                    }
                }
                return Err(own);
            }
            // While it starts, its workers are asked too: one that's gone never comes back by itself.
            if several && !ready && workers_asked.is_none_or(|t| t.elapsed() >= WORKER_POLL) {
                workers_asked = Some(Instant::now());
                let asked = self.workers_up(spec).await;
                if !self.current(&id, generation) {
                    return Ok(());
                }
                up = asked?;
            }
            let mut stage = plan::stage_from_logs(&logs, http == "200");
            if several && stage == Stage::Starting {
                stage = Stage::Joining { have: up, need };
            }
            if several && !transport_read && matches!(stage, Stage::Loading { .. }) {
                transport_read = true;
                self.read_transport(spec).await;
            }
            self.update(&id, |d| {
                d.stage = Some(stage.clone());
                d.log_tail = logs.clone();
                if !ready && stage != Stage::Ready {
                    let warning = d.transport_warning.as_ref().map(|w| format!(" {w}")).unwrap_or_default();
                    d.message = format!("{}{warning}", stage_message(&stage));
                }
            });
            // Up by its log or /health: Ready only once it answers.
            let mut answered = false;
            let mut held = false;
            if stage == Stage::Ready && !ready {
                checking = true;
                answered = self.gate(spec, &http, generation).await?;
                held = !answered && self.deployments.lock().unwrap().iter().any(|d| d.spec.id == id && d.readiness.as_ref().is_some_and(|r| r.why.as_deref() == Some(readiness::HELD)));
            }
            if ready && http != "200" {
                self.update(&id, |d| d.message = format!("{} isn't answering /health right now.", d.endpoint));
            }
            if held {
                started += since;
            }
            let within = *self.ready_within.lock().unwrap();
            // Ready at this poll counts, however late it came.
            if !ready && !answered && started.elapsed() > within {
                self.update(&id, |d| d.recoverable = false);
                return Err(not_ready_within(spec, within));
            }
        }
    }

    /// The readiness gate (P1.35): a load whose log or /health says it's up is Ready only when it
    /// answers (readiness::check): /health 200, its name in /v1/models, a test token with its
    /// key, and 17+25 = 42 the first time this recipe meets this image. With a key it must also
    /// refuse a caller without one: vLLM serving open means the key never reached it. Ready: the
    /// verdict is kept, it's put in Chat and the Server, then its native APIs are read (P1.37) and
    /// the recipe marked verified. A wrong answer, or serving open, fails the load (C-X). Anything
    /// else keeps it starting with the reason, asked again at the next poll. Ok(true) once Ready.
    async fn gate(&self, spec: &DeploySpec, http: &str, generation: u64) -> Result<bool, String> {
        let id = &spec.id;
        let head = &spec.nodes[0];
        let host = plan::probe_host(spec);
        let key = if spec.require_key { secrets::get(&plan::key_account(id)).ok().flatten() } else { None };
        let recipe = readiness::recipe_key(spec);
        let health = Some(http.parse().unwrap_or(0));
        let r = readiness::check_with(&self.live, head, host, spec.port, &spec.served_name, key.as_deref(), self.sanity.needs(&recipe), health).await;
        if let Some(s) = &r.sanity {
            self.sanity.record(&recipe, s);
        }
        let current = || self.deployments.lock().unwrap().iter().any(|d| &d.spec.id == id && d.state == DeploymentState::Starting && d.generation == generation);
        if !current() {
            return Ok(false); // unloaded or restarted while it was asked
        }
        if r.wrong() {
            let why = r.why.clone().unwrap_or_default();
            self.update(id, |d| {
                d.readiness = Some(r);
                d.recoverable = false;
            });
            return Err(why);
        }
        if !r.ok {
            let why = r.why.clone().unwrap_or_default();
            self.update(id, |d| {
                d.message = format!("Up; checking it answers… ({why})");
                d.readiness = Some(r);
            });
            return Ok(false);
        }
        if spec.require_key {
            let cmd = readiness::models_command(host, spec.port, false);
            let asked = audit::as_probe(self.live.run(head, &cmd, None, Duration::from_secs(20))).await;
            match asked.as_ref().map(|o| readiness::parse_models(&o.stdout, &spec.served_name)) {
                Ok(readiness::Listed::Refused(_)) => {}
                Ok(readiness::Listed::Yes | readiness::Listed::No) => {
                    let why = format!("{} answered without its API key, so it was stopped rather than left open.", engine_label(spec));
                    self.update(id, |d| {
                        d.readiness = Some(r);
                        d.recoverable = false;
                    });
                    return Err(why);
                }
                Ok(readiness::Listed::Failed(_)) | Err(_) => {
                    self.update(id, |d| d.message = "Up; checking it asks for its key…".into());
                    return Ok(false);
                }
            }
        }
        // In Chat and the Server by the time the load reads as ready.
        self.register(id).await;
        #[cfg(test)]
        {
            let hold = self.after_register.lock().unwrap().take();
            if let Some((reached, go)) = hold {
                let _ = reached.send(());
                let _ = go.await;
            }
        }
        // An unload or a restart that landed meanwhile stands: this load isn't Ready, and what
        // register just added goes, unless a newer start of this id is Ready and owns it.
        let mut ready = false;
        let mut restarting = false;
        let mut leftover = String::new();
        self.update(id, |d| {
            if d.state == DeploymentState::Starting && d.generation == generation {
                d.state = DeploymentState::Ready;
                d.stage = Some(Stage::Ready);
                d.message = format!("Serving at {}", d.endpoint);
                d.readiness = Some(r);
                d.recoverable = true;
                ready = true;
            } else if d.state != DeploymentState::Ready {
                restarting = d.state == DeploymentState::Starting;
                leftover = std::mem::take(&mut d.provider_id);
            }
        });
        if !ready {
            if !leftover.is_empty() {
                // The provider's key is the load's: a newer start still asks with it.
                let account = plan::key_account(id);
                let keep = if restarting { secrets::get(&account).ok().flatten() } else { None };
                let _ = self.llm.remove(&leftover).await;
                if let Some(k) = keep {
                    let _ = secrets::set(&account, &k);
                }
            }
            return Ok(false);
        }
        // Recipes are vLLM's: an SGLang load has none to verify.
        if plan::is_native(spec) {
            self.note_verified(spec);
        }
        if plan::is_native(spec) && spec.nodes.len() > 1 {
            self.read_transport(spec).await;
        }
        if let Some(p) = readiness::probe_protocols(&self.live, head, host, spec.port, &spec.image_id).await {
            self.protocol_store.record(&p);
            self.update(id, |d| d.protocols = Some(p));
        }
        Ok(true)
    }

    /// Makes a loaded model available in Chat, the local server and the menu bar. Its key
    /// (when it has one) is already in the Keychain under the provider's own account.
    async fn register(&self, id: &str) {
        let Some(d) = self.deployments.lock().unwrap().iter().find(|d| d.spec.id == id).cloned() else { return };
        if d.spec.listen == "fabric" {
            return; // only the other Sparks can reach it
        }
        let provider = Provider {
            id: format!("vllm-{id}"),
            name: format!("{} on {}", engine_label(&d.spec), d.spec.nodes.join("+")),
            kind: ProviderKind::OpenaiCompatible,
            base_url: d.endpoint.clone(),
            models: vec![d.spec.served_name.clone()],
            default_model: Some(d.spec.served_name.clone()),
            ..Default::default()
        };
        if self.llm.upsert(provider.clone()).await.is_ok() {
            self.update(id, |d| d.provider_id = provider.id.clone());
        }
    }

    /// Unloads a load (P1.36). Its containers are looked for on every connected node, not only
    /// its own; then a task ("Unload gemma-3-27b-it from cell2") stops them (docker stop -t 30,
    /// then docker rm), marks the load Stopped once no node has any left, removes its provider
    /// and key, and checks that each node's memory, the port and /dev/shm came back. Returns the
    /// task's id. Refused, with nothing stopped, when a node can't be asked or holds the other
    /// build's or unlabelled containers of it (P1.2).
    pub async fn unload(self: &Arc<Self>, ctx: &Ctx, id: &str) -> Result<String, String> {
        if !ctx.live {
            let d = self.demo.lock().unwrap().unload(id).ok_or("Unknown model deployment.")?;
            self.live.audit.record_simulated(&d.spec.nodes.join(","), &format!("Unload {}", d.spec.served_name), &plan::unload_command(id));
            return Ok(String::new());
        }
        // A docker run of it under way ends first, so the containers it makes are found.
        let starting = self.start_lock(id).lock_owned().await;
        let d = self.deployments(ctx).into_iter().find(|d| d.spec.id == id).ok_or("Unknown model deployment.")?;
        let (targets, found) = self.unload_targets(ctx, &d).await?;
        Ok(self.start_unload(d, targets, found, false, starting))
    }

    /// The nodes an unload stops containers on: the load's own, and any connected node that holds
    /// one of its containers. Whether any of this build's were found.
    async fn unload_targets(&self, ctx: &Ctx, d: &Deployment) -> Result<(Vec<String>, bool), String> {
        let id = &d.spec.id;
        let name = &d.spec.served_name;
        let mut ask = d.spec.nodes.clone();
        ask.extend(ctx.node_ids().into_iter().filter(|n| !d.spec.nodes.contains(n) && self.live.is_connected(n)));
        let before = self.load_containers(&ask, id).await;
        Self::all_mine(name, &before)?;
        let holding: Vec<String> = before.iter().filter(|(_, c)| c.as_ref().is_some_and(|c| !c.mine.is_empty())).map(|(n, _)| n.clone()).collect();
        let mut targets = d.spec.nodes.clone();
        targets.extend(holding.iter().filter(|n| !d.spec.nodes.contains(n)).cloned());
        Ok((targets, !holding.is_empty()))
    }

    /// Who owns a load's containers, on every node asked, before anything is removed (P1.2): the
    /// other SparkBox's, and ones started before instance labels, are left alone, and so are that
    /// load's key and launch files, in a folder both builds share.
    fn all_mine(name: &str, before: &[(String, Option<plan::LoadContainers>)]) -> Result<(), String> {
        let on = |pick: fn(&plan::LoadContainers) -> bool| before.iter().filter(|(_, c)| c.as_ref().is_some_and(pick)).map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
        let unchecked: Vec<&str> = before.iter().filter(|(_, c)| c.is_none()).map(|(n, _)| n.as_str()).collect();
        if !unchecked.is_empty() {
            return Err(format!("Couldn't check {name}'s containers on {}; nothing was stopped.", unchecked.join(", ")));
        }
        let others = on(|c| !c.others.is_empty());
        if !others.is_empty() {
            return Err(format!("{name}'s containers on {others} are managed by the other SparkBox; nothing was stopped."));
        }
        let unlabelled = on(|c| !c.unlabelled.is_empty());
        if !unlabelled.is_empty() {
            return Err(format!("{name}'s containers on {unlabelled} have no SparkBox instance label, so whose they are can't be told; nothing was stopped. Remove them in Containers."));
        }
        Ok(())
    }

    /// Starts the unload's task on `targets` (`unload_targets`); `forget` drops the load from the
    /// list once its containers are gone. `starting` (the load's start_lock) is held until it's
    /// marked Stopped, so no start begins in between. Returns the task's id (the operation's,
    /// without a task list).
    fn start_unload(self: &Arc<Self>, d: Deployment, targets: Vec<String>, found: bool, forget: bool, starting: tokio::sync::OwnedMutexGuard<()>) -> String {
        let title = format!("Unload {} from {}", d.spec.served_name, targets.join(", "));
        let task = self.tasks.lock().unwrap().clone().map(|t| t.start(crate::tasks::TaskSpec::new("model", title.clone(), &targets)));
        let op_id = task.as_ref().map(|t| t.id().to_string()).unwrap_or_else(|| format!("unload-{}-{}", d.spec.id, now_ms()));
        let me = self.clone();
        let op = audit::OpMeta { title, source: "ui".into() };
        tauri::async_runtime::spawn(audit::with_op_id(op_id.clone(), op, async move {
            let id = d.spec.id.clone();
            let outcome = me.unload_in(d, &targets, found, task.as_ref(), starting).await;
            if outcome.is_ok() && forget {
                if let Err(e) = me.forget(&id) {
                    if let Some(t) = &task { t.finish_warn("Unloaded; the history note was kept", e); }
                }
            }
        }));
        op_id
    }

    /// The unload's steps (`start_unload`): stop, then check what came back.
    async fn unload_in(self: &Arc<Self>, d: Deployment, targets: &[String], found: bool, task: Option<&crate::tasks::TaskHandle>, starting: tokio::sync::OwnedMutexGuard<()>) -> Result<(), String> {
        let id = d.spec.id.clone();
        let name = d.spec.served_name.clone();
        let steps = Steps { task };
        let fail = |why: String| {
            steps.failed(why.clone());
            if let Some(t) = task {
                t.finish_err(why.clone());
            }
            Err(why)
        };
        steps.step("Stopping (docker stop -t 30, then docker rm)".into());
        let cmd = plan::unload_command(&id);
        let stop = |n: String| {
            let cmd = &cmd;
            async move { as_class(FenceClass::ModelUnload, self.live.run(&n, cmd, None, Duration::from_secs(120))).await }
        };
        // The head first: its workers lose it either way, and it's the one with an API to drain.
        let mut results = Vec::new();
        if let Some(head) = targets.first() {
            results.push(stop(head.clone()).await);
        }
        results.extend(join_all(targets.iter().skip(1).cloned().map(stop)).await);
        let mut said = Vec::new();
        let mut unreached = Vec::new();
        for (n, r) in targets.iter().zip(results) {
            match r {
                // Refused at the exit (look-only, the fence, the lease, the pin): in its own words.
                Err(e) if refused(&e) => return fail(words(e)),
                Err(_) => unreached.push(n.clone()),
                Ok(o) if !o.stderr.trim().is_empty() => said.push(format!("{n}: {}", last_line(&o.stderr))),
                Ok(_) => {}
            }
        }
        if !unreached.is_empty() {
            return fail(format!("Couldn't reach {} to stop its containers; they may still be running.", unreached.join(", ")));
        }
        // Stopped only once every node shows none of its containers left.
        let after = self.load_containers(targets, &id).await;
        let left: Vec<&str> = after.iter().filter(|(_, c)| c.as_ref().is_none_or(|c| !c.mine.is_empty())).map(|(n, _)| n.as_str()).collect();
        if !left.is_empty() {
            let docker = if said.is_empty() { String::new() } else { format!(" (docker said: {})", said.join("; ")) };
            let why = format!("Its containers on {} are still there{docker}; it isn't marked stopped.", left.join(", "));
            self.update(&id, |d| d.message = why.clone());
            return fail(why);
        }
        steps.ok(Some(if found { format!("Stopped on {}", targets.join(", ")) } else { "No containers of it were running".into() }));
        // Its provider as it is now: it may have become Ready since the unload was asked.
        let mut provider = String::new();
        self.update(&id, |d| {
            d.state = DeploymentState::Stopped;
            d.message = if found { "Unloaded".into() } else { "No containers of it were running.".into() };
            provider = d.provider_id.clone();
            if let Some(f) = &mut d.failure {
                f.left_on.clear();
            }
        });
        drop(starting);
        if !provider.is_empty() {
            let _ = self.llm.remove(&provider).await;
        }
        let _ = secrets::clear(&plan::key_account(&id));

        // What came back: memory, the port, /dev/shm (reads). A shortfall doesn't undo Stopped.
        steps.step("Checking memory, port and /dev/shm".into());
        let report = self.check_unloaded(&d, targets).await;
        let summary = if found { report.summary.clone() } else { format!("No containers of {name} from this SparkBox were running; it's marked stopped · {}", report.summary) };
        let saved = self.update_saved(&id, |d| {
            d.message = summary.clone();
            d.unload_report = Some(report.clone());
        });
        if let Err(e) = saved {
            steps.failed(format!("{summary} · {e}"));
            if let Some(t) = task { t.finish_warn(summary, e.clone()); }
            return Err(e);
        }
        if report.ok {
            steps.ok(Some(summary.clone()));
            if let Some(t) = task {
                t.finish_ok(summary);
            }
        } else {
            steps.failed(summary.clone());
            if let Some(t) = task {
                t.finish_warn(summary, "Unloaded, but something didn't come back; see the report.");
            }
        }
        Ok(())
    }

    /// After an unload, on each node: MemAvailable read every few seconds until it's back within
    /// 2 GiB of the baseline (or the wait is over), then the port (the head) and /dev/shm against
    /// the baseline's names. Reads only.
    async fn check_unloaded(&self, d: &Deployment, targets: &[String]) -> UnloadReport {
        let (every, within) = *self.unload_wait.lock().unwrap();
        let head = d.spec.nodes.first().cloned().unwrap_or_default();
        let port = d.spec.port;
        let mut nodes = Vec::new();
        let mut parts = Vec::new();
        let mut ok = true;
        for n in targets {
            let base = d.baseline.iter().find(|b| &b.node == n);
            // Memory: back within the slack, or the wait is over.
            let started = Instant::now();
            let mem = loop {
                let m = self.live.run(n, MEM_AVAILABLE, None, Duration::from_secs(15)).await.ok().and_then(|o| mem_available(&o.stdout));
                let back = matches!((m, base), (Some(m), Some(b)) if m + MEM_BACK_SLACK >= b.mem_available);
                if back || base.is_none() || started.elapsed() + every > within {
                    break m;
                }
                tokio::time::sleep(every).await;
            };
            let mem_back = matches!((mem, base), (Some(m), Some(b)) if m + MEM_BACK_SLACK >= b.mem_available);
            let gib = |b: u64| format!("{:.1}", b as f64 / plan::GIB);
            let mem_line = match (mem, base) {
                (Some(m), Some(b)) if mem_back => format!("memory back ({} GiB available, baseline {})", gib(m), gib(b.mem_available)),
                (Some(m), Some(b)) => format!("memory not back after {} s ({} GiB available, baseline {})", within.as_secs(), gib(m), gib(b.mem_available)),
                (Some(m), None) => format!("memory: no baseline recorded ({} GiB available)", gib(m)),
                (None, _) => "memory: couldn't read MemAvailable".to_string(),
            };
            ok &= mem_back || (base.is_none() && mem.is_some());
            // The port, on the node that served it.
            let port_line = if *n == head {
                match self.live.run(n, &format!("ss -Hltn 'sport = :{port}'"), None, Duration::from_secs(15)).await {
                    Ok(o) if o.stdout.trim().is_empty() && o.code == Some(0) => Some((true, format!("port {port} closed"))),
                    Ok(o) if o.code == Some(0) => {
                        let addr = o.stdout.trim().lines().next().and_then(|l| l.split_whitespace().nth(3)).unwrap_or("").to_string();
                        Some((false, format!("port {port} still taken (listening on {addr})")))
                    }
                    _ => Some((false, format!("port {port}: couldn't check it"))),
                }
            } else {
                None
            };
            let port_closed = port_line.as_ref().is_none_or(|(c, _)| *c);
            ok &= port_closed;
            // /dev/shm: what wasn't there before the load.
            let (new_shm, shm_line) = match self.live.run(n, "ls -la /dev/shm", None, Duration::from_secs(15)).await {
                Ok(o) if o.code == Some(0) => {
                    let lines: Vec<&str> = o.stdout.lines().collect();
                    match base {
                        Some(b) => {
                            let new: Vec<(String, u64)> = plan::shm_listing(&lines).into_iter().filter(|(name, _)| !b.shm.contains(name)).collect();
                            let line = if new.is_empty() {
                                "/dev/shm: nothing new".to_string()
                            } else {
                                let named: Vec<String> = new.iter().map(|(name, size)| format!("{name} ({})", crate::files::human_bytes(*size))).collect();
                                format!("/dev/shm: new since the load: {}; removing them needs sudo and your OK (not done)", named.join(", "))
                            };
                            (new.into_iter().map(|(name, _)| name).collect::<Vec<_>>(), line)
                        }
                        None => (Vec::new(), "/dev/shm: no baseline recorded".to_string()),
                    }
                }
                _ => {
                    ok = false;
                    (Vec::new(), "/dev/shm: couldn't list it".to_string())
                }
            };
            ok &= new_shm.is_empty();
            let mut segs = vec![mem_line];
            if let Some((_, l)) = port_line {
                segs.push(l);
            }
            segs.push(shm_line);
            parts.push(if targets.len() > 1 { format!("{n}: {}", segs.join(", ")) } else { segs.join(" · ") });
            nodes.push(NodeUnload { node: n.clone(), stopped: true, mem_available: mem, mem_back, port_closed, new_shm });
        }
        UnloadReport { at_ms: now_ms(), ok, summary: format!("Unloaded from {} · {}", targets.join(", "), parts.join(" · ")), nodes }
    }

    /// A load's containers on each node, by owner (`plan::load_containers`); None where the
    /// node couldn't be asked.
    async fn load_containers(&self, nodes: &[String], id: &str) -> Vec<(String, Option<plan::LoadContainers>)> {
        let cmd = plan::load_containers_command(id);
        let out = join_all(nodes.iter().map(|n| self.live.run(n, &cmd, None, Duration::from_secs(30)))).await;
        nodes.iter().zip(out).map(|(n, r)| (n.clone(), r.ok().filter(|o| o.code == Some(0)).map(|o| plan::load_containers(&o.stdout)))).collect()
    }

    /// Removes what earlier loads left (P2.9): C-U2 for each id on each node, as a task whose id is
    /// returned. Refused, with nothing removed, when a node can't be asked, when an id isn't this
    /// build's (`all_mine`: C-U2's filter keeps its containers safe, but its `rm -rf` of the key and
    /// launch files has no filter), when it's a running download, whose container has the job's id
    /// where a load's has its own, or when a load is starting or serving: unload it instead.
    pub async fn clean_leftovers(self: &Arc<Self>, ctx: &Ctx, nodes: Vec<String>, ids: Vec<String>) -> Result<String, String> {
        if !ctx.live {
            return Err("Available with Live data".into());
        }
        if nodes.is_empty() || ids.is_empty() {
            return Err("Pick what to remove.".into());
        }
        if let Some(n) = nodes.iter().find(|n| !ctx.node_ids().contains(n)) {
            return Err(format!("Unknown node {n}"));
        }
        if let Some(bad) = ids.iter().find(|id| id.len() > 40 || !plan::is_plain_word(id)) {
            return Err(format!("{bad} isn't a load's id."));
        }
        if let Some(d) = self.deployments(ctx).into_iter().find(|d| d.state.active() && ids.contains(&d.spec.id)) {
            return Err(format!("{} is loading or serving: unload it instead.", d.spec.served_name));
        }
        for id in &ids {
            self.left_by_this_build(ctx, &nodes, id).await?;
        }
        let title = format!("Remove what earlier loads left on {}", nodes.join(", "));
        let task = self.tasks.lock().unwrap().clone().map(|t| t.start(crate::tasks::TaskSpec::new("model", title.clone(), &nodes)));
        let op_id = task.as_ref().map(|t| t.id().to_string()).unwrap_or_else(|| format!("clean-{}", now_ms()));
        let me = self.clone();
        let op = audit::OpMeta { title, source: "ui".into() };
        tauri::async_runtime::spawn(audit::with_op_id(op_id.clone(), op, async move {
            let steps = Steps { task: task.as_ref() };
            let mut failed = None;
            for id in &ids {
                steps.step(format!("Stopping and removing {id} (docker stop -t 30, then docker rm)"));
                let cmd = plan::unload_command(id);
                let runs = join_all(nodes.iter().map(|n| as_class(FenceClass::ModelUnload, me.live.run(n, &cmd, None, Duration::from_secs(120))))).await;
                let refused = nodes.iter().zip(runs).find_map(|(n, r)| match r {
                    Ok(o) if o.code == Some(0) => None,
                    Ok(o) => Some(format!("{n}: {}", last_line(&o.stderr))),
                    Err(e) => Some(format!("{n}: {}", words(e))),
                });
                match refused {
                    Some(why) => {
                        steps.failed(why.clone());
                        failed = Some(why);
                    }
                    None => {
                        steps.ok(None);
                        me.update(id, |d| {
                            if let Some(f) = &mut d.failure {
                                f.left_on.retain(|n| !nodes.contains(n));
                            }
                        });
                    }
                }
            }
            if let Some(t) = &task {
                match failed {
                    Some(why) => t.finish_err(why),
                    None => t.finish_ok(format!("Removed from {}", nodes.join(", "))),
                }
            }
        }));
        Ok(op_id)
    }

    /// `id` on `nodes` is this build's own leftover: `clean_leftovers`' reads, before anything goes.
    async fn left_by_this_build(&self, ctx: &Ctx, nodes: &[String], id: &str) -> Result<(), String> {
        let known = self.deployments(ctx).into_iter().find(|d| d.spec.id == id);
        let name = known.as_ref().map(|d| d.spec.served_name.as_str()).unwrap_or(id);
        let before = self.load_containers(nodes, id).await;
        Self::all_mine(name, &before)?;
        let cmd = plan::container_names_command(id);
        let listed = join_all(nodes.iter().map(|n| self.live.run(n, &cmd, None, Duration::from_secs(30)))).await;
        let download = format!("{}{id}", plan::DOWNLOAD_CONTAINER);
        let mut running = Vec::new();
        for (node, names) in nodes.iter().zip(listed) {
            let names = names.map_err(|e| format!("Couldn't check {name}'s containers on {node}; nothing was stopped. {}", words(e)))?;
            if names.stdout.lines().any(|l| l.trim() == download) {
                running.push(node.as_str());
            }
        }
        if !running.is_empty() {
            return Err(format!("{id} is a download on {}, not something a load left. Stop it in Models › Jobs.", running.join(", ")));
        }
        let mine = before.iter().any(|(_, c)| c.as_ref().is_some_and(|c| !c.mine.is_empty()));
        if !mine && known.is_none() {
            return Err(format!("Nothing of {id} from this SparkBox is on {}; nothing was stopped.", nodes.join(", ")));
        }
        Ok(())
    }

    /// Stops a load's containers and starts it again with the same settings and key, as a task
    /// that ends when it answers again (restart_ordered). A restart by hand resets Keep running.
    /// Returns the task's id (the window that asked notes it, for one toast per outcome).
    pub async fn restart(self: &Arc<Self>, ctx: &Ctx, id: &str) -> Result<String, String> {
        self.restart_with_cache_consent(ctx, id, false).await
    }

    /// Manual consent applies to this attempt only, never the saved spec or a task's Retry.
    /// The existing Admin, ownership, lease and write-exit checks still decide every change.
    pub async fn restart_with_cache_consent(self: &Arc<Self>, ctx: &Ctx, id: &str, drop_caches_once: bool) -> Result<String, String> {
        self.reset_supervisor(id);
        let d = self.deployments(ctx).into_iter().find(|d| d.spec.id == id && d.state != DeploymentState::Stopped).ok_or("Unknown model deployment.")?;
        if d.recovery == Some(recovery::Recovery::Maintenance) { return Err("This model is paused for maintenance. Resume or restore it from Software Updates.".into()); }
        crate::ops::rolling::check_load(&self.live, !ctx.live, &d.spec.nodes)?;
        if ctx.live && d.spec.require_key && secrets::get(&plan::key_account(id))?.is_none() {
            return Err("Its API key is gone from the Keychain; load it again instead.".into());
        }
        let slot = RestartSlot::take(&self.restarting, &format!("load:{id}"), &d.spec.served_name)?;
        let (task, title) = self.restart_task(&d.spec.served_name, &d.spec.nodes, "Restart", false);
        let task_id = task.as_ref().map(|t| t.id().to_string()).unwrap_or_default();
        let me = self.clone();
        let ctx = ctx.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn(async move {
            let _slot = slot;
            if let Err(e) = me.restart_ordered_in(&ctx, &id, task, title, RestartOptions { drop_caches_once, ..Default::default() }).await {
                eprintln!("restart {id}: {e}");
            }
        });
        Ok(task_id)
    }

    /// An ordered restart is planned work from before its first stop: the endpoint reads Paused
    /// (this Models' own note, so it ends with the restart), and the health engine's window
    /// (alerts' maintenance) keeps its alerts Planned until it answers again. The returned guard
    /// ends both when the restart's task is over, however it ended.
    fn planned_restart(self: &Arc<Self>, ctx: &Ctx, endpoint_id: &str, nodes: &[String]) -> PlannedGuard {
        let now = now_ms();
        crate::health::maintenance::begin_endpoint_at(!ctx.live, endpoint_id, nodes, Some(now + PLANNED_RESTART_MS), PLANNED_REASON, now);
        self.eps.lock().unwrap().planned.insert(endpoint_id.to_string(), now);
        PlannedGuard { models: self.clone(), endpoint: endpoint_id.to_string(), nodes: nodes.to_vec(), demo: !ctx.live }
    }

    /// The task a restart runs as: "Restart llama-3.3-70b in order" on several nodes.
    fn restart_task(&self, name: &str, nodes: &[String], verb: &str, by_supervisor: bool) -> (Option<crate::tasks::TaskHandle>, String) {
        let title = restart_title(verb, name, nodes.len() > 1);
        let task = self.tasks.lock().unwrap().clone().map(|t| {
            let ts = crate::tasks::TaskSpec::new("model", title.clone(), nodes);
            t.start(if by_supervisor { ts.source("system") } else { ts })
        });
        (task, title)
    }

    /// A load's ordered restart, as a task (C-O): stop the head, stop the workers, check the nodes
    /// again, write the launch files again, start the workers, start the head, then verify that it
    /// answers. `by_supervisor` marks Keep running's own retries.
    pub async fn restart_ordered(self: &Arc<Self>, ctx: &Ctx, id: &str, verb: &str, by_supervisor: bool) -> Result<(), String> {
        let d = self.deployments(ctx).into_iter().find(|d| d.spec.id == id && d.state != DeploymentState::Stopped).ok_or("Unknown model deployment.")?;
        let _slot = RestartSlot::take(&self.restarting, &format!("load:{id}"), &d.spec.served_name)?;
        let (task, title) = self.restart_task(&d.spec.served_name, &d.spec.nodes, verb, by_supervisor);
        self.restart_ordered_in(ctx, id, task, title, RestartOptions { by_supervisor, ..Default::default() }).await
    }

    /// restart_ordered inside its task. One operation in the activity log (the task's id): a head
    /// line, each step stamped as it runs, and a Verify step that carries the outcome.
    async fn restart_ordered_in(self: &Arc<Self>, ctx: &Ctx, id: &str, task: Option<crate::tasks::TaskHandle>, title: String, options: RestartOptions) -> Result<(), String> {
        let RestartOptions { by_supervisor, drop_caches_once } = options;
        let mut d = self.deployments(ctx).into_iter().find(|d| d.spec.id == id && d.state != DeploymentState::Stopped).ok_or("Unknown model deployment.")?;
        // Use a local copy in both preflights and startup; never persist this additional grant.
        if drop_caches_once && !by_supervisor && plan::is_native(&d.spec) {
            d.spec.drop_caches = true;
        }
        let spec = d.spec.clone();
        let name = spec.served_name.clone();
        if let Some(t) = &task {
            t.set_retry("deployment_restart", json!({ "id": id }));
        }
        let op = audit::OpMeta { title: title.clone(), source: if by_supervisor { "system".into() } else { "ui".into() } };
        let op_id = task.as_ref().map(|t| t.id().to_string()).unwrap_or_else(|| format!("restart-{id}-{}", now_ms()));
        let me = self.clone();
        let ctx = ctx.clone();
        let id = id.to_string();
        audit::with_op_id(op_id, op, async move {
            let head_line = me.live.audit.begin_op(&spec.nodes.join(", "), &title, "", !ctx.live);
            let steps = Steps { task: task.as_ref() };
            let head = spec.nodes[0].clone();
            let workers: Vec<String> = spec.nodes[1..].to_vec();
            let multi = spec.nodes.len() > 1;
            let part = if multi { "head" } else { "server" };
            // Planned before the first stop: Paused, never a transient Down (until this op ends).
            let planned = me.endpoints().into_iter().find(|e| e.deployment_id.as_deref() == Some(id.as_str())).map(|e| me.planned_restart(&ctx, &e.id, &spec.nodes));

            // Set once the restart has taken the load over; a failure before that is a plain one.
            let mut generation = None;
            // Refused before the first stop: the load is left exactly as it was.
            let mut untouched = true;
            let why = if by_supervisor { Starting::KeepRunning } else { Starting::Restart };
            let started: Result<(), StartFailed> = async {
                let nothing = |e: String| StartFailed::on(e, &[]);
                let role = if multi { "head" } else { "single" };
                if !ctx.live {
                    // The simulated containers stop and start as fast as a real docker's would take.
                    steps.step(format!("Stop the {part} on {head}"));
                    demo_step(&me, &head, &format!("Stop the {part} on {head}"), &plan::stop_role_command(&id, role)).await;
                    steps.ok(None);
                    if multi {
                        steps.step(format!("Stop the workers on {}", workers.join(", ")));
                        for n in &workers {
                            demo_step(&me, n, &format!("Stop the worker on {n}"), &plan::stop_role_command(&id, "worker")).await;
                        }
                        steps.ok(None);
                        steps.step(format!("Start the workers on {}", workers.join(", ")));
                        for n in &workers {
                            demo_step(&me, n, &format!("Start the worker on {n}"), &format!("docker run -d --name {} …", plan::container_name(&id, n, "worker"))).await;
                        }
                        steps.ok(None);
                    }
                    steps.step(format!("Start the {part} on {head}"));
                    me.demo.lock().unwrap().restart(&id).ok_or_else(|| nothing("Unknown model deployment.".into()))?;
                    demo_step(&me, &head, &format!("Start the {part} on {head}"), &format!("docker run -d --name {} …", plan::container_name(&id, &head, role))).await;
                    steps.ok(None);
                    return Ok(());
                }
                let key = if spec.require_key { secrets::get(&plan::key_account(&id)).map_err(nothing)? } else { None };
                if spec.require_key && key.is_none() {
                    return Err(nothing("Its API key is gone from the Keychain; load it again instead.".into()));
                }
                if plan::is_native(&spec) {
                    steps.step("Check the nodes first".into());
                    if let Err(e) = me.restart_blocked(&d, &ctx.settings, why).await {
                        untouched = true;
                        return Err(nothing(e));
                    }
                    steps.ok(None);
                }
                steps.step(format!("Stop the {part} on {head}"));
                as_class(FenceClass::ModelUnload, me.live.run_on_admitted(&head, &plan::stop_role_command(&id, role), None, Duration::from_secs(120), || {
                    // A refused stop leaves the running load and its watcher intact. Once
                    // admitted, retire that watcher before SSH can stop the containers.
                    untouched = false;
                    me.update(&id, |d| {
                        d.state = DeploymentState::Starting;
                        d.stage = Some(Stage::Starting);
                        d.message = "Restarting…".into();
                        d.generation += 1;
                        generation = Some(d.generation);
                        d.first_start = false;
                        d.started_at = now();
                        d.failure = None;
                        d.recovery = None;
                        d.transport = None;
                        d.transport_warning = None;
                    });
                })).await.map_err(|e| nothing(format!("{head}: {e}")))?;
                let generation = generation.ok_or_else(|| nothing("It was unloaded meanwhile; nothing was started.".into()))?;
                steps.ok(None);
                if multi {
                    steps.step(format!("Stop the workers on {}", workers.join(", ")));
                    let cmd = plan::stop_role_command(&id, "worker");
                    let results = join_all(workers.iter().map(|n| as_class(FenceClass::ModelUnload, me.live.run(n, &cmd, None, Duration::from_secs(120))))).await;
                    for (n, r) in workers.iter().zip(results) {
                        r.map_err(|e| nothing(format!("{n}: {e}")))?;
                    }
                    steps.ok(None);
                }
                if !plan::is_native(&spec) {
                    let nets = me.prepare(&spec, &ctx.settings).await.map_err(nothing)?;
                    // As a load's first start: an Unload waits for these docker runs (start_lock).
                    let lock = me.start_lock(&id);
                    let starting = lock.lock().await;
                    if !me.current(&id, generation) {
                        return Err(nothing("It was unloaded meanwhile; nothing was started.".into()));
                    }
                    steps.step(format!("Start the {part} on {head}"));
                    let container = me.start_head(&spec, &nets).await.map_err(nothing)?;
                    steps.started();
                    steps.ok(None);
                    if multi {
                        steps.step(format!("Start the workers on {} (they join the head)", workers.join(", ")));
                        me.start_workers(&spec, &nets).await.map_err(nothing)?;
                        steps.ok(None);
                    }
                    drop(starting);
                    let (me2, spec_) = (me.clone(), spec.clone());
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = me2.clone().follow(&spec_, &container, generation).await {
                            me2.fail(&spec_.id, e);
                        }
                    });
                    return Ok(());
                }
                // The nodes again, as a new load's are. The memory it held is back by now.
                steps.step("Check the nodes again".into());
                let mut fresh = spec.clone();
                let (baseline, nets) = me.ready_to_load(&mut fresh, &ctx.settings, why).await.map_err(nothing)?;
                me.update(&id, |d| {
                    d.spec.image_id = fresh.image_id.clone();
                    d.baseline = baseline;
                });
                steps.ok(None);
                // As a load's first start: an Unload waits for these docker runs (start_lock).
                let lock = me.start_lock(&id);
                let starting = lock.lock().await;
                if !me.current(&id, generation) {
                    return Err(nothing("It was unloaded meanwhile; nothing was started.".into()));
                }
                let begun = me.start_ranks(&fresh, &nets, key.as_deref(), &steps).await;
                drop(starting);
                begun?;
                let me2 = me.clone();
                tauri::async_runtime::spawn(async move {
                    let container = plan::rank_container(&fresh.id, 0, fresh.nodes.len());
                    if let Err(e) = me2.clone().follow(&fresh, &container, generation).await {
                        me2.fail_start(&fresh, StartFailed::on(e, &fresh.nodes), generation).await;
                    }
                });
                Ok(())
            }
            .await;
            if let Err(failed) = started {
                let e = audit::redact(&words(failed.why.clone()));
                match generation.filter(|_| ctx.live) {
                    Some(generation) => me.fail_start(&spec, failed, generation).await,
                    None if ctx.live && !untouched => me.fail(&id, e.clone()),
                    None => {}
                }
                steps.failed(e.clone());
                if let Some(t) = &task {
                    t.finish_err(e.clone());
                }
                me.live.audit.end_op(&head_line, false, None, Some(e.clone()));
                return Err(e);
            }

            // Its containers are up again: from here Verify sees it as it is (Loading, Ready or Down).
            drop(planned);
            // It isn't done until it answers again.
            let mut refs = verify::affected_in(&me.endpoints(), &verify::Scope::Deployment(id.clone()));
            for r in &refs {
                me.clear_restarts(&r.id);
            }
            if refs.is_empty() {
                let eid = format!("{head}-{}-{}", if spec.engine == "sglang" { "sglang" } else { "vllm" }, spec.port);
                refs.push(verify::EndpointRef { id: eid, model: name.clone(), nodes: spec.nodes.clone() });
            }
            // As long as a start may take: a restart compiles what a first load does.
            let limit = if ctx.live { *me.ready_within.lock().unwrap() } else { Duration::from_secs(6 * 60) };
            me.verify_restart(&refs[0], limit, &steps, task.as_ref(), &head_line, !ctx.live).await
        })
        .await
    }

    /// Restarts an endpoint SparkBox didn't start, in the order Ray needs (a worker that goes
    /// away takes the head's engine with it): stop the workers, stop the head, start the head,
    /// start the workers, then verify it answers. A single container just restarts. A task and
    /// one operation in the activity log; returns the task's id.
    pub async fn restart_external(self: &Arc<Self>, ctx: &Ctx, endpoint: &str) -> Result<String, String> {
        let e = self.endpoints().into_iter().find(|e| e.id == endpoint).ok_or("That endpoint isn't listed any more.")?;
        if e.managed_by == endpoints::ManagedBy::Sparkbox {
            return Err("SparkBox loaded this one: restart it from its card instead.".into());
        }
        let head = e.members.iter().find(|m| m.head).cloned().ok_or_else(|| format!("{} on {} isn't a container: restart it in Services or Workloads.", e.engine.label(), e.head_node))?;
        let name = endpoints::label_of(&e);
        let slot = RestartSlot::take(&self.restarting, &format!("endpoint:{}", e.id), &name)?;
        let (task, title) = self.restart_task(&name, &e.nodes, "Restart", false);
        let task_id = task.as_ref().map(|t| t.id().to_string()).unwrap_or_default();
        let me = self.clone();
        let ctx = ctx.clone();
        tauri::async_runtime::spawn(async move {
            let _slot = slot;
            if let Err(why) = me.restart_external_in(&ctx, e, head, task, title).await {
                eprintln!("restart {name}: {why}");
            }
        });
        Ok(task_id)
    }

    async fn restart_external_in(self: &Arc<Self>, ctx: &Ctx, e: Endpoint, head: endpoints::Member, task: Option<crate::tasks::TaskHandle>, title: String) -> Result<(), String> {
        let workers: Vec<endpoints::Member> = e.members.iter().filter(|m| !m.head).cloned().collect();
        let name = endpoints::label_of(&e);
        if let Some(t) = &task {
            t.set_retry("endpoint_restart", json!({ "id": e.id }));
        }
        let op_id = task.as_ref().map(|t| t.id().to_string()).unwrap_or_else(|| format!("restart-{}-{}", e.id, now_ms()));
        let me = self.clone();
        let ctx = ctx.clone();
        audit::with_op_id(op_id, audit::OpMeta { title: title.clone(), source: "ui".into() }, async move {
            let head_line = me.live.audit.begin_op(&e.nodes.join(", "), &title, "", !ctx.live);
            // Planned before the first stop: Paused, never a transient Down (until this op ends).
            let planned = me.planned_restart(&ctx, &e.id, &e.nodes);
            let steps = Steps { task: task.as_ref() };
            let act = |m: endpoints::Member, action: &'static str| {
                let me = me.clone();
                let live = ctx.live;
                async move {
                    if live {
                        me.live.container_action(&m.node, &m.id, action).await
                    } else {
                        let cluster = me.cluster.clone().ok_or("No simulated cluster.")?;
                        cluster.lock().unwrap().container_action(&m.node, &m.id, action)?;
                        let verb = if action == "stop" { "Stop" } else { "Start" };
                        demo_step(&me, &m.node, &format!("{verb} {} on {}", m.name, m.node), &format!("docker {action} {}", m.name)).await;
                        Ok(())
                    }
                }
            };
            let plan: Vec<(String, Vec<endpoints::Member>, &'static str)> = if workers.is_empty() {
                vec![(format!("Stop {} on {}", head.name, head.node), vec![head.clone()], "stop"), (format!("Start {} on {}", head.name, head.node), vec![head.clone()], "start")]
            } else {
                let on = workers.iter().map(|w| w.node.clone()).collect::<Vec<_>>().join(", ");
                vec![
                    (format!("Stop the workers on {on}"), workers.clone(), "stop"),
                    (format!("Stop the head on {}", head.node), vec![head.clone()], "stop"),
                    (format!("Start the head on {}", head.node), vec![head.clone()], "start"),
                    (format!("Start the workers on {on} (they join the head)"), workers.clone(), "start"),
                ]
            };
            for (label, members, action) in plan {
                steps.step(label);
                for m in members {
                    if let Err(why) = act(m.clone(), action).await {
                        let why = words(format!("{} on {}: {why}", m.name, m.node));
                        steps.failed(why.clone());
                        if let Some(t) = &task {
                            t.finish_err(why.clone());
                        }
                        me.live.audit.end_op(&head_line, false, None, Some(why.clone()));
                        return Err(why);
                    }
                    if action == "start" {
                        steps.started();
                    }
                }
                steps.ok(None);
            }
            // Its containers are up again: from here Verify sees it as it is (Loading, Ready or Down).
            drop(planned);
            me.clear_restarts(&e.id);
            let r = verify::EndpointRef { id: e.id.clone(), model: name.clone(), nodes: e.nodes.clone() };
            let limit = if ctx.live { Duration::from_secs(25 * 60) } else { Duration::from_secs(5 * 60) };
            me.verify_restart(&r, limit, &steps, task.as_ref(), &head_line, !ctx.live).await
        })
        .await
    }

    /// The Verify step of a restart: waits until the endpoint answers and generates (or stays
    /// down), says so in the task, in the activity log under the restart's own head line, and to
    /// the health engine, which closes its endpoint-down and crash-loop alerts at once.
    async fn verify_restart(self: &Arc<Self>, r: &verify::EndpointRef, limit: Duration, steps: &Steps<'_>, task: Option<&crate::tasks::TaskHandle>, head_line: &str, demo: bool) -> Result<(), String> {
        steps.step(format!("Verify {} answers", r.model));
        let progress = |_: &verify::EndpointRef, e: &Endpoint| {
            if let Some(t) = task {
                let detail = match (e.health, e.loading_pct) {
                    (endpoints::Health::Loading, Some(p)) => format!("Loading weights {p}%"),
                    _ => e.health_reason.clone().unwrap_or_else(|| "Loading".into()),
                };
                t.progress(e.loading_pct.map(|p| p as f32 / 100.0).unwrap_or(0.0), Some(detail));
            }
        };
        let (r, o) = verify::wait_on(self, std::slice::from_ref(r), limit, Duration::from_secs(2), progress).await.remove(0);
        let sentence = o.sentence(&r);
        let ready = matches!(o, verify::VerifyOutcome::Ready { .. });
        // What it proved: an endpoint no token can go to answered its model list only.
        let proved = match &o {
            verify::VerifyOutcome::Ready { untested: Some(why), .. } => format!("answers its model list (not tested: {why})"),
            _ => "answers its model list and a test token".to_string(),
        };
        let nodes = r.nodes.join(", ");
        self.live.audit.record(audit::Entry {
            node: nodes,
            kind: "command".into(),
            command: format!("verify: {} {proved}", r.model),
            title: format!("Verify {} answers", r.model),
            class: "read".into(),
            state: if ready { "ok" } else { "failed" }.into(),
            stdout_tail: ready.then(|| sentence.clone()),
            error: (!ready).then(|| sentence.clone()),
            simulated: demo,
            ..Default::default()
        });
        if ready {
            let reason = format!("Verified: {sentence}");
            if let Some(h) = self.health.lock().unwrap().clone() {
                h.resolve_now("endpoint-down", &r.id, &reason);
                for m in self.endpoints().iter().filter(|e| e.id == r.id).flat_map(|e| e.members.clone()) {
                    h.resolve_now("crash-loop", &m.name, &reason);
                }
            }
            steps.ok(Some(sentence.clone()));
            if let Some(t) = task {
                t.finish_ok(reason.clone());
            }
            self.live.audit.end_op(head_line, true, Some(reason), None);
            Ok(())
        } else {
            steps.failed(sentence.clone());
            if let Some(t) = task {
                t.finish_err(sentence.clone());
            }
            self.live.audit.end_op(head_line, false, None, Some(sentence.clone()));
            Err(sentence)
        }
    }

    pub fn forget(&self, id: &str) -> Result<(), String> {
        let mut all = self.deployments.lock().unwrap();
        let mut next = all.clone();
        next.retain(|d| !(d.spec.id == id && !d.state.active() && d.recovery != Some(recovery::Recovery::Maintenance)));
        if all.len() != next.len() {
            self.deployment_history.lock().unwrap().save(&next)?;
            *all = next;
            self.live.audit.record_app("Forgot deployment note", &format!("{id}; recovery copy retained in deployment-history"));
        }
        Ok(())
    }

    /// Forgets a load that failed or was unloaded. One whose containers (this build's) are still
    /// on a node is unloaded first, as unload's own task, and forgotten once they're gone: the
    /// task's id is returned. Its refusals are unload's; a load still starting or serving is
    /// unloaded instead.
    pub async fn forget_checked(self: &Arc<Self>, ctx: &Ctx, id: &str) -> Result<Option<String>, String> {
        if self.deployments(ctx).iter().any(|d| d.spec.id == id && d.recovery == Some(recovery::Recovery::Maintenance)) { return Err("This model is paused for maintenance. Restore or Unload it before forgetting it.".into()); }
        if !ctx.live {
            self.demo.lock().unwrap().forget(id);
            return Ok(None);
        }
        let starting = self.start_lock(id).lock_owned().await;
        let d = self.deployments(ctx).into_iter().find(|d| d.spec.id == id).ok_or("Unknown model deployment.")?;
        if d.state.active() {
            return Err(format!("{} is loading or serving: unload it instead.", d.spec.served_name));
        }
        let (targets, found) = self.unload_targets(ctx, &d).await?;
        if !found {
            self.forget(id)?;
            return Ok(None);
        }
        Ok(Some(self.start_unload(d, targets, found, true, starting)))
    }

    /// Resume observation once per saved generation. Reopening never repeats docker run.
    pub fn resume(self: &Arc<Self>, settings: &ModelSettings) {
        let active: Vec<(DeploySpec, u64)> = self.deployments.lock().unwrap().iter()
            .filter(|d| !d.demo && d.state.active()).map(|d| (d.spec.clone(), d.generation)).collect();
        for (spec, generation) in active {
            let key = (spec.id.clone(), generation);
            if !self.resuming.lock().unwrap().insert(key.clone()) { continue; }
            self.update(&spec.id, |d| {
                if d.generation == generation && d.state.active() {
                    d.recovery = Some(recovery::Recovery::Checking);
                    d.message = "Checking the saved load against fresh node reports…".into();
                }
            });
            let me = self.clone();
            let settings = settings.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(RESUME_AFTER).await;
                if me.current(&spec.id, generation) {
                    if let Err(e) = me.clone().watch_only(&spec, &settings, generation).await {
                        // A confirmed readiness failure retains the existing guarded rollback.
                        me.fail_start(&spec, StartFailed::on(e, &spec.nodes), generation).await;
                        me.update(&spec.id, |d| {
                            if d.generation == generation && d.state == DeploymentState::Failed {
                                d.recovery = d.failure.as_ref().is_none_or(|f| !f.left_on.is_empty()).then_some(recovery::Recovery::Cleanup);
                                d.recoverable = false;
                            }
                        });
                    }
                }
                me.resuming.lock().unwrap().remove(&key);
            });
        }
    }

    /// After SparkBox restarted: keeps watching a load it was starting or serving. One that was
    /// still starting is Ready only through the readiness gate, as `follow` makes it, and it has
    /// `follow`'s limit, counted from when the load began: SparkBox being closed doesn't stop that
    /// clock. An unreachable node leaves the saved load waiting for fresh evidence; a missed
    /// connection is never permission to clean up. Time look-only holds the test token
    /// doesn't count.
    async fn watch_only(self: Arc<Self>, spec: &DeploySpec, _settings: &ModelSettings, generation: u64) -> Result<(), String> {
        if spec.nodes.is_empty() || !plan::is_plain_word(&spec.id) || spec.id.len() > 40 {
            self.recovery_owner(spec, generation, recovery::Recovery::OwnerAction, "The saved load has invalid node or deployment settings. Review it before changing the cluster.");
            return Ok(());
        }
        let head = &spec.nodes[0];
        let container = containers_of(spec).remove(0);
        let probe = plan::probe_command(&container, plan::probe_host(spec), spec.port);
        let Some(began) = self.deployments.lock().unwrap().iter().find(|d| d.spec.id == spec.id && d.generation == generation && d.state.active()).map(|d| d.started_at) else { return Ok(()) };
        let before = Duration::from_secs(now().saturating_sub(began));
        let mut started = Instant::now();
        let mut last = started;

        loop {
            let state = self.deployments.lock().unwrap().iter().find(|d| d.spec.id == spec.id && d.generation == generation).map(|d| d.state);
            if !state.is_some_and(|s| s.active()) {
                return Ok(()); // unloaded, or restarted (a newer watcher has it)
            }
            let starting = state == Some(DeploymentState::Starting);
            match self.reconcile_load(spec, generation).await {
                recovery::Reconciled::Wait => {
                    tokio::time::sleep(RESUME_POLL_STARTING).await;
                    continue;
                }
                recovery::Reconciled::Owner => return Ok(()),
                recovery::Reconciled::Present => {}
            }
            let polled = self.poll(head, &probe, Duration::from_secs(20)).await;
            let since = last.elapsed();
            last = Instant::now();
            if !self.current(&spec.id, generation) {
                return Ok(()); // unloaded while it asked
            }
            let mut answered = false;
            let mut held = false;
            if polled.is_err() {
                self.recovery_wait(&spec.id, generation, "Waiting for the head node to answer. The saved load is still being watched; nothing will be relaunched or cleaned up while it is unreachable.");
                tokio::time::sleep(RESUME_POLL_STARTING).await;
                continue;
            }
            if let Ok(p) = polled {
                let (state, http, logs) = probe_parts(&p);
                let logs = audit::redact(&logs);
                let restarts = restart_count(&state);
                if starting && restarts > 0 {
                    return Err(started_again(spec, head, restarts, &logs));
                }
                if !state.starts_with("running") {
                    return Err(format!("vLLM isn't running on {head} any more."));
                }
                self.update(&spec.id, |d| {
                    if d.generation == generation {
                        d.recovery = None;
                        if !starting { d.message = "Watching the recovered load.".into(); }
                    }
                });
                if starting {
                    let stage = plan::stage_from_logs(&logs, http == "200");
                    self.update(&spec.id, |d| {
                        d.stage = Some(stage.clone());
                        d.log_tail = logs.clone();
                        if stage != Stage::Ready {
                            d.message = stage_message(&stage);
                        }
                    });
                    if stage == Stage::Ready {
                        answered = self.gate(spec, &http, generation).await?;
                        held = !answered && self.deployments.lock().unwrap().iter().any(|d| d.spec.id == spec.id && d.readiness.as_ref().is_some_and(|r| r.why.as_deref() == Some(readiness::HELD)));
                    }
                }
            }
            if held {
                started += since;
            }
            let within = *self.ready_within.lock().unwrap();
            if starting && !answered && !held && before + started.elapsed() > within {
                self.update(&spec.id, |d| d.recoverable = false);
                return Err(not_ready_within(spec, within));
            }
            tokio::time::sleep(if starting { RESUME_POLL_STARTING } else { RESUME_POLL }).await;
        }
    }

    // ---- Benchmark

    /// The benchmarks of the data source in use (the last endpoint round's): Demo's runs never
    /// show in Live, nor Live's in Demo. Before the first round, all of them.
    pub fn benchmarks(&self) -> Vec<Benchmark> {
        let live = self.eps.lock().unwrap().was_live;
        bench::of_source(&self.benchmarks.lock().unwrap(), live)
    }
}

#[cfg(test)]
mod check_and_job_tests {
    //! Check nodes under the write exit (a fence that allows container-test, look-only, none),
    //! and the tasks a download or a copy runs as, against fake nodes.

    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::fake_node::{self, fake_node, real_shell, temp, Script};
    use crate::live::exit::{Enforce, WritePolicy};
    use crate::live::fence::{self, Spec};
    use crate::live::fence_class::FenceClass;
    use crate::vpn::Vpn;

    struct Policy(AtomicBool);

    impl WritePolicy for Policy {
        fn look_only(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }

        fn admin_unlocked(&self) -> bool {
            true
        }
    }

    fn models(dir: &std::path::Path, live: Arc<Live>) -> Arc<Models> {
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        Models::new(dir.to_path_buf(), live, llm, None)
    }

    fn ctx(live: &Live, inventory: &Inventory) -> Ctx {
        Ctx { live: true, settings: ModelSettings { model_root: "~/models".into(), ..Default::default() }, inventory: inventory.clone(), snapshot: Some(live.snapshot(inventory)) }
    }

    #[tokio::test]
    async fn check_nodes_runs_the_full_check_under_a_container_test_fence_and_quick_facts_in_look_only() {
        let home = temp("check-fence-home");
        std::fs::create_dir_all(home.join("models")).unwrap();
        let (node, port) = fake_node(&home, real_shell()).await;
        let base = temp("check-fence-app");
        let (live, inventory) = fake_node::connect(&base, &[("cell1", port)]).await;
        let instance = crate::identity::instance_name();
        let rules = Enforce {
            instance: instance.into(),
            fence_dir: base.join("SparkBox Fence"),
            config_dir: Some(base.join(format!("config-{instance}"))),
            noauth: false,
            tag: "p1-r2-test".into(),
            mac: Some("Studio".into()),
        };
        std::fs::create_dir_all(&rules.fence_dir).unwrap();
        let key = fence::new_key().unwrap();
        let (key_id, public_key) = fence::public_of(&key).unwrap();
        let trust = fence::KeyFile { key_id, public_key, kind: "keychain".into() };
        fence::save_trust(rules.config_dir.as_ref().unwrap(), &trust).unwrap();
        live.enforce_writes(rules.clone());
        let policy = Arc::new(Policy(AtomicBool::new(false)));
        live.set_write_policy(policy.clone());
        let m = models(&base, live.clone());
        let full_checks = || node.ran("sparkjob.py check").into_iter().filter(|c| !c.ends_with(" quick")).collect::<Vec<_>>();

        // No fence: the refusal is the node's answer, not facts read some other way.
        let checks = m.check(&ctx(&live, &inventory)).await;
        let e = checks[0].error.clone().expect("refused");
        assert!(e.contains("No fence is open"), "{e}");
        assert!(node.ran("sparkjob.py check").is_empty(), "nothing reached the node");

        // A fence that allows container-test on cell1: the full check runs, its test container
        // named for this build.
        let spec = Spec { nodes: vec!["cell1".into()], classes: vec![FenceClass::ContainerTest], minutes: 180, instance: instance.into(), ..Default::default() };
        let record = fence::record_for(&spec, &key, crate::models::now_ms(), "Studio").unwrap();
        fence::write(&rules.fence_dir, rules.config_dir.as_ref().unwrap(), &trust, &record).unwrap();
        let checks = m.check(&ctx(&live, &inventory)).await;
        assert!(checks[0].error.is_none(), "{:?}", checks[0].error);
        assert!(checks[0].root_exists);
        let ran = full_checks();
        assert_eq!(ran.len(), 1, "{ran:?}");
        assert!(ran[0].ends_with(&format!(" --instance '{instance}'")), "{ran:?}");
        let row = live.audit.entries().into_iter().find(|e| e.command.contains("sparkjob.py check ") && e.command.contains("--instance")).expect("its activity line");
        assert_eq!(row.fence.as_deref(), Some("container-test"));

        let refused = || live.audit.entries().into_iter().filter(|e| e.title == "Refused").count();
        assert_eq!(refused(), 1, "the one without a fence");

        // Look-only: the full check is refused once, and the quick facts are read instead.
        policy.0.store(true, Ordering::SeqCst);
        let checks = m.check(&ctx(&live, &inventory)).await;
        assert!(checks[0].error.is_none(), "{:?}", checks[0].error);
        assert!(checks[0].root_exists);
        assert_eq!(full_checks().len(), 1, "no second full check reached the node");
        assert!(node.ran("sparkjob.py check").last().unwrap().ends_with(" quick"));
        assert_eq!(refused(), 2);
        live.stop();
    }

    /// sparkjob.py's answers for one job that runs until `done` is set.
    fn jobs(done: Arc<AtomicBool>) -> Script {
        Arc::new(move |cmd, _| {
            let job = |id: &str| {
                let state = if done.load(Ordering::SeqCst) { "done" } else { "running" };
                json!({ "id": id, "kind": "download", "state": state, "step": "downloading", "message": "", "bytesDone": 1_000_000_000u64, "bytesTotal": 4_000_000_000u64, "rateBps": 50e6 })
            };
            if cmd.contains("sparkjob.py start") {
                return Some((job(cmd.split_whitespace().last().unwrap()).to_string(), 0));
            }
            if cmd.contains("sparkjob.py list") {
                return Some((json!([job("dl-test")]).to_string(), 0));
            }
            None
        })
    }

    #[tokio::test]
    async fn turning_look_only_on_says_a_running_download_keeps_going_on_its_node() {
        let done = Arc::new(AtomicBool::new(false));
        let (_node, port) = fake_node(&temp("job-task-home"), jobs(done.clone())).await;
        let base = temp("job-task-app");
        let (live, _) = fake_node::connect(&base, &[("cell1", port)]).await;
        let m = models(&base, live.clone());
        let tasks = crate::tasks::Tasks::new(None);
        m.set_tasks(tasks.clone());

        let job = m.start_job("cell1", "download", "dl-test", &json!({ "repo": "Qwen/Qwen3-8B" })).await.unwrap();
        m.follow_job(&job, "Download Qwen/Qwen3-8B to cell1".into());
        let task = || tasks.list().into_iter().find(|t| t.title == "Download Qwen/Qwen3-8B to cell1").unwrap();
        for _ in 0..100 {
            if task().progress.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(task().progress, Some(0.25), "its progress comes from the job");
        assert!(task().on_node && !task().finished());

        let held = tasks.hold_non_read("Look-only turned on");
        assert_eq!((held.stopped, held.stopping, held.on_node), (0, 0, 1));
        assert_eq!(held.words().as_deref(), Some("1 task's work keeps running on its node"));
        let t = task();
        assert_eq!(t.held.as_deref(), Some("Look-only turned on: what it started on cell1 keeps running; SparkBox can't stop it in look-only"));
        assert!(!t.cancel_requested && !t.finished(), "not claimed stopped");

        // It ends with the job.
        done.store(true, Ordering::SeqCst);
        for _ in 0..100 {
            if task().finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(task().summary.as_deref(), Some("Finished on cell1"));
        live.stop();
    }

    #[tokio::test]
    async fn a_download_sparkbox_quit_while_following_is_picked_up_again_and_ends_with_its_job() {
        let home = temp("pickup-home");
        let (_node, port) = fake_node(&home, real_shell()).await;
        let base = temp("pickup-app");
        // The download sparkjob.py runs on cell1: its own process, and the status file it keeps
        // (what `sparkjob.py list` reads, for real, over SSH).
        let mut worker = std::process::Command::new("sleep").arg("600").spawn().unwrap();
        let (pid, status) = (worker.id(), home.join(".sparkbox/jobs/dl-5e1f00aa.json"));
        std::fs::create_dir_all(status.parent().unwrap()).unwrap();
        let write = |state: &str, done: u64| {
            let st = json!({ "id": "dl-5e1f00aa", "startedAt": now(), "kind": "download", "state": state, "step": "downloading", "message": "",
                             "bytesDone": done, "bytesTotal": 4_000_000_000u64, "rateBps": 50e6, "label": "Download Qwen/Qwen3-8B", "pid": pid, "updatedAt": now() });
            std::fs::write(&status, st.to_string()).unwrap();
        };
        write("running", 1_000_000_000);
        let title = "Download Qwen/Qwen3-8B to cell1";
        let until = |tasks: &Arc<crate::tasks::Tasks>, title: &str, done: fn(&crate::tasks::Task) -> bool| {
            let (tasks, title) = (tasks.clone(), title.to_string());
            async move {
                for _ in 0..200 {
                    if let Some(t) = tasks.list().into_iter().find(|t| t.title == title && done(t)) {
                        return t;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                panic!("{title}: {:?}", tasks.list());
            }
        };

        // The run before: it followed the download, then SparkBox quit.
        let (live1, _) = fake_node::connect(&base, &[("cell1", port)]).await;
        let tasks1 = crate::tasks::Tasks::new(Some(base.join("tasks.jsonl")));
        let m1 = models(&base, live1.clone());
        m1.set_tasks(tasks1.clone());
        let job = Job { id: "dl-5e1f00aa".into(), node: "cell1".into(), kind: "download".into(), state: "running".into(), ..Default::default() };
        m1.follow_job(&job, title.into());
        until(&tasks1, title, |t| t.progress == Some(0.25)).await;
        tasks1.save_now();
        live1.stop();

        // SparkBox starts again: the old task is over, and the first Live round follows the
        // download again (once, however many rounds run).
        let (live, inventory) = fake_node::connect(&base, &[("cell1", port)]).await;
        let tasks = crate::tasks::Tasks::new(Some(base.join("tasks.jsonl")));
        assert!(tasks.list().iter().any(|t| t.title == title && t.finished()), "{:?}", tasks.list());
        let m = models(&base, live.clone());
        m.set_tasks(tasks.clone());
        m.endpoints_tick(&ctx(&live, &inventory), false).await;
        m.endpoints_tick(&ctx(&live, &inventory), false).await;
        let again = "Download Qwen/Qwen3-8B to cell1 (picked up again)";
        let t = until(&tasks, again, |t| t.progress == Some(0.25)).await;
        assert!(t.on_node && !t.finished());
        assert_eq!(tasks.list().iter().filter(|t| t.title == again).count(), 1);

        // It ends with the job, and the next start has nothing to pick up.
        write("done", 4_000_000_000);
        let t = until(&tasks, again, |t| t.finished()).await;
        assert_eq!(t.summary.as_deref(), Some("Finished on cell1"));
        // The follower lets go of its entry right after it finishes the task.
        let mut saved: Vec<Followed> = load(&base.join("jobs-followed.json"));
        for _ in 0..40 {
            if saved.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            saved = load(&base.join("jobs-followed.json"));
        }
        assert!(saved.is_empty(), "{saved:?}");
        let _ = worker.kill();
        live.stop();
    }

    #[tokio::test]
    async fn a_copy_to_another_node_is_an_on_node_task_too() {
        let home = temp("share-task-home");
        let dir = home.join("models/Qwen/Qwen3-8B");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("model.safetensors"), vec![7u8; 4096]).unwrap();
        let key = dir.display().to_string();
        let started: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let ids = started.clone();
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py check") {
                let fabric = json!([{ "iface": "enp1s0f1np1", "ip": "192.168.100.1", "prefix": 24, "speedMbps": 200000, "rdma": "rocep1s0f1" }]);
                return Some((json!({ "home": "/home/sparkuser", "rootExists": true, "docker": true, "fabric": fabric }).to_string(), 0));
            }
            if cmd.contains("sparkjob.py start") {
                let id = cmd.split_whitespace().last().unwrap().to_string();
                ids.lock().unwrap().push(id.clone());
                return Some((json!({ "id": id, "state": "running" }).to_string(), 0));
            }
            if cmd.contains("sparkjob.py list") {
                let jobs: Vec<Value> = ids.lock().unwrap().iter().map(|id| json!({ "id": id, "state": "running", "step": "copying" })).collect();
                return Some((Value::from(jobs).to_string(), 0));
            }
            None
        });
        let (_src, p1) = fake_node(&home, script.clone()).await;
        let (_dst, p2) = fake_node(&temp("share-task-home2"), script).await;
        let base = temp("share-task-app");
        let (live, inventory) = fake_node::connect(&base, &[("cell1", p1), ("cell2", p2)]).await;
        let m = models(&base, live.clone());
        let tasks = crate::tasks::Tasks::new(None);
        m.set_tasks(tasks.clone());
        let jobs = m.share(&ctx(&live, &inventory), &key, &["cell2".into()]).await.unwrap();
        assert_eq!(jobs.len(), 1);
        let t = tasks.list().into_iter().find(|t| t.title.starts_with("Copy ")).expect("a task for the copy");
        assert_eq!((t.title.as_str(), t.nodes.as_slice(), t.on_node), ("Copy Qwen/Qwen3-8B from cell1 to cell2", &["cell2".to_string()][..], true));
        live.stop();
    }

    /// A copy's temporary file server outlives a SparkBox that quit while it served: the next
    /// start's first Live round watches it again (from jobs-shares.json), and once the pull ends
    /// it's stopped (`sparkjob cancel <serve id>` on the source), once, as a titled row. A node
    /// that can't be asked meanwhile stops nothing.
    #[tokio::test]
    async fn a_copys_file_server_is_stopped_after_a_restart_once_its_pulls_end() {
        let home = temp("share-rearm-home");
        let dir = home.join("models/Qwen/Qwen3-8B");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("model.safetensors"), vec![7u8; 4096]).unwrap();
        let key = dir.display().to_string();
        let done = Arc::new(AtomicBool::new(false));
        // A node that can't say how its jobs are (sparkjob fails there for a while).
        let unreadable = Arc::new(AtomicBool::new(false));
        let started: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let (ids, finished, failing) = (started.clone(), done.clone(), unreadable.clone());
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py check") {
                let fabric = json!([{ "iface": "enp1s0f1np1", "ip": "192.168.100.1", "prefix": 24, "speedMbps": 200000, "rdma": "rocep1s0f1" }]);
                return Some((json!({ "home": "/home/sparkuser", "rootExists": true, "docker": true, "fabric": fabric }).to_string(), 0));
            }
            if cmd.contains("sparkjob.py start") {
                let id = cmd.split_whitespace().last().unwrap().to_string();
                ids.lock().unwrap().push(id.clone());
                return Some((json!({ "id": id, "state": "running" }).to_string(), 0));
            }
            if cmd.contains("sparkjob.py list") && failing.load(Ordering::SeqCst) {
                return Some(("!stderr:OSError: [Errno 5] Input/output error: '/home/sparkuser/.sparkbox/jobs'\n".into(), 1));
            }
            if cmd.contains("sparkjob.py list") {
                let state = if finished.load(Ordering::SeqCst) { "done" } else { "running" };
                let jobs: Vec<Value> = ids.lock().unwrap().iter().map(|id| json!({ "id": id, "state": if id.ends_with("-serve") { "running" } else { state } })).collect();
                return Some((Value::from(jobs).to_string(), 0));
            }
            if cmd.contains("sparkjob.py cancel") {
                return Some((json!({ "state": "cancelled" }).to_string(), 0));
            }
            None
        });
        let (src, p1) = fake_node(&home, script.clone()).await;
        let (_dst, p2) = fake_node(&temp("share-rearm-home2"), script).await;
        let base = temp("share-rearm-app");
        let cancels = || src.ran("sparkjob.py cancel");

        // The run before: the copy starts, then SparkBox quits while cell2 still pulls.
        let (live1, inventory) = fake_node::connect(&base, &[("cell1", p1), ("cell2", p2)]).await;
        let m1 = models(&base, live1.clone());
        m1.set_tasks(crate::tasks::Tasks::new(None));
        m1.share(&ctx(&live1, &inventory), &key, &["cell2".into()]).await.unwrap();
        let serve = started.lock().unwrap().iter().find(|id| id.ends_with("-serve")).cloned().expect("a serve job");
        let saved: Vec<Share> = load(&base.join("jobs-shares.json"));
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert_eq!((saved[0].source.as_str(), saved[0].serve_id.as_str()), ("cell1", serve.as_str()));
        assert_eq!(saved[0].pulls, [("cell2".to_string(), serve.replace("-serve", "-cell2"))]);
        live1.stop();
        // Its watcher can't ask anything now: the pull ending is no reason to stop anything yet.
        done.store(true, Ordering::SeqCst);
        tokio::time::sleep(JOB_POLL * 3).await;
        assert!(cancels().is_empty(), "{:?}", cancels());

        // SparkBox starts again: its first Live round watches the server again. While cell2 can't
        // say how its pull is, nothing is stopped; once it can (the pull is done), the server is.
        let (live, inventory) = fake_node::connect(&base, &[("cell1", p1), ("cell2", p2)]).await;
        let m = models(&base, live.clone());
        m.set_tasks(crate::tasks::Tasks::new(None));
        unreadable.store(true, Ordering::SeqCst);
        m.endpoints_tick(&ctx(&live, &inventory), false).await;
        m.endpoints_tick(&ctx(&live, &inventory), false).await;
        tokio::time::sleep(JOB_POLL * 4).await;
        assert!(cancels().is_empty(), "a failed poll stops nothing: {:?}", cancels());
        unreadable.store(false, Ordering::SeqCst);
        for _ in 0..100 {
            if !cancels().is_empty() && load::<Vec<Share>>(&base.join("jobs-shares.json")).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let sent = cancels();
        assert!(sent.len() == 1 && sent[0].ends_with(&format!("sparkjob.py cancel {serve}")), "{sent:?}");
        assert!(load::<Vec<Share>>(&base.join("jobs-shares.json")).is_empty(), "nothing left to pick up");
        assert!(m.serving_nodes().is_empty(), "no work left on cell1 to keep a lease for");
        let row = live.audit.entries().into_iter().find(|e| e.command.contains("sparkjob.py cancel")).expect("its row");
        assert_eq!(row.title, "Stop the file server for the copy of Qwen/Qwen3-8B on cell1");
        tokio::time::sleep(JOB_POLL * 3).await;
        assert_eq!(cancels().len(), 1, "stopped once");
        live.stop();
    }

    /// Look-only that never asks for Touch ID; turning it off answers yes.
    struct Yes;

    impl crate::admin::Authenticator for Yes {
        fn authenticate(&self, _reason: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// A model folder on a fake node's home, as the copy tests share it: its key.
    fn model_home(home: &std::path::Path) -> String {
        let dir = home.join("models/Qwen/Qwen3-8B");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("model.safetensors"), vec![7u8; 4096]).unwrap();
        dir.display().to_string()
    }

    /// A node's sparkjob as the copy tests need it: `check` with a fabric address; `start` of a
    /// job, which runs (a pull fails to start with `full`, as on a full disk); `list` of every job
    /// started, its pulls done once `done`; and `cancel`.
    fn copy_node(started: Arc<std::sync::Mutex<Vec<String>>>, done: Arc<AtomicBool>, full: bool) -> Script {
        Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py check") {
                let fabric = json!([{ "iface": "enp1s0f1np1", "ip": "192.168.100.1", "prefix": 24, "speedMbps": 200000, "rdma": "rocep1s0f1" }]);
                return Some((json!({ "home": "/home/sparkuser", "rootExists": true, "docker": true, "fabric": fabric }).to_string(), 0));
            }
            if full && cmd.contains("sparkjob.py start pull") {
                return Some(("!stderr:OSError: [Errno 28] No space left on device\n".into(), 1));
            }
            if cmd.contains("sparkjob.py start") {
                let id = cmd.split_whitespace().last().unwrap().to_string();
                started.lock().unwrap().push(id.clone());
                return Some((json!({ "id": id, "state": "running" }).to_string(), 0));
            }
            if cmd.contains("sparkjob.py list") {
                let state = if done.load(Ordering::SeqCst) { "done" } else { "running" };
                let jobs: Vec<Value> = started.lock().unwrap().iter().map(|id| json!({ "id": id, "state": if id.ends_with("-serve") { "running" } else { state } })).collect();
                return Some((Value::from(jobs).to_string(), 0));
            }
            if cmd.contains("sparkjob.py cancel") {
                return Some((json!({ "state": "cancelled" }).to_string(), 0));
            }
            None
        })
    }

    fn serve_of(started: &std::sync::Mutex<Vec<String>>) -> String {
        started.lock().unwrap().iter().find(|id| id.ends_with("-serve")).cloned().expect("a serve job")
    }

    /// Waits up to 5 s for `f`.
    async fn until(f: impl Fn() -> bool) {
        for _ in 0..100 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// A copy whose only pull can't start (cell2's disk is full) stops the file server it just
    /// started, at once, as its titled row, and says why: nothing is left serving for 12 h.
    #[tokio::test]
    async fn a_copy_whose_pull_cannot_start_stops_its_file_server_at_once() {
        let home = temp("share-pullfail-home");
        let key = model_home(&home);
        let (started, done) = (Arc::<std::sync::Mutex<Vec<String>>>::default(), Arc::new(AtomicBool::new(false)));
        let (src, p1) = fake_node(&home, copy_node(started.clone(), done.clone(), false)).await;
        let (_dst, p2) = fake_node(&temp("share-pullfail-home2"), copy_node(started.clone(), done, true)).await;
        let base = temp("share-pullfail-app");
        let (live, inventory) = fake_node::connect(&base, &[("cell1", p1), ("cell2", p2)]).await;
        let m = models(&base, live.clone());
        m.set_tasks(crate::tasks::Tasks::new(None));
        let e = m.share(&ctx(&live, &inventory), &key, &["cell2".into()]).await.unwrap_err();
        assert!(e.contains("No space left on device"), "{e}");
        let serve = serve_of(&started);
        let sent = src.ran("sparkjob.py cancel");
        assert!(sent.len() == 1 && sent[0].ends_with(&format!("sparkjob.py cancel {serve}")), "stopped before share() returned: {sent:?}");
        let row = live.audit.entries().into_iter().find(|e| e.command.contains("sparkjob.py cancel")).expect("its row");
        assert_eq!((row.title.as_str(), row.source.as_str(), row.state.as_str()), ("Stop the file server for the copy of Qwen/Qwen3-8B on cell1", "system", "ok"));
        assert!(load::<Vec<Share>>(&base.join("jobs-shares.json")).is_empty(), "nothing left to watch");
        tokio::time::sleep(JOB_POLL * 3).await;
        assert_eq!(src.ran("sparkjob.py cancel").len(), 1, "stopped once");
        live.stop();
    }

    /// The file server is saved as soon as it starts, and each pull as it starts, so a quit at
    /// any point leaves it for the next start to stop. When a later pull can't start, the ones
    /// that did keep their server until they end; then it's stopped, once.
    #[tokio::test]
    async fn a_copy_whose_second_pull_cannot_start_keeps_its_server_for_the_first_then_stops_it() {
        let home = temp("share-pullfail2-home");
        let key = model_home(&home);
        let base = temp("share-pullfail2-app");
        let file = base.join("jobs-shares.json");
        let (started, done) = (Arc::<std::sync::Mutex<Vec<String>>>::default(), Arc::new(AtomicBool::new(false)));
        // What jobs-shares.json said when cell2's pull was asked to start.
        let saved_then: Arc<std::sync::Mutex<Option<Vec<Share>>>> = Arc::default();
        let (inner, seen, at) = (copy_node(started.clone(), done.clone(), false), saved_then.clone(), file.clone());
        let cell2: Script = Arc::new(move |cmd, stdin| {
            if cmd.contains("sparkjob.py start pull") {
                *seen.lock().unwrap() = Some(load(&at));
            }
            inner(cmd, stdin)
        });
        let (src, p1) = fake_node(&home, copy_node(started.clone(), done.clone(), false)).await;
        let (_d2, p2) = fake_node(&temp("share-pullfail2-home2"), cell2).await;
        let (_d3, p3) = fake_node(&temp("share-pullfail2-home3"), copy_node(started.clone(), done.clone(), true)).await;
        let (live, inventory) = fake_node::connect(&base, &[("cell1", p1), ("cell2", p2), ("cell3", p3)]).await;
        let m = models(&base, live.clone());
        let tasks = crate::tasks::Tasks::new(None);
        m.set_tasks(tasks.clone());
        let e = m.share(&ctx(&live, &inventory), &key, &["cell2".into(), "cell3".into()]).await.unwrap_err();
        assert!(e.contains("No space left on device"), "{e}");
        let serve = serve_of(&started);
        let then = saved_then.lock().unwrap().clone().expect("cell2's pull was started");
        assert!(then.len() == 1 && then[0].serve_id == serve && then[0].pulls.is_empty(), "saved before any pull started: {then:?}");
        let saved: Vec<Share> = load(&file);
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert_eq!(saved[0].pulls, [("cell2".to_string(), serve.replace("-serve", "-cell2"))], "the pull that started");
        assert!(tasks.list().iter().any(|t| t.title == "Copy Qwen/Qwen3-8B from cell1 to cell2"), "{:?}", tasks.list());
        // cell2 still pulls: its server stays.
        tokio::time::sleep(JOB_POLL * 3).await;
        assert!(src.ran("sparkjob.py cancel").is_empty(), "{:?}", src.ran("sparkjob.py cancel"));
        done.store(true, Ordering::SeqCst);
        until(|| !src.ran("sparkjob.py cancel").is_empty() && load::<Vec<Share>>(&file).is_empty()).await;
        let sent = src.ran("sparkjob.py cancel");
        assert!(sent.len() == 1 && sent[0].ends_with(&format!("sparkjob.py cancel {serve}")), "{sent:?}");
        assert!(load::<Vec<Share>>(&file).is_empty());
        tokio::time::sleep(JOB_POLL * 3).await;
        assert_eq!(src.ran("sparkjob.py cancel").len(), 1, "stopped once");
        live.stop();
    }

    /// Look-only holds a copy's file server stop: the pull ends, nothing is sent and no Refused
    /// row is written (it would be one every 5 s), and it stays saved. Once look-only is off it's
    /// stopped, once. The watcher's checks are one poll row in the activity log, apart from the
    /// pull's own follower; the stop is a row of its own.
    #[tokio::test]
    async fn look_only_holds_a_copys_file_server_stop_until_it_ends() {
        let home = temp("share-held-home");
        let key = model_home(&home);
        let (started, done) = (Arc::<std::sync::Mutex<Vec<String>>>::default(), Arc::new(AtomicBool::new(false)));
        let (src, p1) = fake_node(&home, copy_node(started.clone(), done.clone(), false)).await;
        let (_dst, p2) = fake_node(&temp("share-held-home2"), copy_node(started.clone(), done.clone(), false)).await;
        let base = temp("share-held-app");
        let file = base.join("jobs-shares.json");
        let (live, inventory) = fake_node::connect(&base, &[("cell1", p1), ("cell2", p2)]).await;
        let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
        live.set_write_policy(gate.clone());
        let m = models(&base, live.clone());
        m.set_gate(gate.clone());
        m.set_tasks(crate::tasks::Tasks::new(None));
        m.share(&ctx(&live, &inventory), &key, &["cell2".into()]).await.unwrap();
        let serve = serve_of(&started);
        let refused = || live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect::<Vec<_>>();

        gate.set_look_only(true).unwrap();
        done.store(true, Ordering::SeqCst);
        tokio::time::sleep(JOB_POLL * 6).await;
        assert!(src.ran("sparkjob.py cancel").is_empty(), "{:?}", src.ran("sparkjob.py cancel"));
        assert!(refused().is_empty(), "{:?}", refused());
        assert_eq!(load::<Vec<Share>>(&file).len(), 1, "still saved: a quit now leaves it for the next start");

        gate.set_look_only(false).unwrap();
        until(|| !src.ran("sparkjob.py cancel").is_empty() && load::<Vec<Share>>(&file).is_empty()).await;
        let sent = src.ran("sparkjob.py cancel");
        assert!(sent.len() == 1 && sent[0].ends_with(&format!("sparkjob.py cancel {serve}")), "{sent:?}");
        tokio::time::sleep(JOB_POLL * 3).await;
        assert_eq!(src.ran("sparkjob.py cancel").len(), 1, "stopped once");
        assert!(refused().is_empty(), "{:?}", refused());

        // The pull's follower and the watcher ask cell2 the same thing at the same moment: Live
        // runs it once for both, as one row. Either way, each row is one of their two polls.
        let rows = live.audit.entries();
        let lists: Vec<_> = rows.iter().filter(|e| e.command.contains("sparkjob.py list")).collect();
        let ops: std::collections::BTreeSet<(&str, &str)> = lists.iter().map(|e| (e.op_id.as_deref().unwrap_or(""), e.title.as_str())).collect();
        let polls = ["Follow copy Qwen/Qwen3-8B from cell1 to cell2", "Watch the file server for the copy of Qwen/Qwen3-8B on cell1"];
        assert!(ops.len() <= 2 && ops.iter().all(|(op, title)| op.starts_with("poll-") && polls.contains(title)), "{ops:?}");
        assert!(!lists.is_empty() && lists.iter().all(|e| e.source == "poll" && e.class == "probe"), "{lists:?}");
        let stop = rows.iter().find(|e| e.command.contains("sparkjob.py cancel")).expect("its row");
        assert_eq!((stop.title.as_str(), stop.source.as_str()), ("Stop the file server for the copy of Qwen/Qwen3-8B on cell1", "system"));
        assert_ne!(stop.class, "probe");
        live.stop();
    }

    /// A saved file server past its 12 h TTL has stopped by itself: picking up drops it from
    /// jobs-shares.json, and nothing asks about its pull or stops it. One within its TTL is kept
    /// and watched.
    #[tokio::test]
    async fn a_saved_file_server_past_its_ttl_is_dropped_at_pick_up_and_never_asked_about() {
        let (started, done) = (Arc::<std::sync::Mutex<Vec<String>>>::default(), Arc::new(AtomicBool::new(false)));
        let (src, p1) = fake_node(&temp("share-ttl-home"), copy_node(started.clone(), done.clone(), false)).await;
        let (old_pull, p2) = fake_node(&temp("share-ttl-home2"), copy_node(started.clone(), done.clone(), false)).await;
        let (new_pull, p3) = fake_node(&temp("share-ttl-home3"), copy_node(started.clone(), done, false)).await;
        let base = temp("share-ttl-app");
        let file = base.join("jobs-shares.json");
        let share = |id: &str, pull: &str, age: u64| Share {
            source: "cell1".into(),
            serve_id: format!("share-{id}-serve"),
            pulls: vec![(pull.to_string(), format!("share-{id}-{pull}"))],
            started_at: now() - age,
            repo: "Qwen/Qwen3-8B".into(),
        };
        let (old, fresh) = (share("0ld", "cell2", SERVE_TTL_S + 60), share("f7e5", "cell3", 60));
        started.lock().unwrap().extend([old.pulls[0].1.clone(), fresh.pulls[0].1.clone()]);
        save(&file, &vec![old, fresh.clone()]);
        let (live, inventory) = fake_node::connect(&base, &[("cell1", p1), ("cell2", p2), ("cell3", p3)]).await;
        let m = models(&base, live.clone());
        assert_eq!(m.serving_nodes(), ["cell1"], "the one within its TTL, of the two saved");
        m.pick_up_shares(&ctx(&live, &inventory));
        assert_eq!(load::<Vec<Share>>(&file), [fresh.clone()], "dropped as it was picked up");
        until(|| !new_pull.ran("sparkjob.py list").is_empty()).await;
        tokio::time::sleep(JOB_POLL * 3).await;
        assert!(!new_pull.ran("sparkjob.py list").is_empty(), "the one within its TTL is watched");
        assert!(old_pull.ran("sparkjob.py list").is_empty(), "{:?}", old_pull.ran("sparkjob.py list"));
        assert!(src.ran("sparkjob.py cancel").is_empty(), "{:?}", src.ran("sparkjob.py cancel"));
        assert_eq!(load::<Vec<Share>>(&file), [fresh]);
        // The watcher's checks are one poll in the activity log.
        let rows: Vec<_> = live.audit.entries().into_iter().filter(|e| e.command.contains("sparkjob.py list")).collect();
        let ops: std::collections::BTreeSet<Option<String>> = rows.iter().map(|e| e.op_id.clone()).collect();
        assert!(rows.len() >= 2 && ops.len() == 1 && ops.iter().all(|o| o.as_deref().is_some_and(|o| o.starts_with("poll-share-f7e5-serve-"))), "{rows:?}");
        assert!(rows.iter().all(|e| (e.title.as_str(), e.source.as_str(), e.class.as_str(), e.node.as_str()) == ("Watch the file server for the copy of Qwen/Qwen3-8B on cell1", "poll", "probe", "cell3")), "{rows:?}");
        live.stop();
    }

    /// The Jobs tab's reads and a followed download's are SparkBox's polls in the activity log:
    /// each Jobs-tab round is one row ("Check jobs", every node), and a followed job is one row
    /// for all its rounds ("Follow download … to cell1").
    #[tokio::test]
    async fn the_jobs_tab_and_a_followed_job_each_poll_as_one_row() {
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let script: Script = Arc::new(move |cmd, _| {
            if !cmd.contains("sparkjob.py list") {
                return None;
            }
            let state = if finished.load(Ordering::SeqCst) { "done" } else { "running" };
            Some((json!([{ "id": "dl-5e1f00aa", "kind": "download", "state": state, "bytesDone": 1, "bytesTotal": 4, "startedAt": 1 }]).to_string(), 0))
        });
        let (cell1, p1) = fake_node(&temp("jobs-poll-home"), script.clone()).await;
        let (_cell2, p2) = fake_node(&temp("jobs-poll-home2"), script).await;
        let base = temp("jobs-poll-app");
        let (live, inventory) = fake_node::connect(&base, &[("cell1", p1), ("cell2", p2)]).await;
        let m = models(&base, live.clone());
        let tasks = crate::tasks::Tasks::new(None);
        m.set_tasks(tasks.clone());
        let lists = || live.audit.entries().into_iter().filter(|e| e.command.contains("sparkjob.py list")).collect::<Vec<_>>();
        let by_op = |rows: &[crate::live::audit::Entry]| {
            let mut ops: BTreeMap<String, Vec<crate::live::audit::Entry>> = BTreeMap::new();
            for e in rows {
                ops.entry(e.op_id.clone().unwrap_or_default()).or_default().push(e.clone());
            }
            ops
        };

        for _ in 0..2 {
            assert_eq!(m.jobs(&ctx(&live, &inventory)).await.unwrap().len(), 2);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let tab = by_op(&lists());
        assert_eq!(tab.len(), 2, "one per round: {tab:?}");
        for (op, rows) in &tab {
            assert!(op.starts_with("poll-jobs-"), "{op}");
            let nodes: std::collections::BTreeSet<&str> = rows.iter().map(|e| e.node.as_str()).collect();
            assert_eq!(nodes.into_iter().collect::<Vec<_>>(), ["cell1", "cell2"]);
            assert!(rows.iter().all(|e| (e.title.as_str(), e.source.as_str(), e.class.as_str()) == ("Check jobs", "poll", "probe")), "{rows:?}");
        }

        let job = Job { id: "dl-5e1f00aa".into(), node: "cell1".into(), kind: "download".into(), state: "running".into(), ..Default::default() };
        m.follow_job(&job, "Download Qwen/Qwen3-8B to cell1".into());
        let before = cell1.ran("sparkjob.py list").len();
        until(|| cell1.ran("sparkjob.py list").len() >= before + 3).await;
        done.store(true, Ordering::SeqCst);
        until(|| tasks.list().iter().any(|t| t.title == "Download Qwen/Qwen3-8B to cell1" && t.finished())).await;
        let follow: Vec<_> = lists().into_iter().filter(|e| !tab.contains_key(e.op_id.as_deref().unwrap_or_default())).collect();
        let ops = by_op(&follow);
        assert_eq!(ops.len(), 1, "one row for every round: {ops:?}");
        let (op, rows) = ops.into_iter().next().unwrap();
        assert!(op.starts_with("poll-follow-") && rows.len() >= 4, "{op}: {} rounds", rows.len());
        assert!(rows.iter().all(|e| (e.title.as_str(), e.source.as_str(), e.class.as_str(), e.node.as_str()) == ("Follow download Qwen/Qwen3-8B to cell1", "poll", "probe", "cell1")), "{rows:?}");
        live.stop();
    }
}
