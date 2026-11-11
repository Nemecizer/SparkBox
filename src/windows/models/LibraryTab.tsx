// Models on disk across the cluster: which nodes have a verified copy, which are
// being served right now, and copying or deleting them. Selecting one shows what it is
// (its config, where each copy lives, whether it's in memory and what last ran it); the
// Ollama tags on each node are listed too, read-only.
import { Fragment, useEffect, useRef, useState } from "react";
import { Play, RefreshCw, Share2, Trash2 } from "lucide-react";
import { Table, type Column } from "../../components/Table";
import { Badge, HelpTip, MiddleEllipsis, Notice, Spacer, Toolbar, ToolButton } from "../../components/ui";
import { api } from "../../lib/api";
import { useAdmin } from "../../lib/admin";
import { fmtBytes } from "../../lib/format";
import { listNames } from "../../lib/nodeStatus";
import { useDeployments, type Deployment } from "../../lib/deployments";
import { endpointLabel, useEndpoints } from "../../lib/endpoints";
import { finishedCopy, finishedNodes, modelsApi, type LibraryDetail, type LibraryEntry, type LibraryNodeStatus } from "../../lib/modelsApi";
import { confirmAction } from "../../lib/session";
import { toast } from "../../lib/toasts";
import { Ago, errText, locked, type NodeRef } from "./shared";
import { checkTransferDisk, DiskPreflight } from "./DiskPreflight";

/** One node's copy of a folder, as its Library cell reads. */
function CopyCell({ copy, status }: { copy: LibraryEntry["copies"][string] | undefined; status?: LibraryNodeStatus }) {
  if (copy?.stale) return <span className="muted" title="Last-known copy. Reconnect and press Find models before using it.">◷ last known</span>;
  if (status && status.state !== "ready") return <span className="muted" title="This node has no successful current scan. Its copies are unknown.">unknown</span>;
  if (!copy) return <span className="muted" title="No copy">—</span>;
  if (copy.missingCacheFiles && copy.partial === "stopped") return <span className="copy-found" title="Some Hugging Face cache links point to missing files. This copy cannot be loaded or used as a copy source. Size counts only files still present; no files were changed.">◐ incomplete ({copy.missingCacheFiles} missing)</span>;
  if (copy.partial === "stopped") return <span className="copy-found" title="A download or copy stopped partway. Copy to nodes… picks up where it stopped, and a new download should too.">◐ incomplete</span>;
  if (copy.partial) {
    const p = copy.progress;
    const phase = p?.step ?? copy.partial;
    const percent = p && p.bytesTotal > 0 && (phase === "downloading" || phase === "copying")
      ? ` ${Math.min(100, Math.floor(p.bytesDone / p.bytesTotal * 100))}%` : "";
    return <span className="copy-found" title="Still running. Follow it in Jobs.">◐ {phase}{percent}</span>;
  }
  if (!copy.verified) return <span className="copy-found" title="Found on disk; SparkBox didn't download it, so it hasn't checked it">● found</span>;
  return copy.check === "sizes"
    ? <span className="copy-ok" title="Copied over the fabric: every file's size matches the source. Checksums weren't compared.">✓ sizes match</span>
    : <span className="copy-ok" title={copy.check === "source-sha256" ? "Every file's size and SHA-256 match the source node's fresh manifest" : "Checked against Hugging Face's sizes and checksums"}>✓ verified</span>;
}

/** Still being written on that node: Copy to nodes… and Delete leave it alone. */
function stillRunning(copy: LibraryEntry["copies"][string] | undefined): boolean {
  return copy?.partial === "copying" || copy?.partial === "downloading";
}

/** A folder of links into the Hugging Face cache: its bytes are in the blobs folder beside it. */
export function isHfCache(key: string): boolean {
  return /\/models--[^/]+\/snapshots\//.test(key);
}

