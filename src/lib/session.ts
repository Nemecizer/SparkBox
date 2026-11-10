// App-wide UI state that isn't cluster data: the dialog queue (confirmations,
// alerts and text prompts) and the shell's panels. Dialogs queue up, so a second
// request waits for the first instead of replacing it, and every promise settles.
import { useSyncExternalStore, type ReactNode } from "react";

function createStore<T>(initial: T) {
  let value = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => value,
    set: (v: T) => {
      value = v;
      listeners.forEach((l) => l());
    },
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
}

// --- Admin mode: while locked, every action that changes something is disabled.
// The padlock lives in admin.ts (enforced in Rust); kept exported here for existing callers.

export { isAdminUnlocked, setAdmin, useAdmin } from "./admin";

// --- Dialogs

export type DialogKind = "confirm" | "info" | "error" | "prompt";

export interface DialogAction {
  label: string;
  run: () => void | Promise<void>;
}

export interface ConfirmOptions {
  title: string;
  body?: ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  /** Destructive: Cancel is the default button and clicking outside does nothing. */
  danger?: boolean;
  /** Extra detail behind a disclosure, with a Copy button. */
  details?: ReactNode;
  /** The user must type this (e.g. a node name) before the confirm button works. */
  requireTyping?: string;
  /** Shows the dialog as a sheet on that window instead of over the whole app. */
  winId?: string;
  /**
   * Asking again for the same thing (e.g. "restart:cell1:3f2a") while that dialog is
   * showing or queued returns the same promise instead of queueing a second dialog.
   */
  dedupeKey?: string;
}

export interface ChoiceOptions extends ConfirmOptions {
  /**
   * A second way to go ahead, on the left like "Don't Save": "Restart 3 containers anyway"
   * beside the default "Restart llama-3.3-70b in order".
   */
  alternativeLabel: string;
}

export interface AlertOptions {
  title: string;
  body?: ReactNode;
  details?: ReactNode;
  okLabel?: string;
  winId?: string;
}

export interface ErrorOptions {
  title: string;
  body?: ReactNode;
  details?: string;
  /** Extra buttons beside OK, e.g. "Open Settings". Each closes the alert, then runs. */
  actions?: DialogAction[];
  winId?: string;
}

export interface PromptOptions {
  title: string;
  body?: ReactNode;
  label: string;
  initial?: string;
  placeholder?: string;
  confirmLabel: string;
  /** Returns an error message for invalid input, or null. */
  validate?: (value: string) => string | null;
  winId?: string;
}

export interface DialogRequest {
  id: number;
  kind: DialogKind;
  title: string;
  body?: ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  danger?: boolean;
  details?: ReactNode;
  requireTyping?: string;
  winId?: string;
  actions?: DialogAction[];
  /** A second confirm button (confirmChoice). */
  alternative?: { label: string };
  prompt?: { label: string; initial: string; placeholder?: string; validate?: (v: string) => string | null };
  /** The element that had focus when it was asked for; focus goes back there afterwards. */
  opener: Element | null;
  /** ok: confirmed (or OK); value: the prompt's text. */
  settle: (ok: boolean, value?: string) => void;
}

/** Kept for existing callers: the request ConfirmHost shows. */
export type ConfirmRequest = DialogRequest;

const queue = createStore<DialogRequest[]>([]);
let seq = 0;

function enqueue(req: Omit<DialogRequest, "id" | "opener" | "settle">): Promise<{ ok: boolean; value?: string }> {
  return new Promise((resolve) => {
    const id = ++seq;
    const entry: DialogRequest = {
      ...req,
      id,
      opener: typeof document !== "undefined" ? document.activeElement : null,
      settle: (ok, value) => {
        if (!queue.get().some((r) => r.id === id)) return;
        queue.set(queue.get().filter((r) => r.id !== id));
        resolve({ ok, value });
      },
    };
    queue.set([...queue.get(), entry]);
  });
}

const pendingConfirms = new Map<string, Promise<boolean>>();

