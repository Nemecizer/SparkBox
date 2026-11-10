// Where alerts go (src-tauri/src/delivery and notify.rs): this Mac's
// Notification Center plus phone and team channels (ntfy, Pushover, a webhook
// or Slack, email) and the dead-man's switch, each with its last delivery, and
// the delivery log: one row per alert event and channel with what became of it.
// Secrets (tokens, keys, passwords) go to the Keychain and never come back:
// the UI only learns whether one is saved.
import { invoke } from "@tauri-apps/api/core";

export type ChannelKind = "mac" | "ntfy" | "pushover" | "webhook" | "email" | "heartbeat";

export interface Channel {
  id: string;
  kind: ChannelKind;
  name: string;
  enabled: boolean;
  /** Only critical alerts go to this channel. */
  criticalOnly: boolean;
  /** Sent from this Mac only: neither it nor its secrets are copied to the node agents. */
  macOnly?: boolean;
  // ntfy
  server?: string;
  topic?: string;
  // Pushover
  device?: string;
  // Webhook, dead-man's switch. The URL is a credential: it's typed as a secret ("url") and
  // only its host comes back.
  url?: string;
  urlHost?: string;
  /** Sends {"text": …} so a Slack (or Mattermost, Discord /slack) incoming webhook takes it. */
  slack?: boolean;
  // Email
  host?: string;
  port?: number;
  security?: "starttls" | "ssl";
  user?: string;
  from?: string;
  to?: string;
  /** Which secret fields are saved in the Keychain, e.g. { token: true }. */
  secrets: Record<string, boolean>;
  /** The same, as stored in notify.json (the backend keeps it; the UI doesn't edit it). */
  saved?: string[];
  /** Where it delivers, for the list: "ntfy.sh · sparkbox-alerts", "Notification Center", "healthchecks.io". */
  target?: string;
  /** The latest delivery to this channel, for "Sent 2 min ago" / "Failed: …". */
  last: Delivery | null;
}

/** What became of one alert event on one channel (or, with no channel, why it wasn't sent). */
export type Outcome = "sent" | "coalesced" | "duplicate" | "held" | "silenced" | "planned" | "eased" | "not-paged" | "failed" | "demo" | "agent" | "flapping" | "muted";

export interface Delivery {
  id: string;
  tsMs: number;
  /** "" when nothing was sent (held, duplicate, silenced…). */
  channel: string;
  channelName: string;
  kind: ChannelKind;
  title: string;
  body: string;
  severity: "info" | "warn" | "crit";
  ok: boolean;
  /** "HTTP 401 — check the token". */
  error: string | null;
  /** "mac", or "agent:cell1" for what a node agent sent. */
  via: string;
  /** Demo: recorded, not sent. */
  demo: boolean;
  test: boolean;
  attempts: number;
  /** Always set by this version's backend; absent in a round-two delivery log. */
  outcome?: Outcome;
  /** Shared by every event that went out in one (grouped) message. */
  messageId?: string;
  rule?: string;
  alertId?: string;
  incident?: string;
  /** Why it wasn't sent: "Quiet hours, 22:00 to 07:00 · America/New_York", a silence's reason. */
  reason?: string;
  /** A failure that will be tried again then, out of `maxAttempts` tries. */
  nextRetryMs?: number;
  maxAttempts?: number;
  latencyMs?: number;
}

/** Mirrors NotifySettings in src-tauri/src/notify.rs (notify.json). */
export interface NotifySettings {
  enabled: boolean;
  node: boolean;
  disk: boolean;
  gpu: boolean;
  fabric: boolean;
  containers: boolean;
  tasks: boolean;
  criticalOnly: boolean;
  resolved: boolean;
  quietHours: boolean;
  quietStart: string;
  quietEnd: string;
  /** The IANA zone quiet hours follow; captured from this Mac when they're saved. */
  quietTz: string;
  /** "Still firing" for unacknowledged critical alerts every this many minutes; 0 never. */
  repeatMinutes: number;
  /** When the channels or these settings last changed. */
  changedMs?: number;
}

/** The secret fields each kind has, and how they're labelled. A URL field is shown as typed. */
export const SECRET_FIELDS: Record<ChannelKind, { field: string; label: string; placeholder?: string }[]> = {
  mac: [],
  ntfy: [{ field: "token", label: "Access token" }],
  pushover: [{ field: "appToken", label: "App token" }, { field: "userKey", label: "User key" }],
  webhook: [{ field: "url", label: "URL", placeholder: "https://hooks.slack.com/services/…" }, { field: "secret", label: "Signing secret" }],
  email: [{ field: "password", label: "Password" }],
  heartbeat: [{ field: "url", label: "Ping URL", placeholder: "https://hc-ping.com/…" }],
};

export const KIND_LABEL: Record<ChannelKind, string> = {
  mac: "This Mac",
  ntfy: "ntfy",
  pushover: "Pushover",
  webhook: "Webhook",
  email: "Email",
  heartbeat: "Dead-man's switch",
};

