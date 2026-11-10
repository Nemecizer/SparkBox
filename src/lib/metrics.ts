// Every number derived from a snapshot, defined once. The Dashboard, Topology,
// node windows, status bar and switch window all read these, so they can't
// disagree about what "fabric traffic" or "serving" means.
import { endpointLabel, isServing } from "./endpoints";
import { GiB } from "./format";
import type { ClusterSnapshot, DiskStats, NetStats, NodeSnapshot } from "./types";

export { GiB };

/** One ConnectX-7 cable per node into the CRS804. Its two netdevs (one per PCIe root) share it. */
export const FABRIC_LINK_BPS = 200e9;

const fabricNets = (n: NodeSnapshot) => n.nets.filter((x) => x.role === "fabric");

/**
 * RDMA bytes, counted once per RDMA device. mlx5 netdev counters (/proc/net/dev) only see
 * traffic that went through the kernel's IP stack; RoCE traffic shows up on the RDMA port
 * counters instead, so the two add up without overlap.
 */
function rdmaSum(nets: NetStats[], key: "rdmaRxBps" | "rdmaTxBps"): number {
  const seen = new Set<string>();
  let sum = 0;
  for (const x of nets) {
    const v = x[key];
    if (v == null) continue;
    const dev = x.rdmaDev ?? x.iface;
    if (seen.has(dev)) continue;
    seen.add(dev);
    sum += v;
  }
  return sum;
}

function ipSum(nets: NetStats[], key: "rxBps" | "txBps"): number {
  return nets.reduce((a, x) => a + x[key], 0);
}

/** RDMA (RoCE) receive rate on the fabric, bits/s. */
export const rdmaRx = (n: NodeSnapshot) => rdmaSum(fabricNets(n), "rdmaRxBps");
export const rdmaTx = (n: NodeSnapshot) => rdmaSum(fabricNets(n), "rdmaTxBps");
/** Kernel IP (TCP/UDP) traffic on the fabric, bits/s. */
export const tcpRx = (n: NodeSnapshot) => ipSum(fabricNets(n), "rxBps");
export const tcpTx = (n: NodeSnapshot) => ipSum(fabricNets(n), "txBps");

/** Everything a node receives over the fabric (IP + RDMA), bits/s. */
export const fabricRx = (n: NodeSnapshot) => tcpRx(n) + rdmaRx(n);
/** Everything a node sends over the fabric (IP + RDMA), bits/s. */
export const fabricTx = (n: NodeSnapshot) => tcpTx(n) + rdmaTx(n);

export const lanRx = (n: NodeSnapshot) => ipSum(n.nets.filter((x) => x.role === "lan"), "rxBps");
export const lanTx = (n: NodeSnapshot) => ipSum(n.nets.filter((x) => x.role === "lan"), "txBps");

/** The negotiated speed of a node's fabric cable (bits/s), or 0 when no fabric port is up. */
export function fabricCapBps(n: NodeSnapshot): number {
  const up = fabricNets(n).filter((x) => x.up);
  if (up.length === 0) return 0;
  const mbps = Math.max(...up.map((x) => x.speedMbps));
  return mbps > 0 ? mbps * 1e6 : FABRIC_LINK_BPS;
}

/** Share of a link in use, 0..1: the busier direction over the capacity. */
export function linkUtil(rx: number, tx: number, capBps: number): number {
  if (!(capBps > 0)) return 0;
  return Math.max(0, Math.min(1, Math.max(rx, tx) / capBps));
}

/** Utilisation of one direction, 0..1. */
export const dirUtil = (bps: number, capBps: number) => (capBps > 0 ? Math.max(0, Math.min(1, bps / capBps)) : 0);

const reporting = (n: NodeSnapshot) => n.status === "online";

/**
 * Fabric throughput (sent): the sum of what every node transmits. Each byte crosses the
 * switch once, so summing transmit (not receive + transmit) doesn't count it twice.
 */
export function clusterFabric(snap: ClusterSnapshot): number {
  return snap.nodes.filter(reporting).reduce((a, n) => a + fabricTx(n), 0);
}

export function clusterFabricRx(snap: ClusterSnapshot): number {
  return snap.nodes.filter(reporting).reduce((a, n) => a + fabricRx(n), 0);
}

