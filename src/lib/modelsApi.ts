// Types and calls for Models, endpoints, Chat, Providers and the Server that
// grew in round 1 (mirrors src-tauri/src/models, llm and server). The older
// shapes in types.ts are extended here rather than edited there.
import { invoke } from "@tauri-apps/api/core";
import { streamChat } from "./chatStream";
import type {
  AnalysisResult as BaseAnalysisResult, ArchFacts as BaseArch, ChatMessage as BaseMessage, ChatStats as BaseStats,
  Conversation as BaseConversation, Deployment as BaseDeployment, DeploySpec as BaseSpec, FitRow as BaseFitRow,
  Benchmark, FabricIf, HfModel as BaseHfModel, LibraryEntry as BaseLibraryEntry, ModelDetails as BaseDetails, PreviewStep,
  ProviderView as BaseProviderView, ServerLogEntry as BaseLogEntry, ServerSettings as BaseServerSettings, ServerView as BaseServerView,
} from "./types";
import type { Endpoint } from "./endpoints";

export type { Endpoint };

// ---- Loads

export type Listen = "lan" | "fabric" | "all";

export type LoadEngine = "vllm" | "sglang";

export interface DeploySpec extends BaseSpec {
  replicas?: number;
  importedRecipe?: ImportedRecipe | null;
  /** vLLM (the default) or SGLang. */
  engine?: LoadEngine;
  /** --trust-remote-code, from its own checkbox only. */
  allowRemoteCode?: boolean;
  listen?: Listen;
  /** Resolved by SparkBox; leave empty. */
  host?: string;
  /** A per-load API key (VLLM_API_KEY from the Keychain). On unless turned off. */
  requireKey?: boolean;
  maxNumSeqs?: number | null;
  kvCacheDtype?: "auto" | "bfloat16" | "fp8" | "fp8_e4m3" | "fp8_e5m2" | null;
  /** null leaves vLLM's default (on). */
  prefixCaching?: boolean | null;
  toolCallParser?: string | null;
  reasoningParser?: string | null;
  keepRunning?: boolean;
  overrideMemory?: boolean;
}

export interface ImportedRecipe {
  id: string; title: string; match: string[]; image: string; backend: "native"; topologies: string[];
  env: Record<string, string>; args: { flag: string; value: string | null; json: unknown | null }[];
  mounts: string[]; defaults: { share: number | null; context: number | null; seqs: number | null };
  toolParser: string | null; reasoningParser: string | null; multiNodeDocker: "standard" | "privileged"; note: string | null;
}
export interface RecipeImport { recipe: ImportedRecipe; modelPath: string; servedName: string; tensorParallel: number; pipelineParallel: number; kvCacheDtype?: string; prefixCaching?: boolean; expertParallel?: boolean; warnings: string[] }

export interface Deployment extends Omit<BaseDeployment, "spec"> {
  spec: DeploySpec;
}

/** An inert history projection. Deliberately has no DeploySpec or endpoint actions. */
export interface HistoricalLoad {
  id: string; model: string | null; nodes: string[]; savedState: string;
  startedAt: number | null; generation: number | null; message: string | null; failure: string | null;
  unloadReport: import("./types").UnloadReport | null; reportOnly: boolean;
}
export interface DeploymentHistorySource {
  name: string; kind: "recovery" | "file" | "demo"; sha256: string; bytes: number;
  records: HistoricalLoad[]; issues: string[];
}
export interface DeploymentHistoryView {
  demo: boolean; folder: string | null; sources: DeploymentHistorySource[]; issues: string[];
}

/** A node's memory from /proc/meminfo, in bytes. */
export interface PreflightMem {
  total: number;
  free: number;
  available: number;
  cached: number;
}

/** A labelled container with no active load on a node, or a Ray worker (`rayLeftovers`). */
export interface Leftover {
  id: string;
  name: string;
  /** Its sparkbox.deployment label. */
  deployment: string;
  /** Its sparkbox.instance label, empty when it has none. */
  instance: string;
  /** "Up 3 hours", "Exited (1) 2 days ago". */
  status: string;
  /** This build started it, so SparkBox may remove it. Another build's is shown, never removed. */
  ours: boolean;
}

