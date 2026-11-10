// Runs before every test file (vite.config.ts › test.setupFiles).
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";
import { mockTauri } from "./tauri";

// jsdom has no layout engine: give components the browser APIs they expect.
if (!window.matchMedia) {
  window.matchMedia = (query: string): MediaQueryList => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  });
}

class ResizeObserverStub implements ResizeObserver {
  constructor(private cb: ResizeObserverCallback) {}
  observe(target: Element) {
    // Report the element's (test-assigned) width once, like a first layout pass.
    const width = (target as HTMLElement).clientWidth || 1200;
    const entry = { target, contentRect: { width, height: 400 } } as unknown as ResizeObserverEntry;
    queueMicrotask(() => this.cb([entry], this));
  }
  unobserve() {}
  disconnect() {}
}
if (typeof window.ResizeObserver === "undefined") window.ResizeObserver = ResizeObserverStub;

if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => {};
// CodeMirror measures a selected script line on an animation frame. jsdom implements
// element rectangles but omits Range geometry; use the same empty layout for ranges.
if (!Range.prototype.getClientRects) Range.prototype.getClientRects = () => document.createElement("span").getClientRects();
if (!Range.prototype.getBoundingClientRect) Range.prototype.getBoundingClientRect = () => new DOMRect();

// A Tauri backend exists in every test (so `inTauri` is true); tests replace
// its handlers with mockTauri({...}).
mockTauri({});

afterEach(() => {
  cleanup();
  try {
    localStorage.clear();
    sessionStorage.clear();
  } catch {
    // Storage can be unavailable; nothing to reset then.
  }
  for (const a of ["data-theme", "data-contrast", "data-motion", "style"]) document.documentElement.removeAttribute(a);
  mockTauri({});
});
