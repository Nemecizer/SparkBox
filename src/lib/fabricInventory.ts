import type { FabricExpectations, FabricLink, Inventory, NodeSnapshot } from "./types";

export const isIPv4 = (s: string) => /^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/.test(s);
const deviceName = (s: string) => /^[A-Za-z0-9._-]{1,64}$/.test(s);

/** Keys are relative to the node's fabric field, shared by the editor and save validation. */
export function fabricErrors(fabric?: FabricExpectations | null): Record<string, string> {
  const errors: Record<string, string> = {};
  const seen = new Set<string>();
  fabric?.links.forEach((l, i) => {
    if (!deviceName(l.iface)) errors[`${i}:iface`] = "Use 1–64 letters, digits, dots, underscores or hyphens.";
    else if (seen.has(l.iface)) errors[`${i}:iface`] = "This interface is listed twice.";
    seen.add(l.iface);
    if (!isIPv4(l.ip)) errors[`${i}:ip`] = "Enter a valid fabric IPv4 address.";
    if (l.rdma && !deviceName(l.rdma)) errors[`${i}:rdma`] = "Use 1–64 letters, digits, dots, underscores or hyphens, or leave blank.";
  });
  return errors;
}

/** Unknown readings never seed an owner confirmation from cached facts. */
export function reportedFabric(node?: NodeSnapshot): FabricLink[] | null {
  if (!node || node.status !== "online" || node.stale) return null;
  return node.nets.filter((n) => n.role === "fabric").map((n) => ({ iface: n.iface, ip: n.ip ?? "", rdma: n.rdmaDev ?? "" }));
}

export function inventoryRequiresReconnect(next: Inventory, old: Inventory): boolean {
  const withoutFabric = (i: Inventory) => ({ ...i, nodes: i.nodes.map(({ fabric: _fabric, ...n }) => n) });
  return JSON.stringify(withoutFabric(next)) !== JSON.stringify(withoutFabric(old));
}