/** The settings whose change points a channel somewhere else, so its saved secrets are cleared. */
export const DESTINATION_FIELDS: Record<ChannelKind, (keyof Channel)[]> = {
  mac: [],
  ntfy: ["server"],
  pushover: [],
  webhook: [],
  email: ["host", "port", "security", "user"],
  heartbeat: [],
};

/** The host of a URL typed into a secret field, as the backend keeps it ("hooks.slack.com"). */
function urlHost(url: string): string | null {
  try {
    return new URL(url.trim()).hostname || null;
  } catch {
    return null;
  }
}

const FIELD_WORD: Partial<Record<keyof Channel, string>> = { server: "server", url: "URL", host: "SMTP server", port: "port", security: "security", user: "user" };

/**
 * "Changing the server clears the saved token. Enter it again." when an edit points a channel
 * somewhere else and a saved secret wasn't typed again (the backend clears it on save).
 */
export function clearsSecretsNote(original: Channel | null, edited: Channel, typed: Record<string, string>): string | null {
  if (!original) return null;
  // A new URL on another host points the channel somewhere else too (the backend compares hosts).
  const newHost = typed.url ? urlHost(typed.url) : null;
  const changed = original.kind !== edited.kind ? "kind"
    : DESTINATION_FIELDS[edited.kind].find((f) => (original[f] ?? "") !== (edited[f] ?? ""))
      ?? (newHost && original.urlHost && newHost !== original.urlHost ? "url" : undefined);
  if (!changed) return null;
  const saved = SECRET_FIELDS[original.kind].filter((s) => original.secrets[s.field] && !typed[s.field]);
  if (!saved.length) return null;
  const what = changed === "kind" ? "kind" : FIELD_WORD[changed] ?? String(changed);
  const names = saved.map((s) => s.label.replace(/^\w/, (c) => c.toLowerCase()));
  const list = names.length > 1 ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}` : names[0];
  return `Changing the ${what} clears the saved ${list}. Enter it again.`;
}

export interface TestResult {
  ok: boolean;
  /** What happened, in a sentence: "Sent from this Mac." / "Failed: HTTP 401 — check the token". */
  message: string;
  delivery: Delivery | null;
}

export const notifyApi = {
  settings: () => invoke<NotifySettings>("notify_settings_get"),
  setSettings: (settings: NotifySettings) => invoke<NotifySettings>("notify_settings_set", { settings }),
  channels: () => invoke<Channel[]>("notify_channels_get"),
  /** `secrets` maps "<channel id>.<field>" to a new value; "" removes it. */
  setChannels: (channels: Channel[], secrets: Record<string, string> = {}) => invoke<Channel[]>("notify_channels_set", { channels, secrets }),
  /** via: undefined sends from this Mac; a node id sends from that node's agent. */
  test: (id: string, via?: string) => invoke<TestResult>("notify_channel_test", { id, via: via ?? null }),
  deliveries: (limit = 100) => invoke<Delivery[]>("notify_deliveries", { limit }),
};

const OUTCOME_WORD: Record<Outcome, string> = {
  sent: "Sent",
  coalesced: "Listed with its incident",
  duplicate: "Duplicate, not sent again",
  held: "Held",
  silenced: "Silenced",
  planned: "Planned work, not paged",
  eased: "Eased to warning, not paged",
  "not-paged": "Not paged",
  failed: "Failed",
  demo: "Demo — not sent",
  agent: "Sent by the node's agent",
  flapping: "Flapping, not paged again",
  muted: "Muted for this run, not sent",
};

/** The outcome in words: "Held · Quiet hours, 22:00 to 07:00 · America/New_York", "Sent by the agent on cell1". */
export function outcomeLabel(d: Delivery): string {
  const outcome = d.outcome ?? (d.demo ? "demo" : d.ok ? "sent" : "failed");
  if (outcome === "agent") return `Sent by the agent on ${d.via.replace(/^agent:/, "")}`;
  const word = OUTCOME_WORD[outcome] ?? outcome;
  return d.reason && outcome !== "sent" ? `${word} · ${d.reason}` : word;
}

/** "Sent 2 min ago", "Failed · retrying 2 of 4", "Failed: HTTP 401 — check the token", "Demo — not sent", "Never used". */
export function deliveryLine(d: Delivery | null, ago: (ms: number) => string): string {
  if (!d) return "Never used";
  const outcome = d.outcome ?? (d.demo ? "demo" : d.ok ? "sent" : "failed");
  if (outcome === "demo") return `Demo — not sent · ${ago(d.tsMs)}`;
  if (outcome === "failed") {
    if (d.nextRetryMs && d.maxAttempts) return `Failed · retrying ${d.attempts + 1} of ${d.maxAttempts}`;
    return `Failed: ${d.error ?? "unknown error"}`;
  }
  if (outcome === "sent") return `Sent ${ago(d.tsMs)}${d.test ? " (test)" : ""}${d.latencyMs != null && d.test ? ` in ${(d.latencyMs / 1000).toFixed(1)} s` : ""}`;
  return outcomeLabel(d);
}
