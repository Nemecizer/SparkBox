import { useEffect, useLayoutEffect, useRef, useState, type MouseEvent, type ReactNode, type RefObject } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "../lib/api";
import { nodeMenu, showContextMenu } from "../lib/contextMenu";
import { fmtMs } from "../lib/format";
import { statusTooltip, switchDisplayStatus, useHealth, useHealthSummary, type HealthState } from "../lib/health";
import { useEndpoints, type Endpoint } from "../lib/endpoints";
import { nodeNotes, nodeTone } from "../lib/metrics";
import { netApi } from "../lib/netApi";
import { useServerView } from "../lib/server";
import { requestPower } from "../lib/power";
import { getCluster, useCluster, useSnapshotAge } from "../lib/store";
import { openTerminal } from "../lib/terminals";
import type { ClusterSnapshot, NodeSnapshot, SwitchSnapshot } from "../lib/types";
import { linkWord, offlineReason, powerLabel } from "../lib/nodeStatus";
import { inMaintenance, maintenanceSpan } from "../lib/maintenance";
import { protocolName, useVpnStatus, VPN_DOT, VPN_LABEL } from "../lib/vpn";
import { awakeLine, useAwake } from "../lib/awake";
import { openTasksWindow } from "../lib/tasks";
import { useWM, type WM } from "../wm/WindowManager";
import { showFilesOn } from "../windows/FilesWindow";
import { openOnNode } from "../windows/NodeDetail";
import { DotGlyph, StateGlyph, StatusDot, type DotStatus, type GlyphState } from "./ui";
import { TasksChip } from "./TasksChip";
import "../windows/monitoring.css";

/** Older than this, the numbers on screen are called stale. */
const STALE_AFTER_MS = 3000;

/** "22 ms", "Stale 8 s", "Off (planned)", "Restarting", "Offline" (an SSH timeout too), "Error" (a host key or a login). */
export function nodeText(n: NodeSnapshot): string {
  if (inMaintenance(n)) return "In maintenance";
  if (n.status === "online" && n.stale) return `Stale ${Math.round((n.frameAgeMs ?? 0) / 1000)} s`;
  if (n.status === "online") return n.latencyMs != null ? fmtMs(n.latencyMs) : "Online";
  const planned = powerLabel(n);
  if (planned) return planned;
  return linkWord(n);
}

/** Words for a node this Mac can't reach while its own path is the problem (health/reach.rs). */
export function notConfirmed(n: NodeSnapshot, cluster: Pick<ClusterSnapshot, "macReach"> | null): string | null {
  const r = cluster?.macReach;
  if (!r || r.state === "ok" || r.state === "waking" || linkWord(n) !== "Offline") return null;
  return `not confirmed down: ${r.title.replace(/^This Mac/, "this Mac")}`;
}

/**
 * The latency chip's own explanation: SparkBox's last command over SSH. The VPN window's TCP
 * connect time isn't quoted here until there's a figure for each node: it once showed one
 * node's probe for all of them.
 */
export function latencyTitle(n: NodeSnapshot): string | null {
  if (n.status !== "online" || n.latencyMs == null) return null;
  return `${fmtMs(n.latencyMs)}: round trip of SparkBox's last command over SSH.`;
}

function switchText(s: SwitchSnapshot): string {
  if (s.configured === false) return "Not set up";
  if (s.status !== "online") return s.status === "connecting" ? "Connecting" : "Unreachable";
  return s.latencyMs != null ? `${s.latencyMs < 10 ? s.latencyMs.toFixed(1) : s.latencyMs.toFixed(0)} ms` : "Online";
}

/** How much the device chips give up to fit: 0 full, 1 dot and name, 2 the dot alone. */
export type Compact = 0 | 1 | 2;

/** Healthy is quiet; only a warning, a critical or an unreachable device is coloured. */
export function chipTone(st: DotStatus | string): "ok" | "warn" | "crit" {
  if (st === "warn" || st === "stale") return "warn";
  if (st === "crit" || st === "error" || st === "offline") return "crit";
  return "ok";
}

