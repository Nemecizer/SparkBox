// Right-click menus. A component calls showContextMenu(e, items) from its
// onContextMenu handler; ContextMenuHost draws the menu and handles the keys.
import type { MouseEvent as ReactMouseEvent } from "react";
import { useSyncExternalStore } from "react";
import { Copy, FolderOpen, LayoutList, Power, RotateCcw, Server, SquareTerminal, Sunrise, type LucideIcon } from "lucide-react";
import type { NodeSnapshot } from "./types";
import { POWER } from "./vocabulary";

export type MenuItem =
  | {
      label: string;
      icon?: LucideIcon;
      shortcut?: string;
      /** true, or the reason it's unavailable (shown under the label). */
      disabled?: boolean | string;
      danger?: boolean;
      /** Shows a checkmark. */
      checked?: boolean;
      /** A small tag after the label, e.g. "minimized". */
      hint?: string;
      /** Needs Admin mode, which is locked: a trailing lock, and disabled in look-only mode (ContextMenuHost, admin.ts lockOf). */
      locked?: boolean;
      run?: () => unknown;
    }
  | { separator: true }
  | { label: string; icon?: LucideIcon; disabled?: boolean | string; submenu: MenuItem[] };

export interface OpenMenu {
  x: number;
  y: number;
  items: MenuItem[];
  /** Gets focus back when the menu closes. */
  opener: HTMLElement | null;
  /** Opened from the keyboard: the first item starts highlighted. */
  fromKeyboard: boolean;
  /**
   * The button it hangs from (menuAt): it opens under the button's left edge, flips above
   * it when there's no room below, and lines up with its right edge when there's none right.
   */
  anchor?: { left: number; right: number; top: number; bottom: number };
}

let current: OpenMenu | null = null;
const listeners = new Set<() => void>();

function set(m: OpenMenu | null) {
  current = m;
  listeners.forEach((l) => l());
}

/**
 * Opens a menu at the pointer (or under the focused element for Shift+F10). A menu a button
 * opens on click hangs from that button instead (menuAt), so a click from the keyboard, with
 * no pointer position, never draws it at the screen's corner.
 */
export function showContextMenu(e: MouseEvent | ReactMouseEvent, items: MenuItem[]) {
  e.preventDefault();
  e.stopPropagation();
  if (!items.length) return;
  const target = e.currentTarget instanceof HTMLElement ? e.currentTarget : (e.target as HTMLElement | null);
  if (e.type === "click" && target) {
    menuAt(target, items, { fromKeyboard: e.detail === 0 });
    return;
  }
  // Shift+F10 sends a synthetic event with detail -1 (see keymap.ts).
  const fromKeyboard = (e as MouseEvent).detail === -1;
  set({ x: e.clientX, y: e.clientY, items, opener: (document.activeElement as HTMLElement | null) ?? target, fromKeyboard });
}

/**
 * Opens a menu hanging from a button: its top-left 2 px under the button's bottom-left,
 * wherever the pointer was (a synthetic click at 0,0 included). Focus goes back to the
 * button when it closes. `fromKeyboard` highlights the first item, as Enter or Space would.
 */
export function menuAt(anchor: HTMLElement, items: MenuItem[], opts: { fromKeyboard?: boolean } = {}) {
  if (!items.length) return;
  const r = anchor.getBoundingClientRect();
  set({
    x: r.left, y: r.bottom + 2, items, opener: anchor, fromKeyboard: !!opts.fromKeyboard,
    anchor: { left: r.left, right: r.right, top: r.top, bottom: r.bottom },
  });
}

/**
 * The click handler for a menu button: opens `items()` under the button (menuAt), from the
 * keyboard when the click came from Enter or Space (detail 0).
 */
export function menuButton(items: () => MenuItem[]) {
  return (e: ReactMouseEvent<HTMLElement>) => {
    e.preventDefault();
    e.stopPropagation();
    menuAt(e.currentTarget, items(), { fromKeyboard: e.detail === 0 });
  };
}

export function closeContextMenu(restoreFocus = true) {
  const opener = current?.opener;
  set(null);
  if (restoreFocus && opener?.isConnected) opener.focus({ preventScroll: true });
}

export function useContextMenu(): OpenMenu | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}

export const isSeparator = (i: MenuItem): i is { separator: true } => "separator" in i;
export const hasSubmenu = (i: MenuItem): i is Extract<MenuItem, { submenu: MenuItem[] }> => "submenu" in i;
export const isDisabled = (i: MenuItem) => !isSeparator(i) && !!i.disabled;
export const disabledReason = (i: MenuItem) => (!isSeparator(i) && typeof i.disabled === "string" ? i.disabled : null);

