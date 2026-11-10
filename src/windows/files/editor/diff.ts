// Line diffs for "Review changes" (before a sudo save) and "Show diff" (after a conflict):
// Myers' algorithm on the lines between the common start and end, grouped into unified
// hunks with three lines of context.

export type DiffLine = { kind: "same" | "add" | "del"; text: string; a: number | null; b: number | null };

export interface Hunk {
  aStart: number;
  aLen: number;
  bStart: number;
  bLen: number;
  lines: DiffLine[];
}

/** Past this many differing lines the diff gives up on minimality and shows a replace. */
const MAX_EDITS = 4000;

function split(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** The edit script turning a into b (Myers, O((N+M)·D)). */
function myers(a: string[], b: string[]): ("same" | "add" | "del")[] {
  const n = a.length;
  const m = b.length;
  const max = n + m;
  const offset = max + 1;
  const v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  for (let d = 0; d <= max; d++) {
    trace.push(v.slice());
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[offset + k - 1] < v[offset + k + 1]) ? v[offset + k + 1] : v[offset + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x++;
        y++;
      }
      v[offset + k] = x;
      if (x >= n && y >= m) {
        // Walk back through the saved frontiers.
        const ops: ("same" | "add" | "del")[] = [];
        let cx = n;
        let cy = m;
        for (let dd = d; dd > 0; dd--) {
          const pv = trace[dd];
          const ck = cx - cy;
          const prevK = ck === -dd || (ck !== dd && pv[offset + ck - 1] < pv[offset + ck + 1]) ? ck + 1 : ck - 1;
          const px = pv[offset + prevK];
          const py = px - prevK;
          while (cx > px && cy > py) {
            ops.push("same");
            cx--;
            cy--;
          }
          ops.push(cx === px ? "add" : "del");
          cx = px;
          cy = py;
        }
        while (cx > 0 && cy > 0) {
          ops.push("same");
          cx--;
          cy--;
        }
        return ops.reverse();
      }
    }
    if (d > MAX_EDITS) break;
  }
  return [...a.map(() => "del" as const), ...b.map(() => "add" as const)];
}

export function diffLines(before: string, after: string): DiffLine[] {
  const a = split(before);
  const b = split(after);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let end = 0;
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++;
  const ops = myers(a.slice(start, a.length - end), b.slice(start, b.length - end));
  const out: DiffLine[] = [];
  for (let i = 0; i < start; i++) out.push({ kind: "same", text: a[i], a: i + 1, b: i + 1 });
  let ai = start;
  let bi = start;
  for (const op of ops) {
    if (op === "same") out.push({ kind: "same", text: a[ai], a: ++ai, b: ++bi });
    else if (op === "del") out.push({ kind: "del", text: a[ai], a: ++ai, b: null });
    else out.push({ kind: "add", text: b[bi], a: null, b: ++bi });
  }
  for (let i = 0; i < end; i++) out.push({ kind: "same", text: a[ai], a: ++ai, b: ++bi });
  return out;
}

export function hunks(lines: DiffLine[], context = 3): Hunk[] {
  const out: Hunk[] = [];
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind === "same") {
      i++;
      continue;
    }
    const from = Math.max(0, i - context);
    let to = i;
    // Extend over changes separated by less than two contexts' worth of unchanged lines.
    for (let j = i; j < lines.length; j++) {
      if (lines[j].kind !== "same") to = j;
      else if (j - to > context * 2) break;
    }
    const until = Math.min(lines.length, to + context + 1);
    const slice = lines.slice(from, until);
    const firstA = slice.find((l) => l.a !== null)?.a ?? 0;
    const firstB = slice.find((l) => l.b !== null)?.b ?? 0;
    out.push({
      aStart: firstA,
      aLen: slice.filter((l) => l.kind !== "add").length,
      bStart: firstB,
      bLen: slice.filter((l) => l.kind !== "del").length,
      lines: slice,
    });
    i = until;
  }
  return out;
}

/** `diff -u` text, e.g. for Copy. */
export function unified(before: string, after: string, aName: string, bName: string): string {
  const hs = hunks(diffLines(before, after));
  if (!hs.length) return "";
  let s = `--- ${aName}\n+++ ${bName}\n`;
  for (const h of hs) {
    s += `@@ -${h.aStart},${h.aLen} +${h.bStart},${h.bLen} @@\n`;
    for (const l of h.lines) s += `${l.kind === "add" ? "+" : l.kind === "del" ? "-" : " "}${l.text}\n`;
  }
  return s;
}

export function diffStats(lines: DiffLine[]): { added: number; removed: number } {
  return { added: lines.filter((l) => l.kind === "add").length, removed: lines.filter((l) => l.kind === "del").length };
}
