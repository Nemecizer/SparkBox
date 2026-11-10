// C5: a table of every node's rows never loses the Node column. Containers, Workloads (its
// processes and its listening ports), Services and the Activity log are drawn at their tile
// minimum, against the browser preview's backend, and every row has to say which node it's on.
// jsdom has no layout, so the measuring pass is given the widths WebKit would measure: each
// column as wide as its widest cell's text at 12 px.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { ReactNode } from "react";
import { defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { resetStore, startClusterStore } from "../lib/store";
import { mockTauri } from "./tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { tileMinOf } from "../windows/registry";
import { ActivityLog } from "../windows/ActivityLog";
import { Containers } from "../windows/Containers";
import { Services } from "../windows/Services";
import { Workloads } from "../windows/Workloads";

const CHAR_PX = 6.8;
const CELL_PAD = 17;
let tableWidth = 460;

/** The cell under header `col` in a row, counting a cell that spans its neighbours. */
function cellAt(tr: Element, col: number): HTMLElement | null {
  let i = 0;
  for (const td of tr.children as HTMLCollectionOf<HTMLTableCellElement>) {
    if (td.classList.contains("check-cell")) continue;
    const span = td.colSpan || 1;
    if (col < i + span) return span === 1 ? td : null;
    i += span;
  }
  return null;
}

function contentWidth(th: HTMLElement): number {
  const table = th.closest("table")!;
  const heads = [...table.querySelectorAll<HTMLElement>("thead th[data-col]")];
  const col = heads.indexOf(th);
  const narrow = table.classList.contains("measuring-narrow");
  let w = 0;
  for (const tr of table.querySelectorAll("tbody tr[data-key]:not(.row-detail)")) {
    const td = cellAt(tr, col);
    if (!td) continue;
    const shown = narrow ? td.querySelector(".cell-narrow") ?? td.querySelector(".cell-wide") ?? td : td.querySelector(".cell-wide") ?? td;
    w = Math.max(w, (shown.textContent ?? "").length * CHAR_PX + CELL_PAD);
  }
  return w;
}

const real = {
  rect: Element.prototype.getBoundingClientRect,
  cw: Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth")!,
};

beforeAll(() => {
  Element.prototype.getBoundingClientRect = function (this: Element) {
    if (this instanceof HTMLElement && this.tagName === "TH" && this.dataset.col && this.closest("table.measuring")) {
      const w = contentWidth(this);
      return { width: w, right: w, x: 0, y: 0, top: 0, left: 0, bottom: 0, height: 0, toJSON: () => ({}) } as DOMRect;
    }
    return real.rect.call(this);
  };
  Object.defineProperty(Element.prototype, "clientWidth", {
    configurable: true,
    get() {
      return (this as Element).classList?.contains("table-scroll") ? tableWidth : 0;
    },
  });
});

afterAll(() => {
  Element.prototype.getBoundingClientRect = real.rect;
  Object.defineProperty(Element.prototype, "clientWidth", real.cw);
});

async function show(kind: string, node: ReactNode, width?: number) {
  resetStore();
  mockTauri({ ...defaultHandlers(), ...Object.assign({}, ...MOCKS) });
  await startClusterStore();
  // The window's body at its tile minimum, less its frame and padding.
  tableWidth = width ?? tileMinOf(kind)[0] - 26;
  let out!: ReturnType<typeof render>;
  await act(async () => {
    out = render(<WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}>{node}</WindowManagerProvider>);
  });
  // Let the lists load and the allocation settle (it re-measures on the next render).
  for (let i = 0; i < 4; i++) await act(async () => new Promise((r) => setTimeout(r, 30)));
  return out;
}

const NODES = ["cell1", "cell2", "cell3", "gx10-0000"];
/** Every row says its node: in full, or in its short form ("c1", "gx10"), or "All 4" for a fleet row. */
function expectNodeOnEveryRow(container: HTMLElement, label: string) {
  const table = container.querySelector("table.grid")!;
  const heads = [...table.querySelectorAll("thead th[data-col]")].map((th) => (th as HTMLElement).dataset.col);
  // Services is a matrix, a column per node: then every node keeps its column.
  if (!heads.includes("node") && heads.some((h) => h?.startsWith("n:"))) {
    expect(heads.filter((h) => h?.startsWith("n:")).length, `${label}: columns ${heads.join(", ")}`).toBe(NODES.length);
    return;
  }
  expect(heads, `${label}: columns ${heads.join(", ")}`).toContain("node");
  const rows = [...table.querySelectorAll("tbody tr[data-key]:not(.row-detail)")];
  expect(rows.length, `${label}: rows`).toBeGreaterThan(0);
  const col = heads.indexOf("node");
  for (const tr of rows) {
    const td = cellAt(tr, col);
    const text = td ? (td.querySelector(":is(.is-narrow) > .cell-narrow")?.textContent ?? td.textContent ?? "") : "";
    expect(text.trim().length, `${label}: row ${tr.getAttribute("data-key")} has no node`).toBeGreaterThan(0);
    // A node, its short form, a group of them, or this Mac (SparkBox's own rows in the Activity log).
    expect(NODES.some((n) => text.includes(n)) || /^(c\d|gx10|all|\d)/i.test(text.trim()) || /,|–|Mac/.test(text), `${label}: "${text}"`).toBe(true);
  }
}

describe("the Node column at the tile minimum (C5)", () => {
  it("Containers", async () => {
    const { container } = await show("containers", <Containers params={{}} />);
    expectNodeOnEveryRow(container, "Containers");
  });

  it("Workloads: processes", async () => {
    const { container } = await show("workloads", <Workloads />);
    expectNodeOnEveryRow(container, "Workloads");
  });

  it("Workloads: listening ports", async () => {
    const { container } = await show("workloads", <Workloads />);
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: /Listening ports/ }));
    });
    for (let i = 0; i < 3; i++) await act(async () => new Promise((r) => setTimeout(r, 30)));
    expectNodeOnEveryRow(container, "Listening ports");
  });

  it("Containers and Workloads narrowed by hand to 300 px", async () => {
    const c = await show("containers", <Containers params={{}} />, 300);
    expectNodeOnEveryRow(c.container, "Containers at 300 px");
    c.unmount();
    const w = await show("workloads", <Workloads />, 300);
    expectNodeOnEveryRow(w.container, "Workloads at 300 px");
  });

  it("Services", async () => {
    const { container } = await show("services", <Services params={{}} />);
    expectNodeOnEveryRow(container, "Services");
  });

  it("Activity log", async () => {
    const { container } = await show("activity", <ActivityLog params={{}} />);
    expectNodeOnEveryRow(container, "Activity log");
  });
});
