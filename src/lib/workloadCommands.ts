// Container and service actions, in one place for every way in: the Containers and
// Services windows, their context menus and the ⌘K palette ("restart vllm-head",
// "logs open-webui", "restart nvidia-dgx-telemetry on cell1"). Each action confirms
// with what it takes down, unlocks Admin mode if it has to, and runs once: asking
// again while the same action is still being confirmed or running joins it instead
// of queueing a second dialog or a second task.
import { createElement as h, useMemo } from "react";
import { FileSearch, FileText, FolderOpen, Gauge, Play, Power, RotateCcw, ScrollText, Server, SquareTerminal, Sunrise } from "lucide-react";
import { StopIcon } from "../components/Icon";
import { requestAdmin } from "./admin";
import { api, type ContainerAction, type ServiceAction } from "./api";
import { useCommands, type Command } from "./commands";
import { getEndpoints } from "./endpoints";
import { describeError } from "./errors";
import { dependencyOrder, deploymentsOf, impactOf, isNativeLoad, restartAdvice, shortModel, type DeploymentMember } from "./impact";
import { opsApi } from "./opsApi";
import { ImpactList, requestPower } from "./power";
import { confirmAction, confirmChoice, singleFlight } from "./session";
import { getCluster, useCluster } from "./store";
import { waitTask, withOrigin } from "./tasks";
import { openTerminal } from "./terminals";
import { toast } from "./toasts";
import type { ClusterSnapshot, Container, NodeSnapshot, Process } from "./types";
import { POWER } from "./vocabulary";
import { openWindow } from "../wm/WindowManager";
import { restartModel } from "../windows/models/endpointActions";
import { openOnNode } from "../windows/NodeDetail";
import { getDeployments, useDeployments, deploymentLabel, type Deployment } from "./deployments";
import { explainDeployment } from "../windows/models/DeploymentSummary";

export const CONTAINER_ACTION_LABEL: Record<ContainerAction, string> = {
  start: "Start", stop: "Stop", restart: "Restart", kill: "Force-stop", pause: "Pause", unpause: "Resume", remove: "Remove",
};

export const SERVICE_ACTION_LABEL: Record<ServiceAction, string> = {
  start: "Start", stop: "Stop", restart: "Restart", reload: "Reload", enable: "Enable", disable: "Disable",
  "reset-failed": "Reset failed", mask: "Mask", unmask: "Unmask", "daemon-reload": "Reload unit files",
};

/**
 * Why an action doesn't apply to these containers, naming their state: "Already running",
 * "Stop it first", "It isn't paused".
 */
export function notApplicable(action: ContainerAction, cs: Container[]): string {
  const one = cs.length === 1;
  const all = (st: string) => cs.length > 0 && cs.every((c) => c.state === st);
  switch (action) {
    case "start": return all("paused") ? (one ? "It's paused: resume it" : "They're paused: resume them") : one ? "Already running" : "All already running";
    case "stop": case "kill": case "pause":
      return all("paused") && action === "pause" ? (one ? "Already paused" : "All already paused") : one ? `It isn't running (${cs[0].state})` : "None of them is running";
    case "restart": return all("paused") ? (one ? "It's paused: resume it first" : "They're paused: resume them first") : one ? `It's ${cs[0].state}` : "None of them is running or stopped";
    case "unpause": return one ? "It isn't paused" : "None of them is paused";
    case "remove": return one ? "Stop it first" : "Stop them first";
  }
}

/** Which containers an action applies to. */
export function applicable(action: ContainerAction, c: Container): boolean {
  switch (action) {
    case "start": return c.state === "exited" || c.state === "created";
    case "stop": case "kill": case "pause": return c.state === "running";
    case "restart": return c.state === "running" || c.state === "exited";
    case "unpause": return c.state === "paused";
    case "remove": return c.state === "exited" || c.state === "created" || c.state === "dead";
  }
}

