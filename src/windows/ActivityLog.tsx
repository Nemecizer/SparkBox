// The activity log: every command SparkBox ran on a node and every change it made on the Mac,
// grouped by operation ("Restart 3 containers" on cell1–cell3 · 3/3 ok) with what ran, where,
// who asked and how it ended, in the same words as Tasks: ok, ok · warning, failed. Demo
// actions are listed with the command they would have run ("Demo" says so once, in the
// footer). A command that started but never reported back reads "didn't finish". Admin
// unlocks and relocks fold into one "Admin session" row, and a shell's open and close into
// one "Shell" row. Changes follows the audit classes: what changed a node or the Mac,
// authentication (Admin sessions) and root shells; Everything adds reads and checks. A background
// poll is one row, "Check updates · 4 nodes · ok", with its commands in the detail.
import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { ChevronDown, ChevronRight, Download, FolderOpen, ListChecks, LoaderCircle, ScrollText, X } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Icon } from "../components/Icon";
import { Table, type Column } from "../components/Table";
import { Badge, CodeBlock, EmptyState, HelpTip, Notice, SearchField, Segmented, Spacer, Switch, Toolbar, ToolButton } from "../components/ui";
import { useAdmin } from "../lib/admin";
import { inTauri } from "../lib/api";
import { useDataSource } from "../lib/dataSource";
import { showContextMenu } from "../lib/contextMenu";
import { showError } from "../lib/errors";
import { fmtDateTime } from "../lib/format";
import { opsApi, type ActivityEntry } from "../lib/opsApi";
import { useCluster } from "../lib/store";
import { getTask, openTasksWindow } from "../lib/tasks";
import { toast } from "../lib/toasts";
import { useWinState, type Params } from "../wm/WindowManager";
import { absTime, relTime } from "./TasksWindow";
import "./ops.css";

type Scope = "changes" | "everything";
type Period = "today" | "week" | "all";

interface Group {
  key: string;
  head: ActivityEntry;
  entries: ActivityEntry[];
  /** When the operation started (its head line, else its first command), for ordering. */
  ts: number;
  /** An Admin session (unlock … relock): shown under Everything only. */
  session?: boolean;
}

interface Row {
  key: string;
  entry: ActivityEntry;
  group: Group;
  child: boolean;
}

export function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
}

const SOURCE: Record<string, string> = { ui: "You", tray: "Menu bar", system: "SparkBox", palette: "You (palette)", poll: "SparkBox" };

const isAdmin = (e: ActivityEntry) => e.node === "mac" && e.kind === "app" && e.title.startsWith("Admin mode");

function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

/** A shell tab: one entry from open to close (never its keystrokes). */
const isShell = (e: ActivityEntry) => e.kind === "terminal" && e.class === "session";

/** "Shell on cell1 · 12:03–12:41", "Root shell on cell2 · since 14:10". */
export function shellTitle(e: ActivityEntry, nodeName: (id: string) => string): string {
  const span = e.state === "running" ? `since ${clock(e.tsMs)}` : `${clock(e.tsMs)}–${clock(e.tsMs + e.ms)}`;
  return `${e.escalated ? "Root shell" : "Shell"} on ${nodeName(e.node)} · ${span}`;
}

/** "No changes for 15 minutes · unlocked at …" → "relocked after 15 min idle". */
export function relockPhrase(detail: string): string {
  const why = detail.split(" · ")[0].trim();
  const idle = why.match(/^No changes for (\d+) minutes/);
  if (idle) return `relocked after ${idle[1]} min idle`;
  if (why.startsWith("Live data")) return "relocked when Live data was turned on";
  return why ? why.charAt(0).toLowerCase() + why.slice(1) : "locked";
}

/**
 * Folds each Admin unlock with its relock (and any "confirmed it's you" between) into one
 * session: a head entry "Admin session 12:03–12:18 (relocked after 15 min idle)" whose
 * children are the original records. None of it counts as a change.
 */