/** The fabric's total capacity: one 200G link per node ("4×200G"). */
export const clusterFabricCap = (snap: ClusterSnapshot) => snap.nodes.length * FABRIC_LINK_BPS;

export function rootDisk(n: NodeSnapshot) {
  return n.disks.find((d) => d.mount === "/") ?? n.disks[0];
}

export const pct = (part: number, whole: number) => (whole > 0 ? (part / whole) * 100 : 0);
export const memPct = (n: NodeSnapshot) => pct(n.mem.used, n.mem.total);
export function diskPct(n: NodeSnapshot): number {
  const d = rootDisk(n);
  return d ? diskUsedPct(d) : 0;
}

// ── r1-c ──
/** Free space as df reports it: `avail` (what a normal user can still write), or total − used from an older collector. */
export const diskFree = (d: Pick<DiskStats, "total" | "used" | "avail">) => (d.avail != null ? d.avail : Math.max(0, d.total - d.used));
/** df's Use%: used / (used + avail), since ext4 keeps 5% back for root; over the total without `avail`. */
export const diskUsedPct = (d: Pick<DiskStats, "total" | "used" | "avail">) => (d.avail != null ? pct(d.used, d.used + d.avail) : pct(d.used, d.total));

/**
 * Why a node's GPU numbers are unknown right now, else null: the frame said the GPU couldn't
 * be read (GpuStats.readable false, with its reason in GpuStats.error). The same test as
 * model::gpu_readable; no error text is matched. While it holds, the numbers read "—" and
 * charts keep a gap rather than an idle 0.
 */
export function gpuError(n: NodeSnapshot): string | null {
  if (n.gpu.readable !== false) return null;
  return n.gpu.error || "The GPU couldn't be read";
}

export const runningContainers = (n: NodeSnapshot) => n.containers.filter((c) => c.state === "running");
/** Stopped with a failure: a non-zero exit code or killed for lack of memory. */
export const failedContainers = (n: NodeSnapshot) =>
  n.containers.filter((c) => c.state !== "running" && c.state !== "created" && (c.exitCode !== 0 || c.oomKilled));

// ---- Serving: which GPUs are doing inference work right now.

/** A GPU counts as serving when it holds model weights or has been busy. */
export const SERVING_ALLOC_BYTES = 1 * GiB;
export const SERVING_UTIL_PCT = 10;
const SERVING_WINDOW_MS = 30_000;
const SERVING_HOLD_MS = 10_000;

interface ServingTrack {
  lastTsMs: number;
  util: { t: number; v: number }[];
  lastTrueMs: number | null;
  serving: boolean;
}

const tracks = new Map<string, ServingTrack>();

export interface GpuServing {
  count: number;
  total: number;
  /** Ids of the serving nodes, in snapshot order. */
  nodes: string[];
}

/** What gpuServing needs to know about an endpoint (src/lib/endpoints.ts `Endpoint`). */
export interface ServingEndpoint {
  health: string;
  nodes: string[];
  /** Every node the deployment should span; a pipeline's members. */
  expectedNodes?: string[];
  /** Unix ms of the last probe the endpoint answered. Absent only on backends older than round 2. */
  lastOkMs?: number | null;
}

/** An endpoint counts as serving only when it answered a probe this recently. */
export const SERVING_FRESH_MS = 60_000;

/**
 * GPUs doing inference work right now. With `endpoints` (the truthful count, §2.4): the GPUs
 * on the member nodes of every endpoint that is Ready and answered a probe in the last
 * 60 s, on nodes that are reporting. A pipeline with a stage offline is Down, so none of its
 * GPUs count. Without endpoints (older callers) it falls back to the GPU heuristic below.
 */
export function gpuServing(snap: ClusterSnapshot, endpoints?: readonly ServingEndpoint[]): GpuServing {
  if (!endpoints) return gpuBusy(snap);
  const now = snap.tsMs;
  const members = new Set<string>();
  for (const e of endpoints) {
    if (e.health !== "ready") continue;
    if (e.lastOkMs !== undefined && (e.lastOkMs == null || now - e.lastOkMs > SERVING_FRESH_MS)) continue;
    for (const id of e.expectedNodes?.length ? e.expectedNodes : e.nodes) members.add(id);
  }
  const nodes = snap.nodes.filter((n) => reporting(n) && members.has(n.id)).map((n) => n.id);
  return { count: nodes.length, total: snap.nodes.length, nodes };
}

