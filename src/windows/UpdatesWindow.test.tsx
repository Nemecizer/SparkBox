// Updates › Open DGX Dashboard (P1.18): the selected node's NVIDIA dashboard opens through an
// SSH tunnel to port 11000 on that node, and only when the node lists 11000 among its ports.
// A tunnel needs Admin (tunnel_open is gated), so the button follows the lock rule (P1.43).
// P1 r3: the window reads the background check's cache (updates_summary); the dry run shows the
// exact commands and the guard's refusal; needrestart's services each have a Restart; security
// only and the GPU-stack pin; Demo's new cells say they need Live data.
// P1 r4: Upgrade installs exactly the plan a dry run showed (sent back as opts.plan), the dry-run
// panel shows that plan and C1′, Check all nodes doesn't check again, a lost run says so, and
// Restart… says it's checking while the checks before it run.
// P1 r5: Stop before it installs, no download limit in the dialog, the guard's new words, Retry
// lands on its node, a dry run with nothing to install, and the snaps tooltip.
import { act, fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TasksChip } from "../components/TasksChip";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP } from "../lib/admin";
import { DRY_RUN_FIRST, type AdminState, type NodeUpdates, type PendingPackage, type PlannedPkg, type UpdateSummary } from "../lib/opsApi";
import * as session from "../lib/session";
import { resetStore, startClusterStore } from "../lib/store";
import { _resetTasksForTests, retryTask, type Task } from "../lib/tasks";
import { getToasts } from "../lib/toasts";
import type { ClusterSnapshot } from "../lib/types";
import { emitMock, extendMock, mockCalls, mockTauri, type MockHandlers } from "../test/tauri";
import { Desktop, WindowManagerProvider, type BodyComponent } from "../wm/WindowManager";
import { DGX_DASHBOARD_PORT, UpdatesWindow, dashboardBlocked, dockerLine, holdDrift, kernelLine, planLine, type UpgradeRun } from "./UpdatesWindow";

const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const LOCKED: AdminState = { ...UNLOCKED, unlocked: false };
const LOOK_ONLY: AdminState = { ...LOCKED, lookOnly: true };

/** Live data: the first node listens on the dashboard's port, the second doesn't. */
function snapshot(demo = false): ClusterSnapshot {
  const s = structuredClone(baseSnapshot) as ClusterSnapshot;
  s.tsMs = Date.now();
  s.demo = demo;
  const listener = (port: number, addr: string) => ({ port, addr, pid: null, process: null });
  s.nodes.forEach((n, i) => {
    n.listeners = i === 0 ? [listener(22, "0.0.0.0"), listener(DGX_DASHBOARD_PORT, "127.0.0.1")] : [listener(22, "0.0.0.0")];
  });
  return s;
}

async function backend(admin: AdminState, extra: MockHandlers, demo: boolean) {
  resetStore();
  _resetTasksForTests();
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, {
    get_snapshot: () => snapshot(demo),
    admin_state: () => admin,
    tunnel_open: () => "http://127.0.0.1:50123/?sbt=0123",
    updates_summary: ({ nodes }: { nodes: string[] | null }) => summaries(demo).filter((s) => !nodes || nodes.includes(s.node)),
    updates_progress: () => [],
  }, extra));
  _resetAdminForTests(admin);
  await startClusterStore();
}

async function show(select?: string, admin: AdminState = UNLOCKED, extra: MockHandlers = {}, demo = false) {
  await backend(admin, extra, demo);
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1400, 900]} isKnown={() => true}><UpdatesWindow params={{}} /></WindowManagerProvider>);
  });
  const root = r.container;
  for (let i = 0; i < 20 && !root.querySelector("[role=toolbar]"); i++) await act(async () => new Promise((res) => setTimeout(res, 50)));
  if (select) {
    const row = root.querySelector<HTMLElement>(`tr[data-key="${select}"]`)!;
    await act(async () => {
      fireEvent.mouseDown(row.querySelector("td")!, { button: 0 });
    });
  }
  return root;
}

const dashboard = (root: HTMLElement) =>
  [...root.querySelectorAll<HTMLButtonElement>("[role=toolbar] button")].find((b) => b.getAttribute("aria-label")?.startsWith("Open DGX Dashboard"))!;

describe("Updates › Open DGX Dashboard", () => {
  it("opens the selected node's port 11000 through a tunnel", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id);
    const button = dashboard(root);
    expect(button.disabled).toBe(false);
    await act(async () => {
      fireEvent.click(button);
    });
    for (let i = 0; i < 20 && !mockCalls.some((c) => c.cmd === "tunnel_open"); i++) await act(async () => new Promise((res) => setTimeout(res, 50)));
    expect(mockCalls.filter((c) => c.cmd === "tunnel_open").map((c) => c.args)).toEqual([{ node: first.id, port: 11000 }]);
  });

  it("is greyed, with the reason, for a node that doesn't listen on 11000", async () => {
    const second = snapshot().nodes[1];
    const root = await show(second.id);
    const button = dashboard(root);
    expect([button.disabled, button.title]).toEqual([true, `${second.name} isn't listening on port 11000`]);
  });

  it("in look-only it's off, with the look-only tip, and opens nothing", async () => {
    const root = await show(snapshot().nodes[0].id, LOOK_ONLY);
    const button = dashboard(root);
    expect([button.disabled, button.title, button.dataset.lock]).toEqual([true, LOOK_ONLY_TIP, "lookOnly"]);
    await act(async () => {
      fireEvent.click(button);
    });
    expect(mockCalls.some((c) => c.cmd === "tunnel_open" || c.cmd === "admin_unlock")).toBe(false);
  });

  it("with Admin locked it shows the lock, and a click asks with the in-app sheet first", async () => {
    const root = await show(snapshot().nodes[0].id, LOCKED);
    const button = dashboard(root);
    expect([button.disabled, button.dataset.lock, button.title.endsWith(NEEDS_ADMIN_TIP)]).toEqual([false, "locked", true]);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await act(async () => {
      fireEvent.click(button);
    });
    expect(asked).toHaveBeenCalledOnce();
    expect(asked.mock.calls[0][0]).toMatchObject({ title: "Unlock Admin mode?", body: expect.stringContaining("To open a web page on a node, SparkBox needs Admin mode.") });
    expect(mockCalls.some((c) => c.cmd === "tunnel_open" || c.cmd === "admin_unlock")).toBe(false);
    asked.mockRestore();
  });

  it("says why it can't open", () => {
    const [on, off] = snapshot().nodes;
    expect(dashboardBlocked(on, false)).toBeNull();
    expect(dashboardBlocked(off, false)).toBe(`${off.name} isn't listening on port 11000`);
    expect(dashboardBlocked(off, true)).toBeNull();
    expect(dashboardBlocked(undefined, false)).toBe("Select a node");
  });
});

const pkg = (name: string, security: boolean, extra: Partial<PendingPackage> = {}): PendingPackage =>
  ({ name, from: "1.0-1", to: "1.0-2", origin: security ? "noble-updates, noble-security" : "noble-updates", security, sensitive: false, held: false, ...extra });

function apt(node: string, pending: PendingPackage[], held: string[] = []): NodeUpdates {
  return { node, pending, rebootRequired: false, rebootPackages: [], unattended: false, held, driver: null, kernel: null, checkedMs: Date.now() - 60_000 };
}

