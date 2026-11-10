import { Table, type Column } from "../../components/Table";
import { TimeChart } from "../../components/TimeChart";
import { HelpTip, Meter, SectionTitle, Tile, Tiles } from "../../components/ui";
import { movingAverage } from "../../lib/chartScale";
import { fmtByteRate, fmtBytes } from "../../lib/format";
import { diskFree, diskUsedPct, LIMITS, memPct, pct } from "../../lib/metrics";
import { recent } from "../../lib/store";
import { seqColor, useDarkMode } from "../../lib/theme";
import type { DiskStats, NodeSnapshot } from "../../lib/types";
import { minus, useNodeSeries } from "./series";
import type { ChartProps } from "./shared";
import { gibFmt, pctFmt } from "./Overview";

export function CpuTab({ node, chart }: { node: NodeSnapshot; chart: ChartProps }) {
  const dark = useDarkMode();
  const s = useNodeSeries(node.id, chart.rangeS, ["cpu"], chart.nowS);
  const cores = node.cpu.perCore;
  const half = Math.min(10, cores.length);
  const reporting = node.status === "online" && !node.stale;

  const group = (label: string, from: number, to: number) => (
    <section className="mon-section">
      <div className="section-title">{label}</div>
      <div className="cores" role="list" aria-label={label}>
        {cores.slice(from, to).map((p, i) => {
          const c = seqColor(p, dark);
          return (
            <div key={from + i} role="listitem" className="core" style={{ background: c.bg, color: c.fg }} title={`Core ${from + i}: ${p.toFixed(0)}%`}>
              <span className="core-id">{from + i}</span>
              <span className="num">{p.toFixed(0)}%</span>
            </div>
          );
        })}
      </div>
    </section>
  );

  return (
    <div className="mon-node-body">
      <Tiles>
        <Tile label="All cores" value={reporting ? node.cpu.totalPct.toFixed(0) : "—"} unit={reporting ? "%" : undefined} trend={recent(node.id, "cpu", 120)} trendMax={100} color="var(--m-cpu)" />
        <Tile label="Load average" value={node.cpu.load.map((l) => l.toFixed(2)).join(" · ")} sub="1, 5 and 15 minutes" />
        <Tile label="CPU temperature" value={reporting ? node.cpu.tempC.toFixed(0) : "—"} unit={reporting ? "°C" : undefined} />
      </Tiles>
      <TimeChart ts={s.ts} range={s.range} zoom={chart.zoom} onZoom={chart.onZoom} startsAt={s.startsAt} recordedFrom={s.recordedFrom} relative={chart.rangeS <= 900}
        title="CPU" subtitle={<HelpTip topic="mon.node.cpu" label="About the CPU tab" />} yMax={100} fmt={pctFmt} height={150} series={[{ label: "All cores", color: "--m-cpu", values: s.get("cpu"), band: s.band("cpu") }]} />
      {cores.length > 0 && group("Performance cores · Cortex-X925", 0, half)}
      {cores.length > half && group("Efficiency cores · Cortex-A725", half, cores.length)}
      <div className="scale-legend" aria-hidden>
        <span>0%</span>
        <div className="scale-ramp" />
        <span>100%</span>
      </div>
    </div>
  );
}

/**
 * Memory and IO pressure (/proc/pressure, avg10): "Pressure: none", "Pressure: memory 2% · IO 14%".
 * The share of the last 10 s that some task waited; null when the kernel doesn't expose it.
 */
export function pressureLine(m: NodeSnapshot["mem"]): string {
  if (m.psiSome10 == null && m.psiIoSome10 == null) return "Pressure: not reported by this node";
  const parts = [
    m.psiSome10 != null && m.psiSome10 >= 0.5 ? `memory ${m.psiSome10.toFixed(m.psiSome10 < 10 ? 1 : 0)}%` : null,
    m.psiIoSome10 != null && m.psiIoSome10 >= 0.5 ? `IO ${m.psiIoSome10.toFixed(m.psiIoSome10 < 10 ? 1 : 0)}%` : null,
  ].filter(Boolean);
  return parts.length ? `Pressure: ${parts.join(" · ")} of the last 10 s waiting` : "Pressure: none";
}

