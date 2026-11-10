import { describe, expect, it, vi } from "vitest";
import { getAppearance, isDarkTheme, onThemeChange, seqColor, setAppearance, setThemePref } from "../lib/theme";
import { mockCalls, mockTauri } from "./tauri";

describe("appearance", () => {
  it("sets the document attributes, saves the choice and tells the native window", async () => {
    mockTauri({});
    const seen = vi.fn();
    const off = onThemeChange(seen);
    setThemePref("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(JSON.parse(localStorage.getItem("sparkbox.appearance")!).theme).toBe("dark");
    expect(seen).toHaveBeenCalledWith({ pref: "dark", effective: "dark" });
    await Promise.resolve();
    expect(mockCalls.find((c) => c.cmd === "plugin:window|set_theme")?.args).toMatchObject({ value: "dark" });

    setAppearance({ theme: "system", textScale: 1.12, contrast: "more", motion: "reduce" });
    const root = document.documentElement;
    expect(root.hasAttribute("data-theme")).toBe(false);
    expect(root.getAttribute("data-contrast")).toBe("more");
    expect(root.getAttribute("data-motion")).toBe("reduce");
    expect(root.style.getPropertyValue("--text-scale")).toBe("1.12");
    expect(seen).toHaveBeenLastCalledWith({ pref: "system", effective: "light" });
    off();

    setAppearance({ textScale: 1, contrast: "system", motion: "system" });
    expect(root.style.getPropertyValue("--text-scale")).toBe("");
    expect(getAppearance()).toEqual({ theme: "system", textScale: 1, motion: "system", contrast: "system" });
  });

  it("treats Black as its own theme in the document and as dark for the native window", async () => {
    mockTauri({});
    const seen = vi.fn();
    const off = onThemeChange(seen);
    setThemePref("black");
    const root = document.documentElement;
    expect(root.getAttribute("data-theme")).toBe("black");
    expect(JSON.parse(localStorage.getItem("sparkbox.appearance")!).theme).toBe("black");
    expect(seen).toHaveBeenCalledWith({ pref: "black", effective: "black" });
    // A listener (the file editor's CodeMirror theme) sees Black as dark, the same as useDarkMode.
    expect(isDarkTheme(seen.mock.calls.at(-1)![0].effective)).toBe(true);
    expect(isDarkTheme("dark")).toBe(true);
    expect(isDarkTheme("light")).toBe(false);
    await Promise.resolve();
    // Tauri has no black: the title bar and menus take the dark look.
    expect(mockCalls.filter((c) => c.cmd === "plugin:window|set_theme").at(-1)?.args).toMatchObject({ value: "dark" });
    // The heat ramp is anchored on true black, not flipped: 0% recedes into the surface.
    expect(seqColor(0)).toEqual({ bg: "#122237", fg: "#ffffff" });
    expect(seqColor(100)).toEqual({ bg: "#cbe0fc", fg: "#13161b" });
    off();
    setThemePref("system");
    expect(root.hasAttribute("data-theme")).toBe(false);
  });

  it("picks readable text on the heat ramp in both themes", () => {
    expect(seqColor(0, false)).toEqual({ bg: "#cde2fb", fg: "#13161b" });
    expect(seqColor(100, false).fg).toBe("#ffffff");
    expect(seqColor(0, true)).toEqual({ bg: "#0d366b", fg: "#ffffff" });
  });
});
