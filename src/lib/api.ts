import { Channel, invoke as tauriInvoke, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { streamChat } from "./chatStream";
import type {
  ActivityEntry, AnalysisResult, Benchmark, ChatEvent, FleetResult, Service, ChatMessage, ChatParams, ClusterSnapshot, Conversation, ConversationSummary, Deployment,
  DeploySpec, HfSearchHit, Inventory, Job, LibraryEntry, ModelDetails, NodeCheck, PreviewStep, Provider, ProviderView,
  Prefs, SelfTestResult, ServerLogEntry, ServerSettings, ServerView, TermEvent, TermTarget, VpnForward,
  VpnMode, VpnStatus, VpnView,
} from "./types";

/** False when the UI is opened in a plain browser (`npm run dev`) instead of the app. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/**
 * A refusal or failure from a command, in one shape whatever the command returns: commands
 * that return SbError reject with {kind, message, node?, detail?}, and commands that return a
 * plain string send their gate's refusal as that object's JSON (SbError::to_json in
 * src-tauri/src/errors.rs). String(e) and e.message are the sentence either way.
 */
export class CommandError extends Error {
  kind: string;
  node?: string;
  detail?: string;
  constructor(e: { kind: string; message: string; node?: string; detail?: string }) {
    super(e.message);
    this.name = "CommandError";
    this.kind = e.kind;
    if (e.node) this.node = e.node;
    if (e.detail) this.detail = e.detail;
  }
  override toString() {
    return this.message;
  }
}

/** `e` as a CommandError when it's an SbError object or its JSON; anything else unchanged. */
export function commandError(e: unknown): unknown {
  if (e instanceof CommandError) return e;
  let v: unknown = e;
  // serde_json may put the keys in any order ({"detail":…,"kind":…}).
  if (typeof e === "string" && e.startsWith("{") && e.includes('"kind"')) {
    try {
      v = JSON.parse(e);
    } catch {
      return e;
    }
  }
  if (v && typeof v === "object" && !(v instanceof Error) && typeof (v as { kind?: unknown }).kind === "string" && typeof (v as { message?: unknown }).message === "string") {
    // Object.assign keeps what a kind adds, such as a SaveError's sha256, mtime and size.
    return Object.assign(new CommandError(v as { kind: string; message: string; node?: string; detail?: string }), v);
  }
  return e;
}

/**
 * Reads whose identical calls may share one promise while it's in flight: `get_…`, and
 * `…_get/_list/_poll/_status/_state/_history/_since/_query/_results/_log/_read/_backups/_inspect/_detail`
 * (every one of them in READ_ONLY or a CONDITIONAL read in admin.rs). Never a command that changes
 * or sends anything (terminal input, a save, menu_set_state), so nothing is lost by sharing.
 */
const SHARED_READ = /^(get_[a-z_]+|[a-z_]+_(get|list|poll|status|state|history|since|query|results|log|read|backups|inspect|detail))$/;
const inFlight = new Map<string, Promise<unknown>>();

/** Whether `cmd` is a read that identical calls in flight share (StrictMode mounts effects twice). */
export const isSharedRead = (cmd: string) => SHARED_READ.test(cmd) && !cmd.includes("set_");

async function call<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args, options);
  } catch (e) {
    throw commandError(e);
  }
}

/**
 * Tauri's invoke, with errors in the one shape above. The safety, ops, files and terminal
 * wrappers use it. Identical reads in flight share one promise (`isSharedRead`).
 */
export function invoke<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  if (options || !isSharedRead(cmd)) return call<T>(cmd, args, options);
  let key: string;
  try {
    key = `${cmd} ${JSON.stringify(args ?? null)}`;
  } catch {
    return call<T>(cmd, args, options);
  }
  const running = inFlight.get(key);
  if (running) return running as Promise<T>;
  const p = call<T>(cmd, args, options);
  inFlight.set(key, p);
  const forget = () => inFlight.delete(key);
  p.then(forget, forget);
  return p;
}

