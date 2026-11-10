import type { ComponentType } from "react";
import {
  Activity, Boxes, Command, Database, FileSearch, FolderOpen, KeyRound, LayoutDashboard, MessageSquare, Network,
  RadioTower, Router, ScrollText, Server, Settings as SettingsIcon, ShieldCheck, SlidersHorizontal, Sparkles,
  SquareTerminal, Monitor, type LucideIcon,
} from "lucide-react";
import { setViewParams, type Params, type WinState } from "../wm/WindowManager";
import { ActivityLog } from "./ActivityLog";
import { ChatWindow } from "./ChatWindow";
import { Containers } from "./Containers";
import { Dashboard } from "./Dashboard";
import { FilesWindow } from "./FilesWindow";
import { FleetCommand } from "./FleetCommand";
import { InspectWindow } from "./InspectWindow";
import { ModelsWindow } from "./ModelsWindow";
import { NodeDetail } from "./NodeDetail";
import { Planned } from "./Planned";
import { PLANNED } from "./plannedWindows";
import { ProvidersWindow } from "./ProvidersWindow";
import { ServerWindow } from "./ServerWindow";
import { Services } from "./Services";
import { Settings } from "./Settings";
import { SwitchWindow } from "./SwitchWindow";
import { RdpWindow } from "./RdpWindow";
import { HowToWindow } from "./HowToWindow";
import { BookOpen } from "lucide-react";
import { TerminalWindow } from "./TerminalWindow";
import { Topology } from "./Topology";
import { VpnWindow } from "./VpnWindow";
import { Workloads } from "./Workloads";
import { TasksWindow } from "./TasksWindow";
import { UpdatesWindow } from "./UpdatesWindow";
import { ListChecks, PackageCheck } from "lucide-react";
// ── P5 monitoring ──
import { Bell, ChartLine } from "lucide-react";
import { AlertsWindow } from "./AlertsWindow";
import { ClusterMetrics } from "./ClusterMetrics";
// ── P6 term-files ──
import { FilePen } from "lucide-react";
import { EditorWindow } from "./EditorWindow";
// ── P7 ai ──
import { Container, Cpu } from "lucide-react";
import { EndpointsWindow } from "./EndpointsWindow";
import { OllamaWindow } from "./OllamaWindow";
import { Cable } from "lucide-react";
import { ConnectWizard } from "./ConnectWizard";
// ── r2-shell ── (imports)
import { getCluster } from "../lib/store";
import { windowTitle } from "../lib/vocabulary";
// ── r2-design ── (imports)
// ── r2-monitor ── (imports)
// ── r2-ai ── (imports)
// ── r2-fabric ── (imports)
import { FabricWindow } from "./FabricWindow";
// ── r2-live ── (imports)
// ── r2-safety ── (imports)
// ── r3-paging ── (imports)
// ── r3-agent ── (imports)
// ── r3-incidents ── (imports)
// ── r3-serving ── (imports)
// ── r3-charts ── (imports)
// ── r3-desk ── (imports)
// ── r3-safety ── (imports)
// ── r4-nodes ── (imports)
// ── r4-serving ── (imports)
// ── r4-alerts ── (imports)
// ── r4-agent ── (imports)
// ── r4-safety ── (imports)
import { Layers } from "lucide-react";
import { Images } from "./Images";
// ── r4-demo ── (imports)
// ── r4-craft ── (imports)
import { CircleHelp } from "lucide-react";
import { HelpHost } from "../help/HelpHost";

export interface WindowDef {
  title: (p: Params) => string;
  /** Default size when it first opens. */
  size: [number, number];
  /** Smallest size it can be resized to (default 360×200). */
  minSize?: [number, number];
  /**
   * Smallest size Tile may give it, where its compact layout still works (the Dashboard's
   * compact tiles, Files' single pane, Services' detail under the list). Default: its minSize,
   * capped at TILE_MIN. Below this Tile sends it to the dock instead.
   */
  tileMin?: [number, number];
  /** Shown before the title, in the dock, the Windows menu and the palette. */
  icon?: LucideIcon;
  /**
   * What the title bar's info button explains: an article id from docs/help/manifest.json
   * (its windowHelp map). The help lint keeps the two the same. `helpFor` overrides it for a
   * window whose params make it a different thing.
   */
  help: string;
  helpFor?: (p: Params) => string | undefined;
  /**
   * Params it can't open without (e.g. a node id). Kinds that list any aren't
   * offered as plain "Open …" commands in the palette.
   */
  needs?: string[];
  /**
   * Params that pick a view inside it (a tab, a node filter) rather than which window it is.
   * A link with one brings the open window forward on that view instead of opening a copy.
   */
  views?: string[];
  component: ComponentType<{ params: Params }>;
  /** Ignored: every window stays mounted while minimized now. */
  keepMounted?: boolean;
}

