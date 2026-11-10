//! Deterministic fit numbers: what a model needs per node for 1..N nodes, which
//! split modes its attention heads allow, context memory per token, and a rough
//! decode-speed ceiling. Exact arithmetic is done here; the AI analysis gets
//! these numbers as facts rather than being asked to compute them.
//!
//! The table is honest about the things that usually go unsaid: the longest
//! request can't exceed the model's own maximum however much memory is left,
//! linear-attention layers keep a fixed state per sequence on top of the KV
//! cache, a vision encoder takes memory too, and pipeline parallel doesn't make
//! one stream any faster.

use serde::Serialize;
use serde_json::Value;

/// vLLM's share of each node's memory (NVIDIA's multi-Spark guide uses 0.8).
pub const GPU_MEMORY_UTILIZATION: f64 = 0.8;
/// The most a new load starts at (P1.33): room for the OS, vLLM's own process and the page cache
/// on a 128 GB GB10. It's the share of a model too big to size (plan::sized_share), and the
/// fallback when the plan can't size it. The fit tables keep GPU_MEMORY_UTILIZATION for their
/// estimates.
pub const FIRST_LOAD_SHARE: f64 = 0.75;
/// The least share SparkBox takes, or sizes a load to: about 24 GiB of a GB10, the 6 GiB runtime
/// and 18 GiB for weights and KV cache. The runtime figure is an estimate, so a small model's 10%
/// headroom is too thin to lean on, and a share that's too small only fails once the weights have
/// loaded. Going lower saves nothing: a node holds one load, and what's left becomes KV cache.
pub const MIN_SHARE: f64 = 0.2;
/// Activations, CUDA graphs and runtime per node, on top of weights.
pub const RUNTIME_OVERHEAD: f64 = 6.0 * 1024.0 * 1024.0 * 1024.0;
/// GB10 LPDDR5x bandwidth, bytes/s (bounds single-stream decode for dense models).
pub const NODE_BANDWIDTH: f64 = 273e9;
/// Share of the bandwidth ceiling decode really reaches (kernels, KV reads, sampling).
pub const DECODE_EFFICIENCY: f64 = 0.85;
/// Unified memory the OS reports on a GB10 (~119.7 GiB) when live numbers aren't available.
pub const DEFAULT_NODE_MEMORY: u64 = 128_530_000_000;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArchFacts {
    pub layers: u64,
    /// Layers that keep a per-token KV cache (full, sparse or MLA attention; not linear attention).
    pub kv_layers: u64,
    pub heads: u64,
    pub kv_heads: u64,
    pub head_dim: u64,
    /// MLA latent size per token per layer (kv_lora_rank + rope dims), when the model uses MLA.
    pub mla_dim: Option<u64>,
    pub experts: Option<u64>,
    pub experts_active: Option<u64>,
    pub max_context: Option<u64>,
    /// Linear-attention (or Mamba) layers: no per-token cache, but a fixed state per sequence.
    pub linear_layers: u64,
    /// That state for one sequence across all linear layers, in bytes.
    pub state_bytes_per_seq: u64,
    /// Memory the vision encoder needs (weights and activations), when the model has one.
    pub vision_bytes: u64,
    /// DeepSeek-style sparse attention over an MLA cache (an indexer picks the tokens each query
    /// reads: index_topk, index_n_heads, index_head_dim). On a GB10 vLLM has one kernel for it.
    pub sparse_mla: bool,
    /// Sliding-window layers retain only this many tokens per request.
    pub sliding_window: Option<u64>,
    pub sliding_layers: u64,
}

/// Architectures that use sparse MLA attention, for configs that don't spell out the indexer.
const SPARSE_MLA_ARCHS: [&str; 5] = ["DeepseekV32ForCausalLM", "DeepseekV3_2ForCausalLM", "GlmMoeDsaForCausalLM", "Glm5NextForConditionalGeneration", "Glm5NextForCausalLM"];

/// What a fit row says instead of numbers when GB10 has no attention kernel for its KV cache.
pub const NO_KERNEL_GB10: &str = "No kernel on GB10 for this sparse-MLA model with an auto or bf16 KV cache. Use a supported recipe and image; FP8 alone does not establish support.";

