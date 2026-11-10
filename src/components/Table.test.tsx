import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { allocateColumns, fitWidths, matchShortcut, naturalCompare, Table, type AllocColumn, type Column, type ColumnMeasure, type SortState } from "./Table";

interface Row {
  name: string;
  port: number;
  dir?: boolean;
}

const ROWS: Row[] = [
  { name: "node10", port: 8000 },
  { name: "node2", port: 22 },
  { name: "Node1", port: 11434 },
  { name: "models", port: 0, dir: true },
];

const cols = (extra: Partial<Column<Row>> = {}): Column<Row>[] => [
  { key: "name", label: "Name", sort: (r) => r.name, render: (r) => r.name },
  { key: "port", label: "Port", align: "right", sort: (r) => r.port, render: (r) => r.port, priority: 2, ...extra },
  { key: "note", label: "Note", render: () => "—", priority: 3 },
];

const names = () => screen.getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("gridcell")[0].textContent);
const grid = () => screen.getByRole("grid");

describe("sorting", () => {
  it("is natural: node2 before node10, case-insensitive, numbers numerically", async () => {
    const user = userEvent.setup();
    render(<Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} />);
    await user.click(screen.getByRole("button", { name: "Name" }));
    expect(names()).toEqual(["models", "Node1", "node2", "node10"]);
    expect(screen.getByRole("columnheader", { name: /Name/ })).toHaveAttribute("aria-sort", "ascending");
    await user.click(screen.getByRole("button", { name: "Name" }));
    expect(names()).toEqual(["node10", "node2", "Node1", "models"]);
    await user.click(screen.getByRole("button", { name: "Port" }));
    expect(names()).toEqual(["models", "node2", "node10", "Node1"]);
    expect(naturalCompare("crs804/qsfp56-dd-1-2", "crs804/qsfp56-dd-1-10")).toBeLessThan(0);
  });

  it("drops a sort whose column is gone (the same Table reused for another view)", () => {
    const procCols: Column<Row>[] = [{ key: "cpu", label: "CPU", sort: (r) => r.port, render: (r) => r.port }, ...cols()];
    const { rerender } = render(<Table columns={procCols} rows={ROWS} rowKey={(r) => r.name} initialSort={{ key: "cpu", dir: -1 }} />);
    expect(screen.getByRole("columnheader", { name: /CPU/ })).toHaveAttribute("aria-sort", "descending");
    rerender(<Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} initialSort={{ key: "port", dir: 1 }} />);
    expect(screen.getByRole("columnheader", { name: /Port/ })).toHaveAttribute("aria-sort", "ascending");
    expect(names()).toEqual(["models", "node2", "node10", "Node1"]);
  });

  it("keeps pinFirst rows on top in both directions", async () => {
    const user = userEvent.setup();
    render(<Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} pinFirst={(r) => !!r.dir} initialSort={{ key: "name", dir: -1 }} />);
    expect(names()[0]).toBe("models");
    await user.click(screen.getByRole("button", { name: "Name" }));
    expect(names()[0]).toBe("models");
    await user.click(screen.getByRole("button", { name: "Port" }));
    expect(names()).toEqual(["models", "node2", "node10", "Node1"]);
  });

  it("can be controlled", async () => {
    const user = userEvent.setup();
    const onSortChange = vi.fn();
    const { rerender } = render(<Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} sort={{ key: "port", dir: -1 }} onSortChange={onSortChange} />);
    expect(names()[0]).toBe("Node1");
    await user.click(screen.getByRole("button", { name: "Name" }));
    expect(onSortChange).toHaveBeenCalledWith({ key: "name", dir: 1 });
    expect(names()[0]).toBe("Node1");
    rerender(<Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} sort={{ key: "name", dir: 1 } as SortState} onSortChange={onSortChange} />);
    expect(names()[0]).toBe("models");
  });
});

function Multi(props: { onActivate?: (r: Row) => void; onKeyAction?: (a: "delete", rows: Row[]) => void; checkboxes?: boolean }) {
  const [sel, setSel] = useState<Set<string>>(new Set());
  return (
    <Table
      columns={cols()}
      rows={ROWS}
      rowKey={(r) => r.name}
      initialSort={{ key: "name", dir: 1 }}
      selectionMode="multi"
      selection={sel}
      onSelectionChange={(k) => setSel(k)}
      footer
      itemNoun={["port", "ports"]}
      {...props}
    />
  );
}

