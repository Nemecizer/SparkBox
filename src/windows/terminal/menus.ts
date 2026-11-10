// The "+" button's node menu: per node, a shell, a shell in a container, a container's
// logs or a unit's journal (the units SparkBox knows matter, plus any that failed). Containers
// are opened by name, so the tab, the recent list and the activity log all say "vllm-head",
// and a tab reopened later finds a container that was recreated under a new ID.
import { Box, ScrollText, Settings2, SquareTerminal } from "lucide-react";
import type { MenuItem } from "../../lib/contextMenu";
import type { ClusterSnapshot } from "../../lib/types";
import { connectTo } from "./QuickConnect";

const UNITS = ["docker.service", "nvidia-persistenced.service", "ssh.service", "dgx-dashboard.service", "systemd-networkd.service"];

export function nodeMenu(cluster: ClusterSnapshot | null, admin: boolean, lookOnly = false): MenuItem[] {
  const nodes = cluster?.nodes ?? [];
  if (!nodes.length) return [{ label: "No nodes yet", disabled: true }];
  return nodes.map((n): MenuItem => {
    const running = n.containers.filter((c) => c.state === "running");
    const units = [...new Set([...(n.failedUnits ?? []), ...UNITS])];
    const offline = n.status !== "online" ? `${n.name} is ${n.status}` : undefined;
    return {
      label: n.name,
      icon: SquareTerminal,
      disabled: offline,
      submenu: [
        // Shells need Admin (connectTo asks); in look-only they're `locked`, which the menu disables.
        { label: "Shell", icon: SquareTerminal, hint: admin || lookOnly ? undefined : "needs Admin", locked: lookOnly || undefined, run: () => connectTo({ node: n.id, target: { kind: "shell" } }) },
        running.length
          ? { label: "Container shell", icon: Box, submenu: running.map((c) => ({ label: c.name, hint: c.image.split("/").pop(), locked: lookOnly || undefined, run: () => connectTo({ node: n.id, target: { kind: "containerShell", container: c.name || c.id }, title: c.name }) })) }
          : { label: "Container shell", icon: Box, disabled: "No containers are running" },
        n.containers.length
          ? { label: "Logs", icon: ScrollText, submenu: n.containers.map((c) => ({ label: c.name, hint: c.state, run: () => connectTo({ node: n.id, target: { kind: "containerLogs", container: c.name || c.id }, title: c.name }) })) }
          : { label: "Logs", icon: ScrollText, disabled: "No containers" },
        { label: "Journal", icon: Settings2, submenu: units.map((u) => ({ label: u, hint: n.failedUnits?.includes(u) ? "failed" : undefined, run: () => connectTo({ node: n.id, target: { kind: "journal", unit: u } }) })) },
      ],
    };
  });
}
