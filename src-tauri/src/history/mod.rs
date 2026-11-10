//! Metric history that survives restarts, kept the way RRD tools do: 1 s
//! readings for the last hour in memory, then 10 s, 1 min and 15 min buckets
//! (min, average and max of each) for a day, a week and a month.
//!
//! History is kept per entity: `node:<id>` (16 series), `endpoint:<id>` (serving
//! speed, latency, queue and cache), `container:<node>/<name>` (CPU, memory),
//! `port:<switch>/<port>` (traffic, errors, optics) and `net:<node>/<iface>`
//! (RoCE congestion and error rates).
//!
//! Every tier is a fixed ring indexed by bucket number, so old buckets fall
//! away by being overwritten and nothing is ever shifted. A slot remembers
//! which bucket it holds; a slot from an older lap reads as empty. Seconds an
//! entity wasn't reporting are stored as NaN and come back as null, so charts
//! show the outage as a gap.
//!
//! The bucket tiers are saved per entity, within a 500 MB budget (the oldest data goes
//! first), so a restart keeps every minute before it in the 10 s tier the 15-minute
//! charts are filled from. Each entity has a base file (its tiers whole) and per-day
//! segments (store.rs has both formats). The minute save appends to today's segment
//! only the buckets that changed since the last save, for the entities that had
//! readings: a few kilobytes each, where rewriting every base each minute came to ~34 GB
//! a day. A compaction rewrites bases from memory and retires their segments: once an
//! hour for the entities with segments, oldest first, about a twenty-fourth of them
//! each time (so each base about once a day); on Quit and SIGTERM or SIGINT (a dev
//! restart, a `kill`: `save_final`) for every entity with segments or changes, so a clean
//! quit leaves only bases; and when the files outgrow the budget. Loading reads each
//! base, then its segments in order, a later bucket replacing an earlier one; a record
//! that fails its checksum loses only that record, and a segment cut off midway (a crash)
//! only its damaged tail. A p1-r3 build reads the bases
//! and doesn't see segments: after a clean quit that's everything, after a crash the
//! minutes since each entity's last compaction.
//!
//! A save never holds up publishing: it copies one entity at a time under the lock, then
//! encodes and writes outside it. Only a switch between Demo and Live waits for it
//! (`History::on_source`). Each file written is fsynced and one barrier follows
//! for all of them; Quit and SIGTERM flush the drive's cache too.
//! Demo and Live keep separate histories, so simulated numbers never mix with the
//! real cluster's, and each knows when its recording began: one "History starts"
//! time that every chart of that source shows. Time a node agent recorded while
//! SparkBox was closed is merged in (`merge_agent`) and marked as such; every time
//! history grows that way, `history://filled {entities}` tells the windows.

pub mod backfill;
mod store;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::health::EndpointInfo;
use crate::model::{ClusterSnapshot, LinkStatus, NodeSnapshot};
pub use store::{Series, Written};

/// Every series kept per node, in storage order. Rates are bits/s (network) or bytes/s (disk).
pub const METRICS: [&str; 16] = [
    "cpu", "gpu", "memUsed", "gpuAlloc", "power", "gpuTemp", "fabricRx", "fabricTx", "rdmaRx", "rdmaTx", "lanRx", "lanTx",
    "diskRead", "diskWrite", "memCache", "smClock",
];
pub const N: usize = METRICS.len();

/// An inference endpoint: tokens/s, latency in ms, requests, KV cache %, preemptions (count).
pub const ENDPOINT_METRICS: [&str; 10] = ["genTps", "promptTps", "ttftP50", "ttftP95", "itlP50", "running", "waiting", "kvPct", "preemptions", "canaryMs"];
/// A container: CPU % and memory bytes.
pub const CONTAINER_METRICS: [&str; 2] = ["cpu", "mem"];
/// A switch port: bits/s, errors per second, the weakest lane's receive power.
pub const PORT_METRICS: [&str; 4] = ["rxBps", "txBps", "errors", "rxPowerDbm"];
/// A fabric interface's RoCE counters, as events per second.
pub const NET_METRICS: [&str; 10] = ["ecn", "cnpSent", "cnpHandled", "oos", "seqErr", "timeouts", "pauseRx", "pauseTx", "discards", "crc"];

