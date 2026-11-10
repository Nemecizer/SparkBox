import type { DiskCheck, DiskRequest } from "../../lib/types";
import { snapshotAt } from "../mockBackend";

const GiB = 1024 ** 3;
/** Browser-only scenarios: ?disk=low, warning, unknown, or changed (space lost after review). */
export function demoDiskPreflight(request: DiskRequest, bytes = 1_200_000_000, starting = false): DiskCheck[] {
  const mode = typeof location === "undefined" ? null : new URLSearchParams(location.search).get("disk");
  if (mode === "unknown") throw new Error("Cannot read destination disk space on the selected Demo node. Nothing was started.");
  const nodes = request.kind === "copy" ? request.targets : [request.node];
  const dest = request.kind === "copy" ? request.key : request.kind === "resume" ? request.expected.dest : `/home/demo/models/${request.repo}`;
  return nodes.map((node) => {
    if (!snapshotAt(0).nodes.some((n) => n.id === node && n.status === "online")) throw new Error(`${node} is not connected. Reconnect it before checking disk space.`);
    const availableBytes = (mode === "low" || (mode === "changed" && starting) ? 10 : mode === "warning" ? 500 : 2000) * GiB;
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
