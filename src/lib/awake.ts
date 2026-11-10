// "Keeping this Mac awake while … runs" (src-tauri/src/awake.rs): the sleep assertions SparkBox
// holds while an orchestration runs. `awake://status` comes on every change and every 5 s while
// one is held, so a window that loads mid-run catches up; without word for 15 s the line goes.
import { useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./api";

export interface AwakeHold {
  /** "Restart cell2", "Fleet: apt-get -y upgrade (as root)". */
  what: string;
  sinceMs: number;
  /** The task it belongs to, when it's one. */
  task: string | null;
}

export interface AwakeStatus {
  holds: AwakeHold[];
}

export const AWAKE_EVENT = "awake://status";
const SILENT_AFTER_MS = 15_000;

let holds: AwakeHold[] = [];
let started = false;
let quiet: ReturnType<typeof setTimeout> | null = null;
const listeners = new Set<() => void>();

function publish(s: AwakeStatus) {
  holds = s.holds ?? [];
  if (quiet) clearTimeout(quiet);
  quiet = holds.length ? setTimeout(() => publish({ holds: [] }), SILENT_AFTER_MS) : null;
  listeners.forEach((l) => l());
}

function start() {
  if (started || !inTauri) return;
  started = true;
  listen<AwakeStatus>(AWAKE_EVENT, (e) => publish(e.payload)).catch(() => {
    started = false;
  });
}

/** What SparkBox is keeping this Mac awake for right now, oldest first. */
export function useAwake(): AwakeHold[] {
  return useSyncExternalStore(
    (l) => {
      start();
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => holds,
  );
}

/** "Keeping this Mac awake while Restart cell2 runs", or null while nothing is held. */
export function awakeLine(h: readonly AwakeHold[]): string | null {
  if (!h.length) return null;
  const what = h.length === 1 ? h[0].what : h.length === 2 ? `${h[0].what} and ${h[1].what}` : `${h[0].what} and ${h.length - 1} more`;
  return `Keeping this Mac awake while ${what} ${h.length === 1 ? "runs" : "run"}`;
}

/** Tests: forget what was heard and listen again. */
export function _resetAwakeForTests() {
  if (quiet) clearTimeout(quiet);
  quiet = null;
  holds = [];
  started = false;
}
