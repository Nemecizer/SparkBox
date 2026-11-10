// What an action on a node will break, worked out from the snapshot: the model deployments it
// belongs to (a vLLM head and its workers, a Compose project, or SparkBox's own
// `sparkbox.deployment` label), the endpoints and models they serve, the other containers
// that stop, open terminals, and whether you're away (VPN connected) or the node is the one
// SparkBox uses to tell whether the cluster is reachable. Confirmations show this list.
//
// It never promises a reload that won't happen: what brings a model back after its node or
// containers restart is Keep running, a restart policy, or you (Restart model…), and it says which.
import { getDeployments } from "./deployments";
import { endpointLabel, getEndpoints, isServing, type Endpoint } from "./endpoints";
import { getCluster } from "./store";
import type { ClusterDeployment, ClusterSnapshot, Container, NodeSnapshot } from "./types";

const ENGINE: Record<string, string> = { vllm: "vLLM", ollama: "Ollama", sglang: "SGLang", llamacpp: "llama.cpp" };

export type ImpactAction = "reboot" | "poweroff" | "start" | "stop" | "restart" | "kill" | "pause" | "unpause" | "remove";
export type ImpactKind = "deployment" | "endpoint" | "container" | "probe" | "remote" | "terminal" | "offline";

export interface ImpactItem {
  kind: ImpactKind;
  text: string;
  tone: "info" | "warn" | "crit";
}

export interface DeploymentMember {
  node: string;
  id: string;
  name: string;
  role: "head" | "worker" | "member";
  running: boolean;
}

export interface Deployment {
  /** The label value, Compose project, or "vllm" for unlabelled head/worker containers. */
  id: string;
  /** What it serves, when it can be told: "Llama-3.3-70B-Instruct". */
  model: string | null;
  nodes: string[];
  members: DeploymentMember[];
}

export interface ImpactReport {
  tier: "low" | "medium" | "high";
  items: ImpactItem[];
  deployments: Deployment[];
  endpoints: Endpoint[];
  /** Other running containers on the nodes (not part of a deployment above). */
  containers: { node: string; name: string; restarts: boolean }[];
  probeTarget: boolean;
  remote: boolean;
  /** SparkBox reaches the cluster through this node (VPN reachability), and you're away. */
  onlyPath: boolean;
  terminals: number;
}

export interface ImpactOptions {
  nodes: string[];
  action: ImpactAction;
  /** Containers the action is aimed at (container actions); the rest of the node is unaffected. */
  containerIds?: string[];
  endpoints?: Endpoint[];
  /** Open terminal tabs, by node. */
  terminals?: { node: string }[];
  /** The VPN is connected: you're away from the cluster. */
  remote?: boolean;
  /** `remote` is an assumption: the VPN status couldn't be read (fail closed). */
  remoteUnknown?: boolean;
}

/** The model a container serves, from its labels or command line. */
export function modelOf(c: Container): string | null {
  const label = c.labels?.["sparkbox.model"];
  if (label) return label;
  const words = c.command.split(/\s+/);
  const after = (flag: string) => {
    const i = words.indexOf(flag);
    return i >= 0 && words[i + 1] && !words[i + 1].startsWith("-") ? words[i + 1] : null;
  };
  const eq = words.find((w) => w.startsWith("--model="))?.slice(8) ?? null;
  return after("--model") ?? after("--model-path") ?? eq ?? after("serve") ?? after("--served-model-name");
}

export function shortModel(m: string): string {
  return m.split("/").filter(Boolean).pop() ?? m;
}

function groupOf(c: Container): string | null {
  return c.labels?.["sparkbox.deployment"] ?? c.project ?? (/^vllm-(head|worker)/.test(c.name) ? "vllm" : null);
}

function roleOf(c: Container): DeploymentMember["role"] {
  const r = c.labels?.["sparkbox.role"];
  if (r === "head" || r === "worker") return r;
  if (/head/.test(c.name)) return "head";
  if (/worker/.test(c.name)) return "worker";
  return "member";
}

/** A deployment from the backend's list (models/postmortem.rs) in this file's shape, its members as the snapshot has them now. */
function fromCluster(d: ClusterDeployment, snapshot: ClusterSnapshot): Deployment {
  const running = (node: string, id: string) => snapshot.nodes.find((n) => n.id === node)?.containers.find((c) => sameId(c.id, id))?.state === "running";
  return {
    id: d.id,
    model: d.model ?? d.served ?? null,
    nodes: [...d.nodes],
    members: d.members.map((m) => ({ node: m.node, id: m.id, name: m.container, role: m.role, running: running(m.node, m.id) })),
  };
}

