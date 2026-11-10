//! The ring tiers, their roll-up and query maths, and the file formats.
//!
//! Base file (one per entity, `<entity, percent-encoded>.sbh`): a JSON header line,
//! then for each tier in the header's order `count` buckets, oldest first, each
//! `metrics.len()` × (min, avg, max) little-endian f32. NaN marks a bucket with
//! no readings. Version 1 files (one per node, `<id>.sbh`, before entities) are
//! read as `node:<id>` and rewritten as version 2 by the next compaction. The header's
//! `segEpoch` (left out when 0, so p1-r3 and earlier read the file as before) says which
//! segments it already holds.
//!
//! Segments (`<entity, percent-encoded>.e<epoch>.<YYYYMMDD>.sbs`, one per UTC day and
//! epoch, next to the base file): what the minute saves appended since the base was
//! written. Each record is `len` u32, CRC-32 of the payload u32, then the payload: `H`
//! and a JSON header (entity, metrics: written first in every file, and again by every
//! run that appends to it), or `B`, the save's time (unix ms, u64) and, for each bucket
//! tier in order, a u32 count of buckets, each its number (i64) and `metrics.len()` ×
//! (min, avg, max) f32, little-endian. A bucket in a later record replaces the same
//! bucket from the base or an earlier record. A compaction writes the base with the next
//! epoch, so every older segment is already in it: those are skipped when loading and
//! deleted after the next barrier.
//!
//! Each tier marks the slots that changed since the last save took them (`take_delta`), so
//! the minute save appends only the buckets with new readings.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::{now_ms, QueryResult};
use crate::util::{Batch, Flush, Syncer};

pub struct TierSpec {
    pub name: &'static str,
    pub step_s: i64,
    pub len: usize,
}

/// 1 s × 1 hour, in memory only.
pub const RAW: TierSpec = TierSpec { name: "1s", step_s: 1, len: 3600 };
/// 10 s × 24 h, 1 min × 7 days, 15 min × 30 days; saved to disk.
pub const TIERS: [TierSpec; 3] = [
    TierSpec { name: "10s", step_s: 10, len: 8640 },
    TierSpec { name: "1m", step_s: 60, len: 10_080 },
    TierSpec { name: "15m", step_s: 900, len: 2880 },
];

const FORMAT: &str = "sparkbox-history";
const VERSION: u32 = 2;

/// One ring of buckets, `width` metrics each. Storage is flat (slot × metric) and only
/// allocated on the first reading, so an entity seen for a minute costs little.
#[derive(Clone)]
pub struct Tier {
    pub name: &'static str,
    pub step_s: i64,
    len: usize,
    width: usize,
    /// Which bucket each slot holds; a slot from an earlier lap of the ring reads as empty.
    bucket: Vec<i64>,
    count: Vec<u16>,
    sum: Vec<f32>,
    min: Vec<f32>,
    max: Vec<f32>,
    /// Slots whose bucket changed since the last save took them, one bit each.
    dirty: Vec<u64>,
    /// Any bit of `dirty` is set.
    changed: bool,
}

impl Tier {
    fn new(spec: &TierSpec, width: usize) -> Tier {
        Tier { name: spec.name, step_s: spec.step_s, len: spec.len, width, bucket: Vec::new(), count: Vec::new(), sum: Vec::new(), min: Vec::new(), max: Vec::new(), dirty: Vec::new(), changed: false }
    }

    fn allocate(&mut self) {
        if self.bucket.is_empty() {
            let cells = self.len * self.width;
            self.bucket = vec![i64::MIN; self.len];
            self.count = vec![0; cells];
            self.sum = vec![0.0; cells];
            self.min = vec![f32::NAN; cells];
            self.max = vec![f32::NAN; cells];
            self.dirty = vec![0; self.len.div_ceil(64)];
        }
    }

    /// Slot `i` changed: the next save takes it.
    fn mark(&mut self, i: usize) {
        self.dirty[i / 64] |= 1 << (i % 64);
        self.changed = true;
    }

    fn ring(&self) -> i64 {
        self.len as i64
    }

    fn index(&self, bucket: i64) -> usize {
        bucket.rem_euclid(self.ring()) as usize
    }

    /// The slot holding `bucket`, if it still does.
    fn get(&self, bucket: i64) -> Option<usize> {
        let i = self.index(bucket);
        (self.bucket.get(i) == Some(&bucket)).then_some(i)
    }

    /// The slot for `bucket`, emptied if it held an older lap; None when it holds a newer
    /// bucket (a reading too old for the ring never overwrites recent data).
    fn slot_mut(&mut self, bucket: i64) -> Option<usize> {
        self.allocate();
        let i = self.index(bucket);
        if self.bucket[i] > bucket {
            return None;
        }
        if self.bucket[i] != bucket {
            self.bucket[i] = bucket;
            let cells = i * self.width..(i + 1) * self.width;
            self.count[cells.clone()].fill(0);
            self.sum[cells.clone()].fill(0.0);
            self.min[cells.clone()].fill(f32::NAN);
            self.max[cells].fill(f32::NAN);
        }
        Some(i)
    }

    fn avg(&self, slot: usize, k: usize) -> Option<f32> {
        let c = slot * self.width + k;
        (self.count[c] > 0).then(|| self.sum[c] / self.count[c] as f32)
    }

    fn any(&self, slot: usize) -> bool {
        self.count[slot * self.width..(slot + 1) * self.width].iter().any(|&c| c > 0)
    }

    /// Folds one reading of metric `k` into the bucket for `t`.
    fn put(&mut self, t: i64, k: usize, x: f32) {
        if x.is_nan() || k >= self.width {
            return;
        }
        let Some(i) = self.slot_mut(t.div_euclid(self.step_s)) else { return };
        self.mark(i);
        let c = i * self.width + k;
        if self.count[c] == 0 {
            self.min[c] = x;
            self.max[c] = x;
        } else {
            self.min[c] = self.min[c].min(x);
            self.max[c] = self.max[c].max(x);
        }
        self.count[c] = self.count[c].saturating_add(1);
        self.sum[c] += x;
    }

    /// Folds a row of readings (NaN = nothing, so an outage stays a gap) into its bucket.
    fn add(&mut self, t: i64, v: &[f32]) {
        for (k, &x) in v.iter().enumerate() {
            self.put(t, k, x);
        }
    }

