//! How a load is started, following NVIDIA's multi-Spark vLLM guide and vLLM's own multi-node
//! mode: one container per node from the same image, workers first and the head last, meeting at
//! the head's fabric address with `--nnodes/--node-rank/--headless`, with NCCL/Gloo/UCX pinned to
//! each node's ConnectX-7 interface. No shell reads a load's options: `rank_argv` and `rank_env`
//! go to the node as JSON files, and `launch.py` supervises vLLM from them.
//! Everything here is a pure function of the spec; mod.rs runs it over SSH.
//!
//! Guardrails live here too, so the Load page and `deploy()` apply the very same rules: the port
//! must be free, the memory plan must not be red (unless the user typed the node's name to
//! override it), the split must suit the model, raw vLLM options can't reach what SparkBox sets
//! itself or what loads code, `--trust-remote-code` only comes from its own checkbox, and the API
//! key never touches a command line or the container's config: it arrives on stdin, goes into a
//! 0400 file on the node, and is mounted read-only where the launcher reads it into vLLM's
//! environment. SGLang keeps its own `sglang_command`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::fit::{self, ArchFacts};
use super::recipe;
use crate::identity::instance_name;
use crate::live::sh_quote;
use crate::model::NodeSnapshot;

pub const RAY_PORT: u16 = 6379;
/// Where SparkBox starts looking for a free API port.
pub const FIRST_PORT: u16 = 8000;
/// Where a several-node load's ranks meet, on the head's fabric address.
pub const MASTER_PORT: u16 = 29501;
/// The launcher, mounted read-only into every vLLM container.
pub const LAUNCH: &str = include_str!("../live/launch.py");
/// Different builds must never replace the launcher another load is about to mount.
pub fn launcher_name() -> String {
    format!("launch-{}.py", crate::live::sha256_hex(LAUNCH.as_bytes()))
}
/// launch.py's exit when it won't start vLLM, among them a missing or empty key file.
pub const KEY_REFUSED: i32 = 78;

fn yes() -> bool {
    true
}

fn lan() -> String {
    "lan".into()
}

fn vllm() -> String {
    "vllm".into()
}

fn one() -> u64 { 1 }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeploySpec {
    pub id: String,
    /// "vllm" or "sglang".
    #[serde(default = "vllm")]
    pub engine: String,
    /// Name clients use in requests, e.g. "qwen3.8-27b".
    pub served_name: String,
    /// Absolute path of the model folder; identical on every node.
    pub model_path: String,
    /// Node ids; the first one is the head and serves the API.
    pub nodes: Vec<String>,
    /// Independent loads, each with TP × PP ranks. Expanded before launching anything.
    #[serde(default = "one")]
    pub replicas: u64,
    pub tensor_parallel: u64,
    pub pipeline_parallel: u64,
    pub expert_parallel: bool,
    pub max_model_len: Option<u64>,
    pub gpu_memory_utilization: f64,
    pub port: u16,
    pub image: String,
    pub extra_args: Vec<String>,
    /// --trust-remote-code runs Python from the model repo; only from its own checkbox.
    #[serde(default)]
    pub allow_remote_code: bool,
    /// Where the API listens: "lan" (the head node's LAN address), "fabric" or "all".
    #[serde(default = "lan")]
    pub listen: String,
    /// The address `listen` resolved to; filled in by SparkBox, not the UI.
    #[serde(default)]
    pub host: String,
    /// Protect the API with a per-load key (Keychain "llm:vllm-<id>", given to vLLM as VLLM_API_KEY).
    #[serde(default = "yes")]
    pub require_key: bool,
    /// Requests served at once (--max-num-seqs); the memory plan reserves KV for this many full-length ones.
    #[serde(default)]
    pub max_num_seqs: Option<u64>,
    /// "auto" or "fp8" (half the KV-cache memory).
    #[serde(default)]
    pub kv_cache_dtype: Option<String>,
    /// None leaves vLLM's default (on); Some(false) turns prefix caching off.
    #[serde(default)]
    pub prefix_caching: Option<bool>,
    /// Tool calling: the parser for this model family (hermes, llama3_json, qwen3_coder…).
    #[serde(default)]
    pub tool_call_parser: Option<String>,
    #[serde(default)]
    pub reasoning_parser: Option<String>,
    /// Start again after a crash or reboot (single node: docker's unless-stopped policy).
    #[serde(default)]
    pub keep_running: bool,
    /// Load despite a red memory plan (the UI asks for the node's name first).
    #[serde(default)]
    pub override_memory: bool,
    /// The image picked for a vLLM load, by name: `--label sparkbox.image` carries it.
    #[serde(default)]
    pub image_ref: String,
    /// That image's local id on the node (sha256:<64 hex>), what `docker run` starts. Filled in
    /// by SparkBox from the pre-flight, never by the UI.
    #[serde(default)]
    pub image_id: String,
    /// The owner's consent to drop the page cache first (sudo) when vLLM would otherwise refuse
    /// to start (P1.34).
    #[serde(default)]
    pub drop_caches: bool,
    /// The recipe's id, empty for the generic one (recipe.rs).
    #[serde(default)]
    pub recipe: String,
    /// A reviewed read-only script import. It is validated again before every launch.
    #[serde(default)]
    pub imported_recipe: Option<recipe::Recipe>,
    /// The recipe's environment. Filled in by Rust at deploy, never by the UI, so Restart
    /// rebuilds the same files.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The recipe's vLLM options, one argv word each; filled in as `env` is.
    #[serde(default)]
    pub recipe_args: Vec<String>,
    /// "standard" or "privileged"; filled in as `env` is. Only a head or worker runs privileged.
    #[serde(default)]
    pub docker_access: String,
    /// `sparkbox.group`; empty means the load's own id.
    #[serde(default)]
    pub group: String,
}

impl Default for DeploySpec {
    fn default() -> Self {
        DeploySpec {
            id: String::new(),
            engine: vllm(),
            served_name: String::new(),
            model_path: String::new(),
            nodes: Vec::new(),
            replicas: 1,
            tensor_parallel: 1,
            pipeline_parallel: 1,
            expert_parallel: false,
            max_model_len: None,
            gpu_memory_utilization: fit::FIRST_LOAD_SHARE,
            port: FIRST_PORT,
            image: String::new(),
            extra_args: Vec::new(),
            allow_remote_code: false,
            listen: lan(),
            host: String::new(),
            require_key: true,
            max_num_seqs: None,
            kv_cache_dtype: None,
            prefix_caching: None,
            tool_call_parser: None,
            reasoning_parser: None,
            keep_running: false,
            override_memory: false,
            image_ref: String::new(),
            image_id: String::new(),
            drop_caches: false,
            recipe: String::new(),
            imported_recipe: None,
            env: BTreeMap::new(),
            recipe_args: Vec::new(),
            docker_access: String::new(),
            group: String::new(),
        }
    }
}

/// The image a vLLM load starts when none is on the node yet: the one on all four Sparks (P1.32).
pub const DEFAULT_IMAGE: &str = "vllm/vllm-openai:glm53-flash-arm64-cu130";

/// A load started by the launcher: the image by its local id, the caches mounted, on any number
/// of nodes (P1.32, P1.33, P2.2). SGLang keeps its own command.
pub fn is_native(spec: &DeploySpec) -> bool {
    spec.engine == "vllm"
}

/// What node `rank` of the load does: rank 0 serves the API.
pub fn role_of(spec: &DeploySpec, rank: usize) -> &'static str {
    match (spec.nodes.len(), rank) {
        (0 | 1, _) => "single",
        (_, 0) => "head",
        _ => "worker",
    }
}

/// A rank's container. Named by its rank: a node's id, name and hostname are three different things.
pub fn rank_container(id: &str, rank: usize, nodes: usize) -> String {
    if nodes < 2 {
        format!("sparkbox-{id}")
    } else {
        format!("sparkbox-{id}-r{rank}")
    }
}

/// The image's name for a load: the one picked, or (a load saved before images were picked) its image.
pub fn image_ref(spec: &DeploySpec) -> &str {
    if spec.image_ref.is_empty() {
        &spec.image
    } else {
        &spec.image_ref
    }
}

/// A local image id as Docker prints it: sha256: and 64 hex digits.
pub fn is_image_id(id: &str) -> bool {
    id.strip_prefix("sha256:").is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

/// An image name SparkBox passes on: `[A-Za-z0-9._/:@-]`, nothing a shell would read.
pub fn is_image_ref(r: &str) -> bool {
    !r.is_empty() && r.len() <= 255 && r.chars().all(|c| c.is_ascii_alphanumeric() || "._/:@-".contains(c))
}

/// Where a load is. Serialised as the plain words the UI and saved files have always used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentState {
    #[default]
    Starting,
    Ready,
    Failed,
    Stopped,
}

impl DeploymentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeploymentState::Starting => "starting",
            DeploymentState::Ready => "ready",
            DeploymentState::Failed => "failed",
            DeploymentState::Stopped => "stopped",
        }
    }

    /// Holding nodes and memory: starting or serving.
    pub fn active(&self) -> bool {
        matches!(self, DeploymentState::Starting | DeploymentState::Ready)
    }
}

impl From<&str> for DeploymentState {
    fn from(s: &str) -> Self {
        match s {
            "ready" => DeploymentState::Ready,
            "failed" => DeploymentState::Failed,
            "stopped" => DeploymentState::Stopped,
            _ => DeploymentState::Starting,
        }
    }
}

impl PartialEq<&str> for DeploymentState {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl std::fmt::Display for DeploymentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A node's fabric identity, from `sparkjob.py check`.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeNet {
    pub node_id: String,
    pub iface: String,
    pub ip: String,
    /// RDMA devices of the fabric interfaces that have addresses (NCCL_IB_HCA).
    pub hcas: Vec<String>,
}

fn safe(s: &str, extra: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c) || extra.contains(c))
}

/// One word no shell or path would read anything into: letters, digits and . _ -, not starting
/// with . or -. A load's id ends up in `rm -rf ~/.sparkbox/deploy/<id>`.
pub fn is_plain_word(s: &str) -> bool {
    safe(s, "") && !s.contains('/') && !s.starts_with(['-', '.'])
}

pub fn validate(spec: &DeploySpec) -> Result<(), String> {
    if !is_plain_word(&spec.id) || spec.id.len() > 40 {
        return Err("bad deployment id".into());
    }
    if spec.nodes.is_empty() {
        return Err("Pick at least one node.".into());
    }
    if !(1..=4).contains(&spec.replicas) || spec.nodes.iter().collect::<std::collections::HashSet<_>>().len() != spec.nodes.len() {
        return Err("Choose one to four replicas on distinct nodes.".into());
    }
    if spec.replicas > 1 && (!is_native(spec) || spec.pipeline_parallel != 1 || ![1, 2].contains(&spec.tensor_parallel)) {
        return Err("Replicas support independent single-node loads or TP2 pairs with vLLM.".into());
    }
    if spec.tensor_parallel.checked_mul(spec.pipeline_parallel).and_then(|n| n.checked_mul(spec.replicas)) != Some(spec.nodes.len() as u64) {
        return Err(format!(
            "Tensor parallel ({}) × pipeline parallel ({}) × replicas must equal the number of nodes ({}): one GPU per Spark.",
            spec.tensor_parallel,
            spec.pipeline_parallel,
            spec.nodes.len()
        ));
    }
    if !spec.model_path.starts_with('/') || !safe(&spec.model_path, "+@") {
        return Err(format!("The model path “{}” must be absolute and use plain characters.", spec.model_path));
    }
    if !safe(&spec.served_name, ":") {
        return Err("The served model name may use letters, digits and . _ - / : only.".into());
    }
    if !spec.image_ref.is_empty() && !is_image_ref(&spec.image_ref) {
        return Err("An image name may use letters, digits and . _ / : @ - only.".into());
    }
    if !safe(&spec.image, ":@") {
        return Err("bad image name".into());
    }
    if !spec.image_id.is_empty() && !is_image_id(&spec.image_id) {
        return Err("An image's local id is sha256: and 64 hex digits.".into());
    }
    if !(fit::MIN_SHARE..=0.95).contains(&spec.gpu_memory_utilization) {
        return Err("GPU memory utilization must be between 0.2 and 0.95.".into());
    }
    if spec.port < 1024 {
        return Err("Use a port of 1024 or above.".into());
    }
    if !["vllm", "sglang"].contains(&spec.engine.as_str()) {
        return Err("The engine must be vLLM or SGLang.".into());
    }
    if spec.engine == "vllm" {
        check_extra_args(&spec.extra_args)?;
        check_extra_args(&spec.recipe_args)?;
        recipe::check_env(&spec.env)?;
    } else if !spec.extra_args.is_empty() {
        return Err("Expert options are for vLLM loads only.".into());
    }
    if !spec.recipe.is_empty() && !is_plain_word(&spec.recipe) {
        return Err("bad recipe name".into());
    }
    if !spec.group.is_empty() && !is_plain_word(&spec.group) {
        return Err("bad group name".into());
    }
    if !["", "standard", "privileged"].contains(&spec.docker_access.as_str()) {
        return Err("Docker access must be standard or privileged.".into());
    }
    if !["lan", "fabric", "all"].contains(&spec.listen.as_str()) {
        return Err("Listen on must be LAN, fabric or all interfaces.".into());
    }
    if !spec.host.is_empty() && !spec.host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err("bad listen address".into());
    }
    for (name, v) in [("tool-call parser", &spec.tool_call_parser), ("reasoning parser", &spec.reasoning_parser), ("KV cache type", &spec.kv_cache_dtype)] {
        if let Some(v) = v {
            if v.is_empty() || !v.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) {
                return Err(format!("The {name} may use letters, digits and _ - . only."));
            }
        }
    }
    if spec.max_num_seqs == Some(0) {
        return Err("Requests at once must be at least 1.".into());
    }
    if spec.engine == "sglang" {
        if spec.pipeline_parallel != 1 {
            return Err("SGLang loads split by tensor parallel across nodes: set pipeline parallel to 1.".into());
        }
        if spec.require_key {
            return Err(
                "SGLang only takes an API key on its command line, where anyone on the node could read it. Turn off “Require an API key” (and listen on the fabric, or use SparkBox Server in front)."
                    .into(),
            );
        }
    }
    Ok(())
}

/// vLLM options SparkBox sets itself from the load's fields and its rank.
pub const MANAGED_FLAGS: &[&str] = &[
    "--model",
    "--served-model-name",
    "--host",
    "--port",
    "--tensor-parallel-size",
    "--tensor-parallel",
    "--pipeline-parallel-size",
    "--pipeline-parallel",
    "--distributed-executor-backend",
    "--enable-expert-parallel",
    "--max-model-len",
    "--gpu-memory-utilization",
    "--max-num-seqs",
    "--kv-cache-dtype",
    "--enable-prefix-caching",
    "--no-enable-prefix-caching",
    "--tool-call-parser",
    "--enable-auto-tool-choice",
    "--reasoning-parser",
    "--nnodes",
    "--node-rank",
    "--master-addr",
    "--master-port",
    "--headless",
];

/// Short spellings of managed options; `-n4` is `--nnodes 4` to argparse.
const MANAGED_SHORT: &[&str] = &["-tp", "-pp", "-n", "-r"];

/// Options that load code or open files. `--config` is here because it reads a file of options,
/// so it could carry anything else this list refuses.
const CODE_FLAGS: &[&str] = &[
    "--middleware",
    "--tool-parser-plugin",
    "--reasoning-parser-plugin",
    "--allowed-local-media-path",
    "--logits-processors",
    "--worker-cls",
    "--worker-extension-cls",
    "--scheduler-cls",
    "--config",
    "--ssl-",
];

/// Options whose value is JSON: checked here, and passed on as one argv word.
const JSON_FLAGS: &[&str] = &["--speculative-config", "--hf-overrides", "--compilation-config", "--limit-mm-per-prompt", "--override-generation-config"];

/// `name` (`--like-this`) is one of these, or an abbreviation of one, which argparse would expand.
fn names_one(name: &str, list: &[&str]) -> bool {
    list.iter().any(|f| f.starts_with(name))
}

/// The Expert rule for raw vLLM options, the way vLLM's parser reads them: `_` and `-` are the same
/// in a name, `--flag=value` is a flag and its value, a shortened name means the longer one, and
/// `--flag.key value` sets a part of `--flag`. Each element is one argv word and no shell reads
/// it, so quotes and $ are fine. A bare word is the value of the flag before it.
pub fn check_extra_args(args: &[String]) -> Result<(), String> {
    let mut i = 0;
    let mut takes_word = false;
    while i < args.len() {
        let raw = &args[i];
        if raw.is_empty() || raw.contains(['\0', '\n', '\r']) {
            return Err("An option can't be empty or hold a line break.".into());
        }
        if !raw.starts_with('-') || raw.parse::<f64>().is_ok() {
            if !takes_word {
                return Err(format!("“{raw}” isn't an option (options start with --)."));
            }
            takes_word = false;
            i += 1;
            continue;
        }
        let (word, inline) = match raw.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (raw.as_str(), None),
        };
        if word.trim_start_matches('-').is_empty() {
            return Err(format!("“{raw}” isn't an option (options start with --)."));
        }
        if word.contains(char::is_whitespace) {
            return Err(format!("“{raw}” has a space in its name: put the value in its own element."));
        }
        let name = format!("--{}", word.trim_start_matches('-').replace('_', "-"));
        let root = name.split('.').next().unwrap_or(&name);
        if names_one(root, &["--trust-remote-code"]) {
            return Err(
                "--trust-remote-code runs Python code from the model's repository. Tick “Allow remote code” to opt in; it can't be added as an Expert option."
                    .into(),
            );
        }
        if names_one(root, &["--api-key", "--hf-token"]) {
            return Err("Don't put an API key in the options: SparkBox gives vLLM its key from a file only it can read.".into());
        }
        if names_one(root, MANAGED_FLAGS) || (!word.starts_with("--") && MANAGED_SHORT.iter().any(|s| word.starts_with(s))) {
            return Err(format!("{name} is set by SparkBox from the fields above; remove it from the raw vLLM args."));
        }
        if names_one(root, CODE_FLAGS) || root.starts_with("--ssl") || name.contains("plugin") {
            return Err(format!("{name} loads code or opens files, so SparkBox won't pass it on."));
        }
        if inline == Some("") {
            return Err(format!("{name} needs a value."));
        }
        takes_word = inline.is_none();
        if JSON_FLAGS.contains(&name.as_str()) {
            let value = inline.or_else(|| args.get(i + 1).map(String::as_str)).ok_or_else(|| format!("{name} needs a value."))?;
            serde_json::from_str::<Value>(value).map_err(|_| format!("{name} takes JSON, and this isn't: {value}"))?;
            if inline.is_none() {
                i += 1;
                takes_word = false;
            }
        }
        i += 1;
    }
    Ok(())
}

/// Options the AI advisor may fill in. Anything else it suggests is shown, not applied.
pub const ADVISOR_ALLOWED: &[&str] = &[
    "--enable-prefix-caching",
    "--kv-cache-dtype",
    "--max-num-seqs",
    "--quantization",
    "--enforce-eager",
    "--max-model-len",
    "--gpu-memory-utilization",
    "--tool-call-parser",
    "--enable-auto-tool-choice",
    "--reasoning-parser",
];

/// Pulls an image once per node (SGLang's needs nothing added).
pub fn pull_script(image: &str) -> String {
    format!("set -e\ndocker image inspect {image} >/dev/null 2>&1 || docker pull -q {image}\necho ready\n")
}

/// SGLang's options for node `rank` of the load. Several nodes run one launch_server each,
/// tensor parallel across all of them, meeting at the head's fabric address on port 5000.
pub fn sglang_args(spec: &DeploySpec, rank: usize, head_ip: &str) -> Vec<String> {
    let n = spec.nodes.len();
    let mut a: Vec<String> = vec![
        "python3".into(),
        "-m".into(),
        "sglang.launch_server".into(),
        "--model-path".into(),
        spec.model_path.clone(),
        "--served-model-name".into(),
        spec.served_name.clone(),
        "--host".into(),
        bind_host(spec).into(),
        "--port".into(),
        spec.port.to_string(),
        "--tp".into(),
        (spec.tensor_parallel * spec.pipeline_parallel).max(1).to_string(),
        "--mem-fraction-static".into(),
        format!("{:.2}", spec.gpu_memory_utilization),
        "--enable-metrics".into(),
    ];
    if n > 1 {
        a.extend(["--nnodes".into(), n.to_string(), "--node-rank".into(), rank.to_string(), "--dist-init-addr".into(), format!("{head_ip}:{SGLANG_DIST_PORT}")]);
    }
    if let Some(len) = spec.max_model_len {
        a.extend(["--context-length".into(), len.to_string()]);
    }
    if let Some(n) = spec.max_num_seqs {
        a.extend(["--max-running-requests".into(), n.to_string()]);
    }
    if spec.kv_cache_dtype.as_deref().is_some_and(|d| d.starts_with("fp8")) {
        a.extend(["--kv-cache-dtype".into(), "fp8_e5m2".into()]);
    }
    if let Some(p) = &spec.tool_call_parser {
        a.extend(["--tool-call-parser".into(), p.clone()]);
    }
    if let Some(p) = &spec.reasoning_parser {
        a.extend(["--reasoning-parser".into(), p.clone()]);
    }
    if spec.allow_remote_code {
        a.push("--trust-remote-code".into());
    }
    a
}

/// Where a multi-node SGLang load's ranks meet (on the head's fabric address).
pub const SGLANG_DIST_PORT: u16 = 5000;

/// `docker run` for SGLang rank `rank`; `net` pins NCCL to that node's fabric interface.
pub fn sglang_command(spec: &DeploySpec, rank: usize, head_ip: &str, net: Option<&NodeNet>) -> String {
    let role = role_of(spec, rank);
    let node = &spec.nodes[rank];
    let env = net.map(|n| format!(" {}", fabric_env(n, head_ip))).unwrap_or_default();
    let args: Vec<String> = sglang_args(spec, rank, head_ip).iter().map(|a| format!("'{a}'")).collect();
    format!("{}{env} {} {}", run_prefix(spec, node, role, instance_name()), spec.image, args.join(" "))
}

/// Image with Ray added (NVIDIA's newer vLLM images ship without it).
pub fn ray_image(image: &str) -> String {
    let tag = image.rsplit_once(':').map(|(_, t)| t).filter(|t| !t.contains('/')).unwrap_or("latest");
    let tag: String = tag.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '-' }).collect();
    format!("sparkbox/vllm-ray:{tag}")
}

/// The address vLLM's API binds to.
pub fn bind_host(spec: &DeploySpec) -> &str {
    if spec.listen == "all" || spec.host.is_empty() {
        "0.0.0.0"
    } else {
        &spec.host
    }
}

/// Where to reach the API from the head node itself (for probes).
pub fn probe_host(spec: &DeploySpec) -> &str {
    match bind_host(spec) {
        "0.0.0.0" => "127.0.0.1",
        h => h,
    }
}

/// The load's own options, the same on every rank: its fields, then the recipe's, then Expert's.
fn serve_flags(spec: &DeploySpec) -> Vec<String> {
    let mut a: Vec<String> = vec!["--served-model-name".into(), spec.served_name.clone(), "--gpu-memory-utilization".into(), format!("{:.2}", spec.gpu_memory_utilization)];
    if spec.expert_parallel {
        a.push("--enable-expert-parallel".into());
    }
    if let Some(n) = spec.max_model_len {
        a.extend(["--max-model-len".into(), n.to_string()]);
    }
    if let Some(n) = spec.max_num_seqs {
        a.extend(["--max-num-seqs".into(), n.to_string()]);
    }
    if let Some(d) = spec.kv_cache_dtype.as_deref().filter(|d| *d != "auto") {
        a.extend(["--kv-cache-dtype".into(), d.into()]);
    }
    match spec.prefix_caching {
        Some(true) => a.push("--enable-prefix-caching".into()),
        Some(false) => a.push("--no-enable-prefix-caching".into()),
        None => {}
    }
    if let Some(p) = &spec.tool_call_parser {
        a.extend(["--enable-auto-tool-choice".into(), "--tool-call-parser".into(), p.clone()]);
    }
    if let Some(p) = &spec.reasoning_parser {
        a.extend(["--reasoning-parser".into(), p.clone()]);
    }
    if spec.allow_remote_code {
        a.push("--trust-remote-code".into());
    }
    a.extend(spec.recipe_args.iter().cloned());
    a.extend(spec.extra_args.iter().cloned());
    a
}

/// What vLLM serves, whichever rank: the arguments after `vllm`, with the split. A load's identity
/// for the sanity verdict; the argv a rank really gets is `rank_argv`.
pub fn vllm_args(spec: &DeploySpec) -> Vec<String> {
    let mut a: Vec<String> = vec!["serve".into(), spec.model_path.clone()];
    a.extend(serve_flags(spec));
    if spec.nodes.len() > 1 {
        a.extend(["--tensor-parallel-size".into(), spec.tensor_parallel.to_string(), "--pipeline-parallel-size".into(), spec.pipeline_parallel.to_string()]);
    }
    a.extend(["--host".into(), bind_host(spec).into(), "--port".into(), spec.port.to_string()]);
    a
}

/// A rank's argv (C-V). `nets` is every rank's fabric identity, by rank; a load on one node needs none.
pub fn rank_argv(spec: &DeploySpec, rank: usize, nets: &[NodeNet]) -> Vec<String> {
    let n = spec.nodes.len();
    let mut a: Vec<String> = vec!["vllm".into(), "serve".into(), spec.model_path.clone()];
    a.extend(serve_flags(spec));
    if n > 1 {
        a.extend(["--tensor-parallel-size".into(), spec.tensor_parallel.to_string()]);
        if spec.pipeline_parallel > 1 {
            a.extend(["--pipeline-parallel-size".into(), spec.pipeline_parallel.to_string()]);
        }
        let head_ip = nets.first().map(|h| h.ip.clone()).unwrap_or_default();
        a.extend(["--nnodes".into(), n.to_string(), "--node-rank".into(), rank.to_string(), "--master-addr".into(), head_ip, "--master-port".into(), MASTER_PORT.to_string()]);
    }
    if rank == 0 {
        a.extend(["--host".into(), bind_host(spec).into(), "--port".into(), spec.port.to_string()]);
    } else {
        a.push("--headless".into());
    }
    a
}

/// The names `rank_env` sets itself, which a recipe can't.
pub const RANK_ENV: &[&str] = &[
    "HF_HUB_OFFLINE",
    "VLLM_ENGINE_READY_TIMEOUT_S",
    "VLLM_HOST_IP",
    "NCCL_SOCKET_IFNAME",
    "GLOO_SOCKET_IFNAME",
    "TP_SOCKET_IFNAME",
    "UCX_NET_DEVICES",
    "OMPI_MCA_btl_tcp_if_include",
    "NCCL_IB_HCA",
    "NCCL_IB_DISABLE",
    "NCCL_IGNORE_CPU_AFFINITY",
    "NCCL_DEBUG",
];