/**
 * Every deployment in the cluster with its members: the backend's list (which also infers
 * deployments SparkBox didn't start, like 4 ranks of one script), then any group the labels,
 * a Compose project or vllm-head/worker names make that the list doesn't cover.
 */
export function deploymentsOf(snapshot: ClusterSnapshot, known: readonly ClusterDeployment[] = getDeployments()): Deployment[] {
  const groups = new Map<string, Deployment>();
  const covered = new Set<string>();
  for (const d of known) {
    // Only what the snapshot still shows (the list may be a round older).
    if (!d.members.some((m) => snapshot.nodes.find((n) => n.id === m.node)?.containers.some((c) => sameId(c.id, m.id)))) continue;
    groups.set(`rust:${d.id}`, fromCluster(d, snapshot));
    d.members.forEach((m) => covered.add(`${m.node}/${m.id}`));
  }
  for (const n of snapshot.nodes) {
    for (const c of n.containers) {
      const g = groupOf(c);
      if (!g || [...covered].some((k) => k.startsWith(`${n.id}/`) && sameId(k.slice(n.id.length + 1), c.id))) continue;
      const d = groups.get(g) ?? { id: g, model: null, nodes: [], members: [] };
      d.members.push({ node: n.id, id: c.id, name: c.name, role: roleOf(c), running: c.state === "running" });
      if (!d.nodes.includes(n.id)) d.nodes.push(n.id);
      d.model = d.model ?? (roleOf(c) !== "worker" ? modelOf(c) : null) ?? null;
      groups.set(g, d);
    }
  }
  // A Compose project of one container that serves nothing isn't a deployment worth naming.
  return [...groups.values()].filter((d) => d.members.length > 1 || d.model);
}

// ---- What a restart or shutdown of a node does to the models on it (r4-serving)

export interface PowerImpact {
  /** The model paragraph for the confirm: what pauses, stops or is untouched. Empty when nothing serves there. */
  paragraph: string;
  /** "Then restart llama-3.3-70b in order once cell3 is back": checked by default; runs model_restart_after. */
  followUp?: { label: string; endpointId: string };
}

/**
 * The model paragraph for Restart…, Shut down… and Wake on a node, and the one-click recovery to
 * offer with it: a multi-node model with a stage there that nothing restarts by itself (Keep
 * running off, or SparkBox didn't start it) can be restarted in order once the node is back.
 */
export function powerImpact(
  node: string,
  action: "reboot" | "poweroff" | "wake",
  ctx: { snapshot?: ClusterSnapshot | null; endpoints?: readonly Endpoint[] } = {},
): PowerImpact {
  const snapshot = ctx.snapshot ?? getCluster();
  const endpoints = ctx.endpoints ?? getEndpoints();
  const nameOf = (id: string) => snapshot?.nodes.find((n) => n.id === id)?.name ?? id;
  const nodeNames = (ids: string[]) => (ids.length <= 1 ? ids.map(nameOf).join("") : `${ids.slice(0, -1).map(nameOf).join(", ")} and ${nameOf(ids[ids.length - 1])}`);
  const name = nameOf(node);
  const on = endpoints.filter((e) => e.nodes.includes(node) || e.headNode === node);
  if (!on.length) return { paragraph: "" };
  const sentences: string[] = [];
  let followUp: PowerImpact["followUp"];
  for (const e of on) {
    const label = endpointLabel(e);
    const multi = e.nodes.length > 1;
    if (!isServing(e)) {
      if (e.health === "idle") sentences.push(`${label} has nothing in memory; it comes back with ${name}.`);
      continue;
    }
    const keep = e.supervisor?.enabled === true;
    const restartable = canRestartModel(e) && multi && !keep;
    const across = multi ? `${label} runs across ${nodeNames(e.nodes)}` : `${label} runs on ${name}`;
    const pause = action === "poweroff"
      ? `It stops until ${name} is woken`
      : action === "wake"
        ? `It's down while ${name} is off`
        : `It pauses while ${name} restarts (planned, so nobody is paged)`;
    const back = keep
      ? "Keep running restarts it in order once the node is back, while SparkBox is open."
      : restartable && !followUp && action !== "poweroff"
        ? "With the follow-up below selected, SparkBox restarts it in order once the node is back."
        : multi ? "It doesn't come back by itself."
        : e.managedBy === "external" ? "Restart it yourself once the node is back." : "";
    sentences.push(`${across}. ${pause}. ${back}`.trim());
    if (restartable && !followUp && action !== "poweroff") {
      followUp = { label: `Then restart ${label} in order once ${name} is back`, endpointId: e.id };
    }
  }
  return { paragraph: sentences.join(" "), ...(followUp ? { followUp } : {}) };
}

