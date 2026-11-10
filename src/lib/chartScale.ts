// Axis maths for TimeChart, kept free of the DOM so it can be tested: nice
// value ticks (1/2/2.5/5 × 10ⁿ, or powers of two for bytes), time ticks
// aligned to the local clock, and the labels that go with them.

export type AxisUnit = "pct" | "bytes" | "number";

const NICE = [1, 2, 2.5, 5, 10];

/** The smallest 1, 2, 2.5 or 5 × 10ⁿ that is at least `raw`. */
export function niceStep(raw: number): number {
  if (!(raw > 0) || !Number.isFinite(raw)) return 1;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const f = raw / mag;
  const m = NICE.find((n) => f <= n + 1e-9) ?? 10;
  return m * mag;
}

export interface ValueAxis {
  min: number;
  max: number;
  /** Tick values from min to max; the last one is the top of the axis, so it's always labelled. */
  ticks: number[];
}

/**
 * A 0-based axis that ends on a tick: [0, 87] becomes 0, 25, 50, 75, 100. `target` is how
 * many intervals to aim for (about 4 in a 140–320 px chart).
 */
export function niceRange(min: number, max: number, target = 4): ValueAxis {
  const lo = Math.min(0, Number.isFinite(min) ? min : 0);
  const hi = Number.isFinite(max) && max > lo ? max : lo + 1;
  const step = niceStep((hi - lo) / target);
  const top = Math.ceil(hi / step - 1e-9) * step;
  const bottom = Math.floor(lo / step + 1e-9) * step;
  const ticks: number[] = [];
  for (let v = bottom; v <= top + step * 1e-6; v += step) ticks.push(Math.round(v / step) * step);
  return { min: bottom, max: top, ticks };
}

/** Power-of-two steps for bytes: 0, 32, 64, 96, 128 GiB rather than 0, 25, 50 GiB. */
export function byteStep(max: number, target = 4): number {
  if (!(max > 0)) return 1;
  let step = 2 ** Math.floor(Math.log2(max / target));
  while (max / step > target + 1) step *= 2;
  return step;
}

/**
 * The value axis for a chart. A bounded metric (`bound`: 100 for percentages, the total for
 * memory, the link speed for a port) runs exactly to its bound, which is labelled; otherwise
 * the top rounds up to a nice tick. `softMin` (temperature: 20 °C) is a floor the axis starts
 * at unless the data goes lower.
 */
export function valueAxis(dataMin: number, dataMax: number, opts: { unit?: AxisUnit; bound?: number; softMin?: number; target?: number } = {}): ValueAxis {
  const target = opts.target ?? 4;
  let lo = 0;
  if (opts.softMin !== undefined) lo = Math.min(opts.softMin, Number.isFinite(dataMin) ? dataMin : opts.softMin);
  if (opts.unit === "bytes") {
    const top = opts.bound ?? dataMax;
    const step = byteStep(Math.max(top, 1), target);
    if (opts.bound !== undefined) return { min: 0, max: opts.bound, ticks: boundedTicks(0, opts.bound, step) };
    const max = Math.max(step, Math.ceil(Math.max(dataMax, 1) / step - 1e-9) * step);
    const ticks: number[] = [];
    for (let v = 0; v <= max + step * 1e-6; v += step) ticks.push(v);
    return { min: 0, max, ticks };
  }
  if (opts.bound !== undefined) {
    const step = niceStep((opts.bound - lo) / target);
    const first = Math.ceil(lo / step - 1e-9) * step || 0;
    return { min: lo, max: opts.bound, ticks: boundedTicks(first, opts.bound, step, lo) };
  }
  const axis = niceRange(lo, Math.max(dataMax, lo + 1e-9), target);
  if (opts.softMin !== undefined) {
    const step = axis.ticks.length > 1 ? axis.ticks[1] - axis.ticks[0] : 1;
    const bottom = Math.floor(lo / step) * step;
    const ticks: number[] = [];
    for (let v = bottom; v <= axis.max + step * 1e-6; v += step) ticks.push(Math.round(v / step) * step);
    return { min: bottom, max: axis.max, ticks };
  }
  return axis;
}

/** Regular ticks up to a bound, ending exactly on the bound; drops a tick that would crowd it. */
function boundedTicks(first: number, bound: number, step: number, floor?: number): number[] {
  const ticks: number[] = [];
  if (floor !== undefined && floor < first - step * 1e-6) ticks.push(floor);
  for (let v = first; v < bound - step * 0.4; v += step) ticks.push(v);
  ticks.push(bound);
  return ticks;
}

// ---- Time

const SECOND_STEPS = [1, 2, 5, 10, 15, 30];
const MINUTE_STEPS = [1, 2, 5, 10, 15, 30].map((m) => m * 60);
const HOUR_STEPS = [1, 2, 3, 6, 12].map((h) => h * 3600);
const DAY_STEPS = [1, 2, 7].map((d) => d * 86400);
const TIME_STEPS = [...SECOND_STEPS, ...MINUTE_STEPS, ...HOUR_STEPS, ...DAY_STEPS];

