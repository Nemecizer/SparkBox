import { useEffect, useMemo, useState } from "react";
import { Pause, Undo2 } from "lucide-react";
import { Icon } from "../components/Icon";
import { ChartGrid, TimeChart } from "../components/TimeChart";
import { HelpTip, SectionTitle, Segmented, Spacer, Toolbar, Waiting } from "../components/ui";
import { endpointLabel, useEndpoints, fmtLatency } from "../lib/endpoints";
import { hoverTime, RANGES, rangeSeconds, timeLabel, usesStore, type RangeId } from "../lib/chartScale";
import { fmtBits, fmtByteRate, GiB } from "../lib/format";
import { inTauri } from "../lib/api";
import { queryHistory, type EndpointMetric, type HistoryMetric } from "../lib/historyApi";
import { LIMITS } from "../lib/metrics";
import { alignTo, CLUSTER, getHistory, historyStart, useCluster, type SeriesKey } from "../lib/store";
import { seqColor, useDarkMode } from "../lib/theme";
import { useWinState } from "../wm/WindowManager";
import "./monitoring.css";

interface Panel {
  key: SeriesKey;
  title: string;
  fmt: (v: number) => string;
  yMax?: number;
  unit?: "bytes";
  softMin?: number;
  thresholds?: { value: number; label: string; tone: "warn" | "crit" }[];
  /** Bursty: the shared domain tops out at p98 × 1.15 instead of the one highest reading. */
  bursty?: boolean;
  /** Each row's value is its mean over this many seconds: a counter rate is 0 most seconds when idle. */
  meanS?: number;
}

const PANELS: Panel[] = [
  { key: "gpu", title: "GPU utilization", fmt: (v) => `${v.toFixed(0)}%`, yMax: 100 },
  { key: "cpu", title: "CPU", fmt: (v) => `${v.toFixed(0)}%`, yMax: 100 },
  { key: "memUsed", title: "Unified memory used", fmt: (v) => `${(v / GiB).toFixed(0)} GiB`, unit: "bytes" },
  { key: "power", title: "GPU power", fmt: (v) => `${v.toFixed(0)} W`, bursty: true },
  {
    key: "gpuTemp", title: "GPU temperature", fmt: (v) => `${v.toFixed(0)} °C`, softMin: 20,
    thresholds: [{ value: LIMITS.gpuTempC[0], label: `${LIMITS.gpuTempC[0]} °C`, tone: "warn" }],
  },
  { key: "fabricRx", title: "Fabric received", fmt: fmtBits, bursty: true, meanS: 10 },
  { key: "fabricTx", title: "Fabric sent", fmt: fmtBits, bursty: true, meanS: 10 },
  { key: "diskWrite", title: "Root disk writes", fmt: fmtByteRate, bursty: true },
];

/** Each panel draws in its metric's own hue: one metric per chart, one row per node. */
const COLOR: Partial<Record<SeriesKey, string>> = { gpu: "gpu", cpu: "cpu", memUsed: "mem", power: "power", gpuTemp: "temp", fabricRx: "rx", fabricTx: "tx", diskWrite: "write" };

type Mode = "lines" | "heatmap";
type Data = { ts: number[]; byNode: Record<string, Partial<Record<SeriesKey, (number | null)[]>>>; startsAt: number | null };

