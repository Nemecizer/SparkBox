// The always-on node agents (src-tauri/src/agent): which nodes keep watching
// while SparkBox is closed, who sends alerts right now, and whether "24/7" is
// proven end to end. `watchBadge` says it in one word for the top bar: "24/7"
// only when a phone channel exists, every agent is healthy and runs the current
// settings, nothing failed to deliver in the last hour, a test page from every
// node got through the channels each node runs now, and a dead-man's switch
// watches the watchers. `pushAgain` and `sendTestPage` are the badge's actions,
// shared with Settings › Notifications.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";
import { ADMIN_REFUSED, confirmUnlock, withAdmin, type AdminScope } from "./admin";
import { inTauri } from "./api";
import { showError } from "./errors";
import { confirmAction } from "./session";
import { openTasksWindow, waitTask } from "./tasks";
import { dismissToast, getToasts, toast } from "./toasts";

export type AgentRunState = "running" | "stopped" | "failed" | "not-installed" | "unknown";

export interface PushError {
  atMs: number;
  /** "sudo: a password is required". */
  message: string;
  attempts: number;
}

export interface TestCell {
  ok: boolean;
  error?: string | null;
  /** How long the channel took to accept it. */
  ms?: number | null;
  name?: string | null;
}

/** One node's last test page: a result per channel id. */
export interface TestRun {
  atMs: number;
  results: Record<string, TestCell>;
}

export interface AgentStatus {
  node: string;
  name: string;
  installed: boolean;
  state: AgentRunState;
  version: string | null;
  /** Unix ms of the agent's last heartbeat. */
  lastBeatMs: number | null;
  /** The oldest sample the agent holds (its history span starts here). */
  historyFromMs: number | null;
  firing: number;
  /** Unix ms the agent last delivered (or tried to deliver) an alert. */
  lastDeliveryMs: number | null;
  lastDeliveryOk: boolean | null;
  uptimeS: number | null;
  /** The agent serves Prometheus metrics on this port. */
  metricsPort: number | null;
  /** Why it isn't running, when it isn't. */
  error: string | null;
  /** Demo: a simulated agent. */
  simulated: boolean;
  /** The settings hash the agent reports, and the one SparkBox pushed. */
  configHash?: string | null;
  expectedHash?: string | null;
  /** It runs what SparkBox pushed last. */
  configCurrent?: boolean | null;
  /** The last push failed and is still pending. */
  pushError?: PushError | null;
  lastTest?: TestRun | null;
  lastCycleMs?: number | null;
  peersMissing?: string[];
  isolated?: string | null;
  leader?: boolean | null;
  endpointsProbed?: string[];
  heartbeatOkMs?: number | null;
  outboxPending?: number;
  lastDeliveryChannel?: string | null;
  quietTz?: string | null;
}

export interface WatchChannel {
  id: string;
  name: string;
  kind: string;
  /** A phone or team channel (not this Mac, not the dead-man's switch). */
  remote: boolean;
  macOnly: boolean;
  enabled: boolean;
  lastOkMs: number | null;
  lastError: string | null;
  lastErrorMs: number | null;
  /** Demo: simulated, never saved. */
  simulated: boolean;
}

export interface EndToEndCell {
  node: string;
  channel: string;
  channelName: string;
  ok: boolean;
  error: string | null;
  ms: number | null;
  atMs: number;
}

/** The latest test page from every node, on every phone channel. */
export interface EndToEnd {
  /** The oldest of the nodes' last tests. */
  testedAtMs: number | null;
  /** Every node reached every channel, after the last settings change. */
  ok: boolean;
  cells: EndToEndCell[];
}

export interface Deadman {
  configured: boolean;
  /** "healthchecks.io". */
  service: string | null;
  lastPingMs: number | null;
  /** The node whose agent pinged it last. */
  by: string | null;
}

/** What the agents cover while this Mac sleeps. */
export interface Watched {
  nodes: boolean;
  gpus: boolean;
  fabric: boolean;
  containers: boolean;
  /** Endpoints a healthy, current agent checks with one token every minute. */
  endpoints: string[];
  /** The leader agent checks the fabric switch's management address. */
  switch: boolean;
  /** Switches and routers nothing watches while this Mac sleeps. */
  unwatched: string[];
}

