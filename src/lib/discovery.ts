import { invoke } from "@tauri-apps/api/core";
import type { Inventory } from "./types";
export interface DiscoveryScope { id: string; interface: string; address: string; network: string; eligible: boolean; reason: string }
export interface DiscoveryFacts {
  hostname: string; machineId: string; model: string; gpu: string;
  interfaces: { name: string; mac: string; addresses: string[]; mtu: number | null; speedMbps: number | null }[];
  neighbors: { iface: string; system: string; chassis: string; port: string }[];
}
export interface DiscoveryCandidate {
  address: string; ports: number[]; hint: string; status: string; message: string;
  fingerprint: string | null; changedKey: boolean; facts: DiscoveryFacts | null;
}
export interface DiscoveryRun {
  id: string; demo: boolean; scope: DiscoveryScope; startedMs: number; status: string; checked: number; total: number;
  message: string; verifying: string | null; candidates: DiscoveryCandidate[];
}
export interface DiscoveryProposal {
  inventory: Inventory;
  changes: { id: string; name: string; address: string; action: string }[];
  links: { node: string; iface: string; peer: string; port: string; evidence: string }[];
  networks: Record<string, string[]>; warnings: string[];
}
export const discoveryApi = {
  networks: () => invoke<DiscoveryScope[]>("discovery_networks"),
  start: (scopeId: string, expectedDemo: boolean) => invoke<DiscoveryRun>("discovery_start", { scopeId, expectedDemo }),
  status: () => invoke<DiscoveryRun | null>("discovery_status"),
  cancel: (id: string) => invoke<void>("discovery_cancel", { id }),
  verify: (id: string, address: string, user: string) => invoke<DiscoveryRun>("discovery_verify", { id, address, user }),
  propose: (id: string, selected: string[], inventory: Inventory, user: string) => invoke<DiscoveryProposal>("discovery_propose", { id, selected, inventory, user }),
};