// ---- A node's own state (§2.3): reachability and the node's own alerts, never a model's

/** Alerts about something that runs on a node rather than the node: they never colour it. */
const NOT_THE_NODE = new Set(["endpoint", "container"]);

/**
 * A node's glyph in the status bar and the sidebar: from nodeTone (reachability, then the
 * node's own alerts), the loud offline glyph when it's unreachable, a grey wrench while
 * SparkBox is restarting it.
 */
export function nodeGlyph(n: NodeSnapshot, health: HealthState | null, cluster: Pick<ClusterSnapshot, "nodes"> | null): GlyphState {
  // Maintenance mode (P1.16): planned whether it's up or not.
  if (inMaintenance(n)) return "planned";
  if (n.status !== "online" && n.maintenance) return "planned";
  return nodeTone(n.id, health, cluster);
}

const GLYPH_STATUS: Record<GlyphState, string> = {
  ok: "Online", warn: "Warning", crit: "Critical", offline: "Offline", planned: "Restarting (SparkBox)",
  affected: "Affected", recovering: "Recovering", silenced: "Silenced",
};

/** The word for a node's glyph: "Offline", "Restarting (SparkBox)", "Warning"… */
export function nodeGlyphWord(n: NodeSnapshot, g: GlyphState): string {
  if (g === "planned" && n.maintenance && inMaintenance(n)) return `In maintenance ${maintenanceSpan(n.maintenance)}`;
  if (g === "offline") return n.powerState === "off" ? "Off" : linkWord(n) === "Error" ? "Error" : "Offline";
  return GLYPH_STATUS[g];
}

/**
 * A node's tooltip: its state and its own issues, then a neutral note for each model it serves
 * that isn't Ready ("Serving llama-3.3-70b · down").
 */
export function nodeTitle(n: NodeSnapshot, g: GlyphState, health: HealthState | null, endpoints: readonly Endpoint[], extra?: string | null): string {
  const own = (health?.nodes[n.id]?.issues ?? []).filter((i) => {
    const a = health?.alerts.find((x) => x.id === i.alertId);
    return !a || !NOT_THE_NODE.has(a.entity.kind);
  });
  const head = `${n.name} — ${nodeGlyphWord(n, g)}${extra ? ` · ${extra}` : ""}`;
  const lines = own.slice(0, 6).map((i) => `${i.severity === "crit" ? "Critical" : "Warning"}: ${i.summary}`);
  if (own.length > 6) lines.push(`and ${own.length - 6} more`);
  const notes = nodeNotes(n.id, health, endpoints).map((x) => (x.reason ? `${x.text}: ${x.reason}` : x.text));
  return [head, ...lines, ...notes].join("\n");
}

/** A device in the status bar. Its tooltip always has the full story, however compact it is. */
function DeviceChip({ status, glyph, name, extra, title, compact, onClick, onContextMenu }: {
  status: DotStatus;
  /** A node's own state (nodeGlyph), drawn instead of the link dot. */
  glyph?: GlyphState;
  name: string;
  extra?: ReactNode;
  title: string;
  compact: Compact;
  onClick: () => void;
  onContextMenu?: (e: MouseEvent) => void;
}) {
  const tone = glyph ? (glyph === "offline" || glyph === "crit" ? "crit" : glyph === "warn" ? "warn" : "ok") : chipTone(status);
  return (
    <button type="button" className={`sb-item sb-dev ${tone}`} onClick={onClick} onContextMenu={onContextMenu} title={title} aria-label={title}>
      {glyph ? <span className="status"><StateGlyph state={glyph} label={title.split("\n")[0]} /></span> : <StatusDot status={status} />}
      {compact < 2 && <span className="sb-dev-name">{name}</span>}
      {compact < 1 && extra != null && <span className="sb-dev-extra num">{extra}</span>}
    </button>
  );
}

