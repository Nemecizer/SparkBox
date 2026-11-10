// Network backend for the browser preview: demo scenarios, switch probes and port
// actions, the VPN console (direct at home by default; mock.html?vpn=tunnel shows
// it connected from away), and the Connect wizard's trust step.
import crs804Profile from "../../assets/hardware/crs804.json";
import type { SwitchPort } from "../../lib/types";
import type { DoctorReport, FabricState, IfaceHealth, PortTables } from "../../lib/fabric";
import type { ScenarioInfo } from "../../lib/netApi";
import type { VpnLogEntry, VpnReach, VpnView } from "../../lib/types";
import type { MockHandlers } from "../../test/tauri";
import { baseSnapshot, snapshotAt } from "../mockBackend";

const hardwarePreview = typeof location !== "undefined" && new URLSearchParams(location.search).get("hardware") === "1";
function hardwareSnapshot() {
  const s = snapshotAt(0); s.demo = true;
  const fourth = s.nodes.find((n) => n.id === "gx10-0000"); if (fourth) fourth.name = "cell4";
  const sw = s.switches.find((x) => x.id === "crs804");
  if (sw) Object.assign(sw, { configured: true, board: "CRS804-4DDQ-hRM", status: "online", host: "192.168.50.2", version: "7.20.2", error: null,
    ports: [1, 2, 3, 4].map((c) => ({ name: `qsfp56-dd-${c}-1`, node: s.nodes[c-1]?.id ?? null, comment: s.nodes[c-1]?.name ?? "", running: true, disabled: false, speed: "200G", capacityMbps: 200000, rxBps: 0, txBps: 0, rxErrors: 0, txErrors: 0, rxDrops: 0, txDrops: 0, rxPause: 0, txPause: 0, linkDowns: 0, fcsErrors: 0, rxPowerDbm: -2, txPowerDbm: 0, moduleTempC: 30, laneRxPowerDbm: [-2,-2,-2,-2], mtu: 9216, mappedBy: "lldp" } as SwitchPort)) });
  return s;
}
const SCENARIOS: ScenarioInfo[] = [
  { id: "default", title: "Everyday cluster", description: "The usual world: Llama 3.3 70B served across cell1-cell3, Ollama on gx10-0000, and one failed service (nvidia-dgx-telemetry) on cell1.", periodS: 0, affects: [] },
  { id: "healthy", title: "All healthy", description: "Everything up and nothing failed, for a clean baseline.", periodS: 0, affects: [] },
  { id: "node-offline", title: "Node offline", description: "cell3 hangs for a minute (no SSH, no work), then recovers. Its fabric link stays up, but the Llama pipeline has a stage there, so it stalls until cell3 is back.", periodS: 120, affects: ["cell3"] },
  { id: "disk-filling", title: "Disk filling", description: "A runaway log fills gx10-0000's root disk to 97% over 10 minutes; it's cleaned up a minute later.", periodS: 780, affects: ["gx10-0000"] },
  { id: "thermal-throttle", title: "Thermal throttling", description: "cell1's GPU runs at 88-91 °C, its clock drops 30% (HW thermal slowdown, SW power cap), then it cools down.", periodS: 180, affects: ["cell1"] },
  { id: "oom-kill", title: "Out of memory", description: "Ollama on gx10-0000 loads a second model until memory reaches 97%; the kernel kills it (exit 137) and Docker restarts it.", periodS: 120, affects: ["gx10-0000"] },
  { id: "crash-loop", title: "Crash loop", description: "vllm-worker on cell2 keeps crashing and restarting every 15 seconds, so the model is down; after 4 minutes it stays up and the model reloads.", periodS: 300, affects: ["cell2"] },
  { id: "fabric-flap", title: "Fabric link flapping", description: "cell2's 200G link drops for 5 seconds out of every 20 for 2 minutes. Carrier changes, link-downs, receive errors and FCS errors climb; the model can't stay loaded.", periodS: 180, affects: ["cell2", "crs804"] },
  { id: "collector-stall", title: "Collector stalled", description: "cell1's numbers stop updating for 30 seconds (the connection is up but no data arrives), then resume.", periodS: 90, affects: ["cell1"] },
  { id: "optic-degraded", title: "Degraded optic", description: "Lane 3 on cell2's CRS804 port fades to -9.5 dBm (the others stay near -2) and FCS errors appear, until the module is reseated.", periodS: 240, affects: ["cell2", "crs804"] },
  { id: "switch-port-down", title: "Switch port down", description: "CRS804 port qsfp56-dd-3-1 goes down for a minute, taking cell3's fabric and the model with it.", periodS: 120, affects: ["cell3", "crs804"] },
  { id: "sudo-rejected", title: "Sudo password rejected", description: "Every action that needs sudo (services, power, updates) fails with \"sudo: incorrect password\", as if the saved password were out of date.", periodS: 0, affects: [] },
  { id: "slow-node", title: "Slow node", description: "cell2 answers in 1.8-2.4 seconds instead of a few milliseconds for 90 seconds, then recovers.", periodS: 120, affects: ["cell2"] },
  { id: "fabric-congestion", title: "Fabric congestion", description: "All-reduce bursts between cell1-cell3: ECN marks and CNPs climb as DCQCN paces senders. Nothing is dropped.", periodS: 120, affects: ["cell1", "cell2", "cell3", "crs804"] },
  { id: "roce-loss", title: "RoCE packet loss", description: "cell2's half 2 starts losing packets: out-of-sequence, ACK timeouts and CRC errors climb, and the link flaps once.", periodS: 240, affects: ["cell2", "crs804"] },
  { id: "pfc-storm", title: "PFC pause storm", description: "gx10-0000 sends 50,000 pause frames a second on its half 1 for 90 seconds, stalling the switch's priority 3 queue.", periodS: 180, affects: ["gx10-0000", "crs804"] },
  { id: "fabric-partition", title: "Fabric lost", description: "cell2 loses both fabric halves for two minutes while its LAN stays up: one incident, cell2 lost the fabric, with the model down; the node stays online.", periodS: 180, affects: ["cell2", "crs804"] },
  { id: "miscabled", title: "Cables swapped", description: "cell2 and cell3's fabric cables are in each other's CRS804 ports. Port comments still name the old nodes.", periodS: 0, affects: ["cell2", "cell3", "crs804"] },
  { id: "model-oom", title: "Model out of memory", description: "A new deployment runs out of GPU memory while loading its weights and exits with CUDA out of memory.", periodS: 0, affects: ["cell1"] },
  { id: "gated-401", title: "Gated model refused", description: "Hugging Face answers 401 for a gated repository: the token is missing or hasn't been granted access.", periodS: 0, affects: [] },
  { id: "ray-join-timeout", title: "Ray worker can't join", description: "A multi-node deployment waits for a Ray worker that never registers with the head, and gives up after 3 minutes.", periodS: 0, affects: ["cell2"] },
  { id: "copy-disk-full", title: "Copy fills a disk", description: "A fabric copy of model weights stops when the destination disk is full.", periodS: 0, affects: ["cell3"] },
  { id: "endpoint-slow", title: "Endpoint slow", description: "llama-3.3-70b answers, but time to first token climbs past 4 seconds and tokens per second halve.", periodS: 0, affects: ["cell1"] },
  { id: "endpoint-stall", title: "Endpoint stalled", description: "llama-3.3-70b still lists its model and holds 3 running requests, but no token comes out: the one-token check times out and it goes Down as stalled.", periodS: 0, affects: ["cell1", "cell2", "cell3"] },
  { id: "agent-offline", title: "Agent offline", description: "The SparkBox agent on cell2 stops sending heartbeats, so 24/7 monitoring of that node lapses.", periodS: 0, affects: ["cell2"] },
  { id: "paging-drift", title: "Agent settings out of date", description: "cell3's agent didn't take the last settings push (sudo refused), so it would page with old rules: 24/7 turns amber, Settings › Notifications offers Push again.", periodS: 0, affects: ["cell3"] },
  { id: "gpu-xid", title: "GPU Xid error", description: "gx10-0000's kernel log reports Xid 79 (the GPU has fallen off the bus): its readings stop, Ollama goes Down, and the alert stays open until the node restarts.", periodS: 0, affects: ["gx10-0000"] },
  { id: "mtu-mismatch", title: "MTU mismatch", description: "gx10-0000's half 2 is at MTU 1500 while the rest of the fabric is at 9000, so jumbo frames to it fail.", periodS: 0, affects: ["gx10-0000"] },
];

