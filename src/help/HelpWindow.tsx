// SparkBox Help: a normal inner window with a table of contents, instant search, Back and
// Forward, and articles from docs/help. Nothing here reaches the network, a node or the core.
// The window loads as a chunk of its own (HelpHost.tsx), content included.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft, ArrowRight, BookA, BookOpen, ChevronRight, Compass, FileText, House, Lightbulb, ListChecks, PanelLeft, SearchX, ShieldQuestion, Wrench,
  type LucideIcon,
} from "lucide-react";
import { Icon } from "../components/Icon";
import { EmptyState, SearchField, Toolbar, ToolButton } from "../components/ui";
import { announce } from "../lib/announce";
import { useViewLink, useWindowTitle, useWinState, useWMActions, type Params } from "../wm/WindowManager";
import { getArticle, glossaryEntry, renderArticle, searchDocs } from "./content";
import { PictureView, TermPopover } from "./HelpPopovers";
import { articleMeta, manifest, neighbors, splitTopic, tocPlace, tocSection, TYPE_LABEL } from "./meta";
import { parseOpenLink } from "./nav";
import { slug } from "./render";
import { buildIndex, landing, matchedTerms, searchOrNearest, segments, snippet, type Hit, type Index, type Seg } from "./search";
import type { ArticleMeta, HelpMenuEntry } from "./types";
import "./help.css";

/** One place in Help. No fields at all is the home page. */
interface Loc {
  article?: string;
  section?: string;
  anchor?: string;
  /** The results for a search. */
  q?: string;
  /** Words to mark in the article (it was opened from a search). */
  hl?: string;
  /** How far it was scrolled when the reader left. */
  top?: number;
}

interface Nav {
  stack: Loc[];
  at: number;
}

const HOME: Loc = {};
const MAX_HISTORY = 60;
const RESULTS_SHOWN = 40;
/** With more headings than this, "On this page" lists only the main ones, so it stays a map. */
const FULL_OUTLINE = 40;
const LAST_KEY = "sparkbox.help.last";
const TOC_KEY = "sparkbox.help.toc";

function readLocal<T>(key: string): T | null {
  try {
    const v = localStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : null;
  } catch {
    return null;
  }
}

function writeLocal(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Only a convenience: Help opens on its home page next time.
  }
}

const exists = (l: Loc) => (!l.article || !!articleMeta(l.article)) && (!l.section || !!tocSection(l.section));

function locFromParams(p: Params): Loc | null {
  if (p.q) return { q: p.q };
  if (p.article) return { article: p.article, anchor: p.anchor || undefined };
  if (p.section) return { section: p.section, anchor: p.anchor || undefined };
  return null;
}

/** Where a new Help window starts: where a link asked, else where the reader left off. */
function startNav(p: Params): Nav {
  const asked = locFromParams(p);
  const last = readLocal<Loc>(LAST_KEY);
  const start = asked && exists(asked) ? asked : last && exists(last) ? last : HOME;
  return { stack: [start], at: 0 };
}

let sharedIndex: Index | null = null;
const searchIndex = (): Index => (sharedIndex ??= buildIndex(searchDocs()));

const SECTION_ICON: Record<string, LucideIcon> = {
  "getting-started": Compass, "how-to": ListChecks, features: BookOpen, concepts: Lightbulb, reference: FileText, troubleshooting: Wrench, glossary: BookA,
};

const SECTION_BLURB: Record<string, string> = {
  "getting-started": "Learn SparkBox by doing, from the first launch to your first model.",
  "how-to": "Step-by-step instructions for every main task.",
  features: "What each window and setting does.",
  concepts: "The ideas behind SparkBox, in plain words.",
  reference: "Shortcuts, menus, status words and other facts to look up.",
  troubleshooting: "What a message means, and how to fix it.",
  glossary: "The technical words you meet in SparkBox, explained.",
};

const START_HERE = ["getting-started", "tour-the-demo", "your-first-model", "connect-your-cluster"];

/** The How-Tos the Help menu lists: the main ones. */
function mainHowTos(): ArticleMeta[] {
  const menu = manifest.helpMenu.find((e) => e.label === "How-To Guides")?.submenu ?? [];
  return menu.flatMap((e: HelpMenuEntry) => (e.article ? [articleMeta(e.article)] : [])).filter((a): a is ArticleMeta => !!a);
}

