// Software updates on every node (apt), like Cockpit's Software Updates side by side: how much
// is pending, how much of it is security, whether a restart is waiting, what's held, and the
// packages that need care (kernel, NVIDIA driver, CUDA, Docker, Mellanox/DOCA) on top, from
// the background check's cache (updates_summary), with DGX OS, firmware, Ollama and snaps. A
// dry run shows the plan, and the GPU-stack guard refuses one that would move the stack; Upgrade
// installs exactly the plan shown (its versions pinned), or nothing. An upgrade runs detached on
// the node; this window follows it: the live apt log, "Unpacking 3 of 7", a Cancel that stops
// apt only while that's safe, and afterwards the services still on old libraries, each with a
// Restart.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CircleStop, ExternalLink, FlaskConical, PackageCheck, Pin, PinOff, RefreshCw, RotateCcw, ShieldAlert, ShieldCheck } from "lucide-react";
import { Icon } from "../components/Icon";
import { Table, type Column } from "../components/Table";
import { Badge, CodeBlock, EmptyState, HelpTip, Notice, SearchField, Spacer, Toolbar, ToolButton, ToolSeparator, Waiting } from "../components/ui";
import { isLookOnly, LOOK_ONLY_TIP, lockedHint, requestAdmin, useAdmin, useAdminState } from "../lib/admin";
import { showError } from "../lib/errors";
import { deploymentsOf } from "../lib/impact";
import { LIVE_ONLY } from "../lib/maintenance";
import { DRY_RUN_FIRST, opsApi, type NeedRestart, type NodeUpdates, type PendingPackage, type PlannedPkg, type SimulatedUpgrade, type UpdateSummary } from "../lib/opsApi";
import { requestPower } from "../lib/power";
import { alertInfo, confirmAction } from "../lib/session";
import { useCluster } from "../lib/store";
import { openTasksWindow, useTasks, waitTask } from "../lib/tasks";
import type { NodeSnapshot } from "../lib/types";
import { runServiceAction } from "../lib/workloadCommands";
import { useViewLink, useWinState, useWM, type Params } from "../wm/WindowManager";
import { openOnNode } from "./NodeDetail";
import { relTime, tasksFor } from "./TasksWindow";
import "./ops.css";
import "./updates.css";
import { RollingUpdates } from "./RollingUpdates";
import { ProtectedUpdates } from "./ProtectedUpdates";

/** An upgrade running (or just finished) on a node (src-tauri/src/ops/updates.rs). */
export interface UpgradeRun {
  node: string;
  taskId: string;
  title: string;
  unit: string;
  logPath: string;
  packages: string[];
  /** checking: its dry run is under way and nothing has started; starting: the start is sent;
   * lost: SparkBox lost the node while apt may still run there (followed again when it's back). */
  phase: "checking" | "starting" | "preparing" | "downloading" | "unpacking" | "configuring" | "finishing" | "done" | "failed" | "cancelled" | "lost";
  done: number;
  total: number;
  current: string | null;
  log: string[];
  cancellable: boolean;
  /** Why the last Stop didn't work, until a later one is answered. */
  stopFailed?: string | null;
  startedMs: number;
  exitCode: number | null;
  simulated: boolean;
  /** The package list saved on the node before apt started. */
  dpkgBefore?: string | null;
  /** Its copy in the evidence folder on this Mac. */
  evidence?: string | null;
  /** What needrestart listed after apt. */
  needrestart?: NeedRestart | null;
  /** What apt-get check printed after apt. */
  aptCheck?: string | null;
  /** Freshly read after apt and the package-database checks; absent means not verified. */
  verification?: { checkedMs: number; remaining: PendingPackage[] } | null;
}

export const updatesProgress = () => invoke<UpgradeRun[]>("updates_progress");
export const updatesCancel = (node: string) => invoke<string>("updates_cancel", { node });

const PHASE_WORD: Record<UpgradeRun["phase"], string> = {
  checking: "Checking what will change",
  starting: "Starting apt",
  preparing: "Starting apt",
  downloading: "Downloading packages",
  unpacking: "Unpacking",
  configuring: "Setting up",
  finishing: "Finishing",
  done: "Finished",
  failed: "Failed",
  cancelled: "Stopped",
  lost: "Lost the node; apt may still be running there",
};

const finished = (r: UpgradeRun) => r.phase === "done" || r.phase === "failed" || r.phase === "cancelled";
/** A task follows it now (a lost one waits for its node). */
const followed = (r: UpgradeRun) => !finished(r) && r.phase !== "lost";

/** "libbrotli1: 1.1.0-2build2 → 1.1.0-2ubuntu0.1", or "libnew: 2.0-1 (new)". */
export function planLine(p: PlannedPkg): string {
  return p.new || !p.from ? `${p.name}: ${p.to} (new)` : `${p.name}: ${p.from} → ${p.to}`;
}

/** A refusal as a sentence: the backend's words, with one full stop. */
const sentence = (s: string) => (/[.!?]$/.test(s) ? s : `${s}.`);

/** NVIDIA's DGX Dashboard (it also shows DGX OS updates) listens on this port on each node. */
export const DGX_DASHBOARD_PORT = 11000;

/** Why the selected node's DGX Dashboard can't open, or null when it can (Demo explains itself). */
export function dashboardBlocked(node: NodeSnapshot | undefined, demo: boolean): string | null {
  if (!node) return "Select a node";
  if (demo) return null;
  return node.listeners.some((l) => l.port === DGX_DASHBOARD_PORT) ? null : `${node.name} isn't listening on port ${DGX_DASHBOARD_PORT}`;
}

/** "Unpacking 3 of 7 · libnvidia-compute-580". */
export function phaseText(r: UpgradeRun): string {
  const step = r.phase === "unpacking" || r.phase === "configuring";
  const n = Math.max(1, Math.min(r.done, r.total));
  return `${PHASE_WORD[r.phase]}${step && r.total ? ` ${n} of ${r.total}` : ""}${step && r.current ? ` · ${r.current}` : ""}`;
}

/** Overall progress: unpacking is the first half, setting up the second. */
function fraction(r: UpgradeRun): number {
  const m = Math.max(1, r.total);
  if (r.phase === "unpacking") return r.done / (2 * m);
  if (r.phase === "configuring") return (m + r.done) / (2 * m);
  if (r.phase === "finishing" || r.phase === "done") return 1;
  return 0;
}

/** Lines the upgrade's unit writes for SparkBox around apt's own output. */
const MARKER = /^SPARKBOX-(EXIT|AFTER|AFTER-END)\b/;

