import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Endpoint } from "../lib/endpoints";
import { dependencyOrder, isNativeLoad, reloadSentence, restartAdvice, deploymentsOf } from "../lib/impact";
import type { JournalLine } from "../lib/opsApi";
import type { ChoiceOptions, ConfirmOptions } from "../lib/session";
import type { ClusterDeployment, ClusterSnapshot, Container, NodeSnapshot, Service } from "../lib/types";
import { groupsFooter, imageTail, shortImage } from "./Containers";
import { cellTone, failTone, focusLine, journalTime, nodeZone, stateReason } from "./Services";

// Confirmations are answered by stubs that record what they were asked (and cancel).
const asked: (ConfirmOptions | ChoiceOptions)[] = [];
vi.mock("../lib/session", async (importOriginal) => {
  const real = await importOriginal<typeof import("../lib/session")>();
  return {
    ...real,
    confirmAction: vi.fn(async (o: ConfirmOptions) => (asked.push(o), false)),
    confirmChoice: vi.fn(async (o: ChoiceOptions) => (asked.push(o), null)),
  };
});
let world: ClusterSnapshot | null = null;
let eps: Endpoint[] = [];
vi.mock("../lib/store", async (importOriginal) => ({ ...(await importOriginal<typeof import("../lib/store")>()), getCluster: () => world }));
vi.mock("../lib/endpoints", async (importOriginal) => ({ ...(await importOriginal<typeof import("../lib/endpoints")>()), getEndpoints: () => eps }));

const line = (text: string, priority = 6, tsMs: number | null = null): JournalLine => ({ time: "Oct 28 14:38:46", tsMs, priority, text });
const svc = (active: string, extra: Partial<Service> = {}): Service => ({ unit: "x.service", description: "", load: "loaded", active, sub: active === "active" ? "running" : "dead", enabled: "enabled", ...extra } as Service);

