// Appearance: theme (System / Light / Dark / Black), text size, reduced motion
// and increased contrast. The choice lives in localStorage['sparkbox.appearance'];
// index.html applies it before the first paint and this module keeps <html>'s
// data-theme / data-contrast / data-motion and --text-scale in step with it.
// Changing the theme also sets the native window's appearance, so the macOS
// title bar and menus follow. Black (true black, for an OLED screen or a dark
// room) is never chosen by System; to everything outside styles.css it is a dark
// theme, and the native window gets the dark look.
import { useSyncExternalStore } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type ThemePref = "system" | "light" | "dark" | "black";
export type Effective = "light" | "dark" | "black";

export interface Appearance {
  theme: ThemePref;
  /** Multiplies every font-size token: 0.92, 1, 1.12 or 1.25. */
  textScale: number;
  motion: "system" | "reduce";
  contrast: "system" | "more";
}

export const TEXT_SCALES: { value: number; label: string }[] = [
  { value: 0.92, label: "Small" },
  { value: 1, label: "Default" },
  { value: 1.12, label: "Large" },
  { value: 1.25, label: "Larger" },
];

const KEY = "sparkbox.appearance";
const DEFAULTS: Appearance = { theme: "system", textScale: 1, motion: "system", contrast: "system" };
const THEME_EVENT = "sparkbox:theme";

function load(): Appearance {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<Appearance>;
    return {
      theme: raw.theme === "light" || raw.theme === "dark" || raw.theme === "black" ? raw.theme : "system",
      textScale: TEXT_SCALES.some((s) => s.value === raw.textScale) ? raw.textScale! : 1,
      motion: raw.motion === "reduce" ? "reduce" : "system",
      contrast: raw.contrast === "more" ? "more" : "system",
    };
  } catch {
    return { ...DEFAULTS };
  }
}

const darkQuery: MediaQueryList | null = typeof window !== "undefined" && window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;

let appearance = load();
let themeSnap = computeTheme();
const listeners = new Set<() => void>();

function computeTheme(): { pref: ThemePref; effective: Effective } {
  const effective: Effective = appearance.theme === "system" ? (darkQuery?.matches ? "dark" : "light") : appearance.theme;
  return { pref: appearance.theme, effective };
}

function applyToDocument(a: Appearance) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  const attr = (name: string, value: string | null) => (value ? root.setAttribute(name, value) : root.removeAttribute(name));
  attr("data-theme", a.theme === "system" ? null : a.theme);
  attr("data-contrast", a.contrast === "more" ? "more" : null);
  attr("data-motion", a.motion === "reduce" ? "reduce" : null);
  if (a.textScale === 1) root.style.removeProperty("--text-scale");
  else root.style.setProperty("--text-scale", String(a.textScale));
}

/** Points the native window (title bar, menus, scroll bars) at the same theme; Black gets dark. */
function applyToWindow(pref: ThemePref) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
  try {
    getCurrentWindow().setTheme(pref === "system" ? null : pref === "black" ? "dark" : pref).catch(() => {
      // A build without the set-theme permission keeps the system look.
    });
  } catch {
    // No native window (tests, the browser preview).
  }
}

function notify(themeChanged: boolean) {
  const next = computeTheme();
  const effectiveChanged = next.effective !== themeSnap.effective || next.pref !== themeSnap.pref;
  if (effectiveChanged) themeSnap = next;
  listeners.forEach((l) => l());
  if (themeChanged || effectiveChanged) window.dispatchEvent(new CustomEvent(THEME_EVENT, { detail: themeSnap }));
}

darkQuery?.addEventListener?.("change", () => {
  if (appearance.theme === "system") notify(true);
});

applyToDocument(appearance);
if (appearance.theme !== "system") applyToWindow(appearance.theme);

function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function getAppearance(): Appearance {
  return appearance;
}

export function setAppearance(patch: Partial<Appearance>) {
  const next = { ...appearance, ...patch };
  const themeChanged = next.theme !== appearance.theme;
  appearance = next;
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Not persisted (private storage); it still applies for this session.
  }
  applyToDocument(next);
  if (themeChanged) applyToWindow(next.theme);
  notify(themeChanged);
}