/**
 * Nodes whose GPU looks busy: more than 1 GiB of GPU memory allocated, or a 30-second
 * mean utilisation above 10%. It stays "busy" for 10 s after both drop, so a pause
 * between requests doesn't flicker the count. Call it with each snapshot (any number of
 * times; each snapshot is only counted once).
 */
export function gpuBusy(snap: ClusterSnapshot): GpuServing {
  const nodes: string[] = [];
  for (const n of snap.nodes) {
    let tr = tracks.get(n.id);
    if (!tr) {
      tr = { lastTsMs: -1, util: [], lastTrueMs: null, serving: false };
      tracks.set(n.id, tr);
    }
    if (snap.tsMs > tr.lastTsMs) {
      tr.lastTsMs = snap.tsMs;
      if (!reporting(n)) {
        tr.util = [];
        tr.lastTrueMs = null;
        tr.serving = false;
      } else {
        tr.util.push({ t: snap.tsMs, v: n.gpu.utilPct });
        while (tr.util.length && tr.util[0].t <= snap.tsMs - SERVING_WINDOW_MS) tr.util.shift();
        const mean = tr.util.reduce((a, s) => a + s.v, 0) / tr.util.length;
        const raw = n.mem.gpuAlloc > SERVING_ALLOC_BYTES || mean > SERVING_UTIL_PCT;
        if (raw) tr.lastTrueMs = snap.tsMs;
        tr.serving = raw || (tr.lastTrueMs != null && snap.tsMs - tr.lastTrueMs < SERVING_HOLD_MS);
      }
    }
    if (tr.serving) nodes.push(n.id);
  }
  return { count: nodes.length, total: snap.nodes.length, nodes };
}

/** Forgets serving state (tests). */
export function resetServing() {
  tracks.clear();
}

/** What a node is serving: running containers holding GPU memory, else GPU processes. */
export function engines(n: NodeSnapshot): string[] {
  const containers = n.containers.filter((c) => c.state === "running" && c.gpuMem > 0).map((c) => c.name);
  if (containers.length) return containers;
  return [...new Set(n.gpu.procs.map((p) => p.name))];
}

/**
 * Where meters turn amber and red and where charts draw their dashed lines: the same
 * numbers the health engine alerts on (src-tauri/src/health/rules.rs), so a red bar
 * always has an alert behind it. GPU temperature warns at 85 °C (sustained) and is
 * marked critical at 90 °C on charts.
 */
export const LIMITS = {
  memPct: [90, 95],
  diskPct: [85, 95],
  gpuTempC: [85, 90],
  opticDbm: [-7, -10],
} as const;

/**
 * A node's GPU power limit in watts, or null when nvidia-smi reports none (a GB10 says
 * "[N/A]"). Never a guess: a missing limit draws no limit line and no "of N W" text.
 */
export const powerLimit = (n: NodeSnapshot): number | null => {
  const w = n.gpu.powerLimitW;
  return w != null && Number.isFinite(w) && w > 0 ? w : null;
};

/** A node whose readings are current: online and not stale. Totals count only these. */
export const isReporting = (n: NodeSnapshot) => n.status === "online" && !n.stale;

export interface ClusterTotals {
  /** Nodes whose readings went into the sums, and all nodes. */
  reporting: number;
  total: number;
  /** "3 of 4 reporting" when some aren't, else "". */
  reportingLabel: string;
  memUsed: number;
  /** Unified memory of the reporting nodes. */
  memTotal: number;
  powerW: number;
  /** Mean GPU power per reporting node, or null with none reporting. */
  powerAvgW: number | null;
  /** The sum of the limits, only when every reporting node reports one; else null. */
  powerLimitW: number | null;
  /** Mean GPU utilisation of the reporting nodes, or null. */
  gpuUtilAvg: number | null;
}

/**
 * Cluster sums over the nodes that are reporting, and how many those are (§2.4). An offline
 * node adds nothing, not its last reading, and its capacity leaves the "of" figures too.
 */
