//! Every inference server on the cluster, whoever started it. Three sources are
//! merged: SparkBox's own loads, servers found in the snapshot (containers and
//! processes running `vllm serve`, `ollama serve`, `llama-server`, SGLang,
//! `trtllm-serve` or an NVIDIA NIM, with Ray workers attached to their head),
//! and endpoints added by hand. A load SparkBox started through launch.py says what it
//! serves on its labels instead: its Command is only `python3 /run/sparkbox/launch.py`.
//! The pure parts live here; Models runs the probes and keeps the metrics (watch.rs).
//!
//! Health is derived from evidence, never assumed (`assess`). A model is Ready
//! only when every node it needs is online and reporting, every container behind
//! it is running, and it answered a probe in the last minute. A head that has
//! exited stays listed as Down with its exit code, and so does a server that
//! vanished, for a day. Anything else that is starting and making progress is
//! Loading; a crash loop never is.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::metrics::{EndpointMetrics, OllamaLoaded};
use super::plan::Stage;
use super::readiness::{Protocols, Readiness};
use super::{Deployment, DeploymentState};
use crate::inventory::Inventory;
use crate::llm::{Provider, ProviderKind};
use crate::model::{ClusterSnapshot, Container, LinkStatus, NetRole, NodeSnapshot};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Vllm,
    Ollama,
    Sglang,
    Llamacpp,
    Trtllm,
    Nim,
    #[default]
    Other,
}

impl Engine {
    pub fn slug(self) -> &'static str {
        match self {
            Engine::Vllm => "vllm",
            Engine::Ollama => "ollama",
            Engine::Sglang => "sglang",
            Engine::Llamacpp => "llamacpp",
            Engine::Trtllm => "trtllm",
            Engine::Nim => "nim",
            Engine::Other => "other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Engine::Vllm => "vLLM",
            Engine::Ollama => "Ollama",
            Engine::Sglang => "SGLang",
            Engine::Llamacpp => "llama.cpp",
            Engine::Trtllm => "TensorRT-LLM",
            Engine::Nim => "NIM",
            Engine::Other => "Engine",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Engine::Vllm | Engine::Trtllm | Engine::Nim => 8000,
            Engine::Ollama => 11434,
            Engine::Sglang => 30000,
            Engine::Llamacpp => 8080,
            Engine::Other => 0,
        }
    }

    fn from_port(port: u16) -> Engine {
        [Engine::Vllm, Engine::Ollama, Engine::Sglang, Engine::Llamacpp].into_iter().find(|e| e.default_port() == port).unwrap_or(Engine::Other)
    }

    /// Engines that publish Prometheus metrics SparkBox reads (metrics.rs).
    pub fn has_metrics(self) -> bool {
        matches!(self, Engine::Vllm | Engine::Sglang | Engine::Trtllm | Engine::Nim)
    }

    /// The path that lists its models (a probe that proves it answers).
    pub fn models_path(self) -> &'static str {
        if self == Engine::Ollama {
            "/api/tags"
        } else {
            "/v1/models"
        }
    }

    /// The engine's own health check: 200 while its engine works. vLLM's API server keeps
    /// answering /v1/models after its engine died; /health says 503 then.
    pub fn health_path(self) -> Option<&'static str> {
        match self {
            Engine::Vllm | Engine::Sglang | Engine::Trtllm | Engine::Llamacpp => Some("/health"),
            Engine::Nim => Some("/v1/health/ready"),
            Engine::Ollama | Engine::Other => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedBy {
    Sparkbox,
    #[default]
    External,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ready,
    #[default]
    Loading,
    Down,
    /// The node cannot currently supply fresh evidence; this is not proof the engine failed.
    Unknown,
    /// Stopped on purpose (exit 0, or a clean `docker stop`), not broken.
    Stopped,
    /// It answers, with nothing in memory to generate with (an Ollama that has loaded nothing):
    /// healthy at rest, never tested, never Down.
    Idle,
    /// Not found on the cluster for a while (every source missed it): neutral, since vanishing
    /// is no evidence that it broke.
    Gone,
    /// A node it needs is in a maintenance window SparkBox opened (a planned restart).
    Paused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Deployment,
    #[default]
    Discovered,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parallel {
    pub tp: u64,
    pub pp: u64,
}

impl Parallel {
    /// Labels and external argv are untrusted. Never wrap a rank count or narrow it with `as`.
    pub fn ranks(self) -> Option<usize> {
        self.tp.checked_mul(self.pp).filter(|n| *n > 0).and_then(|n| usize::try_from(n).ok())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupervisorState {
    #[default]
    Watching,
    Restarting,
    GaveUp,
}

/// Keep running for a multi-node load: what the supervisor is doing (supervisor.rs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorView {
    pub enabled: bool,
    pub state: SupervisorState,
    /// Restarts in the last 30 minutes; once it gave up, the count it gave up at.
    pub restarts: u32,
    /// Unix ms of the next restart, while one is scheduled.
    pub next_retry_ms: Option<u64>,
    /// "Held · look-only" while look-only holds it, "Held · locked" after a relock dropped a
    /// retry, or why it never restarts this load (supervisor.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
}

/// One container behind an endpoint, as the snapshot shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Member {
    pub node: String,
    pub head: bool,
    pub id: String,
    pub name: String,
    /// Docker state: running | restarting | exited | created | paused | dead.
    pub state: String,
    pub exit_code: i32,
    pub finished_at: u64,
    pub started_at: u64,
    pub oom: bool,
    pub restart_count: u32,
    /// Its place in the pipeline, 0 for the head: SparkBox's `sparkbox.rank` label, or SGLang's
    /// --node-rank. Ray workers have none (their order is remembered instead).
    pub stage: Option<u32>,
}

impl Member {
    fn of(node: &str, head: bool, c: &Container) -> Member {
        let launched = num(flag(&words(&c.command), &["--node-rank"])).and_then(|r| u32::try_from(r).ok());
        Member {
            node: node.into(),
            head,
            id: c.id.clone(),
            name: c.name.clone(),
            state: c.state.clone(),
            exit_code: c.exit_code,
            finished_at: c.finished_at,
            started_at: c.started_at,
            oom: c.oom_killed,
            restart_count: c.restart_count,
            stage: if head { Some(0) } else { c.labels.get("sparkbox.rank").and_then(|r| r.parse().ok()).or(launched) },
        }
    }

    pub fn running(&self) -> bool {
        self.state == "running"
    }

    /// How alive a state is, for picking between two containers with the same endpoint id.
    fn rank(&self) -> u8 {
        match self.state.as_str() {
            "running" => 0,
            "restarting" => 1,
            "paused" => 2,
            "created" => 3,
            _ => 4,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    /// `{node}-{engine}-{port}`: also the provider id of a discovered endpoint.
    pub id: String,
    pub engine: Engine,
    /// Names clients put in "model".
    pub models: Vec<String>,
    /// Every node it runs on, head first (the expected ones, present or not).
    pub nodes: Vec<String>,
    pub head_node: String,
    pub port: u16,
    /// OpenAI-style base URL (ends in /v1).
    pub url: String,
    pub managed_by: ManagedBy,
    pub health: Health,
    /// Why it isn't Ready, in a sentence: "Pipeline stage 3 of 3 on cell3 is offline".
    pub health_reason: Option<String>,
    pub parallel: Option<Parallel>,
    pub container_ids: Vec<String>,
    pub container_names: Vec<String>,
    pub deployment_id: Option<String>,
    pub metrics: Option<EndpointMetrics>,
    /// "server": the engine's own /metrics; "sparkbox": only requests SparkBox sent or relayed.
    pub metrics_source: Option<String>,
    pub provider_id: Option<String>,
    pub max_context: Option<u64>,
    /// Weight loading progress while Loading (from the engine's log, or the simulator).
    pub loading_pct: Option<u64>,
    /// Unix ms of the last probe it answered.
    pub last_ok_ms: Option<u64>,
    /// The nodes it needs: tensor × pipeline parallel of them.
    pub expected_nodes: Vec<String>,
    /// Expected nodes that are offline, not reporting, or not running their part.
    pub missing_nodes: Vec<String>,
    /// Container restarts behind it (Docker's count), when any.
    pub restarts: Option<u32>,
    pub supervisor: Option<SupervisorView>,
    /// The same as health_reason while it isn't Ready (older UI reads this).
    pub message: Option<String>,
    /// SparkBox didn't start it and it isn't running (its head exited, or it vanished): Forget drops it.
    pub forgettable: bool,
    pub ollama: Vec<OllamaLoaded>,
    /// "all" | "lan" | "fabric" | "local" | "unknown".
    pub listen: String,
    pub source: Source,
    pub probed_ms: Option<u64>,
    /// The model folder or repo it serves, when known (the Library marks it "in use").
    pub model_path: Option<String>,
    /// The last one-token test (`canary`): when, whether a token came back, and how long it took.
    pub canary: Option<Canary>,
    /// Unix ms when live traffic last proved it generates (its token counter rose and /health
    /// was fine), so no test token was needed.
    pub passive_ok_ms: Option<u64>,
    /// Requests are running but no token has come out for this long (s).
    pub stalled_s: Option<u64>,
    /// A note while it's Ready: "Slow: 1 token took 12.4 s".
    pub health_note: Option<String>,
    // ── r4-serving ──
    /// What the UI calls it: the served name ("glm-5.3-flash"), or "Ollama on cell3".
    pub label: String,
    /// Who can reach it: "node-only" (loopback), "lan", "fabric" or "unknown".
    pub reach: String,
    /// The models an Ollama keeps on disk (/api/tags), and their sizes in the same order.
    pub on_disk: Vec<String>,
    pub on_disk_bytes: Vec<u64>,
    /// Unix ms it was last found, while Gone.
    pub gone_since_ms: Option<u64>,
    // ── p1r4 readiness ──
    /// The image it runs: a SparkBox load's `sparkbox.image` label (the name it was picked by),
    /// else the container's own image (an id when it was started by one).
    pub image: Option<String>,
    /// A SparkBox load's last readiness check (P1.35).
    pub readiness: Option<Readiness>,
    /// The native APIs its /openapi.json lists (P1.37), from its load.
    pub protocols: Option<Protocols>,
    // ── p2r1 progress ──
    /// vLLM's count of requests with NaNs in their logits, when it keeps one (metrics.rs).
    pub corrupted_requests: Option<f64>,
    /// Its log says it's compiling, every rank it has is up, and the log has moved within
    /// START_LIMIT_MS: Keep running leaves it be.
    #[serde(skip)]
    pub compiling: bool,
    /// A Live SparkBox load: Ready only once its readiness check passed, never on /health alone.
    #[serde(skip)]
    pub needs_readiness: bool,
    /// A Live SparkBox load an older SparkBox made Ready, before readiness checks: 17+25 was never asked.
    #[serde(skip)]
    pub ready_before_readiness: bool,
    /// Address to probe from the head node itself.
    #[serde(skip)]
    pub probe_host: String,
    /// The containers behind it, head first (the evidence `assess` reads).
    #[serde(skip)]
    pub members: Vec<Member>,
    /// Its port shows in the head node's listeners (or the node doesn't report listeners).
    #[serde(skip)]
    pub listening: bool,
    /// A SparkBox load's own state and stage.
    #[serde(skip)]
    pub deployment: Option<(DeploymentState, Option<Stage>, String, u64)>,
    /// Keep running may restart it once it's Failed (`Deployment::recoverable`).
    #[serde(skip)]
    pub recoverable: bool,
    #[serde(skip)]
    pub recovery_pending: bool,
    #[serde(default)]
    pub maintenance_hold: bool,
    /// Not in the snapshot any more: listed from memory (watch.rs).
    #[serde(skip)]
    pub gone: bool,
    /// No source found it this round, but it hasn't been missing long enough to be Gone: listed
    /// (and probed) as last seen.
    #[serde(skip)]
    pub recalled: bool,
}

/// One test request for a single token (`/v1/completions` with max_tokens 1, or Ollama's
/// `/api/generate` with num_predict 1): the proof that Ready means it generates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Canary {
    pub at_ms: u64,
    pub ok: bool,
    /// How long the one token took, request to reply (ms).
    pub latency_ms: Option<u64>,
    /// Why no token came back: "HTTP 500", "no reply in 30 s".
    pub error: Option<String>,
    /// How many endpoint rounds had run when it was sent: an operation's check (verify.rs) counts
    /// only a token sent once a round that began after the operation had run. Not saved.
    #[serde(skip)]
    pub after_round: Option<u64>,
}

/// What a server's command line says about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub engine: Engine,
    pub served: Option<String>,
    pub model: Option<String>,
    pub port: Option<u16>,
    pub host: Option<String>,
    pub tp: u64,
    pub pp: u64,
    pub max_len: Option<u64>,
}

fn words(cmd: &str) -> Vec<String> {
    cmd.split(|c: char| c.is_whitespace() || c == ';')
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn flag<'a>(w: &'a [String], names: &[&str]) -> Option<&'a str> {
    for (i, x) in w.iter().enumerate() {
        for n in names {
            if x == n {
                return w.get(i + 1).map(|s| s.as_str()).filter(|v| !v.starts_with('-') || v.parse::<f64>().is_ok());
            }
            if let Some(v) = x.strip_prefix(n).and_then(|r| r.strip_prefix('=')) {
                return Some(v);
            }
        }
    }
    None
}

fn num(v: Option<&str>) -> Option<u64> {
    v.and_then(|s| s.parse().ok())
}

fn port(v: Option<&str>) -> Option<u16> {
    num(v).and_then(|p| u16::try_from(p).ok())
}

/// Reads an engine's command line: `vllm serve <model> --port 8000 -tp 2 …`, the OpenAI
/// api_server module, `python -m sglang.launch_server`, `trtllm-serve`, `llama-server`,
/// `ollama serve`.
pub fn parse_command(cmd: &str) -> Option<Parsed> {
    let w = words(cmd);
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    let serve = w.windows(2).position(|p| base(&p[0]) == "vllm" && p[1] == "serve");
    let module = w.iter().position(|x| x == "vllm.entrypoints.openai.api_server");
    if let Some(start) = serve.or(module) {
        // Only vLLM's own options count (a wrapper script may run `ray start --port=…` first).
        let w = &w[start..];
        // A rank above 0 of a multi-node load serves no API. Its container's labels say so, but
        // its process can show up in the sample before its container does.
        if w.iter().any(|x| x == "--headless") {
            return None;
        }
        let model = match serve {
            Some(_) => w.get(2).filter(|m| !m.starts_with('-')).map(|s| s.as_str()).or_else(|| flag(w, &["--model"])),
            None => flag(w, &["--model"]),
        };
        return Some(Parsed {
            engine: Engine::Vllm,
            served: flag(w, &["--served-model-name"]).map(String::from),
            model: model.map(String::from),
            port: port(flag(w, &["--port"])),
            host: flag(w, &["--host"]).map(String::from),
            tp: num(flag(w, &["--tensor-parallel-size", "-tp"])).unwrap_or(1).max(1),
            pp: num(flag(w, &["--pipeline-parallel-size", "-pp"])).unwrap_or(1).max(1),
            max_len: num(flag(w, &["--max-model-len"])),
        });
    }
    if w.iter().any(|x| x == "sglang.launch_server") {
        // Multi-node SGLang runs one launch_server per node; --node-rank 0 serves the API.
        if num(flag(&w, &["--node-rank"])).is_some_and(|r| r > 0) {
            return None;
        }
        return Some(Parsed {
            engine: Engine::Sglang,
            served: flag(&w, &["--served-model-name"]).map(String::from),
            model: flag(&w, &["--model-path", "--model"]).map(String::from),
            port: port(flag(&w, &["--port"])),
            host: flag(&w, &["--host"]).map(String::from),
            tp: num(flag(&w, &["--tp", "--tp-size", "--tensor-parallel-size"])).unwrap_or(1).max(1),
            pp: num(flag(&w, &["--pp", "--pp-size"])).unwrap_or(1).max(1),
            max_len: num(flag(&w, &["--context-length"])),
        });
    }
    if let Some(start) = w.iter().position(|x| base(x) == "trtllm-serve") {
        // `trtllm-serve [serve] <model> --tp_size 2 --port 8000`, often under mpirun.
        let w = &w[start..];
        let model = w.iter().skip(1).find(|x| x.as_str() != "serve" && !x.starts_with('-')).cloned();
        return Some(Parsed {
            engine: Engine::Trtllm,
            served: None,
            model,
            port: port(flag(w, &["--port"])),
            host: flag(w, &["--host"]).map(String::from),
            tp: num(flag(w, &["--tp_size", "--tp-size"])).unwrap_or(1).max(1),
            pp: num(flag(w, &["--pp_size", "--pp-size"])).unwrap_or(1).max(1),
            max_len: num(flag(w, &["--max_seq_len", "--max-seq-len"])),
        });
    }
    if w.first().is_some_and(|x| base(x) == "llama-server") {
        return Some(Parsed {
            engine: Engine::Llamacpp,
            served: flag(&w, &["--alias", "-a"]).map(String::from),
            model: flag(&w, &["--model", "-m", "--hf-repo", "-hf"]).map(String::from),
            port: port(flag(&w, &["--port"])),
            host: flag(&w, &["--host"]).map(String::from),
            tp: 1,
            pp: 1,
            max_len: num(flag(&w, &["--ctx-size", "-c"])),
        });
    }
    if w.len() >= 2 && base(&w[0]) == "ollama" && w[1] == "serve" {
        return Some(Parsed { engine: Engine::Ollama, tp: 1, pp: 1, ..Default::default() });
    }
    None
}

/// A rank of a SparkBox load that serves no API: it joins its head by `sparkbox.deployment`.
fn labelled_worker(c: &Container) -> bool {
    c.labels.contains_key("sparkbox.path") && c.labels.get("sparkbox.role").is_some_and(|r| r == "worker")
}

/// What a load started through launch.py serves, from its labels (plan::rank_command).
fn from_labels(c: &Container) -> Option<Parsed> {
    let label = |k: &str| c.labels.get(k).map(String::as_str);
    let model = label("sparkbox.path")?;
    let (tp, pp) = label("sparkbox.parallel").map(|p| p.split_once('x').unwrap_or(("0", "0"))).unwrap_or(("1", "1"));
    Some(Parsed {
        engine: Engine::Vllm,
        served: label("sparkbox.model").map(String::from),
        model: Some(model.to_string()),
        port: port(label("sparkbox.port")),
        host: label("sparkbox.listen").map(String::from),
        tp: tp.parse().unwrap_or(0),
        pp: pp.parse().unwrap_or(0),
        max_len: num(label("sparkbox.maxlen")),
    })
}

/// A container's engine: SparkBox's labels, its command line, or an NVIDIA NIM image
/// (`nvcr.io/nim/<org>/<model>:<tag>`), whose entrypoint starts the server on port 8000 and
/// serves the model the image is named after.
pub fn parse_container(c: &Container) -> Option<Parsed> {
    if labelled_worker(c) {
        return None;
    }
    if let Some(p) = from_labels(c) {
        return Some(p);
    }
    if let Some(p) = parse_command(&c.command) {
        return Some(p);
    }
    let rest = c.image.strip_prefix("nvcr.io/nim/")?;
    let repo = rest.split(['@', ':']).next().unwrap_or(rest);
    Some(Parsed { engine: Engine::Nim, served: Some(repo.to_string()), model: Some(repo.to_string()), tp: 1, pp: 1, ..Default::default() })
}

/// "org/name" from a model path like /models/org/name or …/models--org--name/snapshots/<rev>.
pub fn repo_of(path: &str) -> Option<String> {
    if let Some(i) = path.find("models--") {
        let folder = path[i + 8..].split('/').next()?;
        return Some(folder.replacen("--", "/", 1));
    }
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    if path.starts_with('/') && parts.len() >= 2 {
        return Some(format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]));
    }
    (parts.len() == 2).then(|| parts.join("/"))
}

/// The host port a container publishes for `inner` (or its first one).
fn published(ports: &[String], inner: u16) -> Option<u16> {
    let parse = |p: &str| -> Option<(u16, u16)> {
        let (host, rest) = p.split_once("->")?;
        let h = host.rsplit(':').next()?.parse().ok()?;
        let i = rest.split('/').next()?.parse().ok()?;
        Some((h, i))
    };
    let all: Vec<(u16, u16)> = ports.iter().filter_map(|p| parse(p)).collect();
    all.iter().find(|(_, i)| *i == inner).or(all.first()).map(|(h, _)| *h)
}

fn lan_ip(node: &NodeSnapshot, inventory: &Inventory) -> String {
    node.nets
        .iter()
        .find(|n| n.role == NetRole::Lan && n.ip.is_some())
        .and_then(|n| n.ip.clone())
        .or_else(|| inventory.nodes.iter().find(|c| c.id == node.id).and_then(|c| c.ip.clone()).filter(|ip| !ip.is_empty()))
        .unwrap_or_else(|| node.host.clone())
}

fn node_ips(node: &NodeSnapshot, inventory: &Inventory) -> Vec<String> {
    let mut ips: Vec<String> = node.nets.iter().filter_map(|n| n.ip.clone()).collect();
    ips.push(node.host.clone());
    ips.push(node.id.clone());
    if let Some(c) = inventory.nodes.iter().find(|c| c.id == node.id) {
        ips.extend(c.ip.clone());
        ips.push(c.host.clone());
    }
    ips
}

/// How a bind address reads: everywhere, LAN, fabric or loopback only.
fn listen_kind(addr: &str, node: &NodeSnapshot) -> &'static str {
    match addr {
        "0.0.0.0" | "::" | "*" | "[::]" | "" => "all",
        "127.0.0.1" | "::1" | "localhost" => "local",
        a if a.starts_with("192.168.100.") || a.starts_with("192.168.101.") => "fabric",
        a if node.nets.iter().any(|n| n.role == NetRole::Lan && n.ip.as_deref() == Some(a)) => "lan",
        _ => "unknown",
    }
}

/// Exited servers stay listed (as Down) for a day, then age out.
pub const KEEP_GONE_S: u64 = 24 * 3600;

/// Whether a container counts: anything not exited, or exited within the last day.
fn recent(c: &Container, now: u64) -> bool {
    !matches!(c.state.as_str(), "exited" | "dead") || now.saturating_sub(c.finished_at) < KEEP_GONE_S || c.labels.contains_key("sparkbox.deployment")
}

/// (engine, rendezvous host, rendezvous port, Ray). Never confuse the API port with
/// the distributed rendezvous port. IPv6 brackets are retained just like node addresses.
fn worker_rendezvous(cmd: &str) -> Option<(Engine, String, Option<u16>, bool)> {
    let w = words(cmd);
    let address = |a: &str| match a.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()) {
        Some((host, port)) => (host.to_string(), port.parse().ok()),
        None => (a.to_string(), None),
    };
    if cmd.contains("ray start") && !w.iter().any(|x| x == "--head") {
        let (host, port) = address(flag(&w, &["--address"])?);
        Some((Engine::Vllm, host, port, true))
    } else if cmd.contains("sglang.launch_server") && num(flag(&w, &["--node-rank"])).is_some_and(|r| r > 0) {
        let (host, port) = address(flag(&w, &["--dist-init-addr"])?);
        Some((Engine::Sglang, host, port, false))
    } else if w.iter().any(|x| x == "--headless") {
        Some((Engine::Vllm, flag(&w, &["--master-addr"])?.to_string(), flag(&w, &["--master-port"]).and_then(|p| p.parse().ok()), false))
    } else { None }
}