/**
 * Keeps WebKit's own menu (Reload, Inspect Element…) out of the app, except in
 * text fields and terminals where Cut/Copy/Paste belong. Holding ⌥ in a dev build
 * still gets it, for the inspector.
 */
export function suppressNativeMenus(): () => void {
  const onMenu = (e: MouseEvent) => {
    if (e.defaultPrevented) return;
    const t = e.target as HTMLElement | null;
    if (t?.closest("input, textarea, [contenteditable=''], [contenteditable='true'], .xterm, .cm-editor")) return;
    if (import.meta.env.DEV && e.altKey) return;
    e.preventDefault();
  };
  document.addEventListener("contextmenu", onMenu);
  return () => document.removeEventListener("contextmenu", onMenu);
}

/** Shift+F10: opens the focused element's context menu, as if right-clicked at its corner. */
export function openMenuForFocused(): boolean {
  const el = document.activeElement as HTMLElement | null;
  if (!el || el === document.body) return false;
  const r = el.getBoundingClientRect();
  const ev = new MouseEvent("contextmenu", {
    bubbles: true, cancelable: true, clientX: Math.round(r.left + Math.min(24, r.width / 2)), clientY: Math.round(r.bottom - 2), detail: -1,
  });
  el.dispatchEvent(ev);
  return ev.defaultPrevented;
}

// ---- one node menu (§C4): the sidebar, the Dashboard, Topology and the status bar use it

/** A node action another package names for its current state ("Trust host key…"), when it has one. */
export type NodeAction = { label: string; run: () => unknown } | null;
let nodeActionOf: (n: NodeSnapshot) => NodeAction = () => null;

/** Installs the source of a node's state action (nodeStatus.ts' nodeActionFor). */
export function setNodeActionSource(f: (n: NodeSnapshot) => NodeAction) {
  nodeActionOf = f;
}

function copyText(text: string) {
  navigator.clipboard?.writeText(text).catch(() => {});
}

/** The node's LAN address as people type it (the LAN interface's IP, else the host it's reached by). */
export function nodeAddress(n: NodeSnapshot): string {
  return n.nets.find((x) => x.role === "lan" && x.ip)?.ip ?? n.host;
}

/**
 * Every surface's right-click menu for a node, the same items in the same order: the state's
 * own action first when there is one (Trust host key…), then Open, Terminal, Files,
 * Containers and Services, the power verbs (Wake only while it's off), and the copies. The
 * windows, the terminal and power are loaded when an item runs, so this stays cheap to import.
 */
export function nodeMenu(n: NodeSnapshot): MenuItem[] {
  const open = (kind: string, params: Record<string, string> = {}) => () => import("../wm/WindowManager").then((m) => m.openWindow(kind, params));
  const notConnected = n.status !== "online" ? `${n.name} isn't connected` : false;
  const ip = nodeAddress(n);
  const action = nodeActionOf(n);
  return [
    ...(action ? [{ label: action.label, run: action.run }, { separator: true } as const] : []),
    { label: "Open", icon: Server, run: open("node", { id: n.id }) },
    {
      label: "Terminal", icon: SquareTerminal, disabled: notConnected,
      run: () => import("./terminals").then((t) => {
        t.openTerminal(n.id);
        return open("terminal")();
      }),
    },
    { label: "Files", icon: FolderOpen, run: open("files", { place: n.id, path: "~" }) },
    { separator: true },
    { label: "Containers", icon: LayoutList, run: open("containers", { node: n.id }) },
    { label: "Services", icon: LayoutList, run: open("services", { node: n.id }) },
    { separator: true },
    { label: POWER.restart, icon: RotateCcw, disabled: notConnected, run: () => import("./power").then((p) => p.requestPower(n.id, "reboot")) },
    { label: POWER.shutdown, icon: Power, disabled: notConnected, run: () => import("./power").then((p) => p.requestPower(n.id, "poweroff")) },
    ...(n.status !== "online" ? [{ label: POWER.wake, icon: Sunrise, run: () => import("./power").then((p) => p.requestPower(n.id, "wake")) }] : []),
    { separator: true },
    { label: `Copy IP address (${ip})`, icon: Copy, run: () => copyText(ip) },
    { label: "Copy hostname", icon: Copy, run: () => copyText(n.host) },
  ];
}
