// Files › New file: on a node with Live data and on this Mac it's there (locked until Admin, in
// look-only held); a Demo node has no simulation of it, so it says "Available with Live data".
import { act, fireEvent, render, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests } from "../lib/admin";
import type { AdminState } from "../lib/opsApi";
import { resetStore, startClusterStore } from "../lib/store";
import { SECRET_REFUSED } from "../lib/transfers";
import type { ClusterSnapshot } from "../lib/types";
import { mockCalls, mockTauri, type MockHandlers } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { FilesWindow } from "./FilesWindow";

const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };

async function show(place: string, demo: boolean, admin: AdminState = UNLOCKED, more: MockHandlers = {}) {
  resetStore();
  const snap = { ...(structuredClone(baseSnapshot) as ClusterSnapshot), tsMs: Date.now(), demo };
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => snap, admin_state: () => admin }, more));
  _resetAdminForTests(admin);
  await startClusterStore();
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1200, 800]} isKnown={() => true}><FilesWindow params={{ place }} /></WindowManagerProvider>);
  });
  return r.container;
}

const newFile = (root: HTMLElement) => [...root.querySelectorAll<HTMLButtonElement>("[role=toolbar] button")].find((b) => b.getAttribute("aria-label")?.startsWith("New file"))!;

describe("New file", () => {
  it("is there on a node with Live data and on this Mac, and says it needs Live data on a Demo node", async () => {
    const node = baseSnapshot.nodes[0].id;
    expect([newFile(await show(node, false)).disabled, newFile(await show(node, false)).title]).toEqual([false, "New file, opened in an editor"]);
    expect([newFile(await show(node, true)).disabled, newFile(await show(node, true)).title]).toEqual([true, "Available with Live data"]);
    // This Mac's files are real in Demo too: its pane (the left one here) offers it.
    const root = await show("mac", true);
    await act(async () => {
      fireEvent.pointerDown(root.querySelector('section.sb-pane[data-pane="0"]')!, { button: 0 });
    });
    expect(root.querySelector('section.sb-pane[data-pane="0"]')!.className).toContain("active");
    expect(newFile(root).disabled).toBe(false);
  });

  it("is held in look-only", async () => {
    const b = newFile(await show(baseSnapshot.nodes[0].id, false, { ...UNLOCKED, unlocked: false, lookOnly: true }));
    expect([b.disabled, b.getAttribute("aria-label")]).toEqual([true, "New file — look-only"]);
  });
});

describe("Copy and Move of keys or credentials", () => {
  const entry = (name: string) => ({ name, kind: "dir", size: 0, mtime: 1, mode: "drwx------", target: null, linkDir: false });
  const tool = (root: HTMLElement, label: string) => [...root.querySelectorAll<HTMLButtonElement>("[role=toolbar] button")].find((b) => b.getAttribute("aria-label")?.startsWith(label))!;

  // ~ on this Mac: Library holds the Keychains, so a copy of it takes them along.
  const home = (a: { place: string }) => ({ place: a.place, path: "/Users/me", entries: [entry("Library"), entry("Documents")], free: null, capacity: null });

  async function pick(admin: AdminState, name: string) {
    const root = await show("mac", true, admin, { files_list: home });
    const pane = root.querySelector<HTMLElement>('section.sb-pane[data-pane="0"]')!;
    await act(async () => {
      fireEvent.pointerDown(pane, { button: 0 });
      fireEvent.mouseDown(within(pane).getByText(name));
    });
    return root;
  }

  it("are off with the reason as their tooltip, whatever Admin says, and nothing starts", async () => {
    for (const admin of [UNLOCKED, { ...UNLOCKED, unlocked: false }]) {
      const root = await pick(admin, "Library");
      for (const label of ["Copy to", "Move to"]) {
        const b = tool(root, label);
        expect([b.disabled, b.title, b.hasAttribute("data-lock")], label).toEqual([true, SECRET_REFUSED, false]);
      }
    }
    expect(mockCalls.filter((c) => c.cmd === "admin_authenticate" || c.cmd === "files_transfer" || c.cmd === "files_preflight")).toEqual([]);
  });

  it("an ordinary folder beside it stays a normal copy", async () => {
    const b = tool(await pick(UNLOCKED, "Documents"), "Copy to");
    expect(b.disabled).toBe(false);
    expect(b.title).not.toBe(SECRET_REFUSED);
  });
});

