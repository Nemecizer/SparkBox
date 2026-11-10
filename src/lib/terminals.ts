// Terminal tabs live here, outside React, so a tab keeps its session and scrollback while
// its window is minimized or re-rendered. The Terminal window shows them; other windows open
// new ones with openTerminal().
//
// Each tab is a small state machine (connecting → open → closed, and disposed once closed
// for good): a tab disposed while it was still connecting closes its session the moment the
// backend answers. Output is acknowledged as xterm draws it (terminal_ack): that paces what the
// backend sends, never what it reads from the node. A paused log tab holds what arrived and
// stops acknowledging; the backend keeps the newest 2 MiB and the tab shows how much it skipped.
// A shell tab is authorised once, when it opens in Admin mode (Touch ID with Live data; an
// explicit unlock prompt in Demo, never a silent one), and then keeps taking keystrokes
// through Admin mode's idle relock. It locks when the owner locks (the top bar, the screen,
// sleep, quit) or nobody types in it for Settings › Terminal's idle time; the backend says so
// with a `locked` event, warns a minute before an idle lock (`idleWarning`), and refuses input
// itself. A keystroke while locked isn't sent: the tab flashes and says so. The Locked banner
// authenticates and lets the same session type again. Log and journal tabs never take input,
// so broadcast and Ctrl-C can't reach them. A log tab whose stream ends says so in one line:
// "— End of log · glm53 exited 2 days ago —"; it follows again only if its container still runs.
// The last session's tabs and the recent sessions are kept per data source (dataSource.ts), so
// Live's quick-connect never offers a Demo session.
import { useSyncExternalStore } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import { adminState, confirmUnlock, isLocked, onAdminChange, requestAdmin } from "./admin";
import { currentSource, migrateLegacy, onSourceChange, scopedKey, type Source } from "./dataSource";
import { fmtBytes, fmtRelative } from "./format";
import { api, invoke } from "./api";
import { registerCommands } from "./commands";
import { confirmAction } from "./session";
import { getCluster } from "./store";
import { onThemeChange } from "./theme";
import { toast } from "./toasts";
import type { ClusterSnapshot, Container, TermEvent, TermTarget } from "./types";
import { metaSequence, OptionTracker, terminalShortcut, type TermShortcut } from "../windows/terminal/keys";
import { fontStack, getTermPrefs, onTermPrefs, setTermPrefs } from "../windows/terminal/prefs";
import { terminalTheme } from "../windows/terminal/theme";

/** A shell may start in a folder (Files › Open terminal here); a log tab from a point in time. */
export type TermTargetEx =
  | TermTarget
  | { kind: "shell"; cwd: string }
  | { kind: "containerLogs"; container: string; since?: LogSince }
  | { kind: "journal"; unit: string; since?: LogSince };
/** How far back a log tab starts (none: the last few hundred lines). */
export type LogSince = "10m" | "1h" | "6h" | "24h" | "boot";
export type TabPhase = "connecting" | "open" | "closed" | "disposed";
/** The old name of TabPhase, kept for existing callers. */
export type TabStatus = TabPhase;

export interface TabDescriptor {
  node: string;
  target: TermTargetEx;
  /** A name the user gave the tab. */
  title?: string;
}

export interface TermTab {
  key: string;
  node: string;
  /** The data source it was opened under; only that source's session list keeps it. */
  source?: Source;
  target: TermTargetEx;
  /** Log and journal tabs: read-only, never in broadcast. */
  readOnly: boolean;
  /** What the tab is called when nothing renamed it: node, container or unit. */
  baseTitle: string;
  /** Set by double-click rename; wins over titles the shell sets. */
  customTitle: string | null;
  /** Set by the shell (OSC 0/2), e.g. "sparkuser@cell1: ~/models". */
  oscTitle: string | null;
  phase: TabPhase;
  /** The old name of phase, kept for existing callers. */
  status: TabPhase;
  /** Backend session id while open. */
  id?: string;
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  /** Receives broadcast keystrokes (when broadcast is on). */
  inBroadcast: boolean;
  /** Output arrived while another tab was showing. */
  activity: boolean;
  bell: boolean;
  lastOutputMs: number;
  connectedMs: number | null;
  /** How the last session ended. */
  exitCode: number | null | undefined;
  exitMessage: string | null;
  /** Waiting to reconnect after the node dropped. */
  reconnect: { attempt: number; atMs: number; needsAdmin: boolean } | null;
  /** The backend stopped this shell's input (the owner locked, or nobody typed for its idle time). */
  locked: boolean;
  /** Why it locked, until it's authorized again. */
  lockReason: string | null;
  /** When a keystroke was last refused because the tab is locked (it flashes, and says so). */
  refusedAtMs: number | null;
  /** "This shell locks in 60 s: …", until something is typed or it locks. */
  idleWarning: string | null;
  /** Log tabs: output is held (not drawn) while paused. */
  paused: boolean;
  held: number;
  /** Output the backend dropped for this tab since it last resumed (or started), and why. */
  skipped: { bytes: number; whilePaused: boolean } | null;
  /** Log tabs: lines don't wrap (the tab scrolls sideways). */
  wrap: boolean;
}

