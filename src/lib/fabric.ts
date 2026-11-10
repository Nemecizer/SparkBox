// Fabric health (src-tauri/src/fabric): RoCE verdicts per interface, the NCCL
// transport check, cabling against its learned baseline, and the Fabric doctor.
import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./api";

/** lossless: nothing lost in the last hour. wasLossy: lost something earlier in the hour, not now. */
export type VerdictKind = "lossless" | "congested" | "lossy" | "phy" | "pauseStorm" | "dropping" | "notLossless" | "wasLossy" | "down" | "noData";
export type VerdictLevel = "ok" | "info" | "warn" | "crit";

export interface Verdict {
  kind: VerdictKind;
  level: VerdictLevel;
  /** "Lossless", "RoCE is retransmitting" */
  title: string;
  /** One sentence of evidence: "No drops, retransmits or pause storms in the last hour". */
  detail: string;
  /** What to do, when there's something to do. */
  action?: string | null;
}

/** One counter on one interface. */
export interface CounterReading {
  /** RoceCounters field, e.g. "outOfSequence"; pause counters are "txPause3". */
  key: string;
  label: string;
  total: number | null;
  /** Per second over the last 10 s; null until two readings exist. */
  perS: number | null;
  /** What the counter rising means. */
  group: "loss" | "phy" | "congestion" | "pause" | "drop" | "link";
}

export interface IfaceHealth {
  node: string;
  nodeName: string;
  iface: string;
  /** 0 is port 2 half 1 (192.168.100.x), 1 is half 2 (192.168.101.x). */
  half: 0 | 1;
  up: boolean;
  speedMbps: number;
  mtu: number | null;
  ip: string | null;
  rdmaDev: string | null;
  rxBps: number;
  txBps: number;
  rdmaRxBps: number | null;
  rdmaTxBps: number | null;
  /** RDMA share of this interface's bytes, 0–1; null when idle. */
  rdmaShare: number | null;
  /** "qsfp56-dd-1-2" on `switchId`. */
  switchPort: string | null;
  switchId: string | null;
  mappedBy: "mac" | "lldp" | "comment" | null;
  /** False when the cabling baseline expects a different port. */
  cablingOk: boolean | null;
  expectedPort: string | null;
  verdict: Verdict;
  counters: CounterReading[];
  /** Loss and PHY errors counted over the last hour. */
  lossLastHour: number;
  lastLossMs: number | null;
}

/** Is a multi-node deployment's NCCL traffic really on RDMA? */
export interface TransportCheck {
  /** The serving group, e.g. "llama-3.3-70b". */
  name: string;
  nodes: string[];
  /** RDMA share of the group's fabric bytes while it was generating, 0–100; null if it hasn't been busy. */
  rdmaPct: number | null;
  level: VerdictLevel;
  /** "NCCL over RoCE: 98% of fabric bytes are RDMA" */
  message: string;
  /** Environment problems: "NCCL_IB_DISABLE=1 on cell2". */
  envIssues: string[];
}

export interface CablingIssue {
  node: string;
  iface: string;
  half: 0 | 1;
  switchId: string;
  port: string;
  expectedPort: string;
  /** "cell2 half 1 is on 2-1; expected 1-2" */
  message: string;
}

export interface FabricState {
  tsMs: number;
  verdict: Verdict;
  /** "Fabric healthy · lossless · 200G per node · NCCL on RoCE" */
  summary: string;
  ifaces: IfaceHealth[];
  transport: TransportCheck[];
  cabling: CablingIssue[];
  /** A cabling baseline exists for every RouterOS switch. */
  baselineSet: boolean;
  // ── r4-alerts ──
  /** Known configuration worth knowing, never counted or paged: PFC off. */
  advisory?: Advisory | null;
}

/** A grey ⓘ banner: what it is, what it has cost (from the counters), how to change it. */
export interface Advisory {
  kind: "pfcOff" | string;
  /** "The fabric isn't lossless." */
  title: string;
  body: string;
  /** "PFC off" */
  short: string;
}

let state: FabricState | null = null;
let started = false;
const listeners = new Set<() => void>();

function publish(s: FabricState) {
  state = s;
  listeners.forEach((l) => l());
}

async function start() {
  if (started) return;
  started = true;
  if (inTauri) await listen<FabricState>("fabric://state", (e) => publish(e.payload));
  try {
    const s = await invoke<FabricState | null>("fabric_state");
    if (s && !state) publish(s);
  } catch {
    // An older core without the fabric monitor: no fabric health to show.
  }
}

