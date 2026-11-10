// The other SparkBox's containers (P1.2): one whose sparkbox.instance label isn't this build's
// says "managed by the other SparkBox", can be selected and read, and every action on it is
// greyed with that reason. This build's own, and unlabelled ones, can be acted on.
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { ContextMenuHost } from "../components/ContextMenuHost";
import { _resetAdminForTests } from "../lib/admin";
import type { AdminState } from "../lib/opsApi";
import { resetStore, startClusterStore } from "../lib/store";
import type { ClusterSnapshot, Container } from "../lib/types";
import { mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { Containers } from "./Containers";

const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const OTHER = import.meta.env.DEV ? "release" : "dev";
const MINE = import.meta.env.DEV ? "dev" : "release";

/** cell1 runs three containers: this build's, the other build's, and one with no instance label. */
function snapshot(demo = false): ClusterSnapshot {
  const s = structuredClone(baseSnapshot) as ClusterSnapshot;
  s.tsMs = Date.now();
  s.demo = demo;
  const base = s.nodes.flatMap((n) => n.containers)[0];
  const running = (id: string, name: string, labels: Record<string, string>): Container => ({
    ...base, id, name, state: "running", exitCode: 0, finishedAt: 0, image: "nvcr.io/nvidia/vllm:26.05-py3", labels,
  });
  for (const n of s.nodes) n.containers = [];
  s.nodes.find((n) => n.id === "cell1")!.containers = [
    running("aaaaaaaaaaa1", "sb-mine", { "sparkbox.instance": MINE, "sparkbox.deployment": "d1" }),
    running("bbbbbbbbbbb2", "sb-theirs", { "sparkbox.instance": OTHER, "sparkbox.deployment": "d2" }),
    running("ccccccccccc3", "by-hand", {}),
  ];
  return s;
}

async function show(opts: { params?: Record<string, string>; demo?: boolean; admin?: AdminState; handlers?: Record<string, (a: any) => unknown> } = {}) {
  const admin = opts.admin ?? UNLOCKED;
  resetStore();
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => snapshot(opts.demo), admin_state: () => admin }, opts.handlers ?? {}));
  _resetAdminForTests(admin);
  await startClusterStore();
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1200, 800]} isKnown={() => true}><Containers params={opts.params ?? {}} /><ContextMenuHost /></WindowManagerProvider>);
  });
  for (let i = 0; i < 20 && !r.container.querySelector('tr[data-key="cell1/bbbbbbbbbbb2"]'); i++) await act(async () => new Promise((res) => setTimeout(res, 50)));
  return r.container;
}

const row = (root: HTMLElement, id: string) => root.querySelector<HTMLElement>(`tr[data-key="cell1/${id}"]`)!;
const tool = (root: HTMLElement, name: string) => [...root.querySelectorAll<HTMLButtonElement>("[role=toolbar] button")].find((b) => b.getAttribute("aria-label")?.startsWith(name))!;

describe("a container the other SparkBox manages", () => {
  it("says so, and its actions are greyed with that reason; ours and unlabelled ones aren't", async () => {
    const root = await show();
    expect(row(root, "bbbbbbbbbbb2").textContent).toContain("managed by the other SparkBox");
    expect(row(root, "aaaaaaaaaaa1").textContent).not.toContain("managed by the other SparkBox");
    expect(row(root, "ccccccccccc3").textContent).not.toContain("managed by the other SparkBox");

    // Selected, it can be read (its command shows below), never acted on.
    await act(async () => {
      fireEvent.mouseDown(row(root, "bbbbbbbbbbb2").querySelector("td")!, { button: 0 });
    });
    expect(row(root, "bbbbbbbbbbb2").getAttribute("aria-selected")).toBe("true");
    for (const name of ["Stop", "Restart"]) {
      expect([tool(root, name).disabled, tool(root, name).title], name).toEqual([true, "managed by the other SparkBox"]);
    }
    expect(tool(root, "More").disabled, "More").toBe(true);

    // This build's own: Stop and Restart are there to use.
    for (const id of ["aaaaaaaaaaa1", "ccccccccccc3"]) {
      await act(async () => {
        fireEvent.mouseDown(row(root, id).querySelector("td")!, { button: 0 });
      });
      expect(row(root, id).getAttribute("aria-selected"), id).toBe("true");
      for (const name of ["Stop", "Restart"]) expect(tool(root, name).disabled, `${id}: ${name}`).toBe(false);
    }
  });
});

