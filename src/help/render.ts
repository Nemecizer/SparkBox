// Turns a Help article into safe HTML. The approach is src/lib/markdown.ts's (marked, then
// DOMPurify), with a profile for trusted, bundled pages: bundled images and help:, open: links
// are allowed, and blockquotes that start with [!NOTE], [!TIP], [!WARNING] or [!DANGER] become
// callouts. Heading anchors follow docs/help/STYLE.md, section 8.
import DOMPurify from "dompurify";
import { Lexer, Marked, type Token, type Tokens } from "marked";
import { linkFirstUses, type Term } from "./terms";

export interface Heading {
  level: number;
  text: string;
  anchor: string;
  /** Where it starts in the plain text of the article. */
  at: number;
}

export interface Rendered {
  html: string;
  headings: Heading[];
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** A heading's anchor: lower case, spaces as hyphens, punctuation removed, "&" read as "and". */
export function slug(text: string): string {
  return text
    .toLowerCase()
    .replace(/&/g, " and ")
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .trim()
    .replace(/[\s-]+/g, "-");
}

/** Anchors for an article's headings, in order. A repeated heading gets "-2", "-3". */
function anchorFor(seen: Map<string, number>, text: string): string {
  const base = slug(text);
  const n = (seen.get(base) ?? 0) + 1;
  seen.set(base, n);
  return n === 1 ? base : `${base}-${n}`;
}

/** The text of inline tokens, without their formatting. */
function inlineText(tokens: Token[] | undefined): string {
  return (tokens ?? [])
    .map((t) => {
      if ("tokens" in t && t.tokens?.length) return inlineText(t.tokens);
      if (t.type === "html") return "";
      if (t.type === "br") return " ";
      return "text" in t ? String(t.text) : "";
    })
    .join("")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'");
}

const CALLOUT = /^\[!(NOTE|TIP|WARNING|DANGER)\][ \t]*\n?/;
const CALLOUT_LABEL: Record<string, string> = { NOTE: "Note", TIP: "Tip", WARNING: "Warning", DANGER: "Danger" };

/** Lucide's Info, Lightbulb, TriangleAlert and OctagonAlert, set after sanitizing (no SVG gets through it). */
const svg = (d: string) =>
  `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${d}</svg>`;
const CALLOUT_ICON: Record<string, string> = {
  NOTE: svg('<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>'),
  TIP: svg('<path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5"/><path d="M9 18h6"/><path d="M10 22h4"/>'),
  WARNING: svg('<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>'),
  DANGER: svg('<path d="M12 16h.01"/><path d="M12 8v4"/><path d="M15.31 2a2 2 0 0 1 1.41.59l4.69 4.69a2 2 0 0 1 .59 1.41v6.62a2 2 0 0 1-.59 1.41l-4.69 4.69a2 2 0 0 1-1.41.59H8.69a2 2 0 0 1-1.41-.59l-4.69-4.69A2 2 0 0 1 2 15.31V8.69a2 2 0 0 1 .59-1.41l4.69-4.69A2 2 0 0 1 8.69 2z"/>'),
};

/** What one render collects, and how it finds images. Rendering is synchronous, so one is enough. */
let pass: { headings: Omit<Heading, "at">[]; seen: Map<string, number>; image: (src: string) => string | null } | null = null;

function figure(src: string, caption: string): string {
  const url = pass?.image(src) ?? null;
  const dark = pass?.image(src.replace(/\.(jpg|svg)$/, "-dark.$1")) ?? null;
  const label = escapeHtml(caption);
  // A picture still to be taken is marked for writers, and readers of the app never see the gap.
  if (!url) return import.meta.env.PROD ? "" : `<figure class="help-figure missing"><figcaption>${label}</figcaption></figure>`;
  // Pictures hold twice the pixels of the screen they show. Marked 2x, a page shows them at their
  // real size, sharp on a Retina screen; enlarged, they use every pixel. Diagrams are drawn the same size.
  const shot = (at: string, theme: string) =>
    `<img class="help-shot${theme}" src="${escapeHtml(at)}" data-retina="${escapeHtml(at)}" alt="${label}" tabindex="0" role="button" title="Click to enlarge">`;
  // Each theme gets its own picture; help.css shows the one that matches.
  const shots = dark ? shot(url, " for-light") + shot(dark, " for-dark") : shot(url, "");
  return `<figure class="help-figure">${shots}<figcaption>${label}</figcaption></figure>`;
}

const md = new Marked({
  gfm: true,
  renderer: {
    heading(t: Tokens.Heading) {
      // The Help window draws the title itself.
      if (t.depth === 1) return "";
      const text = inlineText(t.tokens);
      const anchor = pass ? anchorFor(pass.seen, text) : slug(text);
      pass?.headings.push({ level: t.depth, text, anchor });
      return `<h${t.depth} data-anchor="${escapeHtml(anchor)}">${this.parser.parseInline(t.tokens)}</h${t.depth}>\n`;
    },
    paragraph(t: Tokens.Paragraph) {
      // A picture alone on its line is a figure with a caption, never inside a <p>.
      const only = t.tokens.length === 1 ? t.tokens[0] : null;
      if (only?.type === "image") return figure((only as Tokens.Image).href, (only as Tokens.Image).text) + "\n";
      return false;
    },
    image(t: Tokens.Image) {
      return figure(t.href, t.text);
    },
    blockquote(t: Tokens.Blockquote) {
      const first = t.tokens[0];
      const lead = first?.type === "paragraph" ? (first as Tokens.Paragraph).tokens[0] : undefined;
      const m = lead?.type === "text" ? CALLOUT.exec((lead as Tokens.Text).text) : null;
      if (!m) return false;
      const rest = (lead as Tokens.Text).text.slice(m[0].length);
      const para = first as Tokens.Paragraph;
      const tokens = rest ? t.tokens : t.tokens.slice(1);
      if (rest) para.tokens = [{ ...lead, text: rest, raw: rest } as Tokens.Text, ...para.tokens.slice(1)];
      else if (para.tokens.length > 1) tokens.unshift({ ...para, tokens: para.tokens.slice(1).filter((x, i) => i > 0 || x.type !== "br") });
      const kind = m[1];
      return `<div class="help-callout ${kind.toLowerCase()}" role="note"><div class="help-callout-title"><span data-callout="${kind}"></span>${CALLOUT_LABEL[kind]}</div>${this.parser.parse(tokens)}</div>\n`;
    },
    list(t: Tokens.List) {
      if (!t.ordered) return false;
      const start = typeof t.start === "number" ? t.start : 1;
      const items = t.items.map((item, i) => `<li data-n="${start + i}">${this.parser.parse(item.tokens)}</li>\n`).join("");
      return `<ol class="help-steps">\n${items}</ol>\n`;
    },
    code(t: Tokens.Code) {
      return `<div class="code-block"><div class="code-head"><span>${escapeHtml(t.lang ?? "")}</span><span data-copy=""></span></div><pre><code>${escapeHtml(t.text)}</code></pre></div>\n`;
    },
    link(t: Tokens.Link) {
      const inner = this.parser.parseInline(t.tokens);
      const href = escapeHtml(t.href);
      if (t.href.startsWith("open:")) return `<a class="help-open" href="${href}">${inner}</a>`;
      if (/^https?:/i.test(t.href)) return `<a class="help-ext" href="${href}" title="Opens in your browser">${inner}</a>`;
      if (t.href.startsWith("help:glossary#")) return `<a class="help-term" href="${href}"${t.title ? ` title="${escapeHtml(t.title)}"` : ""}>${inner}</a>`;
      return `<a href="${href}">${inner}</a>`;
    },
  },
});

const PURIFY = {
  FORBID_TAGS: ["svg", "math", "iframe", "form", "style", "input", "button", "textarea", "select", "video", "audio", "object", "embed"],
  FORBID_ATTR: ["style", "srcset", "action", "formaction"],
  // A picture is reachable from the keyboard: it opens at full size.
  ADD_ATTR: ["tabindex"],
  ADD_URI_SAFE_ATTR: ["tabindex"],
  ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|help:|open:|#|data:image\/|\/|\.{1,2}\/)/i,
};

/**
 * Renders an article's Markdown. `image` gives the bundled URL of "img/x.jpg", or null. With
 * `glossary`, the first use of each core term links to its entry (terms.ts).
 */
export function renderHelp(source: string, image: (src: string) => string | null = () => null, glossary?: { terms: Term[]; id?: string }): Rendered {
  pass = { headings: [], seen: new Map(), image };
  const tokens = md.lexer(source);
  if (glossary) linkFirstUses(tokens, glossary.terms, glossary.id);
  const raw = md.parser(tokens);
  const found = pass.headings;
  pass = null;
  const clean = DOMPurify.sanitize(raw, PURIFY) as string;
  const html = clean
    // Put SparkBox's own pieces back, now the content has been cleaned.
    .replace(/ data-retina="([^"]*)"/g, ' srcset="$1 2x"')
    .replace(/<span data-copy=""><\/span>/g, '<button type="button" class="code-copy">Copy</button>')
    .replace(/<span data-callout="(NOTE|TIP|WARNING|DANGER)"><\/span>/g, (_, kind: string) => CALLOUT_ICON[kind])
    .replace(/<table>/g, '<div class="help-table"><table>')
    .replace(/<\/table>/g, "</table></div>");
  return { html, headings: found.map((h) => ({ ...h, at: 0 })) };
}

