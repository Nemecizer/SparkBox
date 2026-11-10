import { describe, expect, it, vi } from "vitest";
import menuRs from "../../src-tauri/src/menu.rs?raw";
import { registerCommands } from "./commands";
import { displayAccel, installKeymap, matchesAccel, SHORTCUTS } from "./keymap";
import { MENU_COMMAND_IDS } from "./menuBridge";

// The native menu (src-tauri/src/menu.rs) owns the shortcuts in the app; this table mirrors it.
const ITEMS = [...menuRs.matchAll(/^\s*\("([a-z-]+:[a-z-]+)", "([^"]+)", (?:Some\("([^"]+)"\)|None)\),$/gm)]
  .map((m) => ({ id: m[1], label: m[2], accel: m[3] ?? null }));

describe("the menu bar and the keymap agree", () => {
  it("parses the menu's item table", () => {
    expect(ITEMS.length).toBeGreaterThan(30);
  });

  it("has every shortcut with the same keys", () => {
    for (const s of SHORTCUTS) {
      const item = ITEMS.find((i) => i.id === s.id);
      expect(item, s.id).toBeDefined();
      expect(item!.accel, s.id).toBe(s.accel);
    }
    for (const i of ITEMS.filter((x) => x.accel)) {
      expect(SHORTCUTS.some((s) => s.id === i.id), `${i.id} has a shortcut in menu.rs but not in keymap.ts`).toBe(true);
    }
  });

  it("routes every menu item to a command id the bridge listens for", () => {
    for (const i of ITEMS) expect(MENU_COMMAND_IDS, i.id).toContain(i.id);
  });
});

describe("shortcuts", () => {
  it("display in the Mac's modifier order", () => {
    expect(displayAccel("CmdOrCtrl+Shift+T")).toBe("⇧⌘T");
    expect(displayAccel("Ctrl+CmdOrCtrl+S")).toBe("⌃⌘S");
    expect(displayAccel("Ctrl+Alt+Left")).toBe("⌃⌥←");
    expect(displayAccel("CmdOrCtrl+Alt+W")).toBe("⌥⌘W");
    expect(displayAccel("CmdOrCtrl+`")).toBe("⌘`");
    expect(displayAccel("CmdOrCtrl+=")).toBe("⌘+");
    expect(displayAccel("CmdOrCtrl+Shift+/")).toBe("⌘?");
    expect(displayAccel("CmdOrCtrl+/")).toBe("⌘/");
  });

  it("match physical keys exactly, modifiers included", () => {
    const key = (code: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}) =>
      ({ code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods });
    expect(matchesAccel(key("KeyK", { metaKey: true }), "CmdOrCtrl+K")).toBe(true);
    expect(matchesAccel(key("KeyK"), "CmdOrCtrl+K")).toBe(false);
    expect(matchesAccel(key("KeyT", { metaKey: true, shiftKey: true }), "CmdOrCtrl+T")).toBe(false);
    expect(matchesAccel(key("Backquote", { metaKey: true, shiftKey: true }), "CmdOrCtrl+Shift+`")).toBe(true);
    expect(matchesAccel(key("ArrowLeft", { ctrlKey: true, altKey: true }), "Ctrl+Alt+Left")).toBe(true);
    expect(matchesAccel(key("Digit1", { metaKey: true }), "CmdOrCtrl+1")).toBe(true);
  });

  it("are unique", () => {
    const accels = SHORTCUTS.map((s) => s.accel);
    expect(new Set(accels).size).toBe(accels.length);
  });

  // AppKit never matches ⌘? to the menu item: Shift-/ types "?" and the item's key is "/".
  it("open Help on ⌘? from the page, even with the native menu", async () => {
    const run = vi.fn();
    const unregister = registerCommands([{ id: "help:open", title: "SparkBox Help", group: "App", run }]);
    const stop = installKeymap({ nativeMenu: true });
    const press = (init: KeyboardEventInit) => window.dispatchEvent(new KeyboardEvent("keydown", { cancelable: true, ...init }));
    press({ code: "Slash", key: "/", metaKey: true });
    expect(press({ code: "Slash", key: "?", metaKey: true, shiftKey: true })).toBe(false);
    await vi.waitFor(() => expect(run).toHaveBeenCalledTimes(1));
    stop();
    unregister();
  });
});
