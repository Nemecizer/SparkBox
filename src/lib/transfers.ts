// The transfer queue: copies and moves between this Mac and the nodes, outside React so
// they keep going while the Files window is minimized or closed. Each transfer is checked
// first (sizes, free space, names already taken), waits for a conflict decision if needed,
// then queues: at most two run at once per route (Mac → cell1, cell1 → cell2…). Every one
// is mirrored as a task in the Tasks window, and the last 50 are kept as history, per data
// source: a Demo transfer never shows (or retries) in Live, and the other way round.
import { useSyncExternalStore } from "react";
import { errorText, filesApi, MAC, type Placed, type Policy, type Preflight, type TransferEvent, type TransferSpec } from "./filesApi";
import { fmtBytes, fmtDuration } from "./format";
import { adminState, confirmUnlock, unlockedFor, withAdmin, type AdminScope, type AdminState } from "./admin";
import { currentSource, migrateLegacy, onSourceChange, scopedKey, type Source } from "./dataSource";
import { startClientTask, type ClientTask } from "./tasks";
import { toast } from "./toasts";

export type TransferState = "checking" | "needsDecision" | "queued" | "running" | "done" | "failed" | "cancelled";

export interface Transfer {
  id: string;
  spec: TransferSpec;
  /** e.g. "3 items" or "config.json". */
  what: string;
  fromName: string;
  toName: string;
  state: TransferState;
  preflight: Preflight | null;
  done: number;
  total: number;
  files: number;
  filesTotal: number;
  /** The file in flight (from tar -v). */
  current: string | null;
  /** Smoothed (exponential moving average) bytes per second. */
  rateBps: number | null;
  etaS: number | null;
  createdMs: number;
  startedMs: number | null;
  finishedMs: number | null;
  /** The result in a sentence, or why it failed. */
  message: string | null;
  placed: Placed[] | null;
  /** The window that started it (its conflict sheet shows there). */
  origin: string | null;
  /** A staging folder it couldn't remove (SparkBox tries again with the node back and Admin unlocked). */
  leftover?: { place: string; path: string; bytes: number | null } | null;
  /** The data source it ran against; Retry is only offered in the same one. */
  source?: Source;
}

const MAX_PER_ROUTE = 2;
const HISTORY = 50;
const STORE_KEY = "sparkbox.transfers.v1";
const EMA_ALPHA = 0.3;

interface Runtime {
  backendId: string | null;
  stopRequested: boolean;
  task: ClientTask | null;
  last: { at: number; done: number } | null;
}

// Rows saved before history was kept per source came from Demo (Live was first used on Oct 29).
migrateLegacy<Transfer>(STORE_KEY, (t) => t?.source ?? "demo");
let items: Transfer[] = loadHistory(currentSource());
const runtime = new Map<string, Runtime>();
const listeners = new Set<() => void>();
const landed = new Set<(t: Transfer) => void>();
let seq = 0;

function loadHistory(source: Source): Transfer[] {
  try {
    const v = JSON.parse(localStorage.getItem(scopedKey(STORE_KEY, source)) ?? "[]");
    return Array.isArray(v) ? v.filter((t: Transfer) => t && t.spec && ["done", "failed", "cancelled"].includes(t.state)).map((t: Transfer) => ({ ...t, source })) : [];
  } catch {
    return [];
  }
}

function persist(source: Source = currentSource()) {
  try {
    const finished = items.filter((t) => isFinished(t) && (t.source ?? source) === source).slice(0, HISTORY);
    localStorage.setItem(scopedKey(STORE_KEY, source), JSON.stringify(finished));
  } catch {
    // History is a convenience.
  }
}

// Switching the data source swaps the history; transfers still running carry on and land in
// the history of the source they started in.
onSourceChange((next, prev) => {
  persist(prev);
  items = [...items.filter((t) => isActive(t)), ...loadHistory(next)];
  listeners.forEach((l) => l());
});

function publish() {
  // Newest first; history beyond 50 finished transfers drops off.
  let finished = 0;
  items = items.filter((t) => !isFinished(t) || ++finished <= HISTORY);
  listeners.forEach((l) => l());
}

