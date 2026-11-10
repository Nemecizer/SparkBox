// Checks the colour tokens in styles.css: WCAG contrast for every pair the UI
// relies on (all three themes, plus Increase contrast), the OKLCH neutral ramp,
// and the rules that keep metric colours distinct. Parses the CSS text itself,
// so a token edit that breaks legibility fails here. Black is true black: its
// desk is #000000, the window surface is barely lifted off it (so a window floats
// by a 1 px ring, not by lightness), and its metric chroma cap is higher (0.19)
// because on black the saturation is what makes a line visible.
import { describe, expect, it } from "vitest";
import css from "../styles.css?raw";
import { sheets } from "./sheets";

type Tokens = Record<string, string>;

function block(selector: string, from = 0): string {
  const start = css.indexOf(`${selector} {`, from);
  if (start < 0) throw new Error(`no block ${selector}`);
  const open = css.indexOf("{", start);
  let depth = 0;
  for (let i = open; i < css.length; i++) {
    if (css[i] === "{") depth++;
    else if (css[i] === "}" && --depth === 0) return css.slice(open + 1, i);
  }
  throw new Error(`unterminated ${selector}`);
}

function parse(body: string): Tokens {
  const out: Tokens = {};
  for (const m of body.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) out[m[1]] = m[2].trim();
  return out;
}

const light = parse(block(":root"));
const darkBody = block(':root[data-theme="dark"]');
const dark: Tokens = { ...light, ...parse(darkBody) };
const mediaDark = block(':root:not([data-theme="light"])', css.indexOf("@media (prefers-color-scheme: dark)"));
const blackBody = block(':root[data-theme="black"]');
const black: Tokens = { ...light, ...parse(blackBody) };
const more = parse(block(':root[data-contrast="more"]'));
const mediaMore = parse(block(':root:not([data-contrast])', css.indexOf("@media (prefers-contrast: more)")));
// Black's own Increase-contrast step (its inactive selection is already gray-6).
const moreBlack = parse(block(':root[data-theme="black"][data-contrast="more"]'));
const mediaMoreBlack = parse(block(':root[data-theme="black"]:not([data-contrast])', css.indexOf("@media (prefers-contrast: more)")));
const isBlack = (name: string) => name.startsWith("black");

type RGB = [number, number, number];