// ---- Each node's whole process list
// Each second's snapshot carries a node's 15 busiest processes and the listeners' owners
// (processCount says how many it reported); Workloads asks for the whole list (processes_get)
// every 5 s while it's open and keeps it here. The snapshot's rows are the fresher: the fetched
// list only fills in the processes the snapshot left out.

interface Fetched {
  processes: Process[];
  atMs: number;
}
/** What processes_get last gave, by data source and node. */
const fetched = new Map<string, Fetched>();
/** A fetched list older than this gives way to the snapshot's again. */
export const FETCHED_FOR_MS = 15_000;

/** Keeps a node's whole process list as processes_get gave it. */
export function rememberProcesses(demo: boolean, nodeId: string, processes: Process[], atMs = Date.now()) {
  fetched.set(`${demo ? "demo" : "live"}:${nodeId}`, { processes, atMs });
}

/**
 * A node's processes to list: the snapshot's, when it has them all; when it's cut, its rows
 * (a second old) with the rest of the whole list Workloads fetched in the last 15 s.
 */
export function processesOf(cluster: Pick<ClusterSnapshot, "demo">, n: Pick<NodeSnapshot, "id" | "processes" | "processCount">, now = Date.now()): Process[] {
  if ((n.processCount ?? 0) <= n.processes.length) return n.processes;
  const f = fetched.get(`${cluster.demo ? "demo" : "live"}:${n.id}`);
  if (!f || now - f.atMs >= FETCHED_FOR_MS) return n.processes;
  const fresh = new Map(n.processes.map((p) => [p.pid, p]));
  const listed = new Set(f.processes.map((p) => p.pid));
  return [...f.processes.map((p) => fresh.get(p.pid) ?? p), ...n.processes.filter((p) => !listed.has(p.pid))];
}

/** The snapshot's list is cut (to the 15 busiest and the listeners' owners) and nothing fuller was fetched. */
export function processesCut(cluster: Pick<ClusterSnapshot, "demo">, n: Pick<NodeSnapshot, "id" | "processes" | "processCount">, now = Date.now()): boolean {
  return processesOf(cluster, n, now) === n.processes && (n.processCount ?? 0) > n.processes.length;
}

// ---- Nodes that went offline keep their rows

interface Known {
  node: NodeSnapshot;
  containers: Container[];
  processes: Process[];
  atMs: number;
}
/** What each node last reported while online, by data source and node. */
const lastKnown = new Map<string, Known>();

export interface ListedNode {
  node: NodeSnapshot;
  /** Offline: these are the containers and processes it last reported; their state is unknown. */
  stale: boolean;
  /** When it last reported them (Unix ms), while stale. */
  knownAtMs: number | null;
}

/**
 * The nodes as Containers and Workloads list them. An online node is as it is; an offline one
 * keeps the containers and processes it last reported, marked stale, so its rows stay (greyed,
 * "unknown · cell3 offline") instead of vanishing while it's away.
 */
export function withLastKnown(cluster: Pick<ClusterSnapshot, "nodes" | "demo" | "tsMs">): ListedNode[] {
  const prefix = `${cluster.demo ? "demo" : "live"}:`;
  const nodes = [...cluster.nodes];
  for (const [key, known] of lastKnown) {
    if (key.startsWith(prefix) && !nodes.some((n) => n.id === known.node.id)) nodes.push({ ...known.node, status: "offline" });
  }
  return nodes.map((n) => {
    const key = `${cluster.demo ? "demo" : "live"}:${n.id}`;
    if (n.status === "online" && (!n.stale || (n.frameAgeMs != null && n.frameAgeMs <= 10_000))) {
      // The whole list where Workloads fetched it, else the snapshot's busiest and listeners' owners.
      const processes = processesOf(cluster, n);
      lastKnown.set(key, { node: n, containers: n.containers, processes, atMs: cluster.tsMs });
      return { node: processes === n.processes ? n : { ...n, processes }, stale: false, knownAtMs: null };
    }
    const k = lastKnown.get(key);
    const containers = n.containers.length ? n.containers : k?.containers ?? [];
    const processes = n.processes.length ? n.processes : k?.processes ?? [];
    return { node: { ...n, containers, processes }, stale: true, knownAtMs: k?.atMs ?? null };
  });
}

