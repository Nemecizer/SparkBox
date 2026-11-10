import { reviewDemoChain, startDemoChain, cancelDemoChain, demoChainJobs, demoChainLibrary } from "./chains";
// AI backend for the browser preview: the Demo world's endpoints (Llama 3.3 70B
// pipeline-parallel over cell1–cell3, Ollama on gx10-0000) with metrics that
// move like the Rust simulator's, the model library, the fit table and memory
// plan for Qwen3.8 27B FP8, providers, chats with a branch, and the server log.
// Numbers follow the same arithmetic as src-tauri/src/models (fit.rs, sim.rs).
import type { Endpoint, EndpointMetrics } from "../../lib/endpoints";
import type { ChatMessage, Conversation, DeployPlan, DeploySpec, Deployment, FitRow, HfModel, LibraryEntry, ModelDetails, NodePlan, ProviderView, ServerLogEntry, ServerView } from "../../lib/modelsApi";
import type { ClusterDeployment } from "../../lib/types";
import { channelEnd, channelSend, emitMock, type MockHandlers } from "../../test/tauri";
import { preparedImageJobs, downloadRecoveryJobs, resumeDemoDownload, cancelDemoDownload, demoDownloadTools, startDemoDownload } from "./jobs";

import { demoHistory } from "./deploymentHistory";
import { demoDiskPreflight, demoDownloadCapacity } from "./disk";
import type { DiskRequest, DownloadSelection } from "../../lib/types";

const GiB = 1024 ** 3;
const TOTAL = 128_530_000_000;
const now = () => Date.now();
const nowS = () => Math.floor(Date.now() / 1000);
// Browser-only regression fixture: first reply is interrupted; later replies finish.
let interruptNextChat = typeof location !== "undefined" && new URLSearchParams(location.search).get("chat") === "interrupted";

// ---- Metrics that move

function wave(t: number, period: number, phase = 0) {
  return 0.5 + 0.5 * Math.sin(t / period + phase);
}

/** One endpoint's 15 minutes of history ending now, from smooth curves plus bursts. */
function history(kind: "llama" | "ollama", endS: number) {
  const ts: number[] = [], gen: number[] = [], run: number[] = [], kv: number[] = [], prompt: number[] = [], wait: number[] = [], ttft: (number | null)[] = [];
  for (let i = 299; i >= 0; i--) {
    const t = endS - i * 3;
    ts.push(t);
    if (kind === "llama") {
      const burst = ((t / 420) % 1) < 0.08 ? 4 : 0;
      const r = Math.max(0, Math.round(2 * (0.45 + wave(t, 240)) + burst + (Math.sin(t * 1.7) * 0.4)));
      const running = Math.min(r, 16);
      const per = 1.645 / (1 + 0.015 * Math.max(0, running - 3));
      run.push(running);
      wait.push(Math.max(0, r - 16));
      gen.push(+(running * per * (0.96 + 0.08 * wave(t, 7))).toFixed(2));
      prompt.push(+(running ? (running / 170) * 1400 * (0.7 + 0.6 * wave(t, 13)) : 0).toFixed(1));
      kv.push(+Math.min(100, (running * 9000 * 100) / 331_000).toFixed(1));
      ttft.push(running ? Math.round(380 + 90 * wave(t, 31) + (burst ? 500 : 0)) : null);
    } else {
      const chatting = Math.sin(t / 90) > 0.35 || Math.cos(t / 37) > 0.92;
      run.push(chatting ? 1 : 0);
      wait.push(0);
      gen.push(chatting ? +(48 * (0.92 + 0.16 * wave(t, 5))).toFixed(1) : 0);
      prompt.push(chatting ? 50 : 0);
      kv.push(0);
      ttft.push(chatting ? Math.round(125 + 40 * wave(t, 11)) : null);
    }
  }
  return { ts, genTps: gen, running: run, kvPct: kv, promptTps: prompt, waiting: wait, ttftP50Ms: ttft };
}

function metrics(kind: "llama" | "ollama"): EndpointMetrics {
  const h = history(kind, nowS());
  const last = h.ts.length - 1;
  const ttfts = h.ttftP50Ms.slice(-20).filter((x): x is number => x != null);
  const p50 = ttfts.length ? ttfts.sort((a, b) => a - b)[Math.floor(ttfts.length / 2)] : null;
  return {
    genTps: h.genTps[last],
    promptTps: h.promptTps[last],
    running: h.running[last],
    waiting: h.waiting[last],
    kvPct: h.kvPct[last],
    kvKnown: kind === "llama",
    ttftP50Ms: p50,
    ttftP95Ms: p50 != null ? Math.round(p50 * (kind === "llama" ? 2.25 : 2.2)) : null,
    itlP50Ms: kind === "llama" ? 607.5 : 20.6,
    e2eP50Ms: kind === "llama" ? 171_000 : 6_400,
    e2eP95Ms: kind === "llama" ? 236_000 : 11_200,
    reqPerS: kind === "llama" ? +(h.running[last] / 170).toFixed(3) : h.running[last] / 12,
    prefixHitPct: kind === "llama" ? 46 : null,
    updatedMs: now(),
    history: h,
  };
}

// ---- The world

const llamaId = "cell1-vllm-8000";
const ollamaId = "gx10-0000-ollama-11434";

const library: LibraryEntry[] = [
  {
    key: "/home/sparkuser/models/meta-llama/Llama-3.3-70B-Instruct", inUse: llamaId, repo: "meta-llama/Llama-3.3-70B-Instruct", revision: "demo", format: "safetensors",
    sizeBytes: 141_107_412_992, root: "/home/sparkuser/models",
    copies: Object.fromEntries(["cell1", "cell2", "cell3"].map((n) => [n, { path: "/home/sparkuser/models/meta-llama/Llama-3.3-70B-Instruct", sizeBytes: 141_107_412_992, verified: true }])),
  },
  {
    key: "/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8", inUse: null, repo: "Qwen/Qwen3.8-27B-FP8", revision: "demo", format: "safetensors",
    sizeBytes: 30_923_764_531, root: "/home/sparkuser/models",
    copies: Object.fromEntries(["cell1", "gx10-0000"].map((n) => [n, { path: "/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8", sizeBytes: 30_923_764_531, verified: true }])),
  },
];

const deployments: Deployment[] = [];
const started = new Map<string, number>();

