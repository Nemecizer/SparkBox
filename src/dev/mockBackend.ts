// A stand-in for the Rust side, so the UI runs in any browser (mock.html) and in
// tests. It serves the cluster as SparkBox found it (fixtures/snapshot.json, a real
// Live snapshot of Oct 29) and re-emits it
// every second with believable jitter, keeps settings it is sent in memory, and
// answers the commands every window needs on open. Packages add their own
// commands in src/dev/mocks/<package>.ts.
import type { ActivityEntry, ClusterSnapshot, Inventory, NodeSnapshot, Prefs, ServerView, VpnView } from "../lib/types";
import { emitMock, type MockHandlers } from "../test/tauri";
import fixture from "./fixtures/snapshot.json";
import versionReport from "./fixtures/version-report.json";

const GiB = 1024 ** 3;

// Rust tests verify this comparison against the unchanged recorded fixture.
export const baseSnapshot = { ...fixture, versions: versionReport } as unknown as ClusterSnapshot;

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
/** A value that wanders around `base` by up to ±`frac`, smoothly from tick to tick. */
function wander(base: number, frac: number, phase: number, t: number) {
  return base * (1 + frac * (0.6 * Math.sin(t / 7 + phase) + 0.4 * Math.sin(t / 2.3 + phase * 3)));
}

let tick = 0;

/** mock.html?offline=cell3: that node stops answering SSH (the Rust Demo's node-offline fault phase). */
const offlineIds = typeof location === "undefined" ? [] : (new URLSearchParams(location.search).get("offline") ?? "").split(",").filter(Boolean);
const offlineSince = Date.now() - 75_000;

/** The fixture as it looks `t` seconds in: rates and loads move, identity doesn't. */
export function snapshotAt(t: number, now = Date.now()): ClusterSnapshot {
  const s = structuredClone(baseSnapshot);
  s.tsMs = now;
  // A scenario changed the evidence: do not keep a comparison of the original nodes.
  if (offlineIds.length) s.versions = null;
  for (const n of s.nodes) {
    if (!offlineIds.includes(n.id)) continue;
    n.status = "offline";
    n.error = `SSH timeout: no reply from ${n.host}:22 in 15 s`;
    n.lastSeenMs = offlineSince;
  }
  s.nodes.forEach((n: NodeSnapshot, i) => {
    if (n.status !== "online") return;
    const ph = i * 1.7;
    n.cpu.perCore = n.cpu.perCore.map((c, k) => +clamp(wander(c + 0.5, 0.35, ph + k, t), 0, 100).toFixed(1));
    n.cpu.totalPct = +(n.cpu.perCore.reduce((a, b) => a + b, 0) / n.cpu.perCore.length).toFixed(1);
    n.gpu.utilPct = +clamp(wander(n.gpu.utilPct, 0.12, ph, t), 0, 100).toFixed(1);
    n.gpu.powerW = +(18 + n.gpu.utilPct * 0.9 + Math.sin(t / 3 + ph)).toFixed(1);
    n.gpu.tempC = +(52 + n.gpu.utilPct / 5 + Math.sin(t / 11 + ph) * 0.6).toFixed(1);
    n.mem.used = Math.round(clamp(n.mem.used + Math.sin(t / 9 + ph) * 0.35 * GiB, 0, n.mem.total));
    n.mem.available = n.mem.total - n.mem.used;
    n.latencyMs = n.latencyMs === null ? null : +clamp(wander(n.latencyMs, 0.25, ph, t), 0.3, 9).toFixed(2);
    n.lastSeenMs = now - 200;
    for (const net of n.nets) {
      if (!net.up) continue;
      net.rxBps = Math.max(0, wander(net.rxBps, 0.08, ph + 0.4, t));
      net.txBps = Math.max(0, wander(net.txBps, 0.08, ph + 0.9, t));
      if (net.rdmaRxBps) net.rdmaRxBps = Math.max(0, wander(net.rdmaRxBps, 0.05, ph + 1.3, t));
      if (net.rdmaTxBps) net.rdmaTxBps = Math.max(0, wander(net.rdmaTxBps, 0.05, ph + 2.1, t));
    }
    for (const d of n.disks) {
      if (d.mount !== "/") continue;
      d.readBps = Math.max(0, wander(d.readBps, 0.6, ph, t));
      d.writeBps = Math.max(0, wander(d.writeBps, 0.6, ph + 1, t));
    }
    for (const c of n.containers) if (c.state === "running") c.cpuPct = +Math.max(0, wander(c.cpuPct, 0.1, ph, t)).toFixed(1);
    for (const p of n.processes) p.cpuPct = +Math.max(0, wander(p.cpuPct, 0.12, ph + p.pid, t)).toFixed(1);
  });
  for (const sw of s.switches) {
    sw.latencyMs = sw.latencyMs === null ? null : +clamp(wander(sw.latencyMs, 0.3, 4, t), 0.2, 9).toFixed(2);
    for (const p of sw.ports) {
      if (!p.running) continue;
      p.rxBps = Math.max(0, wander(p.rxBps, 0.05, 1, t));
      p.txBps = Math.max(0, wander(p.txBps, 0.05, 2, t));
    }
  }
  return s;
}