export function useAppearance(): Appearance {
  return useSyncExternalStore(subscribe, getAppearance);
}

export function setThemePref(pref: ThemePref) {
  setAppearance({ theme: pref });
}

export function useTheme(): { pref: ThemePref; effective: Effective } {
  return useSyncExternalStore(subscribe, () => themeSnap);
}

/** Whether an effective theme takes the dark look: Dark and Black both do. One rule, so hooks and listeners agree. */
export function isDarkTheme(effective: Effective): boolean {
  return effective !== "light";
}

/** True while the effective theme is dark (System following macOS, Dark or Black chosen). */
export function useDarkMode(): boolean {
  return isDarkTheme(useTheme().effective);
}

/** Runs `cb` whenever the effective theme changes; canvas charts redraw with it. */
export function onThemeChange(cb: (t: { pref: ThemePref; effective: Effective }) => void): () => void {
  const h = (e: Event) => cb((e as CustomEvent<{ pref: ThemePref; effective: Effective }>).detail);
  window.addEventListener(THEME_EVENT, h);
  return () => window.removeEventListener(THEME_EVENT, h);
}

/** Reads a CSS custom property, for canvas drawing (uPlot) that can't use var(). */
export function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

export type MetricKey =
  | "cpu" | "gpu" | "mem" | "mem-gpu" | "mem-cache" | "power" | "temp"
  | "rx" | "tx" | "read" | "write" | "rdma" | "tcp";

/** The resolved colour of a metric (--m-*), for canvas. In CSS use `var(--m-<key>)`. */
export function metricColor(key: MetricKey): string {
  return cssVar(`--m-${key}`);
}

/** Node identity colour by position in the inventory (0-based), resolved for canvas. */
export function nodeColor(index: number): string {
  return cssVar(nodeVarName(index));
}

/** `--node-1` … `--node-6`, cycling after six. */
export function nodeVarName(index: number): string {
  return `--node-${(((index % 6) + 6) % 6) + 1}`;
}

/** Sequential blue ramp (light → dark) for magnitude, e.g. per-core CPU load. */
export const SEQ_BLUE = [
  "#cde2fb", "#b7d3f6", "#9ec5f4", "#86b6ef", "#6da7ec", "#5598e7", "#3987e5",
  "#2a78d6", "#256abf", "#1c5cab", "#184f95", "#104281", "#0d366b",
];

/** The same ramp anchored on true black for the Black theme (dark → light, not flipped): 0% is a
 *  near-surface step, so an idle core or an empty cell recedes instead of lighting up navy. */
export const SEQ_BLUE_BLACK = [
  "#122237", "#132f52", "#113c6f", "#0f4a8c", "#0858aa", "#1768c1", "#2478d8",
  "#3c89e6", "#5a9bed", "#75adf4", "#91bef8", "#aecffa", "#cbe0fc",
];

function luminance(hex: string): number {
  const n = parseInt(hex.slice(1), 16);
  const lin = (v: number) => {
    const c = v / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
}

/**
 * Maps 0..100 onto the ramp. In dark mode near-zero recedes toward the dark
 * surface; under Black it recedes into true black on the black ramp. `dark`
 * defaults to the current effective theme. The text colour is whichever of
 * near-black and white reads better on the cell.
 */
export function seqColor(pct: number, dark: boolean = isDarkTheme(themeSnap.effective)): { bg: string; fg: string } {
  const black = dark && themeSnap.effective === "black";
  const ramp = black ? SEQ_BLUE_BLACK : SEQ_BLUE;
  const steps = ramp.length;
  let i = Math.round((Math.max(0, Math.min(100, pct)) / 100) * (steps - 1));
  if (dark && !black) i = steps - 1 - i;
  const bg = ramp[i];
  const l = luminance(bg);
  return { bg, fg: (1.05 / (l + 0.05)) >= ((l + 0.05) / 0.0575) ? "#ffffff" : "#13161b" };
}
