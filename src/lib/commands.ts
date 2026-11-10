// The command layer: everything the app can do, by id. The ⌘K palette lists
// these, the native menu bar runs them (menuBridge.ts), and windows add their
// own while they're open (useCommands). A command with a `scope` only shows up
// while the window with that id is the active one.
import { useEffect, useSyncExternalStore, type DependencyList } from "react";
import type { LucideIcon } from "lucide-react";
import { toast } from "./toasts";
import { migrateLegacy, scopedKey } from "./dataSource";

export interface Command {
  /** Stable id, e.g. "wm:tile" or "go:dashboard". Menu items use the same ids. */
  id: string;
  title: string;
  /** Palette section, e.g. "Windows", "Nodes", "Cluster". */
  group: string;
  /** The group's name doesn't find it: otherwise "help" would list every Help article. */
  groupUnsearched?: boolean;
  subtitle?: string;
  keywords?: string[];
  icon?: LucideIcon;
  /** Display form, e.g. "⌘K" or "⌥⌘T". */
  shortcut?: string;
  /** true when it can run now, otherwise the reason it can't (shown in the palette). */
  when?: () => true | string;
  /** Needs Admin mode: the palette shows a lock and asks to unlock first. */
  needsAdmin?: boolean;
  /** Verbs the palette matches besides the title's, e.g. ["restart", "reboot"]. */
  verbs?: string[];
  /** What it acts on; its name and id match too ("restart cell3", "logs vllm-head"). */
  entity?: { kind: string; id: string; name: string };
  run: () => unknown;
}

interface Registration {
  cmds: Command[];
  scope?: string;
}

const registrations = new Map<number, Registration>();
const listeners = new Set<() => void>();
let seq = 0;
let activeScope: string | null = null;
let list: Command[] = [];

/** Ids of the commands the active window registered; the palette ranks them higher. */
let activeIds = new Set<string>();

function rebuild() {
  // Later registrations win on duplicate ids, and the active window's own win over app-wide
  // ones (its ⌘T can mean "a shell in the selected container"). Out-of-scope ones are left out.
  const byId = new Map<string, Command>();
  const scoped = new Set<string>();
  for (const pass of [false, true]) {
    for (const r of registrations.values()) {
      if (!!r.scope !== pass || (r.scope && r.scope !== activeScope)) continue;
      for (const c of r.cmds) {
        byId.set(c.id, c);
        if (r.scope) scoped.add(c.id);
      }
    }
  }
  list = [...byId.values()];
  activeIds = scoped;
  listeners.forEach((l) => l());
}

/** The active window's own commands, for ranking. */
export function activeCommandIds(): Set<string> {
  return activeIds;
}

/** Adds commands; returns a function that removes them again. */
export function registerCommands(cmds: Command[], scope?: string): () => void {
  const key = ++seq;
  registrations.set(key, { cmds, scope });
  rebuild();
  return () => {
    registrations.delete(key);
    rebuild();
  };
}

/** Registers commands for as long as the component is mounted; re-registers when deps change. */
export function useCommands(cmds: Command[], deps: DependencyList, opts?: { scope?: string }) {
  useEffect(() => registerCommands(cmds, opts?.scope), [...deps, opts?.scope]);
}

/** The window manager calls this when the active window changes. */
export function setActiveScope(scope: string | null) {
  if (scope === activeScope) return;
  activeScope = scope;
  rebuild();
}

export function getCommands(): Command[] {
  return list;
}

export function getCommand(id: string): Command | undefined {
  return list.find((c) => c.id === id);
}

export function useCommandList(): Command[] {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => list,
  );
}

/** Why a command can't run right now, or null when it can. */
export function unavailableReason(c: Command): string | null {
  if (!c.when) return null;
  try {
    const r = c.when();
    return r === true ? null : r;
  } catch {
    return "Not available right now";
  }
}

// --- Admin hook: wired by App so this module doesn't depend on the admin store.

let unlockAdmin: (reason: string) => Promise<boolean> = async () => false;
let isAdmin: () => boolean = () => false;

export function setAdminHooks(hooks: { isAdmin: () => boolean; unlock: (reason: string) => Promise<boolean> }) {
  isAdmin = hooks.isAdmin;
  unlockAdmin = hooks.unlock;
}

export function adminUnlocked(): boolean {
  return isAdmin();
}

// --- Recently run, for the palette's "Recent" section.

const RECENT_KEY = "sparkbox.palette.recent";
const RECENT_MAX = 8;
// Kept per data source: "Shut down cell3" run in Demo isn't offered first with Live data.
migrateLegacy(RECENT_KEY);