/** The local clock's offset from UTC at a moment, in seconds (east positive). */
function offsetS(t: number): number {
  return -new Date(t * 1000).getTimezoneOffset() * 60;
}

/** The narrowest gap between x labels' centres: room for "10:45 AM" or "−10m" and a space. */
export const MIN_TICK_PX = { clock12: 76, clock24: 58, relative: 52 } as const;

/**
 * Ticks for [from, to] (unix seconds) on round local times (9:00, 9:05, 9:10…): the finest
 * step whose labels stay at least `minSpacingPx` apart in `widthPx`, at most 8 of them.
 */
export function timeTicks(fromS: number, toS: number, widthPx: number, minSpacingPx: number = MIN_TICK_PX.clock24): { step: number; ticks: number[] } {
  const span = Math.max(1, toS - fromS);
  const most = Math.max(1, Math.min(8, Math.floor(Math.max(1, widthPx) / Math.max(1, minSpacingPx))));
  const step = TIME_STEPS.find((s) => span / s <= most) ?? TIME_STEPS[TIME_STEPS.length - 1];
  const ticks: number[] = [];
  const off = offsetS(fromS);
  let t = Math.ceil((fromS + off) / step) * step - off;
  for (; t <= toS + 1e-6; t += step) ticks.push(t);
  return { step, ticks };
}

let hour12: boolean | undefined;

/** Whether this Mac shows 12-hour times (from the locale), for "9:05" vs "09:05". */
export function uses12Hour(): boolean {
  if (hour12 === undefined) {
    try {
      hour12 = new Intl.DateTimeFormat(undefined, { hour: "numeric" }).resolvedOptions().hour12 ?? false;
    } catch {
      hour12 = false;
    }
  }
  return hour12;
}

/** Overrides the 12/24-hour choice (tests). */
export function setHour12(v: boolean | undefined) {
  hour12 = v;
}

/**
 * "9:05 AM", "9:05:30 AM", "21:05" or "Oct 27" for whole days. On a 12-hour Mac every clock
 * label says AM or PM, so no tick is ambiguous (`first` is kept for callers; it changes nothing).
 */
export function timeLabel(t: number, step: number, first: boolean, h12: boolean = uses12Hour()): string {
  void first;
  const d = new Date(t * 1000);
  const local = t + offsetS(t);
  if (step >= 86400 || (step >= 3600 && local % 86400 === 0)) {
    return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  }
  const h = d.getHours();
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  const hour = h12 ? ((h + 11) % 12) + 1 : h;
  const hh = h12 ? String(hour) : String(hour).padStart(2, "0");
  let s = `${hh}:${mm}`;
  if (step < 60) s += `:${ss}`;
  if (h12) s += h < 12 ? " AM" : " PM";
  return s;
}

/**
 * "−4m", "−30s", "now": time before the right edge, for ranges of 15 minutes or less. One
 * unit per axis: with a `step` of whole minutes every label is in minutes, otherwise every
 * label is in seconds ("−90s −60s −30s now"), never "−3m −3m −90s".
 */
export function relLabel(t: number, nowS: number, step?: number): string {
  const ago = Math.round(nowS - t);
  if (ago <= 0) return "now";
  const minutes = step !== undefined ? step >= 60 && step % 60 === 0 : ago % 60 === 0;
  return minutes ? `−${Math.round(ago / 60)}m` : `−${ago}s`;
}

/**
 * The x axis for [from, to]: ticks and their labels, at least `minSpacingPx` apart. Relative
 * ones count back from "now" at the right edge in one unit; clock ones are on round local
 * times. No two labels are the same.
 */
export function xAxis(span: [number, number], widthPx: number, relative: boolean, h12: boolean = uses12Hour()): { ticks: number[]; labels: string[]; step: number } {
  const [from, to] = span;
  const min = relative ? MIN_TICK_PX.relative : h12 ? MIN_TICK_PX.clock12 : MIN_TICK_PX.clock24;
  const t = timeTicks(from, to, widthPx, min);
  if (relative) {
    // Seconds steps only up to 30 s; a minute or more is in whole minutes.
    const step = t.step < 60 ? t.step : Math.max(60, Math.round(t.step / 60) * 60);
    const ticks: number[] = [];
    for (let x = to; x >= from - 1e-6; x -= step) ticks.unshift(x);
    return { ticks, labels: ticks.map((x) => relLabel(x, to, step)), step };
  }
  const labels = t.ticks.map((x) => timeLabel(x, t.step, false, h12));
  // A day boundary can repeat a date label ("Oct 27" twice at 12 h steps): keep the first.
  const seen = new Set<string>();
  const keep = labels.map((l) => (seen.has(l) ? false : (seen.add(l), true)));
  return { ticks: t.ticks.filter((_, i) => keep[i]), labels: labels.filter((_, i) => keep[i]), step: t.step };
}

/** A full readout time for the hover header: "9:04:12" or "Oct 27, 9:04". */
export function hoverTime(t: number, spanS: number, h12: boolean = uses12Hour()): string {
  const d = new Date(t * 1000);
  const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", second: spanS <= 6 * 3600 ? "2-digit" : undefined, hour12: h12 });
  if (spanS <= 86400) return time;
  return `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${time}`;
}

