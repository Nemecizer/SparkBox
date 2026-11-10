// What can be done to an endpoint, as plain functions so the cards, their context
// menus, the ⌘K palette and Containers' bulk restart run the same thing with the same
// confirmations. Each asks for Admin mode when it's locked and then carries on. The
// window it was started from (`origin`) owns its outcome: one toast, which a success
// from the window in front leaves out, and a restart's task reports under it.
import { requestAdmin } from "../../lib/admin";
import { endpointKey, endpointLabel, refreshEndpoints, type Endpoint } from "../../lib/endpoints";
import { modelsApi } from "../../lib/modelsApi";
import { confirmModelRestart } from "../../lib/modelRestart";
import { confirmAction, singleFlight } from "../../lib/session";
import { getCluster } from "../../lib/store";
import { withOrigin } from "../../lib/tasks";
import { noteOpOrigin, toast } from "../../lib/toasts";
import { errText } from "./shared";

/** Which window an action came from (its outcome's toast belongs to it). */
export interface ActionFrom {
  origin?: string;
}

/** A server in containers SparkBox didn't start: it restarts them in order. */
export function restartsExternally(e: Endpoint): boolean {
  return e.managedBy === "external" && e.source !== "manual" && (e.containerIds.length > 0 || (e.containerNames?.length ?? 0) > 0);
}

export function canRestart(e: Endpoint): boolean {
  return !!e.deploymentId || restartsExternally(e);
}

export async function unloadEndpoint(e: Endpoint, { origin }: ActionFrom = {}) {
  if (!e.deploymentId) return;
  if (!(await requestAdmin(`unload ${endpointLabel(e)}`))) return;
  const ok = await confirmAction({
    title: `Unload ${endpointLabel(e)}?`,
    body: `Stops its containers on ${e.nodes.map((id) => getCluster()?.nodes.find((n) => n.id === id)?.name ?? id).join(", ")} (docker stop, then docker rm) and checks that their memory, the port and /dev/shm came back. Apps using ${e.url} stop working, and it leaves Chat and SparkBox Server.`,
    confirmLabel: "Unload",
    danger: true,
    dedupeKey: `unload:${e.id}`,
  });
  if (!ok) return;
  try {
    const task = await modelsApi.deploymentUnload(e.deploymentId);
    if (task && origin) noteOpOrigin(task, origin);
    toast(task
      ? { kind: "info", opId: task, origin, title: `Unloading ${endpointLabel(e)}`, body: "Tasks shows each step, and what came back." }
      : { kind: "success", key: endpointKey(e.id), origin, title: `Unloaded ${endpointLabel(e)}` });
    refreshEndpoints().catch(() => {});
  } catch (err) {
    toast({ kind: "error", key: endpointKey(e.id), origin, title: "Couldn't unload it", body: errText(err) });
  }
}

export async function stopEndpoint(e: Endpoint, { origin }: ActionFrom = {}) {
  if (!(await requestAdmin(`stop ${e.containerNames?.join(", ") ?? "a server"}`))) return;
  const names = e.containerNames ?? [];
  const multi = e.nodes.length > 1;
  const ok = await confirmAction({
    title: `Stop ${names.join(", ")} on ${e.headNode}?`,
    body: `SparkBox didn't start ${names.length === 1 ? "this container" : "these containers"}. Stopping ${names.length === 1 ? "it" : "them"} takes ${endpointLabel(e)} offline${multi ? `; the workers on ${e.nodes.slice(1).join(", ")} lose their head and stop serving too` : ""}. You can start ${names.length === 1 ? "it" : "them"} again in Containers.`,
    details: names.map((n, i) => `${n}  (${e.containerIds[i] ?? "?"}) on ${e.headNode}`).join("\n"),
    confirmLabel: names.length === 1 ? "Stop container" : "Stop containers",
    danger: true,
    requireTyping: e.headNode,
    dedupeKey: `stop:${e.id}`,
  });
  if (!ok) return;
  try {
    await modelsApi.endpointStop(e.id);
    toast({ kind: "success", key: endpointKey(e.id), origin, title: `Stopped ${names.join(", ")}` });
  } catch (err) {
    toast({ kind: "error", key: endpointKey(e.id), origin, title: "Couldn't stop it", body: errText(err) });
  }
}