function patch(id: string, p: Partial<Transfer>) {
  items = items.map((t) => (t.id === id ? { ...t, ...p } : t));
  publish();
}

const get = (id: string) => items.find((t) => t.id === id);

export const isFinished = (t: Pick<Transfer, "state">) => t.state === "done" || t.state === "failed" || t.state === "cancelled";
export const isActive = (t: Pick<Transfer, "state">) => !isFinished(t);
export const routeOf = (s: Pick<TransferSpec, "from" | "to">) => `${s.from}→${s.to}`;

/** ~/Downloads and the folders in it (where a copy from a node arrives like a download). */
export function isDownloads(dir: string): boolean {
  return /^(~|\/Users\/[^/]+)\/Downloads(\/|$)/.test(dir);
}

/**
 * The places whose Admin scope a transfer needs before it starts: the destination, and the
 * source when a move removes the originals. Landing on this Mac needs Mac scope, except a copy
 * from a node into ~/Downloads under new names: that arrives like a browser download
 * (quarantined, without execute bits) and needs none; its conflict choices may add the Mac
 * (`policyNeeds`). The backend checks the same (files/commands.rs).
 */
export function transferNeeds(spec: Pick<TransferSpec, "from" | "to" | "toDir" | "op" | "policies">): string[] {
  const out = new Set<string>();
  const download = spec.to === MAC && spec.from !== MAC && spec.op === "copy" && isDownloads(spec.toDir);
  if (!download) out.add(spec.to);
  if (spec.op === "move") out.add(spec.from);
  for (const p of policyNeeds(spec)) out.add(p);
  return [...out];
}

/** Replacing or merging changes what's already at the destination. */
export function policyNeeds(spec: Pick<TransferSpec, "to" | "policies">): string[] {
  return Object.values(spec.policies).some((p) => p !== "skip" && p !== "keepBoth") ? [spec.to] : [];
}

/** Paths in the home folder that hold keys and credentials (files/mac.rs SECRET_DIRS). */
export const SECRET_DIRS = [
  ".ssh", "Library/Keychains", ".gnupg", ".aws", ".config/gh", ".kube", ".config/gcloud", ".azure", ".netrc",
  ".git-credentials", ".cache/huggingface/token", ".docker/config.json",
];
/** The same on a node, home-relative, plus the two the owner keeps there (files/mac.rs NODE_SECRETS). */
export const NODE_SECRETS = [
  ".ssh", ".gnupg", ".aws", ".config/gh", ".config/gcloud", ".azure", ".kube", ".netrc", ".git-credentials",
  ".cache/huggingface/token", ".docker/config.json", "hf_token.txt", "glm53/api_key",
];
/** Why a copy or move of keys never starts: the backend's preflight refuses it, Touch ID or not (files/mac.rs SECRET_PATH). */
export const SECRET_REFUSED = "SparkBox won't copy a folder that may hold keys or credentials.";

/**
 * Whether copying `path` off this Mac reads keys or passwords: it is in a key folder, or one
 * is inside it (copying ~, ~/Library, ~/.config or /Users). Case doesn't matter, as on APFS.
 * The backend decides with symlinks resolved (files/mac.rs `holds_secrets`); this mirrors it.
 */
export function holdsSecrets(path: string, home = "~"): boolean {
  const p = path.replace(/\/+$/, "") || "/";
  const homeDir = (home !== "~" ? home : p.match(/^\/Users\/[^/]+/)?.[0] ?? "").replace(/\/+$/, "").toLowerCase();
  const lower = p.toLowerCase();
  let rel: string;
  if (p === "~") rel = "";
  else if (p.startsWith("~/")) rel = lower.slice(2);
  else if (homeDir && lower === homeDir) rel = "";
  else if (homeDir && lower.startsWith(`${homeDir}/`)) rel = lower.slice(homeDir.length + 1);
  // Outside the home folder: only / and /Users hold one.
  else return lower === "/" || lower === "/users";
  return rel === "" || SECRET_DIRS.some((d) => {
    const s = d.toLowerCase();
    return rel === s || rel.startsWith(`${s}/`) || s.startsWith(`${rel}/`);
  });
}