/// How often the bucket tiers are written to disk: a restart loses at most this much of the 10 s tier.
pub const SAVE_EVERY_S: u64 = 60;
/// The second Demo node's restart in the backfilled day (the Demo agents' alert log agrees).
pub const REBOOT_AGO_MS: u64 = backfill::REBOOT_AGO_S as u64 * 1000;
pub const REBOOT_GAP_MS: u64 = backfill::REBOOT_GAP_S as u64 * 1000;
/// The most the saved history may take on disk.
pub const BUDGET_BYTES: usize = 500 * 1024 * 1024;
/// How far past now a query may ask: the rest of its range would only be empty buckets.
const QUERY_AHEAD_MS: u64 = 3_600_000;
/// The most buckets one query answers with.
const QUERY_MAX_POINTS: usize = 20_000;
/// How often a minute save also compacts: rewrites some bases and retires their segments.
pub const COMPACT_EVERY_S: i64 = 3600;
/// Each hourly compaction takes the oldest 1/COMPACT_SHARE of the entities with segments,
/// so each base is rewritten about once a day.
const COMPACT_SHARE: usize = 24;

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The series an entity key keeps, or None for a kind history doesn't know.
pub fn metrics_of(entity: &str) -> Option<&'static [&'static str]> {
    let (kind, id) = entity.split_once(':')?;
    if id.is_empty() {
        return None;
    }
    Some(match kind {
        "node" => &METRICS,
        "endpoint" => &ENDPOINT_METRICS,
        "container" => &CONTAINER_METRICS,
        "port" => &PORT_METRICS,
        "net" => &NET_METRICS,
        _ => return None,
    })
}

fn metric_names(entity: &str) -> Vec<String> {
    metrics_of(entity).unwrap_or(&METRICS).iter().map(|m| m.to_string()).collect()
}

fn sum_nets(n: &NodeSnapshot, fabric: bool, rx: bool) -> f64 {
    let role = if fabric { crate::model::NetRole::Fabric } else { crate::model::NetRole::Lan };
    n.nets.iter().filter(|x| x.role == role).map(|x| if rx { x.rx_bps } else { x.tx_bps }).sum()
}

/// RDMA traffic, counted once per RDMA device (two netdevs can share one).
fn rdma(n: &NodeSnapshot, rx: bool) -> f64 {
    let mut seen: Vec<&str> = Vec::new();
    let mut sum = 0.0;
    for x in n.nets.iter().filter(|x| x.role == crate::model::NetRole::Fabric) {
        let Some(v) = (if rx { x.rdma_rx_bps } else { x.rdma_tx_bps }) else { continue };
        let dev = x.rdma_dev.as_deref().unwrap_or(&x.iface);
        if seen.contains(&dev) {
            continue;
        }
        seen.push(dev);
        sum += v;
    }
    sum
}

/// One second of readings in METRICS order. Fabric totals are IP plus RDMA, the same
/// definition as src/lib/metrics.ts.
pub fn sample(n: &NodeSnapshot) -> [f32; N] {
    let root = n.disks.iter().find(|d| d.mount == "/").or(n.disks.first());
    let (rdma_rx, rdma_tx) = (rdma(n, true), rdma(n, false));
    // A GPU that can't be read is a gap, not an idle GPU (GpuStats.readable, never the error text).
    let gpu = |v: f32| if !n.gpu.readable { f32::NAN } else { v };
    [
        n.cpu.total_pct,
        gpu(n.gpu.util_pct),
        n.mem.used as f32,
        n.mem.gpu_alloc as f32,
        gpu(n.gpu.power_w),
        gpu(n.gpu.temp_c),
        (sum_nets(n, true, true) + rdma_rx) as f32,
        (sum_nets(n, true, false) + rdma_tx) as f32,
        rdma_rx as f32,
        rdma_tx as f32,
        sum_nets(n, false, true) as f32,
        sum_nets(n, false, false) as f32,
        root.map(|d| d.read_bps as f32).unwrap_or(0.0),
        root.map(|d| d.write_bps as f32).unwrap_or(0.0),
        n.mem.cached as f32,
        gpu(n.gpu.sm_clock_mhz as f32),
    ]
}

/// What `history_query` returns: averages per bucket, plus min/max for the bucket tiers.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    /// Bucket start times, unix ms.
    pub ts: Vec<u64>,
    pub series: BTreeMap<String, Vec<Option<f32>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<BTreeMap<String, Vec<Option<f32>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<BTreeMap<String, Vec<Option<f32>>>>,
    pub tier: String,
    pub step_ms: u64,
    /// The first time this tier has any data for the entity.
    pub starts_at_ms: Option<u64>,
    /// Stretches (unix ms) filled from a node agent's own history while SparkBox was
    /// closed: real data, so charts draw no "SparkBox was closed" band there.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agent_ranges: Vec<[u64; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Demo,
    Live,
}

impl Source {
    fn dir(self) -> &'static str {
        match self {
            Source::Demo => "demo",
            Source::Live => "live",
        }
    }
}

struct Inner {
    source: Source,
    series: BTreeMap<String, Series>,
    /// Demo history has been checked for missing time (and synthesized) since this source loaded.
    backfilled: bool,
    /// Demo endpoint and interface series already filled this session.
    entities_filled: HashSet<String>,
    /// The second of the last snapshot recorded: a longer pause (the Mac slept) is filled again in Demo.
    last_record_s: Option<i64>,
    /// The last counter readings behind per-second rates: entity → (unix ms, counters).
    counters: HashMap<String, (u64, Vec<Option<f64>>)>,
    /// Stretches filled from node agents, per entity.
    agent_ranges: BTreeMap<String, Vec<[u64; 2]>>,
    /// When this source's history begins (unix s): the oldest reading of any node. Every
    /// query answers with it, so every window says the same "History starts" time.
    started_s: Option<i64>,
    /// Segments the last load found already in their base: the next save deletes them.
    obsolete: Vec<PathBuf>,
}

