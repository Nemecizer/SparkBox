// The Editor's Save: it unlocks for the file's own scope (a save on this Mac needs Touch ID or
// the password within 5 minutes, whatever node scope says), and the status line reads the file.
import { EditorState } from "@codemirror/state";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { _resetAdminForTests } from "../lib/admin";
import { bufferKey, closeBuffer, getBuffer, setBufferState } from "../lib/editorBuffers";
import { MAC } from "../lib/filesApi";
import type { AdminState } from "../lib/opsApi";
import type { ConfirmOptions } from "../lib/session";
import { mockCalls, mockTauri, type MockHandlers } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { EditorWindow } from "./EditorWindow";

const asked: ConfirmOptions[] = [];
let answer = true;
vi.mock("../lib/session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/session")>()),
  confirmAction: vi.fn(async (o: ConfirmOptions) => (asked.push(o), answer)),
}));

const PATH = "/Users/me/notes.txt";
const file = (content: string) => ({ path: PATH, content, size: content.length, mtime: 1, sha256: "aa", mode: "-rw-r--r--", owner: "me:staff", writable: true, large: false, head: null, tail: null });
const NODE_ONLY: AdminState = { unlocked: true, expiresAtMs: Date.now() + 600_000, idleMinutes: 15, requiresAuth: true, authenticated: false, lookOnly: false };
const BOTH: AdminState = { ...NODE_ONLY, authenticated: true, macExpiresAtMs: Date.now() + 300_000 };

async function open(content: string, admin: AdminState, more: MockHandlers = {}, place = MAC) {
  mockTauri({
    files_read: () => file(content), files_backups: () => [], files_write: () => ({ path: PATH, sha256: "bb", mtime: 2, size: 1, backup: null }),
    admin_state: () => admin, admin_authenticate: () => BOTH, get_snapshot: () => null, ...more,
  });
  _resetAdminForTests(admin);
  await act(async () => {
    render(<WindowManagerProvider sizeOf={() => [800, 600]} isKnown={() => true}><EditorWindow params={{ place, path: PATH }} /></WindowManagerProvider>);
  });
}

const adminCalls = () => mockCalls.filter((c) => c.cmd.startsWith("admin_auth") || c.cmd === "admin_unlock").map((c) => c.cmd);
const writes = () => mockCalls.filter((c) => c.cmd === "files_write").map((c) => c.args.content);

async function editAndSave(place = MAC) {
  const key = bufferKey(place, PATH);
  await act(async () => {
    setBufferState(key, EditorState.create({ doc: "edited\n" }), true);
  });
  const save = await screen.findByRole("button", { name: /^Save/ });
  await act(async () => {
    fireEvent.click(save);
  });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
}

beforeEach(() => {
  asked.length = 0;
  answer = true;
  closeBuffer(bufferKey(MAC, PATH));
  closeBuffer(bufferKey("cell2", PATH));
});

describe("Save", () => {
  it("with node scope unlocked and Mac scope lapsed, a Mac save asks for Touch ID, then saves", async () => {
    await open("a\n", NODE_ONLY);
    await editAndSave();
    expect(asked.map((a) => a.title)).toEqual(["Unlock Admin mode?"]);
    expect(adminCalls()).toEqual(["admin_authenticate"]);
    expect(mockCalls.find((c) => c.cmd === "admin_authenticate")?.args.action, "the OS prompt's words").toBe("files_mac");
    expect(writes()).toEqual(["edited\n"]);
  });

  it("cancelling the sheet saves nothing and asks macOS nothing", async () => {
    answer = false;
    await open("a\n", NODE_ONLY);
    await editAndSave();
    expect(adminCalls()).toEqual([]);
    expect(writes()).toEqual([]);
  });

  it("Mac scope still good: no prompt", async () => {
    await open("a\n", BOTH);
    await editAndSave();
    expect(asked).toEqual([]);
    expect(adminCalls()).toEqual([]);
    expect(writes()).toEqual(["edited\n"]);
  });

  it("a node's file needs only node scope", async () => {
    await open("a\n", NODE_ONLY, {}, "cell2");
    await editAndSave("cell2");
    expect(adminCalls()).toEqual([]);
    expect(writes()).toEqual(["edited\n"]);
  });
});

describe("the status line", () => {
  it("shows the file's line endings, and a CRLF file opens with no changes", async () => {
    await open("a\r\nb\r\n", BOTH);
    expect(await screen.findByText("UTF-8 · CRLF")).toBeInTheDocument();
    expect(screen.getByText("No changes")).toBeInTheDocument();
  });

  it("mixed endings say what saving does", async () => {
    await open("a\r\nb\n", BOTH);
    expect(await screen.findByText("UTF-8 · Mixed line endings")).toHaveAttribute("title", "Saving makes them all LF");
  });
});


it("a failure-details editor selects its script line and cannot save even a dirty buffer", async () => {
  mockTauri({ files_read: () => file("first\nsecond\nthird\n"), files_backups: () => [] }); _resetAdminForTests(BOTH);
  await act(async () => { render(<WindowManagerProvider sizeOf={() => [800, 600]} isKnown={() => true}><EditorWindow params={{ place: "cell2", path: PATH, readOnly: "1", line: "2" }} /></WindowManagerProvider>); });
  expect(screen.getByText("Opened read-only from the failure details.")).toBeInTheDocument();
  await act(async () => { await new Promise<void>((resolve) => requestAnimationFrame(() => resolve())); });
  const state = getBuffer(bufferKey("cell2", PATH))?.state;
  expect(state?.sliceDoc(state.selection.main.from, state.selection.main.to)).toBe("second");
  await act(async () => { setBufferState(bufferKey("cell2", PATH), EditorState.create({ doc: "changed" }), true); });
  const save = screen.getByRole("button", { name: /^Save/ }); expect(save).toBeDisabled();
  fireEvent.click(save); expect(writes()).toEqual([]); expect(adminCalls()).toEqual([]);
});
