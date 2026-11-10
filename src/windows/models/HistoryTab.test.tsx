import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, expect, it } from "vitest";
import { HistoryTab } from "./HistoryTab";
import { demoHistory } from "../../dev/mocks/deploymentHistory";
import { _resetDataSourceForTests, noteSource } from "../../lib/dataSource";
import { mockCalls, mockTauri } from "../../test/tauri";
import type { DeploymentHistorySource } from "../../lib/modelsApi";

beforeEach(() => { _resetDataSourceForTests("demo"); });

it("labels historical Ready as unverified current state, without any load or restore action", async () => {
  mockTauri({ deployment_history_list: () => demoHistory() });
  render(<HistoryTab />);
  const record = await screen.findByRole("article", { name: "Historical load example-ready" });
  expect(record).toHaveTextContent("Saved state: ready");
  expect(record).toHaveTextContent("Current state: not checked");
  expect(screen.getByRole("button", { name: "Open report file…" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: /restore|resume|unload|forget|chat|restart/i })).toBeNull();
  expect(mockCalls.filter((c) => !c.cmd.startsWith("plugin:")).every((c) => ["deployment_history_list", "admin_state", "get_snapshot"].includes(c.cmd))).toBe(true);
});

it("shows all ten older reports, both warnings, original node checks, and search", async () => {
  mockTauri({ deployment_history_list: () => demoHistory() });
  render(<HistoryTab />);
  await screen.findByRole("combobox", { name: "History source" });
  fireEvent.change(screen.getByRole("combobox", { name: "History source" }), { target: { value: "copy:older-reports-demo.json" } });
  expect(screen.getAllByRole("article")).toHaveLength(10);
  expect(screen.getByText("10 of 10 records · 2 unload warnings in this file")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("checkbox", { name: "Unload warnings only" }));
  expect(screen.getAllByRole("article")).toHaveLength(2);
  const warning = screen.getByRole("article", { name: "Historical load example-05" });
  expect(warning).toHaveTextContent("Older report only. Model name, load settings and start time were not retained.");
  expect(warning).toHaveTextContent("Shared memory remains; original warning kept.");
  fireEvent.click(within(warning).getByText("Recorded node checks"));
  expect(warning).toHaveTextContent("example-semaphore");
  fireEvent.change(screen.getByRole("textbox", { name: "Find history" }), { target: { value: "example-08" } });
  expect(screen.getAllByRole("article")).toHaveLength(1);
  expect(screen.queryByRole("button", { name: /restore|unload|forget|restart/i })).toBeNull();
});

it("does not turn a failed read into an empty success and can retry", async () => {
  let fail = true;
  mockTauri({ deployment_history_list: () => { if (fail) throw new Error("Folder is unreadable"); return { demo: true, folder: null, sources: [], issues: [] }; } });
  render(<HistoryTab />);
  await screen.findByText(/Folder is unreadable/);
  expect(screen.queryByText(/No recovery copies yet/)).toBeNull();
  fail = false;
  fireEvent.click(screen.getByRole("button", { name: "Refresh copies" }));
  await screen.findByText(/No recovery copies yet/);
  expect(screen.queryByText(/Folder is unreadable/)).toBeNull();
});

it("reads only selected bytes in memory, keeps source identity, and closes without saving", async () => {
  _resetDataSourceForTests("live");
  const source = { ...demoHistory().sources[1], name: "older.json", kind: "file" as const };
  mockTauri({ deployment_history_list: () => ({ demo: false, folder: "/example/deployment-history", sources: [], issues: [] }), deployment_history_file: () => source });
  render(<HistoryTab />);
  await screen.findByText(/No recovery copies yet/);
  const file = new File(["[]"], "older.json", { type: "application/json" });
  Object.defineProperty(file, "arrayBuffer", { value: async () => new Uint8Array([91, 93]).buffer });
  fireEvent.change(screen.getByLabelText("Report JSON file"), { target: { files: [file] } });
  await screen.findByText("older.json");
  expect(mockCalls.find((c) => c.cmd === "deployment_history_file")?.args).toEqual({ name: "older.json", bytes: [91, 93] });
  expect(screen.getAllByRole("article")).toHaveLength(10);
  fireEvent.click(screen.getByRole("button", { name: "Close file" }));
  expect(screen.queryByRole("article")).toBeNull();
  expect(mockCalls.filter((c) => !c.cmd.startsWith("plugin:")).every((c) => ["deployment_history_list", "deployment_history_file", "admin_state", "get_snapshot"].includes(c.cmd))).toBe(true);
});

it("rejects an oversized selection before invoking the backend", async () => {
  _resetDataSourceForTests("live");
  mockTauri({ deployment_history_list: () => ({ demo: false, folder: null, sources: [], issues: [] }) });
  render(<HistoryTab />);
  const file = new File(["[]"], "large.json"); Object.defineProperty(file, "size", { value: 4 * 1024 * 1024 + 1 });
  fireEvent.change(screen.getByLabelText("Report JSON file"), { target: { files: [file] } });
  await screen.findByText(/exceeds the 4 MiB/);
  expect(mockCalls.some((c) => c.cmd === "deployment_history_file")).toBe(false);
});

it("drops delayed live file results when switching to Demo", async () => {
  _resetDataSourceForTests("live");
  let resolve!: (source: DeploymentHistorySource) => void;
  const waiting = new Promise<DeploymentHistorySource>((r) => { resolve = r; });
  mockTauri({ deployment_history_list: () => ({ demo: false, folder: null, sources: [], issues: [] }), deployment_history_file: () => waiting });
  render(<HistoryTab />);
  const file = new File(["[]"], "private.json"); Object.defineProperty(file, "arrayBuffer", { value: async () => new ArrayBuffer(0) });
  fireEvent.change(screen.getByLabelText("Report JSON file"), { target: { files: [file] } });
  await waitFor(() => expect(mockCalls.some((c) => c.cmd === "deployment_history_file")).toBe(true));
  mockTauri({ deployment_history_list: () => demoHistory() });
  act(() => noteSource("demo"));
  await act(async () => { resolve({ ...demoHistory().sources[1], name: "private.json", kind: "file" }); await waiting; });
  await screen.findByRole("article", { name: "Historical load example-ready" });
  expect(screen.queryByText("private.json")).toBeNull();
  expect(screen.queryByRole("button", { name: "Close file" })).toBeNull();
});

it("surfaces damaged copies without claiming no history", async () => {
  const demo = demoHistory();
  demo.sources[0].kind = "recovery";
  demo.sources[0].issues = ["The contents do not match the recovery filename's SHA-256."];
  demo.sources[0].records = [];
  mockTauri({ deployment_history_list: () => demo });
  render(<HistoryTab />);
  await screen.findByText(/The contents do not match/);
  expect(screen.queryByText(/No recovery copies yet/)).toBeNull();
  expect(screen.getByText(/checksum could not be verified/)).toBeInTheDocument();
  expect(screen.getByText(/No readable records/)).toBeInTheDocument();
});
