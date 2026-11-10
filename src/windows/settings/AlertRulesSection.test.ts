// Settings › Alerts' headings match the rules: none is empty, and none of the rules goes unheaded.
import { describe, expect, it } from "vitest";
import rules from "../../../src-tauri/src/health/fixtures/default-rules.json";
import { AREAS } from "./AlertRulesSection";

describe("the rule headings", () => {
  const areas = new Set(rules.rules.map((r) => r.area));

  it("each has rules: the RoCE rules are Fabric's, so there's no RoCE heading", () => {
    expect(AREAS.map((a) => a.label)).toEqual(["Nodes", "Disk", "GPU", "Fabric", "Containers & services", "Serving"]);
    for (const a of AREAS) expect(areas.has(a.id), a.id).toBe(true);
  });

  it("every rule has one", () => {
    for (const a of areas) expect(AREAS.some((x) => x.id === a), a).toBe(true);
  });
});