export function MemoryTab({ node, chart }: { node: NodeSnapshot; chart: ChartProps }) {
  const h = useNodeSeries(node.id, chart.rangeS, ["memUsed", "gpuAlloc", "memCache"], chart.nowS);
  const m = node.mem;
  const other = Math.max(0, m.used - m.gpuAlloc);
  const free = Math.max(0, m.total - m.used - m.cached);
  const segments = [
    { label: "GPU allocations", bytes: m.gpuAlloc, color: "var(--m-mem-gpu)" },
    { label: "Other used", bytes: other, color: "var(--m-mem)" },
    { label: "Page cache", bytes: m.cached, color: "var(--m-mem-cache)" },
    { label: "Free", bytes: free, color: "var(--track)", hollow: true },
  ];
  // Free is the empty track; where meters wear colour (Black) it also gets a ring in the track's grey,
  // so it reads as "empty" beside the solid page-cache segment. Light and Dark mix to the track itself.
  const ring = (s: { hollow?: boolean }) => (s.hollow ? { boxShadow: "inset 0 0 0 1px color-mix(in srgb, var(--text-3) var(--meter-tint, 0%), var(--track))" } : undefined);
  const used = memPct(node);
  return (
    <div className="mon-node-body">
      <Tiles>
        <Tile label="Used" value={fmtBytes(m.used)} sub={`${used.toFixed(0)}% of ${fmtBytes(m.total, 0)}`} tone={used >= LIMITS.memPct[1] ? "crit" : used >= LIMITS.memPct[0] ? "warn" : undefined}
          trend={recent(node.id, "memUsed", 120)} trendMax={m.total} color="var(--m-mem)" />
        <Tile label="Available" value={fmtBytes(m.available)} sub="Free plus reclaimable cache" />
        <Tile label="GPU allocations" value={fmtBytes(m.gpuAlloc)} sub={`${pct(m.gpuAlloc, m.total).toFixed(0)}% of the pool`} trend={recent(node.id, "gpuAlloc", 120)} trendMax={m.total} color="var(--m-mem-gpu)" />
        <Tile label="Swap" value={m.swapTotal ? fmtBytes(m.swapUsed) : "None"} sub={m.swapTotal ? `of ${fmtBytes(m.swapTotal)}` : "No swap configured"} />
      </Tiles>
      <SectionTitle
        title="How the pool is split"
        infoTopic="mon.node.memory"
      />
      <div className="stackbar" role="img" aria-label={segments.map((s) => `${s.label} ${fmtBytes(s.bytes)}`).join(", ")}>
        {segments.map((s) => (
          <div key={s.label} style={{ flexGrow: s.bytes, background: s.color, ...ring(s) }} title={`${s.label}: ${fmtBytes(s.bytes)}`} />
        ))}
      </div>
      <div className="legend">
        {segments.map((s) => (
          <span key={s.label}><i style={{ background: s.color, ...ring(s) }} />{s.label} <b className="num">{fmtBytes(s.bytes)}</b></span>
        ))}
      </div>
      <p className="muted mon-pressure" title="From /proc/pressure: the share of the last 10 seconds some task waited for memory or disk IO">{pressureLine(m)}</p>
      <TimeChart ts={h.ts} range={h.range} zoom={chart.zoom} onZoom={chart.onZoom} startsAt={h.startsAt} recordedFrom={h.recordedFrom} relative={chart.rangeS <= 900}
        title="Unified memory" subtitle={`GPU allocations, other use and page cache of ${fmtBytes(m.total, 0)}`} height={170}
        unit="bytes" yMax={m.total || undefined} fmt={gibFmt} stacked
        thresholds={m.total ? [{ value: m.total * LIMITS.memPct[0] / 100, label: `${LIMITS.memPct[0]}% used`, tone: "warn", series: 1 }, { value: m.total * LIMITS.memPct[1] / 100, label: `${LIMITS.memPct[1]}%`, tone: "crit", series: 1 }] : undefined}
        series={[
          { label: "GPU", color: "--m-mem-gpu", values: h.get("gpuAlloc") },
          { label: "Other", color: "--m-mem", values: minus(h.get("memUsed"), h.get("gpuAlloc")) },
          { label: "Cache", color: "--m-mem-cache", values: h.get("memCache") },
        ]} />
    </div>
  );
}

