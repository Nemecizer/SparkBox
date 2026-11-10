import { render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { exitSentence, groupLine, latestFailed, memberLine, onDeploymentsChanged, setDeploymentsForTest } from "../../lib/deployments";
import { endpointLabel, endpointView, isServing, reachLine, type Endpoint } from "../../lib/endpoints";
import { endpointCommands } from "../../lib/modelCommands";
import { powerImpact } from "../../lib/impact";
import type { ClusterDeployment } from "../../lib/types";
import { mockCalls, mockTauri } from "../../test/tauri";
import { WindowManagerProvider } from "../../wm/WindowManager";
import { EnvValue, envEntry } from "../InspectWindow";
import { imageKeepEnd } from "../Containers";
import { FailedDeploymentCard, IdleServersLine, decisiveLine, failedTitle } from "./DeploymentSummary";
import { EndpointCard, logTarget } from "./EndpointCard";
import { LoadedTab } from "./LoadedTab";

// The real cluster as found on Oct 29 (snapshot-tour-2): glm53 on all four nodes exited 1 on
// Oct 26 after 54 s; Ollama runs as a service on cell1 (nothing on disk) and cell3 (gemma4).
const DECISIVE =
  "ValueError: No valid attention backend found for cuda with AttentionSelectorConfig(head_size=512, dtype=torch.bfloat16, kv_cache_dtype=bfloat16, block_size=None, use_mla=True, has_sink=False, use_sparse=True, use_mm_prefix=False, use_per_head_quant_scales=False, attn_type=AttentionType.DECODER, has_sliding_window=False, use_non_causal=False, use_batch_invariant=False, use_kv_connector=False, use_adaptive_verification=False, use_pcp=False, use_dcp=False). Reasons: {TRITON_MLA: [sparse not supported], FLASHINFER_MLA_SPARSE_SM120: [kv_cache_dtype not supported, kv_cache_dtype not supported]}.";
const member = (node: string, id: string, rank: number, start: number, end: number) => ({
  node, container: "glm53", id, rank, role: rank === 0 ? "head" : "worker", state: "exited", exitCode: 1, oom: false, startedMs: start * 1000, finishedMs: end * 1000,
}) as ClusterDeployment["members"][number];

export const GLM53: ClusterDeployment = {
  id: "glm53",
  label: "GLM-5.3-Flash-NVFP4",
  served: "glm-5.3-flash",
  engine: "vllm",
  image: "vllm/vllm-openai:glm53-flash-arm64-cu130",
  model: "/models/GLM-5.3-Flash-NVFP4",
  nodes: ["cell1", "cell2", "cell3", "gx10-0000"],
  members: [
    member("cell1", "7da4d848a41a", 0, 1793053226, 1793053280),
    member("cell2", "2e35a6ef4126", 1, 1793053226, 1793053279),
    member("cell3", "015bcb21d2bd", 2, 1793053225, 1793053280),
    member("gx10-0000", "9eadeae9efaf", 3, 1793053224, 1793053279),
  ],
  state: "failed",
  inferred: true,
  how: "same image and command, ranks 0–3, started together",
  startedMs: 1793053224000,
  finishedMs: 1793053280000,
  ranS: 54,
  exitCode: 1,
  postmortem: {
    title: "No attention kernel for a bf16 KV cache on GB10",
    cause: "sparse-mla-kv-dtype",
    explanation: "GLM-5.3-Flash uses sparse MLA attention. On GB10 (compute 12.1) this vLLM build (0.28.1rc1) has one kernel for it, FlashInfer's SM120 sparse MLA, and that kernel doesn't accept a bfloat16 KV cache; Triton MLA has no sparse support.",
    fix: "Check the model's recipe for a compatible checkpoint, image and KV cache type. Changing the KV cache type alone may not fix this. SparkBox doesn't change your launch script.",
    lines: ["attention_cls = current_platform.get_attn_backend_cls(", "raise ValueError(", DECISIVE],
    logLine: `(Worker_TP0 pid=251) ERROR 10-26 22:21:14 [multiproc_executor.py:944] ${DECISIVE}`,
    scriptLine: "~/glm53/inner.sh line 22: --kv-cache-dtype bfloat16",
    readMs: 1793273000000,
    logNode: "cell1",
    logContainer: "glm53",
    vllm: "0.28.1rc1",
    tp: 4,
    kvDtype: "bf16",
    error: null,
  },
  demo: false,
};

function ollama(node: string, onDisk: string[], bytes: number[]): Endpoint {
  return {
    id: `${node}-ollama-11434`, engine: "ollama", models: onDisk, nodes: [node], headNode: node, port: 11434, url: "http://127.0.0.1:11434/v1",
    managedBy: "external", health: "idle", healthReason: "Nothing in memory", message: "Nothing in memory", containerIds: [], containerNames: [],
    listen: "local", reach: "node-only", label: `Ollama on ${node}`, onDisk, onDiskBytes: bytes, ollama: [], source: "discovered",
  };
}

export const AS_FOUND: Endpoint[] = [ollama("cell1", [], []), ollama("cell3", ["gemma4:latest"], [9_556_302_029])];

const NOW = 1793274235000; // Oct 29, the tour

function wm(children: React.ReactNode) {
  return render(<WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}>{children}</WindowManagerProvider>);
}

