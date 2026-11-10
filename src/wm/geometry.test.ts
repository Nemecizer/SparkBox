import { describe, expect, it } from "vitest";
import { REGISTRY, sizeOf, tileMinOf } from "../windows/registry";
import {
  CASCADE_NEAR, CASCADE_STEP, cascadeFree, cascadeLayout, clampRect, DOCK_H, fitInside, fitRect, GRAB_W, normalizeZ, placeNew, shareOut, snapZoneAt, TITLE_H,
  tileCells, tileLayout, zoneRect, type Arrangeable, type Rect, type Size, type TileWin,
} from "./geometry";

const desk = { w: 1200, h: 800 };
const win = (id: string, size: [number, number] = [600, 400], min: [number, number] = [360, 200]): Arrangeable => ({ id, size, min });
const inside = (r: Rect) => r.x >= 0 && r.y >= 0 && r.x + r.w <= desk.w + 0.5 && r.y + r.h <= desk.h + 0.5;

describe("clampRect", () => {
  it("keeps 120 px of the title bar on the desktop horizontally", () => {
    expect(clampRect({ x: -900, y: 10, w: 600, h: 400 }, desk).x).toBe(GRAB_W - 600);
    expect(clampRect({ x: 5000, y: 10, w: 600, h: 400 }, desk).x).toBe(desk.w - GRAB_W);
  });

  it("keeps the whole title bar on the desktop vertically", () => {
    expect(clampRect({ x: 10, y: -50, w: 600, h: 400 }, desk).y).toBe(0);
    expect(clampRect({ x: 10, y: 5000, w: 600, h: 400 }, desk).y).toBe(desk.h - TITLE_H);
  });

  it("leaves a reachable window alone", () => {
    expect(clampRect({ x: 40, y: 60, w: 600, h: 400 }, desk)).toEqual({ x: 40, y: 60, w: 600, h: 400 });
  });
});

describe("fitRect", () => {
  it("shrinks a window bigger than the desktop, but not below its minimum", () => {
    expect(fitRect({ x: 0, y: 0, w: 2000, h: 1400 }, desk, [360, 200])).toEqual({ x: 0, y: 0, w: 1200, h: 800 });
    expect(fitRect({ x: 0, y: 0, w: 2000, h: 1400 }, { w: 300, h: 150 }, [360, 200])).toMatchObject({ w: 360, h: 200 });
  });

  it("brings a window from a bigger screen back into reach", () => {
    const r = fitRect({ x: 2400, y: 1500, w: 800, h: 500 }, desk, [360, 200]);
    expect(r.x).toBeLessThanOrEqual(desk.w - GRAB_W);
    expect(r.y).toBeLessThanOrEqual(desk.h - TITLE_H);
  });
});

describe("snapping", () => {
  it("offers halves at the side edges, quarters at the corners and maximize at the top", () => {
    expect(snapZoneAt(3, 400, desk)).toBe("left");
    expect(snapZoneAt(1195, 400, desk)).toBe("right");
    expect(snapZoneAt(0, 10, desk)).toBe("top-left");
    expect(snapZoneAt(1200, 790, desk)).toBe("bottom-right");
    expect(snapZoneAt(600, 2, desk)).toBe("max");
    expect(snapZoneAt(600, -20, desk)).toBe("max");
    expect(snapZoneAt(600, 400, desk)).toBeNull();
    expect(snapZoneAt(20, 400, desk)).toBeNull();
  });

  it("fills exactly the half or quarter", () => {
    expect(zoneRect("left", desk)).toEqual({ x: 0, y: 0, w: 600, h: 800 });
    expect(zoneRect("right", desk)).toEqual({ x: 600, y: 0, w: 600, h: 800 });
    expect(zoneRect("bottom-left", desk)).toEqual({ x: 0, y: 400, w: 600, h: 400 });
    expect(zoneRect("max", desk)).toEqual({ x: 0, y: 0, w: 1200, h: 800 });
    const odd = { w: 1001, h: 701 };
    expect(zoneRect("left", odd).w + zoneRect("right", odd).w).toBe(1001);
  });
});

