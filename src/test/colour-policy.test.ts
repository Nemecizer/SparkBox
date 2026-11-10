// The colour policy (plan §2.1) and the type floor (§1.4, §D11), checked on every
// stylesheet under src/: healthy is quiet, so no border, background, outline or
// shadow is drawn in the "good" green outside a short list of status marks; and no
// font size is a literal px value (sizes come from the --fs-* tokens, so Text size
// in Settings › Appearance scales them all).
import { describe, expect, it } from "vitest";
import { sheets } from "./sheets";

/** Selectors allowed a green paint, each for a reason: they are marks, not surfaces. */
const ALLOWED: [RegExp, string][] = [
  [/\.dot\b|\.sdot\b|\.ok-dot\b|-dot\b/, "status dots"],
  [/\.toast-success\s+\.toast-icon|\.toast-icon\.success/, "the success toast's check icon"],
  [/\.success-check|\.task-icon\.ok/, "the success check"],
  [/\.sb-diff-line\.add/, "diff additions in the editor (a change, not health)"],
];

interface Decl {
  file: string;
  line: number;
  selector: string;
  prop: string;
  value: string;
}

/** Every declaration with its selector, through @media and @container blocks. */
export function declarations(file: string, css: string): Decl[] {
  const out: Decl[] = [];
  const src = css.replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "));
  const lineAt = (i: number) => src.slice(0, i).split("\n").length;
  const walk = (from: number, to: number) => {
    let i = from;
    while (i < to) {
      const open = src.indexOf("{", i);
      if (open < 0 || open >= to) return;
      const selector = src.slice(i, open).trim().replace(/\s+/g, " ");
      let depth = 1;
      let j = open + 1;
      for (; j < to && depth; j++) {
        if (src[j] === "{") depth++;
        else if (src[j] === "}") depth--;
      }
      const close = j - 1;
      if (selector.startsWith("@")) {
        if (/^@(media|container|supports|layer)\b/.test(selector)) walk(open + 1, close);
      } else {
        const body = src.slice(open + 1, close);
        for (const m of body.matchAll(/([\w-]+)\s*:\s*([^;]+)/g)) {
          out.push({ file, line: lineAt(open + 1 + (m.index ?? 0)), selector, prop: m[1].toLowerCase(), value: m[2].trim() });
        }
      }
      i = close + 1;
    }
  };
  walk(0, src.length);
  return out;
}

const all = Object.entries(sheets).flatMap(([file, css]) => declarations(file, css));
const where = (d: Decl) => `${d.file}:${d.line} ${d.selector} { ${d.prop}: ${d.value} }`;

/** A font size with a px value that isn't a var() fallback. */
export function literalPx(value: string): boolean {
  let v = value;
  for (let prev = ""; prev !== v; ) {
    prev = v;
    v = v.replace(/var\([^()]*\)/g, "");
  }
  return /\d(\.\d+)?px/.test(v);
}

describe("colour policy: healthy is quiet", () => {
  it("found the stylesheets", () => {
    expect(Object.keys(sheets).length).toBeGreaterThan(8);
    expect(Object.keys(sheets)).toContain("src/styles.css");
  });

  it("paints no border, background, outline or shadow in the good green outside status marks", () => {
    const green = /var\(\s*--(good|good-bg|ok)\b|var\(\s*--ok[\w-]*/;
    const offenders = all
      .filter((d) => /^(border|background|outline|box-shadow)/.test(d.prop) && green.test(d.value))
      .filter((d) => !ALLOWED.some(([re]) => re.test(d.selector)))
      .map(where);
    expect(offenders).toEqual([]);
  });

  it("uses no literal px font size anywhere", () => {
    const offenders = all.filter((d) => (d.prop === "font-size" || d.prop === "font") && literalPx(d.value)).map(where);
    expect(offenders).toEqual([]);
  });

  it("knows a literal px from a token with a fallback", () => {
    expect(literalPx("var(--fs-sm, 11px)")).toBe(false);
    expect(literalPx("calc(var(--fs-sm, 11px) * 0.9)")).toBe(false);
    expect(literalPx("11px")).toBe(true);
    expect(literalPx("600 10px/1 var(--font)")).toBe(true);
  });
});
