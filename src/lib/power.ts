// The confirmations in front of node operations, shared by the windows that start them and by
// Retry (which reopens the same flow, never replays a call):
// - Restart or shut down a node: what goes down with it (impact.ts), now or in 1 or 5 minutes,
//   and — when the stakes are high (you're away, the node is part of a multi-node model, or
//   SparkBox reaches the cluster through it) — typing the node's name. Shut down always asks
//   for the name and says when Wake-on-LAN couldn't bring the node back. Wake asks nothing.
//   With Live data a restart shows the checks before it (P1.26, ops/prereboot.rs): which kernel
//   boots next, its initrd and NVIDIA module; "Checking cell2 before the restart…" shows while
//   they run. When one blocks, there is no Restart button, only the reasons (the backend refuses
//   the restart then too). In 5 minutes is checked again a minute before its time (In 1 minute
//   isn't: it was just checked); if a check fails then, SparkBox tries to call it off (shutdown
//   -c can be refused or fail).
// - Fleet Command's tiers: Read runs at once; Change lists what's on the targets (and asks for
//   "4 nodes" typed for root on several); Destructive adds a red banner, forces one node at a
//   time with Stop on first failure, and runs a canary first. Cuts access (stopping SSH, the
//   network or the firewall, auth files, power verbs on several nodes) says SparkBox may lose
//   its connection, and SparkBox logs in to each node again before the next.
// - Bulk container and service actions, with their impact.
// The backend then follows each one as a task.
import { createElement as h, useState } from "react";
import { requestAdmin } from "./admin";
import { getEndpoints } from "./endpoints";
import { describeError, showError } from "./errors";
import { fleetImpact, impactOf, powerImpact, type ImpactAction, type ImpactItem } from "./impact";
import { opsApi, type Classified, type CommandClass, type FleetStrategy, type Impact } from "./opsApi";
import { alertInfo, confirmAction } from "./session";
import { getCluster } from "./store";
import { getTerminals } from "./terminals";
import { dismissToast, toast } from "./toasts";
import { api, inTauri, invoke } from "./api";

export type PowerAction = "reboot" | "poweroff" | "wake";

const WHEN = [
  { value: 0, label: "Now" },
  { value: 1, label: "In 1 minute" },
  { value: 5, label: "In 5 minutes" },
];

/** The impact list, as the dialog shows it: each line with its weight. */
export function ImpactList({ items }: { items: ImpactItem[] }) {
  if (!items.length) return null;
  return h(
    "ul",
    { className: "ops-impact", "aria-label": "What this affects" },
    items.map((it, i) =>
      h("li", { key: i, className: `ops-impact-item ${it.tone}` }, h("span", { className: "ops-impact-mark", "aria-hidden": true }, it.tone === "crit" ? "!" : it.tone === "warn" ? "▲" : "•"), h("span", null, it.text)),
    ),
  );
}

function WhenPicker({ onChange }: { onChange: (m: number) => void }) {
  const [value, setValue] = useState(0);
  return h(
    "fieldset",
    { className: "ops-when" },
    h("legend", null, "When"),
    WHEN.map((w) =>
      h(
        "label",
        { key: w.value, className: "ops-when-option" },
        h("input", {
          type: "radio",
          name: "sparkbox-power-when",
          checked: value === w.value,
          onChange: () => {
            setValue(w.value);
            onChange(w.value);
          },
        }),
        w.label,
      ),
    ),
  );
}

/**
 * Whether you're away from the cluster (the VPN carries SparkBox's traffic). Fails closed: a
 * status SparkBox can't read, or a tunnel that's connecting or broken, counts as away.
 */
export async function awayStatus(): Promise<"home" | "away" | "unknown"> {
  if (!inTauri) return "home";
  try {
    const state = (await api.vpnGet()).status.state;
    return state === "off" || state === "direct" ? "home" : state === "connected" ? "away" : "unknown";
  } catch {
    return "unknown";
  }
}

export async function isRemote(): Promise<boolean> {
  return (await awayStatus()) !== "home";
}

interface WolStatus {
  enabled: boolean | null;
  routes: unknown[];
}

async function wolStatus(node: string): Promise<WolStatus | null> {
  try {
    return await invoke<WolStatus>("node_wol_status", { node });
  } catch {
    return null;
  }
}

/** "You won't be able to turn cell3 back on remotely: …", or null when Wake-on-LAN is on. */
export function wolWarning(name: string, wol: WolStatus | null): string | null {
  if (wol?.enabled === true) return null;
  return wol?.enabled === false
    ? `You won't be able to turn ${name} back on remotely: Wake-on-LAN is off on its LAN port.`
    : `You may not be able to turn ${name} back on remotely: SparkBox can't tell whether Wake-on-LAN is on.`;
}

