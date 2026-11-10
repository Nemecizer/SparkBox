import { useState } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ConfirmHost } from "../components/ConfirmHost";
import { Toasts } from "../components/Toasts";
import { dismissToast, getToasts } from "../lib/toasts";
import { _resetUndoForTests } from "../lib/undo";
import { plural } from "../lib/vocabulary";
import {
  Desktop, setViewParams, useCloseGuard, useViewLink, useWindowEvent, useWindowTitle, useWinState, useWM, WindowManagerProvider, type Params, type WM,
} from "./WindowManager";

const KINDS = ["notes", "files", "term", "plain", "linked"];

/** Unsaved text, like the editor. */
function Files() {
  const [dirty, setDirty] = useState(false);
  useCloseGuard(() => (dirty ? "/etc/hosts has unsaved changes" : null));
  return <button onClick={() => setDirty(true)}>edit hosts</button>;
}

/** Closing with several tabs closes one tab instead, like the Terminal. */
function Term() {
  const [tabs, setTabs] = useState(2);
  useWindowEvent("close-request", (e) => {
    if (tabs > 1) {
      e.preventDefault();
      setTabs((n) => n - 1);
    }
  });
  return <span>{tabs} tabs</span>;
}

/** View state that should survive a restart. */
function Notes() {
  const [tab, setTab] = useWinState("tab", "overview");
  return <button onClick={() => setTab("gpu")}>tab: {tab}</button>;
}

/** Follows links to a tab, like a node window. */
function Linked({ params }: { params: Params }) {
  const [tab, setTab] = useWinState("tab", params.tab ?? "overview");
  useViewLink(params, () => params.tab && setTab(params.tab));
  return <button onClick={() => setTab("disks")}>linked: {tab}</button>;
}

function Body({ kind, params }: { kind: string; params: Params }) {
  if (kind === "linked") return <Linked params={params} />;
  if (kind === "files") return <Files />;
  if (kind === "term") return <Term />;
  if (kind === "notes") return <Notes />;
  return <span>plain body</span>;
}

let wm!: WM;
function Probe() {
  wm = useWM();
  return null;
}

function mount(opts: { tileMin?: [number, number] } = {}) {
  return render(
    <WindowManagerProvider
      sizeOf={() => [600, 400]}
      minSizeOf={() => [360, 200]}
      tileMinOf={() => opts.tileMin ?? [360, 200]}
      isKnown={(k) => KINDS.includes(k)}
      titleOf={(w) => w.kind[0].toUpperCase() + w.kind.slice(1)}
    >
      <Desktop Body={Body} overlays={<Toasts />} />
      <ConfirmHost />
      <Probe />
    </WindowManagerProvider>,
  );
}

const ids = () => wm.windows.map((w) => w.id);

beforeEach(() => {
  _resetUndoForTests();
  for (const t of getToasts()) dismissToast(t.id);
});

