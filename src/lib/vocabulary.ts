// One vocabulary for every surface. Import these instead of spelling the words
// out, so the menu, the palette, the sidebar and the windows always agree:
// "Restart…" (never "Reboot"), "1 critical · 2 warnings" (never "issues"),
// window titles that match their sidebar labels.

/**
 * Power verbs. An ellipsis means the command opens a dialog first; Wake doesn't.
 * The keys match `requestPower`'s actions in power.ts ("reboot" is the wire name).
 */
export const POWER = {
  restart: "Restart…",
  shutdown: "Shut down…",
  wake: "Wake",
} as const;

/** The same verbs without the ellipsis, for task titles and confirm buttons ("Restart cell3"). */
export const POWER_VERB = {
  restart: "Restart",
  shutdown: "Shut down",
  wake: "Wake",
} as const;

/** `count` of `noun`, with the plural when it isn't 1: plural(2, "warning") → "2 warnings". */
export function plural(count: number, noun: string, many = `${noun}s`): string {
  return `${count} ${count === 1 ? noun : many}`;
}

/**
 * Alert counts, critical first: "1 critical · 2 warnings", "3 warnings", "1 critical".
 * Zero counts are left out; with nothing at all it's "No alerts". Never "issues".
 */
export function issueSummary(crit: number, warn: number): string {
  const parts: string[] = [];
  if (crit > 0) parts.push(`${crit} critical`);
  if (warn > 0) parts.push(plural(warn, "warning"));
  return parts.length ? parts.join(" · ") : "No alerts";
}

/** The worst severity among the counts, for tinting a badge; null when there's nothing. */
export function worstSeverity(crit: number, warn: number): "crit" | "warn" | null {
  return crit > 0 ? "crit" : warn > 0 ? "warn" : null;
}

export type EndpointHealth = "ready" | "loading" | "down" | "stopped";

/** Model and endpoint states. Ready is quiet (neutral dot plus the word); Down is red with a reason. */
export const ENDPOINT_STATE: Record<EndpointHealth, string> = {
  ready: "Ready",
  loading: "Loading",
  down: "Down",
  stopped: "Stopped",
};

/** "Ready", "Loading 42%", "Down · Pipeline stage 3 of 3 on cell3 is offline", "Stopped". */
export function endpointStateLabel(health: EndpointHealth, opts: { pct?: number; reason?: string } = {}): string {
  const word = ENDPOINT_STATE[health];
  if (health === "loading" && opts.pct != null && Number.isFinite(opts.pct)) return `${word} ${Math.round(opts.pct)}%`;
  if (health === "down" && opts.reason) return `${word} · ${opts.reason}`;
  return word;
}

/**
 * Window titles, equal to their sidebar labels. Scope ("All nodes") goes in a
 * subtitle, not the title. Kinds whose title depends on params (a node, a switch,
 * an editor) are titled by the registry from those params.
 */
const WINDOW_TITLES: Record<string, string> = {
  dashboard: "Dashboard",
  topology: "Topology",
  alerts: "Alerts",
  "cluster-metrics": "Cluster metrics",
  workloads: "Workloads",
  containers: "Containers",
  services: "Services",
  models: "Models",
  endpoints: "Endpoints",
  ollama: "Ollama",
  chat: "Chat",
  server: "Server",
  providers: "Providers",
  vpn: "VPN",
  fabric: "Fabric",
  files: "Files",
  terminal: "Terminal",
  rdp: "RDP",
  howto: "How To",
  fleet: "Fleet Command",
  activity: "Activity log",
  updates: "Updates",
  tasks: "Tasks",
  settings: "Settings",
  connect: "Connect my Sparks",
  node: "Node",
  switch: "Switch",
  inspect: "Inspect",
  editor: "Editor",
};

/** The title for a window kind; unknown kinds get their id with a capital first letter. */
export function windowTitle(kind: string): string {
  return WINDOW_TITLES[kind] ?? (kind ? kind[0].toUpperCase() + kind.slice(1) : kind);
}
