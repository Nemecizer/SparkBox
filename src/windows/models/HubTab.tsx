// Hugging Face: search, see whether a model fits the cluster (the fit table),
// ask the AI advisor, download, and go on to load it.
import { useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Gauge, HardDriveDownload, Play, Search, Sparkles } from "lucide-react";
import { Table, type Column } from "../../components/Table";
import { Badge, EmptyState, HelpTip, LoadingState, MiddleEllipsis, Notice, Toolbar, ToolButton } from "../../components/ui";
import { api } from "../../lib/api";
import { isAdminUnlocked, useAdmin } from "../../lib/admin";
import { fmtBytes } from "../../lib/format";
import { finishedCopy, finishedNodes, MLX_NOTE, modelsApi, type AnalysisResult, type HfHit, type LibraryEntry, type ModelDetails } from "../../lib/modelsApi";
import { confirmAction } from "../../lib/session";
import { toast } from "../../lib/toasts";
import type { Analysis, DownloadSelection, HfRef } from "../../lib/types";
import { useWinState } from "../../wm/WindowManager";
import { chooseDownloadTool, downloadToolConsent } from "./DownloadToolChoice";
import { checkTransferDisk, DiskPreflight } from "./DiskPreflight";
import { DownloadCapacity } from "./DownloadCapacity";
import { FitTable } from "./FitTable";
import { forgetQueued, noteSize, requestSize, sizeOf, useSizes } from "./hfSizes";
import { Ago, errText, locked, useNodes } from "./shared";

/** What the Load page opens with: a library folder and, optionally, the AI's advice or a load to edit. */
export interface Draft {
  key: string;
  rec?: Analysis["recommended"];
  allowed?: string[][];
  notApplied?: string[];
  repo?: string;
  /** Edit & reload: this load's settings, replacing it when you press Load. */
  spec?: import("../../lib/modelsApi").DeploySpec;
  replaces?: string;
  /** The engine to open with (the SGLang launcher asks for "sglang"). */
  engine?: import("../../lib/modelsApi").LoadEngine;
}

type Hit = HfHit;

/** Asks for a row's real size once it's on screen (and stops asking when it scrolls away first). */
function useOnScreen(repo: string): React.RefObject<HTMLSpanElement | null> {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === "undefined") {
      requestSize(repo);
      return;
    }
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) requestSize(repo);
      else forgetQueued(repo);
    });
    io.observe(el);
    return () => io.disconnect();
  }, [repo]);
  return ref;
}

/** The served weights from the repo's files: "…" until they're read, never a wrong estimate. */
function SizeCell({ h }: { h: Hit }) {
  const ref = useOnScreen(h.id);
  const e = sizeOf(h.id);
  if (e?.sizing) {
    const s = e.sizing;
    if (s.weightsFormat === "none") {
      return <span ref={ref} className="muted" title={s.gguf ? "Only GGUF files: for Ollama or llama.cpp, not vLLM" : "No safetensors or .bin weights"}>{s.gguf ? "GGUF" : "—"}</span>;
    }
    return <span ref={ref} className="num" title="The weights vLLM serves, from the repo's files (copies like original/ don't count)">{fmtBytes(s.weightBytes)}</span>;
  }
  if (e?.error) {
    // The files couldn't be read: the Hub's parameter counts, marked as an estimate.
    return h.weightBytes
      ? <span ref={ref} className="num muted" title={`Estimated from the Hub's parameter counts: the file sizes couldn't be read (${e.error})`}>≈ {fmtBytes(h.weightBytes)}</span>
      : <span ref={ref} className="muted" title={e.error}>—</span>;
  }
  return <span ref={ref} className="muted" aria-label="Reading the file sizes" title="Reading the file sizes from Hugging Face">…</span>;
}

function isMlx(h: Hit): boolean {
  return !!(sizeOf(h.id)?.sizing?.mlx ?? h.mlx);
}

/** Good starting points on GB10 Sparks, with why. */
const RECOMMENDED: { repo: string; why: string }[] = [
  { repo: "Qwen/Qwen3.8-27B-FP8", why: "FP8 on one Spark, with room for long context" },
  { repo: "nvidia/GLM-5.3-Flash-NVFP4", why: "NVFP4 mixture of experts across 3–4 Sparks" },
  { repo: "openai/gpt-oss-120b", why: "MXFP4 mixture of experts: one Spark, fast" },
  { repo: "meta-llama/Llama-3.3-70B-Instruct", why: "bf16 dense: two or more Sparks, slow per stream" },
];

