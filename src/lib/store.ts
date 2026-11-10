// Holds the latest cluster snapshot plus one hour of per-node history at 1 s,
// so charts open with data already in them. Seconds a node was offline or
// stale are kept as gaps (null), never as zeros. Longer ranges come from the
// Rust history store (historyApi.ts), which also fills this hour before the
// first paint and again whenever it grows (`history://filled`: an agent's
// recording arrived), merged in without duplicates. Switching between Demo and
// Live starts the hour over from the new source's own history, with its first
// frame plotted at once.
import { useRef, useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, inTauri } from "./api";
import { migrateLegacy, scopedKey, type Source } from "./dataSource";
import { queryHistory, type HistoryMetric, type HistoryResult } from "./historyApi";
import { fabricRx, fabricTx, gpuError, lanRx, lanTx, rdmaRx, rdmaTx, rootDisk } from "./metrics";
import type { ClusterSnapshot, NodeSnapshot } from "./types";

export const HISTORY_SECONDS = 3600;
const CAPACITY = HISTORY_SECONDS + 64;
/** A pause longer than this between readings breaks the line instead of joining across it. */
const GAP_S = 2.5;

export const SERIES = [
  "cpu", "gpu", "memUsed", "gpuAlloc", "power", "gpuTemp",
  "fabricRx", "fabricTx", "rdmaRx", "rdmaTx", "lanRx", "lanTx", "diskRead", "diskWrite", "memCache", "smClock",
] as const;
export type SeriesKey = (typeof SERIES)[number];
/** Oldest first; ts in unix seconds; null where the node wasn't reporting. */
export type NodeHistory = { ts: number[] } & Record<SeriesKey, (number | null)[]>;

/** Fixed-size history for one node: no array shifting, one slot per reading. */
class Ring {
  readonly ts = new Float64Array(CAPACITY);
  readonly cols = SERIES.map(() => new Float64Array(CAPACITY));
  start = 0;
  len = 0;
  version = 0;

  get lastTs(): number | undefined {
    return this.len ? this.ts[(this.start + this.len - 1) % CAPACITY] : undefined;
  }

  /** `values` in SERIES order, or null for a gap. */
  push(t: number, values: ArrayLike<number> | null) {
    const i = (this.start + this.len) % CAPACITY;
    this.ts[i] = t;
    for (let k = 0; k < SERIES.length; k++) this.cols[k][i] = values ? values[k] : NaN;
    if (this.len < CAPACITY) this.len++;
    else this.start = (this.start + 1) % CAPACITY;
    this.version++;
  }

  /** Replaces the contents (oldest first), keeping the newest CAPACITY entries. */
  replace(ts: number[], rows: (ArrayLike<number> | null)[]) {
    const skip = Math.max(0, ts.length - CAPACITY);
    this.start = 0;
    this.len = 0;
    for (let j = skip; j < ts.length; j++) {
      const i = this.len;
      this.ts[i] = ts[j];
      for (let k = 0; k < SERIES.length; k++) this.cols[k][i] = rows[j] ? (rows[j] as ArrayLike<number>)[k] : NaN;
      this.len++;
    }
    this.version++;
  }

  view(fromS: number): NodeHistory {
    let first = 0;
    while (first < this.len && this.ts[(this.start + first) % CAPACITY] < fromS) first++;
    const n = this.len - first;
    const out = { ts: new Array<number>(n) } as NodeHistory;
    for (const key of SERIES) out[key] = new Array<number | null>(n);
    for (let j = 0; j < n; j++) {
      const i = (this.start + first + j) % CAPACITY;
      out.ts[j] = this.ts[i];
      for (let k = 0; k < SERIES.length; k++) {
        const v = this.cols[k][i];
        out[SERIES[k]][j] = Number.isNaN(v) ? null : v;
      }
    }
    return out;
  }
}

let snapshot: ClusterSnapshot | null = null;
let receivedAtMs = 0;
const rings = new Map<string, Ring>();
const listeners = new Set<() => void>();

