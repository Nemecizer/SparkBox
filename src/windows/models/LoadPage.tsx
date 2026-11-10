// Loading a model with vLLM (or SGLang), with the guardrails LM Studio taught
// everyone to expect: a free port, a per-node memory plan that says where each
// Spark ends up once the engine has reserved its share (red at 95%, amber over
// the 90% alert line), a share sized to the model until someone picks one, an API key that never
// touches a command line, and remote code only on explicit opt-in. A vLLM load, on one
// node or several, takes the recipe for its folder and a split that model can take, picks
// its image from what's on the chosen nodes and shows "Before it starts" for the set: one
// verdict, then each node's memory, earlyoom, the image, the folder, the ports, /dev/shm,
// the cache folders, its fabric and what an earlier load left, with the page-cache drop
// offered only where vLLM would refuse to start without it. The same checks run again in
// Rust when you press Load. In Edit & reload the load being replaced counts as gone: its
// port and its share come back at the unload, and Rust reads the nodes again after it.
import { useEffect, useMemo, useRef, useState } from "react";
import { RecipeImport } from "./RecipeImport";
import type { ImportedRecipe } from "../../lib/modelsApi";
import { ArrowLeft, Lock, ShieldAlert, Sparkles } from "lucide-react";
import { Icon } from "../../components/Icon";
import { VersionComparison } from "../../components/VersionComparison";
import { Badge, CodeBlock, FormGroup, FormRow, HelpTip, InfoTip, Notice, Segmented, Spacer, StatusDot, Switch, Toolbar, ToolButton } from "../../components/ui";
import { api } from "../../lib/api";
import { ADMIN_REFUSED, isLookOnly, lockOf, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, requestAdmin, useAdminState, withAdmin } from "../../lib/admin";
import { fmtBytes } from "../../lib/format";
import { finishedCopy, finishedNodes, inNames, modelsApi, verifiedWords, type DeployPlan, type DeploySpec, type Deployment, type LibraryEntry, type Listen, type LoadEngine, type NodePlan, type Preflight, type PreflightSet, type RecipeView, type TopologyOffer } from "../../lib/modelsApi";
import { opsApi, type NodeImages } from "../../lib/opsApi";
import { confirmAction } from "../../lib/session";
import { waitTask } from "../../lib/tasks";
import { toast } from "../../lib/toasts";
import { endpointLabel, isServing, useEndpoints } from "../../lib/endpoints";
import { useCluster } from "../../lib/store";
import { openWindow } from "../../wm/WindowManager";
import type { FabricIf, ModelSettings, PreviewStep } from "../../lib/types";
import type { Draft } from "./HubTab";
import { fabricState } from "./PrepareTab";
import { Ago, errText, fmtTokens, type NodeRef } from "./shared";
import { openHelp } from "../../help/nav";
import { checkTransferDisk, DiskPreflight } from "./DiskPreflight";

/** vLLM options SparkBox sets from the fields (src-tauri/src/models/plan.rs MANAGED_FLAGS), with whether they take a value. */
const MANAGED_FLAGS: Record<string, boolean> = {
  "--model": true, "--served-model-name": true, "--host": true, "--port": true, "--tensor-parallel-size": true, "-tp": true,
  "--pipeline-parallel-size": true, "-pp": true, "--distributed-executor-backend": true, "--enable-expert-parallel": false,
  "--max-model-len": true, "--gpu-memory-utilization": true, "--max-num-seqs": true, "--kv-cache-dtype": true,
  "--enable-prefix-caching": false, "--no-enable-prefix-caching": false, "--tool-call-parser": true, "--enable-auto-tool-choice": false,
  "--reasoning-parser": true, "--trust-remote-code": false, "--api-key": true, "--nnodes": true, "--node-rank": true, "--master-addr": true, "--master-port": true, "--headless": false,
};

const TOOL_PARSERS = ["hermes", "llama3_json", "llama4_pythonic", "mistral", "qwen3_coder", "glm45", "deepseek_v3", "openai", "kimi_k2", "granite", "pythonic"];
const REASONING_PARSERS = ["qwen3", "deepseek_r1", "glm45", "mistral", "granite"];

/** The tool-call parser vLLM uses for a model family, from its repo name. */
export function toolParserFor(repo: string): string {
  const r = repo.toLowerCase();
  if (r.includes("qwen") && r.includes("coder")) return "qwen3_coder";
  if (r.includes("qwen")) return "hermes";
  if (r.includes("llama-4") || r.includes("llama4")) return "llama4_pythonic";
  if (r.includes("llama")) return "llama3_json";
  if (r.includes("glm")) return "glm45";
  if (r.includes("deepseek")) return "deepseek_v3";
  if (r.includes("gpt-oss")) return "openai";
  if (r.includes("kimi")) return "kimi_k2";
  if (r.includes("mistral") || r.includes("devstral") || r.includes("magistral")) return "mistral";
  if (r.includes("granite")) return "granite";
  return "hermes";
}

/** The reasoning parser for families that think out loud, or null. */
export function reasoningParserFor(repo: string): string | null {
  const r = repo.toLowerCase();
  if (r.includes("deepseek-r1") || r.includes("qwq")) return "deepseek_r1";
  if (r.includes("qwen3") && !r.includes("instruct-2507") && !r.includes("coder")) return "qwen3";
  if (r.includes("glm-4.5") || r.includes("glm-5")) return "glm45";
  if (r.includes("magistral")) return "mistral";
  return null;
}

/**
 * The Expert field as argv: one option per line, and the first space splits the flag from its
 * value, so the value stays one word whatever's in it (JSON with spaces, quotes). No shell reads it.
 */
export function parseExpertArgs(text: string): string[] {
  return text.split("\n").map((l) => l.trim()).filter(Boolean).flatMap((l) => {
    const at = l.indexOf(" ");
    return at < 0 ? [l] : [l.slice(0, at), l.slice(at + 1).trim()];
  });
}

/** The Expert field for saved args: a new line at each flag, and one for a flag's second value. */
export function expertText(args: string[]): string {
  const lines: string[][] = [];
  for (const a of args) {
    const last = lines[lines.length - 1];
    if (last?.length === 1 && !/^--?[a-zA-Z][^ ]*$/.test(a)) last.push(a);
    else lines.push([a]);
  }
  return lines.map((l) => l.join(" ")).join("\n");
}

/** Rust's sentence, when it's about the Expert field: it says "option", or names the flag with `_` read as `-`. */
export function expertRefusal(problem: string | null, text: string): string | null {
  if (!problem || !text.trim()) return null;
  const firsts = text.split("\n").map((l) => l.trim().split(/[ =]/)[0]).filter(Boolean);
  return /\boptions?\b/.test(problem) || firsts.some((a) => problem.includes(a.replace(/_/g, "-"))) ? problem : null;
}

/** The AI's extra options minus the ones SparkBox sets from fields, for the Expert field. */
function extraFromAdvice(groups: string[][]): string {
  return groups.filter((g) => !(g[0] in MANAGED_FLAGS)).map(expertText).join("\n");
}

function defaultName(repo: string) {
  return (repo.split("/").pop() ?? repo).toLowerCase();
}

const LEVEL_TONE = { green: "neutral", amber: "warn", red: "crit" } as const;

/** The image a vLLM load starts when the nodes have no better one (plan.rs DEFAULT_IMAGE). */
export const DEFAULT_IMAGE = "vllm/vllm-openai:glm53-flash-arm64-cu130";
/** The share a load stays at when the plan can't size it to the model (fit.rs FIRST_LOAD_SHARE). */
export const FIRST_LOAD_SHARE = 0.75;
/** The least share a load can take, and that the plan sizes one to (fit.rs MIN_SHARE). */
export const MIN_SHARE = 0.2;
/** What to try when an image answers garbage (readiness.rs FALLBACK_IMAGE): NVIDIA's own vLLM. */
const FALLBACK_IMAGE = "nvcr.io/nvidia/vllm:26.05-py3";

/** An image the Load page offers for the chosen nodes: its name, and on each its short id (null: none there) and whether that node's listing was read. */
export interface ImageChoice {
  ref: string;
  on: { node: string; id: string | null; read: boolean }[];
}

/**
 * The images to offer on `nodes`: the saved one, SparkBox's default, the fallback for an image
 * that answers garbage, then each vLLM image those nodes have.
 */
export function imageChoices(images: NodeImages[] | null, nodes: string[], saved: string | undefined): ImageChoice[] {
  const of = (node: string) => (images ?? []).find((n) => n.node === node)?.images.filter((i) => !i.dangling) ?? [];
  const idOn = (node: string, ref: string) => of(node).find((i) => `${i.repository}:${i.tag}` === ref)?.id ?? null;
  // A listing that failed, or hasn't come back, says nothing about what's on the node.
  const read = (node: string) => !!images?.some((n) => n.node === node && !n.error);
  const seen = nodes.flatMap((n) => of(n).filter((i) => /vllm/i.test(i.repository)).map((i) => `${i.repository}:${i.tag}`));
  const refs = [saved, DEFAULT_IMAGE, FALLBACK_IMAGE, ...seen].filter((r): r is string => !!r);
  return [...new Set(refs)].map((ref) => ({ ref, on: nodes.map((node) => ({ node, id: idOn(node, ref), read: read(node) })) }));
}

/**
 * The image a load starts unless another is picked: the saved one if every chosen node has it,
 * else SparkBox's default if they do (docker lists newest first, so a nightly would come before
 * it), else the first vLLM image they all have, else SparkBox's default.
 */
export function defaultImage(choices: ImageChoice[], saved: string | undefined): string {
  const onAll = (c: ImageChoice) => c.on.length > 0 && c.on.every((o) => o.id);
  const there = (ref: string | undefined) => choices.find((c) => c.ref === ref && onAll(c));
  return there(saved)?.ref ?? there(DEFAULT_IMAGE)?.ref ?? choices.find(onAll)?.ref ?? DEFAULT_IMAGE;
}

/** "on cell1, cell2 · sha256:5a0f8b91…; not on cell3; not read on cell4": one part per id, then the nodes without it, then those whose listing failed. */
export function imagePresence(c: ImageChoice, nameOf: (node: string) => string = (n) => n): string {
  const byId = new Map<string, string[]>();
  for (const o of c.on) if (o.id) byId.set(o.id, [...(byId.get(o.id) ?? []), nameOf(o.node)]);
  const parts = [...byId].map(([id, names]) => `on ${names.join(", ")} · sha256:${id.replace(/^sha256:/, "").slice(0, 8)}…`);
  const absent = c.on.filter((o) => !o.id && o.read).map((o) => nameOf(o.node));
  if (absent.length) parts.push(`not on ${absent.join(", ")}`);
  const unread = c.on.filter((o) => !o.id && !o.read).map((o) => nameOf(o.node));
  if (unread.length) parts.push(`not read on ${unread.join(", ")}`);
  return parts.join("; ");
}

