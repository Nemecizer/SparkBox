import { useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  AppWindow, Columns2, Contrast, Eclipse, ExternalLink, FolderOpen, Grid2x2, Keyboard, LayoutGrid, Layers, Lock, LockOpen, Maximize2, Minimize2, Minus,
  Monitor, Moon, PanelLeft, PanelsTopLeft, RefreshCw, Router, Save, Search, Server, Settings, Sparkles, SquareTerminal,
  SquareX, Stethoscope, Sun, Type, Undo2, Wind, X, ZoomIn, ZoomOut, type LucideIcon,
  CircleHelp,
} from "lucide-react";
import { CommandPalette } from "./components/CommandPalette";
import { ConfirmHost } from "./components/ConfirmHost";
import { ContextMenuHost } from "./components/ContextMenuHost";
import { CoreBanner, SettingsFileBanner } from "./components/CoreBanner";
import { FenceBanner } from "./components/FenceBanner";
import { Launcher } from "./components/Launcher";
import { ShortcutsHelp } from "./components/ShortcutsHelp";
import { Sidebar, UPCOMING } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { Welcome } from "./components/Welcome";
import { WindowErrorBoundary } from "./components/WindowErrorBoundary";
import { applyLayout, saveLayoutAs, useLayoutNames } from "./components/WindowsMenu";
import { api, inTauri } from "./lib/api";
import { runCommand, setAdminHooks, useCommandList, useCommands, type Command } from "./lib/commands";
import { setNodeActionSource, suppressNativeMenus } from "./lib/contextMenu";
import { nodeActionFor } from "./lib/nodeStatus";
import { setDockBadge, setDockProgress } from "./lib/dock";
import { showError } from "./lib/errors";
import { useHealthSummary } from "./lib/health";
import { registerHealthCommands } from "./lib/healthCommands";
import { installKeymap, shortcutFor } from "./lib/keymap";
import { disabledMenuIds, startMenuBridge, useMenuState, useMenuWindows } from "./lib/menuBridge";
import { confirmUnlock, isAdminUnlocked, isLookOnly, lockAdmin, lookOnlyTip, requestAdmin, useAdmin, withAdmin } from "./lib/admin";
import { registerModelCommands } from "./lib/modelCommands";
import { registerNetCommands } from "./lib/netCommands";
import { registerOpsCommands } from "./lib/opsCommands";
import { installModality } from "./lib/modality";
import { onDeskRequests, openPopout } from "./lib/popout";
import { closePanel, openPanel, togglePanel, usePanel } from "./lib/session";
import { useCluster, useClusterSelector } from "./lib/store";
import { openTerminal, setPicker } from "./lib/terminals";
import { getAppearance, setAppearance, TEXT_SCALES, useAppearance, type ThemePref } from "./lib/theme";
import { toast, useToasts } from "./lib/toasts";
import { canUndo, runLatestUndo } from "./lib/undo";
import { useWorkloadCommands } from "./lib/workloadCommands";
import { helpOf, iconOf, isKnown, minSizeOf, openableKinds, REGISTRY, sizeOf, tileMinOf, titleOf, WindowBody } from "./windows/registry";
import { loadHelpMeta, openHelp, openHelpSection } from "./help/nav";
import { setPrefs, useShowUpcoming } from "./windows/settings/GeneralSection";
import { PLANNED } from "./windows/plannedWindows";
import { SECTIONS } from "./windows/settings/index";
import {
  Desktop, dispatchWindowEvent, getControlsStyle, refreshTitles, setControlsStyle, useControlsStyle, useWMActions, useWMState, useTitleVersion,
  windowElement, WindowManagerProvider, type WinState, type WMActions,
} from "./wm/WindowManager";

// --- Admin: commands that need it ask to unlock first, through the in-app sheet (confirmUnlock),
// then Touch ID or the Mac password when Live.

setAdminHooks({ isAdmin: isAdminUnlocked, unlock: (reason) => confirmUnlock(reason) });

// --- Webview zoom (View › Actual Size / Zoom In / Zoom Out), kept between launches.

const ZOOM_KEY = "sparkbox.zoom";
const ZOOM_STEPS = [0.8, 0.9, 1, 1.1, 1.25, 1.4, 1.6];