/// Whether a config describes sparse MLA: the indexer's settings, a sparse attention layer type,
/// or a known architecture (`architectures` is checked too when the caller has the top level).
pub fn sparse_mla(cfg: &Value, architectures: &[String]) -> bool {
    let indexer = ["index_topk", "index_n_heads", "index_head_dim"].iter().any(|k| n(cfg, k).is_some());
    let layers = cfg["layer_types"].as_array().is_some_and(|t| t.iter().any(|x| x.as_str().is_some_and(|x| x.contains("sparse"))));
    let arch = cfg["architectures"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).chain(architectures.iter().map(String::as_str)).any(|a| SPARSE_MLA_ARCHS.contains(&a));
    n(cfg, "kv_lora_rank").is_some() && (indexer || layers || arch)
}

fn n(cfg: &Value, key: &str) -> Option<u64> {
    cfg[key].as_u64().filter(|v| *v > 0)
}

/// Fixed per-sequence state of one linear-attention layer (Gated DeltaNet / Mamba2 style, bf16).
fn linear_state_per_layer(cfg: &Value) -> u64 {
    if let (Some(vh), Some(kd), Some(vd)) = (n(cfg, "linear_num_value_heads"), n(cfg, "linear_key_head_dim"), n(cfg, "linear_value_head_dim")) {
        let kh = n(cfg, "linear_num_key_heads").unwrap_or(vh);
        let kernel = n(cfg, "linear_conv_kernel_dim").unwrap_or(4);
        let recurrent = vh * kd * vd * 2;
        let conv = (2 * kh * kd + vh * vd) * (kernel - 1) * 2;
        return recurrent + conv;
    }
    if let (Some(h), Some(d), Some(s)) = (n(cfg, "mamba_num_heads"), n(cfg, "mamba_head_dim"), n(cfg, "ssm_state_size")) {
        return h * d * s * 2 + h * d * n(cfg, "conv_kernel").unwrap_or(4) * 2;
    }
    0
}

/// Weights plus activations of a vision tower, from its config (ViT-style: ~12·d² per layer).
pub fn vision_bytes(vision: &Value) -> u64 {
    if !vision.is_object() {
        return 0;
    }
    let depth = n(vision, "depth").or_else(|| n(vision, "num_hidden_layers")).unwrap_or(0);
    let hidden = n(vision, "hidden_size").or_else(|| n(vision, "embed_dim")).unwrap_or(0);
    let weights = 12 * depth * hidden * hidden * 2;
    // Image activations and the projector: a fixed allowance, so even a tiny encoder counts.
    weights + GIB as u64
}

pub fn arch_facts(cfg: &Value) -> ArchFacts {
    let layers = n(cfg, "num_hidden_layers").unwrap_or(0);
    let heads = n(cfg, "num_attention_heads").unwrap_or(0);
    let kv_heads = n(cfg, "num_key_value_heads").unwrap_or(heads);
    let head_dim = n(cfg, "head_dim").unwrap_or_else(|| {
        let hidden = n(cfg, "hidden_size").unwrap_or(0);
        if heads > 0 { hidden / heads } else { 0 }
    });
    let kv_layers = match cfg["layer_types"].as_array() {
        Some(types) => types.iter().filter(|t| !t.as_str().unwrap_or("").contains("linear") && !t.as_str().unwrap_or("").contains("mamba")).count() as u64,
        None => match n(cfg, "full_attention_interval") {
            Some(every) => layers / every,
            None => layers,
        },
    };
    let linear_layers = layers.saturating_sub(kv_layers);
    let sliding_window = n(cfg, "sliding_window");
    let sliding_layers = if sliding_window.is_some() {
        match cfg["layer_types"].as_array() {
            Some(types) => types.iter().filter(|t| t.as_str().is_some_and(|s| s.contains("sliding"))).count() as u64,
            None => n(cfg, "sliding_window_pattern").map(|every| layers.saturating_sub(layers / every)).unwrap_or(kv_layers),
        }.min(kv_layers)
    } else { 0 };
    let mla_dim = n(cfg, "kv_lora_rank").map(|r| r + cfg["qk_rope_head_dim"].as_u64().unwrap_or(0));
    ArchFacts {
        layers,
        kv_layers,
        heads,
        kv_heads,
        head_dim,
        mla_dim,
        experts: n(cfg, "n_routed_experts").or_else(|| n(cfg, "num_experts")).or_else(|| n(cfg, "num_local_experts")),
        experts_active: n(cfg, "num_experts_per_tok"),
        max_context: n(cfg, "max_position_embeddings"),
        linear_layers,
        state_bytes_per_seq: linear_layers * linear_state_per_layer(cfg),
        vision_bytes: vision_bytes(&cfg["vision_config"]),
        sparse_mla: sparse_mla(cfg, &[]),
        sliding_window,
        sliding_layers,
    }
}

