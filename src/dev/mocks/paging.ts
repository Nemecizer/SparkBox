// Paging backend for the browser preview, in step with notify.rs and agent/: the
// notification settings (quiet hours in one zone, repeats), channels with "this Mac
// only" and the dead-man's switch, the delivery log with outcomes, and simulated
// agents on current settings that passed a test page 2 hours ago, like the Demo
// baseline, with its simulated phone channel and dead-man's switch as rows. Push again
// runs as a task that ends after a moment (one toast). `?paging=drift` plays the
// paging-drift scenario (cell3's push fails).
import type { AgentStatus, EndToEnd, WatchState } from "../../lib/agent";
import type { Channel, Delivery, NotifySettings } from "../../lib/notifyApi";
import { ingestTask } from "../../lib/tasks";
import type { Task } from "../../lib/opsApi";
import type { MockHandlers } from "../../test/tauri";
import { baseSnapshot } from "../mockBackend";

const MIN = 60_000;
const started = Date.now();
const drift = typeof location !== "undefined" && new URLSearchParams(location.search).get("paging") === "drift";

let notify: NotifySettings = {
  enabled: true, node: true, disk: true, gpu: true, fabric: true, containers: true, tasks: true,
  criticalOnly: false, resolved: true, quietHours: true, quietStart: "22:00", quietEnd: "07:00", quietTz: "America/New_York", repeatMinutes: 60, changedMs: 0,
};

let channels: Channel[] = [];
const demoPhone = { id: "demo-phone", name: "Phone (ntfy · Demo)" };

function row(p: Partial<Delivery> & Pick<Delivery, "title" | "outcome">): Delivery {
  return {
    id: `d${Math.random().toString(16).slice(2, 8)}`, tsMs: Date.now(), channel: "", channelName: "", kind: "ntfy", body: "", severity: "warn",
    ok: p.outcome !== "failed", error: null, via: "mac", demo: true, test: false, attempts: 1, ...p,
  };
}

let deliveries: Delivery[] = [
  row({ tsMs: started - 47 * MIN, channel: demoPhone.id, channelName: demoPhone.name, title: "gx10-0000 disk 85%", outcome: "demo", via: "agent:gx10-0000", rule: "disk-full", messageId: "m-1" }),
  row({ tsMs: started - 3 * 3600_000, title: "nvidia-dgx-telemetry failed on cell1", outcome: "held", reason: "Quiet hours, 22:00 to 07:00 · America/New_York", rule: "unit-failed" }),
  ...baseSnapshot.nodes.map((n) => row({ tsMs: started - 2 * 3600_000, channel: demoPhone.id, channelName: demoPhone.name, title: `Test from ${n.name}`, body: `Test from ${n.name} via ${demoPhone.name}: delivered in 0.8 s`, outcome: "demo", via: `agent:${n.id}`, test: true, latencyMs: 800 })),
];

let installed = new Set(baseSnapshot.nodes.map((n) => n.id));
let channelsChangedMs = 0;
let pushSeq = 0;
let pushRunning: string | null = null;
let tested = started - 2 * 3600_000;
let pushError: { atMs: number; message: string; attempts: number } | null = drift ? { atMs: started, message: "sudo: a password is required", attempts: 1 } : null;
let agentSettings = { metricsEnabled: false, metricsPort: 9470, port: 9469, hasToken: false };

function agents(now: number): AgentStatus[] {
  return baseSnapshot.nodes.map((n, i) => {
    const on = installed.has(n.id);
    const stale = n.id === "cell3" && !!pushError;
    return {
      node: n.id, name: n.name, installed: on, state: on ? "running" : "not-installed", version: on ? "1.2.0" : null,
      lastBeatMs: on ? now - ((Math.floor(now / 1000) + i) % 2) * 1000 : null, historyFromMs: on ? now - 14 * 86_400_000 : null, firing: 0,
      lastDeliveryMs: on ? tested : null, lastDeliveryOk: on ? true : null, uptimeS: on ? 6 * 86_400 + 4 * 3600 : null, metricsPort: null, error: null, simulated: true,
      configHash: on ? (stale ? "9b1e4d7a" : "5d0c9a7f") : null, expectedHash: on ? "5d0c9a7f" : null, configCurrent: on ? !stale : null,
      pushError: stale ? pushError : null, endpointsProbed: on && n.id === "cell1" ? ["llama-3.3-70b"] : on && n.id === "gx10-0000" ? ["ollama-gx10"] : [],
      heartbeatOkMs: on && i === 0 ? now - (Math.floor(now / 1000) % 60) * 1000 : null, outboxPending: 0,
    };
  });
}