/** The dry run of the packages asked for: each 1.0-1 → 1.0-2, its plan with it. */
const planOf = (names: string[]) => ({ node, packages }: { node: string; packages: string[] | null }) => {
  const take = packages ?? names;
  return {
    node, installs: [], upgrades: take.map((n) => `${n} 1.0-1 → 1.0-2`), removals: [], heldBack: [], text: take.map((n) => `Inst ${n} [1.0-1] (1.0-2 Ubuntu:24.04/noble-security [arm64])`).join("\n"),
    command: `env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -y --no-remove install --only-upgrade ${take.map((n) => `'${n}=1.0-2'`).join(" ")}`,
    dryRun: `apt-get -s -o DPkg::Lock::Timeout=300 -y install --only-upgrade ${take.map((n) => `'${n}'`).join(" ")}`, phased: [], sensitive: [], refused: null,
    plan: take.map((name): PlannedPkg => ({ name, from: "1.0-1", to: "1.0-2", new: false })), removes: [],
  };
};

/** What the background check has for each node (C10's shape). */
function summaries(demo = false): UpdateSummary[] {
  const ids = snapshot().nodes.map((n) => n.id);
  return ids.map((node, i) => ({
    node,
    checkedMs: Date.now() - 30_000,
    error: null,
    demo,
    apt: apt(node, i === 0
      ? [pkg("libbrotli1", true), pkg("nginx", true), pkg("linux-image-7.0.0-1020-nvidia", true, { sensitive: true }), pkg("google-chrome-stable", false)]
      : [pkg("libbrotli1", true), pkg("google-chrome-stable", false)], i === 3 ? ["docker-ce"] : []),
    dgx: demo ? null : { build: "7.2.3", ota: "7.6.0", otaDate: "2026-10-15", otaPending: i === 2 ? "7.7.0" : null },
    firmware: demo ? null : { state: (["needsSudo", "none", "available", "unknown"] as const)[i], count: i === 2 ? 2 : 0, devices: [], detail: i === 3 ? "fwupdmgr: command not found" : null },
    ollama: demo || i % 2 ? null : { installed: "0.21.0", latest: i === 0 ? "0.22.1" : null, newer: i === 0 },
    snaps: demo ? null : i === 1 ? [{ name: "firefox", current: "140.0", available: "141.0" }] : [],
    dockerLiveRestore: demo ? null : false,
    stamps: { listsMs: null, dpkgMs: null, rebootRequiredMs: null },
    securitySinceMs: null,
    refresh: demo || i !== 1 ? null : { atMs: Date.now() - 120_000, ok: true, warnings: ["W: https://dl.google.com/linux/chrome/deb/dists/stable/InRelease: Key is stored in legacy trusted.gpg keyring"], errors: [] },
  }));
}

const settle = async (_root: HTMLElement, until: () => boolean) => {
  for (let i = 0; i < 40 && !until(); i++) await act(async () => new Promise((res) => setTimeout(res, 25)));
};
const cellsOf = (root: HTMLElement, node: string) => [...root.querySelectorAll<HTMLElement>(`tr[data-key="${node}"] td`)].map((td) => td.textContent ?? "");
const tool = (root: HTMLElement, label: string) =>
  [...root.querySelectorAll<HTMLButtonElement>("[role=toolbar] button")].find((b) => b.getAttribute("aria-label")?.startsWith(label))!;

const rowUpgrade = (root: HTMLElement, node: string) => root.querySelector<HTMLButtonElement>(`tr[data-key="${node}"] .upd-row-action button`)!;
const rowRestart = (root: HTMLElement, node: string) => root.querySelector<HTMLButtonElement>(`tr[data-key="${node}"] .upd-restart .tool`)!;

describe("Restart and reconnect from each node row", () => {
  const task = (node: string): Task & { demo: boolean } => ({
    id: "t-restart", kind: "power", title: `Restart ${node}`, nodes: [node], source: "ui", demo: false,
    cancellable: true, state: "running", startedMs: Date.now(), finishedMs: null,
    progress: null, detail: null, rateBps: null, etaS: null, steps: [], log: [], summary: null,
    error: null, cancelRequested: false, retry: { command: "node_power", args: { node, action: "reboot" } }, opId: "t-restart",
  });
  it("restarts the clicked node through the existing confirmation, even without a pending-update flag", async () => {
    const [first, second] = snapshot().nodes;
    const root = await show(second.id, UNLOCKED, { node_power: () => "t-restart", vpn_get: () => ({ status: { state: "direct" } }) });
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await act(async () => fireEvent.click(rowRestart(root, first.id)));
    await settle(root, () => mockCalls.some((c) => c.cmd === "node_power"));
    expect(asked.mock.calls[0][0].title).toBe(`Restart ${first.name}?`);
    expect(render(<>{asked.mock.calls[0][0].body}</>).container.textContent).toContain("SparkBox reconnects when it's back");
    expect(mockCalls.filter((c) => c.cmd === "node_power").map((c) => c.args)).toEqual([{ node: first.id, action: "reboot", delayMin: 0 }]);
    asked.mockRestore();
  });
  it("does not send a restart after cancellation", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await act(async () => fireEvent.click(rowRestart(root, first.id)));
    expect(asked).toHaveBeenCalled();
    expect(mockCalls.some((c) => c.cmd === "node_power")).toBe(false);
    asked.mockRestore();
  });
  it("keeps Look only disabled and offers the normal unlock when Admin is locked", async () => {
    const first = snapshot().nodes[0];
    const readOnly = await show(first.id, LOOK_ONLY);
    expect(rowRestart(readOnly, first.id).disabled).toBe(true);
    expect(rowRestart(readOnly, first.id).dataset.lock).toBe("lookOnly");
    const locked = await show(first.id, LOCKED);
    expect(rowRestart(locked, first.id).disabled).toBe(false);
    expect(rowRestart(locked, first.id).dataset.lock).toBe("locked");
    expect(mockCalls.some((c) => c.cmd === "node_power")).toBe(false);
  });
  it("blocks offline, stale and upgrading nodes", async () => {
    const s = snapshot(); s.nodes[0].status = "offline"; s.nodes[1].stale = true;
    const root = await show(undefined, UNLOCKED, { get_snapshot: () => s, updates_progress: () => [{ node: s.nodes[2].id, phase: "lost", simulated: false }] });
    for (const n of s.nodes.slice(0, 3)) expect(rowRestart(root, n.id).disabled).toBe(true);
    expect(rowRestart(root, s.nodes[2].id).title).toContain("package upgrade");
    expect(rowRestart(root, s.nodes[3].id).disabled).toBe(false);
  });
  it("follows reconnection, prevents duplicate restarts, and refreshes package status after verified completion", async () => {
    const first = snapshot().nodes[0];
    const t = task(first.id);
    const root = await show(first.id, UNLOCKED, { tasks_list: () => [t] });
    expect(rowRestart(root, first.id).disabled).toBe(true);
    const s = snapshot(); s.nodes[0].status = "offline"; s.nodes[0].powerState = "rebooting";
    await act(async () => emitMock("cluster://snapshot", s));
    expect(rowRestart(root, first.id).textContent).toContain("Reconnecting");
    expect(root.textContent).toContain("View progress");
    expect(mockCalls.some((c) => c.cmd === "node_power")).toBe(false);
    const reads = () => mockCalls.filter((c) => c.cmd === "updates_summary" && c.args?.fresh === true);
    expect(reads()).toHaveLength(0);
    await act(async () => {
      await emitMock("cluster://snapshot", snapshot());
      await emitMock("tasks://update", { ...t, state: "ok", finishedMs: Date.now(), summary: "Back on a new boot" });
    });
    expect(reads().map((c) => c.args)).toEqual([{ nodes: [first.id], fresh: true }]);
    expect(root.textContent).toContain("Restarted · view result");
    expect(rowRestart(root, first.id).disabled).toBe(false);
  });
  it("keeps failures visible and never calls a timeout a reattachment", async () => {
    const first = snapshot().nodes[0];
    const t = task(first.id);
    const root = await show(first.id, UNLOCKED, { tasks_list: () => [t] });
    await act(async () => emitMock("tasks://update", { ...t, state: "failed", error: "No return after 5 minutes", finishedMs: Date.now() }));
    expect(root.textContent).toContain("View restart result");
    expect(root.textContent).not.toContain("Restarted · view result");
    expect(mockCalls.some((c) => c.cmd === "updates_summary" && c.args?.fresh === true)).toBe(false);
    expect(mockCalls.some((c) => c.cmd === "node_power")).toBe(false);
  });
  it("does not show a Demo restart as a Live restart", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, UNLOCKED, { tasks_list: () => [{ ...task(first.id), demo: true }] });
    expect(rowRestart(root, first.id).disabled).toBe(false);
    expect(root.textContent).not.toContain("View progress");
  });
});