/** "unknown · cell3 offline": the state of a row whose node isn't reporting. */
export const staleState = (node: Pick<NodeSnapshot, "name" | "status">) => `unknown · ${node.name} ${node.status === "online" ? "not reporting" : "offline"}`;

/** Test helper. */
export function _forgetLastKnownForTests() {
  lastKnown.clear();
  fetched.clear();
}

/** cell1, cell2, cell3 → "cell1–cell3"; otherwise the names, comma-separated. */
export function rangeOf(snapshot: ClusterSnapshot, ids: string[]): string {
  const names = ids.map((id) => snapshot.nodes.find((n) => n.id === id)?.name ?? id);
  if (names.length <= 2) return names.join(", ");
  const m = names.map((n) => n.match(/^(.*?)(\d+)$/));
  const same = m.every((x) => x && x[1] === m[0]![1]);
  const nums = m.map((x) => Number(x?.[2]));
  const consecutive = same && nums.every((n, i) => i === 0 || n === nums[i - 1] + 1);
  return consecutive ? `${names[0]}–${names[names.length - 1]}` : names.join(", ");
}

export interface ContainerTarget {
  node: NodeSnapshot;
  c: Container;
}

/** The key two requests for the same action share: "restart:cell1:3f2a…". */
export function containerActionKey(action: ContainerAction, targets: { node: { id: string }; c: { id: string } }[]): string {
  return `${action}:${targets.map((t) => `${t.node.id}:${t.c.id}`).sort().join(",")}`;
}

/**
 * Starts, stops, restarts (…) containers: one confirmation that says what goes down, Admin
 * mode, then one task for the lot, in dependency order (dependencyOrder: SparkBox's vLLM loads
 * and other servers go opposite ways). `origin` is the window it came from, for its toasts.
 */
export function runContainerAction(action: ContainerAction, targets: ContainerTarget[], opts: { origin?: string } = {}): Promise<void> {
  const key = containerActionKey(action, targets);
  return singleFlight(key, () => containerAction(action, targets, key, opts.origin));
}

