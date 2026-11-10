// Maintenance mode (P1.16): the owner puts a node in maintenance for an hour, or until they end
// it, and its alerts are Planned and SparkBox pages nobody about them (src-tauri/src/health/
// maintenance.rs begin_manual). The node agents aren't told, so with agents installed the words say
// they may still page. Nothing runs on the node. It needs Admin (node scope; Mac scope too for
// "until I end it"), look-only refuses it, and with Demo data it says "Available with Live data".
// The node header, the palette and the status bar read the window from the snapshot
// (`maintenance`, reason "in maintenance").
import { Wrench } from "lucide-react";
import { withAdmin } from "./admin";
import { invoke } from "./api";
import { registerCommands, type Command } from "./commands";
import { showError } from "./errors";
import { confirmChoice } from "./session";
import { getCluster } from "./store";
import { toast } from "./toasts";
import type { ClusterSnapshot, MaintInfo, NodeSnapshot } from "./types";

/** The reason a window the owner opened carries (MANUAL_REASON in maintenance.rs). */
export const MANUAL_REASON = "in maintenance";
export const LIVE_ONLY = "Available with Live data";

/** The node is in maintenance mode (a window the owner opened, not a restart SparkBox started). */
export function inMaintenance(n: Pick<NodeSnapshot, "maintenance">): boolean {
  return n.maintenance?.reason === MANUAL_REASON;
}

/** "1 h", "90 min". */
function span(minutes: number): string {
  return minutes % 60 === 0 ? `${minutes / 60} h` : `${minutes} min`;
}

const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/** "for 1 h · until 11:42", "until you end it". */
export function maintenanceSpan(m: MaintInfo): string {
  if (!m.untilMs) return "until you end it";
  return `for ${span(Math.round((m.untilMs - m.startedMs) / 60_000))} · until ${clock(m.untilMs)}`;
}

/** "cell2 in maintenance for 1 h · until 11:42", "cell2 in maintenance until you end it"; null when it isn't. */
export function maintenanceText(n: Pick<NodeSnapshot, "name" | "maintenance">): string | null {
  return n.maintenance && inMaintenance(n) ? `${n.name} in maintenance ${maintenanceSpan(n.maintenance)}` : null;
}

/** Some node runs the SparkBox agent: the agents page on their own and aren't told about maintenance mode. */
function agentsInstalled(cluster: Pick<ClusterSnapshot, "nodes"> | null): boolean {
  return !!cluster?.nodes.some((n) => n.agent);
}

/** What maintenance mode does to the node's pages: "its alerts are planned and page nobody", or with agents installed, that they may still page. */
export function quietText(cluster: Pick<ClusterSnapshot, "nodes"> | null): string {
  return agentsInstalled(cluster)
    ? "its alerts are planned and SparkBox pages nobody about them; the node agents aren't told and may still page"
    : "its alerts are planned and page nobody";
}

const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Puts the node in maintenance for `minutes` (null: until it's ended, which needs Mac scope). */
export async function beginMaintenance(n: Pick<NodeSnapshot, "id" | "name">, minutes: number | null): Promise<void> {
  const action = `Put ${n.name} in maintenance`;
  try {
    const m = await withAdmin(() => invoke<MaintInfo>("maintenance_begin", { node: n.id, minutes }), { action: "maintenance_begin", scope: minutes === null ? "mac" : "node" });
    toast({ kind: "success", title: `${n.name} in maintenance ${maintenanceSpan(m)}`, body: `${sentence(quietText(getCluster()))}. Nothing on the node changes.` });
  } catch (e) {
    showError(e, { action, node: n.id });
  }
}

export async function endMaintenance(n: Pick<NodeSnapshot, "id" | "name">): Promise<void> {
  try {
    await withAdmin(() => invoke<void>("maintenance_end", { node: n.id }), { action: "maintenance_end" });
    toast({ kind: "success", title: `Maintenance ended on ${n.name}`, body: "Its alerts count again." });
  } catch (e) {
    showError(e, { action: `End maintenance on ${n.name}`, node: n.id });
  }
}

/** The node header's choice: for an hour, or until you end it. */
export async function askMaintenance(n: Pick<NodeSnapshot, "id" | "name">): Promise<void> {
  const choice = await confirmChoice({
    title: `Put ${n.name} in maintenance?`,
    body: `While ${n.name} is in maintenance, ${quietText(getCluster())}. Nothing on the node changes.`,
    confirmLabel: "For 1 hour",
    alternativeLabel: "Until I end it",
    cancelLabel: "Cancel",
  });
  if (choice) await beginMaintenance(n, choice === "confirm" ? 60 : null);
}

/** What the palette offers per node: begin for 1 h, begin until ended, end. */
export function maintenanceCommands(cluster: Pick<ClusterSnapshot, "demo" | "nodes"> | null): Command[] {
  if (!cluster) return [];
  return cluster.nodes.flatMap((n): Command[] => {
    const entity = { kind: "node", id: n.id, name: n.name };
    const on = inMaintenance(n);
    const canBegin = () => (cluster.demo ? LIVE_ONLY : on ? `${n.name} is in maintenance ${maintenanceSpan(n.maintenance!)}` : true);
    const base = { group: "Nodes", icon: Wrench, entity, needsAdmin: true, keywords: ["maintenance", "planned", "quiet alerts"] };
    const quiet = sentence(quietText(cluster));
    return [
      { ...base, id: `node:${n.id}:maintenance-1h`, title: `Put ${n.name} in maintenance for 1 h`, subtitle: quiet, verbs: ["maintenance"], when: canBegin, run: () => beginMaintenance(n, 60) },
      { ...base, id: `node:${n.id}:maintenance-open`, title: `Put ${n.name} in maintenance until I end it`, subtitle: quiet, verbs: ["maintenance"], when: canBegin, run: () => beginMaintenance(n, null) },
      {
        ...base, id: `node:${n.id}:maintenance-end`, title: `End maintenance on ${n.name}`, subtitle: "Its alerts count again", verbs: ["end", "maintenance"],
        when: () => (cluster.demo ? LIVE_ONLY : on ? true : `${n.name} isn't in maintenance`), run: () => endMaintenance(n),
      },
    ];
  });
}

/** What the palette's maintenance rows depend on, so they're rebuilt only when it changes. */
export function maintenanceSig(cluster: Pick<ClusterSnapshot, "demo" | "nodes"> | null): string {
  if (!cluster) return "";
  return `${cluster.demo}|${agentsInstalled(cluster)}|${cluster.nodes.map((n) => `${n.id}:${n.name}:${inMaintenance(n) ? `${n.maintenance!.startedMs}-${n.maintenance!.untilMs ?? ""}` : ""}`).join(",")}`;
}

/** Keeps the palette's maintenance rows in step with the cluster while it's open. Returns the unregister. */
export function registerMaintenanceCommands(): () => void {
  let stop: (() => void) | null = null;
  let last: string | null = null;
  const refresh = () => {
    const cluster = getCluster();
    const sig = maintenanceSig(cluster);
    if (sig === last) return;
    last = sig;
    const next = registerCommands(maintenanceCommands(cluster));
    stop?.();
    stop = next;
  };
  refresh();
  const poll = window.setInterval(refresh, 1000);
  return () => {
    window.clearInterval(poll);
    stop?.();
  };
}