describe("Services", () => {
  it("points at the line that says why a unit failed, not systemd's summary", () => {
    const journal = [
      line("systemd[1]: Starting nvidia-dgx-telemetry.service - NVIDIA DGX telemetry..."),
      line("nvidia-dgx-telemetry[2211]: telemetry: cannot open /var/lib/nvidia-dgx-telemetry/state.db: Permission denied", 3),
      line("systemd[1]: nvidia-dgx-telemetry.service: Main process exited, code=exited, status=1/FAILURE", 5),
      line("systemd[1]: nvidia-dgx-telemetry.service: Failed with result 'exit-code'.", 3),
      line("systemd[1]: Failed to start nvidia-dgx-telemetry.service - NVIDIA DGX telemetry.", 3),
    ];
    expect(focusLine(journal)).toBe(1);
    // Only systemd's errors: the newest of them. No errors: the newest line.
    expect(focusLine([journal[0], journal[3], journal[4]])).toBe(2);
    expect(focusLine([journal[0], journal[2]])).toBe(1);
  });

  it("keeps running units quiet and colours failed, masked and out-of-step ones", () => {
    const running = svc("active");
    expect(cellTone(running, [running])).toBe("quiet");
    // A failed unit takes its alert's severity: a warning by default, red only when the rule says critical.
    expect(cellTone(svc("failed"), [])).toBe("warn");
    expect(cellTone(svc("failed"), [], "crit")).toBe("crit");
    expect(cellTone(svc("inactive", { load: "masked" }), [])).toBe("warn");
    // Inactive while another node runs it: amber. Inactive everywhere: plain.
    expect(cellTone(svc("inactive"), [svc("inactive"), running])).toBe("warn");
    expect(cellTone(svc("inactive"), [svc("inactive")])).toBe("plain");
  });

  it("shows journal times in local time with UTC in the tooltip", () => {
    const ms = Date.UTC(2026, 9, 28, 18, 38, 46);
    const t = journalTime({ tsMs: ms, time: "Oct 28 18:38:46" }, ms + 60_000);
    expect(t.text).toBe(new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" }));
    expect(t.title).toMatch(/2026-10-28 18:38:46 UTC$/);
  });

  it("reads journalctl's year-less times in the node's zone, from its systemctl times", () => {
    expect(nodeZone("Sun 2026-10-25 14:43:47 UTC; 3 days ago")).toBe("UTC");
    expect(nodeZone("Sun 2026-10-25 10:43:47 EDT")).toBe("EDT");
    expect(nodeZone(null)).toBeUndefined();
    const now = Date.UTC(2026, 9, 28, 19, 0, 0);
    const t = journalTime({ tsMs: null, time: "Oct 25 14:43:47" }, now, "UTC");
    expect(t.title).toMatch(/2026-10-25 14:43:47 UTC$/);
    // A December line read in January belongs to last year.
    const jan = Date.UTC(2027, 0, 2, 12, 0, 0);
    expect(journalTime({ tsMs: null, time: "Dec 31 23:59:01" }, jan, "UTC").title).toMatch(/2026-12-31 23:59:01 UTC$/);
  });
});

describe("Containers", () => {
  it("folds an image's middle path and keeps its name and tag", () => {
    expect(shortImage("nvcr.io/nvidia/vllm:26.05-py3")).toBe("nvcr.io/…/vllm:26.05-py3");
    expect(shortImage("ollama/ollama:0.12.6")).toBe("ollama/ollama:0.12.6");
    expect(imageTail("nvcr.io/nvidia/vllm:26.05-py3")).toBe("/vllm:26.05-py3".length);
    // A long last segment keeps its tag end, not all of it.
    expect(imageTail("lmsysorg/sglang:v0.5.4-cu130-spark")).toBe(20);
  });

  it("counts groups in the Deployments footer", () => {
    expect(groupsFooter(6, 2)).toBe("6 containers in 2 groups");
    expect(groupsFooter(1, 1)).toBe("1 container in 1 group");
  });
});


// ---- A bulk restart of a model's containers (D4)

const container = (name: string, over: Partial<Container> = {}): Container => ({
  id: `${name}-${over.labels?.node ?? ""}0123456789ab`.slice(0, 12), name, image: "nvcr.io/nvidia/vllm:26.05-py3", state: "running", startedAt: 0, finishedAt: 0,
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
/** The Demo world: llama-3.3-70b across cell1–cell3 (started outside SparkBox), Ollama on gx10-0000. */
function demoWorld(): ClusterSnapshot {
  const llama = { project: "llama-70b" };
  return {
    tsMs: 0, demo: true, switches: [],
    nodes: [
      node("cell1", [
        container("vllm-head", { ...llama, command: "vllm serve /models/meta-llama/Llama-3.3-70B-Instruct --served-model-name llama-3.3-70b --port 8000" }),
        container("open-webui", { restartPolicy: "unless-stopped" }),
      ]),
      node("cell2", [container("vllm-worker", { ...llama, labels: { node: "2" }, command: "ray start --block" })]),
      node("cell3", [container("vllm-worker", { ...llama, labels: { node: "3" }, command: "ray start --block" })]),
      node("gx10-0000", [container("ollama", { project: "ollama", restartPolicy: "unless-stopped", command: "/bin/ollama serve", labels: { "sparkbox.model": "qwen3:32b" } })]),
    ],
  };
}
function llamaEndpoint(over: Partial<Endpoint> = {}): Endpoint {
  const w = demoWorld();
  return {
    id: "cell1-vllm-8000", engine: "vllm", models: ["llama-3.3-70b"], nodes: ["cell1", "cell2", "cell3"], headNode: "cell1", port: 8000,
    url: "http://cell1:8000/v1", managedBy: "external", source: "discovered", health: "ready", containerIds: [w.nodes[0].containers[0].id],
    containerNames: ["vllm-head"], ...over,
  } as Endpoint;
}
const sel = (w: ClusterSnapshot, picks: [string, string][]) =>
  picks.map(([n, name]) => {
    const c = w.nodes.find((x) => x.id === n)!.containers.find((x) => x.name === name)!;
    return { node: n, id: c.id, name: c.name };
  });

describe("restarting a model's containers", () => {
  it("defaults to the model restart in order when all of llama's containers are selected", () => {
    const w = demoWorld();
    const a = restartAdvice(w, sel(w, [["cell1", "vllm-head"], ["cell2", "vllm-worker"], ["cell3", "vllm-worker"]]), [llamaEndpoint()]);
    expect(a.preferModel).toBe(true);
    expect(a.model?.label).toBe("Restart llama-3.3-70b in order");
    expect(a.containersLabel).toBe("Restart 3 containers anyway");
    const text = a.items.map((i) => i.text).join(" ");
    expect(text).toContain("These 3 containers are llama-3.3-70b's pipeline (head on cell1, workers on cell2 and cell3)");
    expect(text).toContain("The default action restarts this model in order. The alternative restarts the selected containers only.");
    expect(text).not.toMatch(/reloads?\b/);
  });

  it("warns that a part of the pipeline breaks it, and offers the model restart", () => {
    const w = demoWorld();
    const a = restartAdvice(w, sel(w, [["cell2", "vllm-worker"]]), [llamaEndpoint()]);
    expect(a.preferModel).toBe(false);
    expect(a.containersLabel).toBe("Restart vllm-worker");
    expect(a.model?.label).toBe("Restart llama-3.3-70b in order");
    expect(a.items[0].text).toContain("Restarting the head makes the workers exit, and restarting a worker stops the model until the head restarts.");
  });

  it("says Keep running restarts it when it's on, and offers nothing it can't do", () => {
    const w = demoWorld();
    const kept = restartAdvice(w, sel(w, [["cell2", "vllm-worker"]]), [llamaEndpoint({ managedBy: "sparkbox", deploymentId: "llama-70b", supervisor: { enabled: true, state: "watching", restarts: 0, nextRetryMs: null } })]);
    expect(kept.items.map((i) => i.text)).toContain("Keep running then restarts it in order, while SparkBox is open on this Mac.");
    // A manual endpoint SparkBox can't restart: the warning stays, the button doesn't.
    const manual = restartAdvice(w, sel(w, [["cell2", "vllm-worker"]]), [llamaEndpoint({ source: "manual" })]);
    expect(manual.model).toBeNull();
    expect(manual.items[0].tone).toBe("crit");
  });

  it("says a one-node model is offline until it has loaded again, and leaves other containers out", () => {
    const w = demoWorld();
    const a = restartAdvice(w, sel(w, [["gx10-0000", "ollama"]]), []);
    expect(a.model).toBeNull();
    expect(a.items.map((i) => i.text)).toEqual(["qwen3:32b on gx10-0000 stops answering until it has loaded again."]);
    expect(restartAdvice(w, sel(w, [["cell1", "open-webui"]]), []).items).toEqual([]);
  });

  it("never promises a reload in the restart confirm of a node", () => {
    const w = demoWorld();
    const d = deploymentsOf(w).find((x) => x.id === "llama-70b")!;
    expect(reloadSentence(d, [llamaEndpoint()], w)).toBe("SparkBox didn't start this model: once the node is back, restart it yourself (Restart llama-3.3-70b…).");
    expect(reloadSentence(d, [llamaEndpoint({ managedBy: "sparkbox", deploymentId: "llama-70b", supervisor: { enabled: false, state: "watching", restarts: 0, nextRetryMs: null } })], w))
      .toBe("Keep running is off: once the node is back, restart it (Restart llama-3.3-70b…).");
    const ollama = deploymentsOf(w).find((x) => x.nodes[0] === "gx10-0000")!;
    expect(reloadSentence(ollama, [], w)).toBe("Its container restarts by itself when the node is back (restart policy) and loads the model again.");
  });

  it("asks with the model restart as the default button and the containers as the other", async () => {
    const { runContainerAction } = await import("../lib/workloadCommands");
    world = demoWorld();
    eps = [llamaEndpoint()];
    asked.length = 0;
    const targets = [["cell1", "vllm-head"], ["cell2", "vllm-worker"], ["cell3", "vllm-worker"]].map(([n, name]) => {
      const nd = world!.nodes.find((x) => x.id === n)!;
      return { node: nd, c: nd.containers.find((c) => c.name === name)! };
    });
    await runContainerAction("restart", targets, { origin: "containers" });
    expect(asked).toHaveLength(1);
    const q = asked[0] as ChoiceOptions;
    expect(q.title).toBe("Restart 3 containers on cell1–cell3?");
    expect(q.confirmLabel).toBe("Restart llama-3.3-70b in order");
    expect(q.alternativeLabel).toBe("Restart 3 containers anyway");
    expect(q.danger).toBe(false);
    // One worker: the containers' restart is the default and the model restart the other.
    asked.length = 0;
    await runContainerAction("restart", [targets[1]], { origin: "containers" });
    expect((asked[0] as ChoiceOptions).confirmLabel).toBe("Restart vllm-worker");
    expect((asked[0] as ChoiceOptions).alternativeLabel).toBe("Restart llama-3.3-70b in order");
  });
});

describe("the order of a bulk action on a deployment's containers", () => {
  const members = (native: boolean) => [{ role: "worker" as const, n: 2, native }, { role: "head" as const, n: 1, native }, { role: "worker" as const, n: 3, native }];
  const roles = (list: { role: string }[]) => list.map((m) => m.role);

  it("starts SparkBox's own load's workers first, and stops its head first", () => {
    for (const up of ["start", "restart", "unpause"]) expect(roles(dependencyOrder(members(true), up))).toEqual(["worker", "worker", "head"]);
    for (const down of ["stop", "kill", "pause"]) expect(roles(dependencyOrder(members(true), down))).toEqual(["head", "worker", "worker"]);
  });

  it("leaves a server SparkBox didn't start as it was: its head starts first and stops last", () => {
    for (const up of ["start", "restart", "unpause"]) expect(roles(dependencyOrder(members(false), up))).toEqual(["head", "worker", "worker"]);
    for (const down of ["stop", "kill", "pause"]) expect(roles(dependencyOrder(members(false), down))).toEqual(["worker", "worker", "head"]);
    // Containers lists a group head first with no flag at all.
    expect(roles(dependencyOrder([{ role: "worker" as const }, { role: "head" as const }], "start"))).toEqual(["head", "worker"]);
  });

  it("orders each by its own rule when a selection holds both", () => {
    const mixed = [{ role: "head" as const, n: "ours", native: true }, { role: "worker" as const, n: "ours", native: true }, { role: "head" as const, n: "theirs" }, { role: "worker" as const, n: "theirs" }];
    expect(dependencyOrder(mixed, "start").map((m) => `${m.n} ${m.role}`)).toEqual(["theirs head", "theirs worker", "ours worker", "ours head"]);
    expect(dependencyOrder(mixed, "stop").map((m) => `${m.n} ${m.role}`)).toEqual(["theirs worker", "theirs head", "ours head", "ours worker"]);
  });

  it("knows SparkBox's vLLM loads by the launcher's label: SGLang's start head first, as Rust starts them", () => {
    expect(isNativeLoad({ labels: { "sparkbox.deployment": "d1", "sparkbox.recipe": "generic" } })).toBe(true);
    expect(isNativeLoad({ labels: { "sparkbox.deployment": "d1", "sparkbox.role": "head" } })).toBe(false);
    expect(isNativeLoad({ labels: { "sparkbox.model": "qwen3:32b" } })).toBe(false);
    expect(isNativeLoad({})).toBe(false);
  });

  it("shows that order before it asks, on SparkBox's own load", async () => {
    const { runContainerAction } = await import("../lib/workloadCommands");
    const labels = (role: string, rank: number) => ({ "sparkbox.instance": "dev", "sparkbox.deployment": "d1", "sparkbox.recipe": "gemma-3-27b-it", "sparkbox.role": role, "sparkbox.rank": String(rank) });
    world = { tsMs: 0, demo: false, switches: [], nodes: [node("cell1", [container("sparkbox-d1-r0", { labels: labels("head", 0) })]), node("cell2", [container("sparkbox-d1-r1", { labels: labels("worker", 1) })])] } as ClusterSnapshot;
    eps = [];
    const targets = world.nodes.map((nd) => ({ node: nd, c: nd.containers[0] }));
    const order = async (action: "stop" | "restart") => {
      asked.length = 0;
      await runContainerAction(action, targets, { origin: "containers" });
      return renderToStaticMarkup(asked[0].body as ReactElement);
    };
    expect(await order("stop")).toContain("Container order: sparkbox-d1-r0 (cell1) → sparkbox-d1-r1 (cell2).");
    expect(await order("restart")).toContain("Container order: sparkbox-d1-r1 (cell2) → sparkbox-d1-r0 (cell1).");
  });
});

describe("Services: one severity per failed unit", () => {
  const unit = "nvidia-dgx-telemetry.service";
  const alert = (severity: "warn" | "crit") => ({
    id: `unit-failed:cell1:${unit}`, rule: "unit-failed", category: "containers" as const, severity, state: "firing" as const,
    entity: { kind: "service" as const, id: unit, node: "cell1", switch: null, label: `${unit} on cell1` },
    message: `${unit} failed`, summary: `nvidia-dgx-telemetry failed on cell1`, detail: null, value: 0, sinceMs: 0, firingSinceMs: 0,
    resolvedMs: null, ackedMs: null, mutedUntilMs: null,
  });

  it("reads the alert's severity, a warning when there's none", () => {
    expect(failTone({ alerts: [alert("crit")] }, "cell1", unit)).toBe("crit");
    expect(failTone({ alerts: [alert("warn")] }, "cell1", unit)).toBe("warn");
    expect(failTone({ alerts: [] }, "cell1", unit)).toBe("warn");
    expect(failTone({ alerts: [alert("crit")] }, "cell2", unit)).toBe("warn");
  });

  it("names the state when an action doesn't apply", () => {
    const on = svc("active", { unit });
    expect(stateReason("start", [on])).toBe("Already running");
    expect(stateReason("stop", [svc("inactive")])).toBe("It isn't running");
    expect(stateReason("restart", [svc("inactive", { load: "masked" })])).toBe("It's masked: unmask it first");
    expect(stateReason("enable", [on])).toBe("Already starts at boot");
  });

  it("renders a warning-level failure with no critical colour anywhere in the window", async () => {
    const { createElement: h } = await import("react");
    const { act, render, screen } = await import("@testing-library/react");
    const { mockTauri } = await import("../test/tauri");
    const { resetStore, startClusterStore } = await import("../lib/store");
    const { applyHealth } = await import("../lib/health");
    const { Desktop, openWindow, WindowManagerProvider } = await import("../wm/WindowManager");
    const { Services } = await import("./Services");
    world = demoWorld();
    const failedSvc = { unit, description: "NVIDIA DGX telemetry", load: "loaded", active: "failed", sub: "failed", enabled: "enabled" };
    const ssh = { unit: "ssh.service", description: "OpenBSD Secure Shell server", load: "loaded", active: "active", sub: "running", enabled: "enabled" };
    mockTauri({
      get_snapshot: () => world,
      services_list: ({ node }: { node: string }) => (node === "cell1" ? [failedSvc, ssh] : [ssh]),
      service_detail: () => ({
        unit, description: "NVIDIA DGX telemetry", loaded: "/lib/systemd/system/nvidia-dgx-telemetry.service", active: "failed (Result: exit-code)",
        activeSince: "Sun 2026-10-25 14:43:47 UTC; 3 days ago", mainPid: null, result: "exit-code", memory: null, cpu: null, tasks: null, statusText: "",
        journal: [
          { time: "Oct 25 14:43:46", tsMs: null, priority: 6, text: "systemd[1]: Starting nvidia-dgx-telemetry.service - NVIDIA DGX telemetry..." },
          { time: "Oct 25 14:43:46", tsMs: null, priority: 3, text: "nvidia-dgx-telemetry[2211]: telemetry: cannot open /var/lib/nvidia-dgx-telemetry/state.db: Permission denied" },
          { time: "Oct 25 14:43:47", tsMs: null, priority: 3, text: "systemd[1]: nvidia-dgx-telemetry.service: Failed with result 'exit-code'." },
        ],
      }),
      health_get: () => ({ tsMs: Date.now(), alerts: [alert("warn")], summary: { warn: 1, crit: 0, pending: 0, muted: 0 }, nodes: {}, switches: {}, lastAlertMs: null }),
    });
    resetStore();
    await startClusterStore();
    applyHealth({ tsMs: Date.now(), alerts: [alert("warn")], summary: { warn: 1, crit: 0, pending: 0, muted: 0 }, nodes: {}, switches: {}, lastAlertMs: null });
    render(h(WindowManagerProvider, { sizeOf: (): [number, number] => [1180, 680], isKnown: () => true, children: h(Desktop, { Body: ({ params }: { params: Record<string, string> }) => h(Services, { params }) }) }));
    await act(async () => openWindow("services"));
    await screen.findByText(/Journal · last 3 lines/);
    const win = document.querySelector(".services-window")!;
    // Amber where it shows: the cell, the row, the banner, the Result and Error rows, the journal's errors.
    expect(win.querySelector(".svc-cell.warn")).not.toBeNull();
    expect(win.querySelector("tr.row-warn")).not.toBeNull();
    expect(win.querySelector(".notice.warn")).not.toBeNull();
    expect(win.querySelectorAll("dd.warn-text").length).toBe(2);
    expect(win.querySelectorAll(".badge.warn").length).toBe(2);
    expect(win.querySelector(".svc-journal-scroll.wrap")).not.toBeNull();
    // And red nowhere.
    const red = win.querySelectorAll(".crit, .crit-text, .row-crit, .sdot-error, .sdot-crit, .notice.error, .tone-crit, li.err:not(.tone-warn)");
    expect([...red].map((e) => e.className)).toEqual([]);
  });
});

describe("an offline node keeps its rows (D9)", () => {
  it("lists the containers and processes it last reported, marked stale, per data source", async () => {
    const { withLastKnown, _forgetLastKnownForTests, staleState } = await import("../lib/workloadCommands");
    _forgetLastKnownForTests();
    const w = demoWorld();
    w.nodes[2].processes = [{ pid: 4242, name: "ray", cmd: "ray start --block", user: "root", cpuPct: 3, rss: 1, gpuMem: 0, ports: [], container: "vllm-worker", unit: null, nice: 0, state: "S" } as never];
    withLastKnown({ ...w, tsMs: 1000 });
    // cell3 goes offline: its snapshot has nothing in it any more.
    const away = demoWorld();
    away.nodes[2] = { ...away.nodes[2], status: "offline", containers: [], processes: [] };
    const listed = withLastKnown({ ...away, tsMs: 5000 });
    const cell3 = listed.find((l) => l.node.id === "cell3")!;
    expect(cell3.stale).toBe(true);
    expect(cell3.knownAtMs).toBe(1000);
    expect(cell3.node.containers.map((c) => c.name)).toEqual(["vllm-worker"]);
    expect(cell3.node.processes.map((p) => p.pid)).toEqual([4242]);
    expect(staleState(cell3.node)).toBe("unknown · cell3 offline");
    expect(listed.find((l) => l.node.id === "cell1")!.stale).toBe(false);
    // Live never sees what Demo reported.
    expect(withLastKnown({ ...away, demo: false, tsMs: 6000 }).find((l) => l.node.id === "cell3")!.node.containers).toEqual([]);
  });

  it("shows them greyed in Containers, 'unknown · cell3 offline', and won't act on them", async () => {
    const { createElement: h } = await import("react");
    const { act, render, screen } = await import("@testing-library/react");
    const { mockTauri, emitMock } = await import("../test/tauri");
    const { resetStore, startClusterStore } = await import("../lib/store");
    const { _forgetLastKnownForTests } = await import("../lib/workloadCommands");
    const { Desktop, openWindow, WindowManagerProvider } = await import("../wm/WindowManager");
    const { Containers } = await import("./Containers");
    _forgetLastKnownForTests();
    let snap = demoWorld();
    snap.tsMs = Date.now();
    mockTauri({ get_snapshot: () => snap });
    resetStore();
    await startClusterStore();
    render(h(WindowManagerProvider, { sizeOf: (): [number, number] => [1160, 440], isKnown: () => true, children: h(Desktop, { Body: ({ params }: { params: Record<string, string> }) => h(Containers, { params }) }) }));
    await act(async () => openWindow("containers"));
    expect(await screen.findAllByText("vllm-worker")).toHaveLength(2);
    // cell3 stops answering.
    snap = demoWorld();
    snap.tsMs = Date.now() + 1000;
    snap.nodes[2] = { ...snap.nodes[2], status: "offline", containers: [], processes: [] };
    await act(async () => {
      await emitMock("cluster://snapshot", snap);
    });
    const stale = await screen.findByText("unknown · cell3 offline");
    const row = stale.closest("tr")!;
    expect(row).toHaveClass("row-dim");
    expect(row).toHaveTextContent("vllm-worker");
    expect(screen.getAllByText("vllm-worker")).toHaveLength(2);
    // Grouped by deployment, the footer counts containers, never "8 rows".
    const { fireEvent } = await import("@testing-library/react");
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: /Deployments/ }));
    });
    const footers = [...document.querySelectorAll(".containers-window .table-footer")].map((f) => f.textContent ?? "");
    expect(footers).toHaveLength(1);
    expect(footers[0]).toMatch(/^5 containers in 3 groups/);
    expect(footers[0]).not.toMatch(/rows/);
  });
});

