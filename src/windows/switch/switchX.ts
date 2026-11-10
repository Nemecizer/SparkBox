// The Firewalla Switch X's panel, worked out from its three sources: LLDP as the nodes hear it,
// the owner's labels, and Firewalla's cloud. Everything here is a pure function of those, so the
// rules can be tested. The rules that matter:
//
// - A speed colour comes from Firewalla's linkSpeed, or from the NIC of a node with evidence for
//   this port: labelled here and on no other port, placed here by LLDP, and with nothing else
//   heard through it. Nothing else may colour a port.
// - All the nodes may hear the same switch port, through another switch or because the switch
//   sends a fixed id, so LLDP places what it hears on a LAN segment, never on a cable.
// - A node's NIC speed is its link to whatever is next to it, so with LLDP alone it's a remark.
import type { MspPort, MspState, MspStatus, PortLabel, PortsView } from "../../lib/firewalla";
import type { ClusterSnapshot, LldpNeighbor, NetStats, NodeSnapshot, SwitchSnapshot } from "../../lib/types";
import { lldpNode } from "../node/NetworkTab";

/** The front panel, from Firewalla's product photo: RJ45 ports 1 to 8, then SFP+ cages 9 to 12. */
export const RJ45_PORTS = 8;
export const SFP_PORTS = 4;
export const PORT_COUNT = RJ45_PORTS + SFP_PORTS;
/** The cloud's answer stops lighting ports when it's this old: a lockout can last for hours. */
export const MSP_STALE_MS = 3_600_000;
/** The cloud's answer says the last reading may not be this switch's, so it lights nothing. */
const UNTRUSTED: readonly MspState[] = ["not-listed", "shape", "not-json"];

export type Light = "10g" | "5g" | "2.5g" | "1g" | "100m" | "unknown" | "off";

export const LIGHT_WORD: Record<Light, string> = { "10g": "10G", "5g": "5G", "2.5g": "2.5G", "1g": "1G", "100m": "100M", unknown: "Connected, speed unknown", off: "No link or empty" };
/** The legend's order. */
export const LIGHTS: Light[] = ["10g", "5g", "2.5g", "1g", "100m", "unknown", "off"];

/** The speed the panel can colour, for a link speed in Mb/s. Any other value isn't one of the switch's. */
export function speedLight(mbps: number | null | undefined): Exclude<Light, "unknown" | "off"> | null {
  switch (mbps) {
    case 10_000: return "10g";
    case 5_000: return "5g";
    case 2_500: return "2.5g";
    case 1_000: return "1g";
    case 100: return "100m";
    default: return null;
  }
}

/** "1 Gb/s", "100 Mb/s", "2.5 Gb/s". */
export function fmtMbps(mbps: number): string {
  return mbps >= 1000 ? `${mbps / 1000} Gb/s` : `${mbps} Mb/s`;
}

/** A port number from an LLDP port id: Gi0/2, GigabitEthernet0/2, Te0/9. */
export function parseLldpPort(id: string | null | undefined): number | null {
  const m = /^\s*(?:gi|gigabitethernet|te|tengigabitethernet|xe|ge)\s*\d+\/(\d+)\s*$/i.exec(id ?? "");
  const n = m ? Number(m[1]) : NaN;
  return n >= 1 && n <= PORT_COUNT ? n : null;
}

const alnum = (s: string | null | undefined) => (s ?? "").replace(/[^a-z0-9]/gi, "").toLowerCase();

/** Is this LLDP neighbour the switch itself, by system name or by a chassis id known to be its? Another Firewalla switch isn't. */
export function isTheSwitch(nb: LldpNeighbor, sw: SwitchSnapshot, chassis: ReadonlySet<string>): boolean {
  const name = alnum(nb.system);
  return (!!name && (name === alnum(sw.name) || name === "firewallaswitchx")) || (!!nb.chassisId && chassis.has(nb.chassisId.toLowerCase()));
}

/** One thing heard on the LAN segment behind a port. */
export interface HeardDevice {
  key: string;
  name: string;
  /** Its port, as it sent it ("ether2"). Null for a node reporting for itself. */
  port: string | null;
  chassis: string | null;
  node: NodeSnapshot | null;
  /** The nodes that heard it, or that heard the switch through this port. */
  heardBy: string[];
}

