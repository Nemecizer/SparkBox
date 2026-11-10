import { beforeEach, describe, expect, it } from "vitest";
import {
  clusterFabric, clusterFabricCap, clusterTotals, dirUtil, fabricCapBps, fabricRx, fabricTx, GiB, gpuError, gpuServing, linkUtil, modelsInMemory,
  nodeNotes, nodeTone, powerLimit, powerSubLabel, problemView, rdmaRx, resetServing, tcpRx, type ProblemAlert, type ServingEndpoint,
} from "./metrics";
import { loadWidth, offlineReason, overlaps, rampStep, TOPO_TEXT_MIN, topologyLayout, topologyScale, type TopoLayout } from "../windows/Topology";
import { getHistory, memTotalOf, recent, recordSnapshot, resetStore } from "./store";
import type { ClusterSnapshot, NetStats, NodeSnapshot } from "./types";

function net(p: Partial<NetStats>): NetStats {
  return { iface: "x", role: "fabric", up: true, speedMbps: 200000, rxBps: 0, txBps: 0, ip: null, ...p };
}

function node(id: string, p: Partial<NodeSnapshot> = {}): NodeSnapshot {
  return {
    id, name: id, host: `${id}.local`, status: "online", latencyMs: 1, error: null,
    cpu: { totalPct: 5, perCore: [], load: [0, 0, 0], tempC: 40 },
    mem: { total: 128 * GiB, used: 20 * GiB, available: 108 * GiB, cached: 0, gpuAlloc: 0, swapTotal: 0, swapUsed: 0 },
    gpu: { utilPct: 0, tempC: 40, powerW: 10, smClockMhz: 0, procs: [] },
    disks: [], nets: [], system: { hostname: id, model: "", os: "", kernel: "", driver: "", cuda: "", uptimeS: 0 },
    containers: [], processes: [], listeners: [],
    ...p,
  };
}

const snap = (tsMs: number, nodes: NodeSnapshot[]): ClusterSnapshot => ({ tsMs, demo: true, nodes, switches: [] });

describe("fabric rates", () => {
  const n = node("cell1", {
    nets: [
      net({ iface: "enp1s0f1np1", rxBps: 2e6, txBps: 1e6, rdmaDev: "rocep1s0f1", rdmaRxBps: 20e9, rdmaTxBps: 18e9 }),
      net({ iface: "enP2p1s0f1np1", rxBps: 3e6, txBps: 4e6, rdmaDev: "roceP2p1s0f1", rdmaRxBps: 4e9, rdmaTxBps: 2e9 }),
      net({ iface: "enP7s7", role: "lan", speedMbps: 10000, rxBps: 9e9, txBps: 9e9 }),
    ],
  });

  it("adds IP and RDMA traffic and ignores the LAN", () => {
    expect(tcpRx(n)).toBe(5e6);
    expect(rdmaRx(n)).toBe(24e9);
    expect(fabricRx(n)).toBe(24e9 + 5e6);
    expect(fabricTx(n)).toBe(20e9 + 5e6);
  });

  it("counts an RDMA device shared by two netdevs once", () => {
    const shared = node("b", {
      nets: [
        net({ iface: "a0", rdmaDev: "mlx5_bond_0", rdmaRxBps: 10e9 }),
        net({ iface: "a1", rdmaDev: "mlx5_bond_0", rdmaRxBps: 10e9 }),
      ],
    });
    expect(fabricRx(shared)).toBe(10e9);
  });

  it("treats missing RDMA counters as no RDMA traffic", () => {
    expect(fabricRx(node("c", { nets: [net({ rxBps: 7e9 })] }))).toBe(7e9);
  });

  it("takes capacity from the negotiated speed of the cable, not the sum of its halves", () => {
    expect(fabricCapBps(n)).toBe(200e9);
    expect(fabricCapBps(node("d", { nets: [net({ up: false })] }))).toBe(0);
    expect(fabricCapBps(node("e", { nets: [net({ speedMbps: 100000 })] }))).toBe(100e9);
  });
});

