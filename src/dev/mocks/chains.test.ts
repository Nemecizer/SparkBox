import { afterEach, expect, it, vi } from "vitest";
import { demoDownloadTools } from "./jobs";
afterEach(() => { vi.useRealTimers(); vi.resetModules(); history.replaceState({}, "", "/"); });
const selection = { repo: "fixture/model", revision: "a".repeat(40), files: [{ path: "model.safetensors", size: 10, sha256: null }] };
it.each(["", "source-fail", "copy-fail"])("Demo chain %s exposes only verified library copies", async (fault) => {
  history.replaceState({}, "", `/?chain=${fault}`); vi.useFakeTimers();
  const api = await import("./chains");
  const review = api.reviewDemoChain(selection, "cell1", ["cell2", "cell3"]);
  api.startDemoChain(review, { node: "cell1", tool: demoDownloadTools("cell1").options[0], installConsent: false });
  expect(Object.keys(api.demoChainLibrary()[0].copies)).toEqual([]);
  await vi.advanceTimersByTimeAsync(9000);
  expect(Object.keys(api.demoChainLibrary()[0].copies)).toEqual(fault === "source-fail" ? [] : ["cell1"]);
  await vi.advanceTimersByTimeAsync(12000);
  expect(Object.keys(api.demoChainLibrary()[0].copies)).toEqual(fault === "source-fail" ? [] : fault === "copy-fail" ? ["cell1", "cell3"] : ["cell1", "cell2", "cell3"]);
  const rows = api.demoChainJobs();
  expect(rows[0].state).toBe(fault ? "failed" : "done");
  api.cancelDemoChain(rows[0].node, rows[0].id);
  expect(api.demoChainJobs()).toHaveLength(3);
});
it("cancel stops pending Demo jobs without adding copies or permitting a silent retry", async () => {
  const api = await import("./chains");
  const review = api.reviewDemoChain(selection, "cell1", ["cell2"]);
  const tool = { node: "cell1", tool: demoDownloadTools("cell1").options[0], installConsent: false };
  const rows = api.startDemoChain(review, tool);
  api.cancelDemoChain(rows[1].node, rows[1].id);
  expect(api.demoChainJobs().every((j) => j.state === "cancelled")).toBe(true);
  expect(Object.keys(api.demoChainLibrary()[0].copies)).toEqual([]);
  expect(() => api.startDemoChain(review, tool)).toThrow(/already contain/);
});
