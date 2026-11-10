import { describe, expect, it } from "vitest";
import type { LldpNeighbor, NetStats, NodeSnapshot } from "../../lib/types";
import { lldpNode, lldpView, speed } from "./NetworkTab";

describe("the link column's speed", () => {
  it("is compact and never reads like a part number (cell3's Wi-Fi at 2402 Mb/s)", () => {
    expect(speed({ up: true, speedMbps: 200_000 })).toBe("200G");
    expect(speed({ up: true, speedMbps: 1000 })).toBe("1G");
    expect(speed({ up: true, speedMbps: 2402 })).toBe("2.4G");
    expect(speed({ up: true, speedMbps: 867 })).toBe("867M");
    expect(speed({ up: true, speedMbps: 0 })).toBe("—");
    expect(speed({ up: false, speedMbps: 1000 })).toBe("—");
  });
});

// ── r1-c ──
/** A Spark as the Live tour found it (doctor.txt): its hostname and its NICs' MACs. */
function spark(id: string, hostname: string, macs: Record<string, string>, heard: Record<string, { peers?: LldpNeighbor[]; selfOnly?: boolean; lldp?: LldpNeighbor | null }> = {}): NodeSnapshot {
  const nets = Object.entries(macs).map(([iface, mac]) => ({
    iface, mac, role: iface.startsWith("enP7") ? "lan" : iface.startsWith("wl") ? "wifi" : "fabric", up: true, speedMbps: 200_000, rxBps: 0, txBps: 0, ip: null,
    lldpPeers: heard[iface]?.peers ?? [], lldpSelfOnly: heard[iface]?.selfOnly ?? false, lldp: heard[iface]?.lldp ?? heard[iface]?.peers?.[0] ?? null,
  })) as NetStats[];
  return { id, name: id, system: { hostname }, nets } as unknown as NodeSnapshot;
}

const CELL1 = { enP7s7: "fc:9d:05:d5:54:96", enp1s0f1np1: "fc:9d:05:c3:bb:46", enP2p1s0f1np1: "fc:9d:05:ed:d4:6c" };
const CELL3 = { enP7s7: "fc:9d:05:5e:bd:9c", enp1s0f1np1: "fc:9d:05:ea:06:b7", enP2p1s0f1np1: "fc:9d:05:33:7f:84" };
const CELL2 = { enP7s7: "fc:9d:05:9b:e9:14", enp1s0f1np1: "fc:9d:05:3d:3e:ae", enP2p1s0f1np1: "fc:9d:05:d1:dc:1c" };
const cell3Lan: LldpNeighbor = { system: "cell3-example", port: "enP7s7", chassisId: "fc:9d:05:5e:bd:9c", portId: "fc:9d:05:5e:bd:9c" };

