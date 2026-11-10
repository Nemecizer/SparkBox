#!/usr/bin/env node
// Saturation audit for the colour policy (plan §2.1): how much of a screenshot is
// saturated colour (OKLCH chroma above 0.1), by hue, outside the chart regions you
// pass in. Healthy should be quiet: in the Demo baseline only the two warnings and,
// when unlocked, the Admin pill should show up here.
//
//   node scripts/saturation-audit.mjs shot.png [--rects rects.json] [--exclude x,y,w,h]...
//        [--css-width 1440] [--offset-y 0] [--chroma 0.1] [--json]
//
// Rects (from --rects, a JSON array of {x, y, width, height}, or --exclude) are in CSS px
// of the page; --css-width maps them onto the image (scale = image width / css width) and
// --offset-y adds the window's title bar above the page. No dependencies: the PNG is
// decoded here (8-bit RGB or RGBA, not interlaced, which is what screencapture writes).
import { readFileSync } from "node:fs";
import { inflateSync } from "node:zlib";

function decodePng(buf) {
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error("not a PNG");
  let pos = 8;
  let width = 0, height = 0, depth = 0, type = 0, interlace = 0;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const kind = buf.toString("ascii", pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (kind === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      depth = data[8];
      type = data[9];
      interlace = data[12];
    } else if (kind === "IDAT") idat.push(data);
    else if (kind === "IEND") break;
    pos += 12 + len;
  }
  if (depth !== 8 || (type !== 2 && type !== 6) || interlace) throw new Error(`unsupported PNG (depth ${depth}, type ${type}, interlace ${interlace})`);
  const bpp = type === 6 ? 4 : 3;
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * bpp;
  const px = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y++) {
    const f = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? px[y * stride + x - bpp] : 0;
      const b = y > 0 ? px[(y - 1) * stride + x] : 0;
      const c = x >= bpp && y > 0 ? px[(y - 1) * stride + x - bpp] : 0;
      let v = line[x];
      if (f === 1) v += a;
      else if (f === 2) v += b;
      else if (f === 3) v += (a + b) >> 1;
      else if (f === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      px[y * stride + x] = v & 255;
    }
  }
  return { width, height, bpp, px };
}

const lin = (c) => {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
};

function oklch(r, g, b) {
  const [R, G, B] = [lin(r), lin(g), lin(b)];
  const l = Math.cbrt(0.4122214708 * R + 0.5363325363 * G + 0.0514459929 * B);
  const m = Math.cbrt(0.2119034982 * R + 0.6806995451 * G + 0.1073969566 * B);
  const s = Math.cbrt(0.0883024619 * R + 0.2817188376 * G + 0.6299787005 * B);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const Bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [L, Math.hypot(A, Bb), ((Math.atan2(Bb, A) * 180) / Math.PI + 360) % 360];
}

/** Hue buckets in OKLCH degrees, named the way a person would. */
const BUCKETS = [
  ["red", 0, 40], ["orange", 40, 65], ["amber", 65, 100], ["olive", 100, 130], ["green", 130, 170],
  ["teal", 170, 205], ["cyan", 205, 235], ["blue", 235, 270], ["violet", 270, 310], ["magenta", 310, 345], ["red", 345, 360],
];
const bucketOf = (h) => BUCKETS.find(([, a, b]) => h >= a && h < b)[0];

function parseArgs(argv) {
  const o = { file: null, rects: [], cssWidth: null, offsetY: 0, chroma: 0.1, json: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--rects") o.rects.push(...JSON.parse(readFileSync(argv[++i], "utf8")));
    else if (a === "--exclude") {
      const [x, y, width, height] = argv[++i].split(",").map(Number);
      o.rects.push({ x, y, width, height });
    } else if (a === "--css-width") o.cssWidth = Number(argv[++i]);
    else if (a === "--offset-y") o.offsetY = Number(argv[++i]);
    else if (a === "--chroma") o.chroma = Number(argv[++i]);
    else if (a === "--json") o.json = true;
    else o.file = a;
  }
  if (!o.file) {
    console.error("usage: saturation-audit.mjs shot.png [--rects rects.json] [--exclude x,y,w,h]... [--css-width 1440] [--offset-y 0] [--chroma 0.1] [--json]");
    process.exit(2);
  }
  return o;
}

