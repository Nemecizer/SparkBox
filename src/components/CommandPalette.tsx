import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { ChevronRight, CircleHelp, Lock, Search } from "lucide-react";
import { ensureHelpCommands, HELP_GROUP, searchHelp } from "../help/nav";
import { LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, useAdminState } from "../lib/admin";
import { announce } from "../lib/announce";
import {
  adminUnlocked, rankCommands, recentCommandIds, runResolved, unavailableReason, useCommandList, type Command, type Ranked,
} from "../lib/commands";
import { closePanel, usePanel } from "../lib/session";
import { useClusterSelector } from "../lib/store";
import { registerMaintenanceCommands } from "../lib/maintenance";
import { raiseSeq } from "../wm/WindowManager";
import "./shell.css";

/** Order of sections when the palette opens empty. */
const GROUP_ORDER = ["Recent", "Open windows", "Windows", "Nodes", "Containers", "Services", "Models", "Switches", "Window", "Cluster", "Settings", "Appearance", "Demo", "App", "Coming soon", HELP_GROUP];
const RECENT_SHOWN = 5;

interface Section {
  name: string;
  rows: Ranked[];
}

/** Builds the sections: Recent first (empty query), then groups in a fixed order or by best match. */
export function paletteSections(cmds: Command[], query: string, recent: string[]): Section[] {
  const ranked = rankCommands(cmds, query, recent);
  const sections = new Map<string, Ranked[]>();
  const add = (name: string, r: Ranked) => {
    let rows = sections.get(name);
    if (!rows) sections.set(name, (rows = []));
    rows.push(r);
  };
  if (!query.trim()) {
    const recentRows = recent.map((id) => ranked.find((r) => r.cmd.id === id)).filter((r): r is Ranked => !!r).slice(0, RECENT_SHOWN);
    for (const r of recentRows) add("Recent", r);
    const shown = new Set(recentRows.map((r) => r.cmd.id));
    // Every Help article is here, but only for a search: an empty palette would be all Help.
    for (const r of ranked) if (!shown.has(r.cmd.id) && r.cmd.group !== HELP_GROUP) add(r.cmd.group, r);
    const rank = (g: string) => (GROUP_ORDER.indexOf(g) + 1 || GROUP_ORDER.length + 1);
    return [...sections.entries()].sort((a, b) => rank(a[0]) - rank(b[0])).map(([name, rows]) => ({ name, rows }));
  }
  for (const r of ranked) add(r.cmd.group, r);
  // Sections follow their best row, so the top hit is always first.
  return [...sections.entries()].map(([name, rows]) => ({ name, rows }));
}