export function foldAdminSessions(entries: ActivityEntry[], unlockedNow = true): ActivityEntry[] {
  const out: ActivityEntry[] = [];
  const ascending = [...entries].sort((a, b) => a.tsMs - b.tsMs);
  let open: { start: ActivityEntry; members: ActivityEntry[] } | null = null;
  const close = (end: ActivityEntry | null, why: string) => {
    if (!open) return;
    const { start, members } = open;
    const id = `admin-${start.id}`;
    const span = end ? `${clock(start.tsMs)}–${clock(end.tsMs)}` : `since ${clock(start.tsMs)}`;
    out.push({
      // Still open only when it's the latest unlock; one cut off by a later unlock just never recorded its relock.
      ...start, id, kind: "op", opId: id, class: "session", changes: false, state: end || why !== "unlocked now" ? "ok" : "running",
      title: `Admin session ${span} (${why})`, command: start.command, ms: end ? end.tsMs - start.tsMs : 0,
      updatedMs: (end ?? start).updatedMs, stdoutTail: null, stderrTail: null, error: null,
    });
    for (const m of members) out.push({ ...m, opId: id, changes: false });
    open = null;
  };
  for (const e of ascending) {
    if (!isAdmin(e)) {
      out.push(e);
      continue;
    }
    if (e.title === "Admin mode unlocked") {
      close(null, "no relock recorded: SparkBox was stopped");
      open = { start: e, members: [e] };
    } else if (e.title === "Admin mode locked") {
      if (open) {
        open.members.push(e);
        close(e, relockPhrase(e.command));
      } else {
        // Its unlock is older than what's loaded.
        const id = `admin-${e.id}`;
        out.push({ ...e, id, kind: "op", opId: id, class: "session", changes: false, title: `Admin session until ${clock(e.tsMs)} (${relockPhrase(e.command)})` });
        out.push({ ...e, opId: id, changes: false });
      }
    } else if (open) {
      open.members.push(e);
    } else {
      out.push({ ...e, changes: false });
    }
  }
  // The latest unlock is still open only while Admin mode is on; otherwise SparkBox quit before relocking.
  close(null, unlockedNow ? "unlocked now" : "no relock recorded: SparkBox was stopped");
  return out.sort((a, b) => b.tsMs - a.tsMs);
}

/**
 * Entries that share an operation become one group, the latest started first: a group is
 * placed (and dated) by when its operation began, so a long task that's still writing rows
 * doesn't jump above the ones started after it.
 */
export function groupEntries(entries: ActivityEntry[]): Group[] {
  const groups = new Map<string, Group>();
  for (const e of entries) {
    const key = e.opId ?? e.id;
    const g = groups.get(key);
    if (g) {
      g.entries.push(e);
      if (e.kind === "op") g.head = e;
    } else {
      groups.set(key, { key, head: e, entries: [e], ts: e.tsMs, session: e.class === "session" });
    }
  }
  for (const g of groups.values()) {
    g.entries.sort((a, b) => a.tsMs - b.tsMs);
    if (g.head.kind !== "op") g.head = g.entries[0];
    g.session = g.head.class === "session";
    g.ts = g.head.kind === "op" ? g.head.tsMs : g.entries[0].tsMs;
  }
  return [...groups.values()].sort((a, b) => b.ts - a.ts);
}

/** A background poll (audit OpMeta source "poll"): one row, never opened into its commands. */
export const isPoll = (g: Pick<Group, "head">) => g.head.source === "poll";

/**
 * A poll's row beside its title: "4 nodes · ok", "4 nodes · 1 failed" (nodes with a command
 * that failed or didn't finish), "4 nodes · running".
 */
export function pollSummary(g: Pick<Group, "entries">): string {
  const nodes = new Set(g.entries.map((e) => e.node));
  const failed = new Set(g.entries.filter((e) => e.state === "failed" || e.state === "unfinished").map((e) => e.node));
  const how = g.entries.some((e) => e.state === "running") ? "running" : failed.size ? `${failed.size} failed` : "ok";
  return `${nodes.size} node${nodes.size === 1 ? "" : "s"} · ${how}`;
}

/**
 * The commands of an operation, beside its outcome: "3/3 ok", "2/3 ok · 1 failed", "running".
 * Never "3/3 ok" beside a failed outcome: when every command ran but the operation failed
 * (its check didn't pass), it's just "3 commands", and the outcome says the rest.
 */