/** The default inventory (src-tauri/src/inventory.rs). */
export const inventoryFixture: Inventory = {
  nodes: [
    { id: "cell1", name: "cell1", host: "cell1-example.local", ip: "192.168.50.51", model: "MSI EdgeXpert", port: 22, user: "sparkuser" },
    { id: "cell2", name: "cell2", host: "cell2-example.local", ip: "192.168.50.154", model: "MSI EdgeXpert", port: 22, user: "sparkuser" },
    { id: "cell3", name: "cell3", host: "cell3-example.local", ip: "192.168.50.98", model: "MSI EdgeXpert", port: 22, user: "sparkuser" },
    { id: "gx10-0000", name: "gx10-0000", host: "gx10-0000.local", ip: "192.168.50.181", model: "ASUS Ascent GX10", port: 22, user: "sparkuser" },
  ],
  switches: [
    { id: "crs804", name: "CRS804 DDQ", kind: "routerOs", host: "", user: "sparkbox" },
    { id: "firewalla", name: "Firewalla Switch X", kind: "firewalla", host: "", user: null },
    { id: "gold", name: "Firewalla Gold", kind: "gateway", host: "", user: null },
  ],
};

export const prefsFixture: Prefs = {
  keepInMenuBar: true,
  dataSource: "demo",
  models: { modelRoot: "~/models", image: "nvcr.io/nvidia/vllm:26.05-py3", advisorProvider: "demo", advisorModel: "demo-advisor" },
};

const vpnFixture: VpnView = {
  settings: {
    mode: "auto",
    profile: {
      protocol: "amneziawg", endpointHost: "vpn.example.net", endpointPort: 51820, addresses: ["10.66.0.7/32"], dns: ["192.168.50.1"], mtu: 1380,
      peerPublicKey: "Jx2…demo", hasPresharedKey: true, allowedIps: ["192.168.50.0/24", "192.168.100.0/24", "192.168.101.0/24"], keepalive: 25,
      obfuscation: { Jc: "4", Jmin: "40", Jmax: "70" }, importedAt: 1792675200000,
    },
    forwards: [],
  },
  status: { state: "direct", protocol: null, endpoint: null, handshakeAgeSec: null, rxBytes: 0, txBytes: 0, latencyMs: null, message: "At home: the cluster is reachable directly.", forwards: [] },
};

const serverFixture: ServerView = {
  settings: { port: 8686, requireKey: true, autostart: false, aliases: {} },
  running: false,
  url: null,
  error: null,
  clients: [{ name: "laptop", createdAt: 1792675200000, preview: "sbx-…3f9a" }],
};

function activityFixture(now: number): ActivityEntry[] {
  const e = (ago: number, node: string, command: string, ms: number, changes = false, kind: ActivityEntry["kind"] = "command"): ActivityEntry =>
    ({ tsMs: now - ago * 1000, node, kind, command, code: 0, ms, changes, error: null });
  return [
    e(4, "cell1", "docker ps --all --no-trunc --format '{{json .}}'", 182),
    e(19, "cell3", "systemctl list-units --type=service --all --plain --no-legend", 94),
    e(64, "gx10-0000", "docker restart ollama", 2210, true),
    e(310, "cell1", "sudo -k -S -p '' systemctl restart nvidia-dgx-telemetry.service", 640, true, "sudo"),
    e(3600, "cell2", "bash -l", 0, false, "terminal"),
  ];
}

