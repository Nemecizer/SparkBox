// Demo is never Live (plan §3.1): every store that keeps rows between launches is keyed by data
// source. Each test switches the source the way the app does (the snapshot's demo flag, through
// dataSource.noteSource) and reads the store through its owner's public getter: only the current
// source's rows come back, and switching back brings the other side's again.
//
// Stores partitioned in the backend are tested there: tasks (tasks.rs
// each_task_keeps_its_source_and_the_list_shows_one_source), chat conversations (llm/history.rs
// each_source_lists_its_own_conversations), the activity log (live/audit.rs
// each_data_source_sees_only_its_own_rows) and benchmarks (models/bench.rs
// benchmarks_are_listed_per_data_source).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { recentCommandIds } from "../lib/commands";
import { currentSource, migrateLegacy, noteSource, onSourceChange, scopedKey, _resetDataSourceForTests } from "../lib/dataSource";
import { ackExit, isExitAcked, _resetExitsForTests } from "../lib/exits";
import { memTotalOf, recordSnapshot, resetStore } from "../lib/store";
import { closeAll, getTerminals, openTerminal, recentSessions } from "../lib/terminals";
import { canRetry, getTransfers, type Transfer } from "../lib/transfers";
import type { ClusterSnapshot, NodeSnapshot } from "../lib/types";
import { recentChoices, rememberChoice } from "../windows/chat/ModelPicker";
import { addFavorite, useFavorites } from "../windows/files/stores";
import { tasksFor } from "../windows/TasksWindow";
import { fleetHistory } from "../windows/FleetCommand";
import type { Task } from "../lib/tasks";
import { mockTauri } from "./tauri";

const GiB = 1024 ** 3;

beforeEach(() => {
  _resetDataSourceForTests("demo");
  mockTauri({ terminal_open: () => "t1", terminal_close: () => null, terminal_resize: () => null, terminal_ack: () => null });
});
afterEach(() => {
  closeAll();
  noteSource("demo");
});

describe("dataSource", () => {
  it("keys a store by source and tells listeners when it changes", () => {
    expect(currentSource()).toBe("demo");
    expect(scopedKey("sparkbox.x")).toBe("sparkbox.x:demo");
    const seen: string[] = [];
    const off = onSourceChange((next, prev) => seen.push(`${prev}→${next}`));
    noteSource("live");
    noteSource("live");
    expect(scopedKey("sparkbox.x")).toBe("sparkbox.x:live");
    noteSource("demo");
    off();
    noteSource("live");
    expect(seen).toEqual(["demo→live", "live→demo"]);
  });

  it("splits a store saved before sources once: by row where it can tell, else Demo", () => {
    localStorage.setItem("sparkbox.legacy", JSON.stringify([{ n: 1, source: "live" }, { n: 2 }, { n: 3, source: "demo" }]));
    localStorage.setItem(scopedKey("sparkbox.legacy", "demo"), JSON.stringify([{ n: 0 }]));
    migrateLegacy<{ n: number; source?: "demo" | "live" }>("sparkbox.legacy", (r) => r.source);
    expect(localStorage.getItem("sparkbox.legacy")).toBeNull();
    expect(JSON.parse(localStorage.getItem("sparkbox.legacy:live")!)).toEqual([{ n: 1, source: "live" }]);
    expect(JSON.parse(localStorage.getItem("sparkbox.legacy:demo")!).map((r: { n: number }) => r.n)).toEqual([0, 2, 3]);
    // An object store moves whole, to Demo.
    localStorage.setItem("sparkbox.legacy.obj", JSON.stringify({ cell1: 1 }));
    migrateLegacy("sparkbox.legacy.obj");
    expect(JSON.parse(localStorage.getItem("sparkbox.legacy.obj:demo")!)).toEqual({ cell1: 1 });
  });
});