export type ContainerAction = "start" | "stop" | "restart" | "kill" | "pause" | "unpause" | "remove";
export type ProcessSignal = "TERM" | "INT" | "HUP" | "KILL" | "STOP" | "CONT";
export type ServiceAction = "start" | "stop" | "restart" | "reload" | "enable" | "disable" | "reset-failed" | "mask" | "unmask" | "daemon-reload";

/** Keychain account names; see src-tauri/src/secrets.rs. */
export const SSH_ACCOUNT = "ssh";
export const routerOsAccount = (switchId: string) => `routeros:${switchId}`;
export const HF_ACCOUNT = "hf";

export const api = {
  getSnapshot: () => invoke<ClusterSnapshot | null>("get_snapshot"),
  onSnapshot: (cb: (s: ClusterSnapshot) => void): Promise<UnlistenFn> =>
    listen<ClusterSnapshot>("cluster://snapshot", (e) => cb(e.payload)),

  getInventory: () => invoke<Inventory>("get_inventory"),
  saveInventory: (inventory: Inventory) => invoke<void>("save_inventory", { inventory }),

  credentialStatus: (accounts: string[]) =>
    invoke<Record<string, boolean>>("credential_status", { accounts }),
  setCredential: (account: string, secret: string) =>
    invoke<void>("set_credential", { account, secret }),
  clearCredential: (account: string) => invoke<void>("clear_credential", { account }),

  containerAction: (nodeId: string, containerId: string, action: ContainerAction, removeVolumes = false) =>
    invoke<void>("container_action", { nodeId, containerId, action, removeVolumes }),
  containerInspect: (nodeId: string, containerId: string) =>
    invoke<Record<string, unknown>>("container_inspect", { nodeId, containerId }),

  vpnGet: () => invoke<VpnView>("vpn_get"),
  onVpnStatus: (cb: (s: VpnStatus) => void): Promise<UnlistenFn> =>
    listen<VpnStatus>("vpn://status", (e) => cb(e.payload)),
  vpnImport: (confText: string, endpointHost: string) => invoke<VpnView>("vpn_import", { confText, endpointHost }),
  vpnSetEndpoint: (host: string) => invoke<VpnView>("vpn_set_endpoint", { host }),
  vpnSetMode: (mode: VpnMode) => invoke<VpnView>("vpn_set_mode", { mode }),
  vpnSetForwards: (forwards: VpnForward[]) => invoke<VpnView>("vpn_set_forwards", { forwards }),
  vpnReconnect: () => invoke<VpnView>("vpn_reconnect"),
  vpnRemove: () => invoke<VpnView>("vpn_remove"),

  providersList: () => invoke<ProviderView[]>("providers_list"),
  providersUpsert: (provider: Provider) => invoke<ProviderView[]>("providers_upsert", { provider }),
  providersRemove: (id: string) => invoke<ProviderView[]>("providers_remove", { id }),
  providersSetKey: (id: string, key: string) => invoke<ProviderView[]>("providers_set_key", { id, key }),
  providersClearKey: (id: string) => invoke<ProviderView[]>("providers_clear_key", { id }),
  providersRefresh: (id: string) => invoke<string[]>("providers_refresh", { id }),
  providersScan: () => invoke<Provider[]>("providers_scan"),

  /** Streams one reply; resolves when it has finished (or failed). */
  chatSend: (
    request: { requestId: string; provider: string; model: string; system: string; params: ChatParams; messages: ChatMessage[] },
    onEvent: (e: ChatEvent) => void,
  ) => {
    return streamChat(request, onEvent, invoke);
  },
  chatStop: (requestId: string) => invoke<void>("chat_stop", { requestId }),
  conversationsList: () => invoke<ConversationSummary[]>("conversations_list"),
  conversationGet: (id: string) => invoke<Conversation>("conversation_get", { id }),
  conversationSave: (conversation: Conversation) => invoke<void>("conversation_save", { conversation }),
  conversationDelete: (id: string) => invoke<void>("conversation_delete", { id }),

  serverGet: () => invoke<ServerView>("server_get"),
  onServerStatus: (cb: (v: ServerView) => void): Promise<UnlistenFn> =>
    listen<ServerView>("server://status", (e) => cb(e.payload)),
  serverSave: (settings: ServerSettings) => invoke<ServerView>("server_save", { settings }),
  serverStart: () => invoke<ServerView>("server_start"),
  serverStop: () => invoke<ServerView>("server_stop"),
  serverCreateKey: (name: string) => invoke<string>("server_create_key", { name }),
  serverRevokeKey: (name: string) => invoke<ServerView>("server_revoke_key", { name }),
  serverLogs: () => invoke<ServerLogEntry[]>("server_logs"),
  onServerLog: (cb: (e: ServerLogEntry) => void): Promise<UnlistenFn> =>
    listen<ServerLogEntry>("server://log", (e) => cb(e.payload)),
  serverSelfTest: (model: string) => invoke<SelfTestResult[]>("server_self_test", { model }),

  hfSearch: (query: string) => invoke<HfSearchHit[]>("hf_search", { query }),
  hfModel: (repo: string, revision?: string) => invoke<ModelDetails>("hf_model", { repo, revision }),
  hfRefs: (repo: string) => invoke<import("./types").HfRef[]>("hf_refs", { repo }),
  modelsAnalyze: (repo: string, revision?: string) => invoke<AnalysisResult>("models_analyze", { repo, revision }),
  libraryList: () => invoke<LibraryEntry[]>("library_list"),
  jobsList: () => invoke<Job[]>("jobs_list"),
  jobCancel: (node: string, id: string) => invoke<void>("job_cancel", { node, id }),
  jobResumeDownload: (node: string, id: string, expected: import("./types").DownloadResume, review: import("./types").DownloadToolReview) => invoke<Job>("job_resume_download", { node, id, expected, review }),
  modelDownloadTools: (node: string) => invoke<import("./types").DownloadTools>("model_download_tools", { node }),
  modelChainReview: (selection: import("./types").DownloadSelection, source: string, targets: string[]) => invoke<import("./types").ChainReview>("model_chain_review", { selection, source, targets }),
  modelChainStart: (expected: import("./types").ChainReview, tool: import("./types").DownloadToolReview) => invoke<Job[]>("model_chain_start", { expected, tool }),
  modelChainCancel: (node: string, id: string) => invoke<import("./types").ChainCancel[]>("model_chain_cancel", { node, id }),
  modelDownloadCapacity: (selection: import("./types").DownloadSelection, nodes: string[]) => invoke<import("./types").DownloadCapacity>("model_download_capacity", { selection, nodes }),
  modelDiskPreflight: (request: import("./types").DiskRequest) => invoke<import("./types").DiskCheck[]>("model_disk_preflight", { request }),
  modelDownload: (selection: import("./types").DownloadSelection, review: import("./types").DownloadToolReview) => invoke<Job>("model_download", { selection, review }),
  modelShare: (key: string, targets: string[]) => invoke<Job[]>("model_share", { key, targets }),
  modelDelete: (key: string, nodes: string[]) => invoke<void>("model_delete", { key, nodes }),
  clusterCheck: (image?: string, nodes?: string[]) => invoke<NodeCheck[]>("cluster_check", image ? { image, nodes } : undefined),
  deployPreview: (spec: DeploySpec) => invoke<PreviewStep[]>("deploy_preview", { spec }),
  deploy: (spec: DeploySpec) => invoke<Deployment>("deploy", { spec }),
  deploymentsList: () => invoke<Deployment[]>("deployments_list"),
  deploymentUnload: (id: string) => invoke<void>("deployment_unload", { id }),
  deploymentForget: (id: string) => invoke<void>("deployment_forget", { id }),
  benchmarksList: () => invoke<Benchmark[]>("benchmarks_list"),

  /** Opens a terminal on a node; output and the exit arrive through `onEvent`. Resolves to its id. */
  terminalOpen: (node: string, target: TermTarget, cols: number, rows: number, onEvent: (e: TermEvent) => void) => {
    const channel = new Channel<TermEvent>();
    channel.onmessage = onEvent;
    return invoke<string>("terminal_open", { node, target, cols, rows, onEvent: channel });
  },
  terminalInput: (id: string, data: string) => invoke<void>("terminal_input", { id, data }),
  terminalResize: (id: string, cols: number, rows: number) => invoke<void>("terminal_resize", { id, cols, rows }),
  terminalClose: (id: string) => invoke<void>("terminal_close", { id }),

  // Files and the editor have their own wrappers in filesApi.ts.

  /** With `startTime` (from the snapshot) the node checks the PID still belongs to the same process. */
  processSignal: (node: string, pid: number, signal: ProcessSignal, startTime?: number | null) =>
    invoke<void>("process_signal", { node, pid, signal, startTime: startTime ?? null }),
  processRenice: (node: string, pid: number, nice: number) => invoke<void>("process_renice", { node, pid, nice }),
  servicesList: (node: string) => invoke<Service[]>("services_list", { node }),
  serviceAction: (node: string, unit: string, action: ServiceAction) =>
    invoke<void>("service_action", { node, unit, action }),
  serviceStatus: (node: string, unit: string) => invoke<string>("service_status", { node, unit }),
  /** Starts a restart or shutdown (now, or in `delayMin` minutes); resolves to its task's id. */
  nodePower: (node: string, action: "reboot" | "poweroff", delayMin = 0) => invoke<string>("node_power", { node, action, delayMin }),
  fleetRun: (nodes: string[], command: string, sudo: boolean, timeoutS: number) =>
    invoke<FleetResult[]>("fleet_run", { nodes, command, sudo, timeoutS }),

  /** Forwards a local port to `port` on the node over SSH; resolves to the local URL. */
  tunnelOpen: (node: string, port: number) => invoke<string>("tunnel_open", { node, port }),

  activityLog: () => invoke<ActivityEntry[]>("activity_log"),
  activityLogPath: () => invoke<string | null>("activity_log_path"),

  prefsGet: () => invoke<Prefs>("prefs_get"),
  doctorRun: () => invoke<string>("doctor_run"),
  knownHostsForget: () => invoke<void>("known_hosts_forget"),
  /** The menu bar menu asking for a window, e.g. a node or Chat. */
  onTrayOpen: (cb: (e: { kind: string; params: Record<string, string> }) => void): Promise<UnlistenFn> =>
    listen<{ kind: string; params: Record<string, string> }>("tray://open", (e) => cb(e.payload)),
  // ── r2-design ──
  // ── r2-shell ──
  // ── r2-safety ──
  /** Demo ⇄ Live. Mac scope: wrap in withAdmin(…, { scope: "mac", action: "data_source_set" }). */
  dataSourceSet: (source: "demo" | "live") => invoke<Prefs>("data_source_set", { source }),
  /** Closes the SSH tunnel to a node's page (its window closed). */
  tunnelClose: (node: string, port: number) => invoke<void>("tunnel_close", { node, port }),
  /** One masked environment value of a container, in full (node scope; logged). */
  containerReveal: (nodeId: string, containerId: string, name: string) => invoke<string>("container_reveal", { nodeId, containerId, name }),
  /** Open at login (Mac scope; logged). */
  autostartGet: () => invoke<boolean>("autostart_get"),
  autostartSet: (enabled: boolean) => invoke<boolean>("autostart_set", { enabled }),
  // ── r2-fabric ──
  // ── r2-monitor ──
  // ── r2-live ──
  // ── r2-ai ──
  // ── r3-paging ──
  // ── r3-agent ──
  // ── r3-incidents ──
  // ── r3-serving ──
  // ── r3-charts ──
  // ── r3-desk ──
  // ── r3-safety ──
  /** Changes only the given preferences, under one lock (two windows saving at once don't undo each other). Mac scope for dataSource or models. */
  prefsPatch: (patch: Partial<Prefs>) => invoke<Prefs>("prefs_patch", { patch }),
  /** The SSH tunnels open now (Settings › Security & privacy). */
  tunnelsList: () => invoke<{ node: string; port: number; page: string; localPort: number; openedMs: number; lastUsedMs: number; active: number }[]>("tunnels_list"),
  // ── r4-nodes ──
  // ── r4-serving ──
  // ── r4-alerts ──
  // ── r4-agent ──
  // ── r4-safety ──
  // ── r4-demo ──
  // ── r4-craft ──
};