describe("Activity log tells one story (D10)", () => {
  const entry = (over: Partial<import("../lib/opsApi").ActivityEntry>): import("../lib/opsApi").ActivityEntry => ({
    id: Math.random().toString(36).slice(2), tsMs: 1_000_000, updatedMs: 1_000_000, node: "cell1", kind: "command", command: "docker restart vllm-head",
    code: 0, ms: 1200, changes: true, error: null, opId: null, title: "", source: "ui", actor: "ui", escalated: false, state: "ok",
    stdoutTail: null, stderrTail: null, simulated: true, class: "change", ...over,
  });

  it("uses the outcome words of Tasks, and never puts 3/3 ok beside failed", async () => {
    const { groupEntries, groupState, groupSummary, outcomeOf } = await import("./ActivityLog");
    expect(["ok", "warning", "failed"].map((s) => outcomeOf(s as never).word)).toEqual(["ok", "ok · warning", "failed"]);
    const head = entry({ id: "op1", kind: "op", opId: "op1", title: "Restart llama-3.3-70b in order", command: "", state: "failed", error: "Verify: it didn't answer within 3 min" });
    const cmds = ["cell1", "cell2", "cell3"].map((n) => entry({ node: n, opId: "op1" }));
    const [g] = groupEntries([head, ...cmds]);
    expect(outcomeOf(groupState(g)).word).toBe("failed");
    expect(groupSummary(g)).toBe("3 commands");
    const okHead = entry({ ...head, id: "op2", opId: "op2", state: "warning", error: null });
    const [g2] = groupEntries([okHead, ...cmds.map((c) => ({ ...c, id: `${c.id}b`, opId: "op2" }))]);
    expect(outcomeOf(groupState(g2)).word).toBe("ok · warning");
    expect(groupSummary(g2)).toBe("3/3 ok");
  });

  it("files under Changes by audit class: changes, Mac changes, Admin sessions and root shells, not reads or log views", async () => {
    const { foldAdminSessions, groupEntries, isChange } = await import("./ActivityLog");
    const list = [
      entry({ id: "a", class: "change" }),
      entry({ id: "b", class: "read", changes: false, kind: "terminal", title: "Open vllm-head's logs on cell1" }),
      entry({ id: "c", class: "session", changes: false, kind: "terminal", title: "Open a shell on cell1" }),
      entry({ id: "d", class: "session", changes: true, kind: "terminal", escalated: true, title: "Open a root shell on cell2" }),
      entry({ id: "e", class: "probe", changes: false, source: "system" }),
      entry({ id: "f", node: "mac", kind: "app", class: "app", title: "Settings changed" }),
      entry({ id: "u", node: "mac", kind: "app", class: "app", title: "Admin mode unlocked", tsMs: 900_000 }),
      entry({ id: "l", node: "mac", kind: "app", class: "app", title: "Admin mode locked", command: "No changes for 15 minutes", tsMs: 990_000 }),
    ];
    const groups = groupEntries(foldAdminSessions(list, false));
    const changes = groups.filter(isChange).map((g) => g.head.title || g.head.id);
    expect(changes).toContain("Settings changed");
    expect(changes).toContain("Open a root shell on cell2");
    expect(changes.some((t) => t.startsWith("Admin session"))).toBe(true);
    expect(changes).not.toContain("Open vllm-head's logs on cell1");
    expect(changes).not.toContain("Open a shell on cell1");
    expect(groups.filter(isChange).some((g) => g.head.id === "e")).toBe(false);
  });

  it("reads a shell as one session row with its span", async () => {
    const { shellTitle } = await import("./ActivityLog");
    const at = new Date(2026, 9, 29, 12, 3).getTime();
    const open = entry({ kind: "terminal", class: "session", tsMs: at, ms: 38 * 60_000, state: "ok" });
    expect(shellTitle(open, (id) => id)).toMatch(/^Shell on cell1 · 12:03\s?(PM)?–12:41\s?(PM)?$/);
    expect(shellTitle({ ...open, escalated: true, state: "running" }, (id) => id)).toMatch(/^Root shell on cell1 · since 12:03/);
  });
});

