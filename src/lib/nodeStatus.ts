// What a node's state means in words, one place for every window: why it isn't reporting, what
// planned power work it's in, its host key, the one action that would help, and whether its GPU
// is really throttled. The backend gives kinds (NodeSnapshot.errorKind); older backends and the
// Demo give only the error text, which is read as a fallback.
import { createElement as h } from "react";
import { confirmUnlock } from "./admin";
import { netApi } from "./netApi";
import { confirmAction } from "./session";
import { toast } from "./toasts";
import type { GpuStats, NodeErrorKind, NodeSnapshot } from "./types";
import { openWindow } from "../wm/WindowManager";

/** The command that prints a node's host key fingerprint, to compare before trusting it. */
export const COMPARE_KEY_CMD = "ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub";

/** The error kind, from the backend or else from the error text. */
export function errorKind(n: Pick<NodeSnapshot, "errorKind" | "error">): NodeErrorKind | null {
  if (n.errorKind) return n.errorKind;
  const e = (n.error ?? "").toLowerCase();
  if (!e) return null;
  if (/hasn't connected to .* before|not trusted yet/.test(e)) return "hostKeyUnknown";
  if (/host key .*changed|host key for .* changed/.test(e)) return "hostKeyChanged";
  if (/timed? ?out|timeout|didn't answer/.test(e)) return "timeout";
  if (/refused/.test(e)) return "refused";
  if (/no route|unreachable|host is down/.test(e)) return "unreachable";
  if (/not known|resolve|nodename/.test(e)) return "dns";
  if (/auth|permission denied|password|didn't accept/.test(e)) return "authFailed";
  return null;
}

/** Failures that say this Mac couldn't get to the node, not that the node turned SparkBox away. */
const REACH_KINDS: readonly NodeErrorKind[] = ["timeout", "unreachable", "dns", "refused"];

/**
 * One word for a node's link, the same in the sidebar, the status bar and Alerts (P1.10): a node
 * SparkBox can't reach is Offline, retrying included; Error only when it answered and turned
 * SparkBox away (a host key, a login) or failed in a way SparkBox can't name.
 */
export function linkWord(n: Pick<NodeSnapshot, "status" | "errorKind" | "error">): "Online" | "Connecting" | "Offline" | "Error" {
  if (n.status === "online") return "Online";
  const kind = errorKind(n);
  if (kind && REACH_KINDS.includes(kind)) return "Offline";
  if (kind) return "Error";
  if (n.status === "connecting") return "Connecting";
  return n.status === "error" ? "Error" : "Offline";
}

const KIND_WORDS: Record<NodeErrorKind, string> = {
  hostKeyUnknown: "Host key not trusted yet",
  hostKeyChanged: "Host key changed",
  authFailed: "Login failed",
  timeout: "No SSH reply",
  refused: "SSH refused the connection",
  unreachable: "Unreachable on the LAN",
  dns: "Hostname not found",
};

/** Planned power work in words: "Restarting", "Shutting down", "Off (planned)", "Waking"; null otherwise. */
export function powerLabel(n: Pick<NodeSnapshot, "powerState" | "maintenance" | "status">): string | null {
  switch (n.powerState) {
    case "rebooting":
      return "Restarting";
    case "shutting-down":
      return "Shutting down";
    case "off":
      return "Off (planned)";
    case "waking":
      return "Waking";
  }
  // Maintenance mode (P1.16): the owner's window, not a restart SparkBox started.
  if (n.maintenance?.reason === "in maintenance" && n.status !== "online") return "In maintenance";
  // A shutdown SparkBox started, after the power state was dropped (the app restarted).
  if (n.maintenance && n.status !== "online") return /shut/.test(n.maintenance.reason) ? "Off (planned)" : "Restarting";
  return null;
}

/** Why a node isn't reporting, short enough for a box: "No SSH reply", "Host key not trusted yet". Never an IP. */
export function offlineReason(n: NodeSnapshot): string | null {
  if (n.powerState === "off") return "Shut down from SparkBox";
  if (n.powerState === "rebooting") return "Restarting";
  if (n.powerState === "shutting-down") return "Shutting down";
  if (n.powerState === "waking") return "Waking";
  if (!n.error) return n.status === "connecting" ? "Connecting" : null;
  const kind = errorKind(n);
  if (kind) return KIND_WORDS[kind];
  // Anything else, without addresses: those belong in the tooltip, not cut in half on a box.
  return n.error.replace(/\b\d{1,3}(\.\d{1,3}){3}(:\d+)?\b/g, "").replace(/\s{2,}/g, " ").trim() || "No reply";
}

export interface HostKeyState {
  kind: "unknown" | "changed";
  fingerprint: string;
  /** host:port, as SparkBox pins it. */
  address: string;
  /** Other nodes that present (or are pinned to) the same key. */
  sharedWith: string[];
}

const FP = /SHA256:[A-Za-z0-9+/=]{20,}/g;

/** The host key a node is waiting on: first contact, or a key that changed. Null when it's trusted. */
export function hostKeyState(n: NodeSnapshot): HostKeyState | null {
  const kind = errorKind(n);
  if (kind !== "hostKeyUnknown" && kind !== "hostKeyChanged") return null;
  const shown = n.error ?? "";
  // "…changed (was A, now B)": the new key is the last one named.
  const fps = shown.match(FP) ?? [];
  const fingerprint = n.hostKey?.fingerprint ?? fps[fps.length - 1] ?? "";
  const address = n.hostKey?.address ?? shown.match(/\b(\d{1,3}(?:\.\d{1,3}){3}:\d+|[\w.-]+:\d+)\b/)?.[1] ?? `${n.host}:22`;
  if (!fingerprint) return null;
  return { kind: kind === "hostKeyUnknown" ? "unknown" : "changed", fingerprint, address, sharedWith: n.hostKey?.sharedWith ?? [] };
}

/** "SHA256:example…": a fingerprint short enough for a row; the full one goes in the tooltip. */
export function shortFingerprint(fp: string): string {
  const body = fp.replace(/^SHA256:/, "");
  return body.length > 10 ? `SHA256:${body.slice(0, 4)}…${body.slice(-3)}` : fp;
}

/** "cell1 and cell2", "cell1, cell2 and cell3". */
export function listNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** A host key to decide on: the node's name, where it's pinned, the key, and who shares it. */
export interface KeyToTrust {
  name: string;
  address: string;
  fingerprint: string;
  sharedWith: string[];
  changed?: boolean;
}

/**
 * Asks before pinning a node's host key: its address and fingerprint, how to compare it, and
 * whether other nodes present it too (a cloned image). Trust pins it (Mac scope); the node then
 * connects on its next try. Resolves to whether it was trusted.
 */
export async function trustKey(k: KeyToTrust, opts: { winId?: string } = {}): Promise<boolean> {
  const shared = k.sharedWith.length > 0
    ? `${listNames(k.sharedWith)} present${k.sharedWith.length === 1 ? "s" : ""} the same key (a cloned image). Trusting it for ${k.name} is your choice.`
    : null;
  const ok = await confirmAction({
    title: k.changed ? `${k.name}'s host key changed` : `Trust ${k.name}'s host key?`,
    body: h(
      "div",
      { className: "hostkey-sheet" },
      k.changed && h("p", null, "SparkBox pinned a different key for this node. If you reinstalled it, trust the new key; otherwise something may be intercepting the connection."),
      h("dl", { className: "hostkey-facts" },
        h("dt", null, "Address"), h("dd", null, h("code", null, k.address)),
        h("dt", null, "Key"), h("dd", null, h("code", null, k.fingerprint))),
      h("p", null, "Compare it on the node: ", h("code", null, COMPARE_KEY_CMD)),
      shared && h("p", null, shared),
      // ── p1r1-x ──
      h("p", null, "The shared cluster password, which is also the sudo password, will be sent to this machine on the next connection."),
      k.changed && h("p", null, `Type ${k.name} to trust the new key.`),
    ),
    confirmLabel: k.changed ? "Trust new key" : "Trust",
    cancelLabel: "Cancel",
    danger: !!k.changed,
    requireTyping: k.changed ? k.name : undefined,
    winId: opts.winId,
    dedupeKey: `trust-host-key:${k.address}`,
  });
  if (!ok) return false;
  // Pinning a key is Mac-side state: the unlock sheet, then Touch ID or the password.
  if (!(await confirmUnlock(`trust ${k.name}'s host key`, "mac", "known_hosts_trust"))) return false;
  try {
    await netApi.knownHostsTrust(k.address, k.fingerprint, k.changed ? k.name : undefined);
    toast({ kind: "success", title: `Trusted ${k.name}'s host key`, body: "SparkBox connects to it on its next try, within 30 s." });
    return true;
  } catch (e) {
    toast({ kind: "error", title: "Couldn't trust the host key", body: String((e as { message?: string })?.message ?? e) });
    return false;
  }
}

/** The Trust sheet for a node that's waiting on its host key. */
export async function trustHostKey(n: NodeSnapshot, opts: { winId?: string } = {}): Promise<boolean> {
  const k = hostKeyState(n);
  if (!k) return false;
  return trustKey({ name: n.name, address: k.address, fingerprint: k.fingerprint, sharedWith: k.sharedWith, changed: k.kind === "changed" }, opts);
}

/** Nodes in `nodes` (other than `name`) presenting `fingerprint`. */
export function sharingKey(nodes: NodeSnapshot[], fingerprint: string, name: string): string[] {
  return nodes.filter((n) => n.name !== name && n.hostKey?.fingerprint === fingerprint).map((n) => n.name);
}

export interface NodeAction {
  label: string;
  run: (opts?: { winId?: string }) => void;
}

/** The one thing that would get a node reporting again, when SparkBox can offer it. */
export function nodeActionFor(n: NodeSnapshot): NodeAction | null {
  if (n.status === "online" && !n.error) return null;
  const k = hostKeyState(n);
  if (k) return { label: k.kind === "changed" ? "Review host key…" : "Trust host key…", run: (o) => void trustHostKey(n, o) };
  if (errorKind(n) === "authFailed") return { label: "Open Cluster login", run: () => openWindow("settings", { section: "login" }) };
  return null;
}

/** A link a backend message points at in words ("…in Connect or Settings › Nodes"). */
export interface MessageLink {
  label: string;
  run: () => void;
}

/** The places an error message names, as buttons: Connect, Settings › Nodes, Cluster login. */
export function linksIn(message: string | null | undefined): MessageLink[] {
  const m = message ?? "";
  const out: MessageLink[] = [];
  if (/\bConnect\b(?! to)/.test(m)) out.push({ label: "Connect my Sparks…", run: () => openWindow("connect") });
  if (/Settings ›\s*Nodes/.test(m)) out.push({ label: "Open Settings › Nodes", run: () => openWindow("settings", { section: "nodes" }) });
  if (/Settings ›\s*Cluster login/.test(m)) out.push({ label: "Open Cluster login", run: () => openWindow("settings", { section: "login" }) });
  if (/Settings ›\s*Diagnostics/.test(m)) out.push({ label: "Open Diagnostics", run: () => openWindow("settings", { section: "diagnostics" }) });
  return out;
}

// ---- GPU throttle: the rule in src-tauri/src/health/fixtures/gpu-throttle-cases.json (health's
// gpu-throttle alert and the node agent read the same file; nodeStatus.test.ts checks this copy).

export const THROTTLE_RULE = {
  busyUtilPct: 50,
  clockBelow: 0.9,
  powerAt: 0.8,
  always: ["HW slowdown", "HW thermal slowdown", "HW power brake slowdown"],
  never: ["GPU idle", "Applications clocks setting", "Display clock setting"],
};
/** Below this utilisation, with no GPU process, the GPU has nothing to run. */
export const IDLE_UTIL_PCT = 5;

export interface ThrottleVerdict {
  throttled: boolean;
  /** "Throttled: SW power cap", "Idle (P8) · clocks are low because nothing is running", "Not throttled". */
  text: string;
  idle: boolean;
  /** The gpu-throttle alert's words, when it's throttled: "GPU throttled: SW power cap at 98% busy, 1.9 of 3.0 GHz". */
  alertText: string | null;
}

const ghz = (mhz: number) => (mhz / 1000).toFixed(1);

/**
 * Whether the GPU is really held back. Hardware slowdowns always count; the power cap, thermal
 * software slowdown and sync boost only while it's busy and below its top clock (or near a
 * reported power limit). An idle GB10 (P8, 4 W, 208 MHz) reports "SW power cap" on every other
 * reading: low clocks with nothing to run are not throttling.
 */
export function throttleVerdict(gpu: GpuStats): ThrottleVerdict {
  if (gpu.readable === false) return { throttled: false, text: "Not readable", idle: false, alertText: null };
  const r = THROTTLE_RULE;
  const reasons = (gpu.throttle ?? []).filter((x) => !r.never.includes(x));
  const util = gpu.utilPct ?? 0;
  const clockLow = !gpu.clockMaxMhz || gpu.smClockMhz < r.clockBelow * gpu.clockMaxMhz;
  const nearLimit = !!gpu.powerLimitW && gpu.powerW >= r.powerAt * gpu.powerLimitW;
  const busy = (util >= r.busyUtilPct && clockLow) || nearLimit;
  const counted = reasons.filter((x) => r.always.includes(x) || busy);
  const idle = util < IDLE_UTIL_PCT && (gpu.procs ?? []).length === 0;
  if (counted.length) {
    const clocks = gpu.clockMaxMhz ? `${ghz(gpu.smClockMhz)} of ${ghz(gpu.clockMaxMhz)} GHz` : `${ghz(gpu.smClockMhz)} GHz`;
    return { throttled: true, text: `Throttled: ${counted.join(", ")}`, idle, alertText: `GPU throttled: ${counted.join(", ")} at ${Math.round(util)}% busy, ${clocks}` };
  }
  if (idle) return { throttled: false, text: `Idle${gpu.pstate ? ` (${gpu.pstate})` : ""} · clocks are low because nothing is running`, idle: true, alertText: null };
  return { throttled: false, text: "Not throttled", idle: false, alertText: null };
}

// ---- The node Test's summary (Settings › Nodes, Connect)

export interface TestStep {
  id: string;
  status: "ok" | "warn" | "fail" | "skip";
  /** Skipped because an earlier step stopped the run (not a check that came back unknown). */
  blocked?: boolean;
  fingerprint?: string | null;
}

export interface TestSummary {
  text: string;
  tone: "ok" | "warn" | "fail" | "unfinished";
  /** The host key step that stopped the run, to trust from the summary. */
  trust?: TestStep;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** What a Test found, from its steps: never "everything works" with a check that failed or didn't run. */
export function testSummary(steps: TestStep[]): TestSummary {
  const failed = steps.filter((s) => s.status === "fail").length;
  const blocked = steps.filter((s) => s.status === "skip" && s.blocked !== false && (s.blocked || isBlockedLegacy(s))).length;
  const warned = steps.filter((s) => s.status === "warn" && !s.fingerprint).length;
  const key = steps.find((s) => s.id === "hostkey" && s.status === "warn" && s.fingerprint);
  if (key && blocked) return { text: `Not finished: trust the host key to run the other ${plural(blocked, "check")}.`, tone: "unfinished", trust: key };
  if (failed) return { text: `${plural(failed, "check")} failed.${blocked ? ` ${plural(blocked, "other")} couldn't run.` : ""}`, tone: "fail" };
  if (blocked) return { text: `Not finished: ${plural(blocked, "check")} couldn't run.`, tone: "unfinished" };
  if (warned) return { text: `Works, with ${plural(warned, "thing")} to look at.`, tone: "warn" };
  return { text: "Everything SparkBox needs works on this node.", tone: "ok" };
}

// An older core marks a step skipped by an earlier one only in its words ("Needs…", "After…").
function isBlockedLegacy(s: TestStep & { detail?: string }): boolean {
  return /^(needs |after you )/i.test(s.detail ?? "");
}

// ---- Advisories (§3.4): worth knowing, never a problem, never counted

export interface Advisory {
  id: string;
  /** "cell3 is on your LAN twice." */
  title: string;
  /** Ends where the command goes: "…turn it off on the node". */
  body: string;
  /** A command to run on the node, shown as code after the body. */
  command?: string;
  /** What follows the command: "SparkBox doesn't change it." */
  after?: string;
  /** Where it's explained in full. */
  tab: "network";
}

const subnet24 = (ip: string | null | undefined) => (ip && /^\d+\.\d+\.\d+\.\d+$/.test(ip) ? ip.split(".").slice(0, 3).join(".") : null);

/** "1G cable", "10G cable". */
const cable = (mbps: number) => (mbps >= 1000 ? `${mbps / 1000}G cable` : mbps > 0 ? `${mbps}M cable` : "cable");

/**
 * What's worth knowing about a node that isn't wrong: Wi-Fi up on the same network as its LAN
 * port (cell3: wlP9s9 at 192.168.50.229 beside enP7s7 at .98), so replies can leave over Wi-Fi.
 */
export function nodeAdvisories(n: NodeSnapshot): Advisory[] {
  const out: Advisory[] = [];
  const lan = n.nets.find((x) => x.role === "lan" && x.up && x.ip);
  for (const w of n.nets.filter((x) => x.role === "wifi" && x.up && x.ip)) {
    if (!lan || subnet24(w.ip) !== subnet24(lan.ip)) continue;
    const drops = w.carrierChanges ?? null;
    out.push({
      id: `dual-homed:${w.iface}`,
      title: `${n.name} is on your LAN twice.`,
      body: `${lan.iface} (${lan.ip}, ${cable(lan.speedMbps)}) and Wi-Fi ${w.iface} (${w.ip}) are on the same network, so replies can leave over Wi-Fi` +
        `${drops ? `, and its link has dropped ${drops} times since boot` : ""}. If ${n.name} doesn't need Wi-Fi, turn it off on the node`,
      command: "nmcli radio wifi off",
      after: "SparkBox doesn't change it.",
      tab: "network",
    });
  }
  return out;
}
