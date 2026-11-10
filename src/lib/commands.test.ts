import { describe, expect, it, vi } from "vitest";
import { paletteSections } from "../components/CommandPalette";
import fixture from "../dev/fixtures/snapshot.json";
import {
  activeCommandIds, editDistance, getCommands, matchCommand, rankCommands, registerCommands, runCommand, setActiveScope, setAdminHooks, tokenize,
  type Command,
} from "./commands";
import type { ClusterSnapshot } from "./types";
import { nodeCommands, workloadCommands } from "./workloadCommands";

const cmd = (id: string, title: string, group = "Windows", extra: Partial<Command> = {}): Command => ({ id, title, group, run: () => {}, ...extra });

const CMDS = [
  cmd("go:dashboard", "Dashboard"),
  cmd("go:topology", "Topology"),
  cmd("go:terminal", "Terminal"),
  cmd("node:cell1:terminal", "Terminal on cell1", "Nodes"),
  cmd("wm:tile", "Tile windows", "Window"),
  cmd("cluster:admin", "Unlock Admin mode…", "Cluster", { keywords: ["padlock", "read-only"] }),
  cmd("container:cell1:abc", "vllm-head @ cell1", "Containers", { subtitle: "running · nvcr.io/nvidia/vllm" }),
  cmd("app:settings", "Settings", "App", { keywords: ["preferences"] }),
];

describe("matching", () => {
  it("needs every word typed to start a word of the title, a verb, a keyword or the entity", () => {
    const c = cmd("node:cell3:restart", "Restart cell3…", "Nodes", { entity: { kind: "node", id: "cell3", name: "cell3" }, verbs: ["restart"] });
    expect(matchCommand(c, "restart cell3")).not.toBeNull();
    expect(matchCommand(c, "cell3 restart")).not.toBeNull();
    expect(matchCommand(c, "restart cell4")).toBeNull();
    expect(matchCommand(c, "start")).toBeNull();
  });

  it("forgives one typo in words of four letters or more", () => {
    const c = cmd("x", "Restart cell3…");
    expect(matchCommand(c, "rest cel3")).not.toBeNull();
    expect(matchCommand(c, "retsart")).not.toBeNull();
    expect(matchCommand(c, "cl3")).toBeNull();
    expect(editDistance("cel3", "cell")).toBe(1);
    expect(editDistance("ab", "ba")).toBe(1);
  });

  it("folds verb synonyms: reboot is restart, power off is shut down, show is open", () => {
    expect(tokenize("Reboot cell3")).toEqual(["restart", "cell3"]);
    expect(tokenize("power off cell2")).toEqual(["shut", "cell2"]);
    expect(tokenize("shutdown gx10")).toEqual(["shut", "gx10"]);
    expect(tokenize("show log")).toEqual(["open", "logs"]);
    expect(matchCommand(cmd("x", "Shut down cell3…"), "power off cell3")).not.toBeNull();
  });

  it("matches across separators, so a port's short name finds it", () => {
    expect(matchCommand(cmd("p", "Port qsfp56-dd-2-1 on CRS804 DDQ"), "port 2-1")).not.toBeNull();
    expect(matchCommand(cmd("p", "Port qsfp56-dd-2-1 on CRS804 DDQ"), "port 3-1")).toBeNull();
  });

  it("highlights the matched part of the title", () => {
    expect(matchCommand(cmd("x", "Restart cell3…"), "rest")!.hits).toEqual([0, 1, 2, 3]);
  });
});

describe("palette ranking", () => {
  it("puts the closest title first", () => {
    expect(rankCommands(CMDS, "term")[0].cmd.id).toBe("go:terminal");
    expect(rankCommands(CMDS, "toc")[0].cmd.id).toBe("node:cell1:terminal");
    expect(rankCommands(CMDS, "dash")[0].cmd.id).toBe("go:dashboard");
  });

  it("finds commands by keyword and subtitle", () => {
    expect(rankCommands(CMDS, "padlock")[0].cmd.id).toBe("cluster:admin");
  });

  it("matches a subtitle only when it contains the query", () => {
    const cmds: Command[] = [
      { id: "a", title: "Collector stalled", subtitle: "cell1's numbers stop updating for 30 seconds", group: "Demo", run: () => {} },
      { id: "b", title: "Node offline", subtitle: "cell3 stops answering SSH", group: "Demo", run: () => {} },
    ];
    expect(rankCommands(cmds, "cell3").map((r) => r.cmd.id)).toEqual(["b"]);
    expect(rankCommands(CMDS, "preferences")[0].cmd.id).toBe("app:settings");
    expect(rankCommands(CMDS, "nvcr").map((r) => r.cmd.id)).toContain("container:cell1:abc");
  });

  it("nudges recently used commands up", () => {
    const plain = rankCommands(CMDS, "t").map((r) => r.cmd.id);
    const withRecent = rankCommands(CMDS, "t", ["wm:tile"]).map((r) => r.cmd.id);
    expect(withRecent.indexOf("wm:tile")).toBeLessThan(plain.indexOf("wm:tile"));
  });

  it("keeps the registry's order when nothing is typed", () => {
    expect(rankCommands(CMDS.slice(0, 3), "", [], new Set()).map((r) => r.cmd.id)).toEqual(["go:dashboard", "go:topology", "go:terminal"]);
  });

  it("opens empty with Recent first, then the groups in a fixed order", () => {
    const sections = paletteSections(CMDS, "", ["app:settings", "go:topology"]);
    expect(sections[0].name).toBe("Recent");
    expect(sections[0].rows.map((r) => r.cmd.id)).toEqual(["app:settings", "go:topology"]);
    // (Settings, the only App command, is already under Recent.)
    expect(sections.map((s) => s.name).slice(1)).toEqual(["Windows", "Nodes", "Containers", "Window", "Cluster"]);
    // Recent rows aren't listed twice.
    expect(sections.flatMap((s) => s.rows).filter((r) => r.cmd.id === "app:settings")).toHaveLength(1);
  });

  it("groups search results by section, best section first", () => {
    const sections = paletteSections(CMDS, "cell1", []);
    expect(sections[0].rows[0].cmd.title).toMatch(/cell1/);
  });
});

