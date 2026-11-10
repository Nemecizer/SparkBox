// One endpoint over the last hour to 14 days: how fast it generated, how long the first
// token and the test tokens took, its queue, KV cache and preemptions, with its member
// nodes' GPUs and their fabric's RoCE retransmits on the same time axis, so "what was p95
// TTFT last night, and was the fabric retransmitting then?" is one look. Alerts and
// changes on the endpoint or its nodes are marked on the top chart (All cluster events adds
// the rest). It opens on the shortest range that holds all of its history. Time the Mac was
// asleep comes from the node agents' own history (merged by the history store).
import { useEffect, useMemo, useState } from "react";
import { ArrowLeft, History } from "lucide-react";
import { ChartGrid, TimeChart, type ChartEvent, type ChartSeries } from "../../components/TimeChart";
import { EmptyState, HelpTip, Segmented, Spacer, Switch, Toolbar, ToolButton } from "../../components/ui";
import { inTauri } from "../../lib/api";
import { endpointLabel, fmtLatency, type Endpoint } from "../../lib/endpoints";
import { healthApi, useHealth } from "../../lib/health";
import { queryHistory, type AnyMetric, type HistoryResult } from "../../lib/historyApi";
import { opsApi } from "../../lib/opsApi";
import { useCluster } from "../../lib/store";
import type { ClusterSnapshot } from "../../lib/types";
import { useWinState } from "../../wm/WindowManager";
import { fmtTps } from "./shared";

export type HistoryRange = "1h" | "24h" | "7d" | "14d";

export const RANGE_S: Record<HistoryRange, number> = { "1h": 3600, "24h": 86_400, "7d": 7 * 86_400, "14d": 14 * 86_400 };

const RANGES: HistoryRange[] = ["1h", "24h", "7d", "14d"];

/** The shortest range that holds all of the history (it starts at `startsAtMs`); 14 d when it's older or unknown. */
export function defaultRange(startsAtMs: number | null | undefined, nowMs: number): HistoryRange {
  if (startsAtMs == null) return "14d";
  return RANGES.find((r) => nowMs - RANGE_S[r] * 1000 <= startsAtMs) ?? "14d";
}

/** Points per chart: about one per 2 px of a wide window, and never finer than the store keeps. */
const MAX_POINTS = 360;

/** The serving series, then the one test tokens keep (asked apart: an older store may not know it). */
export const SERVING: AnyMetric[] = ["genTps", "promptTps", "ttftP50", "ttftP95", "waiting", "kvPct", "preemptions"];
export const CANARY: AnyMetric = "canaryMs";

export interface HistoryQuery {
  /** What a chart row is called, and which query it reads. */
  key: string;
  entity: string;
  metrics: AnyMetric[];
  fromMs: number;
  toMs: number;
  maxPoints: number;
}

/** The fabric interfaces with RoCE counters on each of `nodes`, from the snapshot. */
export function roceIfaces(snap: ClusterSnapshot | null, nodes: string[]): { node: string; iface: string }[] {
  return nodes.flatMap((n) => (snap?.nodes.find((x) => x.id === n)?.nets ?? []).filter((x) => x.role === "fabric" && x.roce).map((x) => ({ node: n, iface: x.iface })));
}

/** Every history query the view runs for a range ending at `nowMs`: the endpoint, its nodes' GPUs, their RoCE retransmits. */
export function historyQueries(e: Pick<Endpoint, "id" | "nodes">, ifaces: { node: string; iface: string }[], range: HistoryRange, nowMs: number): HistoryQuery[] {
  const toMs = nowMs;
  const fromMs = nowMs - RANGE_S[range] * 1000;
  const q = (key: string, entity: string, metrics: AnyMetric[]): HistoryQuery => ({ key, entity, metrics, fromMs, toMs, maxPoints: MAX_POINTS });
  return [
    q("serving", `endpoint:${e.id}`, SERVING),
    q("canary", `endpoint:${e.id}`, [CANARY]),
    ...e.nodes.map((n) => q(`gpu:${n}`, `node:${n}`, ["gpu"])),
    ...ifaces.map((i) => q(`oos:${i.node}/${i.iface}`, `net:${i.node}/${i.iface}`, ["oos"])),
  ];
}

interface Loaded {
  results: Map<string, HistoryResult>;
  /** Queries the store refused (e.g. canaryMs before it kept it), by key. */
  refused: Map<string, string>;
  at: number;
}

