// The sidebar's Updates row from the update check's facts in the snapshot (P1.21): no Updates
// window opens and nothing is asked of the backend for it; nothing shows in Demo or before a
// node has been checked.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { resetStore, startClusterStore } from "../lib/store";
import type { ClusterSnapshot, UpdateFacts } from "../lib/types";
import { mockCalls, mockTauri } from "../test/tauri";
import { useWM, WindowManagerProvider } from "../wm/WindowManager";
import { Sidebar, updatesRow } from "./Sidebar";

/** Which windows are open, for what a row's click did. */
function OpenWindows() {
  return <div data-testid="open-windows">{useWM().windows.map((w) => w.kind).join(" ")}</div>;
}

const facts = (pending: number, security: number, more: Partial<UpdateFacts> = {}): UpdateFacts => ({
  pending, security, rebootRequired: false, checkedMs: 1_793_000_000_000, ollamaNewer: false, ...more,
});

/** A Live snapshot with each node's facts as the update check leaves them (null: not checked). */
function live(byNode: (UpdateFacts | null)[]): ClusterSnapshot {
  const s = structuredClone(baseSnapshot) as ClusterSnapshot;
  s.demo = false;
  s.nodes.forEach((n, i) => (n.updates = byNode[i] ?? null));
  return s;
}

describe("the Updates row", () => {
  it("counts security updates across the nodes, with a line per node", () => {
    const s = live([
      facts(4, 3, { firmware: 0, ollamaNewer: true }),
      facts(2, 1, { firmware: 2 }),
      facts(2, 1, { otaPending: "7.7.0" }),
      facts(2, 1),
    ]);
    const names = s.nodes.map((n) => n.name);
    const row = updatesRow(s);
    expect(row.badge?.text).toBe("6 security");
    expect(row.badge?.tone).toBe("warn");
    expect(row.trailing).toBeUndefined();
    expect(row.title).toBe([
      `${names[0]} 4 (3 security) · firmware: none · Ollama update available`,
      `${names[1]} 2 (1 security) · firmware: 2`,
      `${names[2]} 2 (1 security) · DGX OS update 7.7.0 pending`,
      `${names[3]} 2 (1 security)`,
    ].join("\n"));
    expect(row.badge?.title).toBe(row.title);
  });

  it("shows how many are pending when none is security, and only checked nodes", () => {
    const s = live([facts(3, 0), null, facts(0, 0), null]);
    const row = updatesRow(s);
    expect(row.badge).toBeUndefined();
    expect(row.trailing).toBe("3");
    expect(row.title).toBe(`${s.nodes[0].name} 3\n${s.nodes[2].name} packages up to date`);
    expect(updatesRow(live([facts(0, 0), facts(0, 0), facts(0, 0), facts(0, 0)])).trailing).toBeUndefined();
  });

  it("says nothing without facts, and nothing in Demo", () => {
    expect(updatesRow(null)).toEqual({});
    expect(updatesRow(live([null, null, null, null]))).toEqual({});
    const demo = live([facts(4, 3), facts(2, 1), facts(2, 1), facts(2, 1)]);
    demo.demo = true;
    expect(updatesRow(demo)).toEqual({});
  });
});

async function sidebar(snapshot: ClusterSnapshot) {
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => structuredClone(snapshot) }));
  resetStore();
  await startClusterStore();
  let container!: HTMLElement;
  await act(async () => {
    container = render(<WindowManagerProvider sizeOf={() => [1180, 760]} isKnown={() => true}><Sidebar /></WindowManagerProvider>).container;
  });
  await act(async () => new Promise((r) => setTimeout(r, 0)));
  const row = [...container.querySelectorAll<HTMLButtonElement>(".src-row")].find((b) => b.querySelector(".src-label")?.textContent === "Updates")!;
  return { row, calls: mockCalls.map((c) => c.cmd) };
}

describe("the sidebar", () => {
  it("badges Updates from the snapshot alone", async () => {
    const { row, calls } = await sidebar(live([facts(4, 3), facts(2, 1), facts(2, 1), facts(2, 1)]));
    expect(row.querySelector(".src-badge.warn")?.textContent).toBe("6 security");
    expect(row.title).toContain("4 (3 security)");
    expect(calls.filter((c) => c.startsWith("updates_"))).toEqual([]);
  });

  it("shows no badge or count without facts", async () => {
    const { row } = await sidebar(live([null, null, null, null]));
    expect(row.querySelector(".src-badge")).toBeNull();
    expect(row.querySelector(".src-trailing")).toBeNull();
  });
});

describe("the Overview group", () => {
  const overview = async () => {
    mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => structuredClone(live([null, null, null, null])) }));
    resetStore();
    await startClusterStore();
    let root!: HTMLElement;
    await act(async () => {
      root = render(
        <WindowManagerProvider sizeOf={() => [1180, 760]} isKnown={() => true}>
          <Sidebar />
          <OpenWindows />
        </WindowManagerProvider>,
      ).container;
    });
    return root;
  };
  const rows = (root: HTMLElement) => [...root.querySelectorAll<HTMLElement>(".src-section")].find((s) => s.textContent?.startsWith("Overview"))!.querySelectorAll<HTMLButtonElement>(".src-row");

  it("has Cluster metrics after Topology", async () => {
    const root = await overview();
    expect([...rows(root)].map((b) => b.querySelector(".src-label")?.textContent)).toEqual(["Dashboard", "Topology", "Cluster metrics", "Alerts"]);
    expect(rows(root)[2].querySelector("svg.lucide-chart-line"), "the window's own icon").not.toBeNull();
  });

  it("opens that window", async () => {
    const root = await overview();
    await act(async () => {
      fireEvent.click(rows(root)[2]);
    });
    expect(screen.getByTestId("open-windows").textContent).toContain("cluster-metrics");
  });
});