function sample(n: NodeSnapshot): number[] {
  const root = rootDisk(n);
  // A GPU that can't be read is a gap, not an idle GPU.
  const gpu = (v: number) => (gpuError(n) ? NaN : v);
  const byKey: Record<SeriesKey, number> = {
    cpu: n.cpu.totalPct,
    gpu: gpu(n.gpu.utilPct),
    memUsed: n.mem.used,
    gpuAlloc: n.mem.gpuAlloc,
    power: gpu(n.gpu.powerW),
    gpuTemp: gpu(n.gpu.tempC),
    fabricRx: fabricRx(n),
    fabricTx: fabricTx(n),
    rdmaRx: rdmaRx(n),
    rdmaTx: rdmaTx(n),
    lanRx: lanRx(n),
    lanTx: lanTx(n),
    diskRead: root?.readBps ?? 0,
    diskWrite: root?.writeBps ?? 0,
    memCache: n.mem.cached,
    smClock: gpu(n.gpu.smClockMhz),
  };
  return SERIES.map((k) => byKey[k]);
}

function ringFor(id: string): Ring {
  let r = rings.get(id);
  if (!r) {
    r = new Ring();
    rings.set(id, r);
  }
  return r;
}

/** The id of the ring that holds cluster totals (sums; CPU and GPU are averages, temperature the hottest). */
export const CLUSTER = "__cluster__";
const AVERAGED = new Set<SeriesKey>(["cpu", "gpu"]);

function push(ring: Ring, t: number, values: number[] | null) {
  const last = ring.lastTs;
  if (last !== undefined && t - last < 0.5) return; // an action-triggered refresh, not a new second
  if (last !== undefined && t - last > GAP_S) ring.push(last + 1, null);
  ring.push(t, values);
}

// Each node's memory size as it last reported it, so its history still reads as a share of
// memory while it's offline (an offline node reports a total of 0). Kept across reloads, per
// data source (C2): Demo's 119.7 GiB nodes are not the real 121.7 GiB ones.
const MEM_TOTALS_KEY = "sparkbox.memTotals.v1";
const memTotals: Record<Source, Map<string, number>> = { demo: new Map(), live: new Map() };
let memLoaded = false;

function loadMemTotals() {
  if (memLoaded) return;
  memLoaded = true;
  // Totals saved before they were kept per source came from the simulator.
  migrateLegacy(MEM_TOTALS_KEY);
  for (const src of ["demo", "live"] as const) {
    try {
      const v = JSON.parse(localStorage.getItem(scopedKey(MEM_TOTALS_KEY, src)) ?? "{}");
      for (const [id, total] of Object.entries(v)) if (typeof total === "number" && total > 0) memTotals[src].set(id, total);
    } catch {
      // Unreadable: learnt again from the next snapshot.
    }
  }
}

const sourceOf = (s: Pick<ClusterSnapshot, "demo">): Source => (s.demo ? "demo" : "live");

function rememberMemTotal(n: NodeSnapshot, src: Source) {
  loadMemTotals();
  const totals = memTotals[src];
  if (!(n.mem.total > 0) || totals.get(n.id) === n.mem.total) return;
  totals.set(n.id, n.mem.total);
  try {
    localStorage.setItem(scopedKey(MEM_TOTALS_KEY, src), JSON.stringify(Object.fromEntries(totals)));
  } catch {
    // Private window or blocked storage: remembered for this session only.
  }
}

/** The node's memory size: as reported now, or as last reported (in this data source) while it's offline (0 if never). */
export function memTotalOf(n: NodeSnapshot): number {
  if (n.mem.total > 0) return n.mem.total;
  loadMemTotals();
  return memTotals[ringSource ?? (snapshot ? sourceOf(snapshot) : "demo")].get(n.id) ?? 0;
}

/** The source the rings hold, so a switch starts them over from the new source's history. */
let ringSource: Source | null = null;
/** When the current source's history begins (unix s), from the store: one time for every chart. */
let historyStartS: number | null = null;

/** When recorded history begins for the source on screen (unix s), or null before it's known. */
export function historyStart(): number | null {
  return historyStartS;
}

/**
 * Demo and Live never share a chart: on a switch the hour starts over, the new source's first
 * frame is drawn at once, and its own stored history is merged in when the store answers.
 */
function switchSource(s: ClusterSnapshot) {
  rings.clear();
  cache.clear();
  historyStartS = null;
  if (inTauri) void loadHistory(s.nodes.map((n) => n.id), s.tsMs - 500).then((changed) => changed && notify());
}