interface State {
  tabs: TermTab[];
  active: string | null;
  broadcast: boolean;
  /** Tabs from the last session, offered for reopening (never reconnected on their own). */
  restorable: TabDescriptor[];
  /** The quick-connect picker (⌘T) is showing. */
  picker: boolean;
  findOpen: boolean;
  /** The old font size field, kept for existing callers; Settings › Terminal sets it now. */
  fontSize: number;
}

const SESSION_KEY = "sparkbox.terminal.session";
const RECENT_KEY = "sparkbox.terminal.recent";
const RECENT_MAX = 8;
// Both keys are kept per data source: scopedKey(SESSION_KEY) is "sparkbox.terminal.session:live".
const RUNNING_MS = 5000;
const ACK_BATCH = 32 * 1024;
const BACKOFF_S = [1, 2, 4, 8, 15, 30];
const MAX_ATTEMPTS = 12;

// Stores from before sources were kept apart hold Demo sessions (Live was first used Oct 29).
migrateLegacy(SESSION_KEY);
migrateLegacy(RECENT_KEY);

function loadSession(): TabDescriptor[] {
  try {
    const v = JSON.parse(localStorage.getItem(scopedKey(SESSION_KEY)) ?? "[]");
    return Array.isArray(v) ? v.filter((d) => d && typeof d.node === "string" && d.target && typeof d.target.kind === "string") : [];
  } catch {
    return [];
  }
}

let state: State = { tabs: [], active: null, broadcast: false, restorable: loadSession(), picker: false, findOpen: false, fontSize: getTermPrefs().fontSize };
const listeners = new Set<() => void>();

// Another source: its own last session to offer (the open tabs stay until closed, saved under theirs).
onSourceChange(() => {
  state = { ...state, restorable: state.tabs.some((t) => t.source === currentSource()) ? [] : loadSession() };
  listeners.forEach((l) => l());
});

// Per-tab bookkeeping that isn't shown.
interface Private {
  generation: number;
  unackedLocal: number;
  ackTimer: number | null;
  pendingAck: number;
  heldChunks: Uint8Array[];
  reconnectTimer: number | null;
  tracker: OptionTracker;
  swallowKeypress: boolean;
  userClosed: boolean;
  pasteHook: ((e: ClipboardEvent) => void) | null;
  pasting: boolean;
  /** When the tab last left Pause (a skip reported just after belongs to the pause). */
  resumedAtMs: number;
}
const priv = new Map<string, Private>();

function set(patch: Partial<State>) {
  const tabsChanged = patch.tabs !== undefined && patch.tabs !== state.tabs;
  state = { ...state, ...patch };
  if (tabsChanged) saveSession();
  listeners.forEach((l) => l());
}

/** Re-render after a tab's own fields change. */
function touch() {
  state = { ...state, tabs: [...state.tabs] };
  listeners.forEach((l) => l());
}

