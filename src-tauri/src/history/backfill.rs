//! Filling history so charts open full (§2.4, C5).
//!
//! Demo: two days of plausible history, so every range has something to show the
//! moment the simulator starts. It follows a working day (quiet overnight, busy in
//! the afternoon and evening), keeps the pipeline-parallel model loaded on its
//! nodes, gives the Ollama node a few sessions, and reboots the second node once:
//! four minutes of nothing, then the model loading back from disk. The simulated
//! agents kept recording while SparkBox was closed, so *every* gap in the last 48 h
//! is filled at 10 s (not only the time before the first run), and the last 15
//! minutes at 1 s; the last ten minutes ease into the live snapshot's values so the
//! join doesn't jump. Endpoint and RoCE (`net:`) series are filled the same way.
//!
//! Live: the node agents keep 48 h at 10 s. On launch and whenever a node comes
//! back, the last hour is pulled first (`export --since now-1h`) straight into the
//! 10 s tier, so the charts that are open fill at once, then everything since
//! SparkBox's own last reading. Each pull ends with `history://filled {entities}`,
//! and the windows merge what arrived.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use super::{sample, History, Series, ENDPOINT_METRICS, N, NET_METRICS};
use crate::health::EndpointInfo;
use crate::model::{ClusterSnapshot, LinkStatus, NodeSnapshot};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const DAY_S: i64 = 86_400;
const HOUR_S: i64 = 3600;
/// How far back Demo history is filled (the agents keep 48 h at 10 s).
pub const FILL_S: i64 = 2 * DAY_S;
/// The stretch before now filled at 1 s too, for the 5 and 15 minute charts.
pub const RAW_FILL_S: i64 = 15 * 60;
/// The stretch before now that eases into the live values (the traces already agree; this
/// only smooths what the day model adds).
const JOIN_S: f32 = 60.0;
/// Prefill bursts are the cluster's, not a node's: a long prompt flows through every stage at once.
/// How long ago the second node rebooted, and how long it was gone.
pub const REBOOT_AGO_S: i64 = 9 * HOUR_S + 20 * 60;
pub const REBOOT_GAP_S: i64 = 4 * 60;
const RELOAD_S: i64 = 6 * 60;

#[derive(Clone, Copy, PartialEq)]
enum Work {
    /// A shard of the pipeline-parallel vLLM model.
    Pipeline,
    /// Ollama: loads its model for a session, unloads it 5 minutes after the last request.
    Ollama,
    Idle,
}

struct Profile {
    id: String,
    work: Work,
    head: bool,
    mem_total: f64,
    base_mem: f64,
    alloc: f64,
    power_limit: f64,
    reboots: bool,
    seed: u64,
    end: [f32; N],
}

fn profile(n: &NodeSnapshot, index: usize) -> Profile {
    let gpu_containers: Vec<_> = n.containers.iter().filter(|c| c.state == "running" && c.gpu_mem > 0).collect();
    let work = if gpu_containers.iter().any(|c| c.image.contains("vllm")) {
        Work::Pipeline
    } else if gpu_containers.iter().any(|c| c.image.contains("ollama")) || n.containers.iter().any(|c| c.image.contains("ollama")) {
        Work::Ollama
    } else if n.mem.gpu_alloc as f64 > GIB {
        Work::Pipeline
    } else {
        Work::Idle
    };
    let mem_total = if n.mem.total > 0 { n.mem.total as f64 } else { 119.7 * GIB };
    let alloc = match work {
        Work::Pipeline if n.mem.gpu_alloc > 0 => n.mem.gpu_alloc as f64,
        Work::Pipeline => 0.70 * mem_total,
        Work::Ollama => 41.5 * GIB,
        Work::Idle => 0.0,
    };
    let end = if n.status == LinkStatus::Online && !n.stale { sample(n) } else { [f32::NAN; N] };
    Profile {
        id: n.id.clone(),
        work,
        head: n.containers.iter().any(|c| c.name.contains("head")),
        mem_total,
        base_mem: ((n.mem.used as f64 - n.mem.gpu_alloc as f64).max(6.0 * GIB)).min(0.25 * mem_total),
        alloc,
        power_limit: n.gpu.power_limit_w.map(f64::from).filter(|w| *w > 0.0).unwrap_or(140.0),
        reboots: index == 1,
        seed: n.id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3)),
        end,
    }
}