/// A rank's environment (C-E). With several nodes every value comes from the rank's own `NodeNet`,
/// so all of its traffic stays on that node's fabric interface. The API key is never here.
pub fn rank_env(spec: &DeploySpec, rank: usize, nets: &[NodeNet]) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("HF_HUB_OFFLINE".into(), "1".into());
    env.insert("VLLM_ENGINE_READY_TIMEOUT_S".into(), "3600".into());
    env.extend(spec.env.clone());
    if let Some(net) = nets.get(rank).filter(|_| spec.nodes.len() > 1) {
        env.insert("VLLM_HOST_IP".into(), net.ip.clone());
        for name in ["NCCL_SOCKET_IFNAME", "GLOO_SOCKET_IFNAME", "TP_SOCKET_IFNAME", "UCX_NET_DEVICES", "OMPI_MCA_btl_tcp_if_include"] {
            env.insert(name.into(), net.iface.clone());
        }
        if !net.hcas.is_empty() {
            env.insert("NCCL_IB_HCA".into(), net.hcas.join(","));
        }
        env.insert("NCCL_IB_DISABLE".into(), "0".into());
        env.insert("NCCL_IGNORE_CPU_AFFINITY".into(), "1".into());
        env.insert("NCCL_DEBUG".into(), "INFO".into());
    }
    env
}

/// What `sparkjob.py deploy-files` reads on stdin (C-F). The key goes to rank 0 only.
pub fn launch_payload(spec: &DeploySpec, rank: usize, nets: &[NodeNet], key: Option<&str>) -> Value {
    json!({ "argv": rank_argv(spec, rank, nets), "env": rank_env(spec, rank, nets), "key": key.filter(|_| rank == 0 && spec.require_key) })
}

pub fn container_name(id: &str, node: &str, role: &str) -> String {
    match role {
        "single" => format!("sparkbox-{id}"),
        "head" => format!("sparkbox-{id}-head"),
        _ => format!("sparkbox-{id}-{node}"),
    }
}

/// Env var vLLM reads its API key from (the value never appears in a command).
pub const KEY_ENV: &str = "VLLM_API_KEY";
/// Where the key file is mounted inside the container (read-only).
pub const KEY_MOUNT: &str = "/run/sparkbox/key";

/// The key file on the node: readable by its owner only, removed on unload.
pub fn key_file(id: &str) -> String {
    format!(".sparkbox/keys/{id}")
}

/// A load's launch files on the node, removed on unload.
pub fn deploy_dir(id: &str) -> String {
    format!(".sparkbox/deploy/{id}")
}

/// Keychain account of a load's API key; also its provider's key, so Chat and the server use it.
pub fn key_account(id: &str) -> String {
    format!("llm:vllm-{id}")
}

/// vLLM's compile caches (torch.compile, FlashInfer's JIT, Triton's kernels), kept on the node
/// so the next load of a model skips the compiling (P1.33): the folder under the cluster
/// user's home, and where the container's root user looks for it.
const CACHE_DIRS: [(&str, &str); 3] = [(".cache/vllm", "/root/.cache/vllm"), (".cache/flashinfer", "/root/.cache/flashinfer"), (".triton", "/root/.triton")];

/// `docker run` up to the image, for SGLang. It takes no key: `validate` refuses one.
fn run_prefix(spec: &DeploySpec, node: &str, role: &str, instance: &str) -> String {
    let restart = if spec.keep_running && role == "single" { "unless-stopped" } else { "no" };
    // Its place in the pipeline (the head is 0), so a stage keeps its number even when its node is gone.
    let rank = spec.nodes.iter().position(|n| n == node).unwrap_or(0);
    // Every container SparkBox starts is tagged with the instance that owns it, so the other
    // SparkBox never pre-cleans, reconciles or stops it (P1.2).
    format!(
        "docker run -d --name {} --label sparkbox.instance={instance} --label sparkbox.deployment={} --label sparkbox.role={role} --label sparkbox.rank={rank} --label sparkbox.model={} \
         --network host --ipc host --gpus all --restart {restart} -v {}:{}:ro",
        container_name(&spec.id, node, role),
        spec.id,
        spec.served_name,
        mount_root(&spec.model_path),
        mount_root(&spec.model_path)
    )
}

/// Where the launcher, its files and the key sit inside a container.
const LAUNCH_MOUNT: &str = "/run/sparkbox/launch.py";
const DEPLOY_MOUNT: &str = "/run/sparkbox/deploy";

/// A rank's `docker run` (C-L2), this build's instance. One command, no preamble, nothing in it a
/// shell has to read but `$HOME`: its options are in the deploy folder, which `deploy-files`
/// writes first. Labels carry what discovery needs, because the Command only says launch.py.
pub fn rank_command(spec: &DeploySpec, rank: usize) -> String {
    rank_command_for(spec, rank, instance_name())
}

/// `rank_command` as the `instance` build writes it.
pub fn rank_command_for(spec: &DeploySpec, rank: usize, instance: &str) -> String {
    let id = &spec.id;
    let role = role_of(spec, rank);
    let group = if spec.group.is_empty() { id } else { &spec.group };
    let recipe = if spec.recipe.is_empty() { "generic" } else { &spec.recipe };
    let listens = if rank == 0 { format!(" --label sparkbox.port={} --label sparkbox.listen={}", spec.port, bind_host(spec)) } else { String::new() };
    let maxlen = spec.max_model_len.map(|n| format!(" --label sparkbox.maxlen={n}")).unwrap_or_default();
    // NVIDIA's flag for the first runs; tightening it is a Session 1 result.
    let privileged = if role != "single" && spec.docker_access == "privileged" { " --privileged" } else { "" };
    let restart = if spec.keep_running && role == "single" { "unless-stopped" } else { "no" };
    let root = mount_root(&spec.model_path);
    let caches: String = CACHE_DIRS.iter().map(|(d, inside)| format!(" -v \"$HOME/{d}:{inside}\"")).collect();
    let keyed = spec.require_key && rank == 0;
    let (key_mount, key_flag) = if keyed { (format!(" -v \"$HOME/{}:{KEY_MOUNT}:ro\"", key_file(id)), " --key") } else { (String::new(), "") };
    format!(
        "docker run -d --name {name} --label sparkbox.instance={instance} --label sparkbox.deployment={id} --label sparkbox.group={group} \
         --label sparkbox.role={role} --label sparkbox.rank={rank} --label sparkbox.model={served} --label sparkbox.recipe={recipe} \
         --label sparkbox.image={image} --label sparkbox.path={path} --label sparkbox.parallel={tp}x{pp} --label sparkbox.gpu-share={share:.2}{listens}{maxlen} \
         --network host --ipc host --gpus all --ulimit memlock=-1 --ulimit stack=67108864 --ulimit nofile=1048576:1048576{privileged} \
         --restart {restart} -v {root}:{root}:ro{caches} -v \"$HOME/{deploy}:{DEPLOY_MOUNT}:ro\" -v \"$HOME/.sparkbox/{launcher}:{LAUNCH_MOUNT}:ro\"{key_mount} \
         --entrypoint python3 {image_id} {LAUNCH_MOUNT}{key_flag}",
        launcher = launcher_name(),
        name = rank_container(id, rank, spec.nodes.len()),
        served = spec.served_name,
        image = image_ref(spec),
        path = spec.model_path,
        tp = spec.tensor_parallel,
        pp = spec.pipeline_parallel,
        share = spec.gpu_memory_utilization,
        deploy = deploy_dir(id),
        image_id = spec.image_id,
    )
}

/// What to mount for a model folder. A Hugging Face cache snapshot is a folder of
/// symlinks into ../../blobs, so the whole models--org--name folder goes in.
pub fn mount_root(path: &str) -> &str {
    match path.find("/models--") {
        Some(i) => match path[i + 1..].find('/') {
            Some(j) => &path[..i + 1 + j],
            None => path,
        },
        None => path,
    }
}

/// NVIDIA's settings: every transport pinned to the node's own fabric interface.
fn fabric_env(n: &NodeNet, head_ip: &str) -> String {
    let mut env = vec![
        format!("VLLM_HOST_IP={}", n.ip),
        format!("NCCL_SOCKET_IFNAME={}", n.iface),
        format!("GLOO_SOCKET_IFNAME={}", n.iface),
        format!("TP_SOCKET_IFNAME={}", n.iface),
        format!("UCX_NET_DEVICES={}", n.iface),
        format!("OMPI_MCA_btl_tcp_if_include={}", n.iface),
        format!("MASTER_ADDR={head_ip}"),
        "RAY_memory_monitor_refresh_ms=0".into(),
    ];
    if !n.hcas.is_empty() {
        env.push(format!("NCCL_IB_HCA={}", n.hcas.join(",")));
    }
    env.iter().map(|e| format!("-e {e}")).collect::<Vec<_>>().join(" ")
}

/// The single-node vLLM load: rank 0 of one.
#[cfg(test)]
pub fn single_command(spec: &DeploySpec) -> String {
    rank_command(spec, 0)
}

/// `single_command` as the `instance` build (release or dev) writes it.
#[cfg(test)]
pub fn single_command_for(spec: &DeploySpec, instance: &str) -> String {
    rank_command_for(spec, 0, instance)
}

/// Stops a load's containers on a node and removes them, then its key file and launch files
/// (C-U2). vLLM gets 30 s to shut down on TERM (docker then KILLs it), and only a stopped
/// container is removed: never `rm -f`, whose KILL can leave vLLM's shared memory behind in
/// /dev/shm. A container still running makes `docker rm` fail, and the unload's check that none
/// is left reports it.
pub fn unload_command(id: &str) -> String {
    unload_command_for(id, instance_name())
}

/// `unload_command` as the `instance` build writes it.
pub fn unload_command_for(id: &str, instance: &str) -> String {
    // An id can come from a label on the node, and this one goes to `rm -rf`: never send it unchecked.
    if !is_plain_word(id) {
        return "echo 'not a load id' >&2; exit 1".into();
    }
    // Only this instance's containers (P1.2): the other SparkBox's are never pre-cleaned or removed.
    format!("{}rm -rf ~/{} ~/{}; echo ok", stop_and_remove(&format!("--filter label=sparkbox.instance={instance} --filter label=sparkbox.deployment={id}")), key_file(id), deploy_dir(id))
}

/// `ids=$(docker ps -aq <filters>)`, then `docker stop -t 30` and `docker rm` of what it found.
fn stop_and_remove(filters: &str) -> String {
    format!("ids=$(docker ps -aq {filters}); [ -z \"$ids\" ] || docker stop -t 30 $ids >/dev/null; [ -z \"$ids\" ] || docker rm $ids >/dev/null; ")
}

/// Every container of a load on a node, whichever SparkBox started it (a read): its id and its
/// sparkbox.instance label, empty when it has none, one per line.
pub fn load_containers_command(id: &str) -> String {
    format!("docker ps -a --filter label=sparkbox.deployment={id} --format '{{{{.ID}}}} {{{{.Label \"sparkbox.instance\"}}}}'")
}

/// The names of the containers labelled with this deployment id on a node (a read). A download's
/// container carries its job's id there, so an id alone doesn't say it's a load's.
pub fn container_names_command(id: &str) -> String {
    format!("docker ps -a --filter label=sparkbox.deployment={id} --format '{{{{.Names}}}}'")
}

/// What sparkjob.py names a download's container, with its job's id after it.
pub const DOWNLOAD_CONTAINER: &str = "sparkbox-dl-";

/// A load's containers on one node, from `load_containers_command`, by who owns them.
#[derive(Debug, Default, PartialEq)]
pub struct LoadContainers {
    /// This instance's.
    pub mine: Vec<String>,
    /// The other instance's: never ours to remove (P1.2).
    pub others: Vec<String>,
    /// With no instance label (started before labels): whose they are can't be told.
    pub unlabelled: Vec<String>,
}

pub fn load_containers(out: &str) -> LoadContainers {
    let mut found = LoadContainers::default();
    for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (id, instance) = line.split_once(' ').unwrap_or((line, ""));
        let list = match instance.trim() {
            "" => &mut found.unlabelled,
            i if i == instance_name() => &mut found.mine,
            _ => &mut found.others,
        };
        list.push(id.to_string());
    }
    found
}

/// Stops (and removes) one role's container of a load on its node, for an ordered restart (C-S):
/// `docker stop -t 30`, then `docker rm`, as an unload does.
pub fn stop_role_command(id: &str, role: &str) -> String {
    stop_role_command_for(id, role, instance_name())
}

/// `stop_role_command` as the `instance` build writes it.
pub fn stop_role_command_for(id: &str, role: &str, instance: &str) -> String {
    format!("{}echo ok", stop_and_remove(&format!("--filter label=sparkbox.instance={instance} --filter label=sparkbox.deployment={id} --filter label=sparkbox.role={role}")))
}

/// Head container state, HTTP status of /health, and the log tail, in one round trip. The state
/// line ends with Docker's RestartCount: a load with `--restart unless-stopped` reads `running` at
/// every look while it crashes and comes back, and only the count tells.
pub fn probe_command(container: &str, host: &str, port: u16) -> String {
    format!(
        "docker inspect -f '{{{{.State.Status}}}} {{{{.State.ExitCode}}}} {{{{.RestartCount}}}}' {container} 2>/dev/null || echo 'missing 0'; \
         curl -s -o /dev/null -m 3 -w '%{{http_code}}\\n' http://{host}:{port}/health || echo 000; \
         docker logs --tail 40 {container} 2>&1 | tail -n 40"
    )
}

/// Prints anything listening on the port (empty when free); run right before `docker run`.
pub fn port_check_command(port: u16) -> String {
    format!("ss -Hltn 'sport = :{port}' 2>/dev/null")
}

// ---- Pre-flight on the head node (P1.34)

const GIB_BYTES: u64 = 1024 * 1024 * 1024;
/// What vLLM's startup check wants free besides its share: it counts the page cache as used, so
/// MemFree, not MemAvailable, is what has to be there.
pub const FREE_HEADROOM: u64 = 4 * GIB_BYTES;

/// A node's memory from /proc/meminfo, in bytes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mem {
    pub total: u64,
    pub free: u64,
    pub available: u64,
    pub cached: u64,
}

/// earlyoom on the node: running or not, and its options. Shown, never blocking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Earlyoom {
    pub active: bool,
    /// What `systemctl is-active` said ("active", "inactive", "failed"…).
    pub state: String,
    /// The EARLYOOM_ARGS line of /etc/default/earlyoom (None: no such file, or no such line).
    pub config: Option<String>,
}

/// One of vLLM's cache folders on the node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheDir {
    /// "~/.cache/vllm".
    pub path: String,
    pub exists: bool,
    pub owner: Option<String>,
}

/// A labelled container on a node (C-P2's `containers`), or a Ray worker (`ray`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Leftover {
    pub id: String,
    pub name: String,
    /// Its sparkbox.deployment label.
    pub deployment: String,
    /// Its sparkbox.instance label, empty when it has none.
    pub instance: String,
    /// `docker ps`'s status, "Up 3 hours" or "Exited (1) 2 days ago".
    pub status: String,
    /// This build started it, so SparkBox may remove it. Another build's, or one with no
    /// instance label, is reported and never removed.
    pub ours: bool,
}

/// One fabric interface as `sparkjob.py check` reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FabricFact {
    pub iface: String,
    pub ip: String,
    pub prefix: u32,
    pub speed_mbps: i64,
    pub rdma: String,
    pub mtu: Option<u32>,
    pub operstate: Option<String>,
    pub rdma_state: Option<String>,
    pub rdma_phys: Option<String>,
}

/// What a node has before a load (C-P2): its memory, earlyoom, the image's local id, Docker's
/// image store, the model folder, the ports, /dev/shm, the cache folders, what's left of earlier
/// loads and what Ollama holds in memory.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    #[serde(default)]
    pub versions: std::collections::BTreeMap<String, Option<String>>,
    pub node: String,
    /// Demo data: nothing was read.
    pub demo: bool,
    pub mem: Option<Mem>,
    /// share × MemTotal + 4 GiB: what vLLM wants free at startup.
    pub need_free: u64,
    /// MemFree is below `need_free`: vLLM would refuse to start unless the page cache is dropped.
    pub drop_needed: bool,
    pub earlyoom: Option<Earlyoom>,
    pub image_ref: String,
    /// sha256:<64 hex>, when the image is on the node.
    pub image_id: Option<String>,
    pub model_present: bool,
    pub port_free: bool,
    /// What listens on the port, when something does.
    pub port_owner: Option<String>,
    /// The names in /dev/shm.
    pub shm: Vec<String>,
    pub caches: Vec<CacheDir>,
    /// What stops the load, in words.
    pub problems: Vec<String>,
    /// Edit & reload, read while the load it replaces still serves here: that load's name
    /// (`count_replaced`).
    #[serde(default)]
    pub replacing: Option<String>,
    /// The replaced load's share of MemTotal, counted as free when `drop_needed` is judged.
    #[serde(default)]
    pub freed: u64,
    /// Docker's image store: "containerd", or the storage driver ("overlay2"). None: Docker
    /// didn't say. Image ids only compare within one store.
    #[serde(default)]
    pub store: Option<String>,
    /// Every sparkbox-labelled container here, whichever load and build. mod.rs tells a live
    /// load's from one left behind.
    #[serde(default)]
    pub leftovers: Vec<Leftover>,
    /// Download containers here, which share that label: running jobs, not leftovers. mod.rs moves
    /// them here from `leftovers`; a load can start beside one.
    #[serde(default)]
    pub downloads: Vec<Leftover>,
    /// Containers running `ray start --address`, which nothing of SparkBox's starts.
    #[serde(default)]
    pub ray_leftovers: Vec<Leftover>,
    /// The models Ollama holds in memory here.
    #[serde(default)]
    pub ollama_resident: Vec<String>,
    /// From `sparkjob.py check`; filled in by mod.rs.
    #[serde(default)]
    pub fabric: Vec<FabricFact>,
    /// The read didn't finish, so its other fields say nothing about the node (`problems` says
    /// where it stopped).
    #[serde(default)]
    pub cut_off: bool,
}

/// The chosen nodes as one, checked just now: the Load page's verdict and `deploy()`'s.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightSet {
    #[serde(default)]
    pub versions: Option<crate::versions::Report>,
    pub nodes: Vec<Preflight>,
    /// "ready", "blocked" or "demo".
    pub verdict: String,
    pub problems: Vec<String>,
    /// The one image id every node holds, when they agree.
    pub image_id: Option<String>,
    pub checked_at_ms: u64,
}

/// The pre-flight read (C-P2), one round trip: every section is read whatever the others say.
pub fn preflight_script(spec: &DeploySpec) -> String {
    let caches: Vec<String> = CACHE_DIRS.iter().map(|(d, _)| format!("~/{d}")).collect();
    let ports = if spec.nodes.len() > 1 { format!("( sport = :{} or sport = :{MASTER_PORT} )", spec.port) } else { format!("sport = :{}", spec.port) };
    format!(
        "echo '--- mem'; grep -E '^(MemTotal|MemFree|MemAvailable|Cached):' /proc/meminfo; \
         echo '--- earlyoom'; systemctl is-active earlyoom; cat /etc/default/earlyoom 2>&1; \
         echo '--- image'; docker image inspect -f '{{{{.Id}}}}' {} 2>&1; \
         echo '--- store'; docker info -f '{{{{.Driver}}}} {{{{json .DriverStatus}}}}' 2>&1; \
         echo '--- model'; {} && echo present; \
         echo '--- port'; ss -Hltn '{ports}'; \
         echo '--- shm'; ls -la /dev/shm; \
         echo '--- caches'; ls -ld {} 2>&1; \
         echo '--- containers'; docker ps -a --filter label=sparkbox.deployment --format '{{{{.ID}}}}|{{{{.Names}}}}|{{{{.Label \"sparkbox.deployment\"}}}}|{{{{.Label \"sparkbox.instance\"}}}}|{{{{.Status}}}}' 2>&1; \
         echo '--- ray'; docker ps -a --no-trunc --format '{{{{.ID}}}}|{{{{.Names}}}}|{{{{.Status}}}}|{{{{.Command}}}}' 2>&1 | grep -e 'ray start --address'; \
         echo '--- ollama'; curl -s -m 3 http://127.0.0.1:11434/api/ps 2>&1; true",
        sh_quote(image_ref(spec)),
        model_ready_test(&spec.model_path),
        caches.join(" ")
    )
}

/// The node has this folder, finished: a copy or download left partway still has its marker
/// (sparkjob's .sparkbox.partial). Used by the pre-flight and by a several-node load's check.
pub fn model_ready_test(path: &str) -> String {
    format!("test -f {} && test ! -e {}", sh_quote(&format!("{path}/config.json")), sh_quote(&format!("{path}/.sparkbox.partial")))
}

/// What a load says when `node` doesn't have a finished copy of the folder.
pub fn not_on_node(node: &str, path: &str) -> String {
    format!("{node} doesn't have {path} yet. Copy it to that node first (Library › Copy to nodes…).")
}

/// The sections of `preflight_script`'s output, in its order (a header out of order is a line
/// of the section before, so an earlyoom config can't start a section).
const PREFLIGHT_SECTIONS: [&str; 11] = ["mem", "earlyoom", "image", "store", "model", "port", "shm", "caches", "containers", "ray", "ollama"];

/// Each section in words, for a read that stopped there.
const PREFLIGHT_WORDS: [&str; PREFLIGHT_SECTIONS.len()] = ["the memory", "earlyoom", "the image", "Docker's image store", "the model folder", "the ports", "/dev/shm", "the cache folders", "the containers", "Ray workers", "Ollama"];

impl Preflight {
    /// The read stopped at this section. From there on nothing is known, so this is all it says:
    /// neither "free" nor "not on the node" would be true.
    fn cut_off_at(&mut self, section: usize) {
        self.cut_off = true;
        self.problems = vec![format!("Couldn't finish reading {}: it stopped at {}, and nothing from there on was checked. Check again.", self.node, PREFLIGHT_WORDS[section])];
    }
}

/// `preflight_script`'s output as its read ended. The script ends in `true`, so a read that
/// finished exits 0. Anything else stopped partway, and only the exit can say so once the last
/// header is out (curl can sit there for 3 s).
pub fn read_preflight(node: &str, spec: &DeploySpec, out: &str, code: Option<u32>) -> Preflight {
    let mut p = parse_preflight(node, spec, out);
    if code != Some(0) && !p.cut_off {
        p.cut_off_at(PREFLIGHT_SECTIONS.len() - 1);
    }
    p
}

/// Reads `preflight_script`'s output for `spec` on `node`. Output that stops before the last
/// section is a read cut off, never a section with nothing to report.
pub fn parse_preflight(node: &str, spec: &DeploySpec, out: &str) -> Preflight {
    let mut sections: Vec<Vec<&str>> = vec![Vec::new(); PREFLIGHT_SECTIONS.len()];
    let mut at: Option<usize> = None;
    for line in out.lines() {
        let next = at.map_or(0, |i| i + 1);
        if PREFLIGHT_SECTIONS.get(next).is_some_and(|s| line.trim_end() == format!("--- {s}")) {
            at = Some(next);
        } else if let Some(i) = at {
            sections[i].push(line);
        }
    }
    let sec = |name: &str| -> &[&str] { PREFLIGHT_SECTIONS.iter().position(|s| *s == name).map(|i| sections[i].as_slice()).unwrap_or(&[]) };
    let mut p = Preflight { node: node.into(), image_ref: image_ref(spec).into(), port_free: true, ..Default::default() };

    let kb = |key: &str| sec("mem").iter().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()).map(|k| k * 1024);
    p.mem = match (kb("MemTotal"), kb("MemFree"), kb("MemAvailable"), kb("Cached")) {
        (Some(total), Some(free), Some(available), Some(cached)) => Some(Mem { total, free, available, cached }),
        _ => None,
    };
    match &p.mem {
        Some(m) => {
            p.need_free = (m.total as f64 * spec.gpu_memory_utilization) as u64 + FREE_HEADROOM;
            p.drop_needed = m.free < p.need_free;
        }
        None => p.problems.push(format!("Couldn't read {node}'s memory (/proc/meminfo).")),
    }

    if let Some((state, rest)) = sec("earlyoom").split_first() {
        let config = rest.iter().map(|l| l.trim()).find(|l| l.starts_with("EARLYOOM_ARGS=")).map(String::from);
        p.earlyoom = Some(Earlyoom { active: state.trim() == "active", state: state.trim().into(), config });
    }

    let image = sec("image").iter().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("");
    let r = image_ref(spec);
    if is_image_id(image) {
        p.image_id = Some(image.into());
    } else if image.contains("No such image") || image.is_empty() {
        p.problems.push(format!("{node} doesn't have {r}. Pull it first (Fleet: docker pull {r}), or pick an image that's there."));
    } else {
        p.problems.push(format!("{node}: couldn't ask Docker about {r}: {image}"));
    }

    p.model_present = sec("model").iter().any(|l| l.trim() == "present");
    if !p.model_present {
        p.problems.push(not_on_node(node, &spec.model_path));
    }

    p.store = sec("store").iter().find_map(|l| store_name(l));

    // A worker doesn't bind the API port, so what listens there is no concern of this load's;
    // every rank needs the rendezvous port clear.
    let serves = spec.nodes.first().is_none_or(|h| h == node);
    for l in sec("port").iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        let addr = l.split_whitespace().nth(3).unwrap_or(l);
        let port = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(spec.port);
        let by = format!("listening on {addr}");
        if port == spec.port {
            if serves {
                p.port_free = false;
                p.problems.push(PreflightError::PortInUse { node: node.into(), port, by: by.clone() }.to_string());
                p.port_owner = Some(by);
            }
        } else if port == MASTER_PORT {
            p.port_free = false;
            p.problems.push(master_port_taken(node, &by));
            p.port_owner = Some(by);
        }
    }

    p.shm = shm_listing(sec("shm")).into_iter().map(|(name, _)| name).collect();

    p.caches = CACHE_DIRS
        .iter()
        .map(|(d, _)| {
            let line = sec("caches").iter().find(|l| l.trim_end().ends_with(&format!("/{d}")) || l.contains(&format!("/{d}'")));
            let listed = line.filter(|l| !l.starts_with("ls:"));
            CacheDir { path: format!("~/{d}"), exists: listed.is_some(), owner: listed.and_then(|l| l.split_whitespace().nth(2)).map(String::from) }
        })
        .collect();

    // Split on |: an empty label would collapse spaces.
    p.leftovers = sec("containers")
        .iter()
        .filter_map(|l| {
            let w: Vec<&str> = l.trim().splitn(5, '|').collect();
            let [id, name, deployment, instance, status] = w[..] else { return None };
            Some(Leftover { id: id.into(), name: name.into(), deployment: deployment.into(), instance: instance.into(), status: status.into(), ours: instance == instance_name() })
        })
        .collect();

    p.ray_leftovers = sec("ray")
        .iter()
        .filter_map(|l| {
            let w: Vec<&str> = l.trim().splitn(4, '|').collect();
            let [id, name, status, _] = w[..] else { return None };
            Some(Leftover { id: id.into(), name: name.into(), status: status.into(), ..Default::default() })
        })
        .collect();
    for r in p.ray_leftovers.iter().filter(|r| r.status.starts_with("Up")) {
        p.problems.push(ray_worker_left(node, &r.name));
    }

    p.ollama_resident = ollama_models(&sec("ollama").join("\n"));
    if !p.ollama_resident.is_empty() {
        p.problems.push(ollama_holds(node, &p.ollama_resident.join(", ")));
    }
    if at != Some(PREFLIGHT_SECTIONS.len() - 1) {
        p.cut_off_at(at.unwrap_or(0));
    }
    p
}

/// `docker info`'s `{{.Driver}} {{json .DriverStatus}}`. The containerd image store reports its
/// snapshotter as the driver and says so in DriverStatus; anything Docker prints that isn't that
/// shape (an error) is no answer.
fn store_name(line: &str) -> Option<String> {
    let (driver, status) = line.trim().split_once(' ')?;
    let status: Value = serde_json::from_str(status).ok()?;
    let containerd = status.as_array().is_some_and(|pairs| pairs.iter().any(|pair| pair[0] == "driver-type" && pair[1].as_str().is_some_and(|t| t.starts_with("io.containerd.snapshotter"))));
    Some(if containerd { "containerd".to_string() } else { driver.to_string() })
}

