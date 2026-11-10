// C4: windows never leave the desk. Every registered kind, opened in any order (with and
// without a saved frame, the saved one anywhere, even off the desk), lands wholly on a
// 1220×834 or 1440×900 desk with 12 px to spare on the right and below.
import { describe, expect, it } from "vitest";
import { minSizeOf, openableKinds, sizeOf } from "../windows/registry";
import { cascadeFree, fitInside, ORIGIN, openFrame, type Rect } from "./geometry";

const DESKS = [{ w: 1220, h: 834 }, { w: 1440, h: 900 }];
const rng = (seed: number) => () => ((seed = (seed * 16807) % 2147483647) / 2147483647);

function onDesk(r: Rect, desk: { w: number; h: number }) {
  return r.x >= 0 && r.y >= 0 && r.x + r.w <= desk.w - ORIGIN && r.y + r.h <= desk.h - ORIGIN;
}

describe("windows on the desk (C4)", () => {
  const kinds = openableKinds();

  it("knows the registered kinds", () => {
    expect(kinds.length).toBeGreaterThan(20);
    expect(kinds).toEqual(expect.arrayContaining(["dashboard", "workloads", "containers", "models", "services"]));
  });

  for (const desk of DESKS) {
    it(`opens any 8 kinds in a row inside a ${desk.w}×${desk.h} desk, saved frames or not`, () => {
      const rand = rng(desk.w);
      for (let c = 0; c < 400; c++) {
        const n = 1 + Math.floor(rand() * 8);
        const taken: Rect[] = [];
        for (let i = 0; i < n; i++) {
          const kind = kinds[Math.floor(rand() * kinds.length)];
          const min = minSizeOf(kind);
          // Half the time a saved frame: anywhere, of any size, sometimes past the desk's edge.
          const saved = rand() < 0.5
            ? { x: Math.round(rand() * desk.w * 1.2) - 40, y: Math.round(rand() * desk.h * 1.2) - 20, w: Math.round(200 + rand() * 1400), h: Math.round(150 + rand() * 900) }
            : null;
          const r = openFrame(sizeOf(kind), min, saved, taken.length, taken, desk);
          const label = `case ${c}: ${kind} #${i} ${JSON.stringify(r)} saved ${JSON.stringify(saved)}`;
          if (min[0] <= desk.w - 2 * ORIGIN && min[1] <= desk.h - 2 * ORIGIN) {
            expect(onDesk(r, desk), label).toBe(true);
            expect(r.w, label).toBeGreaterThanOrEqual(min[0]);
            expect(r.h, label).toBeGreaterThanOrEqual(min[1]);
          }
          taken.push(r);
        }
      }
    });
  }

  it("the six windows of the Live tour open on the desk, the later ones narrower rather than past the edge", () => {
    const desk = DESKS[0];
    const taken: Rect[] = [];
    for (const kind of ["dashboard", "topology", "workloads", "containers", "models", "services"]) {
      const r = openFrame(sizeOf(kind), minSizeOf(kind), null, taken.length, taken, desk);
      expect(onDesk(r, desk), `${kind} ${JSON.stringify(r)}`).toBe(true);
      // Never on another window's corner.
      expect(taken.some((t) => Math.abs(t.x - r.x) <= 8 && Math.abs(t.y - r.y) <= 8), kind).toBe(false);
      taken.push(r);
    }
  });

  it("brings a window back onto a desk that shrank, at no less than its minimum", () => {
    const wide = { x: 680, y: 40, w: 600, h: 500 };
    const r = fitInside(wide, { w: 800, h: 600 }, [360, 200]);
    expect(onDesk(r, { w: 800, h: 600 })).toBe(true);
    expect(r.w).toBe(600);
    expect(cascadeFree(r, [{ x: r.x, y: r.y }], { w: 800, h: 600 }, [360, 200]).x + 360).toBeLessThanOrEqual(800 - ORIGIN);
  });
});