/// KV-cache bytes per token across the whole model (bf16 cache).
pub fn kv_bytes_per_token(a: &ArchFacts) -> Option<u64> {
    if a.kv_layers == 0 {
        return None;
    }
    let per_layer = match a.mla_dim {
        Some(d) => d * 2,
        None if a.kv_heads > 0 && a.head_dim > 0 => 2 * a.kv_heads * a.head_dim * 2,
        None => return None,
    };
    Some(per_layer * a.kv_layers)
}

/// Bytes per KV element for vLLM's --kv-cache-dtype: "fp8" halves the bf16 cache.
pub fn kv_dtype_factor(dtype: Option<&str>) -> f64 {
    match dtype {
        Some(d) if d.starts_with("fp8") => 0.5,
        _ => 1.0,
    }
}

/// One request's cache, including fixed state. Shared by Load and the Hub.
pub fn request_bytes(a: &ArchFacts, per_token: u64, tokens: u64, dtype: Option<&str>) -> f64 {
    let sliding = a.sliding_layers.min(a.kv_layers);
    let effective = if a.kv_layers > 0 {
        ((a.kv_layers - sliding) as f64 * tokens as f64
            + sliding as f64 * tokens.min(a.sliding_window.unwrap_or(tokens)) as f64) / a.kv_layers as f64
    } else { tokens as f64 };
    effective * per_token as f64 * kv_dtype_factor(dtype) + a.state_bytes_per_seq as f64
}

/// Invert the shared cache estimate without assuming every layer has an unbounded cache.
pub fn context_capacity(a: &ArchFacts, per_token: u64, bytes: f64, dtype: Option<&str>, limit: u64) -> u64 {
    let (mut lo, mut hi) = (0, limit);
    while lo < hi {
        let mid = lo + (hi - lo) / 2 + (hi - lo) % 2;
        if request_bytes(a, per_token, mid, dtype) <= bytes { lo = mid; } else { hi = mid - 1; }
    }
    lo
}

/// Whether vLLM can split the attention heads `tp` ways (KV heads may be replicated).
pub fn tensor_parallel_ok(a: &ArchFacts, tp: u64) -> bool {
    if tp == 1 {
        return true;
    }
    if a.heads == 0 {
        return false;
    }
    let kv_ok = a.mla_dim.is_some() || a.kv_heads.is_multiple_of(tp) || (a.kv_heads > 0 && tp.is_multiple_of(a.kv_heads));
    a.heads.is_multiple_of(tp) && kv_ok
}

