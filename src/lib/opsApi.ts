// Tauri wrappers and types for the safety core and node operations (src-tauri/src/admin.rs,
// tasks.rs, live/audit.rs and ops/). Mirrors the Rust structs (serde camelCase).
import { invoke } from "./api";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Listener } from "./types";

// ---- Admin mode

export interface AdminState {
  unlocked: boolean;
  expiresAtMs: number | null;
  idleMinutes: number;
  /** Unlocking asks for Touch ID or the Mac password (Live data). */
  requiresAuth: boolean;
  /** Touch ID or the password confirmed it's you in the last 5 minutes: Mac-scope changes are allowed. */
  authenticated?: boolean;
  /** When the Mac scope runs out (5 minutes after Touch ID; nothing extends it). */
  macExpiresAtMs?: number | null;
  /** How long before the relock its warning shows (1 minute; less with SPARKBOX_IDLE_SCALE). */
  warnBeforeMs?: number;
  /** When this unlock started. */
  unlockedAtMs?: number | null;
  /** Why it last locked by itself, for the notice. */
  lockReason?: string | null;
  /** Look-only mode (SPARKBOX_LOOK_ONLY=1 or Settings › Security): nothing unlocks; the padlock reads "Look only". */
  lookOnly?: boolean;
  /** Look-only was set by SPARKBOX_LOOK_ONLY for this run: Settings can't turn it off. */
  lookOnlyForced?: boolean;
}

/** node: changes a node, a switch or a model · mac: changes state on this Mac (Keychain, VPN, server…). */
export type AdminScope = "node" | "mac";

export interface SecuritySettings {
  idleMinutes: 5 | 15 | 60 | 120 | 240 | 480 | 1440;
  retentionDays: 30 | 90 | 365;
  /** Lock a shell after this many minutes with nothing typed; 0 never. */
  shellIdleMinutes?: 15 | 60 | 240 | 0;
  /** Look-only mode kept across launches; changed only with lookOnlySet. */
  lookOnly?: boolean;
}

export interface CacheGrant { version: number; instance: string; nodes: string[]; grantedMs: number }

// ---- Errors

/** What commands that return SbError reject with. */
export interface SbError {
  kind: "locked" | "needs_auth" | "authRejected" | "sudoRejected" | "noPassword" | "timeout" | "unreachable" | "demoOnly" | "invalid" | "gone" | "cancelled" | "failed" | "lookOnly"
    // The write exit's refusals (live/exit.rs): outside the fence, the other SparkBox's, the wrong machine.
    | "fenced" | "otherController" | "wrongNode";
  message: string;
  node?: string;
  detail?: string;
}

// ---- Tasks

export type TaskState = "running" | "ok" | "failed" | "cancelled";
/** How an operation ended: the same words in Tasks, the activity log's head line and the toast. */
export type Outcome = "ok" | "warning" | "failed" | "cancelled";
export type StepState = "running" | "ok" | "failed" | "skipped";

export interface TaskStep {
  label: string;
  state: StepState;
  startedMs: number;
  finishedMs: number | null;
  detail: string | null;
}

export interface Task {
  id: string;
  kind: string;
  title: string;
  nodes: string[];
  source: string;
  cancellable: boolean;
  state: TaskState;
  startedMs: number;
  finishedMs: number | null;
  progress: number | null;
  detail: string | null;
  rateBps: number | null;
  etaS: number | null;
  steps: TaskStep[];
  log: string[];
  summary: string | null;
  error: string | null;
  /** Finished, but its Verify step found a model that didn't come back. */
  warning?: string | null;
  /** A second line under the title (a Fleet snippet's command). */
  subtitle?: string | null;
  cancelRequested: boolean;
  retry: { command: string; args: Record<string, unknown> } | null;
  /** Benchmarks and checks: cancelling needs no Admin mode. */
  readOnly?: boolean;
  /** How it ended (once finished); its activity-log head line says the same. */
  outcome?: Outcome | null;
  opId: string | null;
  /** Started in the UI (copies, client-side flows) rather than by the backend. */
  client?: boolean;
}

// ---- Activity log

/**
 * session: a shell tab from open to close (kind "terminal") or broadcast typing on/off, sent by
 * the backend; the Activity log window also folds an Admin unlock … relock into one. Sessions
 * aren't changes, except a root shell (escalated). Log and journal tabs are reads.
 */
export type ActivityClass = "probe" | "read" | "change" | "destructive" | "app" | "session";

