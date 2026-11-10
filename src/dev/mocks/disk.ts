import type { DiskCheck, DiskRequest, DownloadCapacity, DownloadSelection } from "../../lib/types";
import { snapshotAt } from "../mockBackend";

const GiB = 1024 ** 3;
/** Browser-only scenarios: ?disk=capacity, low, warning, unknown, or changed (space lost after review). */
export function demoDiskPreflight(request: DiskRequest, bytes = 1_200_000_000, starting = false): DiskCheck[] {
  if (request.kind === "download") bytes = request.selection.files.reduce((sum, file) => sum + (file.size ?? 0), 0);
  const mode = typeof location === "undefined" ? null : new URLSearchParams(location.search).get("disk");
  if (mode === "unknown") throw new Error("Cannot read destination disk space on the selected Demo node. Nothing was started.");
  const nodes = request.kind === "copy" ? request.targets : [request.node];
  const dest = request.kind === "copy" ? request.key : request.kind === "resume" ? request.expected.dest : `/home/demo/models/${request.selection.repo}`;
  return nodes.map((node) => {
    if (!snapshotAt(0).nodes.some((n) => n.id === node && n.status === "online")) throw new Error(`${node} is not connected. Reconnect it before checking disk space.`);
    const availableBytes = (mode === "capacity" ? (node === "cell2" ? 2500 : node === "cell1" ? 10 : 1000) : mode === "low" || (mode === "changed" && starting) ? 10 : mode === "warning" ? 500 : 2000) * GiB;
    const capacityBytes = 4000 * GiB;
    const requiredBytes = Math.ceil(bytes * 1.05) + 20 * GiB;
    const usedPercent = 100 * (capacityBytes - availableBytes) / capacityBytes;
    const projectedUsedPercent = Math.min(100, 100 * (capacityBytes - availableBytes + bytes) / capacityBytes);
    return { node, dest, probe: "/home/demo", transferBytes: bytes, availableBytes, capacityBytes, requiredBytes,
      usedPercent, projectedUsedPercent, ok: availableBytes >= requiredBytes,
      warning: projectedUsedPercent > 85 ? `Demo: this disk is ${usedPercent.toFixed(1)}% full and could be ${projectedUsedPercent.toFixed(1)}% full after the transfer.` : null };
  });
}

export function requireDemoDisk(request: DiskRequest): void {
  if (demoDiskPreflight(request, undefined, true).some((c) => !c.ok)) throw new Error("Not enough destination disk space now. No Demo job was started; review the destination again.");
}

/** Deliberately uneven synthetic capacities make the suggestion visible in browser Demo. */
export function demoDownloadCapacity(selection: DownloadSelection, nodes: string[]): DownloadCapacity {
  if (!nodes.length || new Set(nodes).size !== nodes.length || nodes.some((id) => !snapshotAt(0).nodes.some((n) => n.id === id))) throw new Error("Choose distinct inventory nodes to compare destination capacity.");
  if (!selection.files.length || selection.files.some((file) => file.size == null || !Number.isSafeInteger(file.size) || file.size < 0)
    || !Number.isSafeInteger(selection.files.reduce((sum, file) => sum + file.size!, 0))) throw new Error("The transfer has unknown or invalid file sizes. Refresh its details before comparing.");
  const readings = nodes.slice().sort().map((node) => {
    try {
      const check = demoDiskPreflight({ kind: "download", selection, node })[0];
      return { node, check, error: null };
    } catch (e) { return { node, check: null, error: String(e).replace(/^Error: /, "") }; }
  });
  const ranked = readings.flatMap((r) => r.check?.ok ? [r.check] : []).sort((a, b) => (b.availableBytes - b.requiredBytes) - (a.availableBytes - a.requiredBytes) || (a.node < b.node ? -1 : a.node > b.node ? 1 : 0));
  return { readings, suggested: ranked[0]?.node ?? null };
}
