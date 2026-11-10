import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Bell, BellOff, BellRing, Check, ChevronDown, ChevronRight, CircleCheck, Cpu, ExternalLink, History, Send, Settings2 } from "lucide-react";
import { Table, type Column } from "../components/Table";
import {
  AdvisoryGlyph, Badge, EmptyState, HelpTip, InfoTip, MiddleEllipsis, Notice, SearchField, Segmented, Spacer, StateGlyph, StatusDot, Tabs, Toolbar, ToolButton, ToolSeparator,
  type DotStatus, type GlyphState,
} from "../components/ui";
import { showContextMenu, type MenuItem } from "../lib/contextMenu";
import { ADMIN_REFUSED, unlockedFor, useAdminState, withAdmin } from "../lib/admin";
import { inTauri } from "../lib/api";
import { fmtDateTime } from "../lib/format";
import {
  alertScope, alertShownAs, applyHealth, DURATION_LABEL, entityKey, entityTarget, fmtAgo, fmtClock, fmtFor, healthApi, silenceLine, useHealth,
  type Alert, type AlertEvent, type AlertEntity, type HealthState, type Incident, type Severity, type Silence, type SilenceDuration,
} from "../lib/health";
import { label as silenceLabel } from "../lib/healthCommands";
import { nodeActionFor } from "../lib/nodeStatus";
import { deliveryLine, KIND_LABEL, notifyApi, outcomeLabel, type Delivery, type Outcome } from "../lib/notifyApi";
import { useCluster } from "../lib/store";
import { toast } from "../lib/toasts";
import { useWindowId, useWM, useWinState, windowElement, type Params } from "../wm/WindowManager";
import "./monitoring.css";
import "./alerts.css";

const SEVERITY_WORD: Record<Severity, string> = { info: "Info", warn: "Warning", crit: "Critical" };

/** Info has no colour of its own: a neutral dot and the word. */
const sevDot = (s: Severity): DotStatus => (s === "info" ? "unconfigured" : s);

const EVENT_WORD: Record<AlertEvent["kind"], string> = {
  raised: "Raised",
  escalated: "Became critical",
  eased: "Eased to warning",
  resolved: "Cleared",
  acked: "Acknowledged",
  muted: "Muted",
  unmuted: "Unmuted",
  silenced: "Silenced",
  unsilenced: "Unsilenced",
  silenceEnded: "Silence ended",
};

function EntityButton({ entity }: { entity: AlertEntity }) {
  const wm = useWM();
  const t = entityTarget(entity);
  return (
    <button
      type="button"
      className="mon-link"
      onClick={(e) => {
        e.stopPropagation();
        wm.open(t.kind, t.params);
      }}
      title={`Open ${entity.label}`}
    >
      {entity.label}
    </button>
  );
}

/** Ticks once a second, so "Firing 3 min" and "clears in 1:34" stay current between health updates. */
function useNow(every = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), every);
    return () => window.clearInterval(t);
  }, [every]);
  return now;
}

/** Polls `load` while the tab is open; returns the latest result or the error. */
function usePolled<T>(load: () => Promise<T>, everyMs: number, key: unknown): { data: T | null; error: string | null } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!inTauri) return;
    let alive = true;
    const run = () => load().then((d) => alive && (setData(d), setError(null)), (e) => alive && setError(String(e)));
    run();
    const t = window.setInterval(run, everyMs);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
    // `load` is a fresh closure every render; `key` says when it really changed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [everyMs, key]);
  return { data, error };
}

/** "Paging resumes for nvidia-dgx-telemetry on cell1." after a silence ends. */
function resumesFor(labels: string[]): string {
  if (!labels.length) return "Anything still wrong counts and pages again.";
  return `Paging resumes for ${labels.length === 1 ? labels[0] : `${labels.slice(0, -1).join(", ")} and ${labels[labels.length - 1]}`}.`;
}

/** The message of a gate or engine error, whatever shape it came in. */
function errText(e: unknown): string {
  const m = (e as { message?: string })?.message ?? String(e);
  try {
    const j = JSON.parse(m) as { message?: string };
    return j.message ?? m;
  } catch {
    return m;
  }
}

type TabId = "active" | "silenced" | "history" | "events" | "deliveries";
const TABS: TabId[] = ["active", "silenced", "history", "events", "deliveries"];

export function AlertsWindow({ params }: { params: Params }) {
  const initial = TABS.includes(params.tab as TabId) ? (params.tab as TabId) : "active";
  const [tab, setTab] = useWinState<string>("tab", initial);
  // A node window's "Show in Alerts" opens Events for that node only.
  const [eventsNode, setEventsNode] = useWinState<string | null>("eventsNode", params.node ?? null);
  useEffect(() => {
    if (params.tab && TABS.includes(params.tab as TabId)) setTab(params.tab);
    if (params.tab === "events") setEventsNode(params.node ?? null);
  }, [params.tab, params.node, setTab, setEventsNode]);
  const health = useHealth();
  const counted = (health?.incidents ?? []).filter((i) => i.counted).length;
  const silences = health?.silences?.length ?? 0;
  const [sheet, setSheet] = useState<SheetRequest | null>(null);
  return (
    <div className="col fill mon-alerts">
      <Tabs
        label="Alerts"
        value={tab}
        onChange={setTab}
        tabs={[
          { id: "active", label: "Active", badge: counted || undefined },
          { id: "silenced", label: silences ? `Silenced (${silences})` : "Silenced" },
          { id: "history", label: "History" },
          { id: "events", label: "Events" },
          { id: "deliveries", label: "Deliveries" },
        ]}
      >
        {tab === "active" && <ActiveIncidents health={health} onSilence={setSheet} onShowHistory={() => setTab("history")} onShowSilenced={() => setTab("silenced")} />}
        {tab === "silenced" && <Silenced health={health} />}
        {tab === "history" && <AlertHistory refreshKey={health?.lastAlertMs ?? 0} demo={health?.demo ?? true} />}
        {tab === "events" && <KernelEvents node={eventsNode} onShowAll={() => setEventsNode(null)} refreshKey={health?.tsMs ? Math.floor(health.tsMs / 5000) : 0} />}
        {tab === "deliveries" && <Deliveries />}
      </Tabs>
      {sheet && <SilenceSheet req={sheet} demo={health?.demo ?? true} onClose={() => setSheet(null)} />}
    </div>
  );
}

// ---------------------------------------------------------------- Active

/** What a silence would cover: an incident, or one alert in it. */
interface SheetRequest {
  matcher: Silence["matcher"];
  label: string;
  duration: SilenceDuration;
  /** The same rule on every node instead ("every node-offline alert"): Mac scope and a reason. */
  wide?: { matcher: Silence["matcher"]; label: string };
}

/** One alert (its rule and entity) or one incident: what node scope may silence (health/silence.rs narrow). */
const narrow = (m: Silence["matcher"]) => !!m.incident || (!!m.rule && !!m.entity);

/**
 * health/silence.rs needs_mac: one alert or one incident for an hour or a day is node scope; a rule
 * or an entity everywhere, until it clears, and a week or longer need Mac scope (with Live data) and
 * a reason.
 */
export const needsMac = (m: Silence["matcher"], d: SilenceDuration) => !narrow(m) || d === "1w" || d === "never" || d === "resolved";

