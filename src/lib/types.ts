// Mirrors src-tauri/src/model.rs and inventory.rs (serde camelCase).

export type LinkStatus = "online" | "connecting" | "offline" | "error";
export type NetRole = "lan" | "fabric" | "wifi";
/** routerOs: the CRS804 (REST API). firewalla: the Switch X. gateway: the Firewalla Gold (router and VPN endpoint). */
export type SwitchKind = "routerOs" | "firewalla" | "gateway";

export interface CpuStats {
  totalPct: number;
  perCore: number[];
  load: [number, number, number];
  tempC: number;
}

export interface MemStats {
  total: number;
  used: number;
  available: number;
  cached: number;
  gpuAlloc: number;
  swapTotal: number;
  swapUsed: number;
}

export interface GpuProc {
  pid: number;
  name: string;
  mem: number;
  container: string | null;
}

export interface GpuStats {
  utilPct: number;
  tempC: number;
  powerW: number;
  smClockMhz: number;
  procs: GpuProc[];
  clockMaxMhz?: number | null;
  powerLimitW?: number | null;
  /** P0 (busiest) … P8 (idle). */
  pstate?: string | null;
  /** Active clock-throttle reasons in plain words, e.g. "HW thermal slowdown". */
  throttle?: string[];
  xidErrors?: number;
}

export interface DiskStats {
  mount: string;
  device: string;
  fsType: string;
  total: number;
  used: number;
  readBps: number;
  writeBps: number;
}

export interface NetStats {
  iface: string;
  role: NetRole;
  up: boolean;
  speedMbps: number;
  rxBps: number;
  txBps: number;
  ip: string | null;
  mtu?: number | null;
  mac?: string | null;
  rxErrors?: number;
  txErrors?: number;
  rxDrops?: number;
  txDrops?: number;
  carrierChanges?: number | null;
  /** The RDMA device (e.g. rocep1s0f1). RoCE traffic bypasses rxBps/txBps and shows up only in rdma*. */
  rdmaDev?: string | null;
  rdmaRxBps?: number | null;
  rdmaTxBps?: number | null;
  /** "<switch id>/<port>", e.g. "crs804/qsfp56-dd-1-1". */
  switchPort?: string | null;
  // ── r2-fabric ──
  /** RoCE and PHY counters (fabric interfaces with an RDMA device). */
  roce?: RoceCounters | null;
  /** Wake-on-LAN on the LAN interface, from ethtool or NetworkManager. */
  lanWol?: WolInfo | null;
}

/** Cumulative RoCE and PHY counters. A counter the driver doesn't expose is null, never 0. */
export interface RoceCounters {
  ecnMarked?: number | null;
  cnpSent?: number | null;
  cnpHandled?: number | null;
  cnpIgnored?: number | null;
  outOfSequence?: number | null;
  packetSeqErr?: number | null;
  localAckTimeoutErr?: number | null;
  impliedNakSeqErr?: number | null;
  rnrNakRetryErr?: number | null;
  duplicateRequest?: number | null;
  rxIcrcErr?: number | null;
  reqCqeErr?: number | null;
  respCqeErr?: number | null;
  portRcvErrors?: number | null;
  portXmitDiscards?: number | null;
  symbolError?: number | null;
  linkDowned?: number | null;
  /** Per priority 0–7. */
  rxPause?: (number | null)[];
  txPause?: (number | null)[];
  rxPauseCtrl?: number | null;
  txPauseCtrl?: number | null;
  rxDiscardsPhy?: number | null;
  rxCrcErrorsPhy?: number | null;
  rxSymbolErrPhy?: number | null;
  rxOutOfBuffer?: number | null;
  /** "pcp" or "dscp", from mlnx_qos. */
  trust?: string | null;
  pfcPrios?: number[] | null;
  roceV2?: boolean | null;
}

export interface SystemInfo {
  collectorVersion?: string | null;
  containerToolkit?: string | null;
  imageStore?: string | null;
  hostname: string;
  model: string;
  os: string;
  kernel: string;
  driver: string;
  cuda: string;
  uptimeS: number;
  bios?: string | null;
  /** ConnectX-7 firmware. */
  cx7Fw?: string | null;
  docker?: string | null;
  ntpSynced?: boolean | null;
  clockOffsetMs?: number | null;
  rebootRequired?: boolean;
}

export interface Container {
  imageId?: string | null;
  id: string;
  name: string;
  image: string;
  state: string;
  startedAt: number;
  finishedAt: number;
  exitCode: number;
  cpuPct: number;
  mem: number;
  gpuMem: number;
  ports: string[];
  project: string | null;
  command: string;
  health?: "healthy" | "starting" | "unhealthy" | null;
  restartCount?: number;
  oomKilled?: boolean;
  restartPolicy?: string | null;
  labels?: Record<string, string>;
  // ── r2-fabric ──
  /** NCCL_* and VLLM_HOST_IP from the environment (secret-looking values redacted). */
  envFlags?: Record<string, string>;
}