export function clusterTotals(snap: ClusterSnapshot): ClusterTotals {
  const rep = snap.nodes.filter(isReporting);
  const sum = (f: (n: NodeSnapshot) => number) => rep.reduce((a, n) => a + f(n), 0);
  const limits = rep.map(powerLimit);
  const powerW = sum((n) => n.gpu.powerW);
  return {
    reporting: rep.length,
    total: snap.nodes.length,
    reportingLabel: rep.length < snap.nodes.length ? `${rep.length} of ${snap.nodes.length} reporting` : "",
    memUsed: sum((n) => n.mem.used),
    memTotal: sum((n) => n.mem.total),
    powerW,
    powerAvgW: rep.length ? powerW / rep.length : null,
    powerLimitW: rep.length && limits.every((l) => l != null) ? limits.reduce((a, l) => a + (l as number), 0) : null,
    gpuUtilAvg: rep.length ? sum((n) => n.gpu.utilPct) / rep.length : null,
  };
}

/** "cell1–cell3" for a run of numbered names, else "cell1, gx10-0000". */
export function nodeRange(ids: string[]): string {
  if (ids.length <= 1) return ids.join("");
  const m = ids.map((id) => /^(.*?)(\d+)$/.exec(id));
  const prefix = m[0]?.[1];
  const nums = m.map((x) => (x && x[1] === prefix ? Number(x[2]) : NaN));
  const consecutive = nums.every((n, i) => Number.isFinite(n) && (i === 0 || n === nums[i - 1] + 1));
  return consecutive && ids.length > 2 ? `${ids[0]}–${ids[ids.length - 1]}` : ids.join(", ");
}

/**
 * The GPU power tile's second line: "4 nodes · avg 50 W", "· of 560 W limit" only when every
 * reporting node reports a limit, and "avg 67 W · 3 of 4 reporting" while some aren't.
 */
export function powerSubLabel(t: ClusterTotals): string {
  if (!t.reporting) return "No node reporting";
  const avg = `avg ${Math.round(t.powerAvgW ?? 0)} W`;
  if (t.reportingLabel) return `${avg} · ${t.reportingLabel}`;
  const limit = t.powerLimitW != null ? ` · of ${Math.round(t.powerLimitW)} W limit` : "";
  return `${t.reporting} node${t.reporting === 1 ? "" : "s"} · ${avg}${limit}`;
}

// ---- Node colour describes the node (§2.2, §2.3)

/** What nodeTone and nodeNotes need of an alert (src/lib/health.ts `Alert`). */
export interface ToneAlert {
  severity: string;
  state: string;
  rule?: string;
  entity: { kind: string; id: string; node: string | null };
  mutedUntilMs?: number | null;
  ackedMs?: number | null;
  /** r3: a silence covers it, it's planned work, or it clears by itself soon. */
  silenceId?: string | null;
  planned?: string | null;
  recoversAtMs?: number | null;
}

/** Alerts about the node itself: its link, disks, GPU, interfaces, services, kernel and agent. */
const NODE_SCOPED = new Set(["node", "disk", "gpu", "interface", "net", "service", "kernel", "agent"]);

/** Firing and counted: not pending, muted, silenced, acknowledged, planned or recovering. */
function counts(a: ToneAlert, now: number): boolean {
  if (a.state !== "firing") return false;
  if (a.mutedUntilMs && a.mutedUntilMs > now) return false;
  if (a.silenceId || a.planned || a.ackedMs) return false;
  if (a.recoversAtMs && a.recoversAtMs > now) return false;
  return a.severity === "warn" || a.severity === "crit";
}

const onNode = (a: ToneAlert, nodeId: string) => a.entity.node === nodeId || (a.entity.kind === "node" && a.entity.id === nodeId);

/**
 * A node's colour, from the node alone (M11): its reachability, then the worst counted alert
 * whose entity is the node or something of the node's own (a disk, the GPU, an interface, a
 * unit, the kernel, the agent). An endpoint or container alert never colours a node: a model
 * that's down shows on its member nodes as a neutral note (nodeNotes) instead.
 */