/** One of the checks before a restart (ops/prereboot.rs `Check`). */
export interface PreRebootCheck {
  id: string;
  label: string;
  /** fail blocks the restart; info is recorded only. */
  state: "ok" | "fail" | "info";
  detail: string;
}

/** What the checks found on one node (ops/prereboot.rs `PreReboot`). */
export interface PreReboot {
  node: string;
  kernel: string | null;
  ok: boolean;
  blocking: string[];
  checks: PreRebootCheck[];
}

/** The checks on `nodeId` now (`node_prereboot_check`, reads only), or why they couldn't run. */
export async function prerebootChecks(nodeId: string): Promise<PreReboot | { error: string }> {
  try {
    return await invoke<PreReboot>("node_prereboot_check", { node: nodeId });
  } catch (e) {
    return { error: describeError(e).message };
  }
}

/** The checks as the confirmation lists them: a failed one is marked, recorded ones say so. */
export function checkItems(r: PreReboot): ImpactItem[] {
  return r.checks.map((c): ImpactItem => ({ kind: "probe", tone: c.state === "fail" ? "crit" : "info", text: `${c.label}${c.state === "info" ? " (recorded)" : ""}: ${c.detail}` }));
}

/** The reasons, as the sheet says them ("Not restarted." is the task's word, after the fact). */
function reasons(r: PreReboot): string {
  return r.blocking.map((b) => b.replace(/ Not restarted\.$/, "")).join(" ");
}

function ChecksBeforeRestart({ result }: { result: PreReboot | { error: string } }) {
  if ("error" in result) {
    const why = result.error.trim();
    return h(
      "p",
      { className: "ops-confirm-note" },
      `The checks before the restart couldn't run: ${/[.!?]$/.test(why) ? why : `${why}.`} SparkBox runs them again before it sends the restart, and doesn't restart the node if they fail.`,
    );
  }
  return h("div", null, h("p", { className: "ops-confirm-note" }, `Checks before the restart${result.ok ? ": passed" : ""}`), h(ImpactList, { items: checkItems(result) }));
}

async function startPower(nodeId: string, action: PowerAction, delayMin: number, name: string): Promise<string | null> {
  const verb = action === "reboot" ? "Restart" : action === "poweroff" ? "Shut down" : "Wake";
  if (!(await requestAdmin("node_power"))) return null;
  try {
    return await invoke<string>("node_power", { node: nodeId, action, delayMin });
  } catch (e) {
    showError(e, { action: `${verb} ${name}`, node: name });
    return null;
  }
}

/**
 * Asks, unlocks Admin mode if needed (Touch ID or password with Live data), and starts the
 * restart, shutdown or wake. Resolves to the task's id, or null when cancelled or refused.
 */