export interface NodeSnapshot {
  id: string;
  name: string;
  host: string;
  status: LinkStatus;
  latencyMs: number | null;
  error: string | null;
  cpu: CpuStats;
  mem: MemStats;
  gpu: GpuStats;
  disks: DiskStats[];
  nets: NetStats[];
  system: SystemInfo;
  containers: Container[];
  processes: Process[];
  listeners: Listener[];
  /** No fresh numbers for over 3 s: what's shown is the last frame. */
  stale?: boolean;
  frameAgeMs?: number | null;
  /** Unix ms of the last frame. */
  lastSeenMs?: number | null;
  failedUnits?: string[];
  powerState?: "rebooting" | "shutting-down" | "off" | "waking" | null;
  // ── r2-fabric ──
  /** Recent kernel events, newest last, at most 50. */
  kernelEvents?: KernelEvent[];
  /** The always-on node agent's heartbeat, when one is installed. */
  agent?: AgentBeat | null;
  // ── r3-incidents ──
  /** Changes on every boot; a latched GPU fault clears when it does. */
  bootId?: string | null;
  /** A fatal GPU fault (latched Xid, or GPU readings unreadable). Its endpoints are Down. */
  gpuFault?: GpuFault | null;
  /** Planned work SparkBox started on this node (a restart or shutdown). */
  maintenance?: MaintInfo | null;
}

export interface KernelEvent {
  tsMs: number;
  /** "link": a ConnectX-7 cable or link changing state (health/kernel.rs stamp), information only. */
  kind: "xid" | "mlx5" | "oom" | "hung" | "link";
  /** The Xid number, for kind "xid". */
  code?: number | null;
  text: string;
  severity: "info" | "warn" | "crit";
  process?: string | null;
}

export interface AgentBeat {
  version: string;
  lastBeatMs: number;
  /** The oldest sample the agent still holds. */
  historyFromMs?: number | null;
  /** Rules firing on the node right now. */
  firing?: number;
  // ── r3-paging ──
  /** The settings hash the agent reports. */
  configHash?: string | null;
  /** The reported hash equals the one SparkBox last pushed. */
  configCurrent?: boolean | null;
  /** Why the last settings push to this node failed, while it's pending. */
  pushError?: string | null;
  /** When it failed. */
  pushErrorMs?: number | null;
  lastCycleMs?: number | null;
  /** Peers nothing answers on, as this agent sees them. */
  peersMissing?: string[];
  /** "fabric" or "network" when this agent can't see its peers itself. */
  isolated?: string | null;
  /** Endpoints this agent probes with a one-token check. */
  endpointsProbed?: string[];
  lastDelivery?: DeliveryBeat | null;
  /** The dead-man's switch answered this agent's last ping at this time. */
  heartbeatOkMs?: number | null;
  outboxPending?: number;
  // ── r3-agent ──
  /** This agent leads and checks the fabric switch's management address. */
  switchWatched?: boolean;
}

/** One of a node's busiest processes (top CPU, plus every GPU user and listener). */
export interface Process {
  pid: number;
  name: string;
  user: string;
  cmd: string;
  cpuPct: number;
  rss: number;
  nice: number;
  container: string | null;
  unit: string | null;
  ports: number[];
  gpuMem: number;
  /** R running, S sleeping, D waiting on IO, T stopped, Z zombie. */
  state?: string | null;
  ppid?: number | null;
  threads?: number | null;
  /** Clock ticks after boot. */
  startTime?: number | null;
  cpuTimeS?: number | null;
}

export interface Listener {
  port: number;
  addr: string;
  pid: number | null;
  process: string | null;
}

export interface SwitchPort {
  name: string;
  comment: string;
  running: boolean;
  disabled: boolean;
  speed: string;
  rxBps: number;
  txBps: number;
  rxErrors: number;
  txErrors: number;
  moduleTempC: number | null;
  /** The weakest lane's receive power. */
  rxPowerDbm: number | null;
  node?: string | null;
  mtu?: number | null;
  linkDowns?: number;
  lastLinkUpMs?: number | null;
  rxDrops?: number;
  txDrops?: number;
  fcsErrors?: number;
  rxPause?: number;
  txPowerDbm?: number | null;
  laneRxPowerDbm?: number[];
  sfpVendor?: string | null;
  sfpPart?: string | null;
  /** Mb/s; 200000 for a 200G port. */
  capacityMbps?: number;
  cablingOk?: boolean | null;
  // ── r2-fabric ──
  /** The node the cabling baseline expects here, when it differs from what's seen. */
  expectedNode?: string | null;
  /** How `node` was found. */
  mappedBy?: "mac" | "lldp" | "comment" | null;
  /** Why this port carries SparkBox's own path to the switch (Disable and Bounce need it typed out). */
  management?: string | null;
  /** A disable waiting to revert on the switch: when it will (Unix ms). */
  revertAtMs?: number | null;
}

/** A switch health reading: a temperature, a fan or a power supply. */
export interface Sensor {
  name: string;
  value: number;
  /** "°C", "RPM", "V", "W" or "" for a plain ok/fail state. */
  unit: string;
  ok: boolean;
}

