// Monitoring backend for the browser preview: the health engine's view of the
// Demo baseline (one failed unit on cell1, gx10-0000's disk at 85%) as incidents,
// silences and acknowledgements kept in memory (with the engine's rules: the five
// durations, a reason for the long ones, known issues never counted), a short
// alert log with one "while away" event, a day of history shaped like the Rust
// backfill, the alert rules (the same defaults file the Rust engine writes),
// simulated always-on agents, alert channels and a delivery log kept in memory.
import type { WatchState } from "../../lib/agent";
import type { Alert, AlertEvent, HealthState, Incident, Silence, SilenceDuration } from "../../lib/health";
import type { HistoryResult } from "../../lib/historyApi";
import type { Channel, Delivery } from "../../lib/notifyApi";
import type { MockHandlers } from "../../test/tauri";
import { baseSnapshot } from "../mockBackend";
import defaultRules from "../../../src-tauri/src/health/fixtures/default-rules.json";
import kernelMlx5 from "../../../src-tauri/src/health/fixtures/kernel-mlx5.jsonl?raw";
import type { DecodedEvent } from "../../windows/AlertsWindow";

const MIN = 60_000;

function alert(p: Partial<Alert> & Pick<Alert, "id" | "rule" | "category" | "severity" | "entity" | "message" | "summary">, sinceAgoMs: number): Alert {
  const since = Date.now() - sinceAgoMs;
  return { state: "firing", detail: null, value: null, sinceMs: since, firingSinceMs: since, resolvedMs: null, ackedMs: null, mutedUntilMs: null, counted: true, ...p };
}

/** "disk:gx10-0000:/": the key a silence matches (health/silence.rs entity_key). */
const keyOf = (a: Alert) => `${a.entity.kind}:${a.entity.switch ?? a.entity.node ?? ""}:${a.entity.id}`;
/** The incident an alert is in (health/incident.rs: here every baseline alert is its own). */
const rootOf = (a: Alert) => `${a.entity.kind}:${a.entity.switch ?? a.entity.node ?? ""}/${a.entity.id}`;

const disk = alert({
  id: "disk-full:disk:gx10-0000:/", rule: "disk-full", category: "disk", severity: "warn", value: 85, short: "Disk full",
  entity: { kind: "disk", id: "/", node: "gx10-0000", switch: null, label: "Root disk on gx10-0000" },
  message: "Root disk 85% full (143 GiB free)", summary: "gx10-0000 disk 85%",
}, 47 * MIN);
const unit = alert({
  id: "unit-failed:service:cell1:nvidia-dgx-telemetry.service", rule: "unit-failed", category: "containers", severity: "warn", short: "Unit failed",
  entity: { kind: "service", id: "nvidia-dgx-telemetry.service", node: "cell1", switch: null, label: "nvidia-dgx-telemetry.service on cell1" },
  message: "nvidia-dgx-telemetry.service failed", summary: "nvidia-dgx-telemetry failed on cell1",
}, 3 * 3600_000 + 12 * MIN);

let active: Alert[] = [disk, unit];
let silences: Silence[] = [];

/** Each alert as the engine derives it: its incident, silence, and whether it counts. */
function derived(now: number): Alert[] {
  silences = silences.filter((s) => s.until.kind !== "at" || s.until.ms > now);
  return active.map((a) => {
    const incidentId = rootOf(a);
    const s = silences.find((x) => (!x.matcher.incident || x.matcher.incident === incidentId) && (!x.matcher.rule || x.matcher.rule === a.rule) && (!x.matcher.entity || x.matcher.entity === keyOf(a)));
    const counted = a.state === "firing" && !s && !a.ackedMs && a.severity !== "info";
    return { ...a, incidentId, rootKey: incidentId, silenceId: s?.id ?? null, mutedUntilMs: s ? (s.until.kind === "at" ? s.until.ms : 253_402_300_799_000) : null, counted };
  });
}

function incidentsOf(alerts: Alert[]): Incident[] {
  return alerts.map((a) => ({
    id: a.incidentId!, title: a.summary, severity: a.severity, children: [a.id], cause: null, sinceMs: a.firingSinceMs ?? a.sinceMs,
    affects: a.entity.label, headline: a.id, counted: !!a.counted, recoversAtMs: null, planned: null,
    state: a.silenceId ? "silenced" : a.ackedMs ? "acknowledged" : a.severity === "info" ? "info" : "firing",
  }));
}

