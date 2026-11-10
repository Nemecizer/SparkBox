// systemd services across the cluster, like Cockpit's Services page with every node side by
// side: failed units first, one column per node. Selecting a cell shows that unit's status
// (when it started, its main process, why it failed) and its recent journal, scrolled to the
// line that matters. Actions run on the nodes you pick, one at a time, and say first what
// they'll break. Running units are quiet; only failed, masked or out-of-step ones stand out.
// A failed unit reads in its alert's severity everywhere in the window (amber for the default
// warning, red only when the unit-failed rule is set to critical): cell, row rule, banner,
// detail header, Result and Error rows and the journal's error lines. Like Cockpit, a unit a
// timer starts, a oneshot or a static unit that's inactive is idle, in grey: only a failed unit,
// or an enabled long-running one that's stopped without a skipped condition, is amber.
//
// It reads each node with one `systemctl show` every 10 s while open, and the installed unit
// files on open and every 5 minutes. A unit seen in the last 5 minutes stays listed, so the
// count doesn't flicker as systemd loads and unloads idle units.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { Check, ChevronDown, FileText, Play, Power, PowerOff, RotateCcw } from "lucide-react";
import { Icon, StopIcon } from "../components/Icon";
import { Table, type Column } from "../components/Table";
import { Badge, CodeBlock, EmptyState, HelpTip, Notice, SearchField, Segmented, Spacer, StatusDot, Toolbar, ToolButton, ToolSeparator } from "../components/ui";
import { useAdmin } from "../lib/admin";
import { api, inTauri, type ServiceAction } from "../lib/api";
import { showContextMenu, type MenuItem } from "../lib/contextMenu";
import { fmtSystemdTime } from "../lib/format";
import { useHealth, type HealthState } from "../lib/health";
import { shortNode } from "../lib/knownPorts";
import { netApi } from "../lib/netApi";
import { opsApi, type JournalLine, type ServiceDetail } from "../lib/opsApi";
import { useCluster } from "../lib/store";
import { openTerminal } from "../lib/terminals";
import type { Service } from "../lib/types";
import { runServiceAction, SERVICE_ACTION_LABEL as ACTION_LABEL } from "../lib/workloadCommands";
import { useViewLink, useWindowEvent, useWindowId, useWM, useWinState, type Params } from "../wm/WindowManager";
import "./ops.css";
import "./workloads.css";

type Show = "running" | "failed" | "all";
type DotStatus = "online" | "unconfigured" | "offline" | "error" | "warn";

// In between is neutral: a real Spark's plymouth-quit-wait stays "activating" from boot on, and
// a unit mid-restart is only passing through. Neither is a warning.
const ACTIVE_DOT: Record<string, DotStatus> = { active: "online", activating: "unconfigured", deactivating: "unconfigured", reloading: "unconfigured", failed: "error", inactive: "offline" };

/** Actions that ask first, so their labels end in "…". */
const ASKS = new Set<ServiceAction>(["stop", "restart", "disable", "mask"]);
const verb = (a: ServiceAction) => `${ACTION_LABEL[a]}${ASKS.has(a) ? "…" : ""}`;

/** Failed first, then starting/stopping, then the rest by name. */
export function unitRank(states: (Service | null)[]): number {
  if (states.some((s) => s?.active === "failed")) return 0;
  if (states.some((s) => s && (s.active === "activating" || s.active === "deactivating"))) return 1;
  return 2;
}

/** Read again this often while the window is open. */
const UNITS_EVERY_MS = 10_000;
/** The installed unit files change rarely: on open and this often. */
const FILES_EVERY_MS = 5 * 60_000;
/** A unit seen this recently stays listed when a read misses it. */
const STICKY_MS = 5 * 60_000;
const LONG_RUNNING = new Set(["simple", "exec", "forking", "notify", "notify-reload", "dbus", "idle"]);

/** Inactive because nothing needs it now: a timer starts it, it runs once (oneshot), it's static, or its condition wasn't met. */
export function isIdle(s: Service): boolean {
  if (s.active !== "inactive" || s.load === "masked" || s.enabled === "masked") return false;
  return s.kind === "oneshot" || (s.triggeredBy?.length ?? 0) > 0 || s.enabled === "static" || s.condition === false;
}

/** "Idle · runs on a timer", "Idle · runs when needed". */
export function idleWords(s: Service): string {
  if (s.triggeredBy?.some((t) => t.endsWith(".timer"))) return "Idle · runs on a timer";
  if (s.condition === false) return "Idle · its start condition isn't met";
  return "Idle · runs when needed";
}