describe("utilisation", () => {
  it("uses the busier direction", () => {
    expect(linkUtil(50e9, 20e9, 200e9)).toBe(0.25);
    expect(linkUtil(20e9, 180e9, 200e9)).toBe(0.9);
  });
  it("clamps and survives a zero capacity", () => {
    expect(linkUtil(400e9, 0, 200e9)).toBe(1);
    expect(linkUtil(1e9, 1e9, 0)).toBe(0);
    expect(dirUtil(24.1e9, 200e9)).toBeCloseTo(0.1205);
  });
});

describe("cluster fabric", () => {
  it("sums what online nodes send, so each byte counts once", () => {
    const s = snap(1000, [
      node("a", { nets: [net({ txBps: 1e9, rxBps: 2e9 })] }),
      node("b", { nets: [net({ txBps: 2e9, rxBps: 1e9 })] }),
      node("c", { status: "offline", nets: [net({ txBps: 9e9 })] }),
    ]);
    expect(clusterFabric(s)).toBe(3e9);
    expect(clusterFabricCap(s)).toBe(600e9);
  });
});

describe("gpuServing", () => {
  beforeEach(resetServing);

  it("counts a GPU holding model weights at once", () => {
    const s = snap(1000, [node("a", { mem: { ...node("a").mem, gpuAlloc: 80 * GiB } }), node("b")]);
    expect(gpuServing(s)).toEqual({ count: 1, total: 2, nodes: ["a"] });
  });

  it("uses a 30 s mean, not a single busy second", () => {
    const busy = (t: number, util: number) => snap(t, [node("a", { gpu: { ...node("a").gpu, utilPct: util } })]);
    expect(gpuServing(busy(0, 0)).count).toBe(0);
    for (let t = 1000; t < 10_000; t += 1000) gpuServing(busy(t, 0));
    // One 100% second in ten: the mean is 10%, not above it.
    expect(gpuServing(busy(10_000, 100)).count).toBe(0);
    expect(gpuServing(busy(11_000, 100)).count).toBe(1);
  });

  it("holds for 10 s after the work stops, then lets go", () => {
    const at = (t: number, alloc: number) => snap(t, [node("a", { mem: { ...node("a").mem, gpuAlloc: alloc } })]);
    gpuServing(at(0, 40 * GiB));
    expect(gpuServing(at(5_000, 0)).count).toBe(1);
    expect(gpuServing(at(9_000, 0)).count).toBe(1);
    expect(gpuServing(at(10_000, 0)).count).toBe(0);
  });

  it("counts each snapshot once however often it's asked", () => {
    const s = snap(1000, [node("a", { gpu: { ...node("a").gpu, utilPct: 50 } })]);
    for (let i = 0; i < 20; i++) gpuServing(s);
    expect(gpuServing(s).count).toBe(1);
  });

  it("drops a node that goes offline immediately", () => {
    const alloc = { ...node("a").mem, gpuAlloc: 40 * GiB };
    gpuServing(snap(0, [node("a", { mem: alloc })]));
    expect(gpuServing(snap(1000, [node("a", { mem: alloc, status: "offline" })])).count).toBe(0);
  });
});