/** The latest fabric health, or null until the first snapshot has been assessed. */
export function useFabricHealth(): FabricState | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      void start();
      return () => listeners.delete(l);
    },
    () => state,
  );
}

/** Calls `l` on every fabric update; returns the unsubscribe. */
export function subscribeFabric(l: () => void): () => void {
  listeners.add(l);
  void start();
  return () => listeners.delete(l);
}

/** The latest fabric health without subscribing (the sidebar re-reads it with every snapshot). */
export function peekFabricHealth(): FabricState | null {
  void start();
  return state;
}

/** "qsfp56-dd-2-1" → "2-1", the way the faceplate labels it. */
export function shortPort(name: string | null | undefined): string {
  if (!name) return "—";
  const m = /(\d+-\d+)$/.exec(name);
  return m ? m[1] : name;
}

export const halfLabel = (half: 0 | 1) => (half === 0 ? "Half 1" : "Half 2");

// ---- The Fabric doctor (src-tauri/src/fabric/doctor.rs)

export type DoctorStatus = "pass" | "warn" | "fail" | "skipped" | "running";

export interface DoctorChecks {
  mtu: boolean;
  ping: boolean;
  bandwidth: boolean;
  latency: boolean;
  nccl: boolean;
}

export interface DoctorRequest {
  nodes: string[];
  checks: DoctorChecks;
  /** Gb/s per half; 92 when not set. */
  expectedGbps?: number | null;
  allReducePerf?: string | null;
  mpirun?: string | null;
}

export interface Prereq {
  node: string;
  missing: string[];
  /** "perftest isn't installed on cell2" */
  message: string;
  package: string | null;
}

export interface MtuRow {
  node: string;
  iface: string;
  half: 0 | 1;
  mtu: number | null;
  switchPort: string | null;
  switchMtu: number | null;
  status: DoctorStatus;
  message: string;
}

export interface PingCell {
  from: string;
  to: string;
  half: 0 | 1;
  status: DoctorStatus;
  /** "ok", "fails: MTU" or "unreachable" */
  result: string;
  rttMs: number | null;
  detail: string | null;
}

export interface BwCell {
  from: string;
  to: string;
  half: 0 | 1;
  gbps: number | null;
  latUs: number | null;
  expectedGbps: number;
  baselineGbps: number | null;
  status: DoctorStatus;
  message: string;
}

export interface NcclResult {
  nodes: string[];
  busbwGbps: number | null;
  expectedGbps: number;
  status: DoctorStatus;
  message: string;
}

export interface DoctorReport {
  taskId: string | null;
  startedMs: number;
  finishedMs: number | null;
  demo: boolean;
  nodes: string[];
  checks: DoctorChecks;
  expectedGbps: number;
  prereqs: Prereq[];
  mtu: MtuRow[];
  ping: PingCell[];
  bandwidth: BwCell[];
  nccl: NcclResult | null;
  /** Worst first: "cell2→cell3 half 2: 61 Gb/s, 34% below its baseline of 93 Gb/s" */
  findings: string[];
  baselineSaved: boolean;
  cancelled: boolean;
  /** What's running now. */
  current: string | null;
}

export interface Baseline {
  gbps: number;
  latUs: number | null;
  tsMs: number;
}

/** What a disable, enable or bounce did (src-tauri/src/routeros/commands.rs). */
export interface PortChange {
  port: string;
  disabled: boolean;
  /** When the switch re-enables it by itself (Unix ms). */
  revertAtMs: number | null;
  /** "Keep disabled" is offered (never on the management path). */
  keepAllowed: boolean;
  message: string;
}

export interface HostEntry {
  mac: string;
  port: string;
  dynamic: boolean;
  local: boolean;
}

export interface Neighbor {
  port: string;
  identity: string;
  mac: string | null;
  address: string | null;
  platform: string | null;
  discoveredBy: string;
}

export interface PortTables {
  hosts: HostEntry[];
  neighbors: Neighbor[];
  addresses: [string, string][];
  arp: [string, string, string][];
}