/** What the Silence sheet says a silence needs; null for one alert or incident for an hour or a day. */
export function silenceNote(m: Silence["matcher"], label: string, d: SilenceDuration, demo: boolean): string | null {
  const needs = demo ? "needs a reason" : "needs Mac scope (Touch ID) and a reason";
  const clears = d === "resolved" ? ", until it clears (at most 7 days)" : "";
  if (!narrow(m)) return `${label[0].toUpperCase()}${label.slice(1)}, on every node${clears}: a silence that wide ${needs}.`;
  if (d === "resolved") return `Until it clears (at most 7 days): it ends when the alert clears, or after 7 days if it doesn't. It ${needs}.`;
  return needsMac(m, d) ? `A week or longer ${needs}.` : null;
}

/** "Flapping · raised 3 times in 10 min" (health/mod.rs FlapInfo::words). */
const flapText = (f: NonNullable<Alert["flapping"]>, now: number) => `Flapping · raised ${f.count} times in ${Math.max(1, Math.floor((now - f.sinceMs) / 60_000))} min`;

type Group = "firing" | "recovering" | "planned" | "acknowledged" | "info";
const GROUPS: { id: Group; title: string; help: string }[] = [
  { id: "firing", title: "Firing", help: "mon.alerts.firing" },
  { id: "recovering", title: "Recovering", help: "mon.alerts.recovering" },
  { id: "planned", title: "Planned", help: "mon.alerts.planned" },
  { id: "acknowledged", title: "Acknowledged", help: "mon.alerts.acknowledged" },
  { id: "info", title: "Information", help: "mon.alerts.information" },
];

interface Row {
  key: string;
  incident: Incident;
  /** The alert the row shows: the incident's root on its own row, another member on a child row. */
  alert: Alert | null;
  /** A child row: one of the incident's other members (the root is never repeated under itself). */
  child: boolean;
  /** How many members besides the root (the row gets a disclosure when there are any). */
  others: number;
}

/**
 * An identifier cut in the middle, keeping where it is: "qsfp56-dd-1-2 (cell2) on CRS804 DDQ"
 * narrows to "qsfp56-dd-1-2 (c… CRS804 DDQ", never to "qsfp56-…".
 */
function Place({ text, className }: { text: string; className?: string }) {
  const at = text.lastIndexOf(" on ");
  // Without a place ("cell2 half 2"), an ordinary end cut; with one, the place always shows.
  return <MiddleEllipsis text={text} keepEnd={at > 0 ? text.length - at - 4 : 0} className={`al-place${className ? ` ${className}` : ""}`} />;
}

/** The glyph for a shown state (§2.3). */
function glyphOf(shown: string, severity: Severity, offline: boolean): GlyphState {
  if (shown === "recovering") return "recovering";
  if (shown === "planned") return "planned";
  if (shown === "silenced" || shown === "acknowledged") return "silenced";
  if (severity === "info" || shown === "pending") return "ok";
  if (offline) return "offline";
  return severity === "crit" ? "crit" : "warn";
}

const SHOWN_WORD: Record<string, string> = {
  firing: "Firing", recovering: "Recovering", planned: "Planned", silenced: "Silenced", acknowledged: "Acknowledged", pending: "Confirming", info: "Information",
};

/** The state pill: "Firing", "Flapping · raised 3 times in 10 min", "Recovering · clears in 1:34", "Planned · cell3 restarting (SparkBox, 10:42)". */
function statePill(shown: string, flapping: Alert["flapping"], recoversAtMs: number | null, planned: string | null, now: number): { text: string; tone: "warn" | "neutral" } {
  if (shown === "firing" && flapping) return { text: flapText(flapping, now), tone: "warn" };
  switch (shown) {
    case "recovering": return { text: `Recovering · clears in ${fmtClock((recoversAtMs ?? now) - now)}`, tone: "neutral" };
    case "planned": return { text: `Planned · ${planned ?? "work SparkBox started"}`, tone: "neutral" };
    case "pending": return { text: "Confirming…", tone: "neutral" };
    default: return { text: SHOWN_WORD[shown] ?? "Firing", tone: "neutral" };
  }
}

/**
 * What's wrong, next to Affects: the title without the "cell2 half 2: " Affects already says,
 * the headline alert's own sentence when the title is just its summary ("Root disk 85% full
 * (143 GiB free)" rather than "gx10-0000 disk 85%"), or the incident's own title ("cell2 lost
 * the fabric").
 */
function what(i: Incident, head: Alert | undefined): string {
  const prefix = `${i.affects}: `;
  if (i.title.startsWith(prefix)) return i.title.slice(prefix.length);
  if (head && i.title === head.summary) return head.message;
  return i.title;
}

function groupOf(i: Incident): Group | null {
  switch (i.state) {
    case "firing": return "firing";
    case "recovering": return "recovering";
    case "planned": return "planned";
    case "acknowledged": return "acknowledged";
    case "info": return "info";
    default: return null; // silenced ones are in the Silenced tab
  }
}