/** Adds one second of readings. Exported for tests. */
export function recordSnapshot(s: ClusterSnapshot) {
  const src = sourceOf(s);
  if (ringSource !== null && ringSource !== src) switchSource(s);
  ringSource = src;
  const t = s.tsMs / 1000;
  const samples: number[][] = [];
  for (const n of s.nodes) {
    rememberMemTotal(n, src);
    const reporting = n.status === "online" && !n.stale;
    const v = reporting ? sample(n) : null;
    if (v) samples.push(v);
    push(ringFor(n.id), t, v);
  }
  const total = samples.length
    ? SERIES.map((k, i) => {
        const vals = samples.map((v) => v[i]);
        if (k === "gpuTemp") return Math.max(...vals);
        const sum = vals.reduce((a, b) => a + b, 0);
        return AVERAGED.has(k) ? sum / vals.length : sum;
      })
    : null;
  push(ringFor(CLUSTER), t, total);
}

/** Clears everything (tests). */
export function resetStore() {
  snapshot = null;
  rings.clear();
  cache.clear();
  memTotals.demo.clear();
  memTotals.live.clear();
  memLoaded = false;
  ringSource = null;
  historyStartS = null;
  refillNodes.clear();
  if (refill !== undefined) window.clearTimeout(refill);
  refill = undefined;
}

function notify() {
  listeners.forEach((l) => l());
}

const isEmpty = (h: NodeHistory, j: number) => SERIES.every((k) => h[k][j] == null);

/**
 * Merges what the history store has into a ring (§2.4 "charts open full"): a stored bucket is
 * taken only where the ring has no entry at its start (live 1 s readings and offline seconds
 * win), gap markers the new points fill are dropped, and a null goes wherever a gap is left so
 * the line breaks there. Returns whether anything was added.
 */
function mergeInto(ring: Ring, r: HistoryResult): boolean {
  const step = Math.max(1, (r.stepMs || 1000) / 1000);
  const have = ring.view(-Infinity);
  const n = have.ts.length;
  // A null followed by a jump is a marker the ring left where recording stopped.
  const marker = (j: number) => isEmpty(have, j) && j + 1 < n && have.ts[j + 1] - have.ts[j] > GAP_S;
  const kept: { t: number; row: number[] | null; fetched: boolean; broken?: boolean }[] = [];
  for (let j = 0; j < n; j++) {
    if (marker(j)) continue;
    kept.push({ t: have.ts[j], row: isEmpty(have, j) ? null : SERIES.map((k) => have[k][j] ?? NaN), fetched: false, broken: j > 0 && marker(j - 1) });
  }
  const times = kept.map((e) => e.t);
  // Whether the ring has an entry at second t: binary search over its sorted times.
  const covered = (t: number) => {
    let lo = 0;
    let hi = times.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (times[mid] < t - 0.5) lo = mid + 1;
      else hi = mid;
    }
    return lo < times.length && times[lo] <= t + 0.5;
  };
  const added: typeof kept = [];
  for (let i = 0; i < r.ts.length; i++) {
    const t = r.ts[i] / 1000;
    const row = SERIES.map((k) => r.series[k]?.[i] ?? NaN);
    if (row.every(Number.isNaN) || covered(t)) continue;
    added.push({ t, row, fetched: true });
  }
  if (!added.length) return false;
  const all = [...kept, ...added].sort((a, b) => a.t - b.t);
  const ts: number[] = [];
  const rows: (number[] | null)[] = [];
  for (let j = 0; j < all.length; j++) {
    const e = all[j];
    const prev = all[j - 1];
    if (prev) {
      // Next to a stored bucket, a step is `step`; two entries the ring already had stay as they
      // were (broken only where they were).
      const gap = prev.fetched || e.fetched ? e.t - prev.t > Math.max(GAP_S, step * 1.5) : !!e.broken;
      if (gap) {
        ts.push(prev.t + (prev.fetched ? step : 1));
        rows.push(null);
      }
    }
    ts.push(e.t);
    rows.push(e.row);
  }
  ring.replace(ts, rows);
  return true;
}

/** Cluster totals for every second the nodes have, from their rings (sums; CPU and GPU averaged, temperature the hottest). */
function rebuildCluster(nodeIds: string[]) {
  const views = nodeIds.map((id) => rings.get(id)?.view(-Infinity)).filter((h): h is NodeHistory => !!h && h.ts.length > 0);
  if (!views.length) return;
  const byTs = new Map<number, number[][]>();
  const seen = new Set<number>();
  for (const h of views) {
    h.ts.forEach((t, i) => {
      const key = Math.round(t);
      seen.add(key);
      const vals = SERIES.map((k) => h[k][i]);
      if (vals.every((v) => v == null)) return;
      const list = byTs.get(key) ?? [];
      list.push(vals.map((v) => v ?? 0));
      byTs.set(key, list);
    });
  }
  const ts = [...seen].sort((a, b) => a - b);
  const rows = ts.map((t) => {
    const samples = byTs.get(t);
    if (!samples) return null;
    return SERIES.map((k, i) => {
      const vals = samples.map((v) => v[i]);
      if (k === "gpuTemp") return Math.max(...vals);
      const sum = vals.reduce((a, b) => a + b, 0);
      return AVERAGED.has(k) ? sum / vals.length : sum;
    });
  });
  ringFor(CLUSTER).replace(ts, rows);
}

