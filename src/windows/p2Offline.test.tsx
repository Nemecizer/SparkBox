import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { mockCalls, mockTauri } from "../test/tauri";
import { RecipeImport } from "./models/RecipeImport";
import { EarlyoomPreparation } from "./models/EarlyoomPreparation";
import { ImagePreparation } from "./models/ImagePreparation";
import { RollingUpdates, type RollingPlan } from "./RollingUpdates";
import { CacheGrant } from "./settings/CacheGrant";

const state = vi.hoisted(() => ({ demo: true, confirm: true }));
vi.mock("../lib/store", () => ({ useCluster: () => ({ demo: state.demo, nodes: ["cell1", "cell2"].map((id) => ({ id, name: `Display ${id}`, system: {} })) }) }));
vi.mock("../lib/admin", () => ({ useAdmin: () => true, requestAdmin: async () => true, withAdmin: async (fn: () => unknown) => fn() }));
vi.mock("../lib/session", async (original) => ({ ...await original<object>(), confirmAction: async () => state.confirm }));
vi.mock("../lib/tasks", () => ({ waitTask: async () => ({ state: "ok" }) }));
vi.mock("../wm/WindowManager", () => ({ openWindow: vi.fn() }));

beforeEach(() => { state.demo = true; state.confirm = true; mockTauri({ drop_cache_grant_get: () => null, updates_rolling_status: () => null }); });
const click = async (name: string) => act(async () => { fireEvent.click(screen.getByRole("button", { name })); });

it("imports only after review and applies a draft only when explicitly chosen", async () => {
  const draft = { modelPath: "/models/qwen", tensorParallel: 2, pipelineParallel: 1, recipe: { image: "vllm:test", args: [] }, warnings: ["Choose the host Library folder."] };
  mockTauri({ model_recipe_import: () => draft });
  const use = vi.fn(); render(<RecipeImport nodes={[{ id: "cell1", name: "Display cell1" }]} onUse={use} />);
  fireEvent.click(screen.getByText("Import a launch script as a recipe…"));
  fireEvent.change(screen.getByLabelText("Script node"), { target: { value: "cell1" } });
  state.confirm = false; await click("Read and preview…"); expect(mockCalls).toHaveLength(0);
  state.confirm = true; await click("Read and preview…");
  expect(mockCalls[0]).toMatchObject({ cmd: "model_recipe_import", args: { node: "cell1", folder: "~/glm53" } });
  expect(use).not.toHaveBeenCalled(); expect(screen.getByText("Choose the host Library folder.")).toBeInTheDocument();
  await click("Use in this Load form"); expect(use).toHaveBeenCalledWith(draft);
  expect(mockCalls.some((c) => c.cmd === "model_deploy")).toBe(false);
  fireEvent.change(screen.getByLabelText("Script folder"), { target: { value: "~/another" } });
  expect(screen.queryByRole("button", { name: "Use in this Load form" })).not.toBeInTheDocument();
});

it("image distribution excludes its source and requires confirmation before admitting jobs", async () => {
  mockTauri({ model_image_jobs: () => [] }); render(<ImagePreparation />);
  fireEvent.change(screen.getByLabelText("Image tag"), { target: { value: "vllm/test:1" } });
  fireEvent.click(screen.getByLabelText("Display cell1")); fireEvent.click(screen.getByLabelText("Display cell2"));
  fireEvent.change(screen.getByLabelText("Image source"), { target: { value: "cell1" } });
  state.confirm = false; await click("Prepare image…"); expect(mockCalls).toHaveLength(0);
  state.confirm = true; await click("Prepare image…");
  expect(mockCalls[0]).toMatchObject({ cmd: "model_image_jobs", args: { image: "vllm/test:1", nodes: ["cell2"], source: "cell1" } });
});

it("rolling updates preview with no restarts and never start from the preview alone", async () => {
  const plan: RollingPlan = { id: "p", demo: true, instance: "dev", createdMs: 1, phase: "preview", loads: [], paused: [], message: "", nodes: [{ node: "cell2", versions: [{ name: "curl", to: "1" }], packages: ["curl"], restart: false, phase: "pending", child: null, admittedMs: 0, bootBefore: null, kernel: null, driver: null }] };
  mockTauri({ updates_rolling_status: () => null, updates_rolling_preview: () => plan, updates_rolling_start: () => "task" });
  await act(async () => { render(<RollingUpdates />); });
  fireEvent.click(screen.getByText("Update all nodes one by one…")); await click("Preview routine updates");
  expect(mockCalls.find((c) => c.cmd === "updates_rolling_preview")?.args).toEqual({ nodes: ["cell1", "cell2"], canary: null, restart: [] });
  expect(mockCalls.some((c) => c.cmd === "updates_rolling_start")).toBe(false);
  state.confirm = false; await click("Start reviewed plan…"); expect(mockCalls.some((c) => c.cmd === "updates_rolling_start")).toBe(false);
  state.confirm = true; await click("Start reviewed plan…"); expect(mockCalls.find((c) => c.cmd === "updates_rolling_start")?.args).toEqual({ plan });
});

it("Demo cannot activate a standing grant", async () => {
  await act(async () => { render(<CacheGrant />); });
  expect(screen.getByRole("button", { name: "Grant with Touch ID…" })).toBeDisabled();
  expect(mockCalls.some((c) => c.cmd === "drop_cache_grant_set")).toBe(false);
});


it("an earlyoom response cannot apply after its node, names or data source changed", async () => {
  state.demo = false;
  let complete!: (p: {before: string; after: string; sha256: string}) => void;
  const result = {before: "old", after: "reviewed rule", sha256: "abc"};
  mockTauri({ earlyoom_preview: () => new Promise((resolve) => { complete = resolve; }), earlyoom_apply: () => "Saved" });
  const view = render(<EarlyoomPreparation />);
  fireEvent.change(screen.getByLabelText("earlyoom node"), {target: {value: "cell1"}});
  fireEvent.change(screen.getByLabelText("Recorded comm names"), {target: {value: "VLLM::EngineCor"}});
  await click("Preview earlyoom edit");
  fireEvent.change(screen.getByLabelText("earlyoom node"), {target: {value: "cell2"}});
  await act(async () => { complete(result); });
  expect(screen.queryByRole("button", {name: "Save and restart earlyoom…"})).not.toBeInTheDocument();
  await click("Preview earlyoom edit");
  fireEvent.change(screen.getByLabelText("Recorded comm names"), {target: {value: "VLLM::Worker_TP"}});
  await act(async () => { complete(result); });
  expect(screen.queryByText("reviewed rule")).not.toBeInTheDocument();
  await click("Preview earlyoom edit");
  await act(async () => { complete(result); });
  state.demo = true; view.rerender(<EarlyoomPreparation />);
  expect(screen.queryByText("reviewed rule")).not.toBeInTheDocument();
  state.demo = false; view.rerender(<EarlyoomPreparation />);
  await click("Save and restart earlyoom…");
  expect(mockCalls.find((c) => c.cmd === "earlyoom_apply")?.args).toEqual({node: "cell2", names: ["VLLM::Worker_TP"], sha256: "abc"});
});
