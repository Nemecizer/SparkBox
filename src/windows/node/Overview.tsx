import { TimeChart, useChartEvents, type ChartSeries } from "../../components/TimeChart";
import type { ReactNode } from "react";
import { HelpTip, KnownMark, Tile, Tiles } from "../../components/ui";
import { movingAverage } from "../../lib/chartScale";
import { fmtBits, fmtBitsPair, fmtByteRate, fmtBytes, fmtDuration, GiB, splitUnit } from "../../lib/format";
import { alertShownAs, useHealth, type HealthState } from "../../lib/health";
import { diskFree, diskPct, fabricRx, fabricTx, gpuError, LIMITS, memPct, powerLimit, rootDisk } from "../../lib/metrics";
import { recent } from "../../lib/store";
import type { NodeSnapshot } from "../../lib/types";
import { minus, useNodeSeries } from "./series";
import type { ChartProps } from "./shared";

const KEYS = ["cpu", "gpu", "memUsed", "gpuAlloc", "memCache", "power", "gpuTemp", "fabricRx", "fabricTx", "rdmaRx", "rdmaTx", "diskRead", "diskWrite"] as const;

export const pctFmt = (v: number) => `${v.toFixed(0)}%`;
export const gibFmt = (v: number) => `${(v / GiB).toFixed(v >= 10 * GiB || v === 0 ? 0 : 1)} GiB`;
export const wattFmt = (v: number) => `${v.toFixed(0)} W`;
export const tempFmt = (v: number) => `${v.toFixed(0)} °C`;

// Judged on the value as printed (as Meter does), so a tile reading "85%" at an 85% line is amber, not a hair under it.
const tone = (v: number, [warn, crit]: readonly [number, number]) => {
  const r = Math.round(v);
  return r >= crit ? "crit" : r >= warn ? "warn" : undefined;
};

/** The alert rules each tile shows the metric of. */
const TILE_RULES: Record<"gpu" | "mem" | "disk", string[]> = { gpu: ["gpu-hot", "gpu-throttle", "gpu-unreadable"], mem: ["mem-high"], disk: ["disk-full", "disk-eta"] };

/**
 * A tile whose alert is a known issue (silenced or acknowledged) is drawn muted (§3.4): no
 * amber or red, a grey bell-slash beside its line. Returns the mark, or null.
 */
export function knownIssue(health: HealthState | null, node: string, tile: keyof typeof TILE_RULES): string | null {
  const a = health?.alerts.find((x) => x.entity.kind === "node" && x.entity.id === node && TILE_RULES[tile].includes(x.rule));
  if (!a) return null;
  const shown = alertShownAs(a);
  return shown === "silenced" ? "Known issue: silenced" : shown === "acknowledged" ? "Known issue: acknowledged" : null;
}

const withMark = (known: string | null, sub: ReactNode) => (known ? <>{sub} <KnownMark title={known} /></> : sub);

/** "Oct 25 12:05": when the node last started, from its uptime. */
export function bootTime(n: NodeSnapshot, now = Date.now()): string {
  const d = new Date(now - n.system.uptimeS * 1000);
  return `${d.toLocaleDateString([], { month: "short", day: "numeric" })} ${d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`;
}