const selected = () => screen.getAllByRole("row").filter((r) => r.getAttribute("aria-selected") === "true").map((r) => within(r).getAllByRole("gridcell").at(-3)?.textContent ?? within(r).getAllByRole("gridcell")[0].textContent);

describe("selection", () => {
  it("clicks select one; ⌘-click toggles; ⇧-click extends from the anchor", () => {
    render(<Multi />);
    const row = (name: string) => screen.getByText(name).closest("tr")!;
    fireEvent.mouseDown(row("Node1"));
    expect(selected()).toEqual(["Node1"]);
    fireEvent.mouseDown(row("node10"), { shiftKey: true });
    expect(selected()).toEqual(["Node1", "node2", "node10"]);
    fireEvent.mouseDown(row("node2"), { metaKey: true });
    expect(selected()).toEqual(["Node1", "node10"]);
    fireEvent.mouseDown(row("models"));
    expect(selected()).toEqual(["models"]);
    expect(screen.getByRole("status")).toHaveTextContent("4 ports · 1 selected");
  });

  it("drives from the keyboard: ↓, ⇧↓, Space, ⌘A, Enter and ⌘⌫", async () => {
    const user = userEvent.setup();
    const onActivate = vi.fn();
    const onKeyAction = vi.fn();
    render(<Multi onActivate={onActivate} onKeyAction={onKeyAction} />);
    grid().focus();
    await user.keyboard("{ArrowDown}");
    expect(selected()).toEqual(["models"]);
    expect(grid().getAttribute("aria-activedescendant")).toBe(screen.getByText("models").closest("tr")!.id);
    await user.keyboard("{ArrowDown}{Shift>}{ArrowDown}{/Shift}");
    expect(selected()).toEqual(["Node1", "node2"]);
    await user.keyboard(" ");
    expect(selected()).toEqual(["Node1"]);
    await user.keyboard("{Meta>}a{/Meta}");
    expect(selected()).toHaveLength(4);
    expect(screen.getByRole("status")).toHaveTextContent("4 ports · 4 selected");
    await user.keyboard("{End}{Enter}");
    expect(onActivate).toHaveBeenCalledWith(ROWS[0]);
    await user.keyboard("{Shift>}{Home}{/Shift}{Meta>}{Backspace}{/Meta}");
    expect(onKeyAction).toHaveBeenCalledWith("delete", expect.arrayContaining([ROWS[0], ROWS[1], ROWS[2], ROWS[3]]));
  });

  it("finds rows by typing their name", async () => {
    const user = userEvent.setup();
    render(<Multi />);
    grid().focus();
    await user.keyboard("node1");
    expect(selected()).toEqual(["Node1"]);
    await user.keyboard("0");
    expect(selected()).toEqual(["node10"]);
  });

  it("keeps the legacy single selection working", async () => {
    const user = userEvent.setup();
    function Legacy() {
      const [s, setS] = useState<string | null>("node2");
      return <Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} selected={s} onSelect={setS} initialSort={{ key: "name", dir: 1 }} />;
    }
    render(<Legacy />);
    expect(selected()).toEqual(["node2"]);
    grid().focus();
    await user.keyboard("{ArrowDown}");
    expect(selected()).toEqual(["node10"]);
    fireEvent.mouseDown(screen.getByText("models").closest("tr")!, { metaKey: true });
    expect(selected()).toEqual(["models"]);
  });

  it("offers a checkbox column", () => {
    render(<Multi checkboxes />);
    fireEvent.mouseDown(screen.getAllByRole("checkbox", { name: "Select row" })[1]);
    fireEvent.mouseDown(screen.getAllByRole("checkbox", { name: "Select row" })[2]);
    expect(selected()).toEqual(["Node1", "node2"]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all" }));
    expect(selected()).toHaveLength(4);
  });
});