export function nodeTone(
  nodeId: string,
  view: { alerts: readonly ToneAlert[] } | null | undefined,
  cluster: Pick<ClusterSnapshot, "nodes"> | null | undefined,
  now: number = Date.now(),
): "ok" | "warn" | "crit" | "offline" {
  const n = cluster?.nodes.find((x) => x.id === nodeId);
  if (n) {
    // SparkBox restarted it on purpose: planned, neutral, never offline-red.
    if (n.status !== "online" && !n.maintenance) return n.status === "connecting" ? "warn" : "offline";
    if (n.stale && !n.maintenance) return "warn";
  }
  const own = nodeAlerts(nodeId, view, now);
  return own.length ? (own[0].severity === "crit" ? "crit" : "warn") : "ok";
}

/** The counted alerts about the node itself (nodeTone's), critical first, then oldest first. */
export function nodeAlerts<A extends ToneAlert>(nodeId: string, view: { alerts: readonly A[] } | null | undefined, now: number = Date.now()): A[] {
  const sev = (a: A) => (a.severity === "crit" ? 0 : 1);
  return (view?.alerts ?? [])
    .filter((a) => NODE_SCOPED.has(a.entity.kind) && onNode(a, nodeId) && counts(a, now))
    .sort((a, b) => sev(a) - sev(b));
}

/** The rules behind a node metric the Dashboard draws (disk, memory, GPU temperature). */
const KNOWN_AREA: Record<"disk" | "mem" | "temp", RegExp> = { disk: /^disk-/, mem: /^mem-/, temp: /temp/ };

/**
 * A known issue on one of a node's metrics (§3.4): a firing alert for it that's silenced or
 * acknowledged, so it isn't counted and its cell is drawn muted (grey bar, bell-slash). Returns
 * the tooltip ("Known issue: silenced indefinitely"), or null when there's none.
 */
export function knownIssue(
  nodeId: string,
  area: "disk" | "mem" | "temp",
  view: { alerts: readonly (ToneAlert & { rule: string })[]; silences?: readonly { id: string; until: { kind: string; ms?: number } }[] } | null | undefined,
): string | null {
  const a = (view?.alerts ?? []).find((x) => x.state === "firing" && onNode(x, nodeId) && KNOWN_AREA[area].test(x.rule) && (x.silenceId || x.ackedMs));
  if (!a) return null;
  if (!a.silenceId) return "Known issue: acknowledged";
  const until = view?.silences?.find((x) => x.id === a.silenceId)?.until;
  if (!until || until.kind === "never") return "Known issue: silenced indefinitely";
  if (until.kind === "resolved") return "Known issue: silenced until it resolves";
  return `Known issue: silenced until ${new Date(until.ms ?? 0).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" })}`;
}

/** What nodeNotes needs of an endpoint (src/lib/endpoints.ts `Endpoint`). */
export interface NoteEndpoint {
  id: string;
  models: string[];
  nodes: string[];
  expectedNodes?: string[];
  health: string;
  healthReason?: string | null;
  /** For its name (endpointLabel): "Ollama on cell3", never "cell3-ollama-11434". */
  engine?: string;
  headNode?: string;
  containerNames?: string[];
  label?: string;
}

export interface NodeNote {
  /** "Serving llama-3.3-70b · down". */
  text: string;
  /** The model's state word ("down"), in its own tone; the rest of the note is neutral. */
  state: string;
  tone: "neutral" | "warn" | "crit";
  /** Opens the endpoint. */
  link: { kind: "endpoints"; params: Record<string, string> };
  reason: string | null;
}

const NOTE_STATE: Record<string, { word: string; tone: NodeNote["tone"] }> = {
  down: { word: "down", tone: "crit" },
  loading: { word: "loading", tone: "neutral" },
  stopped: { word: "stopped", tone: "neutral" },
};

/**
 * Neutral notes for every node a model spans while the model isn't Ready, one per model:
 * "Serving llama-3.3-70b · down". Every pipeline member gets the same note, the head node
 * no more than the others; the node's own colour stays its own (nodeTone).
 */
export function nodeNotes(nodeId: string, _view: unknown, endpoints: readonly NoteEndpoint[] | null | undefined): NodeNote[] {
  const out: NodeNote[] = [];
  for (const e of endpoints ?? []) {
    const members = e.expectedNodes?.length ? e.expectedNodes : e.nodes;
    if (!members.includes(nodeId)) continue;
    // Idle, gone, paused and stopped servers aren't serving: no note on their nodes (§3.2).
    if (!isServing(e as Parameters<typeof isServing>[0])) continue;
    const st = NOTE_STATE[e.health];
    if (!st) continue;
    const model = endpointLabel({ engine: "other", headNode: "", ...e } as Parameters<typeof endpointLabel>[0]);
    out.push({ text: `Serving ${model} · ${st.word}`, state: st.word, tone: st.tone, link: { kind: "endpoints", params: { id: e.id } }, reason: e.healthReason ?? null });
  }
  return out;
}

