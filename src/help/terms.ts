// Links the first use of each core term in an article to the glossary, so that no reader meets a
// word nobody explained (docs/help/STYLE.md, section 4). The list is docs/help/terms.json.
//
// A term isn't linked where the article defines it on the spot ("A **port** is…", "SSH, the secure
// connection…"), where it has its own glossary link, or inside a heading, a link, code, bold UI
// text or a quoted message: those are the app's words, and the first must stay exactly as they are.
import type { Token, Tokens } from "marked";

export interface TermSpec {
  anchor: string;
  forms: string[];
  /** Match case: names and abbreviations. */
  exact?: boolean;
  /** Where this word means something else: matches inside these spans don't count. */
  not?: string;
  /** Articles that are about this term. */
  skip?: string[];
  /** A regular expression: a code span it matches is this term, such as a path that starts "~/". */
  code?: string;
}

export interface Term extends TermSpec {
  find: RegExp;
  exclude: RegExp | null;
  code?: string;
  inCode: RegExp | null;
  /** The glossary's first sentence, shown when the pointer is over the link. */
  definition?: string;
}

const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export function compileTerms(specs: TermSpec[], definitions: Map<string, string> = new Map()): Term[] {
  return specs.map((t) => {
    const forms = [...t.forms].sort((a, b) => b.length - a.length).map((f) => esc(f).replace(/ /g, "\\s+")).join("|");
    return {
      ...t,
      // No forms: a term known only by its code spans.
      find: new RegExp(forms ? `(?<![\\p{L}\\p{N}_/.~$-])(?:${forms})(?![\\p{L}\\p{N}_/-])` : "(?!)", t.exact ? "gu" : "giu"),
      exclude: t.not ? new RegExp(t.not, t.exact ? "gu" : "giu") : null,
      inCode: t.code ? new RegExp(t.code) : null,
      definition: definitions.get(t.anchor),
    };
  });
}

/** "A **port** is", "SSH, the secure…", "apt is the…": the words after a term that define it. */
const DEFINES = /^(?:\*\*)?(?:,| is| are| means)\s+(?:a|an|the|one|called|how|what|where|when)\b/;
/** After bold, a comma or a verb is enough: "**tokens**, small pieces of text". */
const BOLD_DEFINES = /^(?:,| is| are| means)\s/;