export interface SwitchSnapshot {
  id: string;
  name: string;
  kind: SwitchKind;
  host: string;
  status: LinkStatus;
  latencyMs: number | null;
  version: string | null;
  cpuPct: number | null;
  tempC: number | null;
  uptimeS: number | null;
  ports: SwitchPort[];
  /** False until an address is set in Settings; then status is always "offline" and the UI shows "Not set up". */
  configured?: boolean;
  board?: string | null;
  sensors?: Sensor[];
  lossPct?: number | null;
  error?: string | null;
}

export interface ClusterSnapshot {
  versions?: VersionReport | null;
  tsMs: number;
  demo: boolean;
  nodes: NodeSnapshot[];
  switches: SwitchSnapshot[];
  /** The demo scenario being played (Demo only). */
  scenario?: string | null;
  // ── r2-fabric ──
  /** Where the scenario is in its cycle (Demo only). */
  scenarioPhase?: ScenarioPhase | null;
  // ── r1-h ──
  /** Whether this Mac can reach the cluster at all (health/reach.rs); Live only. */
  macReach?: MacReach | null;
}

export interface ScenarioPhase {
  phase: "fault" | "recovering" | "healthy";
  /** "Node offline · recovering" */
  label: string;
  /** Seconds until the next phase, when the scenario moves on by itself. */
  nextChangeS?: number | null;
}

export interface FabricLink { iface: string; ip: string; rdma: string }
export type VersionState = "matching" | "mismatched" | "unknown";
export interface VersionRow {
  key: string;
  label: string;
  state: VersionState;
  values: { node: string; value: string | null; reason: string | null }[];
  note: string | null;
}
export interface VersionReport { state: VersionState; rows: VersionRow[] }
export interface FabricExpectations { links: FabricLink[]; headCandidate: boolean | null }

export interface NodeConfig {
  id: string;
  name: string;
  host: string;
  ip: string | null;
  model: string;
  port: number;
  user: string;
  fabric?: FabricExpectations | null;
}

export interface SwitchConfig {
  id: string;
  name: string;
  kind: SwitchKind;
  host: string;
  user: string | null;
}

export interface Inventory {
  nodes: NodeConfig[];
  switches: SwitchConfig[];
}

// --- VPN (mirrors src-tauri/src/vpn/mod.rs)

export type VpnMode = "auto" | "always" | "off";
export type VpnState = "off" | "direct" | "connecting" | "connected" | "error";

export interface VpnProfile {
  protocol: "wireguard" | "amneziawg";
  endpointHost: string;
  endpointPort: number;
  addresses: string[];
  dns: string[];
  mtu: number | null;
  peerPublicKey: string;
  hasPresharedKey: boolean;
  allowedIps: string[];
  keepalive: number | null;
  obfuscation: Record<string, string>;
  importedAt: number;
}

export interface VpnForward {
  label: string;
  localPort: number;
  target: string;
  enabled: boolean;
}

export interface VpnSettings {
  mode: VpnMode;
  profile: VpnProfile | null;
  forwards: VpnForward[];
  /** "Direct at home": Auto never starts the tunnel while this Mac is on the nodes' LAN (P1.7). */
  homePin?: boolean;
}

export interface VpnStatus {
  state: VpnState;
  protocol: string | null;
  endpoint: string | null;
  handshakeAgeSec: number | null;
  rxBytes: number;
  txBytes: number;
  latencyMs: number | null;
  /** Plain-English state; never contains the endpoint. */
  message: string | null;
  forwards: { localPort: number; active: boolean; error: string | null }[];
  /** When the current Direct or Connected state began (Unix ms). */
  upSinceMs?: number | null;
  /** Tunnel throughput in bits per second. */
  rxRateBps?: number;
  txRateBps?: number;
  /** Set while the user has overridden the mode for now. */
  override?: "connect" | "disconnect" | null;
  /** The Mac's network, e.g. "Wi-Fi Home · gateway 192.168.50.1". */
  network?: string | null;
}

/** One line of the VPN connection log. */
export interface VpnLogEntry {
  tsMs: number;
  kind: "state" | "network" | "action" | "helper";
  state: VpnState | null;
  text: string;
}

/** Whether a node (or the Gold) answers, directly or through the tunnel. */
export interface VpnReach {
  id: string;
  name: string;
  /** "ip:port" */
  addr: string;
  via: "direct" | "tunnel";
  ok: boolean;
  rttMs: number | null;
  error: string | null;
  checkedMs: number;
}

export interface VpnView {
  settings: VpnSettings;
  status: VpnStatus;
  /** The last 50 changes, oldest first. */
  log?: VpnLogEntry[];
  reach?: VpnReach[];
  rates?: { tsMs: number; rxBps: number; txBps: number }[];
}

// --- AI: providers, chat, server (mirrors src-tauri/src/llm and src-tauri/src/server)

export type ProviderKind = "anthropic" | "openai" | "openaiCompatible" | "demo";