    /// Puts a saved bucket back.
    fn restore(&mut self, bucket: i64, min: &[f32], avg: &[f32], max: &[f32]) {
        let Some(i) = self.slot_mut(bucket) else { return };
        self.set(i, min, avg, max);
    }

    /// Puts a segment's bucket in place of whatever the slot holds for it (a later record wins).
    fn replace(&mut self, bucket: i64, min: &[f32], avg: &[f32], max: &[f32]) {
        let Some(i) = self.slot_mut(bucket) else { return };
        let cells = i * self.width..(i + 1) * self.width;
        self.count[cells.clone()].fill(0);
        self.sum[cells.clone()].fill(0.0);
        self.min[cells.clone()].fill(f32::NAN);
        self.max[cells].fill(f32::NAN);
        self.set(i, min, avg, max);
    }

    fn set(&mut self, i: usize, min: &[f32], avg: &[f32], max: &[f32]) {
        self.mark(i);
        for k in 0..self.width {
            if avg[k].is_nan() {
                continue;
            }
            let c = i * self.width + k;
            self.count[c] = 1;
            self.sum[c] = avg[k];
            self.min[c] = if min[k].is_nan() { avg[k] } else { min[k] };
            self.max[c] = if max[k].is_nan() { avg[k] } else { max[k] };
        }
    }

    fn held(&self, now_bucket: i64) -> impl Iterator<Item = i64> + '_ {
        self.held_in(now_bucket - self.ring() + 1, now_bucket)
    }

    /// The buckets from `first` to `last` that hold data.
    fn held_in(&self, first: i64, last: i64) -> impl Iterator<Item = i64> + '_ {
        self.bucket.iter().enumerate().filter(move |(i, &b)| b >= first && b <= last && self.any(*i)).map(|(_, &b)| b)
    }

    /// The first and last bucket a file keeps at `now_bucket`: everything still within retention,
    /// and what's newer than that clock (a reading that came after the save read it, or a node
    /// agent's clock running ahead). A file's tiers are dense, so "newer" stops one lap ahead: a
    /// bucket further out is a wrong clock, and writing the gap to it would cost a lap of nothing.
    fn file_span(&self, now_bucket: i64) -> Option<(i64, i64)> {
        let held = || self.held_in(now_bucket - self.ring() + 1, now_bucket + self.ring() - 1);
        Some((held().min()?, held().max()?))
    }

    /// The oldest bucket that's still within retention and holds data.
    fn first_bucket(&self, now_bucket: i64) -> Option<i64> {
        self.held(now_bucket).min()
    }

    fn last_bucket(&self, now_bucket: i64) -> Option<i64> {
        self.held(now_bucket).max()
    }

    fn retention_s(&self) -> i64 {
        self.step_s * self.ring()
    }

    /// Forgets every bucket that starts before `t` (unix s). Returns whether any went.
    fn drop_before(&mut self, t: i64) -> bool {
        let step = self.step_s;
        let mut dropped = false;
        for b in self.bucket.iter_mut() {
            if *b != i64::MIN && *b * step < t {
                *b = i64::MIN;
                dropped = true;
            }
        }
        dropped
    }

    /// The changed slots' buckets, each as (bucket, min/avg/max per metric), and the bits taken.
    fn take_dirty(&mut self) -> (Vec<(i64, Vec<f32>)>, Vec<u64>) {
        let bits = self.take_bits();
        let mut out = Vec::new();
        for (w, word) in bits.iter().enumerate().filter(|(_, w)| **w != 0) {
            for bit in (0..64).filter(|b| word & (1 << b) != 0) {
                let i = w * 64 + bit;
                if self.bucket[i] == i64::MIN {
                    continue; // dropped since (the budget): nothing to write
                }
                let mut cells = Vec::with_capacity(self.width * 3);
                for k in 0..self.width {
                    let (lo, a, hi) = self.avg(i, k).map(|a| (self.min[i * self.width + k], a, self.max[i * self.width + k])).unwrap_or((f32::NAN, f32::NAN, f32::NAN));
                    cells.extend_from_slice(&[lo, a, hi]);
                }
                out.push((self.bucket[i], cells));
            }
        }
        (out, bits)
    }

    /// Takes `dirty` and clears it (a compaction's copy holds those buckets now).
    fn take_bits(&mut self) -> Vec<u64> {
        self.changed = false;
        let words = self.dirty.len();
        std::mem::replace(&mut self.dirty, vec![0; words])
    }

    /// Puts back bits a save took but couldn't write.
    fn put_back(&mut self, bits: &[u64]) {
        for (d, b) in self.dirty.iter_mut().zip(bits) {
            *d |= b;
        }
        self.changed = self.dirty.iter().any(|w| *w != 0);
    }
}

/// One entity's history: the 1 s ring plus the three bucket tiers, `metrics.len()` wide.
pub struct Series {
    pub metrics: Vec<String>,
    pub raw: Tier,
    pub tiers: [Tier; 3],
    last_t: Option<i64>,
    /// The base file's segment epoch: the minute saves append to segments of this epoch.
    pub(super) epoch: u32,
    /// The base file doesn't hold what this does (a version 1 file, or the budget dropped
    /// buckets): the next compaction rewrites it.
    pub(super) base_stale: bool,
    /// When its oldest segment not yet compacted was written (unix ms); None: it has none.
    pub(super) segments_since_ms: Option<u64>,
}

/// Buckets of each bucket tier: (bucket number, min/avg/max per metric).
type Buckets = [Vec<(i64, Vec<f32>)>; 3];

/// The buckets that changed in each bucket tier since the last save (`Series::take_delta`).
pub(super) struct Delta {
    tiers: Buckets,
    bits: [Vec<u64>; 3],
}

impl Delta {
    /// The changed bits it took, to put back if it can't be written.
    pub(super) fn bits(&self) -> &[Vec<u64>; 3] {
        &self.bits
    }

    /// How many buckets of each tier it holds.
    #[cfg(test)]
    pub(super) fn counts(&self) -> [usize; 3] {
        self.tiers.each_ref().map(Vec::len)
    }
}

impl Default for Series {
    fn default() -> Self {
        Series::with_metrics(super::METRICS.iter().map(|m| m.to_string()).collect())
    }
}

impl Series {
    #[cfg(test)]
    pub fn new() -> Series {
        Series::default()
    }

    pub fn with_metrics(metrics: Vec<String>) -> Series {
        let w = metrics.len();
        Series { metrics, raw: Tier::new(&RAW, w), tiers: TIERS.each_ref().map(|s| Tier::new(s, w)), last_t: None, epoch: 0, base_stale: false, segments_since_ms: None }
    }

