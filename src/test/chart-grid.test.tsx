// One grid per window (§2.4, C4): charts in a ChartGrid get one left-axis width and one right
// gutter, so their plots start and end at the same x and, for one span, carry the same ticks.
import { act, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ChartGrid, TimeChart, useChartGridInsets } from "../components/TimeChart";
import { StopIcon } from "../components/ui";

interface Opts {
  padding: [number, number, number, number];
  axes: { size?: number | ((...a: unknown[]) => number); splits?: (u: unknown) => number[] }[];
}
const built: Opts[] = [];

vi.mock("uplot", () => {
  class FakePlot {
    series: { show: boolean }[] = [];
    width = 600;
    height = 160;
    bbox = { left: 0, top: 0, width: 1000, height: 300 };
    constructor(opts: Opts) {
      built.push(opts);
    }
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

function Probe({ need, out }: { need: { left: number; right: number }; out: { left: number; right: number }[] }) {
  const ins = useChartGridInsets(need);
  out.push({ left: ins.left, right: ins.right });
  return null;
}

describe("ChartGrid", () => {
  it("gives every chart the widest left axis and right gutter any of them needs", async () => {
    const a: { left: number; right: number }[] = [];
    const b: { left: number; right: number }[] = [];
    await act(async () => {
      render(
        <ChartGrid>
          <Probe need={{ left: 38, right: 8 }} out={a} />
          <Probe need={{ left: 84, right: 64 }} out={b} />
        </ChartGrid>,
      );
    });
    expect(a[a.length - 1]).toEqual({ left: 84, right: 64 });
    expect(b[b.length - 1]).toEqual({ left: 84, right: 64 });
    // Outside a grid a chart keeps its own.
    const solo: { left: number; right: number }[] = [];
    render(<Probe need={{ left: 38, right: 8 }} out={solo} />);
    expect(solo[solo.length - 1]).toEqual({ left: 38, right: 8 });
  });

  it("builds two charts with identical plot insets and x ticks", async () => {
    built.length = 0;
    const now = 1_800_000_900;
    const ts = Array.from({ length: 900 }, (_, i) => now - 899 + i);
    await act(async () => {
      render(
        <ChartGrid>
          <TimeChart title="Power" ts={ts} range={[now - 900, now]} fmt={(v) => `${v.toFixed(0)} W`} series={[{ label: "W", color: "--m-power", values: ts.map((_, i) => 50 + (i % 7)) }]} />
          <TimeChart title="Fabric" ts={ts} range={[now - 900, now]} robust="auto" fmt={(v) => `${(v / 1e6).toFixed(0)} Mb/s`}
            thresholds={[{ value: 4e8, label: "Busy 400 Mb/s", tone: "warn" }]}
            series={[{ label: "Total", color: "--m-tx", values: ts.map((_, i) => (i % 50 === 0 ? 6e9 : 1e8)) }]} />
        </ChartGrid>,
      );
    });
    // The last build of each chart is the one on screen.
    const [p, f] = built.slice(-2);
    expect(p.padding[1]).toBe(f.padding[1]);
    expect(p.padding[3]).toBe(f.padding[3]);
    expect(p.axes[1].size).toBe(f.axes[1].size);
    const fake = { bbox: { width: 1000 } };
    expect(p.axes[0].splits!(fake)).toEqual(f.axes[0].splits!(fake));
  });
});

describe("small multiples at rest (C1)", () => {
  const now = 1_800_000_900;

  it("draws a recording gap and the empty seconds around it as one band with one \"No data\"", () => {
    // The Live tour's shot 00: 6 minutes recorded, SparkBox closed for 2.5 minutes, empty seconds beside the gap.
    const before = Array.from({ length: 200 }, (_, i) => now - 600 + i);
    const after = Array.from({ length: 220 }, (_, i) => now - 219 + i);
    // The store leaves an empty reading one second after the last one when recording stops (store.push).
    const ts = [...before, before[before.length - 1] + 1, ...after];
    const row = (v: number) => ts.map((_, i) => (i === before.length || (i >= 195 && i < 199) ? null : v));
    const { container } = render(
      <TimeChart title="GPU utilization" mode="multiples" ts={ts} range={[now - 600, now]} relative yMax={100} fmt={(v) => `${v.toFixed(0)}%`}
        series={["cell1", "cell2", "cell3", "gx10-0000"].map((n) => ({ label: n, color: "--m-gpu", values: row(0) }))} />,
    );
    const labels = [...container.querySelectorAll(".tc-row-gap-label")].map((t) => t.textContent);
    expect(labels.filter((l) => /No data/.test(l ?? ""))).toHaveLength(1);
  });

  it("labels the minutes before this run's first reading \"No data recorded\" without guessing the cause", () => {
    // History began hours ago; this run has recorded the last 3 minutes only.
    const ts = Array.from({ length: 180 }, (_, i) => now - 179 + i);
    const { container } = render(
      <TimeChart title="GPU utilization" mode="multiples" ts={ts} range={[now - 900, now]} relative yMax={100} recordedFrom={now - 6 * 3600} fmt={(v) => `${v.toFixed(0)}%`}
        series={["cell1", "cell2"].map((n) => ({ label: n, color: "--m-gpu", values: ts.map(() => 1) }))} />,
    );
    const labels = [...container.querySelectorAll(".tc-row-gap-label")].map((t) => t.textContent);
    expect(labels).toEqual(["No data recorded"]);
  });

  it("shows \"—\" for a GPU that stopped answering, never its last busy value", () => {
    const ts = Array.from({ length: 120 }, (_, i) => now - 119 + i);
    const dead = ts.map((_, i) => (i < 100 ? 64 : null));
    const { container } = render(
      <TimeChart title="GPU utilization" mode="multiples" ts={ts} range={[now - 120, now]} relative yMax={100} fmt={(v) => `${v.toFixed(0)}%`}
        series={[{ label: "cell1", color: "--m-gpu", values: ts.map(() => 3) }, { label: "gx10-0000", color: "--m-gpu", values: dead }]} />,
    );
    const values = [...container.querySelectorAll(".tc-row-value")].map((t) => t.textContent);
    expect(values).toEqual(["3%", "—"]);
  });

  it("says \"No readings yet\" when no row has a reading", () => {
    const ts = Array.from({ length: 30 }, (_, i) => now - 29 + i);
    const { container } = render(
      <TimeChart title="GPU utilization" mode="multiples" ts={ts} range={[now - 120, now]} relative yMax={100} fmt={(v) => `${v.toFixed(0)}%`}
        series={[{ label: "cell1", color: "--m-gpu", values: ts.map(() => null) }, { label: "cell2", color: "--m-gpu", values: ts.map(() => null) }]} />,
    );
    expect(container.textContent).toContain("No readings yet");
    expect(container.textContent).not.toMatch(/No data/);
  });

  it("shows a row's recent mean when asked (an idle fabric link isn't 0 b/s every second)", () => {
    const ts = Array.from({ length: 60 }, (_, i) => now - 59 + i);
    // An LLDP frame every 10 s: 4230 b/s for one second, 0 the rest.
    const trickle = ts.map((_, i) => (i % 10 === 5 ? 4230 : 0));
    const { container } = render(
      <TimeChart title="Fabric received" mode="multiples" ts={ts} range={[now - 60, now]} relative valueMeanS={10} fmt={(v) => `${v.toFixed(0)} b/s`}
        series={[{ label: "gx10-0000", color: "--m-rx", values: trickle }]} />,
    );
    expect(container.querySelector(".tc-row-value")?.textContent).toBe("423 b/s");
  });
});

describe("stacked charts", () => {
  const now = Math.floor(Date.now() / 1000);

  it("read out each series' own value, not the running total it's drawn at (cell1's page cache, 2.8 GiB)", () => {
    const ts = Array.from({ length: 30 }, (_, i) => now - 29 + i);
    const { container } = render(
      <TimeChart title="Unified memory" ts={ts} range={[now - 30, now]} relative stacked yMax={122} fmt={(v) => `${v.toFixed(1)} GiB`}
        series={[
          { label: "Other", color: "--m-mem", values: ts.map(() => 4.9) },
          { label: "Cache", color: "--m-mem-cache", values: ts.map(() => 2.8) },
        ]} />,
    );
    const keys = [...container.querySelectorAll(".tc-key")].map((k) => k.textContent);
    expect(keys).toEqual(["Other4.9 GiB", "Cache2.8 GiB"]);
  });
});

describe("StopIcon", () => {
  it("is a filled square in a circle, not an empty checkbox", () => {
    const { container } = render(<StopIcon size={14} />);
    const svg = container.querySelector("svg")!;
    expect(svg.querySelector("circle")).not.toBeNull();
    expect(svg.querySelector("rect")?.getAttribute("fill")).toBe("currentColor");
  });
});
