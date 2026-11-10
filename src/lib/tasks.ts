// Tasks: what SparkBox is doing or has done to the cluster. Backend tasks (src-tauri/src/tasks.rs)
// arrive as `tasks://update` events (coalesced, without the log) plus `tasks://log` deltas
// with new log lines; client tasks (copies and other flows the UI drives) are started here.
// Both appear in the status bar's Tasks chip and the Tasks window, newest first. A task that
// finishes posts a toast under its op id: success goes away on its own, failure and a Verify
// warning stay with Details. Retry reopens the task's original confirmation, never a replay.
//
// Every task has an origin, the window it was started from: given (runTask's `origin`,
// withOrigin), or else the window the last click or key press went to. A success from the
// origin window while it's in front shows no toast, since the window already says so.
import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { requestAdmin } from "./admin";
import { inTauri } from "./api";
import { describeError, showError } from "./errors";
import { opsApi, type FleetStrategy, type Task, type TaskStep } from "./opsApi";
import { requestContainersBulk, requestPower, requestServicesApply, type ContainerTarget, type PowerAction } from "./power";
import { confirmAction, singleFlight } from "./session";
import { confirmModelRestart } from "./modelRestart";
import { getEndpoints } from "./endpoints";
import { recoveryFor } from "../windows/models/endpointActions";
import { modelsApi } from "./modelsApi";
import { aliasOp, frontWindow, noteOpOrigin, toast } from "./toasts";

export type { Task, TaskStep };

const KEEP = 200;
const LOG_LINES = 400;

let tasks = new Map<string, Task>();
let list: Task[] = [];
const listeners = new Set<() => void>();
const waiters = new Map<string, ((t: Task) => void)[]>();
const clientCancels = new Map<string, () => void>();
const clientRetries = new Map<string, () => void>();
/** Tasks already finished when first seen (loaded from disk) don't toast. */
const announced = new Set<string>();
/** The window each task was started from (task id → window id). */
const origins = new Map<string, string>();
let started = false;
let seq = 0;

// ---- Origins

/** A task the backend starts this soon after a click or key press in a window is that window's. */
const GESTURE_MS = 15_000;
/** Task updates arrive coalesced (every 200 ms): a gesture still claims tasks this long after it ends. */
export const GESTURE_GRACE_MS = 1500;

let lastGesture: { win: string | null; at: number } | null = null;
let watching = false;

interface OpenGesture {
  opId: string;
  origin: string | null;
  match?: (t: Task) => boolean;
  /** The subject its toasts report on ("endpoint:<id>"), handed on to the tasks it starts. */
  key?: string;
  /** Still open while 0; afterwards, when it stops claiming tasks. */
  until: number;
}
let gestures: OpenGesture[] = [];

/**
 * The window a DOM event happened in: its frame, or the window in front for a dialog or menu it
 * opened. The command palette belongs to no window.
 */
function windowOf(target: EventTarget | null): string | null {
  if (target instanceof Element && target.closest(".palette-scrim")) return null;
  const el = target instanceof Element ? target.closest<HTMLElement>(".win[data-win-id]") : null;
  return el?.dataset.winId ?? frontWindow();
}

function watchGestures() {
  if (watching || typeof window === "undefined") return;
  watching = true;
  const note = (e: Event) => {
    lastGesture = { win: windowOf(e.target), at: Date.now() };
  };
  window.addEventListener("pointerdown", note, true);
  window.addEventListener("keydown", note, true);
}

/** The window a task just started belongs to, and the gesture it reports under, if any. */
function claim(t: Task): { origin: string | null; gesture?: OpenGesture } {
  const now = Date.now();
  gestures = gestures.filter((g) => !g.until || g.until > now);
  const gesture = [...gestures].reverse().find((g) => !g.match || g.match(t));
  if (gesture) return { origin: gesture.origin, gesture };
  // No click or key press lately (a scheduled check, a task already running at launch): no window to credit.
  return { origin: lastGesture && now - lastGesture.at < GESTURE_MS ? lastGesture.win : null };
}

// ---- Keys: the subject a task's toasts report on (C7)

/** Op ids (and task ids) → the toast key their outcome is said under, e.g. "endpoint:<id>". */
const opKeys = new Map<string, string>();

/**
 * Says what an operation is about, so its outcome toast replaces (and is replaced by) other
 * toasts about the same thing: a failed restart's "didn't come back" and the endpoint's later
 * "is answering again" are one toast, keyed "endpoint:<id>", not matched by the words in it.
 */