describe("store history", () => {
  beforeEach(resetStore);

  it("keeps offline and stale seconds as gaps, not zeros", () => {
    recordSnapshot(snap(1_000, [node("a")]));
    recordSnapshot(snap(2_000, [node("a", { status: "offline" })]));
    recordSnapshot(snap(3_000, [node("a", { stale: true })]));
    recordSnapshot(snap(4_000, [node("a", { cpu: { ...node("a").cpu, totalPct: 50 } })]));
    const h = getHistory("a")!;
    expect(h.ts).toEqual([1, 2, 3, 4]);
    expect(h.cpu).toEqual([5, null, null, 50]);
  });

  it("remembers a node's memory size while it's offline, so its history still reads as a share", () => {
    const off = node("a", { status: "offline", mem: { ...node("a").mem, total: 0, used: 0 } });
    expect(memTotalOf(off)).toBe(0);
    recordSnapshot(snap(1_000, [node("a")]));
    recordSnapshot(snap(2_000, [off]));
    expect(memTotalOf(off)).toBe(128 * GiB);
    expect(memTotalOf(node("a", { mem: { ...node("a").mem, total: 64 * GiB } }))).toBe(64 * GiB);
  });

  it("keeps a GPU that can't be read as a gap, not an idle 0", () => {
    // GpuStats.readable decides, never the error text (§5).
    const hung = node("a", { gpu: { ...node("a").gpu, readable: false, error: "nvidia-smi didn't answer in 10 s" } });
    expect(gpuError(hung)).toBe("nvidia-smi didn't answer in 10 s");
    expect(gpuError(node("a", { gpu: { ...node("a").gpu, readable: false } }))).toBe("The GPU couldn't be read");
    expect(gpuError(node("a", { error: "GPU has fallen off the bus (Xid 79)" }))).toBeNull();
    expect(gpuError(node("a", { error: "docker: permission denied" }))).toBeNull();
    recordSnapshot(snap(1_000, [node("a")]));
    recordSnapshot(snap(2_000, [hung]));
    const h = getHistory("a")!;
    expect(h.gpu).toEqual([0, null]);
    expect(h.power).toEqual([10, null]);
    expect(h.cpu).toEqual([5, 5]);
  });

  it("breaks the line across missing seconds", () => {
    recordSnapshot(snap(1_000, [node("a")]));
    recordSnapshot(snap(9_000, [node("a")]));
    const h = getHistory("a")!;
    expect(h.ts).toEqual([1, 2, 9]);
    expect(h.gpu).toEqual([0, null, 0]);
  });

  it("ignores a second snapshot within the same second", () => {
    recordSnapshot(snap(1_000, [node("a")]));
    recordSnapshot(snap(1_200, [node("a")]));
    expect(getHistory("a")!.ts).toEqual([1]);
  });

  it("keeps one hour in a fixed ring and returns the newest window", () => {
    for (let t = 0; t < 4000; t++) recordSnapshot(snap(t * 1000, [node("a", { cpu: { ...node("a").cpu, totalPct: t % 100 } })]));
    const all = getHistory("a")!;
    expect(all.ts[all.ts.length - 1]).toBe(3999);
    expect(all.ts.length).toBe(3601);
    const last5 = getHistory("a", 300)!;
    expect(last5.ts[0]).toBe(3699);
    expect(recent("a", "cpu", 3)).toEqual([97, 98, 99]);
  });

  it("returns the same arrays until a new second arrives", () => {
    recordSnapshot(snap(1_000, [node("a")]));
    const a = getHistory("a");
    expect(getHistory("a")).toBe(a);
    recordSnapshot(snap(2_000, [node("a")]));
    expect(getHistory("a")).not.toBe(a);
  });
});

