import { useState } from "react";
import { invoke } from "../../lib/api";
import { FormGroup, FormRow, Notice } from "../../components/ui";
import { requestAdmin } from "../../lib/admin";
import { confirmAction } from "../../lib/session";
import { useCluster } from "../../lib/store";

type Preview = { before: string; after: string; sha256: string };
type Reviewed = Preview & { node: string; names: string[]; demo: boolean | undefined };
export function EarlyoomPreparation() {
  const cluster = useCluster();
  const [node, setNode] = useState("");
  const [names, setNames] = useState("");
  const [preview, setPreview] = useState<Reviewed | null>(null);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const args = { node, names: names.split(/[\s,]+/).filter(Boolean) };
  const reviewed = preview && preview.node === node && preview.demo === cluster?.demo && JSON.stringify(preview.names) === JSON.stringify(args.names) ? preview : null;
  const read = async () => {
    setBusy(true); setMessage(""); setPreview(null);
    try { setPreview({ ...await invoke<Preview>("earlyoom_preview", args), ...args, demo: cluster?.demo }); }
    catch (e) { setMessage(String(e)); }
    finally { setBusy(false); }
  };
  const apply = async () => {
    if (!reviewed || !(await requestAdmin("earlyoom_apply"))) return;
    if (!await confirmAction({ title: `Save the earlyoom rule on ${cluster?.nodes.find((n) => n.id === reviewed.node)?.name ?? reviewed.node}?`, body: `Save this exact preview for ${reviewed.names.join(", ")} with sudo, retaining a backup, then restart earlyoom. Existing options and its avoid expression are retained. This is a preference about which processes earlyoom kills; memory preflight remains necessary.`, confirmLabel: "Save and restart earlyoom" })) return;
    setBusy(true);
    try { setMessage(await invoke<string>("earlyoom_apply", { node: reviewed.node, names: reviewed.names, sha256: reviewed.sha256 })); setPreview(null); }
    catch (e) { setMessage(String(e)); }
    finally { setBusy(false); }
  };
  return <FormGroup title="earlyoom process preference">
    <FormRow label="Node"><select aria-label="earlyoom node" value={node} onChange={(e) => { setNode(e.target.value); setPreview(null); }}><option value="">Choose a node…</option>{cluster?.nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></FormRow>
    <FormRow label="Recorded comm names" hint="Exact /proc/PID/comm values, truncated by Linux to 15 characters. For example VLLM::EngineCor and VLLM::Worker_TP; verify them from the serving process before applying. Never python3."><input className="wide mono" aria-label="Recorded comm names" value={names} onChange={(e) => { setNames(e.target.value); setPreview(null); }} /></FormRow>
    <button className="btn" disabled={busy || !node || !args.names.length} onClick={() => void read()}>Preview earlyoom edit</button>
    {reviewed && <><p className="muted">{cluster?.demo ? "Demo fixture; nothing will be applied." : "Proposed /etc/default/earlyoom contents:"}</p><pre className="cmd">{reviewed.after}</pre><button className="btn" disabled={busy || cluster?.demo} onClick={() => void apply()}>Save and restart earlyoom…</button></>}
    {message && <Notice tone="info">{message}</Notice>}
  </FormGroup>;
}
