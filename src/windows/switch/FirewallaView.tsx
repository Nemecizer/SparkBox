import { useCallback, useEffect, useMemo, useState } from "react";
import { ExternalLink, PlugZap } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { TimeChart } from "../../components/TimeChart";
import { Table, type Column } from "../../components/Table";
import { Badge, HelpTip, Notice, Spacer, StatusDot, Tile, Tiles, Toolbar, ToolButton } from "../../components/ui";
import { SwitchState } from "./SwitchState";
import { SwitchXDrawer } from "./SwitchXDrawer";
import { SwitchXLegend, SwitchXPanel } from "./SwitchXPanel";
import { buildPorts, cloudLine } from "./switchX";
import { firewallaApi, type MspStatus, type PortsView } from "../../lib/firewalla";
import { fmtBits, fmtDateTime, fmtDuration, fmtRelative } from "../../lib/format";
import { relTime } from "../TasksWindow";
import { netApi, type ProbeHistory } from "../../lib/netApi";
import type { ClusterSnapshot, NetStats, NodeSnapshot, SwitchSnapshot } from "../../lib/types";
import { useVpnStatus } from "../../lib/vpn";
import { useWinState, useWM } from "../../wm/WindowManager";

interface LanRow {
  node: NodeSnapshot;
  nic: NetStats | undefined;
}

const lanColumns: Column<LanRow>[] = [
  { key: "node", label: "Node", sort: (r) => r.node.name, priority: 1, render: (r) => <span className="strong">{r.node.name}</span> },
  {
    key: "link", label: "Link", priority: 1, sort: (r) => (r.nic?.up ? 0 : 1),
    render: (r) =>
      r.node.status !== "online" ? <StatusDot status="offline" text="No data" /> : r.nic ? <StatusDot status={r.nic.up ? "online" : "offline"} text={r.nic.up ? "Up" : "Down"} /> : <span className="muted">—</span>,
  },
  { key: "nic", label: "Interface", priority: 3, render: (r) => <span className="mono">{r.nic?.iface ?? "—"}</span> },
  { key: "ip", label: "Address", priority: 2, render: (r) => <span className="mono">{r.nic?.ip ?? r.node.host}</span> },
  { key: "speed", label: "Speed", align: "right", priority: 1, sort: (r) => r.nic?.speedMbps ?? 0, render: (r) => <span className="num">{r.nic?.up ? (r.nic.speedMbps >= 1000 ? `${r.nic.speedMbps / 1000} Gb/s` : `${r.nic.speedMbps} Mb/s`) : "—"}</span> },
  // 2.5G and faster Ethernet only runs full duplex; slower links report it but the collector doesn't send it.
  { key: "duplex", label: "Duplex", priority: 3, render: (r) => (r.nic?.up ? (r.nic.speedMbps >= 1000 ? "Full" : "—") : "—") },
  { key: "rx", label: "↓ / ↑", align: "right", priority: 2, sort: (r) => (r.nic?.rxBps ?? 0) + (r.nic?.txBps ?? 0), render: (r) => <span className="num">{r.nic ? `${fmtBits(r.nic.rxBps)} / ${fmtBits(r.nic.txBps)}` : "—"}</span> },
  { key: "errors", label: "Errors", align: "right", priority: 2, sort: (r) => (r.nic?.rxErrors ?? 0) + (r.nic?.txErrors ?? 0), render: (r) => <span className="num">{r.nic ? ((r.nic.rxErrors ?? 0) + (r.nic.txErrors ?? 0)).toLocaleString() : "—"}</span> },
  { key: "drops", label: "Drops", align: "right", priority: 3, sort: (r) => r.nic?.rxDrops ?? 0, render: (r) => <span className="num">{r.nic ? ((r.nic.rxDrops ?? 0) + (r.nic.txDrops ?? 0)).toLocaleString() : "—"}</span> },
  {
    key: "flaps", label: "Flaps", align: "right", priority: 2, sort: (r) => r.nic?.carrierChanges ?? 0,
    // Carrier changes count down and up; one boot-time link-up counts once.
    render: (r) => <span className="num">{r.nic?.carrierChanges != null ? Math.max(0, Math.floor((r.nic.carrierChanges - 1) / 2)).toLocaleString() : "—"}</span>,
  },
];

/** How often the panel re-reads the labels and the cloud's last answer. Both come from memory. */
const VIEW_EVERY_MS = 15_000;

/** The Switch X's labels and cloud state, kept fresh. `reload` follows a saved label. */
function usePortsView(switchId: string, enabled: boolean) {
  const [view, setView] = useState<PortsView | null>(null);
  const reload = useCallback(() => {
    if (enabled) firewallaApi.view(switchId).then(setView, () => {});
  }, [switchId, enabled]);
  useEffect(() => {
    reload();
    if (!enabled) return;
    const t = setInterval(reload, VIEW_EVERY_MS);
    return () => clearInterval(t);
  }, [reload, enabled]);
  return { view, reload };
}

