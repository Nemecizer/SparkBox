import { Table, type Column } from "../../components/Table";
import { TimeChart } from "../../components/TimeChart";
import { Badge, SectionTitle, Tile, Tiles } from "../../components/ui";
import { fmtBytes } from "../../lib/format";
import { gpuError, LIMITS, powerLimit } from "../../lib/metrics";
import { throttleVerdict } from "../../lib/nodeStatus";
import { recent } from "../../lib/store";
import type { GpuProc, GpuStats, NodeSnapshot } from "../../lib/types";
import { useNodeSeries } from "./series";
import type { ChartProps } from "./shared";
import { pctFmt, tempFmt, wattFmt } from "./Overview";

const KEYS = ["gpu", "smClock", "power", "gpuTemp"] as const;

export const gpuProcColumns: Column<GpuProc>[] = [
  { key: "pid", label: "PID", align: "right", fit: "content", sort: (p) => p.pid, render: (p) => <span className="num">{p.pid}</span> },
  { key: "name", label: "Process", priority: 1, fit: "flex", sort: (p) => p.name, render: (p) => p.name },
  { key: "container", label: "Container", priority: 2, fit: "flex", sort: (p) => p.container ?? "", render: (p) => p.container ?? <span className="muted">Not in a container</span> },
  { key: "mem", label: "GPU memory", short: "Memory", align: "right", fit: "content", priority: 1, sort: (p) => p.mem, render: (p) => <span className="num">{fmtBytes(p.mem)}</span> },
];

/**
 * The GPU's clocks in words, by the same rule as the gpu-throttle alert (throttleVerdict): an
 * idle GB10 at P8 reads "Idle", whatever reason nvidia-smi gives for its low clocks.
 */
export function ThrottleBadge({ gpu }: { gpu: GpuStats }) {
  const v = throttleVerdict(gpu);
  if (v.throttled) return <Badge tone="warn" title={v.alertText ?? v.text}>{v.text}</Badge>;
  const reported = (gpu.throttle ?? []).length ? `nvidia-smi reports: ${(gpu.throttle ?? []).join(", ")}` : undefined;
  return <Badge tone="neutral" title={reported}>{v.text}</Badge>;
}

export function GpuTab({ node, chart }: { node: NodeSnapshot; chart: ChartProps }) {
  const g = node.gpu;
  const s = useNodeSeries(node.id, chart.rangeS, [...KEYS], chart.nowS);
  const limit = powerLimit(node);
  const maxClock = g.clockMaxMhz ?? null;
  const common = { ts: s.ts, range: s.range, zoom: chart.zoom, onZoom: chart.onZoom, startsAt: s.startsAt, recordedFrom: s.recordedFrom, relative: chart.rangeS <= 900, height: 170 };
  // Unknown while the GPU can't be read: "—", not an idle 0.
  const why = gpuError(node);
  const reporting = node.status === "online" && !node.stale && !why;
  return (
    <div className="mon-node-body">
      <div className="mon-gpu-head">
        {reporting && <ThrottleBadge gpu={g} />}
        {g.pstate && !throttleVerdict(g).idle && (
          <span className={why ? "muted row-dim" : "muted"} title={why ? "The last state read before the GPU stopped answering" : undefined}>Performance state {g.pstate}{why ? " (last reading)" : ""}</span>
        )}
        {(g.xidErrors ?? 0) > 0 && <Badge tone="crit">{g.xidErrors} Xid error{g.xidErrors === 1 ? "" : "s"}</Badge>}
      </div>
      {/* The SM clock wears the GPU family's light tone (--m-gpu-clock), so its tile and chart are not a second green beside Utilization. */}
      <Tiles>
        <Tile label="Utilization" value={reporting ? g.utilPct.toFixed(0) : "—"} unit={reporting ? "%" : undefined} sub={why ?? undefined} trend={recent(node.id, "gpu", 120)} trendMax={100} color="var(--m-gpu)" />
        <Tile label="SM clock" value={reporting ? g.smClockMhz : "—"} unit={reporting ? "MHz" : undefined} sub={maxClock ? `of ${maxClock} MHz` : "Max not reported"}
          trend={recent(node.id, "smClock", 120)} trendMax={maxClock ?? undefined} color="var(--m-gpu-clock)" />
        <Tile label="Power" value={reporting ? g.powerW.toFixed(0) : "—"} unit={reporting ? "W" : undefined} sub={limit != null ? `Limit ${limit.toFixed(0)} W` : "No limit reported"}
          trend={recent(node.id, "power", 120)} trendMax={limit ?? undefined} color="var(--m-power)" />
        <Tile label="Temperature" value={reporting ? g.tempC.toFixed(0) : "—"} unit={reporting ? "°C" : undefined}
          sub={`Warns above ${LIMITS.gpuTempC[0]} °C`} tone={reporting && g.tempC >= LIMITS.gpuTempC[0] ? "warn" : undefined}
          trend={recent(node.id, "gpuTemp", 120)} trendMax={100} color="var(--m-temp)" />
      </Tiles>
      <div className="mon-chart-grid">
        <TimeChart {...common} title="Utilization" yMax={100} fmt={pctFmt} series={[{ label: "GPU", color: "--m-gpu", values: s.get("gpu"), band: s.band("gpu") }]} />
        <TimeChart {...common} title="SM clock" subtitle={maxClock ? `Max ${maxClock} MHz` : undefined} yMax={maxClock ?? undefined} fmt={(v) => `${v.toFixed(0)} MHz`}
          thresholds={maxClock ? [{ value: maxClock, label: `Max ${maxClock} MHz`, tone: "neutral" }] : undefined}
          series={[{ label: "SM clock", color: "--m-gpu-clock", values: s.get("smClock"), band: s.band("smClock") }]} />
        <TimeChart {...common} title="Power" yMax={limit ?? undefined} fmt={wattFmt} robust={limit == null ? "auto" : undefined}
          thresholds={limit != null ? [{ value: limit, label: `Limit ${limit.toFixed(0)} W`, tone: "neutral" }] : undefined}
          series={[{ label: "Power", color: "--m-power", values: s.get("power"), band: s.band("power") }]} />
        <TimeChart {...common} title="Temperature" softMin={20} fmt={tempFmt}
          thresholds={[{ value: LIMITS.gpuTempC[0], label: `${LIMITS.gpuTempC[0]} °C`, tone: "warn" }, { value: LIMITS.gpuTempC[1], label: `${LIMITS.gpuTempC[1]} °C`, tone: "crit" }]}
          series={[{ label: "Temperature", color: "--m-temp", values: s.get("gpuTemp"), band: s.band("gpuTemp") }]} />
      </div>
      <SectionTitle
        title="Processes on the GPU"
        infoTopic="mon.node.gpu"
      />
      <div className="boxed">
        {/* While the GPU can't be read, what it last listed is greyed: not known to be running now. */}
        <Table tableId="node-gpu-procs" ariaLabel="GPU processes" columns={gpuProcColumns} rows={g.procs} rowKey={(p) => String(p.pid)}
          rowClass={why ? () => "row-dim" : undefined}
          empty={why ? "The GPU isn't answering, so what uses it isn't known." : "Nothing is using the GPU."} footer itemNoun={["process", "processes"]} />
      </div>
    </div>
  );
}
