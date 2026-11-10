//! Demo mode's inference servers. A simulated vLLM keeps the same counters and
//! histograms the real one does and renders them as Prometheus text, which then
//! goes through the very parser live mode uses. Its speed comes from the fit
//! math: one stream decodes at about efficiency × 273 GB/s × TP ÷ weights
//! (pipeline parallel adds no single-stream speed; batching adds throughput),
//! so a bf16 Llama 70B over three nodes gives about 1.6 tok/s per request.
//! Load is a slow background curve with the occasional burst, plus whatever
//! Chat and the Server send to it. Latency has its own spread per engine:
//! prompts vary in length (log-normal), so a 70B pipeline's first token takes
//! about 0.4 s typically and 1 s at p95, Ollama's 20B about 0.14 and 0.3 s. The
//! histograms use 10 ms buckets, so their percentiles are precise (a real vLLM's
//! default buckets are 250 ms wide, and the UI shows those as ranges). A server
//! that is down produces nothing.

use super::fit::{DECODE_EFFICIENCY, NODE_BANDWIDTH};
use super::metrics::Scrape;
use crate::llm::{DemoSpeed, Traffic};

const E2E_BUCKETS: &[f64] = &[0.3, 0.5, 0.8, 1.0, 1.5, 2.0, 2.5, 5.0, 10.0, 15.0, 20.0, 30.0, 40.0, 50.0, 60.0, 120.0, 240.0, 480.0, 960.0, 1920.0, 7680.0];

/// Fine buckets for the simulators: 10 ms to 2 s, 50 ms to 6 s, then coarse.
fn fine(step_to: &[(f64, f64)], tail: &[f64]) -> &'static [f64] {
    let mut v = Vec::new();
    let mut at = 0.0;
    for &(step, to) in step_to {
        while at + step <= to + 1e-9 {
            at += step;
            v.push((at * 1000.0_f64).round() / 1000.0);
        }
    }
    v.extend_from_slice(tail);
    Box::leak(v.into_boxed_slice())
}

fn fine_ttft() -> &'static [f64] {
    static B: std::sync::OnceLock<&'static [f64]> = std::sync::OnceLock::new();
    B.get_or_init(|| fine(&[(0.01, 2.0), (0.05, 6.0)], &[10.0, 20.0, 40.0, 80.0]))
}

fn fine_itl() -> &'static [f64] {
    static B: std::sync::OnceLock<&'static [f64]> = std::sync::OnceLock::new();
    B.get_or_init(|| fine(&[(0.002, 0.1), (0.01, 1.5)], &[2.5, 5.0, 10.0]))
}

/// A log-normal sample with this median and spread (Box–Muller).
fn lognormal(rng: &mut fastrand::Rng, median: f64, sigma: f64) -> f64 {
    let (u1, u2) = (rng.f64().max(1e-12), rng.f64());
    let z = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
    median * (sigma * z).exp()
}

/// Batched decode slows each stream a little per extra request.
const BATCH_PENALTY: f64 = 0.015;

/// One histogram: per-bucket counts (the last is +Inf), sum and count.
#[derive(Clone)]
struct Hist {
    bounds: &'static [f64],
    counts: Vec<f64>,
    sum: f64,
}

impl Hist {
    fn new(bounds: &'static [f64]) -> Hist {
        Hist { bounds, counts: vec![0.0; bounds.len() + 1], sum: 0.0 }
    }

    fn observe(&mut self, v: f64, n: f64) {
        let i = self.bounds.iter().position(|b| v <= *b).unwrap_or(self.bounds.len());
        self.counts[i] += n;
        self.sum += v * n;
    }

    fn cumulative(&self) -> Vec<(f64, f64)> {
        let mut acc = 0.0;
        self.bounds
            .iter()
            .copied()
            .chain(std::iter::once(f64::INFINITY))
            .zip(&self.counts)
            .map(|(b, c)| {
                acc += c;
                (b, acc)
            })
            .collect()
    }

    fn render(&self, out: &mut String, name: &str, labels: &str) {
        for (b, c) in self.cumulative() {
            let le = if b.is_infinite() { "+Inf".to_string() } else { format!("{b}") };
            out.push_str(&format!("{name}_bucket{{{labels},le=\"{le}\"}} {c:.1}\n"));
        }
        let count: f64 = self.counts.iter().sum();
        out.push_str(&format!("{name}_count{{{labels}}} {count:.1}\n{name}_sum{{{labels}}} {:.3}\n", self.sum));
    }
}