function view(): HealthState {
  const now = Date.now();
  const alerts = derived(now);
  const counted = alerts.filter((a) => a.counted);
  const nodes: HealthState["nodes"] = {};
  for (const n of baseSnapshot.nodes) nodes[n.id] = { level: "ok", issues: [] };
  for (const a of counted) {
    const n = a.entity.node && nodes[a.entity.node];
    if (!n) continue;
    if (a.severity === "info") continue;
    if (a.severity === "crit" || n.level === "ok") n.level = a.severity;
    n.issues.push({ alertId: a.id, severity: a.severity, summary: a.message, label: a.entity.label });
  }
  const switches: HealthState["switches"] = {};
  for (const s of baseSnapshot.switches) switches[s.id] = { level: "ok", issues: [] };
  const known = alerts.filter((a) => a.state === "firing" && (a.silenceId || a.ackedMs)).map((a) => {
    const s = silences.find((x) => x.id === a.silenceId);
    return { alertId: a.id, incidentId: a.incidentId!, title: a.summary, severity: a.severity, why: s ? ("silenced" as const) : ("acknowledged" as const), untilMs: s?.until.kind === "at" ? s.until.ms : null, reason: s?.reason || null };
  });
  return {
    tsMs: now,
    alerts,
    summary: {
      warn: counted.filter((a) => a.severity === "warn").length,
      crit: counted.filter((a) => a.severity === "crit").length,
      pending: alerts.filter((a) => a.state === "pending").length,
      muted: known.length,
      recovering: 0,
      planned: 0,
      known,
      pendingAlerts: [],
      alerts: counted.length,
    },
    nodes,
    switches,
    lastAlertMs: now - 47 * MIN,
    incidents: incidentsOf(alerts),
    silences,
    demo: true,
  };
}

const DURATION_MS: Partial<Record<SilenceDuration, number>> = { "1h": 3_600_000, "1d": 86_400_000, "1w": 7 * 86_400_000 };

/** The same thing silenced (health/silence.rs Matcher::same_as): a second silence replaces the first. */
const sameMatcher = (a: Silence["matcher"], b: Silence["matcher"]) =>
  a.incident || b.incident ? !!a.incident && a.incident === b.incident : (a.rule ?? null) === (b.rule ?? null) && (a.entity ?? null) === (b.entity ?? null);

/** alert_silence, with the engine's checks (health/silence.rs make) and its upsert by matcher. */
function silence({ matcher, duration, reason, label }: { matcher: Silence["matcher"]; duration: SilenceDuration; reason: string; label: string }): HealthState {
  if (!["1h", "1d", "1w", "resolved", "never"].includes(duration)) throw new Error(`A silence lasts 1 hour, 1 day, 1 week, until it clears or indefinitely, not "${duration}".`);
  if ((duration === "1w" || duration === "never") && !reason.trim()) throw new Error("Say why: a silence for a week or longer needs a reason.");
  const now = Date.now();
  const ms = DURATION_MS[duration];
  const until: Silence["until"] = ms ? { kind: "at", ms: now + ms } : duration === "resolved" ? { kind: "resolved" } : { kind: "never" };
  const same = silences.find((x) => sameMatcher(x.matcher, matcher));
  silences = same
    ? silences.map((x) => (x === same ? { ...x, until, reason: reason.trim(), duration, label } : x))
    : [...silences, { id: `sil-${now.toString(16)}`, matcher, reason: reason.trim(), createdMs: now, createdBy: "Demo", source: "demo", label, duration, until }];
  return view();
}

// ---- Kernel events: the 58 real mlx5 lines of Oct 25–26 (health/fixtures/kernel-mlx5.jsonl),
// decoded and grouped the way health/kernel.rs does it.

type RawEvent = { node: string; tsMs: number; kind: string; code: number | null; text: string; severity: string; process: string | null };

/** health/kernel.rs link_text. */
function linkText(text: string): string | null {
  const t = text.toLowerCase();
  const words = (text.split("mlx5_core")[1] ?? "").trim().split(/\s+/);
  const pci = words[0]?.replace(/:$/, "");
  const iface = words[1]?.endsWith(":") ? words[1].slice(0, -1) : null;
  if (t.includes("cable unplugged")) return `ConnectX-7 cable unplugged (${pci})`;
  if (t.includes("cable plugged")) return `ConnectX-7 cable plugged in (${pci})`;
  if (t.includes("port: ") && t.endsWith("link down")) return `RoCE port down on ${iface}`;
  if (t.includes("port: ") && (t.endsWith("link active") || t.endsWith("link up"))) return `RoCE port up on ${iface}`;
  if (t.endsWith("link down")) return `Link down on ${iface}`;
  if (t.endsWith("link up")) return `Link up on ${iface}`;
  return null;
}