/// Whether the model fits on enough nodes as they are right now.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FitsNow {
    pub ok: bool,
    /// Nodes with enough free memory for this row's share, best first.
    pub free_nodes: Vec<String>,
    /// e.g. "only gx10-0000 free".
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitRow {
    pub nodes: u64,
    pub weights_per_node: u64,
    pub budget_per_node: u64,
    /// Memory left per node for KV cache after weights, the vision encoder and runtime overhead.
    pub kv_room_per_node: i64,
    pub fits: bool,
    pub tensor_parallel_ok: bool,
    pub pipeline_parallel_ok: bool,
    /// Tokens of KV cache the whole deployment can hold (shared by all requests).
    pub kv_capacity_tokens: Option<u64>,
    /// The longest single request: the memory limit, capped at the model's own maximum.
    pub max_context_tokens: Option<u64>,
    /// How many requests of the model's full length fit at once, e.g. 2.4.
    pub full_requests: Option<f64>,
    /// Realistic single-stream decode ceiling (dense models), from memory bandwidth.
    pub decode_ceiling_tps: Option<f64>,
    /// No tensor parallel split at this size: pipeline parallel, so no single-stream speedup.
    pub pipeline_only: bool,
    pub kv_dtype: String,
    pub fits_now: Option<FitsNow>,
    /// Why it can't run however much memory there is: NO_KERNEL_GB10 for a bf16 or fp16 KV cache
    /// with sparse MLA on a GB10.
    pub no_kernel: Option<String>,
    pub share: f64,
    pub safe_share: f64,
    pub host_runtime: u64,
}

/// How many ways the KV cache divides across `nodes`. Tensor parallel splits it by KV head,
/// but MLA's single latent cache (and any KV head beyond the head count) is copied to every
/// rank; pipeline parallel divides the layers instead.
pub fn kv_shards(a: &ArchFacts, nodes: u64, tp_ok: bool) -> u64 {
    match (nodes, tp_ok, a.mla_dim) {
        (1, _, _) => 1,
        (_, true, Some(_)) => 1,
        (_, true, None) => a.kv_heads.clamp(1, nodes),
        (_, false, _) => nodes,
    }
}

/// What the table is computed for.
#[derive(Clone, Debug, Default)]
pub struct FitOptions {
    /// vLLM's --kv-cache-dtype ("fp8" halves the cache).
    pub kv_dtype: Option<String>,
    /// Memory each node could give a new model right now: (node, bytes). Empty when unknown.
    pub free_now: Vec<(String, u64)>,
    /// The nodes are GB10s (compute 12.x): sparse MLA runs only with an FP8 KV cache there.
    pub gb10: bool,
    /// Known recipe refusal, independent of estimated cache capacity.
    pub blocked: Option<String>,
}

pub fn fit_table(weight_bytes: u64, a: &ArchFacts, node_memory: u64, max_nodes: u64) -> Vec<FitRow> {
    fit_table_with(weight_bytes, a, node_memory, max_nodes, &FitOptions::default())
}

