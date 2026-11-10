// The status bar's task indicator. Nothing while idle; a spinner and "2 running" while tasks
// run; for 10 s after one ends, how it ended. Clicking opens a drawer with the running tasks
// (progress, rate, time left, current step, Cancel) and the 20 most recent.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ExternalLink, ListChecks, LoaderCircle, OctagonX, Square, TriangleAlert, X } from "lucide-react";
import { fmtBytes } from "../lib/format";
import { cancelTask, openTasksWindow, useTasks, type Task } from "../lib/tasks";
import { showError } from "../lib/errors";
import { useWM } from "../wm/WindowManager";
import { Icon, type IconSize } from "./Icon";
import "./tasks.css";

const RECENT = 20;
const LINGER_MS = 10_000;

export function fmtElapsed(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(s / 3600)}h${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}m`;
}

export function fmtEta(s: number | null): string | null {
  if (s === null || !Number.isFinite(s) || s < 0) return null;
  return `${fmtElapsed(s * 1000)} left`;
}

export function taskDuration(t: Task, now = Date.now()): number {
  return (t.finishedMs ?? now) - t.startedMs;
}

export const STATE_WORD: Record<Task["state"], string> = { running: "Running", ok: "Done", failed: "Failed", cancelled: "Stopped" };

/** "Done", "Done with a warning", "Failed"… */
export function stateWord(t: Task): string {
  return t.state === "ok" && t.warning ? "Done with a warning" : STATE_WORD[t.state];
}

export function TaskStateIcon({ t, size = 14 }: { t: Task; size?: IconSize }) {
  if (t.state === "running") return <Icon icon={LoaderCircle} size={size} className="spin task-icon running" aria-label="Running" />;
  if (t.state === "ok" && t.warning) return <Icon icon={TriangleAlert} size={size} className="task-icon warning" aria-label="Done with a warning" />;
  if (t.state === "ok") return <Icon icon={Check} size={size} className="task-icon ok" aria-label="Done" />;
  if (t.state === "cancelled") return <Icon icon={Square} size={size} className="task-icon cancelled" aria-label="Stopped" />;
  return <Icon icon={OctagonX} size={size} className="task-icon failed" aria-label="Failed" />;
}

/** A running task's progress bar (indeterminate when the fraction isn't known). */
export function TaskProgress({ t }: { t: Task }) {
  const pct = t.progress === null ? null : Math.round(t.progress * 100);
  return (
    <div
      className={`task-bar${pct === null ? " indeterminate" : ""}`}
      role="progressbar"
      aria-label={t.title}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={pct ?? undefined}
    >
      <div className="task-bar-fill" style={pct === null ? undefined : { width: `${pct}%` }} />
    </div>
  );
}

function useNow(active: boolean, every = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setNow(Date.now()), every);
    return () => window.clearInterval(t);
  }, [active, every]);
  return now;
}

export function TasksChip() {
  const wm = useWM();
  const tasks = useTasks();
  const [open, setOpen] = useState(false);
  const chipRef = useRef<HTMLButtonElement>(null);
  const running = tasks.filter((t) => t.state === "running");
  const last = tasks.find((t) => t.state !== "running" && t.finishedMs !== null);
  const now = useNow(true);
  const lingering = !running.length && last && last.finishedMs !== null && now - last.finishedMs < LINGER_MS;

  // Other code asks for windows by event (tasks.ts, errors.ts, Models' "Chat with").
  useEffect(() => {
    const onOpen = (e: Event) => {
      const d = (e as CustomEvent<{ kind: string; params?: Record<string, string> }>).detail;
      if (d?.kind) wm.open(d.kind, d.params ?? {});
    };
    window.addEventListener("sparkbox:open-window", onOpen);
    return () => window.removeEventListener("sparkbox:open-window", onOpen);
  }, [wm.open]);

  if (!running.length && !lingering && !open) return null;

  const label = running.length
    ? running.length === 1 ? running[0].title : `${running.length} running`
    : last ? `${last.state === "ok" && !last.warning ? "" : `${stateWord(last)}: `}${last.title}` : "Tasks";

  return (
    <>
      <button
        ref={chipRef}
        type="button"
        className={`sb-item tasks-chip${running.length ? " busy" : last ? ` ${last.state === "ok" && last.warning ? "warning" : last.state}` : ""}`}
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        aria-haspopup="dialog"
        title={running.length ? `${running.length} task${running.length === 1 ? "" : "s"} running — click for details` : "Recent tasks"}
      >
        {running.length ? <Icon icon={LoaderCircle} size={12} className="spin" /> : last ? <TaskStateIcon t={last} size={12} /> : <Icon icon={ListChecks} size={12} />}
        <span className="tasks-chip-label">{label}</span>
        {running.length === 1 && running[0].progress !== null && <span className="muted num">{Math.round(running[0].progress * 100)}%</span>}
      </button>
      {open && <TasksDrawer anchor={chipRef.current} tasks={tasks} onClose={() => setOpen(false)} onOpen={(id) => {
        setOpen(false);
        openTasksWindow(id);
      }} />}
    </>
  );
}

function TasksDrawer({ anchor, tasks, onClose, onOpen }: {
  anchor: HTMLElement | null;
  tasks: Task[];
  onClose: () => void;
  onOpen: (id?: string) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ bottom: number; right: number }>({ bottom: 28, right: 8 });
  const running = tasks.filter((t) => t.state === "running");
  const recent = tasks.filter((t) => t.state !== "running").slice(0, RECENT);
  const now = useNow(running.length > 0);

  useLayoutEffect(() => {
    if (!anchor) return;
    const r = anchor.getBoundingClientRect();
    setPos({ bottom: window.innerHeight - r.top + 4, right: Math.max(8, window.innerWidth - r.right) });
  }, [anchor]);

  useEffect(() => {
    ref.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
        anchor?.focus();
      }
    };
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node) && !anchor?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown);
    };
  }, [anchor, onClose]);

  return createPortal(
    <div ref={ref} className="tasks-drawer" role="dialog" aria-label="Tasks" tabIndex={-1} style={{ bottom: pos.bottom, right: pos.right }}>
      <div className="tasks-drawer-head">
        <span className="section-title">Running{running.length ? ` · ${running.length}` : ""}</span>
        <span className="spacer" />
        <button type="button" className="btn small" onClick={() => onOpen()}>
          <Icon icon={ExternalLink} size={12} /> All tasks
        </button>
        <button type="button" className="notice-close" onClick={onClose} aria-label="Close" title="Close">
          <Icon icon={X} size={12} />
        </button>
      </div>
      {running.length === 0 && <div className="tasks-empty muted">Nothing running. Reboots, service and container actions, Fleet runs and upgrades show up here while they work.</div>}
      {running.map((t) => (
        <div key={t.id} className="task-running">
          <div className="task-row">
            <TaskStateIcon t={t} />
            <span className="task-title" title={t.title}>{t.title}</span>
            <span className="muted num">{fmtElapsed(taskDuration(t, now))}</span>
            {t.cancellable && (
              <button type="button" className="btn small" disabled={t.cancelRequested} onClick={() => cancelTask(t.id).catch((e) => showError(e, { action: `Stop “${t.title}”` }))}>
                {t.cancelRequested ? "Stopping…" : "Cancel"}
              </button>
            )}
          </div>
          <TaskProgress t={t} />
          <div className="task-sub muted">
            <span className="task-step">{t.detail ?? "Starting…"}</span>
            {t.rateBps !== null && <span className="num">{fmtBytes(t.rateBps)}/s</span>}
            {fmtEta(t.etaS) && <span className="num">{fmtEta(t.etaS)}</span>}
          </div>
        </div>
      ))}
      <div className="tasks-drawer-head">
        <span className="section-title">Recent</span>
      </div>
      {recent.length === 0 && <div className="tasks-empty muted">No finished tasks yet.</div>}
      <ul className="task-recent" aria-label="Recent tasks">
        {recent.map((t) => (
          <li key={t.id} className={`task-row ${t.state}`}>
            <TaskStateIcon t={t} />
            <span className="task-title" title={t.error ?? t.summary ?? t.title}>{t.title}</span>
            <span className="muted task-outcome" title={t.warning ?? t.error ?? t.summary ?? undefined}>{t.state === "failed" ? t.error ?? "Failed" : t.warning ?? t.summary ?? STATE_WORD[t.state]}</span>
            <span className="muted num">{fmtElapsed(taskDuration(t))}</span>
            <button type="button" className="btn small" onClick={() => onOpen(t.id)} aria-label={`Open ${t.title}`}>Open</button>
          </li>
        ))}
      </ul>
    </div>,
    document.body,
  );
}