function savedZoom(): number {
  try {
    const z = Number(localStorage.getItem(ZOOM_KEY));
    return ZOOM_STEPS.includes(z) ? z : 1;
  } catch {
    return 1;
  }
}

let zoom = savedZoom();

function applyZoom(next: number) {
  zoom = next;
  try {
    localStorage.setItem(ZOOM_KEY, String(next));
  } catch {
    // Not remembered; still applied.
  }
  if (inTauri) getCurrentWebview().setZoom(next).catch(() => {});
  else document.documentElement.style.zoom = String(next);
}

function stepZoom(dir: 1 | -1) {
  const i = ZOOM_STEPS.indexOf(zoom);
  const next = ZOOM_STEPS[Math.max(0, Math.min(ZOOM_STEPS.length - 1, (i < 0 ? 2 : i) + dir))];
  if (next !== zoom) applyZoom(next);
  toast({ kind: "info", title: `Zoom ${Math.round(next * 100)}%`, id: "zoom", durationMs: 1400 });
}

// --- Sidebar (View › Toggle Sidebar), kept between launches.

const SIDEBAR_KEY = "sparkbox.sidebar.hidden";

function useSidebarHidden(): [boolean, (v: boolean) => void] {
  const [hidden, setHidden] = useState(() => {
    try {
      return localStorage.getItem(SIDEBAR_KEY) === "1";
    } catch {
      return false;
    }
  });
  const set = (v: boolean) => {
    setHidden(v);
    try {
      localStorage.setItem(SIDEBAR_KEY, v ? "1" : "0");
    } catch {
      // Session only.
    }
  };
  return [hidden, set];
}

// Every node menu leads with the one action that would get the node reporting again ("Trust host key…").
setNodeActionSource((n) => {
  const a = nodeActionFor(n);
  return a ? { label: a.label, run: () => a.run() } : null;
});

/** In full screen the traffic lights are hidden, so the top bar gives their space back. */
function useNativeFullscreen(): boolean {
  const [full, setFull] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    const win = getCurrentWindow();
    const check = () => win.isFullscreen().then(setFull, () => {});
    check();
    let stop: (() => void) | undefined;
    let alive = true;
    win.onResized(check).then((u) => (alive ? (stop = u) : u()), () => {});
    return () => {
      alive = false;
      stop?.();
    };
  }, []);
  return full;
}

/** Focuses the active window's search field (⌘F), unless the window handles Find itself. */
function findInActive(wm: WMActions) {
  const id = wm.getActiveId();
  if (!id) return;
  if (!dispatchWindowEvent(id, "find")) return;
  const field = windowElement(id)?.querySelector<HTMLElement>("[data-find-target]");
  if (field) {
    field.focus();
    if (field instanceof HTMLInputElement) field.select();
  }
}

