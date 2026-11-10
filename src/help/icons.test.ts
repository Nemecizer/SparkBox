// What an info icon says. The owner's rule is that clicking one explains that thing, so an icon
// can't fall back to its article's general summary, and text kept in the code gets the same
// plain-language rules as text in tips.json.
import { describe, expect, it } from "vitest";
import defaults from "../../src-tauri/src/health/fixtures/default-rules.json";
import { COUNTER_INFO } from "../lib/fabric";
import { getArticle } from "./content";
import { manifest, resolveTopic, splitTopic, tips } from "./meta";
import { headingsOf, slug } from "./render";

const code = import.meta.glob("../**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const sources = Object.entries(code).filter(([file]) => !file.includes(".test.") && !file.includes("/help/"));

/** Every topic the lint can read in the code: an icon's, a section's or a column's. */
const TOPIC = /<(?:HelpTip|InfoTip)\b[^>]*?\btopic="([^"]+)"|\binfoTopic="([^"]+)"|\bhelp: "([a-z0-9][a-z0-9.#-]*)"/g;
const topicsIn = (src: string) => [...src.matchAll(TOPIC)].map((m) => m[1] ?? m[2] ?? m[3]);

/**
 * Icons in other packages that still name a heading. Each one shows its whole article's summary,
 * and that article's How-Tos. Take a topic off this list when its icon gets a tip of its own.
 */
const WAITING_FOR_A_TIP = new Set([
  "providers#add-another-server",
  "leases-and-identity#controller-lease",
  "leases-and-identity#identity-pin",
  "models-hugging-face#fit-table",
  "formats-and-quantization#mlx",
]);

/** Words for people who already know the answer. A tip says what the thing does for the reader. */
const JARGON = /\b(?:perftest|mpirun|all_reduce_perf|ping -M|DCQCN|ECN|CNPs?|NAK(?:ed|s)?|NIC|HCA|PFC|PCIe|lldpd|nvidia-smi|NVML|timedatectl|queue pairs|uninterruptible)\b|\/proc\/|\/interface\/|\/ip\/|\/system\//;

/** The text of a JSX value: no tags, no expressions, and no quotes around a plain string. */
const textOf = (jsx: string) => jsx.replace(/<[^>]*>/g, " ").replace(/\{[^{}]*\}/g, " ").replace(/[{}"]/g, " ").replace(/\s+/g, " ").trim();

/** The value that starts at `at`: a quoted string, or a {…} expression. */
function valueAt(src: string, at: number): string {
  if (src[at] === '"') return src.slice(at + 1, src.indexOf('"', at + 1));
  let depth = 0;
  for (let i = at; i < src.length; i++) {
    if (src[i] === "{") depth++;
    if (src[i] === "}" && --depth === 0) return src.slice(at + 1, i);
  }
  return "";
}

/** What an icon's popover shows from the code: an InfoTip's children and a section's `info`. */
function inlineText(src: string): string[] {
  const children = [...src.matchAll(/<InfoTip\b[^>]*[^/]>([\s\S]*?)<\/InfoTip>/g)].map((m) => m[1]);
  const props = [...src.matchAll(/\binfo=/g)].map((m) => valueAt(src, m.index + "info=".length));
  return [...children, ...props].map(textOf).filter(Boolean);
}

describe("an icon explains its own thing", () => {
  it("a topic never names a bare heading: it would show the whole article's summary", () => {
    const named = sources.flatMap(([file, src]) => topicsIn(src).filter((t) => t.includes("#") && !tips[t] && !WAITING_FOR_A_TIP.has(t)).map((t) => `${file}: "${t}" needs a tip in tips.json`));
    expect(named).toEqual([]);
  });

  it("the waiting list holds only icons that are still waiting", () => {
    const used = new Set(sources.flatMap(([, src]) => topicsIn(src)));
    expect([...WAITING_FOR_A_TIP].filter((t) => !used.has(t))).toEqual([]);
  });

  it("every icon has a topic, so it leads to a guide", () => {
    const bare = sources.flatMap(([file, src]) => [...src.matchAll(/<InfoTip\b(?![^>]*\btopic=)[^>]*>/g)].filter(() => !/components\/ui\.tsx$/.test(file)).map((m) => `${file}: ${m[0]}`));
    expect(bare).toEqual([]);
  });

  it("no two of the monitoring tips say the same thing about different things", () => {
    const seen = new Map<string, string>();
    const twice: string[] = [];
    for (const [id, t] of Object.entries(tips)) {
      if (!id.startsWith("mon.")) continue;
      if (seen.has(t.body)) twice.push(`${id} and ${seen.get(t.body)}`);
      seen.set(t.body, id);
    }
    expect(twice).toEqual([]);
  });
});

describe("text in the code", () => {
  it("the lint reads what an icon really shows", () => {
    const src = `<InfoTip topic="a.b" label="x"><p>{v.detail}</p><p>Run perftest now.</p></InfoTip><SectionTitle title="y" info={<p>Reads /proc/meminfo.</p>} /><SectionTitle title="z" info="Plain words." />`;
    expect(inlineText(src)).toEqual(["Run perftest now.", "Reads /proc/meminfo.", "Plain words."]);
    expect(inlineText(`<InfoTip topic="a.b" />`)).toEqual([]);
    expect(JARGON.test("Run perftest now.")).toBe(true);
    expect(JARGON.test("Reads /proc/meminfo.")).toBe(true);
    expect(JARGON.test("Plain words.")).toBe(false);
  });

  it("uses no expert words in an icon's popover", () => {
    const found = sources.flatMap(([file, src]) => inlineText(src).filter((t) => JARGON.test(t)).map((t) => `${file}: ${t}`));
    expect(found).toEqual([]);
  });

  it("uses no expert words in a fabric counter's explanation", () => {
    expect(Object.entries(COUNTER_INFO).filter(([, t]) => JARGON.test(t))).toEqual([]);
  });

  it("uses no expert words in a monitoring tip", () => {
    expect(Object.entries(tips).filter(([id, t]) => id.startsWith("mon.") && JARGON.test(t.body)).map(([id]) => id)).toEqual([]);
  });
});

interface RuleData {
  id: string;
  title: string;
  warn: number | null;
  crit: number | null;
  clear: number | null;
  forS: number;
  clearS: number;
  counters?: { label: string; warn: number | null; crit: number | null }[];
}

describe("the alert rules", () => {
  const rules = defaults.rules as RuleData[];
  const body = getArticle("alert-rules-reference")!.body;

  /** What's under a rule's "### " heading. */
  function entry(title: string): string | null {
    const at = body.indexOf(`\n### ${title}\n`);
    if (at < 0) return null;
    const rest = body.slice(at + 1).replace(/^.*\n/, "");
    const end = rest.search(/^#{2,3} /m);
    return end < 0 ? rest : rest.slice(0, end);
  }

  const number = (v: number) => Math.abs(v).toLocaleString("en-US");
  /** "120" can read "120 s" or "2 min"; "3600", "1 h". */
  const times = (s: number) => [`${s} s`, ...(s % 60 === 0 ? [`${s / 60} min`] : []), ...(s % 3600 === 0 ? [`${s / 3600} h`] : [])];

  it("each has its own tip, named as on screen, that lands on its own entry", () => {
    const problems = rules.flatMap((r) => {
      const t = tips[`mon.rule.${r.id}`];
      if (!t) return [`${r.id}: no tip`];
      const { id, anchor } = splitTopic(t.article);
      const want = slug(r.title);
      const landing = headingsOf(getArticle(id)!.body).find((h) => h.anchor === anchor);
      return [
        ...(t.title === r.title ? [] : [`${r.id}: the tip says "${t.title}", the screen says "${r.title}"`]),
        ...(id === "alert-rules-reference" && anchor === want ? [] : [`${r.id}: lands on ${t.article}`]),
        ...(landing ? [] : [`${r.id}: no heading #${anchor}`]),
        ...(resolveTopic(`mon.rule.${r.id}`)?.howtos.length ? [] : [`${r.id}: no How-To`]),
        ...(r.clear === null || t.body.includes(String(Math.abs(r.clear))) ? [] : [`${r.id}: the tip doesn't say where it clears, ${r.clear}`]),
      ];
    });
    expect(problems).toEqual([]);
  });

  it("no tip is left over from a rule that's gone", () => {
    const ids = new Set(rules.map((r) => `mon.rule.${r.id}`));
    expect(Object.keys(tips).filter((k) => k.startsWith("mon.rule.") && !ids.has(k))).toEqual([]);
  });

  it("the reference has an entry for each, with the shipped numbers", () => {
    const problems = rules.flatMap((r) => {
      const text = entry(r.title);
      if (text === null) return [`${r.title}: no entry`];
      const missing: string[] = [];
      for (const v of [r.warn, r.crit, r.clear]) if (v && !text.includes(String(Math.abs(v)))) missing.push(number(v));
      for (const s of [r.forS, r.clearS]) if (s > 0 && !times(s).some((t) => text.includes(t))) missing.push(times(s)[0]);
      return missing.length ? [`${r.title}: doesn't say ${missing.join(", ")}`] : [];
    });
    expect(problems).toEqual([]);
  });

  it("the reference counts the rules, and lists each counter with its lines", () => {
    expect(body).toContain(`lists the ${rules.length} rules`);
    const counters = rules.flatMap((r) => r.counters ?? []);
    const problems = counters.flatMap((c) => {
      const row = body.split("\n").find((l) => l.startsWith(`| **${c.label}** |`));
      if (!row) return [`${c.label}: no row`];
      const [, , warn, crit] = row.split("|").map((x) => x.trim());
      return warn === `${number(c.warn ?? 0)}/s` && crit === (c.crit === null ? "None" : `${number(c.crit)}/s`) ? [] : [`${c.label}: reads ${warn}, ${crit}`];
    });
    expect(problems).toEqual([]);
  });

  it("Settings says how many counters there are", () => {
    const n = rules.flatMap((r) => r.counters ?? []).length;
    expect(getArticle("settings-alerts")!.body).toContain(`**${n} counters**`);
  });
});

describe("the map of icons", () => {
  it("still names only articles that exist", () => {
    const ids = new Set(manifest.articles.map((a) => a.id));
    const bad = Object.entries(tips).filter(([id, t]) => id.startsWith("mon.") && !ids.has(splitTopic(t.article).id)).map(([id]) => id);
    expect(bad).toEqual([]);
  });
});
