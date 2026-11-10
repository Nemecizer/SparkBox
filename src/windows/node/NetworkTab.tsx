import { useRef } from "react";
import { TriangleAlert } from "lucide-react";
import { Icon } from "../../components/Icon";
import { Table, type Column } from "../../components/Table";
import { AdvisoryGlyph, SectionTitle, Sparkline, StatusDot } from "../../components/ui";
import { fmtBits } from "../../lib/format";
import { listNames, nodeAdvisories } from "../../lib/nodeStatus";
import { useCluster } from "../../lib/store";
import type { LldpNeighbor, NetStats, NodeSnapshot } from "../../lib/types";
import { useWM } from "../../wm/WindowManager";
import { RocePanel } from "../fabric/RocePanel";

const FABRIC_MTU = 9000;
/** Errors that rose this recently count as "increasing". */
const RISING_MS = 60_000;
const SPARK = 60;

interface Track {
  rx: number[];
  tx: number[];
  errors: number;
  roseAt: number | null;
}

/**
 * Per-interface history kept by this window: a minute of rates for the sparklines and
 * when the error counters last went up. It starts when the tab opens.
 */
function useIfaceTracks(node: NodeSnapshot): Map<string, Track> {
  const tracks = useRef(new Map<string, Track>());
  const seen = useRef<NodeSnapshot | null>(null);
  if (seen.current !== node) {
    seen.current = node;
    const now = Date.now();
    for (const x of node.nets) {
      let t = tracks.current.get(x.iface);
      const errors = (x.rxErrors ?? 0) + (x.txErrors ?? 0) + (x.rxDrops ?? 0) + (x.txDrops ?? 0);
      if (!t) {
        t = { rx: [], tx: [], errors, roseAt: null };
        tracks.current.set(x.iface, t);
      }
      if (errors > t.errors) t.roseAt = now;
      t.errors = errors;
      const reporting = node.status === "online" && !node.stale;
      if (reporting) {
        t.rx.push(x.rxBps + (x.rdmaRxBps ?? 0));
        t.tx.push(x.txBps + (x.rdmaTxBps ?? 0));
        if (t.rx.length > SPARK) t.rx.shift();
        if (t.tx.length > SPARK) t.tx.shift();
      }
    }
  }
  return tracks.current;
}

/** The ConnectX-7 cage a netdev belongs to: enp1s0f1np1 and enP2p1s0f1np1 are the two halves of port 1. */
function cageOf(iface: string): number | null {
  const m = /f(\d+)np\d+$/.exec(iface);
  return m ? Number(m[1]) : null;
}

const total = (x: NetStats, dir: "rx" | "tx") => (dir === "rx" ? x.rxBps + (x.rdmaRxBps ?? 0) : x.txBps + (x.rdmaTxBps ?? 0));

/** "200G", "10G", "2.4G" (Wi-Fi's 2402 Mb/s), "867M": the link column's compact speed (the full words are its tooltip). Unknown is "—", never "0M". */
export function speed(x: Pick<NetStats, "up" | "speedMbps">): string {
  if (!x.up || !x.speedMbps) return "—";
  return x.speedMbps >= 1000 ? `${Number((x.speedMbps / 1000).toFixed(1))}G` : `${x.speedMbps}M`;
}

function linkTitle(x: NetStats): string {
  if (!x.up) return x.ip ? "Down, though it has an address" : "No link: nothing is plugged in, or the other end is off";
  if (x.role === "wifi") return wifiLine(x);
  return `Up at ${x.speedMbps >= 1000 ? `${x.speedMbps / 1000} Gb/s` : `${x.speedMbps} Mb/s`}${x.mtu ? ` · MTU ${x.mtu}` : ""}`;
}

/** "wlP9s9 · Connected to ExampleLAN · −52 dBm · 867 Mb/s · 192.168.50.229 · 154 drops since boot". */
export function wifiLine(x: NetStats): string {
  const w = x.wifi;
  const parts = [
    x.iface,
    w ? (w.ssid ? `Connected to ${w.ssid}` : "Connected") : "Up (iw isn't there to say more)",
    w?.signalDbm != null ? `${w.signalDbm < 0 ? "−" : ""}${Math.abs(w.signalDbm)} dBm` : null,
    w?.bitrateMbps != null ? `${Math.round(w.bitrateMbps)} Mb/s` : null,
    x.ip,
    x.carrierChanges != null ? `${x.carrierChanges.toLocaleString()} drops since boot` : null,
  ];
  return parts.filter(Boolean).join(" · ");
}

const firstLabel = (name: string | null | undefined) => (name ?? "").split(".")[0].toLowerCase();

