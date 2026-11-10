// Containers that exited badly and still deserve attention: an error exit or an
// OOM kill in the last 24 hours that nobody has hidden yet. The sidebar badge,
// the Dashboard's Containers tile and the Containers window all count these, so
// a test container that died last week doesn't keep an amber badge forever.
// "Hide this exit" acknowledges one exit (by its finish time, so the next crash
// of the same container shows again); acknowledgements live in localStorage, one list per
// data source, so hiding a Demo exit never hides a Live one.
import { useMemo, useSyncExternalStore } from "react";
import { migrateLegacy, onSourceChange, scopedKey } from "./dataSource";
import { useCluster } from "./store";
import type { ClusterSnapshot, Container } from "./types";

export interface RecentExit {
  node: string;
  id: string;
  name: string;
  code: number;
  /** Unix seconds, as docker reports it. */
  finishedAt: number;
  oom: boolean;
}

/** Exits older than this are history, not news. */
export const RECENT_EXIT_S = 24 * 3600;
const ACK_KEY = "sparkbox.exits.acked.v1";
const ACK_MAX = 200;

const ackKey = (node: string, id: string, finishedAt: number) => `${node}/${id}@${Math.round(finishedAt)}`;

let acked: Set<string> | null = null;
const listeners = new Set<() => void>();
let version = 0;
let watching = false;

function readAcked(): Set<string> {
  if (!watching) {
    watching = true;
    // Hidden exits from before the split were Demo's (Live was first used on Oct 29).
    migrateLegacy(ACK_KEY);
    onSourceChange(() => {
      acked = null;
      version++;
      listeners.forEach((l) => l());
    });
  }
  if (acked) return acked;
  try {
    const v = JSON.parse(localStorage.getItem(scopedKey(ACK_KEY)) ?? "[]");
    acked = new Set(Array.isArray(v) ? v.filter((x) => typeof x === "string") : []);
  } catch {
    acked = new Set();
  }
  return acked;
}

/** An exit that ended badly: a non-zero code, an OOM kill, or a dead container. */
export function isBadExit(c: Container): boolean {
  if (c.state !== "exited" && c.state !== "dead") return false;
  return c.exitCode !== 0 || !!c.oomKilled || c.state === "dead";
}

/** Whether an exit happened within the last 24 hours of `nowS`. */
export function isRecentExit(c: Container, nowS: number): boolean {
  return c.finishedAt > 0 && nowS - c.finishedAt <= RECENT_EXIT_S;
}

export function isExitAcked(node: string, id: string, finishedAt: number): boolean {
  return readAcked().has(ackKey(node, id, finishedAt));
}

/** Hides one exit from the badges and tiles. A later exit of the same container shows again. */
export function ackExit(node: string, id: string, finishedAt: number) {
  const set = readAcked();
  const key = ackKey(node, id, finishedAt);
  if (set.has(key)) return;
  set.add(key);
  // Oldest first in insertion order; keep the list from growing forever.
  const keep = [...set].slice(-ACK_MAX);
  acked = new Set(keep);
  try {
    localStorage.setItem(scopedKey(ACK_KEY), JSON.stringify(keep));
  } catch {
    // Hidden for this session only.
  }
  version++;
  listeners.forEach((l) => l());
}

/** The recent, unacknowledged bad exits in a snapshot, newest first. */
export function recentExits(snap: ClusterSnapshot | null): RecentExit[] {
  if (!snap) return [];
  const nowS = snap.tsMs / 1000;
  const out: RecentExit[] = [];
  for (const n of snap.nodes) {
    for (const c of n.containers) {
      if (!isBadExit(c) || !isRecentExit(c, nowS) || isExitAcked(n.id, c.id, c.finishedAt)) continue;
      out.push({ node: n.id, id: c.id, name: c.name, code: c.exitCode, finishedAt: c.finishedAt, oom: !!c.oomKilled });
    }
  }
  return out.sort((a, b) => b.finishedAt - a.finishedAt);
}

function useAckVersion(): number {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => version,
  );
}

/** Bad exits in the last 24 h that haven't been hidden, newest first. */
export function useRecentExits(): RecentExit[] {
  const cluster = useCluster();
  const v = useAckVersion();
  // The key changes only when an exit appears, goes or is hidden, not every second.
  const key = (cluster?.nodes ?? [])
    .flatMap((n) => n.containers.filter(isBadExit).map((c) => `${n.id}/${c.id}@${c.finishedAt}`))
    .join(",");
  // Re-checked every 10 min, so an exit ages out of the 24 h window on time.
  const bucket = cluster ? Math.floor(cluster.tsMs / 600_000) : 0;
  return useMemo(() => recentExits(cluster), [key, v, bucket]);
}

/** For tests: forget every acknowledgement. */
export function _resetExitsForTests() {
  acked = new Set();
  version++;
  try {
    localStorage.removeItem(scopedKey(ACK_KEY, "demo"));
    localStorage.removeItem(scopedKey(ACK_KEY, "live"));
  } catch {
    // ignore
  }
}