const diskColumns: Column<DiskStats>[] = [
  { key: "mount", label: "Mount", priority: 1, fit: "flex", minWidth: 80, ellipsis: "middle", sort: (d) => d.mount, render: (d) => d.mount },
  { key: "device", label: "Device", priority: 3, fit: "flex", minWidth: 90, ellipsis: "middle", sort: (d) => d.device, render: (d) => d.device },
  { key: "fs", label: "Type", priority: 3, fit: "content", render: (d) => d.fsType },
  { key: "size", label: "Size", align: "right", fit: "content", priority: 2, sort: (d) => d.total, render: (d) => <span className="num">{fmtBytes(d.total, { decimals: 1 })}</span> },
  // As df reports them: free is what a normal user can still write, and Used% leaves out the space root keeps back.
  { key: "free", label: "Free", align: "right", fit: "content", priority: 1, sort: (d) => diskFree(d), render: (d) => <span className="num">{fmtBytes(diskFree(d), { decimals: 1 })}</span> },
  {
    key: "use", label: "Used", fit: "content", priority: 1, sort: (d) => diskUsedPct(d),
    render: (d) => <Meter pct={diskUsedPct(d)} warn={LIMITS.diskPct[0]} crit={LIMITS.diskPct[1]} color="var(--m-write)" label={`${diskUsedPct(d).toFixed(0)}% · ${fmtBytes(d.used, { decimals: 1 })}`} ariaLabel={`${d.mount} used`} />,
  },
  { key: "read", label: "Read", align: "right", fit: "content", priority: 2, sort: (d) => d.readBps, render: (d) => <span className="num">{fmtByteRate(d.readBps)}</span> },
  { key: "write", label: "Write", align: "right", fit: "content", priority: 2, sort: (d) => d.writeBps, render: (d) => <span className="num">{fmtByteRate(d.writeBps)}</span> },
];

export function DisksTab({ node, chart }: { node: NodeSnapshot; chart: ChartProps }) {
  const s = useNodeSeries(node.id, chart.rangeS, ["diskRead", "diskWrite"], chart.nowS);
  return (
    <div className="mon-node-body">
      <SectionTitle
        title="Disks"
        infoTopic="mon.node.disks"
      />
      <div className="boxed">
        <Table tableId="node-disks-r2" ariaLabel="Disks" columns={diskColumns} rows={node.disks} rowKey={(d) => d.mount} empty="No disks reported."
          rowClass={(d) => (diskUsedPct(d) >= LIMITS.diskPct[1] ? "row-crit" : diskUsedPct(d) >= LIMITS.diskPct[0] ? "row-warn" : undefined)} />
      </div>
      <TimeChart ts={s.ts} range={s.range} zoom={chart.zoom} onZoom={chart.onZoom} startsAt={s.startsAt} recordedFrom={s.recordedFrom} relative={chart.rangeS <= 900}
        title="Root disk throughput" subtitle="Smoothed over 3 readings" fmt={fmtByteRate} height={170} robust="auto"
        series={[
          { label: "Read", color: "--m-read", values: movingAverage(s.get("diskRead")) },
          { label: "Write", color: "--m-write", values: movingAverage(s.get("diskWrite")) },
        ]} />
    </div>
  );
}