beforeEach(() => {
  mockTauri({ cluster_deployments_list: () => [GLM53], endpoints_list: () => AS_FOUND, deployments_list: () => [] });
  setDeploymentsForTest([GLM53]);
});
afterEach(() => setDeploymentsForTest(null));

describe("the failed GLM-5.3 deployment", () => {
  it("is one card: model, nodes, exits and time, the likely cause and the decisive line", () => {
    wm(<FailedDeploymentCard d={GLM53} />);
    const card = screen.getByRole("article");
    expect(within(card).getByText("GLM-5.3-Flash-NVFP4")).toBeInTheDocument();
    expect(failedTitle(GLM53)).toBe("GLM-5.3-Flash-NVFP4 · 4 nodes · Failed");
    // A neutral card: only the pill is red.
    expect(card).not.toHaveClass("ai-card-crit");
    expect(within(card).getByText("Failed")).toHaveClass("ai-failed-pill");
    // "54\u00a0s": the number never ends a line without its unit.
    expect(exitSentence(GLM53)).toBe("Exited 1 on cell1, cell2, cell3 and gx10-0000 after 54\u00a0s");
    expect(card).toHaveTextContent("Exited 1 on cell1, cell2, cell3 and gx10-0000 after 54 s · Oct 26 at");
    expect(card).toHaveTextContent("Likely cause: No attention kernel for a bf16 KV cache on GB10");
    // The exception and start of its message remain readable; the whole line is in the tooltip.
    expect(decisiveLine(GLM53)).toBe(DECISIVE);
    expect(card.querySelector(".ai-dep-line")).toHaveAttribute("title", DECISIVE);
    for (const label of ["Explain…", "Show log", "Containers"]) expect(within(card).getByRole("button", { name: label })).toBeInTheDocument();
  });

  it("counts as failed for a week, then it's history", () => {
    expect(latestFailed([GLM53], 7, NOW)?.id).toBe("glm53");
    expect(latestFailed([GLM53], 7, NOW + 6 * 86_400_000)).toBeNull();
  });

  it("has its log read once when it first arrives without a postmortem", async () => {
    // The backend's own round can announce a failed run before anything listed it: being shown, it's read.
    const bare = { ...GLM53, id: "glm53-event", postmortem: null };
    const lists = () => mockCalls.filter((c) => c.cmd === "cluster_deployments_list").length;
    const before = lists();
    onDeploymentsChanged([bare]);
    await Promise.resolve();
    expect(lists()).toBe(before + 1);
    onDeploymentsChanged([bare]);
    onDeploymentsChanged([{ ...bare, postmortem: GLM53.postmortem }]);
    expect(lists()).toBe(before + 1);
    // A new exit of the same run is a new postmortem.
    onDeploymentsChanged([{ ...bare, finishedMs: bare.finishedMs! + 3_600_000 }]);
    await Promise.resolve();
    expect(lists()).toBe(before + 2);
  });

  it("groups and names its members for Containers' Deployments view", () => {
    expect(groupLine(GLM53)).toBe("glm53 · 4 nodes · ranks 0–3 · Failed");
    expect(memberLine(GLM53.members[0])).toBe("rank 0 · cell1 · head");
    expect(memberLine(GLM53.members[3])).toBe("rank 3 · gx10-0000");
    expect(memberLine(GLM53.members[0], false)).toBe("rank 0 · head");
  });

  it("is a Failed row in Models › Loaded, next to the idle Ollamas", async () => {
    // Loaded keeps a failed run for a week of real time, so the clock sits on the tour's day.
    vi.useFakeTimers({ toFake: ["Date"], now: NOW });
    wm(<LoadedTab deployments={[]} refresh={() => {}} onLoad={() => {}} onBenchmark={() => {}} />);
    const row = (await screen.findByText("GLM-5.3-Flash-NVFP4")).closest("tr") as HTMLElement;
    vi.useRealTimers();
    expect(row).toHaveTextContent("vLLM");
    expect(row).toHaveTextContent("4 nodes");
    expect(row).toHaveTextContent("Failed");
    expect(row).toHaveTextContent("Exited 1 ·");
    expect(within(row).getByRole("button", { name: "Explain…" })).toBeInTheDocument();
    const cells = row.querySelectorAll("td");
    expect([...cells].some((td) => td.textContent === "—")).toBe(true);
  });
});