export interface Drift {
  node: string;
  name: string;
  reason: string;
}

export interface PushFailure {
  node: string;
  name: string;
  atMs: number;
  message: string;
  attempts: number;
}

export interface PagingFailure {
  atMs: number;
  channel: string;
  message: string;
  /** "mac" or the node whose agent it was. */
  by: string;
}

/** Which secrets an install or push copies to a node, by name. */
export interface SecretCopy {
  node: string;
  name: string;
  items: string[];
}

export interface WatchState {
  /** agents: every node has a healthy agent; partial: some do; mac-only: none. */
  mode: "agents" | "partial" | "mac-only";
  agents: AgentStatus[];
  /** Who pages the phone: the agents, this Mac, or nobody (no channel set up). */
  deliveryOwner: "agents" | "mac" | "none";
  channels?: WatchChannel[];
  endToEnd?: EndToEnd;
  /** When the channels or notification settings last changed. */
  changedMs?: number;
  drift?: Drift[];
  pushFailures?: PushFailure[];
  deadman?: Deadman;
  watched?: Watched;
  /** Silences in effect. */
  silences?: number;
  pagingFailure?: PagingFailure | null;
  secretCopies?: SecretCopy[];
  /** Demo: the agents, their channel and test are simulated. */
  demo?: boolean;
  /** The settings push agent_push_config started or joined. */
  pushTask?: string;
}

/** Where the agents keep the channels' secrets, said the same way everywhere. */
export const SECRET_DISCLOSURE = "Readable by the sparkbox-agent service, root and anyone in the docker group, which includes your login user.";
/** What to give the agents instead of your own passwords. */
export const SCOPED_CREDENTIALS = "Give the agents credentials that can only send: a publish-only ntfy token, an app password for email, a dedicated Pushover application.";

/** A heartbeat younger than this means the agent is watching. */
export const HEALTHY_BEAT_MS = 60_000;

const MAC_ONLY: WatchState = { mode: "mac-only", agents: [], deliveryOwner: "mac" };

let state: WatchState = MAC_ONLY;
const listeners = new Set<() => void>();
let started = false;

function set(s: WatchState) {
  state = s;
  listeners.forEach((l) => l());
}

function start() {
  if (started || !inTauri) return;
  started = true;
  listen<WatchState>("agent://state", (e) => set(e.payload)).catch(() => {});
  invoke<WatchState>("agent_status")
    .then(set)
    .catch(() => {
      // An older backend without agents: SparkBox is the only watcher.
    });
}