function kernelEvents(): DecodedEvent[] {
  const raw: RawEvent[] = kernelMlx5.trim().split("\n").map((l: string) => JSON.parse(l) as RawEvent);
  const meaning = "a fabric link or cable changed state (recabling, or the other end restarting)";
  const action = "Nothing to do if you were recabling or restarting. A link that stays down shows in Fabric.";
  const decoded: DecodedEvent[] = raw
    .map((e) => ({ node: e.node, tsMs: e.tsMs, kind: "link" as const, code: null, text: e.text, severity: "info" as const, process: null, title: linkText(e.text) ?? "ConnectX-7 link event", meaning, action }))
    .sort((a, b) => a.tsMs - b.tsMs);
  const groups: DecodedEvent[][] = [];
  for (const e of decoded) {
    const g = groups[groups.length - 1];
    if (g && e.tsMs - g[g.length - 1].tsMs <= 15 * MIN) g.push(e);
    else groups.push([e]);
  }
  return groups
    .map((list) => {
      const nodes = [...new Set(list.map((e) => e.node))].sort();
      const titles = list.map((e) => e.title.toLowerCase());
      const what = [titles.some((t) => t.includes("unplugged")) && "cables unplugged", titles.some((t) => t.includes(" down")) && "links down"].filter(Boolean).join(" and ");
      const where = nodes.length === 1 ? nodes[0] : `${nodes.length} nodes`;
      const title = `${list.length} link events on ${where}${what ? ` (${what})` : ""}`;
      return {
        node: nodes.length === 1 ? nodes[0] : "", tsMs: list[0].tsMs, kind: "link" as const, code: null, text: title, severity: "info" as const, process: null, title,
        meaning: "fabric cables or links changed state together: recabling, or nodes restarting", action,
        group: `link-${list[0].tsMs}`, events: [...list].reverse(), nodes, toMs: list[list.length - 1].tsMs,
      };
    })
    .reverse();
}

function past(a: Alert, kind: AlertEvent["kind"], agoMs: number): AlertEvent {
  const ts = Date.now() - agoMs;
  return { tsMs: ts, kind, alert: { ...a, state: kind === "resolved" ? "resolved" : "firing", resolvedMs: kind === "resolved" ? ts : null } };
}

const offline = alert({
  id: "node-offline:node:cell3", rule: "node-offline", category: "node", severity: "crit",
  entity: { kind: "node", id: "cell3", node: "cell3", switch: null, label: "cell3" },
  message: "Offline · SSH timeout: no reply from 192.168.50.98:22 in 15 s", summary: "cell3 offline",
}, 5 * 3600_000);
const throttle = alert({
  id: "gpu-throttle:gpu:cell1:gpu0", rule: "gpu-throttle", category: "gpu", severity: "warn",
  entity: { kind: "gpu", id: "gpu0", node: "cell1", switch: null, label: "GPU on cell1" },
  message: "GPU throttled: HW thermal slowdown, SW power cap", summary: "cell1 GPU throttled",
}, 2 * 3600_000);

const peer = alert({
  id: "peer-unreachable:node:cell2", rule: "peer-unreachable", category: "node", severity: "crit",
  entity: { kind: "node", id: "cell2", node: "cell2", switch: null, label: "cell2" },
  message: "Unreachable from the other agents for 60 s", summary: "cell2 is unreachable from the other agents",
}, 9 * 3600_000 + 20 * MIN);

const log: AlertEvent[] = [
  past(disk, "raised", 47 * MIN),
  past(throttle, "resolved", 118 * MIN),
  past(throttle, "raised", 121 * MIN),
  past(unit, "raised", 3 * 3600_000 + 12 * MIN),
  past(offline, "resolved", 5 * 3600_000 - 2 * MIN),
  past(offline, "raised", 5 * 3600_000),
  { ...past(peer, "resolved", 9 * 3600_000 + 16 * MIN), away: true, source: "agent:cell1" },
  { ...past(peer, "raised", 9 * 3600_000 + 19 * MIN), away: true, source: "agent:cell1" },
];

// ---- History shaped like src-tauri/src/history/backfill.rs

const demand = (h: number) => {
  const bump = (c: number, w: number) => Math.exp(-(((h - c) / w) ** 2));
  return Math.min(1, 0.08 + 0.55 * bump(14.5, 3) + 0.45 * bump(20.5, 1.8) + 0.2 * bump(10, 1.5));
};