// ---- Value labels (C1): decimals follow the step, one unit per axis

/** Decimals a tick step needs: 5 → 0, 2.5 → 1, 0.25 → 2, 0.05 → 2. */
export function stepDecimals(step: number): number {
  if (!(step > 0) || !Number.isFinite(step)) return 0;
  for (let d = 0; d <= 6; d++) {
    const scaled = step * 10 ** d;
    if (Math.abs(scaled - Math.round(scaled)) < 1e-6 * Math.max(1, scaled)) return d;
  }
  return 6;
}

const NUM_RE = /^([-−]?)([\d,]*\.?\d+)(\s?)(.*)$/;

/**
 * Labels for an axis's ticks in the unit of its top tick, with the decimals the step needs:
 * "0, 5, 10, 15, 20 tok/s" (never "5.00"), and "0 kb/s" for the zero tick of a kb/s axis.
 * `fmt` is the chart's own formatter; when its output isn't "<number><unit>" the labels are
 * fmt's own. Labels that would still repeat (a degenerate axis) keep only the first.
 */
export function axisLabels(ticks: number[], fmt: (v: number) => string): string[] {
  const raw = ticks.map((v) => fmt(v));
  if (ticks.length < 2) return raw;
  const top = ticks.reduce((a, b) => (Math.abs(b) > Math.abs(a) ? b : a), 0);
  const m = NUM_RE.exec(fmt(top).trim());
  const topNum = m ? Number(m[2].replace(/,/g, "")) * (m[1] ? -1 : 1) : NaN;
  if (!m || !(top !== 0) || !Number.isFinite(topNum) || topNum === 0) return dedupe(raw);
  const [, , , space, unit] = m;
  const steps = ticks.slice(1).map((v, i) => Math.abs(v - ticks[i])).filter((d) => d > 0);
  // The grid's step is the one most ticks keep: a bound off the grid (memory's 121.7 GiB after
  // 96 GiB) or a floor below it makes one short step at an end.
  const same = (a: number, b: number) => Math.abs(a - b) <= 1e-6 * Math.max(a, b);
  const minStep = steps.reduce((best, d) => {
    const n = steps.filter((e) => same(e, d)).length;
    const nb = steps.filter((e) => same(e, best)).length;
    return n > nb || (n === nb && d > best) ? d : best;
  }, steps[0] ?? 0);
  // The unit's scale from the largest tick on that grid: the bound, printed "122 GiB", would
  // skew it and read "32.0821 GiB" a tick.
  const onGrid = minStep > 0 ? ticks.filter((v) => v !== 0 && Math.abs((v - ticks[0]) / minStep - Math.round((v - ticks[0]) / minStep)) < 1e-6) : [];
  const ref = onGrid.reduce((a, b) => (Math.abs(b) > Math.abs(a) ? b : a), 0);
  const rm = ref !== 0 && ref !== top ? NUM_RE.exec(fmt(ref).trim()) : null;
  const refNum = rm && rm[4] === unit ? Number(rm[2].replace(/,/g, "")) * (rm[1] ? -1 : 1) : NaN;
  const scale = Number.isFinite(refNum) && refNum !== 0 ? refNum / ref : topNum / top;
  const step = minStep * Math.abs(scale);
  const d = stepDecimals(Number(step.toPrecision(6)));
  const out = ticks.map((v) => {
    const x = v * scale;
    const n = Math.abs(x) >= 10_000 ? x.toLocaleString("en-US", { minimumFractionDigits: d, maximumFractionDigits: d }) : x.toFixed(d);
    // The foot of the axis is plain "0 kb/s" whatever the step's decimals ("0.25 kb/s" above it).
    if (Math.abs(x) < 1e-9) return `0${space}${unit}`;
    return `${n.replace(/^-/, "−")}${space}${unit}`;
  });
  return dedupe(out);
}

/** Blanks a label that repeats the one before it (the tick stays, its label doesn't). */
function dedupe(labels: string[]): string[] {
  return labels.map((l, i) => (labels.slice(0, i).includes(l) ? "" : l));
}

/**
 * A value axis that says something (C1): none for a chart with no readings; for a series that
 * never moves (a constant, or all zero) one tick at its value instead of five that read
 * "0.00 tok/s"; otherwise `axis` as given. `flat` is the constant value, when there is one.
 */
export function degenerateAxis(values: ArrayLike<number | null | undefined>[], bounded: boolean): { kind: "empty" } | { kind: "flat"; value: number } | null {
  let lo = Infinity;
  let hi = -Infinity;
  for (const s of values) for (let i = 0; i < s.length; i++) {
    const v = s[i];
    if (v == null || !Number.isFinite(v)) continue;
    lo = Math.min(lo, v);
    hi = Math.max(hi, v);
  }
  if (!(hi >= lo)) return { kind: "empty" };
  if (bounded) return null;
  return hi - lo <= Math.max(Math.abs(hi), Math.abs(lo)) * 1e-6 ? { kind: "flat", value: hi } : null;
}