function ActiveIncidents({ health, onSilence, onShowHistory, onShowSilenced }: {
  health: HealthState | null;
  onSilence: (r: SheetRequest) => void;
  onShowHistory: () => void;
  onShowSilenced: () => void;
}) {
  const wm = useWM();
  const winId = useWindowId();
  const cluster = useCluster();
  const now = useNow();
  const admin = useAdminState();
  // Acknowledge, Unsilence and the short silences are node scope; a week or longer asks again (alertScope).
  const scope = alertScope("alert_ack", health?.demo !== false);
  const locked = !unlockedFor(scope, admin);
  const [selection, setSelection] = useState<Set<string>>(new Set());
  const [open, setOpen] = useWinState<string[]>("expanded", []);
  const incidents = useMemo(() => health?.incidents ?? [], [health?.incidents]);
  const byId = useMemo(() => new Map((health?.alerts ?? []).map((a) => [a.id, a])), [health?.alerts]);
  const known = health?.summary.known?.filter((k) => k.why === "silenced").length ?? 0;

  const rowsOf = (g: Group): Row[] => {
    const out: Row[] = [];
    for (const i of incidents.filter((x) => groupOf(x) === g)) {
      const others = i.children.filter((id) => id !== i.headline && byId.has(id));
      out.push({ key: i.id, incident: i, alert: byId.get(i.headline) ?? null, child: false, others: others.length });
      if (others.length && open.includes(i.id)) {
        for (const id of others) out.push({ key: `a:${id}`, incident: i, alert: byId.get(id) ?? null, child: true, others: 0 });
      }
    }
    return out;
  };
  const allRows = GROUPS.flatMap((g) => rowsOf(g.id));
  const picked = allRows.filter((r) => selection.has(r.key));
  const pickedOne = picked.length === 1 ? picked[0] : null;

  /** The incident or alert ids a toolbar action applies to: a root row stands for its incident. */
  const targets = () => picked.map((r) => (r.child && r.alert ? r.alert.id : r.incident.id));

  const ack = async (ids = targets()) => {
    if (!ids.length) return;
    try {
      let last: HealthState | null = null;
      await withAdmin(async () => {
        for (const id of ids) last = await healthApi.ack(id);
      }, { scope, action: "alert_ack" });
      if (last) applyHealth(last);
      const n = picked.length === 1 ? (picked[0].child ? picked[0].alert?.summary : picked[0].incident.title) ?? picked[0].incident.title : `${ids.length} alerts`;
      toast({ kind: "success", title: `Acknowledged ${n}`, body: "It's a known issue now: not counted, no repeats, until it gets worse.", key: "alerts:ack" });
    } catch (e) {
      if (e === ADMIN_REFUSED) return;
      toast({ kind: "error", title: "Couldn't acknowledge it", body: errText(e), key: "alerts:ack" });
    }
  };

  const silenceFor = (r: Row, duration: SilenceDuration): SheetRequest => {
    // Rule-wide is offered in the sheet, with what it needs.
    const head = r.child ? r.alert : (r.alert ?? byId.get(r.incident.headline) ?? null);
    const wide = head ? { matcher: { rule: head.rule }, label: `every ${head.rule} alert` } : undefined;
    if (r.child && r.alert) {
      return { matcher: { rule: r.alert.rule, entity: entityKey(r.alert) }, label: r.alert.entity.label, duration, wide };
    }
    return { matcher: { incident: r.incident.id }, label: silenceLabel(r.incident), duration, wide };
  };

  const silenceMenu = (e: ReactMouseEvent, r: Row) => {
    const items: MenuItem[] = (Object.keys(DURATION_LABEL) as SilenceDuration[]).map((d) => ({
      label: `${DURATION_LABEL[d]}${needsMac(silenceFor(r, d).matcher, d) ? "…" : ""}`,
      run: () => onSilence(silenceFor(r, d)),
    }));
    showContextMenu(e, items);
  };

  const unsilence = async (ids: string[]) => {
    const labels = ids.map((id) => health?.silences?.find((x) => x.id === id)?.label).filter((l): l is string => !!l);
    try {
      let last: HealthState | null = null;
      await withAdmin(async () => {
        for (const id of ids) last = await healthApi.unsilence(id);
      }, { scope, action: "alert_unsilence" });
      if (last) applyHealth(last);
      toast({ kind: "success", title: ids.length === 1 ? "Silence ended" : `${ids.length} silences ended`, body: resumesFor(labels), key: "alerts:unsilence" });
    } catch (e) {
      if (e === ADMIN_REFUSED) return;
      toast({ kind: "error", title: "Couldn't end the silence", body: errText(e), key: "alerts:unsilence" });
    }
  };
  const silenceIds = [...new Set(picked.flatMap((r) => (r.child && r.alert ? [r.alert] : r.incident.children.map((id) => byId.get(id)).filter((a): a is Alert => !!a))).map((a) => a.silenceId).filter((s): s is string => !!s))];

  const openRow = (r: Row) => {
    const a = r.alert ?? byId.get(r.incident.headline);
    if (!a) return;
    const t = entityTarget(a.entity);
    wm.open(t.kind, t.params);
  };

  const toggle = (id: string) => setOpen(open.includes(id) ? open.filter((x) => x !== id) : [...open, id]);

  /** ArrowRight shows an incident's other alerts, ArrowLeft hides them (from a child too); Enter opens. */
  const onKeys = (e: ReactKeyboardEvent) => {
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    const r = pickedOne;
    if (!r) return;
    const expanded = open.includes(r.incident.id);
    if (e.key === "ArrowRight" && !r.child && r.others && !expanded) setOpen([...open, r.incident.id]);
    else if (e.key === "ArrowLeft" && (r.child || expanded)) {
      setOpen(open.filter((x) => x !== r.incident.id));
      if (r.child) setSelection(new Set([r.incident.id]));
    } else return;
    e.preventDefault();
    e.stopPropagation();
  };

  const columns: Column<Row>[] = [
    {
      key: "state", label: "", width: 46, priority: 1, hideable: false,
      sort: (r) => (r.incident.severity === "crit" ? 3 : r.incident.severity === "warn" ? 2 : 1),
      render: (r) => {
        const a = r.alert;
        const shown = r.child && a ? alertShownAs(a) : r.incident.state;
        const sev = r.child && a ? a.severity : r.incident.severity;
        const glyph = glyphOf(shown, sev, a?.rule === "node-offline");
        const word = `${SEVERITY_WORD[sev]} · ${SHOWN_WORD[shown] ?? "Firing"}`;
        const expanded = open.includes(r.incident.id);
        return (
          <span className={`al-state${r.child ? " child" : ""}`}>
            {r.others ? (
              <button type="button" className="al-disclose" aria-expanded={expanded} aria-label={`${expanded ? "Hide" : "Show"} its ${r.others === 1 ? "other alert" : `${r.others} other alerts`}`}
                title={`${expanded ? "Hide" : "Show"} ${r.others === 1 ? "the other alert" : `the other ${r.others} alerts`} (→ ←)`}
                onClick={(e) => {
                  e.stopPropagation();
                  toggle(r.incident.id);
                }}>
                {expanded ? <ChevronDown size={12} aria-hidden /> : <ChevronRight size={12} aria-hidden />}
              </button>
            ) : (
              <span className="al-disclose-space" aria-hidden />
            )}
            <span className="al-glyph-box"><StateGlyph state={glyph} label={word} /></span>
          </span>
        );
      },
    },
    {
      key: "affects", label: "Affects", width: 200, minWidth: 150, kind: "message", priority: 1,
      sort: (r) => (r.child ? r.alert?.entity.label : r.incident.affects) ?? "",
      title: (r) => (r.child ? r.alert?.entity.label : r.incident.affects) ?? "",
      render: (r) => (r.child && r.alert ? <Place text={r.alert.entity.label} className="al-child-affects" /> : <Place text={r.incident.affects} className="al-affects" />),
    },
    {
      key: "what", label: "What", kind: "message", priority: 1,
      sort: (r) => (r.child ? r.alert?.message : r.incident.title) ?? "",
      title: (r) => (r.child && r.alert ? [r.alert.message, r.alert.detail].filter(Boolean).join("\n") : `${r.incident.title}${r.others ? `\nand ${r.others === 1 ? "1 other alert" : `${r.others} other alerts`}` : ""}`),
      render: (r) => {
        if (r.child) return r.alert?.message ?? "";
        const i = r.incident;
        return (
          <span className="al-what">
            <span className="al-what-text">{what(i, r.alert ?? undefined)}</span>
            {r.others > 0 && <span className="al-count" title={`and ${r.others === 1 ? "1 other alert" : `${r.others} other alerts`} in this incident`}>+{r.others}</span>}
          </span>
        );
      },
    },
    {
      key: "pill", label: "State", width: 96, priority: 2,
      sort: (r) => (r.child && r.alert ? alertShownAs(r.alert) : r.incident.state),
      title: (r) => {
        const a = r.child ? r.alert : null;
        return statePill(a ? alertShownAs(a) : r.incident.state, (a ?? r.incident).flapping ?? null, a ? (a.recoversAtMs ?? null) : r.incident.recoversAtMs, a ? (a.planned ?? null) : r.incident.planned, now).text;
      },
      render: (r) => {
        const a = r.child ? r.alert : null;
        const p = statePill(a ? alertShownAs(a) : r.incident.state, (a ?? r.incident).flapping ?? null, a ? (a.recoversAtMs ?? null) : r.incident.recoversAtMs, a ? (a.planned ?? null) : r.incident.planned, now);
        return <Badge tone={p.tone}>{p.text}</Badge>;
      },
    },
    {
      key: "since", label: "Since", width: 96, align: "right", priority: 1,
      sort: (r) => (r.child && r.alert ? (r.alert.firingSinceMs ?? r.alert.sinceMs) : r.incident.sinceMs),
      title: (r) => `Since ${fmtDateTime(r.child && r.alert ? (r.alert.firingSinceMs ?? r.alert.sinceMs) : r.incident.sinceMs, now)}`,
      render: (r) => <span className="al-since">{fmtFor(now - (r.child && r.alert ? (r.alert.firingSinceMs ?? r.alert.sinceMs) : r.incident.sinceMs))}</span>,
    },
  ];

  const menu = (r: Row, e: ReactMouseEvent) => {
    // A node that needs the owner (an untrusted host key, a refused password): the one action that helps.
    const a = r.alert ?? byId.get(r.incident.headline);
    const node = a?.rule === "node-needs-you" ? cluster?.nodes.find((n) => n.id === a.entity.node) : undefined;
    const fix = node ? nodeActionFor(node) : null;
    const items: MenuItem[] = [
      { label: "Open", icon: ExternalLink, run: () => openRow(r) },
      ...(fix ? [{ label: fix.label, run: () => fix.run({ winId }) }] : []),
      { separator: true },
      { label: "Acknowledge", icon: Check, disabled: r.incident.state === "acknowledged" ? "Already acknowledged" : false, run: () => ack([r.child && r.alert ? r.alert.id : r.incident.id]) },
      {
        label: "Silence",
        icon: BellOff,
        submenu: (Object.keys(DURATION_LABEL) as SilenceDuration[]).map((d) => ({ label: `${DURATION_LABEL[d]}${needsMac(silenceFor(r, d).matcher, d) ? "…" : ""}`, run: () => onSilence(silenceFor(r, d)) })),
      },
    ];
    showContextMenu(e, items);
  };

  const loading = !health;
  const quiet = !loading && !incidents.some((i) => groupOf(i) !== null);
  const silenceButton = useRef<HTMLSpanElement>(null);
  return (
    <>
      <Toolbar label="Alert actions">
        <ToolButton icon={Check} label="Acknowledge" disabled={!picked.length || picked.every((r) => r.incident.state === "acknowledged")} locked={locked} scope={scope} action="alert_ack"
          onClick={() => ack()} title="A known issue: no repeats and it stops counting, until it gets worse" />
        <HelpTip topic="mon.alerts.acknowledge" />
        <span ref={silenceButton} className="al-tool-menu">
          <ToolButton icon={BellOff} label="Silence ▾" disabled={picked.length !== 1} locked={locked} scope={scope} action="alert_silence"
            title={picked.length > 1 ? "Silence one incident or alert at a time" : "1 hour, 1 day, 1 week, until it clears (at most 7 days), or indefinitely"}
            onClick={() => {
              const el = silenceButton.current?.querySelector("button");
              if (!el || !pickedOne) return;
              const r = el.getBoundingClientRect();
              silenceMenu({ clientX: r.left, clientY: r.bottom + 2, preventDefault() {}, stopPropagation() {}, currentTarget: el, target: el, detail: 0 } as unknown as ReactMouseEvent, pickedOne);
            }} />
        </span>
        <HelpTip topic="mon.alerts.silence" />
        <ToolButton icon={BellRing} label="Unsilence" disabled={!silenceIds.length} locked={locked} scope={scope} action="alert_unsilence" priority={2} onClick={() => unsilence(silenceIds)} />
        <ToolSeparator />
        <ToolButton icon={ExternalLink} label="Open" disabled={!pickedOne} onClick={() => pickedOne && openRow(pickedOne)} title="Show what it's about" />
        <Spacer />
        {known > 0 && (
          <button type="button" className="mon-link al-known" onClick={onShowSilenced} title="Silenced alerts: listed, never counted">
            {known === 1 ? "1 silenced" : `${known} silenced`}
          </button>
        )}
        <span className="toolbar-text muted">{health?.lastAlertMs ? `Last change ${fmtAgo(health.lastAlertMs, now)}` : "No alerts recorded yet"}</span>
      </Toolbar>
      {quiet ? (
        <EmptyState
          icon={CircleCheck}
          title="All systems healthy"
          body={health?.lastAlertMs ? `Nothing needs attention. The last alert changed ${fmtAgo(health.lastAlertMs, now)}.` : "Nothing needs attention."}
          action={{ label: "Show history", onClick: onShowHistory }}
        />
      ) : loading ? (
        <EmptyState icon={Bell} title="Waiting for the health engine…" body="" />
      ) : (
        <div className="al-groups">
          {GROUPS.map((g) => {
            const rows = rowsOf(g.id);
            if (!rows.length) return null;
            const n = incidents.filter((i) => groupOf(i) === g.id).length;
            return (
              <section key={g.id} className={`al-group al-group-${g.id}`} aria-label={g.title}>
                <h3 className="al-group-title">
                  {g.title} <span className="al-group-n">{n}</span>
                  <InfoTip topic={g.help} label={`About ${g.title.toLowerCase()}`} />
                </h3>
                <div className="al-keys" onKeyDownCapture={onKeys}>
                <Table
                  tableId={`alerts-${g.id}`}
                  ariaLabel={`${g.title} incidents`}
                  columns={columns}
                  rows={rows}
                  rowKey={(r) => r.key}
                  selection={selection}
                  onSelectionChange={(keys) => setSelection(keys)}
                  selectionMode="multi"
                  onActivate={openRow}
                  onRowContextMenu={menu}
                  rowClass={(r) => {
                    const cls = r.child ? "al-row-child" : "al-row";
                    if (g.id !== "firing" || (r.child && r.alert && !r.alert.counted)) return `${cls} row-dim`;
                    const sev = r.child && r.alert ? r.alert.severity : r.incident.severity;
                    return `${cls} ${sev === "crit" ? "row-crit" : sev === "warn" ? "row-warn" : ""}`;
                  }}
                  rowDetail={(r) => {
                    // The root's own detail on its incident's second line (the counter rates behind
                    // a RoCE alert, what a memory alert means); a child's evidence under the child.
                    const d = r.alert?.detail;
                    if (!d || (r.child && r.alert?.rule !== "roce-iface")) return null;
                    return <span className="al-evidence">{r.alert?.rule === "roce-iface" ? d.split(". ")[0] : d}</span>;
                  }}
                  empty="None."
                />
                </div>
              </section>
            );
          })}
        </div>
      )}
    </>
  );
}