function endpoints(): Endpoint[] {
  const out: Endpoint[] = [
    {
      id: llamaId, engine: "vllm", models: ["llama-3.3-70b"], nodes: ["cell1", "cell2", "cell3"], headNode: "cell1", port: 8000,
      url: "http://192.168.50.51:8000/v1", managedBy: "external", health: "ready", parallel: { tp: 1, pp: 3 },
      lastOkMs: now() - 3000, expectedNodes: ["cell1", "cell2", "cell3"], missingNodes: [], restarts: null, supervisor: null,
      containerIds: ["3f9a1c27b0de"], containerNames: ["vllm-head"], deploymentId: null, metrics: metrics("llama"), metricsSource: "server",
      providerId: llamaId, maxContext: 32768, listen: "all", source: "discovered", probedMs: now() - 4000,
      modelPath: "/models/meta-llama/Llama-3.3-70B-Instruct",
      // Its users keep it busy: the token counter's rise is the proof it generates.
      canary: { atMs: now() - 95_000, ok: true, latencyMs: 641 }, passiveOkMs: now() - 3000,
    },
    {
      id: ollamaId, engine: "ollama", models: ["gpt-oss:20b", "qwen3:32b"], nodes: ["gx10-0000"], headNode: "gx10-0000", port: 11434,
      url: "http://192.168.50.181:11434/v1", managedBy: "external", health: "ready", parallel: null,
      lastOkMs: now() - 3000, expectedNodes: ["gx10-0000"], missingNodes: [],
      containerIds: ["b2e47d901c55"], containerNames: ["ollama"], deploymentId: null, metrics: metrics("ollama"), metricsSource: "sparkbox",
      providerId: ollamaId, maxContext: null, listen: "all", source: "discovered", probedMs: now() - 4000,
      canary: { atMs: now() - 12_000, ok: true, latencyMs: 152 },
      ollama: [
        { name: "gpt-oss:20b", sizeBytes: 14_600_000_000, vramBytes: 14_600_000_000, expiresAt: nowS() + 240, contextLength: 8192 },
        { name: "qwen3:32b", sizeBytes: 29_960_000_000, vramBytes: 29_960_000_000, expiresAt: nowS() + 240, contextLength: 8192 },
      ],
      label: "Ollama on gx10-0000", reach: "lan", onDisk: ["gpt-oss:20b", "qwen3:32b"], onDiskBytes: [13_780_173_839, 20_201_253_588],
    },
    // The real cells' Ollama, as found on Oct 29: a systemd service on 127.0.0.1, nothing in memory.
    ...[["cell1", [] as string[], [] as number[]], ["cell3", ["gemma4:latest"], [9_556_302_029]]].map(([node, tags, bytes]) => ({
      id: `${node}-ollama-11434`, engine: "ollama" as const, models: tags as string[], nodes: [node as string], headNode: node as string, port: 11434,
      url: "http://127.0.0.1:11434/v1", managedBy: "external" as const, health: "idle" as const, healthReason: "Nothing in memory", message: "Nothing in memory",
      lastOkMs: now() - 4000, expectedNodes: [node as string], missingNodes: [], containerIds: [], containerNames: [], deploymentId: null, metrics: null,
      providerId: null, listen: "local", source: "discovered" as const, probedMs: now() - 4000, ollama: [],
      label: `Ollama on ${node}`, reach: "node-only" as const, onDisk: tags as string[], onDiskBytes: bytes as number[],
    })),
  ];
  for (const d of deployments.filter((x) => x.state !== "stopped")) {
    const age = (now() - (started.get(d.spec.id) ?? now())) / 1000;
    if (d.state === "starting" && age > 14) {
      d.state = "ready";
      d.stage = { kind: "ready" };
      d.message = `Serving at ${d.endpoint} (simulated)`;
      d.providerId = `vllm-${d.spec.id}`;
    } else if (d.state === "starting") {
      const p = Math.min(99, Math.round((age / 12) * 100));
      d.stage = { kind: "loading", percent: p };
      d.message = `Loading weights ${p}%`;
    }
    const m = metrics("llama");
    out.unshift({
      id: `${d.spec.nodes[0]}-vllm-${d.spec.port}`, engine: "vllm", models: [d.spec.servedName], nodes: d.spec.nodes, headNode: d.spec.nodes[0], port: d.spec.port,
      url: d.endpoint, managedBy: "sparkbox", health: d.state === "ready" ? "ready" : "loading",
      loadingPct: d.stage?.kind === "loading" ? d.stage.percent : null, message: d.state === "ready" ? null : d.message,
      healthReason: d.state === "ready" ? null : d.message, lastOkMs: d.state === "ready" ? now() - 2000 : null, expectedNodes: d.spec.nodes, missingNodes: [],
      supervisor: d.spec.keepRunning && d.spec.nodes.length > 1 ? { enabled: true, state: "watching", restarts: 0, nextRetryMs: null } : null,
      parallel: d.spec.nodes.length > 1 ? { tp: d.spec.tensorParallel, pp: d.spec.pipelineParallel } : null,
      containerIds: d.state === "ready" ? ["e81c0b5f2a93"] : [], containerNames: [`sparkbox-${d.spec.id}`], deploymentId: d.spec.id,
      metrics: d.state === "ready" ? { ...m, genTps: +(m.genTps * 2.7).toFixed(1), running: Math.min(1, m.running), kvPct: 1.4, kvKnown: true, ttftP50Ms: 310, itlP50Ms: 141 } : null,
      metricsSource: "server", providerId: d.state === "ready" ? `vllm-${d.spec.id}` : null, maxContext: d.spec.maxModelLen ?? 262144,
      listen: d.spec.listen ?? "lan", source: "deployment", modelPath: d.spec.modelPath,
      canary: d.state === "ready" ? { atMs: now() - 20_000, ok: true, latencyMs: 84 } : null,
    });
  }
  return out;
}

// ---- Fit and plan, with fit.rs's arithmetic for Qwen3.8 27B FP8

const QWEN_KV = 16 * 2 * 4 * 256 * 2;
const QWEN_STATE = 48 * (48 * 128 * 128 * 2 + (2 * 16 * 128 + 48 * 128) * 3 * 2);
const VISION = 12 * 27 * 1152 * 1152 * 2 + GiB;

function fitRows(weight: number, fp8: boolean): FitRow[] {
  const kv = QWEN_KV * (fp8 ? 0.5 : 1);
  const free: [string, number][] = [["cell1", 12.9e9], ["cell2", 12.9e9], ["cell3", 12.9e9], ["gx10-0000", 79.7e9]];
  return [1, 2, 3, 4].map((n) => {
    const room = TOTAL * 0.8 - weight / n - 6 * GiB - VISION;
    const tpOk = n !== 3;
    const shards = n === 1 ? 1 : tpOk ? Math.min(4, n) : n;
    const cap = room * shards;
    const need = weight / n + 6 * GiB + VISION + 2 * GiB;
    const ok = free.filter(([, f]) => f >= need).map(([id]) => id);
    return {
      nodes: n, weightsPerNode: weight / n, budgetPerNode: TOTAL * 0.8, kvRoomPerNode: room, fits: true, tensorParallelOk: tpOk, pipelineParallelOk: true,
      kvCapacityTokens: Math.floor(cap / kv), maxContextTokens: Math.min(262144, Math.floor((cap - QWEN_STATE) / kv)),
      fullRequests: cap / (262144 * kv + QWEN_STATE), decodeCeilingTps: (0.85 * 273e9 * (tpOk ? n : 1)) / weight, pipelineOnly: !tpOk, kvDtype: fp8 ? "fp8" : "bf16",
      fitsNow: { ok: ok.length >= n, freeNodes: ok, text: ok.length >= n ? `yes, on ${ok.slice(0, n).join(", ")}` : ok.length ? `only ${ok.join(", ")} free` : "no node has room" },
    };
  });
}