/** "Before it starts": what one chosen node has before a load (the same read deploy makes). */
export interface Preflight {
  node: string;
  /** Demo data: nothing was read. */
  demo: boolean;
  mem: PreflightMem | null;
  /** share × MemTotal + 4 GiB: what vLLM wants free at startup. */
  needFree: number;
  /** MemFree is below needFree: vLLM would refuse to start unless the page cache is dropped. */
  dropNeeded: boolean;
  earlyoom: { active: boolean; state: string; config: string | null } | null;
  imageRef: string;
  imageId: string | null;
  modelPresent: boolean;
  portFree: boolean;
  portOwner: string | null;
  shm: string[];
  caches: { path: string; exists: boolean; owner: string | null }[];
  /** What stops the load, in words. */
  problems: string[];
  /** Edit & reload, read while the load it replaces still serves: its name. Its share (`freed`) counts as free, and its own port isn't a problem. */
  replacing: string | null;
  /** The replaced load's share of MemTotal, in bytes. */
  freed: number;
  /** Docker's image store: "containerd", or the storage driver ("overlay2"). */
  store: string | null;
  leftovers: Leftover[];
  /** Download containers here: running jobs, not leftovers. A load can start beside one. */
  downloads: Leftover[];
  /** Containers still running `ray start --address`, which SparkBox didn't start. */
  rayLeftovers: Leftover[];
  /** The models this node's Ollama holds in memory. */
  ollamaResident: string[];
  /** The node's ConnectX-7 halves, from its quick check. */
  fabric: FabricIf[];
  /** The read didn't finish, so the other fields say nothing about the node. */
  cutOff: boolean;
}

/** model_preflight: the chosen nodes as a set, with one verdict. */
export interface PreflightSet {
  versions?: import("./types").VersionReport | null;
  nodes: Preflight[];
  /** "demo": nothing was read. */
  verdict: "ready" | "blocked" | "demo";
  problems: string[];
  /** The one image id every node agrees on. */
  imageId: string | null;
  checkedAtMs: number;
}

export type PlanLevel = "green" | "amber" | "red";

export interface NodePlan {
  node: string;
  weights: number;
  kv: number;
  overhead: number;
  need: number;
  budget: number;
  free: number;
  available: number;
  total: number;
  holders: { name: string; bytes: number }[];
  level: PlanLevel;
  /** "Won't start", "Won't fit", "Won't start safely", "Tight" or "Fits". */
  verdict: string;
  /** What the node already uses (other processes and the OS: its used memory). */
  inUse: number;
  /** vLLM's own process outside its share (Python, the CUDA libraries), on top of inUse. */
  hostRuntime?: number;
  /** The node's memory once vLLM has reserved its share, in % of all of it. */
  afterPct: number;
  maxShare: number;
  /** The largest share (0.05 steps) that keeps the node under the 90% alert line once vLLM reserves it. */
  safeShare: number;
  /** The model fits in safeShare. */
  fitsSafe: boolean;
  /** When it doesn't: the longest context that would fit in safeShare. */
  safeContext: number | null;
  /** Leads with the blocker: "Won't start: vLLM reserves 95.8 GiB (0.80 share) at startup; 74.2 GiB is free." */
  reason: string | null;
}

export interface ArchFacts extends BaseArch {
  /** DeepSeek-style sparse attention over an MLA cache: on a GB10 only an FP8 KV cache has a kernel. */
  sparseMla?: boolean;
  linearLayers: number;
  stateBytesPerSeq: number;
  visionBytes: number;
}

/** A way to split a model across the chosen nodes, and whether this model can take it. */
export interface TopologyOffer {
  id: string;
  tp: number;
  pp: number;
  replicas?: number;
  nodes: number;
  ok: boolean;
  /** Refused: why. Offered: something worth reading, when there is. */
  reason: string | null;
  advanced: boolean;
  /** Refused only because this recipe doesn't list it: the model can take it. */
  unlisted: boolean;
}

/** One load that passed readiness with a recipe. */
export interface RecipeVerified {
  imageId: string;
  topology: string;
  nodeCount: number;
  atMs: number;
}

/** A load recipe as the page shows it. */
export interface RecipeView {
  id: string;
  title: string;
  image: string;
  topologies: string[];
  defaults: { share: number | null; context: number | null; seqs: number | null };
  toolParser: string | null;
  reasoningParser: string | null;
  note: string | null;
  /** Holds a place for a model that can't load yet: its note says why. */
  placeholder: boolean;
  /** Every recipe that fits this folder, this one first: a menu when there are two. Empty in recipes_list. */
  fits: { id: string; title: string }[];
  verifiedOn: RecipeVerified[];
}

