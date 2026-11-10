// Fleet Command: one shell command on several nodes, like pdsh with dshbak. Output streams in
// per node; when they're done, identical answers merge into one card ("cell1, cell2, cell3 ·
// exit 0") and the odd ones out are marked. The time limit is enforced on the node itself.
// Run classifies the exact command first (power.ts `confirmFleet`, failing closed): reads run
// at once, changes show what's on the targets, and anything destructive runs on one node
// first and asks before the rest, which then go one at a time.
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Copy, History, Lock, Pin, PinOff, Play, RotateCcw, Save, Square, Table2, WrapText, X } from "lucide-react";
import { Icon } from "../components/Icon";
import { VersionComparison } from "../components/VersionComparison";
import { Badge, HelpTip, LockChip, Notice, Segmented, Spacer, StatusDot, Switch, Toolbar, ToolButton } from "../components/ui";
import { ADMIN_REFUSED, lockOf, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, useAdminState, withAdmin } from "../lib/admin";
import { currentSource, migrateLegacy, scopedKey, useDataSource } from "../lib/dataSource";
import { inTauri } from "../lib/api";
import { showError } from "../lib/errors";
import { copyText } from "../lib/markdown";
import { opsApi, type Classified, type FleetNodeEvent, type FleetNodeState, type FleetOutputEvent, type FleetStrategy } from "../lib/opsApi";
import { confirmCanaryRest, cutsAccess, requestFleetRun, requestPower, type FleetRequest } from "../lib/power";
import { REDACTED, redact } from "../lib/redact";
import { useCluster } from "../lib/store";
import { waitTask, type FleetRerun } from "../lib/tasks";
import type { NodeSnapshot } from "../lib/types";
import { pushUndo } from "../lib/undo";
import { useWinState, type Params } from "../wm/WindowManager";
import { relTime } from "./TasksWindow";
import "./ops.css";

export const BUILT_IN = [
  { name: "GPU", command: "nvidia-smi --query-gpu=name,driver_version,utilization.gpu,temperature.gpu,power.draw --format=csv,noheader" },
  { name: "Uptime", command: "uptime" },
  { name: "Disk", command: "df -h / /home 2>/dev/null" },
  { name: "Memory", command: "free -h" },
  { name: "Containers", command: "docker ps --format 'table {{.Names}}\\t{{.Image}}\\t{{.Status}}'" },
  { name: "Updates", command: "apt list --upgradable 2>/dev/null" },
  { name: "Kernel", command: "uname -srm" },
  { name: "Fabric links", command: "for i in /sys/class/net/enp1s0f*np* /sys/class/net/enP2p1s0f*np*; do [ -e $i ] && s=$(cat $i/operstate) && echo \"$(basename $i) $s$([ \"$s\" = up ] && echo \" $(cat $i/speed)Mb/s\")\"; done" },
];

const SNIPPETS_KEY = "sparkbox.fleet.snippets";
/** Kept per data source (scopedKey): Demo runs never show in Live's history. */
const HISTORY_KEY = "sparkbox.fleet.history";
const HISTORY_MAX = 50;
// Runs saved before history was kept per source were Demo ones (Live was first used on Oct 29).
migrateLegacy(HISTORY_KEY);

interface Snippet { name: string; command: string }

export interface HistoryItem {
  command: string;
  sudo: boolean;
  nodes: string[];
  failed: string[];
  total: number;
  tsMs: number;
}

function load<T>(key: string, fallback: T): T {
  try {
    const v = JSON.parse(localStorage.getItem(key) ?? "null");
    return v ?? fallback;
  } catch {
    return fallback;
  }
}

function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Private mode: kept for this session only.
  }
}

/** The saved run history of one data source (the current one by default), newest first. */
export function fleetHistory(source: "demo" | "live" = currentSource()): HistoryItem[] {
  return load<HistoryItem[]>(scopedKey(HISTORY_KEY, source), []);
}

/** History entries are kept with anything that looks like a secret hidden. */
export function historyItem(run: { command: string; sudo: boolean; nodes: string[]; startedMs: number }, failed: string[]): HistoryItem {
  return { command: redact(run.command), sudo: run.sudo, nodes: run.nodes, failed, total: run.nodes.length, tsMs: run.startedMs };
}

/** `reboot`, `shutdown`, `systemctl poweroff`…: Restart… and Shut down… do these better. */
export function isPowerVerb(reason: string | null | undefined): boolean {
  return !!reason && /^(Takes the node down|Shuts the node down)/.test(reason);
}

// ---- Output

export interface NodeOutput {
  node: string;
  state: FleetNodeState;
  code: number | null;
  stdout: string;
  stderr: string;
  ms: number | null;
  startedMs: number | null;
  error?: string | null;
  truncated?: boolean;
}

export interface OutputGroup {
  nodes: string[];
  stdout: string;
  stderr: string;
  code: number | null;
  state: FleetNodeState;
  error?: string | null;
  /** Its answer isn't the one most nodes gave. */
  differs: boolean;
}

const DONE: FleetNodeState[] = ["ok", "failed", "timedOut", "skipped", "cancelled"];