/** A problem in the cloud layer, in the app's words, with the way to fix it when there is one. */
function CloudNotice({ msp, onSettings }: { msp: MspStatus | undefined; onSettings: () => void }) {
  if (!msp?.message || msp.state === "ok") return null;
  const fix = msp.state === "auth" || msp.state === "no-token" || msp.state === "not-json";
  return <Notice tone="warn" action={fix ? { label: "Open Settings", onClick: onSettings } : undefined}>{msp.message}</Notice>;
}

/** The Firewalla Switch X and Gold: no local API, so SparkBox probes them and shows the LAN as the nodes see it. The Switch X adds its ports. */
export function FirewallaView({ sw, cluster }: { sw: SwitchSnapshot; cluster: ClusterSnapshot }) {
  const gold = sw.kind === "gateway";
  const wm = useWM();
  const { view, reload } = usePortsView(sw.id, !gold);
  const [selected, setSelected] = useWinState<number | null>("xport", null);
  const { ports, lldp } = useMemo(() => buildPorts(cluster, sw, view, cluster.tsMs), [cluster, sw, view]);
  const port = gold ? null : ports.find((p) => p.n === selected) ?? null;
  const settings = () => wm.open("settings", { section: "switches" });
  const closeDrawer = () => {
    const n = selected;
    setSelected(null);
    // Back to the port the keyboard came from.
    requestAnimationFrame(() => document.querySelector<SVGGElement>(`.sx-port[data-port="${n}"]`)?.focus());
  };
  const vpn = useVpnStatus();
  const [history, setHistory] = useState<ProbeHistory | null>(null);
  const [test, setTest] = useState<{ ok: boolean; message: string } | null>(null);

  useEffect(() => {
    let live = true;
    const load = () => netApi.switchProbeHistory(sw.id).then((h) => live && setHistory(h), () => {});
    load();
    const t = setInterval(load, 5000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [sw.id]);

  const rows: LanRow[] = cluster.nodes.map((n) => ({ node: n, nic: n.nets.find((x) => x.role === "lan") }));
  const samples = history?.samples ?? [];
  const outage = history?.lastOutage;
  const vpnRtt = vpn?.state === "connected" ? vpn.latencyMs : null;

  const runTest = async () => {
    try {
      const r = await netApi.switchTest(sw.id);
      setTest({ ok: r.ok, message: r.message });
    } catch (e) {
      setTest({ ok: false, message: String(e) });
    }
  };

  return (
    <div className="switch-win">
      <Toolbar label={`${sw.name} actions`}>
        <ToolButton icon={PlugZap} label="Probe now" onClick={runTest} />
        <HelpTip topic="firewalla-view" />
        <Spacer />
        <ToolButton icon={ExternalLink} label="Open Firewalla web UI" onClick={() => openUrl("https://my.firewalla.com").catch(() => {})} title="my.firewalla.com: the Firewalla app's web version" />
      </Toolbar>
      <div className="switch-strip">
        <SwitchState sw={sw} />
        {sw.latencyMs != null && <span className="num muted">{sw.latencyMs.toFixed(1)} ms</span>}
        <span className="strip-sep" />
        <span>{sw.board ?? sw.name}</span>
        {sw.version && <span className="muted">{sw.version}</span>}
        <Spacer />
        <span className="mono muted">{sw.host}</span>
        {cluster.demo && <Badge tone="neutral">Demo data</Badge>}
      </div>
      {sw.error && <Notice tone="error">{sw.error}</Notice>}
      {sw.configured === false && (
        <Notice action={{ label: "Open Settings", onClick: settings }}>This switch has no address yet, so SparkBox can't check that it answers. The ports below come from what your nodes hear.</Notice>
      )}
      {test && <Notice tone={test.ok ? "success" : "error"} onDismiss={() => setTest(null)}>{test.message}</Notice>}

      <div className="switch-main">
      <div className="switch-scroll">
        {gold ? (
          <ManagedPanel />
        ) : (
          <section className="sx-section" aria-label="Ports">
            <SwitchXPanel name={sw.name} ports={ports} selected={selected} onSelect={(n) => setSelected(selected === n ? null : n)} />
            <SwitchXLegend />
            <CloudNotice msp={view?.msp} onSettings={settings} />
            <p className="sx-sources muted">
              <span>Live from LLDP · Your labels · {cloudLine(view?.msp, fmtRelative)}</span>
              <HelpTip topic="mon.switch.x-cloud" label="Where each fact comes from" />
              {!view?.msp.configured && <button type="button" className="link" onClick={settings}>Set up</button>}
            </p>
            {(lldp.unplaced.length > 0 || !!view?.msp.sw?.skipped) && (
              <p className="sx-sources muted">
                {lldp.unplaced.length > 0 && `LLDP names a port SparkBox can't place: ${lldp.unplaced.join(", ")}. `}
                {!!view?.msp.sw?.skipped && `Firewalla listed ${view.msp.sw.skipped} port${view.msp.sw.skipped === 1 ? "" : "s"} SparkBox couldn't place.`}
              </p>
            )}
            <p className="sx-sources muted">
              Click a port for details. Changes to a port are made in the Firewalla app: this panel reads, and never changes the switch.
              <HelpTip topic="mon.switch.x-panel" label="About this panel" />
            </p>
          </section>
        )}

        <Tiles>
          <Tile label="Latency" value={sw.latencyMs != null ? sw.latencyMs.toFixed(2) : "—"} unit={sw.latencyMs != null ? "ms" : undefined} sub="TCP to port 443, every 5 s" />
          <Tile label="Loss" value={history?.lossPct != null ? history.lossPct.toFixed(0) : "—"} unit={history?.lossPct != null ? "%" : undefined} sub="Last 5 minutes" tone={history?.lossPct ? (history.lossPct > 10 ? "crit" : "warn") : undefined} />
          <Tile label="Uptime" value={history?.uptimePct != null ? history.uptimePct.toFixed(2) : "—"} unit={history?.uptimePct != null ? "%" : undefined} sub="Last 24 hours" />
          <Tile
            label="Last outage"
            title={outage ? `${fmtDateTime(outage.startMs)}: ${outage.reason}` : undefined}
            value={outage ? relTime(outage.startMs) : "None seen"}
            sub={outage ? `${outage.endMs ? fmtDuration(Math.round((outage.endMs - outage.startMs) / 1000)) : "Still down"} · ${outage.reason}` : "Since SparkBox started watching"}
            tone={outage && !outage.endMs ? "crit" : undefined}
          />
          {gold && (
            <Tile
              label="VPN round trip"
              value={vpnRtt != null ? vpnRtt.toFixed(0) : "—"}
              unit={vpnRtt != null ? "ms" : undefined}
              sub={vpn?.state === "connected" ? "Through the tunnel to cell1" : vpn?.state === "direct" ? "Not needed: on the home network" : "Tunnel not connected"}
            />
          )}
        </Tiles>

        <div className="fw-chart">
          <TimeChart
            title="Latency"
            subtitle={samples.some((s) => s.ms == null) ? "Gaps are lost probes" : "No lost probes"}
            ts={samples.map((s) => s.tsMs / 1000)}
            relative
            height={150}
            fmt={(v) => `${v.toFixed(2)} ms`}
            // One slow probe shouldn't flatten the rest against the floor: it gets a ▲ instead.
            robust="auto"
            series={[{ label: "Round trip", color: "--accent", values: samples.map((s) => s.ms) }]}
          />
        </div>

        <h4 className="section-title">LAN health, from each node's side <HelpTip topic="mon.switch.lan-health" /></h4>
        <div className="fw-lan" style={{ height: 66 + rows.length * 30 }}>
          <Table columns={lanColumns} rows={rows} rowKey={(r) => r.node.id} tableId="switch.lan" ariaLabel="LAN links of each node" footer itemNoun={["node", "nodes"]} />
        </div>
      </div>
      {port && <SwitchXDrawer p={port} switchId={sw.id} cluster={cluster} msp={view?.msp} onClose={closeDrawer} onSaved={reload} />}
      </div>
    </div>
  );
}

/** The Gold's greyed 4-port drawing: there to orient, not to click. */
function ManagedPanel() {
  return (
    <figure className="managed-panel" aria-label="Ports are managed in the Firewalla app">
      <svg viewBox="0 0 520 64" aria-hidden preserveAspectRatio="xMinYMid meet">
        <rect className="mp-chassis" x={1} y={1} width={518} height={58} rx={7} />
        <text className="mp-brand" x={16} y={35}>Firewalla Gold</text>
        {Array.from({ length: 4 }, (_, i) => (
          <g key={i}>
            <rect className="mp-port" x={150 + i * 44} y={16} width={34} height={28} rx={3} />
            <rect className="mp-port-tab" x={160 + i * 44} y={40} width={14} height={4} />
            <text className="mp-port-label" x={167 + i * 44} y={56} textAnchor="middle">{i + 1}</text>
          </g>
        ))}
      </svg>
      <figcaption>
        Managed in the Firewalla app
        <HelpTip topic="mon.switch.firewalla-ports" label="Why ports are managed there" />
      </figcaption>
    </figure>
  );
}