export async function requestPower(nodeId: string, action: PowerAction): Promise<string | null> {
  const cluster = getCluster();
  const node = cluster?.nodes.find((n) => n.id === nodeId);
  const name = node?.name ?? nodeId;
  // Waking breaks nothing: no dialog. The task sends the packet, waits and checks the models.
  if (action === "wake") return startPower(nodeId, "wake", 0, name);

  // A restart with Live data: its checks first (Demo has no node to read; a shutdown isn't checked),
  // with a notice while they run (they read the node, a few seconds).
  const live = !!cluster && !cluster.demo;
  const checking = action === "reboot" && live ? toast({ kind: "progress", title: `Checking ${name} before the restart…`, progress: null, key: `prereboot:${nodeId}` }) : null;
  const [away, wol, checks] = await Promise.all([
    awayStatus(),
    action === "poweroff" ? wolStatus(nodeId) : Promise.resolve(null),
    action === "reboot" && live ? prerebootChecks(nodeId) : Promise.resolve(null),
  ]).finally(() => {
    if (checking) dismissToast(checking);
  });
  if (checks && !("error" in checks) && !checks.ok) {
    await alertInfo({
      title: `${name} can't restart now`,
      body: h("div", { className: "ops-confirm" }, h("p", null, `SparkBox won't restart ${name}: ${reasons(checks)}`), h(ChecksBeforeRestart, { result: checks })),
    });
    return null;
  }
  const report = cluster
    ? impactOf(cluster, { nodes: [nodeId], action, endpoints: getEndpoints(), remote: away !== "home", remoteUnknown: away === "unknown", terminals: getTerminals().tabs.filter((t) => t.phase === "open" || t.phase === "connecting") })
    : { tier: "high" as const, items: [] };
  const items = [...report.items];
  const noWake = action === "poweroff" ? wolWarning(name, wol) : null;
  if (noWake) items.unshift({ kind: "offline", tone: "crit", text: noWake });
  let delay = 0;
  const reboot = action === "reboot";
  const lead = reboot
    ? `${name} restarts. Its containers stop, and SparkBox reconnects when it's back (usually 2–3 minutes).`
    : `${name} shuts down and stays off.`;
  // What happens to the models there (r4-serving's powerImpact), and the one-click recovery:
  // checked, the restart is followed by model_restart_after (in order once the node is back).
  const models = powerImpact(nodeId, action);
  let follow = !!models.followUp;
  const ok = await confirmAction({
    title: reboot ? `Restart ${name}?` : `Shut down ${name}?`,
    body: h(
      "div",
      { className: "ops-confirm" },
      h("p", null, lead),
      models.paragraph ? h("p", { className: "ops-confirm-models" }, models.paragraph) : null,
      h(ImpactList, { items }),
      checks ? h(ChecksBeforeRestart, { result: checks }) : null,
      reboot && !live ? h("p", { className: "ops-confirm-note" }, "Checks before the restart: Available with Live data") : null,
      models.followUp ? h(FollowUp, { label: models.followUp.label, onChange: (on: boolean) => (follow = on) }) : null,
      h(WhenPicker, { onChange: (m: number) => (delay = m) }),
      reboot && live
        ? h("p", { className: "ops-confirm-note" }, `It's checked now. In 5 minutes is checked again a minute before its time; neither is checked when the restart fires. If that second check fails, SparkBox tries to call it off (shutdown -c) and says in the task if it couldn't. If SparkBox isn't running at that moment, ${name} restarts on its own schedule without that second check.`)
        : null,
    ),
    confirmLabel: reboot ? "Restart" : "Shut down",
    danger: true,
    requireTyping: !reboot || report.tier === "high" ? name : undefined,
    dedupeKey: `power:${action}:${nodeId}`,
  });
  if (!ok) return null;
  const id = await startPower(nodeId, action, delay, name);
  if (id && follow && models.followUp) {
    try {
      await invoke<string>("model_restart_after", { node: nodeId, endpointId: models.followUp.endpointId });
    } catch (e) {
      showError(e, { action: models.followUp.label.replace(/^Then r/, "R"), node: name });
    }
  }
  return id;
}

/** The confirm's recovery checkbox, checked by default. */
function FollowUp({ label, onChange }: { label: string; onChange: (on: boolean) => void }) {
  return h(
    "label",
    { className: "ops-confirm-follow" },
    h("input", { type: "checkbox", defaultChecked: true, onChange: (e: { target: { checked: boolean } }) => onChange(e.target.checked) }),
    h("span", null, label),
  );
}

// ---- Fleet Command's tiers

export type FleetTier = "read" | "change" | "destructive";

export interface FleetRequest {
  nodes: string[];
  command: string;
  sudo: boolean;
  timeoutS: number;
  strategy: FleetStrategy;
  stopOnFailure: boolean;
  /** A snippet's name: the task is "Fleet: <name>". */
  title?: string | null;
}

export interface FleetPlan extends FleetRequest {
  tier: FleetTier;
  /** Run on the first node alone, then ask before the rest. */
  canary: boolean;
  /** SparkBox may lose its connection: after each node it logs in again before the next. */
  cutsAccess?: boolean;
}

const UNCHECKED = "SparkBox couldn't check this command, so it treats it as destructive.";

/** The tier for the exact command about to run. Fails closed: no answer is Destructive. */
export async function classifyForRun(command: string, sudo: boolean): Promise<Classified & { tier: FleetTier }> {
  let c: Classified;
  try {
    c = await opsApi.classify(command);
  } catch {
    c = { class: "destructive", escalates: true, reason: UNCHECKED };
  }
  const cls: CommandClass = c?.class ?? "destructive";
  const tier: FleetTier = cls === "destructive" ? "destructive" : cls === "change" || sudo || c.escalates ? "change" : "read";
  return { ...c, reason: cls === "destructive" ? c.reason ?? UNCHECKED : c.reason, tier };
}

function nodeCount(n: number): string {
  return `${n} node${n === 1 ? "" : "s"}`;
}

function DestructiveBanner({ reason, cuts, power }: { reason?: string | null; cuts?: boolean; power?: boolean }) {
  return h(
    "div",
    { className: "ops-danger-banner", role: "alert" },
    // A power verb takes the nodes down (it isn't a "cuts access" command of its own).
    h("strong", null, power ? "This takes these nodes down" : cuts ? "SparkBox may lose its connection to these nodes" : "This can destroy data or take nodes down"),
    reason ? h("span", null, reason) : null,
  );
}

