// Palette commands for node operations: Restart…, Shut down…, Wake and the DGX Dashboard for
// every node, the built-in Fleet snippets on all nodes ("Run GPU snippet on all nodes"), and
// Cancel for every running task. App.tsx calls registerOpsCommands() once; the list follows
// the cluster and the tasks as they change. Everything goes through the same confirmations
// as the windows (power.ts), so the palette is never a shortcut past one.
import { Command, Gauge, Power, PowerOff, RotateCcw, Square } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { withAdmin } from "./admin";
import { api } from "./api";
import { registerCommands, type Command as PaletteCommand } from "./commands";
import { showError } from "./errors";
import { requestFleetRun, requestPower } from "./power";
import { getCluster } from "./store";
import { cancelTask, onTasksChange, openTasksWindow, getTasks } from "./tasks";
import type { ClusterSnapshot, NodeSnapshot } from "./types";
import { POWER, POWER_VERB } from "./vocabulary";
import { BUILT_IN } from "../windows/FleetCommand";

function openWindow(kind: string, params: Record<string, string> = {}) {
  window.dispatchEvent(new CustomEvent("sparkbox:open-window", { detail: { kind, params } }));
}

const isOff = (n: NodeSnapshot) => n.powerState === "off" || n.status !== "online";

/** NVIDIA's DGX Dashboard for a node, through an SSH tunnel (node scope: it's logged). */
export async function openDgxDashboard(n: NodeSnapshot, demo: boolean): Promise<void> {
  if (demo) {
    openWindow("node", { id: n.id });
    showError("In Demo mode there's no real node to connect to. Switch to Live data to open the DGX Dashboard through an SSH tunnel.", { action: `Open the DGX Dashboard of ${n.name}` });
    return;
  }
  try {
    const url = await withAdmin(() => api.tunnelOpen(n.id, 11000), { action: "tunnel_open" });
    await openUrl(url);
  } catch (e) {
    showError(e, { action: `Open the DGX Dashboard of ${n.name}`, node: n.id });
  }
}

export function nodeOpsCommands(cluster: ClusterSnapshot | null): PaletteCommand[] {
  if (!cluster) return [];
  return cluster.nodes.flatMap((n): PaletteCommand[] => {
    const entity = { kind: "node", id: n.id, name: n.name };
    const up = () => (n.status === "online" && !n.powerState ? (true as const) : `${n.name} isn't connected`);
    return [
      {
        id: `node:${n.id}:restart`, title: `${POWER_VERB.restart} ${n.name}…`, group: "Nodes", subtitle: "Asks first, and lists what goes down with it",
        icon: RotateCcw, entity, verbs: ["restart", "reboot"], needsAdmin: true, when: up, run: () => requestPower(n.id, "reboot"),
      },
      {
        id: `node:${n.id}:shutdown`, title: `${POWER_VERB.shutdown} ${n.name}…`, group: "Nodes", subtitle: "Asks for its name; it stays off until woken",
        icon: PowerOff, entity, verbs: ["shut", "shutdown", "poweroff"], needsAdmin: true, when: up, run: () => requestPower(n.id, "poweroff"),
      },
      {
        id: `node:${n.id}:wake`, title: `${POWER.wake} ${n.name}`, group: "Nodes", subtitle: "Wake-on-LAN, then waits for it and its models",
        icon: Power, entity, verbs: ["wake", "power", "start"], needsAdmin: true,
        when: () => (isOff(n) ? true : `${n.name} is on`), run: () => requestPower(n.id, "wake"),
      },
      {
        id: `node:${n.id}:dgx`, title: `DGX Dashboard on ${n.name}`, group: "Nodes", subtitle: "NVIDIA's dashboard, through an SSH tunnel", icon: Gauge, entity,
        keywords: ["nvidia", "jupyter", "tunnel"], verbs: ["open"], needsAdmin: !cluster.demo, when: up, run: () => openDgxDashboard(n, !!cluster.demo),
      },
    ];
  });
}

/** "Run GPU snippet on all nodes": the online nodes, through Fleet's tiers. */
export function snippetCommands(cluster: ClusterSnapshot | null): PaletteCommand[] {
  if (!cluster) return [];
  const online = cluster.nodes.filter((n) => n.status === "online").map((n) => n.id);
  const name = (id: string) => cluster.nodes.find((n) => n.id === id)?.name ?? id;
  return BUILT_IN.map((s) => ({
    id: `fleet:snippet:${s.name.toLowerCase().replace(/\s+/g, "-")}`,
    title: `Run ${s.name} snippet on all nodes`,
    group: "Fleet",
    subtitle: s.command,
    icon: Command,
    verbs: ["run"],
    keywords: ["fleet", "all nodes", s.name],
    needsAdmin: true,
    when: () => (online.length ? true : "No node is connected"),
    run: async () => {
      const started = await requestFleetRun({ nodes: online, command: s.command, sudo: false, timeoutS: 60, strategy: "parallel", stopOnFailure: false, title: s.name }, name);
      if (started) openTasksWindow(started.id);
    },
  }));
}

/** "Cancel <task>" for every running task that can stop. */
export function cancelCommands(): PaletteCommand[] {
  return getTasks()
    .filter((t) => t.state === "running" && t.cancellable && !t.cancelRequested)
    .map((t) => ({
      id: `task:${t.id}:cancel`, title: `Cancel ${t.title}`, group: "Tasks", subtitle: t.detail ?? "Running", icon: Square, verbs: ["cancel", "stop"],
      needsAdmin: !t.readOnly,
      run: () => cancelTask(t.id).catch((e) => showError(e, { action: `Stop “${t.title}”` })),
    }));
}

/** Registers the commands and keeps them in step with the cluster and the tasks. Returns an unsubscribe. */
export function registerOpsCommands(): () => void {
  let stop: (() => void) | null = null;
  let last = "";
  const refresh = () => {
    const cluster = getCluster();
    const running = getTasks().filter((t) => t.state === "running" && t.cancellable && !t.cancelRequested);
    const sig = `${cluster?.demo}|${(cluster?.nodes ?? []).map((n) => `${n.id}:${n.name}:${n.status}:${n.powerState ?? ""}`).join(",")}|${running.map((t) => t.id).join(",")}`;
    if (sig === last) return;
    last = sig;
    const next = registerCommands([...nodeOpsCommands(cluster), ...snippetCommands(cluster), ...cancelCommands()]);
    stop?.();
    stop = next;
  };
  refresh();
  const unTasks = onTasksChange(refresh);
  const poll = window.setInterval(refresh, 2000);
  return () => {
    window.clearInterval(poll);
    unTasks();
    stop?.();
  };
}
