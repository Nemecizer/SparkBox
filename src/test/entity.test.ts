import { describe, expect, it } from "vitest";
import { entityTarget } from "../components/EntityLink";
import type { ClusterSnapshot } from "../lib/types";
// The Demo world (the browser preview's snapshot.json is the real cluster at rest, with no
// switch addresses or test containers).
import demo from "../dev/fixtures/demo-snapshot.json";

const baseSnapshot = demo as unknown as ClusterSnapshot;

describe("EntityLink targets", () => {
  it("opens nodes and switches in their own windows, with key numbers in the tooltip", () => {
    const n = entityTarget(baseSnapshot, "node", "cell1");
    expect(n).toMatchObject({ kind: "node", params: { id: "cell1" }, label: "cell1" });
    expect(n.title).toMatch(/^cell1 — GPU \d+% · CPU \d+% · .+ memory$/);
    const s = entityTarget(baseSnapshot, "switch", "crs804");
    expect(s).toMatchObject({ kind: "switch", params: { id: "crs804", name: "CRS804 DDQ" } });
    expect(s.title).toContain("192.168.50.2");
  });

  it("opens a container's Inspect window, or its node with ⌘", () => {
    const c = entityTarget(baseSnapshot, "container", "sglang-test", "gx10-0000");
    expect(c.kind).toBe("inspect");
    expect(c.params).toMatchObject({ node: "gx10-0000", name: "sglang-test" });
    expect(c.title).toContain("exited");
    expect(entityTarget(baseSnapshot, "container", "sglang-test", "gx10-0000", true)).toMatchObject({ kind: "node", params: { id: "gx10-0000" } });
    expect(entityTarget(baseSnapshot, "service", "nvidia-dgx-telemetry.service", "cell1").title).toContain("failed");
  });
});
