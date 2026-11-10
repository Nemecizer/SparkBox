import { useEffect, useMemo, useState } from "react";
import { Copy, KeyRound, Play, Plus, ShieldAlert, Square, Trash2 } from "lucide-react";
import { Icon } from "../components/Icon";
import { Table, type Column } from "../components/Table";
import { Badge, FormGroup, FormRow, HelpTip, InfoTip, Notice, Segmented, Spacer, StatusDot, Switch, Toolbar, ToolButton } from "../components/ui";
import { ADMIN_REFUSED, unlockedFor, useAdminState, withAdmin } from "../lib/admin";
import { api } from "../lib/api";
import { useDataSource, type Source } from "../lib/dataSource";
import { copyText } from "../lib/markdown";
import { modelsApi, type ProviderView, type ServerLogEntry, type ServerSettings, type ServerView } from "../lib/modelsApi";
import { setServerView, useServerView } from "../lib/server";
import { confirmAction } from "../lib/session";
import { toast } from "../lib/toasts";
import type { SelfTestResult } from "../lib/types";
import { useCloseGuard } from "../wm/WindowManager";
import { ModelPicker, useModelItems, type ModelChoice } from "./chat/ModelPicker";
import { fmtMs, fmtTps } from "./models/shared";
import "./ai.css";

type Tab = "claude" | "codex" | "openai" | "anthropic" | "curl";

/** The server and its keys are state on this Mac: Touch ID or the password first. */
const onMac = <T,>(action: string, fn: () => Promise<T>) => withAdmin(fn, { scope: "mac", action });
const errText = (e: unknown) => (e as { message?: string })?.message ?? String(e);

const TABS: { value: Tab; label: string }[] = [
  { value: "claude", label: "Claude Code" },
  { value: "codex", label: "Codex" },
  { value: "openai", label: "OpenAI SDK" },
  { value: "anthropic", label: "Anthropic SDK" },
  { value: "curl", label: "curl" },
];

/**
 * What the Copy button hands over. It names the variable SPARKBOX_API_KEY and never holds a key, so
 * that the text can go in a file or a shell history: the reader puts the key in the variable
 * (Help: Connect an app to SparkBox).
 */
export function snippet(tab: Tab, url: string, model: string): string {
  switch (tab) {
    case "claude":
      return [
        `export ANTHROPIC_BASE_URL=${url}`,
        `export ANTHROPIC_AUTH_TOKEN=$SPARKBOX_API_KEY`,
        `export ANTHROPIC_MODEL=${model}`,
        `# Claude Code also picks "opus", "sonnet" and "haiku" models for side tasks;`,
        `# map them too, or those requests try to reach Anthropic directly.`,
        `export ANTHROPIC_DEFAULT_OPUS_MODEL=${model}`,
        `export ANTHROPIC_DEFAULT_SONNET_MODEL=${model}`,
        `export ANTHROPIC_DEFAULT_HAIKU_MODEL=${model}`,
        `claude`,
      ].join("\n");
    case "codex":
      return [
        `# ~/.codex/config.toml`,
        `model = "${model}"`,
        `model_provider = "sparkbox"`,
        ``,
        `[model_providers.sparkbox]`,
        `name = "SparkBox"`,
        `base_url = "${url}/v1"`,
        `env_key = "SPARKBOX_API_KEY"`,
        `wire_api = "responses"`,
      ].join("\n");
    case "openai":
      return [
        `import os`,
        `from openai import OpenAI`,
        ``,
        `client = OpenAI(base_url="${url}/v1", api_key=os.environ["SPARKBOX_API_KEY"])`,
        `reply = client.chat.completions.create(`,
        `    model="${model}",`,
        `    messages=[{"role": "user", "content": "Hello"}],`,
        `)`,
        `print(reply.choices[0].message.content)`,
      ].join("\n");
    case "anthropic":
      return [
        `import os`,
        `import anthropic`,
        ``,
        `client = anthropic.Anthropic(base_url="${url}", api_key=os.environ["SPARKBOX_API_KEY"])`,
        `message = client.messages.create(`,
        `    model="${model}",`,
        `    max_tokens=16000,`,
        `    messages=[{"role": "user", "content": "Hello"}],`,
        `)`,
        `if message.stop_reason == "refusal":`,
        `    print("The model declined this request.")`,
        `else:`,
        `    print("".join(b.text for b in message.content if b.type == "text"))`,
      ].join("\n");
    case "curl":
      return [
        `curl ${url}/v1/chat/completions \\`,
        `  -H "Authorization: Bearer $SPARKBOX_API_KEY" \\`,
        `  -H "Content-Type: application/json" \\`,
        `  -d '{"model": "${model}", "messages": [{"role": "user", "content": "Hello"}]}'`,
      ].join("\n");
  }
}