/** Every node, switch, container and open window, plus the app's own commands. */
function ShellCommands({ sidebarHidden, setSidebarHidden }: { sidebarHidden: boolean; setSidebarHidden: (v: boolean) => void }) {
  const wm = useWMActions();
  const { windows, activeId } = useWMState();
  const titleV = useTitleVersion();
  const cluster = useCluster();
  const admin = useAdmin();
  const controls = useControlsStyle();
  const hasWindows = windows.length > 0;
  const active = windows.find((w) => w.id === activeId) ?? null;

  const noWindow = () => (wm.getActiveId() ? true : "No window is open");
  const visibleCount = () => wm.getWindows().filter((w) => !w.minimized).length;
  // ⌘T opens a shell on the node whose window is in front, else on the first node online.
  const terminalNode = () => {
    const front = wm.getWindows().find((w) => w.id === wm.getActiveId());
    if (front?.kind === "node" && front.params.id) return front.params.id;
    return cluster?.nodes.find((n) => n.status === "online")?.id ?? null;
  };
  const terminalInFront = () => wm.getWindows().find((w) => w.id === wm.getActiveId())?.kind === "terminal";
  const go = (kind: string, icon?: LucideIcon): Command => ({
    id: `go:${kind}`,
    title: titleOf({ kind, params: {} }),
    group: "Windows",
    icon: icon ?? iconOf(kind),
    shortcut: shortcutFor(`go:${kind}`),
    keywords: kind === "howto" ? [kind, "playbooks", "skills", "guides", "nvidia"] : [kind],
    run: () => wm.open(kind),
  });

  // Windows that open without params, in registry order.
  useCommands(openableKinds().filter((k) => k !== "help").map((k) => go(k)), [REGISTRY]);

  // Planned launchers are searchable while Show upcoming features is on: "TensorRT-LLM (coming
  // soon)" opens its planned window; one that a built window covers now says where it lives.
  const upcoming = useShowUpcoming();
  useCommands(
    upcoming
      ? UPCOMING.filter((u) => !(u.kind && isKnown(u.kind))).map((u): Command => {
          const redirect = PLANNED[u.key]?.redirect;
          return {
            id: `planned:${u.key}`,
            title: redirect ? u.label : `${u.label} (coming soon)`,
            subtitle: redirect ? PLANNED[u.key]?.summary : "Planned: what it will do",
            group: "Coming soon",
            icon: u.icon,
            keywords: ["planned", "upcoming", "coming soon", "launcher", u.key],
            run: () => wm.open("planned", { key: u.key }),
          };
        })
      : [],
    [upcoming],
  );

  // Window management and the app.
  useCommands([
    { id: "app:palette", title: "Command palette", group: "App", icon: Search, shortcut: shortcutFor("app:palette"), run: () => togglePanel("palette") },
    { id: "app:settings", title: "Settings", group: "App", icon: Settings, shortcut: shortcutFor("app:settings"), keywords: ["preferences"], run: () => wm.open("settings") },
    { id: "app:shortcuts", title: "Keyboard shortcuts", group: "App", icon: Keyboard, shortcut: shortcutFor("app:shortcuts"), run: () => openPanel("shortcuts") },
    { id: "help:open", title: "SparkBox Help", group: "App", icon: CircleHelp, shortcut: shortcutFor("help:open"), keywords: ["documentation", "manual", "guide", "how to"], run: () => wm.open("help") },
    { id: "app:welcome", title: "Show Welcome", group: "App", icon: Sparkles, run: () => openPanel("welcome") },
    { id: "app:doctor", title: "Open Diagnostics…", group: "App", icon: Stethoscope, subtitle: "Opens Settings › Diagnostics", keywords: ["diagnostics", "doctor"], run: () => wm.open("settings", { section: "diagnostics" }) },
    { id: "app:activity", title: "Show Activity log", group: "App", icon: REGISTRY.activity?.icon, run: () => wm.open("activity") },
    {
      id: "app:sidebar", title: sidebarHidden ? "Show sidebar" : "Hide sidebar", group: "App", icon: PanelLeft,
      shortcut: shortcutFor("app:sidebar"), keywords: ["toggle sidebar"], run: () => setSidebarHidden(!sidebarHidden),
    },
    { id: "app:zoom-in", title: "Zoom in", group: "App", icon: ZoomIn, shortcut: shortcutFor("app:zoom-in"), run: () => stepZoom(1) },
    { id: "app:zoom-out", title: "Zoom out", group: "App", icon: ZoomOut, shortcut: shortcutFor("app:zoom-out"), run: () => stepZoom(-1) },
    { id: "app:zoom-reset", title: "Actual size", group: "App", icon: Search, shortcut: shortcutFor("app:zoom-reset"), run: () => applyZoom(1) },
    {
      id: "app:controls-style",
      title: controls === "mac" ? "Window controls: WinBox style" : "Window controls: macOS style",
      subtitle: controls === "mac" ? "Flat buttons on the right of each title bar" : "Close, minimize and zoom circles on the left",
      group: "App", icon: AppWindow, keywords: ["traffic lights", "buttons"],
      run: () => setControlsStyle(getControlsStyle() === "mac" ? "winbox" : "mac"),
    },
    {
      id: "app:minimize", title: "Minimize SparkBox", group: "App", icon: Minus, shortcut: shortcutFor("app:minimize"),
      run: () => void invoke("menu_minimize_main").catch(() => {}),
    },
    {
      id: "term:new-tab", title: "New terminal tab", group: "Window", icon: SquareTerminal, shortcut: shortcutFor("term:new-tab"),
      subtitle: "A shell on the node in front, else the first one online", keywords: ["ssh", "shell"], needsAdmin: true,
      when: () => (terminalInFront() || terminalNode() ? true : "No node is online"),
      run: () => {
        // In the Terminal window itself it offers the quick-connect list, like ⌘T in a tab.
        if (terminalInFront()) return setPicker(true);
        openTerminal(terminalNode()!);
        wm.open("terminal");
      },
    },
    { id: "files:open", title: "Open Files", group: "Window", icon: FolderOpen, shortcut: shortcutFor("go:files"), run: () => wm.open("files") },
    {
      id: "wm:popout", title: "Open in separate window", group: "Window", icon: ExternalLink, shortcut: shortcutFor("wm:popout"),
      subtitle: active ? `${wm.titleFor(active)} in a window of its own` : "A window of its own, e.g. for a second screen", keywords: ["pop out", "detach", "undock"],
      when: () => (!wm.popOut ? "Only in the app" : wm.getActiveId() ? true : "No window is open"), run: () => wm.popOut?.(wm.getActiveId()!),
    },
    {
      id: "wm:close", title: "Close window", group: "Window", icon: X, shortcut: shortcutFor("wm:close"),
      subtitle: active ? wm.titleFor(active) : "Hides SparkBox (it keeps running in the menu bar)",
      run: () => {
        const id = wm.getActiveId();
        if (id) void wm.requestClose(id);
        // Nothing open inside: ⌘W closes SparkBox's own window, like any Mac app.
        else if (inTauri) void invoke("menu_close_main").catch(() => {});
      },
    },
    { id: "wm:close-all", title: "Close all windows", group: "Window", icon: SquareX, shortcut: shortcutFor("wm:close-all"), when: () => (wm.getWindows().length ? true : "No windows are open"), run: () => void wm.requestCloseAll() },
    {
      id: "wm:reopen", title: "Reopen closed window", group: "Window", icon: Undo2, shortcut: shortcutFor("wm:reopen"),
      run: () => wm.reopenLast(),
    },
    { id: "wm:minimize", title: "Minimize window", group: "Window", icon: Minus, shortcut: shortcutFor("wm:minimize"), when: noWindow, run: () => wm.minimize(wm.getActiveId()!) },
    { id: "wm:fill", title: "Fill desktop", group: "Window", icon: Maximize2, shortcut: shortcutFor("wm:fill"), when: noWindow, keywords: ["maximize", "zoom"], run: () => wm.snap(wm.getActiveId()!, "max") },
    { id: "wm:restore", title: "Restore window size", group: "Window", icon: Minimize2, shortcut: shortcutFor("wm:restore"), when: noWindow, run: () => wm.snap(wm.getActiveId()!, "restore") },
    { id: "wm:snap-left", title: "Snap window left", group: "Window", icon: Columns2, shortcut: shortcutFor("wm:snap-left"), when: noWindow, run: () => wm.snap(wm.getActiveId()!, "left") },
    { id: "wm:snap-right", title: "Snap window right", group: "Window", icon: Columns2, shortcut: shortcutFor("wm:snap-right"), when: noWindow, run: () => wm.snap(wm.getActiveId()!, "right") },
    { id: "wm:cascade", title: "Cascade windows", group: "Window", icon: Layers, shortcut: shortcutFor("wm:cascade"), when: () => (wm.getWindows().length ? true : "No windows are open"), run: () => wm.cascade() },
    {
      id: "wm:tile", title: "Tile all windows", group: "Window", icon: LayoutGrid, shortcut: shortcutFor("wm:tile"), keywords: ["arrange", "grid"],
      subtitle: "Side by side without overlapping; what doesn't fit goes to the dock",
      when: () => (visibleCount() ? true : "No windows are open"), run: () => wm.tile("all"),
    },
    {
      // ⌘K, the Tile menu and View › Hide Sidebar and Tile All: the sidebar's room goes to the tile
      // (with the sidebar already hidden, it tiles).
      id: "wm:tile-wide", title: "Hide sidebar and tile all", group: "Window", icon: PanelLeft, keywords: ["arrange", "grid", "more room"],
      subtitle: sidebarHidden ? "The sidebar is hidden: tiles all" : "Gives the windows the sidebar's room first",
      when: () => (visibleCount() ? true : "No windows are open"),
      run: () => {
        if (!sidebarHidden) setSidebarHidden(true);
        // Restore layout puts the sidebar back with the windows (only if this hid it).
        wm.tile("all", { afterResize: !sidebarHidden, onRestore: sidebarHidden ? undefined : () => setSidebarHidden(false) });
      },
    },
    { id: "wm:tile-two", title: "Tile two side by side", group: "Window", icon: Columns2, keywords: ["split", "arrange"], when: () => (visibleCount() >= 2 ? true : "Needs two windows"), run: () => wm.tile("two") },
    { id: "wm:tile-four", title: "Tile four in a grid", group: "Window", icon: Grid2x2, keywords: ["2x2", "quarters", "arrange"], when: () => (visibleCount() >= 2 ? true : "Needs two windows"), run: () => wm.tile("four") },
    {
      id: "wm:restore-layout", title: "Restore layout", group: "Window", icon: Undo2, subtitle: "Puts tiled and cascaded windows back where you had them",
      keywords: ["untile", "arrange"], when: () => (wm.canRestoreLayout() ? true : "Nothing has been tiled or cascaded"), run: () => wm.restoreLayout(),
    },
    { id: "wm:next", title: "Next window", group: "Window", icon: AppWindow, shortcut: shortcutFor("wm:next"), run: () => wm.cycle(1) },
    { id: "wm:prev", title: "Previous window", group: "Window", icon: AppWindow, shortcut: shortcutFor("wm:prev"), run: () => wm.cycle(-1) },
    { id: "wm:front", title: "Bring all to front", group: "Window", icon: AppWindow, subtitle: "Restores minimized windows", run: wm.bringAllToFront },
    { id: "wm:overview", title: "Show all windows", group: "Window", icon: PanelsTopLeft, shortcut: "F3", when: () => (wm.getWindows().length ? true : "No windows are open"), run: () => togglePanel("overview") },
    { id: "wm:save-layout", title: "Save layout…", group: "Window", icon: Save, when: () => (wm.getWindows().length ? true : "No windows are open"), run: () => saveLayoutAs(wm) },
    { id: "wm:find", title: "Find in window", group: "Window", icon: Search, shortcut: shortcutFor("wm:find"), when: noWindow, run: () => findInActive(wm) },
    { id: "wm:refresh", title: "Refresh window", group: "Window", icon: RefreshCw, shortcut: shortcutFor("wm:refresh"), when: noWindow, run: () => void dispatchWindowEvent(wm.getActiveId()!, "refresh") },
    {
      id: "wm:undo", title: "Undo", group: "Window", icon: Undo2, subtitle: "Closing a window, moving to the Trash…",
      when: () => (canUndo() ? true : "Nothing to undo"), run: () => void runLatestUndo(),
    },
  ], [sidebarHidden, controls, active?.id, titleV, hasWindows, cluster?.nodes.map((n) => `${n.id}:${n.status}`).join(",")]);

  // Admin mode and the data source.
  useCommands([
    {
      id: "cluster:admin", title: admin ? "Lock Admin mode" : "Unlock Admin mode…", group: "Cluster",
      icon: admin ? Lock : LockOpen, shortcut: shortcutFor("cluster:admin"), keywords: ["admin", "read-only", "padlock"],
      when: () => (!isAdminUnlocked() && isLookOnly() ? lookOnlyTip() : true),
      run: async () => {
        if (isAdminUnlocked()) await lockAdmin();
        else await requestAdmin();
      },
    },
    {
      id: "cluster:source-demo", title: "Use Demo data", group: "Cluster", icon: Sparkles, subtitle: "The built-in simulated cluster",
      when: () => (!cluster || cluster.demo ? "Already showing Demo data" : true),
      run: () => setDataSource("demo"),
    },
    {
      id: "cluster:source-live", title: "Use Live data", group: "Cluster", icon: Server, subtitle: "Your Sparks over SSH",
      when: () => (cluster && !cluster.demo ? "Already live" : true),
      run: () => setDataSource("live"),
    },
  ], [admin, cluster?.demo]);

  // Switches; nodes, containers and failed services come from workloadCommands.ts.
  const switchKey = cluster?.switches.map((s) => `${s.id}:${s.name}`).join(",") ?? "";
  useCommands((cluster?.switches ?? []).map((s): Command => ({
    id: `switch:${s.id}`, title: s.name, group: "Switches", subtitle: `${s.host || "not set up"} · open switch`, icon: Router,
    entity: { kind: "switch", id: s.id, name: s.name }, verbs: ["open"], keywords: [s.kind], run: () => wm.open("switch", { id: s.id, name: s.name }),
  })), [switchKey]);
  // Nodes (Open, Terminal, Files, Restart…, Shut down…, DGX Dashboard), containers and failed services.
  useWorkloadCommands();

  // Settings sections and appearance, so "notifications", "theme" or "text size" go straight there.
  useCommands(SECTIONS.map((sec): Command => ({
    id: `settings:${sec.id}`, title: `Settings › ${sec.label}`, group: "Settings", icon: sec.icon, keywords: sec.keywords, verbs: ["open"],
    run: () => wm.open("settings", { section: sec.id }),
  })), []);
  const appearance = useAppearance();
  const THEMES: { pref: ThemePref; label: string; icon: LucideIcon }[] = [
    { pref: "light", label: "Light", icon: Sun }, { pref: "dark", label: "Dark", icon: Moon }, { pref: "black", label: "Black", icon: Eclipse },
    { pref: "system", label: "System", icon: Monitor },
  ];
  useCommands([
    ...THEMES.map((t): Command => ({
      id: `appearance:theme-${t.pref}`, title: `Theme: ${t.label}`, group: "Appearance", icon: t.icon,
      subtitle: appearance.theme === t.pref ? "Current" : t.pref === "system" ? "Follows macOS" : undefined,
      keywords: ["appearance", "colour", "color", "mode"], run: () => setAppearance({ theme: t.pref }),
    })),
    ...TEXT_SCALES.map((t): Command => ({
      id: `appearance:text-${t.value}`, title: `Text size: ${t.label}`, group: "Appearance", icon: Type,
      subtitle: appearance.textScale === t.value ? "Current" : undefined, keywords: ["appearance", "font", "bigger", "smaller"],
      run: () => setAppearance({ textScale: t.value }),
    })),
    {
      id: "appearance:motion", title: appearance.motion === "reduce" ? "Reduce motion: turn off" : "Reduce motion: turn on", group: "Appearance", icon: Wind,
      keywords: ["appearance", "animation", "accessibility"], run: () => setAppearance({ motion: getAppearance().motion === "reduce" ? "system" : "reduce" }),
    },
    {
      id: "appearance:contrast", title: appearance.contrast === "more" ? "Increase contrast: turn off" : "Increase contrast: turn on", group: "Appearance", icon: Contrast,
      keywords: ["appearance", "accessibility"], run: () => setAppearance({ contrast: getAppearance().contrast === "more" ? "system" : "more" }),
    },
  ], [appearance.theme, appearance.textScale, appearance.motion, appearance.contrast]);

  // Open windows: "Switch to …".
  useCommands(windows.map((w): Command => ({
    id: `win:${w.id}`, title: wm.titleFor(w), group: "Open windows", subtitle: w.minimized ? "Minimized · switch to it" : w.id === activeId ? "Current window" : "Switch to it",
    icon: iconOf(w.kind) ?? AppWindow, run: () => wm.focus(w.id),
  })), [windows.map((w) => `${w.id}:${w.minimized}`).join(","), activeId, titleV]);

  // The menu bar's Window list and the state its labels show.
  const layouts = useLayoutNames();
  const commandList = useCommandList();
  const [scenarios, setScenarios] = useState<{ id: string; title: string }[]>([]);
  useEffect(() => {
    if (!inTauri || !cluster?.demo) return;
    loadScenarios().then(setScenarios);
  }, [cluster?.demo]);
  useMenuWindows(windows.map((w) => ({ id: w.id, title: wm.titleFor(w), active: w.id === activeId, minimized: w.minimized })));
  useMenuState({
    admin,
    demo: cluster?.demo ?? true,
    scenario: cluster?.scenario ?? null,
    nodes: (cluster?.nodes ?? []).map((n) => ({ id: n.id, name: n.name })),
    scenarios,
    layouts,
    disabled: useMemo(() => disabledMenuIds(), [commandList, windows.length, activeId, admin, cluster?.demo]),
    controls,
  });
  // The one native menu item the shared bridge doesn't list (View › Hide Sidebar and Tile All).
  useEffect(() => {
    if (!isTauri()) return;
    const un = listen("menu://wm:tile-wide", () => void runCommand("wm:tile-wide"));
    return () => void un.then((u) => u()).catch(() => {});
  }, []);

  useEffect(() => startMenuBridge({
    openHelp: (id) => openHelp(id),
    openHelpSection: (id) => openHelpSection(id),
    focusWindow: (id) => wm.focus(id),
    openNode: (id) => wm.open("node", { id }),
    setScenario: (id) => void invoke("demo_scenario_set", { id }).catch((e) => toast({ kind: "error", title: "Couldn't change the scenario", body: String(e) })),
    openLayout: (name) => void applyLayout(wm, name),
  }), [wm]);
  return null;
}

