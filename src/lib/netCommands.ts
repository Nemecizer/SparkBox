// Palette commands for the network (App.tsx registers them once): open the Fabric
// window, test the fabric, accept the current cabling, and open any switch port by
// its short name ("Open port 2-1 on CRS804 DDQ"). Port commands follow the switches.
import { Cable, Router, Stethoscope } from "lucide-react";
import { withAdmin } from "./admin";
import { registerCommands, type Command } from "./commands";
import { fabricApi, peekFabricHealth, shortPort, subscribeFabric } from "./fabric";
import { confirmAction } from "./session";
import { getCluster } from "./store";
import { toast } from "./toasts";
import { openWindow } from "../wm/WindowManager";

function staticCommands(): Command[] {
  return [
    { id: "net:fabric", title: "Open Fabric", group: "Network", subtitle: "RoCE health, NCCL transport and cabling", icon: Cable, keywords: ["roce", "rdma", "nccl", "pfc", "ecn", "lossless"], run: () => openWindow("fabric") },
    {
      id: "net:fabric-test",
      title: "Test fabric…",
      group: "Network",
      subtitle: "MTU audit, jumbo ping, ib_write_bw and ib_send_lat between nodes",
      icon: Stethoscope,
      verbs: ["test", "doctor", "benchmark"],
      keywords: ["perftest", "ib_write_bw", "bandwidth", "latency", "mtu", "ping"],
      run: () => openWindow("fabric", { tab: "doctor" }),
    },
    {
      id: "net:cabling-accept",
      title: "Accept current cabling…",
      group: "Network",
      subtitle: "Remember which switch port each node's fabric cable is in now",
      icon: Cable,
      verbs: ["accept"],
      keywords: ["cabling", "ports", "baseline", "swapped"],
      when: () => ((peekFabricHealth()?.cabling.length ?? 0) > 0 ? true : "The cabling matches what was learned"),
      run: async () => {
        const issues = peekFabricHealth()?.cabling ?? [];
        const ok = await confirmAction({
          title: "Accept the current cabling?",
          body: "SparkBox remembers which switch port each node's fabric cable is in now, and stops flagging these moves.",
          details: issues.length ? issues.map((c) => c.message).join(". ") + "." : undefined,
          confirmLabel: "Accept cabling",
          dedupeKey: "net:cabling-accept",
        });
        if (!ok) return;
        try {
          const n = await withAdmin(() => fabricApi.acceptCabling(), { scope: "mac", action: "fabric_cabling_accept" });
          toast({ kind: "success", title: "Cabling accepted", body: `${n} fabric interfaces recorded` });
        } catch (e) {
          if ((e as { kind?: string })?.kind === "cancelled") return;
          toast({ kind: "error", title: "Couldn't accept the cabling", body: (e as { message?: string })?.message ?? String(e) });
        }
      },
    },
  ];
}

/** "Open port 2-1 on CRS804 DDQ", one per port of every RouterOS switch. */
function portCommands(): Command[] {
  const cluster = getCluster();
  const out: Command[] = [];
  for (const sw of cluster?.switches ?? []) {
    if (sw.kind !== "routerOs") continue;
    for (const p of sw.ports) {
      const short = shortPort(p.name);
      const node = p.node ? cluster?.nodes.find((n) => n.id === p.node)?.name ?? p.node : null;
      out.push({
        id: `net:port:${sw.id}:${p.name}`,
        title: `Open port ${short} on ${sw.name}`,
        group: "Network",
        subtitle: node ? `${p.name} · ${node}` : p.comment ? `${p.name} · ${p.comment}` : p.name,
        icon: Router,
        verbs: ["open", "port"],
        keywords: [p.name, short, "port", ...(node ? [node] : [])],
        entity: { kind: "port", id: `${sw.id}/${p.name}`, name: `port ${short}` },
        run: () => openWindow("switch", { id: sw.id, name: sw.name, port: p.name, nav: String(Date.now()) }),
      });
    }
  }
  return out;
}

export function registerNetCommands(): () => void {
  const stopStatic = registerCommands(staticCommands());
  let key = "";
  let stopPorts: () => void = () => {};
  const refresh = () => {
    const c = getCluster();
    const next = (c?.switches ?? []).map((s) => `${s.id}:${s.name}:${s.ports.map((p) => `${p.name}=${p.node ?? ""}`).join(",")}`).join(";");
    if (next === key) return;
    key = next;
    stopPorts();
    stopPorts = registerCommands(portCommands());
  };
  refresh();
  // The fabric store ticks with every snapshot, which is when ports or their nodes can change.
  const unsub = subscribeFabric(refresh);
  return () => {
    unsub();
    stopPorts();
    stopStatic();
  };
}
