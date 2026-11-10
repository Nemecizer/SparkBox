//! Live inference metrics: the engine's Prometheus `/metrics` (vLLM, SGLang,
//! TensorRT-LLM, or a NIM's vLLM-style ones), read every 3 s on the head node,
//! turned into the numbers the vLLM production-stack dashboards show: tokens per
//! second in and out, requests running and queued, KV-cache use, and
//! time-to-first-token / inter-token latency percentiles. Counters become rates
//! (surviving a server restart), histograms become p50/p95 over the last minute,
//! and each endpoint keeps 15 minutes of history. A percentile that falls in a
//! wide histogram bucket is reported as that bucket's range, not a made-up
//! precise number. Ollama has no /metrics (and no KV gauge); its /api/ps (what's
//! loaded, where) is parsed here too.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How often the head node is asked for /metrics.
pub const SCRAPE_SECS: u64 = 3;
/// History kept per endpoint.
pub const HISTORY_SECS: u64 = 15 * 60;
/// Latency percentiles cover this much recent traffic (longer when it was quiet).
const WINDOW_MS: u64 = 60_000;
const LONG_WINDOW_MS: u64 = 5 * 60_000;

// ---- Prometheus text format

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub value: f64,
}

impl Sample {
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

fn parse_value(s: &str) -> Option<f64> {
    match s {
        "+Inf" | "Inf" => Some(f64::INFINITY),
        "-Inf" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        _ => s.parse().ok(),
    }
}

/// `{a="1",b="x\"y"}` → pairs; None when malformed.
fn parse_labels(s: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ',' || c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return Some(out);
        }
        let key: String = std::iter::from_fn(|| chars.next_if(|c| *c != '=')).collect();
        chars.next()?; // '='
        if chars.next()? != '"' {
            return None;
        }
        let mut val = String::new();
        loop {
            match chars.next()? {
                '\\' => match chars.next()? {
                    'n' => val.push('\n'),
                    c => val.push(c),
                },
                '"' => break,
                c => val.push(c),
            }
        }
        out.push((key.trim().to_string(), val));
    }
}

/// Parses the Prometheus text exposition format, skipping comments and anything malformed.
pub fn parse(text: &str) -> Vec<Sample> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, labels, rest) = match line.find('{') {
            Some(open) => {
                let Some(close) = line.rfind('}') else { continue };
                let Some(labels) = parse_labels(&line[open + 1..close]) else { continue };
                (&line[..open], labels, &line[close + 1..])
            }
            None => match line.split_once(char::is_whitespace) {
                Some((n, r)) => (n, Vec::new(), r),
                None => continue,
            },
        };
        let Some(value) = rest.split_whitespace().next().and_then(parse_value) else { continue };
        out.push(Sample { name: name.trim().to_string(), labels, value });
    }
    out
}

/// Sum of a metric across its label sets (e.g. every finished_reason).
pub fn sum(samples: &[Sample], name: &str) -> Option<f64> {
    let mut found = false;
    let mut total = 0.0;
    for s in samples.iter().filter(|s| s.name == name && s.value.is_finite()) {
        found = true;
        total += s.value;
    }
    found.then_some(total)
}

/// The first of several names that is present (vLLM renamed some metrics between versions).
fn first(samples: &[Sample], names: &[&str]) -> Option<f64> {
    names.iter().find_map(|n| sum(samples, n))
}

/// Cumulative histogram buckets summed across label sets: (upper bound, count), ascending.
pub fn buckets(samples: &[Sample], base: &str) -> Vec<(f64, f64)> {
    let name = format!("{base}_bucket");
    let mut by: BTreeMap<u64, (f64, f64)> = BTreeMap::new();
    for s in samples.iter().filter(|s| s.name == name) {
        let Some(le) = s.label("le").and_then(parse_value) else { continue };
        // Order by the bound itself (+Inf last); f64 bits sort correctly for non-negative numbers.
        let e = by.entry(le.to_bits()).or_insert((le, 0.0));
        e.1 += s.value;
    }
    by.into_values().collect()
}

/// Prometheus-style histogram_quantile: linear interpolation inside the bucket holding the rank.
pub fn quantile(q: f64, buckets: &[(f64, f64)]) -> Option<f64> {
    let total = buckets.last()?.1;
    if total <= 0.0 {
        return None;
    }
    let rank = q * total;
    let mut lower = (0.0, 0.0);
    for &(le, count) in buckets {
        if count >= rank {
            if le.is_infinite() {
                return Some(lower.0); // can't interpolate into +Inf: the highest finite bound
            }
            let span = count - lower.1;
            let frac = if span > 0.0 { (rank - lower.1) / span } else { 1.0 };
            return Some(lower.0 + (le - lower.0) * frac);
        }
        lower = (le, count);
    }
    None
}

