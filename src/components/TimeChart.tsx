import {
  createContext, useCallback, useContext, useEffect, useId, useLayoutEffect, useMemo, useRef, useState,
  type CSSProperties, type MouseEvent, type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { Ellipsis, Undo2 } from "lucide-react";
import uPlot from "uplot";
import {
  axisLabels, badgeWidth, clipsAbove, clusterText, currentOf, degenerateAxis, excludeChip, flatAxis, gapRuns, gapStyle, gutterLayout, hoverTime,
  latest, leadingGap, medianStep, mergeRanges, p98Top, pinnedValue, placeGlyphs, robustTop, thresholdLabelSides, thresholdLabelXs, thresholdText, thresholdTone, timeLabel, toCsv, uses12Hour,
  valueAxis, xAxis, xTickLabels, type AxisUnit, type GlyphCluster, type GridClaim, type ValueAxis,
} from "../lib/chartScale";
import { showContextMenu } from "../lib/contextMenu";
import { healthApi, useHealth } from "../lib/health";
import { opsApi } from "../lib/opsApi";
import { cssVar, onThemeChange, useDarkMode } from "../lib/theme";
import { toast } from "../lib/toasts";
import { useWindowId } from "../wm/WindowManager";
import { Icon } from "./Icon";
import "../windows/monitoring.css";

export interface ChartSeries {
  label: string;
  /**
   * CSS custom property, e.g. "--m-gpu" or "--node-2"; resolved for the canvas. The wash under
   * the line is the same colour unless the theme defines a "-fill" twin ("--m-mem-gpu-fill"):
   * Black washes a light tone with its family's base hue, which keeps its chroma on black.
   */
  color: string;
  /** One value per timestamp; null is a gap (offline, not recorded). */
  values: (number | null)[];
  /** Lowest/highest reading in each bucket, drawn as a faint band behind the line. */
  band?: { min: (number | null)[]; max: (number | null)[] };
  /** No area fill under this one (e.g. a limit drawn as a series). */
  noFill?: boolean;
  dash?: boolean;
}

export interface Threshold {
  value: number;
  label: string;
  /** The tone once the series reaches it in the visible window; until then it's drawn neutral (§2.4). */
  tone: "warn" | "crit" | "neutral";
  /** A floor rather than a ceiling (a weak optic's receive power): it trips at or below the value. */
  below?: boolean;
  /** Judged on this series alone (its running total in a stacked chart): "90% used" is about
   *  memory in use, not the page cache drawn on top of it. Every line drawn when unset. */
  series?: number;
}

export interface TimeChartProps {
  title: ReactNode;
  /** Unix seconds, oldest first. */
  ts: number[];
  series: ChartSeries[];
  fmt: (v: number) => string;
  /** The top of a bounded metric: 100 for %, the total for memory, the limit for power. */
  yMax?: number;
  unit?: AxisUnit;
  /** A floor the axis starts at unless the data goes lower (temperature: 20). */
  softMin?: number;
  /** Dashed lines, labelled by chips in the right gutter that never overlap. */
  thresholds?: Threshold[];
  /** Preferred plot height when the container doesn't set one (140–320). */
  height?: number;
  /** The x window [from, to] in unix seconds; by default the data's own span. */
  range?: [number, number];
  /** Relative labels (−4m … now) instead of clock times. */
  relative?: boolean;
  /** Charts with the same key share a crosshair; defaults to the window's id. */
  syncKey?: string;
  /** A zoomed window shared by the charts around it; with onZoom, drag-zoom is reported instead of kept locally. */
  zoom?: [number, number] | null;
  onZoom?: (z: [number, number] | null) => void;
  /** Stack the series (each area sits on the one before), e.g. memory by kind. */
  stacked?: boolean;
  /** When recorded history begins (unix s); a chip in the top gutter marks it. */
  startsAt?: number | null;
  /** Small text under the title, e.g. "of 200 Gb/s". */
  subtitle?: ReactNode;
  /**
   * How several series of one metric are drawn: "lines" (default) overlays them; "multiples"
   * gives each its own compact row on a shared domain with its current value on the right;
   * "band" draws their mean with a min–max band and reveals one on hover.
   */
  mode?: "lines" | "band" | "multiples";
  /**
   * Bursty series (§2.4): "auto" tops the axis by robustTop (median, p90, p98, max and the
   * current value) when the maximum is an outlier, draws the taller points on the top edge
   * with ▲ and names the peak in a "▲ peaks" chip; "p98" is the older p98 × 1.15 rule;
   * "none" (default) runs the axis to the maximum. Use "auto" for fabric, LAN, disk IO and power.
   */
  robust?: "auto" | "p98" | "none";
  /** Older spelling: clamp "p98" is robust "auto". */
  clamp?: "p98" | "none";
  /** The live value as a labelled dot at the right edge (default on for line charts; multiples print it per row). */
  pin?: boolean;
  /** A log value axis, and a Log toggle in the header; without it the toggle starts off. */
  log?: boolean;
  /** Labels of series that are offline: their legend value reads "—" and multiples hatch them. */
  offline?: string[];
  /** Of the offline ones, those SparkBox took down on purpose, with their word ("Restarting"): drawn neutral. */
  planned?: Record<string, string>;
  /** Hairlines with a glyph in the top gutter: alerts, change operations, notes. */
  events?: ChartEvent[];
  /** Gaps the agent backfilled (unix s ranges): drawn as gaps without the recording-gap band. */
  backfilled?: [number, number][];
  /**
   * When this data source's history begins (unix s, store.historyStart()). Time in the window
   * before its first reading, after that, is drawn as a recording gap.
   */
  recordedFrom?: number | null;
  /** What's read, for the stale note: "GPU" gives "No GPU reading since 06:58". */
  noun?: string;
  /**
   * Multiples: each row's value is its mean over the last this many seconds rather than its last
   * reading, for counter rates that are 0 most seconds (a near-idle fabric link is 423 b/s, not 0).
   */
  valueMeanS?: number;
}

export interface ChartEvent {
  /** Unix seconds. */
  ts: number;
  kind: "alert" | "change" | "info";
  label: string;
  severity?: "warn" | "crit";
  /** An alert that is still firing: a filled glyph in its tone with a hairline. Resolved ones are hollow and grey. */
  active?: boolean;
}

const MIN_H = 140;
const MAX_H = 320;
/** Multiples: one row per series, 28–36 px. */
const ROW_H = 30;
const NO_DATA = "No data recorded";
const DASH = "—";
const NO_READINGS = "No readings yet";
/** ▲ markers closer than this cluster into one (§2.4); event glyphs space by their badge width. */
const GLYPH_GAP = 12;
/** From this long a window, events are a density strip instead of glyphs (C1). */
const STRIP_FROM_S = 86_400;
/** The plot's left padding inside the uPlot box. */
const PAD_L = 2;

/** `color` with alpha, as rgba(): canvas can't use var() or color-mix(). */
function withAlpha(color: string, a: number): string {
  const c = toRgb(color);
  return c ? `rgba(${c[0]}, ${c[1]}, ${c[2]}, ${a})` : color;
}

let probe: CanvasRenderingContext2D | null | undefined;
const rgbCache = new Map<string, [number, number, number] | null>();

function canvas2d(): CanvasRenderingContext2D | null {
  if (probe === undefined) {
    try {
      probe = document.createElement("canvas").getContext("2d");
    } catch {
      probe = null;
    }
  }
  return probe;
}

function toRgb(color: string): [number, number, number] | null {
  if (rgbCache.has(color)) return rgbCache.get(color)!;
  let out: [number, number, number] | null = null;
  const hex = /^#([0-9a-f]{6})$/i.exec(color.trim());
  if (hex) {
    const n = parseInt(hex[1], 16);
    out = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  } else {
    const ctx = canvas2d();
    if (ctx) {
      // The canvas normalises any CSS colour; drawing one pixel reads it back as RGB.
      ctx.clearRect(0, 0, 1, 1);
      ctx.fillStyle = "#000";
      ctx.fillStyle = color;
      ctx.fillRect(0, 0, 1, 1);
      const d = ctx.getImageData(0, 0, 1, 1).data;
      out = [d[0], d[1], d[2]];
    }
  }
  rgbCache.set(color, out);
  return out;
}

/** Resolves "--m-gpu" (or a literal colour) for the canvas. */
function resolve(color: string): string {
  return color.startsWith("--") ? cssVar(color) || "#888" : color;
}

/** Text width at a CSS font, for laying out gutter chips before the canvas exists. */
function textWidth(text: string, font: string): number {
  const ctx = canvas2d();
  if (!ctx) return text.length * 6.5;
  ctx.font = font;
  return ctx.measureText(text).width;
}

/** Re-reads colours when the theme changes (a counter to key the chart on). */
function useThemeTick(): number {
  const dark = useDarkMode();
  const [tick, setTick] = useState(0);
  useEffect(() => onThemeChange(() => setTick((t) => t + 1)), []);
  return tick * 2 + (dark ? 1 : 0);
}

/** The x window shown: a zoom, the given range, or the data's own span. */
function useSpan(ts: number[], range: [number, number] | undefined, zoom: [number, number] | null): [number, number] {
  return useMemo<[number, number]>(() => {
    if (zoom) return zoom;
    if (range) return range;
    const first = ts[0] ?? Date.now() / 1000 - 300;
    const last = ts[ts.length - 1] ?? Date.now() / 1000;
    return [first, Math.max(last, first + 1)];
  }, [zoom, range, ts]);
}

/** Recording gaps as unix-second ranges, minus the ones an agent filled in. */
function closedGaps(ts: number[], backfilled: [number, number][] | undefined): [number, number][] {
  return gapRuns(ts)
    .map(([a, b]) => [ts[a], ts[b]] as [number, number])
    .filter(([a, b]) => !(backfilled ?? []).some(([f, t]) => f <= a + 1 && t >= b - 1));
}

// ---- crosshair sync with uPlot charts (and between multiples)

interface SyncSource {
  cursor: { sync: { values: [number | null, null]; scales: [string, null] } };
  axes: { side: number }[];
}

/**
 * Joins uPlot's crosshair group `key` as a plain listener/publisher, so the multiples chart
 * (SVG, not uPlot) moves with the window's other charts and moves them in turn.
 */
function useCursorSync(key: string | undefined, onTime: (t: number | null) => void): (t: number | null, x: number, w: number, h: number) => void {
  const cb = useRef(onTime);
  cb.current = onTime;
  const src = useRef<SyncSource>({ cursor: { sync: { values: [null, null], scales: ["x", null] } }, axes: [{ side: 2 }] });
  useEffect(() => {
    if (!key) return;
    const sync = uPlot.sync(key);
    const client = {
      pub: (type: string, from: { cursor?: { sync?: { values?: [number | null, unknown] } } }, x: number) => {
        if (type !== "mousemove" && type !== "mouseleave") return;
        const t = type === "mousemove" && x >= 0 ? (from.cursor?.sync?.values?.[0] ?? null) : null;
        cb.current(t);
      },
    };
    sync.sub(client as unknown as uPlot);
    return () => sync.unsub(client as unknown as uPlot);
  }, [key]);
  return useCallback((t, x, w, h) => {
    if (!key) return;
    src.current.cursor.sync.values[0] = t;
    uPlot.sync(key).pub("mousemove", src.current as unknown as uPlot, t == null ? -10 : x, t == null ? -10 : h / 2, w, h, null as unknown as number);
  }, [key]);
}

// ---- header: title, live values (legend), Log, Back to live, More

/** Legend keys on one line: what doesn't fit goes into a "+N" chip with a popover. */
function Legend({ keys, renderKey }: { keys: number[]; renderKey: (i: number) => ReactNode }) {
  const rowRef = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState(keys.length);
  /** Each key's width when last drawn, so the row can tell when they'd all fit again. */
  const widths = useRef(new Map<number, number>());
  // Two series never overflow: the title gives way instead (rm1).
  const fixed = keys.length <= 2;
  const [open, setOpen] = useState(false);
  const chipRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ right: number; top: number } | null>(null);
  const sig = keys.join(",");

  useEffect(() => setShown(keys.length), [sig]);
  // Steps down until the row fits, and back up as soon as every key would fit again (a value
  // got shorter, the header got wider): never "+1" with room to spare.
  useLayoutEffect(() => {
    const el = rowRef.current;
    if (!el || fixed) return;
    const kids = [...el.children].filter((c) => !c.classList.contains("tc-overflow")) as HTMLElement[];
    kids.forEach((c, k) => widths.current.set(keys[k], c.getBoundingClientRect().width));
    if (shown > 0 && el.scrollWidth > el.clientWidth + 1) {
      setShown((n) => Math.max(0, n - 1));
      return;
    }
    if (shown < keys.length && keys.every((k) => widths.current.has(k))) {
      const gap = parseFloat(getComputedStyle(el).columnGap) || 4;
      const all = keys.reduce((a, k) => a + widths.current.get(k)!, 0) + gap * (keys.length - 1);
      const room = el.clientWidth + (el.parentElement ? Math.max(0, el.parentElement.clientWidth - el.parentElement.scrollWidth) : 0);
      if (all <= room - 1) setShown(keys.length);
    }
  });
  useLayoutEffect(() => {
    const el = rowRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    let w = el.clientWidth;
    const ro = new ResizeObserver(() => {
      if (el.clientWidth === w) return;
      w = el.clientWidth;
      setShown(keys.length);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [sig]);
  useLayoutEffect(() => {
    if (!open || !chipRef.current) return;
    const r = chipRef.current.getBoundingClientRect();
    setPos({ right: Math.max(8, window.innerWidth - r.right), top: r.bottom + 4 });
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => {
      if (e.target instanceof Node && (popRef.current?.contains(e.target) || chipRef.current?.contains(e.target))) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      setOpen(false);
      chipRef.current?.focus();
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  const rest = fixed ? [] : keys.slice(shown);
  return (
    <div className={`tc-legend${fixed ? " fixed" : ""}`} ref={rowRef}>
      {(fixed ? keys : keys.slice(0, shown)).map((i) => renderKey(i))}
      {rest.length > 0 && (
        <button ref={chipRef} type="button" className="tc-overflow" aria-expanded={open} aria-haspopup="dialog" onClick={() => setOpen((o) => !o)}
          title={`${rest.length} more`}>
          +{rest.length}
        </button>
      )}
      {open && rest.length > 0 && createPortal(
        <div ref={popRef} className="tc-legend-pop" role="dialog" aria-label="More series" style={pos ? { right: pos.right, top: pos.top } : { visibility: "hidden" }}>
          {rest.map((i) => renderKey(i))}
        </div>,
        document.body,
      )}
    </div>
  );
}

interface HeadProps {
  title: ReactNode;
  subtitle?: ReactNode;
  series: ChartSeries[];
  hidden: ReadonlySet<number>;
  offline: ReadonlySet<string>;
  valueAt: (s: ChartSeries) => number | null;
  fmt: (v: number) => string;
  atLabel: string | null;
  onToggle: (i: number, e: MouseEvent) => void;
  onKeyHover?: (i: number | null) => void;
  zoomed: boolean;
  onLive: () => void;
  logToggle: boolean | null;
  onLog: () => void;
  onMenu: (e: MouseEvent) => void;
  /** Multiples label and value every row themselves. */
  noLegend?: boolean;
  /** Shown where the hover time goes while not hovering ("History starts 3:02 PM"). */
  note?: string;
}

function ChartHead(h: HeadProps) {
  const keys = h.series.map((_, i) => i);
  const renderKey = (i: number) => {
    const s = h.series[i];
    const off = h.offline.has(s.label);
    const v = off ? null : h.valueAt(s);
    return (
      <button
        key={s.label}
        type="button"
        className={`tc-key${h.hidden.has(i) ? " off" : ""}${off ? " offline" : ""}`}
        onClick={(e) => h.onToggle(i, e)}
        onMouseEnter={() => h.onKeyHover?.(i)}
        onMouseLeave={() => h.onKeyHover?.(null)}
        onFocus={() => h.onKeyHover?.(i)}
        onBlur={() => h.onKeyHover?.(null)}
        aria-pressed={!h.hidden.has(i)}
        title={off ? `${s.label} is offline` : `${h.hidden.has(i) ? "Show" : "Hide"} ${s.label} (⇧-click to show only this one)`}
      >
        <i style={{ background: `var(${s.color.startsWith("--") ? s.color : "--accent"})` }} className={s.dash ? "dash" : undefined} />
        <span className="tc-label">{s.label}</span>
        <b className="num">{v == null ? DASH : h.fmt(v)}</b>
      </button>
    );
  };
  return (
    <div className="tc-head">
      <div className="tc-title">
        <span className="tc-name">{h.title}</span>
        {h.subtitle && <span className="tc-sub">{h.subtitle}</span>}
      </div>
      <div className="tc-values" aria-live="off">
        {h.atLabel ? <span className="tc-at">at {h.atLabel}</span> : h.note ? <span className="tc-at tc-note">{h.note}</span> : null}
        {!h.noLegend && <Legend keys={keys} renderKey={renderKey} />}
        {h.logToggle !== null && (
          <button type="button" className="tc-log" aria-pressed={h.logToggle} onClick={h.onLog} title={h.logToggle ? "Linear scale" : "Logarithmic scale"}>
            Log
          </button>
        )}
        {h.zoomed && (
          <button type="button" className="tc-live" onClick={h.onLive} title="Return to the live window (or double-click the chart)">
            <Icon icon={Undo2} size={12} />Back to live
          </button>
        )}
        <button type="button" className="tc-more" onClick={h.onMenu} aria-label={`More for ${typeof h.title === "string" ? h.title : "this chart"}`} title="More">
          <Icon icon={Ellipsis} size={14} />
        </button>
      </div>
    </div>
  );
}

/** Legend toggling (click hides, ⇧-click solos), shared by every mode. */
function useHidden(series: ChartSeries[]) {
  const [hidden, setHidden] = useState<ReadonlySet<number>>(new Set());
  const toggle = (i: number, e: MouseEvent) => {
    setHidden((prev) => {
      if (e.shiftKey) {
        const only = prev.size === series.length - 1 && !prev.has(i);
        return only ? new Set() : new Set(series.map((_, j) => j).filter((j) => j !== i));
      }
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else if (next.size < series.length - 1) next.add(i);
      return next;
    });
  };
  return { hidden, setHidden, toggle };
}

function useChartMenu(props: TimeChartProps, zoom: [number, number] | null, setZoom: (z: [number, number] | null) => void, hidden: ReadonlySet<number>, showAll: () => void) {
  const { ts, series, title } = props;
  const copyCsv = useCallback(async () => {
    const csv = toCsv(ts, series.map((s) => ({ label: s.label, values: s.values })));
    try {
      await navigator.clipboard.writeText(csv);
      toast({ kind: "success", title: "Chart data copied", body: `${ts.length} rows of ${typeof title === "string" ? title : "chart"} data as CSV.` });
    } catch {
      toast({ kind: "error", title: "Couldn't copy the chart data", body: "The clipboard wasn't available." });
    }
  }, [ts, series, title]);
  return (e: MouseEvent) =>
    showContextMenu(e, [
      { label: "Copy data as CSV", run: copyCsv },
      { label: "Back to live", disabled: !zoom, run: () => setZoom(null) },
      { separator: true },
      { label: "Show all series", disabled: hidden.size === 0, run: showAll },
    ]);
}

/**
 * X-axis labels for [from, to] (C1): relative ones count back from "now" at the right edge in
 * one unit, clock ones carry AM/PM on every tick on a 12-hour Mac; the step comes from the
 * width, so labels stay a readable distance apart and none repeats.
 */
function xTicks(span: [number, number], widthPx: number, relative: boolean): { ticks: number[]; labels: string[] } {
  return xAxis(span, widthPx, relative, uses12Hour());
}

/** Log-axis ticks: 0, then every thousandfold step (1k, 1M, 1G…) up to the top decade. */
function logAxis(dataMax: number): ValueAxis {
  const top = 10 ** Math.max(1, Math.ceil(Math.log10(Math.max(dataMax, 10))));
  const ticks = [0];
  for (let e = 0; 10 ** e < top; e += 3) if (e > 0) ticks.push(10 ** e);
  ticks.push(top);
  return { min: 0, max: top, ticks };
}

// ---- one grid per window (§2.4)

interface GridValue {
  syncKey?: string;
  left: number;
  right: number;
  claim: (id: string, c: GridClaim | null) => void;
}

const GridCtx = createContext<GridValue | null>(null);

/**
 * Charts inside share one layout: the widest left axis and the widest right gutter any of
 * them needs, so every plot starts and ends at the same x and (for one span) carries the same
 * ticks; `syncKey` also gives them one crosshair. Use one per window or tab.
 */
export function ChartGrid({ syncKey, children }: { syncKey?: string; children: ReactNode }) {
  const claims = useRef(new Map<string, GridClaim>());
  const [ins, setIns] = useState({ left: 0, right: 0 });
  const claim = useCallback((id: string, c: GridClaim | null) => {
    if (c) claims.current.set(id, c);
    else claims.current.delete(id);
    const all = [...claims.current.values()];
    const next = { left: Math.ceil(Math.max(0, ...all.map((x) => x.left))), right: Math.ceil(Math.max(0, ...all.map((x) => x.right))) };
    setIns((p) => (p.left === next.left && p.right === next.right ? p : next));
  }, []);
  const value = useMemo(() => ({ syncKey, left: ins.left, right: ins.right, claim }), [syncKey, ins, claim]);
  return <GridCtx.Provider value={value}>{children}</GridCtx.Provider>;
}

/**
 * Claims what this chart needs and returns the insets every chart in the grid uses (the
 * claim itself outside a grid). Exported for charts drawn elsewhere that join a grid.
 */
export function useChartGridInsets(need: GridClaim): GridClaim & { syncKey?: string; inGrid: boolean } {
  const grid = useContext(GridCtx);
  const id = useId();
  const claim = grid?.claim;
  useLayoutEffect(() => {
    claim?.(id, need);
  }, [claim, id, need.left, need.right]);
  useLayoutEffect(() => () => claim?.(id, null), [claim, id]);
  if (!grid || !(grid.left > 0)) return { ...need, syncKey: grid?.syncKey, inGrid: !!grid };
  return { left: Math.max(grid.left, need.left), right: Math.max(grid.right, need.right), syncKey: grid.syncKey, inGrid: true };
}

interface GapBox {
  left: number;
  width: number;
  style: "hairline" | "band" | "label";
  /** A recording gap (SparkBox wasn't running) rather than time no series has a reading for. */
  closed: boolean;
}

interface Overlay {
  gaps: GapBox[];
  /** Event glyphs at their x (css px within the plot host), before clustering. */
  events: { x: number; e: ChartEvent }[];
  starts: { x: number; label: string } | null;
  /** The "▲ peaks 6.1 Gb/s" chip, right-aligned to the plot. */
  peaks: string | null;
  plot: { left: number; top: number; width: number; height: number };
  /** A day or more: events are a density strip with a hover list instead of glyphs (C1). */
  strip: boolean;
  /** Nothing to plot: "No readings yet" in the middle, no axis. */
  empty: boolean;
}

const sameOverlay = (a: Overlay | null, b: Overlay) => a !== null && JSON.stringify(a) === JSON.stringify(b);

/** The worst tone among a cluster's still-active alerts; resolved alerts and changes are neutral. */
function clusterTone(members: { e: ChartEvent }[]): "crit" | "warn" | "neutral" {
  let tone: "crit" | "warn" | "neutral" = "neutral";
  for (const { e } of members) {
    if (e.kind !== "alert" || !e.active) continue;
    if (e.severity === "crit") return "crit";
    if (e.severity === "warn") tone = "warn";
  }
  return tone;
}

function eventLine(e: ChartEvent): string {
  const state = e.kind === "alert" ? (e.active ? "" : " · resolved") : e.kind === "change" ? " · change" : "";
  return `${e.label} · ${hoverTime(e.ts, 86400 * 2)}${state}`;
}

/** One glyph or numbered cluster in the top gutter, with its hairline down the plot. */
function EventGlyph({ c, y, plotTop, plotBottom }: { c: GlyphCluster<{ x: number; e: ChartEvent }>; y: number; plotTop: number; plotBottom: number }) {
  const x = Math.round(c.x) + 0.5;
  const one = c.members.length === 1 ? c.members[0].e : null;
  const tone = clusterTone(c.members);
  const active = c.members.some(({ e }) => e.kind === "alert" && e.active);
  const title = c.members.length === 1 ? eventLine(one!) : `${c.members.length} events\n${c.members.map(({ e }) => `• ${eventLine(e)}`).join("\n")}`;
  let glyph: ReactNode;
  const w = badgeWidth(c.members.length);
  if (!one) {
    // A pill sized to its count, so "12" never overflows a 14 px circle (C1).
    glyph = (
      <>
        <rect x={x - w / 2} y={y - 7} width={w} height={14} rx={7} className={`tc-cluster tone-${tone}`} />
        <text x={x} y={y + 0.5} className="tc-cluster-n" textAnchor="middle" dominantBaseline="central">{clusterText(c.members.length)}</text>
      </>
    );
  } else if (one.kind === "change") {
    glyph = <path d={`M${x} ${y - 4.5}V${y + 4.5}`} className="tc-tick" />;
  } else if (one.kind === "alert" && one.active && one.severity === "warn") {
    glyph = <path d={`M${x} ${y - 4.6}L${x + 4.8} ${y + 3.8}H${x - 4.8}Z`} className="tc-glyph-warn" />;
  } else if (one.kind === "alert" && one.active) {
    glyph = <path d={`M${x} ${y - 5}L${x + 5} ${y}L${x} ${y + 5}L${x - 5} ${y}Z`} className="tc-glyph-crit" />;
  } else if (one.kind === "alert") {
    glyph = <path d={`M${x} ${y - 4.2}L${x + 4.2} ${y}L${x} ${y + 4.2}L${x - 4.2} ${y}Z`} className="tc-glyph-resolved" />;
  } else {
    glyph = <circle cx={x} cy={y} r={3} className="tc-glyph-info" />;
  }
  return (
    <g className={`tc-glyph${active ? " active" : ""}`} role="img" aria-label={title.split("\n")[0]}>
      <title>{title}</title>
      <line x1={x} x2={x} y1={plotTop} y2={plotBottom} className={`tc-hair tone-${tone}`} />
      {/* A wider transparent target, so a 9 px glyph is easy to point at. */}
      <rect x={x - Math.max(7, w / 2)} y={y - 8} width={Math.max(14, w)} height={16} className="tc-glyph-hit" />
      {glyph}
    </g>
  );
}

/**
 * A day or more of events (C1): a thin strip of ticks in the top gutter, one per event in its
 * tone, with the whole list on hover, instead of a row of numbered clusters.
 */
function DensityStrip({ events, y, left, width }: { events: { x: number; e: ChartEvent }[]; y: number; left: number; width: number }) {
  const list = [...events].sort((a, b) => b.e.ts - a.e.ts);
  const shown = list.slice(0, 20).map(({ e }) => `• ${eventLine(e)}`);
  const title = `${list.length} event${list.length === 1 ? "" : "s"} in this window, newest first\n${shown.join("\n")}${list.length > 20 ? `\nand ${list.length - 20} more` : ""}`;
  return (
    <g className="tc-strip" role="img" aria-label={`${list.length} events`}>
      <title>{title}</title>
      <rect x={left} y={y - 4} width={width} height={8} className="tc-strip-bg" />
      {events.map(({ x, e }, i) => (
        <rect key={i} x={Math.round(x) - 0.5} y={y - 4} width={1.5} height={8} className={`tc-strip-tick tone-${clusterTone([{ e }])}`} />
      ))}
    </g>
  );
}

/**
 * Over the canvas: recording gaps (a hairline, a band, or a labelled "No data" band by width
 * and length, each stretch once), and in the top gutter the History chip, the "▲ peaks" chip
 * and the event glyphs, clustered so no two badges come within 4 px (a density strip from a
 * day up). The chips are measured (getBBox) and glyphs on them (or within 4 px) give way.
 */
function OverlayLayer({ o, gutterTop }: { o: Overlay | null; gutterTop: number }) {
  const startsRef = useRef<SVGTextElement>(null);
  const peaksRef = useRef<SVGTextElement>(null);
  const [chipW, setChipW] = useState<{ starts: number; peaks: number }>({ starts: 0, peaks: 0 });
  const startsLabel = o?.starts?.label ?? "";
  const peaksLabel = o?.peaks ?? "";
  useLayoutEffect(() => {
    const measure = (el: SVGTextElement | null, label: string) => {
      if (!el || !label) return 0;
      try {
        const w = el.getBBox().width;
        if (w > 0) return w;
      } catch {
        // Not laid out (jsdom, a hidden window): estimate from the font below.
      }
      return textWidth(label, `${parseFloat(getComputedStyle(el).fontSize) || 10.5}px -apple-system, system-ui, sans-serif`);
    };
    const next = { starts: Math.ceil(measure(startsRef.current, startsLabel)), peaks: Math.ceil(measure(peaksRef.current, peaksLabel)) };
    setChipW((p) => (p.starts === next.starts && p.peaks === next.peaks ? p : next));
  }, [startsLabel, peaksLabel]);
  if (!o) return null;
  const { plot } = o;
  const cy = Math.max(7, plot.top - gutterTop / 2 - 1);
  const plotBottom = plot.top + plot.height;
  const right = plot.left + plot.width;
  // The History chip sits left of its rule, or right of it when the rule is too near the plot's
  // left edge (it would run over the value labels); the peaks chip at the right end of the gutter.
  const startsRight = !!o.starts && o.starts.x - 4 - chipW.starts - 10 < plot.left;
  const startsBox = o.starts && chipW.starts
    ? startsRight ? { left: o.starts.x - 2, right: o.starts.x + 4 + chipW.starts + 12 } : { left: o.starts.x - 4 - chipW.starts - 12, right: o.starts.x + 2 }
    : null;
  const peaksBox = o.peaks && chipW.peaks ? { left: right - chipW.peaks - 10, right } : null;
  const clusters = o.strip ? [] : excludeChip(excludeChip(placeGlyphs(o.events, badgeWidth, plot.left + 6, right - 6), startsBox), peaksBox);
  return (
    <div className="tc-overlay">
      {o.gaps.map((g, i) =>
        g.style === "hairline" ? (
          <div key={`g${i}`} className="tc-gap-hair" title={g.closed ? NO_DATA : "No data"} style={{ left: g.left + Math.floor(g.width / 2), top: plot.top, height: plot.height }} />
        ) : (
          <div key={`g${i}`} className={`tc-gap${g.style === "label" ? " labelled" : ""}`} title={g.closed ? NO_DATA : "No data"} style={{ left: g.left, width: g.width, top: plot.top, height: plot.height }}>
            {g.style === "label" && <span className="tc-gap-label">{g.closed && g.width >= 150 ? NO_DATA : "No data"}</span>}
          </div>
        ),
      )}
      {o.empty && <div className="tc-empty" style={{ left: plot.left, width: plot.width, top: plot.top, height: plot.height }}>{NO_READINGS}</div>}
      <svg className="tc-overlay-svg" width="100%" height="100%" aria-hidden={false}>
        {o.starts && (
          <g className="tc-chip tc-starts" role="note">
            <title>{o.starts.label}</title>
            <rect x={startsRight ? o.starts.x + 4 : o.starts.x - 4 - (chipW.starts + 10)} y={cy - 7} width={chipW.starts + 10} height={14} rx={7} />
            <text ref={startsRef} x={startsRight ? o.starts.x + 9 : o.starts.x - 9} y={cy + 0.5} textAnchor={startsRight ? "start" : "end"} dominantBaseline="central">{o.starts.label}</text>
          </g>
        )}
        {o.peaks && (
          <g className="tc-chip tc-peaks" role="note">
            <title>{`The highest reading in this window: ${o.peaks.replace(/^▲ peaks /, "")}. Taller points sit on the top edge with ▲; the readout shows their value.`}</title>
            <rect x={right - chipW.peaks - 10} y={cy - 7} width={chipW.peaks + 10} height={14} rx={7} />
            <text ref={peaksRef} x={right - 5} y={cy + 0.5} textAnchor="end" dominantBaseline="central">{o.peaks}</text>
          </g>
        )}
        {o.strip && o.events.length > 0 && <DensityStrip events={o.events} y={cy} left={plot.left} width={plot.width} />}
        {clusters.map((c, i) => (
          <EventGlyph key={`e${i}`} c={c} y={cy} plotTop={plot.top} plotBottom={plotBottom} />
        ))}
      </svg>
    </div>
  );
}

/** Index range [first, last] of `ts` inside [from, to] (inclusive), or [0, -1] when none. */
function visibleRange(ts: number[], from: number, to: number): [number, number] {
  let a = 0;
  while (a < ts.length && ts[a] < from) a++;
  let b = ts.length - 1;
  while (b >= a && ts[b] > to) b--;
  return [a, b];
}

/** Runs of indices where every given series is null (no reading at all), as [first, last]. */
function emptyRuns(n: number, cols: (number | null)[][]): [number, number][] {
  const out: [number, number][] = [];
  let start = -1;
  for (let i = 0; i < n; i++) {
    const empty = cols.length > 0 && cols.every((c) => c[i] == null);
    if (empty && start < 0) start = i;
    if (!empty && start >= 0) {
      out.push([start, i - 1]);
      start = -1;
    }
  }
  if (start >= 0) out.push([start, n - 1]);
  return out;
}

interface AxisInfo extends ValueAxis {
  /** Points rise above the top: ▲ on the edge and a peaks chip. */
  clamped: boolean;
  peak: number;
  /** No readings in the window: no axis, "No readings yet" (C1). */
  empty?: boolean;
  /** Log ticks are labelled by the chart's own formatter, not in one unit. */
  log?: boolean;
}

/** Recording gaps and stretches no series has a reading for, merged so each is drawn (and labelled) once. */
function gapBoxes(closed: [number, number][], empty: [number, number][], px: (a: number, b: number) => { left: number; width: number }): GapBox[] {
  const all = [...closed.map((r) => ({ r, closed: true })), ...empty.map((r) => ({ r, closed: false }))];
  return mergeRanges(all.map((x) => x.r), 1).map(([a, b]) => {
    const isClosed = all.some((x) => x.closed && x.r[0] < b && x.r[1] > a);
    const box = px(a, b);
    // A stretch with no reading only counts when it's wide enough to label; a recording gap always does.
    const style = gapStyle(box.width, b - a);
    return { ...box, style, closed: isClosed };
  }).filter((g) => g.closed || g.style === "label");
}

/**
 * A time-series chart in the Grafana mould (§2.4): a header with the title and each series'
 * live value (the values at the pointer while hovering, "—" for an offline series), nice
 * ticks that end on a labelled top, x labels that never clip ("now" sits right-aligned
 * inside the plot), the live value pinned as a labelled dot at the right edge, threshold
 * chips in a right gutter that never overlap (neutral until the series reaches them), a
 * History chip and clustered event glyphs in the top gutter, recording gaps drawn by width,
 * robust axes with ▲ markers and a "▲ peaks" chip for bursty series, a Log toggle, a
 * crosshair shared with the window's other charts (and one grid with them inside a
 * ChartGrid), drag to zoom and double-click to reset. Several series of one metric can be
 * drawn as small multiples or as a mean with a min–max band. Click a legend item to hide
 * that series; ⇧-click to show only it.
 */
export function TimeChart(props: TimeChartProps) {
  return props.mode === "multiples" ? <MultiplesChart {...props} /> : <LineChart {...props} />;
}

/** The robust mode a chart asked for, "clamp" being the older spelling. */
const robustOf = (p: TimeChartProps): "auto" | "p98" | "none" => p.robust ?? (p.clamp === "p98" ? "auto" : "none");

function LineChart(props: TimeChartProps) {
  const { title, ts, fmt, unit = "number", height = 160, relative, stacked, subtitle } = props;
  const band = props.mode === "band" && props.series.length > 1 && !stacked;
  const winId = useWindowId();
  const hostRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);
  const themeTick = useThemeTick();
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const [focusKey, setFocusKey] = useState<number | null>(null);
  const { hidden, setHidden, toggle } = useHidden(props.series);
  const [localZoom, setLocalZoom] = useState<[number, number] | null>(null);
  const zoom = props.onZoom ? (props.zoom ?? null) : localZoom;
  const setZoom = props.onZoom ?? setLocalZoom;
  const robust = robustOf(props);
  const pin = props.pin ?? true;
  const canLog = robust !== "none" || props.log !== undefined;
  const [logOn, setLogOn] = useState(!!props.log);
  const [overlay, setOverlay] = useState<Overlay | null>(null);
  const [fontPx, setFontPx] = useState(11);
  const offline = useMemo(() => new Set(props.offline ?? []), [props.offline]);

  // Band mode draws the mean and the min–max across series, plus each series (hidden until hovered).
  const series = useMemo<ChartSeries[]>(() => {
    if (!band) return props.series;
    const n = ts.length;
    const mean = new Array<number | null>(n);
    const lo = new Array<number | null>(n);
    const hi = new Array<number | null>(n);
    for (let j = 0; j < n; j++) {
      const vals = props.series.map((s) => s.values[j]).filter((v): v is number => v != null && Number.isFinite(v));
      mean[j] = vals.length ? vals.reduce((a, b) => a + b, 0) / vals.length : null;
      lo[j] = vals.length ? Math.min(...vals) : null;
      hi[j] = vals.length ? Math.max(...vals) : null;
    }
    const color = props.series[0].color;
    return [{ label: "Mean", color, values: mean, band: { min: lo, max: hi }, noFill: true }, ...props.series.map((s) => ({ ...s, noFill: true }))];
  }, [band, props.series, ts]);

  const span = useSpan(ts, props.range, zoom);
  const spanRef = useRef(span);
  spanRef.current = span;

  // Stacked charts draw cumulative values; the header still shows each series' own.
  const data = useMemo(() => {
    const cols: (number | null)[][] = [];
    const acc = new Array<number | null>(ts.length).fill(null);
    series.forEach((s, i) => {
      if (stacked && !hidden.has(i)) {
        const col = s.values.map((v, j) => (v == null ? acc[j] : (acc[j] ?? 0) + v));
        col.forEach((v, j) => (acc[j] = v));
        cols.push(col);
      } else {
        cols.push(s.values);
      }
    });
    const bands = series.flatMap((s) => (s.band && !stacked ? [s.band.min, s.band.max] : []));
    return [ts, ...cols, ...bands] as uPlot.AlignedData;
  }, [ts, series, stacked, hidden]);

  // What's drawn in the visible window decides the axis, the thresholds' tone and the peaks chip.
  const [i0, i1] = useMemo(() => visibleRange(ts, span[0], span[1]), [ts, span]);
  const shownCols = useMemo(() => {
    const idx = series.map((_, i) => i).filter((i) => (band ? i === 0 || focusKey === i - 1 : !hidden.has(i)));
    return idx.map((i) => (data[i + 1] as (number | null)[]).slice(i0, i1 + 1));
  }, [series, data, hidden, band, focusKey, i0, i1]);
  const ths = props.thresholds ?? [];
  const thTones = useMemo(
    () =>
      ths.map((th) => {
        if (th.series == null) return thresholdTone(th, shownCols);
        const col = data[th.series + 1] as (number | null)[] | undefined;
        return thresholdTone(th, col && !hidden.has(th.series) ? [col.slice(i0, i1 + 1)] : []);
      }),
    [ths, shownCols, data, hidden, i0, i1],
  );
  const axis = useMemo<AxisInfo>(() => {
    let dataMax = 0;
    let dataMin = 0;
    for (const c of shownCols) for (const v of c) if (v != null && Number.isFinite(v)) {
      if (v > dataMax) dataMax = v;
      if (v < dataMin) dataMin = v;
    }
    // Without a robust axis the band's highs belong on the chart too.
    if (robust === "none") for (const s of series) if (s.band && !stacked) for (const v of s.band.max.slice(i0, i1 + 1)) if (v != null && v > dataMax) dataMax = v;
    const thVals = ths.map((t) => t.value);
    // No readings: no axis at all. One value all along: one tick at it, not five "0.00" (C1).
    const deg = degenerateAxis(shownCols, props.yMax !== undefined);
    if (deg?.kind === "empty") return { min: 0, max: 1, ticks: [], clamped: false, peak: 0, empty: true };
    if (deg?.kind === "flat" && !stacked && !ths.some((t) => t.value > deg.value * 2)) return { ...flatAxis(deg.value), clamped: false, peak: deg.value };
    if (logOn) return { ...logAxis(Math.max(dataMax, ...thVals)), clamped: false, peak: dataMax, log: true };
    let top = dataMax;
    let peak = dataMax;
    let cut = false;
    if (robust !== "none" && props.yMax === undefined && !stacked) {
      if (robust === "p98") {
        const p = p98Top(shownCols, thVals);
        if (p > 0 && p < dataMax) [top, cut] = [p, true];
      } else {
        const r = robustTop(shownCols, { thresholds: thVals, current: currentOf(shownCols) });
        [top, peak, cut] = [r.top, r.peak, r.robust];
      }
    }
    const a = valueAxis(dataMin, Math.max(top, ...thVals), { unit: props.unit, bound: props.yMax, softMin: props.softMin });
    return { ...a, clamped: cut && clipsAbove(shownCols, a.max), peak };
  }, [shownCols, series, robust, logOn, ths, props.yMax, props.unit, props.softMin, stacked, i0, i1]);
  const axisRef = useRef(axis);
  axisRef.current = axis;

  // The insets this chart needs (its y labels; threshold chips and the pinned value on the
  // right), shared with the rest of its ChartGrid.
  const fontCss = `${fontPx}px -apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif`;
  // The newest reading of each series, or "—" when the newest point is empty or more than
  // two steps old (a GPU that stopped answering): then no Live dot, and the head says why (C1).
  const freshness = useMemo(() => {
    const step = Math.max(1, medianStep(ts));
    const nowS = Math.max(span[1], ts[ts.length - 1] ?? 0);
    return series.map((s, i) => {
      const drawn = pinnedValue(ts, data[i + 1] as (number | null)[], zoom ? span[1] : nowS, step * 2 + 0.5);
      // Stacked: the dot sits on the running total, but the value read out is the series' own.
      return { ...drawn, own: stacked ? pinnedValue(ts, s.values, zoom ? span[1] : nowS, step * 2 + 0.5).value : drawn.value };
    });
  }, [series, data, ts, span, zoom, stacked]);
  const pinned = useMemo(() => {
    if (!pin) return [];
    return series
      .map((s, i) => ({ s, i, v: freshness[i]?.value ?? null, own: freshness[i]?.own ?? null, since: freshness[i]?.since ?? null }))
      .filter(({ s, i, since }) => since != null && !offline.has(s.label) && (band ? i === 0 : !hidden.has(i)));
  }, [pin, series, freshness, hidden, band, offline]);
  const staleSince = pinned.length && pinned.every((p) => p.v == null) ? Math.max(...pinned.map((p) => p.since ?? 0)) : null;
  const need = useMemo<GridClaim>(() => {
    const yLabels = axis.log ? axis.ticks.map((v) => fmt(v)) : axisLabels(axis.ticks, fmt);
    const left = PAD_L + Math.ceil(Math.max(3 * fontPx * 0.58, ...yLabels.map((l) => textWidth(l, fontCss)))) + 12;
    // The right gutter holds live values only (thresholds are labelled inside the plot). The
    // label holds the widest value the axis can show, so it doesn't jitter.
    const pinW = pinned.length ? Math.max(textWidth(DASH, fontCss), ...[axis.max, axis.peak, ...pinned.map((p) => p.own ?? 0)].map((v) => textWidth(fmt(v), fontCss))) + 18 : 0;
    const right = Math.max(8, Math.ceil(pinW / 8) * 8);
    return { left: Math.ceil(left / 4) * 4, right };
  }, [axis, fmt, fontCss, fontPx, pinned]);
  const insets = useChartGridInsets(need);
  const syncKey = props.syncKey ?? insets.syncKey ?? (winId ? `win:${winId}` : undefined);

  const live = useRef({ props, zoom, setZoom, hidden, logOn, series, band, focusKey, thTones, pinned });
  live.current = { props, zoom, setZoom, hidden, logOn, series, band, focusKey, thTones, pinned };

  const gutterTop = props.startsAt != null || (props.events?.length ?? 0) > 0 || axis.clamped ? 18 : 0;
  const shape = `${series.map((s) => `${s.label}:${s.color}:${s.band ? 1 : 0}:${s.dash ? 1 : 0}:${s.noFill ? 1 : 0}`).join("|")}/${unit}/${props.yMax ?? ""}/${props.softMin ?? ""}/${stacked ? 1 : 0}/${relative ? 1 : 0}/${syncKey ?? ""}/${band ? 1 : 0}/${logOn ? 1 : 0}/${gutterTop}/${insets.left}/${insets.right}`;

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (host) setFontPx(parseFloat(getComputedStyle(host).fontSize) || 11);
  }, [themeTick]);

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const colors = series.map((s) => resolve(s.color));
    const fills = series.map((s, i) => (s.color.startsWith("--") && cssVar(`${s.color}-fill`)) || colors[i]);
    const axisText = cssVar("--chart-axis-text") || cssVar("--muted") || "#888";
    const grid = cssVar("--chart-grid") || cssVar("--hairline") || "#ddd";
    const font = parseFloat(getComputedStyle(host).fontSize) || 11;
    const fontStr = `${font}px -apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif`;
    // An uncrossed threshold and its label read the theme's --chart-threshold tokens (--text-3 and
    // --muted in Light and Dark; --text-2 in Black and under Increase contrast).
    const tone = { warn: cssVar("--warning") || "#efa810", crit: cssVar("--critical") || "#cc3336", neutral: cssVar("--chart-threshold") || cssVar("--text-3") || axisText };
    const toneText = { warn: cssVar("--warn-text") || tone.warn, crit: cssVar("--crit-text") || tone.crit, neutral: cssVar("--chart-threshold-text") || axisText };
    const textColor = cssVar("--text") || "#222";
    const surface = cssVar("--surface") || "#fff";
    // The threshold label's chip: a translucent surface box, or in Black a solid one with a --border edge.
    const chipEdge = ((c) => (c && c !== "transparent" ? c : null))(cssVar("--chart-chip-edge"));
    const nBands = series.filter((s) => s.band && !stacked).length;
    // The drawing rules each theme sets (--chart-*): Black draws 2px lines, stronger washes and
    // 8px hover points; Light and Dark keep the values written here as fallbacks.
    const rule = (name: string, fallback: number) => {
      const v = parseFloat(cssVar(name));
      return Number.isFinite(v) ? v : fallback;
    };
    const lineWidth = rule("--chart-line", 1.5);
    const fillTop = rule("--chart-fill-top", 0.14);
    const fillStack = rule("--chart-fill-stack", 0.35);
    const bandAlpha = rule("--chart-band", 0.1);
    const pointSize = rule("--chart-point", 6);

    const gradient = (color: string) => (u: uPlot) => {
      const { top, height: h } = u.bbox;
      if (!(h > 0)) return withAlpha(color, 0.1);
      const g = u.ctx.createLinearGradient(0, top, 0, top + h);
      g.addColorStop(0, withAlpha(color, fillTop));
      g.addColorStop(1, withAlpha(color, 0));
      return g;
    };

    const drawOverlays = (u: uPlot) => {
      const { ctx, bbox } = u;
      const dpr = devicePixelRatio || 1;
      const lv = live.current;
      const p = lv.props;
      const a = axisRef.current;
      ctx.save();
      ctx.font = `${font * dpr}px -apple-system, system-ui, sans-serif`;
      ctx.textBaseline = "middle";

      // X labels, placed so none is clipped: "now" right-aligned inside the plot.
      const { ticks, labels } = xTicks(spanRef.current, bbox.width / dpr, !!(p.relative && !lv.zoom));
      const boxes = ticks.map((t, i) => ({ x: u.valToPos(t, "x", true), w: ctx.measureText(labels[i]).width }));
      const placed = xTickLabels(boxes, bbox.left, bbox.left + bbox.width, 6 * dpr);
      ctx.fillStyle = axisText;
      const ly = bbox.top + bbox.height + (font * 0.9 + 5) * dpr;
      placed.forEach((pl, i) => {
        if (!pl.show) return;
        ctx.textAlign = pl.align;
        ctx.fillText(labels[i], pl.x, ly);
      });

      // History begins inside the window: shade the empty part (there's no data there).
      const starts = p.startsAt;
      if (starts != null && starts > spanRef.current[0]) {
        const x = Math.round(u.valToPos(starts, "x", true));
        if (x > bbox.left + 2) {
          ctx.fillStyle = withAlpha(axisText, 0.06);
          ctx.fillRect(bbox.left, bbox.top, x - bbox.left, bbox.height);
          ctx.strokeStyle = withAlpha(axisText, 0.55);
          ctx.setLineDash([2 * dpr, 3 * dpr]);
          ctx.lineWidth = dpr;
          ctx.beginPath();
          ctx.moveTo(x + 0.5, bbox.top - 4 * dpr);
          ctx.lineTo(x + 0.5, bbox.top + bbox.height);
          ctx.stroke();
          ctx.setLineDash([]);
        }
      }

      // Thresholds (neutral until the series reaches them): a dashed line labelled inline at its
      // left end, in one format ("90%", "95%"). The right gutter is for live values only (C1).
      const chipH = (font + 5) * dpr;
      const shownThs = a.empty ? [] : (p.thresholds ?? []).map((th, i) => ({ th, tone: lv.thTones[i] ?? "neutral" })).filter(({ th }) => th.value >= a.min && th.value <= a.max * 1.0001);
      const pins = lv.pinned.map((pp) => ({ ...pp, y: u.valToPos(Math.min(a.max, Math.max(a.min, pp.v ?? 0)), "y", true) }));
      const cx = bbox.left + bbox.width + 4 * dpr;
      // Threshold labels: a step under the axis text, never below the 10.5 px type floor (§D11).
      const small = `${Math.max(10.5, font - 1.5) * dpr}px -apple-system, system-ui, sans-serif`;
      // A chip sits above its line (below one that runs along the top); thresholds closer than
      // two chip heights (90% and 95%) each get a chip centred on their own line, moved right of
      // another chip it would overlap. Every line is stroked before any chip, so a chip breaks
      // the line it labels and no later line is drawn across an earlier chip.
      ctx.save();
      ctx.font = small;
      const chipBox = font * dpr;
      const thYs = shownThs.map(({ th }) => Math.round(u.valToPos(th.value, "y", true)) + 0.5);
      const thSides = thresholdLabelSides(thYs, chipBox, 2 * dpr, bbox.top, bbox.top + bbox.height);
      const thLabels = shownThs.map(({ th }, k) => {
        const w = ctx.measureText(thresholdText(th.label)).width + 4 * dpr;
        return { cy: thSides[k].cy, w, top: thSides[k].cy - chipBox / 2, bottom: thSides[k].cy + chipBox / 2 };
      });
      ctx.restore();
      const thXs = thresholdLabelXs(thLabels, bbox.left + 6 * dpr, 6 * dpr);
      shownThs.forEach(({ tone: t }, k) => {
        ctx.strokeStyle = withAlpha(tone[t], t === "neutral" ? 0.55 : 0.9);
        ctx.lineWidth = dpr;
        ctx.setLineDash([5 * dpr, 4 * dpr]);
        ctx.beginPath();
        ctx.moveTo(bbox.left, thYs[k]);
        ctx.lineTo(bbox.left + bbox.width, thYs[k]);
        ctx.stroke();
        ctx.setLineDash([]);
      });
      ctx.save();
      ctx.font = small;
      ctx.textBaseline = "middle";
      ctx.textAlign = "left";
      shownThs.forEach(({ th, tone: t }, k) => {
        const { cy, w, top } = thLabels[k];
        const x = Math.round(thXs[k]);
        const boxTop = Math.round(top);
        // The chip is opaque: it covers the line it labels, and the data under it, in every theme.
        ctx.fillStyle = surface;
        ctx.fillRect(x, boxTop, w, chipBox);
        if (chipEdge) {
          ctx.strokeStyle = chipEdge;
          ctx.lineWidth = dpr;
          ctx.strokeRect(x + 0.5, boxTop + 0.5, w - 1, chipBox - 1);
        }
        ctx.fillStyle = toneText[t];
        ctx.fillText(thresholdText(th.label), x + 2 * dpr, cy + 0.5 * dpr);
      });
      ctx.restore();
      const placedPins = gutterLayout(pins.map((pp) => ({ y: pp.y, h: chipH })), bbox.top - chipH / 2 + 1, bbox.top + bbox.height + chipH / 2 - 1, 2 * dpr);

      // Points above a robust axis sit on the top edge with a ▲, clustered per 12 px.
      if (a.clamped) {
        const xs = u.data[0];
        lv.series.forEach((_s, si) => {
          if (lv.hidden.has(si) || (lv.band && si > 0 && lv.focusKey !== si - 1)) return;
          const col = u.data[si + 1] as (number | null)[];
          const above: { x: number }[] = [];
          for (let j = 0; j < col.length; j++) {
            const v = col[j];
            if (v == null || v <= a.max) continue;
            const x = u.valToPos(xs[j], "x", true);
            if (x >= bbox.left && x <= bbox.left + bbox.width) above.push({ x });
          }
          ctx.fillStyle = colors[si];
          const top = bbox.top + 1 * dpr;
          for (const c of placeGlyphs(above, GLYPH_GAP * dpr, bbox.left + 4 * dpr, bbox.left + bbox.width - 4 * dpr)) {
            ctx.beginPath();
            ctx.moveTo(c.x, top);
            ctx.lineTo(c.x - 4 * dpr, top + 6 * dpr);
            ctx.lineTo(c.x + 4 * dpr, top + 6 * dpr);
            ctx.closePath();
            ctx.fill();
          }
        });
      }

      // The live value: a dot where each line ends, its value in the right gutter; one moved
      // more than 4 px from its point gets a leader back to it. A series with no fresh reading
      // gets no dot, and "—" in the gutter (C1).
      pins.forEach((pp, k) => {
        const ly = placedPins[k].y;
        if (pp.v == null) {
          ctx.fillStyle = axisText;
          ctx.textAlign = "left";
          ctx.fillText(DASH, cx + 9 * dpr, ly + 0.5 * dpr);
          return;
        }
        const col = u.data[pp.i + 1] as (number | null)[];
        let j = col.length - 1;
        while (j >= 0 && col[j] == null) j--;
        if (j < 0) return;
        const x = Math.min(bbox.left + bbox.width, u.valToPos(u.data[0][j], "x", true));
        ctx.fillStyle = surface;
        ctx.strokeStyle = colors[pp.i];
        ctx.lineWidth = 2 * dpr;
        ctx.beginPath();
        ctx.arc(x, pp.y, 3.5 * dpr, 0, Math.PI * 2);
        ctx.fill();
        ctx.stroke();
        if (placedPins[k].leader) {
          ctx.strokeStyle = withAlpha(colors[pp.i], 0.6);
          ctx.lineWidth = dpr;
          ctx.beginPath();
          ctx.moveTo(bbox.left + bbox.width, pp.y);
          ctx.lineTo(cx + 1 * dpr, ly);
          ctx.stroke();
        }
        ctx.fillStyle = colors[pp.i];
        ctx.beginPath();
        ctx.arc(cx + 3 * dpr, ly, 2.5 * dpr, 0, Math.PI * 2);
        ctx.fill();
        ctx.fillStyle = textColor;
        ctx.textAlign = "left";
        ctx.fillText(p.fmt(pp.own ?? pp.v), cx + 9 * dpr, ly + 0.5 * dpr);
      });
      ctx.restore();

      // DOM layer: gaps with their tooltip, event glyphs, the History and peaks chips.
      const toCss = (x: number) => x / dpr;
      const [from, to] = spanRef.current;
      const visible = (t: number) => t >= from && t <= to;
      const px = (t0: number, t1: number) => {
        const l = u.valToPos(Math.max(t0, from), "x");
        const r = u.valToPos(Math.min(t1, to), "x");
        return { left: Math.round(toCss(bbox.left) + l), width: Math.max(1, Math.round(r - l)) };
      };
      const lead = leadingGap(p.ts, from, p.recordedFrom);
      const closed = [...(lead ? [lead] : []), ...closedGaps(p.ts, p.backfilled)].filter(([g0, g1]) => g1 > from && g0 < to);
      // Time every shown series is empty (not a recording gap): merged with the recording gaps,
      // so a stretch is drawn and labelled once (C1: never "No data" over "No data").
      const cols = lv.series.map((_, i) => i).filter((i) => !lv.hidden.has(i)).map((i) => u.data[i + 1] as (number | null)[]);
      const xs = u.data[0] as number[];
      const blank = a.empty ? [] : emptyRuns(xs.length, cols)
        .map(([a0, a1]) => [xs[Math.max(0, a0 - 1)], xs[Math.min(xs.length - 1, a1 + 1)]] as [number, number])
        .filter(([t0, t1]) => t1 > from && t0 < to);
      const gaps = gapBoxes(closed, blank, px);
      const startsChip = starts != null && starts > from && starts < to
        ? { x: Math.round(toCss(bbox.left) + u.valToPos(starts, "x")), label: `History starts ${timeLabel(starts, 60, true)}` }
        : null;
      const evs = (p.events ?? []).filter((e) => visible(e.ts)).map((e) => ({ x: Math.round(toCss(bbox.left) + u.valToPos(e.ts, "x")), e }));
      const next: Overlay = {
        gaps, events: evs, starts: startsChip,
        peaks: a.clamped ? `▲ peaks ${p.fmt(a.peak)}` : null,
        plot: { left: toCss(bbox.left), top: toCss(bbox.top), width: toCss(bbox.width), height: toCss(bbox.height) },
        strip: to - from >= STRIP_FROM_S, empty: !!a.empty,
      };
      setOverlay((prev) => (sameOverlay(prev, next) ? prev : next));
    };

    const valueSeries: uPlot.Series[] = series.map((s, i) => ({
      label: s.label,
      stroke: colors[i],
      width: lineWidth,
      dash: s.dash ? [5, 4] : undefined,
      fill: s.noFill ? undefined : stacked ? withAlpha(fills[i], fillStack) : gradient(fills[i]),
      points: { show: false },
      spanGaps: false,
      show: band ? i === 0 : !hidden.has(i),
    }));
    const bandSeries: uPlot.Series[] = series.flatMap((s) =>
      s.band && !stacked
        ? [
            { label: `${s.label} min`, stroke: "transparent", width: 0, points: { show: false }, spanGaps: false },
            { label: `${s.label} max`, stroke: "transparent", width: 0, points: { show: false }, spanGaps: false },
          ]
        : [],
    );
    const bands: uPlot.Band[] = [];
    let b = series.length + 1;
    series.forEach((s, i) => {
      if (s.band && !stacked) {
        bands.push({ series: [b + 1, b], fill: withAlpha(colors[i], bandAlpha + (band ? 0.06 : 0)) });
        b += 2;
      }
    });
    if (stacked) {
      for (let i = series.length; i > 1; i--) bands.push({ series: [i, i - 1], fill: withAlpha(fills[i - 1], fillStack) });
    }

    const opts: uPlot.Options = {
      width: host.clientWidth || 320,
      height: Math.max(MIN_H, Math.min(MAX_H, host.clientHeight || height)),
      padding: [8 + gutterTop, insets.right, 0, PAD_L],
      legend: { show: false },
      cursor: {
        sync: syncKey ? { key: syncKey, setSeries: false } : undefined,
        drag: { x: true, y: false, setScale: false },
        // The hovered point: a surface-filled dot with a 2px ring in the series colour, so it reads
        // where it crosses the line (the min/max band series get none).
        points: {
          size: pointSize,
          width: 2,
          fill: (_u, si) => (si <= series.length ? surface : "transparent"),
          stroke: (_u, si) => colors[si - 1] ?? "transparent",
        },
        focus: { prox: 16 },
      },
      focus: { alpha: 0.35 },
      select: { show: true, left: 0, top: 0, width: 0, height: 0 },
      bands: stacked || nBands ? bands : undefined,
      scales: {
        x: { time: true, range: () => spanRef.current },
        y: {
          distr: logOn ? 4 : 1,
          asinh: 1,
          range: () => [axisRef.current.min, axisRef.current.max],
        },
      },
      axes: [
        {
          stroke: axisText,
          font: fontStr,
          grid: { show: false },
          ticks: { show: true, stroke: grid, width: 1, size: 4 },
          size: Math.round(font * 1.9 + 6),
          gap: 3,
          splits: (u) => xTicks(spanRef.current, u.bbox.width / (devicePixelRatio || 1), !!(live.current.props.relative && !live.current.zoom)).ticks,
          // Drawn in the draw hook instead, where the first and last can be aligned to the plot's edges.
          values: (_u, splits) => splits.map(() => ""),
        },
        {
          stroke: axisText,
          font: fontStr,
          grid: { stroke: grid, width: 1 },
          ticks: { show: false },
          gap: 6,
          // One width for every chart in the grid, so the plots line up.
          size: insets.left - PAD_L,
          splits: () => axisRef.current.ticks,
          // Decimals from the step, in one unit (C1); log ticks keep the chart's own format.
          values: (_u, splits) => (axisRef.current.log ? splits.map((v) => live.current.props.fmt(v)) : axisLabels(splits, live.current.props.fmt)),
        },
      ],
      series: [{}, ...valueSeries, ...bandSeries],
      hooks: {
        setCursor: [
          (u) => {
            const idx = u.cursor.idx;
            setHoverIdx(idx == null || u.cursor.left == null || u.cursor.left < 0 ? null : idx);
          },
        ],
        setSelect: [
          (u) => {
            if (u.select.width < 6) return;
            const from = u.posToVal(u.select.left, "x");
            const to = u.posToVal(u.select.left + u.select.width, "x");
            u.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
            if (to - from >= 5) live.current.setZoom([from, to]);
          },
        ],
        draw: [drawOverlays],
      },
    };
    const plot = new uPlot(opts, data, host);
    plotRef.current = plot;

    let frame = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const w = host.clientWidth;
        const h = Math.max(MIN_H, Math.min(MAX_H, host.clientHeight || height));
        if (w > 0 && (w !== plot.width || h !== plot.height)) plot.setSize({ width: w, height: h });
      });
    });
    ro.observe(host);
    return () => {
      cancelAnimationFrame(frame);
      ro.disconnect();
      plot.destroy();
      plotRef.current = null;
    };
    // Rebuilt only when the chart's shape, insets or the theme change; data and the window flow in below.
  }, [shape, themeTick, height]);

  // New data (or a new window, or a new axis): one setData, no rebuild.
  useEffect(() => {
    const plot = plotRef.current;
    if (!plot) return;
    plot.setData(data);
  }, [data, span, axis, props.events, props.thresholds, thTones, pinned]);

  // Which series show: the legend's choice; in band mode the mean, plus the one being pointed at.
  useEffect(() => {
    const plot = plotRef.current;
    if (!plot) return;
    series.forEach((_, i) => {
      const show = band ? i === 0 || focusKey === i - 1 : !hidden.has(i);
      if (plot.series[i + 1]?.show !== show) plot.setSeries(i + 1, { show });
    });
  }, [hidden, series, band, focusKey]);

  const menu = useChartMenu(props, zoom, setZoom, hidden, () => setHidden(new Set()));
  const at = hoverIdx != null && ts[hoverIdx] != null ? hoverIdx : null;
  // Not hovering: each series' fresh value ("—" once its readings stopped), not its last one.
  const valueAt = (s: ChartSeries) => (at != null ? (s.values[at] ?? null) : (freshness[series.indexOf(s)]?.own ?? (series.includes(s) ? null : latest(s.values))));
  // The legend lists the real series; in band mode they sit after the mean.
  const legendSeries = band ? props.series : series;

  return (
    <div className="tc" style={{ "--tc-h": `${height}px` } as CSSProperties} onContextMenu={menu}>
      <ChartHead
        title={title}
        subtitle={subtitle}
        series={legendSeries}
        hidden={band ? new Set() : hidden}
        offline={offline}
        valueAt={valueAt}
        fmt={fmt}
        atLabel={at != null ? hoverTime(ts[at], span[1] - span[0]) : null}
        onToggle={band ? () => undefined : toggle}
        onKeyHover={band ? setFocusKey : undefined}
        zoomed={!!zoom}
        onLive={() => setZoom(null)}
        logToggle={canLog ? logOn : null}
        onLog={() => setLogOn((v) => !v)}
        onMenu={menu}
        note={staleSince ? `No ${props.noun ? `${props.noun} ` : ""}reading since ${timeLabel(staleSince, 60, true)}` : undefined}
      />
      <div
        ref={hostRef}
        className="tc-plot"
        onDoubleClick={() => setZoom(null)}
        role="img"
        aria-label={`${typeof title === "string" ? title : "Chart"}: ${legendSeries.map((s) => `${s.label} ${offline.has(s.label) ? "offline" : (() => { const v = latest(s.values); return v == null ? "no data" : fmt(v); })()}`).join(", ")}`}
      >
        <OverlayLayer o={overlay} gutterTop={gutterTop} />
      </div>
    </div>
  );
}

