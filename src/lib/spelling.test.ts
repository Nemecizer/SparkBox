// UI text is American. Every string literal and piece of JSX text in the app is read with Vite's
// own parser, so comments and identifiers don't count (`colour` in a variable, `aria-labelledby`
// as an attribute); tests and src/dev don't ship. A value that isn't text the reader sees (a state
// on the wire, a CSS class) is listed in WIRE, whole, and stays as it is. The backend's strings reach
// the reader too (the Activity log's titles, its errors), so its Rust is read the same way.
import { parseSync } from "vite";
import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob(["../**/*.{ts,tsx}", "!../**/*.test.{ts,tsx}", "!../dev/**", "!../test/**"], { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const BACKEND = import.meta.glob(["../../src-tauri/src/**/*.rs", "!../../src-tauri/src/**/*tests.rs", "!../../src-tauri/src/**/fake*.rs"], { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** British forms, as whole words, whatever their case or ending. */
const BRITISH = new RegExp(
  `\\b(${[
    "colou?red", "colour\\w*", "favourit\\w*", "neighbour\\w*", "synchronis\\w*", "\\w*utilis\\w*", "catalogue\\w*", "labell\\w*", "unlabell\\w*", "cancell(ed|ing)", "gr[e]y\\w*", "centre\\w*",
    "summaris\\w*", "prioritis\\w*", "\\w*authoris\\w*", "analys(e|es|ed|ing)", "behaviour\\w*", "honour\\w*", "licence\\w*", "defence\\w*", "fibre\\w*", "metres?", "litres?", "dialogue\\w*",
    "modell\\w*", "signall\\w*", "travell\\w*", "fulfil", "fulfils", "enrol", "enrols",
    // The -ise and -our families, past the ones above.
    "\\w+(is|ys)ation\\w*", "\\w+[^aeiouw\\s]ise(s|d)?", "\\w+[^aeiouw\\s]ising", "\\w+our(s|ed|ing|ful|ite)?",
  ].join("|")})\\b`,
  "gi",
);

/** Ordinary American words those two families also catch. */
const AMERICAN = new Set([
  "advertise", "advertised", "advise", "advised", "advises", "advising", "arise", "arises", "arising", "comprise", "comprised", "comprises", "compromise", "compromised", "concise", "demise", "despise", "devise", "devised",
  "enterprise", "excise", "exercise", "exercised", "exercises", "exercising", "expertise", "franchise", "improvise", "merchandise", "paradise", "precise", "premise", "premises", "promise", "promised", "promises", "promising",
  "revise", "revised", "revising", "supervise", "supervised", "supervising", "surprise", "surprised", "surprises", "surprising", "televise", "treatise",
  "our", "ours", "your", "yours", "hour", "hours", "four", "tour", "tours", "pour", "pours", "sour", "flour", "scour", "detour", "detours", "contour", "contours", "devour",
]);

/** Text that isn't read as words: what crosses the wire or names a CSS class. Whole values, nothing else. */
const WIRE = new Set([
  "cancelled", // A job's, task's or transfer's state, and an error's kind.
  "colour", // A command palette keyword, so searching the British word finds Appearance.
  " labelled", // TimeChart's CSS class.
  "task-icon cancelled", // TasksChip's CSS class.
  "sb-tq-ico cancelled", // The transfer queue's CSS class.
  "Cancelled.", // What admin.rs says to itself for a dismissed Touch ID prompt; the reader gets the caller's own sentence.
]);

/** Help's search: it's there to understand the words a reader types, in either spelling. */
const SEARCH = "../help/search.ts";

/** The words `text` shouldn't say. */
export function britishIn(text: string): string[] {
  if (WIRE.has(text)) return [];
  return [...text.matchAll(BRITISH)].map((m) => m[0]).filter((w) => !AMERICAN.has(w.toLowerCase()));
}

/** Every piece of text in a source file, by line. */
function textsIn(name: string, source: string): { line: number; text: string }[] {
  const { program, errors } = parseSync(name, source, { lang: name.endsWith("x") ? "tsx" : "ts" });
  expect(errors, name).toEqual([]);
  const out: { line: number; text: string }[] = [];
  const add = (at: number, text: string) => out.push({ line: source.slice(0, at).split("\n").length, text });
  const visit = (n: unknown) => {
    if (Array.isArray(n)) return n.forEach(visit);
    if (!n || typeof n !== "object") return;
    const node = n as { type?: string; start: number; value?: unknown; source?: unknown };
    // An import's path, and a type's literal ("running" | "cancelled"), aren't text anyone reads.
    if (node.type === "ImportDeclaration" || node.type === "TSLiteralType") return;
    if (node.type === "Literal" && typeof node.value === "string") add(node.start, node.value);
    if (node.type === "JSXText" && typeof node.value === "string") add(node.start, node.value);
    if (node.type === "TemplateElement") add(node.start, (node.value as { cooked?: string }).cooked ?? "");
    for (const [key, child] of Object.entries(n)) if (key !== "source" || node.type === "ImportExpression") visit(child);
  };
  visit(program);
  return out;
}

/** What in Rust can hold or hide a quote: comments, raw strings (text in 2), strings (text in 3) and chars. */
const RUST_TOKEN = /\/\/[^\n]*|\/\*[\s\S]*?\*\/|r(#*)"([\s\S]*?)"\1|"((?:[^"\\]|\\[\s\S])*)"|'(?:[^'\\\n]|\\.[^']*)'/g;

/** Every string in a Rust file, by line, up to its test module, which doesn't ship. */
function rustTextsIn(source: string): { line: number; text: string }[] {
  const shipped = source.split(/#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s/)[0];
  return [...shipped.matchAll(RUST_TOKEN)]
    .filter((m) => m[2] !== undefined || m[3] !== undefined)
    .map((m) => ({ line: shipped.slice(0, m.index).split("\n").length, text: m[2] ?? m[3] }));
}

describe("the lint itself", () => {
  it("catches each listed form, in any case and with any ending", () => {
    for (const w of ["colour", "Colours", "coloured", "favourite", "Favourites", "neighbour", "synchronised", "utilisation", "catalogue", "labelled", "cancelled", "Cancelled", "grey", "centre", "summarise", "prioritise", "re-prioritise", "authorised", "unauthorised", "analyse", "analysed", "behaviour", "honoured", "normalise", "optimisation", "initialised", "recognise", "organising"]) {
      expect(britishIn(`It was ${w} here.`), w).not.toEqual([]);
    }
  });

  it("leaves American words and listed wire values alone", () => {
    for (const w of ["color", "favorite", "neighbor", "synchronized", "utilization", "catalog", "labeled", "canceled", "gray", "center", "summarize", "your four hours", "otherwise", "exercise", "raise", "precise", "expertise", "cancelled", "noise", "promises", "our", "tour"]) {
      expect(britishIn(w), w).toEqual([]);
    }
    expect(britishIn("task-icon cancelled")).toEqual([]);
  });

  it("reads literals, templates and JSX text, and skips comments, types and imports", () => {
    const seen = textsIn("x.tsx", `
      // A colour in a comment.
      import x from "./colour";
      type T = "colour" | "ok";
      const colour = "plain";
      const a = "one colour", b = \`two colour \${a} three colour\`;
      const c = <p title="four colour">five colour</p>;
    `);
    expect(seen.flatMap((t) => britishIn(t.text))).toEqual(["colour", "colour", "colour", "colour", "colour"]);
  });
});

describe("the lint on Rust", () => {
  it("reads strings of every kind, and skips comments, chars and the test module", () => {
    const seen = rustTextsIn(`
      // A colour in a comment.
      /* A colour in a block. */
      let q = '"'; let e = '\\''; let l: &'static str = "one colour";
      let raw = r#"two "colour""#;
      let esc = "three \\"colour\\"";
      #[cfg(test)]
      fn helper_before_the_end() { let x = "four colour"; }
      #[cfg(test)]
      mod tests { let x = "five colour"; }
    `);
    expect(seen.flatMap((t) => britishIn(t.text))).toEqual(["colour", "colour", "colour", "colour"]);
    expect(seen.map((t) => t.line)).toEqual([4, 5, 6, 8]);
  });
});

describe("UI text", () => {
  it("found the app's sources", () => {
    expect(Object.keys(SOURCES).length).toBeGreaterThan(200);
  });

  it("is American", () => {
    const found = Object.entries(SOURCES)
      .filter(([name]) => name !== SEARCH)
      .flatMap(([name, source]) => textsIn(name, source).flatMap((t) => britishIn(t.text).map((w) => `${name.replace("../", "src/")}:${t.line} ${w}: ${t.text.trim().slice(0, 70)}`)));
    expect(found).toEqual([]);
  });

  it("is American in the backend too", () => {
    expect(Object.keys(BACKEND).length).toBeGreaterThan(100);
    const found = Object.entries(BACKEND).flatMap(([name, source]) => rustTextsIn(source).flatMap((t) => britishIn(t.text).map((w) => `${name.replace("../../", "")}:${t.line} ${w}: ${t.text.trim().slice(0, 70)}`)));
    expect(found).toEqual([]);
  });
});