/** Every node's series for the range: the in-memory hour, or the history store from 6 h. */
function useClusterData(ids: string[], rangeS: number, version: number): Data | null {
  const stored = usesStore(rangeS);
  const [fromStore, setFromStore] = useState<Data | null>(null);
  const idKey = ids.join(",");
  useEffect(() => {
    if (!stored || !inTauri) {
      setFromStore(null);
      return;
    }
    let alive = true;
    const load = async () => {
      const now = Date.now();
      const keys = PANELS.map((p) => p.key) as HistoryMetric[];
      const results = await Promise.all(idKey.split(",").map((id) => queryHistory(`node:${id}`, keys, now - rangeS * 1000, now, 480).catch(() => null)));
      if (!alive) return;
      const first = results.find((r) => r && r.ts.length);
      const ts = first ? first.ts.map((t) => t / 1000) : [];
      const byNode: Data["byNode"] = {};
      idKey.split(",").forEach((id, i) => {
        const r = results[i];
        const at = new Map<number, number>();
        r?.ts.forEach((t, j) => at.set(t / 1000, j));
        byNode[id] = Object.fromEntries(keys.map((k) => [k, ts.map((t) => (at.has(t) ? (r!.series[k]?.[at.get(t)!] ?? null) : null))]));
      });
      const starts = results.map((r) => r?.startsAtMs).filter((v): v is number => v != null);
      setFromStore({ ts, byNode, startsAt: starts.length ? Math.min(...starts) / 1000 : null });
    };
    void load();
    const t = window.setInterval(load, Math.max(15_000, (rangeS * 1000) / 60));
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [stored, idKey, rangeS]);

  return useMemo(() => {
    if (stored) return fromStore;
    const h = getHistory(CLUSTER, rangeS);
    if (!h) return null;
    const byNode: Data["byNode"] = {};
    for (const id of ids) byNode[id] = Object.fromEntries(PANELS.map((p) => [p.key, alignTo(h.ts, id, p.key, rangeS)]));
    // History from before this run: the minutes before its first reading read "SparkBox was
    // closed" (recordedFrom), not "History starts" at this run's first second.
    const recorded = historyStart();
    const fresh = h.ts.length && h.ts[0] > h.ts[h.ts.length - 1] - rangeS + 5;
    return { ts: h.ts, byNode, startsAt: fresh && !(recorded != null && recorded < h.ts[0] - 5) ? h.ts[0] : null };
    // `version` moves with every recorded second.
  }, [stored, fromStore, idKey, rangeS, version]);
}

// ---- Serving: every model's speed, latency, queue and cache on the same crosshair

const SERVING: { key: EndpointMetric; title: string; fmt: (v: number) => string; yMax?: number }[] = [
  { key: "genTps", title: "Generation", fmt: (v) => `${v.toFixed(v >= 10 ? 0 : 1)} tok/s` },
  { key: "ttftP95", title: "Time to first token, p95", fmt: (v) => fmtLatency(v) },
  { key: "waiting", title: "Requests waiting", fmt: (v) => v.toFixed(0) },
  { key: "kvPct", title: "KV cache", fmt: (v) => `${v.toFixed(0)}%`, yMax: 100 },
];
// Endpoints are identities, not metrics, so each wears a node-identity hue by its index (as bench runs
// do), never a --m-* token: the indigo that is TTFT on an endpoint's card must not be Ollama here. The
// first four validate every pair under colour blindness; the lines of one panel share a legend.
const SERVING_COLORS = ["--node-1", "--node-2", "--node-3", "--node-4", "--node-5", "--node-6"];

type ServingData = { ts: number[]; byEndpoint: Record<string, Partial<Record<EndpointMetric, (number | null)[]>>> };

/** The history store's serving series of each endpoint for the range, on one time axis. */
function useServingData(ids: string[], rangeS: number): ServingData | null {
  const [data, setData] = useState<ServingData | null>(null);
  const idKey = ids.join(",");
  useEffect(() => {
    if (!inTauri || !idKey) {
      setData(null);
      return;
    }
    let alive = true;
    const load = async () => {
      const now = Date.now();
      const keys = SERVING.map((p) => p.key);
      const list = idKey.split(",");
      const results = await Promise.all(list.map((id) => queryHistory(`endpoint:${id}`, keys, now - rangeS * 1000, now, 480).catch(() => null)));
      if (!alive) return;
      // One axis: every bucket any endpoint has.
      const ts = [...new Set(results.flatMap((r) => r?.ts ?? []))].sort((a, b) => a - b);
      const byEndpoint: ServingData["byEndpoint"] = {};
      list.forEach((id, i) => {
        const r = results[i];
        const at = new Map<number, number>();
        r?.ts.forEach((t, j) => at.set(t, j));
        byEndpoint[id] = Object.fromEntries(keys.map((k) => [k, ts.map((t) => (at.has(t) ? (r!.series[k]?.[at.get(t)!] ?? null) : null))]));
      });
      setData({ ts: ts.map((t) => t / 1000), byEndpoint });
    };
    void load();
    const t = window.setInterval(load, Math.max(10_000, (rangeS * 1000) / 60));
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [idKey, rangeS]);
  return data;
}

/** GPU utilisation as a grid: one row per node, one cell per time bucket, darker = busier. */
function Heatmap({ data, nodes, range }: { data: Data; nodes: { id: string; name: string }[]; range: [number, number] }) {
  const dark = useDarkMode();
  const [hover, setHover] = useState<{ node: string; t: number; v: number | null } | null>(null);
  const cols = 120;
  const [from, to] = range;
  const step = (to - from) / cols;
  const cells = nodes.map((n) => {
    const vals = data.byNode[n.id]?.gpu ?? [];
    const sums = new Array<number>(cols).fill(0);
    const counts = new Array<number>(cols).fill(0);
    data.ts.forEach((t, i) => {
      const v = vals[i];
      const c = Math.floor((t - from) / step);
      if (v == null || c < 0 || c >= cols) return;
      sums[c] += v;
      counts[c]++;
    });
    return sums.map((s, i) => (counts[i] ? s / counts[i] : null));
  });
  const labelW = 90;
  const rowH = 26;
  const w = 960;
  const cw = (w - labelW) / cols;
  const ticks = [0, 0.25, 0.5, 0.75, 1].map((f) => from + f * (to - from));
  return (
    <div className="mon-heat">
      <div className="tc-head">
        <div className="tc-title"><span className="tc-name">GPU utilization by node</span><span className="tc-sub">Each cell is the average over {Math.round(step)} s; a hatched cell had no readings</span></div>
        <div className="tc-values">
          {hover ? (
            <span className="tc-at">{hover.node} · {hoverTime(hover.t, to - from)} · <b className="num">{hover.v == null ? "no data" : `${hover.v.toFixed(0)}%`}</b></span>
          ) : (
            <span className="mon-heat-scale" aria-hidden>0%<i className={dark ? "dark" : ""} />100%</span>
          )}
        </div>
      </div>
      <svg viewBox={`0 0 ${w} ${nodes.length * rowH + 22}`} width="100%" fontSize={11} role="img" aria-label="GPU utilization heatmap" onMouseLeave={() => setHover(null)}>
        <defs>
          {/* A cell with no readings is hatched, so it is never taken for a 0% cell (which is a solid near-surface blue). */}
          <pattern id="mon-heat-nodata" width="4" height="4" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
            <line x1="0" y1="0" x2="0" y2="4" className="mon-heat-nodata" />
          </pattern>
        </defs>
        {nodes.map((n, r) => (
          <g key={n.id}>
            <text x={0} y={r * rowH + 17} className="mon-heat-label">{n.name}</text>
            {cells[r].map((v, c) => (
              <rect key={c} x={labelW + c * cw} y={r * rowH + 2} width={Math.max(0.5, cw - 0.6)} height={rowH - 4} rx={1.5}
                style={{ fill: v == null ? "url(#mon-heat-nodata)" : seqColor(v, dark).bg }}
                onMouseEnter={() => setHover({ node: n.name, t: from + (c + 0.5) * step, v })} />
            ))}
          </g>
        ))}
        {ticks.map((t, i) => (
          <text key={t} x={labelW + ((t - from) / (to - from)) * (w - labelW)} y={nodes.length * rowH + 16} textAnchor={i === 0 ? "start" : i === ticks.length - 1 ? "end" : "middle"} className="mon-heat-label muted">
            {timeLabel(t, to - from > 3 * 86400 ? 86400 : to - from > 7200 ? 3600 : 60, i === 0)}
          </text>
        ))}
      </svg>
    </div>
  );
}

export function ClusterMetrics() {
  const cluster = useCluster();
  const [range, setRange] = useWinState<RangeId>("range", "1h");
  const [mode, setMode] = useWinState<Mode>("mode", "lines");
  const [zoom, setZoom] = useState<[number, number] | null>(null);
  const rangeS = rangeSeconds(range);
  const ids = cluster?.nodes.map((n) => n.id) ?? [];
  const data = useClusterData(ids, rangeS, cluster?.tsMs ?? 0);
  const endpoints = useEndpoints().filter((e) => e.health === "ready" || e.health === "loading");
  const serving = useServingData(endpoints.map((e) => e.id), rangeS);
  if (!cluster) return <Waiting />;
  const now = cluster.tsMs / 1000;
  const span: [number, number] = [now - rangeS, now];

  return (
    <div className="col fill mon-cluster">
      <Toolbar label="Cluster metrics">
        <Segmented<RangeId> label="Time range" value={range} onChange={(r) => { setRange(r); setZoom(null); }} options={RANGES.map((r) => ({ value: r.id, label: r.label }))} />
        <Segmented<Mode> label="View" value={mode} onChange={setMode} options={[{ value: "lines", label: "Charts" }, { value: "heatmap", label: "GPU heatmap" }]} />
        <HelpTip topic="mon.metrics.views" />
        {zoom && (
          <>
            <span className="mon-paused"><Icon icon={Pause} size={12} />Paused</span>
            <button type="button" className="btn small" onClick={() => setZoom(null)}><Icon icon={Undo2} size={12} />Back to live</button>
          </>
        )}
        <Spacer />
      </Toolbar>
      <div className="mon-node-scroll">
        {!data ? (
          <Waiting label="Loading history…" />
        ) : mode === "heatmap" ? (
          <div className="mon-node-body">
            <Heatmap data={data} nodes={cluster.nodes} range={span} />
          </div>
        ) : (
          <div className="mon-node-body">
            <ChartGrid>
            <div className="mon-chart-grid">
              {PANELS.map((p) => (
                <TimeChart
                  key={p.key}
                  title={p.title}
                  ts={data.ts}
                  range={span}
                  zoom={zoom}
                  onZoom={setZoom}
                  relative={rangeS <= 900}
                  startsAt={data.startsAt}
                  recordedFrom={historyStart()}
                  fmt={p.fmt}
                  yMax={p.yMax}
                  unit={p.unit}
                  softMin={p.softMin}
                  thresholds={p.thresholds}
                  mode="multiples"
                  robust={p.bursty ? "auto" : undefined}
                  valueMeanS={p.meanS}
                  subtitle={p.meanS ? `Last ${p.meanS} s mean` : undefined}
                  offline={cluster.nodes.filter((n) => n.status !== "online" || n.stale).map((n) => n.name)}
                  series={cluster.nodes.map((n) => ({ label: n.name, color: `--m-${COLOR[p.key]}`, values: data.byNode[n.id]?.[p.key] ?? [] }))}
                />
              ))}
            </div>
            {endpoints.length > 0 && serving && (
              <>
                <SectionTitle title="Serving" infoTopic="mon.metrics.serving" />
                <div className="mon-chart-grid">
                  {SERVING.map((p) => (
                    <TimeChart
                      key={p.key}
                      title={p.title}
                      ts={serving.ts}
                      range={span}
                      zoom={zoom}
                      onZoom={setZoom}
                      relative={rangeS <= 900}
                      fmt={p.fmt}
                      yMax={p.yMax}
                      robust={p.key === "ttftP95" || p.key === "waiting" ? "auto" : undefined}
                      height={150}
                      series={endpoints.map((e, i) => ({ label: endpointLabel(e), color: SERVING_COLORS[i % SERVING_COLORS.length], values: serving.byEndpoint[e.id]?.[p.key] ?? [] }))}
                    />
                  ))}
                </div>
              </>
            )}
            </ChartGrid>
          </div>
        )}
      </div>
    </div>
  );
}