/** A vLLM load of SparkBox's own: only its launcher's `docker run` sets the recipe label, so SGLang's containers don't count. */
export function isNativeLoad(c: Pick<Container, "labels">): boolean {
  return !!c.labels?.["sparkbox.deployment"] && !!c.labels["sparkbox.recipe"];
}

/**
 * Order for acting on a deployment's containers. Other servers, and SGLang: workers stop before
 * the head, and the head starts first. SparkBox's vLLM loads (`native`) go the other way, as
 * Rust starts and unloads them: the head stops first, and starts once its workers are up.
 */
export function dependencyOrder<T extends { role: DeploymentMember["role"]; native?: boolean }>(members: T[], action: string): T[] {
  const rank = (m: T) => (m.role === "head" ? 0 : m.role === "member" ? 1 : 2);
  const headFirst = (list: T[]) => [...list].sort((a, b) => rank(a) - rank(b));
  const down = action === "stop" || action === "kill" || action === "pause";
  const theirs = headFirst(members.filter((m) => !m.native));
  const ours = headFirst(members.filter((m) => m.native));
  return [...(down ? theirs.reverse() : theirs), ...(down ? ours : ours.reverse())];
}

function names(snapshot: ClusterSnapshot, ids: string[]): string {
  const list = ids.map((id) => snapshot.nodes.find((n) => n.id === id)?.name ?? id);
  if (list.length <= 1) return list.join("");
  return `${list.slice(0, -1).join(", ")} and ${list[list.length - 1]}`;
}

const sameId = (a: string, b: string) => !!a && !!b && (a === b || a.startsWith(b) || b.startsWith(a));

/** The endpoint a deployment serves: by SparkBox's deployment id, else by its containers (id, or head name on the head node). */
export function endpointOf(d: Deployment, endpoints: readonly Endpoint[]): Endpoint | null {
  return endpoints.find((e) =>
    (e.deploymentId && e.deploymentId === d.id)
    || e.containerIds.some((id) => d.members.some((m) => sameId(m.id, id)))
    || d.members.some((m) => m.node === e.headNode && !!e.containerNames?.includes(m.name))) ?? null;
}

/** Keep running (r2-ai's supervisor) for the endpoint a deployment serves: true, false or unknown. */
function keepRunning(d: Deployment, endpoints: Endpoint[]): boolean | null {
  const sup = endpointOf(d, endpoints)?.supervisor;
  return typeof sup?.enabled === "boolean" ? sup.enabled : null;
}

/** The name a deployment's model goes by: its endpoint's ("llama-3.3-70b"), else the head's path. */
export function modelName(d: Deployment, endpoints: readonly Endpoint[]): string {
  const e = endpointOf(d, endpoints);
  return e ? endpointLabel(e) : shortModel(d.model ?? d.id);
}

/** Every container of the deployment comes back by itself after a reboot (restart policy). */
function selfRestarting(d: Deployment, snapshot: ClusterSnapshot | undefined): boolean {
  if (!snapshot) return false;
  return d.members.every((m) => {
    const c = snapshot.nodes.find((n) => n.id === m.node)?.containers.find((x) => x.id === m.id);
    return !!c && restarts(c);
  });
}

/**
 * What brings a deployment back after its node restarts. Only what will happen: Keep running
 * (a SparkBox load, while SparkBox is open), a restart policy (one node), or you.
 */
export function reloadSentence(d: Deployment, endpoints: Endpoint[], snapshot?: ClusterSnapshot): string {
  const e = endpointOf(d, endpoints);
  const model = modelName(d, endpoints);
  if (keepRunning(d, endpoints) === true) return "Keep running restarts it in order once the node is back, while SparkBox is open on this Mac.";
  if (d.nodes.length === 1 && selfRestarting(d, snapshot)) return "Its container restarts by itself when the node is back (restart policy) and loads the model again.";
  if (e?.managedBy === "external") return `SparkBox didn't start this model: once the node is back, restart it yourself (Restart ${model}…).`;
  if (e) return `Keep running is off: once the node is back, restart it (Restart ${model}…).`;
  return `Nothing restarts it by itself: once the node is back, start it again.`;
}