describe("columns", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("hides and restores columns from the header menu, and remembers it", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} tableId="ports" />);
    fireEvent.contextMenu(screen.getAllByRole("columnheader")[0]);
    const menu = screen.getByRole("menu", { name: "Columns" });
    expect(within(menu).getByRole("checkbox", { name: "Name" })).toBeDisabled();
    await user.click(within(menu).getByRole("checkbox", { name: "Port" }));
    expect(screen.queryByRole("columnheader", { name: /Port/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Name" }));
    unmount();

    const saved = JSON.parse(localStorage.getItem("sparkbox.table.ports")!);
    expect(saved.hidden).toEqual(["port"]);
    expect(saved.sort).toEqual({ key: "name", dir: 1 });

    render(<Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} tableId="ports" />);
    expect(screen.queryByRole("columnheader", { name: /Port/ })).toBeNull();
    expect(names()).toEqual(["models", "Node1", "node2", "node10"]);
    fireEvent.contextMenu(screen.getAllByRole("columnheader")[0]);
    await user.click(screen.getByRole("menuitem", { name: "Reset columns" }));
    expect(screen.getByRole("columnheader", { name: /Port/ })).toBeInTheDocument();
  });

  it("fits wide content by shrinking only the widest columns", () => {
    const natural = [{ key: "node", w: 80 }, { key: "pid", w: 60 }, { key: "cmd", w: 900 }, { key: "runs", w: 300 }];
    const fit = fitWidths(natural, { node: 48, pid: 48, cmd: 64, runs: 64 }, 800);
    expect(fit.node).toBe(80);
    expect(fit.pid).toBe(60);
    expect(fit.cmd).toBeLessThan(900);
    expect(fit.runs).toBe(300);
    expect(Object.values(fit).reduce((a, b) => a + b, 0)).toBeLessThanOrEqual(800);
    // Both long columns give way once one alone isn't enough.
    const tight = fitWidths(natural, { node: 48, pid: 48, cmd: 64, runs: 64 }, 500);
    expect(tight.cmd).toBe(tight.runs);
    expect(tight.node + tight.pid + tight.cmd + tight.runs).toBeLessThanOrEqual(500);
    expect(fitWidths(natural, {}, 2000)).toEqual({ node: 80, pid: 60, cmd: 900, runs: 300 });
  });

  it("shows the empty message and counts in the footer", () => {
    render(<Table columns={cols()} rows={[]} rowKey={(r) => r.name} empty="No listening ports reported yet." footer={<span>refreshed 5 s ago</span>} itemNoun={["port", "ports"]} />);
    expect(screen.getByText("No listening ports reported yet.")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("0 portsrefreshed 5 s ago");
  });
});

// ---- §D5: allocation, keyboard menus, row actions

/** A Workloads-like column set; widths are what each needs at text scale 1 (px, padding included). */
const WORKLOADS: (AllocColumn & { m: ColumnMeasure })[] = [
  { key: "node", fit: "content", priority: 1, m: { content: 78, header: 52 } },
  { key: "pid", fit: "content", priority: 1, m: { content: 58, header: 44 } },
  { key: "user", fit: "content", priority: 3, m: { content: 70, header: 50 } },
  { key: "cpu", fit: "content", priority: 1, m: { content: 56, header: 66, short: 48 } },
  { key: "mem", fit: "content", priority: 2, m: { content: 72, header: 82, short: 52 } },
  { key: "time", fit: "content", priority: 3, m: { content: 64, header: 54 } },
  { key: "cmd", fit: "flex", priority: 1, minWidth: 120, m: { content: 900, header: 92 } },
  { key: "image", fit: "flex", priority: 2, minWidth: 100, m: { content: 320, header: 64 } },
];

const scaled = (k: number) => (key: string): ColumnMeasure => {
  const m = WORKLOADS.find((c) => c.key === key)!.m;
  return { content: m.content * k, header: m.header * k, short: m.short != null ? m.short * k : undefined };
};

