// Settings › Notifications: what paging rests on (a checklist from the watch badge),
// where alerts go (this Mac, ntfy, Pushover, a webhook or Slack, email, and the
// dead-man's switch), when they're sent, and the always-on agents that send them
// while SparkBox is closed. Channel secrets go to the Keychain and never come back;
// the editor says where they're copied and who can read them there. A new channel
// sends from this Mac only until you choose to copy it to the agents.
import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check as CheckIcon, Download, HeartPulse, Pencil, Plus, RefreshCw, Send, Trash2, Upload } from "lucide-react";
import { FormGroup, FormRow, HelpTip, InfoTip, Notice, SectionTitle, Segmented, StateGlyph, Switch, ToolButton } from "../../components/ui";
import {
  agentApi, agentHealthy, agentTone, agoWords, applyWatchState, clockWords, listWords, pushAgain, pushScope, pushWords, PUSH_WORDS, SCOPED_CREDENTIALS, SECRET_DISCLOSURE,
  sendTestPage, useWatchState, watchBadge, type AgentStatus, type EndToEnd, type WatchState,
} from "../../lib/agent";
import { ADMIN_REFUSED, unlockedFor, useAdminState, withAdmin } from "../../lib/admin";
import { inTauri } from "../../lib/api";
import { showContextMenu, type MenuItem } from "../../lib/contextMenu";
import { clearsSecretsNote, deliveryLine, KIND_LABEL, notifyApi, SECRET_FIELDS, type Channel, type ChannelKind, type NotifySettings } from "../../lib/notifyApi";
import { confirmAction } from "../../lib/session";
import { toast } from "../../lib/toasts";
import "./notifications.css";

const CATEGORIES: { key: keyof NotifySettings; label: string; hint: string }[] = [
  { key: "node", label: "Node status", hint: "Offline, back online, not updating, memory, clock, OOM kills" },
  { key: "disk", label: "Disk", hint: "Root disk full, or filling up within a day" },
  { key: "gpu", label: "GPU", hint: "Running hot, throttling, Xid errors" },
  { key: "fabric", label: "Fabric & switches", hint: "Links down or flapping, errors, MTU, weak optics, RoCE counters" },
  { key: "containers", label: "Containers, services & models", hint: "Crashes, crash loops, failed units, a model down or slow" },
  { key: "tasks", label: "Tasks", hint: "Long operations that finish while SparkBox is in the background (this Mac only)" },
];

const REPEATS: { value: string; label: string }[] = [
  { value: "15", label: "15 min" },
  { value: "30", label: "30 min" },
  { value: "60", label: "1 h" },
  { value: "240", label: "4 h" },
  { value: "0", label: "Never" },
];

/** The Help tip beside a channel's name, in the list and on its form. */
const KIND_HELP: Partial<Record<ChannelKind, string>> = {
  ntfy: "mon.notify.ntfy",
  pushover: "mon.notify.pushover",
  webhook: "mon.notify.webhook",
  email: "mon.notify.email",
};

const DEADMAN_INFO =
  "Point this at a check that pages you when pings stop (healthchecks.io, Uptime Kuma push, Cronitor). If the house loses power or internet, the pings stop and that service pages you.";

function errText(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

/** This Mac's IANA time zone, as the system reports it now. */
function macZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  } catch {
    return "";
  }
}

/**
 * A fresh channel of a kind, enabled, with a unique id. It sends from this Mac only: its
 * secrets reach the nodes only once you turn that off. The dead-man's switch is the agents'
 * own (they ping it), so it goes to them.
 */
function newChannel(kind: ChannelKind, taken: string[]): Channel {
  let n = 1;
  while (taken.includes(`${kind}-${n}`)) n += 1;
  const base: Channel = { id: `${kind}-${n}`, kind, name: KIND_LABEL[kind], enabled: true, criticalOnly: false, macOnly: kind !== "heartbeat", secrets: {}, last: null };
  if (kind === "ntfy") return { ...base, name: "Phone (ntfy)", server: "https://ntfy.sh", topic: "" };
  if (kind === "pushover") return { ...base, name: "Phone (Pushover)" };
  if (kind === "webhook") return { ...base, name: "Webhook", slack: false };
  if (kind === "heartbeat") return { ...base, name: "Dead-man's switch" };
  return { ...base, name: "Email", host: "", port: 587, security: "starttls", user: "", from: "", to: "" };
}

type Editing = { channel: Channel; original: Channel | null; secrets: Record<string, string> };