export function Overview({ node, chart }: { node: NodeSnapshot; chart: ChartProps }) {
  const s = useNodeSeries(node.id, chart.rangeS, [...KEYS], chart.nowS);
  const reporting = node.status === "online" && !node.stale;
  const gpuReadable = reporting && !gpuError(node);
  const mem = node.mem;
  const disk = rootDisk(node);
  const limit = powerLimit(node);
  const events = useChartEvents(chart.rangeS, node.id);
  const health = useHealth();
  const known = { gpu: knownIssue(health, node.id, "gpu"), mem: knownIssue(health, node.id, "mem"), disk: knownIssue(health, node.id, "disk") };
  const common = { ts: s.ts, range: s.range, zoom: chart.zoom, onZoom: chart.onZoom, startsAt: s.startsAt, recordedFrom: s.recordedFrom, relative: chart.rangeS <= 900, height: 170, events };
  const line = (label: string, color: string, values: (number | null)[], extra: Partial<ChartSeries> = {}): ChartSeries => ({ label, color, values, ...extra });
  const tcpRx = minus(s.get("fabricRx"), s.get("rdmaRx"));
  const tcpTx = minus(s.get("fabricTx"), s.get("rdmaTx"));

  return (
    <div className="mon-node-body">
      <Tiles>
        <Tile label="CPU" value={reporting ? node.cpu.totalPct.toFixed(0) : "—"} unit={reporting ? "%" : undefined}
          sub={reporting ? `Load ${node.cpu.load[0].toFixed(2)} · ${node.cpu.tempC.toFixed(0)} °C` : "Not reporting"} trend={recent(node.id, "cpu", 120)} trendDomain={[0, 100]} color="var(--m-cpu)" />
        <Tile label="GPU" value={gpuReadable ? node.gpu.utilPct.toFixed(0) : "—"} unit={gpuReadable ? "%" : undefined}
          sub={withMark(known.gpu, gpuReadable ? `${node.gpu.powerW.toFixed(0)} W · ${node.gpu.tempC.toFixed(0)} °C` : reporting ? gpuError(node) ?? "" : "Not reporting")}
          trend={recent(node.id, "gpu", 120)} trendDomain={[0, 100]} color="var(--m-gpu)" tone={gpuReadable && !known.gpu ? tone(node.gpu.tempC, LIMITS.gpuTempC) : undefined} />
        <Tile label="Unified memory" value={reporting ? gibFmt(mem.used).replace(" GiB", "") : "—"} unit={reporting ? `of ${fmtBytes(mem.total, { decimals: 1 })}` : undefined}
          sub={withMark(known.mem, reporting ? `${memPct(node).toFixed(0)}% · ${fmtBytes(mem.gpuAlloc, { decimals: 1 })} on the GPU` : "Not reporting")}
          trend={recent(node.id, "memUsed", 120)} trendDomain={[0, mem.total || 1]} color="var(--m-mem)" tone={reporting && !known.mem ? tone(memPct(node), LIMITS.memPct) : undefined} />
        <Tile label="Root disk" value={disk ? diskPct(node).toFixed(0) : "—"} unit={disk ? "%" : undefined}
          sub={withMark(known.disk, disk ? `${fmtBytes(diskFree(disk), { decimals: 1 })} free of ${fmtBytes(disk.total, { decimals: 1 })}` : "No disk reported")}
          meter={disk ? { pct: diskPct(node), warn: known.disk ? 101 : LIMITS.diskPct[0], crit: known.disk ? 101 : LIMITS.diskPct[1], color: "var(--m-write)" } : undefined}
          tone={disk && !known.disk ? tone(diskPct(node), LIMITS.diskPct) : undefined} />
        <Tile label="Fabric ↓ / ↑" value={reporting ? splitUnit(fmtBitsPair(fabricRx(node), fabricTx(node)))[0] : "—"}
          unit={reporting ? splitUnit(fmtBitsPair(fabricRx(node), fabricTx(node)))[1] : undefined}
          sub="IP and RDMA together" trend={recent(node.id, "fabricRx", 120)} color="var(--m-rx)" />
        <Tile label="Uptime" value={node.status === "online" && node.system.uptimeS ? fmtDuration(node.system.uptimeS) : "—"}
          sub={node.system.rebootRequired ? "Restart needed for updates" : node.status === "online" && node.system.uptimeS ? `since ${bootTime(node)}` : "Not reporting"}
          tone={node.system.rebootRequired ? "warn" : undefined} />
      </Tiles>
      <div className="mon-chart-grid">
        <TimeChart {...common} title="CPU and GPU" subtitle={<HelpTip topic="mon.node.overview" label="About the Overview tab" />} yMax={100} fmt={pctFmt}
          series={[line("CPU", "--m-cpu", s.get("cpu"), { band: s.band("cpu") }), line("GPU", "--m-gpu", s.get("gpu"), { band: s.band("gpu") })]} />
        <TimeChart {...common} title="Unified memory" subtitle={`GPU allocations, other use and page cache of ${fmtBytes(mem.total, 0)}`}
          unit="bytes" yMax={mem.total || undefined} fmt={gibFmt} stacked
          thresholds={mem.total ? [{ value: mem.total * 0.9, label: "90% used", tone: "warn", series: 1 }, { value: mem.total * 0.95, label: "95%", tone: "crit", series: 1 }] : undefined}
          series={[
            line("GPU", "--m-mem-gpu", s.get("gpuAlloc")),
            line("Other", "--m-mem", minus(s.get("memUsed"), s.get("gpuAlloc"))),
            line("Cache", "--m-mem-cache", s.get("memCache")),
          ]} />
        <TimeChart {...common} title="GPU power" subtitle={limit != null ? `Limit ${limit.toFixed(0)} W` : undefined} yMax={limit ?? undefined} fmt={wattFmt}
          robust={limit == null ? "auto" : undefined}
          thresholds={limit != null ? [{ value: limit, label: `Limit ${limit.toFixed(0)} W`, tone: "neutral" }] : undefined}
          series={[line("Power", "--m-power", s.get("power"), { band: s.band("power") })]} />
        <TimeChart {...common} title="GPU temperature" softMin={20} fmt={tempFmt}
          thresholds={[{ value: LIMITS.gpuTempC[0], label: `${LIMITS.gpuTempC[0]} °C`, tone: "warn" }, { value: LIMITS.gpuTempC[1], label: `${LIMITS.gpuTempC[1]} °C`, tone: "crit" }]}
          series={[line("Temperature", "--m-temp", s.get("gpuTemp"), { band: s.band("gpuTemp") })]} />
        {/* Per-second packet counts jitter; unsmoothed, an idle link drew as a solid block of spikes. */}
        <TimeChart {...common} title="Fabric" subtitle="RoCE and TCP, smoothed over 5 readings" fmt={fmtBits} robust="auto"
          series={[
            line("RDMA ↓", "--m-rdma", movingAverage(s.get("rdmaRx"), 5)),
            line("RDMA ↑", "--m-rdma", movingAverage(s.get("rdmaTx"), 5), { dash: true, noFill: true }),
            line("TCP ↓", "--m-tcp", movingAverage(tcpRx, 5)),
            line("TCP ↑", "--m-tcp", movingAverage(tcpTx, 5), { dash: true, noFill: true }),
          ]} />
        <TimeChart {...common} title="Root disk" subtitle="Read and write, smoothed over 3 readings" fmt={fmtByteRate} robust="auto"
          series={[line("Read", "--m-read", movingAverage(s.get("diskRead"))), line("Write", "--m-write", movingAverage(s.get("diskWrite")))]} />
      </div>
    </div>
  );
}
