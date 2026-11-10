import "../rdp.css";
import { useEffect, useRef, useState } from "react";
import { invoke } from "../lib/api";
import { requestAdmin } from "../lib/admin";
import { confirmAction } from "../lib/session";
import { useCluster } from "../lib/store";
import { HelpTip, Notice } from "../components/ui";

export interface RdpCheck { label: string; ok: boolean | null; detail: string }
export interface RdpView {
  node: string; address: string; demo: boolean; mac: RdpCheck;
  report: { port: number | null; user: string; profile: string; fingerprint: string | null; checks: RdpCheck[];
    changes: string[]; blockers: string[]; digest: string; backup: string | null };
}
export function rdpStatus(view: RdpView | undefined): string {
  if (!view) return "Not checked";
  if (view.report.checks.some((c) => c.ok === false) || view.mac.ok === false) return "Needs attention";
  if (view.mac.ok !== true || view.report.blockers.length) return "Check incomplete";
  return "Encrypted connection checked";
}
export function RdpWindow() {
  const cluster = useCluster();
  const [profile, setProfile] = useState("keep");
  const [reports, setReports] = useState<Record<string, RdpView>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [checked, setChecked] = useState<Record<string, number>>({});
  const source = `${cluster?.demo}:${profile}:${cluster?.nodes.map((n) => `${n.id}@${n.host}`).join(",")}`;
  const current = useRef(source); current.current = source;
  useEffect(() => { setReports({}); setErrors({}); setChecked({}); }, [source]);
  const accept = (id: string, v: RdpView, at: string) => {
    if (at !== current.current || v.node !== id || v.demo !== !!cluster?.demo) return;
    setReports((r) => ({ ...r, [id]: v })); setChecked((r) => ({ ...r, [id]: Date.now() }));
  };
  const check = async (nodes: string[]) => {
    const at = source;
    if (!await requestAdmin("check RDP configuration and certificate access") || current.current !== at) return;
    setBusy("check");
    try {
      for (const node of nodes) {
        if (current.current !== at) break;
        setErrors((e) => ({ ...e, [node]: "" }));
        setReports((r) => { const next = { ...r }; delete next[node]; return next; });
        try { accept(node, await invoke<RdpView>("rdp_check", { node, profile }), at); }
        catch (e) { if (current.current === at) setErrors((r) => ({ ...r, [node]: String(e) })); }
      }
    } finally { setBusy(null); }
  };
  const repair = async (node: string, name: string) => {
    const v = reports[node]; const at = source;
    if (!v || v.report.blockers.length || !v.report.changes.length) return;
    if (Date.now() - checked[node] > 120000) { setErrors((e) => ({ ...e, [node]: "Check RDP again before reviewing this repair." })); return; }
    if (!await requestAdmin("repair RDP on this node") || current.current !== at) return;
    if (!await confirmAction({ title: `Repair RDP on ${name}?`, danger: true, requireTyping: name, confirmLabel: "Repair RDP",
      body: `Configure ${v.report.profile === "gnome" ? "Ubuntu GNOME" : "XFCE"} at ${v.address} and restart its RDP services. Log out of every RDP desktop first. A private backup keeps the old configuration; failed verification restores unchanged files. This enables remote desktop access on the shown node port. Passwords, firewall rules, packages, GPU drivers and GNOME Remote Desktop stay unchanged.`,
      details: <ul>{v.report.changes.map((c) => <li key={c}>{c}</li>)}</ul> })) return;
    if (current.current !== at) return;
    setBusy(node); setErrors((e) => ({ ...e, [node]: "" }));
    // Discard the old result immediately: an interrupted repair cannot leave a green reading.
    setReports((r) => { const next = { ...r }; delete next[node]; return next; });
    try { accept(node, await invoke<RdpView>("rdp_repair", { node, profile, digest: v.report.digest }), at); }
    catch (e) { if (current.current === at) setErrors((r) => ({ ...r, [node]: String(e) })); }
    finally { setBusy(null); }
  };
  return <div className="rdp-window window-scroll">
    <div className="toolbar" role="toolbar" aria-label="RDP">
      <button disabled={!!busy || !cluster?.nodes.length} onClick={() => void check(cluster?.nodes.map((n) => n.id) ?? [])}>Check all nodes</button>
      <label>Desktop for repair <select aria-label="Desktop for repair" disabled={!!busy} value={profile} onChange={(e) => setProfile(e.target.value)}>
        <option value="keep">Keep existing desktop</option><option value="gnome">Ubuntu GNOME</option><option value="xfce">XFCE</option>
      </select></label><HelpTip topic="ops.rdp.check" />
    </div>
    <div className="pad">
      <p>Check remote desktop access to your nodes. Admin mode lets SparkBox inspect protected RDP settings and certificate access.</p>
      {cluster?.demo && <Notice tone="info">Demo simulates checks and repairs. No real desktop connection or node change is tested.</Notice>}
      <Notice tone="info">Desktop login is not tested. After the connection checks pass, open the shown address in your RDP client, select Xorg and log in with your Linux password. SparkBox’s built-in VPN does not provide a route for other apps.</Notice>
      {busy && <p role="status">{busy === "check" ? "Checking RDP…" : "Repairing RDP and checking the result…"}</p>}
      {cluster?.nodes.map((n) => {
        const v = reports[n.id]; const r = v?.report;
        return <section key={n.id} className="rdp-node" aria-label={`RDP on ${n.name}`}>
          <div className="rdp-heading"><strong>{n.name}</strong><span>{errors[n.id] ? "Check incomplete" : rdpStatus(v)}</span>
            <button disabled={!!busy} onClick={() => void check([n.id])}>Check {n.name}</button>
            {v && <button disabled={!!busy || !!r?.blockers.length || !r?.changes.length} onClick={() => void repair(n.id, n.name)}>Review repair…</button>}
          </div>
          {errors[n.id] && <Notice tone="warn">{errors[n.id]}</Notice>}
          {v && r && <>
            <p><strong>PC name:</strong> <code>{v.address}</code> · <strong>Username:</strong> {r.user || "Unknown"} · <strong>Desktop:</strong> {r.profile === "gnome" ? "Ubuntu GNOME" : r.profile === "xfce" ? "XFCE" : "Unknown"}</p>
            <dl className="rdp-checks">{[...r.checks, v.mac].map((c) => <div key={c.label}><dt>{c.ok === true ? "Passed" : c.ok === false ? "Needs attention" : "Not verified"} — {c.label}</dt><dd>{c.detail}</dd></div>)}</dl>
            {r.blockers.length > 0 && <Notice tone="warn"><ul>{r.blockers.map((b) => <li key={b}>{b}</li>)}</ul></Notice>}
            {r.changes.length > 0 && <details><summary>Proposed repair ({r.changes.length} changes)</summary><ul>{r.changes.map((c) => <li key={c}>{c}</li>)}</ul></details>}
            {r.fingerprint && <p className="rdp-fingerprint">Certificate SHA-256: <code>{r.fingerprint.match(/.{1,2}/g)?.join(":").toUpperCase()}</code></p>}
            {r.backup && <Notice tone="info">Configuration backup: <code>{r.backup}</code>. It contains private certificate material; keep it on the node. See Help before restoring it.</Notice>}
          </>}
        </section>;
      })}
    </div>
  </div>;
}
