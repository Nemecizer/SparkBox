//! Hugging Face Hub: search, model details, config.json and the model card.
//! Called from the Mac; the optional token (Keychain "hf") unlocks gated models.
//!
//! Sizes count only the weights vLLM serves, the way the Hub's file view shows a
//! repo: the shards the root `model.safetensors.index.json` names (or the root
//! `*.safetensors`), never the copies many repos also ship (`original/` and
//! `metal/` checkpoints, `consolidated.safetensors`, `.pth`/`.bin` duplicates,
//! GGUF/ONNX/TF exports). gpt-oss-120b is 60.8 GiB, not the 182 GiB of its repo.
//! The download job gets the same file list, so it fetches only those.
//!
//! The search list shows the same number: each visible row's file sizes are read
//! (`sizing`, the detail's own path), so the list and the detail agree. Until they
//! arrive the row shows "…"; the Hub's parameter counts are only a fallback, and
//! read packed 4- and 8-bit tensors by their quantization's bits. MLX repos
//! (Apple's format) are marked: vLLM on a GB10 can't serve them.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

pub const HUB: &str = "https://huggingface.co";
const README_CHARS: usize = 6000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline: Option<String>,
    pub tags: Vec<String>,
    pub last_modified: Option<String>,
    /// From the safetensors index, when the repo has one.
    pub params: Option<u64>,
    /// Weight bytes from the parameter count per dtype (an estimate until the details load).
    pub weight_bytes: Option<u64>,
    /// The fewest Sparks it fits on (weights only), when the size is known.
    pub min_nodes: Option<u64>,
    pub library: Option<String>,
    /// MLX weights (Apple silicon only): not deployable on a GB10.
    pub mlx: bool,
}

/// An MLX repo: Apple's format, which vLLM on a GB10 can't load. The Hub says so in
/// `library_name` or the tags, or the repo lives in the mlx-community org.
pub fn is_mlx(id: &str, library: Option<&str>, tags: &[String]) -> bool {
    library == Some("mlx") || tags.iter().any(|t| t == "mlx" || t == "apple") || id.to_ascii_lowercase().starts_with("mlx-community/")
}

/// Bits per weight of a packed quantization, and the scales it stores beside them (bytes per
/// weight): MXFP4 one E8M0 byte per 32 values, NVFP4 one FP8 per 16, MLX a bf16 scale and bias
/// per 64, bitsandbytes and AWQ/GPTQ about a byte per 64.
fn packing(q: &Value) -> Option<(f64, f64)> {
    let method = q["quant_method"].as_str().unwrap_or("").to_ascii_lowercase();
    let algo = q["quant_algo"].as_str().unwrap_or("").to_ascii_lowercase();
    let bits = q["bits"].as_f64().or_else(|| q["w_bit"].as_f64());
    Some(match method.as_str() {
        "mxfp4" => (4.0, 1.0 / 32.0),
        "modelopt" if algo.contains("fp4") => (4.0, 1.0 / 16.0),
        "bitsandbytes" if q["load_in_4bit"].as_bool() == Some(true) => (4.0, 1.0 / 64.0),
        "bitsandbytes" => (8.0, 1.0 / 64.0),
        "awq" | "gptq" | "compressed-tensors" | "auto-round" => (bits.unwrap_or(4.0), 1.0 / 64.0),
        // MLX writes only {"bits", "group_size"}.
        "" => (bits?, 2.0 * 2.0 / q["group_size"].as_f64().unwrap_or(64.0)),
        _ => return None,
    })
}

/// Bytes per stored element of a safetensors dtype.
fn dtype_bytes(dtype: &str) -> f64 {
    match dtype.to_ascii_uppercase().as_str() {
        "F64" | "I64" | "U64" => 8.0,
        "F32" | "I32" | "U32" => 4.0,
        "BF16" | "F16" | "I16" | "U16" => 2.0,
        "F8_E4M3" | "F8_E5M2" | "I8" | "U8" | "F8_E8M0" | "BOOL" => 1.0,
        "F4" | "FP4" | "I4" | "U4" => 0.5,
        _ => 2.0,
    }
}