describe("Ollama at rest", () => {
  it("is idle, neutral and node-only, never Down, never serving", () => {
    const e = AS_FOUND[1];
    expect(endpointView(e)).toMatchObject({ word: "Idle", tone: "idle", reason: "Nothing in memory" });
    expect(isServing(e)).toBe(false);
    expect(isServing({ health: "paused" })).toBe(false);
    expect(isServing({ health: "gone" })).toBe(false);
    expect(isServing({ health: "ready" })).toBe(true);
    expect(reachLine(e)).toBe("Reachable from cell3 only (127.0.0.1:11434). Chat and the API server on this Mac can't use it.");
    wm(<EndpointCard endpoint={e} variant="full" />);
    const card = screen.getByRole("article");
    expect(within(card).getByText("Ollama on cell3")).toBeInTheDocument();
    expect(card).not.toHaveTextContent("Ollama ollama");
    expect(card).not.toHaveClass("ai-card-crit");
    expect(card).toHaveTextContent("Idle");
    expect(card).toHaveTextContent("gemma4:latest · 8.9 GiB");
    expect(card).toHaveTextContent("Reachable from cell3 only (127.0.0.1:11434)");
    // A service, not a container: its log is the unit's journal, and there's no container to stop.
    expect(logTarget(e)).toEqual({ target: { kind: "journal", unit: "ollama.service" }, what: "journalctl -u ollama.service" });
    expect(within(card).getByRole("button", { name: "Logs" })).toBeEnabled();
    expect(within(card).queryByRole("button", { name: "Stop containers…" })).toBeNull();
  });

  it("says which servers are idle in one quiet line", () => {
    wm(<IdleServersLine endpoints={AS_FOUND} />);
    expect(screen.getByRole("button")).toHaveTextContent("Ollama on cell1 and cell3 · idle, nothing in memory");
  });

  it("names every endpoint, never its id", () => {
    expect(endpointLabel({ engine: "vllm", models: ["glm-5.3-flash"], headNode: "cell1" })).toBe("glm-5.3-flash");
    expect(endpointLabel({ engine: "vllm", models: ["/models/GLM-5.3-Flash-NVFP4"], headNode: "cell1" })).toBe("GLM-5.3-Flash-NVFP4");
    expect(endpointLabel({ engine: "ollama", models: ["gemma4:latest"], headNode: "cell3" })).toBe("Ollama on cell3");
    expect(endpointLabel({ engine: "sglang", models: [], headNode: "gx10-0000" })).toBe("SGLang on gx10-0000");
  });

  it("no surface shows -ollama-11434 or -vllm-8000 for the as-found world", async () => {
    const view = wm(
      <>
        {AS_FOUND.map((e) => <EndpointCard key={e.id} endpoint={e} variant="full" />)}
        {AS_FOUND.map((e) => <EndpointCard key={`c-${e.id}`} endpoint={e} variant="compact" />)}
        <LoadedTab deployments={[]} refresh={() => {}} onLoad={() => {}} onBenchmark={() => {}} />
        <FailedDeploymentCard d={GLM53} />
        <IdleServersLine endpoints={AS_FOUND} />
      </>,
    );
    await screen.findAllByText("GLM-5.3-Flash-NVFP4");
    const text = view.container.textContent ?? "";
    expect(text).not.toMatch(/-ollama-11434|-vllm-8000/);
    const titles = [...view.container.querySelectorAll("[title], [aria-label]")].map((el) => `${el.getAttribute("title") ?? ""} ${el.getAttribute("aria-label") ?? ""}`).join("\n");
    expect(titles).not.toMatch(/-ollama-11434|-vllm-8000/);
    const palette = AS_FOUND.flatMap((e) => endpointCommands(e).map((c) => `${c.title} ${c.subtitle ?? ""}`)).join("\n");
    expect(palette).not.toMatch(/-ollama-11434|-vllm-8000/);
    expect(palette).toContain("Open endpoint Ollama on cell3");
  });
});