/** What Delete says it's about to do, and what it says afterwards. Deleting a cache folder removes only its links. */
export function deleteWords(entry: LibraryEntry, nodes: string[]) {
  const on = nodes.join(", ");
  const size = fmtBytes(entry.sizeBytes);
  if (isHfCache(entry.key)) {
    return {
      title: `Remove ${entry.repo}'s links from ${on}?`,
      body: `This removes the folder of links at ${entry.key}. It frees almost no disk space: the model's ${size} stay in the Hugging Face cache's blobs folder, and the model leaves this list. Help's "Delete a model" shows how to free the space.`,
      confirmLabel: "Remove links",
      doneTitle: `Removed the links to ${entry.repo} from ${on}`,
      doneBody: `Its ${size} are still on disk.`,
    };
  }
  return {
    title: `Delete ${entry.repo} from ${on}?`,
    body: `This removes ${entry.key} (${size}) from ${nodes.length === 1 ? "that node" : "those nodes"}. You'd need to download or copy it again to use it there.`,
    confirmLabel: "Delete",
    doneTitle: `Deleted ${entry.repo} from ${on}`,
    doneBody: undefined,
  };
}

export function LibraryTab({ library, error, scanning, nodes, statuses, onRefresh, onLoad, onJobs, onShowLoaded }: {
  library: LibraryEntry[] | null;
  error: string | null;
  scanning: boolean;
  nodes: NodeRef[];
  statuses?: LibraryNodeStatus[];
  onRefresh: () => void;
  onLoad: (key: string) => void;
  onJobs: () => void;
  onShowLoaded: () => void;
}) {
  const admin = useAdmin();
  const [selected, setSelected] = useState<string | null>(null);
  const [mode, setMode] = useState<"share" | "delete" | null>(null);
  const [picked, setPicked] = useState<string[]>([]);
  const [checkingDisk, setCheckingDisk] = useState(false);
  const entry = library?.find((e) => e.key === selected) ?? null;

  const current = (n: NodeRef) => n.online && (!statuses || statuses.find((s) => s.node === n.id)?.state === "ready");
  const eligible = (n: NodeRef, m: "share" | "delete") => !!entry && current(n) && !entry.copies[n.id]?.stale
    && !stillRunning(entry.copies[n.id]) && (m === "share" ? !finishedCopy(entry, n.id) : !!entry.copies[n.id]);
  const hasSource = !!entry && nodes.some((n) => current(n) && finishedCopy(entry, n.id));
  const canAct = !!mode && picked.length > 0 && picked.every((id) => nodes.some((n) => n.id === id && eligible(n, mode)))
    && (mode !== "share" || hasSource) && (mode !== "delete" || !entry?.inUse);
  // A scan/connectivity change during a disk check or consent dialog requires a new review.
  const actionStamp = JSON.stringify([entry, picked, mode, canAct]);
  const latestAction = useRef({ stamp: actionStamp, version: 0 });
  if (latestAction.current.stamp !== actionStamp) {
    latestAction.current = { stamp: actionStamp, version: latestAction.current.version + 1 };
  }
  const actionVersion = latestAction.current.version;

  const columns: Column<LibraryEntry>[] = [
    {
      key: "repo", label: "Model", sort: (e) => e.repo, priority: 1,
      render: (e) => (
        <span className="ai-lib-name">
          <span className="strong">{e.repo}</span>
          {e.inUse && (
            <button type="button" className="ai-badge-btn" onClick={(ev) => { ev.stopPropagation(); onShowLoaded(); }} title={`Being served by ${e.inUse}`}>
              <Badge tone="good">in use</Badge>
            </button>
          )}
        </span>
      ),
    },
    { key: "size", label: "Size", align: "right", sort: (e) => e.sizeBytes, render: (e) => <span className="num">{fmtBytes(e.sizeBytes)}</span> },
    { key: "format", label: "Format", priority: 3, render: (e) => e.format },
    ...nodes.map<Column<LibraryEntry>>((n, i) => ({
      key: `node-${n.id}`,
      label: n.name,
      help: i === 0 ? "models.library.verified-found" : undefined,
      align: "center",
      priority: 2,
      render: (e) => <CopyCell copy={e.copies[n.id]} status={statuses?.find((s) => s.node === n.id)} />,
    })),
    { key: "path", label: "Folder", priority: 3, fit: "flex", minWidth: 140, title: (e) => e.key, render: (e) => <MiddleEllipsis className="mono" text={e.key.replace(/^\/home\/[^/]+\//, "~/")} title={e.key} /> },
  ];

  const startMode = (m: "share" | "delete") => {
    if (!entry) return;
    setMode(m);
    setPicked(m === "share" ? nodes.filter((n) => eligible(n, "share")).map((n) => n.id) : []);
  };

  const share = async () => {
    if (!entry || checkingDisk || !canAct) return;
    setCheckingDisk(true);
    try {
      const checks = await checkTransferDisk({ kind: "copy", key: entry.key, targets: picked });
      if (!checks) return;
      if (!await confirmAction({ title: `Copy ${entry.repo} to ${picked.join(", ")}?`,
        body: <div className="col gap-md"><DiskPreflight checks={checks} /><p>Copy the files over the fabric and verify them on each selected node. The source and existing job records are kept.</p></div>,
        confirmLabel: "Copy over the fabric", danger: checks.some((c) => !!c.warning) })) return;
      if (latestAction.current.version !== actionVersion) throw new Error("Library changed during review. Find models and review the copy again.");
      const jobs = await api.modelShare(entry.key, picked);
      setMode(null);
      toast({ kind: "info", title: `Copying ${entry.repo}`, body: `To ${jobs.map((j) => j.node).join(", ")} over the fabric.`, actions: [{ label: "Show jobs", run: onJobs }] });
      onJobs();
    } catch (e) {
      toast({ kind: "error", title: "Couldn't start the copy", body: errText(e) });
    } finally {
      setCheckingDisk(false);
    }
  };

  const remove = async () => {
    if (!entry || !canAct) return;
    const words = deleteWords(entry, picked);
    const ok = await confirmAction({
      title: words.title,
      body: words.body,
      confirmLabel: words.confirmLabel,
      danger: true,
      requireTyping: picked.length > 1 ? entry.repo.split("/").pop() : undefined,
    });
    if (!ok) return;
    try {
      if (latestAction.current.version !== actionVersion) throw new Error("Library changed during review. Find models and review deletion again.");
      await api.modelDelete(entry.key, picked);
      setMode(null);
      toast({ kind: "success", title: words.doneTitle, body: words.doneBody });
      onRefresh();
    } catch (e) {
      toast({ kind: "error", title: "Couldn't delete it", body: errText(e) });
    }
  };

  return (
    <div className="col fill">
      <Toolbar label="Library">
        <ToolButton icon={RefreshCw} label={scanning ? "Looking…" : "Find models"} onClick={onRefresh} disabled={scanning}
          title="Look for model folders on every node: the model folder from Settings and the Hugging Face cache" />
        <HelpTip topic="models.library.find-models" />
        <span className="toolbar-sep" />
        <ToolButton icon={Play} label="Load…" disabled={!hasSource} onClick={() => entry && onLoad(entry.key)} />
        <ToolButton icon={Share2} label="Copy to nodes…" locked={!admin} disabled={!entry || checkingDisk} title={locked(admin) ?? "Copy this model to other nodes over the 200G fabric"} onClick={() => startMode("share")} />
        <ToolButton icon={Trash2} label="Delete…" locked={!admin} disabled={!entry || !!entry?.inUse || checkingDisk} title={entry?.inUse ? "It's being served; unload it first" : locked(admin)} onClick={() => startMode("delete")} priority={2} />
        <Spacer />
        <HelpTip topic="models-library" />
      </Toolbar>
      {statuses && <div className="ai-library-scans" aria-label="Library scan results">
        {statuses.map((s) => <div key={s.node}>
          <strong>{nodes.find((n) => n.id === s.node)?.name ?? s.node}</strong>{": "}
          {s.state === "ready" ? "Scanned" : s.state === "offline" ? "Offline" : s.state === "error" ? "Scan failed" : "Not scanned"}
          {s.checkedAtMs ? <> · {s.state === "ready" ? "checked" : "last successful scan"} <Ago ts={s.checkedAtMs / 1000} /></> : " · copies unknown"}
          {s.state !== "ready" && s.checkedAtMs && " · showing last-known copies"}
          {s.error && <span className="muted"> · {s.error}</span>}
        </div>)}
      </div>}
      {error && <Notice tone="error" onRetry={onRefresh}>{error}</Notice>}
      {mode && entry && (
        <div className="inline-panel">
          <span className="strong">{mode === "share" ? "Copy to:" : "Delete from:"}</span>
          {nodes.map((n) => {
            const copy = entry.copies[n.id];
            const running = stillRunning(copy);
            const has = !!finishedCopy(entry, n.id);
            const usable = eligible(n, mode);
            const tag = !current(n) || copy?.stale ? "scan needed" : copy?.partial === "stopped" ? "incomplete" : running ? copy?.partial : has ? "has it" : null;
            return (
              <label key={n.id} className="check" title={running ? "Still running. Follow it in Jobs." : mode === "share" && has ? "Already has it" : undefined}>
                <input type="checkbox" disabled={(!usable && !picked.includes(n.id)) || checkingDisk} checked={picked.includes(n.id)}
                  onChange={(e) => setPicked(e.target.checked ? [...picked, n.id] : picked.filter((p) => p !== n.id))} />
                {n.name}
                {(mode === "share" || running || !current(n) || copy?.stale) && tag && <span className="muted"> · {tag}</span>}
              </label>
            );
          })}
          {!canAct && picked.length > 0 && <span className="muted">Find models again or remove unavailable targets. A current finished source is required to copy.</span>}
          <Spacer />
          {mode === "share"
            ? <button className="btn primary" disabled={!canAct || checkingDisk} onClick={share}>{checkingDisk ? "Checking disk…" : "Copy over the fabric"}</button>
            : <button className="btn danger" disabled={!canAct} onClick={remove}>Delete…</button>}
          <button className="btn" disabled={checkingDisk} onClick={() => setMode(null)}>Cancel</button>
        </div>
      )}
      <div className="ai-lib-body">
      <div className="ai-lib-main">
      <Table
        columns={columns}
        rows={library ?? []}
        rowKey={(e) => e.key}
        selected={selected}
        onSelect={setSelected}
        onActivate={(e) => { if (finishedNodes(e).length) onLoad(e.key); }}
        tableId="models.library"
        footer={<HelpTip topic="models.library.copies" label="About copies" />}
        itemNoun={["model", "models"]}
        ariaLabel="Models on the cluster"
        empty={library === null && !error ? "Looking for models on each node…" : error || statuses?.some((s) => s.state !== "ready") ? "No confirmed models to show. Some nodes could not be scanned; their copies are unknown. Reconnect and press Find models." : "No models found. Download one from the Hugging Face tab, or set the model folder in Settings › Models and press Find models."}
      />
      <OllamaGroup />
      </div>
      {entry && <LibraryDetailPane entry={entry} nodes={nodes} onShowLoaded={onShowLoaded} />}
      </div>
    </div>
  );
}

/** The deployment that last ran a folder (same folder name): "glm53 (failed Oct 26)". */
export function lastUsedBy(ds: readonly Deployment[], key: string): Deployment | null {
  const name = (key.replace(/\/+$/, "").split("/").pop() ?? key).toLowerCase();
  return ds.filter((d) => [d.label, d.model ?? ""].some((x) => (x.replace(/\/+$/, "").split("/").pop() ?? "").toLowerCase() === name)).sort((a, b) => b.startedMs - a.startedMs)[0] ?? null;
}

function quantOf(e: LibraryEntry, d: LibraryDetail | null): string {
  if (d?.quantization) return d.quantization.toUpperCase();
  const m = /(nvfp4|mxfp4|fp8|fp4|w4a16|w8a8|awq|gptq|int4|int8|bf16)/i.exec(e.repo);
  return m ? m[1].toUpperCase() : "none (bf16)";
}

/** One line per folder: the nodes that hold a copy there, then the folder (usually the same on every node). */
function copyGroups(copies: LibraryEntry["copies"]): { path: string; nodes: string[] }[] {
  const by = new Map<string, string[]>();
  for (const [n, c] of Object.entries(copies)) by.set(c.path, [...(by.get(c.path) ?? []), n]);
  return [...by].map(([path, nodes]) => ({ path, nodes }));
}

/** What the selected folder is, where its copies are, and what last ran it. */
function LibraryDetailPane({ entry, nodes, onShowLoaded }: { entry: LibraryEntry; nodes: NodeRef[]; onShowLoaded: () => void }) {
  const deployments = useDeployments();
  const [detail, setDetail] = useState<LibraryDetail | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    setDetail(null);
    setFailed(null);
    modelsApi.libraryDetail(entry.key).then((d) => live && setDetail(d), (e) => live && setFailed(errText(e)));
    return () => {
      live = false;
    };
  }, [entry.key]);
  const used = lastUsedBy(deployments, entry.key);
  const a = detail?.arch;
  const name = (n: string) => nodes.find((x) => x.id === n)?.name ?? n;
  const config = a && a.layers
    ? [
        detail?.architectures[0] ?? detail?.modelType,
        `${a.layers} layers`,
        a.experts ? `MoE ${a.experts} experts, ${a.expertsActive ?? "?"} active` : null,
        a.sparseMla ? "sparse MLA attention" : a.mlaDim ? "MLA attention" : null,
        a.maxContext ? `${a.maxContext.toLocaleString()} tokens max` : null,
      ].filter(Boolean).join(" · ")
    : null;
  return (
    <aside className="ai-lib-detail" aria-label={`About ${entry.repo}`}>
      <h4 className="strong">{entry.repo}</h4>
      <dl>
        <dt>Size</dt><dd className="num">{fmtBytes(entry.sizeBytes)}</dd>
        <dt>Copies</dt>
        <dd>
          {copyGroups(entry.copies).map(({ path, nodes: on }) => (
            <div key={path} className="ai-lib-copy">
              <span>{listNames(on.map((n) => (entry.copies[n].stale ? `${name(n)} (last known)` : entry.copies[n].partial ? `${name(n)} (${entry.copies[n].partial === "stopped" ? "incomplete" : entry.copies[n].progress?.step ?? entry.copies[n].partial})` : name(n))))}</span>
              <MiddleEllipsis className="mono muted" text={path.replace(/^\/home\/[^/]+\//, "~/")} title={path} />
            </div>
          ))}
        </dd>
        {Object.entries(entry.copies).filter(([, c]) => c.missingCacheFiles).map(([id, c]) => <Fragment key={id}>
          <dt>{name(id)}{c.stale ? " (last known)" : ""}</dt>
          <dd>{c.missingCacheFiles} missing cache {c.missingCacheFiles === 1 ? "file" : "files"}. Size counts only files still present. Find models again after restoring the missing files; SparkBox does not repair or remove them.</dd>
        </Fragment>)}
        <dt>Format</dt><dd>{entry.format}</dd>
        <dt>Quantization</dt><dd>{quantOf(entry, detail)}</dd>
        <dt>Config</dt>
        <dd>
          {config ?? (failed
            ? <span className="muted" title={failed}>Couldn't read config.json</span>
            : <span className="muted">{detail ? "No config.json in this folder" : "Reading config.json…"}</span>)}
        </dd>
        <dt>In memory</dt>
        <dd>{entry.inUse ? <button type="button" className="link" onClick={onShowLoaded}>yes, served by {entry.inUse}</button> : "no"}</dd>
        <dt>Last used by</dt>
        <dd>
          {used
            ? <span title={used.postmortem?.title ? `Likely cause: ${used.postmortem.title}` : undefined}>
                {used.id} ({used.state === "failed" ? "failed" : used.state} {new Date(used.finishedMs ?? used.startedMs).toLocaleDateString([], { month: "short", day: "numeric" })})
              </span>
            : <span className="muted">nothing SparkBox has seen</span>}
        </dd>
      </dl>
    </aside>
  );
}

/** Ollama's models on each node (from its /api/tags), read-only: Ollama manages its own store. */
function OllamaGroup() {
  const endpoints = useEndpoints().filter((e) => e.engine === "ollama" && (e.onDisk?.length ?? 0) > 0);
  if (!endpoints.length) return null;
  const rows = endpoints.flatMap((e) => (e.onDisk ?? []).map((tag, i) => ({ tag, node: e.headNode, bytes: e.onDiskBytes?.[i] ?? 0, loaded: !!e.ollama?.some((m) => m.name === tag), owner: endpointLabel(e) })));
  return (
    <section className="ai-lib-ollama" aria-label="Ollama models">
      <h4 className="section-title">Ollama</h4>
      <ul>
        {rows.map((r) => (
          <li key={`${r.node}/${r.tag}`} title={`${r.owner} keeps it in its own store; manage it in the Ollama window`}>
            <span className="mono">{r.tag}</span>
            <span className="muted"> · {r.node}{r.bytes ? ` · ${fmtBytes(r.bytes)}` : ""}{r.loaded ? " · in memory" : ""}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