/** "121.7 GiB": the pre-flight's figures, as the refusals word them. */
function gib(bytes: number): string {
  return `${(bytes / 2 ** 30).toFixed(1)} GiB`;
}

/** "cell1, cell2 and cell3". */
function listOf(names: string[]): string {
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** Rust takes these as whole numbers, or inside this range: past here serde or a sum would answer for them. */
const LIMITS = { maxContext: 1_000_000_000, requests: 10_000, split: 1024 };

/** What's wrong with a whole-number field, as a sentence. Empty is fine: the field is optional. */
export function wholeProblem(label: string, text: string, min: number, max: number): string | null {
  const t = text.trim();
  const n = Number(t);
  // Ports and the like read as they're typed; a billion needs its commas.
  const word = (x: number) => (x < 100_000 ? String(x) : x.toLocaleString());
  return !t || (Number.isInteger(n) && n >= min && n <= max) ? null : `${label} must be a whole number from ${word(min)} to ${word(max)}.`;
}

/** A part of the split from its number field: what's typed, as a whole number the sum in Rust can take. */
export function splitPart(text: string): number {
  return Math.min(LIMITS.split, Math.max(1, Math.trunc(Number(text)) || 1));
}

/** The verdict line for the node set. */
export function verdictLine(set: PreflightSet, nameOf: (node: string) => string = (n) => n): string {
  const on = listOf(set.nodes.map((p) => nameOf(p.node)));
  if (set.verdict === "ready") return `Ready to load on ${on}.`;
  const n = set.problems.length;
  return n ? `Not ready to load on ${on}: ${n} ${n === 1 ? "problem" : "problems"} to fix first.` : `Not ready to load on ${on}.`;
}

/** One ConnectX-7 half: "enp1s0f1np1 192.168.100.51 · 200 Gb/s · MTU 9000 · RDMA ACTIVE". */
export function fabricWords(f: FabricIf): string {
  return `${f.iface} ${f.ip} · ${f.speedMbps >= 1000 ? `${f.speedMbps / 1000} Gb/s` : "link down"} · ${fabricState(f)}`;
}

/** Rust's answer for a node it couldn't read has no memory and no cache folders: nothing else in it is a reading. */
export function wasRead(p: Pick<Preflight, "mem" | "caches">): boolean {
  return !!p.mem || p.caches.length > 0;
}

/** One chosen node's block in "Before it starts". */
function NodeChecks({ p, rank, count, share, port, nameOf, lock, onClean }: {
  p: Preflight;
  rank: number;
  count: number;
  share: number;
  port: number;
  nameOf: (node: string) => string;
  lock: ReturnType<typeof lockOf>;
  onClean: (p: Preflight) => void;
}) {
  const name = nameOf(p.node);
  const m = p.mem;
  const eo = p.earlyoom;
  // Edit & reload: the figures count the replaced load's share as coming back.
  const once = p.replacing ? ` once ${p.replacing} is unloaded` : "";
  const mine = p.leftovers.filter((l) => l.ours);
  // Every rank needs the rendezvous port; only the head binds the API's.
  const ports = count === 1 ? String(port) : rank === 0 ? `${port} and 29501` : "29501";
  const title = <div className="strong">{name}{count > 1 && <span className="muted"> · {rank === 0 ? "head, serves the API" : `worker, rank ${rank}`}</span>}</div>;
  if (p.cutOff || !wasRead(p)) {
    return <div className="col gap-sm ai-step">{title}<span className="error-text">Couldn't read {name}</span></div>;
  }
  return (
    <div className="col gap-sm ai-step">
      {title}
      <FormRow label="Memory" hint={`${name}'s /proc/meminfo`}>
        <span className="num">{m ? `${gib(m.total)} in all · ${gib(m.free)} free · ${gib(m.available)} available · ${gib(m.cached)} cached` : "Couldn't read it"}</span>
      </FormRow>
      <FormRow label={<>What vLLM needs <HelpTip topic="models.load.what-vllm-needs" /></>} hint="Its startup check counts the page cache as used, so this much must be free">
        <span>{`${gib(p.needFree)} free at share ${share.toFixed(2)} (4 GiB of it headroom)`}{m ? (p.dropNeeded ? ` · ${name} has ${gib(m.free + p.freed)} free${once}` : ` · enough is free${once}`) : ""}</span>
      </FormRow>
      <FormRow label={<>earlyoom <HelpTip topic="models.load.earlyoom" /></>} hint="Shown, never blocking">
        <span className="mono">{eo ? `${eo.active ? "active" : eo.state}${eo.config ? ` · ${eo.config}` : " · no EARLYOOM_ARGS"}` : "Couldn't read it"}</span>
      </FormRow>
      <FormRow label="Image" hint={p.store ? `Docker image store: ${p.store}` : undefined}>
        <span className="mono">{p.imageId ? `${p.imageId.slice(0, 15)}…` : `not on ${name}`}</span>
      </FormRow>
      <FormRow label="Model folder"><span>{p.modelPresent ? `present on ${name}` : `not on ${name}`}</span></FormRow>
      <FormRow label={count > 1 && rank === 0 ? "Ports" : "Port"}><span>{p.portFree ? `${ports} free on ${name}` : count > 1 ? `Taken on ${name} (${p.portOwner})` : `${port} taken on ${name} (${p.portOwner})`}</span></FormRow>
      <FormRow label={<>/dev/shm <HelpTip topic="models.load.dev-shm" /></>}><span className="mono">{p.shm.length ? p.shm.join(", ") : "empty"}</span></FormRow>
      <FormRow label={<>Cache folders <HelpTip topic="models.load.cache-folders" /></>} hint="Mounted into the container; a missing one is made at the start">
        <span className="mono">{p.caches.map((c) => `${c.path} ${c.exists ? `(${c.owner ?? "?"})` : "(missing)"}`).join(" · ")}</span>
      </FormRow>
      <FormRow label="Fabric" hint="What the node reports. The path between nodes isn't tested yet">
        {p.fabric.length ? <span className="mono">{p.fabric.map(fabricWords).join(" · ")}</span> : <span className="error-text">no address</span>}
      </FormRow>
      <FormRow label="Ollama"><span>{p.ollamaResident.length ? `${p.ollamaResident.join(", ")} in memory on ${name}` : `nothing in memory on ${name}`}</span></FormRow>
      <FormRow label={<>Left by an earlier load <HelpTip topic="clear-what-an-earlier-load-left-behind" /></>} hint={p.leftovers.some((l) => !l.ours) ? "Another build's containers are shown, never removed" : undefined}>
        <span className="ai-inline">
          <span className="mono">{p.leftovers.length ? p.leftovers.map((l) => `${l.name} (${l.status})`).join(", ") : "nothing"}</span>
          {mine.length > 0 && (
            <button type="button" className="btn small" disabled={lock === "lookOnly"} onClick={() => onClean(p)}
              title={lock === "lookOnly" ? LOOK_ONLY_TIP : lock ? `Remove ${mine.map((l) => l.name).join(", ")} · ${NEEDS_ADMIN_TIP}` : `Remove ${mine.map((l) => l.name).join(", ")}`}>
              Remove leftovers…{lock && <Icon icon={Lock} size={12} className="tool-lock" />}
            </button>
          )}
        </span>
      </FormRow>
      {p.downloads.length > 0 && (
        <FormRow label="Download running" hint="A job, not a leftover. It goes on beside the load; stop it in Jobs">
          <span className="mono">{p.downloads.map((l) => `${l.name} (${l.status})`).join(", ")}</span>
        </FormRow>
      )}
    </div>
  );
}

/**
 * "Before it starts" for the chosen nodes: what model_preflight read, as one verdict and then one
 * block per node (Demo: nothing is read). The jumbo-frame path is only tested when Load is pressed,
 * so the page says that rather than showing a result it doesn't have.
 */
export function BeforeItStarts({ set, problem, share, port, drop, onDrop, nameOf = (n) => n, lock = null, onClean = () => {}, onRecheck }: {
  set: PreflightSet | null;
  problem: string | null;
  share: number;
  port: number;
  drop: boolean;
  onDrop: (on: boolean) => void;
  nameOf?: (node: string) => string;
  lock?: ReturnType<typeof lockOf>;
  onClean?: (p: Preflight) => void;
  onRecheck?: () => void;
}) {
  if (!set) {
    if (!problem) return <div className="muted">Reading the nodes…</div>;
    return <Notice tone="error" action={onRecheck ? { label: "Check again", onClick: onRecheck } : undefined}>{problem}</Notice>;
  }
  if (set.verdict === "demo") return <>
    <div className="muted">Available with Live data: SparkBox reads each node's memory, earlyoom, the image, the folder, the ports, /dev/shm, the cache folders, the fabric and what an earlier load left before a load.</div>
    {set.nodes.length > 1 && <VersionComparison report={set.versions} nodes={set.nodes.map((p) => ({ id: p.node, name: nameOf(p.node) }))} load demo />}
  </>;
  const short = set.nodes.filter((p) => p.dropNeeded);
  const several = set.nodes.length > 1;
  // Set-wide problems first; a node's own only when the set's list doesn't carry it.
  const problems = [...new Set([...set.problems, ...set.nodes.flatMap((p) => p.problems)])];
  return (
    <>
      {problem && <Notice tone="error">{problem}</Notice>}
      <FormRow label={<span className={set.verdict === "ready" ? "strong" : "error-text"} role="status">{verdictLine(set, nameOf)}</span>} hint={<>Checked <Ago ts={set.checkedAtMs / 1000} /></>}>
        {onRecheck && <button type="button" className="btn small" onClick={onRecheck}>Check again</button>}
      </FormRow>
      {problems.map((x) => <Notice key={x} tone="error">{x}</Notice>)}
      {several && <VersionComparison report={set.versions} nodes={set.nodes.map((p) => ({ id: p.node, name: nameOf(p.node) }))} load />}
      {short.length > 0 && (
        <FormRow label={<>Page cache <HelpTip topic="models.load.drop-page-cache" /></>} hint="Without it vLLM would refuse to start">
          <label className="check">
            <input type="checkbox" checked={drop} onChange={(e) => onDrop(e.target.checked)} />
            Drop the page cache first on {listOf(short.map((p) => nameOf(p.node)))} (sudo; the kernel refills it)
          </label>
        </FormRow>
      )}
      {several && <FormRow label="Fabric path" hint="NCCL_IB_HCA lists both halves, so a half that fails stops the load"><span>Tested when you press Load: each worker sends jumbo frames to the head on each half.</span></FormRow>}
      {set.nodes.map((p, i) => <NodeChecks key={p.node} p={p} rank={i} count={set.nodes.length} share={share} port={port} nameOf={nameOf} lock={lock} onClean={onClean} />)}
    </>
  );
}

/** "Needs 13.3 GiB; this share reserves 30.4 GiB, and the KV cache gets the rest, about 5 full-length requests." */
export function shareWords(p: Pick<NodePlan, "need" | "budget" | "kv">, requests: number | null): string {
  const holds = requests === null ? "" : `, about ${requests} full-length ${requests === 1 ? "request" : "requests"}`;
  return `Needs ${p.kv > 0 ? "" : "at least "}${fmtBytes(p.need)}; this share reserves ${fmtBytes(p.budget)}, and the KV cache gets the rest${holds}.`;
}

/**
 * Where a node ends up once the engine has reserved its share: what's in use already (gray),
 * then the share split into weights, runtime and the KV cache the rest becomes. Red marks only
 * what lies beyond the node's memory; the dashed line is the 90% alert line.
 */
function PlanBar({ p, name, engine = "vLLM", requests }: { p: NodePlan; name: string; engine?: string; requests: number | null }) {
  const kvRoom = Math.max(0, p.budget - p.weights - p.overhead);
  const kv = p.need > p.budget ? p.kv : kvRoom;
  const host = p.hostRuntime ?? 0;
  const end = p.inUse + host + p.weights + p.overhead + kv;
  const scale = Math.max(p.total, end, 1);
  const pct = (b: number) => `${Math.max(0, (100 * b) / scale)}%`;
  const others = p.holders.map((h) => `${h.name} ${fmtBytes(h.bytes)}`).join(", ");
  return (
    <div className={`ai-plan ai-plan-${p.level}`}>
      <div className="ai-plan-head">
        <span className="strong">{name}</span>
        <Badge tone={LEVEL_TONE[p.level]}>{p.verdict}</Badge>
        <span className="muted num">
          {p.need > p.budget ? `needs ${fmtBytes(p.need)}, more than its ${fmtBytes(p.budget)} share` : `${Math.round(p.afterPct)}% of its memory once loaded`}
        </span>
      </div>
      <div className="ai-plan-bar" role="img"
        aria-label={`${name}: in use ${fmtBytes(p.inUse)}, ${engine} host runtime ${fmtBytes(host)}, weights ${fmtBytes(p.weights)}, runtime ${fmtBytes(p.overhead)}, KV cache ${fmtBytes(kv)}, ${Math.round(p.afterPct)}% of ${fmtBytes(p.total)}`}>
        <span className="seg seg-others" style={{ width: pct(p.inUse) }} title={`In use already: ${others || "the OS and its page cache"} (what the node reports as used)`} />
        {host > 0 && <span className="seg seg-host" style={{ width: pct(host) }} title={`${engine} host runtime: its own process outside the share (Python, CUDA libraries), ${fmtBytes(host)}`} />}
        <span className="seg seg-weights" style={{ width: pct(p.weights) }} title={`Weights ${fmtBytes(p.weights)}`} />
        <span className="seg seg-overhead" style={{ width: pct(p.overhead) }} title={`Runtime and CUDA graphs${p.overhead > 7e9 ? ", vision encoder" : ""}: ${fmtBytes(p.overhead)}`} />
        <span className="seg seg-kv" style={{ width: pct(kv) }} title={p.need > p.budget ? `KV cache it needs: ${fmtBytes(kv)}` : `KV cache: everything left in the share, ${fmtBytes(kv)}`} />
        {end > p.total && <span className="seg-over" style={{ left: pct(p.total), width: pct(end - p.total) }} title={`${fmtBytes(end - p.total)} more than ${name} has`} />}
        <span className="ai-plan-line" style={{ left: pct(0.9 * p.total) }} title="The 90% memory alert line" />
        {scale > p.total && <span className="ai-plan-total" style={{ left: pct(p.total) }} title={`All of ${name}'s memory: ${fmtBytes(p.total)}`} />}
      </div>
      <div className="ai-plan-legend muted">
        <span><i className="sw seg-others" />in use {fmtBytes(p.inUse)}</span>
        {host > 0 && <span><i className="sw seg-host" />{engine} host runtime {fmtBytes(host)}</span>}
        <span><i className="sw seg-weights" />weights {fmtBytes(p.weights)}</span>
        <span><i className="sw seg-overhead" />runtime {fmtBytes(p.overhead)}</span>
        <span><i className="sw seg-kv" />KV cache {fmtBytes(kv)}</span>
        <span><i className="sw seg-free" />free</span>
        <span><i className="sw line" />90% alert line</span>
        <span>{fmtBytes(p.total)} in all</span>
      </div>
      {p.need <= p.budget && <div className="muted small">{shareWords(p, requests)}</div>}
      {p.reason && <div className={p.level === "red" ? "error-text" : "warn-text"}>{p.reason}</div>}
    </div>
  );
}

type AiField = "split" | "maxLen" | "gpuMem" | "extra" | "seqs" | "kv" | "prefix" | "tools" | "reasoning";
const ALL_FIELDS: AiField[] = ["split", "maxLen", "gpuMem", "extra", "seqs", "kv", "prefix", "tools", "reasoning"];

/** The recipe that has no opinion on a model (recipes/generic.json). */
const GENERIC_RECIPE = "generic";

/** What a recipe's defaults set on the page: only the fields nobody chose yet. The generic recipe leaves the model family's parsers. */
export function recipeFill(r: RecipeView, chosen: ReadonlySet<string>): { gpuMem?: string; maxLen?: string; seqs?: string; tools?: boolean; toolParser?: string; reasoning?: string } {
  const fill: ReturnType<typeof recipeFill> = {};
  if (r.defaults.share != null && !chosen.has("gpuMem")) fill.gpuMem = String(r.defaults.share);
  if (r.defaults.context != null && !chosen.has("maxLen")) fill.maxLen = String(r.defaults.context);
  if (r.defaults.seqs != null && !chosen.has("seqs")) fill.seqs = String(r.defaults.seqs);
  if (r.id === GENERIC_RECIPE) return fill;
  if (!chosen.has("tools")) {
    fill.tools = !!r.toolParser;
    if (r.toolParser) fill.toolParser = r.toolParser;
  }
  if (!chosen.has("reasoning")) fill.reasoning = r.reasoningParser ?? "";
  return fill;
}

/** The first split offered for this many nodes that the model can take. A plan for another node count offers nothing. */
export function firstOffer(offers: TopologyOffer[], nodes: number): TopologyOffer | null {
  return offers.find((o) => o.nodes === nodes && o.ok) ?? null;
}

/** "2 × 1: tensor parallel 2", as the Split menu lists an offer. `held`: the recipe can't load at all, so no split is the reason. */
export function offerWords(o: TopologyOffer, held = false): string {
  const how = o.tp > 1 && o.pp > 1 ? "tensor and pipeline parallel" : o.pp > 1 ? `pipeline parallel ${o.pp}` : o.tp > 1 ? `tensor parallel ${o.tp}` : "one node";
  const word = o.ok || held ? "" : o.unlisted ? " (not in this recipe)" : " (won't work)";
  return `${(o.replicas ?? 1) > 1 ? `${o.replicas} independent replicas · ` : ""}${o.tp} × ${o.pp}: ${how}${o.advanced ? ", advanced" : ""}${word}`;
}

interface LoadPageProps {
  draft: Draft;
  library: LibraryEntry[];
  nodes: NodeRef[];
  deployments: Deployment[];
  settings: ModelSettings | null;
  onClose: () => void;
  onStarted: () => void;
  onShared: () => void;
}

/** What makes a draft a different page: another folder, load, engine or advice starts over, so a saved load's values always open as saved. */
export function draftKey(d: Draft): string {
  return JSON.stringify([d.key, d.replaces ?? null, d.spec?.id ?? null, d.engine ?? null, d.repo ?? null, d.rec ? [d.rec.gpuMemoryUtilization, d.rec.maxModelLen, d.rec.nodes] : null, d.allowed ?? null]);
}

export function LoadPage(props: LoadPageProps) {
  return <LoadPageFor key={draftKey(props.draft)} {...props} />;
}

/**
 * What the sized share depends on: the engine, the folder, the nodes and the split, the context,
 * the requests at once, the KV cache type and the recipe. A plan is for one of these; a plan for
 * another says nothing about this one's share.
 */
function sizeKeyOf(engine: LoadEngine, key: string, ordered: string[], tp: number, pp: number, maxLen: string, seqs: string, kvDtype: string, recipePick: string | null): string {
  return JSON.stringify([engine, key, ordered, tp, pp, maxLen.trim(), seqs.trim(), kvDtype, recipePick]);
}

/** A plan as the page keeps it: with the identity it was asked about and the share it was asked at. */
interface Planned {
  plan: DeployPlan;
  key: string;
  share: number;
}

function LoadPageFor({ draft, library, nodes, deployments, settings, onClose, onStarted, onShared }: LoadPageProps) {
  const adminState = useAdminState();
  const admin = adminState.unlocked;
  const endpoints = useEndpoints();
  const snap = useCluster();
  // The load being edited doesn't count as busy: it's replaced.
  const busyNodes = new Set(deployments.filter((d) => (d.state === "starting" || d.state === "ready") && d.spec.id !== draft.replaces).flatMap((d) => d.spec.nodes));
  /** What already serves on each node (any engine, SparkBox's or not). */
  const servingOn = (id: string) => endpoints.filter((e) => e.health !== "down" && isServing(e) && e.nodes.includes(id)).map((e) => `${endpointLabel(e)}${e.managedBy === "external" && e.engine !== "ollama" ? " (external)" : ""}`);
  /** A node whose GPU failed can't take a load: its reason ("GPU fault: Xid 79, fell off the bus"). */
  const faultOf = (id: string) => {
    const f = snap?.nodes.find((n) => n.id === id)?.gpuFault;
    return f ? `GPU fault: ${f.code != null ? `Xid ${f.code}, ` : ""}${f.text}` : null;
  };
  const freeOf = (id: string) => snap?.nodes.find((n) => n.id === id)?.mem.available ?? 0;
  /** A node's name as the owner set it. Ids go to Rust; nothing is worked out from a name. */
  const nameOf = (id: string) => nodes.find((n) => n.id === id)?.name ?? id;
  const namesOf = (ids: string[]) => ids.map(nameOf).join(", ");
  /** Rust's sentences say a node's id; the page says its name. */
  const named = (text: string) => inNames(text, nodes);
  const rec = draft.rec;
  const was = draft.spec;
  const advice = useMemo(() => new Map((draft.allowed ?? []).map((g) => [g[0], g[1] ?? ""])), [draft.allowed]);

  // Nodes with a copy and nothing serving first, the most memory free first, then the rest.
  const nodesFor = (e: LibraryEntry | undefined, want: number) => {
    const score = (n: NodeRef) => (e && finishedCopy(e, n.id) ? 0 : 4) + (busyNodes.has(n.id) ? 2 : 0) + (servingOn(n.id).length ? 1 : 0);
    return [...nodes].sort((a, b) => score(a) - score(b) || freeOf(b.id) - freeOf(a.id)).slice(0, want).map((n) => n.id);
  };
  // Opens on what was asked for; otherwise on a model that fits the freest node holding a copy
  // (its weights plus room for the runtime and a KV cache inside vLLM's 0.8 share), or on nothing:
  // never on a red plan.
  const [first] = useState(() => {
    if (draft.key) return { key: draft.key, nodes: was?.nodes ?? nodesFor(library.find((e) => e.key === draft.key), rec?.nodes ?? 1) };
    for (const e of [...library].filter((x) => !x.inUse).sort((a, b) => a.sizeBytes - b.sizeBytes)) {
      const node = finishedNodes(e).filter((n) => !busyNodes.has(n) && !servingOn(n).length).sort((a, b) => freeOf(b) - freeOf(a))[0];
      if (node && e.sizeBytes * 1.2 + 12e9 <= freeOf(node) * 0.8) return { key: e.key, nodes: [node] };
    }
    return { key: "", nodes: [] as string[] };
  });
  const [key, setKey] = useState(first.key);
  const entry = library.find((e) => e.key === key);
  const repo = entry?.repo ?? draft.repo ?? key;
  // A folder with no finished copy can't load: it's listed only when it's the one asked for, to say why.
  const folders = library.filter((e) => finishedNodes(e).length > 0 || e.key === draft.key);
  const unfinished = entry && Object.keys(entry.copies).length > 0 && !finishedNodes(entry).length
    ? Object.values(entry.copies).some((c) => c.stale) ? `${entry.repo} has no current finished copy. Reconnect and press Find models in Library.` : `${entry.repo} isn't complete on ${namesOf(Object.keys(entry.copies))}. Finish it first.`
    : null;

  const [picked, setPicked] = useState<string[]>(first.nodes);
  const [servedName, setServedName] = useState(was?.servedName ?? defaultName(repo));
  const [tp, setTp] = useState(was?.tensorParallel ?? rec?.tensorParallelSize ?? Math.max(1, picked.length));
  const [pp, setPp] = useState(was?.pipelineParallel ?? rec?.pipelineParallelSize ?? 1);
  const [replicas, setReplicas] = useState(was?.replicas ?? 1);
  const [imported, setImported] = useState<ImportedRecipe | null>(was?.importedRecipe ?? null);
  const [ep, setEp] = useState(was?.expertParallel ?? rec?.expertParallel ?? false);
  const [maxLen, setMaxLen] = useState(was ? (was.maxModelLen ? String(was.maxModelLen) : "") : advice.get("--max-model-len") ?? (rec?.maxModelLen ? String(rec.maxModelLen) : ""));
  const [gpuMem, setGpuMem] = useState(was ? String(was.gpuMemoryUtilization) : advice.get("--gpu-memory-utilization") ?? String(rec?.gpuMemoryUtilization ?? FIRST_LOAD_SHARE));
  const [port, setPort] = useState<string | null>(was ? String(was.port) : null);
  const [listen, setListen] = useState<Listen>(was?.listen ?? "lan");
  // The SGLang launcher opens this page with SGLang picked (draft.engine).
  const [engine, setEngine] = useState<LoadEngine>(was?.engine ?? draft.engine ?? "vllm");
  const sglang = engine === "sglang";
  const [requireKey, setRequireKey] = useState(was?.requireKey ?? true);
  const [remoteCode, setRemoteCode] = useState(was?.allowRemoteCode ?? false);
  const [keepRunning, setKeepRunning] = useState(was?.keepRunning ?? false);
  const [seqs, setSeqs] = useState(was ? (was.maxNumSeqs ? String(was.maxNumSeqs) : "") : advice.get("--max-num-seqs") ?? "");
  const [kvDtype, setKvDtype] = useState<NonNullable<DeploySpec["kvCacheDtype"]>>(was?.kvCacheDtype ?? (advice.get("--kv-cache-dtype") === "fp8" ? "fp8" : "auto"));
  const [prefix, setPrefix] = useState(was ? was.prefixCaching !== false : !advice.has("--no-enable-prefix-caching"));
  const [tools, setTools] = useState(was ? !!was.toolCallParser : advice.has("--enable-auto-tool-choice") || advice.has("--tool-call-parser"));
  const [toolParser, setToolParser] = useState(was?.toolCallParser || advice.get("--tool-call-parser") || toolParserFor(repo));
  const [reasoning, setReasoning] = useState<string>(was ? was.reasoningParser ?? "" : advice.get("--reasoning-parser") ?? reasoningParserFor(repo) ?? "");
  const [extra, setExtra] = useState(was ? expertText(was.extraArgs) : extraFromAdvice(draft.allowed ?? []));
  const [override, setOverride] = useState(false);
  const [aiFields, setAiFields] = useState<Set<AiField>>(() => {
    const s = new Set<AiField>();
    if (rec) s.add("split");
    if (rec?.maxModelLen || advice.has("--max-model-len")) s.add("maxLen");
    if (rec || advice.has("--gpu-memory-utilization")) s.add("gpuMem");
    if (advice.has("--max-num-seqs")) s.add("seqs");
    if (advice.has("--kv-cache-dtype")) s.add("kv");
    if (advice.has("--enable-prefix-caching")) s.add("prefix");
    if (advice.has("--tool-call-parser") || advice.has("--enable-auto-tool-choice")) s.add("tools");
    if (advice.has("--reasoning-parser")) s.add("reasoning");
    if (extraFromAdvice(draft.allowed ?? [])) s.add("extra");
    return s;
  });
  // What a recipe's defaults leave alone: what was touched here, the AI's fields and a saved load's settings.
  const [chosen, setChosen] = useState<Set<AiField>>(() => new Set(was ? ALL_FIELDS : aiFields));
  const without = (s: Set<AiField>, f: AiField) => (s.has(f) ? new Set([...s].filter((x) => x !== f)) : s);
  const touched = (f: AiField) => {
    setAiFields((s) => without(s, f));
    setChosen((s) => (s.has(f) ? s : new Set([...s, f])));
  };
  const ai = (f: AiField) => (aiFields.has(f) ? " ai-filled" : "");

  // The load's id, made now so the preview shows its real container names and Keychain item.
  const [loadId] = useState(() => draft.replaces ?? `m${Array.from(crypto.getRandomValues(new Uint8Array(3)), (b) => b.toString(16).padStart(2, "0")).join("")}`);
  const [planned, setPlanned] = useState<Planned | null>(null);
  // The last plan, for its Split menu, its port and its recipe; whether it's for what's chosen now is `current`.
  const plan = planned?.plan ?? null;
  const [preview, setPreview] = useState<PreviewStep[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const copyingMissing = useRef(false);
  const [planProblem, setPlanProblem] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);

  // Nodes in cluster order; the first one is the head and serves the API.
  const ordered = nodes.map((n) => n.id).filter((id) => picked.includes(id));
  const head = ordered[0];
  const headName = head ? nameOf(head) : undefined;
  // A vLLM load on any number of nodes: its image by local id, and the pre-flight for the set (P2.7).
  const checked = !sglang && ordered.length > 0;
  const [images, setImages] = useState<NodeImages[] | null>(null);
  useEffect(() => {
    opsApi.imagesList().then(setImages, () => setImages([]));
  }, []);
  const choices = imageChoices(images, ordered, settings?.image);
  const [pickedImage, setPickedImage] = useState<string | null>(was?.imageRef || null);
  const imageRef = pickedImage ?? defaultImage(choices, settings?.image);
  const [preflight, setPreflight] = useState<PreflightSet | null>(null);
  const [imageParsers, setImageParsers] = useState<{ key: string; imageId: string; tools: string[] | null; reasoning: string[] | null } | null>(null);
  const [parserBusy, setParserBusy] = useState(false);
  const [parserProblem, setParserProblem] = useState<string | null>(null);
  const parserKey = imageRef + "@" + ordered.join(",");
  const knownParsers = imageParsers?.key === parserKey && (!preflight?.imageId || preflight.imageId === imageParsers.imageId) ? imageParsers : null;
  const readParsers = async () => {
    if (parserBusy || !(await requestAdmin())) return;
    const key = parserKey;
    setParserBusy(true); setParserProblem(null);
    try {
      const found = await api.clusterCheck(imageRef, ordered);
      const rows = found.map((c) => c.parsers);
      if (rows.length !== ordered.length || rows.some((r) => !r) || new Set(rows.map((r) => r?.imageId)).size !== 1) throw new Error("The selected image's parsers could not be confirmed on every node.");
      const common = (field: "tools" | "reasoning") => rows.every((r) => r?.[field]) ? rows[0]![field]!.filter((p) => rows.every((r) => r![field]!.includes(p))) : null;
      setImageParsers({ key, imageId: rows[0]!.imageId, tools: common("tools"), reasoning: common("reasoning") });
    } catch (e) { setParserProblem(errText(e)); }
    finally { setParserBusy(false); }
  };
  const toolChoices = knownParsers?.tools ?? TOOL_PARSERS;
  const reasoningChoices = knownParsers?.reasoning ?? REASONING_PARSERS;
  const parserRefused = tools && knownParsers?.tools && !knownParsers.tools.includes(toolParser) ? "The selected image does not list this tool-call parser. Pick one it supports."
    : reasoning && knownParsers?.reasoning && !knownParsers.reasoning.includes(reasoning) ? "The selected image does not list this reasoning parser. Pick one it supports." : null;

  const [pfProblem, setPfProblem] = useState<string | null>(null);
  // A removal that failed: unlike a failed read, it stands when the nodes are read again.
  const [cleanProblem, setCleanProblem] = useState<string | null>(null);
  const [pfSeq, setPfSeq] = useState(0);
  const [pfAgain, setPfAgain] = useState(0);
  const checks = preflight && { ...preflight, problems: preflight.problems.map(named), nodes: preflight.nodes.map((p) => ({ ...p, problems: p.problems.map(named) })) };
  // The tick is consent for the nodes it names, on the nodes chosen when it was given: only to
  // the drop the page offered, and another node that comes up short asks again.
  const short = (preflight?.nodes ?? []).filter((p) => p.dropNeeded).map((p) => p.node).join(",");
  const [dropFor, setDropFor] = useState<string | null>(null);
  const dropConsent = !!short && dropFor === short;
  useEffect(() => setDropFor(null), [ordered.join(",")]);
  const [recipePick, setRecipePick] = useState<string | null>(was?.recipe || null);
  // Picking nodes picks the split again: all tensor parallel until the plan offers the first valid one.
  const toggle = (id: string, on: boolean) => {
    const next = on ? [...picked, id] : picked.filter((p) => p !== id);
    setPicked(next);
    setAiFields((s) => without(s, "split"));
    setChosen((s) => without(s, "split"));
    setTp(Math.max(next.length, 1));
    setPp(1);
    setReplicas(1);
  };

  const portNum = port === null ? (plan?.suggestedPort ?? 8000) : Number(port);
  // Numbers the fields can't turn into what Rust takes never leave the page: serde's sentence is no help here.
  const maxLenBad = wholeProblem("Max context", maxLen, 1, LIMITS.maxContext);
  const seqsBad = wholeProblem("Requests at once", seqs, 1, LIMITS.requests);
  const portBad = port === null ? null : wholeProblem("Port", port || "0", 1024, 65535);
  // The share follows the plan until someone chooses one. A saved load, the AI's advice and
  // anything typed are in `chosen`, so they stay. Otherwise it's what the plan for this very
  // model, nodes and limits said: a recipe's own share (it has a reason), the share sized to the
  // model, or the first-load share when the plan has none and for SGLang, whose share is worked
  // out differently (the sizing arithmetic is vLLM's). Until that plan arrives there is no share:
  // a plan for another model says nothing about this one, the nodes aren't read, and Load waits.
  const shareChosen = chosen.has("gpuMem");
  const sizeKey = sizeKeyOf(engine, entry?.key ?? "", ordered, tp, pp, maxLen, seqs, kvDtype, recipePick) + `@${sglang ? 1 : replicas}`;
  const current = planned?.key === sizeKey;
  const targetOf = (p: DeployPlan) => (sglang ? FIRST_LOAD_SHARE : p.recipe?.defaults.share ?? p.sizedShare ?? FIRST_LOAD_SHARE);
  // What the page follows: the first plan's answer for this identity. A later plan for the same
  // identity, asked at the followed share, doesn't move it, so a plan that flips can't chase itself.
  const [followed, setFollowed] = useState<{ key: string; share: number } | null>(null);
  useEffect(() => {
    if (planned && planned.key === sizeKey && followed?.key !== sizeKey) setFollowed({ key: sizeKey, share: targetOf(planned.plan) });
  }, [planned, sizeKey, followed?.key]);
  const share: number | null = shareChosen ? Number(gpuMem) : followed?.key === sizeKey ? followed.share : null;
  const shareBad = !shareChosen || (share !== null && share >= MIN_SHARE && share <= 0.95) ? null : `GPU memory share must be a number from ${MIN_SHARE} to 0.95.`;
  const numbers = maxLenBad ?? shareBad ?? seqsBad ?? portBad;
  const spec: DeploySpec | null = useMemo(() => {
    if (!settings || !entry) return null;
    return {
      id: loadId,
      engine,
      servedName: servedName.trim(),
      modelPath: entry.key,
      nodes: ordered,
      replicas: sglang ? 1 : replicas,
      tensorParallel: sglang ? Math.max(1, ordered.length) : tp,
      pipelineParallel: sglang ? 1 : pp,
      expertParallel: ep,
      maxModelLen: maxLen.trim() ? Number(maxLen) : null,
      // With no share yet, the plan is asked at the last one followed (or the first-load share) to learn this model's: that answer sets the share, and the plan is asked again at it.
      gpuMemoryUtilization: share ?? followed?.share ?? FIRST_LOAD_SHARE,
      port: portNum,
      image: sglang ? settings.sglangImage || "lmsysorg/sglang:spark" : imageRef,
      imageRef: sglang ? undefined : imageRef,
      dropCaches: !sglang && dropConsent,
      recipe: sglang ? "" : recipePick ?? "",
      importedRecipe: sglang ? null : imported,
      group: was?.group ?? "",
      extraArgs: sglang ? [] : parseExpertArgs(extra),
      allowRemoteCode: remoteCode,
      listen,
      // SGLang reads its key only from its command line, so SGLang loads go without one.
      requireKey: sglang ? false : requireKey,
      maxNumSeqs: seqs.trim() ? Number(seqs) : null,
      kvCacheDtype: kvDtype,
      prefixCaching: prefix ? null : false,
      toolCallParser: tools ? toolParser : null,
      reasoningParser: reasoning || null,
      keepRunning,
      overrideMemory: override,
    };
  }, [loadId, engine, settings, entry, servedName, ordered.join(","), tp, pp, replicas, imported, ep, maxLen, share, followed?.share, portNum, extra, remoteCode, listen, requireKey, seqs, kvDtype, prefix, tools, toolParser, reasoning, keepRunning, override, imageRef, dropConsent, recipePick, was?.group]);

  // The pre-flight reads the nodes again when what it reads changes, and when the drop is ticked:
  // memory short with no consent is a problem in its verdict.
  // The nodes aren't read until the share is known: a read at a share this load won't use would offer a drop it doesn't need.
  const pfKey = checked && spec && share !== null ? JSON.stringify([spec.nodes, imageRef, spec.modelPath, spec.port, spec.gpuMemoryUtilization, spec.tensorParallel, spec.pipelineParallel, spec.replicas]) : "";
  useEffect(() => {
    setPreflight(null);
    setPfProblem(null);
    setCleanProblem(null);
  }, [pfKey]);
  useEffect(() => {
    if (!pfKey || !spec || numbers) return;
    // A slower read of nodes no longer chosen doesn't overwrite a newer one.
    let stale = false;
    const t = window.setTimeout(() => {
      modelsApi.modelPreflight(spec).then(
        (p) => {
          if (stale) return;
          setPreflight(p);
          setPfProblem(null);
          setPfSeq((n) => n + 1);
        },
        (e) => {
          if (!stale) setPfProblem(named(errText(e)));
        },
      );
    }, 250);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [pfKey, dropConsent, pfAgain, !numbers]);

  useEffect(() => {
    if (!spec || !spec.nodes.length) {
      setPlanned(null);
      setPlanProblem(null);
      setPreview(null);
      return;
    }
    // The last plan stays for its Split menu, under the sentence that says what to fix.
    if (numbers) {
      setPreview(null);
      setProblem(null);
      return;
    }
    // A slower answer about an earlier choice doesn't overwrite a newer one.
    let stale = false;
    const t = window.setTimeout(() => {
      modelsApi.deployPlan(spec).then(
        (p) => {
          if (stale) return;
          setPlanned({ plan: p, key: sizeKey, share: spec.gpuMemoryUtilization });
          setPlanProblem(null);
        },
        (e) => {
          if (stale) return;
          setPlanned(null);
          setPlanProblem(named(errText(e)));
        },
      );
      modelsApi.deployPreview(spec).then(
        (steps) => {
          if (stale) return;
          setPreview(steps);
          setProblem(null);
        },
        (e) => {
          if (stale) return;
          setPreview(null);
          setProblem(named(errText(e)));
        },
      );
    }, 250);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
    // Again once the pre-flight has read the image's local id.
  }, [spec, pfSeq]);

  // The recipe the plan says: the one picked here, else the folder's. Its defaults fill what nobody chose, once for each.
  const recipe = sglang ? null : plan?.recipe ?? null;
  const filled = useRef<string | null>(null);
  useEffect(() => {
    if (!recipe || filled.current === recipe.id) return;
    filled.current = recipe.id;
    const fill = recipeFill(recipe, chosen);
    if (fill.gpuMem !== undefined) setGpuMem(fill.gpuMem);
    if (fill.maxLen !== undefined) setMaxLen(fill.maxLen);
    if (fill.seqs !== undefined) setSeqs(fill.seqs);
    if (fill.tools !== undefined) setTools(fill.tools);
    if (fill.toolParser !== undefined) setToolParser(fill.toolParser);
    if (fill.reasoning !== undefined) setReasoning(fill.reasoning);
  }, [recipe?.id]);
  const recipeBlock = recipe?.placeholder ? recipe.note ?? `${recipe.title} can't load yet.` : null;

  // The plan on the page: for what's chosen now, and (unless the share was chosen) at the share
  // the page follows, so a plan asked only to learn the share is never drawn.
  const shown = planned && current && (shareChosen || (share !== null && Math.abs(planned.share - share) < 1e-9)) ? planned.plan : null;
  const sized = !sglang && current ? plan?.sizedShare ?? null : null;
  const recipeShare = recipe?.defaults.share ?? null;
  // Back to what the plan would use, as the plan says it now.
  const followPlan = () => {
    if (!planned || !current) return;
    setAiFields((s) => without(s, "gpuMem"));
    setChosen((s) => without(s, "gpuMem"));
    setFollowed({ key: sizeKey, share: targetOf(planned.plan) });
  };
  const target = planned && current ? targetOf(planned.plan) : null;
  const shareBack = target !== null && share !== null && Math.abs(share - target) > 1e-9 ? target : null;
  const sizedHere = !shareChosen && recipeShare === null && sized !== null;

  // The split: the offers for this many nodes. Until one is chosen, the first this model can take.
  const offers = sglang ? [] : plan?.topologies ?? [];
  const offer = offers.find((o) => o.nodes === ordered.length && o.tp === tp && o.pp === pp && (o.replicas ?? 1) === replicas) ?? null;
  const best = firstOffer(offers, ordered.length);
  const splitChosen = chosen.has("split");
  useEffect(() => {
    if (splitChosen || !best) return;
    setTp(best.tp);
    setPp(best.pp);
    setReplicas(best.replicas ?? 1);
  }, [best?.tp, best?.pp, best?.replicas, splitChosen]);
  // A recipe that can't load has one reason, its note: a menu it isn't in has nothing to add.
  const splitRefused = !recipeBlock && offer && !offer.ok && (splitChosen || !best) ? offer.reason : null;
  const splitWarning = offer?.ok ? offer.reason : null;

  const missing = entry ? ordered.filter((n) => !finishedCopy(entry, n)) : [];
  const busy = ordered.filter((n) => busyNodes.has(n));
  // Verdicts and offers come only from a plan for what's chosen now.
  const judged = current ? plan : null;
  const red = judged?.level === "red";
  const portTaken = judged?.portTakenBy ?? null;
  const suggested = judged?.suggestedShare ?? null;
  const shorter = judged?.suggestedContext ?? null;
  // Red, and no share or shorter context keeps every node under the alert line with the model in it.
  const noSafeShare = red && suggested == null && shorter == null && judged?.nodes.some((p) => p.level === "red" && !p.fitsSafe);

  const shareMissing = async () => {
    if (!entry || copyingMissing.current) return;
    copyingMissing.current = true;
    try {
      const checks = await checkTransferDisk({ kind: "copy", key: entry.key, targets: missing });
      if (!checks) return;
      if (!await confirmAction({ title: `Copy ${entry.repo} to ${missing.join(", ")}?`,
        body: <div className="col gap-md"><DiskPreflight checks={checks} /><p>Copy and verify the files on the selected nodes. This does not load the model.</p></div>,
        confirmLabel: "Copy over the fabric", danger: checks.some((c) => !!c.warning) })) return;
      await api.modelShare(entry.key, missing);
      onShared();
    } catch (e) {
      setProblem(named(errText(e)));
    } finally {
      copyingMissing.current = false;
    }
  };

  const start = async () => {
    if (!spec || !plan || !current || !head) return;
    if (!admin && !(await requestAdmin(`load ${spec.servedName}`))) return;
    if (draft.replaces) {
      const ok = await confirmAction({
        title: `Reload ${spec.servedName} with these settings?`,
        body: "Its containers stop, then it loads again with the new settings: it's offline until it's ready again, and apps using it get errors meanwhile.",
        confirmLabel: "Reload",
      });
      if (!ok) return;
    }
    if (red) {
      const ok = await confirmAction({
        title: "Load despite a red memory plan?",
        body: `${plan.nodes.filter((p) => p.level === "red").map((p) => named(p.reason ?? "")).join(" ")} A Spark that runs out of unified memory can freeze instead of failing cleanly.`,
        confirmLabel: "Load anyway",
        danger: true,
        requireTyping: headName,
      });
      if (!ok) return;
    } else if (plan.level === "amber") {
      const tight = plan.nodes.filter((p) => p.level === "amber");
      const ok = await confirmAction({
        title: "Load above the 90% memory alert line?",
        body: `${tight.map((p) => `${nameOf(p.node)} would use ${Math.round(p.afterPct)}% of its memory`).join("; ")}. It starts with little room left. Anything else that grows there can run the node out of memory. Alerts check available memory and pressure after loading.${suggested ? ` ${suggested.toFixed(2)} stays under the line.` : ""}`,
        confirmLabel: "Load",
      });
      if (!ok) return;
    }
    setStarting(true);
    try {
      if (draft.replaces) {
        // The new load starts once the old one is gone and its memory checked.
        const task = await modelsApi.deploymentUnload(draft.replaces);
        if (task) await waitTask(task);
      }
      await modelsApi.deploy(spec);
      toast({ kind: "info", title: `Loading ${spec.servedName}`, body: `On ${namesOf(ordered)}. Endpoints shows its progress; it's in Chat once it answers.` });
      onStarted();
    } catch (e) {
      setProblem(named(errText(e)));
      setStarting(false);
    }
  };

  // After Remove leftovers: nothing's left to confirm but what the nodes say now.
  const cleanLeftovers = async (p: Preflight) => {
    const left = p.leftovers.filter((l) => l.ours);
    const ok = await confirmAction({
      title: `Remove what an earlier load left on ${nameOf(p.node)}?`,
      body: "SparkBox stops and removes these containers and the launch files it made for them. Only this build's own are touched.",
      details: left.map((l) => l.name).join("\n"),
      confirmLabel: "Remove leftovers",
      danger: true,
      dedupeKey: `leftovers:${p.node}`,
    });
    if (!ok) return;
    setCleanProblem(null);
    try {
      const task = await withAdmin(() => modelsApi.deploymentCleanLeftovers([p.node], [...new Set(left.map((l) => l.deployment))]), { action: "deployment_clean_leftovers" });
      const done = task ? await waitTask(task) : null;
      if (done && done.state !== "ok") setCleanProblem(named(done.error ?? `Couldn't remove what was left on ${nameOf(p.node)}.`));
    } catch (e) {
      if (e === ADMIN_REFUSED) return;
      setCleanProblem(named(errText(e)));
    }
    // Part of it may be gone, whatever the answer.
    setPfAgain((n) => n + 1);
  };

  // Check again starts clean: the new read says what's true now.
  const recheck = () => {
    setPfProblem(null);
    setCleanProblem(null);
    setPfAgain((n) => n + 1);
  };

  // Live only: Demo reads no node, and its loads start whatever the image.
  const live = snap ? !snap.demo : false;
  // Only from a listing of the node that worked; the pre-flight says it either way.
  const imageOn = choices.find((c) => c.ref === imageRef)?.on ?? [];
  const absentOn = imageOn.filter((o) => !o.id && o.read).map((o) => o.node);
  const imageMissing = checked && live && absentOn.length ? `${imageRef} isn't on ${namesOf(absentOn)}: pull it first (Fleet: docker pull ${imageRef}), or pick an image that's there` : null;
  const pfBlock = checked && checks && checks.verdict !== "demo"
    ? checks.problems[0] ?? checks.nodes.flatMap((p) => p.problems)[0]
      ?? (checks.nodes.some((p) => p.dropNeeded) && !dropConsent ? "Not enough free memory: tick the page-cache drop or lower the share" : null)
      ?? (checks.verdict === "blocked" ? verdictLine(checks, nameOf) : null)
    : null;
  const blocked = unfinished ? unfinished
    : !spec ? "Pick a model"
    : !ordered.length ? "Pick at least one node"
    : recipeBlock ? recipeBlock
    : parserRefused ? parserRefused
    : numbers ? numbers
    : !current ? (planProblem ? `Couldn't work out the plan: ${planProblem}` : "Working out the memory plan…")
    : missing.length ? `Copy the model to ${namesOf(missing)} first`
    : busy.length ? `${namesOf(busy)} already serve${busy.length === 1 ? "s" : ""} a model`
    : splitRefused ? splitRefused
    : portTaken ? `Port ${portNum} is taken`
    : imageMissing ? imageMissing
    : pfBlock ? pfBlock
    : red && !override ? "The memory plan is red"
    : problem ? problem
    : null;

  const expertRefused = expertRefusal(problem, extra);
  // A vLLM head or worker runs with --privileged (recipe multiNodeDocker); one node never does.
  const privileged = !sglang && ordered.length / replicas > 1;

  return (
    <div className="col fill">
      <Toolbar label="Load">
        <ToolButton icon={ArrowLeft} label="Back" onClick={onClose} />
        <span className="toolbar-text strong">{draft.replaces ? `Edit ${was?.servedName ?? "the load"} and reload` : `Load a model with ${sglang ? "SGLang" : "vLLM"}`}</span>
        <Spacer />
        {blocked && !starting && <span className="ai-blocked" role="status">{blocked}</span>}
        <button className="btn primary" disabled={!!blocked || starting || (!admin && isLookOnly())} title={!admin && isLookOnly() ? LOOK_ONLY_TIP : blocked ?? (admin ? undefined : "Asks to unlock Admin mode first")} onClick={start}>
          {starting ? "Starting…" : draft.replaces ? "Reload" : `Load on ${ordered.length} node${ordered.length === 1 ? "" : "s"}`}
        </button>
      </Toolbar>
      <div className="ai-load">
        {rec && (
          <Notice tone="info">
            <Icon icon={Sparkles} size={12} /> Filled in from the AI analysis (highlighted): {rec.why}
          </Notice>
        )}

        <FormGroup title="Model">
          {!sglang && <RecipeImport nodes={nodes} onUse={(d) => {
            setImported(d.recipe); setRecipePick(d.recipe.id); setPickedImage(d.recipe.image);
            setTp(d.tensorParallel); setPp(d.pipelineParallel); setReplicas(1); setExtra(""); setKvDtype((d.kvCacheDtype || "auto") as NonNullable<DeploySpec["kvCacheDtype"]>); setPrefix(d.prefixCaching !== false); setEp(!!d.expertParallel);
            setMaxLen(d.recipe.defaults.context == null ? "" : String(d.recipe.defaults.context));
            setSeqs(d.recipe.defaults.seqs == null ? "" : String(d.recipe.defaults.seqs));
            setTools(!!d.recipe.toolParser); if (d.recipe.toolParser) setToolParser(d.recipe.toolParser);
            setReasoning(d.recipe.reasoningParser ?? "");
            if (d.recipe.defaults.share != null) setGpuMem(String(d.recipe.defaults.share));
            setChosen(new Set(ALL_FIELDS.filter((f) => f !== "gpuMem" || d.recipe.defaults.share != null))); setAiFields(new Set());
            if (d.servedName) setServedName(d.servedName);
            if (library.some((e) => e.key === d.modelPath)) { setKey(d.modelPath); setProblem(null); }
            else { setKey(""); setProblem("The imported model path is not in the Library. Choose its model folder before loading."); }
          }} />}
          <FormRow label={<>Engine <HelpTip topic="models.load.engine" /></>} hint={sglang ? "SGLang serves the same OpenAI API; several nodes split it by tensor parallel" : undefined}>
            <Segmented<LoadEngine> label="Engine" value={engine} onChange={setEngine}
              options={[{ value: "vllm", label: "vLLM" }, { value: "sglang", label: "SGLang" }]} />
          </FormRow>
          <FormRow label={<>Folder <HelpTip topic="models.load.folder" /></>}>
            <select className="wide" value={key} onChange={(e) => {
              setKey(e.target.value);
              if (key) setImported(null);
              const next = library.find((x) => x.key === e.target.value);
              setRecipePick(null);
              if (next) {
                setServedName(defaultName(next.repo));
                setToolParser(toolParserFor(next.repo));
                setReasoning(reasoningParserFor(next.repo) ?? "");
                setChosen((s) => without(without(s, "tools"), "reasoning"));
                // Nothing picked yet: the freest nodes that hold it.
                if (!picked.length) {
                  const want = Math.max(1, Math.ceil((next.sizeBytes * 1.2 + 12e9) / Math.max(1, Math.max(...nodes.map((n) => freeOf(n.id))) * 0.8)));
                  const ids = nodesFor(next, Math.min(want, nodes.length));
                  setPicked(ids);
                  setTp(ids.length);
                  setPp(1);
                }
              }
            }}>
              {!key && <option value="">Choose a model folder…</option>}
              {folders.map((e) => <option key={e.key} value={e.key}>{e.repo} — {fmtBytes(e.sizeBytes)}{e.inUse ? " (in use)" : ""}</option>)}
            </select>
          </FormRow>
          {unfinished && <Notice tone="warn">{unfinished}</Notice>}
          {recipe && (
            <FormRow label={<>Recipe <HelpTip topic="use-a-recipe-for-a-load" /></>} hint={verifiedWords(recipe)}>
              {recipe.fits.length > 1
                ? (
                  <select className="wide" aria-label="Recipe" value={recipePick ?? recipe.id} onChange={(e) => { setRecipePick(e.target.value); setImported(null); }}>
                    {recipe.fits.map((r) => <option key={r.id} value={r.id}>{r.title}</option>)}
                  </select>
                )
                : <span>{recipe.title}</span>}
              {recipe.note && !recipe.placeholder && <div className="muted small">{recipe.note}</div>}
            </FormRow>
          )}
          {recipeBlock && <Notice tone="warn">{recipeBlock}</Notice>}
          <FormRow label={<>Name in the API <HelpTip topic="models.load.api-name" /></>} hint={'What apps put in "model"'}>
            <input className="wide mono" value={servedName} onChange={(e) => setServedName(e.target.value)} />
          </FormRow>
        </FormGroup>

        <FormGroup
          title={
            <>
              Nodes{" "}
              <InfoTip label="About nodes" topic="models.load.nodes">
                {ordered.length > 1
                  ? sglang
                    ? `${headName} is rank 0 and serves the API; the others meet it on its fabric address (port 5000) and split the model by tensor parallel.`
                    : `${headName} is the head and serves the API. The workers start first, then the head, and they meet on the fabric.`
                  : `One node: ${sglang ? "SGLang" : "vLLM"} runs directly.`}
              </InfoTip>
            </>
          }
        >
          <div className="ai-node-picks">
            {nodes.map((n) => {
              const has = !!(entry && finishedCopy(entry, n.id));
              const partial = entry?.copies[n.id]?.partial;
              const stale = entry?.copies[n.id]?.stale;
              const taken = busyNodes.has(n.id);
              const serving = servingOn(n.id);
              const fault = faultOf(n.id);
              return (
                <label key={n.id} className={`check node-pick${ai("split")}${fault ? " disabled" : ""}`} title={fault ?? (taken ? "Already serving a SparkBox load" : serving.length ? `Serving ${serving.join(", ")}` : has ? "Has a copy" : stale ? "Last-known copy; find models again" : "No copy yet")}>
                  <input type="checkbox" checked={picked.includes(n.id) && !fault} disabled={!!fault} onChange={(e) => toggle(n.id, e.target.checked)} />
                  <StatusDot status={n.online ? "online" : "offline"} />
                  {n.name}
                  <span className="muted small">{fault ?? [entry ? (has ? "has copy" : stale ? "last known" : partial === "stopped" ? "incomplete" : partial ?? "no copy") : null, serving.length ? `serving ${serving[0]}` : null].filter(Boolean).join(" · ")}</span>
                </label>
              );
            })}
          </div>
          {missing.length > 0 && !unfinished && (
            <Notice tone="warn" action={admin && !missing.some((n) => entry?.copies[n]?.stale) ? { label: "Copy it there", onClick: shareMissing } : undefined}>
              {missing.some((n) => entry?.copies[n]?.stale) ? "Some selected copies are last known. Reconnect and press Find models in Library before loading or copying."
                : `${namesOf(missing)} ${missing.length === 1 ? "doesn't" : "don't"} have this model yet. Copy it over the fabric, then come back.`}
            </Notice>
          )}
          {busy.length > 0 && <Notice tone="warn">{namesOf(busy)} already {busy.length === 1 ? "serves" : "serve"} a SparkBox load. Unload it first (Loaded tab).</Notice>}
          {ordered.length > 1 && <Notice tone="info" action={{ label: "What's proven", onClick: () => openHelp("whats-proven") }}>
            Each model, image and split needs its own checks. A successful load on one layout doesn't prove another.
          </Notice>}
          {!!judged?.inventoryWarnings?.length && <Notice tone="warn" action={{ label: "Review in Settings", onClick: () => openWindow("settings", { section: "nodes" }) }}>
            <strong>Inventory expectations need review.</strong>
            <ul>{judged.inventoryWarnings.map((warning, i) => <li key={i}>{warning}</li>)}</ul>
            These are advisory. Loads use the fabric reported by fresh node checks.
          </Notice>}
        </FormGroup>

        <FormGroup
          title={
            <>
              Memory plan{" "}
              <HelpTip topic="models.load.memory-plan" />
            </>
          }
        >
          {numbers ? <div className="ai-plans muted">{numbers}</div> : shown ? (
            <div className="ai-plans">
              {shown.nodes.map((p) => <PlanBar key={p.node} p={{ ...p, reason: p.reason && named(p.reason) }} name={nameOf(p.node)} engine={sglang ? "SGLang" : "vLLM"} requests={shown.fullRequests ?? null} />)}
              {suggested != null && (
                <div className="ai-inline">
                  <button type="button" className="btn small" onClick={() => { setGpuMem(suggested.toFixed(2)); touched("gpuMem"); }}>
                    Use {suggested.toFixed(2)} — fits and stays under 90%
                  </button>
                </div>
              )}
              {shorter && (
                <div className="ai-inline">
                  <button type="button" className="btn small" onClick={() => { setGpuMem(shorter[0].toFixed(2)); setMaxLen(String(shorter[1])); touched("gpuMem"); touched("maxLen"); }}>
                    Use {shorter[0].toFixed(2)} and a {fmtTokens(shorter[1])} context — fits and stays under 90%
                  </button>
                </div>
              )}
              {noSafeShare && <div className="muted small">No share fits this model under the 90% line here: lower the max context or requests at once, or add nodes.</div>}
              {red && (
                <label className="check ai-override">
                  <input type="checkbox" checked={override} onChange={(e) => setOverride(e.target.checked)} />
                  Override memory guardrail <span className="muted">(you'll type {headName} to confirm)</span> <HelpTip topic="models.load.override" />
                </label>
              )}
            </div>
          ) : <div className="ai-plans muted">{!entry ? "Choose a model folder to see where it fits." : !ordered.length ? "Pick the nodes to see the plan." : planProblem ? `Couldn't work out the plan: ${planProblem}` : "Working out the plan…"}</div>}
        </FormGroup>

        <FormGroup title={<>Split and limits <HelpTip topic="splitting-a-model" /></>}>
          {sglang ? (
            <FormRow label={<>Tensor parallel <HelpTip topic="models.load.tensor-pipeline" /></>} hint="SGLang splits the model across every node picked (one GPU each)">
              <span className="num">TP {Math.max(1, ordered.length)}</span>
            </FormRow>
          ) : (
          <>
            {offers.some((o) => o.nodes === ordered.length) && (
              <FormRow label="Split" hint="What this model can take on these nodes">
                <select aria-label="Split" value={offer?.id ?? ""} onChange={(e) => {
                  const next = offers.find((o) => o.id === e.target.value);
                  if (!next) return;
                  setTp(next.tp);
                  setPp(next.pp);
                  setReplicas(next.replicas ?? 1);
                  touched("split");
                }}>
                  {!offer && <option value="">{tp} × {pp}: your own</option>}
                  {offers.filter((o) => o.nodes === ordered.length).map((o) => <option key={o.id} value={o.id}>{offerWords(o, !!recipeBlock)}</option>)}
                </select>
              </FormRow>
            )}
            <FormRow label={<>Tensor × pipeline parallel <HelpTip topic="models.load.tensor-pipeline" /></>} hint={`Multiplied by ${replicas} replica${replicas === 1 ? "" : "s"}, must equal the number of nodes (one GPU each)`}>
              <span className={`ai-inline${ai("split")}`}>
                <input className="narrow" type="number" min={1} value={tp} aria-label="Tensor parallel" onChange={(e) => { setTp(splitPart(e.target.value)); touched("split"); }} />
                <span className="muted">×</span>
                <input className="narrow" type="number" min={1} value={pp} aria-label="Pipeline parallel" onChange={(e) => { setPp(splitPart(e.target.value)); touched("split"); }} />
              </span>
            </FormRow>
            {splitRefused && <Notice tone="error">{splitRefused}</Notice>}
            {splitWarning && <Notice tone="info">{splitWarning}</Notice>}
            {replicas > 1 && <Notice tone="info">{replicas} independent loads share this served name. Nodes are paired in the order shown for TP2. Each replica has its own endpoint, key and Unload action. A failed replica does not stop the others; request balancing is not included.</Notice>}
          </>
          )}
          <FormRow label={<>Expert parallel <HelpTip topic="models.load.expert-parallel" /></>} hint="Spread MoE experts across nodes">
            <Switch checked={ep} onChange={(v) => { setEp(v); touched("split"); }} label="Expert parallel" />
          </FormRow>
          <FormRow label={<>Max context <HelpTip topic="models.load.max-context" /></>} hint={plan?.maxContext ? `The model allows ${plan.maxContext.toLocaleString()}` : "Tokens; lower it if the KV cache doesn't fit"}>
            <input className={`narrow-wide${ai("maxLen")}`} type="number" placeholder={plan?.maxContext ? String(plan.maxContext) : "model default"} value={maxLen}
              onChange={(e) => { setMaxLen(e.target.value); touched("maxLen"); }} />
          </FormRow>
          {maxLenBad && <Notice tone="error">{maxLenBad}</Notice>}
          <FormRow label={<>GPU memory share <HelpTip topic="models.load.gpu-share" /></>} hint={share === null ? "Waiting for the memory plan to size it" : sglang ? "Of each node's unified memory, reserved by SGLang at start. SparkBox sizes the share for vLLM only, so SGLang starts at the usual share" : `${sizedHere ? "Sized to this model. " : "Of each node's unified memory. "}vLLM reserves all of it at start, and what the model leaves becomes KV cache`}>
            <span className="ai-inline">
              <input className={`narrow${ai("gpuMem")}`} type="number" aria-label="GPU memory share" step={0.05} min={MIN_SHARE} max={0.95} placeholder="…" value={shareChosen ? gpuMem : share === null ? "" : String(share)}
                onChange={(e) => { setGpuMem(e.target.value); touched("gpuMem"); }} />
              {shareBack !== null && (
                <button type="button" className="btn small" onClick={followPlan}>
                  Use {shareBack.toFixed(2)} — {!sglang && recipeShare !== null ? "the recipe's share" : sized !== null ? "sized to this model" : "the usual start"}
                </button>
              )}
            </span>
          </FormRow>
          {shareBad && <Notice tone="error">{shareBad}</Notice>}
        </FormGroup>

        <FormGroup title="Capabilities">
          {!sglang && <div className="small"><button type="button" className="btn small" disabled={parserBusy || !ordered.length || !admin || isLookOnly(adminState)} onClick={() => void readParsers()}>{parserBusy ? "Checking image…" : "Check image parsers…"}</button> {knownParsers ? "Choices read from the selected image on every node." : "Parser suggestions have not been checked against this image. This check starts temporary test containers."}</div>}
          {parserProblem && <Notice tone="warn">{parserProblem}</Notice>}
          {parserRefused && <Notice tone="error">{parserRefused}</Notice>}
          <FormRow label={<>Tool calling <HelpTip topic="models.load.tool-calling" /></>} hint="Lets apps like Claude Code and Codex call tools">
            <span className={`ai-inline${ai("tools")}`}>
              <Switch checked={tools} onChange={(v) => { setTools(v); touched("tools"); }} label="Tool calling" />
              <select value={toolParser} disabled={!tools} aria-label="Tool-call parser" onChange={(e) => { setToolParser(e.target.value); touched("tools"); }}>
                {[...new Set([toolParser, ...toolChoices])].map((p) => <option key={p} value={p}>{p}{p === toolParserFor(repo) ? " (this model family)" : ""}</option>)}
              </select>
            </span>
          </FormRow>
          <FormRow label={<>Reasoning parser <HelpTip topic="models.load.reasoning" /></>} hint="Separates the model's thinking from its answer">
            <select className={ai("reasoning")} value={reasoning} onChange={(e) => { setReasoning(e.target.value); touched("reasoning"); }}>
              <option value="">None</option>
              {[...new Set([...(reasoning ? [reasoning] : []), ...reasoningChoices])].map((p) => <option key={p} value={p}>{p}{p === reasoningParserFor(repo) ? " (this model family)" : ""}</option>)}
            </select>
          </FormRow>
          <FormRow label={<>Prefix caching <HelpTip topic="models.load.prefix-caching" /></>} hint="Reuses the KV cache of repeated prompt starts (system prompts, chat history)">
            <span className={ai("prefix")}><Switch checked={prefix} onChange={(v) => { setPrefix(v); touched("prefix"); }} label="Prefix caching" /></span>
          </FormRow>
          <FormRow label={<>KV cache type <HelpTip topic="models.load.kv-cache-type" /></>}>
            <span className={ai("kv")}>
              <select aria-label="KV cache type" value={kvDtype} onChange={(e) => { setKvDtype(e.target.value as NonNullable<DeploySpec["kvCacheDtype"]>); touched("kv"); }}>
                <option value="auto">Model's own</option><option value="bfloat16">bfloat16</option><option value="fp8">fp8 (half the memory)</option><option value="fp8_e4m3">fp8_e4m3</option><option value="fp8_e5m2">fp8_e5m2</option>
              </select>
            </span>
          </FormRow>
          <FormRow label={<>Requests at once <HelpTip topic="models.load.requests-at-once" /></>} hint="Full-length requests the KV cache must hold together (empty: the engine decides, the plan counts one and the share leaves room for four). Changes the memory plan.">
            <input className={`narrow${ai("seqs")}`} type="number" min={1} placeholder="auto" value={seqs} onChange={(e) => { setSeqs(e.target.value); touched("seqs"); }} />
          </FormRow>
          {seqsBad && <Notice tone="error">{seqsBad}</Notice>}
        </FormGroup>

        <FormGroup title="Access">
          <FormRow label={<>Port <HelpTip topic="models.load.port" /></>}>
            <span className="ai-inline">
              <input className={`narrow${portBad || portTaken ? " invalid" : ""}`} type="number" aria-label="Port" value={port ?? String(portNum)} aria-invalid={!!portBad || !!portTaken} onChange={(e) => setPort(e.target.value)} />
              {portBad ? <span className="error-text" role="alert">{portBad}</span> : portTaken
                ? (
                  <span className="error-text" role="alert">
                    In use on {headName} by {portTaken}.{" "}
                    {plan && <button type="button" className="link" onClick={() => setPort(String(plan.suggestedPort))}>Use {plan.suggestedPort}</button>}
                  </span>
                )
                : <span className="muted">free on {headName ?? "the head node"}</span>}
            </span>
          </FormRow>
          <FormRow label={<>Listen on <HelpTip topic="models.load.listen-on" /></>} hint={plan ? `Apps reach it at ${plan.url}` : undefined}>
            <Segmented<Listen>
              label="Listen on"
              value={listen}
              onChange={setListen}
              options={[
                { value: "lan", label: "LAN", title: "The head node's LAN address: this Mac and your network" },
                { value: "fabric", label: "Fabric only", title: "Only the other Sparks; Chat and the Server can't reach it" },
                { value: "all", label: "All interfaces", title: "Every interface, the fabric included" },
              ]}
            />
          </FormRow>
          <FormRow label={<>API key <HelpTip topic="models.load.api-key" /></>} hint={sglang ? "SGLang only takes a key on its command line, where anyone on the node could read it, so SGLang loads have none" : "A new key, kept in the Keychain; Chat and SparkBox Server use it for you"}>
            <Switch checked={sglang ? false : requireKey} onChange={setRequireKey} label="Require an API key" disabled={sglang} />
          </FormRow>
          {(sglang || !requireKey) && listen !== "fabric" && <Notice tone="warn">Without a key, anything on your network can use this model.</Notice>}
          <FormRow label={<>Keep running <HelpTip topic="models.load.keep-running" /></>} hint={ordered.length > 1 ? "SparkBox restarts it in order while SparkBox is open on this Mac (after 30 s, then 2, 5 and 10 minutes). It gives up after 5 restarts in 30 minutes, and doesn't restart a load that has never been Ready." : "Docker restarts it after a crash or a reboot (unless-stopped), with or without SparkBox open."}>
            <Switch checked={keepRunning} onChange={setKeepRunning} label="Keep running" />
          </FormRow>
          <FormRow label={<>Allow remote code <HelpTip topic="models.load.remote-code" /></>} hint={`Only for models that need their own Python (trust_remote_code).${privileged ? " On several nodes the containers run with --privileged, so that Python runs as root with every device on the node." : ""}`}>
            <Switch checked={remoteCode} onChange={setRemoteCode} label="Allow remote code" />
          </FormRow>
          {remoteCode && (
            <div className="notice error ai-remote" role="alert">
              <Icon icon={ShieldAlert} size={14} />
              <div>
                <strong>{repo}</strong> will run its own Python code inside the vLLM container on {namesOf(ordered) || "the nodes"}, with access to {privileged ? "every device on those nodes, as root" : "the GPU and the model folder"}. Only allow this for repositories you trust.
              </div>
            </div>
          )}
          {!sglang && admin && (
            <FormRow label={<>Expert: raw vLLM args <HelpTip topic="models.load.extra-options" /></>} hint="One option per line; the first space splits the flag from its value. Goes to vLLM as it stands, with no shell">
              <textarea className={`wide mono${ai("extra")}`} rows={Math.min(8, Math.max(2, extra.split("\n").length))} aria-label="Expert: raw vLLM args" spellCheck={false}
                placeholder={"--enforce-eager\n--quantization fp8"} value={extra} onChange={(e) => { setExtra(e.target.value); touched("extra"); }} />
            </FormRow>
          )}
          {!sglang && !admin && extra.trim() && <FormRow label="Expert: raw vLLM args" hint="Unlock Admin to change them"><span className="mono">{extra.split("\n").join("  ")}</span></FormRow>}
          {!sglang && expertRefused && <Notice tone="error">{expertRefused}</Notice>}
          {draft.notApplied && draft.notApplied.length > 0 && (
            <FormRow label="AI also suggested (not applied)">
              <span className="mono muted">{draft.notApplied.join("  ")}</span>
            </FormRow>
          )}
          {checked ? (
            <FormRow label={<>Image <HelpTip topic="models.load.image" /></>} hint={`Started by its local id on ${namesOf(ordered)}`}>
              <select className="wide mono" aria-label="Image" value={imageRef} onChange={(e) => setPickedImage(e.target.value)}>
                {choices.map((c) => <option key={c.ref} value={c.ref}>{c.ref} — {imagePresence(c, nameOf)}</option>)}
                {!choices.some((c) => c.ref === imageRef) && <option value={imageRef}>{imageRef}</option>}
              </select>
            </FormRow>
          ) : (
            <FormRow label={<>Image <HelpTip topic="models.load.image" /></>} hint={sglang ? "Change it in Settings › Models" : "Pick the nodes to see where it is"}>
              <span className="mono">{sglang ? settings?.sglangImage || "lmsysorg/sglang:spark" : imageRef}</span>
            </FormRow>
          )}
        </FormGroup>

        {checked && (
          <FormGroup title={<>Before it starts <HelpTip topic="models.load.before-it-starts" /></>}>
            {share === null ? <div className="muted">Waiting for the memory plan: the nodes are read once the share is known.</div> : (
              <BeforeItStarts set={checks} problem={cleanProblem ?? pfProblem ?? (checks ? null : numbers)} share={share} port={portNum} drop={dropConsent} onDrop={(on) => setDropFor(on ? short : null)}
                nameOf={nameOf} lock={lockOf(!admin, adminState)} onClean={(p) => void cleanLeftovers(p)} onRecheck={numbers ? undefined : recheck} />
            )}
          </FormGroup>
        )}

        <FormGroup
          title={
            <>
              What will run{" "}
              <HelpTip topic="models.load.what-will-run" />
            </>
          }
        >
          {problem && problem !== recipeBlock && !expertRefused && !splitRefused && <Notice tone="error">{problem}</Notice>}
          {preview?.map((s, i) => (
            <div key={i} className="col gap-sm ai-step">
              <div className="muted"><span className="strong">{namesOf(s.node.split(", "))}</span> · {named(s.title)}</div>
              <CodeBlock text={s.command} maxHeight={180} label="Shell" />
            </div>
          ))}
          {plan && !numbers && plan.needs.kvBytesPerToken != null && (
            <p className="note">
              {fmtBytes(plan.needs.weightBytes)} of weights · KV cache {fmtBytes(plan.needs.kvBytesPerToken * (kvDtype.startsWith("fp8") ? 0.5 : 1))} per token
              {maxLen ? ` · ${fmtTokens(Number(maxLen))} context` : ""}
            </p>
          )}
        </FormGroup>
      </div>
    </div>
  );
}