// ---------------------------------------------------------------- Silence sheet

function SilenceSheet({ req, demo, onClose }: { req: SheetRequest; demo: boolean; onClose: () => void }) {
  const winId = useWindowId();
  const host = windowElement(winId);
  const [duration, setDuration] = useState<SilenceDuration>(req.duration);
  const [wide, setWide] = useState(false);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  // Single flight: a double click (or Enter, then a click) silences once, before React re-renders.
  const inFlight = useRef(false);
  useEffect(() => input.current?.focus(), []);
  const matcher = wide && req.wide ? req.wide.matcher : req.matcher;
  const label = wide && req.wide ? req.wide.label : req.label;
  // Wider than one alert or incident, until it clears, or a week or longer: Mac scope and a reason.
  const mac = needsMac(matcher, duration);
  const missing = mac && !reason.trim();
  const note = silenceNote(matcher, label, duration, demo);
  const save = async () => {
    if (missing || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      const state = await withAdmin(() => healthApi.silence(matcher, duration, reason.trim(), label), { scope: mac && !demo ? "mac" : alertScope("alert_silence", demo, duration), action: "alert_silence" });
      applyHealth(state);
      toast({ kind: "success", title: `Silenced ${label} ${durationWords(duration)}`, body: reason.trim() || undefined, key: "alerts:silence" });
      onClose();
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(errText(e));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };
  if (!host) return null;
  return createPortal(
    <div className="sb-sheet-scrim" data-shell-overlay onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        className="sb-dialog sheet al-sheet"
        role="dialog"
        aria-modal="true"
        aria-labelledby="al-silence-title"
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          } else if (e.key === "Enter" && e.target instanceof HTMLInputElement) {
            e.preventDefault();
            void save();
          }
        }}
      >
        <BellOff className="sb-dialog-icon info" size={28} strokeWidth={1.75} aria-hidden />
        <div className="sb-dialog-main">
          <h2 id="al-silence-title" className="sb-dialog-title">Silence {label}</h2>
          <div className="sb-dialog-body">
            Not counted and not paged while silenced; it's listed in Alerts › Silenced.{demo ? " This is a Demo silence: Live alerts never see it." : " The node agents get it too."}
          </div>
          {req.wide && (
            <div className="sb-dialog-field">
              <span>What</span>
              <Segmented<"one" | "wide">
                label="What to silence"
                value={wide ? "wide" : "one"}
                onChange={(v) => setWide(v === "wide")}
                options={[{ value: "one", label: req.matcher.incident ? "This incident" : "This alert" }, { value: "wide", label: `${req.wide.label[0].toUpperCase()}${req.wide.label.slice(1)}` }]}
              />
            </div>
          )}
          <div className="sb-dialog-field">
            <span id="al-silence-for">For</span>
            <Segmented<SilenceDuration>
              label="How long"
              value={duration}
              onChange={setDuration}
              options={(Object.keys(DURATION_LABEL) as SilenceDuration[]).map((d) => ({ value: d, label: DURATION_LABEL[d] }))}
            />
          </div>
          {note && <span className="muted small">{note}</span>}
          <label className="sb-dialog-field">
            <span>Reason{mac ? "" : " (optional)"}</span>
            <input
              ref={input}
              value={reason}
              maxLength={200}
              placeholder={mac ? "Say why, for whoever reads the log" : "Expanding the disk on Friday"}
              onChange={(e) => setReason(e.target.value)}
              aria-invalid={missing && reason.length > 0}
              autoComplete="off"
            />
            {/* A hint until there's something to judge: nothing has gone wrong yet when the sheet opens. */}
            {missing && <span className={reason.length > 0 ? "sb-dialog-error" : "muted small"}>Say why: this silence needs a reason.</span>}
          </label>
          {error && <span className="sb-dialog-error" role="alert">{error}</span>}
          <div className="sb-dialog-actions">
            <span className="spacer" />
            <button type="button" className="btn" onClick={onClose}>Cancel</button>
            <button type="button" className="btn primary" disabled={missing || busy} onClick={save}>{busy ? "Silencing…" : "Silence"}</button>
          </div>
        </div>
      </div>
    </div>,
    host,
  );
}

