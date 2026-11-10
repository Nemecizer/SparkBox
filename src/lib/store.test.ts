import { afterEach, describe, expect, it } from "vitest";
import { baseSnapshot } from "../dev/mockBackend";
import { emitMock, mockCalls, mockTauri } from "../test/tauri";
import type { HistoryResult } from "./historyApi";
import { chartWindow, getHistory, resetStore, startClusterStore } from "./store";
import type { ClusterSnapshot } from "./types";

// Charts open full (§2.4, C5): the stored hour is in the rings before the first paint, and a
// gap the history store fills later (an agent's recording arrived) is merged in on
// `history://filled`, without duplicates.

const NOW = 1_800_000_000_000;
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

function snap(tsMs = NOW): ClusterSnapshot {
  const s = structuredClone(baseSnapshot);
  s.tsMs = tsMs;
  return s;
}

/** A history store that has `has(tMs)` for each 10 s bucket of the hour (and 1 s for the last 15 minutes). */
function store(has: (tMs: number) => boolean) {
  return ({ entity, metrics, fromMs, toMs }: { entity: string; metrics: string[]; fromMs: number; toMs: number }): HistoryResult => {
    const step = toMs - fromMs <= 900_000 ? 1000 : 10_000;
    const ts: number[] = [];
    for (let t = Math.floor(fromMs / step) * step; t <= toMs; t += step) ts.push(t);
    const node = entity.slice(5);
    const series: Record<string, (number | null)[]> = {};
    for (const m of metrics) series[m] = ts.map((t) => (has(t) ? (m === "gpu" ? 40 + (node.length % 5) : 1) : null));
    return { ts, series, tier: step === 1000 ? "1s" : "10s", stepMs: step, startsAtMs: ts.find(has) ?? null };
  };
}

afterEach(() => resetStore());

describe("charts open full", () => {
  it("has the stored hour in the rings before the first snapshot shows", async () => {
    resetStore();
    mockTauri({ get_snapshot: () => snap(), history_query: store(() => true) });
    await startClusterStore();
    const id = baseSnapshot.nodes[0].id;
    const h = getHistory(id, 3600)!;
    expect(h.ts[0]).toBeLessThanOrEqual(NOW / 1000 - 3590);
    // The last 15 minutes came at 1 s and won over the hour's 10 s buckets.
    const recent = h.ts.filter((t) => t >= NOW / 1000 - 890);
    expect(recent.length).toBeGreaterThan(850);
    expect(h.gpu.filter((v) => v == null).length).toBe(0);
    // Two queries per node: the last 15 minutes and the hour.
    expect(mockCalls.filter((c) => c.cmd === "history_query" && c.args.entity === `node:${id}`)).toHaveLength(2);
  });

  it("merges a gap filled later on history://filled, once, with no duplicate times", async () => {
    resetStore();
    // SparkBox was closed between −50 and −20 minutes; the agent's copy arrives later.
    const closed = (t: number) => t >= NOW - 50 * 60_000 && t < NOW - 20 * 60_000;
    let agentIn = false;
    mockTauri({ get_snapshot: () => snap(), history_query: (a: { entity: string; metrics: string[]; fromMs: number; toMs: number }) => store((t) => agentIn || !closed(t))(a) });
    await startClusterStore();
    const id = baseSnapshot.nodes[0].id;
    const before = getHistory(id, 3600)!;
    const inGap = (h: typeof before) => h.ts.filter((t, i) => t * 1000 >= NOW - 49 * 60_000 && t * 1000 < NOW - 21 * 60_000 && h.gpu[i] != null).length;
    expect(inGap(before)).toBe(0);
    // A null marks where recording stopped, so the line breaks instead of joining across.
    expect(before.ts.some((t, i) => before.gpu[i] == null && t * 1000 >= NOW - 51 * 60_000 && t * 1000 < NOW - 49 * 60_000)).toBe(true);

    agentIn = true;
    await emitMock("history://filled", { entities: [`node:${id}`, "net:cell1/enp1s0f1np1"] });
    await wait(600);
    const after = getHistory(id, 3600)!;
    expect(inGap(after)).toBeGreaterThanOrEqual(160);
    // Merged without duplicates: strictly increasing times.
    for (let i = 1; i < after.ts.length; i++) expect(after.ts[i]).toBeGreaterThan(after.ts[i - 1]);
    // The filled gap no longer breaks the line.
    const nulls = after.ts.filter((t, i) => after.gpu[i] == null && t * 1000 > NOW - 55 * 60_000 && t * 1000 < NOW - 15 * 60_000);
    expect(nulls).toEqual([]);
    // A second announcement of the same data changes nothing.
    const version = after;
    await emitMock("history://filled", { entities: [`node:${id}`] });
    await wait(600);
    expect(getHistory(id, 3600)!.ts).toEqual(version.ts);
  });

  it("collapses the window to the data when history is truly missing, and widens it as data comes", () => {
    const now = NOW / 1000;
    // Recording began 30 s ago and no agent has more: two minutes shown, "History starts" inside them.
    const young = chartWindow([now - 30, now - 29, now], now, 900);
    expect(young.range).toEqual([now - 120, now]);
    expect(young.startsAt).toBe(now - 30);
    // Five minutes in: the window is those five minutes, with no empty part and no chip.
    const five = chartWindow([now - 300, now], now, 900);
    expect(five.range).toEqual([now - 300, now]);
    expect(five.startsAt).toBeNull();
    // A full hour of history: the normal window.
    expect(chartWindow([now - 3600, now], now, 900)).toEqual({ range: [now - 900, now], startsAt: null });
  });
});