export interface ActivityEntry {
  id: string;
  tsMs: number;
  updatedMs: number;
  node: string;
  kind: "command" | "sudo" | "terminal" | "stream" | "op" | "app";
  command: string;
  code: number | null;
  ms: number;
  changes: boolean;
  error: string | null;
  opId: string | null;
  title: string;
  source: string;
  actor: "ui" | "tray" | "system" | string;
  escalated: boolean;
  /** unfinished: begun but never ended (SparkBox quit or the connection dropped). warning: done, but its check found a problem. */
  state: "running" | "ok" | "warning" | "failed" | "cancelled" | "unfinished";
  stdoutTail: string | null;
  stderrTail: string | null;
  simulated: boolean;
  class: ActivityClass;
  /** The data source it was recorded under (rows from before round 4 have none). */
  demo?: boolean;
}

// ---- Docker images (ops/images.rs), read only

export interface DockerImage {
  node: string;
  /** Short id, as `docker image ls` prints it. */
  id: string;
  /** "nvcr.io/nvidia/vllm"; "<none>" when dangling. */
  repository: string;
  tag: string;
  /** Bytes on disk (shared layers counted in each image, as Docker reports it). */
  size: number;
  createdMs: number | null;
  /** No tag: left behind by a pull or a build. */
  dangling: boolean;
  /** Containers made from it, running or not. */
  usedBy: string[];
  running: number;
  /** When one of its containers last ran (now while one runs); null when none uses it. */
  lastUsedMs: number | null;
}

export interface NodeImages {
  node: string;
  images: DockerImage[];
  /** Docker's own total for images (docker system df). */
  total: number;
  /** What Docker says could be freed. */
  reclaimable: number | null;
  /** Why this node's images couldn't be read. */
  error: string | null;
}

// ---- Fleet Command

export type FleetStrategy = "parallel" | "oneByOne" | "canary";
export type FleetNodeState = "queued" | "running" | "ok" | "failed" | "timedOut" | "skipped" | "cancelled";

export interface FleetOutputEvent {
  runId: string;
  node: string;
  stream: "stdout" | "stderr";
  chunk: string;
}

export interface FleetNodeEvent {
  runId: string;
  node: string;
  state: FleetNodeState;
  code: number | null;
  ms: number | null;
  error?: string | null;
  truncated?: boolean;
}

export interface FleetResult {
  node: string;
  code: number | null;
  stdout: string;
  stderr: string;
  ms: number;
  error: string | null;
  timedOut: boolean;
  truncated: boolean;
  state: FleetNodeState;
}

export type CommandClass = "probe" | "read" | "change" | "destructive";

export interface Classified {
  class: CommandClass;
  /** Some part runs through sudo, doas or pkexec. */
  escalates: boolean;
  /** Why it's destructive, for the confirmation's banner. */
  reason?: string | null;
  /** Destructive, and SparkBox may not get back in: stops SSH, the network, the firewall, logins. */
  cutsAccess?: boolean;
  /** Restarts or switches off the node; on several nodes at once that cuts access too. */
  power?: "restart" | "off" | null;
  /** Runs with the node's root through Docker (Docker is root-equivalent): Fleet asks for the
   * typed count, runs a canary and one node at a time (P1.42). */
  root?: boolean;
}

// ---- Services

export interface ServiceDetail {
  unit: string;
  description: string;
  loaded: string | null;
  /** "active (running)", "failed (Result: exit-code)"… */
  active: string | null;
  activeSince: string | null;
  mainPid: number | null;
  result: string | null;
  memory: string | null;
  cpu: string | null;
  tasks: string | null;
  statusText: string;
  journal: JournalLine[];
}

export interface JournalLine {
  time: string;
  tsMs: number | null;
  /** syslog priority: 0 emerg … 3 err, 4 warning, 6 info, 7 debug. */
  priority: number;
  text: string;
}

export interface Impact {
  /** low: no dialog · medium: confirm · high: type the node name. */
  tier: "low" | "medium" | "high";
  title: string;
  lines: string[];
  requireTyping: string | null;
  /** Refused outright (e.g. disabling ssh while you're on the VPN without typing the name). */
  blocked: string | null;
  confirmLabel: string;
}

// ---- Updates

export interface PendingPackage {
  name: string;
  from: string;
  to: string;
  origin: string;
  security: boolean;
  sensitive: boolean;
  held: boolean;
}

export interface NodeUpdates {
  node: string;
  pending: PendingPackage[];
  rebootRequired: boolean;
  rebootPackages: string[];
  unattended: boolean | null;
  held: string[];
  /** "580.82.07 → 580.95.05" when an NVIDIA driver update is pending. */
  driver: string | null;
  kernel: string | null;
  checkedMs: number | null;
}