const durationWords = (d: SilenceDuration) => (d === "resolved" ? "until it clears (at most 7 days)" : d === "never" ? "indefinitely" : `for ${DURATION_LABEL[d]}`);

/** A silence's end in words: "until it clears (at most 7 days)", "until Tue 10:42". */
const untilWords = (s: Silence, now: number) => silenceLine(s, now).replace(/^Silenced /, "").replace(/^until it clears/, "until it clears (at most 7 days)");

// ---------------------------------------------------------------- Silenced

function Silenced({ health }: { health: HealthState | null }) {
  const wm = useWM();
  const now = useNow(15_000);
  const admin = useAdminState();
  const scope = alertScope("alert_unsilence", health?.demo !== false);
  const [selection, setSelection] = useState<Set<string>>(new Set());
  const list = health?.silences ?? [];
  const alerts = health?.alerts ?? [];
  const covering = (s: Silence) => alerts.filter((a) => a.silenceId === s.id);
  const picked = list.filter((s) => selection.has(s.id));
  // Enter or double-click opens what the silence covers, like a row in Active.
  const openSilenced = (s: Silence) => {
    const a = covering(s)[0];
    if (!a) return;
    const t = entityTarget(a.entity);
    wm.open(t.kind, t.params);
  };

  const end = async (ids = picked.map((s) => s.id)) => {
    if (!ids.length) return;
    const labels = ids.map((id) => list.find((x) => x.id === id)?.label).filter((l): l is string => !!l);
    try {
      let last: HealthState | null = null;
      await withAdmin(async () => {
        for (const id of ids) {
          // Ending one silence ends every silence on the same alerts: skip the ones already gone.
          if (last && !last.silences?.some((x) => x.id === id)) continue;
          last = await healthApi.unsilence(id);
        }
      }, { scope, action: "alert_unsilence" });
      if (last) applyHealth(last);
      toast({ kind: "success", title: ids.length === 1 ? "Silence ended" : `${ids.length} silences ended`, body: resumesFor(labels), key: "alerts:unsilence" });
    } catch (e) {
      if (e === ADMIN_REFUSED) return;
      toast({ kind: "error", title: "Couldn't end the silence", body: errText(e), key: "alerts:unsilence" });
    }
  };

  const columns: Column<Silence>[] = [
    { key: "glyph", label: "", width: 30, priority: 1, hideable: false, render: () => <StateGlyph state="silenced" /> },
    { key: "what", label: "Silenced", width: 240, priority: 1, sort: (s) => s.label, title: (s) => s.label, render: (s) => s.label || "(an alert)" },
    {
      key: "until", label: "Until", kind: "message", priority: 1, sort: (s) => (s.until.kind === "at" ? s.until.ms : Number.MAX_SAFE_INTEGER),
      title: (s) => `Silenced ${untilWords(s, now)}\nSet ${fmtDateTime(s.createdMs, now)} by ${s.createdBy}`,
      render: (s) => untilWords(s, now),
    },
    {
      key: "now", label: "Now", width: 150, priority: 2, sort: (s) => covering(s).length,
      render: (s) => {
        const n = covering(s).filter((a) => a.state === "firing").length;
        return <span className="muted">{n ? `${n === 1 ? "1 alert" : `${n} alerts`} firing` : "Nothing firing"}</span>;
      },
    },
    { key: "source", label: "", width: 64, priority: 3, render: (s) => (s.source === "demo" ? <Badge tone="neutral" title="Set in Demo: Live alerts never see it">Demo</Badge> : null) },
  ];

  return (
    <>
      <Toolbar label="Silence actions">
        <ToolButton icon={BellRing} label="End silence" disabled={!picked.length} locked={!unlockedFor(scope, admin)} scope={scope} action="alert_unsilence" onClick={() => end()} />
        <Spacer />
        <HelpTip topic="mon.alerts.silenced" label="About silences" />
      </Toolbar>
      {list.length ? (
        <Table
          tableId="alerts-silenced"
          ariaLabel="Silences"
          columns={columns}
          rows={list}
          rowKey={(s) => s.id}
          selection={selection}
          onSelectionChange={setSelection}
          selectionMode="multi"
          rowClass={() => "row-dim"}
          onActivate={openSilenced}
          onRowContextMenu={(s, e) => showContextMenu(e, [{ label: "End silence", icon: BellRing, run: () => end([s.id]) }])}
          footer
          itemNoun={["silence", "silences"]}
        />
      ) : (
        <EmptyState icon={BellOff} title="Nothing is silenced" body="Silence a known issue from Active: 1 hour, 1 day, 1 week, until it clears (at most 7 days), or indefinitely." />
      )}
    </>
  );
}

// ---------------------------------------------------------------- History

type KindFilter = "all" | "raised" | "resolved" | "away";
type SevFilter = "any" | "crit" | "warn";