/** What LLDP says about one port. */
export interface LldpPort {
  port: number;
  /** The port ids as the switch sent them: Gi0/2. */
  rawIds: string[];
  /** The nodes that hear the switch on this port. */
  reporters: NodeSnapshot[];
  heard: HeardDevice[];
  /** The up LAN link speeds of those nodes, in Mb/s. */
  linkMbps: number[];
  /** When the oldest of those reports was read. */
  readMs: number | null;
}

export interface LldpMap {
  ports: Map<number, LldpPort>;
  /** Port ids SparkBox couldn't place on the panel: "Gi0/99 (cell1)". */
  unplaced: string[];
  /** The MAC the switch sends, when it's been heard. */
  chassis: string | null;
}

const lanOf = (n: NodeSnapshot): NetStats | undefined => n.nets.find((x) => x.role === "lan");
const byName = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true });

/** Sorts what the nodes hear on the LAN into the switch ports they hear it through. */
export function lldpPorts(cluster: ClusterSnapshot, sw: SwitchSnapshot, mspMac?: string | null): LldpMap {
  const listening = cluster.nodes.filter((n) => n.status === "online" && lanOf(n)?.lldpPeers?.length);
  // A chassis the switch used anywhere is the switch everywhere, name or no name.
  const chassis = new Set<string>(mspMac ? [mspMac.toLowerCase()] : []);
  for (const n of listening) {
    for (const nb of lanOf(n)?.lldpPeers ?? []) if (nb.chassisId && isTheSwitch(nb, sw, chassis)) chassis.add(nb.chassisId.toLowerCase());
  }
  const ports = new Map<number, LldpPort>();
  const unplaced: string[] = [];
  for (const n of listening) {
    const nic = lanOf(n)!;
    const peers = nic.lldpPeers ?? [];
    for (const nb of peers.filter((p) => isTheSwitch(p, sw, chassis))) {
      const raw = nb.port ?? nb.portId ?? null;
      const number = parseLldpPort(nb.portId) ?? parseLldpPort(nb.port);
      if (number == null) {
        if (raw) unplaced.push(`${raw} (${n.name})`);
        continue;
      }
      const entry = ports.get(number) ?? { port: number, rawIds: [], reporters: [], heard: [], linkMbps: [], readMs: null };
      ports.set(number, entry);
      if (raw && !entry.rawIds.includes(raw)) entry.rawIds.push(raw);
      entry.reporters.push(n);
      if (nic.up && nic.speedMbps && !entry.linkMbps.includes(nic.speedMbps)) entry.linkMbps.push(nic.speedMbps);
      const read = n.lastSeenMs ?? cluster.tsMs;
      entry.readMs = entry.readMs == null ? read : Math.min(entry.readMs, read);
      const add = (key: string, device: Omit<HeardDevice, "key" | "heardBy">) => {
        const found = entry.heard.find((d) => d.key === key);
        if (found) {
          if (!found.heardBy.includes(n.name)) found.heardBy.push(n.name);
          found.port ??= device.port;
          found.chassis ??= device.chassis;
        } else {
          entry.heard.push({ key, ...device, heardBy: [n.name] });
        }
      };
      add(`node:${n.id}`, { name: n.name, port: null, chassis: nic.mac?.toLowerCase() ?? null, node: n });
      for (const other of peers.filter((p) => !isTheSwitch(p, sw, chassis))) {
        const node = lldpNode(other, cluster.nodes);
        if (node) add(`node:${node.id}`, { name: node.name, port: null, chassis: other.chassisId ?? null, node });
        else add(`dev:${other.chassisId ?? other.system}`, { name: other.system ?? other.chassisId ?? "Unnamed device", port: other.port ?? other.portId ?? null, chassis: other.chassisId ?? null, node: null });
      }
    }
  }
  for (const p of ports.values()) {
    p.heard.sort((a, b) => Number(!a.node) - Number(!b.node) || byName(a.name, b.name));
    for (const d of p.heard) d.heardBy.sort(byName);
    p.linkMbps.sort((a, b) => a - b);
  }
  return { ports, unplaced, chassis: chassis.values().next().value ?? null };
}

/** "The nodes behind it link at 1 Gb/s": a remark, never the port's own speed. */
export function behindText(l: LldpPort): string | null {
  if (!l.linkMbps.length) return null;
  const speeds = l.linkMbps.map(fmtMbps).join(" and ");
  return `${l.reporters.length > 1 ? "The nodes" : `${l.reporters[0].name}, the node`} behind it link${l.reporters.length > 1 ? "" : "s"} at ${speeds}`;
}

