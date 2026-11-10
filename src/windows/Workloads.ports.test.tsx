// Workloads › Listening ports › Open in browser (P1.18): it offers the ports the node itself
// lists, never SSH's or DNS's, and opens one through an SSH tunnel to that port; in Demo it says
// it needs Live data. A tunnel needs Admin, so the button and a row's double-click follow the
// lock rule (P1.43).
import { act, fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP } from "../lib/admin";
import type { AdminState } from "../lib/opsApi";
import * as session from "../lib/session";
import { resetStore, startClusterStore } from "../lib/store";
import { nodeCommands } from "../lib/workloadCommands";
import type { ClusterSnapshot } from "../lib/types";
import { mockCalls, mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { browsable, Workloads } from "./Workloads";

const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const LOCKED: AdminState = { ...UNLOCKED, unlocked: false };
const LOOK_ONLY: AdminState = { ...LOCKED, lookOnly: true };

/** cell1 lists SSH, DNS and a model server on 8000; the other nodes list nothing. */
function snapshot(demo: boolean): ClusterSnapshot {
  const s = structuredClone(baseSnapshot) as ClusterSnapshot;
  s.tsMs = Date.now();
  s.demo = demo;
  for (const n of s.nodes) n.listeners = [];
  s.nodes[0].listeners = [
    { port: 22, addr: "0.0.0.0", pid: null, process: null },
    { port: 53, addr: "127.0.0.53", pid: null, process: null },
    { port: 8000, addr: "0.0.0.0", pid: null, process: null },
  ];
  return s;
}

async function show(demo: boolean, admin: AdminState = UNLOCKED) {
  resetStore();
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, {
    get_snapshot: () => snapshot(demo),
    admin_state: () => admin,
    tunnel_open: () => "http://127.0.0.1:50124/?sbt=0123",
  }));
  _resetAdminForTests(admin);
  await startClusterStore();
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1400, 900]} isKnown={() => true}><Workloads /></WindowManagerProvider>);
  });
  const root = r.container;
  for (let i = 0; i < 20 && !root.querySelector(".seg"); i++) await act(async () => new Promise((res) => setTimeout(res, 50)));
  const ports = [...root.querySelectorAll<HTMLButtonElement>(".seg button")].find((b) => b.textContent === "Listening ports")!;
  await act(async () => {
    fireEvent.click(ports);
  });
  return root;
}

const node0 = snapshot(false).nodes[0];
const row = (root: HTMLElement, port: number) => root.querySelector<HTMLElement>(`tr[data-key="${node0.id}:${port === 53 ? "127.0.0.53" : "0.0.0.0"}:${port}"]`)!;
const openButton = (root: HTMLElement) =>
  [...root.querySelectorAll<HTMLButtonElement>("[role=toolbar] button")].find((b) => b.getAttribute("aria-label")?.startsWith("Open in browser"))!;

async function pick(root: HTMLElement, port: number) {
  await act(async () => {
    fireEvent.mouseDown(row(root, port).querySelector("td")!, { button: 0 });
  });
}

describe("Workloads › Open in browser", () => {
  it("offers the node's own listening ports but SSH's and DNS's", async () => {
    const root = await show(false);
    expect(root.querySelectorAll("tr[data-key]").length).toBe(3);
    await pick(root, 22);
    expect([openButton(root).disabled, openButton(root).title]).toEqual([true, "Port 22 is SSH, not a web page"]);
    await pick(root, 53);
    expect([openButton(root).disabled, openButton(root).title]).toEqual([true, "Port 53 is DNS, not a web page"]);
    await pick(root, 8000);
    expect(openButton(root).disabled).toBe(false);
    await act(async () => {
      fireEvent.click(openButton(root));
    });
    for (let i = 0; i < 20 && !mockCalls.some((c) => c.cmd === "tunnel_open"); i++) await act(async () => new Promise((res) => setTimeout(res, 50)));
    expect(mockCalls.filter((c) => c.cmd === "tunnel_open").map((c) => c.args)).toEqual([{ node: node0.id, port: 8000 }]);
  });

  it("says it needs Live data in Demo, and opens nothing", async () => {
    const root = await show(true);
    await pick(root, 8000);
    await act(async () => {
      fireEvent.click(openButton(root));
    });
    expect(root.textContent).toContain(`Available with Live data: SparkBox opens ${node0.name}'s port 8000 in your browser through an SSH tunnel.`);
    expect(mockCalls.some((c) => c.cmd === "tunnel_open")).toBe(false);
  });

  it("in look-only the button is off with the look-only tip, and a double-click opens nothing", async () => {
    const root = await show(false, LOOK_ONLY);
    await pick(root, 8000);
    const button = openButton(root);
    expect([button.disabled, button.title, button.dataset.lock]).toEqual([true, LOOK_ONLY_TIP, "lookOnly"]);
    const told = vi.spyOn(session, "alertInfo").mockResolvedValue();
    await act(async () => {
      fireEvent.doubleClick(row(root, 8000).querySelector("td")!);
    });
    expect(told).toHaveBeenCalledOnce();
    expect(told.mock.calls[0][0]).toMatchObject({ title: "Look only", body: expect.stringContaining("SparkBox can't open a web page on a node in look-only mode") });
    expect(mockCalls.some((c) => c.cmd === "tunnel_open" || c.cmd === "admin_unlock")).toBe(false);
    told.mockRestore();
  });

  it("with Admin locked the button shows the lock, and it and a double-click ask with the in-app sheet first", async () => {
    const root = await show(false, LOCKED);
    await pick(root, 8000);
    const button = openButton(root);
    expect([button.disabled, button.dataset.lock, button.title.endsWith(NEEDS_ADMIN_TIP)]).toEqual([false, "locked", true]);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await act(async () => {
      fireEvent.click(button);
    });
    await act(async () => {
      fireEvent.doubleClick(row(root, 8000).querySelector("td")!);
    });
    expect(asked).toHaveBeenCalledTimes(2);
    expect(asked.mock.calls.map((c) => c[0].title)).toEqual(["Unlock Admin mode?", "Unlock Admin mode?"]);
    expect(mockCalls.some((c) => c.cmd === "tunnel_open" || c.cmd === "admin_unlock")).toBe(false);
    asked.mockRestore();
  });

  it("the palette's DGX Dashboard needs Admin with Live data, like the buttons", () => {
    const dgx = (demo: boolean) => nodeCommands(snapshot(demo)).find((c) => c.id === `node:${node0.id}:dgx`)!;
    expect([dgx(false).needsAdmin, dgx(true).needsAdmin]).toEqual([true, false]);
  });

  it("leaves out SSH and DNS", () => {
    expect([22, 53, 8000, 11000, 11434].map((port) => browsable({ port }))).toEqual([false, false, true, true, true]);
  });
});
