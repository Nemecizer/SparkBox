import { useEffect, useRef, useState } from "react";
import { invoke } from "../lib/api";
import { HelpTip, Notice } from "../components/ui";
import { requestAdmin } from "../lib/admin";
import { confirmAction } from "../lib/session";
import { useCluster } from "../lib/store";
import { openTasksWindow, waitTask } from "../lib/tasks";
import type { SimulatedUpgrade } from "../lib/opsApi";

export interface SystemUpdateRecord { node: string; demo: boolean; phase: string; message: string; task: string | null; previous?: SystemUpdateRecord | null; fresh1025?: boolean; completed?: SystemUpdateRecord[] }
export function ProtectedUpdates() {
  const cluster = useCluster();
  const [node, setNode] = useState("cell2");
  const [continuation, setContinuation] = useState(false);
  const [preview, setPreview] = useState<SimulatedUpgrade | null>(null);
  const [saved, setSaved] = useState<SystemUpdateRecord | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState("");
  const source = `${cluster?.demo}:${node}`; const current = useRef(source); current.current = source;
  const refresh = async () => { const at = current.current; const r = await invoke<SystemUpdateRecord | null>("updates_system_status"); if (current.current === at) setSaved(r); };
  useEffect(() => { setPreview(null); setSaved(null); setError(""); void refresh().catch((e) => setError(String(e))); }, [source]); // eslint-disable-line react-hooks/exhaustive-deps
  const name = (id: string) => cluster?.nodes.find((n) => n.id === id)?.name ?? id;
  const canInstall = !!preview?.plan?.length && !preview.refused && preview.node === (continuation ? saved?.node : node);
  const fresh1025 = !continuation && ["cell1", "cell3", "gx10-0000"].includes(preview?.node ?? node);
  const inspect = async (continuing = false) => {
    const at = source;
    if (!await requestAdmin("read the protected boot configuration") || current.current !== at) return;
    setBusy(true); setPreview(null); setContinuation(continuing); setError("");
    try { const p = await invoke<SimulatedUpgrade>("updates_system_preview", { node: continuing ? saved!.node : node }); if (current.current === at) setPreview(p); }
    catch (e) { if (current.current === at) setError(String(e)); } finally { setBusy(false); }
  };
  const install = async () => {
    const p = preview; const at = source;
    if (!p || !canInstall || !p.plan || !await requestAdmin("install the reviewed system updates")) return;
    if (current.current !== at) return;
    if (!await confirmAction({ title: `${continuation ? "Continue to kernel 1025" : "Install system updates"} on ${name(p.node)}?`, requireTyping: name(p.node), danger: true, confirmLabel: "Install reviewed system updates",
      body: fresh1025 ? "Install 16 protected updates and eight new kernel packages at the exact versions shown, directly from kernel 1019 to 1025. Keep the 580 GPU driver and the 1019 recovery kernel. Healthy startup and the reviewed persistent recovery-boot setting are required before installation. This can interrupt node services. Have a current data backup and local keyboard/display access. Restart separately, then verify every check before moving to another node." : continuation ? "Install nine updates and eight new kernel packages at the exact versions shown. Keep the original 1024 checkpoint and both 1019 and 1024 recovery kernels. The 580 GPU driver stays unchanged. This can interrupt node services. Have a current data backup and local keyboard/display access for recovery. Restart separately, then verify the new boot. Other protected updates remain blocked until every check passes." : "Install 13 protected updates and eight new kernel packages at the exact versions shown. NVIDIA's boot fix is required; the 580 GPU driver stays unchanged. Unload models first. This can interrupt node services. Have local keyboard/display access and a current data backup. Keep the previous kernel for GRUB recovery. A restart is a separate action. Other system updates wait until this node passes verification.",
      details: fresh1025 ? <p>This direct path has offline test coverage only. Cell2 passed a different, two-step path after boot-setting and Plymouth repairs. Missing prerequisites stop installation; this review does not repair startup or boot settings.</p> : continuation ? <p>This continues only the saved cell2 update after healthy 1024 startup. It does not authorize updates on the other nodes. A changed package plan or failed health check stops the continuation.</p> : <p>Kernel 7.0.0-1024 has reported issues. The included NVIDIA fix addresses the display-related reboot loop; it does not establish that every hardware issue is resolved. Use one canary first.</p> })) return;
    if (current.current !== at) return;
    setBusy(true); setError("");
    try { const id = await invoke<string>("updates_system_start", { node: p.node, plan: p.plan.map(({ name, to }) => ({ name, to })) }); setPreview(null); await refresh(); openTasksWindow(id); await waitTask(id); await refresh(); }
    catch (e) { if (current.current === at) setError(String(e)); void refresh().catch(() => {}); } finally { setBusy(false); }
  };
  const verify = async () => {
    const at = source;
    if (!await requestAdmin("verify the protected boot configuration") || current.current !== at) return;
    setBusy(true); setPreview(null); setError("");
    try { const r = await invoke<SystemUpdateRecord>("updates_system_verify"); if (current.current === at) setSaved(r); }
    catch (e) { if (current.current === at) setError(String(e)); } finally { setBusy(false); }
  };
  const discard = async () => {
    const at = source;
    if (!await requestAdmin("discard an interrupted system-update review") || current.current !== at) return;
    setBusy(true); setPreview(null); setError("");
    try { const r = await invoke<SystemUpdateRecord>("updates_system_discard_review"); if (current.current === at) setSaved(r); }
    catch (e) { if (current.current === at) setError(String(e)); } finally { setBusy(false); }
  };
  const active = saved && !["verified", "refused"].includes(saved.phase);
  const canContinue = active && saved.node === "cell2" && saved.phase === "installing" && !!saved.task && !saved.previous;
  return <details className="pad"><summary>Protected system updates… <HelpTip topic="ops.updates.system" /></summary>
    <p>Cell1, cell3 and cell4: review 16 upgrades and eight new packages directly from kernel 1019 to 1025. Healthy startup and the reviewed recovery-boot setting are required first. One idle node at a time; no removals, automatic repair or automatic restart. Cell2 retains its separate original and continuation paths. Routine Upgrade keeps its existing protections.</p>
    {cluster?.demo && <Notice tone="info">Demo simulates this plan. No real packages, restart or hardware compatibility are tested.</Notice>}
    {saved && <Notice tone={saved.phase === "verified" ? "info" : "warn"}>{name(saved.node)}: {saved.message}</Notice>}
    {saved?.previous && <p>The original 1024 checkpoint is retained. Both 1019 and 1024 kernels stay available for local recovery. {saved.previous.task && <button className="btn small" onClick={() => openTasksWindow(saved.previous!.task!)}>View original package task</button>}</p>}
    {!!saved?.completed?.length && <details><summary>Completed system updates</summary>{saved.completed.map((r, i) => <p key={i}>{name(r.node)}: {r.message} {r.task && <button className="btn small" onClick={() => openTasksWindow(r.task!)}>View {name(r.node)} package task</button>}{r.previous?.task && <button className="btn small" onClick={() => openTasksWindow(r.previous!.task!)}>View {name(r.node)} original package task</button>}</p>)}</details>}
    {active ? <><p>Follow the package task first. After it succeeds, use Restart… beside this node below, wait for it to reconnect, then check here. A startup warning keeps the next node blocked.</p><button className="btn small" disabled={busy} onClick={() => void verify()}>Check system update</button>{saved.phase === "checking" && !saved.task && <button className="btn small" disabled={busy} onClick={() => void discard()}>Discard interrupted review</button>}{saved.task && <button className="btn small" onClick={() => openTasksWindow(saved.task!)}>View package task</button>}{canContinue && <p><button className="btn small" disabled={busy} onClick={() => void inspect(true)}>Review 1025 continuation</button> <HelpTip topic="ops.updates.continue1025" /></p>}</> : <>
      <label>Node <select aria-label="System update node" disabled={busy} value={node} onChange={(e) => { setNode(e.target.value); setPreview(null); }}>{cluster?.nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></label>{" "}
      <button className="btn small" disabled={busy || !cluster?.nodes.some((n) => n.id === node)} onClick={() => void inspect()}>{busy ? "Checking…" : "Review system updates"}</button>
    </>}
    {preview && <section aria-label="Proposed system updates">
      <p><strong>Proposed updates for {name(preview.node)}</strong> · {preview.plan?.length ?? 0} packages</p>
      <p>{fresh1025 ? "Direct to kernel 1025: 16 upgrades, eight new packages, no removals. Startup and recovery prerequisites must pass before installation." : continuation ? "Continue cell2 to kernel 1025: nine upgrades, eight new packages, no removals." : "Install exactly these versions."} A changed plan, held package or failed safety check refuses the update.</p>
      <div className="protected-update-actions">
        <button className="btn primary" disabled={busy || !canInstall} onClick={() => void install()}>Update {name(preview.node)} with proposed updates…</button>
        <button className="btn small" disabled={busy} onClick={() => setPreview(null)}>Close review</button>
      </div>
      <p>Review the packages below, then use the Update button to confirm installation on {name(preview.node)}. Admin mode and typing the node's name are required. Restart is a separate action.</p>
      {preview.refused && <Notice tone="warn">{preview.refused}</Notice>}
      <ul className="ops-list mono protected-packages" aria-label="Reviewed system packages">{preview.plan?.map((p) => <li key={p.name}>{p.name}: {p.from ? `${p.from} → ` : "new "}{p.to}</li>)}</ul>
      <p>Recovery: retain kernel 7.0.0-1019-nvidia{continuation ? " and kernel 7.0.0-1024-nvidia" : ""}. If the new kernel fails, select the previous kernel in GRUB using local access. SparkBox does not downgrade packages or erase the node.</p>
    </section>}
    {error && <Notice tone="warn">{error}</Notice>}
  </details>;
}