/**
 * Loads the stored hour for these nodes into their rings: the last 15 minutes at 1 s first
 * (they win), then the hour at whatever resolution the store has (10 s after a restart, or
 * where an agent filled it). Then the cluster totals are rebuilt.
 */
async function loadHistory(nodeIds: string[], toMs: number): Promise<boolean> {
  let changed = false;
  await Promise.all(
    nodeIds.map(async (id) => {
      try {
        const metrics = [...SERIES] as HistoryMetric[];
        const [recent15, hour] = await Promise.all([
          queryHistory(`node:${id}`, metrics, toMs - 900_000, toMs, 900),
          queryHistory(`node:${id}`, metrics, toMs - HISTORY_SECONDS * 1000, toMs, HISTORY_SECONDS),
        ]);
        const ring = ringFor(id);
        if (mergeInto(ring, recent15)) changed = true;
        if (mergeInto(ring, hour)) changed = true;
        const starts = recent15.startsAtMs ?? hour.startsAtMs;
        if (starts != null && (historyStartS == null || starts / 1000 < historyStartS)) {
          historyStartS = starts / 1000;
          changed = true;
        }
      } catch {
        // No history store (older backend): charts fill from now on.
      }
    }),
  );
  if (changed) rebuildCluster(nodeIds);
  return changed;
}

/** The longest the first paint waits for stored history. */
const PREFILL_WAIT_MS = 1500;
let refill: number | undefined;
const refillNodes = new Set<string>();

/** History grew (an agent's recording arrived, or Demo filled a gap): merge it in, once per burst. */
function onFilled(entities: string[]) {
  for (const e of entities) if (e.startsWith("node:")) refillNodes.add(e.slice(5));
  if (!refillNodes.size || refill !== undefined) return;
  refill = window.setTimeout(async () => {
    refill = undefined;
    const ids = [...refillNodes];
    refillNodes.clear();
    const to = (snapshot?.tsMs ?? Date.now()) - 500;
    if (await loadHistory(ids.filter((id) => snapshot?.nodes.some((n) => n.id === id) ?? true), to)) notify();
  }, 400);
}

/**
 * Starts following snapshots. Charts open full: nothing is shown (windows say "Waiting for
 * data…") until the stored hour is in the rings, or 1.5 s have passed.
 */
export async function startClusterStore() {
  if (!inTauri) return;
  let ready = false;
  const pending: ClusterSnapshot[] = [];
  await api.onSnapshot((s) => {
    if (ready) {
      snapshot = s;
      receivedAtMs = Date.now();
      recordSnapshot(s);
      notify();
    } else {
      pending.push(s);
    }
  });
  listen<{ entities: string[] }>("history://filled", (e) => onFilled(e.payload?.entities ?? [])).catch(() => {});
  const first = (await api.getSnapshot()) ?? pending[0] ?? null;
  if (first) {
    const loaded = loadHistory(first.nodes.map((n) => n.id), first.tsMs - 500);
    await Promise.race([loaded, new Promise((r) => window.setTimeout(r, PREFILL_WAIT_MS))]);
    recordSnapshot(first);
    // Slower than the wait: merged in when it arrives.
    void loaded.then((changed) => changed && notify());
  }
  ready = true;
  const later = pending.filter((s) => !first || s.tsMs > first.tsMs);
  later.forEach(recordSnapshot);
  snapshot = later[later.length - 1] ?? first ?? snapshot;
  receivedAtMs = Date.now();
  notify();
}

/**
 * The x window for a chart of the last `seconds` (unix s): normally [now − seconds, now]. When
 * history is truly missing (nothing recorded that far back by SparkBox or an agent), the
 * window collapses to the data, at least 2 minutes, and widens as data arrives; `startsAt`
 * marks where recording began only when that's inside the window. `recordedFrom` is when the
 * source's history begins (historyStart()), so every window names the same time.
 */