// ---- small multiples

/** One row's path on [lo, hi], broken at gaps, in a 0..w × 0..h box. */
function rowPath(ts: number[], values: (number | null)[], span: [number, number], lo: number, hi: number, w: number, h: number): string {
  const [from, to] = span;
  const sx = (t: number) => ((t - from) / (to - from || 1)) * w;
  const sy = (v: number) => h - 1 - ((Math.max(lo, Math.min(hi, v)) - lo) / (hi - lo || 1)) * (h - 3);
  let d = "";
  let pen = false;
  for (let i = 0; i < ts.length; i++) {
    const v = values[i];
    if (ts[i] < from - 2 || ts[i] > to + 2 || v == null || !Number.isFinite(v)) {
      pen = false;
      continue;
    }
    d += `${pen ? "L" : "M"}${sx(ts[i]).toFixed(1)},${sy(v).toFixed(1)}`;
    pen = true;
  }
  return d;
}

/**
 * A row at pixel resolution. Null unless the window holds more samples than pixel columns;
 * then one point per column: its mean, min and max, a null where the column's samples were
 * all null (a gap stays a gap), nothing where it had no samples.
 */
function rowColumns(ts: number[], values: (number | null)[], span: [number, number], w: number): { ts: number[]; mean: (number | null)[]; min: (number | null)[]; max: (number | null)[] } | null {
  const [from, to] = span;
  const cols = Math.max(1, Math.floor(w));
  let n = 0;
  for (let i = 0; i < ts.length; i++) if (ts[i] >= from && ts[i] <= to) n++;
  if (n <= cols) return null;
  const sum = new Float64Array(cols);
  const cnt = new Uint32Array(cols);
  const seen = new Uint8Array(cols);
  const mn = new Float64Array(cols).fill(Infinity);
  const mx = new Float64Array(cols).fill(-Infinity);
  for (let i = 0; i < ts.length; i++) {
    if (ts[i] < from || ts[i] > to) continue;
    const c = Math.min(cols - 1, Math.floor(((ts[i] - from) / (to - from || 1)) * cols));
    seen[c] = 1;
    const v = values[i];
    if (v == null || !Number.isFinite(v)) continue;
    sum[c] += v;
    cnt[c]++;
    if (v < mn[c]) mn[c] = v;
    if (v > mx[c]) mx[c] = v;
  }
  const out = { ts: [] as number[], mean: [] as (number | null)[], min: [] as (number | null)[], max: [] as (number | null)[] };
  for (let c = 0; c < cols; c++) {
    if (!seen[c]) continue;
    out.ts.push(from + ((c + 0.5) / cols) * (to - from));
    const has = cnt[c] > 0;
    out.mean.push(has ? sum[c] / cnt[c] : null);
    out.min.push(has ? mn[c] : null);
    out.max.push(has ? mx[c] : null);
  }
  return out;
}

