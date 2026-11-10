import { describe, expect, it, vi } from "vitest";
import cases from "../../src-tauri/src/health/fixtures/gpu-throttle-cases.json";
import type { GpuStats, NodeSnapshot } from "./types";

vi.mock("../wm/WindowManager", () => ({ openWindow: vi.fn() }));
const { errorKind, hostKeyState, linksIn, nodeActionFor, offlineReason, powerLabel, shortFingerprint, testSummary, THROTTLE_RULE, throttleVerdict } = await import("./nodeStatus");

const FP = "SHA256:BZ5Ax5nfR3u/LCFrjzg3eroa9t08yevI46BSyXedAbg";
const node = (p: Partial<NodeSnapshot> = {}): NodeSnapshot => ({ id: "cell3", name: "cell3", host: "192.168.50.98", status: "error", latencyMs: null, error: null, ...p } as NodeSnapshot);
// The words live/ssh.rs uses for first contact and for a key that changed.
const FIRST = `SparkBox hasn't connected to 192.168.50.98:22 before. Its host key is ${FP}. Compare it with the node (ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub) and trust it in Connect or Settings › Nodes.`;
const CHANGED = `The host key for 192.168.50.98:22 changed (was SHA256:nIaTibRJmp5ZwhYU6VwFEDVsD0+j5agpqky/U9s2yC0, now ${FP}). If you reinstalled the node, use Settings › Diagnostics › Forget host keys; otherwise something may be intercepting the connection.`;

describe("offlineReason", () => {
  it("calls a first connection not trusted yet, and only a real mismatch changed", () => {
    expect(offlineReason(node({ error: FIRST }))).toBe("Host key not trusted yet");
    expect(offlineReason(node({ error: FIRST, errorKind: "hostKeyUnknown" }))).toBe("Host key not trusted yet");
    expect(offlineReason(node({ error: CHANGED }))).toBe("Host key changed");
    expect(offlineReason(node({ error: "anything", errorKind: "hostKeyChanged" }))).toBe("Host key changed");
  });

  it("keeps the round-3 words for everything else", () => {
    expect(offlineReason(node({ error: "SSH timeout: no reply from 192.168.50.53:22 in 10 s" }))).toBe("No SSH reply");
    expect(offlineReason(node({ error: "Permission denied (publickey,password)." }))).toBe("Login failed");
    expect(offlineReason(node({ error: "The password for sparkuser@192.168.50.98 was rejected. Check it in Settings." }))).toBe("Login failed");
    expect(offlineReason(node({ error: "Couldn't reach 192.168.50.98:22: Connection refused (os error 61)" }))).toBe("SSH refused the connection");
    expect(offlineReason(node({ error: "weird failure talking to 192.168.50.53:22 now" }))).toBe("weird failure talking to now");
    expect(offlineReason(node({ powerState: "off" }))).toBe("Shut down from SparkBox");
    expect(offlineReason(node({ status: "connecting" }))).toBe("Connecting");
    expect(errorKind(node({ error: "No route to host" }))).toBe("unreachable");
  });
});

describe("powerLabel", () => {
  it("names planned work, never Offline", () => {
    expect(powerLabel(node({ powerState: "rebooting" }))).toBe("Restarting");
    expect(powerLabel(node({ powerState: "shutting-down" }))).toBe("Shutting down");
    expect(powerLabel(node({ powerState: "off" }))).toBe("Off (planned)");
    expect(powerLabel(node({ powerState: "waking" }))).toBe("Waking");
    expect(powerLabel(node({ status: "offline", maintenance: { reason: "shut down", startedMs: 1 } }))).toBe("Off (planned)");
    expect(powerLabel(node({ status: "online" }))).toBeNull();
  });
});

describe("host keys", () => {
  it("reads the key from the backend's fields, else from its words", () => {
    expect(hostKeyState(node({ error: FIRST }))).toEqual({ kind: "unknown", fingerprint: FP, address: "192.168.50.98:22", sharedWith: [] });
    expect(hostKeyState(node({ error: CHANGED }))?.fingerprint).toBe(FP);
    const k = hostKeyState(node({ error: FIRST, errorKind: "hostKeyUnknown", hostKey: { address: "192.168.50.98:22", fingerprint: FP, sharedWith: ["cell1", "cell2"] } }));
    expect(k?.sharedWith).toEqual(["cell1", "cell2"]);
    expect(hostKeyState(node({ status: "online" }))).toBeNull();
    expect(shortFingerprint(FP)).toBe("SHA256:BZ5A…Abg");
  });

  it("offers Trust for a first connection and Review for a changed key", () => {
    expect(nodeActionFor(node({ error: FIRST }))?.label).toBe("Trust host key…");
    expect(nodeActionFor(node({ error: CHANGED }))?.label).toBe("Review host key…");
    expect(nodeActionFor(node({ error: "The password for sparkuser@x was rejected." }))?.label).toBe("Open Cluster login");
    expect(nodeActionFor(node({ status: "online" }))).toBeNull();
  });

  it("turns the places a message names into buttons", () => {
    expect(linksIn(FIRST).map((l) => l.label)).toEqual(["Connect my Sparks…", "Open Settings › Nodes"]);
    expect(linksIn(CHANGED).map((l) => l.label)).toEqual(["Open Diagnostics"]);
    expect(linksIn("SparkBox couldn't connect to 1.2.3.4")).toEqual([]);
  });
});