describe("topology layout", () => {
  const all = (l: TopoLayout) => [l.internet, l.gateway, l.lanSwitch, l.fabricSwitch, l.mac, ...l.nodes];

  for (const width of [1200, 760, 480]) {
    for (const mac of ["lan", "vpn"] as const) {
      it(`fits 4 nodes at ${width} px without overlaps (Mac on ${mac})`, () => {
        const l = topologyLayout(4, width, mac);
        expect(l.mode).toBe(width >= 900 ? "wide" : width >= 640 ? "medium" : "compact");
        const boxes = all(l).filter((r) => r.w > 0);
        for (const r of boxes) {
          expect(r.x).toBeGreaterThanOrEqual(0);
          expect(r.y).toBeGreaterThanOrEqual(0);
          expect(r.x + r.w).toBeLessThanOrEqual(l.width);
          expect(r.y + r.h).toBeLessThanOrEqual(l.height);
        }
        for (let i = 0; i < boxes.length; i++) {
          for (let j = i + 1; j < boxes.length; j++) expect(overlaps(boxes[i], boxes[j]), `${JSON.stringify(boxes[i])} vs ${JSON.stringify(boxes[j])}`).toBe(false);
        }
      });
    }
  }

  it("tightens spacing to fit a short window but never shrinks boxes", () => {
    const roomy = topologyLayout(4, 1200, "lan");
    const tight = topologyLayout(4, 1200, "lan", 470);
    expect(tight.height).toBeLessThan(roomy.height);
    expect(tight.nodes[0].h).toBe(roomy.nodes[0].h);
    expect(tight.fabricSwitch.h).toBe(roomy.fabricSwitch.h);
  });

  it("wraps nodes into rows when a medium window can't hold them in one", () => {
    const l = topologyLayout(6, 700, "lan");
    expect(new Set(l.nodes.map((r) => r.y)).size).toBeGreaterThan(1);
  });
});

describe("link load", () => {
  it("colours each direction on the 0/25/50/75/90% ramp", () => {
    expect([0, 0.1, 0.25, 0.49, 0.5, 0.8, 0.9, 1].map(rampStep)).toEqual([0, 0, 1, 1, 2, 3, 4, 4]);
  });

  it("measures each direction against the negotiated speed", () => {
    const n = node("a", { nets: [net({ txBps: 1e9, rdmaTxBps: 49e9, rxBps: 0, rdmaRxBps: 2e9, rdmaDev: "r0" })] });
    expect(dirUtil(fabricTx(n), fabricCapBps(n))).toBe(0.25);
    expect(dirUtil(fabricRx(n), fabricCapBps(n))).toBe(0.01);
    expect(rampStep(dirUtil(fabricTx(n), fabricCapBps(n)))).toBe(1);
  });

  it("draws busier links thicker, from 2 to 8 px", () => {
    expect(loadWidth(0)).toBe(2);
    expect(loadWidth(1)).toBe(8);
    expect(loadWidth(0.25)).toBeGreaterThan(loadWidth(0.05));
  });
});

describe("truthful totals (§2.4)", () => {
  const gb10 = (id: string, watts: number, p: Partial<NodeSnapshot> = {}) =>
    node(id, { gpu: { ...node(id).gpu, powerW: watts, powerLimitW: null }, mem: { ...node(id).mem, total: 119.7 * GiB, used: 30 * GiB }, ...p });

  it("sums power and memory over reporting nodes only and says how many those are", () => {
    // cell3 is offline but still carries its last reading (87 W, 60 GiB): it must add nothing.
    const s = snap(1000, [
      gb10("cell1", 50), gb10("cell2", 52),
      gb10("cell3", 87, { status: "offline", mem: { ...node("x").mem, total: 119.7 * GiB, used: 60 * GiB } }),
      gb10("gx10-0000", 49),
    ]);
    const t = clusterTotals(s);
    expect(t.reporting).toBe(3);
    expect(t.total).toBe(4);
    expect(t.reportingLabel).toBe("3 of 4 reporting");
    expect(t.powerW).toBe(151);
    expect(t.memUsed).toBe(90 * GiB);
    expect(t.memTotal).toBeCloseTo(3 * 119.7 * GiB);
    expect(powerSubLabel(t)).toBe("avg 50 W · 3 of 4 reporting");
  });

  it("treats a stale node as not reporting", () => {
    const t = clusterTotals(snap(1000, [gb10("a", 40), gb10("b", 40, { stale: true })]));
    expect([t.reporting, t.powerW, t.reportingLabel]).toEqual([1, 40, "1 of 2 reporting"]);
  });

  it("has no limit, and no limit text, when a node doesn't report one", () => {
    const all = snap(1000, [gb10("cell1", 50), gb10("cell2", 51), gb10("cell3", 50), gb10("gx10-0000", 50)]);
    expect(powerLimit(all.nodes[0])).toBeNull();
    const t = clusterTotals(all);
    expect(t.powerLimitW).toBeNull();
    expect(powerSubLabel(t)).toBe("4 nodes · avg 50 W");
    expect(powerSubLabel(t)).not.toMatch(/limit/);
    // The limit only shows when every reporting node has one; one N/A hides it.
    const capped = (id: string, w: number | null) => node(id, { gpu: { ...node(id).gpu, powerW: 50, powerLimitW: w } });
    expect(clusterTotals(snap(1, [capped("a", 100), capped("b", 100)])).powerLimitW).toBe(200);
    expect(powerSubLabel(clusterTotals(snap(1, [capped("a", 100), capped("b", 100)])))).toBe("2 nodes · avg 50 W · of 200 W limit");
    expect(clusterTotals(snap(1, [capped("a", 100), capped("b", null)])).powerLimitW).toBeNull();
  });
});

