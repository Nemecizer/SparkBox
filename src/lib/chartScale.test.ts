import { afterEach, describe, expect, it } from "vitest";
import {
  bucketMeans, byteStep, clipsAbove, excludeChip, gapRuns, gapStyle, gridLayout, isConstant, latest, movingAverage, niceRange, niceStep,
  p98Top, percentile, placeGlyphs, placeLabels, relLabel, robustTop, setHour12, smoothRate, thresholdTone, timeLabel, timeTicks, toCsv,
  valueAxis, xTickLabels, xAxis, axisLabels, degenerateAxis, flatAxis, pinnedValue, mergeRanges, badgeWidth, GLYPH_PAD, gutterLayout,
  LEADER_PX, leadingGap, thresholdLabelXs, thresholdLabelSides, thresholdText, dwellsInSteps, MIN_TICK_PX,
} from "./chartScale";

const GiB = 1024 ** 3;

describe("niceStep and niceRange", () => {
  it("rounds up to 1, 2, 2.5 or 5 × 10ⁿ", () => {
    expect([0.7, 1.3, 2.2, 3, 7, 12, 23, 240].map(niceStep)).toEqual([1, 2, 2.5, 5, 10, 20, 25, 250]);
  });

  it("ends on a labelled tick", () => {
    expect(niceRange(0, 87)).toEqual({ min: 0, max: 100, ticks: [0, 25, 50, 75, 100] });
    expect(niceRange(0, 41).ticks).toEqual([0, 20, 40, 60]);
    expect(niceRange(0, 3.2e9).ticks).toEqual([0, 1e9, 2e9, 3e9, 4e9]);
  });

  it("survives an empty or flat series", () => {
    expect(niceRange(0, 0).max).toBeGreaterThan(0);
    expect(niceRange(0, NaN).ticks.length).toBeGreaterThan(1);
  });
});

describe("value axes", () => {
  it("runs a percentage to exactly 100", () => {
    expect(valueAxis(0, 63, { bound: 100 })).toEqual({ min: 0, max: 100, ticks: [0, 25, 50, 75, 100] });
  });

  it("splits bytes in powers of two and labels the total on top", () => {
    expect(byteStep(119.7 * GiB)).toBe(32 * GiB);
    const a = valueAxis(0, 90 * GiB, { unit: "bytes", bound: 119.7 * GiB });
    expect(a.ticks.map((t) => t / GiB)).toEqual([0, 32, 64, 96, 119.7]);
    expect(valueAxis(0, 50 * GiB, { unit: "bytes" }).ticks.map((t) => t / GiB)).toEqual([0, 16, 32, 48, 64]);
  });

  it("keeps a soft floor for temperature unless the data goes lower", () => {
    const a = valueAxis(41, 63, { softMin: 20 });
    expect(a.min).toBe(20);
    expect(a.ticks[0]).toBe(20);
    expect(a.max).toBeGreaterThanOrEqual(63);
    expect(valueAxis(12, 30, { softMin: 20 }).min).toBeLessThanOrEqual(12);
  });

  it("drops a tick that would crowd the bound", () => {
    expect(valueAxis(0, 50, { bound: 140 }).ticks).toEqual([0, 50, 100, 140]);
  });
});

describe("time ticks", () => {
  afterEach(() => setHour12(undefined));

  it("puts about six ticks on round local times", () => {
    const from = Date.UTC(2026, 9, 28, 13, 0, 7) / 1000;
    const { step, ticks } = timeTicks(from, from + 900, 600);
    expect(step).toBe(120);
    expect(ticks.length).toBeGreaterThanOrEqual(3);
    expect(ticks.length).toBeLessThanOrEqual(8);
    for (const t of ticks) expect((t + -new Date(t * 1000).getTimezoneOffset() * 60) % step).toBe(0);
  });

  it("gets sparser as the chart narrows", () => {
    const from = 1_800_000_000;
    expect(timeTicks(from, from + 3600, 200).ticks.length).toBeLessThan(timeTicks(from, from + 3600, 900).ticks.length);
  });

  it("labels like a clock: seconds only for short steps, AM/PM on every 12-hour tick", () => {
    const t = new Date(2026, 9, 28, 9, 5, 30).getTime() / 1000;
    expect(timeLabel(t, 300, false, true)).toBe("9:05 AM");
    expect(timeLabel(t, 300, true, true)).toBe("9:05 AM");
    expect(timeLabel(t, 30, false, true)).toBe("9:05:30 AM");
    expect(timeLabel(t + 12 * 3600, 300, true, true)).toBe("9:05 PM");
    expect(timeLabel(t + 12 * 3600, 300, true, false)).toBe("21:05");
    const midnight = new Date(2026, 9, 28, 0, 0, 0).getTime() / 1000;
    expect(timeLabel(midnight, 6 * 3600, false, true)).toMatch(/Oct\s*28|28\s*Oct/);
  });

  it("labels short ranges relative to now", () => {
    expect(relLabel(1000, 1000)).toBe("now");
    expect(relLabel(1000 - 240, 1000)).toBe("−4m");
    expect(relLabel(1000 - 30, 1000)).toBe("−30s");
  });
});

