import { useEffect, useMemo, useRef, useState } from "react";
import { Box, Clock, ScrollText, Settings2, SquareTerminal, type LucideIcon } from "lucide-react";
import { HelpTip, StatusDot } from "../../components/ui";
import { useAdmin } from "../../lib/admin";
import { useDataSource } from "../../lib/dataSource";
import { fmtDuration } from "../../lib/format";
import { useCluster } from "../../lib/store";
import {
  dismissRestore, needsAdmin, openTerminal, recentSessions, restoreSession, restoreSummary, setPicker, titleFor, unlockForShell, useTerminals,
  type TabDescriptor,
} from "../../lib/terminals";
import type { NodeSnapshot } from "../../lib/types";

const ICON: Record<string, LucideIcon> = { shell: SquareTerminal, containerShell: Box, containerLogs: ScrollText, journal: Settings2 };

interface Row {
  id: string;
  icon: LucideIcon;
  title: string;
  sub: string;
  node?: NodeSnapshot;
  disabled?: string;
  run: () => void;
}

/** Opens a session, asking to unlock Admin mode first for a shell: Touch ID with Live data, a dialog in Demo. */
export async function connectTo(d: TabDescriptor) {
  if (needsAdmin(d.target) && !(await unlockForShell(d.node))) return;
  openTerminal(d.node, d.target, d.title ? { title: d.title } : {});
}

function ago(ms: number): string {
  const s = (Date.now() - ms) / 1000;
  return s < 60 ? "just now" : `${fmtDuration(s).split(" ")[0]} ago`;
}

/**
 * The quick-connect list: every node with its status and latency, then recent sessions (this
 * data source's only). As the Terminal's empty state it sits in the window; as the ⌘T picker it
 * floats, with a filter field and arrow keys. A Terminal tiled smaller than the list needs keeps
 * its nodes as one row of buttons (terminal.css › the tile picker).
 */
export function QuickConnect({ floating }: { floating?: boolean }) {
  const cluster = useCluster();
  const admin = useAdmin();
  const { restorable } = useTerminals();
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  // This data source's sessions only: another source's would open on machines that aren't these.
  const source = useDataSource();
  const recents = useMemo(() => recentSessions(), [source]);

  useEffect(() => {
    if (floating) input.current?.focus();
  }, [floating]);

  const nodes = cluster?.nodes ?? [];
  const rows: Row[] = [
    ...nodes.map((n): Row => ({
      id: `node:${n.id}`,
      icon: SquareTerminal,
      title: n.name,
      sub: n.status === "online"
        ? `${n.host}${n.latencyMs != null ? ` · ${n.latencyMs.toFixed(n.latencyMs < 10 ? 1 : 0)} ms` : ""}`
        : n.status === "connecting" ? "Connecting…" : (n.error ?? "Offline"),
      node: n,
      run: () => void connectTo({ node: n.id, target: { kind: "shell" } }),
    })),
    ...recents.map((r, i): Row => ({
      id: `recent:${i}`,
      icon: ICON[r.target.kind] ?? Clock,
      title: r.title ?? titleFor(r.node, r.target),
      sub: `${r.target.kind === "shell" ? "Shell" : r.target.kind === "containerShell" ? "Container shell" : r.target.kind === "containerLogs" ? "Logs" : "Journal"} on ${r.node} · ${ago(r.atMs)}`,
      run: () => void connectTo(r),
    })),
  ];
  const q = query.trim().toLowerCase();
  const shown = q ? rows.filter((r) => `${r.title} ${r.sub}`.toLowerCase().includes(q)) : rows;
  const at = Math.min(cursor, Math.max(0, shown.length - 1));
  const firstRecent = shown.findIndex((r) => r.id.startsWith("recent:"));

  const run = (r: Row | undefined) => {
    if (!r) return;
    if (floating) setPicker(false);
    r.run();
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = Math.max(0, Math.min(shown.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)));
      setCursor(next);
      list.current?.querySelectorAll<HTMLElement>("[data-row]")[next]?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(shown[at]);
    } else if (e.key === "Escape" && floating) {
      e.preventDefault();
      e.stopPropagation();
      setPicker(false);
    }
  };

  const body = (
    <>
      {floating && (
        <input
          ref={input}
          className="sb-qc-filter"
          value={query}
          placeholder="Connect to a node or a recent session…"
          aria-label="Filter"
          spellCheck={false}
          onChange={(e) => { setQuery(e.target.value); setCursor(0); }}
          onKeyDown={onKey}
        />
      )}
      {!floating && (
        <div className="sb-qc-intro">
          <div className="sb-qc-intro-title">Open a terminal <HelpTip topic="open-a-terminal" /></div>
          <div className="muted">A shell on a node, a container's shell, or its logs. Each tab is a real SSH session (simulated in Demo mode) and keeps running while this window is minimized.</div>
        </div>
      )}
      {!floating && restorable.length > 0 && (
        <div className="sb-qc-restore">
          <span>Reopen {restorable.length} terminal tab{restorable.length === 1 ? "" : "s"} from last time? <span className="muted">{restoreSummary()}</span></span>
          <button type="button" className="btn small primary" onClick={restoreSession}>Reopen</button>
          <button type="button" className="btn small" onClick={dismissRestore}>Not now</button>
        </div>
      )}
      {!floating && (
        <div className="sb-qc-compact" role="group" aria-label="Open a shell on">
          <span className="sb-qc-compact-label muted">Shell on</span>
          {nodes.map((n) => (
            <button key={n.id} type="button" className="btn small sb-qc-chip" onClick={() => void connectTo({ node: n.id, target: { kind: "shell" } })}
              title={`${n.name}: ${n.status === "online" ? n.host : n.error ?? "offline"}${admin ? "" : " · needs Admin"}`}>
              <StatusDot status={n.status} />
              {n.name}
            </button>
          ))}
        </div>
      )}
      <div className="sb-qc-list" role="listbox" aria-label="Quick connect" ref={list} onKeyDown={floating ? undefined : onKey} tabIndex={floating ? -1 : 0}>
        {shown.length > 0 && !shown[0].id.startsWith("recent:") && <div className="sb-qc-head">Nodes</div>}
        {shown.map((r, i) => {
          const Icon = r.icon;
          return (
            <div key={r.id} className="sb-qc-group">
              {i === firstRecent && <div className="sb-qc-head">Recent sessions</div>}
              <button
                type="button"
                data-row
                role="option"
                aria-selected={i === at}
                className={`sb-qc-row${i === at ? " on" : ""}`}
                onMouseEnter={() => setCursor(i)}
                onClick={() => run(r)}
              >
                <Icon size={15} className="sb-qc-icon" />
                <span className="sb-qc-title">{r.title}</span>
                {r.node && <StatusDot status={r.node.status} />}
                <span className="sb-qc-sub">{r.sub}</span>
                {r.id.startsWith("node:") && !admin && <span className="sb-qc-lock muted">needs Admin</span>}
              </button>
            </div>
          );
        })}
        {shown.length === 0 && <div className="sb-qc-empty muted">Nothing matches “{query}”.</div>}
      </div>
      <div className="sb-qc-foot muted">
        {floating ? "↑↓ to choose · Return to connect · Esc to close" : "⌘T opens this list from any tab · shells need Admin mode; logs don't"}
      </div>
    </>
  );

  if (!floating) return <div className="sb-qc">{body}</div>;
  return (
    <div className="sb-qc-backdrop" onPointerDown={(e) => { if (e.target === e.currentTarget) setPicker(false); }}>
      <div className="sb-qc floating" role="dialog" aria-label="New terminal tab">{body}</div>
    </div>
  );
}