/** A whole question, not a name to find. */
const QUESTION = /^(?:what|what's|whats|why|how|where|when|which|who|can|could|does|do|is|are|should)\s+\S/i;

/** The row for Help's own search, which reads the articles' text too: last, or first for a question. */
export function withHelpSearch(found: Section[], q: string): Section[] {
  const row: Ranked = { cmd: { id: "help:search", title: `Search Help for “${q}”`, group: HELP_GROUP, icon: CircleHelp, run: () => searchHelp(q) }, score: 0, hits: [] };
  const section = { name: "Search Help", rows: [row] };
  return QUESTION.test(q) ? [section, ...found] : [...found, section];
}

/** What the palette offers: with Live data, no Demo scenario commands (they play only with Demo data). */
export function paletteCommands(cmds: Command[], live: boolean): Command[] {
  return live ? cmds.filter((c) => !c.id.startsWith("demo:")) : cmds;
}

/** Why a row can't run now: its own reason, or look-only for one that needs Admin (nothing promises an unlock). */
export function rowReason(c: Command, lookOnly: boolean): string | null {
  return unavailableReason(c) ?? (lookOnly && c.needsAdmin ? LOOK_ONLY_TIP : null);
}

export function CommandPalette() {
  const panel = usePanel();
  if (panel !== "palette") return null;
  return createPortal(<Palette />, document.body);
}

function Palette() {
  const live = useClusterSelector((s) => s?.demo === false);
  // Maintenance mode per node, while the palette is open (P1.16).
  useEffect(() => registerMaintenanceCommands(), []);
  useEffect(() => ensureHelpCommands(), []);
  const all = useCommandList();
  const cmds = useMemo(() => paletteCommands(all, live), [all, live]);
  const lookOnly = !!useAdminState().lookOnly;
  const [query, setQuery] = useState("");
  const [hi, setHi] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const opener = useRef<Element | null>(document.activeElement);
  /** Set once a command runs: focus then goes where the command puts it, not back to the opener. */
  const ran = useRef(false);
  const recent = useMemo(() => recentCommandIds(), []);

  const sections = useMemo(() => {
    const found = paletteSections(cmds, query, recent);
    const q = query.trim();
    return q ? withHelpSearch(found, q) : found;
  }, [cmds, query, recent]);
  const flat = useMemo(() => sections.flatMap((s) => s.rows), [sections]);

  useEffect(() => {
    inputRef.current?.focus();
    const back = opener.current as HTMLElement | null;
    return () => {
      // Focusing the opener would raise its window over the one a command just opened.
      if (!ran.current && back?.isConnected) back.focus?.({ preventScroll: true });
    };
  }, []);

  useEffect(() => setHi(0), [query]);
  // Escape closes it from anywhere: focus that wandered out of the field (a click on the scrim's
  // edge, a window behind) still ends the palette, before any window takes the key.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || (e.target instanceof Node && e.target.isConnected && (e.target as Element).closest?.(".palette"))) return;
      e.preventDefault();
      e.stopPropagation();
      closePanel();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
  useEffect(() => {
    listRef.current?.querySelector(`[data-row="${hi}"]`)?.scrollIntoView({ block: "nearest" });
  }, [hi]);
  useEffect(() => {
    if (query) announce(flat.length ? `${flat.length} result${flat.length === 1 ? "" : "s"}` : "No results");
  }, [flat.length, query]);

  const run = (r: Ranked | undefined) => {
    if (!r) return;
    const reason = rowReason(r.cmd, lookOnly);
    if (reason) {
      announce(`${r.cmd.title}: ${reason}`, "assertive");
      return;
    }
    // Close first and run once it's gone, so the window or dialog the command opens comes
    // to the front with focus, and nothing hands focus back to the window behind.
    ran.current = true;
    const back = opener.current as HTMLElement | null;
    const raised = raiseSeq();
    closePanel();
    window.setTimeout(() => {
      void runResolved(r.cmd, { remember: true });
      // A command that opened or focused no window (Lock, a theme) gives focus back to where it was.
      window.setTimeout(() => {
        const now = document.activeElement;
        if ((!now || now === document.body) && raiseSeq() === raised && back?.isConnected) back.focus?.({ preventScroll: true });
      }, 120);
    }, 0);
  };

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "ArrowDown" || (e.key === "n" && e.ctrlKey)) {
      e.preventDefault();
      setHi((h) => Math.min(flat.length - 1, h + 1));
    } else if (e.key === "ArrowUp" || (e.key === "p" && e.ctrlKey)) {
      e.preventDefault();
      setHi((h) => Math.max(0, h - 1));
    } else if (e.key === "PageDown") {
      e.preventDefault();
      setHi((h) => Math.min(flat.length - 1, h + 8));
    } else if (e.key === "PageUp") {
      e.preventDefault();
      setHi((h) => Math.max(0, h - 8));
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(flat[hi]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (query) setQuery("");
      else closePanel();
    }
  };

  let row = -1;
  const locked = !adminUnlocked();
  return (
    <div className="palette-scrim" data-shell-overlay onPointerDown={(e) => e.target === e.currentTarget && closePanel()}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Command palette" onKeyDown={onKeyDown}>
        <div className="palette-field">
          <Search size={16} aria-hidden />
          <input
            ref={inputRef}
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-autocomplete="list"
            aria-activedescendant={flat.length ? `palette-row-${hi}` : undefined}
            aria-label="Search commands, windows, nodes, containers and Help"
            placeholder="Search windows, nodes, containers, commands and Help"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
          <kbd className="kbd">esc</kbd>
        </div>
        <div className="palette-list" id="palette-list" role="listbox" aria-label="Results" ref={listRef}>
          {flat.length === 0 && <div className="palette-empty">No commands match “{query}”.</div>}
          {sections.map((s) => (
            <div key={s.name} role="group" aria-label={s.name}>
              <div className="palette-group" aria-hidden>{s.name}</div>
              {s.rows.map((r) => {
                row++;
                const i = row;
                const reason = rowReason(r.cmd, lookOnly);
                const Icon = r.cmd.icon ?? ChevronRight;
                const needsUnlock = r.cmd.needsAdmin && locked;
                return (
                  <div
                    key={r.cmd.id}
                    id={`palette-row-${i}`}
                    data-row={i}
                    role="option"
                    aria-selected={i === hi}
                    aria-disabled={reason ? true : undefined}
                    className={`palette-row${i === hi ? " hi" : ""}${reason ? " disabled" : ""}`}
                    onPointerMove={() => i !== hi && setHi(i)}
                    onClick={() => run(r)}
                  >
                    <Icon size={15} className="palette-icon" aria-hidden />
                    <span className="palette-text">
                      <span className="palette-title">{highlight(r.cmd.title, r.hits)}</span>
                      {(reason || r.cmd.subtitle) && <span className="palette-sub">{reason ?? r.cmd.subtitle}</span>}
                    </span>
                    {needsUnlock && (
                      <span className="palette-lock" title={lookOnly ? LOOK_ONLY_TIP : NEEDS_ADMIN_TIP}>
                        <Lock size={12} aria-label={lookOnly ? "Look-only" : "Needs Admin mode"} />
                      </span>
                    )}
                    {r.cmd.shortcut && <kbd className="kbd">{r.cmd.shortcut}</kbd>}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
        <div className="palette-foot" aria-hidden>
          <span><kbd className="kbd">↑</kbd><kbd className="kbd">↓</kbd> to move</span>
          <span><kbd className="kbd">return</kbd> to run</span>
          {lookOnly
            ? <span><Lock size={11} /> Look-only: nothing runs</span>
            : locked && flat.some((r) => r.cmd.needsAdmin) && <span><Lock size={11} /> asks to unlock Admin mode first</span>}
        </div>
      </div>
    </div>
  );
}

function highlight(text: string, hits: number[]): ReactNode {
  if (!hits.length) return text;
  const set = new Set(hits);
  const out: ReactNode[] = [];
  let buf = "";
  let on = false;
  const flush = (k: number) => {
    if (!buf) return;
    out.push(on ? <mark key={k}>{buf}</mark> : buf);
    buf = "";
  };
  for (let i = 0; i < text.length; i++) {
    const hit = set.has(i);
    if (hit !== on) {
      flush(i);
      on = hit;
    }
    buf += text[i];
  }
  flush(text.length);
  return out;
}
