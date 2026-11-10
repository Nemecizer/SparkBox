import { describe, expect, it } from "vitest";
import type { MspPort, MspStatus, PortLabel, PortsView } from "../../lib/firewalla";
import type { ClusterSnapshot, LldpNeighbor, NetStats, NodeSnapshot, SwitchSnapshot } from "../../lib/types";
import { behindText, buildPorts, cloudLine, flaps, isTheSwitch, lldpPorts, MSP_STALE_MS, parseLldpPort, speedLight, type XPort } from "./switchX";

const NOW = 1_800_000_000_000;
const sw = { id: "firewalla", name: "Firewalla Switch X", kind: "firewalla", host: "192.168.50.238" } as SwitchSnapshot;
const crs = { id: "crs804", name: "CRS804 DDQ", kind: "routerOs" } as SwitchSnapshot;
const gold = { id: "gold", name: "Firewalla Gold", kind: "gateway" } as SwitchSnapshot;

const SWITCH: LldpNeighbor = { system: "Firewalla-Switch-X", port: "Gi0/2", chassisId: "20:6d:31:be:00:15", portId: "Gi0/2" };
const CRS: LldpNeighbor = { system: "CRS804", port: "ether2", chassisId: "38:32:7a:79:9b:9e", portId: "ether2" };

const macOf = (name: string) => `aa:bb:cc:00:00:0${name.length % 10}${name.at(-1)}`.slice(0, 17);

function node(name: string, nic: Partial<NetStats> = {}, status: NodeSnapshot["status"] = "online"): NodeSnapshot {
  const peers = (["cell1", "cell2", "cell3", "gx10-0000"].filter((n) => n !== name).map((n) => ({ system: `${n}-example`, port: "enP7s7", chassisId: macOf(n), portId: macOf(n) })) as LldpNeighbor[]);
  return {
    id: name, name, status, lastSeenMs: NOW - 2000, system: { hostname: `${name}-example` },
    nets: [{ iface: "enP7s7", role: "lan", up: true, speedMbps: 1000, rxBps: 1000, txBps: 2000, rxErrors: 0, txErrors: 0, mac: macOf(name), carrierChanges: 3, lldpPeers: [SWITCH, CRS, ...peers], ...nic }],
  } as unknown as NodeSnapshot;
}

const cluster = (nodes: NodeSnapshot[]) => ({ tsMs: NOW, nodes, switches: [sw, crs, gold] }) as unknown as ClusterSnapshot;
/** This morning's real LAN: four nodes, all hearing Gi0/2, the CRS804 and each other, at 1 Gb/s. */
const real = () => cluster(["cell1", "cell2", "cell3", "gx10-0000"].map((n) => node(n)));

const off: MspStatus = { configured: false, domain: null, state: "off", message: null, lastOkMs: null, lastTryMs: null, nextPollMs: null, sw: null, held: false };
const port = (p: Partial<MspPort>): MspPort => ({ port: 1, linkUp: true, linkSpeedMbps: null, rxBytes: null, txBytes: null, rxErrors: null, txErrors: null, rxDiscards: null, txDiscards: null, statsSinceS: null, poeStatus: null, poeMode: null, poeWatts: null, stpState: null, stpRole: null, portType: null, sfp: null, uplink: false, ...p });
const cloud = (ports: MspPort[], lastOkMs = NOW - 60_000): MspStatus => ({ ...off, configured: true, state: "ok", lastOkMs, sw: { mac: "20:6d:31:be:00:15", name: "X", ip: null, model: null, version: null, online: true, ports, skipped: 0 } });
const view = (labels: Record<string, PortLabel> = {}, msp: MspStatus = off): PortsView => ({ labels, msp });
const build = (c: ClusterSnapshot, v: PortsView | null = view()) => buildPorts(c, sw, v, NOW).ports;
const at = (ports: XPort[], n: number) => ports[n - 1];