/** Words in a block's tokens, in reading order. */
function blockText(tokens: Token[]): string {
  const out: string[] = [];
  for (const t of tokens) {
    switch (t.type) {
      case "paragraph":
      case "text":
        out.push("tokens" in t && t.tokens ? inlineText(t.tokens) : (t as Tokens.Text).text);
        break;
      case "code":
        out.push((t as Tokens.Code).text);
        break;
      case "blockquote":
        out.push(blockText((t as Tokens.Blockquote).tokens).replace(CALLOUT, ""));
        break;
      case "list":
        for (const item of (t as Tokens.List).items) out.push(blockText(item.tokens));
        break;
      case "table": {
        const table = t as Tokens.Table;
        for (const row of [table.header, ...table.rows]) out.push(row.map((c) => inlineText(c.tokens)).join(" · "));
        break;
      }
    }
  }
  return out.join("\n");
}

/** An article as plain text with its headings' places in it: what search reads. */
export function plainDoc(source: string): { text: string; headings: Heading[] } {
  const seen = new Map<string, number>();
  const headings: Heading[] = [];
  let text = "";
  for (const t of new Lexer({ gfm: true }).lex(source)) {
    if (t.type === "heading") {
      const h = t as Tokens.Heading;
      if (h.depth === 1) continue;
      const title = inlineText(h.tokens);
      headings.push({ level: h.depth, text: title, anchor: anchorFor(seen, title), at: text.length });
      text += `${title}\n`;
    } else if (t.type !== "space") {
      const part = blockText([t]);
      if (part) text += `${part}\n`;
    }
  }
  return { text, headings };
}

export const headingsOf = (source: string) => plainDoc(source).headings;

/** The first sentence under each heading, by anchor: the glossary's short definitions. */
export function firstSentences(source: string): Map<string, string> {
  const seen = new Map<string, number>();
  const out = new Map<string, string>();
  let at: string | null = null;
  for (const t of new Lexer({ gfm: true }).lex(source)) {
    if (t.type === "heading") {
      at = (t as Tokens.Heading).depth === 1 ? null : anchorFor(seen, inlineText((t as Tokens.Heading).tokens));
    } else if (t.type === "paragraph" && at) {
      const text = inlineText((t as Tokens.Paragraph).tokens);
      out.set(at, /^.*?[.!?](?=\s|$)/s.exec(text)?.[0] ?? text);
      at = null;
    }
  }
  return out;
}
