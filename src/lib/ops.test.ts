import { beforeEach, describe, expect, it } from "vitest";
import { asMarkdown, ansiToSpans, csvTable, groupOutputs, stripAnsi, type NodeOutput } from "../windows/FleetCommand";
import { emptyAtRest, groupEntries, groupSummary, shortWhere } from "../windows/ActivityLog";
import { imageRef, isUnused, nodeSummary } from "../windows/Images";
import { applicable, exitReason } from "../windows/Containers";
import { portOwner } from "../windows/Workloads";
import { majority } from "../windows/UpdatesWindow";
import { describeError, kindOf } from "./errors";
import { dependencyOrder, deploymentsOf, impactOf, modelOf } from "./impact";
import type { ActivityEntry, DockerImage, NodeImages } from "./opsApi";
import { redact, redactEnv } from "./redact";
import { _resetTasksForTests, startClientTask, useTasks, waitTask, getTask } from "./tasks";
import type { ClusterSnapshot, Container, NodeSnapshot, VpnStatus } from "./types";
import { setVpnStatus } from "./vpn";

const container = (name: string, over: Partial<Container> = {}): Container => ({
  id: `${name}-0123456789ab`.slice(0, 12).padEnd(12, "0"), name, image: "nvcr.io/nvidia/vllm:26.05-py3", state: "running", startedAt: 0, finishedAt: 0,
  exitCode: 0, cpuPct: 1, mem: 0, gpuMem: 0, ports: [], project: null, command: "", restartPolicy: "no", labels: {}, ...over,
});

const node = (id: string, containers: Container[], over: Partial<NodeSnapshot> = {}): NodeSnapshot => ({
  id, name: id, host: `${id}.local`, status: "online", latencyMs: 1, error: null,
  cpu: { totalPct: 0, perCore: [], load: [0, 0, 0], tempC: 40 },
  mem: { total: 1, used: 0, available: 1, cached: 0, gpuAlloc: 0, swapTotal: 0, swapUsed: 0 },
  gpu: { utilPct: 0, tempC: 40, powerW: 10, smClockMhz: 0, procs: [] },
  disks: [], nets: [], system: { hostname: id, model: "", os: "", kernel: "", driver: "580.95.05", cuda: "", uptimeS: 100 },
  containers, processes: [], listeners: [], ...over,
});

/** The Demo world's shape: a vLLM model across cell1–cell3, Ollama on gx10-0000. */
function cluster(): ClusterSnapshot {
  const llama = { project: "llama-70b" };
  return {
    tsMs: 0, demo: true, switches: [],
    nodes: [
      node("cell1", [
        container("vllm-head", { ...llama, command: "vllm serve /models/meta-llama/Llama-3.3-70B-Instruct --served-model-name llama-3.3-70b --port 8000" }),
        container("open-webui", { project: "webui", restartPolicy: "unless-stopped" }),
      ]),
      node("cell2", [container("vllm-worker", { ...llama, command: "ray start --block" })]),
      node("cell3", [container("vllm-worker", { ...llama, command: "ray start --block" })]),
      node("gx10-0000", [container("ollama", { restartPolicy: "unless-stopped", command: "/bin/ollama serve" }), container("sglang-test", { state: "exited", exitCode: 137, oomKilled: true })]),
    ],
  };
}

