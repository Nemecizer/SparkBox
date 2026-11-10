import { useEffect, useMemo, useRef, useState } from "react";
import { BookOpen, Search, Star } from "lucide-react";
import { api, invoke } from "../lib/api";
import { useDataSource } from "../lib/dataSource";
import { useCluster } from "../lib/store";
import { useCloseGuard, useWM, type Params } from "../wm/WindowManager";
import { requestAdmin } from "../lib/admin";
import { confirmAction } from "../lib/session";
import { HelpTip, Notice } from "../components/ui";
import { onMarkdownClick } from "../lib/markdown";
import { fmtBytes } from "../lib/format";
import type { NodeCheck, ProviderView } from "../lib/types";
import { AI_LIMIT, LIBRARY, aiPrompt, aiProviders, draftWithAI, guideText, howto, importedGuide, matches, replaceSection, sections, sourceUrl, type Document, type Draft, type Guide, type LocalGuide, type SourceCheck, type Summary } from "../howto/library";
import { renderGuide } from "../howto/markdown";
import "../howto/howto.css";

const ACTIONS: Record<string, { title: string; params?: Params }> = {
  models: { title: "Models and downloads", params: { tab: "library" } }, providers: { title: "Connect an AI provider" }, chat: { title: "Test in Chat" }, endpoints: { title: "Inspect endpoints" },
  fabric: { title: "Inspect fabric links" }, topology: { title: "Review topology" }, connect: { title: "Connect or discover nodes" }, updates: { title: "Review package updates" }, files: { title: "Open Files" }, workloads: { title: "Watch running work" },
};
const localText = (g: Guide, local: LocalGuide) => local.editions.at(-1)?.text ?? (LIBRARY.guides.some((b) => b.id === g.id) ? guideText(g) : local.sources.at(-1)?.document.text ?? guideText(g));
function Markdown({ text, revision = LIBRARY.revision, path }: { text: string; revision?: string; path: string }) {
  const html = useMemo(() => renderGuide(text, revision, path), [text, revision, path]);
  return <div className="howto-markdown" onClick={onMarkdownClick} dangerouslySetInnerHTML={{ __html: html }} />;
}
function OptionSections({ text }: { text: string }) {
  return <><option value="all" disabled={text.length > AI_LIMIT}>Whole document{ text.length > AI_LIMIT ? " — choose a smaller section" : ""}</option>{sections(text).map((s, i) => <option key={i} value={i} disabled={s.text.length > AI_LIMIT}>{s.label} ({s.text.length.toLocaleString()} characters)</option>)}</>;
}
function DocumentReader({ document, revision }: { document: Document; revision: string }) {
  const [part, setPart] = useState("all"); const [raw, setRaw] = useState(false); const parts = useMemo(() => sections(document.text), [document.text]);
  useEffect(() => setPart("all"), [document.sha256]);
  const text = part === "all" ? document.text : parts[Number(part)]?.text ?? "";
  return <><label>Read a section <select aria-label={`Read a section of ${document.path}`} value={part} onChange={(e) => setPart(e.target.value)}><option value="all">Entire document</option>{parts.map((s, i) => <option key={i} value={i}>{s.label}</option>)}</select></label><button onClick={() => setRaw(!raw)}>{raw ? "Show formatted text" : "Show original Markdown"}</button>{raw ? <pre className="howto-raw">{text}</pre> : <Markdown text={text} revision={revision} path={document.path} />}</>;
}

