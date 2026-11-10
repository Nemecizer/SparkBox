// Window geometry as pure functions, so clamping, snapping and arranging can be
// tested without a DOM. Coordinates are desktop pixels; (0, 0) is its top left.

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Size {
  w: number;
  h: number;
}

export type SnapZone = "left" | "right" | "top-left" | "top-right" | "bottom-left" | "bottom-right" | "max";

/** Height of an inner window's title bar. */
export const TITLE_H = 30;
/** How much of the title bar stays on the desktop horizontally, so it can always be grabbed. */
export const GRAB_W = 120;
/** Dragging the pointer this close to an edge offers to snap there. */
export const SNAP_EDGE = 12;
/** Along a side edge, this close to a corner snaps to a quarter instead of a half. */
export const SNAP_CORNER = 64;
/** The row of minimized windows at the bottom of the desktop. */
export const DOCK_H = 30;
export const CASCADE_STEP = 26;
/** The margin a window keeps from the desk's right and bottom edges, as the cascade starts from the left and top. */
export const ORIGIN = 12;
const GAP = 4;

/** Keeps a title bar reachable: at least GRAB_W px of it inside horizontally, all of it vertically. */
export function clampRect(r: Rect, desk: Size): Rect {
  const minX = Math.min(0, GRAB_W - r.w);
  const maxX = Math.max(0, desk.w - GRAB_W);
  const maxY = Math.max(0, desk.h - TITLE_H);
  return { ...r, x: Math.round(Math.min(maxX, Math.max(minX, r.x))), y: Math.round(Math.min(maxY, Math.max(0, r.y))) };
}

/** Shrinks a window that no longer fits (never below its minimum), then clamps its position. */
export function fitRect(r: Rect, desk: Size, min: [number, number] = [0, 0]): Rect {
  const w = Math.max(min[0], Math.min(r.w, desk.w));
  const h = Math.max(min[1], Math.min(r.h, desk.h));
  return clampRect({ ...r, w, h }, desk);
}

/**
 * For a window coming back (a restart, a reopen, a restore) or a desk that got smaller, the
 * way macOS keeps windows in the visible frame (C4): moved in first, then shrunk toward its
 * minimum, so it lies wholly on the desk with ORIGIN px to spare on the right and below, never
 * flush with an edge or past it. Only a desk smaller than the minimum lets the minimum win
 * (its title bar stays reachable).
 */
export function fitInside(r: Rect, desk: Size, min: [number, number] = [0, 0]): Rect {
  const w = Math.round(Math.max(min[0], Math.min(r.w, desk.w - 2 * ORIGIN)));
  const h = Math.round(Math.max(min[1], Math.min(r.h, desk.h - 2 * ORIGIN)));
  const x = Math.round(Math.max(0, Math.min(r.x, desk.w - ORIGIN - w)));
  const y = Math.round(Math.max(0, Math.min(r.y, desk.h - ORIGIN - h)));
  return clampRect({ x, y, w, h }, desk);
}

/** Whether a frame lies wholly on the desk with the ORIGIN margin on the right and below. */
export function onTheDesk(r: Rect, desk: Size): boolean {
  return r.x >= 0 && r.y >= 0 && r.x + r.w <= desk.w - ORIGIN && r.y + r.h <= desk.h - ORIGIN;
}

/** Which snap zone a pointer at (px, py), relative to the desktop, is offering. */
export function snapZoneAt(px: number, py: number, desk: Size): SnapZone | null {
  const left = px <= SNAP_EDGE;
  const right = px >= desk.w - SNAP_EDGE;
  if (left || right) {
    const side = left ? "left" : "right";
    if (py <= SNAP_CORNER) return `top-${side}`;
    if (py >= desk.h - SNAP_CORNER) return `bottom-${side}`;
    return side;
  }
  if (py <= SNAP_EDGE) return "max";
  return null;
}