export interface Provider {
  id: string;
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  enabled: boolean;
  models: string[];
  defaultModel: string | null;
}

export interface ProviderView extends Provider {
  hasKey: boolean;
}

export interface ChatStats {
  ttftMs: number | null;
  totalMs: number;
  inputTokens: number | null;
  outputTokens: number | null;
  tokensPerSec: number | null;
}

export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
  thinking?: string | null;
  /** Claude's raw content blocks; sent back unchanged on later turns. */
  anthropicBlocks?: unknown;
  provider?: string | null;
  model?: string | null;
  stopReason?: string | null;
  stats?: ChatStats | null;
  createdAt: number;
}

export interface ChatParams {
  effort?: string | null;
  temperature?: number | null;
  maxTokens?: number | null;
}

export interface Conversation {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  provider: string;
  model: string;
  system: string;
  params: ChatParams;
  messages: ChatMessage[];
}

export interface ConversationSummary {
  id: string;
  title: string;
  updatedAt: number;
  provider: string;
  model: string;
  messageCount: number;
}

export type ChatEvent =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "notice"; text: string }
  | { type: "done"; message: ChatMessage }
  | { type: "error"; message: string };

export interface ServerSettings {
  port: number;
  requireKey: boolean;
  autostart: boolean;
  aliases: Record<string, string>;
}

export interface ServerView {
  settings: ServerSettings;
  running: boolean;
  url: string | null;
  error: string | null;
  clients: { name: string; createdAt: number; preview: string }[];
}

export interface ServerLogEntry {
  tsMs: number;
  client: string;
  api: string;
  model: string;
  provider: string;
  status: number;
  ms: number;
  stream: boolean;
  error: string | null;
}

// --- App preferences and diagnostics (mirrors src-tauri/src/prefs.rs, server self-test)

export type DataSource = "demo" | "live";

export interface ModelSettings {
  /** Where models live on every node; "~" is each node's home. */
  modelRoot: string;
  image: string;
  advisorProvider: string;
  advisorModel: string;
}

export interface Prefs {
  keepInMenuBar: boolean;
  dataSource: DataSource;
  models: ModelSettings;
  /** Mask public IPs and the home folder for screenshots. */
  screenshotSafe?: boolean;
  hardwareAutoLookup?: boolean;
}

// ---- Models (src-tauri/src/models)

export interface HfSearchHit {
  id: string;
  downloads: number;
  likes: number;
  pipeline: string | null;
  tags: string[];
  lastModified: string | null;
}

export interface DownloadSelection {
  repo: string;
  revision: string;
  files: HfModel["files"];
}
export interface HfRef { name: string; kind: "branch" | "tag"; revision: string }

export interface HfModel {
  configError?: string | null;
  id: string;
  revision: string;
  gated: boolean;
  license: string | null;
  pipeline: string | null;
  library: string | null;
  params: number | null;
  dtypes: Record<string, number>;
  weightBytes: number;
  weightsFormat: string;
  quantization: string | null;
  architectures: string[];
  modelType: string | null;
  files: { path: string; size: number | null; sha256: string | null }[];
  readmeExcerpt: string;
}

export interface ArchFacts {
  layers: number;
  kvLayers: number;
  heads: number;
  kvHeads: number;
  headDim: number;
  mlaDim: number | null;
  experts: number | null;
  expertsActive: number | null;
  maxContext: number | null;
}

export interface FitRow {
  nodes: number;
  weightsPerNode: number;
  budgetPerNode: number;
  kvRoomPerNode: number;
  fits: boolean;
  tensorParallelOk: boolean;
  pipelineParallelOk: boolean;
  kvCapacityTokens: number | null;
  decodeCeilingTps: number | null;
}

export interface ModelDetails {
  model: HfModel;
  arch: ArchFacts;
  kvBytesPerToken: number | null;
  fit: FitRow[];
  nodeMemory: number;
  nodesAvailable: number;
}

export type Verdict = "good" | "workable" | "not_recommended" | "wont_fit";

export interface Analysis {
  verdict: Verdict;
  summary: string;
  recommended: {
    nodes: number;
    parallelism: "single" | "tensor" | "pipeline" | "tensor+pipeline";
    tensorParallelSize: number;
    pipelineParallelSize: number;
    expertParallel: boolean;
    maxModelLen: number;
    gpuMemoryUtilization: number;
    extraVllmArgs: string[];
    why: string;
  };
  alternatives: { nodes: number; parallelism: string; tradeoff: string }[];
  expectedPerformance: string;
  engineSupport: { status: "supported" | "likely" | "unknown" | "unsupported"; notes: string };
  risks: string[];
  betterVariant: { repo: string; reason: string };
}

export interface AnalysisResult {
  analysis: Analysis;
  provider: string;
  model: string;
}

export interface LibraryEntry {
  /** The folder's absolute path, the same on every node with a copy. */
  key: string;
  repo: string;
  revision: string;
  format: string;
  sizeBytes: number;
  root: string;
  copies: Record<string, { path: string; sizeBytes: number; verified: boolean; partial?: "downloading" | "copying" | "stopped"; check?: "sha256" | "source-sha256" | "sizes" | "" }>;
}

