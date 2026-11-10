// The articles, read from docs/help through Vite. Everything here sits in the Help window's
// own chunk: nothing is fetched, and nothing loads until Help is opened.
import { parse as parseYaml } from "yaml";
import phrasesJson from "../../docs/help/phrases.json";
import termsJson from "../../docs/help/terms.json";
import { parseGlossary, type Definition } from "./definitions";
import { articleMeta, manifest } from "./meta";
import { firstSentences, plainDoc, renderHelp, type Rendered } from "./render";
import type { SearchDoc } from "./search";
import { compileTerms, type Term, type TermSpec } from "./terms";
import type { ArticleMeta } from "./types";

const RAW = import.meta.glob("../../docs/help/*/*.md", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const IMAGES = import.meta.glob("../../docs/help/img/*.{jpg,jpeg,png,svg}", { query: "?url&no-inline", import: "default", eager: true }) as Record<string, string>;

const FRONT = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/;

export interface Article {
  meta: ArticleMeta;
  /** The Markdown, without its frontmatter. */
  body: string;
  /** "start/getting-started.md" */
  file: string;
  front: Record<string, unknown>;
}

function parse(path: string, source: string): Article | null {
  const m = FRONT.exec(source);
  if (!m) return null;
  let front: Record<string, unknown>;
  try {
    front = (parseYaml(m[1]) ?? {}) as Record<string, unknown>;
  } catch {
    return null;
  }
  const known = articleMeta(String(front.id ?? ""));
  if (!known) return null;
  const list = (v: unknown) => (Array.isArray(v) ? v.map(String) : []);
  return {
    file: path.replace(/^.*docs\/help\//, ""),
    body: source.slice(m[0].length),
    front,
    meta: {
      ...known,
      keywords: [...new Set([...known.keywords, ...list(front.keywords)])],
      related: [...new Set([...known.related, ...list(front.related)])].filter((id) => !!articleMeta(id)),
    },
  };
}

const articles = new Map<string, Article>();
for (const [path, source] of Object.entries(RAW)) {
  const a = parse(path, source);
  if (a) articles.set(a.meta.id, a);
}

export const getArticle = (id: string): Article | undefined => articles.get(id);
export const allArticles = (): Article[] => [...articles.values()];
/** Every Markdown file under docs/help's folders, parsed or not. */
export const rawFiles = (): Record<string, string> => RAW;

/** "img/x.jpg" → its bundled URL. */
export function imageUrl(src: string): string | null {
  const name = src.replace(/^(\.\/)?img\//, "");
  const key = Object.keys(IMAGES).find((k) => k.endsWith(`/img/${name}`));
  return key ? IMAGES[key] : null;
}

export const termSpecs = termsJson.terms as TermSpec[];

let glossaryTerms: Term[] | null = null;

/** The core terms, each with the first sentence of its glossary entry. */
export function coreTerms(): Term[] {
  const glossary = articles.get("glossary");
  glossaryTerms ??= compileTerms(termSpecs, glossary ? firstSentences(glossary.body) : undefined);
  return glossaryTerms;
}

let definitions: Map<string, Definition> | null = null;

/** A Glossary entry by its heading's anchor: what a defined term shows when it's clicked. */
export function glossaryEntry(anchor: string): Definition | undefined {
  definitions ??= parseGlossary(articles.get("glossary")?.body ?? "");
  return definitions.get(anchor);
}

const rendered = new Map<string, Rendered>();

export function renderArticle(id: string): Rendered | null {
  const hit = rendered.get(id);
  if (hit) return hit;
  const a = articles.get(id);
  if (!a) return null;
  // The glossary explains the words, so it doesn't link them to itself.
  const out = renderHelp(a.body, imageUrl, id === "glossary" ? undefined : { terms: coreTerms(), id });
  rendered.set(id, out);
  return out;
}

const phrases = phrasesJson.phrases as Record<string, string[]>;

/** What search reads: every article in the manifest, with its text when it has been written. */
export function searchDocs(): SearchDoc[] {
  return manifest.articles.map((m) => {
    const a = articles.get(m.id);
    const doc = a ? plainDoc(a.body) : { text: "", headings: [] };
    return {
      id: m.id,
      title: m.title,
      type: m.type,
      summary: m.summary,
      keywords: a?.meta.keywords ?? m.keywords,
      phrases: phrases[m.id],
      headings: doc.headings,
      body: doc.text,
      testBuild: m.testBuild,
    };
  });
}