/** What the palette holds in the Demo world: the real node, container and service commands from
 * the fixture snapshot, plus registrations shaped like the other packages' (models, ports, settings). */
/**
 * The fixture plus the busy cluster the QA queries were written against (the fixture is the
 * cluster at rest now): llama's vLLM pipeline and Open WebUI running, telemetry failed on cell1.
 */
function busySnapshot(): ClusterSnapshot {
  const snap = structuredClone(fixture) as unknown as ClusterSnapshot;
  const like = snap.nodes[0].containers[0];
  const run = (id: string, name: string, image: string) => ({ ...like, id, name, image, state: "running", status: "Up 2 hours", exitCode: 0, finishedAt: 0 });
  snap.nodes[0].containers.push(run("4f1c2a9e7b31", "vllm-head", "vllm/vllm-openai:v0.11.0"), run("b1e09f2a7c44", "open-webui", "ghcr.io/open-webui/open-webui:main"));
  snap.nodes[1].containers.push(run("8a0d33e1c4f2", "vllm-worker", "vllm/vllm-openai:v0.11.0"));
  snap.nodes[2].containers.push(run("c2b7e09f1d64", "vllm-worker", "vllm/vllm-openai:v0.11.0"));
  snap.nodes[0].failedUnits = ["nvidia-dgx-telemetry.service"];
  return snap;
}

function demoPalette(): Command[] {
  const snap = busySnapshot();
  return [
    ...CMDS,
    ...nodeCommands(snap),
    ...workloadCommands(snap),
    cmd("wm:tile-two", "Tile two side by side", "Window"),
    cmd("wm:tile-four", "Tile four in a grid", "Window"),
    cmd("wm:restore-layout", "Restore layout", "Window"),
    ...["Light", "Dark", "Black", "System"].map((t) => cmd(`appearance:theme-${t.toLowerCase()}`, `Theme: ${t}`, "Appearance", { keywords: ["appearance", "colour", "color", "mode"] })),
    cmd("appearance:text-1.12", "Text size: Large", "Appearance", { keywords: ["appearance", "font"] }),
    cmd("settings:appearance", "Settings › Appearance", "Settings", { keywords: ["theme", "dark", "light", "black", "oled", "text size", "font", "motion", "contrast", "accessibility"] }),
    cmd("settings:notifications", "Settings › Notifications", "Settings", { keywords: ["alerts", "notify"] }),
    cmd("model:qwen", "Qwen3-30B-A3B-Instruct", "Models", { entity: { kind: "model", id: "Qwen/Qwen3-30B-A3B-Instruct-2507", name: "Qwen3-30B-A3B-Instruct" }, verbs: ["open", "load"] }),
    cmd("model:llama", "Llama 3.3 70B Instruct", "Models", { entity: { kind: "model", id: "meta-llama/Llama-3.3-70B-Instruct", name: "Llama 3.3 70B Instruct" }, verbs: ["open"] }),
    cmd("port:crs804:qsfp56-dd-2-1", "Port qsfp56-dd-2-1 on CRS804 DDQ", "Switches", { entity: { kind: "port", id: "crs804/qsfp56-dd-2-1", name: "qsfp56-dd-2-1" }, keywords: ["cell3"] }),
    cmd("port:crs804:qsfp56-dd-1-1", "Port qsfp56-dd-1-1 on CRS804 DDQ", "Switches", { entity: { kind: "port", id: "crs804/qsfp56-dd-1-1", name: "qsfp56-dd-1-1" }, keywords: ["cell1"] }),
  ];
}

