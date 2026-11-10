// Help's search: instant, ranked, and forgiving of how a person types. Every word in the
// corpus maps to a stem, so "downloading" finds "download". The word being typed may also match
// the start of a word. A word the corpus doesn't have is mended first ("downlaod", "adminmode"),
// so that it reaches phrase matches too. Common words are ignored, so "how do I load a model"
// works like "load model". Ranking is BM25F: title and keywords count most, and long pages get no
// credit for being long. An article needs most of the words, not all of them.
// src/help/search.real.test.ts holds this to the real articles, and search.heldout.test.ts to
// questions it was never tuned on.
import { editDistance } from "../lib/commands";
import type { Heading } from "./render";

export interface SearchDoc {
  id: string;
  title: string;
  type: string;
  summary: string;
  keywords: string[];
  /** What readers type (phrases.json). It finds the page, but never teaches search a word. */
  phrases?: string[];
  headings: Heading[];
  body: string;
  /** A page about the test build: it waits until the query asks for it. */
  testBuild?: boolean;
}

export interface Seg {
  text: string;
  hit: boolean;
}

export interface Hit {
  doc: SearchDoc;
  score: number;
  /** Stems that matched; `segments` marks them. */
  terms: Set<string>;
  /** Not every word of the query is in this article. */
  partial: boolean;
}

const STOP = new Set(
  "a actually all almost already also always an and any anything anyone are as at basically be between but by can cannot cant completely could currently do does doesnt dont entirely even ever every everything for from fully get getting got how i if im in into is isnt it its ive just keep keeps kind lot lots maybe me mostly my nothing now of often old on or our perhaps please probably quite rather really same should so some someone something sometimes stuff sure than that the their them then there these thing things this to totally us very want was we what whats when where which while who whole why will with wont would you your without".split(" "),
);

/** Every page is about these, so they count only when nothing else was typed. */
const OURS = new Set(["sparkbox", "app"]);

/** Words that say what the reader does, not what it's about: "tell" in "tell if I'm in Demo". */
const LIGHT = new Set("able bring change check come display find give go happen help know let look make manage mean need open put say see set show take tell think try turn use used using view way work".split(" "));
/** How much a light word counts. */
const LIGHT_WEIGHT = 0.25;

/** Common words that still change what a phrase means: "model wont load" isn't "model load". */
const NEGATIVE = new Set("cannot cant doesnt dont isnt nothing wont".split(" "));