describe("the node Test summary", () => {
  const ok = (id: string) => ({ id, status: "ok" as const });
  const rest = ["auth", "method", "sudo", "python", "docker", "gpu", "fabric"];
  it.each([
    ["every check passed", [ok("resolve"), ok("tcp"), ok("hostkey"), ...rest.map(ok)], "Everything SparkBox needs works on this node.", "ok"],
    ["stopped at an untrusted key", [ok("resolve"), ok("tcp"), { id: "hostkey", status: "warn" as const, fingerprint: FP }, ...rest.slice(0, 6).map((id) => ({ id, status: "skip" as const, blocked: true }))],
      "Not finished: trust the host key to run the other 6 checks.", "unfinished"],
    ["two failures", [ok("resolve"), ok("tcp"), ok("hostkey"), { id: "auth", status: "fail" as const }, { id: "gpu", status: "fail" as const }], "2 checks failed.", "fail"],
    ["a warning only", [ok("resolve"), ok("hostkey"), { id: "docker", status: "warn" as const }], "Works, with 1 thing to look at.", "warn"],
    ["Wake-on-LAN unreadable is a skip on its own, not a stop", [ok("resolve"), ok("hostkey"), { id: "wol", status: "skip" as const, blocked: false }], "Everything SparkBox needs works on this node.", "ok"],
  ])("%s", (_, steps, text, tone) => {
    const s = testSummary(steps);
    expect(s.text).toBe(text);
    expect(s.tone).toBe(tone);
    // Never "everything works" with a failure, a warning or a check that didn't run.
    expect(s.text === "Everything SparkBox needs works on this node.").toBe(!steps.some((x) => x.status === "fail" || x.status === "warn" || (x.status === "skip" && (x as { blocked?: boolean }).blocked)));
  });
});

describe("throttleVerdict", () => {
  const gpu = (p: Partial<GpuStats>): GpuStats => ({ utilPct: 0, tempC: 44, powerW: 4.1, smClockMhz: 208, clockMaxMhz: 3003, procs: [], pstate: "P8", throttle: [], ...p });
  it("uses the same rule as the gpu-throttle alert", () => {
    const { busyUtilPct, clockBelow, powerAt, always, never } = cases.rule;
    expect(THROTTLE_RULE).toEqual({ busyUtilPct, clockBelow, powerAt, always, never });
  });
  it.each(cases.verdicts.map((v) => [v.name, v] as const))("%s", (_, v) => {
    const g = { procs: [], ...v.gpu } as unknown as GpuStats;
    const got = throttleVerdict(g);
    expect(got.throttled).toBe(v.throttled);
    expect(got.alertText).toBe(v.text);
  });
  it("calls an idle GPU idle, whichever idle reason it reports", () => {
    // The real idle alternation on all four GB10s: [] and ["SW power cap"] on alternate readings.
    for (const throttle of [["GPU idle"], ["SW power cap"], ["GPU idle", "SW power cap"], []]) {
      expect(throttleVerdict(gpu({ throttle }))).toEqual({ throttled: false, idle: true, alertText: null, text: "Idle (P8) · clocks are low because nothing is running" });
    }
    expect(throttleVerdict(gpu({ utilPct: 97, pstate: "P0", smClockMhz: 1905, throttle: ["SW power cap"] })).text).toBe("Throttled: SW power cap");
    expect(throttleVerdict(gpu({ readable: false })).text).toBe("Not readable");
  });
});

describe("advisories", async () => {
  const { nodeAdvisories } = await import("./nodeStatus");
  it("flags cell3 on the LAN twice, as the tour found it, and nothing else", () => {
    const nets = [
      { iface: "enP7s7", role: "lan", up: true, speedMbps: 1000, rxBps: 0, txBps: 0, ip: "192.168.50.98" },
      { iface: "wlP9s9", role: "wifi", up: true, speedMbps: 867, rxBps: 0, txBps: 0, ip: "192.168.50.229", carrierChanges: 154 },
    ];
    const [a] = nodeAdvisories(node({ status: "online", nets } as Partial<NodeSnapshot>));
    expect(`${a.title} ${a.body} (${a.command}); ${a.after}`).toBe(
      "cell3 is on your LAN twice. enP7s7 (192.168.50.98, 1G cable) and Wi-Fi wlP9s9 (192.168.50.229) are on the same network, so replies can leave over Wi-Fi, " +
      "and its link has dropped 154 times since boot. If cell3 doesn't need Wi-Fi, turn it off on the node (nmcli radio wifi off); SparkBox doesn't change it.");
    expect(nodeAdvisories(node({ nets: [nets[0], { ...nets[1], up: false }] } as Partial<NodeSnapshot>))).toEqual([]);
    expect(nodeAdvisories(node({ nets: [nets[0], { ...nets[1], ip: "10.0.0.5" }] } as Partial<NodeSnapshot>))).toEqual([]);
  });
});