describe("Tasks with the detail open (D7)", () => {
  it("drops the Nodes and Outcome columns instead of cutting them, and says which window started it", async () => {
    const { createElement: h } = await import("react");
    const { act, render, screen, within } = await import("@testing-library/react");
    const { mockTauri } = await import("../test/tauri");
    const { _resetTasksForTests, startClientTask } = await import("../lib/tasks");
    const { Desktop, openWindow, WindowManagerProvider } = await import("../wm/WindowManager");
    const { TasksWindow } = await import("./TasksWindow");
    mockTauri({ tasks_list: () => [] });
    _resetTasksForTests();
    render(h(WindowManagerProvider, { sizeOf: (): [number, number] => [1040, 600], isKnown: () => true, titleOf: (w: { kind: string }) => (w.kind === "tasks" ? "Tasks" : "Containers"), children: h(Desktop, { Body: ({ kind, params }: { kind: string; params: Record<string, string> }) => (kind === "tasks" ? h(TasksWindow, { params }) : h("span", null, kind)) }) }));
    await act(async () => openWindow("containers"));
    const t = startClientTask({ kind: "container", title: "Restart 3 containers", nodes: ["cell1", "cell2", "cell3"], origin: "containers" });
    t.finish(false, "vllm-worker on cell3 didn't stop");
    await act(async () => openWindow("tasks"));
    const headers = () => screen.getAllByRole("columnheader").map((c) => c.textContent);
    expect(headers()).toEqual(expect.arrayContaining(["Nodes", "Outcome"]));
    const { fireEvent } = await import("@testing-library/react");
    await act(async () => {
      fireEvent.mouseDown(screen.getByText("Restart 3 containers"));
    });
    expect(headers()).not.toContain("Nodes");
    expect(headers()).not.toContain("Outcome");
    const detail = screen.getByLabelText("Task details");
    expect(within(detail).getByText("Started from").nextElementSibling).toHaveTextContent("Containers");
  });
});

