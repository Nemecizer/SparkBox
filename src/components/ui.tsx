// Shared building blocks for every window. Status is never colour alone: dots
// differ by shape and carry a word, notices carry an icon, meters say their value.
import {
  Children, isValidElement, useEffect, useId, useLayoutEffect, useRef, useState,
  type CSSProperties, type KeyboardEvent, type ReactElement, type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import {
  Check, CircleAlert, CircleCheck, Copy, Ellipsis, Info, Lock, LoaderCircle, RotateCw, TriangleAlert, WrapText, X,
  type LucideIcon,
} from "lucide-react";
import { bucketMeans, dwellsInSteps, isConstant, p98Top, smoothRate } from "../lib/chartScale";
import { level, type Level } from "../lib/format";
import type { LinkStatus } from "../lib/types";
import { confirmUnlock, lockOf, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, showLookOnly, useAdmin, useAdminState } from "../lib/admin";
import { closePanel } from "../lib/session";
import { loadHelpMeta, openHelp, preloadHelpMeta, useHelpMeta } from "../help/nav";
import "../help/helptip.css";
import { Icon, StopIcon } from "./Icon";

export { StopIcon };

// ---------------------------------------------------------------- status

export type DotStatus = LinkStatus | "stale" | "unconfigured" | "warn" | "crit";

const STATUS_LABEL: Record<DotStatus, string> = {
  online: "Online",
  connecting: "Connecting",
  offline: "Offline",
  error: "Error",
  stale: "Stale",
  unconfigured: "Not set up",
  warn: "Warning",
  crit: "Critical",
};

/** The dot alone, drawn so each state has its own shape (readable without colour). */
export function DotGlyph({ status, word }: { status: DotStatus; word: string }) {
  return (
    <svg className={`sdot sdot-${status}`} width={10} height={10} viewBox="0 0 10 10" role="img" aria-label={word}>
      {status === "online" && <circle cx={5} cy={5} r={4.5} fill="currentColor" />}
      {status === "connecting" && <circle className="sdot-pulse" cx={5} cy={5} r={3.6} fill="none" stroke="currentColor" strokeWidth={1.8} />}
      {status === "offline" && <circle cx={5} cy={5} r={3.75} fill="none" stroke="currentColor" strokeWidth={1.5} />}
      {status === "error" && (
        <>
          <circle cx={5} cy={5} r={4.5} fill="currentColor" />
          <path className="sdot-mark" d="M3.25 3.25 6.75 6.75M6.75 3.25 3.25 6.75" strokeWidth={1.4} strokeLinecap="round" />
        </>
      )}
      {status === "stale" && (
        <>
          <circle cx={5} cy={5} r={3.75} fill="none" stroke="currentColor" strokeWidth={1.5} />
          <path d="M5 1.25a3.75 3.75 0 0 0 0 7.5z" fill="currentColor" />
        </>
      )}
      {status === "unconfigured" && <circle cx={5} cy={5} r={3.75} fill="none" stroke="currentColor" strokeWidth={1.3} strokeDasharray="1.7 1.25" />}
      {status === "warn" && <path d="M5 .9 9.4 8.7H.6z" fill="currentColor" strokeLinejoin="round" stroke="currentColor" strokeWidth={0.8} />}
      {status === "crit" && (
        <>
          <path d="M5 .3 9.7 5 5 9.7.3 5z" fill="currentColor" />
          <path className="sdot-mark" d="M5 2.9v2.6" strokeWidth={1.3} strokeLinecap="round" />
          <circle className="sdot-mark-fill" cx={5} cy={7.1} r={0.75} />
        </>
      )}
    </svg>
  );
}

/**
 * A status dot whose shape and accessible name carry the state: online filled,
 * connecting a pulsing ring, offline a hollow ring, error filled with ✕, stale
 * half-filled amber, not set up a dashed ring, warning a triangle, critical a
 * diamond. `label` shows the word; `text` replaces it (e.g. "Up" for a port).
 */
export function StatusDot({ status, label, text }: { status: DotStatus; label?: boolean; text?: string }) {
  const word = text ?? STATUS_LABEL[status] ?? status;
  const shown = label || text !== undefined;
  return (
    <span className="status" title={shown ? undefined : word}>
      <DotGlyph status={status} word={word} />
      {shown && <span aria-hidden>{word}</span>}
    </span>
  );
}

export function statusWord(status: DotStatus): string {
  return STATUS_LABEL[status] ?? status;
}

// ---------------------------------------------------------------- one severity mapping (§2.3)

/**
 * What a thing is, for its glyph: ok, warn and crit come from its worst alert; offline is a
 * node or switch that isn't answering (louder than crit); affected is healthy itself but
 * part of something that isn't (a pipeline member of a model that's down); recovering,
 * planned and silenced are known states that don't count.
 */
export type GlyphState = "ok" | "warn" | "crit" | "offline" | "affected" | "recovering" | "planned" | "silenced";
export type ToneName = "neutral" | "warn" | "crit";

const GLYPH_WORD: Record<GlyphState, string> = {
  ok: "Healthy",
  warn: "Warning",
  crit: "Critical",
  offline: "Offline",
  affected: "Affected",
  recovering: "Recovering",
  planned: "Planned",
  silenced: "Silenced",
};

/**
 * The one tone for a severity or state word, so a badge, chip, row rule and glyph in one
 * window always agree: amber for warnings (and stale or connecting), red for critical,
 * offline, error, down and failed; everything else (ok, info, affected, recovering,
 * planned, silenced, known) is neutral.
 */
export function toneOf(s: string | null | undefined): ToneName {
  switch (s) {
    case "warn": case "warning": case "stale": case "connecting": case "waking":
      return "warn";
    case "crit": case "critical": case "offline": case "error": case "down": case "failed":
      return "crit";
    default:
      return "neutral";
  }
}

/** The glyph state for a node or switch's display status (link state, then health). */
export function glyphOfStatus(s: DotStatus): GlyphState {
  switch (s) {
    case "offline": case "error": return "offline";
    case "crit": return "crit";
    case "warn": case "stale": case "connecting": return "warn";
    case "unconfigured": return "affected";
    default: return "ok";
  }
}

/**
 * The glyph for a state (§2.3), drawn so each reads without colour: ok a small neutral dot,
 * warn an amber triangle, crit a filled red diamond, offline a filled red circle with a
 * white slash (the loudest), affected a hollow neutral ring, recovering a grey clock,
 * planned a grey wrench, silenced a grey bell with a slash. `label` overrides the word
 * screen readers hear.
 */
export function StateGlyph({ state, label, size = 10 }: { state: GlyphState; label?: string; size?: 10 | 12 | 14 }) {
  const word = label ?? GLYPH_WORD[state];
  return (
    <svg className={`sglyph sglyph-${state} tone-${toneOf(state)}`} width={size} height={size} viewBox="0 0 10 10" role="img" aria-label={word}>
      {state === "ok" && <circle cx={5} cy={5} r={3} fill="currentColor" />}
      {state === "warn" && <path d="M5 .9 9.4 8.7H.6z" fill="currentColor" strokeLinejoin="round" stroke="currentColor" strokeWidth={0.8} />}
      {state === "crit" && (
        <>
          <path d="M5 .3 9.7 5 5 9.7.3 5z" fill="currentColor" />
          <path className="sdot-mark" d="M5 2.9v2.6" strokeWidth={1.3} strokeLinecap="round" />
          <circle className="sdot-mark-fill" cx={5} cy={7.1} r={0.75} />
        </>
      )}
      {state === "offline" && (
        <>
          <circle cx={5} cy={5} r={4.8} fill="currentColor" />
          <path className="sglyph-slash" d="M2.6 7.4 7.4 2.6" strokeWidth={1.5} strokeLinecap="round" />
        </>
      )}
      {state === "affected" && <circle cx={5} cy={5} r={3.6} fill="none" stroke="currentColor" strokeWidth={1.4} />}
      {state === "recovering" && (
        <>
          <circle cx={5} cy={5} r={4} fill="none" stroke="currentColor" strokeWidth={1.2} />
          <path d="M5 2.7V5l1.6 1" fill="none" stroke="currentColor" strokeWidth={1.2} strokeLinecap="round" strokeLinejoin="round" />
        </>
      )}
      {state === "planned" && (
        <path d="M6.9.9a2.4 2.4 0 0 0-2.3 3.1L1.1 7.5a.9.9 0 0 0 1.3 1.3L5.9 5.3A2.4 2.4 0 0 0 9 3l-1.4 1.4-1.3-.3-.3-1.3z" fill="currentColor" />
      )}
      {state === "silenced" && (
        <>
          <path d="M2.2 7.2h5.9L7.3 6V4.3A2.3 2.3 0 0 0 5 2a2.3 2.3 0 0 0-2.3 2.3V6zM4.1 8.1a.9.9 0 0 0 1.8 0" fill="none" stroke="currentColor" strokeWidth={1.1} strokeLinejoin="round" />
          <path d="M1.2 1.2 8.8 8.8" stroke="currentColor" strokeWidth={1.2} strokeLinecap="round" />
        </>
      )}
    </svg>
  );
}

/**
 * A known issue (silenced or acknowledged), drawn muted wherever its metric shows (§3.4): a
 * small grey bell with a slash. `title` says how it's known ("Known issue: silenced indefinitely").
 */
export function KnownMark({ title = "Known issue: silenced", size = 10 }: { title?: string; size?: 10 | 12 | 14 }) {
  return (
    <span className="known-mark" title={title}>
      <StateGlyph state="silenced" label={title} size={size} />
    </span>
  );
}

/**
 * Advisory (§3.4): worth knowing, not a problem and never counted, such as PFC off or a node
 * on the LAN by Wi-Fi too. A grey ⓘ glyph in the neutral tone; `title` is the explanation.
 */
export function AdvisoryGlyph({ title, size = 10 }: { title: string; size?: 10 | 12 | 14 }) {
  return (
    <svg className="sglyph sglyph-advisory tone-neutral" width={size} height={size} viewBox="0 0 10 10" role="img" aria-label={title}>
      <title>{title}</title>
      <circle cx={5} cy={5} r={4.4} fill="none" stroke="currentColor" strokeWidth={1.1} />
      <circle cx={5} cy={3.1} r={0.75} fill="currentColor" />
      <path d="M5 4.6v2.8" stroke="currentColor" strokeWidth={1.2} strokeLinecap="round" />
    </svg>
  );
}

/** The word for a glyph state ("Offline", "Affected"…), for tooltips and table cells. */
export function glyphWord(state: GlyphState): string {
  return GLYPH_WORD[state];
}

// ---------------------------------------------------------------- numbers

export function Meter({ pct, label, warn, crit, color, ariaLabel }: {
  pct: number;
  label: string;
  warn?: number;
  crit?: number;
  /** Fill colour while healthy, e.g. "var(--m-gpu)". */
  color?: string;
  /** What the meter measures, for screen readers ("GPU on cell1"). */
  ariaLabel?: string;
}) {
  // Judged on the value as printed, so "85%" next to an 85% threshold is amber, not a hair under it.
  const lv: Level = warn !== undefined && crit !== undefined ? level(Math.round(pct), warn, crit) : "ok";
  const clamped = Math.max(0, Math.min(100, pct));
  const word = lv === "crit" ? "Critical" : lv === "warn" ? "Warning" : null;
  return (
    <div
      className={`meter ${lv}`}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(clamped)}
      aria-valuetext={word ? `${label}, ${word.toLowerCase()}` : label}
      aria-label={ariaLabel}
      style={color ? ({ "--meter-color": color } as CSSProperties) : undefined}
    >
      <div className="meter-track">
        <div className="meter-fill" style={{ width: `${clamped}%` }} />
      </div>
      <span className="meter-label">
        {word && <Icon icon={lv === "crit" ? CircleAlert : TriangleAlert} size={12} />}
        {label}
      </span>
    </div>
  );
}