/** The axis for a constant series: its value is the one tick, sitting in the middle (or at the floor for 0). */
export function flatAxis(value: number): ValueAxis {
  if (value === 0) return { min: 0, max: 1, ticks: [0] };
  if (value > 0) return { min: 0, max: value * 2, ticks: [value] };
  return { min: value * 2, max: 0, ticks: [value] };
}

// ---- Series helpers

/** A centred moving average over `n` points that keeps gaps as gaps. */
export function movingAverage(values: (number | null)[], n = 3): (number | null)[] {
  const half = Math.floor(n / 2);
  return values.map((v, i) => {
    if (v == null) return null;
    let sum = 0;
    let count = 0;
    for (let j = i - half; j <= i + half; j++) {
      const x = values[j];
      if (x != null) {
        sum += x;
        count++;
      }
    }
    return sum / count;
  });
}

/**
 * The value a chart pins at its right edge (C1): the newest reading, unless the newest point is
 * empty or the newest reading is more than `maxAgeS` older than `nowS` (a GPU that stopped
 * answering). Then the value is null and `since` says when the last reading was.
 */
export function pinnedValue(ts: ArrayLike<number>, values: ArrayLike<number | null | undefined>, nowS: number, maxAgeS: number): { value: number | null; since: number | null } {
  for (let i = values.length - 1; i >= 0; i--) {
    const v = values[i];
    if (v == null || Number.isNaN(v)) continue;
    const stale = i < values.length - 1 && values[values.length - 1] == null;
    return stale || nowS - ts[i] > maxAgeS ? { value: null, since: ts[i] } : { value: v, since: ts[i] };
  }
  return { value: null, since: null };
}

/** The last non-null value, or null. */
export function latest(values: ArrayLike<number | null | undefined>): number | null {
  for (let i = values.length - 1; i >= 0; i--) {
    const v = values[i];
    if (v != null && !Number.isNaN(v)) return v;
  }
  return null;
}

/** A chart's data as CSV: an ISO time column, then one column per series. */
export function toCsv(ts: number[], series: { label: string; values: (number | null)[] }[]): string {
  const esc = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);
  const head = ["time", ...series.map((s) => esc(s.label))].join(",");
  const rows = ts.map((t, i) => [new Date(t * 1000).toISOString(), ...series.map((s) => (s.values[i] == null ? "" : String(s.values[i])))].join(","));
  return [head, ...rows].join("\n") + "\n";
}

/** Range choices for charts. Ranges of 6 h and more come from the Rust history store. */
export const RANGES = [
  { id: "5m", label: "5m", seconds: 300 },
  { id: "15m", label: "15m", seconds: 900 },
  { id: "1h", label: "1h", seconds: 3600 },
  { id: "6h", label: "6h", seconds: 6 * 3600 },
  { id: "24h", label: "24h", seconds: 86400 },
  { id: "7d", label: "7d", seconds: 7 * 86400 },
] as const;
export type RangeId = (typeof RANGES)[number]["id"];
export const rangeSeconds = (id: string) => RANGES.find((r) => r.id === id)?.seconds ?? 900;
/** Longer than the in-memory hour: ask the history store. */
export const usesStore = (seconds: number) => seconds >= 6 * 3600;

// ---- Chart grammar (§2.2): clamping, edge labels, gutter chips, trends

/** The `p`th percentile (0–100, nearest rank) of the finite values, or null with none. */
export function percentile(values: ArrayLike<number | null | undefined>, p: number): number | null {
  const xs: number[] = [];
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (v != null && Number.isFinite(v)) xs.push(v);
  }
  if (!xs.length) return null;
  xs.sort((a, b) => a - b);
  const rank = Math.ceil((Math.max(0, Math.min(100, p)) / 100) * xs.length);
  return xs[Math.max(0, rank - 1)];
}

/**
 * The top of a bursty chart's value axis: the 98th percentile of every series × 1.15, and
 * never below a threshold the chart draws (a 90% line stays on screen). One outlier no
 * longer flattens the rest of the chart; points above the top are drawn at the edge.
 */
export function p98Top(series: ArrayLike<number | null | undefined>[], thresholds: number[] = []): number {
  const all: (number | null | undefined)[] = [];
  for (const s of series) for (let i = 0; i < s.length; i++) all.push(s[i]);
  const p = percentile(all, 98) ?? 0;
  return Math.max(p * 1.15, ...thresholds, 0);
}

/** Whether any value rises above `top` (the chart then marks those points with ▲). */
export function clipsAbove(series: ArrayLike<number | null | undefined>[], top: number): boolean {
  for (const s of series) for (let i = 0; i < s.length; i++) if ((s[i] ?? -Infinity) > top) return true;
  return false;
}

export interface TickLabel {
  /** Where the label's anchor goes and which side of the text it is. */
  x: number;
  align: "left" | "center" | "right";
  show: boolean;
}

/**
 * Places x-axis labels so none is clipped: each is centred on its tick, then pulled inside
 * [left, right] (so the first reads left-aligned at the plot's edge and the last, "now",
 * right-aligned), and a label that would touch its neighbour is dropped. The last label is
 * always kept, and the first unless it collides with the last.
 */