/**
 * Whether `path` on a node is a keys-and-credentials path or inside one. Lexical and
 * case-sensitive, like the backend's `is_node_secret`; a folder that only holds one isn't.
 */
export function isNodeSecret(path: string): boolean {
  const p = path.trim().replace(/\/+$/, "");
  const rel = p === "~" ? "" : p.startsWith("~/") ? p.slice(2) : (p.match(/^\/root\/(.+)$/)?.[1] ?? p.match(/^\/home\/[^/]+\/(.+)$/)?.[1]);
  return rel != null && NODE_SECRETS.some((s) => rel === s || rel.startsWith(`${s}/`));
}

export interface TransferGate {
  /** The scopes to unlock first; Mac scope covers node scope. Empty: nothing to unlock. */
  needs: AdminScope[];
  /** Why, in a sentence (the locked button's tooltip). */
  reason: string;
  /** Set when no unlock can help: the transfer never starts, and this says why. */
  refused?: string;
}

/**
 * What a copy or move needs unlocked, for the toolbar button, ⌘V and drag and drop alike (one
 * function, so they can't disagree). A copy from a node into ~/Downloads needs nothing: it
 * arrives quarantined, without execute bits, like a browser download. Landing anywhere else
 * on this Mac or moving files off it needs the Mac scope; landing on a node needs node scope.
 * The backend checks the same (files/commands.rs `gate_place`). A source that is or holds keys
 * is refused outright, with no unlock to ask for: the preflight refuses it whatever is unlocked
 * (files/commands.rs `preflight_refused`).
 */
export function transferGate(spec: Pick<TransferSpec, "from" | "to" | "dir" | "names" | "toDir" | "op" | "policies">, home = "~"): TransferGate {
  const join = (d: string, n: string) => `${d.replace(/\/+$/, "")}/${n}`;
  const secret = spec.names.some((n) => (spec.from === MAC ? holdsSecrets(join(spec.dir, n), home) : isNodeSecret(join(spec.dir, n))));
  if (secret) return { needs: [], reason: SECRET_REFUSED, refused: SECRET_REFUSED };
  const places = transferNeeds(spec);
  if (places.includes(MAC)) {
    const reason = spec.to === MAC
      ? `Copying into ${spec.toDir} changes this Mac, so it needs Admin mode with Touch ID or your Mac password. Copies into Downloads don't.`
      : "Moving files off this Mac removes them here, so it needs Admin mode with Touch ID or your Mac password.";
    return { needs: ["mac"], reason };
  }
  if (places.length) return { needs: ["node"], reason: `This changes files on ${places.find((p) => p !== MAC) ?? spec.to}, so it needs Admin mode.` };
  return { needs: [], reason: "A copy into Downloads arrives like a browser download (quarantined, without execute bits), so it needs no unlock." };
}

/** Whether the gate's scopes aren't all unlocked yet: the button shows a lock, and a click or drop asks first. */
export function transferLocked(gate: TransferGate, s: AdminState = adminState()): boolean {
  return gate.needs.some((scope) => !unlockedFor(scope, s));
}

/**
 * Asks for what `transferGate` says (the in-app sheet first, then Touch ID or the password)
 * and queues the transfer; resolves to its id, or null when the gate refused it or the unlock
 * was cancelled (then nothing is queued and nothing is asked of the backend). `words` is the
 * sheet's "copy node-launch.sh to Documents". The Files toolbar, ⌘V and drag and drop all start here.
 */
export async function startGatedTransfer(spec: TransferSpec, names: { fromName: string; toName: string; origin?: string }, words: string, home = "~"): Promise<string | null> {
  const gate = transferGate(spec, home);
  if (gate.refused) {
    toast({ kind: "error", title: spec.op === "move" ? "Not moved" : "Not copied", body: gate.refused });
    return null;
  }
  for (const scope of gate.needs) {
    if (!unlockedFor(scope) && !(await confirmUnlock(words, scope, "files_transfer"))) return null;
  }
  return enqueueTransfer(spec, names);
}