/** The min–max band of a row drawn at pixel resolution: along the maxima, back along the minima, per run. */
function rowBand(ts: number[], min: (number | null)[], max: (number | null)[], span: [number, number], lo: number, hi: number, w: number, h: number): string {
  const [from, to] = span;
  const sx = (t: number) => ((t - from) / (to - from || 1)) * w;
  const sy = (v: number) => h - 1 - ((Math.max(lo, Math.min(hi, v)) - lo) / (hi - lo || 1)) * (h - 3);
  let d = "";
  let run: string[] = [];
  const close = () => {
    if (run.length) d += `M${run.join("L")}Z`;
    run = [];
  };
  const back: string[] = [];
  for (let i = 0; i < ts.length; i++) {
    const a = min[i];
    const b = max[i];
    if (a == null || b == null) {
      run.push(...back.reverse());
      back.length = 0;
      close();
      continue;
    }
    const x = sx(ts[i]).toFixed(1);
    run.push(`${x},${sy(b).toFixed(1)}`);
    back.push(`${x},${sy(a).toFixed(1)}`);
  }
  run.push(...back.reverse());
  close();
  return d;
}

/** The area under a row's line: each run closed down to the strip's floor, gaps left open. */
function rowArea(ts: number[], values: (number | null)[], span: [number, number], lo: number, hi: number, w: number, h: number): string {
  const [from, to] = span;
  const sx = (t: number) => ((t - from) / (to - from || 1)) * w;
  const sy = (v: number) => h - 1 - ((Math.max(lo, Math.min(hi, v)) - lo) / (hi - lo || 1)) * (h - 3);
  const floor = (h - 1).toFixed(1);
  let d = "";
  let runStart: string | null = null;
  let lastX = "";
  const close = () => {
    if (runStart != null) d += `L${lastX},${floor}L${runStart},${floor}Z`;
    runStart = null;
  };
  for (let i = 0; i < ts.length; i++) {
    const v = values[i];
    if (ts[i] < from - 2 || ts[i] > to + 2 || v == null || !Number.isFinite(v)) {
      close();
      continue;
    }
    const x = sx(ts[i]).toFixed(1);
    if (runStart == null) {
      runStart = x;
      d += `M${x},${floor}`;
    }
    d += `L${x},${sy(v).toFixed(1)}`;
    lastX = x;
  }
  close();
  return d;
}

