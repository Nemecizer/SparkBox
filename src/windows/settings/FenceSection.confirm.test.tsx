// Settings › Fence › Open fence… (P1 r2, R2-SAFE): the Touch ID sheet names the fence. The form
// asks fence_confirm with the spec it's about to open (the backend words the sheet from it),
// then fence_open with the same spec; the UI never sends the sheet's words.
import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { defaultHandlers } from "../../dev/mockBackend";
import { _resetDataSourceForTests } from "../../lib/dataSource";
import type { FenceView } from "../../lib/fence";
import { resetStore, startClusterStore } from "../../lib/store";
import { extendMock, mockCalls, mockTauri } from "../../test/tauri";
import { FenceSection } from "./FenceSection";

// The in-app confirmation is answered yes.
vi.mock("../../lib/session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/session")>()),
  confirmAction: vi.fn(async () => true),
}));

const view = (extra: Partial<FenceView> = {}): FenceView => ({
  state: "none", record: null, mine: false, summary: null, remainingMs: 0, keyKind: "keychain", keyId: null, trustedKeyId: null,
  instance: "release", canOpen: true, canTrust: false, why: null, ...extra,
});

async function openForm(confirm: (args: { spec: unknown }) => unknown) {
  resetStore();
  mockTauri(defaultHandlers());
  const opened: unknown[] = [];
  extendMock({
    fence_state: () => view(),
    lease_state: () => [],
    node_identity: () => [],
    fence_confirm: confirm,
    fence_open: ({ spec }: { spec: unknown }) => (opened.push(spec), view({ state: "valid", mine: true })),
  });
  _resetDataSourceForTests("live");
  await act(async () => {
    await startClusterStore();
  });
  render(<FenceSection />);
  fireEvent.click(await screen.findByRole("button", { name: "Agent tests on cell2 (test build)" }));
  fireEvent.click(screen.getByRole("button", { name: "Open fence…" }));
  return opened;
}

describe("Settings › Fence › Open fence…", () => {
  it("asks Touch ID with the spec it opens, then opens that spec", async () => {
    const opened = await openForm(() => ({ unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false }));
    await waitFor(() => expect(opened).toHaveLength(1));
    const asked = mockCalls.filter((c) => c.cmd === "fence_confirm" || c.cmd === "fence_open" || c.cmd === "admin_authenticate");
    expect(asked.map((c) => c.cmd)).toEqual(["fence_confirm", "fence_open"]);
    expect(asked[0].args.spec).toEqual(opened[0]);
    expect(opened[0]).toMatchObject({ nodes: ["cell2"], classes: ["container-test", "process-test", "files-test"], minutes: 480, instance: "dev" });
    // The sheet's words are the backend's: nothing but the spec is sent.
    expect(Object.keys(asked[0].args)).toEqual(["spec"]);
    _resetDataSourceForTests("demo");
  });

  it("a cancelled Touch ID opens nothing", async () => {
    const opened = await openForm(() => {
      throw { kind: "cancelled", message: "Admin mode stays locked." };
    });
    await waitFor(() => expect(mockCalls.some((c) => c.cmd === "fence_confirm")).toBe(true));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(opened).toHaveLength(0);
    expect(mockCalls.some((c) => c.cmd === "fence_open")).toBe(false);
    _resetDataSourceForTests("demo");
  });
});