/** Retry is offered only for a finished transfer from the data source in use. */
export function canRetry(t: Pick<Transfer, "state" | "source">, source: Source = currentSource()): boolean {
  return isFinished(t) && t.state !== "done" && (t.source ?? source) === source;
}

export function useTransfers(): Transfer[] {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => items,
  );
}

export function getTransfers(): Transfer[] {
  return items;
}

/** Called when a transfer finishes successfully (the panes refresh the destination). */
export function onTransferLanded(cb: (t: Transfer) => void): () => void {
  landed.add(cb);
  return () => landed.delete(cb);
}

/** "1 transfer in progress (2.0 of 4.0 GiB)", for the Files window's close guard. */
export function transferGuardReason(): string | null {
  const active = items.filter(isActive);
  if (!active.length) return null;
  const done = active.reduce((s, t) => s + t.done, 0);
  const total = active.reduce((s, t) => s + (t.total || t.preflight?.bytes || 0), 0);
  const n = active.length;
  return `${n} transfer${n === 1 ? "" : "s"} in progress (${fmtBytes(done)} of ${fmtBytes(total)})`;
}

/** How the route is described: node to node goes through this Mac, and says so. */
export function routeLabel(t: Pick<Transfer, "spec">): string {
  const { from, to } = t.spec;
  if (from === to) return from === MAC ? "on this Mac" : "on the node";
  if (from !== MAC && to !== MAC) return "via this Mac";
  return "over SSH";
}

/**
 * Checks, then queues a copy or move. Names already taken at the destination wait for a
 * decision (the Files window shows the conflict sheet), unless `spec.policies` covers them.
 */
export function enqueueTransfer(spec: TransferSpec, names: { fromName: string; toName: string; origin?: string }): string {
  const id = `t${Date.now().toString(36)}${(++seq).toString(36)}`;
  const t: Transfer = {
    id, spec, what: spec.names.length === 1 ? spec.names[0] : `${spec.names.length} items`, fromName: names.fromName, toName: names.toName,
    origin: names.origin ?? null,
    state: "checking", preflight: null, done: 0, total: 0, files: 0, filesTotal: 0, current: null, rateBps: null, etaS: null,
    createdMs: Date.now(), startedMs: null, finishedMs: null, message: null, placed: null, source: currentSource(),
  };
  items = [t, ...items];
  runtime.set(id, { backendId: null, stopRequested: false, task: null, last: null });
  publish();
  void check(id);
  return id;
}

async function check(id: string) {
  const t = get(id);
  if (!t) return;
  let pre: Preflight;
  try {
    pre = await filesApi.preflight(t.spec);
  } catch (e) {
    return fail(id, errorText(e));
  }
  if (get(id)?.state !== "checking") return; // stopped meanwhile
  const incoming = pre.bytes - pre.conflicts.filter((c) => t.spec.policies[c.name] === "skip").reduce((s, c) => s + c.source.bytes, 0);
  if (pre.free !== null && t.spec.from !== t.spec.to && incoming > pre.free) {
    return fail(id, `Not enough space on ${t.toName}: this needs ${fmtBytes(incoming)} and ${fmtBytes(pre.free)} is free.`, pre);
  }
  const undecided = pre.conflicts.filter((c) => !t.spec.policies[c.name]);
  patch(id, { preflight: pre, total: pre.bytes, filesTotal: pre.entries, state: undecided.length ? "needsDecision" : "queued" });
  schedule();
}

/** The conflict sheet's answer: a policy per taken name, or null to call the transfer off. */
export function decideConflicts(id: string, policies: Record<string, Policy> | null) {
  const t = get(id);
  if (!t || t.state !== "needsDecision") return;
  if (!policies) {
    patch(id, { state: "cancelled", finishedMs: Date.now(), message: "Canceled before it started." });
    persist();
    return;
  }
  patch(id, { spec: { ...t.spec, policies: { ...t.spec.policies, ...policies } }, state: "queued" });
  schedule();
}

