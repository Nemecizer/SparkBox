import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { mockCalls, mockTauri } from "../test/tauri";
import { ProtectedUpdates } from "./ProtectedUpdates";
import packages from "../test/fixtures/protected-updates.json";
const state = vi.hoisted(() => ({ demo: true, confirm: true, admin: true, confirmArgs: null as unknown }));
vi.mock("../lib/store", () => ({ useCluster: () => ({ demo: state.demo, nodes: ["cell1", "cell2", "cell3", "gx10-0000"].map((id) => ({ id, name: `Display ${id === "gx10-0000" ? "cell4" : id}` })) }) }));
vi.mock("../lib/admin", () => ({ requestAdmin: async () => state.admin }));
vi.mock("../lib/session", async (original) => ({ ...await original<object>(), confirmAction: async (a: unknown) => { state.confirmArgs = a; return state.confirm; } }));
vi.mock("../lib/tasks", () => ({ waitTask: async () => ({ state: "ok" }), openTasksWindow: vi.fn() }));
vi.mock("../components/ui", async (original) => ({ ...await original<object>(), HelpTip: () => null }));
const plan = { node: "cell2", plan: packages, upgrades: [], installs: [], removals: [], heldBack: [], text: "fixture" };
beforeEach(() => { state.demo = true; state.confirm = true; state.admin = true; mockTauri({ updates_system_status: () => null, updates_system_preview: () => plan, updates_system_start: () => "task" }); });
const click = async (label: string) => act(async () => { fireEvent.click(screen.getByRole("button", { name: label })); });
it("requires review, Admin and typed confirmation, then binds every new and existing version", async () => {
  await act(async () => { render(<ProtectedUpdates />); });
  expect(screen.queryByRole("button", { name: "Update Display cell2 with proposed updates…" })).not.toBeInTheDocument();
  await click("Review system updates"); expect(screen.getByLabelText("Reviewed system packages").children).toHaveLength(21);
  state.admin = false; await click("Update Display cell2 with proposed updates…"); expect(state.confirmArgs).toBeNull();
  state.admin = true; state.confirm = false; await click("Update Display cell2 with proposed updates…"); expect(mockCalls.some((c) => c.cmd === "updates_system_start")).toBe(false);
  expect(state.confirmArgs).toMatchObject({ requireTyping: "Display cell2", danger: true });
  state.confirm = true; await click("Update Display cell2 with proposed updates…");
  expect(mockCalls.find((c) => c.cmd === "updates_system_start")?.args).toEqual({ node: "cell2", plan: packages.map(({ name, to }) => ({ name, to })) });
  expect(mockCalls.some((c) => c.cmd === "node_power")).toBe(false);
});
it("discarded source and node reviews cannot install", async () => {
  let resolve!: (p: typeof plan) => void;
  mockTauri({ updates_system_status: () => null, updates_system_preview: () => new Promise((r) => { resolve = r; }) });
  const v = render(<ProtectedUpdates />); await click("Review system updates");
  state.demo = false; v.rerender(<ProtectedUpdates />);
  await act(async () => { resolve(plan); });
  expect(screen.queryByRole("button", { name: "Update Display cell2 with proposed updates…" })).not.toBeInTheDocument();
  mockTauri({ updates_system_status: () => null, updates_system_preview: () => plan });
  await click("Review system updates"); fireEvent.change(screen.getByLabelText("System update node"), { target: { value: "cell1" } });
  expect(screen.queryByRole("button", { name: "Update Display cell2 with proposed updates…" })).not.toBeInTheDocument();
});
it("keeps a startup warning and reservation visible; checking never restarts or installs", async () => {
  const saved = { node: "cell2", demo: false, task: "t1", phase: "installing", message: "Verify the new boot first." };
  mockTauri({ updates_system_status: () => saved, updates_system_verify: () => { throw new Error("systemd state is starting"); } });
  await act(async () => { render(<ProtectedUpdates />); });
  expect(screen.queryByRole("button", { name: "Review system updates" })).not.toBeInTheDocument();
  await click("Check system update"); expect(screen.getByText(/systemd state is starting/)).toBeInTheDocument();
  expect(screen.getByText(/Verify the new boot first/)).toBeInTheDocument();
  expect(mockCalls.filter((c) => c.cmd === "node_power" || c.cmd === "updates_system_start")).toHaveLength(0);
});