/// Per-bucket difference between two cumulative histograms; None after a reset.
fn bucket_delta(now: &[(f64, f64)], then: &[(f64, f64)]) -> Option<Vec<(f64, f64)>> {
    if then.is_empty() {
        return Some(now.to_vec());
    }
    if now.len() != then.len() {
        return None;
    }
    let mut out = Vec::with_capacity(now.len());
    for (a, b) in now.iter().zip(then) {
        if a.0 != b.0 || a.1 < b.1 {
            return None;
        }
        out.push((a.0, a.1 - b.1));
    }
    Some(out)
}

/// A counter's increase per second, treating a drop as a restart from zero.
pub fn rate(now: f64, then: f64, secs: f64) -> f64 {
    if secs <= 0.0 {
        return 0.0;
    }
    let delta = if now >= then { now - then } else { now };
    delta / secs
}

/// The bucket that holds the q-th quantile: (lower, upper) bounds, when it is a finite one.
pub fn quantile_bucket(q: f64, buckets: &[(f64, f64)]) -> Option<(f64, f64)> {
    let total = buckets.last()?.1;
    if total <= 0.0 {
        return None;
    }
    let rank = q * total;
    let mut lower = 0.0;
    for &(le, count) in buckets {
        if count >= rank {
            return le.is_finite().then_some((lower, le));
        }
        lower = le;
    }
    None
}

/// A bucket too wide to quote one number from: its upper bound more than 25% above its lower
/// (vLLM's TTFT buckets go 0.25, 0.5, 0.75, 1, 2.5 s), or a first bucket wider than 50 ms.
fn coarse((lo, hi): (f64, f64)) -> bool {
    if lo > 0.0 {
        hi / lo > 1.25
    } else {
        hi > 0.05
    }
}

/// The q-th quantile's bucket in milliseconds, when the bucket is coarse.
fn range_ms(q: f64, buckets: &[(f64, f64)]) -> Option<[f64; 2]> {
    quantile_bucket(q, buckets).filter(|b| coarse(*b)).map(|(lo, hi)| [(lo * 1000.0).round(), (hi * 1000.0).round()])
}

// ---- One scrape, and the history built from them

/// Each number SparkBox reads, under the names the engines publish it: vLLM (and NIM, which
/// runs vLLM), SGLang (`--enable-metrics`) and trtllm-serve. The first name present wins.
const GEN: &[&str] = &["vllm:generation_tokens_total", "sglang:generation_tokens_total", "trtllm_generation_tokens_total"];
const PROMPT: &[&str] = &["vllm:prompt_tokens_total", "sglang:prompt_tokens_total", "trtllm_prompt_tokens_total"];
const DONE: &[&str] = &["vllm:request_success_total", "sglang:num_requests_total", "trtllm_request_success_total"];
const RUNNING: &[&str] = &["vllm:num_requests_running", "sglang:num_running_reqs", "trtllm_num_requests_running"];
const WAITING: &[&str] = &["vllm:num_requests_waiting", "sglang:num_queue_reqs", "trtllm_num_requests_waiting"];
const KV: &[&str] = &["vllm:kv_cache_usage_perc", "vllm:gpu_cache_usage_perc", "sglang:token_usage", "trtllm_kv_cache_utilization"];
const GEN_GAUGE: &[&str] = &["sglang:gen_throughput"];
const TTFT: &[&str] = &["vllm:time_to_first_token_seconds", "sglang:time_to_first_token_seconds", "trtllm_time_to_first_token_seconds"];
const ITL: &[&str] = &[
    "vllm:inter_token_latency_seconds",
    "vllm:time_per_output_token_seconds",
    "sglang:inter_token_latency_seconds",
    "sglang:time_per_output_token_seconds",
    "trtllm_time_per_output_token_seconds",
];
const E2E: &[&str] = &["vllm:e2e_request_latency_seconds", "sglang:e2e_request_latency_seconds", "trtllm_e2e_request_latency_seconds"];
const PREEMPT: &[&str] = &["vllm:num_preemptions_total", "sglang:num_retracted_reqs_total"];
/// Requests with NaNs in their logits. vLLM names the counter vllm:corrupted_requests and
/// prometheus_client publishes it with _total, as every counter here; it's only there at all
/// when vLLM runs with VLLM_COMPUTE_NANS_IN_LOGITS.
const CORRUPTED: &[&str] = &["vllm:corrupted_requests_total", "vllm:corrupted_requests"];

