// Maintenance mode (P1.16) as the UI shows and asks for it: the node header's and status bar's
// words, the palette's rows (Live only, refused in look-only), and the commands they send.
import { describe, expect, it, vi } from "vitest";
import { rowReason } from "../components/CommandPalette";
import { nodeGlyph, nodeGlyphWord, nodeText } from "../components/StatusBar";
import { mockCalls, mockTauri } from "../test/tauri";
import { LOOK_ONLY_TIP } from "./admin";
import { unavailableReason } from "./commands";
import { beginMaintenance, endMaintenance, inMaintenance, maintenanceCommands, maintenanceSpan, maintenanceText, MANUAL_REASON } from "./maintenance";
import { getToasts } from "./toasts";
import type { NodeSnapshot } from "./types";

// Admin mode is open: the commands go straight to the backend.
vi.mock("./admin", async (importOriginal) => ({ ...(await importOriginal<typeof import("./admin")>()), withAdmin: vi.fn(async (fn: () => Promise<unknown>) => fn()) }));

const T = new Date(2026, 9, 30, 10, 42).getTime();
const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const node = (patch: Partial<NodeSnapshot>): NodeSnapshot => ({ id: "cell2", name: "cell2", host: "192.168.50.154", status: "online", nets: [], latencyMs: 22, ...patch }) as NodeSnapshot;
const forAnHour = node({ maintenance: { reason: MANUAL_REASON, startedMs: T, untilMs: T + 3_600_000 } });
const untilEnded = node({ maintenance: { reason: MANUAL_REASON, startedMs: T, untilMs: null } });
const restarting = node({ status: "offline", maintenance: { reason: "restarting", startedMs: T, untilMs: T + 480_000 } });

describe("maintenance mode", () => {
  it("reads as the owner's window: for 1 h, or until it's ended", () => {
    expect(maintenanceText(forAnHour)).toBe(`cell2 in maintenance for 1 h · until ${clock(T + 3_600_000)}`);
    expect(maintenanceText(untilEnded)).toBe("cell2 in maintenance until you end it");
    expect(maintenanceSpan({ reason: MANUAL_REASON, startedMs: T, untilMs: T + 90 * 60_000 })).toBe(`for 90 min · until ${clock(T + 90 * 60_000)}`);
    // A restart SparkBox started is planned work too, but not maintenance mode.
    expect(inMaintenance(restarting)).toBe(false);
    expect(maintenanceText(restarting)).toBeNull();
  });

  it("shows in the status bar and on the node's glyph, up or down", () => {
    expect(nodeText(forAnHour)).toBe("In maintenance");
    expect(nodeText({ ...untilEnded, status: "offline" })).toBe("In maintenance");
    expect(nodeGlyph(forAnHour, null, { nodes: [forAnHour] })).toBe("planned");
    expect(nodeGlyphWord(untilEnded, "planned")).toBe("In maintenance until you end it");
    // SparkBox's own restart and a node in no window keep their words.
    expect(nodeGlyphWord(restarting, "planned")).toBe("Restarting (SparkBox)");
    expect(nodeText(node({}))).toBe("22 ms");
  });

  it("offers begin and end in the palette, with Live data only, and look-only refuses them", () => {
    const idle = node({ id: "cell1", name: "cell1" });
    const cmds = maintenanceCommands({ demo: false, nodes: [idle, untilEnded] });
    const by = Object.fromEntries(cmds.map((c) => [c.id, c]));
    expect(cmds.map((c) => c.title)).toEqual([
      "Put cell1 in maintenance for 1 h", "Put cell1 in maintenance until I end it", "End maintenance on cell1",
      "Put cell2 in maintenance for 1 h", "Put cell2 in maintenance until I end it", "End maintenance on cell2",
    ]);
    expect(cmds.every((c) => c.needsAdmin && c.group === "Nodes")).toBe(true);
    expect(unavailableReason(by["node:cell1:maintenance-1h"])).toBeNull();
    expect(unavailableReason(by["node:cell1:maintenance-end"])).toBe("cell1 isn't in maintenance");
    expect(unavailableReason(by["node:cell2:maintenance-open"])).toBe("cell2 is in maintenance until you end it");
    expect(unavailableReason(by["node:cell2:maintenance-end"])).toBeNull();
    expect(rowReason(by["node:cell1:maintenance-1h"], true)).toBe(LOOK_ONLY_TIP);
    const demo = maintenanceCommands({ demo: true, nodes: [idle] });
    expect(demo.map((c) => unavailableReason(c))).toEqual(["Available with Live data", "Available with Live data", "Available with Live data"]);
  });

  it("sends the node and the minutes (none: until it's ended)", async () => {
    mockTauri({
      maintenance_begin: (a) => ({ reason: MANUAL_REASON, startedMs: T, untilMs: a.minutes ? T + a.minutes * 60_000 : null }),
      maintenance_end: () => null,
    });
    await beginMaintenance(node({}), 60);
    await beginMaintenance(node({}), null);
    await endMaintenance(node({}));
    expect(mockCalls.filter((c) => c.cmd.startsWith("maintenance_")).map((c) => [c.cmd, c.args])).toEqual([
      ["maintenance_begin", { node: "cell2", minutes: 60 }],
      ["maintenance_begin", { node: "cell2", minutes: null }],
      ["maintenance_end", { node: "cell2" }],
    ]);
    const titles = getToasts().map((t) => t.title);
    expect(titles).toContain(`cell2 in maintenance for 1 h · until ${clock(T + 3_600_000)}`);
    expect(titles).toContain("Maintenance ended on cell2");
  });
});

// ---- R2-OPS ----
describe("maintenance mode's words and scope", () => {
  it("says the node agents may still page once any node has one", async () => {
    const { quietText } = await import("./maintenance");
    expect(quietText({ nodes: [node({})] })).toBe("its alerts are planned and page nobody");
    expect(quietText({ nodes: [node({}), node({ id: "cell3", agent: { lastBeatMs: T } as NodeSnapshot["agent"] })] })).toBe(
      "its alerts are planned and SparkBox pages nobody about them; the node agents aren't told and may still page",
    );
    const cmds = maintenanceCommands({ demo: false, nodes: [node({ agent: { lastBeatMs: T } as NodeSnapshot["agent"] })] });
    expect(cmds[0].subtitle).toBe("Its alerts are planned and SparkBox pages nobody about them; the node agents aren't told and may still page");
  });

  it("asks for Mac scope for until I end it, node scope for an hour", async () => {
    const { withAdmin } = await import("./admin");
    mockTauri({ maintenance_begin: (a) => ({ reason: MANUAL_REASON, startedMs: T, untilMs: a.minutes ? T + a.minutes * 60_000 : null }) });
    vi.mocked(withAdmin).mockClear();
    await beginMaintenance(node({}), 60);
    await beginMaintenance(node({}), null);
    expect(vi.mocked(withAdmin).mock.calls.map((c) => c[1])).toEqual([
      { action: "maintenance_begin", scope: "node" },
      { action: "maintenance_begin", scope: "mac" },
    ]);
  });
});