function phase(tab: TermTab, p: TabPhase) {
  tab.phase = p;
  tab.status = p;
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useTerminals(): State {
  return useSyncExternalStore(subscribe, () => state);
}

/** The open terminal tabs, in order. */
export function useTerminalTabs(): TermTab[] {
  return useSyncExternalStore(subscribe, () => state.tabs);
}

export function getTerminals(): State {
  return state;
}

/** Sessions that are open or connecting (for the Terminal window's close guard and the dock). */
export function liveSessionCount(): number {
  return state.tabs.filter((t) => t.phase === "open" || t.phase === "connecting").length;
}

export const isRunning = (t: TermTab, now = Date.now()) => t.phase === "open" && now - t.lastOutputMs < RUNNING_MS;

/** "4 SSH sessions are open, 1 is running a command", "1 log stream is open", or null when nothing would be lost. */
export function terminalGuardReason(now = Date.now()): string | null {
  const live = state.tabs.filter((t) => t.phase === "open" || t.phase === "connecting");
  if (!live.length) return null;
  const shells = live.filter((t) => !t.readOnly).length;
  const logs = live.length - shells;
  const parts = [
    shells ? (shells === 1 ? "1 SSH session" : `${shells} SSH sessions`) : "",
    logs ? (logs === 1 ? "1 log stream" : `${logs} log streams`) : "",
  ].filter(Boolean);
  const head = `${parts.join(" and ")} ${live.length === 1 ? "is" : "are"} open`;
  const running = live.filter((t) => isRunning(t, now)).length;
  return running ? `${head}, ${running} ${running === 1 ? "is" : "are"} running a command` : head;
}

/** A container log tab's container as the cluster shows it now (undefined: no longer there). */
export function logContainer(tab: Pick<TermTab, "node" | "target">, cluster: ClusterSnapshot | null = getCluster()): Container | undefined {
  if (tab.target.kind !== "containerLogs") return undefined;
  const c = (tab.target as { container: string }).container;
  return cluster?.nodes.find((n) => n.id === tab.node)?.containers.find((x) => x.name === c || x.id === c || (c.length >= 12 && x.id.startsWith(c.slice(0, 12))));
}

const RUNNING = new Set(["running", "restarting", "paused"]);

/** Whether a stopped log tab can follow again: its container still runs. */
export function canFollowAgain(tab: Pick<TermTab, "node" | "target">, cluster: ClusterSnapshot | null = getCluster()): boolean {
  const c = logContainer(tab, cluster);
  return !!c && RUNNING.has(c.state);
}

/**
 * The line that closes a log tab whose stream ended: "— End of log · glm53 exited 2 days ago —"
 * for a container that has stopped (docker logs prints what it wrote and ends), "— The log
 * stream ended · glm53 is still running —" when it runs on, and the reason when there is one.
 */
export function logEndLine(tab: Pick<TermTab, "node" | "target">, message: string | null, cluster: ClusterSnapshot | null = getCluster(), now = Date.now()): string {
  if (message) return `— ${message} —`;
  if (tab.target.kind === "journal") return "— End of journal —";
  const name = (tab.target as { container?: string }).container ?? "";
  const c = logContainer(tab, cluster);
  if (!c) return `— End of log · ${name.slice(0, 12)} is no longer on ${tab.node} —`;
  if (RUNNING.has(c.state)) return `— The log stream ended · ${c.name} is still running —`;
  const when = c.finishedAt > 0 ? fmtRelative(c.finishedAt * 1000, now) : "";
  return `— End of log · ${c.name} ${c.state === "created" ? "never started" : when ? `exited ${when}` : "has stopped"} —`;
}

function saveSession() {
  const source = currentSource();
  const mine = state.tabs.filter((t) => (t.source ?? source) === source);
  // Don't wipe the last session's list before the user has decided about it.
  if (!mine.length && state.restorable.length) return;
  try {
    const list: TabDescriptor[] = mine
      .filter((t) => t.phase !== "disposed")
      .map((t) => ({ node: t.node, target: t.target, ...(t.customTitle ? { title: t.customTitle } : {}) }));
    localStorage.setItem(scopedKey(SESSION_KEY, source), JSON.stringify(list));
  } catch {
    // Restoring tabs is a convenience.
  }
}

const isReadOnly = (t: TermTargetEx) => t.kind === "containerLogs" || t.kind === "journal";
/** Shells (anything you can type commands into) need Admin mode; following logs doesn't. */
export const needsAdmin = (t: TermTargetEx) => t.kind === "shell" || t.kind === "containerShell";

function containerName(node: string, idOrName: string): string {
  const c = getCluster()?.nodes.find((n) => n.id === node)?.containers.find((x) => x.name === idOrName || x.id === idOrName || x.id.startsWith(idOrName) || (idOrName.length >= 12 && idOrName.startsWith(x.id)));
  return c?.name ?? idOrName.slice(0, 12);
}

export function titleFor(node: string, target: TermTargetEx, name?: string): string {
  switch (target.kind) {
    case "shell":
      return "cwd" in target && target.cwd ? `${node} · ${target.cwd.replace(/\/+$/, "").split("/").pop() || "/"}` : node;
    case "containerShell":
      return `${name ?? containerName(node, target.container)} · ${node}`;
    case "containerLogs":
      return name ?? containerName(node, target.container);
    case "journal":
      return target.unit;
  }
}

/** What a tab shows: the user's name for it, else the shell's title, else node / container / unit. */
export const tabTitle = (t: Pick<TermTab, "customTitle" | "oscTitle" | "baseTitle">) => t.customTitle ?? t.oscTitle ?? t.baseTitle;

const decode = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));

function receivers(): TermTab[] {
  return state.tabs.filter((t) => t.inBroadcast && !t.readOnly && t.phase === "open" && !t.locked);
}

/** Tabs broadcast keystrokes would reach right now (0 when broadcast is off). */
export function broadcastCount(): number {
  return state.broadcast ? receivers().length : 0;
}

/** The sentence a locked tab shows when you type into it. */
export const REFUSED_INPUT = "Locked — authenticate to keep typing. Keystrokes typed while locked aren't sent.";

function refused(t: TermTab, reason?: string) {
  t.locked = true;
  t.lockReason ??= reason ?? null;
  t.refusedAtMs = Date.now();
  t.idleWarning = null;
  touch();
}

function send(tab: TermTab, data: string) {
  // Broadcast: the same keystrokes go to every tab that's in it (when this one is).
  const targets = state.broadcast && tab.inBroadcast && !tab.readOnly ? receivers() : [tab];
  for (const t of targets) {
    if (t.phase !== "open" || !t.id || t.readOnly) continue;
    if (t.idleWarning) {
      t.idleWarning = null;
      touch();
    }
    // The backend refuses a locked shell's keystrokes too (it may know first): never silently.
    api.terminalInput(t.id, data).catch((e) => {
      if (isLocked(e)) refused(t);
    });
  }
}

function ack(tab: TermTab, bytes: number) {
  const p = priv.get(tab.key);
  if (!p) return;
  p.pendingAck += bytes;
  const flush = () => {
    p.ackTimer = null;
    if (!tab.id || !p.pendingAck) return;
    const n = p.pendingAck;
    p.pendingAck = 0;
    invoke("terminal_ack", { id: tab.id, bytes: n }).catch(() => {});
  };
  if (p.pendingAck >= ACK_BATCH) {
    if (p.ackTimer !== null) window.clearTimeout(p.ackTimer);
    flush();
  } else if (p.ackTimer === null) {
    p.ackTimer = window.setTimeout(flush, 16);
  }
}

function draw(tab: TermTab, bytes: Uint8Array) {
  tab.term.write(bytes, () => ack(tab, bytes.length));
}

