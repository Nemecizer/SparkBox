// The node window's power buttons offer what the sidebar's menu offers: Wake for any node that
// isn't online, not just one SparkBox shut down itself.
import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests } from "../lib/admin";
import { resetStore, startClusterStore } from "../lib/store";
import type { ClusterSnapshot, NodeSnapshot } from "../lib/types";
import { mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { NodeDetail } from "./NodeDetail";

// Its charts need a canvas; only the toolbar is under test.
vi.mock("../components/TimeChart", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../components/TimeChart")>()),
  TimeChart: () => null,
}));

async function show(patch: Partial<NodeSnapshot>) {
  resetStore();
  const snap = { ...(structuredClone(baseSnapshot) as ClusterSnapshot), tsMs: Date.now() };
  const id = snap.nodes[0].id;
  Object.assign(snap.nodes[0], patch);
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => snap }));
  _resetAdminForTests();
  await startClusterStore();
  await act(async () => {
    render(<WindowManagerProvider sizeOf={() => [1000, 700]} isKnown={() => true}><NodeDetail params={{ id }} /></WindowManagerProvider>);
  });
}

const offered = () => screen.getAllByRole("button").map((b) => b.querySelector(".tool-label")?.textContent).filter((l) => l === "Wake" || l === "Restart…" || l === "Shut down…");

describe("Wake in the node window", () => {
  it("shows for a node that dropped off by itself, beside the buttons that say it isn't online", async () => {
    await show({ status: "offline" });
    expect(offered()).toEqual(["Wake", "Restart…", "Shut down…"]);
  });

  it("shows for one that's connecting or in error", async () => {
    await show({ status: "connecting" });
    expect(offered()).toContain("Wake");
    await show({ status: "error" });
    expect(offered()).toContain("Wake");
  });

  it("shows alone for a node SparkBox shut down", async () => {
    await show({ status: "offline", powerState: "off" });
    expect(offered()).toEqual(["Wake"]);
  });

  it("doesn't show for a node that's online", async () => {
    await show({ status: "online" });
    expect(offered()).toEqual(["Restart…", "Shut down…"]);
  });
});