export interface SimulatedUpgrade {
  node: string;
  installs: string[];
  upgrades: string[];
  removals: string[];
  heldBack: string[];
  text: string;
  /** The apt-get the upgrade runs, as root in its own unit. */
  command?: string;
  /** What this dry run ran: the same apt-get with -s. */
  dryRun?: string;
  /** Upgrades apt defers this time (phased updates). */
  phased?: string[];
  /** GPU-stack packages in the plan. */
  sensitive?: string[];
  /** Why this plan can't run on one node (the GPU-stack guard), or null. */
  refused?: string | null;
  /** The plan Upgrade binds to: every package it installs, at the version shown. */
  plan?: PlannedPkg[];
  /** What it would remove (a plan that removes anything is refused). */
  removes?: string[];
}

/** One package of a dry run's plan (ops/updates.rs `PlannedPkg`). */
export interface PlannedPkg {
  /** Without apt's ":arch". */
  name: string;
  /** The version installed now; null for a new dependency. */
  from: string | null;
  to: string;
  /** Not installed yet: left to apt, never pinned. */
  new: boolean;
}

/** A package of the plan shown, as Upgrade sends it back: SparkBox installs exactly this version. */
export interface PlanEntry {
  name: string;
  to: string;
}

/** Upgrade with Live data and no dry run to bind to (ops/updates.rs `DRY_RUN_FIRST`). */
export const DRY_RUN_FIRST = "Run the dry run first: SparkBox installs exactly what it showed.";

/** needrestart's list after an upgrade (`needrestart -b -r l`). */
export interface NeedRestart {
  /** 1: the running kernel is the newest; 2 or 3: a newer one is installed; 0: unknown. */
  kernelStatus: number | null;
  kernelCurrent: string | null;
  kernelExpected: string | null;
  /** Services still running old libraries. */
  services: string[];
}

export interface UpgradeOptions {
  /** The pending security fixes that aren't GPU-stack or held; the packages named are ignored. */
  securityOnly?: boolean;
  /** The plan the dry run showed. Required with Live data: SparkBox installs exactly it, or refuses. */
  plan?: PlanEntry[];
}

/** "Pin the GPU stack" (hold) or its release on one node: the packages, and its task once it ran. */
export interface HoldPreset {
  node: string;
  hold: boolean;
  packages: string[];
  taskId: string | null;
}

export interface FirmwareDevice {
  name: string;
  current: string;
  available: string;
}

export interface SnapUpdate {
  name: string;
  /** Always null today: `snap refresh --list` prints only the new version. */
  current: string | null;
  available: string;
}

/** Everything the background check knows about one node's updates (ops/update_check.rs). */
export interface UpdateSummary {
  node: string;
  checkedMs: number | null;
  error: string | null;
  /** Demo data: only `apt` is filled in. */
  demo: boolean;
  apt: NodeUpdates | null;
  dgx: { build: string | null; ota: string | null; otaDate: string | null; otaPending: string | null } | null;
  firmware: { state: "none" | "available" | "needsSudo" | "unknown"; count: number; devices: FirmwareDevice[]; detail: string | null } | null;
  /** null on a node without Ollama. */
  ollama: { installed: string | null; latest: string | null; newer: boolean } | null;
  snaps: SnapUpdate[] | null;
  engineImages?: { image: string; checkedMs: number; tagMoved: boolean | null; newerTag: string | null; remoteDigest: string | null; error: string | null }[] | null;
  dockerLiveRestore: boolean | null;
  stamps: { listsMs: number | null; dpkgMs: number | null; rebootRequiredMs: number | null };
  securitySinceMs: number | null;
  /** The last "Check all nodes" on it: every W: and E: line apt printed. */
  refresh: { atMs: number; ok: boolean; warnings: string[]; errors: string[] } | null;
}

// ---- Calls