// ---- Fabric (src-tauri/src/fabric): a lossless fabric built from the base snapshot,
// or cell2's half 2 retransmitting with mock.html?scenario=roce-loss.
function fabricState(): FabricState {
  const lossy = scenario === "roce-loss";
  const ifaces: IfaceHealth[] = [];
  for (const n of baseSnapshot.nodes) {
    const fabric = n.nets.filter((x) => x.role === "fabric" && x.ip);
    for (const x of fabric) {
      const half = (x.ip?.startsWith("192.168.101.") ? 1 : 0) as 0 | 1;
      const bad = lossy && n.id === "cell2" && half === 1;
      const r = (v: number) => (bad ? v : 0);
      // mlnx_qos as the real nodes report it: trust pcp, no PFC priority (fabric/mod.rs verdict).
      const roce = (x as { roce?: { pfcPrios?: number[] | null; trust?: string | null; outOfSequence?: number | null } }).roce;
      const pfcOff = !bad && roce?.pfcPrios?.length === 0;
      ifaces.push({
        node: n.id, nodeName: n.name, iface: x.iface, half, up: x.up, speedMbps: x.speedMbps, mtu: x.mtu ?? null, ip: x.ip, rdmaDev: x.rdmaDev ?? null,
        rxBps: x.rxBps, txBps: x.txBps, rdmaRxBps: x.rdmaRxBps ?? null, rdmaTxBps: x.rdmaTxBps ?? null, rdmaShare: 0.98,
        switchPort: x.switchPort?.split("/")[1] ?? null, switchId: "crs804", mappedBy: "mac", cablingOk: true, expectedPort: null,
        verdict: bad
          ? { kind: "lossy", level: "crit", title: "RoCE is retransmitting", detail: "Out of sequence 112/s, implied naks 7/s, sequence errors 11/s; crc errors 17/s point at the optic or cable.", action: "Check the optic or cable between cell2's port 2 and switch port 1-2." }
          : pfcOff
            ? { kind: "notLossless", level: "info", title: "RoCE priority unprotected", detail: `PFC is off on ${n.name} half ${half + 1} (trust ${roce?.trust ?? "pcp"}, no PFC priority): RoCE resends what it loses.`, action: "Verify node DSCP trust and PFC priority 3 together with RoCE ToS 106 and switch DSCP 26 → priority 3 mapping. SparkBox changes no settings." }
            : { kind: "notLossless", level: "info", title: "PFC protection unknown", detail: "This recorded fixture has no per-priority traffic evidence.", action: null },
        counters: [
          { key: "outOfSequence", label: "Out of sequence", total: bad ? 41_220 : (roce?.outOfSequence ?? 0), perS: r(112), group: "loss" },
          { key: "packetSeqErr", label: "Sequence errors", total: bad ? 3_904 : 0, perS: r(11), group: "loss" },
          { key: "localAckTimeoutErr", label: "ACK timeouts", total: bad ? 812 : 0, perS: r(3), group: "loss" },
          { key: "rxCrcErrorsPhy", label: "CRC errors", total: bad ? 5_120 : 0, perS: r(17), group: "phy" },
          { key: "ecnMarked", label: "ECN marked", total: 44_210, perS: 2, group: "congestion" },
          { key: "cnpSent", label: "CNPs sent", total: 41_900, perS: 2, group: "congestion" },
          { key: "txPause", label: "Pause sent", total: 3_204, perS: 0, group: "pause" },
          { key: "rxPause", label: "Pause received", total: 2_811, perS: 0, group: "pause" },
          { key: "linkDowned", label: "Link downs", total: bad ? 2 : 1, perS: 0, group: "link" },
        ],
        lossLastHour: bad ? 9_402 : 0,
        lastLossMs: bad ? Date.now() - 1000 : null,
      });
    }
  }
  const worst = ifaces.find((i) => i.verdict.level === "crit");
  const off = ifaces.filter((i) => i.up && i.verdict.kind === "notLossless");
  const oos = off.map((i) => i.counters.find((c) => c.key === "outOfSequence")?.total ?? 0);
  const [lo, hi] = [Math.min(...oos), Math.max(...oos)];
  const pfcOff = !worst && off.length > 0;
  return {
    tsMs: Date.now(),
    verdict: worst
      ? { ...worst.verdict, title: `RoCE is retransmitting on ${worst.nodeName} half 2` }
      : pfcOff
        ? { ...off[0].verdict, title: "RoCE priority unprotected" }
        : { kind: "lossless", level: "ok", title: "Fabric healthy", detail: "No drops, retransmits or pause storms in the last 14 min.", action: null },
    summary: worst ? `RoCE is retransmitting on ${worst.nodeName} half 2` : pfcOff ? `Fabric up · PFC protection unverified · ${ifaces.filter((i) => i.up).length} fabric ports` : "Fabric up · RoCE priority unknown · switch evidence unverified",
    ifaces,
    transport: pfcOff ? [] : [{ name: "llama-70b", nodes: ["cell1", "cell2", "cell3"], rdmaPct: 98, level: "ok", message: "NCCL over RoCE: 98% of fabric bytes are RDMA", envIssues: [] }],
    cabling: [],
    baselineSet: true,
    // fabric/mod.rs pfc_advisory, from the same counters.
    advisory: pfcOff
      ? {
          kind: "pfcOff", title: "Fabric PFC needs verification.", short: "PFC protection unverified",
          body: `PFC is off on all ${off.length} fabric ports (trust pcp, no PFC priority). RoCE still works by resending lost packets: since they came up, the ports counted ${lo}–${hi} out-of-sequence packets each. Check node DSCP trust, PFC priority 3, RoCE ToS 106 and switch DSCP 26 → priority 3 mapping together. SparkBox changes no settings.`,
        }
      : null,
  };
}