/// The Mac's offset from UTC in seconds, so "afternoon" means the owner's afternoon.
pub(super) fn local_offset_s() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let (sign, digits) = match text.split_at_checked(1) {
            Some(("-", d)) => (-1, d),
            Some(("+", d)) => (1, d),
            _ => return 0,
        };
        let (h, m) = (digits.get(..2).and_then(|s| s.parse::<i64>().ok()), digits.get(2..4).and_then(|s| s.parse::<i64>().ok()));
        sign * (h.unwrap_or(0) * 3600 + m.unwrap_or(0) * 60)
    })
}

/// Demand over a day, 0..1: low overnight, rising through the morning, peaks mid-afternoon and evening.
fn demand(local_hour: f64) -> f64 {
    let bump = |center: f64, width: f64| (-((local_hour - center) / width).powi(2)).exp();
    (0.08 + 0.55 * bump(14.5, 3.0) + 0.45 * bump(20.5, 1.8) + 0.2 * bump(10.0, 1.5)).min(1.0)
}

/// Whether the Ollama node has a session running (model loaded) at this local hour.
fn ollama_session(local_hour: f64) -> bool {
    [(9.6, 11.4), (15.0, 16.8), (20.2, 22.5)].iter().any(|(a, b)| local_hour >= *a && local_hour < *b)
}

