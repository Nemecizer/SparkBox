import { describe, expect, it } from "vitest";
import { filesLayout, switcherLabels } from "./layout";

describe("Files at tile size", () => {
  it("shows one pane with a switcher below 720 px, both above", () => {
    expect(filesLayout(460)).toBe("single");
    expect(filesLayout(719)).toBe("single");
    expect(filesLayout(720)).toBe("split");
    expect(filesLayout(1100)).toBe("split");
    expect(filesLayout(0)).toBe("split"); // not laid out yet
  });

  it("names the panes by place, and by folder when both are on one", () => {
    expect(switcherLabels(["This Mac", "cell1"], ["~", "~/models"])).toEqual(["This Mac", "cell1"]);
    expect(switcherLabels(["cell1", "cell1"], ["/home/sparkuser/models", "/etc/"])).toEqual(["cell1 · models", "cell1 · etc"]);
  });
});