describe("gpuServing with endpoints", () => {
  const nodes = ["cell1", "cell2", "cell3", "gx10-0000"];
  const at = 1_000_000;
  const cluster = (offline: string[] = []) => snap(at, nodes.map((id) => node(id, { status: offline.includes(id) ? "offline" : "online" })));
  const pp3: ServingEndpoint = { health: "ready", nodes: ["cell1", "cell2", "cell3"], expectedNodes: ["cell1", "cell2", "cell3"], lastOkMs: at - 2000 };
  const ollama: ServingEndpoint = { health: "ready", nodes: ["gx10-0000"], expectedNodes: ["gx10-0000"], lastOkMs: at - 5000 };

  it("counts every member GPU of a Ready endpoint that answered recently", () => {
    expect(gpuServing(cluster(), [pp3, ollama])).toEqual({ count: 4, total: 4, nodes });
  });

  it("counts only gx10's Ollama when the pipeline is missing a stage", () => {
    // The backend marks the PP3 deployment Down once cell3 goes; its members serve nothing.
    const down: ServingEndpoint = { ...pp3, health: "down", nodes: ["cell1", "cell2"], lastOkMs: at - 40_000 };
    expect(gpuServing(cluster(["cell3"]), [down, ollama])).toEqual({ count: 1, total: 4, nodes: ["gx10-0000"] });
  });

  it("doesn't trust a Ready endpoint that hasn't answered for a minute, or a member that's offline", () => {
    expect(gpuServing(cluster(), [{ ...pp3, lastOkMs: at - 61_000 }, ollama]).nodes).toEqual(["gx10-0000"]);
    expect(gpuServing(cluster(), [{ ...pp3, lastOkMs: null }]).count).toBe(0);
    expect(gpuServing(cluster(["cell2"]), [pp3]).nodes).toEqual(["cell1", "cell3"]);
    expect(gpuServing(cluster(), [{ ...ollama, health: "loading" }]).count).toBe(0);
  });
});