/** "agent:cell1" → "cell1's agent". */
function sourceWord(source?: string): string | null {
  return source?.startsWith("agent:") ? `the agent on ${source.slice(6)}` : null;
}

function AlertHistory({ refreshKey, demo }: { refreshKey: number; demo: boolean }) {
  const wm = useWM();
  const [events, setEvents] = useState<AlertEvent[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [kind, setKind] = useWinState<KindFilter>("historyKind", "all");
  const [sev, setSev] = useWinState<SevFilter>("historySeverity", "any");
  const [query, setQuery] = useState("");
  const now = useNow(30_000);

  useEffect(() => {
    if (!inTauri) return;
    healthApi.history(1000).then(setEvents, (e) => setError(String(e)));
  }, [refreshKey, demo]);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (events ?? [])
      .filter((e) => kind === "all" || (kind === "raised" ? e.kind === "raised" || e.kind === "escalated" : kind === "away" ? e.away : e.kind === "resolved"))
      .filter((e) => sev === "any" || e.alert.severity === sev)
      .filter((e) => !q || `${e.alert.summary} ${e.alert.message} ${e.alert.entity.label} ${e.incidentTitle ?? ""}`.toLowerCase().includes(q))
      .map((e, i) => ({ ...e, key: `${e.tsMs}-${e.kind}-${e.alert.id}-${i}` }));
  }, [events, kind, sev, query]);
  const awayCount = (events ?? []).filter((e) => e.away).length;

  const quietKinds: AlertEvent["kind"][] = ["muted", "acked", "unmuted", "silenced", "unsilenced"];
  const columns: Column<(typeof rows)[number]>[] = [
    {
      key: "time", label: "Time", width: 170, priority: 1, sort: (e) => e.tsMs,
      render: (e) => <span className="num" title={new Date(e.tsMs).toUTCString()}>{fmtDateTime(e.tsMs, now)}</span>,
    },
    {
      key: "event", label: "Event", width: 200, priority: 1, sort: (e) => e.kind,
      render: (e) => (
        <span className="al-event">
          <StatusDot status={e.kind === "resolved" ? "online" : quietKinds.includes(e.kind) || e.alert.planned ? "unconfigured" : sevDot(e.alert.severity)} text={e.alert.planned && e.kind !== "resolved" ? `${EVENT_WORD[e.kind]} · planned` : EVENT_WORD[e.kind]} />
          {e.away && (
            <Badge tone="neutral" title={`${EVENT_WORD[e.kind]} ${fmtDateTime(e.tsMs, now)} while SparkBox was closed; recorded by ${sourceWord(e.source) ?? "a node agent"}`}>
              While away
            </Badge>
          )}
        </span>
      ),
    },
    { key: "sev", label: "Severity", width: 90, priority: 3, sort: (e) => e.alert.severity, render: (e) => SEVERITY_WORD[e.alert.severity] },
    { key: "entity", label: "Affects", width: 210, priority: 1, sort: (e) => e.alert.entity.label, render: (e) => <EntityButton entity={e.alert.entity} /> },
    {
      key: "message", label: "Message", kind: "message", priority: 1, sort: (e) => e.alert.message,
      render: (e) => (e.kind === "resolved" && e.alert.reason ? `${e.alert.message} · ${e.alert.reason}` : e.note ? `${e.alert.message} · ${e.note}` : e.alert.message),
      title: (e) => [e.incidentTitle && e.incidentTitle !== e.alert.summary ? `Incident: ${e.incidentTitle}` : null, e.alert.detail ?? e.alert.message, sourceWord(e.source) ? `Recorded by ${sourceWord(e.source)}` : null].filter(Boolean).join("\n"),
    },
  ];

  if (error) return <EmptyState icon={History} title="The alert log couldn't be read" body={error} />;
  return (
    <>
      <Toolbar label="History filters">
        <Segmented<KindFilter>
          label="Events"
          value={kind}
          onChange={setKind}
          options={[
            { value: "all", label: "All" },
            { value: "raised", label: "Raised" },
            { value: "resolved", label: "Cleared" },
            ...(awayCount ? [{ value: "away" as const, label: `While away (${awayCount})` }] : []),
          ]}
        />
        <Segmented<SevFilter>
          label="Severity"
          value={sev}
          onChange={setSev}
          options={[{ value: "any", label: "Any severity" }, { value: "crit", label: "Critical" }, { value: "warn", label: "Warning" }]}
        />
        <Spacer />
        {demo && <Badge tone="neutral" title="The Demo simulator's own log; Live keeps a separate one">Demo log</Badge>}
        <SearchField value={query} onChange={setQuery} placeholder="Filter alerts" count={events ? rows.length : undefined} />
        <HelpTip topic="mon.alerts.history" />
      </Toolbar>
      <Table
        tableId="alerts-history"
        ariaLabel="Alert history"
        columns={columns}
        rows={rows}
        rowKey={(e) => e.key}
        initialSort={{ key: "time", dir: -1 }}
        onActivate={(e) => {
          const t = entityTarget(e.alert.entity);
          wm.open(t.kind, t.params);
        }}
        footer
        itemNoun={["event", "events"]}
        empty={events ? "Nothing matches these filters." : "Loading the alert log…"}
      />
    </>
  );
}

// ---------------------------------------------------------------- Kernel events

/** A kernel event with what it means (`kernel_events`, src-tauri/src/health/kernel.rs). */
export interface DecodedEvent {
  /** "" for a group that spans nodes (see `nodes`). */
  node: string;
  tsMs: number;
  /** link: a cable, link or RoCE port changing state (recabling), information only. boot: a node's boot, its boot-time link events under it. */
  kind: "xid" | "mlx5" | "link" | "oom" | "hung" | "boot";
  code: number | null;
  text: string;
  severity: "info" | "warn" | "crit";
  process: string | null;
  title: string;
  meaning: string;
  action: string;
  // ── r4-alerts ──
  /** One recabling: link events within 15 minutes of each other, on any node. */
  group?: string | null;
  /** A group's row: its events, newest first. */
  events?: DecodedEvent[];
  nodes?: string[];
  /** A group's row: its last event (`tsMs` is its first). */
  toMs?: number | null;
}

const KIND_WORD: Record<DecodedEvent["kind"], string> = { xid: "GPU Xid", mlx5: "ConnectX-7", link: "Fabric link", oom: "Out of memory", hung: "Hung task", boot: "Boot" };

/** "Oct 26, 2:16–2:29 PM" (one day), "Oct 25, 1:47 PM – Oct 26, 2:29 PM". */
export function fmtRange(from: number, to: number): string {
  const a = new Date(from);
  const b = new Date(to);
  const day = (d: Date) => d.toLocaleDateString([], { month: "short", day: "numeric" });
  const time = (d: Date) => d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (day(a) !== day(b)) return `${day(a)}, ${time(a)} – ${day(b)}, ${time(b)}`;
  const [ta, tb] = [time(a), time(b)];
  if (ta === tb) return `${day(a)}, ${ta}`;
  const suffix = (t: string) => t.match(/\s?[AP]M$/i)?.[0] ?? "";
  const same = suffix(ta) && suffix(ta) === suffix(tb);
  return `${day(a)}, ${same ? ta.slice(0, -suffix(ta).length) : ta}–${tb}`;
}

type EventSev = "any" | "crit" | "warn" | "info";

