// Ollama on a node: what's installed, what's in memory and until when, pulling
// a model (a task with progress, cancellable) and deleting one.
import { useEffect, useMemo, useState } from "react";
import { Download, MessageSquare, RefreshCw, Trash2 } from "lucide-react";
import { Table, type Column } from "../components/Table";
import { Badge, EmptyState, HelpTip, Notice, Spacer, Toolbar, ToolButton } from "../components/ui";
import { isLookOnly, LOOK_ONLY_TIP, requestAdmin, useAdmin } from "../lib/admin";
import { useEndpoints } from "../lib/endpoints";
import { plural } from "../lib/vocabulary";
import { fmtBytes } from "../lib/format";
import { modelsApi, type OllamaModel, type OllamaView } from "../lib/modelsApi";
import { confirmAction } from "../lib/session";
import { useTasks } from "../lib/tasks";
import { toast } from "../lib/toasts";
import { useViewLink, useWindowEvent, useWinState, type Params } from "../wm/WindowManager";
import { Ago, errText, useOpenChat } from "./models/shared";
import { Container } from "lucide-react";
import "./ai.css";

/** Common picks, to start typing from. */
const SUGGESTIONS = ["gpt-oss:20b", "gpt-oss:120b", "qwen3:8b", "qwen3:32b", "gemma3:27b", "llama3.2:3b", "mistral-small3.2", "nomic-embed-text"];