describe("names, reasons and verbs (D8)", () => {
  it("offers Start, not Restart, for a stopped container in ⌘K, and Resume for a paused one", async () => {
    const { workloadCommands } = await import("../lib/workloadCommands");
    const w = demoWorld();
    w.nodes[3].containers.push(container("sglang-test", { state: "exited", exitCode: 137 }), container("comfy", { state: "paused" }));
    const titles = workloadCommands(w).map((c) => c.title);
    expect(titles).toContain("Start sglang-test on gx10-0000");
    expect(titles).not.toContain("Restart sglang-test on gx10-0000…");
    expect(titles).toContain("Resume comfy on gx10-0000");
    expect(titles).toContain("Restart vllm-head on cell1…");
    // "restart sglang" still finds the right thing to do.
    expect(workloadCommands(w).find((c) => c.title === "Start sglang-test on gx10-0000")?.verbs).toContain("restart");
  });

  it("says why a container action is greyed, naming the state", async () => {
    const { notApplicable } = await import("../lib/workloadCommands");
    const running = container("a");
    const stopped = container("b", { state: "exited" });
    const paused = container("c", { state: "paused" });
    expect(notApplicable("start", [running])).toBe("Already running");
    expect(notApplicable("remove", [running])).toBe("Stop it first");
    expect(notApplicable("remove", [running, container("d")])).toBe("Stop them first");
    expect(notApplicable("stop", [stopped])).toBe("It isn't running (exited)");
    expect(notApplicable("unpause", [running])).toBe("It isn't paused");
    expect(notApplicable("pause", [paused])).toBe("Already paused");
    expect(notApplicable("start", [paused])).toBe("It's paused: resume it");
  });
});

