import { useEffect, useRef, useState } from "react";
import { Notice } from "../../components/ui";
import { api } from "../../lib/api";
import { fmtBytes } from "../../lib/format";
import type { DownloadCapacity as Capacity, DownloadSelection } from "../../lib/types";
import { errText, type NodeRef } from "./shared";

export type CapacityNode = NodeRef & { unavailable?: string };
const MAX_AGE_MS = 60_000;

/** A comparison never sets the target. Only the explicit Use button does that. */
export function DownloadCapacity({ selection, nodes, target, disabled, onSelect }: {
  selection: DownloadSelection; nodes: CapacityNode[]; target: string | null;
  disabled: boolean; onSelect: (node: string) => void;
}) {
  const [scope, setScope] = useState(() => nodes.filter((n) => n.online && !n.unavailable).map((n) => n.id));
  const eligible = nodes.filter((n) => scope.includes(n.id) && n.online && !n.unavailable).map((n) => n.id).sort();
  const context = JSON.stringify([selection, scope.slice().sort(), nodes.map((n) => [n.id, n.online, n.unavailable]).sort()]);
  const [review, setReview] = useState<{ context: string; result: Capacity; expires: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [now, setNow] = useState(Date.now);
  const generation = useRef(0);
  useEffect(() => {
    generation.current++;
    setReview(null); setError(null); setPending(false);
    return () => { generation.current++; };
  }, [context, disabled]);
  useEffect(() => {
    if (!review) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [review]);
  const current = review?.context === context && !disabled ? review : null;
  const expired = !!current && now >= current.expires;
  const suggested = current && nodes.find((n) => n.id === current.result.suggested && eligible.includes(n.id));
  const compare = async () => {
    const request = ++generation.current;
    const started = Date.now();
    setReview(null); setError(null); setPending(true);
    try {
      const result = await api.modelDownloadCapacity(structuredClone(selection), eligible);
      if (request !== generation.current) return;
      setNow(Date.now());
      setReview({ context, result, expires: started + MAX_AGE_MS });
    } catch (e) {
      if (request === generation.current) setError(errText(e));
    } finally {
      if (request === generation.current) setPending(false);
    }
  };
  return <details className="col gap-sm">
    <summary>Suggest a download node</summary>
    <div className="col gap-sm">
      <p>Select the nodes you are considering for this load, then compare their destination disks. This does not select load members or start a download or copy.</p>
      <div className="ai-capacity-row" role="group" aria-label="Nodes to compare">
        {nodes.map((node) => <label key={node.id} className="ai-capacity-check">
          <input type="checkbox" aria-label={`Compare ${node.name}`} checked={scope.includes(node.id)} disabled={disabled || !node.online || !!node.unavailable}
            onChange={(e) => setScope((old) => e.target.checked ? [...old, node.id] : old.filter((id) => id !== node.id))} />
          {node.name}{!node.online ? " — not connected" : node.unavailable ? ` — ${node.unavailable}` : ""}
        </label>)}
      </div>
      <div><button type="button" className="btn small" disabled={disabled || pending || !eligible.length} onClick={compare}>{pending ? "Checking capacity…" : "Compare destination capacity"}</button></div>
      {!eligible.length && <span className="muted">Select a connected node without a finished or running copy to compare.</span>}
      {error && <Notice tone="warn">{error}</Notice>}
      {current && <>
        <p>Suggested by space left after the full transfer plus 5% and 20 GiB. Ties use node ID. Readings expire after one minute and do not reserve space. Download checks the disk again.</p>
        {current.result.readings.map((row) => <div className="col gap-sm" key={row.node}>
          <span className="strong">{nodes.find((n) => n.id === row.node)?.name ?? row.node} · {row.check ? row.check.ok ? "Enough space" : "Not enough space" : "Could not check"}</span>
          {row.check && <>
            <span className="mono ai-capacity-path">{row.check.dest}</span>
            <span>{fmtBytes(row.check.availableBytes)} available · {fmtBytes(row.check.requiredBytes)} required{row.check.ok ? ` · ${fmtBytes(row.check.availableBytes - row.check.requiredBytes)} left after reserve` : ""}</span>
            {row.check.warning && <Notice tone="warn">{row.check.warning}</Notice>}
          </>}
          {row.error && <Notice tone="warn">{row.error}</Notice>}
        </div>)}
        {expired ? <Notice tone="info">Capacity readings expired. Compare again before using the suggestion.</Notice> : suggested ? <div className="ai-capacity-row">
          <span>Suggested: {suggested.name}</span>
          <button type="button" className="btn small" disabled={target === suggested.id} onClick={() => {
            if (Date.now() < current.expires) onSelect(suggested.id);
            else setNow(Date.now());
          }}>{target === suggested.id ? "Already selected" : `Use ${suggested.name}`}</button>
        </div> : <Notice tone="warn">No checked node has enough known destination space. Your download node has not changed.</Notice>}
      </>}
    </div>
  </details>;
}
