import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emitMock, mockTauri } from "../test/tauri";
import type { Task } from "./opsApi";
import { _resetTasksForTests, getTasks, runTask, taskOrigin, withOrigin } from "./tasks";
import { aliasOp, DEDUPE_MS, dismissToast, getToasts, MAX_VISIBLE, noteOpOrigin, pauseToast, resumeToast, setFrontWindowProbe, toast, updateToast, visibleToasts } from "./toasts";
import { canUndo, focusOwnsUndo, pushUndo, runLatestUndo } from "./undo";
import { runServiceAction } from "./workloadCommands";

// Service actions ask nothing for a low-impact restart, and Admin mode is open.
vi.mock("./admin", async (importOriginal) => ({ ...(await importOriginal<typeof import("./admin")>()), requestAdmin: vi.fn(async () => true) }));

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  for (const t of getToasts()) dismissToast(t.id);
  setFrontWindowProbe(() => false);
  vi.useRealTimers();
});

describe("toasts", () => {
  it("lets success go after 4 s but keeps errors until dismissed", () => {
    const ok = toast({ kind: "success", title: "Saved hosts" });
    const bad = toast({ kind: "error", title: "Couldn't save", body: "Permission denied" });
    vi.advanceTimersByTime(3900);
    expect(getToasts().map((t) => t.id)).toEqual([ok, bad]);
    vi.advanceTimersByTime(200);
    expect(getToasts().map((t) => t.id)).toEqual([bad]);
    vi.advanceTimersByTime(60_000);
    expect(getToasts().map((t) => t.id)).toEqual([bad]);
    dismissToast(bad);
    expect(getToasts()).toEqual([]);
  });

  it("stops the clock while the pointer is over it", () => {
    const id = toast({ kind: "success", title: "Copied" });
    vi.advanceTimersByTime(3000);
    pauseToast(id);
    vi.advanceTimersByTime(10_000);
    expect(getToasts()).toHaveLength(1);
    resumeToast(id);
    vi.advanceTimersByTime(1100);
    expect(getToasts()).toHaveLength(1);
    vi.advanceTimersByTime(200);
    expect(getToasts()).toHaveLength(0);
  });

  it("turns a progress toast into a success that then goes away", () => {
    const id = toast({ kind: "progress", title: "Copying model", progress: 0.2 });
    vi.advanceTimersByTime(30_000);
    updateToast(id, { progress: 0.7 });
    expect(getToasts()[0].progress).toBe(0.7);
    updateToast(id, { kind: "success", title: "Copied model" });
    vi.advanceTimersByTime(4100);
    expect(getToasts()).toHaveLength(0);
  });

  it("replaces a toast that reuses an id", () => {
    toast({ kind: "info", title: "Zoom 110%", id: "zoom" });
    toast({ kind: "info", title: "Zoom 125%", id: "zoom" });
    expect(getToasts().map((t) => t.title)).toEqual(["Zoom 125%"]);
  });

  it("lets info go after 4 s too", () => {
    toast({ kind: "info", title: "Zoom 110%" });
    vi.advanceTimersByTime(3900);
    expect(getToasts()).toHaveLength(1);
    vi.advanceTimersByTime(200);
    expect(getToasts()).toHaveLength(0);
  });

  it("shows the newest three and counts the rest as “N more”", () => {
    const err = toast({ kind: "error", title: "e" });
    for (let i = 0; i < 4; i++) toast({ kind: "info", title: `i${i}` });
    const { shown, more } = visibleToasts();
    expect(MAX_VISIBLE).toBe(3);
    expect(shown.map((t) => t.title)).toEqual(["i1", "i2", "i3"]);
    expect(more).toBe(2);
    expect(visibleToasts(getToasts(), true).shown).toHaveLength(5);
    // Errors are kept until dismissed, even past the cap.
    for (let i = 0; i < 20; i++) toast({ kind: "info", title: `x${i}` });
    expect(getToasts().some((t) => t.id === err)).toBe(true);
  });
});