/**
 * Fits a bar on one line: when it overflows, it steps to the next compact level (the status
 * bar's chips drop their latency, then their names; the top bar's tools their labels). It
 * starts over from full whenever the bar is resized or gains or loses an item (a task chip,
 * a scenario pill), so a new item squeezes the others instead of hiding.
 */
export function useCompact(): [RefObject<HTMLElement | null>, Compact] {
  const ref = useRef<HTMLElement | null>(null);
  const [level, setLevel] = useState<Compact>(0);
  useLayoutEffect(() => {
    const bar = ref.current;
    if (bar && level < 2 && bar.scrollWidth > bar.clientWidth + 1) setLevel((l) => Math.min(2, l + 1) as Compact);
  });
  useEffect(() => {
    const bar = ref.current;
    if (!bar) return;
    let width = bar.clientWidth;
    const ro = new ResizeObserver(() => {
      if (bar.clientWidth === width) return;
      width = bar.clientWidth;
      setLevel(0);
    });
    const mo = new MutationObserver(() => setLevel(0));
    ro.observe(bar);
    mo.observe(bar, { childList: true });
    return () => {
      ro.disconnect();
      mo.disconnect();
    };
  }, []);
  return [ref, level];
}

/** The scenario's title, e.g. "Disk filling" for "disk-filling". */
export function useScenarioTitles(enabled: boolean): Map<string, string> {
  const [titles, setTitles] = useState(new Map<string, string>());
  useEffect(() => {
    if (!enabled || !inTauri) return;
    netApi.demoScenariosList().then((l) => setTitles(new Map(l.map((s) => [s.id, s.title])))).catch(() => {});
  }, [enabled]);
  return titles;
}

/** Problems as the top bar counts them: incidents, not their children (known issues and recovering ones aside). */
function AlertChips({ summary, onOpen }: { summary: { warn: number; crit: number } | null; onOpen: () => void }) {
  if (!summary) return null;
  const { warn, crit } = summary;
  if (!warn && !crit) {
    return (
      <button type="button" className="sb-item sb-alerts ok" onClick={onOpen} title="No active alerts — open Alerts">
        <DotGlyph status="online" word="Healthy" />
        <span>Healthy</span>
      </button>
    );
  }
  const words = [crit ? `${crit} critical` : null, warn ? `${warn} warning${warn === 1 ? "" : "s"}` : null].filter(Boolean).join(", ");
  return (
    <button type="button" className="sb-item sb-alerts" onClick={onOpen} title={`${words} — open Alerts`} aria-label={`${words}. Open Alerts`}>
      {crit > 0 && (
        <span className="sb-alert crit">
          <DotGlyph status="crit" word="Critical" />
          <span className="num">{crit}</span>
        </span>
      )}
      {warn > 0 && (
        <span className="sb-alert warn">
          <DotGlyph status="warn" word="Warning" />
          <span className="num">{warn}</span>
        </span>
      )}
    </button>
  );
}

/** "Live · 1 s" with a heartbeat; amber "Stale 8 s" once the newest snapshot is over 3 s old. */
function Freshness({ demo }: { demo: boolean }) {
  const age = useSnapshotAge();
  if (age == null) return <span className="sb-fresh waiting">Waiting for data…</span>;
  const stale = age > STALE_AFTER_MS;
  const source = demo ? "Demo" : "Live";
  return (
    <span
      className={`sb-fresh${stale ? " stale" : ""}`}
      role="status"
      title={stale ? `No new readings for ${Math.round(age / 1000)} s. What's shown is the last thing received.` : `${source} data, updated every second`}
    >
      <span className={`sb-pulse${stale ? "" : " beat"}`} aria-hidden />
      {stale ? `Stale ${Math.round(age / 1000)} s` : `${source} · 1 s`}
    </span>
  );
}

/**
 * "Keeping this Mac awake while Restart cell2 runs" while SparkBox holds a sleep assertion for
 * an orchestration (awake.rs). Compact, it reads "Keeping this Mac awake"; it opens the task.
 */