/** Whether a run cuts access: the command itself, or a power verb on several nodes at once. */
export function cutsAccess(c: Pick<Classified, "cutsAccess" | "power">, nodes: number): boolean {
  return !!c.cutsAccess || (!!c.power && nodes > 1);
}

/**
 * Fleet's confirmation for `req` on its tier. Resolves to the plan to run (the Destructive tier
 * forces one node at a time, Stop on first failure and a canary), or null when cancelled.
 */
export async function confirmFleet(req: FleetRequest, names: (id: string) => string): Promise<FleetPlan | null> {
  const c = await classifyForRun(req.command, req.sudo);
  const n = req.nodes.length;
  const root = req.sudo || c.escalates;
  if (c.tier === "read") return { ...req, tier: "read", canary: false };
  const cluster = getCluster();
  const remote = await isRemote();
  const items = cluster ? fleetImpact(cluster, { nodes: req.nodes, endpoints: getEndpoints(), remote, destructive: c.tier === "destructive" }) : [];
  const list = req.nodes.map(names).join(", ");
  const typed = n > 1 ? nodeCount(n) : names(req.nodes[0] ?? "");
  const cmd = h("p", { className: "mono ops-confirm-cmd" }, `${req.sudo ? "# " : "$ "}${req.command}`);
  if (c.tier === "destructive") {
    const canary = n > 1;
    const cuts = cutsAccess(c, n);
    const first = names(req.nodes[0]);
    const reason = cuts && !c.cutsAccess ? `It ${c.power === "off" ? "switches off" : "restarts"} each node, so SparkBox loses each one for a while.` : c.reason;
    const how = cuts
      ? canary
        ? `${first} first. SparkBox then waits until it can log in to ${first} again over SSH, and asks before the other ${nodeCount(n - 1)}. They run one at a time, and it stops at the first one that fails or that it can't reach.`
        : `It runs on ${list}, and SparkBox then checks it can still log in over SSH.`
      : canary
        ? `${first} first. If it works, SparkBox asks before the other ${nodeCount(n - 1)}, which then run one at a time and stop at the first failure.`
        : `It runs on ${list}.`;
    const ok = await confirmAction({
      title: `Run on ${nodeCount(n)}${req.sudo ? " as root" : ""}?`,
      body: h("div", { className: "ops-confirm" }, h(DestructiveBanner, { reason, cuts, power: cuts && !c.cutsAccess }), cmd, h("p", null, how), h(ImpactList, { items })),
      confirmLabel: canary ? `Run on ${first}` : "Run",
      danger: true,
      requireTyping: typed,
      dedupeKey: `fleet:${req.command}:${req.nodes.join(",")}`,
    });
    if (!ok) return null;
    return { ...req, tier: "destructive", strategy: "oneByOne", stopOnFailure: true, canary, cutsAccess: cuts };
  }
  // Docker is root-equivalent (P1.42): a root docker command runs a canary first, then the
  // rest one node at a time, so a bad one doesn't hit the whole fleet at once.
  const dockerRoot = !!c.root && n > 1;
  const first = names(req.nodes[0]);
  const how = dockerRoot
    ? ` Docker is root on the node, so ${first} runs first; if it's fine, the other ${nodeCount(n - 1)} run one at a time, stopping at the first failure.`
    : `${req.strategy === "oneByOne" ? " One node at a time" : req.strategy === "canary" ? ` ${first} first, then the rest` : " All at once"}${req.strategy !== "parallel" && req.stopOnFailure ? ", stopping at the first failure." : "."}`;
  const ok = await confirmAction({
    title: `Run on ${nodeCount(n)}${req.sudo ? " as root" : ""}?`,
    body: h(
      "div",
      { className: "ops-confirm" },
      cmd,
      h("p", null, `${root ? "This runs as root on " : "This command can change things on "}${list}.${how}`),
      h(ImpactList, { items }),
    ),
    confirmLabel: dockerRoot ? `Run on ${first}` : "Run",
    danger: true,
    requireTyping: root && n > 1 ? typed : undefined,
    dedupeKey: `fleet:${req.command}:${req.nodes.join(",")}`,
  });
  if (!ok) return null;
  return dockerRoot
    ? { ...req, tier: "change", strategy: "oneByOne", stopOnFailure: true, canary: true }
    : { ...req, tier: "change", canary: false };
}