function note(tab: TermTab, text: string) {
  tab.term.write(`\r\n\x1b[2m${text}\x1b[0m\r\n`);
}

/** The line a tab shows where output was dropped: "… 3.2 MiB skipped while paused …". */
export function skipMarker(bytes: number, whilePaused = true): string {
  const why = whilePaused ? "while paused" : "(more than the window could draw)";
  return `\r\n\x1b[2m… ${fmtBytes(bytes, { decimals: 1 })} skipped ${why} …\x1b[0m\r\n`;
}

function onEvent(tab: TermTab, generation: number, e: TermEvent) {
  const p = priv.get(tab.key);
  if (!p || p.generation !== generation || tab.phase === "disposed") return;
  if (e.kind === "skipped") {
    const whilePaused = tab.paused || Date.now() - p.resumedAtMs < 3000;
    const marker = new TextEncoder().encode(skipMarker(e.bytes, whilePaused));
    tab.skipped = { bytes: (tab.skipped?.bytes ?? 0) + e.bytes, whilePaused };
    touch();
    // In order with the output around it: held while paused, drawn otherwise.
    if (tab.paused) p.heldChunks.push(marker);
    else tab.term.write(marker);
    return;
  }
  if (e.kind === "locked") {
    // Keystrokes still reach handleInput, which refuses them out loud.
    tab.locked = true;
    tab.lockReason = e.reason;
    tab.idleWarning = null;
    touch();
    return;
  }
  if (e.kind === "unlocked") {
    tab.locked = false;
    tab.lockReason = null;
    tab.refusedAtMs = null;
    touch();
    return;
  }
  if (e.kind === "idleWarning") {
    tab.idleWarning = e.text;
    touch();
    return;
  }
  if (e.kind === "data") {
    const bytes = decode(e.data);
    tab.lastOutputMs = Date.now();
    if (state.active !== tab.key && !tab.activity) {
      tab.activity = true;
      touch();
    }
    if (tab.paused) {
      p.heldChunks.push(bytes);
      tab.held += bytes.length;
      touch();
    } else {
      draw(tab, bytes);
    }
    return;
  }
  const wasOpen = tab.phase === "open";
  phase(tab, "closed");
  tab.id = undefined;
  tab.exitCode = e.code;
  tab.exitMessage = e.message;
  const dropped = !p.userClosed && e.message !== null && (e.code === null || e.code === 255);
  if (dropped) {
    note(tab, `[${e.message}]`);
    scheduleReconnect(tab);
  } else if (tab.readOnly) {
    // Whatever was held while paused comes first, then the line that ends the log.
    if (tab.paused) {
      for (const c of p.heldChunks.splice(0)) draw(tab, c);
      tab.paused = false;
      tab.held = 0;
    }
    note(tab, logEndLine(tab, e.message));
  } else {
    const why = e.message ?? (e.code === null ? "closed" : `exited with code ${e.code}`);
    note(tab, `[${why}. Press Return to reconnect]`);
  }
  if (!wasOpen && !dropped) tab.reconnect = null;
  touch();
}

function connect(tab: TermTab) {
  const p = priv.get(tab.key);
  if (!p || tab.phase === "disposed") return;
  const generation = ++p.generation;
  p.userClosed = false;
  phase(tab, "connecting");
  tab.id = undefined;
  touch();
  const { cols, rows } = tab.term;
  api
    .terminalOpen(tab.node, tab.target as TermTarget, cols, rows, (e) => onEvent(tab, generation, e))
    .then((id) => {
      // Closed (or reconnected) while this was connecting: end the session it opened.
      if (tab.phase === "disposed" || p.generation !== generation) {
        api.terminalClose(id).catch(() => {});
        return;
      }
      if (tab.phase !== "connecting") {
        // It already ended (e.g. a command that exits at once).
        api.terminalClose(id).catch(() => {});
        return;
      }
      tab.id = id;
      phase(tab, "open");
      if (tab.reconnect && tab.connectedMs !== null) note(tab, "[Reconnected]");
      tab.connectedMs = Date.now();
      tab.reconnect = null;
      if (tab.term.cols !== cols || tab.term.rows !== rows) api.terminalResize(id, tab.term.cols, tab.term.rows).catch(() => {});
      if (p.pendingAck) ack(tab, 0);
      touch();
    })
    .catch((err) => {
      if (tab.phase === "disposed" || p.generation !== generation) return;
      phase(tab, "closed");
      const msg = String(err?.message ?? err).replace(/^Error: /, "");
      tab.exitMessage = msg;
      if (tab.reconnect) {
        tab.term.write(`\x1b[2m[${msg}]\x1b[0m\r\n`);
        scheduleReconnect(tab);
      } else {
        tab.term.write(`\x1b[31m${msg}\x1b[0m\r\n\x1b[2m[Press Return to try again]\x1b[0m\r\n`);
      }
      touch();
    });
}

function scheduleReconnect(tab: TermTab) {
  const p = priv.get(tab.key);
  if (!p) return;
  const attempt = (tab.reconnect?.attempt ?? 0) + 1;
  if (attempt > MAX_ATTEMPTS) {
    tab.reconnect = null;
    note(tab, "[Stopped trying to reconnect. Press Return or use Reconnect]");
    touch();
    return;
  }
  const delay = BACKOFF_S[Math.min(attempt - 1, BACKOFF_S.length - 1)] * 1000;
  tab.reconnect = { attempt, atMs: Date.now() + delay, needsAdmin: false };
  if (p.reconnectTimer !== null) window.clearTimeout(p.reconnectTimer);
  p.reconnectTimer = window.setTimeout(() => tryReconnect(tab), delay);
  touch();
}

