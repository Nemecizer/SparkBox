// What's running on the nodes: every process (up to the 200 busiest per node, always with the
// model servers, GPU users, listeners and system daemons) with htop's signals and priority,
// and every listening port with what owns it. Return opens a process's detail (a container's
// inspect, a service's status), right-click or ⇧F10 has every signal. A node that goes
// offline keeps its last-known processes, greyed, state "unknown · cell3 offline", until it's
// back. Like htop, the list holds still while the pointer is over it or a row is selected, and
// a selected process that exits stays, greyed, for a minute. Each second's snapshot carries only
// a node's 15 busiest processes and the listeners' owners: while this window is open it asks for
// the whole list (processes_get) of each shown node whose snapshot is cut, every 5 s.
import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { Box, ChevronDown, ExternalLink, Eye, FileText, Gauge, Lock, Minus, OctagonX, Plus, ServerCog, SquareTerminal } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { Icon, StopIcon } from "../components/Icon";
import { Table, type Column } from "../components/Table";
import { Badge, CodeBlock, HelpTip, InfoTip, Notice, runTool, SearchField, Segmented, Spacer, Toolbar, ToolButton, ToolSeparator, Waiting } from "../components/ui";
import { lockedHint, lockOf, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, requestAdmin, useAdmin, useAdminState } from "../lib/admin";
import { api, type ProcessSignal } from "../lib/api";
import { menuAt, showContextMenu, type MenuItem } from "../lib/contextMenu";
import { showError } from "../lib/errors";
import { fmtBytes } from "../lib/format";
import { portOwner as ownerOf, REACH_WORD, scopeOf, shortNode, usually, type PortOwner } from "../lib/knownPorts";
import { copyText } from "../lib/markdown";
import { opsApi } from "../lib/opsApi";
import { confirmAction } from "../lib/session";
import { getCluster, useCluster } from "../lib/store";
import { openTerminal } from "../lib/terminals";
import { toast } from "../lib/toasts";
import type { Listener, NodeSnapshot, Process } from "../lib/types";
import { processesCut, processesOf, rememberProcesses, staleState, withLastKnown } from "../lib/workloadCommands";
import { useWM, useWinState } from "../wm/WindowManager";
import { openOnNode } from "./NodeDetail";
import "./ops.css";
import "./workloads.css";

/** A selected process that exited stays in the list, greyed, this long. */
const GONE_KEEP_MS = 60_000;
/** Unheld, the list re-sorts at most this often. */
const RESORT_MS = 5_000;
/** Userland processes the collector sends at most per node (collector.py MAX_PROCESSES). */
const MAX_SENT = 200;
/** How often an open window asks for each shown node's whole process list. */
export const PROCESSES_EVERY_MS = 5_000;

const SIGNALS: { sig: ProcessSignal; label: string; what: string }[] = [
  { sig: "TERM", label: "TERM — stop cleanly", what: "asks it to shut down cleanly" },
  { sig: "INT", label: "INT — interrupt (Ctrl-C)", what: "interrupts it, like Ctrl-C" },
  { sig: "HUP", label: "HUP — reload", what: "usually makes a daemon reload its settings" },
  { sig: "KILL", label: "KILL — force stop", what: "ends it at once, with no chance to clean up" },
  { sig: "STOP", label: "STOP — pause", what: "pauses it until it gets CONT" },
  { sig: "CONT", label: "CONT — resume", what: "resumes a paused process" },
];

type View = "processes" | "ports";
type Only = "all" | "gpu" | "listening";

interface ProcRow {
  key: string;
  node: NodeSnapshot;
  p: Process;
  /** Its node is offline: the last-known process, state unknown. */
  stale?: boolean;
  /** Selected, then gone from the node's list: when it was last seen (Unix ms). */
  goneMs?: number;
  /** Gone because the list was cut at 200, not because it exited. */
  cut?: boolean;
}

interface PortRow extends PortOwner {
  key: string;
  node: NodeSnapshot;
  l: Listener;
}

const STATE_WORD: Record<string, string> = { R: "running", S: "sleeping", D: "waiting on disk", T: "paused", Z: "zombie", I: "idle" };

/**
 * Who owns a listening port, when that's known for certain: the process the collector saw, a
 * process that lists the port, a container's port mapping, or what sudo revealed. The Owner
 * column also shows a likely owner (knownPorts.portOwner) when it isn't.
 */
export function portOwner(node: NodeSnapshot, l: Listener, revealed?: Listener[]): { owner: string | null; via: "process" | "container" | "sudo" | null } {
  const o = ownerOf(node, l, revealed);
  return o.confidence === "confirmed" ? { owner: o.owner, via: o.via === "inferred" ? null : o.via } : { owner: null, via: null };
}