/**
 * Merges nodes that gave exactly the same answer (dshbak -c). Groups come largest first;
 * those that aren't the most common answer are marked `differs` (unless every node said
 * something different, which is normal for e.g. hostname).
 */
export function groupOutputs(outputs: NodeOutput[]): OutputGroup[] {
  const groups: OutputGroup[] = [];
  for (const o of outputs) {
    const g = groups.find((g) => g.stdout === o.stdout && g.stderr === o.stderr && g.code === o.code && g.state === o.state && (g.error ?? null) === (o.error ?? null));
    if (g) g.nodes.push(o.node);
    else groups.push({ nodes: [o.node], stdout: o.stdout, stderr: o.stderr, code: o.code, state: o.state, error: o.error ?? null, differs: false });
  }
  groups.sort((a, b) => b.nodes.length - a.nodes.length);
  if (groups.length > 1 && groups[0].nodes.length > 1) {
    for (const g of groups.slice(1)) g.differs = true;
  }
  return groups;
}

// ---- ANSI colours (SGR) to styled spans

export interface AnsiSpan {
  text: string;
  fg?: string;
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

const BASIC = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

function color256(n: number): string {
  if (n < 8) return `var(--ansi-${BASIC[n]})`;
  if (n < 16) return `var(--ansi-bright-${BASIC[n - 8]})`;
  if (n < 232) {
    const v = n - 16;
    const c = (x: number) => (x === 0 ? 0 : 55 + x * 40);
    return `rgb(${c(Math.floor(v / 36))}, ${c(Math.floor(v / 6) % 6)}, ${c(v % 6)})`;
  }
  const g = 8 + (n - 232) * 10;
  return `rgb(${g}, ${g}, ${g})`;
}

/** Splits text with ANSI escapes into styled spans; other escape sequences are dropped. */
export function ansiToSpans(input: string): AnsiSpan[] {
  const out: AnsiSpan[] = [];
  let style: Omit<AnsiSpan, "text"> = {};
  // CSI sequences: ESC [ params final-byte; also bare OSC titles (ESC ] … BEL).
  const re = /\x1b\[([0-9;?]*)([A-Za-z])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][A-Za-z0-9]|\r(?!\n)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  const push = (text: string) => {
    if (!text) return;
    const prev = out[out.length - 1];
    if (prev && JSON.stringify({ ...prev, text: "" }) === JSON.stringify({ ...style, text: "" })) prev.text += text;
    else out.push({ text, ...style });
  };
  while ((m = re.exec(input))) {
    push(input.slice(last, m.index));
    last = re.lastIndex;
    if (m[2] !== "m") continue;
    const codes = (m[1] || "0").split(";").map((c) => Number(c || 0));
    for (let i = 0; i < codes.length; i++) {
      const c = codes[i];
      if (c === 0) style = {};
      else if (c === 1) style = { ...style, bold: true };
      else if (c === 2) style = { ...style, dim: true };
      else if (c === 3) style = { ...style, italic: true };
      else if (c === 4) style = { ...style, underline: true };
      else if (c === 22) style = { ...style, bold: false, dim: false };
      else if (c === 23) style = { ...style, italic: false };
      else if (c === 24) style = { ...style, underline: false };
      else if (c >= 30 && c <= 37) style = { ...style, fg: `var(--ansi-${BASIC[c - 30]})` };
      else if (c >= 90 && c <= 97) style = { ...style, fg: `var(--ansi-bright-${BASIC[c - 90]})` };
      else if (c >= 40 && c <= 47) style = { ...style, bg: `var(--ansi-${BASIC[c - 40]})` };
      else if (c >= 100 && c <= 107) style = { ...style, bg: `var(--ansi-bright-${BASIC[c - 100]})` };
      else if (c === 39) style = { ...style, fg: undefined };
      else if (c === 49) style = { ...style, bg: undefined };
      else if ((c === 38 || c === 48) && codes[i + 1] === 5) {
        const col = color256(codes[i + 2] ?? 0);
        style = c === 38 ? { ...style, fg: col } : { ...style, bg: col };
        i += 2;
      } else if ((c === 38 || c === 48) && codes[i + 1] === 2) {
        const col = `rgb(${codes[i + 2] ?? 0}, ${codes[i + 3] ?? 0}, ${codes[i + 4] ?? 0})`;
        style = c === 38 ? { ...style, fg: col } : { ...style, bg: col };
        i += 4;
      }
    }
  }
  push(input.slice(last));
  return out.map((s) => Object.fromEntries(Object.entries(s).filter(([, v]) => v !== undefined && v !== false)) as AnsiSpan);
}

export function stripAnsi(s: string): string {
  return ansiToSpans(s).map((x) => x.text).join("");
}

function Ansi({ text, className }: { text: string; className?: string }) {
  const spans = useMemo(() => ansiToSpans(redact(text)), [text]);
  return (
    <span className={className}>
      {spans.map((s, i) => (
        <span key={i} style={{ color: s.fg, background: s.bg, fontWeight: s.bold ? 600 : undefined, opacity: s.dim ? 0.7 : undefined, fontStyle: s.italic ? "italic" : undefined, textDecoration: s.underline ? "underline" : undefined }}>
          {s.text}
        </span>
      ))}
    </span>
  );
}

// ---- CSV as a table

/** Splits one CSV line (quotes allowed). */
export function csvCells(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let q = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (q) {
      if (ch === '"' && line[i + 1] === '"') { cur += '"'; i++; } else if (ch === '"') q = false; else cur += ch;
    } else if (ch === '"') q = true;
    else if (ch === ",") { out.push(cur.trim()); cur = ""; } else cur += ch;
  }
  out.push(cur.trim());
  return out;
}

