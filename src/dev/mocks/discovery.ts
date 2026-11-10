// Browser-only Demo fixture. Native discovery and proposal rules are tested in Rust.
import candidates from "../../../src-tauri/src/discovery/fixture.json";
import facts from "../../../src-tauri/src/discovery/facts.json";
import type { DiscoveryCandidate, DiscoveryFacts, DiscoveryProposal, DiscoveryRun } from "../../lib/discovery";
import type { Inventory } from "../../lib/types";
import type { MockHandlers } from "../../test/tauri";
export const scope = { id: "demo-lan", interface: "Demo Ethernet", address: "192.168.50.10", network: "192.168.50.0/24", eligible: true, reason: "Fixture only; no network traffic" };
let run: DiscoveryRun | null = null;
export const handlers: MockHandlers = {
  discovery_networks: () => [scope],
  discovery_start: () => {
    run = { id: `demo-${Date.now()}`, scope, demo: true, startedMs: Date.now(), status: "complete", checked: 253, total: 253, message: "Survey complete. Verify selected SSH candidates to identify GB10 Sparks.", verifying: null, candidates: structuredClone(candidates) as DiscoveryCandidate[] };
    return structuredClone(run);
  },
  discovery_status: () => structuredClone(run),
  discovery_cancel: () => { if (run) run.status = "cancelled"; },
  discovery_verify: ({ address }) => {
    const c = run?.candidates.find((c) => c.address === address); if (!c || !run || run.status !== "complete") throw "Survey again.";
    const f = (facts as Record<string, DiscoveryFacts>)[address as string];
    Object.assign(c, f ? { status: "spark", facts: f, fingerprint: "Demo key — no real trust", message: "GB10 reported through trusted SSH. Review identity and topology before using the proposal." } : { status: "error", message: "Demo: this device is a generic SSH host, not a Spark." });
    return structuredClone(run);
  },
  discovery_propose: ({ selected, inventory, user }) => {
    if (!run || run.status !== "complete" || Date.now() - run.startedMs >= 600_000) throw "Survey again.";
    const p: DiscoveryProposal = { inventory: structuredClone(inventory) as Inventory, changes: [], links: [], networks: {}, warnings: ["Shared network addresses establish network membership, not physical cabling. LLDP neighbors are reports, not verified switch configuration."] };
    for (const address of selected as string[]) {
      const c = run.candidates.find((c) => c.address === address && c.status === "spark"); if (!c?.facts) throw "Verify this candidate first.";
      const f = c.facts;
      const old = p.inventory.nodes.find((n) => n.ip === address || n.host.replace(/\.local$/, "") === f.hostname);
      const node = { ...old, id: old?.id ?? `demo-${f.interfaces[0].mac.replaceAll(":", "")}`, name: old?.name ?? f.hostname, ip: address, host: `${f.hostname}.local`, port: 22, user: user as string, model: f.model };
      if (old) p.inventory.nodes[p.inventory.nodes.indexOf(old)] = node; else p.inventory.nodes.push(node);
      p.changes.push({ id: node.id, name: node.name, address, action: old ? "Update address; preserve saved ID and expectations" : "Add verified Spark" });
      for (const i of f.interfaces) for (const a of i.addresses) { const net = a.replace(/\d+\/24$/, "0/24"); (p.networks[net] ??= []).push(`${node.name} · ${i.name} · ${a} · ${i.speedMbps} Mb/s`); }
      for (const n of f.neighbors) p.links.push({ node: node.name, iface: n.iface, peer: n.system, port: n.port, evidence: `LLDP chassis ${n.chassis}` });
      if (!f.neighbors.length) p.warnings.push(`${node.name}: no LLDP neighbors were reported; cabling is unknown.`);
    }
    return p;
  },
};
