import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { History, LayoutTemplate } from "lucide-react";
import { showContextMenu } from "../lib/contextMenu";
import { useHealthSummary } from "../lib/health";
import { shortcutFor } from "../lib/keymap";
import { useEndpoints } from "../lib/endpoints";
import { gpuServing } from "../lib/metrics";
import { useCluster } from "../lib/store";
import { plural } from "../lib/vocabulary";
import { latestFailed, useDeployments } from "../lib/deployments";
import { modelsServing, topSummary } from "./TopBar";
import { useVpnStatus, VPN_LABEL } from "../lib/vpn";
import { closedGroups, lastLayout, useWMActions, useWMState, type WinState } from "../wm/WindowManager";
import { BrandMark } from "./BrandMark";
import "./shell.css";

/** The six places on ⌘1–⌘6, in the Go menu's order. */
export const LAUNCHER_TILES: { kind: string; blurb: string }[] = [
  { kind: "dashboard", blurb: "Every node at a glance" },
  { kind: "topology", blurb: "Nodes, switches and links" },
  { kind: "containers", blurb: "Docker on every node" },
  { kind: "workloads", blurb: "Processes and listening ports" },
  { kind: "models", blurb: "Find, copy and serve models" },
  { kind: "terminal", blurb: "SSH into any node" },
];

const RECENT_MAX = 8;

/**
 * How many chips fit on one line, keeping room for a "+N" chip when some don't.
 * `widths` are each chip's width, `room` the line's, `label` what the line starts with.
 */
export function chipsThatFit(widths: number[], room: number, label: number, gap = 6, more = 40): number {
  let used = label;
  for (let i = 0; i < widths.length; i++) {
    const tail = i < widths.length - 1 ? gap + more : 0;
    if (used + gap + widths[i] + tail > room) return i;
    used += gap + widths[i];
  }
  return widths.length;
}

/** What an empty desktop shows: the cluster in one line, the main places, and a way back to what was open. */
export function Launcher() {
  const wm = useWMActions();
  useWMState(); // re-render as windows close, so "recently closed" stays current
  const cluster = useCluster();
  const health = useHealthSummary();
  const vpn = useVpnStatus();

  const nodes = cluster?.nodes ?? [];
  const online = nodes.filter((n) => n.status === "online").length;
  // Serving means a Ready endpoint that answered lately, counted over its member GPUs.
  const endpoints = useEndpoints();
  const serving = cluster ? gpuServing(cluster, endpoints).count : 0;
  const failed = latestFailed(useDeployments());
  const last = useMemo(() => lastLayout(), []);
  const recent = useMemo(() => {
    const seen = new Set<string>();
    const out: WinState[] = [];
    for (const g of closedGroups()) {
      for (const w of g.wins) {
        if (seen.has(w.id) || out.length >= RECENT_MAX) continue;
        seen.add(w.id);
        out.push(w);
      }
    }
    return out;
  }, []);

  // One line of chips: every chip stays rendered (the ones that don't fit out of the flow, so
  // they can still be measured), and "+N" lists the rest.
  const rowRef = useRef<HTMLDivElement>(null);
  const [fit, setFit] = useState(recent.length);
  useLayoutEffect(() => {
    const row = rowRef.current;
    if (!row) return;
    const measure = () => {
      const widths = [...row.querySelectorAll<HTMLElement>("[data-chip]")].map((c) => c.offsetWidth);
      const label = row.querySelector<HTMLElement>(".launcher-label")?.offsetWidth ?? 0;
      setFit(row.clientWidth ? chipsThatFit(widths, row.clientWidth, label) : recent.length);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(row);
    row.querySelectorAll("[data-chip]").forEach((c) => ro.observe(c));
    return () => ro.disconnect();
  }, [recent.length]);
  const hiddenRecent = recent.slice(fit);

  const crit = health?.crit ?? 0;
  const warn = health?.warn ?? 0;
  const summary = topSummary({ online, total: nodes.length, planned: nodes.filter((n) => n.status !== "online" && n.maintenance).length,
    models: cluster ? modelsServing(endpoints, cluster.tsMs) : [], gpus: serving, crit, warn, recovering: health?.recovering, known: health?.known });
  return (
    <div className="launcher" role="region" aria-label="Start">
      <div className="launcher-card">
        <BrandMark size={36} className="launcher-mark" />
        <p className="launcher-health" aria-live="polite">
          {cluster ? (
            <>
              <span title={summary.tooltip}>{summary.parts.map((p) => p.text).join(" · ")}</span>
              {failed && <> · <button className="btn small" onClick={() => wm.open("models", { tab: "loaded" })}>1 failed deployment</button></>}
              {vpn && <> · VPN {VPN_LABEL[vpn.state].toLowerCase()}</>}
            </>
          ) : "Waiting for the cluster…"}
        </p>
        <div className="launcher-grid">
          {LAUNCHER_TILES.map((t) => {
            const Icon = wm.iconFor(t.kind);
            const keys = shortcutFor(`go:${t.kind}`);
            return (
              <button key={t.kind} className="launcher-tile" onClick={() => wm.open(t.kind)} data-autofocus={t.kind === "dashboard" || undefined}>
                {Icon && <Icon size={22} strokeWidth={1.6} aria-hidden />}
                <span className="launcher-tile-title">{wm.titleFor({ id: t.kind, kind: t.kind, params: {} })}</span>
                <span className="launcher-tile-blurb">{t.blurb}</span>
                {keys && <kbd className="kbd launcher-tile-key">{keys}</kbd>}
              </button>
            );
          })}
        </div>
        {(last.length > 0 || recent.length > 0) && (
          <div className="launcher-more">
            {last.length > 0 && (
              <button className="btn launcher-restore" onClick={() => wm.restoreWindows(last)}>
                <LayoutTemplate size={14} aria-hidden /> Restore last layout <span className="muted">({plural(last.length, "window")})</span>
              </button>
            )}
            {recent.length > 0 && (
              <div className="launcher-recent" ref={rowRef}>
                <span className="launcher-label"><History size={13} aria-hidden /> Recently closed</span>
                {recent.map((w, i) => {
                  const Icon = wm.iconFor(w.kind);
                  const away = i >= fit;
                  return (
                    <button
                      key={w.id}
                      data-chip
                      className={`chip launcher-chip${away ? " launcher-chip-away" : ""}`}
                      aria-hidden={away || undefined}
                      tabIndex={away ? -1 : undefined}
                      onClick={() => wm.restoreWindows([w])}
                      title={`Reopen ${wm.titleFor(w)}`}
                    >
                      {Icon && <Icon size={12} aria-hidden />} {wm.titleFor(w)}
                    </button>
                  );
                })}
                {hiddenRecent.length > 0 && (
                  <button
                    className="chip launcher-chip launcher-chip-more"
                    aria-label={`${hiddenRecent.length} more recently closed`}
                    onClick={(e) => showContextMenu(e, hiddenRecent.map((w) => ({ label: wm.titleFor(w), icon: wm.iconFor(w.kind), run: () => wm.restoreWindows([w]) })))}
                  >
                    +{hiddenRecent.length}
                  </button>
                )}
              </div>
            )}
          </div>
        )}
        <div className="launcher-keys">
          <span><kbd className="kbd">⌘K</kbd> Search and run anything</span>
          <span><kbd className="kbd">⌘`</kbd> Next window</span>
          <span><kbd className="kbd">⌘,</kbd> Settings</span>
          <span><kbd className="kbd">⌘/</kbd> All shortcuts</span>
        </div>
      </div>
    </div>
  );
}
