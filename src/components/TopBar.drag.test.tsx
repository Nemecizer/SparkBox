import { describe, expect, it } from "vitest";
import shellCss from "./shell.css?raw";

// isClickableElement and isDragRegion, verbatim from Tauri 2.12.5 (src/window/scripts/drag.js):
// what decides whether a mousedown in the window starts a native drag of the window.
const TAURI_DRAG_REGION_ATTR = "data-tauri-drag-region";
const CLICKABLE_TAGS = new Set(["A", "BUTTON", "INPUT", "SELECT", "TEXTAREA", "LABEL", "SUMMARY"]);
const INTERACTIVE_ROLES = new Set(["button", "link", "menuitem", "tab", "checkbox", "radio", "switch", "option"]);

function isClickableElement(el: HTMLElement) {
  return (
    CLICKABLE_TAGS.has(el.tagName)
    || (el.hasAttribute("contenteditable")
      && el.getAttribute("contenteditable") !== "false")
    || (el.hasAttribute("tabindex") && el.getAttribute("tabindex") !== "-1")
    || INTERACTIVE_ROLES.has(el.getAttribute("role")!)
  );
}

function isDragRegion(composedPath: EventTarget[]) {
  for (const el of composedPath) {
    if (!(el instanceof HTMLElement)) continue;

    const attr = el.getAttribute(TAURI_DRAG_REGION_ATTR);

    if (isClickableElement(el) && attr === null) return false;
    if (attr === null) continue;
    if (attr === "false") return false;
    if (attr === "deep") return true;
    if (attr === "" || attr === "true") return el === composedPath[0];
  }

  return false;
}

/** The composed path of a mousedown on `el`: it, its ancestors, the document and the window. */
function pathOf(el: Element): EventTarget[] {
  const path: EventTarget[] = [];
  for (let n: Node | null = el; n; n = n.parentNode) path.push(n);
  path.push(window);
  return path;
}

/** The bar's controls, by tag: what a click there should do is the control's action. */
const CONTROLS = "button, a, input, select, textarea";

/** Renders the bar, then checks it with `check` (at `width`, if given, the bar folds all the way). */
async function withBar(check: (bar: HTMLElement) => void, width?: number) {
  const { act, render } = await import("@testing-library/react");
  const { baseSnapshot } = await import("../dev/mockBackend");
  const { resetStore, startClusterStore } = await import("../lib/store");
  const { applyWatchState } = await import("../lib/agent");
  const { mockTauri } = await import("../test/tauri");
  const { WindowManagerProvider } = await import("../wm/WindowManager");
  const { TopBar } = await import("./TopBar");
  resetStore();
  mockTauri({ get_snapshot: () => structuredClone(baseSnapshot) });
  await startClusterStore();
  // A watch badge with an action: the most the bar carries.
  act(() => applyWatchState({ mode: "mac-only", agents: [], deliveryOwner: "mac", channels: [] } as never));
  const cw = Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth")!;
  const sw = Object.getOwnPropertyDescriptor(Element.prototype, "scrollWidth")!;
  if (width) {
    // jsdom has no layout: a bar that never fits folds all the way (TopBar's useTopBarFit).
    Object.defineProperty(Element.prototype, "clientWidth", { configurable: true, get() { return this.classList.contains("topbar") ? width : 0; } });
    Object.defineProperty(Element.prototype, "scrollWidth", { configurable: true, get() { return this.classList.contains("topbar") ? width + 100 : 0; } });
  }
  try {
    await act(async () => {
      render(<WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}><TopBar /></WindowManagerProvider>);
    });
    check(document.querySelector<HTMLElement>("header.topbar")!);
  } finally {
    Object.defineProperty(Element.prototype, "clientWidth", cw);
    Object.defineProperty(Element.prototype, "scrollWidth", sw);
  }
}

function checkDrag(bar: HTMLElement) {
  const els = [bar, ...bar.querySelectorAll("*")];
  const controls = els.filter((el) => el.matches(CONTROLS));
  expect(controls.length).toBeGreaterThan(3);
  for (const el of els) {
    const insideControl = !!el.closest(CONTROLS);
    // Everything that isn't a control (or inside one) drags the window; controls never do.
    expect(isDragRegion(pathOf(el)), `${el.tagName.toLowerCase()}.${el.getAttribute("class") ?? ""}`).toBe(!insideControl);
  }
}

describe("top bar drag region (P1.0)", () => {
  it("drags the window from every part of the bar that isn't a button", async () => {
    await withBar((bar) => {
      expect(bar.getAttribute("data-tauri-drag-region")).toBe("deep");
      // The empty strip exists, straight under the bar, and drags.
      const strip = bar.querySelector<HTMLElement>(":scope > .topbar-drag")!;
      expect(strip).not.toBeNull();
      expect(isDragRegion(pathOf(strip))).toBe(true);
      checkDrag(bar);
    });
  });

  it("still drags with everything folded into the … menu", async () => {
    await withBar((bar) => {
      expect(bar.getAttribute("data-fit")).toBe("4");
      expect(bar.querySelector(".tb-fold")).not.toBeNull();
      expect(bar.querySelector(":scope > .topbar-drag")).not.toBeNull();
      checkDrag(bar);
    }, 560);
  });

  it("keeps at least 48 px of empty strip, so a 960 px window has space to drag", async () => {
    const rule = shellCss.match(/(^|\n)\.topbar-drag\s*\{([^}]*)\}/);
    expect(rule).not.toBeNull();
    const min = rule![2].match(/min-width:\s*(\d+)px/);
    expect(Number(min?.[1])).toBeGreaterThanOrEqual(48);
    expect(rule![2]).toMatch(/flex:\s*1\b/);
    // Nothing turns the drag region off inside the bar, or hides the strip from the mouse.
    expect(shellCss).not.toMatch(/\.topbar[^{]*\{[^}]*pointer-events:\s*none/);
    await withBar((bar) => expect(bar.querySelector("[data-tauri-drag-region='false']")).toBeNull());
  });
});
