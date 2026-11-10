// Cluster health as judged by the Rust health engine (src-tauri/src/health):
// active alerts grouped into incidents, a level per node and switch, silences
// and the alert log. Every status dot in the app goes through displayStatus(),
// so a node with a failed unit shows amber everywhere, not green in one place
// and amber in another. Counts everywhere are incidents (useHealthSummary), and
// acknowledged, silenced, recovering and planned alerts never count.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";
import { inTauri } from "./api";
import type { LinkStatus, NodeSnapshot, SwitchSnapshot } from "./types";
import { openWindow } from "../wm/WindowManager";

/** info: shown in Alerts, never counted on a node or notified (an idle model). */
export type Severity = "info" | "warn" | "crit";
export type Level = "ok" | "warn" | "crit";
export type AlertState = "pending" | "firing" | "resolved";
/** Matches the notification switches in Settings › Notifications. */
export type AlertCategory = "node" | "disk" | "gpu" | "fabric" | "containers";

export interface AlertEntity {
  kind: "node" | "container" | "service" | "interface" | "disk" | "gpu" | "switch" | "port" | "endpoint" | "fabric" | "sensor";
  /** Unique within its kind on its node or switch: a container name, a unit, an interface. */
  id: string;
  node: string | null;
  switch: string | null;
  /** How the entity reads in a sentence, e.g. "nvidia-dgx-telemetry.service on cell1". */
  label: string;
}

export interface Alert {
  id: string;
  rule: string;
  category: AlertCategory;
  severity: Severity;
  state: AlertState;
  entity: AlertEntity;
  /** Reads after the entity's name: "Root disk 86% full". */
  message: string;
  /** Stands alone: "gx10-0000 disk 86%". */
  summary: string;
  detail: string | null;
  value: number | null;
  /** When the condition was first seen (unix ms). */
  sinceMs: number;
  firingSinceMs: number | null;
  resolvedMs: number | null;
  ackedMs: number | null;
  mutedUntilMs: number | null;
  /** False for information that's never notified. */
  notify?: boolean;
  // ── r3-incidents ──
  /** The incident it belongs to (its root key). */
  incidentId?: string | null;
  rootKey?: string | null;
  /** Recovering: nothing breaches any more; it clears at this time. */
  recoversAtMs?: number | null;
  /** Planned work: "cell3 restarting (SparkBox, 10:42)". */
  planned?: string | null;
  silenceId?: string | null;
  /** Counts toward badges, rows, the sidebar, the status bar and the top bar. */
  counted?: boolean;
  /** What it means, in a few words: "check the optic or cable". */
  cause?: string | null;
  /** Why it resolved, when not simply because the condition went: "No data", "Verified: …". */
  reason?: string | null;
  // ── r4-alerts ──
  /** At most 24 characters, for where a sentence doesn't fit: "GPU throttled", "Crash loop". */
  short?: string;
  /** It keeps coming back: raised again within 10 minutes of its resolve. */
  flapping?: FlapInfo | null;
}

/** How often an alert went bad in the last 30 minutes. */
export interface FlapInfo {
  count: number;
  sinceMs: number;
}

/** "Flapping · 4 times in 30 min" (health::FlapInfo::words). */
export function flapWords(f: FlapInfo, now = Date.now()): string {
  return `Flapping · ${f.count} times in ${Math.max(1, Math.floor((now - f.sinceMs) / 60_000))} min`;
}

/** An alert's short label (≤ 24 characters), from its rule; older backends: the summary's first words. */
export function alertShort(a: Alert): string {
  if (a.short) return a.short;
  const s = a.summary.replace(`${a.entity.node ?? ""} `, "");
  return s.length <= 24 ? s : `${s.slice(0, 23).trimEnd()}…`;
}

/** How an incident reads: firing counts; the others are quiet. */
export type IncidentState = "firing" | "recovering" | "planned" | "silenced" | "acknowledged" | "info";

/** Alerts that share a root cause (src-tauri/src/health/incident.rs). */
export interface Incident {
  /** The root key: "link:cell2/enP2p1s0f1np1", "node:cell3", "endpoint:…", "gpu:gx10-0000". */
  id: string;
  title: string;
  severity: Severity;
  state: IncidentState;
  /** Alert ids, the headline first. */
  children: string[];
  cause: string | null;
  sinceMs: number;
  /** "cell2 half 2", "cell3", "llama-3.3-70b on cell1–cell3". */
  affects: string;
  headline: string;
  counted: boolean;
  recoversAtMs: number | null;
  planned: string | null;
  /** The most flaps of any alert in it. */
  flapping?: FlapInfo | null;
}

