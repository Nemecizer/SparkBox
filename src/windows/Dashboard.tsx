import { useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type MouseEvent, type ReactNode } from "react";
import { ChartLine } from "lucide-react";
import { EntityLink } from "../components/EntityLink";
import { Icon } from "../components/Icon";
import { Table, type Column } from "../components/Table";
import { ChartGrid, TimeChart, useChartEvents } from "../components/TimeChart";
import { HelpTip, KnownMark, Meter, SectionTitle, Sparkline, StateGlyph, Tile, Tiles, Waiting, type GlyphState } from "../components/ui";
import { nodeMenu, showContextMenu } from "../lib/contextMenu";
import { useWatchState, watchLabel } from "../lib/agent";
import { endpointLabel, isServing, useEndpoints, type Endpoint } from "../lib/endpoints";
import { deploymentLabel, latestFailed, useDeployments } from "../lib/deployments";
import { useRecentExits } from "../lib/exits";
import { fmtBits, fmtBitsPair, fmtBytes, fmtRatePair, GiB, splitUnit } from "../lib/format";
import { alertShort, entityTarget, fmtSpan, statusTooltip, summarize, useHealth, DISPLAY_LABEL, displayStatus, type Alert, type HealthState, type Incident } from "../lib/health";
import {
  clusterFabric, clusterFabricCap, clusterFabricRx, clusterTotals, diskFree, diskPct, fabricRx, fabricTx, gpuError, gpuServing, isReporting,
  knownIssue, lanRx, lanTx, LIMITS, memPct, modelsInMemory, nodeAlerts, nodeNotes, nodeRange, nodeTone, pct, powerSubLabel, problemView,
  rootDisk, runningContainers, type NodeNote, type ProblemAlert,
} from "../lib/metrics";
import { nodeActionFor, offlineReason, powerLabel } from "../lib/nodeStatus";
import { alignTo, chartWindow, CLUSTER, getHistory, historyStart, memTotalOf, recent, useCluster } from "../lib/store";
import type { ClusterSnapshot, NodeSnapshot } from "../lib/types";
import { issueSummary, plural } from "../lib/vocabulary";
import { useWM, useWinState } from "../wm/WindowManager";
import { EndpointCard } from "./models/EndpointCard";
import { FailedDeploymentCard, IdleServersLine } from "./models/DeploymentSummary";
import { fmtShare, quietSinceMs } from "./Topology";
import "./monitoring.css";

const SPARK_POINTS = 90;
const CHART_SECONDS = 15 * 60;
const STRIP_LINES = 4;
/** Below this width the Dashboard shows the problems and the node table, charts on request. */
const COMPACT_W = 640;

/** How long ago, in one form for the band and the rows alike (fmtSpan): "44s", "2m", "1h 5m". */
function since(ms: number | null | undefined, now: number): string | null {
  return ms ? fmtSpan(now - ms) : null;
}

/** A row's trend moves less than this (percentage points): it would draw as an underline, so it isn't drawn. */
const SPARK_MIN_RANGE = 2;

/** A node row's CPU or GPU trend, or the same space left empty while it sits flat (an idle node). */
export function RowSpark({ values, color }: { values: (number | null)[]; color: string }) {
  let lo = Infinity;
  let hi = -Infinity;
  for (const v of values) if (v != null) [lo, hi] = [Math.min(lo, v), Math.max(hi, v)];
  if (!(hi - lo >= SPARK_MIN_RANGE)) return <span className="mon-spark-blank" aria-hidden />;
  return <Sparkline values={values} width={44} height={14} color={color} curve="monotone" />;
}

/** The node's glyph state: its own colour (nodeTone), planned while SparkBox restarts it, affected when a model it serves isn't Ready. */
function nodeGlyph(n: NodeSnapshot, health: HealthState | null, cluster: ClusterSnapshot, notes: NodeNote[]): GlyphState {
  if (n.status !== "online" && n.maintenance) return "planned";
  const tone = nodeTone(n.id, health, cluster);
  if (tone !== "ok") return tone;
  return notes.length ? "affected" : "ok";
}

/** Problems first: offline (0), critical (1), warnings (2), affected (3), then the healthy (4). */
export function nodeRank(n: NodeSnapshot, health: HealthState | null, cluster?: ClusterSnapshot | null): 0 | 1 | 2 | 3 | 4 {
  const tone = nodeTone(n.id, health, cluster ?? { nodes: [n] });
  return tone === "offline" ? 0 : tone === "crit" ? 1 : tone === "warn" ? 2 : n.status !== "online" ? 3 : 4;
}

/**
 * An alert's short state for the Status cell (§3.5): the rule's own short label ("GPU fault ·
 * Xid 79", at most 24 characters), else the message's first clause, never a sentence cut in
 * half. Several failed units are counted: "2 units failed".
 */
function shortOf(a: Alert, all: Alert[]): string {
  if (a.entity.kind === "service") {
    const units = all.filter((x) => x.entity.kind === "service").length;
    return `${plural(units, "unit")} failed`;
  }
  return alertShort(a);
}

export interface NodeState {
  glyph: GlyphState;
  /** The Status cell: a short state. */
  short: string;
  tone: "ok" | "warn" | "crit" | "neutral";
  /** Line 2 of a problem row: the message only. */
  sentence: string | null;
  /** Line 2's right end, muted: "for 1m · and 1 more". */
  suffix?: string | null;
  /** A model this node serves isn't Ready (neutral; the state word in its own tone): its own chip on line 2. */
  note: NodeNote | null;
}