const VERDICT: Record<Analysis["verdict"], { text: string; tone: string }> = {
  good: { text: "Serves well", tone: "good" },
  workable: { text: "Workable", tone: "warn" },
  not_recommended: { text: "Not recommended", tone: "serious" },
  wont_fit: { text: "Won't fit", tone: "crit" },
};

export function HubTab({ library, onLoad, onDownloading }: {
  library: LibraryEntry[];
  onLoad: (d: Draft) => void;
  onDownloading: () => void;
}) {
  const admin = useAdmin();
  const nodes = useNodes();
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[] | null>(null);
  const [task, setTask] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);
  const [repo, setRepo] = useState<string | null>(null);
  const [details, setDetails] = useState<ModelDetails | null>(null);
  const [revisionInput, setRevisionInput] = useState("main");
  const [inspectedRevision, setInspectedRevision] = useState("main");
  const inspection = useRef(0);
  useEffect(() => () => { inspection.current++; }, []);
  const [analysis, setAnalysis] = useState<AnalysisResult | null>(null);
  const [busy, setBusy] = useState<"details" | "analysis" | "download" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [split, setSplit] = useWinState("hub.split", 480);
  const [targetId, setTargetId] = useWinState<string | null>("hub.downloadNode", null);
  const target = nodes.find((n) => n.id === targetId);
  const [copyTargets, setCopyTargets] = useState<string[]>([]);
  // A change of source or inspected model requires a new explicit copy choice.
  useEffect(() => { setCopyTargets([]); }, [targetId, details?.model.id, details?.model.revision]);
  // Choose once, then keep the stable ID even if the list reorders or the node disappears.
  useEffect(() => {
    if (targetId === null && nodes.some((n) => n.online)) setTargetId(nodes.find((n) => n.online)!.id);
  }, [nodes, targetId, setTargetId]);
  const splitRef = useRef<HTMLDivElement>(null);

  // Drag the divider (or use ←/→ on it) to share the width between results and details.
  const drag = (e: ReactPointerEvent) => {
    const box = splitRef.current?.getBoundingClientRect();
    if (!box) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => setSplit(Math.round(Math.min(box.width - 360, Math.max(260, ev.clientX - box.left))));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const inspect = async (id: string, revision = "main") => {
    if (busy === "download" || busy === "analysis") return;
    const request = ++inspection.current;
    revision = revision.trim();
    setRevisionInput(revision);
    setInspectedRevision(revision);
    setRepo(id);
    setDetails(null);
    setAnalysis(null);
    setError(null);
    setBusy("details");
    try {
      const d = await modelsApi.hfModel(id, revision === "main" ? undefined : revision);
      if (request !== inspection.current) return;
      setDetails(d);
      setRepo(d.model.id);
      // The row says what the detail says. With no bf16 kernel on GB10 (sparse MLA), the fewest
      // Sparks are the FP8 table's, not "more than 4".
      const bf16 = d.fit.find((r) => r.fits)?.nodes ?? null;
      const fp8 = bf16 == null && d.fit.some((r) => r.noKernel) ? (d.fitFp8?.find((r) => r.fits)?.nodes ?? null) : null;
      if (revision === "main") noteSize({
        id: d.model.id, weightBytes: d.model.weightBytes, weightsFormat: d.model.weightsFormat, mlx: !!d.model.mlx,
        minNodes: bf16 ?? fp8, kvFp8: fp8 != null, gguf: d.model.skipped.some((x) => /GGUF/.test(x.what)),
      });
    } catch (e) {
      if (request === inspection.current) setError(errText(e));
    } finally {
      if (request === inspection.current) setBusy(null);
    }
  };

  const search = async () => {
    const q = query.trim();
    if (!q) return;
    setError(null);
    // A pasted repo id or huggingface.co link goes straight to its details.
    if (/^https?:\/\/huggingface\.co\//.test(q) || /^[\w.-]+\/[\w.-]+$/.test(q)) inspect(q);
    setSearching(true);
    try {
      setHits((await api.hfSearch(q.replace(/^https?:\/\/huggingface\.co\//, ""))) as Hit[]);
      setTask(null);
    } catch (e) {
      setError(errText(e));
    } finally {
      setSearching(false);
    }
  };

  const analyze = async () => {
    if (!repo) return;
    setBusy("analysis");
    setError(null);
    try {
      setAnalysis(await modelsApi.modelsAnalyze(repo, details?.model.revision));
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  };

  const download = async () => {
    if (!repo || !details || details.model.configError || revisionInput.trim() !== inspectedRevision || !target?.online || !isAdminUnlocked() || busy === "download") return;
    const selection: DownloadSelection = { repo: details.model.id, revision: details.model.revision, files: structuredClone(details.model.files) };
    const head = target.name;
    const resume = presence.kind === "incomplete";
    setBusy("download");
    setError(null);
    try {
      const review = await chooseDownloadTool(target.id);
      if (!review) return;
      const chain = copyTargets.length ? await api.modelChainReview(selection, target.id, [...copyTargets]) : null;
      const checks = chain ? chain.checks : await checkTransferDisk({ kind: "download", node: target.id, selection });
      if (!checks) return;
      const installing = review.tool.kind === "install";
      const ok = await confirmAction({
        title: chain ? `Download ${repo} to ${head}, then copy to ${chain.targets.join(", ")}?` : installing ? `Install the tool and download ${repo} to ${head}?` : resume ? `Download ${repo} again to ${head}?` : `Download ${repo} to ${head}?`,
        body: <div className="col gap-md"><DiskPreflight checks={checks} />{chain && <div className="col gap-sm"><p>The chain needs empty destination folders. It arms the targets, downloads and verifies on the source, then copies and checks every target against fresh SHA-256 hashes. No copy starts before source verification.</p><p>Keep SparkBox open until all jobs appear. The nodes then continue on their own, for at most 24 hours. The temporary file server stops after acknowledgments, 30 minutes without activity, cancellation or expiry. This does not load the model. If the source fails before sharing, targets wait until canceled or expired. A failed or rebooted chain needs explicit recovery; reopening Jobs starts no replacement work.</p>{chain.nodes.map((n) => <span className="mono ai-capacity-path" key={n.node}>{n.node}: {n.dest} · fabric {n.address}</span>)}<span>Temporary source port: {chain.port}. No firewall rule is changed.</span></div>}<p>{`${downloadToolConsent(review)} ${resume ? "For the original revision and files, use Resume download… in Jobs. Download again uses the revision inspected here. " : ""}${fmtBytes(details.model.downloadBytes)} in ${selection.files.length} files: the weights vLLM serves and their config${details.model.skipped.length ? ` (not ${details.model.skipped.map((x) => x.what).join(" or ")})` : ""}. SparkBox rechecks this exact commit and file list before starting. Any mismatch stops the download for a new review. The downloaded files are verified. ${chain ? "Keep SparkBox open until setup finishes; this chain still needs separate live acceptance on your cluster." : "Keep SparkBox open and this Mac awake; unattended recovery is not proven on your cluster."}`}</p><span className="mono wrap">Commit {selection.revision}</span></div>,
        confirmLabel: chain ? installing ? "Install tool and start chain" : "Start download and copies" : installing ? "Install tool and download" : resume ? "Download again" : "Download",
        danger: installing,
      });
      if (!ok) return;
      review.installConsent = installing;
      if (chain) await api.modelChainStart(chain, review);
      else await api.modelDownload(selection, review);
      toast({ kind: "info", title: `Downloading ${repo}`, body: `To ${head}. Follow it in Jobs.` });
      onDownloading();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  };

  const inLibrary = repo ? libraryMatch(library, repo, targetId ?? undefined) : undefined;
  const presence = hubPresence(inLibrary, targetId ?? undefined);
  const loadEntry = repo ? libraryMatch(library, repo) : undefined;
  const ready = loadEntry && finishedNodes(loadEntry).length ? loadEntry : undefined;
  const tasks = useMemo(() => [...new Set((hits ?? []).map((h) => h.pipeline).filter((p): p is string => !!p))].sort(), [hits]);
  const shown = (hits ?? []).filter((h) => !task || h.pipeline === task);

  useSizes();
  const hitColumns: Column<Hit>[] = [
    {
      key: "id", label: "Model", sort: (h) => h.id, priority: 1, minWidth: 180,
      render: (h) => {
        // The repo's name first (cut in the middle, so the part that differs shows), its org under it.
        const [org, name] = h.id.includes("/") ? [h.id.slice(0, h.id.indexOf("/")), h.id.slice(h.id.indexOf("/") + 1)] : ["", h.id];
        return (
          <span className="ai-hit-name" title={h.id}>
            <MiddleEllipsis text={name} keepEnd={Math.min(14, Math.floor(name.length / 2))} className="strong" title={h.id} />
            <span className="ai-hit-org">
              <span className="muted">{org}</span>
              {isMlx(h) && <Badge tone="neutral" title={MLX_NOTE}>MLX · Apple only</Badge>}
            </span>
          </span>
        );
      },
    },
    { key: "pipeline", label: "Task", priority: 3, sort: (h) => h.pipeline ?? "", render: (h) => h.pipeline ?? "—" },
    { key: "size", label: "Weights", help: "models.hub.weights", align: "right", sort: (h) => sizeOf(h.id)?.sizing?.weightBytes ?? -1, render: (h) => <SizeCell h={h} /> },
    {
      key: "fit", label: "Fits on", help: "models.hub.fits-on", sort: (h) => (isMlx(h) ? 100 : sizeOf(h.id)?.sizing?.minNodes ?? 99), priority: 2,
      title: () => "The fewest Sparks whose vLLM share holds the weights (context needs more; see the fit table)",
      render: (h) => {
        if (isMlx(h)) return <span className="muted">—</span>;
        const s = sizeOf(h.id)?.sizing;
        if (!s) return <span className="muted">…</span>;
        if (s.weightsFormat === "none") return <span className="muted">—</span>;
        if (!s.minNodes) return <span className="warn-text">more than 4</span>;
        const n = s.minNodes === 1 ? "1 Spark" : `${s.minNodes} Sparks`;
        return s.kvFp8 ? <span title="Only with --kv-cache-dtype fp8: GB10 has no kernel for this model's attention with a bf16 KV cache">{n} · fp8</span> : <span>{n}</span>;
      },
    },
    { key: "downloads", label: "Downloads", align: "right", priority: 3, sort: (h) => h.downloads, render: (h) => <span className="num">{h.downloads.toLocaleString()}</span> },
    { key: "likes", label: "Likes", align: "right", priority: 3, sort: (h) => h.likes, render: (h) => <span className="num">{h.likes.toLocaleString()}</span> },
    { key: "updated", label: "Updated", priority: 3, sort: (h) => h.lastModified ?? "", render: (h) => (h.lastModified ? <Ago ts={Date.parse(h.lastModified) / 1000} /> : "—") },
  ];

  return (
    <div className="col fill">
      <Toolbar label="Hugging Face">
        <input
          type="search"
          className="wide"
          placeholder="Search Hugging Face, or paste org/name or a link"
          aria-label="Search Hugging Face"
          value={query}
          data-find-target
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && search()}
        />
        <ToolButton icon={Search} label={searching ? "Searching…" : "Search"} onClick={search} disabled={searching || !query.trim()} />
        <HelpTip topic="models-hugging-face" />
      </Toolbar>
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
      <div className="ai-hub" ref={splitRef} style={{ gridTemplateColumns: `${split}px 6px minmax(0, 1fr)` }}>
        <div className="ai-hub-hits">
          {hits ? (
            <>
              {tasks.length > 1 && (
                <div className="ai-chips" role="group" aria-label="Task">
                  <button type="button" className={`chip${task === null ? " on" : ""}`} onClick={() => setTask(null)}>All tasks</button>
                  {tasks.map((t) => (
                    <button key={t} type="button" className={`chip${task === t ? " on" : ""}`} onClick={() => setTask(task === t ? null : t)}>{t}</button>
                  ))}
                </div>
              )}
              <Table columns={hitColumns} rows={shown} rowKey={(h) => h.id} selected={repo} onSelect={(id) => inspect(id)} empty="No models match."
                tableId="models.hub" footer itemNoun={["model", "models"]} ariaLabel="Search results" />
            </>
          ) : (
            <div className="ai-recommended">
              <div className="section-title">Recommended for GB10 <HelpTip topic="models.hub.recommended" /></div>
              {RECOMMENDED.map((r) => (
                <button key={r.repo} type="button" className={`ai-rec${repo === r.repo ? " on" : ""}`} onClick={() => inspect(r.repo)}>
                  <span className="strong">{r.repo}</span>
                  <span className="muted">{r.why}</span>
                </button>
              ))}
            </div>
          )}
        </div>
        <div
          className="ai-splitter"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize the results list"
          aria-valuenow={split}
          tabIndex={0}
          onPointerDown={drag}
          onKeyDown={(e) => {
            if (e.key === "ArrowLeft") setSplit((w) => Math.max(260, w - 24));
            if (e.key === "ArrowRight") setSplit((w) => w + 24);
          }}
        />
        <div className="ai-hub-detail">
          {!repo && (
            <EmptyState icon={Gauge} title="Will it fit?"
              body="Search for a model, or pick one on the left, to see how much memory it needs and how many Sparks it takes." />
          )}
          {repo && busy === "details" && <LoadingState label={`Reading ${repo} from Hugging Face…`} />}
          {repo && <RevisionPicker repo={repo} value={revisionInput} onChange={setRevisionInput} onInspect={(revision) => inspect(repo, revision)} busy={busy !== null} />}
          {details && revisionInput.trim() !== inspectedRevision && <Notice tone="info">Click Inspect revision to review this choice before downloading.</Notice>}
          {details && (
            <ModelFacts details={details} downloadTarget={!details.model.mlx && <>
                  <label className="row wrap gap-sm">Download node <HelpTip topic="models.hub.download-target" />
                    <select aria-label="Download node" value={targetId ?? ""} disabled={busy !== null} onChange={(e) => setTargetId(e.target.value)}>
                      {targetId === null && <option value="">Choose a connected node</option>}
                      {targetId !== null && !target && <option value={targetId}>{targetId} — removed from inventory</option>}
                      {nodes.map((n) => <option key={n.id} value={n.id} disabled={!n.online}>{n.name}{n.online ? "" : " — not connected"}</option>)}
                    </select>
                  </label>
                  <details><summary>Then copy to</summary>
                    <div className="col gap-sm"><p>Choose copy targets for a node-side download → verify → copy chain. All destinations must be empty. Existing copies stay in the Library.</p>
                      <div className="ai-capacity-row" role="group" aria-label="Then copy to">{nodes.filter((n) => n.id !== targetId).map((n) => {
                        const existing = hubPresence(libraryMatch(library, details.model.id, n.id), n.id).kind !== "none";
                        return <label className="ai-capacity-check" key={n.id}><input type="checkbox" aria-label={`Then copy to ${n.name}`} checked={copyTargets.includes(n.id)} disabled={busy !== null || ((!n.online || existing) && !copyTargets.includes(n.id))}
                          onChange={(e) => setCopyTargets((old) => e.target.checked ? [...old,n.id] : old.filter((id) => id !== n.id))} />{n.name}{!n.online ? " — not connected" : existing ? " — files already present" : ""}</label>;
                      })}</div>
                      {copyTargets.length > 0 && <span>Copy targets: {copyTargets.join(", ")}. Final review checks every destination and fabric route.</span>}
                    </div>
                  </details>
                  <DownloadCapacity key={`${details.model.id}:${details.model.revision}`} selection={{ repo: details.model.id, revision: details.model.revision, files: details.model.files }}
                    nodes={nodes.map((node) => {
                      const state = hubPresence(libraryMatch(library, details.model.id, node.id), node.id);
                      return { ...node, unavailable: state.kind === "finished" ? "copy already finished" : state.kind === "running" ? "transfer running" : undefined };
                    })} target={targetId} onSelect={setTargetId}
                    disabled={busy !== null || !!details.model.configError || details.model.weightsFormat === "none" || !details.model.files.length || revisionInput.trim() !== inspectedRevision} />
                  {!target?.online && <span className="muted">Choose a connected download node. The target is never changed automatically.</span>}
                </>}>
              {/* MLX weights don't run on a GB10: nothing to analyze, download or load. */}
              {!details.model.mlx && (
                <>
                  <ToolButton icon={Sparkles} label={busy === "analysis" ? "Analyzing…" : "Check with AI"} onClick={analyze} disabled={busy !== null || !!details.model.configError || revisionInput.trim() !== inspectedRevision}
                    title="Ask your Anthropic or OpenAI model (Settings › Models) whether this serves well on the cluster, and how to split it" />
                  <HelpTip topic="models.hub.check-with-ai" />
                  {ready && (
                    <span className="ai-hub-have" title={finishedNodes(ready).map((n) => `${n}: ${ready.copies[n].path}`).join("\n")}>
                      {copiesText(ready, nodes.length)}
                    </span>
                  )}
                  {presence.kind === "running" ? (
                    <span className="ai-hub-have">{presence.partial === "copying" ? "Copying" : "Downloading"} on {target?.name ?? presence.node}</span>
                  ) : presence.kind !== "finished" && (
                    <>
                      {presence.kind === "incomplete" && <span className="ai-hub-have">Incomplete on {target?.name ?? presence.node}</span>}
                      <ToolButton icon={HardDriveDownload} label={busy === "download" ? "Starting…" : `${presence.kind === "incomplete" ? "Download again" : "Download"} to ${target?.name ?? "selected node"}`}
                        onClick={download} locked={!admin} disabled={!target?.online || busy !== null || details.model.weightsFormat === "none" || !!details.model.configError || !details.model.files.length || revisionInput.trim() !== inspectedRevision} title={locked(admin)} />
                    </>
                  )}
                  <ToolButton icon={Play} label="Load…" disabled={!ready} locked={!!ready && !admin}
                    onClick={() => {
                      if (!ready) return;
                      const matchingAdvice = ready.revision === details.model.revision ? analysis : null;
                      onLoad({ key: ready.key, rec: matchingAdvice?.analysis.recommended, allowed: matchingAdvice?.allowedArgs, notApplied: matchingAdvice?.notApplied, repo: details.model.id });
                    }}
                    title={ready ? (admin ? `Load the saved copy at ${ready.key}, revision ${ready.revision || "unknown"}` : locked(admin)) : "Download it first"} />
                </>
              )}
            </ModelFacts>
          )}
          {details && ready && ready.revision !== details.model.revision && <p className="muted pad">Load uses the saved Library copy at revision {ready.revision || "unknown"}. Choosing a revision here changes new downloads; it does not replace a finished copy or apply advice for a different revision.</p>}
          {analysis && <AnalysisCard result={analysis} />}
        </div>
      </div>
    </div>
  );
}

function RevisionPicker({ repo, value, onChange, onInspect, busy }: {
  repo: string; value: string; onChange: (value: string) => void; onInspect: (revision: string) => void; busy: boolean;
}) {
  const [refs, setRefs] = useState<HfRef[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    let active = true;
    setRefs([]); setError(null); setLoading(true);
    modelsApi.hfRefs(repo).then((result) => { if (active) setRefs(result); })
      .catch((e) => { if (active) setError(errText(e)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [repo, refresh]);
  return <div className="col gap-sm pad">
    <div className="row wrap gap-sm"><span>Branch or tag <HelpTip topic="models.hub.revision" /></span>
      <select aria-label="Branch or tag" value="" disabled={busy || loading} onChange={(e) => {
        if (!e.target.value) return;
        const ref = refs[Number(e.target.value)];
        if (ref) onInspect(ref.revision);
      }}>
        <option value="">{loading ? "Reading branches and tags…" : "Choose a branch or tag"}</option>
        {refs.map((ref, index) => <option key={`${ref.kind}:${ref.name}`} value={index}>{ref.name} ({ref.kind}) · {ref.revision.slice(0, 12)}</option>)}
      </select>
      <button type="button" className="btn small" disabled={busy || loading} onClick={() => setRefresh((n) => n + 1)}>Refresh branches</button>
    </div>
    {error && <Notice tone="warn">Branches and tags: {error} You can still enter an exact revision below.</Notice>}
    <form className="row wrap gap-sm" onSubmit={(e) => { e.preventDefault(); if (!busy && value.trim()) onInspect(value.trim()); }}>
      <label className="row gap-sm">Revision <input aria-label="Revision" value={value} disabled={busy} onChange={(e) => onChange(e.target.value)} placeholder="Branch, tag or full commit" /></label>
      <button type="submit" className="btn small" disabled={busy || !value.trim()}>Inspect revision</button>
    </form>
  </div>;
}

function ModelFacts({ details, children, downloadTarget }: { details: ModelDetails; children: React.ReactNode; downloadTarget?: React.ReactNode }) {
  const { model: m, arch } = details;
  const params = m.params ? `${(m.params / 1e9).toFixed(m.params >= 1e10 ? 0 : 1)}B parameters` : "parameters unknown";
  const moe = arch.experts ? ` · MoE ${arch.experts} experts, ${arch.expertsActive ?? "?"} active` : "";
  return (
    <div className="col gap pad">
      <div className="col gap-sm">
        <div className="model-title">
          <a className="strong" href={`https://huggingface.co/${m.id}`} target="_blank" rel="noreferrer">{m.id}</a>
          {m.mlx && <><Badge tone="neutral">MLX · Apple only</Badge> <HelpTip topic="formats-and-quantization#mlx" /></>}
          {m.gated && <><Badge tone="warn" title="Accept the license on huggingface.co and add a token in Settings › Models">gated</Badge> <HelpTip topic="models.hub.gated" /></>}
          {m.license && <span className="muted">{m.license}</span>}
        </div>
        <div className="muted">
          {params}{moe} · {m.weightsFormat === "none" ? "no safetensors or .bin weights" : `${fmtBytes(m.weightBytes)} of ${m.weightsFormat} weights`}
          {m.quantization ? ` · ${m.quantization}` : ""} · {m.architectures.join(", ") || m.modelType || "unknown architecture"}
          {arch.sparseMla ? " · sparse MLA attention" : ""}
        </div>
        {m.weightsFormat !== "none" && !m.mlx && (
          <div className="muted" title={m.skipped.map((x) => `${x.what}: ${x.files} file${x.files === 1 ? "" : "s"}, ${fmtBytes(x.bytes)}`).join("\n") || undefined}>
            Downloads {fmtBytes(m.downloadBytes)}
            {m.skipped.length > 0 && <> of the repo's {fmtBytes(m.repoBytes)}: leaves out {m.skipped.slice(0, 3).map((x) => `${x.what} (${fmtBytes(x.bytes)})`).join(", ")}{m.skipped.length > 3 ? ` and ${m.skipped.length - 3} more` : ""}</>}
          </div>
        )}
      </div>
      <span className="mono wrap">Commit {m.revision}</span>
      <details><summary>Files to download ({m.files.length})</summary>
        <ul className="tight">{m.files.map((f) => <li key={f.path}><span className="mono wrap">{f.path}</span> · {f.size == null ? "size unknown" : fmtBytes(f.size)}{f.sha256 && <div className="mono wrap">SHA-256 {f.sha256}</div>}</li>)}</ul>
      </details>
      {downloadTarget}
      {!m.mlx && <Toolbar label="Model">{children}</Toolbar>}
      {/* Without weights there's nothing to size: a table of "0 B per node, fits" would be a made-up answer. */}
      {m.mlx ? (
        <Notice tone="info">{MLX_NOTE}</Notice>
      ) : m.weightsFormat === "none" ? (
        <Notice tone="info">
          This repo has no safetensors or .bin weights for vLLM to serve{m.skipped.some((x) => /GGUF/i.test(x.what)) ? " (its GGUF files are for Ollama or llama.cpp)" : ""}, so SparkBox can't size it or download it.
        </Notice>
      ) : m.configError ? <Notice tone="warn">{m.configError}</Notice> : <FitTable details={details} />}
    </div>
  );
}

function AnalysisCard({ result }: { result: AnalysisResult }) {
  const a = result.analysis;
  const v = VERDICT[a.verdict];
  const r = a.recommended;
  return (
    <div className="card col gap-sm">
      <div className="model-title">
        <span className={`verdict ${v.tone}`}>{v.text}</span>
        <span className="muted">by {result.provider} · {result.model}</span>
      </div>
      <p className="note">{a.summary}</p>
      <dl className="kv">
        <div><dt>Recommended</dt><dd>
          {r.nodes} node{r.nodes === 1 ? "" : "s"} · {r.parallelism}
          {r.nodes > 1 ? ` (TP ${r.tensorParallelSize} × PP ${r.pipelineParallelSize})` : ""}
          {r.expertParallel ? " · expert parallel" : ""} · context {r.maxModelLen.toLocaleString()} · GPU memory {r.gpuMemoryUtilization}
        </dd></div>
        {result.allowedArgs.length > 0 && <div><dt>Options it fills in</dt><dd className="mono">{result.allowedArgs.map((g) => g.join(" ")).join("  ")}</dd></div>}
        {result.notApplied.length > 0 && (
          <div><dt>AI also suggested (not applied)</dt><dd><span className="mono">{result.notApplied.join("  ")}</span><div className="muted">Not on the list SparkBox lets the advisor set. Add them yourself if you agree.</div></dd></div>
        )}
        <div><dt>Why</dt><dd>{r.why}</dd></div>
        <div><dt>Expected speed</dt><dd>{a.expectedPerformance}</dd></div>
        <div><dt>vLLM support</dt><dd>{a.engineSupport.status} — {a.engineSupport.notes}</dd></div>
        {a.alternatives.map((alt, i) => (
          <div key={i}><dt>{i === 0 ? "Alternatives" : ""}</dt><dd>{alt.nodes} node{alt.nodes === 1 ? "" : "s"}, {alt.parallelism}: {alt.tradeoff}</dd></div>
        ))}
        {a.risks.length > 0 && <div><dt>Risks</dt><dd><ul className="tight">{a.risks.map((x) => <li key={x}>{x}</li>)}</ul></dd></div>}
        {a.betterVariant.repo && <div><dt>Consider</dt><dd><span className="mono">{a.betterVariant.repo}</span> — {a.betterVariant.reason}</dd></div>}
      </dl>
    </div>
  );
}

/**
 * The Library folder holding this repo: the same repo, or a folder named like it (a model
 * copied by hand to ~/models/GLM-5.3-Flash-NVFP4 has no repo of its own).
 */
export function libraryMatch(library: readonly LibraryEntry[], repo: string, target?: string): LibraryEntry | undefined {
  const r = repo.toLowerCase();
  const name = r.split("/").pop() ?? r;
  // A folder left partway doesn't hide a finished one of the same model.
  const best = (found: LibraryEntry[]) => (target ? found.find((e) => finishedCopy(e, target)) ?? found.find((e) => e.copies[target]) : undefined)
    ?? found.find((e) => finishedNodes(e).length > 0) ?? found[0];
  return best(library.filter((e) => e.repo.toLowerCase() === r)) ?? best(library.filter((e) => (e.repo.toLowerCase().split("/").pop() ?? "") === name || (e.key.toLowerCase().replace(/\/+$/, "").split("/").pop() ?? "") === name));
}

/** "On all 4 nodes (~/models/GLM-5.3-Flash-NVFP4)", "On cell1 and cell2 (~/models/…)": finished copies only. */
export function copiesText(e: LibraryEntry, nodeCount: number): string {
  const nodes = finishedNodes(e);
  const where = nodes.length >= nodeCount && nodeCount > 1 ? `all ${nodes.length} nodes` : nodes.length <= 1 ? nodes.join("") : `${nodes.slice(0, -1).join(", ")} and ${nodes[nodes.length - 1]}`;
  const path = (nodes[0] ? finishedCopy(e, nodes[0])?.path : undefined) ?? e.key;
  return `On ${where} (${path.replace(/^\/home\/[^/]+\//, "~/")})`;
}

export type HubPresence =
  | { kind: "none" | "finished" }
  | { kind: "incomplete"; node: string }
  | { kind: "running"; node: string; partial: "downloading" | "copying" };

/**
 * Download state belongs to the selected node. Load availability is calculated separately,
 * so another node's finished copy neither hides this partial folder nor blocks a new download.
 */
export function hubPresence(e: LibraryEntry | undefined, target: string | undefined): HubPresence {
  if (!e || !target) return { kind: "none" };
  if (finishedCopy(e, target)) return { kind: "finished" };
  const partial = e.copies[target]?.partial;
  if (partial === "stopped") return { kind: "incomplete", node: target };
  if (partial) return { kind: "running", node: target, partial };
  return { kind: "none" };
}