export function keyOp(opId: string, key: string) {
  opKeys.delete(opId);
  opKeys.set(opId, key);
  if (opKeys.size > 200) opKeys.delete(opKeys.keys().next().value!);
}

/** The toast key a task reports under: its own, its op's, or the gesture's that started it. */
export function taskKey(t: Pick<Task, "id" | "opId">): string | undefined {
  return opKeys.get(t.id) ?? (t.opId ? opKeys.get(t.opId) : undefined);
}

function adopt(t: Task, given?: string | null) {
  if (origins.has(t.id)) return;
  const { origin, gesture } = given !== undefined ? { origin: given, gesture: undefined } : claim(t);
  const opId = t.opId ?? t.id;
  if (gesture) aliasOp(opId, gesture.opId);
  if (gesture?.key) keyOp(t.id, gesture.key);
  if (!origin) return;
  origins.set(t.id, origin);
  noteOpOrigin(opId, origin);
  if (origins.size > KEEP * 1.5) origins.delete(origins.keys().next().value!);
}

/** The window a task was started from, when known. */
export function taskOrigin(id: string): string | null {
  return origins.get(id) ?? null;
}

/**
 * Runs `fn` as one gesture from window `origin`, under one op id that it's handed. Tasks the
 * backend starts meanwhile (and up to GESTURE_GRACE_MS after) take that origin and report
 * under that op id, so the action's own error toast and the task's result are one toast,
 * whichever arrives first. `match` narrows which tasks count (e.g. the node it acts on).
 */
export async function withOrigin<T>(origin: string | null | undefined, fn: (opId: string) => Promise<T>, match?: (t: Task) => boolean, opts: { key?: string } = {}): Promise<T> {
  start();
  // No origin (the palette, the menu bar): the result is toasted wherever you are.
  const g: OpenGesture = { opId: `g${Date.now().toString(36)}${(++seq).toString(36)}`, origin: origin ?? null, match, until: 0, key: opts.key };
  if (g.origin) noteOpOrigin(g.opId, g.origin);
  if (opts.key) keyOp(g.opId, opts.key);
  gestures.push(g);
  try {
    return await fn(g.opId);
  } finally {
    g.until = Date.now() + GESTURE_GRACE_MS;
  }
}

function publish() {
  list = [...tasks.values()].sort((a, b) => b.startedMs - a.startedMs).slice(0, KEEP);
  if (tasks.size > KEEP * 1.5) tasks = new Map(list.map((t) => [t.id, t]));
  listeners.forEach((l) => l());
}

/**
 * What a task shows (C7): progress never moves backwards while it runs, and stops at 99%
 * until it has really finished (a model warming up after its download stays at 99%, or
 * indeterminate when it never had a number); once it ends, its summary (or error) replaces
 * the step it was on.
 */
export function shownTask(t: Task, before: Task | undefined): Task {
  if (t.state === "running") {
    if (t.progress == null) return t;
    const was = before?.state === "running" ? before.progress : null;
    const p = Math.min(0.99, Math.max(t.progress, was ?? 0));
    return p === t.progress ? t : { ...t, progress: p };
  }
  const said = t.summary ?? t.error;
  return said && t.detail !== said ? { ...t, detail: said } : t;
}

function upsert(t: Task, quiet = false) {
  const before = tasks.get(t.id);
  if (!before && !t.client) adopt(t);
  t = shownTask(t, before);
  // Backend updates carry no log; the lines arrive as deltas (appendLog).
  if (before && !t.client && t.log.length === 0 && before.log.length) t = { ...t, log: before.log };
  tasks.set(t.id, t);
  publish();
  if (t.state !== "running") {
    waiters.get(t.id)?.forEach((w) => w(t));
    waiters.delete(t.id);
    if (!quiet && !announced.has(t.id) && (!before || before.state === "running")) finishToast(t);
    announced.add(t.id);
  }
}

/** New log lines for a task (`tasks://log`), kept to the last 400. */
export function appendLog(id: string, lines: string[]) {
  const t = tasks.get(id);
  if (!t || !lines.length) return;
  tasks.set(id, { ...t, log: [...t.log, ...lines].slice(-LOG_LINES) });
  publish();
}

function openWindow(kind: string, params: Record<string, string> = {}) {
  window.dispatchEvent(new CustomEvent("sparkbox:open-window", { detail: { kind, params } }));
}