function synth(nodeIndex: number, metric: string, t: number): number | null {
  const n = baseSnapshot.nodes[nodeIndex];
  if (!n) return null;
  const d = new Date(t);
  const hour = d.getHours() + d.getMinutes() / 60;
  const ago = Date.now() - t;
  if (nodeIndex === 1 && ago > 9 * 3600_000 + 16 * MIN && ago < 9 * 3600_000 + 20 * MIN) return null;
  const wob = Math.sin(t / 47_000 + nodeIndex) * 0.5 + Math.sin(t / 13_000 + nodeIndex * 2) * 0.5;
  const dm = demand(hour);
  const serving = n.mem.gpuAlloc > 2 ** 30;
  const util = serving && nodeIndex < 3 ? Math.max(0, Math.min(99, 12 + 78 * dm + 8 * wob)) : nodeIndex === 3 && (hour % 6) < 1.5 ? 40 + 30 * dm : 0;
  switch (metric) {
    case "gpu": return util;
    case "cpu": return 3 + 9 * dm * (util / 100 + 0.3) + wob;
    case "memUsed": return n.mem.used + wob * 0.4 * 2 ** 30;
    case "gpuAlloc": return n.mem.gpuAlloc;
    case "power": return 11 + (util / 100) * 100;
    case "gpuTemp": return 36 + util * 0.36 + wob;
    case "fabricRx": case "fabricTx": return nodeIndex < 3 ? (20e6 + 60e6 * dm) * (1 + 0.3 * wob) : 0;
    case "rdmaRx": case "rdmaTx": return nodeIndex < 3 ? 0.9 * (20e6 + 60e6 * dm) * (1 + 0.3 * wob) : 0;
    case "lanRx": return 0.3e6 + 25e6 * dm;
    case "lanTx": return 0.1e6 + 9e6 * dm;
    case "diskRead": return 0.2e6 + 1.5e6 * Math.abs(wob);
    case "diskWrite": return 0.4e6 + 2.5e6 * dm * Math.abs(wob);
    case "memCache": return n.mem.cached + wob * 2 ** 30;
    case "smClock": return util > 1 ? 2400 + 200 * wob : 650;
    default: return null;
  }
}

function historyQuery({ entity, metrics, fromMs, toMs, maxPoints }: { entity: string; metrics: string[]; fromMs: number; toMs: number; maxPoints: number }): HistoryResult {
  const id = entity.replace(/^[a-z]+:/, "").split("/")[0];
  const index = baseSnapshot.nodes.findIndex((n) => n.id === id);
  const spanS = (toMs - fromMs) / 1000;
  const tier = spanS <= 3660 ? "1s" : spanS <= 86_400 ? "10s" : spanS <= 7 * 86_400 ? "1m" : "15m";
  const base = { "1s": 1000, "10s": 10_000, "1m": 60_000, "15m": 900_000 }[tier];
  const step = Math.max(base, Math.ceil((toMs - fromMs) / maxPoints / base) * base);
  // The preview pretends history began a day ago.
  const startsAtMs = Math.floor((Date.now() - 86_400_000) / base) * base;
  const ts: number[] = [];
  for (let t = Math.floor(fromMs / step) * step; t <= toMs; t += step) ts.push(t);
  const series: Record<string, (number | null)[]> = {};
  for (const m of metrics) series[m] = ts.map((t) => (t < startsAtMs ? null : synth(index, m, t)));
  return { ts, series, tier, stepMs: step, startsAtMs };
}

let notify = {
  enabled: true, node: true, disk: true, gpu: true, fabric: true, containers: true, tasks: true,
  criticalOnly: false, resolved: true, quietHours: false, quietStart: "22:00", quietEnd: "07:00",
};

// ---- Channels and deliveries

