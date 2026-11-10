// A name that opens its window: a node, switch, container or service mentioned
// anywhere (a table cell, an alert, a log line). Hovering shows its key numbers.
import type { MouseEvent, ReactNode } from "react";
import { fmtBytesPair, fmtDuration } from "../lib/format";
import { gpuError } from "../lib/metrics";
import { useCluster } from "../lib/store";
import type { ClusterSnapshot } from "../lib/types";
import { useWM, type Params } from "../wm/WindowManager";

export type EntityKind = "node" | "container" | "service" | "switch";

interface Target {
  kind: string;
  params?: Params;
  label: string;
  title: string;
}

/**
 * Where a link goes. A click opens the entity's own window; ⌘-click opens the
 * node it runs on instead (for containers and services).
 */
export function entityTarget(snap: ClusterSnapshot | null, kind: EntityKind, id: string, node?: string, alt = false): Target {
  const n = snap?.nodes.find((x) => x.id === (kind === "node" ? id : node));
  if (kind === "node" || (alt && n)) {
    const nodeId = kind === "node" ? id : node!;
    const stats = n
      ? n.status === "online"
        ? `GPU ${gpuError(n) ? "—" : `${Math.round(n.gpu.utilPct)}%`} · CPU ${Math.round(n.cpu.totalPct)}% · ${fmtBytesPair(n.mem.used, n.mem.total)} memory`
        : n.status
      : "not in the inventory";
    return { kind: "node", params: { id: nodeId }, label: n?.name ?? nodeId, title: `${n?.name ?? nodeId} — ${stats}` };
  }
  if (kind === "switch") {
    const s = snap?.switches.find((x) => x.id === id);
    const stats = s ? [s.status, s.latencyMs != null ? `${s.latencyMs.toFixed(1)} ms` : null, s.host || null].filter(Boolean).join(" · ") : "not in the inventory";
    return { kind: "switch", params: { id, name: s?.name ?? id }, label: s?.name ?? id, title: `${s?.name ?? id} — ${stats}` };
  }
  if (kind === "container") {
    const c = n?.containers.find((x) => x.id === id || x.name === id);
    const up = c?.state === "running" && c.startedAt ? `up ${fmtDuration(Date.now() / 1000 - c.startedAt)}` : c?.state;
    const stats = c ? [up, c.image].filter(Boolean).join(" · ") : "not found";
    return { kind: "inspect", params: { node: node ?? "", id: c?.id ?? id, name: c?.name ?? id }, label: c?.name ?? id, title: `${c?.name ?? id} on ${n?.name ?? node} — ${stats}` };
  }
  const failed = n?.failedUnits?.includes(id);
  return { kind: "services", label: id, title: `${id}${node ? ` on ${n?.name ?? node}` : ""}${failed ? " — failed" : ""}` };
}

export function EntityLink({ kind, id, node, children }: { kind: EntityKind; id: string; node?: string; children?: ReactNode }) {
  const snap = useCluster();
  const wm = useWM();
  const t = entityTarget(snap, kind, id, node);
  const open = (e: MouseEvent) => {
    e.stopPropagation();
    const to = e.metaKey ? entityTarget(snap, kind, id, node, true) : t;
    wm.open(to.kind, to.params);
  };
  return (
    <button type="button" className="entity-link" title={`${t.title}${node && kind !== "node" ? "\n⌘-click opens the node" : ""}`} onClick={open}>
      {children ?? t.label}
    </button>
  );
}
