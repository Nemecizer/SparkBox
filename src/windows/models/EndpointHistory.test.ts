import { describe, expect, it } from "vitest";
import type { ClusterSnapshot } from "../../lib/types";
import { CANARY, defaultRange, historyQueries, increases, RANGE_S, roceIfaces, SERVING } from "./EndpointHistory";

const llama = { id: "cell1-vllm-8000", nodes: ["cell1", "cell2", "cell3"] };
const NOW = 1_800_000_000_000;

describe("Endpoints history", () => {
  it("turns a range into the queries it runs: the endpoint, its nodes' GPUs and RoCE retransmits", () => {
    const ifaces = [{ node: "cell2", iface: "enP2p1s0f1np1" }, { node: "cell2", iface: "enp1s0f1np1" }];
    const q = historyQueries(llama, ifaces, "24h", NOW);
    expect(q.map((x) => x.entity)).toEqual([
      "endpoint:cell1-vllm-8000", "endpoint:cell1-vllm-8000", "node:cell1", "node:cell2", "node:cell3", "net:cell2/enP2p1s0f1np1", "net:cell2/enp1s0f1np1",
    ]);
    expect(q[0].metrics).toEqual(SERVING);
    expect(q[0].metrics).toEqual(expect.arrayContaining(["genTps", "ttftP50", "ttftP95", "waiting", "kvPct", "preemptions"]));
    expect(q[1].metrics).toEqual([CANARY]);
    expect(q[2].metrics).toEqual(["gpu"]);
    expect(q[5].metrics).toEqual(["oos"]);
    for (const x of q) expect([x.fromMs, x.toMs, x.maxPoints]).toEqual([NOW - 86_400_000, NOW, 360]);
  });

  it("covers 1 h, 24 h, 7 d and 14 d", () => {
    for (const [range, s] of [["1h", 3600], ["24h", 86_400], ["7d", 604_800], ["14d", 1_209_600]] as const) {
      expect(RANGE_S[range]).toBe(s);
      const [first] = historyQueries(llama, [], range, NOW);
      expect(NOW - first.fromMs).toBe(s * 1000);
    }
  });

  it("finds its nodes' fabric interfaces that count RoCE", () => {
    const net = (iface: string, role: string, roce: boolean) => ({ iface, role, roce: roce ? { oos: 0 } : null });
    const snap = { nodes: [
      { id: "cell1", nets: [net("enP7s7", "lan", false), net("enp1s0f1np1", "fabric", true)] },
      { id: "gx10-0000", nets: [net("enp1s0f1np1", "fabric", true)] },
    ] } as unknown as ClusterSnapshot;
    expect(roceIfaces(snap, ["cell1", "cell2"])).toEqual([{ node: "cell1", iface: "enp1s0f1np1" }]);
    expect(roceIfaces(null, ["cell1"])).toEqual([]);
  });

  it("charts the preemption counter as its growth per point", () => {
    expect(increases([null, 4, 4, 7, null, 9, 2])).toEqual([null, null, 0, 3, null, 2, 2]);
  });

  it("opens on the shortest range that holds all of its history", () => {
    expect(defaultRange(NOW - 20 * 60_000, NOW)).toBe("1h");
    expect(defaultRange(NOW - 3 * 3600_000, NOW)).toBe("24h");
    expect(defaultRange(NOW - 2 * 86_400_000, NOW)).toBe("7d");
    expect(defaultRange(NOW - 10 * 86_400_000, NOW)).toBe("14d");
    expect(defaultRange(NOW - 40 * 86_400_000, NOW)).toBe("14d");
    expect(defaultRange(null, NOW)).toBe("14d");
  });
});