export type SilenceUntil = { kind: "at"; ms: number } | { kind: "resolved" } | { kind: "never" };
/** The only durations a silence can have. */
export type SilenceDuration = "1h" | "1d" | "1w" | "resolved" | "never";

export interface Silence {
  id: string;
  matcher: { rule?: string | null; entity?: string | null; incident?: string | null };
  until: SilenceUntil;
  reason: string;
  createdMs: number;
  createdBy: string;
  source: "live" | "demo";
  /** What it silences: "Root disk on gx10-0000". */
  label: string;
  duration: SilenceDuration;
}

/** An acknowledged or silenced alert: one quiet line, never counted. */
export interface KnownIssue {
  alertId: string;
  incidentId: string;
  title: string;
  severity: Severity;
  why: "silenced" | "acknowledged";
  untilMs: number | null;
  reason: string | null;
}

/** An alert still confirming: "cell3 offline · confirming…". */
export interface PendingAlert {
  alertId: string;
  title: string;
  severity: Severity;
  sinceMs: number;
  firesAtMs: number;
}

export interface Issue {
  alertId: string;
  severity: Severity;
  summary: string;
  label: string;
}

export interface EntityHealth {
  level: Level;
  issues: Issue[];
}

export interface HealthState {
  tsMs: number;
  /** Pending and firing alerts, worst first. */
  alerts: Alert[];
  /** warn, crit and info count incidents; pending and muted count alerts. */
  summary: {
    info?: number;
    warn: number;
    crit: number;
    pending: number;
    /** Known issues (the round-2 name). */
    muted: number;
    recovering?: number;
    planned?: number;
    known?: KnownIssue[];
    pendingAlerts?: PendingAlert[];
    /** Counted alerts behind the incidents. */
    alerts?: number;
  };
  nodes: Record<string, EntityHealth>;
  switches: Record<string, EntityHealth>;
  /** When an alert last fired or resolved. */
  lastAlertMs: number | null;
  incidents?: Incident[];
  /** The silences of the data source in use. */
  silences?: Silence[];
  demo?: boolean;
}

export interface AlertEvent {
  tsMs: number;
  kind: "raised" | "escalated" | "eased" | "resolved" | "acked" | "muted" | "unmuted" | "silenced" | "unsilenced" | "silenceEnded";
  alert: Alert;
  /** Recorded by a node agent while SparkBox was closed. */
  away?: boolean;
  /** "agent:cell1" when a node agent evaluated it. */
  source?: string;
  /** From the Demo simulator (kept in Demo's own log, never sent). */
  demo?: boolean;
  incidentId?: string | null;
  incidentTitle?: string | null;
  /** For silences: "for 1 week: expanding disk Friday". */
  note?: string | null;
}

let state: HealthState | null = null;
const listeners = new Set<() => void>();
let started = false;

function set(s: HealthState) {
  state = s;
  listeners.forEach((l) => l());
  outside.forEach((l) => l(s));
}

const outside = new Set<(s: HealthState | null) => void>();

/** Follows health changes outside React (palette commands); returns an unsubscribe. */
export function subscribeHealth(l: (s: HealthState | null) => void): () => void {
  start();
  outside.add(l);
  return () => outside.delete(l);
}

/** Connects to the backend once; until it answers every hook returns null. */
function start() {
  if (started || !inTauri) return;
  started = true;
  followNotifications();
  listen<HealthState>("health://state", (e) => set(e.payload)).catch(() => {});
  invoke<HealthState>("health_get")
    .then((s) => {
      if (s && (!state || s.tsMs >= state.tsMs)) set(s);
    })
    .catch(() => {
      // An older backend without the health engine: indicators fall back to link status.
    });
}

/**
 * A click on a notification opens what it's about (`notify://clicked`). When SparkBox comes to
 * the front some other way after a notification, nothing opens by itself: the backend sends
 * `notify://pending` and the shell offers "Last notification · Open".
 */
function followNotifications() {
  listen<{ kind: string; params?: Record<string, string> }>("notify://clicked", ({ payload }) => {
    if (payload?.kind) openWindow(payload.kind, payload.params ?? {});
  }).catch(() => {});
  window.addEventListener("focus", () => {
    invoke("notify_take_pending").catch(() => {});
  });
}

function subscribe(l: () => void) {
  start();
  listeners.add(l);
  return () => listeners.delete(l);
}

const getState = () => state;

export function useHealth(): HealthState | null {
  return useSyncExternalStore(subscribe, getState);
}

/** The current health, for code outside React (e.g. displayStatus). */
export function getHealth(): HealthState | null {
  start();
  return state;
}

const OK: EntityHealth = { level: "ok", issues: [] };