describe("cascade", () => {
  it("steps windows down and right, sized to at most 75% of the desktop", () => {
    const out = cascadeLayout([win("a", [2000, 2000]), win("b"), win("c")], desk);
    expect(out.get("a")).toMatchObject({ x: 12, y: 12, w: 900, h: 600 });
    expect(out.get("b")!.x - out.get("a")!.x).toBe(26);
    expect(out.get("c")!.y - out.get("b")!.y).toBe(26);
  });

  it("keeps the right margin equal to the left by narrowing the window", () => {
    const tall = { w: 1000, h: 1400 };
    const out = cascadeLayout(Array.from({ length: 12 }, (_, i) => win(`w${i}`, [880, 500])), tall);
    for (const r of out.values()) expect(tall.w - (r.x + r.w)).toBeGreaterThanOrEqual(12);
    // The first fits at its own size (75% of the desktop); later ones are narrowed to keep the margin.
    expect(out.get("w0")).toMatchObject({ x: 12, w: 750 });
    const last = out.get("w11")!;
    expect(last.x).toBe(12 + 11 * 26);
    expect(tall.w - (last.x + last.w)).toBe(12);
  });

  it("wraps back to the top before a window would overflow", () => {
    const many = Array.from({ length: 20 }, (_, i) => win(`w${i}`, [700, 500]));
    const out = cascadeLayout(many, desk);
    for (const r of out.values()) expect(r.x + r.w <= desk.w && r.y + r.h <= desk.h).toBe(true);
    expect(new Set([...out.values()].map((r) => r.y)).has(12)).toBe(true);
    expect([...out.values()].filter((r) => r.y === 12).length).toBeGreaterThan(1);
  });
});

describe("overview cells", () => {
  it("puts two side by side", () => {
    const c = tileCells(2, desk);
    expect(c).toHaveLength(2);
    expect(c[0].y).toBe(c[1].y);
    expect(c[0].h).toBeGreaterThan(780);
  });

  it("puts three as one tall and two stacked", () => {
    const [a, b, c] = tileCells(3, desk);
    expect(a.h).toBeGreaterThan(780);
    expect(b.x).toBe(c.x);
    expect(c.y).toBeGreaterThan(b.y);
    expect(b.h).toBeLessThan(400);
  });

  it("uses 2×2 for four and 3×2 for five or six", () => {
    expect(new Set(tileCells(4, desk).map((r) => r.y)).size).toBe(2);
    expect(new Set(tileCells(4, desk).map((r) => r.x)).size).toBe(2);
    const six = tileCells(6, desk);
    expect(new Set(six.map((r) => r.x)).size).toBe(3);
    expect(new Set(six.map((r) => r.y)).size).toBe(2);
    // Five: the short last row shares the full width.
    const five = tileCells(5, desk);
    expect(five[3].w).toBeGreaterThan(five[0].w);
  });

  it("uses a grid for seven or more, every cell on the desktop", () => {
    for (const n of [7, 9, 12]) {
      const cells = tileCells(n, desk);
      expect(cells).toHaveLength(n);
      for (const c of cells) expect(inside(c)).toBe(true);
    }
  });
});

/** Round 2's default sizes and tile minimums: bigger than today's registry, so harder to tile. */
const KINDS: Record<string, { size: [number, number]; min: [number, number] }> = {
  dashboard: { size: [1180, 820], min: [720, 480] },
  topology: { size: [980, 560], min: [560, 380] },
  containers: { size: [1160, 440], min: [760, 300] },
  services: { size: [1180, 680], min: [640, 360] },
  activity: { size: [1100, 560], min: [640, 320] },
  workloads: { size: [1240, 580], min: [700, 300] },
  node: { size: [1000, 680], min: [640, 420] },
  terminal: { size: [960, 600], min: [480, 260] },
  files: { size: [1180, 680], min: [760, 360] },
  chat: { size: [1080, 720], min: [560, 360] },
  settings: { size: [960, 660], min: [620, 400] },
  inspect: { size: [820, 620], min: [480, 300] },
};