function endToEnd(list: AgentStatus[]): EndToEnd {
  const on = list.filter((a) => a.installed);
  const phones = [demoPhone, ...channels.filter((c) => c.enabled && !c.macOnly && c.kind !== "heartbeat").map((c) => ({ id: c.id, name: c.name }))];
  const cells = on.flatMap((a, i) => phones.map((p) => ({ node: a.node, channel: p.id, channelName: p.name, ok: true, error: null, ms: 620 + i * 170, atMs: tested })));
  // What a test proves is the channels: quiet hours, repeats and silences don't make it old.
  return { testedAtMs: on.length ? tested : null, ok: on.length > 0 && tested >= channelsChangedMs, cells };
}

function watch(): WatchState {
  const now = Date.now();
  const list = agents(now);
  const healthy = list.filter((a) => a.installed).length;
  const mode = healthy === 0 ? "mac-only" : healthy === list.length ? "agents" : "partial";
  const stale = list.filter((a) => a.installed && a.configCurrent === false);
  return {
    mode, agents: list, deliveryOwner: stale.length || mode !== "agents" ? "mac" : "agents",
    channels: [
      ...channels.map((c) => ({ id: c.id, name: c.name, kind: c.kind, remote: c.kind !== "heartbeat", macOnly: !!c.macOnly, enabled: c.enabled, lastOkMs: null, lastError: null, lastErrorMs: null, simulated: false })),
      { ...demoPhone, kind: "ntfy", remote: true, macOnly: false, enabled: true, lastOkMs: tested, lastError: null, lastErrorMs: null, simulated: true },
      { id: "demo-deadman", name: "Dead-man's switch (Demo)", kind: "Dead-man's switch", remote: false, macOnly: false, enabled: true, lastOkMs: now - (Math.floor(now / 1000) % 60) * 1000, lastError: null, lastErrorMs: null, simulated: true },
    ],
    endToEnd: endToEnd(list),
    changedMs: notify.changedMs ?? 0,
    drift: stale.map((a) => ({ node: a.node, name: a.name, reason: `push failed: ${pushError?.message}` })),
    pushFailures: stale.map((a) => ({ node: a.node, name: a.name, atMs: pushError!.atMs, message: pushError!.message, attempts: pushError!.attempts })),
    deadman: { configured: true, service: "healthchecks.io", lastPingMs: now - (Math.floor(now / 1000) % 60) * 1000, by: "cell1" },
    watched: { nodes: true, gpus: true, fabric: true, containers: true, endpoints: ["llama-3.3-70b", "Ollama"], switch: true, unwatched: ["Firewalla Switch X", "Firewalla Gold"] },
    silences: 0,
    pagingFailure: null,
    secretCopies: list.map((a) => ({ node: a.node, name: a.name, items: channels.filter((c) => c.enabled && !c.macOnly && Object.values(c.secrets).some(Boolean)).map((c) => `${c.name}: token`) })),
    demo: true,
  };
}

function channelViews(): Channel[] {
  const mac: Channel = { id: "mac", kind: "mac", name: "This Mac", enabled: notify.enabled, criticalOnly: notify.criticalOnly, secrets: {}, target: "Notification Center", last: null };
  const target = (c: Channel) => (c.kind === "ntfy" ? `${(c.server ?? "https://ntfy.sh").replace(/^https?:\/\//, "")} · ${c.topic ?? ""}` : c.kind === "heartbeat" ? "healthchecks.io" : c.kind === "email" ? c.to ?? "" : "");
  return [mac, ...channels.map((c) => ({ ...c, target: target(c), last: deliveries.find((d) => d.channel === c.id) ?? null }))];
}