export function useNodeHealth(id: string): EntityHealth | null {
  const h = useHealth();
  return h ? (h.nodes[id] ?? OK) : null;
}

export function useSwitchHealth(id: string): EntityHealth | null {
  const h = useHealth();
  return h ? (h.switches[id] ?? OK) : null;
}

/** What every count shows: incidents, never alerts; recovering, planned and known issues aside. */
export interface HealthSummary {
  crit: number;
  warn: number;
  recovering: number;
  planned: number;
  /** How many acknowledged and silenced alerts there are ("2 known issues"). */
  known: number;
  knownIssues: KnownIssue[];
  /** Alerts still confirming ("cell3 offline · confirming…"). */
  pending: PendingAlert[];
  /** Counted alerts behind the incidents, for tooltips. */
  alerts: number;
}

/** The summary of a health state (a pure function, for tests and code outside React). */
export function summarize(h: HealthState): HealthSummary {
  const s = h.summary;
  return {
    crit: s.crit,
    warn: s.warn,
    recovering: s.recovering ?? 0,
    planned: s.planned ?? 0,
    known: s.known?.length ?? s.muted ?? 0,
    knownIssues: s.known ?? [],
    pending: s.pendingAlerts ?? [],
    alerts: s.alerts ?? s.crit + s.warn,
  };
}

let summaryOf: { state: HealthState | null; value: HealthSummary | null } = { state: null, value: null };
const getSummary = () => {
  if (summaryOf.state !== state) summaryOf = { state, value: state ? summarize(state) : null };
  return summaryOf.value;
};

export function useHealthSummary(): HealthSummary | null {
  return useSyncExternalStore(subscribe, getSummary);
}

const NO_INCIDENTS: Incident[] = [];
const getIncidents = () => state?.incidents ?? NO_INCIDENTS;

/** Active incidents, worst first. */
export function useIncidents(): Incident[] {
  return useSyncExternalStore(subscribe, getIncidents);
}

/** Whether an alert counts toward badges and levels (older engines: firing and not muted). */
export function alertCounts(a: Alert, now = Date.now()): boolean {
  if (a.counted !== undefined) return a.counted;
  return a.state === "firing" && a.severity !== "info" && !(a.mutedUntilMs && a.mutedUntilMs > now);
}

/** The state an alert is shown in. */
export type AlertShownAs = "pending" | "firing" | "recovering" | "planned" | "silenced" | "acknowledged" | "info";

export function alertShownAs(a: Alert): AlertShownAs {
  if (a.state === "pending") return "pending";
  if (a.planned) return "planned";
  if (a.silenceId) return "silenced";
  if (a.recoversAtMs) return "recovering";
  if (a.ackedMs) return "acknowledged";
  if (a.severity === "info") return "info";
  return "firing";
}

/** "3 min", "2 h 5 min", "3 days": how long something has been going. */
export function fmtFor(ms: number): string {
  const m = Math.max(0, Math.floor(ms / 60_000));
  if (m < 1) return `${Math.max(0, Math.floor(ms / 1000))} s`;
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
  const d = Math.floor(h / 24);
  return d === 1 ? "1 day" : `${d} days`;
}