describe("allocateColumns", () => {
  const widths = [1180, 900, 760, 640, 520, 420, 360];
  for (const scale of [1, 1.25, 1.4]) {
    it(`never cuts PID, Node or CPU%, nor any header, at text scale ${scale}`, () => {
      const measure = scaled(scale);
      for (const w of widths) {
        const a = allocateColumns(WORKLOADS, w, measure);
        for (const key of ["node", "pid", "cpu"]) {
          expect(a.hidden).not.toContain(key);
          expect(a.widths[key], `${key} at ${w}px`).toBeGreaterThanOrEqual(Math.ceil(measure(key).content));
        }
        for (const [key, width] of Object.entries(a.widths)) {
          const m = measure(key);
          const head = a.short.includes(key) && m.short != null ? Math.min(m.short, m.header) : m.header;
          expect(width, `${key} header at ${w}px`).toBeGreaterThanOrEqual(Math.ceil(head));
        }
        // No phantom sideways scroll bar: it either fits exactly or says it scrolls.
        if (!a.scroll) expect(a.total).toBeLessThanOrEqual(w);
        else expect(Object.keys(a.widths).every((k) => WORKLOADS.find((c) => c.key === k)!.priority === 1)).toBe(true);
      }
    });
  }

  it("shrinks the flex columns first, then shortens headers, then hides priority 3 and then 2, one at a time", () => {
    const m = scaled(1);
    const roomy = allocateColumns(WORKLOADS, 2000, m);
    expect(roomy).toMatchObject({ hidden: [], short: [], scroll: false, total: 2000 });
    expect(roomy.widths.cmd).toBeGreaterThanOrEqual(900);

    const mid = allocateColumns(WORKLOADS, 900, m);
    expect(mid.hidden).toEqual([]);
    expect(mid.widths.cmd).toBeLessThan(900);
    expect(mid.widths.pid).toBe(58);
    expect(mid.total).toBe(900);

    // One column at a time, the rightmost of the lowest priority first: Time goes, User stays.
    const narrow = allocateColumns(WORKLOADS, 600, m);
    expect(narrow.hidden).toEqual(["time"]);
    expect(narrow.scroll).toBe(false);
    const less = allocateColumns(WORKLOADS, 540, m);
    expect(less.hidden.slice(0, 2)).toEqual(["time", "user"]);

    // Image is the rightmost priority-2 column, so it goes before Memory.
    const tight = allocateColumns(WORKLOADS, 420, m);
    expect(tight.hidden).toEqual(["time", "user", "image"]);
    expect(tight.scroll).toBe(false);
    expect(tight.total).toBe(420);
  });

  it("hides priority-2 columns one at a time, the rightmost first", () => {
    const cols: AllocColumn[] = [
      { key: "name", fit: "content", priority: 1 },
      { key: "a", fit: "content", priority: 2 },
      { key: "b", fit: "content", priority: 2 },
      { key: "c", fit: "content", priority: 2 },
    ];
    const m = () => ({ content: 100, header: 40 });
    expect(allocateColumns(cols, 400, m).hidden).toEqual([]);
    expect(allocateColumns(cols, 399, m).hidden).toEqual(["c"]);
    expect(allocateColumns(cols, 299, m).hidden).toEqual(["c", "b"]);
    expect(allocateColumns(cols, 199, m).hidden).toEqual(["c", "b", "a"]);
  });

  it("draws a column's short form (Column.narrow) before the table scrolls", () => {
    const cols: AllocColumn[] = [
      { key: "node", fit: "content", priority: 1, narrow: true },
      { key: "name", fit: "content", priority: 1 },
      { key: "cpu", fit: "content", priority: 2 },
    ];
    const m = (key: string): ColumnMeasure => (key === "node" ? { content: 90, header: 50, narrow: 52 } : { content: 120, header: 40 });
    const wide = allocateColumns(cols, 400, m);
    expect(wide).toMatchObject({ hidden: [], narrow: [], scroll: false });
    // CPU goes first; only then does Node shrink to "c1".
    const tile = allocateColumns(cols, 200, m);
    expect(tile.hidden).toEqual(["cpu"]);
    expect(tile.narrow).toEqual(["node"]);
    expect(tile.scroll).toBe(false);
    expect(tile.widths.node).toBeLessThan(90);
    expect(tile.total).toBe(200);
  });

  it("scrolls only when the priority-1 columns can't fit at their narrowest", () => {
    const a = allocateColumns(WORKLOADS, 300, scaled(1));
    expect(a.scroll).toBe(true);
    expect(Object.keys(a.widths).sort()).toEqual(["cmd", "cpu", "node", "pid"]);
    expect(a.short).toContain("cpu");
  });

  it("gives the leftover room to flex columns, so the table spans the width exactly", () => {
    const cols: AllocColumn[] = [{ key: "a", fit: "content", priority: 1 }, { key: "b", fit: "flex", priority: 1 }];
    const a = allocateColumns(cols, 500, () => ({ content: 60, header: 40 }));
    expect(a.widths).toEqual({ a: 60, b: 440 });
    const fixedOnly = allocateColumns([{ key: "a", fit: "content", priority: 1 }, { key: "b", fit: "content", priority: 1 }], 500, () => ({ content: 100, header: 40 }));
    expect(fixedOnly.widths).toEqual({ a: 250, b: 250 });
    expect(a.total).toBe(500);
  });
});