export function groupSummary(g: Group): string | null {
  if (g.session) return null;
  if (isPoll(g)) return pollSummary(g);
  const commands = g.entries.filter((e) => e.kind !== "op");
  if (g.head.kind === "op" && g.head.state === "running") return "running";
  if (commands.length <= 1 && g.head.kind !== "op") return null;
  if (!commands.length) return null;
  const ok = commands.filter((e) => e.state === "ok" || e.state === "warning").length;
  const failed = commands.filter((e) => e.state === "failed" || e.state === "unfinished").length;
  if (failed) return `${ok}/${commands.length} ok · ${failed} failed`;
  return outcomeOf(groupState(g)).tone === "crit" ? `${commands.length} command${commands.length === 1 ? "" : "s"}` : `${ok}/${commands.length} ok`;
}

/** One outcome vocabulary with Tasks and toasts (audit Outcome): ok, ok · warning, failed. */
export function outcomeOf(state: ActivityEntry["state"]): { word: string; tone: "ok" | "warn" | "crit" | "info" } {
  switch (state) {
    case "ok": return { word: "ok", tone: "ok" };
    case "warning": return { word: "ok · warning", tone: "warn" };
    case "failed": return { word: "failed", tone: "crit" };
    case "unfinished": return { word: "didn't finish", tone: "warn" };
    case "cancelled": return { word: "stopped", tone: "info" };
    default: return { word: "running", tone: "info" };
  }
}

/**
 * Whether an operation belongs under Changes, by its audit class: something that changed a
 * node or the Mac, authentication (an Admin session), or a root shell. Reads, log and journal
 * views, ordinary shells and SparkBox's own checks are under Everything only.
 */
export function isChange(g: Pick<Group, "entries" | "session" | "head">): boolean {
  if (g.session && g.head.title.startsWith("Admin session")) return true;
  return g.entries.some((e) => {
    switch (e.class) {
      case "change": case "destructive": case "app": return true;
      case "session": return e.escalated;
      case "read": case "probe": return false;
      default: return e.changes;
    }
  });
}

/** "cell1–cell3" for a run of numbered nodes, else "cell1, gx10-0000". */
export function nodeRange(names: string[]): string {
  const uniq = [...new Set(names)];
  const m = uniq.map((n) => n.match(/^(.*?)(\d+)$/));
  if (uniq.length > 2 && m.every((x) => x && x[1] === m[0]![1])) {
    const nums = m.map((x) => Number(x![2])).sort((a, b) => a - b);
    if (nums.every((n, i) => i === 0 || n === nums[i - 1] + 1)) return `${m[0]![1]}${nums[0]}–${m[0]![1]}${nums[nums.length - 1]}`;
  }
  return uniq.join(", ");
}

/** An operation's outcome: its head's (the task's), else the worst of its commands. */
export function groupState(g: Pick<Group, "head" | "entries">): ActivityEntry["state"] {
  if (g.head.kind === "op") return g.head.state;
  if (g.entries.some((e) => e.state === "running")) return "running";
  if (g.entries.some((e) => e.state === "unfinished")) return "unfinished";
  if (g.entries.some((e) => e.state === "failed")) return "failed";
  return g.entries.some((e) => e.state === "warning") ? "warning" : "ok";
}

function ResultBadge({ e, state }: { e: ActivityEntry; state?: ActivityEntry["state"] }) {
  const s = state ?? e.state;
  const o = outcomeOf(s);
  const exit = e.code !== null && e.code !== 0 ? `Exit code ${e.code}` : undefined;
  return (
    <span className="ops-badges">
      {s === "running" && e.class === "session" && <span className="muted">open</span>}
      {s === "running" && e.class !== "session" && <Badge tone="info"><Icon icon={LoaderCircle} size={12} className="spin" /> running</Badge>}
      {s === "ok" && <span className="ops-ok">ok</span>}
      {(s === "warning" || s === "unfinished") && <Badge tone="warn" title={e.error ?? undefined}>{o.word}</Badge>}
      {s === "failed" && <Badge tone="crit" title={[exit, e.error].filter(Boolean).join(" · ") || undefined}>{o.word}</Badge>}
      {s === "cancelled" && <span className="muted">{o.word}</span>}
    </span>
  );
}