/**
 * A node's state in words (§2.5): a short state for the Status cell ("Offline · 35 s",
 * "Root disk 85% full", "1 unit failed", "Healthy") and, for a problem, the full sentence for
 * the row's second line. A model that's down shows as a neutral note on every member node.
 */
export function nodeState(n: NodeSnapshot, health: HealthState | null, cluster: ClusterSnapshot, endpoints: Endpoint[], now: number): NodeState {
  const notes = nodeNotes(n.id, health, endpoints);
  const note = notes[0] ?? null;
  const glyph = nodeGlyph(n, health, cluster, notes);
  const noteText = notes.map((x) => x.text).join(" · ");
  if (n.status !== "online") {
    // Planned power work in its own word ("Off (planned)"); otherwise why it isn't reporting.
    const word = powerLabel(n) ?? (n.status === "connecting" ? "Connecting" : n.status === "error" ? "No SSH reply" : "Offline");
    const ago = since(quietSinceMs(n, health), now);
    const reason = offlineReason(n);
    const short = [word, ago, reason && reason !== word && !word.startsWith(reason) ? reason : null].filter(Boolean).join(" · ");
    const planned = n.maintenance ? (n.maintenance.reason === "in maintenance" ? `Planned · ${n.name} is in maintenance` : `Planned · SparkBox is ${n.maintenance.reason} ${n.name}`) : null;
    return {
      glyph, short, tone: glyph === "planned" ? "neutral" : n.status === "connecting" ? "warn" : "crit",
      sentence: planned ?? n.error ?? null, note,
    };
  }
  if (n.stale) return { glyph, short: `Not updating${n.lastSeenMs ? ` · ${since(n.lastSeenMs, now)}` : ""}`, tone: "warn", sentence: "No fresh numbers from the collector: what's shown is its last frame.", note };
  const own = nodeAlerts(n.id, health, now) as Alert[];
  if (own.length) {
    const a = own[0];
    const suffix = [a.sinceMs ? `for ${fmtSpan(now - a.sinceMs)}` : null, own.length > 1 ? `and ${own.length - 1} more` : null].filter(Boolean).join(" · ");
    // Line 2 is the message alone; the time and the count go to its right end, and a pipeline
    // member's model note is its own chip between them.
    return { glyph, short: shortOf(a, own), tone: a.severity === "crit" ? "crit" : "warn", sentence: a.message, suffix: suffix || null, note };
  }
  if (note) return { glyph, short: note.text, tone: "neutral", sentence: null, note };
  return { glyph, short: noteText || "Healthy", tone: "ok", sentence: null, note: null };
}

// ---- Problems box

/** "cell3 offline · for 40s": how long the condition has held (since first seen, not since it began to page). */
function alertLine(a: Alert, now: number): string {
  const going = a.sinceMs ?? a.firingSinceMs;
  return `${a.summary}${going ? ` · for ${fmtSpan(now - going)}` : ""}`;
}