/** A double-click on Restart… asks once and restarts once. */
export function restartEndpoint(e: Endpoint, from: ActionFrom = {}): Promise<void> {
  return singleFlight(`restart-model:${e.id}`, () => askRestart(e, from));
}

async function askRestart(e: Endpoint, from: ActionFrom) {
  const model = endpointLabel(e);
  if (!canRestart(e)) return;
  if (!(await requestAdmin(`restart ${model}`))) return;
  const workers = e.nodes.slice(1);
  const order = !workers.length
    ? `Its container on ${e.headNode} stops and starts.`
    : e.deploymentId
      ? `In order: the head on ${e.headNode} stops, then the workers on ${workers.join(", ")}. The workers start, then the head.`
      : `In order: the workers on ${workers.join(", ")} stop, the head on ${e.headNode} stops and starts, then the workers start and join it.`;
  const dropCachesOnce = await confirmModelRestart({
    title: `Restart ${model}?`,
    body: `${order} ${e.deploymentId ? "It loads again with the same settings and key." : "SparkBox didn't start it; its containers keep their settings."} It's offline until it answers again; the task follows it until then.`,
    confirmLabel: workers.length ? `Restart ${model} in order` : "Restart",
    dedupeKey: `restart:${e.id}`,
  }, !!e.deploymentId && e.engine === "vllm");
  if (dropCachesOnce === null) return;
  await restartModel(e, { ...from, confirmed: true, dropCachesOnce });
}

/**
 * Restarts a model in order (a SparkBox load: the head stops, then the workers, and the workers
 * start before the head; a server it didn't start: the workers stop, the head stops and starts,
 * the workers join it), as one task that ends when it answers and generates again. Containers' bulk
 * restart calls it once its own confirmation was answered (`confirmed`). Resolves to the
 * task's id, or null when it didn't start.
 */
export async function restartModel(e: Endpoint, { origin, confirmed, dropCachesOnce = false }: ActionFrom & { confirmed?: boolean; dropCachesOnce?: boolean } = {}): Promise<string | null> {
  if (!canRestart(e)) return null;
  const model = endpointLabel(e);
  if (!confirmed) {
    await restartEndpoint(e, { origin });
    return null;
  }
  if (!(await requestAdmin(`restart ${model}`))) return null;
  return singleFlight(`restart-model-run:${e.id}`, () => withOrigin(origin, async (opId) => {
    try {
      const task = e.deploymentId ? await modelsApi.deploymentRestart(e.deploymentId, dropCachesOnce) : await modelsApi.endpointRestart(e.id);
      if (task && origin) noteOpOrigin(task, origin);
      toast({ kind: "info", opId: task || opId, origin, title: `Restarting ${model}`, body: "Tasks shows each step, and whether it answered." });
      return task || null;
    } catch (err) {
      toast({ kind: "error", opId, origin, title: `Couldn't restart ${model}`, body: errText(err) });
      return null;
    }
  }, (t) => t.nodes.some((n) => e.nodes.includes(n))));
}

export async function forgetEndpoint(e: Endpoint, { origin }: ActionFrom = {}) {
  if (!(await requestAdmin(`forget ${endpointLabel(e)}`))) return;
  try {
    await modelsApi.endpointForget(e.id);
    refreshEndpoints().catch(() => {});
  } catch (err) {
    toast({ kind: "error", key: endpointKey(e.id), origin, title: "Couldn't forget it", body: errText(err) });
  }
}

/**
 * For a "<model> didn't come back" warning (a restart whose Verify failed): the one-click
 * recovery, "Restart llama-3.3-70b in order…", when SparkBox can restart that model.
 * The toast that says it didn't come back puts this first.
 */
export function recoveryFor(text: string, endpoints: readonly Endpoint[]): { label: string; run: () => void } | null {
  const e = endpoints.find((x) => canRestart(x) && text.startsWith(`${endpointLabel(x)} didn't come back`));
  if (!e) return null;
  return { label: e.nodes.length > 1 ? `Restart ${endpointLabel(e)} in order…` : `Restart ${endpointLabel(e)}…`, run: () => void restartEndpoint(e) };
}
