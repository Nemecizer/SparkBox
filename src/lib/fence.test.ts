// The fence presets and the open form (src/lib/fence.ts, Settings › Fence), as the release build
// shows them: a fence is for the test build only, so the one preset is T2's, and Open fence… sends
// exactly what the form says, always for the test build, to fence_confirm and fence_open. The
// Session A and Session B presets left with the release build's own fence on 2026-10-30.
import { createElement } from "react";
import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { defaultHandlers } from "../dev/mockBackend";
import { FenceSection } from "../windows/settings/FenceSection";
import { _resetDataSourceForTests } from "./dataSource";
import { AUTONOMY_CLASSES, ONLY_CLASSES, OWNER_CLASSES, presets, specSummary, type FenceSpec, type FenceView } from "./fence";
import { confirmAction } from "./session";
import { resetStore, startClusterStore } from "./store";
import { extendMock, mockCalls, mockTauri } from "../test/tauri";

// The in-app confirmation is answered yes.
vi.mock("./session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./session")>()),
  confirmAction: vi.fn(async () => true),
}));

const FOUR = ["cell1", "cell2", "cell3", "gx10-0000"];

const view = (extra: Partial<FenceView> = {}): FenceView => ({
  state: "none", record: null, mine: false, summary: null, remainingMs: 0, keyKind: "keychain", keyId: null, trustedKeyId: null,
  instance: "release", canOpen: true, canTrust: false, why: null, ...extra,
});

/** Settings › Fence in the release build, with Live data; `opened` collects what Open fence… sends. */
async function releaseForm(): Promise<{ opened: FenceSpec[]; done: () => void }> {
  resetStore();
  mockTauri(defaultHandlers());
  const opened: FenceSpec[] = [];
  extendMock({
    fence_state: () => view(),
    lease_state: () => [],
    node_identity: () => [],
    fence_confirm: () => ({ unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false }),
    fence_open: ({ spec }: { spec: FenceSpec }) => (opened.push(spec), view({ state: "valid", record: null })),
  });
  _resetDataSourceForTests("live");
  await act(async () => {
    await startClusterStore();
  });
  const { unmount } = render(createElement(FenceSection));
  await screen.findByRole("button", { name: "Open fence…" });
  return {
    opened,
    done: () => {
      unmount();
      _resetDataSourceForTests("demo");
    },
  };
}

const ticked = (names: readonly string[]) => names.filter((n) => (screen.getByRole("checkbox", { name: n }) as HTMLInputElement).checked);
const value = (label: string) => (screen.getByLabelText(label) as HTMLInputElement).value;

describe("The presets", () => {
  it("are T2's alone: cell2, three classes, 8 hours, for the test build", () => {
    const all = presets(FOUR);
    expect(all.map((p) => p.label)).toEqual(["Agent tests on cell2 (test build)"]);
    expect(all[0].spec).toEqual({ nodes: ["cell2"], classes: ["container-test", "process-test", "files-test"], only: {}, minutes: 480, instance: "dev", note: "Agent tests on cell2" });
    expect(specSummary(all[0].spec)).toBe("cell2 · 3 classes · 8 h");
    expect(all.every((p) => p.spec.instance === "dev")).toBe(true);
  });

  it("T2 is what Settings opens", async () => {
    const { opened, done } = await releaseForm();
    fireEvent.click(screen.getByRole("button", { name: "Agent tests on cell2 (test build)" }));
    fireEvent.click(screen.getByRole("button", { name: "Open fence…" }));
    await waitFor(() => expect(opened).toHaveLength(1));
    expect(mockCalls.find((c) => c.cmd === "fence_confirm")?.args.spec).toEqual(opened[0]);
    expect(opened[0]).toEqual(presets(FOUR)[0].spec);
    done();
  });

  // A preset only fills the form. Nothing is confirmed or opened until Open fence….
  it("pressing T2 fills the form and opens nothing", async () => {
    vi.mocked(confirmAction).mockClear();
    const { opened, done } = await releaseForm();
    const [p] = presets(FOUR);
    fireEvent.click(screen.getByRole("button", { name: p.label }));
    expect(ticked(FOUR)).toEqual(p.spec.nodes);
    expect(ticked([...OWNER_CLASSES, ...AUTONOMY_CLASSES]).sort()).toEqual([...p.spec.classes].sort());
    for (const c of p.spec.classes.filter((c) => ONLY_CLASSES[c])) expect(value(`Only (${c})`)).toBe("");
    expect(value("Hours")).toBe("8");
    expect(value("Note")).toBe("Agent tests on cell2");
    expect(opened).toHaveLength(0);
    expect(confirmAction).not.toHaveBeenCalled();
    expect(mockCalls.filter((c) => c.cmd === "fence_confirm" || c.cmd === "fence_open")).toHaveLength(0);
    done();
  });
});

describe("The open form", () => {
  it("makes a fence by hand for the test build, narrowed where the owner narrows it", async () => {
    vi.mocked(confirmAction).mockClear();
    const { opened, done } = await releaseForm();
    fireEvent.click(screen.getByRole("checkbox", { name: "cell3" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "service" }));
    fireEvent.change(screen.getByLabelText("Only (service)"), { target: { value: "nginx.service, earlyoom.service" } });
    fireEvent.change(screen.getByLabelText("Hours"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "Open fence…" }));
    await waitFor(() => expect(opened).toHaveLength(1));
    expect(opened[0]).toEqual({ nodes: ["cell3"], classes: ["service"], only: { service: ["nginx.service", "earlyoom.service"] }, minutes: 120, instance: "dev", note: "" });
    // The confirmation says who it's for.
    const asked = vi.mocked(confirmAction).mock.calls[0][0];
    expect(asked.body).toBe("The test build may make changes of these classes on cell3 until the fence runs out or you close it. It asks for Touch ID.");
    done();
  });

  it("has no way to open a fence for the release build, or an `any` fence", async () => {
    const { done } = await releaseForm();
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
    expect(screen.queryByRole("checkbox", { name: "any" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Session/ })).toBeNull();
    done();
  });
});
