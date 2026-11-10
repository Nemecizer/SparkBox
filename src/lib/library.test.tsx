import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { libraryView, useLibraryReport } from "./library";
import { finishedNodes, modelsApi, type LibraryReport } from "./modelsApi";
import { hubPresence } from "../windows/models/HubTab";

const report = (): LibraryReport => ({ entries: [{ key: "/home/demo/models/org/tiny", repo: "org/tiny", revision: "abc", root: "/home/demo/models", sizeBytes: 100, format: "safetensors", inUse: null,
  copies: Object.fromEntries(["a", "b", "c"].map((id) => [id, { path: "/home/demo/models/org/tiny", sizeBytes: 100, verified: true }])) }],
  nodes: ["a", "b", "c"].map((node) => ({ node, state: "ready", checkedAtMs: 1000, error: null })) });
const nodes = ["a", "b", "c", "d"].map((id) => ({ id, online: true }));
afterEach(() => vi.restoreAllMocks());

describe("Library observations", () => {
  it("keeps successful copies, marks failed/offline copies stale and distinguishes unknown from empty", () => {
    const r = report(); r.nodes[1].state = "error"; r.nodes[1].error = "unreadable";
    const view = libraryView(r, nodes.map((n) => ({ ...n, online: n.id !== "c" })));
    expect(view.statuses.map((s) => s.state)).toEqual(["ready", "error", "offline", "unknown"]);
    expect(finishedNodes(view.entries![0])).toEqual(["a"]);
    expect(hubPresence(view.entries![0], "b")).toEqual({ kind: "unknown" });
    expect(view.entries![0].copies.c.stale).toBe(true);
    expect(r.entries[0].copies.b.stale).toBeUndefined();
  });
  it("global transport failure invalidates all copies, and removed nodes leave the view", () => {
    const view = libraryView(report(), nodes.filter((n) => n.id !== "c"), "IPC unavailable");
    expect(finishedNodes(view.entries![0])).toEqual([]);
    expect(view.entries![0].copies.c).toBeUndefined();
    expect(libraryView(report(), []).entries).toEqual([]);
  });
});

function pending<T>() { let resolve!: (x: T) => void; let reject!: (x: Error) => void;
  const promise = new Promise<T>((a,b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }

describe("Library refresh ordering", () => {
  it("retains observations on failure, then clears genuinely deleted copies after a successful empty scan", async () => {
    const api = vi.spyOn(modelsApi, "libraryReport").mockResolvedValueOnce(report()).mockRejectedValueOnce(new Error("scan transport failed"))
      .mockResolvedValueOnce({ entries: [], nodes: report().nodes });
    const { result } = renderHook(() => useLibraryReport("live/root"));
    await waitFor(() => expect(result.current.report?.entries).toHaveLength(1));
    await act(() => result.current.refresh());
    expect(result.current.error).toBe("scan transport failed"); expect(result.current.report?.entries).toHaveLength(1);
    await act(() => result.current.refresh());
    expect(result.current.report?.entries).toEqual([]); expect(result.current.error).toBeNull(); expect(api).toHaveBeenCalledTimes(3);
  });
  it("a late earlier refresh cannot overwrite a newer observation or its error state", async () => {
    const older = pending<LibraryReport>(), newer = pending<LibraryReport>();
    vi.spyOn(modelsApi, "libraryReport").mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    const { result } = renderHook(() => useLibraryReport("live/root"));
    let refresh!: Promise<void>; act(() => { refresh = result.current.refresh(); });
    await act(async () => { newer.resolve({ entries: [], nodes: report().nodes }); await refresh; });
    await act(async () => { older.resolve(report()); await older.promise; });
    expect(result.current.report?.entries).toEqual([]); expect(result.current.scanning).toBe(false);
  });
  it("reconnection does not revive a previously verified badge before a new scan succeeds", async () => {
    const offline = pending<LibraryReport>(), online = pending<LibraryReport>();
    vi.spyOn(modelsApi, "libraryReport").mockResolvedValueOnce(report()).mockReturnValueOnce(offline.promise).mockReturnValueOnce(online.promise);
    const { result, rerender } = renderHook(({ connection }) => useLibraryReport("live/root", connection), { initialProps: { connection: "online" } });
    await waitFor(() => expect(result.current.report?.nodes[0].state).toBe("ready"));
    rerender({ connection: "offline" }); rerender({ connection: "online" });
    expect(finishedNodes(libraryView(result.current.report, nodes).entries![0])).toEqual([]);
    await act(async () => { offline.resolve(report()); await offline.promise; });
    expect(result.current.report?.nodes[0].state).toBe("unknown");
    await act(async () => { online.resolve(report()); await online.promise; });
    expect(result.current.report?.nodes[0].state).toBe("ready");
  });
  it("changing source or model root hides old observations immediately and rejects a late reply", async () => {
    const old = pending<LibraryReport>(), next = pending<LibraryReport>();
    vi.spyOn(modelsApi, "libraryReport").mockResolvedValueOnce(report()).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
    const { result, rerender } = renderHook(({ context }) => useLibraryReport(context), { initialProps: { context: "live/root" } });
    await waitFor(() => expect(result.current.report?.entries).toHaveLength(1));
    act(() => { void result.current.refresh(); });
    rerender({ context: "demo/other-root" });
    expect(result.current.report).toBeNull();
    await act(async () => { old.resolve(report()); await old.promise; });
    expect(result.current.report).toBeNull();
    await act(async () => { next.resolve({ entries: [], nodes: [] }); await next.promise; });
    expect(result.current.report?.entries).toEqual([]);
  });
});
