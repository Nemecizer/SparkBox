// Admin mode as the backend enforces it (src-tauri/src/admin.rs): the padlock state, the
// unlock prompt, and the warnings before it relocks. Two scopes: "node" (changes a node, a
// switch or a model: Touch ID or the Mac password with Live data, nothing in Demo) and "mac"
// (changes state on this Mac — Keychain, VPN, server, providers, inventory: always Touch ID or
// the password, with Demo data too). In a plain browser (mock harness) it's simulated locally.
import { useSyncExternalStore } from "react";
import { commandError, inTauri } from "./api";
import { opsApi, type AdminScope, type AdminState, type SbError } from "./opsApi";
import { alertInfo, confirmAction } from "./session";
import { dismissToast, toast } from "./toasts";

export type { AdminScope, AdminState };

const WARN_MS = 60_000;
/** The backend's NEEDS_CONFIRM: a Keychain write confirms every time, even within the Mac scope's 5 minutes. */
const NEEDS_CONFIRM = "changes a saved password or key";
const initial: AdminState = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: false };

let state: AdminState = initial;
const listeners = new Set<() => void>();
const changeListeners = new Set<(next: AdminState, prev: AdminState) => void>();
let started = false;
let warnToast: string | null = null;
/** The next relock was asked for (Lock button), so it needs no notice. */
let lockingOnPurpose = false;

function set(next: AdminState) {
  const prev = state;
  state = next;
  listeners.forEach((l) => l());
  changeListeners.forEach((cb) => cb(next, prev));
  if (prev.unlocked && !next.unlocked) {
    clearWarning();
    if (!lockingOnPurpose) {
      toast({ kind: "info", title: "Admin mode locked", body: next.lockReason ?? `No changes for ${next.idleMinutes} minutes, so SparkBox is read-only again.` });
    }
    lockingOnPurpose = false;
  }
  if (next.unlocked && next.expiresAtMs && next.expiresAtMs - Date.now() > (next.warnBeforeMs ?? WARN_MS)) clearWarning();
}

function clearWarning() {
  if (warnToast) dismissToast(warnToast);
  warnToast = null;
}

function tick() {
  const s = state;
  if (!s.unlocked || !s.expiresAtMs) return;
  const left = s.expiresAtMs - Date.now();
  if (!inTauri && left <= 0) {
    set({ ...s, unlocked: false, authenticated: false, expiresAtMs: null, unlockedAtMs: null, macExpiresAtMs: null });
    return;
  }
  if (!inTauri && s.authenticated && s.macExpiresAtMs && s.macExpiresAtMs <= Date.now()) {
    set({ ...s, authenticated: false, macExpiresAtMs: null });
  }
  if (left <= (s.warnBeforeMs ?? WARN_MS) && left > 0 && !warnToast) {
    warnToast = toast({
      kind: "info",
      title: "Admin mode locks in 1 min",
      body: "With Live data, open shells stop taking keystrokes when it locks. They stay connected.",
      durationMs: 0,
      actions: [{ label: "Keep unlocked", run: () => void keepUnlocked() }],
    });
  }
}

function start() {
  if (started || typeof window === "undefined") return;
  started = true;
  if (inTauri) {
    read();
    opsApi.onAdminState(set).catch(() => {});
  }
  window.setInterval(tick, 1000);
}

/** Whether the backend's state has been read yet (a read at module load can come too early). */
let synced = false;
function read() {
  opsApi.adminState().then((s) => {
    synced = true;
    set(s);
  }, () => {});
}

