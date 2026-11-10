import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { registerCommands, setAdminHooks } from "../lib/commands";
import { menuAt, showContextMenu } from "../lib/contextMenu";
import { openPanel } from "../lib/session";
import { CommandPalette } from "./CommandPalette";
import { ContextMenuHost } from "./ContextMenuHost";

describe("context menus", () => {
  function Target({ onRestart, onOpen }: { onRestart: () => void; onOpen: () => void }) {
    return (
      <button
        onContextMenu={(e) => showContextMenu(e, [
          { label: "Open", run: onOpen },
          { label: "Restart", shortcut: "⌘R", run: onRestart },
          { separator: true },
          { label: "Remove", disabled: "Stop it first", danger: true, run: () => {} },
          { label: "Move to", submenu: [{ label: "cell2", run: () => {} }] },
        ])}
      >
        vllm-head
      </button>
    );
  }

  it("opens at the pointer and runs an item with the keyboard", () => {
    const onRestart = vi.fn();
    render(<><Target onRestart={onRestart} onOpen={() => {}} /><ContextMenuHost /></>);
    fireEvent.contextMenu(screen.getByText("vllm-head"), { clientX: 40, clientY: 50 });
    const menu = screen.getByRole("menu");
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Open", "Restart⌘R", "RemoveStop it first", "Move to"]);
    // Disabled rows say why, and are skipped by the arrows.
    expect(screen.getByText("Stop it first")).toBeInTheDocument();
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    fireEvent.keyDown(menu, { key: "Enter" });
    expect(onRestart).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("jumps by type-ahead, opens submenus with →, and closes with Esc", () => {
    render(<><Target onRestart={() => {}} onOpen={() => {}} /><ContextMenuHost /></>);
    fireEvent.contextMenu(screen.getByText("vllm-head"));
    const menu = screen.getByRole("menu");
    fireEvent.keyDown(menu, { key: "m" });
    expect(screen.getByText("Move to").closest("[role=menuitem]")).toHaveClass("hi");
    fireEvent.keyDown(menu, { key: "ArrowRight" });
    expect(screen.getAllByRole("menu")).toHaveLength(2);
    expect(screen.getByText("cell2")).toBeInTheDocument();
    fireEvent.keyDown(screen.getAllByRole("menu")[1], { key: "Escape" });
    expect(screen.getAllByRole("menu")).toHaveLength(1);
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("opens the focused element's menu with Shift+F10", async () => {
    const { openMenuForFocused } = await import("../lib/contextMenu");
    render(<><Target onRestart={() => {}} onOpen={() => {}} /><ContextMenuHost /></>);
    screen.getByText("vllm-head").focus();
    act(() => {
      expect(openMenuForFocused()).toBe(true);
    });
    // From the keyboard, the first item starts highlighted.
    expect(screen.getByText("Open").closest("[role=menuitem]")).toHaveClass("hi");
  });
});

describe("context menus and the lock rule (P1.43)", () => {
  const items = (run: () => void) => [
    { label: "Stop (TERM)…", locked: true, run },
    { label: "Reload", hint: "Admin", run },
    { label: "Copy PID", run },
    { label: "Send signal", submenu: [{ label: "HUP…", locked: true, run }] },
  ];

  it("locked: a trailing lock, the tooltip, and the click goes through the unlock sheet first", async () => {
    const { _resetAdminForTests, NEEDS_ADMIN_TIP } = await import("../lib/admin");
    const session = await import("../lib/session");
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: false });
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const run = vi.fn();
    render(<><button onContextMenu={(e) => showContextMenu(e, items(run))}>pid 4242</button><ContextMenuHost /></>);
    fireEvent.contextMenu(screen.getByText("pid 4242"));
    const stop = screen.getByRole("menuitem", { name: /Stop \(TERM\)/ });
    expect(stop).toHaveAttribute("title", NEEDS_ADMIN_TIP);
    expect(stop.querySelector(".ctx-lock")).not.toBeNull();
    const reload = screen.getByRole("menuitem", { name: /Reload/ });
    expect(reload.querySelector(".ctx-lock"), "the older Admin tag reads the same way").not.toBeNull();
    expect(reload.querySelector(".ctx-hint")).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Copy PID" }).querySelector(".ctx-lock")).toBeNull();
    await act(async () => {
      fireEvent.click(stop);
    });
    expect(asked).toHaveBeenCalledOnce();
    expect(asked.mock.calls[0][0]).toMatchObject({ title: "Unlock Admin mode?" });
    expect(run, "a cancelled unlock runs nothing").not.toHaveBeenCalled();
  });

  it("look-only: locked items are disabled (the arrows pass them by) and say how to turn it off", async () => {
    const { _resetAdminForTests, LOOK_ONLY_TIP } = await import("../lib/admin");
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: true });
    const run = vi.fn();
    render(<><button onContextMenu={(e) => showContextMenu(e, items(run))}>pid 4242</button><ContextMenuHost /></>);
    fireEvent.contextMenu(screen.getByText("pid 4242"));
    const menu = screen.getByRole("menu");
    for (const name of [/Stop \(TERM\)/, /Reload/]) {
      const item = screen.getByRole("menuitem", { name });
      expect(item).toHaveAttribute("aria-disabled", "true");
      expect(item).toHaveAttribute("title", LOOK_ONLY_TIP);
    }
    fireEvent.click(screen.getByRole("menuitem", { name: /Stop \(TERM\)/ }));
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    fireEvent.keyDown(menu, { key: "Enter" });
    expect(run, "Enter lands on Copy PID, the first item that can run").toHaveBeenCalledTimes(1);
    expect(document.body.textContent).not.toMatch(/unlock/i);
    _resetAdminForTests();
  });
});