/// Servers found in the snapshot, with their Ray workers attached. Heads that exited in the
/// last day are kept (with their exit code) so a crash doesn't make a model vanish.
pub fn discover(snap: &ClusterSnapshot, inventory: &Inventory) -> Vec<Endpoint> {
    let now = snap.ts_ms / 1000;
    let mut out: Vec<Endpoint> = Vec::new();
    // A worker's address alone doesn't identify a deployment: two servers can share a head.
    // Labels are authoritative, including the instance. Unlabelled workers need an unambiguous
    // engine/rendezvous match; incomplete metadata must never attach a worker to two loads.
    let workers: Vec<(&NodeSnapshot, &Container)> = snap.nodes.iter().flat_map(|n| {
        n.containers.iter().filter(move |c| recent(c, now)).filter(|c| {
            labelled_worker(c) || worker_rendezvous(&c.command).is_some()
        }).map(move |c| (n, c))
    }).collect();

    for n in &snap.nodes {
        let ips = node_ips(n, inventory);
        let mut found: Vec<(Parsed, Vec<u16>, Option<&Container>, &str)> = Vec::new();
        for c in n.containers.iter().filter(|c| recent(c, now)) {
            if let Some(p) = parse_container(c) {
                found.push((p, Vec::new(), Some(c), &c.command));
            }
        }
        if n.status == LinkStatus::Online {
            for p in n.processes.iter().filter(|p| p.container.is_none()) {
                if let Some(parsed) = parse_command(&p.cmd) {
                    found.push((parsed, p.ports.clone(), None, &p.cmd));
                }
            }
            // A listener on Ollama's own port is Ollama, found or not in the process sample: as a
            // systemd service (user `ollama`, 127.0.0.1) ss shows it with no owner, and its process
            // is idle, so most samples leave it out. (Other engines' ports are too common to say.)
            let ollama = Engine::Ollama.default_port();
            if !found.iter().any(|(p, ..)| p.engine == Engine::Ollama) && n.listeners.iter().any(|l| l.port == ollama) {
                found.push((Parsed { engine: Engine::Ollama, tp: 1, pp: 1, ..Default::default() }, vec![ollama], None, ""));
            }
        }
        let matches = |p: &Parsed, c: Option<&Container>, cmd: &str, wc: &Container| {
            let label = |c: &Container| c.labels.get("sparkbox.deployment").filter(|x| !x.is_empty()).cloned();
            let head_label = c.and_then(label);
            let worker_label = label(wc);
            if head_label.is_some() || worker_label.is_some() {
                return head_label.is_some() && head_label == worker_label
                    && c.and_then(|c| c.labels.get("sparkbox.instance")) == wc.labels.get("sparkbox.instance");
            }
            let Some((engine, host, worker_port, ray)) = worker_rendezvous(&wc.command) else { return false };
            if p.engine != engine || !ips.contains(&host) || (p.tp == 1 && p.pp == 1) { return false; }
            let w = words(cmd);
            if ray && (flag(&w, &["--master-addr"]).is_some() || flag(&w, &["--distributed-executor-backend"]) == Some("mp")) { return false; }
            if !ray && engine == Engine::Vllm && (cmd.contains("ray start") || flag(&w, &["--distributed-executor-backend"]) == Some("ray")) { return false; }
            let head_port = if engine == Engine::Sglang {
                flag(&w, &["--dist-init-addr"]).and_then(|x| x.rsplit_once(':')).and_then(|(_, p)| p.parse::<u16>().ok())
            } else if !ray { flag(&w, &["--master-port"]).and_then(|p| p.parse().ok()) } else {
                cmd.split_once("ray start").and_then(|(_, rest)| flag(&words(rest.split(';').next().unwrap_or(rest)), &["--port"]).and_then(|p| p.parse().ok()))
            };
            // Known different ports are different groups. Missing ports are usable only if this
            // worker has exactly one possible head below.
            head_port.zip(worker_port).is_none_or(|(a, b)| a == b)
        };
        for (p, proc_ports, c, cmd) in &found {
            let c = *c;
            let port = p
                .port
                .or_else(|| c.and_then(|c| published(&c.ports, p.engine.default_port())))
                .or_else(|| proc_ports.first().copied())
                .unwrap_or(p.engine.default_port());
            let id = format!("{}-{}-{port}", n.id, p.engine.slug());
            let head = c.map(|c| Member::of(&n.id, true, c));
            if let Some(i) = out.iter().position(|e: &Endpoint| e.id == id) {
                // Two containers for one endpoint (an old exited one and its replacement): keep the live one.
                let better = match (&head, out[i].members.first()) {
                    (Some(h), Some(old)) => (h.rank(), u64::MAX - h.started_at) < (old.rank(), u64::MAX - old.started_at),
                    _ => false,
                };
                if !better {
                    continue;
                }
                out.remove(i);
            }
            let listener = n.listeners.iter().find(|l| l.port == port);
            let bind = listener.map(|l| l.addr.clone()).or(p.host.clone()).unwrap_or_else(|| "0.0.0.0".into());
            let listen = listen_kind(&bind, n).to_string();
            // A loopback server's URL is the one that works: on the node itself.
            let reachable_at = match listen.as_str() {
                "fabric" | "lan" | "unknown" => bind.clone(),
                "local" => "127.0.0.1".to_string(),
                _ => lan_ip(n, inventory),
            };
            let deployment = c.and_then(|c| c.labels.get("sparkbox.deployment").cloned());
            // Workers: SparkBox's carry the load's label; others point --address at the head.
            let want = Parallel { tp: p.tp, pp: p.pp }.ranks().unwrap_or(0);
            let mut nodes = vec![n.id.clone()];
            let mut members: Vec<Member> = head.into_iter().collect();
            let mine: Vec<_> = workers.iter().filter(|(wn, wc)| {
                if wn.id == n.id || !matches(p, c, cmd, wc) { return false; }
                let candidates: std::collections::BTreeSet<_> = found.iter()
                    .filter(|(p, _, c, cmd)| matches(p, *c, cmd, wc))
                    .map(|(p, ports, c, _)| (p.engine, p.port.or_else(|| c.and_then(|c| published(&c.ports, p.engine.default_port()))).or_else(|| ports.first().copied()).unwrap_or(p.engine.default_port())))
                    .collect();
                candidates.len() == 1
            }).collect();
            // One worker per node (the most alive one), by rank where they have one, else in the
            // cluster's node order: a pipeline's stages.
            let alive = |c: &Container| (Member::of("", false, c).rank(), u64::MAX - c.started_at);
            let mut best: Vec<(&NodeSnapshot, &Container)> = Vec::new();
            for &&(wn, wc) in &mine {
                match best.iter_mut().find(|(n, _)| n.id == wn.id) {
                    Some(b) if alive(wc) < alive(b.1) => b.1 = wc,
                    Some(_) => {}
                    None => best.push((wn, wc)),
                }
            }
            best.sort_by_key(|(_, c)| Member::of("", false, c).stage.unwrap_or(u32::MAX));
            for (wn, wc) in best {
                if nodes.len() >= want {
                    break;
                }
                nodes.push(wn.id.clone());
                members.push(Member::of(&wn.id, false, wc));
            }
            let model_path = p.model.clone();
            let served = p.served.clone().or_else(|| model_path.clone());
            out.push(Endpoint {
                id,
                engine: p.engine,
                models: served.into_iter().collect(),
                nodes,
                head_node: n.id.clone(),
                port,
                url: format!("http://{reachable_at}:{port}/v1"),
                managed_by: if deployment.is_some() { ManagedBy::Sparkbox } else { ManagedBy::External },
                parallel: (p.tp != 1 || p.pp != 1).then_some(Parallel { tp: p.tp, pp: p.pp }),
                container_ids: c.map(|c| vec![c.id.clone()]).unwrap_or_default(),
                container_names: c.map(|c| vec![c.name.clone()]).unwrap_or_default(),
                // A SparkBox load runs its image by id: the name it was picked by is on its label.
                image: c.map(|c| c.labels.get("sparkbox.image").cloned().unwrap_or_else(|| c.image.clone())).filter(|i| !i.is_empty()),
                deployment_id: deployment,
                max_context: p.max_len,
                listen,
                source: Source::Discovered,
                model_path,
                // It goes into a command on the node: only a plain IPv4 address, else loopback.
                probe_host: match bind.as_str() {
                    b if !b.is_empty() && b != "0.0.0.0" && b.split('.').count() == 4 && b.chars().all(|c| c.is_ascii_digit() || c == '.') => b.to_string(),
                    _ => "127.0.0.1".into(),
                },
                members,
                // Listening means its API is up; a node that reports no listeners at all can't tell.
                listening: n.status == LinkStatus::Online && (listener.is_some() || n.listeners.is_empty()),
                ..Default::default()
            });
        }
    }
    out
}

/// Adds SparkBox's loads (managed, with their real node list and stage) and hand-added
/// providers to what was discovered.
pub fn merge(mut found: Vec<Endpoint>, deployments: &[Deployment], manual: &[Provider], inventory: &Inventory) -> Vec<Endpoint> {
    for d in deployments.iter().filter(|d| d.state != DeploymentState::Stopped) {
        let s = &d.spec;
        let Some(head) = s.nodes.first() else { continue };
        let engine = if s.engine == "sglang" { Engine::Sglang } else { Engine::Vllm };
        let id = format!("{head}-{}-{}", engine.slug(), s.port);
        let parallel = (s.nodes.len() > 1).then_some(Parallel { tp: s.tensor_parallel, pp: s.pipeline_parallel });
        let i = match found.iter().position(|e| e.id == id || e.deployment_id.as_deref() == Some(s.id.as_str())) {
            Some(i) => i,
            None => {
                found.push(Endpoint {
                    id,
                    engine,
                    head_node: head.clone(),
                    port: s.port,
                    url: d.endpoint.clone(),
                    listen: s.listen.clone(),
                    probe_host: super::plan::probe_host(s).into(),
                    // Not in the snapshot yet: its own watcher's /health says whether it's up; probes confirm.
                    listening: d.state == DeploymentState::Ready,
                    ..Default::default()
                });
                found.len() - 1
            }
        };
        let e = &mut found[i];
        e.managed_by = ManagedBy::Sparkbox;
        e.source = Source::Deployment;
        e.engine = engine;
        e.deployment_id = Some(s.id.clone());
        e.models = vec![s.served_name.clone()];
        e.nodes = s.nodes.clone();
        e.parallel = parallel;
        e.url = d.endpoint.clone();
        e.max_context = s.max_model_len.or(e.max_context);
        e.model_path = Some(s.model_path.clone());
        e.provider_id = (!d.provider_id.is_empty()).then(|| d.provider_id.clone());
        e.listen = s.listen.clone();
        e.probe_host = super::plan::probe_host(s).into();
        e.deployment = Some((d.state, d.stage.clone(), d.message.clone(), d.started_at));
        e.recoverable = d.recoverable && d.recovery.is_none();
        e.recovery_pending = matches!(d.recovery, Some(super::recovery::Recovery::Checking | super::recovery::Recovery::Waiting));
        e.maintenance_hold = d.recovery == Some(super::recovery::Recovery::Maintenance);
        // A passed check speaks for the run it was made on: one starting again hasn't passed yet.
        e.readiness = d.readiness.clone().filter(|r| !r.ok || d.state == DeploymentState::Ready);
        e.protocols = d.protocols.clone();
        // Demo loads have no readiness check: the simulator answers for them. A load an older
        // SparkBox made Ready has none saved, and nothing would check it now: it answers as
        // any server does (its model list and test tokens), with a note.
        let older = d.state == DeploymentState::Ready && d.readiness.is_none();
        e.needs_readiness = !d.demo && !older;
        e.ready_before_readiness = !d.demo && older;
        if e.image.is_none() && !s.image_ref.is_empty() {
            e.image = Some(s.image_ref.clone());
        }
        if e.container_names.is_empty() {
            // Not in the snapshot yet: what it will be called. SGLang and Demo keep their old names.
            let role = if s.nodes.len() == 1 { "single" } else { "head" };
            let name = if s.engine == "sglang" || d.demo { super::plan::container_name(&s.id, head, role) } else { super::plan::rank_container(&s.id, 0, s.nodes.len()) };
            e.container_names = vec![name];
        }
    }
    for p in manual.iter().filter(|p| p.kind == ProviderKind::OpenaiCompatible && !p.id.starts_with("vllm-")) {
        let Ok(u) = url::Url::parse(&p.base_url) else { continue };
        let host = u.host_str().unwrap_or_default().to_string();
        let port = u.port_or_known_default().unwrap_or(0);
        if found.iter().any(|e| e.port == port && e.url.contains(&format!("//{host}:"))) || found.iter().any(|e| e.id == p.id) {
            continue;
        }
        let node = inventory.nodes.iter().find(|n| n.ip.as_deref() == Some(host.as_str()) || n.host == host).map(|n| n.id.clone());
        found.push(Endpoint {
            id: p.id.clone(),
            engine: Engine::from_port(port),
            models: crate::llm::catalog::visible_models(p),
            nodes: node.clone().into_iter().collect(),
            head_node: node.unwrap_or_default(),
            port,
            url: p.base_url.clone(),
            managed_by: ManagedBy::External,
            provider_id: Some(p.id.clone()),
            listen: "unknown".into(),
            source: Source::Manual,
            listening: true,
            ..Default::default()
        });
    }
    found.sort_by(|a, b| (a.managed_by, &a.head_node, a.port).cmp(&(b.managed_by, &b.head_node, b.port)));
    found
}

// ---- Health from evidence

/// A crash loop: CRASH_RESTARTS restarts within CRASH_WINDOW_S, the last within CRASH_RECENT_S.
/// The health engine's one definition, so the endpoint and its crash-loop alert agree.
pub use crate::health::rules::{CRASH_RECENT_S, CRASH_RESTARTS, CRASH_WINDOW_S};
/// Ready needs an answer this recent (its model list, or /health)…
pub const READY_WITHIN_MS: u64 = 60_000;
/// …and Idle (an Ollama with nothing in memory, asked once a minute: models/watch.rs) this
/// recent: room for the minute's ask and a retry 15 s later when that one finds the channels busy
/// or goes unanswered. Two unanswered asks in a row are Down before then (PROBE_FAILS_DOWN).
pub const IDLE_READY_WITHIN_MS: u64 = 105_000;
/// …and proof that it generates this recent: a test token, or live traffic.
pub const PROOF_WITHIN_MS: u64 = 120_000;
/// A test token every minute (none while live traffic proves it), and a second try sooner.
pub const CANARY_EVERY_MS: u64 = 60_000;
pub const CANARY_RETRY_MS: u64 = 15_000;
/// How long one test token may take before it counts as failed.
pub const CANARY_TIMEOUT_S: u32 = 30;
/// One token taking longer than this is Slow (still Ready, with a note).
pub const SLOW_CANARY_MS: u64 = 10_000;
/// Tokens counted as live traffic within this long, with /health fine, are proof enough.
pub const PASSIVE_WITHIN_MS: u64 = 60_000;
/// Requests running or waiting with no tokens out for this long is a stall (the endpoint-stalled
/// rule's for_s). /health can stay 200 on a deadlocked engine.
pub const STALL_AFTER_S: u64 = 60;
/// An Ollama model this close to unloading isn't tested (the test would have to keep it).
pub const OLLAMA_MIN_LEFT_S: u64 = 30;
/// Failed probes in a row before it's Down.
pub const PROBE_FAILS_DOWN: u32 = 2;
/// Loading with no progress for this long is stuck: Down for a server SparkBox didn't start, and
/// for a load of its own a line on the card, since a first start can be quiet for a long time.
/// Anything new in the log moves the clock for a load of SparkBox's own, and a new compile line
/// for any server (watch.rs `note_progress`): the words a compile logs aren't known for certain.
pub const STUCK_MS: u64 = 20 * 60_000;
/// How long a load of SparkBox's own has to become ready; past this `Models::follow` fails it.
pub const START_LIMIT_MS: u64 = 90 * 60_000;
/// The note on a load an older SparkBox made Ready, before readiness checks.
pub const BEFORE_READINESS: &str = "Loaded by an older SparkBox: 17+25 wasn't asked";
/// A node whose numbers are older than this can't vouch for its part.
const STALE_MS: u64 = 10_000;

/// A cached frame is evidence about the past. A stale frame without an age cannot prove freshness.
pub(super) fn reporting(n: &NodeSnapshot) -> bool {
    n.status == LinkStatus::Online && (!n.stale || n.frame_age_ms.is_some_and(|age| age <= STALE_MS))
}

/// What SparkBox remembers about an endpoint between rounds (watch.rs keeps one per id).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    /// The nodes it last ran on complete, head first.
    pub members: Vec<String>,
    pub last_ok_ms: Option<u64>,
    /// Failed probes in a row, and the last error ("connection refused").
    pub fails: u32,
    pub error: Option<String>,
    /// Restarts per node in the last 10 minutes, and when the last one was seen (unix s).
    pub restarts: BTreeMap<String, (u32, u64)>,
    /// Loading: the weights' percentage, the stage, and when either last moved or (see
    /// `note_progress` in watch.rs) a new line reached its log.
    pub pct: Option<u64>,
    pub stage: Option<Stage>,
    pub progress_ms: Option<u64>,
    pub progress_key: String,
    pub progress_line: String,
    /// The engine's last log line worth quoting (an exited head's error), and the exit it's from.
    pub last_log: Option<String>,
    pub last_log_at: u64,
    /// When it was last found on the cluster (unix ms).
    pub seen_ms: u64,
    /// The engine's own /health (vLLM, SGLang, TensorRT-LLM, NIM): when it last said 200, and
    /// non-200 answers in a row with the last status ("/health 503").
    pub health_ok_ms: Option<u64>,
    pub health_fails: u32,
    pub health_error: Option<String>,
    /// The last test token, failed ones in a row, and when one last came back.
    pub canary: Option<Canary>,
    pub canary_fails: u32,
    pub canary_ok_ms: Option<u64>,
    /// /v1/completions answered "not here": the next tests go to /v1/chat/completions.
    pub chat_only: bool,
    /// The engine's generated-token counter as last read, and when it last rose.
    pub gen_total: Option<f64>,
    pub gen_rose_ms: Option<u64>,
    /// Requests running and waiting at the last read.
    pub running: f64,
    pub waiting: f64,
    /// Since when requests have been running or waiting with the token counter standing still.
    pub flat_since_ms: Option<u64>,
    /// Live traffic proved it generates: tokens rose while /health was fine.
    pub passive_ok_ms: Option<u64>,
    /// Ollama has nothing in memory (or only models about to unload): idle, nothing to test.
    pub idle: bool,
    /// The engine wants an API key SparkBox doesn't have, so it can't send a test token.
    pub untestable: bool,
    /// When /metrics was last read.
    pub scrape_ms: Option<u64>,
    /// The endpoint-stalled rule's wait (0: STALL_AFTER_S).
    pub stall_after_s: u64,
    /// A test token is on its way (Live runs it beside the rounds: it can take 30 s).
    pub canary_running: bool,
    /// When a test was last asked for, whatever came of it (one that wants a key isn't retried each round).
    pub canary_tried_ms: Option<u64>,
    // ── r4-serving ──
    /// Rounds in a row that no source found it (a server process, a listener on its port, its
    /// systemd unit or an answering probe), and since when.
    pub missed: u32,
    pub missed_since_ms: Option<u64>,
    // ── p1r1 look-only ──
    /// A test token was due and look-only held it: it answers but couldn't be tested. Not a
    /// failure; the next real test token clears it.
    pub canary_held: bool,
    // ── p1r2 busy spells ──
    /// Since when every probe has found the head node's SSH channels busy (cleared by the next
    /// probe that gets through). Busy is no evidence: its answer is aged only up to then.
    pub busy_since_ms: Option<u64>,
    /// The same for proof that it generates: since when a probe or a test token found the
    /// channels busy, with no token back since (cleared by the next one that comes back).
    pub untested_since_ms: Option<u64>,
    // ── p1r4 readiness ──
    /// An operation's check (verify.rs) waits for a test token sent after this many rounds had
    /// run: one is due until it comes back, live traffic or not.
    pub verify_after: Option<u64>,
}

/// An endpoint no source finds is Gone only after this many rounds in a row…
pub const GONE_AFTER_ROUNDS: u32 = 3;
/// …and this long: one sample that leaves out an idle process is no evidence.
pub const GONE_AFTER_MS: u64 = 30_000;
/// A planned restart stays planned this long past its expected time (the node-reboot rule's wait).
const PLANNED_GRACE_MS: u64 = 120_000;

/// What the UI calls an endpoint: the name clients use ("glm-5.3-flash"), or for Ollama, whose
/// models come and go, "Ollama on cell3". A model path reads as its last part.
pub fn label_of(e: &Endpoint) -> String {
    let at = |what: &str| if e.head_node.is_empty() { what.to_string() } else { format!("{what} on {}", e.head_node) };
    if e.engine == Engine::Ollama {
        return at("Ollama");
    }
    let served = e.models.first().map(|m| if m.starts_with('/') { m.trim_end_matches('/').rsplit('/').next().unwrap_or(m).to_string() } else { m.clone() });
    served.filter(|s| !s.is_empty()).or_else(|| e.container_names.first().cloned()).unwrap_or_else(|| at(e.engine.label()))
}

/// Who can reach a server, from its bind: loopback only its own node; 0.0.0.0 and the LAN
/// address this Mac too; the fabric only the other nodes.
pub fn reach_of(listen: &str, source: Source) -> &'static str {
    match listen {
        _ if source == Source::Manual => "lan",
        "local" => "node-only",
        "all" | "lan" => "lan",
        "fabric" => "fabric",
        _ => "unknown",
    }
}

/// While SparkBox restarts it in order (from before the first stop until its containers are up
/// again): what would read Down, Gone or Stopped reads "Paused · llama-3.3-70b restarting in
/// order (planned by SparkBox, 06:06)". Loading keeps its progress.
pub fn pause_for_restart(e: &mut Endpoint, started_ms: u64) {
    if !matches!(e.health, Health::Down | Health::Gone | Health::Stopped) {
        return;
    }
    let why = format!("{} restarting in order (planned by SparkBox, {})", e.label, crate::health::maintenance::local_hhmm(started_ms));
    e.health = Health::Paused;
    e.loading_pct = None;
    e.health_reason = Some(why.clone());
    e.message = Some(why);
}