describe("series helpers", () => {
  it("smooths with a 3-point average and keeps gaps", () => {
    expect(movingAverage([3, 6, 9, null, 3, 3])).toEqual([4.5, 6, 7.5, null, 3, 3]);
  });

  it("finds the latest real value", () => {
    expect(latest([1, 2, null, NaN])).toBe(2);
    expect(latest([])).toBeNull();
  });

  it("writes CSV with blank cells for gaps", () => {
    const csv = toCsv([0, 1], [{ label: "GPU, %", values: [5, null] }]);
    expect(csv).toBe('time,"GPU, %"\n1970-01-01T00:00:00.000Z,5\n1970-01-01T00:00:01.000Z,\n');
  });
});

describe("chart grammar (§2.2)", () => {
  it("tops a bursty axis at p98 × 1.15, never below a threshold", () => {
    const values = Array.from({ length: 100 }, (_, i) => (i === 50 ? 1000 : 10 + (i % 5)));
    expect(percentile(values, 98)).toBe(14);
    expect(p98Top([values])).toBeCloseTo(14 * 1.15);
    expect(p98Top([values], [90])).toBe(90);
    expect(clipsAbove([values], p98Top([values]))).toBe(true);
    expect(p98Top([[null, null]])).toBe(0);
    // Two series share one domain.
    expect(p98Top([[1, 2, 3], [100, 100, 100]])).toBeCloseTo(115);
  });

  it("keeps the first x label inside the left edge and the last ('now') inside the right", () => {
    const labels = xTickLabels([{ x: 100, w: 40 }, { x: 300, w: 40 }, { x: 498, w: 30 }], 100, 500);
    expect(labels[0]).toEqual({ x: 100, align: "left", show: true });
    expect(labels[1]).toEqual({ x: 300, align: "center", show: true });
    expect(labels[2]).toEqual({ x: 500, align: "right", show: true });
    // Crowded labels drop out, but "now" never does.
    const crowded = xTickLabels([{ x: 0, w: 50 }, { x: 30, w: 50 }, { x: 60, w: 50 }, { x: 95, w: 30 }], 0, 100);
    expect(crowded[3].show).toBe(true);
    expect(crowded.filter((l) => l.show).length).toBeLessThan(4);
  });

  it("stacks gutter labels so none overlaps, across 50 random cases", () => {
    let seed = 7;
    const rand = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    for (let c = 0; c < 50; c++) {
      const n = 1 + Math.floor(rand() * 6);
      const top = 0;
      const bottom = 120 + rand() * 200;
      const items = Array.from({ length: n }, () => ({ y: top + rand() * (bottom - top), h: 12 + rand() * 6 }));
      const ys = placeLabels(items, top, bottom, 2);
      const boxes = ys.map((y, i) => [y - items[i].h / 2, y + items[i].h / 2] as const).sort((a, b) => a[0] - b[0]);
      for (let i = 1; i < boxes.length; i++) expect(boxes[i][0]).toBeGreaterThanOrEqual(boxes[i - 1][1] + 2 - 1e-9);
      const total = items.reduce((a, it) => a + it.h + 2, 0);
      if (total <= bottom - top) for (const [a, b] of boxes) expect(a >= top - 1e-9 && b <= bottom + 1e-9).toBe(true);
    }
    // A label with room stays exactly where it wants to be.
    expect(placeLabels([{ y: 50, h: 12 }], 0, 100)).toEqual([50]);
  });

  it("calls a series constant when it moves less than 1% of its domain", () => {
    expect(isConstant([40, 40.5, 40.2], [0, 100])).toBe(true);
    expect(isConstant([40, 42, 40], [0, 100])).toBe(false);
    expect(isConstant([3, 3, 3])).toBe(true);
    expect(isConstant([null, null])).toBe(true);
    expect(isConstant([0, 0, 1e9])).toBe(false);
  });

  it("downsamples to bucket means and keeps gaps", () => {
    const v = Array.from({ length: 120 }, (_, i) => i);
    const b = bucketMeans(v, 26);
    expect(b.length).toBe(26);
    expect(b[0]).toBeCloseTo(1.5);
    expect(b[25]).toBeCloseTo(117);
    expect(bucketMeans([1, null, null, null, 5, 5], 3)).toEqual([1, null, 5]);
    expect(bucketMeans([1, 2], 10)).toEqual([1, 2]);
  });

  it("finds where recording stopped, not where a node was offline or the store has 10 s buckets", () => {
    expect(gapRuns([1, 2, 3, 4, 300, 301, 302])).toEqual([[3, 4]]);
    expect(gapRuns([0, 10, 20, 30, 40])).toEqual([]);
    expect(gapRuns([0, 10, 20, 21, 22, 23])).toEqual([]);
  });
});