/** The plotted points of a series: x from 0 to `width`, y with `lo` at the bottom and `hi` at the top. */
function plotPoints(values: (number | null)[], lo: number, hi: number, width: number, height: number, inset = 1): ({ x: number; y: number } | null)[] {
  const span = hi - lo || 1;
  const n = values.length;
  return values.map((v, i) => {
    if (v == null || !Number.isFinite(v)) return null;
    const t = (Math.max(lo, Math.min(hi, v)) - lo) / span;
    return { x: n > 1 ? (i / (n - 1)) * width : width / 2, y: height - inset - t * (height - 2 * inset) };
  });
}

const f2 = (v: number) => v.toFixed(2);

/**
 * An SVG path through the points, broken at gaps (null). `monotone` is the
 * Fritsch–Carlson cubic (d3's curveMonotoneX): smooth, never overshooting, and never a step.
 */
export function trendPath(points: ({ x: number; y: number } | null)[], curve: "linear" | "monotone" = "linear"): string {
  const runs: { x: number; y: number }[][] = [];
  let run: { x: number; y: number }[] = [];
  for (const p of points) {
    if (p) run.push(p);
    else if (run.length) {
      runs.push(run);
      run = [];
    }
  }
  if (run.length) runs.push(run);
  return runs.map((r) => {
    if (r.length === 1) return `M${f2(r[0].x)},${f2(r[0].y)}h0.01`;
    if (curve === "linear" || r.length === 2) return `M${r.map((p) => `${f2(p.x)},${f2(p.y)}`).join("L")}`;
    const n = r.length;
    const slope = (i: number) => (r[i + 1].y - r[i].y) / (r[i + 1].x - r[i].x || 1);
    const t = new Array<number>(n);
    for (let i = 1; i < n - 1; i++) {
      const h0 = r[i].x - r[i - 1].x;
      const h1 = r[i + 1].x - r[i].x;
      const s0 = slope(i - 1);
      const s1 = slope(i);
      const p = (s0 * h1 + s1 * h0) / (h0 + h1 || 1);
      t[i] = (Math.sign(s0) + Math.sign(s1)) * Math.min(Math.abs(s0), Math.abs(s1), 0.5 * Math.abs(p)) || 0;
    }
    const end = (i: number, j: number, ti: number) => {
      const h = r[j].x - r[i].x;
      return h ? (3 * (r[j].y - r[i].y) / h - ti) / 2 : ti;
    };
    t[0] = end(0, 1, t[1] ?? 0);
    t[n - 1] = end(n - 2, n - 1, t[n - 2] ?? 0);
    let d = `M${f2(r[0].x)},${f2(r[0].y)}`;
    for (let i = 0; i < n - 1; i++) {
      const dx = (r[i + 1].x - r[i].x) / 3;
      d += `C${f2(r[i].x + dx)},${f2(r[i].y + dx * t[i])},${f2(r[i + 1].x - dx)},${f2(r[i + 1].y - dx * t[i + 1])},${f2(r[i + 1].x)},${f2(r[i + 1].y)}`;
    }
    return d;
  }).join("");
}