/// The models in Ollama's /api/ps. Empty when it holds none or isn't there to answer.
fn ollama_models(out: &str) -> Vec<String> {
    let Ok(ps) = serde_json::from_str::<Value>(out.trim()) else { return Vec::new() };
    ps["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().or(m["model"].as_str())).map(String::from).collect()
}

// ---- The refusals (C-W): each sentence once, here, whoever finds the problem

/// `Different Docker image stores on <a> and <b>; …`
pub fn different_stores(a: &str, b: &str) -> String {
    format!("Different Docker image stores on {a} and {b}; pull the same tag on each node.")
}

/// "abcdef012345": the twelve digits Docker shows.
fn short_id(id: &str) -> &str {
    let hex = id.strip_prefix("sha256:").unwrap_or(id);
    &hex[..hex.len().min(12)]
}

/// `<ref> isn't the same image on every node: …`, with two nodes' ids.
pub fn different_images(image: &str, a: &str, a_id: &str, b: &str, b_id: &str) -> String {
    format!("{image} isn't the same image on every node: {a} has {}, {b} has {}. Pull the same tag on each node.", short_id(a_id), short_id(b_id))
}

pub fn ollama_holds(node: &str, model: &str) -> String {
    format!("{node} has {model} in memory in Ollama, and unified memory is shared. Unload it there first.")
}

pub fn leftovers_block(node: &str, names: &str) -> String {
    format!("{node} has containers left from an earlier load ({names}). Remove them first.")
}

pub fn ray_worker_left(node: &str, name: &str) -> String {
    format!("{node} has a Ray worker still looking for its head ({name}). SparkBox didn't start it; remove it in Containers.")
}

pub fn jumbo_failed(node: &str, head: &str, iface: &str) -> String {
    format!("The jumbo-frame path from {node} to {head} failed on {iface}: the model's traffic can't cross the fabric.")
}

pub fn files_differ(node: &str, count: usize, first: &str) -> String {
    format!("The model's files differ on {node}: {count} files differ, first {first}.")
}

fn master_port_taken(node: &str, by: &str) -> String {
    format!("Port {MASTER_PORT}, where a several-node load's ranks meet, is already in use on {node} ({by}). Free it first.")
}

impl PreflightSet {
    /// One verdict for the chosen nodes: each node's own problems, memory it's short of with no
    /// consent to drop the page cache, whether every worker shares the head's fabric subnets, then
    /// whether every node holds the same image. Ids are compared only within one image store
    /// (P2.5): across stores the same image reads differently.
    pub fn judge(spec: &DeploySpec, nodes: Vec<Preflight>, checked_at_ms: u64) -> PreflightSet {
        let versions = (nodes.len() > 1).then(|| {
            let facts: Vec<_> = nodes.iter().map(|p| {
                let mut values = p.versions.clone();
                values.insert("imageId".into(), p.image_id.clone());
                values.insert("store".into(), p.store.clone());
                crate::versions::Facts { node: p.node.clone(), name: p.node.clone(), unavailable: p.cut_off.then(|| "Read did not finish".into()), values, ..Default::default() }
            }).collect();
            crate::versions::compare(&facts, false)
        });
        if nodes.iter().any(|p| p.demo) {
            return PreflightSet { versions, nodes, verdict: "demo".into(), problems: Vec::new(), image_id: None, checked_at_ms };
        }
        let mut problems: Vec<String> = nodes.iter().flat_map(|p| p.problems.iter().cloned()).collect();
        problems.extend(nodes.iter().filter(|p| p.drop_needed && !spec.drop_caches).map(|p| short_of_memory(p, spec.gpu_memory_utilization, false)));
        // A node with no fabric facts has its own problem already.
        if let Some((head, workers)) = nodes.split_first().filter(|(h, _)| !h.fabric.is_empty()) {
            for w in workers.iter().filter(|w| !w.fabric.is_empty()) {
                problems.extend(unpaired_half(&w.fabric, &head.fabric).map(|iface| jumbo_failed(&w.node, &head.node, &iface)));
            }
        }
        let mut image_id = None;
        if let Some(first) = nodes.first() {
            let unread = nodes.iter().find(|p| nodes.len() > 1 && p.store.is_none());
            let other_store = nodes.iter().find(|p| p.store != first.store);
            let other_image = nodes.iter().find(|p| p.image_id.is_some() && first.image_id.is_some() && p.image_id != first.image_id);
            if let Some(p) = unread {
                problems.push(format!("Couldn't read Docker's image store on {}, so its image can't be compared.", p.node));
            } else if let Some(p) = other_store.filter(|_| nodes.len() > 1) {
                problems.push(different_stores(&first.node, &p.node));
            } else if let Some(p) = other_image {
                problems.push(different_images(&first.image_ref, &first.node, first.image_id.as_deref().unwrap_or(""), &p.node, p.image_id.as_deref().unwrap_or("")));
            } else if nodes.iter().all(|p| p.image_id.is_some()) {
                image_id = first.image_id.clone();
            }
        }
        let verdict = if problems.is_empty() { "ready" } else { "blocked" };
        PreflightSet { versions, nodes, verdict: verdict.into(), problems, image_id, checked_at_ms }
    }
}

// ---- The jumbo path (C-J)

const JUMBO: &str = "ping -c 2 -W 2 -M do -s 8972";

/// One ping per half: `(local iface, the head's address on that half)`, each half reading ok or
/// failed. A pair a shell would read something into is never sent, and reads as failed.
pub fn jumbo_script(ifaces: &[(String, String)]) -> String {
    let lines: Vec<String> = ifaces
        .iter()
        .enumerate()
        .map(|(i, (iface, addr))| {
            let n = i + 1;
            if plain_name(iface) && plain_address(addr) {
                format!("{JUMBO} -I {iface} {addr} >/dev/null 2>&1 && echo half{n} ok || echo half{n} failed")
            } else {
                format!("echo half{n} failed")
            }
        })
        .collect();
    lines.join("; ")
}

/// The halves of `ifaces` that `jumbo_script`'s output doesn't say are ok, by position.
pub fn jumbo_failures(out: &str, halves: usize) -> Vec<usize> {
    (0..halves).filter(|i| !out.lines().any(|l| l.trim() == format!("half{} ok", i + 1))).collect()
}

fn fabric_subnet(f: &FabricFact) -> Option<(u32, u32)> {
    let ip: std::net::Ipv4Addr = f.ip.parse().ok()?;
    (1..=32).contains(&f.prefix).then(|| (u32::from(ip) >> (32 - f.prefix), f.prefix))
}

/// What a worker pings: each half it has, against the head's address on the same subnet.
pub fn jumbo_pairs(own: &[FabricFact], head: &[FabricFact]) -> Vec<(String, String)> {
    own.iter().filter_map(|f| head.iter().find(|h| fabric_subnet(h).is_some() && fabric_subnet(h) == fabric_subnet(f)).map(|h| (f.iface.clone(), h.ip.clone()))).collect()
}

/// An interface on a subnet only one side has. `jumbo_pairs` has no ping to send for it, and the
/// ranks wouldn't refuse: a worker off the head's first half waits for `--master-addr`, and
/// NCCL_IB_HCA lists every half.
pub fn unpaired_half(own: &[FabricFact], head: &[FabricFact]) -> Option<String> {
    let alone = |side: &[FabricFact], other: &[FabricFact]| side.iter().find(|f| fabric_subnet(f).is_none_or(|s| !other.iter().any(|o| fabric_subnet(o) == Some(s)))).map(|f| f.iface.clone());
    alone(own, head).or_else(|| alone(head, own))
}

/// The pre-flight of an Edit & reload, read while the load it replaces (`name`, on `port` at
/// `share`) still serves: its share comes back at the unload, so it counts as free before the
/// drop is judged, and the listeners on its own port and the rendezvous port are its own. vLLM's
/// process outside the share comes back too, so this errs short. deploy() reads the node again
/// after the unload.
pub fn count_replaced(p: &mut Preflight, spec: &DeploySpec, name: &str, port: u16, share: f64) {
    p.replacing = Some(name.to_string());
    if let Some(m) = &p.mem {
        p.freed = (m.total as f64 * share) as u64;
        p.drop_needed = m.free + p.freed < p.need_free;
    }
    if port == spec.port && !p.port_free {
        // `port_owner` is whichever listener ss printed last, so go by the sentence's start.
        let own = format!("Port {port} is already in use on {} (", p.node);
        p.problems.retain(|x| !x.starts_with(&own));
        p.port_owner = Some(format!("{name}, the load being replaced; free once it's unloaded"));
    }
    let rendezvous = format!("Port {MASTER_PORT}, ");
    p.problems.retain(|x| !x.starts_with(&rendezvous));
}

/// `ls -la /dev/shm` as (name, size in bytes), without `.`, `..` and the total line.
pub fn shm_listing(lines: &[&str]) -> Vec<(String, u64)> {
    lines
        .iter()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() < 9 || w[0] == "total" {
                return None;
            }
            let name = w[8..].join(" ");
            let name = if w[0].starts_with('l') { name.split(" -> ").next().unwrap_or("").to_string() } else { name };
            (name != "." && name != "..").then(|| (name, w[4].parse().unwrap_or(0)))
        })
        .collect()
}

/// "41.2 GiB".
fn gib_text(b: u64) -> String {
    format!("{:.1} GiB", b as f64 / GIB)
}

/// Why a load can't start for want of free memory: what the node has, what vLLM at this share
/// wants. `after_drop`: the page cache was dropped already.
pub fn short_of_memory(p: &Preflight, share: f64, after_drop: bool) -> String {
    let free = p.mem.as_ref().map(|m| m.free).unwrap_or(0);
    let figures = format!(
        "{} has {} free{}; vLLM at share {share:.2} needs {} free (4 GiB of it headroom).",
        p.node,
        gib_text(free),
        if after_drop { " after dropping the page cache" } else { "" },
        gib_text(p.need_free)
    );
    if after_drop {
        format!("{figures} Lower the share or free memory.")
    } else {
        format!("{figures} Tick 'Drop the page cache first' to let SparkBox free it (sudo; the kernel refills it).")
    }
}

/// `short_of_memory` for a restart: manual confirmation can grant one attempt's cache drop.
/// Read before the stop, `free` counts what the load gives back (`count_replaced`).
pub fn restart_short_of_memory(p: &Preflight, share: f64) -> String {
    let free = p.mem.as_ref().map(|m| m.free + p.freed).unwrap_or(0);
    let when = if p.replacing.is_some() { " once this load stops" } else { "" };
    format!(
        "{} has {} free{when}; vLLM at share {share:.2} needs {} free (4 GiB of it headroom). Click Restart and choose 'Restart and allow cache drop' to permit it for this attempt (sudo; the kernel refills it).",
        p.node,
        gib_text(free),
        gib_text(p.need_free)
    )
}

/// The page-cache drop (C-D), run with sudo and only with the owner's consent.
pub const DROP_CACHES: &str = "sync; echo 3 > /proc/sys/vm/drop_caches";

// ---- Pre-flight: port and memory

/// Why a load can't start (or needs a confirmation).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PreflightError {
    PortInUse { node: String, port: u16, by: String },
    Memory { node: String, need: u64, free: u64, reason: String },
}

impl std::fmt::Display for PreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreflightError::PortInUse { node, port, by } => {
                write!(f, "Port {port} is already in use on {node} ({by}). Pick another port.")
            }
            PreflightError::Memory { node, reason, .. } => {
                write!(f, "The memory plan for {node} is red: {reason} Or override the guardrail.")
            }
        }
    }
}

pub const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// What's listening on `port` on a node, from the snapshot.
pub fn port_owner(node: &NodeSnapshot, port: u16) -> Option<String> {
    if let Some(l) = node.listeners.iter().find(|l| l.port == port) {
        return Some(l.process.clone().unwrap_or_else(|| format!("listening on {}", l.addr)));
    }
    node.containers
        .iter()
        .filter(|c| c.state == "running")
        .find(|c| c.ports.iter().any(|p| p.split("->").next().and_then(|h| h.rsplit(':').next()) == Some(&port.to_string())))
        .map(|c| format!("container {}", c.name))
}

/// The first port from 8000 up that nothing on the node listens on (skipping the ranks' own).
pub fn first_free_port(node: &NodeSnapshot, taken: &[u16]) -> u16 {
    (FIRST_PORT..u16::MAX)
        .find(|p| *p != MASTER_PORT && !taken.contains(p) && port_owner(node, *p).is_none())
        .unwrap_or(FIRST_PORT)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Green,
    Amber,
    Red,
}

/// Something already holding GPU memory on a node.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub name: String,
    pub bytes: u64,
}

/// One node's share of a load against what that node has free now.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePlan {
    pub node: String,
    pub weights: u64,
    pub kv: u64,
    pub overhead: u64,
    pub need: u64,
    /// total × GPU memory share: what vLLM reserves when it starts.
    pub budget: u64,
    /// What this load can use: the budget, less what other GPU processes hold.
    pub free: u64,
    /// Unified memory not held by other GPU processes or the OS.
    pub available: u64,
    pub total: u64,
    pub holders: Vec<Holder>,
    pub level: Level,
    /// "Won't start", "Won't fit", "Won't start safely", "Tight" or "Fits".
    pub verdict: String,
    /// What the node already uses (other processes and the OS: the snapshot's used memory), before the share.
    pub in_use: u64,
    /// vLLM's own process outside its share (VLLM_HOST_MEMORY), on top of `in_use`.
    pub host_runtime: u64,
    /// The node's memory once vLLM has reserved its share, as a percentage of all of it.
    pub after_pct: f64,
    /// The largest GPU memory share vLLM's startup check would accept here now.
    pub max_share: f64,
    /// The largest share, in steps of 0.05, that keeps the node under the 90% memory planning
    /// line once vLLM has reserved it (never more than `max_share`). The one number the
    /// reason and the Load page's button both quote.
    pub safe_share: f64,
    /// Whether the model (with its context and requests at once) fits in `safe_share`.
    pub fits_safe: bool,
    /// When it doesn't: the longest context (in steps of 1,024 tokens) that would fit in
    /// `safe_share` with the requests at once chosen, if any does.
    pub safe_context: Option<u64>,
    /// Why it's red or amber, leading with the blocker.
    pub reason: Option<String>,
}

/// Projected memory at or above this is red: a GB10 that runs out of unified memory hangs.
pub const RED_PCT: f64 = 95.0;

/// What DGX OS itself keeps (kernel, desktop, page cache it won't give back).
pub const OS_RESERVE: u64 = 4 * 1024 * 1024 * 1024;

/// vLLM's own process memory outside its GPU share (Python, the CUDA libraries).
pub const VLLM_HOST_MEMORY: u64 = 9_400_000_000;

/// What the model needs, independent of where it runs.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelNeeds {
    pub weight_bytes: u64,
    pub arch: ArchFacts,
    pub kv_bytes_per_token: Option<u64>,
}

/// How many full-length requests a share sized to the model holds when Requests at once is empty.
pub const SIZED_REQUESTS: u64 = 4;
/// What a sized share adds to the bytes it counts: the runtime figure is an estimate.
pub const SIZED_HEADROOM: f64 = 1.1;

/// KV for `max_num_seqs` full-length requests (one when unset: the least vLLM starts with).
pub fn kv_need(spec: &DeploySpec, m: &ModelNeeds) -> u64 {
    let Some(per) = m.kv_bytes_per_token else { return 0 };
    let len = spec.max_model_len.or(m.arch.max_context).unwrap_or(8192);
    let seqs = spec.max_num_seqs.unwrap_or(1);
    let bytes = fit::request_bytes(&m.arch, per, len, spec.kv_cache_dtype.as_deref());
    (bytes * seqs as f64) as u64
}

/// How many ways a load's KV cache divides across its nodes.
fn kv_ways(spec: &DeploySpec, m: &ModelNeeds) -> u64 {
    let nodes = (spec.nodes.len() as u64 / spec.replicas.max(1)).max(1);
    let tp_ok = fit::tensor_parallel_ok(&m.arch, spec.tensor_parallel);
    fit::kv_shards(&m.arch, nodes, tp_ok && spec.pipeline_parallel == 1).max(1)
}

/// Per-node memory plan: weights/node + KV/node + overhead against what vLLM can have there now.
/// Severity comes from where the node ends up once vLLM has reserved its share, co-tenant or
/// not: at 95% or more it's red (it won't start safely), from 90% amber (it runs above the
/// alert line). It's also red when vLLM's startup check would refuse the share, or when the
/// model doesn't fit in it.
pub fn memory_plan(spec: &DeploySpec, m: &ModelNeeds, nodes: &[&NodeSnapshot], own_containers: &[String]) -> Vec<NodePlan> {
    let n = (spec.nodes.len() as u64 / spec.replicas.max(1)).max(1);
    let shards = kv_ways(spec, m);
    let kv = kv_need(spec, m) / shards;
    let weights = m.weight_bytes / n;
    let alert = crate::health::rules::MEM_WARN_PCT;
    spec.nodes
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let snap = nodes.iter().find(|s| s.id == *id);
            let total = snap.map(|s| s.mem.total).filter(|t| *t > 0).unwrap_or(fit::DEFAULT_NODE_MEMORY);
            let mut holders: Vec<Holder> = Vec::new();
            for p in snap.map(|s| s.gpu.procs.as_slice()).unwrap_or_default() {
                if p.container.as_ref().is_some_and(|c| own_containers.contains(c)) {
                    continue;
                }
                let name = p.container.clone().unwrap_or_else(|| p.name.clone());
                match holders.iter_mut().find(|h| h.name == name) {
                    Some(h) => h.bytes += p.mem,
                    None => holders.push(Holder { name, bytes: p.mem }),
                }
            }
            holders.sort_by(|a, b| b.bytes.cmp(&a.bytes));
            let used: u64 = holders.iter().map(|h| h.bytes).sum();
            let share = spec.gpu_memory_utilization;
            let budget = (total as f64 * share) as u64;
            let available = total.saturating_sub(used).saturating_sub(OS_RESERVE);
            // vLLM takes its share of *total* memory for itself, and refuses to start unless
            // that much is free right now: other GPU users count against the start, not the share.
            let free = budget.min(available);
            let vision = if (i as u64).is_multiple_of(n) { m.arch.vision_bytes } else { 0 };
            let overhead = fit::RUNTIME_OVERHEAD as u64 + vision;
            let need = weights + kv + overhead;
            let max_share = ((available as f64 / total as f64) * 20.0).floor() / 20.0;
            // Everything in use now (this load's own containers aside), or the holders and the OS
            // when there's no reading, plus vLLM's own process; its whole share comes on top of that.
            let own: u64 = snap
                .map(|s| s.gpu.procs.iter().filter(|p| p.container.as_ref().is_some_and(|c| own_containers.contains(c))).map(|p| p.mem).sum())
                .unwrap_or(0);
            let in_use = snap.map(|s| s.mem.used.saturating_sub(own)).filter(|u| *u > 0).unwrap_or(used + OS_RESERVE);
            let before = in_use + VLLM_HOST_MEMORY;
            let after_pct = 100.0 * (before + budget) as f64 / total as f64;
            // Under the alert line means strictly below it: step down when the room is an exact multiple.
            let room = (alert / 100.0 * total as f64 - before as f64) / total as f64;
            let mut safe_share = (room * 20.0 - 1e-9).floor() / 20.0;
            safe_share = (safe_share.min(max_share).max(0.0) * 100.0).round() / 100.0;
            // Fits means room to spare: a share the model fills to the byte leaves the KV cache
            // nothing beyond the requests it was counted for.
            let fits_safe = (need as f64) < total as f64 * safe_share;
            // What context would fit in the safe share instead: the KV room it leaves, per request,
            // over the bytes per token (this node's share of them).
            let safe_context = (!fits_safe).then(|| {
                let per_token = m.kv_bytes_per_token?;
                let seqs = spec.max_num_seqs.unwrap_or(1) as f64;
                let max = spec.max_model_len.or(m.arch.max_context).unwrap_or(u64::MAX);
                let room = (total as f64 * safe_share - weights as f64 - overhead as f64) * shards as f64 / seqs;
                let tokens = fit::context_capacity(&m.arch, per_token, room, spec.kv_cache_dtype.as_deref(), max) / 1024 * 1024;
                (tokens >= 4096).then_some(tokens.min(max))
            }).flatten();
            let gib = |b: u64| b as f64 / GIB;
            let (level, verdict, reason) = if budget > available {
                let why = format!("Won't start: {} reserves {:.1} GiB ({share:.2} share) at startup; {:.1} GiB is free.", super::engine_label(spec), gib(budget), gib(available));
                (Level::Red, "Won't start", Some(why))
            } else if need >= budget {
                let why = format!(
                    "Won't fit: needs {:.1} GiB, but a {share:.2} share gives vLLM {:.1} GiB. Lower the context or requests at once, or use more nodes.",
                    gib(need),
                    gib(budget)
                );
                (Level::Red, "Won't fit", Some(why))
            } else if after_pct >= RED_PCT {
                let why = format!("Won't start safely: with this load {id} would use {after_pct:.0}% of its memory, and a Spark that runs out hangs.");
                (Level::Red, "Won't start safely", Some(why))
            } else if after_pct >= alert {
                let why = format!("Tight: runs above the {alert:.0}% alert line ({after_pct:.0}% of {id}'s memory).");
                (Level::Amber, "Tight", Some(why))
            } else {
                (Level::Green, "Fits", None)
            };
            NodePlan {
                node: id.clone(),
                weights,
                kv,
                overhead,
                need,
                budget,
                free,
                available,
                total,
                holders,
                level,
                verdict: verdict.into(),
                in_use,
                host_runtime: VLLM_HOST_MEMORY,
                after_pct,
                max_share,
                safe_share,
                fits_safe,
                safe_context,
                reason,
            }
        })
        .collect()
}

/// The share sized to this model on these nodes: the least, in steps of 0.05, that holds its
/// weights, its runtime and the KV cache for `SIZED_REQUESTS` full-length requests (or as many as
/// Requests at once says) on every node, with SIZED_HEADROOM on top. vLLM reserves the whole share
/// and hands what's left to the KV cache, so anything larger only shuts the page cache out.
///
/// It doesn't depend on the share in `spec`, so the Load page can follow it without chasing itself.
/// It's held under `safe_share` as long as the model still fits there, and None hands the load back
/// to FIRST_LOAD_SHARE and `suggested_share`: when the KV's size isn't known, when no context is
/// chosen and the config doesn't give the model's (vLLM finds its own, and the plan's 8,192 is
/// only a guess), when the model is too big to come in below FIRST_LOAD_SHARE, and when no share
/// that fits holds it.
pub fn sized_share(spec: &DeploySpec, m: &ModelNeeds, plan: &[NodePlan]) -> Option<f64> {
    m.kv_bytes_per_token?;
    spec.max_model_len.or(m.arch.max_context)?;
    if plan.is_empty() {
        return None;
    }
    let requests = DeploySpec { max_num_seqs: Some(spec.max_num_seqs.unwrap_or(SIZED_REQUESTS)), ..spec.clone() };
    let kv = (kv_need(&requests, m) / kv_ways(spec, m)) as f64;
    let wanted = plan.iter().map(|p| ((p.weights + p.overhead) as f64 + kv) * SIZED_HEADROOM / p.total as f64).fold(0.0, f64::max);
    let sized = ((wanted * 20.0 - 1e-9).ceil() / 20.0).max(fit::MIN_SHARE);
    if sized > fit::FIRST_LOAD_SHARE {
        return None;
    }
    let share = plan.iter().map(|p| p.safe_share).fold(sized, f64::min);
    (share >= fit::MIN_SHARE && plan.iter().all(|p| (p.need as f64) < p.total as f64 * share)).then_some(share)
}

/// How many full-length requests the KV cache holds at the share chosen: what the share leaves
/// after the weights and the runtime, over one request's KV. A load serves what its tightest node
/// holds. None when a request's size isn't known.
pub fn full_requests(spec: &DeploySpec, plan: &[NodePlan]) -> Option<u64> {
    let seqs = spec.max_num_seqs.unwrap_or(1) as f64;
    let each = plan.iter().map(|p| (p.kv > 0).then(|| (p.budget.saturating_sub(p.weights + p.overhead) as f64 / (p.kv as f64 / seqs)) as u64));
    each.collect::<Option<Vec<_>>>()?.into_iter().min()
}

/// The one share to offer for a whole plan: the largest every node can take and stay under the
/// alert line, when the model fits in it there and it's lower than the share chosen.
pub fn suggested_share(spec: &DeploySpec, plan: &[NodePlan]) -> Option<f64> {
    if plan.is_empty() || plan.iter().all(|p| p.level == Level::Green) {
        return None;
    }
    let safe = plan.iter().map(|p| p.safe_share).fold(f64::INFINITY, f64::min);
    let fits = plan.iter().all(|p| (p.need as f64) < p.total as f64 * safe);
    (fits && safe >= fit::MIN_SHARE && safe < spec.gpu_memory_utilization - 1e-9).then_some(safe)
}

/// When no share fits the context asked for: the safe share with the longest context that fits
/// on every node at it ("Use 0.40 and a 96k context"). Only when every node has the same safe
/// share, since each node's context is worked out at its own.
pub fn suggested_context(spec: &DeploySpec, plan: &[NodePlan]) -> Option<(f64, u64)> {
    if plan.is_empty() || plan.iter().all(|p| p.level == Level::Green) || suggested_share(spec, plan).is_some() {
        return None;
    }
    let safe = plan[0].safe_share;
    if safe < fit::MIN_SHARE || plan.iter().any(|p| (p.safe_share - safe).abs() > 1e-9) {
        return None;
    }
    let ctx = plan.iter().map(|p| if p.fits_safe { u64::MAX } else { p.safe_context.unwrap_or(0) }).min()?;
    (4096..u64::MAX).contains(&ctx).then_some((safe, ctx))
}