describe("reading LLDP", () => {
  it("parses GiX/N and refuses anything it can't place", () => {
    expect(parseLldpPort("Gi0/2")).toBe(2);
    expect(parseLldpPort("gi1/12")).toBe(12);
    expect(parseLldpPort("GigabitEthernet0/3")).toBe(3);
    expect(parseLldpPort("Te0/9")).toBe(9);
    for (const bad of ["Gi0/13", "Gi0/0", "ether2", "Gi2", "port 2", "", null, undefined, "aa:bb:cc:dd:ee:ff"]) expect(parseLldpPort(bad)).toBeNull();
  });

  it("knows the switch by name or by a chassis it has used, and not by being a Firewalla", () => {
    expect(isTheSwitch(SWITCH, sw, new Set())).toBe(true);
    expect(isTheSwitch({ system: "Firewalla Switch X" }, sw, new Set())).toBe(true);
    expect(isTheSwitch({ chassisId: "20:6d:31:be:00:15" }, sw, new Set(["20:6d:31:be:00:15"]))).toBe(true);
    expect(isTheSwitch(CRS, sw, new Set())).toBe(false);
    expect(isTheSwitch({ system: "Firewalla-Gold", chassisId: "20:6d:31:91:49:b6" }, sw, new Set(["20:6d:31:be:00:15"]))).toBe(false);
    expect(isTheSwitch({ system: "Firewalla-Switch-SE", chassisId: "20:6d:31:6f:a9:f6" }, sw, new Set())).toBe(false);
  });

  it("keeps a second Firewalla switch's LLDP off this panel, and lists it as something heard", () => {
    const se: LldpNeighbor = { system: "Firewalla-Switch-SE", port: "Gi0/7", chassisId: "20:6d:31:6f:a9:f6", portId: "Gi0/7" };
    const { ports } = lldpPorts(cluster([node("cell1", { lldpPeers: [SWITCH, se] })]), sw, "20:6d:31:be:00:15");
    expect([...ports.keys()]).toEqual([2]);
    expect(ports.get(2)!.heard.map((d) => d.name)).toEqual(["cell1", "Firewalla-Switch-SE"]);
  });

  it("puts the real LAN on one port: the four nodes and the CRS804, each once", () => {
    const { ports, unplaced, chassis } = lldpPorts(real(), sw);
    expect([...ports.keys()]).toEqual([2]);
    const p = ports.get(2)!;
    expect(p.rawIds).toEqual(["Gi0/2"]);
    expect(p.heard.map((d) => d.name)).toEqual(["cell1", "cell2", "cell3", "gx10-0000", "CRS804"]);
    expect(p.heard.at(-1)).toMatchObject({ port: "ether2", chassis: "38:32:7a:79:9b:9e", node: null });
    expect(p.heard[4].heardBy).toEqual(["cell1", "cell2", "cell3", "gx10-0000"]);
    expect(p.linkMbps).toEqual([1000]);
    expect(behindText(p)).toBe("The nodes behind it link at 1 Gb/s");
    expect(unplaced).toEqual([]);
    expect(chassis).toBe("20:6d:31:be:00:15");
  });

  it("uses a chassis one node heard for a node that heard no name, and says what it can't place", () => {
    const c = cluster([
      node("cell1"),
      node("cell2", { lldpPeers: [{ chassisId: SWITCH.chassisId, port: "Gi0/2", portId: "Gi0/2" }] }),
      node("cell3", { lldpPeers: [{ ...SWITCH, port: "Gi0/77", portId: "Gi0/77" }] }),
    ]);
    const { ports, unplaced } = lldpPorts(c, sw);
    expect(ports.get(2)!.reporters.map((n) => n.name)).toEqual(["cell1", "cell2"]);
    expect(unplaced).toEqual(["Gi0/77 (cell3)"]);
  });

  it("ignores nodes that aren't reporting", () => {
    const { ports } = lldpPorts(cluster([node("cell1"), node("cell2", {}, "offline")]), sw);
    expect(ports.get(2)!.reporters.map((n) => n.name)).toEqual(["cell1"]);
  });

  it("says one node's link in the singular, and two speeds when they differ", () => {
    const one = lldpPorts(cluster([node("cell1")]), sw).ports.get(2)!;
    expect(behindText(one)).toBe("cell1, the node behind it links at 1 Gb/s");
    const two = lldpPorts(cluster([node("cell1"), node("cell2", { speedMbps: 2500 })]), sw).ports.get(2)!;
    expect(behindText(two)).toBe("The nodes behind it link at 1 Gb/s and 2.5 Gb/s");
  });
});

