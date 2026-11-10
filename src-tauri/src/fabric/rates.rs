//! Per-interface, per-counter RoCE and PHY rates, worked out once per snapshot from the
//! fabric monitor's samples. The Fabric verdict and the health engine's `roce-iface` rule
//! both read this table, so the Fabric window and the alert list can't disagree about
//! what is moving.
//!
//! Two windows: the last 10 s (what is happening now) and the last 60 s (whether it has
//! lasted). A rate is None, never 0, when the counter isn't reported or there isn't enough
//! history yet; an interface whose node isn't reporting has no counters at all.

use std::collections::{BTreeMap, VecDeque};

use serde::Serialize;

use crate::model::RoceCounters;

/// The "now" window.
pub const SHORT_MS: u64 = 10_000;
/// The "has it lasted" window.
pub const LONG_MS: u64 = 60_000;
/// Samples within this much of a window's start still count as its start (one-second ticks jitter).
const SLACK_MS: u64 = 500;
/// The shortest span a short rate is taken over.
const MIN_SPAN_MS: u64 = 2_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterRate {
    /// The cumulative value now.
    pub total: Option<u64>,
    /// Per second over the last 10 s.
    pub r10: Option<f64>,
    /// Per second over the last full minute (None until a minute of samples is held).
    pub r60: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IfaceRates {
    pub node: String,
    pub node_name: String,
    pub iface: String,
    /// 0 for half 1 (192.168.100.x), 1 for half 2.
    pub half: u8,
    pub up: bool,
    /// "crs804/qsfp56-dd-1-2", when the interface is placed on a switch port.
    pub switch_port: Option<String>,
    /// Empty when the node isn't reporting or sends no RoCE counters: no data, not zero.
    pub counters: BTreeMap<String, CounterRate>,
    /// How long pause frames have run above the storm rate, tick by tick (seconds).
    pub pause_storm_s: Option<u64>,
}

impl IfaceRates {
    pub fn r10(&self, key: &str) -> Option<f64> {
        self.counters.get(key).and_then(|c| c.r10)
    }

    pub fn r60(&self, key: &str) -> Option<f64> {
        self.counters.get(key).and_then(|c| c.r60)
    }

    /// "cell2 half 2".
    pub fn label(&self) -> String {
        format!("{} half {}", self.node_name, self.half + 1)
    }
}

/// Every cabled fabric interface's rates for one snapshot, keyed "node/iface".
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    pub ts_ms: u64,
    pub ifaces: BTreeMap<String, IfaceRates>,
}

/// One reading of an interface's counters.
pub struct Sample {
    pub ts: u64,
    pub c: RoceCounters,
}

/// The first sample at least `window` back from `now` (within the slack), if the span to the
/// latest one is at least `min_span`.
fn base(samples: &VecDeque<Sample>, now: u64, window: u64, min_span: u64) -> Option<&Sample> {
    let last = samples.back()?;
    samples.iter().find(|s| now.saturating_sub(s.ts) <= window + SLACK_MS).filter(|s| last.ts.saturating_sub(s.ts) >= min_span)
}

fn per_s(from: &Sample, to: &Sample, key: &str) -> Option<f64> {
    let get = |c: &RoceCounters| super::readings(c).into_iter().find(|r| r.0 == key).and_then(|r| r.3);
    let (a, b) = (get(&from.c)?, get(&to.c)?);
    let dt = to.ts.saturating_sub(from.ts) as f64 / 1000.0;
    (dt > 0.0).then(|| b.saturating_sub(a) as f64 / dt)
}

/// Every counter's total and its 10 s and 60 s rates, from an interface's samples.
pub fn counters(samples: &VecDeque<Sample>, now: u64) -> BTreeMap<String, CounterRate> {
    let Some(last) = samples.back() else { return BTreeMap::new() };
    let short = base(samples, now, SHORT_MS, MIN_SPAN_MS);
    // A full minute: the oldest sample has to reach back (nearly) the whole window.
    let long = base(samples, now, LONG_MS, LONG_MS - 5_000);
    super::readings(&last.c)
        .into_iter()
        .map(|(key, _, _, total)| {
            let rate = |b: Option<&Sample>| total.and(b).and_then(|b| per_s(b, last, &key));
            let r = CounterRate { total, r10: rate(short), r60: rate(long) };
            (key, r)
        })
        .collect()
}

/// "162/s", "4.1/s", "0.004/s", "0/s": real precision, never "0.0/s" for something that moved.
pub fn fmt_per_s(v: f64) -> String {
    if v <= 0.0 {
        return "0/s".into();
    }
    if v >= 10_000.0 {
        return format!("{:.0}k/s", v / 1_000.0);
    }
    if v >= 1_000.0 {
        return format!("{:.1}k/s", v / 1_000.0);
    }
    if v >= 10.0 {
        return format!("{v:.0}/s");
    }
    if v >= 1.0 {
        return format!("{v:.1}/s");
    }
    // Two significant digits below 1: 0.42/s, 0.041/s, 0.004/s.
    let digits = (-v.log10().floor()) as usize + 1;
    let s = format!("{v:.digits$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}/s")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(points: &[(u64, u64)]) -> VecDeque<Sample> {
        points.iter().map(|&(ts, oos)| Sample { ts, c: RoceCounters { out_of_sequence: Some(oos), ..Default::default() } }).collect()
    }

    #[test]
    fn short_and_long_rates_and_no_data() {
        // 61 one-second samples, +5 a second.
        let s = samples(&(0..=60).map(|i| (i * 1000, i * 5)).collect::<Vec<_>>());
        let c = counters(&s, 60_000);
        let oos = &c["outOfSequence"];
        assert_eq!((oos.total, oos.r10, oos.r60), (Some(300), Some(5.0), Some(5.0)));
        // A counter the driver doesn't have stays None.
        assert_eq!(c["rxCrcErrorsPhy"], CounterRate::default());
        // Under a minute of history: no long rate yet.
        let s = samples(&(0..=20).map(|i| (i * 1000, i * 5)).collect::<Vec<_>>());
        let oos = &counters(&s, 20_000)["outOfSequence"];
        assert_eq!((oos.r10, oos.r60), (Some(5.0), None));
        // Stopped 10 s ago: the short rate is 0 at once, the long one still remembers.
        let mut pts: Vec<(u64, u64)> = (0..=50).map(|i| (i * 1000, i * 5)).collect();
        pts.extend((51..=61).map(|i| (i * 1000, 250)));
        let oos = &counters(&samples(&pts), 61_000)["outOfSequence"];
        assert_eq!(oos.r10, Some(0.0));
        assert!(oos.r60.unwrap() > 3.0);
    }

    #[test]
    fn rates_print_with_real_precision() {
        assert_eq!(fmt_per_s(0.0), "0/s");
        assert_eq!(fmt_per_s(0.004), "0.004/s");
        assert_eq!(fmt_per_s(0.0412), "0.041/s");
        assert_eq!(fmt_per_s(0.42), "0.42/s");
        assert_eq!(fmt_per_s(4.12), "4.1/s");
        assert_eq!(fmt_per_s(162.4), "162/s");
        assert_eq!(fmt_per_s(50_200.0), "50k/s");
    }
}
