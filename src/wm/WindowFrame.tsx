import { useEffect, useLayoutEffect, useRef, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { Copy, Minus, Square, X, type LucideIcon } from "lucide-react";
import { InfoTip } from "../components/ui";
import { showContextMenu } from "../lib/contextMenu";
import { dialogOpen } from "../lib/session";
import { clampRect, overviewPlacement, snapZoneAt, TITLE_H, zoneRect, type Rect, type Size, type SnapZone } from "./geometry";
import { raiseSeq, setSnapPreview, useControlsStyle, useWMActions, type WinState } from "./WindowManager";
import "./wm.css";

type Edge = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";
const EDGES: Edge[] = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];
const DRAG_SLOP = 3;

export function WindowFrame({ win, active, appFocused, title, icon: Icon, help, bounds, overviewCell, picked, onPick, children }: {
  win: WinState;
  active: boolean;
  appFocused: boolean;
  title: string;
  icon?: LucideIcon;
  /** What the title bar's info button explains: a topic from docs/help. */
  help?: string;
  bounds: Size;
  /** In the overview: the grid cell this window is scaled into. */
  overviewCell?: Rect | null;
  picked?: boolean;
  onPick?: () => void;
  children: ReactNode;
}) {
  const wm = useWMActions();
  const controls = useControlsStyle();
  const frameRef = useRef<HTMLElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  /** The element that last had focus in this window; it gets it back when the window does. */
  const lastFocus = useRef<HTMLElement | null>(null);
  /** While a drag or resize runs, the frame's geometry lives in its style, not in state. */
  const dragging = useRef(false);

  // Becoming the active window puts keyboard focus inside it: where it was last,
  // else [data-autofocus], else the frame itself (Tab then reaches the first control).
  // Never an arbitrary toolbar button: a ring on "Files" when a node opens reads as a selection.
  useEffect(() => {
    if (!active || win.minimized) return;
    // Another window opened or focused before this frame runs (a ⌘K command on a busy main
    // thread): it's that window's turn, and grabbing focus here would raise this one over it.
    const seq = raiseSeq();
    const raf = window.requestAnimationFrame(() => {
      const frame = frameRef.current;
      if (!frame || dialogOpen() || raiseSeq() !== seq) return;
      const focused = document.activeElement;
      if (focused && frame.contains(focused)) return;
      // Leave focus alone in overlays (palette, menus) that are still closing.
      if (focused?.closest("[data-shell-overlay]")) return;
      const body = bodyRef.current;
      const remembered = lastFocus.current?.isConnected && frame.contains(lastFocus.current) ? lastFocus.current : null;
      const target = remembered
        ?? body?.querySelector<HTMLElement>("[data-autofocus]")
        ?? frame;
      target.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(raf);
  }, [active, win.minimized]);

  const rendered = (): Rect => (win.maximized ? { x: 0, y: 0, w: bounds.w, h: bounds.h } : { x: win.x, y: win.y, w: win.w, h: win.h });

  const deskRect = () => wm.desktopRef.current?.getBoundingClientRect() ?? new DOMRect(0, 0, bounds.w, bounds.h);

  /** Move by the title bar: clamped so it can't be lost, with snap zones at the edges (⌥ turns them off). */
  const startMove = (e: ReactPointerEvent<HTMLElement>) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button, input, select, a")) return;
    const el = frameRef.current;
    if (!el) return;
    e.preventDefault();
    wm.focus(win.id);
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const sx = e.clientX;
    const sy = e.clientY;
    const preDrag = win.restore ?? rendered();
    let base = rendered();
    let cur = base;
    let moved = false;
    let zone: SnapZone | null = null;
    const desk = () => wm.getBounds();

    const onMove = (ev: PointerEvent) => {
      const dx = ev.clientX - sx;
      const dy = ev.clientY - sy;
      if (!moved) {
        if (Math.abs(dx) + Math.abs(dy) < DRAG_SLOP) return;
        moved = true;
        dragging.current = true;
        document.body.classList.add("dragging");
        // A maximized or snapped window drops back to its own size under the pointer.
        if ((win.maximized || win.snapped) && win.restore) {
          const ratio = (sx - deskRect().left - base.x) / Math.max(1, base.w);
          const r = win.restore;
          base = { x: Math.round(sx - deskRect().left - ratio * r.w), y: base.y, w: r.w, h: r.h };
          Object.assign(el.style, { left: `${base.x}px`, top: `${base.y}px`, width: `${base.w}px`, height: `${base.h}px` });
        }
      }
      cur = clampRect({ ...base, x: base.x + dx, y: base.y + dy }, desk());
      el.style.transform = `translate(${cur.x - base.x}px, ${cur.y - base.y}px)`;
      const d = deskRect();
      zone = ev.altKey ? null : snapZoneAt(ev.clientX - d.left, ev.clientY - d.top, desk());
      setSnapPreview(zone ? zoneRect(zone, desk()) : null);
    };

    const finish = (commit: boolean) => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onCancel);
      document.body.classList.remove("dragging");
      setSnapPreview(null);
      dragging.current = false;
      if (!moved) return;
      // Put the frame where the pointer left it, so a snap animates from there.
      el.style.transform = "";
      if (!commit) {
        const r = rendered();
        Object.assign(el.style, { left: `${r.x}px`, top: `${r.y}px`, width: `${r.w}px`, height: `${r.h}px` });
        return;
      }
      Object.assign(el.style, { left: `${cur.x}px`, top: `${cur.y}px` });
      if (zone) {
        void el.offsetWidth;
        wm.snap(win.id, zone, preDrag);
      } else {
        wm.move(win.id, { x: cur.x, y: cur.y, w: cur.w, h: cur.h });
      }
    };
    const onUp = () => finish(true);
    const onCancel = () => finish(false);
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onCancel);
  };

  /** Resize from any of the eight edges and corners, down to the kind's minimum size. */
  const startResize = (edge: Edge) => (e: ReactPointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    const el = frameRef.current;
    if (!el) return;
    e.preventDefault();
    e.stopPropagation();
    wm.focus(win.id);
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const sx = e.clientX;
    const sy = e.clientY;
    const start = rendered();
    const [minW, minH] = wm.minSizeFor(win.kind);
    let cur = start;
    dragging.current = true;
    document.body.classList.add("dragging", `resizing-${edge}`);

    const onMove = (ev: PointerEvent) => {
      const b = wm.getBounds();
      const dx = ev.clientX - sx;
      const dy = ev.clientY - sy;
      let { x, y, w, h } = start;
      if (edge.includes("e")) w = Math.min(Math.max(minW, start.w + dx), Math.max(minW, b.w - start.x));
      if (edge.includes("s")) h = Math.min(Math.max(minH, start.h + dy), Math.max(minH, b.h - start.y));
      if (edge.includes("w")) {
        w = Math.max(minW, start.w - dx);
        x = start.x + start.w - w;
        if (x < 0 && start.x >= 0) {
          w += x;
          x = 0;
        }
      }
      if (edge.includes("n")) {
        h = Math.max(minH, start.h - dy);
        y = start.y + start.h - h;
        if (y < 0) {
          h = Math.max(minH, h + y);
          y = 0;
        }
      }
      cur = { x, y, w, h };
      Object.assign(el.style, { left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${h}px` });
    };
    const finish = (commit: boolean) => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onCancel);
      document.body.classList.remove("dragging", `resizing-${edge}`);
      dragging.current = false;
      if (commit) wm.move(win.id, cur);
      else Object.assign(el.style, { left: `${start.x}px`, top: `${start.y}px`, width: `${start.w}px`, height: `${start.h}px` });
    };
    const onUp = () => finish(true);
    const onCancel = () => finish(false);
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onCancel);
  };

  const titleMenu = (e: ReactMouseEvent) => {
    const many = wm.getWindows().length > 1;
    showContextMenu(e, [
      { label: "Minimize", shortcut: "⌘M", run: () => wm.minimize(win.id) },
      { label: win.maximized ? "Restore" : "Maximize", shortcut: win.maximized ? "⌃⌥↓" : "⌃⌥↑", run: () => wm.toggleMaximize(win.id) },
      { separator: true },
      { label: "Snap left", shortcut: "⌃⌥←", run: () => wm.snap(win.id, "left") },
      { label: "Snap right", shortcut: "⌃⌥→", run: () => wm.snap(win.id, "right") },
      ...(wm.popOut ? [{ separator: true } as const, { label: "Open in separate window", shortcut: "⇧⌘O", run: () => void wm.popOut?.(win.id) }] : []),
      { separator: true },
      { label: "Close others", disabled: many ? false : "No other windows are open", run: () => void wm.requestCloseOthers(win.id) },
      { label: "Close", shortcut: "⌘W", run: () => void wm.requestClose(win.id) },
    ]);
  };

  const r = rendered();
  // In the overview the window keeps its geometry and is scaled into its cell.
  let transform = "";
  if (overviewCell) {
    const p = overviewPlacement(overviewCell, r);
    transform = `translate(${p.dx}px, ${p.dy}px) scale(${p.scale.toFixed(4)})`;
  }
  // Drags write the frame's style directly; make sure it matches state again after every render.
  useLayoutEffect(() => {
    const el = frameRef.current;
    if (!el || dragging.current) return;
    Object.assign(el.style, { left: `${r.x}px`, top: `${r.y}px`, width: `${r.w}px`, height: `${r.h}px`, transform });
  });
  const style = {
    left: r.x, top: r.y, width: r.w, height: r.h, zIndex: win.z,
    ...(win.minimized && !overviewCell ? { display: "none" } : {}),
    ...(overviewCell ? { transformOrigin: "0 0" } : {}),
  };
  const looksActive = active && appFocused;
  const zoomLabel = win.maximized ? "Restore" : "Zoom";
  const buttons = controls === "mac" ? (
    <div className="win-controls mac" onPointerDown={(e) => e.stopPropagation()}>
      <button className="wc wc-close" aria-label={`Close ${title}`} title="Close" onClick={() => void wm.requestClose(win.id)}>
        <X size={8} strokeWidth={3} aria-hidden />
      </button>
      <button className="wc wc-min" aria-label={`Minimize ${title}`} title="Minimize" onClick={() => wm.minimize(win.id)}>
        <Minus size={8} strokeWidth={3} aria-hidden />
      </button>
      <button className="wc wc-zoom" aria-label={`${zoomLabel} ${title}`} title={zoomLabel} onClick={() => wm.toggleMaximize(win.id)}>
        {win.maximized ? <Copy size={7} strokeWidth={3} aria-hidden /> : <Square size={6} strokeWidth={3.5} aria-hidden />}
      </button>
    </div>
  ) : (
    <div className="win-controls winbox" onPointerDown={(e) => e.stopPropagation()}>
      <button aria-label={`Minimize ${title}`} title="Minimize" onClick={() => wm.minimize(win.id)}><Minus size={13} aria-hidden /></button>
      <button aria-label={`${win.maximized ? "Restore" : "Maximize"} ${title}`} title={win.maximized ? "Restore" : "Maximize"} onClick={() => wm.toggleMaximize(win.id)}>
        {win.maximized ? <Copy size={11} aria-hidden /> : <Square size={10} aria-hidden />}
      </button>
      <button className="close" aria-label={`Close ${title}`} title="Close" onClick={() => void wm.requestClose(win.id)}><X size={14} aria-hidden /></button>
    </div>
  );

  return (
    <section
      ref={frameRef}
      className={`win${looksActive ? " active" : ""}${active ? " front" : ""}${win.maximized ? " max" : ""}${win.snapped ? " snapped" : ""}${picked ? " picked" : ""} controls-${controls}`}
      data-win-id={win.id}
      data-kind={win.kind}
      style={style}
      tabIndex={-1}
      aria-label={title}
      onPointerDownCapture={(e) => {
        if (overviewCell) {
          e.preventDefault();
          e.stopPropagation();
          onPick?.();
          return;
        }
        wm.focus(win.id);
      }}
      onFocusCapture={(e) => {
        if (e.target !== frameRef.current) lastFocus.current = e.target as HTMLElement;
        wm.focus(win.id);
      }}
    >
      <header
        className="win-title"
        style={{ height: TITLE_H }}
        onPointerDown={startMove}
        onDoubleClick={(e) => !(e.target as HTMLElement).closest("button") && wm.toggleMaximize(win.id)}
        onContextMenu={titleMenu}
      >
        {controls === "mac" && buttons}
        <span className="win-title-label">
          {Icon && <Icon className="win-title-icon" size={13} aria-hidden />}
          <span className="win-title-text">{title}</span>
        </span>
        {help && <InfoTip topic={help} />}
        {controls === "winbox" && buttons}
      </header>
      <div className="win-body" ref={bodyRef}>{children}</div>
      {!win.maximized && EDGES.map((edge) => (
        <div key={edge} className={`rz rz-${edge}`} onPointerDown={startResize(edge)} aria-hidden />
      ))}
    </section>
  );
}