/** The rectangle a snap zone fills. */
export function zoneRect(zone: SnapZone, desk: Size): Rect {
  const hw = Math.floor(desk.w / 2);
  const hh = Math.floor(desk.h / 2);
  switch (zone) {
    case "left": return { x: 0, y: 0, w: hw, h: desk.h };
    case "right": return { x: hw, y: 0, w: desk.w - hw, h: desk.h };
    case "top-left": return { x: 0, y: 0, w: hw, h: hh };
    case "top-right": return { x: hw, y: 0, w: desk.w - hw, h: hh };
    case "bottom-left": return { x: 0, y: hh, w: hw, h: desk.h - hh };
    case "bottom-right": return { x: hw, y: hh, w: desk.w - hw, h: desk.h - hh };
    case "max": return { x: 0, y: 0, w: desk.w, h: desk.h };
  }
}

export interface Arrangeable {
  id: string;
  size: [number, number];
  min: [number, number];
}

/**
 * Cascade: each window steps down and right from the origin, at its default size but no
 * more than 75% of the desktop. Near the right or bottom edge it gets narrower (or shorter)
 * rather than coming closer to that edge than the cascade starts from the left and top. Once
 * it would shrink below 60% of that size (or its minimum), a new lap starts back at the top,
 * a little further right.
 */
export function cascadeLayout(wins: Arrangeable[], desk: Size): Map<string, Rect> {
  const out = new Map<string, Rect>();
  let x = ORIGIN;
  let y = ORIGIN;
  let lap = 0;
  for (const win of wins) {
    const capW = Math.min(win.size[0], Math.round(desk.w * 0.75));
    const capH = Math.min(win.size[1], Math.round(desk.h * 0.75));
    const floorW = Math.max(win.min[0], Math.round(capW * 0.6));
    const floorH = Math.max(win.min[1], Math.round(capH * 0.6));
    const fit = () => ({ w: Math.min(capW, desk.w - x - ORIGIN), h: Math.min(capH, desk.h - y - ORIGIN) });
    let { w, h } = fit();
    if ((w < floorW || h < floorH) && (x !== ORIGIN || y !== ORIGIN)) {
      lap++;
      x = ORIGIN + (lap % 4) * CASCADE_STEP * 2;
      y = ORIGIN;
      ({ w, h } = fit());
    }
    // A desktop too small for the window's minimum: the minimum wins, kept reachable.
    out.set(win.id, clampRect({ x, y, w: Math.max(win.min[0], w), h: Math.max(win.min[1], h) }, desk));
    x += CASCADE_STEP;
    y += CASCADE_STEP;
  }
  return out;
}

function cell(x: number, y: number, w: number, h: number): Rect {
  return { x: Math.round(x + GAP / 2), y: Math.round(y + GAP / 2), w: Math.round(w - GAP), h: Math.round(h - GAP) };
}

/** `n` cells in `cols` columns; a short last row shares the full width. */
function gridCells(n: number, cols: number, desk: Size): Rect[] {
  const rows = Math.ceil(n / cols);
  const out: Rect[] = [];
  for (let r = 0; r < rows; r++) {
    const inRow = r === rows - 1 ? n - cols * (rows - 1) : cols;
    for (let c = 0; c < inRow; c++) out.push(cell((c * desk.w) / inRow, (r * desk.h) / rows, desk.w / inRow, desk.h / rows));
  }
  return out;
}

/** The cells a tile of `n` windows uses: 2 side by side, 3 as one tall and two stacked, 4 as 2×2, 5–6 as 3×2, more as a grid. */
export function tileCells(n: number, desk: Size): Rect[] {
  if (n <= 0) return [];
  const W = desk.w;
  const H = desk.h;
  if (n === 1) return [cell(0, 0, W, H)];
  if (n === 2) return [cell(0, 0, W / 2, H), cell(W / 2, 0, W / 2, H)];
  if (n === 3) return [cell(0, 0, W / 2, H), cell(W / 2, 0, W / 2, H / 2), cell(W / 2, H / 2, W / 2, H / 2)];
  return gridCells(n, n <= 4 ? 2 : n <= 6 ? 3 : Math.ceil(Math.sqrt(n)), desk);
}

export interface TileWin {
  id: string;
  /** Its default size: what it would like to get. */
  size: [number, number];
  /** The smallest it may be tiled at (the kind's tileMin). */
  min: [number, number];
}

export interface TileOptions {
  /** Space between windows. */
  gutter?: number;
  /** Space between the windows and the desktop's edges. */
  margin?: number;
  /** Tile at most this many, the most recently focused first. */
  max?: number;
  /**
   * "row": the two most recent side by side, stacked top and bottom when they can't sit
   * across, never one of them docked. "grid": four as 2×2 when they fit.
   */
  shape?: "auto" | "row" | "grid";
}

