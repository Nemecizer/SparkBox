// Hugging Face list rows sized the way the detail sizes them: from the repo's own files
// (Rust hf::sizing, the weights vLLM serves), not the Hub's parameter counts, which are
// 44–277% off for packed 4- and 8-bit repos. Only rows on screen are asked for, four at a
// time, and an answer is kept for a day (Rust keeps it a day too, across windows).
import { useSyncExternalStore } from "react";
import { modelsApi, type HfSizing } from "../../lib/modelsApi";
import { errText } from "./shared";

export const AT_ONCE = 4;
export const KEEP_MS = 24 * 3600_000;

export interface SizeEntry {
  at: number;
  sizing?: HfSizing;
  /** Why the files couldn't be read (the row falls back to an estimate, marked as one). */
  error?: string;
}

const cache = new Map<string, SizeEntry>();
const queue: string[] = [];
const inFlight = new Set<string>();
const listeners = new Set<() => void>();
let fetcher: (repo: string) => Promise<HfSizing> = (repo) => modelsApi.hfSize(repo);

function emit() {
  listeners.forEach((l) => l());
}

function fresh(e: SizeEntry | undefined): boolean {
  return !!e && Date.now() - e.at < KEEP_MS;
}

function pump() {
  while (inFlight.size < AT_ONCE && queue.length) {
    const repo = queue.shift()!;
    if (fresh(cache.get(repo))) continue;
    inFlight.add(repo);
    fetcher(repo)
      .then(
        (sizing) => cache.set(repo, { at: Date.now(), sizing }),
        (e) => cache.set(repo, { at: Date.now(), error: errText(e) }),
      )
      .finally(() => {
        inFlight.delete(repo);
        emit();
        pump();
      });
  }
}

/** Asks for a row's size (a row that scrolled into view); does nothing when it's known or on its way. */
export function requestSize(repo: string) {
  if (fresh(cache.get(repo)) || inFlight.has(repo) || queue.includes(repo)) return;
  queue.push(repo);
  pump();
}

/** Rows that scrolled away before their turn aren't asked for. */
export function forgetQueued(repo: string) {
  const i = queue.indexOf(repo);
  if (i >= 0) queue.splice(i, 1);
}

/** The detail read the files itself: the list shows the same number from now on. */
export function noteSize(sizing: HfSizing) {
  cache.set(sizing.id, { at: Date.now(), sizing });
  emit();
}

export function sizeOf(repo: string): SizeEntry | undefined {
  const e = cache.get(repo);
  return fresh(e) ? e : undefined;
}

let version = 0;
listeners.add(() => version++);

/** Re-renders when any row's size arrives. */
export function useSizes(): number {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => version,
  );
}

/** Tests: swap the backend call, and start over. */
export function _setFetcher(f: (repo: string) => Promise<HfSizing>) {
  fetcher = f;
  cache.clear();
  queue.length = 0;
  inFlight.clear();
}

export function _inFlight(): number {
  return inFlight.size;
}
