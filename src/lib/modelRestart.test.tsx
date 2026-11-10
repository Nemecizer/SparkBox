import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ConfirmHost } from "../components/ConfirmHost";
import { restartEndpoint, restartModel } from "../windows/models/endpointActions";
import { mockCalls, mockTauri } from "../test/tauri";
import { _resetAdminForTests } from "./admin";
import type { Endpoint } from "./endpoints";
import type { Task } from "./opsApi";
import { retryTask } from "./tasks";
import { RESTART_WITH_CACHE_DROP } from "./modelRestart";

const endpoint: Endpoint = {
  id: "cell1-vllm-8000", engine: "vllm", models: ["qwen"], nodes: ["cell1"], headNode: "cell1",
  port: 8000, url: "http://cell1:8000/v1", managedBy: "sparkbox", health: "ready", parallel: null,
  containerIds: ["abc"], deploymentId: "m1", source: "deployment",
};
const restarts = () => mockCalls.filter((c) => c.cmd === "deployment_restart" || c.cmd === "endpoint_restart");

beforeEach(() => {
  _resetAdminForTests({ unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false });
  mockTauri({
    deployment_restart: () => "restart-task",
    endpoint_restart: () => "external-task",
    deployments_list: () => [{ spec: { id: "m1", engine: "vllm", servedName: "qwen", nodes: ["cell1"] }, state: "ready" }],
  });
  render(<ConfirmHost />);
});
afterEach(() => _resetAdminForTests());

async function begin(action: () => Promise<unknown>) {
  let pending!: Promise<unknown>;
  await act(async () => { pending = action(); });
  return { pending };
}
async function answer(label: string, pending: Promise<unknown>) {
  fireEvent.click(screen.getByRole("button", { name: label }));
  await act(async () => { await pending; });
}

describe("manual restart cache consent", () => {
  it("defaults to Restart without a new grant; the separate option explains its lifetime", async () => {
    const { pending } = await begin(() => restartEndpoint(endpoint));
    expect(screen.getByRole("button", { name: "Restart" })).toHaveFocus();
    expect(screen.getByRole("alertdialog")).toHaveTextContent("not saved for later restarts or Keep running");
    await answer("Restart", pending);
    expect(restarts()).toEqual([{ cmd: "deployment_restart", args: { id: "m1", dropCachesOnce: false } }]);
  });

  it("grants only an explicitly chosen attempt and deduplicates double clicks", async () => {
    const { pending } = await begin(() => Promise.all([restartEndpoint(endpoint), restartEndpoint(endpoint)]));
    expect(screen.getAllByRole("alertdialog")).toHaveLength(1);
    await answer(RESTART_WITH_CACHE_DROP, pending);
    expect(restarts()).toEqual([{ cmd: "deployment_restart", args: { id: "m1", dropCachesOnce: true } }]);
    const next = await begin(() => restartEndpoint(endpoint));
    await answer("Restart", next.pending);
    expect(restarts().at(-1)?.args.dropCachesOnce).toBe(false);
  });

  it("Cancel sends no restart", async () => {
    const { pending } = await begin(() => restartEndpoint(endpoint));
    await answer("Cancel", pending);
    expect(restarts()).toEqual([]);
  });

  it.each(["external", "sglang"])("does not offer a page-cache grant for %s", async (kind) => {
    const e: Endpoint = kind === "external"
      ? { ...endpoint, deploymentId: null, managedBy: "external", source: "discovered" }
      : { ...endpoint, engine: "sglang" };
    const { pending } = await begin(() => restartEndpoint(e));
    expect(screen.queryByRole("button", { name: RESTART_WITH_CACHE_DROP })).toBeNull();
    await answer("Restart", pending);
    expect(restarts()[0]).toEqual(kind === "external"
      ? { cmd: "endpoint_restart", args: { id: endpoint.id } }
      : { cmd: "deployment_restart", args: { id: "m1", dropCachesOnce: false } });
  });

  it("bulk restart adds no grant when its own confirmation was answered", async () => {
    await act(async () => { await restartModel(endpoint, { confirmed: true }); });
    expect(restarts()[0]?.args).toEqual({ id: "m1", dropCachesOnce: false });
  });

  it("Tasks Retry asks anew and discards even a grant present in old retry arguments", async () => {
    const task: Task = {
      id: "old-task", kind: "model", title: "Restart qwen", nodes: ["cell1"], source: "ui", cancellable: false,
      state: "failed", startedMs: 1, finishedMs: 2, progress: null, detail: null, rateBps: null, etaS: null,
      steps: [], log: [], summary: null, error: "memory", cancelRequested: false, opId: "old-task",
      retry: { command: "deployment_restart", args: { id: "m1", dropCachesOnce: true } },
    };
    const first = await begin(() => retryTask(task));
    await answer("Restart", first.pending);
    expect(restarts()[0]?.args).toEqual({ id: "m1", dropCachesOnce: false });
    const second = await begin(() => retryTask(task));
    await answer(RESTART_WITH_CACHE_DROP, second.pending);
    expect(restarts()[1]?.args).toEqual({ id: "m1", dropCachesOnce: true });
    const third = await begin(() => retryTask(task));
    await answer("Cancel", third.pending);
    expect(restarts()).toHaveLength(2);
  });

  it("look-only prevents consent and dispatch", async () => {
    _resetAdminForTests({ unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: true });
    const { pending } = await begin(() => restartEndpoint(endpoint));
    expect(screen.queryByRole("button", { name: RESTART_WITH_CACHE_DROP })).toBeNull();
    await answer("OK", pending);
    expect(restarts()).toEqual([]);
  });

  it("a relock during confirmation still requires an Admin unlock", async () => {
    const { pending } = await begin(() => restartEndpoint(endpoint));
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: RESTART_WITH_CACHE_DROP })); });
    expect(screen.getByRole("alertdialog")).toHaveAccessibleName("Unlock Admin mode?");
    await answer("Cancel", pending);
    expect(restarts()).toEqual([]);
  });
});