/** Ports a tunnel never opens in the browser: SSH and DNS aren't web pages (tunnels.rs NOT_PAGES). */
const NOT_PAGES = [22, 53];

/**
 * Whether "Open in browser" offers a listed port. Only ports in the node's own listener list
 * are rows here, so this leaves out SSH and DNS; the backend checks the list again.
 */
export function browsable(l: Pick<Listener, "port">): boolean {
  return !NOT_PAGES.includes(l.port);
}

/** "exited · last seen 12 s ago", "not among the 200 busiest · last seen 12 s ago". */
export function goneWords(r: Pick<ProcRow, "goneMs" | "cut">, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - (r.goneMs ?? now)) / 1000));
  return `${r.cut ? `not among the ${MAX_SENT} busiest` : "exited"} · last seen ${s} s ago`;
}

/** The order a sort column gives, held while the list is being read (hovered, or a row selected). */
export class StableSort<R extends { key: string }> {
  private at = 0;
  private values = new Map<string, Map<string, number | string>>();
  /** Starts a fresh order when the list isn't held and the last one is `every` ms old. */
  refresh(held: boolean, now = Date.now(), every = RESORT_MS) {
    if (!held && now - this.at >= every) {
      this.at = now;
      this.values = new Map();
    }
  }
  /** A column's sort that returns the value the row had when the order was made. */
  by(col: string, f: (r: R) => number | string): (r: R) => number | string {
    return (r) => {
      let m = this.values.get(r.key);
      if (!m) this.values.set(r.key, (m = new Map()));
      if (!m.has(col)) m.set(col, f(r));
      return m.get(col)!;
    };
  }
}

/** Where a process runs: its container or systemd unit, with a kind icon, or a dash. */
export function RunsIn({ p }: { p: Pick<Process, "container" | "unit"> }) {
  if (p.container) {
    return <span className="wl-in" title={`Container ${p.container}`}><Icon icon={Box} size={12} aria-label="Container" /><span className="wl-in-name">{p.container}</span></span>;
  }
  if (p.unit) {
    return <span className="wl-in" title={`systemd unit ${p.unit}`}><Icon icon={ServerCog} size={12} aria-label="Service" /><span className="wl-in-name">{p.unit.replace(/\.service$/, "")}</span></span>;
  }
  return <span className="muted">—</span>;
}