export function xTickLabels(ticks: { x: number; w: number }[], left: number, right: number, gap = 6): TickLabel[] {
  const boxes = ticks.map(({ x, w }) => {
    let a = x - w / 2;
    if (a < left) a = left;
    if (a + w > right) a = right - w;
    return { a, b: a + w, x };
  });
  const show = boxes.map(() => true);
  const n = boxes.length;
  // Keep the last, then walk left to right keeping what fits before it.
  let lastEnd = -Infinity;
  for (let i = 0; i < n; i++) {
    const end = i === n - 1 ? Infinity : boxes[n - 1].a - gap;
    show[i] = i === n - 1 || (boxes[i].a >= lastEnd + gap && boxes[i].b <= end);
    if (show[i]) lastEnd = boxes[i].b;
  }
  return boxes.map((bx, i) => {
    const w = bx.b - bx.a;
    const align = bx.a <= left + 0.5 && bx.x - w / 2 < left ? "left" : bx.b >= right - 0.5 && bx.x + w / 2 > right ? "right" : "center";
    return { x: align === "left" ? bx.a : align === "right" ? bx.b : bx.x, align, show: show[i] };
  });
}

/**
 * Stacks labels along one axis so they never overlap: each wants to sit at `y` (its
 * centre) and is `h` tall; the result keeps the order, moves each as little as it can and
 * stays inside [top, bottom]. Used for threshold chips in the right gutter.
 */
export function placeLabels(items: { y: number; h: number }[], top: number, bottom: number, gap = 2): number[] {
  const order = items.map((it, i) => ({ ...it, i })).sort((a, b) => a.y - b.y || a.i - b.i);
  const out = new Array<number>(items.length);
  // Down pass: nothing above its lower neighbour's top.
  let floor = top;
  const ys = order.map((it) => {
    const y = Math.max(it.y, floor + it.h / 2);
    floor = y + it.h / 2 + gap;
    return y;
  });
  // Up pass: pulls the stack back inside the bottom edge.
  let ceil = bottom;
  for (let k = order.length - 1; k >= 0; k--) {
    const h = order[k].h;
    ys[k] = Math.min(ys[k], ceil - h / 2);
    ceil = ys[k] - h / 2 - gap;
  }
  // A stack taller than the space starts at the top and runs over the bottom rather than up.
  if (order.length && ys[0] - order[0].h / 2 < top) {
    let f = top;
    for (let k = 0; k < order.length; k++) {
      ys[k] = Math.max(ys[k], f + order[k].h / 2);
      f = ys[k] + order[k].h / 2 + gap;
    }
  }
  order.forEach((it, k) => (out[it.i] = ys[k]));
  return out;
}

/** A value label this far from its point or further gets a leader line (C1). */
export const LEADER_PX = 4;

/**
 * The right gutter (C1) holds live values only: each label wants to sit level with its line's
 * end; they're stacked so none overlaps, and a label moved more than LEADER_PX from its point
 * gets a 1 px leader back to it.
 */
export function gutterLayout(pins: { y: number; h: number }[], top: number, bottom: number, gap = 2): { y: number; leader: boolean }[] {
  const ys = placeLabels(pins, top, bottom, gap);
  return pins.map((p, i) => ({ y: ys[i], leader: Math.abs(ys[i] - p.y) > LEADER_PX }));
}

/**
 * Where each threshold's inline label starts (C1): at the plot's left edge, unless it would
 * overlap a label already placed (two lines close together, "85 °C" and "90 °C" on a short
 * plot); then just right of that one, `gap` px on. Labels are [top, bottom] spans and widths.
 */
export function thresholdLabelXs(labels: { top: number; bottom: number; w: number }[], left: number, gap: number): number[] {
  const placed: { x0: number; x1: number; top: number; bottom: number }[] = [];
  return labels.map((l) => {
    let x = left;
    for (let moved = true; moved; ) {
      moved = false;
      for (const p of placed) {
        if (l.top < p.bottom && l.bottom > p.top && x < p.x1 && x + l.w > p.x0) {
          x = p.x1 + gap;
          moved = true;
        }
      }
    }
    placed.push({ x0: x, x1: x + l.w, top: l.top, bottom: l.bottom });
    return x;
  });
}

/**
 * Which side of its line each threshold's label sits on: above it (below one that runs along the
 * top), as a lone label does. Thresholds closer than two chip heights are a cluster (90% and 95%
 * at the top of a memory chart): each of those chips sits centred on its own line, which it
 * breaks, so no label is nearer the other threshold's line than its own. `ys` are the lines'
 * pixel rows, `h` the chip height, `pad` the gap between a line and a chip above or below it,
 * `top`/`bottom` the plot's edges (a chip never leaves it). Returns each chip's vertical centre.
 */