/** What a reader may type, and the word SparkBox uses (STYLE.md, section 9). Read as stems below. */
const WRITTEN_SYNONYMS: Record<string, string[]> = {
  reboot: ["restart"],
  shutdown: ["shut"],
  poweroff: ["shut"],
  deploy: ["load"],
  launch: ["load"],
  serve: ["load"],
  fetch: ["download"],
  pull: ["download"],
  ram: ["memory"],
  vram: ["memory"],
  alarm: ["alert"],
  mute: ["silence"],
  snooze: ["silence"],
  machine: ["node"],
  host: ["node"],
  computer: ["node"],
  spark: ["node"],
  llm: ["model"],
  ai: ["model"],
  preference: ["setting"],
  password: ["keychain"],
  hot: ["temperature"],
  heat: ["temperature"],
  temp: ["temperature"],
  faster: ["speed"],
  fast: ["speed"],
  slow: ["speed"],
  notify: ["notification"],
  notifi: ["notification"],
  clean: ["delete"],
  cleanup: ["delete"],
  remove: ["delete"],
  erase: ["delete"],
  wipe: ["delete"],
  directory: ["folder"],
  chatgpt: ["openai"],
  administrator: ["root"],
  large: ["big"],
  huge: ["big"],
  drive: ["disk"],
  storage: ["disk"],
  color: ["theme"],
  colour: ["theme"],
  rid: ["delete"],
  broken: ["fail"],
  responding: ["answer", "offline", "reach"],
  forgotten: ["forgot"],
  forget: ["forgot"],
  lost: ["forgot"],
  reply: ["answer"],
  disabled: ["lock"],
  dimmed: ["lock"],
  grey: ["lock"],
  gray: ["lock"],
  greyed: ["lock"],
  grayed: ["lock"],
  nvidia: ["gpu"],
  upgrade: ["update"],
  patch: ["update"],
  chatbot: ["chat"],
  bot: ["chat"],
  assistant: ["chat"],
  talk: ["chat"],
  speak: ["chat"],
  gpt: ["openai"],
  kill: ["stop"],
  terminate: ["stop"],
  halt: ["stop"],
  abort: ["cancel"],
  hung: ["stuck"],
  hang: ["stuck"],
  overheat: ["temperature"],
  thermal: ["temperature"],
  cooling: ["temperature"],
  throughput: ["speed"],
  bandwidth: ["speed"],
  wifi: ["network"],
  ethernet: ["network"],
  internet: ["network"],
  wire: ["cable"],
  price: ["cost"],
  pay: ["cost"],
  charge: ["cost"],
  billing: ["cost"],
  iphone: ["phone"],
  android: ["phone"],
  mobile: ["phone"],
  sms: ["phone"],
  transfer: ["copy"],
  upload: ["copy"],
  sync: ["copy"],
  locate: ["find"],
  modify: ["change"],
  adjust: ["change"],
  tweak: ["change"],
  begin: ["start"],
  uninstall: ["delete"],
  discard: ["delete"],
  purge: ["delete"],
  dead: ["crash"],
  died: ["crash"],
  hotkey: ["shortcut"],
  hottest: ["temperature"],
  coolest: ["temperature"],
  biggest: ["big"],
  largest: ["big"],
  busiest: ["busy"],
  fastest: ["speed"],
  slowest: ["speed"],
  done: ["finish"],
  complete: ["finish"],
  cancel: ["revoke"],
  actual: ["real"],
  cafe: ["vpn"],
  coffee: ["vpn"],
  hotel: ["vpn"],
  airport: ["vpn"],
  travel: ["vpn"],
  traveling: ["vpn"],
  vacation: ["vpn"],
  abroad: ["vpn"],
  program: ["app"],
  software: ["app"],
  application: ["app"],
  fill: ["full"],
  output: ["log"],
  interrupt: ["fail"],
  nonsense: ["garbage"],
  font: ["text"],
  logon: ["login"],
  signin: ["login"],
  workstation: ["node"],
  hardware: ["node"],
  asleep: ["sleep"],
  dozed: ["sleep"],
  night: ["quiet"],
  overnight: ["quiet"],
  nighttime: ["quiet"],
  garbled: ["nonsense", "garbage"],
  gibberish: ["nonsense", "garbage"],
  jibberish: ["nonsense", "garbage"],
  scrambled: ["nonsense", "garbage"],
  junk: ["nonsense", "garbage"],
  blank: ["empty"],
  frozen: ["stuck"],
  freeze: ["stuck"],
  froze: ["stuck"],
  laggy: ["speed"],
  sluggish: ["speed"],
  rollback: ["undo"],
  revert: ["undo"],
  downgrade: ["undo"],
  autostart: ["login"],
  automatically: ["login"],
  popup: ["dialog"],
  credential: ["keychain"],
  secret: ["keychain"],
  quit: ["close"],
  exit: ["close"],
  issue: ["problem"],
  trouble: ["problem"],
  bug: ["problem"],
  glitch: ["problem"],
  hf: ["hugging"],
  warm: ["temperature"],
  warmth: ["temperature"],
  toasty: ["temperature"],
  fan: ["temperature"],
  noisy: ["temperature"],
  loud: ["temperature"],
  usage: ["utilization"],
  utilisation: ["utilization"],
  maxed: ["busy"],
  pegged: ["busy"],
  disappeared: ["gone"],
  vanished: ["gone"],
  missing: ["gone"],
  tried: ["proven"],
  tested: ["proven"],
  verified: ["proven"],
  confirmed: ["proven"],
  repair: ["fix"],
  troubleshoot: ["fix"],
  diagnose: ["fix"],
  solve: ["fix"],
  nickname: ["alias"],
  configuration: ["setting"],
  config: ["setting"],
  unfinished: ["coming"],
  planned: ["coming"],
  upcoming: ["coming"],
  roadmap: ["coming"],
  history: ["activity"],
  audit: ["activity"],
  breaking: ["safe"],
  safely: ["safe"],
  plugged: ["connect"],
  plug: ["connect"],
  unplugged: ["cable"],
  screen: ["window"],
  move: ["copy"],
  duplicate: ["copy"],
  send: ["copy"],
  buy: ["cost"],
  cheap: ["cost"],
  expensive: ["cost"],
  deny: ["refuse"],
  dropdown: ["menu"],
  gauge: ["chart"],
  inactive: ["idle"],
  lag: ["speed"],
  location: ["folder"],
  logout: ["lock"],
  multiple: ["several"],
  navigate: ["find"],
  organize: ["arrange"],
  performance: ["speed"],
  router: ["gateway"],
  scan: ["check"],
  standby: ["sleep"],
  suspend: ["sleep"],
  hibernate: ["sleep"],
  statistic: ["metric"],
  stat: ["metric"],
  tail: ["log"],
  toggle: ["turn"],
  hide: ["silence"],
  dismiss: ["silence"],
  ignore: ["silence"],
  private: ["privacy"],
  confidential: ["privacy"],
  telemetry: ["privacy"],
  tracking: ["privacy"],
  subscription: ["cost"],
  fee: ["cost"],
  console: ["terminal"],
  cli: ["terminal"],
  bash: ["terminal"],
  shell: ["terminal"],
  journal: ["log"],
  journalctl: ["log"],
  dmesg: ["log"],
  syslog: ["log"],
  watt: ["power"],
  wattage: ["power"],
  electricity: ["power"],
  accident: ["accidental"],
};

/** Ways of saying one thing, each folded into the one the articles use. */
const SAYINGS: [RegExp, string][] = [
  [/\b(?:wont|doesnt|didnt|cant|not|stopped) (?:respond|responding)\b/g, "not responding"],
  [/\bunresponsive\b/g, "not responding"],
  [/\bno (?:response|reply|answer)\b/g, "not responding"],
  [/\b(?:could not|couldnt|can not|cannot|unable to|failed to|fails to|will not|wont|does not|doesnt|did not|didnt)\b/g, "cant"],
  [/\bbackups?\b/g, "back up"],
  // "its log on the node" is a record, and "log on to cell2" is signing in. "Logs" is the record
  // too, unless the sentence says who is logging in.
  [/\b(service|container|node|system|error|docker|kernel|boot|activity|model|server|whole|its|a) log (?:in|on)\b/g, "$1 logs"],
  [/\blog (?:in|on) (?=(?:the|my|this|each|every|node|spark|cell\d+|gx10\S*|terminal|containers)\b)/g, "logs "],
  [/\b(sparkbox|it|then|only|report) logs (?:in|into)\b/g, "$1 login"],
  [/\b(?:log|logged|logging|sign|signed|signing) (?:in|into|on)\b/g, "login"],
  [/\bhow tos?\b/g, "howto"],
  [/\beating\b/g, "using"],
  // "What is using my GPU" asks which program, not what a word means. A full disk has its own page.
  [/\b(?:whats|what is|what are|whos|who is) (?:using|taking|hogging|consuming)(?: up| all)*\b(?! (?:my |the |all )*(?:disk|space|storage))/g, "process using"],
  [/\b(?:whats|what is) (?:happening|going on)\b/g, "status"],
  [/\b(?:went|goes|going|go|gone|fell|falls|falling|fallen) (?:to sleep|asleep)\b/g, "sleeps"],
  [/\bget(?:ting)? (?:going|up and running)\b/g, "get started"],
  [/\bby (?:accident|mistake)\b/g, "accidental"],
  [/\b(?:cant|not|never) (?:get|getting|got|receive|receiving)\b/g, "not arriving"],
  [/\bbeen (?:up|on|running)\b/g, "uptime"],
  [/\bup to date\b/g, "updated"],
  // "Every" is a common word, but "on every node" is what a page like Fleet Command is about.
  [/\b(?:on|to|across|for|from) (?:all|every|each)(?: of)?(?: the| my| our)?(?: four| 4)? (?:nodes?|sparks?|machines?|computers?)\b/g, "everynode node"],
  [/\b(?:next to|beside) (?:each other|one another)\b/g, "side by side"],
  [/\b(?:where did|where has) (?:the app|the window|sparkbox|it) (?:go|gone)\b/g, "sparkbox window gone"],
  [/\b(?:isnt|arent|not|cant be) (?:real|actual|genuine)\b/g, "fake"],
];