describe("Upgrade beside each node", () => {
  it.each([undefined, { checkedMs: 1, remaining: [] }, { checkedMs: 1, remaining: [pkg("linux-image-test", true, { sensitive: true })] }])("shows only the final check's actual verdict: %j", async (verification) => {
    const first = snapshot().nodes[0];
    const run: UpgradeRun = { node: first.id, taskId: "t-check", title: "Upgrade", unit: "u", logPath: "l", packages: ["libbrotli1"], phase: "done", done: 1, total: 1, current: null, log: [], cancellable: false, startedMs: 1, exitCode: 0, simulated: false, verification };
    const root = await show(first.id, UNLOCKED, { updates_progress: () => [run] });
    const panel = root.querySelector(`[aria-label="Upgrade on ${first.name}"]`)!;
    if (!verification) expect(panel.textContent).toContain("Remaining updates are unknown");
    else if (verification.remaining.length) expect(panel.textContent).toContain("1 update remains. linux-image-test (GPU stack)");
    else expect(panel.textContent).toContain("no package updates remain");
    if (!verification || verification.remaining.length) expect(panel.textContent).not.toContain("no package updates remain");
  });
  it("refreshes the clicked node, previews eligible packages, then sends exactly that plan", async () => {
    const [first, second] = snapshot().nodes;
    const root = await show(second.id, UNLOCKED, {
      updates_simulate: planOf([]), updates_upgrade: () => "t-row",
      updates_summary: ({ nodes }: { nodes: string[] }) => summaries().filter((s) => nodes.includes(s.node)).map((s) => ({ ...s, apt: { ...s.apt!, pending: [...s.apt!.pending, pkg("held-app", false, { held: true }), pkg("libxnvctrl0", true, { sensitive: true })] } })),
    });
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await act(async () => fireEvent.click(rowUpgrade(root, first.id)));
    await settle(root, () => mockCalls.some((c) => c.cmd === "updates_upgrade"));
    expect(mockCalls.filter((c) => c.cmd === "updates_summary").map((c) => c.args)).toContainEqual({ nodes: [first.id], fresh: true });
    const names = ["libbrotli1", "nginx", "google-chrome-stable"];
    expect(mockCalls.find((c) => c.cmd === "updates_simulate")?.args).toEqual({ node: first.id, packages: names });
    expect(mockCalls.find((c) => c.cmd === "updates_upgrade")?.args).toEqual({ node: first.id, packages: names, opts: { plan: names.map((name) => ({ name, to: "1.0-2" })) } });
    expect(asked.mock.calls[0][0].title).toBe(`Upgrade 3 packages on ${first.name}?`);
    const body = render(<>{asked.mock.calls[0][0].body}</>).container.textContent;
    expect(body).toContain("held-app (held)");
    expect(body).toContain("linux-image-7.0.0-1020-nvidia (GPU stack)");
    expect(body).toContain("libxnvctrl0 (GPU stack)");
    expect(root.querySelector('tr[data-key="libxnvctrl0"]')?.textContent).toContain("NVIDIA");
    asked.mockRestore();
  });

  it("does not install when the preview is refused or the owner cancels", async () => {
    const first = snapshot().nodes[0];
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const info = vi.spyOn(session, "alertInfo").mockResolvedValue(undefined);
    const root = await show(first.id, UNLOCKED, { updates_simulate: planOf([]) });
    await act(async () => fireEvent.click(rowUpgrade(root, first.id)));
    await settle(root, () => asked.mock.calls.length > 0);
    expect(mockCalls.some((c) => c.cmd === "updates_upgrade")).toBe(false);
    extendMock({ updates_simulate: (args: { node: string; packages: string[] | null }) => ({ ...planOf([])(args), refused: "The plan would change the GPU stack" }) });
    await act(async () => fireEvent.click(rowUpgrade(root, first.id)));
    await settle(root, () => info.mock.calls.length > 0);
    expect(info.mock.calls[0][0].body).toContain("Upgrade refused");
    expect(mockCalls.some((c) => c.cmd === "updates_upgrade")).toBe(false);
    asked.mockRestore(); info.mockRestore();
  });

  it("leaves look-only and protected-only nodes disabled", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, LOOK_ONLY);
    expect(rowUpgrade(root, first.id).disabled).toBe(true);
    expect(rowUpgrade(root, first.id).dataset.lock).toBe("lookOnly");
    await act(async () => fireEvent.click(rowUpgrade(root, first.id)));
    expect(mockCalls.some((c) => c.cmd === "updates_upgrade" || c.cmd === "updates_simulate")).toBe(false);
  });

  it("shows protected-only, empty, offline and unreadable package states without offering an upgrade", async () => {
    const ns = snapshot().nodes;
    const snap = snapshot(); snap.nodes[2].status = "offline";
    const data = summaries();
    data[0].apt!.pending = [pkg("linux-image-test", true, { sensitive: true })];
    data[1].apt!.pending = [];
    data[3].apt = null; data[3].error = "Cannot read apt";
    const root = await show(ns[0].id, UNLOCKED, { get_snapshot: () => snap, updates_summary: () => data });
    for (const n of ns) expect(rowUpgrade(root, n.id).disabled).toBe(true);
    expect(rowUpgrade(root, ns[0].id).title).toContain("Only held or GPU-stack");
    expect(rowUpgrade(root, ns[1].id).title).toContain("No package updates remain");
    expect(rowUpgrade(root, ns[2].id).title).toContain("reconnect");
    expect(rowUpgrade(root, ns[3].id).title).toContain("Cannot read apt");
  });
});

