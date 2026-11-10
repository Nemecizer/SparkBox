// Undo for app-level actions (closing windows, moving a file to the Trash…):
// each one shows a toast with an Undo button, and ⌘Z outside text fields and
// terminals undoes the most recent one that hasn't expired.
import { dismissToast, toast } from "./toasts";

export interface UndoEntry {
  label: string;
  undo: () => void | Promise<void>;
  /** How long Undo stays available (default 10 s). */
  ttlMs?: number;
  /**
   * Offers of the same kind that are still live fold into one toast whose Undo
   * reverts them all, newest first: three closes read "3 windows closed".
   */
  collapse?: { key: string; count: number; label: (count: number) => string };
}

interface Live {
  id: number;
  entry: UndoEntry;
  expiresAt: number;
  toastId: string;
}

const DEFAULT_TTL = 10_000;
let stack: Live[] = [];
let seq = 0;

function prune() {
  const now = Date.now();
  stack = stack.filter((e) => e.expiresAt > now);
}

/** Offers Undo for something that just happened. Returns a function that withdraws the offer. */
export function pushUndo(entry: UndoEntry): () => void {
  prune();
  if (entry.collapse) entry = fold(entry);
  const id = ++seq;
  const ttl = entry.ttlMs ?? DEFAULT_TTL;
  const live: Live = { id, entry, expiresAt: Date.now() + ttl, toastId: "" };
  live.toastId = toast({
    kind: "info",
    title: entry.label,
    durationMs: ttl,
    undo: () => {
      void runUndo(id);
    },
  });
  stack.push(live);
  return () => {
    stack = stack.filter((e) => e.id !== id);
    dismissToast(live.toastId);
  };
}

/** Merges the live offers with the same collapse key into `entry`, and withdraws them. */
function fold(entry: UndoEntry): UndoEntry {
  const c = entry.collapse!;
  const earlier = stack.filter((e) => e.entry.collapse?.key === c.key);
  if (!earlier.length) return entry;
  stack = stack.filter((e) => !earlier.includes(e));
  for (const e of earlier) dismissToast(e.toastId);
  const count = earlier.reduce((n, e) => n + (e.entry.collapse?.count ?? 1), c.count);
  const undos = [entry.undo, ...earlier.reverse().map((e) => e.entry.undo)];
  return {
    ...entry,
    label: c.label(count),
    collapse: { ...c, count },
    undo: async () => {
      for (const u of undos) await u();
    },
  };
}

async function runUndo(id: number): Promise<boolean> {
  const live = stack.find((e) => e.id === id);
  if (!live) return false;
  stack = stack.filter((e) => e.id !== id);
  dismissToast(live.toastId);
  try {
    await live.entry.undo();
  } catch (e) {
    toast({ kind: "error", title: `Couldn't undo: ${live.entry.label}`, body: String(e).replace(/^Error: /, "") });
    return false;
  }
  return true;
}

/** Undoes the most recent action that's still undoable. False when there's nothing to undo. */
export function runLatestUndo(): Promise<boolean> {
  prune();
  const latest = stack[stack.length - 1];
  return latest ? runUndo(latest.id) : Promise.resolve(false);
}

export function canUndo(): boolean {
  prune();
  return stack.length > 0;
}

/** True when ⌘Z belongs to the focused element (a text field, the editor or a terminal). */
export function focusOwnsUndo(el: Element | null = document.activeElement): boolean {
  if (!el || el === document.body) return false;
  if (el.closest(".xterm, .cm-editor, [contenteditable=''], [contenteditable='true']")) return true;
  if (el instanceof HTMLTextAreaElement) return true;
  if (el instanceof HTMLInputElement) return !["checkbox", "radio", "button", "submit", "range", "color"].includes(el.type);
  return el instanceof HTMLSelectElement;
}

/** For tests: forget every offer. */
export function _resetUndoForTests() {
  for (const e of stack) dismissToast(e.toastId);
  stack = [];
}
