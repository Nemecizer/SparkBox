import fleetPackages from "../../test/fixtures/protected-updates-fleet1025.json";
import continuationPackages from "../../test/fixtures/protected-updates-1025.json";
import protectedPackages from "../../test/fixtures/protected-updates.json";
import type { SystemUpdateRecord } from "../../windows/ProtectedUpdates";
let systemUpdate: SystemUpdateRecord | null = null;
// The safety core and node operations for the browser preview: Admin mode (no password,
// like Demo), tasks that run through their steps on timers, a grouped activity log, Fleet
// runs that stream per node, systemd services with a failed unit, and apt updates with
// driver drift on gx10-0000. Mirrors what the Rust Demo backend does.
import type { MockHandlers } from "../../test/tauri";
import { emitMock } from "../../test/tauri";
import type { ActivityEntry, AdminState, FleetResult, NodeUpdates, PendingPackage, SecuritySettings, ServiceDetail, Task } from "../../lib/opsApi";
import type { UpgradeRun } from "../../windows/UpdatesWindow";
import type { Service } from "../../lib/types";
import { baseSnapshot } from "../mockBackend";

const NODES = baseSnapshot.nodes.map((n) => n.id);
const nameOf = (id: string) => baseSnapshot.nodes.find((n) => n.id === id)?.name ?? id;
let seq = 0;
const id = (p: string) => `${p}${Date.now().toString(16)}${(++seq).toString(16)}`;

// ---- Admin mode

const security: SecuritySettings = { idleMinutes: 15, retentionDays: 90, shellIdleMinutes: 60 };
// mock.html?lookOnly=1 starts in look-only mode, as SPARKBOX_LOOK_ONLY=1 does the app.
const lookOnlyRun = typeof window !== "undefined" && /[?&]lookOnly=1\b/.test(window.location?.search ?? "");
let admin: AdminState = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: false, lockReason: null, lookOnly: lookOnlyRun, lookOnlyForced: lookOnlyRun };
const LOOK_ONLY_REFUSAL = { kind: "lookOnly", message: "SparkBox is in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date." };
function setAdmin(next: AdminState) {
  admin = next;
  void emitMock("admin://state", admin);
  return admin;
}

// ---- Tasks

const tasks = new Map<string, Task>();
const cancels = new Set<string>();

function newTask(kind: string, title: string, nodes: string[], cancellable = false): Task {
  const t: Task = {
    id: id("t"), kind, title, nodes, source: "ui", cancellable, state: "running", startedMs: Date.now(), finishedMs: null, progress: null,
    detail: null, rateBps: null, etaS: null, steps: [], log: [], summary: null, error: null, cancelRequested: false, retry: null, opId: null,
  };
  t.opId = t.id;
  tasks.set(t.id, t);
  void emitMock("tasks://update", t);
  return t;
}

function change(t: Task, f: (t: Task) => void) {
  f(t);
  void emitMock("tasks://update", structuredClone(t));
}

function step(t: Task, label: string) {
  change(t, (x) => {
    x.steps.forEach((s) => s.state === "running" && Object.assign(s, { state: "ok", finishedMs: Date.now() }));
    x.steps.push({ label, state: "running", startedMs: Date.now(), finishedMs: null, detail: null });
    x.detail = label;
  });
}