export interface TileResult {
  frames: Map<string, Rect>;
  /** Windows that didn't fit at their minimum and go to the dock instead. */
  minimized: string[];
  /** Side by side that didn't fit across: the two are stacked top and bottom instead. */
  stacked?: boolean;
}

/** Sizes along one axis: equal shares, except that none goes below its minimum. Sums to `total`. */
export function shareOut(total: number, mins: number[]): number[] {
  const n = mins.length;
  const fixed = new Array<boolean>(n).fill(false);
  let share = total / n;
  for (;;) {
    let used = 0;
    let free = 0;
    for (let i = 0; i < n; i++) {
      if (fixed[i]) used += mins[i];
      else free++;
    }
    share = free ? (total - used) / free : 0;
    let changed = false;
    for (let i = 0; i < n; i++) {
      if (!fixed[i] && mins[i] > share) {
        fixed[i] = true;
        changed = true;
      }
    }
    if (!changed) break;
  }
  const out = mins.map((m, i) => (fixed[i] ? m : Math.floor(share)));
  // Whole pixels: the remainder goes to the last free one (or the last one).
  const last = fixed.lastIndexOf(false);
  out[last >= 0 ? last : n - 1] += total - out.reduce((a, b) => a + b, 0);
  return out;
}

interface Packing {
  frames: Map<string, Rect>;
  score: number;
}

/**
 * Splits windows into `lines` rows (or columns) for `axis` 0 (rows: windows side by side)
 * or 1 (columns: windows stacked). "balanced" keeps the counts even, "fit" fills a line
 * before starting the next. Null when a line would be empty.
 */
function distribute(wins: TileWin[], lines: number, axis: 0 | 1, how: "balanced" | "fit", room: number, gutter: number): TileWin[][] | null {
  const out: TileWin[][] = Array.from({ length: lines }, () => []);
  const sum = (l: TileWin[]) => l.reduce((a, w) => a + w.min[axis], 0) + gutter * Math.max(0, l.length - 1);
  const cap = Math.ceil(wins.length / lines);
  for (const w of [...wins].sort((a, b) => b.min[axis] - a.min[axis])) {
    let pick = -1;
    if (how === "balanced") {
      for (let i = 0; i < lines; i++) if (out[i].length < cap && (pick < 0 || sum(out[i]) < sum(out[pick]))) pick = i;
    } else {
      pick = out.findIndex((l) => sum(l) + (l.length ? gutter : 0) + w.min[axis] <= room);
      if (pick < 0) for (let i = 0; i < lines; i++) if (pick < 0 || sum(out[i]) < sum(out[pick])) pick = i;
    }
    out[pick].push(w);
  }
  return out.some((l) => !l.length) ? null : out;
}

/** Lays out lines of windows in `inner`, or null when some window would get less than its minimum. */
function place(lines: TileWin[][], axis: 0 | 1, inner: Size, gutter: number, rank: Map<string, number>): Packing | null {
  const along = axis === 0 ? inner.w : inner.h;
  const across = axis === 0 ? inner.h : inner.w;
  const cross = axis === 0 ? 1 : 0;
  // The most recently used window comes first: top left.
  const ordered = lines
    .map((l) => [...l].sort((a, b) => rank.get(a.id)! - rank.get(b.id)!))
    .sort((a, b) => rank.get(a[0].id)! - rank.get(b[0].id)!);
  const needs = ordered.map((l) => Math.max(...l.map((w) => w.min[cross])));
  const crossRoom = across - gutter * (ordered.length - 1);
  if (needs.reduce((a, b) => a + b, 0) > crossRoom) return null;
  const crossSizes = shareOut(crossRoom, needs);
  const frames = new Map<string, Rect>();
  let score = 0;
  let c = 0;
  for (let li = 0; li < ordered.length; li++) {
    const line = ordered[li];
    const room = along - gutter * (line.length - 1);
    const mins = line.map((w) => w.min[axis]);
    if (mins.reduce((a, b) => a + b, 0) > room) return null;
    const sizes = shareOut(room, mins);
    let a = 0;
    line.forEach((w, i) => {
      const r = axis === 0 ? { x: a, y: c, w: sizes[i], h: crossSizes[li] } : { x: c, y: a, w: crossSizes[li], h: sizes[i] };
      frames.set(w.id, r);
      // How much of the size it would like it gets.
      const pw = Math.min(w.size[0], inner.w);
      const ph = Math.min(w.size[1], inner.h);
      score += Math.min(1, r.w / pw) * Math.min(1, r.h / ph);
      a += sizes[i] + gutter;
    });
    c += crossSizes[li] + gutter;
  }
  return { frames, score };
}