describe("topology level of detail (§D8)", () => {
  it("never draws diagram text below 11 px: anything that wouldn't fit becomes the list", () => {
    for (const textScale of [1, 1.12, 1.25]) {
      for (let w = 280; w <= 1800; w += 37) {
        for (let h = 200; h <= 1100; h += 41) {
          for (const n of [1, 4, 6]) {
            const t = topologyScale(w, h, n, "lan", textScale);
            if (t.list) continue;
            expect(t.textPx).toBeGreaterThanOrEqual(11);
            expect(t.scale * TOPO_TEXT_MIN).toBeGreaterThanOrEqual(11);
            // The diagram fits the window as drawn: nothing scaled down, nothing cut.
            expect(t.layout.width * t.scale).toBeLessThanOrEqual(w + 1);
            expect(t.layout.height * t.scale).toBeLessThanOrEqual(h + 1);
          }
        }
      }
    }
  });

  it("shows the list in a tiled or small window and the diagram in a roomy one", () => {
    expect(topologyScale(1180, 760, 4, "lan").list).toBe(false);
    expect(topologyScale(620, 700, 4, "lan").list).toBe(true);
    expect(topologyScale(900, 400, 4, "lan").list).toBe(true);
    // A quarter of a 1440×900 desktop.
    expect(topologyScale(600, 400, 4, "lan").list).toBe(true);
    // Larger text needs more room before the diagram fits.
    expect(topologyScale(1000, 560, 4, "lan", 1).list).toBe(false);
    expect(topologyScale(1000, 560, 4, "lan", 1.25).list).toBe(true);
  });

  it("says why a node is offline without cutting an address in half", () => {
    const n = node("cell3", { status: "offline", error: "ssh: connect to host 192.168.50.53 port 22: Operation timed out" });
    expect(offlineReason(n)).toBe("No SSH reply");
    expect(offlineReason({ ...n, error: "Permission denied (publickey,password)." })).toBe("Login failed");
    expect(offlineReason({ ...n, error: "weird failure talking to 192.168.50.53:22 now" })).toBe("weird failure talking to now");
    expect(offlineReason({ ...n, powerState: "off" })).toBe("Shut down from SparkBox");
  });
});

// ---- Node colour describes the node (M11, §2.3)

function alert(p: Partial<ProblemAlert> & { id: string; kind: string; node: string | null; severity: "warn" | "crit" }): ProblemAlert {
  const { kind, node, ...rest } = p;
  return { summary: p.id, state: "firing", entity: { kind, id: kind === "node" ? node! : p.id, node }, sinceMs: 1000, ...rest };
}

describe("nodeTone and nodeNotes", () => {
  // Scenario node-offline: cell3 stops answering; llama-3.3-70b (cell1–cell3, PP 3) goes Down.
  const cluster = snap(10_000, [node("cell1"), node("cell2"), node("cell3", { status: "offline", error: "SSH timeout" }), node("gx10-0000")]);
  const view = {
    alerts: [
      alert({ id: "node-offline:cell3", rule: "node-offline", kind: "node", node: "cell3", severity: "crit" }),
      alert({ id: "endpoint-down:cell1-vllm-8000", rule: "endpoint-down", kind: "endpoint", node: "cell1", severity: "crit" }),
      alert({ id: "crash-loop:vllm-head", rule: "crash-loop", kind: "container", node: "cell1", severity: "warn" }),
    ],
  };
  const endpoints = [
    { id: "cell1-vllm-8000", models: ["llama-3.3-70b"], nodes: ["cell1"], expectedNodes: ["cell1", "cell2", "cell3"], health: "down", healthReason: "Pipeline stage 3 of 3 on cell3 is offline" },
    { id: "gx10-0000-ollama-11434", models: ["gpt-oss:20b"], nodes: ["gx10-0000"], health: "ready" },
  ];

  it("paints only the node that's offline, never the head of a model that's down", () => {
    expect(nodeTone("cell1", view, cluster)).toBe("ok");
    expect(nodeTone("cell2", view, cluster)).toBe("ok");
    expect(nodeTone("cell3", view, cluster)).toBe("offline");
    expect(nodeTone("gx10-0000", view, cluster)).toBe("ok");
  });

  it("gives every pipeline member the same neutral note, linking to the model", () => {
    for (const id of ["cell1", "cell2", "cell3"]) {
      const notes = nodeNotes(id, view, endpoints);
      expect(notes).toHaveLength(1);
      expect(notes[0]).toMatchObject({ text: "Serving llama-3.3-70b · down", state: "down", tone: "crit", link: { kind: "endpoints", params: { id: "cell1-vllm-8000" } } });
    }
    expect(nodeNotes("gx10-0000", view, endpoints)).toEqual([]);
  });

  it("counts the node's own alerts: disk, GPU, interfaces, units, kernel, agent", () => {
    const own = { alerts: [alert({ id: "disk-full:/", rule: "disk-full", kind: "disk", node: "cell2", severity: "warn" })] };
    expect(nodeTone("cell2", own, cluster)).toBe("warn");
    const gpu = { alerts: [...own.alerts, alert({ id: "xid:gx10", rule: "gpu-xid", kind: "gpu", node: "cell2", severity: "crit" })] };
    expect(nodeTone("cell2", gpu, cluster)).toBe("crit");
  });

  it("doesn't count silenced, acknowledged, recovering, planned or pending alerts", () => {
    const quiet = (p: Partial<ProblemAlert>) => ({ alerts: [{ ...alert({ id: "disk", kind: "disk", node: "cell1", severity: "crit" }), ...p }] });
    expect(nodeTone("cell1", quiet({ silenceId: "s1" }), cluster)).toBe("ok");
    expect(nodeTone("cell1", quiet({ ackedMs: 5 }), cluster)).toBe("ok");
    expect(nodeTone("cell1", quiet({ recoversAtMs: Date.now() + 60_000 }), cluster)).toBe("ok");
    expect(nodeTone("cell1", quiet({ planned: "restarting" }), cluster)).toBe("ok");
    expect(nodeTone("cell1", quiet({ state: "pending" }), cluster)).toBe("ok");
    // A restart SparkBox started reads as planned, not offline.
    const restarting = snap(10_000, [node("cell1", { status: "offline", maintenance: { reason: "restarting", startedMs: 1, untilMs: 2 } })]);
    expect(nodeTone("cell1", { alerts: [] }, restarting)).toBe("ok");
  });
});