/** gpt-oss-120b as the Hub lists it (src-tauri/src/models/fixtures/hf): 60.8 GiB of root shards in a 182 GiB repo. */
function gptOss(): HfModel {
  return {
    id: "openai/gpt-oss-120b", revision: "b5c939de8f754692c1647ca79fbf85e8c1e70f8a", gated: false, license: "apache-2.0", pipeline: "text-generation", library: "transformers",
    params: 116_829_156_672, dtypes: { BF16: 2_167_371_072, U8: 114_661_785_600 }, weightBytes: 65_248_893_184, weightsFormat: "safetensors", quantization: "mxfp4",
    architectures: ["GptOssForCausalLM"], modelType: "gpt_oss", files: [], readmeExcerpt: "",
    downloadBytes: 65_276_800_000, repoBytes: 195_764_040_609,
    skipped: [{ what: "original/", files: 7, bytes: 65_248_869_800 }, { what: "metal/", files: 1, bytes: 65_238_253_568 }],
  };
}

function details(repo: string): ModelDetails {
  if (repo === "openai/gpt-oss-120b") return gptOssDetails();
  const weight = 30_923_764_531;
  return {
    model: {
      id: repo, revision: "8c2f0d1e4b7a9c3d5e6f7a8b9c0d1e2f3a4b5c6d", gated: false, license: "apache-2.0", pipeline: "image-text-to-text", library: "transformers",
      params: 27_300_000_000, dtypes: { BF16: 1_900_000_000, F8_E4M3: 25_400_000_000 }, weightBytes: weight, weightsFormat: "safetensors", quantization: "fp8",
      architectures: ["Qwen3_8ForConditionalGeneration"], modelType: "qwen3_8", files: [], readmeExcerpt: "",
      downloadBytes: weight + 11_400_000, repoBytes: weight + 11_400_000, skipped: [],
    },
    arch: {
      layers: 64, kvLayers: 16, heads: 24, kvHeads: 4, headDim: 256, mlaDim: null, experts: null, expertsActive: null, maxContext: 262144,
      linearLayers: 48, stateBytesPerSeq: QWEN_STATE, visionBytes: VISION,
    },
    kvBytesPerToken: QWEN_KV,
    fit: fitRows(weight, false),
    fitFp8: fitRows(weight, true),
    nodeMemory: TOTAL,
    nodesAvailable: 4,
  };
}

/** fit.rs's table for gpt-oss-120b: 36 layers, 8 KV heads of 64, 128 experts (MoE: no decode ceiling). */
function gptOssDetails(): ModelDetails {
  const weight = 65_248_893_184;
  const kv = 18 * 2 * 8 * 64 * 2;
  return {
    model: gptOss(),
    arch: { layers: 36, kvLayers: 36, heads: 64, kvHeads: 8, headDim: 64, mlaDim: null, experts: 128, expertsActive: 4, maxContext: 131072, linearLayers: 0, stateBytesPerSeq: 0, visionBytes: 0 },
    kvBytesPerToken: 36 * 2 * 8 * 64 * 2,
    fit: [1, 2, 3, 4].map((n) => {
      const room = TOTAL * 0.8 - weight / n - 6 * GiB;
      const tpOk = n !== 3;
      const cap = room * (n === 1 ? 1 : tpOk ? Math.min(8, n) : n);
      return {
        nodes: n, weightsPerNode: weight / n, budgetPerNode: TOTAL * 0.8, kvRoomPerNode: room, fits: true, tensorParallelOk: tpOk, pipelineParallelOk: true,
        kvCapacityTokens: Math.floor(cap / (kv * 2)), maxContextTokens: 131072, fullRequests: cap / (131072 * kv * 2), decodeCeilingTps: null, pipelineOnly: !tpOk,
        kvDtype: "bf16" as const, fitsNow: null,
      };
    }),
    fitFp8: [],
    nodeMemory: TOTAL,
    nodesAvailable: 4,
  };
}

const HOLDERS: Record<string, { name: string; bytes: number }[]> = {
  cell1: [{ name: "vllm-head", bytes: 0.7 * TOTAL }],
  cell2: [{ name: "vllm-worker", bytes: 0.7 * TOTAL }],
  cell3: [{ name: "vllm-worker", bytes: 0.7 * TOTAL }],
  "gx10-0000": [{ name: "ollama", bytes: 44_560_000_000 }],
};