// ---------------------------------------------------------------- table of contents

interface Row {
  key: string;
  level: number;
  label: string;
  article?: string;
  section?: string;
  /** Undefined for a row that doesn't fold. */
  expanded?: boolean;
  parent?: string;
}

function tocRows(open: Set<string>): Row[] {
  const rows: Row[] = [];
  for (const s of manifest.toc) {
    const key = `s:${s.id}`;
    const ids = s.groups.flatMap((g) => g.articles);
    if (ids.length === 1) {
      rows.push({ key: `a:${ids[0]}`, level: 1, label: s.title, article: ids[0] });
      continue;
    }
    const sOpen = open.has(key);
    rows.push({ key, level: 1, label: s.title, section: s.id, expanded: sOpen });
    if (!sOpen) continue;
    for (const g of s.groups) {
      if (!g.title) {
        for (const id of g.articles) rows.push({ key: `a:${id}`, level: 2, label: articleMeta(id)?.title ?? id, article: id, parent: key });
        continue;
      }
      const gKey = `g:${s.id}:${g.title}`;
      const gOpen = open.has(gKey);
      rows.push({ key: gKey, level: 2, label: g.title, expanded: gOpen, parent: key });
      if (gOpen) for (const id of g.articles) rows.push({ key: `a:${id}`, level: 3, label: articleMeta(id)?.title ?? id, article: id, parent: gKey });
    }
  }
  return rows;
}

