import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { AppWindow, ChevronDown, Columns2, Ellipsis, Eye, Grid2x2, Layers, LayoutGrid, Lock, LockOpen, Search, SquareX } from "lucide-react";
import { pushAgain, sendTestPage, useWatchState, watchBadge, type WatchBadge, type WatchState } from "../lib/agent";
import { padlockText, showLookOnly } from "../lib/admin";
import { getCommand, runCommand } from "../lib/commands";
import { menuAt, type MenuItem } from "../lib/contextMenu";
import { useHealthSummary } from "../lib/health";
import { shortcutFor } from "../lib/keymap";
import { fmtCountdown } from "../lib/format";
import { useEndpoints } from "../lib/endpoints";
import { deploymentLabel, latestFailed, useDeployments } from "../lib/deployments";
import { gpuServing, SERVING_FRESH_MS, type ServingEndpoint } from "../lib/metrics";
import { useAdminState } from "../lib/admin";
import { useCluster } from "../lib/store";
import type { ScenarioPhase } from "../lib/types";
import { plural } from "../lib/vocabulary";
import { useWMActions, useWMState, type WMActions } from "../wm/WindowManager";
import { BrandMark } from "./BrandMark";
import { useScenarioTitles } from "./StatusBar";
import { tileAllItems, useLayoutNames, WindowsMenu, windowsMenuItems } from "./WindowsMenu";
import "./shell.css";

/** Ticks once a second while `on`, so a countdown re-renders. */
function useTick(on: boolean) {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!on) return;
    const t = window.setInterval(() => setTick((n) => n + 1), 1000);
    return () => window.clearInterval(t);
  }, [on]);
}

/** Time left until `until` (Unix ms), worked out from the clock on every render. */
export function countdown(until: number, now = Date.now()): string {
  return fmtCountdown((until - now) / 1000);
}

/** "Node offline · fault · recovers in 0:42": the scenario and where it is in its cycle. */
export function scenarioBadgeText(title: string, phase: ScenarioPhase | null | undefined): string {
  if (!phase) return title;
  const next = phase.nextChangeS;
  const label = phase.label || title;
  if (next == null) return label;
  const when = `${Math.floor(next / 60)}:${String(next % 60).padStart(2, "0")}`;
  const verb = phase.phase === "fault" ? "recovers" : phase.phase === "recovering" ? "healthy" : "repeats";
  return `${label} · ${verb} in ${when}`;
}

type Open = (kind: string, params?: Record<string, string>) => void;

/**
 * The watch badge's action: Settings › Notifications, Push again (the current alert settings to
 * every agent), or a test page from every node through every phone channel.
 */
export async function runWatchAction(kind: NonNullable<WatchBadge["action"]>["kind"], open: Open, watch: WatchState): Promise<void> {
  const settings = () => open("settings", { section: "notifications" });
  if (kind === "settings") return settings();
  // Push again is the agents' own single-flight push: one toast, when its task ends (RM8).
  if (kind === "push") return pushAgain();
  // The same confirm (with its message count), unlock and toast as Settings › Notifications.
  await sendTestPage(watch, { label: "Open Settings", run: settings });
}

/**
 * The models being served: distinct models of endpoints that are Ready and answered in the last
 * minute. For Ollama, the ones loaded in memory, not every model it has installed.
 */
export function modelsServing(endpoints: readonly (ServingEndpoint & { engine?: string; models: string[]; ollama?: { name: string }[] | null })[], nowMs: number): string[] {
  const out = new Set<string>();
  for (const e of endpoints) {
    if (e.health !== "ready") continue;
    if (e.lastOkMs !== undefined && (e.lastOkMs == null || nowMs - e.lastOkMs > SERVING_FRESH_MS)) continue;
    for (const m of e.engine === "ollama" && e.ollama ? e.ollama.map((o) => o.name) : e.models) out.add(m);
  }
  return [...out];
}

export interface TopSummaryInput {
  online: number;
  total: number;
  /** Of those not online, how many SparkBox is restarting or has shut down (planned work: never tinted). */
  planned?: number;
  /** Models served (modelsServing), apart from the GPUs serving them. */
  models: string[];
  gpus: number;
  /** Incidents (useHealthSummary): recovering ones and known issues aren't counted, only told. */
  crit: number;
  warn: number;
  recovering?: number;
  known?: number;
}