/**
 * Whether a series moves in steps (a count of requests, a rate sampled from a counter): it
 * dwells on levels and jumps between them (chartScale.dwellsInSteps, C1). Such a series is
 * smoothed before it's drawn small, so it doesn't read as a square wave.
 */
export function isStepLike(values: ArrayLike<number | null | undefined>): boolean {
  return dwellsInSteps(values);
}

/**
 * A tiny trend line (§2.4): bucket means of at most width/2 points, linear or monotone, at the theme's
 * --spark-width (1.5px; 2px in Black), gaps left open, no fill unless asked (then the theme's --spark-fill,
 * 8%; a number caps at 8%). A step-like series (a request
 * count, a rate from a counter) is drawn from 4 px bucket means with a light moving average
 * instead, so it never shows vertical risers (`smooth` forces either way). `max`/`min` fix
 * the domain; `label` makes it an image for screen readers.
 */
export function Sparkline({ values, max = 100, min = 0, width = 72, height = 18, color = "var(--accent)", fill, label, curve = "linear", smooth = "auto" }: {
  values: (number | null)[];
  max?: number;
  min?: number;
  width?: number;
  height?: number;
  color?: string;
  /** A soft area under the line: true is the theme's --spark-fill (8%; more in Black), a number its opacity (capped at 8%). */
  fill?: boolean | number;
  label?: string;
  curve?: "linear" | "monotone";
  smooth?: boolean | "auto";
}) {
  const a11y = label ? { role: "img", "aria-label": label } : { "aria-hidden": true };
  const steps = smooth === true || (smooth === "auto" && isStepLike(values));
  const pts = steps ? smoothRate(values, width) : bucketMeans(values, Math.max(2, Math.floor(width / 2)));
  if (pts.filter((v) => v != null).length < 2) return <svg className="spark" width={width} height={height} {...a11y} />;
  const plotted = plotPoints(pts, min, Math.max(max, min + 1e-9), width, height);
  const d = trendPath(plotted, steps ? "monotone" : curve);
  const alpha = fill === true ? 0.08 : typeof fill === "number" ? Math.min(0.08, Math.max(0, fill)) : 0;
  const first = plotted.find(Boolean);
  const last = [...plotted].reverse().find(Boolean);
  return (
    <svg className="spark" width={width} height={height} viewBox={`0 0 ${width} ${height}`} {...a11y}>
      {alpha > 0 && first && last && <path className="spark-fill" d={`${d}L${f2(last.x)},${height}L${f2(first.x)},${height}Z`} style={{ fill: color, fillOpacity: fill === true ? "var(--spark-fill, 0.08)" : alpha }} />}
      <path d={d} fill="none" style={{ stroke: color, strokeWidth: "var(--spark-width, 1.5)" }} strokeWidth={1.5} strokeLinejoin="round" strokeLinecap="round" />
    </svg>
  );
}

const TREND_POINTS = 80;

/** A tile's trend: a line across the tile (the theme's --spark-width, 1.5px; 2px in Black), no fill, on a fixed domain where there is one. */
function TrendLine({ values, domain, color }: { values: (number | null)[]; domain: [number, number]; color: string }) {
  const pts = bucketMeans(values, TREND_POINTS);
  const d = trendPath(plotPoints(pts, domain[0], domain[1], 100, 24, 1.5), "monotone");
  return (
    <svg className="tile-trend" viewBox="0 0 100 24" preserveAspectRatio="none" aria-hidden>
      <path d={d} fill="none" style={{ stroke: color, strokeWidth: "var(--spark-width, 1.5)" }} strokeWidth={1.5} vectorEffect="non-scaling-stroke" strokeLinejoin="round" strokeLinecap="round" />
    </svg>
  );
}

/** The domain a tile trend is drawn on: the one given, else 0 to the series' p98 (bursty values don't flatten it). */
export function trendDomainFor(values: (number | null)[], domain?: [number, number], max?: number): [number, number] {
  if (domain) return domain;
  if (max !== undefined) return [0, max];
  const lo = Math.min(0, ...values.filter((v): v is number => v != null && Number.isFinite(v)));
  return [lo, Math.max(lo + 1e-9, p98Top([values]))];
}

/** Whether a tile shows its trend: there is one, it isn't switched off, and it actually moves. */
export function showsTrend(values: (number | null)[] | undefined, kind: "line" | "none" | undefined, domain: [number, number]): values is (number | null)[] {
  if (!values || kind === "none") return false;
  if (values.filter((v) => v != null).length < 2) return false;
  return !isConstant(values, domain);
}

export type Tone = "good" | "warn" | "crit";

const TONE_WORD: Record<Tone, string> = { good: "Good", warn: "Warning", crit: "Critical" };