function Toc({ current, section, onArticle, onSection }: { current?: string; section?: string; onArticle: (id: string) => void; onSection: (id: string) => void }) {
  const [open, setOpen] = useState<Set<string>>(() => new Set(readLocal<string[]>(TOC_KEY) ?? ["s:getting-started"]));
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const rows = useMemo(() => tocRows(open), [open]);
  const here = current ? `a:${current}` : section ? `s:${section}` : null;

  const change = (next: Set<string>) => {
    setOpen(next);
    writeLocal(TOC_KEY, [...next]);
  };
  const toggle = (key: string) => {
    const next = new Set(open);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    change(next);
  };

  // The way to the article being read is always open.
  useEffect(() => {
    const place = current ? tocPlace(current) : undefined;
    if (!place) return;
    const need = [`s:${place.section.id}`, ...(place.group.title ? [`g:${place.section.id}:${place.group.title}`] : [])];
    if (!need.every((k) => open.has(k))) change(new Set([...open, ...need]));
  }, [current]);

  useEffect(() => {
    if (!here) return;
    listRef.current?.querySelector(`[data-key="${here.replace(/"/g, '\\"')}"]`)?.scrollIntoView({ block: "nearest" });
  }, [here, rows.length]);

  const activate = (r: Row) => {
    if (r.article) onArticle(r.article);
    else if (r.section) {
      onSection(r.section);
      if (!r.expanded) toggle(r.key);
    } else toggle(r.key);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const key = (e.target as HTMLElement).closest<HTMLElement>("[data-key]")?.dataset.key;
    const i = rows.findIndex((r) => r.key === key);
    if (i < 0) return;
    const r = rows[i];
    const moveTo = (k: string | undefined) => {
      if (!k) return;
      setFocusKey(k);
      requestAnimationFrame(() => listRef.current?.querySelector<HTMLElement>(`[data-key="${k.replace(/"/g, '\\"')}"]`)?.focus());
    };
    let handled = true;
    if (e.key === "ArrowDown") moveTo(rows[i + 1]?.key);
    else if (e.key === "ArrowUp") moveTo(rows[i - 1]?.key);
    else if (e.key === "Home") moveTo(rows[0]?.key);
    else if (e.key === "End") moveTo(rows[rows.length - 1]?.key);
    else if (e.key === "ArrowRight") {
      if (r.expanded === false) toggle(r.key);
      else if (r.expanded) moveTo(rows[i + 1]?.key);
    } else if (e.key === "ArrowLeft") {
      if (r.expanded) toggle(r.key);
      else moveTo(r.parent);
    } else if (e.key === "Enter" || e.key === " ") activate(r);
    else handled = false;
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  const tabStop = rows.some((r) => r.key === focusKey) ? focusKey : rows.some((r) => r.key === here) ? here : rows[0]?.key;

  return (
    <div className="help-tree" role="tree" aria-label="Help contents" ref={listRef} onKeyDown={onKeyDown}>
      {rows.map((r) => (
        <div
          key={r.key}
          data-key={r.key}
          role="treeitem"
          aria-level={r.level}
          aria-expanded={r.expanded}
          aria-selected={r.key === here}
          tabIndex={r.key === tabStop ? 0 : -1}
          className={`help-row level-${r.level}${r.key === here ? " on" : ""}${r.article ? "" : " fold"}`}
          onClick={() => {
            setFocusKey(r.key);
            activate(r);
          }}
        >
          {r.expanded !== undefined && (
            <span
              className={`help-twist${r.expanded ? " open" : ""}`}
              aria-hidden
              onClick={(e) => {
                e.stopPropagation();
                toggle(r.key);
              }}
            >
              <Icon icon={ChevronRight} size={12} />
            </span>
          )}
          <span className="help-row-label">{r.label}</span>
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------- pages

function ArticleLink({ a }: { a: ArticleMeta }) {
  return (
    <a className="help-card" href={`help:${a.id}`}>
      <span className="help-card-title">{a.title}</span>
      <span className="help-card-sub">{a.summary}</span>
    </a>
  );
}

function Home() {
  return (
    <div className="help-page wide">
      <div className="help-article">
        <h1>SparkBox Help</h1>
        <p className="help-lede">Everything SparkBox does, in plain language. Start with a guide, or search for what you want to do.</p>
        <h2 className="help-h">Start here</h2>
        <div className="help-cards">
          {START_HERE.map((id) => articleMeta(id)).filter((a): a is ArticleMeta => !!a).map((a) => <ArticleLink key={a.id} a={a} />)}
        </div>
        <h2 className="help-h">Browse Help</h2>
        <div className="help-cards">
          {manifest.toc.map((s) => {
            const n = s.groups.reduce((sum, g) => sum + g.articles.length, 0);
            const only = n === 1 ? s.groups[0].articles[0] : null;
            return (
              <a key={s.id} className="help-card" href={only ? `help:${only}` : `helpsection:${s.id}`}>
                <span className="help-card-title"><Icon icon={SECTION_ICON[s.id] ?? BookOpen} size={16} />{s.title}</span>
                <span className="help-card-sub">{SECTION_BLURB[s.id] ?? ""}</span>
              </a>
            );
          })}
        </div>
        <h2 className="help-h">Main How-Tos</h2>
        <ul className="help-links">
          {mainHowTos().map((a) => <li key={a.id}><a href={`help:${a.id}`}>{a.title}</a></li>)}
        </ul>
      </div>
    </div>
  );
}

function SectionPage({ id }: { id: string }) {
  const s = tocSection(id);
  if (!s) return null;
  const titled = s.groups.flatMap((g) => (g.title ? [g.title] : []));
  return (
    <div className="help-page wide">
      <div className="help-article">
        <h1>{s.title}</h1>
        {SECTION_BLURB[id] && <p className="help-lede">{SECTION_BLURB[id]}</p>}
        {titled.length > 1 && (
          <nav className="help-jump" aria-label={`Groups in ${s.title}`}>
            {titled.map((title) => <a key={title} href={`#${slug(title)}`}>{title}</a>)}
          </nav>
        )}
        {s.groups.map((g) => (
          <section key={g.title ?? "all"}>
            {g.title && <h2 className="help-h" data-anchor={slug(g.title)}>{g.title}</h2>}
            {g.lead && <p className="help-group-lead">New to this? Start with <a href={`help:${g.lead}`}>{articleMeta(g.lead)?.title}</a>.</p>}
            <div className="help-cards">
              {g.articles.map((a) => articleMeta(a)).filter((a): a is ArticleMeta => !!a).map((a) => <ArticleLink key={a.id} a={a} />)}
            </div>
          </section>
        ))}
      </div>
    </div>
  );
}

function Marked({ segs }: { segs: Seg[] }) {
  return <>{segs.map((s, i) => (s.hit ? <mark key={i}>{s.text}</mark> : <span key={i}>{s.text}</span>))}</>;
}

function Result({ hit, query, n }: { hit: Hit; query: string; n: number }) {
  const place = tocPlace(hit.doc.id);
  const { segs } = useMemo(() => snippet(hit), [hit]);
  // Opens at a section only when that is what was asked for; otherwise at the title.
  const anchor = useMemo(() => landing(hit, query), [hit, query]);
  const heading = anchor ? hit.doc.headings.find((h) => h.anchor === anchor)?.text : undefined;
  const crumbs = [place?.section.title, place?.group.title].filter(Boolean).join(" › ");
  return (
    <li>
      <a className="help-result" data-result={n} data-hl={query} href={`help:${hit.doc.id}${anchor ? `#${anchor}` : ""}`}>
        <span className="help-result-title"><Marked segs={segments(hit.doc.title, hit.terms)} /></span>
        <span className="help-result-where">
          {crumbs}{heading ? <> › <Marked segs={segments(heading, hit.terms)} /></> : null}
        </span>
        <span className="help-result-text"><Marked segs={segs} /></span>
      </a>
    </li>
  );
}

function Results({ query: asked }: { query: string }) {
  const ix = searchIndex();
  const { hits: all, instead } = useMemo(() => searchOrNearest(ix, asked), [ix, asked]);
  const query = instead ?? asked;
  const [everything, setEverything] = useState(false);
  useEffect(() => setEverything(false), [query]);
  const hits = everything ? all : all.slice(0, RESULTS_SHOWN);
  const partial = all.length > 0 && all.every((h) => h.partial);
  useEffect(() => {
    announce(all.length ? `${all.length} result${all.length === 1 ? "" : "s"}` : "No results");
  }, [all.length, query]);
  if (!all.length) {
    return (
      <div className="help-page wide">
        <EmptyState icon={SearchX} title={`Nothing in Help matches “${asked.trim()}”`} body="Try fewer words, or the words SparkBox shows on screen." />
      </div>
    );
  }
  return (
    <div className="help-page wide">
      <div className="help-article">
        <h1 className="help-results-title">Results for “{query.trim()}”</h1>
        <p className="help-results-count">
          {hits.length < all.length ? `Showing the top ${hits.length} of ${all.length} articles` : all.length === 1 ? "1 article" : `${all.length} articles`}
          {partial ? ". None has all those words, so these have some of them." : ""}
          {instead ? `. Nothing matches “${asked.trim()}”, so these are for the nearest spelling.` : ""}
        </p>
        <ol className="help-results">
          {hits.map((h, i) => <Result key={h.doc.id} hit={h} query={query} n={i} />)}
        </ol>
        {hits.length < all.length && (
          <button type="button" className="help-more" onClick={() => setEverything(true)}>Show all {all.length} articles</button>
        )}
      </div>
    </div>
  );
}

/** Wraps the words a search found in <mark>, without touching the article's markup. */
function markTerms(root: HTMLElement, terms: Set<string>) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode()) nodes.push(walker.currentNode as Text);
  for (const node of nodes) {
    if (node.parentElement?.closest(".code-head, .help-callout-title")) continue;
    const segs = segments(node.data, terms);
    if (!segs.some((s) => s.hit)) continue;
    const frag = document.createDocumentFragment();
    for (const s of segs) {
      if (!s.hit) {
        frag.append(s.text);
        continue;
      }
      const mark = document.createElement("mark");
      mark.className = "help-hit";
      mark.textContent = s.text;
      frag.append(mark);
    }
    node.replaceWith(frag);
  }
}

function Prose({ html, terms }: { html: string; terms: Set<string> | null }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.innerHTML = html;
    if (terms?.size) markTerms(el, terms);
  }, [html, terms]);
  return <div ref={ref} className="help-prose" />;
}

function OnThisPage({ headings, active, inline }: { headings: { level: number; text: string; anchor: string }[]; active: string | null; inline?: boolean }) {
  const list = (
    <ul>
      {headings.map((h) => (
        <li key={h.anchor} className={`level-${h.level}${h.anchor === active && !inline ? " on" : ""}`}>
          <a href={`#${h.anchor}`} aria-current={h.anchor === active && !inline ? "location" : undefined}>{h.text}</a>
        </li>
      ))}
    </ul>
  );
  if (inline) {
    return (
      <details className="help-otp-inline">
        <summary>On this page</summary>
        {list}
      </details>
    );
  }
  return (
    <aside className="help-otp" aria-label="On this page">
      <div className="help-otp-title">On this page</div>
      {list}
    </aside>
  );
}

function ArticlePage({ id, terms, active }: { id: string; terms: Set<string> | null; active: string | null }) {
  const meta = articleMeta(id);
  const article = getArticle(id);
  const rendered = useMemo(() => renderArticle(id), [id]);
  if (!meta) return null;
  const related = (article?.meta.related ?? meta.related).map((r) => articleMeta(r)).filter((a): a is ArticleMeta => !!a);
  const { prev, next } = neighbors(id);
  const outline = (rendered?.headings ?? []).filter((h) => h.level <= 3);
  const headings = outline.length > FULL_OUTLINE ? outline.filter((h) => h.level === 2) : outline;
  return (
    <div className="help-page">
      <article className="help-article" data-help-article={id}>
        <p className="help-eyebrow">{TYPE_LABEL[meta.type]}</p>
        <h1>{meta.title}</h1>
        {meta.verified === false && (
          <p className="help-calm"><Icon icon={ShieldQuestion} size={14} />{manifest.calmLine}</p>
        )}
        {headings.length >= 2 && <OnThisPage headings={headings} active={active} inline />}
        {rendered
          ? <Prose html={rendered.html} terms={terms} />
          : <p className="help-lede">{meta.summary} The full guide for this page hasn't been written yet.</p>}
        {related.length > 0 && (
          <section className="help-related">
            <h2 className="help-h">Related articles</h2>
            <div className="help-cards">{related.map((a) => <ArticleLink key={a.id} a={a} />)}</div>
          </section>
        )}
        {(prev || next) && (
          <nav className="help-pager" aria-label="Next and previous article">
            {prev ? <a href={`help:${prev.id}`}><span>Previous</span>{prev.title}</a> : <span />}
            {next ? <a className="next" href={`help:${next.id}`}><span>Next</span>{next.title}</a> : <span />}
          </nav>
        )}
      </article>
      {headings.length >= 2 && <OnThisPage headings={headings} active={active} />}
    </div>
  );
}

// ---------------------------------------------------------------- the window

function Crumbs({ loc, go }: { loc: Loc; go: (l: Loc) => void }) {
  const items: { label: string; to?: Loc }[] = [{ label: "Help", to: HOME }];
  const place = loc.article ? tocPlace(loc.article) : undefined;
  if (place) {
    const n = place.section.groups.reduce((sum, g) => sum + g.articles.length, 0);
    if (n > 1) items.push({ label: place.section.title, to: { section: place.section.id } });
    if (place.group.title) items.push({ label: place.group.title, to: { section: place.section.id, anchor: slug(place.group.title) } });
  }
  const here = loc.article ? articleMeta(loc.article)?.title : loc.section ? tocSection(loc.section)?.title : loc.q !== undefined ? "Search results" : null;
  if (here) items.push({ label: here });
  const last = items.length - 1;
  return (
    <nav className="help-crumbs" aria-label="Breadcrumb">
      <ol>
        {items.map((c, i) => (
          <li key={`${c.label}${i}`}>
            {i > 0 && <Icon icon={ChevronRight} size={12} />}
            {c.to && i < last
              ? <button type="button" onClick={() => go(c.to!)}>{c.label}</button>
              : <span aria-current={i === last ? "page" : undefined}>{c.label}</span>}
          </li>
        ))}
      </ol>
    </nav>
  );
}

const keyOf = (l: Loc) => `${l.article ?? ""}|${l.section ?? ""}|${l.q === undefined ? "" : "?"}`;

/** The same page at the same heading: going there again is no step to go Back over. */
const samePlace = (a: Loc, b: Loc) => keyOf(a) === keyOf(b) && (a.q ?? "") === (b.q ?? "") && (a.anchor ?? "") === (b.anchor ?? "");

export default function HelpWindow({ params }: { params: Params }) {
  const wm = useWMActions();
  const initial = useMemo(() => startNav(params), []);
  const [nav, setNav] = useWinState<Nav>("nav", initial);
  // A saved layout can name an article that's gone since.
  const here = nav.stack[nav.at];
  const loc = here && exists(here) ? here : HOME;
  const [query, setQuery] = useState(loc.q ?? loc.hl ?? "");
  const [tocOpen, setTocOpen] = useState(false);
  const [active, setActive] = useState<string | null>(null);
  const [term, setTerm] = useState<{ anchor: string; from: HTMLElement } | null>(null);
  const [picture, setPicture] = useState<{ src: string; caption: string; from: HTMLElement } | null>(null);
  const mainRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLDivElement>(null);
  const tocRef = useRef<HTMLElement>(null);
  const contentsRef = useRef<HTMLSpanElement>(null);
  /** The Contents panel closed on the way to a page: its row can't hold focus any more. */
  const returnToPage = useRef(false);
  const meta = loc.article ? articleMeta(loc.article) : undefined;

  useWindowTitle(meta ? `Help — ${meta.title}` : null);

  const scrollTop = () => mainRef.current?.scrollTop ?? 0;

  const atNow = useRef(here);
  atNow.current = here;

  const go = useCallback((next: Loc, how: "push" | "replace" = "push") => {
    const top = scrollTop();
    setTocOpen((open) => {
      if (open) returnToPage.current = true;
      return false;
    });
    const again = how === "push" && !!atNow.current && samePlace(atNow.current, next);
    // No new page draws, so this is the only thing that moves.
    if (again && !(next.anchor && toAnchor(next.anchor)) && mainRef.current) mainRef.current.scrollTop = 0;
    setNav((n) => {
      const stack = n.stack.slice(0, n.at + 1);
      if (how === "replace" || again) {
        stack[n.at] = next;
        return { stack, at: n.at };
      }
      stack[n.at] = { ...stack[n.at], top };
      stack.push(next);
      const kept = stack.slice(-MAX_HISTORY);
      return { stack: kept, at: kept.length - 1 };
    });
  }, [setNav]);

  const move = useCallback((by: 1 | -1) => {
    const top = scrollTop();
    setNav((n) => {
      const at = Math.max(0, Math.min(n.stack.length - 1, n.at + by));
      if (at === n.at) return n;
      const stack = [...n.stack];
      stack[n.at] = { ...stack[n.at], top };
      return { stack, at };
    });
  }, [setNav]);

  useViewLink(params, () => {
    const next = locFromParams(params);
    if (!next || !exists(next)) return;
    if (next.q) setQuery(next.q);
    go(next);
  });

  const pageKey = `${nav.at}|${nav.stack.length}|${keyOf(loc)}`;

  /** Brings a heading to the top, and shows briefly which one it was. */
  const toAnchor = (anchor: string) => {
    const el = mainRef.current;
    const target = [...(el?.querySelectorAll<HTMLElement>("[data-anchor]") ?? [])].find((h) => h.dataset.anchor === anchor);
    if (!el || !target) return false;
    el.scrollTop += target.getBoundingClientRect().top - el.getBoundingClientRect().top - 12;
    target.classList.remove("help-target");
    void target.offsetWidth;
    target.classList.add("help-target");
    return true;
  };

  // Every move lands at the top, on its heading, or (Back) where the reader was.
  useLayoutEffect(() => {
    const el = mainRef.current;
    if (!el) return;
    const place = () => {
      if (loc.top !== undefined) el.scrollTop = loc.top;
      else if (!(loc.anchor && toAnchor(loc.anchor))) el.scrollTop = 0;
    };
    place();
    setTerm(null);
    setQuery(loc.q ?? loc.hl ?? "");
    if (!document.activeElement || document.activeElement === document.body || returnToPage.current) el.focus({ preventScroll: true });
    returnToPage.current = false;
    if (loc.anchor || loc.top) {
      // A picture that loads above the place pushes it down: follow it for a moment.
      const pending = [...el.querySelectorAll("img")].filter((i) => !i.complete);
      const until = Date.now() + 1500;
      const again = () => {
        if (Date.now() < until) place();
      };
      pending.forEach((i) => i.addEventListener("load", again, { once: true }));
    }
  }, [pageKey]);

  useEffect(() => {
    if (loc.q !== undefined) return;
    writeLocal(LAST_KEY, { article: loc.article, section: loc.section, anchor: loc.anchor });
    if (meta) announce(meta.title);
  }, [pageKey]);

  // At home, the first thing to do is usually to search.
  useEffect(() => {
    if (!loc.article && !loc.section) searchRef.current?.querySelector("input")?.focus({ preventScroll: true });
  }, []);

  const onQuery = (v: string) => {
    setQuery(v);
    if (v.trim()) {
      go({ q: v }, loc.q !== undefined ? "replace" : "push");
    } else if (loc.q !== undefined) {
      if (nav.at > 0) move(-1);
      else go(HOME, "replace");
    } else if (loc.hl) {
      go({ ...loc, hl: undefined }, "replace");
    }
  };

  const focusResult = (n: number) => mainRef.current?.querySelector<HTMLElement>(`[data-result="${n}"]`)?.focus();

  const onSearchKey = (e: KeyboardEvent) => {
    if (loc.q === undefined) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      focusResult(0);
    } else if (e.key === "Enter") {
      e.preventDefault();
      mainRef.current?.querySelector<HTMLElement>('[data-result="0"]')?.click();
    }
  };

  const onMainKey = (e: KeyboardEvent) => {
    const shot = e.target as HTMLElement;
    if (shot.matches?.("img.help-shot") && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      openPicture(shot as HTMLImageElement);
      return;
    }
    const r = (e.target as HTMLElement).closest<HTMLElement>("[data-result]");
    if (!r || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    e.preventDefault();
    const n = Number(r.dataset.result) + (e.key === "ArrowDown" ? 1 : -1);
    if (n < 0) searchRef.current?.querySelector("input")?.focus();
    else focusResult(n);
  };

  const closeToc = useCallback(() => {
    setTocOpen(false);
    contentsRef.current?.querySelector("button")?.focus({ preventScroll: true });
  }, []);

  // Opening the Contents panel takes focus into it; a press outside it closes it.
  useEffect(() => {
    if (!tocOpen) return;
    tocRef.current?.querySelector<HTMLElement>('[tabindex="0"]')?.focus({ preventScroll: true });
    const away = (e: Event) => {
      if (!(e.target instanceof Node) || tocRef.current?.contains(e.target) || contentsRef.current?.contains(e.target)) return;
      setTocOpen(false);
    };
    window.addEventListener("pointerdown", away, true);
    return () => window.removeEventListener("pointerdown", away, true);
  }, [tocOpen]);

  const onWindowKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && tocOpen) {
      e.preventDefault();
      e.stopPropagation();
      closeToc();
      return;
    }
    if (!e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    if (e.key === "[" || e.key === "]") {
      e.preventDefault();
      move(e.key === "[" ? -1 : 1);
    }
  };

  const openPicture = (img: HTMLImageElement) => setPicture({ src: img.currentSrc || img.src, caption: img.alt, from: img });

  /** Everything clickable in a page is a link, so all of them land here. */
  const onMainClick = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    if (target.matches?.("img.help-shot")) {
      openPicture(target as HTMLImageElement);
      return;
    }
    const copy = target.closest(".code-copy");
    if (copy) {
      const code = copy.closest(".code-block")?.querySelector("code")?.textContent ?? "";
      void navigator.clipboard?.writeText(code).then(() => {
        copy.textContent = "Copied";
        window.setTimeout(() => (copy.textContent = "Copy"), 1200);
      }, () => {});
      return;
    }
    const link = target.closest("a");
    const href = link?.getAttribute("href");
    if (!link || !href) return;
    e.preventDefault();
    if (link.classList.contains("help-term") && href.startsWith("help:glossary#")) {
      // The definition opens beside the word: the reader stays on the page.
      const anchor = href.slice("help:glossary#".length);
      // A word inside a definition swaps the definition, still beside the first word.
      if (term?.from === link) setTerm(null);
      else if (glossaryEntry(anchor)) setTerm({ anchor, from: term && link.closest(".help-term-pop") ? term.from : link });
      else go({ article: "glossary", anchor });
      return;
    }
    setTerm(null);
    if (href.startsWith("#")) {
      // Back returns here, like a browser. A second click on the same link has no new place to go.
      const anchor = href.slice(1);
      if (anchor === loc.anchor) toAnchor(anchor);
      else go({ ...loc, anchor, top: undefined });
    } else if (href.startsWith("help:")) {
      const { id, anchor } = splitTopic(href.slice(5));
      go({ article: id, anchor, hl: link.dataset.hl || undefined });
    } else if (href.startsWith("helpsection:")) {
      go({ section: href.slice(12) });
    } else if (href.startsWith("open:")) {
      const open = parseOpenLink(href);
      if (open) wm.open(open.kind, open.params);
    } else if (/^(https?:|mailto:)/i.test(href)) {
      openUrl(href).catch(() => {});
    }
  };

  const spy = useRef(0);
  const onScroll = () => {
    if (term) setTerm(null);
    if (spy.current) return;
    spy.current = requestAnimationFrame(() => {
      spy.current = 0;
      const el = mainRef.current;
      if (!el) return;
      const line = el.getBoundingClientRect().top + 48;
      let on: string | null = null;
      for (const h of el.querySelectorAll<HTMLElement>(".help-prose [data-anchor]")) {
        if (h.getBoundingClientRect().top > line) break;
        on = h.dataset.anchor ?? null;
      }
      setActive(on);
    });
  };
  useEffect(() => onScroll(), [pageKey]);
  useEffect(() => () => cancelAnimationFrame(spy.current), []);

  const terms = useMemo(() => (loc.hl ? matchedTerms(searchIndex(), loc.hl) : null), [loc.hl]);
  const definition = term ? glossaryEntry(term.anchor) : undefined;

  let page: ReactNode;
  if (loc.q !== undefined) page = <Results query={loc.q} />;
  else if (loc.article) page = <ArticlePage id={loc.article} terms={terms} active={active} />;
  else if (loc.section) page = <SectionPage id={loc.section} />;
  else page = <Home />;

  return (
    <div className={`help-win${tocOpen ? " toc-open" : ""}`} onKeyDown={onWindowKey}>
      <Toolbar label="Help">
        <span className="help-toc-toggle" ref={contentsRef}>
          <ToolButton icon={PanelLeft} label="Contents" active={tocOpen} onClick={() => setTocOpen((o) => !o)} />
        </span>
        <ToolButton icon={ArrowLeft} label="Back" title="Back (⌘[)" disabled={nav.at === 0} onClick={() => move(-1)} />
        <ToolButton icon={ArrowRight} label="Forward" title="Forward (⌘])" disabled={nav.at >= nav.stack.length - 1} onClick={() => move(1)} />
        <ToolButton icon={House} label="Help home" disabled={!loc.article && !loc.section && loc.q === undefined} onClick={() => go(HOME)} />
        <Crumbs loc={loc} go={go} />
        <div className="help-search" ref={searchRef} onKeyDown={onSearchKey}>
          <SearchField value={query} onChange={onQuery} placeholder="Search Help" label="Search Help" />
        </div>
      </Toolbar>
      <div className="help-split">
        <nav className="help-toc" aria-label="Table of contents" ref={tocRef}>
          <Toc
            current={loc.article}
            section={loc.section}
            onArticle={(id) => go({ article: id })}
            onSection={(id) => go({ section: id })}
          />
        </nav>
        <div className="help-main" ref={mainRef} tabIndex={0} aria-label="Help page" onClick={onMainClick} onKeyDown={onMainKey} onScroll={onScroll}>
          {page}
          {term && definition && (
            <TermPopover term={definition.term} html={definition.html} from={term.from} onClose={() => setTerm(null)}>
              <a className="helptip-link helptip-more" href={`help:glossary#${term.anchor}`}>Open in Glossary</a>
            </TermPopover>
          )}
        </div>
      </div>
      {picture && <PictureView {...picture} onClose={() => setPicture(null)} />}
    </div>
  );
}
