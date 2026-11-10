// The main menu as a Mac source list: fixed sections with quiet uppercase
// headers (collapsible from a chevron that appears on hover), one row per node
// and switch with its live status, badges for what needs attention, a filter at
// the top and Settings pinned at the bottom. Healthy rows are quiet; a node
// that's down moves to the top of Nodes and says so in red. A node's glyph is the
// node's own state (a model it serves being down is a note in its tooltip, not a colour).
import { useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { useEndpoints, type Endpoint } from "../lib/endpoints";
import {
  Activity, Bell, BookOpen, Boxes, ChartLine, ChevronDown, Command, Container, Copy, Database, Download, EthernetPort, Feather, FolderOpen, Gauge,
  Globe, KeyRound, LayoutDashboard, ListChecks, MessageSquare, Network, Package, RadioTower, Rocket, Router,
  CircleHelp, ScrollText, Server, ServerCog, Settings, ShieldCheck, SquareTerminal, Monitor, Waypoints, type LucideIcon,
} from "lucide-react";
// ── r4-safety ── (imports)
import { Layers } from "lucide-react";
import { DISPLAY_LABEL, displayStatus, statusTooltip, switchDisplayStatus, useHealth, useHealthSummary, type HealthState } from "../lib/health";
import { nodeMenu, showContextMenu, type MenuItem as ContextItem } from "../lib/contextMenu";
import { linkWord, powerLabel } from "../lib/nodeStatus";
import { useRecentExits, type RecentExit } from "../lib/exits";
import { gpuError } from "../lib/metrics";
import { useCluster } from "../lib/store";
import type { ClusterSnapshot, NodeSnapshot, SwitchKind } from "../lib/types";
import { issueSummary, worstSeverity } from "../lib/vocabulary";
import { VPN_DOT, VPN_LABEL, useVpnStatus } from "../lib/vpn";
import { peekFabricHealth } from "../lib/fabric"; // r2-fabric
import { Cable as FabricIcon } from "lucide-react"; // r2-fabric
import { isKnown } from "../windows/registry";
import { useShowUpcoming } from "../windows/settings/GeneralSection";
import { useWM, winId, type Params } from "../wm/WindowManager";
import { Icon } from "./Icon";
import { nodeGlyph, nodeGlyphWord, nodeTitle } from "./StatusBar";
import { DotGlyph, StateGlyph, statusWord, type DotStatus, type GlyphState } from "./ui";

interface Row {
  id: string;
  label: string;
  icon: LucideIcon;
  kind: string;
  params?: Params;
  /** Overlaid on the icon (nodes, switches, VPN). */
  status?: DotStatus;
  /** A node's own state, drawn with the state glyphs instead of the link dot (offline is the loudest). */
  glyph?: GlyphState;
  statusWord?: string;
  trailing?: string;
  /** A trailing word in red: "Offline". */
  trailingCrit?: boolean;
  badge?: { text: string; tone: "warn" | "crit"; title: string };
  /** Extra words the filter matches. */
  keywords?: string[];
  /** Only listed while the filter matches it. */
  filterOnly?: boolean;
  menu?: () => ContextItem[];
  title?: string;
}

interface Section {
  id: string;
  label: string;
  rows: Row[];
}

const COLLAPSE_KEY = "sparkbox.sidebar.collapsed";
// Every section starts open, Coming soon included: turning on "Show upcoming features" should show them.
const DEFAULT_COLLAPSED: Record<string, boolean> = {};

const collapseListeners = new Set<() => void>();
let collapsedCache: Record<string, boolean> | null = null;

function readCollapsed(): Record<string, boolean> {
  if (collapsedCache) return collapsedCache;
  try {
    const v = JSON.parse(localStorage.getItem(COLLAPSE_KEY) ?? "null");
    collapsedCache = v && typeof v === "object" ? { ...DEFAULT_COLLAPSED, ...v } : { ...DEFAULT_COLLAPSED };
  } catch {
    collapsedCache = { ...DEFAULT_COLLAPSED };
  }
  return collapsedCache!;
}

function setCollapsed(id: string, on: boolean) {
  collapsedCache = { ...readCollapsed(), [id]: on };
  try {
    localStorage.setItem(COLLAPSE_KEY, JSON.stringify(collapsedCache));
  } catch {
    // Remembered for this session only.
  }
  collapseListeners.forEach((l) => l());
}

function useCollapsed(): Record<string, boolean> {
  return useSyncExternalStore((l) => {
    collapseListeners.add(l);
    return () => collapseListeners.delete(l);
  }, readCollapsed);
}

const SWITCH_ICON: Record<SwitchKind, LucideIcon> = { routerOs: Router, firewalla: EthernetPort, gateway: Globe };

/** Planned launchers, listed under Coming soon (and in ⌘K) when Settings › General asks for them. */
export const UPCOMING: { label: string; key: string; icon: LucideIcon; kind?: string }[] = [
  { label: "SGLang", key: "launch-sglang", icon: Rocket },
  { label: "TensorRT-LLM", key: "launch-trtllm", icon: Gauge },
  { label: "llama.cpp", key: "launch-llamacpp", icon: Feather },
  { label: "NIM", key: "launch-nim", icon: Package },
  { label: "Ollama", key: "launch-ollama", icon: Container, kind: "ollama" },
];

/** Down, not just busy: pinned to the top of Nodes. A node retrying after a timeout stays down (one word: Offline). */
const isDown = (n: NodeSnapshot) => n.status !== "online" && linkWord(n) !== "Connecting";

/** A window kind another package may add; until then the planned window stands in (or the row hides). */
const kindOr = (kind: string, plannedKey?: string): { kind: string; params?: Params } | null =>
  isKnown(kind) ? { kind } : plannedKey ? { kind: "planned", params: { key: plannedKey } } : null;

function nodeTrailing(n: NodeSnapshot, s: DotStatus): string {
  // Planned power work in the one set of words (nodeStatus.powerLabel): "Off (planned)", never "Restarting" for a shutdown.
  const power = powerLabel(n);
  if (power) return power;
  if (n.status === "online" && (s === "online" || s === "warn" || s === "crit")) return gpuError(n) ? "GPU —" : `GPU ${Math.round(n.gpu.utilPct)}%`;
  return isDown(n) ? linkWord(n) : DISPLAY_LABEL[s];
}

function copy(text: string) {
  navigator.clipboard?.writeText(text).catch(() => {});
}

function buildSections(
  cluster: ClusterSnapshot | null,
  health: HealthState | null,
  alerts: { warn: number; crit: number } | null,
  exits: RecentExit[],
  vpn: ReturnType<typeof useVpnStatus>,
  upcoming: boolean,
  open: (kind: string, params?: Params) => void,
  endpoints: readonly Endpoint[] = [],
): Section[] {
  const nodes = cluster?.nodes ?? [];
  const failed = nodes.reduce((acc, n) => acc + (n.failedUnits?.length ?? 0), 0);
  const unitsCrit = !!health?.alerts.some((a) => a.rule === "unit-failed" && a.severity === "crit" && a.state === "firing");
  // Exits from the last 24 h that nobody has hidden; older ones are history, not news.
  const exited = exits.length;
  const alertsTarget = kindOr("alerts", "alerts")!;
  const endpointsTarget = kindOr("endpoints", "inference-endpoints")!;
  const updatesTarget = kindOr("updates", "updates")!;
  const tasksTarget = kindOr("tasks");
  // The total, tinted by the worst, so it matches the top bar's "1 critical · 2 warnings".
  const worst = alerts ? worstSeverity(alerts.crit, alerts.warn) : null;
  const alertBadge = alerts && worst
    ? { text: String(alerts.crit + alerts.warn), tone: worst, title: issueSummary(alerts.crit, alerts.warn) }
    : undefined;

  const sections: Section[] = [
    {
      id: "overview",
      label: "Overview",
      rows: [
        { id: "dashboard", label: "Dashboard", icon: LayoutDashboard, kind: "dashboard", keywords: ["overview", "cluster"] },
        { id: "topology", label: "Topology", icon: Network, kind: "topology", keywords: ["fabric", "map", "network"] },
        { id: "cluster-metrics", label: "Cluster metrics", icon: ChartLine, kind: "cluster-metrics", keywords: ["charts", "history", "heatmap", "graphs"] },
        { id: "alerts", label: "Alerts", icon: Bell, ...alertsTarget, badge: alertBadge, keywords: ["health", "notifications", "problems"] },
      ],
    },
    {
      id: "nodes",
      label: "Nodes",
      // A node that's down comes first, so it can't scroll out of sight.
      rows: [...nodes.filter(isDown), ...nodes.filter((n) => !isDown(n))].map((n): Row => {
        const s = displayStatus(n, health);
        const g = nodeGlyph(n, health, cluster);
        const ip = n.nets.find((x) => x.role === "lan" && x.ip)?.ip ?? n.host;
        return {
          id: `node:${n.id}`,
          label: n.name,
          icon: Server,
          kind: "node",
          params: { id: n.id },
          status: s,
          glyph: g === "ok" ? undefined : g,
          statusWord: nodeGlyphWord(n, g),
          trailing: nodeTrailing(n, s),
          trailingCrit: isDown(n) && n.powerState !== "off" && g !== "planned",
          keywords: [n.name, n.host, n.system?.model ?? "", ip],
          title: nodeTitle(n, g, health, endpoints, n.host),
          // The one node menu every surface uses (C4).
          menu: () => nodeMenu(n),
        };
      }),
    },
    {
      id: "workloads",
      label: "Workloads",
      rows: [
        { id: "workloads", label: "Workloads", icon: Activity, kind: "workloads", keywords: ["processes", "ports", "listening", "pid"] },
        {
          id: "containers", label: "Containers", icon: Boxes, kind: "containers", keywords: ["docker"],
          badge: exited
            ? { text: `${exited} exited`, tone: "warn", title: `Exited with an error in the last 24 hours: ${exits.map((e) => `${e.name}${e.oom ? " (out of memory)" : ` (${e.code})`}`).join(", ")}. Hide one from its row in Containers.` }
            : undefined,
        },
        {
          id: "services", label: "Services", icon: ServerCog, kind: "services", keywords: ["systemd", "units", "daemons"],
          // The tone of the unit-failed alerts it mirrors (a warning unless the rule says critical).
          badge: failed ? { text: `${failed} failed`, tone: unitsCrit ? "crit" : "warn", title: `${failed} systemd unit${failed === 1 ? "" : "s"} failed` } : undefined,
        },
        {
          id: "models", label: "Models", icon: Database, kind: "models", keywords: ["hugging face", "download", "deploy", "vllm"],
          menu: () => [
            { label: "Open Models", icon: Database, run: () => open("models") },
            { label: "Load a model…", icon: Rocket, run: () => open("models", { tab: "load" }) },
          ],
        },
        { id: "vllm", label: "vLLM", icon: Rocket, kind: "models", params: { tab: "load" }, keywords: ["deploy", "serve", "inference"], filterOnly: true },
        { id: "endpoints", label: "Endpoints", icon: Waypoints, ...endpointsTarget, keywords: ["inference", "api", "serving", "openai"] },
      ],
    },
    {
      id: "ai",
      label: "AI",
      rows: [
        { id: "chat", label: "Chat", icon: MessageSquare, kind: "chat", keywords: ["llm", "conversation"] },
        { id: "server", label: "Server", icon: RadioTower, kind: "server", keywords: ["api", "openai", "anthropic", "local"] },
        { id: "providers", label: "Providers", icon: KeyRound, kind: "providers", keywords: ["api keys", "openai", "anthropic"] },
      ],
    },
    {
      id: "network",
      label: "Network",
      rows: [
        {
          id: "vpn",
          label: "VPN",
          icon: ShieldCheck,
          kind: "vpn",
          keywords: ["wireguard", "amneziawg", "firewalla", "remote"],
          status: vpn ? VPN_DOT[vpn.state] : undefined,
          statusWord: vpn ? VPN_LABEL[vpn.state] : undefined,
          trailing: vpn ? VPN_LABEL[vpn.state] : undefined,
        },
        ...(cluster?.switches ?? []).map((s): Row => {
          const st = switchDisplayStatus(s, health);
          return {
            id: `switch:${s.id}`,
            label: s.name,
            icon: SWITCH_ICON[s.kind] ?? Router,
            kind: "switch",
            params: { id: s.id, name: s.name },
            status: st,
            statusWord: DISPLAY_LABEL[st],
            trailing: st === "online" ? "Online" : DISPLAY_LABEL[st],
            keywords: [s.name, s.host, s.kind],
            title: statusTooltip(s.name, st, health?.switches[s.id]?.issues ?? [], s.host || null),
            menu: () => [
              { label: `Open ${s.name}`, icon: SWITCH_ICON[s.kind] ?? Router, run: () => open("switch", { id: s.id, name: s.name }) },
              { separator: true },
              { label: s.host ? `Copy IP address (${s.host})` : "Copy IP address", icon: Copy, disabled: s.host ? false : "No address set", run: () => copy(s.host) },
              { label: "Edit in Settings…", icon: Settings, run: () => open("settings", { section: "switches" }) },
            ],
          };
        }),
        // ── r2-fabric ── (Network rows; e.g. a "Fabric" entry)
        fabricRow(),
        // ── r3-paging ── (Network rows)
        // ── r3-agent ── (Network rows)
        // ── r3-incidents ── (Network rows)
        // ── r3-serving ── (Network rows)
        // ── r3-charts ── (Network rows)
        // ── r3-desk ── (Network rows)
        // ── r3-safety ── (Network rows)
        // ── r4-nodes ── (Network rows)
        // ── r4-serving ── (Network rows)
        // ── r4-alerts ── (Network rows)
        // ── r4-agent ── (Network rows)
        // ── r4-safety ── (Network rows)
        // ── r4-demo ── (Network rows)
        // ── r4-craft ── (Network rows)
      ],
    },
    {
      id: "tools",
      label: "Tools",
      rows: [
        { id: "howto", label: "How To", icon: BookOpen, kind: "howto", keywords: ["skills", "playbooks", "nvidia", "tutorial", "guides", "learn"] },
        { id: "files", label: "Files", icon: FolderOpen, kind: "files", keywords: ["sftp", "upload", "download", "editor"] },
        { id: "terminal", label: "Terminal", icon: SquareTerminal, kind: "terminal", keywords: ["ssh", "shell", "console"] },
        { id: "rdp", label: "RDP", icon: Monitor, kind: "rdp", keywords: ["remote desktop", "xrdp", "gnome", "xfce"] },
        { id: "fleet", label: "Fleet Command", icon: Command, kind: "fleet", keywords: ["run", "all nodes", "broadcast"] },
        { id: "activity", label: "Activity log", icon: ScrollText, kind: "activity", keywords: ["audit", "history", "commands"] },
        { id: "updates", label: "Updates", icon: Download, ...updatesTarget, keywords: ["apt", "packages", "upgrade"], ...updatesRow(cluster) },
        ...(tasksTarget ? [{ id: "tasks", label: "Tasks", icon: ListChecks, ...tasksTarget, keywords: ["jobs", "progress", "running"] }] : []),
      ],
    },
  ];

  if (upcoming) {
    const soon = UPCOMING.filter((u) => !(u.kind && isKnown(u.kind)));
    if (soon.length) {
      sections.push({
        id: "soon",
        label: "Coming soon",
        rows: soon.map((u) => ({ id: `soon:${u.key}`, label: u.label, icon: u.icon, kind: "planned", params: { key: u.key }, title: `${u.label}: planned` })),
      });
    }
  }
  // An Ollama window, once it exists, sits with the other workloads.
  if (isKnown("ollama")) sections[2].rows.push({ id: "ollama", label: "Ollama", icon: Container, kind: "ollama", keywords: ["models", "local"] });
  // ── r4-safety ── Images, read only, right under Workloads.
  if (isKnown("images")) sections[2].rows.splice(1, 0, { id: "images", label: "Images", icon: Layers, kind: "images", keywords: ["docker", "image", "disk", "dangling", "unused"] });
  return sections;
}

// ── p1r3-check ── (helpers)
/**
 * The Updates row from each node's update facts in the snapshot (the update check, Live only):
 * "<n> security" while any security update waits, else how many are pending; the tooltip has a
 * line per node. Nothing in Demo, or before a node has been checked.
 */
export function updatesRow(cluster: ClusterSnapshot | null): Pick<Row, "badge" | "trailing" | "title"> {
  const checked = cluster && !cluster.demo ? cluster.nodes.filter((n) => n.updates) : [];
  if (!checked.length) return {};
  const security = checked.reduce((sum, n) => sum + (n.updates?.security ?? 0), 0);
  const pending = checked.reduce((sum, n) => sum + (n.updates?.pending ?? 0) + (n.updates?.engineImages ?? 0), 0);
  const title = checked.map(updatesLine).join("\n");
  if (security) return { badge: { text: `${security} security`, tone: "warn", title }, title };
  return { trailing: pending ? String(pending) : undefined, title };
}

/** "cell1 4 (3 security) · firmware: none · Ollama update available · DGX OS update 7.7.0 pending" */
function updatesLine(n: NodeSnapshot): string {
  const u = n.updates!;
  const parts = [u.pending ? `${n.name} ${u.pending}${u.security ? ` (${u.security} security)` : ""}` : `${n.name} packages up to date`];
  if (u.firmware != null) parts.push(u.firmware ? `firmware: ${u.firmware}` : "firmware: none");
  if (u.ollamaNewer) parts.push("Ollama update available");
  if (u.engineImages) parts.push(`${u.engineImages} engine image update${u.engineImages === 1 ? "" : "s"} available`);
  if (u.otaPending) parts.push(`DGX OS update ${u.otaPending} pending`);
  return parts.join(" · ");
}

// ── r3-paging ── (helpers)
// ── r3-agent ── (helpers)
// ── r3-incidents ── (helpers)
// ── r3-serving ── (helpers)
// ── r3-charts ── (helpers)
// ── r3-desk ── (helpers)
// ── r3-safety ── (helpers)

// ── r2-fabric ──
/** Fabric: quiet when lossless; amber or red with the verdict when not. */
function fabricRow(): Row {
  const f = peekFabricHealth();
  const level = f?.verdict.level;
  const loud = level === "warn" || level === "crit";
  return {
    id: "fabric",
    label: "Fabric",
    icon: FabricIcon,
    kind: "fabric",
    keywords: ["roce", "rdma", "nccl", "pfc", "ecn", "perftest", "doctor", "cabling", "test fabric"],
    status: loud ? level : undefined,
    statusWord: loud ? f?.verdict.title : undefined,
    // Known configuration (PFC off) reads grey, like "Not set up": never Warning (r4-alerts A8).
    trailing: loud ? (level === "crit" ? "Problem" : "Warning") : f?.advisory?.short,
    trailingCrit: level === "crit",
    title: f ? (f.advisory && !loud ? `${f.summary}. ${f.advisory.title}` : f.summary) : "Fabric",
  };
}

function rowMatches(r: Row, q: string): boolean {
  return [r.label, ...(r.keywords ?? [])].some((w) => w.toLowerCase().includes(q));
}

export function Sidebar() {
  const cluster = useCluster();
  const health = useHealth();
  const alerts = useHealthSummary();
  const exits = useRecentExits();
  const vpn = useVpnStatus();
  const wm = useWM();
  const upcoming = useShowUpcoming();
  const endpoints = useEndpoints();
  const collapsed = useCollapsed();
  const [filter, setFilter] = useState("");
  const navRef = useRef<HTMLElement>(null);

  const sections = useMemo(
    () => buildSections(
      cluster,
      health,
      alerts,
      exits,
      vpn,
      upcoming,
      wm.open,
      endpoints,
    ),
    [cluster, health, alerts, exits, vpn, upcoming, wm.open, endpoints],
  );

  const q = filter.trim().toLowerCase();
  const openIds = new Set(wm.windows.map((w) => w.id));
  const shown = sections
    .map((s) => ({
      ...s,
      rows: s.rows.filter((r) => (q ? rowMatches(r, q) : !r.filterOnly)),
    }))
    .filter((s) => s.rows.length > 0 || (!q && s.id === "nodes"));

  // ↑/↓ move between rows, like a source list.
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = [...(navRef.current?.querySelectorAll<HTMLButtonElement>(".src-row") ?? [])];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    e.preventDefault();
    items[Math.max(0, Math.min(items.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))]?.focus();
  };

  const onContext = (r: Row) => (e: ReactMouseEvent) => {
    const items: ContextItem[] = r.menu?.() ?? [{ label: `Open ${r.label}`, icon: r.icon, run: () => wm.open(r.kind, r.params) }];
    showContextMenu(e, items);
  };

  const settingsId = winId("settings");

  return (
    <nav className="sidebar" aria-label="Main menu" ref={navRef} onKeyDown={onKey}>
      <div className="src-filter">
        <input
          type="search"
          value={filter}
          placeholder="Filter"
          aria-label="Filter the menu"
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              setFilter("");
            } else if (e.key === "ArrowDown" || e.key === "Enter") {
              const first = navRef.current?.querySelector<HTMLButtonElement>(".src-row");
              if (!first) return;
              e.preventDefault();
              if (e.key === "Enter") first.click();
              else first.focus();
            }
          }}
        />
      </div>
      <div className="src-scroll">
        {shown.map((s) => {
          const isCollapsed = !q && !!collapsed[s.id];
          const listId = `src-${s.id}`;
          return (
            <div key={s.id} className="src-section" role="group" aria-labelledby={`${listId}-h`}>
              <div className="src-header">
                <span id={`${listId}-h`}>{s.label}</span>
                {!q && (
                  <button
                    type="button"
                    className="src-toggle"
                    aria-expanded={!isCollapsed}
                    aria-controls={listId}
                    aria-label={`${isCollapsed ? "Show" : "Hide"} ${s.label}`}
                    title={isCollapsed ? "Show" : "Hide"}
                    onClick={() => setCollapsed(s.id, !isCollapsed)}
                  >
                    <Icon icon={ChevronDown} size={12} className={isCollapsed ? "src-chev collapsed" : "src-chev"} />
                  </button>
                )}
              </div>
              {!isCollapsed && (
                <ul id={listId} className="src-list">
                  {s.rows.length === 0 && <li className="src-empty">Waiting for the cluster…</li>}
                  {s.rows.map((r) => {
                    const id = winId(r.kind, r.params);
                    const active = id === wm.activeId;
                    const isOpen = !active && openIds.has(id);
                    return (
                      <li key={r.id}>
                        <button
                          type="button"
                          className={`src-row menu-item${active ? " active" : ""}`}
                          aria-current={active ? "page" : undefined}
                          title={r.title}
                          onClick={() => wm.open(r.kind, r.params)}
                          onContextMenu={onContext(r)}
                        >
                          <span className="src-icon">
                            <Icon icon={r.icon} size={16} />
                            {r.glyph
                              ? <span className="src-dot"><StateGlyph state={r.glyph} label={r.statusWord} /></span>
                              : r.status && <span className="src-dot"><DotGlyph status={r.status} word={r.statusWord ?? statusWord(r.status)} /></span>}
                          </span>
                          <span className="src-label">{r.label}</span>
                          {r.badge && <span className={`src-badge ${r.badge.tone}`} title={r.badge.title}>{r.badge.text}</span>}
                          {!r.badge && r.trailing && <span className={`src-trailing${r.trailingCrit ? " crit-text" : ""}`}>{r.trailing}</span>}
                          {isOpen && <span className="src-open" title="Open in a window behind" aria-label="Window open" role="img" />}
                        </button>
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          );
        })}
        {q && shown.length === 0 && <p className="src-empty">Nothing matches “{filter}”.</p>}
      </div>
      <div className="src-pinned">
        <button
          type="button"
          className={`src-row menu-item${wm.activeId === "help" ? " active" : ""}`}
          aria-current={wm.activeId === "help" ? "page" : undefined}
          onClick={() => wm.open("help")}
          title="SparkBox Help (⌘?)"
        >
          <span className="src-icon"><Icon icon={CircleHelp} size={16} /></span>
          <span className="src-label">Help</span>
          <span className="src-trailing src-kbd" aria-label="Command question mark">⌘?</span>
        </button>
        <button
          type="button"
          className={`src-row menu-item${wm.activeId === settingsId ? " active" : ""}`}
          aria-current={wm.activeId === settingsId ? "page" : undefined}
          onClick={() => wm.open("settings")}
          title="Settings (⌘,)"
        >
          <span className="src-icon"><Icon icon={Settings} size={16} /></span>
          <span className="src-label">Settings</span>
          <span className="src-trailing src-kbd" aria-label="Command comma">⌘,</span>
        </button>
      </div>
    </nav>
  );
}