export type JobState = "running" | "done" | "failed" | "cancelled";

export interface DownloadTool {
  kind: "host" | "managed" | "install" | "image";
  path: string;
  fingerprint: string;
  label: string;
}
export interface DownloadToolReview {
  node: string;
  tool: DownloadTool;
  installConsent: boolean;
}
export interface DownloadTools {
  node: string;
  options: DownloadTool[];
  notes: string[];
}

export interface DownloadResume {
  repo: string;
  revision: string;
  dest: string;
  planHash: string;
}

export interface Job {
  chain?: DownloadChain | null;
  id: string;
  node: string;
  kind: "download" | "serve" | "pull";
  state: JobState;
  step: string;
  message: string;
  label: string;
  bytesDone: number;
  bytesTotal: number;
  rateBps: number;
  startedAt: number;
  updatedAt: number;
  resume?: DownloadResume | null;
  toolUsed?: string;
  toolVersion?: string;
}

/** A ConnectX-7 half as `sparkjob.py check` reads it from sysfs. */
export interface FabricIf {
  iface: string;
  ip: string;
  prefix: number;
  speedMbps: number;
  rdma: string;
  mtu?: number | null;
  operstate?: string | null;
  /** /sys/class/infiniband/<dev>/ports/1/state: "4: ACTIVE". */
  rdmaState?: string | null;
  /** /sys/class/infiniband/<dev>/ports/1/phys_state: "5: LinkUp". */
  rdmaPhys?: string | null;
}

export interface NodeCheck {
  imageRef?: string;
  parsers?: { imageId: string; tools: string[] | null; reasoning: string[] | null } | null;
  node: string;
  error: string | null;
  home: string;
  rootPath: string;
  rootExists: boolean;
  freeBytes: number;
  docker: boolean;
  rsync: boolean;
  python: string;
  imagePresent: boolean;
  vllmVersion: string;
  fabric: FabricIf[];
  /** nvidia-smi -L's first line inside the image, "" when it sees no GPU; null when it wasn't asked (no image). */
  gpuInContainer?: string | null;
  /** When this node was read. */
  checkedAtMs?: number;
}

export interface DeploySpec {
  id: string;
  servedName: string;
  modelPath: string;
  nodes: string[];
  tensorParallel: number;
  pipelineParallel: number;
  expertParallel: boolean;
  maxModelLen: number | null;
  gpuMemoryUtilization: number;
  port: number;
  image: string;
  extraArgs: string[];
}

export type Stage =
  | { kind: "starting" }
  | { kind: "joining"; have: number; need: number }
  | { kind: "compiling"; what: string }
  | { kind: "loading"; percent: number }
  | { kind: "warming" }
  | { kind: "ready" };

export interface Deployment {
  spec: DeploySpec;
  endpoint: string;
  state: "starting" | "ready" | "failed" | "stopped";
  stage: Stage | null;
  message: string;
  logTail: string;
  startedAt: number;
  providerId: string;
  demo: boolean;
}

export interface PreviewStep {
  node: string;
  title: string;
  command: string;
}

export interface BenchRow {
  concurrency: number;
  completed: number;
  ttftMsMedian: number;
  tokensPerSecPerStream: number;
  aggregateTokensPerSec: number;
}

export interface Benchmark {
  deployment: string;
  servedName: string;
  nodes: string[];
  tensorParallel: number;
  pipelineParallel: number;
  at: number;
  rows: BenchRow[];
  demo: boolean;
}

export interface SelfTestResult {
  api: string;
  ok: boolean;
  status: number;
  ms: number;
  detail: string;
}

// ---- Terminals (src-tauri/src/term)

export type TermTarget =
  | { kind: "shell" }
  | { kind: "containerShell"; container: string }
  | { kind: "containerLogs"; container: string }
  | { kind: "journal"; unit: string };

export type TermEvent =
  | { kind: "data"; data: string }
  | { kind: "exit"; code: number | null; message: string | null }
  // ── r2-live ──
  /** Output dropped (oldest first) while the window wasn't taking any. */
  | { kind: "skipped"; bytes: number }
  /** Admin mode relocked: the shell takes no input until it's authorized again. */
  | { kind: "locked"; reason: string }
  | { kind: "unlocked" }
  // ── r3-safety ──
  /** The shell locks in a minute unless something is typed. */
  | { kind: "idleWarning"; text: string };

// ---- Files (src-tauri/src/files)

export interface FileEntry {
  name: string;
  kind: "file" | "dir" | "link";
  size: number;
  mtime: number;
  mode: string;
  target: string | null;
  linkDir: boolean;
}

export interface Listing {
  /** "mac" or a node id. */
  place: string;
  path: string;
  entries: FileEntry[];
}

export type TransferEvent =
  | { kind: "progress"; done: number; total: number }
  | { kind: "done"; bytes: number }
  | { kind: "failed"; message: string };

