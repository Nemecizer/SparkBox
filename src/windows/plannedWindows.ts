// Windows that are on the menu but not built yet. Each says what it will do,
// so a planned feature is explained rather than a dead link. The sidebar shows
// the launchers only when Settings › General › Show upcoming features is on;
// Alerts, Endpoints and Updates fall back here only until their windows exist.

export interface PlannedWindow {
  title: string;
  summary: string;
  points: string[];
  /** A built window that covers it now: the planned entry opens that instead. */
  redirect?: { kind: string; params?: Record<string, string> };
}

export const PLANNED: Record<string, PlannedWindow> = {
  "inference-endpoints": {
    title: "Endpoints",
    summary: "Every inference server on the cluster lives in Endpoints.",
    points: [],
    redirect: { kind: "endpoints" },
  },
  "launch-vllm": {
    title: "vLLM",
    summary: "vLLM deployment lives in Models › Load.",
    points: [],
    redirect: { kind: "models", params: { tab: "load" } },
  },
  "launch-sglang": {
    title: "SGLang",
    summary: "SGLang loads live in Models › Load, with SGLang picked as the engine.",
    points: [],
    redirect: { kind: "models", params: { tab: "load", engine: "sglang" } },
  },
  "launch-trtllm": {
    title: "TensorRT-LLM",
    summary: "Serve a model with TensorRT-LLM's trtllm-serve.",
    points: [
      "Single node, or multi-node through MPI across the selected Sparks",
      "Engine build step shown with live progress (it can take a while)",
      "Commands previewed per node before launch",
    ],
  },
  "launch-llamacpp": {
    title: "llama.cpp",
    summary: "Run llama-server for GGUF models.",
    points: [
      "Single node, or spread layers across nodes with llama.cpp's RPC backend (rpc-server on the helpers)",
      "Pick a GGUF from the Models list, context size, GPU layers and port",
    ],
  },
  "launch-ollama": {
    title: "Ollama",
    summary: "Ollama's models are in the Ollama window.",
    points: [],
    redirect: { kind: "ollama" },
  },
  "launch-nim": {
    title: "NIM",
    summary: "Run NVIDIA NIM microservice containers.",
    points: [
      "NGC API key kept in the Keychain, never on disk",
      "Pick a NIM image and node; SparkBox handles docker login and the cache volume",
      "Single node first; multi-node only for NIMs that support it",
    ],
  },
  updates: {
    title: "Updates",
    summary: "OS and package updates, one node at a time.",
    points: [
      "Pending apt and DGX OS updates per node",
      "Rolling update: stop workloads, upgrade, reboot, wait for the node to return, then move on",
    ],
  },
  alerts: {
    title: "Alerts",
    summary: "Thresholds and macOS notifications. (The log of every command SparkBox runs is already in Activity log.)",
    points: [
      "Memory pressure warning before a Spark runs out of unified memory and freezes, with a one-click “stop the biggest container”",
      "Disk filling up, GPU too hot, node unreachable, container exited unexpectedly",
    ],
  },
};