it("shows the backend's structured refusal in plain language", async () => {
  mockTauri({ updates_system_status: () => null, updates_system_preview: () => { throw { kind: "invalid", message: "The previous kernel recovery entry is missing." }; } });
  await act(async () => { render(<ProtectedUpdates />); }); await click("Review system updates");
  expect(screen.getByText("The previous kernel recovery entry is missing.")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Update Display cell2 with proposed updates…" })).not.toBeInTheDocument();
});

it("requires Admin for protected boot reads and keeps a cancelled check reserved", async () => {
  state.admin = false;
  const v = render(<ProtectedUpdates />);
  await click("Review system updates");
  expect(mockCalls.some((c) => c.cmd === "updates_system_preview")).toBe(false);
  v.unmount();
  mockTauri({ updates_system_status: () => ({ node: "cell2", phase: "installing", task: "t1", message: "Verification pending." }) });
  await act(async () => { render(<ProtectedUpdates />); });
  await click("Check system update");
  expect(mockCalls.some((c) => c.cmd === "updates_system_verify")).toBe(false);
  expect(screen.getByText("cell2: Verification pending.", { exact: false })).toBeInTheDocument();
});

const original = { node: "cell2", demo: true, task: "original-task", phase: "installing", message: "Newer packages remain; checkpoint retained." };
it("reviews the exact continuation with Admin and typed confirmation, preserving original task access", async () => {
  const { default: delta } = await import("../test/fixtures/protected-updates-1025.json");
  let saved: typeof original & { previous?: typeof original } = original;
  mockTauri({ updates_system_status: () => saved, updates_system_preview: () => ({ ...plan, plan: delta }),
    updates_system_start: () => { saved = { ...original, task: "continued-task", previous: original }; return "continued-task"; } });
  await act(async () => { render(<ProtectedUpdates />); });
  state.admin = false; await click("Review 1025 continuation");
  expect(mockCalls.some((c) => c.cmd === "updates_system_preview")).toBe(false);
  state.admin = true; await click("Review 1025 continuation");
  expect(screen.getByLabelText("Reviewed system packages").children).toHaveLength(17);
  expect(screen.getByText(/retain kernel 7.0.0-1019-nvidia and kernel 7.0.0-1024-nvidia/)).toBeInTheDocument();
  state.confirm = false; await click("Update Display cell2 with proposed updates…");
  expect(state.confirmArgs).toMatchObject({ title: "Continue to kernel 1025 on Display cell2?", requireTyping: "Display cell2", danger: true });
  expect(mockCalls.some((c) => c.cmd === "updates_system_start")).toBe(false);
  state.confirm = true; await click("Update Display cell2 with proposed updates…");
  expect(mockCalls.find((c) => c.cmd === "updates_system_start")?.args).toEqual({ node: "cell2", plan: delta.map(({ name, to }) => ({ name, to })) });
  expect(screen.getByRole("button", { name: "View original package task" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Review 1025 continuation" })).not.toBeInTheDocument();
  expect(mockCalls.some((c) => c.cmd === "node_power")).toBe(false);
});
it("a refused or closed continuation review leaves the saved reservation intact", async () => {
  mockTauri({ updates_system_status: () => original, updates_system_preview: () => { throw new Error("Linux startup is still starting."); } });
  await act(async () => { render(<ProtectedUpdates />); }); await click("Review 1025 continuation");
  expect(screen.getByText(/Linux startup is still starting/)).toBeInTheDocument();
  expect(screen.getByText(/Newer packages remain/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Update Display cell2 with proposed updates…" })).not.toBeInTheDocument();
  mockTauri({ updates_system_status: () => original, updates_system_preview: () => plan });
  await click("Review 1025 continuation"); await click("Close review");
  expect(screen.queryByRole("button", { name: "Update Display cell2 with proposed updates…" })).not.toBeInTheDocument();
  expect(mockCalls.some((c) => c.cmd === "updates_system_start" || c.cmd === "updates_system_discard_review")).toBe(false);
});
it("an interrupted continuation returns to the original reservation when discarded", async () => {
  const checking = { ...original, phase: "checking", task: null, previous: original };
  mockTauri({ updates_system_status: () => checking, updates_system_discard_review: () => original });
  await act(async () => { render(<ProtectedUpdates />); }); await click("Discard interrupted review");
  expect(screen.getByRole("button", { name: "Review 1025 continuation" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Review system updates" })).not.toBeInTheDocument();
});
it("late continuation reviews from another source cannot install", async () => {
  let resolve!: (p: typeof plan) => void;
  mockTauri({ updates_system_status: () => original, updates_system_preview: () => new Promise((r) => { resolve = r; }) });
  const v = render(<ProtectedUpdates />); await act(async () => {}); await click("Review 1025 continuation");
  state.demo = false; v.rerender(<ProtectedUpdates />);
  await act(async () => { resolve(plan); });
  expect(screen.queryByRole("button", { name: "Update Display cell2 with proposed updates…" })).not.toBeInTheDocument();
});
it("does not offer cell2's continuation for another node's checkpoint", async () => {
  mockTauri({ updates_system_status: () => ({ ...original, node: "cell1" }) });
  await act(async () => { render(<ProtectedUpdates />); });
  expect(screen.queryByRole("button", { name: "Review 1025 continuation" })).not.toBeInTheDocument();
});

it.each([
  { ...plan, plan: [] },
  { ...plan, node: "cell1" },
  { ...plan, refused: "The package plan changed. Review it again." },
])("does not enable the explicit Update action for an unusable review: %j", async (preview) => {
  mockTauri({ updates_system_status: () => null, updates_system_preview: () => preview });
  await act(async () => { render(<ProtectedUpdates />); });
  await click("Review system updates");
  const button = screen.getByRole("button", { name: `Update Display ${preview.node} with proposed updates…` });
  expect(button).toBeDisabled();
  fireEvent.click(button);
  expect(mockCalls.some((c) => c.cmd === "updates_system_start")).toBe(false);
});

it.each(["cell1", "cell3", "gx10-0000"])("reviews the direct 24-package plan for %s with a display-name confirmation", async (node) => {
  const { default: fleet } = await import("../test/fixtures/protected-updates-fleet1025.json");
  mockTauri({ updates_system_status: () => null, updates_system_preview: () => ({ ...plan, node, plan: fleet }), updates_system_start: () => "task" });
  await act(async () => { render(<ProtectedUpdates />); });
  await act(async () => { fireEvent.change(screen.getByLabelText("System update node"), { target: { value: node } }); });
  await click("Review system updates");
  expect(screen.getByLabelText("Reviewed system packages").children).toHaveLength(24);
  expect(screen.getByText(/Direct to kernel 1025: 16 upgrades/)).toBeInTheDocument();
  const label = node === "gx10-0000" ? "cell4" : node;
  await click(`Update Display ${label} with proposed updates…`);
  expect(state.confirmArgs).toMatchObject({ requireTyping: `Display ${label}`, body: expect.stringContaining("Healthy startup and the reviewed persistent recovery-boot setting") });
  expect(mockCalls.find((c) => c.cmd === "updates_system_start")?.args).toEqual({ node, plan: fleet.map(({ name, to }) => ({ name, to })) });
  expect(mockCalls.some((c) => c.cmd === "node_power")).toBe(false);
});
it("keeps completed cell2 and its original task accessible during the next node's update", async () => {
  const { openTasksWindow } = await import("../lib/tasks");
  mockTauri({ updates_system_status: () => ({ node: "cell1", demo: true, task: "next", phase: "installing", fresh1025: true, message: "Verify first.", completed: [{ ...original, phase: "verified", task: "verified-task", previous: original }] }) });
  await act(async () => { render(<ProtectedUpdates />); });
  fireEvent.click(screen.getByText("Completed system updates"));
  await click("View Display cell2 package task"); expect(openTasksWindow).toHaveBeenLastCalledWith("verified-task");
  await click("View Display cell2 original package task"); expect(openTasksWindow).toHaveBeenLastCalledWith("original-task");
  expect(screen.queryByRole("button", { name: "Review system updates" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Review 1025 continuation" })).not.toBeInTheDocument();
});
