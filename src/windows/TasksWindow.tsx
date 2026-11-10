// Every task SparkBox has run on the cluster (the last 200), like Proxmox's task list: what,
// where, when, how it ended. The detail shows each step with its timing, the log, and the
// error, with Cancel for running tasks and Retry for finished ones. Retry reopens the
// original confirmation (typed name, impact, Fleet's tier); it never replays the call.
// Only the current data source's tasks are listed: with Live data, never a simulated
// "Shut down cell3"; with Demo data the window says they're Demo.
import { useEffect, useMemo, useState } from "react";
import { Ban, Check, CircleDashed, ListChecks, LoaderCircle, OctagonX, RotateCcw, ScrollText } from "lucide-react";
import { Icon, StopIcon } from "../components/Icon";
import { Table, type Column } from "../components/Table";
import { TaskProgress, TaskStateIcon, fmtElapsed, fmtEta, stateWord, taskDuration } from "../components/TasksChip";
import { Badge, CodeBlock, EmptyState, HelpTip, Notice, SearchField, Segmented, Spacer, Toolbar, ToolButton } from "../components/ui";
import { useDataSource, type Source } from "../lib/dataSource";
import { showError } from "../lib/errors";
import { fmtBytes } from "../lib/format";
import { canRetry, cancelTask, retryTask, taskOrigin, useFocusTask, useTasks, type Task, type TaskStep } from "../lib/tasks";
import { useCluster } from "../lib/store";
import { useWM, useWinState, type Params } from "../wm/WindowManager";
import "./ops.css";

type Show = "all" | "running" | "failed";

/** A task as the backend sends it: with the data source it ran under (tasks.rs `Task.demo`). */
type SourcedTask = Task & { demo?: boolean };

/**
 * The tasks of one data source. The backend lists the current source's only; this also drops
 * the ones the app heard about before a switch. A task started in this window (a copy) belongs
 * to the source it was started under, so one without a source counts as the current one; a
 * backend task without one predates sources and is Demo.
 */
export function tasksFor(list: Task[], source: Source): Task[] {
  return list.filter((t) => {
    const demo = (t as SourcedTask).demo;
    if (demo === undefined) return t.client ? true : source === "demo";
    return demo === (source === "demo");
  });
}

