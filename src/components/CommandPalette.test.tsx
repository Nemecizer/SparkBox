// The palette's fixed order, with no query: Demo's scenario commands sit in their place, not last.
// App.tsx isn't rendered in tests (it's the whole app), so its command and its preload are read
// from the source.
import { describe, expect, it } from "vitest";
import appSource from "../App.tsx?raw";
import type { Command } from "../lib/commands";
import { paletteSections } from "./CommandPalette";

const cmd = (id: string, group: string): Command => ({ id, title: id, group, run: () => {} });

describe("the palette's groups", () => {
  it("puts Demo between Appearance and App", () => {
    const cmds = [cmd("a", "App"), cmd("b", "Demo"), cmd("c", "Appearance"), cmd("d", "Coming soon")];
    expect(paletteSections(cmds, "", []).map((s) => s.name)).toEqual(["Appearance", "Demo", "App", "Coming soon"]);
  });
});

describe("App's own commands", () => {
  it("names Diagnostics for what it does: it opens Settings and runs nothing", () => {
    expect(appSource).toContain('id: "app:doctor", title: "Open Diagnostics…", group: "App", icon: Stethoscope, subtitle: "Opens Settings › Diagnostics"');
    expect(appSource).not.toContain("Run Doctor");
  });

  it("loads the Help index at start, so an info icon has its name", () => {
    expect(appSource).toContain("useEffect(() => void loadHelpMeta().catch(() => {}), []);");
  });
});