/// What a simulated server is serving.
#[derive(Clone, Debug)]
pub struct SimModel {
    pub served: String,
    /// Bytes of weights read per decoded token (all of them for a dense model).
    pub weights: f64,
    pub tp: u64,
    pub pp: u64,
    /// KV-cache capacity in tokens across the deployment.
    pub kv_tokens: f64,
    pub max_seqs: f64,
    /// Background requests, roughly: 0 for a quiet model, 2–3 for a busy shared one.
    pub busyness: f64,
}

impl SimModel {
    /// One stream's decode speed with `running` requests in the batch.
    pub fn per_stream_tps(&self, running: f64) -> f64 {
        let single = DECODE_EFFICIENCY * NODE_BANDWIDTH * self.tp as f64 / self.weights.max(1.0);
        single / (1.0 + BATCH_PENALTY * (running - self.pp as f64).max(0.0))
    }

    /// Prefill is compute-bound: about 7,500 tokens/s per 10 GB of weights per GPU in the split.
    pub fn prefill_tps(&self) -> f64 {
        7.5e3 * (self.tp * self.pp) as f64 * 1e10 / self.weights.max(1.0)
    }
}

pub struct VllmSim {
    pub model: SimModel,
    phase: f64,
    t: f64,
    rng: fastrand::Rng,
    prompt_total: f64,
    gen_total: f64,
    done: [f64; 3],
    prefix_queries: f64,
    prefix_hits: f64,
    pending_done: f64,
    ttft: Hist,
    itl: Hist,
    e2e: Hist,
    pub running: f64,
    pub waiting: f64,
    pub kv: f64,
    preemptions: f64,
    created: f64,
    /// The endpoint-slow scenario: more demand than it can take, so a queue builds.
    pub slow: bool,
}

/// Prompt and reply sizes of the background requests (prompts vary log-normally around this).
const BG_PROMPT: f64 = 1_100.0;
const BG_OUTPUT: f64 = 280.0;
/// Tokens a running background request holds in the KV cache on average (chat history, documents).
const BG_CONTEXT: f64 = 9_000.0;
const PREFIX_HIT: f64 = 0.46;

