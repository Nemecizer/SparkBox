#!/usr/bin/env node
// Draws the diagrams in Help: docs/help/img/<id>.svg, and <id>-dark.svg with the dark colors.
// Each one is an entry in docs/help/shots.json with "source": "diagram", which holds its caption.
//
//   node tools/agents/docs-diagrams.mjs
//
// A diagram is 1160 units wide, twice the width Help shows it at, so 26 units of text reads as 13
// points. Nothing here measures text: after a change, look at both themes in Help. The rules for
// what a diagram says are in docs/help/STYLE.md, section 8.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "../../docs/help/img");

/** The app's own tokens (src/styles.css), so a diagram sits in the page like the rest of it. */
const LIGHT = { surface: "#fbfeff", raised: "#edf0f4", line: "#b9bec4", lan: "#646970", fabric: "#646970", text: "#13161b", dim: "#41454c", node1: "#2c74cd", node2: "#18a07e" };
const DARK = { surface: "#25282c", raised: "#2d3135", line: "#5c6167", lan: "#9ba0a7", fabric: "#9ba0a7", text: "#eef0f3", dim: "#c8ccd2", node1: "#468ae2", node2: "#32b08d" };

const FONT = `-apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui, sans-serif`;
const MONO = `ui-monospace, 'SF Mono', Menlo, monospace`;

const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** The drawing tools, bound to one theme. */
function pen(c) {
  const text = (x, y, words, { size = 24, weight = 400, anchor = "start", fill = c.text, mono = false } = {}) =>
    `<text x="${x}" y="${y}" font-family="${mono ? MONO : FONT}" font-size="${size}" font-weight="${weight}" text-anchor="${anchor}" fill="${fill}">${esc(words)}</text>`;
  const box = (x, y, w, h, { fill = c.surface, stroke = c.line, width = 2, rx = 10, dash, opacity } = {}) =>
    `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rx}" fill="${fill}"${opacity === undefined ? "" : ` fill-opacity="${opacity}"`} stroke="${stroke}" stroke-width="${width}"${dash ? ` stroke-dasharray="${dash}"` : ""}/>`;
  const line = (x1, y1, x2, y2, { color = c.lan, width = 3, dash } = {}) =>
    `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" stroke="${color}" stroke-width="${width}" stroke-linecap="round"${dash ? ` stroke-dasharray="${dash}"` : ""}/>`;
  const head = (x, y, dx, dy, color, size = 13) => {
    const l = Math.hypot(dx, dy) || 1;
    const [ux, uy] = [dx / l, dy / l];
    const [bx, by] = [x - ux * size * 1.5, y - uy * size * 1.5];
    return `<polygon points="${x},${y} ${bx - uy * size * 0.75},${by + ux * size * 0.75} ${bx + uy * size * 0.75},${by - ux * size * 0.75}" fill="${color}"/>`;
  };
  const arrow = (x1, y1, x2, y2, { color = c.text, width = 3, dash, both, size = 13 } = {}) => {
    const l = Math.hypot(x2 - x1, y2 - y1);
    const [ux, uy] = [(x2 - x1) / l, (y2 - y1) / l];
    const inset = size * 1.1;
    const a = both ? [x1 + ux * inset, y1 + uy * inset] : [x1, y1];
    return line(a[0], a[1], x2 - ux * inset, y2 - uy * inset, { color, width, dash }) + head(x2, y2, ux, uy, color, size) + (both ? head(x1, y1, -ux, -uy, color, size) : "");
  };
  const badge = (x, y, n) => `<circle cx="${x}" cy="${y}" r="20" fill="${c.text}"/>` + text(x, y + 8, n, { size: 24, weight: 600, anchor: "middle", fill: c.surface });
  return { text, box, line, arrow, badge };
}

const svg = (c, width, height, body) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">\n<rect width="${width}" height="${height}" fill="${c.surface}"/>\n${body.flat(Infinity).join("\n")}\n</svg>\n`;