/// Weight bytes from the Hub's per-dtype parameter counts, the fallback while (or when) the file
/// sizes can't be read. Packed integer tensors (U32, I32, U8) hold several quantized weights
/// each, and the Hub counts them two ways: sometimes the stored words (the counts add up to far
/// less than `total`), sometimes the weights they pack (they add up to `total`). Stored words are
/// their own width; packed weights take the quantization's bits plus its scales.
pub fn weight_bytes_from_params(params: &serde_json::Map<String, Value>, total: Option<u64>, quant: &Value) -> u64 {
    let sum: f64 = params.values().filter_map(|n| n.as_f64()).sum();
    let unpacked = total.is_none_or(|t| (sum - t as f64).abs() <= t as f64 * 0.02);
    let pack = packing(quant);
    params
        .iter()
        .map(|(dtype, n)| {
            let n = n.as_f64().unwrap_or(0.0);
            match (dtype.to_ascii_uppercase().as_str(), pack) {
                ("U32" | "I32" | "U8", Some((bits, scales))) if unpacked => n * (bits / 8.0 + scales),
                (d, _) => n * dtype_bytes(d),
            }
        })
        .sum::<f64>() as u64
}

/// Fewest nodes whose vLLM share holds the weights with room to spare.
pub fn min_nodes(weight_bytes: u64, nodes: u64) -> Option<u64> {
    let room = super::fit::DEFAULT_NODE_MEMORY as f64 * super::fit::GPU_MEMORY_UTILIZATION - super::fit::RUNTIME_OVERHEAD - 4.0 * 1024.0 * 1024.0 * 1024.0;
    (1..=nodes.max(1)).find(|n| weight_bytes as f64 / *n as f64 <= room)
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HfFile {
    pub path: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// Files in the repo that aren't downloaded, grouped the way the UI names them.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    /// "original/", "metal/", "consolidated.safetensors", "GGUF files"…
    pub what: String,
    pub files: u64,
    pub bytes: u64,
}

/// Root files vLLM needs besides the weights: config, generation config, the tokenizer and
/// its templates (Mistral's tekken.json included), and processor config for vision models.
fn needed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "config.json" | "generation_config.json" | "special_tokens_map.json" | "preprocessor_config.json" | "merges.txt" | "added_tokens.json" | "tekken.json"
    ) || lower.starts_with("tokenizer")
        || lower.starts_with("chat_template")
        || lower.ends_with(".jinja")
        || lower.ends_with(".tiktoken")
        || lower.starts_with("vocab")
        || lower == "model.safetensors.index.json"
}