async function containerAction(action: ContainerAction, targets: ContainerTarget[], key: string, origin?: string): Promise<void> {
  const cluster = getCluster();
  if (!cluster) return;
  const label = CONTAINER_ACTION_LABEL[action];
  const todo = targets.filter((t) => applicable(action, t.c));
  const skipped = targets.length - todo.length;
  if (!todo.length) {
    toast({
      kind: "info",
      title: `Nothing to ${label.toLowerCase()}`,
      body: action === "remove" ? "Only stopped containers can be removed." : "None of the selected containers is in a state for that.",
    });
    return;
  }
  const deployments = deploymentsOf(cluster);
  const roleOf = (t: ContainerTarget): DeploymentMember["role"] =>
    deployments.flatMap((d) => d.members).find((m) => m.node === t.node.id && m.id === t.c.id)?.role ?? "member";
  const ordered = dependencyOrder(todo.map((t) => ({ ...t, role: roleOf(t), native: isNativeLoad(t.c) })), action);
  const nodes = [...new Set(ordered.map((r) => r.node.id))];
  const impact = impactOf(cluster, { nodes, action, containerIds: ordered.map((r) => r.c.id), endpoints: getEndpoints() });
  const what = ordered.length === 1 ? ordered[0].c.name : `${ordered.length} containers`;
  const title = `${label} ${what}`;
  let removeVolumes = false;
  const quiet = (action === "start" || action === "unpause") && impact.items.length === 0;
  // A restart of a model's containers says what it does to the model, and offers the model restart in order.
  const advice = action === "restart" ? restartAdvice(cluster, ordered.map((r) => ({ node: r.node.id, id: r.c.id, name: r.c.name })), getEndpoints()) : null;
  if (!quiet) {
    const served = impact.deployments.filter((d) => d.model);
    const until = action === "pause" ? "resumed" : action === "remove" ? "deployed again" : "started again";
    const lead = advice?.items.length
      ? null
      : served.length
        ? `${ordered.length === 1 ? "It serves" : "They serve"} ${served.map((d) => shortModel(d.model!)).join(" and ")}, which ${served.length === 1 ? "is" : "are"} unavailable until ${until}.`
        : action === "remove" ? "The containers are deleted; their images stay." : null;
    const items = advice?.items.length
      ? [...advice.items, ...impact.items.filter((i) => i.kind !== "deployment" && i.kind !== "endpoint")]
      : impact.items.filter((i) => !(served.length && i.kind === "deployment"));
    const body = h(
      "div",
      { className: "ops-confirm" },
      lead && h("p", null, lead),
      h(ImpactList, { items }),
      advice?.model && h("p", { className: "muted" }, `${advice.model.label} ${advice.model.endpoint.deploymentId ? "stops the head, then the workers, starts the workers, then the head," : "stops the workers, restarts the head, starts the workers again"} and checks it answers.`),
      ordered.length > 1 && h("p", { className: "muted" }, `Container order: ${ordered.map((r) => `${r.c.name} (${r.node.name})`).join(" → ")}.`),
      skipped > 0 && h("p", { className: "muted" }, `${skipped} selected container${skipped === 1 ? " isn't" : "s aren't"} in a state for this and ${skipped === 1 ? "is" : "are"} left alone.`),
      action === "remove" && h(
        "label",
        { className: "check" },
        h("input", { type: "checkbox", onChange: (e: { target: { checked: boolean } }) => (removeVolumes = e.target.checked) }),
        " Also remove anonymous volumes",
      ),
    );
    const ask = { title: `${title} on ${rangeOf(cluster, nodes)}?`, body, dedupeKey: key };
    if (advice?.model) {
      // Two ways to go ahead: the model restart (the default when the selection is the whole pipeline) or the containers.
      const choice = await confirmChoice({
        ...ask,
        confirmLabel: advice.preferModel ? advice.model.label : advice.containersLabel,
        alternativeLabel: advice.preferModel ? advice.containersLabel : advice.model.label,
        danger: !advice.preferModel,
      });
      if (!choice) return;
      // The model restart in order (r3-serving's), already confirmed here.
      if ((choice === "confirm") === advice.preferModel) {
        await restartModel(advice.model.endpoint, { origin, confirmed: true });
        return;
      }
    } else {
      const ok = await confirmAction({
        ...ask,
        confirmLabel: advice ? advice.containersLabel : label,
        danger: action !== "start" && action !== "unpause",
        requireTyping: impact.tier === "high" && action === "remove" ? cluster.nodes.find((n) => n.id === nodes[0])?.name : undefined,
      });
      if (!ok) return;
    }
  }
  if (!(await requestAdmin(ordered.length === 1 ? "container_action" : "containers_bulk"))) return;
  // One gesture: the task the backend starts reports under the same op id as an error here.
  await withOrigin(origin, async (opId) => {
    try {
      if (ordered.length === 1) {
        await api.containerAction(ordered[0].node.id, ordered[0].c.id, action, removeVolumes);
      } else {
        await opsApi.containersBulk(ordered.map((r) => ({ node: r.node.id, id: r.c.id, name: r.c.name })), action, removeVolumes, title);
      }
    } catch (e) {
      showOpError(e, title, opId);
    }
  }, (t) => t.nodes.some((n) => nodes.includes(n)));
}

/**
 * An action's error, as the same toast its task's result lands in (the backend runs most
 * actions as a task, and both report the failure).
 */
export function showOpError(e: unknown, action: string, opId: string) {
  const d = describeError(e, { action });
  if (d.kind === "cancelled") return;
  toast({ kind: "error", opId, title: d.title, body: d.message, actions: d.actions.length ? d.actions : undefined });
}