describe("LLDP never names a node's own NIC as its switch port", () => {
  // cell1 as the frame-6 collector sends it (live/fixtures/lldp/cell1-neighbors.json): the two
  // fabric halves heard only cell1 itself (dropped there), the LAN port heard cell3.
  const cell1 = spark("cell1", "cell1-example", CELL1, { enp1s0f1np1: { selfOnly: true }, enP2p1s0f1np1: { selfOnly: true }, enP7s7: { peers: [cell3Lan] } });
  const cell3 = spark("cell3", "cell3-example", CELL3);
  const nodes = [cell1, cell3, spark("cell2", "cell2-example", CELL2)];
  const fabric = cell1.nets.filter((x) => x.role === "fabric");
  const lan = cell1.nets.find((x) => x.iface === "enP7s7")!;

  it("reads Port 2 as no LLDP from the switch, and the LAN neighbour as the node it is", () => {
    expect(lldpView(fabric, cell1, nodes)).toEqual({ kind: "self", text: "No LLDP from the switch (only this node's other half answers)", title: expect.stringContaining("hears its own LLDP back") });
    expect(lldpView([fabric[0]], cell1, nodes)?.text, "a half on its own (Fabric health)").toBe("No LLDP from the switch (only this node's other half answers)");
    expect(lldpView([lan], cell1, nodes)).toMatchObject({ kind: "peer", text: "Neighbor: cell3 (via switch)" });
    for (const x of [...fabric, lan]) expect(lldpView([x], cell1, nodes)?.text).not.toMatch(/fc:9d:05:13:87|Switch port/);
  });

  it("drops cell1's own MAC heard from a collector older than frame 6", () => {
    // Frame 5 sent one neighbour per interface and kept itself: "Switch port fc:9d:05:c3:bb:46".
    const old = spark("cell1", "cell1-example", CELL1, {
      enP2p1s0f1np1: { lldp: { system: "cell1-example", port: "fc:9d:05:c3:bb:46" } },
      enp1s0f1np1: { lldp: { system: null, port: "fc:9d:05:ed:d4:6c" } },
      enP7s7: { lldp: { system: "cell3-example", port: "fc:9d:05:5e:bd:9c" } },
    });
    const halves = old.nets.filter((x) => x.role === "fabric");
    expect(lldpView(halves, old, nodes)?.text).toBe("No LLDP from the switch (only this node's other half answers)");
    expect(lldpView([old.nets.find((x) => x.iface === "enP7s7")!], old, nodes)?.text).toBe("Neighbor: cell3 (via switch)");
  });

  it("names a switch that speaks LLDP by its port, and other nodes heard through it as nodes", () => {
    const crs: LldpNeighbor = { system: "crs804", port: "qsfp56-dd-1-1", chassisId: "48:a9:8a:00:00:01", portId: "qsfp56-dd-1-1" };
    const viaCrs: LldpNeighbor = { system: "cell2-example", port: "enp1s0f1np1", chassisId: "fc:9d:05:9b:e9:14", portId: "fc:9d:05:3d:3e:ae" };
    const withSwitch = spark("cell1", "cell1-example", CELL1, { enp1s0f1np1: { peers: [viaCrs, crs] } });
    expect(lldpView(withSwitch.nets.filter((x) => x.iface === "enp1s0f1np1"), withSwitch, nodes)).toMatchObject({ kind: "switch", text: "Switch port qsfp56-dd-1-1 (LLDP)" });
    const peers = spark("cell1", "cell1-example", CELL1, { enp1s0f1np1: { peers: [viaCrs] }, enP2p1s0f1np1: { peers: [{ ...viaCrs, portId: "fc:9d:05:d1:dc:1c", system: null }] } });
    expect(lldpView(peers.nets.filter((x) => x.role === "fabric"), peers, nodes)).toMatchObject({ kind: "peer", text: "No LLDP from the switch (cell2 answers through it)" });
    // A neighbour without a name is still a node when its chassis is one of the node's MACs.
    expect(lldpNode({ system: null, chassisId: "fc:9d:05:5e:bd:9c" }, nodes)?.id).toBe("cell3");
    expect(lldpNode(crs, nodes)).toBeNull();
    expect(lldpView([spark("cell1", "cell1-example", CELL1).nets[0]], cell1, nodes), "nothing heard").toBeNull();
  });
});

describe("LLDP on the real cluster, Oct 29 evening", () => {
  // As the frame-6 collector read it: each node's fabric halves hear the CRS804 on their own port,
  // and every LAN port hears the CRS804's ether2, the Firewalla Switch X's Gi0/2 and the other nodes.
  const crs = (port: string): LldpNeighbor => ({ system: "CRS804", port, chassisId: "38:32:7a:79:9b:9e", portId: port });
  const lan = (except: string): LldpNeighbor[] => [
    crs("ether2"),
    { system: "Firewalla-Switch-X", port: "Gi0/2", chassisId: "20:6d:31:be:00:15", portId: "Gi0/2" },
    ...[["cell1-example", "fc:9d:05:d5:54:96"], ["cell2-example", "fc:9d:05:9b:e9:14"], ["cell3-example", "fc:9d:05:5e:bd:9c"]]
      .filter(([h]) => !h.startsWith(except))
      .map(([system, mac]) => ({ system, port: "enP7s7", chassisId: mac, portId: mac })),
  ];
  const node = (id: string, macs: Record<string, string>, fabricPort: string) =>
    spark(id, `${id}-example`, macs, { enp1s0f1np1: { peers: [crs(fabricPort)] }, enP2p1s0f1np1: { peers: [crs(fabricPort)] }, enP7s7: { peers: lan(id) } });
  const nodes = [node("cell1", CELL1, "qsfp56-dd-1-1"), node("cell2", CELL2, "qsfp56-dd-4-1"), node("cell3", CELL3, "qsfp56-dd-2-1")];

  it("reads each node's own CRS804 port on Port 2, and its LAN neighbours through the switch", () => {
    const [cell1] = nodes;
    expect(lldpView(cell1.nets.filter((x) => x.role === "fabric"), cell1, nodes)?.text).toBe("Switch port qsfp56-dd-1-1 (LLDP)");
    const view = lldpView([cell1.nets.find((x) => x.iface === "enP7s7")!], cell1, nodes)!;
    expect(view.text).toBe("Neighbors: cell2 and cell3 (via switch)");
    expect(view.title).toContain("Also heard, as every node hears it: CRS804 ether2, Firewalla-Switch-X Gi0/2");
  });
});