function subscribe(l: () => void) {
  start();
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useWatchState(): WatchState {
  return useSyncExternalStore(subscribe, () => state);
}

/** Whether an agent is installed and its heartbeat is fresh. */
export function agentHealthy(a: AgentStatus, now = Date.now()): boolean {
  return a.installed && a.state === "running" && a.lastBeatMs !== null && now - a.lastBeatMs < HEALTHY_BEAT_MS;
}

/** "20 s ago", "4 min ago", "2 h ago", "3 days ago". */
export function agoWords(ms: number, now = Date.now()): string {
  const s = Math.max(1, Math.round((now - ms) / 1000));
  if (s < 60) return `${s} s ago`;
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 2 * 86_400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86_400)} days ago`;
}

/** "10:42" on this Mac's clock. */
export function clockWords(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
}

/** "cell1", "cell1 and cell2", "cell1, cell2 and cell3". */
export function listWords(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** What a push failure says in a line: sudo's refusals read "sudo refused". */
export function pushWords(message: string): string {
  if (/password is required|incorrect password|password was rejected|not in the sudoers|not allowed to/i.test(message)) return "sudo refused";
  return message.length > 60 ? `${message.slice(0, 59)}…` : message;
}

/** Why an agent isn't watching: "no heartbeat for 3 min", "agent failed", "no agent". */
function downWords(a: AgentStatus, now: number): string {
  if (!a.installed) return "no agent";
  if (a.state === "failed") return "agent not answering";
  if (a.lastBeatMs !== null && now - a.lastBeatMs >= HEALTHY_BEAT_MS) return `no heartbeat for ${agoWords(a.lastBeatMs, now).replace(/ ago$/, "")}`;
  if (a.state === "unknown") return "couldn't be checked";
  return `agent ${a.state}`;
}

export interface WatchBadge {
  /** "24/7", "24/7 · untested", "This Mac only"… */
  label: string;
  tone: "neutral" | "warn";
  /** What it rests on, for the tooltip and Settings. */
  lines: string[];
  action?: { label: string; kind: "settings" | "push" | "test" };
}

/**
 * The top bar's watch badge, checked in this order: no agents, no phone channel, agents
 * not healthy, settings not on a node, paging failed, untested since the last change, and
 * only then "24/7" with what it covers.
 */
export function watchBadge(w: WatchState, now = Date.now()): WatchBadge {
  const installed = w.agents.filter((a) => a.installed);
  const settings = { label: "Set up…", kind: "settings" as const };
  // Agents that stopped answering, or a page that failed: nothing to set up, the rows say why.
  const details = { label: "Details…", kind: "settings" as const };
  if (!installed.length) {
    return { label: "This Mac only", tone: "neutral", lines: ["Alerts reach you only while SparkBox runs on this Mac."], action: settings };
  }
  const remote = (w.channels ?? []).filter((c) => c.remote && c.enabled);
  const fromNodes = remote.filter((c) => !c.macOnly);
  const hasPhone = w.channels ? fromNodes.length > 0 : w.deliveryOwner !== "none";
  if (!hasPhone) {
    const line = remote.length
      ? "Agents are watching, but every phone channel sends from this Mac only."
      : "Agents are watching, but no phone channel is set up. Add ntfy, Pushover, a webhook or email.";
    return { label: "Not paging", tone: "warn", lines: [line], action: settings };
  }
  const down = w.agents.filter((a) => !agentHealthy(a, now));
  if (down.length) {
    const healthy = w.agents.length - down.length;
    return {
      label: healthy ? `24/7 on ${healthy} of ${w.agents.length}` : "Agents not answering",
      tone: "warn",
      lines: [`Watched only while this Mac is awake: ${down.map((a) => `${a.name} (${downWords(a, now)})`).join(", ")}.`],
      action: details,
    };
  }
  const drift = w.drift ?? installed.filter((a) => a.configCurrent === false).map((a) => ({ node: a.node, name: a.name, reason: a.pushError ? `push failed: ${a.pushError.message}` : "has older settings" }));
  if (drift.length) {
    const names = drift.map((d) => d.name);
    const lines = drift.map((d) => {
      const fail = w.pushFailures?.find((p) => p.node === d.node) ?? w.agents.find((a) => a.node === d.node)?.pushError ?? null;
      const why = fail ? `push failed ${clockWords(fail.atMs)}: ${pushWords(fail.message)}` : d.reason;
      return `${d.name} has old alert settings (${why}). SparkBox pages for ${d.name} itself while this Mac is awake.`;
    });
    return { label: `24/7 · settings not on ${listWords(names)}`, tone: "warn", lines, action: { label: "Push again", kind: "push" } };
  }
  const failedCells = (w.endToEnd?.cells ?? []).filter((c) => !c.ok && c.atMs >= (w.changedMs ?? 0));
  const failure = w.pagingFailure && now - w.pagingFailure.atMs < 3_600_000 ? w.pagingFailure : null;
  if (failure || failedCells.length) {
    const lines = failure
      ? [failure.channel ? `${failure.channel} failed ${clockWords(failure.atMs)}: ${failure.message}` : `${w.agents.find((a) => a.node === failure.by)?.name ?? failure.by}: ${failure.message}`]
      : failedCells.map((c) => `The test page from ${w.agents.find((a) => a.node === c.node)?.name ?? c.node} didn't reach ${c.channelName}${c.error ? `: ${c.error}` : ""}`);
    return { label: "24/7 · paging failed", tone: "warn", lines, action: details };
  }
  const e2e = w.endToEnd;
  if (!e2e?.ok) {
    const line = !e2e?.testedAtMs
      ? "No test page has gone out from every node yet."
      : `The channels the agents send through changed after the last test page (${agoWords(e2e.testedAtMs, now)}).`;
    return { label: "24/7 · untested", tone: "warn", lines: [line], action: { label: "Send a test page…", kind: "test" } };
  }
  const via = [...new Set(e2e.cells.filter((c) => c.ok).map((c) => c.channelName))];
  const lines = [`${installed.length} agent${installed.length === 1 ? "" : "s"} on current settings · test page ${agoWords(e2e.testedAtMs ?? now, now)} via ${listWords(via)}`];
  const watched = w.watched;
  if (watched) {
    const what = [watched.nodes && "nodes", watched.gpus && "GPUs", watched.fabric && "fabric", watched.switch && "the fabric switch", watched.containers && "containers"].filter(Boolean) as string[];
    const endpoints = watched.endpoints.length ? `, ${watched.endpoints.join(", ")} (one-token check every minute)` : "";
    lines.push(`Watching: ${what.join(", ")}${endpoints}`);
    if (watched.unwatched.length) lines.push(`Not watched while this Mac sleeps: ${watched.unwatched.join(", ")}`);
  }
  const d = w.deadman;
  if (d?.configured) {
    lines.push(d.lastPingMs ? `Dead-man's switch: ${d.service}, pinged by ${d.by} ${agoWords(d.lastPingMs, now)}` : `Dead-man's switch: ${d.service}, not pinged yet`);
  }
  if (w.silences) lines.push(`${w.silences} silence${w.silences === 1 ? "" : "s"} active`);
  if (w.demo) lines.push("Demo: the agents, their phone channel, the dead-man's switch and the test page are simulated.");
  if (d && !d.configured) {
    // Without one, a house that loses power or internet, or every agent at once, pages nobody.
    return {
      label: "24/7 · no dead-man's switch",
      tone: "warn",
      lines: ["If the power or the internet goes, or every agent stops at once, nothing pages you. A dead-man's switch (healthchecks.io, Uptime Kuma) pages when the agents' pings stop.", ...lines],
      action: { label: "Set up…", kind: "settings" },
    };
  }
  return { label: "24/7", tone: "neutral", lines };
}