/// The pre-flight `deploy()` runs: the port is free on the head and no node's plan is red
/// (unless the user overrode the memory guardrail).
pub fn preflight(spec: &DeploySpec, plan: &[NodePlan], head: Option<&NodeSnapshot>) -> Result<(), PreflightError> {
    if let Some(by) = head.and_then(|h| port_owner(h, spec.port)) {
        return Err(PreflightError::PortInUse { node: spec.nodes[0].clone(), port: spec.port, by });
    }
    if !spec.override_memory {
        if let Some(p) = plan.iter().find(|p| p.level == Level::Red) {
            return Err(PreflightError::Memory { node: p.node.clone(), need: p.need, free: p.free, reason: p.reason.clone().unwrap_or_default() });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Stage {
    Starting,
    /// `have` ranks of `need` running.
    Joining { have: u64, need: u64 },
    Loading { percent: u64 },
    /// A long compile, which reads as progress rather than a hang (P2.8).
    Compiling { what: String },
    Warming,
    Ready,
}

/// Where vLLM is in its startup, from the container log: the newest line that says (C-ST). The
/// compile words come from research, not a capture, so R1-WATCH also counts any new line as life.
pub fn stage_from_logs(logs: &str, healthy: bool) -> Stage {
    if healthy {
        return Stage::Ready;
    }
    logs.lines().rev().find_map(line_stage).unwrap_or(Stage::Starting)
}

fn line_stage(raw: &str) -> Option<Stage> {
    let compiling = |what: &str| Some(Stage::Compiling { what: what.into() });
    let line = raw.to_lowercase();
    let has = |w: &str| line.contains(w);
    if has("application startup complete") || has("the server is fired up") {
        return Some(Stage::Ready);
    }
    if has("capturing cuda graph") {
        return Some(Stage::Warming);
    }
    // vLLM prints its settings when it starts, and a setting's name isn't a step it has reached.
    if !has("initializing a v1 llm engine") && !has("non-default args") {
        if (has("autotune") && !has("autotune=")) || has("no available shared memory broadcast block found") {
            return compiling("FlashInfer autotune");
        }
        if has("flashinfer") && ["jit", "compil", "build", "kernel"].iter().any(|w| has(w)) {
            return compiling("FlashInfer kernels");
        }
        if has("triton") && has("compil") {
            return compiling("Triton kernels");
        }
        if has("torch.compile") || has("compiling a graph") {
            return compiling("torch.compile");
        }
    }
    // tqdm redraws its bar with \r, so one line can hold every update, and the newest is the last.
    let shards = line.rmatch_indices("checkpoint shards:").find_map(|(i, _)| {
        let pct: String = line[i..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
        pct.parse().ok()
    });
    if let Some(p) = shards {
        return Some(Stage::Loading { percent: p });
    }
    if has("loading model from scratch") {
        return Some(Stage::Loading { percent: 0 });
    }
    let nums: Vec<u64> = line.strip_prefix("sparkbox: ")?.split_whitespace().filter_map(|w| w.parse().ok()).collect();
    (nums.len() == 2).then(|| Stage::Joining { have: nums[0], need: nums[1] })
}

/// What the head's log says about its traffic (C-T). Only the log's words decide, and these
/// needles come from the plan, not a capture; the real line is recorded at Session 1.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Transport {
    /// The RDMA devices NCCL says it's using.
    pub ib: Vec<String>,
    pub socket: bool,
    /// The address vLLM's message queue connects to.
    pub mq_ip: Option<String>,
}

/// The lines `transport_from_logs` reads, from the head's log.
pub fn transport_command(head: &str) -> String {
    format!("docker logs {head} 2>&1 | grep -m 12 -E 'NET/(IB|Socket)|mq_connect_ip='")
}

/// "NET/IB : Using [0]rocep1s0f1:1/RoCE [1]roceP2p1s0f1:1/RoCE [RO]; OOB …" gives both devices;
/// "NET/IB : No device found." gives none, and NCCL goes on with sockets.
pub fn transport_from_logs(logs: &str) -> Transport {
    let mut t = Transport::default();
    for line in logs.lines() {
        if let (Some(i), None) = (line.find("mq_connect_ip="), &t.mq_ip) {
            let ip: String = line[i + "mq_connect_ip=".len()..].chars().take_while(|c| c.is_ascii_hexdigit() || ".:".contains(*c)).collect();
            t.mq_ip = Some(ip).filter(|ip| !ip.is_empty());
        }
        if line.contains("NET/Socket") {
            t.socket = true;
        }
        if let Some(i) = line.find("NET/IB").filter(|_| line.contains("Using")) {
            for word in line[i..].split_whitespace() {
                let device = word.strip_prefix('[').and_then(|w| w.split_once(']')).and_then(|(_, d)| d.split_once(':'));
                if let Some((dev, _)) = device.filter(|(dev, _)| !t.ib.iter().any(|d| d == dev)) {
                    t.ib.push(dev.to_string());
                }
            }
        }
    }
    t
}

/// The warning a load carries when its log says traffic left the RDMA fabric. `fabric` is the
/// head's fabric addresses.
pub fn transport_warning(t: &Transport, fabric: &[String]) -> Option<String> {
    let mut why: Vec<String> = Vec::new();
    if let Some(ip) = t.mq_ip.as_ref().filter(|ip| !fabric.contains(ip)) {
        why.push(format!("its message queue is on {ip}, which isn't a fabric address"));
    }
    if t.socket && t.ib.is_empty() {
        why.push("NCCL is on sockets, with no RDMA device".into());
    }
    (!why.is_empty()).then(|| format!("It isn't using the RDMA fabric: {}.", why.join(", and ")))
}

/// A name no shell would read anything into: an interface or an RDMA device.
fn plain_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn plain_address(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>().is_ok()
}

/// The fabric interface to use: the lowest address. All ranks pick the same subnet because a set
/// whose subnets don't match is refused first (`unpaired_half`, in `PreflightSet::judge`). Its HCA
/// comes first in NCCL_IB_HCA, then the other half's. An interface whose name or address a shell
/// would read something into is left out.
pub fn pick_fabric(node_id: &str, fabric: &[(String, String, String)]) -> Option<NodeNet> {
    let usable: Vec<&(String, String, String)> = fabric.iter().filter(|(iface, ip, rdma)| plain_name(iface) && plain_address(ip) && (rdma.is_empty() || plain_name(rdma))).collect();
    let primary = usable.iter().min_by_key(|(_, ip, _)| ip.parse::<std::net::Ipv4Addr>().ok())?;
    let others = usable.iter().filter(|f| f.0 != primary.0).map(|f| f.2.clone());
    Some(NodeNet {
        node_id: node_id.into(),
        iface: primary.0.clone(),
        ip: primary.1.clone(),
        hcas: std::iter::once(primary.2.clone()).chain(others).filter(|r| !r.is_empty()).collect(),
    })
}

// ---- Which splits a model takes (P2.3)

/// One way to split a model across the chosen nodes. `ok` false: `reason` says why it won't work.
/// `ok` true with a `reason`: it can be chosen, and the reason is worth reading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyOffer {
    /// "single", "tp2", "pp3", "tp2pp2": what a recipe's `topologies` lists.
    pub id: String,
    pub tp: u64,
    pub pp: u64,
    #[serde(default = "one")]
    pub replicas: u64,
    pub nodes: u64,
    pub ok: bool,
    pub reason: Option<String>,
    pub advanced: bool,
    /// Refused only because the recipe doesn't list it: the model can take it (`Recipe::offers`).
    pub unlisted: bool,
}

/// The id a recipe's `topologies` knows a split by.
pub fn topology_id(tp: u64, pp: u64) -> String {
    match (tp, pp) {
        (1, 1) => "single".into(),
        (t, 1) => format!("tp{t}"),
        (1, p) => format!("pp{p}"),
        (t, p) => format!("tp{t}pp{p}"),
    }
}

const HEADS_UNKNOWN: &str = "This model's config doesn't say how many attention heads it has, so SparkBox can't check this split. vLLM should refuse a bad one when it starts, and SparkBox cleans up.";

/// Layers unknown (no config to read) isn't a refusal; vLLM should refuse when it starts.
fn pp_problem(a: &ArchFacts, pp: u64) -> Option<String> {
    (a.layers > 0 && pp > a.layers).then(|| format!("Pipeline parallel {pp} won't work: this model has {} layers.", a.layers))
}

fn tp_problem(a: &ArchFacts, tp: u64, nodes: u64) -> Option<String> {
    if a.heads == 0 || fit::tensor_parallel_ok(a, tp) {
        return None;
    }
    if !a.heads.is_multiple_of(tp) {
        let instead = if nodes > 1 && pp_problem(a, nodes).is_none() { format!(" Use pipeline parallel {nodes}.") } else { String::new() };
        return Some(format!("Tensor parallel {tp} won't work for this model: {} heads don't divide by {tp}.{instead}", a.heads));
    }
    Some(format!("Tensor parallel {tp} won't work for this model: {k} KV heads don't divide by {tp}, and {tp} isn't a multiple of them.", k = a.kv_heads))
}

/// Refuses a split the model can't take (C-W). A model whose heads aren't known passes: vLLM
/// should refuse when it starts, and rollback cleans up.
pub fn validate_topology(spec: &DeploySpec, arch: &ArchFacts) -> Result<(), String> {
    if spec.engine == "vllm" && arch.sparse_mla && fit::kv_dtype_factor(spec.kv_cache_dtype.as_deref()) >= 1.0 {
        return Err(fit::NO_KERNEL_GB10.into());
    }
    match tp_problem(arch, spec.tensor_parallel, spec.nodes.len() as u64).or_else(|| pp_problem(arch, spec.pipeline_parallel)) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// The splits to offer on `nodes` nodes, best first, including independent replicas.
pub fn topologies(arch: &ArchFacts, nodes: usize) -> Vec<TopologyOffer> {
    if nodes > 4 { return Vec::new(); }
    let n = nodes as u64;
    let offer = |tp: u64, pp: u64, advanced: Option<&str>| {
        let problem = tp_problem(arch, tp, n).or_else(|| pp_problem(arch, pp));
        let unknown = tp > 1 && arch.heads == 0;
        TopologyOffer {
            id: topology_id(tp, pp),
            tp,
            pp,
            replicas: 1,
            nodes: n,
            ok: problem.is_none(),
            reason: problem.or(advanced.map(String::from)).or(unknown.then(|| HEADS_UNKNOWN.to_string())),
            advanced: advanced.is_some(),
            unlisted: false,
        }
    };
    let mut offers = match n {
        0 => Vec::new(),
        1 => vec![offer(1, 1, None)],
        4 => vec![offer(4, 1, None), offer(1, 4, None), offer(2, 2, Some("Advanced: two pipeline stages of two nodes each."))],
        _ => vec![offer(n, 1, None), offer(1, n, None)],
    };
    if n > 1 {
        let mut replicas = offer(1, 1, None);
        replicas.id = format!("replicas{n}");
        replicas.replicas = n;
        offers.push(replicas);
        if n == 4 {
            let mut pairs = offer(2, 1, None);
            pairs.id = "replicas2tp2".into();
            pairs.replicas = 2;
            offers.push(pairs);
        }
    }
    offers
}

/// A group has a durable common label and distinct deployment/credential/container identities.
pub fn replica_specs(spec: &DeploySpec) -> Result<Vec<DeploySpec>, String> {
    validate(spec)?;
    if spec.replicas == 1 { return Ok(vec![spec.clone()]); }
    let ranks = (spec.tensor_parallel * spec.pipeline_parallel) as usize;
    spec.nodes.chunks(ranks).enumerate().map(|(i, nodes)| {
        let mut child = spec.clone();
        child.id = format!("{}-r{}", spec.id, i + 1);
        child.group = if spec.group.is_empty() { spec.id.clone() } else { spec.group.clone() };
        child.nodes = nodes.to_vec();
        child.replicas = 1;
        child.host.clear();
        child.image_id.clear();
        validate(&child)?;
        Ok(child)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(nodes: &[&str], tp: u64, pp: u64) -> DeploySpec {
        DeploySpec {
            id: "glm53".into(),
            served_name: "glm-5.3-flash".into(),
            model_path: "/home/sparkuser/models/nvidia/GLM-5.3-Flash-NVFP4".into(),
            nodes: nodes.iter().map(|n| n.to_string()).collect(),
            tensor_parallel: tp,
            pipeline_parallel: pp,
            expert_parallel: false,
            max_model_len: Some(131072),
            gpu_memory_utilization: 0.8,
            port: 8000,
            image: "nvcr.io/nvidia/vllm:26.05-py3".into(),
            extra_args: vec![],
            allow_remote_code: true,
            image_id: local_id(),
            ..Default::default()
        }
    }

    /// The contracts' local image id: sha256: and 64 × a.
    fn local_id() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    fn net(id: &str, n: u8) -> NodeNet {
        NodeNet { node_id: id.into(), iface: "enp1s0f1np1".into(), ip: format!("192.168.100.{n}"), hcas: vec!["rocep1s0f1".into(), "roceP2p1s0f1".into()] }
    }

    /// P1.2: every docker run line carries this instance's label, and the pre-clean and
    /// stop-role queries filter by it, so the other SparkBox's containers are never touched.
    #[test]
    fn every_docker_run_carries_the_instance_label_and_pre_clean_filters_by_it() {
        let mine = instance_name();
        assert_eq!(mine, "dev", "a debug build is the dev instance");
        let label = format!("--label sparkbox.instance={mine}");
        let single = single_command(&spec(&["cell1"], 1, 1));
        let head = rank_command(&spec(&["cell1", "cell2"], 2, 1), 0);
        let worker = rank_command(&spec(&["cell1", "cell2"], 2, 1), 1);
        let sg = sglang_command(&DeploySpec { engine: "sglang".into(), image: "lmsysorg/sglang:spark".into(), ..spec(&["cell1", "cell2"], 2, 1) }, 0, "192.168.100.1", Some(&net("cell1", 1)));
        for cmd in [&single, &head, &worker, &sg] {
            assert!(cmd.contains(&label), "every docker run carries the instance label: {cmd}");
        }
        // The pre-clean and stop-role queries only match this instance's containers.
        let clean = unload_command("glm53");
        assert!(clean.contains(&format!("--filter label=sparkbox.instance={mine}")), "pre-clean filters by instance: {clean}");
        assert!(clean.contains("--filter label=sparkbox.deployment=glm53"));
        assert!(stop_role_command("glm53", "worker").contains(&format!("--filter label=sparkbox.instance={mine}")));
        // A container the other instance labelled isn't matched (docker ANDs the filters).
        assert!(!clean.contains("sparkbox.instance=release"), "never the other instance: {clean}");
    }

    #[test]
    fn nodes_must_equal_tensor_times_pipeline() {
        assert!(validate(&spec(&["cell1", "cell2", "cell3", "gx10-0000"], 4, 1)).is_ok());
        assert!(validate(&spec(&["cell1", "cell2", "cell3", "gx10-0000"], 2, 2)).is_ok());
        assert!(validate(&spec(&["cell1", "cell2"], 4, 1)).unwrap_err().contains("one GPU per Spark"));
        let mut s = spec(&["cell1"], 1, 1);
        s.model_path = "relative/path".into();
        assert!(validate(&s).is_err());
        s = spec(&["cell1"], 1, 1);
        s.served_name = "a b".into();
        assert!(validate(&s).is_err());
        s = spec(&["cell1"], 1, 1);
        s.extra_args = vec!["--enforce-eager".into(), "--quantization".into(), "fp8".into()];
        assert!(validate(&s).is_ok());
        s.extra_args = vec!["--max-num-seqs".into(), "8".into()];
        assert!(validate(&s).unwrap_err().contains("set by SparkBox"), "requests at once is a field");
        s.extra_args = vec!["--distributed-executor-backend".into(), "ray".into()];
        assert!(validate(&s).unwrap_err().contains("set by SparkBox"), "no duplicate of a flag SparkBox sets");
        s.extra_args = vec!["--max-model-len=8192".into()];
        assert!(validate(&s).is_err());
    }

    fn expert(args: &[&str]) -> Result<(), String> {
        check_extra_args(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn expert_options_are_argv_words_read_the_way_vllm_reads_them() {
        let ok = expert;
        assert!(ok(&["--enforce-eager", "--quantization", "fp8", "--max-num-batched-tokens=8192", "-q", "awq", "--seed", "-1"]).is_ok());
        assert!(ok(&["--max_num_batched_tokens", "4096"]).is_ok(), "underscores are dashes");
        // No allowlist now: an option vLLM has that SparkBox never heard of goes through.
        assert!(ok(&["--a-new-option-of-vllms", "3", "--model-impl", "vllm"]).is_ok());
        // Every spelling of the dangerous ones is caught, abbreviations argparse would expand too.
        for bad in [&["--trust_remote_code"][..], &["--trust-remote-code=true"], &["---trust-remote-code"], &["--trust-remote"], &["--trust"], &["-trust-remote-code"]] {
            assert!(ok(bad).unwrap_err().contains("Allow remote code"), "{bad:?}");
        }
        for bad in ["--api_key", "--api-key", "--api-k", "--hf-token"] {
            assert!(ok(&[bad, "x"]).unwrap_err().contains("API key"), "{bad}");
        }
        for bad in [
            "--max_model_len=8192", "--tensor_parallel_size", "--max-model", "--served-model-name", "--host", "--port", "--nnodes", "--node-rank", "--master-addr",
            "--master-port", "--headless", "--distributed-executor-backend", "--no-enable-prefix-caching", "--reasoning-parser", "-tp", "-pp", "-n", "-r", "-n4", "-r1", "-tp2",
        ] {
            assert!(ok(&[bad, "2"]).unwrap_err().contains("set by SparkBox"), "{bad}");
        }
        // What loads code or opens files, or could carry options past this rule.
        for bad in [
            "--middleware", "--middle", "--tool-parser-plugin", "--tool-parser-pl", "--allowed-local-media-path", "--allowed-local", "--ssl-keyfile", "--ssl-certfile", "--ssl_ca_certs",
            "--ss", "--reasoning-parser-plugin", "--logits-processors", "--worker-cls", "--worker-extension-cls", "--scheduler-cls", "--config", "--conf",
            "--structured-outputs-config.reasoning_parser_plugin",
        ] {
            assert!(ok(&[bad, "x"]).unwrap_err().contains("loads code or opens files"), "{bad}");
            assert!(ok(&["--seed", "1", &format!("{bad}=x")]).is_err(), "{bad}=x");
        }
        assert!(ok(&["--config-format", "hf"]).is_ok(), "a longer name isn't its prefix");
        // Each element is one word: a stray value, a line break, a NUL, an empty one.
        assert!(ok(&["fp8"]).unwrap_err().contains("isn't an option"), "a stray value");
        assert!(ok(&["--seed", "1", "2"]).unwrap_err().contains("isn't an option"), "a second value");
        assert!(ok(&["-1"]).unwrap_err().contains("isn't an option"));
        assert!(ok(&["--"]).unwrap_err().contains("isn't an option"), "the end-of-options marker");
        assert!(ok(&["--quantization="]).unwrap_err().contains("needs a value"));
        assert!(ok(&["--seed 1"]).unwrap_err().contains("space in its name"));
        for bad in ["--seed\n1", "--a\0b", "a\rb", ""] {
            assert!(ok(&["--seed", bad]).unwrap_err().contains("line break"), "{bad:?}");
        }
        // Quotes and $ are plain text: no shell reads them.
        assert!(ok(&["--chat-template-content-format", "it's $(id) `x` \"y\""]).is_ok());
        let s = DeploySpec { extra_args: vec!["--seed".into(), "$(id)".into()], ..spec(&["cell1"], 1, 1) };
        assert!(validate(&s).is_ok(), "{:?}", validate(&s));
        assert!(rank_command(&s, 0).find("$(id)").is_none(), "and it never reaches a command line");
    }

    #[test]
    fn json_flags_are_validated_and_stay_one_word() {
        for flag in ["--speculative-config", "--hf-overrides", "--compilation-config", "--limit-mm-per-prompt", "--override-generation-config"] {
            assert!(expert(&[flag, r#"{"method": "mtp", "num_speculative_tokens": 5}"#]).is_ok(), "{flag}");
            assert!(expert(&[&format!("{flag}={{\"a\": 1}}")]).is_ok(), "{flag}");
            assert!(expert(&[flag, "{not json"]).unwrap_err().contains("takes JSON"), "{flag}");
            assert!(expert(&[&format!("{flag}={{nope")]).unwrap_err().contains("takes JSON"), "{flag}");
            assert!(expert(&[flag]).unwrap_err().contains("needs a value"), "{flag}");
        }
        assert!(expert(&["--hf-overrides.max_position_embeddings", "4096"]).is_ok(), "a part of it is a plain value");
        let json = r#"{"method":"mtp","num_speculative_tokens":5}"#;
        let s = DeploySpec { extra_args: vec!["--speculative-config".into(), json.into()], ..spec(&["cell1"], 1, 1) };
        assert!(validate(&s).is_ok());
        let argv = rank_argv(&s, 0, &[]);
        assert_eq!(argv.iter().position(|a| a == json), argv.iter().position(|a| a == "--speculative-config").map(|i| i + 1), "{argv:?}");
    }

    #[test]
    fn sglang_keeps_its_own_rule() {
        let s = DeploySpec { engine: "sglang".into(), require_key: false, extra_args: vec!["--seed".into(), "1".into()], ..spec(&["cell1"], 1, 1) };
        assert_eq!(validate(&s).unwrap_err(), "Expert options are for vLLM loads only.");
    }

    #[test]
    fn what_a_recipe_fills_in_is_checked_like_the_rest() {
        let mut s = spec(&["cell1"], 1, 1);
        s.recipe_args = vec!["--api-key".into(), "x".into()];
        assert!(validate(&s).unwrap_err().contains("API key"));
        s = spec(&["cell1"], 1, 1);
        s.env.insert("HF_TOKEN".into(), "x".into());
        assert!(validate(&s).unwrap_err().contains("secret"));
        s = spec(&["cell1"], 1, 1);
        s.docker_access = "root".into();
        assert!(validate(&s).unwrap_err().contains("standard or privileged"));
        for bad in ["a b", "a;b", "$(id)", "../x", "-x"] {
            assert!(validate(&DeploySpec { recipe: bad.into(), ..spec(&["cell1"], 1, 1) }).is_err(), "{bad}");
            assert!(validate(&DeploySpec { group: bad.into(), ..spec(&["cell1"], 1, 1) }).is_err(), "{bad}");
        }
        // A saved load from before recipes reads back.
        let old: DeploySpec = serde_json::from_value(json!({
            "id": "a", "servedName": "m", "modelPath": "/m", "nodes": ["cell1"], "tensorParallel": 1, "pipelineParallel": 1, "expertParallel": false,
            "maxModelLen": null, "gpuMemoryUtilization": 0.75, "port": 8000, "image": "i", "extraArgs": []
        }))
        .unwrap();
        assert!(old.recipe.is_empty() && old.env.is_empty() && old.recipe_args.is_empty() && old.docker_access.is_empty() && old.group.is_empty());
    }

    #[test]
    fn sglang_runs_one_launch_server_per_node_meeting_at_the_head() {
        let mut s = DeploySpec { engine: "sglang".into(), require_key: false, allow_remote_code: false, image: "lmsysorg/sglang:spark".into(), ..spec(&["cell1", "cell2"], 2, 1) };
        assert!(validate(&s).is_ok(), "{:?}", validate(&s));
        let head = sglang_command(&s, 0, "192.168.100.1", Some(&net("cell1", 1)));
        for needle in [
            "--name sparkbox-glm53-head", "--label sparkbox.role=head", "-e NCCL_SOCKET_IFNAME=enp1s0f1np1", " lmsysorg/sglang:spark 'python3' '-m' 'sglang.launch_server'",
            "'--tp' '2'", "'--nnodes' '2' '--node-rank' '0' '--dist-init-addr' '192.168.100.1:5000'", "'--mem-fraction-static' '0.80'", "'--enable-metrics'",
            "'--context-length' '131072'",
        ] {
            assert!(head.contains(needle), "missing {needle}\n{head}");
        }
        let worker = sglang_command(&s, 1, "192.168.100.1", Some(&net("cell2", 2)));
        assert!(worker.contains("--name sparkbox-glm53-cell2") && worker.contains("'--node-rank' '1'"), "{worker}");
        assert!(super::super::endpoints::parse_command(&sglang_args(&s, 1, "192.168.100.1").join(" ")).is_none(), "rank 1 isn't an endpoint");
        let p = super::super::endpoints::parse_command(&sglang_args(&s, 0, "192.168.100.1").join(" ")).unwrap();
        assert_eq!((p.engine, p.tp, p.port), (super::super::endpoints::Engine::Sglang, 2, Some(8000)));
        let one = DeploySpec { nodes: vec!["cell1".into()], tensor_parallel: 1, ..s.clone() };
        assert!(!sglang_command(&one, 0, "127.0.0.1", None).contains("--nnodes"));
        s.require_key = true;
        assert!(validate(&s).unwrap_err().contains("command line"), "no key on a command line, ever");
        s.require_key = false;
        s.pipeline_parallel = 2;
        s.tensor_parallel = 1;
        assert!(validate(&s).is_err());
        assert_eq!(stage_from_logs("The server is fired up and ready to roll!", false), Stage::Ready);
    }

    #[test]
    fn hf_cache_snapshots_mount_the_whole_repo_folder() {
        assert_eq!(
            mount_root("/home/u/.cache/huggingface/hub/models--Qwen--Qwen3.8-27B-FP8/snapshots/abc123"),
            "/home/u/.cache/huggingface/hub/models--Qwen--Qwen3.8-27B-FP8"
        );
        assert_eq!(mount_root("/home/u/models/Qwen/Qwen3.8-27B-FP8"), "/home/u/models/Qwen/Qwen3.8-27B-FP8");
    }

    #[test]
    fn ray_image_tags_are_derived_safely() {
        assert_eq!(ray_image("nvcr.io/nvidia/vllm:26.05-py3"), "sparkbox/vllm-ray:26.05-py3");
        assert_eq!(ray_image("localhost:5000/vllm"), "sparkbox/vllm-ray:latest");
    }

    #[test]
    fn startup_stages_are_read_from_the_log() {
        assert_eq!(stage_from_logs("sparkbox: 2 of 4 GPUs joined", false), Stage::Joining { have: 2, need: 4 });
        assert_eq!(stage_from_logs("sparkbox: the API key file is missing or empty, so vLLM was not started.", false), Stage::Starting);
        assert_eq!(
            stage_from_logs("x\nLoading safetensors checkpoint shards:  45% Completed | 15/33", false),
            Stage::Loading { percent: 45 }
        );
        assert_eq!(stage_from_logs("Capturing CUDA graphs (mixed prefill-decode)", false), Stage::Warming);
        assert_eq!(stage_from_logs("INFO: Application startup complete.", false), Stage::Ready);
        assert_eq!(stage_from_logs("The server is fired up and ready to roll!", false), Stage::Ready);
        assert_eq!(stage_from_logs("", true), Stage::Ready);
        assert_eq!(stage_from_logs("anything at all", true), Stage::Ready, "healthy decides");
        assert_eq!(stage_from_logs("pulling", false), Stage::Starting);
    }

    /// tqdm redraws with \r, so `docker logs` can hold a whole bar as one line. Its shape comes
    /// from tqdm, not a capture: no real shard line has been kept yet.
    #[test]
    fn a_bar_redrawn_with_carriage_returns_reads_its_newest_percent() {
        let bar = |p: u32| format!("Loading safetensors checkpoint shards: {p:3}% Completed | {}/33 [00:12<00:14]", p * 33 / 100);
        let joined = |ps: &[u32]| ps.iter().map(|p| bar(*p)).collect::<Vec<_>>().join("\r");
        assert_eq!(stage_from_logs(&joined(&[0, 14, 57]), false), Stage::Loading { percent: 57 });
        assert_eq!(stage_from_logs(&format!("older line\n{}\r", joined(&[0, 100])), false), Stage::Loading { percent: 100 });
        assert_eq!(stage_from_logs(&joined(&[0, 14, 57]).replace('\r', "\n"), false), Stage::Loading { percent: 57 }, "the same on separate lines");
        // A line cut in the middle of a redraw: the last whole number.
        assert_eq!(stage_from_logs(&format!("{}\rLoading safetensors checkpoint shards:", joined(&[0, 14])), false), Stage::Loading { percent: 14 });
    }

    /// Every row of C-ST, in the log's case and in shouting, and newest-first between them.
    #[test]
    fn each_row_of_the_stage_table_reads_as_it_says() {
        let compiling = |what: &str| Stage::Compiling { what: what.into() };
        let rows: Vec<(&str, Stage)> = vec![
            ("INFO:     Application startup complete.", Stage::Ready),
            ("[2026-11-01 12:00:00] The server is fired up and ready to roll!", Stage::Ready),
            ("(Worker pid=1) INFO [gpu_worker.py:1] Capturing CUDA graphs (decode, FULL): 40%", Stage::Warming),
            ("(Worker pid=1) INFO FlashInfer autotune: searching for the best kernel split", compiling("FlashInfer autotune")),
            ("(Worker pid=1) WARNING No available shared memory broadcast block found in 60 seconds", compiling("FlashInfer autotune")),
            ("(Worker pid=1) INFO FlashInfer JIT: building fused attention", compiling("FlashInfer kernels")),
            ("flashinfer: compiling sm120 module", compiling("FlashInfer kernels")),
            ("building flashinfer", compiling("FlashInfer kernels")),
            ("FlashInfer kernel cache miss", compiling("FlashInfer kernels")),
            ("Triton compiling kernels for gpu", compiling("Triton kernels")),
            ("Compilation of triton kernel took 3 s", compiling("Triton kernels")),
            ("Running torch.compile on the model", compiling("torch.compile")),
            ("Compiling a graph for dynamic shape", compiling("torch.compile")),
            ("Loading safetensors checkpoint shards:  45% Completed | 15/33 [00:12<00:14]", Stage::Loading { percent: 45 }),
            ("(Worker_TP0 pid=251) INFO 10-26 22:21:14 [model_runner.py:390] Loading model from scratch...", Stage::Loading { percent: 0 }),
            ("sparkbox: 3 of 4 nodes", Stage::Joining { have: 3, need: 4 }),
        ];
        for (line, want) in &rows {
            assert_eq!(&stage_from_logs(line, false), want, "{line}");
            assert_eq!(&stage_from_logs(&format!("older line\n{line}\nnewer line that says nothing\n"), false), want, "{line} with quiet lines around it");
            assert_eq!(&stage_from_logs(&line.to_uppercase(), false), want, "{line}, any case");
        }
        // The newest line decides, not the table's order: weights after a compile read as loading.
        assert_eq!(stage_from_logs("Compiling a graph\nLoading safetensors checkpoint shards:  10% Completed", false), Stage::Loading { percent: 10 });
        assert_eq!(stage_from_logs("Loading model from scratch...\nTriton compiling", false), compiling("Triton kernels"));
        assert_eq!(stage_from_logs("checkpoint shards:\nLoading model from scratch...", false), Stage::Loading { percent: 0 }, "a shards line with no number says nothing");
        // Words that look like a row and aren't.
        for quiet in ["flashinfer is installed", "triton 3.1 found", "FlashInfer All Reduce is disabled because it is not supported for world_size=4."] {
            assert_eq!(stage_from_logs(quiet, false), Stage::Starting, "{quiet}");
        }
    }

    #[test]
    fn a_stage_reads_on_the_wire_as_the_ui_expects() {
        assert_eq!(serde_json::to_value(Stage::Compiling { what: "torch.compile".into() }).unwrap(), json!({ "kind": "compiling", "what": "torch.compile" }));
        let back: Stage = serde_json::from_value(json!({ "kind": "joining", "have": 1, "need": 2 })).unwrap();
        assert_eq!(back, Stage::Joining { have: 1, need: 2 });
    }

    /// vLLM dumps its settings into one long line at startup, and those name autotune, FlashInfer,
    /// JIT and kernels. They aren't a step it has reached.
    #[test]
    fn the_real_glm53_logs_read_as_loading_and_the_settings_dump_as_nothing() {
        let wrapped = include_str!("fixtures/term-logs-glm53-cell1.txt");
        let raw = include_str!("fixtures/logs/glm53-rank0.log");
        assert_eq!(stage_from_logs(wrapped, false), Stage::Loading { percent: 0 }, "as far as it got: it died on the attention backend");
        assert_eq!(stage_from_logs(raw, false), Stage::Loading { percent: 0 });
        for log in [wrapped, raw] {
            let before: String = log.lines().take_while(|l| !l.contains("Loading model from scratch")).map(|l| format!("{l}\n")).collect();
            assert!(before.contains("autotune"), "the dump is in the log");
            assert_eq!(stage_from_logs(&before, false), Stage::Starting);
        }
    }

    #[test]
    fn the_transport_is_read_from_the_log() {
        // The real line, from the owner's run: no VLLM_HOST_IP, so the queue took the LAN.
        let real = transport_from_logs(include_str!("fixtures/logs/glm53-rank0.log"));
        assert_eq!(real, Transport { ib: vec![], socket: false, mq_ip: Some("192.168.50.51".into()) });
        let fabric = ["192.168.100.1".to_string(), "192.168.101.1".to_string()];
        assert_eq!(transport_warning(&real, &fabric).as_deref(), Some("It isn't using the RDMA fabric: its message queue is on 192.168.50.51, which isn't a fabric address."));

        // The needles from the plan: NCCL's NET/IB and NET/Socket lines.
        let ib = "mq_connect_ip=192.168.100.1 (local), world_size=2\nNCCL INFO NET/IB : Using [0]rocep1s0f1:1/RoCE [1]roceP2p1s0f1:1/RoCE [RO]; OOB enp1s0f1np1:192.168.100.1<0>\n";
        let t = transport_from_logs(ib);
        assert_eq!(t, Transport { ib: vec!["rocep1s0f1".into(), "roceP2p1s0f1".into()], socket: false, mq_ip: Some("192.168.100.1".into()) });
        assert_eq!(transport_warning(&t, &fabric), None);

        let socket = "NCCL INFO NET/IB : No device found.\nNCCL INFO NET/Socket : Using [0]enp1s0f1np1:192.168.100.1<0>\n";
        let t = transport_from_logs(socket);
        assert_eq!(t, Transport { ib: vec![], socket: true, mq_ip: None });
        assert_eq!(transport_warning(&t, &fabric).as_deref(), Some("It isn't using the RDMA fabric: NCCL is on sockets, with no RDMA device."));
        let both = transport_from_logs(&format!("{ib}{socket}"));
        assert_eq!(transport_warning(&both, &fabric), None, "RDMA in use with a socket line besides it is fine");
        assert_eq!(transport_from_logs(""), Transport::default());
        assert!(transport_command("sparkbox-d1-r0").ends_with("grep -m 12 -E 'NET/(IB|Socket)|mq_connect_ip='"));
    }

    #[test]
    fn the_first_subnet_is_preferred_and_its_hca_comes_first() {
        let f = vec![
            ("enP2p1s0f1np1".to_string(), "192.168.101.1".to_string(), "roceP2p1s0f1".to_string()),
            ("enp1s0f1np1".to_string(), "192.168.100.1".to_string(), "rocep1s0f1".to_string()),
        ];
        let n = pick_fabric("cell1", &f).unwrap();
        assert_eq!((n.iface.as_str(), n.ip.as_str()), ("enp1s0f1np1", "192.168.100.1"));
        assert_eq!(n.hcas, ["rocep1s0f1", "roceP2p1s0f1"], "the primary half's first, as the owner's containers had it");
        let same = pick_fabric("cell1", &[f[1].clone(), f[0].clone()]).unwrap();
        assert_eq!(same, n, "whichever order the node lists them");
        assert!(pick_fabric("cell1", &[]).is_none());

        // One half up, and no RDMA device.
        let only = pick_fabric("cell1", std::slice::from_ref(&f[0])).unwrap();
        assert_eq!((only.iface.as_str(), only.hcas.len()), ("enP2p1s0f1np1", 1));
        let plain = pick_fabric("cell1", &[("enp1s0f1np1".into(), "10.0.0.1".into(), String::new())]).unwrap();
        assert!(plain.hcas.is_empty());
    }

    #[test]
    fn a_fabric_name_a_shell_would_read_is_rejected() {
        let good = ("enp1s0f1np1".to_string(), "192.168.100.1".to_string(), "rocep1s0f1".to_string());
        for bad in [
            ("en$(reboot)".to_string(), "192.168.100.1".to_string(), "rocep1s0f1".to_string()),
            ("en p1".to_string(), "192.168.100.1".to_string(), "rocep1s0f1".to_string()),
            ("-en".to_string(), "192.168.100.1".to_string(), "rocep1s0f1".to_string()),
            ("enp1".to_string(), "192.168.100.1;id".to_string(), "rocep1s0f1".to_string()),
            ("enp1".to_string(), "192.168.100.1".to_string(), "rocep`id`".to_string()),
            ("enp1".to_string(), "192.168.100.1".to_string(), "a,b".to_string()),
        ] {
            assert_eq!(pick_fabric("cell1", std::slice::from_ref(&bad)), None, "{bad:?}");
            let n = pick_fabric("cell1", &[bad, good.clone()]).unwrap();
            assert_eq!((n.iface.as_str(), n.hcas.as_slice()), ("enp1s0f1np1", &["rocep1s0f1".to_string()][..]), "the bad one is left out");
        }
    }

    // ---- The launch (C-API, C-E, C-V, C-L2)

    /// Four nodes whose every fabric value is its own, so a value from the wrong rank shows.
    fn nets4() -> Vec<NodeNet> {
        (1..=4u8)
            .map(|i| NodeNet { node_id: format!("cell{i}"), iface: format!("en{i}"), ip: format!("192.168.100.{}", i * 10), hcas: vec![format!("hca{i}a"), format!("hca{i}b")] })
            .collect()
    }

    fn load(nodes: usize, tp: u64, pp: u64) -> DeploySpec {
        DeploySpec {
            id: "d1".into(),
            served_name: "gemma-3-27b-it".into(),
            model_path: "/home/sparkuser/models/google/gemma-3-27b-it".into(),
            nodes: (1..=nodes).map(|i| format!("cell{i}")).collect(),
            tensor_parallel: tp,
            pipeline_parallel: pp,
            max_model_len: Some(32768),
            host: "192.168.50.51".into(),
            image: DEFAULT_IMAGE.into(),
            image_ref: DEFAULT_IMAGE.into(),
            image_id: local_id(),
            recipe: "gemma-3-27b-it".into(),
            docker_access: "privileged".into(),
            ..Default::default()
        }
    }

    const SERVE: &str = "vllm serve /home/sparkuser/models/google/gemma-3-27b-it --served-model-name gemma-3-27b-it --gpu-memory-utilization 0.75 --max-model-len 32768";

    #[test]
    fn the_rank_argv_is_c_v_for_one_node_tp2_pp3_and_tp4() {
        let nets = nets4();
        let argv = |s: &DeploySpec, r: usize| rank_argv(s, r, &nets).join(" ");
        assert_eq!(rank_argv(&load(1, 1, 1), 0, &[]).join(" "), format!("{SERVE} --host 192.168.50.51 --port 8000"));

        let tp2 = load(2, 2, 1);
        assert_eq!(argv(&tp2, 0), format!("{SERVE} --tensor-parallel-size 2 --nnodes 2 --node-rank 0 --master-addr 192.168.100.10 --master-port 29501 --host 192.168.50.51 --port 8000"));
        assert_eq!(argv(&tp2, 1), format!("{SERVE} --tensor-parallel-size 2 --nnodes 2 --node-rank 1 --master-addr 192.168.100.10 --master-port 29501 --headless"));

        let pp3 = load(3, 1, 3);
        assert_eq!(argv(&pp3, 0), format!("{SERVE} --tensor-parallel-size 1 --pipeline-parallel-size 3 --nnodes 3 --node-rank 0 --master-addr 192.168.100.10 --master-port 29501 --host 192.168.50.51 --port 8000"));
        assert_eq!(argv(&pp3, 2), format!("{SERVE} --tensor-parallel-size 1 --pipeline-parallel-size 3 --nnodes 3 --node-rank 2 --master-addr 192.168.100.10 --master-port 29501 --headless"));

        let tp4 = load(4, 4, 1);
        for r in 0..4 {
            let a = argv(&tp4, r);
            assert!(a.contains(&format!("--tensor-parallel-size 4 --nnodes 4 --node-rank {r} --master-addr 192.168.100.10 --master-port 29501")), "{a}");
            assert_eq!(a.contains("--host"), r == 0, "{a}");
            assert_eq!(a.ends_with("--headless"), r > 0, "{a}");
            assert!(!a.contains("--distributed-executor-backend") && !a.contains("ray"), "{a}");
        }
        // The fields, then the recipe's options, then Expert's, then the rank's.
        let full = DeploySpec {
            max_num_seqs: Some(4),
            kv_cache_dtype: Some("fp8".into()),
            prefix_caching: Some(false),
            tool_call_parser: Some("hermes".into()),
            reasoning_parser: Some("qwen3".into()),
            allow_remote_code: true,
            expert_parallel: true,
            recipe_args: vec!["--block-size".into(), "256".into()],
            extra_args: vec!["--seed".into(), "7".into()],
            ..load(2, 2, 1)
        };
        assert_eq!(
            argv(&full, 1),
            format!("{SERVE} --max-num-seqs 4 --kv-cache-dtype fp8 --no-enable-prefix-caching --enable-auto-tool-choice --tool-call-parser hermes --reasoning-parser qwen3 --trust-remote-code --block-size 256 --seed 7 --tensor-parallel-size 2 --nnodes 2 --node-rank 1 --master-addr 192.168.100.10 --master-port 29501 --headless")
                .replace("--max-model-len 32768", "--enable-expert-parallel --max-model-len 32768")
        );
    }

    #[test]
    fn a_loads_identity_includes_its_split() {
        assert_ne!(vllm_args(&load(2, 2, 1)), vllm_args(&load(2, 1, 2)));
        assert!(!vllm_args(&load(1, 1, 1)).iter().any(|a| a == "--tensor-parallel-size"), "a load on one node reads as it always did");
    }

    #[test]
    fn every_env_value_comes_from_the_ranks_own_node() {
        let nets = nets4();
        let map = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();
        let base = map(&[("HF_HUB_OFFLINE", "1"), ("VLLM_ENGINE_READY_TIMEOUT_S", "3600")]);
        assert_eq!(rank_env(&load(1, 1, 1), 0, &[]), base, "one node: no fabric names at all");
        assert_eq!(rank_env(&load(1, 1, 1), 0, &nets), base);
        for s in [load(2, 2, 1), load(3, 1, 3), load(4, 4, 1)] {
            for rank in 0..s.nodes.len() {
                let n = &nets[rank];
                let hca = n.hcas.join(",");
                let mut want = base.clone();
                want.extend(map(&[
                    ("VLLM_HOST_IP", &n.ip), ("NCCL_SOCKET_IFNAME", &n.iface), ("GLOO_SOCKET_IFNAME", &n.iface), ("TP_SOCKET_IFNAME", &n.iface), ("UCX_NET_DEVICES", &n.iface),
                    ("OMPI_MCA_btl_tcp_if_include", &n.iface), ("NCCL_IB_HCA", &hca), ("NCCL_IB_DISABLE", "0"), ("NCCL_IGNORE_CPU_AFFINITY", "1"), ("NCCL_DEBUG", "INFO"),
                ]));
                assert_eq!(rank_env(&s, rank, &nets), want, "rank {rank}");
            }
        }
        for name in rank_env(&load(2, 2, 1), 1, &nets).keys() {
            assert!(RANK_ENV.contains(&name.as_str()), "{name} is listed, so a recipe can't set it");
            assert!(!name.starts_with("RAY_") && name != "MASTER_ADDR", "{name}");
        }
        // The recipe's come through, and the rank's own are never overridden.
        let mut s = load(2, 2, 1);
        s.env.insert("CUTE_DSL_ARCH".into(), "sm_121a".into());
        s.env.insert("VLLM_HOST_IP".into(), "192.168.50.51".into());
        let env = rank_env(&s, 1, &nets);
        assert_eq!(env["CUTE_DSL_ARCH"], "sm_121a");
        assert_eq!(env["VLLM_HOST_IP"], "192.168.100.20");
    }

    const C_L2: &str = r#"docker run -d --name sparkbox-d1 --label sparkbox.instance=release --label sparkbox.deployment=d1 --label sparkbox.group=d1 --label sparkbox.role=single --label sparkbox.rank=0 --label sparkbox.model=gemma-3-27b-it --label sparkbox.recipe=gemma-3-27b-it --label sparkbox.image=vllm/vllm-openai:glm53-flash-arm64-cu130 --label sparkbox.path=/home/sparkuser/models/google/gemma-3-27b-it --label sparkbox.parallel=1x1 --label sparkbox.gpu-share=0.75 --label sparkbox.port=8000 --label sparkbox.listen=192.168.50.51 --label sparkbox.maxlen=32768 --network host --ipc host --gpus all --ulimit memlock=-1 --ulimit stack=67108864 --ulimit nofile=1048576:1048576 --restart no -v /home/sparkuser/models/google/gemma-3-27b-it:/home/sparkuser/models/google/gemma-3-27b-it:ro -v "$HOME/.cache/vllm:/root/.cache/vllm" -v "$HOME/.cache/flashinfer:/root/.cache/flashinfer" -v "$HOME/.triton:/root/.triton" -v "$HOME/.sparkbox/deploy/d1:/run/sparkbox/deploy:ro" -v "$HOME/.sparkbox/launch-12b1d2f4c5b5565459585085c89dab0171be5d22c0618d3f7a219b871733ddac.py:/run/sparkbox/launch.py:ro" -v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro" --entrypoint python3 sha256:aaaa…aaaa /run/sparkbox/launch.py --key"#;

    #[test]
    fn the_rank_commands_are_c_l2_byte_for_byte() {
        let id = |s: &str| s.replace("sha256:aaaa…aaaa", &local_id());
        assert_eq!(single_command_for(&load(1, 1, 1), "release"), id(C_L2));
        assert_eq!(rank_command_for(&load(1, 1, 1), 0, "release"), id(C_L2));
        assert_eq!(single_command(&load(1, 1, 1)), id(C_L2).replace("sparkbox.instance=release", &format!("sparkbox.instance={}", instance_name())));
        assert!(!id(C_L2).contains('\n') && !C_L2.contains(" -e ") && !C_L2.contains("--shm-size"), "one line, no -e, no --shm-size");

        // No key: neither the mount nor --key.
        let open = rank_command_for(&DeploySpec { require_key: false, ..load(1, 1, 1) }, 0, "release");
        assert_eq!(open, id(C_L2).replace(r#" -v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro""#, "").replace(" --key", ""));

        // Keep running is a single's, and so is having no --privileged.
        let keep = rank_command_for(&DeploySpec { keep_running: true, ..load(1, 1, 1) }, 0, "release");
        assert_eq!(keep, id(C_L2).replace("--restart no", "--restart unless-stopped"));

        // A head: named by its rank, --privileged in its place, the same key handling.
        let head = rank_command_for(&DeploySpec { keep_running: true, ..load(2, 2, 1) }, 0, "release");
        let want = id(C_L2)
            .replace("--name sparkbox-d1 ", "--name sparkbox-d1-r0 ")
            .replace("sparkbox.role=single", "sparkbox.role=head")
            .replace("sparkbox.parallel=1x1", "sparkbox.parallel=2x1")
            .replace("--ulimit nofile=1048576:1048576 ", "--ulimit nofile=1048576:1048576 --privileged ");
        assert_eq!(head, want);

        // A worker: no port, no listen, no key, no --key; its rank.
        let worker = rank_command_for(&load(2, 2, 1), 1, "release");
        let want = want
            .replace("--name sparkbox-d1-r0 ", "--name sparkbox-d1-r1 ")
            .replace("sparkbox.role=head", "sparkbox.role=worker")
            .replace("sparkbox.rank=0", "sparkbox.rank=1")
            .replace(" --label sparkbox.port=8000 --label sparkbox.listen=192.168.50.51", "")
            .replace(r#" -v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro""#, "")
            .replace(" --key", "");
        assert_eq!(worker, want);

        // Standard access: no --privileged, on any rank. A group, a parallel of 2x2, no context.
        let plain = rank_command_for(&DeploySpec { docker_access: "standard".into(), ..load(2, 2, 1) }, 0, "release");
        assert!(!plain.contains("--privileged"), "{plain}");
        let wide = rank_command_for(&DeploySpec { group: "g1".into(), max_model_len: None, recipe: String::new(), ..load(4, 2, 2) }, 3, "release");
        for needle in ["--label sparkbox.group=g1 ", "--label sparkbox.parallel=2x2 ", "--label sparkbox.recipe=generic ", "--name sparkbox-d1-r3 "] {
            assert!(wide.contains(needle), "{needle}: {wide}");
        }
        assert!(!wide.contains("sparkbox.maxlen"), "{wide}");
        // --privileged is never a single's, even with the field set.
        assert!(!single_command(&load(1, 1, 1)).contains("--privileged"));

        // A Hugging Face cache snapshot mounts its models--org--name folder (cell2's gemma).
        let snap = "/home/sparkuser/.cache/huggingface/hub/models--google--gemma-3-27b-it/snapshots/005ad3404e59d6023443cb575daa05336842228a";
        let s = DeploySpec { model_path: snap.into(), ..load(1, 1, 1) };
        let hf = single_command_for(&s, "release");
        assert!(hf.contains(" -v /home/sparkuser/.cache/huggingface/hub/models--google--gemma-3-27b-it:/home/sparkuser/.cache/huggingface/hub/models--google--gemma-3-27b-it:ro "), "{hf}");
        assert!(hf.contains(&format!("--label sparkbox.path={snap} ")), "{hf}");
        assert_eq!(rank_argv(&s, 0, &[])[2], snap);
    }

    /// A node's id, its name and its hostname are three things: none reaches a name or a label.
    #[test]
    fn a_renamed_node_appears_in_no_name_or_label() {
        let mut s = load(4, 4, 1);
        s.nodes[3] = "cell4".into();
        let nets: Vec<NodeNet> = nets4().into_iter().enumerate().map(|(i, n)| NodeNet { node_id: s.nodes[i].clone(), ..n }).collect();
        for rank in 0..4 {
            let cmd = rank_command(&s, rank);
            let payload = launch_payload(&s, rank, &nets, Some("sbk-x")).to_string();
            for text in [&cmd, &payload] {
                for word in ["cell4", "cell4-gx10", "gx10-0000", "cell1", "cell2", "cell3"] {
                    assert!(!text.contains(word), "{word} in {text}");
                }
            }
        }
    }

    #[test]
    fn the_api_key_is_in_no_argv_env_or_command() {
        let key = "sbk-3f2a9c";
        let nets = nets4();
        for s in [load(1, 1, 1), load(2, 2, 1), load(4, 4, 1)] {
            for rank in 0..s.nodes.len() {
                let (argv, env, cmd) = (rank_argv(&s, rank, &nets), rank_env(&s, rank, &nets), rank_command(&s, rank));
                assert!(!argv.iter().any(|a| a.contains(key) || a == "--api-key"), "{argv:?}");
                assert!(!env.contains_key(KEY_ENV) && !env.values().any(|v| v.contains(key)), "{env:?}");
                assert!(!cmd.contains(key) && !cmd.contains(KEY_ENV), "{cmd}");
                let payload = launch_payload(&s, rank, &nets, Some(key));
                assert_eq!(payload["key"].as_str().is_some(), rank == 0, "the key goes to rank 0 only");
                assert_eq!(payload["argv"], json!(argv));
                assert_eq!(payload["env"], json!(env));
            }
        }
        assert_eq!(launch_payload(&load(1, 1, 1), 0, &[], None)["key"], Value::Null);
        assert_eq!(launch_payload(&DeploySpec { require_key: false, ..load(1, 1, 1) }, 0, &[], Some(key))["key"], Value::Null, "no key load, no key file");
        assert!(load(1, 1, 1).require_key && single_command(&load(1, 1, 1)).ends_with("launch.py --key"));
        assert_eq!(LAUNCH.matches("/run/sparkbox").count(), 1, "launch.py finds its root from its own path");
        assert!(LAUNCH.contains(&format!("REFUSED = {KEY_REFUSED}")));
    }

    #[test]
    fn names_and_roles_follow_the_rank() {
        assert_eq!(rank_container("d1", 0, 1), "sparkbox-d1");
        assert_eq!(rank_container("d1", 0, 2), "sparkbox-d1-r0");
        assert_eq!(rank_container("d1", 3, 4), "sparkbox-d1-r3");
        assert_eq!(role_of(&load(1, 1, 1), 0), "single");
        assert_eq!((role_of(&load(4, 4, 1), 0), role_of(&load(4, 4, 1), 3)), ("head", "worker"));
        assert!(is_native(&load(1, 1, 1)) && is_native(&load(4, 4, 1)));
        assert!(!is_native(&DeploySpec { engine: "sglang".into(), ..load(1, 1, 1) }));
    }

    /// What goes to a node has to parse as what it is, in the shell that will read it.
    #[test]
    fn what_goes_to_a_node_is_one_valid_shell_line() {
        let jumbo = jumbo_script(&[("enp1s0f1np1".into(), "192.168.100.1".into())]);
        for script in [rank_command(&load(2, 2, 1), 0), preflight_script(&load(2, 2, 1)), unload_command("d1"), jumbo] {
            let out = std::process::Command::new("sh").args(["-n", "-c", &script]).output().unwrap();
            assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
            assert!(!script.contains('\n'), "{script}");
        }
    }

    // ---- Which splits a model takes (P2.3)

    fn gemma_arch() -> ArchFacts {
        ArchFacts { layers: 42, heads: 32, kv_heads: 16, head_dim: 128, ..Default::default() }
    }

    fn offer_row(arch: &ArchFacts, nodes: usize) -> Vec<(String, u64, u64, bool, bool)> {
        topologies(arch, nodes).into_iter().filter(|o| o.replicas == 1).map(|o| (o.id, o.tp, o.pp, o.ok, o.advanced)).collect()
    }

    #[test]
    fn replica_groups_keep_ranks_memory_keys_and_unloads_independent() {
        for (nodes, replicas, tp) in [(3, 3, 1), (4, 2, 2)] {
            let s = DeploySpec { replicas, ..load(nodes, tp, 1) };
            let children = replica_specs(&s).unwrap();
            assert_eq!(children.len(), replicas as usize);
            assert_eq!(children.iter().flat_map(|c| c.nodes.clone()).collect::<Vec<_>>(), s.nodes);
            let keys = children.iter().map(|c| key_account(&c.id)).collect::<std::collections::HashSet<_>>();
            assert_eq!(keys.len(), replicas as usize);
            for child in &children {
                assert_eq!(child.group, s.id); assert_eq!(child.nodes.len(), tp as usize); assert_eq!(child.replicas,1);
                assert!(rank_command(child,0).contains(&format!("sparkbox.group={}", s.id)));
                for sibling in children.iter().filter(|c| c.id != child.id) { assert!(!unload_command(&child.id).contains(&sibling.id)); }
            }
            let needs = ModelNeeds { weight_bytes: 60 * 1024u64.pow(3), arch: ArchFacts { vision_bytes: 2 * 1024u64.pow(3), ..Default::default() }, ..Default::default() };
            let parent = memory_plan(&s, &needs, &[], &[]);
            let child = memory_plan(&children[0], &needs, &[], &[]);
            assert_eq!(parent[0].weights, child[0].weights);
            for rank in 0..parent.len() { assert_eq!(parent[rank].need, child[rank % child.len()].need, "vision overhead repeats on each replica head"); }
        }
        assert!(validate(&DeploySpec { replicas:3, ..load(3,3,1) }).is_err());
        assert!(validate(&DeploySpec { replicas:2, ..load(4,1,2) }).is_err());
    }

    #[test]
    fn gemma_refuses_three_way_tensor_parallel_and_takes_tp2_and_tp4() {
        let a = gemma_arch();
        let at = |n: usize, tp: u64, pp: u64| validate_topology(&load(n, tp, pp), &a);
        assert_eq!(at(1, 1, 1), Ok(()));
        assert_eq!(at(2, 2, 1), Ok(()));
        assert_eq!(at(4, 4, 1), Ok(()));
        assert_eq!(at(3, 3, 1).unwrap_err(), "Tensor parallel 3 won't work for this model: 32 heads don't divide by 3. Use pipeline parallel 3.");
        assert_eq!(at(3, 1, 3), Ok(()), "pipeline parallel is what it says to use");
        assert_eq!(at(4, 2, 2), Ok(()));

        let row = |id: &str, tp, pp, ok, adv| (id.to_string(), tp, pp, ok, adv);
        assert_eq!(offer_row(&a, 1), [row("single", 1, 1, true, false)]);
        assert_eq!(offer_row(&a, 2), [row("tp2", 2, 1, true, false), row("pp2", 1, 2, true, false)]);
        assert_eq!(offer_row(&a, 3), [row("tp3", 3, 1, false, false), row("pp3", 1, 3, true, false)]);
        assert_eq!(offer_row(&a, 4), [row("tp4", 4, 1, true, false), row("pp4", 1, 4, true, false), row("tp2pp2", 2, 2, true, true)]);
        let three = topologies(&a, 3);
        assert!(three[0].reason.as_deref().unwrap().contains("32 heads don't divide by 3"));
        assert_eq!((three[0].nodes, three[1].reason.clone()), (3, None));
        assert!(topologies(&a, 4)[2].reason.as_deref().unwrap().starts_with("Advanced"));
        assert!(topologies(&a, 0).is_empty());
        assert!(topologies(&a, 5).is_empty(), "only the four inventory nodes are offered");
    }

    #[test]
    fn the_refusals_are_the_contracts_sentences() {
        // KV heads: Qwen's 24 heads divide by 3, its 4 KV heads don't.
        let qwen = ArchFacts { layers: 64, heads: 24, kv_heads: 4, head_dim: 256, ..Default::default() };
        assert_eq!(validate_topology(&load(3, 3, 1), &qwen).unwrap_err(), "Tensor parallel 3 won't work for this model: 4 KV heads don't divide by 3, and 3 isn't a multiple of them.");
        assert_eq!(validate_topology(&load(4, 4, 1), &qwen), Ok(()), "TP may be a multiple of the KV heads");
        // Pipeline parallel can't exceed the layers.
        let small = ArchFacts { layers: 2, heads: 8, kv_heads: 8, ..Default::default() };
        assert_eq!(validate_topology(&load(3, 1, 3), &small).unwrap_err(), "Pipeline parallel 3 won't work: this model has 2 layers.");
        assert_eq!(validate_topology(&load(2, 1, 2), &small), Ok(()));
        // No pipeline parallel to suggest when that wouldn't work either.
        let tiny = ArchFacts { layers: 2, heads: 7, kv_heads: 7, ..Default::default() };
        assert_eq!(validate_topology(&load(3, 3, 1), &tiny).unwrap_err(), "Tensor parallel 3 won't work for this model: 7 heads don't divide by 3.");
        assert!(topologies(&tiny, 3).iter().filter(|o| o.replicas == 1).all(|o| !o.ok));
        assert!(topologies(&tiny, 3).iter().any(|o| o.replicas == 3 && o.ok), "independent single-node replicas do not split its layers or heads");
    }

    #[test]
    fn unknown_heads_are_allowed_with_a_warning() {
        let unknown = ArchFacts::default();
        assert_eq!(validate_topology(&load(3, 3, 1), &unknown), Ok(()), "vLLM itself should refuse it when it starts");
        assert_eq!(validate_topology(&load(3, 1, 3), &unknown), Ok(()));
        let offers = topologies(&unknown, 2);
        assert!(offers[0].ok && offers[0].reason.as_deref().unwrap().contains("doesn't say how many attention heads"), "{offers:?}");
        assert!(offers[1].ok && offers[1].reason.is_none(), "pipeline parallel doesn't depend on heads");
        assert_eq!(topologies(&unknown, 1)[0].reason, None);
    }

    #[test]
    fn topology_ids_are_what_recipes_list() {
        assert_eq!(["single", "tp2", "pp3", "tp2pp2"], [topology_id(1, 1), topology_id(2, 1), topology_id(1, 3), topology_id(2, 2)]);
    }

    // ---- Guardrails

    #[test]
    fn unload_removes_the_key_and_the_launch_files() {
        assert!(unload_command("glm53").contains("rm -rf ~/.sparkbox/keys/glm53 ~/.sparkbox/deploy/glm53"), "{}", unload_command("glm53"));
        let mut s = spec(&["cell1"], 1, 1);
        s.extra_args = vec!["--api-key".into(), "sbk-3f2a9c".into()];
        assert!(validate(&s).unwrap_err().contains("from a file only it can read"));
    }

    #[test]
    fn remote_code_needs_its_own_opt_in() {
        let mut s = DeploySpec { allow_remote_code: false, ..spec(&["cell1"], 1, 1) };
        s.extra_args = vec!["--trust-remote-code".into()];
        assert!(validate(&s).unwrap_err().contains("Allow remote code"));
        s.extra_args.clear();
        assert!(!vllm_args(&s).contains(&"--trust-remote-code".to_string()));
        s.allow_remote_code = true;
        assert!(validate(&s).is_ok());
        assert_eq!(vllm_args(&s).iter().filter(|a| *a == "--trust-remote-code").count(), 1);
    }

    #[test]
    fn capabilities_become_vllm_options() {
        let s = DeploySpec {
            max_num_seqs: Some(8),
            kv_cache_dtype: Some("fp8".into()),
            prefix_caching: Some(false),
            tool_call_parser: Some("llama3_json".into()),
            reasoning_parser: Some("qwen3".into()),
            listen: "lan".into(),
            host: "192.168.50.51".into(),
            keep_running: true,
            allow_remote_code: false,
            ..spec(&["cell1"], 1, 1)
        };
        let a = vllm_args(&s).join(" ");
        for needle in [
            "--host 192.168.50.51", "--max-num-seqs 8", "--kv-cache-dtype fp8", "--no-enable-prefix-caching",
            "--enable-auto-tool-choice --tool-call-parser llama3_json", "--reasoning-parser qwen3",
        ] {
            assert!(a.contains(needle), "missing {needle}: {a}");
        }
        assert!(single_command(&s).contains("--restart unless-stopped"));
        assert_eq!(probe_host(&s), "192.168.50.51", "probes reach it where it listens");
        assert_eq!(probe_host(&DeploySpec { listen: "all".into(), ..s.clone() }), "127.0.0.1");
        let bad = DeploySpec { tool_call_parser: Some("x; rm -rf /".into()), ..s };
        assert!(validate(&bad).is_err());
    }

    fn node(id: &str, gpu: &[(&str, u64)], listeners: &[u16]) -> NodeSnapshot {
        use crate::model::{GpuProc, Listener};
        let mut n = NodeSnapshot { id: id.into(), name: id.into(), ..Default::default() };
        n.mem.total = 128_530_000_000;
        n.gpu.procs = gpu.iter().map(|(c, m)| GpuProc { pid: 1, name: "python3".into(), mem: *m, container: Some(c.to_string()) }).collect();
        n.listeners = listeners.iter().map(|p| Listener { port: *p, addr: "0.0.0.0".into(), pid: None, process: Some("python3".into()), ..Default::default() }).collect();
        n
    }

    fn qwen_needs() -> ModelNeeds {
        let arch = ArchFacts { layers: 64, kv_layers: 16, heads: 24, kv_heads: 4, head_dim: 256, max_context: Some(262_144), ..Default::default() };
        ModelNeeds { weight_bytes: 30_900_000_000, kv_bytes_per_token: fit::kv_bytes_per_token(&arch), arch }
    }

    #[test]
    fn a_taken_port_is_refused_and_the_next_free_one_offered() {
        let head = node("cell1", &[], &[8000, 8001, 8265]);
        let s = DeploySpec { max_model_len: Some(32768), ..spec(&["cell1"], 1, 1) };
        let plan = memory_plan(&s, &qwen_needs(), &[&head], &[]);
        let err = preflight(&s, &plan, Some(&head)).unwrap_err();
        assert_eq!(err, PreflightError::PortInUse { node: "cell1".into(), port: 8000, by: "python3".into() });
        assert!(err.to_string().contains("Port 8000 is already in use on cell1"));
        assert_eq!(first_free_port(&head, &[]), 8002);
        assert_eq!(first_free_port(&head, &[8002]), 8003, "ports other loads are about to use count too");
        assert!(preflight(&DeploySpec { port: 8002, ..s }, &plan, Some(&head)).is_ok());
    }

    #[test]
    fn the_memory_plan_is_red_or_amber_from_where_the_node_ends_up() {
        let s = DeploySpec { max_model_len: Some(32768), ..spec(&["gx10-0000"], 1, 1) };
        let needs = qwen_needs();
        // KV for one 32k request: 32768 × 64 KiB = 2 GiB.
        assert_eq!(kv_need(&s, &needs), 32768 * 65536);
        let at = |share: f64, node: &NodeSnapshot| memory_plan(&DeploySpec { gpu_memory_utilization: share, ..s.clone() }, &needs, &[node], &[]).remove(0);

        // Next to Ollama (41.5 GiB), vLLM's 0.8 share can't even be reserved: the blocker leads.
        let shared = node("gx10-0000", &[("ollama", 44_560_000_000)], &[]);
        let p = at(0.8, &shared);
        assert_eq!((p.level, p.verdict.as_str()), (Level::Red, "Won't start"));
        assert_eq!(p.reason.as_deref(), Some("Won't start: vLLM reserves 95.8 GiB (0.80 share) at startup; 74.2 GiB is free."));
        let sglang = memory_plan(&DeploySpec { engine: "sglang".into(), gpu_memory_utilization: 0.8, ..s.clone() }, &needs, &[&shared], &[]).remove(0);
        assert_eq!(sglang.reason.as_deref(), Some("Won't start: SGLang reserves 95.8 GiB (0.80 share) at startup; 74.2 GiB is free."));
        // One number to offer: the largest share in 0.05 steps that keeps the node under 90%:
        // 0.9 × 128.5 GB − (44.6 GB Ollama + the OS + vLLM's own 9.4 GB) leaves room for 0.40.
        assert_eq!((p.safe_share, p.max_share, p.fits_safe), (0.4, 0.6, true));
        assert_eq!(suggested_share(&s, &[p.clone()]), Some(0.4));

        // 0.60 passes vLLM's startup check, but the node would end up over 100%: red.
        let p = at(0.6, &shared);
        assert_eq!((p.level, p.verdict.as_str()), (Level::Red, "Won't start safely"), "{:?}", p.reason);
        assert!(p.after_pct >= RED_PCT && p.reason.as_deref().unwrap().contains("would use 105%"), "{:?}", p.reason);
        assert!(preflight(&DeploySpec { gpu_memory_utilization: 0.6, ..s.clone() }, &[p], Some(&shared)).is_err(), "deploy refuses it too");
        // 0.45 lands just over the alert line: amber, co-tenant or not.
        let p = at(0.45, &shared);
        assert_eq!((p.level, p.verdict.as_str()), (Level::Amber, "Tight"), "{}", p.after_pct);
        assert!((90.0..95.0).contains(&p.after_pct));
        assert!(p.reason.as_deref().unwrap().starts_with("Tight: runs above the 90% alert line"));
        // 0.40 is fine, Ollama next to it or not.
        let p = at(0.4, &shared);
        assert_eq!((p.level, p.reason.as_deref()), (Level::Green, None), "{}", p.after_pct);
        assert!(p.after_pct < 90.0);
        assert_eq!(suggested_share(&DeploySpec { gpu_memory_utilization: 0.4, ..s.clone() }, &[p]), None, "nothing to suggest when it's fine");

        // An empty node at NVIDIA's 0.8: the OS and vLLM's own process push it over the line.
        let empty = node("gx10-0000", &[], &[]);
        let p = at(0.8, &empty);
        assert_eq!(p.level, Level::Amber, "{}", p.after_pct);
        assert_eq!(p.need, 30_900_000_000 + 32768 * 65536 + fit::RUNTIME_OVERHEAD as u64);
        assert_eq!(p.safe_share, 0.75);
        assert_eq!(at(0.75, &empty).level, Level::Green);

        // Doesn't fit in its share.
        let p = at(0.3, &shared);
        assert_eq!((p.level, p.verdict.as_str()), (Level::Red, "Won't fit"));
        assert!(p.reason.as_deref().unwrap().starts_with("Won't fit: needs 36.8 GiB, but a 0.30 share gives vLLM 35.9 GiB."), "{:?}", p.reason);

        // Its full 262k context doesn't fit in the safe share next to Ollama: offer a shorter one.
        let long = DeploySpec { max_model_len: None, ..s.clone() };
        let p = memory_plan(&long, &needs, &[&shared], &[]).remove(0);
        assert!(!p.fits_safe);
        let ctx = p.safe_context.unwrap();
        assert_eq!(ctx % 1024, 0);
        assert!((100_000..262_144).contains(&ctx), "{ctx}");
        assert_eq!(suggested_share(&long, &[p.clone()]), None);
        assert_eq!(suggested_context(&long, &[p.clone()]), Some((0.4, ctx)));
        let fixed = DeploySpec { max_model_len: Some(ctx), gpu_memory_utilization: 0.4, ..long.clone() };
        let q = memory_plan(&fixed, &needs, &[&shared], &[]).remove(0);
        assert_eq!(q.level, Level::Green, "the suggestion really fits: {:?}", q.reason);

        // A bigger model doesn't fit even in the safe share: no button, the reason stands alone.
        let big = ModelNeeds { weight_bytes: 70_000_000_000, ..qwen_needs() };
        let p = memory_plan(&s, &big, &[&shared], &[]).remove(0);
        assert!(!p.fits_safe);
        assert_eq!(suggested_share(&s, &[p]), None);

        let busy = node("gx10-0000", &[("vllm-head", 89_900_000_000)], &[]);
        let s6 = DeploySpec { gpu_memory_utilization: 0.6, ..s.clone() };
        let plan = memory_plan(&s6, &needs, &[&busy], &[]);
        assert_eq!(plan[0].level, Level::Red);
        let err = preflight(&s6, &plan, Some(&busy)).unwrap_err();
        assert!(matches!(err, PreflightError::Memory { ref node, .. } if node == "gx10-0000"));
        assert!(preflight(&DeploySpec { override_memory: true, ..s6.clone() }, &plan, Some(&busy)).is_ok(), "typed override");

        // Its own containers don't count against it (an Edit & reload of the same load).
        let plan = memory_plan(&s6, &needs, &[&busy], &["vllm-head".into()]);
        assert_eq!(plan[0].level, Level::Green);

        // fp8 KV and more requests at once move the KV term.
        let fp8 = DeploySpec { kv_cache_dtype: Some("fp8".into()), max_num_seqs: Some(4), ..s };
        assert_eq!(kv_need(&fp8, &needs), 4 * 32768 * 65536 / 2);
    }

    /// A GB10 as cell1 read on the owner's cluster: 121.7 GiB in all, 5.4 GiB in use.
    fn gb10(id: &str) -> NodeSnapshot {
        let mut n = node(id, &[], &[]);
        n.mem.total = (121.7 * GIB) as u64;
        n.mem.used = (5.4 * GIB) as u64;
        n
    }

    /// Qwen3-1.7B as cell1 reads it: 3.8 GiB of weights, 114,688 bytes a token, 40,960 tokens at most.
    fn tiny_needs() -> ModelNeeds {
        let arch = ArchFacts { layers: 28, kv_layers: 28, heads: 16, kv_heads: 8, head_dim: 128, max_context: Some(40960), ..Default::default() };
        ModelNeeds { weight_bytes: (3.8 * GIB) as u64, kv_bytes_per_token: fit::kv_bytes_per_token(&arch), arch }
    }

    fn sized(s: &DeploySpec, m: &ModelNeeds, nodes: &[&NodeSnapshot]) -> Option<f64> {
        sized_share(s, m, &memory_plan(s, m, nodes, &[]))
    }

    /// What sized_share promises, from the plan's own per-node figures: at this share every node
    /// holds the requests it was sized for with the headroom, and a step lower some node doesn't.
    fn holds_and_is_the_least(s: &DeploySpec, m: &ModelNeeds, nodes: &[&NodeSnapshot], share: f64) {
        let full = DeploySpec { max_num_seqs: Some(s.max_num_seqs.unwrap_or(SIZED_REQUESTS)), ..s.clone() };
        let plan = memory_plan(&full, m, nodes, &[]);
        let room = |share: f64, p: &NodePlan| p.need as f64 * SIZED_HEADROOM <= p.total as f64 * share;
        assert!(plan.iter().all(|p| room(share, p)), "{share} holds it on every node: {plan:?}");
        assert!(!plan.iter().all(|p| room(share - 0.05, p)), "{} would hold it too: {plan:?}", share - 0.05);
    }

    #[test]
    fn a_small_model_gets_a_share_its_own_size() {
        let cell1 = gb10("cell1");
        let needs = tiny_needs();
        assert_eq!(needs.kv_bytes_per_token, Some(114_688));
        let s = DeploySpec { max_model_len: Some(32768), gpu_memory_utilization: fit::FIRST_LOAD_SHARE, ..spec(&["cell1"], 1, 1) };
        // 3.8 weights + 6 runtime + 4 × 3.5 KV = 23.8 GiB, 26.2 with the headroom, a 0.22 share.
        assert_eq!(sized(&s, &needs, &[&cell1]), Some(0.25));
        holds_and_is_the_least(&s, &needs, &[&cell1], 0.25);

        // It's what the owner hit: 0.75 reserves 91.3 GiB for a model that needs 13.3.
        let now = memory_plan(&s, &needs, &[&cell1], &[]).remove(0);
        assert_eq!((now.need as f64 / GIB * 10.0).round() / 10.0, 13.3);
        assert_eq!((now.budget as f64 / GIB * 10.0).round() / 10.0, 91.3);
        // At its own size the plan is green, and vLLM asks for 30.4 + 4 GiB free, not 95.3.
        let at = memory_plan(&DeploySpec { gpu_memory_utilization: 0.25, ..s.clone() }, &needs, &[&cell1], &[]).remove(0);
        assert_eq!((at.level, at.verdict.as_str(), at.reason.as_deref()), (Level::Green, "Fits", None));
        assert_eq!((at.budget as f64 / GIB * 10.0).round() / 10.0, 30.4);
        assert_eq!((at.after_pct).round(), 37.0);
        assert_eq!(suggested_share(&DeploySpec { gpu_memory_utilization: 0.25, ..s.clone() }, &[at]), None, "nothing to suggest at its own size");

        // Its example: 13 GiB needed, 30 reserved, and the KV cache gets the rest: 20.6 GiB, over
        // 3.5 a request. The request count holds with Requests at once set, too.
        let tight = DeploySpec { gpu_memory_utilization: 0.25, ..s.clone() };
        assert_eq!(full_requests(&tight, &memory_plan(&tight, &needs, &[&cell1], &[])), Some(5));
        let eight = DeploySpec { max_num_seqs: Some(8), gpu_memory_utilization: 0.35, ..s.clone() };
        assert_eq!(full_requests(&eight, &memory_plan(&eight, &needs, &[&cell1], &[])), Some(9));
        assert_eq!(full_requests(&s, &memory_plan(&s, &needs, &[&cell1], &[])), Some(23), "at 0.75: why the owner saw 81.5 GiB of KV");
        let unknown = ModelNeeds { kv_bytes_per_token: None, ..needs.clone() };
        assert_eq!(full_requests(&s, &memory_plan(&s, &unknown, &[&cell1], &[])), None);
        assert_eq!(full_requests(&s, &[]), None);

        // The share on the page has no say in it, so following it can't chase itself.
        for share in [0.2, 0.25, 0.5, 0.75, 0.95] {
            assert_eq!(sized(&DeploySpec { gpu_memory_utilization: share, ..s.clone() }, &needs, &[&cell1]), Some(0.25), "from {share}");
        }
    }

    #[test]
    fn the_sized_share_follows_context_requests_and_the_kv_type() {
        let cell1 = gb10("cell1");
        let needs = tiny_needs();
        let s = DeploySpec { max_model_len: Some(32768), ..spec(&["cell1"], 1, 1) };
        // Empty Max context plans for the model's own maximum, as the memory plan does.
        assert_eq!(sized(&DeploySpec { max_model_len: None, ..s.clone() }, &needs, &[&cell1]), Some(0.25));
        for (longer, want) in [(65536, 0.35), (131_072, 0.6)] {
            let t = DeploySpec { max_model_len: Some(longer), ..s.clone() };
            assert_eq!(sized(&t, &needs, &[&cell1]), Some(want), "{longer}");
            holds_and_is_the_least(&t, &needs, &[&cell1], want);
        }
        // Requests at once replaces the four, more or fewer: it's what the load will really serve.
        for (seqs, want) in [(1, 0.2), (8, 0.35), (16, 0.6)] {
            let t = DeploySpec { max_num_seqs: Some(seqs), ..s.clone() };
            assert_eq!(sized(&t, &needs, &[&cell1]), Some(want), "{seqs}");
        }
        // fp8 halves the KV cache: 3.8 + 6 + 7 = 16.8 GiB × 1.1 is 0.15 of the node.
        let fp8 = DeploySpec { kv_cache_dtype: Some("fp8".into()), max_num_seqs: Some(8), ..s.clone() };
        assert_eq!(sized(&fp8, &needs, &[&cell1]), Some(0.25));
        holds_and_is_the_least(&fp8, &needs, &[&cell1], 0.25);
        let state = ModelNeeds { arch: ArchFacts { state_bytes_per_seq: (2.0 * GIB) as u64, ..needs.arch.clone() }, ..needs.clone() };
        assert_eq!(sized(&s, &state, &[&cell1]), Some(0.3), "a per-sequence state counts for each request");
        holds_and_is_the_least(&s, &state, &[&cell1], 0.3);
    }

    /// The floor is there for the smallest models, whose 10% headroom is less than the error in
    /// the runtime figure: nothing gets below it, however little it needs.
    #[test]
    fn the_sized_share_stops_at_the_floor() {
        let cell1 = gb10("cell1");
        let small = ModelNeeds { weight_bytes: GIB as u64, ..tiny_needs() };
        let s = DeploySpec { max_model_len: Some(4096), ..spec(&["cell1"], 1, 1) };
        let wanted = memory_plan(&DeploySpec { max_num_seqs: Some(SIZED_REQUESTS), ..s.clone() }, &small, &[&cell1], &[])[0].need as f64 * SIZED_HEADROOM / cell1.mem.total as f64;
        assert!(wanted < 0.1, "{wanted}");
        assert_eq!(sized(&s, &small, &[&cell1]), Some(fit::MIN_SHARE));
        assert_eq!(fit::MIN_SHARE, 0.2);
    }

    /// Every split, through the plan's own per-node figures, so the shards can't drift apart.
    #[test]
    fn the_sized_share_divides_what_each_split_divides() {
        let (a, b, c, d) = (gb10("cell1"), gb10("cell2"), gb10("cell3"), gb10("cell4"));
        let dense = ArchFacts { layers: 42, kv_layers: 42, heads: 32, kv_heads: 16, head_dim: 128, max_context: Some(131_072), ..Default::default() };
        let dense = ModelNeeds { weight_bytes: 54_000_000_000, kv_bytes_per_token: fit::kv_bytes_per_token(&dense), arch: dense };
        assert_eq!(dense.kv_bytes_per_token, Some(344_064));
        let base = |nodes: &[&str], tp, pp| DeploySpec { max_model_len: Some(32768), ..spec(nodes, tp, pp) };
        let two = [&a, &b];
        let all = [&a, &b, &c, &d];

        // One node: 50.3 weights + 6 + 4 × 10.5 KV is 98.3 GiB, 0.89 with the headroom: too big
        // to size, so it stays at the first-load share with today's suggestions.
        assert_eq!(sized(&base(&["cell1"], 1, 1), &dense, &[&a]), None);
        // Tensor parallel 2: half the weights and, by KV head, half the cache.
        let tp2 = base(&["cell1", "cell2"], 2, 1);
        assert_eq!(kv_ways(&tp2, &dense), 2);
        assert_eq!(sized(&tp2, &dense, &two), Some(0.5));
        holds_and_is_the_least(&tp2, &dense, &two, 0.5);
        // Pipeline parallel 2 divides the layers, so the same here.
        let pp2 = base(&["cell1", "cell2"], 1, 2);
        assert_eq!(kv_ways(&pp2, &dense), 2);
        assert_eq!(sized(&pp2, &dense, &two), Some(0.5));
        holds_and_is_the_least(&pp2, &dense, &two, 0.5);
        // Four nodes, by tensor parallel, by pipeline parallel and by both.
        for (tp, pp) in [(4, 1), (1, 4), (2, 2)] {
            let s = base(&["cell1", "cell2", "cell3", "cell4"], tp, pp);
            assert_eq!(kv_ways(&s, &dense), 4, "{tp}x{pp}");
            assert_eq!(sized(&s, &dense, &all), Some(0.3), "{tp}x{pp}");
            holds_and_is_the_least(&s, &dense, &all, 0.3);
        }

        // MLA keeps one latent cache, copied to every rank under tensor parallel, while pipeline
        // parallel still divides the layers: the two ways come apart.
        let arch = ArchFacts { layers: 80, kv_layers: 80, heads: 64, kv_heads: 64, mla_dim: Some(1088), max_context: Some(131_072), ..Default::default() };
        let mla = ModelNeeds { weight_bytes: 54_000_000_000, kv_bytes_per_token: fit::kv_bytes_per_token(&arch), arch };
        let (tp2, pp2) = (base(&["cell1", "cell2"], 2, 1), base(&["cell1", "cell2"], 1, 2));
        assert_eq!((kv_ways(&tp2, &mla), kv_ways(&pp2, &mla)), (1, 2));
        let (t, p) = (sized(&tp2, &mla, &two).unwrap(), sized(&pp2, &mla, &two).unwrap());
        assert!(t > p, "{t} {p}");
        holds_and_is_the_least(&tp2, &mla, &two, t);
        holds_and_is_the_least(&pp2, &mla, &two, p);

        // KV heads beyond the head count are copied too: one KV head stays whole across nodes.
        let arch = ArchFacts { layers: 42, kv_layers: 42, heads: 32, kv_heads: 1, head_dim: 128, max_context: Some(131_072), ..Default::default() };
        let one_head = ModelNeeds { weight_bytes: 54_000_000_000, kv_bytes_per_token: fit::kv_bytes_per_token(&arch), arch };
        assert_eq!(kv_ways(&tp2, &one_head), 1);
        let t = sized(&tp2, &one_head, &two).unwrap();
        holds_and_is_the_least(&tp2, &one_head, &two, t);
    }

    #[test]
    fn the_sized_share_answers_for_the_worst_node() {
        let (a, b) = (gb10("cell1"), gb10("cell2"));
        let needs = tiny_needs();
        let s = DeploySpec { max_model_len: Some(32768), ..spec(&["cell1", "cell2"], 2, 1) };
        let bare = sized(&s, &needs, &[&a, &b]).unwrap();
        assert_eq!(bare, 0.2);

        // A vision encoder sits on the head alone, and sizes the share for both.
        let arch = ArchFacts { vision_bytes: (24.0 * GIB) as u64, ..needs.arch.clone() };
        let vision = ModelNeeds { arch, ..needs.clone() };
        let plan = memory_plan(&s, &vision, &[&a, &b], &[]);
        assert!(plan[0].overhead > plan[1].overhead);
        assert_eq!(sized(&s, &vision, &[&a, &b]), Some(0.4));
        holds_and_is_the_least(&s, &vision, &[&a, &b], 0.4);
        // The head has the least KV room, and it's what the load can serve.
        let at = DeploySpec { gpu_memory_utilization: 0.4, ..s.clone() };
        let plan = memory_plan(&at, &vision, &[&a, &b], &[]);
        let room = |p: &NodePlan| (p.budget - p.weights - p.overhead) / p.kv;
        assert!(room(&plan[0]) < room(&plan[1]));
        assert_eq!(full_requests(&at, &plan), Some(room(&plan[0])));

        // One smaller node decides for the set: the same bytes are more of its memory.
        let mut small = gb10("cell2");
        small.mem.total = (60.0 * GIB) as u64;
        let split = DeploySpec { max_model_len: Some(131_072), ..s.clone() };
        let share = sized(&split, &needs, &[&a, &small]).unwrap();
        assert_eq!(share, sized(&split, &needs, &[&small, &small]).unwrap());
        assert!(share > sized(&split, &needs, &[&a, &b]).unwrap());
    }

    #[test]
    fn the_sized_share_never_passes_what_fits() {
        let needs = tiny_needs();
        let s = DeploySpec { max_model_len: Some(32768), ..spec(&["gx10-0000"], 1, 1) };

        // Next to Ollama the model's own 0.25 is well inside the 0.40 that's safe.
        let shared = node("gx10-0000", &[("ollama", 44_560_000_000)], &[]);
        let p = memory_plan(&s, &needs, &[&shared], &[]).remove(0);
        assert_eq!((p.safe_share, p.max_share), (0.4, 0.6));
        assert_eq!(sized(&s, &needs, &[&shared]), Some(0.25));

        // A model that wants more stops at the safe share, while its plan still fits there.
        let t = DeploySpec { max_num_seqs: None, max_model_len: Some(65536), ..s.clone() };
        let bigger = ModelNeeds { weight_bytes: 30_000_000_000, ..needs.clone() };
        let wanted = sized(&t, &bigger, &[&node("gx10-0000", &[], &[])]).unwrap();
        assert!(wanted > 0.4, "{wanted}");
        assert_eq!(sized(&t, &bigger, &[&shared]), Some(0.4));
        let at = memory_plan(&DeploySpec { gpu_memory_utilization: 0.4, ..t.clone() }, &bigger, &[&shared], &[]).remove(0);
        assert_eq!(at.level, Level::Green);

        // A model that doesn't fit there gets no share: the page stays at the first-load share
        // and today's suggestions answer.
        let too_big = ModelNeeds { weight_bytes: 40_000_000_000, ..needs.clone() };
        let p = memory_plan(&t, &too_big, &[&shared], &[]).remove(0);
        assert!(!p.fits_safe);
        assert_eq!(sized(&t, &too_big, &[&shared]), None);
        assert!(suggested_context(&t, &[p]).is_some());

        // Nothing fits at all on a node this full: not even the floor is on offer.
        let full = node("gx10-0000", &[("ollama", 95_000_000_000)], &[]);
        let p = memory_plan(&s, &needs, &[&full], &[]).remove(0);
        assert!(p.safe_share < fit::MIN_SHARE, "{}", p.safe_share);
        assert_eq!(sized(&s, &needs, &[&full]), None);

        // One busy node holds back the set.
        let (quiet, busy) = (gb10("cell1"), node("cell2", &[("ollama", 44_560_000_000)], &[]));
        let two = DeploySpec { max_model_len: Some(131_072), ..spec(&["cell1", "cell2"], 2, 1) };
        let plan = memory_plan(&two, &bigger, &[&quiet, &busy], &[]);
        assert_eq!(sized_share(&two, &bigger, &plan), Some(0.4));
    }

    #[test]
    fn a_model_too_big_or_unknown_stays_at_the_first_load_share() {
        let cell1 = gb10("cell1");
        let s = DeploySpec { max_model_len: Some(32768), gpu_memory_utilization: fit::FIRST_LOAD_SHARE, ..spec(&["cell1"], 1, 1) };
        // No KV figure, no share: its size can't be known.
        let unknown = ModelNeeds { kv_bytes_per_token: None, ..tiny_needs() };
        assert_eq!(sized(&s, &unknown, &[&cell1]), None);
        assert_eq!(sized_share(&s, &tiny_needs(), &[]), None);
        // No context chosen and none in its config, as gemma's reads: the plan's 8,192 is a guess.
        let mut no_max = tiny_needs();
        no_max.arch.max_context = None;
        assert_eq!(sized(&DeploySpec { max_model_len: None, ..s.clone() }, &no_max, &[&cell1]), None);
        assert_eq!(sized(&s, &no_max, &[&cell1]), Some(0.25), "a context chosen is enough");
        // 70 GiB + 6 + 14 × 1.1 is over 0.8 of the node: above the first-load share, so no.
        let big = ModelNeeds { weight_bytes: (70.0 * GIB) as u64, ..tiny_needs() };
        assert_eq!(sized(&s, &big, &[&cell1]), None);
        // Above it by the requests at once alone: the same model and context, sized for fewer.
        let many = DeploySpec { max_num_seqs: Some(32), ..s.clone() };
        assert_eq!(sized(&many, &tiny_needs(), &[&cell1]), None);
        // Today's answers stand for it.
        let p = memory_plan(&s, &big, &[&cell1], &[]).remove(0);
        assert_eq!((p.level, suggested_share(&s, &[p])), (Level::Green, None));
        // Just inside it, it's still sized: there's nothing to lose by saying so.
        let inside = ModelNeeds { weight_bytes: (60.0 * GIB) as u64, ..tiny_needs() };
        assert_eq!(sized(&s, &inside, &[&cell1]), Some(fit::FIRST_LOAD_SHARE));
    }

    /// A share the model fills to the byte is no fit: the KV cache would have nothing to spare.
    #[test]
    fn zero_spare_is_not_a_fit() {
        let mut n = node("cell1", &[], &[]);
        n.mem.total = 100 * GIB as u64;
        let s = DeploySpec { gpu_memory_utilization: 0.5, ..spec(&["cell1"], 1, 1) };
        let exact = ModelNeeds { weight_bytes: 50 * GIB as u64 - fit::RUNTIME_OVERHEAD as u64, kv_bytes_per_token: None, ..Default::default() };
        let p = memory_plan(&s, &exact, &[&n], &[]).remove(0);
        assert_eq!((p.need, p.budget), (50 * GIB as u64, 50 * GIB as u64));
        assert_eq!((p.level, p.verdict.as_str()), (Level::Red, "Won't fit"));
        let under = ModelNeeds { weight_bytes: exact.weight_bytes - 1, ..exact.clone() };
        let q = memory_plan(&s, &under, &[&n], &[]).remove(0);
        assert_eq!((q.level, q.verdict.as_str()), (Level::Green, "Fits"), "{:?}", q.reason);
        // The same line for the safe share and the shares offered.
        let at_safe = ModelNeeds { weight_bytes: (n.mem.total as f64 * q.safe_share) as u64 - fit::RUNTIME_OVERHEAD as u64, ..exact.clone() };
        let r = memory_plan(&DeploySpec { gpu_memory_utilization: 0.9, ..s.clone() }, &at_safe, &[&n], &[]).remove(0);
        assert!(!r.fits_safe && r.need == (r.total as f64 * r.safe_share) as u64, "{r:?}");
        assert_eq!(suggested_share(&DeploySpec { gpu_memory_utilization: 0.9, ..s.clone() }, &[r]), None);
        let tiny = tiny_needs();
        let t = DeploySpec { max_model_len: Some(32768), ..s.clone() };
        let plan = memory_plan(&t, &tiny, &[&n], &[]);
        let sized = sized_share(&t, &tiny, &plan).unwrap();
        let full = DeploySpec { max_num_seqs: Some(SIZED_REQUESTS), ..t.clone() };
        let at = memory_plan(&DeploySpec { gpu_memory_utilization: sized, ..full }, &tiny, &[&n], &[]).remove(0);
        assert!(at.need < at.budget, "the sized share leaves room past its four requests: {at:?}");
    }

    /// A model too big to size opens at the first-load share even where that share is over the
    /// line: the plan says so, and the one share that fits is still offered (the review's probe).
    #[test]
    fn the_fallback_share_still_gets_the_verdict_and_the_offer() {
        let mut busy = gb10("cell1");
        busy.mem.used = (14.0 * GIB) as u64;
        let s = DeploySpec { max_model_len: Some(32768), gpu_memory_utilization: fit::FIRST_LOAD_SHARE, ..spec(&["cell1"], 1, 1) };
        let m = ModelNeeds { weight_bytes: (66.0 * GIB) as u64, ..tiny_needs() };
        let plan = memory_plan(&s, &m, &[&busy], &[]);
        assert_eq!(sized_share(&s, &m, &plan), None, "too big to size");
        assert_eq!(plan[0].safe_share, 0.7, "and 0.75 is over the line here");
        assert_eq!((plan[0].level, plan[0].verdict.as_str()), (Level::Amber, "Tight"));
        assert_eq!(suggested_share(&s, &plan), Some(0.7));
        let at = memory_plan(&DeploySpec { gpu_memory_utilization: 0.7, ..s.clone() }, &m, &[&busy], &[]).remove(0);
        assert_eq!(at.level, Level::Green, "{:?}", at.reason);
        // A smaller model on the same node is held at the safe share instead, with no offer to make.
        let small = ModelNeeds { weight_bytes: (60.0 * GIB) as u64, ..tiny_needs() };
        let plan = memory_plan(&s, &small, &[&busy], &[]);
        assert_eq!(sized_share(&s, &small, &plan), Some(0.7));
        let at = memory_plan(&DeploySpec { gpu_memory_utilization: 0.7, ..s.clone() }, &small, &[&busy], &[]);
        assert_eq!((at[0].level, suggested_share(&DeploySpec { gpu_memory_utilization: 0.7, ..s.clone() }, &at)), (Level::Green, None));
    }

    /// A safe share under the floor is no answer even where the model would fit in it (the
    /// review's R8 and R19): the floor is the least a load can take.
    #[test]
    fn nothing_is_offered_below_the_floor() {
        let needs = tiny_needs();
        // Ollama leaves room for 0.15: the model's 13.3 GiB fits in 18.3, but no load can take 0.15.
        let shared = node("gx10-0000", &[("ollama", 81_000_000_000)], &[]);
        let s = DeploySpec { max_model_len: Some(32768), ..spec(&["gx10-0000"], 1, 1) };
        let plan = memory_plan(&s, &needs, &[&shared], &[]);
        assert_eq!(plan[0].safe_share, 0.15, "{plan:?}");
        assert!((plan[0].need as f64) < plan[0].total as f64 * 0.15, "it would fit there");
        assert_eq!(sized_share(&s, &needs, &plan), None);
        assert_eq!(suggested_share(&s, &plan), None);
        // Nor a shorter context at that share.
        let long = DeploySpec { max_model_len: Some(1_000_000), ..s.clone() };
        let plan = memory_plan(&long, &needs, &[&shared], &[]);
        assert!(!plan[0].fits_safe && plan[0].safe_context.is_some_and(|c| c >= 4096), "{plan:?}");
        assert_eq!(suggested_context(&long, &plan), None);
    }

    /// The field, `validate` and the sized share must agree, or a share the page offers is refused.
    #[test]
    fn a_sized_share_is_one_a_load_can_take() {
        let s = DeploySpec { max_model_len: Some(32768), ..gemma(true) };
        assert_eq!(validate(&DeploySpec { gpu_memory_utilization: 0.25, ..s.clone() }), Ok(()));
        assert_eq!(validate(&DeploySpec { gpu_memory_utilization: fit::MIN_SHARE, ..s.clone() }), Ok(()));
        let low = validate(&DeploySpec { gpu_memory_utilization: 0.15, ..s.clone() });
        assert_eq!(low.unwrap_err(), "GPU memory utilization must be between 0.2 and 0.95.");
        assert!(validate(&DeploySpec { gpu_memory_utilization: 0.96, ..s.clone() }).is_err());
        assert!(rank_argv(&DeploySpec { gpu_memory_utilization: 0.25, ..s }, 0, &[]).windows(2).any(|w| w[0] == "--gpu-memory-utilization" && w[1] == "0.25"));
    }

    /// A share below the old 0.3 is a real answer now, so a busy node can be offered one.
    #[test]
    fn a_suggestion_can_go_as_low_as_a_load_can() {
        let needs = tiny_needs();
        let s = DeploySpec { max_model_len: Some(8192), ..spec(&["gx10-0000"], 1, 1) };
        let busy = node("gx10-0000", &[("ollama", 66_000_000_000)], &[]);
        let p = memory_plan(&s, &needs, &[&busy], &[]).remove(0);
        assert_eq!(p.safe_share, 0.25);
        assert_eq!(suggested_share(&s, &[p]), Some(0.25));
        let full = node("gx10-0000", &[("ollama", 78_000_000_000)], &[]);
        let p = memory_plan(&s, &needs, &[&full], &[]).remove(0);
        assert!(p.safe_share < fit::MIN_SHARE, "{}", p.safe_share);
        assert_eq!(suggested_share(&s, &[p]), None);
    }

    #[test]
    fn deployment_states_read_and_compare_as_words() {
        assert_eq!(DeploymentState::from("ready"), DeploymentState::Ready);
        assert!(DeploymentState::Ready == "ready");
        assert_eq!(serde_json::to_string(&DeploymentState::Stopped).unwrap(), "\"stopped\"");
        assert!(DeploymentState::Starting.active() && !DeploymentState::Failed.active());
    }

    // ---- The contracts, byte for byte (C-U2, C-S, C-P2)

    /// The contracts' test load: gemma-3-27b-it on cell2, deployment d1, port 8000, share 0.75.
    fn gemma(require_key: bool) -> DeploySpec {
        DeploySpec {
            id: "d1".into(),
            served_name: "gemma-3-27b-it".into(),
            model_path: "/home/sparkuser/models/google/gemma-3-27b-it".into(),
            nodes: vec!["cell2".into()],
            max_model_len: Some(32768),
            port: 8000,
            host: "192.168.50.154".into(),
            image: DEFAULT_IMAGE.into(),
            image_ref: DEFAULT_IMAGE.into(),
            image_id: local_id(),
            require_key,
            ..Default::default()
        }
    }

    #[test]
    fn the_first_load_share_is_the_fallback() {
        assert_eq!(fit::FIRST_LOAD_SHARE, 0.75);
        assert_eq!(DeploySpec::default().gpu_memory_utilization, 0.75, "what a load starts from until the plan sizes it");
        assert!(validate(&gemma(true)).is_ok(), "{:?}", validate(&gemma(true)));
    }

    #[test]
    fn sglang_lost_the_shm_size_and_keeps_its_command() {
        let sg = sglang_command(&DeploySpec { engine: "sglang".into(), require_key: false, image: "lmsysorg/sglang:spark".into(), ..spec(&["cell1"], 1, 1) }, 0, "127.0.0.1", None);
        assert!(!sg.contains("--shm-size"), "{sg}");
        assert!(sg.contains("--network host --ipc host --gpus all --restart no"), "{sg}");
        assert!(!sg.contains("HF_HUB_OFFLINE") && !sg.contains("--label sparkbox.image=") && !sg.contains(".cache/vllm"), "{sg}");
    }

    #[test]
    fn unload_and_stop_role_are_c_u2_and_c_s_byte_for_byte() {
        assert_eq!(
            unload_command_for("d1", "release"),
            r#"ids=$(docker ps -aq --filter label=sparkbox.instance=release --filter label=sparkbox.deployment=d1); [ -z "$ids" ] || docker stop -t 30 $ids >/dev/null; [ -z "$ids" ] || docker rm $ids >/dev/null; rm -rf ~/.sparkbox/keys/d1 ~/.sparkbox/deploy/d1; echo ok"#
        );
        assert_eq!(
            stop_role_command_for("d1", "head", "release"),
            r#"ids=$(docker ps -aq --filter label=sparkbox.instance=release --filter label=sparkbox.deployment=d1 --filter label=sparkbox.role=head); [ -z "$ids" ] || docker stop -t 30 $ids >/dev/null; [ -z "$ids" ] || docker rm $ids >/dev/null; echo ok"#
        );
        // The id goes to rm -rf, so what isn't one never reaches a shell.
        for bad in ["..", ".", "../..", "a/b", "", "-rf", "a b", "$(id)", "a;b"] {
            assert_eq!(unload_command_for(bad, "release"), "echo 'not a load id' >&2; exit 1", "{bad}");
            assert!(validate(&DeploySpec { id: bad.into(), ..gemma(true) }).unwrap_err().contains("bad deployment id"), "{bad}");
        }
        for cmd in [unload_command("d1"), stop_role_command("d1", "worker")] {
            assert!(!cmd.contains("rm -f $ids") && !cmd.contains("rm -f \"$ids\""), "never docker rm -f: {cmd}");
            assert!(cmd.find("docker stop -t 30").unwrap() < cmd.find("docker rm").unwrap(), "{cmd}");
        }
    }

    const C_P2: &str = r#"echo '--- mem'; grep -E '^(MemTotal|MemFree|MemAvailable|Cached):' /proc/meminfo; echo '--- earlyoom'; systemctl is-active earlyoom; cat /etc/default/earlyoom 2>&1; echo '--- image'; docker image inspect -f '{{.Id}}' 'vllm/vllm-openai:glm53-flash-arm64-cu130' 2>&1; echo '--- store'; docker info -f '{{.Driver}} {{json .DriverStatus}}' 2>&1; echo '--- model'; test -f '/home/sparkuser/models/google/gemma-3-27b-it/config.json' && test ! -e '/home/sparkuser/models/google/gemma-3-27b-it/.sparkbox.partial' && echo present; echo '--- port'; ss -Hltn '( sport = :8000 or sport = :29501 )'; echo '--- shm'; ls -la /dev/shm; echo '--- caches'; ls -ld ~/.cache/vllm ~/.cache/flashinfer ~/.triton 2>&1; echo '--- containers'; docker ps -a --filter label=sparkbox.deployment --format '{{.ID}}|{{.Names}}|{{.Label "sparkbox.deployment"}}|{{.Label "sparkbox.instance"}}|{{.Status}}' 2>&1; echo '--- ray'; docker ps -a --no-trunc --format '{{.ID}}|{{.Names}}|{{.Status}}|{{.Command}}' 2>&1 | grep -e 'ray start --address'; echo '--- ollama'; curl -s -m 3 http://127.0.0.1:11434/api/ps 2>&1; true"#;

    #[test]
    fn the_preflight_read_is_c_p2_byte_for_byte() {
        assert_eq!(preflight_script(&DeploySpec { nodes: vec!["cell1".into(), "cell2".into()], ..gemma(true) }), C_P2);
        // A single-node spec watches its own port alone.
        assert_eq!(preflight_script(&gemma(true)), C_P2.replace("( sport = :8000 or sport = :29501 )", "sport = :8000"));
        // A load saved before images were picked names its image.
        let old = DeploySpec { image_ref: String::new(), image: "nvcr.io/nvidia/vllm:26.05-py3".into(), ..gemma(true) };
        assert!(preflight_script(&old).contains("docker image inspect -f '{{.Id}}' 'nvcr.io/nvidia/vllm:26.05-py3' 2>&1"));
        assert_eq!(DROP_CACHES, "sync; echo 3 > /proc/sys/vm/drop_caches");
        // A several-node load asks the same thing, and it's a read too.
        let check = model_ready_test("/home/sparkuser/models/org/m");
        assert_eq!(check, "test -f '/home/sparkuser/models/org/m/config.json' && test ! -e '/home/sparkuser/models/org/m/.sparkbox.partial'");
        assert!(crate::ops::classify::read_only(&check), "{check}");
    }

    #[test]
    fn the_image_is_its_local_id_and_its_name_plain() {
        assert!(validate(&gemma(true)).is_ok());
        for bad in ["5a0f8b914da5", "sha256:5a0f8b914da5", &format!("sha256:{}", "A".repeat(64)), &format!("sha256:{}g", "a".repeat(63)), &format!("sha512:{}", "a".repeat(64)), &format!("{} ", local_id())] {
            let e = validate(&DeploySpec { image_id: bad.to_string(), ..gemma(true) }).unwrap_err();
            assert!(e.contains("sha256: and 64 hex digits"), "{bad}: {e}");
        }
        for bad in ["vllm/vllm-openai:x;reboot", "vllm $(id)", "a'b", "img\nname", "a`b`", "x|y"] {
            let e = validate(&DeploySpec { image_ref: bad.into(), ..gemma(true) }).unwrap_err();
            assert!(e.contains("letters, digits and . _ / : @ -"), "{bad:?}: {e}");
        }
        assert!(validate(&DeploySpec { image_ref: "nvcr.io/nvidia/vllm@sha256:0123".into(), ..gemma(true) }).is_ok());
        assert!(is_image_id(&format!("sha256:{}", "0123456789abcdef".repeat(4))));
    }

    /// cell2's real numbers (the Oct 29 doctor capture: MemTotal 127598748 kB).
    fn preflight_out(mem_free_kb: u64, earlyoom: &str, image: &str, model: bool, port: &str, shm: &str, caches: &str) -> String {
        format!(
            "--- mem\nMemTotal:       127598748 kB\nMemFree:        {mem_free_kb} kB\nMemAvailable:   122356776 kB\nCached:          3196204 kB\n--- earlyoom\n{earlyoom}\n--- image\n{image}\n--- store\n{OVERLAY2}\n--- model\n{}--- port\n{port}--- shm\n{shm}--- caches\n{caches}--- containers\n--- ray\n--- ollama\n{{\"models\":[]}}\n",
            if model { "present\n" } else { "" }
        )
    }

    const OVERLAY2: &str = r#"overlay2 [["Backing Filesystem","extfs"],["Supports d_type","true"],["Using metacopy","false"],["Native Overlay Diff","true"],["userxattr","false"]]"#;
    const CONTAINERD: &str = r#"overlayfs [["driver-type","io.containerd.snapshotter.v1"]]"#;

    const EARLYOOM_ON: &str = "active\n# Default settings for earlyoom. This file is sourced by /bin/sh from\n# /etc/init.d/earlyoom or by systemd from earlyoom.service.\n\n# Options to pass to earlyoom\nEARLYOOM_ARGS=\"-r 3600 -n --avoid '(^|/)(init|systemd|Xorg|sshd)$'\"\n\n# Examples:\n# --- a line like a header in a comment\n";
    const SHM_EMPTY: &str = "total 0\ndrwxrwxrwt  2 root root   40 Oct 29 09:33 .\ndrwxr-xr-x 20 root root 4600 Oct 29 09:33 ..\n";
    const CACHES_FRESH: &str = "ls: cannot access '/home/sparkuser/.cache/vllm': No such file or directory\nls: cannot access '/home/sparkuser/.cache/flashinfer': No such file or directory\nls: cannot access '/home/sparkuser/.triton': No such file or directory\n";

    #[test]
    fn the_preflight_reads_what_ubuntu_prints() {
        let id5a = "sha256:5a0f8b914da56ea2ae2dbe569e4219a14605ce798e7eff12ae6db63328adc4f1";
        let out = preflight_out(119_861_600, EARLYOOM_ON, id5a, true, "", SHM_EMPTY, CACHES_FRESH);
        let p = parse_preflight("cell2", &gemma(true), &out);
        assert_eq!(p.mem, Some(Mem { total: 127_598_748 * 1024, free: 119_861_600 * 1024, available: 122_356_776 * 1024, cached: 3_196_204 * 1024 }));
        // 0.75 × 121.7 GiB + 4 GiB.
        assert_eq!(p.need_free, (127_598_748.0 * 1024.0 * 0.75) as u64 + FREE_HEADROOM);
        assert_eq!(gib_text(p.need_free), "95.3 GiB");
        assert!(!p.drop_needed, "114.3 GiB free is enough");
        let eo = p.earlyoom.clone().unwrap();
        assert!(eo.active && eo.state == "active");
        assert_eq!(eo.config.as_deref(), Some("EARLYOOM_ARGS=\"-r 3600 -n --avoid '(^|/)(init|systemd|Xorg|sshd)$'\""));
        assert_eq!(p.image_id.as_deref(), Some(id5a));
        assert!(p.model_present && p.port_free && p.port_owner.is_none());
        assert!(p.shm.is_empty(), "{:?}", p.shm);
        assert_eq!(p.caches.iter().map(|c| (c.path.as_str(), c.exists)).collect::<Vec<_>>(), [("~/.cache/vllm", false), ("~/.cache/flashinfer", false), ("~/.triton", false)]);
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert!(!p.demo);

        // earlyoom off, and not installed (systemd says inactive; there's no config file).
        let off = parse_preflight("cell2", &gemma(true), &preflight_out(1, "inactive\nEARLYOOM_ARGS=\"-r 60\"", id5a, true, "", SHM_EMPTY, CACHES_FRESH));
        assert_eq!(off.earlyoom, Some(Earlyoom { active: false, state: "inactive".into(), config: Some("EARLYOOM_ARGS=\"-r 60\"".into()) }));
        let none = parse_preflight("cell3", &gemma(true), &preflight_out(1, "inactive\ncat: /etc/default/earlyoom: No such file or directory", id5a, true, "", SHM_EMPTY, CACHES_FRESH));
        assert_eq!(none.earlyoom, Some(Earlyoom { active: false, state: "inactive".into(), config: None }));
        assert!(none.problems.is_empty(), "earlyoom never blocks: {:?}", none.problems);

        // The image isn't there: Docker 29's own words, and the refusal says what to do.
        let missing = parse_preflight("cell2", &gemma(true), &preflight_out(119_861_600, EARLYOOM_ON, "Error: No such image: vllm/vllm-openai:glm53-flash-arm64-cu130", true, "", SHM_EMPTY, CACHES_FRESH));
        assert_eq!(missing.image_id, None);
        assert_eq!(
            missing.problems,
            ["cell2 doesn't have vllm/vllm-openai:glm53-flash-arm64-cu130. Pull it first (Fleet: docker pull vllm/vllm-openai:glm53-flash-arm64-cu130), or pick an image that's there."]
        );

        // The model folder missing, and port 8000 taken (ss -Hltn without owners).
        let taken = parse_preflight(
            "cell2",
            &gemma(true),
            &preflight_out(119_861_600, EARLYOOM_ON, id5a, false, "LISTEN 0      4096         0.0.0.0:8000       0.0.0.0:*    \n", SHM_EMPTY, CACHES_FRESH),
        );
        assert!(!taken.model_present && !taken.port_free);
        assert_eq!(taken.port_owner.as_deref(), Some("listening on 0.0.0.0:8000"));
        assert_eq!(
            taken.problems,
            [
                "cell2 doesn't have /home/sparkuser/models/google/gemma-3-27b-it yet. Copy it to that node first (Library › Copy to nodes…).",
                "Port 8000 is already in use on cell2 (listening on 0.0.0.0:8000). Pick another port.",
            ]
        );

        // /dev/shm with vLLM's leftovers, and the cache folders: one root-owned, one the user's, one missing.
        let shm = "total 2097160\ndrwxrwxrwt  2 root   root          80 Oct 30 12:00 .\ndrwxr-xr-x 20 root   root        4600 Oct 29 09:33 ..\n-rw-------  1 root   root  2147483648 Oct 30 12:00 psm_5f3c2a91\n-rw-r--r--  1 sparkuser sparkuser       16 Oct 30 12:00 sem.mp-x 1\n";
        let caches = "ls: cannot access '/home/sparkuser/.triton': No such file or directory\ndrwxr-xr-x 3 root   root   4096 Oct 30 12:00 /home/sparkuser/.cache/vllm\ndrwxrwxr-x 2 sparkuser sparkuser 4096 Oct 30 12:01 /home/sparkuser/.cache/flashinfer\n";
        let p = parse_preflight("cell2", &gemma(true), &preflight_out(119_861_600, EARLYOOM_ON, id5a, true, "", shm, caches));
        assert_eq!(p.shm, ["psm_5f3c2a91", "sem.mp-x 1"]);
        assert_eq!(shm_listing(&shm.lines().collect::<Vec<_>>()), [("psm_5f3c2a91".to_string(), 2_147_483_648), ("sem.mp-x 1".to_string(), 16)]);
        assert_eq!(
            p.caches,
            [
                CacheDir { path: "~/.cache/vllm".into(), exists: true, owner: Some("root".into()) },
                CacheDir { path: "~/.cache/flashinfer".into(), exists: true, owner: Some("sparkuser".into()) },
                CacheDir { path: "~/.triton".into(), exists: false, owner: None },
            ]
        );

        // Nothing read (a node that answered nothing): it says so, and nothing is assumed free.
        let blank = parse_preflight("cell2", &gemma(true), "");
        assert_eq!(blank.mem, None);
        assert!(blank.cut_off);
        assert_eq!(blank.problems, ["Couldn't finish reading cell2: it stopped at the memory, and nothing from there on was checked. Check again."]);
        assert!(!blank.model_present && blank.image_id.is_none());

        // A read section whose figures can't be used is a different thing.
        let garbled = parse_preflight("cell2", &gemma(true), &with_section("mem", "cat: /proc/meminfo: Permission denied\n"));
        assert!(!garbled.cut_off);
        assert_eq!(garbled.problems, ["Couldn't read cell2's memory (/proc/meminfo)."]);
    }

    /// `out`, up to this section's header.
    fn cut_before(out: &str, section: &str) -> String {
        out[..out.find(&format!("--- {section}\n")).unwrap()].to_string()
    }

    /// C-P2: a read that stopped says so, whatever it did or didn't reach, so no section can read as
    /// clear for want of being read. Ollama's model is there to be seen: only a whole read shows it.
    #[test]
    fn a_read_cut_off_anywhere_is_never_ready() {
        let ps = r#"{"models":[{"name":"qwen3:27b"}]}"#;
        let whole = with_section("ollama", &format!("{ps}\n"));
        let spec = two_nodes();
        let judged = |read: Preflight| {
            let store = read.store.clone();
            let head = Preflight { node: "cell1".into(), image_id: Some(ID5A.into()), store, ..Default::default() };
            PreflightSet::judge(&spec, vec![head, read], 0)
        };
        for (i, section) in PREFLIGHT_SECTIONS.iter().enumerate() {
            // Before its header, the last section read is the one before.
            let p = read_preflight("cell2", &spec, &cut_before(&whole, section), None);
            let stopped = PREFLIGHT_WORDS[i.saturating_sub(1)];
            assert!(p.cut_off, "{section}");
            assert_eq!(p.problems, [format!("Couldn't finish reading cell2: it stopped at {stopped}, and nothing from there on was checked. Check again.")], "{section}");
            assert_eq!(judged(p).verdict, "blocked", "{section}");
            // A missing header is enough: exit 0 doesn't excuse it.
            assert!(read_preflight("cell2", &spec, &cut_before(&whole, section), Some(0)).cut_off, "{section}");
        }

        // Past the last header only the exit can tell: curl is still waiting for Ollama.
        let in_the_last = whole[..whole.find(ps).unwrap()].to_string();
        for code in [None, Some(255), Some(1)] {
            let p = read_preflight("cell2", &spec, &in_the_last, code);
            assert!(p.cut_off, "{code:?}");
            assert_eq!(p.problems, ["Couldn't finish reading cell2: it stopped at Ollama, and nothing from there on was checked. Check again."], "{code:?}");
            assert_eq!(judged(p).verdict, "blocked", "{code:?}");
        }

        // The same text from a read that finished: Ollama had nothing to say.
        let finished = read_preflight("cell2", &spec, &in_the_last, Some(0));
        assert!(!finished.cut_off && finished.problems.is_empty(), "{:?}", finished.problems);
        // And a whole read still says what it found.
        let p = read_preflight("cell2", &spec, &whole, Some(0));
        assert!(!p.cut_off);
        assert_eq!(p.problems, ["cell2 has qwen3:27b in memory in Ollama, and unified memory is shared. Unload it there first."]);
    }

    /// What stopped it may have nothing to do with what the node has: a cut read names no image or
    /// folder as missing.
    #[test]
    fn a_cut_read_doesnt_call_what_it_didnt_reach_missing() {
        let p = read_preflight("cell2", &two_nodes(), &cut_before(&idle(), "image"), None);
        assert_eq!(p.problems.len(), 1, "{:?}", p.problems);
        assert!(p.problems[0].contains("stopped at earlyoom"), "{:?}", p.problems);
    }

    const ID5A: &str = "sha256:5a0f8b914da56ea2ae2dbe569e4219a14605ce798e7eff12ae6db63328adc4f1";

    fn two_nodes() -> DeploySpec {
        DeploySpec { nodes: vec!["cell1".into(), "cell2".into()], tensor_parallel: 2, ..gemma(true) }
    }

    fn idle() -> String {
        preflight_out(119_861_600, EARLYOOM_ON, ID5A, true, "", SHM_EMPTY, CACHES_FRESH)
    }

    /// `idle()` with one section's body replaced.
    fn with_section(section: &str, body: &str) -> String {
        let out = idle();
        let head = format!("--- {section}\n");
        let a = out.find(&head).unwrap() + head.len();
        let b = out[a..].find("--- ").map(|i| a + i).unwrap_or(out.len());
        format!("{}{body}{}", &out[..a], &out[b..])
    }

    #[test]
    fn an_idle_node_reads_clean() {
        let p = parse_preflight("cell2", &two_nodes(), &idle());
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert!(p.leftovers.is_empty() && p.ray_leftovers.is_empty() && p.ollama_resident.is_empty() && p.fabric.is_empty());
        assert_eq!(p.store.as_deref(), Some("overlay2"));
        assert!(p.port_free);
    }

    #[test]
    fn the_image_store_is_read_as_docker_says_it() {
        let store = |line: &str| parse_preflight("cell1", &two_nodes(), &with_section("store", &format!("{line}\n"))).store;
        assert_eq!(store(OVERLAY2).as_deref(), Some("overlay2"));
        assert_eq!(store(CONTAINERD).as_deref(), Some("containerd"));
        assert_eq!(store("overlay2 null").as_deref(), Some("overlay2"), "an empty DriverStatus still names the driver");
        assert_eq!(store("Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?"), None);
        assert_eq!(store(""), None);
    }

    #[test]
    fn leftovers_are_listed_with_who_owns_them() {
        let body = format!(
            "abc123def456|sparkbox-d9|d9|{mine}|Exited (1) 2 hours ago\n0123456789ab|sparkbox-d8-r1|d8|release|Up 3 hours\nfedcba987654|sparkbox-old||  |Created\nCannot connect to the Docker daemon\n",
            mine = instance_name()
        );
        let p = parse_preflight("cell1", &two_nodes(), &with_section("containers", &body));
        assert_eq!(p.leftovers.len(), 3, "{:?}", p.leftovers);
        assert_eq!(p.leftovers[0], Leftover { id: "abc123def456".into(), name: "sparkbox-d9".into(), deployment: "d9".into(), instance: instance_name().into(), status: "Exited (1) 2 hours ago".into(), ours: true });
        assert_eq!((p.leftovers[1].instance.as_str(), p.leftovers[1].ours), ("release", false), "the other build's is reported, never ours to remove");
        assert_eq!((p.leftovers[2].deployment.as_str(), p.leftovers[2].ours), ("", false));
        assert!(p.problems.is_empty(), "mod.rs decides which are left over: {:?}", p.problems);
        assert_eq!(leftovers_block("cell1", "sparkbox-d9, sparkbox-d7"), "cell1 has containers left from an earlier load (sparkbox-d9, sparkbox-d7). Remove them first.");
    }

    #[test]
    fn a_running_ray_worker_refuses_and_an_exited_one_doesnt() {
        let up = "aaa|old-ray|Up 2 days|\"bash -c 'until ray start --address=192.168.100.1:6379 --node-ip-address=192.168.100.2 --block; do sleep 3; done'\"\n";
        let p = parse_preflight("cell3", &two_nodes(), &with_section("ray", up));
        assert_eq!(p.problems, ["cell3 has a Ray worker still looking for its head (old-ray). SparkBox didn't start it; remove it in Containers."]);
        assert_eq!((p.ray_leftovers[0].id.as_str(), p.ray_leftovers[0].status.as_str()), ("aaa", "Up 2 days"));
        let gone = parse_preflight("cell3", &two_nodes(), &with_section("ray", "aaa|old-ray|Exited (137) 3 days ago|\"x|y ray start --address=1\"\n"));
        assert!(gone.problems.is_empty() && gone.ray_leftovers.len() == 1, "{:?}", gone.problems);
    }

    #[test]
    fn ollama_in_memory_refuses_with_its_model() {
        let ps = r#"{"models":[{"name":"qwen3:27b","model":"qwen3:27b","size":21000000000,"expires_at":"2026-11-01T14:00:00Z"}]}"#;
        let p = parse_preflight("cell3", &two_nodes(), &with_section("ollama", &format!("{ps}\n")));
        assert_eq!(p.ollama_resident, ["qwen3:27b"]);
        assert_eq!(p.problems, ["cell3 has qwen3:27b in memory in Ollama, and unified memory is shared. Unload it there first."]);
        let two = r#"{"models":[{"name":"a"},{"model":"b"}]}"#;
        assert_eq!(parse_preflight("cell3", &two_nodes(), &with_section("ollama", &format!("{two}\n"))).ollama_resident, ["a", "b"]);
        for quiet in ["", "{\"models\":[]}\n", "<html>not ollama</html>\n"] {
            let p = parse_preflight("cell3", &two_nodes(), &with_section("ollama", quiet));
            assert!(p.ollama_resident.is_empty() && p.problems.is_empty(), "{quiet:?}: {:?}", p.problems);
        }
    }

    #[test]
    fn a_worker_cares_about_the_rendezvous_port_and_not_the_api_port() {
        let api = "LISTEN 0      4096         0.0.0.0:8000       0.0.0.0:*    \n";
        let meet = "LISTEN 0      4096   192.168.100.2:29501      0.0.0.0:*    \n";
        let at = |node: &str, port: &str| parse_preflight(node, &two_nodes(), &with_section("port", port));
        assert_eq!(at("cell1", api).problems, ["Port 8000 is already in use on cell1 (listening on 0.0.0.0:8000). Pick another port."], "the head serves there");
        assert!(at("cell2", api).problems.is_empty() && at("cell2", api).port_free, "a worker doesn't bind it");
        for node in ["cell1", "cell2"] {
            let p = at(node, meet);
            assert_eq!(p.problems, [format!("Port 29501, where a several-node load's ranks meet, is already in use on {node} (listening on 192.168.100.2:29501). Free it first.")]);
            assert!(!p.port_free);
        }
        assert_eq!(at("cell1", &format!("{api}{meet}")).problems.len(), 2);
        assert_eq!(at("cell1", "LISTEN 0 4096 [::]:8000 [::]:*\n").port_owner.as_deref(), Some("listening on [::]:8000"));
    }

    fn read(node: &str, section: &str, body: &str) -> Preflight {
        parse_preflight(node, &two_nodes(), &with_section(section, body))
    }

    #[test]
    fn the_set_has_one_verdict() {
        let s = two_nodes();
        let ready = PreflightSet::judge(&s, vec![read("cell1", "shm", SHM_EMPTY), read("cell2", "shm", SHM_EMPTY)], 42);
        assert_eq!((ready.verdict.as_str(), ready.image_id.as_deref(), ready.checked_at_ms), ("ready", Some(ID5A), 42));
        assert!(ready.problems.is_empty(), "{:?}", ready.problems);

        // One node's problem blocks the set.
        let image = format!("Error: No such image: {DEFAULT_IMAGE}\n");
        let blocked = PreflightSet::judge(&s, vec![read("cell1", "shm", SHM_EMPTY), read("cell2", "image", &image)], 0);
        assert_eq!(blocked.verdict, "blocked");
        assert_eq!(blocked.image_id, None);
        assert!(blocked.problems[0].starts_with("cell2 doesn't have"), "{:?}", blocked.problems);

        // Same store, different image.
        let other = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
        let differ = PreflightSet::judge(&s, vec![read("cell1", "shm", SHM_EMPTY), read("cell2", "image", other)], 0);
        assert_eq!(differ.problems, [format!("{DEFAULT_IMAGE} isn't the same image on every node: cell1 has 5a0f8b914da5, cell2 has 0123456789ab. Pull the same tag on each node.")]);
        assert_eq!(differ.image_id, None);

        // Different stores: ids aren't compared at all.
        let stores = PreflightSet::judge(&s, vec![read("cell1", "shm", SHM_EMPTY), read("cell2", "store", &format!("{CONTAINERD}\n"))], 0);
        assert_eq!(stores.problems, ["Different Docker image stores on cell1 and cell2; pull the same tag on each node."]);
        let both = PreflightSet::judge(&s, vec![read("cell1", "shm", SHM_EMPTY), {
            let mut p = read("cell2", "store", &format!("{CONTAINERD}\n"));
            p.image_id = Some(other.trim().into());
            p
        }], 0);
        assert_eq!(both.problems.len(), 1, "{:?}", both.problems);

        // Memory short with no consent is a problem; with consent it isn't.
        let short = |consent: bool| {
            let mut p = read("cell1", "shm", SHM_EMPTY);
            p.drop_needed = true;
            PreflightSet::judge(&DeploySpec { drop_caches: consent, ..s.clone() }, vec![p], 0)
        };
        assert_eq!(short(false).verdict, "blocked");
        assert!(short(false).problems[0].contains("Tick 'Drop the page cache first'"), "{:?}", short(false).problems);
        assert_eq!(short(true).verdict, "ready");

        // A store nobody could read: ids can't be compared, on several nodes. One node compares with nobody.
        let silent = PreflightSet::judge(&s, vec![read("cell1", "shm", SHM_EMPTY), read("cell2", "store", "")], 0);
        assert_eq!((silent.problems.len(), silent.image_id.as_deref()), (1, None));
        assert!(silent.problems[0].contains("image store on cell2"), "{:?}", silent.problems);
        let alone = PreflightSet::judge(&gemma(true), vec![read("cell2", "store", "")], 0);
        assert_eq!((alone.verdict.as_str(), alone.image_id.as_deref()), ("ready", Some(ID5A)));

        // Demo reads nothing.
        let demo = PreflightSet::judge(&s, vec![Preflight { node: "cell1".into(), demo: true, ..Default::default() }], 0);
        assert_eq!((demo.verdict.as_str(), demo.problems.len()), ("demo", 0));
        assert_eq!(serde_json::to_value(&demo).unwrap()["checkedAtMs"], 0);
    }

    #[test]
    fn the_other_refusals_are_word_for_word() {
        assert_eq!(jumbo_failed("cell2", "cell1", "enp1s0f1np1"), "The jumbo-frame path from cell2 to cell1 failed on enp1s0f1np1: the model's traffic can't cross the fabric.");
        assert_eq!(files_differ("cell3", 2, "model-00001.safetensors"), "The model's files differ on cell3: 2 files differ, first model-00001.safetensors.");
    }

    #[test]
    fn the_jumbo_path_is_c_j() {
        let halves = vec![("enp1s0f1np1".to_string(), "192.168.100.1".to_string()), ("enP2p1s0f1np1".to_string(), "192.168.101.1".to_string())];
        assert_eq!(
            jumbo_script(&halves),
            "ping -c 2 -W 2 -M do -s 8972 -I enp1s0f1np1 192.168.100.1 >/dev/null 2>&1 && echo half1 ok || echo half1 failed; ping -c 2 -W 2 -M do -s 8972 -I enP2p1s0f1np1 192.168.101.1 >/dev/null 2>&1 && echo half2 ok || echo half2 failed"
        );
        assert_eq!(jumbo_script(&halves[..1]), "ping -c 2 -W 2 -M do -s 8972 -I enp1s0f1np1 192.168.100.1 >/dev/null 2>&1 && echo half1 ok || echo half1 failed");
        assert_eq!(jumbo_script(&[]), "");
        // A name a shell would read is never sent: the half reads failed.
        assert_eq!(jumbo_script(&[("en;reboot".into(), "192.168.100.1".into())]), "echo half1 failed");
        assert_eq!(jumbo_script(&[("enp1".into(), "$(id)".into())]), "echo half1 failed");

        assert_eq!(jumbo_failures("half1 ok\nhalf2 ok\n", 2), Vec::<usize>::new());
        assert_eq!(jumbo_failures("half1 ok\nhalf2 failed\n", 2), [1]);
        assert_eq!(jumbo_failures("half1 ok\n", 2), [1], "a half that didn't answer failed");
        assert_eq!(jumbo_failures("", 1), [0]);

        let fact = |iface: &str, ip: &str| FabricFact { iface: iface.into(), ip: ip.into(), prefix: 24, ..Default::default() };
        let own = [fact("enP2p1s0f1np1", "192.168.101.2"), fact("enp1s0f1np1", "192.168.100.2")];
        let head = [fact("enp1s0f1np1", "192.168.100.1"), fact("enP2p1s0f1np1", "192.168.101.1")];
        assert_eq!(jumbo_pairs(&own, &head), [("enP2p1s0f1np1".to_string(), "192.168.101.1".to_string()), ("enp1s0f1np1".to_string(), "192.168.100.1".to_string())], "paired by subnet, not by name");
        assert!(jumbo_pairs(&[fact("en1", "10.0.0.2")], &head).is_empty(), "no half in common");
        assert!(jumbo_pairs(&[FabricFact { prefix: 0, ..fact("en1", "192.168.100.2") }], &head).is_empty(), "no prefix, no subnet to pair by");
    }

    fn half(iface: &str, ip: &str) -> FabricFact {
        FabricFact { iface: iface.into(), ip: ip.into(), prefix: 24, ..Default::default() }
    }

    const A: &str = "enp1s0f1np1";
    const B: &str = "enP2p1s0f1np1";

    #[test]
    fn a_half_only_one_side_has_is_named_whichever_side_has_it() {
        let both = |n: u8| [half(A, &format!("192.168.100.{n}")), half(B, &format!("192.168.101.{n}"))];
        assert_eq!(unpaired_half(&both(2), &both(1)), None);
        let swapped = [half(B, "192.168.101.2"), half(A, "192.168.100.2")];
        assert_eq!(unpaired_half(&swapped, &both(1)), None, "by subnet, not by order");
        // A worker off the head's first half: --master-addr has no route, whatever .101 says.
        assert_eq!(unpaired_half(&both(2)[1..], &both(1)).as_deref(), Some(A));
        // A worker with a half the head lacks.
        assert_eq!(unpaired_half(&both(2), &both(1)[..1]).as_deref(), Some(B));
        // Nothing in common, and a fact with no subnet to go by.
        assert_eq!(unpaired_half(&[half("en1", "10.0.0.2")], &both(1)).as_deref(), Some("en1"));
        let unknown = [FabricFact { prefix: 0, ..half(A, "192.168.100.2") }, half(B, "192.168.101.2")];
        assert_eq!(unpaired_half(&unknown, &both(1)).as_deref(), Some(A));
        let bad = [half(A, "not an address"), half(B, "192.168.101.2")];
        assert_eq!(unpaired_half(&bad, &both(1)).as_deref(), Some(A));
    }

    /// The Load page's verdict: nothing is pinged, and this is the one check that sees a half with
    /// no ping to send.
    #[test]
    fn a_worker_off_the_heads_subnets_is_refused_without_a_ping() {
        let spec = two_nodes();
        let at = |node: &str, fabric: Vec<FabricFact>| Preflight { fabric, ..parse_preflight(node, &spec, &idle()) };
        let full = |n: u8| vec![half(A, &format!("192.168.100.{n}")), half(B, &format!("192.168.101.{n}"))];
        let judged = |head: Vec<FabricFact>, worker: Vec<FabricFact>| PreflightSet::judge(&spec, vec![at("cell1", head), at("cell2", worker)], 0);
        assert_eq!(judged(full(1), full(2)).verdict, "ready");

        let off_first = judged(full(1), vec![half(B, "192.168.101.2")]);
        assert_eq!(off_first.verdict, "blocked");
        assert_eq!(off_first.problems, [jumbo_failed("cell2", "cell1", A)]);
        // rank_argv would have given it the head's .100 with nothing on .100 to reach it by.
        let net = |n: &str, f: &[FabricFact]| pick_fabric(n, &f.iter().map(|x| (x.iface.clone(), x.ip.clone(), String::new())).collect::<Vec<_>>()).unwrap();
        assert_eq!(net("cell1", &full(1)).ip, "192.168.100.1");
        assert_eq!(net("cell2", &[half(B, "192.168.101.2")]).ip, "192.168.101.2");

        let extra = judged(vec![half(A, "192.168.100.1")], full(2));
        assert_eq!(extra.problems, [jumbo_failed("cell2", "cell1", B)]);

        // Three nodes: each worker answers for itself.
        let three = DeploySpec { nodes: vec!["cell1".into(), "cell2".into(), "cell3".into()], ..spec.clone() };
        let set = PreflightSet::judge(&three, vec![at("cell1", full(1)), at("cell2", full(2)), at("cell3", vec![half(B, "192.168.101.3")])], 0);
        assert_eq!(set.problems, [jumbo_failed("cell3", "cell1", A)]);

        // A node that read no fabric has its own problem in mod.rs; this adds none.
        assert!(judged(full(1), Vec::new()).problems.is_empty());
        assert!(judged(Vec::new(), full(2)).problems.is_empty());
        // One node meets nobody.
        assert_eq!(PreflightSet::judge(&gemma(true), vec![at("cell2", vec![half(B, "192.168.101.2")])], 0).verdict, "ready");
    }

    /// A several-node load's head listens on both ports, and ss's order isn't by port.
    #[test]
    fn a_reload_of_a_several_node_load_counts_both_its_ports_in_either_order() {
        let api = "LISTEN 0      2048       192.168.50.51:8000      0.0.0.0:*\n";
        let meet = "LISTEN 0      4096       192.168.100.1:29501     0.0.0.0:*\n";
        let spec = two_nodes();
        for port in [format!("{api}{meet}"), format!("{meet}{api}")] {
            let mut p = parse_preflight("cell1", &spec, &with_section("port", &port));
            assert_eq!(p.problems.len(), 2, "{:?}", p.problems);
            count_replaced(&mut p, &spec, "gemma-3-27b-it", 8000, 0.75);
            assert!(p.problems.is_empty(), "{port}: {:?}", p.problems);
            assert_eq!(p.port_owner.as_deref(), Some("gemma-3-27b-it, the load being replaced; free once it's unloaded"));
        }
        // Docker's published port listens on both families, one sentence each.
        let both = "LISTEN 0 4096 0.0.0.0:8000 0.0.0.0:*\nLISTEN 0 4096 [::]:8000 [::]:*\n";
        let mut p = parse_preflight("cell1", &spec, &with_section("port", both));
        assert_eq!(p.problems.len(), 2, "{:?}", p.problems);
        count_replaced(&mut p, &spec, "gemma-3-27b-it", 8000, 0.75);
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        // Moved to another port, what listens on the new one isn't the load's.
        let mut moved = parse_preflight("cell1", &spec, &with_section("port", api));
        count_replaced(&mut moved, &spec, "gemma-3-27b-it", 8001, 0.75);
        assert_eq!(moved.problems, ["Port 8000 is already in use on cell1 (listening on 192.168.50.51:8000). Pick another port."]);
    }

    #[test]
    fn a_reloads_preflight_counts_the_replaced_loads_port_and_share_as_its_own() {
        let id5a = "sha256:5a0f8b914da56ea2ae2dbe569e4219a14605ce798e7eff12ae6db63328adc4f1";
        // gemma serves on cell2 at 0.75 on port 8000: 23.0 GiB free, its listener on the port.
        let listen = "LISTEN 0      4096         192.168.50.154:8000       0.0.0.0:*    \n";
        let read = || parse_preflight("cell2", &gemma(true), &preflight_out(24_162_539, EARLYOOM_ON, id5a, true, listen, SHM_EMPTY, CACHES_FRESH));
        let p = read();
        assert!(p.drop_needed && !p.port_free);
        assert_eq!(p.problems, ["Port 8000 is already in use on cell2 (listening on 192.168.50.154:8000). Pick another port."]);
        let mut r = read();
        count_replaced(&mut r, &gemma(true), "gemma-3-27b-it", 8000, 0.75);
        assert_eq!(r.replacing.as_deref(), Some("gemma-3-27b-it"));
        assert_eq!(r.freed, (127_598_748.0 * 1024.0 * 0.75) as u64);
        assert!(!r.drop_needed, "23.0 + 91.3 GiB is above 95.3 GiB");
        assert!(r.problems.is_empty() && !r.port_free, "{:?}", r.problems);
        assert_eq!(r.port_owner.as_deref(), Some("gemma-3-27b-it, the load being replaced; free once it's unloaded"));
        // Its share at 0.5 doesn't free enough: the drop is still asked for.
        let mut half = read();
        count_replaced(&mut half, &gemma(true), "gemma-3-27b-it", 8000, 0.5);
        assert!(half.drop_needed);
        // A new port: whatever listens there is somebody else's.
        let mut moved = read();
        count_replaced(&mut moved, &gemma(true), "gemma-3-27b-it", 8001, 0.75);
        assert_eq!(moved.problems, p.problems);
        assert_eq!(moved.port_owner.as_deref(), Some("listening on 192.168.50.154:8000"));
        // Not read: nothing is counted.
        let mut blank = parse_preflight("cell2", &gemma(true), "");
        count_replaced(&mut blank, &gemma(true), "gemma-3-27b-it", 8000, 0.75);
        assert_eq!(blank.freed, 0);
        assert!(blank.cut_off && blank.problems[0].starts_with("Couldn't finish reading cell2"), "{:?}", blank.problems);
    }

    #[test]
    fn a_drop_is_offered_only_below_share_times_total_plus_4_gib() {
        let id5a = "sha256:5a0f8b914da56ea2ae2dbe569e4219a14605ce798e7eff12ae6db63328adc4f1";
        let need_kb = (127_598_748.0 * 0.75) as u64 + 4 * 1024 * 1024;
        let at = |free_kb: u64, share: f64| parse_preflight("cell2", &DeploySpec { gpu_memory_utilization: share, ..gemma(true) }, &preflight_out(free_kb, EARLYOOM_ON, id5a, true, "", SHM_EMPTY, CACHES_FRESH));
        assert!(!at(need_kb + 1, 0.75).drop_needed, "just above");
        assert!(at(need_kb - 1, 0.75).drop_needed, "just below");
        assert!(!at(need_kb - 1, 0.5).drop_needed, "a lower share needs less");
        // The words, with the figures: 41.2 GiB free, 95.3 GiB needed.
        let short = at(43_201_000, 0.75);
        assert!(short.drop_needed);
        assert_eq!(
            short_of_memory(&short, 0.75, false),
            "cell2 has 41.2 GiB free; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Tick 'Drop the page cache first' to let SparkBox free it (sudo; the kernel refills it)."
        );
        assert_eq!(
            short_of_memory(&short, 0.75, true),
            "cell2 has 41.2 GiB free after dropping the page cache; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Lower the share or free memory."
        );
    }
}
