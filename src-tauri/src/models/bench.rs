//! Benchmarks the way vLLM's own `benchmark_serving` measures: synthetic prompts
//! of an exact length (token ids, so no tokenizer guesswork), a fixed number of
//! output tokens (`ignore_eos` + `min_tokens`), a warm-up request, then a sweep
//! of concurrency levels. Each level reports requests per second, total output
//! tokens per second and p50/p90/p99 of time to first token, time per output
//! token and inter-token latency. Streamed progress goes to a task you can
//! cancel. Demo mode runs the same sweep against the simulated model.

use std::time::{Duration, Instant};

use futures_util::future::join_all;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::sim::SimModel;
use crate::llm::sse::SseParser;
use crate::llm::throughput::Meter;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BenchSpec {
    /// chat | rag | long | custom
    pub profile: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub concurrency: Vec<u64>,
    /// Requests at each level (at least the concurrency); 0 picks 2 × concurrency, at least 4.
    #[serde(default)]
    pub requests: u64,
}

impl BenchSpec {
    pub fn profile(name: &str) -> Option<BenchSpec> {
        let (i, o) = match name {
            "chat" => (512, 256),
            "rag" => (4096, 256),
            "long" => (32768, 512),
            _ => return None,
        };
        Some(BenchSpec { profile: name.into(), input_tokens: i, output_tokens: o, concurrency: vec![1, 2, 4, 8, 16, 32], requests: 0 })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.input_tokens == 0 || self.input_tokens > 1_000_000 || self.output_tokens == 0 || self.output_tokens > 32_768 {
            return Err("Pick between 1 and 1,000,000 prompt tokens and 1 to 32,768 output tokens.".into());
        }
        if self.concurrency.is_empty() || self.concurrency.iter().any(|c| *c == 0 || *c > 256) {
            return Err("Concurrency levels go from 1 to 256.".into());
        }
        Ok(())
    }

    fn requests_at(&self, c: u64) -> u64 {
        if self.requests > 0 { self.requests.max(c) } else { (2 * c).max(4) }
    }
}

/// Median, 90th and 99th percentile.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Pct {
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
}

