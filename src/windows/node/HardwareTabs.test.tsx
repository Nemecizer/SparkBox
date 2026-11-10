// The node's Memory, Disks and GPU tabs read what the node reports (P1.11, P1.44): each memory
// series its own value (not the running total it's stacked at), disk free as df prints it, and
// an unreadable GPU's last processes greyed.
import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import disksDoctor from "../../../src-tauri/src/live/fixtures/disks-doctor.json";
import { diskFree, diskUsedPct } from "../../lib/metrics";
import { recordSnapshot, resetStore } from "../../lib/store";
import type { ClusterSnapshot, NodeSnapshot } from "../../lib/types";
import { GpuTab } from "./GpuTab";
import { DisksTab, MemoryTab } from "./HardwareTabs";

vi.mock("uplot", () => {
  class FakePlot {
    series: { show: boolean }[] = [];
    width = 600;
    height = 160;
    bbox = { left: 0, top: 0, width: 1000, height: 300 };
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

const GiB = 2 ** 30;

/** cell1 at rest in the Live tour (doctor.txt): 4.8 GiB in use, 3.7 GiB of page cache, nothing on the GPU. */
function cell1(over: Partial<NodeSnapshot> = {}): NodeSnapshot {
  return {
    id: "cell1", name: "cell1", host: "cell1.local", status: "online", stale: false,
    cpu: { totalPct: 1, perCore: [], load: [0.1, 0.1, 0.1], tempC: 46 },
    mem: { total: 130661117952, used: 5187248128, available: 125473869824, cached: 3921817600, gpuAlloc: 0, swapTotal: 17179865088, swapUsed: 73859072 },
    gpu: { utilPct: 0, tempC: 44, powerW: 4.1, smClockMhz: 208, procs: [], readable: true, pstate: "P0" },
    disks: [], nets: [], containers: [], processes: [], listeners: [], failedUnits: [], kernelEvents: [],
    system: { hostname: "cell1-example", model: "MS-C931", os: "", kernel: "", driver: "580.178.04", cuda: "13.0", uptimeS: 310049 },
    ...over,
  } as unknown as NodeSnapshot;
}

const chart = (nowS: number) => ({ rangeS: 900, nowS, zoom: null, onZoom: () => {} });

describe("the Memory tab", () => {
  beforeEach(() => resetStore());

  it("reads each series' own value, not the stacked total (Cache 3.7 GiB, not 8.5)", () => {
    const now = Math.floor(Date.now() / 1000);
    const node = cell1();
    for (let i = 30; i >= 0; i--) recordSnapshot({ tsMs: (now - i) * 1000, demo: false, nodes: [node], switches: [] } as unknown as ClusterSnapshot);
    const { container } = render(<MemoryTab node={node} chart={chart(now)} />);
    const keys = [...container.querySelectorAll(".tc-key")].map((k) => k.textContent);
    expect(keys).toEqual(["GPU0 GiB", "Other4.8 GiB", "Cache3.7 GiB"]);
    expect(keys.join(" ")).not.toContain("8.5 GiB");
    // The pool split above it says the same.
    expect(container.querySelector(".legend")?.textContent).toContain("Page cache 3.7 GiB");
  });
});

describe("disk free as df reports it", () => {
  const gx10 = disksDoctor.disks.find((d) => d.node === "gx10-0000")!;
  const root = { mount: "/", device: "/dev/nvme0n1p2", fsType: "ext4", total: gx10.df.total, used: gx10.df.used, avail: gx10.df.avail, readBps: 0, writeBps: 0 };

  it("is avail, and Use% is used / (used + avail)", () => {
    expect((diskFree(root) / GiB).toFixed(1)).toBe("520.5");
    expect(Math.ceil(diskUsedPct(root)), "df rounds Use% up").toBe(gx10.df.usePct);
    // An older collector sends no avail: total minus used, over the total.
    const old = { ...root, avail: undefined };
    expect((diskFree(old) / GiB).toFixed(1)).toBe("567.1");
    expect(Math.round(diskUsedPct(old))).toBe(38);
    expect(diskFree({ ...root, avail: null })).toBe(diskFree(old));
    // Full for everyone but root: avail 0, the 5% reserve still unused. df: Available 0, Use% 100%.
    const full = { ...root, total: 239_946_252 * 4096, used: (239_946_252 - 11_997_312) * 4096, avail: 0 };
    expect([diskFree(full), diskUsedPct(full)]).toEqual([0, 100]);
  });

  it("reads that way in the Disks tab", () => {
    const now = Math.floor(Date.now() / 1000);
    render(<DisksTab node={cell1({ id: "gx10-0000", name: "gx10-0000", disks: [root] })} chart={chart(now)} />);
    const row = screen.getByRole("row", { name: /nvme0n1p2/ });
    expect(within(row).getByText("520.5 GiB")).toBeInTheDocument();
    // 40.08% (df rounds it up to 41%; SparkBox rounds as everywhere else), not the 38% of used / total.
    expect(within(row).getByRole("progressbar")).toHaveAttribute("aria-valuetext", "40% · 348.2 GiB");
  });
});

describe("an unreadable GPU", () => {
  it("greys what it last listed and its performance state", () => {
    const now = Math.floor(Date.now() / 1000);
    const gpu = { utilPct: 0, tempC: 0, powerW: 0, smClockMhz: 0, readable: false, error: "nvidia-smi didn't answer in 10 s", pstate: "P0", procs: [{ pid: 4242, name: "vllm", mem: 40 * GiB, container: "glm53" }] };
    const { container } = render(<GpuTab node={cell1({ gpu } as Partial<NodeSnapshot>)} chart={chart(now)} />);
    const row = screen.getByRole("row", { name: /vllm/ });
    expect(row.className).toContain("row-dim");
    expect(screen.getByText(/Performance state P0 \(last reading\)/)).toHaveClass("row-dim");
    expect(container.querySelectorAll(".row-dim").length).toBeGreaterThanOrEqual(2);
    // Readable again: nothing greyed.
    const ok = render(<GpuTab node={cell1({ gpu: { ...gpu, readable: true, error: null } } as Partial<NodeSnapshot>)} chart={chart(now)} />);
    expect(ok.container.querySelectorAll(".row-dim")).toHaveLength(0);
  });

  it("says what it uses isn't known, rather than nothing", () => {
    const now = Math.floor(Date.now() / 1000);
    render(<GpuTab node={cell1({ gpu: { utilPct: 0, tempC: 0, powerW: 0, smClockMhz: 0, readable: false, error: "nvidia-smi didn't answer in 10 s", procs: [] } } as Partial<NodeSnapshot>)} chart={chart(now)} />);
    expect(screen.getByText("The GPU isn't answering, so what uses it isn't known.")).toBeInTheDocument();
  });
});