describe("chart grammar v2 (§2.4)", () => {
  // A deterministic random stream for fixtures.
  const rng = (seed: number) => () => ((seed = (seed * 16807) % 2147483647) / 2147483647);

  it("keeps a bursty fabric line off the floor: 100 Mb/s ± 10% with 2% spikes to 6 Gb/s", () => {
    const rand = rng(11);
    const values = Array.from({ length: 900 }, () => (rand() < 0.02 ? 6e9 : 100e6 * (0.9 + 0.2 * rand())));
    values[values.length - 1] = 105e6;
    const r = robustTop([values]);
    expect(r.robust).toBe(true);
    expect(r.peak).toBe(6e9);
    const axis = valueAxis(0, r.top);
    expect(axis.max).toBeLessThanOrEqual(1e9);
    // Markers: points above the axis exist (the chart draws ▲ for them).
    expect(clipsAbove([values], axis.max)).toBe(true);
    // The live value sits at least a tenth of the way up the plot.
    expect(105e6 / axis.max).toBeGreaterThanOrEqual(0.1);
  });

  it("runs a series that isn't bursty to a nice maximum", () => {
    const rand = rng(5);
    const values = Array.from({ length: 600 }, (_, i) => 40 + 30 * Math.sin(i / 40) + 5 * rand());
    const r = robustTop([values]);
    expect(r.robust).toBe(false);
    const max = Math.max(...values);
    expect(r.top).toBe(max);
    expect(valueAxis(0, r.top)).toEqual(niceRange(0, max));
  });

  it("scales a quiet disk to its kB/s, not to a rare 80 MB/s write", () => {
    const rand = rng(3);
    const values = Array.from({ length: 900 }, (_, i) => (i % 300 === 7 ? 80e6 : 151e3 * (0.85 + 0.3 * rand())));
    const axis = valueAxis(0, robustTop([values]).top);
    expect(axis.max).toBeLessThanOrEqual(1e6);
    expect(clipsAbove([values], axis.max)).toBe(true);
  });

  it("never cuts below a threshold it draws or the current value", () => {
    const values = [...Array.from({ length: 99 }, () => 10), 1000];
    expect(robustTop([values], { thresholds: [90] }).top).toBeGreaterThanOrEqual(90);
    const live = [...Array.from({ length: 98 }, () => 10), 1000, 300];
    expect(robustTop([live]).top).toBeGreaterThanOrEqual(300 * 1.25);
    expect(robustTop([[null, null]])).toEqual({ top: 0, robust: false, peak: 0 });
  });

  it("sizes cluster pills to their text and never lets two badges overlap, across 200 random cases", () => {
    const rand = rng(29);
    for (let c = 0; c < 200; c++) {
      const n = 1 + Math.floor(rand() * 120);
      const width = 160 + rand() * 900;
      // Bunched events, as an endpoint's day of alerts and changes are.
      const items = Array.from({ length: n }, (_, i) => ({ x: rand() < 0.5 ? 40 + rand() * 30 : rand() * width, i }));
      const placed = placeGlyphs(items, badgeWidth, 6, width - 6);
      for (let k = 1; k < placed.length; k++) {
        const a = placed[k - 1];
        const b = placed[k];
        const edgeGap = b.x - a.x - (badgeWidth(a.members.length) + badgeWidth(b.members.length)) / 2;
        expect(edgeGap).toBeGreaterThanOrEqual(GLYPH_PAD - 1e-9);
      }
      expect(placed.reduce((a, p) => a + p.members.length, 0)).toBe(n);
    }
    // A "12" pill is wider than a "2" one.
    expect(badgeWidth(12)).toBeGreaterThan(badgeWidth(2));
    expect(badgeWidth(150)).toBe(26);
  });

  it("clusters glyphs so no two are closer than 12 px, across 50 random cases", () => {
    const rand = rng(17);
    for (let c = 0; c < 50; c++) {
      const n = 1 + Math.floor(rand() * 25);
      const width = 200 + rand() * 600;
      const items = Array.from({ length: n }, (_, i) => ({ x: rand() < 0.3 ? 40 + rand() * 20 : rand() * width, i }));
      const placed = placeGlyphs(items, 12, 0, width);
      for (let k = 1; k < placed.length; k++) expect(placed[k].x - placed[k - 1].x).toBeGreaterThanOrEqual(12 - 1e-9);
      // Every glyph lands in exactly one cluster.
      expect(placed.flatMap((p) => p.members.map((m) => m.i)).sort((a, b) => a - b)).toEqual(items.map((it) => it.i).sort((a, b) => a - b));
      for (const p of placed) expect(p.x >= 0 && p.x <= width).toBe(true);
    }
    // Five alerts in one spot become one numbered glyph.
    expect(placeGlyphs([{ x: 100 }, { x: 102 }, { x: 103 }, { x: 105 }, { x: 108 }]).map((p) => p.members.length)).toEqual([5]);
  });

  it("keeps glyphs off the History chip and 4 px around it", () => {
    const glyphs = [{ x: 10 }, { x: 46 }, { x: 60 }, { x: 140 }, { x: 154 }, { x: 155 }];
    expect(excludeChip(glyphs, { left: 50, right: 150 }).map((g) => g.x)).toEqual([10, 155]);
    expect(excludeChip(glyphs, null)).toHaveLength(6);
  });

  it("draws gaps by width: a hairline, a band, then a labelled band", () => {
    expect(gapStyle(3)).toBe("hairline");
    expect(gapStyle(8)).toBe("band");
    expect(gapStyle(59)).toBe("band");
    expect(gapStyle(60)).toBe("label");
  });

  it("draws a gap under a minute as a hairline, and labels only one of 2 minutes and 60 px", () => {
    // A quick restart is a hairline however wide the chart draws it.
    expect(gapStyle(120, 45)).toBe("hairline");
    expect(gapStyle(120, 90)).toBe("band");
    expect(gapStyle(59, 600)).toBe("band");
    expect(gapStyle(60, 120)).toBe("label");
  });

  it("merges overlapping gaps so each is labelled once", () => {
    expect(mergeRanges([[10, 40], [35, 60], [100, 120], [118, 130], [200, 210]])).toEqual([[10, 60], [100, 130], [200, 210]]);
    expect(mergeRanges([[10, 20], [21, 30]], 1)).toEqual([[10, 30]]);
  });

  it("keeps a threshold neutral until the series reaches it in the window", () => {
    const th = { value: 90, tone: "warn" as const };
    expect(thresholdTone(th, [[40, 60, 85]])).toBe("neutral");
    expect(thresholdTone(th, [[40, 91, 85]])).toBe("warn");
    expect(thresholdTone({ value: 95, tone: "crit" as const }, [[96]])).toBe("crit");
    expect(thresholdTone({ value: -70, tone: "warn" as const, below: true }, [[-60, -72]])).toBe("warn");
    expect(thresholdTone({ value: 5, tone: "neutral" as const }, [[9]])).toBe("neutral");
  });

  it("smooths a square-wave rate so no step is taller than 35% of the sparkline (FC1)", () => {
    const height = 18;
    const square = Array.from({ length: 90 }, (_, i) => (Math.floor(i / 15) % 2 ? 100 : 0));
    const pts = smoothRate(square, 44);
    const ys = pts.filter((v): v is number => v != null).map((v) => (v / 100) * height);
    let worst = 0;
    for (let i = 1; i < ys.length; i++) worst = Math.max(worst, Math.abs(ys[i] - ys[i - 1]));
    expect(worst).toBeLessThanOrEqual(0.35 * height);
    // Unsmoothed, the same wave jumps the full height.
    const raw = bucketMeans(square, 22).map((v) => ((v ?? 0) / 100) * height);
    expect(Math.max(...raw.slice(1).map((y, i) => Math.abs(y - raw[i])))).toBeGreaterThan(0.35 * height);
  });

  it("gives every chart in a grid the same insets and ticks", () => {
    const span: [number, number] = [1_800_000_000, 1_800_000_900];
    const a = gridLayout([{ left: 38, right: 8 }, { left: 52, right: 64 }], span, 600);
    const b = gridLayout([{ left: 52, right: 64 }, { left: 38, right: 8 }], span, 600);
    expect(a).toEqual(b);
    expect(a.left).toBe(52);
    expect(a.right).toBe(64);
    expect(a.ticks.length).toBeGreaterThan(2);
  });
});