function startOfToday(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

export function ActivityLog({ params }: { params: Params }) {
  const cluster = useCluster();
  const source = useDataSource();
  const [entries, setEntries] = useState<ActivityEntry[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [path, setPath] = useState<string | null>(null);
  const [scope, setScope] = useWinState<Scope>("scope", "changes");
  const [failedOnly, setFailedOnly] = useWinState("failedOnly", false);
  const [node, setNode] = useWinState("node", "all");
  const [period, setPeriod] = useWinState<Period>("period", "week");
  const [text, setText] = useState("");
  const [op, setOp] = useState<string | null>(params.op ?? null);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(params.op ? [params.op] : []));
  const [selected, setSelected] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const latest = useRef(0);

  useEffect(() => {
    setOp(params.op ?? null);
    if (params.op) setExpanded((s) => new Set(s).add(params.op));
  }, [params.op, params.nav]);

  // The whole log once, then live entries as they're recorded (and a slow catch-up poll).
  useEffect(() => {
    if (!inTauri) {
      setLoaded(true);
      return;
    }
    let stop: (() => void) | undefined;
    setEntries([]);
    latest.current = 0;
    const merge = (incoming: ActivityEntry[]) => {
      if (!incoming.length) return;
      latest.current = Math.max(latest.current, ...incoming.map((e) => e.updatedMs || e.tsMs));
      setEntries((prev) => {
        const byId = new Map(prev.map((e) => [e.id, e]));
        for (const e of incoming) byId.set(e.id, e);
        return [...byId.values()].sort((a, b) => b.tsMs - a.tsMs);
      });
    };
    opsApi.activityLog().then((all) => {
      merge(all);
      setLoaded(true);
    }, (e) => {
      setLoaded(true);
      showError(e, { action: "Read the activity log" });
    });
    opsApi.activityPath().then(setPath, () => {});
    // Rows recorded under the other data source (stamped by the backend) aren't this log's.
    opsApi.onActivity((e) => (e.demo === undefined || e.demo === (source === "demo")) && merge([e])).then((u) => (stop = u), () => {});
    const poll = window.setInterval(() => {
      opsApi.activitySince(latest.current).then(merge, () => {});
      setNow(Date.now());
    }, 5000);
    return () => {
      stop?.();
      window.clearInterval(poll);
    };
    // The backend returns only the data source in use: switching it reads the log afresh.
  }, [source]);

  const needle = text.trim().toLowerCase();
  const since = period === "today" ? startOfToday() : period === "week" ? now - 7 * 86_400_000 : 0;
  // Group first, then filter whole operations, so a group keeps all of its commands.
  const admin = useAdmin();
  const all = useMemo(() => groupEntries(foldAdminSessions(entries, admin)), [entries, admin]);
  const groups = useMemo(() => all.filter((g) => {
    if (op) return g.key === op;
    if (scope === "changes" && !isChange(g)) return false;
    if (failedOnly && !g.entries.some((e) => e.state === "failed" || e.state === "unfinished")) return false;
    if (node !== "all" && !g.entries.some((e) => e.node === node || e.node.split(", ").includes(node))) return false;
    if (g.ts < since) return false;
    if (needle && !g.entries.some((e) => `${e.title} ${e.command} ${e.node} ${e.error ?? ""}`.toLowerCase().includes(needle))) return false;
    return true;
  }), [all, op, scope, failedOnly, node, since, needle]);
  const rows: Row[] = useMemo(() => groups.flatMap((g) => {
    const head: Row = { key: `g:${g.key}`, entry: g.head, group: g, child: false };
    if (g.entries.length <= 1 || !expanded.has(g.key) || isPoll(g)) return [head];
    return [head, ...g.entries.filter((e) => e !== g.head).map((e) => ({ key: `e:${e.id}`, entry: e, group: g, child: true }))];
  }), [groups, expanded]);
  const current = rows.find((r) => r.key === selected) ?? null;
  const nodeName = (id: string) => (id === "mac" ? "This Mac" : cluster?.nodes.find((n) => n.id === id)?.name ?? id);
  /** Where a group ran: every node of its commands, "cell1–cell3" when they're a run. */
  const where = (r: Row) => {
    const ids = r.child ? r.entry.node.split(", ") : r.group.entries.flatMap((e) => e.node.split(", "));
    return nodeRange(ids.filter(Boolean).map(nodeName));
  };

  const toggle = (key: string) => setExpanded((s) => {
    const next = new Set(s);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  });

  // Demo said once, in the footer, rather than on every row, and only in Demo.
  const demoShown = source === "demo";
  const columns: Column<Row>[] = [
    {
      key: "time", label: "Time", width: 118, priority: 1,
      title: (r) => absTime(r.entry.tsMs),
      render: (r) => <span className={r.child ? "muted" : undefined}>{relTime(r.entry.tsMs, now)}</span>,
    },
    { key: "node", label: "Node", width: 120, priority: 1, title: (r) => where(r), render: (r) => where(r), narrow: (r) => shortWhere(where(r)) },
    {
      key: "what", label: "What", minWidth: 240, priority: 1,
      title: (r) => (r.entry.title && r.entry.command ? `${r.entry.title}\n${r.entry.command}` : r.entry.title || r.entry.command),
      render: (r) => {
        const e = r.entry;
        const many = !r.child && r.group.entries.length > 1 && !isPoll(r.group);
        const summary = !r.child ? groupSummary(r.group) : null;
        const label = isShell(e) ? shellTitle(e, nodeName) : !r.child && e.title ? e.title : e.command || e.title;
        return (
          <span className={`ops-what${r.child ? " child" : ""}`}>
            {many ? (
              <button type="button" className="ops-disclosure" aria-expanded={expanded.has(r.group.key)} aria-label={expanded.has(r.group.key) ? "Hide its commands" : "Show its commands"}
                onClick={(ev: MouseEvent) => {
                  ev.stopPropagation();
                  toggle(r.group.key);
                }}>
                <Icon icon={expanded.has(r.group.key) ? ChevronDown : ChevronRight} size={12} />
              </button>
            ) : <span className="ops-disclosure-space" />}
            <span className={!r.child && e.title ? "ops-what-title" : "mono ops-what-cmd"}>{label}</span>
            {summary && <span className="muted ops-what-summary">· {summary}</span>}
            {e.class === "destructive" && <Badge tone="crit" title="Can destroy data or take a node down">destructive</Badge>}
            {e.escalated && <span className="ops-tag" title="Ran as root (sudo)">root</span>}
          </span>
        );
      },
    },
    {
      key: "result", label: "Result", width: 150, priority: 1,
      title: (r) => r.entry.error ?? "",
      render: (r) => <ResultBadge e={r.entry} state={r.child ? undefined : groupState(r.group)} />,
    },
    { key: "ms", label: "Took", width: 76, align: "right", priority: 2, render: (r) => (r.entry.ms ? <span className="num">{fmtMs(r.entry.ms)}</span> : "") },
    { key: "source", label: "Source", width: 92, priority: 2, render: (r) => <span className="muted">{SOURCE[r.entry.source] ?? r.entry.source}</span> },
  ];

  const exportAs = async (format: "csv" | "json") => {
    try {
      // The period shown, read from the files on disk (not only what's loaded here).
      const saved = await opsApi.activityExport(format, since || undefined, Date.now());
      toast({ kind: "success", title: "Activity log exported", body: saved.split("/").pop() ?? saved, actions: [{ label: "Show in Finder", run: () => void revealItemInDir(saved) }] });
    } catch (e) {
      showError(e, { action: "Export the activity log" });
    }
  };

  const opTitle = op ? entries.find((e) => (e.opId === op || e.id === op) && e.title)?.title ?? "one operation" : null;
  const task = current?.entry.opId ? getTask(current.entry.opId) : null;
  const periodWord = period === "today" ? "today's" : period === "week" ? "the last 7 days'" : "the whole";

  return (
    <div className="col fill ops-window ops-activity">
      <Toolbar label="Activity log">
        <Segmented<Scope> value={scope} onChange={setScope} label="Show" options={[
          { value: "changes", label: "Changes", title: "What changed a node or the Mac, Admin sessions and root shells" },
          { value: "everything", label: "Everything", title: "Also reads, shells, log views and SparkBox's own checks" },
        ]} />
        <HelpTip topic="ops.activity.views" />
        <Switch checked={failedOnly} onChange={setFailedOnly} label="Failed only" showLabel />
        <Segmented<Period> value={period} onChange={setPeriod} label="When" options={[
          { value: "today", label: "Today" },
          { value: "week", label: "7 days" },
          { value: "all", label: "All" },
        ]} />
        <Spacer />
        <select value={node} onChange={(e) => setNode(e.target.value)} aria-label="Node">
          <option value="all">All nodes</option>
          {(cluster?.nodes ?? []).map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}
          <option value="mac">This Mac</option>
        </select>
        <SearchField value={text} onChange={setText} placeholder="Filter commands" count={groups.length} />
        <button type="button" className="tool" title={`Save ${periodWord} log to Downloads`} aria-label="Export"
          onClick={(e) => showContextMenu(e, [
            { label: "Export as CSV…", run: () => exportAs("csv") },
            { label: "Export as JSON…", run: () => exportAs("json") },
          ])}>
          <Icon icon={Download} size={14} />
          <span className="tool-label">Export</span>
        </button>
        <ToolButton icon={FolderOpen} label="Show log file" priority={3} disabled={!path} title={path ?? undefined} onClick={() => path && revealItemInDir(path)} />
        <HelpTip topic="ops.activity.about" label="About the activity log" />
      </Toolbar>
      {op && (
        <Notice tone="info" action={{ label: "Show everything", onClick: () => setOp(null) }}>
          Showing the commands of “{opTitle}”.
        </Notice>
      )}
      {loaded && entries.length === 0 ? (
        <EmptyState icon={ScrollText} title="Nothing logged yet"
          body="Commands SparkBox runs on a node, and changes it makes on this Mac, show up here." />
      ) : (
        <div className={`ops-split${current ? "" : " no-detail"}`}>
          <div className="ops-split-list">
            <Table
              columns={columns}
              rows={rows}
              rowKey={(r) => r.key}
              selected={selected}
              onSelect={setSelected}
              onActivate={(r) => !r.child && r.group.entries.length > 1 && !isPoll(r.group) && toggle(r.group.key)}
              tableId="activity"
              ariaLabel="Activity log"
              rowClass={(r) => (r.entry.state === "failed" ? "row-crit" : r.entry.state === "unfinished" ? "row-warn" : r.child ? "ops-child-row" : undefined)}
              empty={!loaded ? "Reading the log…" : failedOnly ? "Nothing failed in this period." : text.trim() || node !== "all" ? "Nothing matches these filters." : emptyAtRest(scope, period)}
            />
            <div className="table-footer ops-activity-foot" role="status">
              <span>{groups.length} operation{groups.length === 1 ? "" : "s"}{expandedShown(rows)}</span>
              {demoShown && (
                <span className="table-footer-slot">
                  <span className="ops-demo-mark">Demo</span>
                  <HelpTip topic="ops.activity.demo" label="About Demo entries" />
                </span>
              )}
            </div>
          </div>
          {current && (
            <div className="ops-split-detail">
              <EntryDetail e={current.entry} nodeName={nodeName} where={where(current)} group={current.child ? null : current.group} taskOpen={!!task} onClose={() => setSelected(null)} />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** What an unfiltered, empty log says: at rest, nothing changing is the good news. */
export function emptyAtRest(scope: Scope, period: Period): string {
  const when = period === "today" ? "today" : period === "week" ? "in the last 7 days" : "yet";
  return scope === "changes" ? `Nothing changed on the cluster or this Mac ${when}.` : `Nothing ran ${when}.`;
}

/** The Node column's short form: "c1–c3", "gx10", "Mac". */
export function shortWhere(where: string): string {
  return where
    .replace(/This Mac/g, "Mac")
    .replace(/\bcell(\d+)/g, "c$1")
    .replace(/\bgx10-[a-z0-9]+/g, "gx10");
}

/** " · 6 commands shown" while operations are open. */
function expandedShown(rows: Row[]): string {
  const children = rows.filter((r) => r.child).length;
  return children ? ` · ${children} command${children === 1 ? "" : "s"} shown` : "";
}

function kindWord(e: ActivityEntry): string {
  switch (e.class) {
    case "destructive": return "Can destroy data or take a node down";
    case "change": return "Changes something";
    case "app": return "Change on this Mac";
    case "session": return e.kind === "terminal" ? (e.escalated ? "Root shell" : "Shell session") : "Admin mode";
    case "probe": return "SparkBox's own check";
    default: return "Only reads";
  }
}

function EntryDetail({ e, nodeName, where, group, taskOpen, onClose }: {
  e: ActivityEntry;
  nodeName: (id: string) => string;
  where: string;
  group: Group | null;
  taskOpen: boolean;
  onClose: () => void;
}) {
  const commands = group ? group.entries.filter((x) => x.kind !== "op") : [];
  if (!e) return <EmptyState icon={ScrollText} title="Nothing selected" body="Select a row to see the command, its output and how it ended." />;
  return (
    <div className="ops-detail" aria-label="Entry details">
      <div className="ops-detail-head">
        <h3 className="ops-detail-title">{e.title || e.command}</h3>
        <ResultBadge e={e} />
        <Spacer />
        {e.opId && taskOpen && (
          <button type="button" className="btn small" onClick={() => openTasksWindow(e.opId!)}>
            <Icon icon={ListChecks} size={12} /> Open task
          </button>
        )}
        <button type="button" className="tool icon-only" aria-label="Close details" title="Close details" onClick={onClose}>
          <Icon icon={X} size={14} />
        </button>
      </div>
      <dl className="ops-kv">
        <div><dt>When</dt><dd title={absTime(e.tsMs)}>{fmtDateTime(e.tsMs)}</dd></div>
        <div><dt>Where</dt><dd>{where || e.node.split(", ").map(nodeName).join(", ")}</dd></div>
        <div><dt>Who</dt><dd>{SOURCE[e.source] ?? e.source}{e.escalated ? " · as root (sudo)" : ""}</dd></div>
        {e.code !== null && <div><dt>Exit code</dt><dd className="num">{e.code}</dd></div>}
        {e.ms > 0 && <div><dt>Took</dt><dd className="num">{fmtMs(e.ms)}</dd></div>}
        <div><dt>Kind</dt><dd>{kindWord(e)}</dd></div>
        {e.opId && !e.opId.startsWith("admin-") && <div><dt>Operation</dt><dd className="mono">{e.opId}</dd></div>}
      </dl>
      {e.simulated && <Notice tone="info">Demo mode: nothing ran on a node. This is the command SparkBox would have run.</Notice>}
      {e.state === "unfinished" && <Notice tone="warn">{e.error ?? "Didn't finish (app quit or connection lost)"}. It may or may not have run on the node.</Notice>}
      {e.error && e.state !== "unfinished" && <Notice tone="error">{e.error}</Notice>}
      {e.command && (
        <>
          <div className="section-title">{e.kind === "app" || e.class === "session" ? "Details" : "Command"}</div>
          <CodeBlock text={e.command} label="Command" />
        </>
      )}
      {e.stdoutTail && (
        <>
          <div className="section-title">{e.kind === "op" ? "Outcome" : "Output"}</div>
          <CodeBlock text={e.stdoutTail} maxHeight={200} label="Output" />
        </>
      )}
      {e.stderrTail && (
        <>
          <div className="section-title">Errors</div>
          <CodeBlock text={e.stderrTail} maxHeight={160} label="Error output" />
        </>
      )}
      {commands.length > 0 && (
        <>
          <div className="section-title">{group?.session ? "Records" : "Commands"} · {commands.length}</div>
          <ul className="ops-cmd-list">
            {commands.map((c) => (
              <li key={c.id}>
                <ResultBadge e={c} />
                <span className="muted">{nodeName(c.node)}</span>
                <span className="mono">{group?.session ? `${c.title}${c.command ? ` · ${c.command}` : ""}` : c.command}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