/** The key two requests for the same service action share: "restart:cell1,cell2:foo.service". */
export function serviceActionKey(action: ServiceAction, nodes: string[], unit: string): string {
  return `${action}:${[...nodes].sort().join(",")}:${unit}`;
}

/**
 * Runs a systemd action on one or more nodes, one at a time: asks first unless the backend
 * rates it harmless, unlocks Admin mode, then runs it as a task. Resolves once it's done.
 */
export function runServiceAction(nodes: string[], unit: string, action: ServiceAction, opts: { description?: string; origin?: string } = {}): Promise<void> {
  const key = serviceActionKey(action, nodes, unit);
  return singleFlight(key, () => serviceAction(nodes, unit, action, key, opts));
}

async function serviceAction(nodes: string[], unit: string, action: ServiceAction, key: string, opts: { description?: string; origin?: string }): Promise<void> {
  if (!nodes.length) return;
  const label = SERVICE_ACTION_LABEL[action];
  const name = (id: string) => getCluster()?.nodes.find((n) => n.id === id)?.name ?? id;
  let impact = null;
  try {
    impact = await opsApi.serviceImpact(nodes, unit, action);
  } catch {
    impact = null;
  }
  if (!impact || impact.tier !== "low") {
    const ok = await confirmAction({
      title: impact?.title ?? `${label} ${unit} on ${nodes.map(name).join(", ")}?`,
      body: h(
        "div",
        { className: "ops-confirm" },
        h(
          "ul",
          { className: "ops-impact" },
          (impact?.lines ?? [opts.description ?? unit]).map((l, i) =>
            h(
              "li",
              { key: i, className: `ops-impact-item ${impact?.tier === "high" ? "crit" : "warn"}` },
              h("span", { className: "ops-impact-mark", "aria-hidden": true }, impact?.tier === "high" ? "!" : "▲"),
              h("span", null, l),
            ),
          ),
        ),
      ),
      confirmLabel: impact?.confirmLabel ?? label,
      danger: impact?.tier === "high" || ["stop", "disable", "mask"].includes(action),
      requireTyping: impact?.requireTyping ?? undefined,
      dedupeKey: key,
    });
    if (!ok) return;
  }
  if (!(await requestAdmin(nodes.length === 1 ? "service_action" : "services_apply"))) return;
  await withOrigin(opts.origin, async (opId) => {
    try {
      if (nodes.length === 1) {
        // The task's own notice says how it went; a failure here merges into it.
        await api.serviceAction(nodes[0], unit, action);
      } else {
        await waitTask(await opsApi.servicesApply(nodes, unit, action));
      }
    } catch (e) {
      showOpError(e, unit ? `${label} ${unit}` : label, opId);
    }
  }, (t) => t.kind === "service" && t.nodes.some((n) => nodes.includes(n)));
}

/** A container's log, followed in a Terminal tab. */
export function openContainerLogs(node: NodeSnapshot, c: Container) {
  openTerminal(node.id, { kind: "containerLogs", container: c.id }, { title: `${c.name} · ${node.name}` });
  openWindow("terminal");
}

/** A shell inside a running container (Admin mode, like any shell). */
export async function openContainerShell(node: NodeSnapshot, c: Container) {
  if (!(await requestAdmin("terminal_open"))) return;
  openTerminal(node.id, { kind: "containerShell", container: c.id }, { title: c.name });
  openWindow("terminal");
}

export function inspectContainer(node: NodeSnapshot, c: Container) {
  openWindow("inspect", { node: node.id, id: c.id, name: c.name });
}