function plan(spec: DeploySpec): DeployPlan {
  const e = library.find((l) => l.key === spec.modelPath) ?? library[1];
  const weight = e.sizeBytes;
  const n = Math.max(1, spec.nodes.length);
  const len = spec.maxModelLen ?? 262144;
  const kvOne = (len * QWEN_KV * (spec.kvCacheDtype === "fp8" ? 0.5 : 1) + QWEN_STATE) / (n === 3 ? 3 : Math.min(4, n));
  const kv = kvOne * (spec.maxNumSeqs ?? 1);
  const nodes: NodePlan[] = spec.nodes.map((node, i) => {
    const holders = HOLDERS[node] ?? [];
    const used = holders.reduce((a, h) => a + h.bytes, 0);
    const share = spec.gpuMemoryUtilization;
    const budget = TOTAL * share;
    const available = TOTAL - used - 4 * GiB;
    const overhead = 6 * GiB + (i === 0 ? VISION : 0);
    const need = weight / n + kv + overhead;
    const maxShare = Math.floor((available / TOTAL) * 20) / 20;
    // As plan.rs: everything in use before the load (holders, the OS, vLLM's own ~9.4 GB process), then its share.
    const inUse = used + 4 * GiB + 9.4e9;
    const afterPct = (100 * (inUse + budget)) / TOTAL;
    const safeShare = Math.min(maxShare, Math.max(0, Math.floor(((0.9 * TOTAL - inUse) / TOTAL) * 20 - 1e-9) / 20));
    const g = (b: number) => (b / GiB).toFixed(1);
    let level: NodePlan["level"] = "green";
    let verdict = "Fits";
    let reason: string | null = null;
    if (budget > available) {
      [level, verdict] = ["red", "Won't start"];
      reason = `Won't start: vLLM reserves ${g(budget)} GiB (${share.toFixed(2)} share) at startup; ${g(available)} GiB is free.`;
    } else if (need >= budget) {
      [level, verdict] = ["red", "Won't fit"];
      reason = `Won't fit: needs ${g(need)} GiB, but a ${share.toFixed(2)} share gives vLLM ${g(budget)} GiB. Lower the context or requests at once, or use more nodes.`;
    } else if (afterPct >= 95) {
      [level, verdict] = ["red", "Won't start safely"];
      reason = `Won't start safely: with this load ${node} would use ${afterPct.toFixed(0)}% of its memory, and a Spark that runs out hangs.`;
    } else if (afterPct >= 90) {
      [level, verdict] = ["amber", "Tight"];
      reason = `Tight: runs above the 90% alert line (${afterPct.toFixed(0)}% of ${node}'s memory).`;
    }
    return {
      node, weights: weight / n, kv, overhead, need, budget, free: Math.min(budget, available), available, total: TOTAL, holders, level, verdict,
      inUse, afterPct, maxShare, safeShare, fitsSafe: need < TOTAL * safeShare,
      safeContext: need < TOTAL * safeShare ? null : Math.floor((TOTAL * safeShare - weight / n - overhead - QWEN_STATE) / QWEN_KV / 1024) * 1024 || null, reason,
    };
  });
  const level = nodes.some((p) => p.level === "red") ? "red" : nodes.some((p) => p.level === "amber") ? "amber" : "green";
  const taken = spec.nodes[0] === "cell1" ? [8000, 8265, 3000] : spec.nodes[0] === "gx10-0000" ? [11434] : [];
  const ip = { cell1: "192.168.50.51", cell2: "192.168.50.154", cell3: "192.168.50.98", "gx10-0000": "192.168.50.181" }[spec.nodes[0] ?? "cell1"] ?? "";
  return {
    nodes, level, needs: { weightBytes: weight, arch: details(e.repo).arch, kvBytesPerToken: QWEN_KV },
    portTakenBy: taken.includes(spec.port) ? (spec.port === 8000 ? "python3" : "ollama") : null,
    suggestedPort: [8000, 8001, 8002].find((p) => !taken.includes(p)) ?? 8001,
    host: spec.listen === "all" ? "0.0.0.0" : ip, url: `http://${ip}:${spec.port}/v1`, keyPreview: spec.requireKey === false ? null : "sbk-••••••••", maxContext: 262144, recipe: null, topologies: [
      { id: "single", tp: 1, pp: 1, replicas: 1, nodes: 1, ok: true, reason: null, advanced: false, unlisted: false },
      { id: "tp2", tp: 2, pp: 1, replicas: 1, nodes: 2, ok: true, reason: null, advanced: false, unlisted: false },
      { id: "pp3", tp: 1, pp: 3, replicas: 1, nodes: 3, ok: true, reason: null, advanced: false, unlisted: false },
      { id: "replicas3", tp: 1, pp: 1, replicas: 3, nodes: 3, ok: true, reason: "Three independent endpoints; shared routing is future work.", advanced: false, unlisted: false },
      { id: "tp4", tp: 4, pp: 1, replicas: 1, nodes: 4, ok: true, reason: null, advanced: false, unlisted: false },
      { id: "replicas2tp2", tp: 2, pp: 1, replicas: 2, nodes: 4, ok: true, reason: "Two independent endpoints; shared routing is future work.", advanced: false, unlisted: false },
    ],
    suggestedContext: (() => {
      if (nodes.every((p) => p.level === "green")) return null;
      const safe = Math.min(...nodes.map((p) => p.safeShare));
      if (nodes.every((p) => p.need < p.total * safe)) return null;
      const ctx = Math.min(...nodes.map((p) => p.safeContext ?? 0));
      return ctx >= 4096 ? [safe, Math.min(ctx, spec.maxModelLen ?? 262144)] as [number, number] : null;
    })(),
    suggestedShare: (() => {
      if (nodes.every((p) => p.level === "green")) return null;
      const safe = Math.min(...nodes.map((p) => p.safeShare));
      return nodes.every((p) => p.need < p.total * safe) && safe >= 0.2 && safe < spec.gpuMemoryUtilization ? safe : null;
    })(),
    fullRequests: nodes.length ? Math.min(...nodes.map((p) => Math.floor(Math.max(0, p.budget - p.weights - p.overhead) / kvOne))) : null,
    // As plan.rs sized_share.
    sizedShare: (() => {
      const wanted = Math.max(...nodes.map((p) => (p.weights + p.overhead + kvOne * (spec.maxNumSeqs ?? 4)) * 1.1 / p.total));
      const sized = Math.max(0.2, Math.ceil(wanted * 20 - 1e-9) / 20);
      const share = Math.min(sized, ...nodes.map((p) => p.safeShare));
      return sized <= 0.75 && share >= 0.2 && nodes.every((p) => p.need < p.total * share) ? share : null;
    })(),
  };
}

function preview(spec: DeploySpec) {
  const args = [
    "serve", spec.modelPath, "--served-model-name", spec.servedName, "--host", spec.listen === "all" ? "0.0.0.0" : "192.168.50.181", "--port", String(spec.port),
    "--gpu-memory-utilization", spec.gpuMemoryUtilization.toFixed(2),
    ...(spec.maxModelLen ? ["--max-model-len", String(spec.maxModelLen)] : []),
    ...(spec.maxNumSeqs ? ["--max-num-seqs", String(spec.maxNumSeqs)] : []),
    ...(spec.kvCacheDtype === "fp8" ? ["--kv-cache-dtype", "fp8"] : []),
    ...(spec.toolCallParser ? ["--enable-auto-tool-choice", "--tool-call-parser", spec.toolCallParser] : []),
    ...(spec.reasoningParser ? ["--reasoning-parser", spec.reasoningParser] : []),
    ...(spec.allowRemoteCode ? ["--trust-remote-code"] : []),
    ...spec.extraArgs,
  ].map((a) => `'${a}'`).join(" ");
  const key = spec.requireKey === false ? "" : `# stdin: VLLM_API_KEY=•••••••• (a new key, kept in the Keychain as llm:vllm-${spec.id})\numask 077 && mkdir -p ~/.sparkbox/keys && rm -f ~/.sparkbox/keys/${spec.id} && IFS= read -r k && printf '%s' "$k" > ~/.sparkbox/keys/${spec.id} && unset k && chmod 0400 ~/.sparkbox/keys/${spec.id} && `;
  return [
    { node: spec.nodes.join(", "), title: "Prepare the vLLM image (skipped when it's already there)", command: `set -e\nif ! docker image inspect sparkbox/vllm-ray:26.05-py3 >/dev/null 2>&1; then\n  docker image inspect ${spec.image} >/dev/null 2>&1 || docker pull -q ${spec.image}\n  …\nfi\necho ready` },
    { node: spec.nodes[0], title: `Check that nothing else took port ${spec.port} (stops here if something did)`, command: `ss -Hltn 'sport = :${spec.port}' 2>/dev/null` },
    {
      node: spec.nodes[0], title: "Start vLLM",
      command: `${key}docker run -d --name sparkbox-${spec.id} --label sparkbox.deployment=${spec.id} --label sparkbox.role=single --label sparkbox.model=${spec.servedName} --network host --ipc host --shm-size 16g --gpus all --restart ${spec.keepRunning ? "unless-stopped" : "no"}${spec.requireKey === false ? "" : ` -v "$HOME/.sparkbox/keys/${spec.id}:/run/sparkbox/key:ro"`} -v /home/sparkuser/models/Qwen/Qwen3.8-27B-FP8:/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8:ro ${spec.requireKey === false ? `--entrypoint vllm sparkbox/vllm-ray:26.05-py3 ${args}` : `--entrypoint sh sparkbox/vllm-ray:26.05-py3 -c 'VLLM_API_KEY="$(cat /run/sparkbox/key)" exec vllm ${args.replace(/'/g, '"')}'`}`,
    },
  ];
}

// ---- Benchmarks, with sim.rs's arithmetic