    pub fn width(&self) -> usize {
        self.metrics.len()
    }

    pub fn index_of(&self, metric: &str) -> Option<usize> {
        self.metrics.iter().position(|m| m == metric)
    }

    /// One second of readings (NaN where a value is missing). A second reading within
    /// the same second (an action-triggered refresh) is ignored.
    pub fn push(&mut self, t: i64, v: &[f32]) {
        if self.last_t.is_some_and(|last| t <= last) {
            return;
        }
        self.last_t = Some(t);
        self.raw.add(t, v);
        for tier in &mut self.tiers {
            tier.add(t, v);
        }
    }

    /// One reading of one metric (`History::record_entity`). It shares its second with the
    /// row `push` brings, so it doesn't count as that second's reading.
    pub fn put(&mut self, t: i64, k: usize, x: f32) {
        self.raw.put(t, k, x);
        for tier in &mut self.tiers {
            tier.put(t, k, x);
        }
    }

    /// A reading that only goes into the bucket tiers (synthesized or agent history at 10 s).
    pub(super) fn push_buckets(&mut self, t: i64, v: &[f32]) {
        for tier in &mut self.tiers {
            tier.add(t, v);
        }
    }

    /// A reading that only goes into the 1 s ring (synthesized last hour).
    pub(super) fn push_raw(&mut self, t: i64, v: &[f32]) {
        self.raw.add(t, v);
        self.last_t = Some(self.last_t.map_or(t, |l| l.max(t)));
    }

    /// Whether the 10 s tier holds readings for the bucket containing `t` (unix s).
    pub(super) fn has_bucket(&self, t: i64) -> bool {
        let tier = &self.tiers[0];
        tier.get(t.div_euclid(tier.step_s)).is_some_and(|s| tier.any(s))
    }

    /// Whether the finest bucket tier that still holds time `t` (unix s) has readings for it:
    /// the 10 s tier for the last day, the 1 min tier before that.
    pub(super) fn has_data_at(&self, t: i64, now_s: i64) -> bool {
        for tier in &self.tiers {
            let b = t.div_euclid(tier.step_s);
            if b > now_s.div_euclid(tier.step_s) - tier.ring() {
                return tier.get(b).is_some_and(|s| tier.any(s));
            }
        }
        false
    }

    /// Whether the 1 s ring holds a reading for second `t`.
    pub(super) fn has_raw(&self, t: i64) -> bool {
        self.raw.get(t).is_some_and(|s| self.raw.any(s))
    }

    /// The newest time (unix s) with data in any tier.
    pub(super) fn last_recorded_s(&self, now_s: i64) -> Option<i64> {
        self.all_tiers().iter().filter_map(|t| t.last_bucket(now_s.div_euclid(t.step_s)).map(|b| b * t.step_s)).max()
    }

    /// When this entity's recording began (unix s), as precisely as the saved tiers still know
    /// it: the finest bucket tier that hasn't dropped anything to retention yet (the 10 s tier for
    /// the first day), else the coarsest's first bucket. The 1 s ring isn't saved, so after a
    /// restart it only knows the restart, and doesn't count.
    pub(super) fn started_s(&self, now_s: i64) -> Option<i64> {
        let mut coarsest = None;
        for tier in &self.tiers {
            let now_b = now_s.div_euclid(tier.step_s);
            let Some(first) = tier.first_bucket(now_b) else { continue };
            if first > now_b - tier.ring() + 1 {
                return Some(first * tier.step_s);
            }
            coarsest = Some(first * tier.step_s);
        }
        coarsest.or_else(|| self.raw.first_bucket(now_s).map(|b| b * self.raw.step_s))
    }

    /// The oldest time (unix s) with data in any tier.
    pub(super) fn oldest_s(&self, now_s: i64) -> Option<i64> {
        self.all_tiers().iter().filter_map(|t| t.first_bucket(now_s.div_euclid(t.step_s)).map(|b| b * t.step_s)).min()
    }

    pub(super) fn drop_before(&mut self, t: i64) {
        self.raw.drop_before(t);
        for tier in &mut self.tiers {
            // What's in the files has to go too: only a new base (and no segments) forgets it.
            self.base_stale |= tier.drop_before(t);
        }
    }

    fn all_tiers(&self) -> [&Tier; 4] {
        [&self.raw, &self.tiers[0], &self.tiers[1], &self.tiers[2]]
    }

    /// Buckets changed since the last save took them.
    pub(super) fn dirty(&self) -> bool {
        self.tiers.iter().any(|t| t.changed)
    }

    /// Anything the files don't hold yet: changed buckets, or a base that has to be rewritten.
    pub(super) fn changed(&self) -> bool {
        self.dirty() || self.base_stale
    }

    /// What the minute save appends for this entity: the buckets of its bucket tiers (never
    /// the 1 s ring, which isn't saved) that changed since the last save took them, else None.
    pub(super) fn take_delta(&mut self) -> Option<Delta> {
        if !self.dirty() {
            return None;
        }
        let [a, b, c] = &mut self.tiers;
        let (ta, ba) = a.take_dirty();
        let (tb, bb) = b.take_dirty();
        let (tc, bc) = c.take_dirty();
        Some(Delta { tiers: [ta, tb, tc], bits: [ba, bb, bc] })
    }

    /// A compaction's copy: the bucket tiers whole, with the next epoch. Takes the changed
    /// bits too (the copy holds those buckets); `put_back` returns them if it isn't written.
    pub(super) fn copy_for_base(&mut self) -> (Series, [Vec<u64>; 3]) {
        let [a, b, c] = &mut self.tiers;
        let bits = [a.take_bits(), b.take_bits(), c.take_bits()];
        let w = self.width();
        let copy = Series { metrics: self.metrics.clone(), raw: Tier::new(&RAW, w), tiers: self.tiers.clone(), last_t: None, epoch: self.epoch + 1, base_stale: false, segments_since_ms: None };
        (copy, bits)
    }

    /// Bits a save took but couldn't write go back, so the next save takes them again.
    pub(super) fn put_back(&mut self, bits: &[Vec<u64>; 3]) {
        for (t, b) in self.tiers.iter_mut().zip(bits) {
            t.put_back(b);
        }
    }

    /// Its files now hold what it holds.
    pub(super) fn saved(&mut self) {
        for t in &mut self.tiers {
            t.take_bits();
        }
    }

