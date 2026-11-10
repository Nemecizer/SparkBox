// Every inference server on the cluster, whoever started it: SparkBox's own
// loads, engines found in the nodes' containers and processes, and endpoints
// added by hand. Rust merges and probes them (src-tauri/src/models/endpoints.rs)
// and decides each one's health from evidence: Ready only when every node it
// needs is online, its containers run, it answered in the last minute and it
// generated in the last two (a one-token test, or live traffic). The list
// arrives on `endpoints://update`; until the first one does, it's worked out
// from the snapshot, and says so ("Checking") instead of claiming Ready. When
// one that was down answers again, the failure toast about it is replaced.
import { useMemo, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./api";
import { useCluster } from "./store";
import { getToasts, toast } from "./toasts";
import type { ClusterSnapshot } from "./types";
import { ENDPOINT_STATE } from "./vocabulary";

export type Engine = "vllm" | "ollama" | "sglang" | "llamacpp" | "trtllm" | "nim" | "other";
/**
 * Ready, Loading, Down and Stopped, plus three quiet ones (r4): `idle` answers with nothing in
 * memory (an Ollama at rest), `gone` hasn't been found for a while (no evidence it broke), and
 * `paused` waits out a restart SparkBox planned on one of its nodes.
 */
export type EndpointHealth = "ready" | "loading" | "down" | "stopped" | "idle" | "gone" | "paused" | "unknown";

export interface EndpointHistory {
  /** Unix seconds, one point per scrape (every 3 s), 15 minutes. */
  ts: number[];
  genTps: number[];
  running: number[];
  kvPct: number[];
  promptTps: number[];
  waiting: number[];
  ttftP50Ms: (number | null)[];
}

export interface EndpointMetrics {
  /** Generated tokens per second, all requests together. */
  genTps: number;
  promptTps: number;
  running: number;
  waiting: number;
  /** KV cache in use, 0–100 (only when kvKnown). */
  kvPct: number;
  /** The engine publishes a KV-cache gauge (Ollama and llama.cpp don't). */
  kvKnown?: boolean;
  ttftP50Ms: number | null;
  ttftP95Ms: number | null;
  /** Set when the percentile fell in a wide histogram bucket: that bucket, [low, high] in ms. */
  ttftP50Range?: [number, number] | null;
  ttftP95Range?: [number, number] | null;
  itlP50Ms: number | null;
  itlP50Range?: [number, number] | null;
  e2eP50Ms: number | null;
  e2eP95Ms: number | null;
  reqPerS: number;
  prefixHitPct: number | null;
  /** The engine's preemption counter, when it publishes one. */
  preemptionsTotal?: number | null;
  /** Optional cumulative vLLM speculative-decoding counters; absence is not zero. */
  speculative?: { draftsTotal: number | null; draftTokensTotal: number | null; acceptedTokensTotal: number | null } | null;
  /** Unix ms of the last scrape. */
  updatedMs: number;
  history: EndpointHistory;
}

/** A model an Ollama server holds in memory right now (/api/ps). */
export interface OllamaLoaded {
  name: string;
  sizeBytes: number;
  vramBytes: number;
  /** Unix seconds when Ollama unloads it, if it said. */
  expiresAt: number | null;
  contextLength?: number | null;
}

export interface Endpoint {
  /** `{node}-{engine}-{port}`; also the provider id Chat and Server use. */
  id: string;
  engine: Engine;
  models: string[];
  nodes: string[];
  headNode: string;
  port: number;
  /** OpenAI-style base URL, ending in /v1. */
  url: string;
  managedBy: "sparkbox" | "external";
  health: EndpointHealth;
  /** Why it isn't Ready, in a sentence: "Pipeline stage 3 of 3 on cell3 is offline". */
  healthReason?: string | null;
  /** Unix ms of the last probe it answered. */
  lastOkMs?: number | null;
  /** The nodes it needs (tensor × pipeline parallel of them), head first. */
  expectedNodes?: string[];
  /** Expected nodes that are offline, silent or not running their part. */
  missingNodes?: string[];
  /** Container restarts behind it (Docker's count). */
  restarts?: number | null;
  /** SparkBox didn't start it and it isn't running (its head exited, or it vanished): Forget drops it. */
  forgettable?: boolean;
  /** Keep running for a multi-node SparkBox load. */
  supervisor?: Supervisor | null;
  parallel?: { tp: number; pp: number } | null;
  containerIds: string[];
  deploymentId?: string | null;
  metrics?: EndpointMetrics | null;
  /** "server": the engine's own /metrics; "sparkbox": only requests SparkBox sent or relayed. */
  metricsSource?: "server" | "sparkbox" | null;
  /** The folder or repo it serves (the Library marks it "in use"). */
  modelPath?: string | null;
  /** Provider id registered for Chat and Server, when there is one. */
  providerId?: string | null;
  /** Longest request the server accepts, in tokens. */
  maxContext?: number | null;
  /** Weight loading progress while health is "loading". */
  loadingPct?: number | null;
  /** What's happening, in a sentence (loading stage, last probe error). */
  message?: string | null;
  /** Head container names, for Logs and Stop. */
  containerNames?: string[];
  ollama?: OllamaLoaded[];
  /** "lan" | "fabric" | "all" | "local" | "unknown". */
  listen?: string;
  source?: "deployment" | "discovered" | "manual";
  /** Unix ms of the last /v1/models probe. */
  probedMs?: number | null;
  /** The last one-token test: Ready means a token came back recently. */
  canary?: Canary | null;
  /** Unix ms when live traffic last proved it generates (no test token needed then). */
  passiveOkMs?: number | null;
  /** Requests running but no token out for this long (s). */
  stalledS?: number | null;
  /** A note while Ready: "Slow: 1 token took 12.4 s". */
  healthNote?: string | null;
  // ── r4-serving ──
  /** What the UI calls it: the served name ("glm-5.3-flash"), or "Ollama on cell3". Use endpointLabel(). */
  label?: string;
  /** Who can reach it: "node-only" (loopback), "lan", "fabric" or "unknown". */
  reach?: "node-only" | "lan" | "fabric" | "unknown";
  /** The models an Ollama keeps on disk (/api/tags), with their sizes in the same order. */
  onDisk?: string[];
  onDiskBytes?: number[];
  /** Unix ms it was last found, while Gone. */
  goneSinceMs?: number | null;
  // ── p1r4 readiness ──
  /** The image it runs: a SparkBox load's sparkbox.image label, else the container's image (an id when started by id). */
  image?: string | null;
  /** A SparkBox load's last readiness check (P1.35): Ready only once it answers. */
  readiness?: Readiness | null;
  /** The native APIs its /openapi.json lists (P1.37), checked once the load was Ready. */
  protocols?: { messages: boolean; countTokens: boolean; responses: boolean; checkedMs: number } | null;
}

/** /health, the model list, a test token and (the first time a recipe meets an image) 17+25. */
export interface Readiness {
  health: boolean;
  modelsListed: boolean;
  token?: boolean | null;
  sanity?: { ok: boolean; answer: string; atMs: number } | null;
  ok: boolean;
  why?: string | null;
  atMs: number;
  tokenMs?: number | null;
  /** "Ready: /health 200, gemma-3-27b-it listed, a test token in 0.4 s, 17+25 → 42 (…)". */
  summary?: string;
}

/** One test request for a single token (max_tokens 1, or Ollama's num_predict 1). */
export interface Canary {
  atMs: number;
  ok: boolean;
  /** Request to reply, ms. */
  latencyMs?: number | null;
  /** Why no token came back: "HTTP 500", "no reply in 30 s". */
  error?: string | null;
}

let endpoints: Endpoint[] | null = null;
let started = false;
const listeners = new Set<() => void>();

function set(list: Endpoint[]) {
  const before = endpoints;
  endpoints = list;
  if (before) noteRecoveries(before, list);
  listeners.forEach((l) => l());
}

const ENGINE_NAME: Record<Engine, string> = { vllm: "vLLM", ollama: "Ollama", sglang: "SGLang", llamacpp: "llama.cpp", trtllm: "TensorRT-LLM", nim: "NIM", other: "Server" };

/**
 * What every surface calls an endpoint, never its id: the name clients use ("glm-5.3-flash"),
 * or for Ollama, whose models come and go, "Ollama on cell3". A model path reads as its last part.
 */
export function endpointLabel(e: Pick<Endpoint, "engine" | "models" | "headNode" | "containerNames"> & { label?: string }): string {
  if (e.label) return e.label;
  const at = (what: string) => (e.headNode ? `${what} on ${e.headNode}` : what);
  if (e.engine === "ollama") return at("Ollama");
  const served = e.models[0]?.startsWith("/") ? e.models[0].replace(/\/+$/, "").split("/").pop() : e.models[0];
  return served || e.containerNames?.[0] || at(ENGINE_NAME[e.engine] ?? "Server");
}

/** What toasts and commands call an endpoint (endpointLabel; kept for older callers). */
export function endpointName(e: Pick<Endpoint, "engine" | "models" | "headNode" | "id" | "containerNames"> & { label?: string }): string {
  return endpointLabel(e);
}

/** It's serving a model right now, or getting there: false at rest (idle), when it vanished (gone) and while a planned restart pauses it. */
export function isServing(e: Pick<Endpoint, "health">): boolean {
  return e.health !== "idle" && e.health !== "gone" && e.health !== "paused" && e.health !== "stopped";
}

/** "Reachable from cell3 only (127.0.0.1:11434). Chat and the API server on this Mac can't use it." */
export function reachLine(e: Pick<Endpoint, "reach" | "listen" | "headNode" | "port" | "url">): string | null {
  const reach = e.reach ?? (e.listen === "local" ? "node-only" : e.listen === "fabric" ? "fabric" : null);
  if (reach === "node-only") return `Reachable from ${e.headNode} only (127.0.0.1:${e.port}). Chat and the API server on this Mac can't use it.`;
  if (reach === "fabric") return "Reachable from the other nodes over the fabric only. Chat and the API server on this Mac can't use it.";
  return null;
}

/** The toast key for what's said about an endpoint ("is down", "is answering again"). */
export const endpointKey = (id: string) => `endpoint:${id}`;

/**
 * An endpoint that was down answers again: the failure toast still about it (a restart that
 * "didn't come back", a Verify that timed out) becomes "llama-3.3-70b is answering again".
 */
export function noteRecoveries(before: Endpoint[], after: Endpoint[]) {
  for (const e of after) {
    const was = before.find((x) => x.id === e.id);
    if (!was || was.health === "ready" || e.health !== "ready") continue;
    const name = endpointLabel(e);
    const key = endpointKey(e.id);
    // Only the toast keyed to this endpoint: a toast that merely mentions the model is about something else.
    const stale = getToasts().find((t) => t.key === key);
    if (!stale) continue;
    const proof = proofLine(e, Date.now());
    toast({ id: stale.id, key, kind: "success", title: `${name} is answering again`, body: proof?.text });
  }
}

/** What a Ready card's note means: busy SSH channels and held test tokens aren't proof it generates. */
function noteTitle(note: string): string {
  if (/^Not checked since /.test(note)) return "Ready when last checked: no SSH channel on its node was free to check it since";
  if (/^Not tested/.test(note)) return "Ready: it answered within the last minute; the note says why no recent test token shows it generates";
  return "Ready, with a note: it answered and generated within the last two minutes";
}

/** How the card says it generates: "Answered 12 s ago · 1 token in 84 ms" or "Generating · live traffic". */
export function proofLine(e: Pick<Endpoint, "health" | "canary" | "passiveOkMs" | "healthNote" | "stalledS" | "metrics" | "lastOkMs">, nowMs: number): { text: string; title: string; note: boolean } | null {
  if (e.health === "unknown") return {
    text: e.lastOkMs != null ? `Last successful check ${new Date(e.lastOkMs).toLocaleString()}` : "No current cluster report",
    title: "Last-known information; reconnect and wait for fresh checks to confirm its state", note: true,
  };
  if (e.health !== "ready") return null;
  if (e.healthNote) return { text: e.healthNote, title: noteTitle(e.healthNote), note: true };
  if (e.stalledS != null) {
    const running = Math.round(e.metrics?.running ?? 0);
    const waiting = Math.round(e.metrics?.waiting ?? 0);
    const [n, word] = running > 0 || waiting === 0 ? [running, "running"] : [waiting, "waiting"];
    return { text: `No tokens for ${e.stalledS} s · ${n} request${n === 1 ? "" : "s"} ${word}`, title: "It reads Stalled (Down) after 60 s of this", note: true };
  }
  const ago = (ms: number) => {
    const s = Math.max(0, Math.round((nowMs - ms) / 1000));
    return s < 5 ? "just now" : s < 60 ? `${s} s ago` : `${Math.round(s / 60)} min ago`;
  };
  // Live traffic is the proof while tokens keep coming; the test token takes over a minute after they stop.
  if (e.passiveOkMs != null && nowMs - e.passiveOkMs <= 60_000) {
    // "Generating" while requests are still running; a counter that rose a moment ago with none left "generated".
    const now = nowMs - e.passiveOkMs <= 10_000 && (e.metrics?.running ?? 1) > 0;
    return { text: now ? "Generating · live traffic" : `Generated ${ago(e.passiveOkMs)} · live traffic`, title: "Its token counter rose while /health was fine: no test token needed", note: false };
  }
  const c = e.canary;
  if (c?.ok) {
    const took = c.latencyMs != null ? ` · 1 token in ${fmtLatency(c.latencyMs)}` : "";
    return { text: `Answered ${ago(c.atMs)}${took}`, title: "SparkBox asks for one token every minute; Ready means one came back in the last two", note: false };
  }
  return null;
}

/** What a SparkBox load proved when it became Ready (its readiness check), or null. Only while it's
 *  Ready: a load that's down, or restarting, hasn't proved it again. */
export function readinessLine(e: Pick<Endpoint, "managedBy" | "health" | "readiness">): string | null {
  if (e.managedBy !== "sparkbox" || e.health !== "ready" || !e.readiness?.ok) return null;
  return e.readiness.summary || null;
}

/** "Native API (from /openapi.json): Messages ✓ · count_tokens ✓ · Responses ✗", once probed. */
export function protocolsLine(e: Pick<Endpoint, "protocols">): string | null {
  const p = e.protocols;
  if (!p) return null;
  const mark = (yes: boolean) => (yes ? "✓" : "✗");
  return `Native API (from /openapi.json): Messages ${mark(p.messages)} · count_tokens ${mark(p.countTokens)} · Responses ${mark(p.responses)}`;
}

async function start() {
  if (started || !inTauri) return;
  started = true;
  await listen<Endpoint[]>("endpoints://update", (e) => set(e.payload));
  try {
    set(await invoke<Endpoint[]>("endpoints_list"));
  } catch {
    // Older backend: fall back to the snapshot until an update arrives.
  }
}

function subscribe(l: () => void) {
  listeners.add(l);
  start();
  return () => listeners.delete(l);
}

/** Calls `l` whenever a new endpoint list arrives (outside React); returns the unsubscribe. */
export function subscribeEndpoints(l: () => void): () => void {
  const off = subscribe(l);
  return () => {
    off();
  };
}

const NONE: Endpoint[] = [];

/** Every endpoint on the cluster; never null (empty until anything is known). */
export function useEndpoints(): Endpoint[] {
  const list = useSyncExternalStore(subscribe, () => endpoints);
  const snap = useCluster();
  const fallback = useMemo(() => (list || !snap ? NONE : fromSnapshot(snap)), [list, snap]);
  return list ?? fallback;
}

export function useEndpoint(id: string | null | undefined): Endpoint | undefined {
  const all = useEndpoints();
  return id ? all.find((e) => e.id === id) : undefined;
}

/** The latest list from the backend, for code outside React. */
export function getEndpoints(): Endpoint[] {
  return endpoints ?? NONE;
}

/** Asks the backend to probe every endpoint now instead of waiting for the next round. */
export async function refreshEndpoints(): Promise<Endpoint[]> {
  const list = await invoke<Endpoint[]>("endpoints_refresh");
  set(list);
  return list;
}

export interface Supervisor {
  enabled: boolean;
  state: "watching" | "restarting" | "gave-up";
  /** Restarts in the last 30 minutes; once it gave up, the count it gave up at. */
  restarts: number;
  /** Unix ms of the next restart, while one is scheduled. */
  nextRetryMs: number | null;
  /** "Held · look-only" while look-only holds it; "Held · locked" after a relock dropped a retry; or why it never restarts this load (supervisor.rs). */
  held?: string | null;
}

/** How an endpoint's state reads: the word, its tone, and the reason when it isn't Ready. */
export interface EndpointView {
  /** "Ready", "Loading 42%", "Loading", "Down", "Stopped". */
  word: string;
  /** Ready is quiet (a neutral dot); Loading is in progress; Down is loud; idle and paused are neutral. */
  tone: "ok" | "progress" | "crit" | "off" | "idle" | "paused";
  /** Why it isn't Ready, when it isn't. */
  reason: string | null;
  pct: number | null;
  /** Its numbers are meaningless right now (show "—"). */
  stale: boolean;
}

/** The words for the quiet states vocabulary.ts doesn't list (r4). */
const QUIET_STATE: Record<"idle" | "gone" | "paused" | "unknown", string> = { idle: "Idle", gone: "Not found", paused: "Paused", unknown: "Unknown" };

export function endpointView(e: Pick<Endpoint, "health" | "healthReason" | "message" | "loadingPct">): EndpointView {
  const reason = e.health === "ready" ? null : e.healthReason ?? e.message ?? null;
  const pct = e.health === "loading" && e.loadingPct != null ? Math.round(e.loadingPct) : null;
  const h = e.health;
  const word = pct != null ? `${ENDPOINT_STATE.loading} ${pct}%` : h === "idle" || h === "gone" || h === "paused" || h === "unknown" ? QUIET_STATE[h] : ENDPOINT_STATE[h];
  // Idle, gone and paused are neutral: at rest, vanished without evidence, or planned.
  const tone = h === "ready" ? "ok" : h === "loading" ? "progress" : h === "down" ? "crit" : h === "idle" ? "idle" : h === "paused" ? "paused" : "off";
  return { word, tone, reason, pct, stale: e.health !== "ready" };
}

/** "420 ms", or "250–500 ms" when the engine's histogram only says it was in that bucket. */
export function fmtLatency(ms: number | null | undefined, range?: [number, number] | null): string {
  const one = (v: number) => (v >= 1000 ? `${(v / 1000).toFixed(v >= 10_000 ? 0 : 1)} s` : `${Math.round(v)} ms`);
  if (range && range[1] > range[0]) {
    if (range[1] >= 1000) return `${(range[0] / 1000).toFixed(range[0] % 1000 ? 2 : 0)}–${(range[1] / 1000).toFixed(range[1] % 1000 ? 1 : 0)} s`;
    return `${Math.round(range[0])}–${Math.round(range[1])} ms`;
  }
  if (ms == null || !Number.isFinite(ms)) return "—";
  return one(ms);
}

// ---- Opening Chat on a model, from anywhere

export interface ChatPick {
  providerId: string;
  model: string;
  /** Bumps on every request, so asking twice for the same model still starts a new chat. */
  seq: number;
}

let pick: ChatPick | null = null;
let pickSeq = 0;
const pickListeners = new Set<() => void>();

/**
 * Starts a new chat with `model` from `providerId`. The Chat window picks it up when it
 * mounts or, if it's already open, right away; the window itself is opened through the
 * `sparkbox:open-window` event (see useOpenChatWith for components that hold the WM).
 */
export function openChatWith(providerId: string, model: string) {
  pick = { providerId, model, seq: ++pickSeq };
  pickListeners.forEach((l) => l());
  window.dispatchEvent(new CustomEvent("sparkbox:open-window", { detail: { kind: "chat", params: {} } }));
}

export function usePendingChat(): ChatPick | null {
  return useSyncExternalStore(
    (l) => {
      pickListeners.add(l);
      return () => pickListeners.delete(l);
    },
    () => pick,
  );
}

export function takePendingChat(seq: number) {
  if (pick?.seq === seq) pick = null;
}

// ---- The snapshot fallback (mirrors the Rust discovery, without probes or metrics)

const DEFAULT_PORT: Record<Engine, number> = { vllm: 8000, ollama: 11434, sglang: 30000, llamacpp: 8080, trtllm: 8000, nim: 8000, other: 0 };

function flag(words: string[], names: string[]): string | undefined {
  for (let i = 0; i < words.length; i++) {
    for (const n of names) {
      if (words[i] === n) return words[i + 1];
      if (words[i].startsWith(`${n}=`)) return words[i].slice(n.length + 1);
    }
  }
  return undefined;
}

function hostPort(ports: string[]): number | undefined {
  for (const p of ports) {
    const m = /:(\d+)->/.exec(p);
    if (m) return Number(m[1]);
  }
  return undefined;
}

export function parseCommand(cmd: string): { engine: Engine; model?: string; port?: number; tp?: number; pp?: number } | null {
  const words = cmd.trim().split(/\s+/);
  const num = (v?: string) => (v && /^\d+$/.test(v) ? Number(v) : undefined);
  const serve = words.indexOf("serve");
  if (/(^|\/)vllm$/.test(words[0] ?? "") && serve >= 0) {
    return {
      engine: "vllm",
      model: flag(words, ["--served-model-name"]) ?? (words[serve + 1]?.startsWith("-") ? flag(words, ["--model"]) : words[serve + 1]),
      port: num(flag(words, ["--port"])),
      tp: num(flag(words, ["--tensor-parallel-size", "-tp"])),
      pp: num(flag(words, ["--pipeline-parallel-size", "-pp"])),
    };
  }
  if (cmd.includes("vllm.entrypoints.openai.api_server")) {
    return {
      engine: "vllm",
      model: flag(words, ["--served-model-name", "--model"]),
      port: num(flag(words, ["--port"])),
      tp: num(flag(words, ["--tensor-parallel-size", "-tp"])),
      pp: num(flag(words, ["--pipeline-parallel-size", "-pp"])),
    };
  }
  if (cmd.includes("sglang.launch_server")) {
    return { engine: "sglang", model: flag(words, ["--served-model-name", "--model-path", "--model"]), port: num(flag(words, ["--port"])), tp: num(flag(words, ["--tp", "--tp-size"])) };
  }
  if (/(^|\/)llama-server$/.test(words[0] ?? "")) {
    return { engine: "llamacpp", model: flag(words, ["--alias", "-a", "--model", "-m"])?.split("/").pop(), port: num(flag(words, ["--port"])) };
  }
  if (/(^|\/)ollama$/.test(words[0] ?? "") && words[1] === "serve") return { engine: "ollama" };
  return null;
}

function fromSnapshot(snap: ClusterSnapshot): Endpoint[] {
  const out: Endpoint[] = [];
  const workers = snap.nodes.filter((n) => n.containers.some((c) => c.state === "running" && /ray start/.test(c.command) && !/--head/.test(c.command)));
  for (const n of snap.nodes) {
    if (n.status !== "online") continue;
    for (const c of n.containers) {
      if (c.state !== "running") continue;
      const p = parseCommand(c.command);
      if (!p) continue;
      const port = p.port ?? hostPort(c.ports) ?? DEFAULT_PORT[p.engine];
      const tp = p.tp ?? 1;
      const pp = p.pp ?? 1;
      const extra = Math.max(0, tp * pp - 1);
      const nodes = [n.id, ...workers.filter((w) => w.id !== n.id).slice(0, extra).map((w) => w.id)];
      const ip = n.nets.find((x) => x.role === "lan")?.ip ?? n.host;
      const deployment = c.labels?.["sparkbox.deployment"];
      out.push({
        id: `${n.id}-${p.engine}-${port}`,
        engine: p.engine,
        models: p.model ? [p.model] : [],
        nodes,
        headNode: n.id,
        port,
        url: `http://${ip}:${port}/v1`,
        managedBy: deployment ? "sparkbox" : "external",
        // Not probed yet: never Ready on the snapshot's word alone.
        health: "loading",
        healthReason: "Checking that it answers…",
        parallel: tp * pp > 1 ? { tp, pp } : null,
        containerIds: [c.id],
        containerNames: [c.name],
        deploymentId: deployment ?? null,
        metrics: null,
        source: deployment ? "deployment" : "discovered",
      });
    }
  }
  return out;
}