// Switching what SparkBox talks to is Mac-side state: it asks for Touch ID or the password.
async function setDataSource(source: "demo" | "live") {
  try {
    setPrefs(await withAdmin(() => api.dataSourceSet(source), { scope: "mac", action: "data_source_set" }));
  } catch (e) {
    showError(e, { action: source === "demo" ? "Switch to Demo data" : "Switch to Live data" });
    return;
  }
  toast({ kind: "success", title: source === "demo" ? "Showing Demo data" : "Connecting to your Sparks", body: source === "live" ? "Numbers switch over as each node answers." : undefined });
}

async function loadScenarios(): Promise<{ id: string; title: string }[]> {
  try {
    const list = await invoke<{ id: string; title?: string; name?: string; label?: string }[]>("demo_scenarios_list");
    return list.map((s) => ({ id: s.id, title: s.title ?? s.name ?? s.label ?? s.id }));
  } catch {
    return [];
  }
}

// Commands other packages add (ops, health, models, the fabric): each register…Commands()
// returns a function removing them again.
const PACKAGE_COMMANDS: [string, () => () => void][] = [
  ["registerOpsCommands", registerOpsCommands],
  ["registerHealthCommands", registerHealthCommands],
  ["registerModelCommands", registerModelCommands],
  ["registerNetCommands", registerNetCommands],
];