export function HowToWindow({ params = {} }: { params?: Params }) {
  const { open } = useWM(); const dataSource = useDataSource(); const demo = dataSource === "demo"; const cluster = useCluster();
  const [id, setId] = useState(params.guide ?? "playbook-vllm");
  const [query, setQuery] = useState(""); const [filter, setFilter] = useState("spark"); const [category, setCategory] = useState("all");
  const [saved, setSaved] = useState<Record<string, Summary>>({}); const [local, setLocal] = useState<LocalGuide | null>(null);
  const [notes, setNotes] = useState(""); const [completed, setCompleted] = useState<number[]>([]);
  const [tab, setTab] = useState("guide"); const [document, setDocument] = useState<Document | null>(null); const [docPath, setDocPath] = useState("");
  const [readme, setReadme] = useState<Document | null>(null);
  const [check, setCheck] = useState<SourceCheck | null>(null); const [busy, setBusy] = useState(""); const [error, setError] = useState(""); const [message, setMessage] = useState("");
  const [draft, setDraft] = useState<Draft | null>(null); const [providers, setProviders] = useState<ProviderView[]>([]); const [provider, setProvider] = useState(""); const [model, setModel] = useState("");
  const [instruction, setInstruction] = useState("Update this section using the selected NVIDIA reference. Explain each step in plain language, keep the safeguards, and state what still needs verification.");
  const [targetPart, setTargetPart] = useState("all"); const [referencePart, setReferencePart] = useState("all"); const [stream, setStream] = useState("");
  const [nodes, setNodes] = useState<string[]>([]); const [facts, setFacts] = useState<NodeCheck[]>([]); const [image, setImage] = useState(""); const [estimate, setEstimate] = useState("30");
  const [compare, setCompare] = useState(false);
  const scope = `${dataSource}:${id}`; const current = useRef(scope); current.current = scope;
  const ai = useRef<{ id: string; canceled: boolean } | null>(null);
  const selectedNodes = useRef(""); selectedNodes.current = nodes.join(",");
  const guides = useMemo(() => [...LIBRARY.guides, ...Object.entries(saved).filter(([id]) => !LIBRARY.guides.some((g) => g.id === id)).map(([id, s]) => importedGuide(id, s.title))], [saved]);
  const g = guides.find((g) => g.id === id) ?? importedGuide(id, id);
  const bundled = LIBRARY.guides.some((g) => g.id === id);
  const latestSource = local?.sources.at(-1);
  const reference = latestSource?.document ?? readme;
  const referenceRevision = latestSource?.revision ?? LIBRARY.revision;
  const base = local ? localText(g, local) : "";
  const editing = draft?.text ?? base;
  const dirty = !!draft || !!local && (notes !== local.notes || JSON.stringify(completed) !== JSON.stringify(local.completed));
  useCloseGuard(() => dirty ? "This guide has unsaved text or progress. Save it or discard it before closing." : busy === "ai" ? "AI is still drafting. Cancel it before closing." : null);
  const accept = (value: LocalGuide) => { setLocal(value); setNotes(value.notes); setCompleted(value.completed); };
  const refreshSaved = (at = scope) => howto.saved(demo).then((v) => { if (current.current === at) setSaved(v); });
  useEffect(() => {
    setSaved({}); setCheck(null); setNodes([]); setFacts([]);
    let active = true;
    void Promise.all([howto.saved(demo), howto.lastCheck(demo)]).then(([s, c]) => { if (active) { setSaved(s); setCheck(c); } }).catch((e) => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [demo]);
  useEffect(() => {
    const at = scope; let active = true;
    setLocal(null); setDocument(null); setReadme(null); setDraft(null); setError(""); setMessage(""); setFacts([]); setBusy(""); setCompare(false); setStream("");
    setImage(g.images[0] ?? ""); setDocPath(g.readme); setTargetPart("all"); setReferencePart("all");
    void howto.read(demo, id).then((v) => { if (active && current.current === at) accept(v); }).catch((e) => { if (active) setError(String(e)); });
    if (bundled) void howto.document(g.readme).then((d) => { if (active && current.current === at) setReadme(d); }).catch((e) => { if (active) setError(String(e)); });
    return () => { active = false; if (ai.current) { ai.current.canceled = true; void api.chatStop(ai.current.id).catch(() => {}); ai.current = null; } };
    // A source fetch changes saved metadata, not the current editor.
  }, [scope]);
  useEffect(() => {
    if (!bundled || !docPath) return;
    let active = true; const at = scope; setDocument(null);
    void howto.document(docPath).then((d) => { if (active && current.current === at) setDocument(d); }).catch((e) => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [docPath, scope, bundled]);
  useEffect(() => {
    if (tab !== "edit") return;
    let active = true;
    void api.providersList().then((all) => { if (active) { const p = aiProviders(all, demo); setProviders(p); setProvider(""); setModel(""); } }).catch((e) => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [tab, demo]);
  useEffect(() => { setFacts([]); }, [nodes.join(","), cluster?.nodes.map((n) => `${n.id}@${n.host}`).join(",")]);
  const pick = async (next: string) => {
    if (next === id || busy) return;
    if (dirty && !await confirmAction({ title: "Leave unsaved changes?", body: "Your saved editions are kept. The unsaved draft and progress in this guide will be discarded.", confirmLabel: "Discard and continue" })) return;
    setId(next); setTab("guide");
  };
  // A Help link can retarget an already open window; retain its unsaved-work guard.
  useEffect(() => { if (params.guide) void pick(params.guide); }, [params.guide, params.nav]);
  const save = async (edition: Draft | null = null, favorite = local?.favorite ?? false) => {
    if (!local) return; const at = scope; setBusy("save"); setError("");
    try {
      const v = await howto.save(demo, { id, expected: local.version, favorite, notes, completed, edition });
      if (current.current !== at) return;
      accept(v); if (edition) { setDraft(null); setMessage("Local edition saved. The bundled original and previous editions are kept."); } else setMessage("Progress saved on this Mac.");
      await refreshSaved(at);
    } catch (e) { if (current.current === at) setError(String(e)); }
    finally { if (current.current === at) setBusy(""); }
  };
  const beginDraft = () => {
    if (!reference) return;
    setDraft({ text: base, author: "Owner", sourceRevision: referenceRevision, sourceSha256: reference.sha256 });
    setTargetPart(base.length > AI_LIMIT ? "0" : "all"); setReferencePart(reference.text.length > AI_LIMIT ? "0" : "all"); setTab("edit");
  };
  const reloadLocal = async () => {
    const at = scope;
    if (dirty && !await confirmAction({ title: "Reload saved copy?", body: "Keep the open edition draft, but replace unsaved notes and checklist selections with the saved copy. Compare your draft with that copy before saving again.", confirmLabel: "Reload saved copy" })) return;
    if (current.current !== at) return;
    setBusy("reload"); setError("");
    try { const v = await howto.read(demo, id); if (current.current === at) { accept(v); setMessage("Saved copy reloaded. Any open edition draft is still yours to review."); } }
    catch (e) { if (current.current === at) setError(String(e)); }
    finally { if (current.current === at) setBusy(""); }
  };
  const runAI = async () => {
    if (!draft || !reference || !provider || !model) return;
    const p = providers.find((p) => p.id === provider); if (!p) return;
    const at = scope; const original = draft.text;
    const target = targetPart === "all" ? null : sections(original)[Number(targetPart)];
    const source = referencePart === "all" ? reference.text : sections(reference.text)[Number(referencePart)]?.text ?? "";
    let prompt: string;
    try { prompt = aiPrompt(target?.text ?? original, source, instruction, g, referenceRevision); } catch (e) { setError(String(e)); return; }
    if (!await confirmAction({ title: "Send these sections to AI?", body: `Send the selected guide text, NVIDIA reference section and your instruction to ${p.name} (${model}). ${p.kind === "demo" ? "This is a simulated reply." : `Provider address: ${p.baseUrl}. Provider usage may have a cost.`} Notes, credentials and node facts are not included. The reply is a draft; it cannot run commands or save itself.`, confirmLabel: "Create draft" })) return;
    if (current.current !== at) return;
    const request = { id: `howto-${crypto.randomUUID()}`, canceled: false }; ai.current = request;
    setBusy("ai"); setError(""); setStream("");
    try {
      const reply = p.kind === "demo"
        ? `${target?.text ?? original}\n\n> Demo revision: verify the selected endpoint before starting another workload. This simulated draft has not checked a real model or contacted NVIDIA.\n`
        : await draftWithAI(request.id, provider, model, prompt, (s) => { if (current.current === at) setStream(s); }, () => request.canceled || current.current !== at);
      if (current.current !== at || request.canceled) return;
      setDraft({ text: target ? replaceSection(original, target, reply) : reply, author: `AI draft reviewed by owner: ${p.name} / ${model}`, sourceRevision: referenceRevision, sourceSha256: reference.sha256 });
      setCompare(true); setMessage("AI draft ready. Compare it with your saved text, edit it, then choose Save local edition.");
    } catch (e) { if (current.current === at) setError(String(e)); }
    finally { if (current.current === at) { setBusy(""); setStream(""); } if (ai.current === request) ai.current = null; }
  };
  const checkSource = async () => {
    const at = scope; setBusy("source"); setError(""); setTab("updates");
    try { const c = await howto.check(demo); if (current.current === at) { setCheck(c); setMessage("Source comparison saved. Local editions and executable actions are unchanged."); } }
    catch (e) { if (current.current === at) setError(String(e)); }
    finally { if (current.current === at) setBusy(""); }
  };
  const fetchSource = async (fetchId: string) => {
    if (!check) return; const at = scope; setBusy("fetch"); setError("");
    try {
      const v = await howto.fetch(demo, fetchId, check.revision);
      if (current.current !== at) return;
      await refreshSaved(at);
      if (fetchId === id) setLocal(v); // Keep unsaved notes and draft text intact.
      setMessage("NVIDIA source cached for comparison. Save a reviewed local edition when you are ready.");
      if (fetchId !== id && !dirty) { setId(fetchId); setTab("source"); } else if (fetchId === id) setTab("source");
    } catch (e) { if (current.current === at) setError(String(e)); }
    finally { if (current.current === at) setBusy(""); }
  };
  const checkNodes = async () => {
    const at = scope; const chosen = nodes.join(","); setBusy("nodes"); setFacts([]); setError("");
    try { const f = await invoke<NodeCheck[]>("howto_check_nodes", { demo, id, nodes }); if (current.current === at && chosen === selectedNodes.current) setFacts(f); }
    catch (e) { if (current.current === at) setError(String(e)); }
    finally { if (current.current === at) setBusy(""); }
  };
  const prepare = async () => {
    const at = scope; const chosen = nodes.join(","); const size = Number(estimate);
    if (!Number.isFinite(size) || size <= 0 || size > 1024) { setError("Enter an unpacked size estimate greater than zero and up to 1024 GiB."); return; }
    if (!await requestAdmin("prepare the reviewed playbook image") || current.current !== at) return;
    const names = nodes.map((id) => cluster?.nodes.find((n) => n.id === id)?.name ?? id).join(", ");
    if (!await confirmAction({ title: "Prepare this container image?", body: `Download ${image} on ${names}, allowing for about ${size} GiB per node. This can use substantial bandwidth and disk space. Tags may change upstream; Jobs records the downloaded image identity. The job does not run a container, install host packages or start the playbook. Cancel it in Models › Jobs; remove an unused image only after checking that nothing needs it.`, confirmLabel: "Prepare image" })) return;
    if (current.current !== at || chosen !== selectedNodes.current) return;
    setBusy("image"); setError("");
    try { await invoke("howto_prepare_image", { demo, id, image, nodes, estimatedBytes: Math.ceil(size * 1024 ** 3) }); if (current.current === at) { setMessage("Image jobs started. Follow progress and any partial-start errors in Models › Jobs."); open("models", { tab: "jobs" }); } }
    catch (e) { if (current.current === at) setError(String(e)); }
    finally { if (current.current === at) setBusy(""); }
  };
  const visible = guides.filter((g) => matches(g, query) && (category === "all" || g.category === category) && (filter === "all" || filter === "spark" && g.spark || filter === "current" && g.published || filter === "reference" && !g.spark || filter === "favorites" && saved[g.id]?.favorite));
  const selectedProvider = providers.find((p) => p.id === provider);
  const changes = check?.changes.filter((c) => c.status !== "unchanged") ?? [];

  return <div className="howto-window">
    <div className="toolbar howto-top"><BookOpen size={18} /><strong>How To</strong><span className="muted">Skills for your Spark cluster</span><span className="spacer" /><button disabled={!!busy} onClick={() => void reloadLocal()}>Reload saved copy</button><button disabled={!!busy} onClick={() => void checkSource()}>Check NVIDIA for updates</button><HelpTip topic="ops.howto.library" /></div>
    <div className="howto-layout">
      <aside className="howto-library" aria-label="Playbook library">
        <label className="howto-search"><Search size={15} /><input aria-label="Search playbooks" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search a task or model…" /></label>
        <select aria-label="Show playbooks" value={filter} onChange={(e) => setFilter(e.target.value)}><option value="spark">Current Spark guides</option><option value="current">All current guides</option><option value="all">Entire library</option><option value="reference">Other hardware and references</option><option value="favorites">Favorites</option></select>
        <select aria-label="Playbook category" value={category} onChange={(e) => setCategory(e.target.value)}><option value="all">Every topic</option>{[...new Set(guides.map((g) => g.category))].sort().map((c) => <option key={c}>{c}</option>)}</select>
        <small>{visible.length} guides · {LIBRARY.guides.length} bundled</small>
        <div className="howto-results">{visible.map((item) => <button key={item.id} className={item.id === id ? "selected" : ""} aria-pressed={item.id === id} disabled={!!busy} onClick={() => void pick(item.id)}><span>{saved[item.id]?.favorite && <Star size={12} aria-label="Favorite" />}{item.title}</span><small>{item.category} · {item.example ? "Example" : item.spark ? "Spark" : item.platforms.join(", ")}</small>{!!saved[item.id]?.editions && <small>Local edition saved</small>}</button>)}{!visible.length && <p>No guides match these filters.</p>}</div>
        <small>Bundled source: {LIBRARY.reviewed}<br />{LIBRARY.revision.slice(0, 12)}<br />Reading works offline.</small>
      </aside>
      <main className="howto-detail">
        <header className="howto-heading"><div><small>{g.category} · {g.published ? "Currently listed by NVIDIA" : "Reference / locally fetched"}</small><h2>{g.title}</h2><p>{g.purpose}</p></div><button aria-label={local?.favorite ? "Remove favorite" : "Add favorite"} disabled={!local || !!busy} onClick={() => void save(null, !local?.favorite)}><Star size={16} fill={local?.favorite ? "currentColor" : "none"} /></button></header>
        {demo && <Notice tone="info">Demo: node checks, image jobs, source updates and AI replies are simulated. Saved Demo notes and editions stay separate from Live.</Notice>}
        {error && <Notice tone="warn"><span role="alert">{error}</span></Notice>}
        {message && <p className="howto-message" role="status">{message}</p>}
        {busy && <p role="status">{busy === "ai" ? "AI is drafting…" : "Working…"}{busy === "ai" && <button onClick={() => { if (ai.current) { ai.current.canceled = true; void api.chatStop(ai.current.id).catch(() => {}); } }}>Cancel AI</button>}</p>}
        <div className="howto-tabs" role="tablist" aria-label="Guide views">{[["guide", "Walkthrough"], ["source", "NVIDIA source"], ["edit", "My edition"], ["updates", "Source updates"]].map(([key, label]) => <button key={key} role="tab" aria-selected={tab === key} onClick={() => setTab(key)}>{label}</button>)}</div>
        <div className="howto-content">
          {tab === "guide" && <>
            <Notice tone={g.spark ? "info" : "warn"}>{g.attention}</Notice>
            {local?.editions.length ? <><p><strong>Your local edition</strong> · {local.editions.at(-1)?.author}</p><Markdown text={base} revision={local.editions.at(-1)?.sourceRevision} path={g.readme} /></> : <>
              <h3>Before you start</h3><p>Read the hardware table and prerequisites in <button className="howto-inline" onClick={() => setTab("source")}>NVIDIA source</button>. Commands and supporting files stay linked to the reviewed version. Installer, model license, account and hardware decisions are yours.</p>
              <h3>Steps</h3><ol className="howto-steps">{g.steps.map((step, i) => <li key={i}><label><input type="checkbox" disabled={!local || !!busy} checked={completed.includes(i)} onChange={(e) => setCompleted(e.target.checked ? [...completed, i].sort((a, b) => a - b) : completed.filter((j) => j !== i))} /><span>{step}</span></label></li>)}</ol>
              <h3>What you should see</h3><p>{g.success}</p><h3>If something goes wrong</h3><p>Stop at the failed step. Check the platform, required versions, model access, free memory and disk space against the source. Its troubleshooting section explains workload-specific failures. Keep datasets, checkpoints and existing workloads while diagnosing the problem.</p>
            </>}
            <section className="howto-actions" aria-label="SparkBox actions"><h3>Let SparkBox help <HelpTip topic="ops.howto.actions" /></h3>
              <p>These controls use SparkBox's reviewed tools. Opening a tool does not start its work. Source text and AI drafts cannot add commands to these controls.</p>
              {g.spark ? <>
                <div className="howto-node-picks">{cluster?.nodes.map((n) => <label key={n.id}><input type="checkbox" checked={nodes.includes(n.id)} disabled={!!busy} onChange={(e) => setNodes(e.target.checked ? [...nodes, n.id] : nodes.filter((id) => id !== n.id))} />{n.name}</label>)}</div>
                <button disabled={!!busy || !nodes.length} onClick={() => void checkNodes()}>Check selected nodes</button><small>Reads Docker, Python, available disk, configured image and fabric facts. It does not launch a GPU test or prove the whole playbook works.</small>
                {!!facts.length && <div className="howto-facts">{facts.map((f) => <section key={f.node}><strong>{cluster?.nodes.find((n) => n.id === f.node)?.name ?? f.node}</strong>{f.error ? <p>Check incomplete: {f.error}</p> : <p>{demo ? "Simulated" : "Read now"} · Docker {f.docker ? "available" : "missing"} · Python {f.python || "unknown"} · {fmtBytes(f.freeBytes)} free at {f.rootPath || "model storage"} · {f.fabric.length} fabric interfaces. GPU execution is not tested.</p>}</section>)}</div>}
                {!!g.images.length && <div className="howto-image"><label>Reviewed image <select aria-label="Reviewed image" value={image} disabled={!!busy} onChange={(e) => setImage(e.target.value)}>{g.images.map((s) => <option key={s}>{s}</option>)}</select></label><label>Unpacked size estimate (GiB) <input aria-label="Unpacked size estimate (GiB)" type="number" min="1" max="1024" value={estimate} disabled={!!busy} onChange={(e) => setEstimate(e.target.value)} /></label><button disabled={!!busy || !nodes.length} onClick={() => void prepare()}>Prepare image…</button><small>Downloads only. Review the size for this image before continuing. Jobs checks space and records its image identity; no workload is launched.</small></div>}
                <div className="howto-tool-links">{g.actions.filter((a) => ACTIONS[a]).map((a) => <button key={a} onClick={() => open(a, ACTIONS[a].params)}>{ACTIONS[a].title}</button>)}</div>
              </> : <p>This guide has no Spark execution actions. Check its platform and follow the appropriate current Spark guide.</p>}
            </section>
            <section><h3>My progress</h3>{!!local?.editions.length && <details><summary>Original walkthrough checklist</summary><ol className="howto-steps">{g.steps.map((step, i) => <li key={i}><label><input type="checkbox" disabled={!!busy} checked={completed.includes(i)} onChange={(e) => setCompleted(e.target.checked ? [...completed, i].sort((a, b) => a - b) : completed.filter((j) => j !== i))} /><span>{step}</span></label></li>)}</ol></details>}<p>Checklist selections record what you have reviewed; they are not test results.</p><label>Notes for this guide<textarea aria-label="Guide notes" maxLength={20_000} value={notes} disabled={!local || !!busy} onChange={(e) => setNotes(e.target.value)} placeholder="Record versions, outcomes and what to try next. Keep secrets out of notes." /></label><button disabled={!local || !!busy} onClick={() => void save()}>Save progress</button></section>
            <details><summary>Full instruction outline ({g.sourceSteps.length} steps)</summary><ol>{g.sourceSteps.map((s, i) => <li key={i}>{s}</li>)}</ol><button onClick={() => setTab("source")}>Read detailed NVIDIA instructions</button></details>
          </>}
          {tab === "source" && <>
            <p>The original text is reference material. Copying a command does not validate it for your node. Supporting images, scripts and notebooks open at the pinned GitHub revision and need internet access.</p>
            <p><a href={sourceUrl(referenceRevision, g.readme)} onClick={onMarkdownClick}>Open pinned source on GitHub</a> · Apache 2.0 and third-party notices <button onClick={() => setDocPath("LICENSE")}>License</button> <button onClick={() => setDocPath("LICENSE-3rd-party")}>Third-party notices</button></p>
            {bundled && <label>Bundled document <select value={docPath} onChange={(e) => setDocPath(e.target.value)}>{[...g.documents, "LICENSE", "LICENSE-3rd-party"].map((p) => <option key={p}>{p}</option>)}</select></label>}
            {latestSource && <details open><summary>Fetched source · {latestSource.revision.slice(0, 12)} · cached {new Date(latestSource.atMs).toLocaleString()}</summary><p>It has not replaced your local edition. Compare it with the bundled document below.</p><DocumentReader document={latestSource.document} revision={latestSource.revision} /></details>}
            {document && <details open={!latestSource}><summary>Bundled original · {LIBRARY.revision.slice(0, 12)} · SHA-256 {document.sha256.slice(0, 16)}</summary><DocumentReader document={document} revision={LIBRARY.revision} /></details>}
            <button disabled={!local || !reference || !!busy || !!draft} onClick={beginDraft}>Create local edition…</button>
          </>}
          {tab === "edit" && <>
            <Notice tone="info">Local editions change instructions only. The original, history and executable actions stay separate. Review AI output for accuracy before saving.</Notice>
            {!draft ? <><p>{local?.editions.length ?? 0} saved editions. Your current text is shown below.</p><button disabled={!local || !reference || !!busy} onClick={beginDraft}>Edit local edition</button><Markdown text={base} revision={local?.editions.at(-1)?.sourceRevision} path={g.readme} /></> : <>
              <div className="howto-editor-toolbar"><button disabled={!!busy || !draft.text.trim() || draft.text.length > 256 * 1024} onClick={() => void save(draft)}>Save local edition</button><button disabled={!!busy} onClick={() => setCompare(!compare)}>{compare ? "Hide comparison" : "Compare with saved text"}</button><button disabled={!!busy} onClick={async () => { if (await confirmAction({ title: "Discard this draft?", body: "Saved editions are kept. Only the unsaved draft will be removed.", confirmLabel: "Discard draft" })) setDraft(null); }}>Discard draft</button></div>
              {compare && <div className="howto-compare"><section><h4>Saved text</h4><pre>{base}</pre></section><section><h4>Proposed text</h4><pre>{draft.text}</pre></section></div>}
              <label>Local edition text<textarea className="howto-editor" aria-label="Local edition text" value={draft.text} maxLength={256 * 1024} disabled={!!busy} onChange={(e) => setDraft({ ...draft, text: e.target.value })} /></label>
              <details open className="howto-ai"><summary>Ask AI to revise a section <HelpTip topic="ops.howto.ai" /></summary>
                <p>Choose the provider explicitly. Only the selected text and instruction will be sent. Large documents must be edited one section at a time.</p>
                <label>AI provider <select aria-label="AI provider" disabled={!!busy} value={provider} onChange={(e) => { setProvider(e.target.value); setModel(""); }}><option value="">Choose a provider…</option>{providers.map((p) => <option key={p.id} value={p.id}>{p.name}{p.kind === "demo" ? " (simulated)" : ` — ${p.baseUrl}`}</option>)}</select></label>
                <label>Model <select aria-label="AI model" disabled={!!busy || !selectedProvider} value={model} onChange={(e) => setModel(e.target.value)}><option value="">Choose a model…</option>{selectedProvider?.models.map((m) => <option key={m}>{m}</option>)}</select></label>
                {!providers.length && <p>No available provider. <button onClick={() => open("providers")}>Open Providers</button> to configure one; Demo uses only simulated models.</p>}
                <label>Section to revise <select aria-label="Section to revise" disabled={!!busy} value={targetPart} onChange={(e) => setTargetPart(e.target.value)}><OptionSections text={editing} /></select></label>
                <label>NVIDIA reference section <select aria-label="NVIDIA reference section" disabled={!!busy} value={referencePart} onChange={(e) => setReferencePart(e.target.value)}><OptionSections text={reference?.text ?? ""} /></select></label>
                <label>What should AI improve?<textarea aria-label="AI instruction" value={instruction} maxLength={2000} disabled={!!busy} onChange={(e) => setInstruction(e.target.value)} /></label><button disabled={!!busy || !provider || !model || !reference} onClick={() => void runAI()}>Review AI request…</button>
                {stream && <pre className="howto-stream" aria-label="AI draft in progress">{stream}</pre>}
              </details>
              <details><summary>Preview draft</summary><Markdown text={draft.text} revision={draft.sourceRevision} path={g.readme} /></details>
            </>}
            <h3>Saved history</h3>{local?.editions.length ? [...local.editions].reverse().map((e, i) => <details key={`${e.atMs}:${i}`}><summary>{new Date(e.atMs).toLocaleString()} · {e.author}</summary><p>Source {e.sourceRevision.slice(0, 12)} · Text SHA-256 {e.sha256.slice(0, 16)}</p><button disabled={!!busy || !!draft} onClick={() => { setDraft({ text: e.text, author: "Owner restored a previous edition", sourceRevision: e.sourceRevision, sourceSha256: e.sourceSha256 }); setCompare(true); }}>Restore as a new draft</button><Markdown text={e.text} revision={e.sourceRevision} path={g.readme} /></details>) : <p>No local editions saved yet. The bundled walkthrough is always available.</p>}
          </>}
          {tab === "updates" && <>
            <h3>Compare with NVIDIA</h3><p>Checks run only when you choose Check NVIDIA for updates. SparkBox reads the public repository, pins the revision and compares it with the bundle. Code or asset changes also count as a changed guide. Fetching downloads text only; local editions and reviewed actions stay unchanged.</p>
            {check ? <><p>{check.demo ? "Simulated check" : "Last successful check"}: {new Date(check.checkedAtMs).toLocaleString()} · revision {check.revision.slice(0, 12)} · {check.total} folders</p><p>{changes.length} added, changed or removed guides compared with the bundled revision.</p><div className="howto-changes">{changes.map((c) => <section key={c.id}><strong>{c.status} · {guides.find((g) => g.id === c.id)?.title ?? c.id}</strong><small>{c.published ? "Linked in the current NVIDIA index" : "Reference / not linked in the index"}</small>{c.status !== "removed" && <button disabled={!!busy} onClick={() => void fetchSource(c.id)}>Fetch source for review</button>}</section>)}</div>{!changes.length && <p>The guide folders and listing match the bundled revision.</p>}</> : <p>No source check has been made for this data source. All bundled guides are available offline.</p>}
            <h3>Keep your local work</h3><p>Fetching a source keeps your notes and editions. New guides appear as references without execution actions. Review the fetched text in NVIDIA source, create a local edition, and ask your chosen AI to revise only the sections you select.</p>
          </>}
        </div>
      </main>
    </div>
  </div>;
}
