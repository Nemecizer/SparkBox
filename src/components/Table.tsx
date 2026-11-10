// A pro list in the WinBox / Activity Monitor mould: sortable (naturally),
// keyboard-driven, single or multi-select, resizable and hideable columns that
// get out of the way in narrow windows, a count footer, and per-table memory of
// sort, widths and hidden columns.
import {
  memo, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState,
  type KeyboardEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode,
} from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import { showContextMenu, type MenuItem } from "../lib/contextMenu";
import { Icon } from "./Icon";
import { HelpTip, MiddleEllipsis } from "./ui";

export interface Column<T> {
  key: string;
  label: string;
  /** A Help tip beside the header: a topic from docs/help. It never sorts the column. */
  help?: string;
  render: (row: T) => ReactNode;
  /** Present when the column can be sorted by clicking its header. */
  sort?: (row: T) => number | string;
  align?: "left" | "right" | "center";
  width?: number | string;
  /** Narrowest the column can be dragged (default 48). */
  minWidth?: number;
  /**
   * 1 never auto-hides. When the table runs out of room, priority-3 columns hide one at a
   * time, the rightmost first, then priority-2 ones the same way.
   */
  priority?: 1 | 2 | 3;
  /**
   * A short form of the cell ("c1", "gx10" for a node) drawn when even the priority-1 columns
   * don't fit at full width, before the table scrolls sideways. The full form is its tooltip
   * when the column has no `title`.
   */
  narrow?: (row: T) => ReactNode;
  /** Whether the header menu can hide it (default: every column but the first). */
  hideable?: boolean;
  /** Tooltip for the cell; by default the cell's own text when it's cut. */
  title?: (row: T) => string;
  /**
   * "content" (ids, numbers, short states) never shrinks below its widest cell; "flex"
   * (commands, paths, images) gives way first, with an ellipsis. Default: "content" for
   * right-aligned columns and columns 104px or narrower, else "flex".
   */
  fit?: "content" | "flex";
  /** A narrow header ("CPU" for "CPU usage") used when the full label won't fit; the full one is its tooltip. */
  short?: string;
  /** Where a cut cell's ellipsis goes: "end" (default) or "middle" for paths and image names. */
  ellipsis?: "end" | "middle";
  /**
   * "message": the sentence that explains a problem ("Root disk 85% full (143 GiB free)"). It
   * keeps its full text up to 48 characters: priority-3, then priority-2 columns hide before
   * it gives up any width (§2.5).
   */
  kind?: "message";
  /**
   * Keys of the columns right after this one that the cell may take over for a row (an
   * offline node's status spans its empty metric cells). Only visible neighbours, in order.
   */
  absorb?: (row: T) => string[] | null | undefined;
}

export interface RowAction {
  label: string;
  /** Shown in the context menu; pressed while the grid has focus it runs the action, e.g. "⌘⌫" or "Space". */
  shortcut?: string;
  run: () => unknown;
  danger?: boolean;
  disabled?: boolean | string;
}

export type SortState = { key: string; dir: 1 | -1 } | null;

export interface TableProps<T> {
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  /** Legacy single selection. */
  selected?: string | null;
  onSelect?: (key: string) => void;
  /** Double-click or Enter: the row's default action. */
  onActivate?: (row: T) => void;
  /** Actions on a row: listed in its context menu, and their shortcuts work while the grid has focus. */
  rowActions?: (row: T) => RowAction[];
  empty?: ReactNode;
  initialSort?: SortState;
  /** Controlled selection (keys). */
  selection?: ReadonlySet<string>;
  onSelectionChange?: (keys: Set<string>, primary: string | null) => void;
  selectionMode?: "single" | "multi";
  /** A checkbox column (multi mode). */
  checkboxes?: boolean;
  onRowContextMenu?: (row: T, e: ReactMouseEvent, selectedKeys: Set<string>) => void;
  /** Controlled sort. */
  sort?: SortState;
  onSortChange?: (s: SortState) => void;
  /** Remembers sort, widths and hidden columns in localStorage['sparkbox.table.<id>']. */
  tableId?: string;
  /** Rows that stay on top under every sort (e.g. folders). */
  pinFirst?: (row: T) => boolean;
  rowClass?: (row: T) => string | undefined;
  /**
   * A second line for a row, spanning every column under it (a problem row's full sentence,
   * §2.5): part of the same row for selection, hover, the problem rule and the zebra.
   */
  rowDetail?: (row: T) => ReactNode | null | undefined;
  /** `true` shows "6 items · 1 selected"; a node shows that plus the node on the right. */
  footer?: ReactNode | true;
  /** Singular and plural for the footer, e.g. ["container", "containers"]. */
  itemNoun?: [string, string];
  /** ⌘⌫ / Delete on the selection. */
  onKeyAction?: (action: "delete", rows: T[]) => void;
  ariaLabel?: string;
}

interface Saved {
  sort?: SortState;
  widths?: Record<string, number>;
  hidden?: string[];
}

const STORE_PREFIX = "sparkbox.table.";
const TYPEAHEAD_MS = 500;
const MIN_COL = 48;
/** Columns up to this wide are never squeezed to fit the window. */
const FIT_KEEP = 104;

function loadSaved(id?: string): Saved {
  if (!id) return {};
  try {
    const v = JSON.parse(localStorage.getItem(STORE_PREFIX + id) ?? "{}");
    return v && typeof v === "object" ? (v as Saved) : {};
  } catch {
    return {};
  }
}

