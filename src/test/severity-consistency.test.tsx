// One severity mapping (§2.3, M12): every surface in a window takes its tone from the same
// alert severity, and a window showing only warnings paints nothing red. Also the glyph for
// each state, and that offline is the loudest one.
import { act, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { glyphOfStatus, StateGlyph, toneOf, type GlyphState } from "../components/ui";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { resetStore, startClusterStore } from "../lib/store";
import { Dashboard } from "../windows/Dashboard";
import { FabricWindow } from "../windows/FabricWindow";
import { NodeDetail } from "../windows/NodeDetail";
import { SwitchWindow } from "../windows/SwitchWindow";
import { Topology } from "../windows/Topology";
import { WindowManagerProvider } from "../wm/WindowManager";
import { mockTauri } from "./tauri";

// jsdom has no canvas: uPlot gets a stand-in that draws nothing (the DOM around it is what's checked).
vi.mock("uplot", () => {
  class FakePlot {
    series: { show: boolean }[] = [];
    width = 400;
    height = 160;
    setData() {}
    setSeries() {}
    setSize() {}
    setSelect() {}
    destroy() {}
    static sync() {
      return { sub() {}, unsub() {}, pub() {} };
    }
  }
  return { default: FakePlot };
});

const STATES: GlyphState[] = ["ok", "warn", "crit", "offline", "affected", "recovering", "planned", "silenced"];

describe("StateGlyph and toneOf (§2.3)", () => {
  it("draws each state with its own shape, word and tone", () => {
    const { container } = render(<>{STATES.map((s) => <StateGlyph key={s} state={s} />)}</>);
    const glyphs = [...container.querySelectorAll("svg.sglyph")];
    expect(glyphs.map((g) => g.getAttribute("aria-label"))).toEqual(["Healthy", "Warning", "Critical", "Offline", "Affected", "Recovering", "Planned", "Silenced"]);
    const tone = (s: GlyphState) => [...glyphs[STATES.indexOf(s)].classList].find((c) => c.startsWith("tone-"));
    expect(STATES.map(tone)).toEqual(["tone-neutral", "tone-warn", "tone-crit", "tone-crit", "tone-neutral", "tone-neutral", "tone-neutral", "tone-neutral"]);
    // Shapes: a triangle, a filled diamond, a filled circle with a white slash, a hollow ring.
    const shape = (s: GlyphState) => glyphs[STATES.indexOf(s)].innerHTML;
    expect(shape("warn")).toContain('d="M5 .9 9.4 8.7H.6z"');
    expect(shape("crit")).toContain('d="M5 .3 9.7 5 5 9.7.3 5z"');
    expect(shape("offline")).toMatch(/<circle[^>]*r="4.8"[^>]*fill="currentColor"/);
    expect(shape("offline")).toContain('class="sglyph-slash"');
    expect(shape("affected")).toMatch(/<circle[^>]*fill="none"/);
    // Offline is the biggest filled mark: louder than critical.
    expect(shape("offline")).not.toEqual(shape("crit"));
    expect(container.innerHTML).toMatchSnapshot();
  });

  it("maps every severity and state word to one tone", () => {
    expect(["ok", "online", "info", "affected", "recovering", "planned", "silenced", "known", undefined].map(toneOf)).toEqual(Array(9).fill("neutral"));
    expect(["warn", "warning", "stale", "connecting"].map(toneOf)).toEqual(Array(4).fill("warn"));
    expect(["crit", "critical", "offline", "error", "down", "failed"].map(toneOf)).toEqual(Array(6).fill("crit"));
    expect(glyphOfStatus("offline")).toBe("offline");
    expect(glyphOfStatus("error")).toBe("offline");
    expect(glyphOfStatus("stale")).toBe("warn");
    expect(glyphOfStatus("online")).toBe("ok");
  });
});

/** Anything painted red: critical or offline glyphs, rows, bands, text and chips. */
const CRIT = [
  ".sglyph-crit", ".sglyph-offline", ".tone-crit", ".row-crit", ".row-off", ".state-crit", ".crit-text", ".crit-fill",
  ".sdot-crit", ".sdot-error", ".level-crit", ".tone-text-crit", ".mon-state.crit", ".fab-chip.tone-crit",
].join(", ");

function Host({ children }: { children: ReactNode }) {
  return (
    <WindowManagerProvider sizeOf={() => [900, 640]} isKnown={() => true}>
      {children}
    </WindowManagerProvider>
  );
}

describe("a warning-only cluster paints nothing red (M12)", () => {
  // The Demo baseline: gx10-0000's disk at 85% and a failed unit on cell1, both warnings.
  const backend = () => mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS));
  beforeAll(async () => {
    resetStore();
    backend();
    await startClusterStore();
  });
  // Each test's teardown resets the backend; the stores keep what they loaded.
  beforeEach(backend);

  const windows: [string, () => ReactNode][] = [
    ["Dashboard", () => <Dashboard />],
    ["Topology", () => <Topology />],
    ["cell1's node window", () => <NodeDetail params={{ id: "cell1" }} />],
    ["gx10-0000's node window", () => <NodeDetail params={{ id: "gx10-0000" }} />],
    ["Fabric", () => <FabricWindow params={{}} />],
    ["CRS804", () => <SwitchWindow params={{ id: "crs804" }} />],
  ];

  for (const [name, el] of windows) {
    it(name, async () => {
      let container!: HTMLElement;
      await act(async () => {
        container = render(<Host>{el()}</Host>).container;
      });
      // Let health, endpoints and fabric arrive.
      for (let i = 0; i < 20 && /^Waiting/.test(container.textContent ?? ""); i++) await act(async () => new Promise((r) => setTimeout(r, 50)));
      await act(async () => new Promise((r) => setTimeout(r, 60)));
      const red = [...container.querySelectorAll(CRIT)].map((e) => `${e.tagName.toLowerCase()}.${[...e.classList].join(".")} "${(e.textContent ?? "").slice(0, 40)}"`);
      expect(red, `${name} paints red`).toEqual([]);
      // It rendered the real window (not a waiting state), and it does show the warnings, in amber.
      expect(container.textContent ?? "", name).not.toMatch(/^Waiting/);
      if (name === "CRS804" && baseSnapshot.switches.find((x) => x.id === "crs804")?.configured === false) {
        // The real cluster's CRS804 isn't set up in SparkBox yet: a quiet "Not set up", nothing drawn as a fault.
        expect(container.textContent).toMatch(/Not set up/);
        return;
      }
      expect(container.querySelectorAll(".sglyph, .sdot, .fab-cell, .switch-state").length, `${name} draws states`).toBeGreaterThan(0);
      if (name === "Dashboard") {
        expect(container.querySelectorAll(".mon-issues.state-warn").length).toBe(1);
        expect(container.querySelectorAll("tr.row-warn:not(.row-detail)").length).toBe(2);
        expect(container.querySelectorAll(".sglyph-warn").length).toBeGreaterThanOrEqual(4);
      }
      if (name === "Fabric") {
        // The healthy fabric is quiet (§C10): no counter chips, one line per cell instead.
        expect(container.querySelectorAll(".fab-cell").length).toBe(8);
        expect(container.querySelectorAll(".fab-chip").length).toBe(0);
        expect(container.querySelectorAll(".fab-chips-quiet").length).toBe(8);
        expect(container.querySelector(".fab-chips-quiet")?.textContent).toMatch(/^No errors in 1 h/);
        // Rates print their units.
        expect(container.querySelector(".fab-cell-rates")?.textContent).toMatch(/↓ [\d.]+ [kMG]?b\/s ↑ [\d.]+ [kMG]?b\/s/);
      }
    });
  }
});
