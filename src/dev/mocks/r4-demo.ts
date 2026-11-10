// r4-demo's mock handlers: the scenario list as the Rust simulator gives it, with the cluster as
// found on Oct 29 first and the everyday serving world second (demo/scenarios.rs DEFS).
import type { MockHandlers } from "../../test/tauri";
import type { ScenarioInfo } from "../../lib/netApi";
import { handlers as net } from "./net";

const AS_FOUND: ScenarioInfo = {
  id: "as-found",
  title: "Your cluster at rest (Oct 29)",
  description: "Your four nodes as SparkBox found them on Oct 29: nothing serving, the 4-node GLM-5.3-Flash run that exited after 54 s two days before, Ollama on cell1 and cell3 listening on the node only, and the recabling in the kernel log.",
  periodS: 0,
  affects: [],
};

const EVERYDAY = "The same machines serving: Llama 3.3 70B across cell1-cell3, Ollama on gx10-0000, and one failed service (nvidia-dgx-telemetry) on cell1.";

export const handlers: MockHandlers = {
  demo_scenarios_list: () => {
    const rest = (net.demo_scenarios_list({}) as ScenarioInfo[]).filter((s) => s.id !== "as-found");
    return [AS_FOUND, ...rest.map((s) => (s.id === "default" ? { ...s, title: "Everyday serving", description: EVERYDAY } : s))];
  },
};