/** Tensor parallel against pipeline parallel, on two nodes and six layers. */
function tensorPipeline(c) {
  const { text, box, line, arrow } = pen(c);
  const ROW = 50, GAP = 8, TOP = 132;
  const rowY = (i) => TOP + i * (ROW + GAP);
  const layers = [1, 2, 3, 4, 5, 6];
  const out = [];
  const panel = (x, title, sub, notes, cell) => {
    const col1 = x + 128, col2 = x + 356, W = 166;
    out.push(box(x, 20, 550, 566, { fill: c.raised, stroke: c.raised }));
    out.push(text(x + 28, 66, title, { size: 28, weight: 600 }), text(x + 522, 66, sub, { size: 24, anchor: "end", fill: c.dim }));
    out.push(text(col1 + W / 2, 112, "cell1", { size: 24, anchor: "middle", mono: true }), text(col2 + W / 2, 112, "cell2", { size: 24, anchor: "middle", mono: true }));
    for (const n of layers) {
      const y = rowY(n - 1);
      out.push(text(x + 28, y + 33, `Layer ${n}`, { size: 24, fill: c.dim }));
      out.push(cell(n, y, col1, col2, W));
    }
    notes.forEach((note, i) => out.push(text(x + 28, 512 + 34 * i, note, { size: 24 })));
  };
  const slice = (x, y, w, color) => box(x, y, w, ROW, { fill: color, stroke: color, opacity: 0.22, rx: 6 });
  const empty = (x, y, w) => box(x, y, w, ROW, { fill: "none", stroke: c.line, width: 1.5, rx: 6, dash: "6 6" });
  panel(20, "Tensor parallel", "2 × 1",
    ["Every node holds a slice of every layer.", "The nodes talk many times for every token."],
    (n, y, a, b, w) => [slice(a, y, w, c.node1), slice(b, y, w, c.node2), arrow(a + w + 6, y + ROW / 2, b - 6, y + ROW / 2, { color: c.fabric, both: true, size: 10 })]);
  panel(590, "Pipeline parallel", "1 × 2",
    ["Each node holds its own run of layers.", "A token passes along, once per stage."],
    (n, y, a, b, w) => [n <= 3 ? slice(a, y, w, c.node1) : empty(a, y, w), n > 3 ? slice(b, y, w, c.node2) : empty(b, y, w)]);
  // The one hand-over: from the last layer on cell1 across to the first on cell2.
  const [fromX, fromY] = [590 + 128 + 166, rowY(2) + ROW / 2];
  const toX = 590 + 356 + 83;
  out.push(line(fromX, fromY, toX, fromY, { color: c.fabric }), arrow(toX, fromY, toX, rowY(3), { color: c.fabric }));
  return svg(c, 1160, 606, out);
}

/** How a request reaches a model: straight to its port, or through the Server on this Mac. */
function appToHeadNode(c) {
  const { text, box, arrow, line, badge } = pen(c);
  const out = [];
  // This Mac
  out.push(box(20, 20, 400, 440, { fill: c.raised, stroke: c.raised }), text(44, 62, "This Mac", { size: 28, weight: 600 }));
  out.push(box(50, 92, 340, 88), text(220, 146, "An app", { size: 26, weight: 600, anchor: "middle" }));
  out.push(box(50, 300, 340, 128), text(220, 346, "The Server", { size: 26, weight: 600, anchor: "middle" }), text(220, 382, "Port", { size: 24, anchor: "middle", fill: c.dim }), text(220, 412, "8484", { size: 24, anchor: "middle", mono: true }));
  out.push(arrow(150, 180, 150, 300), badge(150, 240, "2"));
  // The LAN
  out.push(text(530, 62, "The LAN", { size: 26, weight: 600, anchor: "middle" }));
  // cell1, the head node
  out.push(box(640, 20, 500, 440 - 110, { fill: c.raised, stroke: c.raised }));
  out.push(text(664, 62, "cell1", { size: 28, weight: 600, mono: true }), text(1116, 62, "head node", { size: 24, anchor: "end", fill: c.dim }));
  out.push(box(664, 92, 130, 228, { fill: c.surface, stroke: c.node1 }), text(729, 196, "Port", { size: 24, anchor: "middle", fill: c.dim }), text(729, 228, "8000", { size: 24, anchor: "middle", mono: true }));
  out.push(box(844, 92, 272, 94, { fill: c.surface }), text(980, 130, "Engine", { size: 26, weight: 600, anchor: "middle" }), text(980, 164, "such as vLLM", { size: 24, anchor: "middle", fill: c.dim }));
  out.push(box(844, 226, 272, 94, { fill: c.node1, stroke: c.node1, opacity: 0.22 }), text(980, 282, "Model", { size: 26, weight: 600, anchor: "middle" }));
  out.push(arrow(794, 139, 844, 139), arrow(980, 186, 980, 226));
  // The two ways in
  out.push(arrow(390, 136, 664, 136, { dash: "12 10" }), badge(530, 136, "1"));
  out.push(arrow(390, 364, 664, 276, { dash: "12 10" }), badge(530, 319, "2"));
  // A worker, over the fabric
  out.push(box(640, 396, 500, 64, { fill: c.raised, stroke: c.raised }));
  out.push(text(664, 438, "cell2", { size: 26, weight: 600, mono: true }), text(1116, 438, "worker, on several nodes", { size: 24, anchor: "end", fill: c.dim }));
  out.push(line(980, 350, 980, 396, { color: c.fabric, width: 8 }), text(1000, 382, "The fabric", { size: 24, fill: c.dim }));
  // What each number means
  out.push(badge(44, 510, "1"), text(78, 519, "Straight to the endpoint:", { size: 24 }), text(376, 519, "http://192.168.50.51:8000/v1", { size: 24, mono: true }));
  out.push(badge(44, 566, "2"), text(78, 575, "Through the Server:", { size: 24 }), text(376, 575, "http://127.0.0.1:8484", { size: 24, mono: true }));
  return svg(c, 1160, 600, out);
}

