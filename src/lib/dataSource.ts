// Which world the app is showing: the simulator (Demo) or the real cluster (Live). Every store
// that keeps rows between launches keys them by source (scopedKey), so a Demo task, chat or
// terminal session never shows in Live, and switching the source swaps every store at once.
//
// The truth is the snapshot's `demo` flag: it follows Settings › Data source and a run-only
// override (SPARKBOX_DATA_SOURCE) alike, and it changes the moment the backend switches. The
// last source seen is remembered on this Mac, so a store read at start-up, before the first
// snapshot arrives, already reads the right rows; if the first snapshot disagrees, every
// listener hears about it and reloads.
import { useSyncExternalStore } from "react";
import { api, inTauri } from "./api";
import type { DataSource } from "./types";

export type Source = DataSource;

const LAST_KEY = "sparkbox.dataSource";

function readLast(): Source {
  try {
    return localStorage.getItem(LAST_KEY) === "live" ? "live" : "demo";
  } catch {
    // Demo is the default for a new install.
    return "demo";
  }
}

let source: Source = readLast();
let started = false;
const listeners = new Set<(next: Source, prev: Source) => void>();
const hookListeners = new Set<() => void>();

function start() {
  if (started || !inTauri) return;
  started = true;
  api.getSnapshot().then((s) => s && noteSource(s.demo ? "demo" : "live"), () => {});
  api.onSnapshot((s) => noteSource(s.demo ? "demo" : "live")).catch(() => {});
}

/** Tells the app which source it now shows (the snapshot listener calls it; so can a test). */
export function noteSource(next: Source) {
  if (next !== "demo" && next !== "live") return;
  if (next === source) return;
  const prev = source;
  source = next;
  try {
    localStorage.setItem(LAST_KEY, next);
  } catch {
    // Remembered for this session only.
  }
  listeners.forEach((cb) => cb(next, prev));
  hookListeners.forEach((l) => l());
}

/** "demo" or "live": what the app shows right now. */
export function currentSource(): Source {
  start();
  return source;
}

/** The current source, re-rendering when it changes. */
export function useDataSource(): Source {
  return useSyncExternalStore(
    (l) => {
      start();
      hookListeners.add(l);
      return () => hookListeners.delete(l);
    },
    () => source,
  );
}

/** A storage key for the current source: `base + ":" + source`, e.g. "sparkbox.terminal.recent:live". */
export function scopedKey(base: string, s: Source = currentSource()): string {
  return `${base}:${s}`;
}

/** Calls `cb` whenever the source changes; returns the unsubscribe. */
export function onSourceChange(cb: (next: Source, prev: Source) => void): () => void {
  start();
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/**
 * Splits a store saved before stores were keyed by source (one localStorage key) into its Demo
 * and Live keys, once, then removes the old key. A JSON array is split row by row; anything else
 * moves as a whole. `classify` names a row's source; a row it can't place is Demo, because Live
 * was first used on Oct 29 and everything older came from the simulator. Rows already under a
 * scoped key stay first.
 */
export function migrateLegacy<T = unknown>(key: string, classify: (row: T) => Source | null | undefined = () => "demo"): void {
  let raw: string | null;
  try {
    raw = localStorage.getItem(key);
  } catch {
    return;
  }
  if (raw === null) return;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    value = raw;
  }
  const sourceOf = (row: T): Source => {
    try {
      return classify(row) === "live" ? "live" : "demo";
    } catch {
      return "demo";
    }
  };
  try {
    if (Array.isArray(value)) {
      for (const s of ["demo", "live"] as const) {
        const rows = (value as T[]).filter((r) => sourceOf(r) === s);
        if (!rows.length) continue;
        const k = scopedKey(key, s);
        let kept: unknown[] = [];
        try {
          const v = JSON.parse(localStorage.getItem(k) ?? "[]");
          kept = Array.isArray(v) ? v : [];
        } catch {
          kept = [];
        }
        localStorage.setItem(k, JSON.stringify([...kept, ...rows]));
      }
    } else {
      const k = scopedKey(key, sourceOf(value as T));
      if (localStorage.getItem(k) === null) localStorage.setItem(k, raw);
    }
    localStorage.removeItem(key);
  } catch {
    // Storage full or blocked: the old key stays, and is tried again next launch.
  }
}

/** Puts the source back without telling anyone (tests; the stores' listeners stay subscribed). */
export function _resetDataSourceForTests(next: Source = "demo") {
  source = next;
  started = false;
}