export const opsApi = {
  adminState: () => invoke<AdminState>("admin_state"),
  /** `action` names the command that asked; the backend picks the prompt's words (free text is ignored). */
  adminUnlock: (action?: string, scope: AdminScope = "node") => invoke<AdminState>("admin_unlock", { reason: action ?? null, scope }),
  /** `fresh`: ask even within the Mac scope's 5 minutes, for a command that confirms every time (Keychain writes). */
  adminAuthenticate: (action?: string, fresh = false) => invoke<AdminState>("admin_authenticate", { action: action ?? null, fresh }),
  adminExtend: () => invoke<AdminState>("admin_extend"),
  adminLock: () => invoke<AdminState>("admin_lock"),
  onAdminState: (cb: (s: AdminState) => void): Promise<UnlistenFn> => listen<AdminState>("admin://state", (e) => cb(e.payload)),
  securityGet: () => invoke<SecuritySettings>("security_get"),
  cacheGrantGet: () => invoke<CacheGrant | null>("drop_cache_grant_get"),
  cacheGrantSet: (enabled: boolean) => invoke<CacheGrant | null>("drop_cache_grant_set", { enabled }),
  securitySet: (settings: SecuritySettings) => invoke<SecuritySettings>("security_set", { settings }),
  /** Settings › Security › Look only: on at once; off asks for Touch ID (refused under SPARKBOX_LOOK_ONLY). */
  lookOnlySet: (on: boolean) => invoke<AdminState>("look_only_set", { on }),

  /** Every node's Docker images and Docker's disk totals (docker image ls / system df; read only). */
  imagesList: () => invoke<NodeImages[]>("images_list"),
  tasksList: () => invoke<Task[]>("tasks_list"),
  taskGet: (id: string) => invoke<Task | null>("task_get", { id }),
  taskCancel: (id: string) => invoke<void>("task_cancel", { id }),
  onTask: (cb: (t: Task) => void): Promise<UnlistenFn> => listen<Task>("tasks://update", (e) => cb(e.payload)),

  activityLog: () => invoke<ActivityEntry[]>("activity_log"),
  activitySince: (tsMs: number, limit?: number) => invoke<ActivityEntry[]>("activity_log_since", { tsMs, limit: limit ?? null }),
  activityPath: () => invoke<string | null>("activity_log_path"),
  activityExport: (format: "csv" | "json", fromMs?: number, toMs?: number) => invoke<string>("activity_export", { format, fromMs: fromMs ?? null, toMs: toMs ?? null }),
  onActivity: (cb: (e: ActivityEntry) => void): Promise<UnlistenFn> => listen<ActivityEntry>("activity://entry", (e) => cb(e.payload)),

  /** `cutsAccess`: the plan cuts access (a canary of a power verb meant for several nodes); the backend checks the command too. */
  fleetStart: (args: { nodes: string[]; command: string; sudo: boolean; timeoutS: number; strategy: FleetStrategy; stopOnFailure: boolean; title?: string | null; cutsAccess?: boolean }) =>
    invoke<string>("fleet_start", args),
  fleetCancel: (runId: string) => invoke<void>("fleet_cancel", { runId }),
  fleetResults: (runId: string) => invoke<FleetResult[] | null>("fleet_results", { runId }),
  onFleetOutput: (cb: (e: FleetOutputEvent) => void): Promise<UnlistenFn> => listen<FleetOutputEvent>("fleet://output", (e) => cb(e.payload)),
  onFleetNode: (cb: (e: FleetNodeEvent) => void): Promise<UnlistenFn> => listen<FleetNodeEvent>("fleet://node", (e) => cb(e.payload)),
  classify: (command: string) => invoke<Classified>("ops_classify", { command }),

  servicesApply: (nodes: string[], unit: string, action: string) => invoke<string>("services_apply", { nodes, unit, action }),
  serviceDetail: (node: string, unit: string) => invoke<ServiceDetail>("service_detail", { node, unit }),
  serviceImpact: (nodes: string[], unit: string, action: string) => invoke<Impact>("service_impact", { nodes, unit, action }),

  containersBulk: (items: { node: string; id: string; name: string }[], action: string, removeVolumes: boolean, title?: string) =>
    invoke<string>("containers_bulk", { items, action, removeVolumes, title: title ?? null }),
  portsReveal: (node: string) => invoke<Listener[]>("ports_reveal", { node }),

  updatesStatus: (node: string) => invoke<NodeUpdates>("updates_status", { node }),
  /** The cached update status of each node (all of them when `nodes` is left out); `fresh` re-reads them now (reads only). */
  updatesSummary: (nodes?: string[], fresh?: boolean) => invoke<UpdateSummary[]>("updates_summary", { nodes: nodes ?? null, fresh: fresh ?? null }),
  onUpdatesChanged: (cb: (nodes: string[]) => void): Promise<UnlistenFn> => listen<{ nodes: string[] }>("updates://changed", (e) => cb(e.payload?.nodes ?? [])),
  updatesSimulate: (node: string, packages?: string[]) => invoke<SimulatedUpgrade>("updates_simulate", { node, packages: packages ?? null }),
  updatesHold: (node: string, packages: string[], hold: boolean) => invoke<void>("updates_hold", { node, packages, hold }),
  updatesRefresh: (nodes: string[]) => invoke<string>("updates_refresh", { nodes }),
  updatesUpgrade: (node: string, packages?: string[], opts?: UpgradeOptions) => invoke<string>("updates_upgrade", { node, packages: packages ?? null, opts: opts ?? null }),
  /** Pin the GPU stack (`hold`) or release the pin; `preview` only reads which packages that is. */
  updatesHoldPreset: (node: string, hold: boolean, preview = false) => invoke<HoldPreset>("updates_hold_preset", { node, hold, preview }),
};