/**
 * One headline number: a label, a 22px tabular value with a muted unit, an optional line
 * below and an optional trend. Quiet when normal: `tone` "warn" or "crit" adds an amber or
 * red edge and a word for screen readers ("good" draws nothing). The trend is a thin line
 * on `trendDomain` (memory: 0 to its capacity), hidden when it doesn't move or when
 * `trendKind` is "none" (a count). With `onClick` the whole tile is a button.
 */
export function Tile({ label, value, unit, sub, trend, trendMax, trendDomain, trendKind, tone, onClick, title, color, meter }: {
  label: ReactNode;
  value: ReactNode;
  unit?: ReactNode;
  sub?: ReactNode;
  trend?: (number | null)[];
  /** Older spelling of trendDomain [0, trendMax]. */
  trendMax?: number;
  trendDomain?: [number, number];
  trendKind?: "line" | "none";
  tone?: Tone;
  onClick?: () => void;
  title?: string;
  /** Trend colour, e.g. "var(--m-cpu)". */
  color?: string;
  /** A usage bar in place of a trend (a disk): neutral (its `color`, e.g. "var(--m-write)", in Black), amber past `warn`, red past `crit`. */
  meter?: { pct: number; warn?: number; crit?: number; color?: string };
}) {
  const domain = trend ? trendDomainFor(trend, trendDomain, trendMax) : ([0, 1] as [number, number]);
  const body = (
    <>
      <span className="tile-label">{label}{tone && tone !== "good" && <span className="sr-only">, {TONE_WORD[tone]}</span>}</span>
      <span className="tile-value">
        {value}
        {unit !== undefined && <span className="tile-unit">{unit}</span>}
      </span>
      {sub !== undefined && <span className="tile-sub">{sub}</span>}
      {meter ? (
        <span
          className={`tile-meter ${meter.warn !== undefined && meter.crit !== undefined ? level(Math.round(meter.pct), meter.warn, meter.crit) : "ok"}`}
          style={meter.color ? ({ "--meter-color": meter.color } as CSSProperties) : undefined}
          aria-hidden
        >
          <span style={{ width: `${Math.max(0, Math.min(100, meter.pct))}%` }} />
        </span>
      ) : showsTrend(trend, trendKind, domain) && <TrendLine values={trend} domain={domain} color={color ?? "var(--accent)"} />}
    </>
  );
  const cls = `tile${tone && tone !== "good" ? ` tone-${tone}` : ""}`;
  return onClick ? (
    <button type="button" className={cls} onClick={onClick} title={title}>{body}</button>
  ) : (
    <div className={cls} title={title}>{body}</div>
  );
}

const TILE_MIN = 170;
const TILE_GAP = 8;

/** Columns for `n` tiles in `width` px: one row when they fit, else balanced rows (6 → 3 + 3, never 5 + 1). */
export function tileColumns(n: number, width: number): number {
  const fit = Math.max(1, Math.floor((width + TILE_GAP) / (TILE_MIN + TILE_GAP)));
  if (n <= fit) return Math.max(1, n);
  return Math.ceil(n / Math.ceil(n / fit));
}