function restarts(c: Container): boolean {
  return ["always", "unless-stopped"].includes(c.restartPolicy ?? "");
}

// ---- Restarting containers that a model is made of

/** A deployment that a selection of containers touches, and how much of it. */
export interface PipelineCover {
  deployment: Deployment;
  endpoint: Endpoint | null;
  /** "llama-3.3-70b". */
  model: string;
  /** Members in the selection. */
  selected: DeploymentMember[];
  /** Every member is selected. */
  all: boolean;
  /** A head and workers on more than one node: restarting them one by one breaks it. */
  pipeline: boolean;
}

/** The deployments a selection of containers belongs to, with how much of each it covers. */
export function pipelineCoverage(snapshot: ClusterSnapshot, selection: { node: string; id: string }[], endpoints: readonly Endpoint[]): PipelineCover[] {
  const picked = (m: DeploymentMember) => selection.some((s) => s.node === m.node && s.id === m.id);
  return deploymentsOf(snapshot)
    .filter((d) => d.members.some(picked))
    .map((d) => {
      const selected = d.members.filter(picked);
      return {
        deployment: d,
        endpoint: endpointOf(d, endpoints),
        model: modelName(d, endpoints),
        selected,
        all: selected.length === d.members.length,
        pipeline: d.nodes.length > 1 && d.members.some((m) => m.role === "head"),
      };
    });
}

/** What a container restart does to the model it serves, and the better way when there is one. */
export interface RestartAdvice {
  items: ImpactItem[];
  /** The model restart in order, offered when a pipeline is involved and SparkBox can do it. */
  model: { endpoint: Endpoint; label: string } | null;
  /** The selection is a whole pipeline: the model restart is the default button. */
  preferModel: boolean;
  /** The containers' own button: "Restart vllm-worker", or "Restart 3 containers anyway" beside the model restart. */
  containersLabel: string;
}

/** Whether SparkBox can restart the endpoint in order (it started it, or knows its containers). */
export function canRestartModel(e: Endpoint): boolean {
  return !!e.deploymentId || (e.managedBy === "external" && e.source !== "manual" && (e.containerIds.length > 0 || (e.containerNames?.length ?? 0) > 0));
}

/** What happens to a pipeline after its containers were restarted one by one. */
function afterContainerRestart(cover: PipelineCover): string {
  const e = cover.endpoint;
  if (e?.supervisor?.enabled) return "Keep running then restarts it in order, while SparkBox is open on this Mac.";
  if (e?.managedBy === "external") return `SparkBox didn't start this model: after the restart, restart it yourself (Restart ${cover.model}…).`;
  if (e) return `Keep running is off: after the restart, restart it yourself (Restart ${cover.model}…).`;
  return "Nothing restarts it by itself: after the restart, start it again in order.";
}

/**
 * Restarting containers: a whole pipeline defaults to restarting the model in order
 * ("Restart 3 containers anyway" is the other button); part of one warns what breaks and offers
 * the model restart; a single-node model says it's offline until it has loaded again.
 */
export function restartAdvice(snapshot: ClusterSnapshot, selection: { node: string; id: string; name: string }[], endpoints: readonly Endpoint[]): RestartAdvice {
  const covers = pipelineCoverage(snapshot, selection, endpoints);
  const items: ImpactItem[] = [];
  const pipelines = covers.filter((c) => c.pipeline);
  const one = selection.length === 1 ? selection[0].name : null;
  for (const c of covers) {
    const d = c.deployment;
    if (!c.pipeline) {
      items.push({ kind: "deployment", tone: "warn", text: `${c.model} on ${names(snapshot, d.nodes)} stops answering until it has loaded again.` });
    } else if (c.all) {
      const head = d.members.find((m) => m.role === "head");
      const workers = [...new Set(d.members.filter((m) => m.role !== "head").map((m) => m.node))];
      items.push({
        kind: "deployment",
        tone: "crit",
        text: `These ${d.members.length} containers are ${c.model}'s pipeline (head on ${names(snapshot, head ? [head.node] : [])}, workers on ${names(snapshot, workers)}). Restarted as containers, the workers exit when the head restarts and ${c.model} stops answering.`,
      });
      items.push({ kind: "deployment", tone: "info", text: afterContainerRestart(c) });
    } else {
      items.push({ kind: "deployment", tone: "crit", text: `${c.model} runs across ${names(snapshot, d.nodes)}. Restarting the head makes the workers exit, and restarting a worker stops the model until the head restarts.` });
      items.push({ kind: "deployment", tone: "info", text: afterContainerRestart(c) });
    }
  }
  // One model restart can replace the containers' restart: offer it when there's exactly one pipeline.
  const target = pipelines.length === 1 && pipelines[0].endpoint && canRestartModel(pipelines[0].endpoint) ? pipelines[0] : null;
  const preferModel = !!target && target.all && covers.length === 1;
  if (preferModel) {
    const last = items.findIndex((i) => i.text === afterContainerRestart(target!));
    if (last >= 0) items[last] = { kind: "deployment", tone: "info", text: "The default action restarts this model in order. The alternative restarts the selected containers only." };
  }
  const count = `${selection.length} container${selection.length === 1 ? "" : "s"}`;
  return {
    items,
    model: target ? { endpoint: target.endpoint!, label: `Restart ${target.model} in order` } : null,
    preferModel,
    containersLabel: preferModel ? `Restart ${count} anyway` : one ? `Restart ${one}` : `Restart ${count}`,
  };
}

