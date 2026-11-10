import { useState } from "react";
import { FormRow, Notice } from "../../components/ui";
import { modelsApi, type RecipeImport as Draft } from "../../lib/modelsApi";
import { confirmAction } from "../../lib/session";

export function RecipeImport({ onUse, nodes }: { onUse: (draft: Draft) => void; nodes: { id: string; name: string }[] }) {
  const [node, setNode] = useState("");
  const [folder, setFolder] = useState("~/glm53");
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const read = async () => {
    if (!await confirmAction({ title: "Read this launch as a recipe?", body: `Read config.env and inner.sh in ${folder} on ${nodes.find((n) => n.id === node)?.name ?? node}. Parse literal settings without sourcing or executing either file. Secret options are omitted. Review the draft before using it; importing does not load a model.`, confirmLabel: "Read and preview" })) return;
    setBusy(true); setError(""); setDraft(null);
    try { setDraft(await modelsApi.importRecipe(node, folder)); } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <details><summary>Import a launch script as a recipe…</summary>
    <FormRow label="Script node"><select aria-label="Script node" value={node} onChange={(e) => { setNode(e.target.value); setDraft(null); }}><option value="">Choose a node…</option>{nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></FormRow>
    <FormRow label="Script folder"><input className="wide mono" aria-label="Script folder" value={folder} onChange={(e) => { setFolder(e.target.value); setDraft(null); }} /></FormRow>
    <button className="btn small" disabled={busy || !node || !folder.trim()} onClick={() => void read()}>{busy ? "Reading…" : "Read and preview…"}</button>
    {error && <Notice tone="warn">{error}</Notice>}
    {draft && <><p>{draft.modelPath} · TP {draft.tensorParallel} × PP {draft.pipelineParallel}. Compatibility remains unverified.</p>
      {draft.warnings.map((w) => <p className="muted" key={w}>{w}</p>)}<pre className="cmd">{JSON.stringify(draft.recipe, null, 2)}</pre>
      <button className="btn small" onClick={() => onUse(draft)}>Use in this Load form</button><p className="muted">Replaces the recipe, image, split, limits and parsers in this form. The normal Load confirmation and safeguards still apply.</p></>}
  </details>;
}