/** Equal-width tiles, 170px or wider, wrapping into rows of equal length. */
export function Tiles({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const n = Children.toArray(children).length;
  const [cols, setCols] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => el.clientWidth && setCols(tileColumns(n, el.clientWidth));
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [n]);
  return (
    <div ref={ref} className="tiles" style={cols ? { gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` } : undefined}>
      {children}
    </div>
  );
}

// ---------------------------------------------------------------- toolbar

export type AdminScope = "node" | "mac";

export interface ToolButtonProps {
  icon: LucideIcon;
  label: string;
  /** May return a Promise: the button is then busy (aria-busy, clicks ignored) until it settles. */
  onClick?: () => unknown;
  disabled?: boolean;
  title?: string;
  /** 1 always shows; 3 moves into "More" first when the window is narrow, then 2. */
  priority?: 1 | 2 | 3;
  /** A destructive action. It never shrinks to an icon: when labels give way it moves into More, label and all. */
  danger?: boolean;
  active?: boolean;
  /**
   * Needs Admin mode, which is locked. The button keeps its label and icon and gets a trailing
   * lock (the one rule, admin.ts lockOf). Locked, it stays enabled and a click asks to unlock
   * (confirmUnlock: the sheet, then Touch ID with Live data) and carries on; in look-only mode
   * it's disabled, and its tooltip says how to turn look-only off.
   */
  locked?: boolean;
  /** Which unlock `locked` asks for: the nodes (default) or state on this Mac (Keychain, VPN, server). */
  scope?: AdminScope;
  /** The command the unlock is for ("container_restart"): the prompt's words come from it. */
  action?: string;
  /** Working: aria-busy, a spinner, clicks ignored. */
  busy?: boolean;
}

/**
 * A locked control never goes straight to Touch ID (§3.3, C6): the in-app sheet asks first
 * ("To restart cell3, SparkBox needs Admin mode…"), and in look-only mode it only says why
 * nothing can change.
 */
const unlock = (reason: string | undefined, scope: AdminScope = "node"): Promise<boolean> => confirmUnlock(reason ?? "make changes", scope);

/** "Restart…" → "restart", for the unlock prompt's "…to restart". */
const reasonOf = (label: string) => label.replace(/…$/, "").replace(/^\p{Lu}(?!\p{Lu})/u, (c) => c.toLowerCase());

/**
 * Runs a tool's action the way every toolbar does: unlock first when it's locked (and stop
 * if that's cancelled), then the action. Resolves when the action's Promise, if any, settles.
 */
export async function runTool(t: Pick<ToolButtonProps, "label" | "onClick" | "locked" | "scope" | "action">): Promise<void> {
  if (t.locked && !(await unlock(t.action ?? reasonOf(t.label), t.scope ?? "node"))) return;
  await t.onClick?.();
}

/** A locked action's tooltip: its own title (when it says more than the label), then the rule's words. */
function lockTip(lock: "locked" | "lookOnly" | null, label: string, title?: string): string {
  if (lock === "lookOnly") return LOOK_ONLY_TIP;
  if (lock === "locked") return `${title && title !== label ? `${title} · ` : ""}${NEEDS_ADMIN_TIP}`;
  return title ?? label;
}

/** "Stop — needs Admin", "Stop — look-only". */
function lockLabel(lock: "locked" | "lookOnly" | null, label: string): string {
  return lock === "lookOnly" ? `${label} — look-only` : lock === "locked" ? `${label} — needs Admin` : label;
}

export function ToolButton({ icon, label, onClick, disabled, title, priority = 1, danger, active, locked, scope, action, busy }: ToolButtonProps) {
  const [working, setWorking] = useState(false);
  const pending = useRef(false);
  const lock = lockOf(locked, useAdminState());
  const isBusy = busy || working;
  const click = () => {
    // A ref, not state: three quick clicks arrive before React re-renders.
    if (pending.current || isBusy || lock === "lookOnly") return;
    const result = locked ? runTool({ label, onClick, locked, scope, action }) : onClick?.();
    if (!(result instanceof Promise)) return;
    pending.current = true;
    setWorking(true);
    const done = () => {
      pending.current = false;
      setWorking(false);
    };
    result.then(done, done);
  };
  return (
    <button
      type="button"
      className={`tool${danger ? " danger" : ""}${active ? " on" : ""}${lock ? " locked" : ""}`}
      onClick={click}
      disabled={disabled || lock === "lookOnly"}
      title={lockTip(lock, label, title)}
      aria-label={lockLabel(lock, label)}
      aria-pressed={active === undefined ? undefined : active}
      aria-busy={isBusy || undefined}
      data-priority={priority}
      data-danger={danger || undefined}
      data-lock={lock ?? undefined}
    >
      <Icon icon={isBusy ? LoaderCircle : icon} size={14} className={isBusy ? "spin" : undefined} />
      <span className="tool-label">{label}</span>
      {lock && <Icon icon={Lock} size={12} className="tool-lock" />}
    </button>
  );
}

/**
 * The leading chip of a toolbar whose actions need Admin: "Read-only" (a click unlocks), or
 * "Needs Touch ID" when Admin is on but they change this Mac. In look-only mode it reads
 * "Look only" and a click only says how to turn it off.
 */
export function LockChip({ scope = "node", macOnly = false }: { scope?: AdminScope; macOnly?: boolean }) {
  const s = useAdminState();
  if (s.lookOnly) {
    return (
      <button type="button" className="toolbar-lock" title={LOOK_ONLY_TIP} aria-label="Look only. Turn it off in Settings › Security & privacy" onClick={() => void showLookOnly()}>
        <Icon icon={Lock} size={12} />
        <span>Look only</span>
      </button>
    );
  }
  return (
    <button
      type="button"
      className="toolbar-lock"
      title={macOnly ? "Changes on this Mac need Touch ID or your password · click to confirm it's you" : `Read-only · ${NEEDS_ADMIN_TIP}`}
      aria-label={macOnly ? "Changes on this Mac need Touch ID. Confirm it's you" : "Read-only. Unlock Admin mode"}
      onClick={() => void unlock(undefined, scope)}
    >
      <Icon icon={Lock} size={12} />
      <span>{macOnly ? "Needs Touch ID" : "Read-only"}</span>
    </button>
  );
}

export function ToolSeparator() {
  return <span className="toolbar-sep" role="separator" aria-orientation="vertical" />;
}

/** Collects ToolButtons (through fragments) so the toolbar can list overflowed ones under More. */
function collectTools(children: ReactNode, out: ToolButtonProps[] = []): ToolButtonProps[] {
  Children.forEach(children, (c) => {
    if (!isValidElement(c)) return;
    const el = c as ReactElement<{ children?: ReactNode } & ToolButtonProps>;
    if (el.type === ToolButton) out.push(el.props);
    else if (el.props.children !== undefined && typeof el.type !== "string") collectTools(el.props.children, out);
  });
  return out;
}

/** Enabled items the narrow-window rules currently show in the More menu. */
function menuItems(menu: HTMLElement | null): HTMLButtonElement[] {
  return [...(menu?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])].filter((b) => getComputedStyle(b).display !== "none");
}

/**
 * How much a toolbar has to give to fit on one line, beyond what the window-width rules do:
 * 1 drops the labels of priority 2–3 buttons, 2 drops every label, 3 moves priority 3 into
 * More, 4 moves priority 2 there too.
 */
const FIT_MAX = 4;

/**
 * A window toolbar that never wraps: it measures itself and gives way in FIT_MAX steps,
 * moving buttons into a More menu at the end. When any button needs Admin, one leading chip
 * says so (LockChip: "Read-only", or "Look only"), and each such button shows the rule itself.
 */
export function Toolbar({ children, label }: { children: ReactNode; label?: string }) {
  const [open, setOpen] = useState(false);
  const barRef = useRef<HTMLDivElement>(null);
  const moreRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; right: number } | null>(null);
  const tools = collectTools(children);
  // Lower-priority tools and every destructive one (which never shows as a bare icon).
  const overflow = tools.filter((t) => (t.priority ?? 1) > 1 || t.danger);
  const lockedTool = tools.find((t) => t.locked);
  // Admin is on but these tools change this Mac: they wait for Touch ID, so "Read-only" would mislead.
  const adminOn = useAdmin();
  const admin = useAdminState();
  const macOnly = !!lockedTool && adminOn && lockedTool.scope === "mac";
  const [fit, setFit] = useState(0);

  // Measured, not guessed from the window width: a toolbar with twelve buttons needs to give
  // way sooner than one with three. Rendering only ever steps it up; a resize starts over.
  const overflows = (el: HTMLElement) => el.scrollWidth > el.clientWidth + 1;
  const settle = (el: HTMLElement, from: number) => {
    const mark = (n: number) => (n ? (el.dataset.fit = String(n)) : delete el.dataset.fit);
    let level = from;
    mark(level);
    while (level < FIT_MAX && overflows(el)) mark(++level);
    setFit(level);
  };
  useLayoutEffect(() => {
    const el = barRef.current;
    if (el && fit < FIT_MAX && overflows(el)) settle(el, fit);
  });
  useLayoutEffect(() => {
    const el = barRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    let width = el.clientWidth;
    const ro = new ResizeObserver(() => {
      if (el.clientWidth === width) return;
      width = el.clientWidth;
      settle(el, 0);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useLayoutEffect(() => {
    if (!open || !moreRef.current) return;
    const r = moreRef.current.getBoundingClientRect();
    setPos({ top: r.bottom + 4, right: window.innerWidth - r.right });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => {
      if (e.target instanceof Node && (menuRef.current?.contains(e.target) || moreRef.current?.contains(e.target))) return;
      setOpen(false);
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("blur", close);
    menuItems(menuRef.current)[0]?.focus();
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("blur", close);
    };
  }, [open]);

  const onMenuKey = (e: KeyboardEvent) => {
    const items = menuItems(menuRef.current);
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      e.stopPropagation();
      setOpen(false);
      moreRef.current?.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length];
      next?.focus();
    }
  };

  return (
    <div ref={barRef} className="toolbar" role="toolbar" aria-label={label} data-fit={fit || undefined}>
      {lockedTool && <LockChip scope={lockedTool.scope ?? "node"} macOnly={macOnly} />}
      {children}
      {overflow.length > 0 && (
        <>
          <button
            ref={moreRef}
            type="button"
            className="tool toolbar-more"
            data-p3={overflow.some((t) => t.priority === 3) || undefined}
            data-danger={overflow.some((t) => t.danger) || undefined}
            data-danger-low={overflow.some((t) => t.danger && (t.priority ?? 1) > 1) || undefined}
            aria-label="More"
            title="More"
            aria-haspopup="menu"
            aria-expanded={open}
            onClick={() => setOpen((o) => !o)}
          >
            <Icon icon={Ellipsis} size={14} />
          </button>
          {open && (
            <div
              ref={menuRef}
              className="toolbar-menu"
              role="menu"
              aria-label="More"
              onKeyDown={onMenuKey}
              style={pos ? { top: pos.top, right: pos.right } : { visibility: "hidden" }}
            >
              {overflow.map((t) => {
                const lock = lockOf(t.locked, admin);
                return (
                  <button
                    key={t.label}
                    type="button"
                    role="menuitem"
                    className={`toolbar-menu-item${t.danger ? " danger" : ""}`}
                    data-priority={t.priority ?? 1}
                    data-lock={lock ?? undefined}
                    disabled={t.disabled || t.busy || lock === "lookOnly"}
                    title={lock ? lockTip(lock, t.label, t.title) : t.title}
                    aria-label={lock ? lockLabel(lock, t.label) : undefined}
                    onClick={() => {
                      setOpen(false);
                      void runTool(t);
                    }}
                  >
                    <Icon icon={t.icon} size={14} />
                    <span>{t.label}</span>
                    {lock && <Icon icon={Lock} size={12} className="tool-lock" />}
                    {t.active && <Icon icon={Check} size={12} className="toolbar-menu-check" />}
                  </button>
                );
              })}
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** The window's filter box. ⌘F focuses it (data-find-target); Esc clears it. */
export function SearchField({ value, onChange, placeholder = "Filter", count, label }: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  /** Matching rows, shown while filtering. */
  count?: number;
  label?: string;
}) {
  return (
    <span className="search-field">
      <input
        type="search"
        value={value}
        placeholder={placeholder}
        aria-label={label ?? placeholder}
        data-find-target
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && value) {
            e.stopPropagation();
            onChange("");
          }
        }}
      />
      {value && count !== undefined && <span className="search-count" aria-live="polite">{count}</span>}
    </span>
  );
}

export function Spacer() {
  return <div className="spacer" />;
}

// ---------------------------------------------------------------- messages and states

export type NoticeTone = "info" | "warn" | "error" | "success";

const NOTICE_ICON: Record<NoticeTone, LucideIcon> = { info: Info, warn: TriangleAlert, error: CircleAlert, success: CircleCheck };

export function Notice({ children, tone = "info", onDismiss, onRetry, action }: {
  children: ReactNode;
  tone?: NoticeTone;
  onDismiss?: () => void;
  onRetry?: () => void;
  action?: { label: string; onClick: () => void };
}) {
  return (
    <div className={`notice ${tone}`} role={tone === "error" ? "alert" : "status"}>
      <Icon icon={NOTICE_ICON[tone]} size={14} className="notice-icon" />
      <div className="notice-body">{children}</div>
      {onRetry && (
        <button type="button" className="btn small" onClick={onRetry}>
          <Icon icon={RotateCw} size={12} />Retry
        </button>
      )}
      {action && <button type="button" className="btn small" onClick={action.onClick}>{action.label}</button>}
      {onDismiss && (
        <button type="button" className="notice-close" onClick={onDismiss} aria-label="Dismiss" title="Dismiss">
          <Icon icon={X} size={12} />
        </button>
      )}
    </div>
  );
}

/** Before the first data arrives. With `since`, says how long it has been. */
export function Waiting({ label = "Waiting for data…", since }: { label?: string; since?: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === undefined) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [since]);
  const secs = since !== undefined ? Math.max(0, Math.round((now - since) / 1000)) : 0;
  return (
    <div className="waiting" role="status">
      <Icon icon={LoaderCircle} size={14} className="spin" />
      <span>{label}</span>
      {secs >= 3 && <span className="muted">{secs} s</span>}
    </div>
  );
}

/** Skeleton rows while a list loads; the shimmer stops under reduced motion. */
export function LoadingState({ rows = 6, label = "Loading" }: { rows?: number; label?: string }) {
  return (
    <div className="loading-state" role="status" aria-label={label}>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="skeleton-row">
          <span className="skeleton-bar" style={{ width: `${18 + ((i * 37) % 22)}%` }} />
          <span className="skeleton-bar" style={{ width: `${30 + ((i * 53) % 30)}%` }} />
          <span className="skeleton-bar short" />
        </div>
      ))}
    </div>
  );
}

export function EmptyState({ icon, title, body, action }: {
  icon: LucideIcon;
  title: string;
  body?: ReactNode;
  action?: { label: string; onClick: () => void; disabled?: boolean };
}) {
  return (
    <div className="empty-state">
      <Icon icon={icon} size={32} className="empty-state-icon" />
      <div className="empty-state-title">{title}</div>
      {body && <div className="empty-state-body">{body}</div>}
      {action && <button type="button" className="btn" onClick={action.onClick} disabled={action.disabled}>{action.label}</button>}
    </div>
  );
}

// ---------------------------------------------------------------- choice controls

/** Moves focus and selection inside a group of buttons with arrow keys, Home and End. */
function rovingKey(e: KeyboardEvent, count: number, index: number, horizontalOnly: boolean): number | null {
  const prev = e.key === "ArrowLeft" || (!horizontalOnly && e.key === "ArrowUp");
  const next = e.key === "ArrowRight" || (!horizontalOnly && e.key === "ArrowDown");
  if (prev) return (index - 1 + count) % count;
  if (next) return (index + 1) % count;
  if (e.key === "Home") return 0;
  if (e.key === "End") return count - 1;
  return null;
}

export interface SegmentOption<T extends string> {
  value: T;
  label: ReactNode;
  icon?: LucideIcon;
  disabled?: boolean;
  title?: string;
}

/** A segmented control: one choice of a few, as a radio group (arrow keys move and choose). */
export function Segmented<T extends string>({ value, options, onChange, label }: {
  value: T;
  options: SegmentOption<T>[];
  onChange: (v: T) => void;
  label: string;
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const enabled = options.filter((o) => !o.disabled);
  const current = Math.max(0, options.findIndex((o) => o.value === value));
  const onKey = (e: KeyboardEvent) => {
    const at = enabled.findIndex((o) => o.value === value);
    const to = rovingKey(e, enabled.length, Math.max(0, at), false);
    if (to === null) return;
    e.preventDefault();
    const opt = enabled[to];
    onChange(opt.value);
    refs.current[options.indexOf(opt)]?.focus();
  };
  return (
    <div className="seg" role="radiogroup" aria-label={label} onKeyDown={onKey}>
      {options.map((o, i) => (
        <button
          key={o.value}
          ref={(el) => {
            refs.current[i] = el;
          }}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          tabIndex={i === current ? 0 : -1}
          className={o.value === value ? "on" : undefined}
          disabled={o.disabled}
          title={o.title ?? (o.icon && typeof o.label === "string" ? o.label : undefined)}
          onClick={() => onChange(o.value)}
        >
          {o.icon && <Icon icon={o.icon} size={14} />}
          {/* With an icon, the words can give way in a crowded toolbar (data-fit 4). */}
          {o.icon ? <span className="seg-label">{o.label}</span> : o.label}
        </button>
      ))}
    </div>
  );
}

export interface TabDef {
  id: string;
  label: ReactNode;
  badge?: ReactNode;
}

/**
 * WAI-ARIA tabs: ←/→, Home and End move between tabs and select them. Pass
 * `children` to render the selected panel, wired to its tab with aria-controls.
 */
export function Tabs({ tabs, value, onChange, label, children }: {
  tabs: TabDef[];
  value: string;
  onChange: (id: string) => void;
  label: string;
  children?: ReactNode;
}) {
  const base = useId();
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const current = Math.max(0, tabs.findIndex((t) => t.id === value));
  const tabId = (id: string) => `${base}-tab-${id}`;
  const panelId = (id: string) => `${base}-panel-${id}`;
  const onKey = (e: KeyboardEvent) => {
    const to = rovingKey(e, tabs.length, current, true);
    if (to === null) return;
    e.preventDefault();
    onChange(tabs[to].id);
    refs.current[to]?.focus();
  };
  return (
    <>
      <div className="tabs" role="tablist" aria-label={label} onKeyDown={onKey}>
        {tabs.map((t, i) => (
          <button
            key={t.id}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="tab"
            id={tabId(t.id)}
            aria-selected={t.id === value}
            aria-controls={panelId(t.id)}
            tabIndex={i === current ? 0 : -1}
            className={`tab${t.id === value ? " on" : ""}`}
            onClick={() => onChange(t.id)}
          >
            {t.label}
            {t.badge !== undefined && t.badge !== null && t.badge !== false && <span className="tab-badge">{t.badge}</span>}
          </button>
        ))}
      </div>
      {children !== undefined && (
        <div className="tab-body" role="tabpanel" id={panelId(value)} aria-labelledby={tabId(value)}>
          {children}
        </div>
      )}
    </>
  );
}

/** An on/off switch (role="switch"). The label is its accessible name; `showLabel` also prints it. */
export function Switch({ checked, onChange, label, disabled, description, id, showLabel }: {
  checked: boolean;
  onChange: (on: boolean) => void;
  label: string;
  disabled?: boolean;
  description?: string;
  id?: string;
  showLabel?: boolean;
}) {
  const button = (
    <button
      type="button"
      role="switch"
      id={id}
      className="switch"
      aria-checked={checked}
      aria-label={showLabel ? undefined : label}
      title={description}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span className="switch-thumb" />
    </button>
  );
  if (!showLabel) return button;
  return (
    <label className="switch-row">
      {button}
      <span>{label}</span>
    </label>
  );
}

// ---------------------------------------------------------------- small pieces

export function Badge({ tone = "neutral", children, title }: {
  tone?: "good" | "warn" | "crit" | "info" | "neutral";
  children: ReactNode;
  title?: string;
}) {
  return <span className={`badge ${tone}`} title={title}>{children}</span>;
}

/** A key cap, e.g. <Kbd>⌘</Kbd><Kbd>K</Kbd>. */
export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

/** A settings-style card of rows (label left, control right) with a title above and a footnote below. */
export function FormGroup({ title, footnote, children }: { title?: ReactNode; footnote?: ReactNode; children: ReactNode }) {
  return (
    <section className="form-group">
      {title && <h3 className="section-title">{title}</h3>}
      <div className="form-group-card">{children}</div>
      {footnote && <p className="form-footnote">{footnote}</p>}
    </section>
  );
}

export function FormRow({ label, htmlFor, hint, children }: { label: ReactNode; htmlFor?: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="fg-row">
      <div className="fg-label">
        {htmlFor ? <label htmlFor={htmlFor}>{label}</label> : <span>{label}</span>}
        {hint && <span className="fg-hint">{hint}</span>}
      </div>
      <div className="fg-control">{children}</div>
    </div>
  );
}

/** Read-only text such as a command or a log: copy button, wrap toggle, capped height. */
export function CodeBlock({ text, wrap: wrapDefault = true, maxHeight = 320, label }: {
  text: string;
  wrap?: boolean;
  maxHeight?: number;
  label?: string;
}) {
  const [wrap, setWrap] = useState(wrapDefault);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(t);
  }, [copied]);
  const copy = () => {
    navigator.clipboard?.writeText(text).then(() => setCopied(true), () => setCopied(false));
  };
  return (
    <div className="codeblock">
      <div className="codeblock-head">
        <span className="codeblock-label">{label}</span>
        <button type="button" className="codeblock-btn" aria-pressed={wrap} onClick={() => setWrap((w) => !w)} title={wrap ? "Don't wrap lines" : "Wrap long lines"}>
          <Icon icon={WrapText} size={12} />Wrap
        </button>
        <button type="button" className="codeblock-btn" onClick={copy} title="Copy to the clipboard">
          <Icon icon={copied ? Check : Copy} size={12} />{copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre className={`codeblock-pre${wrap ? " wrap" : ""}`} style={{ maxHeight }} tabIndex={0}>{text}</pre>
    </div>
  );
}

// ---------------------------------------------------------------- prose in popovers, clipping

/**
 * An ⓘ button that opens a short explanation in a popover (§M5: prose lives here, not in
 * footers). Click, Enter or Space opens it; Esc, a click outside or Tab away closes it and
 * focus returns to the button.
 *
 * With a `topic` (docs/help: a tip id, an article id, or "article#heading") it becomes a Help
 * tip: below the text it lists How-To links and Read the full guide, and with no children it
 * takes its title and text from the topic.
 *
 * It isn't a <button> on purpose: a read-only form is a disabled <fieldset>, which would turn
 * off every button inside it, and explanations must stay readable in look-only mode.
 */
export function InfoTip({ children, label, topic }: { children?: ReactNode; label?: string; topic?: string }) {
  const [open, setOpen] = useState(false);
  const btnRef = useRef<HTMLSpanElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const id = useId();
  const meta = useHelpMeta();
  const tip = topic && meta ? meta.resolveTopic(topic) : null;
  // Named for what it explains, not for where it sits: a column's icon can explain something else.
  const name = label ?? (tip ? `About ${tip.name}` : "About this");

  useEffect(() => {
    if (topic) preloadHelpMeta();
  }, [topic]);
  useEffect(() => {
    if (open && topic) void loadHelpMeta().catch(() => {});
  }, [open, topic]);

  useLayoutEffect(() => {
    if (!open) return;
    const b = btnRef.current?.getBoundingClientRect();
    const pop = popRef.current;
    if (!b || !pop) return;
    const w = pop.offsetWidth;
    const h = pop.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const left = Math.max(8, Math.min(b.left - 10, vw - w - 8));
    const below = b.bottom + 6;
    const top = below + h > vh - 8 && b.top - h - 6 > 8 ? b.top - h - 6 : below;
    setPos({ left, top: Math.max(8, Math.min(top, vh - h - 8)) });
  }, [open, !!tip]);

  // A tip has links in it, so the keyboard needs to get to them. Once it's placed: until then
  // it's hidden, and a hidden element can't take focus.
  useEffect(() => {
    if (open && topic && pos) popRef.current?.focus({ preventScroll: true });
  }, [open, topic, !!pos]);

  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => {
      if (e.target instanceof Node && (popRef.current?.contains(e.target) || btnRef.current?.contains(e.target))) return;
      setOpen(false);
    };
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      setOpen(false);
      btnRef.current?.focus();
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("focusin", close, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("focusin", close, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("blur", close);
    };
  }, [open]);

  const toggle = () => setOpen((o) => !o);
  // Whatever it sits in (a sortable header, a label, a title bar) doesn't get the click.
  const hold = (e: { stopPropagation(): void }) => e.stopPropagation();
  const guide = (to: string) => {
    setOpen(false);
    // Help opens on the desk: a panel over it (Keyboard shortcuts) has to get out of the way.
    if (btnRef.current?.closest("[data-shell-overlay]")) closePanel();
    openHelp(to);
  };

  return (
    <>
      <span
        ref={btnRef}
        role="button"
        tabIndex={0}
        className={`infotip${open ? " on" : ""}`}
        aria-label={name}
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        title={open ? undefined : name}
        onClick={(e) => {
          e.stopPropagation();
          e.preventDefault();
          toggle();
        }}
        onKeyDown={(e) => {
          if (e.key !== "Enter" && e.key !== " ") return;
          e.stopPropagation();
          e.preventDefault();
          toggle();
        }}
        onPointerDown={hold}
        onMouseDown={hold}
        onDoubleClick={hold}
      >
        <Icon icon={Info} size={14} />
      </span>
      {open && createPortal(
        <div
          ref={popRef}
          id={id}
          className={`infotip-pop${btnRef.current?.closest("[data-shell-overlay], .sb-overlay") ? " over-panel" : ""}`}
          role="dialog"
          aria-label={name}
          tabIndex={-1}
          style={pos ? { left: pos.left, top: pos.top } : { visibility: "hidden", left: 0, top: 0 }}
        >
          {children ?? (tip && <><p className="helptip-title">{tip.title}</p><p>{tip.body}</p></>)}
          {topic && !tip && !children && <p>{meta ? "This guide isn't available." : "Loading…"}</p>}
          {tip && tip.howtos.length > 0 && (
            <ul className="helptip-links" aria-label="How-Tos">
              {tip.howtos.map((h) => (
                <li key={h.id}><button type="button" className="helptip-link" onClick={() => guide(h.id)}>{h.title}</button></li>
              ))}
            </ul>
          )}
          {tip && (
            <button type="button" className="helptip-link helptip-more" onClick={() => guide(tip.anchor ? `${tip.article}#${tip.anchor}` : tip.article)}>
              Read the full guide
            </button>
          )}
        </div>,
        document.body,
      )}
    </>
  );
}