/** "Verified on: nothing yet", or each record's image, split and date. */
export function verifiedWords(r: Pick<RecipeView, "verifiedOn">): string {
  if (!r.verifiedOn.length) return "Verified on: nothing yet";
  const one = (v: RecipeVerified) => `${v.topology} on ${v.nodeCount} node${v.nodeCount === 1 ? "" : "s"}, ${v.imageId.replace(/^sha256:/, "").slice(0, 8)}…, ${new Date(v.atMs).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" })}`;
  return `Verified on: ${r.verifiedOn.map(one).join("; ")}`;
}

export interface DeployPlan {
  inventoryWarnings?: string[];
  nodes: NodePlan[];
  level: PlanLevel;
  needs: { weightBytes: number; arch: ArchFacts; kvBytesPerToken: number | null };
  portTakenBy: string | null;
  suggestedPort: number;
  host: string;
  url: string;
  keyPreview: string | null;
  maxContext: number | null;
  /** The one share to offer: it fits, and every node stays under the alert line. */
  suggestedShare: number | null;
  /** When the context asked for doesn't fit under the line: [share, longest context that does]. */
  suggestedContext: [number, number] | null;
  /** The share sized to this model on these nodes, which the page follows until someone chooses one. Null: stay at the first-load share. */
  sizedShare: number | null;
  /** How many full-length requests the KV cache holds at this share. Null: a request's size isn't known. */
  fullRequests: number | null;
  /** The recipe picked for this folder. */
  recipe: RecipeView | null;
  /** The splits for this many nodes, in the order to offer them. */
  topologies: TopologyOffer[];
}

// ---- Fit table

export interface FitRow extends BaseFitRow {
  share?: number;
  safeShare?: number;
  hostRuntime?: number;
  maxContextTokens: number | null;
  fullRequests: number | null;
  pipelineOnly: boolean;
  kvDtype: "bf16" | "fp8";
  fitsNow: { ok: boolean; freeNodes: string[]; text: string } | null;
  /** Why it can't run whatever the memory: "No kernel on GB10 (sparse MLA needs an FP8 KV cache)". */
  noKernel?: string | null;
}

/** Files the download leaves out (original/ checkpoints, consolidated weights, GGUF…). */
export interface SkippedFiles {
  what: string;
  files: number;
  bytes: number;
}

export interface HfModel extends BaseHfModel {
  /** Everything downloaded: the served weights plus config and tokenizer files. */
  downloadBytes: number;
  /** The whole repo, every copy and format included. */
  repoBytes: number;
  skipped: SkippedFiles[];
  /** MLX weights (Apple silicon only): no download, load or fit on a GB10. */
  mlx?: boolean;
}

/** A search row once its files are read: the served weights, the detail's own number. */
export interface HfSizing {
  id: string;
  weightBytes: number;
  /** "safetensors", "bin" or "none" (only GGUF or other formats). */
  weightsFormat: string;
  minNodes: number | null;
  /** minNodes holds only with an FP8 KV cache (no bf16 kernel on GB10, e.g. sparse MLA). */
  kvFp8?: boolean;
  mlx: boolean;
  gguf: boolean;
}

/** A search result as the Hub lists it; its size is an estimate until its files are read (HfSizing). */
export interface HfHit {
  id: string;
  downloads: number;
  likes: number;
  pipeline: string | null;
  tags: string[];
  lastModified: string | null;
  params?: number | null;
  /** From the Hub's parameter counts: only a fallback when the files can't be read. */
  weightBytes?: number | null;
  minNodes?: number | null;
  library?: string | null;
  mlx?: boolean;
}

/** What the detail says instead of a fit table, a download or a load for MLX weights. */
export const MLX_NOTE = "MLX weights run on Apple silicon, not on a GB10. Look for the same model as safetensors, GGUF or NVFP4.";

export interface ModelDetails extends Omit<BaseDetails, "fit" | "arch" | "model"> {
  model: HfModel;
  arch: ArchFacts;
  fit: FitRow[];
  fitFp8: FitRow[];
}

export interface AnalysisResult extends BaseAnalysisResult {
  /** Allowlisted extra options, one array per option and its value. */
  allowedArgs: string[][];
  /** Suggested but never applied (e.g. --trust-remote-code). */
  notApplied: string[];
}

/** What a Library folder's config.json says (library_detail). */
export interface LibraryDetail {
  architectures: string[];
  modelType: string | null;
  /** "nvfp4", "fp8", "awq"… */
  quantization: string | null;
  weightBytes: number;
  kvBytesPerToken: number | null;
  arch: ArchFacts;
}

export interface LibraryEntry extends BaseLibraryEntry {
  /** Endpoint id serving this folder right now. */
  inUse: string | null;
}