const said = (text: string) => SAYINGS.reduce((t, [from, to]) => t.replace(from, to), text);

/**
 * A whole complaint and the phrase the articles list for it, once `said` has made every "won't" a
 * "cant". These replace all of what was typed, so they read only what a reader types, never an
 * article.
 */
const COMPLAINTS: [RegExp, string][] = [
  [/\bcant i (?:restart|stop|delete|change|edit|click|press|unload|install|remove|update)\b/, "cant change anything"],
  [/\b(?:cant|not|never|stopped) let(?:s|ting)? me\b.*\b(?:change|changes|changing|edit|anything)\b/, "cant change anything"],
  [/\b(?:refuses?|refused|refusing|blocks?|blocked|blocking|den(?:y|ies|ied)|forbidden|disallowed|not allowed)\b.*\b(?:change|changes|changing|anything)\b/, "cant change anything"],
  [/\b(?:changes?|changing|anything)\b.*\b(?:refused|blocked|denied|forbidden|disallowed|not allowed)\b/, "cant change anything"],
  [/\bi (?:cant|dont|do not) know (?:anything|much|a thing)\b|\bi know (?:nothing|very little|little)\b|\bim (?:a|an|a complete|a total|a real|an absolute) (?:beginner|novice|newbie)\b|\bnever (?:used|done|tried|run)\b/, "im new"],
  [/\bnothing works?\b|\b(?:everything|it all) (?:stopped working|is broken|broke)\b/, "not working"],
  [/^(?!.*\b(?:fail\w*|errors?|stuck|cant|broken|crash\w*)\b).*\b(?:now what|what now|what next|what do i do now|what do i do next)\b/, "what now"],
  [/^what can (?:i|you|it) do(?: with| in)?(?: this| the)?(?: app| program| software| tool| thing)?$/, "what can i do"],
];
/** Longer than this it's a pasted message, not someone saying what's wrong. */
const COMPLAINT_WORDS = 9;

/** Someone asking about the fence is asking about the fence, and its messages say "refused". */
function complaint(text: string): string {
  if (text.split(" ").length > COMPLAINT_WORDS || ASKS_FOR_TEST_BUILD.test(text)) return text;
  return COMPLAINTS.find(([re]) => re.test(text))?.[1] ?? text;
}

/** A word that is a state, not a form of the verb: a node "not responding" isn't "Respond to an alert". */
const WHOLE = new Set(["responding", "setting", "state", "theme"]);