/** One port of the panel, with everything known about it. */
export interface XPort {
  n: number;
  kind: "rj45" | "sfp";
  light: Light;
  /** What's written under the port: only ever a speed the colour also says. */
  speedText: string | null;
  speedFrom: "firewalla" | "node" | null;
  /** Up, No link… in a word or two. */
  linkWord: string;
  /** The short line under the port. */
  short: string | null;
  /** The whole sentence, for a screen reader and the detail panel. */
  what: string;
  /** Things the sources disagree on. */
  notes: string[];
  lldp: LldpPort | null;
  label: PortLabel | null;
  /** The node the label names, and its LAN link. */
  node: NodeSnapshot | null;
  nic: NetStats | null;
  /** The cloud's report, however old. */
  msp: MspPort | null;
  /** The report is too old or can't be trusted, so it lights nothing. */
  mspUnused: boolean;
}

/** The label in words: the node's name, or what was typed. */
export function labelText(l: PortLabel, cluster: ClusterSnapshot): string {
  switch (l.kind) {
    case "node": return cluster.nodes.find((n) => n.id === l.node)?.name ?? l.node ?? "A node";
    case "crs804": return cluster.switches.find((s) => s.kind === "routerOs")?.name ?? "CRS804";
    case "gold": return cluster.switches.find((s) => s.kind === "gateway")?.name ?? "Firewalla Gold";
    case "mac": return "This Mac";
    case "text": return l.text ?? "";
    case "empty": return "Empty";
  }
}

const heardWord = (n: number) => `${n} heard`;

function onePort(n: number, cluster: ClusterSnapshot, placed: LldpMap["ports"], label: PortLabel | null, msp: MspPort | null, mspFresh: boolean, labelledOn: Map<string, number>): XPort {
  const lldp = placed.get(n) ?? null;
  const node = label?.kind === "node" ? cluster.nodes.find((x) => x.id === label.node) ?? null : null;
  const nic = node ? lanOf(node) ?? null : null;
  const reporting = node?.status === "online" && !node.stale && !!nic;
  const text = label ? labelText(label, cluster) : null;
  const named = label && label.kind !== "empty" ? text : null;
  const base: XPort = {
    n, kind: n <= RJ45_PORTS ? "rj45" : "sfp", light: "off", speedText: null, speedFrom: null, linkWord: "No link", short: null, what: "", notes: [],
    lldp, label, node, nic, msp, mspUnused: !!msp && !mspFresh,
  };
  const live = mspFresh ? msp : null;
  const notes: string[] = [];
  // The evidence for reading a node's NIC as this port's speed: LLDP puts it on this port, nothing
  // else is heard through the port, and no other port carries its label. A label alone isn't enough.
  const heardOn = node ? [...placed.values()].filter((p) => p.reporters.some((r) => r.id === node.id)).map((p) => p.port) : [];
  const here = heardOn.includes(n);
  const others = lldp ? lldp.heard.filter((d) => d.node?.id !== node?.id).length : 0;
  const twice = !!node && (labelledOn.get(node.id) ?? 0) > 1;
  const single = here && !others && !twice;
  if (twice) notes.push(`${node!.name} is labeled on more than one port, so SparkBox can't tell which one its link speed belongs to.`);
  if (node && others) notes.push(`${others} other device${others === 1 ? " is" : "s are"} heard through this port, so ${node.name}'s link speed may be to something else.`);
  if (node && reporting && !here) {
    notes.push(heardOn.length ? `LLDP places ${node.name} on port ${heardOn.join(" and ")}, not this one, so its link speed isn't used here. That doesn't prove the label is wrong.` : `${node.name} doesn't hear this switch over LLDP, so nothing shows its link is on this port.`);
  }
  if (label?.kind === "node" && !node) notes.push("The node this label names isn't in the inventory any more.");

  const setLight = (light: Light, linkWord: string, from: XPort["speedFrom"] = null, speedText: string | null = null) => {
    Object.assign(base, { light, linkWord, speedFrom: from, speedText });
  };
  if (live?.linkUp === true) {
    const l = speedLight(live.linkSpeedMbps);
    if (l) setLight(l, "Up", "firewalla", LIGHT_WORD[l]);
    else setLight("unknown", live.linkSpeedMbps ? `Up, ${fmtMbps(live.linkSpeedMbps)}` : "Up, speed not reported");
    if (label?.kind === "empty") notes.push("You marked this port empty, but Firewalla reports a link.");
  } else if (live?.linkUp === false && !lldp) {
    // What the switch says about its own port beats a label. Only LLDP heard now outranks it,
    // so a label can't turn a heard port dark.
    setLight("off", "No link");
    if (named) notes.push(`Firewalla reports no link here, but you labeled it ${named}.`);
  } else if (node) {
    if (live?.linkUp === false) notes.push("Firewalla reported no link here, but LLDP hears the switch through this port now.");
    if (!reporting) {
      setLight("unknown", "Not known");
      notes.push(`${node.name} isn't reporting, so its link can't be read.`);
    } else if (!nic!.up) {
      setLight("off", `${node.name}'s LAN link is down`);
    } else if (single && live?.linkUp !== false && speedLight(nic!.speedMbps)) {
      const l = speedLight(nic!.speedMbps)!;
      setLight(l, "Up", "node", LIGHT_WORD[l]);
    } else {
      setLight("unknown", "Up");
    }
  } else if (lldp) {
    setLight("unknown", "Heard over LLDP");
    if (live?.linkUp === false) notes.push("Firewalla reported no link here, but LLDP hears the switch through this port now.");
    if (label?.kind === "empty") notes.push("You marked this port empty, but LLDP hears the switch through it.");
  } else if (label && label.kind !== "empty") {
    setLight("unknown", "Not known");
  }

  const behind = lldp ? behindText(lldp) : null;
  // Under a port there's room for a short name.
  const brief = label?.kind === "crs804" ? "CRS804" : label?.kind === "gold" ? "Gold" : named;
  base.short = brief ?? (lldp ? heardWord(lldp.heard.length) : label?.kind === "empty" ? "Empty" : base.light !== "off" ? "Connected" : null);
  const name = `Port ${n}`;
  const speedWords = base.speedText ? `${base.speedText}, from ${base.speedFrom === "firewalla" ? "Firewalla" : `${node?.name}'s own link`}` : null;
  const parts = [base.light === "off" ? base.linkWord : speedWords ?? "connected, speed unknown", named, lldp ? `${lldp.heard.length} heard through it` : null].filter(Boolean);
  base.what = `${name}: ${parts.join(", ")}${behind && !speedWords ? `. ${behind}` : ""}`;
  base.notes = notes;
  return base;
}