/// What saves remember between them (under `History::saving`).
#[derive(Default)]
struct Saves {
    /// When the last compaction ran (unix s); None until the first save of this run.
    compacted_s: Option<i64>,
    /// Segments retired by a compaction, deleted once a later barrier has put its base
    /// before them on the drive.
    obsolete: Vec<PathBuf>,
}

/// When the first node reading was recorded (unix s), to the precision the saved tiers keep.
fn first_node_reading(series: &BTreeMap<String, Series>, now_s: i64) -> Option<i64> {
    series.iter().filter(|(k, _)| k.starts_with("node:")).filter_map(|(_, s)| s.started_s(now_s)).min()
}

/// The managed history store.
/// Told the entities whose history just grew (`history://filled`).
type FilledHook = Box<dyn Fn(Vec<String>) + Send + Sync>;

pub struct History {
    dir: Option<PathBuf>,
    budget: usize,
    inner: Mutex<Inner>,
    on_filled: Mutex<Option<FilledHook>>,
    /// One save at a time (the minute timer, Quit and SIGTERM can overlap), and no switch of
    /// source during one. Taken before `inner`, never while holding it.
    saving: Mutex<Saves>,
    /// The paths of the segments this run has written a header to; held while appending
    /// (store::Appender).
    headed: Mutex<HashSet<PathBuf>>,
    /// How files reach the disk (tests count the syncs, or hold a save up in one).
    syncer: Mutex<Arc<dyn crate::util::Syncer>>,
}

impl History {
    /// `dir` is where the entity files go (app data/history); None keeps everything in memory (tests).
    pub fn new(dir: Option<PathBuf>) -> History {
        History::with_budget(dir, BUDGET_BYTES)
    }

    pub fn with_budget(dir: Option<PathBuf>, budget: usize) -> History {
        let inner = Inner {
            source: Source::Demo,
            series: BTreeMap::new(),
            backfilled: false,
            entities_filled: HashSet::new(),
            last_record_s: None,
            counters: HashMap::new(),
            agent_ranges: BTreeMap::new(),
            started_s: None,
            obsolete: Vec::new(),
        };
        History {
            dir,
            budget,
            inner: Mutex::new(inner),
            on_filled: Mutex::new(None),
            saving: Mutex::new(Saves::default()),
            headed: Mutex::new(HashSet::new()),
            syncer: Mutex::new(Arc::new(crate::util::OsSync)),
        }
    }

    /// Syncs files through `s` instead of the disk's own calls.
    #[cfg(test)]
    pub fn set_syncer(&self, s: Arc<dyn crate::util::Syncer>) {
        *self.syncer.lock().unwrap() = s;
    }

    fn syncer(&self) -> Arc<dyn crate::util::Syncer> {
        self.syncer.lock().unwrap().clone()
    }

    /// Called (outside the lock) with the entities a Demo fill just added to; the app
    /// turns it into `history://filled`.
    pub fn on_filled(&self, f: impl Fn(Vec<String>) + Send + Sync + 'static) {
        *self.on_filled.lock().unwrap() = Some(Box::new(f));
    }

    fn filled(&self, entities: Vec<String>) {
        if entities.is_empty() {
            return;
        }
        if let Some(f) = self.on_filled.lock().unwrap().as_ref() {
            f(entities);
        }
    }