describe("a shell in a container", () => {
  it("isn't offered in the other SparkBox's container (a root shell), and is in this build's and unlabelled ones", async () => {
    const root = await show();
    const shell = async (id: string) => {
      await act(async () => fireEvent.contextMenu(row(root, id).querySelector("td")!));
      const menu = await screen.findByRole("menu");
      const item = within(menu).getAllByRole("menuitem").find((el) => (el.textContent ?? "").startsWith("Shell"))!;
      const state = [item.getAttribute("aria-disabled") === "true", item.querySelector(".ctx-reason")?.textContent ?? null];
      await act(async () => fireEvent.keyDown(menu, { key: "Escape" }));
      return state;
    };
    expect(await shell("bbbbbbbbbbb2")).toEqual([true, "managed by the other SparkBox"]);
    expect((await shell("aaaaaaaaaaa1"))[0]).toBe(false);
    expect((await shell("ccccccccccc3"))[0]).toBe(false);
  });
});

describe("Run test container", () => {
  it("starts sparkbox-test-busybox on the one node the Node filter picks, with Live data", async () => {
    const runs: unknown[] = [];
    const root = await show({ params: { node: "cell1" }, handlers: { container_test_run: (a: unknown) => { runs.push(a); return "abc"; } } });
    const run = tool(root, "Run test container");
    expect([run.disabled, run.title]).toEqual([false, "Start sparkbox-test-busybox (busybox, sleeps an hour) on cell1"]);
    await act(async () => {
      fireEvent.click(run);
      await new Promise((res) => setTimeout(res, 20));
    });
    expect(runs).toEqual([{ node: "cell1" }]);
  });

  it("is greyed for all nodes, with Demo data, and says look-only in look-only", async () => {
    let root = await show();
    expect([tool(root, "Run test container").disabled, tool(root, "Run test container").title]).toEqual([true, "Pick one node under Node"]);
    root = await show({ params: { node: "cell1" }, demo: true });
    expect([tool(root, "Run test container").disabled, tool(root, "Run test container").title]).toEqual([true, "Available with Live data"]);
    root = await show({ params: { node: "cell1" }, admin: { ...UNLOCKED, unlocked: false, lookOnly: true } });
    const held = tool(root, "Run test container");
    expect(held.disabled).toBe(true);
    expect(held.getAttribute("aria-label")).toBe("Run test container — look-only");
  });
});

it("shows saved members on a cold offline start without offering container actions", async () => {
  const { setDeploymentsForTest } = await import("../lib/deployments");
  const { _forgetLastKnownForTests } = await import("../lib/workloadCommands");
  _forgetLastKnownForTests();
  const snap = snapshot();
  snap.nodes.forEach((n) => { n.status = "offline"; n.containers = []; });
  const d = {
    id: "saved-load", label: "Saved model", served: "model", engine: "vllm" as const, image: "local/image", model: "/models/model",
    nodes: ["cell1", "cell2"], state: "unknown" as const, inferred: false, how: "SparkBox's saved load", startedMs: 1000,
    finishedMs: null, ranS: null, exitCode: null, postmortem: null, demo: false,
    members: ["cell1", "cell2"].map((node, rank) => ({ node, rank, id: "", container: `sparkbox-saved-r${rank}`, role: rank === 0 ? "head" as const : "worker" as const, state: "unknown", exitCode: 0, oom: false, startedMs: 0, finishedMs: 0 })),
  };
  setDeploymentsForTest([d]);
  try {
    const root = await show({ params: { view: "deployments" }, handlers: { get_snapshot: () => snap, cluster_deployments_list: () => [d] } });
    expect(root).toHaveTextContent("saved-load · 2 nodes · ranks 0–1 · Unknown");
    expect(root).toHaveTextContent("0/2 running · 2 unknown");
    expect(root).toHaveTextContent("sparkbox-saved-r0");
    expect(root).toHaveTextContent("sparkbox-saved-r1");
    const member = screen.getByText("sparkbox-saved-r0").closest("tr")!;
    fireEvent.click(member);
    expect(tool(root, "Restart")).toBeDisabled();
    expect(member).toHaveClass("row-stale");
  } finally { setDeploymentsForTest(null); }
});