describe("command palette", () => {
  it("is a combobox over a listbox, runs the top match with Return and explains unavailable rows", async () => {
    const run = vi.fn();
    const off = registerCommands([
      { id: "t:tile", title: "Tile windows", group: "Window", run },
      { id: "t:minimize", title: "Minimize window", group: "Window", when: () => "No window is open", run: () => {} },
      { id: "t:reboot", title: "Reboot cell3", group: "Nodes", needsAdmin: true, run: () => {} },
    ]);
    setAdminHooks({ isAdmin: () => false, unlock: async () => false });
    render(<CommandPalette />);
    act(() => openPanel("palette"));
    const box = screen.getByRole("combobox");
    expect(box).toHaveFocus();
    expect(box).toHaveAttribute("aria-controls", "palette-list");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    // Needs Admin mode: a lock.
    expect(screen.getByLabelText("Needs Admin mode")).toBeInTheDocument();
    fireEvent.change(box, { target: { value: "min" } });
    const row = screen.getByRole("option", { name: /Minimize window/ });
    expect(row).toHaveAttribute("aria-disabled", "true");
    expect(row).toHaveTextContent("No window is open");
    fireEvent.change(box, { target: { value: "tile" } });
    expect(box.getAttribute("aria-activedescendant")).toBe("palette-row-0");
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    // The palette closes first; the command runs once it's gone.
    expect(screen.queryByRole("combobox")).toBeNull();
    await waitFor(() => expect(run).toHaveBeenCalledOnce());
    off();
  });

  it("finds a planned launcher, and closes on Escape wherever focus went (C4)", () => {
    const off = registerCommands([
      { id: "planned:launch-trtllm", title: "TensorRT-LLM (coming soon)", group: "Coming soon", keywords: ["planned", "upcoming"], run: () => {} },
      { id: "t:tile", title: "Tile windows", group: "Window", run: () => {} },
    ]);
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    render(<CommandPalette />);
    act(() => openPanel("palette"));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "TensorRT" } });
    expect(screen.getByRole("option", { name: /TensorRT-LLM \(coming soon\)/ })).toBeInTheDocument();
    // Focus wandered out of the palette (a click on a window behind): Escape still closes it.
    outside.focus();
    act(() => {
      fireEvent.keyDown(outside, { key: "Escape" });
    });
    expect(screen.queryByRole("combobox")).toBeNull();
    outside.remove();
    off();
  });
});

describe("keyboard shortcuts sheet", () => {
  it("lists the global bindings, and apart from them what the window in front does with the same keys", async () => {
    const { ShortcutsHelp } = await import("./ShortcutsHelp");
    const { Desktop, WindowManagerProvider, openWindow } = await import("../wm/WindowManager");
    const { closePanel } = await import("../lib/session");
    const off = registerCommands([{ id: "term:new-tab", title: "New terminal tab", group: "App", run: () => {} }]);
    const offScoped = registerCommands([{ id: "term:new-tab", title: "Shell in vllm-head", group: "Containers", run: () => {} }], "containers");
    render(
      <WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true} titleOf={() => "Containers"}>
        <Desktop Body={() => <span>body</span>} />
        <ShortcutsHelp />
      </WindowManagerProvider>,
    );
    act(() => openWindow("containers"));
    act(() => openPanel("shortcuts"));
    const front = await screen.findByRole("region", { name: "In Containers" });
    expect(front).toHaveTextContent("Shell in vllm-head (instead of New Terminal Tab)⌘T");
    // The File menu keeps the global name for the same keys.
    const file = screen.getByRole("heading", { name: "File" }).closest("section")!;
    expect(file).toHaveTextContent("New Terminal Tab⌘T");
    act(() => closePanel());
    off();
    offScoped();
  });

  it("hangs a button's menu under the button, even for a click with no pointer position (C4)", () => {
    const rect = { left: 600, right: 680, top: 100, bottom: 124, width: 80, height: 24, x: 600, y: 100, toJSON: () => ({}) };
    function More() {
      return (
        <button
          onClick={(e) => showContextMenu(e, [{ label: "Kill", run: () => {} }, { label: "Pause", run: () => {} }])}
          ref={(el) => {
            if (el) el.getBoundingClientRect = () => rect as DOMRect;
          }}
        >
          More
        </button>
      );
    }
    render(<><More /><ContextMenuHost /></>);
    // A synthetic click at (0, 0), as Containers › More sent from the keyboard (shot 17c).
    fireEvent.click(screen.getByText("More"), { clientX: 0, clientY: 0, detail: 0 });
    const menu = screen.getByRole("menu");
    const left = parseFloat(menu.style.left);
    const top = parseFloat(menu.style.top);
    expect(Math.abs(left - rect.left)).toBeLessThanOrEqual(4);
    expect(Math.abs(top - rect.bottom)).toBeLessThanOrEqual(4);
    // Opened from the keyboard: the first item is highlighted, and Escape closes it.
    expect(screen.getByText("Kill").closest("[role=menuitem]")).toHaveClass("hi");
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("opens menuAt under its anchor and gives focus back to it", () => {
    const anchor = document.createElement("button");
    document.body.appendChild(anchor);
    anchor.getBoundingClientRect = () => ({ left: 40, right: 140, top: 10, bottom: 34, width: 100, height: 24, x: 40, y: 10, toJSON: () => ({}) }) as DOMRect;
    render(<ContextMenuHost />);
    act(() => menuAt(anchor, [{ label: "Tile all", run: () => {} }]));
    const menu = screen.getByRole("menu");
    expect(parseFloat(menu.style.left)).toBe(40);
    expect(parseFloat(menu.style.top)).toBe(36);
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(document.activeElement).toBe(anchor);
    anchor.remove();
  });
});
