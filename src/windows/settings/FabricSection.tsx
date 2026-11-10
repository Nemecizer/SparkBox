import { useState } from "react";
import { FormGroup, FormRow, HelpTip } from "../../components/ui";
import { fabricErrors, reportedFabric } from "../../lib/fabricInventory";
import type { FabricExpectations, FabricLink, NodeConfig, NodeSnapshot } from "../../lib/types";

export function FabricSection({ nodes, snapshots, editNode }: {
  nodes: NodeConfig[];
  snapshots: NodeSnapshot[];
  editNode: (index: number, patch: Partial<NodeConfig>) => void;
}) {
  const [selected, select] = useState(nodes[0]?.id ?? "");
  const index = Math.max(0, nodes.findIndex((n) => n.id === selected));
  const node = nodes[index];
  if (!node) return null;
  const fabric = node.fabric ?? { links: [], headCandidate: null };
  const reported = reportedFabric(snapshots.find((n) => n.id === node.id));
  const errors = fabricErrors(fabric);
  // Unaddressed spare ports are observable but aren't part of the expected fabric path.
  const addressed = reported?.filter((l) => l.ip) ?? [];
  const canSeed = !!addressed.length && !Object.keys(fabricErrors({ links: addressed, headCandidate: null })).length;
  const update = (patch: Partial<FabricExpectations>) => editNode(index, { fabric: { ...fabric, ...patch } });
  const editLink = (i: number, field: keyof FabricLink, value: string) => update({ links: fabric.links.map((l, j) => j === i ? { ...l, [field]: value } : l) });
  return <section className="node-fabric" aria-label="Fabric and load role">
    <FormGroup title={<>Fabric and load role <HelpTip topic="start.nodes.fabric" /></>} footnote="Expectations are saved on this Mac. Loads use fresh node checks; these fields never configure a node or change the selected head.">
      <FormRow label="Node"><select aria-label="Fabric node" value={node.id} onChange={(e) => select(e.target.value)}>
        {nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}
      </select></FormRow>
      <FormRow label="Reported fabric"><div className="node-fabric-reported">
        {reported === null ? <span className="muted">Unknown — waiting for fresh readings.</span> : !reported.length ? <span className="muted">No fabric interfaces reported.</span> : reported.map((l) => <div key={l.iface}><span className="mono">{l.iface} · {l.ip || "no IPv4"} · {l.rdma || "RDMA device not reported"}</span></div>)}
      </div></FormRow>
      <FormRow label="Load role" hint="The head answers API requests; this is a preference, not permission to load.">
        <select aria-label="Load role" value={fabric.headCandidate === null ? "none" : fabric.headCandidate ? "head" : "worker"} onChange={(e) => update({ headCandidate: e.target.value === "none" ? null : e.target.value === "head" })}>
          <option value="none">No preference</option><option value="head">Head candidate</option><option value="worker">Worker preferred</option>
        </select>
      </FormRow>
      <div className="node-fabric-links">
        {!fabric.links.length && <p className="muted">No fabric expectations recorded. Copy the reported values or add an interface, review them, then save.</p>}
        {fabric.links.map((l, i) => <div className="node-fabric-link" key={i}>
          {([["iface", "Interface"], ["ip", "Fabric IPv4"], ["rdma", "RDMA device (HCA)"]] as const).map(([field, label]) => <label key={field}>{label}
            <input className="mono" aria-label={`${node.name} fabric ${i + 1} ${label}`} value={l[field]} spellCheck={false} autoComplete="off" aria-invalid={!!errors[`${i}:${field}`] || undefined} title={errors[`${i}:${field}`]} onChange={(e) => editLink(i, field, e.target.value)} />
          </label>)}
          <button className="btn" aria-label={`Remove fabric interface ${i + 1}`} onClick={() => update({ links: fabric.links.filter((_, j) => i !== j) })}>Remove</button>
        </div>)}
      </div>
      <div className="inv-foot">
        <button className="btn" disabled={!canSeed} onClick={() => update({ links: addressed })}>Use reported fabric</button>
        <button className="btn" onClick={() => update({ links: [...fabric.links, { iface: "", ip: "", rdma: "" }] })}>Add interface</button>
        <button className="btn" disabled={!node.fabric} onClick={() => editNode(index, { fabric: null })}>Clear expectations</button>
      </div>
    </FormGroup>
  </section>;
}
