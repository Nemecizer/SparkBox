import { useState } from "react";
import { FolderOpen, Gauge, Pause, Power, PowerOff, RotateCcw, SquareTerminal, Undo2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Icon } from "../components/Icon";
import { ChartGrid } from "../components/TimeChart";
import { AdvisoryGlyph, glyphWord, HelpTip, Notice, Segmented, Spacer, StateGlyph, Tabs, Toolbar, ToolButton, ToolSeparator, type GlyphState } from "../components/ui";
import { useAdmin, withAdmin } from "../lib/admin";
import { api } from "../lib/api";
import { useBoots, type BootEnd } from "../lib/boots";
import { RANGES, rangeSeconds, type RangeId } from "../lib/chartScale";
import { showError } from "../lib/errors";
import { fmtDuration } from "../lib/format";
import { useHealth, type Alert } from "../lib/health";
import { useEndpoints } from "../lib/endpoints";
import { nodeAlerts, nodeNotes, nodeTone } from "../lib/metrics";
import { linksIn, nodeActionFor, nodeAdvisories, offlineReason, throttleVerdict } from "../lib/nodeStatus";
import { requestPower } from "../lib/power";
import { confirmAction } from "../lib/session";
import { getCluster, useNode } from "../lib/store";
import { openTerminal } from "../lib/terminals";
import type { NodeSnapshot } from "../lib/types";
import { POWER } from "../lib/vocabulary";
import { useWM, useWinState, type Params } from "../wm/WindowManager";
import { Wrench } from "lucide-react";
import { askMaintenance, endMaintenance, inMaintenance, LIVE_ONLY, maintenanceText, quietText } from "../lib/maintenance";
import { showFilesOn } from "./FilesWindow";
import { GpuTab } from "./node/GpuTab";
import { CpuTab, DisksTab, MemoryTab } from "./node/HardwareTabs";
import { NetworkTab } from "./node/NetworkTab";
import { Overview } from "./node/Overview";
import type { ChartProps } from "./node/shared";
import { SystemTab } from "./node/SystemTab";
import { stateWord } from "./Topology";
import "./monitoring.css";

const TABS = [
  { id: "overview", label: "Overview" },
  { id: "cpu", label: "CPU" },
  { id: "memory", label: "Memory" },
  { id: "gpu", label: "GPU" },
  { id: "disks", label: "Disks" },
  { id: "network", label: "Network" },
  { id: "system", label: "System" },
] as const;
type TabId = (typeof TABS)[number]["id"];
/** Tabs with charts get the range and live/paused bar. */
const CHART_TABS = new Set<TabId>(["overview", "cpu", "memory", "gpu", "disks"]);

const asTab = (t: string | undefined): TabId => (TABS.find((x) => x.id === t?.toLowerCase())?.id ?? "overview");

/** Opens a page that only listens on the node, through an SSH tunnel, in the default browser. */
export async function openOnNode(node: NodeSnapshot, port: number, demo: boolean, openSettings?: () => void) {
  if (demo) {
    // Information, not an error: OK leaves it, "Switch to Live…" opens Settings › Data source.
    const live = await confirmAction({
      title: "DGX Dashboard isn't available in Demo mode",
      body: "In Demo mode there's no real node to connect to. With Live data, SparkBox opens NVIDIA's dashboard for this node in your browser through an SSH tunnel.",
      confirmLabel: "Switch to Live…",
      cancelLabel: "OK",
    });
    if (live) openSettings?.();
    return;
  }
  try {
    // The tunnel is a session on the node, logged like any other: node scope.
    await openUrl(await withAdmin(() => api.tunnelOpen(node.id, port), { action: "tunnel_open" }));
  } catch (e) {
    showError(e, { action: `Open port ${port} on ${node.name}`, node: node.id });
  }
}

/**
 * When the running boot began after a restart SparkBox didn't make ("6:08 PM" today, "Oct 28, 6:08 PM"
 * before), worded by how the boot before ended: without a shutdown it went on its own; a clean
 * shutdown is someone or something restarting it (you, an update).
 */