describe("impact", () => {
  it("finds a multi-node deployment from its Compose project and the model from the head's command", () => {
    const d = deploymentsOf(cluster()).find((x) => x.id === "llama-70b")!;
    expect(d.nodes).toEqual(["cell1", "cell2", "cell3"]);
    expect(d.model).toBe("/models/meta-llama/Llama-3.3-70B-Instruct");
    expect(d.members.find((m) => m.node === "cell1")!.role).toBe("head");
    expect(modelOf(container("x", { labels: { "sparkbox.model": "Qwen/Qwen3-32B" } }))).toBe("Qwen/Qwen3-32B");
  });

  it("restarting a worker of a multi-node model is high impact", () => {
    const r = impactOf(cluster(), { nodes: ["cell2"], action: "reboot" });
    expect(r.tier).toBe("high");
    expect(r.items[0].text).toMatch(/Llama-3.3-70B-Instruct \(llama-70b\) runs across cell1, cell2 and cell3/);
  });

  it("shutting down while away says only Wake-on-LAN could bring it back", () => {
    const r = impactOf(cluster(), { nodes: ["gx10-0000"], action: "poweroff", remote: true });
    expect(r.tier).toBe("high");
    expect(r.items.some((i) => i.text === "gx10-0000 stays off until someone presses its power button. You're away (VPN connected), so only Wake-on-LAN could bring it back.")).toBe(true);
  });

  it("a plain restart at home is a confirm, and says which containers come back", () => {
    const r = impactOf(cluster(), { nodes: ["gx10-0000"], action: "reboot" });
    expect(r.tier).toBe("medium");
    expect(r.items.find((i) => i.kind === "container")!.text).toBe("1 other running container: ollama restarts by itself.");
  });

  it("the first node is the reachability probe; away from home that makes it the only path", () => {
    const home = impactOf(cluster(), { nodes: ["cell1"], action: "reboot" });
    expect(home.probeTarget).toBe(true);
    expect(home.onlyPath).toBe(false);
    const away = impactOf(cluster(), { nodes: ["cell1"], action: "reboot", remote: true, terminals: [{ node: "cell1" }, { node: "cell1" }, { node: "cell2" }] });
    expect(away.onlyPath).toBe(true);
    expect(away.terminals).toBe(2);
    expect(away.items.some((i) => i.kind === "terminal" && i.text.startsWith("2 open terminal tabs on cell1"))).toBe(true);
  });

  it("container actions only count the chosen containers", () => {
    const c = cluster();
    const webui = c.nodes[0].containers[1].id;
    const r = impactOf(c, { nodes: ["cell1"], action: "stop", containerIds: [webui] });
    expect(r.deployments).toEqual([]);
    expect(impactOf(c, { nodes: ["cell1"], action: "start", containerIds: [webui] }).tier).toBe("low");
  });

  it("orders a deployment: workers stop first, the head starts first", () => {
    const m = [{ role: "worker" as const, n: 2 }, { role: "head" as const, n: 1 }, { role: "worker" as const, n: 3 }];
    expect(dependencyOrder(m, "stop").map((x) => x.role)).toEqual(["worker", "worker", "head"]);
    expect(dependencyOrder(m, "start").map((x) => x.role)).toEqual(["head", "worker", "worker"]);
  });
});

describe("describeError", () => {
  it("reads SbError objects and plain strings alike", () => {
    expect(describeError({ kind: "sudoRejected", message: "x", node: "cell2" }).message).toBe("sudo on cell2 didn't accept the saved cluster password.");
    expect(describeError({ kind: "sudoRejected", message: "x" }).actions.map((a) => a.label)).toEqual(["Fix password", "What does this mean?"]);
    expect(describeError("cell2 didn't accept the password").message).toBe("cell2 didn't accept the password. Check the cluster password in Settings.");
    expect(describeError("Admin mode is locked. Unlock it in the top bar to make changes.").actions[0].label).toBe("Unlock");
    expect(describeError("cell3 isn't connected. Use Live data").kind).toBe("unreachable");
    expect(describeError("cell3 isn't connected. Use Live data").actions.map((a) => a.label), "at home: nothing to reconnect").toEqual(["What does this mean?"]);
    setVpnStatus({ state: "connected" } as VpnStatus);
    expect(describeError("cell3 isn't connected. Use Live data").actions[0].label).toBe("Reconnect VPN");
    setVpnStatus({ state: "off" } as VpnStatus);
    expect(describeError(new Error("In Demo mode there's no node")).actions[0].label).toBe("Switch to Live…");
    expect(describeError("boom", { action: "Restart docker on cell2" }).title).toBe("Couldn't restart docker on cell2");
    expect(kindOf("SSH timeout: no reply from 192.168.50.98:22 in 15 s")).toBe("timeout");
  });
});

