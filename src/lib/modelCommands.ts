// Palette commands for models (App.tsx registers them once): load a model, and for
// every model serving on the cluster: chat with it, open its endpoint, benchmark,
// restart or unload it. The per-model commands follow the endpoint list as models
// come and go; the actions are the cards' own (same confirmations, same Admin ask).
import { Gauge, MessageSquare, Play, PowerOff, RotateCcw, Server } from "lucide-react";
import { registerCommands, type Command } from "./commands";
import { endpointLabel, getEndpoints, openChatWith, subscribeEndpoints, type Endpoint } from "./endpoints";
import { getTasks, onTasksChange } from "./tasks";
import { openWindow } from "../wm/WindowManager";
import { canRestart, restartEndpoint, unloadEndpoint } from "../windows/models/endpointActions";
import { ENGINE_LABEL } from "../windows/models/shared";

/** What a command calls the endpoint (endpointLabel): its served name, or for Ollama the server. */
function name(e: Endpoint): string {
  return endpointLabel(e);
}

/** A restart of this model is running as a task (its title starts "Restart <name>"). */
export function restartRunning(e: Endpoint): boolean {
  const label = endpointLabel(e);
  return getTasks().some((t) => t.kind === "model" && t.state === "running" && t.title.startsWith(`Restart ${label}`) && t.nodes.some((n) => e.nodes.includes(n)));
}

function staticCommands(): Command[] {
  return [
    {
      id: "model:load",
      title: "Load model…",
      group: "Models",
      subtitle: "Pick a model folder and nodes, check the memory plan, then load it with vLLM or SGLang",
      icon: Play,
      verbs: ["load", "deploy", "serve", "start"],
      keywords: ["vllm", "sglang", "model", "gpu"],
      run: () => openWindow("models", { tab: "load", nav: String(Date.now()) }),
    },
  ];
}

/** The commands for one endpoint. */
export function endpointCommands(e: Endpoint): Command[] {
  const model = name(e);
  const entity = { kind: "model", id: e.id, name: model };
  const where = `${ENGINE_LABEL[e.engine]} on ${e.nodes.join(", ")}`;
  const out: Command[] = [
    {
      id: `model:open:${e.id}`,
      title: `Open endpoint ${model}`,
      group: "Models",
      subtitle: where,
      icon: Server,
      verbs: ["open", "show", "endpoint"],
      entity,
      run: () => openWindow("endpoints", { id: e.id, nav: String(Date.now()) }),
    },
  ];
  const chatModels = e.engine === "ollama" ? (e.ollama?.map((m) => m.name) ?? []) : e.models.slice(0, 1);
  for (const m of chatModels) {
    out.push({
      id: `model:chat:${e.id}:${m}`,
      title: `Chat with ${m}`,
      group: "Models",
      subtitle: where,
      icon: MessageSquare,
      verbs: ["chat", "ask", "talk"],
      entity: { kind: "model", id: `${e.id}/${m}`, name: m },
      when: () => (e.health !== "ready" ? `${m} isn't ready` : e.providerId ? true : "Not reachable from this Mac"),
      run: () => e.providerId && openChatWith(e.providerId, m),
    });
  }
  // The Benchmarks tab drives vLLM's completions API with token-id prompts.
  if (e.engine === "vllm") {
    out.push({
      id: `model:bench:${e.id}`,
      title: `Benchmark ${model}`,
      group: "Models",
      subtitle: "Throughput and latency at 1 to 32 requests at once",
      icon: Gauge,
      verbs: ["benchmark", "bench", "measure"],
      entity,
      when: () => (e.health === "ready" ? true : `${model} isn't ready`),
      run: () => openWindow("models", { tab: "bench", nav: String(Date.now()) }),
    });
  }
  if (canRestart(e)) {
    out.push({
      id: `model:restart:${e.id}`,
      title: `Restart ${model}…`,
      group: "Models",
      subtitle: e.nodes.length > 1 ? "In order, then checks it answers" : "Then checks it answers",
      icon: RotateCcw,
      verbs: ["restart", "reload", "reboot"],
      entity,
      needsAdmin: true,
      when: () => (restartRunning(e) ? "Already restarting" : true),
      run: () => restartEndpoint(e),
    });
  }
  if (e.deploymentId) {
    out.push({
      id: `model:unload:${e.id}`,
      title: `Unload ${model}`,
      group: "Models",
      subtitle: `Stops its containers on ${e.nodes.join(", ")}`,
      icon: PowerOff,
      verbs: ["unload", "stop", "remove"],
      entity,
      needsAdmin: true,
      run: () => unloadEndpoint(e),
    });
  }
  return out;
}

export function registerModelCommands(): () => void {
  const stopStatic = registerCommands(staticCommands());
  let key = "";
  let stopPer: () => void = () => {};
  const refresh = () => {
    const list = getEndpoints();
    // Re-register only when something a command depends on changed.
    const next = list.map((e) => `${e.id}:${name(e)}:${e.health}:${e.providerId ?? ""}:${e.deploymentId ?? ""}:${(e.ollama ?? []).map((m) => m.name).join(",")}`).join(";");
    if (next === key) return;
    key = next;
    stopPer();
    stopPer = registerCommands(list.flatMap(endpointCommands));
  };
  refresh();
  const unsub = subscribeEndpoints(refresh);
  // "Already restarting" follows the tasks, so re-register when a model task starts or ends.
  let running = "";
  const unsubTasks = onTasksChange(() => {
    const next = getTasks().filter((t) => t.kind === "model" && t.state === "running").map((t) => t.id).join(",");
    if (next === running) return;
    running = next;
    key = "";
    refresh();
  });
  return () => {
    unsub();
    unsubTasks();
    stopPer();
    stopStatic();
  };
}
