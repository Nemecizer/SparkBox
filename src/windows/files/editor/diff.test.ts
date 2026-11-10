import { describe, expect, it } from "vitest";
import { diffLines, diffStats, hunks, unified } from "./diff";

describe("line diff", () => {
  it("says nothing for identical text", () => {
    expect(unified("a\nb\n", "a\nb\n", "a", "b")).toBe("");
    expect(hunks(diffLines("", ""))).toEqual([]);
  });

  it("marks a changed line with context around it", () => {
    const before = "network:\n  version: 2\n  ethernets:\n    enp1s0f1np1:\n      mtu: 1500\n";
    const after = "network:\n  version: 2\n  ethernets:\n    enp1s0f1np1:\n      mtu: 9000\n";
    expect(unified(before, after, "on disk", "yours")).toBe(
      "--- on disk\n+++ yours\n@@ -2,4 +2,4 @@\n   version: 2\n   ethernets:\n     enp1s0f1np1:\n-      mtu: 1500\n+      mtu: 9000\n",
    );
  });

  it("keeps hunks small in a big file and finds insertions and deletions", () => {
    const lines = Array.from({ length: 400 }, (_, i) => `line ${i}`);
    const after = [...lines];
    after.splice(10, 1); // delete line 10
    after.splice(300, 0, "inserted"); // insert near the end
    const d = diffLines(`${lines.join("\n")}\n`, `${after.join("\n")}\n`);
    expect(diffStats(d)).toEqual({ added: 1, removed: 1 });
    const hs = hunks(d);
    expect(hs).toHaveLength(2);
    expect(hs[0].lines.filter((l) => l.kind === "del").map((l) => l.text)).toEqual(["line 10"]);
    expect(hs[1].lines.find((l) => l.kind === "add")?.text).toBe("inserted");
    expect(hs.every((h) => h.lines.length <= 7)).toBe(true);
  });

  it("numbers both sides", () => {
    const d = diffLines("a\nb\nc\n", "a\nx\nb\nc\n");
    expect(d.map((l) => [l.kind, l.a, l.b])).toEqual([["same", 1, 1], ["add", null, 2], ["same", 2, 3], ["same", 3, 4]]);
  });
});