export function restartedWords(ms: number, end?: BootEnd | null, now: number = Date.now()): string {
  const d = new Date(ms);
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const when = d.toDateString() === new Date(now).toDateString() ? time : `${d.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
  if (end === "unclean") return `restarted on its own ${when} · no clean shutdown`;
  return end === "clean" ? `restarted outside SparkBox ${when} (clean shutdown)` : `restarted outside SparkBox ${when}`;
}

export function NodeDetail({ params }: { params: Params }) {
  const node = useNode(params.id);
  const [boots] = useBoots(params.id, !(getCluster()?.demo ?? true), node?.bootId);
  const health = useHealth();
  const endpoints = useEndpoints();
  const admin = useAdmin();
  const wm = useWM();
  const [tab, setTab] = useWinState<TabId>("tab", asTab(params.tab));
  const [range, setRange] = useWinState<RangeId>("range", "15m");
  const [zoom, setZoom] = useState<[number, number] | null>(null);
  const paramTab = `${params.tab}|${params.nav}`;
  const [lastParamTab, setLastParamTab] = useState(paramTab);
  // Opening the window again with a tab (a link from the Dashboard) switches to it.
  if (paramTab !== lastParamTab) {
    setLastParamTab(paramTab);
    if (params.tab) setTab(asTab(params.tab));
  }
  if (!node) return <div className="pad muted">{params.id} isn't in the inventory any more.</div>;

  // The node's own colour (M11): its link and its own alerts; a model that's down is a neutral note.
  const issues = nodeAlerts(node.id, health) as Alert[];
  const notes = nodeNotes(node.id, health, endpoints);
  const tone = nodeTone(node.id, health, getCluster());
  const glyph: GlyphState = node.status !== "online" && node.maintenance ? "planned" : tone !== "ok" ? tone : notes.length ? "affected" : "ok";
  const online = node.status === "online";
  const demo = getCluster()?.demo ?? true;
  const off = node.powerState === "off";
  const reason = online ? null : offlineReason(node);
  const action = online ? null : nodeActionFor(node);
  const links = online ? [] : linksIn(node.error).filter((l) => !(action && l.label.startsWith("Connect")));
  const advisories = nodeAdvisories(node);
  const chart: ChartProps = { rangeS: rangeSeconds(range), nowS: Date.now() / 1000, zoom, onZoom: setZoom };

  return (
    <div className="col fill mon-node">
      <Toolbar label={`${node.name} actions`}>
        <span className={`mon-node-state tone-${glyph}`}>
          <StateGlyph state={glyph} />
          {online ? (glyph === "affected" ? "Healthy" : glyphWord(glyph)) : stateWord(node)}
        </span>
        <span className="toolbar-text mon-node-id" title={`${node.system.hostname || node.host} · ${node.system.model}`}>
          {node.system.hostname || node.host} · {node.system.model}
        </span>
        {/* The uptime is never cut: the name and model above give way first. */}
        {online && node.system.uptimeS ? (
          <span className="mon-node-up">up {fmtDuration(node.system.uptimeS)}{boots?.restartedMs ? ` · ${restartedWords(boots.restartedMs, boots.restartedEnd)}` : ""}</span>
        ) : null}
        {advisories.length > 0 && (
          <button type="button" className="link-plain mon-node-notes" title={advisories.map((a) => a.title).join("\n")} onClick={() => setTab(advisories[0].tab)}>
            <AdvisoryGlyph title={advisories[0].title} /> {advisories.length} note{advisories.length === 1 ? "" : "s"}
          </button>
        )}
        <Spacer />
        <ToolButton icon={SquareTerminal} label="Terminal" locked={!admin} disabled={!online}
          title={online ? `Open a shell on ${node.name}` : `${node.name} isn't online`}
          onClick={() => {
            openTerminal(node.id);
            wm.open("terminal");
          }} />
        <ToolButton icon={FolderOpen} label="Files" title={`Browse ${node.name}'s files next to this Mac's`}
          onClick={() => {
            showFilesOn(node.id);
            wm.open("files");
          }} />
        <ToolButton icon={Gauge} label="DGX Dashboard" priority={2} locked={!admin} action="tunnel_open" disabled={demo || !online}
          title={demo ? "Needs Live data: the DGX Dashboard runs on the node" : online ? "NVIDIA's dashboard for this node, in your browser through an SSH tunnel" : `${node.name} isn't online`}
          onClick={() => openOnNode(node, 11000, demo, () => wm.open("settings", { section: "data" }))} />
        <HelpTip topic="mon.node.dgx-dashboard" />
        {/* Maintenance mode (P1.16): its alerts are planned; nothing runs on the node. */}
        <ToolButton icon={Wrench} label={inMaintenance(node) ? "End maintenance" : "Maintenance"} priority={3} locked={!admin}
          action={inMaintenance(node) ? "maintenance_end" : "maintenance_begin"} disabled={demo || (!!node.maintenance && !inMaintenance(node))}
          title={demo ? LIVE_ONLY : node.maintenance && !inMaintenance(node) ? "SparkBox's own restart window is open: its alerts are planned already" : inMaintenance(node) ? `End maintenance: ${maintenanceText(node)}` : `Put ${node.name} in maintenance: ${quietText(getCluster())}`}
          onClick={() => (inMaintenance(node) ? endMaintenance(node) : askMaintenance(node))} />
        <HelpTip topic="mon.node.maintenance" />
        <ToolSeparator />
        {!online && (
          <ToolButton icon={Power} label={POWER.wake} locked={!admin} title={`Send a Wake-on-LAN packet to ${node.name}`}
            onClick={() => requestPower(node.id, "wake")} />
        )}
        {!off && (
          <>
            <ToolButton icon={RotateCcw} label={POWER.restart} priority={2} locked={!admin} disabled={!online} title={online ? `Restart ${node.name}` : `${node.name} isn't online`}
              onClick={() => requestPower(node.id, "reboot")} />
            <ToolButton icon={PowerOff} label={POWER.shutdown} priority={3} danger locked={!admin} disabled={!online} title={online ? `Turn ${node.name} off` : `${node.name} isn't online`}
              onClick={() => requestPower(node.id, "poweroff")} />
          </>
        )}
        <HelpTip topic="ops.node.power" />
      </Toolbar>
      {inMaintenance(node) && <Notice tone="info">{maintenanceText(node)} · {quietText(getCluster())}</Notice>}
      {!online && (
        <Notice tone={action ? "info" : "error"}>
          <b>{stateWord(node)}</b>{reason ? ` · ${reason}` : ""}
          {node.error && reason !== node.error && <div className="mon-notice-detail" title={node.error}>{node.error}</div>}
          {(action || links.length > 0) && (
            <div className="mon-notice-actions">
              {action && <button type="button" className="btn small" onClick={() => action.run({ winId: undefined })}>{action.label}</button>}
              {links.map((l) => <button key={l.label} type="button" className="btn small" onClick={l.run}>{l.label}</button>)}
            </div>
          )}
        </Notice>
      )}
      {online && node.stale && <Notice tone="warn">No new readings for {Math.round((node.frameAgeMs ?? 0) / 1000)} s. The numbers below are the last ones received.</Notice>}
      {online && !node.stale && issues.length > 0 && (
        <Notice tone={issues.some((i) => i.severity === "crit") ? "error" : "warn"} action={{ label: "Alerts", onClick: () => wm.open("alerts") }}>
          {issues.map((i) => i.summary).join(" · ")}
        </Notice>
      )}
      {online && node.error && !node.stale && <Notice tone="warn">{node.error}</Notice>}
      {notes.map((n) => (
        <div key={n.text} className="mon-node-note" role="note">
          <StateGlyph state="affected" />
          <span>
            {n.text.slice(0, n.text.length - n.state.length)}<span className={`tone-text-${n.tone}`}>{n.state}</span>
            {n.reason ? <span className="muted"> · {n.reason}</span> : null}
          </span>
          <button type="button" className="link-plain" onClick={() => wm.open(n.link.kind, n.link.params)}>Endpoints</button>
        </div>
      ))}
      <Tabs label={`${node.name} sections`} value={tab} onChange={(t) => setTab(t as TabId)} tabs={TABS.map((t) => ({ id: t.id, label: t.label, badge: t.id === "gpu" && online && throttleVerdict(node.gpu).throttled ? "!" : undefined }))}>
        <ChartGrid>
        <div className="mon-node-scroll">
          {CHART_TABS.has(tab) && (
            <div className="mon-chartbar" role="toolbar" aria-label="Chart range">
              <Segmented<RangeId>
                label="Time range"
                value={range}
                onChange={(r) => {
                  setRange(r);
                  setZoom(null);
                }}
                options={RANGES.map((r) => ({ value: r.id, label: r.label, title: r.seconds >= 6 * 3600 ? "From saved history" : "From the last hour, every second" }))}
              />
              {zoom ? (
                <>
                  <span className="mon-paused"><Icon icon={Pause} size={12} />Paused at {new Date(zoom[0] * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}–{new Date(zoom[1] * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}</span>
                  <button type="button" className="btn small" onClick={() => setZoom(null)}><Icon icon={Undo2} size={12} />Back to live</button>
                </>
              ) : (
                <span className="mon-livetag"><span className="sb-pulse beat" aria-hidden />Live</span>
              )}
              <Spacer />
              <HelpTip topic="mon.node.charts" label="About these charts" />
            </div>
          )}
          {tab === "overview" && <Overview node={node} chart={chart} />}
          {tab === "cpu" && <CpuTab node={node} chart={chart} />}
          {tab === "memory" && <MemoryTab node={node} chart={chart} />}
          {tab === "gpu" && <GpuTab node={node} chart={chart} />}
          {tab === "disks" && <DisksTab node={node} chart={chart} />}
          {tab === "network" && <NetworkTab node={node} />}
          {tab === "system" && <SystemTab node={node} />}
        </div>
        </ChartGrid>
      </Tabs>
    </div>
  );
}