function switchTables(): PortTables {
  const hosts = baseSnapshot.nodes.flatMap((n) =>
    n.nets.filter((x) => x.role === "fabric" && x.ip && x.mac).map((x) => ({ mac: x.mac!.toUpperCase(), port: x.switchPort?.split("/")[1] ?? "", dynamic: true, local: false })),
  );
  return {
    hosts: [...hosts, { mac: "20:6d:31:8b:2f:b4", port: "ether1", dynamic: true, local: false }],
    neighbors: baseSnapshot.nodes.map((n, i) => ({ port: `qsfp56-dd-${i + 1}-1`, identity: n.system.hostname, mac: null, address: `192.168.100.${i + 1}`, platform: "Linux 6.11.0-1016-nvidia", discoveredBy: "lldp" })),
    addresses: [["192.168.50.2/24", "bridge"]],
    arp: [["192.168.50.1", "20:6d:31:c4:59:8c", "bridge"]],
  };
}

function doctorReport(): DoctorReport {
  const nodes = ["cell1", "cell2", "cell3"];
  const pairs = [["cell1", "cell2"], ["cell1", "cell3"], ["cell2", "cell3"]];
  return {
    taskId: null, startedMs: Date.now() - 95_000, finishedMs: Date.now() - 5_000, demo: true, nodes,
    checks: { mtu: true, ping: true, bandwidth: true, latency: true, nccl: false }, expectedGbps: 92,
    prereqs: [], findings: [], baselineSaved: false, cancelled: false, current: null, nccl: null,
    mtu: nodes.flatMap((n) => [0, 1].map((h) => ({ node: n, iface: h ? "enP2p1s0f1np1" : "enp1s0f1np1", half: h as 0 | 1, mtu: 9000, switchPort: "qsfp56-dd-1-1", switchMtu: 9216, status: "pass" as const, message: "9000" }))),
    ping: nodes.flatMap((a) => nodes.filter((b) => b !== a).flatMap((b) => [0, 1].map((h) => ({ from: a, to: b, half: h as 0 | 1, status: "pass" as const, result: "ok", rttMs: 0.11, detail: null })))),
    bandwidth: pairs.flatMap(([a, b]) => [0, 1].map((h) => ({ from: a, to: b, half: h as 0 | 1, gbps: h ? 93.4 : 94.8, latUs: 2.2, expectedGbps: 92, baselineGbps: null, status: "pass" as const, message: `${h ? 93 : 95} Gb/s` }))),
  };
}