function finishToast(t: Task) {
  const opId = t.opId ?? t.id;
  const origin = origins.get(t.id);
  const key = taskKey(t);
  if (t.state === "ok" && t.warning) {
    toast({
      kind: "error",
      opId,
      key,
      origin,
      title: t.warning,
      body: t.summary ?? undefined,
      actions: [recoveryFor(t.warning, getEndpoints()), { label: "Open Endpoints", run: () => openWindow("endpoints") }, { label: "Details", run: () => openTasksWindow(t.id) }].filter((a): a is { label: string; run: () => void } => !!a),
    });
  } else if (t.state === "ok") {
    toast({ kind: "success", opId, origin, title: t.title, body: t.summary ?? undefined });
  } else if (t.state === "cancelled") {
    toast({ kind: "info", opId, origin, title: `${t.title}: stopped`, body: t.summary ?? t.error ?? undefined });
  } else {
    toast({
      kind: "error",
      opId,
      key,
      origin,
      title: `${t.title}: failed`,
      body: t.error ?? undefined,
      actions: [{ label: "Details", run: () => openTasksWindow(t.id) }],
    });
  }
}

function start() {
  if (started || typeof window === "undefined") return;
  started = true;
  watchGestures();
  if (!inTauri) return;
  opsApi.tasksList().then((all) => {
    for (const t of all) {
      if (!tasks.has(t.id)) {
        if (t.state !== "running") announced.add(t.id);
        tasks.set(t.id, t);
      }
    }
    publish();
  }, () => {});
  opsApi.onTask((t) => upsert(t)).catch(() => {});
  listen<{ id: string; lines: string[] }>("tasks://log", (e) => appendLog(e.payload.id, e.payload.lines)).catch(() => {});
}

function subscribe(l: () => void) {
  start();
  listeners.add(l);
  return () => listeners.delete(l);
}

/** Every task, newest first. */
export function useTasks(): Task[] {
  return useSyncExternalStore(subscribe, () => list);
}

export function useTask(id: string | null | undefined): Task | null {
  return useSyncExternalStore(subscribe, () => (id ? tasks.get(id) ?? null : null));
}

export function getTask(id: string): Task | null {
  return tasks.get(id) ?? null;
}

/** Every task, newest first (outside React). */
export function getTasks(): Task[] {
  start();
  return list;
}

/** Calls `cb` whenever a task changes. Returns an unsubscribe. */
export function onTasksChange(cb: () => void): () => void {
  return subscribe(cb);
}

// The task the Tasks window should show next (it's one window, whichever task it opens on).
let focusTask: { id: string; seq: number } | null = null;
const focusListeners = new Set<() => void>();

/** Opens the Tasks window, on one task when given. */
export function openTasksWindow(id?: string) {
  if (id) {
    focusTask = { id, seq: (focusTask?.seq ?? 0) + 1 };
    focusListeners.forEach((l) => l());
  }
  window.dispatchEvent(new CustomEvent("sparkbox:open-window", { detail: { kind: "tasks", params: {} } }));
}

/** The task most recently asked for with openTasksWindow(id). */
export function useFocusTask(): { id: string; seq: number } | null {
  return useSyncExternalStore(
    (l) => {
      focusListeners.add(l);
      return () => focusListeners.delete(l);
    },
    () => focusTask,
  );
}

/** Stops a task. Cancelling needs Admin mode like starting it did, except for read-only tasks (checks, benchmarks). */
export async function cancelTask(id: string): Promise<void> {
  const cancel = clientCancels.get(id);
  if (cancel) {
    cancel();
    return;
  }
  const t = tasks.get(id);
  if (!t?.readOnly && !(await requestAdmin("task_cancel"))) return;
  await opsApi.taskCancel(id);
}

type Args = Record<string, unknown>;
const str = (v: unknown, fallback = "") => (typeof v === "string" ? v : fallback);

/**
 * Starts a finished task again through its original confirmation, with the same targets and
 * options: a power action asks for the node's name again, containers and services show their
 * impact, Fleet goes through its tier (in the Fleet window). Never a one-click replay.
 * Resolves to the new task's id, or null when nothing started (yet).
 */