/** Rows when every node printed CSV with the same number of columns (nvidia-smi --format=csv). */
export function csvTable(outputs: NodeOutput[]): { rows: { node: string; cells: string[] }[]; width: number } | null {
  const rows: { node: string; cells: string[] }[] = [];
  let width = 0;
  for (const o of outputs) {
    const lines = stripAnsi(o.stdout).split("\n").filter((l) => l.trim());
    if (!lines.length) return null;
    for (const l of lines) {
      const cells = csvCells(l);
      if (cells.length < 2) return null;
      if (width && cells.length !== width) return null;
      width = cells.length;
      rows.push({ node: o.node, cells });
    }
  }
  return rows.length ? { rows, width } : null;
}

/** Copy all as Markdown: one heading and code block per group. */
export function asMarkdown(command: string, groups: OutputGroup[], name: (id: string) => string): string {
  const parts = [`\`$ ${command}\``, ""];
  for (const g of groups) {
    parts.push(`### ${g.nodes.map(name).join(", ")} · ${stateWord(g.state, g.code)}`, "", "```", stripAnsi(g.stdout + (g.stderr ? `\n${g.stderr}` : "")).trimEnd() || "(no output)", "```", "");
  }
  return parts.join("\n");
}

function stateWord(state: FleetNodeState, code: number | null): string {
  switch (state) {
    case "ok": return "exit 0";
    case "failed": return code !== null ? `exit ${code}` : "couldn't run";
    case "timedOut": return "stopped at the time limit";
    case "skipped": return "skipped";
    case "cancelled": return "canceled";
    case "running": return "running";
    default: return "waiting";
  }
}

const STATE_TONE: Record<FleetNodeState, "good" | "crit" | "warn" | "neutral" | "info"> = {
  queued: "neutral", running: "info", ok: "good", failed: "crit", timedOut: "warn", skipped: "neutral", cancelled: "neutral",
};

function fmtSecs(ms: number): string {
  return ms < 10_000 ? `${(ms / 1000).toFixed(1)} s` : `${Math.round(ms / 1000)} s`;
}

// ---- Versions: backend verdict shared with multi-node preflight.
export function Versions({ nodes, report, demo }: { nodes: NodeSnapshot[]; report?: import("../lib/types").VersionReport | null; demo?: boolean }) {
  return <div className="fleet-versions"><VersionComparison report={report} nodes={nodes} demo={demo} /></div>;
}

// ---- The window

interface Run {
  /** The run on screen now; a canary first is a run of its own. */
  id: string | null;
  ids: string[];
  command: string;
  sudo: boolean;
  nodes: string[];
  startedMs: number;
  outputs: Record<string, NodeOutput>;
  finished: boolean;
}

