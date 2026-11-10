// WinBox-style window manager: child windows float inside the desktop area and
// can be dragged, resized from any edge, snapped to halves and quarters,
// minimized to the dock row, maximized, cascaded or tiled. Opening something
// that's already open brings it to the front instead of duplicating it.
//
// Nothing is lost by accident: every window stays mounted while minimized,
// closing asks the window first (close guards, the close-request event), and a
// closed window can come back with Undo or ⌘⇧T. The layout, each window's last
// geometry and its view state (useWinState) survive restarts via localStorage.
//
// Each window remembers `userFrame`, where it was last put by hand. Tile, Cascade and
// snapping only arrange windows for now: what's saved, and what a reopened window
// comes back at, is always the user's own frame.
import {
  createContext, memo, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState,
  useSyncExternalStore, type ComponentType, type ReactNode, type RefObject,
} from "react";
import { flushSync } from "react-dom";
import { X, type LucideIcon } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { inTauri } from "../lib/api";
import { setActiveScope } from "../lib/commands";
import { showContextMenu } from "../lib/contextMenu";
import { cancelDialogsFor, confirmAction } from "../lib/session";
import { setFrontWindowProbe } from "../lib/toasts";
import { pushUndo } from "../lib/undo";
import { plural } from "../lib/vocabulary";
import {
  cascadeFree, cascadeLayout, clampRect, DOCK_H, fitInside, normalizeZ, onTheDesk, openFrame, overviewPlacement, tileCells, tileLayout, zoneRect,
  type Rect, type Size, type SnapZone,
} from "./geometry";
import { WindowFrame } from "./WindowFrame";

export type Params = Record<string, string>;

export interface WinState {
  id: string;
  kind: string;
  params: Params;
  x: number;
  y: number;
  w: number;
  h: number;
  z: number;
  minimized: boolean;
  maximized: boolean;
  /** Snapped to a half or quarter; follows the desktop when it resizes. */
  snapped?: SnapZone | null;
  /** Geometry to go back to when a maximized or snapped window is restored. */
  restore?: Rect | null;
  /** Where the user last put it (moved or resized by hand). Saved, and used when it reopens. */
  userFrame?: Rect | null;
  /** Placed by Tile or Cascade rather than by hand; Restore layout puts it back at userFrame. */
  arranged?: boolean;
  /** Sent to the dock by Tile because it didn't fit; Restore layout brings it back. */
  tileMinimized?: boolean;
  /** View state the window keeps with the layout (useWinState). */
  view?: Record<string, unknown>;
}

export type TileMode = "all" | "two" | "four";

/** What a Tile would do on the desktop as it is (or `extraW` px wider): how many it tiles of how many. */
export interface TilePreview {
  tiled: number;
  of: number;
  /** Side by side would stack the two top and bottom. */
  stacked: boolean;
}

/** The toast when two can't sit side by side. */
export const STACKED_NOTE = "Stacked top and bottom: too narrow to sit side by side";

export type SnapTarget = SnapZone | "restore";
type Geometry = Partial<Pick<WinState, "x" | "y" | "w" | "h">>;
export type WindowEventName = "refresh" | "find" | "close-request";

export interface ClosedGroup {
  wins: WinState[];
  at: number;
}

export interface WMActions {
  desktopRef: RefObject<HTMLDivElement | null>;
  open(kind: string, params?: Params): void;
  /** Guarded: same as requestClose. */
  close(id: string): void;
  /** Guarded: same as requestCloseAll. */
  closeAll(): void;
  requestClose(id: string): Promise<boolean>;
  requestCloseAll(): Promise<boolean>;
  requestCloseOthers(id: string): Promise<boolean>;
  focus(id: string): void;
  move(id: string, g: Geometry): void;
  minimize(id: string): void;
  toggleMinimize(id: string): void;
  toggleMaximize(id: string): void;
  snap(id: string, zone: SnapTarget, from?: Rect): void;
  cascade(): void;
  /**
   * Tile all visible windows, the two most recent side by side, or the four most recent.
   * `afterResize`: the desktop is about to change size (the sidebar hiding), so tile once it has.
   * `onRestore`: what Restore layout also undoes (showing the sidebar the tile hid).
   */
  tile(mode?: TileMode, opts?: { afterResize?: boolean; onRestore?: () => void }): void;
  /** What tile(mode) would do now, or with a desktop `extraW` px wider (no sidebar). */
  tilePreview(mode: TileMode, extraW?: number): TilePreview;
  /** Puts arranged windows back where the user had them, and brings back what Tile minimized. */
  restoreLayout(): void;
  /** Whether Tile or Cascade has arranged anything that Restore layout would put back. */
  canRestoreLayout(): boolean;
  /** Takes a window off the desk without Undo (it moved to a pop-out). */
  detach(id: string): void;
  /** Moves a window into a native window of its own; null when this build can't. */
  popOut: ((id: string) => Promise<boolean>) | null;
  cycle(dir: 1 | -1): void;
  reopenLast(): void;
  /** Opens windows from a saved layout (skipping ones already open). */
  restoreWindows(wins: WinState[]): void;
  bringAllToFront(): void;
  getWindows(): WinState[];
  getActiveId(): string | null;
  getBounds(): Size;
  titleFor(w: Pick<WinState, "id" | "kind" | "params">): string;
  iconFor(kind: string): LucideIcon | undefined;
  /** The Help topic behind a window's title-bar info button. */
  helpFor(kind: string, params?: Params): string | undefined;
  minSizeFor(kind: string): [number, number];
  tileMinFor(kind: string): [number, number];
  setView(id: string, key: string, value: unknown): void;
  getView(id: string): Record<string, unknown> | undefined;
}

export interface WMState {
  windows: WinState[];
  activeId: string | null;
  /** False while another app is in front: every window then looks inactive. */
  appFocused: boolean;
  /** The usable desktop (minus the dock row when it shows). */
  bounds: Size;
}

export type WM = WMActions & WMState;

const STORAGE_KEY = "sparkbox.windows.v1";
// v2 holds user frames only; v1 (round 1) could hold Tile's frames, so it's dropped once.
const GEOM_KEY = "sparkbox.wingeom.v2";
const LAST_LAYOUT_KEY = "sparkbox.lastlayout.v1";
const CLOSED_KEY = "sparkbox.closed.v1";
try {
  localStorage.removeItem("sparkbox.wingeom.v1");
} catch {
  // No storage: nothing to drop.
}
const CLOSED_MAX = 10;
const Z_LIMIT = 5000;
const ARRANGE_MS = 200;
/** Until the desktop has been measured. */
const INITIAL_DESK: Size = { w: 1200, h: 800 };