export function chartWindow(ts: ArrayLike<number>, nowS: number, seconds: number, recordedFrom: number | null = null): { range: [number, number]; startsAt: number | null } {
  const full: [number, number] = [nowS - seconds, nowS];
  const first = ts.length ? (recordedFrom != null ? Math.min(ts[0], recordedFrom) : ts[0]) : recordedFrom;
  if (first == null || first <= full[0] + 5) return { range: full, startsAt: null };
  const from = Math.max(full[0], Math.min(first, nowS - 120));
  return { range: [from, nowS], startsAt: first > from + 1 ? first : null };
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useCluster(): ClusterSnapshot | null {
  return useSyncExternalStore(subscribe, () => snapshot);
}

export function getCluster(): ClusterSnapshot | null {
  return snapshot;
}

export function useNode(id: string): NodeSnapshot | undefined {
  return useCluster()?.nodes.find((n) => n.id === id);
}

/**
 * A slice of the snapshot that only re-renders when it changes (by `eq`, default ===).
 * For example `useClusterSelector((s) => s?.nodes.map((n) => n.status), shallowEqual)`.
 */
export function useClusterSelector<T>(sel: (s: ClusterSnapshot | null) => T, eq: (a: T, b: T) => boolean = Object.is): T {
  const memo = useRef<{ snap: ClusterSnapshot | null; sel: typeof sel; val: T } | null>(null);
  const get = () => {
    const m = memo.current;
    if (m && m.snap === snapshot && m.sel === sel) return m.val;
    const next = sel(snapshot);
    const val = m && eq(m.val, next) ? m.val : next;
    memo.current = { snap: snapshot, sel, val };
    return val;
  };
  return useSyncExternalStore(subscribe, get);
}

/** Equal when both are arrays (or objects) with the same items by ===. */
export function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || !a || !b) return false;
  const ka = Object.keys(a) as (keyof T)[];
  const kb = Object.keys(b);
  return ka.length === kb.length && ka.every((k) => Object.is(a[k], b[k]));
}

// ---- Snapshot age: drives "Live · 1 s" / "Stale 8 s" in the status bar.

let tick = 0;
let ticker: number | undefined;
const tickListeners = new Set<() => void>();

function subscribeTick(l: () => void) {
  tickListeners.add(l);
  if (ticker === undefined) {
    ticker = window.setInterval(() => {
      tick++;
      tickListeners.forEach((f) => f());
    }, 1000);
  }
  return () => {
    tickListeners.delete(l);
    if (tickListeners.size === 0) {
      window.clearInterval(ticker);
      ticker = undefined;
    }
  };
}

/** Milliseconds since the last snapshot arrived (re-checked every second), or null before the first. */
export function useSnapshotAge(): number | null {
  useSyncExternalStore(subscribeTick, () => tick);
  useSyncExternalStore(subscribe, () => snapshot);
  return snapshot ? Math.max(0, Date.now() - receivedAtMs) : null;
}

// ---- History

const cache = new Map<string, { version: number; fromS: number; h: NodeHistory }>();

/**
 * One node's readings for the last `seconds` (default: all of the last hour), oldest first.
 * The same arrays come back until a new second is recorded, so charts can skip redraws.
 */
export function getHistory(nodeId: string, seconds: number = HISTORY_SECONDS): NodeHistory | undefined {
  const ring = rings.get(nodeId);
  if (!ring || ring.len === 0) return undefined;
  const lastTs = ring.lastTs ?? 0;
  const fromS = lastTs - seconds;
  const key = `${nodeId}|${seconds}`;
  const hit = cache.get(key);
  if (hit && hit.version === ring.version) return hit.h;
  const h = ring.view(fromS);
  cache.set(key, { version: ring.version, fromS, h });
  return h;
}

/** One node's series on another history's timestamps (null where it has no reading then). */
export function alignTo(ts: number[], nodeId: string, key: SeriesKey, seconds: number = HISTORY_SECONDS): (number | null)[] {
  const h = getHistory(nodeId, seconds);
  if (!h) return ts.map(() => null);
  const at = new Map<number, number | null>();
  h.ts.forEach((t, i) => at.set(Math.round(t), h[key][i]));
  return ts.map((t) => at.get(Math.round(t)) ?? null);
}

/** The last `points` readings of one series with gaps dropped, for sparklines. */
export function recent(nodeId: string, key: SeriesKey, points: number): number[] {
  const h = getHistory(nodeId, points);
  if (!h) return [];
  return h[key].slice(-points).filter((v): v is number => v != null);
}