/** A node's name from the inventory ("cell1"), never a stale param. */
const nodeName = (id: string) => getCluster()?.nodes.find((n) => n.id === id)?.name ?? id;
/** The Firewalla devices have a view of their own: no ports to manage from it. */
const switchHelp = (p: Params) => (getCluster()?.switches.find((s) => s.id === p.id)?.kind ?? "routerOs") === "routerOs" ? undefined : "firewalla-view";
/** A switch's inventory name ("CRS804 DDQ"), never its id. */
const switchName = (p: Params) => getCluster()?.switches.find((s) => s.id === p.id)?.name ?? p.name ?? "Switch";

// Each package adds its window kinds directly below its own marker. Give each an icon and a
// minSize, and list required params in `needs` (e.g. an editor's ["place", "path"]). Titles
// equal the sidebar labels (vocabulary.ts); scope like "all nodes" isn't part of a title.
export const REGISTRY: Record<string, WindowDef> = {
  // ── P2 shell ──
  dashboard: { title: () => windowTitle("dashboard"), size: [1180, 820], minSize: [460, 240], tileMin: [460, 380], icon: LayoutDashboard, help: "dashboard", component: Dashboard },
  node: { title: (p) => nodeName(p.id), size: [1000, 680], minSize: [460, 260], tileMin: [460, 360], icon: Server, needs: ["id"], views: ["tab"], help: "node-window", component: NodeDetail },
  containers: { title: () => windowTitle("containers"), size: [1160, 440], minSize: [460, 240], tileMin: [460, 280], icon: Boxes, views: ["node"], help: "containers", component: Containers },
  topology: { title: () => windowTitle("topology"), size: [980, 560], minSize: [400, 300], tileMin: [400, 300], icon: Network, help: "topology", component: Topology },
  switch: { title: switchName, size: [1060, 700], minSize: [520, 260], icon: Router, needs: ["id"], views: ["port"], help: "switch-window", helpFor: switchHelp, component: SwitchWindow },
  chat: { title: () => windowTitle("chat"), size: [1080, 720], minSize: [560, 360], icon: MessageSquare, help: "chat", component: ChatWindow },
  server: { title: () => windowTitle("server"), size: [980, 760], minSize: [560, 360], icon: RadioTower, help: "server", component: ServerWindow },
  providers: { title: () => windowTitle("providers"), size: [860, 660], minSize: [520, 320], icon: KeyRound, help: "providers", component: ProvidersWindow },
  vpn: { title: () => windowTitle("vpn"), size: [880, 680], minSize: [520, 360], icon: ShieldCheck, help: "vpn", component: VpnWindow },
  models: { title: () => "Models", size: [1180, 720], minSize: [680, 400], icon: Database, views: ["tab", "edit"], help: "models-window", component: ModelsWindow },
  rdp: { title: () => windowTitle("rdp"), size: [960, 720], minSize: [580, 360], icon: Monitor, help: "rdp", component: RdpWindow },
  terminal: { title: () => windowTitle("terminal"), size: [960, 600], minSize: [400, 220], tileMin: [400, 220], icon: SquareTerminal, help: "terminal", component: TerminalWindow },
  files: { title: () => windowTitle("files"), size: [1180, 680], minSize: [460, 320], tileMin: [460, 340], icon: FolderOpen, views: ["place", "path"], help: "files", component: FilesWindow },
  workloads: { title: () => windowTitle("workloads"), size: [1240, 580], minSize: [460, 280], tileMin: [460, 280], icon: Activity, help: "workloads", component: Workloads },
  services: { title: () => windowTitle("services"), size: [1180, 680], minSize: [460, 300], tileMin: [460, 300], icon: SlidersHorizontal, views: ["node", "unit"], help: "services", component: Services },
  fleet: { title: () => windowTitle("fleet"), size: [1180, 640], minSize: [600, 340], icon: Command, help: "fleet-command", component: FleetCommand },
  activity: { title: () => windowTitle("activity"), size: [1100, 560], minSize: [460, 280], tileMin: [460, 280], icon: ScrollText, views: ["op"], help: "activity-log", component: ActivityLog },
  inspect: { title: (p) => `${p.name ?? p.id} on ${nodeName(p.node)}`, size: [820, 620], minSize: [480, 300], icon: FileSearch, needs: ["node", "id"], help: "inspect-window", component: InspectWindow },
  settings: { title: () => "Settings", size: [960, 660], minSize: [620, 400], icon: SettingsIcon, views: ["section"], help: "settings-window", component: Settings },
  planned: { title: (p) => PLANNED[p.key]?.title ?? "Planned", size: [560, 320], minSize: [400, 220], icon: Sparkles, needs: ["key"], help: "whats-coming", component: Planned },
  // ── P1 design ──
  // ── P3 ops ──
  tasks: { title: () => "Tasks", size: [1040, 600], minSize: [620, 320], icon: ListChecks, help: "tasks", component: TasksWindow },
  updates: { title: () => windowTitle("updates"), size: [1120, 680], minSize: [680, 420], icon: PackageCheck, views: ["node"], help: "updates", component: UpdatesWindow },
  howto: { title: () => windowTitle("howto"), size: [1120, 780], minSize: [620, 420], icon: BookOpen, views: ["guide"], help: "how-to-library", component: HowToWindow },
  // ── P4 world ──
  connect: { title: () => "Connect my Sparks", size: [900, 660], minSize: [640, 460], icon: Cable, help: "connect-my-sparks", component: ConnectWizard },
  // ── P5 monitoring ──
  alerts: { title: () => "Alerts", size: [1000, 520], minSize: [460, 280], tileMin: [460, 280], icon: Bell, help: "alerts-window", component: AlertsWindow },
  "cluster-metrics": { title: () => "Cluster metrics", size: [1120, 720], minSize: [560, 360], icon: ChartLine, help: "cluster-metrics", component: ClusterMetrics },
  // ── P6 term-files ──
  editor: {
    title: (p) => `${(p.path ?? "").replace(/\/+$/, "").split("/").pop() || "Editor"} — ${p.place === "mac" ? "This Mac" : p.place}`,
    size: [1000, 660], minSize: [480, 300], icon: FilePen, needs: ["place", "path"], help: "editor", component: EditorWindow,
  },
  // ── P7 ai ──
  endpoints: { title: () => "Endpoints", size: [1100, 720], minSize: [460, 300], tileMin: [460, 300], icon: Cpu, views: ["id"], help: "endpoints", component: EndpointsWindow },
  ollama: { title: () => "Ollama", size: [980, 600], minSize: [560, 320], icon: Container, views: ["endpoint"], help: "ollama", component: OllamaWindow },
  // ── r2-shell ──
  // ── r2-design ──
  // ── r2-monitor ──
  // ── r2-ai ──
  // ── r2-fabric ──
  fabric: { title: () => windowTitle("fabric"), size: [1120, 720], minSize: [620, 360], icon: Cable, views: ["tab", "cell"], help: "fabric", component: FabricWindow },
  // ── r2-live ──
  // ── r2-safety ──
  // ── r3-paging ──
  // ── r3-agent ──
  // ── r3-incidents ──
  // ── r3-serving ──
  // ── r3-charts ──
  // ── r3-desk ──
  // ── r3-safety ──
  // ── r4-nodes ──
  // ── r4-serving ──
  // ── r4-alerts ──
  // ── r4-agent ──
  // ── r4-safety ──
  images: { title: () => windowTitle("images"), size: [1080, 540], minSize: [460, 280], tileMin: [460, 280], icon: Layers, views: ["node"], help: "images", component: Images },
  // ── r4-demo ──
  // ── r4-craft ──
  help: {
    title: () => "Help", size: [1180, 760], minSize: [480, 340], tileMin: [460, 320], icon: CircleHelp,
    views: ["article", "anchor", "section", "q"], help: "using-help", component: HelpHost,
  },
};