/** What a snippet names as its model: the one picked, or a stand-in that says so. Demo's demo/echo answers; with Live data nothing would. */
export function snippetModel(picked: string, source: Source): string {
  return picked || (source === "demo" ? "demo/echo" : "provider/model");
}

const logColumns: Column<ServerLogEntry>[] = [
  { key: "time", label: "Time", sort: (e) => e.tsMs, render: (e) => <span className="num" title={new Date(e.tsMs).toLocaleString()}>{new Date(e.tsMs).toLocaleTimeString()}</span> },
  { key: "client", label: "App", sort: (e) => e.client, render: (e) => (e.client === "(refused)" ? <Badge tone="crit">refused</Badge> : e.client) },
  { key: "api", label: "API", priority: 3, render: (e) => <span className="mono">{e.api}</span> },
  { key: "model", label: "Model", sort: (e) => e.model, title: (e) => e.model, render: (e) => <span className="mono">{e.model || "—"}</span> },
  { key: "provider", label: "Provider", priority: 3, render: (e) => e.provider || "—" },
  {
    key: "status", label: "Status", sort: (e) => e.status, title: (e) => e.error ?? "",
    render: (e) => (
      <span className="status">
        <StatusDot status={e.status < 400 ? "online" : "error"} text={String(e.status)} />
        {e.error && <span className="ai-log-err">{e.error}</span>}
      </span>
    ),
  },
  { key: "ttfb", label: "First byte", align: "right", priority: 2, sort: (e) => e.ms, render: (e) => <span className="num">{fmtMs(e.ms)}</span> },
  { key: "total", label: "Total", align: "right", sort: (e) => e.durationMs ?? 0, render: (e) => <span className="num">{e.done ? fmtMs(e.durationMs) : e.stream ? "streaming…" : "—"}</span> },
  {
    key: "tokens", label: "Tokens in / out", align: "right", priority: 2, sort: (e) => e.outputTokens ?? 0,
    render: (e) => <span className="num">{e.inputTokens?.toLocaleString() ?? "—"} / {e.outputTokens?.toLocaleString() ?? "—"}</span>,
  },
  {
    key: "tps", label: "tok/s", align: "right", sort: (e) => e.tokensPerSec ?? 0,
    title: () => "Only for streams long enough to measure (8+ pieces over 0.25 s or more)",
    render: (e) => <span className="num">{e.tokensPerSec != null ? fmtTps(e.tokensPerSec) : "—"}</span>,
  },
];

interface AliasRow {
  name: string;
  target: string;
}

function toRows(a: Record<string, string>): AliasRow[] {
  return Object.entries(a).map(([name, target]) => ({ name, target }));
}

/**
 * What the window says in look-only mode: one policy for inference, held. Turning look-only on
 * stops the server (lib.rs hold hook), it doesn't autostart, any model request it gets meanwhile
 * is answered 503 (server/mod.rs HELD), and chat_send is refused (admin.rs LOOK_ONLY_HOLDS).
 */
export const LOOK_ONLY_SERVER = "Look-only holds inference: turning it on stops the API server, the server doesn't start until it's off, and Chat doesn't send. These settings and keys can't change meanwhile.";

/**
 * What's wrong with an alias row. `error` stops Save: a bad shape, or a new or changed alias on a
 * model nobody offers. `warning` doesn't: a saved alias whose model has gone for now is left as it
 * is, or one gone load would hold back every other setting (server/mod.rs check_aliases).
 */