describe("toasts that stay out of the way", () => {
  it("leaves out a success from the window in front, but not from one behind", () => {
    setFrontWindowProbe((id) => id === "fleet");
    toast({ kind: "success", title: "Ran on 4 nodes", origin: "fleet" });
    expect(getToasts()).toHaveLength(0);
    toast({ kind: "success", title: "Restarted vllm-head", origin: "containers" });
    expect(getToasts().map((t) => t.title)).toEqual(["Restarted vllm-head"]);
    // Errors always show.
    toast({ kind: "error", title: "Couldn't run", origin: "fleet" });
    expect(getToasts()).toHaveLength(2);
  });

  it("closes a progress toast instead of finishing it as a success the window already shows", () => {
    setFrontWindowProbe((id) => id === "models");
    const id = toast({ kind: "progress", title: "Copying", origin: "models" });
    expect(getToasts()).toHaveLength(1);
    updateToast(id, { kind: "success", title: "Copied" });
    expect(getToasts()).toHaveLength(0);
  });

  it("merges toasts for the same operation into one, and an error stays an error", () => {
    toast({ kind: "progress", title: "Restart open-webui", opId: "task-7" });
    toast({ kind: "error", title: "Restart open-webui: failed", body: "exit 1", opId: "task-7" });
    expect(getToasts().map((t) => [t.kind, t.title])).toEqual([["error", "Restart open-webui: failed"]]);
    toast({ kind: "success", title: "Restart open-webui", opId: "task-7" });
    expect(getToasts().map((t) => t.kind)).toEqual(["error"]);
  });

  it("finds a task's window from the op it started", () => {
    setFrontWindowProbe((id) => id === "services");
    noteOpOrigin("task-9", "services");
    toast({ kind: "success", title: "Restarted on 3 nodes", opId: "task-9" });
    expect(getToasts()).toHaveLength(0);
  });
});

describe("undo", () => {
  it("offers Undo for 10 s and runs the latest with ⌘Z", async () => {
    const undoA = vi.fn();
    const undoB = vi.fn();
    pushUndo({ label: "Closed “Files”", undo: undoA });
    pushUndo({ label: "Moved hosts to the Trash", undo: undoB });
    expect(getToasts().filter((t) => t.undo)).toHaveLength(2);
    expect(await runLatestUndo()).toBe(true);
    expect(undoB).toHaveBeenCalledOnce();
    expect(undoA).not.toHaveBeenCalled();
    expect(getToasts().map((t) => t.title)).toEqual(["Closed “Files”"]);
    vi.advanceTimersByTime(10_001);
    expect(canUndo()).toBe(false);
    expect(await runLatestUndo()).toBe(false);
    expect(undoA).not.toHaveBeenCalled();
  });

  it("folds closes into one offer whose Undo brings them all back", async () => {
    const back: string[] = [];
    const close = (name: string) =>
      pushUndo({ label: `Closed “${name}”`, collapse: { key: "close", count: 1, label: (n) => `${n} windows closed` }, undo: () => void back.push(name) });
    close("Files");
    close("Terminal");
    close("Services");
    expect(getToasts().map((t) => t.title)).toEqual(["3 windows closed"]);
    expect(await runLatestUndo()).toBe(true);
    expect(back).toEqual(["Services", "Terminal", "Files"]);
    expect(canUndo()).toBe(false);
  });

  it("leaves ⌘Z to text fields and terminals", () => {
    const input = document.createElement("input");
    const term = document.createElement("div");
    term.className = "xterm";
    const inner = document.createElement("textarea");
    term.appendChild(inner);
    const button = document.createElement("button");
    expect(focusOwnsUndo(input)).toBe(true);
    expect(focusOwnsUndo(inner)).toBe(true);
    expect(focusOwnsUndo(button)).toBe(false);
    expect(focusOwnsUndo(document.body)).toBe(false);
  });
});

