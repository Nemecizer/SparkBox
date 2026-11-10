import { useSyncExternalStore } from "react";
import { AppWindow, ChevronDown, LayoutGrid, Layers, LayoutTemplate, PanelLeftClose, PanelsTopLeft, Save, SquareX, Trash2 } from "lucide-react";
import { runCommand } from "../lib/commands";
import { menuAt, type MenuItem } from "../lib/contextMenu";
import { shortcutFor } from "../lib/keymap";
import { openPanel, promptText } from "../lib/session";
import { toast } from "../lib/toasts";
import { useWMActions, useWMState, type WinState, type WMActions } from "../wm/WindowManager";

// --- Tile all, saying what it does on this desktop.

/** The sidebar's width while it shows: what hiding it would add to the desktop. */
function sidebarWidth(): number {
  if (typeof document === "undefined") return 0;
  return document.querySelector<HTMLElement>(".app:not([data-sidebar='hidden']) .main > .sidebar")?.offsetWidth ?? 0;
}

/**
 * "Tile all", or "Tile all — fits 4 of 6 here" when some would go to the dock, plus "Hide
 * sidebar and tile all (fits 6)" when the sidebar's room would fit more.
 */
export function tileAllItems(wm: WMActions, visible: number): MenuItem[] {
  const base = { icon: LayoutGrid, shortcut: shortcutFor("wm:tile") };
  if (!visible) return [{ ...base, label: "Tile all", disabled: "No windows are open", run: () => {} }];
  const here = wm.tilePreview("all");
  const items: MenuItem[] = [{ ...base, label: here.tiled < here.of ? `Tile all — fits ${here.tiled} of ${here.of} here` : "Tile all", run: () => wm.tile("all") }];
  const extra = here.tiled < here.of ? sidebarWidth() : 0;
  const wide = extra ? wm.tilePreview("all", extra) : null;
  if (wide && wide.tiled > here.tiled) {
    items.push({
      label: `Hide sidebar and tile all (fits ${wide.tiled})`,
      icon: PanelLeftClose,
      // The same command as ⌘K and the View menu: Restore layout shows the sidebar again.
      run: () => void runCommand("wm:tile-wide"),
    });
  }
  return items;
}

// --- Named layouts ("Save layout…"): kept in localStorage by name.

const LAYOUTS_KEY = "sparkbox.layouts.v1";
const layoutListeners = new Set<() => void>();
let layoutsCache: Record<string, WinState[]> | null = null;

function readLayouts(): Record<string, WinState[]> {
  if (layoutsCache) return layoutsCache;
  try {
    const v = JSON.parse(localStorage.getItem(LAYOUTS_KEY) ?? "{}");
    layoutsCache = v && typeof v === "object" && !Array.isArray(v) ? v : {};
  } catch {
    layoutsCache = {};
  }
  return layoutsCache!;
}

function writeLayouts(next: Record<string, WinState[]>) {
  layoutsCache = next;
  try {
    localStorage.setItem(LAYOUTS_KEY, JSON.stringify(next));
  } catch {
    // Kept for this session only.
  }
  layoutListeners.forEach((l) => l());
}

export function layoutNames(): string[] {
  return Object.keys(readLayouts()).sort((a, b) => a.localeCompare(b));
}

export function useLayoutNames(): string[] {
  const all = useSyncExternalStore(
    (l) => {
      layoutListeners.add(l);
      return () => layoutListeners.delete(l);
    },
    readLayouts,
  );
  return Object.keys(all).sort((a, b) => a.localeCompare(b));
}

