import { useEffect, useRef, useState } from "react";
import { HelpTip, Notice } from "../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../lib/admin";
import { currentSource, useDataSource } from "../lib/dataSource";
import { discoveryApi, type DiscoveryCandidate, type DiscoveryProposal, type DiscoveryRun, type DiscoveryScope } from "../lib/discovery";
import { asSbError } from "../lib/errors";
import { trustKey } from "../lib/nodeStatus";
import type { Inventory } from "../lib/types";

/** Survey and review only. The parent's existing Save/Checks flow owns persistence. */
export function NetworkDiscovery({ inventory, user, onUse }: { inventory: Inventory; user: string; onUse: (next: Inventory) => void }) {
  const source = useDataSource();
  const [open, setOpen] = useState(false);
  const [scopes, setScopes] = useState<DiscoveryScope[]>([]);
  const [scope, setScope] = useState("");
  const [run, setRun] = useState<DiscoveryRun | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [proposal, setProposal] = useState<DiscoveryProposal | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [used, setUsed] = useState(false);
  const epoch = useRef(0), owned = useRef<string | null>(null), pending = useRef(false);
  const base = JSON.stringify([inventory, user, selected]);
  const latestBase = useRef(base); latestBase.current = base;
  const proposalBase = useRef("");
  const proposalView = useRef<HTMLDivElement>(null);
  useEffect(() => { if (proposal) proposalView.current?.scrollIntoView?.({ block: "start" }); }, [proposal]);
  const fresh = (e: number) => epoch.current === e && currentSource() === source;
  const stopOwned = () => { const id = owned.current; owned.current = null; if (id) void discoveryApi.cancel(id).catch(() => {}); };
  useEffect(() => {
    ++epoch.current; stopOwned(); pending.current = false; setBusy(false); setRun(null); setSelected([]); setProposal(null); setScopes([]); setScope(""); setError(""); setUsed(false);
    return () => { ++epoch.current; stopOwned(); };
  }, [source]);
  useEffect(() => { setProposal(null); }, [base]);
  useEffect(() => {
    if (!open) return;
    const e = epoch.current;
    discoveryApi.networks().then((s) => { if (fresh(e)) { setScopes(s); const eligible = s.filter((x) => x.eligible); setScope(eligible.length === 1 ? eligible[0].id : ""); } }, (err) => { if (fresh(e)) setError(asSbError(err).message); });
  }, [source, open]);
  useEffect(() => {
    if (!run || run.status !== "running") return;
    const e = epoch.current, id = run.id;
    let active = true, timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = await discoveryApi.status();
        if (!active || !fresh(e)) return;
        if (next?.id === id && next.demo === (source === "demo")) setRun(next);
        else { setRun(null); setError("Survey changed or expired. Survey again."); }
      } catch (err) { if (active && fresh(e)) setError(asSbError(err).message); }
      if (active && fresh(e)) timer = setTimeout(poll, 500);
    };
    timer = setTimeout(poll, 500);
    return () => { active = false; clearTimeout(timer); };
  }, [run?.id, run?.status, source]);
  const action = async (work: (e: number) => Promise<void>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError(""); setUsed(false);
    const e = epoch.current;
    try { await work(e); } catch (err) { if (fresh(e) && err !== ADMIN_REFUSED) setError(asSbError(err).message); }
    finally { if (fresh(e)) { pending.current = false; setBusy(false); } }
  };
  const start = () => action(async (e) => {
    setProposal(null); setSelected([]);
    const r = await withAdmin(() => {
      if (!fresh(e)) throw ADMIN_REFUSED;
      return discoveryApi.start(scope, source === "demo");
    }, { scope: "mac", action: "discovery_start" });
    if (!fresh(e) || r.demo !== (source === "demo")) { await discoveryApi.cancel(r.id); return; }
    owned.current = r.id; setRun(r);
  });
  const verify = (c: DiscoveryCandidate) => action(async (e) => {
    if (!run) return;
    setProposal(null);
    setSelected((s) => s.filter((x) => x !== c.address));
    if (c.status === "trust" && c.fingerprint) {
      // Changed-key typing is checked against the saved inventory by the backend. Use
      // its saved name when present; otherwise its exact host:port identifier.
      const saved = inventory.nodes.find((n) => (n.ip || n.host) === c.address && n.port === 22);
      if (!(await trustKey({ name: saved?.name ?? `${c.address}:22`, address: `${c.address}:22`, fingerprint: c.fingerprint, changed: c.changedKey, sharedWith: run.candidates.filter((x) => x.address !== c.address && x.fingerprint === c.fingerprint).map((x) => x.address) })) || !fresh(e)) return;
    }
    const r = await withAdmin(() => {
      if (!fresh(e)) throw ADMIN_REFUSED;
      return discoveryApi.verify(run.id, c.address, user.trim());
    }, { scope: "mac", action: "discovery_verify" });
    if (fresh(e) && r.id === owned.current) setRun(r);
  });
  const preview = () => action(async (e) => {
    if (!run) return;
    const b = latestBase.current;
    const p = await discoveryApi.propose(run.id, selected, inventory, user.trim());
    if (fresh(e) && latestBase.current === b) { proposalBase.current = b; setProposal(p); }
  });
  const useProposal = () => action(async (e) => {
    if (!run || !proposal || proposalBase.current !== latestBase.current) return;
    const b = latestBase.current;
    // Revalidate freshness, interface and identity pins immediately before copying the draft.
    const p = await discoveryApi.propose(run.id, selected, inventory, user.trim());
    if (fresh(e) && b === latestBase.current && JSON.stringify(p) === JSON.stringify(proposal)) {
      onUse(p.inventory); setProposal(null); setUsed(true);
    } else if (fresh(e)) { setProposal(null); setError("The proposal changed. Preview it again."); }
  });
  const stop = () => { ++epoch.current; stopOwned(); pending.current = false; setBusy(false); setProposal(null); setRun((r) => r && { ...r, status: "cancelled", verifying: null, message: "Stopped. Survey again to propose changes." }); };
  const working = busy || run?.status === "running";
  return <section className="network-discovery" aria-label="Find Sparks">
    <div className="wizard-inline"><button className="btn" aria-expanded={open} onClick={() => { if (open) stop(); setOpen(!open); }}>{open ? "Close discovery" : "Find Sparks on my network…"}</button><HelpTip topic="start.connect.discovery" /></div>
    {open && <>
      <p>{source === "demo" ? "Demo uses a recorded /24 fixture. No network traffic or real credentials." : "Survey one local private /24 on this Mac’s Ethernet or Wi-Fi. Only ports 22, 80 and 443 are checked; at most two minutes. No credentials are sent until you select Verify and trust the SSH key."}</p>
      <div className="wizard-inline"><label htmlFor="discovery-network">Local network</label><select id="discovery-network" value={scope} disabled={!!working} onChange={(e) => { setScope(e.target.value); setProposal(null); }}><option value="">Choose a network</option>{scopes.map((s) => <option key={s.id} value={s.id} disabled={!s.eligible}>{s.interface} · {s.address} · {s.network}{!s.eligible ? " · unavailable" : ""}</option>)}</select></div>
      {scopes.length === 0 && <p className="muted">No eligible network is available. Connect locally or enter node addresses below.</p>}
      {scopes.filter((s) => !s.eligible).map((s) => <p className="muted" key={s.id}>{s.interface}: {s.reason}</p>)}
      <div className="wizard-inline"><button className="btn" disabled={!scope || !!working} onClick={start}>Survey this /24</button>{!!working && <button className="btn" onClick={stop}>Stop discovery</button>}</div>
      {error && <Notice tone="error">{error}</Notice>}
      {run && <>
        <p role="status">{run.scope.network} · {run.checked}/{run.total} addresses · {run.status === "running" ? "Surveying…" : run.message}</p>
        {run.candidates.length > 0 && <><p className="muted">Verify only devices you recognize. Verification uses the Account user, SSH keys and saved password; reads hardware and neighbors; and makes no configuration changes.</p><table className="grid discovery-candidates"><thead><tr><th>Use</th><th>Address / services</th><th>Identity</th><th>Verify</th></tr></thead><tbody>{run.candidates.map((c) => <tr key={c.address}>
          <td><input type="checkbox" aria-label={`Include ${c.address}`} checked={selected.includes(c.address)} disabled={!!working || run.status !== "complete" || c.status !== "spark"} onChange={(e) => setSelected((s) => e.target.checked ? [...s, c.address] : s.filter((x) => x !== c.address))} /></td>
          <td><code>{c.address}</code><small>{c.ports.map((p) => p === 22 ? "SSH" : p === 80 ? "HTTP" : "HTTPS").join(" · ")}</small></td>
          <td><strong>{c.status === "spark" ? `${c.facts?.hostname} · GB10 Spark` : c.hint}</strong><small>{c.message}</small>{c.facts && <small>{c.facts.model} · MAC {c.facts.interfaces.find((i) => i.addresses.some((a) => a.split("/")[0] === c.address))?.mac}</small>}</td>
          <td>{c.ports.includes(22) ? <button className="btn" disabled={!!working || run.status !== "complete"} aria-label={`${c.status === "trust" ? "Trust and verify" : "Verify"} ${c.address}`} onClick={() => verify(c)}>{c.status === "trust" ? "Trust…" : "Verify"}</button> : <span className="muted">Unverified equipment</span>}</td>
        </tr>)}</tbody></table></>}
        <button className="btn" disabled={!!working || run.status !== "complete" || selected.length === 0 || selected.length > 4} onClick={preview}>Preview topology</button>
      </>}
      {proposal && <div className="discovery-proposal" ref={proposalView}><h3>Proposed topology</h3><p>Review observed networks and reported neighbors. Use this proposal to update this wizard’s draft; the existing Next, Checks or Done action saves it.</p>
        <ul>{proposal.changes.map((c) => <li key={c.id}>{c.name} · {c.address} — {c.action}</li>)}</ul>
        {Object.entries(proposal.networks).map(([network, members]) => <div key={network}><h4>{network}</h4><ul>{members.map((m) => <li key={m}>{m}</li>)}</ul></div>)}
        {proposal.links.length > 0 && <><h4>Reported neighbors (LLDP)</h4><ul>{proposal.links.map((l, i) => <li key={i}>{l.node} / {l.iface} → {l.peer} / {l.port} <small>{l.evidence}</small></li>)}</ul></>}
        {proposal.warnings.map((w) => <p key={w} className="discovery-warning">{w}</p>)}
        <button className="btn primary" disabled={busy || proposalBase.current !== base} onClick={useProposal}>Use proposal in draft</button>
      </div>}
      {used && <Notice tone="info">Proposal copied to the draft below. Review it before saving with Next, Checks or Done.</Notice>}
    </>}
  </section>;
}
