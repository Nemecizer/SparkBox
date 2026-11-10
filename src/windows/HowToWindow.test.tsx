import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { HowToWindow } from "./HowToWindow";
import { handlers } from "../dev/mocks/howto";
import { preparedImageJobs } from "../dev/mocks/jobs";
import { emptyLocal, LIBRARY, type LocalGuide } from "../howto/library";
import { mockCalls, mockTauri } from "../test/tauri";
const state = vi.hoisted(() => ({ source: "demo", admin: true, confirm: true, open: vi.fn(), asked: null as unknown }));
vi.mock("../lib/dataSource", async (o) => ({ ...await o<object>(), useDataSource: () => state.source }));
vi.mock("../lib/store", () => ({ useCluster: () => ({ demo: state.source === "demo", nodes: [{ id: "gx10-0000", name: "cell4", host: "demo" }] }) }));
vi.mock("../lib/admin", () => ({ requestAdmin: async () => state.admin }));
vi.mock("../lib/session", async (o) => ({ ...await o<object>(), confirmAction: async (a: unknown) => { state.asked = a; return state.confirm; } }));
vi.mock("../wm/WindowManager", async (o) => ({ ...await o<object>(), useWM: () => ({ open: state.open }), useCloseGuard: () => {} }));
vi.mock("../components/ui", async (o) => ({ ...await o<object>(), HelpTip: () => null }));
const click = async (name: string) => act(async () => { fireEvent.click(screen.getByRole("button", { name })); });
const tab = async (name: string) => act(async () => { fireEvent.click(screen.getByRole("tab", { name })); });
beforeEach(() => { localStorage.clear(); state.source = "demo"; state.admin = true; state.confirm = true; state.open.mockClear(); mockTauri({ ...handlers, providers_list: () => [{ id: "demo", name: "Demo", kind: "demo", enabled: true, models: ["demo-model"], baseUrl: "", defaultModel: "demo-model", hasKey: false }] }); });
it("reads offline, saves progress, and does not automatically check sources, nodes or AI", async () => {
  render(<HowToWindow />); await waitFor(() => expect(screen.getByRole("button", { name: "Save progress" })).toBeEnabled());
  expect(mockCalls.some((c) => /howto_check_source|howto_check_nodes|chat_send/.test(c.cmd))).toBe(false);
  fireEvent.change(screen.getByLabelText("Guide notes"), { target: { value: "Keep the healthy checkpoint." } }); await click("Save progress");
  expect(mockCalls.find((c) => c.cmd === "howto_save")?.args.request).toMatchObject({ id: "playbook-vllm", notes: "Keep the healthy checkpoint.", edition: null });
  expect(screen.getByText("Progress saved on this Mac.")).toBeInTheDocument();
});
it("fetches a changed source without overwriting notes or saving a local edition", async () => {
  render(<HowToWindow />); await waitFor(() => expect(screen.getByRole("button", { name: "Save progress" })).toBeEnabled());
  fireEvent.change(screen.getByLabelText("Guide notes"), { target: { value: "Unsaved owner notes" } });
  await click("Check NVIDIA for updates"); await click("Fetch source for review");
  await waitFor(() => expect(screen.getByText("NVIDIA source cached for comparison. Save a reviewed local edition when you are ready.")).toBeInTheDocument());
  expect(mockCalls.filter((c) => c.cmd === "howto_fetch_source")).toHaveLength(1);
  expect(mockCalls.some((c) => c.cmd === "howto_save")).toBe(false);
  await tab("Walkthrough"); expect(screen.getByLabelText("Guide notes")).toHaveValue("Unsaved owner notes");
});
it("keeps an AI reply as a draft until explicit save and preserves history", async () => {
  render(<HowToWindow />); await tab("My edition"); await waitFor(() => expect(screen.getByRole("button", { name: "Edit local edition" })).toBeEnabled()); await click("Edit local edition");
  fireEvent.change(screen.getByLabelText("AI provider"), { target: { value: "demo" } }); fireEvent.change(screen.getByLabelText("AI model"), { target: { value: "demo-model" } });
  await click("Review AI request…");
  expect((screen.getByLabelText("Local edition text") as HTMLTextAreaElement).value).toContain("Demo revision");
  expect(mockCalls.some((c) => c.cmd === "howto_save" || c.cmd === "chat_send")).toBe(false);
  await click("Save local edition");
  expect(mockCalls.filter((c) => c.cmd === "howto_save")).toHaveLength(1);
  expect(screen.getByRole("button", { name: "Restore as a new draft" })).toBeInTheDocument();
});
it("preserves a draft after a stale-save refusal", async () => {
  mockTauri({ ...handlers, providers_list: () => [], howto_save: () => { throw new Error("This guide changed in another window. Reload it before saving."); } });
  render(<HowToWindow />); await tab("My edition"); await waitFor(() => expect(screen.getByRole("button", { name: "Edit local edition" })).toBeEnabled()); await click("Edit local edition");
  fireEvent.change(screen.getByLabelText("Local edition text"), { target: { value: "# Keep my unsaved edit" } }); await click("Save local edition");
  expect(screen.getByRole("alert")).toHaveTextContent("another window"); expect(screen.getByLabelText("Local edition text")).toHaveValue("# Keep my unsaved edit");
});
it("prepares only the reviewed image after Admin and confirmation, using stable node ids", async () => {
  render(<HowToWindow params={{ guide: "playbook-unsloth" }} />); await waitFor(() => expect(screen.getByRole("button", { name: "Save progress" })).toBeEnabled());
  fireEvent.click(screen.getByLabelText("cell4"));
  state.admin = false; await click("Prepare image…"); expect(mockCalls.some((c) => c.cmd === "howto_prepare_image")).toBe(false);
  state.admin = true; state.confirm = false; await click("Prepare image…"); expect(mockCalls.some((c) => c.cmd === "howto_prepare_image")).toBe(false);
  state.confirm = true; await click("Prepare image…");
  expect(mockCalls.find((c) => c.cmd === "howto_prepare_image")?.args).toMatchObject({ id: "playbook-unsloth", nodes: ["gx10-0000"], image: "nvcr.io/nvidia/pytorch:25.11-py3", demo: true });
  expect(state.open).toHaveBeenCalledWith("models", { tab: "jobs" });
  expect(preparedImageJobs.at(0)).toMatchObject({ node: "gx10-0000", state: "done", label: "Prepare nvcr.io/nvidia/pytorch:25.11-py3" });
});
it("offers no Spark execution controls for a Station guide", async () => {
  render(<HowToWindow params={{ guide: "playbook-mig" }} />); await waitFor(() => expect(screen.getByRole("button", { name: "Save progress" })).toBeEnabled());
  expect(screen.queryByRole("button", { name: "Check selected nodes" })).not.toBeInTheDocument(); expect(screen.queryByRole("button", { name: "Prepare image…" })).not.toBeInTheDocument();
});
it("retargets an existing window only after protecting its unsaved progress", async () => {
  const view = render(<HowToWindow />); await waitFor(() => expect(screen.getByRole("button", { name: "Save progress" })).toBeEnabled());
  fireEvent.change(screen.getByLabelText("Guide notes"), { target: { value: "Keep this note" } });
  state.confirm = false; view.rerender(<HowToWindow params={{ guide: "playbook-unsloth", nav: "first" }} />);
  await act(async () => {}); expect(screen.getByLabelText("Guide notes")).toHaveValue("Keep this note");
  state.confirm = true; view.rerender(<HowToWindow params={{ guide: "playbook-unsloth", nav: "second" }} />);
  await waitFor(() => expect(screen.getByRole("button", { name: "Prepare image…" })).toBeInTheDocument());
  await waitFor(() => expect(screen.getByLabelText("Guide notes")).toHaveValue(""));
});
it("ignores reads that return after a data-source change", async () => {
  let finish!: (value: LocalGuide) => void;
  mockTauri({ ...handlers, howto_read: ({ demo }) => demo ? new Promise((r) => { finish = r; }) : emptyLocal() });
  const view = render(<HowToWindow />); await act(async () => {}); state.source = "live"; view.rerender(<HowToWindow />);
  await act(async () => finish({ ...emptyLocal(), notes: "stale Demo notes" }));
  expect(screen.getByLabelText("Guide notes")).toHaveValue("");
  expect(LIBRARY.revision).toHaveLength(40);
});