export function relTime(ms: number, now = Date.now()): string {
  const s = Math.round((now - ms) / 1000);
  if (s < 5) return "just now";
  if (s < 60) return `${s} s ago`;
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

export function absTime(ms: number): string {
  return new Date(ms).toLocaleString([], { dateStyle: "medium", timeStyle: "medium" });
}

const STEP_ICON = { running: LoaderCircle, ok: Check, failed: OctagonX, skipped: CircleDashed } as const;

function StepsTimeline({ t }: { t: Task }) {
  const now = Date.now();
  if (!t.steps.length) return <div className="muted">No steps recorded.</div>;
  return (
    <ol className="ops-steps" aria-label="Steps">
      {t.steps.map((s: TaskStep, i) => (
        <li key={i} className={`ops-step ${s.state}`}>
          <Icon icon={STEP_ICON[s.state]} size={14} className={s.state === "running" ? "spin" : undefined} aria-label={s.state} />
          <div className="ops-step-body">
            <div className="ops-step-label">{s.label}</div>
            {s.detail && <div className={`ops-step-detail${s.state === "failed" ? " crit" : ""}`}>{s.detail}</div>}
          </div>
          <span className="muted num" title={absTime(s.startedMs)}>
            {s.state === "skipped" ? "skipped" : fmtElapsed((s.finishedMs ?? now) - s.startedMs)}
          </span>
        </li>
      ))}
    </ol>
  );
}

export function TaskDetail({ t }: { t: Task }) {
  const wm = useWM();
  const cluster = useCluster();
  const names = (ids: string[]) => ids.map((id) => cluster?.nodes.find((n) => n.id === id)?.name ?? id).join(", ");
  const [busy, setBusy] = useState(false);
  // The window it was started from, by its title while it's open.
  const origin = taskOrigin(t.id);
  const originWin = origin ? wm.windows.find((w) => w.id === origin) : undefined;
  const startedFrom = t.source === "tray" ? "Menu bar" : t.source === "system" ? "SparkBox" : originWin ? wm.titleFor(originWin) : t.client ? "This window" : "SparkBox window";
  const retry = async () => {
    setBusy(true);
    try {
      await retryTask(t);
    } catch (e) {
      showError(e, { action: `Retry “${t.title}”` });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="ops-detail" aria-label="Task details">
      <div className="ops-detail-head">
        <TaskStateIcon t={t} size={16} />
        <div className="ops-detail-titles">
          <h3 className="ops-detail-title">{t.title}</h3>
          {t.subtitle && <div className="mono muted ops-detail-sub" title={t.subtitle}>{t.subtitle}</div>}
        </div>
        <Badge tone={t.state === "ok" ? (t.warning ? "warn" : "neutral") : t.state === "failed" ? "crit" : t.state === "running" ? "info" : "neutral"}>{stateWord(t)}</Badge>
        <Spacer />
        {t.state === "running" && t.cancellable && (
          <button type="button" className="btn small" disabled={t.cancelRequested} onClick={() => cancelTask(t.id).catch((e) => showError(e, { action: `Stop “${t.title}”` }))}>
            <Icon icon={StopIcon} size={12} /> {t.cancelRequested ? "Stopping…" : "Cancel"}
          </button>
        )}
        {canRetry(t) && (
          <button type="button" className="btn small" disabled={busy} onClick={retry} title="Asks again first, with the same targets and options">
            <Icon icon={RotateCcw} size={12} /> Retry…
          </button>
        )}
        {t.opId && (
          <button type="button" className="btn small" onClick={() => wm.open("activity", { op: t.opId! })} title="The commands this task ran">
            <Icon icon={ScrollText} size={12} /> Commands
          </button>
        )}
      </div>
      <dl className="ops-kv">
        <div><dt>Started</dt><dd title={absTime(t.startedMs)}>{relTime(t.startedMs)} · {new Date(t.startedMs).toLocaleTimeString()}</dd></div>
        <div><dt>Took</dt><dd className="num">{fmtElapsed(taskDuration(t))}{t.state === "running" ? " so far" : ""}</dd></div>
        <div><dt>Nodes</dt><dd>{t.nodes.length ? names(t.nodes) : "—"}</dd></div>
        <div><dt>Started from</dt><dd>{startedFrom}</dd></div>
      </dl>
      {t.state === "running" && (
        <div className="ops-detail-progress">
          <TaskProgress t={t} />
          <div className="muted ops-progress-text">
            {t.detail ?? "Working…"}
            {t.rateBps !== null && ` · ${fmtBytes(t.rateBps)}/s`}
            {fmtEta(t.etaS) && ` · ${fmtEta(t.etaS)}`}
          </div>
        </div>
      )}
      {t.error && <Notice tone="error">{t.error}</Notice>}
      {t.warning && <Notice tone="warn">{t.warning}</Notice>}
      {t.summary && t.state !== "failed" && <Notice tone={t.state === "ok" && !t.warning ? "success" : "info"}>{t.summary}</Notice>}
      <div className="section-title">Steps</div>
      <StepsTimeline t={t} />
      {t.log.length > 0 && (
        <>
          <div className="section-title">Log</div>
          <CodeBlock text={t.log.join("\n")} maxHeight={220} label="Task log" />
        </>
      )}
    </div>
  );
}

export function TasksWindow({ params }: { params: Params }) {
  const source = useDataSource();
  const all = useTasks();
  const tasks = useMemo(() => tasksFor(all, source), [all, source]);
  const cluster = useCluster();
  const [show, setShow] = useWinState<Show>("show", "all");
  const [text, setText] = useState("");
  const focus = useFocusTask();
  const [selected, setSelected] = useState<string | null>(params.task ?? focus?.id ?? null);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (focus) {
      setSelected(focus.id);
      setShow("all");
    }
  }, [focus?.seq]);
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);

  const name = (id: string) => cluster?.nodes.find((n) => n.id === id)?.name ?? id;
  const needle = text.trim().toLowerCase();
  const rows = useMemo(
    () => tasks
      .filter((t) => show === "all" || (show === "running" ? t.state === "running" : t.state === "failed"))
      .filter((t) => !needle || `${t.title} ${t.nodes.join(" ")} ${t.kind} ${t.error ?? ""}`.toLowerCase().includes(needle)),
    [tasks, show, needle],
  );
  const running = tasks.filter((t) => t.state === "running").length;
  const failed = tasks.filter((t) => t.state === "failed").length;
  const current = tasks.find((t) => t.id === selected) ?? null;

  const columns: Column<Task>[] = [
    {
      key: "state", label: "State", fit: "content", priority: 1, sort: (t) => t.state,
      render: (t) => <span className="ops-state"><TaskStateIcon t={t} size={12} />{stateWord(t)}</span>,
    },
    { key: "title", label: "Task", priority: 1, minWidth: 200, sort: (t) => t.title, title: (t) => (t.subtitle ? `${t.title}\n${t.subtitle}` : t.title), render: (t) => <span className="strong">{t.title}</span> },
    { key: "nodes", label: "Nodes", fit: "flex", priority: 2, sort: (t) => t.nodes.join(","), render: (t) => t.nodes.map(name).join(", ") || "—" },
    {
      key: "started", label: "Started", fit: "content", priority: 1, sort: (t) => t.startedMs,
      title: (t) => absTime(t.startedMs), render: (t) => relTime(t.startedMs, now),
    },
    { key: "took", label: "Took", align: "right", fit: "content", priority: 2, sort: (t) => taskDuration(t, now), render: (t) => <span className="num">{fmtElapsed(taskDuration(t, now))}</span> },
    {
      key: "outcome", label: "Outcome", fit: "flex", priority: 3, title: (t) => t.error ?? t.summary ?? "",
      render: (t) => <span className={t.state === "failed" ? "crit-text" : t.warning ? "warn-text" : "muted"}>{t.state === "running" ? t.detail ?? "" : t.error ?? t.warning ?? t.summary ?? ""}</span>,
    },
  ];

  // With the detail open the list is narrow: Nodes and Outcome go (the detail has both) rather than being cut.
  const shown = current ? columns.filter((c) => c.key !== "nodes" && c.key !== "outcome") : columns;

  return (
    <div className="col fill ops-window">
      <Toolbar label="Tasks">
        <Segmented<Show>
          value={show}
          onChange={setShow}
          label="Show"
          options={[
            { value: "all", label: "All" },
            { value: "running", label: `Running${running ? ` (${running})` : ""}` },
            { value: "failed", label: `Failed${failed ? ` (${failed})` : ""}` },
          ]}
        />
        <Spacer />
        <ToolButton icon={Ban} label="Cancel" priority={2} disabled={!current || current.state !== "running" || !current.cancellable || current.cancelRequested}
          title={current && current.state === "running" && !current.cancellable ? "This task can't be stopped part-way" : "Stop the selected task"}
          onClick={() => current && cancelTask(current.id).catch((e) => showError(e))} />
        {source === "demo" && (
          <span title="These tasks ran against the simulator, not your cluster">
            <Badge tone="neutral">Demo</Badge>
          </span>
        )}
        <SearchField value={text} onChange={setText} placeholder="Filter tasks" count={rows.length} />
        <HelpTip topic="tasks-and-jobs" />
      </Toolbar>
      {tasks.length === 0 ? (
        <EmptyState icon={ListChecks} title="No tasks yet"
          body="Restarts, service and container actions, Fleet runs and upgrades appear here with their steps and output, and stay for the next 200 tasks." />
      ) : (
        // Like the Activity log: the detail pane opens with a selection, so the list gets the room until then.
        <div className={`ops-split${current ? "" : " no-detail"}`}>
          <div className="ops-split-list">
            <Table
              columns={shown}
              rows={rows}
              rowKey={(t) => t.id}
              selected={selected}
              onSelect={setSelected}
              tableId="tasks"
              ariaLabel="Tasks"
              footer
              itemNoun={["task", "tasks"]}
              rowClass={(t) => (t.state === "failed" ? "row-crit" : t.warning ? "row-warn" : undefined)}
              empty={show === "running" ? "Nothing is running." : show === "failed" ? "No failed tasks." : "No tasks match."}
            />
          </div>
          {current && (
            <div className="ops-split-detail">
              <TaskDetail t={current} />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