/**
 * Params that pick a view inside a window (a tab, a Settings section, a node filter) rather than
 * which window it is; each kind lists its own (the registry's `views`). They're left out of the
 * id, so opening a window that's already open brings it forward on the new view instead of making
 * a second copy. `nav` counts those re-opens, so a window can follow a link back to a view it
 * moved away from.
 */
const viewParams = new Map<string, Set<string>>();

export function setViewParams(kinds: Record<string, string[] | undefined>) {
  viewParams.clear();
  for (const [kind, keys] of Object.entries(kinds)) if (keys?.length) viewParams.set(kind, new Set(keys));
}

const isView = (kind: string, key: string) => key === "nav" || !!viewParams.get(kind)?.has(key);

export function winId(kind: string, params: Params = {}): string {
  const p = Object.keys(params).filter((k) => !isView(kind, k)).sort().map((k) => `${k}=${params[k]}`).join("&");
  return p ? `${kind}?${p}` : kind;
}

/** Saved windows under today's ids (older layouts kept view params in them), one per id. */
function normaliseIds(wins: WinState[]): WinState[] {
  const byId = new Map<string, WinState>();
  for (const w of wins) {
    const id = winId(w.kind, w.params ?? {});
    const had = byId.get(id);
    if (!had || had.z < w.z) byId.set(id, { ...w, id, params: w.params ?? {} });
  }
  return [...byId.values()];
}

function readJson<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : fallback;
  } catch {
    return fallback;
  }
}

function writeJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Layout persistence is a convenience; ignore storage failures.
  }
}

const frameOf = (w: Rect): Rect => ({ x: w.x, y: w.y, w: w.w, h: w.h });
/** Opens and focus requests so far: the palette can tell whether a command brought a window forward. */
let raises = 0;
export const raiseSeq = () => raises;

/** Top-left corners of the windows on show, which a new one mustn't sit exactly on. */
const visibleCorners = (ws: WinState[]) => ws.filter((w) => !w.minimized && !w.maximized).map((w) => ({ x: w.x, y: w.y }));

/**
 * The frame a saved window comes back at. Windows saved before user frames existed (round 1)
 * may have been saved while tiled, so they keep their place but get their default size back.
 */
export function savedFrame(w: WinState, defaultSize: [number, number]): Rect {
  if (w.userFrame) return frameOf(w.userFrame);
  return { x: w.x, y: w.y, w: defaultSize[0], h: defaultSize[1] };
}

/** What's saved for a window: its own frame, never a tiled, cascaded or snapped one. */
export function persisted(w: WinState): WinState {
  const f = w.userFrame ?? (w.maximized || w.snapped ? w.restore : null) ?? frameOf(w);
  // Tile's minimizing is part of the arrangement too: the window comes back on the desk.
  return {
    ...w, ...frameOf(f), userFrame: frameOf(f), snapped: null, restore: w.maximized ? frameOf(f) : null,
    minimized: w.minimized && !w.tileMinimized, arranged: false, tileMinimized: false,
  };
}

function isWin(w: unknown): w is WinState {
  const x = w as WinState;
  return !!x && typeof x.id === "string" && typeof x.kind === "string" && [x.x, x.y, x.w, x.h, x.z].every((n) => Number.isFinite(n));
}

function loadSaved(): WinState[] {
  const parsed = readJson<unknown>(STORAGE_KEY, []);
  return Array.isArray(parsed) ? normaliseIds(parsed.filter(isWin)) : [];
}

/** The last non-empty layout, offered by the launcher after Close all. */
export function lastLayout(): WinState[] {
  const v = readJson<unknown>(LAST_LAYOUT_KEY, []);
  return Array.isArray(v) ? normaliseIds(v.filter(isWin)) : [];
}

/** Recently closed windows, newest first (the ⌘⇧T stack). */
export function closedGroups(): ClosedGroup[] {
  const v = readJson<unknown>(CLOSED_KEY, []);
  return Array.isArray(v) ? (v as ClosedGroup[]).filter((g) => Array.isArray(g?.wins)) : [];
}

const reducedMotion = () => typeof window !== "undefined" && !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