/** "24/7" when it's proven end to end (the badge is a neutral "24/7"), else null: for "watched 24/7". */
export function watchLabel(w: WatchState, now = Date.now()): string | null {
  const b = watchBadge(w, now);
  return b.tone === "neutral" && b.label === "24/7" ? b.label : null;
}

/**
 * The tone of a node's agent row: amber for what the matching alerts are (an agent not
 * answering, old settings, a failed push are warnings); neutral otherwise.
 */
export function agentTone(a: AgentStatus, now = Date.now()): "neutral" | "warn" {
  if (!a.installed) return "neutral";
  return !agentHealthy(a, now) || a.configCurrent === false || !!a.pushError ? "warn" : "neutral";
}

export const agentApi = {
  status: () => invoke<WatchState>("agent_status"),
  install: (nodes: string[]) => invoke<string>("agent_install", { nodes }),
  uninstall: (nodes: string[]) => invoke<string>("agent_uninstall", { nodes }),
  /** Push the current settings again (to `nodes`, or every node with an agent). */
  pushConfig: (nodes?: string[]) => invoke<WatchState>("agent_push_config", { nodes: nodes ?? null }),
  /** Each node's agent sends one test through each phone channel (at most once a minute). */
  testPage: () => invoke<EndToEnd>("agent_test_page"),
  backfillNow: () => invoke<{ points: number; alerts: number }>("agent_backfill_now"),
};

/** Applies a state returned by a command right away. */
export function applyWatchState(s: WatchState) {
  set(s);
}

/** The one toast a settings push ends with. */
export const PUSH_TOAST = "agent-push";

/** What Push again asks the owner first, in the unlock sheet. */
export const PUSH_WORDS = "push the alert settings to the node agents";

/**
 * The Admin scope an install or push to `nodes` (every node when empty) needs: the Mac scope
 * when it copies one of your secrets with Live data (agent_push_config's gate), else the node
 * scope. Asking for it up front makes it one prompt.
 */
export function pushScope(w: WatchState, nodes?: string[]): AdminScope {
  return !w.demo && (w.secretCopies ?? []).some((c) => c.items.length && (!nodes?.length || nodes.includes(c.node))) ? "mac" : "node";
}

let pushing: Promise<void> | null = null;