// ---- The Dashboard's problems box (§2.2: counts are incidents)

/** What problemView needs of an alert (src/lib/health.ts `Alert` plus the r3 incident fields). */
export interface ProblemAlert extends ToneAlert {
  id: string;
  summary: string;
  sinceMs?: number | null;
  firingSinceMs?: number | null;
  incidentId?: string | null;
  rootKey?: string | null;
}

export interface Incident<A extends ProblemAlert> {
  key: string;
  severity: "warn" | "crit";
  /** The worst, then oldest, alert: the incident's line. */
  lead: A;
  /** Every alert in it (the lead included). */
  members: A[];
}

export interface ProblemView<A extends ProblemAlert> {
  crit: Incident<A>[];
  warn: Incident<A>[];
  /** Clearing by themselves (not counted). */
  recovering: A[];
  /** Work SparkBox started (not counted, never paged). */
  planned: A[];
  /** Seen but not yet confirmed: "cell3 offline · confirming…" (not counted). */
  pending: A[];
  /** Silenced or acknowledged incidents: one quiet line. */
  known: number;
}

/**
 * The problems box in one pass: firing alerts grouped into incidents (by incident id, else
 * root key, else the alert itself) and split by worst severity; recovering, planned and
 * pending alerts listed apart and never counted; silenced, acknowledged and muted incidents
 * counted only as "known issues".
 */
export function problemView<A extends ProblemAlert>(alerts: readonly A[], now: number = Date.now()): ProblemView<A> {
  const out: ProblemView<A> = { crit: [], warn: [], recovering: [], planned: [], pending: [], known: 0 };
  const incidents = new Map<string, Incident<A>>();
  const known = new Set<string>();
  const keyOf = (a: A) => a.incidentId || a.rootKey || a.id;
  for (const a of alerts) {
    if (a.severity !== "warn" && a.severity !== "crit") continue;
    if (a.state === "pending") {
      out.pending.push(a);
      continue;
    }
    if (a.state !== "firing") continue;
    if (a.planned) {
      out.planned.push(a);
      continue;
    }
    if (a.silenceId || a.ackedMs || (a.mutedUntilMs && a.mutedUntilMs > now)) {
      known.add(keyOf(a));
      continue;
    }
    if (a.recoversAtMs && a.recoversAtMs > now) {
      out.recovering.push(a);
      continue;
    }
    const key = keyOf(a);
    const inc = incidents.get(key);
    const sev = a.severity;
    if (!inc) {
      incidents.set(key, { key, severity: sev, lead: a, members: [a] });
      continue;
    }
    inc.members.push(a);
    const older = (a.sinceMs ?? Infinity) < (inc.lead.sinceMs ?? Infinity);
    if ((sev === "crit" && inc.severity !== "crit") || (sev === inc.severity && older)) inc.lead = a;
    if (sev === "crit") inc.severity = "crit";
  }
  for (const inc of incidents.values()) (inc.severity === "crit" ? out.crit : out.warn).push(inc);
  // A key that also has a counted alert isn't "known".
  out.known = [...known].filter((k) => !incidents.has(k)).length;
  return out;
}

/** What modelsInMemory needs of an endpoint. */
export interface MemoryEndpoint {
  health: string;
  engine: string;
  models: string[];
  ollama?: { name: string }[] | null;
}

/**
 * Models loaded in memory and answering: a Ready vLLM (or other) server's models, and for
 * Ollama only the ones it has loaded now (not every model it has installed, m15).
 */
export function modelsInMemory(endpoints: readonly MemoryEndpoint[]): number {
  const names = new Set<string>();
  for (const e of endpoints) {
    if (e.health !== "ready") continue;
    const list = e.engine === "ollama" ? (e.ollama ?? []).map((m) => m.name) : e.models;
    for (const m of list) names.add(m);
  }
  return names.size;
}