function KernelEvents({ node, onShowAll, refreshKey }: { node: string | null; onShowAll: () => void; refreshKey: number }) {
  const wm = useWM();
  const now = useNow(30_000);
  const { data, error } = usePolled(() => invoke<DecodedEvent[]>("kernel_events", { node, sinceMs: 0 }), 10_000, `${refreshKey}|${node}`);
  const [selection, setSelection] = useState<Set<string>>(new Set());
  const [sev, setSev] = useWinState<EventSev>("eventsSeverity", "any");
  const [open, setOpen] = useWinState<string[]>("eventsOpen", []);
  type EvRow = DecodedEvent & { key: string; child: boolean };
  const rows = useMemo(() => {
    const out: EvRow[] = [];
    (data ?? []).filter((e) => sev === "any" || e.severity === sev).forEach((e, i) => {
      const key = e.events?.length ? `g:${e.group}` : `${e.node}-${e.tsMs}-${e.kind}-${e.code ?? ""}-${i}`;
      out.push({ ...e, key, child: false });
      if (e.events?.length && open.includes(key)) e.events.forEach((c, j) => out.push({ ...c, key: `${key}:${j}`, child: true }));
    });
    return out;
  }, [data, sev, open]);
  const counts = useMemo(() => {
    const c = { crit: 0, warn: 0, info: 0 };
    for (const e of data ?? []) c[e.severity] += e.events?.length || 1;
    return c;
  }, [data]);
  const picked = rows.find((r) => selection.has(r.key)) ?? null;
  const toggle = (key: string) => setOpen(open.includes(key) ? open.filter((k) => k !== key) : [...open, key]);
  const onKeys = (e: ReactKeyboardEvent) => {
    if (!picked || (e.key !== "ArrowRight" && e.key !== "ArrowLeft")) return;
    const parent = picked.child ? picked.key.slice(0, picked.key.lastIndexOf(":")) : picked.key;
    if (e.key === "ArrowRight" && !picked.child && picked.events?.length && !open.includes(parent)) setOpen([...open, parent]);
    else if (e.key === "ArrowLeft" && open.includes(parent)) {
      setOpen(open.filter((k) => k !== parent));
      if (picked.child) setSelection(new Set([parent]));
    } else return;
    e.preventDefault();
    e.stopPropagation();
  };
  const nodeWord = (e: DecodedEvent) => e.node || (e.nodes?.length ? (e.nodes.length > 2 ? `${e.nodes.length} nodes` : e.nodes.join(", ")) : "");

  const columns: Column<EvRow>[] = [
    {
      key: "time", label: "Time", width: 170, priority: 1, sort: (e) => e.toMs ?? e.tsMs,
      title: (e) => (e.toMs ? `${new Date(e.tsMs).toUTCString()} to ${new Date(e.toMs).toUTCString()}` : new Date(e.tsMs).toUTCString()),
      render: (e) => <span className={`num${e.child ? " al-ev-child" : ""}`}>{fmtDateTime(e.tsMs, now)}</span>,
    },
    { key: "node", label: "Node", width: 110, priority: 1, sort: (e) => nodeWord(e), title: (e) => (e.nodes?.length ? e.nodes.join(", ") : e.node), render: (e) => nodeWord(e) },
    {
      key: "sev", label: "Severity", width: 100, priority: 2, sort: (e) => (e.severity === "crit" ? 3 : e.severity === "warn" ? 2 : 1),
      render: (e) => (e.severity === "info" ? <span className="al-ev-info"><span className="al-glyph-box"><AdvisoryGlyph title="Information: never an alert" /></span>Info</span> : <StatusDot status={e.severity} text={SEVERITY_WORD[e.severity]} />),
    },
    { key: "kind", label: "Kind", width: 120, priority: 3, sort: (e) => e.kind, render: (e) => KIND_WORD[e.kind] ?? e.kind },
    {
      key: "title", label: "Event", kind: "message", priority: 1, sort: (e) => e.title,
      title: (e) => (e.events?.length ? `${e.title}\n${fmtRange(e.tsMs, e.toMs ?? e.tsMs)}` : e.text),
      render: (e) => {
        if (!e.events?.length) return <span className={e.child ? "al-ev-child" : undefined}>{e.title}</span>;
        const expanded = open.includes(e.key);
        return (
          <span className="al-what">
            <button type="button" className="al-disclose" aria-expanded={expanded} aria-label={`${expanded ? "Hide" : "Show"} the ${e.events.length} events`} title={`${expanded ? "Hide" : "Show"} them (→ ←)`}
              onClick={(ev) => {
                ev.stopPropagation();
                toggle(e.key);
              }}>
              {expanded ? <ChevronDown size={12} aria-hidden /> : <ChevronRight size={12} aria-hidden />}
            </button>
            <span className="al-what-text">{`${e.title} · ${fmtRange(e.tsMs, e.toMs ?? e.tsMs)}`}</span>
          </span>
        );
      },
    },
  ];

  if (error) return <EmptyState icon={Cpu} title="Kernel events couldn't be read" body={error} />;
  return (
    <div className="col fill">
      <Toolbar label="Event filters">
        <Segmented<EventSev>
          label="Severity"
          value={sev}
          onChange={setSev}
          options={[
            { value: "any", label: "Any severity" },
            { value: "crit", label: counts.crit ? `Critical (${counts.crit})` : "Critical" },
            { value: "warn", label: counts.warn ? `Warning (${counts.warn})` : "Warning" },
            { value: "info", label: counts.info ? `Info (${counts.info})` : "Info" },
          ]}
        />
        <Spacer />
        <HelpTip topic="mon.alerts.events" label="About kernel events" />
      </Toolbar>
      {node && <Notice action={{ label: "Show all nodes", onClick: onShowAll }}>Kernel events on {node} only.</Notice>}
      <div className="al-split" onKeyDownCapture={onKeys}>
        <Table
          tableId="alerts-events"
          ariaLabel="Kernel events"
          columns={columns}
          rows={rows}
          rowKey={(e) => e.key}
          initialSort={{ key: "time", dir: -1 }}
          selection={selection}
          onSelectionChange={setSelection}
          selectionMode="single"
          rowClass={(e) => (e.child ? "al-ev-row-child row-dim" : e.severity === "info" ? "row-dim" : undefined)}
          onActivate={(e) => (e.events?.length ? toggle(e.key) : wm.open("node", { id: e.node, tab: e.kind === "mlx5" || e.kind === "link" ? "network" : e.kind === "xid" ? "gpu" : "overview" }))}
          footer
          itemNoun={["event", "events"]}
          empty={data ? (sev === "any" ? `No GPU Xid errors, ConnectX-7 events, out-of-memory kills or hung tasks in ${node ? `${node}'s kernel log` : "the nodes' kernel logs"}.` : "None of this severity.") : "Reading the kernel logs…"}
        />
        {picked && (
          <aside className={`al-detail al-detail-${picked.severity}`} aria-label="Event details">
            <h4 className="al-detail-title">{picked.title}</h4>
            <p className="al-detail-meta">
              {picked.events?.length ? `${picked.nodes?.join(", ")} · ${fmtRange(picked.tsMs, picked.toMs ?? picked.tsMs)}` : `${picked.node} · ${fmtDateTime(picked.tsMs, now)}`}
              {picked.process ? ` · ${picked.process}` : ""}
            </p>
            <p><b>What it means:</b> {picked.meaning}.</p>
            {picked.action && <p><b>What to do:</b> {picked.action}</p>}
            {picked.events?.length ? (
              <pre className="al-detail-raw">{picked.events.slice(0, 12).map((e) => `${e.node}  ${e.text}`).join("\n")}{picked.events.length > 12 ? `\nand ${picked.events.length - 12} more` : ""}</pre>
            ) : (
              <pre className="al-detail-raw">{picked.text}</pre>
            )}
          </aside>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- Deliveries

/** Outcomes this build adds to the delivery log: a dev run's mute, and a flapping re-raise. */
type AnyOutcome = Outcome | "muted" | "flapping";

/** What became of each alert event at the gate, in the words Settings › Notifications uses. */
function outcomeDot(o: AnyOutcome): DotStatus {
  return o === "sent" || o === "agent" ? "online" : o === "failed" ? "crit" : "unconfigured";
}

const outcomeOf = (d: Delivery): AnyOutcome => (d.outcome as AnyOutcome | undefined) ?? (!d.ok ? "failed" : d.demo ? "demo" : "sent");

/** Outcomes that reached a channel or tried to (the default view); the rest weren't paged. */
const REACHED: AnyOutcome[] = ["sent", "agent", "failed", "demo", "muted"];

/** The Outcome column's word; the reason goes in Result, the whole sentence in the tooltip. */
const OUTCOME_SHORT: Record<AnyOutcome, string> = {
  sent: "Sent", agent: "Sent", coalesced: "Grouped", duplicate: "Duplicate", held: "Held", silenced: "Silenced",
  planned: "Planned", eased: "Eased", "not-paged": "Not paged", failed: "Failed", demo: "Demo", muted: "Muted", flapping: "Flapping",
};

/** What happened in detail: when it went out, why it failed, or why it wasn't sent. */
function resultLine(d: Delivery, ago: (ms: number) => string): string {
  const o = outcomeOf(d);
  if (o === "demo") return `Not sent · ${ago(d.tsMs)}`;
  if (o === "muted") return d.reason ?? "Muted in this dev run";
  if (o === "sent" || o === "failed" || o === "agent") return deliveryLine(d, ago);
  return d.reason ?? outcomeLabel(d);
}

function viaWord(d: Delivery): string {
  return d.via.startsWith("agent:") ? `Agent on ${d.via.slice(6)}` : d.via === "agent" ? "The agents" : "This Mac";
}

/** "6:49 AM". */
const fmtTime = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/** "Today", "Yesterday", "Tuesday, Oct 27". */
export function dayWord(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const start = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((start(new Date(now)) - start(d)) / 86_400_000);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  return d.toLocaleDateString([], { weekday: "long", month: "short", day: "numeric" });
}

function Deliveries() {
  const wm = useWM();
  const now = useNow(15_000);
  const { data, error } = usePolled(() => notifyApi.deliveries(200), 5000, "deliveries");
  const [all, setAll] = useWinState<boolean>("deliveriesAll", false);
  const rows = data ?? [];
  const reached = rows.filter((d) => REACHED.includes(outcomeOf(d)) || d.test);
  const notPaged = rows.length - reached.length;
  const shown = all ? rows : reached;
  const failed = rows.filter((d) => outcomeOf(d) === "failed").length;
  const ago = (ms: number) => fmtAgo(ms, now);
  // One table per day, newest first, under its day header.
  const days = useMemo(() => {
    const out: { day: string; rows: Delivery[] }[] = [];
    for (const d of [...shown].sort((a, b) => b.tsMs - a.tsMs)) {
      const day = dayWord(d.tsMs, now);
      const last = out[out.length - 1];
      if (last?.day === day) last.rows.push(d);
      else out.push({ day, rows: [d] });
    }
    return out;
  }, [shown, now]);

  const columns: Column<Delivery>[] = [
    { key: "time", label: "Time", width: 76, align: "right", priority: 1, sort: (d) => d.tsMs, title: (d) => new Date(d.tsMs).toString(), render: (d) => <span className="num">{fmtTime(d.tsMs)}</span> },
    {
      key: "outcome", label: "Outcome", width: 100, fit: "content", priority: 1, sort: (d) => outcomeOf(d),
      title: (d) => outcomeLabel(d),
      render: (d) => <StatusDot status={outcomeDot(outcomeOf(d))} text={OUTCOME_SHORT[outcomeOf(d)] ?? outcomeOf(d)} />,
    },
    { key: "channel", label: "Channel", width: 130, fit: "content", priority: 2, sort: (d) => d.channelName, title: (d) => KIND_LABEL[d.kind], render: (d) => d.channelName || <span className="muted">None</span> },
    { key: "via", label: "Sent by", width: 110, fit: "content", priority: 3, sort: (d) => d.via, render: (d) => viaWord(d) },
    { key: "title", label: "Message", kind: "message", priority: 1, sort: (d) => d.title, render: (d) => (d.test ? `${d.title} (test)` : d.title), title: (d) => [d.title, d.body].filter(Boolean).join("\n") },
    {
      key: "result", label: "Result", width: 240, minWidth: 120, fit: "flex", priority: 2, sort: (d) => (d.ok ? 1 : 0),
      title: (d) => resultLine(d, ago) + (d.attempts > 1 ? ` · try ${d.attempts}` : ""),
      render: (d) => (
        <span className="al-result">
          <span className="al-result-text muted">{resultLine(d, ago)}</span>
          {d.attempts > 1 && <span className="muted"> · try {d.attempts}</span>}
        </span>
      ),
    },
  ];

  if (error) return <EmptyState icon={Send} title="The delivery log couldn't be read" body={error} />;
  return (
    <>
      <Toolbar label="Delivery actions">
        <ToolButton icon={Settings2} label="Channels…" onClick={() => wm.open("settings", { section: "notifications" })} title="Where alerts go: Settings › Notifications" />
        <Segmented<string>
          label="Which deliveries"
          value={all ? "all" : "reached"}
          onChange={(v) => setAll(v === "all")}
          options={[{ value: "reached", label: "Reached or tried a channel" }, { value: "all", label: notPaged ? `Show not paged (${notPaged})` : "Show not paged" }]}
        />
        <Spacer />
        <span className="toolbar-text muted">{failed ? `${failed} failed` : ""}</span>
        <HelpTip topic="mon.alerts.deliveries" label="About deliveries" />
      </Toolbar>
      {data && !shown.length ? (
        <EmptyState
          icon={Send}
          title={rows.length ? "Nothing reached a channel" : "Nothing sent yet"}
          body={rows.length ? `${notPaged === 1 ? "1 alert event wasn't" : `${notPaged} alert events weren't`} paged.` : "Add a channel in Settings › Notifications."}
          action={rows.length ? { label: `Show not paged (${notPaged})`, onClick: () => setAll(true) } : undefined}
        />
      ) : !data ? (
        <EmptyState icon={Send} title="Loading the delivery log…" body="" />
      ) : (
        <div className="al-groups al-deliveries">
          {days.map((g) => (
            <section key={g.day} className="al-group" aria-label={g.day}>
              <h3 className="al-group-title">{g.day} <span className="al-group-n">{g.rows.length}</span></h3>
              <Table
                tableId="alerts-deliveries"
                ariaLabel={`Deliveries, ${g.day}`}
                columns={columns}
                rows={g.rows}
                rowKey={(d) => d.id}
                initialSort={{ key: "time", dir: -1 }}
                rowClass={(d) => (outcomeOf(d) === "failed" ? "row-crit" : REACHED.includes(outcomeOf(d)) ? undefined : "row-dim")}
              />
            </section>
          ))}
        </div>
      )}
    </>
  );
}