pub fn fit_table_with(weight_bytes: u64, a: &ArchFacts, node_memory: u64, max_nodes: u64, opts: &FitOptions) -> Vec<FitRow> {

    let factor = kv_dtype_factor(opts.kv_dtype.as_deref());
    let kv_per_token = kv_bytes_per_token(a).map(|b| b as f64 * factor);
    let dense = a.experts.is_none();
    // vLLM's one sparse MLA kernel on SM12x (FlashInfer's) takes only an FP8 KV cache.
    let no_kernel = opts.blocked.clone().or_else(|| (a.sparse_mla && opts.gb10 && factor >= 1.0).then(|| NO_KERNEL_GB10.to_string()));
    (1..=max_nodes.max(1))
        .map(|nodes| {
            let tp_ok = tensor_parallel_ok(a, nodes);
            let pp_ok = nodes == 1 || a.layers >= nodes;
            let needs = super::plan::ModelNeeds { weight_bytes, kv_bytes_per_token: kv_bytes_per_token(a), arch: a.clone() };
            let snapshots: Vec<crate::model::NodeSnapshot> = (0..nodes).map(|i| {
                let mut node = crate::model::NodeSnapshot { id: i.to_string(), ..Default::default() };
                node.mem.total = node_memory;
                node
            }).collect();
            let refs = snapshots.iter().collect::<Vec<_>>();
            let mut spec = super::plan::DeploySpec {
                nodes: snapshots.iter().map(|n| n.id.clone()).collect(),
                tensor_parallel: if tp_ok { nodes } else { 1 },
                pipeline_parallel: if tp_ok { 1 } else { nodes },
                max_model_len: a.max_context, kv_cache_dtype: opts.kv_dtype.clone(),
                ..Default::default()
            };
            let initial = super::plan::memory_plan(&spec, &needs, &refs, &[]);
            spec.gpu_memory_utilization = if opts.gb10 {
                super::plan::sized_share(&spec, &needs, &initial).unwrap_or(FIRST_LOAD_SHARE)
            } else { GPU_MEMORY_UTILIZATION };
            let memory = super::plan::memory_plan(&spec, &needs, &refs, &[]);
            let first = &memory[0];
            let budget = first.budget as f64;
            let weights = first.weights as f64;
            let room = budget - weights - first.overhead as f64;
            let fits = room > 2.0 * GIB && (tp_ok || pp_ok) && no_kernel.is_none();
            let capacity = (room > 0.0).then(|| room * kv_shards(a, nodes, tp_ok) as f64);
            let kv_capacity_tokens = match (kv_per_token, capacity) {
                (Some(per), Some(cap)) => Some((cap / per) as u64),
                _ => None,
            };
            // One request also needs its linear-attention state.
            let one_request = match (kv_per_token, capacity) {
                (Some(_), Some(cap)) if cap > a.state_bytes_per_seq as f64 => Some(context_capacity(a, kv_bytes_per_token(a).unwrap_or(0), cap, opts.kv_dtype.as_deref(), a.max_context.unwrap_or(u64::MAX))),
                (Some(_), _) => Some(0),
                _ => None,
            };
            let max_context_tokens = match (one_request, a.max_context) {
                (Some(t), Some(m)) => Some(t.min(m)),
                (Some(t), None) => Some(t),
                (None, m) => m.filter(|_| fits),
            };
            let full_requests = match (kv_per_token, capacity, a.max_context) {
                (Some(_), Some(cap), Some(m)) => Some(cap / request_bytes(a, kv_bytes_per_token(a).unwrap_or(0), m, opts.kv_dtype.as_deref()).max(1.0)),
                _ => None,
            };
            let pipeline_only = nodes > 1 && !tp_ok;
            // Tensor parallel reads 1/n of the weights per token on each node; pipeline parallel
            // still reads all of them in turn, so one stream is no faster than on one node.
            let speedup = if pipeline_only { 1.0 } else { nodes as f64 };
            FitRow {
                nodes,
                weights_per_node: weights as u64,
                budget_per_node: budget as u64,
                kv_room_per_node: room as i64,
                fits,
                tensor_parallel_ok: tp_ok,
                pipeline_parallel_ok: pp_ok,
                kv_capacity_tokens,
                max_context_tokens,
                full_requests,
                decode_ceiling_tps: (dense && weight_bytes > 0 && fits)
                    .then(|| DECODE_EFFICIENCY * NODE_BANDWIDTH * speedup / weight_bytes as f64),
                pipeline_only,
                kv_dtype: if factor < 1.0 { "fp8".into() } else { "bf16".into() },
                fits_now: match &no_kernel {
                    Some(_) if !opts.free_now.is_empty() => Some(FitsNow { ok: false, free_nodes: Vec::new(), text: "no kernel on GB10".into() }),
                    _ => fits_now(nodes, budget, &opts.free_now),
                },
                no_kernel: no_kernel.clone(),
                share: spec.gpu_memory_utilization,
                safe_share: first.safe_share,
                host_runtime: first.host_runtime,
            }
        })
        .collect()
}

