// What a glossary link shows when it's clicked: the whole entry, so the reader stays where they
// are. Each "### Term" of the Glossary is one definition.
import { Lexer, type Tokens } from "marked";
import { renderHelp, slug } from "./render";

export interface Definition {
  term: string;
  anchor: string;
  /** The entry as safe HTML. Its links work like any link in Help. */
  html: string;
}

const clean = (heading: string) => heading.replace(/\*\*|`/g, "");

/** Every entry of the Glossary, by the anchor of its heading. */
export function parseGlossary(source: string): Map<string, Definition> {
  const seen = new Map<string, number>();
  const anchorOf = (text: string) => {
    const base = slug(text);
    const n = (seen.get(base) ?? 0) + 1;
    seen.set(base, n);
    return n === 1 ? base : `${base}-${n}`;
  };
  const raw: { term: string; anchor: string; text: string }[] = [];
  let open: (typeof raw)[number] | null = null;
  for (const t of new Lexer({ gfm: true }).lex(source)) {
    if (t.type !== "heading") {
      if (open) open.text += t.raw;
      continue;
    }
    const h = t as Tokens.Heading;
    open = null;
    if (h.depth === 1) continue;
    const anchor = anchorOf(clean(h.text));
    if (h.depth === 3) raw.push((open = { term: clean(h.text), anchor, text: "" }));
  }
  const byAnchor = new Map(raw.map((e) => [e.anchor, e]));
  return new Map(raw.map((e) => {
    // "See [always-on agent](#always-on-agent)." and nothing else: show what it points to.
    const only = /^\s*See \[[^\]]*\]\(#([^)]+)\)\.\s*$/.exec(e.text);
    const text = (only && byAnchor.get(only[1])?.text) || e.text;
    return [e.anchor, { term: e.term, anchor: e.anchor, html: renderHelp(text.trim().replace(/\]\(#/g, "](help:glossary#")).html }];
  }));
}