export function Workloads() {
  const adminNow = useAdminState();
  const cluster = useCluster();
  const admin = useAdmin();
  const wm = useWM();
  const [view, setView] = useWinState<View>("view", "processes");
  const [only, setOnly] = useWinState<Only>("only", "all");
  const [showOwn, setShowOwn] = useWinState("own", false);
  const [showKernel, setShowKernel] = useWinState("kernel", false);
  const [hovering, setHovering] = useState(false);
  const order = useRef(new StableSort<ProcRow>()).current;
  const lastSelected = useRef<ProcRow | null>(null);
  const [nodeFilter, setNodeFilter] = useWinState("node", "all");
  const [text, setText] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [selectedPort, setSelectedPort] = useState<string | null>(null);
  const [revealed, setRevealed] = useState<Record<string, Listener[]>>({});
  const [nice, setNice] = useState<number | null>(null);
  /** The selected process's full detail (command line and all), opened with Return. */
  const [showDetail, setShowDetail] = useState(false);
  const [msg, setMsg] = useState<{ tone?: "error" | "success" | "info"; text: string } | null>(null);

  const [fetchedAt, setFetchedAt] = useState(0);
  const [fetchFailed, setFetchFailed] = useState<Record<string, string>>({});
  // Every 5 s while open, the whole process list of each shown node whose snapshot has only its busiest.
  const demo = !!cluster?.demo;
  const polled = (cluster?.nodes ?? []).filter((n) => n.status === "online" && (nodeFilter === "all" || n.id === nodeFilter) && (n.processCount ?? 0) > n.processes.length).map((n) => n.id).join(",");
  useEffect(() => {
    const ids = polled ? polled.split(",") : [];
    let open = true;
    const pull = () => {
      for (const id of ids) {
        invoke<Process[]>("processes_get", { node: id }).then((list) => {
          if (!open) return;
          rememberProcesses(demo, id, list);
          setFetchFailed((f) => (id in f ? Object.fromEntries(Object.entries(f).filter(([k]) => k !== id)) : f));
          setFetchedAt(Date.now());
        }, (e) => open && setFetchFailed((f) => ({ ...f, [id]: String(e) })));
      }
    };
    pull();
    const every = window.setInterval(pull, PROCESSES_EVERY_MS);
    return () => {
      open = false;
      window.clearInterval(every);
    };
  }, [polled, demo]);

  const needle = text.trim().toLowerCase();
  const listed = useMemo(() => (cluster ? withLastKnown(cluster) : []).filter((l) => nodeFilter === "all" || l.node.id === nodeFilter), [cluster, nodeFilter, fetchedAt]);
  const nodes = useMemo(() => listed.filter((l) => !l.stale).map((l) => l.node), [listed]);
  const live: ProcRow[] = useMemo(() => listed
    .flatMap(({ node, stale }) => node.processes.map((p) => ({ key: `${node.id}/${p.pid}`, node, p, stale })))
    .filter((r) => (showKernel || !r.p.kernel) && (showOwn || !r.p.sparkbox))
    .filter((r) => only === "all" || (only === "gpu" ? r.p.gpuMem > 0 : r.p.ports.length > 0))
    .filter((r) => !needle || `${r.p.name} ${r.p.comm ?? ""} ${r.p.cmd} ${r.p.user} ${r.p.container ?? ""} ${r.p.unit ?? ""} ${r.p.pid}`.toLowerCase().includes(needle)), [listed, only, needle, showOwn, showKernel]);
  // The selected process keeps its row when it leaves the list: greyed, for a minute.
  const now = Date.now();
  const selectedLive = live.find((r) => r.key === selected);
  if (selectedLive) lastSelected.current = selectedLive;
  else if (lastSelected.current && lastSelected.current.key !== selected) lastSelected.current = null;
  const ghost = !selectedLive && lastSelected.current && !lastSelected.current.stale ? lastSelected.current : null;
  if (ghost && ghost.goneMs === undefined) {
    const sent = ghost.node.processes.filter((p) => !p.kernel).length;
    lastSelected.current = { ...ghost, goneMs: now, cut: sent >= MAX_SENT };
  }
  const gone = ghost && now - (lastSelected.current!.goneMs ?? now) < GONE_KEEP_MS ? lastSelected.current! : null;
  const procs = gone ? [...live, gone] : live;
  order.refresh(hovering || selected !== null, now);
  const ports: PortRow[] = useMemo(() => nodes
    .flatMap((node) => node.listeners.map((l) => ({ key: `${node.id}:${l.addr}:${l.port}`, node, l, ...ownerOf(node, l, revealed[node.id]) })))
    .filter((r) => !needle || `${r.l.port} ${r.owner ?? ""} ${usually(r.l) ?? ""} ${r.node.name}`.toLowerCase().includes(needle)), [nodes, needle, revealed]);
  if (!cluster) return <Waiting />;

  // A process on an offline node, or one that's gone, can be read, not signalled.
  const focus = procs.find((r) => r.key === selected);
  const current = focus && !focus.stale && !focus.goneMs ? focus : undefined;
  const currentPort = ports.find((r) => r.key === selectedPort);
  const hidden = ports.filter((r) => r.confidence !== "confirmed");
  // A node whose whole list couldn't be read shows only the snapshot's busiest and listeners' owners: said so.
  const cut = listed.filter((l) => !l.stale && fetchFailed[l.node.id] && processesCut(cluster, l.node));
  const locked = admin ? undefined : lockedHint("stop or re-prioritize processes");
  // The Signal menu button follows the same lock rule as the toolbar's buttons (lockOf).
  const signalLock = lockOf(!admin, adminNow);

  // A listed port in the browser, through an SSH tunnel to the node (the DGX Dashboard by name).
  const openPort = (r: PortRow) => {
    if (cluster.demo && r.l.port !== 11000) {
      setMsg({ tone: "info", text: `Available with Live data: SparkBox opens ${r.node.name}'s port ${r.l.port} in your browser through an SSH tunnel.` });
      return;
    }
    void openOnNode(r.node, r.l.port, cluster.demo, () => wm.open("settings", { section: "data" }));
  };

  const signal = async (row: ProcRow, sig: ProcessSignal) => {
    const { node, p } = row;
    const info = SIGNALS.find((s) => s.sig === sig)!;
    if (sig !== "CONT") {
      const ok = await confirmAction({
        title: `Send ${sig} to ${p.name} (${p.pid}) on ${node.name}?`,
        body: (
          <div className="ops-confirm">
            <p>SIG{sig} {info.what}.</p>
            {p.container && <p>It runs in container {p.container}; stopping the container is usually better (Containers window).</p>}
            {p.unit && <p>It belongs to {p.unit}; systemd may start it again. Use Services to stop it for good.</p>}
            {p.user === "root" && <p className="muted">It's root's process, so this uses sudo with the cluster password.</p>}
          </div>
        ),
        confirmLabel: sig === "KILL" ? "Force Stop" : sig === "STOP" ? "Pause" : sig === "HUP" ? "Send HUP" : "Stop",
        danger: sig === "KILL" || sig === "TERM" || sig === "INT",
      });
      if (!ok) return;
    }
    if (!(await requestAdmin("process_signal"))) return;
    try {
      await api.processSignal(node.id, p.pid, sig, p.startTime ?? null);
    } catch (e) {
      showError(e, { action: `Send ${sig} to ${p.name}`, node: node.name });
      return;
    }
    // The task's notice confirms the signal; for a stop, also say whether it actually went.
    if (sig === "TERM" || sig === "INT" || sig === "KILL") void watchExit(row, sig);
  };

  /** After a stop: watch for 10 s, then say whether it went. */
  const watchExit = async (row: ProcRow, sig: ProcessSignal) => {
    const { node, p } = row;
    const started = Date.now();
    while (Date.now() - started < 10_000) {
      await new Promise((r) => setTimeout(r, 500));
      let alive = true;
      try {
        alive = await invoke<boolean>("process_alive", { node: node.id, pid: p.pid, startTime: p.startTime ?? null });
      } catch {
        const c = getCluster();
        const n = c?.nodes.find((x) => x.id === node.id);
        alive = !!c && !!n && processesOf(c, n).some((x) => x.pid === p.pid);
      }
      if (!alive) {
        toast({ kind: "success", title: `${p.name} (${p.pid}) exited after ${((Date.now() - started) / 1000).toFixed(1)} s` });
        return;
      }
    }
    toast({
      kind: "error",
      title: `${p.name} (${p.pid}) is still running`,
      body: `It didn't exit within 10 s of SIG${sig}.`,
      actions: sig === "KILL" ? undefined : [{ label: "Force stop", run: () => void signal(row, "KILL") }],
    });
  };

  const applyNice = async (row: ProcRow, value: number) => {
    const { node, p } = row;
    if (value === p.nice) return;
    if (!(await requestAdmin("process_renice"))) return;
    try {
      await api.processRenice(node.id, p.pid, value);
      setNice(null);
    } catch (e) {
      showError(e, { action: `Change the priority of ${p.name}`, node: node.name });
    }
  };

  const logs = (row: ProcRow) => {
    const { node, p } = row;
    const containerId = p.container ? node.containers.find((c) => c.name === p.container)?.id : undefined;
    if (containerId) openTerminal(node.id, { kind: "containerLogs", container: containerId }, { title: `${p.container} logs` });
    else if (p.unit) openTerminal(node.id, { kind: "journal", unit: p.unit });
    else return;
    wm.open("terminal");
  };

  const shell = async (row: ProcRow) => {
    const id = row.node.containers.find((c) => c.name === row.p.container)?.id;
    if (!id) return;
    if (!(await requestAdmin("terminal_open"))) return;
    openTerminal(row.node.id, { kind: "containerShell", container: id }, { title: row.p.container ?? undefined });
    wm.open("terminal");
  };

  const reveal = async () => {
    const targets = nodeFilter === "all" ? [...new Set(hidden.map((r) => r.node.id))] : [nodeFilter];
    if (!targets.length) return;
    if (!(await requestAdmin("ports_reveal"))) return;
    for (const id of targets) {
      try {
        const list = await opsApi.portsReveal(id);
        setRevealed((r) => ({ ...r, [id]: list }));
      } catch (e) {
        showError(e, { action: "Read the listening ports with sudo", node: id });
      }
    }
  };

  // Locked signals stay available: choosing one unlocks Admin mode first (the sheet), then asks, then sends it.
  const signalMenu = (row: ProcRow): MenuItem[] => SIGNALS.map((s) => ({
    label: `${s.label}${s.sig === "CONT" ? "" : "…"}`,
    danger: s.sig === "KILL",
    locked: !admin,
    disabled: (s.sig === "CONT" && row.p.state !== "T") ? "It isn't paused" : false,
    run: () => void signal(row, s.sig),
  }));

  /** A process's detail: its container's inspect, its service's status and journal, or the full command line here. */
  const openDetail = (row: ProcRow) => {
    const containerId = row.p.container ? row.node.containers.find((c) => c.name === row.p.container)?.id : undefined;
    if (containerId) return wm.open("inspect", { node: row.node.id, id: containerId, name: row.p.container! });
    if (row.p.unit) return wm.open("services", { node: row.node.id, unit: row.p.unit });
    setSelected(row.key);
    setShowDetail(true);
  };

  // The command line is masked where it holds a secret (P1.46); Reveal shows it in full behind
  // Admin (node scope) and copies it. R1-K.
  const revealCmd = async (row: ProcRow) => {
    if (!(await requestAdmin("process_reveal"))) return;
    try {
      const full = await invoke<string>("process_reveal", { node: row.node.id, pid: row.p.pid });
      await copyText(full);
      toast({ kind: "success", title: "Full command line copied", body: full, durationMs: 8000 });
    } catch (e) {
      showError(e, { action: "Reveal the command line", node: row.node.name });
    }
  };

  const procMenu = (row: ProcRow, e: MouseEvent) => row.stale ? showContextMenu(e, [
    { label: `${row.node.name} is offline`, disabled: "Its processes are as it last reported them", run: () => {} },
    { separator: true },
    { label: "Copy PID", run: () => void copyText(String(row.p.pid)) },
    { label: "Copy command line", run: () => void copyText(row.p.cmd) },
  ]) : showContextMenu(e, [
    { label: row.p.container ? "Inspect its container" : row.p.unit ? `Show ${row.p.unit}` : "Show details", run: () => openDetail(row) },
    { separator: true },
    { label: "Stop (TERM)…", locked: !admin, run: () => void signal(row, "TERM") },
    { label: "Force stop (KILL)…", danger: true, locked: !admin, run: () => void signal(row, "KILL") },
    { label: "Send signal", submenu: signalMenu(row) },
    { separator: true },
    { label: "Logs", disabled: !row.p.container && !row.p.unit ? "Not in a container or service" : false, run: () => logs(row) },
    { label: "Shell in its container", disabled: !row.p.container ? "Not in a container" : false, locked: !admin && !!row.p.container, run: () => void shell(row) },
    { separator: true },
    { label: "Copy PID", run: () => void copyText(String(row.p.pid)) },
    { label: "Copy command line", run: () => void copyText(row.p.cmd) },
    { label: "Reveal command line", locked: !admin, run: () => void revealCmd(row) },
  ]);

  const dim = (r: ProcRow) => r.stale || !!r.goneMs;
  const procColumns: Column<ProcRow>[] = [
    { key: "node", label: "Node", width: 90, priority: 1, sort: (r) => r.node.name, title: (r) => r.node.name, render: (r) => r.node.name, narrow: (r) => shortNode(r.node.name) },
    { key: "pid", label: "PID", align: "right", width: 70, priority: 1, sort: order.by("pid", (r) => r.p.pid), render: (r) => <span className="num">{r.p.pid}</span> },
    {
      key: "name", label: "Process", priority: 1, minWidth: 150, sort: order.by("name", (r) => r.p.name),
      title: (r) => (r.p.comm && r.p.comm !== r.p.name ? `${r.p.cmd}\nThe kernel calls it ${r.p.comm}.` : r.p.cmd),
      render: (r) => (
        <span className="wl-name">
          <span className="strong">{r.p.name}</span>
          {r.p.sparkbox && <span className="wl-tag" title="SparkBox's own collector, or a command it runs">SparkBox</span>}
          {r.p.kernel && <span className="wl-tag" title="A kernel thread">kernel</span>}
        </span>
      ),
    },
    { key: "cmd", label: "Command", priority: 3, minWidth: 160, title: (r) => r.p.cmd, render: (r) => <span className="mono">{r.p.cmd}</span> },
    { key: "user", label: "User", width: 80, priority: 3, sort: order.by("user", (r) => r.p.user), render: (r) => r.p.user },
    {
      key: "state", label: "State", width: 84, priority: 3, sort: order.by("state", (r) => (r.stale || r.goneMs ? "~" : r.p.state ?? "")),
      render: (r) => (r.goneMs ? <span className="muted ops-stale">{goneWords(r, now)}</span> : r.stale ? <span className="muted">{staleState(r.node)}</span>
        : r.p.state === "T" ? <Badge tone="warn">paused</Badge> : r.p.state === "Z" ? <Badge tone="crit">zombie</Badge> : <span className="muted">{STATE_WORD[r.p.state ?? ""] ?? "—"}</span>),
    },
    { key: "cpu", label: "CPU", align: "right", width: 60, priority: 1, sort: order.by("cpu", (r) => (dim(r) ? -1 : r.p.cpuPct)), render: (r) => (dim(r) ? <span className="muted">—</span> : <span className="num">{r.p.cpuPct.toFixed(0)}%</span>) },
    { key: "mem", label: "Memory", align: "right", width: 84, priority: 1, sort: order.by("mem", (r) => (dim(r) ? -1 : r.p.rss)), render: (r) => (dim(r) ? <span className="muted">—</span> : <span className="num">{fmtBytes(r.p.rss)}</span>) },
    { key: "gpu", label: "GPU memory", align: "right", width: 96, priority: 2, sort: order.by("gpu", (r) => r.p.gpuMem), render: (r) => (r.p.gpuMem && !dim(r) ? <span className="num">{fmtBytes(r.p.gpuMem)}</span> : <span className="muted">—</span>) },
    // The container or unit, never cut: Command and Nice give way first.
    { key: "in", label: "Runs in", help: "ops.workloads.runs-in", priority: 2, width: 150, minWidth: 128, fit: "flex", sort: order.by("in", (r) => r.p.container ?? r.p.unit ?? ""), title: (r) => (r.p.container ? `Container ${r.p.container}` : r.p.unit ? `systemd unit ${r.p.unit}` : ""), render: (r) => <RunsIn p={r.p} /> },
    { key: "ports", label: "Ports", width: 90, priority: 3, render: (r) => <span className="mono">{r.p.ports.join(" ") || "—"}</span> },
    { key: "threads", label: "Threads", align: "right", width: 70, priority: 3, hideable: true, sort: order.by("threads", (r) => r.p.threads ?? 0), render: (r) => <span className="num">{r.p.threads ?? "—"}</span> },
    { key: "nice", label: "Nice", align: "right", width: 56, priority: 3, sort: order.by("nice", (r) => r.p.nice), render: (r) => <span className="num">{r.p.nice}</span> },
  ];

  const portColumns: Column<PortRow>[] = [
    { key: "node", label: "Node", width: 90, priority: 1, sort: (r) => r.node.name, title: (r) => r.node.name, render: (r) => r.node.name, narrow: (r) => shortNode(r.node.name) },
    { key: "port", label: "Port", align: "right", width: 70, priority: 1, sort: (r) => r.l.port, render: (r) => <span className="strong num">{r.l.port}</span> },
    {
      key: "addr", label: "Listening on", priority: 3, sort: (r) => r.l.addr,
      title: (r) => (r.l.family === "both" ? `${r.l.addr}, on IPv4 and IPv6` : r.l.addr),
      render: (r) => <span className="mono">{r.l.addr}{r.l.family === "both" ? <span className="muted"> + IPv6</span> : null}</span>,
    },
    { key: "reach", label: "Reachable", width: 112, priority: 2, sort: (r) => scopeOf(r.l), render: (r) => REACH_WORD[scopeOf(r.l)] },
    { key: "what", label: "Usually", priority: 2, minWidth: 120, render: (r) => usually(r.l) ?? <span className="muted">—</span> },
    {
      key: "owner", label: "Owner", priority: 1, minWidth: 150, sort: (r) => r.owner ?? "~", title: (r) => r.title,
      render: (r) => (r.owner
        ? <span>{r.confidence === "likely" ? <>{r.owner.replace(/ \(likely\)$/, "")} <span className="muted">(likely)</span></> : r.owner}{r.via === "sudo" && <span className="muted"> · via sudo</span>}</span>
        : <span className="muted">—</span>),
    },
  ];

  const stepNice = (delta: number) => current && setNice(Math.max(-20, Math.min(19, (nice ?? current.p.nice) + delta)));

  return (
    <div className="col fill ops-window">
      <Toolbar label="Workloads">
        <Segmented<View> value={view} onChange={setView} label="View" options={[
          { value: "processes", label: "Processes" },
          { value: "ports", label: "Listening ports" },
        ]} />
        {view === "processes" ? <HelpTip topic="see-and-stop-processes" /> : <HelpTip topic="ops.workloads.ports" />}
        {view === "processes" && (
          <>
            <ToolSeparator />
            <ToolButton icon={StopIcon} label="Stop" disabled={!current} locked={!admin} title={current ? "Send TERM: ask it to shut down cleanly" : selected ? "Its node is offline" : "Select a process"} onClick={() => current && signal(current, "TERM")} />
            <ToolButton icon={OctagonX} label="Force stop" danger disabled={!current} locked={!admin} title="Send KILL: end it at once" onClick={() => current && signal(current, "KILL")} priority={2} />
            <button type="button" className={`tool${signalLock ? " locked" : ""}`} disabled={!current || signalLock === "lookOnly"} data-lock={signalLock ?? undefined}
              title={signalLock === "lookOnly" ? LOOK_ONLY_TIP : signalLock ? `Send another signal · ${NEEDS_ADMIN_TIP}` : "Send another signal"}
              aria-label={signalLock === "lookOnly" ? "Signal — look-only" : signalLock ? "Signal — needs Admin" : "Signal"} onClick={(e) => current && menuAt(e.currentTarget, signalMenu(current))}>
              <span className="tool-label">Signal</span><Icon icon={ChevronDown} size={12} />{signalLock && <Icon icon={Lock} size={12} className="tool-lock" />}
            </button>
            <HelpTip topic="ops.workloads.signals" />
            <ToolButton icon={FileText} label="Logs" priority={2} disabled={!current || (!current.p.container && !current.p.unit)} title="The container's log or the service's journal, in a Terminal tab" onClick={() => current && logs(current)} />
            <ToolButton icon={SquareTerminal} label="Shell" priority={3} disabled={!current?.p.container} locked={!admin && !!current?.p.container} title="bash inside the process's container" onClick={() => current && shell(current)} />
          </>
        )}
        {view === "ports" && (
          <>
            <ToolSeparator />
            <ToolButton icon={ExternalLink} label="Open in browser" locked={!admin} action="tunnel_open" disabled={!currentPort || !browsable(currentPort.l)}
              title={currentPort && !browsable(currentPort.l) ? `Port ${currentPort.l.port} is ${currentPort.l.port === 22 ? "SSH" : "DNS"}, not a web page` : "Through an SSH tunnel, so it works even when the port only listens on the node itself"}
              onClick={() => currentPort && openPort(currentPort)} />
            <ToolButton icon={Eye} label="Reveal with sudo" disabled={!hidden.length} locked={!admin && hidden.length > 0}
              title={hidden.length ? `Ask the node (with sudo) which process owns ${hidden.length} port${hidden.length === 1 ? "" : "s"} the cluster user can't see` : "Every port's owner is already known"}
              onClick={reveal} />
            <HelpTip topic="ops.workloads.reveal" />
          </>
        )}
        <Spacer />
        {view === "processes" && (
          <>
            <select value={only} onChange={(e) => setOnly(e.target.value as Only)} aria-label="Show">
              <option value="all">Busiest + GPU + listening</option>
              <option value="gpu">Using the GPU</option>
              <option value="listening">Listening on a port</option>
            </select>
            <button type="button" className="tool" aria-label="View options" title="Show SparkBox's own processes or kernel threads"
              onClick={(e) => menuAt(e.currentTarget, [
                { label: "Show SparkBox's own", checked: showOwn, run: () => setShowOwn(!showOwn) },
                { label: "Show kernel threads", checked: showKernel, run: () => setShowKernel(!showKernel) },
              ])}>
              <span className="tool-label">View</span><Icon icon={ChevronDown} size={12} />
            </button>
          </>
        )}
        <select value={nodeFilter} onChange={(e) => setNodeFilter(e.target.value)} aria-label="Node">
          <option value="all">All nodes</option>
          {cluster.nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}
        </select>
        <SearchField value={text} onChange={setText} placeholder="Filter" count={view === "processes" ? procs.length : ports.length} />
      </Toolbar>
      {msg && <Notice tone={msg.tone} onDismiss={() => setMsg(null)}>{msg.text}</Notice>}
      {view === "processes" && cut.length > 0 && (
        <Notice tone="info">
          {cut.map((l) => `${l.node.name} shows ${l.node.processes.length} of its ${l.node.processCount} processes`).join("; ")}: the 15 busiest and those listening on a port. SparkBox couldn't read the whole list: {fetchFailed[cut[0].node.id]}
        </Notice>
      )}
      {view === "processes" && focus && !current && (
        <div className="ops-nice" role="status">
          <span className="strong">{focus.p.name} ({focus.p.pid}) on {focus.node.name}</span>
          <span className="muted">{focus.goneMs ? goneWords(focus, now) : staleState(focus.node)}</span>
          <Spacer />
          <button type="button" className="btn small" aria-expanded={showDetail} onClick={() => setShowDetail(!showDetail)}>{showDetail ? "Hide details" : "Details"}</button>
        </div>
      )}
      {view === "processes" && current && (
        <div className="ops-nice" role="group" aria-label="Priority">
          <span className="strong">{current.p.name} ({current.p.pid}) on {current.node.name}</span>
          <Spacer />
          <Icon icon={Gauge} size={14} />
          <span>Priority (nice)</span>
          <button type="button" className="btn small icon" aria-label="Higher priority (lower nice)" disabled={!admin || (nice ?? current.p.nice) <= -20} onClick={() => stepNice(-1)}><Icon icon={Minus} size={12} /></button>
          <input className="narrow num" type="number" min={-20} max={19} aria-label="Nice value" value={nice ?? current.p.nice} disabled={!admin}
            onChange={(e) => setNice(Math.max(-20, Math.min(19, Number(e.target.value) || 0)))} />
          <button type="button" className="btn small icon" aria-label="Lower priority (higher nice)" disabled={!admin || (nice ?? current.p.nice) >= 19} onClick={() => stepNice(1)}><Icon icon={Plus} size={12} /></button>
          <button type="button" className="btn small" disabled={!admin || nice === null || nice === current.p.nice} title={locked} onClick={() => applyNice(current, nice ?? current.p.nice)}>Apply</button>
          <HelpTip topic="ops.workloads.priority" label="About priority" />
          <button type="button" className="btn small" aria-expanded={showDetail} onClick={() => setShowDetail(!showDetail)}>{showDetail ? "Hide details" : "Details"}</button>
        </div>
      )}
      {view === "processes" && focus && showDetail && (
        <div className="wl-detail" aria-label={`${focus.p.name} (${focus.p.pid})`}>
          <dl className="ops-kv">
            <div><dt>User</dt><dd>{focus.p.user}</dd></div>
            <div><dt>State</dt><dd>{focus.goneMs ? goneWords(focus, now) : STATE_WORD[focus.p.state ?? ""] ?? "—"}</dd></div>
            <div><dt>Runs in</dt><dd><RunsIn p={focus.p} /></dd></div>
            <div><dt>Threads</dt><dd className="num">{focus.p.threads ?? "—"}</dd></div>
            <div><dt>Ports</dt><dd className="mono">{focus.p.ports.join(" ") || "—"}</dd></div>
            {focus.p.comm && focus.p.comm !== focus.p.name && <div><dt>Kernel name</dt><dd className="mono">{focus.p.comm}</dd></div>}
          </dl>
          <CodeBlock text={focus.p.cmd} maxHeight={120} label="Command line" />
        </div>
      )}
      {view === "processes" ? (
        <div className="wl-hold" onPointerEnter={() => setHovering(true)} onPointerLeave={() => setHovering(false)}>
        <Table key="procs" tableId="workloads-procs" columns={procColumns} rows={procs} rowKey={(r) => r.key} selected={selected}
          onSelect={(k) => { setSelected(k); setNice(null); }}
          onActivate={openDetail}
          onRowContextMenu={(r, e) => procMenu(r, e)}
          initialSort={{ key: "cpu", dir: -1 }} itemNoun={["process", "processes"]} ariaLabel="Processes"
          footer={(
            <InfoTip label="About this list" topic="ops.workloads.list">
              Every process on each node, up to the {MAX_SENT} busiest (CPU, then memory), always with model servers, GPU
              users, listeners and the system daemons; refreshed every 5 s. SparkBox's own processes and kernel threads are
              hidden unless you show them (View). The order holds while the pointer is over the list or a row is selected.
              Root's processes are signaled with sudo; a stop is checked against the process's start time, so a reused
              PID is never hit.
            </InfoTip>
          )}
          rowClass={(r) => (r.stale || r.goneMs ? "row-dim row-stale" : r.p.state === "T" ? "row-warn" : r.p.state === "Z" ? "row-crit" : undefined)}
          empty={!listed.some((l) => l.node.processes.length) ? "No processes reported yet (they refresh every 5 seconds)."
            : needle ? `No process matches “${text.trim()}”.`
            : only === "gpu" ? "Nothing is using the GPU."
            : only === "listening" ? "No process shows a listening port (root's need Reveal with sudo, under Listening ports)."
            : "No processes to show."} />
        </div>
      ) : (
        <Table key="ports" tableId="workloads-ports" columns={portColumns} rows={ports} rowKey={(r) => r.key} initialSort={{ key: "port", dir: 1 }}
          selected={selectedPort} onSelect={setSelectedPort} itemNoun={["port", "ports"]} ariaLabel="Listening ports"
          footer={(
            <HelpTip topic="ops.workloads.ports-detail" label="About listening ports" />
          )}
          onActivate={(r) => browsable(r.l) && void runTool({ label: "Open in browser", locked: !admin, action: "tunnel_open", onClick: () => openPort(r) })}
          empty={needle && nodes.some((n) => n.listeners.length) ? `No port matches “${text.trim()}”.` : "No listening ports reported yet."} />
      )}
    </div>
  );
}