/**
 * The node of this cluster an LLDP neighbour is, if it's one: its system name is a node's
 * hostname (or name), or its chassis or port id is one of a node's MACs. Anything else is a switch.
 */
export function lldpNode(nb: LldpNeighbor, nodes: readonly NodeSnapshot[]): NodeSnapshot | null {
  const sys = firstLabel(nb.system);
  const ids = [nb.chassisId, nb.portId].filter(Boolean).map((v) => (v as string).toLowerCase());
  return (
    nodes.find((n) => (sys && (sys === firstLabel(n.system.hostname) || sys === n.name.toLowerCase() || sys === n.id.toLowerCase())) || n.nets.some((x) => x.mac && ids.includes(x.mac.toLowerCase()))) ?? null
  );
}

/** What LLDP heard on a port (one LAN interface, or a fabric port's two halves), in words. */
export interface LldpView {
  text: string;
  title: string;
  /** A switch port; other nodes heard through the switch; only this node itself. */
  kind: "switch" | "peer" | "self";
}

/**
 * Port 2's header, the LAN line and Fabric health say this, never a node's own NIC as its switch
 * port: each ConnectX-7 half hears its sibling half through the switch (the collector drops those
 * and says so in lldpSelfOnly; an older collector's are dropped here). Null when LLDP heard nothing.
 */
export function lldpView(nets: readonly NetStats[], node: NodeSnapshot, nodes: readonly NodeSnapshot[]): LldpView | null {
  const others = nodes.filter((n) => n.id !== node.id);
  // Frame 6 lists every neighbour that isn't this node; an older collector sent one, maybe itself.
  const heardOn = (x: NetStats) => (x.lldpPeers?.length ? x.lldpPeers : x.lldp ? [x.lldp] : []);
  const heard = nets.flatMap(heardOn);
  const own = (nb: LldpNeighbor) => lldpNode(nb, [node]) != null || (!!nb.port && node.nets.some((x) => x.mac && x.mac.toLowerCase() === nb.port!.toLowerCase()));
  const kept = heard.filter((nb) => !own(nb));
  const fabric = nets.some((x) => x.role === "fabric");
  // A switch port another node hears too isn't this port's far end: a switch in between passed
  // its LLDP on (on the real LAN every node hears the Firewalla's Gi0/2 and the CRS804's ether2).
  const portOf = (nb: LldpNeighbor) => `${(nb.chassisId ?? nb.system ?? "").toLowerCase()}|${nb.portId ?? nb.port ?? ""}`;
  const elsewhere = new Set(others.flatMap((n) => n.nets.flatMap(heardOn)).map(portOf));
  const switches = kept.filter((nb) => !lldpNode(nb, others));
  const sw = switches.find((nb) => !elsewhere.has(portOf(nb)));
  if (sw) return { kind: "switch", text: `Switch port ${sw.port ?? sw.portId ?? "?"} (LLDP)`, title: `LLDP neighbor${sw.system ? ` ${sw.system}` : ""}` };
  const passed = switches.map((nb) => `${nb.system ?? "a switch"} ${nb.port ?? nb.portId ?? ""}`.trim());
  const also = passed.length ? `. Also heard, as every node hears it: ${passed.join(", ")}` : "";
  const peers = [...new Set(kept.map((nb) => lldpNode(nb, others)?.name).filter((n): n is string => !!n))].sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
  if (peers.length) {
    const title = `LLDP from ${listNames(peers)}, passed on by the switch between them${also}`;
    if (fabric) return { kind: "peer", text: `No LLDP from the switch (${listNames(peers)} ${peers.length === 1 ? "answers" : "answer"} through it)`, title };
    return { kind: "peer", text: `${peers.length === 1 ? "Neighbor" : "Neighbors"}: ${listNames(peers)} (via switch)`, title };
  }
  if (passed.length) return { kind: "peer", text: `Switch port unknown (${passed[0]} is heard by other nodes too)`, title: `Passed on by a switch in between${also}` };
  if (nets.some((x) => x.lldpSelfOnly) || heard.length) {
    return {
      kind: "self",
      text: `No LLDP from the switch (only this node${fabric ? "'s other half" : ""} answers)`,
      title: `${node.name} hears its own LLDP back through the switch: the switch itself doesn't send LLDP`,
    };
  }
  return null;
}

