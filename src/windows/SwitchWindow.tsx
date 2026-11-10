import { Router } from "lucide-react";
import { EmptyState, Waiting } from "../components/ui";
import { useCluster } from "../lib/store";
import type { SwitchSnapshot } from "../lib/types";
import { useWM, type Params } from "../wm/WindowManager";
import { FirewallaView } from "./switch/FirewallaView";
import { RouterOsView } from "./switch/RouterOsView";
import { lldpPorts } from "./switch/switchX";
import "./net.css";

const KIND_WORD: Record<SwitchSnapshot["kind"], string> = {
  routerOs: "RouterOS REST API (HTTPS)",
  firewalla: "reachability probes",
  gateway: "reachability probes",
};

/** One switch or gateway: the CRS804 in full, the Firewalla devices by probing. */
export function SwitchWindow({ params }: { params: Params }) {
  const cluster = useCluster();
  const wm = useWM();
  const sw = cluster?.switches.find((s) => s.id === params.id);
  if (!cluster) return <Waiting />;
  if (!sw) return <EmptyState icon={Router} title="Not in the inventory" body={`${params.name ?? params.id} was removed in Settings.`} />;
  // The Switch X's ports need no address: LLDP comes from the nodes, so a switch they hear still shows.
  const heard = sw.kind === "firewalla" && lldpPorts(cluster, sw).ports.size > 0;
  if (sw.configured === false && !heard) {
    return (
      <EmptyState
        icon={Router}
        title="Not set up"
        body={`${sw.name} has no address yet. Add it in Settings › Switches & gateway and SparkBox starts watching it through ${KIND_WORD[sw.kind]}.`}
        action={{ label: "Open Settings", onClick: () => wm.open("settings", { section: "switches" }) }}
      />
    );
  }
  return sw.kind === "routerOs"
    ? <RouterOsView sw={sw} cluster={cluster} linkedPort={params.port} nav={params.nav} />
    : <FirewallaView sw={sw} cluster={cluster} />;
}
