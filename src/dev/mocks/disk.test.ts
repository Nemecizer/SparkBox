import { afterEach, expect, it } from "vitest";
import { demoDownloadCapacity } from "./disk";
import type { DownloadSelection } from "../../lib/types";
const selection: DownloadSelection = { repo: "fixture/model", revision: "a".repeat(40), files: [{ path: "model.safetensors", size: 100, sha256: null }] };
afterEach(() => window.history.replaceState({}, "", "/"));
it("compares unequal Demo destinations and resolves ties by ID, within the chosen scope", () => {
  window.history.replaceState({}, "", "/?disk=capacity");
  const report = demoDownloadCapacity(selection, ["cell3", "cell1", "cell2"]);
  expect(report.suggested).toBe("cell2");
  expect(report.readings[0].check?.ok).toBe(false);
  expect(demoDownloadCapacity(selection, ["cell3", "cell1"]).suggested).toBe("cell3");
  window.history.replaceState({}, "", "/");
  expect(demoDownloadCapacity(selection, ["cell3", "cell2"]).suggested).toBe("cell2");
});
it.each(["unknown", "low"])("does not suggest a Demo node with %s capacity", (mode) => {
  window.history.replaceState({}, "", `/?disk=${mode}`);
  expect(demoDownloadCapacity(selection, ["cell2", "cell1"]).suggested).toBeNull();
});
it("refuses unknown sizes or invalid identities", () => {
  for (const nodes of [[], ["foreign"], ["cell1", "cell1"]]) expect(() => demoDownloadCapacity(selection, nodes)).toThrow(/distinct inventory nodes/);
  expect(() => demoDownloadCapacity({ ...selection, files: [{ ...selection.files[0], size: null }] }, ["cell1"])).toThrow(/unknown or invalid file sizes/);
});