/// The files vLLM serves from a repo, and what's left out. Weights: the shards the root index
/// names, else the root `*.safetensors` (consolidated.safetensors only when it's the only one),
/// else the root `*.bin`. Subfolders (`original/`, `metal/`) and other formats never count.
pub fn served_files(files: &[HfFile], index: Option<&Value>) -> (Vec<HfFile>, String, Vec<Skipped>) {
    let root = |f: &&HfFile| !f.path.contains('/');
    let indexed: Vec<String> = index
        .and_then(|i| i["weight_map"].as_object())
        .map(|m| {
            let mut v: Vec<String> = m.values().filter_map(|f| f.as_str().map(String::from)).collect();
            v.sort();
            v.dedup();
            v
        })
        .unwrap_or_default();
    let st: Vec<&HfFile> = files.iter().filter(root).filter(|f| f.path.ends_with(".safetensors")).collect();
    let shards: Vec<&HfFile> = st.iter().copied().filter(|f| !f.path.starts_with("consolidated")).collect();
    let (format, weights): (&str, Vec<&HfFile>) = if !indexed.is_empty() && indexed.iter().all(|n| files.iter().any(|f| &f.path == n)) {
        ("safetensors", files.iter().filter(|f| indexed.contains(&f.path)).collect())
    } else if !shards.is_empty() {
        ("safetensors", shards)
    } else if !st.is_empty() {
        ("safetensors", st)
    } else {
        let bins: Vec<&HfFile> = files.iter().filter(root).filter(|f| f.path.ends_with(".bin")).collect();
        if bins.is_empty() { ("none", Vec::new()) } else { ("bin", bins) }
    };
    let mut served: Vec<HfFile> = files.iter().filter(root).filter(|f| needed(&f.path) && !weights.iter().any(|w| w.path == f.path)).cloned().collect();
    served.extend(weights.into_iter().cloned());
    served.sort_by(|a, b| a.path.cmp(&b.path));

    let mut skipped: Vec<Skipped> = Vec::new();
    let mut skip = |what: String, f: &HfFile| match skipped.iter_mut().find(|s| s.what == what) {
        Some(s) => {
            s.files += 1;
            s.bytes += f.size.unwrap_or(0);
        }
        None => skipped.push(Skipped { what, files: 1, bytes: f.size.unwrap_or(0) }),
    };
    for f in files.iter().filter(|f| !served.iter().any(|s| s.path == f.path) && f.size.unwrap_or(0) > 1_000_000) {
        let what = match f.path.split_once('/') {
            Some((dir, _)) => format!("{dir}/"),
            None if f.path.starts_with("consolidated") => "consolidated weights".into(),
            None => match f.path.rsplit('.').next().unwrap_or("") {
                "gguf" => "GGUF files".into(),
                "onnx" => "ONNX files".into(),
                "pth" | "pt" | "bin" => "PyTorch copies".into(),
                "h5" | "msgpack" => "TensorFlow/Flax copies".into(),
                _ => f.path.clone(),
            },
        };
        skip(what, f);
    }
    skipped.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    (served, format.into(), skipped)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HfModel {
    pub id: String,
    /// Commit the details (and any download) are pinned to.
    pub revision: String,
    pub gated: bool,
    pub license: Option<String>,
    pub pipeline: Option<String>,
    pub library: Option<String>,
    pub params: Option<u64>,
    pub dtypes: BTreeMap<String, u64>,
    /// Bytes of the weights vLLM would load (served_files: safetensors, else .bin).
    pub weight_bytes: u64,
    /// Everything SparkBox downloads: the weights plus config and tokenizer files.
    pub download_bytes: u64,
    /// The whole repo, every copy and format included.
    pub repo_bytes: u64,
    /// What the download leaves out.
    pub skipped: Vec<Skipped>,
    /// "safetensors", "bin" or "none".
    pub weights_format: String,
    pub quantization: Option<String>,
    pub architectures: Vec<String>,
    pub model_type: Option<String>,
    /// config.json (the text model's part for multimodal models).
    pub config: Value,
    /// The vision tower's config, when the model has one (for the fit table's allowance).
    pub vision_config: Value,
    /// The files SparkBox downloads (served_files).
    pub files: Vec<HfFile>,
    pub readme_excerpt: String,
    /// MLX weights (Apple silicon only): no download, load or fit on a GB10.
    pub mlx: bool,
}

fn with_token(req: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    match token {
        Some(t) if !t.is_empty() => req.bearer_auth(t),
        _ => req,
    }
}

fn explain(status: u16, repo: &str) -> String {
    match status {
        401 | 403 => format!(
            "{repo} is gated or private. Accept its license on huggingface.co and add a Hugging Face token in Settings › Models."
        ),
        404 => format!("{repo} wasn't found on Hugging Face."),
        429 => "Hugging Face is rate-limiting requests; try again in a minute.".into(),
        s => format!("Hugging Face returned {s} for {repo}."),
    }
}

pub async fn search(client: &reqwest::Client, token: Option<&str>, query: &str) -> Result<Vec<SearchHit>, String> {
    // `expand` asks for the fields the list shows (without it the Hub leaves lastModified out);
    // config carries quantization_config, which the size estimate needs.
    let url = url::Url::parse_with_params(
        &format!("{HUB}/api/models"),
        &[
            ("search", query),
            ("sort", "downloads"),
            ("direction", "-1"),
            ("limit", "40"),
            ("expand[]", "downloads"),
            ("expand[]", "likes"),
            ("expand[]", "lastModified"),
            ("expand[]", "pipeline_tag"),
            ("expand[]", "tags"),
            ("expand[]", "safetensors"),
            ("expand[]", "config"),
            ("expand[]", "library_name"),
        ],
    )
    .map_err(|e| e.to_string())?;
    let req = client.get(url).timeout(Duration::from_secs(20));
    let resp = with_token(req, token).send().await.map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?;
    if !resp.status().is_success() {
        return Err(explain(resp.status().as_u16(), query));
    }
    let list: Vec<Value> = resp.json().await.map_err(|e| e.to_string())?;
    Ok(list.iter().map(parse_hit).collect())
}

pub fn parse_hit(m: &Value) -> SearchHit {
    let total = m["safetensors"]["total"].as_u64();
    let weight_bytes = m["safetensors"]["parameters"].as_object().map(|o| weight_bytes_from_params(o, total, &m["config"]["quantization_config"])).filter(|b| *b > 0);
    let id: String = m["id"].as_str().or(m["modelId"].as_str()).unwrap_or_default().into();
    let tags: Vec<String> = m["tags"].as_array().map(|t| t.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let library = m["library_name"].as_str().map(String::from);
    SearchHit {
        mlx: is_mlx(&id, library.as_deref(), &tags),
        id,
        downloads: m["downloads"].as_u64().unwrap_or(0),
        likes: m["likes"].as_u64().unwrap_or(0),
        pipeline: m["pipeline_tag"].as_str().map(String::from),
        tags,
        last_modified: m["lastModified"].as_str().map(String::from),
        params: total,
        weight_bytes,
        min_nodes: weight_bytes.and_then(|b| min_nodes(b, 4)),
        library,
    }
}

/// A list row's size from the repo's files: the weights vLLM serves, by the detail's own path
/// (served_files), so the list and the detail always agree.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sizing {
    pub id: String,
    pub weight_bytes: u64,
    /// "safetensors", "bin" or "none" (only GGUF or other formats).
    pub weights_format: String,
    pub min_nodes: Option<u64>,
    pub mlx: bool,
    /// The repo has GGUF files (for Ollama or llama.cpp).
    pub gguf: bool,
}

/// The repo's files (`/api/models/{id}?blobs=true` siblings) with their sizes.
fn files_of(info: &Value) -> Vec<HfFile> {
    info["siblings"]
        .as_array()
        .map(|s| {
            s.iter()
                .map(|f| HfFile {
                    path: f["rfilename"].as_str().unwrap_or_default().into(),
                    size: f["size"].as_u64().or_else(|| f["lfs"]["size"].as_u64()),
                    sha256: f["lfs"]["sha256"].as_str().map(String::from),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Bytes of the weights vLLM serves from `files` (0 when there are none it can serve).
fn served_weight_bytes(files: &[HfFile], format: &str) -> u64 {
    if format == "none" {
        return 0;
    }
    files.iter().filter(|f| f.path.ends_with(".safetensors") || f.path.ends_with(".bin")).filter_map(|f| f.size).sum()
}

fn tags_of(info: &Value) -> Vec<String> {
    info["tags"].as_array().map(|t| t.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn sizing_of(info: &Value, index: Option<&Value>) -> Sizing {
    let all = files_of(info);
    let (files, format, _) = served_files(&all, index);
    let weight_bytes = served_weight_bytes(&files, &format);
    let id: String = info["id"].as_str().unwrap_or_default().into();
    Sizing {
        mlx: is_mlx(&id, info["library_name"].as_str(), &tags_of(info)),
        gguf: all.iter().any(|f| f.path.to_ascii_lowercase().ends_with(".gguf")),
        min_nodes: (weight_bytes > 0).then(|| min_nodes(weight_bytes, 4)).flatten(),
        id,
        weight_bytes,
        weights_format: format,
    }
}

/// Reads a repo's file sizes (and its shard index, when it has one) and sizes it as the detail does.
pub async fn sizing(client: &reqwest::Client, token: Option<&str>, input: &str) -> Result<Sizing, String> {
    let repo = repo_id(input)?;
    let req = client.get(format!("{HUB}/api/models/{repo}?blobs=true")).timeout(Duration::from_secs(20));
    let resp = with_token(req, token).send().await.map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?;
    if !resp.status().is_success() {
        return Err(explain(resp.status().as_u16(), &repo));
    }
    let info: Value = resp.json().await.map_err(|e| e.to_string())?;
    let index = fetch_index(client, token, &repo, &info).await;
    Ok(sizing_of(&info, index.as_ref()))
}

/// The shard list, when the repo has one (gated repos refuse it without a token: then the root shards count).
async fn fetch_index(client: &reqwest::Client, token: Option<&str>, repo: &str, info: &Value) -> Option<Value> {
    let has_index = info["siblings"].as_array().is_some_and(|s| s.iter().any(|f| f["rfilename"] == "model.safetensors.index.json"));
    if !has_index {
        return None;
    }
    let rev = info["sha"].as_str().unwrap_or("main");
    let url = format!("{HUB}/{repo}/resolve/{rev}/model.safetensors.index.json");
    match with_token(client.get(url).timeout(Duration::from_secs(30)), token).send().await {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
        _ => None,
    }
}

/// Accepts "org/name" or a huggingface.co URL.
pub fn repo_id(input: &str) -> Result<String, String> {
    let s = input.trim().trim_end_matches('/');
    let s = s.strip_prefix("https://huggingface.co/").or_else(|| s.strip_prefix("huggingface.co/")).unwrap_or(s);
    let parts: Vec<&str> = s.split('/').collect();
    match parts.as_slice() {
        [org, name, ..] if !org.is_empty() && !name.is_empty() && !["tree", "blob"].contains(org) => Ok(format!("{org}/{name}")),
        _ => Err(format!("“{input}” isn't a Hugging Face model id like org/name.")),
    }
}

pub fn parse_model(info: &Value, config: Value, index: Option<&Value>, readme: &str) -> HfModel {
    let all = files_of(info);
    let (files, format, skipped) = served_files(&all, index);
    let weight_bytes = served_weight_bytes(&files, &format);

    let text_config = if config["text_config"].is_object() { config["text_config"].clone() } else { config.clone() };
    let quant = config["quantization_config"]
        .as_object()
        .or_else(|| info["config"]["quantization_config"].as_object())
        .map(|q| {
            [q.get("quant_method"), q.get("quant_algo")]
                .iter()
                .filter_map(|v| v.and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty());
    let dtypes: BTreeMap<String, u64> = info["safetensors"]["parameters"]
        .as_object()
        .map(|o| o.iter().filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n))).collect())
        .unwrap_or_default();
    let readme_excerpt: String = strip_front_matter(readme).chars().take(README_CHARS).collect();

    let id: String = info["id"].as_str().unwrap_or_default().into();
    HfModel {
        mlx: is_mlx(&id, info["library_name"].as_str(), &tags_of(info)),
        id,
        revision: info["sha"].as_str().unwrap_or("main").into(),
        gated: matches!(&info["gated"], Value::String(_)) || info["gated"] == Value::Bool(true),
        license: info["cardData"]["license"].as_str().map(String::from),
        pipeline: info["pipeline_tag"].as_str().map(String::from),
        library: info["library_name"].as_str().map(String::from),
        params: info["safetensors"]["total"].as_u64(),
        dtypes,
        weight_bytes,
        download_bytes: files.iter().filter_map(|f| f.size).sum(),
        repo_bytes: all.iter().filter_map(|f| f.size).sum(),
        skipped,
        weights_format: format,
        quantization: quant,
        architectures: config["architectures"]
            .as_array()
            .or_else(|| info["config"]["architectures"].as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        model_type: config["model_type"].as_str().or_else(|| info["config"]["model_type"].as_str()).map(String::from),
        config: text_config,
        vision_config: config["vision_config"].clone(),
        files,
        readme_excerpt,
    }
}

fn strip_front_matter(readme: &str) -> &str {
    if let Some(rest) = readme.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start();
        }
    }
    readme
}

pub async fn model(client: &reqwest::Client, token: Option<&str>, input: &str) -> Result<HfModel, String> {
    let repo = repo_id(input)?;
    let req = client.get(format!("{HUB}/api/models/{repo}?blobs=true")).timeout(Duration::from_secs(30));
    let resp = with_token(req, token).send().await.map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?;
    if !resp.status().is_success() {
        return Err(explain(resp.status().as_u16(), &repo));
    }
    let info: Value = resp.json().await.map_err(|e| e.to_string())?;
    let rev = info["sha"].as_str().unwrap_or("main").to_string();

    let get_text = |name: &str| {
        let url = format!("{HUB}/{repo}/resolve/{rev}/{name}");
        with_token(client.get(url).timeout(Duration::from_secs(30)), token).send()
    };
    let config = match get_text("config.json").await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let index = fetch_index(client, token, &repo, &info).await;
    let readme = match get_text("README.md").await {
        Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
        _ => String::new(),
    };
    Ok(parse_model(&info, config, index.as_ref(), &readme))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_hits_read_the_expanded_fields() {
        // As the Hub returns it with expand[]=lastModified,pipeline_tag,safetensors…
        let v = json!([
            { "_id": "x", "id": "Qwen/Qwen3.8-27B-FP8", "downloads": 41230, "likes": 812, "lastModified": "2026-09-30T11:02:41.000Z",
              "pipeline_tag": "image-text-to-text", "tags": ["transformers", "safetensors", "fp8"],
              "safetensors": { "parameters": { "BF16": 1_900_000_000u64, "F8_E4M3": 25_400_000_000u64 }, "total": 27_300_000_000u64 } },
            { "id": "meta-llama/Llama-3.3-70B-Instruct", "downloads": 9, "likes": 1,
              "safetensors": { "parameters": { "BF16": 70_553_706_496u64 }, "total": 70_553_706_496u64 } },
            { "id": "someone/no-index" }
        ]);
        let hits: Vec<SearchHit> = v.as_array().unwrap().iter().map(parse_hit).collect();
        assert_eq!(hits[0].last_modified.as_deref(), Some("2026-09-30T11:02:41.000Z"));
        assert_eq!(hits[0].pipeline.as_deref(), Some("image-text-to-text"));
        assert_eq!(hits[0].weight_bytes, Some(3_800_000_000 + 25_400_000_000));
        assert_eq!(hits[0].min_nodes, Some(1));
        assert_eq!(hits[1].weight_bytes, Some(141_107_412_992));
        assert_eq!(hits[1].min_nodes, Some(2), "141 GB of bf16 weights need two Sparks");
        assert_eq!((hits[2].weight_bytes, hits[2].min_nodes), (None, None));
    }

    fn fixture(name: &str) -> Value {
        let text = match name {
            "gpt-oss-120b.model" => include_str!("fixtures/hf/gpt-oss-120b.model.json"),
            "gpt-oss-120b.index" => include_str!("fixtures/hf/gpt-oss-120b.index.json"),
            "gpt-oss-120b.config" => include_str!("fixtures/hf/gpt-oss-120b.config.json"),
            "llama.model" => include_str!("fixtures/hf/llama-3.3-70b-instruct.model.json"),
            "mistral.model" => include_str!("fixtures/hf/mistral-small-3.2-24b.model.json"),
            "mistral.index" => include_str!("fixtures/hf/mistral-small-3.2-24b.index.json"),
            "mistral.config" => include_str!("fixtures/hf/mistral-small-3.2-24b.config.json"),
            "search" => include_str!("fixtures/hf/search.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    fn paths(m: &HfModel) -> Vec<&str> {
        m.files.iter().map(|f| f.path.as_str()).collect()
    }

    /// The fit table's first row (one Spark) for a model, as the Models window computes it.
    fn fits_on_one(m: &HfModel) -> bool {
        let mut arch = super::super::fit::arch_facts(&m.config);
        arch.vision_bytes = super::super::fit::vision_bytes(&m.vision_config);
        super::super::fit::fit_table(m.weight_bytes, &arch, super::super::fit::DEFAULT_NODE_MEMORY, 4)[0].fits
    }

    #[test]
    fn gpt_oss_120b_counts_its_root_shards_only_and_fits_on_one_spark() {
        let m = parse_model(&fixture("gpt-oss-120b.model"), fixture("gpt-oss-120b.config"), Some(&fixture("gpt-oss-120b.index")), "");
        assert_eq!(m.weight_bytes, 65_248_893_184);
        assert_eq!(format!("{:.1}", m.weight_bytes as f64 / GIB), "60.8", "the Hub's file view: 60.8 GiB");
        let p = paths(&m);
        assert_eq!(p.iter().filter(|f| f.ends_with(".safetensors")).count(), 15);
        assert!(!p.iter().any(|f| f.starts_with("original/") || f.starts_with("metal/")), "{p:?}");
        for needed in ["config.json", "generation_config.json", "tokenizer.json", "tokenizer_config.json", "special_tokens_map.json", "chat_template.jinja", "model.safetensors.index.json"] {
            assert!(p.contains(&needed), "{needed} is downloaded too");
        }
        assert!(!p.contains(&"README.md") && !p.contains(&"LICENSE"));
        assert_eq!(m.weights_format, "safetensors");
        assert_eq!(m.quantization.as_deref(), Some("mxfp4"));
        // What the download leaves out: original/ (the same weights again) and metal/ (Apple's).
        let skipped: Vec<(&str, u64)> = m.skipped.iter().map(|s| (s.what.as_str(), s.files)).collect();
        assert_eq!(skipped, [("original/", 7), ("metal/", 1)]);
        assert_eq!(format!("{:.1}", m.repo_bytes as f64 / GIB), "182.3");
        assert!(m.download_bytes < m.weight_bytes + 100_000_000, "the download is the weights plus small files");
        assert_eq!(min_nodes(m.weight_bytes, 4), Some(1));
        assert!(fits_on_one(&m), "NVIDIA's DGX Spark playbook serves it on one Spark");
    }

    #[test]
    fn llama_70b_leaves_out_its_original_pth_checkpoint() {
        // Gated: without a token the index can't be read, so the root shards count.
        let m = parse_model(&fixture("llama.model"), Value::Null, None, "");
        assert_eq!(m.weight_bytes, 141_107_497_872);
        assert_eq!(format!("{:.1}", m.weight_bytes as f64 / GIB), "131.4");
        let p = paths(&m);
        assert_eq!(p.iter().filter(|f| f.ends_with(".safetensors")).count(), 30);
        assert!(!p.iter().any(|f| f.ends_with(".pth") || f.starts_with("original/")), "{p:?}");
        assert_eq!(m.skipped[0].what, "original/");
        assert!(m.gated);
        assert_eq!(min_nodes(m.weight_bytes, 4), Some(2), "131 GiB of bf16 needs two Sparks");
    }

    #[test]
    fn mistral_skips_consolidated_safetensors_when_hf_shards_exist() {
        for index in [Some(fixture("mistral.index")), None] {
            let m = parse_model(&fixture("mistral.model"), fixture("mistral.config"), index.as_ref(), "");
            assert_eq!(m.weight_bytes, 48_022_800_560, "10 shards, not the shards plus consolidated.safetensors");
            let p = paths(&m);
            assert!(!p.contains(&"consolidated.safetensors"));
            assert!(p.contains(&"tekken.json"), "its tokenizer");
            assert_eq!(m.skipped[0].what, "consolidated weights");
            assert_eq!(m.skipped[0].bytes, 48_022_792_280);
            assert!(fits_on_one(&m));
        }
        // A repo with only consolidated.safetensors serves that.
        let only = vec![HfFile { path: "consolidated.safetensors".into(), size: Some(10), sha256: None }, HfFile { path: "params.json".into(), size: Some(1), sha256: None }];
        let (files, format, _) = served_files(&only, None);
        assert_eq!((files.len(), format.as_str()), (1, "safetensors"));
        // .bin only when there are no safetensors; GGUF and ONNX never.
        let mixed: Vec<HfFile> = ["pytorch_model.bin", "model.safetensors", "model.gguf", "onnx/model.onnx", "tf_model.h5"]
            .iter()
            .map(|p| HfFile { path: p.to_string(), size: Some(2_000_000), sha256: None })
            .collect();
        let (files, _, skipped) = served_files(&mixed, None);
        assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["model.safetensors"]);
        assert_eq!(skipped.len(), 4);
        let (files, format, _) = served_files(&mixed[..1], None);
        assert_eq!((files.len(), format.as_str()), (1, "bin"));
        let (files, format, _) = served_files(&mixed[2..3], None);
        assert_eq!((files.len(), format.as_str()), (0, "none"));
    }

    #[test]
    fn the_search_estimate_matches_the_detail_within_two_percent() {
        let hits: Vec<SearchHit> = fixture("search").as_array().unwrap().iter().map(parse_hit).collect();
        let details = [
            parse_model(&fixture("gpt-oss-120b.model"), fixture("gpt-oss-120b.config"), Some(&fixture("gpt-oss-120b.index")), ""),
            parse_model(&fixture("llama.model"), Value::Null, None, ""),
            parse_model(&fixture("mistral.model"), fixture("mistral.config"), Some(&fixture("mistral.index")), ""),
        ];
        for (h, d) in hits.iter().zip(&details) {
            assert_eq!(h.id, d.id);
            let est = h.weight_bytes.unwrap() as f64;
            let off = (est - d.weight_bytes as f64).abs() / d.weight_bytes as f64;
            assert!(off < 0.02, "{}: search {est} vs detail {} ({:.1}% off)", h.id, d.weight_bytes, off * 100.0);
            assert_eq!(h.min_nodes, min_nodes(d.weight_bytes, 4), "{}: the same verdict in both places", h.id);
        }
        // Counting MXFP4's U8 blocks at a byte each would say 119 GB and two Sparks.
        let p = fixture("search")[0]["safetensors"]["parameters"].as_object().unwrap().clone();
        assert!(weight_bytes_from_params(&p, None, &Value::Null) > 118_000_000_000);
        assert_eq!(hits[0].min_nodes, Some(1));
    }

    /// Real repos (trimmed Hub replies): MXFP4, MLX 8-bit and 4-bit, bitsandbytes 4-bit and AWQ.
    fn quant(name: &str) -> (Value, Value) {
        let (model, index) = match name {
            "mlx-8bit" => (include_str!("fixtures/hf/gpt-oss-120b-mlx-8bit.model.json"), include_str!("fixtures/hf/gpt-oss-120b-mlx-8bit.index.json")),
            "mlx-4bit" => (include_str!("fixtures/hf/gpt-oss-120b-mlx-4bit.model.json"), include_str!("fixtures/hf/gpt-oss-120b-mlx-4bit.index.json")),
            "bnb-4bit" => (include_str!("fixtures/hf/gpt-oss-120b-bnb-4bit.model.json"), include_str!("fixtures/hf/gpt-oss-120b-bnb-4bit.index.json")),
            "awq" => (include_str!("fixtures/hf/qwen3-32b-awq.model.json"), include_str!("fixtures/hf/qwen3-32b-awq.index.json")),
            "mxfp4" => (include_str!("fixtures/hf/gpt-oss-120b.model.json"), include_str!("fixtures/hf/gpt-oss-120b.index.json")),
            _ => unreachable!(),
        };
        (serde_json::from_str(model).unwrap(), serde_json::from_str(index).unwrap())
    }

    #[test]
    fn list_size_is_the_detail_size() {
        let hits: Vec<SearchHit> = serde_json::from_str::<Value>(include_str!("fixtures/hf/search-quants.json")).unwrap().as_array().unwrap().iter().map(parse_hit).collect();
        let gib = |b: u64| format!("{:.1}", b as f64 / GIB);
        for (name, want_gib) in [("mxfp4", "60.8"), ("mlx-8bit", "115.6"), ("mlx-4bit", "61.3"), ("bnb-4bit", "57.8"), ("awq", "18.0")] {
            let (info, index) = quant(name);
            let list = sizing_of(&info, Some(&index));
            let detail = parse_model(&info, info["config"].clone(), Some(&index), "");
            assert_eq!(list.weight_bytes, detail.weight_bytes, "{name}: the list row and the detail read the same files");
            assert_eq!(gib(list.weight_bytes), want_gib, "{name}: the Hub's file view");
            assert_eq!(list.min_nodes, min_nodes(detail.weight_bytes, 4), "{name}");
            assert_eq!(list.mlx, detail.mlx);
            // The fallback estimate from the parameter counts, packed tensors read by their bits.
            if let Some(h) = hits.iter().find(|h| h.id == list.id) {
                let est = h.weight_bytes.unwrap() as f64;
                let off = (est - list.weight_bytes as f64).abs() / list.weight_bytes as f64;
                assert!(off < 0.04, "{name}: estimate {} vs files {} ({:.1}% off)", gib(est as u64), gib(list.weight_bytes), off * 100.0);
                assert_eq!(h.mlx, list.mlx, "{name}: the same MLX verdict in both places");
            }
        }
        // MLX: Apple's format by library or tag (LM Studio's) or org (mlx-community, which tags nothing).
        assert!(sizing_of(&quant("mlx-8bit").0, None).mlx && sizing_of(&quant("mlx-4bit").0, None).mlx);
        assert!(!sizing_of(&quant("bnb-4bit").0, None).mlx && !sizing_of(&quant("awq").0, None).mlx && !sizing_of(&quant("mxfp4").0, None).mlx);
        assert!(is_mlx("someone/model", Some("mlx"), &[]));
        // Before: the 4-bit MLX listed 34.1 GiB (U32 words at 2 B) and bnb-4bit 110 GiB (U8 weights at 1 B).
        let h = hits.iter().find(|h| h.id == "mlx-community/gpt-oss-120b-4bit").unwrap();
        assert_eq!(gib(h.weight_bytes.unwrap()), "61.3");
    }

    #[test]
    fn repo_ids_from_names_and_urls() {
        assert_eq!(repo_id("Qwen/Qwen3.8-27B-FP8").unwrap(), "Qwen/Qwen3.8-27B-FP8");
        assert_eq!(repo_id("https://huggingface.co/nvidia/GLM-5.3-Flash-NVFP4/tree/main").unwrap(), "nvidia/GLM-5.3-Flash-NVFP4");
        assert!(repo_id("just-a-name").is_err());
    }

    #[test]
    fn model_details_pick_weights_quantization_and_text_config() {
        let info = json!({
            "id": "nvidia/GLM-5.3-Flash-NVFP4", "sha": "abc123", "gated": false, "pipeline_tag": "image-text-to-text",
            "cardData": { "license": "mit" },
            "safetensors": { "total": 168893635422u64, "parameters": { "BF16": 16460604030u64, "U8": 152429395968u64 } },
            "siblings": [
                { "rfilename": "config.json", "size": 4000 },
                { "rfilename": "model-00001-of-00002.safetensors", "size": 100, "lfs": { "sha256": "aa", "size": 100 } },
                { "rfilename": "model-00002-of-00002.safetensors", "lfs": { "sha256": "bb", "size": 50 } }
            ]
        });
        let config = json!({
            "architectures": ["Glm5NextForConditionalGeneration"], "model_type": "glm5_next",
            "quantization_config": { "quant_method": "modelopt", "quant_algo": "NVFP4" },
            "text_config": { "num_hidden_layers": 45 }
        });
        let m = parse_model(&info, config, None, "---\nlicense: mit\n---\n# GLM\nServe with vLLM.");
        assert_eq!(m.weight_bytes, 150);
        assert_eq!(m.weights_format, "safetensors");
        assert_eq!(m.quantization.as_deref(), Some("modelopt NVFP4"));
        assert_eq!(m.config["num_hidden_layers"], 45, "text_config is used for multimodal models");
        assert_eq!(m.files[2].sha256.as_deref(), Some("bb"));
        assert!(m.readme_excerpt.starts_with("# GLM"), "front matter dropped");
        assert!(!m.gated);
    }
}