impl VllmSim {
    pub fn new(model: SimModel, seed: u64) -> VllmSim {
        let mut rng = fastrand::Rng::with_seed(seed);
        let phase = rng.f64() * std::f64::consts::TAU;
        let mut sim = VllmSim {
            model,
            phase,
            t: 0.0,
            rng,
            prompt_total: 0.0,
            gen_total: 0.0,
            done: [0.0; 3],
            prefix_queries: 0.0,
            prefix_hits: 0.0,
            pending_done: 0.0,
            ttft: Hist::new(fine_ttft()),
            itl: Hist::new(fine_itl()),
            e2e: Hist::new(E2E_BUCKETS),
            running: 0.0,
            waiting: 0.0,
            kv: 0.0,
            preemptions: 0.0,
            created: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0),
            slow: false,
        };
        // Start with a few hours of history in the counters, like a server that has been up a while.
        for _ in 0..120 {
            sim.step(60.0, &Traffic::default());
        }
        sim.t = 0.0;
        sim
    }

    /// Background requests in flight at time t: a slow tide, plus a burst every few minutes.
    fn background(&mut self) -> f64 {
        let b = self.model.busyness;
        if b <= 0.0 {
            return 0.0;
        }
        let tide = 0.5 + 0.5 * (self.t / 240.0 + self.phase).sin();
        let burst = if (self.t / 420.0 + self.phase).fract() < 0.08 { 2.2 * b } else { 0.0 };
        let slow = if self.slow { b + 5.0 } else { 0.0 };
        (b * (0.45 + tide) + burst + slow + (self.rng.f64() - 0.5) * 0.8).max(0.0).round()
    }

    /// A round while the server is down: nothing runs, nothing waits, the counters stand still.
    pub fn idle(&mut self) {
        self.running = 0.0;
        self.waiting = 0.0;
        self.kv = 0.0;
    }

    /// A round while the engine hangs (a stage stopped answering, or NCCL waits on a peer that
    /// never replies): the requests in flight stay in flight, new ones queue behind them, and no
    /// token comes out. The counters and histograms stand still.
    pub fn stall(&mut self, dt: f64) {
        self.t += dt;
        let arriving = self.background();
        self.running = self.running.max(3.0).min(self.model.max_seqs);
        self.waiting = (self.waiting + (arriving - self.running).max(0.0) * dt / 60.0).min(self.model.max_seqs * 4.0);
    }

    /// Prefix-cache hits over everything it has served (%), as vLLM's stats line reports it.
    pub fn prefix_hit_pct(&self) -> f64 {
        if self.prefix_queries > 0.0 { 100.0 * self.prefix_hits / self.prefix_queries } else { 0.0 }
    }

    /// How long one test token takes right now (ms): a full pass through the model (the whole
    /// pipeline for PP), after any queue, plus the request itself.
    pub fn canary_ms(&self) -> f64 {
        let seqs = if self.slow { self.model.max_seqs.min(6.0) } else { self.model.max_seqs };
        let pass = 1000.0 / (self.model.per_stream_tps(self.running + 1.0) * if self.slow { 0.5 } else { 1.0 }).max(0.01);
        let queue = if self.running + 1.0 > seqs { 250.0 * (self.waiting + 1.0) } else { 0.0 };
        pass + queue + 12.0
    }

    /// Advances `dt` seconds with `chat` (what Chat and the Server sent since the last step).
    pub fn step(&mut self, dt: f64, chat: &Traffic) {
        self.t += dt;
        let bg = self.background();
        let demand = bg + chat.active as f64;
        // vLLM admits up to max_num_seqs, and only while the KV cache has room.
        let kv_cap = (self.model.kv_tokens / BG_CONTEXT).floor().max(1.0);
        let seqs = if self.slow { self.model.max_seqs.min(6.0) } else { self.model.max_seqs };
        let running = demand.min(seqs).min(kv_cap);
        self.waiting = demand - running;
        self.running = running;
        let bg_running = (running - chat.active as f64).max(0.0);
        // endpoint-slow: long prompts (RAG) crowd the pipeline: prefill interferes and decode halves.
        let per = self.model.per_stream_tps(running) * if self.slow { 0.5 } else { 1.0 };
        let (prompt_median, hit) = if self.slow { (3_000.0, 0.3) } else { (BG_PROMPT, PREFIX_HIT) };

        // Decode: background streams at the shared speed, plus what Chat really produced.
        let bg_tokens = bg_running * per * dt;
        self.gen_total += bg_tokens + chat.output_tokens as f64;
        let gap = 1.0 / per.max(0.01);
        let tokens = (bg_tokens + chat.output_tokens as f64).max(0.0).round();
        for _ in 0..4 {
            self.itl.observe(gap * (0.94 + 0.12 * self.rng.f64()), (tokens / 4.0).round());
        }

        // Requests finishing at the rate their length allows, each with a TTFT and an end-to-end time.
        let duration = BG_OUTPUT / per.max(0.01);
        self.pending_done += bg_running * dt / duration.max(1.0);
        let finished = self.pending_done.floor();
        self.pending_done -= finished;
        let prefill = self.model.prefill_tps();
        for _ in 0..finished as u32 {
            // A request that waited went through the queue's prefills first: a stylised quarter second each.
            let queue = self.waiting * 0.25 * self.rng.f64();
            let prompt = lognormal(&mut self.rng, prompt_median, 0.55);
            let ttft = prompt * (1.0 - hit) / prefill + queue + 0.07;
            self.ttft.observe(ttft, 1.0);
            self.e2e.observe(ttft + duration * (0.6 + 0.8 * self.rng.f64()), 1.0);
            let reason = if self.rng.f64() < 0.94 { 2 } else if self.rng.f64() < 0.9 { 1 } else { 0 };
            self.done[reason] += 1.0;
            self.prompt_total += BG_PROMPT;
            self.prefix_queries += BG_PROMPT;
            self.prefix_hits += BG_PROMPT * PREFIX_HIT;
        }
        for t in &chat.ttft_ms {
            self.ttft.observe(t / 1000.0, 1.0);
        }
        if chat.finished > 0 {
            self.done[2] += chat.finished as f64;
        }
        self.prompt_total += chat.prompt_tokens as f64;
        self.prefix_queries += chat.prompt_tokens as f64;
        let ctx = bg_running * BG_CONTEXT + chat.active as f64 * 2_000.0;
        self.kv = (ctx / self.model.kv_tokens.max(1.0)).min(1.0);
        // A full cache pushes the newest request out to make room (vLLM recomputes it later).
        if self.kv >= 0.97 && self.rng.f64() < 0.3 {
            self.preemptions += 1.0;
        }
    }

    /// How a new request would be served right now (Chat streams at this).
    pub fn speed(&self) -> DemoSpeed {
        let seqs = if self.slow { self.model.max_seqs.min(6.0) } else { self.model.max_seqs };
        let per = self.model.per_stream_tps(self.running + 1.0) * if self.slow { 0.5 } else { 1.0 };
        let queue = if self.running + 1.0 > seqs { 250.0 * (self.waiting + 1.0) } else { 0.0 };
        let prompt = if self.slow { 2_100.0 } else { 400.0 };
        DemoSpeed { tps: per, ttft_ms: prompt / self.model.prefill_tps() * 1000.0 + 120.0 + queue }
    }

    /// The server's /metrics, in vLLM's own names and format.
    pub fn prometheus(&self) -> String {
        let l = format!("engine=\"0\",model_name=\"{}\"", self.model.served);
        let mut s = String::new();
        let gauge = |s: &mut String, name: &str, v: f64| s.push_str(&format!("# TYPE {name} gauge\n{name}{{{l}}} {v}\n"));
        let counter = |s: &mut String, name: &str, v: f64| {
            let base = name.trim_end_matches("_total");
            s.push_str(&format!("# TYPE {name} counter\n{name}{{{l}}} {v:.1}\n{base}_created{{{l}}} {:.3}\n", self.created));
        };
        gauge(&mut s, "vllm:num_requests_running", self.running);
        gauge(&mut s, "vllm:num_requests_waiting", self.waiting);
        gauge(&mut s, "vllm:kv_cache_usage_perc", (self.kv * 10_000.0).round() / 10_000.0);
        counter(&mut s, "vllm:prefix_cache_queries_total", self.prefix_queries.round());
        counter(&mut s, "vllm:prefix_cache_hits_total", self.prefix_hits.round());
        counter(&mut s, "vllm:prompt_tokens_total", self.prompt_total.round());
        counter(&mut s, "vllm:generation_tokens_total", self.gen_total.round());
        counter(&mut s, "vllm:num_preemptions_total", self.preemptions);
        s.push_str("# TYPE vllm:request_success_total counter\n");
        for (i, reason) in ["abort", "length", "stop"].iter().enumerate() {
            s.push_str(&format!("vllm:request_success_total{{engine=\"0\",finished_reason=\"{reason}\",model_name=\"{}\"}} {:.1}\n", self.model.served, self.done[i]));
        }
        for (h, name) in [(&self.ttft, "vllm:time_to_first_token_seconds"), (&self.itl, "vllm:inter_token_latency_seconds"), (&self.e2e, "vllm:e2e_request_latency_seconds")] {
            s.push_str(&format!("# TYPE {name} histogram\n"));
            h.render(&mut s, name, &l);
        }
        s
    }

}