export function OllamaWindow({ params }: { params: Params }) {
  const endpoints = useEndpoints().filter((e) => e.engine === "ollama");
  const admin = useAdmin();
  const chat = useOpenChat();
  const [picked, setPicked] = useWinState<string | null>("endpoint", params.endpoint ?? null);
  useViewLink(params, () => params.endpoint && setPicked(params.endpoint));
  const endpoint = endpoints.find((e) => e.id === picked) ?? endpoints[0];
  const [view, setView] = useState<OllamaView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const pulls = useTasks().filter((t) => t.kind === "ollama-pull" && t.state === "running");

  const refresh = async () => {
    if (!endpoint) return;
    try {
      setView(await modelsApi.ollamaList(endpoint.id));
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  };
  useEffect(() => {
    refresh();
    const t = window.setInterval(refresh, 10_000);
    return () => window.clearInterval(t);
  }, [endpoint?.id]);
  // A finished pull changes the list.
  const running = pulls.map((p) => p.id).join(",");
  useEffect(() => {
    refresh();
  }, [running]);
  useWindowEvent("refresh", () => void refresh());

  const pull = async (model = name.trim()) => {
    if (!endpoint || !model) return;
    if (!admin && !(await requestAdmin(`pull ${model} into Ollama`))) return;
    try {
      await modelsApi.ollamaPull(endpoint.id, model);
      setName("");
    } catch (e) {
      toast({ kind: "error", title: `Couldn't pull ${model}`, body: errText(e) });
    }
  };

  const remove = async (m: OllamaModel) => {
    if (!endpoint) return;
    if (!admin && !(await requestAdmin(`delete ${m.name} from Ollama`))) return;
    const ok = await confirmAction({
      title: `Delete ${m.name} from ${endpoint.headNode}?`,
      body: `Frees ${fmtBytes(m.sizeBytes)} on ${endpoint.headNode}. Apps asking Ollama for ${m.name} will get an error until it's pulled again.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    try {
      await modelsApi.ollamaDelete(endpoint.id, m.name);
      toast({ kind: "success", title: `Deleted ${m.name}` });
      refresh();
    } catch (e) {
      toast({ kind: "error", title: `Couldn't delete ${m.name}`, body: errText(e) });
    }
  };

  const model = view?.models.find((m) => m.name === selected) ?? null;
  const total = useMemo(() => (view?.models ?? []).reduce((a, m) => a + m.sizeBytes, 0), [view]);
  const inMemory = (view?.models ?? []).filter((m) => m.loaded);

  const columns: Column<OllamaModel>[] = [
    { key: "name", label: "Model", sort: (m) => m.name, priority: 1, render: (m) => <span className="mono strong">{m.name}</span> },
    {
      key: "mem", label: "In memory", help: "ai.ollama.in-memory", sort: (m) => m.loaded?.vramBytes ?? 0,
      render: (m) => (m.loaded
        ? <span><Badge tone="good">{fmtBytes(m.loaded.vramBytes)}</Badge>{m.loaded.expiresAt ? <span className="muted"> unloads <Ago ts={m.loaded.expiresAt} /></span> : null}</span>
        : <span className="muted">—</span>),
    },
    { key: "params", label: "Parameters", priority: 2, sort: (m) => m.parameterSize ?? "", render: (m) => m.parameterSize || "—" },
    { key: "quant", label: "Quantization", priority: 2, render: (m) => m.quantization || "—" },
    { key: "family", label: "Family", priority: 3, render: (m) => m.family || "—" },
    { key: "size", label: "On disk", align: "right", sort: (m) => m.sizeBytes, render: (m) => <span className="num">{fmtBytes(m.sizeBytes)}</span> },
    { key: "modified", label: "Pulled", align: "right", sort: (m) => m.modifiedAt ?? 0, render: (m) => (m.modifiedAt ? <Ago ts={m.modifiedAt} /> : "—") },
  ];

  if (!endpoint) {
    return (
      <EmptyState icon={Container} title="No Ollama server found"
        body="SparkBox looks for `ollama serve` in every node's containers and services. Start Ollama on a node and it appears here within a few seconds." />
    );
  }

  return (
    <div className="col fill ai-ollama-win">
      <Toolbar label="Ollama">
        {endpoints.length > 1 && (
          <select value={endpoint.id} onChange={(e) => setPicked(e.target.value)} aria-label="Ollama server">
            {endpoints.map((e) => <option key={e.id} value={e.id}>{e.headNode} :{e.port}</option>)}
          </select>
        )}
        <ToolButton icon={RefreshCw} label="Refresh" onClick={refresh} />
        <HelpTip topic="use-ollama" />
        <ToolButton icon={MessageSquare} label="Chat with this" disabled={!model || !endpoint.providerId || /embed/.test(model.name)}
          onClick={() => model && endpoint.providerId && chat(endpoint.providerId, model.name)} />
        <ToolButton icon={Trash2} label="Delete…" locked={!admin} disabled={!model} onClick={() => model && remove(model)} priority={2} />
        <Spacer />
        <span className="toolbar-text muted" title={`Ollama on ${endpoint.headNode}, port ${endpoint.port}`}>
          {endpoint.headNode} :{endpoint.port}{view?.version ? ` · Ollama ${view.version}` : ""} · {plural(view?.models.length ?? 0, "model")}, {fmtBytes(total)} · {inMemory.length ? `${inMemory.length} in memory` : "nothing in memory"}
        </span>
      </Toolbar>
      {error && <Notice tone="error" onRetry={refresh}>{error}</Notice>}
      <div className="ai-pull">
        <input
          list="ollama-suggestions"
          className="mono"
          placeholder="Model to pull, e.g. qwen3:8b"
          aria-label="Model to pull"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && pull()}
        />
        <datalist id="ollama-suggestions">{SUGGESTIONS.map((s) => <option key={s} value={s} />)}</datalist>
        <button className="btn primary" disabled={!name.trim() || (!admin && isLookOnly())} onClick={() => pull()} title={admin ? undefined : isLookOnly() ? LOOK_ONLY_TIP : "Asks to unlock Admin mode first"}>
          <Download size={12} /> Pull
        </button>
        <HelpTip topic="ai.ollama.pull" />
        <span className="muted">Downloads from ollama.com on {endpoint.headNode}; it shows in Tasks with progress and can be canceled there.</span>
      </div>
      {pulls.map((t) => (
        <div key={t.id} className="ai-pull-row">
          <span className="strong">{t.title}</span>
          <div className="meter ok"><div className="meter-track"><div className="meter-fill" style={{ width: `${Math.round((t.progress ?? 0) * 100)}%` }} /></div></div>
          <span className="muted num">{t.detail}</span>
        </div>
      ))}
      <Table columns={columns} rows={view?.models ?? []} rowKey={(m) => m.name} selected={selected} onSelect={setSelected} tableId="ollama.models"
        footer itemNoun={["model", "models"]} ariaLabel="Ollama models"
        onActivate={(m) => endpoint.providerId && !/embed/.test(m.name) && chat(endpoint.providerId, m.name)}
        empty={view ? "Nothing installed yet: pull a model above." : "Asking Ollama…"} />
    </div>
  );
}