export function NotificationsSection() {
  const [s, setS] = useState<NotifySettings | null>(null);
  const [channels, setChannels] = useState<Channel[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const watch = useWatchState();
  const admin = useAdminState();
  const macLocked = !unlockedFor("mac", admin);
  const nodeLocked = !unlockedFor("node", admin);

  useEffect(() => {
    if (!inTauri) return;
    notifyApi.settings().then(setS, (e) => setError(errText(e)));
    notifyApi.channels().then(setChannels, (e) => setError(errText(e)));
    agentApi.status().then(applyWatchState, () => {});
    // Last deliveries and heartbeats move on their own; follow them while the section is open.
    const t = window.setInterval(() => {
      setNow(Date.now());
      notifyApi.channels().then(setChannels, () => {});
      agentApi.status().then(applyWatchState, () => {});
    }, 5000);
    return () => window.clearInterval(t);
  }, []);

  const refreshWatch = () => agentApi.status().then(applyWatchState, () => {});

  const save = async (patch: Partial<NotifySettings>) => {
    if (!s) return;
    const next = { ...s, ...patch };
    try {
      setS(await withAdmin(() => notifyApi.setSettings(next), { scope: "mac", action: "notify_settings_set" }));
      setError(null);
      void refreshWatch();
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(errText(e));
    }
  };

  /** Saves the whole list (Mac scope: it's where alerts go, and secrets go to the Keychain). */
  const saveChannels = async (list: Channel[], secrets: Record<string, string> = {}) => {
    try {
      const saved = await withAdmin(() => notifyApi.setChannels(list, secrets), { scope: "mac", action: "notify_channels_set" });
      setChannels(saved);
      setError(null);
      void refreshWatch();
      return true;
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(errText(e));
      return false;
    }
  };

  const test = async (c: Channel, via?: string) => {
    setBusy(c.id);
    try {
      const r = via ? await withAdmin(() => notifyApi.test(c.id, via), { action: "notify_channel_test" }) : await notifyApi.test(c.id);
      toast({ kind: r.ok ? "success" : "error", title: r.ok ? `Test sent to ${c.name}` : `Test to ${c.name} failed`, body: r.message });
      setChannels(await notifyApi.channels());
    } catch (e) {
      if (e !== ADMIN_REFUSED) toast({ kind: "error", title: `Test to ${c.name} failed`, body: errText(e) });
    } finally {
      setBusy(null);
    }
  };

  const beating = watch.agents.filter((a) => agentHealthy(a, now));
  const testMenu = (c: Channel, e: React.MouseEvent) => {
    if (c.kind === "mac" || c.kind === "heartbeat" || c.macOnly || !beating.length) {
      void test(c);
      return;
    }
    const items: MenuItem[] = [
      { label: "Send test from this Mac", icon: Send, run: () => test(c) },
      { separator: true },
      ...beating.map((a) => ({ label: `Send test from the agent on ${a.name}`, run: () => test(c, a.node) })),
    ];
    showContextMenu(e, items);
  };

  // One push at a time, one toast when it ends (lib/agent pushAgain, shared with the top bar).
  const push = (nodes?: string[]) => void pushAgain(nodes);

  const [matrix, setMatrix] = useState<EndToEnd | null>(null);
  const testPage = async () => {
    const r = await sendTestPage(watch);
    if (r) setMatrix(r);
  };

  const list = channels ?? [];
  const mac = list.find((c) => c.kind === "mac");
  const remote = list.filter((c) => c.kind !== "mac");
  const installed = watch.agents.filter((a) => a.installed);
  const startEdit = (c: Channel) => setEditing({ channel: { ...c }, original: c, secrets: {} });
  const startNew = (kind: ChannelKind) => setEditing({ channel: newChannel(kind, list.map((c) => c.id)), original: null, secrets: {} });
  const submit = async () => {
    if (!editing) return;
    const secrets = Object.fromEntries(Object.entries(editing.secrets).filter(([, v]) => v !== "").map(([k, v]) => [`${editing.channel.id}.${k}`, v]));
    const next = editing.original ? list.map((x) => (x.id === editing.channel.id ? editing.channel : x)) : [...list, editing.channel];
    if (await saveChannels(next, secrets)) setEditing(null);
  };

  return (
    <div className="ntf">
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
      <PagingStatus watch={watch} now={now} nodeLocked={nodeLocked} macLocked={macLocked} onPush={() => push()} onTest={testPage} onDeadman={() => startNew("heartbeat")} />

      <section className="form-group">
        <SectionTitle
          title="Where alerts go"
          infoTopic="mon.notify.channels"
        />
        <div className="form-group-card ntf-list">
          {mac && (
            <div className="ntf-row">
              <div className="ntf-main">
                <div className="ntf-name"><span>This Mac</span><span className="ntf-kind">Notification Center</span></div>
                <div className="ntf-sub">Sends from: this Mac · macOS asks for permission the first time there's something to say</div>
              </div>
              <div className="ntf-actions">
                <button type="button" className="btn small" onClick={() => test(mac)} disabled={busy === mac.id}>Test</button>
                <Switch label="Show notifications on this Mac" checked={mac.enabled} disabled={!channels} onChange={(on) => saveChannels(list.map((c) => (c.id === "mac" ? { ...c, enabled: on } : c)))} />
              </div>
            </div>
          )}
          {(watch.channels ?? []).filter((c) => c.simulated).map((c) => (
            <div key={c.id} className="ntf-row">
              <div className="ntf-main">
                <div className="ntf-name"><span>{c.name}</span><span className="ntf-kind">{c.kind} · simulated</span></div>
                <div className="ntf-sub">
                  {c.remote
                    ? `Sends from: this Mac and ${installed.length} node${installed.length === 1 ? "" : "s"} · Demo — never saved, nothing is sent${c.lastOkMs ? ` · test page ${agoWords(c.lastOkMs, now)}` : ""}`
                    : `Pinged by the node agents${c.lastOkMs ? ` · last ping ${agoWords(c.lastOkMs, now)}` : ""} · Demo — never saved, nothing is pinged`}
                </div>
              </div>
            </div>
          ))}
          {remote.map((c) => (
            <ChannelRow
              key={c.id}
              c={c}
              watch={watch}
              now={now}
              busy={busy === c.id}
              locked={macLocked}
              editing={editing?.original?.id === c.id}
              onToggle={(on) => saveChannels(list.map((x) => (x.id === c.id ? { ...x, enabled: on } : x)))}
              onTest={(e) => testMenu(c, e)}
              onEdit={() => startEdit(c)}
              onRemove={async () => {
                const ok = await confirmAction({ title: `Remove ${c.name}?`, body: "Alerts stop going there, and its saved secrets are removed from the Keychain and, with the next push, from the nodes.", confirmLabel: "Remove", danger: true });
                if (ok) await saveChannels(list.filter((x) => x.id !== c.id));
              }}
            >
              {editing?.original?.id === c.id && <ChannelForm value={editing} agents={installed} onChange={setEditing} onCancel={() => setEditing(null)} onSave={submit} />}
            </ChannelRow>
          ))}
          {editing && !editing.original && (
            <div className="ntf-row">
              <div className="ntf-main">
                <div className="ntf-name"><span>New {KIND_LABEL[editing.channel.kind]}{editing.channel.kind === "heartbeat" ? "" : " channel"}</span>{KIND_HELP[editing.channel.kind] && <InfoTip topic={KIND_HELP[editing.channel.kind]} />}</div>
              </div>
              <ChannelForm value={editing} agents={installed} onChange={setEditing} onCancel={() => setEditing(null)} onSave={submit} />
            </div>
          )}
          <div className="ntf-add">
            {(["ntfy", "pushover", "webhook", "email"] as const).map((k) => (
              <ToolButton key={k} icon={Plus} label={KIND_LABEL[k]} scope="mac" action="notify_channels_set" locked={macLocked} disabled={!channels || !!editing} onClick={() => startNew(k)} />
            ))}
            {!remote.some((c) => c.kind === "heartbeat") && (
              <ToolButton icon={HeartPulse} label="Dead-man's switch" scope="mac" action="notify_channels_set" locked={macLocked} disabled={!channels || !!editing} onClick={() => startNew("heartbeat")} />
            )}
          </div>
          <div className="ntf-secrets">
            <div className="ntf-scoped" role="note">
              <p><span className="ntf-scoped-title">New channels send from this Mac only.</span> A channel you copy to the node agents has its secrets on every node: {SECRET_DISCLOSURE.charAt(0).toLowerCase() + SECRET_DISCLOSURE.slice(1)}</p>
              <p>{SCOPED_CREDENTIALS}</p>
            </div>
          </div>
        </div>
      </section>

      <WhenGroup s={s} save={save} />

      <AgentsGroup now={now} watch={watch} nodeLocked={nodeLocked} macLocked={macLocked} matrix={matrix} onPush={push} onTest={testPage} />
    </div>
  );
}

// ---- What paging rests on

function Check({ ok, neutral, children, action }: { ok: boolean; neutral?: boolean; children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <li className={`ntf-check${ok ? " ok" : neutral ? "" : " warn"}`}>
      {ok ? <CheckIcon size={13} className="ntf-check-mark" aria-label="Done" /> : <StateGlyph state={neutral ? "affected" : "warn"} label={neutral ? "Note" : "Not yet"} />}
      <span className="ntf-check-text">{children}</span>
      {action && <span className="ntf-check-action">{action}</span>}
    </li>
  );
}

function PagingStatus({ watch, now, nodeLocked, macLocked, onPush, onTest, onDeadman }: {
  watch: WatchState;
  now: number;
  nodeLocked: boolean;
  macLocked: boolean;
  onPush: () => void;
  onTest: () => Promise<void>;
  onDeadman: () => void;
}) {
  const badge = watchBadge(watch, now);
  const installed = watch.agents.filter((a) => a.installed);
  const phones = (watch.channels ?? []).filter((c) => c.remote && c.enabled && !c.macOnly);
  const answering = installed.filter((a) => agentHealthy(a, now));
  const current = installed.filter((a) => a.configCurrent === true);
  const e2e = watch.endToEnd;
  const testedNodes = new Set((e2e?.cells ?? []).filter((c) => c.ok && c.atMs >= (watch.changedMs ?? 0)).map((c) => c.node));
  const d = watch.deadman;
  const unwatched = watch.watched?.unwatched ?? [];
  const failedCell = (e2e?.cells ?? []).find((c) => !c.ok);
  return (
    <section className="form-group">
      <SectionTitle
        title="Paging"
        infoTopic="mon.notify.paging"
        actions={<span className={`ntf-badge tone-${badge.tone}`} title={badge.lines.join("\n")}>{badge.label}</span>}
      />
      <div className="form-group-card">
        <ul className="ntf-checks">
          {!installed.length ? (
            <Check ok={false}>No node agents: alerts reach you only while SparkBox runs on this Mac.</Check>
          ) : (
            <>
              <Check ok={phones.length > 0}>{phones.length ? `Phone channel: ${listWords(phones.map((c) => c.name))}` : "No phone channel the agents can use"}</Check>
              <Check
                ok={!!e2e?.ok}
                action={<ToolButton icon={Send} label="Send a test page from every node…" locked={nodeLocked} action="send a test page from every node" disabled={!phones.length} onClick={onTest} />}
              >
                {e2e?.ok
                  ? `Test page from ${testedNodes.size} of ${installed.length} nodes · ${agoWords(e2e.testedAtMs ?? now, now)}`
                  : failedCell
                    ? `The last test page didn't reach ${failedCell.channelName} from ${watch.agents.find((a) => a.node === failedCell.node)?.name ?? failedCell.node}`
                    : e2e?.testedAtMs
                      ? `Not tested since the settings changed (last test page ${agoWords(e2e.testedAtMs, now)})`
                      : "No test page from the nodes yet"}
              </Check>
              {answering.length < installed.length && (
                <Check ok={false}>
                  {`${answering.length} of ${installed.length} agents answering · ${listWords(installed.filter((a) => !agentHealthy(a, now)).map((a) => a.name))} watched only while this Mac is awake`}
                </Check>
              )}
              <Check
                ok={current.length === installed.length}
                action={current.length < installed.length ? <ToolButton icon={Upload} label="Push again" locked={pushScope(watch) === "mac" ? macLocked : nodeLocked} scope={pushScope(watch)} action={PUSH_WORDS} onClick={onPush} /> : undefined}
              >
                {`${current.length} of ${installed.length} agents on current settings`}
              </Check>
              <Check ok={!!d?.configured} action={d?.configured ? undefined : <ToolButton icon={HeartPulse} label="Set up…" scope="mac" locked={macLocked} action="notify_channels_set" onClick={onDeadman} />}>
                {d?.configured
                  ? `Dead-man's switch · ${d.service ?? "set up"}${d.lastPingMs ? ` · pinged by ${watch.agents.find((a) => a.node === d.by)?.name ?? d.by} ${agoWords(d.lastPingMs, now)}` : " · not pinged yet"}`
                  : "No dead-man's switch"}
              </Check>
              {unwatched.length > 0 && <Check ok={false} neutral>{`${listWords(unwatched)} watched only while this Mac is awake`}</Check>}
            </>
          )}
        </ul>
        {badge.tone === "warn" && badge.lines.length > 0 && <p className="ntf-check-why">{badge.lines.join(" ")}</p>}
      </div>
    </section>
  );
}

// ---- Channels

function sendsFrom(c: Channel, agents: AgentStatus[]): string {
  if (c.kind === "heartbeat") return agents.length ? "Pinged by the node agents" : "Needs node agents";
  const n = agents.length;
  if (c.macOnly || !n) return "This Mac only";
  return `Sends from: this Mac and ${n} node${n === 1 ? "" : "s"}`;
}

function ChannelRow({ c, watch, now, busy, locked, editing, onToggle, onTest, onEdit, onRemove, children }: {
  c: Channel;
  watch: WatchState;
  now: number;
  busy: boolean;
  locked: boolean;
  editing: boolean;
  onToggle: (on: boolean) => void;
  onTest: (e: React.MouseEvent) => void;
  onEdit: () => void;
  onRemove: () => void;
  children?: React.ReactNode;
}) {
  const installed = watch.agents.filter((a) => a.installed);
  const failed = c.last?.outcome === "failed" || (c.last && !c.last.ok);
  const ago = (ms: number) => agoWords(ms, now);
  let line = `${sendsFrom(c, installed)} · ${deliveryLine(c.last, ago)}`;
  if (c.kind === "heartbeat") {
    const d = watch.deadman;
    const by = d?.by ? watch.agents.find((a) => a.node === d.by)?.name ?? d.by : null;
    line = !installed.length ? "Needs node agents" : d?.lastPingMs && by ? `Pinged by ${by} ${ago(d.lastPingMs)}` : "Not pinged by an agent yet";
  }
  return (
    <div className="ntf-row">
      <div className="ntf-main">
        <div className="ntf-name">
          <span>{c.name}</span>
          <span className="ntf-kind">{KIND_LABEL[c.kind]}{c.target ? ` · ${c.target}` : ""}{c.criticalOnly ? " · critical only" : ""}</span>
          {c.kind === "heartbeat" && <InfoTip topic="mon.notify.deadman" label="About the dead-man's switch" />}
          {KIND_HELP[c.kind] && <InfoTip topic={KIND_HELP[c.kind]} />}
        </div>
        <div className={`ntf-sub${failed ? " failed" : ""}`} title={c.last?.error ?? undefined}>{line}{failed && c.last?.error && c.last.nextRetryMs ? ` · ${c.last.error}` : ""}</div>
      </div>
      <div className="ntf-actions">
        <button type="button" className="btn small" onClick={onTest} disabled={busy} aria-haspopup={c.kind === "mac" || c.kind === "heartbeat" ? undefined : "menu"}>{busy ? "Sending…" : "Test"}</button>
        <ToolButton icon={Pencil} label="Edit" scope="mac" action="notify_channels_set" locked={locked} disabled={editing} onClick={onEdit} />
        <ToolButton icon={Trash2} label="Remove" title={`Remove ${c.name}`} scope="mac" action="notify_channels_set" locked={locked} onClick={onRemove} />
        <Switch label={c.kind === "heartbeat" ? `Let the agents ping ${c.name}` : `Send alerts to ${c.name}`} checked={c.enabled} onChange={onToggle} />
      </div>
      {children}
    </div>
  );
}

function ChannelForm({ value, agents, onChange, onSave, onCancel }: { value: Editing; agents: AgentStatus[]; onChange: (v: Editing) => void; onSave: () => void; onCancel: () => void }) {
  const c = value.channel;
  const set = (patch: Partial<Channel>) => onChange({ ...value, channel: { ...c, ...patch } });
  const secret = (field: string, v: string) => onChange({ ...value, secrets: { ...value.secrets, [field]: v } });
  const id = (f: string) => `ntf-${c.id}-${f}`;
  const text = (f: keyof Channel, label: string, placeholder = "", type = "text") => (
    <>
      <label htmlFor={id(String(f))}>{label}</label>
      <input id={id(String(f))} className="field" type={type} placeholder={placeholder} value={String(c[f] ?? "")}
        onChange={(e) => set({ [f]: type === "number" ? Number(e.target.value) || undefined : e.target.value } as Partial<Channel>)} />
    </>
  );
  const clears = clearsSecretsNote(value.original, value.channel, value.secrets);
  const hasSecrets = SECRET_FIELDS[c.kind].length > 0;
  const names = listWords(agents.map((a) => a.name));
  return (
    <form className="ntf-edit" onSubmit={(e) => { e.preventDefault(); onSave(); }}>
      {text("name", "Name")}
      {c.kind === "ntfy" && (
        <>
          {text("server", "Server", "https://ntfy.sh")}
          {text("topic", "Topic", "a long, hard-to-guess name")}
        </>
      )}
      {c.kind === "pushover" && text("device", "Device", "All devices")}
      {c.kind === "webhook" && (
        <>
          <label htmlFor={id("slack")}>Slack-compatible</label>
          <Switch id={id("slack")} label="Slack-compatible" checked={!!c.slack} onChange={(on) => set({ slack: on })} />
        </>
      )}
      {c.kind === "heartbeat" && (
        <p className="ntf-edit-note">{DEADMAN_INFO} The leader node's agent pings it every minute; https is needed unless it's on your network.</p>
      )}
      {c.kind === "email" && (
        <>
          {text("host", "SMTP server", "smtp.gmail.com")}
          {text("port", "Port", "587", "number")}
          <label id={id("security")}>Security</label>
          <Segmented<"starttls" | "ssl"> label="Security" value={c.security ?? "starttls"} onChange={(v) => set({ security: v, port: v === "ssl" ? 465 : 587 })}
            options={[{ value: "starttls", label: "STARTTLS" }, { value: "ssl", label: "SSL" }]} />
          {text("user", "User", "you@example.com")}
          {text("from", "From", "SparkBox <you@example.com>")}
          {text("to", "To", "you@example.com, someone@example.com")}
        </>
      )}
      {SECRET_FIELDS[c.kind].map(({ field, label, placeholder }) => (
        <SecretField key={field} id={id(field)} label={label} placeholder={placeholder} saved={!!c.secrets[field] && !clears} value={value.secrets[field] ?? ""} onChange={(v) => secret(field, v)} />
      ))}
      {clears && <p className="ntf-edit-note warn" role="status">{clears}</p>}
      {c.kind !== "heartbeat" && (
        <>
          <label htmlFor={id("crit")}>Critical only</label>
          <Switch id={id("crit")} label="Only critical alerts to this channel" checked={c.criticalOnly} onChange={(on) => set({ criticalOnly: on })} />
          <label htmlFor={id("maconly")}>This Mac only <HelpTip topic="mon.notify.mac-only" /></label>
          <Switch id={id("maconly")} label="Send from this Mac only (not copied to the nodes)" showLabel checked={!!c.macOnly} onChange={(on) => set({ macOnly: on })} />
        </>
      )}
      {hasSecrets && c.macOnly && (
        <p className="ntf-edit-note">Kept in this Mac's Keychain only. The agents don't page through this channel, so it reaches you only while SparkBox runs here.</p>
      )}
      {hasSecrets && !c.macOnly && (
        <div className="ntf-scoped" role="note">
          <p className="ntf-scoped-title">{agents.length ? `Copied to the agents on ${names}` : "Copied to the node agents once they're installed"}</p>
          <p>{SECRET_DISCLOSURE}</p>
          <p>{SCOPED_CREDENTIALS}</p>
        </div>
      )}
      <div className="ntf-edit-actions">
        <button type="button" className="btn" onClick={onCancel}>Cancel</button>
        <button type="submit" className="btn primary">{value.original ? "Save" : c.kind === "heartbeat" ? "Add" : "Add channel"}</button>
      </div>
    </form>
  );
}

/** A Keychain field: what's saved never comes back. A URL (`placeholder` set) is shown as it's typed. */
function SecretField({ id, label, placeholder, saved, value, onChange }: { id: string; label: string; placeholder?: string; saved: boolean; value: string; onChange: (v: string) => void }) {
  return (
    <>
      <label htmlFor={id}>{label}</label>
      <input id={id} className="field" type={placeholder ? "url" : "password"} autoComplete="off" spellCheck={false}
        placeholder={saved ? "Saved in the Keychain · type to replace" : placeholder ?? "Kept in the Keychain"} value={value} onChange={(e) => onChange(e.target.value)} />
    </>
  );
}

// ---- When to notify

function WhenGroup({ s, save }: { s: NotifySettings | null; save: (p: Partial<NotifySettings>) => void }) {
  const zone = macZone();
  const tz = s?.quietTz || zone;
  const moved = !!s?.quietHours && !!s.quietTz && !!zone && zone !== s.quietTz;
  return (
    <FormGroup title="When to notify">
      <FormRow label={<>Critical alerts only <HelpTip topic="mon.notify.critical-only" /></>} htmlFor="ntf-crit" hint="Warnings wait in the Alerts window">
        <Switch id="ntf-crit" label="Critical alerts only" checked={!!s?.criticalOnly} disabled={!s} onChange={(on) => save({ criticalOnly: on })} />
      </FormRow>
      <FormRow label="When an alert clears" htmlFor="ntf-resolved" hint="For example “cell3 is back online”">
        <Switch id="ntf-resolved" label="Notify when an alert clears" checked={!!s?.resolved} disabled={!s} onChange={(on) => save({ resolved: on })} />
      </FormRow>
      <FormRow label={<>Repeat critical alerts <HelpTip topic="mon.notify.repeat" /></>} hint="“Still firing” until it's acknowledged or silenced">
        <Segmented label="Repeat critical alerts every" value={String(s?.repeatMinutes ?? 60)} onChange={(v) => save({ repeatMinutes: Number(v) })} options={REPEATS} />
      </FormRow>
      {CATEGORIES.map((c) => (
        <FormRow key={c.key} label={c.label} htmlFor={`ntf-${c.key}`} hint={c.hint}>
          <Switch id={`ntf-${c.key}`} label={c.label} checked={!!s?.[c.key]} disabled={!s} onChange={(on) => save({ [c.key]: on })} />
        </FormRow>
      ))}
      <FormRow
        label={<>Quiet hours <HelpTip topic="mon.notify.quiet" /></>}
        htmlFor="ntf-quiet"
        hint={s?.quietHours ? `${s.quietStart} to ${s.quietEnd} · ${tz}` : "Only critical alerts and their resolves, in one time zone"}
      >
        <div className="ntf-inline">
          <input id="ntf-from" aria-label="Quiet from" type="time" className="field" value={s?.quietStart ?? "22:00"} disabled={!s || !s.quietHours}
            onChange={(e) => e.target.value && save({ quietStart: e.target.value, quietTz: tz })} />
          <span className="muted">to</span>
          <input id="ntf-until" aria-label="Quiet until" type="time" className="field" value={s?.quietEnd ?? "07:00"} disabled={!s || !s.quietHours}
            onChange={(e) => e.target.value && save({ quietEnd: e.target.value, quietTz: tz })} />
          <Switch id="ntf-quiet" label="Quiet hours" checked={!!s?.quietHours} disabled={!s} onChange={(on) => save({ quietHours: on, quietTz: s?.quietTz || zone })} />
        </div>
      </FormRow>
      {moved && (
        <div className="ntf-zone">
          <Notice tone="info" action={{ label: `Use ${zone}`, onClick: () => save({ quietTz: zone }) }}>
            This Mac is now in {zone}. Quiet hours still follow {s?.quietTz}.
          </Notice>
        </div>
      )}
    </FormGroup>
  );
}

// ---- Always-on monitoring

interface AgentSettingsView {
  metricsEnabled: boolean;
  metricsPort: number;
  port: number;
  hasToken: boolean;
}

/** "cell3 · agent 1.1.0 · running · settings out of date (push failed 10:42: sudo refused)". */
function agentLine(a: AgentStatus, now: number): string {
  if (!a.installed) return a.state === "unknown" ? (a.error ? `Couldn't check: ${a.error}` : "Not checked yet") : "Not installed";
  const parts = [`agent ${a.version ?? "?"}`, a.state === "failed" ? "not answering" : a.state];
  if (a.pushError) parts.push(`settings out of date (push failed ${clockWords(a.pushError.atMs)}: ${pushWords(a.pushError.message)})`);
  else if (a.configCurrent === false) parts.push(a.configHash ? "settings out of date" : "can't report its settings: reinstall it");
  if (a.historyFromMs) {
    const days = Math.round((now - a.historyFromMs) / 86_400_000);
    parts.push(days >= 1 ? `history ${days} day${days === 1 ? "" : "s"}` : `history ${Math.max(1, Math.floor((now - a.historyFromMs) / 3_600_000))} h`);
  }
  if (a.lastBeatMs) parts.push(`heartbeat ${agoWords(a.lastBeatMs, now)}`);
  if (a.endpointsProbed?.length) parts.push(`checks ${a.endpointsProbed.length} endpoint${a.endpointsProbed.length === 1 ? "" : "s"}`);
  return parts.join(" · ");
}

function AgentsGroup({ now, watch, nodeLocked, macLocked, matrix, onPush, onTest }: {
  now: number;
  watch: WatchState;
  nodeLocked: boolean;
  macLocked: boolean;
  matrix: EndToEnd | null;
  onPush: (nodes?: string[]) => void;
  onTest: () => Promise<void>;
}) {
  const [settings, setSettings] = useState<AgentSettingsView | null>(null);
  const [token, setToken] = useState("");
  const [port, setPort] = useState<string>("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    invoke<AgentSettingsView>("agent_settings_get").then((v) => {
      setSettings(v);
      setPort(String(v.metricsPort));
    }, (e) => setError(errText(e)));
  }, []);

  const missing = watch.agents.filter((a) => !a.installed).map((a) => a.node);
  const all = watch.agents.map((a) => a.node);
  // An install that copies your secrets unlocks the Mac scope from the start: one prompt.
  const installLock = (nodes: string[]) => {
    const scope = pushScope(watch, nodes);
    return { scope, locked: scope === "mac" ? macLocked : nodeLocked, action: "install the always-on agent" };
  };

  const run = async (what: "install" | "remove", nodes: string[]) => {
    const picked = watch.agents.filter((a) => nodes.includes(a.node));
    const names = picked.map((a) => a.name);
    const copies = (watch.secretCopies ?? []).filter((c) => nodes.includes(c.node));
    const ok = what === "install"
      ? await confirmAction({
        title: `Install the always-on agent on ${names.length === 1 ? names[0] : `${names.length} nodes`}?`,
        body: (
          <>
            <p>SparkBox sends the agent as one sudo step that checks every file (SHA-256) as root, installs it in /opt/sparkbox-agent and starts it as a systemd service running as its own sparkbox-agent user. It keeps 180 days of history on the node and pages the channels above while SparkBox is closed. It changes nothing else.</p>
            <p>The service is in the docker group, which is root-equivalent: the container rules need `docker ps` and `docker inspect`.</p>
            {copies.some((c) => c.items.length) ? (
              <>
                <ul className="ntf-copies">
                  {copies.map((c) => <li key={c.node}><b>{c.name}</b>: {c.items.length ? c.items.join(", ") : "no secrets"}</li>)}
                </ul>
                <div className="ntf-scoped" role="note">
                  <p className="ntf-scoped-title">Who can read them on the nodes</p>
                  <p>{SECRET_DISCLOSURE}</p>
                  <p>{SCOPED_CREDENTIALS}</p>
                </div>
              </>
            ) : <p>No channel secrets are copied: no channel the agents use has one. Channels you add send from this Mac only until you choose otherwise.</p>}
          </>
        ),
        confirmLabel: "Install",
      })
      : await confirmAction({
        title: `Remove the agent from ${names.length === 1 ? names[0] : `${names.length} nodes`}?`,
        body: "Its service, settings, the copied secrets and the history it kept on the node are deleted. Nothing watches those nodes while SparkBox is closed.",
        confirmLabel: "Remove",
        danger: true,
      });
    if (!ok) return;
    try {
      // An install that copies your secrets needs the Mac scope (agent_install's gate): asked once, up front.
      const scope = what === "install" ? pushScope(watch, nodes) : "node";
      await withAdmin(() => (what === "install" ? agentApi.install(nodes) : agentApi.uninstall(nodes)), { action: what === "install" ? "agent_install" : "agent_uninstall", scope });
      toast({ kind: "info", title: what === "install" ? `Installing the agent on ${listWords(names)}` : `Removing the agent from ${listWords(names)}`, body: "Progress is in Tasks." });
      window.setTimeout(() => agentApi.status().then(applyWatchState, () => {}), 4000);
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(errText(e));
    }
  };

  const saveMetrics = async (patch: Partial<AgentSettingsView>, newToken?: string) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
    try {
      const v = await withAdmin(
        () => invoke<AgentSettingsView>("agent_settings_set", { settings: { metricsEnabled: next.metricsEnabled, metricsPort: next.metricsPort, port: next.port }, token: newToken ?? null }),
        { scope: "mac", action: "agent_settings_set" },
      );
      setSettings(v);
      setToken("");
      setError(null);
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(errText(e));
    }
  };

  const healthyCount = useMemo(() => watch.agents.filter((a) => agentHealthy(a, now)).length, [watch, now]);
  const shown = matrix ?? watch.endToEnd ?? null;
  const phones = (watch.channels ?? []).filter((c) => c.remote && c.enabled && !c.macOnly);
  return (
    <section className="form-group">
      <SectionTitle
        title="Always-on monitoring"
        infoTopic="mon.notify.always-on"
        actions={<span className="muted">{watch.agents.length ? `${healthyCount} of ${watch.agents.length} watching` : ""}</span>}
      />
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
      <div className="form-group-card ntf-list">
        {watch.agents.map((a) => {
          const tone = agentTone(a, now);
          const glyph = !a.installed ? "affected" : tone === "warn" ? "warn" : "ok";
          return (
            <div key={a.node} className={`ntf-row ntf-agent${tone === "warn" ? " warn" : ""}`}>
              <div className="ntf-main">
                <div className="ntf-name"><StateGlyph state={glyph} label={tone === "warn" ? "Warning" : a.installed ? "Watching" : "Not installed"} /><span>{a.name}</span></div>
                <div className={`ntf-sub${tone === "warn" ? " warn" : ""}`} title={a.pushError?.message ?? a.error ?? undefined}>
                  {agentLine(a, now)}{a.error && a.installed && a.state !== "running" ? ` · ${a.error}` : ""}
                </div>
              </div>
              <div className="ntf-actions">
                {a.installed && (a.configCurrent === false || a.pushError) && (
                  <ToolButton icon={Upload} label="Push again" locked={pushScope(watch, [a.node]) === "mac" ? macLocked : nodeLocked} scope={pushScope(watch, [a.node])} action={PUSH_WORDS} onClick={() => onPush([a.node])} />
                )}
                {a.installed ? (
                  <>
                    <ToolButton icon={RefreshCw} label="Reinstall…" title="Send the current agent, rules and channels again and restart it" {...installLock([a.node])} onClick={() => run("install", [a.node])} />
                    <ToolButton icon={Trash2} label="Remove…" locked={nodeLocked} action="remove the always-on agent" danger onClick={() => run("remove", [a.node])} />
                  </>
                ) : (
                  <ToolButton icon={Download} label="Install…" {...installLock([a.node])} onClick={() => run("install", [a.node])} />
                )}
              </div>
            </div>
          );
        })}
        {!watch.agents.length && (
          <div className="ntf-row"><div className="ntf-sub">Add nodes in Settings › Nodes first.</div></div>
        )}
        <div className="ntf-add">
          <ToolButton icon={Download} {...installLock(missing.length ? missing : all)} disabled={!all.length}
            label={missing.length && missing.length < all.length ? `Install on ${missing.length} more node${missing.length === 1 ? "" : "s"}…` : "Install on all nodes…"}
            onClick={() => run("install", missing.length ? missing : all)} />
          <ToolButton icon={Send} label="Send a test page from every node…" locked={nodeLocked} action="send a test page from every node" disabled={!watch.agents.some((a) => a.installed) || !phones.length} onClick={onTest} />
        </div>
        {shown && shown.cells.length > 0 && <TestMatrix e2e={shown} watch={watch} now={now} />}
      </div>
      <div className="form-group-card ntf-list">
        <div className="ntf-row">
          <div className="ntf-main">
            <div className="ntf-name">
              <span>Expose metrics for Prometheus</span>
              <HelpTip topic="mon.notify.metrics" label="About the metrics endpoint" />
            </div>
            <div className="ntf-sub">{settings?.metricsEnabled ? `http://<node>:${settings.metricsPort}/metrics${settings.hasToken ? " · token required" : ""}` : "Off"}</div>
          </div>
          <div className="ntf-actions">
            <Switch label="Expose metrics for Prometheus" checked={!!settings?.metricsEnabled} disabled={!settings} onChange={(on) => saveMetrics({ metricsEnabled: on })} />
          </div>
          {settings?.metricsEnabled && (
            <form className="ntf-edit" onSubmit={(e) => { e.preventDefault(); void saveMetrics({ metricsPort: Number(port) || settings.metricsPort }, token || undefined); }}>
              <label htmlFor="ntf-metrics-port">Port</label>
              <input id="ntf-metrics-port" className="field" type="number" min={1024} max={65535} value={port} onChange={(e) => setPort(e.target.value)} />
              <SecretField id="ntf-metrics-token" label="Bearer token" saved={settings.hasToken} value={token} onChange={setToken} />
              <div className="ntf-edit-actions">
                {settings.hasToken && <button type="button" className="btn" onClick={() => saveMetrics({}, "")}>Remove token</button>}
                <button type="submit" className="btn primary">Save</button>
              </div>
            </form>
          )}
        </div>
      </div>
    </section>
  );
}