/** The stretches of `text` inside double quotes. `open` carries a quote across tokens. */
function quotedSpans(text: string, open: boolean): { spans: [number, number][]; open: boolean } {
  const spans: [number, number][] = [];
  let from = open ? 0 : -1;
  for (const m of text.matchAll(/["“”]/g)) {
    const at = m.index;
    if (from < 0) from = at;
    else if (m[0] !== "“") {
      spans.push([from, at + 1]);
      from = -1;
    }
  }
  if (from >= 0) spans.push([from, text.length]);
  return { spans, open: from >= 0 };
}

const inside = (at: number, end: number, spans: [number, number][]) => spans.some(([a, b]) => at < b && end > a);

function excludedSpans(term: Term, text: string): [number, number][] {
  if (!term.exclude) return [];
  return [...text.matchAll(term.exclude)].map((m) => [m.index, m.index + m[0].length]);
}

interface Hit {
  term: Term;
  at: number;
  end: number;
  defined: boolean;
}

function firstHits(text: string, terms: Term[], quoted: [number, number][], done: Set<string>): Hit[] {
  const hits: Hit[] = [];
  for (const term of terms) {
    if (done.has(term.anchor)) continue;
    const out = excludedSpans(term, text);
    for (const m of text.matchAll(term.find)) {
      const at = m.index;
      const end = at + m[0].length;
      if (inside(at, end, quoted) || inside(at, end, out)) continue;
      hits.push({ term, at, end, defined: DEFINES.test(text.slice(end)) });
      break;
    }
  }
  // Earliest first, and the longest at the same place: "API key" before "API".
  hits.sort((a, b) => a.at - b.at || b.end - a.end);
  const kept: Hit[] = [];
  let upTo = 0;
  for (const h of hits) {
    if (h.at < upTo) continue;
    kept.push(h);
    upTo = h.end;
  }
  return kept;
}

function textToken(s: string, base: Tokens.Text): Tokens.Text {
  return { ...base, raw: s, text: s };
}

function linkToken(word: string, term: Term, base: Tokens.Text): Tokens.Link {
  return {
    type: "link",
    raw: word,
    href: `help:glossary#${term.anchor}`,
    title: term.definition ?? null,
    text: word,
    tokens: [textToken(word, base)],
  };
}

function linkInline(tokens: Token[], terms: Term[], done: Set<string>): Token[] {
  const out: Token[] = [];
  let quote = false;
  let raw = 0;
  tokens.forEach((t, i) => {
    if (t.type === "html") {
      if (/^<kbd\b/i.test(t.raw)) raw++;
      else if (/^<\/kbd/i.test(t.raw)) raw--;
    }
    if (t.type === "strong") {
      // "A **port** is a numbered door": this is the definition, so nothing needs linking.
      const after = tokens[i + 1];
      const word = (t as Tokens.Strong).text.toLowerCase();
      if (after?.type === "text" && BOLD_DEFINES.test((after as Tokens.Text).text)) {
        for (const term of terms) if (term.forms.some((f) => f.toLowerCase() === word)) done.add(term.anchor);
      }
    }
    if (t.type === "link" && (t as Tokens.Link).href.startsWith("help:glossary#")) done.add((t as Tokens.Link).href.slice(14).split(/[?&]/)[0]);
    if (t.type === "codespan" && !raw && !quote) {
      // Code stays as it is, unless the whole span is a term: `~/models`, `/dev/shm`.
      const term = terms.find((x) => !done.has(x.anchor) && x.inCode?.test((t as Tokens.Codespan).text));
      if (term) {
        done.add(term.anchor);
        out.push({ type: "link", raw: t.raw, href: `help:glossary#${term.anchor}`, title: term.definition ?? null, text: t.raw, tokens: [t] });
        return;
      }
    }
    if (t.type !== "text" || raw > 0) {
      out.push(t);
      return;
    }
    const base = t as Tokens.Text;
    const { spans, open } = quotedSpans(base.text, quote);
    quote = open;
    const hits = firstHits(base.text, terms, spans, done);
    let from = 0;
    for (const h of hits) {
      done.add(h.term.anchor);
      if (h.defined) continue;
      if (h.at > from) out.push(textToken(base.text.slice(from, h.at), base));
      out.push(linkToken(base.text.slice(h.at, h.end), h.term, base));
      from = h.end;
    }
    if (from === 0) out.push(t);
    else if (from < base.text.length) out.push(textToken(base.text.slice(from), base));
  });
  return out;
}

function linkBlocks(tokens: Token[], terms: Term[], done: Set<string>) {
  for (const t of tokens) {
    switch (t.type) {
      case "paragraph":
        t.tokens = linkInline(t.tokens ?? [], terms, done);
        break;
      case "text":
        if (t.tokens) t.tokens = linkInline(t.tokens, terms, done);
        break;
      case "blockquote":
        linkBlocks((t as Tokens.Blockquote).tokens, terms, done);
        break;
      case "list":
        for (const item of (t as Tokens.List).items) linkBlocks(item.tokens, terms, done);
        break;
      case "table":
        for (const row of (t as Tokens.Table).rows) for (const cell of row) cell.tokens = linkInline(cell.tokens, terms, done);
        break;
    }
  }
}

/** Adds the links to an article's tokens, in reading order. `id` is the article being rendered. */
export function linkFirstUses(tokens: Token[], terms: Term[], id?: string) {
  const here = id ? terms.filter((t) => !t.skip?.includes(id)) : terms;
  linkBlocks(tokens, here, new Set());
}
