// The Dashboard's readouts (P1.11, P1.44): the cluster GPU power sums the GPUs that report and
// says "(3 of 4)" when one doesn't, and a root disk's free space is what df reports.
import { act, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import disksDoctor from "../../src-tauri/src/live/fixtures/disks-doctor.json";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { resetStore, startClusterStore } from "../lib/store";
import type { ClusterSnapshot, NodeSnapshot } from "../lib/types";
import { mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { Dashboard, gpuPower, gpuPowerSub, sumReporting } from "./Dashboard";

vi.mock("uplot", () => {
  class FakePlot {
    series: { show: boolean }[] = [];
    setData() {}
    setSeries() {}
    setSize() {}
    setSelect() {}
    destroy() {}
    static sync() {
      return { sub() {}, unsub() {}, pub() {} };
    }
  }
  return { default: FakePlot };
});

const NOW = Date.now();

/** The four nodes at rest, gx10-0000's root disk as df printed it, cell3's GPU not answering (an Xid 79). */
function snapshot(): ClusterSnapshot {
  const s = structuredClone(baseSnapshot) as ClusterSnapshot;
  s.tsMs = NOW;
  for (const n of s.nodes) n.gpu = { ...n.gpu, powerW: 20, readable: true, error: null };
  const gx10 = s.nodes.find((n) => n.id === "gx10-0000")!;
  const df = disksDoctor.disks.find((d) => d.node === "gx10-0000")!.df;
  gx10.disks = [{ mount: "/", device: "/dev/nvme0n1p2", fsType: "ext4", total: df.total, used: df.used, avail: df.avail, readBps: 0, writeBps: 0 }];
  const cell3 = s.nodes.find((n) => n.id === "cell3")!;
  cell3.gpu = { ...cell3.gpu, powerW: 0, readable: false, error: "GPU fallen off the bus (Xid 79)" };
  return s;
}

describe("the cluster GPU power", () => {
  it("sums the GPUs that report and says how many of all", () => {
    const s = snapshot();
    expect(gpuPower(s)).toEqual({ watts: 60, counted: 3, total: 4, of: "(3 of 4)" });
    expect(gpuPowerSub(s)).toBe("avg 20 W (3 of 4)");
    // Every GPU reporting: no suffix, the usual line.
    const all = structuredClone(s);
    all.nodes.forEach((n: NodeSnapshot) => (n.gpu = { ...n.gpu, powerW: 20, readable: true, error: null }));
    expect(gpuPower(all).of).toBe("");
    expect(gpuPowerSub(all)).toBe("4 nodes · avg 20 W");
    // An offline node's GPU doesn't report either.
    all.nodes[0].status = "offline";
    expect(gpuPower(all)).toMatchObject({ watts: 60, of: "(3 of 4)" });
    all.nodes.forEach((n: NodeSnapshot) => (n.status = "offline"));
    expect(gpuPowerSub(all)).toBe("No GPU reporting");
  });

  it("charts each second's sum over the GPUs that reported then: a gap only when none did", () => {
    expect(sumReporting([[4, 4, null], [5, null, null], [null, 6, null]], 3)).toEqual([9, 10, null]);
  });
});

describe("the Dashboard", () => {
  it("shows the GPU power of the three that report, and gx10-0000's free disk as df does", async () => {
    mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => snapshot() }));
    resetStore();
    await startClusterStore();
    let container!: HTMLElement;
    await act(async () => {
      container = render(<WindowManagerProvider sizeOf={() => [1180, 760]} isKnown={() => true}><Dashboard /></WindowManagerProvider>).container;
    });
    for (let i = 0; i < 20 && !container.querySelector(".tile"); i++) await act(async () => new Promise((r) => setTimeout(r, 50)));
    const tile = [...container.querySelectorAll(".tile")].find((t) => t.querySelector(".tile-label")?.textContent?.startsWith("GPU power"))!;
    expect(tile.querySelector(".tile-value")?.textContent).toBe("60W");
    expect(tile.textContent).toContain("avg 20 W (3 of 4)");
    const disk = container.querySelector('[role="progressbar"][aria-label="Root disk on gx10-0000"]');
    expect(disk?.getAttribute("aria-valuetext")).toBe("40% · 520.5 GiB free");
  });
});