function subscribe(l: () => void) {
  start();
  // Code that runs at load (terminals.ts) can start this before the browser preview's
  // handlers are in; the first window to subscribe then reads it again.
  if (inTauri && !synced && !listeners.size) read();
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useAdminState(): AdminState {
  return useSyncExternalStore(subscribe, () => state);
}

/** True while changes are allowed. */
export function useAdmin(): boolean {
  return useAdminState().unlocked;
}

/** Whether `s` allows changes in `scope`: Mac scope also needs Touch ID or the password in the last 5 minutes. */
export function unlockedFor(scope: AdminScope, s: AdminState = adminState()): boolean {
  if (!s.unlocked) return false;
  if (scope === "node") return true;
  return !!s.authenticated && (s.macExpiresAtMs == null || s.macExpiresAtMs > Date.now());
}

export function isAdminUnlocked(): boolean {
  start();
  return state.unlocked;
}

export function adminState(): AdminState {
  start();
  return state;
}

/** Calls `cb` on every change (e.g. to drop privileged sessions on relock). Returns an unsubscribe. */
export function onAdminChange(cb: (next: AdminState, prev: AdminState) => void): () => void {
  start();
  changeListeners.add(cb);
  return () => changeListeners.delete(cb);
}

/** What the look-only sheet says (the backend's refusal is the same sentence with a colon). */
export const LOOK_ONLY_TEXT = "SparkBox is in look-only mode. It starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date.";

/** The top bar padlock's words: "Admin", "Look only" or "Read-only". */
export function padlockText(s: AdminState = adminState()): "Admin" | "Look only" | "Read-only" {
  if (s.lookOnly) return "Look only";
  return s.unlocked ? "Admin" : "Read-only";
}

/** Look-only mode is on: nothing unlocks, and nothing asks for Touch ID. */
export function isLookOnly(s: AdminState = adminState()): boolean {
  return !!s.lookOnly;
}

/** The tooltip of an action that needs Admin while it's locked: it stays enabled, and a click asks to unlock first. */
export const NEEDS_ADMIN_TIP = "needs Admin (click to unlock)";
/** The tooltip of every change action in look-only mode, which is disabled: nothing in look-only promises an unlock. */
export const LOOK_ONLY_TIP = "Look-only · turn off in Settings › Security & privacy";

/**
 * The one rule for an action that needs Admin (P1.43), wherever it's shown: its own label and
 * icon, a trailing lock and a tooltip. "locked": enabled, NEEDS_ADMIN_TIP, a click goes through
 * confirmUnlock. "lookOnly": disabled, LOOK_ONLY_TIP. null: it can run now.
 */
export function lockOf(needsUnlock: boolean | undefined, s: AdminState = adminState()): "locked" | "lookOnly" | null {
  if (!needsUnlock) return null;
  return s.lookOnly ? "lookOnly" : "locked";
}

/** The way out of look-only, for the sheet: Settings, or a restart without SPARKBOX_LOOK_ONLY=1. */
function wayOut(s: AdminState): string {
  return s.lookOnlyForced
    ? "SPARKBOX_LOOK_ONLY=1 turned it on for this run: quit SparkBox and start it without it."
    : "Turn it off in Settings › Security & privacy (it asks for Touch ID or your Mac password).";
}

/** LOOK_ONLY_TIP, unless SPARKBOX_LOOK_ONLY=1 set it: the Settings switch can't turn that off. */
export function lookOnlyTip(s: AdminState = adminState()): string {
  return s.lookOnlyForced ? `Look-only · ${wayOut(s)}` : LOOK_ONLY_TIP;
}

/** The look-only sheet: one OK button, no prompt behind it. It names what was blocked (`what`, plain words) and the way out. */
export function showLookOnly(what?: string): Promise<void> {
  const blocked = what ? `SparkBox can't ${what} in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date.` : LOOK_ONLY_TEXT;
  return alertInfo({ title: "Look only", body: `${blocked} ${wayOut(state)}` });
}

/** The words for a command key in the unlock sheet ("to restart, shut down or wake a node"); words pass through. */
const ACTION_WORDS: Record<string, string> = {
  node_power: "restart, shut down or wake a node",
  fleet_start: "run a command on your nodes",
  terminal_open: "open a shell on a node",
  terminal_authorize: "keep typing in a locked shell",
  files_transfer: "copy these files",
  files_write: "save this file",
  files_mac: "change files on this Mac",
  save_inventory: "change the cluster's nodes and switches",
  firewalla_label_set: "label a port on the Switch X",
  firewalla_msp_set_domain: "change where the Firewalla token is sent",
  data_source_set: "switch between Demo and Live data",
  security_set: "change the security settings",
  tunnel_open: "open a web page on a node",
  // ── p1r3 ──
  updates_hold_preset: "pin or release the GPU stack on a node",
  container_test_run: "start a test container on a node",
  // ── p1r5 ──
  node_probe: "test a node's connection settings",
  discovery_start: "survey the selected local /24 network",
  discovery_verify: "verify one selected device through SSH",
  providers_scan: "look for model servers on your nodes",
  switch_trust: "trust this switch's certificate",
  // ── p2r1 ──
  deployment_clean_leftovers: "remove containers an earlier load left on a node",
};

export function actionWords(action?: string): string {
  if (!action) return "make changes";
  if (ACTION_WORDS[action]) return ACTION_WORDS[action];
  action = action.replace(/…$/, "");
  // A command key that isn't in the table ("container_action") reads as a generic change.
  if (/^[a-z0-9]+(_[a-z0-9]+)+$/.test(action)) return "make this change";
  return action.charAt(0).toLowerCase() + action.slice(1);
}

/**
 * The in-app sheet before any unlock: "Unlock Admin mode?" with what it's for and what macOS
 * will ask. In look-only mode it's the look-only notice instead. Resolves true for Unlock….
 */
async function askUnlock(what: string, scope: AdminScope): Promise<boolean> {
  if (state.lookOnly) {
    await showLookOnly(what);
    return false;
  }
  const prompts = scope === "mac" || state.requiresAuth;
  return confirmAction({
    title: "Unlock Admin mode?",
    body: `To ${what}, SparkBox needs Admin mode. ${prompts ? "It asks for Touch ID or your Mac password." : "With Demo data it needs no password."} Admin locks again after ${state.idleMinutes} minutes idle.`,
    confirmLabel: "Unlock…",
    cancelLabel: "Cancel",
    dedupeKey: "unlock-sheet",
  });
}

/**
 * Unlocks for `scope` the way every locked control should: the in-app sheet first ("To copy
 * node-launch.sh to Documents, SparkBox needs Admin mode…"), and only after Unlock… the
 * backend (and with it Touch ID or the password). `action` is plain words; `key` names the
 * command for the OS prompt's own words. In look-only mode it shows that notice and resolves
 * false. Resolves true at once when already unlocked for `scope`.
 */
export async function confirmUnlock(action: string, scope: AdminScope = "node", key?: string): Promise<boolean> {
  start();
  if (state.lookOnly) {
    await showLookOnly(actionWords(action));
    return false;
  }
  if (unlockedFor(scope, state)) return true;
  if (!(await askUnlock(actionWords(action), scope))) return false;
  return requestAdmin(key ?? action, scope, { asked: true });
}

/**
 * Unlocks Admin mode for `scope`. Node scope asks for Touch ID or the Mac password when the data
 * is Live; Mac scope always asks (`admin_authenticate`), with Demo data too. `action` names the
 * command that asked ("vpn_set_endpoint"): the backend picks the prompt's words from its own
 * table, so free text here never reaches the prompt. Resolves to whether it's unlocked.
 *
 * A call for an action that would make macOS prompt shows the in-app sheet first (a locked
 * toolbar button through runTool, a palette verb, an error's Unlock), unless the caller already
 * asked (`asked`: confirmUnlock's sheet, or withAdmin behind a Save or a confirmed dialog). The
 * padlock itself (no action) goes straight on: clicking it is the question. In look-only mode it
 * never reaches the backend.
 */
export async function requestAdmin(action?: string, scope: AdminScope = "node", opts: { fresh?: boolean; asked?: boolean } = {}): Promise<boolean> {
  start();
  if (state.lookOnly) {
    await showLookOnly(action ? actionWords(action) : undefined);
    return false;
  }
  if (!opts.fresh && unlockedFor(scope, state)) return true;
  const prompts = scope === "mac" || state.requiresAuth;
  const locked = scope === "mac" ? !unlockedFor("mac", state) : !state.unlocked;
  if (action && prompts && locked && !opts.asked && !(await askUnlock(actionWords(action), scope))) return false;
  if (!inTauri) {
    const mac = scope === "mac" ? Date.now() + 5 * 60_000 : state.macExpiresAtMs;
    set({ ...state, unlocked: true, authenticated: scope === "mac" || !!state.authenticated, macExpiresAtMs: mac ?? null, expiresAtMs: Date.now() + state.idleMinutes * 60_000, unlockedAtMs: state.unlockedAtMs ?? Date.now() });
    return true;
  }
  try {
    set(scope === "mac" ? await opsApi.adminAuthenticate(action, !!opts.fresh) : await opsApi.adminUnlock(action, "node"));
    return true;
  } catch (e) {
    const err = e as Partial<SbError>;
    if (err?.kind === "lookOnly") {
      // Look-only came on meanwhile (the state event is on its way): the sheet, never a prompt.
      set({ ...state, lookOnly: true, unlocked: false });
      await showLookOnly(action ? actionWords(action) : undefined);
      return false;
    }
    if (err?.kind !== "cancelled") {
      toast({ kind: "error", title: "Admin mode stays locked", body: (err?.message ?? String(e)).replace(/^Admin mode stays locked\.\s*/, "") });
    }
    return false;
  }
}

function errorText(e: unknown): string {
  const text = typeof e === "string" ? e : e instanceof Error ? e.message : (e as SbError)?.message ?? "";
  return typeof text === "string" ? text : "";
}

/** The backend refused because the Mac scope needs Touch ID or the password (a Demo unlock, or its 5 minutes are up). */
export function isNeedsAuth(e: unknown): boolean {
  e = commandError(e);
  if (e && typeof e === "object" && (e as SbError).kind === "needs_auth") return true;
  const text = errorText(e);
  return text.includes("to change settings on this Mac") || text.includes(NEEDS_CONFIRM);
}

/** The backend refused a Keychain write that wasn't confirmed for itself just now. */
export function isNeedsConfirm(e: unknown): boolean {
  return errorText(commandError(e)).includes(NEEDS_CONFIRM);
}

/** The backend refused because SparkBox is in look-only mode. */
export function isLookOnlyRefusal(e: unknown): boolean {
  e = commandError(e);
  if (e && typeof e === "object" && (e as SbError).kind === "lookOnly") return true;
  return errorText(e).includes("look-only mode");
}

/** The backend refused because Admin mode is locked (it relocked meanwhile). */
export function isLocked(e: unknown): boolean {
  e = commandError(e);
  if (e && typeof e === "object" && (e as SbError).kind === "locked") return true;
  return errorText(e).toLowerCase().includes("admin mode is locked");
}

/** What `withAdmin` rejects with when the prompt was cancelled or refused. */
export const ADMIN_REFUSED: SbError = { kind: "cancelled", message: "Admin mode stays locked." };

/**
 * Runs `fn` with Admin mode unlocked for `scope`: unlocks first, and when the backend still
 * says no (it relocked meanwhile, or a Demo unlock doesn't cover the Mac) asks again and
 * retries once. Rejects with ADMIN_REFUSED when you cancel the prompt.
 */
export async function withAdmin<T>(fn: () => Promise<T>, opts: { scope?: AdminScope; action?: string; fresh?: boolean } = {}): Promise<T> {
  const scope = opts.scope ?? "node";
  // The caller's own button or dialog was the question (Save in Settings, a confirmed action):
  // straight to the backend. Locked toolbar buttons and palette verbs ask with the sheet first.
  if (!(await requestAdmin(opts.action, scope, { fresh: opts.fresh, asked: true }))) throw ADMIN_REFUSED;
  try {
    return await fn();
  } catch (e) {
    if (isLookOnlyRefusal(e)) throw commandError(e);
    const again = isNeedsAuth(e) ? "mac" : isLocked(e) ? scope : null;
    if (!again) throw commandError(e);
    // The Mac scope's 5 minutes ran out, or a Keychain write wants its own confirmation: ask again.
    if (!(await requestAdmin(opts.action, again, { fresh: again === "mac" && (isNeedsConfirm(e) || unlockedFor("mac")), asked: true }))) throw ADMIN_REFUSED;
    return await fn();
  }
}

/** Pushes the relock back by the idle time. Never unlocks: too late is too late. */
export async function keepUnlocked(): Promise<void> {
  clearWarning();
  if (!state.unlocked) return;
  if (!inTauri) {
    set({ ...state, expiresAtMs: Date.now() + state.idleMinutes * 60_000 });
    return;
  }
  try {
    set(await opsApi.adminExtend());
  } catch {
    // It relocked in the meantime; the state event says so.
  }
}

export async function lockAdmin(): Promise<void> {
  start();
  if (!state.unlocked) return;
  lockingOnPurpose = true;
  if (!inTauri) {
    set({ ...state, unlocked: false, authenticated: false, expiresAtMs: null, unlockedAtMs: null });
    return;
  }
  try {
    set(await opsApi.adminLock());
  } finally {
    lockingOnPurpose = false;
  }
}

/** Kept for existing callers: true unlocks (with the prompt), false locks. */
export function setAdmin(on: boolean): Promise<unknown> {
  return on ? requestAdmin() : lockAdmin();
}

/** The tooltip for controls that need Admin mode. */
export function lockedHint(what = "make changes", scope: AdminScope = "node"): string {
  if (state.lookOnly) return LOOK_ONLY_TIP;
  return state.requiresAuth || scope === "mac"
    ? `Unlock Admin mode (top bar) to ${what}. It asks for Touch ID or your Mac password.`
    : `Unlock Admin mode (top bar) to ${what}. Demo: no password needed.`;
}

/** Test helper: reset to locked. */
export function _resetAdminForTests(next: AdminState = initial) {
  state = next;
  warnToast = null;
  lockingOnPurpose = false;
  listeners.forEach((l) => l());
}