export function impactOf(snapshot: ClusterSnapshot, opts: ImpactOptions): ImpactReport {
  const { nodes, action } = opts;
  if (action === "start" || action === "unpause") {
    // Bringing things up breaks nothing.
    return { tier: "low", items: [], deployments: [], endpoints: [], containers: [], probeTarget: false, remote: !!opts.remote, onlyPath: false, terminals: 0 };
  }
  const targets = snapshot.nodes.filter((n) => nodes.includes(n.id));
  const power = action === "reboot" || action === "poweroff";
  const hitId = (node: string, id: string) => (opts.containerIds ? opts.containerIds.includes(id) : nodes.includes(node));
  const hit = (n: NodeSnapshot, c: Container) => hitId(n.id, c.id);
  const items: ImpactItem[] = [];
  const deployments = deploymentsOf(snapshot).filter((d) => d.members.some((m) => m.running && hitId(m.node, m.id)));
  const nodeNames = names(snapshot, nodes);
  const what = action === "poweroff" ? "is off" : action === "reboot" ? "is back" : action === "remove" ? "is recreated" : "is running again";

  for (const d of deployments) {
    const model = d.model ? shortModel(d.model) : d.id;
    const multi = d.nodes.length > 1;
    const reload = action === "reboot" ? ` ${reloadSentence(d, opts.endpoints ?? [], snapshot)}` : "";
    items.push({
      kind: "deployment",
      tone: "crit",
      text: multi
        ? `${model} (${d.id}) runs across ${names(snapshot, d.nodes)}. It stops answering ${power ? `while ${nodeNames} ${action === "poweroff" ? "is off" : "restarts"}` : "until all its parts run again, in order"}.${reload}`
        : `${model} (${d.id}) on ${names(snapshot, d.nodes)} is unavailable until it ${what}.${reload}`,
    });
  }

  const endpoints = (opts.endpoints ?? []).filter((e) => e.nodes.some((n) => nodes.includes(n)) && (!opts.containerIds || e.containerIds.some((id) => opts.containerIds!.includes(id))));
  for (const e of endpoints) {
    items.push({ kind: "endpoint", tone: isServing(e) ? "warn" : "info", text: `${endpointLabel(e)} (${ENGINE[e.engine] ?? e.engine}, ${e.url}) stops answering.` });
  }

  const inDeployment = new Set(deployments.flatMap((d) => d.members.map((m) => `${m.node}/${m.id}`)));
  const containers = targets.flatMap((n) =>
    n.containers.filter((c) => c.state === "running" && hit(n, c) && !inDeployment.has(`${n.id}/${c.id}`)).map((c) => ({ node: n.id, name: c.name, restarts: restarts(c) })),
  );
  if (containers.length) {
    const back = containers.filter((c) => c.restarts).map((c) => c.name);
    const stay = containers.filter((c) => !c.restarts).map((c) => c.name);
    const count = `${containers.length} other running container${containers.length === 1 ? "" : "s"}`;
    if (power && action === "reboot") {
      items.push({
        kind: "container",
        tone: "info",
        text: `${count}: ${back.length ? `${back.join(", ")} restart${back.length === 1 ? "s" : ""} by ${back.length === 1 ? "itself" : "themselves"}` : ""}${back.length && stay.length ? "; " : ""}${stay.length ? `${stay.join(", ")} stay${stay.length === 1 ? "s" : ""} stopped (restart policy: no)` : ""}.`,
      });
    } else if (!opts.containerIds) {
      items.push({ kind: "container", tone: "info", text: `${count} stop: ${containers.map((c) => c.name).join(", ")}.` });
    }
  }

  const probeNode = snapshot.nodes[0]?.id;
  const probeTarget = power && !!probeNode && nodes.includes(probeNode);
  const remote = !!opts.remote;
  const onlyPath = probeTarget && remote;
  if (probeTarget) {
    items.push({
      kind: "probe",
      tone: remote ? "crit" : "warn",
      text: `SparkBox checks whether it can reach the cluster through ${names(snapshot, [probeNode!])}. While it's down, VPN Auto mode may decide the cluster is unreachable.`,
    });
  }
  const away = opts.remoteUnknown ? "SparkBox can't tell whether you're away, so it assumes you are" : "You're away (VPN connected)";
  if (power && remote) {
    items.push(
      action === "poweroff"
        ? { kind: "remote", tone: "crit", text: `${nodeNames} stays off until someone presses its power button. ${away}, so only Wake-on-LAN could bring it back.` }
        : { kind: "remote", tone: "warn", text: `${away}. If ${nodeNames} doesn't come back, nobody is there to press its power button.` },
    );
  } else if (action === "poweroff") {
    items.push({ kind: "offline", tone: "warn", text: `${nodeNames} stays off until it's woken (Wake) or someone presses its power button.` });
  }

  const terminals = (opts.terminals ?? []).filter((t) => nodes.includes(t.node)).length;
  if (terminals && (power || !opts.containerIds)) {
    items.push({ kind: "terminal", tone: "info", text: `${terminals} open terminal tab${terminals === 1 ? "" : "s"} on ${nodeNames} disconnect${terminals === 1 ? "s" : ""}.` });
  }

  const multiNode = deployments.some((d) => d.nodes.length > 1);
  const tier: ImpactReport["tier"] =
    (action === "poweroff" && remote) || (power && multiNode) || onlyPath
      ? "high"
      : power || items.length > 0 || action === "remove" || action === "kill"
        ? "medium"
        : "low";
  return { tier, items, deployments, endpoints, containers, probeTarget, remote, onlyPath, terminals };
}

