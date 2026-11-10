// Connects the native menu bar (src-tauri/src/menu.rs) to the command layer.
// Every custom menu item emits `menu://<command id>`; items built from data
// (the Window list, Go › Nodes, Demo Scenarios) emit one event each with the
// item's target as payload. In the other direction, the menu is told which inner
// windows are open and the state its labels and checkmarks show.
import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri } from "./api";
import { getCommand, runCommand, unavailableReason } from "./commands";
import { SHORTCUTS } from "./keymap";

/** Menu items without a shortcut that also run commands. */
export const MENU_ONLY_IDS = [
  "app:welcome", "app:doctor", "app:activity", "app:controls-style", "wm:front", "wm:overview",
  "cluster:source-demo", "cluster:source-live", "wm:save-layout", "wm:tile-two", "wm:tile-four", "wm:restore-layout",
  "files:open", "go:rdp", "go:howto",
];

export const MENU_COMMAND_IDS = [...SHORTCUTS.map((s) => s.id), ...MENU_ONLY_IDS];

export interface MenuHandlers {
  /** Help › an article: the id from docs/help/manifest.json's helpMenu. */
  openHelp: (id: string) => void;
  /** Help › All How-To Guides… and the like: a section id from the manifest's toc. */
  openHelpSection: (id: string) => void;
  focusWindow: (id: string) => void;
  openNode: (id: string) => void;
  setScenario: (id: string | null) => void;
  openLayout: (name: string) => void;
}

/** Starts routing menu clicks; returns a function that stops it. */
export function startMenuBridge(h: MenuHandlers): () => void {
  if (!inTauri) return () => {};
  const subs: Promise<UnlistenFn>[] = MENU_COMMAND_IDS.map((id) =>
    listen(`menu://${id}`, () => void runCommand(id)),
  );
  subs.push(listen<string>("menu://help", (e) => h.openHelp(e.payload)));
  subs.push(listen<string>("menu://help-section", (e) => h.openHelpSection(e.payload)));
  subs.push(listen<string>("menu://window", (e) => h.focusWindow(e.payload)));
  subs.push(listen<string>("menu://node", (e) => h.openNode(e.payload)));
  subs.push(listen<string | null>("menu://scenario", (e) => h.setScenario(e.payload || null)));
  subs.push(listen<string>("menu://layout", (e) => h.openLayout(e.payload)));
  return () => subs.forEach((p) => p.then((u) => u()).catch(() => {}));
}

export interface MenuWindow {
  id: string;
  title: string;
  active: boolean;
  minimized: boolean;
}

export interface MenuState {
  admin: boolean;
  demo: boolean;
  scenario: string | null;
  nodes: { id: string; name: string }[];
  scenarios: { id: string; title: string }[];
  layouts: string[];
  /** Menu ids whose command can't run right now (greyed out). */
  disabled: string[];
  /** Window controls style, for View › WinBox-Style Window Controls. */
  controls: "mac" | "winbox";
}

/** Ids of menu commands that are missing or unavailable now. */
export function disabledMenuIds(): string[] {
  return MENU_COMMAND_IDS.filter((id) => {
    const c = getCommand(id);
    return !c || unavailableReason(c) !== null;
  });
}

/** Keeps the menu's Window list in step (debounced 200 ms). */
export function useMenuWindows(windows: MenuWindow[]) {
  const key = JSON.stringify(windows);
  useEffect(() => {
    if (!inTauri) return;
    const t = window.setTimeout(() => {
      invoke("menu_set_windows", { windows }).catch(() => {});
    }, 200);
    return () => window.clearTimeout(t);
  }, [key]);
}

/** Keeps menu labels and checkmarks current (Admin, Data Source, scenarios, layouts). */
export function useMenuState(state: MenuState) {
  const key = JSON.stringify(state);
  const last = useRef("");
  useEffect(() => {
    if (!inTauri || key === last.current) return;
    const t = window.setTimeout(() => {
      last.current = key;
      invoke("menu_set_state", { state }).catch(() => {});
    }, 200);
    return () => window.clearTimeout(t);
  }, [key]);
}
