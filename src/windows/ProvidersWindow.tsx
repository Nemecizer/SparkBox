import { useEffect, useMemo, useState } from "react";
import { Plus, RefreshCw, ScanSearch, Trash2 } from "lucide-react";
import { Icon } from "../components/Icon";
import { Badge, HelpTip, Notice, Spacer, Switch, Toolbar, ToolButton } from "../components/ui";
import { ADMIN_REFUSED, useAdminState, withAdmin } from "../lib/admin";
import { api } from "../lib/api";
import { modelsApi, type ProviderView } from "../lib/modelsApi";
import { confirmAction } from "../lib/session";
import { toast } from "../lib/toasts";
import type { Provider } from "../lib/types";
import "./ai.css";

const KIND_LABEL = {
  anthropic: "Anthropic API",
  openai: "OpenAI API",
  openaiCompatible: "OpenAI-compatible",
  demo: "Built in",
} as const;

type Saved = Provider & { chatOnly: boolean; hidden: string[] };

/** Providers and their keys are state on this Mac: Touch ID or the password first. */
const onMac = <T,>(action: string, fn: () => Promise<T>) => withAdmin(fn, { scope: "mac", action });

function strip(p: ProviderView): Saved {
  const { hasKey: _k, visible: _v, effectiveDefault: _d, discovered: _f, ...rest } = p;
  return rest;
}

/** Plain http to somewhere other than this Mac: it needs https, or the home-network tick. */
function plainHttpOff(url: string): boolean {
  try {
    const u = new URL(url.trim());
    return u.protocol === "http:" && !["localhost", "127.0.0.1", "[::1]"].includes(u.hostname);
  } catch {
    return false;
  }
}

function slug(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 32);
}

/** Which models Chat and the Server offer from one provider: chat models only by default, and any you turn off. */
function ModelAllowlist({ p, onSave, busy, first }: { p: ProviderView; onSave: (next: Saved) => void; busy: boolean; first: boolean }) {
  const [filter, setFilter] = useState("");
  const all = useMemo(() => {
    const rest = p.models.filter((m) => !p.visible.includes(m)).sort((a, b) => a.localeCompare(b));
    return [...p.visible, ...rest.filter((m) => p.hidden.includes(m)), ...rest.filter((m) => !p.hidden.includes(m))];
  }, [p.models, p.visible, p.hidden]);
  const shown = filter ? all.filter((m) => m.toLowerCase().includes(filter.toLowerCase())) : all;
  if (p.models.length === 0) return <div className="muted ai-pad">No models listed yet. Refresh the list{p.kind !== "demo" && p.kind !== "openaiCompatible" ? " after adding the key" : ""}.</div>;
  const toggle = (m: string, on: boolean) => {
    const hidden = on ? p.hidden.filter((x) => x !== m) : [...p.hidden, m];
    onSave({ ...strip(p), hidden });
  };
  return (
    <div className="ai-allow">
      <div className="ai-allow-head">
        <Switch checked={p.chatOnly} onChange={(v) => onSave({ ...strip(p), chatOnly: v })} label="Chat models only" showLabel disabled={busy}
          description="Hides embedding, audio, image, video and moderation models" />
        {first && <HelpTip topic="ai.providers.chat-only" />}
        <span className="muted">{p.visible.length} of {p.models.length} offered in Chat and the Server</span>
        <Spacer />
        {p.models.length > 10 && <input type="search" placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label={`Filter ${p.name} models`} />}
      </div>
      <div className="ai-allow-list" role="group" aria-label={`${p.name} models`}>
        {shown.map((m) => {
          const nonChat = p.chatOnly && !p.visible.includes(m) && !p.hidden.includes(m);
          const on = p.visible.includes(m);
          return (
            <label key={m} className={`check ai-allow-item${nonChat ? " muted" : ""}`} title={nonChat ? "Not a chat model (turn off “Chat models only” to offer it)" : undefined}>
              <input type="checkbox" checked={on} disabled={nonChat || busy} onChange={(e) => toggle(m, e.target.checked)} />
              <span className="mono">{m}</span>
              {m === p.effectiveDefault && <Badge tone="info">default</Badge>}
              {nonChat && <span className="small">not chat</span>}
            </label>
          );
        })}
      </div>
    </div>
  );
}