/** "1:34": time left until a recovering alert clears. */
function clearsIn(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

type R3Alert = Alert & ProblemAlert & { planned?: string | null; recoversAtMs?: number | null };

/** The glyph for an incident: the loud offline circle when its lead is a node that isn't answering. */
function incidentGlyph(a: Alert, sev: "warn" | "crit"): GlyphState {
  if (sev === "crit" && a.entity.kind === "node" && /offline|unreachable/.test(a.rule)) return "offline";
  return sev;
}

/**
 * Loud only when something is wrong (§2.2): "1 critical · 1 warning" counting incidents, one
 * linked line per incident (its alerts in the tooltip), then the neutral lines that don't
 * count: pending ("cell3 offline · confirming…"), recovering and planned. Silenced and
 * acknowledged incidents are one quiet "2 known issues" line. All clear is a single quiet line.
 */
/** For an incident led by a node that stopped answering: the same clock as its row, time since its last reply. */
function offlineSince(a: Alert | null, cluster: ClusterSnapshot, health: HealthState): number | null {
  if (!a || a.entity.kind !== "node" || !/offline|stale|unreachable/.test(a.rule)) return null;
  const n = cluster.nodes.find((x) => x.id === a.entity.id);
  return n ? quietSinceMs(n, health) : null;
}

function ProblemsBox({ health, cluster, models, watched, onOpen, openTarget }: {
  health: HealthState | null;
  cluster: ClusterSnapshot;
  models: number;
  /** "watched 24/7" when the nodes' agents keep watching with SparkBox closed. */
  watched: string | null;
  onOpen: () => void;
  openTarget: (a: Alert) => void;
}) {
  const now = Date.now();
  if (!health) {
    return <div className="mon-strip neutral" role="status">Checking health…</div>;
  }
  type Line = { key: string; glyph: GlyphState; text: string; title: string; a: Alert | null; cls: string };
  const byId = new Map(health.alerts.map((a) => [a.id, a]));
  let crit: number;
  let warn: number;
  let known: number;
  let lines: Line[];
  let quiet: { pending: number; recovering: number; planned: number };
  if (health.incidents) {
    // The engine's incidents (one optic, one incident) and its counts, as the top bar has them.
    const sum = summarize(health);
    [crit, warn, known] = [sum.crit, sum.warn, sum.known];
    const lead = (i: Incident) => byId.get(i.children[0]) ?? null;
    const counted = health.incidents.filter((i) => i.counted && i.state === "firing" && (i.severity === "crit" || i.severity === "warn"));
    counted.sort((x, y) => (x.severity === y.severity ? x.sinceMs - y.sinceMs : x.severity === "crit" ? -1 : 1));
    const rec = health.incidents.filter((i) => i.state === "recovering");
    const plan = health.incidents.filter((i) => i.state === "planned");
    quiet = { pending: sum.pending.length, recovering: rec.length, planned: plan.length };
    lines = [
      ...counted.map((i) => {
        const a = lead(i);
        const sev = i.severity === "crit" ? "crit" : "warn";
        return {
          key: i.id,
          glyph: a ? incidentGlyph(a, sev) : sev,
          text: `${i.title} · for ${fmtSpan(now - (offlineSince(a, cluster, health) ?? i.sinceMs))}`,
          title: i.children.length > 1 ? `${i.children.length} related alerts:\n${i.children.map((c) => `• ${byId.get(c)?.summary ?? c}`).join("\n")}` : (a?.detail ?? i.title),
          a,
          cls: sev,
        };
      }),
      ...sum.pending.map((p) => ({ key: p.alertId, glyph: "affected" as const, text: `${p.title} · confirming…`, title: "Seen, not yet confirmed: it isn't counted until it holds.", a: byId.get(p.alertId) ?? null, cls: "neutral" })),
      ...rec.map((i) => ({ key: i.id, glyph: "recovering" as const, text: `${i.title} · Recovering · clears in ${clearsIn((i.recoversAtMs ?? now) - now)}`, title: "The condition has cleared; it resolves once it stays clear.", a: lead(i), cls: "neutral" })),
      ...plan.map((i) => ({ key: i.id, glyph: "planned" as const, text: `Planned · ${i.planned ?? i.title}`, title: "Work SparkBox started: never paged.", a: lead(i), cls: "neutral" })),
    ];
  } else {
    // An engine without incidents: grouped here, the same way.
    const v = problemView(health.alerts as R3Alert[], now);
    [crit, warn, known] = [v.crit.length, v.warn.length, v.known];
    quiet = { pending: v.pending.length, recovering: v.recovering.length, planned: v.planned.length };
    lines = [
      ...[...v.crit, ...v.warn].map((inc) => ({
        key: inc.key,
        glyph: incidentGlyph(inc.lead, inc.severity),
        text: alertLine(inc.lead, now),
        title: inc.members.length > 1 ? `${inc.members.length} related alerts:\n${inc.members.map((m) => `• ${m.summary}`).join("\n")}` : (inc.lead.detail ?? inc.lead.summary),
        a: inc.lead,
        cls: inc.severity,
      })),
      ...v.pending.map((a) => ({ key: a.id, glyph: "affected" as const, text: `${a.summary} · confirming…`, title: "Seen, not yet confirmed: it isn't counted until it holds.", a, cls: "neutral" })),
      ...v.recovering.map((a) => ({ key: a.id, glyph: "recovering" as const, text: `${a.summary} · Recovering · clears in ${clearsIn((a.recoversAtMs ?? now) - now)}`, title: "The condition has cleared; it resolves once it stays clear.", a, cls: "neutral" })),
      ...v.planned.map((a) => ({ key: a.id, glyph: "planned" as const, text: `Planned · ${a.planned}`, title: "Work SparkBox started: never paged.", a, cls: "neutral" })),
    ];
  }
  const counted = crit + warn;
  const quietN = quiet.pending + quiet.recovering + quiet.planned;
  const knownLink = known > 0 && (
    <button type="button" className="mon-known" onClick={onOpen} title="Silenced or acknowledged: not counted, and they don't color their node. Open Alerts">
      {plural(known, "known issue")}
    </button>
  );
  if (!counted && !quietN) {
    const nodes = cluster.nodes.length;
    const online = cluster.nodes.filter(isReporting).length;
    const parts = [
      "All clear",
      online === nodes ? plural(nodes, "node") : `${online} of ${nodes} nodes reporting`,
      models ? `${plural(models, "model")} serving` : "nothing serving",
      ...(watched ? [`watched ${watched}`] : []),
    ];
    return (
      <div className="mon-allclear">
        <button type="button" className="mon-strip ok" onClick={onOpen} title="Open Alerts">{parts.join(" · ")}</button>
        {knownLink}
      </div>
    );
  }
  const tone = crit ? "crit" : warn ? "warn" : "neutral";
  const head = counted
    ? issueSummary(crit, warn)
    : [quiet.pending && `${quiet.pending} confirming`, quiet.recovering && `${quiet.recovering} recovering`, quiet.planned && `${quiet.planned} planned`].filter(Boolean).join(" · ");
  const shown = lines.slice(0, STRIP_LINES);
  const more = lines.length - shown.length;
  return (
    <section className={`mon-issues state-${tone}`} aria-label="Problems">
      <div className="mon-issues-head state-head">
        <button type="button" className="mon-issues-open" onClick={onOpen} title="Open Alerts">
          <StateGlyph state={tone === "neutral" ? "affected" : tone} label={tone === "crit" ? "Critical" : tone === "warn" ? "Warning" : "Not counted"} />
          <b className="state-word">{head}</b>
        </button>
        {knownLink && <span className="mon-issues-known">{knownLink}</span>}
      </div>
      <ul className="mon-issues-list">
        {shown.map((l) => (
          <li key={l.key}>
            <button type="button" className={`mon-issue ${l.cls}`} onClick={() => (l.a ? openTarget(l.a) : onOpen())} title={l.title}>
              <StateGlyph state={l.glyph} />
              <span className="mon-issue-text">{l.text}</span>
            </button>
          </li>
        ))}
        {more > 0 && (
          <li>
            <button type="button" className="mon-issue more" onClick={onOpen}>and {more} more in Alerts</button>
          </li>
        )}
      </ul>
    </section>
  );
}

// ---- Serving

/** For a node SparkBox is restarting or has shut down (planned work): what it's doing, else null. */
function plannedWord(n: NodeSnapshot): string | null {
  if (!n.maintenance || n.status === "online") return null;
  if (n.maintenance.reason === "in maintenance") return "In maintenance";
  return n.powerState === "off" ? "Shut down" : n.powerState === "waking" ? "Waking" : "Restarting";
}

/** Free unified memory per node, the largest called out: where the next model fits. */
function CapacityCard({ nodes, onOpen }: { nodes: NodeSnapshot[]; onOpen: (n: NodeSnapshot) => void }) {
  const rows = nodes.map((n) => ({ n, free: isReporting(n) ? Math.max(0, n.mem.total - n.mem.used) : null }));
  // A node whose GPU can't be read is no place for the next model, however much memory is free.
  const best = rows.reduce<{ n: NodeSnapshot; free: number } | null>((b, r) => (r.free != null && !gpuError(r.n) && (!b || r.free > b.free) ? { n: r.n, free: r.free } : b), null);
  const top = Math.max(1, ...nodes.map((n) => n.mem.total));
  // The bars are memory, so they wear the memory colour where meters are coloured (Black); grey elsewhere.
  const bar = { "--meter-color": "var(--m-mem)" } as CSSProperties;
  return (
    <div className="mon-card mon-capacity state-ok" role="group" aria-label="Free memory by node">
      <div className="mon-card-head mon-capacity-head">
        <span className="mon-card-model">Capacity</span>
        <HelpTip topic="mon.dash.capacity" label="About capacity" />
        {best && <span className="mon-capacity-best" title={`${best.n.name} has the most free memory`}>Most free: {best.n.name}</span>}
      </div>
      <div className="mon-capacity-rows">
        {rows.map(({ n, free }) => (
          <button key={n.id} type="button" className={`mon-capacity-row${best?.n.id === n.id ? " best" : ""}`} style={bar} onClick={() => onOpen(n)} title={`${n.name}'s memory`}>
            <span className="mon-capacity-name">{n.name}</span>
            {free == null ? (
              <span className={`mon-capacity-off${plannedWord(n) ? " is-planned" : ""}`}>{plannedWord(n) ?? "Offline"}</span>
            ) : (
              <>
                <span className="mon-capacity-track"><span className="mon-capacity-fill" style={{ width: `${(free / top) * 100}%` }} /></span>
                <span className="mon-capacity-val num">{fmtBytes(free, { decimals: 1 })}</span>
              </>
            )}
          </button>
        ))}
      </div>
    </div>
  );
}

// ---- Node table

/** A cell that opens the node on one of its tabs (the row still selects on a click elsewhere). */
function CellLink({ children, onOpen, title }: { children: ReactNode; onOpen: () => void; title: string }) {
  return (
    <button
      type="button"
      className="mon-cell-link"
      title={title}
      tabIndex={-1}
      onClick={(e) => {
        e.stopPropagation();
        onOpen();
      }}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      {children}
    </button>
  );
}

const dash = <span className="muted">—</span>;
/** The metric columns an offline row's status spans (they'd all read "—"). */
const METRIC_KEYS = ["cpu", "gpu", "mem", "power", "disk", "fabric", "lan"];

function nodeColumns(
  health: HealthState | null,
  states: Map<string, NodeState>,
  openTab: (n: NodeSnapshot, tab: string) => void,
  openContainers: (n: NodeSnapshot) => void,
  openNote: (n: NodeNote) => void,
  exitedOn: (n: NodeSnapshot) => number,
  compact = false,
): Column<NodeSnapshot>[] {
  // In a small window only the node, its state and its GPU are essential; the rest steps aside.
  const p1 = compact ? 2 : 1;
  const live = (render: (n: NodeSnapshot) => ReactNode) => (n: NodeSnapshot) => (isReporting(n) ? render(n) : dash);
  const st = (n: NodeSnapshot) => states.get(n.id)!;
  return [
    {
      key: "node", label: "Node", priority: 1, fit: "content", sort: (n) => n.name, hideable: false,
      render: (n) => {
        const ds = displayStatus(n, health);
        return (
          <span className="cell-node" title={statusTooltip(n.name, ds, health?.nodes[n.id]?.issues ?? [], n.status === "online" ? null : n.error)}>
            <StateGlyph state={st(n).glyph} label={st(n).glyph === "ok" ? DISPLAY_LABEL.online : undefined} />
            <EntityLink kind="node" id={n.id} />
          </span>
        );
      },
    },
    {
      key: "status", label: "Status", priority: 1, kind: "message", minWidth: 110, sort: (n) => st(n).short,
      title: (n) => [st(n).short, st(n).sentence].filter(Boolean).join("\n"),
      // An offline node has no numbers: its state reads across the empty metric cells.
      absorb: (n) => (isReporting(n) ? null : METRIC_KEYS),
      render: (n) => {
        const s = st(n);
        if (s.note && s.tone === "neutral" && n.status === "online") {
          const [head] = s.note.text.split(` · ${s.note.state}`);
          return (
            <button type="button" className="mon-state neutral mon-note" tabIndex={-1} title={s.note.reason ?? s.note.text} onClick={(e) => { e.stopPropagation(); openNote(s.note!); }}>
              {head} · <span className={`tone-text-${s.note.tone}`}>{s.note.state}</span>
            </button>
          );
        }
        // The one thing that would get it reporting again, right where it says what's wrong.
        const action = n.status !== "online" ? nodeActionFor(n) : null;
        return (
          <span className="mon-state-line">
            <span className={`mon-state ${s.tone}`}>{s.short}</span>
            {action && (
              <button type="button" className="mon-link mon-state-action" tabIndex={-1} onClick={(e) => { e.stopPropagation(); action.run(); }}>
                {action.label}
              </button>
            )}
          </span>
        );
      },
    },
    {
      key: "cpu", label: "CPU", align: "right", fit: "content", priority: 2, sort: (n) => (isReporting(n) ? n.cpu.totalPct : -1),
      render: live((n) => (
        <CellLink onOpen={() => openTab(n, "cpu")} title={`${n.name}'s CPU`}>
          <span className="cell-spark">
            <RowSpark values={recent(n.id, "cpu", SPARK_POINTS)} color="var(--m-cpu)" />
            <span className="num">{n.cpu.totalPct.toFixed(0)}%</span>
          </span>
        </CellLink>
      )),
    },
    {
      key: "gpu", label: "GPU", align: "right", fit: "content", priority: 1, sort: (n) => (isReporting(n) ? n.gpu.utilPct : -1),
      render: live((n) => (
        <CellLink onOpen={() => openTab(n, "gpu")} title={`${n.name}'s GPU`}>
          <span className="cell-spark">
            <RowSpark values={recent(n.id, "gpu", SPARK_POINTS)} color="var(--m-gpu)" />
            {gpuError(n) ? <span className="num" title={gpuError(n) ?? undefined}>—</span> : <span className="num">{n.gpu.utilPct.toFixed(0)}%</span>}
          </span>
        </CellLink>
      )),
    },
    {
      key: "mem", label: "Memory", priority: p1, fit: "content", sort: (n) => (isReporting(n) ? memPct(n) : -1),
      title: (n) => (isReporting(n) ? knownIssue(n.id, "mem", health) ?? `${fmtBytes(n.mem.used, { decimals: 1 })} of ${fmtBytes(n.mem.total, { decimals: 1 })} unified memory in use` : ""),
      render: live((n) => {
        // A known issue (silenced or acknowledged) is drawn muted: a grey bar and a bell-slash.
        const known = knownIssue(n.id, "mem", health);
        return (
          <CellLink onOpen={() => openTab(n, "memory")} title={known ?? `${n.name}'s memory`}>
            <Meter pct={memPct(n)} warn={known ? undefined : LIMITS.memPct[0]} crit={known ? undefined : LIMITS.memPct[1]} color="var(--m-mem)" label={`${(n.mem.used / GiB).toFixed(1)}/${(n.mem.total / GiB).toFixed(1)} GiB`} ariaLabel={`Memory on ${n.name}`} />
            {known && <KnownMark title={known} />}
          </CellLink>
        );
      }),
    },
    {
      key: "power", label: "Power · temp", short: "W · °C", align: "right", fit: "content", priority: 2, sort: (n) => (isReporting(n) ? n.gpu.powerW : -1),
      render: live((n) => {
        if (gpuError(n)) return <span className="num" title={gpuError(n) ?? undefined}>—</span>;
        const known = knownIssue(n.id, "temp", health);
        return (
          <span className={`num${!known && n.gpu.tempC >= LIMITS.gpuTempC[0] ? " warn-text" : ""}`} title={known ?? undefined}>
            {n.gpu.powerW.toFixed(0)} W · {n.gpu.tempC.toFixed(0)} °C{known && <KnownMark title={known} />}
          </span>
        );
      }),
    },
    {
      key: "disk", label: "Disk", priority: 2, fit: "content", sort: (n) => diskPct(n),
      title: (n) => {
        const d = rootDisk(n);
        return d ? knownIssue(n.id, "disk", health) ?? `Root disk: ${fmtBytes(diskFree(d), { decimals: 1 })} free of ${fmtBytes(d.total, { decimals: 1 })}` : "";
      },
      render: (n) => {
        const d = rootDisk(n);
        if (!d || !isReporting(n)) return dash;
        const known = knownIssue(n.id, "disk", health);
        return (
          <CellLink onOpen={() => openTab(n, "disks")} title={known ?? `${n.name}'s disks`}>
            <Meter pct={diskPct(n)} warn={known ? undefined : LIMITS.diskPct[0]} crit={known ? undefined : LIMITS.diskPct[1]} color="var(--m-write)" label={`${diskPct(n).toFixed(0)}% · ${fmtBytes(diskFree(d), { decimals: 1 })} free`} ariaLabel={`Root disk on ${n.name}`} />
            {known && <KnownMark title={known} />}
          </CellLink>
        );
      },
    },
    {
      key: "fabric", label: "Fabric ↓/↑ Mb/s", short: "Fab ↓/↑", align: "right", fit: "content", priority: p1, sort: (n) => (isReporting(n) ? fabricRx(n) : -1),
      render: live((n) => (
        <CellLink onOpen={() => openTab(n, "network")} title={`Fabric (IP and RDMA): ↓ ${fmtBits(fabricRx(n))} · ↑ ${fmtBits(fabricTx(n))}`}>
          <span className="num">{fmtRatePair(fabricRx(n), fabricTx(n), { unit: "M" })}</span>
        </CellLink>
      )),
    },
    {
      key: "lan", label: "LAN ↓/↑ Mb/s", short: "LAN ↓/↑", align: "right", fit: "content", priority: 3, sort: (n) => (isReporting(n) ? lanRx(n) : -1),
      render: live((n) => (
        <CellLink onOpen={() => openTab(n, "network")} title={`10 GbE LAN: ↓ ${fmtBits(lanRx(n))} · ↑ ${fmtBits(lanTx(n))}`}>
          <span className="num">{fmtRatePair(lanRx(n), lanTx(n), { unit: "M" })}</span>
        </CellLink>
      )),
    },
    {
      key: "containers", label: "Containers", short: "Ctrs", align: "right", fit: "content", priority: 2, help: "mon.dash.nodes", sort: (n) => runningContainers(n).length,
      render: (n) => {
        if (!n.containers.length && n.status !== "online") return dash;
        const exited = exitedOn(n);
        return (
          <CellLink onOpen={() => openContainers(n)} title={`Containers on ${n.name}${exited ? ` · ${exited} exited with an error` : ""}`}>
            <span className="num">{runningContainers(n).length}/{n.containers.length}</span>
            {exited > 0 && <span className="warn-text">{"\u00a0· "}{exited} exited</span>}
          </CellLink>
        );
      },
    },
  ];
}

/** Cluster fabric traffic in 5-second buckets: the mean, and the lowest and highest second in each. */
function fiveSecond(ts: number[], values: (number | null)[]): { ts: number[]; mean: (number | null)[]; min: (number | null)[]; max: (number | null)[] } {
  const out = { ts: [] as number[], mean: [] as (number | null)[], min: [] as (number | null)[], max: [] as (number | null)[] };
  let bucket = NaN;
  let sum = 0;
  let count = 0;
  let lo = Infinity;
  let hi = -Infinity;
  const flush = () => {
    if (Number.isNaN(bucket)) return;
    out.ts.push(bucket);
    out.mean.push(count ? sum / count : null);
    out.min.push(count ? lo : null);
    out.max.push(count ? hi : null);
  };
  for (let i = 0; i < ts.length; i++) {
    const b = Math.floor(ts[i] / 5) * 5;
    if (b !== bucket) {
      flush();
      [bucket, sum, count, lo, hi] = [b, 0, 0, Infinity, -Infinity];
    }
    const v = values[i];
    if (v == null) continue;
    sum += v;
    count++;
    lo = Math.min(lo, v);
    hi = Math.max(hi, v);
  }
  flush();
  return out;
}

// ---- Window

/**
 * The cluster's GPU power from the GPUs that report: online, current and readable. An unreadable
 * GPU (an Xid, a stuck nvidia-smi) is left out, not counted as 0 W, and "(3 of 4)" says so.
 */
export function gpuPower(snap: Pick<ClusterSnapshot, "nodes">): { watts: number; counted: number; total: number; of: string } {
  const read = snap.nodes.filter((n) => isReporting(n) && !gpuError(n));
  const total = snap.nodes.length;
  return { watts: read.reduce((a, n) => a + n.gpu.powerW, 0), counted: read.length, total, of: read.length < total ? `(${read.length} of ${total})` : "" };
}

/** The GPU power tile's second line: the usual one while every GPU reports, else "avg 24 W (3 of 4)". */
export function gpuPowerSub(snap: Pick<ClusterSnapshot, "nodes">): string {
  const p = gpuPower(snap);
  if (!p.counted) return "No GPU reporting";
  if (!p.of) return powerSubLabel(clusterTotals(snap as ClusterSnapshot));
  return `avg ${Math.round(p.watts / p.counted)} W ${p.of}`;
}

/** Each second's GPU power summed over the GPUs that reported then (a gap only when none did). */
export function sumReporting(series: (number | null)[][], length: number): (number | null)[] {
  return Array.from({ length }, (_, j) => {
    const vals = series.map((s) => s[j]).filter((v): v is number => v != null);
    return vals.length ? vals.reduce((a, b) => a + b, 0) : null;
  });
}

export function Dashboard() {
  const cluster = useCluster();
  const health = useHealth();
  const endpoints = useEndpoints();
  const wm = useWM();
  const [selected, setSelected] = useWinState<string | null>("node", null);
  const [showCharts, setShowCharts] = useWinState("charts", false);
  const events = useChartEvents(CHART_SECONDS);
  const exits = useRecentExits();
  const watch = useWatchState();
  const rootRef = useRef<HTMLDivElement>(null);
  const [compact, setCompact] = useState(false);

  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => el.clientWidth && setCompact(el.clientWidth < COMPACT_W);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [cluster == null]);

  const deployments = useDeployments();
  const failed = latestFailed(deployments);
  const charts = useMemo(() => {
    if (!cluster) return null;
    const h = getHistory(CLUSTER, CHART_SECONDS);
    if (!h) return null;
    const ts = h.ts;
    const fab = fiveSecond(ts, h.fabricTx);
    const rdma = fiveSecond(ts, h.rdmaTx);
    return {
      ts,
      gpu: cluster.nodes.map((n) => ({ label: n.name, color: "--m-gpu", values: alignTo(ts, n.id, "gpu", CHART_SECONDS) })),
      mem: cluster.nodes.map((n) => ({
        label: n.name,
        color: "--m-mem",
        values: alignTo(ts, n.id, "memUsed", CHART_SECONDS).map((v) => (v == null || !memTotalOf(n) ? null : (v / memTotalOf(n)) * 100)),
      })),
      // Summed per node: one unreadable GPU blanks the cluster ring's total, not this one.
      power: sumReporting(cluster.nodes.map((n) => alignTo(ts, n.id, "power", CHART_SECONDS)), ts.length),
      fab,
      rdma,
      // When history is truly missing the window is the data's own (at least 2 minutes).
      window: chartWindow(ts, cluster.tsMs / 1000, CHART_SECONDS, historyStart()),
    };
    // A new snapshot means a new second of history.
  }, [cluster]);

  if (!cluster) return <Waiting />;

  const now = Date.now();
  const nodes = cluster.nodes;
  const totals = clusterTotals(cluster);
  const power = gpuPower(cluster);
  const serving = gpuServing(cluster, endpoints);
  const models = modelsInMemory(endpoints);
  const fabTx = clusterFabric(cluster);
  const fabRx = clusterFabricRx(cluster);
  const fabCap = clusterFabricCap(cluster);
  const offlineNames = nodes.filter((n) => !isReporting(n)).map((n) => n.name);
  const plannedNames = Object.fromEntries(nodes.flatMap((n) => (!isReporting(n) && plannedWord(n) ? [[n.name, plannedWord(n)!]] : [])));
  // Share of memory in use across the nodes reporting at each second (a node going away doesn't bend it).
  const memTrend = (charts?.ts ?? []).slice(-300).map((_, k, arr) => {
    const j = (charts?.ts.length ?? 0) - arr.length + k;
    const vals = (charts?.mem ?? []).map((s) => s.values[j]).filter((v): v is number => v != null);
    return vals.length ? vals.reduce((a, b) => a + b, 0) / vals.length : null;
  });
  const range = charts?.window.range ?? [cluster.tsMs / 1000 - CHART_SECONDS, cluster.tsMs / 1000];
  const startsAt = charts?.window.startsAt ?? null;
  // When this source's history began: the minutes before this run's first reading read "No data recorded".
  const recordedFrom = historyStart();
  const states = new Map(nodes.map((n) => [n.id, nodeState(n, health, cluster, endpoints, now)]));
  const rows = [...nodes].sort((a, b) => nodeRank(a, health, cluster) - nodeRank(b, health, cluster) || a.name.localeCompare(b.name, undefined, { numeric: true }));

  const openTab = (n: NodeSnapshot, tab: string) => wm.open("node", { id: n.id, tab });
  const openContainers = (n: NodeSnapshot) => wm.open("containers", { node: n.id });
  const openNote = (note: NodeNote) => wm.open(note.link.kind, note.link.params);
  const openAlert = (a: Alert) => {
    const t = entityTarget(a.entity);
    wm.open(t.kind, t.params);
  };
  // The same node menu as the sidebar, Topology and the status bar (C4).
  const rowMenu = (n: NodeSnapshot, e: MouseEvent) => showContextMenu(e, nodeMenu(n));
  const chartsShown = !compact || showCharts;
  // Serving at rest (C3): only endpoints that serve get a card; idle and gone servers are a line.
  const servingNow = endpoints.filter(isServing);
  const servingLine = servingNow.length
    ? servingNow.map((e) => `${endpointLabel(e)} · ${e.health === "ready" ? "Ready" : e.health}`).join(" · ")
    : ["Nothing is serving", failed ? `${deploymentLabel(failed)} failed ${new Date(failed.finishedMs ?? failed.startedMs).toLocaleDateString([], { month: "short", day: "numeric" })}` : null].filter(Boolean).join(" · ");

  return (
    <div className={`mon-dash${compact ? " compact" : ""}`} ref={rootRef}>
      <ProblemsBox health={health} cluster={cluster} models={models} watched={watchLabel(watch)} onOpen={() => wm.open("alerts")} openTarget={openAlert} />

      <section className="mon-section mon-dash-table" aria-label="Nodes">
        <Table
          tableId="dashboard-nodes-r3"
          ariaLabel="Nodes"
          columns={nodeColumns(health, states, openTab, openContainers, openNote, (n) => exits.filter((x) => x.node === n.id).length, compact)}
          rows={rows}
          rowKey={(n) => n.id}
          selection={selected ? new Set([selected]) : new Set()}
          onSelectionChange={(_, primary) => setSelected(primary)}
          selectionMode="single"
          onActivate={(n) => wm.open("node", { id: n.id })}
          onRowContextMenu={rowMenu}
          pinFirst={(n) => nodeRank(n, health, cluster) <= 1}
          rowClass={(n) => {
            const g = states.get(n.id)!.glyph;
            return g === "offline" ? "row-off" : g === "crit" ? "row-crit" : g === "warn" ? "row-warn" : undefined;
          }}
          rowDetail={(n) => {
            const s = states.get(n.id)!;
            if (!s.sentence || s.tone === "ok") return null;
            // The message, the model's note as its own chip, then "for 1m · and 1 more" on the right.
            return (
              <span className="mon-detail">
                <span className="mon-sentence">{s.sentence}</span>
                {s.note && s.tone !== "neutral" && (
                  <button type="button" className="mon-note-chip" tabIndex={-1} title={s.note.reason ?? s.note.text} onClick={(e) => { e.stopPropagation(); openNote(s.note!); }}>
                    {s.note.text.split(` · ${s.note.state}`)[0]} · <span className={`tone-text-${s.note.tone}`}>{s.note.state}</span>
                  </button>
                )}
                {s.suffix && <span className="mon-suffix">{s.suffix}</span>}
              </span>
            );
          }}
          footer={totals.reportingLabel ? <span>{totals.reportingLabel}</span> : undefined}
          itemNoun={["node", "nodes"]}
        />
      </section>

      {compact && !showCharts && (
        // The compact layout keeps one Serving line above "Show charts" (rm15).
        <button type="button" className="mon-serving-line" onClick={() => wm.open(failed && !servingNow.length ? "models" : "endpoints", failed && !servingNow.length ? { tab: "loaded" } : {})}>
          <span className="mon-serving-line-label">Serving</span>
          <span className="mon-serving-line-text">{servingLine}</span>
        </button>
      )}
      {compact && (
        <button type="button" className="mon-show-charts" aria-expanded={showCharts} onClick={() => setShowCharts((v) => !v)}>
          <Icon icon={ChartLine} size={14} />{showCharts ? "Hide charts" : "Show charts"}
        </button>
      )}

      {chartsShown && (
        <section className="mon-section" aria-label="Serving">
          <SectionTitle
            title="Serving"
            infoTopic="mon.dash.serving"
          />
          <div className="mon-serving">
            {servingNow.map((e) => <EndpointCard key={e.id} endpoint={e} variant="compact" />)}
            {!servingNow.length && (
              // Nothing serves: say so plainly, name the servers sitting idle, and show the last
              // deployment that failed this week (why it failed is one click away).
              <div className="mon-card mon-serving-empty state-ok">
                <span className="mon-serving-none">Nothing is serving.</span>
                <IdleServersLine endpoints={endpoints} />
                <button type="button" className="mon-link" onClick={() => wm.open("models")}>Load a model…</button>
              </div>
            )}
            {!servingNow.length && failed && <FailedDeploymentCard d={failed} compact />}
            <CapacityCard nodes={nodes} onOpen={(n) => openTab(n, "memory")} />
          </div>
        </section>
      )}

      {chartsShown && (
        <Tiles>
          <Tile
            label={<>GPUs serving <HelpTip topic="mon.dash.gpus-serving" /></>}
            value={`${serving.count}/${serving.total}`}
            sub={serving.count ? `${nodeRange(serving.nodes)}` : servingNow.length ? "None Ready" : "Nothing loaded"}
            trendKind="none"
            onClick={() => wm.open("endpoints")}
            title="Open Endpoints"
          />
          <Tile
            label="Unified memory"
            value={fmtBytes(totals.memUsed, { decimals: 1 })}
            unit={`of ${fmtBytes(totals.memTotal, { decimals: 1 })}`}
            sub={[`${pct(totals.memUsed, totals.memTotal).toFixed(0)}% used`, totals.reportingLabel].filter(Boolean).join(" · ")}
            trend={memTrend}
            trendDomain={[0, 100]}
            tone={nodes.some((n) => isReporting(n) && memPct(n) >= LIMITS.memPct[1]) ? "crit" : nodes.some((n) => isReporting(n) && memPct(n) >= LIMITS.memPct[0]) ? "warn" : undefined}
            color="var(--m-mem)"
          />
          <Tile
            label="GPU power"
            value={power.counted ? power.watts.toFixed(0) : "—"}
            unit={power.counted ? "W" : undefined}
            sub={gpuPowerSub(cluster)}
            trend={recent(CLUSTER, "power", 300)}
            trendDomain={totals.powerLimitW != null ? [0, totals.powerLimitW] : undefined}
            color="var(--m-power)"
          />
          <Tile
            label={<>Fabric ↓ / ↑ <HelpTip topic="mon.dash.fabric" /></>}
            value={splitUnit(fmtBitsPair(fabRx, fabTx))[0]}
            unit={splitUnit(fmtBitsPair(fabRx, fabTx))[1]}
            sub={`${fmtShare(fabTx / fabCap)} of ${nodes.length}×200G`}
            trend={recent(CLUSTER, "fabricTx", 300)}
            color="var(--m-tx)"
            onClick={() => wm.open("topology")}
            title="Open Topology"
          />
        </Tiles>
      )}

      {chartsShown && (
        <ChartGrid>
          <div className="mon-dash-charts">
            <TimeChart
              title="GPU utilization"
              subtitle="Last 15 minutes"
              mode="multiples"
              ts={charts?.ts ?? []}
              range={range}
              relative
              startsAt={startsAt}
              recordedFrom={recordedFrom}
              yMax={100}
              fmt={(v) => `${v.toFixed(0)}%`}
              series={charts?.gpu ?? []}
              offline={offlineNames}
              planned={plannedNames}
            />
            <TimeChart
              title="Unified memory"
              subtitle="Share of each node's memory"
              mode="multiples"
              ts={charts?.ts ?? []}
              range={range}
              relative
              startsAt={startsAt}
              recordedFrom={recordedFrom}
              yMax={100}
              thresholds={[{ value: LIMITS.memPct[0], label: `${LIMITS.memPct[0]}%`, tone: "warn" }]}
              fmt={(v) => `${v.toFixed(0)}%`}
              series={charts?.mem ?? []}
              offline={offlineNames}
              planned={plannedNames}
            />
            <TimeChart
              title="Fabric throughput"
              subtitle={<>Sent by every node · 5 s mean, min–max band <HelpTip topic="mon.dash.chart" label="About this chart" /></>}
              ts={charts?.fab.ts ?? []}
              range={range}
              relative
              robust="auto"
              events={events}
              startsAt={startsAt}
              recordedFrom={recordedFrom}
              fmt={fmtBits}
              series={[
                { label: "Total", color: "--m-tx", values: charts?.fab.mean ?? [], band: charts ? { min: charts.fab.min, max: charts.fab.max } : undefined },
                // RDMA is most of Total and runs along it: dashed, so both lines show where they coincide.
                { label: "RDMA", color: "--m-rdma", values: charts?.rdma.mean ?? [], noFill: true, dash: true },
              ]}
              height={160}
            />
            <TimeChart
              title="GPU power"
              subtitle={power.of ? `GPUs that report ${power.of}` : totals.powerLimitW != null ? `All nodes · limit ${totals.powerLimitW.toFixed(0)} W` : totals.reportingLabel || "All nodes"}
              noun="GPU"
              ts={charts?.ts ?? []}
              range={range}
              relative
              robust="auto"
              events={events}
              startsAt={startsAt}
              recordedFrom={recordedFrom}
              fmt={(v) => `${v.toFixed(0)} W`}
              series={[{ label: "Total", color: "--m-power", values: charts?.power ?? [] }]}
              height={160}
            />
          </div>
        </ChartGrid>
      )}
    </div>
  );
}