describe("Demo and Live never share a chart (C2)", () => {
  it("starts the hour over on a switch, draws Live's first frame at once and merges Live's own history", async () => {
    resetStore();
    const liveHistory = store((t) => t > NOW - 600_000 && t < NOW - 60_000);
    const demoSnap = (t: number) => ({ ...snap(t), demo: true });
    mockTauri({ get_snapshot: () => demoSnap(NOW), history_query: (a: Parameters<typeof liveHistory>[0]) => (live ? liveHistory(a) : store(() => false)(a)) });
    let live = false;
    await startClusterStore();
    const id = baseSnapshot.nodes[0].id;
    for (let i = 1; i <= 5; i++) await emitMock("cluster://snapshot", demoSnap(NOW + i * 1000));
    await wait(20);
    expect(getHistory(id)!.ts.length).toBeGreaterThanOrEqual(5);
    // The backend switches to Live: the next snapshot is Live's.
    live = true;
    const first = { ...snap(NOW + 6000), demo: false };
    await emitMock("cluster://snapshot", first);
    await wait(20);
    // Not one Demo second is left; Live's first frame is there at once.
    const h = getHistory(id)!;
    expect(h.ts.some((t) => t >= NOW / 1000 && t <= NOW / 1000 + 5)).toBe(false);
    expect(h.ts[h.ts.length - 1]).toBe(first.tsMs / 1000);
    expect(h.gpu[h.gpu.length - 1]).not.toBeNull();
    // Live's stored minutes arrive and are merged in, older than the switch.
    await wait(60);
    expect(getHistory(id)!.ts[0]).toBeLessThan(NOW / 1000 - 60);
  });

  it("remembers each node's memory size per data source", async () => {
    resetStore();
    const { memTotalOf, recordSnapshot } = await import("./store");
    const demo = { ...snap(NOW), demo: true };
    const live = { ...snap(NOW + 1000), demo: false };
    live.nodes = live.nodes.map((n) => ({ ...n, mem: { ...n.mem, total: n.mem.total + 2 * 1024 ** 3 } }));
    recordSnapshot(demo);
    recordSnapshot(live);
    const n = live.nodes[0];
    const off = { ...n, status: "offline" as const, mem: { ...n.mem, total: 0 } };
    // Live is on screen: the offline node's share is of its real size, not the simulator's.
    expect(memTotalOf(off)).toBe(n.mem.total);
    expect(memTotalOf(off)).not.toBe(demo.nodes[0].mem.total);
  });
});