/**
 * The top bar's centre: "4/4 online · 2 models · 4 GPUs serving · 1 critical · 2 warnings",
 * counting incidents. Recovering ones and known issues (silenced or acknowledged) go in the
 * tooltip only.
 */
export function topSummary(i: TopSummaryInput): { parts: { text: string; tone?: "warn" | "crit" }[]; tooltip: string } {
  const parts: { text: string; tone?: "warn" | "crit" }[] = [{ text: `${i.online}/${i.total} online`, tone: i.online + (i.planned ?? 0) < i.total ? "crit" : undefined }];
  if (i.models.length) {
    parts.push({ text: plural(i.models.length, "model") }, { text: `${plural(i.gpus, "GPU")} serving` });
  } else {
    parts.push({ text: "no model serving" });
  }
  if (i.crit) parts.push({ text: `${i.crit} critical`, tone: "crit" });
  if (i.warn) parts.push({ text: plural(i.warn, "warning"), tone: "warn" });
  const lines = [
    `${i.online} of ${plural(i.total, "node")} online${i.planned ? ` (${i.planned} restarting or shut down by SparkBox)` : ""}`,
    i.models.length ? `Serving ${i.models.join(", ")} on ${plural(i.gpus, "GPU")}` : "No model is answering",
    i.crit || i.warn ? `${[i.crit ? `${i.crit} critical` : "", i.warn ? plural(i.warn, "warning") : ""].filter(Boolean).join(" · ")} (incidents; related alerts count once)` : "No problems",
  ];
  if (i.recovering) lines.push(`${i.recovering} recovering, not counted`);
  if (i.known) lines.push(`${plural(i.known, "known issue")} (silenced or acknowledged), not counted`);
  return { parts, tooltip: lines.join("\n") };
}

function tileItems(wm: WMActions, visible: number): MenuItem[] {
  const few = visible >= 2 ? false : "Needs two windows";
  const side = visible >= 2 ? wm.tilePreview("two") : null;
  return [
    ...tileAllItems(wm, visible),
    { label: side?.stacked ? "Side by side — stacks here, too narrow" : "Side by side", icon: Columns2, disabled: few, run: () => wm.tile("two") },
    { label: "Tile 4", icon: Grid2x2, disabled: few, run: () => wm.tile("four") },
    { separator: true },
    { label: "Cascade", icon: Layers, shortcut: shortcutFor("wm:cascade"), disabled: visible ? false : "No windows are open", run: () => wm.cascade() },
    { label: "Restore layout", disabled: wm.canRestoreLayout() ? false : "Nothing has been tiled or cascaded", run: () => wm.restoreLayout() },
  ];
}

/**
 * How much the top bar has folded to fit (C4), one step at a time, in this order:
 * 1 the watch badge's action goes; 2 the status reads short ("4/4 online · 2 warnings") and
 * the watch badge keeps only its icon; 3 the tools go icon-only; 4 Tile, Windows and Close
 * all fold into one "…" menu. Measured, not guessed from the width: a scenario pill or the
 * Admin countdown takes room too. A resize, or a pill coming or going, starts over.
 */
export const TOPBAR_FIT_MAX = 4;