/** `node`'s copy, when it's finished: one left partway, or still being written, doesn't count. */
export function finishedCopy(e: Pick<LibraryEntry, "copies">, node: string): LibraryEntry["copies"][string] | undefined {
  const c = e.copies[node];
  return c && !c.partial ? c : undefined;
}

/** The nodes that hold a finished copy. */
export function finishedNodes(e: Pick<LibraryEntry, "copies">): string[] {
  return Object.keys(e.copies).filter((n) => finishedCopy(e, n));
}

/**
 * Rust's sentences say a node's id; the app says its name. Whole ids only, in one pass, so two
 * renames that swap each other's ids stay apart, and cell1 is left alone inside cell10.
 */
export function inNames(text: string, nodes: readonly { id: string; name: string }[]): string {
  const renamed = new Map(nodes.filter((n) => n.id !== n.name).map((n) => [n.id, n.name]));
  if (!renamed.size) return text;
  const ids = [...renamed.keys()].map((id) => id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  return text.replace(new RegExp(`(?<![\\w-])(?:${ids})(?![\\w-])`, "g"), (id) => renamed.get(id) ?? id);
}

// ---- Providers

export interface ProviderView extends BaseProviderView {
  chatOnly: boolean;
  hidden: string[];
  /** Offered models, filtered and in the curated order. */
  visible: string[];
  effectiveDefault: string | null;
  /** Found on the cluster this session, not saved. */
  discovered: boolean;
  /** Plain http is all right for its private address on the home network. */
  allowHttpLan?: boolean;
}

// ---- Chat

export interface ChatStats extends BaseStats {
  deltas?: number | null;
  decodeMs?: number | null;
}

export interface ChatMessage extends Omit<BaseMessage, "stats"> {
  stats?: ChatStats | null;
  id?: string | null;
  parentId?: string | null;
}

export interface Conversation extends Omit<BaseConversation, "messages"> {
  messages: ChatMessage[];
  /** The message the shown branch ends at. */
  leaf?: string | null;
}

// ---- Server

export interface ServerSettings extends BaseServerSettings {
  allowedOrigins: string[];
}

export interface ServerView extends Omit<BaseServerView, "settings"> {
  settings: ServerSettings;
}

export interface ServerLogEntry extends BaseLogEntry {
  id: number;
  inputTokens: number | null;
  outputTokens: number | null;
  durationMs: number | null;
  tokensPerSec: number | null;
  done: boolean;
}

// ---- Benchmarks

export interface BenchSpec {
  profile: "chat" | "rag" | "long" | "custom";
  inputTokens: number;
  outputTokens: number;
  concurrency: number[];
  requests?: number;
}

export interface Pct {
  p50: number;
  p90: number;
  p99: number;
}

export interface BenchLevel {
  concurrency: number;
  requests: number;
  completed: number;
  failed: number;
  durationS: number;
  reqPerS: number;
  outputTps: number;
  inputTps: number;
  ttftMs: Pct;
  tpotMs: Pct;
  itlMs: Pct;
  e2eMs: Pct;
}

export interface BenchRun extends Benchmark {
  spec: BenchSpec | null;
  levels: BenchLevel[];
  endpoint: string | null;
  external: boolean;
}

// ---- Ollama

export interface OllamaLoadedInfo {
  name: string;
  sizeBytes: number;
  vramBytes: number;
  expiresAt: number | null;
  contextLength?: number | null;
}

export interface OllamaModel {
  name: string;
  sizeBytes: number;
  modifiedAt: number | null;
  family: string | null;
  parameterSize: string | null;
  quantization: string | null;
  loaded: OllamaLoadedInfo | null;
}

export interface OllamaView {
  endpoint: string;
  node: string;
  port: number;
  version: string | null;
  models: OllamaModel[];
}

export const modelsApi = {
  /** Restarts a SparkBox load in order as a task that verifies it answers; resolves to the task's id. */
  deploymentRestart: (id: string, dropCachesOnce = false) => invoke<string>("deployment_restart", { id, dropCachesOnce }),
  /** Restarts a server SparkBox didn't start, in order, as a task that verifies it answers; resolves to the task's id. */
  endpointRestart: (id: string) => invoke<string>("endpoint_restart", { id }),
  /** Stops listing a down or stopped endpoint SparkBox didn't start (it returns if it starts again). */
  endpointForget: (id: string) => invoke<void>("endpoint_forget", { id }),
  /** Unloads as a task (stop, then check memory, the port and /dev/shm); resolves to the task's id ("" in Demo). */
  deploymentUnload: (id: string) => invoke<string>("deployment_unload", { id }),
  deploymentResume: (id: string, dropCachesOnce = false) => invoke<void>("deployment_resume", { id, dropCachesOnce }),
  imageJobs: (image: string, nodes: string[], estimatedBytes: number, source?: string) => invoke("model_image_jobs", { image, nodes, estimatedBytes, source: source || null }),
  importRecipe: (node: string, folder: string) => invoke<RecipeImport>("model_recipe_import", { node, folder }),
  /** Forgets a failed or unloaded load; one whose containers are still there is unloaded first: resolves to that task's id. */
  deploymentForget: (id: string) => invoke<string | null>("deployment_forget", { id }),
  modelPreflight: (spec: DeploySpec) => invoke<PreflightSet>("model_preflight", { spec }),
  /** The load recipes and what each has been verified on. */
  recipesList: () => invoke<RecipeView[]>("recipes_list"),
  /** Removes what earlier loads left on these nodes, as a task; resolves to its id. */
  deploymentCleanLeftovers: (nodes: string[], ids: string[]) => invoke<string>("deployment_clean_leftovers", { nodes, ids }),
  benchRun: (endpoint: string, model: string, spec: BenchSpec) => invoke<string>("bench_run", { endpoint, model, spec }),
  benchExport: (at: number, format: "csv" | "json") => invoke<string>("bench_export", { at, format }),
  benchmarksList: () => invoke<BenchRun[]>("benchmarks_list"),
  ollamaList: (endpoint: string) => invoke<OllamaView>("ollama_list", { endpoint }),
  /** Resolves to the pull's task id. */
  ollamaPull: (endpoint: string, name: string) => invoke<string>("ollama_pull", { endpoint, name }),
  ollamaDelete: (endpoint: string, name: string) => invoke<void>("ollama_delete", { endpoint, name }),
  endpointsList: () => invoke<Endpoint[]>("endpoints_list"),
  endpointsRefresh: () => invoke<Endpoint[]>("endpoints_refresh"),
  endpointStop: (id: string) => invoke<void>("endpoint_stop", { id }),
  deployPlan: (spec: DeploySpec) => invoke<DeployPlan>("deploy_plan", { spec }),
  deployPreview: (spec: DeploySpec) => invoke<PreviewStep[]>("deploy_preview", { spec }),
  deploy: (spec: DeploySpec) => invoke<Deployment>("deploy", { spec }),
  deploymentHistory: () => invoke<DeploymentHistoryView>("deployment_history_list"),
  deploymentHistoryFile: (name: string, bytes: number[]) => invoke<DeploymentHistorySource>("deployment_history_file", { name, bytes }),
  deploymentsList: () => invoke<Deployment[]>("deployments_list"),
  hfModel: (repo: string, revision?: string) => invoke<ModelDetails>("hf_model", { repo, revision }),
  hfRefs: (repo: string) => invoke<import("./types").HfRef[]>("hf_refs", { repo }),
  /** A search row's size from its files (the detail's own reading), kept a day. */
  hfSize: (repo: string) => invoke<HfSizing>("hf_size", { repo }),
  modelsAnalyze: (repo: string, revision?: string) => invoke<AnalysisResult>("models_analyze", { repo, revision }),
  libraryList: () => invoke<LibraryEntry[]>("library_list"),
  /** A folder's config summary for the Library's detail pane (reads config.json once). */
  libraryDetail: (key: string) => invoke<LibraryDetail>("library_detail", { key }),
  providersList: () => invoke<ProviderView[]>("providers_list"),
  conversationGet: (id: string) => invoke<Conversation>("conversation_get", { id }),
  conversationSave: (conversation: Conversation) => invoke<void>("conversation_save", { conversation }),
  serverGet: () => invoke<ServerView>("server_get"),
  serverStart: () => invoke<ServerView>("server_start"),
  serverStop: () => invoke<ServerView>("server_stop"),
  serverRevokeKey: (name: string) => invoke<ServerView>("server_revoke_key", { name }),
  serverSave: (settings: ServerSettings) => invoke<ServerView>("server_save", { settings }),
  serverLogs: () => invoke<ServerLogEntry[]>("server_logs"),
};

/** Streams one Chat reply (same as api.chatSend, with the round-1 message shape). */
export function chatSend(
  request: { requestId: string; provider: string; model: string; system: string; params: { effort?: string | null; temperature?: number | null; maxTokens?: number | null }; messages: ChatMessage[] },
  onEvent: (e: ChatEventV2) => void,
) {
  return streamChat(request, onEvent, invoke);
}

export type ChatEventV2 =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "notice"; text: string }
  | { type: "done"; message: ChatMessage }
  | { type: "error"; message: string };