function tryReconnect(tab: TermTab) {
  const p = priv.get(tab.key);
  if (!p || tab.phase === "disposed" || !tab.reconnect) return;
  p.reconnectTimer = null;
  if (needsAdmin(tab.target) && !adminState().unlocked) {
    // Picked up again when Admin mode unlocks.
    tab.reconnect = { ...tab.reconnect, needsAdmin: true };
    touch();
    return;
  }
  const node = getCluster()?.nodes.find((n) => n.id === tab.node);
  if (node && node.status !== "online") {
    scheduleReconnect(tab);
    return;
  }
  connect(tab);
}

function handleInput(tab: TermTab, data: string) {
  if (tab.readOnly) return;
  if (tab.phase === "closed") {
    if (data === "\r") reconnect(tab.key);
    return;
  }
  if (tab.locked) {
    refused(tab);
    return;
  }
  send(tab, data);
}

function runShortcut(sc: TermShortcut, tab: TermTab) {
  switch (sc) {
    case "next-tab":
      return cycleTab(1);
    case "prev-tab":
      return cycleTab(-1);
    case "clear-scrollback":
      return tab.term.clear();
    case "new-tab":
      return set({ picker: true });
    case "find":
      return set({ findOpen: true });
    case "find-next":
    case "find-prev":
      return window.dispatchEvent(new CustomEvent("sparkbox:terminal-find", { detail: sc === "find-next" ? 1 : -1 }));
  }
}

function applyPrefs(tab: TermTab) {
  const p = getTermPrefs();
  const o = tab.term.options;
  o.fontFamily = fontStack(p.font);
  o.fontSize = p.fontSize;
  o.lineHeight = p.lineHeight;
  o.cursorStyle = p.cursorStyle;
  o.cursorBlink = p.cursorBlink;
  o.scrollback = p.scrollback;
  o.theme = terminalTheme(p.theme);
}

export interface RecentSession extends TabDescriptor {
  atMs: number;
}

/** Sessions opened lately with the current data source (newest first), for the quick-connect list. */
export function recentSessions(): RecentSession[] {
  try {
    const v = JSON.parse(localStorage.getItem(scopedKey(RECENT_KEY)) ?? "[]");
    return Array.isArray(v) ? v.filter((d) => d && typeof d.node === "string" && d.target?.kind) : [];
  } catch {
    return [];
  }
}

function recordRecent(d: TabDescriptor) {
  const same = (a: TabDescriptor) => a.node === d.node && JSON.stringify(a.target) === JSON.stringify(d.target);
  const list = [{ ...d, atMs: Date.now() }, ...recentSessions().filter((r) => !same(r))].slice(0, RECENT_MAX);
  try {
    localStorage.setItem(scopedKey(RECENT_KEY), JSON.stringify(list));
  } catch {
    // A convenience only.
  }
}

/** The node the last shell was opened on (the + button opens another there). */
export function lastShellNode(): string | null {
  return recentSessions().find((r) => r.target.kind === "shell")?.node ?? null;
}

let counter = 0;

/** Asks the app to show the Terminal window (handled by the shell's window manager). */
export function showTerminalWindow() {
  window.dispatchEvent(new CustomEvent("sparkbox:open-window", { detail: { kind: "terminal", params: {} } }));
}

/**
 * Opens a new tab, connects it and shows the Terminal window. `opts.title` names it
 * (Containers passes the container's name). Returns the tab's key.
 */