export function cellWord(s: Service | null | undefined): string {
  if (!s) return "—";
  if (s.load === "masked" || s.enabled === "masked") return "masked";
  if (s.active === "active") return s.sub === "running" || !s.sub ? "running" : s.sub;
  if (isIdle(s)) return "idle";
  return s.active;
}

export type FailTone = "warn" | "crit";

/**
 * The tone of a failed unit: its unit-failed alert's severity (warning unless the rule is set to
 * critical). Without an alert (the rule is off) it's the rule's default, a warning.
 */
export function failTone(health: Pick<HealthState, "alerts"> | null, node: string, unit: string): FailTone {
  const a = health?.alerts.find((x) => x.rule === "unit-failed" && x.entity.node === node && x.entity.id === unit);
  return a?.severity === "crit" ? "crit" : "warn";
}

/**
 * How loud a matrix cell is: running is quiet; failed takes its alert's tone; masked, or
 * inactive while the same unit runs on another node, is amber; everything else is plain.
 */
export function cellTone(s: Service | null | undefined, row: (Service | null)[], failed: FailTone = "warn"): "quiet" | "idle" | "plain" | "warn" | "crit" {
  if (!s) return "plain";
  if (s.active === "failed") return failed;
  if (s.load === "masked" || s.enabled === "masked") return "warn";
  if (isIdle(s)) return "idle";
  if (s.active === "inactive" && s.kind !== undefined) {
    // What systemd says it is: amber only for an enabled long-running service that's stopped.
    return s.enabled === "enabled" && LONG_RUNNING.has(s.kind) && s.condition !== false ? "warn" : "plain";
  }
  if (s.active === "inactive") return row.some((o) => o?.active === "active") ? "warn" : "plain";
  if (s.active === "active") return "quiet";
  return "plain";
}

/** Units from one poll, with the unit files that aren't loaded (disabled and not running). */
export function withFiles(units: Service[], files: [string, string][] | null | undefined): Service[] {
  const out = [...units];
  for (const [unit, state] of files ?? []) {
    if (unit.endsWith(".service") && !unit.includes("@") && !out.some((s) => s.unit === unit)) {
      out.push({ unit, description: "", load: "not-loaded", active: "inactive", sub: "dead", enabled: state });
    }
  }
  return out.sort((a, b) => a.unit.localeCompare(b.unit));
}

/**
 * The Active view: units running on some node, and units that were in the last 5 minutes (anacron
 * runs a few minutes an hour), so the list doesn't flicker as they finish. `shown` remembers when.
 */
export function inActiveView(unit: string, states: (Service | null | undefined)[], shown: Map<string, number>, now = Date.now()): boolean {
  if (states.some((s) => s && s.active !== "inactive")) {
    shown.set(unit, now);
    return true;
  }
  return now - (shown.get(unit) ?? -Infinity) < STICKY_MS;
}

/** A node's list, keeping units seen in the last 5 minutes that this read missed. */
export function sticky(fresh: Service[], seen: Map<string, { s: Service; at: number }>, now = Date.now()): Service[] {
  for (const s of fresh) seen.set(s.unit, { s, at: now });
  for (const [u, v] of seen) if (now - v.at > STICKY_MS) seen.delete(u);
  const have = new Set(fresh.map((s) => s.unit));
  return [...fresh, ...[...seen.values()].filter((v) => !have.has(v.s.unit)).map((v) => v.s)].sort((a, b) => a.unit.localeCompare(b.unit));
}