/// Optional engine counters, not a claim that speculative decoding is enabled or supported.
/// Preserve absence instead of inventing zeroes for an image that does not publish them.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpeculativeMetrics {
    pub drafts_total: Option<f64>,
    pub draft_tokens_total: Option<f64>,
    pub accepted_tokens_total: Option<f64>,
}
fn speculative(samples: &[Sample]) -> Option<SpeculativeMetrics> {
    let counter = |names: &[&str]| first(samples, names).filter(|v| v.is_finite() && *v >= 0.0);
    let m = SpeculativeMetrics {
        drafts_total: counter(&["vllm:spec_decode_num_drafts_total", "vllm:spec_decode_num_drafts"]),
        draft_tokens_total: counter(&["vllm:spec_decode_num_draft_tokens_total", "vllm:spec_decode_num_draft_tokens"]),
        accepted_tokens_total: counter(&["vllm:spec_decode_num_accepted_tokens_total", "vllm:spec_decode_num_accepted_tokens"]),
    };
    (m.drafts_total.is_some() || m.draft_tokens_total.is_some() || m.accepted_tokens_total.is_some()).then_some(m)
}

fn first_buckets(samples: &[Sample], names: &[&str]) -> Vec<(f64, f64)> {
    names.iter().map(|n| buckets(samples, n)).find(|b| !b.is_empty()).unwrap_or_default()
}

/// The parts of one /metrics reply SparkBox keeps.
#[derive(Clone, Debug, Default)]
pub struct Scrape {
    pub ts_ms: u64,
    pub prompt_tokens: Option<f64>,
    pub generation_tokens: Option<f64>,
    pub requests_done: Option<f64>,
    pub prefix_queries: Option<f64>,
    pub prefix_hits: Option<f64>,
    pub running: f64,
    pub waiting: f64,
    /// 0–1.
    pub kv_usage: Option<f64>,
    /// SGLang's own generation throughput gauge (tokens/s), used when it has no counter.
    pub gen_gauge: Option<f64>,
    /// Requests pushed out of the KV cache so far (a counter).
    pub preemptions: Option<f64>,
    /// Requests that came back with NaNs in their logits so far (a counter).
    pub corrupted: Option<f64>,
    pub speculative: Option<SpeculativeMetrics>,
    pub ttft: Vec<(f64, f64)>,
    pub itl: Vec<(f64, f64)>,
    pub e2e: Vec<(f64, f64)>,
}

impl Scrape {
    pub fn from_text(text: &str, ts_ms: u64) -> Scrape {
        let s = parse(text);
        Scrape {
            ts_ms,
            prompt_tokens: first(&s, PROMPT),
            generation_tokens: first(&s, GEN),
            requests_done: first(&s, DONE),
            prefix_queries: first(&s, &["vllm:prefix_cache_queries_total", "vllm:gpu_prefix_cache_queries_total"]),
            prefix_hits: first(&s, &["vllm:prefix_cache_hits_total", "vllm:gpu_prefix_cache_hits_total"]),
            running: first(&s, RUNNING).unwrap_or(0.0),
            waiting: first(&s, WAITING).unwrap_or(0.0),
            kv_usage: first(&s, KV),
            gen_gauge: first(&s, GEN_GAUGE),
            preemptions: first(&s, PREEMPT),
            corrupted: first(&s, CORRUPTED),
            speculative: speculative(&s),
            ttft: first_buckets(&s, TTFT),
            itl: first_buckets(&s, ITL),
            e2e: first_buckets(&s, E2E),
        }
    }