export function openTerminal(node: string, target: TermTargetEx = { kind: "shell" }, opts: { title?: string; activate?: boolean; show?: boolean } = {}): string {
  if (target.kind === "containerLogs") {
    const wanted = logContainer({ node, target })?.id ?? target.container;
    const existing = state.tabs.find((t) => t.node === node && t.source === currentSource()
      && t.target.kind === "containerLogs" && (logContainer(t)?.id ?? t.target.container) === wanted);
    if (existing) {
      if (opts.activate !== false) setActive(existing.key);
      if (opts.show !== false) showTerminalWindow();
      return existing.key;
    }
  }
  const prefs = getTermPrefs();
  const readOnly = isReadOnly(target);
  const term = new Terminal({
    fontFamily: fontStack(prefs.font),
    fontSize: prefs.fontSize,
    lineHeight: prefs.lineHeight,
    theme: terminalTheme(prefs.theme),
    cursorBlink: prefs.cursorBlink,
    cursorStyle: prefs.cursorStyle,
    scrollback: prefs.scrollback,
    allowProposedApi: true,
    macOptionIsMeta: false,
    macOptionClickForcesSelection: true,
    disableStdin: readOnly,
    rightClickSelectsWord: true,
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  term.loadAddon(new WebLinksAddon((_e, uri) => { openUrl(uri).catch(() => {}); }));
  const key = `tab${++counter}`;
  const waits = needsAdmin(target) && !adminState().unlocked;
  const tab: TermTab = {
    key, node, source: currentSource(), target, readOnly, baseTitle: titleFor(node, target, opts.title), customTitle: null, oscTitle: null,
    phase: "connecting", status: "connecting", term, fit, search,
    inBroadcast: !readOnly, activity: false, bell: false, lastOutputMs: 0, connectedMs: null, exitCode: undefined, exitMessage: null,
    reconnect: null, locked: false, lockReason: null, refusedAtMs: null, idleWarning: null, paused: false, held: 0, skipped: null, wrap: true,
  };
  priv.set(key, {
    generation: 0, unackedLocal: 0, ackTimer: null, pendingAck: 0, heldChunks: [], reconnectTimer: null,
    tracker: new OptionTracker(), swallowKeypress: false, userClosed: false, pasteHook: null, pasting: false, resumedAtMs: 0,
  });
  const p = priv.get(key)!;
  term.onData((data) => handleInput(tab, data));
  term.onResize(({ cols, rows }) => {
    if (tab.id) api.terminalResize(tab.id, cols, rows).catch(() => {});
  });
  term.onTitleChange((t) => {
    tab.oscTitle = t.trim() || null;
    touch();
  });
  term.onBell(() => {
    if (state.active !== tab.key) {
      tab.bell = true;
      touch();
    }
  });
  term.onSelectionChange(() => {
    if (getTermPrefs().copyOnSelect && term.hasSelection()) navigator.clipboard?.writeText(term.getSelection()).catch(() => {});
  });
  term.attachCustomKeyEventHandler((e) => {
    p.tracker.observe(e);
    if (e.type === "keypress" && p.swallowKeypress) {
      p.swallowKeypress = false;
      return false;
    }
    const sc = terminalShortcut(e);
    if (sc) {
      e.preventDefault();
      runShortcut(sc, tab);
      return false;
    }
    const meta = metaSequence(e, getTermPrefs().optionKey, p.tracker.side);
    if (meta !== null) {
      e.preventDefault();
      p.swallowKeypress = true;
      handleInput(tab, meta);
      return false;
    }
    return true;
  });
  const restoring = state.restorable.length > 0 && opts.activate === false;
  set({
    tabs: [...state.tabs, tab],
    active: opts.activate === false && state.active ? state.active : key,
    restorable: restoring ? state.restorable : [],
    picker: false,
  });
  if (waits) {
    // A shell needs Admin mode: ask for it (out loud, with Demo data too), and connect the moment
    // it's unlocked. The tab's banner says so meanwhile; nothing is written into the terminal.
    phase(tab, "closed");
    tab.reconnect = { attempt: 0, atMs: Date.now(), needsAdmin: true };
    void unlockForShell(node);
  } else {
    connect(tab);
  }
  recordRecent({ node, target, ...(opts.title ? { title: opts.title } : {}) });
  if (opts.show !== false) showTerminalWindow();
  return key;
}

/**
 * Unlocks Admin mode for a shell, never silently: the in-app sheet asks first ("To open a shell
 * on cell1, SparkBox needs Admin mode…"), then with Live data macOS asks for Touch ID or the
 * password. In look-only mode it says so and unlocks nothing. Resolves to whether it's unlocked.
 */
export async function unlockForShell(node: string): Promise<boolean> {
  if (adminState().unlocked) return true;
  return confirmUnlock(`Open a shell on ${node}`, "node", "terminal_open");
}

/**
 * The Locked banner's button: authenticates, then lets this same session take keystrokes
 * again. With Live data that's always Touch ID or the password, even while Admin mode is
 * unlocked; in Demo it's the unlock prompt when locked. Resolves to whether it did.
 */
export async function authenticateTab(key: string): Promise<boolean> {
  const tab = state.tabs.find((t) => t.key === key);
  if (!tab?.id) return false;
  const live = adminState().requiresAuth;
  const ok = live ? await requestAdmin("terminal_authorize", "mac", { fresh: true }) : await unlockForShell(tab.node);
  if (!ok) return false;
  try {
    await invoke("terminal_authorize", { id: tab.id });
  } catch (e) {
    note(tab, `[${String((e as { message?: string })?.message ?? e).replace(/^Error: /, "")}]`);
    return false;
  }
  tab.locked = false;
  tab.lockReason = null;
  tab.refusedAtMs = null;
  tab.idleWarning = null;
  touch();
  tab.term.focus();
  return true;
}

/** Hooks a tab's paste so multi-line pastes into a broadcast ask first (called once it's in the page). */
export function attachPasteGuard(tab: TermTab) {
  const p = priv.get(tab.key);
  const area = tab.term.textarea;
  if (!p || !area || p.pasteHook) return;
  p.pasteHook = (e: ClipboardEvent) => {
    if (p.pasting) return;
    const text = e.clipboardData?.getData("text/plain") ?? "";
    const lines = text.replace(/\r?\n$/, "").split(/\r?\n/).length;
    const n = state.broadcast && tab.inBroadcast ? receivers().length : 1;
    if (lines < 2 || n < 2) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    void confirmAction({
      title: `Paste ${lines} lines into ${n} shells?`,
      body: "Broadcast is on, so every line runs as a command in each of these shells.",
      details: text.length > 2000 ? `${text.slice(0, 2000)}…` : text,
      confirmLabel: "Paste",
      danger: true,
    }).then((ok) => {
      if (!ok) return;
      p.pasting = true;
      try {
        tab.term.paste(text);
      } finally {
        p.pasting = false;
      }
    });
  };
  area.addEventListener("paste", p.pasteHook, true);
}

export function closeTab(key: string) {
  const tab = state.tabs.find((t) => t.key === key);
  const p = priv.get(key);
  if (!tab || !p) return;
  p.userClosed = true;
  phase(tab, "disposed");
  if (p.reconnectTimer !== null) window.clearTimeout(p.reconnectTimer);
  if (p.ackTimer !== null) window.clearTimeout(p.ackTimer);
  if (tab.id) api.terminalClose(tab.id).catch(() => {});
  tab.id = undefined;
  if (p.pasteHook) tab.term.textarea?.removeEventListener("paste", p.pasteHook, true);
  tab.term.dispose();
  priv.delete(key);
  const i = state.tabs.indexOf(tab);
  const tabs = state.tabs.filter((t) => t.key !== key);
  set({ tabs, active: state.active === key ? (tabs[Math.min(i, tabs.length - 1)]?.key ?? null) : state.active });
}

export function closeAll() {
  for (const t of [...state.tabs]) closeTab(t.key);
}

export function setActive(key: string) {
  const tab = state.tabs.find((t) => t.key === key);
  if (tab) {
    tab.activity = false;
    tab.bell = false;
  }
  set({ active: key });
}

export function cycleTab(dir: 1 | -1) {
  const n = state.tabs.length;
  if (n < 2) return;
  const i = state.tabs.findIndex((t) => t.key === state.active);
  setActive(state.tabs[(i + dir + n) % n].key);
}

/** Drag to reorder: moves a tab to `index`. */
export function moveTab(key: string, index: number) {
  const from = state.tabs.findIndex((t) => t.key === key);
  if (from < 0) return;
  const tabs = [...state.tabs];
  const [t] = tabs.splice(from, 1);
  tabs.splice(Math.max(0, Math.min(tabs.length, index)), 0, t);
  set({ tabs });
}

/** Double-click rename; empty goes back to following the shell's title. */
export function renameTab(key: string, title: string | null) {
  const tab = state.tabs.find((t) => t.key === key);
  if (!tab) return;
  tab.customTitle = title?.trim() || null;
  set({ tabs: [...state.tabs] });
}

/** Broadcast on or off; the activity log notes it with the shells it reaches (never the keystrokes). */
export const setBroadcast = (on: boolean) => {
  set({ broadcast: on });
  const ids = receivers().map((t) => t.id).filter((id): id is string => !!id);
  invoke("terminal_broadcast", { on, ids }).catch(() => {});
};

export function toggleInBroadcast(key: string) {
  const tab = state.tabs.find((t) => t.key === key);
  if (tab && !tab.readOnly) {
    tab.inBroadcast = !tab.inBroadcast;
    touch();
  }
}

/** Reconnects a closed tab (Return in it, or the toolbar). Shells need Admin mode. */
export function reconnect(key: string) {
  const tab = state.tabs.find((t) => t.key === key);
  const p = priv.get(key);
  if (!tab || !p || tab.phase !== "closed") return;
  if (needsAdmin(tab.target) && !adminState().unlocked) {
    tab.reconnect = { attempt: 0, atMs: Date.now(), needsAdmin: true };
    touch();
    void unlockForShell(tab.node);
    return;
  }
  if (p.reconnectTimer !== null) window.clearTimeout(p.reconnectTimer);
  tab.reconnect = null;
  connect(tab);
}

/** Log tabs: start the follower again (it ended, or to catch up after a long pause). */
export function restartFollower(key: string) {
  const tab = state.tabs.find((t) => t.key === key);
  const p = priv.get(key);
  if (!tab || !p || !tab.readOnly) return;
  p.userClosed = true;
  if (tab.id) api.terminalClose(tab.id).catch(() => {});
  tab.term.reset();
  p.heldChunks = [];
  tab.held = 0;
  tab.paused = false;
  tab.skipped = null;
  connect(tab);
}

/** Log tabs: start following again from another point in time. */
export function setSince(key: string, since: LogSince | null) {
  const tab = state.tabs.find((t) => t.key === key);
  if (!tab || (tab.target.kind !== "containerLogs" && tab.target.kind !== "journal")) return;
  const { since: _old, ...rest } = tab.target as { since?: LogSince };
  tab.target = (since ? { ...rest, since } : rest) as TermTargetEx;
  restartFollower(key);
  set({ tabs: [...state.tabs] });
}

/** Log tabs: hold new output (Pause) or draw it and keep following (Follow). */
export function setPaused(key: string, paused: boolean) {
  const tab = state.tabs.find((t) => t.key === key);
  const p = priv.get(key);
  if (!tab || !p || !tab.readOnly || tab.paused === paused) return;
  tab.paused = paused;
  if (paused) tab.skipped = null;
  if (!paused) {
    p.resumedAtMs = Date.now();
    for (const chunk of p.heldChunks) draw(tab, chunk);
    p.heldChunks = [];
    tab.held = 0;
    tab.term.scrollToBottom();
  }
  touch();
}

export function setWrap(key: string, wrap: boolean) {
  const tab = state.tabs.find((t) => t.key === key);
  if (!tab) return;
  tab.wrap = wrap;
  touch();
}

/** A tab's scrollback and screen as plain text (for Save). */
export function tabText(key: string): string {
  const tab = state.tabs.find((t) => t.key === key);
  if (!tab) return "";
  const buf = tab.term.buffer.active;
  const lines: string[] = [];
  for (let i = 0; i < buf.length; i++) lines.push(buf.getLine(i)?.translateToString(true) ?? "");
  while (lines.length && !lines[lines.length - 1]) lines.pop();
  return `${lines.join("\n")}\n`;
}

export function clearScrollback(key: string) {
  state.tabs.find((t) => t.key === key)?.term.clear();
}

export function setPicker(open: boolean) {
  set({ picker: open });
}

export function setFindOpen(open: boolean) {
  set({ findOpen: open });
}

/** Kept for existing callers: changes the terminal font size setting. */
export function setFontSize(size: number) {
  setTermPrefs({ fontSize: size });
}

/** Reopens the last session's tabs (they connect now; they never did on their own). */
export function restoreSession() {
  const list = state.restorable;
  if (!list.length) return;
  set({ restorable: [] });
  let first: string | null = null;
  for (const d of list) {
    const key = openTerminal(d.node, d.target, { activate: false, show: false });
    if (d.title) renameTab(key, d.title);
    first ??= key;
  }
  if (first) setActive(first);
  showTerminalWindow();
}

export function dismissRestore() {
  set({ restorable: [] });
  saveSession();
}

/** Plain-language summary of the restorable tabs, e.g. "cell1, cell2 and 2 log tabs". */
export function restoreSummary(list: TabDescriptor[] = state.restorable): string {
  const shells = [...new Set(list.filter((d) => needsAdmin(d.target)).map((d) => d.node))];
  const logs = list.filter((d) => !needsAdmin(d.target)).length;
  const parts = [...shells];
  if (logs) parts.push(`${logs} log tab${logs === 1 ? "" : "s"}`);
  return parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}` : (parts[0] ?? "");
}

// The Terminal window closes every session when it closes. React's dev mode mounts
// components twice, so wait a moment and check the window really went away.
let mounts = 0;
export function windowMounted(): () => void {
  mounts++;
  return () => {
    mounts--;
    window.setTimeout(() => {
      if (mounts === 0) closeAll();
    }, 300);
  };
}

// Admin mode unlocked: shells that were waiting for it connect. A relock changes nothing here:
// open shells keep working, and the backend says when one locks (its `locked` event).
onAdminChange((next) => {
  if (!next.unlocked) return;
  for (const t of state.tabs) {
    if (needsAdmin(t.target) && t.reconnect?.needsAdmin) {
      t.reconnect = { ...t.reconnect, needsAdmin: false };
      tryReconnect(t);
    }
  }
  touch();
});

onTermPrefs(() => {
  for (const t of state.tabs) applyPrefs(t);
  set({ fontSize: getTermPrefs().fontSize });
});

if (typeof window !== "undefined") {
  onThemeChange(() => {
    const theme = terminalTheme(getTermPrefs().theme);
    for (const t of state.tabs) t.term.options.theme = theme;
    touch();
  });
}

registerCommands([
  {
    id: "term:quick-connect",
    title: "Quick connect: terminal on…",
    group: "Terminal",
    subtitle: "Pick a node, a recent session, a container's logs or a journal",
    keywords: ["shell", "ssh", "new tab", "logs", "journal"],
    run: () => {
      showTerminalWindow();
      set({ picker: true });
    },
  },
  {
    id: "term:reopen-tabs",
    title: "Reopen last session's terminal tabs",
    group: "Terminal",
    when: () => (state.restorable.length ? true : "Nothing to reopen"),
    run: restoreSession,
  },
]);

// Offer the last session's tabs once, without connecting anything.
if (import.meta.env.MODE !== "test" && state.restorable.length) {
  const n = state.restorable.length;
  window.setTimeout(() => {
    if (!state.restorable.length) return;
    toast({
      kind: "info",
      title: `Reopen ${n} terminal tab${n === 1 ? "" : "s"}?`,
      body: `From last time: ${restoreSummary()}. Nothing connects until you choose.`,
      durationMs: 20_000,
      actions: [{ label: "Reopen", run: restoreSession }],
    });
  }, 1500);
}

// Dev builds: let the screenshot script look inside.
if (import.meta.env.DEV) {
  (window as unknown as Record<string, unknown>).__terminals = {
    get: () => state,
    open: openTerminal,
    type: (key: string, text: string) => state.tabs.find((t) => t.key === key)?.term.input(text, true),
    setBroadcast,
    setActive,
    setPaused,
    setTermPrefs,
    /** Scrolls a tab so the first line containing `text` is at the top; returns its line or -1. */
    scrollTo: (key: string, text: string) => {
      const tab = state.tabs.find((t) => t.key === key);
      if (!tab) return -1;
      const buf = tab.term.buffer.active;
      for (let i = 0; i < buf.length; i++) {
        if (buf.getLine(i)?.translateToString(true).includes(text)) {
          tab.term.scrollToLine(Math.max(0, i - 3));
          return i;
        }
      }
      return -1;
    },
  };
}