/// Deterministic noise in -1..1 for (seed, t, channel).
fn noise(seed: u64, t: i64, ch: u64) -> f64 {
    let mut x = seed ^ (t as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ ch.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 31;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// One synthesized reading at unix time `t`, or None while the node is rebooting.
fn reading(p: &Profile, t: i64, now: i64, offset: i64) -> Option<[f32; N]> {
    let ago = now - t;
    let since_boot = REBOOT_AGO_S - ago;
    if p.reboots && (0..REBOOT_GAP_S).contains(&since_boot) {
        return None;
    }
    let reloading = p.reboots && (REBOOT_GAP_S..REBOOT_GAP_S + RELOAD_S).contains(&since_boot);
    let hour = ((t + offset).rem_euclid(DAY_S)) as f64 / HOUR_S as f64;
    let d = demand(hour);
    let wobble = |ch: u64, spread: f64| noise(p.seed, t, ch) * spread;
    // Slow drift: noise every 5 minutes, eased in between, so values wander instead of jumping.
    let drift = |ch: u64| {
        let (k, f) = (t.div_euclid(300), t.rem_euclid(300) as f64 / 300.0);
        let (a, b) = (noise(p.seed, k, ch), noise(p.seed, k + 1, ch));
        a + (b - a) * f * f * (3.0 - 2.0 * f)
    };

    let (alloc, util) = match p.work {
        Work::Pipeline => {
            let alloc = if reloading { p.alloc * ((since_boot - REBOOT_GAP_S) as f64 / RELOAD_S as f64).min(1.0) } else { p.alloc };
            let util = if reloading { 2.0 } else { (12.0 + 78.0 * d + 10.0 * drift(1) + wobble(2, 6.0)).clamp(0.0, 99.0) };
            (alloc, util)
        }
        Work::Ollama => {
            // Its models stay in memory (Demo's Ollama keeps them loaded, as the live node shows);
            // the GPU works in chat sessions.
            let busy = ollama_session(hour) && noise(p.seed, t / 40, 3) > -0.2;
            (p.alloc, if busy { (35.0 + 50.0 * d + wobble(4, 12.0)).clamp(0.0, 97.0) } else { 0.0 })
        }
        Work::Idle => (0.0, 0.0),
    };
    // The node's own voice (demo::trace), the same function of time the live simulator reads, so
    // the history and the live readings are one line; the day model above covers the rest.
    let voice = |metric: &str| if reloading || p.work == Work::Idle { None } else { crate::demo::trace::value_at(&p.id, metric, t as u64 * 1000).map(f64::from) };
    // A node with nothing running reads what the live simulator's GB10 at P8 reads (its own
    // 3.7–5.4 W and 43–49 °C, 208 MHz) and a CPU at 0.3–1%, so the chart doesn't step at "now".
    let rest = (p.work == Work::Idle && !reloading).then(|| crate::demo::trace::idle_gpu_at(&p.id, t as u64 * 1000));
    let util = voice("gpuUtil").unwrap_or(util);
    let cpu = match rest {
        Some(_) => 0.65 + wobble(5, 0.35),
        None => voice("cpuPct").unwrap_or_else(|| (2.5 + 9.0 * d * (util / 100.0 + 0.3) + if p.head { 3.0 } else { 0.0 } + wobble(5, 1.2)).max(0.3)),
    };
    let mem_used = p.base_mem + alloc + 0.6 * GIB * (1.0 + drift(6));
    let power = match rest {
        Some((w, _)) => f64::from(w),
        None => voice("gpuPowerW").unwrap_or_else(|| 11.0 + (util / 100.0) * (p.power_limit - 11.0) * 0.78 + wobble(7, 2.0)).max(5.0),
    };
    let temp = match rest {
        Some((_, c)) => f64::from(c),
        None => voice("gpuTempC").unwrap_or_else(|| 36.0 + util * 0.36 + 2.0 * drift(8) + wobble(9, 0.6)),
    };

    // Pipeline-parallel decode moves activations: tens of Mb/s per link, with prefill bursts.
    let (mut fab_rx, mut fab_tx) = (0.0, 0.0);
    if p.work == Work::Pipeline && !reloading {
        let decode = voice("fabricRxBps").unwrap_or_else(|| (20e6 + 60e6 * d) * (1.0 + 0.3 * wobble(10, 1.0)));
        // The live simulator's prefills (demo::trace), second for second.
        let burst = crate::demo::trace::prefill_at(t as u64 * 1000);
        fab_rx = decode + burst;
        fab_tx = decode * 0.92 + burst * 0.9;
    } else if let (Some(rx), Some(tx)) = (voice("fabricRxBps"), voice("fabricTxBps")) {
        // Off the pipeline: the trickle of discovery and keepalives the live node shows.
        (fab_rx, fab_tx) = (rx, tx);
    }
    let rdma_share = 0.9;
    let lan = (0.3e6 + 25e6 * d * if p.head { 1.6 } else { 0.4 }) * (1.0 + 0.4 * wobble(13, 1.0));
    // Disk: a trickle, and now and then a burst of a few seconds, as often and as big as the live
    // simulator's (reads 15–260 MB/s, writes 8–180 MB/s).
    let u01 = |ch: u64| wobble(ch, 1.0) * 0.5 + 0.5;
    let burst = |ch: u64, p_start: f64, lo: f64, hi: f64| {
        if noise(p.seed, t / 2, ch) > 1.0 - 2.0 * p_start { lo * (hi / lo).powf(noise(p.seed, t / 2, ch + 1) * 0.5 + 0.5) } else { 0.0 }
    };
    let disk_read = if reloading { 2.6e9 + 0.4e9 * wobble(14, 1.0) } else { 0.06e6 + 0.3e6 * u01(15) + burst(19, 0.035, 15e6, 260e6) };
    let disk_write = 0.12e6 + 0.6e6 * u01(16) + burst(22, 0.05, 8e6, 180e6);
    // Page cache: what's free, up to 16 GiB, as the live node keeps it.
    let cache = (p.mem_total - mem_used - 1.2 * GIB).clamp(0.2 * GIB, 16.0 * GIB);
    // The SM clock as the live node reports it: boost while it works (never past its 2520 MHz), idle otherwise.
    let clock = if util > 2.0 {
        2418.0 - 120.0 * (wobble(18, 1.0) * 0.5 + 0.5)
    } else if rest.is_some() {
        208.0
    } else {
        405.0 + 12.0 * (wobble(18, 1.0) * 0.5 + 0.5)
    };

    Some([
        cpu as f32,
        util as f32,
        mem_used.min(p.mem_total * 0.97) as f32,
        alloc as f32,
        power as f32,
        temp as f32,
        fab_rx as f32,
        fab_tx as f32,
        (fab_rx * rdma_share) as f32,
        (fab_tx * rdma_share) as f32,
        lan.max(0.0) as f32,
        (lan * 0.35).max(0.0) as f32,
        disk_read as f32,
        disk_write as f32,
        cache.max(0.0) as f32,
        clock as f32,
    ])
}

/// Eases the last few minutes toward the live values, so the history meets the present smoothly.
fn blend(v: [f32; N], end: &[f32; N], weight: f32) -> [f32; N] {
    let mut out = v;
    for k in 0..N {
        if !end[k].is_nan() && !v[k].is_nan() {
            out[k] = v[k] * (1.0 - weight) + end[k] * weight;
        }
    }
    out
}

/// The weight of the live value at `t`: 0 until ten minutes before now, then easing to 1.
fn join_weight(now: i64, t: i64) -> f32 {
    (1.0 - (now - t) as f32 / JOIN_S).max(0.0)
}

/// Fills every gap in the last 48 hours of every node in the snapshot (series keyed
/// `node:<id>`) at 10 s, and the last 15 minutes at 1 s, where nothing was recorded.
/// Returns, per node, the stretch filled and how many points.
pub fn fill(nodes: &mut BTreeMap<String, Series>, snap: &ClusterSnapshot) -> Vec<(String, u64, u64, usize)> {
    fill_at(nodes, snap, local_offset_s())
}

pub(super) fn fill_at(nodes: &mut BTreeMap<String, Series>, snap: &ClusterSnapshot, offset: i64) -> Vec<(String, u64, u64, usize)> {
    let now = (snap.ts_ms / 1000) as i64;
    let mut out = Vec::new();
    for (index, n) in snap.nodes.iter().enumerate() {
        let p = profile(n, index);
        let key = format!("node:{}", n.id);
        let series = nodes.entry(key.clone()).or_default();
        let filled = fill_series(series, now, |t| reading(&p, t, now, offset).map(|v| blend(v, &p.end, join_weight(now, t)).to_vec()));
        if let Some(f) = filled {
            out.push((key, f.0, f.1, f.2));
        }
    }
    out
}

/// The core of every Demo fill: each 10 s bucket of the last 48 h with no data in the
/// finest tier that still holds it, then each second of the last 15 minutes the 1 s ring
/// has nothing for. `at(t)` is the reading (None: the entity was down then, left a gap).
fn fill_series(series: &mut Series, now: i64, at: impl Fn(i64) -> Option<Vec<f32>>) -> Option<(u64, u64, usize)> {
    let (mut count, mut from, mut to) = (0usize, i64::MAX, i64::MIN);
    let start = now - FILL_S;
    let mut t = start - start.rem_euclid(10);
    // The bucket now is still being recorded live.
    while t < now - now.rem_euclid(10) {
        if !series.has_data_at(t, now) {
            if let Some(v) = at(t) {
                series.push_buckets(t, &v);
                count += 1;
                from = from.min(t);
                to = to.max(t + 10);
            }
        }
        t += 10;
    }
    for t in now - RAW_FILL_S..now {
        if !series.has_raw(t) {
            if let Some(v) = at(t) {
                series.push_raw(t, &v);
                from = from.min(t);
                to = to.max(t + 1);
            }
        }
    }
    (count > 0).then(|| (from as u64 * 1000, to as u64 * 1000, count))
}

// ---- Demo endpoints and RoCE interfaces

/// What a Demo inference endpoint did over the day: a vLLM pipeline busy with the day's
/// demand, an Ollama server busy in its sessions only. Ends at its live numbers.
fn endpoint_reading(e: &EndpointInfo, seed: u64, t: i64, now: i64, offset: i64) -> Option<Vec<f32>> {
    let hour = ((t + offset).rem_euclid(DAY_S)) as f64 / HOUR_S as f64;
    let d = demand(hour);
    let ollama = e.id.contains("ollama");
    let w = |ch: u64| noise(seed, t / 10, ch);
    // Ollama keeps its models in memory and answers in the node's own chat sessions (demo::trace),
    // the same ones its GPU history shows.
    let busy = if ollama { (crate::demo::trace::util_at(&e.head_node, t as u64 * 1000) > 0.0) as u8 as f64 } else { 1.0 };
    let running = if ollama { busy * (1.0 + (w(1) > 0.6) as u8 as f64) } else { ((0.4 + 3.2 * d) * (1.0 + 0.4 * w(1))).max(0.0).round() };
    let waiting = if d > 0.75 && w(2) > 0.55 { (1.0 + 2.0 * w(3).abs()).round() } else { 0.0 };
    let gen = if running > 0.0 { running * (if ollama { 34.0 } else { 6.5 }) * (1.0 + 0.15 * w(4)) } else { 0.0 };
    let prompt = gen * (6.0 + 3.0 * w(5).abs());
    let idle = if running > 0.0 { 1.0 } else { f64::NAN };
    let ttft50 = if ollama { 120.0 } else { 380.0 } * (1.0 + 0.6 * d + 0.15 * w(6)) * idle;
    let ttft95 = ttft50 * (1.8 + 0.4 * w(7).abs());
    let itl = if ollama { 28.0 } else { 150.0 } * (1.0 + 0.3 * d + 0.08 * w(8)) * idle;
    let kv = if ollama { f64::NAN } else { (2.0 + 22.0 * d * (running / 4.0).min(1.5) + 2.0 * w(9)).clamp(0.5, 95.0) };
    // One test token a minute: one pass through the model, a little slower when it's busy.
    let canary = if t.rem_euclid(60) == 0 { (if ollama { 150.0 } else { 640.0 }) * (1.0 + 0.25 * d + 0.08 * w(10)) } else { f64::NAN };
    let vals = [gen, prompt, ttft50, ttft95, itl, running, waiting, kv, 0.0, canary];
    let wt = join_weight(now, t);
    Some(
        ENDPOINT_METRICS
            .iter()
            .zip(vals)
            .map(|(m, v)| match e.metric(m) {
                Some(live) if !v.is_nan() => (v * (1.0 - wt as f64) + live * wt as f64) as f32,
                _ => v as f32,
            })
            .collect(),
    )
}

/// RoCE congestion on a healthy fabric: a few ECN marks and CNPs a second while the
/// pipeline is busy, every error counter flat at zero.
fn net_reading(seed: u64, t: i64, offset: i64) -> Vec<f32> {
    let hour = ((t + offset).rem_euclid(DAY_S)) as f64 / HOUR_S as f64;
    let d = demand(hour);
    let ecn = (d * 6.0 * (1.0 + 0.5 * noise(seed, t / 10, 1))).max(0.0).round();
    NET_METRICS
        .iter()
        .map(|m| match *m {
            "ecn" => ecn as f32,
            "cnpSent" | "cnpHandled" => (ecn * 0.8).round() as f32,
            _ => 0.0,
        })
        .collect()
}

fn seed_of(key: &str) -> u64 {
    key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Fills the Demo endpoints' and fabric interfaces' series the first time they're seen:
/// every gap in 48 h at 10 s, the last 15 minutes at 1 s. Returns the entities filled.
pub(super) fn fill_entities(
    series: &mut BTreeMap<String, Series>,
    done: &mut HashSet<String>,
    snap: &ClusterSnapshot,
    endpoints: &[EndpointInfo],
    offset: i64,
) -> Vec<String> {
    let now = (snap.ts_ms / 1000) as i64;
    let mut out = Vec::new();
    for e in endpoints.iter().filter(|e| e.health == "ready" || e.health == "loading") {
        let key = format!("endpoint:{}", e.id);
        if !done.insert(key.clone()) {
            continue;
        }
        let seed = seed_of(&key);
        let s = series.entry(key.clone()).or_insert_with(|| Series::with_metrics(ENDPOINT_METRICS.iter().map(|m| m.to_string()).collect()));
        if fill_series(s, now, |t| endpoint_reading(e, seed, t, now, offset)).is_some() {
            out.push(key);
        }
    }
    for n in snap.nodes.iter().filter(|n| n.status == LinkStatus::Online) {
        for x in n.nets.iter().filter(|x| x.roce.is_some()) {
            let key = format!("net:{}/{}", n.id, x.iface);
            if !done.insert(key.clone()) {
                continue;
            }
            let seed = seed_of(&key);
            let s = series.entry(key.clone()).or_insert_with(|| Series::with_metrics(NET_METRICS.iter().map(|m| m.to_string()).collect()));
            if fill_series(s, now, |t| Some(net_reading(seed, t, offset))).is_some() {
                out.push(key);
            }
        }
    }
    out
}

// ---- Live: the agents' own history

/// One line of `sparkbox_agent.py export`: entity, bucket start (unix ms), step (s), averages.
#[derive(Clone, Debug, Deserialize)]
struct ExportRow {
    e: String,
    t: u64,
    v: BTreeMap<String, f64>,
}

/// Merges export lines into History, entity by entity (every kind it keeps: node, net,
/// container, endpoint, port). Returns the points taken and the entities that got any.
pub fn merge_export_text(history: &History, demo: bool, text: &str) -> (usize, Vec<String>) {
    let mut by_entity: BTreeMap<String, Vec<(u64, BTreeMap<String, f64>)>> = BTreeMap::new();
    for line in text.lines() {
        if let Ok(r) = serde_json::from_str::<ExportRow>(line) {
            by_entity.entry(r.e).or_default().push((r.t, r.v));
        }
    }
    let mut points = 0;
    let mut entities = Vec::new();
    for (e, rows) in &by_entity {
        let n = history.merge_agent(demo, e, rows);
        if n > 0 {
            points += n;
            entities.push(e.clone());
        }
    }
    (points, entities)
}

/// How far back a first pull reaches when SparkBox has nothing for a node.
const PULL_MAX_MS: u64 = 31 * 86_400_000;
const HOUR_MS: u64 = 3_600_000;

/// Pulls what a node's agent recorded. `run` runs one command on the node and returns its
/// stdout; `cli` makes the agent's own command line. The last hour comes first, so the
/// charts that are open fill at once (`first` is called with what it brought); then
/// everything since SparkBox's own last reading before this pull, up to that hour.
/// Returns every entity that got points.
pub async fn pull_agent<R, F>(history: &History, node: &str, now_ms: u64, cli: impl Fn(&str) -> String, run: R, mut first: impl FnMut(&[String])) -> Result<Vec<String>, String>
where
    R: Fn(String) -> F,
    F: std::future::Future<Output = Result<String, String>>,
{
    // Taken before the hour arrives: afterwards the newest point is a moment ago.
    let since = history.last_point_ms(false, &format!("node:{node}")).unwrap_or(0).max(now_ms.saturating_sub(PULL_MAX_MS));
    let hour_ago = now_ms.saturating_sub(HOUR_MS);
    let out = run(cli(&format!("export --since {}", since.max(hour_ago)))).await?;
    let (_, mut entities) = merge_export_text(history, false, &out);
    first(&entities);
    if since < hour_ago {
        let out = run(cli(&format!("export --since {since} --until {hour_ago}"))).await?;
        let (_, more) = merge_export_text(history, false, &out);
        let mut all: BTreeSet<String> = entities.into_iter().collect();
        all.extend(more);
        entities = all.into_iter().collect();
    }
    Ok(entities)
}

/// The payload of `history://filled`: entity keys whose history just grew.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Filled {
    pub entities: Vec<String>,
}

/// Turns History's fills into `history://filled` for the windows (set up once at launch).
pub fn announce_fills(app: &AppHandle) {
    let handle = app.clone();
    app.state::<Arc<History>>().on_filled(move |entities| {
        let _ = handle.emit("history://filled", Filled { entities });
    });
}

/// Live: pulls each node's agent history at launch and whenever the node comes back
/// online, the last hour first. Nodes without an agent answer nothing and are skipped.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(5));
        let mut pulled: HashSet<String> = HashSet::new();
        loop {
            every.tick().await;
            let Some(state) = app.try_state::<crate::AppState>() else { continue };
            if !crate::is_live(&state) {
                pulled.clear();
                continue;
            }
            let snap = state.latest.read().unwrap().clone();
            let Some(snap) = snap else { continue };
            let history = app.state::<Arc<History>>().inner().clone();
            let live = state.live.clone();
            let layout = crate::agent::Layout::system();
            for n in &snap.nodes {
                let up = n.status == LinkStatus::Online && !n.stale;
                if !up {
                    pulled.remove(&n.id);
                    continue;
                }
                // Only nodes whose agent reports in; once per time the node comes online.
                if n.agent.is_none() || !pulled.insert(n.id.clone()) {
                    continue;
                }
                let node = n.id.clone();
                let run = |cmd: String| {
                    let live = live.clone();
                    let node = node.clone();
                    async move { live.probe(&node, &cmd, Duration::from_secs(120)).await.map(|o| o.stdout) }
                };
                // Each merge announces itself (`History::on_filled` → `history://filled`).
                match pull_agent(&history, &node, crate::history::now_ms(), |a| layout.cli(a), run, |_| {}).await {
                    Ok(entities) => crate::logging::write("history", &format!("pulled {} series from {}'s agent", entities.len(), n.name)),
                    Err(e) => {
                        pulled.remove(&node);
                        crate::logging::write("history", &format!("history from {}'s agent wasn't pulled: {e}", n.name));
                    }
                }
            }
        }
    });
}