/** The Dashboard's node table: Status is the message column (§2.5, M13). 48ch is 336 px at scale 1. */
const DASH: (AllocColumn & { m: ColumnMeasure })[] = [
  { key: "node", fit: "content", priority: 1, m: { content: 96, header: 50 } },
  { key: "status", fit: "flex", priority: 1, minWidth: 90, message: true, m: { content: 250, header: 60, messageCap: 336 } },
  { key: "cpu", fit: "content", priority: 2, m: { content: 96, header: 44 } },
  { key: "gpu", fit: "content", priority: 1, m: { content: 96, header: 44 } },
  { key: "mem", fit: "content", priority: 1, m: { content: 170, header: 70 } },
  { key: "power", fit: "content", priority: 2, m: { content: 104, header: 100, short: 60 } },
  { key: "disk", fit: "content", priority: 2, m: { content: 190, header: 44 } },
  { key: "fabric", fit: "content", priority: 1, m: { content: 110, header: 120, short: 70 } },
  { key: "lan", fit: "content", priority: 3, m: { content: 110, header: 100, short: 64 } },
  { key: "ctrs", fit: "content", priority: 2, m: { content: 44, header: 86, short: 44 } },
];

const dashScaled = (k: number) => (key: string): ColumnMeasure => {
  const m = DASH.find((c) => c.key === key)!.m;
  return { content: m.content * k, header: m.header * k, short: m.short != null ? m.short * k : undefined, messageCap: m.messageCap != null ? m.messageCap * k : undefined };
};

describe("message columns (§2.5)", () => {
  for (const scale of [1, 1.25, 1.4]) {
    it(`keep the whole sentence while any priority-2 or -3 column is visible, at text scale ${scale}`, () => {
      const measure = dashScaled(scale);
      for (let w = 1600; w >= 360; w -= 20) {
        const a = allocateColumns(DASH, w, measure);
        const lower = Object.keys(a.widths).filter((k) => DASH.find((c) => c.key === k)!.priority > 1);
        if (lower.length) expect(a.widths.status, `status at ${w}px with ${lower.join(", ")} shown`).toBeGreaterThanOrEqual(Math.ceil(250 * scale));
        if (!a.scroll) expect(a.total).toBeLessThanOrEqual(w);
      }
    });
  }

  it("hides lower-priority columns before the message gives up a pixel, then shrinks it last", () => {
    const m = dashScaled(1);
    const wide = allocateColumns(DASH, 1500, m);
    expect(wide.hidden).toEqual([]);
    // The table that used to cut "Root disk 85% full (143 …": now LAN hides first.
    const mid = allocateColumns(DASH, 1180, m);
    expect(mid.widths.status).toBeGreaterThanOrEqual(250);
    expect(mid.hidden).toContain("lan");
    // Only priority 1 left and still too narrow: the message is the one that gives way.
    const tight = allocateColumns(DASH, 640, m);
    expect(tight.hidden).toEqual(expect.arrayContaining(["lan", "cpu", "power", "disk", "ctrs"]));
    expect(tight.widths.status).toBeLessThan(250);
    expect(tight.widths.status).toBeGreaterThanOrEqual(90);
    expect(tight.scroll).toBe(false);
  });

  it("drops lower-priority columns one at a time, the rightmost first, and doesn't stretch the message", () => {
    const m = dashScaled(1);
    const seen = new Set<string>();
    for (let w = 1400; w >= 700; w -= 10) {
      const a = allocateColumns(DASH, w, m);
      seen.add(a.hidden.join(","));
      const p2 = ["cpu", "power", "disk", "ctrs"].filter((k) => a.hidden.includes(k));
      // Priority 2 goes from the right: ctrs, then disk, then power, then cpu; LAN (priority 3) before any.
      expect(p2).toEqual(["ctrs", "disk", "power", "cpu"].filter((_, i) => i < p2.length).sort((x, y) => ["cpu", "power", "disk", "ctrs"].indexOf(x) - ["cpu", "power", "disk", "ctrs"].indexOf(y)));
      if (p2.length) expect(a.hidden).toContain("lan");
      if (!a.scroll && a.hidden.length === 0) expect(a.widths.status).toBeLessThan(a.total / 3);
    }
    expect(seen.size).toBeGreaterThan(3);
  });

  it("holds at most 48 characters: a longer sentence still shrinks to that and no further", () => {
    const long = (key: string): ColumnMeasure => (key === "status" ? { content: 700, header: 60, messageCap: 336 } : dashScaled(1)(key));
    const a = allocateColumns(DASH, 1300, long);
    const lower = Object.keys(a.widths).filter((k) => DASH.find((c) => c.key === k)!.priority > 1);
    expect(lower.length).toBeGreaterThan(0);
    expect(a.widths.status).toBeGreaterThanOrEqual(336);
  });
});