// ---- Services and Fleet Command (src-tauri/src/ops)

export interface Service {
  unit: string;
  description: string;
  load: string;
  active: string;
  sub: string;
  enabled: string;
}

export interface FleetResult {
  node: string;
  code: number | null;
  stdout: string;
  stderr: string;
  ms: number;
  error: string | null;
}

export interface ActivityEntry {
  tsMs: number;
  node: string;
  kind: "command" | "sudo" | "terminal" | "stream";
  command: string;
  code: number | null;
  ms: number;
  changes: boolean;
  error: string | null;
}

// Types other packages add (optional fields; append under your marker).
// ── r2-design ──
// ── r2-shell ──
// ── r2-monitor ──
// ── r2-ai ──
export interface ModelSettings {
  /** The SGLang container for SGLang loads (Settings › Models). */
  sglangImage?: string;
}
export interface Provider {
  /** Plain http is all right for this provider's private address on the home network. */
  allowHttpLan?: boolean;
}
// ── r2-live ──
// ── r2-safety ──
// ── r2-fabric ──
// ── r3-incidents ──
export interface GpuFault {
  /** The Xid number, for source "xid". */
  code?: number | null;
  /** "GPU has fallen off the bus (Xid 79)" */
  text: string;
  sinceMs: number;
  source: "xid" | "unreadable";
}
/** SparkBox restarted or shut down this node on purpose. */
export interface MaintInfo {
  /** When the node should be back; null for a shutdown (until Wake). */
  untilMs?: number | null;
  /** "restarting" or "shut down" */
  reason: string;
  startedMs: number;
}
// ── r3-paging ──
/** An agent's latest delivery attempt. */
export interface DeliveryBeat {
  ms: number;
  ok: boolean;
  channel: string;
}
// ── r3-agent ──
// ── r3-serving ──
// ── r3-charts ──
// ── r3-desk ──
// ── r3-safety ──
// ── r4-nodes ──
export interface MemStats {
  /** /proc/pressure/memory avg10 (%): time some (or all) tasks waited on memory. Null when not exposed. */
  psiSome10?: number | null;
  psiFull10?: number | null;
  /** Bytes read from swap per second; unknown on first reading or counter reset. */
  swapInBps?: number | null;
  /** /proc/pressure/io avg10 (%). */
  psiIoSome10?: number | null;
  psiIoFull10?: number | null;
}
export interface GpuStats {
  /** False when nvidia-smi gave no numbers this frame (the numbers are then 0, not readings). Default true. */
  readable?: boolean;
  /** Why the GPU couldn't be read. */
  error?: string | null;
}
export interface Process {
  /** The kernel's 15-character name; `name` is the full one from argv[0]. */
  comm?: string | null;
  /** A kernel thread (kworker, ksoftirqd…), hidden unless asked for. */
  kernel?: boolean;
  /** SparkBox's own collector and the commands it runs. */
  sparkbox?: boolean;
}
export type ListenerScope = "local" | "all" | "lan" | "fabric";
export interface Listener {
  /** Who can reach it: loopback only, every interface, the LAN, or the fabric. */
  scope?: ListenerScope | null;
  /** The same port on v4 and v6 is one row ("both"). */
  family?: "v4" | "v6" | "both" | null;
}
/** Wi-Fi as `iw dev <if> link` reports it without root; null where iw printed nothing. */
export interface WifiInfo {
  ssid?: string | null;
  signalDbm?: number | null;
  bitrateMbps?: number | null;
}
/** Wake-on-LAN as far as SparkBox can read it without root. */
export interface WolInfo {
  state: "on" | "off" | "unknown";
  /** ethtool | networkmanager | none */
  source?: string;
  /** "On (NetworkManager: magic packet)", "Not readable without sudo (ethtool needs root)"… */
  detail?: string;
  /** ethtool's Wake-on letters when ethtool could read them. */
  raw?: string | null;
}
export interface LldpNeighbor {
  system?: string | null;
  port?: string | null;
}
export interface NetStats {
  /** Wi-Fi association (role "wifi" only). `speedMbps` is then the tx bitrate, or 0 when unknown (shown as "—"). */
  wifi?: WifiInfo | null;
  /** The switch port LLDP reports on the other end. */
  lldp?: LldpNeighbor | null;
}
/** What `systemctl show` adds to a unit (services_poll): what tells an idle unit from a stopped one. */
export interface Service {
  /** simple | exec | forking | oneshot | dbus | notify… */
  kind?: string;
  /** What starts it, e.g. ["anacron.timer"]. */
  triggeredBy?: string[];
  result?: string;
  /** False when its start condition wasn't met: skipped, not stopped. */
  condition?: boolean | null;
}
export type NodeErrorKind = "hostKeyUnknown" | "hostKeyChanged" | "authFailed" | "timeout" | "refused" | "unreachable" | "dns";
/** A node's SSH host key as SparkBox sees it. */
export interface HostKeyInfo {
  /** host:port as pinned in known_hosts. */
  address: string;
  /** "SHA256:wLFU…" */
  fingerprint: string;
  /** Other nodes pinned to the same key (a cloned image). */
  sharedWith?: string[];
}
export interface NodeSnapshot {
  /** Every process on the node, before the list was cut to the ones sent. */
  procTotal?: number;
  /** Why the connection failed, as a kind; `error` keeps the detail. */
  errorKind?: NodeErrorKind | null;
  /** The node's SSH host key, once seen. */
  hostKey?: HostKeyInfo | null;
}
// ── r4-serving ──
/** Why a failed deployment failed, as far as its head's log tells (models/postmortem.rs). */
export interface DeploymentPostmortem {
  /** "No attention kernel for a bf16 KV cache on GB10" (hedged), when a known cause matched. */
  title: string | null;
  cause: string | null;
  explanation: string | null;
  fix: string | null;
  /** The decisive lines in log order: up to two lines of context, the root-cause exception last. */
  lines: string[];
  /** The decisive line as the log has it (Show log finds it). */
  logLine: string | null;
  /** "~/glm53/inner.sh line 22: --kv-cache-dtype bfloat16" */
  scriptLine: string | null;
  scriptPath?: string | null;
  scriptLineNumber?: number | null;
  retryable?: boolean;
  readMs: number;
  logNode: string;
  logContainer: string;
  vllm: string | null;
  tp: number | null;
  kvDtype: string | null;
  error: string | null;
}
export interface ClusterDeploymentMember {
  lastSeenMs?: number;
  unavailableReason?: string;
  node: string;
  container: string;
  id: string;
  rank: number | null;
  role: "head" | "worker";
  state: string;
  exitCode: number;
  oom: boolean;
  startedMs: number;
  finishedMs: number;
}
/** A deployment on the cluster, inferred from its containers or SparkBox's own (models/postmortem.rs). */
export interface ClusterDeployment {
  /** The shared container name ("glm53"), the label or project, else the image's short name. */
  id: string;
  /** The model it runs ("GLM-5.3-Flash-NVFP4"), else its served name, else the id. Use deploymentLabel(). */
  label: string;
  served: string | null;
  engine: "vllm" | "sglang" | "other";
  image: string;
  model: string | null;
  /** In rank order, head first. */
  nodes: string[];
  members: ClusterDeploymentMember[];
  state: "running" | "failed" | "stopped" | "partial" | "unknown";
  /** SparkBox worked the grouping out: say "likely" and give `how`. */
  inferred: boolean;
  how: string;
  startedMs: number;
  finishedMs: number | null;
  /** How long its head ran. */
  ranS: number | null;
  exitCode: number | null;
  postmortem: DeploymentPostmortem | null;
  demo: boolean;
}
// ── r4-alerts ──
// ── r4-agent ──
// ── r4-safety ──
// ── r4-demo ──
// ── r4-craft ──