export async function retryTask(t: Task): Promise<string | null> {
  const local = clientRetries.get(t.id);
  if (local) {
    local();
    return null;
  }
  if (!t.retry) return null;
  const a = t.retry.args as Args;
  switch (t.retry.command) {
    case "deployment_restart":
      // Re-read the load and ask again. Never replay a one-attempt grant from retry args.
      return singleFlight(`retry-load:${str(a.id)}`, async () => {
        if (!(await requestAdmin("restart a model"))) return null;
        try {
          const d = (await modelsApi.deploymentsList()).find((d) => d.spec.id === str(a.id) && d.state !== "stopped");
          if (!d) throw new Error("That load isn't listed any more.");
          const vllmLoad = (d.spec.engine ?? "vllm") === "vllm";
          const order = d.spec.nodes.length < 2 ? "" : vllmLoad
            ? " The head stops first and starts last."
            : " The head stops first; it starts before the workers.";
          const dropCachesOnce = await confirmModelRestart({
            title: `Restart ${d.spec.servedName}?`,
            body: `Stops the load on ${d.spec.nodes.join(", ")} and starts it again with the same settings and key.${order} It's offline until it answers again; Tasks follows it until then.`,
            confirmLabel: "Restart",
          }, vllmLoad);
          if (dropCachesOnce === null || !(await requestAdmin("restart a model"))) return null;
          return await modelsApi.deploymentRestart(d.spec.id, dropCachesOnce);
        } catch (e) {
          showError(e, { action: `Retry “${t.title}”` });
          return null;
        }
      });
    case "node_power":
      return requestPower(str(a.node), str(a.action, "reboot") as PowerAction);
    case "containers_bulk":
      return requestContainersBulk((a.items as ContainerTarget[]) ?? [], str(a.action), a.removeVolumes === true, typeof a.title === "string" ? a.title : null);
    case "services_apply":
      return requestServicesApply((a.nodes as string[]) ?? [], str(a.unit), str(a.action));
    case "fleet_start": {
      // The Fleet window runs it: its tier's confirmation, and the canary's second step.
      const rerun: FleetRerun = {
        nodes: (a.nodes as string[]) ?? [], command: str(a.command), sudo: a.sudo === true, timeoutS: Number(a.timeoutS) || 60,
        strategy: (str(a.strategy, "parallel") as FleetStrategy), stopOnFailure: a.stopOnFailure !== false, title: typeof a.title === "string" ? a.title : null,
      };
      const seq = String(Date.now());
      openWindow("fleet", { rerun: JSON.stringify(rerun), seq });
      // An open Fleet window hears it here; a new one reads its params.
      window.dispatchEvent(new CustomEvent("sparkbox:fleet-rerun", { detail: { rerun, seq } }));
      return null;
    }
    case "updates_upgrade":
      // Upgrades go through the Updates window's own preview and confirmation.
      openWindow("updates", { node: str(a.node) });
      return null;
    default: {
      const ok = await confirmAction({
        title: `Run “${t.title}” again?`,
        body: `SparkBox starts it again with the same settings${t.nodes.length ? ` on ${t.nodes.join(", ")}` : ""}.`,
        confirmLabel: "Run again",
        danger: true,
      });
      if (!ok || !(await requestAdmin(t.retry.command))) return null;
      try {
        const r = await invoke<unknown>(t.retry.command, t.retry.args);
        return typeof r === "string" ? r : null;
      } catch (e) {
        showError(e, { action: `Retry “${t.title}”` });
        return null;
      }
    }
  }
}

/** What Retry hands the Fleet window (`rerun` param). */
export interface FleetRerun {
  nodes: string[];
  command: string;
  sudo: boolean;
  timeoutS: number;
  strategy: FleetStrategy;
  stopOnFailure: boolean;
  title: string | null;
}

export function canRetry(t: Task): boolean {
  return t.state !== "running" && (clientRetries.has(t.id) || !!t.retry);
}

/** Resolves when the task has finished (immediately if it already has). */
export function waitTask(id: string): Promise<Task> {
  start();
  const t = tasks.get(id);
  if (t && t.state !== "running") return Promise.resolve(t);
  return new Promise((resolve) => waiters.set(id, [...(waiters.get(id) ?? []), resolve]));
}

/** Feeds a task from outside (the mock backend in the browser harness). */
export function ingestTask(t: Task) {
  upsert(t);
}

// ---- Client tasks

export interface ClientTask {
  id: string;
  step(label: string): void;
  progress(frac: number, extra?: { rateBps?: number; etaS?: number; detail?: string }): void;
  log(line: string): void;
  finish(ok: boolean, message?: string): void;
  /** Ends it as cancelled. */
  cancelled(message?: string): void;
}