describe("planned restarts and recovery", () => {
  const llama: Endpoint = {
    id: "cell1-vllm-8000", engine: "vllm", models: ["llama-3.3-70b"], nodes: ["cell1", "cell2", "cell3"], headNode: "cell1", port: 8000, url: "http://192.168.50.51:8000/v1",
    managedBy: "external", health: "ready", containerIds: ["3f9a1c27b0de"], containerNames: ["vllm-head"], source: "discovered", label: "llama-3.3-70b",
  };
  const snap = { tsMs: NOW, demo: true, switches: [], nodes: ["cell1", "cell2", "cell3"].map((id) => ({ id, name: id, containers: [] })) } as never;

  it("offers to restart the pipeline in order once the node is back", () => {
    const r = powerImpact("cell3", "reboot", { snapshot: snap, endpoints: [llama, AS_FOUND[1]] });
    expect(r.paragraph).toContain("llama-3.3-70b runs across cell1, cell2 and cell3. It pauses while cell3 restarts (planned, so nobody is paged). With the follow-up below selected, SparkBox restarts it in order once the node is back.");
    expect(r.paragraph).toContain("Ollama on cell3 has nothing in memory; it comes back with cell3.");
    expect(r.followUp).toEqual({ label: "Then restart llama-3.3-70b in order once cell3 is back", endpointId: "cell1-vllm-8000" });
    // Keep running does it already; a shutdown has nothing to come back to yet.
    expect(powerImpact("cell3", "reboot", { snapshot: snap, endpoints: [{ ...llama, supervisor: { enabled: true, state: "watching", restarts: 0, nextRetryMs: null } }] }).followUp).toBeUndefined();
    expect(powerImpact("cell3", "poweroff", { snapshot: snap, endpoints: [llama] }).followUp).toBeUndefined();
    expect(powerImpact("cell2", "reboot", { snapshot: snap, endpoints: [] })).toEqual({ paragraph: "" });
  });

  it("reads Paused, neutral, with a wrench, while a node restarts on purpose", () => {
    const paused = { ...llama, health: "paused" as const, healthReason: "cell3 restarting (planned by SparkBox, 06:06)" };
    expect(endpointView(paused)).toMatchObject({ word: "Paused", tone: "paused", reason: "cell3 restarting (planned by SparkBox, 06:06)" });
    wm(<EndpointCard endpoint={paused} variant="full" />);
    const card = screen.getByRole("article");
    expect(card).toHaveClass("ai-card-paused");
    expect(card).not.toHaveClass("ai-card-crit");
    expect(card).toHaveTextContent("Paused");
    expect(card).toHaveTextContent("cell3 restarting (planned by SparkBox, 06:06)");
  });
});

describe("Inspect and Containers text", () => {
  it("shows MTP_TOKENS, hides API_KEY, and folds a 2,967-character value", () => {
    // What the backend sends after masking (ops/containers.rs mask_env).
    expect(envEntry("MTP_TOKENS=5")).toEqual({ name: "MTP_TOKENS", value: "5", masked: false });
    expect(envEntry("API_KEY=‹redacted›")).toMatchObject({ name: "API_KEY", masked: true });
    const long = "cuda>=13.0 " + "brand=tesla,driver>=535,driver<536 ".repeat(100);
    const text = long.slice(0, 2967);
    render(<EnvValue value={text} />);
    expect(screen.getByRole("button", { name: "Show all (2,967 characters)" })).toBeInTheDocument();
    render(<EnvValue value="5" />);
    expect(screen.getByText("5")).toBeInTheDocument();
  });

  it("keeps an image's whole tag and the start of its repo, with one ellipsis", () => {
    expect(imageKeepEnd("vllm/vllm-openai:glm53-flash-arm64-cu130")).toBe(":glm53-flash-arm64-cu130".length);
    expect(imageKeepEnd("nvcr.io/nvidia/cuda:13.0.1-devel-ubuntu24.04")).toBe(":13.0.1-devel-ubuntu24.04".length);
    expect(imageKeepEnd("ghcr.io/org/thing")).toBe("/thing".length);
  });
});
