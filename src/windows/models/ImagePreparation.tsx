import { useState } from "react";
import { FormGroup, FormRow, Notice } from "../../components/ui";
import { requestAdmin } from "../../lib/admin";
import { modelsApi } from "../../lib/modelsApi";
import { confirmAction } from "../../lib/session";
import { useCluster } from "../../lib/store";
import { openWindow } from "../../wm/WindowManager";

/** Node-owned jobs survive disconnects. This form authorizes a concrete image and node set. */
export function ImagePreparation() {
  const cluster = useCluster();
  const [image, setImage] = useState("");
  const [source, setSource] = useState("");
  const [nodes, setNodes] = useState<string[]>([]);
  const [size, setSize] = useState("50");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const targets = nodes.filter((n) => n !== source);
  const names = targets.map((id) => cluster?.nodes.find((n) => n.id === id)?.name ?? id).join(", ");
  const begin = async () => {
    if (busy || !(await requestAdmin("model_image_jobs"))) return;
    if (!(await confirmAction({ title: `${source ? "Distribute" : "Pull"} this engine image?`,
      body: `${image.trim()} on ${names}. ${source ? "Export linux/arm64, copy over the fabric, check SHA-256, load and verify the local image ID, then set the requested tag. A tag already there will point to this image." : "Download a public linux/arm64 image through each node's Docker Engine. This pull uses anonymous registry access."} Jobs continue if the Mac disconnects. No containers are restarted.`, confirmLabel: source ? "Distribute image" : "Pull image" }))) return;
    setBusy(true); setError("");
    try {
      await modelsApi.imageJobs(image.trim(), targets, Math.ceil(Number(size) * 1024 ** 3), source);
      openWindow("models", { tab: "jobs" });
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <FormGroup title="Prepare an engine image">
    <FormRow label="Image tag"><input className="wide mono" aria-label="Image tag" value={image} placeholder="registry/repository:tag" onChange={(e) => setImage(e.target.value)} /></FormRow>
    <FormRow label="Source"><select aria-label="Image source" value={source} onChange={(e) => setSource(e.target.value)}>
      <option value="">Pull from its registry on each target</option>
      {cluster?.nodes.map((n) => <option key={n.id} value={n.id}>Copy from {n.name} over the fabric</option>)}
    </select></FormRow>
    <FormRow label="Targets"><span className="ai-inline">{cluster?.nodes.filter((n) => n.id !== source).map((n) => <label key={n.id}>
      <input type="checkbox" checked={nodes.includes(n.id)} onChange={(e) => setNodes(e.target.checked ? [...nodes, n.id] : nodes.filter((v) => v !== n.id))} /> {n.name}
    </label>)}</span></FormRow>
    {!source && <FormRow label="Unpacked size estimate (GiB)" hint="Disk check reserves twice this estimate plus 20 GiB; a registry pull can exceed the estimate."><input className="narrow" type="number" min={1} max={1024} aria-label="Image size estimate" value={size} onChange={(e) => setSize(e.target.value)} /></FormRow>}
    {source && <p className="muted">Copies require matching Docker image stores. Each target checks space for twice the image or archive size plus 20 GiB. Temporary archives are removed when jobs finish or are canceled.</p>}
    {cluster?.demo && <Notice tone="info">Demo simulates job progress; it does not add an installed image.</Notice>}
    {error && <Notice tone="error">{error}</Notice>}
    <button className="btn" disabled={busy || !image.trim() || !targets.length || !Number.isFinite(Number(size)) || Number(size) <= 0 || Number(size) > 1024} onClick={() => void begin()}>{busy ? "Starting jobs…" : "Prepare image…"}</button>
  </FormGroup>;
}