const overlaps = (a: Rect, b: Rect) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
const onDesk = (r: Rect, d: Size) => r.x >= 0 && r.y >= 0 && r.x + r.w <= d.w && r.y + r.h <= d.h;

/** A small seeded generator, so a failing case can be replayed. */
function rng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

describe("tileLayout", () => {
  const tw = (id: string, kind: string): TileWin => ({ id, size: KINDS[kind].size, min: KINDS[kind].min });

  it("never overlaps, keeps everything on the desktop and every tiled window at its minimum (500 random cases)", () => {
    const rand = rng(20261028);
    const kinds = Object.keys(KINDS);
    for (let c = 0; c < 500; c++) {
      const desk = { w: 1280 + Math.floor(rand() * 1281), h: 720 + Math.floor(rand() * 721) };
      const n = 1 + Math.floor(rand() * 12);
      const wins = Array.from({ length: n }, (_, i) => tw(`w${i}`, kinds[Math.floor(rand() * kinds.length)]));
      const { frames, minimized } = tileLayout(wins, desk, { gutter: 8, margin: 8 });
      const tiled = [...frames.entries()];
      const where = `case ${c}: ${n} windows on ${desk.w}×${desk.h}`;
      // Every window is either tiled or minimized, and the most recent ones are the ones kept.
      expect(tiled.length + minimized.length, where).toBe(n);
      expect(tiled.length, where).toBeGreaterThan(0);
      expect(new Set(tiled.map(([id]) => id)), where).toEqual(new Set(wins.slice(0, tiled.length).map((w) => w.id)));
      for (const [id, r] of tiled) {
        const w = wins.find((x) => x.id === id)!;
        expect(onDesk(r, desk), `${where}: ${id} off the desktop`).toBe(true);
        expect(r.w >= w.min[0] && r.h >= w.min[1], `${where}: ${id} ${r.w}×${r.h} below ${w.min}`).toBe(true);
      }
      for (let i = 0; i < tiled.length; i++) {
        for (let j = i + 1; j < tiled.length; j++) {
          expect(overlaps(tiled[i][1], tiled[j][1]), `${where}: ${tiled[i][0]} overlaps ${tiled[j][0]}`).toBe(false);
        }
      }
    }
  });

  it("leaves 8 px gutters between windows and 8 px margins", () => {
    const { frames } = tileLayout([tw("a", "topology"), tw("b", "terminal")], { w: 1400, h: 800 });
    const [a, b] = [frames.get("a")!, frames.get("b")!];
    expect(a).toMatchObject({ x: 8, y: 8, h: 784 });
    expect(b.x - (a.x + a.w)).toBe(8);
    expect(b.x + b.w).toBe(1392);
  });

  it("puts two windows side by side and four in a 2×2 grid when they fit", () => {
    const two = tileLayout([tw("a", "terminal"), tw("b", "terminal")], { w: 1600, h: 900 }).frames;
    expect(two.get("a")!.y).toBe(two.get("b")!.y);
    const four = [...tileLayout(["a", "b", "c", "d"].map((id) => tw(id, "terminal")), { w: 1600, h: 900 }).frames.values()];
    expect(new Set(four.map((r) => r.x)).size).toBe(2);
    expect(new Set(four.map((r) => r.y)).size).toBe(2);
  });

  it("minimizes the windows that don't fit, keeping the most recently focused", () => {
    // Six big windows on a 1440×900 screen's desktop: they can't all get their minimum.
    const wins = ["dashboard", "topology", "containers", "services", "activity", "workloads"].map((k) => tw(k, k));
    const { frames, minimized } = tileLayout(wins, { w: 1232, h: 818 });
    expect(frames.has("dashboard")).toBe(true);
    expect(minimized.length).toBeGreaterThan(0);
    expect(minimized).toEqual(wins.slice(frames.size).map((w) => w.id));
  });

  it("tiles at most two side by side, or four, when asked", () => {
    const wins = ["a", "b", "c", "d", "e"].map((id) => tw(id, "terminal"));
    const side = tileLayout(wins, { w: 1600, h: 900 }, { max: 2, shape: "row" });
    expect([...side.frames.keys()]).toEqual(["a", "b"]);
    expect(side.frames.get("a")!.y).toBe(side.frames.get("b")!.y);
    expect(side.minimized).toEqual(["c", "d", "e"]);
    expect(tileLayout(wins, { w: 1600, h: 900 }, { max: 4 }).frames.size).toBe(4);
  });

  it("gives a window the whole desktop when even it alone can't get its minimum", () => {
    const { frames, minimized } = tileLayout([tw("a", "dashboard"), tw("b", "terminal")], { w: 600, h: 400 });
    expect(frames.get("a")).toEqual({ x: 8, y: 8, w: 584, h: 384 });
    expect(minimized).toEqual(["b"]);
  });
});

