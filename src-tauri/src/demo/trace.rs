//! Each simulated node's own "voice": the shape of its GPU, CPU and fabric traces as a
//! deterministic function of time, so the live simulator and the history synthesized for
//! Demo (history::backfill) are the same curve and the join at "now" doesn't jump.
//!
//! The three pipeline stages follow one shared load (they serve the same requests), each a
//! few hundred milliseconds behind the one before, with an amplitude, floor and noise of its
//! own; the Ollama node has chat sessions of its own. No two nodes trace the same line.

use crate::health::maintenance::local_offset_s;

/// One node's character.
#[derive(Clone, Copy, Debug)]
struct Voice {
    seed: u64,
    /// How far behind the shared load this stage runs, ms (pipeline stages: 300–700 ms apart).
    lag_ms: u64,
    /// The share of the shared load it shows.
    amp: f64,
    /// Utilization it never drops below while serving.
    floor: f64,
    /// Its own short-term noise, ± this much utilization.
    jitter: f64,
    /// How fast its own bursts come and go, seconds.
    burst_s: f64,
    ollama: bool,
}

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn voice(node: &str) -> Voice {
    let seed = hash(node);
    match node {
        "cell1" => Voice { seed, lag_ms: 0, amp: 0.86, floor: 0.06, jitter: 0.09, burst_s: 7.0, ollama: false },
        "cell2" => Voice { seed, lag_ms: 420, amp: 0.78, floor: 0.10, jitter: 0.12, burst_s: 5.0, ollama: false },
        "cell3" => Voice { seed, lag_ms: 980, amp: 0.92, floor: 0.04, jitter: 0.10, burst_s: 9.0, ollama: false },
        "gx10-0000" => Voice { seed, lag_ms: 0, amp: 0.90, floor: 0.0, jitter: 0.07, burst_s: 4.0, ollama: true },
        // Any other node: a voice from its name, a pipeline stage.
        _ => Voice {
            seed,
            lag_ms: 300 + seed % 400,
            amp: 0.75 + (seed >> 8) as f64 % 20.0 / 100.0,
            floor: 0.03 + (seed >> 16) as f64 % 8.0 / 100.0,
            jitter: 0.08 + (seed >> 24) as f64 % 5.0 / 100.0,
            burst_s: 4.0 + (seed >> 32) as f64 % 6.0,
            ollama: false,
        },
    }
}