const MIN = 60_000;

function vpnView(): VpnView {
  const now = Date.now();
  const away = new URLSearchParams(location.search).get("vpn") === "tunnel";
  const nodes = baseSnapshot.nodes.map((n) => ({ id: n.id, name: n.name, ip: n.nets.find((x) => x.role === "lan")?.ip ?? n.host }));
  const reach: VpnReach[] = [
    ...nodes.map((n, i) => ({ id: n.id, name: n.name, addr: `${n.ip}:22`, via: away ? ("tunnel" as const) : ("direct" as const), ok: true, rttMs: away ? 31 + i * 2.4 : 0.6 + i * 0.3, error: null, checkedMs: now - 4_000 })),
    { id: "gold", name: "Firewalla Gold", addr: "192.168.50.1:22", via: away ? "tunnel" : "direct", ok: true, rttMs: away ? 29.2 : 0.4, error: null, checkedMs: now - 4_000 },
  ];
  const log: VpnLogEntry[] = away
    ? [
        { tsMs: now - 134 * MIN, kind: "network", state: null, text: "Network changed to Wi-Fi Blue Bottle · gateway 10.20.0.1; checking the path again." },
        { tsMs: now - 134 * MIN + 1_400, kind: "state", state: "connecting", text: "Connecting: Connecting to the Firewalla…" },
        { tsMs: now - 134 * MIN + 1_450, kind: "helper", state: null, text: "awg: handshake initiated (keepalive 25 s, MTU 1380)" },
        { tsMs: now - 134 * MIN + 2_100, kind: "state", state: "connected", text: "Connected: Handshake with the Firewalla completed." },
        { tsMs: now - 41 * MIN, kind: "action", state: null, text: "Exposed ports changed." },
      ]
    : [
        { tsMs: now - 8 * 3600_000, kind: "state", state: "direct", text: "Direct: The cluster is reachable directly (cell1 answered in 1 ms): home network, or another VPN app is connected." },
        { tsMs: now - 3 * 3600_000, kind: "network", state: null, text: "The Mac woke from sleep; checking the path again." },
      ];
  const rates = Array.from({ length: 60 }, (_, i) => ({
    tsMs: now - (60 - i) * 2000,
    rxBps: away ? 180_000 + 120_000 * Math.abs(Math.sin(i / 5)) + (i % 17 === 0 ? 2_400_000 : 0) : 0,
    txBps: away ? 60_000 + 40_000 * Math.abs(Math.cos(i / 7)) : 0,
  }));
  return {
    settings: {
      mode: "auto",
      profile: {
        protocol: "amneziawg", endpointHost: "203.0.113.17", endpointPort: 51820, addresses: ["10.66.0.7/32"], dns: ["192.168.50.1"], mtu: 1380,
        peerPublicKey: "Jx2…demo", hasPresharedKey: true, allowedIps: ["192.168.50.0/24", "192.168.100.0/24", "192.168.101.0/24"], keepalive: 25,
        obfuscation: { jc: "4", jmin: "40", jmax: "70", s1: "17", s2: "33", h1: "1", h2: "2", h3: "3", h4: "4" }, importedAt: 1_792_675_200,
      },
      forwards: [
        { label: "open-webui on cell1", localPort: 3000, target: "192.168.50.51:3000", enabled: true },
        { label: "LLM API on cell1", localPort: 8000, target: "192.168.50.51:8000", enabled: true },
      ],
    },
    status: away
      ? {
          state: "connected", protocol: "amneziawg", endpoint: "203.0.113.17:51820", handshakeAgeSec: 12, rxBytes: 1_284_000_000, txBytes: 214_000_000,
          latencyMs: 31, message: null, forwards: [{ localPort: 3000, active: true, error: null }, { localPort: 8000, active: true, error: null }],
          upSinceMs: now - 134 * MIN, rxRateBps: 214_000, txRateBps: 71_000, override: null, network: "Wi-Fi Blue Bottle · gateway 10.20.0.1",
        }
      : {
          state: "direct", protocol: null, endpoint: null, handshakeAgeSec: null, rxBytes: 0, txBytes: 0, latencyMs: 0.6,
          message: "The cluster is reachable directly (cell1 answered in 1 ms): home network, or another VPN app is connected.",
          forwards: [], upSinceMs: now - 8 * 3600_000, rxRateBps: 0, txRateBps: 0, override: null, network: "Wi-Fi Home · gateway 192.168.50.1",
        },
    log,
    reach,
    rates,
  };
}