function useTopBarFit(): [RefObject<HTMLElement | null>, number] {
  const ref = useRef<HTMLElement | null>(null);
  const [level, setLevel] = useState(0);
  useLayoutEffect(() => {
    const bar = ref.current;
    if (bar && level < TOPBAR_FIT_MAX && bar.scrollWidth > bar.clientWidth + 1) setLevel((l) => Math.min(TOPBAR_FIT_MAX, l + 1));
  });
  useEffect(() => {
    const bar = ref.current;
    if (!bar || typeof ResizeObserver === "undefined") return;
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

/**
 * The unified title bar: the traffic lights sit in its left padding, and empty
 * space drags the window (double-click zooms it, like any Mac title bar). Quiet
 * when all is well: only the unlocked Admin pill and real problem counts carry colour.
 */
export function TopBar({ scenarioTitle }: { scenarioTitle?: (id: string) => string }) {
  const cluster = useCluster();
  const adminState = useAdminState();
  const admin = adminState.unlocked;
  useTick(admin && adminState.expiresAtMs !== null);
  const left = admin && adminState.expiresAtMs !== null ? countdown(adminState.expiresAtMs) : null;
  const health = useHealthSummary();
  const watch = useWatchState();
  const wm = useWMActions();
  const { windows, activeId } = useWMState();
  const layouts = useLayoutNames();
  // Too much to show: the badge action, then the status text, then tool labels, then the tools fold (C4).
  const [barRef, fit] = useTopBarFit();

  const nodes = cluster?.nodes ?? [];
  const online = nodes.filter((n) => n.status === "online").length;
  const planned = nodes.filter((n) => n.status !== "online" && n.maintenance).length;
  // Serving means a Ready endpoint that answered lately: its models, and its member GPUs.
  const endpoints = useEndpoints();
  const serving = cluster ? gpuServing(cluster, endpoints).count : 0;
  const models = cluster ? modelsServing(endpoints, cluster.tsMs) : [];
  const titles = useScenarioTitles(!!cluster?.demo);
  const scenario = cluster?.demo && cluster.scenario && cluster.scenario !== "default"
    ? (titles.get(cluster.scenario) ?? scenarioTitle?.(cluster.scenario) ?? cluster.scenario)
    : null;
  // Incidents, once the health summary counts them; recovering ones and known issues are told, not counted.
  const counts = health as (typeof health & { recovering?: number; known?: number }) | null;
  const crit = health?.crit ?? 0;
  const warn = health?.warn ?? 0;
  const problems = crit + warn > 0;
  const sum = topSummary({ online, total: nodes.length, planned, models, gpus: serving, crit, warn, recovering: counts?.recovering, known: counts?.known });
  const alertsOpen = !!getCommand("go:alerts");
  const visible = windows.filter((w) => !w.minimized).length;
  const badge = watchBadge(watch);
  const watching = watch.agents.some((a) => a.installed);
  // A deployment that failed in the last week: past, not an outage, so neutral and not counted (§3.4).
  const failed = latestFailed(useDeployments());
  const padlock = padlockText(adminState);
  const lookOnly = padlock === "Look only";
  // Folded (fit 2 and up): the online count and the problem counts, nothing else.
  const shownParts = fit >= 2 ? sum.parts.filter((p, i) => i === 0 || p.tone) : sum.parts;
  const folded = fit >= 4;

  return (
    <header className="topbar" data-tauri-drag-region="deep" ref={barRef} data-fit={fit || undefined}>
      <div className="brand" aria-label="SparkBox">
        <BrandMark size={18} />
      </div>
      {cluster && !cluster.demo && <span className="badge live" title="Numbers come from your Sparks over SSH">Live</span>}
      {cluster?.demo && <span className="tb-pill" title="Numbers come from the built-in simulator. Switch to Live in Settings › Data source.">Demo</span>}
      {scenario && (
        <button
          type="button"
          className="tb-pill tb-scenario"
          onClick={() => wm.open("settings", { section: "scenarios" })}
          title={`Demo scenario playing: ${scenario}. Change it in Settings › Demo scenarios.`}
        >
          {scenarioBadgeText(scenario, cluster?.scenarioPhase)}
        </button>
      )}
      <button
        className={`admin-pill${admin ? " on" : ""}${lookOnly ? " look-only" : ""}`}
        onClick={() => void (lookOnly ? showLookOnly() : runCommand("cluster:admin"))}
        title={lookOnly
          ? "Look only: SparkBox starts no change on the cluster or this Mac in this run. Click for why."
          : admin
            ? `Admin mode: actions can change the cluster. Click to lock now (it relocks itself after ${adminState.idleMinutes} idle minutes).`
            : `Read-only: SparkBox won't make a change you ask for. Click to unlock Admin mode (${shortcutFor("cluster:admin") ?? "⇧⌘L"}).`}
        aria-pressed={admin}
      >
        {admin && !lookOnly ? <LockOpen size={13} aria-hidden /> : <Lock size={13} aria-hidden />}
        {admin && !lookOnly ? <span>Admin{left && <span className="admin-left"> · {left}</span>}</span> : padlock}
      </button>
      <div className="topbar-drag" />
      {cluster && (
        <button
          className="topbar-summary"
          onClick={() => void runCommand(problems && alertsOpen ? "go:alerts" : "go:dashboard")}
          title={`${sum.tooltip}\n${problems && alertsOpen ? "Click to open Alerts" : "Click to open the Dashboard"}`}
        >
          {shownParts.map((p, i) => (
            <span key={p.text} className="topbar-part">
              {i > 0 && <span className="sep">·</span>}
              <span className={p.tone ? `${p.tone}-text` : undefined}>{p.text}</span>
            </span>
          ))}
        </button>
      )}
      {cluster && failed && (
        // Always in the bar, hidden by CSS from fit 2 on: a child that came and went with the fit
        // level would make the bar measure itself again from the start, over and over.
        <button
          type="button"
          className="tb-failed"
          onClick={() => wm.open("models", { tab: "loaded" })}
          title={`${deploymentLabel(failed)} failed${failed.exitCode != null ? ` (exit ${failed.exitCode})` : ""}. Click to open Models › Loaded for what happened.`}
        >
          <span className="sep" aria-hidden>·</span>1 failed deployment
        </button>
      )}
      {cluster && (
        // Who pages you when this Mac is asleep, and what that rests on (its tooltip); the action fixes what's missing.
        <span className={`tb-watch${badge.tone === "warn" ? " warn" : ""}`}>
          <button
            type="button"
            className="tb-watch-label"
            title={`${badge.lines.join("\n")}\nClick to open Settings › Notifications`}
            onClick={() => wm.open("settings", { section: "notifications" })}
          >
            {(watching || fit >= 2) && <Eye size={13} aria-hidden />}
            <span className="tb-watch-text">{badge.label}</span>
          </button>
          {badge.action && fit < 1 && (
            <button type="button" className="tb-link" onClick={() => void runWatchAction(badge.action!.kind, wm.open, watch)}>{badge.action.label}</button>
          )}
        </span>
      )}
      <div className="topbar-tools">
        <button className="tool topbar-search" onClick={() => void runCommand("app:palette")} title="Search windows, nodes, commands and Help (⌘K)">
          <Search size={14} aria-hidden />
          <span>Search</span>
          <kbd className="kbd">⌘K</kbd>
        </button>
        <span className="toolbar-sep" aria-hidden />
        {folded ? (
          // Too narrow for Tile, Windows and Close all: one "…" menu holds them all.
          <button
            type="button"
            className="tool tb-fold"
            aria-label="Windows and arranging"
            aria-haspopup="menu"
            title="Tile, the open windows, arranging and Close all"
            onClick={(e) => menuAt(e.currentTarget, [
              { label: "Tile", icon: LayoutGrid, disabled: visible ? false : "No windows are open", submenu: tileItems(wm, visible) },
              { label: "Windows", icon: AppWindow, submenu: windowsMenuItems(wm, windows, activeId, layouts) },
              { separator: true },
              { label: "Close all", icon: SquareX, disabled: windows.length ? false : "No windows are open", run: () => void wm.requestCloseAll() },
            ], { fromKeyboard: e.detail === 0 })}
          >
            <Ellipsis size={14} aria-hidden />
          </button>
        ) : (
          <>
            <span className="tb-split">
              <button className="tool" onClick={() => wm.tile("all")} disabled={!visible} title={`Tile windows without overlapping (${shortcutFor("wm:tile")})`}>
                <LayoutGrid size={14} aria-hidden /><span>Tile</span>
              </button>
              <button
                className="tool tb-split-more"
                aria-label="Tile options"
                aria-haspopup="menu"
                title="Tile all, side by side, tile 4, cascade, restore layout"
                onClick={(e) => menuAt(e.currentTarget.parentElement ?? e.currentTarget, tileItems(wm, visible), { fromKeyboard: e.detail === 0 })}
              >
                <ChevronDown size={12} aria-hidden />
              </button>
            </span>
            <WindowsMenu />
            <button
              className="tool"
              onClick={() => void wm.requestCloseAll()}
              disabled={!windows.length}
              title={`Close all ${plural(windows.length, "window")} (⌥⌘W); Undo brings them back`}
            >
              <SquareX size={14} aria-hidden /><span>Close all</span>
            </button>
          </>
        )}
      </div>
    </header>
  );
}