describe("redact", () => {
  it("hides the same secrets as the Rust side", () => {
    expect(redact("HF_TOKEN=hf_abcdefghijklmnopqrstuvwxyz0123456789 vllm serve")).toBe("HF_TOKEN=‹redacted› vllm serve");
    // The whole header value goes now, and the closing quote stays (redact_cases.json has the full list).
    expect(redact("curl -H 'Authorization: Bearer abc.def' x")).toBe("curl -H 'Authorization: ‹redacted›' x");
    expect(redact("key sk-ant-api03-abcdefgh and more")).toBe("key ‹redacted› and more");
    expect(redact("tool --password hunter2")).toBe("tool --password ‹redacted›");
    expect(redact("https://user:s3cret@github.com/x")).toBe("https://user:‹redacted›@github.com/x");
    expect(redact("pip install hf_transfer")).toBe("pip install hf_transfer");
    expect(redactEnv("OPENAI_API_KEY=sk-x")).toBe("OPENAI_API_KEY=‹redacted›");
    expect(redactEnv("NCCL_SOCKET_IFNAME=enp1s0f1np1")).toBe("NCCL_SOCKET_IFNAME=enp1s0f1np1");
  });
});

describe("Fleet output", () => {
  const out = (node: string, stdout: string, code = 0, state: NodeOutput["state"] = "ok"): NodeOutput => ({ node, stdout, stderr: "", code, state, ms: 10, startedMs: 0 });

  it("merges identical answers and marks the odd one out", () => {
    const g = groupOutputs([out("cell1", "580.95.05\n"), out("cell2", "580.95.05\n"), out("cell3", "580.95.05\n"), out("gx10-0000", "580.82.07\n")]);
    expect(g.map((x) => [x.nodes.join(","), x.differs])).toEqual([["cell1,cell2,cell3", false], ["gx10-0000", true]]);
  });

  it("doesn't flag anything when every node answers for itself", () => {
    const g = groupOutputs([out("cell1", "cell1\n"), out("cell2", "cell2\n")]);
    expect(g.every((x) => !x.differs)).toBe(true);
  });

  it("keeps failures apart from successes with the same text", () => {
    expect(groupOutputs([out("cell1", ""), out("cell2", "", 1, "failed")])).toHaveLength(2);
  });

  it("renders ANSI colours and drops other escapes", () => {
    const spans = ansiToSpans("\x1b[1;31mFAIL\x1b[0m ok \x1b[38;5;46mgreen\x1b[39m\x1b[K");
    expect(spans[0]).toEqual({ text: "FAIL", bold: true, fg: "var(--ansi-red)" });
    expect(spans[1]).toEqual({ text: " ok " });
    expect(spans[2]).toEqual({ text: "green", fg: "rgb(0, 255, 0)" });
    expect(stripAnsi("\x1b[32mup\x1b[0m")).toBe("up");
  });

  it("shows CSV from every node as one table, and copies as Markdown", () => {
    const t = csvTable([out("cell1", "NVIDIA GB10, 580.95.05, 71 %\n"), out("cell2", "NVIDIA GB10, 580.95.05, 12 %\n")]);
    expect(t!.width).toBe(3);
    expect(t!.rows.map((r) => [r.node, r.cells[2]])).toEqual([["cell1", "71 %"], ["cell2", "12 %"]]);
    expect(csvTable([out("cell1", "up 3 days\n")])).toBeNull();
    const md = asMarkdown("uptime", groupOutputs([out("cell1", "x\n"), out("cell2", "x\n")]), (id) => id);
    expect(md).toContain("### cell1, cell2 · exit 0");
  });
});