/** The last test page: one row per node, one column per channel. */
function TestMatrix({ e2e, watch, now }: { e2e: EndToEnd; watch: WatchState; now: number }) {
  const channels = [...new Map(e2e.cells.map((c) => [c.channel, c.channelName])).entries()];
  const nodes = [...new Set(e2e.cells.map((c) => c.node))];
  const at = Math.max(...e2e.cells.map((c) => c.atMs));
  const ok = e2e.cells.filter((c) => c.ok).length;
  return (
    <div className="ntf-matrix">
      <div className="ntf-matrix-head">Test page {agoWords(at, now)} · {ok} of {e2e.cells.length} delivered</div>
      <table>
        <thead>
          <tr>
            <th scope="col">Node</th>
            {channels.map(([id, name]) => <th key={id} scope="col">{name}</th>)}
          </tr>
        </thead>
        <tbody>
          {nodes.map((n) => (
            <tr key={n}>
              <th scope="row">{watch.agents.find((a) => a.node === n)?.name ?? n}</th>
              {channels.map(([id]) => {
                const c = e2e.cells.find((x) => x.node === n && x.channel === id);
                if (!c) return <td key={id} className="muted">Not sent</td>;
                return (
                  <td key={id} className={c.ok ? undefined : "failed"} title={c.error ?? undefined}>
                    <span className="ntf-cell">
                      {c.ok ? <CheckIcon size={12} className="ntf-check-mark" aria-hidden /> : <StateGlyph state="warn" label="Failed" />}
                      {c.ok ? `Delivered in ${((c.ms ?? 0) / 1000).toFixed(1)} s` : `Failed${c.error ? `: ${c.error}` : ""}`}
                    </span>
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
