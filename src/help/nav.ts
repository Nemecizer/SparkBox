// The light side of Help, in the main bundle: opening the window, and loading the index
// (meta.ts) when it's first wanted. Nothing here imports the content.
import { useSyncExternalStore } from "react";
import { BookOpen, CircleHelp, Compass, FileText, Lightbulb, ListChecks, Wrench, type LucideIcon } from "lucide-react";
import { registerCommands, type Command } from "../lib/commands";
import { openWindow } from "../wm/WindowManager";
import type { ArticleType } from "./types";

type Meta = typeof import("./meta");

let meta: Meta | null = null;
let loading: Promise<Meta> | null = null;
const listeners = new Set<() => void>();

/** The Help index, loading it on first use. */
export function loadHelpMeta(): Promise<Meta> {
  loading ??= import("./meta").then((m) => {
    meta = m;
    listeners.forEach((l) => l());
    return m;
  });
  return loading;
}

/** The Help index once it has loaded, else null. */
export function useHelpMeta(): Meta | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => meta,
  );
}

let scheduled = false;

/** Loads the index soon after startup, so an info icon can name itself and opens at once. */
export function preloadHelpMeta() {
  if (scheduled || meta) return;
  scheduled = true;
  window.setTimeout(() => void loadHelpMeta().catch(() => {}), 1500);
}

/** Opens Help where it was left (no topic), or at an article, "id#heading". */
export function openHelp(topic?: string) {
  if (!topic) return openWindow("help");
  const at = topic.indexOf("#");
  openWindow("help", at < 0 ? { article: topic } : { article: topic.slice(0, at), anchor: topic.slice(at + 1) });
}

/** Opens Help on a section's page of articles. */
export function openHelpSection(id: string) {
  openWindow("help", { section: id });
}

/** Opens Help on the results for a search. */
export function searchHelp(q: string) {
  openWindow("help", { q });
}

/** "open:models?tab=load" → the window and its params. */
export function parseOpenLink(href: string): { kind: string; params: Record<string, string> } | null {
  const m = /^open:([a-z-]+)(?:\?(.*))?$/.exec(href);
  if (!m) return null;
  return { kind: m[1], params: Object.fromEntries(new URLSearchParams(m[2] ?? "")) };
}

const TYPE_ICON: Record<ArticleType, LucideIcon> = {
  tutorial: Compass, howto: ListChecks, feature: BookOpen, concept: Lightbulb, reference: FileText, troubleshooting: Wrench,
};

/** The palette's "Help" group: every article, listed once the palette has been opened. */
export const HELP_GROUP = "Help";

let commands = false;

/** Adds the articles to the command palette. They stay for the rest of the session. */
export function ensureHelpCommands() {
  if (commands) return;
  commands = true;
  void loadHelpMeta().then(({ manifest, TYPE_LABEL }) => {
    const cmds: Command[] = manifest.articles.map((a) => ({
      id: `help-article:${a.id}`,
      title: a.title,
      group: HELP_GROUP,
      groupUnsearched: true,
      subtitle: `${TYPE_LABEL[a.type]} · ${a.summary}`,
      icon: TYPE_ICON[a.type] ?? CircleHelp,
      keywords: a.keywords,
      run: () => openHelp(a.id),
    }));
    registerCommands(cmds);
  }).catch(() => {
    commands = false;
  });
}