describe("Updates from the background check (P1 r3)", () => {
  it("loads every node from the summary, not one apt status per node, and shows DGX OS, firmware, Ollama and snaps", async () => {
    const [n1, n2, n3, n4] = snapshot().nodes;
    const root = await show(n2.id);
    await settle(root, () => cellsOf(root, n1.id).some((c) => c.includes("7.6.0")));
    const asked = mockCalls.filter((c) => c.cmd === "updates_summary");
    expect(asked[0].args).toEqual({ nodes: snapshot().nodes.map((n) => n.id), fresh: null });
    expect(mockCalls.some((c) => c.cmd === "updates_status")).toBe(false);
    expect(cellsOf(root, n1.id)).toEqual(expect.arrayContaining(["7.6.0", "needs sudo", "0.21.0 · latest 0.22.1", "0"]));
    expect(cellsOf(root, n2.id)).toEqual(expect.arrayContaining(["none", "1"]));
    expect(cellsOf(root, n3.id)).toEqual(expect.arrayContaining(["7.6.0 · update pending", "2", "0.21.0 · couldn't check"]));
    expect(cellsOf(root, n4.id).filter((c) => c === "—").length).toBeGreaterThanOrEqual(2);
    // The node detail has the last refresh's W: lines.
    expect(root.querySelector("[aria-label='Last check for updates']")?.textContent).toContain("W: https://dl.google.com/linux/chrome/deb/dists/stable/InRelease: Key is stored in legacy trusted.gpg keyring");
    // Hold drift: gx10-0000 holds docker-ce, the others nothing.
    expect(root.textContent).toContain(`Hold drift: ${n4.name} holds different packages from the other nodes.`);
    // An update event re-reads the nodes it names.
    const before = mockCalls.filter((c) => c.cmd === "updates_summary").length;
    await act(async () => emitMock("updates://changed", { nodes: [n3.id] }));
    await settle(root, () => mockCalls.filter((c) => c.cmd === "updates_summary").length > before);
    expect(mockCalls.filter((c) => c.cmd === "updates_summary").at(-1)!.args).toEqual({ nodes: [n3.id], fresh: null });
  });

  it("in Demo the new cells say they need Live data, and the pin is off", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, UNLOCKED, {}, true);
    await settle(root, () => cellsOf(root, first.id).some((c) => c === "Available with Live data"));
    for (const n of snapshot().nodes) expect(cellsOf(root, n.id).filter((c) => c === "Available with Live data")).toHaveLength(4);
    const pin = tool(root, "Pin the GPU stack…");
    expect([pin.disabled, pin.title]).toEqual([true, "Available with Live data"]);
  });

  it("a refused dry run shows both commands and turns Upgrade off with the guard's reason", async () => {
    const first = snapshot().nodes[0];
    const why = "SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia";
    const root = await show(first.id, UNLOCKED, {
      updates_simulate: ({ node }: { node: string }) => ({
        node, installs: [], upgrades: ["libbrotli1 1.0-1 → 1.0-2", "linux-image-7.0.0-1020-nvidia 1.0-1 → 1.0-2"], removals: [], heldBack: [], text: "Inst libbrotli1 …",
        command: "env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -y dist-upgrade",
        dryRun: "apt-get -s -o DPkg::Lock::Timeout=300 -y dist-upgrade", phased: ["systemd"], sensitive: ["linux-image-7.0.0-1020-nvidia"], refused: why,
      }),
    });
    await settle(root, () => !!tool(root, "Simulate") && !tool(root, "Simulate").disabled);
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => !!root.querySelector("[aria-label='Simulated upgrade']"));
    const panel = root.querySelector("[aria-label='Simulated upgrade']")!;
    expect(panel.textContent).toContain(`Upgrade refused: ${why}.`);
    expect(panel.textContent).toContain("Dry run that showed it");
    expect(panel.textContent).toContain("apt-get -s -o DPkg::Lock::Timeout=300 -y dist-upgrade");
    expect(panel.textContent).toContain("Deferred by phasing");
    const upgrade = tool(root, "Upgrade node");
    expect([upgrade.disabled, upgrade.title]).toEqual([true, why]);
  });

  it("lists needrestart's services after an upgrade, each with a Restart through the service action's dialog", async () => {
    const first = snapshot().nodes[0];
    const run: UpgradeRun = {
      node: first.id, taskId: "t1", title: `Upgrade libbrotli1 on ${first.name}`, unit: "sparkbox-upgrade-t1", logPath: "/var/log/sparkbox/upgrade-t1.log", packages: ["libbrotli1"],
      phase: "done", done: 1, total: 1, current: null, log: ["Setting up libbrotli1 (1.0-2) ...", "SPARKBOX-AFTER", "SPARKBOX-AFTER-END", "SPARKBOX-EXIT 0"], cancellable: false, startedMs: Date.now(), exitCode: 0, simulated: false,
      dpkgBefore: "/var/log/sparkbox/dpkg-before-t1.txt", evidence: "/Users/x/Library/Logs/SparkBox/evidence/2026-10-30-updates/cell1-dpkg-before-t1.txt",
      needrestart: { kernelStatus: 3, kernelCurrent: "7.0.0-1019-nvidia", kernelExpected: "7.0.0-1020-nvidia", services: ["nginx.service", "ollama.service"] },
    };
    const root = await show(first.id, UNLOCKED, {
      updates_progress: () => [run],
      service_impact: () => ({ tier: "medium", title: `Restart nginx.service on ${first.name}?`, lines: ["nginx stops for a moment."], requireTyping: null, blocked: null, confirmLabel: "Restart" }),
      service_action: () => null,
    });
    await settle(root, () => !!root.querySelector("[aria-label='Services still using old libraries']"));
    const panel = root.querySelector<HTMLElement>("[aria-label='Services still using old libraries']")!;
    expect(panel.textContent).toContain("A newer kernel is installed (7.0.0-1020-nvidia); it runs after the next restart.");
    const restarts = [...panel.querySelectorAll<HTMLButtonElement>("button")];
    expect(restarts.map((b) => b.getAttribute("aria-label"))).toEqual(["Restart nginx.service", "Restart ollama.service"]);
    expect(root.querySelector("[aria-label='apt output']")!.textContent).not.toContain("SPARKBOX-");
    expect(root.textContent).toContain("/var/log/sparkbox/dpkg-before-t1.txt");
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await act(async () => fireEvent.click(restarts[0]));
    await settle(root, () => mockCalls.some((c) => c.cmd === "service_action"));
    expect(asked.mock.calls[0][0]).toMatchObject({ title: `Restart nginx.service on ${first.name}?` });
    expect(mockCalls.filter((c) => c.cmd === "service_action").map((c) => c.args)).toEqual([{ node: first.id, unit: "nginx.service", action: "restart" }]);
    asked.mockRestore();
  });

  it("Upgrade security only dry-runs the fixes it takes, asks with that plan, and sends it", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, UNLOCKED, { updates_upgrade: () => "t-up", updates_simulate: planOf(["libbrotli1", "nginx"]) });
    await settle(root, () => !!tool(root, "Upgrade security only") && !tool(root, "Upgrade security only").disabled);
    const security = tool(root, "Upgrade security only");
    expect(security.title).toBe("Dry-runs, then installs, the security fixes that aren't GPU-stack or held: libbrotli1, nginx");
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await act(async () => fireEvent.click(security));
    await settle(root, () => mockCalls.some((c) => c.cmd === "updates_upgrade"));
    expect(mockCalls.filter((c) => c.cmd === "updates_simulate").map((c) => c.args)).toEqual([{ node: first.id, packages: ["libbrotli1", "nginx"] }]);
    const confirm = asked.mock.calls[0][0];
    expect(confirm.title).toBe(`Install 2 security fixes on ${first.name}?`);
    const body = render(<>{confirm.body}</>).container.textContent ?? "";
    expect(body).toContain("Saves the node's package list first (dpkg-query -W) to /var/log/sparkbox and the evidence folder");
    expect(body).not.toContain("Limit the download");
    expect(body).not.toContain("500 KB/s");
    expect(body).toContain("libbrotli1: 1.0-1 → 1.0-2");
    expect(mockCalls.filter((c) => c.cmd === "updates_upgrade").map((c) => c.args)).toEqual([
      { node: first.id, packages: null, opts: { securityOnly: true, plan: [{ name: "libbrotli1", to: "1.0-2" }, { name: "nginx", to: "1.0-2" }] } },
    ]);
    asked.mockRestore();
  });

  it("the pin's confirm lists what it holds, read first, then pins", async () => {
    const first = snapshot().nodes[0];
    const stack = ["libnvidia-compute-580", "linux-image-7.0.0-1019-nvidia", "nvidia-driver-580-open"];
    const root = await show(first.id, UNLOCKED, {
      updates_hold_preset: ({ node, hold, preview }: { node: string; hold: boolean; preview: boolean }) => ({ node, hold, packages: stack, taskId: preview ? null : "t-pin" }),
    });
    await settle(root, () => !!tool(root, "Pin the GPU stack…") && !tool(root, "Pin the GPU stack…").disabled);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await act(async () => fireEvent.click(tool(root, "Pin the GPU stack…")));
    await settle(root, () => mockCalls.filter((c) => c.cmd === "updates_hold_preset").length === 2);
    expect(mockCalls.filter((c) => c.cmd === "updates_hold_preset").map((c) => c.args)).toEqual([{ node: first.id, hold: true, preview: true }, { node: first.id, hold: true, preview: false }]);
    expect(asked.mock.calls[0][0].title).toBe(`Pin the GPU stack on ${first.name}?`);
    const listed = render(<>{asked.mock.calls[0][0].body}</>).container.textContent ?? "";
    for (const p of stack) expect(listed).toContain(p);
    asked.mockRestore();
  });

  it("in look-only Upgrade, security only, Hold and the pin are off and send nothing", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, LOOK_ONLY);
    await settle(root, () => cellsOf(root, first.id).some((c) => c.includes("7.6.0")));
    for (const label of ["Upgrade node", "Upgrade security only", "Pin the GPU stack…", "Release the GPU-stack pin…", "Check all nodes"]) {
      const b = tool(root, label);
      expect([label, b.disabled, b.dataset.lock]).toEqual([label, true, "lookOnly"]);
    }
    expect(mockCalls.some((c) => ["updates_upgrade", "updates_hold_preset", "updates_hold", "updates_refresh"].includes(c.cmd))).toBe(false);
  });

  it("in look-only Cancel on a running upgrade is off and sends nothing", async () => {
    const first = snapshot().nodes[0];
    // A run picked up again: SparkBox follows it in look-only too.
    const running: UpgradeRun = {
      node: first.id, taskId: "t-lo", title: `Upgrade ${first.name} (picked up again)`, unit: "sparkbox-upgrade-t-lo", logPath: "/var/log/sparkbox/upgrade-t-lo.log", packages: ["libbrotli1"],
      phase: "downloading", done: 0, total: 1, current: null, log: ["Get:1 http://ports.ubuntu.com libbrotli1"], cancellable: true, startedMs: Date.now(), exitCode: null, simulated: false,
    };
    const root = await show(first.id, LOOK_ONLY, { updates_progress: () => [running], updates_cancel: () => "Stopping apt before it installs anything" });
    await settle(root, () => !!root.querySelector(`[aria-label='Upgrade on ${first.name}']`));
    const panel = root.querySelector<HTMLElement>(`[aria-label='Upgrade on ${first.name}']`)!;
    const stop = [...panel.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === "Stop before it installs")!;
    expect([stop.disabled, stop.title]).toEqual([true, LOOK_ONLY_TIP]);
    await act(async () => fireEvent.click(stop));
    expect(mockCalls.some((c) => c.cmd === "updates_cancel")).toBe(false);
  });

  const dryRunOf = (refused: string | null, upgrades = ["libbrotli1 1.0-1 → 1.0-2", "linux-image-7.0.0-1020-nvidia 1.0-1 → 1.0-2"]) => ({ node }: { node: string }) => ({
    node, installs: [], upgrades, removals: [], heldBack: [], text: "Inst …",
    command: "env … -y --no-remove install --only-upgrade …", dryRun: "apt-get -s … -y dist-upgrade", phased: [], sensitive: refused ? ["linux-image-7.0.0-1020-nvidia"] : [], refused,
    plan: upgrades.map((u): PlannedPkg => { const [name, from, , to] = u.split(" "); return { name, from, to, new: false }; }), removes: [],
  });
  const pick = async (root: HTMLElement, name: string) => {
    const row = [...root.querySelectorAll<HTMLElement>(`tr[data-key='${name}']`)].at(-1)!;
    const box = row.querySelector<HTMLInputElement>("input[type=checkbox]");
    await act(async () => { if (box) fireEvent.click(box); else fireEvent.mouseDown(row.querySelector("td")!, { button: 0, metaKey: true }); });
  };

  it("a refused dry run of everything speaks for everything only: a safe pick can be upgraded", async () => {
    const first = snapshot().nodes[0];
    const why = "SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia";
    const root = await show(first.id, UNLOCKED, { updates_simulate: dryRunOf(why) });
    await settle(root, () => !!tool(root, "Simulate") && !tool(root, "Simulate").disabled);
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => !!root.querySelector("[aria-label='Simulated upgrade']"));
    expect([tool(root, "Upgrade node").disabled, tool(root, "Upgrade node").title]).toEqual([true, why]);
    await pick(root, "libbrotli1");
    await settle(root, () => !!tool(root, "Upgrade 1"));
    // Live data: Upgrade installs exactly a dry run on screen, so the pick needs its own.
    expect([tool(root, "Upgrade 1").disabled, tool(root, "Upgrade 1").title]).toEqual([true, DRY_RUN_FIRST]);
    expect(root.querySelector("[aria-label='Simulated upgrade']")!.textContent).toContain("This dry run was for everything; Simulate again for what's picked now.");
    extendMock({ updates_simulate: dryRunOf(null, ["libbrotli1 1.0-1 → 1.0-2"]) });
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => !tool(root, "Upgrade 1").disabled);
    expect([tool(root, "Upgrade 1").disabled, tool(root, "Upgrade 1").title]).toEqual([false, "Upgrade 1"]);
    expect(mockCalls.filter((c) => c.cmd === "updates_simulate").at(-1)!.args).toEqual({ node: first.id, packages: ["libbrotli1"] });
  });

  it("an upgrade started after a Simulate shows its own panel and Cancel while it runs", async () => {
    const first = snapshot().nodes[0];
    let started = false;
    const running: UpgradeRun = {
      node: first.id, taskId: "t-up", title: `Upgrade ${first.name}`, unit: "sparkbox-upgrade-t-up", logPath: "/var/log/sparkbox/upgrade-t-up.log", packages: ["libbrotli1"],
      phase: "downloading", done: 0, total: 1, current: null, log: ["Get:1 http://ports.ubuntu.com libbrotli1"], cancellable: true, startedMs: Date.now(), exitCode: null, simulated: false,
    };
    const root = await show(first.id, UNLOCKED, {
      updates_simulate: dryRunOf(null, ["libbrotli1 1.0-1 → 1.0-2"]),
      updates_upgrade: () => { started = true; return "t-up"; },
      updates_progress: () => (started ? [running] : []),
    });
    await settle(root, () => !!tool(root, "Simulate") && !tool(root, "Simulate").disabled);
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => !!root.querySelector("[aria-label='Simulated upgrade']"));
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await act(async () => fireEvent.click(tool(root, "Upgrade node")));
    await settle(root, () => !!root.querySelector(`[aria-label='Upgrade on ${first.name}']`));
    const panel = root.querySelector<HTMLElement>(`[aria-label='Upgrade on ${first.name}']`)!;
    expect(panel.textContent).toContain("Downloading packages");
    const stop = [...panel.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Stop before it installs")!;
    expect(stop.title).toBe("Stops apt now, while it's still downloading. Nothing has been installed yet.");
    expect(root.querySelector("[aria-label='Simulated upgrade']")).toBeNull();
    // The confirm says what's true: exactly the plan, the window while apt runs, needrestart
    // restarts nothing, everything needs the pin. Upgrade sends the plan it showed.
    const body = render(<>{asked.mock.calls[0][0].body}</>).container.textContent ?? "";
    expect(body).toContain("Installs exactly what its dry run showed");
    expect(body).toContain("this window follows its log, and Stop before it installs works until dpkg starts installing.");
    expect(body).toContain("libbrotli1: 1.0-1 → 1.0-2");
    expect(body).toContain("Its alerts are planned while apt runs: 30 minutes from the start, and again each time SparkBox sees it still running; if SparkBox loses the node, they page 30 minutes after it last saw apt running.");
    expect(mockCalls.filter((c) => c.cmd === "updates_upgrade").map((c) => c.args)).toEqual([{ node: first.id, packages: null, opts: { plan: [{ name: "libbrotli1", to: "1.0-2" }] } }]);
    expect(body).toContain("needrestart restarts nothing, but a package's own scripts may restart its service");
    expect(body).toContain("Runs only with the GPU stack pinned (Pin the GPU stack…)");
    expect(body).not.toContain("nothing pages");
    asked.mockRestore();
  });

  it("a running upgrade keeps its panel when Simulate is pressed", async () => {
    const first = snapshot().nodes[0];
    const running: UpgradeRun = {
      node: first.id, taskId: "t-r", title: `Upgrade ${first.name}`, unit: "sparkbox-upgrade-t-r", logPath: "/var/log/sparkbox/upgrade-t-r.log", packages: ["libbrotli1"],
      phase: "unpacking", done: 1, total: 2, current: "libbrotli1", log: ["Unpacking libbrotli1 (1.0-2) over (1.0-1) ..."], cancellable: false, startedMs: Date.now(), exitCode: null, simulated: false,
    };
    const root = await show(first.id, UNLOCKED, { updates_simulate: dryRunOf(null, ["nginx 1.0-1 → 1.0-2"]), updates_progress: () => [running] });
    await settle(root, () => !!root.querySelector(`[aria-label='Upgrade on ${first.name}']`) && !tool(root, "Simulate").disabled);
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => mockCalls.some((c) => c.cmd === "updates_simulate"));
    for (let i = 0; i < 4; i++) await act(async () => new Promise((res) => setTimeout(res, 25)));
    expect(root.querySelector(`[aria-label='Upgrade on ${first.name}'] [role=status]`)!.textContent).toBe("Unpacking 1 of 2 · libbrotli1");
  });

  it("an upgrade whose dry run is under way says so", async () => {
    const first = snapshot().nodes[0];
    const checking: UpgradeRun = {
      node: first.id, taskId: "t-c", title: `Upgrade ${first.name}`, unit: "", logPath: "", packages: [],
      phase: "checking", done: 0, total: 0, current: null, log: [], cancellable: true, startedMs: Date.now(), exitCode: null, simulated: false,
    };
    const root = await show(first.id, UNLOCKED, { updates_progress: () => [checking] });
    await settle(root, () => !!root.querySelector(`[aria-label='Upgrade on ${first.name}']`));
    expect(root.querySelector(`[aria-label='Upgrade on ${first.name}'] [role=status]`)!.textContent).toBe("Checking what will change");
  });

  it("the pin's confirm says fwupd isn't held and SparkBox doesn't upgrade it yet", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, UNLOCKED, {
      updates_hold_preset: ({ node, hold }: { node: string; hold: boolean }) => ({ node, hold, packages: ["docker-ce"], taskId: null }),
    });
    await settle(root, () => !!tool(root, "Pin the GPU stack…") && !tool(root, "Pin the GPU stack…").disabled);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await act(async () => fireEvent.click(tool(root, "Pin the GPU stack…")));
    await settle(root, () => asked.mock.calls.length > 0);
    const body = render(<>{asked.mock.calls[0][0].body}</>).container.textContent ?? "";
    expect(body).toContain("fwupd isn't held, and SparkBox doesn't upgrade it either yet.");
    expect(body).not.toContain("its own fixes still arrive");
    asked.mockRestore();
  });

  it("a GPU-stack pick is refused in the guard's words, before any dry run", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id);
    await settle(root, () => !!root.querySelector("tr[data-key='linux-image-7.0.0-1020-nvidia']"));
    await pick(root, "linux-image-7.0.0-1020-nvidia");
    await settle(root, () => !!tool(root, "Upgrade 1"));
    expect([tool(root, "Upgrade 1").disabled, tool(root, "Upgrade 1").title]).toEqual([true, "SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia"]);
    expect(mockCalls.some((c) => c.cmd === "updates_simulate")).toBe(false);
  });

  it("a dry run with nothing to install says so, shows no upgrade command, and turns Upgrade off", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, UNLOCKED, {
      updates_simulate: ({ node }: { node: string }) => ({ ...planOf([])({ node, packages: [] }), command: "", dryRun: "apt-get -s -o DPkg::Lock::Timeout=300 -y dist-upgrade", phased: ["google-chrome-stable"] }),
    });
    await settle(root, () => !!tool(root, "Simulate") && !tool(root, "Simulate").disabled);
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => !!root.querySelector("[aria-label='Simulated upgrade']"));
    const panel = root.querySelector<HTMLElement>("[aria-label='Simulated upgrade']")!;
    expect(root.querySelector("[aria-label='The plan Upgrade installs']")!.textContent).toContain("Nothing to install.");
    expect(panel.textContent).not.toContain("The upgrade runs, as root");
    expect(panel.textContent).toContain("Dry run that showed it");
    const up = tool(root, "Upgrade node");
    expect([up.disabled, up.title]).toEqual([true, "Nothing to install."]);
  });

  it("security only with an empty plan doesn't ask to install 0 fixes, and reads the node again", async () => {
    const first = snapshot().nodes[0];
    const empty = ({ node }: { node: string }) => ({ ...planOf([])({ node, packages: [] }), command: "" });
    const root = await show(first.id, UNLOCKED, { updates_upgrade: () => "t-up", updates_simulate: empty });
    await settle(root, () => !!tool(root, "Upgrade security only") && !tool(root, "Upgrade security only").disabled);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const told = vi.spyOn(session, "alertInfo").mockResolvedValue(undefined as never);
    const reads = mockCalls.filter((c) => c.cmd === "updates_summary").length;
    await act(async () => fireEvent.click(tool(root, "Upgrade security only")));
    await settle(root, () => told.mock.calls.length > 0 && mockCalls.filter((c) => c.cmd === "updates_summary").length > reads);
    expect(asked).not.toHaveBeenCalled();
    expect(told.mock.calls[0][0]).toMatchObject({ title: `Can't install the security fixes on ${first.name}`, body: "Nothing to install." });
    expect(mockCalls.filter((c) => c.cmd === "updates_summary").at(-1)!.args).toEqual({ nodes: [first.id], fresh: true });
    expect(mockCalls.some((c) => c.cmd === "updates_upgrade")).toBe(false);
    asked.mockRestore();
    told.mockRestore();
  });

  it("a Stop that didn't work says why while the upgrade runs, and can be pressed again", async () => {
    const first = snapshot().nodes[0];
    const said = "Couldn't stop it: pkill: cannot allocate memory";
    const base: UpgradeRun = {
      node: first.id, taskId: "t-up", title: `Upgrade ${first.name}`, unit: "sparkbox-upgrade-t-up", logPath: "/var/log/sparkbox/upgrade-t-up.log", packages: ["libbrotli1"],
      phase: "downloading", done: 0, total: 1, current: null, log: ["Get:1 http://ports.ubuntu.com libbrotli1"], cancellable: true, stopFailed: said, startedMs: Date.now(), exitCode: null, simulated: false,
    };
    let run = base;
    const root = await show(first.id, UNLOCKED, { updates_progress: () => [run], updates_cancel: () => "Stopping apt before it installs anything" });
    await settle(root, () => !!root.querySelector(`[aria-label='Upgrade on ${first.name}']`));
    const panel = () => root.querySelector<HTMLElement>(`[aria-label='Upgrade on ${first.name}']`)!;
    expect(panel().textContent).toContain(`${said}.`);
    const stop = [...panel().querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === "Stop before it installs")!;
    expect(stop.disabled).toBe(false);
    await act(async () => fireEvent.click(stop));
    await settle(root, () => mockCalls.some((c) => c.cmd === "updates_cancel"));
    expect(mockCalls.filter((c) => c.cmd === "updates_cancel").map((c) => c.args)).toEqual([{ node: first.id }]);
    // Answered, or over: the words go.
    run = { ...base, stopFailed: null };
    await settle(root, () => !panel().textContent?.includes(said));
    run = { ...base, phase: "failed", cancellable: false };
    await settle(root, () => panel().textContent?.includes(`Upgrade failed on ${first.name}`) ?? false);
    expect(panel().textContent).not.toContain(said);
  });

  it("a pick apt defers isn't sent: the backend refuses a plan that leaves out a selected package", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, UNLOCKED, {
      updates_simulate: ({ node }: { node: string }) => ({ ...planOf(["libbrotli1"])({ node, packages: ["libbrotli1"] }), phased: ["nginx"] }),
      updates_upgrade: () => "t-up",
    });
    await settle(root, () => !!root.querySelector("tr[data-key='nginx']"));
    await pick(root, "libbrotli1");
    await pick(root, "nginx");
    await settle(root, () => !!tool(root, "Upgrade 2"));
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => !!root.querySelector("[aria-label='The plan Upgrade installs']"));
    await settle(root, () => !tool(root, "Upgrade 2").disabled);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await act(async () => fireEvent.click(tool(root, "Upgrade 2")));
    await settle(root, () => mockCalls.some((c) => c.cmd === "updates_upgrade"));
    expect(mockCalls.filter((c) => c.cmd === "updates_upgrade").map((c) => c.args)).toEqual([{ node: first.id, packages: ["libbrotli1"], opts: { plan: [{ name: "libbrotli1", to: "1.0-2" }] } }]);
    asked.mockRestore();
  });

  it("the Snaps cell's tooltip lists each pending snap, with its version when it's known", async () => {
    const second = snapshot().nodes[1];
    const root = await show(second.id, UNLOCKED, {
      updates_summary: ({ nodes }: { nodes: string[] | null }) =>
        summaries().filter((s) => !nodes || nodes.includes(s.node)).map((s) => (s.node === second.id ? { ...s, snaps: [{ name: "firefox", current: "140.0", available: "141.0" }, { name: "core22", current: null, available: "20261001" }] } : s)),
    });
    await settle(root, () => cellsOf(root, second.id).some((c) => c.includes("7.6.0")));
    const cell = [...root.querySelectorAll<HTMLElement>(`tr[data-key="${second.id}"] td`)].find((td) => td.title.includes("firefox"));
    expect(cell?.title).toBe("firefox: 140.0 → 141.0\ncore22: 20261001");
    expect(cell?.textContent).toBe("2");
  });

  it("says what Docker and the kernel do, and finds hold drift", () => {
    expect(dockerLine(true, 3)).toBe("Docker restarts; with live-restore on, running containers keep running.");
    expect(dockerLine(false, 3)).toBe("Docker restarts: its 3 running containers stop; only those with a restart policy come back.");
    expect(dockerLine(null, 2)).toBe("Docker restarts during the upgrade: its 2 running containers stop, and only those with a restart policy come back by themselves (couldn't read whether live-restore is on).");
    expect(kernelLine({ kernelStatus: 1, kernelCurrent: null, kernelExpected: null, services: [] })).toBeNull();
    expect(kernelLine({ kernelStatus: 2, kernelCurrent: null, kernelExpected: null, services: [] })).toBe("A newer kernel is installed; it runs after the next restart.");
    expect(holdDrift([{ name: "a", held: [] }, { name: "b", held: [] }, { name: "c", held: ["x"] }, { name: "d", held: null }])).toEqual(["c"]);
    expect(holdDrift([{ name: "a", held: ["x", "y"] }, { name: "b", held: ["y", "x"] }])).toEqual([]);
  });
});

