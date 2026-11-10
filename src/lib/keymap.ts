// Keyboard shortcuts. In the app the native menu bar owns the ⌘ shortcuts, all but ⌘?
// (src-tauri/src/menu.rs), so they work even while a terminal has focus; this
// table mirrors it for Keyboard Shortcuts (⌘/) and drives the same shortcuts
// when the UI runs in a plain browser. A test checks the two stay in step.
// Unmodified keys are never taken from the focused control.
import { getCommand, runCommand } from "./commands";
import { openMenuForFocused } from "./contextMenu";
import { canUndo, focusOwnsUndo, runLatestUndo } from "./undo";

export interface Shortcut {
  /** Command id; the menu item has the same id. */
  id: string;
  /** Menu bar section it lives in. */
  menu: "SparkBox" | "File" | "Edit" | "View" | "Go" | "Cluster" | "Window" | "Help";
  label: string;
  /** Accelerator in the menu's syntax, e.g. "CmdOrCtrl+Shift+T". */
  accel: string;
}

export const SHORTCUTS: Shortcut[] = [
  { id: "app:settings", menu: "SparkBox", label: "Settings…", accel: "CmdOrCtrl+," },
  { id: "term:new-tab", menu: "File", label: "New Terminal Tab", accel: "CmdOrCtrl+T" },
  { id: "wm:close", menu: "File", label: "Close Window", accel: "CmdOrCtrl+W" },
  { id: "wm:close-all", menu: "File", label: "Close All Windows", accel: "CmdOrCtrl+Alt+W" },
  { id: "wm:reopen", menu: "File", label: "Reopen Closed Window", accel: "CmdOrCtrl+Shift+T" },
  { id: "wm:find", menu: "Edit", label: "Find…", accel: "CmdOrCtrl+F" },
  { id: "app:palette", menu: "View", label: "Command Palette", accel: "CmdOrCtrl+K" },
  { id: "app:sidebar", menu: "View", label: "Toggle Sidebar", accel: "Ctrl+CmdOrCtrl+S" },
  { id: "wm:cascade", menu: "View", label: "Cascade", accel: "CmdOrCtrl+Alt+C" },
  { id: "wm:tile", menu: "View", label: "Tile All", accel: "CmdOrCtrl+Alt+T" },
  { id: "wm:snap-left", menu: "View", label: "Snap Left", accel: "Ctrl+Alt+Left" },
  { id: "wm:snap-right", menu: "View", label: "Snap Right", accel: "Ctrl+Alt+Right" },
  { id: "wm:fill", menu: "View", label: "Fill Desktop", accel: "Ctrl+Alt+Up" },
  { id: "wm:restore", menu: "View", label: "Restore Size", accel: "Ctrl+Alt+Down" },
  { id: "wm:popout", menu: "View", label: "Open in Separate Window", accel: "CmdOrCtrl+Shift+O" },
  { id: "wm:refresh", menu: "View", label: "Refresh", accel: "CmdOrCtrl+R" },
  { id: "app:zoom-reset", menu: "View", label: "Actual Size", accel: "CmdOrCtrl+0" },
  { id: "app:zoom-in", menu: "View", label: "Zoom In", accel: "CmdOrCtrl+=" },
  { id: "app:zoom-out", menu: "View", label: "Zoom Out", accel: "CmdOrCtrl+-" },
  { id: "go:dashboard", menu: "Go", label: "Dashboard", accel: "CmdOrCtrl+1" },
  { id: "go:topology", menu: "Go", label: "Topology", accel: "CmdOrCtrl+2" },
  { id: "go:containers", menu: "Go", label: "Containers", accel: "CmdOrCtrl+3" },
  { id: "go:workloads", menu: "Go", label: "Workloads", accel: "CmdOrCtrl+4" },
  { id: "go:models", menu: "Go", label: "Models", accel: "CmdOrCtrl+5" },
  { id: "go:terminal", menu: "Go", label: "Terminal", accel: "CmdOrCtrl+6" },
  { id: "go:files", menu: "Go", label: "Files", accel: "CmdOrCtrl+7" },
  { id: "go:activity", menu: "Go", label: "Activity Log", accel: "CmdOrCtrl+8" },
  { id: "go:alerts", menu: "Go", label: "Alerts", accel: "CmdOrCtrl+9" },
  { id: "cluster:admin", menu: "Cluster", label: "Unlock Admin Mode…", accel: "CmdOrCtrl+Shift+L" },
  { id: "go:tasks", menu: "Cluster", label: "Tasks", accel: "CmdOrCtrl+Shift+J" },
  { id: "wm:minimize", menu: "Window", label: "Minimize Window", accel: "CmdOrCtrl+M" },
  { id: "app:minimize", menu: "Window", label: "Minimize SparkBox", accel: "CmdOrCtrl+Alt+M" },
  { id: "wm:next", menu: "Window", label: "Next Window", accel: "CmdOrCtrl+`" },
  { id: "wm:prev", menu: "Window", label: "Previous Window", accel: "CmdOrCtrl+Shift+`" },
  { id: "app:shortcuts", menu: "Help", label: "Keyboard Shortcuts", accel: "CmdOrCtrl+/" },
  { id: "help:open", menu: "Help", label: "SparkBox Help", accel: "CmdOrCtrl+Shift+/" },
];