describe("shareOut", () => {
  it("splits evenly, but never below a minimum, and uses every pixel", () => {
    expect(shareOut(1000, [100, 100])).toEqual([500, 500]);
    expect(shareOut(1000, [700, 100])).toEqual([700, 300]);
    expect(shareOut(1001, [0, 0, 0]).reduce((a, b) => a + b, 0)).toBe(1001);
  });
});

describe("fitInside", () => {
  it("brings a restored window wholly onto the desktop with a 12 px margin, at no less than its minimum", () => {
    expect(fitInside({ x: 900, y: 700, w: 600, h: 400 }, desk, [360, 200])).toEqual({ x: 588, y: 388, w: 600, h: 400 });
    // Too wide for the desk: it shrinks toward its minimum rather than sitting flush or past the edge.
    expect(fitInside({ x: 40, y: 20, w: desk.w + 300, h: 300 }, desk, [360, 200])).toEqual({ x: 12, y: 20, w: desk.w - 24, h: 300 });
    // A tiny remembered size (an old tiled frame) grows back to the minimum.
    expect(fitInside({ x: 10, y: 10, w: 200, h: 120 }, desk, [520, 260])).toEqual({ x: 10, y: 10, w: 520, h: 260 });
  });
});

describe("normalizeZ", () => {
  it("renumbers stacking order to 1..n, keeping it", () => {
    const out = normalizeZ([{ id: "a", z: 9000 }, { id: "b", z: 12 }, { id: "c", z: 4999 }]);
    expect(out.map((w) => [w.id, w.z])).toEqual([["a", 3], ["b", 1], ["c", 2]]);
  });
});

describe("placeNew", () => {
  it("steps new windows and keeps them on the desktop", () => {
    const a = placeNew([600, 400], [360, 200], 0, desk);
    const b = placeNew([600, 400], [360, 200], 1, desk);
    expect(b.x - a.x).toBe(26);
    expect(inside(placeNew([5000, 5000], [360, 200], 3, desk))).toBe(true);
  });

  it("leaves a margin on both sides of a window too wide for the desktop", () => {
    const r = placeNew([1240, 580], [640, 300], 0, desk);
    expect(r.x).toBe(12);
    expect(desk.w - (r.x + r.w)).toBe(12);
  });
});

// The desktop on a 1440×900 screen: the sidebar (212 px), top bar and status bar take the rest.
const SCREEN_DESK = { w: 1220, h: 834 };
const NO_SIDEBAR = { w: 1420, h: 834 };