describe("the light on a port", () => {
  it("has a colour only for the switch's five speeds", () => {
    expect([10_000, 5_000, 2_500, 1_000, 100].map(speedLight)).toEqual(["10g", "5g", "2.5g", "1g", "100m"]);
    for (const other of [0, 10, 40_000, null, undefined]) expect(speedLight(other)).toBeNull();
  });

  it("shows the real cluster as one neutral port: LLDP alone is never a speed", () => {
    const ports = build(real());
    expect(ports).toHaveLength(12);
    const p = at(ports, 2);
    expect(p).toMatchObject({ light: "unknown", speedText: null, speedFrom: null, linkWord: "Heard over LLDP", short: "5 heard" });
    expect(p.what).toBe("Port 2: connected, speed unknown, 5 heard through it. The nodes behind it link at 1 Gb/s");
    expect(ports.filter((x) => x.light !== "off").map((x) => x.n)).toEqual([2]);
    expect(ports.filter((x) => x.speedText)).toEqual([]);
  });

  it("draws eight RJ45 ports and four SFP+ cages", () => {
    const ports = build(real());
    expect(ports.filter((p) => p.kind === "rj45").map((p) => p.n)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    expect(ports.filter((p) => p.kind === "sfp").map((p) => p.n)).toEqual([9, 10, 11, 12]);
  });

  it("is unlit with nothing known, and while the view loads shows LLDP alone", () => {
    expect(at(build(cluster([])), 1)).toMatchObject({ light: "off", short: null, what: "Port 1: No link" });
    expect(at(build(real(), null), 2).light).toBe("unknown");
  });

  it("takes the colour from Firewalla's linkSpeed", () => {
    const ports = build(real(), view({}, cloud([port({ port: 1, linkSpeedMbps: 10_000 }), port({ port: 2, linkSpeedMbps: 2_500 }), port({ port: 3, linkUp: false })])));
    expect(at(ports, 1)).toMatchObject({ light: "10g", speedText: "10G", speedFrom: "firewalla", short: "Connected" });
    expect(at(ports, 2)).toMatchObject({ light: "2.5g", speedText: "2.5G", speedFrom: "firewalla", short: "5 heard" });
    expect(at(ports, 3)).toMatchObject({ light: "off", linkWord: "No link" });
  });

  it("keeps a link with no speed, or a speed the switch doesn't have, neutral", () => {
    const ports = build(cluster([]), view({}, cloud([port({ port: 1 }), port({ port: 2, linkSpeedMbps: 40_000 })])));
    expect(at(ports, 1)).toMatchObject({ light: "unknown", speedText: null, linkWord: "Up, speed not reported" });
    expect(at(ports, 2)).toMatchObject({ light: "unknown", speedText: null, linkWord: "Up, 40 Gb/s" });
  });

  it("stops trusting the cloud once its answer is an hour old, but still shows the reading", () => {
    const ports = build(cluster([]), view({}, cloud([port({ port: 1, linkSpeedMbps: 10_000 })], NOW - MSP_STALE_MS)));
    expect(at(ports, 1)).toMatchObject({ light: "off", speedText: null, mspUnused: true });
    expect(at(ports, 1).msp?.linkSpeedMbps).toBe(10_000);
    expect(at(build(cluster([]), view({}, cloud([port({ port: 1, linkSpeedMbps: 10_000 })], NOW - MSP_STALE_MS + 1000))), 1).light).toBe("10g");
  });

  it("takes no colour from an answer that says it may not be this switch's", () => {
    const c = cloud([port({ port: 1, linkSpeedMbps: 10_000 })]);
    for (const state of ["not-listed", "shape", "not-json"] as const) {
      expect(at(build(cluster([]), view({}, { ...c, state, message: "No." })), 1)).toMatchObject({ light: "off", speedFrom: null, mspUnused: true });
    }
  });

  it("keeps the last reading through a failure that says nothing against it", () => {
    const c = cloud([port({ port: 1, linkSpeedMbps: 10_000 })]);
    for (const state of ["rate-limited", "http", "unreachable"] as const) {
      expect(at(build(cluster([]), view({}, { ...c, state, message: "Later." })), 1).light).toBe("10g");
    }
  });

  it("takes no colour from a switch the cloud says is offline", () => {
    const c = cloud([port({ port: 1, linkSpeedMbps: 10_000 })]);
    expect(at(build(cluster([]), view({}, { ...c, sw: { ...c.sw!, online: false } })), 1)).toMatchObject({ light: "off", speedFrom: null, mspUnused: true });
    expect(at(build(cluster([]), view({}, { ...c, sw: { ...c.sw!, online: null } })), 1).light).toBe("10g");
  });

  it("colours a labelled single node from its own NIC, and says so", () => {
    // gx10-0000 alone behind Gi0/5, as in Demo; the others on Gi0/2.
    const alone = node("gx10-0000", { lldpPeers: [{ ...SWITCH, port: "Gi0/5", portId: "Gi0/5" }] });
    const ports = build(cluster([node("cell1", { lldpPeers: [SWITCH] }), alone]), view({ 5: { kind: "node", node: "gx10-0000" } }));
    expect(at(ports, 5)).toMatchObject({ light: "1g", speedText: "1G", speedFrom: "node", short: "gx10-0000", notes: [] });
    expect(at(ports, 5).what).toContain("1G, from gx10-0000's own link");
  });

  it("needs LLDP to place a node on the port before its NIC can colour it", () => {
    const p = at(build(real(), view({ 5: { kind: "node", node: "cell1" } })), 5);
    expect(p).toMatchObject({ light: "unknown", speedText: null, speedFrom: null, short: "cell1" });
    expect(p.notes).toEqual(["LLDP places cell1 on port 2, not this one, so its link speed isn't used here. That doesn't prove the label is wrong."]);
    expect(p.notes.join(" ")).not.toContain("can't be this port's");
    // Hears only the CRS804, or hears nothing at all: no evidence either way.
    for (const lldpPeers of [[CRS], []]) {
      const q = at(build(cluster([node("cell1", { lldpPeers })]), view({ 5: { kind: "node", node: "cell1" } })), 5);
      expect(q).toMatchObject({ light: "unknown", speedText: null, speedFrom: null });
      expect(q.notes).toEqual(["cell1 doesn't hear this switch over LLDP, so nothing shows its link is on this port."]);
    }
  });

  it("won't colour from a node when several devices are heard through the port", () => {
    const p = at(build(real(), view({ 2: { kind: "node", node: "cell1" } })), 2);
    expect(p).toMatchObject({ light: "unknown", speedText: null, linkWord: "Up", short: "cell1" });
    expect(p.notes).toEqual(["4 other devices are heard through this port, so cell1's link speed may be to something else."]);
  });

  it("won't colour from a node labelled on two ports", () => {
    const on2 = node("cell1", { lldpPeers: [SWITCH] });
    const ports = build(cluster([on2]), view({ 2: { kind: "node", node: "cell1" }, 4: { kind: "node", node: "cell1" } }));
    expect(at(ports, 2)).toMatchObject({ light: "unknown", speedText: null });
    expect(at(ports, 2).notes[0]).toBe("cell1 is labeled on more than one port, so SparkBox can't tell which one its link speed belongs to.");
    expect(at(build(cluster([on2]), view({ 2: { kind: "node", node: "cell1" } })), 2)).toMatchObject({ light: "1g", speedFrom: "node", notes: [] });
  });

  it("follows a labelled node: down is unlit, not reporting is neutral", () => {
    const quiet = { lldpPeers: [] };
    expect(at(build(cluster([node("cell1", { ...quiet, up: false })]), view({ 3: { kind: "node", node: "cell1" } })), 3)).toMatchObject({ light: "off", linkWord: "cell1's LAN link is down" });
    const alone = { lldpPeers: [SWITCH] };
    const stale = { ...node("cell1", alone), stale: true };
    const held = at(build(cluster([stale]), view({ 2: { kind: "node", node: "cell1" } })), 2);
    expect(held).toMatchObject({ light: "unknown", speedText: null, speedFrom: null });
    expect(held.notes).toEqual(["cell1 isn't reporting, so its link can't be read."]);
    const gone = at(build(cluster([node("cell1", quiet, "offline")]), view({ 3: { kind: "node", node: "cell1" } })), 3);
    expect(gone).toMatchObject({ light: "unknown", speedText: null });
    expect(gone.notes).toEqual(["cell1 isn't reporting, so its link can't be read."]);
    expect(at(build(cluster([]), view({ 3: { kind: "node", node: "cell9" } })), 3).notes).toEqual(["The node this label names isn't in the inventory any more."]);
  });

  it("gives other labels the neutral light and no speed", () => {
    const c = cluster([]);
    const ports = build(c, view({ 1: { kind: "gold" }, 2: { kind: "crs804" }, 3: { kind: "mac" }, 4: { kind: "text", text: "NAS" } }));
    expect([1, 2, 3, 4].map((n) => at(ports, n).short)).toEqual(["Gold", "CRS804", "This Mac", "NAS"]);
    expect(at(ports, 1).what).toContain("Firewalla Gold");
    expect([1, 2, 3, 4].every((n) => at(ports, n).light === "unknown" && !at(ports, n).speedText)).toBe(true);
  });

  it("leaves a port marked Empty unlit, unless something says otherwise", () => {
    expect(at(build(cluster([]), view({ 3: { kind: "empty" } })), 3)).toMatchObject({ light: "off", short: "Empty" });
    const heard = at(build(real(), view({ 2: { kind: "empty" } })), 2);
    expect(heard.light).toBe("unknown");
    expect(heard.notes).toEqual(["You marked this port empty, but LLDP hears the switch through it."]);
    const linked = at(build(cluster([]), view({ 3: { kind: "empty" } }, cloud([port({ port: 3, linkSpeedMbps: 1000 })]))), 3);
    expect(linked.notes).toEqual(["You marked this port empty, but Firewalla reports a link."]);
  });

  it("says when the cloud and LLDP disagree, and trusts what's heard now", () => {
    const p = at(build(real(), view({}, cloud([port({ port: 2, linkUp: false })]))), 2);
    expect(p).toMatchObject({ light: "unknown", linkWord: "Heard over LLDP" });
    expect(p.notes).toEqual(["Firewalla reported no link here, but LLDP hears the switch through this port now."]);
    const labelled = at(build(cluster([]), view({ 4: { kind: "text", text: "NAS" } }, cloud([port({ port: 4, linkUp: false })]))), 4);
    expect(labelled.notes).toEqual(["Firewalla reports no link here, but you labeled it NAS."]);
  });

  it("won't let a label turn a heard port dark when the cloud's no-link is older than what's heard", () => {
    const alone = node("gx10-0000", { lldpPeers: [{ ...SWITCH, port: "Gi0/5", portId: "Gi0/5" }] });
    const noLink = cloud([port({ port: 5, linkUp: false })]);
    const bare = at(build(cluster([alone]), view({}, noLink)), 5);
    const labeled = at(build(cluster([alone]), view({ 5: { kind: "node", node: "gx10-0000" } }, noLink)), 5);
    for (const p of [bare, labeled]) expect(p.light).toBe("unknown");
    // Lit, but its NIC's speed can't colour a port the switch says has no link.
    expect(labeled).toMatchObject({ speedFrom: null, speedText: null, linkWord: "Up" });
    expect(labeled.notes).toEqual(["Firewalla reported no link here, but LLDP hears the switch through this port now."]);
    const text = at(build(real(), view({ 2: { kind: "text", text: "Hub" } }, cloud([port({ port: 2, linkUp: false })]))), 2);
    expect(text).toMatchObject({ light: "unknown", speedText: null });
  });

  it("still lets the cloud's no-link darken a labeled port nothing is heard on", () => {
    const off = cluster([node("cell1", { lldpPeers: [SWITCH] })]);
    const p = at(build(off, view({ 5: { kind: "node", node: "cell1" } }, cloud([port({ port: 5, linkUp: false })]))), 5);
    expect(p).toMatchObject({ light: "off", speedFrom: null, linkWord: "No link" });
    expect(p.notes).toContain("Firewalla reports no link here, but you labeled it cell1.");
  });

  it("keeps a node alone on its port coloured, and lets a real cloud answer outrank it", () => {
    const alone = node("gx10-0000", { lldpPeers: [{ ...SWITCH, port: "Gi0/5", portId: "Gi0/5" }] });
    const labels = { 5: { kind: "node", node: "gx10-0000" } } satisfies Record<string, PortLabel>;
    expect(at(build(cluster([alone]), view(labels)), 5)).toMatchObject({ light: "1g", speedFrom: "node" });
    expect(at(build(cluster([alone]), view(labels, cloud([port({ port: 5, linkSpeedMbps: 2500 })]))), 5)).toMatchObject({ light: "2.5g", speedFrom: "firewalla" });
  });
});

describe("the details", () => {
  it("counts a node's flaps as the LAN table does", () => {
    expect(flaps({ carrierChanges: 1 } as NetStats)).toBe(0);
    expect(flaps({ carrierChanges: 3 } as NetStats)).toBe(1);
    expect(flaps({} as NetStats)).toBeNull();
  });

  it("words the cloud's state", () => {
    const f = () => "4 min ago";
    expect(cloudLine(undefined, f)).toBe("Firewalla cloud: not set up");
    expect(cloudLine(off, f)).toBe("Firewalla cloud: not set up");
    expect(cloudLine(cloud([]), f)).toBe("Firewalla cloud: read 4 min ago");
    expect(cloudLine({ ...off, state: "waiting", configured: true }, f)).toBe("Firewalla cloud: first read coming");
    expect(cloudLine({ ...cloud([]), state: "rate-limited" }, f)).toBe("Firewalla cloud: last read 4 min ago");
    expect(cloudLine({ ...off, state: "auth", configured: true }, f)).toBe("Firewalla cloud: no reading yet");
    expect(cloudLine({ ...cloud([]), held: true }, f)).toBe("Firewalla cloud: held in look-only mode, last read 4 min ago");
    expect(cloudLine({ ...off, configured: true, state: "waiting", held: true }, f)).toBe("Firewalla cloud: held in look-only mode");
    const gone = cloud([]);
    expect(cloudLine({ ...gone, sw: { ...gone.sw!, online: false } }, f)).toBe("Firewalla cloud: read 4 min ago, and it says this switch is offline");
  });
});
