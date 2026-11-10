import { describe, expect, it, beforeEach } from "vitest";
import { _resetDataSourceForTests, noteSource } from "./dataSource";
import { _resetExitsForTests, ackExit, isBadExit, recentExits, RECENT_EXIT_S } from "./exits";
import type { ClusterSnapshot, Container } from "./types";

const now = 1_793_201_130;
const c = (id: string, extra: Partial<Container>): Container => ({
  id, name: id, image: "img", state: "exited", startedAt: 0, finishedAt: now - 60, exitCode: 1, cpuPct: 0, mem: 0, gpuMem: 0, ports: [], project: null, command: "", ...extra,
});
const snap = (containers: Container[]): ClusterSnapshot =>
  ({ tsMs: now * 1000, demo: true, switches: [], nodes: [{ id: "gx10", name: "gx10-0000", containers }] } as unknown as ClusterSnapshot);

beforeEach(() => {
  _resetDataSourceForTests("demo");
  _resetExitsForTests();
});

describe("recent exits", () => {
  it("counts error exits and OOM kills from the last 24 h, newest first", () => {
    const list = recentExits(snap([
      c("crashed", { exitCode: 1, finishedAt: now - 3600 }),
      c("oom", { exitCode: 137, oomKilled: true, finishedAt: now - 60 }),
      c("clean", { exitCode: 0 }),
      c("old", { exitCode: 1, finishedAt: now - RECENT_EXIT_S - 1 }),
      c("running", { state: "running", exitCode: 0 }),
    ]));
    expect(list.map((e) => e.id)).toEqual(["oom", "crashed"]);
    expect(list[0]).toMatchObject({ node: "gx10", code: 137, oom: true });
    expect(isBadExit(c("x", { state: "dead", exitCode: 0 }))).toBe(true);
  });

  it("hides an acknowledged exit until the container exits again", () => {
    const first = snap([c("sglang-test", { exitCode: 137, finishedAt: now - 120 })]);
    ackExit("gx10", "sglang-test", now - 120);
    expect(recentExits(first)).toEqual([]);
    expect(localStorage.getItem("sparkbox.exits.acked.v1:demo")).toContain("gx10/sglang-test@");
    const again = snap([c("sglang-test", { exitCode: 137, finishedAt: now - 5 })]);
    expect(recentExits(again).map((e) => e.id)).toEqual(["sglang-test"]);
  });

  it("keeps Demo's hidden exits out of Live", () => {
    const glm = snap([c("glm53", { exitCode: 1, finishedAt: now - 120 })]);
    ackExit("gx10", "glm53", now - 120);
    expect(recentExits(glm)).toEqual([]);
    noteSource("live");
    expect(recentExits(glm).map((e) => e.id)).toEqual(["glm53"]);
    expect(localStorage.getItem("sparkbox.exits.acked.v1:live")).toBeNull();
    noteSource("demo");
    expect(recentExits(glm)).toEqual([]);
  });
});