function storeSaved(id: string | undefined, s: Saved) {
  if (!id) return;
  try {
    localStorage.setItem(STORE_PREFIX + id, JSON.stringify(s));
  } catch {
    // Persistence is a convenience.
  }
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** Numbers numerically, text naturally ("node2" before "node10"), numbers before text. */
export function naturalCompare(a: number | string, b: number | string): number {
  if (typeof a === "number" && typeof b === "number") return a - b;
  if (typeof a === "number") return -1;
  if (typeof b === "number") return 1;
  return collator.compare(a, b);
}

/**
 * Shrinks the widest columns first so the total fits `available`: every column
 * keeps its natural width up to a common cap, and none goes below its minimum.
 */
export function fitWidths(natural: { key: string; w: number }[], mins: Record<string, number>, available: number): Record<string, number> {
  // Rounding up keeps a column that fits from showing an ellipsis over a fraction of a pixel.
  const widthAt = (cap: number) =>
    Object.fromEntries(natural.map((c) => [c.key, c.w <= cap ? Math.ceil(c.w) : Math.max(Math.ceil(mins[c.key] ?? MIN_COL), Math.floor(cap))]));
  const sum = (o: Record<string, number>) => Object.values(o).reduce((a, b) => a + b, 0);
  let hi = Math.max(0, ...natural.map((c) => c.w));
  if (sum(widthAt(hi)) <= available) return widthAt(hi);
  // The highest common cap on the widest columns that still fits (or every column at its minimum).
  let lo = 0;
  for (let i = 0; i < 30; i++) {
    const mid = (lo + hi) / 2;
    if (sum(widthAt(mid)) <= available) lo = mid;
    else hi = mid;
  }
  return widthAt(lo);
}

/** What allocateColumns needs to know about a column. */
export interface AllocColumn {
  key: string;
  fit: "content" | "flex";
  priority: 1 | 2 | 3;
  /** A flex column's preferred width (its `width`); otherwise its widest cell. */
  width?: number;
  /** The narrowest a flex column may get (default 48, and never below its header). */
  minWidth?: number;
  /** A message column: holds its content (up to 48ch) until every lower-priority column has hidden. */
  message?: boolean;
  /** Has a short form (Column.narrow), measured as ColumnMeasure.narrow. */
  narrow?: boolean;
}

/** A column's measured needs in px, padding included. */
export interface ColumnMeasure {
  /** The widest cell. */
  content: number;
  /** The full header label, with its padding and sort icon. */
  header: number;
  /** The short header label, when the column has one. */
  short?: number;
  /** For a message column: 48 characters at the cell font, the most it holds on to. */
  messageCap?: number;
  /** The widest cell in its short form (Column.narrow). */
  narrow?: number;
}

export interface Allocation {
  widths: Record<string, number>;
  /** Hidden to make room, one at a time: the rightmost priority-3 column first, then priority 2. */
  hidden: string[];
  /** Drawn in their short form (Column.narrow). */
  narrow: string[];
  /** Showing their short header label. */
  short: string[];
  /** True only when the priority-1 columns can't fit even at their narrowest. */
  scroll: boolean;
  total: number;
}

/**
 * Lays out columns in `available` px (§D5). Content columns (ids, numbers, short states)
 * keep their widest cell and never shrink. Flex columns (commands, paths, images) shrink
 * first, the widest first, down to their header. Then headers switch to their short label;
 * then columns hide one at a time, the rightmost priority-3 column first, then the
 * rightmost priority-2 one, so the fewest go. Headers are never cut. A message column (§2.5)
 * holds its content up to 48 characters through all of that. With only priority 1 left,
 * columns with a short form (Column.narrow: "c1" for cell1) switch to it, then a message
 * gives way. Only when even that can't fit does the table scroll sideways. Leftover room
 * goes to the flex columns, so the total is exactly `available` and nothing scrolls.
 */
export function allocateColumns(columns: AllocColumn[], available: number, measure: (key: string) => ColumnMeasure): Allocation {
  const m = new Map(columns.map((c) => [c.key, measure(c.key)]));
  const avail = Math.floor(available);
  const head = (c: AllocColumn, short: boolean) => {
    const x = m.get(c.key)!;
    return Math.ceil(short && x.short != null ? Math.min(x.short, x.header) : x.header);
  };

  const plan = (cols: AllocColumn[], short: boolean, force = false, relax = false, narrow = false): Allocation | null => {
    const widths: Record<string, number> = {};
    const narrowKeys = narrow ? cols.filter((c) => c.narrow && m.get(c.key)!.narrow != null).map((c) => c.key) : [];
    const prefs: Record<string, number> = {};
    const floors: Record<string, number> = {};
    let fixed = 0;
    const flex: AllocColumn[] = [];
    for (const c of cols) {
      const x = m.get(c.key)!;
      const h = head(c, short);
      if (c.fit === "content") {
        widths[c.key] = Math.ceil(Math.max(narrowKeys.includes(c.key) ? x.narrow! : x.content, h));
        fixed += widths[c.key];
      } else {
        floors[c.key] = Math.ceil(Math.max(c.minWidth ?? MIN_COL, h));
        // A message keeps its sentence (up to 48ch) until the columns of lower priority are gone.
        if (c.message && !relax) floors[c.key] = Math.max(floors[c.key], Math.ceil(Math.min(x.content, x.messageCap ?? Infinity)));
        prefs[c.key] = Math.ceil(Math.max(c.width ?? x.content, floors[c.key]));
        flex.push(c);
      }
    }
    const sum = (o: Record<string, number>) => Object.values(o).reduce((a, b) => a + b, 0);
    const shortKeys = short ? cols.filter((c) => m.get(c.key)!.short != null).map((c) => c.key) : [];
    if (fixed + sum(floors) > avail) {
      if (!force) return null;
      for (const c of flex) widths[c.key] = floors[c.key];
      const total = fixed + sum(floors);
      return { widths, hidden: [], short: shortKeys, narrow: narrowKeys, scroll: total > avail, total };
    }
    const room = avail - fixed;
    if (sum(prefs) <= room) {
      for (const c of flex) widths[c.key] = prefs[c.key];
      // The rest goes to the flex columns (or, with none, to every column alike), so the table
      // spans the width exactly. A message column already has its whole sentence: it doesn't
      // grow into a gap, the other columns share the room instead.
      let extra = room - sum(prefs);
      const growing = flex.filter((c) => !c.message);
      const takers = growing.length ? growing : cols;
      const weight = takers.reduce((a, c) => a + (prefs[c.key] ?? widths[c.key]), 0) || 1;
      takers.forEach((c, i) => {
        const add = i === takers.length - 1 ? extra : Math.floor(((prefs[c.key] ?? widths[c.key]) / weight) * (room - sum(prefs)));
        widths[c.key] += add;
        extra -= add;
      });
    } else {
      // The widest flex columns give way first: a common cap, never below a column's floor.
      const at = (cap: number) => flex.reduce((a, c) => a + Math.max(floors[c.key], Math.min(prefs[c.key], Math.floor(cap))), 0);
      let lo = 0;
      let hi = Math.max(...flex.map((c) => prefs[c.key]));
      for (let i = 0; i < 32; i++) {
        const mid = (lo + hi) / 2;
        if (at(mid) <= room) lo = mid;
        else hi = mid;
      }
      for (const c of flex) widths[c.key] = Math.max(floors[c.key], Math.min(prefs[c.key], Math.floor(lo)));
      const left = room - flex.reduce((a, c) => a + widths[c.key], 0);
      const capped = flex.find((c) => widths[c.key] < prefs[c.key]);
      if (capped && left > 0) widths[capped.key] += left;
    }
    return { widths, hidden: [], short: shortKeys, narrow: narrowKeys, scroll: false, total: sum(widths) };
  };

  if (!(avail > 0) || !Number.isFinite(avail)) {
    return plan(columns, false, true) ?? { widths: {}, hidden: [], short: [], narrow: [], scroll: false, total: 0 };
  }
  let cols = columns;
  const hidden: string[] = [];
  // Lower-priority columns go one at a time (the rightmost priority 3 first, then priority 2),
  // so a narrow table loses as few as it must and a sentence keeps its room.
  const order = [0, ...[3, 2].flatMap((p) => columns.filter((c) => c.priority === p).map((c) => c.key).reverse())];
  for (const drop of order) {
    if (drop) {
      const gone = cols.filter((c) => c.key === drop);
      if (!gone.length) continue;
      hidden.push(...gone.map((c) => c.key));
      cols = cols.filter((c) => !gone.includes(c));
    }
    const fits = plan(cols, false) ?? plan(cols, true);
    if (fits) return { ...fits, hidden: [...hidden] };
  }
  // Only priority 1 is left and it still doesn't fit: short forms first, then a message column
  // gives way too.
  const narrowed = plan(cols, false, false, false, true) ?? plan(cols, true, false, false, true);
  if (narrowed) return { ...narrowed, hidden: [...hidden] };
  const relaxed = plan(cols, false, false, true, true) ?? plan(cols, true, false, true, true);
  if (relaxed) return { ...relaxed, hidden: [...hidden] };
  const last = plan(cols, true, true, true, true)!;
  return { ...last, hidden: [...hidden] };
}

/** Does a key press match a shortcut written the Mac way: "⌘⌫", "⇧F10", "Space", "⌘C"? */
export function matchShortcut(e: Pick<KeyboardEvent<HTMLElement>, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">, shortcut: string): boolean {
  let rest = shortcut.trim();
  const want = { meta: false, ctrl: false, alt: false, shift: false };
  for (;;) {
    const c = rest[0];
    if (c === "⌘") want.meta = true;
    else if (c === "⌃") want.ctrl = true;
    else if (c === "⌥") want.alt = true;
    else if (c === "⇧") want.shift = true;
    else break;
    rest = rest.slice(1);
  }
  const KEYS: Record<string, string> = { "⌫": "Backspace", "⌦": "Delete", "↩": "Enter", "⏎": "Enter", Space: " ", Esc: "Escape", "→": "ArrowRight", "←": "ArrowLeft", "↑": "ArrowUp", "↓": "ArrowDown" };
  const key = KEYS[rest] ?? rest;
  const sameKey = key.length === 1 ? e.key.toLowerCase() === key.toLowerCase() : e.key === key;
  // A printable key's shift state is part of the character, so it isn't compared for letters.
  const shiftOk = key.length === 1 && /[a-z]/i.test(key) ? true : e.shiftKey === want.shift;
  return sameKey && e.metaKey === want.meta && e.ctrlKey === want.ctrl && e.altKey === want.alt && shiftOk && (!want.shift || e.shiftKey);
}

type RowProps<T> = {
  row: T;
  rowKeyStr: string;
  id: string;
  cols: Column<T>[];
  /** Keys of the columns drawn in their short form. */
  narrowKeys: string;
  sel: boolean;
  active: boolean;
  /** Where the keyboard is: the active row, or the first one before any has been chosen. */
  cursor: boolean;
  cls?: string;
  checkbox: boolean;
  zebra: boolean;
  detail?: ReactNode;
};

function cellContent<T>(c: Column<T>, row: T): ReactNode {
  const v = c.render(row);
  const full = c.ellipsis === "middle" && typeof v === "string" ? <MiddleEllipsis text={v} /> : v;
  // Both forms are in the cell, so either can be measured; the column's class picks one.
  if (!c.narrow) return full;
  return (
    <>
      <span className="cell-wide">{full}</span>
      <span className="cell-narrow">{c.narrow(row)}</span>
    </>
  );
}

function RowInner<T>({ row, rowKeyStr, id, cols, narrowKeys, sel, active, cursor, cls, checkbox, zebra, detail }: RowProps<T>) {
  const narrowSet = narrowKeys ? narrowKeys.split("|") : [];
  const className = [sel && "sel", active && "active-row", cursor && "cursor-row", zebra && "zebra", cls, detail != null && "has-detail"].filter(Boolean).join(" ") || undefined;
  // A cell may take over the visible neighbours it names (absorb): they're skipped.
  const cells: { c: Column<T>; span: number }[] = [];
  for (let i = 0; i < cols.length; i++) {
    const c = cols[i];
    const take = c.absorb?.(row);
    let span = 1;
    if (take?.length) while (i + span < cols.length && take.includes(cols[i + span].key)) span++;
    cells.push({ c, span });
    i += span - 1;
  }
  const main = (
    <tr id={id} role="row" data-key={rowKeyStr} aria-selected={sel} className={className}>
      {checkbox && (
        <td role="gridcell" className="check-cell">
          <input type="checkbox" checked={sel} readOnly tabIndex={-1} aria-label="Select row" data-check />
        </td>
      )}
      {cells.map(({ c, span }) => (
        <td
          key={c.key}
          role="gridcell"
          colSpan={span > 1 ? span : undefined}
          className={[c.kind === "message" && "cell-message", c.narrow && (narrowSet.includes(c.key) ? "is-narrow" : "is-wide")].filter(Boolean).join(" ") || undefined}
          data-fit={span === 1 && fitOf(c) === "content" ? "content" : undefined}
          style={{ textAlign: span > 1 ? undefined : c.align }}
          title={c.title?.(row)}
        >
          {cellContent(c, row)}
        </td>
      ))}
    </tr>
  );
  if (detail == null) return main;
  return (
    <>
      {main}
      <tr role="row" aria-hidden data-key={rowKeyStr} className={`row-detail ${className ?? ""}`}>
        <td colSpan={cols.length + (checkbox ? 1 : 0)} className="row-detail-cell">{detail}</td>
      </tr>
    </>
  );
}

const Row = memo(RowInner) as typeof RowInner;

/**
 * A column's priority. A column keyed "node" is the row's identity in a table of every node's
 * rows (Containers, Workloads, Services, the Activity log): it never steps aside (C5), whatever
 * the window says; give it `narrow` ("c1") for the tightest tiles.
 */
const priorityOf = <T,>(c: Column<T>): 1 | 2 | 3 => (c.key === "node" ? 1 : (c.priority ?? 1));

/** A column's fit when it doesn't say: right-aligned and narrow columns hold their content. */
const fitOf = <T,>(c: Column<T>): "content" | "flex" =>
  c.fit ?? (c.kind === "message" ? "flex" : c.align === "right" || (typeof c.width === "number" && c.width <= FIT_KEEP) ? "content" : "flex");

/** What a header's Help tip adds to its width. */
const HELP_ICON_W = 20;

/** Header text width at the header's font, plus its padding and (when sortable) the sort icon. */
function headerWidth(label: string, font: string, sortable: boolean): number {
  return Math.ceil(measureText(label, font)) + 16 + (sortable ? 15 : 0) + 2;
}

let textCtx: CanvasRenderingContext2D | null | undefined;
function measureText(text: string, font: string): number {
  if (textCtx === undefined) {
    try {
      textCtx = document.createElement("canvas").getContext("2d");
    } catch {
      textCtx = null;
    }
  }
  if (!textCtx) return text.length * 7;
  textCtx.font = font;
  return textCtx.measureText(text).width;
}

/** Re-measures content at most this often while rows change (live data). */
const REMEASURE_MS = 2000;
const warnedNoActivate = new Set<string>();

/** Dense grid: click a header to sort, click a row to select, double-click or Enter to open. */
export function Table<T>(props: TableProps<T>) {
  const {
    columns, rows, rowKey, selected, onSelect, onActivate, empty = "Nothing to show.", initialSort = null,
    selection, onSelectionChange, selectionMode, checkboxes, onRowContextMenu, tableId, pinFirst, rowClass, rowDetail,
    footer, itemNoun = ["item", "items"], onKeyAction, ariaLabel, rowActions,
  } = props;
  const multi = selectionMode === "multi";
  const baseId = useId();
  const saved = useMemo(() => loadSaved(tableId), [tableId]);

  if (import.meta.env.DEV && (onRowContextMenu || rowActions) && !onActivate) {
    const id = tableId ?? ariaLabel ?? "table";
    if (!warnedNoActivate.has(id)) {
      warnedNoActivate.add(id);
      console.warn(`Table "${id}" has a context menu but no onActivate: Enter and double-click do nothing.`);
    }
  }

  // ---- columns: hidden by choice, then by the allocation; resized widths
  const [hidden, setHidden] = useState<Set<string>>(() => new Set(saved.hidden ?? []));
  const [widths, setWidths] = useState<Record<string, number>>(() => saved.widths ?? {});
  const [avail, setAvail] = useState(Number.POSITIVE_INFINITY);
  const wrapRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    // The scroll box, not the wrapper: its width drops when a vertical scroll bar appears.
    const measure = () => el.clientWidth && setAvail(el.clientWidth);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const isHideable = (c: Column<T>, i: number) => c.hideable ?? i > 0;
  const candidates = useMemo(() => columns.filter((c, i) => !(isHideable(c, i) && hidden.has(c.key))), [columns, hidden]);
  const candSig = candidates.map((c) => c.key).join("|");

  // ---- sort (controlled or internal; falls back when its column is gone)
  const colsRef = useRef(columns);
  colsRef.current = columns;
  const pinRef = useRef(pinFirst);
  pinRef.current = pinFirst;
  const colSig = columns.map((c) => (c.sort ? `${c.key}*` : c.key)).join("|");
  const sortable = (s: SortState | undefined): s is NonNullable<SortState> => !!s && columns.some((c) => c.key === s.key && c.sort);
  const [ownSort, setOwnSort] = useState<SortState>(() => (sortable(saved.sort) ? saved.sort : initialSort));
  const requested = props.sort !== undefined ? props.sort : ownSort;
  const sort: SortState = sortable(requested) ? requested : sortable(initialSort) ? initialSort : null;

  useEffect(() => {
    // The same Table reused for other columns (e.g. a view switch) drops a sort it can't apply.
    if (props.sort === undefined && ownSort && !sortable(ownSort)) setOwnSort(sortable(initialSort) ? initialSort : null);
  }, [colSig]);

  const setSort = (s: SortState) => {
    if (props.sort === undefined) setOwnSort(s);
    props.onSortChange?.(s);
  };

  const sorted = useMemo(() => {
    const by = sort ? colsRef.current.find((c) => c.key === sort.key)?.sort : undefined;
    const pin = pinRef.current;
    if (!by && !pin) return rows;
    const decorated = rows.map((r, i) => ({ r, i, v: by ? by(r) : 0, p: pin?.(r) ? 0 : 1 }));
    decorated.sort((a, b) => a.p - b.p || (by && sort ? naturalCompare(a.v, b.v) * sort.dir : 0) || a.i - b.i);
    return decorated.map((d) => d.r);
    // Sorting depends on the column keys, not on the identity of the columns array.
  }, [rows, sort?.key, sort?.dir, colSig, !!pinFirst]);

  const keys = useMemo(() => sorted.map((r) => rowKey(r)), [sorted, rowKey]);

  useEffect(() => {
    storeSaved(tableId, { sort: props.sort === undefined ? ownSort : undefined, widths, hidden: [...hidden] });
  }, [tableId, ownSort, widths, hidden, props.sort]);

  // ---- selection (controlled, legacy single, or internal)
  const [ownSel, setOwnSel] = useState<Set<string>>(() => new Set());
  const current: ReadonlySet<string> = useMemo(
    () => selection ?? (selected !== undefined ? new Set(selected ? [selected] : []) : ownSel),
    [selection, selected, ownSel],
  );
  const [active, setActive] = useState<string | null>(null);
  const anchor = useRef<string | null>(null);

  const commit = useCallback((next: Set<string>, primary: string | null) => {
    if (!selection && selected === undefined) setOwnSel(next);
    onSelectionChange?.(next, primary);
    if (onSelect && primary !== null && (!multi || next.has(primary))) onSelect(primary);
    setActive(primary);
  }, [selection, selected, onSelectionChange, onSelect, multi]);

  const range = (from: string | null, to: string): Set<string> => {
    const a = from ? keys.indexOf(from) : -1;
    const b = keys.indexOf(to);
    if (a < 0) return new Set([to]);
    const [lo, hi] = a < b ? [a, b] : [b, a];
    return new Set(keys.slice(lo, hi + 1));
  };

  const clickRow = (k: string, e: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }) => {
    if (multi && e.shiftKey) {
      const r = range(anchor.current, k);
      if (e.metaKey || e.ctrlKey) current.forEach((x) => r.add(x));
      commit(r, k);
      return;
    }
    if (multi && (e.metaKey || e.ctrlKey)) {
      const next = new Set(current);
      if (next.has(k)) next.delete(k);
      else next.add(k);
      anchor.current = k;
      commit(next, k);
      return;
    }
    anchor.current = k;
    commit(new Set([k]), k);
  };

  const activeKey = active && keys.includes(active) ? active : (keys.find((k) => current.has(k)) ?? null);
  const activeIndex = activeKey ? keys.indexOf(activeKey) : -1;
  const rowId = (i: number) => `${baseId}-r${i}`;

  // Keep the active row in view.
  useEffect(() => {
    if (activeIndex < 0) return;
    document.getElementById(rowId(activeIndex))?.scrollIntoView?.({ block: "nearest" });
  }, [activeIndex]);

  // ---- type-ahead on the first text column
  const typed = useRef({ text: "", at: 0 });
  const textOf = useMemo(() => {
    const col = columns.find((c) => c.sort && sorted.length > 0 && typeof c.sort(sorted[0]) === "string");
    return (r: T) => String(col?.sort?.(r) ?? rowKey(r));
  }, [colSig, sorted, rowKey]);

  const moveTo = (i: number, extend: boolean) => {
    if (keys.length === 0) return;
    const k = keys[Math.max(0, Math.min(keys.length - 1, i))];
    if (multi && extend) {
      if (!anchor.current) anchor.current = activeKey ?? k;
      commit(range(anchor.current, k), k);
    } else {
      anchor.current = k;
      commit(new Set([k]), k);
    }
  };

  // ---- context menus: right-click, ⇧F10 and the context-menu key all end up here
  const actionItems = (row: T): MenuItem[] =>
    (rowActions?.(row) ?? []).map((a) => ({ label: a.label, shortcut: a.shortcut, run: a.run, danger: a.danger, disabled: a.disabled }));
  const openMenuFor = (row: T, e: ReactMouseEvent | MouseEvent, sel: Set<string>) => {
    if (onRowContextMenu) onRowContextMenu(row, e as ReactMouseEvent, sel);
    else if (rowActions) showContextMenu(e, actionItems(row));
  };
  /** Opens the active row's menu from the keyboard, anchored to the row like a right-click on it. */
  const openActiveMenu = (): boolean => {
    if (!onRowContextMenu && !rowActions) return false;
    let i = activeIndex;
    if (i < 0 && keys.length) {
      i = 0;
      commit(new Set([keys[0]]), keys[0]);
    }
    if (i < 0) return false;
    const r = document.getElementById(rowId(i))?.getBoundingClientRect();
    const ev = new MouseEvent("contextmenu", {
      bubbles: true, cancelable: true, detail: -1,
      clientX: Math.round((r?.left ?? 0) + Math.min(24, (r?.width ?? 0) / 2)), clientY: Math.round((r?.bottom ?? 0) - 2),
    });
    const sel = current.has(keys[i]) ? new Set(current) : new Set([keys[i]]);
    openMenuFor(sorted[i], ev, sel);
    return true;
  };

  const NAV = new Set(["ArrowDown", "ArrowUp", "PageDown", "PageUp", "Home", "End"]);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if ((e.target as HTMLElement).closest("input, select, textarea, button:not([data-check])") && e.target !== e.currentTarget) return;
    if ((e.key === "F10" && e.shiftKey) || e.key === "ContextMenu") {
      if (openActiveMenu()) e.preventDefault();
      return;
    }
    if (rowActions && activeIndex >= 0 && !NAV.has(e.key)) {
      const hit = rowActions(sorted[activeIndex]).find((a) => a.shortcut && !a.disabled && matchShortcut(e, a.shortcut));
      if (hit) {
        e.preventDefault();
        void hit.run();
        return;
      }
    }
    const page = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? 240) / 22) - 1);
    const mod = e.metaKey || e.ctrlKey;
    const from = activeIndex < 0 ? -1 : activeIndex;
    switch (e.key) {
      case "ArrowDown": e.preventDefault(); moveTo(from + 1, e.shiftKey); return;
      case "ArrowUp": e.preventDefault(); moveTo(from < 0 ? 0 : from - 1, e.shiftKey); return;
      case "PageDown": e.preventDefault(); moveTo(from + page, e.shiftKey); return;
      case "PageUp": e.preventDefault(); moveTo(from - page, e.shiftKey); return;
      case "Home": e.preventDefault(); moveTo(0, e.shiftKey); return;
      case "End": e.preventDefault(); moveTo(keys.length - 1, e.shiftKey); return;
      case "Enter":
        if (activeKey && onActivate) {
          e.preventDefault();
          onActivate(sorted[activeIndex]);
        }
        return;
      case " ":
        if (!activeKey) return;
        e.preventDefault();
        if (multi) {
          const next = new Set(current);
          if (next.has(activeKey)) next.delete(activeKey);
          else next.add(activeKey);
          anchor.current = activeKey;
          commit(next, activeKey);
        } else commit(new Set([activeKey]), activeKey);
        return;
      case "Backspace":
      case "Delete":
        if (onKeyAction && current.size > 0 && (mod || e.key === "Delete")) {
          e.preventDefault();
          onKeyAction("delete", sorted.filter((r) => current.has(rowKey(r))));
        }
        return;
    }
    if (mod && e.key.toLowerCase() === "a" && multi) {
      e.preventDefault();
      commit(new Set(keys), activeKey ?? keys[0] ?? null);
      return;
    }
    if (e.key.length === 1 && !mod && !e.altKey && e.key !== " ") {
      const now = Date.now();
      typed.current = { text: (now - typed.current.at < TYPEAHEAD_MS ? typed.current.text : "") + e.key.toLowerCase(), at: now };
      const hit = sorted.findIndex((r) => textOf(r).toLowerCase().startsWith(typed.current.text));
      if (hit >= 0) moveTo(hit, false);
    }
  };

  // ---- pointer on rows (delegated, so row components stay memoized)
  const keyFromEvent = (e: { target: EventTarget }) => (e.target as HTMLElement).closest<HTMLElement>("tr[data-key]")?.dataset.key ?? null;
  const onMouseDown = (e: ReactMouseEvent) => {
    if (e.button !== 0) return;
    const k = keyFromEvent(e);
    if (k === null) return;
    if (e.shiftKey) e.preventDefault(); // no text selection on range clicks
    if ((e.target as HTMLElement).closest("[data-check]")) {
      const next = new Set(current);
      if (next.has(k)) next.delete(k);
      else next.add(k);
      anchor.current = k;
      commit(next, k);
      return;
    }
    clickRow(k, e);
  };
  const onDoubleClick = (e: ReactMouseEvent) => {
    const k = keyFromEvent(e);
    if (k === null || !onActivate) return;
    if ((e.target as HTMLElement).closest("button, a, input, select, textarea")) return;
    const r = sorted[keys.indexOf(k)];
    if (r !== undefined) onActivate(r);
  };
  const onContextMenu = (e: ReactMouseEvent) => {
    if (!onRowContextMenu && !rowActions) return;
    const k = keyFromEvent(e);
    if (k === null) {
      // ⇧F10 sent from outside (keymap.ts dispatches on the focused grid): the active row's menu.
      if (e.target === scrollRef.current && openActiveMenu()) e.preventDefault();
      return;
    }
    e.preventDefault();
    let sel = new Set(current);
    if (!sel.has(k)) {
      sel = new Set([k]);
      anchor.current = k;
      commit(sel, k);
    }
    openMenuFor(sorted[keys.indexOf(k)], e, sel);
  };
  const onMouseOver = (e: ReactMouseEvent) => {
    // A clipped cell shows its full text on hover.
    const td = (e.target as HTMLElement).closest("td");
    if (td && !td.title && td.scrollWidth > td.clientWidth + 1) td.title = td.textContent ?? "";
  };

  // ---- column resizing
  const tableRef = useRef<HTMLTableElement>(null);
  const [resizing, setResizing] = useState<string | null>(null);
  const startResize = (key: string, min: number) => (e: ReactPointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    // Freeze every visible column at its current width, then follow the pointer.
    const ths = [...(tableRef.current?.querySelectorAll<HTMLTableCellElement>("thead th[data-col]") ?? [])];
    const base: Record<string, number> = { ...effective };
    for (const th of ths) if (base[th.dataset.col!] === undefined) base[th.dataset.col!] = th.getBoundingClientRect().width;
    const start = base[key] ?? 100;
    const x0 = e.clientX;
    setResizing(key);
    const onMove = (ev: PointerEvent) => setWidths({ ...base, [key]: Math.max(min, Math.round(start + ev.clientX - x0)) });
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      document.body.classList.remove("dragging");
      setResizing(null);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    document.body.classList.add("dragging");
  };

  // ---- allocation: measure what each column needs, then lay them out in the width there is
  const userSized = Object.keys(widths).length > 0;
  const measured = useRef<Record<string, ColumnMeasure>>({});
  const lastMeasure = useRef({ sig: "", at: 0 });
  const [alloc, setAlloc] = useState<Allocation | null>(null);
  const showCheck = !!checkboxes && multi;
  useLayoutEffect(() => {
    const table = tableRef.current;
    const scroll = scrollRef.current;
    if (!table || !scroll || userSized) return;
    const th0 = table.querySelector<HTMLElement>("thead th[data-col]");
    const cs = th0 ? getComputedStyle(th0) : null;
    const font = cs ? `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}` : "600 11px sans-serif";
    const td0 = table.querySelector<HTMLElement>("tbody td");
    const tcs = td0 ? getComputedStyle(td0) : null;
    const cellFont = tcs ? `${tcs.fontWeight} ${tcs.fontSize} ${tcs.fontFamily}` : cs ? `400 ${cs.fontSize} ${cs.fontFamily}` : "400 12px sans-serif";
    const sig = `${candSig}|${font}|${Math.floor(avail)}|${rows.length}|${hidden.size}`;
    const now = Date.now();
    // A content column (ids, numbers, "5 W · 45 °C") is never cut: a value that outgrew its
    // column is measured again at once, not up to 2 s later (C5).
    const outgrown = () => {
      const cells = table.querySelectorAll<HTMLElement>("tbody td[data-fit='content']");
      for (let i = 0; i < cells.length && i < 160; i++) if (cells[i].scrollWidth > cells[i].clientWidth + 1) return true;
      return false;
    };
    if (sig === lastMeasure.current.sig && now - lastMeasure.current.at < REMEASURE_MS && alloc && !outgrown()) return;
    // Live numbers change width from second to second ("9.5 kB/s" → "96.9 kB/s"): a column keeps
    // the widest it has needed until the columns or the font change, so a value never gets cut.
    const base = `${candSig}|${font}`;
    const keepWidest = lastMeasure.current.sig.startsWith(`${base}|`) || lastMeasure.current.sig === "";
    lastMeasure.current = { sig: `${base}|${Math.floor(avail)}|${rows.length}|${hidden.size}`, at: now };
    // Content widths: one pass at max-content with the header labels out of the way, in layout
    // px (a window scaling in from its open animation would otherwise read a few px short).
    const scale = table.offsetWidth ? table.getBoundingClientRect().width / table.offsetWidth : 1;
    // Measuring hides the second lines and the header labels, so the table is shorter for a
    // moment: hold its height, and put back where every scroll box around it was, or a window
    // scrolled to its end (the Dashboard's bottom charts) is pulled up each time it measures.
    const held = table.style.minHeight;
    table.style.minHeight = `${table.offsetHeight}px`;
    const scrolled: [HTMLElement, number][] = [];
    for (let el = table.parentElement; el; el = el.parentElement) if (el.scrollTop > 0) scrolled.push([el, el.scrollTop]);
    table.classList.add("measuring");
    for (const th of table.querySelectorAll<HTMLElement>("thead th[data-col]")) {
      const key = th.dataset.col!;
      const c = candidates.find((x) => x.key === key);
      const content = Math.ceil(th.getBoundingClientRect().width / (scale || 1)) + 1;
      const before = keepWidest ? (measured.current[key]?.content ?? 0) : 0;
      if (c) measured.current[key] = { ...measured.current[key], content: Math.max(content, before) };
    }
    // Columns with a short form are measured again with it showing.
    if (candidates.some((c) => c.narrow)) {
      table.classList.add("measuring-narrow");
      for (const th of table.querySelectorAll<HTMLElement>("thead th[data-col]")) {
        const key = th.dataset.col!;
        if (!candidates.find((x) => x.key === key)?.narrow) continue;
        const w = Math.ceil(th.getBoundingClientRect().width / (scale || 1)) + 1;
        measured.current[key] = { ...measured.current[key], narrow: Math.max(w, keepWidest ? (measured.current[key]?.narrow ?? 0) : 0) };
      }
      table.classList.remove("measuring-narrow");
    }
    table.classList.remove("measuring");
    table.style.minHeight = held;
    for (const [el, top] of scrolled) if (el.scrollTop !== top) el.scrollTop = top;
    const measure = (key: string): ColumnMeasure => {
      const c = candidates.find((x) => x.key === key)!;
      const tip = c.help ? HELP_ICON_W : 0;
      const header = headerWidth(c.label, font, !!c.sort) + tip;
      const short = c.short ? headerWidth(c.short, font, !!c.sort) + tip : undefined;
      // A column hidden before it was ever measured is guessed from its header.
      const content = measured.current[key]?.content ?? header + 40;
      return {
        content, header, short, messageCap: c.kind === "message" ? Math.ceil(measureText("0".repeat(48), cellFont)) + 17 : undefined,
        narrow: c.narrow ? (measured.current[key]?.narrow ?? content) : undefined,
      };
    };
    const room = (Number.isFinite(avail) ? avail : scroll.clientWidth) - (showCheck ? 28 : 0);
    const next = allocateColumns(
      candidates.map((c) => ({ key: c.key, fit: fitOf(c), priority: priorityOf(c), width: typeof c.width === "number" ? c.width : undefined, minWidth: c.minWidth, message: c.kind === "message", narrow: !!c.narrow })),
      room,
      measure,
    );
    setAlloc((prev) => (prev && JSON.stringify(prev) === JSON.stringify(next) ? prev : next));
  });

  const autoHidden = new Set(userSized ? [] : (alloc?.hidden ?? []));
  const visibleCols = candidates.filter((c) => !autoHidden.has(c.key));
  const shortHeads = new Set(userSized ? [] : (alloc?.short ?? []));
  const narrowKeys = userSized ? "" : (alloc?.narrow ?? []).join("|");
  const effective: Record<string, number> = userSized ? widths : (alloc?.widths ?? {});
  const fixed = userSized || !!alloc;
  const lastVisible = visibleCols[visibleCols.length - 1]?.key;
  const tableWidth = !userSized && alloc ? alloc.total + (showCheck ? 28 : 0) : undefined;

  // Buttons inside cells stay out of the Tab order: the grid is one tab stop, rows move with arrows.
  useLayoutEffect(() => {
    tableRef.current?.querySelectorAll<HTMLElement>("tbody button, tbody a[href], tbody [tabindex]:not([tabindex='-1'])").forEach((el) => {
      if (el.tabIndex !== -1) el.tabIndex = -1;
    });
  });

  // ---- header menu: show/hide columns
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menu) return;
    const close = (e: Event) => {
      if (e instanceof globalThis.KeyboardEvent && e.key !== "Escape") return;
      if (e.type === "pointerdown" && menuRef.current?.contains(e.target as Node)) return;
      setMenu(null);
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("keydown", close, true);
    window.addEventListener("blur", close);
    menuRef.current?.querySelector<HTMLElement>("input:not(:disabled), button")?.focus();
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("keydown", close, true);
      window.removeEventListener("blur", close);
    };
  }, [menu]);

  const toggleCol = (key: string) =>
    setHidden((h) => {
      const next = new Set(h);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  // ---- render
  const allChecked = keys.length > 0 && keys.every((k) => current.has(k));
  const someChecked = !allChecked && keys.some((k) => current.has(k));
  const selCount = keys.filter((k) => current.has(k)).length;

  return (
    <div className="table-wrap" ref={wrapRef}>
      <div
        ref={scrollRef}
        className={`table-scroll${userSized || alloc?.scroll ? " scroll-x" : ""}`}
        tabIndex={0}
        role="grid"
        aria-label={ariaLabel}
        aria-multiselectable={multi || undefined}
        aria-rowcount={sorted.length + 1}
        aria-activedescendant={activeIndex >= 0 ? rowId(activeIndex) : undefined}
        onKeyDown={onKeyDown}
        onContextMenu={onContextMenu}
      >
        <table className={`grid rows-classed${fixed ? " fixed" : ""}`} ref={tableRef} role="presentation" style={tableWidth !== undefined ? { width: tableWidth } : undefined}>
          {fixed && (
            <colgroup>
              {showCheck && <col style={{ width: 28 }} />}
              {visibleCols.map((c) => <col key={c.key} style={{ width: userSized && c.key === lastVisible ? undefined : effective[c.key] }} />)}
            </colgroup>
          )}
          <thead role="rowgroup">
            <tr
              role="row"
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setMenu({ x: e.clientX, y: e.clientY });
              }}
            >
              {showCheck && (
                <th role="columnheader" className="check-cell">
                  <input
                    type="checkbox"
                    aria-label={allChecked ? "Deselect all" : "Select all"}
                    checked={allChecked}
                    ref={(el) => {
                      if (el) el.indeterminate = someChecked;
                    }}
                    onChange={() => commit(allChecked ? new Set() : new Set(keys), activeKey)}
                  />
                </th>
              )}
              {visibleCols.map((c) => {
                const on = sort?.key === c.key;
                const shown = shortHeads.has(c.key) && c.short ? c.short : c.label;
                const label = <span className="th-text">{shown}</span>;
                return (
                  <th
                    key={c.key}
                    role="columnheader"
                    data-col={c.key}
                    className={[c.align ? `align-${c.align}` : "", c.help ? "has-help" : ""].filter(Boolean).join(" ") || undefined}
                    style={{ width: fixed ? undefined : c.width, minWidth: c.minWidth }}
                    aria-sort={on ? (sort!.dir === 1 ? "ascending" : "descending") : c.sort ? "none" : undefined}
                    aria-label={shown !== c.label ? c.label : undefined}
                    title={shown !== c.label ? c.label : undefined}
                  >
                    {c.sort ? (
                      <button
                        type="button"
                        className="th-sort"
                        tabIndex={-1}
                        title={`Sort by ${c.label}`}
                        onClick={() => setSort(on ? { key: c.key, dir: sort!.dir === 1 ? -1 : 1 } : { key: c.key, dir: 1 })}
                      >
                        {c.align === "right" && <Icon icon={on && sort!.dir === -1 ? ChevronDown : ChevronUp} size={12} className={`sort-icon${on ? "" : " idle"}`} />}
                        {label}
                        {c.align !== "right" && <Icon icon={on && sort!.dir === -1 ? ChevronDown : ChevronUp} size={12} className={`sort-icon${on ? "" : " idle"}`} />}
                      </button>
                    ) : (
                      <span className="th-label">{label}</span>
                    )}
                    {c.help && <HelpTip topic={c.help} />}
                    <span
                      className={`col-resize${resizing === c.key ? " on" : ""}`}
                      role="separator"
                      aria-orientation="vertical"
                      aria-label={`Resize ${c.label}`}
                      onPointerDown={startResize(c.key, Math.max(c.minWidth ?? MIN_COL, MIN_COL))}
                      onDoubleClick={() => setWidths({})}
                    />
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody role="rowgroup" onMouseDown={onMouseDown} onDoubleClick={onDoubleClick} onMouseOver={onMouseOver}>
            {sorted.map((r, i) => {
              const k = keys[i];
              return (
                <Row
                  key={k}
                  row={r}
                  rowKeyStr={k}
                  id={rowId(i)}
                  cols={visibleCols}
                  narrowKeys={narrowKeys}
                  sel={current.has(k)}
                  active={i === activeIndex}
                  cursor={i === (activeIndex >= 0 ? activeIndex : 0)}
                  cls={rowClass?.(r)}
                  checkbox={showCheck}
                  zebra={i % 2 === 1}
                  detail={rowDetail?.(r) ?? undefined}
                />
              );
            })}
            {sorted.length === 0 && (
              <tr role="row">
                <td role="gridcell" colSpan={visibleCols.length + (showCheck ? 1 : 0)} className="empty">{empty}</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {footer !== undefined && footer !== false && footer !== null && (
        <div className="table-footer" role="status">
          <span>
            {sorted.length} {sorted.length === 1 ? itemNoun[0] : itemNoun[1]}
            {selCount > 0 && ` · ${selCount} selected`}
          </span>
          {footer !== true && <span className="table-footer-slot">{footer}</span>}
        </div>
      )}
      {menu && (
        <div ref={menuRef} className="col-menu" role="menu" aria-label="Columns" style={{ left: menu.x, top: menu.y }}>
          <div className="col-menu-title">Columns</div>
          {columns.map((c, i) => {
            const can = isHideable(c, i);
            const byWidth = autoHidden.has(c.key);
            return (
              <label key={c.key} role="menuitemcheckbox" aria-checked={!hidden.has(c.key) || !can} title={byWidth ? "Hidden to make room; widen the window to show it" : undefined}>
                <input type="checkbox" checked={!can || !hidden.has(c.key)} disabled={!can} onChange={() => toggleCol(c.key)} />
                {c.label}
                {byWidth && <span className="muted">(no room)</span>}
              </label>
            );
          })}
          <hr />
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setHidden(new Set());
              setWidths({});
              setMenu(null);
            }}
          >
            Reset columns
          </button>
        </div>
      )}
    </div>
  );
}