/** Asks before doing something. Resolves true when confirmed. */
export function confirmAction(opts: ConfirmOptions): Promise<boolean> {
  const { dedupeKey, ...rest } = opts;
  const waiting = dedupeKey ? pendingConfirms.get(dedupeKey) : undefined;
  if (waiting) return waiting;
  const p = enqueue({ kind: "confirm", ...rest }).then((r) => r.ok);
  if (dedupeKey) {
    pendingConfirms.set(dedupeKey, p);
    const forget = () => pendingConfirms.delete(dedupeKey);
    p.then(forget, forget);
  }
  return p;
}

/**
 * Asks with two ways to go ahead. Resolves "confirm" (the default button), "alternative",
 * or null when cancelled.
 */
export function confirmChoice(opts: ChoiceOptions): Promise<"confirm" | "alternative" | null> {
  const { dedupeKey: _key, alternativeLabel, ...rest } = opts;
  return enqueue({ kind: "confirm", ...rest, alternative: { label: alternativeLabel } })
    .then((r) => (!r.ok ? null : r.value === "alternative" ? "alternative" : "confirm"));
}

const inFlight = new Map<string, Promise<unknown>>();

/**
 * Runs `fn` unless the same action (by key, e.g. "restart:cell1:3f2a") is already
 * under way, in which case it returns that run's promise. A triple-click on Restart
 * then asks once and restarts once.
 */
export function singleFlight<T>(key: string, fn: () => Promise<T>): Promise<T> {
  const running = inFlight.get(key);
  if (running) return running as Promise<T>;
  const p = (async () => fn())();
  inFlight.set(key, p);
  const forget = () => inFlight.delete(key);
  p.then(forget, forget);
  return p;
}

/** A notice with a single OK button. */
export async function alertInfo(opts: AlertOptions): Promise<void> {
  await enqueue({ kind: "info", title: opts.title, body: opts.body, details: opts.details, confirmLabel: opts.okLabel ?? "OK", winId: opts.winId });
}

/** Something failed: what happened, optional details to copy, and what to do next. */
export async function alertError(opts: ErrorOptions): Promise<void> {
  await enqueue({ kind: "error", title: opts.title, body: opts.body, details: opts.details, confirmLabel: "OK", actions: opts.actions, winId: opts.winId });
}

/** Asks for a line of text, e.g. a layout name. Resolves null when cancelled. */
export async function promptText(opts: PromptOptions): Promise<string | null> {
  const r = await enqueue({
    kind: "prompt",
    title: opts.title,
    body: opts.body,
    confirmLabel: opts.confirmLabel,
    winId: opts.winId,
    prompt: { label: opts.label, initial: opts.initial ?? "", placeholder: opts.placeholder, validate: opts.validate },
  });
  return r.ok ? (r.value ?? "") : null;
}

export function useDialogQueue(): DialogRequest[] {
  return useSyncExternalStore(queue.subscribe, queue.get);
}

/** The app-level dialog being shown (sheets on windows aside), for older callers. */
export function useConfirmRequest(): DialogRequest | null {
  return useDialogQueue().find((r) => !r.winId) ?? null;
}

/** Cancels every dialog waiting on a window, e.g. because it closed. */
export function cancelDialogsFor(winId: string) {
  for (const r of queue.get()) if (r.winId === winId) r.settle(false);
}

export function dialogOpen(): boolean {
  return queue.get().length > 0;
}

// --- Shell panels: the command palette, Keyboard Shortcuts, Welcome and the window overview.

export type ShellPanel = "palette" | "shortcuts" | "welcome" | "overview" | null;
const panel = createStore<ShellPanel>(null);

export function openPanel(p: ShellPanel) {
  panel.set(p);
}

export function togglePanel(p: Exclude<ShellPanel, null>) {
  panel.set(panel.get() === p ? null : p);
}

export function closePanel() {
  panel.set(null);
}

export function usePanel(): ShellPanel {
  return useSyncExternalStore(panel.subscribe, panel.get);
}