function schedule() {
  const running = new Map<string, number>();
  for (const t of items) if (t.state === "running") running.set(routeOf(t.spec), (running.get(routeOf(t.spec)) ?? 0) + 1);
  // Oldest first.
  for (const t of [...items].reverse()) {
    if (t.state !== "queued") continue;
    const r = routeOf(t.spec);
    if ((running.get(r) ?? 0) >= MAX_PER_ROUTE) continue;
    running.set(r, (running.get(r) ?? 0) + 1);
    void run(t.id);
  }
}

function summary(t: Transfer, placed: Placed[], bytes: number): string {
  const verb = t.spec.op === "move" ? "Moved" : "Copied";
  const copied = placed.filter((p) => p.action !== "skipped");
  const secs = t.startedMs ? (Date.now() - t.startedMs) / 1000 : 0;
  let s = `${verb} ${copied.length === 1 ? copied[0].as : `${copied.length} items`} (${fmtBytes(bytes)})${secs >= 1 ? ` in ${fmtDuration(secs)}` : ""}.`;
  const both = placed.filter((p) => p.action === "keptBoth");
  if (both.length) s += ` Kept both: ${both.map((p) => `“${p.as}”`).join(", ")}.`;
  const skipped = placed.filter((p) => p.action === "skipped").length;
  if (skipped) s += ` Skipped ${skipped}.`;
  return s;
}

async function run(id: string) {
  const t = get(id);
  const rt = runtime.get(id);
  if (!t || !rt) return;
  const nodes = [t.spec.from, t.spec.to].filter((p, i, a) => p !== MAC && a.indexOf(p) === i);
  rt.task = startClientTask({
    kind: "transfer",
    title: `${t.spec.op === "move" ? "Move" : "Copy"} ${t.what} to ${t.toName}`,
    nodes,
    source: "files",
    cancel: () => stopTransfer(id),
    retry: () => {
      retryTransfer(id);
    },
  });
  rt.task.step(`${t.fromName} → ${t.toName} (${routeLabel(t)})`);
  patch(id, { state: "running", startedMs: Date.now() });
  const onEvent = (e: TransferEvent) => {
    const cur = get(id);
    if (!cur) return;
    if (e.kind === "progress") {
      const now = Date.now();
      let rate = cur.rateBps;
      if (rt.last && now - rt.last.at >= 200) {
        const inst = Math.max(0, (e.done - rt.last.done) / ((now - rt.last.at) / 1000));
        rate = rate === null ? inst : EMA_ALPHA * inst + (1 - EMA_ALPHA) * rate;
        rt.last = { at: now, done: e.done };
      } else if (!rt.last) {
        rt.last = { at: now, done: e.done };
      }
      const eta = rate && rate > 0 ? Math.max(0, (e.total - e.done) / rate) : null;
      patch(id, { done: e.done, total: e.total, files: e.files, filesTotal: e.filesTotal, current: e.current, rateBps: rate, etaS: eta });
      rt.task?.progress(e.total ? e.done / e.total : 0, { rateBps: rate ?? undefined, etaS: eta ?? undefined, detail: e.current ?? undefined });
      return;
    }
    if (e.kind === "leftover") {
      patch(id, { leftover: { place: e.place, path: e.path, bytes: e.bytes } });
      const where = e.place === MAC ? "this Mac" : e.place;
      toast({
        kind: "error",
        title: `Left a ${e.bytes ? `${fmtBytes(e.bytes, { decimals: 1 })} ` : ""}staging folder on ${where}`,
        body: `SparkBox removes it once ${where} answers and Admin mode is unlocked, or now with Remove.`,
        actions: [{ label: "Remove", run: () => void removeLeftover(e.place, e.path) }],
      });
      return;
    }
    if (e.kind === "done") {
      const message = summary(cur, e.placed, e.bytes);
      patch(id, { state: "done", finishedMs: Date.now(), done: Math.max(cur.done, cur.total), files: e.files, placed: e.placed, message, current: null, etaS: null });
      rt.task?.finish(true, message);
      const landedT = get(id);
      if (landedT) landed.forEach((cb) => cb(landedT));
    } else if (e.kind === "failed") {
      patch(id, { state: "failed", finishedMs: Date.now(), message: e.message, current: null, etaS: null });
      rt.task?.finish(false, e.message);
    } else {
      const left = get(id)?.leftover;
      const message = left ? `Stopped. Nothing was put in place; a staging folder is left on ${left.place === MAC ? "this Mac" : left.place} until it can be removed.` : "Stopped. Nothing was left at the destination.";
      patch(id, { state: "cancelled", finishedMs: Date.now(), message, current: null, etaS: null });
      rt.task?.cancelled("Stopped");
    }
    runtime.delete(id);
    persist();
    schedule();
  };
  try {
    // Conflict choices that replace or merge on the Mac need the Mac scope now.
    const spec = get(id)!.spec;
    const start = () => filesApi.transfer(spec, onEvent);
    // (The window asked for everything else before it queued the transfer.)
    rt.backendId = policyNeeds(spec).includes(MAC) ? await withAdmin(start, { scope: "mac", action: "files_transfer" }) : await start();
    if (rt.stopRequested) await filesApi.cancel(rt.backendId);
  } catch (e) {
    fail(id, errorText(e));
    rt.task?.finish(false, errorText(e));
    runtime.delete(id);
    schedule();
  }
}