    /// Picks the tier: the finest one whose retention reaches back to `from` and whose
    /// recording started by then. When history is younger than the range, the one that
    /// started earliest, preferring a finer tier when they started within a bucket of each
    /// other (so a just-restarted 1 s ring doesn't hide the hour the 10 s tier remembers).
    /// An outage inside the range doesn't matter: it stays a gap in the fine tier instead
    /// of being averaged over by a coarse one.
    fn choose(&self, from_s: i64, now_s: i64) -> &Tier {
        let tiers = self.all_tiers();
        // A minute of slack, so "the last hour" asked a moment late still comes from the 1 s ring.
        let reach: Vec<&Tier> = tiers.iter().copied().filter(|t| t.retention_s() + 60 >= now_s - from_s).collect();
        let candidates = if reach.is_empty() { vec![tiers[3]] } else { reach };
        let starts: Vec<Option<i64>> = candidates.iter().map(|t| t.first_bucket(now_s.div_euclid(t.step_s)).map(|b| b * t.step_s)).collect();
        // A coarse bucket's readings can be anywhere in it, so a tier coarser than a minute
        // only covers `from` when its first bucket had ended by then: ten minutes in the
        // 15 min tier don't cover a 15-minute chart (the 10 s tier answers it, finely).
        let covers = |t: &Tier, s: i64| s <= from_s + t.step_s.min(60) && (t.step_s <= 60 || s + t.step_s <= from_s + 60);
        for (t, start) in candidates.iter().zip(&starts) {
            if start.is_some_and(|s| covers(t, s)) {
                return t;
            }
        }
        let mut chosen: Option<(&Tier, i64)> = None;
        for (t, start) in candidates.iter().zip(&starts).rev() {
            let Some(s) = *start else { continue };
            match chosen {
                Some((c, cs)) if s > cs + c.step_s => {}
                _ => chosen = Some((t, s)),
            }
        }
        chosen.map(|(t, _)| t).unwrap_or(candidates[0])
    }

    pub(super) fn query(&self, names: &[String], idx: &[usize], from_ms: u64, to_ms: u64, max_points: usize, now_ms: u64) -> QueryResult {
        let (from_s, to_s, now_s) = ((from_ms / 1000) as i64, ((to_ms.saturating_sub(1)) / 1000) as i64, (now_ms / 1000) as i64);
        let tier = self.choose(from_s, now_s);
        let step = tier.step_s;
        let (from_b, to_b) = (from_s.div_euclid(step), to_s.div_euclid(step));
        // What's kept of the range, whatever it asks for: a group's buckets past either end cost nothing.
        let oldest = now_s.div_euclid(step) - tier.ring() + 1;
        let newest = tier.bucket.iter().copied().max().unwrap_or(i64::MIN);
        // Merge whole buckets into groups aligned to absolute bucket numbers, so the same
        // range asked twice (or a moment later) comes back on the same boundaries.
        let span = ((to_ms.saturating_sub(from_ms)) as f64 / 1000.0 / step as f64).ceil().max(1.0) as usize;
        let group = span.div_ceil(max_points).max(1) as i64;
        let first_g = from_b.div_euclid(group);
        let last_g = to_b.div_euclid(group);
        let raw = tier.step_s == 1;

        let mut out = QueryResult {
            tier: tier.name.into(),
            step_ms: (step * group * 1000) as u64,
            starts_at_ms: tier.first_bucket(now_s.div_euclid(step)).map(|b| (b * step * 1000) as u64),
            ..Default::default()
        };
        let mut avg: Vec<Vec<Option<f32>>> = vec![Vec::new(); idx.len()];
        let mut mins: Vec<Vec<Option<f32>>> = vec![Vec::new(); idx.len()];
        let mut maxs: Vec<Vec<Option<f32>>> = vec![Vec::new(); idx.len()];
        for g in first_g..=last_g {
            out.ts.push((g * group * step * 1000).max(0) as u64);
            for (j, &k) in idx.iter().enumerate() {
                let (mut sum, mut n, mut lo, mut hi) = (0.0f64, 0u32, f32::INFINITY, f32::NEG_INFINITY);
                for b in (g * group).max(oldest)..=((g + 1) * group - 1).min(newest) {
                    let Some(s) = tier.get(b) else { continue };
                    if let Some(a) = tier.avg(s, k) {
                        let cell = s * tier.width + k;
                        let c = tier.count[cell] as u32;
                        sum += a as f64 * c as f64;
                        n += c;
                        lo = lo.min(tier.min[cell]);
                        hi = hi.max(tier.max[cell]);
                    }
                }
                if n > 0 {
                    avg[j].push(Some((sum / n as f64) as f32));
                    mins[j].push(Some(lo));
                    maxs[j].push(Some(hi));
                } else {
                    avg[j].push(None);
                    mins[j].push(None);
                    maxs[j].push(None);
                }
            }
        }
        for (j, name) in names.iter().enumerate() {
            out.series.insert(name.clone(), std::mem::take(&mut avg[j]));
        }
        if !raw {
            out.min = Some(names.iter().cloned().zip(mins).collect());
            out.max = Some(names.iter().cloned().zip(maxs).collect());
        }
        out
    }

    /// Every bucket of the bucket tiers that holds data and is still kept at `now_s`:
    /// (tier, bucket, min/avg/max per metric as f32 bits, so gaps compare equal), to compare a
    /// history with its files.
    #[cfg(test)]
    pub(super) fn buckets(&self, now_s: i64) -> Vec<(usize, i64, Vec<u32>)> {
        let mut out = Vec::new();
        for (n, tier) in self.tiers.iter().enumerate() {
            let mut held: Vec<i64> = tier.held(now_s.div_euclid(tier.step_s)).collect();
            held.sort();
            for b in held {
                let i = tier.get(b).unwrap();
                let cells = (0..tier.width).flat_map(|k| tier.avg(i, k).map(|a| [tier.min[i * tier.width + k], a, tier.max[i * tier.width + k]]).unwrap_or([f32::NAN; 3])).map(f32::to_bits).collect();
                out.push((n, b, cells));
            }
        }
        out
    }

    // ---- File format

