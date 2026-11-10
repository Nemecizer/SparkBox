// The index of Help: the manifest and the tips, without the articles. It is a chunk of its own,
// loaded through nav.ts after startup, so info icons and the command palette can name things
// without the app paying for the content up front.
import manifestJson from "../../docs/help/manifest.json";
import tipsJson from "../../docs/help/tips.json";
import type { ArticleMeta, ArticleType, Manifest, ResolvedTip, Tip, TocSection } from "./types";

export const manifest = manifestJson as unknown as Manifest;
export const tips = tipsJson as Record<string, Tip>;

const byId = new Map(manifest.articles.map((a) => [a.id, a]));

export const articleMeta = (id: string): ArticleMeta | undefined => byId.get(id);

export const TYPE_LABEL: Record<ArticleType, string> = {
  tutorial: "Tutorial",
  howto: "How-To",
  feature: "Feature guide",
  concept: "Concept",
  reference: "Reference",
  troubleshooting: "Troubleshooting",
};

/** Words a heading writes in capitals, which its anchor can't show. */
const SHOUTED: Record<string, string> = { cpu: "CPU", gpu: "GPU", mlx: "MLX", roce: "RoCE" };

/** "read-the-fit-table" → "Read the fit table": a heading's name, without loading the article. */
export function headingName(anchor: string): string {
  const words = anchor.split("-").map((w) => SHOUTED[w] ?? w);
  const first = words[0];
  return [first[0].toUpperCase() + first.slice(1), ...words.slice(1)].join(" ");
}

/** "node-window#gpu" → the article and the heading. */
export function splitTopic(topic: string): { id: string; anchor?: string } {
  const at = topic.indexOf("#");
  return at < 0 ? { id: topic } : { id: topic.slice(0, at), anchor: topic.slice(at + 1) || undefined };
}

const HOWTO_TYPES: ArticleType[] = ["howto", "tutorial"];
const MAX_HOWTOS = 3;

function howtosOf(ids: string[], self?: string): ResolvedTip["howtos"] {
  const out: ResolvedTip["howtos"] = [];
  for (const id of ids) {
    const a = byId.get(id);
    if (a && a.id !== self && HOWTO_TYPES.includes(a.type)) out.push({ id: a.id, title: a.title });
    if (out.length === MAX_HOWTOS) break;
  }
  return out;
}

/**
 * What an info icon shows for a topic: a named tip (ids with a dot), or an article, with or
 * without a heading. Null when the topic names nothing; the help lint keeps that from shipping.
 */
export function resolveTopic(topic: string): ResolvedTip | null {
  const tip = tips[topic];
  if (tip) {
    const { id, anchor } = splitTopic(tip.article);
    const article = byId.get(id);
    return { title: tip.title, name: tip.title, body: tip.body, article: id, anchor, howtos: howtosOf(tip.howto ?? article?.related ?? [], id) };
  }
  const { id, anchor } = splitTopic(topic);
  const article = byId.get(id);
  if (!article) return null;
  return { title: article.title, name: anchor ? headingName(anchor) : article.title, body: article.summary, article: id, anchor, howtos: howtosOf(article.related, id) };
}

export interface TocPlace {
  section: TocSection;
  group: TocSection["groups"][number];
}

const places = new Map<string, TocPlace>();
for (const section of manifest.toc) {
  for (const group of section.groups) {
    for (const id of group.articles) if (!places.has(id)) places.set(id, { section, group });
  }
}

/** Where an article sits in the table of contents. */
export const tocPlace = (id: string): TocPlace | undefined => places.get(id);

export const tocSection = (id: string): TocSection | undefined => manifest.toc.find((s) => s.id === id);

/** The articles before and after this one in its group, for reading straight through. */
export function neighbors(id: string): { prev?: ArticleMeta; next?: ArticleMeta } {
  const list = places.get(id)?.group.articles ?? [];
  const i = list.indexOf(id);
  return { prev: i > 0 ? byId.get(list[i - 1]) : undefined, next: i >= 0 ? byId.get(list[i + 1]) : undefined };
}
