import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { Keyboard, X } from "lucide-react";
import { activeCommandIds, getCommand, useCommandList } from "../lib/commands";
import { displayAccel, PAGE_SHORTCUTS, SHORTCUTS, type Shortcut } from "../lib/keymap";
import { closePanel, usePanel } from "../lib/session";
import { useWM } from "../wm/WindowManager";
import { HelpTip } from "./ui";
import "./shell.css";

const MENU_ORDER: Shortcut["menu"][] = ["File", "Edit", "View", "Go", "Cluster", "Window", "SparkBox", "Help"];
const MENU_TITLE: Record<Shortcut["menu"], string> = {
  SparkBox: "App", File: "File", Edit: "Edit", View: "View", Go: "Go to", Cluster: "Cluster", Window: "Window", Help: "Help",
};

/**
 * Keyboard Shortcuts (⌘/): built from the menu bar's table, plus the keys the page handles
 * itself. The global bindings come first under their menus; what the window in front does
 * differently with the same keys (Containers' ⌘T opens a shell in the selected container)
 * is listed apart, under that window's name.
 */
export function ShortcutsHelp() {
  const panel = usePanel();
  if (panel !== "shortcuts") return null;
  return createPortal(<Sheet />, document.body);
}

function Sheet() {
  useCommandList(); // re-render when commands come and go (e.g. Alerts once registered)
  const wm = useWM();
  const front = wm.windows.find((w) => w.id === wm.activeId);
  const sheetRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const back = document.activeElement as HTMLElement | null;
    // The sheet itself takes focus (Tab reaches Close), so nothing lights up on open.
    sheetRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closePanel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      if (back?.isConnected) back.focus({ preventScroll: true });
    };
  }, []);

  // The front window's own commands that take a global binding's keys.
  const scoped = activeCommandIds();
  const overrides = front
    ? SHORTCUTS.filter((s) => scoped.has(s.id) && getCommand(s.id)).map((s) => ({ label: getCommand(s.id)!.title, keys: displayAccel(s.accel), global: s.label.replace(/…$/, "") }))
    : [];
  const groups = MENU_ORDER.map((menu) => ({
    title: MENU_TITLE[menu],
    // Only what this build has: Alerts or Tasks show up once their windows exist. The menu's own
    // label, so an override in the window in front doesn't rename the global binding.
    rows: SHORTCUTS.filter((s) => s.menu === menu && getCommand(s.id)).map((s) => ({
      label: scoped.has(s.id) ? s.label.replace(/…$/, "") : getCommand(s.id)?.title ?? s.label.replace(/…$/, ""),
      keys: displayAccel(s.accel),
    })),
  })).filter((g) => g.rows.length);
  // Keys the page handles join the menu's group of the same name, or make their own.
  const all = groups.map((g) => ({ ...g, rows: [...g.rows] }));
  for (const p of PAGE_SHORTCUTS) {
    const row = { label: p.label, keys: p.keys };
    const group = all.find((g) => g.title === p.group);
    if (group) group.rows.push(row);
    else all.push({ title: p.group, rows: [row] });
  }

  return (
    <div className="sb-overlay" data-shell-overlay onPointerDown={(e) => e.target === e.currentTarget && closePanel()}>
      <div ref={sheetRef} className="shortcuts" role="dialog" aria-modal="true" aria-labelledby="shortcuts-title" tabIndex={-1}>
        <header className="shortcuts-head">
          <Keyboard size={18} aria-hidden />
          <h2 id="shortcuts-title">Keyboard shortcuts</h2>
          <HelpTip topic="keyboard-shortcuts" />
          <span className="spacer" />
          <button className="icon-btn" aria-label="Close" title="Close (Esc)" onClick={closePanel}><X size={14} aria-hidden /></button>
        </header>
        {overrides.length > 0 && (
          <section className="shortcuts-group shortcuts-front" aria-label={`In ${wm.titleFor(front!)}`}>
            <h3>In {wm.titleFor(front!)}</h3>
            <dl>
              {overrides.map((r) => (
                <div key={r.keys}>
                  <dt>{r.label} <span className="muted">(instead of {r.global})</span></dt>
                  <dd><kbd className="kbd">{r.keys}</kbd></dd>
                </div>
              ))}
            </dl>
          </section>
        )}
        <div className="shortcuts-grid">
          {all.map((g) => (
            <section key={g.title} className="shortcuts-group">
              <h3>{g.title}</h3>
              <dl>
                {g.rows.map((r) => (
                  <div key={r.label}>
                    <dt>{r.label}</dt>
                    <dd><kbd className="kbd">{r.keys}</kbd></dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
        <footer className="shortcuts-foot muted">
          Every command is also in the menu bar and in the command palette (<kbd className="kbd">⌘K</kbd>).
        </footer>
      </div>
    </div>
  );
}