describe("Workloads holds still while it's read (N10)", () => {
  it("keeps the order while held and re-sorts at most every 5 s", async () => {
    const { StableSort } = await import("./Workloads");
    type R = { key: string; cpu: number };
    const s = new StableSort<R>();
    let rows: R[] = [{ key: "a", cpu: 9 }, { key: "b", cpu: 5 }];
    const sorted = () => [...rows].sort((x, y) => (by(y) as number) - (by(x) as number)).map((r) => r.key);
    const by = s.by("cpu", (r) => r.cpu);
    s.refresh(false, 0);
    expect(sorted()).toEqual(["a", "b"]);
    rows = [{ key: "a", cpu: 1 }, { key: "b", cpu: 50 }];
    s.refresh(true, 6_000);
    // Held: b's jump doesn't move it.
    expect(sorted()).toEqual(["a", "b"]);
    s.refresh(false, 3_000);
    // Unheld, but the order is under 5 s old.
    expect(sorted()).toEqual(["a", "b"]);
    s.refresh(false, 7_000);
    // Re-sorted once the pointer left and 5 s passed.
    expect(sorted()).toEqual(["b", "a"]);
  });

  it("words a selected process that left the list", async () => {
    const { goneWords } = await import("./Workloads");
    expect(goneWords({ goneMs: 1_000 }, 13_000)).toBe("exited · last seen 12 s ago");
    expect(goneWords({ goneMs: 1_000, cut: true }, 4_000)).toBe("not among the 200 busiest · last seen 3 s ago");
  });
});