describe("Activity log grouping", () => {
  const entry = (id: string, over: Partial<ActivityEntry>): ActivityEntry => ({
    id, tsMs: 1000, updatedMs: 1000, node: "cell1", kind: "command", command: "", code: 0, ms: 1, changes: true, error: null, opId: null,
    title: "", source: "ui", actor: "ui", escalated: false, state: "ok", stdoutTail: null, stderrTail: null, simulated: false, class: "change", ...over,
  });

  it("groups an operation's commands under its own line with a count", () => {
    const groups = groupEntries([
      entry("a", { opId: "op1", kind: "op", title: "Restart 3 containers", tsMs: 1000 }),
      entry("b", { opId: "op1", command: "docker restart a", tsMs: 1001 }),
      entry("c", { opId: "op1", command: "docker restart b", tsMs: 1002, node: "cell2" }),
      entry("d", { opId: "op1", command: "docker restart c", tsMs: 1003, node: "cell3", state: "failed", code: 1 }),
      entry("e", { command: "uptime", tsMs: 5000 }),
    ]);
    expect(groups.map((g) => g.key)).toEqual(["e", "op1"]);
    expect(groups[1].head.title).toBe("Restart 3 containers");
    expect(groupSummary(groups[1])).toBe("2/3 ok · 1 failed");
    expect(groupSummary(groups[0])).toBeNull();
  });

  it("orders operations by when they started, not by their latest row (r4 F4)", () => {
    // An Admin session that began 41 minutes ago but relocked 5 minutes ago sat above a change made
    // 35 minutes ago (the Live tour, shot 56). By start, the change comes first.
    const now = 100 * 60_000;
    const groups = groupEntries([
      entry("u", { opId: "admin-1", kind: "op", class: "session", title: "Admin session", tsMs: now - 41 * 60_000 }),
      entry("l", { opId: "admin-1", kind: "app", node: "mac", title: "Admin mode locked", tsMs: now - 5 * 60_000 }),
      entry("c", { opId: "op2", kind: "op", title: "Restart vllm-head", tsMs: now - 35 * 60_000 }),
    ]);
    expect(groups.map((g) => g.key)).toEqual(["op2", "admin-1"]);
    expect(groups[1].ts).toBe(now - 41 * 60_000);
  });

  it("an empty log at rest says nothing changed, not that filters hide it", () => {
    expect(emptyAtRest("changes", "week")).toBe("Nothing changed on the cluster or this Mac in the last 7 days.");
    expect(emptyAtRest("changes", "today")).toBe("Nothing changed on the cluster or this Mac today.");
    expect(emptyAtRest("everything", "all")).toBe("Nothing ran yet.");
  });

  it("the Node column's short form", () => {
    expect(shortWhere("cell1–cell3")).toBe("c1–c3");
    expect(shortWhere("gx10-0000")).toBe("gx10");
    expect(shortWhere("This Mac")).toBe("Mac");
  });
});

describe("Images (r4 F6)", () => {
  const DAY = 86_400_000;
  const now = Date.UTC(2026, 9, 29, 8, 15);
  const img = (over: Partial<DockerImage>): DockerImage => ({
    node: "gx10-0000", id: "9a7e5d0c11f2", repository: "nvcr.io/nvidia/vllm", tag: "26.03.post1-py3", size: 21_400_000_000, createdMs: now - 225 * DAY,
    dangling: false, usedBy: [], running: 0, lastUsedMs: null, ...over,
  });

  it("counts an image unused when nothing has run it for 30 days, or ever here", () => {
    expect(isUnused(img({ lastUsedMs: now - 186 * DAY }), now)).toBe(true);
    expect(isUnused(img({ lastUsedMs: now - 2 * DAY }), now)).toBe(false);
    expect(isUnused(img({ running: 1, lastUsedMs: now }), now)).toBe(false);
    expect(isUnused(img({}), now)).toBe(true);
    expect(imageRef(img({}))).toBe("nvcr.io/nvidia/vllm:26.03.post1-py3");
    expect(imageRef(img({ dangling: true, repository: "<none>", tag: "<none>" }))).toBe("<none> (9a7e5d0c11f2)");
  });

  it("sums a node up in one line, as gx10-0000 was found", () => {
    const n: NodeImages = {
      node: "gx10-0000", total: 49_910_000_000, reclaimable: 30_310_000_000, error: null,
      images: [
        img({ repository: "vllm/vllm-openai", tag: "glm53-flash-arm64-cu130", size: 19_600_000_000, usedBy: ["glm53"], lastUsedMs: now - 2.4 * DAY }),
        img({ usedBy: ["vigilant_hofstadter"], lastUsedMs: now - 186.2 * DAY }),
        img({ repository: "nvcr.io/nvidia/cuda", tag: "13.0.1-devel-ubuntu24.04", size: 8_910_000_000, usedBy: ["a", "b", "c"], lastUsedMs: now - 186.4 * DAY }),
      ],
    };
    expect(nodeSummary("gx10-0000", n, now)).toBe("gx10-0000: 3 images · 46.5 GiB · 2 unused for 186 days");
    // As the Live check found it: three more images no container on gx10-0000 ever ran.
    const found = { ...n, images: [...n.images, img({ repository: "vllm/vllm-openai", tag: "gemma4-cu130" }), img({ tag: "x" }), img({ tag: "y" })] };
    expect(nodeSummary("gx10-0000", found, now)).toBe("gx10-0000: 6 images · 46.5 GiB · 2 unused for 186 days · 3 never used here");
    expect(nodeSummary("cell3", { node: "cell3", images: [], total: 0, reclaimable: null, error: "cell3: the cluster user can't use Docker (permission denied)." }, now))
      .toBe("cell3: the cluster user can't use Docker (permission denied).");
  });
});