// ── r1-h ──
/** health/reach.rs: what the cluster-path monitor makes of this Mac's own connectivity. */
export type MacReachState = "ok" | "lostHomeNetwork" | "lanBetween" | "wiredSwitch" | "vpnProblem" | "noRoute" | "waking";

export interface MacReach {
  state: MacReachState;
  sinceMs: number;
  /** "This Mac can't reach any node; the LAN between them?"; empty when ok. */
  title: string;
  /** One sentence; empty when ok. */
  detail: string;
  gateway?: string | null;
  interface?: string | null;
}

// ── r1-c ──
export interface DiskStats {
  /** What a normal user can still write (df's Available, frame 6; 0 on a disk full for all but root); null or absent from an older collector. */
  avail?: number | null;
}
export interface LldpNeighbor {
  /** The chassis and port ids as sent (a MAC in lower case, or a name). */
  chassisId?: string | null;
  portId?: string | null;
}
export interface NetStats {
  /** Every LLDP neighbour heard here that isn't this node: a switch, or another node through one. */
  lldpPeers?: LldpNeighbor[];
  /** Only this node itself was heard (each ConnectX-7 half hears its sibling through the switch). */
  lldpSelfOnly?: boolean;
}

// ── R2-CHATMODELS ──
export interface ProviderView {
  /** A cluster model that left Ready in the last two minutes: still listed so a chat with it isn't lost, not offered for a new pick. */
  notAnswering?: boolean;
  /** Visible models that take a reasoning effort instead of a temperature (Claude, OpenAI's gpt-5 and o-series). */
  effortModels?: string[];
}