describe("Services as they really are (N9)", () => {
  it("reads idle units grey and keeps amber for a real stop", async () => {
    const { cellTone, cellWord, idleWords, isIdle } = await import("./Services");
    const unit = (p: Partial<Service>) => ({ unit: "x.service", description: "", load: "loaded", active: "inactive", sub: "dead", enabled: "enabled", ...p } as Service);
    // gx10-0000's anacron: a oneshot a timer starts, inactive while cell1–cell3 run theirs.
    const anacron = unit({ unit: "anacron.service", kind: "oneshot", triggeredBy: ["anacron.timer"] });
    const running = unit({ active: "active", sub: "running", kind: "simple" });
    expect(isIdle(anacron)).toBe(true);
    expect(cellTone(anacron, [running, running, running, anacron])).toBe("idle");
    expect(idleWords(anacron)).toBe("Idle · runs on a timer");
    expect(cellWord(anacron)).toBe("idle");
    expect(idleWords(unit({ kind: "oneshot" }))).toBe("Idle · runs when needed");
    expect(cellTone(unit({ enabled: "static", kind: "simple" }), [])).toBe("idle");
    // networkd-dispatcher on gx10-0000: its condition wasn't met, so it never started.
    expect(cellTone(unit({ kind: "dbus", condition: false }), [running])).toBe("idle");
    // An enabled daemon that's down is worth a look; a disabled one isn't.
    expect(cellTone(unit({ kind: "simple" }), [])).toBe("warn");
    expect(cellTone(unit({ kind: "simple", enabled: "disabled" }), [running])).toBe("plain");
    expect(cellTone(unit({ active: "failed", sub: "failed", kind: "oneshot" }), [])).toBe("warn");
  });

  it("draws a unit stuck in activating neutral, as a real Spark's plymouth-quit-wait is from boot on", async () => {
    const { createElement: h } = await import("react");
    const { act, render, screen } = await import("@testing-library/react");
    const { mockTauri } = await import("../test/tauri");
    const { resetStore, startClusterStore } = await import("../lib/store");
    const { Desktop, openWindow, WindowManagerProvider } = await import("../wm/WindowManager");
    const { Services } = await import("./Services");
    world = demoWorld();
    // What services_poll read from cell1 on Oct 29 (systemctl show).
    const plymouth = { unit: "plymouth-quit-wait.service", description: "Hold until boot process finishes up", load: "loaded", active: "activating", sub: "start", enabled: "static", kind: "oneshot", triggeredBy: [], result: "success", condition: true };
    const ssh = { unit: "ssh.service", description: "OpenBSD Secure Shell server", load: "loaded", active: "active", sub: "running", enabled: "enabled", kind: "notify", triggeredBy: [], result: "success", condition: true };
    mockTauri({
      get_snapshot: () => world,
      services_poll: () => ({ units: [plymouth, ssh], files: [] }),
      health_get: () => ({ tsMs: Date.now(), alerts: [], summary: { warn: 0, crit: 0, pending: 0, muted: 0 }, nodes: {}, switches: {}, lastAlertMs: null }),
    });
    resetStore();
    await startClusterStore();
    render(h(WindowManagerProvider, { sizeOf: (): [number, number] => [1180, 680], isKnown: () => true, children: h(Desktop, { Body: ({ params }: { params: Record<string, string> }) => h(Services, { params }) }) }));
    await act(async () => openWindow("services"));
    await screen.findAllByLabelText(/plymouth-quit-wait.service on cell1: activating/);
    const win = document.querySelector(".services-window")!;
    expect(win.querySelector("tr.row-warn, .svc-cell.warn, .dot.connecting, .sdot-connecting, .notice.warn")).toBeNull();
  });

  it("keeps a unit listed for 5 minutes after a read misses it, so the count holds", async () => {
    const { sticky, withFiles } = await import("./Services");
    const seen = new Map();
    const a = { unit: "a.service", description: "", load: "loaded", active: "active", sub: "running", enabled: "enabled" } as Service;
    const b = { ...a, unit: "anacron.service" };
    expect(sticky([a, b], seen, 0).length).toBe(2);
    expect(sticky([a], seen, 60_000).map((s) => s.unit)).toEqual(["a.service", "anacron.service"]);
    expect(sticky([a], seen, 6 * 60_000).map((s) => s.unit)).toEqual(["a.service"]);
    const files = withFiles([a], [["a.service", "enabled"], ["ssh.service", "disabled"], ["getty@.service", "enabled"]]);
    expect(files.map((s) => `${s.unit} ${s.active} ${s.enabled}`)).toEqual(["a.service active enabled", "ssh.service inactive disabled"]);
  });

  it("keeps a unit in the Active view for 5 minutes after it finishes", async () => {
    const { inActiveView } = await import("./Services");
    const shown = new Map<string, number>();
    const unit = (active: string) => ({ unit: "anacron.service", description: "", load: "loaded", active, sub: active === "active" ? "running" : "dead", enabled: "enabled" } as Service);
    expect(inActiveView("anacron.service", [unit("active"), unit("inactive")], shown, 0)).toBe(true);
    expect(inActiveView("anacron.service", [unit("inactive"), unit("inactive")], shown, 4 * 60_000)).toBe(true);
    expect(inActiveView("anacron.service", [unit("inactive"), unit("inactive")], shown, 6 * 60_000)).toBe(false);
    expect(inActiveView("never.service", [unit("inactive")], shown, 0)).toBe(false);
  });
});