export function thresholdLabelSides(ys: number[], h: number, pad: number, top: number, bottom: number): { side: "above" | "below" | "on"; cy: number }[] {
  return ys.map((y, i) => {
    const near = ys.some((o, j) => j !== i && Math.abs(o - y) < 2 * h);
    if (near) return { side: "on", cy: Math.min(bottom - h / 2, Math.max(top + h / 2, y)) };
    return y - pad - h >= top ? { side: "above", cy: y - pad - h / 2 } : { side: "below", cy: y + pad + h / 2 };
  });
}

/** A threshold's inline label (C1), in one format: "90% used" reads "90%", like "95%" beside it. */
export function thresholdText(label: string): string {
  return label.replace(/^([<>≤≥]?\s*[\d.,]+\s*(?:%|°C|[kKMGT]?i?B|W|MHz)?)\s+(?:used|full|free|in use)$/i, "$1");
}

/**
 * A series that barely moves: its range is under 1% of the domain (or of its own largest
 * magnitude when there's no domain). Tiles hide the trend of such a series.
 */
export function isConstant(values: ArrayLike<number | null | undefined>, domain?: [number, number] | null): boolean {
  let lo = Infinity;
  let hi = -Infinity;
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (v == null || !Number.isFinite(v)) continue;
    lo = Math.min(lo, v);
    hi = Math.max(hi, v);
  }
  if (!(hi >= lo)) return true;
  const span = domain ? Math.abs(domain[1] - domain[0]) : Math.max(Math.abs(lo), Math.abs(hi));
  if (!(span > 0)) return true;
  return hi - lo < span * 0.01;
}

/**
 * At most `maxPoints` bucket means, keeping gaps (a bucket with no values is null). A
 * sparkline 52 px wide gets 26 points, so one bad second can't draw a spike.
 */
export function bucketMeans(values: ArrayLike<number | null | undefined>, maxPoints: number): (number | null)[] {
  const n = values.length;
  const k = Math.max(1, Math.floor(maxPoints));
  if (n <= k) return Array.from({ length: n }, (_, i) => (values[i] == null || !Number.isFinite(values[i] as number) ? null : (values[i] as number)));
  const out: (number | null)[] = [];
  for (let b = 0; b < k; b++) {
    const from = Math.floor((b * n) / k);
    const to = Math.floor(((b + 1) * n) / k);
    let sum = 0;
    let count = 0;
    for (let i = from; i < to; i++) {
      const v = values[i];
      if (v != null && Number.isFinite(v)) {
        sum += v;
        count++;
      }
    }
    out.push(count ? sum / count : null);
  }
  return out;
}

/**
 * Where recording stopped (SparkBox was closed or asleep): steps in `ts` more than `maxGap`
 * times the spacing around them, as [from, to] index pairs. A node that was offline still
 * has a (null) reading every second, so it isn't a gap here; 10-second buckets from the
 * history store aren't either, because their neighbours are 10 s apart too.
 */
export function gapRuns(ts: ArrayLike<number>, maxGap = 2): [number, number][] {
  const out: [number, number][] = [];
  const step = (i: number) => (i >= 1 && i < ts.length ? ts[i] - ts[i - 1] : NaN);
  for (let i = 1; i < ts.length; i++) {
    const d = step(i);
    const around = [step(i - 1), step(i + 1)].filter((x) => Number.isFinite(x) && x > 0);
    const local = around.length ? Math.max(...around.map((x) => Math.min(x, d))) : 1;
    if (d > maxGap * Math.max(local, 1)) out.push([i - 1, i]);
  }
  return out;
}

/** The typical spacing of `ts` (the median step), in its own unit; 0 with fewer than two. */
export function medianStep(ts: ArrayLike<number>): number {
  const steps: number[] = [];
  for (let i = 1; i < ts.length; i++) steps.push(ts[i] - ts[i - 1]);
  if (!steps.length) return 0;
  steps.sort((a, b) => a - b);
  return steps[Math.floor(steps.length / 2)];
}

// ---- Chart grammar v2 (§2.4): robust axes, glyph clusters, gaps, neutral thresholds, one grid

/** The latest real value of each series, the largest of them (null with none). */
export function currentOf(series: ArrayLike<number | null | undefined>[]): number | null {
  let out: number | null = null;
  for (const s of series) {
    const v = latest(s);
    if (v != null && (out == null || v > out)) out = v;
  }
  return out;
}

export interface RobustAxis {
  /** The value the axis should reach (before rounding to a nice tick). */
  top: number;
  /** True when the series are bursty and the axis was cut below their maximum. */
  robust: boolean;
  /** The largest value in the window: the "▲ peaks" chip names it. */
  peak: number;
}

/**
 * The top of a bursty chart's value axis (§2.4). Over the visible values it takes the median,
 * p90, p98, maximum and the current value. When the maximum is within 1.5× of p98 the series
 * isn't bursty and the axis runs to the maximum. Otherwise it stops at the largest of p90 × 1.4,
 * current × 1.25, median × 4 and any threshold drawn, so a 105 Mb/s line isn't flattened on
 * the floor of a 6 Gb/s axis by a few spikes; the spikes sit on the top edge with ▲ markers.
 */