function probeHistory(switchId: string) {
  const now = Date.now();
  const gold = switchId === "gold";
  const samples = Array.from({ length: 60 }, (_, i) => ({ tsMs: now - (59 - i) * 5000, ms: i === 23 ? null : +((gold ? 0.38 : 0.55) + 0.4 * Math.abs(Math.sin(i * 1.7)) + (i % 19 === 0 ? 2.8 : 0)).toFixed(2) }));
  const start = now - (gold ? 19 : 31) * 3600_000;
  return {
    switchId,
    target: gold ? "192.168.50.1:443" : "192.168.50.3:443",
    samples,
    lossPct: 100 / 60,
    uptimePct: gold ? 99.72 : 99.86,
    lastOutage: { startMs: start, endMs: start + (gold ? 4 : 2) * MIN + 23_000, reason: gold ? "No answer to 48 probes in a row (rebooted after a Firewalla update)" : "No answer to 23 probes in a row" },
  };
}

let scenario: string | null = new URLSearchParams(location.search).get("scenario");

/** mock.html?scenario=optic-degraded&still=1 shows that scenario's effect on the switch (the Rust simulator plays them for real). */
function withScenario() {
  const s = snapshotAt(0);
  s.scenario = scenario;
  if (scenario === "optic-degraded") {
    const p = s.switches.find((x) => x.id === "crs804")?.ports.find((x) => x.name === "qsfp56-dd-1-2");
    if (p && p.laneRxPowerDbm) {
      p.laneRxPowerDbm[2] = -9.47;
      p.rxPowerDbm = -9.47;
      p.fcsErrors = (p.fcsErrors ?? 0) + 41;
      p.rxErrors += 41;
    }
  }
  return s;
}