function PackageCommands() {
  useEffect(() => {
    const stops: (() => void)[] = [];
    for (const [name, register] of PACKAGE_COMMANDS) {
      try {
        stops.push(register());
      } catch (e) {
        console.error(`${name} failed`, e);
      }
    }
    return () => stops.forEach((stop) => stop());
  }, []);
  return null;
}

/** Window titles that come from the inventory (a node's or switch's name) follow it when it changes. */
function TitleNames() {
  const names = useClusterSelector((s) => [...(s?.nodes ?? []), ...(s?.switches ?? [])].map((x) => `${x.id}=${x.name}`).join(","));
  useEffect(() => refreshTitles(), [names]);
  return null;
}

/**
 * A phone or banner notification that brought SparkBox forward: say which, with a way to open
 * what it's about. Nothing opens by itself.
 */
function NotificationFollow() {
  const wm = useWMActions();
  useEffect(() => {
    if (!inTauri) return;
    let stop: (() => void) | undefined;
    let alive = true;
    listen<{ title: string; kind: string; params?: Record<string, string> }>("notify://pending", ({ payload }) => {
      if (!payload?.title) return;
      toast({
        kind: "info",
        id: "last-notification",
        title: `Last notification: ${payload.title}`,
        durationMs: 8000,
        actions: payload.kind && isKnown(payload.kind) ? [{ label: "Open", run: () => wm.open(payload.kind, payload.params ?? {}) }] : undefined,
      });
    }).then((u) => (alive ? (stop = u) : u()), () => {});
    return () => {
      alive = false;
      stop?.();
    };
  }, [wm]);
  return null;
}