function AwakeChip({ compact }: { compact: Compact }) {
  const holds = useAwake();
  const line = awakeLine(holds);
  if (!line) return null;
  return (
    <button type="button" className="sb-item" onClick={() => openTasksWindow(holds[0].task ?? undefined)} title={`${line}. macOS still sleeps if you choose Sleep.`} aria-label={line}>
      {compact < 1 ? line : "Keeping this Mac awake"}
    </button>
  );
}

/** Node actions picked in the menu bar menu that need the window (a shell, a confirmation). */
function useTrayNodeActions(wm: WM) {
  useEffect(() => {
    if (!inTauri) return;
    const stop = listen<{ node: string; action: string }>("tray://node-action", ({ payload: { node, action } }) => {
      const n = getCluster()?.nodes.find((x) => x.id === node);
      if (action === "terminal") {
        openTerminal(node);
        wm.open("terminal");
      } else if (action === "files") {
        showFilesOn(node);
        wm.open("files");
      } else if (action === "dgx" && n) {
        void openOnNode(n, 11000, getCluster()?.demo ?? true, () => wm.open("settings", { section: "data" }));
      } else if (action === "reboot") {
        wm.open("node", { id: node });
        void requestPower(node, "reboot");
      }
    });
    return () => {
      stop.then((f) => f()).catch(() => {});
    };
  }, [wm]);
}

export function StatusBar() {
  const cluster = useCluster();
  const health = useHealth();
  const summary = useHealthSummary();
  const endpoints = useEndpoints();
  const wm = useWM();
  const vpn = useVpnStatus();
  const server = useServerView();
  const [barRef, compact] = useCompact();
  useTrayNodeActions(wm);

  return (
    <footer className="statusbar" aria-label="Status" ref={barRef}>
      {cluster?.nodes.map((n) => {
        const g = nodeGlyph(n, health, cluster);
        const why = n.status === "online" ? latencyTitle(n) ?? nodeText(n) : [powerLabel(n) ?? offlineReason(n), notConfirmed(n, cluster), n.error].filter(Boolean).join(" · ");
        return (
          <DeviceChip
            key={n.id}
            status="online"
            glyph={g}
            name={n.name}
            extra={nodeText(n)}
            compact={compact}
            onClick={() => wm.open("node", { id: n.id })}
            onContextMenu={(e) => showContextMenu(e, nodeMenu(n))}
            title={nodeTitle(n, g, health, endpoints, why)}
          />
        );
      })}
      {cluster?.switches.map((s) => {
        const st = switchDisplayStatus(s, health);
        return (
          <DeviceChip
            key={s.id}
            status={st}
            name={s.name}
            extra={switchText(s)}
            compact={compact}
            onClick={() => (s.configured === false ? wm.open("settings", { section: "switches" }) : wm.open("switch", { id: s.id, name: s.name }))}
            title={`${statusTooltip(s.name, st, health?.switches[s.id]?.issues ?? [], s.error ?? null)} · ${switchText(s)}`}
          />
        );
      })}
      {vpn && (
        <DeviceChip
          status={VPN_DOT[vpn.state]}
          name="VPN"
          extra={vpn.state === "connected" ? `${protocolName(vpn.protocol)}${vpn.latencyMs != null ? ` · ${vpn.latencyMs.toFixed(0)} ms` : ""}` : VPN_LABEL[vpn.state]}
          compact={compact}
          onClick={() => wm.open("vpn")}
          title={`VPN — ${VPN_LABEL[vpn.state]}${vpn.message ? `: ${vpn.message}` : ""}`}
        />
      )}
      {server?.running && (
        <DeviceChip status="online" name="Server" extra={`:${server.settings.port}`} compact={compact} onClick={() => wm.open("server")} title={`SparkBox Server — running at ${server.url ?? `port ${server.settings.port}`}`} />
      )}
      <div className="spacer" />
      <TasksChip />
      <AwakeChip compact={compact} />
      <AlertChips summary={summary} onOpen={() => wm.open("alerts")} />
      {cluster ? <Freshness demo={cluster.demo} /> : <span className="sb-fresh waiting">Waiting for data…</span>}
    </footer>
  );
}