    /// True when the reply carried an inference engine's metrics at all.
    pub fn is_engine(&self) -> bool {
        self.generation_tokens.is_some() || self.kv_usage.is_some() || self.gen_gauge.is_some()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub ts: Vec<u64>,
    pub gen_tps: Vec<f64>,
    pub running: Vec<f64>,
    pub kv_pct: Vec<f64>,
    pub prompt_tps: Vec<f64>,
    pub waiting: Vec<f64>,
    pub ttft_p50_ms: Vec<Option<f64>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointMetrics {
    pub gen_tps: f64,
    pub prompt_tps: f64,
    pub running: f64,
    pub waiting: f64,
    pub kv_pct: f64,
    /// The engine publishes a KV-cache gauge (Ollama and llama.cpp don't: show "—").
    pub kv_known: bool,
    pub ttft_p50_ms: Option<f64>,
    pub ttft_p95_ms: Option<f64>,
    /// When the percentile fell in a wide histogram bucket: that bucket, in ms ("250–500 ms").
    pub ttft_p50_range: Option<[f64; 2]>,
    pub ttft_p95_range: Option<[f64; 2]>,
    pub itl_p50_ms: Option<f64>,
    pub itl_p50_range: Option<[f64; 2]>,
    pub e2e_p50_ms: Option<f64>,
    pub e2e_p95_ms: Option<f64>,
    pub req_per_s: f64,
    pub prefix_hit_pct: Option<f64>,
    /// The engine's preemption counter (the endpoint-preempt rule counts its growth).
    pub preemptions_total: Option<f64>,
    /// The engine's corrupted-request counter (the endpoint-corrupted rule counts its growth).
    pub corrupted_requests: Option<f64>,
    #[serde(default)]
    pub speculative: Option<SpeculativeMetrics>,
    pub updated_ms: u64,
    pub history: History,
}

/// Recent scrapes of one endpoint and the metrics derived from them.
#[derive(Default)]
pub struct Ring {
    scrapes: VecDeque<Scrape>,
    current: Option<EndpointMetrics>,
}

fn ms(v: Option<f64>) -> Option<f64> {
    v.map(|s| (s * 1000.0 * 10.0).round() / 10.0)
}

impl Ring {
    pub fn metrics(&self) -> Option<&EndpointMetrics> {
        self.current.as_ref()
    }

    /// The histogram over the last minute, or the last five when the last minute was quiet.
    fn windowed(&self, now: &Scrape, pick: impl Fn(&Scrape) -> &Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        for window in [WINDOW_MS, LONG_WINDOW_MS] {
            let then = self.scrapes.iter().find(|s| now.ts_ms.saturating_sub(s.ts_ms) <= window);
            let Some(then) = then else { continue };
            match bucket_delta(pick(now), pick(then)) {
                Some(d) if d.last().is_some_and(|l| l.1 > 0.0) => return d,
                Some(_) => continue,
                None => return pick(now).clone(), // the server restarted: what it has since then
            }
        }
        Vec::new()
    }

    pub fn push(&mut self, s: Scrape) -> &EndpointMetrics {
        let prev = self.scrapes.back();
        let secs = prev.map(|p| s.ts_ms.saturating_sub(p.ts_ms) as f64 / 1000.0).unwrap_or(0.0);
        let r = |now: Option<f64>, then: Option<f64>| match (now, then) {
            (Some(n), Some(t)) => rate(n, t, secs),
            _ => 0.0,
        };
        let gen_tps = match (s.generation_tokens, s.gen_gauge) {
            (None, Some(g)) => g.max(0.0),
            _ => r(s.generation_tokens, prev.and_then(|p| p.generation_tokens)),
        };
        let prompt_tps = r(s.prompt_tokens, prev.and_then(|p| p.prompt_tokens));
        let req_per_s = r(s.requests_done, prev.and_then(|p| p.requests_done));
        let prefix_hit_pct = match (s.prefix_queries, s.prefix_hits, prev.and_then(|p| p.prefix_queries), prev.and_then(|p| p.prefix_hits)) {
            (Some(q), Some(h), Some(q0), Some(h0)) => {
                let dq = rate(q, q0, 1.0);
                (dq > 0.0).then(|| 100.0 * rate(h, h0, 1.0) / dq)
            }
            _ => None,
        };
        let ttft = self.windowed(&s, |x| &x.ttft);
        let itl = self.windowed(&s, |x| &x.itl);
        let e2e = self.windowed(&s, |x| &x.e2e);
        let kv_pct = s.kv_usage.map(|k| (k * 100.0).clamp(0.0, 100.0)).unwrap_or(0.0);

        let mut history = self.current.take().map(|c| c.history).unwrap_or_default();
        let cutoff = (s.ts_ms / 1000).saturating_sub(HISTORY_SECS);
        while history.ts.first().is_some_and(|t| *t < cutoff) {
            history.ts.remove(0);
            history.gen_tps.remove(0);
            history.running.remove(0);
            history.kv_pct.remove(0);
            history.prompt_tps.remove(0);
            history.waiting.remove(0);
            history.ttft_p50_ms.remove(0);
        }
        let ttft_p50_ms = ms(quantile(0.5, &ttft));
        if prev.is_some() {
            history.ts.push(s.ts_ms / 1000);
            history.gen_tps.push(gen_tps);
            history.running.push(s.running);
            history.kv_pct.push(kv_pct);
            history.prompt_tps.push(prompt_tps);
            history.waiting.push(s.waiting);
            history.ttft_p50_ms.push(ttft_p50_ms);
        }
        let m = EndpointMetrics {
            gen_tps,
            prompt_tps,
            running: s.running,
            waiting: s.waiting,
            kv_pct,
            kv_known: s.kv_usage.is_some(),
            ttft_p50_ms,
            ttft_p95_ms: ms(quantile(0.95, &ttft)),
            ttft_p50_range: range_ms(0.5, &ttft),
            ttft_p95_range: range_ms(0.95, &ttft),
            itl_p50_ms: ms(quantile(0.5, &itl)),
            itl_p50_range: range_ms(0.5, &itl),
            e2e_p50_ms: ms(quantile(0.5, &e2e)),
            e2e_p95_ms: ms(quantile(0.95, &e2e)),
            req_per_s,
            prefix_hit_pct,
            preemptions_total: s.preemptions,
            corrupted_requests: s.corrupted,
            speculative: s.speculative.clone(),
            updated_ms: s.ts_ms,
            history,
        };
        self.scrapes.push_back(s);
        while self.scrapes.len() > (LONG_WINDOW_MS / 1000 / SCRAPE_SECS + 2) as usize {
            self.scrapes.pop_front();
        }
        self.current = Some(m);
        self.current.as_ref().unwrap()
    }
}

// ---- Ollama

/// A model Ollama holds in memory (/api/ps).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OllamaLoaded {
    pub name: String,
    pub size_bytes: u64,
    pub vram_bytes: u64,
    /// Unix seconds when Ollama unloads it.
    pub expires_at: Option<u64>,
    pub context_length: Option<u64>,
}

/// Unix seconds from an RFC 3339 time like "2026-10-28T10:15:00.123-04:00".
pub fn rfc3339_secs(s: &str) -> Option<u64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let time_end = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let mut t = rest[..time_end].split(':');
    let (hh, mm) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let ss = t.next().and_then(|x| x.split('.').next()?.parse::<i64>().ok()).unwrap_or(0);
    let offset = match &rest[time_end..] {
        "" | "Z" => 0,
        tz => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let mut p = tz[1..].split(':');
            let oh = p.next()?.parse::<i64>().ok()?;
            let om = p.next().and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
            sign * (oh * 3600 + om * 60)
        }
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss - offset).ok()
}