/**
 * Runs of null in a row, as [from, to] unix s: the node wasn't reporting. Seconds where every
 * row is empty (`allEmpty`) aren't one node's outage: nothing was recorded then.
 */
function nullRuns(ts: number[], values: (number | null)[], allEmpty?: boolean[]): [number, number][] {
  const out: [number, number][] = [];
  let start: number | null = null;
  for (let i = 0; i < ts.length; i++) {
    const missing = values[i] == null && !allEmpty?.[i];
    if (missing && start == null) start = i;
    if (!missing && start != null) {
      out.push([ts[Math.max(0, start - 1)], ts[i]]);
      start = null;
    }
  }
  if (start != null) out.push([ts[Math.max(0, start - 1)], ts[ts.length - 1]]);
  return out;
}

const MULTIPLES_NEED: GridClaim = { left: 84, right: 64 };

/**
 * Small multiples: one compact row per series on a shared domain, the current (or hovered)
 * value on the right, offline time hatched. Reads like Activity Monitor's per-core history.
 */
function MultiplesChart(props: TimeChartProps) {
  const { title, ts, series, fmt, subtitle } = props;
  const winId = useWindowId();
  // In a ChartGrid the label column and value column take the grid's insets, so these rows
  // start and end where the line charts' plots do.
  const insets = useChartGridInsets(MULTIPLES_NEED);
  const LABEL_W = insets.left;
  const VALUE_W = insets.right;
  const syncKey = props.syncKey ?? insets.syncKey ?? (winId ? `win:${winId}` : undefined);
  const zoom = props.onZoom ? (props.zoom ?? null) : null;
  const setZoom = props.onZoom ?? (() => undefined);
  const span = useSpan(ts, props.range, zoom);
  const offline = useMemo(() => new Set(props.offline ?? []), [props.offline]);
  const { hidden, setHidden, toggle } = useHidden(series);
  const plotRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(400);
  const [fontPx, setFontPx] = useState(11);
  const [hoverT, setHoverT] = useState<number | null>(null);
  const publish = useCursorSync(syncKey, setHoverT);

  useLayoutEffect(() => {
    const el = plotRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => el.clientWidth && setWidth(el.clientWidth);
    measure();
    setFontPx(parseFloat(getComputedStyle(el).fontSize) || 11);
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const rows = series.map((s, i) => ({ s, i })).filter(({ i }) => !hidden.has(i));
  const ths = (props.thresholds ?? []).map((t) => t.value);
  const robust = robustOf(props);
  const domainTop = useMemo(() => {
    if (props.yMax !== undefined) return props.yMax;
    const vals = series.map((s) => s.values);
    const top = robust === "p98" ? p98Top(vals, ths) : robust === "auto" ? robustTop(vals, { thresholds: ths }).top : Math.max(0, ...vals.flat().filter((v): v is number => v != null), ...ths);
    return valueAxis(0, top || 1, { unit: props.unit }).max;
  }, [series, props.yMax, robust, props.unit, ths.join(",")]);
  // Threshold lines stay neutral until that row reaches them in the window (§2.4): judged per
  // row, so one hot node doesn't paint the line amber on the cool ones.
  const thTones = useMemo(() => series.map((s) => (props.thresholds ?? []).map((th) => thresholdTone(th, [s.values]))), [props.thresholds, series]);
  const lo = props.softMin ?? 0;

  const plotW = Math.max(40, width - LABEL_W - VALUE_W);
  const [from, to] = span;
  const sx = (t: number) => ((t - from) / (to - from || 1)) * plotW;
  const sxSpan = (a: number, b: number) => sx(Math.min(b, to)) - sx(Math.max(a, from));
  const idxAt = (t: number) => {
    let best = -1;
    for (let i = 0; i < ts.length; i++) if (ts[i] <= t) best = i;
    return best;
  };
  const at = hoverT != null ? idxAt(hoverT) : -1;
  // Not hovering: a row's fresh value, "—" once its readings stopped (a GPU that fell off the bus).
  const maxAge = Math.max(1, medianStep(ts)) * 2 + 0.5;
  const nowS = zoom ? to : Math.max(to, ts[ts.length - 1] ?? 0);
  const recentMean = (s: ChartSeries): number | null => {
    let sum = 0;
    let n = 0;
    for (let i = ts.length - 1; i >= 0 && ts[i] > nowS - (props.valueMeanS ?? 0); i--) {
      const v = s.values[i];
      if (v != null && Number.isFinite(v)) [sum, n] = [sum + v, n + 1];
    }
    return n ? sum / n : null;
  };
  const valueOf = (s: ChartSeries) => {
    if (at >= 0) return s.values[at] ?? null;
    const fresh = pinnedValue(ts, s.values, nowS, maxAge).value;
    return fresh != null && props.valueMeanS ? recentMean(s) : fresh;
  };
  const lead = leadingGap(ts, from, props.recordedFrom);
  const closed = [...(lead ? [lead] : []), ...closedGaps(ts, props.backfilled)].filter(([a, b]) => b > from && a < to);
  // Seconds no row has a reading: drawn once across all rows as "No data", not as every node offline.
  const allEmpty = ts.map((_, i) => series.length > 1 && series.every((x) => x.values[i] == null));
  const blankRuns = nullRuns(ts, ts.map((_, i) => (allEmpty[i] ? null : 0))).filter(([a, b]) => b > from && a < to);
  // One stretch, one band and one label: a recording gap and the empty seconds around it merge (C1).
  const gapList = mergeRanges([...closed, ...blankRuns], 1).map(([a, b]) => {
    const w = sxSpan(a, b);
    return { a, b, w, style: gapStyle(w, b - a), closed: closed.some(([c0, c1]) => c0 < b && c1 > a) };
  }).filter((g) => g.closed || g.style !== "hairline");
  const noReadings = series.every((x) => x.values.every((v) => v == null || !Number.isFinite(v)));
  const xt = xTicks(span, plotW, !!(props.relative && !zoom));
  const placed = xTickLabels(xt.ticks.map((t, i) => ({ x: sx(t), w: textWidth(xt.labels[i], `${fontPx}px -apple-system, system-ui, sans-serif`) })), 0, plotW, 6);
  const rowsH = rows.length * ROW_H;
  // Recorded history begins inside the window: a dotted rule there, and the time in the header.
  const starts = props.startsAt != null && props.startsAt > from + 1 && props.startsAt < to ? props.startsAt : null;

  const onMove = (e: MouseEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const x = e.clientX - r.left - LABEL_W;
    if (x < 0 || x > plotW) {
      setHoverT(null);
      publish(null, 0, plotW, rowsH);
      return;
    }
    const t = from + (x / plotW) * (to - from);
    setHoverT(t);
    publish(t, x, plotW, rowsH);
  };
  const onLeave = () => {
    setHoverT(null);
    publish(null, 0, plotW, rowsH);
  };

  const menu = useChartMenu(props, zoom, setZoom, hidden, () => setHidden(new Set()));
  const hatchId = `tc-hatch-${(winId ?? "w").toString().replace(/[^\w-]/g, "")}-${String(title).replace(/[^\w]/g, "")}`;

  return (
    <div className="tc tc-multiples" onContextMenu={menu}>
      <ChartHead
        title={title}
        subtitle={subtitle}
        series={series}
        hidden={hidden}
        offline={offline}
        valueAt={valueOf}
        fmt={fmt}
        atLabel={hoverT != null && at >= 0 ? hoverTime(ts[at], to - from) : null}
        note={starts != null ? `History starts ${timeLabel(starts, 60, true)}` : undefined}
        onToggle={toggle}
        zoomed={!!zoom}
        onLive={() => setZoom(null)}
        logToggle={null}
        onLog={() => undefined}
        onMenu={menu}
        noLegend
      />
      <div
        ref={plotRef}
        className="tc-rows"
        onMouseMove={onMove}
        onMouseLeave={onLeave}
        role="img"
        aria-label={`${typeof title === "string" ? title : "Chart"}: ${series.map((s) => `${s.label} ${offline.has(s.label) ? "offline" : (() => { const v = latest(s.values); return v == null ? "no data" : fmt(v); })()}`).join(", ")}`}
      >
        <svg width={width} height={rowsH + 18} className="tc-rows-svg" aria-hidden>
          <defs>
            <pattern id={hatchId} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
              <line x1="0" y1="0" x2="0" y2="6" className="tc-hatch-line" />
            </pattern>
            <pattern id={`${hatchId}-n`} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
              <line x1="0" y1="0" x2="0" y2="6" className="tc-hatch-line neutral" />
            </pattern>
          </defs>
          {rows.map(({ s, i }, k) => {
            const y0 = k * ROW_H;
            const off = offline.has(s.label);
            const v = off ? null : valueOf(s);
            const h = ROW_H - 6;
            const colorVar = `var(${s.color.startsWith("--") ? s.color : "--accent"})`;
            // More samples than pixels (an hour of 1 s rates): the line is each column's mean and the
            // band its min–max, so a bursty hour reads as a level with its range, not a wall of ink.
            const cols = rowColumns(ts, s.values, span, plotW);
            return (
              <g key={s.label} transform={`translate(0 ${y0})`} className={off ? `tc-row off${props.planned?.[s.label] ? " is-planned" : ""}` : "tc-row"}>
                <text x={0} y={ROW_H / 2} className="tc-row-label" dominantBaseline="central">{s.label}</text>
                <g transform={`translate(${LABEL_W} 3)`}>
                  <rect x={0} y={0} width={plotW} height={h} className="tc-row-bg" />
                  {nullRuns(ts, s.values, allEmpty).filter(([a, b]) => b > from && a < to).map(([a, b], j) => (
                    <rect key={j} x={sx(Math.max(a, from))} y={0} width={Math.max(1, sx(Math.min(b, to)) - sx(Math.max(a, from)))} height={h} fill={`url(#${hatchId}${off ? "" : "-n"})`} className="tc-row-off">
                      <title>{`${s.label} wasn't reporting`}</title>
                    </rect>
                  ))}
                  {ths.map((tv, j) => {
                    const y = h - 1 - ((tv - lo) / (domainTop - lo || 1)) * (h - 3);
                    return y >= 0 && y <= h ? <line key={j} x1={0} x2={plotW} y1={y} y2={y} className={`tc-row-th tone-${thTones[i]?.[j] ?? "neutral"}`} /> : null;
                  })}
                  {cols && <path d={rowBand(cols.ts, cols.min, cols.max, span, lo, domainTop, plotW, h)} className="tc-row-band" style={{ fill: colorVar }} />}
                  <path d={rowArea(cols?.ts ?? ts, cols?.mean ?? s.values, span, lo, domainTop, plotW, h)} className="tc-row-fill" style={{ fill: colorVar }} />
                  <path d={rowPath(cols?.ts ?? ts, cols?.mean ?? s.values, span, lo, domainTop, plotW, h)} className="tc-row-line" style={{ stroke: colorVar }} />
                </g>
                <text x={width} y={ROW_H / 2} textAnchor="end" dominantBaseline="central" className={`tc-row-value${off ? " off" : ""}`}>
                  {off ? (props.planned?.[s.label] ?? "Offline") : v == null ? DASH : fmt(v)}
                </text>
              </g>
            );
          })}
          {starts != null && (
            <line x1={LABEL_W + sx(starts)} x2={LABEL_W + sx(starts)} y1={0} y2={rowsH} className="tc-row-starts" />
          )}
          {gapList.map(({ a, w, style, closed: wasClosed }, j) => {
            const x = LABEL_W + sx(Math.max(a, from));
            const tip = wasClosed ? NO_DATA : "No data recorded";
            return style === "hairline" ? (
              <line key={`gap${j}`} x1={x + w / 2} x2={x + w / 2} y1={0} y2={rowsH} className="tc-row-gap-hair"><title>{tip}</title></line>
            ) : (
              <g key={`gap${j}`}>
                <rect x={x} y={0} width={w} height={rowsH} className="tc-row-gap"><title>{tip}</title></rect>
                {style === "label" && <text x={x + w / 2} y={rowsH / 2} textAnchor="middle" dominantBaseline="central" className="tc-row-gap-label">{wasClosed && w >= 150 ? NO_DATA : "No data"}</text>}
              </g>
            );
          })}
          {noReadings && rows.length > 0 && (
            <text x={LABEL_W + plotW / 2} y={rowsH / 2} textAnchor="middle" dominantBaseline="central" className="tc-row-gap-label">{NO_READINGS}</text>
          )}
          {hoverT != null && hoverT >= from && hoverT <= to && (
            <line x1={LABEL_W + sx(hoverT)} x2={LABEL_W + sx(hoverT)} y1={0} y2={rowsH} className="tc-row-cursor" />
          )}
          <g transform={`translate(${LABEL_W} ${rowsH + 13})`}>
            {placed.map((p, i) => (p.show ? (
              <text key={i} x={p.x} textAnchor={p.align === "left" ? "start" : p.align === "right" ? "end" : "middle"} className="tc-row-axis">{xt.labels[i]}</text>
            ) : null))}
          </g>
        </svg>
      </div>
    </div>
  );
}

// ---- event annotations: alerts and changes over the charts' window

const EVENTS_POLL_MS = 20_000;

/**
 * What happened in the last `windowS` seconds, for TimeChart's `events`: alerts as they were
 * raised or escalated (alerts_history), each marked `active` while it's still firing, and
 * operations that changed something on the cluster (activity_log_since, entries marked as
 * changes; one per operation). Polled every 20 s; an older backend without either command
 * just gives no annotations.
 */
export function useChartEvents(windowS: number, node?: string): ChartEvent[] {
  const [events, setEvents] = useState<(ChartEvent & { alertId?: string })[]>([]);
  const health = useHealth();
  useEffect(() => {
    let alive = true;
    const load = async () => {
      const from = Date.now() - windowS * 1000;
      const [alerts, changes] = await Promise.all([
        healthApi.history(200).catch(() => []),
        opsApi.activitySince(from, 200).catch(() => []),
      ]);
      if (!alive) return;
      const out: (ChartEvent & { alertId?: string })[] = [];
      for (const a of Array.isArray(alerts) ? alerts : []) {
        if (a.tsMs < from || (a.kind !== "raised" && a.kind !== "escalated")) continue;
        if (node && a.alert.entity.node !== node && a.alert.entity.id !== node) continue;
        const sev = a.alert.severity === "crit" ? "crit" : "warn";
        // The same alert raised again within two minutes is one event.
        if (out.some((e) => e.kind === "alert" && e.label === a.alert.summary && Math.abs(e.ts - a.tsMs / 1000) < 120)) continue;
        out.push({ ts: a.tsMs / 1000, kind: "alert", label: a.alert.summary, severity: sev, alertId: a.alert.id });
      }
      const seenOps = new Set<string>();
      for (const c of Array.isArray(changes) ? changes : []) {
        if (!c.changes || c.tsMs < from || (node && c.node !== node)) continue;
        const key = c.opId ?? c.id;
        if (seenOps.has(key)) continue;
        seenOps.add(key);
        out.push({ ts: c.tsMs / 1000, kind: "change", label: c.title || c.command, severity: c.state === "failed" ? "warn" : undefined });
      }
      setEvents((prev) => (JSON.stringify(prev) === JSON.stringify(out) ? prev : out));
    };
    void load();
    const t = window.setInterval(load, EVENTS_POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [windowS, node]);
  // Active while the alert is still firing; resolved ones turn hollow and grey at once.
  const firing = (health?.alerts ?? []).filter((a) => a.state === "firing").map((a) => a.id).sort().join("|");
  return useMemo(() => {
    const ids = new Set(firing.split("|"));
    return events.map(({ alertId, ...e }) => (e.kind === "alert" ? { ...e, active: !!alertId && ids.has(alertId) } : e));
  }, [events, firing]);
}