export function FleetCommand({ params }: { params?: Params } = {}) {
  const cluster = useCluster();
  const admin = useAdminState();
  const nodes = cluster?.nodes ?? [];
  const name = (id: string) => nodes.find((n) => n.id === id)?.name ?? id;
  const [picked, setPicked] = useState<string[] | null>(null);
  const [command, setCommand] = useWinState("command", "uptime");
  const [sudo, setSudo] = useState(false);
  const [keepSudo, setKeepSudo] = useWinState("keepSudo", false);
  const [timeout, setTimeoutS] = useWinState("timeout", "60");
  const [strategy, setStrategy] = useWinState<FleetStrategy>("strategy", "parallel");
  const [stopOnFailure, setStopOnFailure] = useWinState("stopOnFailure", true);
  const [wrap, setWrap] = useWinState("wrap", true);
  const [perNode, setPerNode] = useWinState("perNode", false);
  const [asTable, setAsTable] = useState(false);
  const [run, setRun] = useState<Run | null>(null);
  const [snippets, setSnippets] = useState<Snippet[]>(() => load(SNIPPETS_KEY, []));
  const source = useDataSource();
  const [history, setHistory] = useState<HistoryItem[]>(() => fleetHistory());
  useEffect(() => setHistory(fleetHistory(source)), [source]);
  const [showHistory, setShowHistory] = useState(false);
  const [klass, setKlass] = useState<Classified | null>(null);
  /** The snippet the command came from, while it's unchanged: the run is titled with its name. */
  const [snippet, setSnippet] = useState<Snippet | null>(null);
  const [view, setView] = useWinState<"run" | "versions">("view", "run");
  const [now, setNow] = useState(() => Date.now());
  const recall = useRef(-1);
  const runRef = useRef<Run | null>(null);
  const pending = useRef(new Map<string, (FleetOutputEvent | FleetNodeEvent)[]>());
  runRef.current = run;

  // Default targets: the nodes that are online once the first snapshot arrives.
  useEffect(() => {
    if (picked === null && nodes.length) setPicked(nodes.filter((n) => n.status === "online").map((n) => n.id));
  }, [nodes.length, picked]);
  const targets = picked ?? [];

  // What the command does, as you type.
  useEffect(() => {
    const c = command.trim();
    if (!c) return setKlass(null);
    const t = window.setTimeout(() => {
      if (inTauri) opsApi.classify(c).then(setKlass, () => setKlass(null));
    }, 150);
    return () => window.clearTimeout(t);
  }, [command]);

  // Streamed output and node states, for the run on screen.
  useEffect(() => {
    if (!inTauri) return;
    const apply = (e: FleetOutputEvent | FleetNodeEvent) => {
      const r = runRef.current;
      if (!r || !r.ids.includes(e.runId)) {
        const buf = pending.current.get(e.runId) ?? [];
        if (buf.length < 2000) buf.push(e);
        pending.current.set(e.runId, buf);
        return;
      }
      setRun((cur) => (cur && cur.ids.includes(e.runId) ? applyEvent(cur, e) : cur));
    };
    // A listener that arrives after this effect was torn down is removed at once; otherwise a
    // remount (React's strict mode does one) would leave two, and every chunk would show twice.
    const stops: (() => void)[] = [];
    let gone = false;
    const keep = (u: () => void) => (gone ? u() : stops.push(u));
    opsApi.onFleetOutput(apply).then(keep, () => {});
    opsApi.onFleetNode(apply).then(keep, () => {});
    return () => {
      gone = true;
      stops.forEach((s) => s());
    };
  }, []);

  const running = !!run && !run.finished;
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(t);
  }, [running]);

  // When every node is done: take the final results, note it in the history.
  const recorded = useRef<string | null>(null);
  useEffect(() => {
    if (!run || run.finished || !run.id || recorded.current === run.id) return;
    const all = run.nodes.every((n) => DONE.includes(run.outputs[n]?.state));
    if (!all) return;
    const id = run.id;
    recorded.current = id;
    opsApi.fleetResults(id).then((results) => {
      setRun((cur) => {
        if (!cur || cur.id !== id) return cur;
        const outputs = { ...cur.outputs };
        for (const r of results ?? []) {
          outputs[r.node] = { ...outputs[r.node], node: r.node, stdout: r.stdout, stderr: r.stderr || (r.error && r.state !== "skipped" ? r.error : ""), code: r.code, state: r.state, ms: r.ms, error: r.error, truncated: r.truncated };
        }
        return { ...cur, outputs, finished: true };
      });
    }, () => setRun((cur) => (cur && cur.id === id ? { ...cur, finished: true } : cur)));
    const failed = run.nodes.filter((n) => run.outputs[n]?.state !== "ok");
    const item = historyItem(run, failed);
    setHistory((h) => {
      const next = [item, ...h].slice(0, HISTORY_MAX);
      save(scopedKey(HISTORY_KEY), next);
      return next;
    });
    if (!keepSudo) setSudo(false);
  }, [run]);

  const start = async (cmd: string, on: string[], asRoot: boolean, opts: { strategy?: FleetStrategy; stopOnFailure?: boolean; title?: string | null; timeoutS?: number } = {}) => {
    cmd = cmd.trim();
    // Look-only: nothing runs, from the button, ⌘↩ or a Re-run.
    if (!cmd || !on.length || running || lookOnly) return;
    const order = nodes.map((n) => n.id).filter((id) => on.includes(id));
    const req: FleetRequest = {
      nodes: order, command: cmd, sudo: asRoot, timeoutS: opts.timeoutS ?? Math.max(1, Number(timeout) || 60),
      strategy: opts.strategy ?? strategy, stopOnFailure: opts.stopOnFailure ?? stopOnFailure,
      title: opts.title !== undefined ? opts.title : snippet && snippet.command === cmd ? snippet.name : null,
    };
    // Classifies the exact command, confirms on its tier and unlocks; nothing runs before that.
    const started = await requestFleetRun(req, name);
    if (!started) return;
    const { id, plan } = started;
    const startedMs = Date.now();
    const queued = (n: string): NodeOutput => ({ node: n, state: "queued", code: null, stdout: "", stderr: "", ms: null, startedMs: null });
    const fresh: Run = { id, ids: [id], command: cmd, sudo: asRoot, nodes: order, startedMs, finished: false, outputs: Object.fromEntries(order.map((n) => [n, queued(n)])) };
    setRun(() => {
      let next = fresh;
      for (const e of pending.current.get(id) ?? []) next = applyEvent(next, e);
      pending.current.delete(id);
      return next;
    });
    setAsTable(false);
    recall.current = -1;
    if (!plan.canary) return;

    // Canary: the first node alone; the rest only if it worked and you say so.
    const [first, ...rest] = order;
    const skipRest = (why: string) => setRun((cur) => (cur && cur.startedMs === startedMs
      ? { ...cur, outputs: { ...cur.outputs, ...Object.fromEntries(rest.map((n) => [n, { ...cur.outputs[n], state: "skipped" as FleetNodeState, error: why }])) } }
      : cur));
    const canary = await waitTask(id);
    if (canary.state !== "ok") return skipRest(`Not run: the canary (${name(first)}) failed`);
    if (!(await confirmCanaryRest(first, rest, name, !!plan.cutsAccess))) return skipRest("Not run: you stopped after the canary");
    try {
      const second = await opsApi.fleetStart({ nodes: rest, command: cmd, sudo: asRoot, timeoutS: plan.timeoutS, strategy: "oneByOne", stopOnFailure: true, title: plan.title ?? null, cutsAccess: !!plan.cutsAccess });
      setRun((cur) => {
        if (!cur || cur.startedMs !== startedMs) return cur;
        let next: Run = { ...cur, id: second, ids: [...cur.ids, second] };
        for (const e of pending.current.get(second) ?? []) next = applyEvent(next, e);
        pending.current.delete(second);
        return next;
      });
    } catch (e) {
      skipRest(String((e as { message?: string })?.message ?? e));
      showError(e, { action: "Run the command on the other nodes" });
    }
  };

  // Retry from the Tasks window: the same run, through the same tier.
  const rerunSeen = useRef<string | null>(null);
  const rerun = (r: FleetRerun, seq: string) => {
    if (rerunSeen.current === seq) return;
    rerunSeen.current = seq;
    setCommand(r.command);
    setPicked(r.nodes);
    setSudo(r.sudo);
    void start(r.command, r.nodes, r.sudo, { strategy: r.strategy, stopOnFailure: r.stopOnFailure, title: r.title, timeoutS: r.timeoutS });
  };
  useEffect(() => {
    if (!params?.rerun || !params.seq) return;
    try {
      rerun(JSON.parse(params.rerun) as FleetRerun, params.seq);
    } catch {
      // A broken parameter: nothing to run.
    }
  }, [params?.seq, nodes.length > 0]);
  useEffect(() => {
    const on = (e: Event) => {
      const d = (e as CustomEvent<{ rerun: FleetRerun; seq: string }>).detail;
      if (d?.rerun) rerun(d.rerun, d.seq);
    };
    window.addEventListener("sparkbox:fleet-rerun", on);
    return () => window.removeEventListener("sparkbox:fleet-rerun", on);
  });

  // Stopping needs Admin mode like starting did (it relocks if left long enough).
  const cancel = () => run?.id && withAdmin(() => opsApi.fleetCancel(run.id!), { action: "fleet_start" }).catch((e) => e !== ADMIN_REFUSED && showError(e, { action: "Stop the run" }));

  const commands = useMemo(() => [...new Set(history.map((h) => h.command))], [history]);
  const outputs = run ? run.nodes.map((n) => run.outputs[n]) : [];
  const doneOutputs = outputs.filter((o) => DONE.includes(o.state));
  const live = outputs.filter((o) => !DONE.includes(o.state));
  const groups = perNode ? doneOutputs.map((o) => ({ nodes: [o.node], stdout: o.stdout, stderr: o.stderr, code: o.code, state: o.state, error: o.error, differs: false })) : groupOutputs(doneOutputs);
  const table = run?.finished ? csvTable(doneOutputs.filter((o) => o.state === "ok")) : null;
  const okCount = outputs.filter((o) => o.state === "ok").length;
  const failedNodes = outputs.filter((o) => ["failed", "timedOut", "cancelled"].includes(o.state)).map((o) => o.node);

  const badge = !command.trim() ? null : klass?.class === "destructive" && klass.power
    // A power verb takes the nodes down; it isn't a "cuts access" command of its own (R1-K).
    ? <Badge tone="crit" title={klass.reason ?? "Restart… and Shut down… show what goes down and follow it back"}>takes nodes down</Badge>
    : klass && klass.class === "destructive" && cutsAccess(klass, targets.length)
    ? <Badge tone="crit" title={klass.reason ?? "SparkBox may lose its connection to these nodes"}>cuts access</Badge>
    : klass?.class === "destructive"
    ? <Badge tone="crit" title={klass.reason ?? "It can destroy data or take nodes down"}>destructive</Badge>
    : sudo || klass?.escalates
      ? <Badge tone="warn" title={klass?.root && !sudo ? "Docker is root on the node: this runs with root's power without sudo" : "Runs with sudo, using the cluster password from the Keychain"}>runs as root</Badge>
      : klass?.class === "change"
        ? <Badge tone="warn" title="Not on SparkBox's list of read-only commands, so it may change something">changes things</Badge>
        : klass ? <Badge tone="neutral" title="Every part of it only reads">read-only</Badge> : null;
  const power = isPowerVerb(klass?.reason);
  // Every run needs Admin (fleet_start): locked, Run asks to unlock; look-only, it's off.
  const lock = lockOf(!admin.unlocked, admin);
  const lookOnly = lock === "lookOnly";

  const forget = (s: Snippet) => {
    const before = snippets;
    const next = snippets.filter((x) => x !== s);
    setSnippets(next);
    save(SNIPPETS_KEY, next);
    pushUndo({ label: `Forgot “${s.name}”`, undo: () => { setSnippets(before); save(SNIPPETS_KEY, before); } });
  };

  return (
    <div className="col fill ops-window fleet">
      <Toolbar label="Fleet Command">
        <Segmented<"run" | "versions"> value={view} onChange={setView} label="View" options={[
          { value: "run", label: "Run" },
          { value: "versions", label: "Versions", title: "What each node runs, with differences marked" },
        ]} />
        {view === "run" && <HelpTip topic="run-a-command-on-every-node" />}
        {lock && <LockChip />}
        {view === "run" && <>
        <span className="toolbar-text fleet-on">on</span>
        <div className="fleet-targets" role="group" aria-label="Nodes">
          {nodes.map((n) => {
            const o = run?.outputs[n.id];
            const on = targets.includes(n.id);
            return (
              <button key={n.id} type="button" className={`chip fleet-node${on ? " on" : ""}`} aria-pressed={on} disabled={running}
                title={`${n.name}: ${n.status}${o ? ` · ${stateWord(o.state, o.code)}` : ""}`}
                onClick={() => setPicked(on ? targets.filter((p) => p !== n.id) : [...targets, n.id])}>
                <StatusDot status={n.status} />
                {n.name}
                {o && run?.nodes.includes(n.id) && (
                  <span className={`fleet-node-state ${o.state}`}>
                    {o.state === "running" && o.startedMs ? fmtSecs(now - o.startedMs) : o.state === "queued" ? "waiting" : o.ms !== null && o.state !== "skipped" ? `${stateWord(o.state, o.code)} · ${fmtSecs(o.ms)}` : stateWord(o.state, o.code)}
                  </span>
                )}
              </button>
            );
          })}
        </div>
        <Spacer />
        <Segmented<FleetStrategy> value={strategy} onChange={setStrategy} label="How" options={[
          { value: "parallel", label: "Parallel", title: "Every node at once" },
          { value: "oneByOne", label: "One at a time", title: "Node by node, in order" },
          { value: "canary", label: "Canary first", title: "The first node alone; the rest only if it worked" },
        ]} />
        <HelpTip topic="ops.fleet.how" />
        <Switch checked={stopOnFailure} onChange={setStopOnFailure} label="Stop if a node fails" showLabel disabled={strategy === "parallel"} />
        <HelpTip topic="ops.fleet.stop-on-failure" />
        </>}
      </Toolbar>
      {view === "versions" ? <Versions nodes={nodes} report={cluster?.versions} demo={!!cluster?.demo} /> : <>

      <div className={`fleet-input${sudo ? " root" : ""}`}>
        <span className="fleet-prompt mono" aria-hidden>{sudo ? "#" : "$"}</span>
        <textarea
          className="mono"
          rows={2}
          value={command}
          spellCheck={false}
          aria-label="Command"
          data-autofocus
          placeholder="A shell command, e.g. nvidia-smi"
          onChange={(e) => setCommand(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void start(command, targets, sudo);
              return;
            }
            const el = e.currentTarget;
            const atTop = !el.value.slice(0, el.selectionStart).includes("\n");
            const atEnd = !el.value.slice(el.selectionEnd).includes("\n");
            if (e.key === "ArrowUp" && atTop && commands.length) {
              e.preventDefault();
              recall.current = Math.min(commands.length - 1, recall.current + 1);
              setCommand(commands[recall.current]);
            } else if (e.key === "ArrowDown" && atEnd && recall.current >= 0) {
              e.preventDefault();
              recall.current -= 1;
              setCommand(recall.current >= 0 ? commands[recall.current] : "");
            }
          }}
        />
        <div className="fleet-side">
          <div className="fleet-side-row">
            {badge}
            <label className="check" title={lookOnly ? LOOK_ONLY_TIP : "Runs the command with sudo, using the cluster password from the Keychain"}>
              <input type="checkbox" checked={sudo} disabled={lookOnly} onChange={(e) => setSudo(e.target.checked)} /> as root
            </label>
            <HelpTip topic="ops.fleet.as-root" />
            <button type="button" className={`tool icon-only${keepSudo ? " on" : ""}`} aria-pressed={keepSudo}
              title={keepSudo ? "“as root” stays on between runs" : "Keep “as root” on after this run"} aria-label="Keep as root"
              onClick={() => setKeepSudo(!keepSudo)}>
              <Icon icon={keepSudo ? Pin : PinOff} size={12} />
            </button>
          </div>
          <div className="fleet-side-row">
            <label className="toolbar-text" htmlFor="fleet-timeout">Time limit</label>
            <input id="fleet-timeout" className="narrow num" type="number" min={1} max={3600} value={timeout} onChange={(e) => setTimeoutS(e.target.value)} />
            <span className="toolbar-text">s</span>
            <HelpTip topic="ops.fleet.time-limit" />
          </div>
          <div className="fleet-side-row">
            {running ? (
              <button type="button" className="btn" onClick={cancel} disabled={!run?.id}><Icon icon={Square} size={12} /> Cancel all</button>
            ) : (
              <button type="button" className="btn primary" disabled={!command.trim() || !targets.length || lookOnly}
                title={lookOnly ? LOOK_ONLY_TIP : lock ? `Run (⌘↩) · ${NEEDS_ADMIN_TIP}` : "Run (⌘↩)"}
                aria-label={lookOnly ? "Run — look-only" : lock ? "Run — needs Admin" : undefined}
                data-lock={lock ?? undefined}
                onClick={() => void start(command, targets, sudo)}>
                <Icon icon={Play} size={12} /> Run <span className="kbd-hint">⌘↩</span>
                {lock && <Icon icon={Lock} size={12} className="tool-lock" />}
              </button>
            )}
            <button type="button" className="btn" disabled={!command.trim()} title="Save as a snippet" onClick={() => {
              const c = command.trim();
              const s = { name: c.split(/\s+/).slice(0, 3).join(" "), command: c };
              const next = [...snippets.filter((x) => x.command !== c), s];
              setSnippets(next);
              save(SNIPPETS_KEY, next);
            }}><Icon icon={Save} size={12} /> Save</button>
          </div>
        </div>
      </div>

      <div className="snippets" role="toolbar" aria-label="Snippets">
        {BUILT_IN.map((s) => <button key={s.name} type="button" className="chip" title={s.command} onClick={() => { setCommand(s.command); setSnippet(s); }}>{s.name}</button>)}
        {snippets.map((s) => (
          <span key={s.command} className="chip saved" title={s.command}>
            <button type="button" className="link-plain" onClick={() => { setCommand(s.command); setSnippet(s); }}>{s.name}</button>
            <button type="button" className="link-plain" aria-label={`Forget ${s.name}`} title="Forget this snippet" onClick={() => forget(s)}><Icon icon={X} size={12} /></button>
          </span>
        ))}
        <HelpTip topic="ops.fleet.snippets" />
        <Spacer />
        <button type="button" className={`chip${showHistory ? " on" : ""}`} aria-pressed={showHistory} onClick={() => setShowHistory(!showHistory)}>
          <Icon icon={History} size={12} /> History{history.length ? ` (${history.length})` : ""}
        </button>
      </div>

      {showHistory && (
        <div className="fleet-history" aria-label="Recent runs">
          {history.length === 0 && <div className="muted pad">Runs you make appear here (the last 50), with the nodes and how many worked.</div>}
          {history.map((h, i) => (
            <div key={i} className="fleet-history-row">
              <span className="mono fleet-history-cmd" title={h.command}>{h.sudo ? "# " : "$ "}{h.command}</span>
              <span className="muted">{h.nodes.map(name).join(", ")}</span>
              <span className={h.failed.length ? "crit-text" : "muted"}>{h.total - h.failed.length}/{h.total} ok</span>
              <span className="muted" title={new Date(h.tsMs).toLocaleString()}>{relTime(h.tsMs, now)}</span>
              <button type="button" className="btn small" disabled={running || lookOnly}
                title={lookOnly ? LOOK_ONLY_TIP : h.command.includes(REDACTED) ? "Part of it was hidden when saved: put it back in the box, then Run" : undefined}
                onClick={() => { setCommand(h.command); if (!h.command.includes(REDACTED)) void start(h.command, h.nodes, h.sudo); }}>
                <Icon icon={RotateCcw} size={12} /> Re-run
              </button>
              {h.failed.length > 0 && !h.command.includes(REDACTED) && (
                <button type="button" className="btn small" disabled={running || lookOnly} title={lookOnly ? LOOK_ONLY_TIP : undefined} onClick={() => { setCommand(h.command); void start(h.command, h.failed, h.sudo); }}>
                  Re-run on failed ({h.failed.length})
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {!targets.length && nodes.length > 0 && !run && <Notice tone="info">Pick at least one node to run on.</Notice>}
      {power && targets.length > 0 && (
        <div className="fleet-power-hint" role="note">
          <span>Fleet runs it one node at a time and follows each node back before the next (built, not yet verified on your cluster). Restart… and Shut down… also show what goes down with the node.</span>
          {targets.slice(0, 4).map((id) => (
            <button key={id} type="button" className="btn small" disabled={lookOnly} title={lookOnly ? LOOK_ONLY_TIP : undefined} onClick={() => void requestPower(id, /poweroff|halt|shut/i.test(command) && !/-r\b|reboot/.test(command) ? "poweroff" : "reboot")}>
              {/poweroff|halt|shut/i.test(command) && !/-r\b|reboot/.test(command) ? "Shut down" : "Restart"} {name(id)}…
            </button>
          ))}
        </div>
      )}

      {run && (
        <div className="fleet-bar">
          <span className="mono fleet-bar-cmd" title={run.command}>{run.sudo ? "# " : "$ "}{run.command}</span>
          <span className="muted num">
            {running ? `${outputs.filter((o) => DONE.includes(o.state)).length}/${outputs.length} done · ${fmtSecs(now - run.startedMs)}` : `${okCount}/${outputs.length} ok`}
          </span>
          <Spacer />
          {run.finished && failedNodes.length > 0 && (
            <ToolButton icon={RotateCcw} label={`Re-run on failed (${failedNodes.length})`} onClick={() => void start(run.command, failedNodes, run.sudo)} />
          )}
          <ToolButton icon={Table2} label="Table" active={asTable} disabled={!table} title={table ? "Show the CSV output as a table" : "Shown when every node prints CSV"} onClick={() => setAsTable(!asTable)} priority={2} />
          <ToolButton icon={WrapText} label="Wrap" active={wrap} onClick={() => setWrap(!wrap)} priority={3} />
          <ToolButton icon={Copy} label="Copy all" title="Copy every node's output as Markdown" disabled={!doneOutputs.length} onClick={() => copyText(asMarkdown(run.command, groups, name))} priority={2} />
          <Switch checked={perNode} onChange={setPerNode} label="Each node" showLabel />
        </div>
      )}

      <div className="fleet-results">
        {!run && (
          <div className="muted pad">
            Pick nodes, type a command and press Run. Output streams in from each node; nodes that print the same thing are shown together.
            A time limit stops the command on the node itself.
          </div>
        )}
        {asTable && table && run ? (
          <div className="fleet-csv">
            <table className="grid">
              <thead>
                <tr>
                  <th>Node</th>
                  {Array.from({ length: table.width }, (_, i) => <th key={i}>{columnName(run.command, i)}</th>)}
                </tr>
              </thead>
              <tbody>
                {table.rows.map((r, i) => (
                  <tr key={i}><td className="strong">{name(r.node)}</td>{r.cells.map((c, j) => <td key={j} className="mono">{c}</td>)}</tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <>
            {live.map((o) => (
              <OutputCard key={o.node} title={name(o.node)} state={o.state} code={o.code} wrap={wrap}
                extra={o.state === "running" && o.startedMs ? <span className="muted num">{fmtSecs(now - o.startedMs)}</span> : null}
                stdout={o.stdout} stderr={o.stderr} />
            ))}
            {groups.map((g) => (
              <OutputCard key={g.nodes.join(",")} title={g.nodes.map(name).join(", ")} state={g.state} code={g.code} wrap={wrap} differs={g.differs}
                stdout={g.stdout} stderr={g.stderr || (g.state !== "ok" && g.state !== "skipped" ? g.error ?? "" : "")}
                note={g.state === "skipped" ? g.error ?? "Skipped" : undefined}
                truncated={g.nodes.some((n) => run?.outputs[n]?.truncated)}
                extra={g.nodes.length === 1 && run?.outputs[g.nodes[0]]?.ms != null ? <span className="muted num">{fmtSecs(run.outputs[g.nodes[0]].ms!)}</span> : null} />
            ))}
          </>
        )}
      </div>
      </>}
    </div>
  );
}

/** nvidia-smi --query-gpu=a,b,c names its CSV columns. */
function columnName(command: string, i: number): string {
  const q = command.match(/--query-[a-z-]+=([^\s]+)/);
  const names = q?.[1].split(",") ?? [];
  return names[i] ?? `Column ${i + 1}`;
}

function OutputCard({ title, state, code, stdout, stderr, wrap, differs, extra, note, truncated }: {
  title: string;
  state: FleetNodeState;
  code: number | null;
  stdout: string;
  stderr: string;
  wrap: boolean;
  differs?: boolean;
  extra?: ReactNode;
  note?: string;
  truncated?: boolean;
}) {
  const empty = !stdout && !stderr;
  return (
    <section className={`fleet-card ${state}`} aria-label={`${title}: ${stateWord(state, code)}`}>
      <div className="fleet-head">
        <span className="strong">{title}</span>
        <Badge tone={STATE_TONE[state]}>{stateWord(state, code)}</Badge>
        {differs && <Badge tone="warn" title="These nodes answered differently from the rest">differs</Badge>}
        {truncated && <Badge tone="neutral" title="More than 1 MiB of output; the start is kept">cut at 1 MiB</Badge>}
        {extra}
        <Spacer />
        <button type="button" className="tool icon-only" title="Copy this output" aria-label={`Copy ${title}'s output`} onClick={() => copyText(stripAnsi(stdout + (stderr ? `\n${stderr}` : "")))}>
          <Icon icon={Copy} size={12} />
        </button>
      </div>
      <pre className={`fleet-out${wrap ? " wrap" : ""}`}>
        {note ? <span className="muted">{note}</span> : empty ? <span className="muted">{state === "running" || state === "queued" ? "Waiting for output…" : "(no output)"}</span> : null}
        {stdout && <Ansi text={stdout} />}
        {stderr && <Ansi text={stdout && !stdout.endsWith("\n") ? `\n${stderr}` : stderr} className="fleet-stderr" />}
      </pre>
    </section>
  );
}

function applyEvent(run: Run, e: FleetOutputEvent | FleetNodeEvent): Run {
  const cur = run.outputs[e.node];
  if (!cur) return run;
  if ("chunk" in e) {
    const next = e.stream === "stdout" ? { ...cur, stdout: cur.stdout + e.chunk } : { ...cur, stderr: cur.stderr + e.chunk };
    return { ...run, outputs: { ...run.outputs, [e.node]: next } };
  }
  const next: NodeOutput = {
    ...cur,
    state: e.state,
    code: e.code ?? cur.code,
    ms: e.ms ?? cur.ms,
    error: e.error ?? cur.error,
    truncated: e.truncated ?? cur.truncated,
    startedMs: e.state === "running" ? Date.now() : cur.startedMs,
  };
  return { ...run, outputs: { ...run.outputs, [e.node]: next } };
}