describe("closing", () => {
  it("closes a window without guards at once, with Undo", async () => {
    mount();
    act(() => wm.open("plain"));
    expect(ids()).toEqual(["plain"]);
    let closed = false;
    await act(async () => {
      closed = await wm.requestClose("plain");
    });
    expect(closed).toBe(true);
    expect(ids()).toEqual([]);
    const undo = await screen.findByRole("button", { name: "Undo" });
    expect(screen.getByText("Closed “Plain”", { selector: ".toast-title" })).toBeInTheDocument();
    act(() => fireEvent.click(undo));
    await waitFor(() => expect(ids()).toEqual(["plain"]));
  });

  it("asks before closing a window with unsaved work, listing why", async () => {
    mount();
    act(() => wm.open("files"));
    fireEvent.click(screen.getByText("edit hosts"));
    let result: Promise<boolean>;
    act(() => {
      result = wm.requestClose("files");
    });
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Close “Files”?")).toBeInTheDocument();
    expect(dialog).toHaveTextContent("/etc/hosts has unsaved changes");
    // A sheet on that window, defaulting to Cancel.
    expect(dialog.closest(".win")?.getAttribute("data-win-id")).toBe("files");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await act(async () => {
      expect(await result!).toBe(false);
    });
    expect(ids()).toEqual(["files"]);

    act(() => {
      result = wm.requestClose("files");
    });
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Close anyway" }));
    await act(async () => {
      expect(await result!).toBe(true);
    });
    expect(ids()).toEqual([]);
  });

  it("lets a window keep itself open with the close-request event", async () => {
    mount();
    act(() => wm.open("term"));
    await act(async () => {
      expect(await wm.requestClose("term")).toBe(false);
    });
    expect(screen.getByText("1 tabs")).toBeInTheDocument();
    await act(async () => {
      expect(await wm.requestClose("term")).toBe(true);
    });
    expect(ids()).toEqual([]);
  });

  it("gathers every window's reasons for Close all", async () => {
    mount();
    act(() => {
      wm.open("files");
      wm.open("plain");
    });
    fireEvent.click(screen.getByText("edit hosts"));
    let result: Promise<boolean>;
    act(() => {
      result = wm.requestCloseAll();
    });
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Close all windows?");
    expect(dialog).toHaveTextContent("Files: /etc/hosts has unsaved changes");
    fireEvent.click(within(dialog).getByRole("button", { name: "Close anyway" }));
    await act(async () => {
      expect(await result!).toBe(true);
    });
    expect(ids()).toEqual([]);
    expect(getToasts().some((t) => t.title === "2 windows closed" && t.undo)).toBe(true);
  });

  it("reopens the last closed window with its geometry", async () => {
    mount();
    act(() => wm.open("plain"));
    act(() => wm.move("plain", { x: 140, y: 90 }));
    await act(async () => {
      await wm.requestClose("plain");
    });
    act(() => wm.reopenLast());
    expect(wm.windows[0]).toMatchObject({ id: "plain", x: 140, y: 90 });
  });
});

describe("state that must not be lost", () => {
  it("keeps minimized windows mounted", async () => {
    mount();
    act(() => wm.open("notes"));
    fireEvent.click(screen.getByText("tab: overview"));
    act(() => wm.minimize("notes"));
    await waitFor(() => expect(wm.windows[0].minimized).toBe(true));
    // Still in the document (hidden), with its state.
    expect(screen.getByText("tab: gpu", { selector: "button" })).toBeInTheDocument();
    expect(document.querySelector<HTMLElement>('.win[data-win-id="notes"]')!.style.display).toBe("none");
    act(() => wm.focus("notes"));
    expect(wm.windows[0].minimized).toBe(false);
    expect(screen.getByText("tab: gpu")).toBeVisible();
  });

  it("saves useWinState with the layout and restores it on the next launch", async () => {
    const first = mount();
    act(() => wm.open("notes"));
    fireEvent.click(screen.getByText("tab: overview"));
    await waitFor(() => expect(wm.windows[0].view).toEqual({ tab: "gpu" }), { timeout: 1500 });
    await waitFor(() => expect(localStorage.getItem("sparkbox.windows.v1")).toContain('"tab":"gpu"'), { timeout: 1500 });
    first.unmount();
    mount();
    expect(screen.getByText("tab: gpu")).toBeInTheDocument();
  });

  it("normalizes stacking order when a saved layout has run away", () => {
    localStorage.setItem("sparkbox.windows.v1", JSON.stringify([
      { id: "plain", kind: "plain", params: {}, x: 0, y: 0, w: 600, h: 400, z: 9001, minimized: false, maximized: false },
      { id: "notes", kind: "notes", params: {}, x: 30, y: 30, w: 600, h: 400, z: 7000, minimized: false, maximized: false },
      { id: "gone", kind: "unknown-kind", params: {}, x: 30, y: 30, w: 600, h: 400, z: 2, minimized: false, maximized: false },
    ]));
    mount();
    expect(wm.windows.map((w) => [w.id, w.z])).toEqual([["plain", 2], ["notes", 1]]);
    expect(wm.activeId).toBe("plain");
  });
});