describe("Retry opens Updates on the node it ran on", () => {
  const retry = (node: string): Task => ({
    id: `t-${node}`, kind: "updates", title: `Upgrade ${node}`, nodes: [node], source: "ui", cancellable: true, state: "failed", startedMs: Date.now(), finishedMs: Date.now(),
    progress: null, detail: null, rateBps: null, etaS: null, steps: [], log: [], summary: null, error: "apt-get failed", time: "", tsMs: null, priority: 6, text: "",
    cancelRequested: false, retry: { command: "updates_upgrade", args: { node, packages: [], opts: {} } }, opId: `t-${node}`,
  }) as unknown as Task;
  const selectedNode = (root: HTMLElement) => root.querySelector<HTMLElement>("table[aria-label='Nodes'] tr[aria-selected='true'], [aria-label='Nodes'] tr[aria-selected='true']")?.dataset.key;

  it("selects the node on open, and again each time Retry brings the one window forward", async () => {
    // The real registry: `node` being a view of this window is what keeps it one window. The
    // chip is what turns Retry's open-window event into a window.
    await import("./registry");
    const [, n2, n3] = snapshot().nodes;
    await backend(UNLOCKED, {}, false);
    const Body: BodyComponent = ({ params }) => <UpdatesWindow params={params} />;
    let r!: ReturnType<typeof render>;
    await act(async () => {
      r = render(<WindowManagerProvider sizeOf={() => [1400, 900]} isKnown={() => true} titleOf={(w) => w.kind}><Desktop Body={Body} /><TasksChip /></WindowManagerProvider>);
    });
    const root = r.container;
    await act(async () => void (await retryTask(retry(n2.id))));
    await settle(root, () => selectedNode(root) === n2.id);
    expect(selectedNode(root)).toBe(n2.id);
    // The owner moves on, then retries the same node, then another.
    await act(async () => {
      fireEvent.mouseDown(root.querySelector(`tr[data-key="${n3.id}"] td`)!, { button: 0 });
    });
    await settle(root, () => selectedNode(root) === n3.id);
    await act(async () => void (await retryTask(retry(n2.id))));
    await settle(root, () => selectedNode(root) === n2.id);
    expect(selectedNode(root)).toBe(n2.id);
    await act(async () => void (await retryTask(retry(n3.id))));
    await settle(root, () => selectedNode(root) === n3.id);
    expect(selectedNode(root)).toBe(n3.id);
    expect(root.querySelectorAll("[role=toolbar][aria-label='Updates']")).toHaveLength(1);
  });
});

