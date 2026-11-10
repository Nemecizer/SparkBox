// The help lint: everything docs/help/STYLE.md asks that a machine can check. It fails the
// build, so a broken link or a missing article can't ship.
//
// While docs/help/pending.json exists an article may be missing, so Help can be written a
// package at a time. Delete that file when the last article is written: from then on the lint
// also requires every article, every picture and every icon in INFO-ICONS.md.
import { Lexer, type Token, type Tokens } from "marked";
import { describe, expect, it, vi } from "vitest";
import iconsMap from "../../docs/help/INFO-ICONS.md?raw";
import styleGuide from "../../docs/help/STYLE.md?raw";
import phrasesJson from "../../docs/help/phrases.json";
import shotsJson from "../../docs/help/shots.json";
import { editDistance, matchCommand } from "../lib/commands";
import * as store from "../lib/store";
import type { ClusterSnapshot } from "../lib/types";
import { helpOf, REGISTRY } from "../windows/registry";
import { SECTIONS, sectionMatches } from "../windows/settings/index";
import { allArticles, getArticle, rawFiles, renderArticle, searchDocs, termSpecs } from "./content";
import { headingName, manifest, resolveTopic, splitTopic, tips } from "./meta";
import { parseOpenLink } from "./nav";
import helpCss from "./help.css?raw";
import { headingsOf, renderHelp } from "./render";
import { READER_WORDS, stem } from "./search";
import type { ArticleMeta, HelpMenuEntry } from "./types";

