import { describe, expect, it } from "vitest";
import { getModality, installModality, isKeyboardNavigation } from "./modality";

describe("input modality", () => {
  it("starts as pointer, turns keyboard on Tab or arrows, and back on a click", () => {
    const stop = installModality();
    const root = document.documentElement;
    expect(root.dataset.modality).toBe("pointer");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    expect(getModality()).toBe("pointer");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab" }));
    expect(getModality()).toBe("keyboard");
    window.dispatchEvent(new Event("pointerdown"));
    expect(getModality()).toBe("pointer");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown" }));
    expect(getModality()).toBe("keyboard");
    stop();
    expect(root.dataset.modality).toBeUndefined();
  });

  it("doesn't count shortcuts or typing as keyboard navigation", () => {
    expect(isKeyboardNavigation({ key: "k", metaKey: true, ctrlKey: false, altKey: false })).toBe(false);
    expect(isKeyboardNavigation({ key: "Tab", metaKey: true, ctrlKey: false, altKey: false })).toBe(false);
    expect(isKeyboardNavigation({ key: "x", metaKey: false, ctrlKey: false, altKey: false })).toBe(false);
    expect(isKeyboardNavigation({ key: "Enter", metaKey: false, ctrlKey: false, altKey: false })).toBe(true);
  });
});
