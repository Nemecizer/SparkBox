// Toasts: short notices at the bottom right of the desktop, above the status bar,
// stacking upward. Success and info go away on their own after 4 s (the timer
// pauses while the pointer is over them); errors and progress stay until
// dismissed or updated. Every toast is announced.
//
// They stay out of the way: a success from a window that's in front, where the
// result is already on screen, isn't shown at all; one operation (`opId`) gets
// one toast however many places report on it; a `key` replaces the toast that
// said the last thing about the same subject; the same words twice within 2 s
// are one toast; and only the newest three show, with "N more" for the rest.
import { useSyncExternalStore } from "react";
import { announce } from "./announce";

export type ToastKind = "success" | "error" | "info" | "progress";

export interface ToastAction {
  label: string;
  run: () => unknown;
}

export interface ToastOptions {
  kind: ToastKind;
  title: string;
  body?: string;
  actions?: ToastAction[];
  /** Shows an Undo button; running it dismisses the toast. */
  undo?: () => unknown;
  /** 0 keeps it until dismissed. Defaults: success and info 4 s, error and progress never. */
  durationMs?: number;
  /** Reuse an id to replace a toast in place. */
  id?: string;
  /**
   * The subject it reports on ("endpoint:llama-3.3-70b"). A toast with the same key replaces
   * it whatever its kind: "llama-3.3-70b is answering again" takes the place of "is down".
   */
  key?: string;
  /** Progress kind: 0..1, or null for indeterminate. */
  progress?: number | null;
  /**
   * The window the action started in. A success from a window that's in front and
   * visible is left out: the window already shows the result.
   */
  origin?: string;
  /**
   * The operation it reports on. Toasts with the same opId are one toast: an action's
   * error and its task's result merge, and an error is never replaced by a milder kind.
   */
  opId?: string;
}

export interface Toast extends ToastOptions {
  id: string;
  createdAt: number;
  /** When its words last changed (the 2 s dedupe window starts here). */
  updatedAt: number;
}

/** How many show at once; the rest wait behind "N more". */
export const MAX_VISIBLE = 3;
/** How many are kept at all; past this the oldest that isn't an error goes. */
const MAX_KEPT = 12;
const DEFAULT_MS: Record<ToastKind, number> = { success: 4000, info: 4000, error: 0, progress: 0 };
/** The same title and body again within this long is the same toast (a double-click). */
export const DEDUPE_MS = 2000;

let toasts: Toast[] = [];
const listeners = new Set<() => void>();
const timers = new Map<string, { handle: number; endsAt: number; remaining: number }>();
let seq = 0;

/** Whether a window is in front and visible; the window manager installs the real one. */
let inFront: (winId: string) => boolean = () => false;
/** The window in front, if any (the window manager installs it with the probe). */
let front: () => string | null = () => null;

export function setFrontWindowProbe(probe: (winId: string) => boolean, current: () => string | null = () => null) {
  inFront = probe;
  front = current;
}

/** The id of the window in front, or null (none, or the app is in the background). */
export function frontWindow(): string | null {
  try {
    return front();
  } catch {
    return null;
  }
}

/** Windows that started an operation, for toasts that report on it by opId alone (a task's result). */
const opOrigins = new Map<string, string>();
/** Op ids that report under another one: a task started by a gesture whose error already showed. */
const opAliases = new Map<string, string>();

function remember<V>(m: Map<string, V>, k: string, v: V) {
  m.delete(k);
  m.set(k, v);
  // Only recent ones matter.
  if (m.size > 100) m.delete(m.keys().next().value!);
}

/** Remembers which window started an operation, e.g. a task it kicked off. */
export function noteOpOrigin(opId: string, winId: string) {
  remember(opOrigins, opId, winId);
}

/**
 * Makes toasts for `opId` report under `as`: a backend task that a gesture started merges
 * with the gesture's own error toast, whichever arrives first.
 */
export function aliasOp(opId: string, as: string) {
  if (opId === as) return;
  remember(opAliases, opId, as);
  const origin = opOrigins.get(as);
  if (origin && !opOrigins.has(opId)) noteOpOrigin(opId, origin);
}

function canonicalOp(opId: string): string {
  return opAliases.get(opId) ?? opId;
}

function emit() {
  listeners.forEach((l) => l());
}

function durationOf(t: ToastOptions): number {
  return t.durationMs ?? DEFAULT_MS[t.kind];
}

function arm(id: string, ms: number) {
  disarm(id);
  if (ms <= 0) return;
  const handle = window.setTimeout(() => dismissToast(id), ms);
  timers.set(id, { handle, endsAt: Date.now() + ms, remaining: ms });
}

function disarm(id: string) {
  const t = timers.get(id);
  if (t) window.clearTimeout(t.handle);
  timers.delete(id);
}

function speak(t: ToastOptions) {
  announce([t.title, t.body].filter(Boolean).join(". "), t.kind === "error" ? "assertive" : "polite");
}

/** A success the user is already looking at: its window is in front. */
function alreadyOnScreen(opts: ToastOptions): boolean {
  if (opts.kind !== "success" || !opts.origin || opts.undo || opts.actions?.length) return false;
  try {
    return inFront(opts.origin);
  } catch {
    return false;
  }
}