/** Opens the window the menu bar icon's menu asked for (a node, Chat, …), and what pop-outs ask the desk for. */
function TrayRequests() {
  const wm = useWMActions();
  useEffect(() => onDeskRequests(({ kind, params }) => {
    if (!isKnown(kind)) return;
    wm.open(kind, params);
    getCurrentWindow().setFocus().catch(() => {});
  }), [wm]);
  useEffect(() => {
    if (!inTauri) return;
    let stop: (() => void) | undefined;
    let alive = true;
    api.onTrayOpen((e) => wm.open(e.kind, e.params)).then((u) => (alive ? (stop = u) : u()));
    return () => {
      alive = false;
      stop?.();
    };
  }, [wm]);
  return null;
}

/** Page-level keys (⌘Z app undo, ⇧F10, F3) and, outside the app, the menu shortcuts. */
function Keys() {
  // The real app has the native menu bar (it owns ⌘ shortcuts); the browser preview doesn't.
  useEffect(() => installKeymap({ nativeMenu: isTauri(), onOverview: () => togglePanel("overview") }), []);
  // The Help index at once, so an info icon has its name by the time anyone reads it.
  useEffect(() => void loadHelpMeta().catch(() => {}), []);
  // Focus rings only after keyboard use (html[data-modality]).
  useEffect(() => installModality(), []);
  useEffect(() => suppressNativeMenus(), []);
  useEffect(() => {
    if (zoom !== 1) applyZoom(zoom);
  }, []);
  return null;
}