describe("links into open windows", () => {
  beforeAll(() => setViewParams({ notes: ["tab", "section"], linked: ["tab"] }));
  afterAll(() => setViewParams({}));

  it("brings the open window forward on the asked-for view instead of opening a copy", () => {
    mount();
    act(() => wm.open("notes", { id: "cell1" }));
    act(() => wm.open("plain"));
    act(() => wm.open("notes", { id: "cell1", tab: "gpu" }));
    expect(ids()).toEqual(["notes?id=cell1", "plain"]);
    expect(wm.activeId).toBe("notes?id=cell1");
    expect(wm.windows[0].params).toEqual({ id: "cell1", tab: "gpu", nav: "1" });
    // The same link again still counts, so a window can go back to a view it moved away from.
    act(() => wm.open("notes", { id: "cell1", tab: "gpu" }));
    expect(wm.windows[0].params.nav).toBe("2");
    // Plain focus leaves the view alone; another node is another window.
    act(() => wm.open("notes", { id: "cell1" }));
    expect(wm.windows[0].params.nav).toBe("2");
    act(() => wm.open("notes", { id: "cell2", tab: "gpu" }));
    expect(ids()).toEqual(["notes?id=cell1", "plain", "notes?id=cell2"]);
  });

  it("moves the open window to the linked view, even one it was on before", () => {
    mount();
    act(() => wm.open("linked", { tab: "gpu" }));
    expect(screen.getByText("linked: gpu")).toBeInTheDocument();
    fireEvent.click(screen.getByText("linked: gpu"));
    expect(screen.getByText("linked: disks")).toBeInTheDocument();
    act(() => wm.open("linked", { tab: "gpu" }));
    expect(screen.getByText("linked: gpu")).toBeInTheDocument();
    expect(ids()).toEqual(["linked"]);
  });

  it("merges saved windows whose ids still carried a view", () => {
    const win = { x: 0, y: 0, w: 600, h: 400, minimized: false, maximized: false };
    localStorage.setItem("sparkbox.windows.v1", JSON.stringify([
      { ...win, id: "notes?section=nodes", kind: "notes", params: { section: "nodes" }, z: 2 },
      { ...win, id: "notes", kind: "notes", params: {}, z: 1 },
    ]));
    mount();
    expect(wm.windows.map((w) => [w.id, w.params])).toEqual([["notes", { section: "nodes" }]]);
  });
});

describe("arranging", () => {
  it("cycles through visible windows with ⌘`", () => {
    mount();
    act(() => {
      wm.open("plain");
      wm.open("notes");
      wm.open("files");
    });
    expect(wm.activeId).toBe("files");
    act(() => wm.cycle(1));
    expect(wm.activeId).toBe("plain");
    act(() => wm.cycle(-1));
    expect(wm.activeId).toBe("files");
  });

  it("snaps to a half and restores the size it had", () => {
    mount();
    act(() => wm.open("plain"));
    act(() => wm.move("plain", { x: 100, y: 80, w: 500, h: 300 }));
    act(() => wm.snap("plain", "left"));
    expect(wm.windows[0]).toMatchObject({ x: 0, y: 0, snapped: "left" });
    expect(wm.windows[0].h).toBe(wm.bounds.h);
    act(() => wm.snap("plain", "restore"));
    expect(wm.windows[0]).toMatchObject({ x: 100, y: 80, w: 500, h: 300, snapped: null });
  });

  it("tiles every visible window inside the desktop, without overlaps", () => {
    mount();
    act(() => {
      for (const k of ["plain", "notes", "files"]) wm.open(k);
    });
    act(() => wm.tile());
    for (const w of wm.windows) {
      expect(w.x).toBeGreaterThanOrEqual(0);
      expect(w.x + w.w).toBeLessThanOrEqual(wm.bounds.w);
      expect(w.y + w.h).toBeLessThanOrEqual(wm.bounds.h);
      for (const o of wm.windows) {
        if (o === w) continue;
        expect(w.x < o.x + o.w && o.x < w.x + w.w && w.y < o.y + o.h && o.y < w.y + w.h).toBe(false);
      }
    }
  });
});