describe("one toast per outcome", () => {
  const task = (over: Partial<Task>): Task => ({
    id: "t1", kind: "service", title: "Restart nvidia-dgx-telemetry.service on cell1", nodes: ["cell1"], source: "ui", cancellable: false,
    state: "running", startedMs: Date.now(), finishedMs: null, progress: null, detail: null, rateBps: null, etaS: null, steps: [], log: [],
    summary: null, error: null, cancelRequested: false, retry: null, opId: "t1", ...over,
  });
  const failure = "systemctl restart nvidia-dgx-telemetry.service failed on cell1: Job failed. See journalctl -xe.";

  beforeEach(() => _resetTasksForTests());
  /** Starts the task store on the current mock backend and lets its listener attach. */
  const listening = async () => {
    getTasks();
    await vi.advanceTimersByTimeAsync(0);
  };

  it("replaces the toast with the same key, whatever its kind", () => {
    toast({ kind: "error", key: "endpoint:llama", title: "llama-3.3-70b stopped answering" });
    toast({ kind: "info", title: "Something else" });
    toast({ kind: "success", key: "endpoint:llama", title: "llama-3.3-70b is answering again" });
    expect(getToasts().map((t) => [t.kind, t.title])).toEqual([["success", "llama-3.3-70b is answering again"], ["info", "Something else"]]);
  });

  it("shows the same words twice within 2 s once (a double-click on Tile)", () => {
    toast({ kind: "info", title: "Tiled 4 windows" });
    vi.advanceTimersByTime(500);
    toast({ kind: "info", title: "Tiled 4 windows" });
    expect(getToasts()).toHaveLength(1);
    vi.advanceTimersByTime(DEDUPE_MS + 100);
    toast({ kind: "info", title: "Tiled 4 windows" });
    expect(getToasts()).toHaveLength(2);
    // Different words are a different toast.
    toast({ kind: "info", title: "Tiled 4 windows", body: "1 minimized" });
    expect(getToasts()).toHaveLength(3);
  });

  it("merges an op reported under an alias, and keeps the first words with both buttons", () => {
    toast({ kind: "error", opId: "g1", title: "Couldn't restart foo", body: "exit 1", actions: [{ label: "Unlock", run: () => {} }] });
    aliasOp("t9", "g1");
    toast({ kind: "error", opId: "t9", title: "Restart foo on cell1: failed", body: "exit 1", actions: [{ label: "Details", run: () => {} }] });
    const all = getToasts();
    expect(all).toHaveLength(1);
    expect(all[0].title).toBe("Couldn't restart foo");
    expect(all[0].actions?.map((a) => a.label)).toEqual(["Unlock", "Details"]);
  });

  it("gives a failing service restart one toast, though the action and its task both report it", async () => {
    mockTauri({
      service_impact: () => ({ tier: "low", title: "Restart nvidia-dgx-telemetry.service on cell1?", lines: [], confirmLabel: "Restart", requireTyping: null }),
      service_action: () => {
        // The backend ran it as a task: its update arrives on the next 200 ms flush, after the error.
        setTimeout(() => void emitMock("tasks://update", task({ state: "failed", finishedMs: Date.now(), error: failure })), 200);
        throw { kind: "failed", message: failure, node: "cell1" };
      },
    });
    await listening();
    await runServiceAction(["cell1"], "nvidia-dgx-telemetry.service", "restart", { origin: "services" });
    expect(getToasts().filter((t) => t.kind === "error")).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(400);
    const errors = getToasts().filter((t) => t.kind === "error");
    expect(errors).toHaveLength(1);
    expect(errors[0].body).toBe(failure);
    expect(errors[0].actions?.map((a) => a.label)).toContain("Details");
    expect(taskOrigin("t1")).toBe("services");
  });

  it("shows nothing for a success from the window in front, and a toast when it's behind", async () => {
    mockTauri({
      service_impact: () => ({ tier: "low", title: "", lines: [], confirmLabel: "Restart", requireTyping: null }),
      service_action: () => {
        setTimeout(() => void emitMock("tasks://update", task({ state: "ok", finishedMs: Date.now(), summary: "Restarted" })), 200);
        return null;
      },
    });
    setFrontWindowProbe((id) => id === "services", () => "services");
    await listening();
    await runServiceAction(["cell1"], "nvidia-dgx-telemetry.service", "restart", { origin: "services" });
    await vi.advanceTimersByTimeAsync(400);
    expect(getToasts()).toEqual([]);

    setFrontWindowProbe((id) => id === "dashboard", () => "dashboard");
    mockTauri({
      service_impact: () => ({ tier: "low", title: "", lines: [], confirmLabel: "Restart", requireTyping: null }),
      service_action: () => {
        setTimeout(() => void emitMock("tasks://update", task({ id: "t2", opId: "t2", state: "ok", finishedMs: Date.now(), summary: "Restarted" })), 200);
        return null;
      },
    });
    _resetTasksForTests();
    await listening();
    await runServiceAction(["cell1"], "nvidia-dgx-telemetry.service", "restart", { origin: "services" });
    await vi.advanceTimersByTimeAsync(400);
    expect(getToasts().map((t) => t.kind)).toEqual(["success"]);
  });

  it("records an origin for every task: given, or the window the last click went to", async () => {
    setFrontWindowProbe((id) => id === "files", () => "files");
    await listening();
    await runTask("Copy hosts", async () => "Copied", { origin: "files" });
    expect(getToasts()).toEqual([]);
    // No origin given: the window the click landed in.
    const frame = document.createElement("div");
    frame.className = "win";
    frame.dataset.winId = "fleet";
    const button = document.createElement("button");
    frame.appendChild(button);
    document.body.appendChild(frame);
    button.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    setFrontWindowProbe((id) => id === "fleet", () => "fleet");
    await emitMock("tasks://update", task({ id: "t3", opId: "t3", kind: "fleet", title: "uptime on 4 nodes", state: "ok", finishedMs: Date.now() }));
    expect(taskOrigin("t3")).toBe("fleet");
    expect(getToasts()).toEqual([]);
    frame.remove();
  });

  it("claims only the tasks a gesture matches", async () => {
    await listening();
    await withOrigin("containers", async () => {
      await emitMock("tasks://update", task({ id: "t4", opId: "t4", kind: "service", nodes: ["cell2"], state: "running" }));
      await emitMock("tasks://update", task({ id: "t5", opId: "t5", kind: "container", nodes: ["cell1"], state: "running" }));
    }, (t) => t.kind === "container");
    expect(taskOrigin("t5")).toBe("containers");
    expect(taskOrigin("t4")).not.toBe("containers");
  });
});