export function ProvidersWindow() {
  const admin = useAdminState();
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [msg, setMsg] = useState<{ tone?: "error"; text: string } | null>(null);
  const [form, setForm] = useState({ name: "", baseUrl: "", key: "", allowHttpLan: false });

  const load = () => modelsApi.providersList().then(setProviders, (e) => setMsg({ tone: "error", text: String(e) }));
  useEffect(() => {
    load();
  }, []);

  const run = async (id: string, fn: () => Promise<string | void>) => {
    setBusy(id);
    try {
      const text = await fn();
      if (text) toast({ kind: "success", title: text });
      setMsg(null);
    } catch (e) {
      if (e !== ADMIN_REFUSED) setMsg({ tone: "error", text: (e as { message?: string })?.message ?? String(e) });
    } finally {
      setBusy(null);
      await load();
    }
  };

  const refresh = (p: ProviderView) =>
    run(p.id, async () => {
      const models = await api.providersRefresh(p.id);
      return `${p.name}: ${models.length} model${models.length === 1 ? "" : "s"} listed`;
    });

  const saveKey = (p: ProviderView) =>
    run(p.id, async () => {
      await onMac("providers_set_key", () => api.providersSetKey(p.id, keys[p.id] ?? ""));
      setKeys({ ...keys, [p.id]: "" });
      const models = await api.providersRefresh(p.id);
      return `Key saved to the Keychain. ${p.name}: ${models.length} models listed`;
    });

  const upsert = (next: Saved) =>
    run(next.id, async () => {
      const had = providers.find((p) => p.id === next.id)?.hasKey;
      const after = await onMac("providers_upsert", () => api.providersUpsert(next));
      // The backend drops a key when the address or kind changes, so it never goes to a new host.
      if (had && !after.find((p) => p.id === next.id)?.hasKey) return `${next.name}'s saved key was removed: its address changed. Add the key again.`;
    });

  const remove = async (p: ProviderView) => {
    const ok = await confirmAction({ title: `Remove ${p.name}?`, body: "Its key (if any) is deleted from the Keychain.", confirmLabel: "Remove", danger: true });
    if (ok) run(p.id, async () => {
      await onMac("providers_remove", () => api.providersRemove(p.id));
      return `Removed ${p.name}`;
    });
  };

  const add = () =>
    run("new", async () => {
      const id = slug(form.name);
      if (!id || !form.baseUrl.trim()) throw new Error("Give the endpoint a name and an address.");
      await onMac("providers_upsert", () => api.providersUpsert({ id, name: form.name.trim(), kind: "openaiCompatible", baseUrl: form.baseUrl.trim(), enabled: true, models: [], defaultModel: null, chatOnly: true, hidden: [], allowHttpLan: form.allowHttpLan } as Saved));
      if (form.key.trim()) await onMac("providers_set_key", () => api.providersSetKey(id, form.key.trim()));
      setForm({ name: "", baseUrl: "", key: "", allowHttpLan: false });
      const models = await api.providersRefresh(id).catch((e) => {
        throw new Error(`Added, but couldn't list its models: ${e}`);
      });
      return `Added ${form.name.trim()} with ${models.length} models`;
    });

  const scan = () =>
    run("scan", async () => {
      const found = await withAdmin(() => api.providersScan(), { action: "providers_scan" });
      return found.length
        ? `Found ${found.map((p) => p.name).join(", ")}`
        : "No engines answered. You need to be home or connected through the VPN, with an engine running on its usual port.";
    });

  const cluster = providers.filter((p) => p.discovered);
  const saved = providers.filter((p) => !p.discovered);

  // Icons that every card would repeat sit on the first card only.
  const firstCard = (saved[0] ?? cluster[0])?.id;
  const card = (p: ProviderView) => (
    <section key={p.id} className="card ai-provider">
      <div className="card-head">
        <span className="strong">{p.name}</span>
        <Badge tone="neutral">{p.discovered && p.kind === "demo" ? "Simulated engine" : KIND_LABEL[p.kind]}</Badge>
        {p.discovered && <Badge tone="info" title="Registered automatically while it's serving; it leaves when the server stops">Found on the cluster</Badge>}
        <span className="muted mono">{p.id}/…</span>
        <Spacer />
        <Switch checked={p.enabled} onChange={() => upsert({ ...strip(p), enabled: !p.enabled })} label="Enabled" showLabel disabled={busy === p.id} />
        {p.id === firstCard && <HelpTip topic="ai.providers.enabled" />}
        {p.kind === "openaiCompatible" && !p.discovered && !p.id.startsWith("vllm-") && (
          <button className="tool" title="Remove" aria-label={`Remove ${p.name}`} onClick={() => remove(p)}><Icon icon={Trash2} size={14} /></button>
        )}
      </div>
      {p.kind !== "demo" && <div className="muted mono">{p.baseUrl}</div>}
      {p.kind === "openaiCompatible" && !p.discovered && plainHttpOff(p.baseUrl) && (
        <div className="form-row">
          <Switch checked={!!p.allowHttpLan} onChange={(v) => upsert({ ...strip(p), allowHttpLan: v })} label="Allow plain HTTP on my network" showLabel disabled={busy === p.id}
            description="Only for a private address on your home network; the key and prompts travel unencrypted" />
          <HelpTip topic="ai.providers.plain-http" />
        </div>
      )}
      {p.kind !== "demo" && !p.discovered && (
        <div className="form-row">
          <label htmlFor={`key-${p.id}`}>API key{p.id === firstCard && <> <HelpTip topic="ai.providers.api-key" /></>}</label>
          <input
            id={`key-${p.id}`}
            type="password"
            value={keys[p.id] ?? ""}
            onChange={(e) => setKeys({ ...keys, [p.id]: e.target.value })}
            placeholder={p.hasKey ? "•••••••• (saved)" : p.kind === "openaiCompatible" ? "Optional" : "Paste your key"}
            autoComplete="off"
          />
          <button className="btn primary" disabled={!keys[p.id] || busy === p.id} onClick={() => saveKey(p)}>Save key</button>
          {p.hasKey && <button className="btn" disabled={busy === p.id} onClick={() => run(p.id, async () => void (await onMac("providers_clear_key", () => api.providersClearKey(p.id))))}>Remove key</button>}
        </div>
      )}
      <div className="form-row">
        <label htmlFor={`default-${p.id}`}>Default model{p.id === firstCard && <> <HelpTip topic="ai.providers.default-model" /></>}</label>
        <select id={`default-${p.id}`} value={p.effectiveDefault ?? ""} onChange={(e) => upsert({ ...strip(p), defaultModel: e.target.value || null })}>
          {p.visible.length === 0 && <option value="">None offered</option>}
          {p.visible.map((m) => <option key={m} value={m}>{m}</option>)}
        </select>
        <span className="muted">what Chat starts with on {p.name}; a request through the Server names its own model</span>
        {p.kind !== "demo" && !p.discovered && (
          <button className="btn" disabled={busy === p.id} onClick={() => refresh(p)}><Icon icon={RefreshCw} size={12} /> Refresh list</button>
        )}
      </div>
      <ModelAllowlist p={p} onSave={upsert} busy={busy === p.id} first={p.id === firstCard} />
    </section>
  );

  return (
    <div className="col fill">
      <Toolbar label="Providers">
        <ToolButton icon={ScanSearch} label="Scan cluster" onClick={scan} disabled={!!busy} locked={!admin.unlocked} action="providers_scan" title="Look for vLLM, SGLang, Ollama and llama.cpp on every node's usual port" />
        <HelpTip topic="ai.providers.scan" />
        <Spacer />
        <span className="toolbar-text">API keys are stored in the macOS Keychain.</span>
      </Toolbar>
      {msg && <Notice tone={msg.tone ?? "info"} onDismiss={() => setMsg(null)}>{msg.text}</Notice>}
      <div className="settings pad col gap">
        {saved.map(card)}
        {cluster.length > 0 && <div className="section-title">On the cluster now <HelpTip topic="ai.providers.on-cluster" /></div>}
        {cluster.map(card)}

        <section className="card">
          <div className="card-head"><span className="strong">Add an engine</span><HelpTip topic="providers#add-another-server" /></div>
          <p className="note">Any OpenAI-compatible server: vLLM, SGLang, TensorRT-LLM, llama.cpp, NIM, Ollama, LM Studio. The address ends in <span className="mono">/v1</span>. Engines on the cluster show up by themselves.</p>
          <div className="form-row">
            <label htmlFor="new-name">Name</label>
            <input id="new-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="LM Studio on my laptop" />
          </div>
          <div className="form-row">
            <label htmlFor="new-url">Address</label>
            <input id="new-url" className="wide" value={form.baseUrl} onChange={(e) => setForm({ ...form, baseUrl: e.target.value })} placeholder="http://192.168.50.20:1234/v1" />
          </div>
          {plainHttpOff(form.baseUrl) && (
            <div className="form-row">
              <Switch checked={form.allowHttpLan} onChange={(v) => setForm({ ...form, allowHttpLan: v })} label="Allow plain HTTP on my network" showLabel
                description="For a private (192.168.x, 10.x) address on your home network; anywhere else needs https" />
              <HelpTip topic="ai.providers.plain-http" />
            </div>
          )}
          <div className="form-row">
            <label htmlFor="new-key">API key</label>
            <input id="new-key" type="password" value={form.key} onChange={(e) => setForm({ ...form, key: e.target.value })} placeholder="Optional" autoComplete="off" />
            <Spacer />
            <button className="btn primary" disabled={busy === "new"} onClick={add}><Icon icon={Plus} size={12} /> Add</button>
          </div>
        </section>
      </div>
    </div>
  );
}