    pub(super) fn encode(&self, entity: &str, now_s: i64) -> Vec<u8> {
        let (header, buckets) = self.header(entity, now_s);
        let w = self.width();
        let mut body = Vec::with_capacity(buckets.iter().map(|b| b.1).sum::<usize>() * w * 12);
        for (tier, (first, count)) in self.tiers.iter().zip(buckets) {
            for b in first..first + count as i64 {
                let s = tier.get(b);
                for k in 0..w {
                    let (lo, a, hi) = s.and_then(|s| tier.avg(s, k).map(|a| (tier.min[s * w + k], a, tier.max[s * w + k]))).unwrap_or((f32::NAN, f32::NAN, f32::NAN));
                    body.extend_from_slice(&lo.to_le_bytes());
                    body.extend_from_slice(&a.to_le_bytes());
                    body.extend_from_slice(&hi.to_le_bytes());
                }
            }
        }
        let mut bytes = serde_json::to_vec(&header).unwrap_or_default();
        bytes.push(b'\n');
        bytes.extend_from_slice(&body);
        bytes
    }

    /// `encode(..).len()` without building the body (the budget sums every entity's).
    pub(super) fn encoded_len(&self, entity: &str, now_s: i64) -> usize {
        let (header, buckets) = self.header(entity, now_s);
        let body: usize = buckets.iter().map(|b| b.1).sum::<usize>() * self.width() * 12;
        serde_json::to_vec(&header).map(|h| h.len()).unwrap_or(0) + 1 + body
    }

    /// The file's header, and the buckets each bucket tier writes: (first, how many), none for one with no data.
    fn header(&self, entity: &str, now_s: i64) -> (Header, [(i64, usize); 3]) {
        let mut header = Header {
            format: FORMAT.into(),
            version: VERSION,
            entity: Some(entity.into()),
            node: entity.strip_prefix("node:").map(String::from),
            metrics: self.metrics.clone(),
            saved_at_ms: now_ms(),
            tiers: Vec::new(),
            seg_epoch: self.epoch,
        };
        let mut buckets = [(0, 0); 3];
        for (tier, range) in self.tiers.iter().zip(buckets.iter_mut()) {
            let Some((first, last)) = tier.file_span(now_s.div_euclid(tier.step_s)) else { continue };
            let count = (last - first + 1) as usize;
            header.tiers.push(TierHeader { name: tier.name.into(), step_s: tier.step_s, first_bucket: first, count });
            *range = (first, count);
        }
        (header, buckets)
    }

    /// Reads a file into a series with `metrics` (saved metrics are mapped by name, so
    /// adding a series later keeps old files readable). Returns the entity key too.
    pub(super) fn decode(bytes: &[u8], now_s: i64, metrics_for: impl Fn(&str) -> Vec<String>) -> Result<(String, Series), String> {
        let nl = bytes.iter().position(|&b| b == b'\n').ok_or("no header line")?;
        let header: Header = serde_json::from_slice(&bytes[..nl]).map_err(|e| format!("header: {e}"))?;
        if header.format != FORMAT || !(1..=VERSION).contains(&header.version) {
            return Err(format!("unsupported format {} v{}", header.format, header.version));
        }
        let entity = match (&header.entity, &header.node) {
            (Some(e), _) => e.clone(),
            (None, Some(n)) => format!("node:{n}"),
            _ => return Err("no entity in the header".into()),
        };
        let mut series = Series::with_metrics(metrics_for(&entity));
        series.epoch = header.seg_epoch;
        let w = series.width();
        let map: Vec<Option<usize>> = header.metrics.iter().map(|m| series.index_of(m)).collect();
        let width = header.metrics.len() * 3 * 4;
        let mut at = nl + 1;
        for th in &header.tiers {
            let need = th.count * width;
            if at + need > bytes.len() {
                return Err(format!("tier {} is cut short", th.name));
            }
            if let Some(tier) = series.tiers.iter_mut().find(|t| t.step_s == th.step_s) {
                let oldest = now_s.div_euclid(tier.step_s) - tier.ring() + 1;
                for i in 0..th.count {
                    let bucket = th.first_bucket + i as i64;
                    if bucket < oldest {
                        continue;
                    }
                    let (mut lo, mut a, mut hi) = (vec![f32::NAN; w], vec![f32::NAN; w], vec![f32::NAN; w]);
                    let base = at + i * width;
                    for (m, k) in map.iter().enumerate() {
                        let Some(k) = *k else { continue };
                        let f = |o: usize| f32::from_le_bytes(bytes[base + m * 12 + o..base + m * 12 + o + 4].try_into().unwrap());
                        lo[k] = f(0);
                        a[k] = f(4);
                        hi[k] = f(8);
                    }
                    if a.iter().any(|v| !v.is_nan()) {
                        tier.restore(bucket, &lo, &a, &hi);
                    }
                }
            }
            at += need;
        }
        // As loaded it's what the file holds: the next save leaves the file alone until a reading
        // comes. A version 1 file is rewritten as version 2 by the next compaction.
        series.saved();
        series.base_stale = header.version < VERSION;
        Ok((entity, series))
    }