describe("tiling that tiles at 1440×900", () => {
  const real = (kind: string, id = kind): TileWin => ({ id, size: sizeOf(kind), min: tileMinOf(kind) });
  const kinds = Object.keys(REGISTRY);

  /** No overlap, all on the desktop, every frame at least its kind's tileMin. */
  function expectTiled(frames: Map<string, Rect>, wins: TileWin[], desk: Size, where: string) {
    const tiled = [...frames.entries()];
    for (const [id, r] of tiled) {
      const w = wins.find((x) => x.id === id)!;
      expect(onDesk(r, desk), `${where}: ${id} off the desktop`).toBe(true);
      expect(r.w >= w.min[0] && r.h >= w.min[1], `${where}: ${id} ${r.w}×${r.h} below its tileMin ${w.min}`).toBe(true);
    }
    for (let i = 0; i < tiled.length; i++) {
      for (let j = i + 1; j < tiled.length; j++) expect(overlaps(tiled[i][1], tiled[j][1]), `${where}: ${tiled[i][0]} overlaps ${tiled[j][0]}`).toBe(false);
    }
  }

  it("asks no more of Tile than half the desktop for any kind", () => {
    for (const k of kinds) {
      const [w, h] = tileMinOf(k);
      expect(w, k).toBeLessThanOrEqual(460);
      expect(h, k).toBeLessThanOrEqual(380);
    }
    expect(tileMinOf("dashboard")).toEqual([460, 380]);
    expect(tileMinOf("terminal")).toEqual([400, 220]);
    expect(tileMinOf("files")).toEqual([460, 340]);
  });

  it("puts any two registered kinds side by side: exactly two frames, none docked", () => {
    for (const a of kinds) {
      for (const b of kinds) {
        const wins = [real(a, "a"), real(b, "b"), real("chat", "c")];
        for (const desk of [SCREEN_DESK, { w: SCREEN_DESK.w, h: SCREEN_DESK.h - DOCK_H }]) {
          const out = tileLayout(wins, desk, { max: 2, shape: "row" });
          const where = `${a} + ${b} on ${desk.w}×${desk.h}`;
          expect([...out.frames.keys()].sort(), where).toEqual(["a", "b"]);
          expect(out.stacked, where).toBeFalsy();
          expect(out.frames.get("a")!.y, where).toBe(out.frames.get("b")!.y);
          expectTiled(out.frames, wins, desk, where);
          expect(out.minimized).toEqual(["c"]);
        }
      }
    }
  });

  it("stacks the two top and bottom when they can't sit across, and never docks one of them", () => {
    const wins = [real("dashboard"), real("terminal")];
    const narrow = { w: 760, h: 834 };
    const out = tileLayout(wins, narrow, { max: 2, shape: "row" });
    expect(out.stacked).toBe(true);
    expect(out.frames.size).toBe(2);
    expect(out.minimized).toEqual([]);
    expect(out.frames.get("dashboard")!.x).toBe(out.frames.get("terminal")!.x);
    expectTiled(out.frames, wins, narrow, "stacked");
    // Too small even for that: halves, still two windows.
    const tiny = tileLayout(wins, { w: 500, h: 400 }, { max: 2, shape: "row" });
    expect(tiny.frames.size).toBe(2);
    expect(tiny.minimized).toEqual([]);
    const [p, q] = [...tiny.frames.values()];
    expect(overlaps(p, q)).toBe(false);
  });

  it("gives Tile 4 of the Dashboard, Topology, Terminal and Containers a 2×2 grid", () => {
    const wins = ["dashboard", "topology", "terminal", "containers"].map((k) => real(k));
    for (const desk of [SCREEN_DESK, { w: SCREEN_DESK.w, h: SCREEN_DESK.h - DOCK_H }]) {
      const out = tileLayout([...wins, real("activity")], desk, { max: 4, shape: "grid" });
      expect(out.frames.size).toBe(4);
      expect(out.minimized).toEqual(["activity"]);
      const frames = [...out.frames.values()];
      expect(new Set(frames.map((r) => r.x)).size).toBe(2);
      expect(new Set(frames.map((r) => r.y)).size).toBe(2);
      expectTiled(out.frames, wins, desk, `tile 4 on ${desk.w}×${desk.h}`);
    }
  });

  it("tiles all six of the qa case with the sidebar hidden, and says it fits 4 of 6 with it", () => {
    const six = ["dashboard", "topology", "containers", "services", "activity", "workloads"].map((k) => real(k));
    const wide = tileLayout(six, NO_SIDEBAR);
    expect(wide.frames.size).toBe(6);
    expect(wide.minimized).toEqual([]);
    expectTiled(wide.frames, six, NO_SIDEBAR, "six, no sidebar");
    const here = tileLayout(six, SCREEN_DESK);
    expect(here.frames.size).toBe(4);
    expectTiled(here.frames, six, SCREEN_DESK, "six with the sidebar");
  });

  it("tiles any six registered kinds without overlap, each at its tileMin (300 random cases)", () => {
    const rand = rng(20261029);
    for (let c = 0; c < 300; c++) {
      const wins = Array.from({ length: 6 }, (_, i) => real(kinds[Math.floor(rand() * kinds.length)], `w${i}`));
      for (const desk of [SCREEN_DESK, NO_SIDEBAR]) {
        const out = tileLayout(wins, desk);
        expect(out.frames.size + out.minimized.length).toBe(6);
        // At least four always fit at 1440×900.
        expect(out.frames.size, `case ${c} on ${desk.w}`).toBeGreaterThanOrEqual(4);
        expectTiled(out.frames, wins, desk, `case ${c} on ${desk.w}×${desk.h}`);
      }
    }
  });
});