/** A word, with any apostrophe inside it: "doesn't" is one word, and so is "node's". */
const WORD = /[\p{L}\p{N}]+(?:['’][\p{L}]+)*/gu;

const plain = (word: string) => word.toLowerCase().replace(/['’]/g, "");

/** Verbs too short for the rule that finds an "-ing", and their word. */
const SHORT_ING: Record<string, string> = { giving: "give", losing: "lose", making: "make", moving: "move", naming: "name", saving: "save", taking: "take", tuning: "tune", typing: "type", using: "use", waking: "wake", wiping: "wipe" };

/** A light stem: plurals and verb endings fold together. Short words stay as they are. */
export function stem(word: string): string {
  const w = SHORT_ING[plain(word)] ?? plain(word);
  if (/\d/.test(w) || WHOLE.has(w)) return w;
  if (w.length <= 4) return w.length === 4 && w.endsWith("s") && !w.endsWith("ss") ? w.slice(0, 3) : w;
  if (w.endsWith("ies")) return `${w.slice(0, -3)}y`;
  if (w.endsWith("sses") || w.endsWith("xes") || w.endsWith("ches") || w.endsWith("shes")) return w.slice(0, -2);
  let s = w;
  if (s.endsWith("s") && !s.endsWith("ss") && !s.endsWith("us")) s = s.slice(0, -1);
  else if (s.endsWith("ing") && s.length > 6) s = s.slice(0, -3);
  else if (s.endsWith("ed") && s.length > 5) s = s.slice(0, -2);
  // "stopped" → "stop", "running" → "run", but "still" stays whole.
  if (s !== w && !w.endsWith("s") && s.length > 3 && s[s.length - 1] === s[s.length - 2] && !/[lsz]/.test(s[s.length - 1])) s = s.slice(0, -1);
  // "update", "updates", "updating" and "updated" are one word.
  return s.length >= 5 && s.endsWith("e") && !WHOLE.has(s) ? s.slice(0, -1) : s;
}

const LIGHT_STEMS = new Set([...LIGHT].map(stem));

const SYNONYMS: Record<string, string[]> = Object.fromEntries(Object.entries(WRITTEN_SYNONYMS).map(([word, say]) => [stem(word), say.map(stem)]));

function wordsOf(text: string): string[] {
  return (text.match(WORD) ?? []).map(plain);
}

/**
 * BM25F. Each field counts a word by its weight, scaled by how long that field is against the
 * same field elsewhere (`b` says how much), and the sum saturates. So a 20,000-character page of
 * messages doesn't outrank a short How-To for saying "model" two hundred times.
 */
const FIELDS = {
  title: { weight: 10, b: 0.3 },
  keywords: { weight: 7, b: 0.2 },
  headings: { weight: 4, b: 0.7 },
  summary: { weight: 4, b: 0.3 },
  body: { weight: 1, b: 0.8 },
};
const SATURATE = 1.5;
/** One matched word, at full strength, in the units the bonuses below use. */
const WORD_SCORE = 30;

type FieldName = keyof typeof FIELDS;

/** What a heading offers a search. */
interface HeadingIndex {
  stems: Set<string>;
  /** Its words that aren't common ones. */
  content: number;
  phrase: string;
}

interface DocIndex {
  titleStems: string[];
  /** Each keyword as the reader might type the whole of it. */
  keyPhrases: Set<string>;
  /** A keyword of two or more words, by its words: a query can say them in any order, or say more. */
  keySets: string[][];
  /** The terms this page defines: the Glossary's headings. */
  defines: Set<string>;
  headings: HeadingIndex[];
  title: string;
  body: string;
  /** A long page of entries: troubleshooting messages or the glossary. */
  catalogue: boolean;
}

const GLOSSARY = "glossary";

export interface Index {
  docs: SearchDoc[];
  /** stem → doc number → how strongly the doc is about that word, 0 to 1. */
  postings: Map<string, Map<number, number>>;
  /** Every word as written → its stem, from what the articles say. phrases.json adds nothing here. */
  words: Map<string, string>;
  /** The stems of those words. */
  stems: Set<string>;
  /** Per doc, what ranking needs beyond the postings. */
  meta: DocIndex[];
}

const squeeze = (text: string) => wordsOf(text).join(" ");

/** Everything listed for an article, by its writer or from what readers type. */
const keysOf = (d: SearchDoc) => [...d.keywords, ...(d.phrases ?? [])];

const QUESTION = /^(?:how (?:do|can|should|would) (?:i|you|we)|how to|what (?:is|are)(?: the| a| an)?|whats(?: the| a| an)?|where (?:is|are|do|can)(?: i| the)?|why (?:is|does|did|do)|can i|i want to|i need to|i have to|help (?:me )?(?:to )?)\s+/;

/** The words of a phrase that carry its meaning, however it was asked. */
function contentStems(text: string): string[] {
  const words = heard(text).replace(QUESTION, "").split(" ").filter((w) => w && (!STOP.has(w) || NEGATIVE.has(w)));
  const heavy = words.filter((w) => !LIGHT_STEMS.has(stem(w)) && !OURS.has(w));
  return (heavy.length ? heavy : words).map(stem);
}

/** A listed phrase as a query reads once it's tidied, so that one word in it can't lose the match. */
const heard = (text: string) => complaint(said(squeeze(text)));

/** A whole phrase, however it was asked: "how do I restart my Spark" and "restart a node" are the same. */
function intent(text: string): string {
  return contentStems(text).map((s) => SYNONYMS[s]?.[0] ?? s).join(" ");
}

/** "what does ready mean" → the phrase it asks about. Null when it isn't asking what something is. */
const DEFINITION = /^(?:what (?:is|are|does|do)|whats|define|definition of|meaning of)\s+(?:the |a |an )?(.+?)(?:\s+(?:mean|means|stand for|stands for))?$/;

/** What a question asks about, in the reader's words. Null when it isn't asking what something is. */
const definedPhrase = (query: string): string | null => DEFINITION.exec(squeeze(query))?.[1] ?? null;

/** A name to compare with a Glossary heading. Every word counts: "using my gpu" isn't "GPU". */
const nameKey = (text: string) => wordsOf(said(squeeze(text))).map(stem).join(" ");

function definedTerm(query: string): string | null {
  const phrase = definedPhrase(query);
  return phrase ? nameKey(phrase) || null : null;
}

/** "Token (Hugging Face)" and "p50, p95": each way a Glossary heading can be asked for. */
function spellingsOf(heading: string): string[] {
  const bare = heading.replace(/\s*\(.*\)\s*$/, "");
  return [heading, bare, ...bare.split(/,\s*/)];
}

const namesOf = (heading: string) => spellingsOf(heading).map(intent).filter(Boolean);

const CATALOGUE_HEADINGS = 40;

export function buildIndex(docs: SearchDoc[]): Index {
  const words = new Map<string, string>();
  const stemOf = (w: string) => {
    let s = words.get(w);
    if (s === undefined) words.set(w, (s = stem(w)));
    return s;
  };
  const tally = (text: string, learn = true) => {
    const counts = new Map<string, number>();
    let length = 0;
    for (const w of wordsOf(said(squeeze(text)))) {
      const s = learn ? stemOf(w) : stem(w);
      counts.set(s, (counts.get(s) ?? 0) + 1);
      length++;
    }
    return { counts, length };
  };

  const names = Object.keys(FIELDS) as FieldName[];
  const texts = (d: SearchDoc): Record<FieldName, string> => ({
    title: d.title, keywords: d.keywords.join(" "), headings: d.headings.map((h) => h.text).join(" "), summary: d.summary, body: d.body,
  });
  const tallies = docs.map((d) => {
    const t = texts(d);
    const out = Object.fromEntries(names.map((f) => [f, tally(t[f])])) as Record<FieldName, ReturnType<typeof tally>>;
    // A phrase counts as a keyword, but a misspelling in one mustn't become a real word.
    const heard = tally((d.phrases ?? []).join(" "), false);
    for (const [s, n] of heard.counts) out.keywords.counts.set(s, (out.keywords.counts.get(s) ?? 0) + n);
    out.keywords.length += heard.length;
    return out;
  });
  const average = Object.fromEntries(
    names.map((f) => [f, Math.max(1, tallies.reduce((sum, t) => sum + t[f].length, 0) / Math.max(1, docs.length))]),
  ) as Record<FieldName, number>;

  const postings = new Map<string, Map<number, number>>();
  tallies.forEach((t, n) => {
    const weighted = new Map<string, number>();
    for (const f of names) {
      const { weight, b } = FIELDS[f];
      const norm = 1 - b + (b * t[f].length) / average[f];
      for (const [s, tf] of t[f].counts) weighted.set(s, (weighted.get(s) ?? 0) + (weight * tf) / norm);
    }
    for (const [s, w] of weighted) {
      let p = postings.get(s);
      if (!p) postings.set(s, (p = new Map()));
      p.set(n, w / (SATURATE + w));
    }
  });

  const meta = docs.map((d): DocIndex => {
    const listed = keysOf(d);
    return {
      titleStems: wordsOf(d.title).filter((w) => !STOP.has(w)).map(stem),
      // "is it running" says no more than "run": it can't earn a page the credit of a whole phrase.
      keyPhrases: new Set(listed.flatMap((k) => (wordsOf(k).length === 1 || intent(k).includes(" ") ? [heard(k), intent(k)] : [heard(k)])).filter(Boolean)),
      keySets: listed.map((k) => [...new Set(intent(k).split(" "))]).filter((k) => k.length >= 2),
      defines: new Set(d.id === GLOSSARY ? d.headings.flatMap((h) => spellingsOf(h.text).map(nameKey)).filter(Boolean) : []),
      headings: d.headings.map((h) => {
        const all = wordsOf(h.text);
        return { stems: new Set(all.map(stem)), content: new Set(all.filter((w) => !STOP.has(w)).map(stem)).size, phrase: said(squeeze(h.text)) };
      }),
      title: said(squeeze(d.title)),
      body: said(squeeze(d.body)),
      catalogue: d.type === "troubleshooting" || d.headings.length > CATALOGUE_HEADINGS,
    };
  });
  return { docs, postings, words, stems: new Set(words.values()), meta };
}

/** The query's words, without common ones (unless that leaves nothing). */
export function queryWords(query: string): string[] {
  const all = wordsOf(query);
  const kept = all.filter((w) => !STOP.has(w));
  const specific = kept.filter((w) => !OURS.has(w));
  return specific.length ? specific : kept.length ? kept : all;
}

/** Below this a match adds a little to the score, but the article doesn't count as having the word. */
const SOLID = 0.3;
/** How strongly a page must say a word to be about it: a title or several mentions, not one in passing. */
const ABOUT = 0.6;
/** "mode" is a word, and also the start of "model": as a start, it barely counts. */
const WEAK_PREFIX = 0.12;

/** Stems a query word can mean, each with how good a match it is. */
function expand(ix: Index, word: string, last: boolean): Map<string, number> {
  const out = new Map<string, number>();
  const add = (s: string, q: number) => out.set(s, Math.max(out.get(s) ?? 0, q));
  const own = stem(word);
  const exact = ix.postings.has(own);
  if (exact) add(own, 1);
  for (const syn of SYNONYMS[own] ?? []) if (ix.postings.has(syn)) add(syn, 0.7);
  // Two letters start too many words to mean anything.
  if (word.length >= 3 && (last || !exact)) {
    const quality = exact ? WEAK_PREFIX : last ? 0.7 : 0.5;
    for (const [w, s] of ix.words) if (w.startsWith(word) && w !== word) add(s, quality);
  }
  return out;
}

const letters = (word: string) => [...word].sort().join("");

/**
 * The nearest word in the corpus, and of those a swap of two letters first, because that is the
 * commonest slip: "modle" is "model", though "mode" is as near and more common. Then the
 * commonest. A typo never changes a number.
 */
function nearest(ix: Index, word: string): string | null {
  const digits = word.replace(/\D/g, "");
  const sorted = letters(word);
  let best: string | null = null;
  let bestD = (word.length >= 8 ? 2 : 1) + 1;
  let bestSeen = 0;
  for (const [w, s] of ix.words) {
    if (Math.abs(w.length - word.length) > bestD) continue;
    if (digits && w.replace(/\D/g, "") !== digits) continue;
    const d = editDistance(word, w);
    // A typo keeps its first letter, nearly always.
    const seen = (ix.postings.get(s)?.size ?? 0) * (w[0] === word[0] ? 10 : 1) * (d <= bestD && w.length === word.length && letters(w) === sorted ? 100 : 1);
    if (d < bestD || (d === bestD && seen > bestSeen)) {
      best = w;
      bestD = d;
      bestSeen = seen;
    }
  }
  return best;
}

const TYPE_WEIGHT: Record<string, number> = { howto: 1.2, tutorial: 1.15, feature: 1, concept: 1, troubleshooting: 1, reference: 0.9 };
/** A page of entries mentions most things somewhere. It leads only when a heading matches. */
const CATALOGUE_WEIGHT = 0.45;
/** Something went wrong, so a page of messages may be just what's wanted. */
const COMPLAINT = /\b(?:stuck|fail(?:s|ed|ing)?|error|errors|wont|cant|broken|broke|not working|not responding)\b/;
/** The owner's app has no fence, so its pages don't answer "it won't let me change anything". */
const TEST_BUILD_WEIGHT = 0.05;
const ASKS_FOR_TEST_BUILD = /\b(?:fences?|test build|debug build|debug)\b/;
/** How much of a query's weight an article needs, once any article has that much. */
const ENOUGH = 0.3;
/** The most one word can count towards how much of a query an article has. */
const RARE_ENOUGH = 2;
/** A phrase the writer listed for an article, said whole: close to the credit of its title. */
const KEYWORD_PHRASE = 110;
/** More for the reader's exact words, so that they beat a page that lists the same words in another phrase. */
const VERBATIM = 40;

/** "adminmode" → "admin mode", when its two parts are both words. */
function parted(ix: Index, word: string): string | null {
  if (word.length < 6 || /\d/.test(word)) return null;
  const middle = word.length >> 1;
  for (let off = 0; off <= middle - 2; off++) {
    for (const at of off ? [middle - off, middle + off] : [middle]) {
      const [a, b] = [word.slice(0, at), word.slice(at)];
      if (ix.words.has(a) && ix.words.has(b) && !STOP.has(a) && !STOP.has(b)) return `${a} ${b}`;
    }
  }
  return null;
}

/**
 * Real words that only phrases.json uses, each close enough to a word in the articles to pass for
 * a typo of it: "tray" isn't "try". A word goes on this list only if it's spelled right.
 * help.test.ts fails on any word that's close to a common one and isn't here.
 */
export const READER_WORDS = new Set("accent acquire alarm batch bindings bundle coffee combos completely debug define detach detached detected died easiest eating feels fifth friends fullest gigabytes global grab hook hotel interval iperf leaked modify nano noise notified oddly owning party pipe popped popping prune qsfp shop slate strange teach tiny trace trail trigger undelete uninstall utilisation vacation viewer vram vscode weird".split(" "));

/** A word the corpus doesn't have, put right: run together, or a typo. A word still being typed stays. */
function repaired(ix: Index, word: string): string {
  if (ix.words.has(word) || ix.stems.has(stem(word)) || READER_WORDS.has(word) || SYNONYMS[stem(word)] || STOP.has(word)) return word;
  for (const w of ix.words.keys()) if (w.startsWith(word)) return word;
  return parted(ix, word) ?? (word.length >= 4 ? nearest(ix, word) : null) ?? word;
}

const NODE_NAME = /^(?:cell\d+|gx10)$/;
/** Longer than this it's a pasted message, and every word in it is the message's own. */
const SHORT = 6;

/**
 * The query as search reads it, so a typo reaches every phrase match, not only the single words.
 * In "ssh to cell2", cell2 means a node: its rarity says nothing about which page is the answer.
 */
function tidied(ix: Index, query: string): string {
  const words = complaint(said(squeeze(query))).split(" ");
  const named = words.length <= SHORT && words.some((w) => !NODE_NAME.test(w) && !STOP.has(w));
  // The articles write "huggingface" only in addresses, so it's a word until a typo has been mended.
  return said(words.map((w) => (named && NODE_NAME.test(w) ? "node" : repaired(ix, w))).join(" ")).replace(/\bhuggingface\b/g, "hugging face");
}

function ranked(ix: Index, raw: string): Hit[] {
  const query = tidied(ix, raw);
  const qs = queryWords(query);
  if (!qs.length) return [];
  const total = ix.docs.length;
  const idfOf = (s: string) => {
    const df = ix.postings.get(s)?.size ?? 0;
    return Math.log(1 + (total - df + 0.5) / (df + 0.5));
  };
  const lightStems = qs.every((w) => LIGHT_STEMS.has(stem(w))) ? new Set<string>() : LIGHT_STEMS;
  const found = new Map<number, { score: number; hits: number; have: number; about: number; terms: Set<string> }>();
  /** How much each word of the query matters: a rare word more than a common one. */
  let want = 0;
  qs.forEach((word, i) => {
    const stems = expand(ix, word, i === qs.length - 1);
    const weight = lightStems.has(stem(word)) ? LIGHT_WEIGHT : 1;
    if (!stems.size) {
      want += weight;
      return;
    }
    // Capped: a word found in one article shouldn't outweigh the rest of the query.
    const worth = weight * Math.min(RARE_ENOUGH, Math.max(...[...stems].filter(([, q]) => q >= SOLID).map(([s]) => idfOf(s)), 0.2));
    want += worth;
    const best = new Map<number, number>();
    const solid = new Map<number, boolean>();
    const firm = new Map<number, number>();
    const matched = new Map<number, Set<string>>();
    for (const [s, quality] of stems) {
      const idf = idfOf(s);
      for (const [n, strength] of ix.postings.get(s)!) {
        best.set(n, Math.max(best.get(n) ?? 0, weight * quality * idf * strength));
        if (quality < SOLID) continue;
        solid.set(n, true);
        firm.set(n, Math.max(firm.get(n) ?? 0, Math.max(0.5, Math.min(1, strength / ABOUT))));
        let m = matched.get(n);
        if (!m) matched.set(n, (m = new Set()));
        m.add(s);
      }
    }
    for (const [n, value] of best) {
      let e = found.get(n);
      if (!e) found.set(n, (e = { score: 0, hits: 0, have: 0, about: 0, terms: new Set() }));
      e.score += value;
      if (!solid.get(n)) continue;
      e.hits++;
      e.have += worth;
      e.about += worth * firm.get(n)!;
      for (const s of matched.get(n)!) e.terms.add(s);
    }
  });

  const phrase = squeeze(query);
  // "how do i get a model": the phrase the writer listed, whichever way it was asked.
  const bare = `${phrase} `.replace(QUESTION, "").trim();
  const asked = intent(query);
  const askedStems = new Set(asked.split(" "));
  // One word left of a longer query: its other words were light, so it isn't the whole phrase.
  const whole = askedStems.size > 1 || qs.length === 1;
  const term = definedTerm(query);
  const unhappy = COMPLAINT.test(query);
  const stems = qs.map(stem);
  const words = phrase.split(" ").length;
  // A question asks for a guide. A message is quoted or pasted.
  const question = QUESTION.test(`${phrase} `);
  // Most of what the query means, weighing rare words above common ones. An article can lose
  // "difference" and keep "demo" and "live". With nothing like that, anything that has a word.
  const enough = (e: { hits: number; have: number }) => !want || e.have / want >= ENOUGH || (qs.length > 2 && e.hits / qs.length >= ENOUGH);
  const strong = [...found.values()].some(enough);
  const hits: Hit[] = [];
  for (const [n, e] of found) {
    if (!e.hits || (strong && !enough(e))) continue;
    const doc = ix.docs[n];
    const m = ix.meta[n];
    // How much of the query the page has, and how much it's about: a long page says most words
    // somewhere, once.
    const has = want ? e.have / want : 1;
    const coverage = want ? e.about / want : 1;
    let score = (e.score * WORD_SCORE * has * coverage) / Math.sqrt(qs.length);
    score *= TYPE_WEIGHT[doc.type] ?? 1;

    // The query is what the heading is about: "node offline" and "A node shows Offline", but not
    // two common words somewhere in a long message.
    const inHeading = m.headings.some((h) => stems.every((s) => h.stems.has(s) || (SYNONYMS[s] ?? []).some((x) => h.stems.has(x))) && stems.length * 2 >= h.content);
    const quoted = phrase.length > 8 && words >= 3 && m.body.includes(phrase);
    if (m.catalogue && (question || !inHeading) && !quoted && !unhappy) score *= CATALOGUE_WEIGHT;

    if (m.title === phrase) score += 200;
    else if (m.title.startsWith(phrase)) score += 60;
    else if (m.title.includes(phrase)) score += 40;
    const verbatim = m.keyPhrases.has(phrase) || m.keyPhrases.has(bare);
    if ((whole && m.keyPhrases.has(asked)) || verbatim) score += (m.catalogue ? 60 : KEYWORD_PHRASE) + (verbatim && words >= 3 ? VERBATIM : 0);
    else {
      // The same words in another order, or with more said around them.
      const said = Math.max(0, ...m.keySets.filter((k) => k.every((s) => askedStems.has(s))).map((k) => k.length));
      score += KEYWORD_PHRASE * (said / askedStems.size);
    }
    if (words >= 2 && m.headings.some((h) => h.phrase.includes(phrase))) score += words >= 3 ? 90 : m.catalogue ? 15 : 40;
    else if (inHeading && qs.length >= 2) score += question ? 0 : 25;
    else if (quoted) score += 25;
    // Titles about the search first: "Download a model" before "Follow a download", typo or not.
    // A title's rare words say what it's about: "Use Ollama" is about Ollama, not about using.
    if (m.titleStems.length) {
      const worth = (t: string) => idfOf(t) * (lightStems.has(t) ? LIGHT_WEIGHT : 1);
      const named = m.titleStems.filter((t) => e.terms.has(t));
      const share = named.reduce((sum, t) => sum + worth(t), 0) / Math.max(0.01, m.titleStems.reduce((sum, t) => sum + worth(t), 0));
      score += 42 * share * coverage;
    }
    if (doc.testBuild && !ASKS_FOR_TEST_BUILD.test(phrase)) score *= TEST_BUILD_WEIGHT;
    hits.push({ doc, score, terms: e.terms, partial: e.hits < qs.length });
  }
  hits.sort((a, b) => b.score - a.score || a.doc.title.length - b.doc.title.length);
  return term ? withDefinition(ix, hits, term) : hits;
}

const DEFINING_PAGES = new Set(["concept", "feature"]);

/** "What is a GPU": the Glossary answers, after a page that is about exactly that, if there is one. */
function withDefinition(ix: Index, hits: Hit[], term: string): Hit[] {
  const at = hits.findIndex((h) => ix.meta[ix.docs.indexOf(h.doc)].defines.has(term));
  if (at < 0) return hits;
  const about = (h: Hit) => DEFINING_PAGES.has(h.doc.type) && term.split(" ").every((s) => ix.meta[ix.docs.indexOf(h.doc)].titleStems.includes(s));
  const [entry] = hits.splice(at, 1);
  let place = 0;
  while (place < 2 && place < hits.length && about(hits[place])) place++;
  hits.splice(place, 0, entry);
  return hits;
}

/** Ranked articles for a query, best first. An empty query finds nothing. */
export function search(ix: Index, query: string, limit = Infinity): Hit[] {
  const hits = ranked(ix, query);
  return limit === Infinity ? hits : hits.slice(0, limit);
}

/** The results for a query. If it finds nothing, the results for the nearest spelling, and which. */
export function searchOrNearest(ix: Index, query: string): { hits: Hit[]; instead?: string } {
  const hits = search(ix, query);
  if (hits.length) return { hits };
  const near = suggestion(ix, query);
  return near ? { hits: search(ix, near), instead: near } : { hits };
}

/** Splits text into plain and matching runs. */
export function segments(text: string, terms: Set<string>): Seg[] {
  const out: Seg[] = [];
  let at = 0;
  for (const m of text.matchAll(WORD)) {
    if (!terms.has(stem(m[0]))) continue;
    if (m.index > at) out.push({ text: text.slice(at, m.index), hit: false });
    out.push({ text: m[0], hit: true });
    at = m.index + m[0].length;
  }
  if (at < text.length) out.push({ text: text.slice(at), hit: false });
  return out;
}

const SNIPPET = 170;

export function snippet(hit: Hit): { segs: Seg[]; anchor?: string; heading?: string } {
  const { doc, terms } = hit;
  const spots: { at: number; stem: string }[] = [];
  for (const m of doc.body.matchAll(WORD)) {
    const s = stem(m[0]);
    if (terms.has(s)) spots.push({ at: m.index, stem: s });
  }
  if (!spots.length) return { segs: segments(doc.summary, terms) };
  let best = 0;
  let bestScore = -1;
  for (let i = 0; i < spots.length; i++) {
    const seen = new Set<string>();
    for (let j = i; j < spots.length && spots[j].at - spots[i].at < SNIPPET; j++) seen.add(spots[j].stem);
    if (seen.size > bestScore) {
      bestScore = seen.size;
      best = i;
    }
  }
  const spot = spots[best].at;
  const heading = doc.headings.filter((h) => h.at <= spot).at(-1);
  const below = doc.headings.find((h) => h.at > spot);
  // A snippet stays in its section: a heading run into the text around it reads like a typing slip.
  const under = heading ? heading.at + heading.text.length : 0;
  const inHeading = !!heading && spot < under;
  const floor = inHeading ? heading.at : under;
  // The start of the line or sentence, when one is close.
  const before = doc.body.slice(Math.max(floor, spot - 60), spot);
  const cut = Math.max(before.lastIndexOf("\n"), before.lastIndexOf(". ") + 1);
  const from = inHeading ? floor : cut > 0 ? spot - before.length + cut + 1 : Math.max(floor, spot - 40);
  const end = Math.min(from + SNIPPET, below?.at ?? doc.body.length);
  const text = doc.body.slice(from, end);
  // A match in the heading itself: the heading, then what it heads.
  const raw = (inHeading ? `${heading.text} · ${text.slice(heading.text.length)}` : text).replace(/\s*\n\s*/g, " ").replace(/ · +/, " · ").trim().replace(/ ·$/, "");
  const clipped = from + SNIPPET < (below?.at ?? doc.body.length) ? `${raw.replace(/\s+\S*$/, "")}…` : raw;
  const lead = from > floor && doc.body[from - 1] !== "\n" && cut <= 0 ? "…" : "";
  return { segs: segments(lead + clipped, terms), anchor: heading?.anchor, heading: heading?.text };
}

/** Headings every article of a type has: a place in the page's shape, not a topic. */
const GENERIC_HEADINGS = new Set([
  "Before you start", "Steps", "What you should see", "Why it works", "Limits", "If something goes wrong", "Next steps",
  "What you'll do", "What you learned", "What it's for", "Where to find it", "What's on the screen", "What it changes", "Related How-Tos",
  "In short", "How it works", "Why it matters in SparkBox", "Related",
]);

/**
 * Where a result opens: a heading's anchor, or undefined for the top of the article. A How-To
 * opens at its title and prerequisites whenever the query names it. The anchor is for a real
 * section match, such as one message on a page of messages.
 */
export function landing(hit: Hit, query: string): string | undefined {
  const { doc, terms } = hit;
  const words = queryWords(query).map(stem);
  // The query names the page, whatever kind of page it is.
  const title = new Set(wordsOf(doc.title).map(stem));
  if (words.every((w) => title.has(w))) return undefined;

  const headings = doc.headings.filter((h) => !GENERIC_HEADINGS.has(h.text));
  // The query is the section's own name: "Cancel a download". The heading it spells comes first:
  // SYNONYMS folds "Spark" into "node", and a stem folds "Loading" into "Load".
  const defined = definedPhrase(query) ?? query;
  const asked = intent(defined);
  const own = squeeze(defined);
  const exact = headings.find((h) => squeeze(h.text) === own) ?? headings.find((h) => namesOf(h.text).includes(asked));
  if (exact) return exact.anchor;

  const catalogue = doc.type === "troubleshooting" || doc.headings.length > CATALOGUE_HEADINGS;
  const named = new Set([...wordsOf(doc.title), ...keysOf(doc).flatMap(wordsOf)].map(stem));
  const names = (w: string) => [w, ...(SYNONYMS[w] ?? [])].some((x) => named.has(x));
  const sought = new Set(catalogue ? terms : words.filter((w) => !names(w)).flatMap((w) => [w, ...(SYNONYMS[w] ?? [])]));
  if (!sought.size) return undefined;

  let best: Heading | undefined;
  let bestCount = 0;
  let bestLength = Infinity;
  for (const h of headings) {
    const stems = wordsOf(h.text).map(stem);
    const count = new Set(stems.filter((w) => sought.has(w))).size;
    if (count > bestCount || (count > 0 && count === bestCount && stems.length < bestLength)) {
      best = h;
      bestCount = count;
      bestLength = stems.length;
    }
  }
  if (best) return best.anchor;
  if (!catalogue) return undefined;
  // A message's own words may not name it: the entry whose text showed in the result.
  const { heading, anchor } = snippet(hit);
  return heading && !GENERIC_HEADINGS.has(heading) ? anchor : undefined;
}

/** The stems a query matches, to mark them in an article. */
export function matchedTerms(ix: Index, query: string): Set<string> {
  const out = new Set<string>();
  const qs = queryWords(tidied(ix, query));
  qs.forEach((w, i) => {
    for (const [s, quality] of expand(ix, w, i === qs.length - 1)) if (quality >= SOLID) out.add(s);
  });
  return out;
}

/** The nearest words to a query that found nothing: "Did you mean…". */
export function suggestion(ix: Index, query: string): string | null {
  const out: string[] = [];
  let changed = false;
  for (const word of queryWords(query)) {
    if (ix.postings.has(stem(word))) {
      out.push(word);
      continue;
    }
    const best = nearest(ix, word);
    if (!best) return null;
    out.push(best);
    changed = true;
  }
  return changed ? out.join(" ") : null;
}
