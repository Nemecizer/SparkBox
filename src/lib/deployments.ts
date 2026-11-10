// Deployments found on the cluster, whoever started them (src-tauri/src/models/postmortem.rs):
// containers on several nodes grouped by SparkBox's label, a Compose project, the same image and
// command apart from a trailing rank, the same head address in their environment, or Ray workers
// and their head. A failed one carries a read-only postmortem: when it ran, how it exited, the
// decisive log lines and the likely cause. The list arrives on `deployments://changed`; the first
// call that lists a failed one has its postmortem read in the background.
import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./api";
import type { ClusterDeployment, DeploymentPostmortem } from "./types";

export type { ClusterDeployment as Deployment, DeploymentPostmortem };

export const DEPLOYMENTS_EVENT = "deployments://changed";

let list: ClusterDeployment[] | null = null;
let started = false;
const listeners = new Set<() => void>();

function set(next: ClusterDeployment[]) {
  list = next;
  listeners.forEach((l) => l());
}

/** Failed deployments whose postmortem was asked for, by id and exit: one ask each. */
const asked = new Set<string>();

/**
 * A failed deployment that arrives without its postmortem (the backend's own round found it, or it
 * failed while the app watched) is being shown, so it's read now: listing it starts the read.
 */
function readWhatsShown(ds: readonly ClusterDeployment[]) {
  const todo = ds.filter((d) => d.state === "failed" && !d.postmortem && !asked.has(`${d.id}@${d.finishedMs ?? 0}`));
  if (!todo.length) return;
  todo.forEach((d) => asked.add(`${d.id}@${d.finishedMs ?? 0}`));
  void refreshDeployments();
}

/** What deployments://changed does: shows the new list, and reads what it newly shows. */
export function onDeploymentsChanged(ds: ClusterDeployment[]) {
  set(ds);
  readWhatsShown(ds);
}

async function start() {
  if (started) return;
  started = true;
  if (inTauri) await listen<ClusterDeployment[]>(DEPLOYMENTS_EVENT, (e) => onDeploymentsChanged(e.payload));
  await refreshDeployments();
}

/** Asks for the list now (it also starts reading a failed one's postmortem). */
export async function refreshDeployments(retry?: string): Promise<ClusterDeployment[]> {
  try {
    const ds = await invoke<ClusterDeployment[]>("cluster_deployments_list", retry ? { retry } : undefined);
    // This call starts the read of any failed one listed here: don't ask again for those.
    ds.filter((d) => d.state === "failed" && !d.postmortem).forEach((d) => asked.add(`${d.id}@${d.finishedMs ?? 0}`));
    set(ds);
  } catch {
    // An older backend has no inferred deployments: nothing to show.
    if (list === null) set([]);
  }
  return list ?? [];
}

function subscribe(l: () => void) {
  listeners.add(l);
  void start();
  return () => listeners.delete(l);
}

const NONE: ClusterDeployment[] = [];

/** Every deployment on the cluster, newest first; empty until the first list arrives. */
export function useDeployments(): ClusterDeployment[] {
  return useSyncExternalStore(subscribe, () => list) ?? NONE;
}

/** The latest list, for code outside React. */
export function getDeployments(): ClusterDeployment[] {
  return list ?? NONE;
}

/** Test seam: the list as the backend would send it. */
export function setDeploymentsForTest(next: ClusterDeployment[] | null) {
  list = next;
  listeners.forEach((l) => l());
}

/** The most recent failed deployment that ended within `withinDays`, or null. */
export function latestFailed(ds: readonly ClusterDeployment[], withinDays = 7, nowMs = Date.now()): ClusterDeployment | null {
  const since = nowMs - withinDays * 86_400_000;
  return ds.filter((d) => d.state === "failed" && (d.finishedMs ?? d.startedMs) >= since).sort((a, b) => (b.finishedMs ?? 0) - (a.finishedMs ?? 0))[0] ?? null;
}