describe("palette verbs (QA queries)", () => {
  const first = (q: string) => rankCommands(demoPalette(), q, [], new Set())[0]?.cmd.id;

  it.each([
    ["restart cell3", "node:cell3:restart"],
    ["rest cel3", "node:cell3:restart"],
    ["reboot cell3", "node:cell3:restart"],
    ["power off cell3", "node:cell3:shutdown"],
    ["wake cell3", "node:cell3:wake"],
    ["appearance", "settings:appearance"],
    ["qwen", "model:qwen"],
    ["llama", "model:llama"],
    ["port 2-1", "port:crs804:qsfp56-dd-2-1"],
    ["tile", "wm:tile"],
    ["files cell1", "node:cell1:files"],
    ["logs open-webui", "container:cell1:"],
  ])("“%s” finds %s first", (q, id) => {
    if (id.endsWith(":")) expect(first(q)).toMatch(new RegExp(`^${id}[^:]+:logs$`));
    else expect(first(q)).toBe(id);
  });

  it("“theme” offers the theme choices first", () => {
    expect(first("theme")).toMatch(/^appearance:theme-/);
  });

  it("“stop vllm” offers stopping a vLLM container first", () => {
    expect(first("stop vllm")).toMatch(/^container:cell\d:[^:]+:stop$/);
    expect(rankCommands(demoPalette(), "stop vllm", [], new Set())[0].cmd.title).toMatch(/^Stop vllm-(head|worker) on cell\d…$/);
  });

  it("finds a failed service's restart and journal", () => {
    const ids = rankCommands(demoPalette(), "restart telemetry", [], new Set()).map((r) => r.cmd.id);
    expect(ids[0]).toBe("service:cell1:nvidia-dgx-telemetry.service:restart");
    expect(first("journal telemetry")).toBe("service:cell1:nvidia-dgx-telemetry.service:journal");
  });

  it("ranks the active window's own commands higher", () => {
    const cmds = [cmd("a:refresh", "Refresh nodes", "App"), cmd("files:refresh", "Refresh files", "Files")];
    expect(rankCommands(cmds, "refresh", [], new Set())[0].cmd.id).toBe("a:refresh");
    expect(rankCommands(cmds, "refresh", [], new Set(["files:refresh"]))[0].cmd.id).toBe("files:refresh");
  });

  it("marks node power and container actions as Admin commands", () => {
    const all = demoPalette();
    for (const id of ["node:cell3:restart", "node:cell3:shutdown", "container:cell1:" ]) {
      expect(all.filter((c) => c.id.startsWith(id) && /restart|shutdown|stop|shell/.test(c.id)).every((c) => c.needsAdmin)).toBe(true);
    }
  });
});

describe("running commands", () => {
  it("explains instead of running when a command isn't available", async () => {
    const run = vi.fn();
    const off = registerCommands([cmd("x:nope", "Nope", "App", { when: () => "No window is open", run })]);
    expect(await runCommand("x:nope")).toBe(false);
    expect(run).not.toHaveBeenCalled();
    off();
  });

  it("asks to unlock Admin mode first, and stops if refused", async () => {
    const run = vi.fn();
    let unlocked = false;
    const unlock = vi.fn(async () => unlocked);
    setAdminHooks({ isAdmin: () => unlocked, unlock });
    const off = registerCommands([cmd("x:admin", "Restart", "Nodes", { needsAdmin: true, run })]);
    expect(await runCommand("x:admin")).toBe(false);
    expect(unlock).toHaveBeenCalledWith("Restart");
    expect(run).not.toHaveBeenCalled();
    unlocked = true;
    expect(await runCommand("x:admin")).toBe(true);
    expect(run).toHaveBeenCalledOnce();
    off();
    setAdminHooks({ isAdmin: () => false, unlock: async () => false });
  });

  it("shows a window's own commands only while it's the active one", () => {
    const off = registerCommands([cmd("files:upload", "Upload…", "Files")], "files");
    setActiveScope("terminal");
    expect(getCommands().some((c) => c.id === "files:upload")).toBe(false);
    setActiveScope("files");
    expect(getCommands().some((c) => c.id === "files:upload")).toBe(true);
    expect(activeCommandIds().has("files:upload")).toBe(true);
    // The active window's command wins over an app-wide one with the same id, whatever the order.
    const offShell = registerCommands([cmd("term:new-tab", "Shell in vllm-head", "Containers")], "files");
    const offApp = registerCommands([cmd("term:new-tab", "New terminal tab", "Window")]);
    expect(getCommands().find((c) => c.id === "term:new-tab")?.title).toBe("Shell in vllm-head");
    setActiveScope("terminal");
    expect(getCommands().find((c) => c.id === "term:new-tab")?.title).toBe("New terminal tab");
    offShell();
    offApp();
    off();
    setActiveScope(null);
  });
});
