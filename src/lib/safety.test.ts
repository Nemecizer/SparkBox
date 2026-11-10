// The safety flows in the UI: Fleet's confirmation tiers, Retry reopening the original
// confirmation, Admin mode's Mac scope, task log deltas, the Activity log's Admin sessions
// and the redaction cases shared with the Rust side. The backend is mockTauri; dialogs are
// answered by a stub of confirmAction that records what it was asked.
import { isValidElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { emitMock, mockCalls, mockTauri, type MockHandlers } from "../test/tauri";
import type { ConfirmOptions } from "./session";
import type { ActivityEntry, Task } from "./opsApi";
import type { ClusterSnapshot, Container, NodeSnapshot } from "./types";
import cases from "./redact_cases.json";

const asked: ConfirmOptions[] = [];
const notices: { title: string; body?: unknown }[] = [];
let answer = true;
vi.mock("./session", async (importOriginal) => {
  const real = await importOriginal<typeof import("./session")>();
  return {
    ...real,
    confirmAction: vi.fn(async (opts: ConfirmOptions) => {
      asked.push(opts);
      return answer;
    }),
    alertInfo: vi.fn(async (opts: { title: string; body?: unknown }) => {
      notices.push(opts);
    }),
  };
});

// r4 F8: the power confirm renders serving's powerImpact; a test can put a known one in.
let powerImpactStub: { paragraph: string; followUp?: { label: string; endpointId: string } } | null = null;
vi.mock("./impact", async (importOriginal) => {
  const real = await importOriginal<typeof import("./impact")>();
  return { ...real, powerImpact: (...args: Parameters<typeof real.powerImpact>) => powerImpactStub ?? real.powerImpact(...args) };
});

let snapshot: ClusterSnapshot | null = null;
vi.mock("./store", async (importOriginal) => {
  const real = await importOriginal<typeof import("./store")>();
  return { ...real, getCluster: () => snapshot };
});

const { _resetAdminForTests, adminState, withAdmin, ADMIN_REFUSED } = await import("./admin");
const { confirmFleet, requestFleetRun, requestPower, wolWarning } = await import("./power");
const { _resetTasksForTests, appendLog, getTask, ingestTask, retryTask } = await import("./tasks");
const { foldAdminSessions, groupEntries, nodeRange } = await import("../windows/ActivityLog");
const { fleetHistory, historyItem, isPowerVerb } = await import("../windows/FleetCommand");
const { redact } = await import("./redact");

const container = (name: string, over: Partial<Container> = {}): Container => ({
  id: `${name}-0123456789ab`.slice(0, 12).padEnd(12, "0"), name, image: "nvcr.io/nvidia/vllm:26.05-py3", state: "running", startedAt: 0, finishedAt: 0,
  exitCode: 0, cpuPct: 1, mem: 0, gpuMem: 0, ports: [], project: null, command: "", restartPolicy: "no", labels: {}, ...over,
});

const node = (id: string, containers: Container[]): NodeSnapshot => ({
  id, name: id, host: `${id}.local`, status: "online", latencyMs: 1, error: null,
  cpu: { totalPct: 0, perCore: [], load: [0, 0, 0], tempC: 40 },
  mem: { total: 1, used: 0, available: 1, cached: 0, gpuAlloc: 0, swapTotal: 0, swapUsed: 0 },
  gpu: { utilPct: 0, tempC: 40, powerW: 10, smClockMhz: 0, procs: [] },
  disks: [], nets: [], system: { hostname: id, model: "", os: "", kernel: "", driver: "580.95.05", cuda: "", uptimeS: 100 },
  containers, processes: [], listeners: [],
});

function cluster(): ClusterSnapshot {
  const llama = { project: "llama-70b" };
  return {
    tsMs: 0, demo: true, switches: [],
    nodes: [
      node("cell1", [container("vllm-head", { ...llama, command: "vllm serve /models/meta-llama/Llama-3.3-70B-Instruct --port 8000" })]),
      node("cell2", [container("vllm-worker", { ...llama, command: "ray start --block" })]),
      node("cell3", [container("vllm-worker", { ...llama, command: "ray start --block" })]),
      node("gx10-0000", [container("ollama", { command: "/bin/ollama serve" })]),
    ],
  };
}

const NODES = ["cell1", "cell2", "cell3", "gx10-0000"];
const name = (id: string) => id;
/** The dialog's body as the user reads it. */
const bodyText = (o: ConfirmOptions) => (isValidElement(o.body) ? renderToStaticMarkup(o.body).replace(/<[^>]+>/g, " ") : String(o.body ?? ""))
  .replace(/&#x27;/g, "'").replace(/&quot;/g, '"').replace(/&amp;/g, "&");

function backend(over: MockHandlers = {}) {
  mockTauri({
    vpn_get: () => ({ status: { state: "direct" } }),
    ops_classify: ({ command }: { command: string }) =>
      command.startsWith("dd ") ? { class: "destructive", escalates: false, reason: "Writes straight to the disk device /dev/nvme0n1 (dd)." }
        : command === "systemctl stop ssh" ? { class: "destructive", escalates: false, reason: "Stops ssh (systemctl stop): SparkBox and you may not get back in until it's started at the node's console.", cutsAccess: true, power: null }
          : command === "reboot" ? { class: "destructive", escalates: false, reason: "Takes the node down (reboot).", cutsAccess: false, power: "restart" }
            : command === "uptime" ? { class: "read", escalates: false, reason: null }
              : { class: "change", escalates: false, reason: null },
    fleet_start: () => "t-fleet",
    node_power: () => "t-power",
    node_wol_status: ({ node }: { node: string }) => ({ node, enabled: node !== "cell3", routes: [] }),
    containers_bulk: () => "t-bulk",
    admin_unlock: () => ({ unlocked: true, expiresAtMs: Date.now() + 900_000, idleMinutes: 15, requiresAuth: false, authenticated: false }),
    admin_authenticate: () => ({ unlocked: true, expiresAtMs: Date.now() + 900_000, idleMinutes: 15, requiresAuth: false, authenticated: true }),
    admin_state: () => adminState(),
    ...over,
  });
}

beforeEach(() => {
  powerImpactStub = null;
  asked.length = 0;
  notices.length = 0;
  answer = true;
  snapshot = cluster();
  _resetAdminForTests();
  _resetTasksForTests();
  backend();
});

describe("Fleet confirmation tiers", () => {
  it("a read runs without asking", async () => {
    const plan = await confirmFleet({ nodes: NODES, command: "uptime", sudo: false, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(plan?.tier).toBe("read");
    expect(asked).toHaveLength(0);
    expect(mockCalls.find((c) => c.cmd === "ops_classify")?.args).toEqual({ command: "uptime" });
  });

  it("a change lists what's on the targets; root on several nodes asks for the count typed", async () => {
    await confirmFleet({ nodes: NODES, command: "systemctl restart docker", sudo: false, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(asked[0].requireTyping).toBeUndefined();
    expect(bodyText(asked[0])).toMatch(/Llama-3.3-70B-Instruct \(llama-70b\) runs on cell1, cell2 and cell3/);
    await confirmFleet({ nodes: NODES, command: "apt-get update", sudo: true, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(asked[1].requireTyping).toBe("4 nodes");
    expect(asked[1].title).toBe("Run on 4 nodes as root?");
  });

  it("dd on four nodes: red banner, typed count, one at a time, stop on failure and a canary", async () => {
    const plan = await confirmFleet({ nodes: NODES, command: "dd if=/dev/zero of=/dev/nvme0n1", sudo: true, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(asked).toHaveLength(1);
    expect(asked[0].requireTyping).toBe("4 nodes");
    expect(asked[0].danger).toBe(true);
    expect(asked[0].confirmLabel).toBe("Run on cell1");
    const body = bodyText(asked[0]);
    expect(body).toContain("This can destroy data or take nodes down");
    expect(body).toContain("Writes straight to the disk device /dev/nvme0n1 (dd).");
    expect(plan).toMatchObject({ tier: "destructive", strategy: "oneByOne", stopOnFailure: true, canary: true });
  });

  it("starts only the canary first", async () => {
    const started = await requestFleetRun({ nodes: NODES, command: "dd if=/dev/zero of=/dev/nvme0n1", sudo: true, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(started?.id).toBe("t-fleet");
    const call = mockCalls.find((c) => c.cmd === "fleet_start")!;
    expect(call.args).toMatchObject({ nodes: ["cell1"], strategy: "oneByOne", stopOnFailure: true });
  });

  it("stop ssh on four nodes: cuts access, canary, then one at a time once SparkBox is back in", async () => {
    const plan = await confirmFleet({ nodes: NODES, command: "systemctl stop ssh", sudo: true, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    const body = bodyText(asked[0]);
    expect(body).toContain("SparkBox may lose its connection to these nodes");
    expect(body).toContain("Stops ssh (systemctl stop)");
    expect(body).toContain("cell1 first. SparkBox then waits until it can log in to cell1 again over SSH");
    expect(asked[0].requireTyping).toBe("4 nodes");
    expect(plan).toMatchObject({ tier: "destructive", strategy: "oneByOne", stopOnFailure: true, canary: true, cutsAccess: true });
    await requestFleetRun({ nodes: NODES, command: "systemctl stop ssh", sudo: true, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(mockCalls.find((c) => c.cmd === "fleet_start")?.args).toMatchObject({ nodes: ["cell1"], cutsAccess: true });
  });

  it("a restart on several nodes cuts access too; on one node it's only destructive", async () => {
    await confirmFleet({ nodes: NODES, command: "reboot", sudo: true, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(bodyText(asked[0])).toContain("It restarts each node, so SparkBox loses each one for a while.");
    // A power verb reads "takes these nodes down", not "may lose its connection" (P1.42).
    expect(bodyText(asked[0])).toContain("This takes these nodes down");
    expect(bodyText(asked[0])).not.toContain("SparkBox may lose its connection");
    const one = await confirmFleet({ nodes: ["cell2"], command: "reboot", sudo: true, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(bodyText(asked[1])).toContain("This can destroy data or take nodes down");
    expect(one?.cutsAccess).toBe(false);
  });

  it("fails closed: no classification is destructive", async () => {
    backend({ ops_classify: () => { throw new Error("backend gone"); } });
    const plan = await confirmFleet({ nodes: NODES, command: "uptime", sudo: false, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(plan?.tier).toBe("destructive");
    expect(bodyText(asked[0])).toContain("SparkBox couldn't check this command");
  });

  it("cancelled means nothing runs", async () => {
    answer = false;
    expect(await requestFleetRun({ nodes: NODES, command: "dd if=/dev/zero of=/dev/nvme0n1", sudo: false, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name)).toBeNull();
    expect(mockCalls.some((c) => c.cmd === "fleet_start")).toBe(false);
  });

  it("a root docker command asks for the typed count and runs a canary one node at a time (P1.42)", async () => {
    backend({ ops_classify: () => ({ class: "change", escalates: true, reason: null, cutsAccess: false, power: null, root: true }) });
    const plan = await confirmFleet({ nodes: NODES, command: "docker run --rm -v /:/host alpine true", sudo: false, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(asked[0].requireTyping).toBe("4 nodes");
    expect(bodyText(asked[0])).toContain("Docker is root on the node, so cell1 runs first");
    expect(plan).toMatchObject({ tier: "change", strategy: "oneByOne", stopOnFailure: true, canary: true });
    // On one node there's no canary (nothing else to stage).
    const one = await confirmFleet({ nodes: ["cell2"], command: "docker run --rm -v /:/host alpine true", sudo: false, timeoutS: 60, strategy: "parallel", stopOnFailure: false }, name);
    expect(one).toMatchObject({ tier: "change", canary: false });
  });

  it("power verbs point at Restart… and history keeps no secrets", () => {
    expect(isPowerVerb("Takes the node down (reboot).")).toBe(true);
    expect(isPowerVerb("Writes straight to the disk device /dev/sda (dd).")).toBe(false);
    const item = historyItem({ command: "curl -H 'Authorization: Bearer abc123' http://x", sudo: false, nodes: ["cell1"], startedMs: 1 }, []);
    expect(item.command).toBe("curl -H 'Authorization: ‹redacted›' http://x");
  });
});

function task(over: Partial<Task>): Task {
  return {
    id: "t1", kind: "power", title: "Restart cell2", nodes: ["cell2"], source: "ui", cancellable: true, state: "failed", startedMs: 1, finishedMs: 2,
    progress: null, detail: null, rateBps: null, etaS: null, steps: [], log: [], summary: null, error: "boom", cancelRequested: false, retry: null, opId: "t1", ...over,
  };
}

describe("Retry reopens the original confirmation", () => {
  it("a restart asks for the node's name again, and nothing runs when cancelled", async () => {
    answer = false;
    await retryTask(task({ retry: { command: "node_power", args: { node: "cell2", action: "reboot" } } }));
    expect(asked).toHaveLength(1);
    expect(asked[0].title).toBe("Restart cell2?");
    expect(asked[0].requireTyping).toBe("cell2");
    expect(mockCalls.some((c) => c.cmd === "node_power")).toBe(false);
  });

  it("a shutdown always asks for the name and warns when Wake-on-LAN is off", async () => {
    await retryTask(task({ title: "Shut down cell3", retry: { command: "node_power", args: { node: "cell3", action: "poweroff" } } }));
    expect(asked[0].requireTyping).toBe("cell3");
    expect(bodyText(asked[0])).toContain("You won't be able to turn cell3 back on remotely: Wake-on-LAN is off");
    expect(mockCalls.find((c) => c.cmd === "node_power")?.args).toMatchObject({ node: "cell3", action: "poweroff" });
  });

  it("bulk containers show their impact, volumes included", async () => {
    answer = false;
    const items = [{ node: "gx10-0000", id: "ollama-01234", name: "ollama" }];
    await retryTask(task({ title: "Remove 1 container", retry: { command: "containers_bulk", args: { items, action: "remove", removeVolumes: true, title: null } } }));
    expect(asked[0].title).toBe("Remove 1 container?");
    expect(bodyText(asked[0])).toContain("Their volumes are deleted too");
    expect(mockCalls.some((c) => c.cmd === "containers_bulk")).toBe(false);
  });

  it("Fleet goes back to the Fleet window and its tier, never straight to fleet_start", async () => {
    const seen: unknown[] = [];
    const on = (e: Event) => seen.push((e as CustomEvent).detail);
    window.addEventListener("sparkbox:fleet-rerun", on);
    await retryTask(task({ kind: "fleet", retry: { command: "fleet_start", args: { nodes: NODES, command: "dd if=/dev/zero of=/dev/nvme0n1", sudo: true, timeoutS: 60, strategy: "parallel", stopOnFailure: false } } }));
    window.removeEventListener("sparkbox:fleet-rerun", on);
    expect(seen).toHaveLength(1);
    expect(mockCalls.some((c) => c.cmd === "fleet_start")).toBe(false);
  });

  it("Wake asks nothing", async () => {
    await requestPower("cell3", "wake");
    expect(asked).toHaveLength(0);
    expect(mockCalls.find((c) => c.cmd === "node_power")?.args).toMatchObject({ node: "cell3", action: "wake" });
  });

  it("an unknown VPN status counts as away", async () => {
    backend({ vpn_get: () => { throw new Error("helper gone"); } });
    answer = false;
    await requestPower("cell1", "reboot");
    expect(bodyText(asked[0])).toContain("SparkBox can't tell whether you're away, so it assumes you are");
    expect(wolWarning("cell3", null)).toContain("can't tell whether Wake-on-LAN is on");
  });
});

describe("Admin mode scopes", () => {
  it("a Demo unlock covers nodes; a Mac change asks to confirm it's you, then retries", async () => {
    let calls = 0;
    backend({
      security_set: () => {
        calls += 1;
        if (!adminState().authenticated) throw { kind: "needs_auth", message: "Confirm it's you with Touch ID or your Mac password to change settings on this Mac." };
        return { idleMinutes: 5, retentionDays: 90 };
      },
    });
    const { opsApi } = await import("./opsApi");
    const saved = await withAdmin(() => opsApi.securitySet({ idleMinutes: 5, retentionDays: 90 }), { action: "security_set" });
    expect(saved.idleMinutes).toBe(5);
    expect(calls).toBe(2);
    expect(mockCalls.map((c) => c.cmd).filter((c) => c.startsWith("admin_"))).toEqual(["admin_unlock", "admin_authenticate"]);
    expect(mockCalls.find((c) => c.cmd === "admin_authenticate")?.args).toEqual({ action: "security_set", fresh: false });
  });

  it("a cancelled prompt rejects with ADMIN_REFUSED and runs nothing", async () => {
    backend({ admin_authenticate: () => { throw { kind: "cancelled", message: "Admin mode stays locked." }; } });
    const fn = vi.fn(async () => 1);
    await expect(withAdmin(fn, { scope: "mac" })).rejects.toBe(ADMIN_REFUSED);
    expect(fn).not.toHaveBeenCalled();
  });
});

describe("task updates", () => {
  it("merges log deltas and keeps the log across updates that carry none", async () => {
    ingestTask(task({ id: "t9", state: "running", log: ["first"] }));
    appendLog("t9", ["second", "third"]);
    expect(getTask("t9")?.log).toEqual(["first", "second", "third"]);
    ingestTask(task({ id: "t9", state: "running", log: [], progress: 0.5 }));
    expect(getTask("t9")?.log).toEqual(["first", "second", "third"]);
    expect(getTask("t9")?.progress).toBe(0.5);
    await emitMock("tasks://log", { id: "t9", lines: ["fourth"] });
  });
});

describe("Activity log", () => {
  const e = (over: Partial<ActivityEntry>): ActivityEntry => ({
    id: Math.random().toString(36), tsMs: 0, updatedMs: 0, node: "mac", kind: "app", command: "", code: null, ms: 0, changes: true, error: null, opId: null,
    title: "", source: "ui", actor: "ui", escalated: false, state: "ok", stdoutTail: null, stderrTail: null, simulated: false, class: "app", ...over,
  });

  it("folds an unlock and its relock into one Admin session that isn't a change", () => {
    const t0 = new Date(2026, 9, 28, 12, 3).getTime();
    const t1 = new Date(2026, 9, 28, 12, 18).getTime();
    const folded = foldAdminSessions([
      e({ title: "Admin mode locked", command: "No changes for 15 minutes · unlocked at 2026-10-28T16:03:00Z", tsMs: t1 }),
      e({ title: "Restart docker on cell2", node: "cell2", kind: "op", class: "change", tsMs: t0 + 60_000, opId: "op1" }),
      e({ title: "Admin mode unlocked", command: "Demo data: no password needed", tsMs: t0 }),
    ]);
    const groups = groupEntries(folded);
    const session = groups.find((g) => g.session)!;
    expect(session.head.title).toMatch(/^Admin session 12:03\s?(PM)?–12:18\s?(PM)? \(relocked after 15 min idle\)$/);
    expect(session.entries.every((x) => !x.changes)).toBe(true);
    expect(session.entries).toHaveLength(3);
    expect(groups.filter((g) => !g.session && g.entries.some((x) => x.changes))).toHaveLength(1);
  });

  it("says a session never relocked when Admin mode is off now (SparkBox quit while unlocked)", () => {
    const t0 = new Date(2026, 9, 28, 23, 29).getTime();
    const entries = [e({ title: "Admin mode unlocked", command: "Demo data: no password needed", tsMs: t0 })];
    const head = (unlocked: boolean) => groupEntries(foldAdminSessions(entries, unlocked)).find((g) => g.session)!.head;
    expect(head(true).title).toMatch(/\(unlocked now\)$/);
    expect(head(true).state).toBe("running");
    expect(head(false).title).toMatch(/\(no relock recorded: SparkBox was stopped\)$/);
    expect(head(false).state).toBe("ok");
  });

  it("names a run of nodes as a range", () => {
    expect(nodeRange(["cell1", "cell2", "cell3"])).toBe("cell1–cell3");
    expect(nodeRange(["cell1", "gx10-0000"])).toBe("cell1, gx10-0000");
  });
});

describe("redaction", () => {
  it("matches the cases the Rust side checks (src/lib/redact_cases.json)", () => {
    for (const [input, want] of cases.redact) expect(redact(input), input).toBe(want);
    for (const keep of cases.keep) expect(redact(keep)).toBe(keep);
  });
});

describe("a root-only file's refusal (r4 F5)", () => {
  it("reads as a permission refusal whatever order the JSON's keys come in", async () => {
    const { commandError } = await import("./api");
    const { permissionRefusal } = await import("./filesApi");
    const detail = JSON.stringify({ mode: "600", owner: "root", path: "/etc/netplan/40-cx7.yaml" });
    const message = "Only root can read 40-cx7.yaml (root, 600). Reading it needs sudo, which needs Admin.";
    // As files_read sent it from cell1 in the Live check (serde_json sorts the keys), and kind first.
    for (const sent of [JSON.stringify({ detail, kind: "permission", message, node: "cell1" }), JSON.stringify({ kind: "permission", message, node: "cell1", detail })]) {
      expect(permissionRefusal(commandError(sent))).toEqual({ path: "/etc/netplan/40-cx7.yaml", mode: "600", owner: "root", message });
    }
    expect(permissionRefusal(commandError("cell1: No such file or directory"))).toBeNull();
    expect(commandError('{"not":"an error"}')).toBe('{"not":"an error"}');
  });
});

describe("a save refused for a changed file (r5)", () => {
  it("keeps what the refusal adds: the file's hash, time and size", async () => {
    const { commandError } = await import("./api");
    const sent = { kind: "changedOnDisk", message: "The file changed.", sha256: null, mtime: 1793485341, size: 12 };
    const e = commandError(JSON.stringify(sent)) as typeof sent;
    expect(e).toMatchObject(sent);
    expect((commandError(sent) as typeof sent).mtime).toBe(1793485341);
  });
});

describe("one error shape", () => {
  // As the backend sends them while Admin mode is locked: an SbError object from commands that
  // return one, the same SbError as JSON from commands that return strings.
  const LOCKED = { kind: "locked", message: "Admin mode is locked. Unlock it in the top bar to make changes." };
  it("a locked refusal reads the same from six commands", async () => {
    mockTauri({
      service_action: () => { throw LOCKED; },
      node_power: () => { throw LOCKED; },
      files_delete: () => { throw LOCKED; },
      task_cancel: () => { throw LOCKED; },
      tunnel_open: () => { throw LOCKED; },
      endpoint_forget: () => { throw JSON.stringify(LOCKED); },
      files_mkdir: () => { throw JSON.stringify(LOCKED); },
    });
    const { invoke, CommandError } = await import("./api");
    const { describeError } = await import("./errors");
    const { isLocked } = await import("./admin");
    const calls: [string, Record<string, unknown>][] = [
      ["service_action", { node: "cell1", unit: "docker.service", action: "restart" }],
      ["node_power", { node: "cell1", action: "reboot" }],
      ["files_delete", { place: "cell1", paths: ["/tmp/x"] }],
      ["task_cancel", { id: "t1" }],
      ["tunnel_open", { node: "cell1", port: 11000 }],
      ["endpoint_forget", { id: "e1" }],
      ["files_mkdir", { place: "cell1", dir: "/tmp", name: "x" }],
    ];
    for (const [cmd, args] of calls) {
      const e = await invoke(cmd, args).then(() => null, (err: unknown) => err);
      expect(e, cmd).toBeInstanceOf(CommandError);
      expect({ kind: (e as InstanceType<typeof CommandError>).kind, message: (e as Error).message }, cmd).toEqual(LOCKED);
      expect(String(e), cmd).toBe(LOCKED.message);
      expect(isLocked(e), cmd).toBe(true);
      expect(describeError(e).title, cmd).toBe("Admin mode is locked");
    }
    // A plain JSON string (a caller that used Tauri's invoke directly) reads the same in describeError.
    expect(describeError(JSON.stringify({ kind: "needs_auth", message: "Confirm it's you with Touch ID or your Mac password to change settings on this Mac." })).title).toBe("Confirm it's you");
  });
});

describe("unlocking asks in-app first (r4 F1)", () => {
  const LIVE = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true };

  it("a locked control shows the sheet before any prompt, and Cancel asks macOS nothing", async () => {
    _resetAdminForTests(LIVE);
    answer = false;
    const { confirmUnlock } = await import("./admin");
    expect(await confirmUnlock("copy node-launch.sh to Documents", "mac", "files_transfer")).toBe(false);
    expect(asked).toHaveLength(1);
    expect(asked[0].title).toBe("Unlock Admin mode?");
    expect(bodyText(asked[0])).toBe("To copy node-launch.sh to Documents, SparkBox needs Admin mode. It asks for Touch ID or your Mac password. Admin locks again after 15 minutes idle.");
    expect(asked[0].confirmLabel).toBe("Unlock…");
    expect(asked[0].cancelLabel).toBe("Cancel");
    expect(mockCalls.filter((c) => c.cmd.startsWith("admin_"))).toEqual([]);
  });

  it("Unlock… goes on to the backend once, with the command's key for the prompt's words", async () => {
    _resetAdminForTests(LIVE);
    const { confirmUnlock } = await import("./admin");
    expect(await confirmUnlock("copy node-launch.sh to Documents", "mac", "files_transfer")).toBe(true);
    expect(asked).toHaveLength(1);
    expect(mockCalls.filter((c) => c.cmd.startsWith("admin_")).map((c) => [c.cmd, c.args])).toEqual([["admin_authenticate", { action: "files_transfer", fresh: false }]]);
  });

  it("requestAdmin from any locked control asks first when macOS would prompt; the padlock itself doesn't", async () => {
    _resetAdminForTests(LIVE);
    answer = false;
    const { requestAdmin } = await import("./admin");
    expect(await requestAdmin("node_power")).toBe(false);
    expect(bodyText(asked[0])).toContain("To restart, shut down or wake a node, SparkBox needs Admin mode.");
    expect(mockCalls.some((c) => c.cmd === "admin_unlock")).toBe(false);
    answer = true;
    _resetAdminForTests(LIVE);
    expect(await requestAdmin()).toBe(true);
    expect(asked).toHaveLength(1);
    expect(mockCalls.filter((c) => c.cmd === "admin_unlock")).toHaveLength(1);
  });

  it("in look-only mode every unlock path shows the look-only notice and never reaches the backend", async () => {
    _resetAdminForTests({ ...LIVE, lookOnly: true, lookOnlyForced: true });
    const { confirmUnlock, requestAdmin, padlockText, LOOK_ONLY_TEXT } = await import("./admin");
    expect(padlockText()).toBe("Look only");
    expect(await confirmUnlock("copy node-launch.sh to Documents", "mac")).toBe(false);
    expect(await requestAdmin("node_power")).toBe(false);
    expect(await requestAdmin()).toBe(false);
    await expect(withAdmin(async () => 1, { scope: "mac" })).rejects.toBe(ADMIN_REFUSED);
    expect(asked).toHaveLength(0);
    // The sheet names what was blocked and the way out (P1.43); here SPARKBOX_LOOK_ONLY set it.
    const out = "SPARKBOX_LOOK_ONLY=1 turned it on for this run: quit SparkBox and start it without it.";
    const cant = (what: string) => `SparkBox can't ${what} in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date. ${out}`;
    expect(notices.map((n) => [n.title, n.body])).toEqual([
      ["Look only", cant("copy node-launch.sh to Documents")],
      ["Look only", cant("restart, shut down or wake a node")],
      ["Look only", `${LOOK_ONLY_TEXT} ${out}`],
      ["Look only", `${LOOK_ONLY_TEXT} ${out}`],
    ]);
    expect(notices.every((n) => !/unlock/i.test(String(n.body))), "nothing in look-only promises an unlock").toBe(true);
    // From the Settings switch instead, the way out is that switch.
    notices.length = 0;
    _resetAdminForTests({ ...LIVE, lookOnly: true });
    expect(await requestAdmin("node_power")).toBe(false);
    expect(notices[0].body).toBe("SparkBox can't restart, shut down or wake a node in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date. Turn it off in Settings › Security & privacy (it asks for Touch ID or your Mac password).");
    expect(mockCalls.filter((c) => c.cmd.startsWith("admin_"))).toEqual([]);
  });

  it("a lookOnly refusal from the backend reads as look only, with no Unlock button", async () => {
    const { describeError } = await import("./errors");
    const d = describeError({ kind: "lookOnly", message: "SparkBox is in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date." });
    expect([d.title, d.actions.map((a) => a.label)]).toEqual(["Look only", ["What does this mean?"]]);
    expect(describeError("SparkBox is in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date.").kind).toBe("lookOnly");
  });
});

describe("transferGate: the button, ⌘V and drag and drop ask the same (r4 F1, F2)", () => {
  type Case = [string, Partial<import("./filesApi").TransferSpec>, ("mac" | "node")[]];
  const HOME = "/Users/sparkuser";
  const cases: Case[] = [
    ["node → ~/Downloads", { from: "cell1", dir: "/home/sparkuser", names: ["node-launch.sh"], to: "mac", toDir: "~/Downloads" }, []],
    ["node → a folder in Downloads", { from: "cell1", dir: "/home/sparkuser", names: ["node-launch.sh"], to: "mac", toDir: `${HOME}/Downloads/models` }, []],
    ["node → Documents (the tour's case)", { from: "cell1", dir: "/home/sparkuser", names: ["node-launch.sh"], to: "mac", toDir: `${HOME}/Documents` }, ["mac"]],
    ["node → ~", { from: "cell1", dir: "/home/sparkuser", names: ["a.txt"], to: "mac", toDir: "~" }, ["mac"]],
    ["node → Downloads replacing a file", { from: "cell1", dir: "/home/sparkuser", names: ["a.txt"], to: "mac", toDir: "~/Downloads", policies: { "a.txt": "replace" } }, ["mac"]],
    ["node → Downloads as a move", { from: "cell1", dir: "/home/sparkuser", names: ["a.txt"], to: "mac", toDir: "~/Downloads", op: "move" }, ["mac"]],
    ["Mac → node", { from: "mac", dir: `${HOME}/models`, names: ["w.safetensors"], to: "cell1", toDir: "/home/sparkuser/models" }, ["node"]],
    ["Mac → node, a move", { from: "mac", dir: `${HOME}/models`, names: ["w.safetensors"], to: "cell1", toDir: "/home/sparkuser/models", op: "move" }, ["mac"]],
    ["node → node", { from: "cell1", dir: "/home/sparkuser", names: ["a.txt"], to: "cell2", toDir: "/home/sparkuser" }, ["node"]],
  ];

  it("gives each of the cases its scope, and locked follows it", async () => {
    const { transferGate, transferLocked } = await import("./transfers");
    const base = { from: "mac", dir: "/", names: [], to: "cell1", toDir: "/", op: "copy" as const, policies: {} };
    const unlockedNode = { unlocked: true, expiresAtMs: Date.now() + 60_000, idleMinutes: 15, requiresAuth: true, authenticated: false };
    for (const [label, spec, needs] of cases) {
      const gate = transferGate({ ...base, ...spec }, HOME);
      expect(gate.needs, label).toEqual(needs);
      expect(gate.reason, label).toBeTruthy();
      expect(transferLocked(gate, { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true }), label).toBe(needs.length > 0);
      expect(transferLocked(gate, unlockedNode), label).toBe(needs.includes("mac"));
    }
  });

  it("no case that can start is refused", async () => {
    const { transferGate } = await import("./transfers");
    const base = { from: "mac", dir: "/", names: [], to: "cell1", toDir: "/", op: "copy" as const, policies: {} };
    for (const [label, spec] of cases) expect(transferGate({ ...base, ...spec }, HOME).refused, label).toBeUndefined();
  });

  it("the tour's case in Demo: the sheet, then Cancel queues nothing and asks the backend nothing", async () => {
    _resetAdminForTests();
    answer = false;
    const { startGatedTransfer, getTransfers, _resetTransfersForTests } = await import("./transfers");
    _resetTransfersForTests();
    const id = await startGatedTransfer(
      { from: "cell1", dir: "/home/sparkuser", names: ["node-launch.sh"], to: "mac", toDir: "/Users/sparkuser/Documents", op: "copy", policies: {} },
      { fromName: "cell1", toName: "This Mac" }, "copy node-launch.sh to Documents", "/Users/sparkuser",
    );
    expect(id).toBeNull();
    expect(asked.map((a) => a.title)).toEqual(["Unlock Admin mode?"]);
    expect(bodyText(asked[0])).toContain("To copy node-launch.sh to Documents, SparkBox needs Admin mode. It asks for Touch ID or your Mac password.");
    expect(getTransfers()).toEqual([]);
    expect(mockCalls.map((c) => c.cmd).filter((c) => c.startsWith("admin_") || c.startsWith("files_"))).toEqual([]);
  });

  it("a copy into Downloads starts without a sheet", async () => {
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true });
    backend({ files_preflight: () => ({ bytes: 10, entries: 1, conflicts: [], free: null, route: "direct" }), files_transfer: () => "x1" });
    const { startGatedTransfer, _resetTransfersForTests } = await import("./transfers");
    _resetTransfersForTests();
    const id = await startGatedTransfer(
      { from: "cell1", dir: "/home/sparkuser", names: ["node-launch.sh"], to: "mac", toDir: "~/Downloads", op: "copy", policies: {} },
      { fromName: "cell1", toName: "This Mac" }, "copy node-launch.sh to Downloads",
    );
    expect(id).toBeTruthy();
    expect(asked).toHaveLength(0);
    _resetTransfersForTests();
  });
});

describe("identical reads in flight share one call (r4 F4)", () => {
  it("two activity_log calls at once reach the backend once; a change never shares", async () => {
    let n = 0;
    backend({
      activity_log: () => new Promise((r) => setTimeout(() => r([{ n: ++n }]), 5)),
      files_mkdir: () => null,
    });
    const { invoke, isSharedRead } = await import("./api");
    const [a, b] = await Promise.all([invoke("activity_log"), invoke("activity_log")]);
    expect(a).toBe(b);
    expect(mockCalls.filter((c) => c.cmd === "activity_log")).toHaveLength(1);
    await invoke("activity_log");
    expect(mockCalls.filter((c) => c.cmd === "activity_log")).toHaveLength(2);
    await Promise.all([invoke("files_mkdir", { place: "cell1", dir: "/tmp", name: "x" }), invoke("files_mkdir", { place: "cell1", dir: "/tmp", name: "x" })]);
    expect(mockCalls.filter((c) => c.cmd === "files_mkdir")).toHaveLength(2);
    expect(["terminal_input", "files_write", "fleet_start", "prefs_patch", "menu_set_state", "files_read_sudo"].some(isSharedRead)).toBe(false);
    expect(["get_snapshot", "tasks_list", "activity_log_since", "admin_state", "files_read", "files_backups", "updates_status", "services_poll"].every(isSharedRead)).toBe(true);
  });
});

describe("Restart… says what happens to the models, and offers the recovery (r4 F8)", () => {
  const STUB = {
    paragraph: "llama-3.3-70b runs across cell1, cell2 and cell3. It pauses while cell3 restarts (planned, so nobody is paged). It doesn't come back by itself.",
    followUp: { label: "Then restart llama-3.3-70b in order once cell3 is back", endpointId: "ep-llama" },
  };

  it("checked by default: the restart starts, then model_restart_after for that endpoint", async () => {
    powerImpactStub = STUB;
    backend({ model_restart_after: () => "t-after", vpn_get: () => ({ status: { state: "direct" } }) });
    const id = await requestPower("cell3", "reboot");
    expect(id).toBe("t-power");
    expect(bodyText(asked[0])).toContain(STUB.paragraph);
    expect(bodyText(asked[0])).toContain(STUB.followUp.label);
    const calls = mockCalls.map((c) => c.cmd).filter((c) => c === "node_power" || c === "model_restart_after");
    expect(calls).toEqual(["node_power", "model_restart_after"]);
    expect(mockCalls.find((c) => c.cmd === "model_restart_after")?.args).toEqual({ node: "cell3", endpointId: "ep-llama" });
  });

  it("Shut down… reuses the paragraph and offers no restart", async () => {
    powerImpactStub = { paragraph: "llama-3.3-70b runs across cell1, cell2 and cell3. It stops until cell3 is woken. It doesn't come back by itself." };
    backend({ node_wol_status: () => ({ node: "cell3", enabled: true, routes: [] }) });
    await requestPower("cell3", "poweroff");
    expect(bodyText(asked[0])).toContain("It stops until cell3 is woken.");
    expect(mockCalls.some((c) => c.cmd === "model_restart_after")).toBe(false);
  });
});

describe("Fleet history is kept per data source (r4 F13)", () => {
  it("a Demo run never shows in Live's history", () => {
    const run = (command: string) => historyItem({ command, sudo: false, nodes: ["cell1"], startedMs: 1 }, []);
    localStorage.setItem("sparkbox.fleet.history:demo", JSON.stringify([run("uptime")]));
    localStorage.setItem("sparkbox.fleet.history:live", JSON.stringify([run("nvidia-smi")]));
    expect(fleetHistory("live").map((h) => h.command)).toEqual(["nvidia-smi"]);
    expect(fleetHistory("demo").map((h) => h.command)).toEqual(["uptime"]);
  });
});
