// Workloads lists every process (P1.47): each second's snapshot carries a node's 15 busiest
// processes and the listeners' owners (processCount says how many it reported), and the open
// window asks for the whole list (processes_get) of each shown node whose snapshot is cut, every
// 5 s. The snapshot's rows stay the fresher: the fetched list only adds the ones it left out.
// Port owners named from an idle process (nginx on 8080, "likely") come from that whole list.
import { act, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests } from "../lib/admin";
import type { AdminState } from "../lib/opsApi";
import { resetStore, startClusterStore } from "../lib/store";
import type { ClusterSnapshot, Process } from "../lib/types";
import { _forgetLastKnownForTests } from "../lib/workloadCommands";
import { emitMock, mockCalls, mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { PROCESSES_EVERY_MS, Workloads } from "./Workloads";

const ADMIN: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const base = (baseSnapshot as ClusterSnapshot).nodes[0].processes[0];
const node0 = (baseSnapshot as ClusterSnapshot).nodes[0];

/** cell1's 40 processes, busiest first; the last is an idle nginx whose port only root could see. */
const ALL: Process[] = [
  ...Array.from({ length: 39 }, (_, i) => ({ ...base, pid: 2000 + i, name: `worker-${i}`, cmd: `worker-${i}`, user: "sparkuser", unit: null, cpuPct: 40 - i, ports: [] })),
  { ...base, pid: 2100, name: "nginx", cmd: "nginx: master process /usr/sbin/nginx", unit: "nginx.service", cpuPct: 0, ports: [] },
];

/** What each second's snapshot says of cell1: its 15 busiest (`busiest`), and that it reported 40. */
function snapshot(busiest: Process[] = ALL.slice(0, 15)): ClusterSnapshot {
  const s = structuredClone(baseSnapshot) as ClusterSnapshot;
  s.tsMs = Date.now();
  s.demo = false;
  s.nodes[0].processes = busiest;
  s.nodes[0].processCount = ALL.length;
  s.nodes[0].listeners = [{ port: 8080, addr: "0.0.0.0", pid: null, process: null }];
  return s;
}

async function show(processesGet: (args: { node: string }) => unknown) {
  resetStore();
  _forgetLastKnownForTests();
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => snapshot(), admin_state: () => ADMIN, processes_get: processesGet }));
  _resetAdminForTests(ADMIN);
  await startClusterStore();
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1400, 900]} isKnown={() => true}><Workloads /></WindowManagerProvider>);
  });
  for (let i = 0; i < 10; i++) await act(async () => new Promise((res) => setTimeout(res, 20)));
  return r.container;
}

const rowsOf = (root: HTMLElement, id: string) => root.querySelectorAll(`tr[data-key^="${id}/"]`).length;
const asked = () => mockCalls.filter((c) => c.cmd === "processes_get").map((c) => c.args.node as string);

afterEach(() => vi.useRealTimers());

describe("Workloads › every process", () => {
  it("asks a node whose snapshot is cut for its whole list every 5 s and lists more than the snapshot's 15", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const root = await show(({ node }) => (node === node0.id ? ALL : snapshot().nodes.find((n) => n.id === node)!.processes));
    expect(rowsOf(root, node0.id)).toBe(40);
    // The other nodes' snapshots carry their whole lists: nothing to ask for.
    expect(snapshot().nodes.slice(1).every((n) => n.processCount == null)).toBe(true);
    expect(asked()).toEqual([node0.id]);
    await act(async () => {
      vi.advanceTimersByTime(PROCESSES_EVERY_MS);
    });
    expect(asked()).toEqual([node0.id, node0.id]);
  });

  it("shows the next second's numbers and a new process before the next fetch", async () => {
    const root = await show(({ node }) => (node === node0.id ? ALL : []));
    const row = (pid: number) => root.querySelector(`tr[data-key="${node0.id}/${pid}"]`)?.textContent ?? "";
    expect(row(2000)).toContain("40%");
    // A second later cell1 reports its busiest at 95% and a newcomer among the 15.
    const newcomer = { ...base, pid: 3999, name: "newcomer", cmd: "newcomer", user: "sparkuser", unit: null, cpuPct: 50, ports: [] };
    await act(async () => {
      await emitMock("cluster://snapshot", snapshot([{ ...ALL[0], cpuPct: 95 }, newcomer, ...ALL.slice(1, 14)]));
    });
    for (let i = 0; i < 5; i++) await act(async () => new Promise((res) => setTimeout(res, 20)));
    expect(row(2000)).toContain("95%");
    expect(row(3999)).toContain("newcomer");
    expect(rowsOf(root, node0.id)).toBe(41);
    expect(asked().filter((n) => n === node0.id).length).toBe(1);
  });

  it("lists a whole snapshot as it comes, without asking for it", async () => {
    resetStore();
    const whole = (cpu: number) => {
      const s = snapshot(ALL.map((p, i) => (i === 0 ? { ...p, cpuPct: cpu } : p)));
      s.nodes[0].processCount = null;
      return s;
    };
    _forgetLastKnownForTests();
    mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => whole(40), admin_state: () => ADMIN, processes_get: () => ALL }));
    _resetAdminForTests(ADMIN);
    await startClusterStore();
    let r!: ReturnType<typeof render>;
    await act(async () => {
      r = render(<WindowManagerProvider sizeOf={() => [1400, 900]} isKnown={() => true}><Workloads /></WindowManagerProvider>);
    });
    for (let i = 0; i < 10; i++) await act(async () => new Promise((res) => setTimeout(res, 20)));
    expect(rowsOf(r.container, node0.id)).toBe(40);
    await act(async () => {
      await emitMock("cluster://snapshot", whole(95));
    });
    for (let i = 0; i < 5; i++) await act(async () => new Promise((res) => setTimeout(res, 20)));
    expect(r.container.querySelector(`tr[data-key="${node0.id}/2000"]`)?.textContent).toContain("95%");
    expect(asked()).toEqual([]);
  });

  it("names a port's likely owner from the whole list", async () => {
    const root = await show(({ node }) => (node === node0.id ? ALL : []));
    const ports = [...root.querySelectorAll<HTMLButtonElement>(".seg button")].find((b) => b.textContent === "Listening ports")!;
    await act(async () => {
      fireEvent.click(ports);
    });
    const row = root.querySelector<HTMLElement>(`tr[data-key="${node0.id}:0.0.0.0:8080"]`)!;
    expect(row.textContent).toContain("nginx");
    expect(row.textContent).toContain("(likely)");
  });

  it("says so when it could only list the snapshot's busiest", async () => {
    const root = await show(() => {
      throw new Error("cell1 isn't connected.");
    });
    expect(rowsOf(root, node0.id)).toBe(15);
    expect(root.textContent).toContain(`${node0.name} shows 15 of its 40 processes: the 15 busiest and those listening on a port. SparkBox couldn't read the whole list`);
  });
});