describe("C1 axes", () => {
  afterEach(() => setHour12(undefined));
  const rng = (seed: number) => () => ((seed = (seed * 16807) % 2147483647) / 2147483647);

  it("never repeats or mixes units on a 15-minute chart holding 6 minutes (the Live tour's −3m −3m −90s)", () => {
    const now = 1_793_000_000;
    // The window collapses to the data it has: 6 minutes, at the widths the Dashboard draws.
    for (const width of [180, 240, 320, 420, 520, 640, 900]) {
      for (const span of [[now - 360, now], [now - 900, now], [now - 150, now], [now - 300, now]] as [number, number][]) {
        const { labels, ticks } = xAxis(span, width, true);
        expect(new Set(labels).size, labels.join(" ")).toBe(labels.length);
        const units = new Set(labels.filter((l) => l !== "now").map((l) => l.slice(-1)));
        expect(units.size, labels.join(" ")).toBeLessThanOrEqual(1);
        // Labels at least the minimum spacing apart.
        const px = width / (span[1] - span[0]);
        for (let i = 1; i < ticks.length; i++) expect((ticks[i] - ticks[i - 1]) * px).toBeGreaterThanOrEqual(MIN_TICK_PX.relative - 1e-6);
      }
    }
    expect(xAxis([now - 360, now], 420, true).labels).toEqual(["−6m", "−5m", "−4m", "−3m", "−2m", "−1m", "now"]);
    expect(xAxis([now - 90, now], 300, true).labels).toEqual(["−90s", "−60s", "−30s", "now"]);
  });

  it("puts AM/PM on every clock tick, and never two the same", () => {
    const from = new Date(2026, 9, 29, 6, 0, 0).getTime() / 1000;
    const { labels } = xAxis([from, from + 3600], 600, false, true);
    expect(labels.length).toBeGreaterThan(2);
    for (const l of labels) expect(l).toMatch(/ (AM|PM)$/);
    expect(new Set(labels).size).toBe(labels.length);
    const h24 = xAxis([from, from + 3600], 600, false, false).labels;
    for (const l of h24) expect(l).not.toMatch(/AM|PM/);
  });

  it("gives tick labels the decimals their step needs, in the top tick's unit", () => {
    const bitsLabel = (v: number) => (v < 1000 ? `${v.toFixed(0)} b/s` : `${(v / 1000).toFixed(1)} kb/s`);
    const tps = (v: number) => `${v >= 10 ? v.toFixed(1) : v.toFixed(2)} tok/s`;
    expect(axisLabels([0, 5, 10, 15, 20], tps)).toEqual(["0 tok/s", "5 tok/s", "10 tok/s", "15 tok/s", "20 tok/s"]);
    // The foot is a plain "0" whatever the step's decimals (the Live tour's "0.00 kb/s" under "0.25 kb/s").
    expect(axisLabels([0, 2.5, 5, 7.5, 10], tps)).toEqual(["0 tok/s", "2.5 tok/s", "5.0 tok/s", "7.5 tok/s", "10.0 tok/s"]);
    expect(axisLabels([0, 250, 500, 750, 1000], bitsLabel)).toEqual(["0 kb/s", "0.25 kb/s", "0.50 kb/s", "0.75 kb/s", "1.00 kb/s"]);
    // The zero tick is in the axis unit: "0 kb/s", not "0 b/s".
    const bits = (v: number) => (v < 1 ? "0 b/s" : v < 1000 ? `${v.toFixed(0)} b/s` : `${(v / 1000).toFixed(1)} kb/s`);
    expect(axisLabels([0, 1000, 2000, 3000, 4000], bits)).toEqual(["0 kb/s", "1 kb/s", "2 kb/s", "3 kb/s", "4 kb/s"]);
    const pct = (v: number) => `${v.toFixed(0)}%`;
    expect(axisLabels([0, 25, 50, 75, 100], pct)).toEqual(["0%", "25%", "50%", "75%", "100%"]);
  });

  it("keeps whole ticks under a bound that isn't on the grid (a node's 121.7 GiB of memory)", () => {
    const gib = (v: number) => `${(v / GiB).toFixed(v / GiB >= 100 ? 0 : 1)} GiB`;
    const total = 121.7 * GiB;
    const axis = valueAxis(0, 5 * GiB, { unit: "bytes", bound: total });
    expect(axisLabels(axis.ticks, gib)).toEqual(["0 GiB", "32 GiB", "64 GiB", "96 GiB", "122 GiB"]);
  });

  it("never draws \"0.00 tok/s\" five times: an empty chart has no axis, a flat one one tick", () => {
    const tps = (v: number) => `${v.toFixed(2)} tok/s`;
    expect(degenerateAxis([[null, null, null]], false)).toEqual({ kind: "empty" });
    expect(degenerateAxis([[0, 0, 0, 0]], false)).toEqual({ kind: "flat", value: 0 });
    expect(degenerateAxis([[12.5, 12.5]], false)).toEqual({ kind: "flat", value: 12.5 });
    expect(degenerateAxis([[0, 0]], true)).toBeNull();
    expect(degenerateAxis([[1, 2]], false)).toBeNull();
    const flat = flatAxis(0);
    expect(flat.ticks).toEqual([0]);
    expect(axisLabels(flat.ticks, tps)).toEqual(["0.00 tok/s"]);
    // Even a hand-made degenerate axis never repeats a label.
    const labels = axisLabels([0, 1e-9, 2e-9, 3e-9, 4e-9], tps).filter(Boolean);
    expect(new Set(labels).size).toBe(labels.length);
    expect(flatAxis(18).ticks).toEqual([18]);
    expect(flatAxis(18).max).toBeGreaterThan(18);
  });

  it("pins nothing for a GPU that stopped answering: a trailing null or a reading older than 2 steps", () => {
    const ts = [100, 101, 102, 103, 104];
    expect(pinnedValue(ts, [64, 64, 64, 64, 64], 104, 2)).toEqual({ value: 64, since: 104 });
    expect(pinnedValue(ts, [64, 64, 64, null, null], 104, 2)).toEqual({ value: null, since: 102 });
    expect(pinnedValue(ts, [64, 64, 64, 64, 64], 110, 2)).toEqual({ value: null, since: 104 });
    expect(pinnedValue(ts, [null, null, null, null, null], 104, 2)).toEqual({ value: null, since: null });
  });

  it("keeps the right gutter for live values, with a leader for any label moved more than 4 px", () => {
    const rand = rng(5);
    for (let c = 0; c < 100; c++) {
      const n = 1 + Math.floor(rand() * 6);
      const pins = Array.from({ length: n }, () => ({ y: 10 + rand() * 120, h: 15 }));
      const placed = gutterLayout(pins, 0, 150, 2);
      placed.forEach((p, i) => {
        const moved = Math.abs(p.y - pins[i].y);
        // No label more than 6 px from its anchor without a leader.
        if (moved > 6) expect(p.leader).toBe(true);
        if (moved <= LEADER_PX) expect(p.leader).toBe(false);
      });
      const ys = placed.map((p) => p.y).sort((a, b) => a - b);
      for (let k = 1; k < ys.length; k++) expect(ys[k] - ys[k - 1]).toBeGreaterThanOrEqual(15 - 1e-6);
    }
  });

  it("calls the minutes before this run's first reading closed only when history began before the window", () => {
    const from = 1000;
    const ts = [1700, 1701, 1702];
    // History began this morning: 11 minutes of the window were SparkBox closed.
    expect(leadingGap(ts, from, 0)).toEqual([1000, 1700]);
    // History began inside the window: that's "History starts", not a gap.
    expect(leadingGap(ts, from, 1650)).toBeNull();
    // The window starts with readings, or there are none at all.
    expect(leadingGap([1002, 1003], from, 0)).toBeNull();
    expect(leadingGap([], from, 0)).toBeNull();
    expect(leadingGap(ts, from, null)).toBeNull();
  });

  it("moves a threshold label right of one it would overlap, and leaves apart ones at the left edge", () => {
    // 85 °C and 90 °C on a short plot: their labels' spans overlap, so the second steps right.
    const xs = thresholdLabelXs([{ top: 10, bottom: 21, w: 30 }, { top: 16, bottom: 27, w: 30 }, { top: 60, bottom: 71, w: 30 }], 3, 6);
    expect(xs).toEqual([3, 39, 3]);
    const rand = rng(11);
    for (let c = 0; c < 100; c++) {
      const labels = Array.from({ length: 1 + Math.floor(rand() * 4) }, () => {
        const top = rand() * 100;
        return { top, bottom: top + 11, w: 20 + rand() * 20 };
      });
      const at = thresholdLabelXs(labels, 0, 4);
      for (let i = 0; i < labels.length; i++) for (let j = 0; j < i; j++) {
        const [a, b] = [labels[i], labels[j]];
        const overlapY = a.top < b.bottom && a.bottom > b.top;
        const overlapX = at[i] < at[j] + b.w && at[i] + a.w > at[j];
        expect(overlapY && overlapX).toBe(false);
      }
    }
  });

  it("centres the chips of thresholds closer than two chip heights on their own lines; a lone chip sits above", () => {
    // 90% and 95% six pixels apart at the top of a 120 px plot, chips 10.5 px tall.
    const ys = [12.5, 6.5];
    const sides = thresholdLabelSides(ys, 10.5, 2, 0, 120);
    expect(sides.map((s) => s.side)).toEqual(["on", "on"]);
    // Every chip's centre is nearer its own line than the other threshold's.
    sides.forEach((s, i) => ys.forEach((y, j) => { if (j !== i) expect(Math.abs(s.cy - ys[i])).toBeLessThan(Math.abs(s.cy - y)); }));
    // A chip never leaves the plot: the 95% chip's top is clamped to it.
    expect(sides[1].cy - 10.5 / 2).toBeGreaterThanOrEqual(0);
    // Alone, a label sits above its line, or below one that runs along the top.
    expect(thresholdLabelSides([60], 10.5, 2, 0, 120)).toEqual([{ side: "above", cy: 60 - 2 - 5.25 }]);
    expect(thresholdLabelSides([6], 10.5, 2, 0, 120)).toEqual([{ side: "below", cy: 6 + 2 + 5.25 }]);
    // 85 °C and 90 °C are a cluster; a 70 °C line far below them is not.
    expect(thresholdLabelSides([30, 36, 90], 10.5, 2, 0, 120).map((s) => s.side)).toEqual(["on", "on", "above"]);
  });

  it("labels thresholds in one format", () => {
    expect(thresholdText("90% used")).toBe("90%");
    expect(thresholdText("95%")).toBe("95%");
    expect(thresholdText("85 °C")).toBe("85 °C");
    expect(thresholdText("Limit 140 W")).toBe("Limit 140 W");
  });

  it("finds steps by dwell: a noisy square wave is step-like, a noisy ramp and a sine aren't", () => {
    const rand = rng(3);
    const square = Array.from({ length: 80 }, (_, i) => (Math.floor(i / 8) % 2 ? 50 : 10) + (rand() - 0.5) * 1.2);
    expect(dwellsInSteps(square)).toBe(true);
    const ramp = Array.from({ length: 80 }, (_, i) => i + (rand() - 0.5));
    expect(dwellsInSteps(ramp)).toBe(false);
    const sine = Array.from({ length: 80 }, (_, i) => 30 + 20 * Math.sin(i / 3));
    expect(dwellsInSteps(sine)).toBe(false);
    expect(dwellsInSteps([5, 5, 5, 5, 5, 5])).toBe(false);
  });
});