export function aliasProblem(r: AliasRow, rows: AliasRow[], known: Set<string>, saved: Record<string, string>): { error: string | null; warning: string | null } {
  const name = r.name.trim();
  const error = (text: string) => ({ error: text, warning: null });
  if (!name) return error("Give it a name");
  if (!/^[\w.:-]+$/.test(name)) return error("Letters, digits and . _ : - only");
  if (rows.filter((x) => x.name.trim() === name).length > 1) return error("Used twice");
  if (!r.target) return error("Pick a model");
  if (known.has(r.target)) return { error: null, warning: null };
  if (saved[name] === r.target) return { error: null, warning: `${r.target} isn't offered right now` };
  return error("Unknown target: no provider offers this model now");
}

export function ServerWindow() {
  const live = useServerView();
  const source = useDataSource();
  const admin = useAdminState();
  const macLocked = !unlockedFor("mac", admin);
  // Look-only: every setting, key and alias here is a change on this Mac, so the forms are read-only (as in Settings).
  const lookOnly = !!admin.lookOnly;
  const [view, setView] = useState<ServerView | null>(null);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [port, setPort] = useState("8484");
  const [requireKey, setRequireKey] = useState(true);
  const [autostart, setAutostart] = useState(false);
  const [aliases, setAliases] = useState<AliasRow[]>([]);
  const [origins, setOrigins] = useState("");
  const [keyName, setKeyName] = useState("Claude Code");
  const [created, setCreated] = useState<{ name: string; key: string } | null>(null);
  const [logs, setLogs] = useState<ServerLogEntry[]>([]);
  const [choice, setChoice] = useState<ModelChoice | null>(null);
  const [alias, setAlias] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("claude");
  const [error, setError] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [results, setResults] = useState<SelfTestResult[] | null>(null);
  const items = useModelItems(providers, false);

  const apply = (v: ServerView) => {
    setView(v);
    setServerView(v);
    setPort(String(v.settings.port));
    setRequireKey(v.settings.requireKey);
    setAutostart(v.settings.autostart);
    setAliases(toRows(v.settings.aliases));
    setOrigins((v.settings.allowedOrigins ?? []).join("\n"));
  };

  useEffect(() => {
    modelsApi.serverGet().then(apply, (e) => setError(String(e)));
    modelsApi.serverLogs().then(setLogs, () => {});
    modelsApi.providersList().then((ps) => {
      setProviders(ps);
      // The one default rule: a ready cluster model first, then the providers' own defaults.
      const usable = ps.filter((p) => p.enabled && (p.hasKey || p.kind === "demo" || p.kind === "openaiCompatible"));
      const cluster = usable.find((p) => p.discovered || p.id.startsWith("vllm-"));
      const pick = cluster ?? usable.find((p) => p.kind === "anthropic") ?? usable.find((p) => p.kind === "openai") ?? usable[0];
      const m = pick?.effectiveDefault ?? pick?.visible[0];
      if (pick && m) setChoice((c) => c ?? { provider: pick.id, model: m });
    });
    let stop: (() => void) | undefined;
    api.onServerLog((e) => {
      const entry = e as ServerLogEntry;
      setLogs((l) => {
        const i = l.findIndex((x) => x.id === entry.id);
        if (i >= 0) return l.map((x, j) => (j === i ? entry : x));
        return [...l.slice(-299), entry];
      });
    }).then((u) => (stop = u));
    return () => stop?.();
  }, []);

  const status = live ?? view;
  const settingsDirty = useMemo(() => {
    if (!view) return false;
    const s = view.settings;
    const a = JSON.stringify(toRows(s.aliases));
    return String(s.port) !== port || s.requireKey !== requireKey || s.autostart !== autostart || a !== JSON.stringify(aliases)
      || (s.allowedOrigins ?? []).join("\n") !== origins.trim();
  }, [view, port, requireKey, autostart, aliases, origins]);
  useCloseGuard(() => (settingsDirty ? "The server settings have unsaved changes" : null));

  if (!view || !status) return <div className="pad muted">{error ?? "Loading…"}</div>;

  const known = new Set(items.map((i) => `${i.provider}/${i.model}`));
  const problemOf = (r: AliasRow) => aliasProblem(r, aliases, known, view.settings.aliases);
  const aliasProblems = aliases.some((r) => problemOf(r).error);

  const run = async (fn: () => Promise<ServerView>, done?: string) => {
    try {
      apply(await fn());
      setError(null);
      if (done) toast({ kind: "success", title: done });
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(errText(e));
    }
  };

  const save = () => {
    const p = Number(port);
    if (!Number.isInteger(p) || p < 1024 || p > 65535) {
      setError("Pick a port between 1024 and 65535.");
      return;
    }
    const settings: ServerSettings = {
      port: p, requireKey, autostart,
      aliases: Object.fromEntries(aliases.map((r) => [r.name.trim(), r.target])),
      allowedOrigins: origins.split(/\s+/).map((o) => o.trim().replace(/\/$/, "")).filter(Boolean),
    };
    run(() => onMac("server_save", () => modelsApi.serverSave(settings)), "Server settings saved");
  };

  const createKey = async () => {
    try {
      const key = await onMac("server_create_key", () => api.serverCreateKey(keyName));
      setCreated({ name: keyName, key });
      apply(await modelsApi.serverGet());
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(errText(e));
    }
  };

  const revoke = async (name: string) => {
    const ok = await confirmAction({ title: `Revoke the key “${name}”?`, body: "Apps using it stop working until they get a new key.", confirmLabel: "Revoke", danger: true });
    if (ok) run(() => onMac("server_revoke_key", () => modelsApi.serverRevokeKey(name)), `Revoked “${name}”`);
  };

  const model = alias ?? (choice ? `${choice.provider}/${choice.model}` : "");
  const runTest = async () => {
    setTesting(true);
    setResults(null);
    try {
      setResults(await api.serverSelfTest(model));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setTesting(false);
    }
  };

  const url = status.url ?? `http://127.0.0.1:${view.settings.port}`;
  const code = snippet(tab, url, snippetModel(model, source));

  return (
    <div className="col fill ai-server">
      <Toolbar label="Server">
        <StatusDot status={status.running ? "online" : "offline"} text={status.running ? `Running at ${status.url}` : "Stopped"} />
        <Spacer />
        {status.running ? (
          <ToolButton icon={Square} label="Stop" locked={macLocked} scope="mac" action="server_stop" onClick={() => run(() => onMac("server_stop", modelsApi.serverStop))} />
        ) : (
          <ToolButton icon={Play} label="Start" locked={macLocked} scope="mac" action="server_start" onClick={() => run(() => onMac("server_start", modelsApi.serverStart))} />
        )}
      </Toolbar>
      {lookOnly && <Notice tone="info">{LOOK_ONLY_SERVER}</Notice>}
      {!view.settings.requireKey && (
        <div className="notice error ai-banner" role="alert">
          <Icon icon={ShieldAlert} size={14} />
          <div>
            <strong>Anyone on this Mac can use your API keys.</strong> The server doesn't require a SparkBox key, so any app or script running here can
            send requests to Claude, OpenAI and your cluster through it. Turn “Require a SparkBox key” back on below.
          </div>
        </div>
      )}
      {(error || status.error) && <Notice tone="error" onDismiss={() => setError(null)}>{error ?? status.error}</Notice>}

      <div className="ai-server-body">
        <p className="note">
          One local address for every model SparkBox can reach: your cluster, OpenAI and Claude. It speaks the OpenAI Chat
          Completions, Responses, Completions and Embeddings APIs (Codex, most apps) and the Anthropic Messages API (Claude Code).
          Apps use a SparkBox key; your provider keys never leave SparkBox.
        </p>

        <fieldset className="settings-fieldset" disabled={lookOnly}>
          <FormGroup
            title={
              <>
                Settings{" "}
                <HelpTip topic="ai.server.settings" label="About the server's settings" />
              </>
            }
          >
            <FormRow label="Port" htmlFor="srv-port">
              <input id="srv-port" className="narrow" value={port} onChange={(e) => setPort(e.target.value)} inputMode="numeric" />
            </FormRow>
            <FormRow label={<>Require a SparkBox key <HelpTip topic="ai.server.require-key" /></>} hint="Every request needs Authorization: Bearer … or x-api-key">
              <Switch checked={requireKey} onChange={setRequireKey} label="Require a SparkBox key" />
            </FormRow>
            <FormRow label={<>Start with SparkBox <HelpTip topic="ai.server.autostart" /></>}>
              <Switch checked={autostart} onChange={setAutostart} label="Start when SparkBox starts" />
            </FormRow>
            <FormRow label={<>Allowed web origins <HelpTip topic="ai.server.origins" /></>} hint="One per line, e.g. http://localhost:3000. Leave empty unless a web app needs it.">
              <textarea className="mono ai-origins" rows={2} value={origins} onChange={(e) => setOrigins(e.target.value)} placeholder="none" />
            </FormRow>
          </FormGroup>

          <FormGroup title={<>Access keys <HelpTip topic="create-an-api-key" /></>}>
            <table className="grid form-grid">
              <thead><tr><th>App</th><th>Key</th><th>Created</th><th /></tr></thead>
              <tbody>
                {view.clients.map((c) => (
                  <tr key={c.name}>
                    <td>{c.name}</td>
                    <td className="mono">{c.preview}</td>
                    <td>{new Date(c.createdAt * 1000).toLocaleDateString()}</td>
                    <td><button className="tool" title="Revoke" aria-label={`Revoke ${c.name}`} onClick={() => revoke(c.name)}><Icon icon={Trash2} size={14} /></button></td>
                  </tr>
                ))}
                {view.clients.length === 0 && <tr><td colSpan={4} className="empty">No keys yet. Create one per app.</td></tr>}
              </tbody>
            </table>
            <div className="ai-inline ai-pad">
              <input value={keyName} onChange={(e) => setKeyName(e.target.value)} placeholder="App name, e.g. Claude Code" aria-label="App name" />
              <button className="btn primary" onClick={createKey}><Icon icon={KeyRound} size={12} /> Create key</button>
            </div>
            {created && (
              <div className="new-key">
                <div><span className="strong">{created.name}</span>: copy this key now; SparkBox won't show it again.</div>
                <div className="ai-inline">
                  <code className="mono key-text">{created.key}</code>
                  <button className="btn" onClick={() => copyText(created.key)}><Icon icon={Copy} size={12} /> Copy</button>
                </div>
              </div>
            )}
          </FormGroup>
        </fieldset>

        <FormGroup title={<>Connect your apps <HelpTip topic="connect-an-app" /></>}>
          <FormRow label="Model" hint="Cluster models first, with whether they're ready">
            <span className="ai-inline">
              <ModelPicker providers={providers} value={alias ? null : choice} onChange={(c) => { setChoice(c); setAlias(null); }} />
              {Object.keys(view.settings.aliases).length > 0 && (
                <select value={alias ?? ""} onChange={(e) => setAlias(e.target.value || null)} aria-label="Or an alias">
                  <option value="">or an alias…</option>
                  {Object.keys(view.settings.aliases).map((a) => <option key={a} value={a}>{a}</option>)}
                </select>
              )}
            </span>
          </FormRow>
          <div className="ai-pad col gap-sm">
            <Segmented<Tab> label="App" value={tab} onChange={setTab} options={TABS} />
            {!model && source !== "demo" && <p className="note">Pick a model above</p>}
            <div className="code-block">
              <div className="code-head"><span>{TABS.find((t) => t.value === tab)?.label}</span><button className="code-copy" onClick={() => copyText(code)}>Copy</button></div>
              <pre><code>{code}</code></pre>
            </div>
            <p className="note">
              The text reads your key from the variable <span className="mono">SPARKBOX_API_KEY</span>, so it never holds the key. Set that variable in the Terminal window you'll use: Help's Connect an app to SparkBox has the line to paste.
            </p>
            {tab === "claude" && (
              <p className="note">
                With an <span className="mono">anthropic/…</span> model, requests go straight to Anthropic with your key. Cluster models need
                tool calling turned on when they're loaded (Models › Load › Capabilities) and a large context window.
              </p>
            )}
            {tab === "codex" && <p className="note">Codex uses the Responses API. OpenAI models and vLLM support it directly; other engines may not yet.</p>}
          </div>
        </FormGroup>

        <FormGroup
          title={
            <>
              Test the server{" "}
              <InfoTip label="About the test" topic="ai.server.test">Sends “Reply with the single word OK.” to {url} in all three API formats, exactly as an outside app would, using the model above.</InfoTip>
            </>
          }
        >
          <div className="ai-inline ai-pad">
            <button className="btn primary" disabled={!status.running || testing || !model || lookOnly} onClick={runTest}>{testing ? "Testing…" : "Send test requests"}</button>
            {!status.running && <span className="muted">Start the server first.</span>}
          </div>
          {results && (
            <table className="grid form-grid">
              <thead><tr><th>API</th><th>Result</th><th>Time</th><th>Reply or error</th></tr></thead>
              <tbody>
                {results.map((r) => (
                  <tr key={r.api}>
                    <td className="mono">{r.api}</td>
                    <td><StatusDot status={r.ok ? "online" : "error"} text={r.ok ? "Works" : `Failed (${r.status || "no answer"})`} /></td>
                    <td className="num">{(r.ms / 1000).toFixed(1)} s</td>
                    <td className="wrap">{r.detail}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </FormGroup>

        <fieldset className="settings-fieldset" disabled={lookOnly}>
          <FormGroup
            title={
              <>
                Aliases{" "}
                <HelpTip topic="ai.server.aliases" label="About aliases" />
              </>
            }
          >
            <table className="grid form-grid ai-aliases">
              <thead><tr><th>Alias</th><th>Model</th><th /></tr></thead>
              <tbody>
                {aliases.map((r, i) => {
                  const { error: err, warning } = problemOf(r);
                  return (
                    <tr key={i}>
                      <td>
                        <input className={`mono${err && !r.name.trim() ? " invalid" : ""}`} value={r.name} aria-label="Alias"
                          onChange={(e) => setAliases(aliases.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))} />
                      </td>
                      <td>
                        <select className={err?.startsWith("Unknown") ? "invalid" : undefined} value={r.target} aria-label={`Model for ${r.name || "alias"}`}
                          onChange={(e) => setAliases(aliases.map((x, j) => (j === i ? { ...x, target: e.target.value } : x)))}>
                          {!known.has(r.target) && <option value={r.target}>{r.target || "Pick a model"}</option>}
                          {(["Cluster", "Anthropic", "OpenAI", "Demo", "Other"] as const).map((g) => {
                            const opts = items.filter((it) => it.group === g && !it.disabled);
                            if (!opts.length) return null;
                            return (
                              <optgroup key={g} label={g}>
                                {opts.map((it) => (
                                  <option key={it.key} value={`${it.provider}/${it.model}`}>
                                    {it.model} — {it.providerName}{it.endpoint ? (it.endpoint.health === "ready" ? " (ready)" : ` (${it.endpoint.health})`) : ""}
                                  </option>
                                ))}
                              </optgroup>
                            );
                          })}
                        </select>
                        {err && <div className="error-text ai-field-err" role="alert">{err}</div>}
                        {warning && <div className="warn-text ai-field-err">{warning}</div>}
                      </td>
                      <td><button className="tool" aria-label={`Remove ${r.name || "alias"}`} onClick={() => setAliases(aliases.filter((_, j) => j !== i))}><Icon icon={Trash2} size={14} /></button></td>
                    </tr>
                  );
                })}
                {aliases.length === 0 && <tr><td colSpan={3} className="empty">No aliases.</td></tr>}
              </tbody>
            </table>
            <div className="ai-pad">
              <button className="btn small" onClick={() => setAliases([...aliases, { name: aliases.length ? "" : "local", target: model }])}>
                <Icon icon={Plus} size={12} /> Add alias
              </button>
            </div>
          </FormGroup>
        </fieldset>

        <section>
          <div className="section-title">Requests <HelpTip topic="ai.server.requests" /></div>
          <Table columns={logColumns} rows={[...logs].reverse()} rowKey={(e) => String(e.id || `${e.tsMs}-${e.model}`)} tableId="server.log"
            footer itemNoun={["request", "requests"]} empty="No requests yet." ariaLabel="Requests" />
        </section>
      </div>
      <div className="footer-bar">
        <span className="muted">{aliasProblems ? "Fix the aliases to save" : settingsDirty ? "Unsaved changes" : ""}</span>
        <Spacer />
        {settingsDirty && <button className="btn" onClick={() => apply(view)}>Revert</button>}
        <button className="btn primary" disabled={!settingsDirty || aliasProblems || lookOnly} onClick={save}>Save settings</button>
      </div>
    </div>
  );
}