setViewParams(Object.fromEntries(Object.entries(REGISTRY).map(([kind, def]) => [kind, def.views])));

const DEFAULT_MIN: [number, number] = [360, 200];
/** The most a kind without its own tileMin asks of Tile: two sit side by side on a 1440×900 screen. */
const TILE_MIN: [number, number] = [460, 300];

export const sizeOf = (kind: string): [number, number] => REGISTRY[kind]?.size ?? [600, 400];
export const minSizeOf = (kind: string): [number, number] => REGISTRY[kind]?.minSize ?? DEFAULT_MIN;
export const tileMinOf = (kind: string): [number, number] => {
  const [w, h] = minSizeOf(kind);
  return REGISTRY[kind]?.tileMin ?? [Math.min(w, TILE_MIN[0]), Math.min(h, TILE_MIN[1])];
};
export const iconOf = (kind: string): LucideIcon | undefined => REGISTRY[kind]?.icon;
export const helpOf = (kind: string, params: Params = {}): string | undefined => REGISTRY[kind]?.helpFor?.(params) ?? REGISTRY[kind]?.help;
export const isKnown = (kind: string) => kind in REGISTRY;
export const titleOf = (w: Pick<WinState, "kind" | "params">) => REGISTRY[w.kind]?.title(w.params) ?? w.kind;
/** Kinds the palette can open without params. */
export const openableKinds = () => Object.keys(REGISTRY).filter((k) => !REGISTRY[k].needs?.length);

export function WindowBody({ kind, params }: { kind: string; params: Params }) {
  const Component = REGISTRY[kind]?.component;
  return Component ? <Component params={params} /> : null;
}