    /// Applies one segment record's buckets (`map`: the record's metric order to this
    /// series'), replacing what the base or an earlier record had for the same bucket.
    /// Buckets older than their tier keeps are skipped.
    fn apply(&mut self, buckets: &Buckets, map: &[Option<usize>], now_s: i64) {
        let w = self.width();
        for (tier, list) in self.tiers.iter_mut().zip(buckets) {
            let oldest = now_s.div_euclid(tier.step_s) - tier.ring() + 1;
            for (bucket, cells) in list.iter().filter(|(b, _)| *b >= oldest) {
                let (mut lo, mut a, mut hi) = (vec![f32::NAN; w], vec![f32::NAN; w], vec![f32::NAN; w]);
                for (m, k) in map.iter().enumerate() {
                    let Some(k) = *k else { continue };
                    lo[k] = cells[m * 3];
                    a[k] = cells[m * 3 + 1];
                    hi[k] = cells[m * 3 + 2];
                }
                tier.replace(*bucket, &lo, &a, &hi);
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TierHeader {
    name: String,
    step_s: i64,
    first_bucket: i64,
    count: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    format: String,
    version: u32,
    /// Version 2: "node:cell1", "endpoint:…", "container:cell1/vllm-head"…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entity: Option<String>,
    /// Version 1 (and node entities, for older readers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    metrics: Vec<String>,
    saved_at_ms: u64,
    tiers: Vec<TierHeader>,
    /// The segments this file already holds: every one of an older epoch (absent: 0).
    #[serde(default, skip_serializing_if = "is_zero")]
    seg_epoch: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// An entity key as a file name: letters, digits, dot, dash and underscore stay, every
/// other byte is %XX ("node%3Acell1.sbh").
pub fn file_name(entity: &str) -> String {
    format!("{}.sbh", stem(entity))
}

fn stem(entity: &str) -> String {
    let mut out = String::new();
    for b in entity.bytes() {
        if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The version 1 name of a node's file.
fn v1_file_name(node: &str) -> String {
    let safe: String = node.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    format!("{safe}.sbh")
}

/// The segment an entity's minute saves append to on UTC day `day` (YYYYMMDD) in `epoch`.
pub fn segment_name(entity: &str, epoch: u32, day: &str) -> String {
    format!("{}.e{epoch}.{day}.sbs", stem(entity))
}

/// (stem, epoch, day) of a segment's file name.
fn parse_segment_name(name: &str) -> Option<(&str, u32, &str)> {
    let rest = name.strip_suffix(".sbs")?;
    let (rest, day) = rest.rsplit_once('.')?;
    let (stem, epoch) = rest.rsplit_once('.')?;
    let epoch = epoch.strip_prefix('e')?.parse().ok()?;
    (day.len() == 8 && day.bytes().all(|b| b.is_ascii_digit())).then_some((stem, epoch, day))
}

/// The UTC day (YYYYMMDD) of unix time `t`.
pub fn utc_day(t: i64) -> String {
    jiff::Timestamp::from_second(t).map(|ts| ts.strftime("%Y%m%d").to_string()).unwrap_or_else(|_| "19700101".into())
}

// ---- Segment records

const SEGMENT_FORMAT: &str = "sparkbox-history-segment";
/// A record bigger than this is damage, not data (a whole month of a node is ~5 MB).
const MAX_RECORD: usize = 64 << 20;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SegmentHeader {
    format: String,
    version: u32,
    entity: String,
    metrics: Vec<String>,
}

/// CRC-32 (IEEE), the records' checksum.
pub(super) fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    !data.iter().fold(!0u32, |c, &b| TABLE[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8))
}

/// One record: length, checksum, payload.
fn record(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn header_record(entity: &str, metrics: &[String]) -> Vec<u8> {
    let h = SegmentHeader { format: SEGMENT_FORMAT.into(), version: 1, entity: entity.into(), metrics: metrics.to_vec() };
    let mut payload = vec![b'H'];
    payload.extend(serde_json::to_vec(&h).unwrap_or_default());
    record(&payload)
}

/// A delta as one `B` record, written at `at_ms`.
pub(super) fn delta_record(delta: &Delta, at_ms: u64) -> Vec<u8> {
    let cells: usize = delta.tiers.iter().flatten().map(|(_, c)| 8 + c.len() * 4).sum();
    let mut payload = Vec::with_capacity(1 + 8 + 12 + cells);
    payload.push(b'B');
    payload.extend_from_slice(&at_ms.to_le_bytes());
    for list in &delta.tiers {
        payload.extend_from_slice(&(list.len() as u32).to_le_bytes());
        for (bucket, c) in list {
            payload.extend_from_slice(&bucket.to_le_bytes());
            for v in c {
                payload.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    record(&payload)
}

/// One `B` record: when it was written, the metrics of the header before it, its buckets in
/// that metric order.
type Record = (u64, Vec<String>, Buckets);

/// What a segment file holds.
struct SegmentRead {
    entity: Option<String>,
    records: Vec<Record>,
    /// How many bytes are framed records, whole or skipped.
    good: usize,
    /// The records left out, each with its offset and why: bytes that fail their checksum, or
    /// that this build can't read.
    skipped: Vec<(usize, String)>,
    /// Why reading stopped before the end: a tail that can't be framed.
    damage: Option<String>,
}

/// Reads records until a tail it can't frame: fewer bytes than a record's length says, or a
/// length no record has. A record that frames but fails its checksum or can't be read is left
/// out, and the ones after it still count.
fn read_segment(bytes: &[u8]) -> SegmentRead {
    let mut out = SegmentRead { entity: None, records: Vec::new(), good: 0, skipped: Vec::new(), damage: None };
    let mut metrics: Option<Vec<String>> = None;
    while out.good < bytes.len() {
        let at = out.good;
        let (payload, sum) = match frame(bytes, at) {
            Ok(framed) => framed,
            Err(e) => {
                out.damage = Some(e);
                break;
            }
        };
        out.good = at + 8 + payload.len();
        let parsed = if crc32(payload) == sum { parse_record(payload, &mut out.entity, &mut metrics) } else { Err("a bad checksum".to_string()) };
        match parsed {
            Ok(rec) => out.records.extend(rec),
            Err(why) => out.skipped.push((at, why)),
        }
    }
    out
}

/// The record at `at`: its payload and checksum, or why no whole record starts there.
fn frame(bytes: &[u8], at: usize) -> Result<(&[u8], u32), String> {
    let head = bytes.get(at..at + 8).ok_or("a record cut short")?;
    let len = u32::from_le_bytes(head[..4].try_into().unwrap()) as usize;
    let sum = u32::from_le_bytes(head[4..].try_into().unwrap());
    if len == 0 || len > MAX_RECORD {
        return Err(format!("a record of {len} bytes"));
    }
    let payload = bytes.get(at + 8..at + 8 + len).ok_or("a record cut short")?;
    Ok((payload, sum))
}

/// One record's payload: a `B` record's contents, or None for a header, which updates `entity`
/// and `metrics` for the records after it.
fn parse_record(payload: &[u8], entity: &mut Option<String>, metrics: &mut Option<Vec<String>>) -> Result<Option<Record>, String> {
    match payload[0] {
        b'H' => {
            let h: SegmentHeader = serde_json::from_slice(&payload[1..]).map_err(|e| format!("header: {e}"))?;
            if h.format != SEGMENT_FORMAT || h.version != 1 {
                return Err(format!("unsupported format {} v{}", h.format, h.version));
            }
            if entity.as_ref().is_some_and(|e| *e != h.entity) {
                return Err(format!("a header for {} in a segment of {}", h.entity, entity.as_deref().unwrap_or_default()));
            }
            *entity = Some(h.entity);
            *metrics = Some(h.metrics);
            Ok(None)
        }
        b'B' => {
            let m = metrics.clone().ok_or("buckets before any header")?;
            let width = m.len() * 3;
            let mut p = 1;
            let take = |p: &mut usize, n: usize| -> Result<&[u8], String> {
                let s = payload.get(*p..*p + n).ok_or("a record shorter than its counts")?;
                *p += n;
                Ok(s)
            };
            let at_ms = u64::from_le_bytes(take(&mut p, 8)?.try_into().unwrap());
            let mut tiers: Buckets = Default::default();
            for list in &mut tiers {
                let n = u32::from_le_bytes(take(&mut p, 4)?.try_into().unwrap()) as usize;
                for _ in 0..n {
                    let bucket = i64::from_le_bytes(take(&mut p, 8)?.try_into().unwrap());
                    let cells = take(&mut p, width * 4)?.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
                    list.push((bucket, cells));
                }
            }
            if p != payload.len() {
                return Err("a record longer than its counts".into());
            }
            Ok(Some((at_ms, m, tiers)))
        }
        k => Err(format!("a record of kind {k}")),
    }
}

// ---- Writing

/// What one save wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Written {
    /// The entities whose buckets reached their files (appended, or in a new base).
    pub entities: Vec<String>,
    /// Every byte written to a file (records appended, bases written).
    pub bytes: u64,
    /// Those that couldn't be written (they're written again next time).
    pub failed: Vec<String>,
    /// The entities whose changed buckets were appended to a segment.
    pub appended: Vec<String>,
    /// The entities whose base was rewritten (a compaction).
    pub compacted: Vec<String>,
}

/// Appends records to segments: each append written and fsynced at once (no barrier; the
/// save issues one for everything). `headed` holds the paths of the segments this run has
/// written a header to (Demo's and Live's folders use the same names), and is held while
/// appending, so two appends to one file never interleave.
pub struct Appender<'a> {
    dir: &'a Path,
    syncer: &'a dyn Syncer,
    headed: &'a Mutex<HashSet<PathBuf>>,
    day: String,
    pub(super) bytes: u64,
}

impl<'a> Appender<'a> {
    pub fn new(dir: &'a Path, syncer: &'a dyn Syncer, headed: &'a Mutex<HashSet<PathBuf>>, now_s: i64) -> Result<Appender<'a>, String> {
        fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
        Ok(Appender { dir, syncer, headed, day: utc_day(now_s), bytes: 0 })
    }

    /// Appends `rec` (a `delta_record`) to the entity's segment for today in `epoch`, after a
    /// header when the file is new, empty, or not yet written by this run. A failed append is
    /// cut back off the file, so the next one starts on a whole record.
    pub(super) fn append(&mut self, entity: &str, metrics: &[String], epoch: u32, rec: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let path = self.dir.join(segment_name(entity, epoch, &self.day));
        let mut headed = self.headed.lock().unwrap();
        let mut f = fs::OpenOptions::new().append(true).create(true).open(&path).map_err(|e| format!("Couldn't open {}: {e}", path.display()))?;
        let before = f.metadata().map(|m| m.len()).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
        let mut bytes = if before == 0 || !headed.contains(&path) { header_record(entity, metrics) } else { Vec::new() };
        bytes.extend_from_slice(rec);
        let result = f.write_all(&bytes).and_then(|_| self.syncer.sync(&f, Flush::Plain));
        if let Err(e) = result {
            let _ = f.set_len(before);
            return Err(format!("Couldn't append to {}: {e}", path.display()));
        }
        headed.insert(path);
        self.bytes += bytes.len() as u64;
        Ok(())
    }
}

/// One compaction's base files: each entity encoded and written (under a temporary name) as
/// it's added, then all put in place behind one barrier (`util::Batch`).
pub struct Writer<'a> {
    dir: &'a Path,
    batch: Batch<'a>,
    now_s: i64,
    /// (file name, entity, bytes) of each file written.
    names: Vec<(String, String, u64)>,
    failed: Vec<String>,
}

impl<'a> Writer<'a> {
    pub fn new(dir: &'a Path, syncer: &'a dyn Syncer, now_s: i64) -> Result<Writer<'a>, String> {
        Ok(Writer { dir, batch: Batch::new(dir, syncer)?, now_s, names: Vec::new(), failed: Vec::new() })
    }

    pub fn add(&mut self, entity: &str, s: &Series) {
        let name = file_name(entity);
        let bytes = s.encode(entity, self.now_s);
        match self.batch.write(&name, &bytes) {
            Ok(()) => self.names.push((name, entity.to_string(), bytes.len() as u64)),
            Err(e) => {
                eprintln!("history for {entity} wasn't saved: {e}");
                self.failed.push(entity.to_string());
            }
        }
    }

    /// Whether `finish` has files to put in place (and so issues the barrier).
    pub fn has_files(&self) -> bool {
        !self.names.is_empty()
    }

    /// Puts every file in place (`full`: flushed to flash too, for Quit and SIGTERM). A node's
    /// version 1 file is removed once its version 2 file is in place.
    pub fn finish(self, full: bool) -> Written {
        let Writer { dir, batch, names, mut failed, .. } = self;
        let not_placed = batch.commit(full);
        for (name, why) in &not_placed {
            eprintln!("history file {name} wasn't saved: {why}");
        }
        let (mut entities, mut bytes) = (Vec::new(), 0);
        for (name, entity, len) in names {
            if not_placed.iter().any(|(n, _)| *n == name) {
                failed.push(entity);
                continue;
            }
            if let Some(node) = entity.strip_prefix("node:") {
                let old = dir.join(v1_file_name(node));
                if old != dir.join(&name) && old.exists() {
                    let _ = fs::remove_file(old);
                }
            }
            entities.push(entity);
            bytes += len;
        }
        Written { compacted: entities.clone(), entities, bytes, failed, ..Default::default() }
    }
}

/// The segments of `entities` in `dir` older than the epoch each maps to (they're in its
/// base now), or every segment of theirs with `None`.
pub fn segments_of(dir: &Path, entities: &HashMap<String, Option<u32>>) -> Vec<PathBuf> {
    let stems: HashMap<String, Option<u32>> = entities.iter().map(|(e, epoch)| (stem(e), *epoch)).collect();
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (stem, epoch, _) = parse_segment_name(&name)?;
            let below = stems.get(stem)?;
            below.is_none_or(|b| epoch < b).then(|| e.path())
        })
        .collect()
}

/// Writes a new base (the next epoch) for every series that changed since it was last saved,
/// so the segments before become obsolete (tests and the compatibility check write files this
/// way). One that couldn't be written stays changed.
#[cfg(test)]
pub fn save_dir(dir: &Path, series: &mut BTreeMap<String, Series>, syncer: &dyn Syncer, full: bool) -> Written {
    let now_s = (now_ms() / 1000) as i64;
    let mut writer = match Writer::new(dir, syncer, now_s) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("history wasn't saved: {e}");
            return Written { failed: series.iter().filter(|(_, s)| s.changed()).map(|(e, _)| e.clone()).collect(), ..Default::default() };
        }
    };
    let mut taken = Vec::new();
    for (entity, s) in series.iter_mut().filter(|(_, s)| s.changed()) {
        let (copy, bits) = s.copy_for_base();
        writer.add(entity, &copy);
        taken.push((entity.clone(), copy.epoch, bits));
    }
    let written = writer.finish(full);
    for (entity, epoch, bits) in taken {
        let Some(s) = series.get_mut(&entity) else { continue };
        if written.failed.contains(&entity) {
            s.put_back(&bits);
        } else {
            s.epoch = epoch;
            s.base_stale = false;
            s.segments_since_ms = None;
        }
    }
    written
}

/// What the saved history takes on disk now (every base and segment file in `dir`).
pub fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries.flatten().filter(|e| matches!(e.path().extension().and_then(|x| x.to_str()), Some("sbh" | "sbs"))).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

/// Removes the files of entities that are no longer kept (pruned for the budget): the base
/// and every segment.
pub fn remove_files(dir: &Path, entities: &[String]) {
    for e in entities {
        let _ = fs::remove_file(dir.join(file_name(e)));
    }
    for path in segments_of(dir, &entities.iter().map(|e| (e.clone(), None)).collect()) {
        let _ = fs::remove_file(path);
    }
}

/// What `load` found.
pub struct Loaded {
    pub series: BTreeMap<String, Series>,
    /// Segments already in their entity's base (left behind by a compaction): to delete after
    /// the next barrier.
    pub obsolete: Vec<PathBuf>,
}

/// Loads every file in `dir` (`load` at the present time).
#[cfg(test)]
pub fn load_dir(dir: &Path, metrics_for: impl Fn(&str) -> Vec<String>) -> BTreeMap<String, Series> {
    load(dir, metrics_for, (now_ms() / 1000) as i64).series
}

/// Loads every base file in `dir`, then every segment in epoch and day order on top of its
/// entity's base (only those of the base's epoch or later). A base file that can't be read
/// is set aside as `<name>.corrupt-<ms>` (history starts over for that entity, from its
/// segments) rather than deleted. A record that fails its checksum loses that record alone: it
/// stays in the file, with one log line. A tail that can't be framed (a save cut off midway) is
/// cut off the file, with one log line, so the next append starts on a whole record.
pub fn load(dir: &Path, metrics_for: impl Fn(&str) -> Vec<String>, now_s: i64) -> Loaded {
    let mut out = Loaded { series: BTreeMap::new(), obsolete: Vec::new() };
    let Ok(entries) = fs::read_dir(dir) else { return out };
    let mut segments = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((_, epoch, day)) = parse_segment_name(&name) {
            segments.push((epoch, day.to_string(), path));
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("sbh") {
            continue;
        }
        let parsed = fs::read(&path).map_err(|e| e.to_string()).and_then(|b| Series::decode(&b, now_s, &metrics_for));
        match parsed {
            Ok((entity, series)) => {
                out.series.insert(entity, series);
            }
            Err(e) => {
                let aside = path.with_extension(format!("sbh.corrupt-{}", now_ms()));
                eprintln!("history file {} couldn't be read ({e}); kept as {}", path.display(), aside.display());
                let _ = fs::rename(&path, &aside);
            }
        }
    }
    segments.sort();
    for (epoch, _, path) in segments {
        let Ok(bytes) = fs::read(&path) else { continue };
        let read = read_segment(&bytes);
        if let Some((at, why)) = read.skipped.first() {
            eprintln!("history segment {}: skipped {} record(s), the first at byte {at} ({why})", path.display(), read.skipped.len());
        }
        if let Some(why) = &read.damage {
            eprintln!("history segment {}: dropped {} bytes after {} ({why})", path.display(), bytes.len() - read.good, read.good);
            if let Ok(f) = fs::OpenOptions::new().write(true).open(&path) {
                let _ = f.set_len(read.good as u64);
            }
        }
        let Some(entity) = read.entity else { continue };
        let series = out.series.entry(entity.clone()).or_insert_with(|| Series::with_metrics(metrics_for(&entity)));
        if epoch < series.epoch {
            out.obsolete.push(path);
            continue;
        }
        for (at_ms, metrics, buckets) in &read.records {
            let map: Vec<Option<usize>> = metrics.iter().map(|m| series.index_of(m)).collect();
            series.apply(buckets, &map, now_s);
            series.segments_since_ms = Some(series.segments_since_ms.map_or(*at_ms, |s| s.min(*at_ms)));
        }
        if read.records.is_empty() && series.segments_since_ms.is_none() {
            // A segment with nothing in it yet still has to go, by the next compaction.
            series.segments_since_ms = Some(0);
        }
        series.saved();
    }
    out
}

/// Keeps the saved size under `budget` bytes, oldest data first: entities that stopped
/// reporting more than a day ago go first (oldest last reading first), then the oldest
/// tenth of what's left, again and again. Returns the entities dropped entirely; those whose
/// buckets were dropped are marked for a new base (`base_stale`).
pub fn enforce_budget(series: &mut BTreeMap<String, Series>, budget: usize, now_s: i64) -> Vec<String> {
    let size = |s: &BTreeMap<String, Series>| s.iter().map(|(e, x)| x.encoded_len(e, now_s)).sum::<usize>();
    let mut total = size(series);
    let mut dropped = Vec::new();
    if total <= budget {
        return dropped;
    }
    let mut idle: Vec<(i64, String)> = series
        .iter()
        .filter_map(|(e, s)| {
            let last = s.last_recorded_s(now_s).unwrap_or(i64::MIN);
            (last < now_s - 86_400).then(|| (last, e.clone()))
        })
        .collect();
    idle.sort();
    for (_, e) in idle {
        if total <= budget {
            break;
        }
        if let Some(s) = series.remove(&e) {
            total -= s.encoded_len(&e, now_s);
            dropped.push(e);
        }
    }
    let mut rounds = 0;
    while total > budget && rounds < 40 {
        rounds += 1;
        let Some(oldest) = series.values().filter_map(|s| s.oldest_s(now_s)).min() else { break };
        let cutoff = oldest + ((now_s - oldest) / 10).max(900);
        for s in series.values_mut() {
            s.drop_before(cutoff);
        }
        total = size(series);
    }
    dropped
}