/** needrestart's kernel line, when a newer kernel than the running one is installed. */
export function kernelLine(nr: NeedRestart | null | undefined): string | null {
  if (!nr || (nr.kernelStatus ?? 0) < 2) return null;
  return `A newer kernel is installed${nr.kernelExpected ? ` (${nr.kernelExpected})` : ""}; it runs after the next restart.`;
}

/** The upgrade on one node: phase, progress, the live apt log and Cancel; afterwards the package
 * list saved before it and what needrestart listed, each service with a Restart. */
function UpgradePanel({ run, name, onCancelled }: { run: UpgradeRun; name: string; onCancelled: () => void }) {
  const logRef = useRef<HTMLPreElement>(null);
  const lookOnly = isLookOnly(useAdminState());
  const [stopping, setStopping] = useState(false);
  const pinned = useRef(true);
  useEffect(() => {
    const el = logRef.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [run.log.length]);
  const cancel = async () => {
    setStopping(true);
    try {
      await updatesCancel(run.node);
      onCancelled();
    } catch (e) {
      showError(e, { action: `Stop the upgrade on ${name}` });
    } finally {
      setStopping(false);
    }
  };
  const running = followed(run);
  const lost = run.phase === "lost";
  const pct = Math.round(fraction(run) * 100);
  return (
    <div className="ops-detail upd-run" aria-label={`Upgrade on ${name}`}>
      <div className="ops-detail-head">
        <h3 className="ops-detail-title">{lost ? `Lost ${name} while it was upgrading` : running ? `Upgrading ${name}` : run.phase === "done" ? `Upgraded ${name}` : run.phase === "cancelled" ? `Upgrade stopped on ${name}` : `Upgrade failed on ${name}`}</h3>
        <span className={`upd-phase${run.phase === "failed" ? " crit-text" : ""}`} role="status">{phaseText(run)}</span>
      </div>
      <div className="upd-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label="Upgrade progress">
        <div className="upd-progress-bar" style={{ width: `${pct}%` }} />
      </div>
      <div className="upd-meta">
        {run.unit && (
          <>
            <span className="mono" title="The transient systemd unit apt runs in; it keeps going if SparkBox disconnects">{run.unit}</span>
            <span className="muted">·</span>
            <span className="mono muted" title="apt's output on the node">{run.logPath}</span>
          </>
        )}
        <Spacer />
        {running && (
          run.cancellable
            ? <button type="button" className="btn small" disabled={stopping || lookOnly} onClick={() => void cancel()} title={lookOnly ? LOOK_ONLY_TIP : "Stops apt now, while it's still downloading. Nothing has been installed yet."}>
                <CircleStop size={12} aria-hidden /> {stopping ? "Stopping…" : "Stop before it installs"}
              </button>
            : <button type="button" className="btn small" disabled title="dpkg is installing packages; stopping it now would leave them half-configured">
                <CircleStop size={12} aria-hidden /> Can't stop safely now
              </button>
        )}
      </div>
      {running && run.stopFailed && <Notice tone="warn">{sentence(run.stopFailed)}</Notice>}
      {lost && <Notice tone="warn">apt may still be running on {name}. SparkBox follows it again when {name} is back, and after a restart (log: {run.logPath}).</Notice>}
      {run.dpkgBefore && (
        <div className="upd-meta">
          <span className="muted">Package list before it:</span>
          <span className="mono" title={run.evidence ? `Also in the evidence folder: ${run.evidence}` : "Only on the node"}>{run.dpkgBefore}</span>
        </div>
      )}
      {finished(run) && (
        <Notice tone={run.verification ? run.verification.remaining.length ? "warn" : "info" : "warn"}>
          {run.verification
            ? run.verification.remaining.length
              ? `Package check complete: ${run.verification.remaining.length} ${run.verification.remaining.length === 1 ? "update remains" : "updates remain"}. ${run.verification.remaining.map((p) => `${p.name}${p.held ? " (held)" : p.sensitive ? " (GPU stack)" : ""}`).join(", ")}.`
              : "Package check passed: no package updates remain."
            : "The final package check did not complete. Remaining updates are unknown."}
        </Notice>
      )}
      {run.needrestart && (
        <div aria-label="Services still using old libraries">
          <div className="section-title">Still using old libraries (needrestart) <HelpTip topic="ops.updates.needrestart" /></div>
          {kernelLine(run.needrestart) && <Notice tone="warn">{kernelLine(run.needrestart)}</Notice>}
          {run.needrestart.services.length === 0 ? (
            <p className="muted">No services are left on old libraries.</p>
          ) : (
            <ul className="ops-list mono">
              {run.needrestart.services.map((svc) => (
                <li key={svc}>
                  {svc}{" "}
                  <button type="button" className="btn small" disabled={lookOnly} title={lookOnly ? LOOK_ONLY_TIP : `Restart ${svc} on ${name}`}
                    aria-label={`Restart ${svc}`} onClick={() => void runServiceAction([run.node], svc, "restart", { origin: "updates" })}>
                    <RotateCcw size={12} aria-hidden /> Restart
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      <pre className="upd-log mono" ref={logRef} tabIndex={0} aria-label="apt output"
        onScroll={(e) => { const el = e.currentTarget; pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24; }}>
        {run.log.filter((l) => !MARKER.test(l)).join("\n") || "Waiting for apt…"}
      </pre>
    </div>
  );
}

interface NodeRow {
  node: NodeSnapshot;
  s: UpdateSummary | null;
  u: NodeUpdates | null;
  error: string | null;
}

/** The value most nodes have; nodes with another one have drifted. */
export function majority(values: (string | null | undefined)[]): string | null {
  const counts = new Map<string, number>();
  for (const v of values) if (v) counts.set(v, (counts.get(v) ?? 0) + 1);
  let best: string | null = null;
  let n = 0;
  for (const [v, c] of counts) if (c > n) [best, n] = [v, c];
  return n > 1 || counts.size === 1 ? best : null;
}

/** The Docker line of the Upgrade confirm, from whether live-restore is on (docker info). */
export function dockerLine(liveRestore: boolean | null | undefined, running: number): string {
  if (liveRestore === true) return "Docker restarts; with live-restore on, running containers keep running.";
  if (liveRestore === false) return `Docker restarts: its ${running} running container${running === 1 ? "" : "s"} stop${running === 1 ? "s" : ""}; only those with a restart policy come back.`;
  return `Docker restarts during the upgrade: ${running === 1 ? "its running container stops, and comes back by itself only if it has" : `its ${running} running containers stop, and only those with`} a restart policy${running === 1 ? "" : " come back by themselves"} (couldn't read whether live-restore is on).`;
}

/** The value each new column shows, from the summary (null: not known). */
export function dgxCell(s: UpdateSummary | null): string | null {
  if (!s?.dgx?.ota) return null;
  return s.dgx.otaPending ? `${s.dgx.ota} · update pending` : s.dgx.ota;
}

export function firmwareCell(s: UpdateSummary | null): string | null {
  const f = s?.firmware;
  if (!f) return null;
  if (f.state === "none") return "none";
  if (f.state === "available") return String(f.count);
  if (f.state === "needsSudo") return "needs sudo";
  return null;
}

export function ollamaCell(s: UpdateSummary | null): string | null {
  const o = s?.ollama;
  if (!o?.installed) return null;
  return `${o.installed} · ${o.latest ? `latest ${o.latest}` : "couldn't check"}`;
}

/** The nodes whose held packages differ from what most nodes hold. */
export function holdDrift(rows: { name: string; held: string[] | null }[]): string[] {
  const key = (held: string[]) => `held:${[...held].sort().join(",")}`;
  const known = rows.filter((r): r is { name: string; held: string[] } => r.held !== null);
  const most = majority(known.map((r) => key(r.held)));
  return most ? known.filter((r) => key(r.held) !== most).map((r) => r.name) : [];
}

/** What "Upgrade security only" takes: security fixes that aren't GPU-stack or held (the backend picks the same). */
const securityFixes = (u: NodeUpdates | null | undefined) => (u?.pending ?? []).filter((p) => p.security && !p.sensitive && !p.held);

function kindOf(p: PendingPackage): string {
  if (p.name.startsWith("linux-")) return "kernel";
  if (p.name.includes("nvidia") || p.name.startsWith("cuda") || p.name.startsWith("libcuda") || p.name.startsWith("libxnvctrl")) return "NVIDIA";
  if (p.name.startsWith("docker") || p.name.startsWith("containerd")) return "Docker";
  if (p.name.startsWith("mlnx") || p.name.startsWith("doca") || p.name.startsWith("mft") || p.name.includes("ibverbs") || p.name.startsWith("rdma")) return "network";
  return "";
}

export function UpdatesWindow({ params }: { params: Params }) {
  const cluster = useCluster();
  const admin = useAdmin();
  const wm = useWM();
  const nodes = cluster?.nodes ?? [];
  const [data, setData] = useState<Record<string, UpdateSummary | string>>({});
  const [loading, setLoading] = useState(true);
  // A task's Retry opens the window on its node, or brings it forward on that node.
  const [selected, setSelected] = useWinState<string | null>("node", params.node ?? null);
  useViewLink(params, () => params.node && setSelected(params.node));
  const [pkgSel, setPkgSel] = useState<Set<string>>(new Set());
  // The dry run shown, and the selection it was for ("" for everything).
  const [sim, setSim] = useState<(SimulatedUpgrade & { picked: string }) | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [runs, setRuns] = useState<UpgradeRun[]>([]);
  const allTasks = useTasks();
  const powerTasks = useMemo(() => tasksFor(allTasks, cluster?.demo ? "demo" : "live").filter((t) => t.kind === "power"), [allTasks, cluster?.demo]);
  const restartTask = (node: string) => powerTasks.find((t) => t.nodes.includes(node) && t.retry?.args.action === "reboot");
  // Upgrades are followed live: quickly while one runs, now and then otherwise.
  const anyRunning = runs.some(followed);
  useEffect(() => {
    let alive = true;
    const tick = () => updatesProgress().then((r) => alive && setRuns(r), () => {});
    tick();
    const t = window.setInterval(tick, anyRunning ? 600 : 3000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [anyRunning]);

  const ids = nodes.map((n) => n.id).join(",");
  // From the background check's cache; `fresh` re-reads the nodes now (reads only).
  const load = useCallback(async (only?: string[], fresh?: boolean) => {
    const list = only ?? (ids ? ids.split(",") : []);
    if (!list.length) return;
    let got: UpdateSummary[] | string;
    try {
      got = await opsApi.updatesSummary(list, fresh);
    } catch (e) {
      got = String((e as { message?: string })?.message ?? e);
    }
    setData((prev) => {
      const next = { ...prev };
      for (const id of list) {
        const s = typeof got === "string" ? got : got.find((x) => x.node === id);
        next[id] = s ?? "Not in the update check yet.";
      }
      return next;
    });
    setLoading(false);
  }, [ids]);
  useEffect(() => void load(), [load]);
  // The cache changed (a background check, a refresh): read those nodes again.
  useEffect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    opsApi.onUpdatesChanged((changed) => void load(changed.length ? changed : undefined)).then((u) => (gone ? u() : (stop = u)), () => {});
    return () => {
      gone = true;
      stop?.();
    };
  }, [load]);
  // A node whose upgrade just ended is read again (pending packages, restart needed).
  const wasRunning = useRef(new Set<string>());
  useEffect(() => {
    const ended = runs.filter((r) => finished(r) && wasRunning.current.has(r.node)).map((r) => r.node);
    wasRunning.current = new Set(runs.filter((r) => !finished(r)).map((r) => r.node));
    if (ended.length) void load(ended, true);
  }, [runs, load]);
  // Read package/restart status again after the restart's checks finish. A timeout is not a
  // successful reboot; the normal connection loop and background checks still follow it.
  const wasRestarting = useRef(new Set<string>());
  useEffect(() => {
    const ended = powerTasks.filter((t) => t.state === "ok" && wasRestarting.current.has(t.id)).flatMap((t) => t.nodes);
    wasRestarting.current = new Set(powerTasks.filter((t) => t.state === "running" && t.retry?.args.action === "reboot").map((t) => t.id));
    if (ended.length) void load([...new Set(ended)], true);
  }, [powerTasks, load]);

  const rows: NodeRow[] = nodes.map((node) => {
    const d = data[node.id];
    const s = d && typeof d !== "string" ? d : null;
    const error = typeof d === "string" ? d : s && !s.apt ? s.error ?? "No apt status yet." : null;
    return { node, s, u: s?.apt ?? null, error };
  });
  const driverMajority = majority(nodes.map((n) => n.system.driver));
  const kernelMajority = majority(nodes.map((n) => n.system.kernel));
  const drift = holdDrift(rows.map((r) => ({ name: r.node.name, held: r.u ? r.u.held : null })));
  const current = rows.find((r) => r.node.id === (selected ?? rows[0]?.node.id)) ?? null;
  const needle = text.trim().toLowerCase();
  const pkgs = useMemo(() => (current?.u?.pending ?? []).filter((p) => !needle || `${p.name} ${p.origin}`.toLowerCase().includes(needle)), [current?.u, needle]);
  const chosen = pkgs.filter((p) => pkgSel.has(p.name));
  const picked = chosen.map((p) => p.name).sort().join(" ");
  useEffect(() => {
    setPkgSel(new Set());
    setSim(null);
  }, [current?.node.id]);

  if (!cluster) return <Waiting />;

  const restartBlocked = (node: NodeSnapshot) => {
    if (powerTasks.some((t) => t.nodes.includes(node.id) && t.state === "running") || node.powerState) return "A power task is already in progress";
    if (runs.some((r) => r.node === node.id && r.simulated === cluster.demo && !finished(r))) return "Wait for the package upgrade to finish";
    if (node.status !== "online" || node.stale) return "Wait for this node to reconnect with fresh readings";
    return null;
  };
  const restart = async (node: NodeSnapshot) => {
    if (busy || restartBlocked(node)) return;
    setBusy("restart");
    try {
      const id = await requestPower(node.id, "reboot");
      if (id) openTasksWindow(id);
    } finally { setBusy(null); }
  };

  const refresh = async (targets: string[]) => {
    if (!(await requestAdmin("check for updates"))) return;
    setBusy("refresh");
    try {
      const id = await opsApi.updatesRefresh(targets);
      await waitTask(id);
    } catch (e) {
      showError(e, { action: "Check for updates" });
    } finally {
      setBusy(null);
      // The refresh checked the nodes again itself: read what it found, don't check again.
      await load(targets);
    }
  };

  const simulate = async () => {
    if (!current) return;
    setBusy("simulate");
    try {
      setSim({ ...(await opsApi.updatesSimulate(current.node.id, chosen.length ? chosen.map((p) => p.name) : undefined)), picked });
    } catch (e) {
      showError(e, { action: `Simulate the upgrade on ${current.node.name}` });
    } finally {
      setBusy(null);
    }
  };

  const hold = async (on: boolean) => {
    if (!current || !chosen.length) return;
    const names = chosen.filter((p) => p.held !== on).map((p) => p.name);
    if (!names.length) return;
    if (!(await requestAdmin(`${on ? "hold" : "release"} packages on ${current.node.name}`))) return;
    setBusy("hold");
    try {
      await opsApi.updatesHold(current.node.id, names, on);
    } catch (e) {
      showError(e, { action: `${on ? "Hold" : "Release"} packages on ${current.node.name}` });
    } finally {
      setBusy(null);
      await load([current.node.id], true);
    }
  };

  // Pin the GPU stack (P1.27): the confirm lists what it holds (or releases), read first.
  const pinStack = async (on: boolean) => {
    if (!current) return;
    const n = current.node;
    const what = on ? `Pin the GPU stack on ${n.name}` : `Release the GPU-stack pin on ${n.name}`;
    if (!(await requestAdmin("updates_hold_preset"))) return;
    setBusy("pin");
    try {
      const plan = await opsApi.updatesHoldPreset(n.id, on, true);
      if (!plan.packages.length) {
        await alertInfo({ title: what, body: on ? `No GPU-stack packages are installed on ${n.name}.` : `Nothing of the GPU stack is held on ${n.name}.` });
        return;
      }
      const ok = await confirmAction({
        title: `${what}?`,
        body: (
          <div className="ops-confirm">
            <p>{on
              ? `Holds these ${plan.packages.length} packages with apt-mark, so no upgrade on this node moves them until you release the pin. fwupd isn't held, and SparkBox doesn't upgrade it either yet.`
              : `Releases the hold on these ${plan.packages.length} packages (apt-mark unhold).`}</p>
            <ul className="ops-list mono">{plan.packages.map((p) => <li key={p}>{p}</li>)}</ul>
            <p className="muted">The list goes in the task's log and the evidence folder.</p>
          </div>
        ),
        confirmLabel: on ? "Pin" : "Release",
      });
      if (!ok) return;
      await opsApi.updatesHoldPreset(n.id, on);
    } catch (e) {
      showError(e, { action: what });
    } finally {
      setBusy(null);
      await load([n.id], true);
    }
  };

  // Upgrade installs exactly the plan its dry run showed (decision b): the dry run on screen for
  // what's picked, or for security only its own, run first. Demo binds to its own without one.
  const upgrade = async (securityOnly: boolean, target?: NodeRow) => {
    let row = target ?? current;
    if (!row?.u) return;
    const n = row.node;
    const rowUpgrade = !!target;
    let take = securityOnly ? securityFixes(row.u) : (chosen.length ? chosen : row.u.pending).filter((p) => !p.held);
    let shown: SimulatedUpgrade | null = !rowUpgrade && !securityOnly && simFits && sim ? sim : null;
    if (rowUpgrade || securityOnly) {
      if (rowUpgrade) {
        if (!(await requestAdmin(`upgrade ${n.name}`))) return;
        setSelected(n.id);
        setPkgSel(new Set());
        setSim(null);
      }
      setBusy("simulate");
      try {
        if (rowUpgrade) {
          const fresh = (await opsApi.updatesSummary([n.id], true)).find((s) => s.node === n.id);
          if (!fresh?.apt || fresh.error) throw new Error(fresh?.error ?? `Couldn't read the pending packages on ${n.name}.`);
          row = { ...row, s: fresh, u: fresh.apt };
          setData((prev) => ({ ...prev, [n.id]: fresh }));
          take = fresh.apt.pending.filter((p) => !p.held && !p.sensitive);
        }
        const names = take.map((p) => p.name);
        if (!names.length) {
          await alertInfo({ title: `Upgrade ${n.name}`, body: row.u!.pending.length ? "Only held or GPU-stack packages remain. SparkBox leaves them unchanged." : "No package updates remain." });
          return;
        }
        if (rowUpgrade) setPkgSel(new Set(names));
        shown = await opsApi.updatesSimulate(n.id, names);
        setSim({ ...shown, picked: [...names].sort().join(" ") });
      } catch (e) {
        showError(e, { action: `Prepare the upgrade on ${n.name}` });
        return;
      } finally {
        setBusy(null);
      }
      if (shown.refused) {
        await alertInfo({ title: securityOnly ? `Can't install the security fixes on ${n.name}` : `Can't upgrade ${n.name}`, body: `Upgrade refused: ${sentence(shown.refused)}` });
        return;
      }
      // The row's count can be out of date (an upgrade from a terminal): apt says there's nothing.
      if (shown.plan?.length === 0) {
        await alertInfo({ title: securityOnly ? `Can't install the security fixes on ${n.name}` : `Upgrade ${n.name}`, body: "Nothing to install." });
        await load([n.id], true);
        return;
      }
    }
    if (!cluster.demo && !shown?.plan) {
      await alertInfo({ title: `Upgrade ${n.name}`, body: DRY_RUN_FIRST });
      return;
    }
    if (!take.length) return;
    const plan = shown?.plan ?? null;
    const count = plan?.length ?? take.length;
    const lines: string[] = [];
    const docker = take.some((p) => kindOf(p) === "Docker");
    const running = n.containers.filter((c) => c.state === "running");
    if (docker) lines.push(dockerLine(row.s?.dockerLiveRestore, running.length));
    if (take.some((p) => kindOf(p) === "NVIDIA")) lines.push("New GPU programs and model loads fail until the node restarts (the driver on disk no longer matches the one loaded); running ones keep going.");
    if (take.some((p) => kindOf(p) === "kernel")) lines.push("The new kernel is used after the next restart.");
    if (take.some((p) => kindOf(p) === "network")) lines.push("Mellanox/DOCA packages can reload the ConnectX-7 driver; the 200G fabric may drop for a few seconds.");
    const multi = deploymentsOf(cluster).some((d) => d.nodes.length > 1 && d.nodes.includes(n.id) && d.members.some((m) => m.running));
    if (multi && (docker || take.some((p) => kindOf(p) === "network"))) lines.push(`${n.name} is part of a model served across several nodes; that model goes down while this runs.`);
    const high = multi && (docker || take.some((p) => kindOf(p) === "network"));
    const everything = !rowUpgrade && !securityOnly && !chosen.length;
    const ok = await confirmAction({
      title: securityOnly ? `Install ${count} security fix${count === 1 ? "" : "es"} on ${n.name}?` : `Upgrade ${count} package${count === 1 ? "" : "s"} on ${n.name}?`,
      body: (
        <div className="ops-confirm">
          <p>Installs exactly what its dry run showed, as root in a systemd unit of its own on {n.name}: apt-get install --only-upgrade of these versions, removing nothing and keeping your changed config files. Just before it starts, SparkBox runs that same command with -s; if apt's plan changed meanwhile, it refuses and nothing runs. It keeps going if SparkBox disconnects or quits; this window follows its log, and Stop before it installs works until dpkg starts installing.</p>
          {plan && <ul className="ops-list mono" aria-label="The plan it installs">{plan.map((p) => <li key={p.name}>{planLine(p)}</li>)}</ul>}
          <p>Saves the node's package list first (dpkg-query -W) to /var/log/sparkbox and the evidence folder. Its alerts are planned while apt runs: 30 minutes from the start, and again each time SparkBox sees it still running; if SparkBox loses the node, they page 30 minutes after it last saw apt running. needrestart restarts nothing, but a package's own scripts may restart its service; afterwards it lists the services still on old libraries.</p>
          {everything && !cluster.demo && <p>Runs only with the GPU stack pinned (Pin the GPU stack…); otherwise it's refused before anything runs as root.</p>}
          {lines.length > 0 && (
            <ul className="ops-impact">
              {lines.map((l, i) => <li key={i} className={`ops-impact-item ${high && i === lines.length - 1 ? "crit" : "warn"}`}><span className="ops-impact-mark" aria-hidden>{high && i === lines.length - 1 ? "!" : "▲"}</span><span>{l}</span></li>)}
            </ul>
          )}
          {rowUpgrade && <p>After installation, SparkBox checks for incomplete packages and reads the pending list again. Held and GPU-stack packages stay unchanged; any remaining updates are listed in the result.</p>}
          {rowUpgrade && row.u!.pending.some((p) => p.held || p.sensitive) && <p className="muted">Left pending: {row.u!.pending.filter((p) => p.held || p.sensitive).map((p) => `${p.name} (${p.held ? "held" : "GPU stack"})`).join(", ")}.</p>}
          {row.u!.held.length > 0 && <p className="muted">Held, so left alone: {row.u!.held.join(", ")}.</p>}
        </div>
      ),
      confirmLabel: securityOnly ? "Install" : "Upgrade",
      danger: true,
      requireTyping: high ? n.name : undefined,
    });
    if (!ok) return;
    if (!(await requestAdmin(`upgrade ${n.name}`))) return;
    setBusy("upgrade");
    try {
      const opts = {
        ...(securityOnly ? { securityOnly: true } : {}),
        ...(plan ? { plan: plan.map((p) => ({ name: p.name, to: p.to })) } : {}),
      };
      // A pick apt deferred (phasing) isn't in the plan, so it isn't asked for.
      const names = take.filter((p) => !plan || plan.some((x) => x.name === p.name)).map((p) => p.name);
      const id = await opsApi.updatesUpgrade(n.id, rowUpgrade || (!securityOnly && chosen.length) ? names : undefined, Object.keys(opts).length ? opts : undefined);
      // The upgrade's own panel takes the dry run's place while it runs.
      setSim(null);
      updatesProgress().then(setRuns, () => {});
      await waitTask(id);
    } catch (e) {
      showError(e, { action: `Upgrade ${n.name}` });
    } finally {
      setBusy(null);
      setSim(null);
      await load([n.id], true);
    }
  };

  const demoCell = (r: NodeRow, value: string | null, title?: string) =>
    r.s?.demo ? <span className="muted">{LIVE_ONLY}</span> : value ? <span className="num" title={title}>{value}</span> : <span className="muted" title={title}>—</span>;

  const nodeColumns: Column<NodeRow>[] = [
    { key: "node", label: "Node", width: 100, priority: 1, sort: (r) => r.node.name, render: (r) => <span className="strong">{r.node.name}</span> },
    { key: "upgrade", label: "Upgrade", help: "ops.updates.upgrade", width: 130, minWidth: 130, priority: 1, render: (r) => {
      const active = runs.some((run) => run.node === r.node.id && run.simulated === cluster.demo && !finished(run));
      const available = r.u?.pending.some((p) => !p.held && !p.sensitive);
      const reason = r.error ?? restartBlocked(r.node) ?? (!r.u ? "Waiting for package status" : !r.u.pending.length ? "No package updates remain" : !available ? "Only held or GPU-stack packages remain" : "Preview and upgrade available packages, leaving held and GPU-stack packages unchanged");
      return <span className="upd-row-action" onMouseDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
        <ToolButton icon={PackageCheck} label="Upgrade" title={reason} locked={!admin} action="updates_upgrade"
          disabled={!available || !!r.error || !!busy || active || !!restartBlocked(r.node)}
          onClick={() => upgrade(false, r)} />
      </span>;
    } },
    {
      key: "pending", label: "Pending", help: "see-pending-updates", width: 80, align: "right", priority: 1, sort: (r) => r.u?.pending.length ?? -1,
      render: (r) => (r.error ? <span className="crit-text" title={r.error}>couldn't read</span> : r.u ? <span className="num">{r.u.pending.length}</span> : <span className="muted">…</span>),
    },
    { key: "security", label: "Security", help: "ops.updates.security", width: 80, align: "right", priority: 1, sort: (r) => r.u?.pending.filter((p) => p.security).length ?? 0, render: (r) => { const n = r.u?.pending.filter((p) => p.security).length ?? 0; return n ? <Badge tone="warn">{n}</Badge> : <span className="muted">0</span>; } },
    { key: "sensitive", label: "Needs care", help: "ops.updates.needs-care", width: 96, align: "right", priority: 2, title: () => "Kernel, NVIDIA, CUDA, Docker or Mellanox packages", sort: (r) => r.u?.pending.filter((p) => p.sensitive).length ?? 0, render: (r) => { const n = r.u?.pending.filter((p) => p.sensitive).length ?? 0; return n ? <Badge tone="info">{n}</Badge> : <span className="muted">0</span>; } },
    {
      key: "driver", label: "NVIDIA driver", priority: 1, minWidth: 140, sort: (r) => r.node.system.driver,
      title: (r) => (r.node.system.driver !== driverMajority && driverMajority ? `Differs from the other nodes (${driverMajority})` : r.node.system.driver),
      render: (r) => (
        <span className={driverMajority && r.node.system.driver && r.node.system.driver !== driverMajority ? "ops-drift" : undefined}>
          <span className="num">{r.node.system.driver || "—"}</span>
          {r.u?.driver && <span className="muted"> · update {r.u.driver.split("→")[1]?.trim()}</span>}
          {driverMajority && r.node.system.driver && r.node.system.driver !== driverMajority && <Badge tone="warn">differs</Badge>}
        </span>
      ),
    },
    {
      key: "kernel", label: "Kernel", priority: 3, minWidth: 150, sort: (r) => r.node.system.kernel,
      render: (r) => <span className={kernelMajority && r.node.system.kernel !== kernelMajority ? "ops-drift mono" : "mono"}>{r.node.system.kernel || "—"}</span>,
    },
    {
      key: "reboot", label: "Restart", help: "ops.updates.restart", width: 170, minWidth: 170, priority: 1, sort: (r) => (r.u?.rebootRequired ? 1 : 0),
      title: (r) => (r.u?.rebootPackages.length ? `Waiting for a restart: ${r.u.rebootPackages.join(", ")}` : ""),
      render: (r) => {
        const task = restartTask(r.node.id);
        const running = task?.state === "running";
        const reason = restartBlocked(r.node);
        return <span className="upd-restart" onMouseDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
          <ToolButton icon={RotateCcw} label={running ? (r.node.status === "online" ? "Restarting…" : "Reconnecting…") : "Restart…"}
            locked={!admin} action="node_power" disabled={!!busy || !!reason}
            title={reason ?? `Restart ${r.node.name}; SparkBox reconnects automatically`} onClick={() => void restart(r.node)} />
          {task ? <button type="button" className="link-plain" onClick={() => openTasksWindow(task.id)} title={task.warning ?? task.error ?? task.summary ?? task.steps.find((s) => s.state === "running")?.label ?? task.title}>
            {running ? "View progress" : task.state === "ok" ? task.warning ? "Restarted · check result" : "Restarted · view result" : "View restart result"}
          </button> : null}
          {!running && (r.u?.rebootRequired ? <Badge tone="warn">needed</Badge> : <span className="muted">{r.u ? "not needed" : "unknown"}</span>)}
        </span>;
      },
    },
    { key: "dgx", label: "DGX OS", help: "ops.updates.dgx-os", priority: 2, minWidth: 110, sort: (r) => r.s?.dgx?.ota ?? "", title: (r) => (r.s?.dgx?.otaDate ? `OTA of ${r.s.dgx.otaDate}${r.s.dgx.build ? ` · build ${r.s.dgx.build}` : ""}` : ""), render: (r) => demoCell(r, dgxCell(r.s)) },
    { key: "firmware", label: "Firmware", help: "ops.updates.firmware", priority: 2, minWidth: 90, title: (r) => r.s?.firmware?.detail ?? (r.s?.firmware?.devices ?? []).map((d) => `${d.name}: ${d.current} → ${d.available}`).join("\n"), render: (r) => demoCell(r, firmwareCell(r.s)) },
    { key: "ollama", label: "Ollama", priority: 2, minWidth: 120, render: (r) => demoCell(r, ollamaCell(r.s)) },
    { key: "engineImages", label: "Engine images", priority: 2, minWidth: 150, render: (r) => r.s?.demo ? "—" : r.s?.engineImages?.length
      ? r.s.engineImages.map((i) => <div key={i.image} title={`${i.image} · checked ${relTime(i.checkedMs)}${i.remoteDigest ? ` · ${i.remoteDigest}` : ""}`}>{i.image}: {i.error ? `unknown (${i.error})` : [i.tagMoved ? "tag moved" : i.tagMoved === false ? "same digest" : "installed digest unknown", i.newerTag ? `newer tag ${i.newerTag}` : null].filter(Boolean).join(" · ")}</div>)
      : r.s?.engineImages ? "No supported engine tags" : "Not checked" },
    { key: "snaps", label: "Snaps", help: "ops.updates.snaps", width: 70, align: "right", priority: 3, title: (r) => (r.s?.snaps ?? []).map((x) => `${x.name}: ${x.current ? `${x.current} → ` : ""}${x.available}`).join("\n"), render: (r) => demoCell(r, r.s?.snaps ? String(r.s.snaps.length) : null) },
    { key: "unattended", label: "Auto-updates", width: 104, priority: 3, render: (r) => (r.u?.unattended === null || !r.u ? <span className="muted">—</span> : r.u.unattended ? "on" : <span className="muted">off</span>) },
    { key: "held", label: "Held", help: "ops.updates.held", priority: 2, minWidth: 90, title: (r) => r.u?.held.join(", ") ?? "", render: (r) => (r.u?.held.length ? <span className="mono">{r.u.held.join(", ")}</span> : <span className="muted">—</span>) },
    // "30 min ago" whole: the column is as wide as its widest value.
    { key: "checked", label: "Checked", fit: "content", priority: 3, sort: (r) => r.u?.checkedMs ?? 0, title: (r) => (r.u?.checkedMs ? new Date(r.u.checkedMs).toLocaleString() : ""), render: (r) => (r.u?.checkedMs ? relTime(r.u.checkedMs) : "—") },
  ];

  const pkgColumns: Column<PendingPackage>[] = [
    {
      key: "name", label: "Package", priority: 1, minWidth: 180, sort: (p) => `${p.sensitive ? 0 : 1}${p.security ? 0 : 1}${p.name}`,
      render: (p) => (
        <span className="ops-pkg">
          <span className="strong">{p.name}</span>
          {p.sensitive && <Badge tone="info" title="Upgrade with care">{kindOf(p) || "sensitive"}</Badge>}
          {p.security && <Badge tone="warn">security</Badge>}
          {p.held && <Badge tone="neutral"><Icon icon={Pin} size={12} /> held</Badge>}
        </span>
      ),
    },
    { key: "ver", label: "From → to", priority: 1, minWidth: 200, title: (p) => `${p.from} → ${p.to}`, render: (p) => <span className="mono">{p.from} → {p.to}</span> },
    { key: "origin", label: "From archive", priority: 3, sort: (p) => p.origin, render: (p) => <span className="muted">{p.origin}</span> },
  ];

  const u = current?.u;
  const locked = admin ? undefined : lockedHint("update nodes");
  // The guard, before anything runs: a GPU-stack selection, or the dry run shown refused.
  const stackChosen = chosen.filter((p) => !p.held && p.sensitive).map((p) => p.name);
  // A dry run speaks for the selection it was for only.
  const simFits = !!sim && sim.node === current?.node.id && sim.picked === picked;
  const refusedWhy = stackChosen.length ? `SparkBox can't upgrade GPU-stack packages yet: ${stackChosen.join(", ")}` : simFits ? sim?.refused ?? null : null;
  // With Live data, Upgrade installs exactly a dry run on screen: one for what's picked now.
  const upgradeWhy = refusedWhy ?? (simFits && sim?.plan?.length === 0 ? "Nothing to install." : !cluster.demo && !(simFits && sim?.plan) ? DRY_RUN_FIRST : null);
  // This data source's runs only (SparkBox keeps Live's while it shows Demo).
  const nodeRun = current ? runs.find((r) => r.node === current.node.id && r.simulated === cluster.demo) : undefined;
  const fixes = securityFixes(u);
  const pinOff = cluster.demo ? LIVE_ONLY : undefined;
  const refreshed = current?.s?.refresh;
  return (
    <div className="col fill ops-window">
      <Toolbar label="Updates">
        <ToolButton icon={RefreshCw} label="Check all nodes" title="apt-get update on every node" disabled={!!busy} locked={!admin} onClick={() => refresh(nodes.map((n) => n.id))} />
        <HelpTip topic="check-for-updates" />
        <ToolSeparator />
        <ToolButton icon={FlaskConical} label="Simulate" title="apt-get with -s: what an upgrade would change, without changing anything. Upgrade then installs exactly that" disabled={!u || !!busy} onClick={simulate} />
        <HelpTip topic="simulate-an-update" />
        <ToolButton icon={PackageCheck} label={chosen.length ? `Upgrade ${chosen.filter((p) => !p.held).length}` : "Upgrade node"} title={upgradeWhy ?? undefined}
          disabled={!u || !u.pending.some((p) => !p.held) || !!busy || !!upgradeWhy} locked={!admin} onClick={() => upgrade(false)} />
        <ToolButton icon={ShieldCheck} label="Upgrade security only" priority={2}
          title={fixes.length ? `Dry-runs, then installs, the security fixes that aren't GPU-stack or held: ${fixes.map((p) => p.name).join(", ")}` : "No security fixes SparkBox may install here"}
          disabled={!fixes.length || !!busy} locked={!admin} onClick={() => upgrade(true)} />
        <HelpTip topic="ops.updates.security-only" />
        {/* A pin, not the padlock: in SparkBox the padlock means Admin mode. */}
        <ToolButton icon={Pin} label="Hold" priority={2} title="apt-mark hold: apt leaves it at this version" disabled={!chosen.some((p) => !p.held) || !!busy} locked={!admin} onClick={() => hold(true)} />
        <HelpTip topic="ops.updates.hold" />
        <ToolButton icon={PinOff} label="Release" priority={2} title="apt-mark unhold" disabled={!chosen.some((p) => p.held) || !!busy} locked={!admin} onClick={() => hold(false)} />
        <ToolButton icon={Pin} label="Pin the GPU stack…" priority={3} title={pinOff ?? "Holds the kernel, NVIDIA, CUDA, Docker and network-stack packages installed on this node"}
          disabled={!current || !!pinOff || !!busy} locked={!admin} action="updates_hold_preset" onClick={() => pinStack(true)} />
        <HelpTip topic="pin-the-gpu-stack" />
        <ToolButton icon={PinOff} label="Release the GPU-stack pin…" priority={3} title={pinOff ?? "Releases the holds the pin set"}
          disabled={!current || !!pinOff || !u?.held.length || !!busy} locked={!admin} action="updates_hold_preset" onClick={() => pinStack(false)} />
        {u?.rebootRequired && current && (
          <ToolButton icon={RotateCcw} label="Restart node…" priority={2} locked={!admin} action="node_power" disabled={!!busy || !!restartBlocked(current.node)} title={restartBlocked(current.node) ?? `Finish installing ${u.rebootPackages.join(", ") || "updates"}`} onClick={() => void restart(current.node)} />
        )}
        <ToolSeparator />
        <ToolButton icon={ExternalLink} label="Open DGX Dashboard" priority={2} locked={!admin} action="tunnel_open" disabled={!!dashboardBlocked(current?.node, cluster.demo)}
          title={dashboardBlocked(current?.node, cluster.demo) ?? `NVIDIA's dashboard for ${current?.node.name}, to look at. Don't install from it: it skips SparkBox's checks.`}
          onClick={() => current && void openOnNode(current.node, DGX_DASHBOARD_PORT, cluster.demo, () => wm.open("settings", { section: "data" }))} />
        <Spacer />
        <SearchField value={text} onChange={setText} placeholder="Filter packages" count={pkgs.length} />
      </Toolbar>
      <RollingUpdates />
      <ProtectedUpdates />
      {driverMajority && nodes.some((n) => n.system.driver && n.system.driver !== driverMajority) && (
        <Notice tone="warn">
          NVIDIA driver drift: {nodes.filter((n) => n.system.driver !== driverMajority).map((n) => `${n.name} runs ${n.system.driver}`).join(", ")} while the rest run {driverMajority}.
          Multi-node models need the same driver everywhere.
        </Notice>
      )}
      {drift.length > 0 && <Notice tone="warn">Hold drift: {drift.join(", ")} {drift.length === 1 ? "holds" : "hold"} different packages from the other nodes.</Notice>}
      {locked && <div className="hint">{locked}</div>}
      <div className="ops-updates">
        <div className="ops-updates-nodes">
          <Table tableId="updates-nodes" ariaLabel="Nodes" columns={nodeColumns} rows={rows} rowKey={(r) => r.node.id}
            selected={current?.node.id ?? null} onSelect={setSelected}
            rowClass={(r) => (r.error ? "row-crit" : r.u?.rebootRequired ? "row-warn" : undefined)}
            empty={loading ? "Asking apt on every node…" : "No nodes."} />
        </div>
        <div className="ops-split">
          <div className="ops-split-list">
            {current?.error ? (
              <Notice tone="error">{current.node.name}: {current.error}</Notice>
            ) : u && u.pending.length === 0 ? (
              <EmptyState icon={ShieldAlert} title={`${current?.node.name} is up to date`} body={u.checkedMs ? `Package lists checked ${relTime(u.checkedMs)}.` : "Check for updates to refresh the package lists."} />
            ) : (
              <Table key={current?.node.id ?? "none"} tableId="updates-packages" ariaLabel="Pending packages" columns={pkgColumns} rows={pkgs} rowKey={(p) => p.name}
                selectionMode="multi" selection={pkgSel} onSelectionChange={(k) => setPkgSel(k)}
                initialSort={{ key: "name", dir: 1 }} footer itemNoun={["package", "packages"]}
                rowClass={(p) => (p.held ? "row-dim" : undefined)}
                empty={u ? "No packages match." : "Reading apt…"} />
            )}
          </div>
          <div className="ops-split-detail">
            {current && nodeRun && (!finished(nodeRun) || !sim) && !(nodeRun.phase === "lost" && sim) ? (
              <UpgradePanel run={nodeRun} name={current.node.name} onCancelled={() => updatesProgress().then(setRuns, () => {})} />
            ) : sim ? (
              <div className="ops-detail" aria-label="Simulated upgrade">
                <div className="ops-detail-head">
                  <h3 className="ops-detail-title">Dry run on {nodes.find((n) => n.id === sim.node)?.name ?? sim.node}</h3>
                  <Badge tone="neutral">nothing changed</Badge>
                </div>
                {!simFits && <p className="muted">This dry run was for {sim.picked ? "the packages picked then" : "everything"}; Simulate again for what's picked now.</p>}
                {sim.refused && <Notice tone="error">Upgrade refused: {sentence(sim.refused)}</Notice>}
                {sim.plan && (
                  <div aria-label="The plan Upgrade installs">
                    <div className="section-title">Upgrade installs exactly this</div>
                    {sim.plan.length > 0 ? <ul className="ops-list mono">{sim.plan.map((p) => <li key={p.name}>{planLine(p)}</li>)}</ul> : <p className="muted">Nothing to install.</p>}
                    <p className="muted">Each installed package at the version shown; new ones as apt picks them. If apt's plan changes before you press Upgrade, SparkBox refuses and nothing runs.</p>
                  </div>
                )}
                {sim.command && <><div className="section-title">The upgrade runs, as root</div><CodeBlock text={sim.command} label="The upgrade's command" /></>}
                {sim.dryRun && <><div className="section-title">Dry run that showed it</div><CodeBlock text={sim.dryRun} label="The dry run's command" /></>}
                <dl className="ops-kv">
                  <div><dt>Upgrades</dt><dd className="num">{sim.upgrades.length}</dd></div>
                  <div><dt>New</dt><dd className="num">{sim.installs.length}</dd></div>
                  <div><dt>Removed</dt><dd className={`num${sim.removals.length ? " crit-text" : ""}`}>{sim.removals.length}</dd></div>
                  <div><dt>Held back</dt><dd className="num">{sim.heldBack.length}</dd></div>
                </dl>
                {sim.removals.length > 0 && <Notice tone="warn">It would remove {sim.removals.join(", ")}.</Notice>}
                {!sim.plan && sim.installs.length > 0 && <><div className="section-title">New packages</div><ul className="ops-list mono">{sim.installs.map((x) => <li key={x}>{x}</li>)}</ul></>}
                {!sim.plan && sim.upgrades.length > 0 && <><div className="section-title">Upgrades</div><ul className="ops-list mono">{sim.upgrades.map((x) => <li key={x}>{x}</li>)}</ul></>}
                {(sim.phased?.length ?? 0) > 0 && <><div className="section-title">Deferred by phasing (not installed this time) <HelpTip topic="ops.updates.phasing" /></div><ul className="ops-list mono">{sim.phased!.map((x) => <li key={x}>{x}</li>)}</ul></>}
                <details className="ops-raw"><summary>apt-get output</summary><CodeBlock text={sim.text} maxHeight={240} label="apt-get -s output" /></details>
              </div>
            ) : (
              <div className="muted pad">
                {current ? `Select packages to upgrade, hold or simulate just those; with none selected, Simulate and Upgrade cover everything pending on ${current.node.name}.` : "Select a node."}
                {u?.rebootRequired && <p>Waiting for a restart: {u.rebootPackages.join(", ") || "installed updates"}.</p>}
                {refreshed && (
                  <div aria-label="Last check for updates">
                    <p>Last check for updates {relTime(refreshed.atMs)}: {refreshed.ok ? "ok" : "failed"}{refreshed.warnings.length + refreshed.errors.length ? "" : ", no warnings"}.</p>
                    {refreshed.errors.length + refreshed.warnings.length > 0 && <ul className="ops-list mono">{[...refreshed.errors, ...refreshed.warnings].map((l, i) => <li key={i}>{l}</li>)}</ul>}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