/**
 * Push again: the current alert settings to `nodes` (every agent when empty). Single-flight:
 * a second click while a push runs joins it, and the backend joins a push task already
 * running. One toast when the task ends, keyed agent-push and to the task's op id ("Pushed to
 * 3 of 4" · "Failed on cell3: sudo: a password is required"); it takes the place of the task's
 * own notice, so it never reads success and then failure. Locked, it asks with the in-app
 * "Unlock Admin mode?" sheet first (confirmUnlock, Demo data too) and then Touch ID with Live
 * data, for the Mac scope at once when the push copies one of your secrets to the nodes (the
 * backend's rule), so there's one prompt. In look-only mode the sheet says why it can't.
 */
export function pushAgain(nodes?: string[]): Promise<void> {
  if (pushing) return pushing;
  pushing = (async () => {
    try {
      const scope = pushScope(state, nodes);
      if (!(await confirmUnlock(PUSH_WORDS, scope, "agent_push_config"))) return;
      const ws = await withAdmin(() => agentApi.pushConfig(nodes), { action: "agent_push_config", scope });
      applyWatchState(ws);
      if (!ws.pushTask) return;
      const t = await waitTask(ws.pushTask);
      const ok = t.state === "ok";
      const [title, ...rest] = (ok ? t.summary ?? "" : t.error ?? t.summary ?? "The push failed").split(" · ");
      // The task's own notice (tasks.ts) is up by now: this one takes its place.
      const theirs = getToasts().find((x) => x.title === t.title || x.title === `${t.title}: failed`);
      const before = getToasts().find((x) => x.key === PUSH_TOAST && x.id !== theirs?.id);
      if (before) dismissToast(before.id);
      toast({
        id: theirs?.id,
        key: PUSH_TOAST,
        opId: t.opId ?? t.id,
        kind: ok ? "success" : "error",
        title: title || "Pushed the alert settings",
        body: rest.length ? rest.join(" · ") : ok ? "Each agent runs the new settings." : undefined,
        actions: ok ? undefined : [{ label: "Details", run: () => openTasksWindow(t.id) }],
      });
      agentApi.status().then(applyWatchState, () => {});
    } catch (e) {
      if (e !== ADMIN_REFUSED) showError(e, { action: "Push the alert settings" });
    }
  })().finally(() => {
    pushing = null;
  });
  return pushing;
}

/** The one confirmation a test page asks, wherever it's started. */
export function testPageConfirm(w: WatchState): { title: string; body: string; confirmLabel: string } {
  const n = w.agents.filter((a) => a.installed).length;
  const phones = (w.channels ?? []).filter((c) => c.remote && c.enabled && !c.macOnly).length;
  const total = n * phones;
  return {
    title: "Send a test page from every node?",
    body: `Each node's agent sends one test through each phone channel, so you know a page reaches you while this Mac sleeps: ${n} node${n === 1 ? "" : "s"} × ${phones} channel${phones === 1 ? "" : "s"} = ${total} message${total === 1 ? "" : "s"}.`,
    confirmLabel: "Send test pages",
  };
}

/**
 * Send a test page from every node: confirms (testPageConfirm), unlocks, sends, and says what
 * arrived. Resolves to the result, or null when called off. `onFailed` is the toast's
 * button when some didn't arrive.
 */
export async function sendTestPage(w: WatchState, onFailed?: { label: string; run: () => void }): Promise<EndToEnd | null> {
  if (!(await confirmAction({ ...testPageConfirm(w), dedupeKey: "watch:test" }))) return null;
  try {
    const r = await withAdmin(() => agentApi.testPage(), { action: "agent_test_page" });
    const bad = r.cells.filter((c) => !c.ok);
    const nodes = new Set(r.cells.map((c) => c.node)).size;
    toast(bad.length
      ? { kind: "error", key: "watch:test", title: `${bad.length} of ${r.cells.length} test pages didn't arrive`, body: bad.map((c) => `${c.node} → ${c.channelName}${c.error ? `: ${c.error}` : ""}`).join("\n"), actions: onFailed ? [onFailed] : undefined }
      : { kind: "success", key: "watch:test", title: `Test pages arrived from ${nodes} node${nodes === 1 ? "" : "s"}`, body: `Via ${[...new Set(r.cells.map((c) => c.channelName))].join(", ")}.` });
    agentApi.status().then(applyWatchState, () => {});
    return r;
  } catch (e) {
    if (e !== ADMIN_REFUSED) showError(e, { action: "Send test pages" });
    return null;
  }
}