pub fn parse_ollama_ps(v: &Value) -> Vec<OllamaLoaded> {
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| OllamaLoaded {
            name: m["name"].as_str().or(m["model"].as_str()).unwrap_or_default().into(),
            size_bytes: m["size"].as_u64().unwrap_or(0),
            vram_bytes: m["size_vram"].as_u64().unwrap_or(0),
            expires_at: m["expires_at"].as_str().and_then(rfc3339_secs),
            context_length: m["context_length"].as_u64(),
        })
        .collect()
}

/// Model names from Ollama's /api/tags.
pub fn parse_ollama_tags(v: &Value) -> Vec<String> {
    v["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().or(m["model"].as_str()).map(String::from)).collect()
}

/// The same tags with their sizes on disk: [("gemma4:latest", 9_608_000_000)].
pub fn parse_ollama_sizes(v: &Value) -> Vec<(String, u64)> {
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["name"].as_str().or(m["model"].as_str()).map(|n| (n.to_string(), m["size"].as_u64().unwrap_or(0))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speculative_counters_are_optional_and_follow_the_current_engine_not_previous_totals() {
        let mut ring = Ring::default();
        let sample = |n| Scrape::from_text(&format!("vllm:generation_tokens_total 10\nvllm:spec_decode_num_drafts_total {n}\nvllm:spec_decode_num_draft_tokens_total 12\nvllm:spec_decode_num_accepted_tokens_total 8\nvllm:spec_decode_num_drafts_created 999\n"), 1000);
        let first = ring.push(sample(4)).speculative.as_ref().unwrap().clone();
        assert_eq!(first, SpeculativeMetrics { drafts_total: Some(4.0), draft_tokens_total: Some(12.0), accepted_tokens_total: Some(8.0) });
        assert_eq!(ring.push(sample(0)).speculative.as_ref().unwrap().drafts_total, Some(0.0), "a reset is not a negative delta or the old total");
        assert!(ring.push(Scrape::from_text("vllm:generation_tokens_total 0", 2000)).speculative.is_none());
        assert!(Scrape::from_text("vllm:spec_decode_num_drafts_total -1\nvllm:spec_decode_num_accepted_tokens_total NaN", 0).speculative.is_none());
        let labels = Scrape::from_text("vllm:spec_decode_num_drafts_total{engine=\"0\"} 3\nvllm:spec_decode_num_drafts_total{engine=\"1\"} 4\nvllm:spec_decode_num_drafts 7", 0);
        assert_eq!(labels.speculative.unwrap().drafts_total, Some(7.0), "sum engines, never count an alias twice");
    }

    const FIXTURE: &str = include_str!("fixtures/vllm_metrics.txt");

    const QWEN_IDLE: &str = include_str!("fixtures/qwen3-06b-cell1-2026-11-07/idle.prom");
    const QWEN_GENERATING: &str = include_str!("fixtures/qwen3-06b-cell1-2026-11-07/generating.prom");
    const QWEN_AFTER: &str = include_str!("fixtures/qwen3-06b-cell1-2026-11-07/after.prom");

    #[test]
    fn real_cell1_qwen_metrics_preserve_counts_histograms_and_absent_features() {
        for (text, generated, prompts, done, running, kv, first_tokens) in [
            (QWEN_IDLE, 12306.0, 2562.0, 7.0, 0.0, 0.0, 7.0),
            (QWEN_GENERATING, 15860.0, 4098.0, 9.0, 1.0, 0.010149013290374542, 10.0),
            (QWEN_AFTER, 20514.0, 5122.0, 12.0, 0.0, 0.0, 12.0),
        ] {
            let s = Scrape::from_text(text, 1_000);
            assert!(s.is_engine());
            assert_eq!((s.generation_tokens, s.prompt_tokens, s.requests_done), (Some(generated), Some(prompts), Some(done)));
            assert_eq!((s.running, s.waiting, s.kv_usage), (running, 0.0, Some(kv)));
            // An in-flight request already has a first token, but has not finished.
            assert_eq!(s.ttft.last(), Some(&(f64::INFINITY, first_tokens)));
            assert_eq!(s.e2e.last(), Some(&(f64::INFINITY, done)));
            assert_eq!(s.itl.last(), Some(&(f64::INFINITY, generated - first_tokens)));
            assert_eq!(s.preemptions, Some(0.0));
            assert!(s.corrupted.is_none(), "this image did not publish a corruption counter");
            assert!(s.speculative.is_none(), "Qwen did not run an MTP/speculative recipe");
        }
    }

    #[test]
    fn real_cell1_qwen_timeline_returns_to_idle_without_losing_the_kv_gauge() {
        let fixture: Value = serde_json::from_str(include_str!("fixtures/qwen3-06b-cell1-2026-11-07/node-samples.json")).unwrap();
        let mut ring = Ring::default();
        let mut active = 0;
        for (i, row) in fixture["timeline"].as_array().unwrap().iter().enumerate() {
            let s = Scrape::from_text(row["metrics"].as_str().unwrap(), row["elapsedMs"].as_u64().unwrap() + 1_000);
            let m = ring.push(s);
            assert!(m.kv_known);
            assert_eq!(m.waiting, 0.0);
            assert!(m.speculative.is_none());
            if m.running > 0.0 {
                active += 1;
                assert_eq!(m.running, 1.0);
                assert!(m.gen_tps > 0.0 && m.gen_tps < 150.0);
                assert!(m.kv_pct > 0.0 && m.kv_pct < 1.3, "fraction must become percent once");
            }
            if i >= 19 {
                assert_eq!((m.gen_tps, m.running, m.kv_pct), (0.0, 0.0, 0.0));
            }
        }
        assert_eq!(active, 17);
        assert_eq!(ring.metrics().unwrap().history.ts.len(), 25);
    }

    #[test]
    fn a_captured_vllm_scrape_parses() {
        let s = Scrape::from_text(FIXTURE, 1_000);
        assert!(s.is_engine());
        assert_eq!(s.running, 3.0);
        assert_eq!(s.waiting, 1.0);
        assert_eq!(s.kv_usage, Some(0.2178));
        assert_eq!(s.generation_tokens, Some(184_211.0));
        assert_eq!(s.prompt_tokens, Some(2_412_598.0));
        assert_eq!(s.requests_done, Some(851.0 + 37.0 + 2.0), "every finished_reason");
        assert_eq!(s.ttft.last(), Some(&(f64::INFINITY, 890.0)));
        assert!(!s.itl.is_empty(), "inter_token_latency_seconds");
        // _created and _sum samples aren't mistaken for the counters themselves.
        assert!(parse(FIXTURE).iter().any(|x| x.name == "vllm:generation_tokens_created"));
        // TTFT p50 sits in the 1.0–2.5 s bucket.
        let p50 = quantile(0.5, &s.ttft).unwrap();
        assert!((1.0..2.5).contains(&p50), "{p50}");
        let p95 = quantile(0.95, &s.ttft).unwrap();
        assert!(p95 > p50 && p95 <= 10.0, "{p95}");
    }

    /// Written in the format prometheus_client gives a counter (its _total sample and _created, one
    /// per engine), from vLLM's source: no Spark has published this counter to SparkBox yet.
    #[test]
    fn corrupted_requests_are_read_under_the_name_vllm_publishes() {
        let text = include_str!("fixtures/vllm_metrics_corrupted.txt");
        let s = Scrape::from_text(text, 1_000);
        assert!(s.is_engine());
        assert_eq!(s.corrupted, Some(2.0 + 1.0), "every engine");
        assert!(parse(text).iter().any(|x| x.name == "vllm:corrupted_requests_created"), "_created isn't the counter");
        let mut ring = Ring::default();
        assert_eq!(ring.push(s).corrupted_requests, Some(3.0));
        // The name research.md gives, in case a build publishes it bare.
        assert_eq!(Scrape::from_text("vllm:corrupted_requests{engine=\"0\"} 4.0\nvllm:generation_tokens_total 1\n", 0).corrupted, Some(4.0));
        // vLLM without VLLM_COMPUTE_NANS_IN_LOGITS has no such counter: nothing is known, not zero.
        assert_eq!(Scrape::from_text(FIXTURE, 1_000).corrupted, None);
    }

    #[test]
    fn sglang_and_trtllm_metrics_read_into_the_same_numbers() {
        let s = Scrape::from_text(include_str!("fixtures/sglang_metrics.txt"), 1_000);
        assert!(s.is_engine());
        assert_eq!((s.running, s.waiting, s.kv_usage), (2.0, 1.0, Some(0.31)));
        assert_eq!((s.generation_tokens, s.requests_done, s.gen_gauge), (Some(96_210.0), Some(411.0), Some(24.6)));
        assert_eq!(s.ttft.last(), Some(&(f64::INFINITY, 411.0)));
        assert!(!s.itl.is_empty(), "inter_token_latency_seconds");
        let mut ring = Ring::default();
        let m = ring.push(s).clone();
        assert_eq!(m.gen_tps, 0.0, "one scrape of counters has no rate…");
        assert!(m.kv_known && (m.kv_pct - 31.0).abs() < 1e-9);
        let gauge_only = Scrape { generation_tokens: None, ..Scrape::from_text(include_str!("fixtures/sglang_metrics.txt"), 4_000) };
        assert_eq!(ring.push(gauge_only).gen_tps, 24.6, "…but SGLang's own throughput gauge stands in for a missing counter");

        let t = Scrape::from_text(include_str!("fixtures/trtllm_metrics.txt"), 1_000);
        assert!(t.is_engine());
        assert_eq!((t.requests_done, t.generation_tokens, t.kv_usage), (Some(128.0), Some(40_211.0), Some(0.12)));
        assert_eq!(t.ttft.len(), 4);
        // trtllm-serve's /metrics without Prometheus enabled is JSON iteration stats: no engine metrics.
        assert!(!Scrape::from_text("[{\"gpuMemUsage\": 1}]", 0).is_engine());
    }

    #[test]
    fn a_percentile_in_a_wide_bucket_is_a_range() {
        // vLLM's TTFT buckets: …0.1, 0.25, 0.5, 0.75, 1.0…; 60 of 100 requests took 0.25–0.5 s.
        let coarse = [(0.1, 10.0), (0.25, 30.0), (0.5, 90.0), (0.75, 98.0), (1.0, 100.0), (f64::INFINITY, 100.0)];
        assert_eq!(range_ms(0.5, &coarse), Some([250.0, 500.0]));
        assert_eq!(range_ms(0.95, &coarse), Some([500.0, 750.0]));
        // 20 ms buckets are fine enough to quote one number.
        let fine: Vec<(f64, f64)> = (1..=40).map(|i| (i as f64 * 0.02, (i * 3) as f64)).chain([(f64::INFINITY, 120.0)]).collect();
        assert_eq!(range_ms(0.5, &fine), None);
        assert_eq!(quantile_bucket(0.5, &fine).map(|(lo, hi)| (lo * 1000.0, hi * 1000.0)).map(|(a, b)| (a.round(), b.round())), Some((380.0, 400.0)));
        let mut ring = Ring::default();
        ring.push(Scrape { ts_ms: 0, ..Default::default() });
        let m = ring.push(Scrape { ts_ms: 3_000, ttft: coarse.to_vec(), ..Default::default() }).clone();
        assert_eq!((m.ttft_p50_range, m.ttft_p95_range), (Some([250.0, 500.0]), Some([500.0, 750.0])));
        assert!(!m.kv_known, "no KV gauge: the UI shows —");
    }

    #[test]
    fn older_vllm_names_are_understood() {
        let text = "vllm:gpu_cache_usage_perc{model_name=\"m\"} 0.5\n\
                    vllm:time_per_output_token_seconds_bucket{le=\"0.1\",model_name=\"m\"} 4\n\
                    vllm:time_per_output_token_seconds_bucket{le=\"+Inf\",model_name=\"m\"} 4\n\
                    vllm:generation_tokens_total{model_name=\"m\"} 10\n";
        let s = Scrape::from_text(text, 0);
        assert_eq!(s.kv_usage, Some(0.5));
        assert_eq!(s.itl.len(), 2);
    }

    #[test]
    fn labels_with_escapes_and_odd_values_parse() {
        let v = parse("x{a=\"q\\\"uote\",b=\"c,d\"} 1.5e+03 1700000000\ny NaN\nbroken{a=\"1\" 3\nz{} +Inf\n");
        assert_eq!(v[0].labels, [("a".to_string(), "q\"uote".to_string()), ("b".to_string(), "c,d".to_string())]);
        assert_eq!(v[0].value, 1500.0);
        assert!(v[1].value.is_nan());
        assert_eq!(v.len(), 3, "the malformed line is skipped");
        assert_eq!(v[2].value, f64::INFINITY);
    }

    #[test]
    fn quantiles_interpolate_like_prometheus() {
        let b = [(0.1, 10.0), (0.5, 30.0), (1.0, 40.0), (f64::INFINITY, 40.0)];
        assert!((quantile(0.5, &b).unwrap() - 0.3).abs() < 1e-9, "rank 20 is halfway through 0.1–0.5");
        assert!((quantile(0.25, &b).unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(quantile(0.5, &[(0.1, 0.0), (f64::INFINITY, 0.0)]), None, "no requests, no latency");
        // Everything above the last finite bound: report that bound rather than infinity.
        assert_eq!(quantile(0.99, &[(0.1, 0.0), (f64::INFINITY, 5.0)]), Some(0.1));
    }

    fn scrape(ts_s: u64, gen: f64, ttft_counts: [f64; 3]) -> Scrape {
        Scrape {
            ts_ms: ts_s * 1000,
            generation_tokens: Some(gen),
            prompt_tokens: Some(gen * 10.0),
            requests_done: Some(gen / 100.0),
            running: 2.0,
            kv_usage: Some(0.25),
            ttft: vec![(0.5, ttft_counts[0]), (1.0, ttft_counts[1]), (f64::INFINITY, ttft_counts[2])],
            ..Default::default()
        }
    }

    #[test]
    fn counter_rates_survive_a_restart() {
        let mut r = Ring::default();
        r.push(scrape(100, 10_000.0, [0.0, 0.0, 0.0]));
        assert_eq!(r.metrics().unwrap().gen_tps, 0.0, "one scrape has no rate");
        let m = r.push(scrape(103, 10_060.0, [0.0, 0.0, 0.0])).clone();
        assert!((m.gen_tps - 20.0).abs() < 1e-9);
        assert!((m.prompt_tps - 200.0).abs() < 1e-9);
        assert_eq!(m.kv_pct, 25.0);
        // vLLM restarted: the counter starts again from 0, so the rate is what it has now.
        let m = r.push(scrape(106, 30.0, [0.0, 0.0, 0.0])).clone();
        assert!((m.gen_tps - 10.0).abs() < 1e-9, "{}", m.gen_tps);
        assert_eq!(m.history.gen_tps.len(), 2);
    }

    #[test]
    fn latency_percentiles_cover_the_last_minute() {
        let mut r = Ring::default();
        // Old traffic was slow (all above 1 s)…
        r.push(scrape(0, 0.0, [0.0, 0.0, 100.0]));
        r.push(scrape(30, 0.0, [0.0, 0.0, 100.0]));
        // …recent traffic is fast: 10 new requests, all under 0.5 s.
        r.push(scrape(100, 0.0, [10.0, 10.0, 110.0]));
        let m = r.push(scrape(103, 0.0, [10.0, 10.0, 110.0])).clone();
        let p50 = m.ttft_p50_ms.unwrap();
        assert!(p50 <= 500.0, "only the last minute counts: {p50}");
    }

    #[test]
    fn history_is_trimmed_to_fifteen_minutes() {
        let mut r = Ring::default();
        for i in 0..400u64 {
            r.push(scrape(i * 3, i as f64 * 100.0, [0.0, 0.0, 0.0]));
        }
        let h = &r.metrics().unwrap().history;
        assert!(h.ts.len() <= 301, "{}", h.ts.len());
        assert_eq!(h.ts.len(), h.kv_pct.len());
        assert!(h.ts.last().unwrap() - h.ts.first().unwrap() <= HISTORY_SECS);
    }

    #[test]
    fn ollama_ps_lists_whats_loaded() {
        let v: Value = serde_json::from_str(
            r#"{"models":[{"name":"qwen3:32b","model":"qwen3:32b","size":24800000000,"size_vram":24800000000,
                "expires_at":"2026-10-28T10:20:00.5-04:00","context_length":8192}]}"#,
        )
        .unwrap();
        let m = parse_ollama_ps(&v);
        assert_eq!(m[0].name, "qwen3:32b");
        assert_eq!(m[0].vram_bytes, 24_800_000_000);
        assert_eq!(m[0].expires_at, Some(rfc3339_secs("2026-10-28T14:20:00Z").unwrap()));
        assert_eq!(rfc3339_secs("1970-01-02T00:00:00Z"), Some(86_400));
        let tags = serde_json::json!({"models":[{"name":"gpt-oss:20b"},{"name":"qwen3:32b"}]});
        assert_eq!(parse_ollama_tags(&tags), ["gpt-oss:20b", "qwen3:32b"]);
    }
}