/// What vLLM's engine-stats line reports: its averages over the last logging interval.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EngineStats {
    pub prompt_tps: f64,
    pub gen_tps: f64,
    pub running: f64,
    pub waiting: f64,
    pub kv_pct: f64,
    pub prefix_hit_pct: f64,
}

/// The line vLLM logs every 10 s (loggers.py), stamped at Unix time `at` in the nodes' UTC.
pub fn stats_line(at: u64, s: &EngineStats) -> String {
    let (_, mo, d, h, m, sec, _) = crate::demo::ops::civil(at);
    format!(
        "INFO {mo:02}-{d:02} {h:02}:{m:02}:{sec:02} [loggers.py:123] Engine 000: Avg prompt throughput: {:.1} tokens/s, Avg generation throughput: {:.1} tokens/s, \
         Running: {:.0} reqs, Waiting: {:.0} reqs, GPU KV cache usage: {:.1}%, Prefix cache hit rate: {:.1}%",
        s.prompt_tps, s.gen_tps, s.running, s.waiting, s.kv_pct, s.prefix_hit_pct
    )
}

/// Ollama in the demo: two models in memory, light background chat, and what SparkBox sends.
/// Ollama has no /metrics (and no KV gauge), so this produces the same counters SparkBox keeps
/// for a live one: tokens, requests, time to first token and between tokens.
pub struct OllamaSim {
    t: f64,
    rng: fastrand::Rng,
    gen_total: f64,
    prompt_total: f64,
    done: f64,
    ttft: Hist,
    itl: Hist,
    pending: f64,
    pub running: f64,
    /// Tokens per second over the last round, for the GPU's load.
    pub tps: f64,
    /// Whether a chat session is generating now, when the caller knows (Demo: the node's trace).
    pub busy: Option<bool>,
}