/** Removes a staging folder a transfer left behind (Files' and the toast's Remove). */
export async function removeLeftover(place: string, path: string): Promise<boolean> {
  try {
    await withAdmin(() => filesApi.removeLeftover(place, path), { scope: place === MAC ? "mac" : "node", action: "files_leftover_remove" });
    for (const t of items) if (t.leftover?.path === path) patch(t.id, { leftover: null });
    persist();
    toast({ kind: "success", title: "Staging folder removed" });
    return true;
  } catch (e) {
    toast({ kind: "error", title: "Couldn't remove the staging folder", body: errorText(e) });
    return false;
  }
}

function fail(id: string, message: string, preflight?: Preflight) {
  patch(id, { state: "failed", finishedMs: Date.now(), message, ...(preflight ? { preflight, total: preflight.bytes } : {}) });
  persist();
}

/** Stops a transfer; what had arrived is removed. The window confirms first above 1 GiB. */
export function stopTransfer(id: string) {
  const t = get(id);
  const rt = runtime.get(id);
  if (!t || isFinished(t)) return;
  if (t.state === "running" && rt) {
    rt.stopRequested = true;
    if (rt.backendId) void filesApi.cancel(rt.backendId);
    return;
  }
  runtime.delete(id);
  patch(id, { state: "cancelled", finishedMs: Date.now(), message: "Canceled before it started." });
  persist();
  schedule();
}

/** Runs a failed or stopped transfer again from the start (checking for conflicts afresh). */
export function retryTransfer(id: string): string | null {
  const t = get(id);
  if (!t || !isFinished(t) || !canRetry(t)) return null;
  return enqueueTransfer({ ...t.spec, policies: {} }, { fromName: t.fromName, toName: t.toName, origin: t.origin ?? undefined });
}

export function removeTransfer(id: string) {
  const t = get(id);
  if (!t || !isFinished(t)) return;
  items = items.filter((x) => x.id !== id);
  publish();
  persist();
}

export function clearFinishedTransfers() {
  items = items.filter(isActive);
  publish();
  persist();
}

/** "Idle · 3 finished, 1 failed": the one-line summary when nothing is running (null while busy or empty). */
export function idleSummary(list: Transfer[]): string | null {
  if (!list.length || list.some(isActive)) return null;
  const failed = list.filter((t) => t.state === "failed").length;
  const done = list.filter((t) => t.state === "done").length;
  const stopped = list.length - failed - done;
  const parts = [done && `${done} finished`, failed && `${failed} failed`, stopped && `${stopped} stopped`].filter(Boolean);
  return `No transfers running · ${parts.join(", ")}`;
}

/** Test helper. */
export function _resetTransfersForTests() {
  items = [];
  runtime.clear();
  publish();
}