describe("Containers", () => {
  it("names why a container stopped", () => {
    expect(exitReason(container("sglang-test", { state: "exited", exitCode: 137, oomKilled: true }))).toBe("137 · OOM-killed");
    expect(exitReason(container("x", { state: "exited", exitCode: 1 }))).toBe("exited 1");
    expect(exitReason(container("x"))).toBeNull();
    expect(applicable("remove", container("x"))).toBe(false);
    expect(applicable("remove", container("x", { state: "exited" }))).toBe(true);
    expect(applicable("unpause", container("x", { state: "paused" }))).toBe(true);
  });
});

describe("Workloads ports", () => {
  it("finds a port's owner from containers' mappings and the process list", () => {
    const n = node("cell1", [container("open-webui", { ports: ["0.0.0.0:3000->8080/tcp"] })], {
      processes: [{ pid: 2211, name: "python3", user: "root", cmd: "", cpuPct: 0, rss: 0, nice: 0, container: "vllm-head", unit: null, ports: [8000], gpuMem: 0 }],
    });
    expect(portOwner(n, { port: 3000, addr: "0.0.0.0", pid: null, process: null }).owner).toBe("container open-webui");
    expect(portOwner(n, { port: 8000, addr: "0.0.0.0", pid: null, process: null }).owner).toBe("container vllm-head");
    expect(portOwner(n, { port: 22, addr: "0.0.0.0", pid: null, process: null }).owner).toBeNull();
    expect(portOwner(n, { port: 22, addr: "0.0.0.0", pid: null, process: null }, [{ port: 22, addr: "0.0.0.0", pid: 1201, process: "sshd" }])).toEqual({ owner: "sshd (1201)", via: "sudo" });
  });
});

describe("Updates", () => {
  it("finds the version most nodes run", () => {
    expect(majority(["580.95.05", "580.95.05", "580.95.05", "580.82.07"])).toBe("580.95.05");
    expect(majority(["a", "b"])).toBeNull();
  });
});

describe("client tasks", () => {
  beforeEach(() => _resetTasksForTests());

  it("runs steps, reports progress and resolves waiters when it finishes", async () => {
    const t = startClientTask({ kind: "transfer", title: "Copy weights to cell2", nodes: ["cell2"] });
    t.step("Copying");
    t.progress(0.5, { rateBps: 1e9, etaS: 12 });
    expect(getTask(t.id)!.progress).toBe(0.5);
    const done = waitTask(t.id);
    t.finish(true, "141 GB copied");
    const finished = await done;
    expect(finished.state).toBe("ok");
    expect(finished.steps[0].state).toBe("ok");
    expect(finished.summary).toBe("141 GB copied");
    expect(useTasks).toBeTypeOf("function");
  });
});