const PENDING = Object.keys(import.meta.glob("../../docs/help/pending.json")).length > 0;
const code = import.meta.glob("../**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const pictures = new Set(Object.keys(import.meta.glob("../../docs/help/img/*.{jpg,svg}")).map((p) => p.replace(/^.*\/img\/|\.(?:jpg|svg)$/g, "")));
const diagramFiles = import.meta.glob("../../docs/help/img/*.svg", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const shots = (shotsJson as { shots: { id: string; source: string; js?: string; caption: string; wait?: number; awaiting?: boolean }[] }).shots;

const ids = new Set(manifest.articles.map((a) => a.id));
const written = allArticles();
const missing = manifest.articles.filter((a) => !getArticle(a.id)).map((a) => a.id);

const BANNED = [
  "simply", "just", "easy", "easily", "obviously", "clearly", "basically", "actually", "of course", "seamless", "seamlessly",
  "powerful", "robust", "leverage", "utilize", "and so on", "in order to", "please", "unfortunately",
];
const BANNED_RE = new RegExp(`\\b(?:${BANNED.join("|").replace(/ /g, "\\s+")})\\b|\\betc\\.`, "gi");

/** Prose only: code, keys and bold UI labels drop out, then anything in double quotes. */
function proseOf(tokens: Token[]): string {
  return tokens
    .map((t): string => {
      switch (t.type) {
        case "code": case "codespan": case "html": case "image": case "strong": return " ∎ ";
        case "table": {
          const table = t as Tokens.Table;
          return [table.header, ...table.rows].map((row) => row.map((c) => proseOf(c.tokens)).join(" | ")).join("\n");
        }
        case "list": return (t as Tokens.List).items.map((i) => proseOf(i.tokens)).join("\n");
        default:
          if ("tokens" in t && t.tokens?.length) return proseOf(t.tokens);
          return "text" in t ? String(t.text) : "";
      }
    })
    .join("");
}

function bannedIn(prose: string): string[] {
  const free = prose.replace(/"[^"\n]*"|“[^”\n]*”/g, " ").replace(/&quot;[^\n]*?&quot;/g, " ").replace(/\s+/g, " ");
  return [...free.matchAll(BANNED_RE)].map((m) => m[0].toLowerCase());
}

const lex = (s: string) => new Lexer({ gfm: true }).lex(s);
/** A key's name isn't prose. */
const textOf = (markdown: string) => proseOf(lex(markdown.replace(/<kbd>[^<]*<\/kbd>/g, " ∎ ")));

function walk(tokens: Token[], visit: (t: Token) => void) {
  for (const t of tokens) {
    visit(t);
    if (t.type === "table") {
      const table = t as Tokens.Table;
      for (const row of [table.header, ...table.rows]) for (const c of row) walk(c.tokens, visit);
    } else if (t.type === "list") {
      walk((t as Tokens.List).items, visit);
    } else if ("tokens" in t && t.tokens) walk(t.tokens, visit);
  }
}

function linksOf(body: string): string[] {
  const out: string[] = [];
  walk(lex(body), (t) => {
    if (t.type === "link") out.push((t as Tokens.Link).href);
  });
  return out;
}

function imagesOf(body: string): { href: string; text: string }[] {
  const out: { href: string; text: string }[] = [];
  walk(lex(body), (t) => {
    if (t.type === "image") out.push({ href: (t as Tokens.Image).href, text: (t as Tokens.Image).text });
  });
  return out;
}

function namedLinksOf(body: string): { text: string; id: string }[] {
  const out: { text: string; id: string }[] = [];
  walk(lex(body), (t) => {
    const m = t.type === "link" ? /^help:([a-z0-9-]+)$/.exec((t as Tokens.Link).href) : null;
    if (m) out.push({ text: (t as Tokens.Link).text, id: m[1] });
  });
  return out;
}

const LITTLE_WORDS = new Set(["a", "an", "the", "your", "my", "to", "of", "on", "and", "in", "for", "from", "with"]);
const wordsOf = (s: string) => s.toLowerCase().replace(/['’]/g, "").split(/[^a-z0-9]+/).filter(Boolean);
const mainWords = (s: string) => new Set(wordsOf(s).filter((w) => !LITTLE_WORDS.has(w)));

/**
 * Does this link text read as an article's name without being its title? A reader who follows
 * it lands on a page that looks like the wrong one. It catches text left over from the slug or
 * from memory. Text in a sentence, one word, a path (Settings › Nodes), a name as it is on
 * screen (Cluster login, for "Settings: Cluster login"), a term and an instruction are all fine.
 */
function staleTitle(text: string, id: string, title: string): boolean {
  if (text === title || !/^[A-Z]/.test(text) || text.includes("›") || /^Turn /.test(text)) return false;
  if (text === "Admin mode" || text === "DGX Dashboard") return false;
  const words = wordsOf(text);
  if (words.length < 2) return false;
  const onScreen = (s: string) => s.replace(/^the /, "").replace(/ tab$/, "");
  if (onScreen(text) === onScreen(title.split(": ").pop() ?? "")) return false;
  const main = mainWords(text);
  const named = mainWords(title);
  const slug = mainWords(id);
  if (main.size === slug.size && [...main].every((w) => slug.has(w))) return true;
  if (wordsOf(title).slice(0, words.length).join(" ") === words.join(" ")) return true;
  const shared = [...main].filter((w) => named.has(w)).length;
  return (2 * shared) / (main.size + named.size) >= 0.6;
}

/** Words STYLE.md retired. Checked in prose, so quoted UI text and code keep their own spelling. */
const RETIRED: [RegExp, string][] = [
  [/\b(?:un)?ticks?\b|\b(?:un)?ticked\b/i, "select, deselect or a checkmark, not tick"],
  [/\b(?:hold|holding|leave|rest) the pointer\b|\bhover\b/i, "move the pointer over"],
  [/\btitled\b/i, "a dialog named **…**"],
  [/\bsign(?:s|ed|ing)?[- ]in\b/i, "log in, login"],
  [/\bafterwards\b/i, "afterward"],
  [/\bcancell(?:ed|ing)\b/i, "canceled, except the state **cancelled**"],
  [/heavy lifting|final word|full story|keep an eye on|your call|wobbles|\bthe story\b|simple to state/i, "an idiom"],
  [/sealed package|packaged copy|saved recipe|a number that apps|trained AI program|has learned to read/i, "the definition in STYLE.md, section 9"],
];

/** Words STYLE.md settles, checked in bold too: "**When Admin mode locks, the load carries on.**" */
const UNSETTLED: [RegExp, string][] = [
  [/\bpart-way\b/i, "partway"],
  [/\bcheck marks?\b/i, "checkmark"],
  [/\bmulti-node\b/i, "several-node"],
  [/\bcarr(?:y|ies|ied|ying) on\b/i, "keeps going, or continue"],
  [/\b(?:API|SparkBox) [Ss]erver\b/i, "the Server"],
  // The app's own words stay: the label "power loss, hang or watchdog" and the alert Hung task.
  [/\b(?:hangs?|hanging)\b(?! or watchdog)|\bhung\b(?![- ]tasks?\b)/i, "freeze, frozen"],
];

/** One event, one sentence (STYLE.md, section 6). */
const UNLOCK_STEP = "If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.";
const OLD_WORDINGS: [RegExp, string][] = [
  [/Your data source is \*\*Live\*\*|You can unlock Admin mode|You need Admin mode, and it doesn't work/, "the Before you start sentence in STYLE.md, section 6"],
  [/dialog named \*\*Unlock Admin mode\?\*\*|dialog asks \*\*Unlock Admin mode\?\*\*|dialog opens first|asks you to unlock it|asks to unlock Admin mode|Admin mode has locked/, UNLOCK_STEP],
];

/** Things said one way only. `body` has code, link targets and quoted text taken out. */
function wordingProblems(body: string): string[] {
  const problems: string[] = [];
  for (const [re, use] of [...UNSETTLED, ...OLD_WORDINGS]) {
    const m = re.exec(body);
    if (m) problems.push(`"${m[0]}": use ${use}`);
  }
  for (const m of body.matchAll(/If Admin mode is locked, the \*\*Unlock Admin mode\?\*\* dialog opens\.[^\n]*/g)) {
    if (!m[0].startsWith(UNLOCK_STEP)) problems.push(`"${m[0].slice(0, 90)}…": use ${UNLOCK_STEP}`);
  }
  return problems;
}

/** Each next step is a bullet: a link, or a link and ", to …". */
const NEXT_STEP = /^- \[[^\]]+\]\((?:help|open):[^)\s]+\)(?:, to [^\n]*[^.\n])?$/;

function nextStepProblems(body: string): string[] {
  const steps = section(body, "Next steps");
  if (steps === null) return [];
  const bullets = steps.split("\n").filter((l) => l.trim());
  const problems = bullets.filter((l) => !NEXT_STEP.test(l)).map((l) => `not a link, or a link and ", to …": ${l}`);
  if (bullets.length > 4) problems.push(`${bullets.length} next steps, at most 4`);
  return problems;
}

/** Each way in is a bullet that starts with where you are. */
const WAY_IN = /^- \*\*[A-Z][^*\n]*:\*\* /;

function whereProblems(body: string): string[] {
  const where = section(body, "Where to find it");
  if (where === null) return [];
  const bullets = where.split("\n").filter((l) => l.startsWith("- "));
  const problems = bullets.filter((l) => !WAY_IN.test(l)).map((l) => `needs a label such as **Sidebar:**: ${l}`);
  if (!bullets.length) problems.push("list each way in as a bullet");
  return problems;
}

function retiredIn(prose: string): string[] {
  const free = prose.replace(/"[^"\n]*"|“[^”\n]*”/g, " ").replace(/&quot;[^\n]*?&quot;/g, " ");
  return RETIRED.flatMap(([re, use]) => {
    const m = re.exec(free);
    return m ? [`"${m[0]}": use ${use}`] : [];
  });
}

/** Spelling is checked in bold too, where a run-in heading can hide it. Bold UI labels have none of these. */
const BAD_SPELLING = /\bgr[e]y/i;
/** Outside code, bold and quotes, a node name has slipped through. */
const NODE_NAME = /(?<![\w/.-])(?:cell[123]|gx10-0000)(?![\w-])/;
/** A node's row never reads Online: it shows its GPU use. */
const NODE_ONLINE = /(?:shows?|must be|must show|is|are|isn't|stays|reads?) \*\*Online\*\*/;

/** Sentences in a block of prose. Code, link targets and quoted text don't end one. */
function sentenceCount(text: string): number {
  const plain = text
    .replace(/`[^`\n]*`/g, "x")
    .replace(/\]\([^)\n]*\)/g, "]")
    .replace(/"[^"\n]*"|“[^”\n]*”/g, "x")
    .replace(/\*\*/g, "");
  return (plain.match(/[^.!?\s][^.!?]*[.!?]+(?=\s|$)/g) ?? []).length;
}

/** The one tag an article writes by hand. */
const OWN_TAGS = new Set(["kbd"]);

/**
 * What a reader would see go wrong on the page. A quoted `<word>` parses as a tag, and the sanitizer
 * drops it with its text. A backtick inside bold opens a code span that swallows the rest, and
 * leaves the ** showing.
 */
function renderProblems(body: string, html: string): string[] {
  const problems: string[] = [];
  walk(lex(body), (t) => {
    if (t.type !== "html") return;
    for (const m of t.raw.matchAll(/<\/?([a-zA-Z][\w-]*)/g)) {
      if (!OWN_TAGS.has(m[1].toLowerCase())) problems.push(`<${m[1]}> reads as a tag, and vanishes: write \\<${m[1]}\\>, or use a code span`);
    }
  });
  const shown = html.replace(/<pre[\s\S]*?<\/pre>|<code>[\s\S]*?<\/code>|<kbd>[\s\S]*?<\/kbd>/g, " ").replace(/<[^>]*>/g, " ");
  for (const m of shown.matchAll(/[^\n]{0,30}(?:\*\*|`)[^\n]{0,30}/g)) problems.push(`Markdown left showing: ${m[0].trim()}`);
  return problems;
}