/// Deterministic noise in -1..1 for (seed, step, channel).
pub fn noise(seed: u64, t: i64, ch: u64) -> f64 {
    let mut x = seed ^ (t as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ ch.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 31;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// Noise that wanders smoothly: a new value every `period` seconds, eased in between.
fn drift(seed: u64, t_s: f64, period: f64, ch: u64) -> f64 {
    let k = (t_s / period).floor();
    let f = t_s / period - k;
    let (a, b) = (noise(seed, k as i64, ch), noise(seed, k as i64 + 1, ch));
    a + (b - a) * f * f * (3.0 - 2.0 * f)
}

/// Demand over a day, 0..1: low overnight, rising through the morning, peaks mid-afternoon and evening.
fn day(local_hour: f64) -> f64 {
    let bump = |center: f64, width: f64| (-((local_hour - center) / width).powi(2)).exp();
    (0.25 + 0.5 * bump(14.5, 3.0) + 0.4 * bump(20.5, 1.8) + 0.2 * bump(10.0, 1.5)).min(1.0)
}

/// What a day's demand is measured from: this Mac's time zone, or in tests UTC, so a fixed
/// time reads the same line wherever the test runs.
fn day_offset_s() -> i64 {
    if cfg!(test) {
        0
    } else {
        local_offset_s()
    }
}

/// The load every pipeline stage serves at `ts_ms`, 0..1: the time of day, slow waves of
/// traffic and the odd lull between batches.
pub fn load_at(ts_ms: u64) -> f64 {
    let t = ts_ms as f64 / 1000.0;
    let hour = ((t as i64 + day_offset_s()).rem_euclid(86_400)) as f64 / 3600.0;
    let lull = drift(0x11a1, t, 90.0, 1) > 0.82;
    if lull {
        return 0.02;
    }
    let waves = 0.55 + 0.25 * drift(0x11a1, t, 37.0, 2) + 0.12 * drift(0x11a1, t, 11.0, 3);
    (waves * (0.55 + 0.45 * day(hour))).clamp(0.0, 1.0)
}

/// The Ollama node's own load: chat sessions of a few minutes with requests in bursts.
fn ollama_at(v: &Voice, t: f64) -> f64 {
    // Sessions: about one minute in three is in a session, in stretches of several minutes.
    let in_session = drift(v.seed, t, 240.0, 20) > 0.15;
    if !in_session {
        return 0.0;
    }
    // Within a session, a reply is generated now and then.
    let busy = drift(v.seed, t, 12.0, 21) > -0.1;
    if !busy {
        return 0.0;
    }
    (0.62 + 0.3 * drift(v.seed, t, 3.0, 22)).clamp(0.0, 1.0)
}

/// A stage's utilization, 0..1, given the load it serves.
fn shaped(v: &Voice, load: f64, t: f64) -> f64 {
    if load <= 0.0 {
        return 0.0;
    }
    let own = v.jitter * (0.6 * noise(v.seed, t as i64, 4) + 0.4 * drift(v.seed, t, v.burst_s, 5));
    (v.floor + v.amp * load + own).clamp(0.0, 1.0)
}

/// A node's GPU utilization, 0..1, at `ts_ms` while it serves the everyday world.
pub fn util_at(node: &str, ts_ms: u64) -> f64 {
    let v = voice(node);
    let t = ts_ms as f64 / 1000.0;
    if v.ollama {
        return shaped(&v, ollama_at(&v, t), t);
    }
    shaped(&v, load_at(ts_ms.saturating_sub(v.lag_ms)), t)
}

/// What the engine reports for a stage (Models' request simulator drives every stage with one
/// number), given this node's voice: the engine's figure, a little more or less of it, and the
/// node's own noise around it.
pub fn voiced(node: &str, engine_util: f64, ts_ms: u64) -> f64 {
    let v = voice(node);
    let t = ts_ms as f64 / 1000.0;
    let own = v.jitter * (0.6 * noise(v.seed, t as i64, 4) + 0.4 * drift(v.seed, t, v.burst_s, 5));
    (engine_util * (1.0 + (v.amp - 0.85) * 0.5) + own).clamp(0.0, 1.0)
}

/// A long prompt's prefill crossing the pipeline at `ts_ms`, in bits/s (0 between them): a
/// second or two at 0.5–3 Gb/s, about once a minute. Every stage sees the same ones.
pub fn prefill_at(ts_ms: u64) -> f64 {
    const SEED: u64 = 0x5eed_b0b5;
    let s = (ts_ms / 1000) as i64;
    for k in [s, s - 1] {
        // One starts in second k about one second in seventy, and half of them last two seconds.
        if noise(SEED, k, 40) > 0.97 && (k == s || noise(SEED, k, 41) > 0.0) {
            return 0.5e9 * 6f64.powf(noise(SEED, k, 42) * 0.5 + 0.5);
        }
    }
    0.0
}

/// How a node splits its NCCL traffic between its two halves, and how busy Ray's TCP is on
/// each half: (half 1's share of the RDMA bytes, a scale for half 1's TCP, one for half 2's).
pub fn fabric_mix(node: &str) -> (f64, f64, f64) {
    let v = voice(node);
    let split = 0.5 + noise(v.seed, 7, 30) * 0.12;
    let tcp1 = 1.0 + noise(v.seed, 7, 31) * 0.35;
    let tcp2 = 1.0 + noise(v.seed, 7, 32) * 0.6;
    (split, tcp1, tcp2)
}

/// A node's idle GPU watts and °C: the real machine's for the four known ones.
fn idle_of(node: &str) -> (f64, f64) {
    super::world::REAL_NODES.iter().find(|r| r.id == node).map(|r| (r.gpu_w as f64, r.gpu_c as f64)).unwrap_or((4.6, 45.0))
}

/// A GPU at rest at `ts_ms` (P8, 208 MHz): its watts, the real machine's level a tenth of a watt
/// either way and never outside GB10's 3.7–5.4 W, and its °C, wandering half a degree. The live
/// simulator reads it, and so can history drawn for a node with nothing running.
pub fn idle_gpu_at(node: &str, ts_ms: u64) -> (f32, f32) {
    let v = voice(node);
    let t = ts_ms as f64 / 1000.0;
    let (w, c) = idle_of(node);
    ((w + 0.12 * noise(v.seed, t as i64, 16)).clamp(3.7, 5.4) as f32, (c + 0.6 * drift(v.seed, t, 41.0, 17)) as f32)
}

/// A node's reading of `metric` at `ts_ms` in the everyday world: gpuUtil (%), gpuPowerW,
/// gpuTempC, cpuPct, fabricRxBps, fabricTxBps, lanRxBps. None for a metric it doesn't have.
pub fn value_at(node: &str, metric: &str, ts_ms: u64) -> Option<f32> {
    let v = voice(node);
    let t = ts_ms as f64 / 1000.0;
    let u = util_at(node, ts_ms);
    // The die warms and cools over about a minute: the utilization a little while back.
    let warm = (0..6).map(|k| util_at(node, ts_ms.saturating_sub(k * 10_000))).sum::<f64>() / 6.0;
    let value = match metric {
        "gpuUtil" => u * 100.0,
        // At rest each GB10 draws its own 3.7–5.4 W and sits at its own 43–49 °C (world::REAL_NODES).
        "gpuPowerW" => {
            let idle = idle_of(node).0;
            idle * (1.0 + 0.08 * noise(v.seed, t as i64, 8)) + 82.0 * u + 1.2 * u * noise(v.seed, t as i64, 15)
        }
        "gpuTempC" => idle_of(node).1 - 1.0 + 36.0 * warm + 0.6 * drift(v.seed, t, 20.0, 9),
        "cpuPct" => (2.5 + 9.0 * u + 1.2 * noise(v.seed, t as i64, 10)).max(0.3),
        "fabricRxBps" | "fabricTxBps" if v.ollama => 4e3 + 2e4 * (noise(v.seed, t as i64, 11) * 0.5 + 0.5),
        "fabricRxBps" => (20e6 + 60e6 * u) * (1.0 + 0.15 * noise(v.seed, t as i64, 12)),
        "fabricTxBps" => (20e6 + 60e6 * u) * (0.94 + 0.15 * noise(v.seed, t as i64, 13)),
        "lanRxBps" => (0.3e6 + 2.2e6 * u) * (1.0 + 0.3 * noise(v.seed, t as i64, 14)),
        _ => return None,
    };
    Some(value as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corr(a: &[f64], b: &[f64]) -> f64 {
        let n = a.len() as f64;
        let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
        let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
        let (va, vb): (f64, f64) = (a.iter().map(|x| (x - ma).powi(2)).sum(), b.iter().map(|y| (y - mb).powi(2)).sum());
        cov / (va.sqrt() * vb.sqrt())
    }

    #[test]
    fn no_two_nodes_trace_the_same_line() {
        let nodes = ["cell1", "cell2", "cell3", "gx10-0000"];
        // Several 15-minute stretches across a day.
        for start in [1_800_000_000_000u64, 1_800_000_000_000 + 4 * 3_600_000, 1_800_000_000_000 + 13 * 3_600_000] {
            let series: Vec<Vec<f64>> = nodes.iter().map(|n| (0..900).map(|i| value_at(n, "gpuUtil", start + i * 1000).unwrap() as f64).collect()).collect();
            for i in 0..nodes.len() {
                for j in i + 1..nodes.len() {
                    let c = corr(&series[i], &series[j]);
                    assert!(c.is_nan() || c < 0.95, "{} and {} correlate {c:.3}", nodes[i], nodes[j]);
                }
            }
            // The pipeline stages still move together: they serve the same requests.
            let c = corr(&series[0], &series[1]);
            assert!(c > 0.4, "cell1 and cell2 serve one pipeline: {c:.3}");
        }
    }

    #[test]
    fn the_same_time_gives_the_same_value() {
        for m in ["gpuUtil", "gpuPowerW", "gpuTempC", "cpuPct", "fabricRxBps", "fabricTxBps", "lanRxBps"] {
            assert_eq!(value_at("cell2", m, 1_800_000_123_000), value_at("cell2", m, 1_800_000_123_000), "{m}");
        }
        assert_eq!(value_at("cell2", "nope", 1), None);
        let u = value_at("gx10-0000", "gpuUtil", 1_800_000_000_000).unwrap();
        assert!((0.0..=100.0).contains(&u));
    }
}
