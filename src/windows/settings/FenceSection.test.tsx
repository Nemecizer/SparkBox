import { describe, expect, it } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { FenceBanner } from "../../components/FenceBanner";
import { _resetAdminForTests } from "../../lib/admin";
import { _resetDataSourceForTests } from "../../lib/dataSource";
import { fmtLeft, presets, specSummary, type FenceView } from "../../lib/fence";
import { describeError } from "../../lib/errors";
import { emitMock, mockCalls, mockTauri } from "../../test/tauri";
import { FenceSection } from "./FenceSection";

const view = (extra: Partial<FenceView> = {}): FenceView => ({
  state: "none", record: null, mine: false, summary: null, remainingMs: 0, keyKind: "keychain", keyId: null, trustedKeyId: null,
  instance: "dev", canOpen: false, canTrust: false, why: null, ...extra,
});

const record = {
  v: 1, id: "fence-1-ab", instance: "dev" as const, nodes: ["cell2"], classes: ["container-test", "process-test", "files-test"] as const,
  only: {}, createdMs: 1, expiresMs: 2, mac: "Studio", keyId: "0123456789abcdef", note: "Agent tests on cell2", sig: "x",
};

describe("Settings › Fence", () => {
  it("says it needs Live data in Demo, and asks the backend nothing", () => {
    _resetDataSourceForTests("demo");
    mockTauri({});
    render(<FenceSection />);
    expect(screen.getByText("Available with Live data")).toBeInTheDocument();
    expect(mockCalls.filter((c) => c.cmd.startsWith("fence_") || c.cmd === "lease_state" || c.cmd === "node_identity")).toHaveLength(0);
  });

  it("in a debug build that shares the release build's settings: no Trust button, and why", async () => {
    _resetDataSourceForTests("live");
    mockTauri({ fence_state: () => view({ keyId: "0123456789abcdef", trustedKeyId: "0123456789abcdef" }), lease_state: () => [], node_identity: () => [] });
    render(<FenceSection />);
    expect(await screen.findByText("No fence")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Trust this Mac's fence key" })).toBeNull();
    expect(screen.getByText("This test build needs a fence for every change: open it in the release SparkBox. This build shares its settings, so it trusts the key the release build pinned.")).toBeInTheDocument();
    _resetDataSourceForTests("demo");
  });

  it("in the debug build: the state and key, no Open button, and Trust this Mac's fence key", async () => {
    _resetDataSourceForTests("live");
    mockTauri({
      fence_state: () => view({ keyId: "0123456789abcdef", canTrust: true }),
      lease_state: () => [{ node: "cell2", heldBy: null, tag: null, mac: null, ageS: null, mine: false, error: null }],
      node_identity: () => [{ node: "cell2", name: "cell2", pin: { hostname: "cell2-example", lanMac: "fc:9d:05:9b:e9:14", clonedMachineId: true, hostKey: null, pinnedMs: 1 }, now: { hostname: "cell2-example", lanMac: "fc:9d:05:9b:e9:14", machineId: null }, matches: true, note: "machine-id is shared by cell1, cell2 and cell3; pinned hostname and LAN MAC" }],
    });
    render(<FenceSection />);
    expect(await screen.findByText("No fence")).toBeInTheDocument();
    expect(screen.getByText("Software key in the release build's Keychain")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open fence…" })).toBeNull();
    expect(screen.getByRole("button", { name: "Trust this Mac's fence key" })).toBeEnabled();
    expect(await screen.findByText("Free")).toBeInTheDocument();
    expect(screen.getByText("cell2-example · fc:9d:05:9b:e9:14")).toBeInTheDocument();
    expect(screen.getByText("machine-id is shared by cell1, cell2 and cell3; pinned hostname and LAN MAC")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Re-pin identity…" })).toBeEnabled();
    _resetDataSourceForTests("demo");
  });

  it("in look-only, what gives rights is off and says why; nothing asks the backend to change", async () => {
    _resetDataSourceForTests("live");
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: true });
    mockTauri({
      fence_state: () => view({ instance: "release", canOpen: true, keyId: "0123456789abcdef" }),
      lease_state: () => [],
      node_identity: () => [{ node: "cell2", name: "cell2", pin: null, now: { hostname: "cell2-example", lanMac: null, machineId: null }, matches: null, note: null }],
    });
    render(<FenceSection />);
    const open = await screen.findByRole("button", { name: "Open fence…" });
    expect(open).toBeDisabled();
    expect(open.title).toBe("Look-only · turn off in Settings › Security & privacy");
    expect(await screen.findByRole("button", { name: "Re-pin identity…" })).toBeDisabled();
    _resetAdminForTests();
    _resetDataSourceForTests("demo");
  });

  it("in the release build: a form for the test build's fence, and no choice of build or preset of its own", async () => {
    _resetDataSourceForTests("live");
    mockTauri({ fence_state: () => view({ instance: "release", canOpen: true }), lease_state: () => [], node_identity: () => [] });
    render(<FenceSection />);
    expect(await screen.findByRole("button", { name: "Open fence…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Agent tests on cell2 (test build)" })).toBeInTheDocument();
    // What it's for, in its own words.
    expect(screen.getByText("The test build can't change anything on the cluster.")).toBeInTheDocument();
    expect(screen.getByText("First runs, sudo, power, apt, network")).toBeInTheDocument();
    expect(screen.getByText("Agents alone")).toBeInTheDocument();
    expect(screen.getByText("Repeats agents may run by themselves")).toBeInTheDocument();
    for (const gone of [/^T1:/, /^T2:/, /Fenced autonomy/]) expect(screen.queryByText(gone)).toBeNull();
    expect(screen.getByText(/Lets the test build make these changes on these nodes for a limited time, 8 hours at most\. This app doesn't need a fence/)).toBeInTheDocument();
    // What it no longer offers.
    for (const gone of [/P1 Session/, /release\) build/, /The dev build/]) expect(screen.queryByRole("button", { name: gone })).toBeNull();
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
    expect(screen.queryByRole("checkbox", { name: "any" })).toBeNull();
    expect(screen.queryByText("Everything")).toBeNull();
    _resetDataSourceForTests("demo");
  });

  it("in the release build: a fence can name model-load beside model-load-one", async () => {
    _resetDataSourceForTests("live");
    mockTauri({ fence_state: () => view({ instance: "release", canOpen: true }), lease_state: () => [], node_identity: () => [] });
    render(<FenceSection />);
    expect(await screen.findByLabelText("model-load")).toBeInTheDocument();
    expect(screen.getByLabelText("model-load-one")).toBeInTheDocument();
    _resetDataSourceForTests("demo");
  });

  it("in the release build: a fence that failed its check is the test build's trouble, and a fence for this build says it does nothing", async () => {
    _resetDataSourceForTests("live");
    let current = view({ instance: "release", canOpen: true, state: "failed" });
    mockTauri({ fence_state: () => current, lease_state: () => [], node_identity: () => [] });
    const { unmount } = render(<FenceSection />);
    expect(await screen.findByText("Failed its check: the test build's changes are refused until a new fence is opened")).toBeInTheDocument();
    unmount();
    current = view({ instance: "release", canOpen: true, state: "valid", mine: true, remainingMs: 3_600_000, record: { ...record, instance: "release", classes: [...record.classes] } });
    render(<FenceSection />);
    expect(await screen.findByText("Open · 1 h left · for this build, which doesn't need one")).toBeInTheDocument();
    _resetDataSourceForTests("demo");
  });

  it("in the release build: an open fence says it's for the test build", async () => {
    _resetDataSourceForTests("live");
    const open = view({ instance: "release", canOpen: true, state: "valid", remainingMs: 8 * 3_600_000, record: { ...record, classes: [...record.classes] } });
    mockTauri({ fence_state: () => open, lease_state: () => [], node_identity: () => [] });
    render(<FenceSection />);
    expect(await screen.findByText("Open · 8 h left · for the test build")).toBeInTheDocument();
    _resetDataSourceForTests("demo");
  });
});

describe("the fence banner", () => {
  it("shows an open fence, a failed one and an untrusted key, and nothing without a fence", async () => {
    _resetDataSourceForTests("live");
    let current = view();
    mockTauri({ fence_state: () => current });
    const { container } = render(<FenceBanner />);
    await act(async () => {});
    expect(container.textContent).toBe("");
    current = view({ state: "valid", record: { ...record, classes: [...record.classes] }, mine: true, remainingMs: (2 * 60 + 41) * 60_000 + 30_000 });
    await act(async () => emitMock("fence://changed", current));
    expect(screen.getByRole("status").textContent).toContain("Fence open: cell2 · 3 classes · 2 h 41 min left");
    await act(async () => emitMock("fence://changed", view({ state: "failed" })));
    expect(screen.getByRole("alert").textContent).toContain("The fence record failed its check · changes on the cluster are refused");
    expect(screen.getByRole("button", { name: "Open a new fence" })).toBeInTheDocument();
    await act(async () => emitMock("fence://changed", view({ state: "untrusted" })));
    expect(screen.getByRole("status").textContent).toContain("This build doesn't trust the fence key yet");
    _resetDataSourceForTests("demo");
  });

  it("in the release build: it's about the test build, and quiet about what doesn't touch this app", async () => {
    _resetDataSourceForTests("live");
    const release = (extra: Partial<FenceView>) => view({ instance: "release", canOpen: true, ...extra });
    let current = release({ state: "untrusted" });
    mockTauri({ fence_state: () => current });
    const { container } = render(<FenceBanner />);
    await act(async () => {});
    expect(container.textContent).toBe("");
    current = release({ state: "failed" });
    await act(async () => emitMock("fence://changed", current));
    expect(screen.getByRole("alert").textContent).toContain("The fence record failed its check · the test build's changes are refused");
    current = release({ state: "valid", record: { ...record, classes: [...record.classes] }, remainingMs: 8 * 3_600_000 });
    await act(async () => emitMock("fence://changed", current));
    expect(screen.getByRole("status").textContent).toContain("Fence open for the test build: cell2 · 3 classes · 8 h left");
    // One from before this build stopped needing them allows nothing, so there's nothing to say.
    current = release({ state: "valid", mine: true, record: { ...record, instance: "release", classes: [...record.classes] }, remainingMs: 3_600_000 });
    await act(async () => emitMock("fence://changed", current));
    expect(container.textContent).toBe("");
    _resetDataSourceForTests("demo");
  });

  it("shows nothing in Demo", async () => {
    _resetDataSourceForTests("demo");
    mockTauri({ fence_state: () => view({ state: "failed" }) });
    const { container } = render(<FenceBanner />);
    await act(async () => {});
    expect(container.textContent).toBe("");
    expect(mockCalls.some((c) => c.cmd === "fence_state")).toBe(false);
  });
});

describe("fence words", () => {
  it("time left, the Touch ID sheet's summary and the presets", () => {
    expect(fmtLeft((2 * 60 + 41) * 60_000)).toBe("2 h 41 min");
    expect(fmtLeft(35 * 60_000)).toBe("35 min");
    expect(fmtLeft(3 * 3_600_000)).toBe("3 h");
    expect(fmtLeft(20_000)).toBe("under a minute");
    const [t2, ...others] = presets(["cell1", "cell2", "cell3", "gx10-0000"]);
    expect(others).toEqual([]);
    expect(specSummary(t2.spec)).toBe("cell2 · 3 classes · 8 h");
    expect(t2.spec.instance).toBe("dev");
  });

  it("refusals from the write exit name their way out", () => {
    for (const kind of ["fenced", "otherController", "wrongNode"] as const) {
      const d = describeError({ kind, message: "cell2 isn't in the open fence.", node: "cell2" });
      expect(d.actions.map((a) => a.label)).toEqual(["Open Settings › Fence", "What does this mean?"]);
    }
    expect(describeError("SparkBox (dev) is controlling cell2 right now.").kind).toBe("otherController");
    expect(describeError("cell2 answers as cell3-example: changes refused.").kind).toBe("wrongNode");
    expect(describeError("The fence expired at 14:05.").kind).toBe("fenced");
  });

  it("a fence refusal is the test build's, and doesn't ask anyone to open one for the release build", () => {
    const message = "No fence is open, so changes on the cluster are refused. This test build needs one: open it in the release SparkBox (Settings › Fence).";
    const d = describeError({ kind: "fenced", message, node: "cell2" });
    expect(d.message).toBe(`${message} The test build's changes need an open fence that names the node and the kind of change.`);
    expect(d.message).not.toMatch(/for the release build/);
  });
});