/** Palette commands for every node: Open, Terminal, Files, Restart…, Shut down…, Wake, DGX Dashboard. */
export function nodeCommands(cluster: ClusterSnapshot | null): Command[] {
  if (!cluster) return [];
  return cluster.nodes.flatMap((n): Command[] => {
    const entity = { kind: "node", id: n.id, name: n.name };
    const up = n.status === "online" ? (true as const) : `${n.name} isn't connected`;
    const sub = n.status === "online" ? n.host : `${n.host} · ${n.powerState === "off" ? "off" : "offline"}`;
    return [
      { id: `node:${n.id}`, title: n.name, group: "Nodes", subtitle: `${sub} · open node`, icon: Server, entity, verbs: ["open"], keywords: [n.host], run: () => openWindow("node", { id: n.id }) },
      {
        id: `node:${n.id}:terminal`, title: `Terminal on ${n.name}`, group: "Nodes", subtitle: "New SSH shell tab", icon: SquareTerminal, entity,
        keywords: ["ssh", "shell"], needsAdmin: true,
        run: () => {
          openTerminal(n.id);
          openWindow("terminal");
        },
      },
      { id: `node:${n.id}:files`, title: `Files on ${n.name}`, group: "Nodes", subtitle: "Browse its disk", icon: FolderOpen, entity, keywords: ["sftp", "browse"], run: () => openWindow("files", { place: n.id }) },
      {
        id: `node:${n.id}:restart`, title: `${POWER.restart.replace(/…$/, "")} ${n.name}…`, group: "Nodes", subtitle: "Asks first, and lists what goes down with it",
        icon: RotateCcw, entity, verbs: ["restart"], needsAdmin: true, when: () => up, run: () => requestPower(n.id, "reboot"),
      },
      {
        id: `node:${n.id}:shutdown`, title: `${POWER.shutdown.replace(/…$/, "")} ${n.name}…`, group: "Nodes", subtitle: "Asks first; it stays off until woken",
        icon: Power, entity, verbs: ["shut"], needsAdmin: true, when: () => up, run: () => requestPower(n.id, "poweroff"),
      },
      {
        id: `node:${n.id}:wake`, title: `${POWER.wake} ${n.name}`, group: "Nodes", subtitle: "Wake-on-LAN, then waits for it to answer", icon: Sunrise, entity,
        verbs: ["wake", "power on", "start"], needsAdmin: true, when: () => (n.status === "online" ? `${n.name} is already on` : true), run: () => requestPower(n.id, "wake"),
      },
      {
        id: `node:${n.id}:dgx`, title: `DGX Dashboard on ${n.name}`, group: "Nodes", subtitle: "NVIDIA's dashboard, through an SSH tunnel", icon: Gauge, entity,
        keywords: ["nvidia", "jupyter"], verbs: ["open"], needsAdmin: !cluster.demo, when: () => up,
        run: () => openOnNode(n, 11000, cluster.demo, () => openWindow("settings", { section: "data" })),
      },
    ];
  });
}