function finish(t: Task, ok: boolean, message: string) {
  change(t, (x) => {
    x.steps.forEach((s) => s.state === "running" && Object.assign(s, { state: ok ? "ok" : "failed", finishedMs: Date.now() }));
    x.state = cancels.has(x.id) ? "cancelled" : ok ? "ok" : "failed";
    x.finishedMs = Date.now();
    x.progress = ok ? 1 : x.progress;
    if (ok || x.state === "cancelled") x.summary = message;
    else x.error = message;
  });
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Runs labelled steps a little apart, then finishes. */
function playSteps(t: Task, steps: string[], gapMs: number, summary: string) {
  void (async () => {
    for (const [i, s] of steps.entries()) {
      if (cancels.has(t.id)) return finish(t, false, "Stopped");
      step(t, s);
      change(t, (x) => (x.progress = i / steps.length));
      await wait(gapMs);
    }
    finish(t, true, summary);
  })();
}

function seedTasks(now: number) {
  const done = (title: string, kind: string, nodes: string[], agoS: number, tookS: number, summary: string, labels: string[], error?: string, stepS?: number[]): Task => {
    const start = now - agoS * 1000;
    const at = (i: number) => start + (stepS ? stepS.slice(0, i).reduce((a, b) => a + b, 0) : i) * 1000;
    return {
      id: id("t"), kind, title, nodes, source: "ui", cancellable: false, state: error ? "failed" : "ok", startedMs: start, finishedMs: start + tookS * 1000,
      progress: 1, detail: null, rateBps: null, etaS: null, log: [], summary: error ? null : summary, error: error ?? null, cancelRequested: false, retry: null, opId: null,
      steps: labels.map((label, i) => ({ label, state: error && i === labels.length - 1 ? "failed" : "ok", startedMs: at(i), finishedMs: at(i + 1), detail: error && i === labels.length - 1 ? error : null })),
    };
  };
  for (const t of [
    done("Restart cell3", "power", ["cell3"], 2 * 3600, 134, "cell3 is back after 2m14s", ["Sending the restart to cell3", "Waiting for cell3 to go offline", "Waiting for cell3 to come back (up to 5 min)", "Checking that it restarted"], undefined, [1, 19, 112, 2]),
    done("Restart 3 containers", "container", ["cell1", "cell2", "cell3"], 50 * 60, 9, "3 of 3 restarted", ["Restart vllm-head on cell1", "Restart vllm-worker on cell2", "Restart vllm-worker on cell3"]),
    done("Restart nvidia-dgx-telemetry.service on cell1", "service", ["cell1"], 20 * 60, 2, "", ["Restart nvidia-dgx-telemetry.service on cell1"], "systemctl restart nvidia-dgx-telemetry.service failed on cell1: Job for nvidia-dgx-telemetry.service failed because the control process exited with error code."),
    done("Fleet: nvidia-smi --query-gpu=name,driver_version,utilization.gpu,te…", "fleet", NODES, 6 * 60, 2, "4 of 4 ok", ["Running on 4 nodes at once"]),
  ]) tasks.set(t.id, t);
}

// ---- Activity log

let activity: ActivityEntry[] = [];

function entry(over: Partial<ActivityEntry>): ActivityEntry {
  const ts = over.tsMs ?? Date.now();
  return {
    id: id("a"), tsMs: ts, updatedMs: ts, node: "cell1", kind: "command", command: "", code: 0, ms: 120, changes: true, error: null, opId: null,
    title: "", source: "ui", actor: "ui", escalated: false, state: "ok", stdoutTail: null, stderrTail: null, simulated: true, class: "change", ...over,
  };
}

function record(e: ActivityEntry) {
  activity.unshift(e);
  void emitMock("activity://entry", e);
}

function seedActivity(now: number) {
  const ago = (s: number) => now - s * 1000;
  const op = (title: string, s: number, kids: Partial<ActivityEntry>[], head: Partial<ActivityEntry> = {}) => {
    const opId = id("op");
    const out = [entry({ opId, kind: "op", title, node: [...new Set(kids.map((k) => k.node ?? "cell1"))].join(", "), tsMs: ago(s), ms: 9000, stdoutTail: head.stdoutTail ?? null, ...head })];
    kids.forEach((k, i) => out.push(entry({ opId, title, tsMs: ago(s) + 200 + i * 900, ...k })));
    return out;
  };
  activity = [
    entry({ node: "mac", kind: "app", title: "Admin mode unlocked", command: "Demo data: no password needed · to restart cell3", class: "app", tsMs: ago(2 * 3600 + 30), simulated: false, ms: 0, code: null }),
    ...op("Restart cell3", 2 * 3600, [{ node: "cell3", kind: "sudo", command: "sudo systemctl reboot", escalated: true }], { command: "systemctl reboot", ms: 134_000, stdoutTail: "cell3 is back after 2m14s" }),
    ...op("Restart 3 containers", 50 * 60, [
      { node: "cell1", command: "docker restart 4f1c2a9e7b31", ms: 2100 },
      { node: "cell2", command: "docker restart 8a0d33e1c4f2", ms: 1900 },
      { node: "cell3", command: "docker restart c2b7e09f1d64", ms: 2000 },
    ]),
    entry({ node: "cell1", kind: "sudo", title: "Restart nvidia-dgx-telemetry.service on cell1", command: "sudo systemctl restart -- 'nvidia-dgx-telemetry.service'", escalated: true, state: "failed", code: 1, tsMs: ago(20 * 60), ms: 640,
      error: "systemctl restart nvidia-dgx-telemetry.service failed on cell1", stderrTail: "Job for nvidia-dgx-telemetry.service failed because the control process exited with error code.\nSee \"systemctl status nvidia-dgx-telemetry.service\" and \"journalctl -xeu nvidia-dgx-telemetry.service\" for details." }),
    ...op("Fleet: nvidia-smi --query-gpu=name,driver_version,utilization.gpu,temperature.gpu,power.draw --format=csv,noheader", 6 * 60,
      NODES.map((n, i) => ({ node: n, kind: "stream" as const, class: "read" as const, changes: false, command: "timeout --signal=TERM --kill-after=5s 60s sh -c 'nvidia-smi --query-gpu=name,driver_version,utilization.gpu,temperature.gpu,power.draw --format=csv,noheader'",
        stdoutTail: `NVIDIA GB10, ${n === "gx10-0000" ? "580.82.07" : "580.95.05"}, ${[71, 64, 66, 0][i]} %, ${[61, 58, 59, 41][i]}, ${[38.2, 35.1, 36.4, 11.9][i]} W` })),
      { class: "read", changes: false, command: "nvidia-smi --query-gpu=name,driver_version,utilization.gpu,temperature.gpu,power.draw --format=csv,noheader", stdoutTail: "4 of 4 ok", ms: 1400 }),
    entry({ node: "mac", kind: "app", title: "Password saved", command: "Hugging Face token", class: "app", tsMs: ago(4 * 60), simulated: false, ms: 0, code: null }),
    entry({ node: "cell2", kind: "command", command: "docker logs --tail 200 -f 8a0d33e1c4f2", class: "read", changes: false, tsMs: ago(90), ms: 0, code: null, simulated: false }),
  ].sort((a, b) => b.tsMs - a.tsMs);
}

// ---- Services

const SERVICES: Omit<Service, "active" | "sub">[] = [
  { unit: "containerd.service", description: "containerd container runtime", load: "loaded", enabled: "enabled" },
  { unit: "cron.service", description: "Regular background program processing daemon", load: "loaded", enabled: "enabled" },
  { unit: "dgx-dashboard.service", description: "NVIDIA DGX Dashboard", load: "loaded", enabled: "enabled" },
  { unit: "docker.service", description: "Docker Application Container Engine", load: "loaded", enabled: "enabled" },
  { unit: "nvidia-dgx-telemetry.service", description: "NVIDIA DGX telemetry", load: "loaded", enabled: "enabled" },
  { unit: "nvidia-persistenced.service", description: "NVIDIA Persistence Daemon", load: "loaded", enabled: "enabled" },
  { unit: "ollama.service", description: "Ollama Service", load: "loaded", enabled: "disabled" },
  { unit: "ssh.service", description: "OpenBSD Secure Shell server", load: "loaded", enabled: "enabled" },
  { unit: "systemd-resolved.service", description: "Network Name Resolution", load: "loaded", enabled: "enabled" },
  { unit: "unattended-upgrades.service", description: "Unattended Upgrades Shutdown", load: "loaded", enabled: "enabled" },
];
const services = new Map<string, Service[]>(NODES.map((n) => [n, SERVICES.map((s) => {
  const failed = n === "cell1" && s.unit === "nvidia-dgx-telemetry.service";
  const off = s.unit === "ollama.service" && n !== "gx10-0000";
  return { ...s, active: failed ? "failed" : off ? "inactive" : "active", sub: failed ? "failed" : off ? "dead" : "running" };
})]));

function detail(node: string, unit: string): ServiceDetail {
  const s = services.get(node)?.find((x) => x.unit === unit);
  const failed = s?.active === "failed";
  const t = (m: number) => new Date(Date.now() - m * 60_000);
  const hhmm = (d: Date) => d.toLocaleTimeString("en-GB", { hour12: false });
  const journal = failed
    ? [
      { time: "", tsMs: t(21).getTime(), priority: 6, text: `systemd[1]: Starting ${unit} - ${s?.description}...` },
      { time: "", tsMs: t(21).getTime() + 900, priority: 3, text: "nvidia-dgx-telemetry[2211]: telemetry: cannot open /var/lib/nvidia-dgx-telemetry/state.db: Permission denied" },
      { time: "", tsMs: t(21).getTime() + 1100, priority: 5, text: `systemd[1]: ${unit}: Main process exited, code=exited, status=1/FAILURE` },
      { time: "", tsMs: t(21).getTime() + 1200, priority: 3, text: `systemd[1]: ${unit}: Failed with result 'exit-code'.` },
      { time: "", tsMs: t(21).getTime() + 1300, priority: 3, text: `systemd[1]: Failed to start ${unit} - ${s?.description}.` },
    ]
    : [
      { time: "", tsMs: t(3000).getTime(), priority: 6, text: `systemd[1]: Started ${unit} - ${s?.description}.` },
      ...(unit === "docker.service" ? [{ time: "", tsMs: t(42).getTime(), priority: 4, text: "dockerd[1342]: level=warning msg=\"failed to get endpoint_count map for scope local\"" }] : []),
    ];
  return {
    unit, description: s?.description ?? "", loaded: `loaded (/usr/lib/systemd/system/${unit}; ${s?.enabled}; preset: enabled)`,
    active: failed ? "failed (Result: exit-code)" : s?.active === "active" ? "active (running)" : "inactive (dead)",
    activeSince: failed ? `${t(21).toDateString()} ${hhmm(t(21))}; 21min ago` : s?.active === "active" ? `${t(3000).toDateString()} ${hhmm(t(3000))}; 2 days ago` : null,
    mainPid: failed ? 2211 : s?.active === "active" ? 1342 : null,
    result: failed ? "exit-code · status 1/FAILURE" : null,
    memory: failed ? null : unit === "docker.service" ? "312.4M (peak: 1.1G)" : "18.2M",
    cpu: failed ? "41ms" : "4min 12.441s",
    tasks: failed ? null : "41",
    statusText: `${failed ? "×" : "●"} ${unit} - ${s?.description}\n     Loaded: loaded (/usr/lib/systemd/system/${unit}; ${s?.enabled}; preset: enabled)\n     Active: ${failed ? "failed (Result: exit-code)" : "active (running)"}`,
    journal,
  };
}

// ---- Updates

const pkg = (name: string, from: string, to: string, origin = "noble-updates", security = false): PendingPackage =>
  ({ name, from, to, origin, security, sensitive: /^(linux-|nvidia|libnvidia|cuda|docker|containerd|mlnx|doca)/.test(name), held: false });
const updates = new Map<string, NodeUpdates>(NODES.map((n, i) => {
  const base = [
    pkg("openssl", "3.0.13-0ubuntu3.5", "3.0.13-0ubuntu3.6", "noble-updates, noble-security", true),
    pkg("curl", "8.5.0-2ubuntu10.6", "8.5.0-2ubuntu10.7", "noble-updates, noble-security", true),
    pkg("tzdata", "2025b-0ubuntu0.24.04", "2025c-0ubuntu0.24.04"),
    pkg("python3.12", "3.12.3-1ubuntu0.8", "3.12.3-1ubuntu0.9", "noble-updates, noble-security", true),
  ].slice(0, 3 + (i % 2));
  if (n === "gx10-0000") {
    base.unshift(pkg("nvidia-driver-580-open", "580.82.07-0ubuntu1", "580.95.05-0ubuntu1", "nvidia"), pkg("libnvidia-compute-580", "580.82.07-0ubuntu1", "580.95.05-0ubuntu1", "nvidia"),
      pkg("linux-image-6.11.0-1016-nvidia", "6.11.0-1015.15", "6.11.0-1016.16"));
  }
  if (n === "cell2") base.unshift({ ...pkg("docker-ce", "5:28.3.2-1~ubuntu.24.04~noble", "5:28.5.1-1~ubuntu.24.04~noble", "docker"), held: true });
  return [n, {
    node: n, pending: base, rebootRequired: n === "cell3", rebootPackages: n === "cell3" ? ["linux-image-6.11.0-1016-nvidia"] : [], unattended: n !== "gx10-0000",
    held: n === "cell2" ? ["docker-ce"] : [], driver: n === "gx10-0000" ? "580.82.07 → 580.95.05" : null, kernel: n === "gx10-0000" ? "6.11.0-1015.15 → 6.11.0-1016.16" : null,
    checkedMs: Date.now() - (3 * 3600 + 120 * i) * 1000,
  }];
}));

const upgradeRuns = new Map<string, UpgradeRun>();

// ---- Fleet

const fleetRuns = new Map<string, FleetResult[]>();

function fleetReply(node: string, cmd: string): { code: number; stdout: string; stderr: string } {
  const i = NODES.indexOf(node);
  const c = cmd.trim();
  if (c.startsWith("nvidia-smi")) return { code: 0, stdout: `NVIDIA GB10, ${node === "gx10-0000" ? "580.82.07" : "580.95.05"}, ${[71, 64, 66, 0][i] ?? 0} %, ${[61, 58, 59, 41][i] ?? 40}, ${[38.2, 35.1, 36.4, 11.9][i] ?? 10} W\n`, stderr: "" };
  if (c.startsWith("uptime")) return { code: 0, stdout: ` 14:02:1${i} up ${[3, 3, 0, 9][i] ?? 1} days,  4:1${i},  1 user,  load average: 0.84, 0.91, 0.88\n`, stderr: "" };
  if (c.startsWith("uname")) return { code: 0, stdout: `Linux 6.11.0-${node === "gx10-0000" ? "1015" : "1016"}-nvidia aarch64\n`, stderr: "" };
  if (c.startsWith("hostname")) return { code: 0, stdout: `${node}\n`, stderr: "" };
  if (c.startsWith("free")) return { code: 0, stdout: "               total        used        free      shared  buff/cache   available\nMem:           119Gi        97Gi       3.1Gi       1.2Gi        19Gi        22Gi\nSwap:             0B          0B          0B\n", stderr: "" };
  if (c.startsWith("df")) return { code: 0, stdout: "Filesystem      Size  Used Avail Use% Mounted on\n/dev/nvme0n1p2  3.7T  1.1T  2.4T  32% /\n", stderr: "" };
  return { code: 127, stdout: "", stderr: `sh: 1: ${c.split(/\s+/)[0]}: not found\n` };
}

// A rough copy of src-tauri/src/ops/classify.rs for the preview: the real one splits the command
// into its parts; this one only knows the commands the preview's snippets and tests use.
/** A rough copy of classify.rs's "cuts access" patterns, for the preview (the app asks the backend). */
const CUTS_ACCESS: [RegExp, string][] = [
  [/\bsystemctl\s+(stop|disable|mask|kill)\b[^;&|]*\b(sshd?(\.service|\.socket)?|NetworkManager|systemd-networkd)\b/, "Stops $2 (systemctl $1): SparkBox and you may not get back in until it's started at the node's console."],
  [/\bip\s+(link|l)\s+(set|s)\b[^;&|]*\bdown\b/, "Takes a network interface down (ip link … down): the node may drop off the network, and SparkBox with it."],
  [/\b(iptables|ip6tables)\b[^;&|]*\s(-F|--flush|-X)\b/, "Flushes the firewall ($1 -F): with a DROP policy that locks everyone out, SparkBox included."],
  [/\bufw\s+(enable|reset|default\s+deny)/, "Changes the firewall (ufw $1): SSH may stop getting through."],
  [/\/proc\/sysrq-trigger/, "Writes to /proc/sysrq-trigger, which can crash, reboot or freeze the node at once."],
  [/\b(passwd|chpasswd)\b/, "Changes a password or locks an account ($1): SparkBox's saved password may stop working."],
];
const DESTRUCTIVE: [RegExp, string][] = [
  [/\bdd\b[^;&|]*\bof=(\/dev\/(sd|nvme|vd|md|dm-|mapper\/)\S+)/, "Writes straight to the disk device $1 (dd)."],
  [/\b(mkfs\S*|wipefs|blkdiscard|sgdisk|sfdisk|parted|fdisk)\b[^;&|]*(\/dev\/(sd|nvme)\S+)/, "Erases the disk device $2 ($1)."],
  [/\brm\s+-\w*[rR]\w*\s+(\/|~|\/home|\/etc|\/usr|\/var|\/boot)(\/?\*?)?(\s|$)/, "Deletes $1 and everything in it (rm -r)."],
  [/(^|[;&|]\s*|sudo\s+)(reboot|poweroff|halt|shutdown(?!\s+-c))\b/, "Takes the node down ($2)."],
  [/\bsystemctl\s+(reboot|poweroff|halt|kexec)\b/, "Takes the node down (systemctl $1)."],
  [/\bdocker\s+(system\s+prune|volume\s+(rm|prune))\b/, "Deletes Docker volumes or everything Docker isn't using."],
  [/>\s*\/dev\/(sd|nvme)\S*/, "Writes straight to a disk device."],
];
function classify(cmd: string): { class: "read" | "change" | "destructive"; escalates: boolean; reason: string | null; cutsAccess?: boolean; power?: "restart" | "off" | null } {
  const c = cmd.trim();
  const escalates = /(^|[;&|]\s*)(sudo|doas|pkexec)\s/.test(c);
  const fill = (why: string, m: RegExpMatchArray) => why.replace(/\$(\d)/g, (_x, i: string) => m[Number(i)] ?? "");
  for (const [re, why] of CUTS_ACCESS) {
    const m = c.match(re);
    if (m) return { class: "destructive", escalates, reason: fill(why, m), cutsAccess: true, power: null };
  }
  for (const [re, why] of DESTRUCTIVE) {
    const m = c.match(re);
    const power = /\b(reboot|shutdown -r|systemctl reboot)\b/.test(c) ? "restart" : /\b(poweroff|halt|shutdown|systemctl poweroff)\b/.test(c) ? "off" : null;
    if (m) return { class: "destructive", escalates, reason: fill(why, m), cutsAccess: false, power };
  }
  // awk system, sed w/e and friends write or run things: never read-only.
  if (/\bawk\b.*\bsystem\s*\(|\bsed\b.*(\bw\s|\/e\b|\de\s)|LD_PRELOAD=|\bss\s+-\w*K/.test(c)) return { class: "change", escalates, reason: null };
  const read = /^(uptime|uname|hostname|free|df|nvidia-smi( --query| -q)?|docker (ps|logs|inspect|images)|systemctl (status|list-)|journalctl(?! --vacuum)|cat |ls|apt list|ip -br|ss |for i in \/sys\/class\/net)/.test(c) && (!/[;&|>`]|\$\(/.test(c) || c.startsWith("for i in /sys/class/net"));
  return { class: read ? "read" : "change", escalates, reason: null };
}

// ---- Docker images: what the Live tour found (gx10-0000's two images unused since April).

const DAYS = 86_400_000;
function images() {
  const now = Date.now();
  const img = (node: string, repository: string, tag: string, size: number, createdDaysAgo: number, usedBy: string[], lastUsedDaysAgo: number | null, running = 0) => ({
    node, id: (repository + tag).split("").reduce((h, c) => ((h * 31 + c.charCodeAt(0)) >>> 0), 7).toString(16).padStart(12, "0").slice(0, 12),
    repository, tag, size, createdMs: now - createdDaysAgo * DAYS, dangling: repository === "<none>", usedBy, running, lastUsedMs: lastUsedDaysAgo === null ? null : now - lastUsedDaysAgo * DAYS,
  });
  const glm = (node: string) => img(node, "vllm/vllm-openai", "glm53-flash-arm64-cu130", 19_600_000_000, 8, ["glm53"], 2.4);
  const nodes = NODES.map((node) => {
    const list = [glm(node)];
    if (node === "cell3") list.push(img(node, "docker.io/traefik", "v3.6.1", 189_000_000, 30, ["workbench-proxy"], 10));
    if (node.startsWith("gx10")) {
      list.push(img(node, "nvcr.io/nvidia/vllm", "26.03.post1-py3", 21_400_000_000, 225, ["vigilant_hofstadter"], 186));
      list.push(img(node, "nvcr.io/nvidia/cuda", "13.0.1-devel-ubuntu24.04", 8_910_000_000, 270, ["adoring_northcutt", "jovial_elgamal", "gracious_babbage"], 186));
    }
    const total = list.reduce((s, i) => s + i.size, 0);
    return { node, images: list, total, reclaimable: list.filter((i) => i.lastUsedMs !== null && now - i.lastUsedMs > 30 * DAYS).reduce((s, i) => s + i.size, 0), error: null };
  });
  return nodes;
}

// ---- Handlers

const rdpFixed = new Set<string>();
function rdpDemo(node: string, profile: string, repaired = false) {
  if (repaired) rdpFixed.add(`${node}:${profile}`);
  repaired = repaired || rdpFixed.has(`${node}:${profile}`);
  return { node, address: `${baseSnapshot.nodes.find((n) => n.id === node)?.host ?? "demo"}:3390`, demo: true,
    mac: { label: "RDP from this Mac", ok: true, detail: "Simulated encrypted connection; desktop login not tested" },
    report: { port: 3390, user: "sparkuser", profile: profile === "xfce" ? "xfce" : "gnome", fingerprint: "a1".repeat(32), digest: "d".repeat(64),
      backup: repaired ? "/var/backups/sparkbox-rdp-demo" : null,
      checks: [{ label: "RDP negotiation on the node", ok: true, detail: "Simulated TLS 1.3" }, { label: "Desktop and NVIDIA software rendering", ok: repaired, detail: repaired ? "Ubuntu GNOME with a software display" : "The software display needs repair" }],
      changes: repaired ? [] : ["Configure the xrdp software display", "Restart xrdp and xrdp-sesman"], blockers: [] } };
}
export const handlers: MockHandlers = {
  rdp_check: ({ node, profile }: { node: string; profile: string }) => { if (!admin.unlocked || admin.lookOnly) throw new Error("Unlock Admin mode first"); return rdpDemo(node, profile); },
  rdp_repair: ({ node, profile, digest }: { node: string; profile: string; digest: string }) => { if (!admin.unlocked || admin.lookOnly || digest !== "d".repeat(64)) throw new Error("Check RDP again with Admin mode unlocked"); return rdpDemo(node, profile, true); },
  images_list: () => images(),
  look_only_set: ({ on }: { on: boolean }) => {
    if (!on && admin.lookOnlyForced) throw { kind: "failed", message: "SPARKBOX_LOOK_ONLY=1 set look-only mode for this run. Quit and start SparkBox without it." };
    return setAdmin({ ...admin, lookOnly: on, unlocked: on ? false : admin.unlocked });
  },
  admin_state: () => admin,
  admin_unlock: ({ scope }: { scope?: string }) => {
    if (admin.lookOnly) throw LOOK_ONLY_REFUSAL;
    return setAdmin({
    ...admin, unlocked: true, authenticated: scope === "mac" || !!admin.authenticated, expiresAtMs: Date.now() + security.idleMinutes * 60_000,
    unlockedAtMs: admin.unlocked ? admin.unlockedAtMs : Date.now(), lockReason: null,
    });
  },
  // The preview has no Touch ID: confirming always works (never in look-only mode).
  admin_authenticate: () => admin.lookOnly ? Promise.reject(LOOK_ONLY_REFUSAL) : setAdmin({ ...admin, unlocked: true, authenticated: true, expiresAtMs: Date.now() + security.idleMinutes * 60_000, unlockedAtMs: admin.unlocked ? admin.unlockedAtMs : Date.now(), lockReason: null }),
  admin_extend: () => {
    if (!admin.unlocked) throw { kind: "locked", message: "Admin mode is locked. Unlock it in the top bar to make changes." };
    return setAdmin({ ...admin, expiresAtMs: Date.now() + security.idleMinutes * 60_000 });
  },
  admin_lock: () => setAdmin({ ...admin, unlocked: false, authenticated: false, expiresAtMs: null, unlockedAtMs: null }),
  autostart_get: () => false,
  autostart_set: ({ enabled }: { enabled: boolean }) => enabled,
  tunnel_close: () => null,
  tunnels_list: () => [],
  container_reveal: ({ name }: { name: string }) => (name === "HF_TOKEN" ? "hf_AbCdEf0123456789AbCdEf0123456789ab" : ""),
  security_get: () => ({ ...security }),
  security_set: ({ settings }: { settings: SecuritySettings }) => {
    Object.assign(security, settings);
    setAdmin({ ...admin, idleMinutes: settings.idleMinutes });
    return { ...security };
  },

  tasks_list: () => {
    if (!tasks.size) seedTasks(Date.now());
    return [...tasks.values()].sort((a, b) => b.startedMs - a.startedMs);
  },
  task_get: ({ id: tid }: { id: string }) => tasks.get(tid) ?? null,
  task_cancel: ({ id: tid }: { id: string }) => {
    const t = tasks.get(tid);
    if (t?.cancellable) {
      cancels.add(tid);
      change(t, (x) => (x.cancelRequested = true));
    }
    return null;
  },

  activity_log: () => {
    if (!activity.length) seedActivity(Date.now());
    return activity;
  },
  activity_log_since: ({ tsMs }: { tsMs: number }) => activity.filter((e) => Math.max(e.updatedMs, e.tsMs) >= tsMs),
  activity_log_path: () => "~/Library/Application Support/com.nemecj.sparkbox/activity.jsonl",
  activity_export: ({ format }: { format: string }) => `~/Downloads/SparkBox-activity-2026-10-28-14-02-11.${format}`,
  diagnostics_export: () => "~/Downloads/SparkBox-diagnostics-2026-10-28-14-02-11.txt",

  ops_classify: ({ command }: { command: string }) => classify(command),
  fleet_start: ({ nodes, command, sudo, title: snippet }: { nodes: string[]; command: string; sudo: boolean; strategy: string; title?: string | null }) => {
    const t = newTask("fleet", `Fleet: ${snippet ?? command.slice(0, 60)}${sudo ? " (as root)" : ""}`, nodes, true);
    if (snippet) t.subtitle = command;
    step(t, `Running on ${nodes.length} nodes at once`);
    const results: FleetResult[] = [];
    for (const n of nodes) void emitMock("fleet://node", { runId: t.id, node: n, state: "queued", code: null, ms: null });
    void Promise.all(nodes.map(async (n, i) => {
      await wait(60);
      void emitMock("fleet://node", { runId: t.id, node: n, state: "running", code: null, ms: null });
      const started = Date.now();
      const r = fleetReply(n, command);
      await wait(150 + i * 170);
      if (r.stdout) void emitMock("fleet://output", { runId: t.id, node: n, stream: "stdout", chunk: r.stdout });
      if (r.stderr) void emitMock("fleet://output", { runId: t.id, node: n, stream: "stderr", chunk: r.stderr });
      const ms = Date.now() - started;
      const state = r.code === 0 ? "ok" : "failed";
      results.push({ node: n, code: r.code, stdout: r.stdout, stderr: r.stderr, ms, error: null, timedOut: false, truncated: false, state });
      void emitMock("fleet://node", { runId: t.id, node: n, state, code: r.code, ms });
      record(entry({ node: n, kind: "stream", title: t.title, opId: t.id, command: `timeout --signal=TERM --kill-after=5s 60s sh -c '${command}'`, code: r.code, state: r.code === 0 ? "ok" : "failed", stdoutTail: r.stdout || null, stderrTail: r.stderr || null, ...classify(command), class: classify(command).class, changes: classify(command).class === "change" }));
    })).then(() => {
      fleetRuns.set(t.id, results);
      const ok = results.filter((r) => r.state === "ok").length;
      finish(t, ok === results.length, `${ok} of ${results.length} ok`);
    });
    return t.id;
  },
  fleet_results: ({ runId }: { runId: string }) => fleetRuns.get(runId) ?? null,
  fleet_cancel: ({ runId }: { runId: string }) => {
    cancels.add(runId);
    return null;
  },

  services_list: ({ node }: { node: string }) => structuredClone(services.get(node) ?? []),
  service_status: ({ node, unit }: { node: string; unit: string }) => detail(node, unit).statusText,
  service_detail: ({ node, unit }: { node: string; unit: string }) => detail(node, unit),
  service_impact: ({ nodes, unit, action }: { nodes: string[]; unit: string; action: string }) => {
    const on = nodes.map(nameOf).join(", ");
    const ssh = unit.startsWith("ssh");
    const high = (ssh && ["stop", "disable", "mask"].includes(action)) || action === "mask" || action === "unmask";
    const lines = ssh && action === "stop" ? nodes.map((n) => `SparkBox and new SSH sessions can't reach ${nameOf(n)} until it's started from a console.`)
      : unit === "docker.service" && action !== "start" ? ["2 running containers on cell1 stop: vllm-head, open-webui.", "Llama-3.3-70B-Instruct (llama-70b on cell1, cell2, cell3) is unavailable until its containers are back."]
        : action === "start" || action === "enable" || action === "reload" || action === "reset-failed" ? [] : [`${unit.replace(/\.service$/, "")} is briefly unavailable on ${on}.`];
    const tier = high || unit === "docker.service" ? "high" : lines.length ? "medium" : "low";
    return { tier, title: `${action[0].toUpperCase()}${action.slice(1)} ${unit.replace(/\.service$/, "")} on ${on}?`, lines, requireTyping: tier === "high" ? (nodes.length === 1 ? nameOf(nodes[0]) : unit) : null, blocked: null, confirmLabel: action[0].toUpperCase() + action.slice(1) };
  },
  service_action: ({ node, unit, action }: { node: string; unit: string; action: string }) => {
    const s = services.get(node)?.find((x) => x.unit === unit);
    if (!s) throw new Error(`Unit ${unit} not found.`);
    if (action === "start" || action === "restart") {
      if (node === "cell1" && unit === "nvidia-dgx-telemetry.service") throw { kind: "failed", message: `systemctl ${action} ${unit} failed on cell1: Job for ${unit} failed because the control process exited with error code.` };
      Object.assign(s, { active: "active", sub: "running" });
    }
    if (action === "stop") Object.assign(s, { active: "inactive", sub: "dead" });
    if (action === "enable") s.enabled = "enabled";
    if (action === "disable") s.enabled = "disabled";
    if (action === "reset-failed" && s.active === "failed") Object.assign(s, { active: "inactive", sub: "dead" });
    record(entry({ node, kind: "sudo", escalated: true, title: `${action} ${unit} on ${nameOf(node)}`, command: `sudo systemctl ${action} -- '${unit}'` }));
    return null;
  },
  services_apply: ({ nodes, unit, action }: { nodes: string[]; unit: string; action: string }) => {
    const t = newTask("service", `${action[0].toUpperCase()}${action.slice(1)} ${unit} on ${nodes.length} nodes`, nodes, true);
    playSteps(t, nodes.map((n) => `${action} ${unit} on ${nameOf(n)}`), 700, `Done on ${nodes.length} nodes`);
    return t.id;
  },

  container_action: ({ nodeId, action }: { nodeId: string; containerId: string; action: string }) => {
    record(entry({ node: nodeId, command: `docker ${action} 4f1c2a9e7b31`, title: `${action} a container on ${nameOf(nodeId)}` }));
    return null;
  },
  containers_bulk: ({ items, action, title }: { items: { node: string; name: string }[]; action: string; title: string | null }) => {
    const t = newTask("container", title ?? `${action} ${items.length} containers`, [...new Set(items.map((i) => i.node))], true);
    playSteps(t, items.map((i) => `${action[0].toUpperCase()}${action.slice(1)} ${i.name} on ${nameOf(i.node)}`), 900, `${items.length} of ${items.length} done`);
    return t.id;
  },
  container_inspect: ({ nodeId, containerId }: { nodeId: string; containerId: string }) => {
    const c = baseSnapshot.nodes.find((n) => n.id === nodeId)?.containers.find((x) => x.id === containerId);
    return {
      Id: `${containerId}${"0".repeat(52)}`, Name: `/${c?.name ?? "container"}`, Created: "2026-10-19T08:02:11.004Z", RestartCount: c?.restartCount ?? 0, SparkBoxDemo: true,
      State: { Status: c?.state ?? "running", ExitCode: c?.exitCode ?? 0, OOMKilled: !!c?.oomKilled, StartedAt: new Date((c?.startedAt ?? 0) * 1000).toISOString(), FinishedAt: "0001-01-01T00:00:00Z", Health: c?.health ? { Status: c.health } : undefined },
      // Masked like the backend's container_inspect; container_reveal shows one value.
      Config: { Image: c?.image, Cmd: (c?.command ?? "").split(" "), Env: ["NCCL_SOCKET_IFNAME=enp1s0f1np1", "VLLM_HOST_IP=192.168.100.1", "HF_TOKEN=‹redacted›", "HF_HOME=/models/.cache/huggingface"], Labels: c?.labels ?? {} },
      HostConfig: { NetworkMode: "host", IpcMode: "host", ShmSize: 17179869184, RestartPolicy: { Name: c?.restartPolicy ?? "no" }, DeviceRequests: [{ Capabilities: [["gpu"]] }] },
      Mounts: [{ Source: "/home/sparkuser/models", Destination: "/models", RW: false }],
      NetworkSettings: { Ports: {} },
    };
  },

  process_signal: ({ node, pid, signal }: { node: string; pid: number; signal: string }) => {
    record(entry({ node, command: `kill -s ${signal} ${pid}`, title: `Send SIG${signal} to process ${pid} on ${nameOf(node)}` }));
    return null;
  },
  process_renice: () => null,
  process_alive: () => false,
  ports_reveal: ({ node }: { node: string }) => (baseSnapshot.nodes.find((n) => n.id === node)?.listeners ?? []).map((l) => ({ ...l, process: l.process ?? (l.port === 22 ? "sshd" : l.port === 53 ? "systemd-resolve" : "root process"), pid: l.pid ?? (l.port === 22 ? 1201 : 812) })),

  node_power: ({ node, action }: { node: string; action: string }) => {
    const verb = action === "reboot" ? "Restart" : action === "wake" ? "Wake" : "Shut down";
    const t = newTask("power", `${verb} ${nameOf(node)}`, [node], action !== "wake");
    t.retry = { command: "node_power", args: { node, action } };
    const serves = ["cell1", "cell2", "cell3"].includes(node);
    playSteps(t, action === "reboot"
      ? [`Sending the restart to ${nameOf(node)}`, `Waiting for ${nameOf(node)} to go offline`, `Waiting for ${nameOf(node)} to come back (up to 5 min)`, "Checking that it restarted", ...(serves ? ["Waiting for llama-3.3-70b to answer…"] : [])]
      : action === "wake"
        ? [`Sending the wake packet to ${nameOf(node)}`, `Waiting for ${nameOf(node)} to come online (up to 5 min)`]
        : [`Sending the shutdown to ${nameOf(node)}`, `Waiting for ${nameOf(node)} to go offline`], 1800,
      action === "reboot" ? `${nameOf(node)} is back after 2m14s${serves ? ". llama-3.3-70b answered after 2m 10s" : ""}` : action === "wake" ? `${nameOf(node)} is on after 1m02s` : `${nameOf(node)} is off.`);
    return t.id;
  },
  node_wol_status: ({ node }: { node: string }) => ({ node, mac: "4c:bb:47:2e:91:0a", iface: "enP7s7", enabled: node !== "cell3", routes: [], fix: null, persist: null }),

  updates_status: ({ node }: { node: string }) => structuredClone(updates.get(node)),
  updates_summary: () => NODES.map((node) => ({ node, checkedMs: Date.now(), error: null, demo: true, apt: structuredClone(updates.get(node)), dgx: null, firmware: null, ollama: null, snaps: null, engineImages: null, dockerLiveRestore: null, stamps: { listsMs: null, dpkgMs: null, rebootRequiredMs: null }, securitySinceMs: null, refresh: null })),
  updates_system_status: () => systemUpdate,
  updates_system_preview: ({ node }: { node: string }) => {
    const active = systemUpdate && !["verified", "refused"].includes(systemUpdate.phase);
    if (active && (node !== "cell2" || systemUpdate!.node !== node || systemUpdate!.previous || systemUpdate!.phase !== "installing")) throw new Error("Finish the reserved node's verification first.");
    if (!["cell1", "cell2", "cell3", "gx10-0000"].includes(node)) throw new Error("No reviewed plan for this node.");
    const plan = active ? continuationPackages : node === "cell2" ? protectedPackages : fleetPackages;
    return { node, plan, upgrades: plan.filter((p) => !p.new).map((p) => p.name), installs: plan.filter((p) => p.new).map((p) => p.name), removals: [], heldBack: [], text: "Demo only", refused: null };
  },
  updates_system_start: ({ node, plan }: { node: string; plan: { name: string; to: string }[] }) => { const continuing = node === "cell2" && plan.length === 17; const previous = continuing && systemUpdate ? structuredClone(systemUpdate) : undefined; const completed = [...(systemUpdate?.completed ?? [])]; if (!continuing && systemUpdate?.phase === "verified") completed.push({ ...structuredClone(systemUpdate), completed: [] }); const t = newTask("updates", `Simulate system updates on ${nameOf(node)}`, [node]); systemUpdate = { node, demo: true, previous, completed, fresh1025: node !== "cell2", phase: "installing", message: "Demo packages simulated. Check system update when the task finishes.", task: t.id }; playSteps(t, ["Check reviewed package versions", "Simulate installation", "Simulate package and boot checks"], 400, "Demo system updates simulated; no real changes."); return t.id; },
  updates_system_verify: () => { if (!systemUpdate) throw new Error("No system update to verify."); systemUpdate = { ...systemUpdate, phase: "verified", message: "Demo verification simulated. No real packages, boot or hardware were checked." }; return systemUpdate; },
  updates_rolling_status: () => null,
  updates_rolling_preview: ({ nodes, canary, restart }: { nodes: string[]; canary: string | null; restart: string[] }) => ({
    id: "browser-preview", demo: true, instance: "dev", createdMs: Date.now(), phase: "preview", message: "", loads: [], paused: [],
    nodes: [...nodes].sort((a,b) => Number(b === (canary || "cell2")) - Number(a === (canary || "cell2"))).map((node) => {
      const pending = updates.get(node)?.pending.filter((p) => !p.held && !p.sensitive) ?? [];
      return { node, packages: pending.map((p) => p.name), versions: pending.map((p) => ({name:p.name,to:p.to})), restart:restart.includes(node), phase:"pending", child:null, admittedMs:0, bootBefore:null, kernel:null, driver:null };
    }),
  }),
  drop_cache_grant_get: () => null,
  updates_simulate: ({ node, packages }: { node: string; packages?: string[] }) => {
    const u = updates.get(node)!;
    const take = u.pending.filter((p) => !p.held && (!packages?.length || packages.includes(p.name)));
    const command = `apt-get -y --no-remove install --only-upgrade ${take.map((p) => `${p.name}=${p.to}`).join(" ")}`;
    return {
      node, plan: take.map((p) => ({ name: p.name, from: p.from, to: p.to })), command, dryRun: command.replace("apt-get ", "apt-get -s "), refused: take.some((p) => p.sensitive) ? "The plan would change the GPU stack" : null, installs: [], upgrades: take.map((p) => `${p.name} ${p.from} → ${p.to}`), removals: [],
      heldBack: u.pending.filter((p) => p.held).map((p) => p.name),
      text: ["NOTE: This is only a simulation!", ...take.map((p) => `Inst ${p.name} [${p.from}] (${p.to} Ubuntu:24.04/${p.origin.split(",")[0]} [arm64])`), `${take.length} upgraded, 0 newly installed, 0 to remove.`].join("\n"),
    };
  },
  updates_hold: ({ node, packages, hold }: { node: string; packages: string[]; hold: boolean }) => {
    const u = updates.get(node)!;
    for (const p of u.pending) if (packages.includes(p.name)) p.held = hold;
    u.held = u.pending.filter((p) => p.held).map((p) => p.name);
    return null;
  },
  updates_refresh: ({ nodes }: { nodes: string[] }) => {
    const t = newTask("updates", `Check for updates on ${nodes.length} nodes`, nodes);
    playSteps(t, nodes.map((n) => `apt-get update on ${nameOf(n)}`), 600, "Package lists are up to date");
    return t.id;
  },
  updates_progress: () => [...upgradeRuns.values()].map((run) => structuredClone(run)),
  updates_cancel: () => Promise.reject({ kind: "invalid", message: "This browser preview finishes its simulated upgrade automatically." }),
  updates_upgrade: ({ node, packages, opts }: { node: string; packages?: string[]; opts?: { plan?: { name: string; to: string }[]; securityOnly?: boolean } }) => {
    const u = updates.get(node)!;
    const take = u.pending.filter((p) => !p.held && !p.sensitive && (!packages?.length || packages.includes(p.name)) && (!opts?.securityOnly || p.security) && (!opts?.plan || opts.plan.some((v) => v.name === p.name && v.to === p.to)));
    const t = newTask("updates", `Upgrade ${nameOf(node)}`, [node]);
    const run: UpgradeRun = { node, taskId: t.id, title: t.title, unit: `sparkbox-upgrade-${t.id}`, logPath: `/var/log/sparkbox/upgrade-${t.id}.log`, packages: take.map((p) => p.name), phase: "unpacking", done: 0, total: take.length, current: null, log: ["Demo: installing the reviewed package plan"], cancellable: false, startedMs: Date.now(), exitCode: null, simulated: true };
    upgradeRuns.set(node, run);
    playSteps(t, ["Checking what will change", `Installing ${take.length} packages`, "Checking remaining packages and restart status"], 1200, `${take.length} packages installed on ${nameOf(node)}`);
    setTimeout(() => {
      u.pending = u.pending.filter((p) => !take.some((chosen) => chosen.name === p.name));
      run.phase = "done";
      run.done = take.length;
      run.exitCode = 0;
      run.verification = { checkedMs: Date.now(), remaining: structuredClone(u.pending) };
      run.log.push("Demo: package database and remaining packages checked");
    }, 3600);
    return t.id;
  },
};