export const handlers: MockHandlers = {
  notify_settings_get: () => notify,
  notify_settings_set: ({ settings }: { settings: NotifySettings }) => (notify = { ...notify, ...settings, changedMs: Date.now() }),
  notify_channels_get: () => channelViews(),
  notify_channels_set: ({ channels: next, secrets }: { channels: Channel[]; secrets: Record<string, string> }) => {
    const mac = next.find((c) => c.kind === "mac");
    if (mac) notify = { ...notify, enabled: mac.enabled, criticalOnly: mac.criticalOnly };
    channels = next
      .filter((c) => c.kind !== "mac")
      .map((c) => ({ ...c, secrets: Object.fromEntries(Object.keys(c.secrets ?? {}).concat(Object.keys(secrets).filter((k) => k.startsWith(`${c.id}.`)).map((k) => k.slice(c.id.length + 1))).map((f) => [f, true])) }));
    notify = { ...notify, changedMs: Date.now() };
    channelsChangedMs = Date.now();
    return channelViews();
  },
  notify_channel_test: ({ id, via }: { id: string; via: string | null }) => {
    const c = channels.find((x) => x.id === id);
    if (!c) return { ok: true, message: "Shown in Notification Center.", delivery: null };
    const d = row({ channel: c.id, channelName: c.name, kind: c.kind, title: via ? `SparkBox test from the agent on ${via}` : "SparkBox test", outcome: via ? "demo" : "sent", demo: !!via, via: via ? `agent:${via}` : "mac", test: true, latencyMs: 700 });
    deliveries = [d, ...deliveries].slice(0, 500);
    return { ok: true, message: via ? `Demo — the agent on ${via} is simulated, so nothing was sent.` : "Sent from this Mac.", delivery: d };
  },
  notify_deliveries: ({ limit }: { limit: number }) => deliveries.slice(0, limit),
  agent_status: () => watch(),
  agent_install: ({ nodes }: { nodes: string[] }) => {
    nodes.forEach((n) => installed.add(n));
    return "t-mock-agent";
  },
  agent_uninstall: ({ nodes }: { nodes: string[] }) => {
    installed = new Set([...installed].filter((n) => !nodes.includes(n)));
    return "t-mock-agent";
  },
  agent_push_config: () => {
    // One push at a time: a second call joins the one running.
    if (pushRunning) return { ...watch(), pushTask: pushRunning };
    const id = `t-mock-push-${++pushSeq}`;
    pushRunning = id;
    const n = installed.size;
    const base: Task = {
      id, kind: "agent-config", title: `Push alert settings to ${n} node${n === 1 ? "" : "s"}`, nodes: [...installed], source: "ui", cancellable: false, state: "running",
      startedMs: Date.now(), finishedMs: null, progress: null, detail: null, rateBps: null, etaS: null, steps: [], log: [], summary: null, error: null,
      cancelRequested: false, retry: null, opId: id,
    };
    ingestTask(base);
    window.setTimeout(() => {
      if (pushError) pushError = { ...pushError, atMs: Date.now(), attempts: pushError.attempts + 1 };
      const ok = !pushError || !installed.has("cell3");
      pushRunning = null;
      ingestTask(ok
        ? { ...base, state: "ok", finishedMs: Date.now(), summary: `Pushed to ${n} of ${n}` }
        : { ...base, state: "failed", finishedMs: Date.now(), error: `Pushed to ${n - 1} of ${n} · Failed on cell3: ${pushError!.message}` });
    }, 1500);
    return { ...watch(), pushTask: id };
  },
  agent_test_page: () => {
    tested = Date.now();
    const e = endToEnd(agents(tested));
    deliveries = [...e.cells.map((c) => row({ channel: c.channel, channelName: c.channelName, title: `Test from ${c.node}`, body: `Test from ${c.node} via ${c.channelName}: delivered in ${((c.ms ?? 0) / 1000).toFixed(1)} s`, outcome: "demo", via: `agent:${c.node}`, test: true, latencyMs: c.ms ?? undefined })), ...deliveries].slice(0, 500);
    return e;
  },
  agent_backfill_now: () => ({ points: 0, alerts: 0 }),
  agent_settings_get: () => agentSettings,
  agent_settings_set: ({ settings, token }: { settings: typeof agentSettings; token: string | null }) =>
    (agentSettings = { ...agentSettings, ...settings, hasToken: token === null ? agentSettings.hasToken : token !== "" }),
};