/** The two networks: the LAN above the nodes, the fabric below them. */
function lanAndFabric(c) {
  const { text, box, line } = pen(c);
  const LAN = "12 10";
  const out = [];
  const device = (x, y, w, name, role, opts) => [box(x, y, w, 88, opts), text(x + w / 2, y + 38, name, { size: 26, weight: 600, anchor: "middle" }), text(x + w / 2, y + 70, role, { size: 24, anchor: "middle", fill: c.dim })];
  out.push(line(330, 64, 445, 64, { dash: LAN }), line(715, 64, 830, 64, { dash: LAN }));
  out.push(device(60, 20, 270, "Firewalla Gold", "gateway"), device(445, 20, 270, "Firewalla Switch X", "LAN switch"), device(830, 20, 270, "This Mac", "SparkBox"));
  const nodes = [["cell1", "192.168.50.51"], ["cell2", "192.168.50.154"], ["cell3", "192.168.50.98"], ["gx10-0000", "192.168.50.181"]];
  const W = 245, GAP = 40, Y = 216, H = 92;
  nodes.forEach(([name], i) => {
    const mid = 30 + i * (W + GAP) + W / 2;
    const up = 580 + (i - 1.5) * 60;
    const down = 580 + (i - 1.5) * 190;
    out.push(line(up, 108, mid, Y, { dash: LAN }));
    out.push(line(mid, Y + H, down, 412, { color: c.fabric, width: 9 }));
  });
  nodes.forEach(([name, address], i) => {
    const x = 30 + i * (W + GAP);
    out.push(box(x, Y, W, H), text(x + W / 2, Y + 40, name, { size: 26, weight: 600, anchor: "middle", mono: true }), text(x + W / 2, Y + 72, address, { size: 22, anchor: "middle", fill: c.dim, mono: true }));
  });
  out.push(device(190, 412, 780, "CRS804 DDQ", "fabric switch"));
  // The key
  out.push(line(44, 548, 124, 548, { dash: LAN }), text(146, 557, "The LAN: how this Mac and your apps reach the nodes.", { size: 24 }));
  out.push(line(44, 600, 124, 600, { color: c.fabric, width: 9 }), text(146, 609, "The fabric: 200G between nodes. This Mac isn't on it.", { size: 24 }));
  return svg(c, 1160, 634, out);
}

const DIAGRAMS = { "diagram-tensor-pipeline": tensorPipeline, "diagram-app-to-head-node": appToHeadNode, "diagram-lan-and-fabric": lanAndFabric };

mkdirSync(OUT, { recursive: true });
for (const [id, draw] of Object.entries(DIAGRAMS)) {
  writeFileSync(join(OUT, `${id}.svg`), draw(LIGHT));
  writeFileSync(join(OUT, `${id}-dark.svg`), draw(DARK));
  console.log(join(OUT, `${id}.svg`));
}