const sameWords = (a: ToastOptions, b: ToastOptions) => a.kind === b.kind && a.title === b.title && (a.body ?? "") === (b.body ?? "");

/**
 * Shows a toast and returns its id. A success whose origin window is in front is not
 * shown (the id is still returned, so updateToast on it is a no-op). One toast stands for
 * one outcome: the same `opId` merges (an error stays an error), the same `key` replaces,
 * and the same words within DEDUPE_MS are the toast already showing.
 */
export function toast(opts: ToastOptions): string {
  if (opts.opId) opts = { ...opts, opId: canonicalOp(opts.opId) };
  if (!opts.origin && opts.opId && opOrigins.has(opts.opId)) opts = { ...opts, origin: opOrigins.get(opts.opId) };
  const now = Date.now();
  const sameOp = opts.opId ? toasts.find((t) => t.opId === opts.opId) : undefined;
  const sameKey = opts.key ? toasts.find((t) => t.key === opts.key) : undefined;
  if (!opts.id && !sameOp && !sameKey) {
    const twin = toasts.find((t) => sameWords(t, opts) && now - t.updatedAt < DEDUPE_MS);
    if (twin) {
      arm(twin.id, durationOf(twin));
      return twin.id;
    }
  }
  const id = opts.id ?? sameKey?.id ?? sameOp?.id ?? `toast-${++seq}`;
  const existing = toasts.find((t) => t.id === id);
  // One operation, one toast: an error already showing for it stays an error.
  if (!sameKey && sameOp?.kind === "error" && opts.kind !== "error") return sameOp.id;
  if (alreadyOnScreen(opts)) {
    if (existing) dismissToast(id);
    return id;
  }
  // An action's error and its task's failure are one notice: the first words stay, the buttons add up.
  const bothErrors = !!sameOp && !sameKey && !opts.id && sameOp.kind === "error" && opts.kind === "error";
  const merged: ToastOptions = bothErrors
    ? { ...opts, title: sameOp!.title, body: sameOp!.body ?? opts.body, actions: mergeActions(sameOp!.actions, opts.actions) }
    : opts;
  const next: Toast = { ...merged, id, createdAt: existing?.createdAt ?? now, updatedAt: now };
  if (existing) {
    const reworded = !sameWords(existing, next);
    if (!reworded) next.updatedAt = existing.updatedAt;
    toasts = toasts.map((t) => (t.id === id ? next : t));
    arm(id, durationOf(next));
    if (reworded) speak(next);
    emit();
    return id;
  }
  toasts = [...toasts, next];
  while (toasts.length > MAX_KEPT) {
    const drop = toasts.find((t) => t.kind !== "error") ?? toasts[0];
    disarm(drop.id);
    toasts = toasts.filter((t) => t !== drop);
  }
  arm(id, durationOf(next));
  speak(next);
  emit();
  return id;
}

/** Buttons from both reports of one outcome, each label once. */
function mergeActions(a: ToastAction[] | undefined, b: ToastAction[] | undefined): ToastAction[] | undefined {
  const all = [...(a ?? []), ...(b ?? [])];
  const out = all.filter((x, i) => all.findIndex((y) => y.label === x.label) === i);
  return out.length ? out : undefined;
}

/** Changes a toast in place, e.g. a progress toast finishing as success. */
export function updateToast(id: string, patch: Partial<ToastOptions>) {
  const t = toasts.find((x) => x.id === id);
  if (!t) return;
  const next = { ...t, ...patch, id, updatedAt: Date.now() };
  if (alreadyOnScreen(next)) return dismissToast(id);
  toasts = toasts.map((x) => (x.id === id ? next : x));
  if (patch.kind || patch.durationMs !== undefined) arm(id, durationOf(next));
  if (patch.kind && patch.kind !== t.kind) speak(next);
  emit();
}

export function dismissToast(id: string) {
  disarm(id);
  if (!toasts.some((t) => t.id === id)) return;
  toasts = toasts.filter((t) => t.id !== id);
  emit();
}

/** Hovering a toast stops its clock; leaving restarts it with the time it had left. */
export function pauseToast(id: string) {
  const t = timers.get(id);
  if (!t) return;
  window.clearTimeout(t.handle);
  timers.set(id, { ...t, handle: 0, remaining: Math.max(0, t.endsAt - Date.now()) });
}

export function resumeToast(id: string) {
  const t = timers.get(id);
  if (!t || t.handle) return;
  arm(id, Math.max(1200, t.remaining));
}

export function getToasts(): Toast[] {
  return toasts;
}

/** What the stack shows: the newest MAX_VISIBLE (oldest first) and how many wait behind them. */
export function visibleToasts(all: Toast[] = toasts, expanded = false): { shown: Toast[]; more: number } {
  if (expanded || all.length <= MAX_VISIBLE) return { shown: all, more: 0 };
  return { shown: all.slice(-MAX_VISIBLE), more: all.length - MAX_VISIBLE };
}

export function useToasts(): Toast[] {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => toasts,
  );
}