/// "cell3 restarting (planned by SparkBox, 06:06)" while a node the endpoint needs is in a
/// maintenance window that hasn't run past its expected time.
fn planned_on(expected: &[String], snap: &ClusterSnapshot, now_ms: u64) -> Option<String> {
    expected.iter().find_map(|id| {
        let n = snap.nodes.iter().find(|n| &n.id == id)?;
        // Back and reporting: from then on the model shows what it really does (Loading, Ready or Down).
        let back = reporting(n) && n.power_state.is_none();
        if back {
            return None;
        }
        let m = n.maintenance.as_ref().filter(|m| crate::health::maintenance::planned_now(m, now_ms, PLANNED_GRACE_MS))?;
        Some(format!("{} {} (planned by SparkBox, {})", n.name, m.reason, crate::health::maintenance::local_hhmm(m.started_ms)))
    })
}

/// "3 min ago", for reasons that are rebuilt every round.
pub fn ago(secs: u64) -> String {
    match secs {
        0..=9 => "just now".into(),
        10..=59 => format!("{secs} s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// Records what /metrics said: the token counter (for live-traffic proof and stalls) and
/// how many requests are running and waiting.
pub fn record_scrape(t: &mut Track, gen_total: Option<f64>, running: f64, waiting: f64, has_health: bool, now_ms: u64) {
    // A counter that went back means the server restarted: anything above zero is new tokens.
    let rose = match (gen_total, t.gen_total) {
        (Some(n), Some(o)) => n > o || (n < o && n > 0.0),
        _ => false,
    };
    if rose {
        t.gen_rose_ms = Some(now_ms);
    }
    let known = gen_total.is_some() && t.gen_total.is_some();
    if (running > 0.0 || waiting > 0.0) && known && !rose {
        // Flat since the reading before this one, at least.
        if t.flat_since_ms.is_none() {
            t.flat_since_ms = Some(t.scrape_ms.unwrap_or(now_ms));
        }
    } else {
        t.flat_since_ms = None;
    }
    // Tokens rising while the engine says it's healthy prove it generates; no test needed.
    let healthy = if has_health { t.health_ok_ms } else { t.last_ok_ms };
    if rose && healthy.is_some_and(|h| now_ms.saturating_sub(h) <= READY_WITHIN_MS) {
        t.passive_ok_ms = Some(now_ms);
        t.untested_since_ms = None;
    }
    if gen_total.is_some() {
        t.gen_total = gen_total;
    }
    t.running = running;
    t.waiting = waiting;
    t.scrape_ms = Some(now_ms);
}

/// Records the engine's /health answer. `status` 0 is no answer at all, which the model-list
/// probe already counts; any other non-200 means the server is up but its engine isn't.
pub fn record_health(t: &mut Track, status: u16, path: &str, now_ms: u64) {
    match status {
        200 => {
            t.health_ok_ms = Some(now_ms);
            t.health_fails = 0;
            t.health_error = None;
        }
        0 => {}
        c => {
            t.health_fails += 1;
            t.health_error = Some(format!("{path} {c}"));
        }
    }
}

/// Records a test token's result.
pub fn record_canary(t: &mut Track, c: Canary) {
    t.canary_tried_ms = Some(c.at_ms);
    if c.ok {
        t.canary_fails = 0;
        t.canary_ok_ms = Some(c.at_ms);
        t.untested_since_ms = None;
    } else {
        // A busy spell's proof stays as it was: two failed tokens in a row are Down, as ever.
        t.canary_fails += 1;
    }
    t.untestable = false;
    t.canary_held = false;
    t.canary = Some(c);
}

/// Whether a test token is due: every minute, sooner after a failure, never while live
/// traffic proves it generates or while nothing is loaded. While an operation's check waits for
/// a token sent after it (`verify_after`), one is due until it comes back, traffic or not.
pub fn canary_due(t: &Track, now_ms: u64) -> bool {
    let verifying = t.verify_after.is_some_and(|r| !t.canary.as_ref().is_some_and(|c| c.ok && c.after_round.is_some_and(|a| a > r)));
    if t.canary_running || (!verifying && t.passive_ok_ms.is_some_and(|p| now_ms.saturating_sub(p) <= PASSIVE_WITHIN_MS)) {
        return false;
    }
    if t.untestable && t.canary_tried_ms.is_some_and(|at| now_ms.saturating_sub(at) < CANARY_EVERY_MS) {
        return false;
    }
    match &t.canary {
        None => true,
        Some(c) if !c.ok => now_ms.saturating_sub(c.at_ms) >= CANARY_RETRY_MS,
        _ if verifying => true,
        // A token from before it last went down proves nothing now: test as soon as it answers.
        Some(_) if t.canary_ok_ms.is_none() => true,
        Some(c) => now_ms.saturating_sub(c.at_ms) >= CANARY_EVERY_MS,
    }
}

/// What a test token goes to: the model, the chat route for chat-only servers, and for Ollama
/// how long the model may stay loaded (what it has left, so the test doesn't keep it longer).
#[derive(Clone, Debug, PartialEq)]
pub struct CanaryRequest {
    pub model: String,
    pub chat: bool,
    pub keep_alive_s: Option<u64>,
}

/// The test request for one token, run with curl on the head node. The API key, when there is
/// one, arrives on stdin as a header line (`-H @-`), never in the command. curl prints the
/// reply, then the status and the seconds it took; the shell adds curl's exit code.
pub fn canary_command(engine: Engine, host: &str, port: u16, req: &CanaryRequest, with_key: bool) -> String {
    let (path, body) = match engine {
        Engine::Ollama => {
            let mut body = serde_json::json!({ "model": req.model, "prompt": "Hi", "stream": false, "options": { "num_predict": 1 } });
            // What it has left, so the test neither keeps it longer nor unloads it sooner.
            if let Some(left) = req.keep_alive_s {
                body["keep_alive"] = format!("{left}s").into();
            }
            ("/api/generate", body)
        }
        _ if req.chat => (
            "/v1/chat/completions",
            serde_json::json!({ "model": req.model, "messages": [{ "role": "user", "content": "Hi" }], "max_tokens": 1, "temperature": 0 }),
        ),
        _ => ("/v1/completions", serde_json::json!({ "model": req.model, "prompt": "Hi", "max_tokens": 1, "temperature": 0 })),
    };
    let key = if with_key { " -H @-" } else { "" };
    format!(
        "curl -s -m {CANARY_TIMEOUT_S} -w '\\n%{{http_code}} %{{time_total}}'{key} -H 'Content-Type: application/json' -d {} http://{host}:{port}{path} 2>/dev/null; printf ' %s\\n' \"$?\"",
        crate::live::sh_quote(&body.to_string())
    )
}

/// What a test request came back with.
#[derive(Clone, Debug, PartialEq)]
pub enum CanaryReply {
    Done(Canary),
    /// /v1/completions isn't served here (a chat-only server): ask /v1/chat/completions.
    ChatOnly,
    /// It wants an API key SparkBox doesn't have (or refused the one it has).
    NeedsKey,
}

/// The engine's own words for an error reply ({"error": {"message"}}, {"error": "…"}, {"detail"}).
fn error_words(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let e = &v["error"];
    let text = e["message"].as_str().or(e.as_str()).or(v["detail"].as_str()).or(v["message"].as_str())?;
    Some(first_line(text).chars().take(120).collect())
}

/// Reads a test request's output (canary_command): reply, "<status> <seconds>", curl's exit.
pub fn parse_canary(engine: Engine, out: &str, at_ms: u64) -> CanaryReply {
    let text = out.trim_end();
    let (body, tail) = text.rsplit_once('\n').unwrap_or(("", text));
    // "200 0.084 0", or only the exit code when curl never ran ("127").
    let f: Vec<&str> = tail.split_whitespace().collect();
    let (status, secs, exit): (u16, Option<f64>, Option<u32>) = match f.as_slice() {
        [code, secs, exit, ..] => (code.parse().unwrap_or(0), secs.parse().ok(), exit.parse().ok()),
        [exit] => (0, None, exit.parse().ok()),
        _ => (0, None, None),
    };
    let latency_ms = secs.filter(|s| *s > 0.0).map(|s| (s * 1000.0).round() as u64);
    let fail = |error: String| CanaryReply::Done(Canary { at_ms, ok: false, latency_ms: None, error: Some(error), after_round: None });
    match status {
        200 => {
            let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let token = match engine {
                Engine::Ollama => v["done"].as_bool() == Some(true) && v["response"].is_string(),
                _ => v["choices"].as_array().is_some_and(|c| !c.is_empty()),
            };
            if token {
                CanaryReply::Done(Canary { at_ms, ok: true, latency_ms, error: None, after_round: None })
            } else {
                fail(error_words(body).unwrap_or_else(|| "the reply had no token".into()))
            }
        }
        401 | 403 => CanaryReply::NeedsKey,
        404 | 405 if engine != Engine::Ollama => CanaryReply::ChatOnly,
        400 if engine != Engine::Ollama && error_words(body).is_some_and(|w| w.to_ascii_lowercase().contains("chat")) => CanaryReply::ChatOnly,
        0 => fail(match exit {
            Some(28) => format!("no reply in {CANARY_TIMEOUT_S} s"),
            Some(7) => "connection refused".into(),
            Some(52) => "empty reply".into(),
            Some(56) => "connection reset".into(),
            Some(127) => "curl isn't installed on the node".into(),
            Some(c) => format!("curl failed (exit {c})"),
            None => "no reply".into(),
        }),
        c => fail(match error_words(body) {
            Some(w) => format!("HTTP {c}: {w}"),
            None => format!("HTTP {c}"),
        }),
    }
}

/// How long requests have been running or waiting with no token out, in whole seconds.
pub fn flat_for_s(t: &Track, now_ms: u64) -> Option<u64> {
    t.flat_since_ms.map(|since| until(since, t.busy_since_ms, now_ms).saturating_sub(since) / 1000)
}

/// How far evidence from `at` is aged: up to now, or up to the start of a busy spell that began
/// after it (no SSH channel came free to look again, which says nothing either way).
fn until(at: u64, busy_since: Option<u64>, now_ms: u64) -> u64 {
    busy_since.filter(|b| *b >= at).map_or(now_ms, |b| b.min(now_ms))
}

/// "fell off the bus": what a fatal Xid means, in a few words (NVIDIA's Xid catalog).
pub fn xid_words(code: u32) -> Option<&'static str> {
    Some(match code {
        13 => "graphics engine exception",
        31 => "GPU memory page fault",
        43 => "GPU stopped processing",
        45 => "preemptive cleanup",
        48 => "double-bit ECC error",
        61 | 62 => "internal microcontroller error",
        63 | 64 => "ECC page retirement",
        74 => "NVLink error",
        79 => "fell off the bus",
        92 => "high single-bit ECC error rate",
        94 => "contained ECC error",
        95 => "uncontained ECC error",
        119 | 120 => "GSP error",
        154 => "GPU needs a reset",
        _ => return None,
    })
}

/// "GPU fault on gx10-0000: Xid 79 (fell off the bus)".
pub fn gpu_fault_reason(node: &str, f: &crate::model::GpuFault) -> String {
    match f.code {
        Some(code) => match xid_words(code) {
            Some(w) => format!("GPU fault on {node}: Xid {code} ({w})"),
            None => format!("GPU fault on {node}: Xid {code}"),
        },
        None => format!("GPU fault on {node}: {}", first_line(&f.text)),
    }
}

/// "Pipeline stage 3 of 3", "Tensor-parallel rank 2 of 4" or "Rank 3 of 4" for node `i`.
fn part(i: usize, parallel: Option<Parallel>) -> String {
    match parallel {
        Some(p) if p.pp > 1 && p.tp == 1 => format!("Pipeline stage {} of {}", i + 1, p.pp),
        Some(p) if p.tp > 1 && p.pp == 1 => format!("Tensor-parallel rank {} of {}", i + 1, p.tp),
        Some(p) if p.ranks().is_some_and(|n| n > 1) => format!("Rank {} of {}", i + 1, p.ranks().unwrap()),
        _ => "The server".into(),
    }
}

fn exit_words(m: &Member) -> String {
    match (m.oom, m.exit_code) {
        (true, c) => format!("out of memory, exit {c}"),
        (false, 137) => "killed, exit 137".into(),
        (false, c) => format!("exit {c}"),
    }
}

/// Docker stopped it, or it finished by itself: an exit nobody needs to look into.
fn clean_exit(m: &Member) -> bool {
    m.exit_code == 0 || (m.exit_code == 143 && !m.oom)
}

/// The first worker to stop during this run of the head, no later than the head did. In native
/// mode a head exits when it loses a rank, so that's where to look.
fn stopped_before<'a>(e: &'a Endpoint, h: &Member) -> Option<&'a Member> {
    let run = h.started_at..=h.finished_at;
    e.members.iter().filter(|m| !m.head && !m.running() && run.contains(&m.finished_at)).min_by_key(|m| m.finished_at)
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// Why its probes fail, naming the path they take: "No answer from 127.0.0.1:11434 on cell3
/// (over SSH)", or "HTTP 502 from …" when something answered with an error. Hand-added endpoints
/// are asked from this Mac.
fn no_answer(e: &Endpoint, err: &str) -> String {
    let path = if e.source == Source::Manual || e.head_node.is_empty() {
        e.url.clone()
    } else {
        let host = if e.probe_host.is_empty() { "127.0.0.1" } else { e.probe_host.as_str() };
        format!("{host}:{} on {} (over SSH)", e.port, e.head_node)
    };
    match err.strip_prefix("HTTP ") {
        Some(code) => format!("HTTP {code} from {path}"),
        None if e.source == Source::Manual => format!("No answer from {path} ({err})"),
        None => format!("No answer from {path}"),
    }
}

/// Decides health from the evidence: the containers behind it, its nodes in the snapshot,
/// its probes and its loading progress. Sets health, the reason, the expected and missing
/// nodes, and Loading's percentage. Planned work comes last: while a node it needs is in a
/// maintenance window SparkBox opened, what would read Down, Loading or Gone reads Paused.
pub fn assess(e: &mut Endpoint, t: &Track, snap: &ClusterSnapshot, now_ms: u64) {
    e.reach = reach_of(&e.listen, e.source).to_string();
    assess_evidence(e, t, snap, now_ms);
    if e.health == Health::Unknown {
        e.forgettable = false;
        e.compiling = false;
        e.metrics = None;
        e.metrics_source = None;
    }
    e.label = label_of(e);
    e.gone_since_ms = (e.health == Health::Gone).then_some(t.seen_ms);
    if matches!(e.health, Health::Down | Health::Loading | Health::Gone | Health::Unknown) && e.source != Source::Manual {
        if let Some(why) = planned_on(&e.expected_nodes, snap, now_ms) {
            e.health = Health::Paused;
            e.loading_pct = None;
            e.health_reason = Some(why.clone());
            e.message = Some(why);
        }
    }
}

fn assess_evidence(e: &mut Endpoint, t: &Track, snap: &ClusterSnapshot, now_ms: u64) {
    let now = now_ms / 1000;
    let want = e.parallel.and_then(Parallel::ranks).unwrap_or(1);
    let mut expected = e.nodes.clone();
    for n in &t.members {
        if expected.len() < want && !expected.contains(n) {
            expected.push(n.clone());
        }
    }
    // Stages in pipeline order (a SparkBox load's own node list already is): the head first,
    // then each member's launch rank or label, else the place it last ran in complete, then
    // the cluster's. A missing middle stage keeps its number.
    if e.deployment.is_none() && expected.len() > 1 {
        let cluster_at = |n: &String| snap.nodes.iter().position(|x| &x.id == n).unwrap_or(usize::MAX);
        let stage = |n: &String| e.members.iter().find(|m| &m.node == n).and_then(|m| m.stage).map(|s| s as usize);
        let remembered = |n: &String| t.members.iter().position(|x| x == n);
        expected.sort_by_key(|n| (n != &e.head_node, stage(n).or(remembered(n)).unwrap_or(usize::MAX), cluster_at(n)));
    }
    e.nodes = expected.clone();
    e.expected_nodes = if e.source == Source::Manual { Vec::new() } else { expected.clone() };
    e.missing_nodes.clear();
    e.last_ok_ms = t.last_ok_ms;
    e.loading_pct = None;
    e.canary = t.canary.clone();
    e.passive_ok_ms = t.passive_ok_ms;
    e.stalled_s = flat_for_s(t, now_ms).filter(|s| *s >= 15);
    e.health_note = None;
    e.forgettable = e.managed_by == ManagedBy::External
        && e.source != Source::Manual
        && (e.gone || e.members.iter().find(|m| m.head).is_some_and(|h| !h.running() && h.state != "restarting"));
    let total = e.members.iter().fold(0u32, |n, m| n.saturating_add(m.restart_count));
    e.restarts = (total > 0).then_some(total);
    let managed = e.deployment.clone();
    let starting = managed.as_ref().is_some_and(|(s, ..)| *s == DeploymentState::Starting);
    // What it's doing: a starting load's own stage, else what its log last said while it wasn't
    // listening. A compile that hasn't moved in a whole start's time has hung.
    let shown = match &managed {
        Some((DeploymentState::Starting, stage, ..)) => stage.as_ref(),
        _ => t.stage.as_ref(),
    };
    // Nothing compiles with a rank gone, or with no head at all, unless it's a start whose
    // containers haven't shown up yet.
    let ranks_up = e.members.iter().all(|m| m.running()) && (starting || e.members.iter().any(|m| m.head));
    let quiet_compile = t.progress_ms.is_some_and(|at| now_ms.saturating_sub(at) >= START_LIMIT_MS);
    e.compiling = matches!(shown, Some(Stage::Compiling { .. })) && ranks_up && !e.listening && !quiet_compile;
    let verdict = |e: &mut Endpoint, h: Health, why: Option<String>| {
        e.health = h;
        e.message = why.clone().filter(|_| h != Health::Ready);
        e.health_reason = e.message.clone();
    };

    if e.maintenance_hold { return verdict(e, Health::Paused, Some("Paused for rolling maintenance. Resume or restore from Software Updates.".into())); }
    if e.recovery_pending {
        return verdict(e, Health::Unknown, e.deployment.as_ref().map(|d| d.2.clone()));
    }

    if e.parallel.is_some_and(|p| p.ranks().is_none()) {
        return verdict(e, Health::Unknown, Some("Invalid parallel configuration: the rank count is out of range.".into()));
    }

    // Every expected node online and reporting.
    let mut why_missing: Vec<String> = Vec::new();
    for (i, node) in expected.iter().enumerate() {
        let who = part(i, e.parallel);
        let why = match snap.nodes.iter().find(|n| &n.id == node) {
            Some(n) if reporting(n) => continue,
            Some(n) if n.status == LinkStatus::Online => {
                let Some(age) = n.frame_age_ms else {
                    e.missing_nodes.push(node.clone());
                    why_missing.push(format!("{node} has no current report"));
                    continue;
                };
                let s = age / 1000;
                if want > 1 { format!("{node} hasn't reported for {s} s ({})", who.to_lowercase()) } else { format!("{node} hasn't reported for {s} s") }
            }
            _ if want > 1 => format!("{who} on {node} is offline"),
            _ => format!("{node} is offline"),
        };
        e.missing_nodes.push(node.clone());
        why_missing.push(why);
    }
    match why_missing.len() {
        0 => {}
        1 => return verdict(e, Health::Unknown, why_missing.pop()),
        n => {
            let why = format!("{n} of {want} nodes are offline: {}", e.missing_nodes.join(", "));
            return verdict(e, Health::Unknown, Some(why));
        }
    }

    if e.supervisor.as_ref().is_some_and(|s| s.state == SupervisorState::GaveUp) {
        let n = e.supervisor.as_ref().map(|s| s.restarts).unwrap_or(0);
        return verdict(e, Health::Down, Some(format!("Gave up after {n} restarts — see logs")));
    }

    // A member whose GPU failed (a latched fatal Xid, or readings it can't take) can't serve its
    // part, whatever its containers say.
    for node in &expected {
        if let Some(f) = snap.nodes.iter().find(|n| &n.id == node).and_then(|n| n.gpu_fault.as_ref()) {
            e.missing_nodes.push(node.clone());
            return verdict(e, Health::Down, Some(gpu_fault_reason(node, f)));
        }
    }

    // The head container.
    if let Some(h) = e.members.iter().find(|m| m.head).cloned() {
        let since = ago(now.saturating_sub(h.finished_at));
        let log = t.last_log.as_deref().map(|l| format!(" · {l}")).unwrap_or_default();
        let worker = stopped_before(e, &h).cloned();
        // A clean exit after a worker crashed isn't someone stopping it.
        let stopped = clean_exit(&h) && worker.as_ref().is_none_or(clean_exit);
        match h.state.as_str() {
            "running" => {}
            "restarting" => {}
            "paused" => return verdict(e, Health::Down, Some(format!("{} is paused", h.name))),
            "created" => return verdict(e, Health::Down, Some(format!("{} was created but never started", h.name))),
            _ if stopped => return verdict(e, Health::Stopped, Some(format!("Stopped {since}"))),
            _ if h.oom && starting => return verdict(e, Health::Down, Some(format!("Out of memory while loading (exit {})", h.exit_code))),
            _ if h.oom => return verdict(e, Health::Down, Some(format!("Head ran out of memory (exit {}) {since}", h.exit_code))),
            _ => {
                let why = worker.map_or(String::new(), |w| {
                    // Docker's clock is in seconds: in the same one, nobody can say which went first.
                    let when = if w.finished_at < h.finished_at { "stopped first" } else { "stopped with it" };
                    let who = expected.iter().position(|n| n == &w.node).map_or(String::new(), |i| format!(" · {}", part(i, e.parallel).to_lowercase()));
                    e.missing_nodes.push(w.node.clone());
                    format!(" · worker on {} {when} ({}){who}", w.node, exit_words(&w))
                });
                return verdict(e, Health::Down, Some(format!("Head exited (code {}) {since}{why}{log}", h.exit_code)));
            }
        }
    }

    // A crash loop is Down, never Loading.
    for m in &e.members {
        let (n, last) = t.restarts.get(&m.node).copied().unwrap_or((0, 0));
        let looping = m.state == "restarting" || (n >= CRASH_RESTARTS && now.saturating_sub(last) <= CRASH_RECENT_S);
        if looping {
            let what = if m.head { String::new() } else { format!(" on {}", m.node) };
            // Below the threshold it's Docker restarting it, not (yet) a loop: say what's known.
            let reason = if n >= CRASH_RESTARTS {
                format!("Crash loop{what}: {n} restarts in 10 min · last exit {}", m.exit_code)
            } else if n >= 1 {
                format!("Restarting{what} after exit {} · {n} restart{} in 10 min", m.exit_code, if n == 1 { "" } else { "s" })
            } else {
                format!("Restarting{what} after exit {}", m.exit_code)
            };
            return verdict(e, Health::Down, Some(reason));
        }
    }

    // Every worker running (a load that is still starting is still starting its workers).
    if !starting {
        for (i, node) in expected.iter().enumerate().skip(1) {
            let who = part(i, e.parallel);
            match e.members.iter().find(|m| !m.head && &m.node == node) {
                Some(m) if m.running() => {}
                Some(m) => {
                    e.missing_nodes.push(node.clone());
                    let since = ago(now.saturating_sub(m.finished_at));
                    let state = if m.state == "exited" { format!("{}, {since}", exit_words(m)) } else { m.state.clone() };
                    return verdict(e, Health::Down, Some(format!("Worker on {node} isn't running ({state}) · {}", who.to_lowercase())));
                }
                // Containers seen for it, but none on this node. (A load whose containers the snapshot
                // doesn't show yet has no evidence either way: its own watcher speaks for it.)
                None if !e.members.is_empty() => {
                    e.missing_nodes.push(node.clone());
                    return verdict(e, Health::Down, Some(format!("{who} on {node} has no worker running")));
                }
                None => {}
            }
        }
        if expected.len() < want && !e.members.is_empty() {
            let short = format!("Needs {want} nodes but only {} {} running", expected.len(), if expected.len() == 1 { "is" } else { "are" });
            // Every rank knows its place: the first one nobody holds is the one that's gone.
            let ranked = e.members.iter().all(|m| m.stage.is_some());
            let gap = (0..want).find(|r| !e.members.iter().any(|m| m.stage == Some(*r as u32))).filter(|_| ranked);
            let why = gap.map_or(short.clone(), |r| format!("{short}: {} is missing", part(r, e.parallel).to_lowercase()));
            return verdict(e, Health::Down, Some(why));
        }
    }

    // Vanished (no process, listener, unit or answer for a while): no evidence it broke.
    if e.gone {
        return verdict(e, Health::Gone, Some(format!("Not found since {}", crate::health::maintenance::local_hhmm(t.seen_ms))));
    }
    if let Some((DeploymentState::Failed, _, msg, _)) = &managed {
        // A load saved by an older SparkBox may hold a refusal's JSON (Models::fail no longer writes one).
        return verdict(e, Health::Down, Some(first_line(&crate::errors::SbError::words(msg))));
    }

    // Probes: two failures in a row on the path that reaches it, and it's Down, whatever the
    // listener says. The reason names that path. One starting or compiling that doesn't listen
    // yet isn't asked, so what's counted is from before: the failures Keep running came for, say.
    if t.fails >= PROBE_FAILS_DOWN && (e.listening || !(starting || e.compiling)) {
        let err = t.error.clone().unwrap_or_else(|| "no answer".into());
        return verdict(e, Health::Down, Some(no_answer(e, &err)));
    }

    // Starting and progressing is Loading. Starting and quiet is still Loading for a load of
    // SparkBox's own, which fails at START_LIMIT_MS; anything else that's quiet is Down.
    let loading = match &managed {
        Some((DeploymentState::Starting, stage, msg, started)) => {
            let pct = match stage {
                Some(Stage::Loading { percent }) => Some(*percent),
                _ => None,
            };
            Some((pct.or(t.pct), stage_text(stage.as_ref(), msg), *started * 1000))
        }
        _ if !e.listening => {
            // It can only start loading once its last container is up.
            let started = e.members.iter().filter(|m| m.running()).map(|m| m.started_at * 1000).max().unwrap_or(now_ms);
            let text = match t.pct {
                Some(p) => format!("Loading weights {p}%"),
                None => stage_text(t.stage.as_ref(), "Loading"),
            };
            Some((t.pct, text, started))
        }
        _ => None,
    };
    if let Some((pct, text, started_ms)) = loading {
        let moved = t.progress_ms.unwrap_or(started_ms).max(started_ms);
        // Look-only holding its readiness check is waiting, not being stuck.
        let held = t.canary_held || e.readiness.as_ref().is_some_and(|r| r.why.as_deref() == Some(super::readiness::HELD));
        let quiet = now_ms.saturating_sub(moved);
        let stuck = started_ms > 0 && !held && quiet > STUCK_MS;
        // A quiet compile is ordinary on a first start: a load of SparkBox's own gets a whole
        // start's time, whatever its state says. A server it didn't start gets the 20 minutes.
        let compiled_out = e.managed_by == ManagedBy::Sparkbox && matches!(shown, Some(Stage::Compiling { .. }));
        let limit = if compiled_out { START_LIMIT_MS } else { STUCK_MS };
        if stuck && !starting && quiet >= limit {
            return verdict(e, Health::Down, Some(format!("Didn't become ready in {} min", limit / 60_000)));
        }
        e.loading_pct = pct;
        let text = if stuck { format!("{text} · no new progress for {} min (limit {} min)", quiet / 60_000, START_LIMIT_MS / 60_000) } else { text };
        return verdict(e, Health::Loading, Some(text));
    }

    // A SparkBox load is Ready only once it answered its readiness check (readiness.rs): its
    // model listed, a test token, the 17+25 prompt the first time. /health alone never makes it so.
    if e.needs_readiness && !e.readiness.as_ref().is_some_and(|r| r.ok) {
        let text = match e.readiness.as_ref().and_then(|r| r.why.as_deref()) {
            Some(why) => format!("Up; checking it answers… ({why})"),
            None => "Up; checking it answers…".to_string(),
        };
        return verdict(e, Health::Loading, Some(text));
    }

    // Listening and answering. The engine itself has to say it's healthy…
    if t.health_fails >= PROBE_FAILS_DOWN {
        let err = t.health_error.clone().unwrap_or_else(|| "/health failed".into());
        return verdict(e, Health::Down, Some(format!("Engine not healthy: {err}")));
    }
    // …tokens have to come out of the requests it's running…
    let stall_after = if t.stall_after_s > 0 { t.stall_after_s } else { STALL_AFTER_S };
    if let Some(flat) = flat_for_s(t, now_ms).filter(|s| *s >= stall_after) {
        let (n, doing) = if t.running > 0.0 { (t.running, "running") } else { (t.waiting, "waiting") };
        let n = n.round() as u64;
        let why = format!("Stalled: {n} request{} {doing}, no tokens for {flat} s", if n == 1 { "" } else { "s" });
        return verdict(e, Health::Down, Some(why));
    }
    // …and a test token has to come back.
    if t.canary_fails >= PROBE_FAILS_DOWN {
        let err = t.canary.as_ref().and_then(|c| c.error.clone()).unwrap_or_else(|| "no token came back".into());
        return verdict(e, Health::Down, Some(format!("Didn't generate: {err} ({} tries)", t.canary_fails)));
    }

    // Ready: it answered within the last minute, and generated within the last two. A busy spell
    // (no SSH channel came free to ask it, or to test it) counts only up to its start.
    let answered = [t.last_ok_ms, t.health_ok_ms].into_iter().flatten().max();
    let proof = [t.canary_ok_ms, t.passive_ok_ms].into_iter().flatten().max();
    let asked_age = |ok: u64| until(ok, t.busy_since_ms, now_ms).saturating_sub(ok);
    let proof_age = |p: u64| until(p, t.untested_since_ms, now_ms).saturating_sub(p);
    let busy_note = match (t.busy_since_ms, t.untested_since_ms) {
        (Some(b), _) => Some(format!("Not checked since {}: SSH channels busy on {}", crate::health::maintenance::local_hhmm(b), e.head_node)),
        (None, Some(b)) if t.canary_fails == 0 => Some(format!("Not tested since {}: SSH channels busy on {}", crate::health::maintenance::local_hhmm(b), e.head_node)),
        _ => None,
    };
    match answered {
        // It answers with nothing in memory: healthy at rest, with nothing to test.
        Some(ok) if t.idle && asked_age(ok) <= IDLE_READY_WITHIN_MS => {
            verdict(e, Health::Idle, Some("Nothing in memory".into()));
            e.health_note = busy_note;
        }
        Some(ok) if asked_age(ok) <= READY_WITHIN_MS => {
            let proven = proof.is_some_and(|p| proof_age(p) <= PROOF_WITHIN_MS);
            // Look-only holds test tokens: answering is all there is to go on, so it's Ready, said so.
            if proven || t.untestable || t.canary_held || e.source == Source::Manual {
                e.health_note = if busy_note.is_some() {
                    busy_note
                } else if t.untestable && !proven {
                    Some("Not tested: it wants an API key SparkBox doesn't have".into())
                } else if t.canary_held && !proven {
                    Some("Not tested: look-only mode holds test tokens".into())
                } else {
                    let slow = t.canary.as_ref().filter(|c| c.ok).and_then(|c| c.latency_ms).filter(|ms| *ms > SLOW_CANARY_MS);
                    slow.map(|ms| format!("Slow: 1 token took {:.1} s", ms as f64 / 1000.0)).or_else(|| e.ready_before_readiness.then(|| BEFORE_READINESS.to_string()))
                };
                verdict(e, Health::Ready, None)
            } else if let Some(p) = proof {
                verdict(e, Health::Down, Some(format!("No token for {} s", proof_age(p) / 1000)))
            } else if t.canary_fails > 0 {
                let err = t.canary.as_ref().and_then(|c| c.error.clone()).unwrap_or_else(|| "no token came back".into());
                verdict(e, Health::Loading, Some(format!("Answers, but no token yet ({err})")))
            } else {
                verdict(e, Health::Loading, Some("Checking that it generates…".into()))
            }
        }
        Some(ok) => verdict(e, Health::Down, Some(format!("No answer for {} s", asked_age(ok) / 1000))),
        None if t.fails > 0 => verdict(e, Health::Loading, Some(format!("Not answering yet ({})", t.error.clone().unwrap_or_else(|| "no answer".into())))),
        None => verdict(e, Health::Loading, Some("Checking that it answers…".into())),
    }
}

/// What a starting load is doing, in a few words.
fn stage_text(stage: Option<&Stage>, msg: &str) -> String {
    match stage {
        Some(Stage::Joining { have, need }) => format!("{have} of {need} nodes up"),
        Some(Stage::Loading { percent }) => format!("Loading weights {percent}%"),
        Some(Stage::Compiling { what }) => format!("Compiling: {what}"),
        Some(Stage::Warming) => "Capturing CUDA graphs".into(),
        _ if !msg.is_empty() => first_line(msg),
        _ => "Starting".into(),
    }
}

/// Model ids and the longest context from an OpenAI-style /v1/models reply.
pub fn parse_models(v: &Value) -> (Vec<String>, Option<u64>) {
    let data = v["data"].as_array().cloned().unwrap_or_default();
    let ids = data.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect();
    let max = data.iter().filter_map(|m| m["max_model_len"].as_u64()).max();
    (ids, max)
}

/// The provider a healthy discovered endpoint gets, so Chat and the Server can use it.
pub fn provider_for(e: &Endpoint, demo: bool) -> Option<Provider> {
    if e.health != Health::Ready || e.models.is_empty() || e.engine == Engine::Other || e.source != Source::Discovered {
        return None;
    }
    if e.listen == "local" || e.listen == "fabric" {
        return None; // not reachable from this Mac
    }
    Some(Provider {
        id: e.id.clone(),
        name: format!("{} on {} :{}", e.engine.label(), e.head_node, e.port),
        kind: if demo { ProviderKind::Demo } else { ProviderKind::OpenaiCompatible },
        base_url: e.url.clone(),
        default_model: e.models.first().cloned(),
        models: e.models.clone(),
        ..Default::default()
    })
}

/// Probe results kept between rounds, by endpoint id.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub ok: bool,
    pub models: Vec<String>,
    pub max_context: Option<u64>,
    pub error: Option<String>,
    pub at_ms: u64,
    pub ollama: Vec<OllamaLoaded>,
    /// The engine's /health status (0: no answer), for engines that have one.
    pub health: Option<u16>,
    // ── r4-serving ──
    /// Ollama's tags with their sizes (/api/tags): what it keeps on disk.
    pub on_disk: Vec<(String, u64)>,
    /// `systemctl show -p ActiveState` of its unit said active (Ollama's ollama.service).
    pub unit_active: Option<bool>,
}