/** Why an action doesn't apply to a unit in these states: "Already running", "Stop it first". */
export function stateReason(a: ServiceAction, states: Service[]): string {
  if (!states.length) return "Not installed here";
  const masked = states.some((s) => s.load === "masked" || s.enabled === "masked");
  switch (a) {
    case "start": return masked ? "It's masked: unmask it first" : "Already running";
    case "stop": case "reload": return "It isn't running";
    case "restart": return "It's masked: unmask it first";
    case "mask": return "Already masked";
    case "enable": return masked ? "It's masked: unmask it first" : "Already starts at boot";
    case "disable": return "Already off at boot";
    case "reset-failed": return "It hasn't failed";
    case "unmask": return "It isn't masked";
    case "daemon-reload": return "Pick a node under Apply to";
  }
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const pad = (n: number | string) => String(n).padStart(2, "0");

/** The time zone a node's systemctl prints, from "Sun 2026-10-25 14:43:47 UTC; 3 days ago" ("UTC"). */
export function nodeZone(activeSince: string | null | undefined): string | undefined {
  return /\s([A-Z]{2,5}|[+-]\d{2}:?\d{2})\s*$/.exec((activeSince ?? "").split(";")[0])?.[1];
}

/**
 * A journal line's time: "14:43:47" today, "Oct 25 14:43:47" before; local, with UTC in the
 * tooltip. journalctl's short form has no year and no zone: it's this year (last year if that
 * would be in the future) in the node's zone, which its systemctl times name (`zone`).
 */
export function journalTime(j: Pick<JournalLine, "tsMs" | "time">, now = Date.now(), zone?: string): { text: string; title: string } {
  let t = fmtSystemdTime(j.tsMs ?? null, now);
  const m = t.ms == null ? /^([A-Z][a-z]{2})\s+(\d{1,2})\s+(\d{2}:\d{2}:\d{2})/.exec(j.time) : null;
  const mo = m ? MONTHS.indexOf(m[1]) : -1;
  if (m && mo >= 0) {
    const at = (y: number) => fmtSystemdTime(`${y}-${pad(mo + 1)}-${pad(m[2])} ${m[3]}${zone ? ` ${zone}` : ""}`, now);
    const year = new Date(now).getFullYear();
    t = at(year);
    if (t.ms != null && t.ms > now + 86_400_000) t = at(year - 1);
  }
  if (t.ms == null) t = fmtSystemdTime(j.time, now);
  if (t.ms == null) return { text: j.time, title: j.time };
  const d = new Date(t.ms);
  const clock = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  const today = d.toDateString() === new Date(now).toDateString();
  return { text: today ? clock : `${d.toLocaleDateString([], { month: "short", day: "numeric" })} ${clock}`, title: `${t.local} · ${t.utc}` };
}

/**
 * The line that says why it failed: since the unit last started, the first error the service
 * logged itself (systemd's "Failed with result…" lines only follow it); else the newest
 * error; else the newest line.
 */
export function focusLine(journal: JournalLine[]): number {
  const fromSystemd = (t: string) => /^systemd\[\d+\]:/.test(t);
  let start = 0;
  for (let i = journal.length - 1; i >= 0; i--) {
    if (fromSystemd(journal[i].text) && /: Start(ing|ed) /.test(journal[i].text)) {
      start = i;
      break;
    }
  }
  for (let i = start; i < journal.length; i++) if (journal[i].priority <= 3 && !fromSystemd(journal[i].text)) return i;
  for (let i = journal.length - 1; i >= 0; i--) if (journal[i].priority <= 3) return i;
  return journal.length - 1;
}

interface MatrixRow {
  unit: string;
  description: string;
  cells: Record<string, Service | null>;
  rank: number;
}

export function Services({ params }: { params: Params }) {
  const cluster = useCluster();
  const health = useHealth();
  const admin = useAdmin();
  const wm = useWM();
  const winId = useWindowId();
  const nodes = cluster?.nodes ?? [];
  const name = (id: string) => nodes.find((n) => n.id === id)?.name ?? id;
  const [scope, setScope] = useWinState<string>("scope", params.node ?? "all");
  const [show, setShow] = useWinState<Show>("show", "running");
  const [text, setText] = useState("");
  const [lists, setLists] = useState<Record<string, Service[] | string>>({});
  const [loading, setLoading] = useState(true);
  const [unit, setUnit] = useState<string | null>(params.unit ?? null);
  const [node, setNode] = useState<string | null>(params.node ?? null);
  // A link from elsewhere (an alert, a node's menu, the palette) while the window is open.
  useViewLink(params, () => {
    if (params.node && scope !== "all") setScope(params.node);
    setUnit(params.unit ?? null);
    setNode(params.node ?? null);
    setTargets(params.node ? [params.node] : []);
  });
  const [targets, setTargets] = useState<string[]>(params.node ? [params.node] : []);
  const [detail, setDetail] = useState<ServiceDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** Long journal lines wrap instead of scrolling sideways: your choice, else on when there's an error line to read. */
  const [wrapChoice, setWrap] = useWinState<boolean | null>("wrap", null);
  /** When the last action started: journal lines since then are highlighted. */
  const [actedAt, setActedAt] = useState<number | null>(null);
  const journalRef = useRef<HTMLOListElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);

  const nodeIds = nodes.map((n) => n.id).join(",");
  /** Per node: the unit files last read, when, and every unit seen in the last 5 minutes. */
  const cache = useRef(new Map<string, { files: [string, string][] | null; filesAt: number; seen: Map<string, { s: Service; at: number }> }>());
  const load = useCallback(async (forceFiles = false) => {
    const ids = nodeIds ? nodeIds.split(",") : [];
    if (!ids.length) return;
    const now = Date.now();
    const results = await Promise.allSettled(ids.map((id) => {
      const c = cache.current.get(id);
      const files = forceFiles || !c || now - c.filesAt >= FILES_EVERY_MS;
      // An older core without services_poll: the full list each time.
      return netApi.servicesPoll(id, files).catch(async (e) => {
        if (!/services_poll/.test(String((e as { message?: string })?.message ?? e))) throw e;
        return { units: await api.servicesList(id), files: null };
      });
    }));
    const next: Record<string, Service[] | string> = {};
    ids.forEach((id, i) => {
      const r = results[i];
      if (r.status !== "fulfilled") {
        next[id] = String((r.reason as { message?: string })?.message ?? r.reason).replace(/^Error: /, "");
        return;
      }
      const c = cache.current.get(id) ?? { files: null, filesAt: 0, seen: new Map() };
      if (r.value.files) {
        c.files = r.value.files;
        c.filesAt = now;
      }
      cache.current.set(id, c);
      next[id] = sticky(withFiles(r.value.units, c.files), c.seen, now);
    });
    setLists(next);
    setLoading(false);
  }, [nodeIds]);

  useEffect(() => {
    void load(true);
    const t = window.setInterval(() => void load(), UNITS_EVERY_MS);
    return () => window.clearInterval(t);
  }, [load]);
  // ⌘R reads every node again now, unit files too, rather than at the next refresh.
  useWindowEvent("refresh", () => {
    void load(true);
    loadDetail();
  });

  const loadDetail = useCallback(() => {
    if (!unit || !node) return setDetail(null);
    setDetailError(null);
    opsApi.serviceDetail(node, unit).then(setDetail, (e) => {
      setDetail(null);
      setDetailError(String((e as { message?: string })?.message ?? e).replace(/^Error: /, ""));
    });
  }, [unit, node]);
  useEffect(loadDetail, [loadDetail]);

  // On open and after every action: the error (or the newest line) in view, not the oldest. The
  // journal scrolls in its own box, so the status above it stays put.
  useLayoutEffect(() => {
    const list = journalRef.current;
    const box = list?.parentElement;
    const line = list?.querySelector<HTMLElement>("[data-focus-line]");
    if (!box || !line || !detail?.journal.length) return;
    box.scrollTop = Math.max(0, line.offsetTop - box.clientHeight / 3);
  }, [detail]);

  const shownNodes = scope === "all" ? nodes.map((n) => n.id) : [scope];
  const activeShown = useRef(new Map<string, number>());
  const needle = text.trim().toLowerCase();
  const rows: MatrixRow[] = useMemo(() => {
    const byUnit = new Map<string, MatrixRow>();
    for (const id of shownNodes) {
      const list = lists[id];
      if (!Array.isArray(list)) continue;
      for (const s of list) {
        const r = byUnit.get(s.unit) ?? { unit: s.unit, description: s.description, cells: {}, rank: 2 };
        r.cells[id] = s;
        if (!r.description) r.description = s.description;
        byUnit.set(s.unit, r);
      }
    }
    return [...byUnit.values()]
      .map((r) => ({ ...r, rank: unitRank(Object.values(r.cells)) }))
      .filter((r) => {
        const states = Object.values(r.cells);
        if (show === "failed") return states.some((s) => s?.active === "failed");
        if (show === "running") return inActiveView(r.unit, states, activeShown.current);
        return true;
      })
      .filter((r) => !needle || `${r.unit} ${r.description}`.toLowerCase().includes(needle));
  }, [lists, shownNodes.join(","), show, needle]);

  const failed = useMemo(() => {
    const out: { node: string; unit: string; tone: FailTone }[] = [];
    for (const id of shownNodes) {
      const list = lists[id];
      if (Array.isArray(list)) for (const s of list) if (s.active === "failed") out.push({ node: id, unit: s.unit, tone: failTone(health, id, s.unit) });
    }
    return out;
  }, [lists, shownNodes.join(","), health]);
  /** The loudest failure in a row, for its rule. */
  const rowTone = (r: MatrixRow): FailTone => (Object.entries(r.cells).some(([id, s]) => s?.active === "failed" && failTone(health, id, r.unit) === "crit") ? "crit" : "warn");
  const unreachable = shownNodes.filter((id) => typeof lists[id] === "string");

  // Open on the first failed unit, so the reason it failed is on screen straight away.
  useEffect(() => {
    if (!unit && failed.length) {
      setUnit(failed[0].unit);
      setNode(failed[0].node);
      setTargets([failed[0].node]);
    }
  }, [failed.length]);

  const select = (u: string, n?: string) => {
    const row = rows.find((r) => r.unit === u);
    const on = n ?? (node && row?.cells[node] ? node : shownNodes.find((id) => row?.cells[id]) ?? null);
    setUnit(u);
    setNode(on);
    setTargets(on ? [on] : []);
  };

  const svc = unit && node && Array.isArray(lists[node]) ? (lists[node] as Service[]).find((s) => s.unit === unit) ?? null : null;
  const hasUnit = (id: string, u = unit) => Array.isArray(lists[id]) && (lists[id] as Service[]).some((s) => s.unit === u);
  const stateOn = (id: string, u: string) => (lists[id] as Service[]).find((s) => s.unit === u)!;

  /** Which actions make sense for a unit on some nodes, and why not when they don't. */
  const applies = (u: string | null, on: string[]) => {
    const states = u ? on.filter((id) => hasUnit(id, u)).map((id) => stateOn(id, u)) : [];
    const masked = states.some((s) => s.load === "masked" || s.enabled === "masked");
    const none = !states.length;
    return {
      none,
      masked,
      can: {
        start: !none && !masked && states.some((s) => s.active !== "active"),
        stop: !none && states.some((s) => s.active === "active"),
        restart: !none && !masked,
        reload: !none && states.some((s) => s.active === "active"),
        enable: !none && states.some((s) => s.enabled === "disabled"),
        disable: !none && states.some((s) => s.enabled === "enabled"),
        "reset-failed": !none && states.some((s) => s.active === "failed"),
        mask: !none && !masked,
        unmask: masked,
        "daemon-reload": true,
      } as Record<ServiceAction, boolean>,
    };
  };
  const chosen = targets.filter((id) => hasUnit(id));
  const here = applies(unit, chosen);

  const act = async (action: ServiceAction, u: string | null = unit, on: string[] = chosen) => {
    if (!u || !on.length) return;
    setBusy(true);
    const started = Date.now();
    try {
      await runServiceAction(on, u, action, { description: svc?.description, origin: winId });
    } finally {
      setBusy(false);
      setActedAt(started);
      await load();
      loadDetail();
    }
  };

  /** `systemctl daemon-reload` on the Apply-to nodes (P1.17): it names no unit. */
  const reloadUnitFiles = async () => {
    setBusy(true);
    try {
      await runServiceAction(targets, "", "daemon-reload", { origin: winId });
    } finally {
      setBusy(false);
      await load();
    }
  };

  const journalInTerminal = (u: string, n: string) => {
    openTerminal(n, { kind: "journal", unit: u });
    wm.open("terminal");
  };

  /** The same actions as the toolbar, for a row's context menu (on the selected node, or where it's installed). */
  const rowMenu = (r: MatrixRow, e: MouseEvent) => {
    const on = r.unit === unit && chosen.length ? chosen : [node && r.cells[node] ? node : shownNodes.find((id) => r.cells[id])!].filter(Boolean);
    const a = applies(r.unit, on);
    const where = on.map(name).join(", ");
    const item = (x: ServiceAction, icon?: typeof Play): MenuItem => ({
      label: verb(x), icon, hint: !admin ? "Admin" : undefined,
      disabled: a.can[x] ? false : `${stateReason(x, on.filter((id) => r.cells[id]).map((id) => r.cells[id]!))} on ${where}`,
      run: () => {
        select(r.unit, on[0]);
        void act(x, r.unit, on);
      },
    });
    showContextMenu(e, [
      item("start", Play), item("stop", StopIcon), item("restart", RotateCcw),
      { separator: true },
      item("enable", Power), item("disable", PowerOff),
      { separator: true },
      item("reload"), item("reset-failed"), a.masked ? item("unmask") : item("mask"),
      { separator: true },
      { label: "Open journal in Terminal", icon: FileText, disabled: on.length ? false : "Not installed here", run: () => journalInTerminal(r.unit, on[0]) },
    ]);
  };

  const nodeColumns: Column<MatrixRow>[] = shownNodes.map((id) => ({
    key: `n:${id}`,
    label: name(id),
    short: shortNode(name(id)),
    width: scope === "all" ? 118 : 150,
    priority: 1 as const,
    sort: (r: MatrixRow) => cellWord(r.cells[id]),
    title: (r: MatrixRow) => {
      const s = r.cells[id];
      if (!s) return `${r.unit} isn't installed on ${name(id)}`;
      const idle = isIdle(s) ? ` · ${idleWords(s)}${s.triggeredBy?.length ? ` (${s.triggeredBy.join(", ")})` : ""}` : "";
      return `${r.unit} on ${name(id)}: ${s.active} (${s.sub})${idle} · ${s.enabled || "?"} at boot`;
    },
    render: (r: MatrixRow) => {
      const s = r.cells[id];
      if (!s) return <span className="muted">—</span>;
      const on = unit === r.unit && node === id;
      const tone = cellTone(s, Object.values(r.cells), failTone(health, id, r.unit));
      const word = cellWord(s);
      return (
        <button type="button" tabIndex={-1} className={`ops-cell svc-cell ${tone}${on ? " on" : ""}`} onClick={(e) => { e.stopPropagation(); select(r.unit, id); }} aria-label={`${r.unit} on ${name(id)}: ${word}`}>
          {tone === "quiet"
            ? <><Icon icon={Check} size={12} className="svc-check" /><span className="svc-word">{scope === "all" ? "" : word}</span></>
            : tone === "idle"
              ? <StatusDot status="unconfigured" text={scope === "all" ? "idle" : idleWords(s)} />
              : <StatusDot status={tone === "crit" ? "error" : tone === "warn" ? "warn" : s.load === "masked" ? "offline" : ACTIVE_DOT[s.active] ?? "offline"} text={word} />}
          {scope !== "all" && <span className="muted"> · {s.enabled || "?"}</span>}
        </button>
      );
    },
  }));
  const columns: Column<MatrixRow>[] = [
    { key: "unit", label: "Service", priority: 1, minWidth: 160, sort: (r) => `${r.rank}${r.unit}`, title: (r) => r.unit, render: (r) => <span className="strong">{r.unit.replace(/\.service$/, "")}</span> },
    ...nodeColumns,
    { key: "desc", label: "Description", priority: 3, minWidth: 160, title: (r) => r.description, render: (r) => <span className="muted">{r.description}</span> },
  ];

  // The selected unit's failure tone (a warning unless its alert is critical).
  const tone: FailTone = unit && node ? failTone(health, node, unit) : "warn";
  const toneText = tone === "crit" ? "crit-text" : "warn-text";
  const failedHere = svc?.active === "failed" || (!!detail?.result && detail.result !== "success");
  const since = fmtSystemdTime(detail?.activeSince?.split(";")[0]);
  const zone = nodeZone(detail?.activeSince);
  const now = Date.now();
  const lineFocus = detail ? focusLine(detail.journal) : -1;
  const wrap = wrapChoice ?? (lineFocus >= 0 && detail!.journal[lineFocus].priority <= 3);
  /** Why an action can't run on the chosen nodes right now, in words that name the state. */
  const why = (a: ServiceAction): string | null => {
    if (!unit) return "Select a service";
    if (!chosen.length) return "Pick a node under Apply to";
    if (busy) return "Busy with the last action";
    return here.can[a] ? null : stateReason(a, unit ? chosen.filter((id) => hasUnit(id)).map((id) => stateOn(id, unit)) : []);
  };
  const moreItems = (): MenuItem[] => [
    { label: verb("reload"), hint: !admin ? "Admin" : undefined, disabled: why("reload") ?? false, run: () => void act("reload") },
    { label: verb("reset-failed"), hint: !admin ? "Admin" : undefined, disabled: why("reset-failed") ?? false, run: () => void act("reset-failed") },
    { label: verb("mask"), hint: !admin ? "Admin" : undefined, disabled: why("mask") ?? false, run: () => void act("mask") },
    { label: verb("unmask"), hint: !admin ? "Admin" : undefined, disabled: why("unmask") ?? false, run: () => void act("unmask") },
    { separator: true },
    {
      label: verb("daemon-reload"), locked: !admin,
      disabled: cluster?.demo ? "Available with Live data" : !targets.length ? "Pick a node under Apply to" : busy ? "Busy with the last action" : false,
      run: () => void reloadUnitFiles(),
    },
    { separator: true },
    { label: "Open journal in Terminal", icon: FileText, disabled: svc && node ? false : "Select a service", run: () => svc && node && journalInTerminal(svc.unit, node) },
  ];

  return (
    <div className="col fill ops-window services-window">
      <Toolbar label="Services">
        <select value={scope} onChange={(e) => setScope(e.target.value)} aria-label="Nodes">
          <option value="all">All nodes</option>
          {nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}
        </select>
        <ToolSeparator />
        <ToolButton icon={Play} label={verb("start")} title={why("start") ?? undefined} disabled={!!why("start")} locked={!admin} onClick={() => act("start")} />
        <ToolButton icon={StopIcon} label={verb("stop")} title={why("stop") ?? undefined} disabled={!!why("stop")} locked={!admin} onClick={() => act("stop")} />
        <ToolButton icon={RotateCcw} label={verb("restart")} title={why("restart") ?? undefined} disabled={!!why("restart")} locked={!admin} onClick={() => act("restart")} />
        <ToolButton icon={Power} label={verb("enable")} priority={2} title={why("enable") ?? "Start at boot"} disabled={!!why("enable")} locked={!admin} onClick={() => act("enable")} />
        <ToolButton icon={PowerOff} label={verb("disable")} priority={2} title={why("disable") ?? "Don't start at boot"} disabled={!!why("disable")} locked={!admin} onClick={() => act("disable")} />
        <HelpTip topic="ops.services.enable-disable" />
        <button type="button" className="tool" disabled={!unit} aria-haspopup="menu" title="Reload, reset failed, mask, unmask, reload unit files, journal"
          onClick={(e) => showContextMenu(e, moreItems())}>
          <span className="tool-label">More</span><Icon icon={ChevronDown} size={12} />
        </button>
        <HelpTip topic="ops.services.more" />
        <Spacer />
        <Segmented<Show> value={show} onChange={setShow} label="Show" options={[
          { value: "running", label: "Active" },
          { value: "failed", label: `Failed${failed.length ? ` (${failed.length})` : ""}` },
          { value: "all", label: "All" },
        ]} />
        <SearchField value={text} onChange={setText} placeholder="Filter services" count={rows.length} />
      </Toolbar>
      {failed.length > 0 && show !== "failed" && (
        // The tone of the unit-failed alerts it mirrors (top bar, Alerts, sidebar badge): amber unless one is critical.
        <Notice tone={failed.some((f) => f.tone === "crit") ? "error" : "warn"} action={{ label: "Show failed", onClick: () => setShow("failed") }}>
          {failed.length === 1
            ? `${failed[0].unit} has failed on ${name(failed[0].node)}.`
            : `${failed.length} services have failed: ${failed.slice(0, 3).map((f) => `${f.unit.replace(/\.service$/, "")} on ${name(f.node)}`).join(", ")}${failed.length > 3 ? "…" : ""}.`}
        </Notice>
      )}
      {unreachable.length > 0 && <Notice tone="warn">Couldn't read services on {unreachable.map((id) => `${name(id)} (${lists[id]})`).join("; ")}.</Notice>}
      {!inTauri && !loading && rows.length === 0 ? (
        <EmptyState icon={Power} title="No services" body="systemd services on every node appear here, failed ones first." />
      ) : (
        <div className={`ops-split svc-split${unit ? "" : " no-detail"}`}>
          <div className="ops-split-list">
            <Table
              key={scope}
              tableId={scope === "all" ? "services-matrix" : "services-node"}
              ariaLabel="Services"
              columns={columns}
              rows={rows}
              rowKey={(r) => r.unit}
              selected={unit}
              onSelect={(u) => select(u)}
              onActivate={(r) => {
                select(r.unit);
                detailRef.current?.focus({ preventScroll: true });
              }}
              onRowContextMenu={(r, e) => rowMenu(r, e)}
              initialSort={{ key: "unit", dir: 1 }}
              rowClass={(r) => (r.rank === 0 ? `row-${rowTone(r)}` : undefined)}
              footer
              itemNoun={["service", "services"]}
              empty={loading ? "Reading systemd on every node…" : "No services match."}
            />
          </div>
          <div className="ops-split-detail" ref={detailRef} tabIndex={-1} aria-label="Service status">
            {!unit ? (
              <div className="muted pad">Select a service, or a node's cell in its row, to see its status and journal.</div>
            ) : (
              <div className="ops-detail">
                <div className="ops-detail-head">
                  <h3 className="ops-detail-title">{unit}</h3>
                  {svc && (isIdle(svc)
                    ? <StatusDot status="unconfigured" text={idleWords(svc)} />
                    : <StatusDot status={svc.active === "failed" ? tone : ACTIVE_DOT[svc.active] ?? "offline"} text={cellWord(svc)} />)}
                </div>
                {(detail?.description || svc?.description) && <div className="muted">{detail?.description || svc?.description}</div>}
                <div className="ops-targets" role="group" aria-label="Apply to">
                  <span className="muted">Apply to</span>
                  {shownNodes.filter((id) => hasUnit(id)).map((id) => {
                    const on = targets.includes(id);
                    return (
                      <button key={id} type="button" className={`chip${on ? " on" : ""}`} aria-pressed={on}
                        onClick={() => setTargets(on ? targets.filter((t) => t !== id) : [...targets, id])}>
                        {name(id)}
                      </button>
                    );
                  })}
                  {chosen.length > 1 && (
                    <HelpTip topic="ops.services.several-nodes" label="About running on several nodes" />
                  )}
                </div>
                {detailError && <Notice tone="error">{detailError}</Notice>}
                {node && detail && (
                  <>
                    <div className="section-title">On {name(node)}</div>
                    <dl className="ops-kv">
                      <div><dt>State</dt><dd>{detail.active ?? "—"}</dd></div>
                      <div><dt>Since</dt><dd title={since.utc || undefined}>{since.ms != null ? `${since.local} (${since.relative})` : "—"}</dd></div>
                      <div><dt>Main PID</dt><dd className="num">{detail.mainPid ?? "—"}</dd></div>
                      <div><dt>Result</dt><dd className={detail.result && detail.result !== "success" ? toneText : undefined}>{detail.result ?? "—"}</dd></div>
                      {lineFocus >= 0 && detail.journal[lineFocus].priority <= 3 && (
                        <div><dt>Error</dt><dd className={`${failedHere ? toneText : "warn-text"} svc-why`} title={`${detail.journal[lineFocus].text}\n${journalTime(detail.journal[lineFocus], now, zone).title}`}>{detail.journal[lineFocus].text}</dd></div>
                      )}
                      <div><dt>Memory</dt><dd>{detail.memory ?? "—"}</dd></div>
                      <div><dt>CPU time</dt><dd>{detail.cpu ?? "—"}</dd></div>
                      {detail.tasks && <div><dt>Tasks</dt><dd>{detail.tasks}</dd></div>}
                      <div><dt>Unit file</dt><dd className="mono">{detail.loaded ?? "—"}</dd></div>
                    </dl>
                    <div className="svc-journal-head">
                      <div className="section-title">Journal · last {detail.journal.length} lines</div>
                      <HelpTip topic="ops.services.journal" />
                      <label className="check svc-wrap" title="Wrap long lines instead of scrolling sideways (on by default when there's an error)">
                        <input type="checkbox" checked={wrap} onChange={(e) => setWrap(e.target.checked)} /> Wrap
                      </label>
                    </div>
                    {detail.journal.length ? (
                      <div className={`svc-journal-scroll${wrap ? " wrap" : ""}`}>
                        <ol className="ops-journal svc-journal" aria-label="Journal" ref={journalRef}>
                          {detail.journal.map((j, i) => {
                            const t = journalTime(j, now, zone);
                            const fresh = actedAt != null && j.tsMs != null && j.tsMs >= actedAt - 1000;
                            // Error lines of a failed unit read in its alert's tone; of a running one, amber.
                            const errTone = failedHere ? tone : "warn";
                            return (
                              <li
                                key={i}
                                className={`${j.priority <= 3 ? `err tone-${errTone}` : j.priority === 4 ? "warning" : ""}${fresh ? " fresh" : ""}`}
                                data-focus-line={i === lineFocus || undefined}
                                title={j.text}
                              >
                                <span className="ops-journal-time" title={t.title}>{t.text}</span>
                                {j.priority <= 3 && <Badge tone={errTone}>error</Badge>}
                                {j.priority === 4 && <Badge tone="warn">warning</Badge>}
                                <span className="ops-journal-text">{j.text}</span>
                              </li>
                            );
                          })}
                        </ol>
                      </div>
                    ) : <div className="muted">No journal entries.</div>}
                    <details className="ops-raw">
                      <summary>systemctl status</summary>
                      <CodeBlock text={detail.statusText} maxHeight={260} label="systemctl status" />
                    </details>
                  </>
                )}
                {node && !detail && !detailError && <div className="muted">Reading status…</div>}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