it("keeps absent nodes and never refreshes last-known rows from a stale report", async () => {
  const { withLastKnown, _forgetLastKnownForTests, staleState } = await import("../lib/workloadCommands");
  _forgetLastKnownForTests();
  const snap = demoWorld();
  withLastKnown({ ...snap, tsMs: 1000 });
  const missing = withLastKnown({ ...snap, nodes: snap.nodes.filter((n) => n.id !== "cell3"), tsMs: 2000 });
  expect(missing.find((n) => n.node.id === "cell3")).toMatchObject({ stale: true, knownAtMs: 1000 });
  const stale = { ...snap.nodes[2], stale: true, frameAgeMs: null, containers: [], processes: [] };
  const row = withLastKnown({ ...snap, nodes: [stale], tsMs: 3000 }).find((n) => n.node.id === stale.id)!;
  expect(row).toMatchObject({ stale: true, knownAtMs: 1000 });
  expect(row.node.containers).not.toHaveLength(0);
  expect(staleState(row.node)).toContain("not reporting");
  const back = withLastKnown({ ...snap, tsMs: 4000 });
  expect(back.every((n) => !n.stale)).toBe(true);
});

it("never matches an unobserved deployment member to an unrelated container", () => {
  const w = demoWorld();
  const unknown: ClusterDeployment = {
    id: "unobserved", label: "Saved load", model: "/models/saved", served: "saved", engine: "vllm", image: "local/image",
    nodes: ["cell1"], members: [{ node: "cell1", id: "", container: "saved-r0", rank: 0, role: "head", state: "unknown", exitCode: 0, oom: false, startedMs: 0, finishedMs: 0 }],
    state: "unknown", inferred: false, how: "SparkBox's saved load", startedMs: 0, finishedMs: null, ranS: null, exitCode: null, postmortem: null, demo: true,
  };
  const groups = deploymentsOf(w, [unknown]);
  expect(groups.some((d) => d.id === "unobserved")).toBe(false);
  expect(groups.some((d) => d.id === "llama-70b")).toBe(true);
});