/** Handlers for the commands the shell and the shared windows call. Stateful per page load. */
export function defaultHandlers(demo = false): MockHandlers {
  let inventory = structuredClone(inventoryFixture);
  let prefs = structuredClone(prefsFixture);
  const secrets = new Set<string>(["ssh"]);
  return {
    get_snapshot: () => ({ ...snapshotAt(tick), demo }),
    get_inventory: () => structuredClone(inventory),
    save_inventory: ({ inventory: next }: { inventory: Inventory }) => {
      inventory = structuredClone(next);
      return null;
    },
    prefs_get: () => structuredClone(prefs),
    prefs_set: ({ prefs: next }: { prefs: Prefs }) => {
      prefs = structuredClone(next);
      return structuredClone(prefs);
    },
    prefs_patch: ({ patch }: { patch: Partial<Prefs> }) => {
      prefs = { ...prefs, ...structuredClone(patch), ...(patch.models ? { models: { ...prefs.models, ...patch.models } } : {}) };
      return structuredClone(prefs);
    },
    data_source_set: ({ source }: { source: Prefs["dataSource"] }) => {
      prefs = { ...prefs, dataSource: source };
      return structuredClone(prefs);
    },
    credential_status: ({ accounts }: { accounts: string[] }) => Object.fromEntries(accounts.map((a) => [a, secrets.has(a)])),
    set_credential: ({ account }: { account: string }) => {
      secrets.add(account);
      return null;
    },
    clear_credential: ({ account }: { account: string }) => {
      secrets.delete(account);
      return null;
    },
    vpn_get: () => structuredClone(vpnFixture),
    server_get: () => structuredClone(serverFixture),
    server_logs: () => [],
    activity_log: () => activityFixture(Date.now()),
    activity_log_path: () => "~/Library/Application Support/com.nemecj.sparkbox/activity.log",
    providers_list: () => [{ id: "demo", name: "Demo", kind: "demo", baseUrl: "", enabled: true, models: ["demo-chat"], defaultModel: "demo-chat", hasKey: false }],
    conversations_list: () => [],
    doctor_run: () => "~/Downloads/sparkbox-doctor-demo.txt",
    node_probe: ({ node }: { node: { host: string; ip: string | null; port: number; user: string } }) => [
      { id: "resolve", label: "Name lookup", status: "ok", detail: `${node.host} → ${node.ip ?? "192.168.50.51"}`, hint: null },
      { id: "tcp", label: "SSH port", status: "ok", detail: `Port ${node.port} answers in 1.2 ms`, hint: null },
      { id: "hostkey", label: "Host key", status: "ok", detail: "Matches the pinned ED25519 key", hint: null },
      { id: "auth", label: "Login", status: "ok", detail: `Signed in as ${node.user}`, hint: null },
      { id: "sudo", label: "sudo", status: "ok", detail: "Password accepted on standard input", hint: null },
      { id: "python", label: "Python", status: "ok", detail: "Python 3.12.3 for the collector", hint: null },
      { id: "docker", label: "Docker", status: "ok", detail: "Docker 28.3.2; user is in the docker group", hint: null },
      { id: "gpu", label: "GPU", status: "ok", detail: "NVIDIA GB10, driver 580.95.05, CUDA 13.0", hint: null },
      { id: "fabric", label: "Fabric", status: "warn", detail: "enp1s0f1np1 up at 200 Gb/s, MTU 9000; enP2p1s0f1np1 has 3 receive errors", hint: "Reseat the cable if the count keeps rising." },
    ],
    switch_test: ({ switchId }: { switchId: string }) =>
      switchId === "crs804"
        ? { ok: true, board: "CRS804-4DDQ", version: "7.20.2 (stable)", fingerprint: null, errorKind: null, message: "Signed in to the REST API." }
        : { ok: true, board: null, version: null, fingerprint: null, errorKind: null, message: "Answers ping in 1.1 ms." },
    known_hosts_forget: () => null,
    "plugin:autostart|is_enabled": () => false,
    "plugin:autostart|enable": () => null,
    "plugin:autostart|disable": () => null,
  };
}

let timer: number | undefined;

/** Emits cluster://snapshot once a second, like the Rust publisher. */
export function startTicker(periodMs = 1000, demo = false) {
  window.clearInterval(timer);
  timer = window.setInterval(() => {
    tick += periodMs / 1000;
    void emitMock("cluster://snapshot", { ...snapshotAt(tick), demo });
  }, periodMs);
}

export function stopTicker() {
  window.clearInterval(timer);
}