/** What's under a "## " heading, or null. */
function section(body: string, title: string): string | null {
  const at = body.search(new RegExp(`^## ${title}$`, "m"));
  if (at < 0) return null;
  const rest = body.slice(at).replace(/^.*\n/, "");
  const end = rest.search(/^## /m);
  return end < 0 ? rest : rest.slice(0, end);
}

/**
 * Steps run in order. Each "### " part continues the list before it or starts again at 1, and a
 * reference to "step 9" names a step that exists.
 */
function stepProblems(body: string): string[] {
  const steps = section(body, "Steps");
  if (steps === null) return [];
  const problems: string[] = [];
  let last = 0;
  let highest = 0;
  for (const part of steps.split(/^#{3,4} .*$/m)) {
    let inPart = 0;
    for (const m of part.matchAll(/^(\d+)\. /gm)) {
      const n = Number(m[1]);
      const ok = inPart ? n === inPart + 1 : n === 1 || n === last + 1;
      if (!ok) problems.push(`step ${n} comes after step ${inPart || last}`);
      inPart = n;
      highest = Math.max(highest, n);
    }
    if (inPart) last = inPart;
  }
  for (const m of body.matchAll(/\b[Ss]teps? (\d+)\b/g)) {
    if (Number(m[1]) > highest) problems.push(`"${m[0]}" doesn't exist: the steps stop at ${highest}`);
  }
  return problems;
}

const heading2s = (body: string) => lex(body).filter((t) => t.type === "heading" && (t as Tokens.Heading).depth === 2).map((t) => (t as Tokens.Heading).text);

/** Is `want` a subsequence of `have`? Returns the first one it can't find. */
function firstMissing(have: string[], want: string[]): string | null {
  let i = 0;
  for (const w of want) {
    const at = have.indexOf(w, i);
    if (at < 0) return w;
    i = at + 1;
  }
  return null;
}

const REQUIRED: Record<string, string[]> = {
  howto: ["Before you start", "Steps", "What you should see", "Why it works", "If something goes wrong", "Next steps"],
  tutorial: ["What you'll do", "Before you start", "What you learned", "Next steps"],
  feature: ["What it's for", "Where to find it", "What's on the screen", "What it changes", "Related How-Tos"],
  concept: ["In short", "How it works", "Why it matters in SparkBox", "Related"],
};

/** A topic is a tip id, an article, or an article and heading; empty when it resolves. */
function topicProblem(topic: string): string | null {
  const anchored = (id: string, anchor?: string) => {
    if (!ids.has(id)) return `no article "${id}"`;
    const a = getArticle(id);
    if (!anchor || !a) return null;
    return headingsOf(a.body).some((h) => h.anchor === anchor) ? null : `no heading #${anchor} in ${id}`;
  };
  if (topic.includes(".")) {
    const t = tips[topic];
    if (!t) return `no tip "${topic}"`;
    const { id, anchor } = splitTopic(t.article);
    return anchored(id, anchor);
  }
  const { id, anchor } = splitTopic(topic);
  return anchored(id, anchor);
}

function linkProblem(href: string, self: ArticleMeta): string | null {
  if (href.startsWith("help:")) return topicProblem(href.slice(5));
  if (href.startsWith("#")) return topicProblem(`${self.id}${href}`);
  if (href.startsWith("open:")) {
    const open = parseOpenLink(href);
    if (!open) return "not an open: link";
    const def = REGISTRY[open.kind];
    if (!def) return `no window kind "${open.kind}"`;
    for (const need of def.needs ?? []) if (!open.params[need]) return `${open.kind} needs ${need}`;
    // Any other param makes a second copy of the window instead of moving this one.
    for (const key of Object.keys(open.params)) if (!def.views?.includes(key) && !def.needs?.includes(key)) return `${open.kind} has no view "${key}"`;
    if (open.kind === "settings" && open.params.section && !SECTIONS.some((s) => s.id === open.params.section)) return `no Settings section "${open.params.section}"`;
    const tabs = TABS[open.kind];
    if (tabs && open.params.tab && !tabs.includes(open.params.tab)) return `no tab "${open.params.tab}" in ${open.kind}`;
    return null;
  }
  if (/^https:\/\//.test(href)) return null;
  return `links must be help:, open: or https: (${href})`;
}

/** The tabs a link can ask for, from each window's own code. */
const TABS: Record<string, string[]> = {
  models: ["library", "hf", "hub", "loaded", "history", "bench", "jobs", "prepare", "load"],
  node: ["overview", "cpu", "memory", "gpu", "disks", "network", "system"],
  fabric: ["matrix", "doctor", "history"],
};

function collect(problems: string[]) {
  expect(problems, problems.join("\n")).toEqual([]);
}

describe("the lint itself", () => {
  const prose = (s: string) => bannedIn(textOf(s));

  it("catches a banned word in prose, a table, a list and a heading", () => {
    expect(prose("Just click Save.")).toEqual(["just"]);
    expect(prose("| A |\n|---|\n| You can simply do it |\n")).toEqual(["simply"]);
    expect(prose("- It is easy\n")).toEqual(["easy"]);
    expect(prose("## Actually\n")).toEqual(["actually"]);
    expect(prose("Use this in  order\nto start, etc.")).toEqual(["in order to", "etc."]);
  });

  it("leaves code, keys, bold labels and quoted messages alone", () => {
    expect(prose("`just` and <kbd>easy</kbd>")).toEqual([]);
    expect(prose("**Just this once**")).toEqual([]);
    expect(prose('The app says "Please wait" here.')).toEqual([]);
    expect(prose("```\nplease\n```\n")).toEqual([]);
    expect(prose("Adjust the justified text.")).toEqual([]);
  });

  it("catches retired words in prose and leaves the app's own words alone", () => {
    const words = (t: string) => retiredIn(textOf(t));
    expect(words("Tick the box.")).toHaveLength(1);
    expect(words("Move it, then hover over the row.")).toHaveLength(1);
    expect(words("A dialog opens, titled Restart.")).toHaveLength(1);
    expect(words("SparkBox signs in to the node.")).toHaveLength(1);
    expect(words('The message says "Tick the box to sign in".')).toEqual([]);
    expect(words("**cancelled** and `sign in` and tick-tock")).toHaveLength(1);
    expect(words("Pushover is a phone app.")).toEqual([]);
  });

  it("counts sentences the way a reader does", () => {
    expect(sentenceCount("One. Two! Three?")).toBe(3);
    expect(sentenceCount('It asks "What is 17+25? Reply." and waits.')).toBe(1);
    expect(sentenceCount("Open `a.b.c` and [the page](help:x.y). Done.")).toBe(2);
    expect(sentenceCount("It's at 0.75 of memory. That's all.")).toBe(2);
  });

  it("holds one event to one sentence, and settled words in bold too", () => {
    const say = (s: string) => wordingProblems(s);
    expect(say("**When Admin mode locks, the load carries on.**")).toHaveLength(1);
    expect(say("It stopped part-way, on a multi-node load, with a check mark.")).toHaveLength(3);
    expect(say("It goes through SparkBox Server, and the API server can hang.")).toHaveLength(2);
    expect(say("If the node has hung, or it hangs again, restart it.")).toHaveLength(1);
    expect(say("**Hung task** and **Ended without a shutdown (power loss, hang or watchdog)**, and hung tasks.")).toEqual([]);
    expect(say("- Your data source is **Live**, and look-only mode is off.")).toHaveLength(1);
    expect(say("If Admin mode is locked, SparkBox asks you to unlock it.")).toHaveLength(1);
    expect(say("If Admin mode is locked, the **Unlock Admin mode?** dialog opens first.")).toHaveLength(1);
    expect(say("If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, and confirm.")).toHaveLength(1);
    expect(say(`Click **Load**. ${UNLOCK_STEP} A bar appears.`)).toEqual([]);
  });

  it("holds Next steps and Where to find it to one form", () => {
    expect(nextStepProblems("## Next steps\n\n- [A](help:a)\n- [B](help:b#c), to do a thing\n")).toEqual([]);
    expect(nextStepProblems("## Next steps\n\n- [A](help:a) explains every part.\n")).toHaveLength(1);
    expect(nextStepProblems("## Next steps\n\n- [A](help:a), for every part\n")).toHaveLength(1);
    expect(nextStepProblems("## Next steps\n\n- [A](help:a), to do a thing.\n")).toHaveLength(1);
    expect(whereProblems("## Where to find it\n\n- **Sidebar:** click it.\n- **Link:** [Open](open:a)\n\nMore.\n")).toEqual([]);
    expect(whereProblems("## Where to find it\n\n- In the sidebar, click it.\n")).toHaveLength(1);
    expect(whereProblems("## Where to find it\n\nLook at the top.\n")).toHaveLength(1);
  });

  it("catches what the page would swallow or leave showing", () => {
    const page = (s: string) => renderProblems(s, renderHelp(s).html);
    expect(page("Use '<provider>/<model>' here.")).toHaveLength(2);
    expect(page("| A |\n|---|\n| Use <provider> |\n")).toHaveLength(1);
    expect(page("**\"near `\\n'\".** It says `<key>`.").length).toBeGreaterThan(0);
    expect(page("Use '\\<provider\\>/\\<model\\>' here.")).toEqual([]);
    expect(renderHelp("Use '\\<provider\\>/\\<model\\>' here.").html).toContain("&lt;provider&gt;/&lt;model&gt;");
    expect(page("Press <kbd>⌘K</kbd>, then **Save**, then `<key>` and `**`.")).toEqual([]);
    expect(page("```\n<key> **bold** `\n```\n")).toEqual([]);
  });

  it("checks step numbers and references", () => {
    expect(stepProblems("## Steps\n\n1. A\n2. B\n\n### Two\n\n3. C\n\n### Three\n\n1. D\n\n## Next\n")).toEqual([]);
    expect(stepProblems("## Steps\n\n1. A\n3. B\n\n## Next\n")).toHaveLength(1);
    expect(stepProblems("## Steps\n\n### One\n\n1. A\n\n### Two\n\n5. B\n\n## Next\n")).toHaveLength(1);
    expect(stepProblems("## Steps\n\n1. Go to step 4.\n\n## Next\n")).toHaveLength(1);
  });

  it("finds a required heading out of order", () => {
    expect(firstMissing(["Steps", "Before you start"], ["Before you start", "Steps"])).toBe("Steps");
    expect(firstMissing(["Before you start", "Extra", "Steps"], ["Before you start", "Steps"])).toBeNull();
  });

  it("refuses links that lead nowhere or would open a second copy of a window", () => {
    const self = manifest.articles[0];
    expect(linkProblem("help:no-such-article", self)).toMatch(/no article/);
    expect(linkProblem("help:check-disk-space#no-such-heading", self)).toMatch(/no heading/);
    expect(linkProblem("help:check-disk-space#steps", self)).toBeNull();
    expect(linkProblem("open:no-such-window", self)).toMatch(/no window kind/);
    expect(linkProblem("open:settings?section=nope", self)).toMatch(/no Settings section/);
    expect(linkProblem("open:models?tab=nope", self)).toMatch(/no tab/);
    expect(linkProblem("open:node", self)).toMatch(/needs id/);
    expect(linkProblem("open:alerts?tab=history", self)).toMatch(/no view/);
    expect(linkProblem("open:models?tab=load", self)).toBeNull();
    expect(linkProblem("http://example.com", self)).toMatch(/must be/);
  });

  it("catches a link that names an article by anything but its title", () => {
    expect(staleTitle("Tell if a model is working", "tell-if-a-model-is-working", "Tell whether a model is working")).toBe(true);
    expect(staleTitle("Splitting a model", "splitting-a-model", "Running one model on several nodes")).toBe(true);
    expect(staleTitle("Delete a model", "delete-a-model", "Delete a model from a node")).toBe(true);
    expect(staleTitle("Security settings", "settings-security", "Settings: Security & privacy")).toBe(true);
    expect(staleTitle("Delete a model from a node", "delete-a-model", "Delete a model from a node")).toBe(false);
    expect(staleTitle("the fabric tests", "test-the-fabric", "Test the fabric")).toBe(false);
    expect(staleTitle("Unload", "unload-a-model", "Unload a model")).toBe(false);
    expect(staleTitle("Settings › Terminal", "settings-terminal", "Settings: Terminal")).toBe(false);
    expect(staleTitle("Cluster login", "settings-cluster-login", "Settings: Cluster login")).toBe(false);
    expect(staleTitle("Jobs tab", "models-jobs", "Models: Jobs tab")).toBe(false);
    expect(staleTitle("Load page", "models-load-page", "Models: the Load page")).toBe(false);
    expect(staleTitle("Admin mode", "admin-mode", "Read-only, Admin mode and Look only")).toBe(false);
    expect(staleTitle("Turn off look-only mode", "use-look-only-mode", "Use look-only mode")).toBe(false);
  });

  it("refuses a topic that resolves to nothing", () => {
    expect(topicProblem("models.load.nope")).toMatch(/no tip/);
    expect(topicProblem("no-such-article")).toMatch(/no article/);
    expect(topicProblem("models.load.gpu-share")).toBeNull();
  });
});

describe("the manifest", () => {
  it("lists each article once, in the table of contents, with all its fields", () => {
    const seen = new Set<string>();
    const toc = manifest.toc.flatMap((s) => s.groups.flatMap((g) => g.articles));
    const problems: string[] = [];
    for (const a of manifest.articles) {
      if (seen.has(a.id)) problems.push(`${a.id}: listed twice`);
      seen.add(a.id);
      if (toc.filter((x) => x === a.id).length !== 1) problems.push(`${a.id}: must be in the table of contents once`);
      if (!a.title || !a.summary || !a.file) problems.push(`${a.id}: missing a field`);
      if (!REQUIRED[a.type] && a.type !== "reference" && a.type !== "troubleshooting") problems.push(`${a.id}: no type "${a.type}"`);
      if (![true, false, null].includes(a.verified)) problems.push(`${a.id}: verified must be true, false or null`);
      if (a.summary.length > 120) problems.push(`${a.id}: summary is ${a.summary.length} characters, at most 120`);
      for (const r of a.related) if (!ids.has(r)) problems.push(`${a.id}: related "${r}" doesn't exist`);
      for (const b of bannedIn(a.summary)) problems.push(`${a.id}: banned word in summary: ${b}`);
    }
    for (const id of toc) if (!ids.has(id)) problems.push(`table of contents: no article "${id}"`);
    collect(problems);
  });

  it("keeps each part of the contents to its one job", () => {
    const allowed: Record<string, string[]> = { "how-to": ["howto"], concepts: ["concept"], features: ["feature"] };
    const problems: string[] = [];
    for (const s of manifest.toc) {
      const types = allowed[s.id];
      for (const g of s.groups) {
        if (g.lead && !ids.has(g.lead)) problems.push(`${s.id}: lead "${g.lead}" doesn't exist`);
        if (g.lead && g.articles.includes(g.lead)) problems.push(`${s.id}: lead "${g.lead}" is already in the group`);
        if (!types) continue;
        for (const id of g.articles) {
          const type = manifest.articles.find((a) => a.id === id)?.type;
          if (type && !types.includes(type)) problems.push(`${id} is a ${type}, in ${s.title}`);
        }
      }
    }
    for (const a of manifest.articles) if (/^How to /.test(a.title) && a.type !== "howto") problems.push(`${a.id}: a ${a.type} shouldn't promise a How-To`);
    collect(problems);
  });

  it("gives the Help menu real articles and sections, and leaves no article out of it", () => {
    const problems: string[] = [];
    const listed = new Set<string>();
    const sections = new Set(manifest.toc.map((s) => s.id));
    const visit = (entries: HelpMenuEntry[]) => {
      for (const e of entries) {
        if (e.article) {
          listed.add(e.article);
          if (!ids.has(e.article)) problems.push(`Help menu: no article "${e.article}"`);
        }
        for (const s of [e.section, e.toc]) if (s && !sections.has(s)) problems.push(`Help menu: no section "${s}"`);
        if (e.toc) manifest.toc.find((s) => s.id === e.toc)?.groups.forEach((g) => g.articles.forEach((a) => listed.add(a)));
        if (e.submenu) visit(e.submenu);
      }
    };
    visit(manifest.helpMenu);
    for (const a of manifest.articles) if (!listed.has(a.id)) problems.push(`Help menu: ${a.id} isn't in it`);
    collect(problems);
  });
});

describe("windows and Settings", () => {
  it("every window kind has its help id, the one the manifest gives", () => {
    const kinds = Object.keys(REGISTRY).sort();
    expect(kinds).toEqual(Object.keys(manifest.windowHelp).sort());
    for (const k of kinds) {
      expect(REGISTRY[k].help, k).toBe(manifest.windowHelp[k]);
      expect(ids.has(REGISTRY[k].help), k).toBe(true);
    }
  });

  it("a Firewalla device's window explains itself, not the CRS804", () => {
    const cluster = vi.spyOn(store, "getCluster").mockReturnValue({
      switches: [{ id: "crs", kind: "routerOs" }, { id: "x", kind: "firewalla" }, { id: "gold", kind: "gateway" }],
    } as unknown as ClusterSnapshot);
    expect(helpOf("switch", { id: "crs" })).toBe("switch-window");
    expect(helpOf("switch", { id: "x" })).toBe("firewalla-view");
    expect(helpOf("switch", { id: "gold" })).toBe("firewalla-view");
    expect(helpOf("switch", { id: "unknown" })).toBe("switch-window");
    expect(helpOf("dashboard")).toBe("dashboard");
    cluster.mockRestore();
  });

  it("Settings search sends Touch ID to Security & privacy, and the Fence is for the test build", () => {
    const found = (q: string) => SECTIONS.filter((s) => sectionMatches(s, q)).map((s) => s.id);
    expect(found("touch id")).toEqual(["security"]);
    expect(found("test build")).toEqual(["fence"]);
    expect(found("release")).toEqual([]);
    // The command palette matches the same keywords (App.tsx).
    const palette = (q: string) => SECTIONS.filter((s) => matchCommand({ title: `Settings › ${s.label}`, keywords: [s.label, ...(s.keywords ?? [])], group: "Settings" }, q)).map((s) => s.id);
    expect(palette("touch id")).toEqual(["security"]);
  });

  it("every Settings section has its help id, the one the manifest gives", () => {
    expect(SECTIONS.map((s) => s.id).sort()).toEqual(Object.keys(manifest.settingsHelp).sort());
    for (const s of SECTIONS) {
      expect(s.help, s.id).toBe(manifest.settingsHelp[s.id]);
      expect(ids.has(s.help), s.id).toBe(true);
    }
  });
});

describe("files", () => {
  const articleFiles = new Set(Object.keys(rawFiles()).map((p) => p.replace(/^.*docs\/help\//, "")));

  it("every file has a manifest entry, valid frontmatter and the path the manifest gives", () => {
    const problems: string[] = [];
    const parsed = new Set(written.map((a) => a.file));
    for (const f of articleFiles) if (!parsed.has(f)) problems.push(`${f}: no frontmatter, or its id isn't in the manifest`);
    for (const a of written) if (a.meta.file !== a.file) problems.push(`${a.meta.id}: lives at ${a.file}, the manifest says ${a.meta.file}`);
    collect(problems);
  });

  it(PENDING ? "every article that exists passes (pending.json allows the rest to be missing)" : "every manifest id has a file", () => {
    if (PENDING) {
      if (missing.length) console.warn(`help lint: ${missing.length} of ${manifest.articles.length} articles still to write`);
      expect(missing.length, "Every article is written: delete docs/help/pending.json, so the lint checks all of it from now on").toBeGreaterThan(0);
    } else {
      expect(missing).toEqual([]);
    }
  });
});

describe.each(written.map((a) => [a.meta.id, a] as const))("%s", (id, a) => {
  const { meta, body, front } = a;

  it("has frontmatter that matches the manifest", () => {
    for (const key of ["id", "title", "type", "area", "summary", "verified"] as const) {
      expect(front[key] ?? null, key).toBe(meta[key]);
    }
    const related = Array.isArray(front.related) ? front.related.map(String) : [];
    const known = manifest.articles.find((x) => x.id === id)!;
    for (const r of known.related) expect(related, `related must keep ${r}`).toContain(r);
    for (const r of related) expect(ids.has(r), `related "${r}" doesn't exist`).toBe(true);
  });

  it("opens with its title and uses headings in order", () => {
    const tokens = lex(body).filter((t) => t.type !== "space");
    const first = tokens[0] as Tokens.Heading;
    expect(first?.type, "the first thing is the # title").toBe("heading");
    expect(first.depth).toBe(1);
    expect(first.text).toBe(meta.title);
    let level = 1;
    for (const t of tokens.slice(1)) {
      if (t.type !== "heading") continue;
      const h = t as Tokens.Heading;
      expect(h.depth, `one # title only: ${h.text}`).toBeGreaterThan(1);
      expect(h.depth, `heading level skipped at "${h.text}"`).toBeLessThanOrEqual(level + 1);
      level = h.depth;
    }
  });

  it("has the headings its type requires, in order", () => {
    const have = heading2s(body);
    const need = REQUIRED[meta.type];
    if (need) expect(firstMissing(have, need), `${meta.type} needs ## ${firstMissing(have, need)}`).toBeNull();
    if (meta.type === "tutorial") expect(have.some((h) => /^Step \d+: /.test(h)), "a tutorial has ## Step 1: … sections").toBe(true);
    if (meta.type === "concept") expect(lex(body).some((t) => t.type === "list" && (t as Tokens.List).ordered), "a concept has no numbered steps").toBe(false);
    if (meta.type === "howto") {
      const tokens = lex(body).filter((t) => t.type !== "space");
      expect(tokens[1]?.type, "the goal, in one sentence, comes right after the title").toBe("paragraph");
    }
    if (meta.type === "troubleshooting") {
      const entries = body.split(/^### /m).slice(1);
      for (const entry of entries) {
        const title = entry.split("\n")[0];
        expect(entry.includes("**What it means.**") && entry.includes("**What to do.**"), `### ${title} needs both bold labels`).toBe(true);
      }
    }
  });

  it("uses no banned word", () => {
    const words = bannedIn(textOf(body));
    expect(words, `banned: ${words.join(", ")}`).toEqual([]);
  });

  it("uses the words STYLE.md settles", () => {
    const raw = body.replace(/^ *```[\s\S]*?^ *```$/gm, " ").replace(/`[^`\n]*`/g, " ").replace(/\]\([^)\n]*\)/g, "]").replace(/"[^"\n]*"|“[^”\n]*”/g, " ");
    const proseFree = textOf(body).replace(/"[^"\n]*"|“[^”\n]*”/g, " ");
    const problems = [
      ...retiredIn(textOf(body)),
      ...(BAD_SPELLING.test(raw) ? ["gray, or dimmed for a button"] : []),
      ...(NODE_ONLINE.test(body) ? ["a node's row never reads Online: see STYLE.md, section 9"] : []),
      ...(NODE_NAME.test(proseFree) ? ["node names are in code format"] : []),
      ...wordingProblems(raw),
      ...nextStepProblems(body),
      ...whereProblems(body),
    ];
    collect(problems);
  });

  it("keeps Why it works to the reason, and its steps in order", () => {
    if (meta.type !== "howto") return;
    const why = section(body, "Why it works") ?? "";
    const n = sentenceCount(why);
    expect(n, `Why it works has ${n} sentences: two to five`).toBeLessThanOrEqual(5);
    expect(n, `Why it works has ${n} sentences: two to five`).toBeGreaterThanOrEqual(2);
    expect(/^\s*(?:[-*] |\d+\. |\|)/m.test(why), "Why it works has no lists or tables").toBe(false);
    collect(stepProblems(body));
  });

  it("links only to things that exist", () => {
    const problems = linksOf(body).map((href) => {
      const p = linkProblem(href, meta);
      return p ? `${href}: ${p}` : null;
    }).filter((x): x is string => !!x);
    collect(problems);
  });

  it("names an article by its exact title", () => {
    const problems = namedLinksOf(body).flatMap(({ text, id }) => {
      const title = manifest.articles.find((a) => a.id === id)?.title;
      return title && staleTitle(text, id, title) ? [`"${text}" should read "${title}"`] : [];
    });
    collect(problems);
  });

  it("shows on the page what it says in Markdown", () => {
    collect(renderProblems(body, renderArticle(id)!.html));
  });

  it("uses pictures that have a recipe", () => {
    const problems: string[] = [];
    for (const img of imagesOf(body)) {
      const m = /^img\/([\w-]+)\.(jpg|svg)$/.exec(img.href);
      if (!m) {
        problems.push(`${img.href}: pictures are img/<id>.jpg, and diagrams img/<id>.svg`);
        continue;
      }
      if (!img.text) problems.push(`${img.href}: needs a caption`);
      const recipe = shots.find((s) => s.id === m[1]);
      if (!recipe) problems.push(`${img.href}: no entry in shots.json`);
      else if ((recipe.source === "diagram") !== (m[2] === "svg")) problems.push(`${img.href}: a diagram is .svg, and a picture .jpg`);
      if (!PENDING && !pictures.has(m[1]) && !recipe?.awaiting) problems.push(`${img.href}: not generated: run ${m[2] === "svg" ? "tools/agents/docs-diagrams.mjs" : "tools/agents/docs-shots.sh"}`);
    }
    collect(problems);
  });
});

describe("tips", () => {
  const PREFIX = /^(start|desk|mon|models|ai|ops|ref)\.[a-z0-9.-]+$/;

  it("are short, plain and lead somewhere", () => {
    const problems: string[] = [];
    for (const [id, t] of Object.entries(tips)) {
      if (!PREFIX.test(id)) problems.push(`${id}: needs a package prefix`);
      if (!t.title || !t.body || !t.article) problems.push(`${id}: needs title, body and article`);
      const words = t.body.trim().split(/\s+/).length;
      if (words > 60) problems.push(`${id}: ${words} words, at most 60`);
      const sentences = t.body.split(/[.!?](?:\s|$)(?=[A-Z"“]|$)/).filter((s) => s.trim()).length;
      if (sentences > 3) problems.push(`${id}: ${sentences} sentences, at most 3`);
      const p = topicProblem(t.article);
      if (p) problems.push(`${id}: ${p}`);
      if ((t.howto ?? []).length > 3) problems.push(`${id}: at most three How-Tos`);
      for (const h of t.howto ?? []) if (!ids.has(h)) problems.push(`${id}: no How-To "${h}"`);
      for (const b of bannedIn(`${t.title}. ${t.body}`)) problems.push(`${id}: banned word: ${b}`);
      for (const r of [...retiredIn(t.body), ...wordingProblems(t.body)]) problems.push(`${id}: ${r}`);
    }
    collect(problems);
  });

  it("every topic used in the code resolves", () => {
    const problems: string[] = [];
    const RE = /<(?:HelpTip|InfoTip)\b[^>]*?\btopic="([^"]+)"|\binfoTopic="([^"]+)"|\bhelp: "([a-z0-9][a-z0-9.#-]*)"/g;
    for (const [file, src] of Object.entries(code)) {
      if (file.includes(".test.") || file.includes("/help/")) continue;
      for (const m of src.matchAll(RE)) {
        const topic = m[1] ?? m[2] ?? m[3];
        const p = topicProblem(topic);
        if (p) problems.push(`${file}: ${p}`);
        if (!resolveTopic(topic)) problems.push(`${file}: "${topic}" shows nothing`);
      }
    }
    collect(problems);
  });

  it("every How-To is offered by an icon, and every window's guide offers one", () => {
    const RE = /<(?:HelpTip|InfoTip)\b[^>]*?\btopic="([^"]+)"|\binfoTopic="([^"]+)"|\bhelp: "([a-z0-9][a-z0-9.#-]*)"/g;
    const topics = new Set([...Object.values(manifest.windowHelp), ...Object.values(manifest.settingsHelp)]);
    for (const [file, src] of Object.entries(code)) {
      if (file.includes(".test.") || file.includes("/help/")) continue;
      for (const m of src.matchAll(RE)) topics.add(m[1] ?? m[2] ?? m[3]);
    }
    const offered = new Set([...topics].flatMap((t) => resolveTopic(t)?.howtos.map((h) => h.id) ?? []));
    const problems = manifest.articles
      .filter((a) => (a.type === "howto" || a.type === "tutorial") && !offered.has(a.id))
      .map((a) => `${a.id}: no icon's tip links to it`);
    for (const id of new Set([...Object.values(manifest.windowHelp), ...Object.values(manifest.settingsHelp)])) {
      if (manifest.articles.find((a) => a.id === id)?.type === "feature" && !resolveTopic(id)?.howtos.length) problems.push(`${id}: its tip offers no How-To`);
    }
    collect(problems);
  });

  it("an icon's name is the real heading it explains", () => {
    const problems: string[] = [];
    const RE = /<(?:HelpTip|InfoTip)\b[^>]*?\btopic="([^"]+#[^"]+)"|\binfoTopic="([^"]+#[^"]+)"|\bhelp: "([a-z0-9][a-z0-9-]*#[a-z0-9-]+)"/g;
    const heads = (topic: string) => {
      const { id, anchor } = splitTopic(topic);
      return { anchor: anchor!, real: headingsOf(getArticle(id)!.body).find((h) => h.anchor === anchor)?.text };
    };
    for (const [file, src] of Object.entries(code)) {
      if (file.includes(".test.") || file.includes("/help/")) continue;
      for (const m of src.matchAll(RE)) {
        const { anchor, real } = heads(m[1] ?? m[2] ?? m[3]);
        if (real && headingName(anchor).toLowerCase() !== real.toLowerCase().replace(/&/g, "and").replace(/[^\p{L}\p{N} -]/gu, "").replace(/\s+/g, " ")) problems.push(`${file}: #${anchor} reads "${headingName(anchor)}", the heading is "${real}"`);
        if (real && /^[^a-z]+$/.test(real) && headingName(anchor) !== real) problems.push(`${file}: add "${real}" to SHOUTED in meta.ts`);
      }
    }
    collect(problems);
  });

  it("a Help tip never hides in the code as a literal: every topic is a string the lint can see", () => {
    const problems: string[] = [];
    for (const [file, src] of Object.entries(code)) {
      if (file.includes(".test.") || file.includes("/help/") || /components\/(ui|Table)\.tsx|windows\/Settings\.tsx|wm\//.test(file)) continue;
      for (const m of src.matchAll(/<HelpTip\b[^>]*\btopic=\{/g)) problems.push(`${file}: ${m[0]}…: use topic="…"`);
    }
    collect(problems);
  });
});

/** What scripts/build-app.sh refuses: Help ships with every picture, in both themes. */
function unfinishedPictures(have: Set<string>): string[] {
  return shots.flatMap((s) => [
    ...(s.awaiting ? [`${s.id}: still marked awaiting`] : []),
    ...(have.has(s.id) ? [] : [`${s.id}: no picture`]),
    ...(have.has(`${s.id}-dark`) ? [] : [`${s.id}: no dark picture`]),
  ]);
}

describe("search's own words", () => {
  it("name articles that exist", () => {
    expect(Object.keys(phrasesJson.phrases).filter((id) => !ids.has(id))).toEqual([]);
  });

  it("keep back only the test build's pages", () => {
    expect(manifest.articles.filter((a) => a.testBuild).map((a) => a.id).sort()).toEqual(["open-a-fence", "settings-fence", "the-fence"]);
  });
});

describe("phrases.json", () => {
  const wordsIn = (text: string) => (text.toLowerCase().match(/[\p{L}\p{N}]+(?:['’][\p{L}]+)*/gu) ?? []).map((w) => w.replace(/['’]/g, ""));
  /** In this many articles or more, a word is common enough for a misspelling of it to matter. */
  const COMMON = 3;

  it("adds no misspelling: a word only it has can't sit next to a common word unless it's a real word", () => {
    const docs = searchDocs();
    const pages = new Map<string, number>();
    for (const d of docs) {
      for (const w of new Set(wordsIn([d.title, d.summary, d.body, ...d.keywords, ...d.headings.map((h) => h.text)].join(" ")))) pages.set(w, (pages.get(w) ?? 0) + 1);
    }
    const known = new Set([...pages.keys()].map(stem));
    const common = [...pages].filter(([w, n]) => n >= COMMON && w.length >= 3).map(([w]) => w);
    const heard = new Set(Object.values(phrasesJson.phrases as Record<string, string[]>).flat().flatMap(wordsIn));
    const suspects = [...heard].filter((w) => w.length >= 4 && !/\d/.test(w) && !pages.has(w) && !known.has(stem(w)) && !READER_WORDS.has(w));
    const wrong = suspects.flatMap((w) => {
      const near = common.find((c) => Math.abs(c.length - w.length) <= 2 && editDistance(w, c) <= 2);
      return near ? [`${w} (near ${near})`] : [];
    });
    expect(wrong, "a misspelling teaches search nothing: delete it. A real word goes on READER_WORDS in search.ts").toEqual([]);
  });

  it("READER_WORDS lists only words readers are known to use", () => {
    const heard = new Set(Object.values(phrasesJson.phrases as Record<string, string[]>).flat().flatMap(wordsIn));
    expect([...READER_WORDS].filter((w) => !heard.has(w))).toEqual([]);
  });
});

describe("the Help window's style sheet", () => {
  it("never lets a word break just to shrink a column", () => {
    expect(helpCss.replace(/\/\*[\s\S]*?\*\//g, "")).not.toMatch(/overflow-wrap:\s*anywhere|word-break:\s*break-all/);
  });
});

describe("pictures", () => {
  it("a release build refuses a missing picture, a missing dark picture and an awaiting mark", () => {
    // The prefix matters: import.meta.env leaves out any variable without VITE_, with no error.
    if (import.meta.env.VITE_HELP_RELEASE) collect(unfinishedPictures(pictures));
    const all = new Set(shots.flatMap((s) => [s.id, `${s.id}-dark`]));
    if (!shots.some((s) => s.awaiting)) expect(unfinishedPictures(all)).toEqual([]);
    const first = shots[0].id;
    expect(unfinishedPictures(new Set([...all].filter((p) => p !== `${first}-dark`)))).toContain(`${first}: no dark picture`);
  });

  it("a recipe marked awaiting has no picture yet", () => {
    const waiting = shots.filter((s) => s.awaiting).map((s) => s.id);
    if (waiting.length) console.warn(`help lint: pictures to take with tools/agents/docs-shots.sh: ${waiting.join(" ")}`);
    collect(waiting.filter((id) => pictures.has(id)).map((id) => `${id}: it's taken now, so remove "awaiting" from its recipe`));
  });

  it("every recipe is complete and uses Demo data", () => {
    const problems: string[] = [];
    const seen = new Set<string>();
    for (const s of shots) {
      if (seen.has(s.id)) problems.push(`${s.id}: listed twice`);
      seen.add(s.id);
      if (!/^[a-z0-9-]+$/.test(s.id)) problems.push(`${s.id}: ids are lower case with hyphens`);
      const diagram = s.source === "diagram";
      if (!diagram && s.source !== "demo") problems.push(`${s.id}: pictures show Demo data only`);
      if (!s.caption || (!diagram && !s.js)) problems.push(`${s.id}: needs ${diagram ? "a caption" : "js and caption"}`);
      for (const b of bannedIn(s.caption ?? "")) problems.push(`${s.id}: banned word in caption: ${b}`);
    }
    collect(problems);
  });

  it("every diagram has both themes, sets its own size, and loads nothing", () => {
    const diagrams = shots.filter((s) => s.source === "diagram").map((s) => s.id);
    expect(diagrams.length).toBeGreaterThan(0);
    const problems: string[] = [];
    for (const id of diagrams.flatMap((d) => [d, `${d}-dark`])) {
      const svg = Object.entries(diagramFiles).find(([path]) => path.endsWith(`/img/${id}.svg`))?.[1];
      if (svg === undefined) {
        problems.push(`${id}: run tools/agents/docs-diagrams.mjs`);
        continue;
      }
      if (!/^<svg [^>]*\bwidth="\d+" height="\d+" viewBox="0 0 \d+ \d+"/.test(svg)) problems.push(`${id}: needs a width, a height and a viewBox`);
      if (/<script|<image|<foreignObject|href=|url\(http/.test(svg)) problems.push(`${id}: a diagram is boxes, lines and words`);
      for (const b of bannedIn([...svg.matchAll(/<text[^>]*>([^<]*)<\/text>/g)].map((m) => m[1]).join(". "))) problems.push(`${id}: banned word: ${b}`);
    }
    collect(problems);
  });
});

/** The words under a glossary heading, by its name. */
function definitionOf(term: string): string | undefined {
  const glossary = getArticle("glossary");
  if (!glossary) return undefined;
  const name = term.replace(/^./, (c) => c.toUpperCase());
  const m = new RegExp(`^### (?:${term}|${name})\\n\\n(.+)$`, "m").exec(glossary.body);
  return m?.[1].replace(/\[([^\]]*)\]\([^)]*\)/g, "$1");
}

describe("feature guides", () => {
  it("show the screen: each window's guide has a picture", () => {
    const guides = new Set([...Object.values(manifest.windowHelp), "settings-fence"]);
    const problems: string[] = [];
    for (const id of guides) {
      const a = getArticle(id);
      if (a?.meta.type === "feature" && imagesOf(a.body).length === 0) problems.push(`${id}: no picture`);
    }
    collect(problems);
  });
});

describe("concepts", () => {
  it("the hardest ideas show a picture", () => {
    const problems: string[] = [];
    for (const id of ["memory-on-a-spark", "splitting-a-model", "endpoints-and-apis", "fabric-and-rdma"]) {
      const a = getArticle(id);
      if (a && imagesOf(a.body).length === 0) problems.push(`${id}: no picture`);
    }
    collect(problems);
  });
});

describe("core terms", () => {
  const glossary = getArticle("glossary");
  const anchors = new Set(glossary ? headingsOf(glossary.body).map((h) => h.anchor) : []);

  it("each one has a glossary entry", () => {
    const problems: string[] = [];
    for (const t of termSpecs) {
      if (!anchors.has(t.anchor)) problems.push(`terms.json: no glossary heading "${t.anchor}"`);
      for (const id of t.skip ?? []) if (!ids.has(id)) problems.push(`terms.json: ${t.anchor} skips "${id}", which doesn't exist`);
      if (t.not) expect(() => new RegExp(t.not!, "giu"), t.anchor).not.toThrow();
    }
    collect(problems);
  });

  it("each has one definition: the glossary's words are the ones in STYLE.md", () => {
    const row = /^\| \*\*(?:the )?([\w ]+)\*\* \| (.+?)(?:\. .+)? \|$/gm;
    const section9 = styleGuide.slice(styleGuide.indexOf("### Core definitions"), styleGuide.indexOf("### Modes and safety"));
    const problems: string[] = [];
    for (const m of section9.matchAll(row)) {
      const entry = definitionOf(m[1]);
      if (entry !== undefined && !entry.toLowerCase().includes(m[2].toLowerCase())) problems.push(`${m[1]}: glossary says "${entry}", STYLE.md says "${m[2]}"`);
    }
    collect(problems);
  });

  it("link the first use on the most-read pages", () => {
    const linked = (id: string) => [...renderArticle(id)!.html.matchAll(/class="help-term" href="help:glossary#([\w-]+)"/g)].map((m) => m[1]);
    const load = linked("load-a-model-on-one-node");
    for (const term of ["token", "image", "port", "gb10", "container", "engine"]) expect(load, term).toContain(term);
    expect(linked("glossary")).toEqual([]);
  });
});

describe("the info-icon map", () => {
  /** "| `src/x.tsx` | Place | add | `topic` |": a blank File cell repeats the one above. */
  function rows(): { file: string; place: string; topic: string }[] {
    const out: { file: string; place: string; topic: string }[] = [];
    let file = "";
    for (const line of iconsMap.split("\n")) {
      const m = /^\|\s*(`src\/[^`]+`)?\s*\|\s*(.+?)\s*\|\s*(add|swap|link)\s*\|\s*`([^`]+)`\s*\|$/.exec(line);
      if (!m) continue;
      if (m[1]) file = m[1].replace(/`/g, "");
      out.push({ file, place: m[2], topic: m[4] });
    }
    return out;
  }

  it("names topics that exist once their writers are done", () => {
    const list = rows();
    expect(list.length).toBeGreaterThan(200);
    if (PENDING) return;
    const problems = list.map((r) => {
      const p = topicProblem(r.topic);
      return p ? `${r.file}, ${r.place}: ${p}` : null;
    }).filter((x): x is string => !!x);
    collect(problems);
  });

  it("every icon it lists is in its file", () => {
    if (PENDING) return;
    const problems: string[] = [];
    for (const r of rows()) {
      const src = code[`../${r.file.replace(/^src\//, "")}`];
      if (src === undefined) problems.push(`${r.file}: no such file`);
      else if (!src.includes(`"${r.topic}"`)) problems.push(`${r.file}: no icon for "${r.topic}" (${r.place})`);
    }
    collect(problems);
  });
});