/** "1:34" until a time (minutes and seconds). */
export function fmtClock(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** "Silenced until Tue 10:42 · expanding disk Friday", "Silenced until it clears", "Silenced indefinitely". */
export function silenceLine(s: Silence, now = Date.now()): string {
  let when: string;
  if (s.until.kind === "at") {
    const d = new Date(s.until.ms);
    const sameDay = new Date(now).toDateString() === d.toDateString();
    const t = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    when = `until ${sameDay ? t : `${d.toLocaleDateString([], { weekday: "short" })} ${t}`}`;
  } else {
    when = s.until.kind === "resolved" ? "until it clears" : "indefinitely";
  }
  return `Silenced ${when}${s.reason ? ` · ${s.reason}` : ""}`;
}

/** The words for each silence duration, as the Silence menu offers them. */
export const DURATION_LABEL: Record<SilenceDuration, string> = {
  "1h": "1 hour",
  "1d": "1 day",
  "1w": "1 week",
  resolved: "Until it clears",
  never: "Indefinitely",
};

/** A week or longer needs a reason (the backend refuses otherwise). */
export const needsReason = (d: SilenceDuration) => d === "1w" || d === "never";

/**
 * The unlock an alert action asks for (health/mod.rs gates): acknowledging, ending a silence and
 * a silence of an hour, a day or until it clears are node scope; a week or longer with Live data
 * is Mac scope (Touch ID).
 */
export function alertScope(action: "alert_ack" | "alert_silence" | "alert_unsilence", demo: boolean, duration?: SilenceDuration): "node" | "mac" {
  return action === "alert_silence" && !demo && duration !== undefined && needsReason(duration) ? "mac" : "node";
}

/** "disk:gx10-0000:/": the entity key a silence's matcher names (silence.rs entity_key). */
export function entityKey(a: Alert): string {
  const place = a.entity.switch ?? a.entity.node ?? "";
  return `${a.entity.kind}:${place}:${a.entity.id}`;
}

/** What a status dot shows: the link state first, then staleness, then health. */
export type DisplayStatus = LinkStatus | "stale" | "unconfigured" | "warn" | "crit";

export function displayStatus(node: NodeSnapshot, health: HealthState | null = getHealth()): DisplayStatus {
  if (node.status !== "online") return node.status;
  if (node.stale) return "stale";
  const level = health?.nodes[node.id]?.level ?? "ok";
  return level === "ok" ? "online" : level;
}

export function switchDisplayStatus(sw: SwitchSnapshot, health: HealthState | null = getHealth()): DisplayStatus {
  if (sw.configured === false) return "unconfigured";
  if (sw.status !== "online") return sw.status;
  const level = health?.switches[sw.id]?.level ?? "ok";
  return level === "ok" ? "online" : level;
}

/** Plain words for a display status, for tooltips and screen readers. */
export const DISPLAY_LABEL: Record<DisplayStatus, string> = {
  online: "Healthy",
  connecting: "Connecting",
  offline: "Offline",
  error: "Error",
  stale: "Not updating",
  unconfigured: "Not set up",
  warn: "Warning",
  crit: "Critical",
};

/** "cell1 — Warning: Root disk 86% full; nvidia-dgx-telemetry.service failed". */
export function statusTooltip(name: string, status: DisplayStatus, issues: Issue[] = [], extra?: string | null): string {
  const head = `${name} — ${DISPLAY_LABEL[status]}`;
  const lines = issues.slice(0, 6).map((i) => `${i.severity === "crit" ? "Critical" : "Warning"}: ${i.summary}`);
  if (issues.length > 6) lines.push(`and ${issues.length - 6} more`);
  return [extra ? `${head} · ${extra}` : head, ...lines].join("\n");
}

export const healthApi = {
  get: () => invoke<HealthState>("health_get"),
  history: (limit: number) => invoke<AlertEvent[]>("alerts_history", { limit }),
  /** An alert id or an incident id (every alert in it). */
  ack: (id: string) => invoke<HealthState>("alert_ack", { id }),
  silence: (matcher: Silence["matcher"], duration: SilenceDuration, reason: string, label: string) =>
    invoke<HealthState>("alert_silence", { matcher, duration, reason, label }),
  unsilence: (id: string) => invoke<HealthState>("alert_unsilence", { id }),
};

/** Applies a state returned by ack/mute right away instead of waiting for the next event. */
export function applyHealth(s: HealthState) {
  if (!state || s.tsMs >= state.tsMs) set(s);
}

/** The window that shows an alert's entity, e.g. a disk alert opens its node on the Disks tab. */
export function entityTarget(e: AlertEntity): { kind: string; params: Record<string, string> } {
  const node = e.node ?? "";
  switch (e.kind) {
    case "node": return { kind: "node", params: { id: e.id } };
    case "disk": return { kind: "node", params: { id: node, tab: "disks" } };
    case "gpu": return { kind: "node", params: { id: node, tab: "gpu" } };
    case "interface": return { kind: "node", params: { id: node, tab: "network" } };
    case "container": return { kind: "containers", params: { node } };
    case "service": return { kind: "services", params: { node } };
    case "switch": return { kind: "switch", params: { id: e.id } };
    case "port": return { kind: "switch", params: { id: e.switch ?? "", port: e.id } };
    case "endpoint": return { kind: "endpoints", params: {} };
    case "fabric": return { kind: "fabric", params: {} };
    case "sensor": return { kind: "switch", params: { id: e.switch ?? "" } };
  }
}

/** "just now", "4 min ago", "3 h ago", "yesterday", "5 days ago". */
export function fmtAgo(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 10) return "just now";
  if (s < 60) return `${s} s ago`;
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86_400) return `${Math.round(s / 3600)} h ago`;
  if (s < 172_800) return "yesterday";
  return `${Math.round(s / 86_400)} days ago`;
}

/** "47m", "3h 12m", "2d 4h": how long an alert has been going. */
export function fmtSpan(ms: number): string {
  const m = Math.max(0, Math.floor(ms / 60_000));
  if (m < 1) return `${Math.max(0, Math.floor(ms / 1000))}s`;
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