export function NetworkTab({ node }: { node: NodeSnapshot }) {
  const cluster = useCluster();
  const wm = useWM();
  const tracks = useIfaceTracks(node);
  const now = Date.now();
  const reporting = node.status === "online" && !node.stale;

  // The fabric MTU every other node uses on the same interface, to spot a mismatch.
  const peerMtu = (iface: string) =>
    new Set((cluster?.nodes ?? []).filter((n) => n.id !== node.id).flatMap((n) => n.nets.filter((x) => x.iface === iface && x.up && x.mtu).map((x) => x.mtu as number)));
  const mtuProblem = (x: NetStats): string | null => {
    if (x.role !== "fabric" || !x.up || x.mtu == null) return null;
    if (x.mtu !== FABRIC_MTU) return `The fabric should use jumbo frames (MTU ${FABRIC_MTU}); this is ${x.mtu}.`;
    const peers = peerMtu(x.iface);
    if (peers.size && !peers.has(x.mtu)) return `Other nodes use MTU ${[...peers].join(", ")} on ${x.iface}.`;
    return null;
  };
  const rising = (x: NetStats) => {
    const t = tracks.get(x.iface);
    return !!t?.roseAt && now - t.roseAt < RISING_MS;
  };
  const switchOf = (x: NetStats) => {
    if (!x.switchPort) return null;
    const [id, port] = x.switchPort.includes("/") ? x.switchPort.split("/") : ["", x.switchPort];
    const sw = cluster?.switches.find((s) => s.id === id) ?? cluster?.switches.find((s) => s.kind === "routerOs");
    return { id: sw?.id ?? id, name: sw?.name ?? id, port };
  };

  const base: Column<NetStats>[] = [
    { key: "iface", label: "Interface", priority: 1, fit: "content", sort: (x) => x.iface, render: (x) => <span className="strong mono">{x.iface}</span> },
    {
      key: "link", label: "Link", fit: "content", priority: 1, sort: (x) => (x.up ? 0 : 1), title: linkTitle,
      render: (x) => (x.up ? <span className="mon-link-up"><span className="ok-dot" aria-hidden />{speed(x)}</span> : <StatusDot status={x.ip ? "error" : "offline"} text={x.ip ? "Down" : "No link"} />),
    },
    { key: "ip", label: "Address", fit: "content", priority: 2, sort: (x) => x.ip ?? "", render: (x) => (x.ip ? <span className="mono">{x.ip}</span> : <span className="muted">—</span>) },
    {
      key: "mtu", label: "MTU", align: "right", fit: "content", priority: 2, sort: (x) => x.mtu ?? 0,
      render: (x) => {
        const problem = mtuProblem(x);
        if (x.mtu == null) return <span className="muted">—</span>;
        return problem ? (
          <span className="warn-text num" title={problem}><Icon icon={TriangleAlert} size={12} /> {x.mtu}</span>
        ) : <span className="num">{x.mtu}</span>;
      },
    },
    { key: "rx", label: "Receive", short: "↓", align: "right", fit: "content", priority: 1, sort: (x) => total(x, "rx"), render: (x) => (reporting ? <span className="num">{fmtBits(total(x, "rx"))}</span> : "—") },
    { key: "tx", label: "Transmit", short: "↑", align: "right", fit: "content", priority: 1, sort: (x) => total(x, "tx"), render: (x) => (reporting ? <span className="num">{fmtBits(total(x, "tx"))}</span> : "—") },
    {
      key: "spark", label: "Last minute", fit: "content", priority: 3,
      render: (x) => {
        // A port with no link has no traffic to draw: a flat line there reads as "idle", not "unplugged".
        if (!x.up) return <span className="muted">—</span>;
        const t = tracks.get(x.iface);
        if (!t || t.rx.length < 2) return <span className="muted small">Collecting…</span>;
        const max = Math.max(1, ...t.rx, ...t.tx);
        return (
          <span className="mon-dual-spark" title="Receive (solid) and transmit over the last minute">
            <Sparkline values={t.rx} max={max} width={90} height={18} color="var(--m-rx)" curve="monotone" />
            <Sparkline values={t.tx} max={max} width={90} height={18} color="var(--m-tx)" curve="monotone" />
          </span>
        );
      },
    },
    {
      key: "errors", label: "Errors · drops", short: "Err · drop", align: "right", fit: "content", priority: 2,
      sort: (x) => (x.rxErrors ?? 0) + (x.txErrors ?? 0),
      render: (x) => {
        const errs = (x.rxErrors ?? 0) + (x.txErrors ?? 0);
        const drops = (x.rxDrops ?? 0) + (x.txDrops ?? 0);
        const up = rising(x);
        return (
          <span className={`num${up ? " warn-text" : ""}`} title={up ? "Rising in the last minute" : "Totals since the interface came up"}>
            {up && <Icon icon={TriangleAlert} size={12} />} {errs.toLocaleString()} · {drops.toLocaleString()}
          </span>
        );
      },
    },
  ];

  const fabricColumns: Column<NetStats>[] = [
    ...base.slice(0, 6),
    {
      key: "rdma", label: "RDMA share", short: "RDMA", align: "right", fit: "content", priority: 2,
      sort: (x) => (x.rdmaRxBps ?? 0) + (x.rdmaTxBps ?? 0),
      render: (x) => {
        const all = total(x, "rx") + total(x, "tx");
        const rdma = (x.rdmaRxBps ?? 0) + (x.rdmaTxBps ?? 0);
        if (x.rdmaRxBps == null && x.rdmaTxBps == null) return <span className="muted" title="No RDMA counters for this interface">—</span>;
        return <span className="num" title={`${x.rdmaDev ?? "RDMA"}: ${fmtBits(rdma)} of ${fmtBits(all)}`}>{all > 0 ? `${((rdma / all) * 100).toFixed(0)}%` : "—"}</span>;
      },
    },
    ...base.slice(6),
    { key: "dev", label: "RDMA device", fit: "content", priority: 3, sort: (x) => x.rdmaDev ?? "", render: (x) => (x.rdmaDev ? <span className="mono">{x.rdmaDev}</span> : <span className="muted">—</span>) },
  ];

  const fabric = node.nets.filter((x) => x.role === "fabric");
  const cages = new Map<string, NetStats[]>();
  for (const x of fabric) {
    const c = cageOf(x.iface);
    const key = c == null ? x.iface : String(c);
    cages.set(key, [...(cages.get(key) ?? []), x]);
  }
  const others = node.nets.filter((x) => x.role !== "fabric");
  const wifiUp = others.filter((x) => x.role === "wifi" && x.up);
  const advisories = nodeAdvisories(node);

  const rowClass = (x: NetStats) => (x.up ? undefined : x.ip ? "row-crit" : "row-dim");

  return (
    <div className="mon-node-body">
      {advisories.map((a) => (
        <div key={a.id} className="net-advisory" role="note">
          <AdvisoryGlyph title="Worth knowing, not a problem" size={12} />
          <p>
            <b>{a.title}</b> {a.body}{a.command && <> (<code>{a.command}</code>)</>}{a.after ? `; ${a.after}` : "."}
          </p>
        </div>
      ))}
      {fabric.length > 0 && (
        <section className="mon-section">
          <SectionTitle title="Fabric health" suffix="RoCE" infoTopic="mon.node.fabric-health" />
          <RocePanel node={node.id} />
        </section>
      )}
      {[...cages.entries()].sort().map(([key, nets]) => {
        const sw = nets.map(switchOf).find(Boolean);
        const lldp = lldpView(nets, node, cluster?.nodes ?? [node]);
        const up = nets.some((x) => x.up);
        const cage = Number.isFinite(Number(key)) ? `Port ${Number(key) + 1}` : key;
        return (
          <section key={key} className="mon-section">
            <SectionTitle
              title={cage}
              suffix={Number.isFinite(Number(key)) ? "ConnectX-7" : undefined}
              infoTopic="mon.node.port"
              actions={
                <>
                  <span className="muted">{up ? "200G cable" : "Not cabled"}</span>
                  {sw && (
                    <button type="button" className="mon-link" onClick={() => wm.open("switch", { id: sw.id, name: sw.name, port: sw.port })}>
                      {sw.port} on {sw.name}
                    </button>
                  )}
                  {!sw && lldp && <span className="muted" title={lldp.title}>· {lldp.text}</span>}
                </>
              }
            />
            <div className="boxed">
              <Table tableId="node-net-fabric-r2" ariaLabel={`${cage} interfaces`} columns={fabricColumns} rows={nets} rowKey={(x) => x.iface} rowClass={rowClass} />
            </div>
          </section>
        );
      })}
      {others.length > 0 && (
        <section className="mon-section">
          <SectionTitle title="LAN and Wi-Fi" />
          <div className="boxed">
            <Table tableId="node-net-lan-r2" ariaLabel="LAN and Wi-Fi interfaces" columns={base} rows={others} rowKey={(x) => x.iface} rowClass={rowClass} />
          </div>
          {wifiUp.map((x) => <p key={x.iface} className="net-wifi-line" title={linkTitle(x)}>{wifiLine(x)}</p>)}
          {others.map((x) => {
            const heard = lldpView([x], node, cluster?.nodes ?? [node]);
            return heard && <p key={`lldp-${x.iface}`} className="net-wifi-line" title={heard.title}>{x.iface} · {heard.text}</p>;
          })}
        </section>
      )}
    </div>
  );
}