/// (name, bytes in memory, decode tok/s for one stream on a GB10).
pub const OLLAMA_MODELS: &[(&str, u64, f64)] = &[("gpt-oss:20b", 14_600_000_000, 48.0), ("qwen3:32b", 29_960_000_000, 9.6)];

impl Default for OllamaSim {
    fn default() -> Self {
        OllamaSim {
            t: 0.0,
            rng: fastrand::Rng::with_seed(11434),
            gen_total: 0.0,
            prompt_total: 0.0,
            done: 0.0,
            ttft: Hist::new(fine_ttft()),
            itl: Hist::new(fine_itl()),
            pending: 0.0,
            running: 0.0,
            tps: 0.0,
            busy: None,
        }
    }
}

impl OllamaSim {
    /// Ollama serves requests one model at a time per slot; most of the demo traffic uses the small, fast model.
    pub fn step(&mut self, dt: f64, sparkbox: &Traffic) -> Scrape {
        self.t += dt;
        let chatting = self.busy.unwrap_or_else(|| (self.t / 90.0).sin() > 0.35 || (self.t / 37.0).cos() > 0.92);
        let bg = if chatting { 1.0 } else { 0.0 };
        self.running = bg + sparkbox.active as f64;
        let tps = OLLAMA_MODELS[0].2 * (0.92 + 0.16 * self.rng.f64());
        self.gen_total += bg * tps * dt + sparkbox.output_tokens as f64;
        self.tps = bg * tps + sparkbox.output_tokens as f64 / dt.max(0.1);
        self.prompt_total += sparkbox.prompt_tokens as f64;
        self.pending += bg * dt / 12.0;
        while self.pending >= 1.0 {
            self.pending -= 1.0;
            self.done += 1.0;
            self.prompt_total += 600.0;
            let ttft = lognormal(&mut self.rng, 0.14, 0.48);
            self.ttft.observe(ttft, 1.0);
            self.itl.observe(1.0 / tps * (0.96 + 0.08 * self.rng.f64()), 1.0);
        }
        for t in &sparkbox.ttft_ms {
            self.ttft.observe(t / 1000.0, 1.0);
        }
        for t in &sparkbox.itl_ms {
            self.itl.observe(t / 1000.0, 1.0);
        }
        self.done += sparkbox.finished as f64;
        self.scrape()
    }

    /// A round while Ollama is down: nothing runs, the counters stand still.
    pub fn idle(&mut self) -> Scrape {
        self.running = 0.0;
        self.tps = 0.0;
        self.scrape()
    }

    fn scrape(&self) -> Scrape {
        Scrape {
            prompt_tokens: Some(self.prompt_total),
            generation_tokens: Some(self.gen_total),
            requests_done: Some(self.done),
            running: self.running,
            ttft: self.ttft.cumulative(),
            itl: self.itl.cumulative(),
            ..Default::default()
        }
    }

    pub fn speed(&self, model: &str) -> DemoSpeed {
        let tps = OLLAMA_MODELS.iter().find(|m| m.0 == model).map(|m| m.2).unwrap_or(OLLAMA_MODELS[0].2);
        DemoSpeed { tps: tps / (1.0 + 0.1 * self.running), ttft_ms: 260.0 }
    }

    /// One test token to a model in memory (ms): a short prompt's prefill and one decode step,
    /// waiting its turn behind a request that's running. `jitter` 0–1 spreads it a little.
    pub fn canary_ms(&self, model: &str, jitter: f64) -> f64 {
        let tps = OLLAMA_MODELS.iter().find(|m| m.0 == model).map(|m| m.2).unwrap_or(OLLAMA_MODELS[0].2);
        110.0 + 1000.0 / tps + 60.0 * jitter + if self.running > 0.0 { 1000.0 / tps * 4.0 } else { 0.0 }
    }
}