describe("every store shows only the current source's rows", () => {
  it("Tasks (tasks.rs Task.demo, TasksWindow)", () => {
    const t = (id: string, demo?: boolean, client?: boolean) => ({ id, title: id, demo, client }) as unknown as Task;
    const all = [t("shut-down-cell3", true), t("uptime", false), t("from-round-3"), t("copy-in-this-window", undefined, true)];
    expect(tasksFor(all, "live").map((x) => x.id)).toEqual(["uptime", "copy-in-this-window"]);
    expect(tasksFor(all, "demo").map((x) => x.id)).toEqual(["shut-down-cell3", "from-round-3", "copy-in-this-window"]);
  });

  it("Chat model picker recents (ModelPicker)", () => {
    rememberChoice({ provider: "demo", model: "echo" });
    noteSource("live");
    expect(recentChoices()).toEqual([]);
    rememberChoice({ provider: "anthropic", model: "claude-opus-5-5" });
    expect(recentChoices().map((c) => c.model)).toEqual(["claude-opus-5-5"]);
    noteSource("demo");
    expect(recentChoices().map((c) => c.model)).toEqual(["echo"]);
  });

  it("Terminal recents and the last session's tabs (terminals.ts)", () => {
    openTerminal("cell1", { kind: "containerLogs", container: "vllm-head" }, { show: false });
    expect(recentSessions().map((r) => r.node)).toEqual(["cell1"]);
    closeAll();
    noteSource("live");
    expect(recentSessions()).toEqual([]);
    openTerminal("cell3", { kind: "journal", unit: "ollama.service" }, { show: false });
    expect(recentSessions().map((r) => r.node)).toEqual(["cell3"]);
    // Live's last session is offered in Live only.
    localStorage.setItem(scopedKey("sparkbox.terminal.session", "demo"), JSON.stringify([{ node: "cell2", target: { kind: "shell" } }]));
    closeAll();
    noteSource("demo");
    expect(getTerminals().restorable.map((d) => d.node)).toEqual(["cell2"]);
    expect(recentSessions().map((r) => r.node)).toEqual(["cell1"]);
  });

  it("File transfers, and Retry only for the source in use (transfers.ts)", () => {
    const row = (id: string): Transfer => ({ id, spec: { from: "cell1", to: "mac" }, state: "failed", what: id }) as unknown as Transfer;
    localStorage.setItem(scopedKey("sparkbox.transfers.v1", "live"), JSON.stringify([row("live-copy")]));
    noteSource("live");
    expect(getTransfers().map((t) => t.id)).toEqual(["live-copy"]);
    const demoRow = { ...row("demo-copy"), source: "demo" as const };
    expect(canRetry(demoRow)).toBe(false);
    expect(canRetry(getTransfers()[0])).toBe(true);
    noteSource("demo");
    expect(getTransfers().map((t) => t.id)).not.toContain("live-copy");
  });

  it("Files favourites (files/stores.ts)", () => {
    const { result } = renderHook(() => useFavorites("cell1"));
    act(() => addFavorite("cell1", "/home/sparkuser/glm53"));
    expect(result.current).toContain("/home/sparkuser/glm53");
    act(() => noteSource("live"));
    expect(result.current).not.toContain("/home/sparkuser/glm53");
    act(() => noteSource("demo"));
    expect(result.current).toContain("/home/sparkuser/glm53");
  });

  it("Hidden exits (exits.ts)", () => {
    _resetExitsForTests();
    ackExit("cell1", "7da4d848a41a", 1_793_053_280);
    expect(isExitAcked("cell1", "7da4d848a41a", 1_793_053_280)).toBe(true);
    noteSource("live");
    expect(isExitAcked("cell1", "7da4d848a41a", 1_793_053_280)).toBe(false);
    noteSource("demo");
    expect(isExitAcked("cell1", "7da4d848a41a", 1_793_053_280)).toBe(true);
  });

  it("Cached memory totals (store.ts)", () => {
    resetStore();
    const node = (total: number, status: NodeSnapshot["status"] = "online") => ({ id: "cell1", status, stale: false, mem: { total }, cpu: {}, gpu: {}, nets: [], disks: [] }) as unknown as NodeSnapshot;
    const snap = (demo: boolean, n: NodeSnapshot, tsMs: number) => ({ demo, tsMs, nodes: [n], switches: [] }) as unknown as ClusterSnapshot;
    recordSnapshot(snap(true, node(119.7 * GiB), 1_000));
    // Live, the node offline before it ever reported: Demo's 119.7 GiB isn't its size.
    recordSnapshot(snap(false, node(0, "offline"), 2_000));
    expect(memTotalOf(node(0, "offline"))).not.toBe(119.7 * GiB);
  });

  it("Palette recents (commands.ts)", () => {
    localStorage.setItem(scopedKey("sparkbox.palette.recent", "demo"), JSON.stringify(["node:cell3:shutdown", "model:restart:cell1-vllm-8000"]));
    localStorage.setItem(scopedKey("sparkbox.palette.recent", "live"), JSON.stringify(["win:topology"]));
    expect(recentCommandIds()).toEqual(["node:cell3:shutdown", "model:restart:cell1-vllm-8000"]);
    noteSource("live");
    expect(recentCommandIds()).toEqual(["win:topology"]);
    noteSource("demo");
    expect(recentCommandIds()).toContain("node:cell3:shutdown");
  });
  it("Fleet history (FleetCommand.tsx)", () => {
    const run = (command: string) => ({ command, sudo: false, nodes: ["cell1"], failed: [], total: 1, tsMs: 1 });
    localStorage.setItem(scopedKey("sparkbox.fleet.history", "demo"), JSON.stringify([run("nvidia-smi -q")]));
    localStorage.setItem(scopedKey("sparkbox.fleet.history", "live"), JSON.stringify([run("uptime")]));
    expect(fleetHistory().map((h) => h.command)).toEqual(["nvidia-smi -q"]);
    noteSource("live");
    expect(fleetHistory().map((h) => h.command)).toEqual(["uptime"]);
  });
});