/** The Dock icon shows progress toasts as a bar and open problems as a badge. */
function DockIcon() {
  const toasts = useToasts();
  const health = useHealthSummary();
  const progress = toasts.filter((t) => t.kind === "progress" && t.progress != null);
  useEffect(() => {
    setDockProgress(progress.length ? progress.reduce((a, t) => a + (t.progress ?? 0), 0) / progress.length : null);
  }, [progress.map((t) => t.progress).join(",")]);
  useEffect(() => {
    setDockBadge(health ? health.crit : null);
  }, [health?.crit]);
  return null;
}

const scenarioTitles = new Map<string, string>();

/** A window of its own: the same kind and params, at the size it had on the desk. */
async function popOutWindow(w: WinState): Promise<boolean> {
  const params = Object.fromEntries(Object.entries(w.params).filter(([k]) => k !== "nav"));
  try {
    await openPopout({ kind: w.kind, params }, { title: titleOf(w), size: [w.w, w.h], minSize: minSizeOf(w.kind) });
    return true;
  } catch (e) {
    toast({ kind: "error", title: "Couldn't open a separate window", body: String(e).replace(/^Error: /, "") });
    return false;
  }
}

export default function App() {
  const [sidebarHidden, setSidebarHidden] = useSidebarHidden();
  const fullscreen = useNativeFullscreen();
  const admin = useAdmin();
  const panel = usePanel();
  const [, setScenarioTick] = useState(0);
  useEffect(() => {
    if (!inTauri) return;
    loadScenarios().then((list) => {
      list.forEach((s) => scenarioTitles.set(s.id, s.title));
      setScenarioTick((n) => n + 1);
    });
  }, []);

  return (
    <WindowManagerProvider
      sizeOf={sizeOf} minSizeOf={minSizeOf} tileMinOf={tileMinOf} isKnown={isKnown} titleOf={titleOf} iconOf={iconOf} helpOf={helpOf}
      popOutWindow={inTauri ? popOutWindow : undefined}
    >
      <div
        className={`app${admin ? " admin" : ""}`}
        data-sidebar={sidebarHidden ? "hidden" : undefined}
        data-fullscreen={fullscreen || undefined}
      >
        <TopBar scenarioTitle={(id) => scenarioTitles.get(id) ?? id} />
        <CoreBanner />
        <FenceBanner />
        <SettingsFileBanner />
        <div className="main">
          <Sidebar />
          <Desktop
            Body={WindowBody}
            wrap={WindowErrorBoundary}
            empty={<Launcher />}
            overlays={<Toasts />}
            overview={panel === "overview"}
            onOverviewDone={closePanel}
          />
        </div>
        <StatusBar />
      </div>
      <ConfirmHost />
      <ContextMenuHost />
      <CommandPalette />
      <ShortcutsHelp />
      <Welcome isKnown={isKnown} />
      <ShellCommands sidebarHidden={sidebarHidden} setSidebarHidden={setSidebarHidden} />
      <PackageCommands />
      <TitleNames />
      <NotificationFollow />
      <TrayRequests />
      <Keys />
      <DockIcon />
      {!inTauri && (
        <div className="browser-note">
          This is the SparkBox UI in a plain browser, so there's no data. Run <code>npm run tauri dev</code> to start the app.
        </div>
      )}
    </WindowManagerProvider>
  );
}