describe("tasks and their toasts (C7)", () => {
  const base = (over: Partial<Task>): Task => ({
    id: "t1", kind: "model", title: "Restart glm-5.3-flash", nodes: ["cell1"], source: "ui", cancellable: false, state: "running",
    startedMs: Date.now(), finishedMs: null, progress: null, detail: null, rateBps: null, etaS: null, steps: [], log: [],
    summary: null, error: null, cancelRequested: false, retry: null, opId: null, ...over,
  });
  beforeEach(() => _resetTasksForTests());

  it("never moves progress backwards, and holds a warming model at 99% until it has finished", async () => {
    const { ingestTask, getTask } = await import("./tasks");
    ingestTask(base({ progress: 0.5, detail: "Downloading" }));
    ingestTask(base({ progress: 1, detail: "Downloaded" }));
    expect(getTask("t1")?.progress).toBe(0.99);
    // The engine starts loading weights and reports its own, lower, fraction: the bar stays.
    ingestTask(base({ progress: 0.2, detail: "Warming up" }));
    expect(getTask("t1")?.progress).toBe(0.99);
    expect(getTask("t1")?.detail).toBe("Warming up");
    ingestTask(base({ state: "ok", progress: 1, finishedMs: Date.now(), summary: "Answering on cell1:8000" }));
    expect(getTask("t1")?.progress).toBe(1);
    // A finished task's line is its summary, not the step it was on.
    expect(getTask("t1")?.detail).toBe("Answering on cell1:8000");
  });

  it("says a task's outcome under the key its gesture named, so a later toast replaces it", async () => {
    const { ingestTask } = await import("./tasks");
    await withOrigin("models", async () => {
      ingestTask(base({ id: "t2", opId: "op-2" }));
    }, undefined, { key: "endpoint:cell1-vllm-8000" });
    ingestTask(base({ id: "t2", opId: "op-2", state: "failed", finishedMs: Date.now(), error: "Head exited (code 1)" }));
    const failed = getToasts().find((t) => t.kind === "error")!;
    expect(failed.key).toBe("endpoint:cell1-vllm-8000");
    // The endpoint answering again replaces it by key, not by the words in it.
    toast({ kind: "success", key: "endpoint:cell1-vllm-8000", title: "glm-5.3-flash is answering again" });
    expect(getToasts().filter((t) => t.key === "endpoint:cell1-vllm-8000").map((t) => t.title)).toEqual(["glm-5.3-flash is answering again"]);
    // A toast that merely mentions the model is left alone.
    toast({ kind: "error", title: "glm-5.3-flash: benchmark failed" });
    toast({ kind: "success", key: "endpoint:cell1-vllm-8000", title: "glm-5.3-flash is answering again" });
    expect(getToasts().some((t) => t.title === "glm-5.3-flash: benchmark failed")).toBe(true);
  });
});