export function robustTop(series: ArrayLike<number | null | undefined>[], opts: { thresholds?: number[]; current?: number | null } = {}): RobustAxis {
  const all: number[] = [];
  for (const s of series) for (let i = 0; i < s.length; i++) {
    const v = s[i];
    if (v != null && Number.isFinite(v)) all.push(v);
  }
  const ths = (opts.thresholds ?? []).filter((t) => Number.isFinite(t));
  if (!all.length) return { top: Math.max(0, ...ths), robust: false, peak: 0 };
  all.sort((a, b) => a - b);
  const at = (p: number) => all[Math.max(0, Math.ceil((p / 100) * all.length) - 1)];
  const max = all[all.length - 1];
  const p98 = at(98);
  const current = opts.current ?? currentOf(series) ?? 0;
  if (!(max > 1.5 * p98) || !(p98 > 0)) return { top: Math.max(max, ...ths), robust: false, peak: max };
  const top = Math.max(at(90) * 1.4, current * 1.25, at(50) * 4, ...ths);
  return top >= max ? { top: max, robust: false, peak: max } : { top, robust: true, peak: max };
}

export interface GlyphCluster<T> {
  /** Where the cluster's glyph goes (px). */
  x: number;
  members: T[];
}

/** Space between two glyph badges' edges (C1). */
export const GLYPH_PAD = 4;

/** The text a cluster badge carries: its count, "99+" past that. */
export const clusterText = (n: number) => (n > 99 ? "99+" : String(n));

/**
 * A glyph's drawn width for a cluster of `n` (C1): a lone glyph is 10 px; a cluster is a pill
 * sized to its count, 14 px for one digit and 6 px more per extra character.
 */
export function badgeWidth(n: number): number {
  return n <= 1 ? 10 : 14 + (clusterText(n).length - 1) * 6;
}

/**
 * Clusters glyphs (event markers, ▲ peaks) so no two badges come within GLYPH_PAD px of each
 * other: neighbours closer than that join a cluster, a cluster sits at the mean of its
 * members, and clusters pushed together by that (or by the [left, right] edges, or by
 * growing a wider pill) merge again until it's stable. `gap` is either a fixed centre spacing
 * (▲ markers, all one size) or a badge width per member count (badgeWidth). Order is kept.
 */
export function placeGlyphs<T extends { x: number }>(items: readonly T[], gap: number | ((n: number) => number) = badgeWidth, left = -Infinity, right = Infinity): GlyphCluster<T>[] {
  const width = typeof gap === "number" ? () => gap - GLYPH_PAD : gap;
  const need = (a: GlyphCluster<T>, b: GlyphCluster<T>) => (width(a.members.length) + width(b.members.length)) / 2 + GLYPH_PAD;
  const sorted = [...items].filter((it) => Number.isFinite(it.x)).sort((a, b) => a.x - b.x);
  let out: GlyphCluster<T>[] = [];
  for (const it of sorted) {
    const last = out[out.length - 1];
    const one = { x: it.x, members: [it] };
    if (last && it.x - last.x < need(last, one)) {
      last.members.push(it);
      last.x = last.members.reduce((a, m) => a + m.x, 0) / last.members.length;
    } else {
      out.push(one);
    }
  }
  for (let guard = 0; guard < 1000; guard++) {
    for (const c of out) c.x = Math.max(left, Math.min(right, c.x));
    const i = out.findIndex((c, k) => k > 0 && c.x - out[k - 1].x < need(out[k - 1], c) - 1e-9);
    if (i < 0) break;
    const a = out[i - 1];
    const b = out[i];
    const members = [...a.members, ...b.members];
    out = [...out.slice(0, i - 1), { x: members.reduce((s, m) => s + m.x, 0) / members.length, members }, ...out.slice(i + 1)];
  }
  return out;
}

/** Glyph clusters that don't sit on the History chip: anything within `pad` px of [left, right] gives way. */
export function excludeChip<T extends { x: number }>(glyphs: readonly T[], chip: { left: number; right: number } | null, pad = 4): T[] {
  if (!chip) return [...glyphs];
  return glyphs.filter((g) => g.x < chip.left - pad || g.x > chip.right + pad);
}

/**
 * How a recording gap is drawn (C1): one shorter than a minute (a quick restart) or under
 * 8 px is a 1 px hairline; one of at least 2 minutes that is at least 60 px wide is a band
 * with a "No data" label; anything between is a band whose words are in its tooltip.
 */
export function gapStyle(widthPx: number, durationS: number = Infinity): "hairline" | "band" | "label" {
  if (widthPx < 8 || durationS < 60) return "hairline";
  return widthPx >= 60 && durationS >= 120 ? "label" : "band";
}

/**
 * The stretch at a chart's left edge that SparkBox wasn't recording (C2): the source's history
 * began before the window (`recordedFrom`), yet the window's first reading comes later, so the
 * app was closed (or showing the other data source) until then. Null when the window starts
 * with readings, or when history itself starts inside the window (that's "History starts").
 */
export function leadingGap(ts: ArrayLike<number>, from: number, recordedFrom: number | null | undefined, slackS = 5): [number, number] | null {
  if (recordedFrom == null || !ts.length || recordedFrom > from + slackS) return null;
  return ts[0] - from > slackS ? [from, ts[0]] : null;
}