describe("cascadeFree", () => {
  const r = (x: number, y: number, w = 600, h = 400): Rect => ({ x, y, w, h });

  it("leaves a window alone when its corner is free", () => {
    expect(cascadeFree(r(232, 52), [{ x: 12, y: 12 }], SCREEN_DESK)).toEqual(r(232, 52));
  });

  it("steps a window off one whose corner it would cover, keeping its size", () => {
    const out = cascadeFree(r(232, 52), [{ x: 232 + CASCADE_NEAR, y: 52 - 3 }], SCREEN_DESK);
    expect(out).toEqual(r(232 + CASCADE_STEP, 52 + CASCADE_STEP));
  });

  it("gives six windows saved at one origin six origins, on the desktop when there's room", () => {
    const taken: { x: number; y: number }[] = [];
    for (let i = 0; i < 6; i++) {
      const out = cascadeFree(r(232, 52), taken, SCREEN_DESK);
      expect(out.w).toBe(600);
      expect(onDesk(out, SCREEN_DESK)).toBe(true);
      taken.push(out);
    }
    expect(new Set(taken.map((t) => `${t.x},${t.y}`)).size).toBe(6);
  });

  it("starts a new lap from the origin near the far edge, and shrinks a window the size of the desktop to fit", () => {
    const taken = Array.from({ length: 12 }, (_, i) => ({ x: 600 + i * CASCADE_STEP, y: 300 + i * CASCADE_STEP }));
    const lap = cascadeFree(r(600, 300), taken, SCREEN_DESK, [360, 200]);
    expect(onDesk(lap, SCREEN_DESK)).toBe(true);
    expect(lap.y).toBe(12);
    // Stepping never runs past the edge: the frame narrows toward its minimum, keeping 12 px.
    const huge = cascadeFree(r(0, 0, SCREEN_DESK.w, SCREEN_DESK.h), [{ x: 0, y: 0 }], SCREEN_DESK, [480, 300]);
    expect(huge).toMatchObject({ x: CASCADE_STEP, y: CASCADE_STEP });
    expect(huge.x + huge.w).toBeLessThanOrEqual(SCREEN_DESK.w - 12);
    expect(huge.y + huge.h).toBeLessThanOrEqual(SCREEN_DESK.h - 12);
  });
});