/// SparkBox's own counters for an engine with no metrics endpoint (a live Ollama).
#[derive(Default)]
pub struct Observed {
    gen_total: f64,
    prompt_total: f64,
    done: f64,
    ttft: Option<Hist>,
    itl: Option<Hist>,
}

impl Observed {
    /// SparkBox times every request itself, so its histograms can be fine-grained.
    pub fn step(&mut self, t: &Traffic) -> Scrape {
        let h = self.ttft.get_or_insert_with(|| Hist::new(fine_ttft()));
        for ms in &t.ttft_ms {
            h.observe(ms / 1000.0, 1.0);
        }
        let ttft = h.cumulative();
        let i = self.itl.get_or_insert_with(|| Hist::new(fine_itl()));
        for ms in &t.itl_ms {
            i.observe(ms / 1000.0, 1.0);
        }
        self.gen_total += t.output_tokens as f64;
        self.prompt_total += t.prompt_tokens as f64;
        self.done += t.finished as f64;
        Scrape {
            prompt_tokens: Some(self.prompt_total),
            generation_tokens: Some(self.gen_total),
            requests_done: Some(self.done),
            running: t.active as f64,
            ttft,
            itl: i.cumulative(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::metrics::{Ring, Scrape};

    fn llama() -> SimModel {
        SimModel { served: "llama-3.3-70b".into(), weights: 141.1e9, tp: 1, pp: 3, kv_tokens: 330_000.0, max_seqs: 16.0, busyness: 2.0 }
    }

    #[test]
    fn llama_70b_over_three_nodes_decodes_at_the_fit_ceiling() {
        let m = llama();
        let one = m.per_stream_tps(1.0);
        assert!((1.5..2.0).contains(&one), "pipeline parallel: no single-stream speedup ({one:.2} tok/s)");
        // Tensor parallel over four nodes is ~4× faster for one stream.
        let tp4 = SimModel { tp: 4, pp: 1, ..m.clone() };
        assert!((tp4.per_stream_tps(1.0) / one - 4.0).abs() < 1e-9);
        // Batching: each stream slows a little, the total grows a lot.
        let eight = m.per_stream_tps(8.0) * 8.0;
        assert!(eight > 6.0 * one, "aggregate at 8 requests: {eight:.1} tok/s");
    }

    #[test]
    fn the_simulated_metrics_parse_into_believable_numbers() {
        let mut sim = VllmSim::new(llama(), 7);
        let mut ring = Ring::default();
        let mut ts = 1_000_000u64;
        for _ in 0..60 {
            sim.step(3.0, &Traffic::default());
            ts += 3000;
            ring.push(Scrape::from_text(&sim.prometheus(), ts));
        }
        let m = ring.metrics().unwrap();
        assert!(m.gen_tps <= 16.0 * llama().per_stream_tps(1.0), "never above the batched ceiling: {}", m.gen_tps);
        assert!((m.running - sim.running).abs() < 1e-9);
        let itl = m.itl_p50_ms.unwrap();
        assert!((450.0..750.0).contains(&itl), "one token every ~0.6 s: {itl}");
        let ttft = m.ttft_p50_ms.unwrap();
        assert!((200.0..5000.0).contains(&ttft), "{ttft}");
        assert!((0.0..=100.0).contains(&m.kv_pct));
    }

    #[test]
    fn chat_traffic_shows_up_and_slows_nothing_impossibly() {
        let mut sim = VllmSim::new(SimModel { busyness: 0.0, ..llama() }, 1);
        sim.step(3.0, &Traffic::default());
        assert_eq!(sim.running, 0.0, "a quiet model");
        let before = Scrape::from_text(&sim.prometheus(), 0).generation_tokens.unwrap();
        let chat = Traffic { active: 1, prompt_tokens: 120, output_tokens: 5, finished: 0, ttft_ms: vec![900.0], itl_ms: vec![] };
        sim.step(3.0, &chat);
        assert_eq!(sim.running, 1.0);
        let after = Scrape::from_text(&sim.prometheus(), 0).generation_tokens.unwrap();
        assert_eq!(after - before, 5.0, "exactly what Chat produced");
        let s = sim.speed();
        assert!(s.tps < 2.0 && s.ttft_ms > 100.0, "{s:?}");
        let line = stats_line(1_800_000_000, &EngineStats { running: sim.running, gen_tps: 1.6, ..Default::default() });
        assert!(line.contains("Running: 1 reqs") && line.starts_with("INFO 01-15 08:00:00 [loggers.py:123]"), "{line}");
    }

    /// p50 and p95 TTFT and p50 ITL (ms) over everything the server has served (a slow 70B
    /// finishes about one request a minute, so a minute's window is a handful of samples), plus
    /// the last round's metrics as Live reads them.
    fn latency(mut step: impl FnMut() -> Scrape, rounds: u64) -> (f64, f64, f64, crate::models::metrics::EndpointMetrics) {
        use crate::models::metrics::quantile;
        let mut ring = Ring::default();
        let mut last = Scrape::default();
        for i in 0..rounds {
            last = Scrape { ts_ms: i * 3000, ..step() };
            ring.push(last.clone());
        }
        let ms = |q: f64, b: &[(f64, f64)]| quantile(q, b).unwrap() * 1000.0;
        (ms(0.5, &last.ttft), ms(0.95, &last.ttft), ms(0.5, &last.itl), ring.metrics().unwrap().clone())
    }

    #[test]
    fn each_engine_has_its_own_latency_and_it_reads_as_one_number() {
        let mut sim = VllmSim::new(llama(), 7);
        let (p50, p95, itl, m) = latency(|| {
            sim.step(3.0, &Traffic::default());
            Scrape::from_text(&sim.prometheus(), 0)
        }, 200);
        assert!((340.0..=520.0).contains(&p50), "Llama 70B PP3 TTFT p50 ≈ 420 ms: {p50}");
        assert!((750.0..=1300.0).contains(&p95), "p95 ≈ 950 ms: {p95}");
        assert!((480.0..=700.0).contains(&itl), "one token per pipeline pass over 141 GB at 273 GB/s: {itl}");
        assert!(m.ttft_p50_range.is_none() && m.itl_p50_range.is_none(), "10 ms buckets: precise numbers");

        let mut o = OllamaSim::default();
        let (p50, p95, itl, m) = latency(|| o.step(3.0, &Traffic::default()), 400);
        assert!((110.0..=180.0).contains(&p50), "gpt-oss:20b TTFT p50 ≈ 140 ms: {p50}");
        assert!((240.0..=420.0).contains(&p95), "p95 ≈ 310 ms: {p95}");
        assert!((17.0..=24.0).contains(&itl), "≈ 20 ms between tokens: {itl}");
        assert!(!m.kv_known, "Ollama has no KV gauge");

        // endpoint-slow: the queue builds and TTFT p95 climbs to about 3 s (over what it served since).
        let mut slow = VllmSim::new(llama(), 7);
        slow.slow = true;
        let before = Scrape::from_text(&slow.prometheus(), 0).ttft;
        let (_, _, _, m) = latency(|| {
            slow.step(3.0, &Traffic::default());
            Scrape::from_text(&slow.prometheus(), 0)
        }, 400);
        let after = Scrape::from_text(&slow.prometheus(), 0).ttft;
        let since: Vec<(f64, f64)> = after.iter().zip(&before).map(|(a, b)| (a.0, a.1 - b.1)).collect();
        let p95 = crate::models::metrics::quantile(0.95, &since).unwrap() * 1000.0;
        assert!((2_500.0..=4_500.0).contains(&p95), "slow TTFT p95 ≈ 3 s: {p95}");
        assert!(m.waiting >= 2.0, "a queue: {}", m.waiting);

        // Down: nothing runs or waits, and the counters stand still.
        let before = Scrape::from_text(&slow.prometheus(), 0).generation_tokens;
        slow.idle();
        let after = Scrape::from_text(&slow.prometheus(), 0);
        assert_eq!((after.running, after.waiting, after.generation_tokens), (0.0, 0.0, before));
    }

    #[test]
    fn ollama_and_observed_counters_feed_the_same_ring() {
        let mut o = OllamaSim::default();
        let mut ring = Ring::default();
        for i in 0..40u64 {
            ring.push(Scrape { ts_ms: i * 3000, ..o.step(3.0, &Traffic::default()) });
        }
        assert!(ring.metrics().unwrap().gen_tps < 80.0);
        let mut seen = Observed::default();
        let t = Traffic { active: 1, output_tokens: 90, ttft_ms: vec![300.0], ..Default::default() };
        let s = seen.step(&t);
        assert_eq!((s.running, s.generation_tokens), (1.0, Some(90.0)));
    }
}