describe("tiling and the user's own frames", () => {
  const rect = (id: string) => {
    const w = wm.windows.find((x) => x.id === id)!;
    return { x: w.x, y: w.y, w: w.w, h: w.h };
  };

  it("brings windows back at their own frame after Tile, Close all and reopen", async () => {
    mount();
    act(() => {
      wm.open("plain");
      wm.open("notes");
    });
    act(() => wm.move("plain", { x: 140, y: 90, w: 500, h: 300 }));
    act(() => wm.move("notes", { x: 300, y: 200, w: 520, h: 340 }));
    act(() => wm.tile());
    expect(rect("plain")).not.toEqual({ x: 140, y: 90, w: 500, h: 300 });
    expect(wm.windows.find((w) => w.id === "plain")!.userFrame).toEqual({ x: 140, y: 90, w: 500, h: 300 });
    // What's saved is the user's frame, never the tiled one.
    await waitFor(() => {
      const saved = JSON.parse(localStorage.getItem("sparkbox.windows.v1") ?? "[]");
      expect(saved.find((w: { id: string }) => w.id === "plain")).toMatchObject({ x: 140, y: 90, w: 500, h: 300 });
    }, { timeout: 1500 });
    await act(async () => {
      await wm.requestCloseAll();
    });
    expect(ids()).toEqual([]);
    act(() => wm.reopenLast());
    expect(rect("plain")).toEqual({ x: 140, y: 90, w: 500, h: 300 });
    expect(rect("notes")).toEqual({ x: 300, y: 200, w: 520, h: 340 });
    // Opened again later from the sidebar, it remembers the same frame.
    await act(async () => {
      await wm.requestClose("plain");
    });
    act(() => wm.open("plain"));
    expect(rect("plain")).toEqual({ x: 140, y: 90, w: 500, h: 300 });
  });

  it("minimizes what doesn't fit, says so with Undo, and Restore layout puts everything back", async () => {
    mount({ tileMin: [700, 380] });
    act(() => {
      for (const k of KINDS) wm.open(k);
    });
    const before = Object.fromEntries(wm.windows.map((w) => [w.id, rect(w.id)]));
    act(() => wm.tile());
    const tiled = wm.windows.filter((w) => !w.minimized);
    const docked = wm.windows.filter((w) => w.minimized);
    expect(tiled.length).toBeGreaterThan(0);
    expect(docked.length).toBeGreaterThan(0);
    // The most recently focused (the last opened) is kept.
    expect(tiled.map((w) => w.id)).toContain("linked");
    for (const w of tiled) expect(w.w >= 700 && w.h >= 380).toBe(true);
    expect(getToasts().find((t) => t.undo)?.title).toBe(`Tiled ${plural(tiled.length, "window")} · ${docked.length} minimized`);
    expect(wm.canRestoreLayout()).toBe(true);
    // Saved without the arrangement: every window on the desk at its own frame.
    await waitFor(() => {
      const saved = JSON.parse(localStorage.getItem("sparkbox.windows.v1") ?? "[]") as { id: string; minimized: boolean; x: number; y: number; w: number; h: number }[];
      expect(saved.every((w) => !w.minimized && w.w === before[w.id].w && w.x === before[w.id].x)).toBe(true);
    }, { timeout: 1500 });
    act(() => wm.restoreLayout());
    expect(wm.windows.every((w) => !w.minimized)).toBe(true);
    for (const w of wm.windows) expect(rect(w.id)).toEqual(before[w.id]);
    expect(wm.canRestoreLayout()).toBe(false);
  });

  it("undoes what Hide sidebar and tile all changed besides the windows, once", () => {
    mount();
    act(() => {
      for (const k of KINDS) wm.open(k);
    });
    const showSidebar = vi.fn();
    act(() => wm.tile("all", { onRestore: showSidebar }));
    act(() => wm.restoreLayout());
    expect(showSidebar).toHaveBeenCalledTimes(1);
    // A later Tile and Restore leaves the sidebar alone.
    act(() => wm.tile());
    act(() => wm.restoreLayout());
    expect(showSidebar).toHaveBeenCalledTimes(1);
  });

  it("brings windows Tile sent to the dock back on the desk after Close all and reopen", async () => {
    mount({ tileMin: [700, 380] });
    act(() => {
      for (const k of KINDS) wm.open(k);
    });
    const before = Object.fromEntries(wm.windows.map((w) => [w.id, rect(w.id)]));
    act(() => wm.tile());
    expect(wm.windows.some((w) => w.minimized)).toBe(true);
    await act(async () => {
      await wm.requestCloseAll();
    });
    act(() => wm.reopenLast());
    expect(wm.windows).toHaveLength(KINDS.length);
    for (const w of wm.windows) {
      expect(w.minimized).toBe(false);
      expect(rect(w.id)).toEqual(before[w.id]);
    }
  });

  it("tiles the two most recent side by side and docks the rest", () => {
    mount();
    act(() => {
      for (const k of ["plain", "notes", "files"]) wm.open(k);
    });
    act(() => wm.tile("two"));
    const [a, b] = ["notes", "files"].map(rect);
    expect(a.y).toBe(b.y);
    expect(wm.windows.find((w) => w.id === "plain")!.minimized).toBe(true);
  });

  it("forgets the arrangement once a window is moved by hand", () => {
    mount();
    act(() => {
      wm.open("plain");
      wm.open("notes");
    });
    act(() => wm.cascade());
    expect(wm.windows.every((w) => w.arranged)).toBe(true);
    act(() => wm.move("plain", { x: 50, y: 60 }));
    const plain = wm.windows.find((w) => w.id === "plain")!;
    expect(plain.arranged).toBe(false);
    expect(plain.userFrame).toMatchObject({ x: 50, y: 60 });
  });
});