/** The best way to tile all of `wins` in `inner`, or null when they can't all get their minimum. */
function pack(wins: TileWin[], inner: Size, gutter: number, shape: "auto" | "row" | "grid", rank: Map<string, number>): Packing | null {
  let best: Packing | null = null;
  const axes: (0 | 1)[] = shape === "row" ? [0] : [0, 1];
  for (const axis of axes) {
    const room = axis === 0 ? inner.w : inner.h;
    // A grid is two lines of two; a row is one line.
    const [from, to] = shape === "row" ? [1, 1] : shape === "grid" ? [2, 2] : [1, wins.length];
    for (let lines = from; lines <= to; lines++) {
      for (const how of ["balanced", "fit"] as const) {
        const split = distribute(wins, lines, axis, how, room, gutter);
        const p = split && place(split, axis, inner, gutter, rank);
        if (p && (!best || p.score > best.score + 1e-9)) best = p;
      }
    }
  }
  return best;
}

/**
 * Tile: windows never overlap and each gets at least its tile minimum. `wins` come most
 * recently focused first; the most of them that fit (in that order) are tiled in rows or
 * columns with gutters between them, picking the arrangement that gives each the most of its
 * default size. The rest are listed in `minimized`, to go to the dock.
 *
 * Side by side ("row") always gives the two most recent a frame each: across when they fit,
 * else stacked top and bottom, else (a desktop smaller than both minimums) halves of it.
 */
export function tileLayout(wins: TileWin[], desk: Size, opts: TileOptions = {}): TileResult {
  const gutter = opts.gutter ?? 8;
  const margin = opts.margin ?? 8;
  const inner = { w: Math.max(0, desk.w - 2 * margin), h: Math.max(0, desk.h - 2 * margin) };
  const rank = new Map(wins.map((w, i) => [w.id, i]));
  const cap = Math.min(wins.length, opts.max ?? wins.length);
  const shift = (frames: Map<string, Rect>) =>
    new Map([...frames].map(([id, r]) => [id, { x: r.x + margin, y: r.y + margin, w: r.w, h: r.h }]));
  if (opts.shape === "row" && cap >= 2) {
    const two = wins.slice(0, 2);
    const minimized = wins.slice(2).map((w) => w.id);
    const across = pack(two, inner, gutter, "row", rank);
    if (across) return { frames: shift(across.frames), minimized };
    const down = place([two], 1, inner, gutter, rank);
    if (down) return { frames: shift(down.frames), minimized, stacked: true };
    // Smaller than both minimums either way: halves of the desktop, along its longer side.
    const halves = place([two.map((w) => ({ ...w, min: [0, 0] as [number, number] }))], inner.w >= inner.h ? 0 : 1, inner, gutter, rank)!;
    return { frames: shift(halves.frames), minimized, stacked: inner.w < inner.h };
  }
  if (opts.shape === "grid" && cap >= 4) {
    const p = pack(wins.slice(0, 4), inner, gutter, "grid", rank);
    if (p) return { frames: shift(p.frames), minimized: wins.slice(4).map((w) => w.id) };
  }
  for (let k = cap; k >= 1; k--) {
    const p = pack(wins.slice(0, k), inner, gutter, opts.shape === "row" ? "row" : "auto", rank);
    if (p) return { frames: shift(p.frames), minimized: wins.slice(k).map((w) => w.id) };
  }
  if (!wins.length) return { frames: new Map(), minimized: [] };
  // Not even the front window fits at its minimum: it gets the whole desktop on its own.
  return { frames: new Map([[wins[0].id, { x: margin, y: margin, w: inner.w, h: inner.h }]]), minimized: wins.slice(1).map((w) => w.id) };
}

/** How close two windows' top-left corners may be before a new one cascades off the other. */
export const CASCADE_NEAR = 8;