/// Folds the last probe into an endpoint: the models it really serves. Whether it
/// answered goes into its Track, and `assess` decides what that means.
pub fn apply_probe(e: &mut Endpoint, p: &Probe) {
    e.probed_ms = Some(p.at_ms);
    if p.ok {
        // Ollama's model list is what it has on disk; what it serves is what's in memory.
        if e.engine == Engine::Ollama {
            e.on_disk = p.on_disk.iter().map(|(n, _)| n.clone()).collect();
            e.on_disk_bytes = p.on_disk.iter().map(|(_, b)| *b).collect();
        }
        if !p.models.is_empty() && e.source != Source::Deployment {
            e.models = p.models.clone();
        }
        e.max_context = p.max_context.or(e.max_context);
        e.ollama = p.ollama.clone();
    }
}

/// Whether this round's probe found it: it answered, or its unit is active.
pub fn probe_finds(p: &Probe) -> bool {
    p.ok || p.unit_active == Some(true)
}

/// Records a probe that found the head node's SSH channels busy (nothing reached the engine):
/// its answer and its proof stop ageing until a probe, and a test token, get through again.
pub fn record_busy(t: &mut Track, now_ms: u64) {
    t.busy_since_ms.get_or_insert(now_ms);
    t.untested_since_ms.get_or_insert(now_ms);
}

/// Records a test token that found the channels busy: its proof stops ageing until one comes back.
pub fn record_canary_busy(t: &mut Track, now_ms: u64) {
    t.untested_since_ms.get_or_insert(now_ms);
}

/// Records a probe in the endpoint's Track: when it last answered, and failures in a row.
pub fn record_probe(t: &mut Track, p: &Probe) {
    t.busy_since_ms = None;
    if p.ok {
        t.last_ok_ms = Some(p.at_ms);
        t.fails = 0;
        t.error = None;
    } else {
        t.fails += 1;
        t.error = p.error.clone();
    }
}

/// Library folders being served right now, by key or repo, for the "in use" badge.
pub fn in_use(endpoints: &[Endpoint]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for e in endpoints.iter().filter(|e| matches!(e.health, Health::Ready | Health::Loading)) {
        if let Some(p) = &e.model_path {
            m.insert(p.clone(), e.id.clone());
            if let Some(r) = repo_of(p) {
                m.insert(r, e.id.clone());
            }
        }
    }
    m
}

/// An endpoint a node agent probes on its own head node while SparkBox is closed
/// (agent.json `endpoints`, read by serving_probe.py). Never the key itself: `key_ref` names
/// the Keychain item whose value SparkBox puts in the agent's credential.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEndpoint {
    pub id: String,
    /// "llama-3.3-70b" (for alerts).
    pub name: String,
    /// vllm | sglang | trtllm | nim | llamacpp | ollama.
    pub engine: String,
    pub port: u16,
    /// What the test token asks for ("" for Ollama: whatever it has in memory).
    pub model: String,
    pub head_node: String,
    pub needs_key: bool,
    pub key_ref: Option<String>,
    /// The address to probe from the head node: 127.0.0.1, or the one address it binds.
    pub host: String,
    /// Members besides the head (an agent there can't vouch for the whole pipeline).
    pub nodes: Vec<String>,
    /// Test tokens go to /v1/chat/completions (the server has no /v1/completions).
    pub chat: bool,
}

/// The Keychain item holding an endpoint's API key: a SparkBox load's own key, or the one added
/// in Providers for a server SparkBox found.
pub fn key_account(e: &Endpoint) -> String {
    match (&e.deployment_id, e.managed_by) {
        (Some(d), ManagedBy::Sparkbox) => super::plan::key_account(d),
        _ => format!("llm:{}", e.provider_id.as_deref().unwrap_or(&e.id)),
    }
}