fn fits_now(nodes: u64, need: f64, free: &[(String, u64)]) -> Option<FitsNow> {
    if free.is_empty() {
        return None;
    }
    let mut ok: Vec<&(String, u64)> = free.iter().filter(|(_, f)| *f as f64 >= need).collect();
    ok.sort_by(|a, b| b.1.cmp(&a.1));
    let names: Vec<String> = ok.iter().map(|(n, _)| n.clone()).collect();
    let enough = names.len() as u64 >= nodes;
    let text = if enough && names.len() == free.len() {
        "yes".to_string()
    } else if enough {
        format!("yes, on {}", names.iter().take(nodes as usize).cloned().collect::<Vec<_>>().join(", "))
    } else if names.is_empty() {
        "no node has room".to_string()
    } else {
        format!("only {} free", names.join(", "))
    };
    Some(FitsNow { ok: enough, free_nodes: names, text })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const GB: u64 = 1_000_000_000;

    fn glm() -> ArchFacts {
        // nvidia/GLM-5.3-Flash-NVFP4 text config (as published).
        arch_facts(&json!({
            "num_hidden_layers": 45, "hidden_size": 4096, "num_attention_heads": 64, "num_key_value_heads": 64,
            "head_dim": 0, "max_position_embeddings": 1048576, "n_routed_experts": 288, "num_experts_per_tok": 8,
            "kv_lora_rank": 512, "qk_rope_head_dim": 0,
            "layer_types": ([vec!["linear_attention"; 34], vec!["deepseek_sparse_attention"; 11]].concat())
        }))
    }

    fn qwen() -> ArchFacts {
        // Qwen/Qwen3.8-27B text config (as published).
        arch_facts(&json!({
            "num_hidden_layers": 64, "hidden_size": 5120, "num_attention_heads": 24, "num_key_value_heads": 4,
            "head_dim": 256, "max_position_embeddings": 262144, "full_attention_interval": 4,
            "layer_types": ([vec!["linear_attention"; 48], vec!["full_attention"; 16]].concat())
        }))
    }

    #[test]
    fn sliding_window_cache_matches_load_and_inverse_capacity() {
        let a = arch_facts(&json!({"num_hidden_layers": 6, "num_attention_heads": 8,
            "num_key_value_heads": 4, "head_dim": 128, "sliding_window": 1024,
            "sliding_window_pattern": 6, "max_position_embeddings": 32768}));
        assert_eq!(a.sliding_layers, 5);
        let per = kv_bytes_per_token(&a).unwrap();
        let bytes = request_bytes(&a, per, 32768, None);
        assert_eq!(bytes, (2 * 4 * 128 * 2 * (32768 + 5 * 1024)) as f64);
        assert_eq!(context_capacity(&a, per, bytes, None, 131072), 32768);
        let needs = super::super::plan::ModelNeeds { weight_bytes: 20 * GB, kv_bytes_per_token: Some(per), arch: a.clone() };
        let spec = super::super::plan::DeploySpec { max_model_len: Some(32768), max_num_seqs: Some(3), ..Default::default() };
        assert_eq!(super::super::plan::kv_need(&spec, &needs), bytes as u64 * 3);
        let row = fit_table_with(20 * GB, &a, DEFAULT_NODE_MEMORY, 1, &FitOptions { gb10: true, ..Default::default() }).remove(0);
        assert!(row.share <= FIRST_LOAD_SHARE && row.share >= MIN_SHARE);
        assert_eq!(row.budget_per_node, (DEFAULT_NODE_MEMORY as f64 * row.share) as u64);
        assert!(row.host_runtime > 0);
    }

    #[test]
    fn kv_cache_per_token_follows_the_attention_layout() {
        assert_eq!(kv_bytes_per_token(&glm()), Some(11 * 512 * 2), "11 MLA layers, 512-wide latent");
        assert_eq!(kv_bytes_per_token(&qwen()), Some(16 * 2 * 4 * 256 * 2), "16 full-attention layers, 4 KV heads");
    }

    #[test]
    fn glm_flash_nvfp4_needs_three_or_four_sparks() {
        let rows = fit_table(204_400_000_000, &glm(), DEFAULT_NODE_MEMORY, 4);
        let fits: Vec<bool> = rows.iter().map(|r| r.fits).collect();
        assert_eq!(fits, [false, false, true, true]);
        assert!(!rows[2].tensor_parallel_ok && rows[2].pipeline_parallel_ok, "64 heads don't split 3 ways");
        assert!(rows[3].tensor_parallel_ok);
        assert!(rows[3].decode_ceiling_tps.is_none(), "MoE speed depends on routing");
        // MLA's latent cache is copied to every tensor-parallel rank: capacity is one node's room.
        let per = kv_bytes_per_token(&glm()).unwrap();
        assert_eq!(rows[3].kv_capacity_tokens, Some(rows[3].kv_room_per_node as u64 / per));
        // Three nodes run pipeline parallel, so each holds a third of the layers' cache.
        assert!(rows[2].kv_capacity_tokens.unwrap() > rows[3].kv_capacity_tokens.unwrap());
        assert!(rows[2].pipeline_only && !rows[3].pipeline_only);
    }

    #[test]
    fn qwen_27b_fits_one_two_and_four_but_not_three_way_tensor_parallel() {
        let rows = fit_table(55_600_000_000, &qwen(), DEFAULT_NODE_MEMORY, 4);
        assert!(rows.iter().all(|r| r.fits));
        assert_eq!(rows.iter().map(|r| r.tensor_parallel_ok).collect::<Vec<_>>(), [true, true, false, true]);
        let one = rows[0].decode_ceiling_tps.unwrap();
        assert!((3.5..5.0).contains(&one), "{one}");
        assert!((rows[3].decode_ceiling_tps.unwrap() / one - 4.0).abs() < 1e-9);
        // Three nodes can only split by pipeline: no faster than one for a single stream.
        assert!(rows[2].pipeline_only);
        assert!((rows[2].decode_ceiling_tps.unwrap() - one).abs() < 1e-9);
        // 4 KV heads split 4 ways under tensor parallel, so the cache spreads over all four nodes.
        let per = kv_bytes_per_token(&qwen()).unwrap();
        assert_eq!(rows[3].kv_capacity_tokens, Some((rows[3].kv_room_per_node as f64 * 4.0 / per as f64) as u64));
        assert!(rows[0].weights_per_node > 55 * GB && rows[3].weights_per_node < 14 * GB);
    }

    #[test]
    fn the_longest_request_is_capped_at_the_models_maximum() {
        let rows = fit_table(55_600_000_000, &qwen(), DEFAULT_NODE_MEMORY, 4);
        for r in &rows {
            assert_eq!(r.max_context_tokens, Some(262_144), "{} nodes: memory for more, but the model stops at 262,144", r.nodes);
            assert!(r.kv_capacity_tokens.unwrap() > 262_144);
        }
        // One node: 40.8 GB of room / 64 KiB per token ≈ 622k tokens, i.e. about 2.4 full-length requests.
        let one = rows[0].full_requests.unwrap();
        assert!((2.2..2.6).contains(&one), "{one}");
    }

    #[test]
    fn an_fp8_cache_doubles_the_full_length_requests() {
        let bf16 = fit_table_with(55_600_000_000, &qwen(), DEFAULT_NODE_MEMORY, 2, &FitOptions::default());
        let fp8 = fit_table_with(55_600_000_000, &qwen(), DEFAULT_NODE_MEMORY, 2, &FitOptions { kv_dtype: Some("fp8".into()), ..Default::default() });
        for (a, b) in bf16.iter().zip(&fp8) {
            let ratio = b.full_requests.unwrap() / a.full_requests.unwrap();
            assert!((ratio - 2.0).abs() < 1e-9, "{ratio}");
            assert_eq!(b.kv_dtype, "fp8");
        }
    }

    #[test]
    fn linear_attention_state_and_vision_encoders_take_memory() {
        let cfg = json!({
            "num_hidden_layers": 48, "num_attention_heads": 16, "num_key_value_heads": 2, "head_dim": 256,
            "max_position_embeddings": 262144, "full_attention_interval": 4,
            "linear_num_value_heads": 32, "linear_num_key_heads": 16, "linear_key_head_dim": 128, "linear_value_head_dim": 128,
            "linear_conv_kernel_dim": 4,
            "vision_config": { "depth": 27, "hidden_size": 1152 }
        });
        let a = arch_facts(&cfg);
        assert_eq!((a.kv_layers, a.linear_layers), (12, 36));
        // 32 heads × 128 × 128 × 2 B recurrent state, plus the short conv state, in each of 36 layers.
        let per_layer = 32 * 128 * 128 * 2 + (2 * 16 * 128 + 32 * 128) * 3 * 2;
        assert_eq!(a.state_bytes_per_seq, 36 * per_layer);
        assert!(a.vision_bytes > 1024 * 1024 * 1024, "weights plus an activation allowance");
        let with_state = fit_table(20 * GB, &a, DEFAULT_NODE_MEMORY, 1)[0].full_requests.unwrap();
        let without = fit_table(20 * GB, &ArchFacts { state_bytes_per_seq: 0, vision_bytes: 0, ..a.clone() }, DEFAULT_NODE_MEMORY, 1)[0]
            .full_requests
            .unwrap();
        assert!(with_state < without);
    }

    /// nvidia/GLM-5.3-Flash-NVFP4's config.json as the Hugging Face API serves it (Oct 29).
    fn real_glm() -> Value {
        serde_json::from_str(include_str!("fixtures/hf/glm-5.3-flash-nvfp4.config.json")).unwrap()
    }

    #[test]
    fn the_real_glm_config_is_sparse_mla() {
        let cfg = real_glm();
        let text = &cfg["text_config"];
        assert_eq!((text["index_topk"].as_u64(), text["index_n_heads"].as_u64(), text["index_head_dim"].as_u64()), (Some(2048), Some(32), Some(128)));
        assert!(arch_facts(text).sparse_mla, "the indexer settings say so");
        assert!(sparse_mla(&json!({ "kv_lora_rank": 512 }), &["Glm5NextForConditionalGeneration".into()]), "and so does the architecture");
        assert!(!arch_facts(&json!({ "kv_lora_rank": 512, "num_hidden_layers": 61 })).sparse_mla, "plain MLA (DeepSeek V3) isn't sparse");
        assert!(!qwen().sparse_mla);
    }

    #[test]
    fn sparse_mla_has_no_bf16_kernel_on_gb10_but_fp8_computes() {
        let a = arch_facts(&real_glm()["text_config"]);
        let free: Vec<(String, u64)> = ["cell1", "cell2", "cell3", "gx10-0000"].iter().map(|n| (n.to_string(), 117 * GB)).collect();
        let gb10 = |kv: Option<&str>| FitOptions { kv_dtype: kv.map(String::from), free_now: free.clone(), gb10: true, blocked: None };
        let bf16 = fit_table_with(204_400_000_000, &a, DEFAULT_NODE_MEMORY, 4, &gb10(None));
        // TP 4 with a bf16 cache is the owner's Oct 26 run: it can't start, whatever the memory.
        assert!(bf16.iter().all(|r| !r.fits && r.no_kernel.as_deref() == Some(NO_KERNEL_GB10)));
        assert_eq!(bf16[3].fits_now.as_ref().map(|f| (f.ok, f.text.as_str())), Some((false, "no kernel on GB10")));
        let fp8 = fit_table_with(204_400_000_000, &a, DEFAULT_NODE_MEMORY, 4, &gb10(Some("fp8")));
        assert!(fp8.iter().all(|r| r.no_kernel.is_none()));
        assert!(fp8[3].fits && fp8[3].fits_now.as_ref().is_some_and(|f| f.ok), "fp8 rows compute as before");
        // Elsewhere (not a GB10) bf16 computes too.
        let other = fit_table_with(204_400_000_000, &a, DEFAULT_NODE_MEMORY, 4, &FitOptions::default());
        assert!(other[3].fits && other[3].no_kernel.is_none());
    }

    #[test]
    fn fits_now_names_the_nodes_with_room() {
        let free = vec![("cell1".to_string(), 12 * GB), ("cell2".to_string(), 12 * GB), ("gx10-0000".to_string(), 120 * GB)];
        let rows = fit_table_with(31 * GB, &qwen(), DEFAULT_NODE_MEMORY, 3, &FitOptions { free_now: free, ..Default::default() });
        let one = rows[0].fits_now.clone().unwrap();
        assert!(one.ok);
        assert_eq!(one.text, "yes, on gx10-0000");
        let two = rows[1].fits_now.clone().unwrap();
        assert!(!two.ok, "15.5 GB each plus overhead doesn't fit in 12 GB");
        assert_eq!(two.text, "only gx10-0000 free");
    }
}