/** The commands run lately with the current data source, newest first. */
export function recentCommandIds(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(scopedKey(RECENT_KEY)) ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function noteRecent(id: string) {
  try {
    const next = [id, ...recentCommandIds().filter((x) => x !== id)].slice(0, RECENT_MAX);
    localStorage.setItem(scopedKey(RECENT_KEY), JSON.stringify(next));
  } catch {
    // Only a convenience.
  }
}

/**
 * Runs a command by id. Unavailable commands explain why in a toast; ones that
 * need Admin mode ask to unlock first. Resolves to whether it ran.
 */
export async function runCommand(id: string, opts: { remember?: boolean } = {}): Promise<boolean> {
  const c = getCommand(id);
  if (!c) return false;
  return runResolved(c, opts);
}

export async function runResolved(c: Command, opts: { remember?: boolean } = {}): Promise<boolean> {
  const reason = unavailableReason(c);
  if (reason) {
    toast({ kind: "info", title: `${c.title}: not available`, body: reason });
    return false;
  }
  if (c.needsAdmin && !isAdmin()) {
    const ok = await unlockAdmin(c.title);
    if (!ok) return false;
  }
  if (opts.remember) noteRecent(c.id);
  try {
    await c.run();
  } catch (e) {
    toast({ kind: "error", title: `${c.title} failed`, body: String(e).replace(/^Error: /, "") });
    return false;
  }
  return true;
}

// --- Ranking for the palette

export interface Ranked {
  cmd: Command;
  score: number;
  /** Indexes into the title that matched, for highlighting. */
  hits: number[];
}

// --- Matching: every word typed must start a word of the title, a verb, a keyword or the
// entity's name ("rest cel3" finds "Restart cell3…"). Words of four letters or more may be
// one typo off (Damerau–Levenshtein ≤ 1). Verbs have synonyms: reboot is restart, power off
// is shut down, log is logs, show is open.

/** Spellings folded to one, on both sides of the match. Multi-word ones are joined first. */
const PHRASES: [RegExp, string][] = [[/\bpower\s*off\b/g, "shut"], [/\bshut\s+down\b/g, "shut down"]];
const SYNONYMS: Record<string, string> = { reboot: "restart", shutdown: "shut", poweroff: "shut", log: "logs", show: "open" };

/** Characters that start a new word: "qsfp56-dd-2-1" has words at each part, and so has "2-1". */
const WORD_SEP = /[\s\-_/.·—@:(),›…]/;

export function canonical(word: string): string {
  return SYNONYMS[word] ?? word;
}

/** The query as canonical lowercase tokens. */
export function tokenize(query: string): string[] {
  let q = query.toLowerCase();
  for (const [re, to] of PHRASES) q = q.replace(re, to);
  return q.split(/\s+/).map((t) => t.replace(/^[“"'(]+|[”"'),.…:]+$/g, "")).filter(Boolean).map(canonical);
}

/** Restricted Damerau–Levenshtein distance (adjacent swaps count as one edit). */
export function editDistance(a: string, b: string): number {
  const d: number[][] = Array.from({ length: a.length + 1 }, (_, i) => [i, ...Array(b.length).fill(0)]);
  for (let j = 1; j <= b.length; j++) d[0][j] = j;
  for (let i = 1; i <= a.length; i++) {
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) d[i][j] = Math.min(d[i][j], d[i - 2][j - 2] + 1);
    }
  }
  return d[a.length][b.length];
}

interface Field {
  text: string;
  lower: string;
  /** Start offsets of its words. */
  starts: number[];
  /** The words, canonical, with where each starts. */
  words: { w: string; at: number }[];
  weight: number;
}

function field(text: string, weight: number): Field {
  const lower = text.toLowerCase();
  const starts: number[] = [];
  const words: { w: string; at: number }[] = [];
  for (let i = 0; i < lower.length; i++) {
    if (WORD_SEP.test(lower[i])) continue;
    if (i === 0 || WORD_SEP.test(lower[i - 1])) {
      starts.push(i);
      let j = i;
      while (j < lower.length && !WORD_SEP.test(lower[j])) j++;
      words.push({ w: canonical(lower.slice(i, j)), at: i });
    }
  }
  return { text, lower, starts, words, weight };
}

/** Tiers, best first: the whole word, the start of a word, one typo away. */
type Tier = 3 | 2 | 1;

interface TokenHit {
  tier: Tier;
  weight: number;
  /** For fuzzy hits: how far the token is from the whole word (closer wins ties). */
  near: number;
  field: Field;
  at: number;
  len: number;
}

function matchToken(tok: string, f: Field): TokenHit | null {
  let best: TokenHit | null = null;
  const better = (h: TokenHit) => !best || h.tier > best.tier || (h.tier === best.tier && h.near < best.near);
  for (const { w, at } of f.words) {
    if (w === tok) return { tier: 3, weight: f.weight, near: 0, field: f, at, len: tok.length };
    if (w.startsWith(tok)) {
      const h: TokenHit = { tier: 2, weight: f.weight, near: w.length - tok.length, field: f, at, len: tok.length };
      if (better(h)) best = h;
    }
  }
  // Tokens can span separators ("2-1" inside "qsfp56-dd-2-1"): a prefix at any word start.
  if (!best && /[\-_/.:]/.test(tok)) {
    for (const at of f.starts) {
      if (f.lower.startsWith(tok, at)) return { tier: 2, weight: f.weight, near: 0, field: f, at, len: tok.length };
    }
  }
  if (best || tok.length < 4) return best;
  // A typo never changes a number: "cel3" is cell3, not cell1.
  const digits = tok.replace(/\D/g, "");
  for (const { w, at } of f.words) {
    if (digits && !w.replace(/\D/g, "").startsWith(digits)) continue;
    let d = Infinity;
    for (const n of [tok.length - 1, tok.length, tok.length + 1]) if (n > 0 && n <= w.length) d = Math.min(d, editDistance(tok, w.slice(0, n)));
    if (d > 1) continue;
    const h: TokenHit = { tier: 1, weight: f.weight, near: editDistance(tok, w), field: f, at, len: Math.min(w.length, tok.length) };
    if (better(h)) best = h;
  }
  return best;
}

export interface MatchResult {
  score: number;
  /** Indexes into the title that matched, for highlighting. */
  hits: number[];
}

/**
 * Matches a query against a command. Null unless every token matches; the score
 * favours exact words over prefixes over typos, the title over other fields, and
 * titles the query covers more of.
 */
export function matchCommand(cmd: Pick<Command, "title" | "keywords" | "verbs" | "entity" | "subtitle" | "group" | "groupUnsearched">, query: string): MatchResult | null {
  const tokens = tokenize(query);
  if (!tokens.length) return { score: 0, hits: [] };
  const title = field(cmd.title, 1);
  const fields = [
    title,
    ...(cmd.verbs ?? []).map((v) => field(v, 0.9)),
    ...(cmd.entity ? [field(cmd.entity.name, 0.9), field(cmd.entity.id, 0.6)] : []),
    ...(cmd.keywords ?? []).map((k) => field(k, 0.7)),
    ...(cmd.groupUnsearched ? [] : [field(cmd.group, 0.4)]),
  ];
  let score = 0;
  const hits: number[] = [];
  const titleWordsHit = new Set<number>();
  for (const tok of tokens) {
    let best: TokenHit | null = null;
    for (const f of fields) {
      const h = matchToken(tok, f);
      if (h && (!best || h.tier * h.weight > best.tier * best.weight || (h.tier * h.weight === best.tier * best.weight && h.near < best.near))) best = h;
    }
    if (!best) return null;
    score += best.tier * 10 * best.weight - Math.min(2, best.near * 0.5);
    if (best.field === title) {
      titleWordsHit.add(best.at);
      for (let i = 0; i < best.len; i++) hits.push(best.at + i);
    }
  }
  // "Restart cell3…" covers more of itself than "Restart vllm-worker on cell3" does.
  score += (titleWordsHit.size / Math.max(1, title.words.length)) * 6;
  score -= Math.min(4, cmd.title.length * 0.02);
  return { score, hits: [...new Set(hits)].sort((a, b) => a - b) };
}

/** Initials: "toc" for "Terminal on cell1". Only when every letter starts a word, in order. */
function acronym(query: string, title: string): MatchResult | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (q.length < 2) return null;
  const f = field(title, 1);
  const hits: number[] = [];
  let wi = 0;
  for (const ch of q) {
    while (wi < f.starts.length && f.lower[f.starts[wi]] !== ch) wi++;
    if (wi >= f.starts.length) return null;
    hits.push(f.starts[wi++]);
  }
  return { score: 8 + q.length, hits };
}

/**
 * Orders commands for a query: the best match first, the active window's own commands
 * and recently run ones nudged up.
 */
export function rankCommands(cmds: Command[], query: string, recent: string[] = [], boost: Set<string> = activeIds): Ranked[] {
  const q = query.trim();
  const out: Ranked[] = [];
  for (const cmd of cmds) {
    const rec = recent.indexOf(cmd.id);
    // Up to +4 for the one run last: enough to settle ties, not to beat a better match.
    const recency = rec >= 0 ? ((recent.length - rec) / recent.length) * 4 : 0;
    if (!q) {
      out.push({ cmd, score: recency, hits: [] });
      continue;
    }
    let m = matchCommand(cmd, q);
    // A subtitle counts only when it contains the query as typed: it's a sentence, not keywords.
    if (!m && cmd.subtitle?.toLowerCase().includes(q.toLowerCase())) m = { score: 6, hits: [] };
    if (!m) m = acronym(q, cmd.title);
    if (m) out.push({ cmd, score: m.score + recency + (boost.has(cmd.id) ? 4 : 0), hits: m.hits });
  }
  // Ties: the shorter title for a query; registration order (the registry's) for an empty palette.
  return out.sort((a, b) => b.score - a.score || (q ? a.cmd.title.length - b.cmd.title.length : 0));
}