let channels: Channel[] = [
  { id: "ntfy-1", kind: "ntfy", name: "Phone (ntfy)", enabled: true, criticalOnly: false, server: "https://ntfy.sh", topic: "sparkbox-alerts-7f3k", secrets: { token: false }, target: "ntfy.sh · sparkbox-alerts-7f3k", last: null },
];
const row = (p: Partial<Delivery> & Pick<Delivery, "id" | "title" | "severity">, agoMs: number): Delivery => ({
  tsMs: Date.now() - agoMs, channel: "", channelName: "", kind: "mac", body: "", ok: true, error: null, via: "mac", demo: true, test: false, attempts: 1, ...p,
});
let deliveries: Delivery[] = [
  row({ id: "d1", channel: "ntfy-1", channelName: "Phone (ntfy)", kind: "ntfy", title: "gx10-0000 disk 85%", body: "Root disk on gx10-0000 — Root disk 85% full (143 GiB free)", severity: "warn", via: "agent:gx10-0000", outcome: "agent" }, 47 * MIN),
  row({ id: "d2", title: "nvidia-dgx-telemetry failed on cell1", severity: "warn", outcome: "coalesced", reason: "Part of “nvidia-dgx-telemetry failed on cell1”, paged 3 h earlier" }, 3 * 3600_000),
  row({ id: "d3", channel: "ntfy-1", channelName: "Phone (ntfy)", kind: "ntfy", title: "nvidia-dgx-telemetry failed on cell1", body: "nvidia-dgx-telemetry.service on cell1 — nvidia-dgx-telemetry.service failed", severity: "warn", outcome: "demo" }, 3 * 3600_000 + 12 * MIN),
  row({ id: "d4", title: "Resolved: cell3 offline", severity: "crit", outcome: "held", reason: "Sent if it stays clear for 10 min" }, 5 * 3600_000 - 2 * MIN),
  row({ id: "d5", channel: "ntfy-1", channelName: "Phone (ntfy)", kind: "ntfy", title: "cell3 is back online", severity: "crit", outcome: "demo" }, 5 * 3600_000 - 12 * MIN),
  row({ id: "d6", channel: "ntfy-1", channelName: "Phone (ntfy)", kind: "ntfy", title: "cell3 offline", body: "cell3 — Offline · SSH timeout: no reply from 192.168.50.98:22 in 15 s", severity: "crit", outcome: "demo" }, 5 * 3600_000),
  row({ id: "d7", title: "cell3 offline", severity: "crit", outcome: "flapping" as Delivery["outcome"], reason: "Back 2 min after it cleared; paged once" }, 26 * 3600_000),
  row({ id: "d8", title: "cell1 restarting", severity: "crit", outcome: "planned", reason: "cell1 restarting (SparkBox, 06:41)" }, 27 * 3600_000),
];
channels[0].last = deliveries[0];

function channelViews(): Channel[] {
  const mac: Channel = { id: "mac", kind: "mac", name: "This Mac", enabled: notify.enabled, criticalOnly: notify.criticalOnly, secrets: {}, target: "Notification Center", last: null };
  return [mac, ...channels.map((c) => ({ ...c, last: deliveries.find((d) => d.channel === c.id) ?? null }))];
}

// ---- Rules

type RuleSetJson = { version: number; rules: { id: string; enabled: boolean; warn: number | null; metric: string; entity: string }[] };
let rules = structuredClone(defaultRules) as RuleSetJson;

/** "Would fire now" for the preview: gx10-0000's disk is at 85%, cell1 has a failed unit. */
function preview({ rules: candidate }: { rules: RuleSetJson }) {
  return candidate.rules.map((r) => {
    const matches: { label: string; severity: "warn" | "crit"; message: string; value: number | null; heldS: number; firesInS: number }[] = [];
    if (r.enabled && r.id === "disk-full" && (r.warn ?? 101) <= 85) matches.push({ label: "Root disk on gx10-0000", severity: "warn", message: "Root disk 85% full (143 GiB free)", value: 85, heldS: 2820, firesInS: 0 });
    if (r.enabled && r.id === "unit-failed") matches.push({ label: "nvidia-dgx-telemetry.service on cell1", severity: "warn", message: "nvidia-dgx-telemetry.service failed", value: null, heldS: 11520, firesInS: 0 });
    if (r.enabled && r.id === "mem-high" && (r.warn ?? 101) <= 72) matches.push({ label: "cell1", severity: "warn", message: "Unified memory 72% used", value: 72, heldS: 0, firesInS: 60 });
    // The real nodes don't report a clock offset (timedatectl gives none without NTP details).
    const unreported = r.id === "ntp-offset" ? ["cell1", "cell2", "cell3", "gx10-0000"] : [];
    return { id: r.id, known: (!r.id.startsWith("roce") || r.entity !== "net") && !unreported.length, matches, unreported };
  });
}

// ---- Agents

let agentsInstalled = new Set(baseSnapshot.nodes.map((n) => n.id));
let agentSettings = { metricsEnabled: false, metricsPort: 9470, port: 9469, hasToken: false };