function escapeAttr(v: string) {
  return v.replace(/["\\]/g, "\\$&");
}

// --- Opening windows from outside React (palette command modules, tray, notifications).

let openFromAnywhere: ((kind: string, params?: Params) => void) | null = null;

/** Opens (or brings forward) a window from code that has no WM context, e.g. a command module. */
export function openWindow(kind: string, params: Params = {}) {
  openFromAnywhere?.(kind, params);
}

/** The frame element of a window, e.g. to portal a sheet into it. */
export function windowElement(id: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`.win[data-win-id="${escapeAttr(id)}"]`);
}

// --- Per-window registries the hooks below write into. Module-level, so a
// window body can use them without re-rendering the manager.

const guards = new Map<string, Set<{ current: () => string | null }>>();
const events = new Map<string, Map<WindowEventName, Set<{ current: (e: Event) => void }>>>();

const titleOverrides = new Map<string, string>();
const titleListeners = new Set<() => void>();
let titleVersion = 0;

function setTitleOverride(id: string, title: string | null) {
  if (title === null) {
    if (!titleOverrides.has(id)) return;
    titleOverrides.delete(id);
  } else {
    if (titleOverrides.get(id) === title) return;
    titleOverrides.set(id, title);
  }
  titleVersion++;
  titleListeners.forEach((l) => l());
}

/** Titles that come from outside the window list (a node's or switch's name) changed: redraw them. */
export function refreshTitles() {
  titleVersion++;
  titleListeners.forEach((l) => l());
}

/** Re-renders when any window's title override changes. */
export function useTitleVersion(): number {
  return useSyncExternalStore(
    (l) => {
      titleListeners.add(l);
      return () => titleListeners.delete(l);
    },
    () => titleVersion,
  );
}

/** Reasons a window gives for not closing quietly, e.g. "/etc/hosts has unsaved changes". */
export function closeReasons(id: string): string[] {
  const out: string[] = [];
  for (const g of guards.get(id) ?? []) {
    try {
      const r = g.current();
      if (r) out.push(r);
    } catch {
      // A broken guard shouldn't stop the window from closing.
    }
  }
  return out;
}

/** Sends a window event; false when a handler called preventDefault (only close-request is cancelable). */
export function dispatchWindowEvent(id: string, name: WindowEventName): boolean {
  const e = new Event(`sparkbox:${name}`, { cancelable: name === "close-request" });
  for (const h of events.get(id)?.get(name) ?? []) {
    try {
      h.current(e);
    } catch (err) {
      console.error(err);
    }
  }
  return !e.defaultPrevented;
}

export function hasWindowEventHandler(id: string, name: WindowEventName): boolean {
  return (events.get(id)?.get(name)?.size ?? 0) > 0;
}

// --- Window controls style: macOS circles on the left, or WinBox buttons on the right.

export type ControlsStyle = "mac" | "winbox";
const CONTROLS_KEY = "sparkbox.wm.controls";
let controls: ControlsStyle = readJson<string>(CONTROLS_KEY, "mac") === "winbox" ? "winbox" : "mac";
const controlsListeners = new Set<() => void>();

export function setControlsStyle(style: ControlsStyle) {
  controls = style;
  writeJson(CONTROLS_KEY, style);
  controlsListeners.forEach((l) => l());
}

export function getControlsStyle(): ControlsStyle {
  return controls;
}

export function useControlsStyle(): ControlsStyle {
  return useSyncExternalStore(
    (l) => {
      controlsListeners.add(l);
      return () => controlsListeners.delete(l);
    },
    () => controls,
  );
}

// --- Snap preview while dragging: its own tiny store so it doesn't re-render windows.

let preview: Rect | null = null;
const previewListeners = new Set<() => void>();

export function setSnapPreview(r: Rect | null) {
  if (r === preview || (r && preview && r.x === preview.x && r.y === preview.y && r.w === preview.w && r.h === preview.h)) return;
  preview = r;
  previewListeners.forEach((l) => l());
}

function useSnapPreview(): Rect | null {
  return useSyncExternalStore(
    (l) => {
      previewListeners.add(l);
      return () => previewListeners.delete(l);
    },
    () => preview,
  );
}

// --- Contexts: stable actions, changing state, and the window a body sits in.

const ActionsCtx = createContext<WMActions | null>(null);
const StateCtx = createContext<WMState | null>(null);
const WinIdCtx = createContext<string>("");

/** Actions only: never re-renders on window changes. */
export function useWMActions(): WMActions {
  const a = useContext(ActionsCtx);
  if (!a) throw new Error("useWMActions outside WindowManagerProvider");
  return a;
}

export function useWMState(): WMState {
  const s = useContext(StateCtx);
  if (!s) throw new Error("useWMState outside WindowManagerProvider");
  return s;
}

/** Everything: actions plus the window list (re-renders when any window changes). */
export function useWM(): WM {
  const a = useWMActions();
  const s = useWMState();
  return useMemo(() => ({ ...a, ...s }), [a, s]);
}

// --- Hooks for window bodies

/** The id of the window this component is in ("" outside a window). */
export function useWindowId(): string {
  return useContext(WinIdCtx);
}

/**
 * Asks before the window closes. `reason` returns why closing would lose
 * something (e.g. "2 SSH sessions are open"), or null when it's fine to close.
 */
export function useCloseGuard(reason: () => string | null) {
  const id = useWindowId();
  const ref = useRef(reason);
  ref.current = reason;
  useEffect(() => {
    if (!id) return;
    let set = guards.get(id);
    if (!set) guards.set(id, (set = new Set()));
    set.add(ref);
    return () => {
      set!.delete(ref);
      if (!set!.size) guards.delete(id);
    };
  }, [id]);
}

/** Replaces the registry title while mounted, e.g. "● hosts — cell1". null keeps the default. */
export function useWindowTitle(title: string | null) {
  const id = useWindowId();
  useEffect(() => {
    if (!id) return;
    setTitleOverride(id, title);
  }, [id, title]);
  useEffect(() => () => setTitleOverride(id, null), [id]);
}

/** Handles ⌘R (refresh), ⌘F (find) or a close request for this window. */
export function useWindowEvent(name: WindowEventName, handler: (e: Event) => void) {
  const id = useWindowId();
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (!id) return;
    let byName = events.get(id);
    if (!byName) events.set(id, (byName = new Map()));
    let set = byName.get(name);
    if (!set) byName.set(name, (set = new Set()));
    set.add(ref);
    return () => {
      set!.delete(ref);
    };
  }, [id, name]);
}

/** State that's saved with the layout, so the window reopens the way it was left. */
/**
 * Runs `apply` when a link brings this window forward on a view again (see `setViewParams`), but
 * not when it first opens: then the view comes from params or the state saved with the layout.
 */
export function useViewLink(params: Params, apply: () => void) {
  const first = useRef(true);
  const latest = useRef(apply);
  latest.current = apply;
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    latest.current();
  }, [params.nav]);
}

export function useWinState<T>(key: string, initial: T): [T, (v: T | ((prev: T) => T)) => void] {
  const id = useWindowId();
  const actions = useContext(ActionsCtx);
  const [value, setValue] = useState<T>(() => {
    const saved = id ? actions?.getView(id)?.[key] : undefined;
    return saved === undefined ? initial : (saved as T);
  });
  const first = useRef(true);
  const latest = useRef(value);
  latest.current = value;
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    if (!id || !actions) return;
    const t = window.setTimeout(() => actions.setView(id, key, latest.current), 300);
    return () => window.clearTimeout(t);
  }, [value, id, key, actions]);
  // Write the last value on unmount (the window closing), so Undo brings it back.
  useEffect(() => () => {
    if (id && actions && !first.current) actions.setView(id, key, latest.current);
  }, [id, key, actions]);
  return [value, setValue];
}

// --- Provider