export function startClientTask(opts: {
  kind: string;
  title: string;
  nodes?: string[];
  source?: string;
  /** Present when it can be stopped part-way; called by Cancel. */
  cancel?: () => void;
  retry?: () => void;
  /** The window it was started from; by default the one the last click or key press went to. */
  origin?: string | null;
  /** What its outcome toast is about ("endpoint:<id>"): a later toast with the same key replaces it. */
  key?: string;
}): ClientTask {
  start();
  const id = `c${Date.now().toString(36)}${(++seq).toString(36)}`;
  if (opts.key) keyOp(id, opts.key);
  const now = Date.now();
  const base: Task = {
    id, kind: opts.kind, title: opts.title, nodes: opts.nodes ?? [], source: opts.source ?? "ui", cancellable: !!opts.cancel,
    state: "running", startedMs: now, finishedMs: null, progress: null, detail: null, rateBps: null, etaS: null,
    steps: [], log: [], summary: null, error: null, cancelRequested: false, retry: null, opId: null, client: true,
  };
  if (opts.cancel) {
    const cancel = opts.cancel;
    clientCancels.set(id, () => {
      change((t) => ({ ...t, cancelRequested: true }));
      cancel();
    });
  }
  if (opts.retry) clientRetries.set(id, opts.retry);
  tasks.set(id, base);
  adopt(base, opts.origin);
  publish();

  function change(f: (t: Task) => Task) {
    const t = tasks.get(id);
    if (!t || t.state !== "running") return;
    upsert(f(t));
  }
  function closeSteps(steps: TaskStep[], state: TaskStep["state"]): TaskStep[] {
    const at = Date.now();
    return steps.map((s) => (s.state === "running" ? { ...s, state, finishedMs: at } : s));
  }
  function end(state: Task["state"], message?: string) {
    const t = tasks.get(id);
    if (!t || t.state !== "running") return;
    clientCancels.delete(id);
    const stepState = state === "ok" ? "ok" : state === "cancelled" ? "skipped" : "failed";
    upsert({
      ...t, state, finishedMs: Date.now(), steps: closeSteps(t.steps, stepState),
      progress: state === "ok" ? 1 : t.progress, rateBps: null, etaS: null,
      summary: state === "failed" ? null : message ?? null,
      error: state === "failed" ? message ?? "Failed" : null,
    });
  }

  return {
    id,
    step: (label) => change((t) => ({ ...t, detail: label, steps: [...closeSteps(t.steps, "ok"), { label, state: "running", startedMs: Date.now(), finishedMs: null, detail: null }] })),
    progress: (frac, extra) => change((t) => ({
      ...t, progress: Math.max(0, Math.min(1, frac)), rateBps: extra?.rateBps ?? null, etaS: extra?.etaS ?? null, detail: extra?.detail ?? t.detail,
    })),
    log: (line) => change((t) => ({ ...t, log: [...t.log, ...line.split("\n")].slice(-400) })),
    finish: (ok, message) => end(ok ? "ok" : "failed", message),
    cancelled: (message) => end("cancelled", message),
  };
}

/**
 * Runs `fn` as a client task. Whatever it returns becomes the summary; a throw fails the
 * task with the error in plain words. Resolves to whether it succeeded.
 */
export async function runTask(
  title: string,
  fn: (t: ClientTask) => Promise<string | void>,
  opts: { kind?: string; nodes?: string[]; source?: string; origin?: string | null } = {},
): Promise<boolean> {
  // No Retry: replaying `fn` would skip whatever confirmation came before it.
  const t = startClientTask({ kind: opts.kind ?? "action", title, nodes: opts.nodes, source: opts.source, origin: opts.origin });
  try {
    const summary = await fn(t);
    t.finish(true, summary ?? undefined);
    return true;
  } catch (e) {
    t.finish(false, describeError(e).message);
    return false;
  }
}

/** Test helper. */
export function _resetTasksForTests() {
  tasks = new Map();
  list = [];
  announced.clear();
  waiters.clear();
  clientCancels.clear();
  clientRetries.clear();
  origins.clear();
  gestures = [];
  lastGesture = null;
  // The next use listens again (a test's mockTauri replaces the event bus).
  started = false;
  listeners.forEach((l) => l());
}