function sweep(name: string, nodes: string[], tp: number, pp: number, weights: number, agoS: number, external: boolean) {
  const single = (0.85 * 273e9 * tp) / weights;
  const prefill = (7.5e3 * tp * pp * 1e10) / weights;
  const levels = [1, 2, 4, 8, 16, 32].map((c) => {
    const per = single / (1 + 0.015 * Math.max(0, c - pp));
    const ttft = (512 * Math.min(4, Math.max(1, c))) / prefill + 0.05;
    const n = Math.max(4, 2 * c);
    const e2e = ttft + 255 / per;
    const wall = Math.ceil(n / c) * e2e;
    const pct = (v: number, spread: number) => ({ p50: v, p90: v * (1 + spread), p99: v * (1 + 2.2 * spread) });
    return {
      concurrency: c, requests: n, completed: n, failed: 0, durationS: wall, reqPerS: n / wall, outputTps: (n * 256) / wall, inputTps: (n * 512) / wall,
      ttftMs: pct(ttft * 1000, 0.12), tpotMs: pct(1000 / per, 0.04), itlMs: pct(1000 / per, 0.06), e2eMs: pct(e2e * 1000, 0.05),
    };
  });
  return {
    deployment: name, servedName: name, nodes, tensorParallel: tp, pipelineParallel: pp, at: nowS() - agoS, demo: true, external,
    endpoint: null, spec: { profile: "chat" as const, inputTokens: 512, outputTokens: 256, concurrency: [1, 2, 4, 8, 16, 32] },
    rows: levels.map((l) => ({ concurrency: l.concurrency, completed: l.completed, ttftMsMedian: Math.round(l.ttftMs.p50), tokensPerSecPerStream: 1000 / l.tpotMs.p50, aggregateTokensPerSec: l.outputTps })),
    levels,
  };
}

// ---- Providers

const OPENAI_ALL = ["babbage-002", "chatgpt-4o-latest", "dall-e-3", "gpt-4.1", "gpt-4.1-mini", "gpt-5", "gpt-5-mini", "gpt-5-nano", "gpt-5.5", "gpt-5.5-2026-08-07", "gpt-5.5-mini", "gpt-5.5-pro", "gpt-image-1", "o3", "o4-mini", "omni-moderation-latest", "sora-2", "text-embedding-3-large", "tts-1", "whisper-1"];
const OPENAI_VISIBLE = ["gpt-5.5", "gpt-5.5-pro", "gpt-5.5-mini", "gpt-5.5-2026-08-07", "gpt-5", "gpt-5-mini", "gpt-5-nano", "gpt-4.1", "gpt-4.1-mini", "o4-mini", "o3", "chatgpt-4o-latest"];