/// What each agent probes (r3-paging writes them into each head node's agent.json): every running
/// endpoint on the cluster with an engine SparkBox knows, each probed by the agent on its head
/// node. `has_key` says whether the Keychain holds a key.
pub fn agent_targets_from(list: &[Endpoint], chat_only: &BTreeMap<String, bool>, has_key: impl Fn(&str) -> bool) -> Vec<AgentEndpoint> {
    list.iter()
        .filter(|e| e.source != Source::Manual && !e.head_node.is_empty() && e.engine != Engine::Other && !e.gone && !matches!(e.health, Health::Stopped | Health::Gone))
        .map(|e| {
            let account = key_account(e);
            let needs = has_key(&account);
            AgentEndpoint {
                id: e.id.clone(),
                name: label_of(e),
                engine: e.engine.slug().into(),
                port: e.port,
                model: if e.engine == Engine::Ollama { String::new() } else { e.models.first().cloned().unwrap_or_default() },
                head_node: e.head_node.clone(),
                needs_key: needs,
                key_ref: needs.then_some(account),
                host: if e.probe_host.is_empty() { "127.0.0.1".into() } else { e.probe_host.clone() },
                nodes: e.nodes.iter().filter(|n| **n != e.head_node).cloned().collect(),
                chat: chat_only.get(&e.id).copied().unwrap_or(false),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Container, Listener, NetStats, Process};
    use crate::models::plan::{DeploySpec, Stage};

    #[test]
    fn malformed_parallelism_and_restart_totals_do_not_panic() {
        let mut snap = cluster();
        snap.nodes[0].containers[0].command = format!("vllm serve m -tp {} -pp 2", u64::MAX);
        let e = assessed(&snap, &BTreeMap::new()).remove(0);
        assert_eq!(e.health, Health::Unknown);
        assert!(e.health_reason.unwrap().contains("rank count"));
        let mut snap = loaded("tp4");
        for n in &mut snap.nodes { for c in &mut n.containers { c.labels.insert("sparkbox.parallel".into(), format!("{}x2", u64::MAX)); } }
        assert_eq!(assessed(&snap, &BTreeMap::new())[0].health, Health::Unknown);
        for value in ["0x1", "bad", "4xno"] {
            for n in &mut snap.nodes { for c in &mut n.containers { c.labels.insert("sparkbox.parallel".into(), value.into()); } }
            assert_eq!(assessed(&snap, &BTreeMap::new())[0].health, Health::Unknown, "{value}");
        }
        let mut snap = cluster();
        for n in &mut snap.nodes { for c in &mut n.containers { c.restart_count = u32::MAX; } }
        assert_eq!(assessed(&snap, &BTreeMap::new())[0].restarts, Some(u32::MAX));
    }

    #[test]
    fn a_worker_never_belongs_to_two_heads_at_the_same_address() {
        let mut snap = cluster();
        snap.nodes[0].containers[0].command = "vllm serve m -tp 2 --port 8000 --master-addr 192.168.100.1 --master-port 29500".into();
        let mut second = snap.nodes[0].containers[0].clone();
        second.id = "second-head".into();
        second.command = second.command.replace("8000", "8001").replace("29500", "29501");
        snap.nodes[0].containers.push(second);
        snap.nodes[1].containers[0].command = "vllm serve m --headless --node-rank 1 --master-addr 192.168.100.1 --master-port 29500".into();
        snap.nodes[2].containers[0].command = snap.nodes[1].containers[0].command.replace("29500", "29501");
        let list = discover(&snap, &Inventory::example());
        assert_eq!(list[0].nodes, ["cell1", "cell2"]);
        assert_eq!(list[1].nodes, ["cell1", "cell3"]);
        for c in &mut snap.nodes[0].containers { c.command = c.command.replace(" --master-port 29500", "").replace(" --master-port 29501", ""); }
        let list = discover(&snap, &Inventory::example());
        assert_eq!(list[0].nodes, ["cell1"], "ambiguous address must not guess");
        assert_eq!(list[1].nodes, ["cell1"]);
    }

    #[test]
    fn deployment_labels_cannot_cross_instances_or_fall_back_to_an_address() {
        let mut snap = cluster();
        for n in &mut snap.nodes[..3] { for c in &mut n.containers {
            c.labels.insert("sparkbox.deployment".into(), "d1".into());
            c.labels.insert("sparkbox.instance".into(), "release".into());
        } }
        snap.nodes[1].containers[0].labels.insert("sparkbox.instance".into(), "dev".into());
        snap.nodes[2].containers[0].labels.clear();
        let list = discover(&snap, &Inventory::example());
        assert_eq!(list[0].nodes, ["cell1"]);
    }

    #[test]
    fn stale_without_an_age_is_unknown_and_a_fresh_report_recovers() {
        let mut snap = cluster();
        snap.nodes[1].stale = true;
        snap.nodes[1].frame_age_ms = None;
        let e = assessed(&snap, &llama_track()).remove(0);
        assert_eq!(e.health, Health::Unknown);
        assert_eq!(e.nodes, ["cell1", "cell2", "cell3"]);
        assert_eq!(e.last_ok_ms, Some(NOW * 1000 - 5_000));
        assert!(!e.forgettable && e.metrics.is_none());
        snap.nodes[1].stale = false;
        assert_eq!(assessed(&snap, &llama_track())[0].health, Health::Ready);
        snap.nodes[1].containers[0].state = "exited".into();
        snap.nodes[1].containers[0].exit_code = 1;
        assert_eq!(assessed(&snap, &llama_track())[0].health, Health::Down, "fresh evidence still reports a failure");
    }

    #[test]
    fn engine_command_lines_are_read() {
        let p = parse_command(
            "vllm serve /models/meta-llama/Llama-3.3-70B-Instruct --served-model-name llama-3.3-70b --pipeline-parallel-size 3 \
             --distributed-executor-backend ray --max-model-len 32768 --port 8000",
        )
        .unwrap();
        assert_eq!(p.engine, Engine::Vllm);
        assert_eq!(p.served.as_deref(), Some("llama-3.3-70b"));
        assert_eq!(p.model.as_deref(), Some("/models/meta-llama/Llama-3.3-70B-Instruct"));
        assert_eq!((p.tp, p.pp, p.port, p.max_len), (1, 3, Some(8000), Some(32768)));

        let p = parse_command("python3 -m vllm.entrypoints.openai.api_server --model Qwen/Qwen3-8B -tp=2 --port=8001").unwrap();
        assert_eq!((p.engine, p.model.as_deref(), p.tp, p.port), (Engine::Vllm, Some("Qwen/Qwen3-8B"), 2, Some(8001)));

        // SparkBox's own multi-node head: vllm runs inside a quoted bash script.
        let p = parse_command(
            "bash -c ray start --head --port=6379; exec vllm \"serve\" \"/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8\" \
             \"--served-model-name\" \"qwen3.8-27b\" \"--host\" \"192.168.50.51\" \"--port\" \"8001\" \"--tensor-parallel-size\" \"2\"",
        )
        .unwrap();
        assert_eq!((p.served.as_deref(), p.port, p.tp, p.host.as_deref()), (Some("qwen3.8-27b"), Some(8001), 2, Some("192.168.50.51")));

        let p = parse_command("python3 -m sglang.launch_server --model-path Qwen/Qwen3-32B --tp 2 --port 30000").unwrap();
        assert_eq!((p.engine, p.model.as_deref(), p.tp), (Engine::Sglang, Some("Qwen/Qwen3-32B"), 2));
        assert!(
            parse_command("python3 -m sglang.launch_server --model-path Qwen/Qwen3-32B --tp 2 --nnodes 2 --node-rank 1 --dist-init-addr 192.168.100.1:5000").is_none(),
            "rank 1 of a multi-node SGLang serves no API"
        );
        let p = parse_command("/opt/llama.cpp/build/bin/llama-server -m /models/gpt-oss-20b.gguf --port 8081 -c 16384").unwrap();
        assert_eq!((p.engine, p.port, p.max_len), (Engine::Llamacpp, Some(8081), Some(16384)));
        assert_eq!(parse_command("/bin/ollama serve").unwrap().engine, Engine::Ollama);
        assert!(parse_command("ollama runner --model /root/.ollama/models/blobs/sha256-x").is_none());
        assert!(parse_command("ray start --address=192.168.100.1:6379 --block").is_none());
        assert!(parse_command("bash start.sh").is_none());
    }

    #[test]
    fn trtllm_serve_and_nim_images_are_recognised() {
        let p = parse_command("mpirun -n 2 --allow-run-as-root trtllm-serve /models/nvidia/Llama-3.3-70B-Instruct-FP4 --host 0.0.0.0 --port 8002 --tp_size 2 --max_seq_len 16384").unwrap();
        assert_eq!((p.engine, p.port, p.tp, p.max_len), (Engine::Trtllm, Some(8002), 2, Some(16384)));
        assert_eq!(p.model.as_deref(), Some("/models/nvidia/Llama-3.3-70B-Instruct-FP4"));
        let p = parse_command("trtllm-serve serve nvidia/Qwen3-8B-FP8 --port 8000").unwrap();
        assert_eq!(p.model.as_deref(), Some("nvidia/Qwen3-8B-FP8"), "the `serve` subcommand isn't the model");

        let nim = Container {
            name: "nim-llama".into(),
            image: "nvcr.io/nim/meta/llama-3.1-8b-instruct:1.3.3".into(),
            command: "/opt/nvidia/nvidia_entrypoint.sh /opt/nim/start_server.sh".into(),
            ..Default::default()
        };
        let p = parse_container(&nim).unwrap();
        assert_eq!((p.engine, p.served.as_deref()), (Engine::Nim, Some("meta/llama-3.1-8b-instruct")));
        assert_eq!(Engine::Nim.default_port(), 8000);
        assert!(parse_container(&Container { image: "nvcr.io/nvidia/vllm:26.05-py3".into(), command: "bash".into(), ..Default::default() }).is_none());
        assert_eq!(serde_json::to_value(Engine::Trtllm).unwrap(), "trtllm");
    }

    #[test]
    fn an_sglang_rank_joins_its_head_like_a_ray_worker() {
        let mut snap = cluster();
        let head = "python3 -m sglang.launch_server --model-path /m/Qwen/Qwen3-32B --tp 2 --nnodes 2 --node-rank 0 --dist-init-addr 192.168.100.4:5000 --port 30000";
        snap.nodes[3].containers.push(container("sglang-0", head, &[]));
        snap.nodes[2].containers.push(container("sglang-1", &head.replace("--node-rank 0", "--node-rank 1"), &[]));
        snap.nodes[3].listeners.push(listen(30000));
        let eps = assessed(&snap, &BTreeMap::new());
        let e = eps.iter().find(|e| e.engine == Engine::Sglang).unwrap();
        assert_eq!((e.id.as_str(), e.nodes.as_slice()), ("gx10-0000-sglang-30000", ["gx10-0000".to_string(), "cell3".to_string()].as_slice()));
        assert_eq!(e.health, Health::Ready);
    }

    #[test]
    fn repos_are_read_from_model_paths() {
        assert_eq!(repo_of("/models/meta-llama/Llama-3.3-70B-Instruct").as_deref(), Some("meta-llama/Llama-3.3-70B-Instruct"));
        assert_eq!(repo_of("/home/u/.cache/huggingface/hub/models--Qwen--Qwen3-8B/snapshots/abc").as_deref(), Some("Qwen/Qwen3-8B"));
        assert_eq!(repo_of("Qwen/Qwen3-8B").as_deref(), Some("Qwen/Qwen3-8B"));
    }

    const NOW: u64 = 1_800_000_000;

    fn node(id: &str, ip: &str, fabric: &str) -> NodeSnapshot {
        NodeSnapshot {
            id: id.into(),
            name: id.into(),
            host: format!("{id}.local"),
            status: LinkStatus::Online,
            nets: vec![
                NetStats { iface: "enP7s7".into(), role: NetRole::Lan, ip: Some(ip.into()), ..Default::default() },
                NetStats { iface: "enp1s0f1np1".into(), role: NetRole::Fabric, ip: Some(fabric.into()), ..Default::default() },
            ],
            ..Default::default()
        }
    }

    fn container(name: &str, cmd: &str, ports: &[&str]) -> Container {
        Container {
            id: format!("{name}-id"),
            name: name.into(),
            state: "running".into(),
            started_at: NOW - 3600,
            command: cmd.into(),
            ports: ports.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        }
    }

    fn listen(port: u16) -> Listener {
        Listener { port, addr: "0.0.0.0".into(), pid: None, process: Some("python3".into()), ..Default::default() }
    }

    /// The demo world: Llama 70B over Ray on cell1–cell3, Ollama on gx10-0000.
    fn cluster() -> ClusterSnapshot {
        let mut c1 = node("cell1", "192.168.50.51", "192.168.100.1");
        c1.containers = vec![container(
            "vllm-head",
            "vllm serve /models/meta-llama/Llama-3.3-70B-Instruct --served-model-name llama-3.3-70b --pipeline-parallel-size 3 --port 8000",
            &[],
        )];
        c1.listeners = vec![listen(8000), listen(6379)];
        let mut workers = Vec::new();
        for (i, id) in ["cell2", "cell3"].iter().enumerate() {
            let mut n = node(id, &format!("192.168.50.{}", 52 + i), &format!("192.168.100.{}", 2 + i));
            n.containers = vec![container("vllm-worker", "ray start --address=192.168.100.1:6379 --node-ip-address=x --block", &[])];
            n.containers[0].id = format!("vllm-worker-{id}");
            workers.push(n);
        }
        let mut gx = node("gx10-0000", "192.168.50.54", "192.168.100.4");
        gx.containers = vec![container("ollama", "/bin/ollama serve", &["0.0.0.0:11434->11434/tcp"])];
        gx.listeners = vec![listen(11434)];
        ClusterSnapshot { ts_ms: NOW * 1000, nodes: [vec![c1], workers, vec![gx]].concat(), ..Default::default() }
    }

    /// A server that answered its model list at `ms` and gave a test token then.
    fn answered(ms: u64) -> Track {
        Track { last_ok_ms: Some(ms), canary_ok_ms: Some(ms), canary: Some(Canary { at_ms: ms, ok: true, latency_ms: Some(84), error: None, after_round: None }), ..Default::default() }
    }

    /// Discovery plus assessment, as the round does it, with probes that answered just now.
    fn assessed(snap: &ClusterSnapshot, tracks: &BTreeMap<String, Track>) -> Vec<Endpoint> {
        let mut eps = discover(snap, &Inventory::example());
        for e in &mut eps {
            let ok = answered(NOW * 1000 - 5_000);
            assess(e, tracks.get(&e.id).unwrap_or(&ok), snap, NOW * 1000);
        }
        eps
    }

    fn llama_track() -> BTreeMap<String, Track> {
        let t = Track { members: vec!["cell1".into(), "cell2".into(), "cell3".into()], ..answered(NOW * 1000 - 5_000) };
        BTreeMap::from([("cell1-vllm-8000".to_string(), t)])
    }

    #[test]
    fn the_demo_cluster_shows_llama_over_three_nodes_and_ollama() {
        let eps = assessed(&cluster(), &BTreeMap::new());
        assert_eq!(eps.len(), 2, "{eps:?}");
        let v = &eps[0];
        assert_eq!(v.id, "cell1-vllm-8000");
        assert_eq!(v.models, ["llama-3.3-70b"]);
        assert_eq!(v.nodes, ["cell1", "cell2", "cell3"], "workers found by their --address");
        assert_eq!(v.expected_nodes, v.nodes);
        assert_eq!(v.parallel, Some(Parallel { tp: 1, pp: 3 }));
        assert_eq!(v.url, "http://192.168.50.51:8000/v1");
        assert_eq!((v.health, v.managed_by, v.listen.as_str()), (Health::Ready, ManagedBy::External, "all"));
        assert!(v.health_reason.is_none() && v.missing_nodes.is_empty());
        assert_eq!(v.container_names, ["vllm-head"]);
        let o = &eps[1];
        assert_eq!((o.id.as_str(), o.engine, o.port), ("gx10-0000-ollama-11434", Engine::Ollama, 11434));
        assert_eq!(o.url, "http://192.168.50.54:11434/v1");
        let p = provider_for(o, false);
        assert!(p.is_none(), "no models known until the probe lists them");
    }

    #[test]
    fn a_planned_restart_of_a_stage_node_is_paused_never_down() {
        let mut snap = cluster();
        let started = NOW * 1000 - 60_000;
        snap.nodes[2] = NodeSnapshot {
            id: "cell3".into(),
            name: "cell3".into(),
            status: LinkStatus::Offline,
            power_state: Some("rebooting".into()),
            maintenance: Some(crate::model::MaintInfo { until_ms: Some(NOW * 1000 + 300_000), reason: "restarting".into(), started_ms: started }),
            ..Default::default()
        };
        let v = &assessed(&snap, &llama_track())[0];
        assert_eq!(v.health, Health::Paused, "{v:?}");
        let at = crate::health::maintenance::local_hhmm(started);
        assert_eq!(v.health_reason.as_deref(), Some(format!("cell3 restarting (planned by SparkBox, {at})").as_str()));
        // Past its expected time and grace, unavailable is Unknown again.
        snap.nodes[2].maintenance.as_mut().unwrap().until_ms = Some(NOW * 1000 - 200_000);
        assert_eq!(assessed(&snap, &llama_track())[0].health, Health::Unknown);
        // Back and reporting: what it really does shows, not Paused (Ready here).
        let snap = cluster();
        let mut back = snap.clone();
        back.nodes[2].maintenance = Some(crate::model::MaintInfo { until_ms: Some(NOW * 1000 + 300_000), reason: "restarting".into(), started_ms: started });
        assert_eq!(assessed(&back, &llama_track())[0].health, Health::Ready);
        // An ordered restart SparkBox runs pauses it from the first stop.
        let mut e = assessed(&snap, &llama_track())[0].clone();
        e.health = Health::Stopped;
        pause_for_restart(&mut e, started);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Paused, Some(format!("llama-3.3-70b restarting in order (planned by SparkBox, {at})").as_str())));
    }

    #[test]
    fn pp3_with_a_stage_node_offline_is_unknown_with_its_reason() {
        let mut snap = cluster();
        // An offline node reports nothing: no containers, no listeners.
        snap.nodes[2] = NodeSnapshot { id: "cell3".into(), name: "cell3".into(), status: LinkStatus::Offline, ..Default::default() };
        let eps = assessed(&snap, &llama_track());
        let v = &eps[0];
        assert_eq!(v.health, Health::Unknown, "{v:?}");
        assert_eq!(v.health_reason.as_deref(), Some("Pipeline stage 3 of 3 on cell3 is offline"));
        assert_eq!(v.message, v.health_reason, "the alert text is the reason");
        assert_eq!(v.missing_nodes, ["cell3"]);
        assert_eq!(v.nodes, ["cell1", "cell2", "cell3"], "the missing node stays in the list");
        assert!(provider_for(v, false).is_none(), "Chat doesn't offer it");

        // Without memory of cell3 the shortfall still shows: 2 nodes for a 3-stage pipeline.
        let eps = assessed(&snap, &BTreeMap::new());
        assert_eq!(eps[0].health, Health::Down);
        assert_eq!(eps[0].health_reason.as_deref(), Some("Needs 3 nodes but only 2 are running"));

        // A node that stopped reporting can't vouch for its stage either.
        let mut snap = cluster();
        snap.nodes[1].stale = true;
        snap.nodes[1].frame_age_ms = Some(25_000);
        let v = &assessed(&snap, &llama_track())[0];
        assert_eq!(v.health_reason.as_deref(), Some("cell2 hasn't reported for 25 s (pipeline stage 2 of 3)"));
        snap.nodes[1].frame_age_ms = Some(4_000);
        assert_eq!(assessed(&snap, &llama_track())[0].health, Health::Ready, "a few seconds late is fine");
    }

    #[test]
    fn a_missing_middle_stage_keeps_its_number() {
        // cell2 (stage 2) went offline: its node reports nothing, so discovery finds only
        // cell1 and cell3. The stage comes from the order it last ran in, not the list's.
        let mut snap = cluster();
        snap.nodes[1] = NodeSnapshot { id: "cell2".into(), name: "cell2".into(), status: LinkStatus::Offline, ..Default::default() };
        let v = &assessed(&snap, &llama_track())[0];
        assert_eq!(v.health_reason.as_deref(), Some("Pipeline stage 2 of 3 on cell2 is offline"));
        assert_eq!(v.nodes, ["cell1", "cell2", "cell3"], "stages in pipeline order");

        // SGLang names its ranks on the command line: rank 2 is stage 3 wherever it runs.
        let rank = |r: u32| Member::of("x", false, &container("s", &format!("python3 -m sglang.launch_server --node-rank {r}"), &[])).stage;
        assert_eq!((rank(1), rank(2)), (Some(1), Some(2)));
        let mut c = container("w", "ray start --address=192.168.100.1:6379 --block", &[]);
        c.labels.insert("sparkbox.rank".into(), "2".into());
        assert_eq!(Member::of("cell3", false, &c).stage, Some(2), "SparkBox's own label");
    }

    #[test]
    fn ready_needs_a_token_not_just_a_model_list() {
        let snap = cluster();
        let now = NOW * 1000;
        // Answered its model list, no test token yet: checking, not Ready.
        let mut t = Track { last_ok_ms: Some(now - 2_000), members: llama_track()["cell1-vllm-8000"].members.clone(), ..Default::default() };
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), t.clone())]))[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Loading, Some("Checking that it generates…")));
        assert!(canary_due(&t, now));

        // The test token comes back: Ready, and the next one is due a minute later.
        record_canary(&mut t, Canary { at_ms: now - 1_000, ok: true, latency_ms: Some(612), error: None, after_round: None });
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), t.clone())]))[0];
        assert_eq!(v.health, Health::Ready);
        assert_eq!(v.canary.as_ref().and_then(|c| c.latency_ms), Some(612));
        assert!(!canary_due(&t, now) && canary_due(&t, now + 60_000));

        // Two minutes on with no new proof, it can't vouch for itself any more.
        t.last_ok_ms = Some(now + 125_000);
        let mut e = discover(&snap, &Inventory::example()).remove(0);
        assess(&mut e, &t, &snap, now + 130_000);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Down, Some("No token for 131 s")));

        // One token taking 12.4 s is Slow: still Ready, with a note.
        record_canary(&mut t, Canary { at_ms: now + 129_000, ok: true, latency_ms: Some(12_400), error: None, after_round: None });
        assess(&mut e, &t, &snap, now + 130_000);
        assert_eq!((e.health, e.health_note.as_deref()), (Health::Ready, Some("Slow: 1 token took 12.4 s")));
    }

    #[test]
    fn a_test_token_failing_twice_is_down() {
        let snap = cluster();
        let now = NOW * 1000;
        let mut t = llama_track().remove("cell1-vllm-8000").unwrap();
        let fail = |at| Canary { at_ms: at, ok: false, latency_ms: None, error: Some("HTTP 500: EngineCore encountered an issue.".into()), after_round: None };
        record_canary(&mut t, fail(now - 20_000));
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), t.clone())]))[0];
        assert_eq!(v.health, Health::Ready, "one miss isn't an outage: the last minute's token still counts");
        record_canary(&mut t, fail(now - 5_000));
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), t.clone())]))[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Down, Some("Didn't generate: HTTP 500: EngineCore encountered an issue. (2 tries)")));
    }

    #[test]
    fn test_tokens_that_find_the_channels_busy_leave_the_proof_as_it_was() {
        let snap = cluster();
        let now = NOW * 1000;
        let id = "cell1-vllm-8000".to_string();
        let mut t = llama_track().remove(&id).unwrap();
        let v = |t: &Track, at: u64| {
            let mut e = discover(&snap, &Inventory::example()).remove(0);
            assess(&mut e, t, &snap, at);
            (e.health, e.health_reason, e.health_note)
        };
        // The probes get through; every test token from a second on finds no channel free.
        record_canary_busy(&mut t, now + 1_000);
        record_canary_busy(&mut t, now + 61_000);
        t.last_ok_ms = Some(now + 150_000);
        let (h, why, note) = v(&t, now + 150_000);
        assert_eq!((h, why), (Health::Ready, None), "a token that was never sent isn't a missing token");
        assert!(note.as_deref().is_some_and(|n| n.starts_with("Not tested since ") && n.ends_with(": SSH channels busy on cell1")), "{note:?}");

        // A token that fails once after it: the usual second try, no note claiming it wasn't tested.
        record_canary(&mut t, Canary { at_ms: now + 151_000, ok: false, latency_ms: None, error: Some("HTTP 500".into()), after_round: None });
        let (h, _, note) = v(&t, now + 152_000);
        assert_eq!((h, note), (Health::Ready, None));
        record_canary(&mut t, Canary { at_ms: now + 166_000, ok: false, latency_ms: None, error: Some("HTTP 500".into()), after_round: None });
        assert_eq!(v(&t, now + 167_000).0, Health::Down, "two in a row, as ever");

        // One that comes back ends the spell: from then on the proof ages as usual.
        record_canary(&mut t, Canary { at_ms: now + 181_000, ok: true, latency_ms: Some(90), error: None, after_round: None });
        assert_eq!(t.untested_since_ms, None);
        t.last_ok_ms = Some(now + 310_000);
        assert_eq!(v(&t, now + 310_000).1.as_deref(), Some("No token for 129 s"));
    }

    #[test]
    fn engine_health_live_traffic_and_stalls() {
        let snap = cluster();
        let now = NOW * 1000;
        let fresh = || Track { last_ok_ms: Some(now - 1_000), health_ok_ms: Some(now - 1_000), members: llama_track()["cell1-vllm-8000"].members.clone(), ..Default::default() };
        let verdict = |t: &Track, at: u64| {
            let mut e = discover(&snap, &Inventory::example()).remove(0);
            assess(&mut e, t, &snap, at);
            (e.health, e.health_reason, e.stalled_s)
        };
        // Tokens rising while /health is fine are proof: no test token needed.
        let mut t = fresh();
        record_scrape(&mut t, Some(1000.0), 3.0, 0.0, true, now - 6_000);
        record_scrape(&mut t, Some(1012.0), 3.0, 0.0, true, now - 3_000);
        assert_eq!(t.passive_ok_ms, Some(now - 3_000));
        assert!(!canary_due(&t, now), "live traffic counts instead");
        assert_eq!(verdict(&t, now).0, Health::Ready);

        // Three requests running and the counter standing still: Stalled once it has for 60 s.
        let mut t = fresh();
        t.canary_ok_ms = Some(now - 1_000);
        for i in 0..=20u64 {
            record_scrape(&mut t, Some(5000.0), 3.0, 0.0, true, now + i * 3_000);
        }
        // Flat from the first reading at `now` to the last at now + 60 s.
        let (h, why, stalled) = verdict(&Track { health_ok_ms: Some(now + 59_000), last_ok_ms: Some(now + 59_000), ..t.clone() }, now + 60_000);
        assert_eq!((h, why.as_deref(), stalled), (Health::Down, Some("Stalled: 3 requests running, no tokens for 60 s"), Some(60)));
        // A longer wait from the endpoint-stalled rule.
        let (h, _, stalled) = verdict(&Track { stall_after_s: 90, health_ok_ms: Some(now + 59_000), last_ok_ms: Some(now + 59_000), ..t.clone() }, now + 60_000);
        assert_eq!((h, stalled), (Health::Ready, Some(60)), "stalledS shows on the card before it's Down");
        // Nothing running or waiting is idle, not stalled.
        let mut idle = fresh();
        for i in 0..20u64 {
            record_scrape(&mut idle, Some(5000.0), 0.0, 0.0, true, now + i * 3_000);
        }
        assert_eq!(idle.flat_since_ms, None);

        // The API server answers but its engine is dead: /health 503 twice.
        let mut t = fresh();
        t.canary_ok_ms = Some(now - 1_000);
        record_health(&mut t, 503, "/health", now - 15_000);
        assert_eq!(verdict(&t, now).0, Health::Ready, "one 503 isn't an outage");
        record_health(&mut t, 503, "/health", now);
        assert_eq!(verdict(&t, now).1.as_deref(), Some("Engine not healthy: /health 503"));
        record_health(&mut t, 0, "/health", now);
        assert_eq!(t.health_fails, 2, "no answer at all is the model-list probe's to count");
    }

    #[test]
    fn a_gpu_fault_on_any_member_downs_the_endpoint() {
        let mut snap = cluster();
        snap.nodes[2].gpu_fault = Some(crate::model::GpuFault { code: Some(79), text: "GPU has fallen off the bus (Xid 79)".into(), since_ms: NOW * 1000 - 60_000, source: "xid".into() });
        let eps = assessed(&snap, &llama_track());
        let v = &eps[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Down, Some("GPU fault on cell3: Xid 79 (fell off the bus)")));
        assert_eq!(v.missing_nodes, ["cell3"]);
        assert_eq!(eps[1].health, Health::Ready, "gx10-0000's Ollama is fine");
        snap.nodes[3].gpu_fault = Some(crate::model::GpuFault { code: None, text: "GPU readings unreadable for 45 s".into(), since_ms: 0, source: "unreadable".into() });
        assert_eq!(assessed(&snap, &llama_track())[1].health_reason.as_deref(), Some("GPU fault on gx10-0000: GPU readings unreadable for 45 s"));
    }

    #[test]
    fn test_tokens_go_by_curl_with_the_key_on_stdin() {
        let req = CanaryRequest { model: "llama-3.3-70b".into(), chat: false, keep_alive_s: None };
        let cmd = canary_command(Engine::Vllm, "127.0.0.1", 8000, &req, true);
        assert!(cmd.starts_with("curl -s -m 30 -w '\\n%{http_code} %{time_total}' -H @- -H 'Content-Type: application/json' -d '{"), "{cmd}");
        assert!(cmd.contains(r#""max_tokens":1"#) && cmd.contains(r#""prompt":"Hi""#) && cmd.contains("http://127.0.0.1:8000/v1/completions"));
        assert!(!canary_command(Engine::Vllm, "127.0.0.1", 8000, &req, false).contains("@-"));
        // A model name with a quote can't break out of the shell word.
        let evil = CanaryRequest { model: "x'; reboot; '".into(), ..req.clone() };
        assert!(canary_command(Engine::Vllm, "127.0.0.1", 8000, &evil, false).contains(r#""model":"x'\''; reboot; '\''""#));
        // Ollama keeps the model exactly as long as it had left.
        let o = canary_command(Engine::Ollama, "127.0.0.1", 11434, &CanaryRequest { model: "gpt-oss:20b".into(), chat: false, keep_alive_s: Some(212) }, false);
        assert!(o.contains(r#""keep_alive":"212s""#) && o.contains(r#""num_predict":1"#) && o.contains("/api/generate"), "{o}");

        let done = |out: &str| match parse_canary(Engine::Vllm, out, 7) {
            CanaryReply::Done(c) => (c.ok, c.latency_ms, c.error),
            other => panic!("{other:?}"),
        };
        let body = include_str!("fixtures/probe/completion.json").trim();
        assert_eq!(done(&format!("{body}\n200 0.084112 0\n")), (true, Some(84), None));
        assert_eq!(done("\n000 30.001 28\n"), (false, None, Some("no reply in 30 s".into())));
        assert_eq!(done("\n000 0.000210 7\n"), (false, None, Some("connection refused".into())));
        assert_eq!(done(&format!("{}\n500 0.2 0", include_str!("fixtures/probe/engine_dead.json").trim())).2.as_deref(), Some("HTTP 500: EngineCore encountered an issue. See stack trace (above) for the root cause."));
        assert_eq!(done(" 127\n").2.as_deref(), Some("curl isn't installed on the node"));
        assert_eq!(parse_canary(Engine::Vllm, "{\"detail\":\"Not Found\"}\n404 0.01 0", 1), CanaryReply::ChatOnly);
        assert_eq!(parse_canary(Engine::Vllm, "Unauthorized\n401 0.01 0", 1), CanaryReply::NeedsKey);
        // A server that wants a key SparkBox doesn't have is asked once a minute, not every round.
        let t = Track { untestable: true, canary_tried_ms: Some(1_000), ..Default::default() };
        assert!(!canary_due(&t, 30_000) && canary_due(&t, 61_000));
        let gen = include_str!("fixtures/probe/ollama_generate.json").trim();
        assert!(matches!(parse_canary(Engine::Ollama, &format!("{gen}\n200 0.15 0"), 1), CanaryReply::Done(Canary { ok: true, .. })));
    }

    #[test]
    fn agents_get_every_running_endpoint_and_only_a_key_reference() {
        let mut eps = assessed(&cluster(), &llama_track());
        eps[0].provider_id = Some("cell1-vllm-8000".into());
        let chat = BTreeMap::from([("gx10-0000-ollama-11434".to_string(), false)]);
        let t = agent_targets_from(&eps, &chat, |account| account == "llm:cell1-vllm-8000");
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].id.as_str(), t[0].engine.as_str(), t[0].port, t[0].model.as_str()), ("cell1-vllm-8000", "vllm", 8000, "llama-3.3-70b"));
        assert_eq!((t[0].needs_key, t[0].key_ref.as_deref(), t[0].host.as_str()), (true, Some("llm:cell1-vllm-8000"), "127.0.0.1"));
        assert_eq!(t[0].nodes, ["cell2", "cell3"]);
        assert_eq!((t[1].engine.as_str(), t[1].model.as_str(), t[1].needs_key, t[1].key_ref.clone()), ("ollama", "", false, None));
        let v = serde_json::to_value(&t[0]).unwrap();
        for key in ["id", "name", "engine", "port", "model", "headNode", "needsKey", "keyRef", "host"] {
            assert!(v.get(key).is_some(), "agent.json needs {key}: {v}");
        }
    }

    #[test]
    fn a_worker_that_exited_is_named() {
        let mut snap = cluster();
        let w = &mut snap.nodes[2].containers[0];
        w.state = "exited".into();
        w.exit_code = 1;
        w.finished_at = NOW - 130;
        let v = &assessed(&snap, &llama_track())[0];
        assert_eq!(v.health, Health::Down);
        assert_eq!(v.health_reason.as_deref(), Some("Worker on cell3 isn't running (exit 1, 2 min ago) · pipeline stage 3 of 3"));
        assert_eq!(v.missing_nodes, ["cell3"]);
    }

    #[test]
    fn a_head_that_exited_stays_listed_as_down() {
        let mut snap = cluster();
        snap.nodes[0].listeners.retain(|l| l.port != 8000);
        let h = &mut snap.nodes[0].containers[0];
        h.state = "exited".into();
        h.exit_code = 1;
        h.finished_at = NOW - 190;
        let mut tracks = llama_track();
        tracks.get_mut("cell1-vllm-8000").unwrap().last_log = Some("RayActorError: The actor died unexpectedly".into());
        let v = &assessed(&snap, &tracks)[0];
        assert_eq!(v.id, "cell1-vllm-8000", "still listed");
        assert_eq!(v.health, Health::Down);
        assert_eq!(v.health_reason.as_deref(), Some("Head exited (code 1) 3 min ago · RayActorError: The actor died unexpectedly"));
        assert!(v.forgettable);

        // Stopped by hand (exit 0) reads Stopped, not Down.
        snap.nodes[0].containers[0].exit_code = 0;
        let v = &assessed(&snap, &tracks)[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Stopped, Some("Stopped 3 min ago")));

        // Killed by the kernel while loading.
        let h = &mut snap.nodes[0].containers[0];
        h.exit_code = 137;
        h.oom_killed = true;
        assert_eq!(assessed(&snap, &tracks)[0].health_reason.as_deref(), Some("Head ran out of memory (exit 137) 3 min ago"));

        // A day later it ages out of discovery.
        snap.nodes[0].containers[0].finished_at = NOW - KEEP_GONE_S - 1;
        assert!(!assessed(&snap, &tracks).iter().any(|e| e.id == "cell1-vllm-8000"));

        // A replacement that's running wins over the exited one with the same port.
        let mut snap = cluster();
        let mut old = snap.nodes[0].containers[0].clone();
        old.id = "old".into();
        old.state = "exited".into();
        old.exit_code = 1;
        old.started_at = NOW - 9000;
        snap.nodes[0].containers.insert(0, old);
        let v = &assessed(&snap, &llama_track())[0];
        assert_eq!((v.health, v.container_ids.as_slice()), (Health::Ready, ["vllm-head-id".to_string()].as_slice()));
    }

    #[test]
    fn a_crash_loop_is_down_never_loading() {
        let mut snap = cluster();
        snap.nodes[0].listeners.retain(|l| l.port != 8000); // the model reloads between crashes
        let w = &mut snap.nodes[1].containers[0];
        w.state = "restarting".into();
        w.exit_code = 1;
        w.restart_count = 5;
        let mut tracks = llama_track();
        tracks.get_mut("cell1-vllm-8000").unwrap().restarts.insert("cell2".into(), (5, NOW - 20));
        let v = &assessed(&snap, &tracks)[0];
        assert_eq!(v.health, Health::Down);
        assert_eq!(v.health_reason.as_deref(), Some("Crash loop on cell2: 5 restarts in 10 min · last exit 1"));
        assert_eq!(v.restarts, Some(5));
        assert_eq!(v.nodes, ["cell1", "cell2", "cell3"], "stages keep the cluster's order, whatever state a worker is in");
        assert!(!v.forgettable, "its head still runs: nothing to forget");

        // Docker restarting it once is Down too, but not called a loop yet.
        tracks.get_mut("cell1-vllm-8000").unwrap().restarts.insert("cell2".into(), (1, NOW - 20));
        let v = &assessed(&snap, &tracks)[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Down, Some("Restarting on cell2 after exit 1 · 1 restart in 10 min")));
        tracks.get_mut("cell1-vllm-8000").unwrap().restarts.insert("cell2".into(), (5, NOW - 20));

        // Between crashes the worker is up, but three restarts in 10 minutes is still a loop.
        snap.nodes[1].containers[0].state = "running".into();
        snap.nodes[1].containers[0].started_at = NOW - 15;
        assert_eq!(assessed(&snap, &tracks)[0].health, Health::Down);
        // Once it has stayed up a while it's Loading again (the model reloads), then Ready.
        tracks.get_mut("cell1-vllm-8000").unwrap().restarts.insert("cell2".into(), (5, NOW - 300));
        assert_eq!(assessed(&snap, &tracks)[0].health, Health::Loading);
    }

    #[test]
    fn probe_failures_make_it_down() {
        let snap = cluster();
        let mut t = llama_track().remove("cell1-vllm-8000").unwrap();
        t.last_ok_ms = None;
        let fail = Probe { ok: false, error: Some("connection refused".into()), at_ms: NOW * 1000, ..Default::default() };
        record_probe(&mut t, &fail);
        let tracks = BTreeMap::from([("cell1-vllm-8000".to_string(), t.clone())]);
        let v = &assessed(&snap, &tracks)[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Loading, Some("Not answering yet (connection refused)")), "one failure isn't an outage");
        record_probe(&mut t, &fail);
        let tracks = BTreeMap::from([("cell1-vllm-8000".to_string(), t.clone())]);
        let v = &assessed(&snap, &tracks)[0];
        // The reason names the path the probes take: through the node, to its own address.
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Down, Some("No answer from 127.0.0.1:8000 on cell1 (over SSH)")));
        // Listening isn't enough: the last answer must be within a minute.
        let t = Track { last_ok_ms: Some(NOW * 1000 - 90_000), ..llama_track().remove("cell1-vllm-8000").unwrap() };
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), t)]))[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Down, Some("No answer for 90 s")));
        record_probe(&mut Track::default(), &Probe { ok: true, at_ms: 1, ..Default::default() });
    }

    #[test]
    fn recovery_goes_loading_with_progress_then_ready() {
        let mut snap = cluster();
        snap.nodes[0].listeners.retain(|l| l.port != 8000);
        snap.nodes[0].containers[0].started_at = NOW - 60;
        let mut t = llama_track().remove("cell1-vllm-8000").unwrap();
        t.last_ok_ms = None;
        t.pct = Some(42);
        t.progress_ms = Some(NOW * 1000 - 3_000);
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), t.clone())]))[0];
        assert_eq!((v.health, v.loading_pct), (Health::Loading, Some(42)));
        // Stuck: a server SparkBox didn't start, with no progress for 20 minutes.
        snap.nodes[0].containers[0].started_at = NOW - 3600;
        t.progress_ms = Some(NOW * 1000 - 21 * 60_000);
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), t.clone())]))[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Down, Some("Didn't become ready in 20 min")));
        // Listening and answered: Ready.
        let snap = cluster();
        t.last_ok_ms = Some(NOW * 1000 - 1_000);
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), t)]))[0];
        assert_eq!((v.health, v.loading_pct, v.last_ok_ms), (Health::Ready, None, Some(NOW * 1000 - 1_000)));
    }

    #[test]
    fn two_servers_on_one_node_get_different_ids() {
        let mut snap = cluster();
        snap.nodes[3].processes = vec![Process {
            pid: 9,
            cmd: "/usr/bin/llama-server -m /m/q.gguf --port 8081".into(),
            ports: vec![8081],
            ..Default::default()
        }];
        snap.nodes[3].listeners.push(listen(8081));
        let eps = discover(&snap, &Inventory::example());
        let ids: Vec<&str> = eps.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["cell1-vllm-8000", "gx10-0000-ollama-11434", "gx10-0000-llamacpp-8081"]);
    }

    #[test]
    fn a_container_that_isnt_listening_yet_is_loading() {
        let mut snap = cluster();
        snap.nodes[0].listeners.retain(|l| l.port != 8000);
        snap.nodes[0].containers[0].started_at = NOW - 60;
        let eps = assessed(&snap, &llama_track());
        assert_eq!(eps[0].health, Health::Loading);
    }

    fn deployment(state: DeploymentState) -> Deployment {
        Deployment {
            spec: DeploySpec {
                id: "m1a2b".into(),
                served_name: "qwen3.8-27b".into(),
                model_path: "/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8".into(),
                nodes: vec!["gx10-0000".into()],
                port: 8000,
                max_model_len: Some(32768),
                ..Default::default()
            },
            endpoint: "http://192.168.50.54:8000/v1".into(),
            state,
            stage: Some(Stage::Loading { percent: 40 }),
            message: "Loading weights 40%".into(),
            log_tail: String::new(),
            started_at: NOW - 30,
            provider_id: "vllm-m1a2b".into(),
            demo: true,
            generation: 0,
            ..Default::default()
        }
    }

    #[test]
    fn sparkbox_loads_are_merged_and_badged() {
        let found = discover(&cluster(), &Inventory::example());
        let mut merged = merge(found.clone(), &[deployment(DeploymentState::Starting)], &[], &Inventory::example());
        for e in &mut merged {
            assess(e, &Track::default(), &cluster(), NOW * 1000);
        }
        let mine = merged.iter().find(|e| e.deployment_id.as_deref() == Some("m1a2b")).unwrap();
        assert_eq!((mine.managed_by, mine.health, mine.loading_pct), (ManagedBy::Sparkbox, Health::Loading, Some(40)));
        assert_eq!(mine.id, "gx10-0000-vllm-8000");
        assert_eq!(mine.container_names, ["sparkbox-m1a2b"]);
        assert_eq!(merged[0].managed_by, ManagedBy::Sparkbox, "SparkBox's own loads sort first");

        // Once its container shows up in the snapshot, it's one endpoint, not two.
        let mut snap = cluster();
        let mut c = container("sparkbox-m1a2b", "vllm serve /home/sparkuser/models/Qwen/Qwen3.8-27B-FP8 --served-model-name qwen3.8-27b --port 8000", &[]);
        c.labels.insert("sparkbox.deployment".into(), "m1a2b".into());
        snap.nodes[3].containers.push(c);
        snap.nodes[3].listeners.push(listen(8000));
        let mut merged = merge(discover(&snap, &Inventory::example()), &[deployment(DeploymentState::Ready)], &[], &Inventory::example());
        for e in &mut merged {
            assess(e, &answered(NOW * 1000), &snap, NOW * 1000);
        }
        assert_eq!(merged.iter().filter(|e| e.deployment_id.is_some()).count(), 1);
        let mine = merged.iter().find(|e| e.deployment_id.is_some()).unwrap();
        assert_eq!((mine.health, mine.provider_id.as_deref()), (Health::Ready, Some("vllm-m1a2b")));
        assert_eq!(mine.container_ids, ["sparkbox-m1a2b-id"]);
        assert_eq!(in_use(&merged).get("Qwen/Qwen3.8-27B-FP8").map(String::as_str), Some("gx10-0000-vllm-8000"));

        // Stopped loads disappear; failed ones stay, down, with the reason.
        assert!(merge(Vec::new(), &[deployment(DeploymentState::Stopped)], &[], &Inventory::example()).is_empty());
        let mut failed = merge(Vec::new(), &[deployment(DeploymentState::Failed)], &[], &Inventory::example());
        assess(&mut failed[0], &Track::default(), &cluster(), NOW * 1000);
        assert_eq!((failed[0].health, failed[0].health_reason.as_deref()), (Health::Down, Some("Loading weights 40%")));

        // A failed load saved by an older SparkBox can hold a refusal as JSON: it reads as its words.
        let refused = crate::errors::SbError::new(crate::errors::ErrorKind::Fenced, "No fence is open, so changes on the cluster are refused.").to_json();
        let mut saved = Deployment { message: refused, ..deployment(DeploymentState::Failed) };
        saved.stage = None;
        let mut failed = merge(Vec::new(), &[saved], &[], &Inventory::example());
        assess(&mut failed[0], &Track::default(), &cluster(), NOW * 1000);
        assert_eq!(failed[0].health_reason.as_deref(), Some("No fence is open, so changes on the cluster are refused."));
    }

    #[test]
    fn the_ui_gets_the_field_names_it_expects() {
        let mut e = assessed(&cluster(), &llama_track()).remove(0);
        let mut ring = crate::models::metrics::Ring::default();
        ring.push(crate::models::metrics::Scrape { ts_ms: 0, ..Default::default() });
        e.metrics = ring.push(crate::models::metrics::Scrape { ts_ms: 3000, ..Default::default() }).clone().into();
        e.supervisor = Some(SupervisorView { enabled: true, state: SupervisorState::GaveUp, restarts: 5, next_retry_ms: None, held: None });
        let v = serde_json::to_value(&e).unwrap();
        for key in [
            "id", "engine", "models", "nodes", "headNode", "port", "url", "managedBy", "health", "healthReason", "parallel", "containerIds",
            "deploymentId", "metrics", "providerId", "maxContext", "loadingPct", "lastOkMs", "expectedNodes", "missingNodes", "restarts", "supervisor",
        ] {
            assert!(v.get(key).is_some(), "missing {key}: {v}");
        }
        assert_eq!((v["engine"].as_str(), v["managedBy"].as_str(), v["health"].as_str()), (Some("vllm"), Some("external"), Some("ready")));
        assert_eq!(v["parallel"], serde_json::json!({ "tp": 1, "pp": 3 }));
        assert_eq!(v["supervisor"], serde_json::json!({ "enabled": true, "state": "gave-up", "restarts": 5, "nextRetryMs": null }));
        assert_eq!(serde_json::to_value(Health::Stopped).unwrap(), "stopped");
        for key in ["genTps", "promptTps", "running", "waiting", "kvPct", "kvKnown", "ttftP50Ms", "ttftP95Ms", "ttftP50Range", "itlP50Ms", "history"] {
            assert!(v["metrics"].get(key).is_some(), "metrics.{key}");
        }
        for key in ["ts", "genTps", "running", "kvPct"] {
            assert!(v["metrics"]["history"][key].is_array(), "history.{key}");
        }
        for key in ["probeHost", "members", "listening", "deployment", "gone"] {
            assert!(v.get(key).is_none(), "{key} is internal only");
        }
    }

    #[test]
    fn hand_added_endpoints_join_unless_already_found() {
        let p = |id: &str, url: &str| Provider { id: id.into(), name: id.into(), base_url: url.into(), models: vec!["m".into()], ..Default::default() };
        let found = discover(&cluster(), &Inventory::example());
        let merged = merge(
            found,
            &[],
            &[p("lmstudio", "http://192.168.50.20:1234/v1"), p("dup", "http://192.168.50.51:8000/v1")],
            &Inventory::example(),
        );
        assert_eq!(merged.iter().filter(|e| e.source == Source::Manual).map(|e| e.id.as_str()).collect::<Vec<_>>(), ["lmstudio"]);
    }

    #[test]
    fn probes_fill_in_models() {
        let mut e = discover(&cluster(), &Inventory::example()).remove(1);
        let reply: Value = serde_json::json!({ "data": [{ "id": "qwen3:32b" }, { "id": "gpt-oss:20b" }] });
        let (models, max) = parse_models(&reply);
        apply_probe(&mut e, &Probe { ok: true, models, max_context: max, at_ms: 5, ..Default::default() });
        assess(&mut e, &answered(NOW * 1000), &cluster(), NOW * 1000);
        assert_eq!(e.models, ["qwen3:32b", "gpt-oss:20b"]);
        let p = provider_for(&e, false).unwrap();
        assert_eq!((p.id.as_str(), p.base_url.as_str()), ("gx10-0000-ollama-11434", "http://192.168.50.54:11434/v1"));
        let vllm: Value = serde_json::json!({ "data": [{ "id": "llama-3.3-70b", "max_model_len": 32768 }] });
        assert_eq!(parse_models(&vllm), (vec!["llama-3.3-70b".to_string()], Some(32768)));
    }

    #[test]
    fn an_operations_check_gets_a_test_token_even_while_live_traffic_proves_it() {
        let now = 1_800_000_000_000;
        let token = |after_round| Canary { at_ms: now - 5_000, ok: true, latency_ms: Some(80), error: None, after_round };
        let busy = Track { passive_ok_ms: Some(now - 1_000), canary_ok_ms: Some(now - 5_000), canary: Some(token(Some(4))), ..Default::default() };
        assert!(!canary_due(&busy, now), "live traffic is proof enough day to day");
        let checking = Track { verify_after: Some(7), ..busy.clone() };
        assert!(canary_due(&checking, now), "a restart's check wants a token sent after it");
        assert!(canary_due(&Track { canary: Some(token(Some(7))), ..checking.clone() }, now), "one sent in the round that was running when it began doesn't count");
        assert!(!canary_due(&Track { canary: Some(token(Some(8))), ..checking.clone() }, now), "one sent after a later round: done");
        assert!(!canary_due(&Track { canary_running: true, ..checking.clone() }, now), "one at a time");
        let failed = Canary { ok: false, at_ms: now - 2_000, error: Some("HTTP 500".into()), ..token(Some(8)) };
        assert!(!canary_due(&Track { canary: Some(failed), ..checking }, now), "a failed one is retried at the usual pace");
    }

    /// Docker's Command for a container: its entrypoint, then its arguments, each one with a space
    /// in it wrapped in single quotes (moby's list.go), as the collector reads it from /containers/json.
    fn docker_command(entrypoint: &str, args: &[String]) -> String {
        let args: Vec<String> = args.iter().map(|a| if a.contains(' ') { format!("'{a}'") } else { a.clone() }).collect();
        format!("{entrypoint} {}", args.join(" "))
    }

    /// A load from before the launcher can still be running when SparkBox updates: it has no
    /// sparkbox.path label, and is found by its Command as ever.
    #[test]
    fn a_load_from_before_the_launcher_is_found_by_its_command_with_its_image_from_the_label() {
        let image_id = format!("sha256:{}", "a".repeat(64));
        let image_ref = "vllm/vllm-openai:glm53-flash-arm64-cu130";
        let args: Vec<String> = ["serve", "/home/sparkuser/models/google/gemma-3-27b-it", "--served-model-name", "gemma-3-27b-it", "--gpu-memory-utilization", "0.75", "--host", "192.168.50.154", "--port", "8000", "--max-model-len", "32768"].map(String::from).to_vec();
        // With a key: `--entrypoint sh <id> -c '<script>'`, the script one argument.
        let quoted: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
        let script = format!("VLLM_API_KEY=\"$(cat /run/sparkbox/key)\" exec vllm {}", quoted.join(" "));
        let keyed = docker_command("sh", &["-c".into(), script]);
        // Without a key: `--entrypoint vllm <id> 'serve' '<folder>' …`, each argument plain.
        let open = docker_command("vllm", &args);
        for (cmd, labelled) in [(keyed, true), (open, false)] {
            let mut snap = cluster();
            let mut c = container("sparkbox-d1", &cmd, &[]);
            c.image = image_id.clone();
            c.labels = BTreeMap::from([("sparkbox.deployment".to_string(), "d1".to_string()), ("sparkbox.instance".to_string(), "release".to_string())]);
            if labelled {
                c.labels.insert("sparkbox.image".into(), image_ref.into());
            }
            snap.nodes[1].containers = vec![c];
            snap.nodes[1].listeners = vec![Listener { port: 8000, addr: "192.168.50.154".into(), ..Default::default() }];
            let eps = discover(&snap, &Inventory::example());
            let e = eps.iter().find(|e| e.head_node == "cell2").unwrap_or_else(|| panic!("not found from {cmd}"));
            assert_eq!((e.id.as_str(), e.models.as_slice(), e.port), ("cell2-vllm-8000", ["gemma-3-27b-it".to_string()].as_slice(), 8000), "{cmd}");
            assert_eq!((e.probe_host.as_str(), e.url.as_str()), ("192.168.50.154", "http://192.168.50.154:8000/v1"));
            assert_eq!((e.managed_by, e.deployment_id.as_deref(), e.max_context), (ManagedBy::Sparkbox, Some("d1"), Some(32768)));
            assert_eq!(e.model_path.as_deref(), Some("/home/sparkuser/models/google/gemma-3-27b-it"));
            // Started by its id: the name it was picked by comes from the label, the id without one.
            let want = if labelled { image_ref } else { image_id.as_str() };
            assert_eq!(e.image.as_deref(), Some(want));
        }
    }

    #[test]
    fn a_live_sparkbox_load_is_loading_until_its_readiness_passes_and_a_demo_one_isnt_held() {
        use crate::models::readiness::Readiness;
        let mut d = deployment(DeploymentState::Ready);
        d.demo = false;
        d.stage = Some(Stage::Ready);
        let mut snap = cluster();
        snap.nodes[3].containers.clear();
        snap.nodes[3].listeners = vec![listen(8000)];
        let eps = |d: &Deployment| {
            let mut list = merge(discover(&snap, &Inventory::example()), std::slice::from_ref(d), &[], &Inventory::example());
            let e = list.iter_mut().find(|e| e.deployment_id.as_deref() == Some("m1a2b")).unwrap();
            assess(e, &answered(NOW * 1000 - 5_000), &snap, NOW * 1000);
            e.clone()
        };
        // Answering, with a test token: Ready for anything else, not for a load whose check hasn't passed.
        d.readiness = Some(Readiness { health: true, ..Default::default() });
        let e = eps(&d);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Loading, Some("Up; checking it answers…")));
        d.readiness = Some(Readiness { health: true, why: Some("It answers /health, but /v1/models doesn't list qwen3.8-27b.".into()), ..Default::default() });
        assert_eq!(eps(&d).health_reason.as_deref(), Some("Up; checking it answers… (It answers /health, but /v1/models doesn't list qwen3.8-27b.)"));
        d.readiness = Some(Readiness { health: true, models_listed: true, token: Some(true), ok: true, ..Default::default() });
        let e = eps(&d);
        assert_eq!(e.health, Health::Ready, "{:?}", e.health_reason);
        assert!(e.readiness.as_ref().is_some_and(|r| r.ok));
        assert_eq!(e.health_note, None);
        // Demo loads have no readiness check: the simulator answers for them.
        let demo = Deployment { demo: true, readiness: None, ..d.clone() };
        let e = eps(&demo);
        assert_eq!((e.health, e.health_note), (Health::Ready, None));
        // Ready under an older SparkBox, which saved no check: nothing would ask it now, so it's
        // Ready as any server is (it answers and generates), and the card says 17+25 wasn't asked.
        let older = Deployment { readiness: None, ..d };
        let e = eps(&older);
        assert_eq!((e.health, e.health_note.as_deref()), (Health::Ready, Some(BEFORE_READINESS)));
        assert_eq!(e.readiness, None);
        // …and only once it answers and generates, as any server.
        let mut list = merge(discover(&snap, &Inventory::example()), std::slice::from_ref(&older), &[], &Inventory::example());
        let e = list.iter_mut().find(|e| e.deployment_id.as_deref() == Some("m1a2b")).unwrap();
        assess(e, &Track { last_ok_ms: Some(NOW * 1000 - 5_000), ..Default::default() }, &snap, NOW * 1000);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Loading, Some("Checking that it generates…")));
    }

    #[test]
    fn a_load_held_by_look_only_waits_however_long_it_takes() {
        use crate::models::readiness::{Readiness, HELD};
        let mut d = deployment(DeploymentState::Starting);
        d.demo = false;
        d.stage = Some(Stage::Ready);
        d.message = format!("Up; checking it answers… ({HELD})");
        d.readiness = Some(Readiness { health: true, models_listed: true, why: Some(HELD.into()), ..Default::default() });
        d.started_at = NOW - 25 * 60;
        let mut snap = cluster();
        snap.nodes[3].containers.clear();
        snap.nodes[3].listeners = vec![listen(8000)];
        // Its stage became Ready 21 minutes ago; look-only has held the check since.
        let stuck = Track { progress_ms: Some(NOW * 1000 - 21 * 60_000), progress_key: format!("{:?}", Stage::Ready), ..answered(NOW * 1000 - 5_000) };
        let eps = |d: &Deployment, t: &Track| {
            let mut list = merge(discover(&snap, &Inventory::example()), std::slice::from_ref(d), &[], &Inventory::example());
            let e = list.iter_mut().find(|e| e.deployment_id.as_deref() == Some("m1a2b")).unwrap();
            assess(e, t, &snap, NOW * 1000);
            (e.health, e.health_reason.clone().unwrap_or_default())
        };
        assert_eq!(eps(&d, &stuck), (Health::Loading, format!("Up; checking it answers… ({HELD})")));
        // Held test tokens on the endpoint count as waiting too.
        let other = Deployment { readiness: Some(Readiness { health: true, why: Some("It doesn't answer /health yet (HTTP 503).".into()), ..Default::default() }), message: "Up; checking it answers…".into(), ..d.clone() };
        assert_eq!(eps(&other, &Track { canary_held: true, ..stuck.clone() }).0, Health::Loading);
        // Anything else that keeps it from Ready for 20 minutes is quiet, and the card says so.
        assert_eq!(eps(&other, &stuck), (Health::Loading, "Up; checking it answers… · no new progress for 21 min (limit 90 min)".to_string()));
    }

    /// A first start can be quiet for a long time, and Down there restarts a load that's working
    /// (Keep running) and pages. A load of SparkBox's own stays Loading, saying how long it has
    /// been quiet, until `Models::follow` fails it at the limit; nothing else gets that patience.
    #[test]
    fn a_starting_load_that_goes_quiet_stays_loading_up_to_its_limit() {
        use crate::models::supervisor::{restartable, Action, Supervisor};
        let (gate, _, _) = crate::admin::testing::gate();
        let mut starting = deployment(DeploymentState::Starting);
        starting.demo = false;
        starting.started_at = NOW - 95 * 60;
        let quiet = |min: u64| Track { pct: Some(40), progress_ms: Some(NOW * 1000 - min * 60_000), ..Track::default() };
        let assessed = |d: &Deployment, t: &Track| {
            let mut list = merge(Vec::new(), std::slice::from_ref(d), &[], &Inventory::example());
            assess(&mut list[0], t, &cluster(), NOW * 1000);
            list.remove(0)
        };
        for min in [21, 89] {
            let e = assessed(&starting, &quiet(min));
            assert_eq!((e.health, e.loading_pct), (Health::Loading, Some(40)), "{min} min quiet");
            assert_eq!(e.health_reason, Some(format!("Loading weights 40% · no new progress for {min} min (limit 90 min)")));
            assert_eq!(e.message, e.health_reason, "older UI reads this");
            // No alert: the endpoint-down rule reads Down and nothing else.
            let snap = cluster();
            let mut engine = crate::health::Engine::default();
            for s in 0..8u64 {
                let at = NOW * 1000 + s * 30_000;
                let events = engine.step_with(&snap, &crate::health::endpoint_infos(std::slice::from_ref(&e)), at);
                assert!(!events.iter().any(|ev| ev.alert.rule == "endpoint-down"), "{min} min quiet: {events:?}");
            }
            // Keep running leaves it alone: nothing is restartable, or scheduled, at 20 minutes.
            assert!(!restartable(&e, |_| true), "{min} min quiet");
            let mut s = Supervisor::default();
            assert_eq!(s.tick(e.health, restartable(&e, |_| true), gate.hold_view(), NOW * 1000), Action::Wait);
            assert_eq!(s.next_retry_ms, None);
        }
        // Moving again: the line goes.
        let e = assessed(&starting, &quiet(19));
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Loading, Some("Loading weights 40%")));

        // Past 20 minutes, a load that isn't starting any more and doesn't listen is stuck, as is
        // a server SparkBox didn't start (recovery_goes_loading_with_progress_then_ready).
        let mut ready = deployment(DeploymentState::Ready);
        ready.started_at = NOW - 95 * 60;
        let mut list = merge(Vec::new(), &[ready], &[], &Inventory::example());
        list[0].listening = false;
        list[0].members = vec![Member { node: "gx10-0000".into(), head: true, name: "sparkbox-m1a2b".into(), state: "running".into(), started_at: NOW - 3600, ..Default::default() }];
        assess(&mut list[0], &quiet(21), &cluster(), NOW * 1000);
        assert_eq!((list[0].health, list[0].health_reason.as_deref()), (Health::Down, Some("Didn't become ready in 20 min")));
        assert!(restartable(&list[0], |_| true));

        // Failed at the limit: Down with the limit's words, as the load's own state says.
        let failed = Deployment { message: "vLLM didn't become ready within 90 minutes.".into(), stage: None, ..deployment(DeploymentState::Failed) };
        let e = assessed(&failed, &quiet(89));
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Down, Some("vLLM didn't become ready within 90 minutes.")));
        // Down, but not Keep running's: a restart would spend another 90 minutes on the same end.
        assert!(!restartable(&e, |_| true));
        let mut s = Supervisor::default();
        assert_eq!(s.tick(e.health, restartable(&e, |_| true), gate.hold_view(), NOW * 1000), Action::Wait);
        assert_eq!(s.next_retry_ms, None);

        // Listening, its probes can read Down while it starts: that isn't Keep running's either.
        let failing = Track { fails: PROBE_FAILS_DOWN, error: Some("no answer".into()), ..Track::default() };
        let mut list = merge(Vec::new(), std::slice::from_ref(&starting), &[], &Inventory::example());
        list[0].listening = true;
        assess(&mut list[0], &failing, &cluster(), NOW * 1000);
        assert_eq!(list[0].health, Health::Down);
        assert!(!restartable(&list[0], |_| true));
        // Not listening yet, it isn't asked: failures from before this start (what Keep running
        // restarted it for) say nothing about it, and verify would read them as a failed restart.
        assert_eq!(assessed(&starting, &failing).health, Health::Loading);

        // Failed after it ran (the crash `follow` saw): this is what Keep running is for.
        let crashed = Deployment { recoverable: true, ..failed };
        assert!(restartable(&assessed(&crashed, &Track::default()), |_| true));
    }

    // ---- A load started through launch.py: its Command says nothing, its labels say it all

    /// The containers of a load from C-L2's labels, as the collector reports them, by rank.
    fn c_l2(load: &str) -> Vec<(String, Container)> {
        let all: Value = serde_json::from_str(include_str!("fixtures/loads-c-l2.json")).unwrap();
        all[load].as_array().unwrap().iter().map(|r| (r["node"].as_str().unwrap().to_string(), serde_json::from_value(r["container"].clone()).unwrap())).collect()
    }

    /// The cluster with only `load` on it, its head listening.
    fn loaded(load: &str) -> ClusterSnapshot {
        let mut snap = cluster();
        for n in &mut snap.nodes {
            n.containers.clear();
            n.listeners.clear();
        }
        for (node, c) in c_l2(load) {
            let n = snap.nodes.iter_mut().find(|n| n.id == node).unwrap();
            if let Some(addr) = c.labels.get("sparkbox.listen") {
                n.listeners.push(Listener { port: 8000, addr: addr.clone(), ..Default::default() });
            }
            n.containers.push(c);
        }
        snap
    }

    fn on<'a>(snap: &'a mut ClusterSnapshot, node: &str) -> &'a mut NodeSnapshot {
        snap.nodes.iter_mut().find(|n| n.id == node).unwrap()
    }

    /// The spec behind each of the fixture's loads.
    fn c_l2_spec(load: &str) -> DeploySpec {
        let (id, nodes, tp, host, listen, require_key, max_model_len) = match load {
            "one" => ("m1one", vec!["cell2"], 1, "192.168.50.52", "lan", true, Some(32768)),
            "tp2" => ("m2tp2", vec!["cell1", "cell3"], 2, "192.168.50.51", "lan", true, None),
            _ => ("m4tp4", vec!["cell3", "cell1", "gx10-0000", "cell2"], 4, "", "all", false, Some(32768)),
        };
        DeploySpec {
            id: id.into(),
            served_name: "gemma-3-27b-it".into(),
            model_path: "/home/sparkuser/models/google/gemma-3-27b-it".into(),
            nodes: nodes.iter().map(|n| n.to_string()).collect(),
            tensor_parallel: tp,
            pipeline_parallel: 1,
            max_model_len,
            gpu_memory_utilization: 0.75,
            port: 8000,
            host: host.into(),
            listen: listen.into(),
            require_key,
            image_ref: "vllm/vllm-openai:glm53-flash-arm64-cu130".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            recipe: "gemma-3-27b-it".into(),
            docker_access: "privileged".into(),
            ..Default::default()
        }
    }

    /// What's read here is what plan.rs writes: every rank's name, labels and command.
    #[test]
    fn the_fixture_is_what_the_launch_writes() {
        for load in ["one", "tp2", "tp4"] {
            let spec = c_l2_spec(load);
            for (rank, (_, c)) in c_l2(load).iter().enumerate() {
                let run = crate::models::plan::rank_command_for(&spec, rank, "dev");
                let labels: BTreeMap<String, String> = run.split(" --label ").skip(1).filter_map(|l| l.split_whitespace().next()?.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
                assert_eq!(labels, c.labels, "{load} rank {rank}: {run}");
                assert!(run.contains(&format!(" --name {} ", c.name)), "{run}");
                let launched = run.rsplit_once(&format!(" {} ", c.image)).map(|(_, rest)| rest).unwrap_or_default();
                assert_eq!(format!("python3 {launched}"), c.command, "Docker's Command: its entrypoint, then its arguments: {run}");
            }
        }
    }

    #[test]
    fn a_load_is_found_from_its_labels_with_every_rank() {
        let image = "vllm/vllm-openai:glm53-flash-arm64-cu130";
        let cases = [
            ("one", vec!["cell2"], None, Some(32768), "lan"),
            ("tp2", vec!["cell1", "cell3"], Some(Parallel { tp: 2, pp: 1 }), None, "lan"),
            // By rank: the fixture's are out of the cluster's order, and the head is cell3.
            ("tp4", vec!["cell3", "cell1", "gx10-0000", "cell2"], Some(Parallel { tp: 4, pp: 1 }), Some(32768), "all"),
        ];
        for (load, nodes, parallel, max_context, listen) in cases {
            let snap = loaded(load);
            assert!(snap.nodes.iter().flat_map(|n| &n.containers).all(|c| parse_command(&c.command).is_none()), "{load}: its Command alone finds nothing");
            let eps = assessed(&snap, &BTreeMap::new());
            assert_eq!(eps.len(), 1, "{load}: one endpoint, however many ranks: {eps:?}");
            let e = &eps[0];
            assert_eq!((e.id.as_str(), e.engine, e.port), (format!("{}-vllm-8000", nodes[0]).as_str(), Engine::Vllm, 8000), "{load}");
            assert_eq!((e.health, e.health_reason.as_deref()), (Health::Ready, None), "{load}");
            assert_eq!(e.nodes, nodes, "{load}: in rank order");
            assert_eq!(e.expected_nodes, nodes, "{load}");
            assert!(e.missing_nodes.is_empty(), "{load}");
            assert_eq!(e.members.iter().map(|m| m.node.as_str()).collect::<Vec<_>>(), nodes, "{load}");
            let ranks: Vec<Option<u32>> = (0..nodes.len() as u32).map(Some).collect();
            assert_eq!(e.members.iter().map(|m| m.stage).collect::<Vec<_>>(), ranks, "{load}: ranks from the label");
            assert_eq!(e.members.iter().filter(|m| m.head).count(), 1, "{load}");
            assert_eq!((e.parallel, e.max_context, e.listen.as_str()), (parallel, max_context, listen), "{load}");
            assert_eq!(e.models, ["gemma-3-27b-it"], "{load}");
            assert_eq!(e.model_path.as_deref(), Some("/home/sparkuser/models/google/gemma-3-27b-it"), "{load}");
            assert_eq!(e.image.as_deref(), Some(image), "{load}: the name it was picked by, not its id");
            assert_eq!(e.managed_by, ManagedBy::Sparkbox, "{load}");
            assert_eq!(e.deployment_id.as_deref(), Some(c_l2_spec(load).id.as_str()), "{load}: what Unload goes by");
            assert_eq!(e.container_names, [c_l2(load)[0].1.name.clone()], "{load}");
            assert_eq!(provider_for(e, false).map(|p| p.models), Some(vec!["gemma-3-27b-it".to_string()]), "{load}: Chat offers it");
            let head = snap.nodes.iter().find(|n| n.id == nodes[0]).unwrap();
            assert_eq!(e.url, format!("http://{}:8000/v1", head.nets[0].ip.as_deref().unwrap()), "{load}");
        }
    }

    #[test]
    fn only_ranks_serve_from_labels_and_other_peoples_containers_stay_on_their_command_lines() {
        let (head, worker) = (c_l2("tp2")[0].1.clone(), c_l2("tp2")[1].1.clone());
        assert_eq!(parse_container(&head).map(|p| (p.engine, p.tp, p.pp, p.port)), Some((Engine::Vllm, 2, 1, Some(8000))));
        assert!(parse_container(&worker).is_none(), "a worker serves no API");
        // A load's labels from before the launcher (C-L): no path, so its Command still says.
        let mut older = container("sparkbox-old", "vllm serve /m/old --served-model-name old --port 8001", &[]);
        older.labels = BTreeMap::from([("sparkbox.deployment".to_string(), "old".to_string()), ("sparkbox.role".to_string(), "worker".to_string())]);
        assert_eq!(parse_container(&older).map(|p| p.port), Some(Some(8001)), "only a load with sparkbox.path is read from its labels");
        // Ray workers still attach by their --address, beside a labelled load on the same cluster.
        let mut snap = cluster();
        on(&mut snap, "gx10-0000").containers = vec![c_l2("tp2")[1].1.clone()];
        let e = assessed(&snap, &llama_track()).into_iter().find(|e| e.id == "cell1-vllm-8000").unwrap();
        assert_eq!(e.nodes, ["cell1", "cell2", "cell3"], "someone else's pipeline keeps its workers");
        assert_eq!(e.health, Health::Ready, "{:?}", e.health_reason);
    }

    /// The sample can see a worker's process before its container is listed: that isn't a server.
    #[test]
    fn a_headless_rank_is_no_server_even_as_a_bare_process() {
        let argv = "vllm serve /home/sparkuser/models/google/gemma-3-27b-it --served-model-name gemma-3-27b-it --tensor-parallel-size 2 --nnodes 2 --node-rank 1 --master-addr 192.168.100.1 --master-port 29501";
        assert!(parse_command(argv).is_some(), "the same command, serving");
        let headless = format!("{argv} --headless");
        assert!(parse_command(&headless).is_none());
        let mut snap = loaded("tp2");
        on(&mut snap, "cell3").containers.clear();
        on(&mut snap, "cell3").processes = vec![Process { pid: 1, cmd: headless, container: None, ..Default::default() }];
        let eps = discover(&snap, &Inventory::example());
        assert_eq!(eps.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["cell1-vllm-8000"], "no phantom on cell3: {eps:?}");
    }

    #[test]
    fn a_worker_that_stops_reads_down_and_names_its_rank() {
        // Rank 2 of the four, on gx10-0000.
        let mut snap = loaded("tp4");
        let w = &mut on(&mut snap, "gx10-0000").containers[0];
        w.state = "exited".into();
        w.exit_code = 1;
        w.finished_at = NOW - 130;
        let e = assessed(&snap, &BTreeMap::new()).remove(0);
        assert_eq!(e.health, Health::Down);
        assert_eq!(e.health_reason.as_deref(), Some("Worker on gx10-0000 isn't running (exit 1, 2 min ago) · tensor-parallel rank 3 of 4"));
        assert_eq!(e.missing_nodes, ["gx10-0000"]);
        assert_eq!(e.nodes, ["cell3", "cell1", "gx10-0000", "cell2"], "it keeps its place");

        // Removed altogether, on the first look at this load: the ranks left say which is gone.
        on(&mut snap, "gx10-0000").containers.clear();
        let e = assessed(&snap, &BTreeMap::new()).remove(0);
        assert_eq!(e.health_reason.as_deref(), Some("Needs 4 nodes but only 3 are running: tensor-parallel rank 3 of 4 is missing"));
        assert_eq!(e.nodes, ["cell3", "cell1", "cell2"]);

        // Removed, with what it ran on remembered: the node too, and it keeps its number.
        let t = Track { members: vec!["cell3".into(), "cell1".into(), "gx10-0000".into(), "cell2".into()], ..answered(NOW * 1000 - 5_000) };
        let e = assessed(&snap, &BTreeMap::from([("cell3-vllm-8000".to_string(), t)])).remove(0);
        assert_eq!(e.health_reason.as_deref(), Some("Tensor-parallel rank 3 of 4 on gx10-0000 has no worker running"));
        assert_eq!(e.nodes, ["cell3", "cell1", "gx10-0000", "cell2"]);
        assert_eq!(e.missing_nodes, ["gx10-0000"]);

        // Its node offline: the same rank.
        on(&mut snap, "gx10-0000").status = LinkStatus::Offline;
        let t = Track { members: vec!["cell3".into(), "cell1".into(), "gx10-0000".into(), "cell2".into()], ..answered(NOW * 1000 - 5_000) };
        let e = assessed(&snap, &BTreeMap::from([("cell3-vllm-8000".to_string(), t)])).remove(0);
        assert_eq!(e.health_reason.as_deref(), Some("Tensor-parallel rank 3 of 4 on gx10-0000 is offline"));
    }

    /// In native mode a head exits when it loses a rank, so the worker that went first is named,
    /// and only one that really did stop during this run of the head, no later than the head.
    #[test]
    fn a_worker_that_takes_the_head_down_is_named() {
        let stopped = |snap: &mut ClusterSnapshot, node: &str, exit: i32, secs_ago: u64| {
            let c = &mut on(snap, node).containers[0];
            c.state = "exited".into();
            c.exit_code = exit;
            c.finished_at = NOW - secs_ago;
        };
        // Rank 2 of the four, on gx10-0000, five seconds before the head on cell3.
        let down = || {
            let mut snap = loaded("tp4");
            on(&mut snap, "cell3").listeners.clear();
            stopped(&mut snap, "cell3", 1, 125);
            snap
        };
        let read = |snap: &ClusterSnapshot| assessed(snap, &BTreeMap::new()).remove(0);
        let mut snap = down();
        stopped(&mut snap, "gx10-0000", 1, 130);
        let e = read(&snap);
        assert_eq!(e.health, Health::Down);
        assert_eq!(e.health_reason.as_deref(), Some("Head exited (code 1) 2 min ago · worker on gx10-0000 stopped first (exit 1) · tensor-parallel rank 3 of 4"));
        assert_eq!(e.missing_nodes, ["gx10-0000"]);
        assert_eq!(e.nodes, ["cell3", "cell1", "gx10-0000", "cell2"], "it keeps its place");

        // What the head's log said comes after it.
        let t = Track { last_log: Some("RuntimeError: Engine core initialization failed.".into()), ..answered(NOW * 1000 - 5_000) };
        let e = assessed(&snap, &BTreeMap::from([("cell3-vllm-8000".to_string(), t)])).remove(0);
        assert_eq!(e.health_reason.as_deref(), Some("Head exited (code 1) 2 min ago · worker on gx10-0000 stopped first (exit 1) · tensor-parallel rank 3 of 4 · RuntimeError: Engine core initialization failed."));

        // Of two, the first, however its exit reads.
        stopped(&mut snap, "cell1", 137, 140);
        let e = read(&snap);
        assert_eq!(e.health_reason.as_deref(), Some("Head exited (code 1) 2 min ago · worker on cell1 stopped first (killed, exit 137) · tensor-parallel rank 2 of 4"));
        assert_eq!(e.missing_nodes, ["cell1"]);

        // One Docker is restarting went too.
        let mut snap = down();
        stopped(&mut snap, "gx10-0000", 1, 130);
        on(&mut snap, "gx10-0000").containers[0].state = "restarting".into();
        assert_eq!(read(&snap).missing_nodes, ["gx10-0000"]);

        // Docker's clock here is in seconds: in the same second, nobody can say which went first.
        let mut snap = down();
        stopped(&mut snap, "gx10-0000", 1, 125);
        let e = read(&snap);
        assert_eq!(e.health_reason.as_deref(), Some("Head exited (code 1) 2 min ago · worker on gx10-0000 stopped with it (exit 1) · tensor-parallel rank 3 of 4"));
        assert_eq!(e.missing_nodes, ["gx10-0000"]);

        // Not the cause: it stopped after the head, or before this run of the head began, or never ran.
        let plain = "Head exited (code 1) 2 min ago";
        let mut after = down();
        stopped(&mut after, "gx10-0000", 1, 120);
        let mut earlier = down();
        stopped(&mut earlier, "gx10-0000", 1, 4000);
        let mut never = down();
        on(&mut never, "gx10-0000").containers[0].state = "created".into();
        for snap in [after, earlier, never] {
            let e = read(&snap);
            assert_eq!((e.health_reason.as_deref(), e.missing_nodes.as_slice()), (Some(plain), &[] as &[String]));
        }

        // A head that left cleanly after a worker crashed wasn't stopped by anyone…
        let mut snap = down();
        stopped(&mut snap, "gx10-0000", 1, 130);
        stopped(&mut snap, "cell3", 0, 125);
        let e = read(&snap);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Down, Some("Head exited (code 0) 2 min ago · worker on gx10-0000 stopped first (exit 1) · tensor-parallel rank 3 of 4")));
        assert_eq!(e.missing_nodes, ["gx10-0000"]);
        // …and a load stopped by hand, worker first, reads as stopped.
        let mut snap = down();
        stopped(&mut snap, "gx10-0000", 143, 130);
        stopped(&mut snap, "cell3", 143, 125);
        let e = read(&snap);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Stopped, Some("Stopped 2 min ago")));

        // A head out of memory is its own failure.
        let mut snap = down();
        stopped(&mut snap, "gx10-0000", 1, 130);
        stopped(&mut snap, "cell3", 137, 125);
        on(&mut snap, "cell3").containers[0].oom_killed = true;
        assert_eq!(read(&snap).health_reason.as_deref(), Some("Head ran out of memory (exit 137) 2 min ago"));

        // A pipeline's words are its own.
        let mut snap = cluster();
        snap.nodes[0].listeners.retain(|l| l.port != 8000);
        stopped(&mut snap, "cell1", 1, 190);
        stopped(&mut snap, "cell3", 1, 200);
        let v = &assessed(&snap, &llama_track())[0];
        assert_eq!(v.health_reason.as_deref(), Some("Head exited (code 1) 3 min ago · worker on cell3 stopped first (exit 1) · pipeline stage 3 of 3"));
    }

    /// The same ranks with no script and no labels: each rank's own command line. Those above 0
    /// serve no API and meet their head at --master-addr, which is what joins them to it.
    #[test]
    fn a_native_load_started_by_hand_is_one_endpoint() {
        let argv = |rank: u32, master: &str| {
            let headless = if rank > 0 { " --headless" } else { "" };
            format!("vllm serve /home/sparkuser/models/google/gemma-3-27b-it --served-model-name gemma-3-27b-it --port 8000 --tensor-parallel-size 4 --nnodes 4 --node-rank {rank} --master-addr {master} --master-port 29501{headless}")
        };
        let mut snap = loaded("tp4");
        for (rank, node) in ["cell1", "cell2", "cell3", "gx10-0000"].into_iter().enumerate() {
            let n = on(&mut snap, node);
            n.listeners.clear();
            n.containers = vec![container(&format!("glm53-{node}"), &argv(rank as u32, "192.168.100.1"), &[])];
        }
        on(&mut snap, "cell1").listeners = vec![listen(8000)];
        let eps = assessed(&snap, &BTreeMap::new());
        assert_eq!(eps.len(), 1, "{eps:?}");
        let e = &eps[0];
        assert_eq!((e.id.as_str(), e.health, e.health_reason.as_deref()), ("cell1-vllm-8000", Health::Ready, None));
        assert_eq!(e.nodes, ["cell1", "cell2", "cell3", "gx10-0000"]);
        assert_eq!(e.members.iter().map(|m| m.stage).collect::<Vec<_>>(), [Some(0), Some(1), Some(2), Some(3)], "by --node-rank");
        assert!(e.missing_nodes.is_empty());
        assert_eq!(e.managed_by, ManagedBy::External);

        // A rank of it that stops reads as any worker does.
        let mut gone = snap.clone();
        let w = &mut on(&mut gone, "gx10-0000").containers[0];
        w.state = "exited".into();
        w.exit_code = 1;
        w.finished_at = NOW - 130;
        let e = assessed(&gone, &BTreeMap::new()).remove(0);
        assert_eq!(e.health_reason.as_deref(), Some("Worker on gx10-0000 isn't running (exit 1, 2 min ago) · tensor-parallel rank 4 of 4"));

        // A rank that meets another head isn't this one's.
        on(&mut snap, "cell2").containers[0].command = argv(1, "192.168.100.7");
        let e = assessed(&snap, &BTreeMap::new()).remove(0);
        assert_eq!(e.nodes, ["cell1", "cell3", "gx10-0000"]);
        // Nor is something else's --headless.
        on(&mut snap, "cell2").containers[0].command = "chrome --headless --remote-debugging-port=9222".into();
        assert_eq!(assessed(&snap, &BTreeMap::new()).remove(0).nodes, ["cell1", "cell3", "gx10-0000"]);
    }

    #[test]
    fn a_head_that_exited_is_down_and_a_load_with_its_record_is_one_endpoint() {
        let mut snap = loaded("tp2");
        on(&mut snap, "cell1").listeners.clear();
        let h = &mut on(&mut snap, "cell1").containers[0];
        h.state = "exited".into();
        h.exit_code = 78;
        h.finished_at = NOW - 70;
        let e = assessed(&snap, &BTreeMap::new()).remove(0);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Down, Some("Head exited (code 78) 1 min ago")));

        // With its record: one endpoint, its own node list and name, Unload by its id.
        let snap = loaded("tp2");
        let spec = c_l2_spec("tp2");
        let d = Deployment { spec: spec.clone(), state: DeploymentState::Ready, endpoint: "http://192.168.50.51:8000/v1".into(), provider_id: "vllm-m2tp2".into(), ..Default::default() };
        let mut list = merge(discover(&snap, &Inventory::example()), std::slice::from_ref(&d), &[], &Inventory::example());
        assert_eq!(list.len(), 1, "{list:?}");
        assess(&mut list[0], &answered(NOW * 1000 - 5_000), &snap, NOW * 1000);
        assert_eq!((list[0].deployment_id.as_deref(), list[0].source), (Some("m2tp2"), Source::Deployment));
        assert_eq!(list[0].members.len(), 2);
        assert_eq!(list[0].container_names, ["sparkbox-m2tp2-r0"]);
        // Not in the snapshot yet: the name it will have, by rank and not by node.
        let list = merge(Vec::new(), std::slice::from_ref(&d), &[], &Inventory::example());
        assert_eq!(list[0].container_names, ["sparkbox-m2tp2-r0"]);
        let demo = Deployment { demo: true, ..d };
        let name = merge(Vec::new(), std::slice::from_ref(&demo), &[], &Inventory::example()).remove(0).container_names.remove(0);
        assert_eq!(name, crate::models::plan::container_name("m2tp2", "cell1", "head"), "Demo keeps its names");
    }

    // ---- Stages, and how long a load may stay quiet

    fn compiling(what: &str) -> Stage {
        Stage::Compiling { what: what.into() }
    }

    #[test]
    fn stages_read_in_a_few_words() {
        assert_eq!(stage_text(Some(&compiling("FlashInfer kernels")), ""), "Compiling: FlashInfer kernels");
        assert_eq!(stage_text(Some(&Stage::Joining { have: 2, need: 4 }), ""), "2 of 4 nodes up");
        assert_eq!(stage_text(Some(&Stage::Loading { percent: 7 }), ""), "Loading weights 7%");
        assert_eq!(stage_text(Some(&Stage::Warming), ""), "Capturing CUDA graphs");
        assert_eq!(stage_text(None, "Restarting…"), "Restarting…");
    }

    /// A long first compile on a Spark is the point of the 90 minutes: it stays Loading, says what
    /// it's doing, raises nothing, and Keep running leaves it alone.
    #[test]
    fn a_load_89_minutes_in_stays_loading() {
        use crate::models::supervisor::restartable;
        let mut d = deployment(DeploymentState::Starting);
        d.demo = false;
        d.stage = Some(compiling("FlashInfer kernels"));
        d.message = "Compiling: FlashInfer kernels".into();
        d.started_at = NOW - 89 * 60;
        let mut list = merge(Vec::new(), std::slice::from_ref(&d), &[], &Inventory::example());
        let t = Track { stage: d.stage.clone(), progress_ms: Some(NOW * 1000 - 88 * 60_000), ..Track::default() };
        let mut snap = cluster();
        snap.ts_ms = NOW * 1000;
        assess(&mut list[0], &t, &snap, NOW * 1000);
        let e = &list[0];
        assert_eq!(e.health, Health::Loading);
        assert_eq!(e.health_reason.as_deref(), Some("Compiling: FlashInfer kernels · no new progress for 88 min (limit 90 min)"));
        assert!(e.compiling && !restartable(e, |_| true));
        let mut engine = crate::health::Engine::default();
        for s in 0..8u64 {
            let at = NOW * 1000 + s * 30_000;
            let events = engine.step_with(&snap, &crate::health::endpoint_infos(std::slice::from_ref(e)), at);
            assert!(!events.iter().any(|ev| ev.alert.rule == "endpoint-down"), "{events:?}");
        }
        // Moving: the same words, with no line about it.
        let t = Track { progress_ms: Some(NOW * 1000 - 5 * 60_000), ..t };
        assess(&mut list[0], &t, &snap, NOW * 1000);
        assert_eq!((list[0].health, list[0].health_reason.as_deref()), (Health::Loading, Some("Compiling: FlashInfer kernels")));
    }

    /// A load of SparkBox's own that isn't Starting can compile too: its container was restarted
    /// by hand, say. Its log is what tells, and a compile gets a start's time, not 20 minutes.
    /// What it logs must still have moved: one that stays silent past that has hung.
    #[test]
    fn a_compile_outside_a_start_gets_a_starts_time_and_no_more() {
        use crate::models::supervisor::restartable;
        let mut ready = deployment(DeploymentState::Ready);
        ready.demo = false;
        ready.started_at = NOW - 3 * 3600;
        let quiet = |min: u64| Track { stage: Some(compiling("FlashInfer autotune")), progress_ms: Some(NOW * 1000 - min * 60_000), ..Track::default() };
        let seen = |t: &Track| {
            let mut list = merge(Vec::new(), std::slice::from_ref(&ready), &[], &Inventory::example());
            list[0].listening = false;
            list[0].members = vec![Member { node: "gx10-0000".into(), head: true, name: "sparkbox-m1a2b".into(), state: "running".into(), started_at: NOW - 2 * 3600, ..Default::default() }];
            assess(&mut list[0], t, &cluster(), NOW * 1000);
            list.remove(0)
        };
        let e = seen(&quiet(25));
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Loading, Some("Compiling: FlashInfer autotune · no new progress for 25 min (limit 90 min)")));
        assert!(e.compiling);
        // Stale probe failures (it answered until it was restarted) don't make it Down…
        let e = seen(&Track { fails: PROBE_FAILS_DOWN, error: Some("no answer".into()), ..quiet(25) });
        assert_eq!(e.health, Health::Loading);
        // …and whatever else reads Down while it compiles isn't Keep running's.
        let mut down = seen(&quiet(25));
        down.health = Health::Down;
        assert!(!restartable(&down, |_| true));
        // Silent for a whole start's time: hung, and a restart may help.
        let e = seen(&quiet(90));
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Down, Some("Didn't become ready in 90 min")));
        assert!(!e.compiling && restartable(&e, |_| true));

        // What its log last said is about that run: not once its head has gone…
        let mut list = merge(Vec::new(), std::slice::from_ref(&ready), &[], &Inventory::example());
        list[0].listening = false;
        list[0].members = vec![Member { node: "gx10-0000".into(), head: true, name: "sparkbox-m1a2b".into(), state: "exited".into(), exit_code: 1, finished_at: NOW - 60, ..Default::default() }];
        assess(&mut list[0], &quiet(25), &cluster(), NOW * 1000);
        assert!(list[0].health == Health::Down && !list[0].compiling && restartable(&list[0], |_| true), "{:?}", list[0].health_reason);
        // …and not once it listens.
        let mut list = merge(Vec::new(), std::slice::from_ref(&ready), &[], &Inventory::example());
        list[0].listening = true;
        assess(&mut list[0], &quiet(25), &cluster(), NOW * 1000);
        assert!(!list[0].compiling);
    }

    /// A compile needs its ranks. A load whose containers went with a failed restart, or that lost
    /// a worker, isn't compiling whatever its log last said, and Keep running may act on it. What
    /// its track remembers stays until it's Ready, so this is the only thing that can tell.
    #[test]
    fn a_failed_restart_or_a_lost_worker_isnt_a_compile() {
        use crate::models::supervisor::restartable;
        let t = Track { stage: Some(compiling("FlashInfer kernels")), progress_ms: Some(NOW * 1000 - 5 * 60_000), ..Track::default() };

        // Rollback took every container: only its record is left.
        let mut failed = deployment(DeploymentState::Failed);
        failed.demo = false;
        failed.recoverable = true;
        failed.message = "vLLM stopped on gx10-0000 (exit 1).".into();
        let mut list = merge(Vec::new(), std::slice::from_ref(&failed), &[], &Inventory::example());
        assess(&mut list[0], &t, &cluster(), NOW * 1000);
        assert_eq!((list[0].health, list[0].members.len()), (Health::Down, 0));
        assert!(!list[0].compiling && restartable(&list[0], |_| true), "{:?}", list[0].health_reason);

        // Its head is compiling, on the log, and a worker has died under it.
        let mut ready = deployment(DeploymentState::Ready);
        ready.demo = false;
        ready.started_at = NOW - 3 * 3600;
        ready.spec.nodes = vec!["cell1".into(), "cell3".into()];
        ready.spec.tensor_parallel = 2;
        ready.spec.pipeline_parallel = 1;
        let seen = |worker: &str, code: i32| {
            let mut list = merge(Vec::new(), std::slice::from_ref(&ready), &[], &Inventory::example());
            list[0].listening = false;
            list[0].members = vec![
                Member { node: "cell1".into(), head: true, name: "sparkbox-m1a2b-r0".into(), state: "running".into(), started_at: NOW - 600, ..Default::default() },
                Member { node: "cell3".into(), name: "sparkbox-m1a2b-r1".into(), state: worker.into(), exit_code: code, finished_at: NOW - 32, started_at: NOW - 600, ..Default::default() },
            ];
            assess(&mut list[0], &t, &cluster(), NOW * 1000);
            list.remove(0)
        };
        let e = seen("exited", 137);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Down, Some("Worker on cell3 isn't running (killed, exit 137, 32 s ago) · tensor-parallel rank 2 of 2")));
        assert!(!e.compiling && restartable(&e, |_| true));
        // Every rank up, it is compiling.
        let e = seen("running", 0);
        assert_eq!((e.health, e.health_reason.as_deref()), (Health::Loading, Some("Compiling: FlashInfer kernels")));
        assert!(e.compiling);
    }

    /// A server SparkBox didn't start is Down at 20 minutes, compiling or not. (A new compile line
    /// moves its clock: watch.rs.)
    #[test]
    fn a_server_sparkbox_didnt_start_gets_twenty_minutes_of_silence() {
        let mut snap = cluster();
        snap.nodes[0].listeners.retain(|l| l.port != 8000);
        snap.nodes[0].containers[0].started_at = NOW - 3600;
        let track = |min: u64| Track { stage: Some(compiling("FlashInfer kernels")), progress_ms: Some(NOW * 1000 - min * 60_000), ..llama_track().remove("cell1-vllm-8000").unwrap() };
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), track(19))]))[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Loading, Some("Compiling: FlashInfer kernels")));
        let v = &assessed(&snap, &BTreeMap::from([("cell1-vllm-8000".to_string(), track(21))]))[0];
        assert_eq!((v.health, v.health_reason.as_deref()), (Health::Down, Some("Didn't become ready in 20 min")));
    }

    // ---- Stalled: requests in the engine, and no tokens out of it

    /// A Ready server, read every 3 s from `now`: `running` and `waiting` requests, `tokens` per read.
    fn scraped(reads: u64, running: f64, waiting: f64, tokens: f64) -> Track {
        let now = NOW * 1000;
        let mut t = Track { last_ok_ms: Some(now), health_ok_ms: Some(now), canary_ok_ms: Some(now), members: llama_track()["cell1-vllm-8000"].members.clone(), ..Default::default() };
        for i in 0..reads {
            record_scrape(&mut t, Some(5000.0 + i as f64 * tokens), running, waiting, true, now + i * 3_000);
        }
        t
    }

    fn stall_verdict(t: &Track, at_s: u64) -> (Health, Option<String>) {
        let snap = cluster();
        let at = NOW * 1000 + at_s * 1000;
        // /health says 200 all along (a deadlocked engine's does), and its model list answers.
        let mut t = Track { last_ok_ms: Some(at), canary_ok_ms: Some(at), ..t.clone() };
        record_health(&mut t, 200, "/health", at);
        let mut e = discover(&snap, &Inventory::example()).remove(0);
        assess(&mut e, &t, &snap, at);
        (e.health, e.health_reason)
    }

    #[test]
    fn a_stall_is_at_60_s_not_59_with_requests_running_or_only_waiting() {
        assert_eq!(STALL_AFTER_S, 60);
        // Read to the second: the last read a whole number of seconds after the first.
        let running = scraped(21, 3.0, 0.0, 0.0);
        assert_eq!(stall_verdict(&running, 59).0, Health::Ready, "59 s isn't a stall");
        assert_eq!(stall_verdict(&running, 60), (Health::Down, Some("Stalled: 3 requests running, no tokens for 60 s".to_string())));
        // Only waiting: the same clock, and the reason says so. /health was 200 throughout.
        let waiting = scraped(21, 0.0, 2.0, 0.0);
        assert_eq!(stall_verdict(&waiting, 59).0, Health::Ready, "59 s isn't a stall");
        assert_eq!(stall_verdict(&waiting, 60), (Health::Down, Some("Stalled: 2 requests waiting, no tokens for 60 s".to_string())));
        assert_eq!(stall_verdict(&scraped(21, 0.0, 1.0, 0.0), 61).1.as_deref(), Some("Stalled: 1 request waiting, no tokens for 61 s"));
        // Both: what's running is what's stuck.
        assert_eq!(stall_verdict(&scraped(21, 1.0, 4.0, 0.0), 60).1.as_deref(), Some("Stalled: 1 request running, no tokens for 60 s"));
        // Neither, however long the counter stands still.
        assert_eq!(stall_verdict(&scraped(40, 0.0, 0.0, 0.0), 120).0, Health::Ready);
    }

    #[test]
    fn rising_tokens_clear_a_stall() {
        let mut t = scraped(21, 0.0, 2.0, 0.0);
        assert_eq!(stall_verdict(&t, 60).0, Health::Down);
        let now = NOW * 1000;
        record_scrape(&mut t, Some(5100.0), 1.0, 1.0, true, now + 63_000);
        assert_eq!(t.flat_since_ms, None, "tokens came out");
        assert_eq!(stall_verdict(&t, 63), (Health::Ready, None));
        // Tokens rising the whole time never stall, however many wait.
        assert_eq!(stall_verdict(&scraped(40, 2.0, 9.0, 12.0), 120).0, Health::Ready);
    }

    #[test]
    fn corrupted_requests_reach_the_endpoints_json() {
        let mut e = assessed(&cluster(), &llama_track()).remove(0);
        let mut ring = crate::models::metrics::Ring::default();
        e.metrics = Some(ring.push(crate::models::metrics::Scrape::from_text(include_str!("fixtures/vllm_metrics_corrupted.txt"), 3_000)).clone());
        e.corrupted_requests = e.metrics.as_ref().and_then(|m| m.corrupted_requests);
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!((v["corruptedRequests"].as_f64(), v["metrics"]["corruptedRequests"].as_f64()), (Some(3.0), Some(3.0)));
    }

    #[test]
    fn a_restarted_load_doesnt_show_the_last_runs_passed_check() {
        use crate::models::readiness::Readiness;
        let mut d = deployment(DeploymentState::Starting);
        d.demo = false;
        d.message = "Restarting…".into();
        d.stage = Some(Stage::Starting);
        let passed = Readiness { ok: true, health: true, models_listed: true, token: Some(true), summary: "Ready: /health 200, qwen3.8-27b listed, a test token in 0.4 s (…)".into(), ..Default::default() };
        d.readiness = Some(passed.clone());
        let mut snap = cluster();
        snap.nodes[3].containers.clear();
        snap.nodes[3].listeners = vec![listen(8000)];
        let mut list = merge(discover(&snap, &Inventory::example()), std::slice::from_ref(&d), &[], &Inventory::example());
        let e = list.iter_mut().find(|e| e.deployment_id.as_deref() == Some("m1a2b")).unwrap();
        assess(e, &Track::default(), &snap, NOW * 1000);
        assert_eq!(e.health, Health::Loading);
        assert_eq!(serde_json::to_value(&*e).unwrap()["readiness"], Value::Null, "the card's readiness line is the new run's");
        // Ready again: its own check shows.
        let ready = Deployment { state: DeploymentState::Ready, stage: Some(Stage::Ready), ..d };
        let list = merge(discover(&snap, &Inventory::example()), std::slice::from_ref(&ready), &[], &Inventory::example());
        assert_eq!(list.iter().find(|e| e.deployment_id.as_deref() == Some("m1a2b")).unwrap().readiness, Some(passed));
    }
}
