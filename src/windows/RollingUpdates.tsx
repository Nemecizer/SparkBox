import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FormRow, Notice } from "../components/ui";
import { requestAdmin, useAdmin } from "../lib/admin";
import { confirmAction } from "../lib/session";
import { useCluster } from "../lib/store";
import { waitTask } from "../lib/tasks";
import type { PlanEntry } from "../lib/opsApi";

export interface RollingPlan {
  id: string; demo: boolean; instance: string; createdMs: number; phase: string; message: string;
  loads: string[]; paused: string[];
  nodes: { node: string; packages: string[]; versions: PlanEntry[]; restart: boolean; phase: string; child: string | null; admittedMs: number; bootBefore: string | null; kernel: string | null; driver: string | null }[];
}

export function RollingUpdates() {
  const cluster = useCluster(); const admin = useAdmin();
  const [canary, setCanary] = useState(""); const [restart, setRestart] = useState<string[]>([]);
  const [preview, setPreview] = useState<RollingPlan | null>(null); const [saved, setSaved] = useState<RollingPlan | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState("");
  const refresh = () => invoke<RollingPlan | null>("updates_rolling_status").then(setSaved);
  useEffect(() => { void refresh().catch(() => {}); const timer = setInterval(() => void refresh().catch(() => {}), 3000); return () => clearInterval(timer); }, [cluster?.demo]);
  const inspect = async () => {
    if (!cluster) return; setBusy(true); setError(""); setPreview(null);
    try { setPreview(await invoke<RollingPlan>("updates_rolling_preview", { nodes: cluster.nodes.map((n) => n.id), canary: canary || null, restart })); }
    catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const run = async (action: "start" | "resume" | "restore") => {
    const plan = action === "start" ? preview : saved;
    if (!plan || !await requestAdmin("run the reviewed maintenance plan")) return;
    const name = (id: string) => cluster?.nodes.find((n) => n.id === id)?.name ?? id;
    if (!await confirmAction({ title: action === "restore" ? "Restore stopped models and end this plan?" : "Update nodes one by one?",
      body: action === "restore" ? "Restore only the models this plan stopped, using their saved settings and the normal load checks. Remaining package and restart steps will be abandoned. An unloaded model is never relaunched."
        : `Order: ${plan.nodes.map((n) => name(n.node)).join(" → ")}. Install only the reviewed routine package versions; GPU-stack updates remain blocked. ${plan.nodes.some((n) => n.restart) ? `Restart: ${plan.nodes.filter((n) => n.restart).map((n) => name(n.node)).join(", ")}. SparkBox stops affected owned loads, checks each reboot and restores each load after its last selected member. Loads: ${plan.loads.join(", ") || "none"}.` : "No node restarts are selected; routine updates leave models serving."} Stop at the first failure. Cancel in Tasks stops before the next step; an admitted apt job is left to finish.`,
      confirmLabel: action === "restore" ? "Restore and end plan" : action === "resume" ? "Resume reviewed plan" : "Start reviewed plan" })) return;
    setBusy(true); setError("");
    try {
      const id = action === "start" ? await invoke<string>("updates_rolling_start", { plan }) : await invoke<string>("updates_rolling_resume", { restoreOnly: action === "restore" });
      setPreview(null); await refresh(); await waitTask(id); await refresh();
    } catch (e) { setError(String(e)); void refresh().catch(() => {}); } finally { setBusy(false); }
  };
  const active = saved && saved.phase !== "done";
  return <details className="pad"><summary>Update all nodes one by one…</summary>
    <p>Routine packages only. Each node uses the existing guarded installer. A saved plan waits for you to resume it after SparkBox restarts.</p>
    {cluster?.demo && <Notice tone="info">Demo simulates updates and restarts. It does not prove package, GPU or boot compatibility.</Notice>}
    {active ? <><Notice tone={saved.phase === "paused" ? "warn" : "info"}>{saved.message || "Maintenance is running. Follow its steps or cancel in Tasks."}</Notice>
      <p>{saved.nodes.map((n) => `${cluster?.nodes.find((x) => x.id === n.node)?.name ?? n.node}: ${n.phase}`).join(" · ")}</p>
      {saved.phase === "paused" && <><button className="btn small" disabled={busy} onClick={() => void run("resume")}>Resume…</button>{" "}<button className="btn small" disabled={busy} onClick={() => void run("restore")}>Restore stopped models and end plan…</button></>}
    </> : <>
      <FormRow label="Canary"><select value={canary} onChange={(e) => { setCanary(e.target.value); setPreview(null); }}><option value="">cell2, or the node with fewest dependents</option>{cluster?.nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></FormRow>
      <FormRow label="Restart when done"><div className="rolling-restarts">{cluster?.nodes.map((n) => <label className="check" key={n.id}><input type="checkbox" checked={restart.includes(n.id)} onChange={(e) => { setRestart((old) => e.target.checked ? [...old, n.id] : old.filter((x) => x !== n.id)); setPreview(null); }} />{n.name}{n.system.rebootRequired ? " (restart required)" : ""}</label>)}</div></FormRow>
      <button className="btn small" disabled={busy || !cluster?.nodes.length} onClick={() => void inspect()}>{busy ? "Working…" : "Preview routine updates"}</button>
      {preview && <><ol>{preview.nodes.map((n) => <li key={n.node}>{cluster?.nodes.find((x) => x.id === n.node)?.name ?? n.node}: {n.versions.length} package versions{n.restart ? ", then restart" : ", no restart"}<details><summary>Package plan</summary><pre className="cmd">{n.versions.map((p) => `${p.name} ${p.to}`).join("\n") || "No routine packages pending."}</pre></details></li>)}</ol><button className="btn small" disabled={busy} title={!admin ? "Needs Admin" : undefined} onClick={() => void run("start")}>Start reviewed plan…</button></>}
    </>}
    {error && <Notice tone="warn">{error}</Notice>}
  </details>;
}