/** After a canary worked: ask before the rest. `cuts`: SparkBox logged in to it again, and does after each. */
export function confirmCanaryRest(first: string, rest: string[], names: (id: string) => string, cuts = false): Promise<boolean> {
  return confirmAction({
    title: `Continue on ${rest.length} more?`,
    body: cuts
      ? `It worked on ${names(first)}, and SparkBox logged in to it again over SSH. Run on ${rest.map(names).join(", ")} one at a time, checking each the same way and stopping at the first that fails or can't be reached?`
      : `It worked on ${names(first)}. Run on ${rest.map(names).join(", ")} one at a time, stopping at the first failure?`,
    confirmLabel: `Continue on ${nodeCount(rest.length)}`,
    danger: true,
  });
}

/** Confirms on its tier, then starts the run (all nodes, or the canary alone). Resolves to the run's id. */
export async function requestFleetRun(req: FleetRequest, names: (id: string) => string): Promise<{ id: string; plan: FleetPlan } | null> {
  const plan = await confirmFleet(req, names);
  if (!plan) return null;
  if (!(await requestAdmin("fleet_start"))) return null;
  const nodes = plan.canary ? plan.nodes.slice(0, 1) : plan.nodes;
  try {
    const id = await opsApi.fleetStart({ nodes, command: plan.command, sudo: plan.sudo, timeoutS: plan.timeoutS, strategy: plan.strategy, stopOnFailure: plan.stopOnFailure, title: plan.title ?? null, cutsAccess: !!plan.cutsAccess });
    return { id, plan };
  } catch (e) {
    showError(e, { action: "Run the command" });
    return null;
  }
}

// ---- Bulk containers and services

export interface ContainerTarget {
  node: string;
  id: string;
  name: string;
}

const CONTAINER_VERB: Record<string, string> = { start: "Start", stop: "Stop", restart: "Restart", kill: "Force-stop", pause: "Pause", unpause: "Resume", remove: "Remove" };

/** Confirms a bulk container action with what it breaks (volumes too), then starts it. */
export async function requestContainersBulk(items: ContainerTarget[], action: string, removeVolumes: boolean, title?: string | null): Promise<string | null> {
  const cluster = getCluster();
  const verb = CONTAINER_VERB[action] ?? "Change";
  const nodes = [...new Set(items.map((i) => i.node))];
  const report = cluster
    ? impactOf(cluster, { nodes, action: action as ImpactAction, containerIds: items.map((i) => i.id), endpoints: getEndpoints(), remote: await isRemote() })
    : { items: [] as ImpactItem[] };
  const list: ImpactItem[] = [...report.items];
  if (action === "remove" && removeVolumes) list.unshift({ kind: "container", tone: "crit", text: "Their volumes are deleted too, with the data in them." });
  const what = title ?? `${verb} ${items.length} container${items.length === 1 ? "" : "s"}`;
  const ok = await confirmAction({
    title: `${what}?`,
    body: h("div", { className: "ops-confirm" }, h("p", null, items.map((i) => i.name || i.id.slice(0, 12)).join(", ")), h(ImpactList, { items: list })),
    confirmLabel: verb,
    danger: action !== "start" && action !== "unpause",
    dedupeKey: `containers:${action}:${items.map((i) => i.id).join(",")}`,
  });
  if (!ok || !(await requestAdmin("containers_bulk"))) return null;
  try {
    return await opsApi.containersBulk(items, action, removeVolumes, title ?? undefined);
  } catch (e) {
    showError(e, { action: what });
    return null;
  }
}

/** Confirms a service action on its impact (typed name when it's high), then starts it. */
export async function requestServicesApply(nodes: string[], unit: string, action: string): Promise<string | null> {
  let impact: Impact | null = null;
  try {
    impact = await opsApi.serviceImpact(nodes, unit, action);
  } catch (e) {
    showError(e, { action: `Check ${unit}` });
    return null;
  }
  if (impact.blocked) {
    showError(impact.blocked, { action: impact.title });
    return null;
  }
  const ok = await confirmAction({
    title: impact.title,
    body: impact.lines.length ? h("ul", { className: "ops-impact" }, impact.lines.map((l, i) => h("li", { key: i, className: "ops-impact-item warn" }, l))) : undefined,
    confirmLabel: impact.confirmLabel,
    danger: impact.tier !== "low",
    requireTyping: impact.requireTyping ?? undefined,
  });
  if (!ok || !(await requestAdmin("services_apply"))) return null;
  try {
    return await opsApi.servicesApply(nodes, unit, action);
  } catch (e) {
    showError(e, { action: impact.title });
    return null;
  }
}