describe("first launch", () => {
  const desk = { w: 1400, h: 860 };
  let restore: () => void = () => {};
  beforeAll(() => {
    const cw = Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth")!;
    const ch = Object.getOwnPropertyDescriptor(Element.prototype, "clientHeight")!;
    Object.defineProperty(Element.prototype, "clientWidth", { configurable: true, get() { return this.classList.contains("desktop") ? desk.w : 0; } });
    Object.defineProperty(Element.prototype, "clientHeight", { configurable: true, get() { return this.classList.contains("desktop") ? desk.h : 0; } });
    restore = () => {
      Object.defineProperty(Element.prototype, "clientWidth", cw);
      Object.defineProperty(Element.prototype, "clientHeight", ch);
    };
  });
  afterAll(() => restore());

  it("clamps a restored window to its minimum size and onto the desktop, 12 px from its edges", async () => {
    const frame = { x: 1300, y: 900, w: 180, h: 90 };
    localStorage.setItem("sparkbox.windows.v1", JSON.stringify([
      { id: "plain", kind: "plain", params: {}, ...frame, userFrame: frame, z: 1, minimized: false, maximized: false },
    ]));
    mount();
    await waitFor(() => expect(wm.bounds).toEqual(desk));
    await waitFor(() => expect(wm.windows[0]).toMatchObject({ w: 360, h: 200, x: desk.w - 360 - 12, y: desk.h - 200 - 12 }));
  });

  it("gives a window saved before user frames (maybe while tiled) its default size back", async () => {
    localStorage.setItem("sparkbox.windows.v1", JSON.stringify([
      { id: "plain", kind: "plain", params: {}, x: 40, y: 30, w: 380, h: 210, z: 1, minimized: false, maximized: false },
    ]));
    mount();
    await waitFor(() => expect(wm.windows[0]).toMatchObject({ x: 40, y: 30, w: 600, h: 400, userFrame: { x: 40, y: 30, w: 600, h: 400 } }));
  });

  it("puts focus on the window frame, not a control in it, and shows no focus ring", async () => {
    const stop = (await import("../lib/modality")).installModality();
    mount();
    act(() => wm.open("notes"));
    const frame = document.querySelector<HTMLElement>('.win[data-win-id="notes"]')!;
    await waitFor(() => expect(document.activeElement).toBe(frame));
    expect(document.documentElement.dataset.modality).toBe("pointer");
    stop();
  });
  it("tiles again when the desktop changes size, until a window is moved by hand", async () => {
    const callbacks: (() => void)[] = [];
    const Real = window.ResizeObserver;
    window.ResizeObserver = class {
      constructor(private cb: ResizeObserverCallback) {
        callbacks.push(() => this.cb([], this as unknown as ResizeObserver));
      }
      observe() {
        queueMicrotask(() => callbacks.forEach((c) => c()));
      }
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
    try {
      mount();
      await waitFor(() => expect(wm.bounds).toEqual(desk));
      act(() => {
        wm.open("plain");
        wm.open("notes");
      });
      act(() => wm.tile());
      const right = () => Math.max(...wm.windows.map((w) => w.x + w.w));
      expect(right()).toBe(desk.w - 8);
      // The sidebar comes back: 220 px less desktop.
      desk.w = 1180;
      act(() => callbacks.forEach((c) => c()));
      await waitFor(() => expect(right()).toBe(1180 - 8));
      // Moved by hand: it's the user's layout now, and stays put.
      act(() => wm.move("plain", { x: 30, y: 40 }));
      desk.w = 1400;
      act(() => callbacks.forEach((c) => c()));
      await waitFor(() => expect(wm.bounds.w).toBe(1400));
      expect(wm.windows.find((w) => w.id === "plain")).toMatchObject({ x: 30, y: 40 });
    } finally {
      window.ResizeObserver = Real;
      desk.w = 1400;
    }
  });
});

describe("a desk that changes size (C4)", () => {
  const desk = { w: 1440, h: 900 };
  let restore: () => void = () => {};
  beforeAll(() => {
    const cw = Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth")!;
    const ch = Object.getOwnPropertyDescriptor(Element.prototype, "clientHeight")!;
    Object.defineProperty(Element.prototype, "clientWidth", { configurable: true, get() { return this.classList.contains("desktop") ? desk.w : 0; } });
    Object.defineProperty(Element.prototype, "clientHeight", { configurable: true, get() { return this.classList.contains("desktop") ? desk.h : 0; } });
    restore = () => {
      Object.defineProperty(Element.prototype, "clientWidth", cw);
      Object.defineProperty(Element.prototype, "clientHeight", ch);
    };
  });
  afterAll(() => restore());

  it("brings a window back onto a desk that shrank, and back to its own frame when it grows", async () => {
    const callbacks: (() => void)[] = [];
    const Real = window.ResizeObserver;
    window.ResizeObserver = class {
      constructor(private cb: ResizeObserverCallback) {
        callbacks.push(() => this.cb([], this as unknown as ResizeObserver));
      }
      observe() {
        queueMicrotask(() => callbacks.forEach((c) => c()));
      }
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
    try {
      mount();
      await waitFor(() => expect(wm.bounds).toEqual(desk));
      act(() => wm.open("plain"));
      act(() => wm.move("plain", { x: 700, y: 300 }));
      const own = { ...wm.windows[0] };
      // The app window gets narrow: the window moves in (and narrows only if it must).
      desk.w = 800;
      act(() => callbacks.forEach((c) => c()));
      await waitFor(() => expect(wm.bounds.w).toBe(800));
      const w = wm.windows[0];
      expect(w.x + w.w).toBeLessThanOrEqual(800 - 12);
      expect(w.x).toBeGreaterThanOrEqual(0);
      // Wide again: it's back where it was put.
      desk.w = 1440;
      act(() => callbacks.forEach((c) => c()));
      await waitFor(() => expect(wm.windows[0]).toMatchObject({ x: own.x, y: own.y, w: own.w, h: own.h }));
    } finally {
      window.ResizeObserver = Real;
      desk.w = 1440;
    }
  });
});

describe("in front, and cascaded", () => {
  it("brings a window opened from ⌘K to the front with focus, and doesn't hand focus back to the window behind", { timeout: 15_000 }, async () => {
    const { CommandPalette } = await import("../components/CommandPalette");
    const { registerCommands } = await import("../lib/commands");
    const { openPanel } = await import("../lib/session");
    render(
      <WindowManagerProvider sizeOf={() => [600, 400]} isKnown={(k) => KINDS.includes(k)} titleOf={(w) => w.kind}>
        <Desktop Body={Body} />
        <CommandPalette />
        <Probe />
      </WindowManagerProvider>,
    );
    act(() => wm.open("notes"));
    // ⌘K pressed while a control in Notes had focus.
    const inNotes = screen.getByText("tab: overview");
    inNotes.focus();
    const off = registerCommands([
      { id: "go:linked", title: "Open Linked", group: "Windows", run: () => wm.open("linked") },
      { id: "go:notes", title: "Focus Notes", group: "Open windows", run: () => wm.focus("notes") },
    ]);
    try {
      act(() => openPanel("palette"));
      const input = await screen.findByRole("combobox");
      fireEvent.change(input, { target: { value: "Open Linked" } });
      fireEvent.keyDown(input, { key: "Enter" });
      // Under a loaded test run the deferred command and the frame's focus take a few frames.
      const slow = { timeout: 4000 };
      await waitFor(() => expect(wm.activeId).toBe("linked"), slow);
      const frame = document.querySelector('.win[data-win-id="linked"]')!;
      await waitFor(() => expect(frame.contains(document.activeElement)).toBe(true), slow);
      // A moment later it's still in front: nothing raised Notes again.
      await act(() => new Promise((r) => setTimeout(r, 250)));
      expect(wm.activeId).toBe("linked");
      expect(frame.classList.contains("front")).toBe(true);

      // Focusing a window that's behind works the same way.
      act(() => openPanel("palette"));
      const again = await screen.findByRole("combobox", {}, slow);
      fireEvent.change(again, { target: { value: "Focus Notes" } });
      fireEvent.keyDown(again, { key: "Enter" });
      await waitFor(() => expect(wm.activeId).toBe("notes"), slow);
      await waitFor(() => expect(document.querySelector('.win[data-win-id="notes"]')!.contains(document.activeElement)).toBe(true), slow);
    } finally {
      off();
    }
  });

  it("opens six kinds saved at one origin at six different origins", () => {
    const six = ["notes", "files", "term", "plain", "linked", "extra"];
    const frame = { x: 232, y: 52, w: 600, h: 400 };
    localStorage.setItem("sparkbox.wingeom.v2", JSON.stringify(Object.fromEntries(six.map((k) => [k, frame]))));
    render(
      <WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true} titleOf={(w) => w.kind}>
        <Desktop Body={Body} />
        <Probe />
      </WindowManagerProvider>,
    );
    for (const k of six) act(() => wm.open(k));
    const origins = wm.windows.map((w) => `${w.x},${w.y}`);
    expect(new Set(origins).size).toBe(6);
    // Each keeps the size it was saved at, and the first stays where it was.
    expect(wm.windows.every((w) => w.w === 600 && w.h === 400)).toBe(true);
    expect(wm.windows[0]).toMatchObject({ x: 232, y: 52 });
  });

  it("gives a Tile clicked twice one toast", () => {
    // Big tile minimums on the test desk: not all five fit, so Tile docks some and says so.
    mount({ tileMin: [500, 500] });
    act(() => {
      for (const k of KINDS) wm.open(k);
    });
    act(() => wm.tile());
    // Everything back, and Tile again at once: the second arrangement docks windows too.
    act(() => wm.restoreLayout());
    expect(wm.windows.every((w) => !w.minimized)).toBe(true);
    act(() => wm.tile());
    expect(wm.windows.some((w) => w.minimized)).toBe(true);
    const tiled = getToasts().filter((t) => t.title.startsWith("Tiled "));
    expect(tiled).toHaveLength(1);
    expect(tiled[0].undo).toBeTruthy();
  });

  it("steps a restored layout off a window already on that spot", () => {
    mount();
    act(() => wm.open("plain"));
    const at = wm.windows[0];
    act(() => wm.restoreWindows([{ id: "notes", kind: "notes", params: {}, x: at.x + 3, y: at.y, w: 600, h: 400, z: 1, minimized: false, maximized: false, userFrame: { x: at.x + 3, y: at.y, w: 600, h: 400 } }]));
    const notes = wm.windows.find((w) => w.id === "notes")!;
    expect(Math.abs(notes.x - at.x) > 8 || Math.abs(notes.y - at.y) > 8).toBe(true);
  });
});

describe("titles", () => {
  it("lets a window override its title, and the native window title follows the window in front", async () => {
    const { mockCalls } = await import("../test/tauri");
    function Editor() {
      const [dirty, setDirty] = useState(false);
      useWindowTitle(dirty ? "● hosts — cell1" : null);
      return <button onClick={() => setDirty(true)}>type</button>;
    }
    render(
      <WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true} titleOf={() => "Editor"}>
        <Desktop Body={Editor} />
        <Probe />
      </WindowManagerProvider>,
    );
    act(() => wm.open("editor"));
    expect(document.querySelector(".win-title-text")).toHaveTextContent("Editor");
    fireEvent.click(screen.getByText("type"));
    expect(document.querySelector(".win-title-text")).toHaveTextContent("● hosts — cell1");
    await waitFor(() => {
      const titles = mockCalls.filter((c) => c.cmd === "plugin:window|set_title").map((c) => c.args.value);
      expect(titles).toContain("SparkBox — ● hosts — cell1");
    });
  });
});