    fn source_dir(&self, source: Source) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(source.dir()))
    }

    fn load(&self, inner: &mut Inner, source: Source) {
        let Some(dir) = self.source_dir(source) else {
            inner.series = BTreeMap::new();
            return;
        };
        let loaded = store::load(&dir, metric_names, (now_ms() / 1000) as i64);
        inner.series = loaded.series;
        inner.obsolete = loaded.obsolete;
    }

    /// Appends every changed bucket of `series` to its segment, behind one barrier (switching
    /// source: the history being left is appended, not compacted).
    fn append_all(&self, dir: &Path, series: &mut BTreeMap<String, Series>, now_s: i64) {
        let syncer = self.syncer();
        let mut appender = match store::Appender::new(dir, &*syncer, &self.headed, now_s) {
            Ok(a) => a,
            Err(e) => return eprintln!("history wasn't saved: {e}"),
        };
        for (entity, s) in series.iter_mut() {
            let Some(delta) = s.take_delta() else { continue };
            if let Err(e) = appender.append(entity, &s.metrics, s.epoch, &store::delta_record(&delta, now_ms())) {
                eprintln!("history for {entity} wasn't saved: {e}");
                s.put_back(delta.bits());
            } else if s.segments_since_ms.is_none() {
                s.segments_since_ms = Some(now_ms());
            }
        }
        if appender.bytes > 0 {
            barrier(dir, &*syncer);
        }
    }

    /// `inner`, recording `demo` (true) or the Live cluster. A switch waits for a save in
    /// progress: the save writes a base with the next epoch, so a switch appending behind its
    /// back would put readings in segments that base has already retired.
    fn on_source(&self, demo: bool) -> MutexGuard<'_, Inner> {
        let mut inner = self.inner.lock().unwrap();
        let source = if demo { Source::Demo } else { Source::Live };
        if inner.source == source {
            self.select(&mut inner, demo);
            return inner;
        }
        drop(inner);
        let _saving = self.saving.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        self.select(&mut inner, demo);
        inner
    }

    /// Switching between Demo and Live saves one history and loads the other. Only `on_source`
    /// calls this.
    fn select(&self, inner: &mut Inner, demo: bool) {
        let source = if demo { Source::Demo } else { Source::Live };
        if inner.source != source {
            if let Some(dir) = self.source_dir(inner.source) {
                self.append_all(&dir, &mut inner.series, (now_ms() / 1000) as i64);
            }
            self.load(inner, source);
            inner.source = source;
            inner.backfilled = false;
            inner.entities_filled.clear();
            inner.last_record_s = None;
            inner.counters.clear();
            inner.agent_ranges.clear();
            inner.started_s = first_node_reading(&inner.series, (now_ms() / 1000) as i64);
        } else if inner.series.is_empty() && !inner.backfilled {
            self.load(inner, source);
            inner.started_s = first_node_reading(&inner.series, (now_ms() / 1000) as i64);
        }
    }

    /// Marks time `t` (unix s) as recorded for the source: history begins no later than it.
    fn note_start(inner: &mut Inner, t: i64) {
        inner.started_s = Some(inner.started_s.map_or(t, |s| s.min(t)));
    }

    /// Adds one second of readings from a published snapshot. In Demo, the first snapshot
    /// (and the first after a pause of more than a minute, when the Mac slept) also fills
    /// every gap in the last 48 hours with what the simulated agents recorded.
    pub fn record(&self, snap: &ClusterSnapshot) {
        let filled = {
            let mut inner = self.on_source(snap.demo);
            let t = (snap.ts_ms / 1000) as i64;
            let paused = inner.last_record_s.is_some_and(|last| t - last > 60);
            let filled = if snap.demo && (!inner.backfilled || paused) { backfill::fill(&mut inner.series, snap) } else { Vec::new() };
            for (entity, from, to, _) in &filled {
                let ranges = inner.agent_ranges.entry(entity.clone()).or_default();
                ranges.push([*from, *to]);
                ranges.sort();
            }
            if !filled.is_empty() {
                inner.started_s = first_node_reading(&inner.series, t);
            }
            if !snap.nodes.is_empty() {
                Self::note_start(&mut inner, t);
            }
            inner.backfilled = true;
            inner.last_record_s = Some(t);
            for n in &snap.nodes {
                let series = inner.series.entry(format!("node:{}", n.id)).or_default();
                let reporting = n.status == LinkStatus::Online && !n.stale;
                let values = if reporting { sample(n) } else { [f32::NAN; N] };
                series.push(t, &values);
            }
            filled.into_iter().map(|f| f.0).collect::<Vec<_>>()
        };
        self.filled(filled);
    }

    /// Adds one second of the other entities: containers, switch ports, fabric RoCE
    /// rates (from counter growth since the last snapshot) and inference endpoints.
    pub fn record_entities(&self, snap: &ClusterSnapshot, endpoints: &[EndpointInfo]) {
        let filled = self.record_entities_inner(snap, endpoints);
        self.filled(filled);
    }

    fn record_entities_inner(&self, snap: &ClusterSnapshot, endpoints: &[EndpointInfo]) -> Vec<String> {
        let mut inner = self.on_source(snap.demo);
        let t = (snap.ts_ms / 1000) as i64;
        // Demo endpoints and fabric interfaces open full too, the first time they're seen.
        let filled = if snap.demo {
            let Inner { series, entities_filled, .. } = &mut *inner;
            backfill::fill_entities(series, entities_filled, snap, endpoints, backfill::local_offset_s())
        } else {
            Vec::new()
        };
        let mut rows: Vec<(String, Vec<f32>)> = Vec::new();
        let mut counters: Vec<(String, Vec<Option<f64>>)> = Vec::new();
        for n in snap.nodes.iter().filter(|n| n.status == LinkStatus::Online && !n.stale) {
            for c in &n.containers {
                if c.state == "running" {
                    rows.push((format!("container:{}/{}", n.id, c.name), vec![c.cpu_pct, c.mem as f32]));
                }
            }
            for x in n.nets.iter().filter(|x| x.roce.is_some()) {
                let values = NET_METRICS.iter().map(|m| crate::health::rules::net_metric(x, m)).collect();
                counters.push((format!("net:{}/{}", n.id, x.iface), values));
            }
        }
        for s in snap.switches.iter().filter(|s| s.status == LinkStatus::Online) {
            for p in s.ports.iter().filter(|p| p.running || p.node.is_some()) {
                let entity = format!("port:{}/{}", s.id, p.name);
                let errors = (p.rx_errors + p.tx_errors + p.fcs_errors) as f64;
                let rate = inner.counters.get(&entity).and_then(|(at, prev)| {
                    let dt = snap.ts_ms.saturating_sub(*at) as f64 / 1000.0;
                    let before = prev.first().copied().flatten()?;
                    (dt > 0.0 && errors >= before).then(|| ((errors - before) / dt) as f32)
                });
                inner.counters.insert(entity.clone(), (snap.ts_ms, vec![Some(errors)]));
                let power = p.lane_rx_power_dbm.iter().copied().reduce(f32::min).or(p.rx_power_dbm).unwrap_or(f32::NAN);
                rows.push((entity, vec![p.rx_bps as f32, p.tx_bps as f32, rate.unwrap_or(f32::NAN), power]));
            }
        }
        for (entity, values) in counters {
            let rates = match inner.counters.get(&entity) {
                Some((at, prev)) if snap.ts_ms > *at => {
                    let dt = (snap.ts_ms - at) as f64 / 1000.0;
                    values
                        .iter()
                        .zip(prev)
                        .map(|(cur, before)| match (cur, before) {
                            // A counter that went back was reset: no rate for this second.
                            (Some(c), Some(b)) if c >= b => ((c - b) / dt) as f32,
                            _ => f32::NAN,
                        })
                        .collect()
                }
                _ => vec![f32::NAN; NET_METRICS.len()],
            };
            inner.counters.insert(entity.clone(), (snap.ts_ms, values));
            rows.push((entity, rates));
        }
        for e in endpoints.iter().filter(|e| e.health == "ready" || e.health == "loading") {
            let row = ENDPOINT_METRICS.iter().map(|m| e.metric(m).map(|v| v as f32).unwrap_or(f32::NAN)).collect();
            rows.push((format!("endpoint:{}", e.id), row));
        }
        for (entity, row) in rows {
            if row.iter().all(|v| v.is_nan()) {
                continue;
            }
            let width = row.len();
            let series = inner.series.entry(entity.clone()).or_insert_with(|| Series::with_metrics(metric_names(&entity)));
            if series.width() == width {
                series.push(t, &row);
            }
        }
        filled
    }

    /// One reading of one metric for any entity (`node:`, `endpoint:`, `container:`,
    /// `port:`, `net:`), into whichever source is recording now. Unknown entities and metrics
    /// are refused. Part of the C-monitor contract; `record_entity_in` also says the source.
    pub fn record_entity(&self, entity: &str, metric: &str, ts_ms: i64, value: f32) -> Result<(), String> {
        let demo = self.inner.lock().unwrap().source == Source::Demo;
        self.record_entity_in(demo, entity, metric, ts_ms, value)
    }

    /// `record_entity` for a reading taken from `demo` (true) or the Live cluster. A reading
    /// from the source that isn't recording any more (it was switched meanwhile) is refused,
    /// so a Demo test token can never land in Live history or the other way round.
    pub fn record_entity_in(&self, demo: bool, entity: &str, metric: &str, ts_ms: i64, value: f32) -> Result<(), String> {
        let names = metrics_of(entity).ok_or_else(|| format!("History doesn't keep \"{entity}\"."))?;
        let k = names.iter().position(|m| *m == metric).ok_or_else(|| format!("Unknown metric \"{metric}\" for {entity}."))?;
        let mut inner = self.inner.lock().unwrap();
        let source = if demo { Source::Demo } else { Source::Live };
        if inner.source != source {
            return Err(format!("This reading is from {} data, but SparkBox now records {}.", source.dir(), inner.source.dir()));
        }
        let series = inner.series.entry(entity.to_string()).or_insert_with(|| Series::with_metrics(metric_names(entity)));
        series.put(ts_ms.div_euclid(1000), k, value);
        Ok(())
    }

    /// Merges points a node agent recorded (`sparkbox_agent.py export`) into the bucket
    /// tiers (the 10 s tier included, so a 15-minute chart has them), and remembers the
    /// stretch as agent-filled. Only time with no readings of SparkBox's own is filled.
    /// Returns how many points were taken; when any were, the windows hear `history://filled`.
    pub fn merge_agent(&self, demo: bool, entity: &str, points: &[(u64, BTreeMap<String, f64>)]) -> usize {
        let taken = self.merge_agent_inner(demo, entity, points);
        if taken > 0 {
            self.filled(vec![entity.to_string()]);
        }
        taken
    }

    fn merge_agent_inner(&self, demo: bool, entity: &str, points: &[(u64, BTreeMap<String, f64>)]) -> usize {
        let Some(names) = metrics_of(entity) else { return 0 };
        let mut inner = self.on_source(demo);
        let now_s = (now_ms() / 1000) as i64;
        let series = inner.series.entry(entity.to_string()).or_insert_with(|| Series::with_metrics(metric_names(entity)));
        let mut taken = 0;
        let (mut from, mut to) = (u64::MAX, 0u64);
        for (ts_ms, values) in points {
            let t = (*ts_ms / 1000) as i64;
            // Older than the bucket tiers keep, or a 10 s bucket SparkBox has readings for itself.
            if t < now_s - 30 * 86_400 || series.has_bucket(t) {
                continue;
            }
            let row: Vec<f32> = names.iter().map(|m| values.get(*m).map(|v| *v as f32).unwrap_or(f32::NAN)).collect();
            series.push_buckets(t, &row);
            taken += 1;
            from = from.min(*ts_ms);
            to = to.max(*ts_ms);
        }
        if taken > 0 && entity.starts_with("node:") {
            Self::note_start(&mut inner, (from / 1000) as i64);
        }
        if taken > 0 {
            let ranges = inner.agent_ranges.entry(entity.to_string()).or_default();
            ranges.push([from, to]);
            ranges.sort();
            let mut merged: Vec<[u64; 2]> = Vec::new();
            for r in ranges.drain(..) {
                match merged.last_mut() {
                    Some(last) if r[0] <= last[1] + 120_000 => last[1] = last[1].max(r[1]),
                    _ => merged.push(r),
                }
            }
            *ranges = merged;
        }
        taken
    }

    /// The newest time (unix ms) with history for `entity`, for "export --since".
    pub fn last_point_ms(&self, demo: bool, entity: &str) -> Option<u64> {
        let inner = self.on_source(demo);
        let now_s = (now_ms() / 1000) as i64;
        inner.series.get(entity).and_then(|s| s.last_recorded_s(now_s)).map(|t| t as u64 * 1000)
    }

    /// Readings for `entity` between two unix-ms times, from the finest tier that covers
    /// the range, merged down to at most `max_points` buckets. The range ends an hour from now
    /// at the latest and `max_points` is held to 2..=20,000, so no answer holds up publishing.
    pub fn query(&self, entity: &str, metrics: &[String], from_ms: u64, to_ms: u64, max_points: usize) -> Result<QueryResult, String> {
        let names = metrics_of(entity).ok_or_else(|| format!("History is kept for nodes, endpoints, containers, switch ports and fabric interfaces, not \"{entity}\"."))?;
        let now = now_ms();
        let to_ms = to_ms.min(now + QUERY_AHEAD_MS);
        if to_ms <= from_ms {
            return Err("The end of the range has to come after its start.".into());
        }
        let mut idx = Vec::with_capacity(metrics.len());
        for m in metrics {
            idx.push(names.iter().position(|k| k == m).ok_or_else(|| format!("Unknown metric \"{m}\"."))?);
        }
        let inner = self.inner.lock().unwrap();
        // One "History starts" for every chart of this source: when its recording began.
        let starts = inner.started_s.map(|s| (s.max(0) as u64) * 1000);
        let Some(series) = inner.series.get(entity) else {
            return Ok(QueryResult { tier: "1s".into(), step_ms: 1000, starts_at_ms: starts, ..Default::default() });
        };
        let mut out = series.query(metrics, &idx, from_ms, to_ms, max_points.clamp(2, QUERY_MAX_POINTS), now);
        out.starts_at_ms = starts.or(out.starts_at_ms);
        out.agent_ranges = inner.agent_ranges.get(entity).map(|r| r.iter().filter(|x| x[1] >= from_ms && x[0] <= to_ms).copied().collect()).unwrap_or_default();
        Ok(out)
    }

    /// The minute save: appends the changed buckets of every entity of the current source
    /// that had readings to today's segments, and once an hour also compacts the oldest
    /// twenty-fourth of the entities with segments; one barrier for all of it.
    pub fn save(&self) -> Written {
        self.save_at(false, (now_ms() / 1000) as i64)
    }

    /// The save on Quit and SIGTERM: compacts every entity with segments or changes (so only
    /// base files are left), then the drive's cache flushed (F_FULLFSYNC), so the minutes
    /// before SparkBox went are on flash, those earlier saves left behind a barrier too.
    pub fn save_final(&self) -> Written {
        self.save_at(true, (now_ms() / 1000) as i64)
    }

    /// A save at unix time `now_s` (the tests run a day of them in a moment).
    fn save_at(&self, full: bool, now_s: i64) -> Written {
        let mut saves = self.saving.lock().unwrap();
        let syncer = self.syncer();
        // The source can't change while `saving` is held (`on_source`).
        let (source, left) = {
            let mut inner = self.inner.lock().unwrap();
            (inner.source, std::mem::take(&mut inner.obsolete))
        };
        saves.obsolete.extend(left);
        let Some(dir) = self.source_dir(source) else { return Written::default() };
        let due = full || saves.compacted_s.is_some_and(|t| now_s - t >= COMPACT_EVERY_S);
        if saves.compacted_s.is_none_or(|t| t > now_s) {
            saves.compacted_s = Some(now_s);
        }
        // The budget, from what's on disk (summed outside the lock): trimming, which is rare,
        // needs it, and then every entity is compacted so the files shrink. Segments already
        // retired don't count: they go after this save's barrier, and compacting because of
        // them would rewrite every base a second time.
        let retiring: u64 = saves.obsolete.iter().filter(|p| p.starts_with(&dir)).filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        let over = store::dir_bytes(&dir).saturating_sub(retiring) > self.budget as u64;
        if over {
            let mut inner = self.inner.lock().unwrap();
            let dropped = store::enforce_budget(&mut inner.series, self.budget, now_s);
            if !dropped.is_empty() {
                inner.started_s = first_node_reading(&inner.series, now_s);
            }
            drop(inner);
            store::remove_files(&dir, &dropped);
        }
        // Which entities get a new base this time; the others with changes get an append.
        let (compact, append) = {
            let inner = self.inner.lock().unwrap();
            let compact = Self::to_compact(&inner.series, full || over, due);
            let append: Vec<String> = inner.series.iter().filter(|(e, s)| s.dirty() && !compact.contains(*e)).map(|(e, _)| e.clone()).collect();
            (compact, append)
        };
        let mut written = Written::default();
        let mut appender = match store::Appender::new(&dir, &*syncer, &self.headed, now_s) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("history wasn't saved: {e}");
                return Written { failed: append.into_iter().chain(compact).collect(), ..Default::default() };
            }
        };
        for entity in &append {
            // One entity at a time: its changed buckets taken under the lock, encoded and appended outside it.
            let taken = {
                let mut inner = self.inner.lock().unwrap();
                inner.series.get_mut(entity).and_then(|s| Some((s.take_delta()?, s.metrics.clone(), s.epoch)))
            };
            let Some((delta, metrics, epoch)) = taken else { continue };
            match appender.append(entity, &metrics, epoch, &store::delta_record(&delta, now_s as u64 * 1000)) {
                Ok(()) => {
                    written.appended.push(entity.clone());
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(s) = inner.series.get_mut(entity).filter(|s| s.segments_since_ms.is_none()) {
                        s.segments_since_ms = Some(now_s as u64 * 1000);
                    }
                }
                Err(e) => {
                    eprintln!("history for {entity} wasn't saved: {e}");
                    written.failed.push(entity.clone());
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(s) = inner.series.get_mut(entity) {
                        s.put_back(delta.bits());
                    }
                }
            }
        }
        // The compaction: each base copied under the lock, written outside it.
        let mut writer = match store::Writer::new(&dir, &*syncer, now_s) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("history wasn't compacted: {e}");
                None
            }
        };
        let mut taken = Vec::new();
        if let Some(writer) = writer.as_mut() {
            for entity in &compact {
                let copy = self.inner.lock().unwrap().series.get_mut(entity).map(Series::copy_for_base);
                if let Some((copy, bits)) = copy {
                    writer.add(entity, &copy);
                    taken.push((entity.clone(), copy.epoch, bits));
                }
            }
        }
        // One barrier: the base files' (in `finish`), or else one for the appends and for
        // deleting what earlier compactions retired.
        let bases = writer.as_ref().is_some_and(store::Writer::has_files);
        if !bases && (appender.bytes > 0 || !saves.obsolete.is_empty()) {
            barrier(&dir, &*syncer);
        }
        let placed = match writer {
            Some(w) => w.finish(full),
            None => {
                flush_if(full, &dir, &*syncer);
                Written { failed: compact.clone(), ..Default::default() }
            }
        };
        // Behind that barrier now: the segments earlier compactions retired can go.
        for path in std::mem::take(&mut saves.obsolete) {
            let _ = std::fs::remove_file(path);
        }
        let mut retired = HashMap::new();
        {
            let mut inner = self.inner.lock().unwrap();
            for (entity, epoch, bits) in &taken {
                let Some(s) = inner.series.get_mut(entity) else { continue };
                if placed.compacted.contains(entity) {
                    s.epoch = *epoch;
                    s.base_stale = false;
                    s.segments_since_ms = None;
                    retired.insert(entity.clone(), Some(*epoch));
                } else {
                    s.put_back(bits);
                }
            }
        }
        // This compaction's retired segments: after the full flush they can go now, else
        // after the next save's barrier.
        let old = store::segments_of(&dir, &retired);
        if full {
            for path in old {
                let _ = std::fs::remove_file(path);
            }
        } else {
            saves.obsolete.extend(old);
        }
        if due {
            saves.compacted_s = Some(now_s);
        }
        written.bytes = appender.bytes + placed.bytes;
        written.compacted = placed.compacted;
        written.failed.extend(placed.failed);
        written.entities = written.appended.iter().chain(&written.compacted).cloned().collect();
        written
    }

    /// The entities a save compacts: with `all` (Quit, SIGTERM, over the budget) every one
    /// with segments or changes; when an hour has passed, every one whose base is stale and
    /// the oldest 1/COMPACT_SHARE of those with segments; otherwise none.
    fn to_compact(series: &BTreeMap<String, Series>, all: bool, due: bool) -> Vec<String> {
        if all {
            return series.iter().filter(|(_, s)| s.changed() || s.segments_since_ms.is_some()).map(|(e, _)| e.clone()).collect();
        }
        if !due {
            return Vec::new();
        }
        let mut out: Vec<String> = series.iter().filter(|(_, s)| s.base_stale).map(|(e, _)| e.clone()).collect();
        let mut queue: Vec<(u64, &String)> = series.iter().filter(|(_, s)| !s.base_stale).filter_map(|(e, s)| Some((s.segments_since_ms?, e))).collect();
        queue.sort();
        let share = queue.len().div_ceil(COMPACT_SHARE);
        out.extend(queue.into_iter().take(share).map(|(_, e)| e.clone()));
        out
    }

    /// Entity keys with any history in the current source.
    #[cfg(test)]
    pub fn entities(&self) -> Vec<String> {
        self.inner.lock().unwrap().series.keys().cloned().collect()
    }

    /// Node ids with any history in the current source.
    #[cfg(test)]
    pub fn node_ids(&self) -> Vec<String> {
        self.inner.lock().unwrap().series.keys().filter_map(|k| k.strip_prefix("node:").map(String::from)).collect()
    }
}