// ── p1r3-check ──
export interface SystemInfo {
  /** /etc/dgx-release: DGX_SWBUILD_VERSION ("7.2.3"). */
  dgxBuild?: string | null;
  /** The last DGX_OTA_VERSION ("7.6.0") and its DGX_OTA_DATE, as the file gives it. */
  dgxOta?: string | null;
  dgxOtaDate?: string | null;
}
export interface NodeSnapshot {
  /** What the update check last found on the node (Live only). */
  updates?: UpdateFacts | null;
}
/** A node's pending updates as the sidebar badge and the update alert rules read them. */
export interface UpdateFacts {
  pending: number;
  security: number;
  /** When SparkBox first saw security updates pending. */
  securitySinceMs?: number | null;
  rebootRequired: boolean;
  /** /var/run/reboot-required's mtime. */
  rebootSinceMs?: number | null;
  /** When the package lists were last refreshed. */
  listsMs?: number | null;
  checkedMs: number;
  /** Firmware updates fwupd offers (0: none); null when it couldn't say. */
  firmware?: number | null;
  ollamaNewer: boolean;
  engineImages?: number | null;
  /** The DGX OS OTA version apt offers. */
  otaPending?: string | null;
  /** Read before the node last (re)connected and not checked since: no alert is raised from it. */
  unconfirmed?: boolean;
  /** The security updates among them in the GPU stack; null when not known. */
  securityGpu?: number | null;
}

// ── p1r3-foot ──
export interface NodeSnapshot {
  /** How many processes the node reported, where the list in each second's snapshot is cut to the 15 busiest and the listeners' owners (processes_get has them all). */
  processCount?: number | null;
}

// ── p1r4-load ──
export interface DeploySpec {
  /** The image picked for a single-node vLLM load, by name. */
  imageRef?: string;
  /** Its local id on the node (sha256:…), filled in by SparkBox from the pre-flight. */
  imageId?: string;
  /** The owner's consent to drop the page cache first when vLLM would otherwise refuse to start. */
  dropCaches?: boolean;
}
// ── p2r1 ──
export interface DeploySpec {
  /** The recipe's id, empty for the generic one. */
  recipe?: string;
  /** Filled in by Rust from the recipe at deploy, never by the page, so Restart rebuilds the same files. */
  env?: Record<string, string>;
  recipeArgs?: string[];
  /** "standard" or "privileged". */
  dockerAccess?: string;
  /** Empty means the load's own id. */
  group?: string;
}
/** One rank of a load that failed to start, read before it was rolled back. */
export interface FailureRank {
  node: string;
  rank: number;
  role: string;
  exit: number | null;
  tail: string;
}
/** Why a start failed, and what it left: the failure's logs are saved on this Mac first. */
export interface FailureRecord {
  atMs: number;
  cause: string;
  ranks: FailureRank[];
  logDir: string;
  /** Nodes that still hold its containers (look-only held the rollback, or the node was out of reach). */
  leftOn: string[];
}
/** What the head's log says about its traffic. */
export interface Transport {
  ib: string[];
  socket: boolean;
  mqIp: string | null;
}
/** A node's memory and /dev/shm right before a load. */
export interface LoadBaseline {
  node: string;
  memAvailable: number;
  shm: string[];
  atMs: number;
}
/** What the last readiness check found (models/readiness.rs). */
export interface LoadReadiness {
  health: boolean;
  modelsListed: boolean;
  token: boolean | null;
  sanity: { ok: boolean; answer: string; atMs: number } | null;
  ok: boolean;
  why: string | null;
  atMs: number;
  summary?: string;
}
/** The native APIs the engine serves (/openapi.json). */
export interface LoadProtocols {
  messages: boolean;
  countTokens: boolean;
  responses: boolean;
  imageId: string;
  checkedMs: number;
}
/** What an unload found once the containers were gone. */
export interface UnloadReport {
  atMs: number;
  ok: boolean;
  summary: string;
  nodes: { node: string; stopped: boolean; memAvailable: number | null; memBack: boolean; portClosed: boolean; newShm: string[] }[];
}
export interface Deployment {
  recovery?: "checking" | "waiting" | "cleanup" | "ownerAction" | "maintenance" | null;
  baseline?: LoadBaseline[];
  readiness?: LoadReadiness | null;
  protocols?: LoadProtocols | null;
  unloadReport?: UnloadReport | null;
  failure?: FailureRecord | null;
  transport?: Transport | null;
  /** "It isn't using the RDMA fabric: …", when the head's log says so. */
  transportWarning?: string | null;
}
export interface DiskCheck {
  node: string; dest: string; probe: string; transferBytes: number; availableBytes: number;
  capacityBytes: number; requiredBytes: number; usedPercent: number; projectedUsedPercent: number;
  ok: boolean; warning: string | null;
}
export type DiskRequest =
  | { kind: "download"; node: string; selection: DownloadSelection }
  | { kind: "resume"; node: string; id: string; expected: DownloadResume }
  | { kind: "copy"; key: string; targets: string[] };

/** Ephemeral, read-only comparison; never permission to start or a space reservation. */
export interface DownloadCapacity {
  readings: { node: string; check: DiskCheck | null; error: string | null }[];
  suggested: string | null;
}

export interface ChainMember { node: string; id: string; dest: string }
export interface DownloadChain { id: string; source: string; repo: string; revision: string; expiresAt: number; members: ChainMember[] }
export interface ChainReview {
  selection: DownloadSelection; source: string; targets: string[];
  nodes: { node: string; dest: string; address: string }[]; checks: DiskCheck[]; port: number;
}
export interface ChainCancel { node: string; id: string; error: string | null }