function watch(): WatchState {
  const now = Date.now();
  const agents = baseSnapshot.nodes.map((n, i) => {
    const on = agentsInstalled.has(n.id);
    return {
      node: n.id, name: n.name, installed: on, state: on ? ("running" as const) : ("not-installed" as const), version: on ? "1.0.0" : null,
      lastBeatMs: on ? now - ((Math.floor(now / 1000) + i) % 2) * 1000 : null, historyFromMs: on ? now - 14 * 86_400_000 : null, firing: 0,
      lastDeliveryMs: null, lastDeliveryOk: null, uptimeS: on ? 6 * 86_400 + 4 * 3600 : null, metricsPort: null, error: null, simulated: true,
    };
  });
  const healthy = agents.filter((a) => a.installed).length;
  const mode = healthy === 0 ? "mac-only" : healthy === agents.length ? "agents" : "partial";
  return { mode, agents, deliveryOwner: !channels.some((c) => c.enabled) ? "none" : mode === "agents" ? "agents" : "mac" };
}

export const handlers: MockHandlers = {
  health_get: () => view(),
  alerts_history: ({ limit }: { limit: number }) => log.slice(0, limit),
  alert_ack: ({ id }: { id: string }) => {
    active = active.map((a) => (a.id === id || rootOf(a) === id ? { ...a, ackedMs: Date.now() } : a));
    return view();
  },
  alert_silence: silence,
  alert_unsilence: ({ id }: { id: string }) => {
    const s = silences.find((x) => x.id === id);
    if (!s) throw new Error("That silence has already ended.");
    // Every silence on the same alerts ends with it (health/mod.rs remove_silence).
    const covered = derived(Date.now()).filter((a) => a.silenceId === id);
    silences = silences.filter((x) => !(x.id === id || sameMatcher(x.matcher, s.matcher) || covered.some((a) => a.incidentId === x.matcher.incident || (x.matcher.rule === a.rule && x.matcher.entity === keyOf(a)))));
    return view();
  },
  history_query: historyQuery,
  notify_settings_get: () => notify,
  notify_settings_set: ({ settings }: { settings: typeof notify }) => (notify = { ...notify, ...settings }),
  notify_take_pending: () => null,
  notify_channels_get: () => channelViews(),
  notify_channels_set: ({ channels: next }: { channels: Channel[] }) => {
    const mac = next.find((c) => c.kind === "mac");
    if (mac) notify = { ...notify, enabled: mac.enabled, criticalOnly: mac.criticalOnly };
    channels = next.filter((c) => c.kind !== "mac");
    return channelViews();
  },
  notify_channel_test: ({ id, via }: { id: string; via: string | null }) => {
    const c = channels.find((x) => x.id === id);
    if (!c) return { ok: true, message: "Shown in Notification Center.", delivery: null };
    const d: Delivery = {
      id: `d${Date.now()}`, tsMs: Date.now(), channel: c.id, channelName: c.name, kind: c.kind, title: via ? `SparkBox test from the agent on ${via}` : "SparkBox test",
      body: `This is how SparkBox alerts reach you on ${c.name}.`, severity: "warn", ok: true, error: null, via: via ? `agent:${via}` : "mac", demo: !!via, test: true, attempts: 1,
    };
    deliveries = [d, ...deliveries].slice(0, 100);
    return { ok: true, message: via ? `Demo — the agent on ${via} is simulated, so nothing was sent.` : "Sent from this Mac.", delivery: d };
  },
  notify_deliveries: ({ limit }: { limit: number }) => deliveries.slice(0, limit),
  rules_get: () => rules,
  rules_defaults: () => structuredClone(defaultRules),
  rules_set: ({ rules: next }: { rules: RuleSetJson }) => (rules = next),
  rules_preview: preview,
  kernel_events: () => kernelEvents(),
  agent_status: () => watch(),
  agent_install: ({ nodes }: { nodes: string[] }) => {
    nodes.forEach((n) => agentsInstalled.add(n));
    return "t-mock-agent";
  },
  agent_uninstall: ({ nodes }: { nodes: string[] }) => {
    agentsInstalled = new Set([...agentsInstalled].filter((n) => !nodes.includes(n)));
    return "t-mock-agent";
  },
  agent_push_config: () => watch(),
  agent_backfill_now: () => ({ points: 0, alerts: 0 }),
  agent_settings_get: () => agentSettings,
  agent_settings_set: ({ settings, token }: { settings: typeof agentSettings; token: string | null }) =>
    (agentSettings = { ...agentSettings, ...settings, hasToken: token === null ? agentSettings.hasToken : token !== "" }),
};