export const fabricApi = {
  state: () => invoke<FabricState | null>("fabric_state"),
  /** Takes the cabling every switch shows now as the baseline (Mac scope: fabric_cabling.json). */
  acceptCabling: () => invoke<number>("fabric_cabling_accept"),
  doctorStart: (request: DoctorRequest) => invoke<string>("fabric_doctor_start", { request }),
  doctorReport: () => invoke<DoctorReport | null>("fabric_doctor_report"),
  baselines: () => invoke<Record<string, Baseline>>("fabric_baselines"),
  baselinesReset: () => invoke<void>("fabric_baselines_reset"),
  /** `sudo apt-get install -y perftest` on these nodes, as a task; resolves to its id. */
  perftestInstall: (nodes: string[]) => invoke<string>("fabric_perftest_install", { nodes }),
  /** A disable reverts on the switch after 60 s unless kept; the management port needs `confirm` ("disable 1-1"). */
  portSet: (switchId: string, port: string, disabled: boolean, confirm?: string) => invoke<PortChange>("switch_port_set", { switchId, port, disabled, confirm: confirm ?? null }),
  portKeepDisabled: (switchId: string, port: string) => invoke<PortChange>("switch_port_keep_disabled", { switchId, port }),
  /** One script on the switch: down 3 s, then up. The management port needs `confirm` ("bounce 1-1"). */
  portBounce: (switchId: string, port: string, confirm?: string) => invoke<PortChange>("switch_port_bounce", { switchId, port, confirm: confirm ?? null }),
  switchTables: (switchId: string) => invoke<PortTables>("switch_tables", { switchId }),
  onDoctor: (cb: (r: DoctorReport) => void) => listen<DoctorReport>("fabric://doctor", (e) => cb(e.payload)),
};

/** "disable 1-1": what must be typed to touch a management port. */
export const confirmPhrase = (verb: "disable" | "bounce", port: string) => `${verb} ${shortPort(port)}`;

/** What each counter means when it rises, for the ⓘ next to it. */
export const COUNTER_INFO: Record<string, string> = {
  outOfSequence: "Packets arrived out of order because earlier ones were lost, and were sent again from the gap. This is the clearest sign that the fabric is losing data.",
  packetSeqErr: "The receiver saw an unexpected packet, and the sender went back to send again.",
  localAckTimeoutErr: "This side waited for a reply that never came, then sent again. Each one holds up the traffic for a few milliseconds.",
  impliedNakSeqErr: "A reply skipped ahead, which implies a lost packet, so the request was tried again.",
  duplicateRequest: "The other side saw a request it had already answered, because the sender sent something again that wasn't really lost.",
  rnrNakRetryErr: "The receiving program wasn't ready for data, and the sender gave up waiting. This points at a program, not the fabric.",
  reqCqeErr: "Pieces of work that ended with an error on the sending side.",
  respCqeErr: "Pieces of work that ended with an error on the receiving side.",
  rxCrcErrorsPhy: "Data arrived damaged. It's almost always a dirty or failing optic, or a bad cable.",
  rxSymbolErrPhy: "The hardware couldn't decode the signal. It has the same usual causes as CRC errors.",
  symbolError: "The same signal trouble as Symbol errors (PHY), as the port counts it.",
  rxIcrcErr: "Packets on the fast path failed their integrity check, because they were damaged on the way.",
  portRcvErrors: "The port received packets with errors and dropped them.",
  rxDiscardsPhy: "The network card dropped packets because its memory for incoming data was full.",
  rxOutOfBuffer: "Packets were dropped because a program hadn't made room for them in time.",
  portXmitDiscards: "The port couldn't send these packets and dropped them, because the link was down or too busy.",
  ecnMarked: "The switch marked these packets to say a queue was building. A trickle under heavy traffic is normal, and it helps the fabric avoid losing data.",
  cnpSent: "Slow-down messages this node sent back to senders after it received marked packets.",
  cnpHandled: "Slow-down messages this node received and obeyed by sending more slowly.",
  cnpIgnored: "Slow-down messages this node received but didn't act on.",
  txPause: "This node asked the switch to stop sending for a moment. Bursts are normal. Tens of thousands a second for a long time is a pause storm.",
  rxPause: "The switch asked this node to stop sending for a moment.",
  linkDowned: "How many times the link has gone down since the node's network driver loaded.",
};

/** The meaning of a per-priority pause counter ("txPause3") or any other key. */
export function counterInfo(key: string): string {
  const m = /^(tx|rx)Pause(\d)$/.exec(key);
  if (m) return `${COUNTER_INFO[`${m[1]}Pause`]} This is priority ${m[2]}${m[2] === "3" ? ", the one the fast path uses" : ""}.`;
  return COUNTER_INFO[key] ?? "";
}