describe("keyboard", () => {
  it("matches Mac-style shortcuts", () => {
    const k = (key: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}) => ({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods });
    expect(matchShortcut(k("Backspace", { metaKey: true }), "⌘⌫")).toBe(true);
    expect(matchShortcut(k("Backspace"), "⌘⌫")).toBe(false);
    expect(matchShortcut(k(" "), "Space")).toBe(true);
    expect(matchShortcut(k("c", { metaKey: true }), "⌘C")).toBe(true);
    expect(matchShortcut(k("F10", { shiftKey: true }), "⇧F10")).toBe(true);
    expect(matchShortcut(k("F10"), "⇧F10")).toBe(false);
  });

  it("opens the active row's menu with ⇧F10 and the context-menu key, anchored to that row", async () => {
    const user = userEvent.setup();
    const onMenu = vi.fn();
    render(<Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} initialSort={{ key: "name", dir: 1 }} onActivate={() => {}} onRowContextMenu={onMenu} />);
    grid().focus();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    await user.keyboard("{Shift>}{F10}{/Shift}");
    expect(onMenu).toHaveBeenCalledTimes(1);
    const [row, ev, sel] = onMenu.mock.calls[0];
    expect(row).toBe(ROWS[2]);
    expect(ev.detail).toBe(-1);
    expect([...sel]).toEqual(["Node1"]);
    fireEvent.keyDown(grid(), { key: "ContextMenu" });
    expect(onMenu).toHaveBeenCalledTimes(2);
    // keymap.ts's own ⇧F10 dispatches a contextmenu on the focused grid: that reaches the row too.
    fireEvent.contextMenu(grid(), { detail: -1 });
    expect(onMenu).toHaveBeenCalledTimes(3);
    expect(onMenu.mock.calls[2][0]).toBe(ROWS[2]);
  });

  it("runs row actions by their shortcut and calls onActivate on Enter", async () => {
    const user = userEvent.setup();
    const remove = vi.fn();
    const copy = vi.fn();
    const onActivate = vi.fn();
    render(
      <Table columns={cols()} rows={ROWS} rowKey={(r) => r.name} initialSort={{ key: "name", dir: 1 }} onActivate={onActivate}
        rowActions={(r) => [{ label: "Copy name", shortcut: "⌘C", run: () => copy(r.name) }, { label: "Remove", shortcut: "⌘⌫", danger: true, run: () => remove(r.name) }]} />,
    );
    grid().focus();
    await user.keyboard("{ArrowDown}");
    await user.keyboard("{Meta>}c{/Meta}");
    expect(copy).toHaveBeenCalledWith("models");
    await user.keyboard("{ArrowDown}{Meta>}{Backspace}{/Meta}");
    expect(remove).toHaveBeenCalledWith("Node1");
    await user.keyboard("{Enter}");
    expect(onActivate).toHaveBeenCalledWith(ROWS[2]);
  });

  it("keeps buttons inside cells out of the Tab order", () => {
    const withButton: Column<Row>[] = [...cols(), { key: "act", label: "Action", render: (r) => <button type="button">Open {r.name}</button> }];
    render(<Table columns={withButton} rows={ROWS} rowKey={(r) => r.name} />);
    for (const b of screen.getAllByRole("button", { name: /^Open / })) expect(b).toHaveAttribute("tabindex", "-1");
  });

  it("warns in development when a table has a context menu but nothing for Enter", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    render(<Table tableId="no-activate" columns={cols()} rows={ROWS} rowKey={(r) => r.name} onRowContextMenu={() => {}} />);
    expect(warn).toHaveBeenCalledWith(expect.stringContaining("no onActivate"));
    warn.mockRestore();
  });

  it("shows the short header with the full label as its name and tooltip", () => {
    const narrow: Column<Row>[] = [{ key: "name", label: "Name", render: (r) => r.name }, { key: "port", label: "Listening port", short: "Port", render: (r) => r.port }];
    render(<Table columns={narrow} rows={ROWS} rowKey={(r) => r.name} />);
    // jsdom has no layout, so everything "fits": the full label shows and the short one waits.
    expect(screen.getByRole("columnheader", { name: /Listening port/ })).toBeInTheDocument();
  });
});