/**
 * A new or restored window whose top-left lies within CASCADE_NEAR of an open window's steps
 * down and right by CASCADE_STEP until it's free (C4). Each step stays wholly on the desk with
 * the ORIGIN margin: the frame narrows (or shortens) toward its minimum to fit there, and once
 * even the minimum wouldn't fit the cascade wraps back to ORIGIN for a new lap, a little
 * further right. With no free spot anywhere it opens at ORIGIN, on the desk, over the others.
 */
export function cascadeFree(r: Rect, taken: { x: number; y: number }[], desk: Size, min: [number, number] = [0, 0]): Rect {
  const free = (x: number, y: number) => !taken.some((t) => Math.abs(t.x - x) <= CASCADE_NEAR && Math.abs(t.y - y) <= CASCADE_NEAR);
  const at = (x: number, y: number): Rect | null => {
    const w = Math.min(r.w, desk.w - ORIGIN - x);
    const h = Math.min(r.h, desk.h - ORIGIN - y);
    return w >= min[0] && h >= min[1] && w > 0 && h > 0 ? { x, y, w, h } : null;
  };
  if (free(r.x, r.y) && onTheDesk(r, desk)) return r;
  for (let lap = 0; lap < 8; lap++) {
    let x = lap ? ORIGIN + lap * CASCADE_STEP * 2 : r.x;
    let y = lap ? ORIGIN : r.y;
    for (let i = 0; i < 40; i++) {
      const spot = at(x, y);
      if (!spot) break;
      if (free(x, y)) return spot;
      x += CASCADE_STEP;
      y += CASCADE_STEP;
    }
  }
  // Nowhere free on the desk: at the origin, fitted to it, over what's there.
  return fitInside({ ...r, x: ORIGIN, y: ORIGIN }, desk, min);
}

/**
 * Where a window opens (C4): at its saved frame brought onto the desk, or stepped from the
 * origin by how many are open; then off any open window's corner by cascadeFree. Always wholly
 * on the desk with the ORIGIN margin, no smaller than its minimum unless the desk is.
 */
export function openFrame(size: [number, number], min: [number, number], saved: Rect | null | undefined, open: number, taken: { x: number; y: number }[], desk: Size): Rect {
  const placed = saved ? fitInside(saved, desk, min) : placeNew(size, min, open, desk);
  return cascadeFree(placed, taken, desk, min);
}

/** Renumbers stacking order to 1..n, keeping the order. */
export function normalizeZ<T extends { id: string; z: number }>(wins: T[]): T[] {
  const order = [...wins].sort((a, b) => a.z - b.z).map((w) => w.id);
  return wins.map((w) => ({ ...w, z: order.indexOf(w.id) + 1 }));
}

/** Where a new window opens: stepped from the origin by how many are open, wrapped to stay on the desktop. */
export function placeNew(size: [number, number], min: [number, number], open: number, desk: Size): Rect {
  // Leave the same margin on the far side as the near one, so a big window never sits flush on an edge.
  const w = Math.max(min[0], Math.min(size[0], desk.w - 2 * ORIGIN));
  const h = Math.max(min[1], Math.min(size[1], desk.h - 2 * ORIGIN));
  const room = Math.max(1, Math.floor(Math.min(desk.w - w, desk.h - h) / CASCADE_STEP));
  const step = (open % Math.min(8, room + 1)) * CASCADE_STEP + ORIGIN;
  return clampRect({ x: Math.min(step, Math.max(0, desk.w - w - ORIGIN)), y: Math.min(step, Math.max(0, desk.h - h - ORIGIN)), w, h }, desk);
}

/** Overview (F3): where a window of size `r` sits, scaled into `cell`, and where its label goes. */
export function overviewPlacement(cell: Rect, r: Rect): { dx: number; dy: number; scale: number; labelY: number } {
  const LABEL = 30;
  const scale = Math.min(1, (cell.w - 12) / r.w, (cell.h - LABEL - 6) / r.h);
  const top = cell.y + (cell.h - LABEL - r.h * scale) / 2;
  return {
    dx: Math.round(cell.x + (cell.w - r.w * scale) / 2 - r.x),
    dy: Math.round(top - r.y),
    scale,
    labelY: Math.round(top + r.h * scale + 6),
  };
}