/** Asks for a name and saves the open windows under it. */
export async function saveLayoutAs(wm: WMActions) {
  const wins = wm.getWindows();
  if (!wins.length) return;
  const name = await promptText({
    title: "Save layout",
    body: `Saves where the ${wins.length} open window${wins.length === 1 ? " is" : "s are"}, so you can bring them back from the Windows menu.`,
    label: "Name",
    placeholder: "e.g. Model bring-up",
    confirmLabel: "Save",
    validate: (v) => (v.length > 40 ? "Keep it under 40 characters." : null),
  });
  if (!name) return;
  // What's on screen now becomes each window's own frame when the layout comes back.
  writeLayouts({ ...readLayouts(), [name]: wins.map(({ view, ...w }) => ({ ...w, userFrame: { x: w.x, y: w.y, w: w.w, h: w.h }, view })) });
  toast({ kind: "success", title: `Saved layout “${name}”` });
}

/** Replaces the open windows with a saved layout (closing them can be undone). */
export async function applyLayout(wm: WMActions, name: string) {
  const saved = readLayouts()[name];
  if (!saved) return;
  if (wm.getWindows().length && !(await wm.requestCloseAll())) return;
  wm.restoreWindows(saved);
}

export function deleteLayout(name: string) {
  const next = { ...readLayouts() };
  delete next[name];
  writeLayouts(next);
}

/** The window list and arranging commands, as menu items (the TopBar button and the palette share it). */
export function windowsMenuItems(wm: WMActions, windows: WinState[], activeId: string | null, layouts: string[]): MenuItem[] {
  const list: MenuItem[] = windows.length
    ? windows.map((w) => ({
        label: wm.titleFor(w),
        hint: w.minimized ? "minimized" : undefined,
        icon: wm.iconFor(w.kind),
        checked: w.id === activeId,
        run: () => wm.focus(w.id),
      }))
    : [{ label: "No windows open", disabled: true }];
  const none = windows.length ? false : "No windows are open";
  const layoutItems: MenuItem[] = layouts.length
    ? [
        {
          label: "Layouts",
          icon: LayoutTemplate,
          submenu: [
            ...layouts.map((n): MenuItem => ({ label: n, run: () => void applyLayout(wm, n) })),
            { separator: true },
            { label: "Delete a layout", submenu: layouts.map((n): MenuItem => ({ label: n, icon: Trash2, danger: true, run: () => deleteLayout(n) })) },
          ],
        },
      ]
    : [];
  return [
    ...list,
    { separator: true },
    { label: "Show all windows", icon: PanelsTopLeft, shortcut: "F3", disabled: none, run: () => openPanel("overview") },
    ...tileAllItems(wm, windows.filter((w) => !w.minimized).length),
    { label: "Cascade", icon: Layers, shortcut: shortcutFor("wm:cascade"), disabled: none, run: () => wm.cascade() },
    { label: "Restore layout", disabled: wm.canRestoreLayout() ? false : "Nothing has been tiled or cascaded", run: () => wm.restoreLayout() },
    { label: "Bring all to front", icon: AppWindow, disabled: windows.some((w) => w.minimized) ? false : "Nothing is minimized", run: wm.bringAllToFront },
    { separator: true },
    { label: "Save layout…", icon: Save, disabled: none, run: () => void saveLayoutAs(wm) },
    ...layoutItems,
    { separator: true },
    { label: "Close all", icon: SquareX, shortcut: shortcutFor("wm:close-all"), disabled: none, run: () => void wm.requestCloseAll() },
  ];
}

/** The TopBar's Windows button: every open window, plus arranging and layouts. */
export function WindowsMenu() {
  const wm = useWMActions();
  const { windows, activeId } = useWMState();
  const layouts = useLayoutNames();
  return (
    <button
      className="tool windows-menu-btn"
      aria-haspopup="menu"
      title="Windows: switch, arrange, save a layout"
      onClick={(e) => menuAt(e.currentTarget, windowsMenuItems(wm, windows, activeId, layouts), { fromKeyboard: e.detail === 0 })}
    >
      <AppWindow size={14} aria-hidden />
      <span>Windows</span>
      {windows.length > 0 && <span className="windows-count">{windows.length}</span>}
      <ChevronDown size={12} aria-hidden />
    </button>
  );
}