/** Every port of the panel, from the snapshot and the view. Null `view` means it isn't loaded yet. */
export function buildPorts(cluster: ClusterSnapshot, sw: SwitchSnapshot, view: PortsView | null, nowMs: number): { ports: XPort[]; lldp: LldpMap } {
  const lldp = lldpPorts(cluster, sw, view?.msp.sw?.mac);
  const status = view?.msp;
  const fresh = !!status?.lastOkMs && nowMs - status.lastOkMs < MSP_STALE_MS && !UNTRUSTED.includes(status.state) && status.sw?.online !== false;
  const labels = view?.labels ?? {};
  const labelledOn = new Map<string, number>();
  for (const l of Object.values(labels)) if (l.kind === "node" && l.node) labelledOn.set(l.node, (labelledOn.get(l.node) ?? 0) + 1);
  const ports = Array.from({ length: PORT_COUNT }, (_, i) => {
    const n = i + 1;
    return onePort(n, cluster, lldp.ports, labels[n] ?? null, status?.sw?.ports.find((p) => p.port === n) ?? null, fresh, labelledOn);
  });
  return { ports, lldp };
}

/** The node's own flap count: carrier changes count down and up, and one boot-time link-up counts once. */
export function flaps(nic: NetStats): number | null {
  return nic.carrierChanges != null ? Math.max(0, Math.floor((nic.carrierChanges - 1) / 2)) : null;
}

/** The line under the legend: what the cloud layer is doing. */
export function cloudLine(status: MspStatus | undefined, nowFmt: (ms: number) => string): string {
  if (!status || status.state === "off") return "Firewalla cloud: not set up";
  if (status.held) return `Firewalla cloud: held in look-only mode${status.lastOkMs ? `, last read ${nowFmt(status.lastOkMs)}` : ""}`;
  if (status.sw?.online === false && status.lastOkMs) return `Firewalla cloud: read ${nowFmt(status.lastOkMs)}, and it says this switch is offline`;
  if (status.state === "ok" && status.lastOkMs) return `Firewalla cloud: read ${nowFmt(status.lastOkMs)}`;
  if (status.state === "waiting") return "Firewalla cloud: first read coming";
  return `Firewalla cloud: ${status.lastOkMs ? `last read ${nowFmt(status.lastOkMs)}` : "no reading yet"}`;
}