const o = parseArgs(process.argv.slice(2));
const img = decodePng(readFileSync(o.file));
const scale = o.cssWidth ? img.width / o.cssWidth : 1;
const boxes = o.rects.map((r) => [
  Math.floor(r.x * scale), Math.floor((r.y + o.offsetY) * scale),
  Math.ceil((r.x + r.width) * scale), Math.ceil((r.y + r.height + o.offsetY) * scale),
]);
const excluded = (x, y) => boxes.some(([x0, y0, x1, y1]) => x >= x0 && x < x1 && y >= y0 && y < y1);

const CELL = Math.max(8, Math.round(24 * scale));
const counts = {};
const cells = new Map();
let considered = 0;
let saturated = 0;
for (let y = 0; y < img.height; y++) {
  for (let x = 0; x < img.width; x++) {
    if (excluded(x, y)) continue;
    considered++;
    const i = (y * img.width + x) * img.bpp;
    if (img.bpp === 4 && img.px[i + 3] < 128) continue;
    const [, C, h] = oklch(img.px[i], img.px[i + 1], img.px[i + 2]);
    if (C <= o.chroma) continue;
    saturated++;
    const b = bucketOf(h);
    counts[b] = (counts[b] ?? 0) + 1;
    const key = `${Math.floor(x / CELL)},${Math.floor(y / CELL)}`;
    const cell = cells.get(key) ?? { n: 0, hues: {} };
    cell.n++;
    cell.hues[b] = (cell.hues[b] ?? 0) + 1;
    cells.set(key, cell);
  }
}

// Saturated cells joined into regions (8-connected), so the report says where, not just how much.
const seen = new Set();
const regions = [];
for (const key of cells.keys()) {
  if (seen.has(key) || cells.get(key).n < 4) continue;
  const stack = [key];
  const region = { n: 0, hues: {}, x0: Infinity, y0: Infinity, x1: -Infinity, y1: -Infinity };
  seen.add(key);
  while (stack.length) {
    const k = stack.pop();
    const [cx, cy] = k.split(",").map(Number);
    const c = cells.get(k);
    region.n += c.n;
    for (const [h, n] of Object.entries(c.hues)) region.hues[h] = (region.hues[h] ?? 0) + n;
    region.x0 = Math.min(region.x0, cx); region.y0 = Math.min(region.y0, cy);
    region.x1 = Math.max(region.x1, cx + 1); region.y1 = Math.max(region.y1, cy + 1);
    for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++) {
      const nk = `${cx + dx},${cy + dy}`;
      if (!seen.has(nk) && cells.has(nk) && cells.get(nk).n >= 4) {
        seen.add(nk);
        stack.push(nk);
      }
    }
  }
  regions.push(region);
}
regions.sort((a, b) => b.n - a.n);
const toCss = (v) => Math.round((v * CELL) / scale);
const report = {
  file: o.file,
  image: `${img.width}×${img.height}`,
  excludedRects: o.rects.length,
  consideredPx: considered,
  saturatedPx: saturated,
  saturatedPct: Number(((saturated / Math.max(1, considered)) * 100).toFixed(3)),
  byHue: Object.fromEntries(Object.entries(counts).sort((a, b) => b[1] - a[1])),
  regions: regions.slice(0, 12).map((r) => ({
    px: r.n,
    hue: Object.entries(r.hues).sort((a, b) => b[1] - a[1])[0][0],
    css: { x: toCss(r.x0), y: toCss(r.y0) - o.offsetY, width: toCss(r.x1 - r.x0), height: toCss(r.y1 - r.y0) },
  })),
};

if (o.json) console.log(JSON.stringify(report, null, 1));
else {
  console.log(`${report.file}: ${report.image}, ${report.excludedRects} chart rects excluded`);
  console.log(`saturated (OKLCH C > ${o.chroma}): ${saturated} of ${considered} px (${report.saturatedPct}%)`);
  for (const [h, n] of Object.entries(report.byHue)) console.log(`  ${h.padEnd(8)} ${String(n).padStart(8)} px`);
  console.log("largest saturated regions (CSS px):");
  for (const r of report.regions) console.log(`  ${r.hue.padEnd(8)} ${String(r.px).padStart(7)} px  at ${r.css.x},${r.css.y} ${r.css.width}×${r.css.height}`);
}