/** Palette commands for every container (Logs, Shell, Restart, Stop, Inspect) and every failed service. */
export function workloadCommands(cluster: ClusterSnapshot | null, observed: readonly Deployment[] = getDeployments()): Command[] {
  if (!cluster) return [];
  const deployments = deploymentsOf(cluster);
  const failed = observed.filter((d) => d.state === "failed");
  const out: Command[] = failed.map((d) => ({
    id: `deployment:${d.id}:explain`, title: `${deploymentLabel(d)} (failed) · Explain…`, group: "Deployments",
    subtitle: `${d.nodes.length} nodes · read the cause and next steps`, icon: FileSearch, verbs: ["explain", "logs", "inspect"],
    entity: { kind: "deployment", id: d.id, name: deploymentLabel(d) },
    keywords: [d.id, d.model ?? "", ...d.members.map((m) => m.container)], run: () => explainDeployment(d),
  }));
  for (const n of cluster.nodes) {
    for (const c of n.containers) {
      if (failed.some((d) => d.members.some((m) => m.node === n.id && m.id === c.id))) continue;
      const model = deployments.find((d) => d.members.some((m) => m.node === n.id && m.id === c.id))?.model;
      const base = {
        group: "Containers",
        entity: { kind: "container", id: c.id, name: c.name },
        keywords: [c.image, n.name, ...(model ? [model, shortModel(model)] : [])],
      };
      const t = { node: n, c };
      const sub = `${c.state} · ${model ? shortModel(model) : c.image}`;
      out.push(
        { ...base, id: `container:${n.id}:${c.id}:logs`, title: `Logs of ${c.name} on ${n.name}`, subtitle: sub, icon: FileText, verbs: ["logs"], run: () => openContainerLogs(n, c) },
        {
          ...base, id: `container:${n.id}:${c.id}:shell`, title: `Shell in ${c.name} on ${n.name}`, subtitle: sub, icon: SquareTerminal, verbs: ["shell", "exec"],
          needsAdmin: true, when: () => (c.state === "running" ? true : "It isn't running"), run: () => openContainerShell(n, c),
        },
        // What the container's state allows: a running one restarts or stops, a stopped one starts, a paused one resumes.
        ...(c.state === "running"
          ? [
              {
                ...base, id: `container:${n.id}:${c.id}:restart`, title: `Restart ${c.name} on ${n.name}…`, subtitle: sub, icon: RotateCcw, verbs: ["restart"],
                needsAdmin: true, run: () => runContainerAction("restart", [t]),
              },
              { ...base, id: `container:${n.id}:${c.id}:stop`, title: `Stop ${c.name} on ${n.name}…`, subtitle: sub, icon: StopIcon, verbs: ["stop"], needsAdmin: true, run: () => runContainerAction("stop", [t]) },
            ]
          : c.state === "paused"
            ? [{ ...base, id: `container:${n.id}:${c.id}:unpause`, title: `Resume ${c.name} on ${n.name}`, subtitle: sub, icon: Play, verbs: ["resume", "unpause"], needsAdmin: true, run: () => runContainerAction("unpause", [t]) }]
            : [{
                ...base, id: `container:${n.id}:${c.id}:start`, title: `Start ${c.name} on ${n.name}`, subtitle: sub, icon: Play, verbs: ["start", "restart"],
                needsAdmin: true, when: () => (applicable("start", c) ? true : notApplicable("start", [c])), run: () => runContainerAction("start", [t]),
              }]),
        { ...base, id: `container:${n.id}:${c.id}:inspect`, title: `Inspect ${c.name} on ${n.name}`, subtitle: sub, icon: FileSearch, verbs: ["inspect", "open"], run: () => inspectContainer(n, c) },
      );
    }
    for (const unit of n.failedUnits ?? []) {
      const base = { group: "Services", entity: { kind: "service", id: `${n.id}/${unit}`, name: unit }, keywords: ["failed", "systemd", n.name] };
      const short = unit.replace(/\.service$/, "");
      out.push(
        {
          ...base, id: `service:${n.id}:${unit}:restart`, title: `Restart ${short} on ${n.name}…`, subtitle: "Failed", icon: RotateCcw, verbs: ["restart"],
          needsAdmin: true, run: () => runServiceAction([n.id], unit, "restart"),
        },
        {
          ...base, id: `service:${n.id}:${unit}:journal`, title: `Show journal of ${short} on ${n.name}`, subtitle: "Failed · its status and recent journal", icon: ScrollText,
          verbs: ["open", "logs"], run: () => openWindow("services", { node: n.id, unit }),
        },
      );
    }
  }
  // Keep diagnostic reads ahead of actions in equally ranked palette results.
  return out.sort((a, b) => Number(!!a.needsAdmin) - Number(!!b.needsAdmin));
}

/** Registers the node, container and failed-service commands for as long as the shell is mounted. */
export function useWorkloadCommands() {
  const cluster = useCluster();
  const observed = useDeployments();
  // Rebuilt when a node, container or failed unit appears, goes or changes state, not every second.
  const key = `${cluster?.demo}|` + (cluster?.nodes ?? [])
    .map((n) => `${n.id}:${n.name}:${n.status}:${n.powerState ?? ""}|${n.containers.map((c) => `${c.id}/${c.name}/${c.state}`).join(",")}|${(n.failedUnits ?? []).join(",")}`)
    .join(";");
  const cmds = useMemo(() => [...nodeCommands(cluster), ...workloadCommands(cluster, observed)], [key, observed]);
  useCommands(cmds, [cmds]);
}