/** Shortcuts handled in the page itself (not menu items). */
export const PAGE_SHORTCUTS: { label: string; keys: string; group: string }[] = [
  { group: "Windows", label: "Show all windows (overview)", keys: "F3" },
  { group: "Windows", label: "Undo closing a window, moving to Trash…", keys: "⌘Z" },
  { group: "Windows", label: "Drag to an edge to snap; hold ⌥ to drop freely", keys: "⌥ drag" },
  { group: "Lists", label: "Open the selected row (Inspect, status, details)", keys: "Return" },
  { group: "Lists", label: "Logs of the selected container", keys: "⌘L" },
  { group: "Lists", label: "Shell in the selected container", keys: "⌘T" },
  { group: "Lists", label: "Inspect the selected container", keys: "⌘I" },
  { group: "Menus", label: "Open the context menu of the focused item", keys: "⇧F10" },
  { group: "Menus", label: "Close a menu, dialog or the palette", keys: "Esc" },
  { group: "Command palette", label: "Move between results", keys: "↑ ↓" },
  { group: "Command palette", label: "Run the selected command", keys: "Return" },
  { group: "Help", label: "Search Help", keys: "⌘F" },
  { group: "Help", label: "Back, Forward", keys: "⌘[ ⌘]" },
];

const KEY_GLYPH: Record<string, string> = {
  Left: "←", Right: "→", Up: "↑", Down: "↓", "`": "`", ",": ",", "/": "/", "=": "+", "-": "−",
};

/** "CmdOrCtrl+Shift+T" → "⇧⌘T", in the Mac's modifier order (⌃⌥⇧⌘). */
export function displayAccel(accel: string): string {
  const parts = accel.split("+");
  // "CmdOrCtrl+=" splits into ["CmdOrCtrl", "="]; "CmdOrCtrl+-" fine too.
  const key = parts[parts.length - 1] || "+";
  const mods = new Set(parts.slice(0, -1).map((m) => m.toLowerCase()));
  // ⇧/ is the key the Mac calls ⌘?.
  if (key === "/" && mods.has("shift") && mods.has("cmdorctrl") && mods.size === 2) return "⌘?";
  let out = "";
  if (mods.has("ctrl") || mods.has("control")) out += "⌃";
  if (mods.has("alt") || mods.has("option")) out += "⌥";
  if (mods.has("shift")) out += "⇧";
  if (mods.has("cmdorctrl") || mods.has("cmd") || mods.has("command") || mods.has("super")) out += "⌘";
  return out + (KEY_GLYPH[key] ?? key.toUpperCase());
}

export function shortcutFor(id: string): string | undefined {
  const s = SHORTCUTS.find((x) => x.id === id);
  return s ? displayAccel(s.accel) : undefined;
}

const CODE_FOR: Record<string, string> = {
  "`": "Backquote", ",": "Comma", "/": "Slash", "=": "Equal", "-": "Minus",
  Left: "ArrowLeft", Right: "ArrowRight", Up: "ArrowUp", Down: "ArrowDown",
};

function codeFor(key: string): string {
  if (CODE_FOR[key]) return CODE_FOR[key];
  if (/^[0-9]$/.test(key)) return `Digit${key}`;
  if (/^[A-Z]$/i.test(key)) return `Key${key.toUpperCase()}`;
  return key;
}

/** Does a key press match an accelerator? Compares physical keys, so it works on any layout. */
export function matchesAccel(e: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">, accel: string): boolean {
  const parts = accel.split("+");
  const key = parts[parts.length - 1] || "=";
  const mods = parts.slice(0, -1).map((m) => m.toLowerCase());
  const want = {
    meta: mods.some((m) => ["cmdorctrl", "cmd", "command", "super"].includes(m)),
    ctrl: mods.some((m) => ["ctrl", "control"].includes(m)),
    alt: mods.some((m) => ["alt", "option"].includes(m)),
    shift: mods.includes("shift"),
  };
  return e.code === codeFor(key) && e.metaKey === want.meta && e.ctrlKey === want.ctrl && e.altKey === want.alt && e.shiftKey === want.shift;
}

const HELP_ACCEL = "CmdOrCtrl+Shift+/";

/**
 * Listens for the keys the page handles itself. In a plain browser (no native
 * menu) it also runs the menu shortcuts. Returns a function that stops it.
 */
export function installKeymap(opts: { nativeMenu: boolean; onOverview?: () => void }): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (e.defaultPrevented || e.isComposing) return;
    const inTerminal = !!(document.activeElement as HTMLElement | null)?.closest?.(".xterm");

    // ⌘Z outside text fields and terminals undoes the last app action (closing a window…).
    if (e.code === "KeyZ" && e.metaKey && !e.shiftKey && !e.altKey && !e.ctrlKey) {
      if (!focusOwnsUndo() && canUndo()) {
        e.preventDefault();
        void runLatestUndo();
      }
      return;
    }
    // ⇧F10 (or the context menu key) opens the focused element's menu.
    if ((e.key === "F10" && e.shiftKey) || e.key === "ContextMenu") {
      if (openMenuForFocused()) e.preventDefault();
      return;
    }
    if (e.key === "F3" && !e.metaKey && !e.ctrlKey && !e.altKey && !inTerminal && opts.onOverview) {
      e.preventDefault();
      opts.onOverview();
      return;
    }
    // ⌘? can't reach the menu item: Shift-/ types "?", AppKit compares that with the item's "/",
    // and Tauri can't give an item the key "?". So the page opens Help, with or without a menu.
    if (matchesAccel(e, HELP_ACCEL) && getCommand("help:open")) {
      e.preventDefault();
      void runCommand("help:open");
      return;
    }
    if (opts.nativeMenu) return;
    // Browser preview: the menu bar's shortcuts, minus ones a text field needs.
    if (!e.metaKey && !e.ctrlKey && !e.altKey) return;
    for (const s of SHORTCUTS) {
      if (!matchesAccel(e, s.accel) || !getCommand(s.id)) continue;
      e.preventDefault();
      void runCommand(s.id);
      return;
    }
  };
  window.addEventListener("keydown", onKey);
  return () => window.removeEventListener("keydown", onKey);
}