/** Merges [from, to] ranges that touch or overlap (within `slack`), so each stretch is drawn and labelled once. */
export function mergeRanges(ranges: [number, number][], slack = 0): [number, number][] {
  const sorted = ranges.filter(([a, b]) => b >= a).sort((x, y) => x[0] - y[0]);
  const out: [number, number][] = [];
  for (const [a, b] of sorted) {
    const last = out[out.length - 1];
    if (last && a <= last[1] + slack) last[1] = Math.max(last[1], b);
    else out.push([a, b]);
  }
  return out;
}

/**
 * A threshold's tone in the visible window: neutral until a series reaches it (a healthy
 * chart spends no amber or red), then its own tone. `below` thresholds trip under the value.
 */
export function thresholdTone<T extends { value: number; tone: "warn" | "crit" | "neutral"; below?: boolean }>(
  th: T,
  series: ArrayLike<number | null | undefined>[],
): "warn" | "crit" | "neutral" {
  if (th.tone === "neutral") return "neutral";
  for (const s of series) for (let i = 0; i < s.length; i++) {
    const v = s[i];
    if (v == null || !Number.isFinite(v)) continue;
    if (th.below ? v <= th.value : v >= th.value) return th.tone;
  }
  return "neutral";
}

/**
 * A step-like rate (a counter sampled once a second) as a sparkline can draw it: bucket means
 * of at least `minPx` px each, then a light exponential moving average run forwards and back
 * (so it doesn't lag), so a square wave reads as a smooth rise and fall instead of vertical
 * risers. Gaps stay gaps.
 */
export function smoothRate(values: ArrayLike<number | null | undefined>, widthPx: number, minPx = 4, alpha = 0.5): (number | null)[] {
  const pts = bucketMeans(values, Math.max(2, Math.floor(widthPx / minPx)));
  const pass = (xs: (number | null)[]) => {
    let ema: number | null = null;
    return xs.map((v) => {
      if (v == null) return (ema = null);
      ema = ema == null ? v : ema + alpha * (v - ema);
      return ema;
    });
  };
  return pass(pass(pts).reverse()).reverse();
}

/**
 * Whether a series moves in steps (C1): it dwells, sitting within 5% of the series' range of
 * one level for at least 3 samples at a time, for most of its length, and it gets from one
 * dwell to the next in a jump (at least a fifth of the range) rather than a slope. A noisy
 * plateau still dwells (its jitter is under 5%), so a counter sampled once a second is
 * smoothed rather than drawn as a square wave; a ramp or a sine doesn't jump between levels.
 */
export function dwellsInSteps(values: ArrayLike<number | null | undefined>, tolerance = 0.05, minRun = 3): boolean {
  const xs: number[] = [];
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (v != null && Number.isFinite(v)) xs.push(v);
  }
  if (xs.length < minRun * 2) return false;
  const range = Math.max(...xs) - Math.min(...xs);
  if (!(range > 0)) return false;
  const tol = range * tolerance;
  const runs: { from: number; to: number; level: number }[] = [];
  let i = 0;
  while (i < xs.length) {
    // The run starting at i: every sample within tol of the run's mean so far.
    let j = i + 1;
    let sum = xs[i];
    while (j < xs.length && Math.abs(xs[j] - sum / (j - i)) <= tol) {
      sum += xs[j];
      j++;
    }
    if (j - i >= minRun) {
      runs.push({ from: i, to: j - 1, level: sum / (j - i) });
      i = j;
    } else i++;
  }
  const dwelt = runs.reduce((a, r) => a + r.to - r.from + 1, 0);
  if (runs.length < 2 || dwelt / xs.length < 0.6) return false;
  // Between neighbouring dwells: a jump, not a slope.
  let jumps = 0;
  for (let k = 1; k < runs.length; k++) if (Math.abs(runs[k].level - runs[k - 1].level) >= range * 0.2 && runs[k].from - runs[k - 1].to <= 2) jumps++;
  return jumps / (runs.length - 1) >= 0.5;
}

export interface GridClaim {
  /** Width the chart's left axis (or row labels) needs, px. */
  left: number;
  /** Width its right gutter (threshold chips, the pinned value, the peaks chip) needs, px. */
  right: number;
}

/**
 * One layout for every chart in a ChartGrid (§2.4): the widest left axis and the widest right
 * gutter any of them needs, so every plot has the same insets, and (given those) the same x
 * ticks for the shared span.
 */
export function gridLayout(claims: readonly GridClaim[], span: [number, number], widthPx: number): { left: number; right: number; ticks: number[]; step: number } {
  const left = Math.ceil(Math.max(0, ...claims.map((c) => c.left)));
  const right = Math.ceil(Math.max(0, ...claims.map((c) => c.right)));
  const t = timeTicks(span[0], span[1], Math.max(1, widthPx - left - right), uses12Hour() ? MIN_TICK_PX.clock12 : MIN_TICK_PX.clock24);
  return { left, right, ticks: t.ticks, step: t.step };
}
