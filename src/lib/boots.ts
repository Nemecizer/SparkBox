// A node's boots (src-tauri/src/health/boots.rs): when each started and ended, how it ended
// (from the node's journal) and how SparkBox knows of it. Live only: Demo has no journal.
import { useEffect, useState } from "react";
import { inTauri, invoke } from "./api";

/** clean: systemd reached a reboot, power-off or shutdown target. unclean: no shutdown (power loss, hang or watchdog). unknown: its journal is gone. */
export type BootEnd = "clean" | "unclean" | "unknown";

export interface Boot {
  /** 32 hex digits, the journal's form. */
  bootId: string;
  startMs: number;
  /** Its journal's last entry; null while it's running. */
  endMs?: number | null;
  /** Null while it's running, or until its journal has been read. */
  ended?: BootEnd | null;
  /** Its last warnings (unclean) or shutdown lines (clean), oldest first. */
  lastLines?: string[];
  /** live: SparkBox saw the bootId change to it. journal: found in the journal. first: running when SparkBox first connected. */
  source: "live" | "journal" | "first";
  /** SparkBox restarted the node into it. */
  planned?: boolean;
}

export interface BootsView {
  node: string;
  /** Newest first. */
  boots: Boot[];
  /** The journal keeps only the current boot. */
  currentOnly: boolean;
  readMs: number | null;
  reading: boolean;
  error: string | null;
  /** The running boot followed a restart SparkBox didn't make, in the last day: when it started. */
  restartedMs: number | null;
  /** With restartedMs: how the boot before it ended, once the journal has said. */
  restartedEnd?: BootEnd | null;
}

export const bootsApi = {
  get: (node: string) => invoke<BootsView>("boots_get", { node }),
  /** Reads the node's journal again: reads only, as the cluster user. */
  refresh: (node: string) => invoke<BootsView>("boots_refresh", { node }),
};

/** SparkBox reads how boots ended for the last 14 days (boots.rs JOURNAL_MS). */
const JOURNAL_MS = 14 * 86_400_000;

/** The Boots table's "How it ended". `next`: the boot after it (newer), if any. */
export function endedWords(b: Boot, running: boolean, next?: Boot, now: number = Date.now()): string {
  if (running) return "Running now";
  switch (b.ended) {
    case "clean":
      return next?.planned ? "Clean shutdown (SparkBox restarted it)" : "Clean shutdown";
    case "unclean":
      return "Ended without a shutdown (power loss, hang or watchdog)";
    case "unknown":
      return "Not known: the journal no longer has it";
    default:
      return b.endMs && now - b.endMs > JOURNAL_MS ? "Not read: more than 14 days ago" : "Not read from the journal yet";
  }
}

/** The Boots table's "Seen". */
export function seenWords(b: Boot): string {
  if (b.source === "live") return b.planned ? "Live · SparkBox restarted it" : "Live";
  if (b.source === "journal") return "Found in the journal";
  return "Running when SparkBox connected";
}

/**
 * A node's boots, read again every 30 s (every 3 s while its journal is being read) and whenever
 * its bootId changes; null in Demo, outside the app, or before the first answer.
 */
export function useBoots(node: string, live: boolean, bootId?: string | null): [BootsView | null, (v: BootsView) => void] {
  const [view, setView] = useState<BootsView | null>(null);
  useEffect(() => {
    if (!inTauri || !live) {
      setView(null);
      return;
    }
    let alive = true;
    let t: number | undefined;
    const load = () =>
      bootsApi.get(node).then(
        (v) => {
          if (!alive) return;
          setView(v);
          t = window.setTimeout(load, v.reading ? 3_000 : 30_000);
        },
        () => alive && (t = window.setTimeout(load, 30_000)),
      );
    load();
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
  }, [node, live, bootId]);
  return [view, setView];
}