export const handlers: MockHandlers = {
  ...(scenario ? { get_snapshot: withScenario } : {}),
  demo_scenarios_list: () => SCENARIOS,
  demo_scenario_set: ({ id }: { id: string | null }) => {
    scenario = id && id !== "default" ? id : null;
    return scenario;
  },
  persistence_warnings: () => [],
  vpn_get: () => vpnView(),
  vpn_override: () => vpnView(),
  vpn_set_mode: () => vpnView(),
  vpn_set_endpoint: () => vpnView(),
  vpn_set_forwards: () => vpnView(),
  vpn_import: () => vpnView(),
  vpn_remove: () => vpnView(),
  vpn_reconnect: () => null,
  switch_probe_history: ({ switchId }: { switchId: string }) => probeHistory(switchId),
  switch_port_set: ({ port, disabled }: { port: string; disabled: boolean }) => ({ port, disabled, revertAtMs: disabled ? Date.now() + 60_000 : null, keepAllowed: disabled && port !== "ether1", message: disabled ? `${port} disabled; it comes back in 60 s unless you keep it disabled` : `${port} enabled` }),
  switch_port_keep_disabled: ({ port }: { port: string }) => ({ port, disabled: true, revertAtMs: null, keepAllowed: false, message: `${port} stays disabled` }),
  switch_port_bounce: ({ port }: { port: string }) => ({ port, disabled: false, revertAtMs: null, keepAllowed: false, message: `${port} bounced; the switch brings it back in 3 s` }),
  switch_tables: () => switchTables(),
  firewalla_view: () => ({ labels: {}, msp: { configured: false, domain: null, state: "off", message: null, lastOkMs: null, lastTryMs: null, nextPollMs: null, sw: null } }),
  firewalla_label_set: () => null,
  firewalla_msp_settings: () => ({ domain: null }),
  firewalla_msp_set_domain: ({ domain }: { domain: string }) => ({ domain: domain || null }),
  firewalla_msp_test: () => ({ ok: true, kind: null, message: "Demo data: nothing was sent to Firewalla. Switch to Live data to test the real one." }),
  fabric_state: () => fabricState(),
  fabric_doctor_report: () => doctorReport(),
  fabric_doctor_start: () => "mock-task",
  fabric_baselines: () => ({}),
  fabric_baselines_reset: () => null,
  fabric_cabling_accept: () => 8,
  fabric_perftest_install: () => "mock-task",
  switch_port_comment: () => null,
  switch_port_reset: () => null,
  switch_port_blink: () => null,
  ...(hardwarePreview ? { get_snapshot: hardwareSnapshot } : {}),
  hardware_profiles: () => ({ enabled: false, demo: true, devices: hardwarePreview ? [{ id: "switch:crs804", name: "CRS804 DDQ", identity: "CRS804-4DDQ-hRM", builtin: true, entry: { identity: "CRS804-4DDQ-hRM", profile: crs804Profile, provider: "Bundled manufacturer specification", model: "", checkedAtMs: 0, error: null } }] : [] }),
  switch_trust: () => ({ ok: true, board: "CRS804-4DDQ", version: "7.20.2 (stable)", fingerprint: null, errorKind: null, message: "Connected to CRS804-4DDQ as sparkbox (RouterOS 7.20.2 (stable))." }),
  known_hosts_trust: () => null,
};