export function WindowManagerProvider({
  children, sizeOf, isKnown, minSizeOf, tileMinOf, titleOf, iconOf, helpOf, openElsewhere, popOutWindow,
}: {
  children: ReactNode;
  sizeOf: (kind: string) => [number, number];
  isKnown: (kind: string) => boolean;
  minSizeOf?: (kind: string) => [number, number];
  /** The smallest a kind may be tiled at (defaults to its minSize). */
  tileMinOf?: (kind: string) => [number, number];
  titleOf?: (w: Pick<WinState, "kind" | "params">) => string;
  iconOf?: (kind: string) => LucideIcon | undefined;
  /** The Help topic behind a window's title-bar info button. */
  helpOf?: (kind: string, params?: Params) => string | undefined;
  /** In a pop-out there's no desk: opening a window asks the main window instead. */
  openElsewhere?: (kind: string, params: Params) => void;
  /** Opens a pop-out for the window; resolves true once it's open (the window then leaves the desk). */
  popOutWindow?: (w: WinState) => Promise<boolean>;
}) {
  // Stacking order starts at 1..n each launch.
  const [windows, setWindows] = useState<WinState[]>(() => normalizeZ(loadSaved().filter((w) => isKnown(w.kind))));
  const [appFocused, setAppFocused] = useState(() => (typeof document === "undefined" ? true : document.hasFocus() || !inTauri));
  const [deskSize, setDeskSize] = useState<Size>(INITIAL_DESK);
  const desktopRef = useRef<HTMLDivElement | null>(null);
  const windowsRef = useRef(windows);
  windowsRef.current = windows;
  const reg = useRef({ sizeOf, isKnown, minSizeOf, tileMinOf, titleOf, iconOf, helpOf, openElsewhere, popOutWindow });
  reg.current = { sizeOf, isKnown, minSizeOf, tileMinOf, titleOf, iconOf, helpOf, openElsewhere, popOutWindow };
  const minOf = (kind: string): [number, number] => reg.current.minSizeOf?.(kind) ?? [360, 200];
  const tileMinFor = (kind: string): [number, number] => reg.current.tileMinOf?.(kind) ?? minOf(kind);
  const measured = useRef(false);

  const hasDock = windows.some((w) => w.minimized);
  const bounds = useMemo<Size>(() => ({ w: deskSize.w, h: Math.max(120, deskSize.h - (hasDock ? DOCK_H : 0)) }), [deskSize, hasDock]);
  const boundsRef = useRef(bounds);
  boundsRef.current = bounds;
  const deskRef = useRef(deskSize);
  deskRef.current = deskSize;

  const activeId = useMemo(() => {
    const visible = windows.filter((w) => !w.minimized);
    return visible.length ? visible.reduce((a, b) => (a.z > b.z ? a : b)).id : null;
  }, [windows]);
  const activeRef = useRef(activeId);
  activeRef.current = activeId;

  // Save the layout (and the last non-empty one, for "Restore last layout").
  useEffect(() => {
    const t = window.setTimeout(() => {
      const saved = windows.map(persisted);
      writeJson(STORAGE_KEY, saved);
      if (windows.length) writeJson(LAST_LAYOUT_KEY, saved);
    }, 300);
    return () => window.clearTimeout(t);
  }, [windows]);

  // Track the desktop's size; keep windows reachable when it shrinks.
  useLayoutEffect(() => {
    const el = desktopRef.current;
    if (!el) return;
    const measure = () => {
      // Not laid out yet (or hidden): keep the last size rather than squeezing windows.
      if (!el.clientWidth || !el.clientHeight) return;
      const w = el.clientWidth;
      const h = el.clientHeight;
      setDeskSize((s) => (s.w === w && s.h === h ? s : { w, h }));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // The first real size fits a restored layout to this desktop. Later, a desk that shrinks
  // (the app window made smaller, the sidebar shown) brings every window back onto it, moved
  // first and then narrowed toward its minimum, as macOS keeps windows in the visible frame;
  // when the desk grows again a window goes back to its own frame (C4).
  useEffect(() => {
    if (deskSize === INITIAL_DESK) return;
    const first = !measured.current;
    measured.current = true;
    setWindows((ws) => {
      let changed = false;
      const next = ws.map((w) => {
        if (w.maximized) return w;
        const own = !w.arranged && w.userFrame ? w.userFrame : w;
        // A restored layout comes back at the user's frames, inside the desktop and no smaller than each minimum.
        const r = w.snapped
          ? zoneRect(w.snapped, bounds)
          : first
            ? fitInside(savedFrame(w, reg.current.sizeOf(w.kind)), bounds, minOf(w.kind))
            : w.minimized
              ? clampRect(w, bounds)
              // A tiled or cascaded layout keeps its frames while they're on the desk (Tile re-runs on a resize).
              : w.arranged
                ? (w.x + w.w <= bounds.w && w.y + w.h <= bounds.h ? frameOf(w) : fitInside(w, bounds, minOf(w.kind)))
                : onTheDesk(own, bounds) ? frameOf(own) : fitInside(own, bounds, minOf(w.kind));
        if (first && !w.snapped && !w.userFrame) {
          changed = true;
          return { ...w, ...r, userFrame: r };
        }
        if (r.x === w.x && r.y === w.y && r.w === w.w && r.h === w.h) return w;
        changed = true;
        return { ...w, ...r };
      });
      return changed ? next : ws;
    });
  }, [bounds]);

  // Every inner window looks inactive while another app is in front.
  useEffect(() => {
    const on = () => setAppFocused(true);
    const off = () => setAppFocused(false);
    window.addEventListener("focus", on);
    window.addEventListener("blur", off);
    return () => {
      window.removeEventListener("focus", on);
      window.removeEventListener("blur", off);
    };
  }, []);

  const titleV = useTitleVersion();
  const titleFor = useCallback((w: Pick<WinState, "id" | "kind" | "params">) => (
    titleOverrides.get(w.id) ?? reg.current.titleOf?.(w) ?? w.kind
  ), []);

  // The native title follows the active window: Mission Control, the Window menu and VoiceOver show it.
  const activeWin = windows.find((w) => w.id === activeId);
  const nativeTitle = activeWin ? `SparkBox — ${titleFor(activeWin)}` : "SparkBox";
  useEffect(() => {
    // A pop-out titles its own native window.
    if (!inTauri || reg.current.openElsewhere) return;
    getCurrentWindow().setTitle(nativeTitle).catch(() => {});
  }, [nativeTitle, titleV]);

  // Commands registered by a window only show while it's the active one.
  useEffect(() => setActiveScope(activeId), [activeId]);

  // A success toast from the window in front isn't needed: the window shows the result.
  const appFocusedRef = useRef(appFocused);
  appFocusedRef.current = appFocused;
  useEffect(() => {
    setFrontWindowProbe((id) => {
      const w = windowsRef.current.find((x) => x.id === id);
      const visible = typeof document === "undefined" || document.visibilityState !== "hidden";
      return !!w && !w.minimized && activeRef.current === id && appFocusedRef.current && visible;
    }, () => activeRef.current);
    return () => setFrontWindowProbe(() => false);
  }, []);

  const arranging = useCallback(() => {
    const el = desktopRef.current;
    if (!el || reducedMotion()) return;
    el.classList.add("arranging");
    window.setTimeout(() => el.classList.remove("arranging"), ARRANGE_MS);
  }, []);

  /** Applies an update, keeping z within the window layer. */
  const commit = useCallback((fn: (ws: WinState[]) => WinState[]) => {
    setWindows((ws) => {
      const next = fn(ws);
      return next.some((w) => w.z > Z_LIMIT) ? normalizeZ(next) : next;
    });
  }, []);

  const raised = (ws: WinState[], id: string, extra: Partial<WinState> = {}) => {
    const top = ws.reduce((m, w) => Math.max(m, w.z), 0);
    return ws.map((w) => (w.id === id ? { ...w, ...extra, z: top + 1 } : w));
  };

  const rememberGeometry = (wins: WinState[]) => {
    const geom = readJson<Record<string, Rect>>(GEOM_KEY, {});
    for (const w of wins) geom[w.id] = persisted(w).userFrame!;
    // Keep it from growing forever: the 60 most recent ids.
    const keys = Object.keys(geom);
    for (const k of keys.slice(0, Math.max(0, keys.length - 60))) delete geom[k];
    writeJson(GEOM_KEY, geom);
  };

  const open = useCallback((kind: string, params: Params = {}) => {
    raises++;
    if (reg.current.openElsewhere) return reg.current.openElsewhere(kind, params);
    const id = winId(kind, params);
    const existing = windowsRef.current.find((w) => w.id === id);
    if (existing) {
      // A link to another view of an open window (Settings › Nodes, a node's GPU tab) moves it there.
      const view = Object.keys(params).some((k) => isView(kind, k))
        ? { params: { ...params, nav: String(Number(existing.params.nav ?? 0) + 1) } }
        : {};
      commit((ws) => raised(ws, id, { minimized: false, ...view }));
      return;
    }
    const b = boundsRef.current;
    const size = reg.current.sizeOf(kind);
    const min = minOf(kind);
    const saved = readJson<Record<string, Rect>>(GEOM_KEY, {})[id];
    // Never exactly on top of an open window, and never off the desk: it steps off the other's
    // corner, narrowing toward its minimum near the edge, or starts a new lap (C4).
    const r = openFrame(size, min, saved, windowsRef.current.length, visibleCorners(windowsRef.current), b);
    commit((ws) => {
      if (ws.some((w) => w.id === id)) return raised(ws, id, { minimized: false });
      const top = ws.reduce((m, w) => Math.max(m, w.z), 0);
      return [...ws, { id, kind, params, ...r, userFrame: r, z: top + 1, minimized: false, maximized: false, snapped: null, restore: null }];
    });
  }, [commit]);

  useEffect(() => {
    openFromAnywhere = open;
    return () => {
      if (openFromAnywhere === open) openFromAnywhere = null;
    };
  }, [open]);

  const focus = useCallback((id: string) => {
    raises++;
    const ws = windowsRef.current;
    const top = ws.reduce((m, w) => Math.max(m, w.z), 0);
    const w = ws.find((x) => x.id === id);
    if (!w || (w.z === top && !w.minimized)) return;
    commit((all) => raised(all, id, { minimized: false }));
  }, [commit]);

  /** A move or resize by hand: it becomes the window's own frame. */
  const move = useCallback((id: string, g: Geometry) => {
    commit((ws) => ws.map((w) => {
      if (w.id !== id) return w;
      const next = { ...w, ...g };
      return { ...next, userFrame: frameOf(next), arranged: false, maximized: false, snapped: null, restore: null };
    }));
  }, [commit]);

  const restoreWindows = useCallback((wins: WinState[]) => {
    const b = boundsRef.current;
    const openIds = new Set(windowsRef.current.map((w) => w.id));
    const fresh = normaliseIds(wins.filter((w) => isWin(w) && reg.current.isKnown(w.kind)))
      .filter((w) => !openIds.has(w.id))
      .sort((a, c) => a.z - c.z);
    if (!fresh.length) {
      // Already open: bring the newest of them forward.
      const last = wins[wins.length - 1];
      const lastId = last && winId(last.kind, last.params ?? {});
      if (lastId) commit((ws) => (ws.some((w) => w.id === lastId) ? raised(ws, lastId, { minimized: false }) : ws));
      return;
    }
    commit((ws) => {
      let top = ws.reduce((m, w) => Math.max(m, w.z), 0);
      const taken = visibleCorners(ws);
      const add = fresh
        .filter((w) => !ws.some((x) => x.id === w.id))
        .map((w) => {
          // Back at the user's own frame, never a tiled or snapped one, and stepped off any open window it would cover.
          const back = fitInside(savedFrame(w, reg.current.sizeOf(w.kind)), b, minOf(w.kind));
          const r = w.minimized ? back : cascadeFree(back, taken, b, minOf(w.kind));
          if (!w.minimized) taken.push(r);
          return {
            ...w, ...r, userFrame: r, params: w.params ?? {}, z: ++top, minimized: !!w.minimized && !w.tileMinimized,
            snapped: null, restore: w.maximized ? r : null, arranged: false, tileMinimized: false,
          };
        });
      return [...ws, ...add];
    });
  }, [commit]);

  const pushClosed = (wins: WinState[]) => {
    const groups = [{ wins, at: Date.now() }, ...closedGroups()].slice(0, CLOSED_MAX);
    writeJson(CLOSED_KEY, groups);
  };

  const forgetClosed = (ids: string[]) => {
    writeJson(CLOSED_KEY, closedGroups()
      .map((g) => ({ ...g, wins: g.wins.filter((w) => !ids.includes(w.id)) }))
      .filter((g) => g.wins.length));
  };

  /** Removes windows for real: no questions, but with Undo. */
  const doClose = useCallback((ids: string[], label: string) => {
    const closing = windowsRef.current.filter((w) => ids.includes(w.id));
    if (!closing.length) return;
    rememberGeometry(closing);
    pushClosed(closing);
    for (const id of ids) cancelDialogsFor(id);
    commit((ws) => ws.filter((w) => !ids.includes(w.id)));
    // Closing one after another folds into one offer: "3 windows closed".
    pushUndo({
      label,
      collapse: { key: "close-windows", count: closing.length, label: (n) => `${plural(n, "window")} closed` },
      undo: () => {
        forgetClosed(closing.map((w) => w.id));
        restoreWindows(closing);
      },
    });
  }, [commit, restoreWindows]);

  // For dialogs and toasts: without the "●" an edited window puts in front of its title.
  const titleOfId = (id: string) => {
    const w = windowsRef.current.find((x) => x.id === id);
    return w ? titleFor(w).replace(/^●\s*/, "") : id;
  };

  const requestClose = useCallback(async (id: string) => {
    if (!windowsRef.current.some((w) => w.id === id)) return false;
    if (!dispatchWindowEvent(id, "close-request")) return false;
    const title = titleOfId(id);
    const reasons = closeReasons(id);
    if (reasons.length) {
      const ok = await confirmAction({
        title: `Close “${title}”?`,
        body: <ReasonList reasons={reasons} />,
        confirmLabel: "Close anyway",
        danger: true,
        winId: id,
      });
      if (!ok) return false;
    }
    doClose([id], `Closed “${title}”`);
    return true;
  }, [doClose]);

  const requestCloseMany = useCallback(async (ids: string[], dialogTitle: string) => {
    if (!ids.length) return true;
    const reasons = ids.flatMap((id) => closeReasons(id).map((r) => `${titleOfId(id)}: ${r}`));
    if (reasons.length) {
      const ok = await confirmAction({
        title: dialogTitle,
        body: <ReasonList reasons={reasons} />,
        confirmLabel: "Close anyway",
        danger: true,
      });
      if (!ok) return false;
    }
    const n = ids.length;
    doClose(ids, n === 1 ? `Closed “${titleOfId(ids[0])}”` : `${plural(n, "window")} closed`);
    return true;
  }, [doClose]);

  const requestCloseAll = useCallback(
    () => requestCloseMany(windowsRef.current.map((w) => w.id), "Close all windows?"),
    [requestCloseMany],
  );

  const requestCloseOthers = useCallback(
    (keep: string) => requestCloseMany(windowsRef.current.filter((w) => w.id !== keep).map((w) => w.id), "Close the other windows?"),
    [requestCloseMany],
  );

  /** Pop-out: only a window that would close without losing anything (a fresh copy opens outside). */
  const popOut = useCallback(async (id: string) => {
    const w = windowsRef.current.find((x) => x.id === id);
    const make = reg.current.popOutWindow;
    if (!w || !make) return false;
    const reasons = closeReasons(id);
    if (reasons.length) {
      await confirmAction({
        title: `Open “${titleOfId(id)}” in a separate window?`,
        body: <>{reasons[0]}. Save or finish it first: the separate window starts fresh.</>,
        confirmLabel: "OK",
        cancelLabel: "Cancel",
        winId: id,
      });
      return false;
    }
    if (!(await make(w))) return false;
    cancelDialogsFor(id);
    commit((ws) => ws.filter((x) => x.id !== id));
    return true;
  }, [commit]);

  const reopenLast = useCallback(() => {
    const [latest, ...rest] = closedGroups();
    if (!latest) return;
    writeJson(CLOSED_KEY, rest);
    restoreWindows(latest.wins);
  }, [restoreWindows]);

  const commitMinimize = useCallback((id: string) => {
    commit((ws) => ws.map((w) => (w.id === id ? { ...w, minimized: true } : w)));
  }, [commit]);

  const minimize = useCallback((id: string) => {
    const el = windowElement(id);
    const w = windowsRef.current.find((x) => x.id === id);
    if (!w || w.minimized) return;
    if (!el || reducedMotion()) return commitMinimize(id);
    // Shrink toward the dock row at the bottom left, then hide.
    const r = el.getBoundingClientRect();
    const desk = desktopRef.current?.getBoundingClientRect();
    if (desk) el.style.transformOrigin = `${desk.left + 40 - r.left}px ${desk.bottom - r.top}px`;
    el.classList.add("minimizing");
    window.setTimeout(() => {
      flushSync(() => commitMinimize(id));
      el.classList.remove("minimizing");
      el.style.transformOrigin = "";
    }, 170);
  }, [commitMinimize]);

  const toggleMinimize = useCallback((id: string) => {
    const w = windowsRef.current.find((x) => x.id === id);
    if (!w) return;
    if (w.minimized) commit((ws) => raised(ws, id, { minimized: false }));
    else minimize(id);
  }, [commit, minimize]);

  const snap = useCallback((id: string, zone: SnapTarget, from?: Rect) => {
    const b = boundsRef.current;
    arranging();
    commit((ws) => {
      const top = ws.reduce((m, w) => Math.max(m, w.z), 0);
      return ws.map((w) => {
        if (w.id !== id) return w;
        const restore = w.maximized || w.snapped ? (w.restore ?? null) : (from ?? { x: w.x, y: w.y, w: w.w, h: w.h });
        const z = top + 1;
        if (zone === "restore") {
          if (!w.maximized && !w.snapped) return { ...w, z };
          const back = fitInside(w.restore ?? w, b, minOf(w.kind));
          return { ...w, ...back, z, maximized: false, snapped: null, restore: null, minimized: false };
        }
        if (zone === "max") return { ...w, z, maximized: true, snapped: null, restore, minimized: false };
        return { ...w, ...zoneRect(zone, b), z, maximized: false, snapped: zone, restore, minimized: false };
      });
    });
  }, [commit, arranging]);

  const toggleMaximize = useCallback((id: string) => {
    const w = windowsRef.current.find((x) => x.id === id);
    if (!w) return;
    snap(id, w.maximized ? "restore" : "max");
  }, [snap]);

  const arrangeable = (ws: WinState[]) => ws.map((w) => ({ id: w.id, size: reg.current.sizeOf(w.kind), min: minOf(w.kind) }));

  const cascade = useCallback(() => {
    lastTile.current = null;
    arranging();
    commit((ws) => {
      const order = [...ws].sort((a, c) => a.z - c.z);
      const layout = cascadeLayout(arrangeable(order), boundsRef.current);
      return ws.map((w) => ({
        ...w, ...layout.get(w.id)!, userFrame: w.userFrame ?? frameOf(w), arranged: true,
        minimized: false, tileMinimized: false, maximized: false, snapped: null, restore: null,
      }));
    });
  }, [commit, arranging]);

  const restoreLayout = useCallback(() => {
    lastTile.current = null;
    // What the arrangement changed outside the windows (the sidebar it hid) comes back too.
    const undoExtra = onRestore.current;
    onRestore.current = null;
    undoExtra?.();
    const b = { w: deskRef.current.w, h: deskRef.current.h };
    const stillDocked = windowsRef.current.some((w) => w.minimized && !w.tileMinimized);
    const desk = stillDocked ? { ...b, h: b.h - DOCK_H } : b;
    arranging();
    commit((ws) => ws.map((w) => {
      if (!w.arranged && !w.tileMinimized) return w;
      const r = fitInside(w.userFrame ?? w, desk, minOf(w.kind));
      return { ...w, ...r, arranged: false, minimized: w.tileMinimized ? false : w.minimized, tileMinimized: false };
    }));
  }, [commit, arranging]);

  /** The last Tile's mode, so a tiled desktop can tile again when it changes size. */
  const lastTile = useRef<TileMode | null>(null);
  /** Run by Restore layout: undoes what a Tile changed besides the windows (Hide sidebar and tile all). */
  const onRestore = useRef<(() => void) | null>(null);
  /** A Tile waiting for the desktop to change size (Hide sidebar and tile all). */
  const pendingTile = useRef<{ mode: TileMode; timer: number } | null>(null);

  /** Lays out the visible windows for `mode` on the desktop as it is now (or `extraW` px wider). */
  const tileNow = (ws: WinState[], mode: TileMode, extraW = 0) => {
    // Most recently focused first: they're the ones kept when not everything fits.
    const visible = ws.filter((w) => !w.minimized).sort((a, c) => c.z - a.z);
    const wins = visible.map((w) => ({ id: w.id, size: reg.current.sizeOf(w.kind), min: tileMinFor(w.kind) }));
    const shape = mode === "two" ? ("row" as const) : mode === "four" ? ("grid" as const) : ("auto" as const);
    const opts = { gutter: 8, margin: 8, max: mode === "two" ? 2 : mode === "four" ? 4 : undefined, shape };
    const desk = { w: deskRef.current.w + extraW, h: deskRef.current.h };
    const docked = ws.some((w) => w.minimized);
    let result = tileLayout(wins, { w: desk.w, h: desk.h - (docked ? DOCK_H : 0) }, opts);
    // Sending windows to the dock makes it appear, so lay out again above it.
    if (!docked && result.minimized.length) result = tileLayout(wins, { w: desk.w, h: desk.h - DOCK_H }, opts);
    const away = new Set(result.minimized);
    const next = ws.map((w) => {
      const r = result.frames.get(w.id);
      if (r) return { ...w, ...r, userFrame: w.userFrame ?? frameOf(w), arranged: true, maximized: false, snapped: null, restore: null };
      if (away.has(w.id)) return { ...w, userFrame: w.userFrame ?? frameOf(w), minimized: true, tileMinimized: true };
      return w;
    });
    return { next, tiled: result.frames.size, away: away.size, stacked: !!result.stacked, of: visible.length };
  };

  const tilePreview = useCallback((mode: TileMode, extraW = 0): TilePreview => {
    const { tiled, of, stacked } = tileNow(windowsRef.current, mode, extraW);
    return { tiled, of: mode === "two" ? Math.min(2, of) : mode === "four" ? Math.min(4, of) : of, stacked };
  }, []);

  const tile = useCallback((mode: TileMode = "all", opts: { afterResize?: boolean; onRestore?: () => void } = {}) => {
    if (!windowsRef.current.some((w) => !w.minimized)) return;
    if (opts.onRestore) onRestore.current = opts.onRestore;
    if (opts.afterResize) {
      // Tiles when the desktop has its new size, or in a moment if it never changes.
      if (pendingTile.current) window.clearTimeout(pendingTile.current.timer);
      pendingTile.current = { mode, timer: window.setTimeout(() => runPendingTile(), 400) };
      return;
    }
    const { tiled, away, stacked } = tileNow(windowsRef.current, mode);
    lastTile.current = mode;
    arranging();
    commit((all) => tileNow(all, mode).next);
    // One notice per arrangement: a second Tile replaces it (and its Undo puts back the first layout).
    const label = stacked ? STACKED_NOTE : away ? `Tiled ${plural(tiled, "window")} · ${away} minimized` : null;
    if (label) pushUndo({ label, undo: restoreLayout, collapse: { key: "tile", count: 1, label: () => label } });
  }, [commit, arranging, restoreLayout]);

  const runPendingTile = () => {
    const p = pendingTile.current;
    if (!p) return;
    window.clearTimeout(p.timer);
    pendingTile.current = null;
    tile(p.mode);
  };

  // A tiled desktop that changes size (the sidebar shown or hidden, the app window resized)
  // tiles again, as long as nothing has been moved by hand since.
  const tiledDesk = useRef(deskSize);
  useEffect(() => {
    const before = tiledDesk.current;
    tiledDesk.current = deskSize;
    if (pendingTile.current && before !== INITIAL_DESK && (before.w !== deskSize.w || before.h !== deskSize.h)) return runPendingTile();
    const mode = lastTile.current;
    if (!mode || before === INITIAL_DESK || (before.w === deskSize.w && before.h === deskSize.h)) return;
    const visible = windowsRef.current.filter((w) => !w.minimized);
    if (!visible.length || !visible.every((w) => w.arranged)) return;
    commit((all) => tileNow(all, mode).next);
  }, [deskSize]);

  const cycle = useCallback((dir: 1 | -1) => {
    const visible = windowsRef.current.filter((w) => !w.minimized);
    if (visible.length < 2) return;
    const i = visible.findIndex((w) => w.id === activeRef.current);
    const next = visible[(i + dir + visible.length) % visible.length];
    commit((ws) => raised(ws, next.id));
  }, [commit]);

  const bringAllToFront = useCallback(() => {
    commit((ws) => ws.map((w) => (w.minimized ? { ...w, minimized: false } : w)));
  }, [commit]);

  const setView = useCallback((id: string, key: string, value: unknown) => {
    commit((ws) => ws.map((w) => {
      if (w.id !== id || w.view?.[key] === value) return w;
      return { ...w, view: { ...w.view, [key]: value } };
    }));
  }, [commit]);

  const actions = useMemo<WMActions>(() => ({
    desktopRef,
    open,
    close: (id) => void requestClose(id),
    closeAll: () => void requestCloseAll(),
    requestClose,
    requestCloseAll,
    requestCloseOthers,
    focus,
    move,
    minimize,
    toggleMinimize,
    toggleMaximize,
    snap,
    cascade,
    tile,
    tilePreview,
    restoreLayout,
    canRestoreLayout: () => windowsRef.current.some((w) => w.arranged || w.tileMinimized),
    detach: (id) => {
      cancelDialogsFor(id);
      commit((ws) => ws.filter((w) => w.id !== id));
    },
    popOut: popOutWindow ? popOut : null,
    cycle,
    reopenLast,
    restoreWindows,
    bringAllToFront,
    getWindows: () => windowsRef.current,
    getActiveId: () => activeRef.current,
    getBounds: () => boundsRef.current,
    titleFor,
    iconFor: (kind) => reg.current.iconOf?.(kind),
    helpFor: (kind, params) => reg.current.helpOf?.(kind, params),
    minSizeFor: minOf,
    tileMinFor,
    setView,
    getView: (id) => windowsRef.current.find((w) => w.id === id)?.view,
  }), [open, requestClose, requestCloseAll, requestCloseOthers, focus, move, minimize, toggleMinimize, toggleMaximize,
    snap, cascade, tile, tilePreview, restoreLayout, cycle, reopenLast, restoreWindows, bringAllToFront, titleFor, setView, popOut, !!popOutWindow]);

  const state = useMemo<WMState>(() => ({ windows, activeId, appFocused, bounds }), [windows, activeId, appFocused, bounds]);

  return (
    <ActionsCtx.Provider value={actions}>
      <StateCtx.Provider value={state}>{children}</StateCtx.Provider>
    </ActionsCtx.Provider>
  );
}

function ReasonList({ reasons }: { reasons: string[] }) {
  if (reasons.length === 1) return <>{reasons[0]}. Closing it now loses that.</>;
  return (
    <>
      <span>Closing now loses:</span>
      <ul className="dialog-list">{reasons.map((r) => <li key={r}>{r}</li>)}</ul>
    </>
  );
}

export type BodyComponent = ComponentType<{ kind: string; params: Params }>;

/**
 * The area windows live in. Every window stays mounted while minimized, so
 * terminals, editors and half-filled forms keep their state. `Body` is
 * memoized so moving a window doesn't re-render its contents.
 */
export function Desktop({ Body, wrap, empty, overlays, overview = false, onOverviewDone }: {
  Body: BodyComponent;
  /** Wraps each window's body, e.g. in an error boundary. */
  wrap?: ComponentType<{ id: string; title: string; children: ReactNode }>;
  /** Shown when no windows are open. */
  empty?: ReactNode;
  /** Extra layers over the desktop, e.g. toasts. */
  overlays?: ReactNode;
  /** Overview (F3): every window, minimized ones too, scaled into a grid to pick from. */
  overview?: boolean;
  onOverviewDone?: () => void;
}) {
  const actions = useWMActions();
  const { windows, activeId, appFocused, bounds } = useWMState();
  useTitleVersion();
  const MemoBody = useMemo(() => memo(Body), [Body]);
  const Wrap = wrap;
  const minimized = windows.filter((w) => w.minimized);
  const showOverview = overview && windows.length > 0;
  const [pick, setPick] = useState(0);

  // Animate into and out of the overview.
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    const el = actions.desktopRef.current;
    if (!el || reducedMotion()) return;
    el.classList.add("arranging");
    const t = window.setTimeout(() => el.classList.remove("arranging"), 260);
    return () => window.clearTimeout(t);
  }, [showOverview]);

  const cells = useMemo(
    () => (showOverview ? tileCells(windows.length, { w: bounds.w - 40, h: bounds.h - 40 }).map((c) => ({ ...c, x: c.x + 20, y: c.y + 20 })) : []),
    [showOverview, windows.length, bounds],
  );

  useEffect(() => {
    if (!showOverview) return;
    setPick(Math.max(0, windows.findIndex((w) => w.id === activeId)));
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onOverviewDone?.();
      } else if (e.key === "ArrowRight" || e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) {
        e.preventDefault();
        setPick((p) => (p + 1) % windows.length);
      } else if (e.key === "ArrowLeft" || e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
        e.preventDefault();
        setPick((p) => (p - 1 + windows.length) % windows.length);
      } else if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        setPick((p) => {
          const w = windows[p];
          if (w) actions.focus(w.id);
          onOverviewDone?.();
          return p;
        });
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [showOverview, windows.length]);

  const choose = (id: string) => {
    actions.focus(id);
    onOverviewDone?.();
  };

  return (
    <div className={`desktop${minimized.length ? " has-dock" : ""}${showOverview ? " overview" : ""}`} ref={actions.desktopRef}>
      {windows.map((w, i) => {
        const title = actions.titleFor(w);
        const body = <MemoBody kind={w.kind} params={w.params} />;
        return (
          <WinIdCtx.Provider key={w.id} value={w.id}>
            <WindowFrame
              win={w}
              active={w.id === activeId}
              appFocused={appFocused}
              title={title}
              icon={actions.iconFor(w.kind)}
              help={actions.helpFor(w.kind, w.params)}
              bounds={bounds}
              overviewCell={showOverview ? cells[i] : null}
              picked={showOverview && i === pick}
              onPick={() => choose(w.id)}
            >
              {Wrap ? <Wrap id={w.id} title={title}>{body}</Wrap> : body}
            </WindowFrame>
          </WinIdCtx.Provider>
        );
      })}
      {showOverview && windows.map((w, i) => {
        const c = cells[i];
        const Icon = actions.iconFor(w.kind);
        const r = w.maximized ? { x: 0, y: 0, w: bounds.w, h: bounds.h } : w;
        return (
          <button
            key={w.id}
            className={`overview-label${i === pick ? " picked" : ""}`}
            style={{ left: c.x, top: overviewPlacement(c, r).labelY, width: c.w }}
            onClick={() => choose(w.id)}
            onPointerEnter={() => setPick(i)}
          >
            {Icon && <Icon size={13} aria-hidden />}
            <span>{actions.titleFor(w)}</span>
            {w.minimized && <span className="overview-tag">minimized</span>}
          </button>
        );
      })}
      <SnapPreview />
      {windows.length === 0 && empty}
      {minimized.length > 0 && !showOverview && <Dock wins={minimized} />}
      {overlays}
    </div>
  );
}

function SnapPreview() {
  const r = useSnapPreview();
  if (!r) return null;
  return <div className="snap-preview" style={{ left: r.x, top: r.y, width: r.w, height: r.h }} aria-hidden />;
}

/** The row of minimized windows. */
function Dock({ wins }: { wins: WinState[] }) {
  const actions = useWMActions();
  return (
    <div className="dock" role="toolbar" aria-label="Minimized windows">
      {wins.map((w) => {
        const title = actions.titleFor(w);
        const Icon = actions.iconFor(w.kind);
        return (
          <div
            key={w.id}
            className="dock-chip"
            onContextMenu={(e) => showContextMenu(e, [
              { label: "Restore", run: () => actions.focus(w.id) },
              { separator: true },
              { label: "Close", run: () => void actions.requestClose(w.id) },
            ])}
          >
            <button className="dock-chip-main" title={`Restore ${title}`} onClick={() => actions.focus(w.id)}>
              {Icon && <Icon size={13} aria-hidden />}
              <span className="dock-chip-title">{title}</span>
            </button>
            <button className="dock-chip-close" title={`Close ${title}`} aria-label={`Close ${title}`} onClick={() => void actions.requestClose(w.id)}>
              <X size={11} aria-hidden />
            </button>
          </div>
        );
      })}
    </div>
  );
}
