// The sections of the Settings window. Each package appends its own entry
// below its marker; the window sorts by `order` (gaps leave room between).
//   general 10 · appearance 20 · login 30 · nodes 40 · switches 50 · data 60 ·
//   scenarios 65 · notifications 70 · terminal 80 · models 90 · security 95 ·
//   diagnostics 100
import type { ComponentType } from "react";
import { Database, KeyRound, Palette, Router, Server, Settings2, Stethoscope, type LucideIcon } from "lucide-react";
import { AppearanceSection } from "./AppearanceSection";
import { DataSourceSection } from "./DataSourceSection";
import { DiagnosticsSection } from "./DiagnosticsSection";
import { GeneralSection } from "./GeneralSection";
import { LoginSection } from "./LoginSection";
import { NodesSection } from "./NodesSection";
import { SwitchesSection } from "./SwitchesSection";
import { FlaskConical } from "lucide-react";
import { ScenariosSection } from "./ScenariosSection";
import { Boxes } from "lucide-react";
import { ModelsSection } from "./ModelsSection";
// ── P5 monitoring ──
// ── r2-monitor ──
import { SlidersHorizontal } from "lucide-react";
import { AlertRulesSection } from "./AlertRulesSection";
import { Bell } from "lucide-react";
import { NotificationsSection } from "./NotificationsSection";
import { LockKeyhole } from "lucide-react";
import { SecuritySection } from "./SecuritySection";
import { SquareTerminal } from "lucide-react";
import { TerminalSection } from "./TerminalSection";
// ── r3-charts ──
// ── r3-paging ──
// ── r3-incidents ──
// ── r3-serving ──
// ── r3-desk ──
// ── r3-safety ──
// ── r4-nodes ──
// ── r4-serving ──
// ── r4-alerts ──
// ── r4-agent ──
// ── r4-safety ──
// ── r4-demo ──
// ── r4-craft ──
// ── p1r1-x ──
import { ShieldCheck } from "lucide-react";
import { FenceSection } from "./FenceSection";

export interface SettingsSection {
  id: string;
  label: string;
  icon: LucideIcon;
  order: number;
  component: ComponentType;
  /** What the info button beside the heading explains: an article id (settingsHelp in docs/help/manifest.json). */
  help: string;
  /** Extra words the section search matches, e.g. "password" for Cluster login. */
  keywords?: string[];
  /** Background of the icon tile in the section list (white glyph on it); defaults to the accent. */
  tint?: string;
}

export const SECTIONS: SettingsSection[] = [
  // ── P1 design ──
  // ── r2-design ──
  { id: "general", label: "General", icon: Settings2, order: 10, help: "settings-general", component: GeneralSection, keywords: ["menu bar", "login", "startup", "upcoming", "coming soon"], tint: "#6e7480" },
  { id: "appearance", label: "Appearance", icon: Palette, order: 20, help: "settings-appearance", component: AppearanceSection, keywords: ["theme", "dark", "light", "black", "oled", "text size", "font", "motion", "contrast", "accessibility"], tint: "#5856d6" },
  { id: "login", label: "Cluster login", icon: KeyRound, order: 30, help: "settings-cluster-login", component: LoginSection, keywords: ["password", "keychain", "user", "ssh", "sudo", "account"], tint: "#b7791f" },
  { id: "nodes", label: "Nodes", icon: Server, order: 40, help: "settings-nodes", component: NodesSection, keywords: ["inventory", "hostname", "ip", "port", "spark", "add node", "test"], tint: "#1f6bc8" },
  { id: "switches", label: "Switches & gateway", icon: Router, order: 50, help: "settings-switches", component: SwitchesSection, keywords: ["crs804", "mikrotik", "routeros", "rest", "firewalla", "gateway", "network"], tint: "#0f8a6c" },
  { id: "data", label: "Data source", icon: Database, order: 60, help: "settings-data-source", component: DataSourceSection, keywords: ["demo", "live", "simulator", "connect"], tint: "#7a4fd0" },
  { id: "diagnostics", label: "Diagnostics", icon: Stethoscope, order: 100, help: "settings-diagnostics", component: DiagnosticsSection, keywords: ["doctor", "report", "host keys", "known hosts", "debug"], tint: "#c2410c" },
  // ── P3 ops ──
  // ── r2-safety ──
  { id: "security", label: "Security & privacy", icon: LockKeyhole, order: 95, help: "settings-security", component: SecuritySection, keywords: ["admin", "lock", "unlock", "touch id", "activity log", "audit", "retention", "privacy"], tint: "#3a6fb0" },
  // ── P4 world ──
  // ── r2-fabric ──
  { id: "scenarios", label: "Demo scenarios", icon: FlaskConical, order: 65, help: "settings-demo-scenarios", component: ScenariosSection, keywords: ["demo", "scenario", "simulate", "failure", "offline", "crash", "test"], tint: "#b83280" },
  // ── P5 monitoring ──
  // ── r2-monitor ──
  { id: "notifications", label: "Notifications", icon: Bell, order: 70, help: "settings-notifications", component: NotificationsSection, keywords: ["alerts", "notify", "quiet hours", "critical", "banner", "ntfy", "pushover", "webhook", "slack", "email", "phone", "agent", "24/7", "prometheus"], tint: "#d83a3a" },
  { id: "alerts", label: "Alerts", icon: SlidersHorizontal, order: 72, help: "settings-alerts", component: AlertRulesSection, keywords: ["rules", "thresholds", "alert rules", "memory", "disk", "gpu", "xid", "ttft", "slo", "override"], tint: "#b4531a" },
  // ── P6 term-files ──
  // ── r2-live ──
  { id: "terminal", label: "Terminal", icon: SquareTerminal, order: 80, help: "settings-terminal", component: TerminalSection, keywords: ["font", "theme", "cursor", "scrollback", "option key", "meta", "copy on select", "ssh", "shell"], tint: "#3b4250" },
  // ── P7 ai ──
  // ── r2-ai ──
  { id: "models", label: "Models", icon: Boxes, order: 90, help: "settings-models", component: ModelsSection, keywords: ["vllm", "image", "hugging face", "token", "model folder", "advisor", "ai"], tint: "#0e7490" },
  // ── r3-charts ──
  // ── r3-paging ──
  // ── r3-incidents ──
  // ── r3-serving ──
  // ── r3-desk ──
  // ── r3-safety ──
  // ── r4-nodes ──
  // ── r4-serving ──
  // ── r4-alerts ──
  // ── r4-agent ──
  // ── r4-safety ──
  // ── r4-demo ──
  // ── r4-craft ──
  // ── p1r1-x ──
  { id: "fence", label: "Fence", icon: ShieldCheck, order: 97, help: "settings-fence", component: FenceSection, keywords: ["fence", "write fence", "lease", "controller", "identity", "pin", "machine-id", "test build", "dev build"], tint: "#2f6f4f" },
];

/** Sections in display order. */
export function sortedSections(): SettingsSection[] {
  return [...SECTIONS].sort((a, b) => a.order - b.order);
}

/** Matches the Settings search against a section's label and keywords. */
export function sectionMatches(s: SettingsSection, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [s.label, ...(s.keywords ?? [])].some((w) => w.toLowerCase().includes(q));
}