/// Nearest-rank percentiles (what benchmark_serving reports, via numpy's linear rule).
pub fn percentiles(values: &[f64]) -> Pct {
    if values.is_empty() {
        return Pct::default();
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| {
        let pos = q * (v.len() - 1) as f64;
        let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
        v[lo] + (v[hi] - v[lo]) * (pos - lo as f64)
    };
    Pct { p50: at(0.5), p90: at(0.9), p99: at(0.99) }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BenchLevel {
    pub concurrency: u64,
    pub requests: u64,
    pub completed: u64,
    pub failed: u64,
    pub duration_s: f64,
    pub req_per_s: f64,
    /// Output tokens per second, all requests together.
    pub output_tps: f64,
    pub input_tps: f64,
    pub ttft_ms: Pct,
    /// Time per output token after the first, per request.
    pub tpot_ms: Pct,
    /// Every gap between two streamed tokens.
    pub itl_ms: Pct,
    pub e2e_ms: Pct,
}

/// One request's timing.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub ttft: f64,
    pub e2e: f64,
    pub tokens: u64,
    pub gaps: Vec<f64>,
}

pub fn level_from(concurrency: u64, requests: u64, wall: f64, input: u64, samples: &[Sample]) -> BenchLevel {
    let ok: Vec<&Sample> = samples.iter().filter(|s| s.tokens > 0).collect();
    let out: u64 = ok.iter().map(|s| s.tokens).sum();
    let wall = wall.max(1e-3);
    let tpot: Vec<f64> = ok.iter().filter(|s| s.tokens > 1).map(|s| (s.e2e - s.ttft) / (s.tokens - 1) as f64).collect();
    let gaps: Vec<f64> = ok.iter().flat_map(|s| s.gaps.iter().copied()).collect();
    BenchLevel {
        concurrency,
        requests,
        completed: ok.len() as u64,
        failed: requests - ok.len() as u64,
        duration_s: wall,
        req_per_s: ok.len() as f64 / wall,
        output_tps: out as f64 / wall,
        input_tps: (input * ok.len() as u64) as f64 / wall,
        ttft_ms: percentiles(&ok.iter().map(|s| s.ttft * 1000.0).collect::<Vec<_>>()),
        tpot_ms: percentiles(&tpot.iter().map(|t| t * 1000.0).collect::<Vec<_>>()),
        itl_ms: percentiles(&gaps.iter().map(|t| t * 1000.0).collect::<Vec<_>>()),
        e2e_ms: percentiles(&ok.iter().map(|s| s.e2e * 1000.0).collect::<Vec<_>>()),
    }
}

/// A prompt of exactly `n` tokens: token ids from the middle of any modern vocabulary.
pub fn prompt_ids(n: u64, seed: u64) -> Vec<u64> {
    let mut rng = fastrand::Rng::with_seed(seed);
    (0..n).map(|_| rng.u64(1_000..20_000)).collect()
}

/// The request body: completions with token ids, exactly `output` tokens out, streamed.
pub fn request_body(model: &str, spec: &BenchSpec, seed: u64) -> Value {
    json!({
        "model": model,
        "prompt": prompt_ids(spec.input_tokens, seed),
        "max_tokens": spec.output_tokens,
        "min_tokens": spec.output_tokens,
        "ignore_eos": true,
        "temperature": 0,
        "stream": true,
        "stream_options": { "include_usage": true },
    })
}

/// One streamed request against an OpenAI-style `/v1/completions`.
pub async fn run_one(client: &reqwest::Client, base: &str, key: Option<&str>, body: &Value) -> Result<Sample, String> {
    let mut meter = Meter::new();
    let mut req = client.post(format!("{}/completions", base.trim_end_matches('/'))).json(body).timeout(Duration::from_secs(1800));
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.map_err(|e| format!("Couldn't reach {base}: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        return Err(crate::llm::api_error("The endpoint", status.as_u16(), &v));
    }
    let mut parser = SseParser::default();
    let mut stream = resp.bytes_stream();
    let mut last: Option<Instant> = None;
    let mut gaps = Vec::new();
    let mut tokens = 0u64;
    let mut usage = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        for ev in parser.push(&chunk) {
            if ev.data.trim() == "[DONE]" {
                continue;
            }
            let v: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
            if v["choices"][0]["text"].as_str().is_some_and(|t| !t.is_empty()) {
                let now = Instant::now();
                if let Some(prev) = last {
                    gaps.push((now - prev).as_secs_f64());
                }
                last = Some(now);
                meter.delta_at(now);
                tokens += 1;
            }
            if let Some(n) = v["usage"]["completion_tokens"].as_u64() {
                usage = Some(n);
            }
        }
    }
    let ttft = meter.ttft().ok_or("No tokens came back.")?.as_secs_f64();
    Ok(Sample { ttft, e2e: meter.started.elapsed().as_secs_f64(), tokens: usage.unwrap_or(tokens), gaps })
}

/// One concurrency level: `requests` requests, at most `c` in flight.
pub async fn run_level(client: &reqwest::Client, base: &str, key: Option<&str>, model: &str, spec: &BenchSpec, c: u64, seed: u64) -> BenchLevel {
    let n = spec.requests_at(c);
    let started = Instant::now();
    let next = std::sync::atomic::AtomicU64::new(0);
    let worker = |_| async {
        let mut out = Vec::new();
        loop {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if i >= n {
                return out;
            }
            out.push(run_one(client, base, key, &request_body(model, spec, seed * 1000 + i)).await.unwrap_or_default());
        }
    };
    let samples: Vec<Sample> = join_all((0..c).map(worker)).await.into_iter().flatten().collect();
    level_from(c, n, started.elapsed().as_secs_f64(), spec.input_tokens, &samples)
}

/// The same level against the Demo's simulated model: the fit math sets each stream's
/// decode speed and prefill time, so a bf16 70B over three nodes is slow, and batching
/// raises the total while each stream slows a little.
pub fn simulate_level(m: &SimModel, spec: &BenchSpec, c: u64, seed: u64) -> BenchLevel {
    let mut rng = fastrand::Rng::with_seed(seed ^ c);
    let n = spec.requests_at(c);
    let per = m.per_stream_tps(c as f64);
    let prefill = m.prefill_tps();
    let mut samples = Vec::new();
    let mut lanes = vec![0.0f64; c as usize];
    for _ in 0..n {
        // The lane that frees up first takes the next request.
        let lane = lanes.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(0);
        let start = lanes[lane];
        // Prefill of everything arriving at once shares the GPU.
        let ttft = spec.input_tokens as f64 * (c as f64).min(4.0).max(1.0) / prefill * (0.9 + 0.2 * rng.f64()) + 0.05;
        let gap = 1.0 / per;
        let gaps: Vec<f64> = (1..spec.output_tokens).map(|_| gap * (0.93 + 0.14 * rng.f64())).collect();
        let e2e = ttft + gaps.iter().sum::<f64>();
        lanes[lane] = start + e2e;
        samples.push(Sample { ttft, e2e, tokens: spec.output_tokens, gaps });
    }
    let wall = lanes.iter().cloned().fold(0.0, f64::max);
    level_from(c, n, wall, spec.input_tokens, &samples)
}

// ---- Running a sweep as a task

use std::sync::Arc;

use super::endpoints::{Engine, ManagedBy};
use super::{now, save, Benchmark, BenchRow, Ctx, Models};
use crate::tasks::{TaskSpec, Tasks};

impl Models {
    /// Runs a sweep against an endpoint as a cancellable task; returns the task's id.
    pub async fn run_benchmark(self: &Arc<Self>, ctx: &Ctx, endpoint: &str, model: &str, spec: BenchSpec, tasks: Arc<Tasks>) -> Result<String, String> {
        spec.validate()?;
        let e = self.endpoints().into_iter().find(|e| e.id == endpoint).ok_or("That endpoint isn't there any more.")?;
        if e.health != super::endpoints::Health::Ready {
            return Err("It isn't ready yet.".into());
        }
        if !e.models.iter().any(|m| m == model) {
            return Err(format!("{} doesn't serve {model}.", e.id));
        }
        // vLLM (and NIM, which runs it), SGLang and TensorRT-LLM take token-id prompts on /v1/completions.
        if matches!(e.engine, Engine::Ollama | Engine::Llamacpp | Engine::Other) {
            return Err(format!("Benchmarks need a completions API that takes token-id prompts; {} has none.", e.engine.label()));
        }
        let provider = match &e.provider_id {
            Some(p) => self.llm.get(p).await,
            None => None,
        };
        let key = provider.as_ref().and_then(|p| p.api_key().ok().flatten());
        let base = provider.map(|p| p.base_url).unwrap_or_else(|| e.url.clone());
        let title = format!("Benchmark {model} ({} {}→{})", spec.profile, spec.input_tokens, spec.output_tokens);
        // A benchmark only sends requests: anyone can stop it, Admin mode or not.
        let task = tasks.start(TaskSpec::new("benchmark", title, &e.nodes).cancellable().read_only());
        let id = task.id().to_string();
        let me = self.clone();
        let (live, model) = (ctx.live, model.to_string());
        let sim = (!live).then(|| super::watch::sim_model_for(&e));
        tauri::async_runtime::spawn(async move {
            let client = match me.llm.client_for(&base).await {
                Ok(c) => c,
                Err(err) => return task.finish_err(err),
            };
            let seed = now();
            let mut levels = Vec::new();
            if live {
                task.step("Warm-up request");
                if let Err(err) = run_one(&client, &base, key.as_deref(), &request_body(&model, &BenchSpec { output_tokens: 16, ..spec.clone() }, seed)).await {
                    return task.finish_err(err);
                }
            }
            let total = spec.concurrency.len() as f32;
            for (i, c) in spec.concurrency.iter().copied().enumerate() {
                if task.is_cancelled() {
                    break;
                }
                task.step(format!("{c} at once ({} requests)", spec.requests_at(c)));
                let level = match &sim {
                    Some(m) => {
                        tokio::time::sleep(Duration::from_millis(700)).await;
                        simulate_level(m, &spec, c, seed)
                    }
                    None => tokio::select! {
                        l = run_level(&client, &base, key.as_deref(), &model, &spec, c, seed + i as u64) => l,
                        _ = task.cancelled() => break,
                    },
                };
                task.log(format!(
                    "{c} at once: {:.1} tok/s total, {:.2} req/s, TTFT p50 {:.0} ms, TPOT p50 {:.0} ms",
                    level.output_tps, level.req_per_s, level.ttft_ms.p50, level.tpot_ms.p50
                ));
                task.progress((i + 1) as f32 / total, Some(format!("{c} at once: {:.1} tok/s", level.output_tps)));
                levels.push(level);
            }
            if levels.is_empty() {
                return task.finish_cancelled("Stopped before the first level finished.");
            }
            let p = e.parallel.unwrap_or(super::endpoints::Parallel { tp: 1, pp: 1 });
            let b = Benchmark {
                deployment: e.deployment_id.clone().unwrap_or_else(|| e.id.clone()),
                served_name: model.clone(),
                nodes: e.nodes.clone(),
                tensor_parallel: p.tp,
                pipeline_parallel: p.pp,
                at: now(),
                rows: levels
                    .iter()
                    .map(|l| BenchRow {
                        concurrency: l.concurrency,
                        completed: l.completed,
                        ttft_ms_median: l.ttft_ms.p50 as u64,
                        tokens_per_sec_per_stream: if l.tpot_ms.p50 > 0.0 { 1000.0 / l.tpot_ms.p50 } else { 0.0 },
                        aggregate_tokens_per_sec: l.output_tps,
                    })
                    .collect(),
                demo: !live,
                spec: Some(spec.clone()),
                levels,
                endpoint: Some(e.id.clone()),
                external: e.managed_by == ManagedBy::External,
            };
            if !live {
                me.live.audit.record_simulated(&e.head_node, &format!("Benchmark {model}"), &format!("POST {base}/completions × {} levels", b.levels.len()));
            }
            let summary = b.levels.iter().map(|l| format!("{}: {:.1} tok/s", l.concurrency, l.output_tps)).collect::<Vec<_>>().join(" · ");
            {
                let mut all = me.benchmarks.lock().unwrap();
                all.push(b);
                save(&me.benchmarks_path, &*all);
            }
            if task.is_cancelled() {
                task.finish_cancelled(format!("Stopped; kept the finished levels ({summary})."));
            } else {
                task.finish_ok(summary);
            }
        });
        Ok(id)
    }

    /// Writes a benchmark (by its time) to Downloads as CSV or JSON; returns the path.
    pub fn export_benchmark(&self, at: u64, format: &str, dir: &std::path::Path) -> Result<String, String> {
        let b = self.benchmarks.lock().unwrap().iter().find(|b| b.at == at).cloned().ok_or("That benchmark is gone.")?;
        let name = format!("SparkBox-bench-{}-{at}", b.served_name.replace(['/', ':', ' '], "-"));
        let (path, text) = match format {
            "json" => (dir.join(format!("{name}.json")), serde_json::to_string_pretty(&b).map_err(|e| e.to_string())?),
            _ => {
                let mut csv = String::from(
                    "model,nodes,tp,pp,profile,input_tokens,output_tokens,concurrency,requests,completed,failed,duration_s,req_per_s,output_tok_s,input_tok_s,\
                     ttft_p50_ms,ttft_p90_ms,ttft_p99_ms,tpot_p50_ms,tpot_p90_ms,tpot_p99_ms,itl_p50_ms,itl_p90_ms,itl_p99_ms,e2e_p50_ms\n",
                );
                let s = b.spec.clone().unwrap_or_else(|| BenchSpec::profile("chat").unwrap());
                for l in &b.levels {
                    csv.push_str(&format!(
                        "{},{},{},{},{},{},{},{},{},{},{},{:.2},{:.3},{:.2},{:.2},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}\n",
                        b.served_name, b.nodes.join("+"), b.tensor_parallel, b.pipeline_parallel, s.profile, s.input_tokens, s.output_tokens,
                        l.concurrency, l.requests, l.completed, l.failed, l.duration_s, l.req_per_s, l.output_tps, l.input_tps,
                        l.ttft_ms.p50, l.ttft_ms.p90, l.ttft_ms.p99, l.tpot_ms.p50, l.tpot_ms.p90, l.tpot_ms.p99, l.itl_ms.p50, l.itl_ms.p90, l.itl_ms.p99, l.e2e_ms.p50
                    ));
                }
                (dir.join(format!("{name}.csv")), csv)
            }
        };
        crate::util::write_atomic(&path, text.as_bytes())?;
        Ok(path.display().to_string())
    }
}

/// The runs that belong to a data source: Live's (`Some(true)`) or Demo's (`Some(false)`); every
/// run when the source isn't known yet.
pub fn of_source(all: &[super::Benchmark], live: Option<bool>) -> Vec<super::Benchmark> {
    all.iter().filter(|b| live.is_none_or(|l| b.demo != l)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmarks_are_listed_per_data_source() {
        let run = |at: u64, demo: bool| super::super::Benchmark {
            deployment: "d".into(), served_name: "llama-3.3-70b".into(), nodes: vec!["cell1".into()], tensor_parallel: 1, pipeline_parallel: 3,
            at, rows: Vec::new(), demo, spec: None, levels: Vec::new(), endpoint: None, external: true,
        };
        let all = [run(1, true), run(2, false), run(3, true)];
        let at = |list: Vec<super::super::Benchmark>| list.iter().map(|b| b.at).collect::<Vec<_>>();
        assert_eq!(at(of_source(&all, Some(true))), [2], "Live never lists the Demo's runs");
        assert_eq!(at(of_source(&all, Some(false))), [1, 3]);
        assert_eq!(at(of_source(&all, None)), [1, 2, 3]);
    }

    #[test]
    fn percentiles_match_numpy() {
        let p = percentiles(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
        assert!((p.p50 - 5.5).abs() < 1e-9 && (p.p90 - 9.1).abs() < 1e-9 && (p.p99 - 9.91).abs() < 1e-9, "{p:?}");
        assert_eq!(percentiles(&[]), Pct::default());
        assert_eq!(percentiles(&[7.0]).p99, 7.0);
    }

    #[test]
    fn prompts_are_exactly_as_long_as_asked() {
        let spec = BenchSpec::profile("rag").unwrap();
        let body = request_body("m", &spec, 1);
        assert_eq!(body["prompt"].as_array().unwrap().len(), 4096);
        assert_eq!((body["max_tokens"].as_u64(), body["min_tokens"].as_u64()), (Some(256), Some(256)));
        assert_eq!(body["ignore_eos"], true);
        assert_ne!(request_body("m", &spec, 2)["prompt"], body["prompt"], "different requests, different prompts (no prefix-cache shortcut)");
        assert!(BenchSpec { concurrency: vec![0], ..spec.clone() }.validate().is_err());
    }

    #[test]
    fn the_demo_sweep_follows_the_fit_math() {
        let llama = SimModel { served: "llama".into(), weights: 141.1e9, tp: 1, pp: 3, kv_tokens: 330_000.0, max_seqs: 64.0, busyness: 0.0 };
        let spec = BenchSpec::profile("chat").unwrap();
        let one = simulate_level(&llama, &spec, 1, 7);
        let eight = simulate_level(&llama, &spec, 8, 7);
        // One stream: ~1.6 tok/s, so ~600 ms between tokens.
        assert!((550.0..700.0).contains(&one.tpot_ms.p50), "{:?}", one.tpot_ms);
        assert!((1.4..1.8).contains(&one.output_tps), "{}", one.output_tps);
        // Eight at once: each a little slower, the total several times higher.
        assert!(eight.tpot_ms.p50 > one.tpot_ms.p50);
        assert!(eight.output_tps > 6.0 * one.output_tps, "{} vs {}", eight.output_tps, one.output_tps);
        assert_eq!((eight.completed, eight.failed), (16, 0));
        assert!(eight.ttft_ms.p99 >= eight.ttft_ms.p50);
    }
}