describe("problemView", () => {
  it("counts incidents, not alerts, and keeps recovering, pending and known issues apart", () => {
    const now = 50_000;
    const v = problemView([
      alert({ id: "roce:cell2", kind: "interface", node: "cell2", severity: "warn", incidentId: "link:cell2/p1" }),
      alert({ id: "port-errors:crs/1", kind: "port", node: null, severity: "crit", incidentId: "link:cell2/p1", sinceMs: 500 }),
      alert({ id: "disk:gx10", kind: "disk", node: "gx10-0000", severity: "warn" }),
      alert({ id: "offline:cell3", kind: "node", node: "cell3", severity: "crit", state: "pending" }),
      alert({ id: "crash:llama", kind: "container", node: "cell1", severity: "warn", recoversAtMs: now + 90_000 }),
      alert({ id: "silenced", kind: "disk", node: "cell1", severity: "warn", silenceId: "s" }),
      alert({ id: "acked", kind: "service", node: "cell1", severity: "crit", ackedMs: 1 }),
      alert({ id: "planned", kind: "node", node: "cell4", severity: "crit", planned: "restarting" }),
    ], now);
    expect(v.crit.map((i) => i.key)).toEqual(["link:cell2/p1"]);
    expect(v.crit[0].members).toHaveLength(2);
    expect(v.crit[0].lead.id).toBe("port-errors:crs/1");
    expect(v.warn.map((i) => i.key)).toEqual(["disk:gx10"]);
    expect(v.pending.map((a) => a.id)).toEqual(["offline:cell3"]);
    expect(v.recovering.map((a) => a.id)).toEqual(["crash:llama"]);
    expect(v.planned.map((a) => a.id)).toEqual(["planned"]);
    expect(v.known).toBe(2);
  });
});

describe("modelsInMemory", () => {
  it("counts loaded models: a Ready server's models, and only what Ollama has in memory", () => {
    expect(modelsInMemory([
      { health: "ready", engine: "vllm", models: ["llama-3.3-70b"] },
      { health: "ready", engine: "ollama", models: ["gpt-oss:20b", "qwen3:32b", "llama3:8b", "phi4", "mistral"], ollama: [{ name: "gpt-oss:20b" }] },
      { health: "down", engine: "vllm", models: ["qwen3.8"] },
    ])).toBe(2);
  });
});