function providers(): ProviderView[] {
  const base: ProviderView[] = [
    { id: "anthropic", name: "Anthropic (Claude)", kind: "anthropic", baseUrl: "https://api.anthropic.com", enabled: true, models: ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-opus-5"], defaultModel: "claude-opus-5-5", hasKey: true, chatOnly: true, hidden: [], visible: ["claude-opus-5-5", "claude-sonnet-5-5", "claude-opus-5", "claude-haiku-5-5"], effectiveDefault: "claude-opus-5-5", discovered: false },
    { id: "openai", name: "OpenAI", kind: "openai", baseUrl: "https://api.openai.com/v1", enabled: true, models: OPENAI_ALL, defaultModel: "gpt-5.5", hasKey: true, chatOnly: true, hidden: [], visible: OPENAI_VISIBLE, effectiveDefault: "gpt-5.5", discovered: false },
    { id: "demo", name: "SparkBox demo", kind: "demo", baseUrl: "", enabled: true, models: ["echo"], defaultModel: "echo", hasKey: false, chatOnly: true, hidden: [], visible: ["echo"], effectiveDefault: "echo", discovered: false },
    { id: llamaId, name: "vLLM on cell1 :8000", kind: "demo", baseUrl: "http://192.168.50.51:8000/v1", enabled: true, models: ["llama-3.3-70b"], defaultModel: "llama-3.3-70b", hasKey: false, chatOnly: true, hidden: [], visible: ["llama-3.3-70b"], effectiveDefault: "llama-3.3-70b", discovered: true },
    { id: ollamaId, name: "Ollama on gx10-0000 :11434", kind: "demo", baseUrl: "http://192.168.50.181:11434/v1", enabled: true, models: ["gpt-oss:20b", "qwen3:32b"], defaultModel: "gpt-oss:20b", hasKey: false, chatOnly: true, hidden: [], visible: ["gpt-oss:20b", "qwen3:32b"], effectiveDefault: "gpt-oss:20b", discovered: true },
  ];
  for (const d of deployments.filter((x) => x.state === "ready")) {
    base.push({ id: `vllm-${d.spec.id}`, name: `vLLM on ${d.spec.nodes.join("+")}`, kind: "demo", baseUrl: d.endpoint, enabled: true, models: [d.spec.servedName], defaultModel: d.spec.servedName, hasKey: false, chatOnly: true, hidden: [], visible: [d.spec.servedName], effectiveDefault: d.spec.servedName, discovered: true });
  }
  return base;
}

// ---- Chat

function msg(m: Partial<ChatMessage> & Pick<ChatMessage, "role" | "content" | "id" | "parentId" | "createdAt">): ChatMessage {
  return { thinking: null, provider: null, model: null, stopReason: null, stats: null, ...m };
}

const t0 = nowS() - 3600;
const conversations: Conversation[] = [
  {
    id: "conv-pp", title: "Why is pipeline parallel slow for one user?", createdAt: t0, updatedAt: t0 + 900, provider: llamaId, model: "llama-3.3-70b", system: "", params: {},
    leaf: "a2",
    messages: [
      msg({ id: "u1", parentId: null, role: "user", content: "Why is pipeline parallel slow for one user?", createdAt: t0 }),
      msg({
        id: "a1", parentId: "u1", role: "assistant", provider: llamaId, model: "llama-3.3-70b", createdAt: t0 + 60, stopReason: "stop",
        content: "Each token has to pass through **every stage in turn**: cell1 runs layers 1–27, then cell2 runs 28–54, then cell3 the rest. With one request the other two nodes wait, so one stream decodes at the speed of a single Spark reading all 141 GB of weights: about 1.6 tokens a second.",
        stats: { ttftMs: 640, totalMs: 41_200, inputTokens: 38, outputTokens: 66, tokensPerSec: 1.6, deltas: 66, decodeMs: 40_500 },
      }),
      msg({
        id: "a2", parentId: "u1", role: "assistant", provider: llamaId, model: "llama-3.3-70b", createdAt: t0 + 300, stopReason: "stop",
        content: "Pipeline parallel splits the **layers**, not the work on each token.\n\n- One request: the stages run one after another, so you get one node's speed (~1.6 tok/s for a bf16 70B).\n- Many requests: every stage is busy with a different request, so the *total* goes up.\n\nFor a faster single stream use **tensor parallel** (TP 2 or 4): each node reads only its share of the weights for every token.\n\n```bash\nvllm serve meta-llama/Llama-3.3-70B-Instruct --tensor-parallel-size 4\n```\n\n![throughput chart](https://example.com/chart.png)",
        stats: { ttftMs: 590, totalMs: 58_900, inputTokens: 38, outputTokens: 94, tokensPerSec: 1.62, deltas: 94, decodeMs: 57_400 },
      }),
    ],
  },
  {
    id: "conv-gpt", title: "Summarise the fabric doctor output", createdAt: t0 - 86400, updatedAt: t0 - 80000, provider: "openai", model: "gpt-5.5", system: "", params: {},
    leaf: "g2",
    messages: [
      msg({ id: "g1", parentId: null, role: "user", content: "Summarise: enP2p1s0f1np1 has 3 receive errors, MTU 9000 everywhere.", createdAt: t0 - 86400 }),
      msg({ id: "g2", parentId: "g1", role: "assistant", provider: "openai", model: "gpt-5.5", createdAt: t0 - 86380, stopReason: "completed", content: "The fabric is healthy: jumbo frames are consistent, and three receive errors on one port is noise unless the count keeps growing.", stats: { ttftMs: 820, totalMs: 1300, inputTokens: 31, outputTokens: 28, tokensPerSec: null, deltas: 3, decodeMs: 90 } }),
    ],
  },
];

const logs: ServerLogEntry[] = [
  { id: 1, tsMs: now() - 52_000, client: "Claude Code", api: "messages", model: "local", provider: llamaId, status: 200, ms: 712, stream: true, error: null, inputTokens: 18_412, outputTokens: 214, durationMs: 133_000, tokensPerSec: 1.63, done: true },
  { id: 2, tsMs: now() - 40_000, client: "(refused)", api: "chat/completions", model: "", provider: "", status: 403, ms: 0, stream: false, error: "Host “evil.example:8484” refused: SparkBox Server only answers 127.0.0.1:8484 or localhost:8484.", inputTokens: null, outputTokens: null, durationMs: null, tokensPerSec: null, done: true },
  { id: 3, tsMs: now() - 31_000, client: "Codex", api: "responses", model: "openai/gpt-5.5", provider: "openai", status: 200, ms: 1210, stream: true, error: null, inputTokens: 9_870, outputTokens: 1_502, durationMs: 21_400, tokensPerSec: 74.9, done: true },
  { id: 4, tsMs: now() - 9_000, client: "laptop", api: "embeddings", model: "nomic", provider: ollamaId, status: 200, ms: 84, stream: false, error: null, inputTokens: 412, outputTokens: null, durationMs: 91, tokensPerSec: null, done: true },
];

const server: ServerView = {
  settings: { port: 8484, requireKey: true, autostart: true, aliases: { local: `${llamaId}/llama-3.3-70b`, fast: `${ollamaId}/gpt-oss:20b` }, allowedOrigins: [] },
  running: true, url: "http://127.0.0.1:8484", error: null,
  clients: [{ name: "Claude Code", createdAt: nowS() - 5 * 86400, preview: "sbx-4c1f9e…" }, { name: "Codex", createdAt: nowS() - 2 * 86400, preview: "sbx-a07b3d…" }],
};

let ticker: number | undefined;

const MLX = /^mlx-community\/|MLX/;
/** Served weights of real repos, as hf::sizing reads them from their files. */
const SIZES: Record<string, number> = {
  "openai/gpt-oss-120b": 65_248_893_184,
  "mlx-community/gpt-oss-120b-4bit": 65_767_710_000,
  "lmstudio-community/gpt-oss-120b-MLX-8bit": 124_167_140_000,
  "unsloth/gpt-oss-120b-unsloth-bnb-4bit": 62_089_000_000,
  "lmsys/gpt-oss-120b-bf16": 233_660_000_000,
  "amd/gpt-oss-120b-w-mxfp4-a-fp8": 65_252_000_000,
  "12bitmisfit/gpt-oss-120b-NVFP4-GGUF": 0,
};
const hit = (id: string, downloads: number, likes: number, daysAgo: number, extra: Record<string, unknown> = {}) => ({
  id, downloads, likes, pipeline: "text-generation", tags: MLX.test(id) ? ["mlx"] : [], lastModified: new Date(Date.now() - daysAgo * 86400_000).toISOString(), mlx: MLX.test(id), ...extra,
});
const GPT_OSS_HITS = [
  hit("openai/gpt-oss-120b", 2_829_813, 5_453, 63),
  hit("ggml-org/gpt-oss-120b-GGUF", 412_118, 312, 70),
  hit("lmstudio-community/gpt-oss-120b-MLX-8bit", 88_201, 64, 80),
  hit("unsloth/gpt-oss-120b-unsloth-bnb-4bit", 61_304, 41, 66),
  hit("mlx-community/gpt-oss-120b-4bit", 39_877, 58, 81),
  hit("lmsys/gpt-oss-120b-bf16", 22_510, 19, 75),
  hit("amd/gpt-oss-120b-w-mxfp4-a-fp8", 9_811, 11, 40),
  hit("12bitmisfit/gpt-oss-120b-NVFP4-GGUF", 1_240, 3, 20),
];

// The owner's failed 4-node GLM-5.3-Flash run (Oct 26), as postmortem.rs reads it from the real log.
const GLM_DECISIVE =
  "ValueError: No valid attention backend found for cuda with AttentionSelectorConfig(head_size=512, dtype=torch.bfloat16, kv_cache_dtype=bfloat16, block_size=None, use_mla=True, has_sink=False, use_sparse=True, use_mm_prefix=False, use_per_head_quant_scales=False, attn_type=AttentionType.DECODER, has_sliding_window=False, use_non_causal=False, use_batch_invariant=False, use_kv_connector=False, use_adaptive_verification=False, use_pcp=False, use_dcp=False). Reasons: {TRITON_MLA: [sparse not supported], FLASHINFER_MLA_SPARSE_SM120: [kv_cache_dtype not supported, kv_cache_dtype not supported]}.";

function clusterDeployments(): ClusterDeployment[] {
  const end = nowS() - 2 * 86_400 - 4 * 3600;
  const member = (node: string, id: string, rank: number) => ({
    node, container: "glm53", id, rank, role: rank === 0 ? "head" : "worker", state: "exited", exitCode: 1, oom: false, startedMs: (end - 54) * 1000, finishedMs: end * 1000,
  }) as ClusterDeployment["members"][number];
  return [{
    id: "glm53", label: "GLM-5.3-Flash-NVFP4", served: "glm-5.3-flash", engine: "vllm", image: "vllm/vllm-openai:glm53-flash-arm64-cu130",
    model: "/models/GLM-5.3-Flash-NVFP4", nodes: ["cell1", "cell2", "cell3", "gx10-0000"],
    members: [member("cell1", "7da4d848a41a", 0), member("cell2", "2e35a6ef4126", 1), member("cell3", "015bcb21d2bd", 2), member("gx10-0000", "9eadeae9efaf", 3)],
    state: "failed", inferred: true, how: "same image and command, ranks 0–3, started together", startedMs: (end - 56) * 1000, finishedMs: end * 1000, ranS: 54, exitCode: 1,
    postmortem: {
      title: "No attention kernel for a bf16 KV cache on GB10", cause: "sparse-mla-kv-dtype",
      explanation: "GLM-5.3-Flash uses sparse MLA attention. On GB10 (compute 12.1) this vLLM build (0.28.1rc1) has one kernel for it, FlashInfer's SM120 sparse MLA, and that kernel doesn't accept a bfloat16 KV cache; Triton MLA has no sparse support.",
      fix: "Check the model's recipe for a compatible checkpoint, image and KV cache type. Changing the KV cache type alone may not fix this. SparkBox doesn't change your launch script.",
      lines: ["attention_cls = current_platform.get_attn_backend_cls(", "raise ValueError(", GLM_DECISIVE],
      logLine: `(Worker_TP0 pid=251) ERROR 10-26 22:21:14 [multiproc_executor.py:944] ${GLM_DECISIVE}`,
      scriptLine: "~/glm53/inner.sh line 22: --kv-cache-dtype bfloat16", readMs: now(), logNode: "cell1", logContainer: "glm53",
      vllm: "0.28.1rc1", tp: 4, kvDtype: "bf16", error: null,
    },
    demo: true,
  }];
}

export const handlers: MockHandlers = {
  cluster_deployments_list: () => clusterDeployments(),
  model_restart_after: () => "task-demo-restart-after",
  library_detail: ({ key }: { key: string }) => ({
    architectures: [/glm/i.test(key) ? "Glm5NextForConditionalGeneration" : "Qwen3ForCausalLM"], modelType: null,
    quantization: /nvfp4/i.test(key) ? "nvfp4" : /fp8/i.test(key) ? "fp8" : null, weightBytes: 0, kvBytesPerToken: null,
    arch: { layers: /glm/i.test(key) ? 45 : 64, kvLayers: 16, heads: 64, kvHeads: 4, headDim: 128, mlaDim: /glm/i.test(key) ? 512 : null, experts: /glm/i.test(key) ? 288 : null, expertsActive: /glm/i.test(key) ? 8 : null, maxContext: /glm/i.test(key) ? 1048576 : 262144, linearLayers: 0, stateBytesPerSeq: 0, visionBytes: 0, sparseMla: /glm/i.test(key) },
  }),
  endpoints_list: () => {
    window.clearInterval(ticker);
    ticker = window.setInterval(() => void emitMock("endpoints://update", endpoints()), 3000);
    return endpoints();
  },
  endpoints_refresh: () => endpoints(),
  endpoint_stop: () => null,
  endpoint_restart: () => "task-demo-restart",
  endpoint_forget: () => null,
  library_list: () => structuredClone([...library, ...demoChainLibrary()]),
  jobs_list: () => [
    ...preparedImageJobs, ...downloadRecoveryJobs(), ...demoChainJobs(),
    { id: "share-a1-gx10-0000", node: "gx10-0000", kind: "pull", state: "done", step: "done", message: "", label: "Copy Qwen/Qwen3.8-27B-FP8 to gx10-0000", bytesDone: 30_923_764_531, bytesTotal: 30_923_764_531, rateBps: 2.4e9, startedAt: nowS() - 1900, updatedAt: nowS() - 1887 },
  ].sort((a, b) => b.startedAt - a.startedAt),
  job_cancel: ({ id }: { id: string }) => cancelDemoDownload(id),
  job_resume_download: resumeDemoDownload,
  model_download_tools: ({ node }: { node: string }) => demoDownloadTools(node),
  model_download_capacity: ({ selection, nodes }: { selection: DownloadSelection; nodes: string[] }) => demoDownloadCapacity(selection, nodes),
  model_disk_preflight: ({ request }: { request: DiskRequest }) => demoDiskPreflight(request, request.kind === "download" ? details(request.selection.repo).model.downloadBytes : undefined),
  model_download: startDemoDownload,
  model_chain_review: ({ selection, source, targets }) => {
    if (library.some((e) => e.repo === selection.repo && [source,...targets].some((n) => e.copies[n]))) throw new Error("A chain needs empty destinations. Keep existing Library copies.");
    return reviewDemoChain(selection, source, targets);
  },
  model_chain_start: ({ expected, tool }) => startDemoChain(expected, tool),
  model_chain_cancel: ({ node, id }) => cancelDemoChain(node, id),
  deployment_history_list: () => demoHistory(),
  deployment_history_file: () => { throw new Error("Demo shows samples only. Choose Live to view a local report file."); },
  deployments_list: () => structuredClone(deployments),
  deploy_plan: ({ spec }: { spec: DeploySpec }) => plan(spec),
  deploy_preview: ({ spec }: { spec: DeploySpec }) => preview(spec),
  deploy: ({ spec }: { spec: DeploySpec }) => {
    const id = `m${Math.random().toString(16).slice(2, 6)}`;
    const d: Deployment = {
      spec: { ...spec, id }, endpoint: plan(spec).url, state: "starting", stage: { kind: "starting" }, message: "Checking the nodes…",
      logTail: "INFO Loading safetensors checkpoint shards:   0% Completed | 0/7", startedAt: nowS(), providerId: "", demo: true,
    };
    deployments.push(d);
    started.set(id, now());
    return d;
  },
  deployment_unload: ({ id }: { id: string }) => {
    const d = deployments.find((x) => x.spec.id === id);
    if (d) {
      d.state = "stopped";
      d.message = "Unloaded";
    }
    return null;
  },
  deployment_restart: ({ id }: { id: string }) => {
    const d = deployments.find((x) => x.spec.id === id);
    if (d) {
      d.state = "starting";
      started.set(id, now());
    }
    return `task-demo-restart-${id}`;
  },
  deployment_forget: ({ id }: { id: string }) => {
    const i = deployments.findIndex((x) => x.spec.id === id);
    if (i >= 0) deployments.splice(i, 1);
    return null;
  },
  benchmarks_list: () => [sweep("llama-3.3-70b", ["cell1", "cell2", "cell3"], 1, 3, 141.1e9, 7200, true), sweep("qwen3.8-27b-fp8", ["gx10-0000"], 1, 1, 30.9e9, 1800, false)],
  bench_run: () => "task-demo-bench",
  bench_export: ({ format }: { format: string }) => `/Users/sparkuser/Downloads/SparkBox-bench-llama-3.3-70b.${format}`,
  hf_search: ({ query }: { query: string }) => /gpt-oss-120b/i.test(query) ? GPT_OSS_HITS : [
    { id: "openai/gpt-oss-120b", downloads: 2_829_813, likes: 5_453, pipeline: "text-generation", tags: ["mxfp4"], lastModified: new Date(Date.now() - 63 * 86400_000).toISOString(), params: 116_829_156_672, weightBytes: 65_248_815_744, minNodes: 1 },
    { id: "Qwen/Qwen3.8-27B-FP8", downloads: 41_230, likes: 812, pipeline: "image-text-to-text", tags: [], lastModified: new Date(Date.now() - 28 * 86400_000).toISOString(), params: 27_300_000_000, weightBytes: 29_200_000_000, minNodes: 1 },
    { id: "Qwen/Qwen3.8-27B", downloads: 88_410, likes: 1_904, pipeline: "image-text-to-text", tags: [], lastModified: new Date(Date.now() - 30 * 86400_000).toISOString(), params: 27_300_000_000, weightBytes: 54_600_000_000, minNodes: 1 },
    { id: "Qwen/Qwen3.8-235B-A22B-FP8", downloads: 19_022, likes: 640, pipeline: "text-generation", tags: [], lastModified: new Date(Date.now() - 12 * 86400_000).toISOString(), params: 235_000_000_000, weightBytes: 236_000_000_000, minNodes: 3 },
    { id: "Qwen/Qwen3-Embedding-8B", downloads: 230_118, likes: 401, pipeline: "feature-extraction", tags: [], lastModified: new Date(Date.now() - 140 * 86400_000).toISOString(), params: 7_600_000_000, weightBytes: 15_200_000_000, minNodes: 1 },
  ],
  hf_refs: ({ repo }: { repo: string }) => [
    { name: "main", kind: "branch", revision: details(repo).model.revision },
    { name: "example-release", kind: "tag", revision: "b".repeat(40) },
  ],
  hf_model: ({ repo, revision }: { repo: string; revision?: string }) => {
    const d = details(repo);
    return { ...d, model: { ...d.model, id: repo, mlx: MLX.test(repo),
      revision: revision && /^[a-f0-9]{40}$/.test(revision) ? revision : d.model.revision,
      files: [{ path: "model.safetensors", size: d.model.weightBytes, sha256: "c".repeat(64) },
        { path: "config.json", size: d.model.downloadBytes - d.model.weightBytes, sha256: null }],
      configError: new URLSearchParams(location.search).get("hf") === "config-error" ? "config.json (HTTP 403): Demo access refused. Fit estimates and download are unavailable." : null,
    } };
  },
  // Rust hf::sizing: the served weights from each repo's files (the numbers are those repos' real ones).
  hf_size: ({ repo }: { repo: string }) =>
    new Promise((resolve) => window.setTimeout(() => resolve({ id: repo, minNodes: 1, gguf: /GGUF/.test(repo), mlx: MLX.test(repo), weightsFormat: /GGUF/.test(repo) ? "none" : "safetensors", weightBytes: SIZES[repo] ?? 30_923_764_531, ...(SIZES[repo] && SIZES[repo] > 105 * GiB ? { minNodes: 2 } : {}) }), 300 + Math.random() * 700)),
  models_analyze: () => null,
  cluster_check: () => [],
  providers_list: () => providers(),
  providers_upsert: () => providers(),
  providers_refresh: () => OPENAI_ALL,
  providers_set_key: () => providers(),
  providers_clear_key: () => providers(),
  providers_scan: () => [],
  conversations_list: () => conversations.map((c) => ({ id: c.id, title: c.title, updatedAt: c.updatedAt, provider: c.provider, model: c.model, messageCount: c.messages.length })),
  conversation_get: ({ id }: { id: string }) => structuredClone(conversations.find((c) => c.id === id)),
  conversation_save: ({ conversation }: { conversation: Conversation }) => {
    const i = conversations.findIndex((c) => c.id === conversation.id);
    if (i >= 0) conversations[i] = conversation;
    else conversations.unshift(conversation);
    return null;
  },
  conversation_delete: () => null,
  chat_send: ({ request, onEvent }: { request: { model: string; provider: string }; onEvent: unknown }) => {
    if (interruptNextChat) {
      interruptNextChat = false;
      channelSend(onEvent, { type: "text", text: "The botanist reached " });
      return new Promise((resolve) => window.setTimeout(() => {
        resolve({ message: { role: "assistant", content: "The botanist reached the garden", provider: request.provider,
          model: request.model, stopReason: "interrupted", createdAt: nowS() }, error: "Demo connection closed before the reply finished." });
        // The last progress callback arrives after the command response.
        window.setTimeout(() => { channelSend(onEvent, { type: "text", text: "the garden" }); channelEnd(onEvent); }, 250);
      }, 600));
    }
    const slow = request.provider === llamaId;
    const text = slow
      ? `This is **${request.model}** in SparkBox's demo cluster, so the reply is simulated, but it streams at the speed the Endpoints window shows for this model right now.`
      : "This is the **SparkBox demo model**, so nothing left your Mac.";
    const words = text.split(/(?<= )/);
    const per = slow ? 600 : 18;
    return new Promise((resolve) => {
      let i = 0;
      const start = Date.now();
      const step = () => {
        if (i < words.length) {
          channelSend(onEvent, { type: "text", text: words[i++] });
          window.setTimeout(step, per * Math.max(1, words[i - 1].length / 4));
          return;
        }
        const tokens = Math.ceil(text.length / 4);
        const total = Date.now() - start;
        const message = { role: "assistant", content: text, provider: request.provider, model: request.model, stopReason: "stop", createdAt: nowS(),
          stats: { ttftMs: slow ? 420 : 40, totalMs: total, inputTokens: 12, outputTokens: tokens, tokensPerSec: (tokens - 1) / Math.max(0.25, (total - (slow ? 420 : 40)) / 1000), deltas: words.length, decodeMs: total } };
        channelEnd(onEvent);
        resolve({ message, error: null });
      };
      window.setTimeout(step, slow ? 420 : 40);
    });
  },
  chat_stop: () => null,
  server_get: () => structuredClone(server),
  server_save: ({ settings }: { settings: ServerView["settings"] }) => {
    server.settings = structuredClone(settings);
    return structuredClone(server);
  },
  server_start: () => structuredClone(server),
  server_stop: () => ({ ...structuredClone(server), running: false, url: null }),
  server_logs: () => structuredClone(logs),
  server_self_test: () => [],
  server_create_key: () => "sbx-demo-0000000000000000",
  server_revoke_key: () => structuredClone(server),
  ollama_list: () => ({
    endpoint: ollamaId, node: "gx10-0000", port: 11434, version: "0.12.6",
    models: [
      { name: "gpt-oss:20b", sizeBytes: 13_793_437_013, modifiedAt: nowS() - 12 * 86400, family: "gptoss", parameterSize: "20.9B", quantization: "MXFP4", loaded: { name: "gpt-oss:20b", sizeBytes: 14_600_000_000, vramBytes: 14_600_000_000, expiresAt: nowS() + 240, contextLength: 8192 } },
      { name: "qwen3:32b", sizeBytes: 20_201_253_588, modifiedAt: nowS() - 9 * 86400, family: "qwen3", parameterSize: "32.8B", quantization: "Q4_K_M", loaded: { name: "qwen3:32b", sizeBytes: 29_960_000_000, vramBytes: 29_960_000_000, expiresAt: nowS() + 240, contextLength: 8192 } },
      { name: "llama3.2:3b", sizeBytes: 2_019_393_189, modifiedAt: nowS() - 40 * 86400, family: "llama", parameterSize: "3.2B", quantization: "Q4_K_M", loaded: null },
      { name: "nomic-embed-text:latest", sizeBytes: 274_302_450, modifiedAt: nowS() - 40 * 86400, family: "nomic-bert", parameterSize: "137M", quantization: "F16", loaded: null },
    ],
  }),
  ollama_pull: () => "task-demo-pull",
  ollama_delete: () => null,
};