/** Runs the queries, again every 1/60 of the range (at least every 10 s). */
function useQueries(queries: HistoryQuery[], key: string, refresh: number): Loaded | null {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  useEffect(() => {
    if (!inTauri) return;
    let alive = true;
    const run = async () => {
      const results = new Map<string, HistoryResult>();
      const refused = new Map<string, string>();
      const now = Date.now();
      const span = queries[0] ? queries[0].toMs - queries[0].fromMs : 0;
      await Promise.all(
        queries.map((q) =>
          queryHistory(q.entity, q.metrics, now - span, now, q.maxPoints).then(
            (r) => results.set(q.key, r),
            (err) => refused.set(q.key, String(err)),
          ),
        ),
      );
      if (alive) setLoaded({ results, refused, at: now });
    };
    void run();
    const t = window.setInterval(run, refresh);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
    // `key` stands for the queries' entities and metrics.
  }, [key, refresh]);
  return loaded;
}

/** Alerts about the endpoint or its nodes, and changes made on them, inside the range (`all`: every alert and change on the cluster). */
function useEndpointEvents(e: Endpoint, rangeS: number, all: boolean): ChartEvent[] {
  const [events, setEvents] = useState<(ChartEvent & { alertId?: string })[]>([]);
  const nodes = e.nodes.join(",");
  useEffect(() => {
    let alive = true;
    const load = async () => {
      const from = Date.now() - rangeS * 1000;
      const [alerts, changes] = await Promise.all([healthApi.history(500).catch(() => []), opsApi.activitySince(from, 500).catch(() => [])]);
      if (!alive) return;
      const mine = new Set(nodes.split(","));
      const out: (ChartEvent & { alertId?: string })[] = [];
      for (const a of Array.isArray(alerts) ? alerts : []) {
        if (a.tsMs < from || (a.kind !== "raised" && a.kind !== "escalated")) continue;
        const ent = a.alert.entity;
        if (!all && ent.id !== e.id && !(ent.node && mine.has(ent.node))) continue;
        out.push({ ts: a.tsMs / 1000, kind: "alert", label: a.alert.summary, severity: a.alert.severity === "crit" ? "crit" : "warn", alertId: a.alert.id });
      }
      const ops = new Set<string>();
      for (const c of Array.isArray(changes) ? changes : []) {
        if (!c.changes || c.tsMs < from || (!all && !c.node.split(/,\s*/).some((n) => mine.has(n)))) continue;
        const k = c.opId ?? c.id;
        if (ops.has(k)) continue;
        ops.add(k);
        out.push({ ts: c.tsMs / 1000, kind: "change", label: c.title || c.command });
      }
      setEvents((prev) => (JSON.stringify(prev) === JSON.stringify(out) ? prev : out));
    };
    void load();
    const t = window.setInterval(load, 30_000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [e.id, nodes, rangeS, all]);
  const health = useHealth();
  const firing = (health?.alerts ?? []).filter((a) => a.state === "firing").map((a) => a.id).sort().join("|");
  return useMemo(() => {
    const ids = new Set(firing.split("|"));
    return events.map(({ alertId, ...ev }) => (ev.kind === "alert" ? { ...ev, active: !!alertId && ids.has(alertId) } : ev));
  }, [events, firing]);
}

/** One query's values on `ts` (unix s), null where it has none. */
function on(ts: number[], r: HistoryResult | undefined, metric: string): (number | null)[] {
  if (!r) return ts.map(() => null);
  const at = new Map(r.ts.map((t, i) => [Math.round(t / 1000), r.series[metric]?.[i] ?? null]));
  return ts.map((t) => at.get(t) ?? null);
}

/** Several queries' values added up on `ts` (a node's interfaces); null where none has a value. */
function summed(ts: number[], rs: (HistoryResult | undefined)[], metric: string): (number | null)[] {
  const each = rs.map((r) => on(ts, r, metric));
  return ts.map((_, i) => {
    const vals = each.map((v) => v[i]).filter((v): v is number => v != null);
    return vals.length ? vals.reduce((a, b) => a + b, 0) : null;
  });
}

/** A counter's growth per point (a drop is a restart: what it counted since), null where unknown. */
export function increases(v: (number | null)[]): (number | null)[] {
  let last: number | null = null;
  return v.map((x) => {
    if (x == null) return null;
    const d = last == null ? null : x >= last ? x - last : x;
    last = x;
    return d;
  });
}

/** A count axis that starts at a readable 0–4 when nothing larger happened (not five "0.0" ticks). */
function countTop(v: (number | null)[]): number | undefined {
  const max = v.reduce<number>((m, x) => (x != null && x > m ? x : m), 0);
  return max < 4 ? 4 : undefined;
}

const fmtCount = (v: number) => (v >= 10 ? v.toFixed(0) : v.toFixed(1));
const fmtRateEvents = (v: number) => `${v >= 10 ? v.toFixed(0) : v >= 1 ? v.toFixed(1) : v.toFixed(2)}/s`;

/** The range chosen, else the shortest one that holds the endpoint's whole history (read once). */
function useRange(e: Endpoint): [HistoryRange | null, (r: HistoryRange) => void] {
  const [picked, setPicked] = useWinState<HistoryRange | null>(`history.range:${e.id}`, null);
  const [auto, setAuto] = useState<HistoryRange | null>(inTauri ? null : "1h");
  useEffect(() => {
    if (picked || !inTauri) return;
    let alive = true;
    const now = Date.now();
    queryHistory(`endpoint:${e.id}`, ["genTps"], now - RANGE_S["14d"] * 1000, now, 60).then(
      (r) => alive && setAuto(defaultRange(r.ts.length ? (r.startsAtMs ?? r.ts[0]) : now, now)),
      () => alive && setAuto("1h"),
    );
    return () => {
      alive = false;
    };
  }, [e.id, picked]);
  return [picked ?? auto, setPicked];
}

export function EndpointHistory({ endpoint: e, onBack }: { endpoint: Endpoint; onBack: () => void }) {
  const [chosen, setRange] = useRange(e);
  const range = chosen ?? "1h";
  const [allEvents, setAllEvents] = useWinState("history.allEvents", false);
  const snap = useCluster();
  const ifaces = useMemo(() => roceIfaces(snap, e.nodes), [snap?.nodes.length, e.nodes.join(",")]);
  const rangeS = RANGE_S[range];
  const queries = useMemo(() => historyQueries(e, ifaces, range, Date.now()), [e.id, e.nodes.join(","), ifaces.map((i) => i.iface).join(","), range]);
  const loaded = useQueries(queries, `${e.id}|${range}|${queries.map((q) => q.key).join(",")}`, Math.max(10_000, (rangeS * 1000) / 60));
  const events = useEndpointEvents(e, rangeS, allEvents);
  const name = endpointLabel(e);
  const syncKey = `endpoint-history:${e.id}`;

  const view = useMemo(() => {
    if (!loaded) return null;
    const serving = loaded.results.get("serving");
    const ts = (serving?.ts ?? loaded.results.values().next().value?.ts ?? []).map((t) => Math.round(t / 1000));
    const to = loaded.at / 1000;
    const from = to - rangeS;
    // "History starts" only for a real stretch before anything was recorded, not a bucket's rounding.
    const startsMs = serving?.startsAtMs ?? null;
    const startsAt = startsMs != null && startsMs / 1000 > from + rangeS * 0.02 ? startsMs / 1000 : null;
    const backfilled = (serving?.agentRanges ?? []).map(([a, b]) => [a / 1000, b / 1000] as [number, number]);
    const s = (m: string) => on(ts, serving, m);
    const gpu: ChartSeries[] = e.nodes.map((n) => ({ label: n, color: "--m-gpu", values: on(ts, loaded.results.get(`gpu:${n}`), "gpu") }));
    const oos: ChartSeries[] = e.nodes
      .filter((n) => ifaces.some((i) => i.node === n))
      .map((n) => ({ label: n, color: "--m-rdma", values: summed(ts, ifaces.filter((i) => i.node === n).map((i) => loaded.results.get(`oos:${i.node}/${i.iface}`)), "oos") }));
    const canary = loaded.results.get("canary");
    const canaryValues = canary ? on(ts, canary, "canaryMs") : null;
    return {
      ts, range: [from, to] as [number, number], startsAt, backfilled,
      gen: s("genTps"), p50: s("ttftP50"), p95: s("ttftP95"), waiting: s("waiting"), kv: s("kvPct"), pre: increases(s("preemptions")),
      canary: canaryValues && canaryValues.some((v) => v != null) ? canaryValues : null,
      gpu, oos, empty: !serving || serving.ts.every((_, i) => serving.series.genTps?.[i] == null && serving.series.kvPct?.[i] == null),
    };
  }, [loaded, rangeS, e.nodes.join(","), ifaces.length]);

  // Events mark the top chart only; the charts under it share its time axis.
  const common = { range: view?.range, syncKey, events: [] as ChartEvent[], startsAt: view?.startsAt ?? null, backfilled: view?.backfilled, height: 130 };
  const kvKnown = e.engine !== "ollama" && e.engine !== "llamacpp";

  return (
    <div className="col fill ai-history">
      <Toolbar label="History">
        <ToolButton icon={ArrowLeft} label="Endpoints" onClick={onBack} />
        <span className="toolbar-text strong">{name}</span>
        <span className="toolbar-text muted">{e.nodes.join(", ")}</span>
        <HelpTip topic="ai.card.history" label="About this history" />
        <Spacer />
        <Switch checked={allEvents} onChange={setAllEvents} label="All cluster events" showLabel
          description="Mark every alert and change on the cluster, not only those on this endpoint and its nodes" />
        <Segmented<HistoryRange>
          label="Range"
          value={range}
          onChange={setRange}
          options={[{ value: "1h", label: "1 h" }, { value: "24h", label: "24 h" }, { value: "7d", label: "7 d" }, { value: "14d", label: "14 d" }]}
        />
      </Toolbar>
      <div className="ai-history-body">
        {!inTauri ? (
          <EmptyState icon={History} title="History needs the app" body="The history store runs in SparkBox itself." />
        ) : !view || !chosen ? (
          <div className="muted pad">Reading {name}'s history…</div>
        ) : (
          <ChartGrid syncKey={syncKey}>
            {view.empty && <div className="ai-history-none muted">No serving numbers recorded for {name} in this range yet: they start once it's Ready.</div>}
            <TimeChart {...common} events={events} title="Generation" subtitle="Tokens generated per second, all requests" fmt={(v) => `${fmtTps(v)} tok/s`} ts={view.ts}
              series={[{ label: "Generated", color: "--m-gpu", values: view.gen }]} />
            <TimeChart {...common} title="Time to first token" subtitle="p50 and p95 over each minute" fmt={(v) => fmtLatency(v)} ts={view.ts} robust="auto"
              series={[{ label: "p50", color: "--m-ttft", values: view.p50, noFill: true }, { label: "p95", color: "--m-ttft", values: view.p95, noFill: true, dash: true }]} />
            {/* Right under latency: was the fabric retransmitting then, and were its GPUs busy? */}
            {view.oos.length > 0 ? (
              <TimeChart {...common} title="RoCE retransmits" subtitle="Out-of-sequence packets per second on its nodes' fabric" mode="multiples" robust="auto" fmt={fmtRateEvents} ts={view.ts} series={view.oos} />
            ) : (
              <div className="ai-history-none muted">Its nodes report no RoCE counters, so there's no retransmit lane.</div>
            )}
            <TimeChart {...common} title="GPU utilization" subtitle="Its nodes" mode="multiples" yMax={100} fmt={(v) => `${v.toFixed(0)}%`} ts={view.ts} series={view.gpu} />
            {view.canary ? (
              <TimeChart {...common} title="Test token" subtitle="One token, request to reply" fmt={(v) => fmtLatency(v)} ts={view.ts} robust="auto"
                series={[{ label: "1 token", color: "--m-ttft", values: view.canary, noFill: true }]} />
            ) : (
              <div className="ai-history-none muted">
                No test-token times yet{e.health === "idle" ? ": nothing is in memory to test" : e.canary?.ok && e.canary.latencyMs != null ? `; the last one took ${fmtLatency(e.canary.latencyMs)}` : ""}.
              </div>
            )}
            <TimeChart {...common} title="Waiting requests" subtitle="Queued for a slot or KV-cache room" fmt={fmtCount} ts={view.ts} yMax={countTop(view.waiting)}
              series={[{ label: "Waiting", color: "--m-tx", values: view.waiting }]} />
            {kvKnown && (
              <TimeChart {...common} title="KV cache" subtitle="Share in use" yMax={100} fmt={(v) => `${v.toFixed(0)}%`} ts={view.ts}
                series={[{ label: "KV cache", color: "--m-mem", values: view.kv }]} />
            )}
            {kvKnown && (
              <TimeChart {...common} title="Preemptions" subtitle="Requests pushed out of the KV cache" fmt={fmtCount} ts={view.ts} yMax={countTop(view.pre)}
                series={[{ label: "Preempted", color: "--m-power", values: view.pre }]} />
            )}
          </ChartGrid>
        )}
      </div>
    </div>
  );
}
