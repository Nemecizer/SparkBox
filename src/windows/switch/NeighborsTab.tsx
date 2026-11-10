// Who the switch sees behind each port, the way WinBox shows it: the bridge host
// table (MAC → port, matched to the node that owns the MAC) and the LLDP/MNDP
// neighbors. This is what places nodes on ports without trusting comments.
import { useEffect, useState } from "react";
import { Table, type Column } from "../../components/Table";
import { HelpTip, Notice, SectionTitle } from "../../components/ui";
import { fabricApi, shortPort, type HostEntry, type Neighbor, type PortTables } from "../../lib/fabric";
import type { ClusterSnapshot, SwitchSnapshot } from "../../lib/types";

function describe(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : (e as { message?: string })?.message ?? String(e);
}

export function NeighborsTab({ sw, cluster, onSelect }: { sw: SwitchSnapshot; cluster: ClusterSnapshot; onSelect: (port: string) => void }) {
  const [tables, setTables] = useState<PortTables | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () =>
      fabricApi
        .switchTables(sw.id)
        .then((t) => alive && (setTables(t), setError(null)))
        .catch((e) => alive && setError(describe(e)));
    void load();
    const t = window.setInterval(load, 10_000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [sw.id]);

  /** The node and interface a MAC belongs to, from the nodes' own reports. */
  const owner = (mac: string) => {
    const m = mac.toUpperCase();
    for (const n of cluster.nodes) {
      const x = n.nets.find((x) => x.mac?.toUpperCase() === m);
      if (x) return { node: n.name, iface: x.iface };
    }
    return null;
  };

  const hostCols: Column<HostEntry>[] = [
    { key: "mac", label: "MAC address", sort: (h) => h.mac, render: (h) => <span className="mono">{h.mac}</span>, fit: "content" },
    { key: "port", label: "Port", sort: (h) => h.port, render: (h) => <span className="strong">{shortPort(h.port)}</span>, title: (h) => h.port, fit: "content" },
    {
      key: "owner", label: "Belongs to", sort: (h) => owner(h.mac)?.node ?? "~", ellipsis: "end",
      render: (h) => {
        const o = owner(h.mac);
        return o ? <span>{o.node} <span className="muted mono">{o.iface}</span></span> : <span className="muted">Not a node</span>;
      },
    },
    { key: "kind", label: "Learned", sort: (h) => (h.local ? 2 : h.dynamic ? 0 : 1), render: (h) => <span className="muted">{h.local ? "Switch's own" : h.dynamic ? "Dynamic" : "Static"}</span>, priority: 2 },
  ];
  const nbCols: Column<Neighbor>[] = [
    { key: "port", label: "Port", sort: (n) => n.port, render: (n) => <span className="strong">{shortPort(n.port)}</span>, title: (n) => n.port, fit: "content" },
    { key: "identity", label: "Identity", sort: (n) => n.identity, render: (n) => <span>{n.identity || "—"}</span>, ellipsis: "end" },
    { key: "address", label: "Address", sort: (n) => n.address ?? "", render: (n) => <span className="mono">{n.address ?? "—"}</span>, priority: 2 },
    { key: "platform", label: "Platform", sort: (n) => n.platform ?? "", render: (n) => <span className="muted">{n.platform ?? "—"}</span>, ellipsis: "end", priority: 3 },
    { key: "by", label: "Seen by", sort: (n) => n.discoveredBy, render: (n) => <span className="muted">{n.discoveredBy.toUpperCase()}</span>, fit: "content", priority: 2 },
  ];

  if (error) return <div className="neighbors"><Notice tone="error">{error}</Notice></div>;
  if (!tables) return <div className="neighbors muted">Reading the switch's tables…</div>;
  return (
    <div className="neighbors">
      <SectionTitle
        title="Bridge hosts"
        infoTopic="mon.switch.bridge-hosts"
      />
      <Table columns={hostCols} rows={tables.hosts} rowKey={(h) => `${h.mac}@${h.port}`} onActivate={(h) => onSelect(h.port)} tableId="switch.hosts" footer itemNoun={["host", "hosts"]} ariaLabel="Bridge hosts" empty="No hosts learned" />
      <SectionTitle
        title="Neighbors"
        infoTopic="mon.switch.neighbors"
      />
      <Table columns={nbCols} rows={tables.neighbors} rowKey={(n) => `${n.port}/${n.identity}/${n.mac ?? ""}`} onActivate={(n) => onSelect(n.port)} tableId="switch.neighbors" footer itemNoun={["neighbor", "neighbors"]} ariaLabel="Neighbors" empty="No neighbors announced" />
      <span className="neighbors-note muted">
        Refreshed every 10 seconds
        <HelpTip topic="mon.switch.tables" label="About these tables" />
      </span>
    </div>
  );
}