/** A Help tip: the same quiet ⓘ everywhere. Its words live in docs/help, not in code. */
export function HelpTip({ topic, label }: { topic: string; label?: string }) {
  return <InfoTip topic={topic} label={label} />;
}

/**
 * A section heading with an optional ⓘ explanation and actions on the right. The title is
 * set in small capitals; `suffix` follows it in normal case, so units and identifiers keep
 * theirs ("Half 2" + "192.168.101.x", "Fit on 4 Sparks" + "120 GiB memory each").
 * `infoTopic` makes the ⓘ a Help tip, with or without `info`.
 */
export function SectionTitle({ title, suffix, info, infoTopic, actions, id }: { title: ReactNode; suffix?: ReactNode; info?: ReactNode; infoTopic?: string; actions?: ReactNode; id?: string }) {
  return (
    <div className="section-head">
      <h3 className="section-title" id={id}>{title}{suffix != null && suffix !== "" && <span className="section-suffix">{suffix}</span>}</h3>
      {(info || infoTopic) && <InfoTip topic={infoTopic} label={infoTopic ? undefined : typeof title === "string" ? `About ${title}` : info ? "About this section" : undefined}>{info}</InfoTip>}
      {actions && <span className="section-actions">{actions}</span>}
    </div>
  );
}

/** One line, cut at the end with "…", the full text in the tooltip. */
export function Truncate({ text, title, className }: { text: string; title?: string; className?: string }) {
  return <span className={`truncate${className ? ` ${className}` : ""}`} title={title ?? text}>{text}</span>;
}

/**
 * One line cut in the middle, so the end (a file name, an image tag, a port) stays:
 * "/home/sparkuser/…/config.json". `keepEnd` is how many characters always show at the end;
 * by default the last path segment (up to 24) or the last 8 characters.
 */
export function MiddleEllipsis({ text, keepEnd, title, className }: { text: string; keepEnd?: number; title?: string; className?: string }) {
  const slash = text.lastIndexOf("/");
  const colon = text.lastIndexOf(":");
  const cut = Math.max(slash, colon);
  const fallback = cut > 0 && text.length - cut <= 25 ? text.length - cut : Math.min(8, Math.floor(text.length / 2));
  const n = Math.max(0, Math.min(text.length, keepEnd ?? fallback));
  const head = text.slice(0, text.length - n);
  const tail = text.slice(text.length - n);
  return (
    <span className={`mid-ellipsis${className ? ` ${className}` : ""}`} title={title ?? text} aria-label={text}>
      <span className="mid-ellipsis-head" aria-hidden>{head}</span>
      <span className="mid-ellipsis-tail" aria-hidden>{tail}</span>
    </span>
  );
}