/** What a deployment is called: the model it runs ("GLM-5.3-Flash-NVFP4"), else its served name, else its id. Never a container id. */
export function deploymentLabel(d: Pick<ClusterDeployment, "label" | "served" | "id">): string {
  return d.label || d.served || d.id;
}

/** "cell1, cell2, cell3 and gx10-0000". */
export function nodeList(nodes: readonly string[]): string {
  if (nodes.length <= 1) return nodes.join("");
  return `${nodes.slice(0, -1).join(", ")} and ${nodes[nodes.length - 1]}`;
}

/** "54 s", "3 min 12 s", "2 h 5 min". */
export function ranFor(s: number): string {
  // A no-break space keeps each number with its unit: a card never ends a line on "54" and starts the next with "s".
  if (s < 60) return `${s}\u00a0s`;
  if (s < 3600) return `${Math.floor(s / 60)}\u00a0min${s % 60 ? ` ${s % 60}\u00a0s` : ""}`;
  return `${Math.floor(s / 3600)}\u00a0h${Math.floor((s % 3600) / 60) ? ` ${Math.floor((s % 3600) / 60)}\u00a0min` : ""}`;
}

/** "ranks 0–3", when its members have ranks. */
export function rankSpan(d: Pick<ClusterDeployment, "members">): string | null {
  const ranks = d.members.map((m) => m.rank).filter((r): r is number => r != null);
  if (ranks.length < 2) return null;
  const lo = Math.min(...ranks);
  const hi = Math.max(...ranks);
  return lo === hi ? `rank ${lo}` : `ranks ${lo}–${hi}`;
}

/** "Failed", "Stopped", "Partly running", "Running". */
export function deploymentStateWord(d: Pick<ClusterDeployment, "state">): string {
  return { unknown: "Unknown", failed: "Failed", stopped: "Stopped", partial: "Partly running", running: "Running" }[d.state] ?? d.state;
}

/** "glm53 · 4 nodes · ranks 0–3 · Failed": the Containers Deployments view's group line. */
export function groupLine(d: ClusterDeployment): string {
  return [d.id, `${d.nodes.length} node${d.nodes.length === 1 ? "" : "s"}`, rankSpan(d), deploymentStateWord(d)].filter(Boolean).join(" · ");
}

/** "rank 0 · cell1 · head"; without the node ("rank 0 · head") next to a Node column. */
export function memberLine(m: ClusterDeployment["members"][number], withNode = true): string {
  return [m.rank != null ? `rank ${m.rank}` : null, withNode ? m.node : null, m.role === "head" ? "head" : null].filter(Boolean).join(" · ");
}

/** Which deployment a container belongs to, by node and id (ids may be short or full). */
export function deploymentOfContainer(ds: readonly ClusterDeployment[], node: string, id: string): ClusterDeployment | null {
  const same = (a: string, b: string) => !!a && !!b && (a === b || a.startsWith(b) || b.startsWith(a));
  return ds.find((d) => d.members.some((m) => m.node === node && same(m.id, id))) ?? null;
}

/**
 * The exits a failed deployment ended with, as one sentence: "Exited 1 on cell1, cell2, cell3 and
 * gx10-0000 after 54 s". Members that ended differently are named after.
 */
export function exitSentence(d: ClusterDeployment): string {
  const byCode = new Map<number, string[]>();
  for (const m of d.members) {
    if (m.state === "running" || m.state === "unknown" || m.state === "missing") continue;
    const nodes = byCode.get(m.exitCode) ?? [];
    if (!nodes.includes(m.node)) nodes.push(m.node);
    byCode.set(m.exitCode, nodes);
  }
  const parts = [...byCode.entries()].sort((a, b) => b[1].length - a[1].length).map(([code, nodes]) => `${code === 0 ? "Stopped" : `Exited ${code}`} on ${nodeList(nodes)}`);
  const after = d.ranS != null ? ` after ${ranFor(d.ranS)}` : "";
  return parts.length ? `${parts[0]}${after}${parts.length > 1 ? `; ${parts.slice(1).join("; ")}` : ""}` : "";
}