describe("Updates keeps its promises (P1 r4)", () => {
  const pick = async (root: HTMLElement, name: string) => {
    const row = [...root.querySelectorAll<HTMLElement>(`tr[data-key='${name}']`)].at(-1)!;
    const box = row.querySelector<HTMLInputElement>("input[type=checkbox]");
    await act(async () => { if (box) fireEvent.click(box); else fireEvent.mouseDown(row.querySelector("td")!, { button: 0, metaKey: true }); });
  };
  const finishedTask = (id: string) => ({
    id, kind: "updates", title: "Check for updates on 4 nodes", nodes: [], source: "ui", cancellable: false, state: "ok", startedMs: Date.now(), finishedMs: Date.now(),
    progress: null, detail: null, rateBps: null, etaS: null, steps: [], log: [], summary: "Package lists are up to date", error: null, time: "", tsMs: null, priority: 6, text: "",
    cancelRequested: false, retry: null, opId: id,
  });

  it("Check all nodes reads what the refresh found: no second full check", async () => {
    const root = await show(snapshot().nodes[0].id, UNLOCKED, {
      updates_refresh: () => {
        setTimeout(() => void emitMock("tasks://update", finishedTask("t-ref")), 50);
        return "t-ref";
      },
      tasks_list: () => [],
    });
    await settle(root, () => !!tool(root, "Check all nodes") && !tool(root, "Check all nodes").disabled);
    // The task store listens on this test's event bus.
    _resetTasksForTests();
    const before = mockCalls.filter((c) => c.cmd === "updates_summary").length;
    await act(async () => fireEvent.click(tool(root, "Check all nodes")));
    await settle(root, () => mockCalls.filter((c) => c.cmd === "updates_summary").length > before);
    for (let i = 0; i < 4; i++) await act(async () => new Promise((res) => setTimeout(res, 25)));
    const after = mockCalls.filter((c) => c.cmd === "updates_summary").slice(before);
    expect(mockCalls.filter((c) => c.cmd === "updates_refresh")).toHaveLength(1);
    expect(after.map((c) => c.args)).toEqual([{ nodes: snapshot().nodes.map((n) => n.id), fresh: null }]);
    // Opening the window checks nothing either: every read is the cache's.
    expect(mockCalls.filter((c) => c.cmd === "updates_summary" && c.args.fresh)).toHaveLength(0);
  });

  it("the dry-run panel shows the plan Upgrade binds to, its refusal, and C1′", async () => {
    const first = snapshot().nodes[0];
    const root = await show(first.id, UNLOCKED, {
      updates_simulate: ({ node }: { node: string }) => ({
        ...planOf(["libbrotli1"])({ node, packages: ["libbrotli1"] }),
        installs: ["libnew 2.0-1"],
        plan: [{ name: "libbrotli1", from: "1.1.0-2build2", to: "1.1.0-2ubuntu0.1", new: false }, { name: "libnew", from: null, to: "2.0-1", new: true }],
        command: "env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y --no-remove install --only-upgrade 'libbrotli1=1.1.0-2ubuntu0.1'",
      }),
    });
    await pick(root, "libbrotli1");
    await settle(root, () => !!tool(root, "Simulate") && !tool(root, "Simulate").disabled);
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => !!root.querySelector("[aria-label='The plan Upgrade installs']"));
    const plan = root.querySelector<HTMLElement>("[aria-label='The plan Upgrade installs']")!;
    expect([...plan.querySelectorAll("li")].map((li) => li.textContent)).toEqual(["libbrotli1: 1.1.0-2build2 → 1.1.0-2ubuntu0.1", "libnew: 2.0-1 (new)"]);
    const panel = root.querySelector<HTMLElement>("[aria-label='Simulated upgrade']")!;
    expect(panel.textContent).toContain("--no-remove install --only-upgrade 'libbrotli1=1.1.0-2ubuntu0.1'");
    expect([tool(root, "Upgrade 1").disabled]).toEqual([false]);
    // A removal: the panel says why, in the backend's words (one full stop), and Upgrade is off.
    const why = "This plan removes libfoo-doc. SparkBox's one-node upgrades only upgrade; run it in a terminal if you mean it.";
    extendMock({ updates_simulate: ({ node }: { node: string }) => ({ ...planOf(["libbrotli1"])({ node, packages: ["libbrotli1"] }), removals: ["libfoo-doc"], removes: ["libfoo-doc"], refused: why }) });
    await act(async () => fireEvent.click(tool(root, "Simulate")));
    await settle(root, () => (root.querySelector("[aria-label='Simulated upgrade']")?.textContent ?? "").includes("libfoo-doc"));
    expect(root.querySelector("[aria-label='Simulated upgrade']")!.textContent).toContain(`Upgrade refused: ${why}`);
    expect(root.querySelector("[aria-label='Simulated upgrade']")!.textContent).not.toContain("mean it..");
    expect([tool(root, "Upgrade 1").disabled, tool(root, "Upgrade 1").title]).toEqual([true, why]);
    expect(planLine({ name: "libfoo", from: "1.0-1", to: "2.0-1", new: false })).toBe("libfoo: 1.0-1 → 2.0-1");
  });

  it("with Live data and no dry run on screen, Upgrade is off and says why", async () => {
    const root = await show(snapshot().nodes[0].id);
    await settle(root, () => !!tool(root, "Upgrade node"));
    const up = tool(root, "Upgrade node");
    expect([up.disabled, up.title]).toEqual([true, DRY_RUN_FIRST]);
    // Demo binds to its own dry run: no dry run needed there.
    const demo = await show(snapshot().nodes[0].id, UNLOCKED, {}, true);
    await settle(demo, () => !!tool(demo, "Upgrade node") && !tool(demo, "Upgrade node").disabled);
    expect(tool(demo, "Upgrade node").disabled).toBe(false);
  });

  it("a lost upgrade says apt may still be running, and offers no Cancel", async () => {
    const first = snapshot().nodes[0];
    const lost: UpgradeRun = {
      node: first.id, taskId: "t-l", title: `Upgrade ${first.name}`, unit: "sparkbox-upgrade-t-l", logPath: "/var/log/sparkbox/upgrade-t-l.log", packages: ["libbrotli1"],
      phase: "lost", done: 0, total: 1, current: null, log: ["Get:1 http://ports.ubuntu.com libbrotli1"], cancellable: false, startedMs: Date.now(), exitCode: null, simulated: false,
    };
    const root = await show(first.id, UNLOCKED, { updates_progress: () => [lost] });
    await settle(root, () => !!root.querySelector(`[aria-label='Upgrade on ${first.name}']`));
    const panel = root.querySelector<HTMLElement>(`[aria-label='Upgrade on ${first.name}']`)!;
    expect(panel.querySelector("h3")!.textContent).toBe(`Lost ${first.name} while it was upgrading`);
    expect(panel.textContent).toContain(`apt may still be running on ${first.name}. SparkBox follows it again when ${first.name} is back, and after a restart (log: /var/log/sparkbox/upgrade-t-l.log).`);
    expect([...panel.querySelectorAll("button")].map((b) => b.textContent?.trim())).not.toContain("Stop before it installs");
    // A Demo run isn't Live's (nor the other way round).
    const demo = await show(first.id, UNLOCKED, { updates_progress: () => [lost] }, true);
    for (let i = 0; i < 6; i++) await act(async () => new Promise((res) => setTimeout(res, 25)));
    expect(demo.querySelector(`[aria-label='Upgrade on ${first.name}']`)).toBeNull();
  });

  it("Restart node… says it's checking while the checks before the restart run, then asks", async () => {
    const first = snapshot().nodes[0];
    let answer!: (v: unknown) => void;
    const root = await show(first.id, UNLOCKED, {
      updates_summary: ({ nodes }: { nodes: string[] | null }) =>
        summaries().filter((s) => !nodes || nodes.includes(s.node)).map((s) => (s.node === first.id ? { ...s, apt: { ...s.apt!, rebootRequired: true, rebootPackages: ["linux-image-7.0.0-1020-nvidia"] } } : s)),
      node_prereboot_check: () => new Promise((res) => (answer = res)),
      vpn_get: () => ({ status: { state: "direct" } }),
    });
    await settle(root, () => !!tool(root, "Restart node…"));
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await act(async () => fireEvent.click(tool(root, "Restart node…")));
    await settle(root, () => mockCalls.some((c) => c.cmd === "node_prereboot_check"));
    expect(getToasts().map((t) => [t.kind, t.title])).toContainEqual(["progress", `Checking ${first.name} before the restart…`]);
    expect(asked).not.toHaveBeenCalled();
    await act(async () => answer({ node: first.id, kernel: "7.0.0-1019-nvidia", ok: true, blocking: [], checks: [{ id: "kernel", label: "Kernel that boots next", state: "ok", detail: "7.0.0-1019-nvidia" }] }));
    await settle(root, () => asked.mock.calls.length > 0);
    expect(getToasts().some((t) => t.title.startsWith("Checking "))).toBe(false);
    const body = render(<>{asked.mock.calls[0][0].body}</>).container.textContent ?? "";
    expect(body).toContain("Checks before the restart: passed");
    // Calling it off can fail (shutdown -c refused or failing): the restart then still happens, and the task says so.
    expect(body).toContain(`It's checked now. In 5 minutes is checked again a minute before its time; neither is checked when the restart fires. If that second check fails, SparkBox tries to call it off (shutdown -c) and says in the task if it couldn't. If SparkBox isn't running at that moment, ${first.name} restarts on its own schedule without that second check.`);
    expect(body).not.toContain("Scheduled, it's checked again");
    expect(body).not.toContain("called off if a check fails");
    asked.mockRestore();
  });
});