export interface FleetImpactOptions {
  nodes: string[];
  endpoints?: Endpoint[];
  remote?: boolean;
  /** The command can destroy data or take nodes down: what's on the nodes is at stake. */
  destructive?: boolean;
}

/**
 * What's on the nodes a Fleet command runs on, for its confirmation: the models and endpoints
 * they serve, whether SparkBox checks the cluster through one of them, and whether you're away.
 */
export function fleetImpact(snapshot: ClusterSnapshot, opts: FleetImpactOptions): ImpactItem[] {
  const { nodes } = opts;
  const tone = opts.destructive ? "crit" : "warn";
  const items: ImpactItem[] = [];
  const deployments = deploymentsOf(snapshot).filter((d) => d.members.some((m) => m.running && nodes.includes(m.node)));
  for (const d of deployments) {
    const model = d.model ? shortModel(d.model) : d.id;
    const on = d.nodes.filter((n) => nodes.includes(n));
    items.push({ kind: "deployment", tone, text: `${model} (${d.id}) runs on ${names(snapshot, on)}${d.nodes.length > on.length ? ` and ${names(snapshot, d.nodes.filter((n) => !on.includes(n)))}` : ""}.` });
  }
  for (const e of (opts.endpoints ?? []).filter((e) => e.nodes.some((n) => nodes.includes(n)))) {
    items.push({ kind: "endpoint", tone: "info", text: `${endpointLabel(e)} (${ENGINE[e.engine] ?? e.engine}, ${e.url}) is served from ${names(snapshot, e.nodes.filter((n) => nodes.includes(n)))}.` });
  }
  const probeNode = snapshot.nodes[0]?.id;
  if (probeNode && nodes.includes(probeNode)) {
    items.push({ kind: "probe", tone: opts.remote ? "crit" : "info", text: `SparkBox checks whether it can reach the cluster through ${names(snapshot, [probeNode])}.` });
  }
  if (opts.remote) {
    items.push({ kind: "remote", tone: opts.destructive ? "crit" : "warn", text: "You're away (VPN connected): if a node stops answering, nobody is there to fix it." });
  }
  return items;
}
