// Terminal settings (Settings › Terminal), kept in localStorage and applied to every open tab.
import { useSyncExternalStore } from "react";

export type TermThemeId = "follow" | "sparkbox-dark" | "sparkbox-light" | "solarized-dark" | "one-dark";
/** Which Option key acts as Meta (sends Esc+key); the other one keeps typing @ | { } [ ] on non-US layouts. */
export type OptionKey = "normal" | "left" | "right" | "both";
export type CursorStyle = "block" | "bar" | "underline";

export interface TermPrefs {
  theme: TermThemeId;
  font: string;
  fontSize: number;
  lineHeight: number;
  cursorStyle: CursorStyle;
  cursorBlink: boolean;
  scrollback: number;
  copyOnSelect: boolean;
  optionKey: OptionKey;
}

export const DEFAULT_PREFS: TermPrefs = {
  theme: "follow",
  font: "SF Mono",
  fontSize: 12,
  lineHeight: 1.15,
  cursorStyle: "block",
  cursorBlink: true,
  scrollback: 10_000,
  copyOnSelect: false,
  optionKey: "normal",
};

export const THEME_LABEL: Record<TermThemeId, string> = {
  follow: "Follow app appearance",
  "sparkbox-dark": "SparkBox Dark",
  "sparkbox-light": "SparkBox Light",
  "solarized-dark": "Solarized Dark",
  "one-dark": "One Dark",
};

const KEY = "sparkbox.terminal.prefs";
const FONT_SIZE = [9, 24] as const;

function load(): TermPrefs {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return sanitize({ ...DEFAULT_PREFS, ...(v && typeof v === "object" ? v : {}) });
  } catch {
    return DEFAULT_PREFS;
  }
}

function sanitize(p: TermPrefs): TermPrefs {
  return {
    ...p,
    fontSize: Math.max(FONT_SIZE[0], Math.min(FONT_SIZE[1], Math.round(Number(p.fontSize) || DEFAULT_PREFS.fontSize))),
    lineHeight: Math.max(1, Math.min(2, Number(p.lineHeight) || DEFAULT_PREFS.lineHeight)),
    scrollback: Math.max(1000, Math.min(200_000, Math.round(Number(p.scrollback) || DEFAULT_PREFS.scrollback))),
    theme: p.theme in THEME_LABEL ? p.theme : DEFAULT_PREFS.theme,
    optionKey: ["normal", "left", "right", "both"].includes(p.optionKey) ? p.optionKey : "normal",
    cursorStyle: ["block", "bar", "underline"].includes(p.cursorStyle) ? p.cursorStyle : "block",
  };
}

let prefs: TermPrefs = load();
const listeners = new Set<() => void>();

export function getTermPrefs(): TermPrefs {
  return prefs;
}

export function setTermPrefs(patch: Partial<TermPrefs>) {
  prefs = sanitize({ ...prefs, ...patch });
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    // Settings still apply for this session.
  }
  listeners.forEach((l) => l());
}

export function onTermPrefs(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function useTermPrefs(): TermPrefs {
  return useSyncExternalStore(onTermPrefs, () => prefs);
}

/** CSS font stacks: SF Mono is reached through ui-monospace in WebKit. */
export function fontStack(font: string): string {
  const fallbacks = `ui-monospace, Menlo, monospace`;
  return font === "SF Mono" ? `"SF Mono", ${fallbacks}` : `"${font}", ${fallbacks}`;
}

const CANDIDATES = ["SF Mono", "Menlo", "Monaco", "JetBrains Mono", "Fira Code", "Source Code Pro", "IBM Plex Mono", "Cascadia Code", "Hack", "Courier New"];
let installed: string[] | null = null;

/**
 * Monospace fonts present on this Mac. A font counts when text set in it measures differently
 * from both generic fallbacks (SF Mono is always offered: WebKit reaches it as ui-monospace).
 */
export function installedMonoFonts(): string[] {
  if (installed) return installed;
  const out = ["SF Mono"];
  try {
    const ctx = document.createElement("canvas").getContext("2d");
    if (!ctx) return out;
    const sample = "mmmmmmmmmmlli1|WW@#{}";
    const width = (font: string) => {
      ctx.font = `72px ${font}`;
      return ctx.measureText(sample).width;
    };
    const mono = width("monospace");
    const serif = width("serif");
    for (const f of CANDIDATES.slice(1)) {
      if (width(`"${f}", monospace`) !== mono || width(`"${f}", serif`) !== serif) out.push(f);
    }
  } catch {
    // No canvas (tests): SF Mono only.
  }
  installed = out;
  return out;
}