function hexToRgb(hex: string): RGB {
  const h = hex.length === 4 ? `#${hex[1]}${hex[1]}${hex[2]}${hex[2]}${hex[3]}${hex[3]}` : hex;
  const n = parseInt(h.slice(1, 7), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** Resolves a token to sRGB: hex, var() chains and `color-mix(in srgb, A p%, B)`. */
function resolve(t: Tokens, value: string, depth = 0): RGB {
  if (depth > 12) throw new Error(`cycle at ${value}`);
  const v = value.trim();
  if (v.startsWith("#")) return hexToRgb(v);
  const varm = /^var\((--[\w-]+)(?:,\s*(.+))?\)$/.exec(v);
  if (varm) {
    const next = t[varm[1]] ?? varm[2];
    if (next === undefined) throw new Error(`undefined ${varm[1]}`);
    return resolve(t, next, depth + 1);
  }
  const mix = /^color-mix\(in srgb,\s*(.+?)\s+(\d+(?:\.\d+)?)%,\s*(.+)\)$/.exec(v);
  if (mix) {
    const a = resolve(t, mix[1], depth + 1);
    const b = resolve(t, mix[3], depth + 1);
    const p = Number(mix[2]) / 100;
    return [0, 1, 2].map((i) => a[i] * p + b[i] * (1 - p)) as RGB;
  }
  throw new Error(`can't resolve ${value}`);
}

const lin = (c: number) => {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
};
const luminance = ([r, g, b]: RGB) => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);

function ratio(t: Tokens, a: string, b: string): number {
  const la = luminance(resolve(t, `var(${a})`));
  const lb = luminance(resolve(t, `var(${b})`));
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

function oklch([r, g, b]: RGB): [number, number, number] {
  const [R, G, B] = [lin(r), lin(g), lin(b)];
  const l = Math.cbrt(0.4122214708 * R + 0.5363325363 * G + 0.0514459929 * B);
  const m = Math.cbrt(0.2119034982 * R + 0.6806995451 * G + 0.1073969566 * B);
  const s = Math.cbrt(0.0883024619 * R + 0.2817188376 * G + 0.6299787005 * B);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const Bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [L, Math.hypot(A, Bb), ((Math.atan2(Bb, A) * 180) / Math.PI + 360) % 360];
}

/** [foreground, background, minimum]: everything the UI needs to stay legible. */
const PAIRS: [string, string, number][] = [
  ["--text", "--surface", 7],
  ["--text", "--chrome", 7],
  ["--sel-text", "--sel", 4.5],
  ["--text", "--sel-inactive", 7],
  ["--text", "--title-active", 7],
  ["--text-2", "--surface", 4.5],
  ["--text-2", "--chrome", 4.5],
  ["--muted", "--surface", 4.5],
  ["--muted", "--surface-2", 4.5],
  ["--muted", "--chrome", 4.5],
  ["--muted", "--title-inactive", 4.5],
  ["--accent-text", "--surface", 4.5],
  ["--accent-text", "--chrome", 4.5],
  ["--on-accent", "--accent-fill", 4.5],
  ["--on-accent", "--danger-fill", 4.5],
  ["--good-text", "--surface", 4.5],
  ["--warn-text", "--surface", 4.5],
  ["--serious-text", "--surface", 4.5],
  ["--crit-text", "--surface", 4.5],
  ["--good-text", "--good-bg", 4.5],
  ["--warn-text", "--warn-bg", 4.5],
  ["--crit-text", "--crit-bg", 4.5],
  ["--accent-text", "--info-bg", 4.5],
  ["--demo-text", "--demo-bg", 4.5],
  ["--field-border", "--surface", 3],
  ["--focus-ring", "--surface", 3],
  ["--warning-border", "--surface", 3],
  ["--critical", "--surface", 3],
  ["--hairline", "--surface", 1.4],
  ["--zebra", "--surface", 1.12],
  ["--surface", "--desk", 1.3],
  // A hovered row takes --text-2 and --sel-link, like a selected one (--muted and the accent are under AA on --hover).
  ["--text-2", "--hover", 4.5],
  ["--hover-link", "--hover", 4.5],
];

describe.each([
  ["light", light],
  ["dark", dark],
  ["black", black],
  ["light, increased contrast", { ...light, ...more }],
  ["dark, increased contrast", { ...dark, ...more }],
  ["black, increased contrast", { ...black, ...more, ...moreBlack }],
])("%s theme contrast", (name, t) => {
  // Black's window is a barely lifted step above its #000 desk: it floats by the ring in its shadow instead.
  it.each(PAIRS.filter(([fg, bg]) => !(isBlack(name) && fg === "--surface" && bg === "--desk")))("%s on %s ≥ %s:1", (fg, bg, min) => {
    expect(ratio(t, fg, bg)).toBeGreaterThanOrEqual(min);
  });
  if (isBlack(name)) {
    it("lifts the window surface barely off the true black desk (desk < window < tile)", () => {
      const [Ld] = oklch(resolve(t, "var(--desk)"));
      const [Ls] = oklch(resolve(t, "var(--surface)"));
      const [Lt] = oklch(resolve(t, "var(--surface-2)"));
      expect(Ld).toBe(0);
      expect(Ls).toBeGreaterThanOrEqual(0.10);
      expect(Ls).toBeLessThanOrEqual(0.17);
      expect(Lt).toBeGreaterThan(Ls);
    });
    it("keeps selection, hover, gridlines and idle wires visible on the lifted black surface", () => {
      // Tints: a 14% wash or gray-4 on near-black is 1.2–1.5:1, invisible on an OLED.
      expect(ratio(t, "--sel", "--surface")).toBeGreaterThanOrEqual(1.8);
      expect(ratio(t, "--sel-inactive", "--surface")).toBeGreaterThanOrEqual(1.7);
      expect(ratio(t, "--hover", "--surface")).toBeGreaterThanOrEqual(1.4);
      // Increase contrast lifts the zebra to gray-4, so its hover steps to gray-6: a striped row still shows a hover.
      expect(ratio(t, "--hover", "--zebra")).toBeGreaterThanOrEqual(1.25);
      // An inactive selection is one ramp step above the hover (gray-6 over gray-5 is 1.23:1; gray-7 over gray-6 under Increase contrast).
      expect(ratio(t, "--sel-inactive", "--hover")).toBeGreaterThanOrEqual(1.2);
      expect(ratio(t, "--accent", "--sel")).toBeGreaterThanOrEqual(3);
      // Gridlines one step off the surface but present; idle wires at the non-text floor even when dashed.
      expect(ratio(t, "--chart-grid", "--surface")).toBeGreaterThanOrEqual(1.5);
      expect(ratio(t, "--line-idle", "--surface")).toBeGreaterThanOrEqual(4.5);
    });
    it("rings every window with a 1 px edge that reads on the black desk", () => {
      for (const k of ["--shadow-active", "--shadow-inactive"]) {
        const ring = /0 0 0 1px (#[0-9a-f]{6})/.exec(t[k])?.[1];
        expect(ring, k).toBeDefined();
        const la = luminance(hexToRgb(ring!));
        const lb = luminance(resolve(t, "var(--desk)"));
        expect((Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)).toBeGreaterThanOrEqual(1.5);
      }
    });
  }
});

describe("selection", () => {
  it.each([["light", light], ["dark", dark], ["black", black]] as const)("keeps body text at 7:1 on the selection (%s)", (_, t) => {
    expect(ratio(t, "--text", "--sel")).toBeGreaterThanOrEqual(7);
    expect(t["--sel-text"]).toBe("var(--text)");
  });
  // --muted is under 4.5:1 on every theme's --sel, so a selected row's secondary text takes --sel-muted.
  it.each([["light", light], ["dark", dark], ["black", black]] as const)("keeps secondary text at AA on the selection (%s)", (_, t) => {
    for (const bg of ["--sel", "--sel-inactive"]) {
      expect(ratio(t, "--sel-muted", bg), bg).toBeGreaterThanOrEqual(4.5);
      expect(ratio(t, "--text-2", bg), bg).toBeGreaterThanOrEqual(4.5);
    }
  });
  // The accent is 4.1–4.2:1 on --sel and 3.5–4.4 on --sel-inactive, so links on a selected row take --sel-link, a
  // lighter (Dark, Black) or darker (Light) accent step; the underline stays, so a link is never colour alone.
  it.each([["light", light], ["dark", dark], ["black", black]] as const)("keeps links at AA on the selection (%s)", (_, t) => {
    for (const bg of ["--sel", "--sel-inactive"]) expect(ratio(t, "--sel-link", bg), bg).toBeGreaterThanOrEqual(4.5);
    expect(ratio(t, "--sel-link", "--surface")).toBeGreaterThanOrEqual(4.5);
  });
});

describe("keyboard row (C5)", () => {
  // The row the arrows act on in a focused table: a light accent tint and a 2 px accent bar.
  const kbd = /--kbd-row:\s*([^;]+);/.exec(css)?.[1] ?? "";
  const lum = (t: Tokens, v: string) => luminance(resolve(t, v));
  const contrast = (a: number, b: number) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  it.each([
    ["light", light],
    ["dark", dark],
    ["black", black],
    ["light, increased contrast", { ...light, ...more }],
    ["dark, increased contrast", { ...dark, ...more }],
    ["black, increased contrast", { ...black, ...more, ...moreBlack }],
  ] as const)("keeps text legible and the row and its bar visible (%s)", (_, t) => {
    expect(kbd).toMatch(/color-mix/);
    // Black mixes its own, stronger tint (the 14% wash is invisible on #000).
    const row = lum(t, t["--kbd-row"] ?? kbd);
    // Body text stays at AA on the tint.
    expect(contrast(lum(t, "var(--text)"), row)).toBeGreaterThanOrEqual(7);
    // The 2 px bar is a non-text indicator: 3:1 against the row it marks (WCAG 1.4.11).
    expect(contrast(lum(t, "var(--accent)"), row)).toBeGreaterThanOrEqual(3);
    // The tint reads apart from a plain and a zebra row.
    expect(Math.abs(row - lum(t, "var(--surface)"))).toBeGreaterThan(0.01);
    expect(row).not.toBeCloseTo(lum(t, "var(--zebra)"), 3);
    // And the focus ring around the list stands out from the window.
    expect(contrast(lum(t, "var(--focus-ring)"), lum(t, "var(--surface)"))).toBeGreaterThanOrEqual(3);
    // Secondary text and links on the row take --text-2 and --hover-link (--muted and the accent are 4.3–4.5:1 on the tint).
    expect(contrast(lum(t, "var(--text-2)"), row)).toBeGreaterThanOrEqual(4.5);
    expect(contrast(lum(t, "var(--hover-link)"), row)).toBeGreaterThanOrEqual(4.5);
  });
});

describe("increased contrast", () => {
  it.each([["light", light], ["dark", dark], ["black", black]] as const)("strengthens lines and selection (%s)", (name, base) => {
    const t = { ...base, ...more, ...(name === "black" ? moreBlack : {}) };
    expect(ratio(t, "--hairline", "--surface")).toBeGreaterThan(ratio(base, "--hairline", "--surface"));
    expect(ratio(t, "--border", "--surface")).toBeGreaterThanOrEqual(3);
    expect(ratio(t, "--sel-text", "--sel")).toBeGreaterThanOrEqual(4.5);
    expect(t["--focus-width"]).toBe("3px");
    // The solid selection is --sel-strong, not the accent fill: a focused selected row keeps its ring (1.4.11).
    expect(ratio(t, "--focus-ring", "--sel")).toBeGreaterThanOrEqual(3);
    expect(ratio(t, "--sel-muted", "--sel")).toBeGreaterThanOrEqual(4.5);
    expect(ratio(t, "--sel-link", "--sel")).toBeGreaterThanOrEqual(4.5);
    // Gridlines step up with the hairlines.
    expect(ratio(t, "--chart-grid", "--surface")).toBeGreaterThanOrEqual(3);
    // The zebra lifts to gray-4, and the hover still reads on it (gray-5 would be 1.15:1); text stays AA on the hover.
    expect(ratio(t, "--hover", "--zebra")).toBeGreaterThanOrEqual(1.25);
    expect(ratio(t, "--text", "--hover")).toBeGreaterThanOrEqual(7);
    expect(ratio(t, "--text-2", "--hover")).toBeGreaterThanOrEqual(4.5);
    expect(ratio(t, "--text", "--sel-inactive")).toBeGreaterThanOrEqual(7);
    // Thresholds and the offline hatch strengthen too, in the app setting and the macOS one alike.
    expect(ratio(t, "--chart-threshold", "--surface")).toBeGreaterThanOrEqual(7);
    expect(ratio(t, "--hatch", "--surface-2")).toBeGreaterThanOrEqual(4.5);
  });
  it("keeps the macOS copy identical to the in-app block", () => {
    expect(mediaMore).toEqual(more);
    expect(mediaMoreBlack).toEqual(moreBlack);
  });
});

describe("neutral ramp", () => {
  it.each([["light", light], ["dark", dark], ["black", black]] as const)("is 12 OKLCH steps at hue 255, chroma 0.005–0.012 (%s)", (name, t) => {
    const steps = Array.from({ length: 12 }, (_, i) => oklch(resolve(t, `var(--gray-${i + 1})`)));
    // Black's desk (step 1) is true black: no lightness, no chroma. Its surface (step 2) is barely lifted.
    const first = isBlack(name) ? 1 : 0;
    for (const [L, C] of steps.slice(0, first)) {
      expect(L).toBe(0);
      expect(C).toBe(0);
    }
    for (const [L, C, h] of steps.slice(first)) {
      // 0.005 at the near-white end rounds to ~0.0035 once stored as a hex value.
      expect(C).toBeGreaterThanOrEqual(0.003);
      expect(C).toBeLessThanOrEqual(0.0135);
      if (C > 0.004) expect(Math.abs(h - 255)).toBeLessThan(14);
      expect(L).toBeGreaterThan(0);
    }
    // Steps 2–12 run one way: from the window surface to the strongest text.
    const Ls = steps.slice(1).map((s) => s[0]);
    const dir = Math.sign(Ls[Ls.length - 1] - Ls[0]);
    for (let i = 1; i < Ls.length; i++) expect(Math.sign(Ls[i] - Ls[i - 1])).toBe(dir);
  });

  it("matches the OKLCH values documented next to each step", () => {
    for (const body of [block(":root"), darkBody, blackBody]) {
      for (const m of body.matchAll(/--gray-(\d+):\s*(#[0-9a-f]{6});\s*\/\* oklch\(([\d.]+)% ([\d.]+) (\d+)\) \*\//g)) {
        const [L, C] = oklch(hexToRgb(m[2]));
        expect(Math.abs(L * 100 - Number(m[3]))).toBeLessThan(0.6);
        expect(Math.abs(C - Number(m[4]))).toBeLessThan(0.0025);
      }
    }
  });

  it("maps roles onto the documented steps", () => {
    const map: Record<string, number> = { "--desk": 1, "--surface": 2, "--surface-2": 3, "--zebra": 3, "--chrome": 3, "--hover": 4, "--hairline": 5, "--track": 5, "--border": 6, "--muted": 9, "--text-2": 11, "--text": 12 };
    for (const [role, step] of Object.entries(map)) expect(light[role]).toBe(`var(--gray-${step})`);
  });
});

describe("theme blocks", () => {
  it("keeps the System dark copy identical to data-theme=dark", () => {
    const norm = (s: string) => JSON.stringify(parse(s));
    expect(norm(mediaDark)).toBe(norm(darkBody));
  });

  it("defines every C1 token in every theme", () => {
    const required = [
      "--fs-2xs", "--fs-xs", "--fs-sm", "--fs-base", "--fs-md", "--fs-lg", "--fs-xl", "--fw-regular", "--fw-medium", "--fw-semibold",
      "--sp-1", "--sp-7", "--r-sm", "--r-md", "--r-lg", "--r-xl", "--dur-1", "--dur-2", "--dur-3", "--ease",
      "--surface-3", "--zebra", "--sel-inactive", "--overlay", "--focus-ring", "--accent-fill", "--accent-text", "--on-accent",
      "--btn-disabled-bg", "--btn-disabled-fg", "--title-text-inactive", "--good-bg", "--warn-bg", "--crit-bg", "--info",
      "--m-cpu", "--m-gpu", "--m-mem", "--m-mem-gpu", "--m-mem-cache", "--m-power", "--m-temp", "--m-rx", "--m-tx", "--m-read", "--m-write", "--m-rdma", "--m-tcp",
      "--node-1", "--node-2", "--node-3", "--node-4", "--node-5", "--node-6",
      "--shadow-active", "--shadow-inactive", "--shadow-pop", "--z-dock", "--z-popover", "--z-toast", "--z-dialog",
    ];
    for (const t of [light, dark, black]) for (const k of required) expect(t[k], k).toBeDefined();
  });

  it("uses font-size tokens everywhere outside the token definitions", () => {
    const literal = css.split("\n").filter((l) => /font(-size)?:\s*[\d.]+px/.test(l) && !/--fs-/.test(l.split(":")[0]));
    expect(literal).toEqual([]);
  });

  it("reduces motion to zero", () => {
    const reduce = parse(block(':root[data-motion="reduce"]'));
    expect([reduce["--dur-1"], reduce["--dur-2"], reduce["--dur-3"]]).toEqual(["0ms", "0ms", "0ms"]);
  });
});

describe("metric colours", () => {
  it.each([["light", light], ["dark", dark], ["black", black]] as const)("pair tones share a hue, families don't (%s)", (_, t) => {
    const c = (k: string) => oklch(resolve(t, `var(${k})`));
    const hueGap = (a: number, b: number) => Math.min(Math.abs(a - b), 360 - Math.abs(a - b));
    for (const [a, b] of [["--m-rx", "--m-tx"], ["--m-read", "--m-write"]]) {
      expect(hueGap(c(a)[2], c(b)[2])).toBeLessThan(10);
      expect(Math.abs(c(a)[0] - c(b)[0])).toBeGreaterThan(0.12);
    }
    expect(hueGap(c("--m-rdma")[2], c("--m-tcp")[2])).toBeGreaterThan(60);
    expect(hueGap(c("--m-cpu")[2], c("--m-gpu")[2])).toBeGreaterThan(60);
    // Page cache is neutral, so it never reads as "healthy" green.
    expect(c("--m-mem-cache")[1]).toBeLessThan(0.03);
    const families = ["--m-cpu", "--m-gpu", "--m-mem", "--m-power", "--m-temp", "--m-tcp", "--m-read", "--m-rdma"].map((k) => c(k)[2]);
    for (let i = 0; i < families.length; i++) for (let j = i + 1; j < families.length; j++) expect(hueGap(families[i], families[j])).toBeGreaterThan(14);
  });
});

// ---- §2.2 metric hues and the §D11 type floor

const hueGap = (a: number, b: number) => Math.min(Math.abs(a - b), 360 - Math.abs(a - b));
const METRICS = ["--m-cpu", "--m-gpu", "--m-mem-gpu", "--m-mem", "--m-power", "--m-temp", "--m-rx", "--m-tx", "--m-tcp", "--m-rdma", "--m-read", "--m-write", "--m-ttft", "--m-itl"];
const NODES = ["--node-1", "--node-2", "--node-3", "--node-4", "--node-5", "--node-6"];

describe("metric hues keep clear of the alarm colours (§2.2)", () => {
  // The chroma cap keeps a series from reading as an alarm; Black's is higher (0.19) because on
  // true black saturation is what makes a line visible, and the 30° rule still holds there.
  it.each([["light", light, 0.161], ["dark", dark, 0.161], ["black", black, 0.195]] as const)("are 30° or more from --crit and --warn, within the chroma cap (%s)", (_, t, cap) => {
    const c = (k: string) => oklch(resolve(t, `var(${k})`));
    const crit = c("--crit")[2];
    const warn = c("--warn")[2];
    for (const k of [...METRICS, ...NODES]) {
      const [, C, h] = c(k);
      expect(C, `${k} chroma`).toBeLessThanOrEqual(cap);
      expect(hueGap(h, crit), `${k} vs --crit`).toBeGreaterThanOrEqual(30);
      expect(hueGap(h, warn), `${k} vs --warn`).toBeGreaterThanOrEqual(30);
    }
  });

  it.each([["light", light], ["dark", dark], ["black", black]] as const)("draws TTFT and ITL in indigo and violet, apart from each other (%s)", (_, t) => {
    const c = (k: string) => oklch(resolve(t, `var(${k})`));
    for (const k of ["--m-ttft", "--m-itl"]) {
      const h = c(k)[2];
      expect(h, k).toBeGreaterThanOrEqual(260);
      expect(h, k).toBeLessThanOrEqual(320);
    }
    expect(hueGap(c("--m-ttft")[2], c("--m-itl")[2])).toBeGreaterThan(14);
  });

  it.each([["light", light], ["dark", dark], ["black", black]] as const)("pairs disk read and write in one hue family, 25 or more apart in lightness (%s)", (_, t) => {
    const [Lr, , hr] = oklch(resolve(t, "var(--m-read)"));
    const [Lw, , hw] = oklch(resolve(t, "var(--m-write)"));
    expect(hueGap(hr, hw)).toBeLessThan(10);
    expect(Math.abs(Lr - Lw)).toBeGreaterThanOrEqual(0.25);
  });

  it("names the alarm hues and the state tokens in every theme", () => {
    for (const t of [light, dark, black]) {
      for (const k of ["--crit", "--warn", "--dot-ok", "--state-warn-edge", "--state-warn-band", "--state-crit-edge", "--state-crit-band", "--m-ttft", "--m-itl"]) expect(t[k], k).toBeDefined();
      // Healthy dots are quiet: low chroma, unlike the alarm colours.
      expect(oklch(resolve(t, "var(--dot-ok)"))[1]).toBeLessThan(0.08);
    }
  });

  it("defines the drawing-rule tokens once in :root and turns them up in Black", () => {
    const rules = ["--chart-line", "--chart-fill-top", "--chart-fill-stack", "--chart-band", "--chart-point", "--chart-cursor", "--chart-select", "--chart-select-edge", "--spark-opacity", "--spark-width", "--spark-fill", "--row-line-opacity", "--meter-tint", "--topo-tint", "--topo-edge",
      "--row-line-width", "--row-fill", "--row-band", "--meter-neutral", "--chart-threshold", "--chart-threshold-text", "--chart-chip-edge", "--hatch", "--hatch-neutral", "--hatch-width", "--gap-fill", "--gap-hatch"];
    const blackOwn = parse(blackBody);
    for (const k of rules) {
      expect(light[k], k).toBeDefined();
      expect(parse(darkBody)[k], `${k} repeated in dark`).toBeUndefined();
      expect(blackOwn[k], k).toBeDefined();
    }
    // Light and Dark keep today's values; Black is where the data is the colour.
    expect(light["--meter-tint"]).toBe("0%");
    expect(blackOwn["--meter-tint"]).toBe("100%");
    expect(light["--spark-opacity"]).toBe("0.8");
    expect(blackOwn["--spark-opacity"]).toBe("1");
    expect(parseFloat(blackOwn["--chart-line"])).toBeGreaterThan(parseFloat(light["--chart-line"]));
    expect(parseFloat(blackOwn["--chart-fill-top"])).toBeGreaterThan(parseFloat(light["--chart-fill-top"]));
  });

  // On black a meter's used fraction has to be read from the bar itself (WCAG 1.4.11, 3:1 for the
  // parts of a graphic that carry its meaning): the track shows against the window and a tile,
  // and every fill a meter can wear shows against the track. Light and Dark keep their quieter
  // gray-5 track, which their lighter surfaces already separate. (The window surface is a lifted
  // #090b0f, not #000, so gray-6 is 1.8:1 on it; on the desk itself it is 1.9:1.)
  it.each([["black", black], ["black, increased contrast", { ...black, ...more }]] as const)("lets a meter be read from its bar (%s)", (_, t) => {
    expect(ratio(t, "--track", "--surface")).toBeGreaterThanOrEqual(1.75);
    expect(ratio(t, "--track", "--surface-2")).toBeGreaterThanOrEqual(1.55);
    // (--critical is 2.9:1 on it; a critical meter also turns its label red and bold with an icon.)
    for (const fill of ["--meter-neutral", "--m-mem", "--m-mem-gpu", "--m-mem-cache", "--m-write", "--m-gpu", "--warning"]) expect(ratio(t, fill, "--track"), fill).toBeGreaterThanOrEqual(3);
    // The uncrossed threshold line and the gridlines show on the plot.
    expect(ratio(t, "--chart-threshold", "--surface")).toBeGreaterThanOrEqual(4.5);
    expect(ratio(t, "--chart-grid", "--surface")).toBeGreaterThanOrEqual(1.75);
  });
});

// ---- Black: the data is the colour

/** Machado, Oliveira and Fernandes (2009) at severity 1, in linear light: the dataviz validator's model. */
const CVD = {
  deutan: [[0.367322, 0.860646, -0.227968], [0.280085, 0.672501, 0.047413], [-0.01182, 0.04294, 0.968881]],
  protan: [[0.152286, 1.052583, -0.204868], [0.114503, 0.786281, 0.099216], [-0.003882, -0.048116, 1.051998]],
};
const delin = (c: number) => 255 * (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);
function simulate(rgb: RGB, m: number[][]): RGB {
  const l = rgb.map(lin);
  return m.map((row) => delin(Math.max(0, Math.min(1, row[0] * l[0] + row[1] * l[1] + row[2] * l[2])))) as RGB;
}
/** OKLab distance, the validator's ΔE before its ×100. */
function deltaE(a: RGB, b: RGB): number {
  const lab = (rgb: RGB) => {
    const [L, C, h] = oklch(rgb);
    return [L, C * Math.cos((h * Math.PI) / 180), C * Math.sin((h * Math.PI) / 180)];
  };
  const [p, q] = [lab(a), lab(b)];
  return Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]);
}

describe("Black: the data is the colour", () => {
  const t = black;
  const c = (k: string) => resolve(t, `var(${k})`);

  it("draws every metric at least as bright as Dark, and more saturated", () => {
    // The family colours sit at the top of the validator's dark band in both themes, so equal
    // lightness (within rounding) counts; what Black adds is chroma. The nodes are not here: three
    // of them step darker on purpose, because lightness is what tells six hues apart under colour
    // blindness (node-4 lifted to L .68 fails the four-node every-pair check). The GPU green steps
    // down from Dark's (L .62 against .66) so that GPU power, boxed between it and --warning, can
    // be bright; it is still 5.8:1 on the plot against Dark's 4.1:1.
    const allowed: Record<string, number> = { "--m-gpu": 0.05 };
    for (const k of METRICS) {
      const [Lb, Cb] = oklch(c(k));
      const [Ld, Cd] = oklch(resolve(dark, `var(${k})`));
      expect(Lb, `${k} lightness`).toBeGreaterThanOrEqual(Ld - (allowed[k] ?? 0.02));
      expect(Cb, `${k} chroma`).toBeGreaterThanOrEqual(Cd);
    }
  });

  it("puts a stat tile on the window with a ring, not on a lifted step", () => {
    // One kind of panel: tiles and chart cards both sit on the window body, ringed by --border, so a row of
    // tiles over a row of charts is not lifted boxes over hollow ones. Light and Dark keep the lifted tile.
    expect(t["--tile-bg"]).toBe("var(--surface)");
    expect(t["--tile-edge"]).toBe("var(--border)");
    expect(light["--tile-bg"]).toBe("var(--surface-2)");
    expect(ratio(t, "--tile-edge", "--tile-bg")).toBeGreaterThanOrEqual(1.5);
  });
  it("draws a hollow neutral ring that reads on the window, a tile and a hovered or selected row", () => {
    // A 1.5 px gray-8 ring anti-aliases to about 2.8:1 on the window (the dataviz 3:1 floor for a mark);
    // Black's offline, not-set-up and "affected" rings are gray-10 at 2 px.
    for (const rule of [".dot.offline { border: 2px solid var(--gray-10); }", ".dot.unconfigured { border: 2px dashed var(--gray-10); }",
      ":is(.sdot-offline, .sdot-unconfigured) { color: var(--gray-10); }", ".sglyph-affected { color: var(--gray-10); }"]) {
      expect(css, rule).toContain(`:root[data-theme="black"] ${rule}`);
    }
    for (const bg of ["--surface", "--surface-2", "--hover", "--sel", "--sel-inactive"]) expect(ratio(t, "--gray-10", bg), bg).toBeGreaterThanOrEqual(3);
  });
  it("keeps every metric and node colour readable on the plot and on a tile", () => {
    // Lines are drawn on --surface (tiles too, in Black), sparklines in a zebra row or a chip on --surface-2: 3:1 is
    // the floor for a graphic (1.4.11); the metrics clear 4:1 on the plot. Meter fills against the track are checked above.
    for (const k of METRICS) {
      expect(ratio(t, k, "--surface"), `${k} on the plot`).toBeGreaterThanOrEqual(4);
      expect(ratio(t, k, "--surface-2"), `${k} on a tile`).toBeGreaterThanOrEqual(3.5);
    }
    for (const k of NODES) {
      expect(ratio(t, k, "--surface"), `${k} on the plot`).toBeGreaterThanOrEqual(3.3);
      expect(ratio(t, k, "--surface-2"), `${k} on a tile`).toBeGreaterThanOrEqual(3);
    }
  });

  it("tells apart, under deuteranopia and protanopia, every pair that shares a plot or a legend", () => {
    // Pairs drawn together: the Overview CPU+GPU chart, the memory stack, the fabric pairs, the
    // disk pair, the Fabric window's retransmit and congestion charts, the port speed legend
    // (neighbouring swatches), and a GPU strip against its offline hatch (Black's --hatch is
    // --crit-text: --critical at 70% sits at the green's lightness). 0.06 is the validator's floor
    // band; a legend, the gutter values or the "Offline" word is the secondary encoding on every one.
    // Not here: --m-mem and --m-temp (one colour to a deuteranope at 4.2) never share a plot or a
    // legend; they meet only as titled panels on Cluster metrics.
    const pairs: [string, string][] = [
      ["--m-cpu", "--m-gpu"], ["--m-mem-gpu", "--m-mem"], ["--m-mem", "--m-mem-cache"], ["--m-rx", "--m-tx"], ["--m-rdma", "--m-tcp"],
      ["--m-read", "--m-write"], ["--m-ttft", "--m-rx"], ["--m-ttft", "--m-tx"], ["--m-itl", "--m-rx"], ["--m-itl", "--m-tx"],
      ["--speed-10g", "--speed-5g"], ["--speed-5g", "--speed-2g5"], ["--speed-2g5", "--speed-1g"], ["--speed-1g", "--speed-100m"],
      ["--m-gpu", "--crit-text"],
    ];
    for (const [a, b] of pairs) for (const [name, m] of Object.entries(CVD)) {
      expect(deltaE(simulate(c(a), m), simulate(c(b), m)), `${a} vs ${b} (${name})`).toBeGreaterThanOrEqual(0.06);
    }
  });

  it("anchors the heat ramp one step above the window and climbs in lightness", () => {
    const steps = [0, 1, 2, 3, 4].map((i) => oklch(c(`--seq-${i}`)));
    // 0% recedes into the surface (the sequential rule), as Light's #e3eefc does on white.
    expect(ratio(t, "--seq-0", "--surface")).toBeGreaterThanOrEqual(1.1);
    expect(ratio(t, "--seq-0", "--surface")).toBeLessThanOrEqual(1.6);
    expect(ratio(t, "--seq-4", "--surface")).toBeGreaterThanOrEqual(12);
    for (let i = 1; i < steps.length; i++) expect(steps[i][0] - steps[i - 1][0], `seq-${i}`).toBeGreaterThanOrEqual(0.12);
    for (const [, C, h] of steps) if (C > 0.02) expect(Math.abs(h - 255)).toBeLessThan(14);
  });
});

describe("type floor (§D11): no text below 10.5 px at text scale 1, in any stylesheet", () => {
  const px = (v: string) => Number(/calc\(([\d.]+)px \* var\(--text-scale\)\)/.exec(v)?.[1] ?? NaN);
  const sizes = Object.fromEntries(Object.entries(light).filter(([k]) => /^--fs-/.test(k)).map(([k, v]) => [k, px(v)]));

  it("has every size token at 10.5 px or more", () => {
    expect(sizes["--fs-2xs"]).toBe(10.5);
    for (const [k, v] of Object.entries(sizes)) expect(v, k).toBeGreaterThanOrEqual(10.5);
  });

  it("uses only defined size tokens, and never scales one below the floor", () => {
    const bad: string[] = [];
    for (const [file, text] of Object.entries(sheets)) {
      const body = text.replace(/\/\*[\s\S]*?\*\//g, "");
      for (const m of body.matchAll(/font(?:-size)?\s*:\s*([^;}]+)/g)) {
        for (const v of m[1].matchAll(/var\((--fs-[\w-]+)/g)) if (!(v[1] in sizes)) bad.push(`${file}: unknown ${v[1]}`);
        for (const c of m[1].matchAll(/calc\(\s*var\((--fs-[\w-]+)[^)]*\)\s*\*\s*([\d.]+)\s*\)/g)) {
          const size = (sizes[c[1]] ?? 0) * Number(c[2]);
          if (size < 10.5) bad.push(`${file}: ${c[0]} is ${size.toFixed(2)} px`);
        }
      }
    }
    expect(bad).toEqual([]);
  });
});