/// One barrier on the folder (F_BARRIERFSYNC): what was written before reaches the drive
/// before anything written after.
fn barrier(dir: &Path, syncer: &dyn crate::util::Syncer) {
    let result = std::fs::File::open(dir).and_then(|f| syncer.sync(&f, crate::util::Flush::Barrier));
    if let Err(e) = result {
        eprintln!("history in {} wasn't synced: {e}", dir.display());
    }
}

/// With `full`, the drive's cache flushed to flash (F_FULLFSYNC on the folder).
fn flush_if(full: bool, dir: &Path, syncer: &dyn crate::util::Syncer) {
    if full {
        if let Err(e) = std::fs::File::open(dir).and_then(|f| syncer.sync(&f, crate::util::Flush::Full)) {
            eprintln!("{} wasn't flushed to disk: {e}", dir.display());
        }
    }
}

/// Write end of the pipe the signal handler wakes the saver through (-1 until installed).
static SIGNAL_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIGNAL_PIPE.load(std::sync::atomic::Ordering::Relaxed);
    if fd >= 0 {
        let b = sig as u8;
        // write(2) is async-signal-safe; everything else happens on the saver thread.
        unsafe { libc::write(fd, &b as *const u8 as *const libc::c_void, 1) };
    }
}

/// SIGTERM and SIGINT (a dev restart, `kill`, a logout) save history before SparkBox goes,
/// the way Quit does, then quit through the normal exit (so tasks are saved and tunnels
/// closed too). A second signal, or an exit that hangs for 5 s, ends the process at once.
pub fn save_on_signals(app: &tauri::AppHandle) {
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        eprintln!("history won't be saved on SIGTERM: no pipe");
        return;
    }
    SIGNAL_PIPE.store(fds[1], std::sync::atomic::Ordering::Relaxed);
    for sig in [libc::SIGTERM, libc::SIGINT] {
        unsafe {
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
            act.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut act.sa_mask);
            libc::sigaction(sig, &act, std::ptr::null_mut());
        }
    }
    let app = app.clone();
    let read_fd = fds[0];
    std::thread::Builder::new()
        .name("history-on-signal".into())
        .spawn(move || {
            let mut b = 0u8;
            let mut signals = 0;
            loop {
                let n = unsafe { libc::read(read_fd, &mut b as *mut u8 as *mut libc::c_void, 1) };
                if n <= 0 {
                    continue;
                }
                signals += 1;
                if signals > 1 {
                    std::process::exit(128 + b as i32);
                }
                use tauri::Manager;
                if let Some(store) = app.try_state::<std::sync::Arc<History>>() {
                    store.save_final();
                }
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    std::process::exit(128 + b as i32);
                });
                app.exit(0);
            }
        })
        .ok();
}

pub mod commands {
    use std::sync::Arc;

    use tauri::State;

    use super::{History, QueryResult};

    #[tauri::command]
    pub fn history_query(
        history: State<'_, Arc<History>>,
        entity: String,
        metrics: Vec<String>,
        from_ms: u64,
        to_ms: u64,
        max_points: usize,
    ) -> Result<QueryResult, String> {
        history.query(&entity, &metrics, from_ms, to_ms, max_points)
    }
}
