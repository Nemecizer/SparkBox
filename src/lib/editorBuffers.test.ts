// The editor's buffers: a file's line endings survive a save, and a save on this Mac asks for
// its own scope. The backend is mockTauri, with its real error shapes: the gate's refusal reaches
// a save as a Failed whose message is the refusal's JSON (files/mod.rs From<String> for SaveError).
import { EditorState } from "@codemirror/state";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mockCalls, mockTauri, type MockHandlers } from "../test/tauri";
import { _resetAdminForTests } from "./admin";
import {
  bufferKey, bufferText, closeBuffer, encodingWords, ensureBuffer, eolOf, getBuffer, loadDiskText, reloadBuffer, replaceBufferText, restoreBackup, saveBuffer, setBufferState,
} from "./editorBuffers";
import { MAC, type TextFile } from "./filesApi";
import type { AdminState } from "./opsApi";

const flush = () => new Promise((r) => setTimeout(r, 0));
const file = (content: string, extra: Partial<TextFile> = {}): TextFile => ({
  path: "/Users/me/notes.txt", content, size: content.length, mtime: 1, sha256: "aa", mode: "-rw-r--r--", owner: "me:staff", writable: true, large: false, head: null, tail: null, ...extra,
});
const saved = (content: string) => ({ path: "/Users/me/notes.txt", sha256: "bb", mtime: 2, size: content.length, backup: null });
const written = () => mockCalls.filter((c) => c.cmd === "files_write").map((c) => c.args.content as string);
const MAC_LAPSED: AdminState = { unlocked: true, expiresAtMs: Date.now() + 600_000, idleMinutes: 15, requiresAuth: true, authenticated: false, lookOnly: false };
const NEEDS_AUTH = JSON.stringify({ kind: "needs_auth", message: "Confirm it's you with Touch ID or your Mac password to change settings on this Mac." });
const key = bufferKey(MAC, "/Users/me/notes.txt");

async function open(content: string, more: MockHandlers = {}) {
  mockTauri({ files_read: () => file(content), files_backups: () => [], files_write: (a) => saved(a.content), ...more });
  ensureBuffer(MAC, "/Users/me/notes.txt");
  await flush();
  return getBuffer(key)!;
}

/** What CodeMirror does with the text: it joins with \n whatever it was given. */
function type(text: string) {
  setBufferState(key, EditorState.create({ doc: text }), true);
}

beforeEach(() => {
  closeBuffer(key);
  _resetAdminForTests({ ...MAC_LAPSED, authenticated: true, macExpiresAtMs: Date.now() + 300_000 });
});
afterEach(() => _resetAdminForTests());

describe("eolOf", () => {
  it("names each ending, and a mix", () => {
    const cases: [string, string][] = [
      ["a\nb\n", "LF"], ["a\r\nb\r\n", "CRLF"], ["a\rb\r", "CR"], ["a\r\nb\nc", "Mixed"], ["a\rb\nc", "Mixed"], ["a\r\nb\rc", "Mixed"], ["one line", "LF"], ["", "LF"],
    ];
    for (const [text, want] of cases) expect(eolOf(text), JSON.stringify(text)).toBe(want);
  });
});

describe("the status line", () => {
  it("reads what the file is", async () => {
    expect(encodingWords(await open("a\nb\n"))).toEqual({ text: "UTF-8 · LF" });
    closeBuffer(key);
    expect(encodingWords(await open("a\r\nb\r\n"))).toEqual({ text: "UTF-8 · CRLF" });
    closeBuffer(key);
    expect(encodingWords(await open("a\rb\r"))).toEqual({ text: "UTF-8 · CR" });
    closeBuffer(key);
    expect(encodingWords(await open("a\r\nb\n"))).toEqual({ text: "UTF-8 · Mixed line endings", title: "Saving makes them all LF" });
    closeBuffer(key);
    expect(encodingWords(await open("﻿a\r\nb\r\n"))).toEqual({ text: "UTF-8 with BOM · CRLF" });
  });

  it("reads a large file from its head, even when the cut falls inside a CRLF", async () => {
    mockTauri({ files_read: () => file("", { large: true, head: "a\r\nb\r\nc\r", tail: "z\r\n" }), files_backups: () => [] });
    ensureBuffer(MAC, "/Users/me/notes.txt");
    await flush();
    expect(encodingWords(getBuffer(key)!).text).toBe("UTF-8 · CRLF");
  });
});

describe("line endings through a save", () => {
  it("a CRLF file opens clean, against CodeMirror's own LF text", async () => {
    const b = await open("a\r\nb\r\n");
    expect(b.dirty).toBe(false);
    expect(b.baseText).toBe("a\nb\n");
    expect(EditorState.create({ doc: b.text }).doc.toString(), "what the editor holds").toBe(b.baseText);
  });

  it("open, edit and save a CRLF file: the bytes are CRLF, and it isn't dirty after", async () => {
    await open("a\r\nb\r\n");
    type("a\nb\nc\n");
    expect(await saveBuffer(key, { sudo: false })).toBe("saved");
    expect(written()).toEqual(["a\r\nb\r\nc\r\n"]);
    const b = getBuffer(key)!;
    expect([b.dirty, b.eol, b.baseText, bufferText(b)]).toEqual([false, "CRLF", "a\nb\nc\n", "a\nb\nc\n"]);
  });

  it("LF stays LF", async () => {
    await open("a\nb\n");
    type("x\ny\n");
    await saveBuffer(key, { sudo: false });
    expect(written()).toEqual(["x\ny\n"]);
  });

  it("CR stays CR", async () => {
    await open("a\rb\r");
    type("x\ny\n");
    await saveBuffer(key, { sudo: false });
    expect(written()).toEqual(["x\ry\r"]);
  });

  it("Mixed saves as LF, as its tooltip says, and then reads LF", async () => {
    await open("a\r\nb\nc\rd");
    type("a\nb\nc\nd\ne");
    await saveBuffer(key, { sudo: false });
    expect(written()).toEqual(["a\nb\nc\nd\ne"]);
    expect(encodingWords(getBuffer(key)!)).toEqual({ text: "UTF-8 · LF" });
  });

  it("a byte order mark stays", async () => {
    await open("﻿a\r\n");
    type("﻿b\n");
    await saveBuffer(key, { sudo: false });
    expect(written()).toEqual(["﻿b\r\n"]);
    expect(encodingWords(getBuffer(key)!).text).toBe("UTF-8 with BOM · CRLF");
  });

  it("Reload reads the file's endings again, and a change outside SparkBox shows in the status line", async () => {
    await open("a\r\n", { files_read: () => file("a\nb\n") });
    await reloadBuffer(key);
    expect(encodingWords(getBuffer(key)!).text).toBe("UTF-8 · LF");
  });

  it("a previous version comes back in its own line endings", async () => {
    await open("a\nb\n", { files_read: () => file("a\r\nb\r\n", { path: "/x" }) });
    await restoreBackup(key, { path: "/backup", mtime: 1, size: 1 });
    const b = getBuffer(key)!;
    expect([b.dirty, b.eol, b.text]).toEqual([false, "CRLF", "a\nb\n"]);
    replaceBufferText(key, "a\r\nc\r\n");
    expect(getBuffer(key)!.dirty).toBe(true);
    await saveBuffer(key, { sudo: false });
    expect(written()).toEqual(["a\r\nc\r\n"]);
  });

  it("Show diff compares like with like: a CRLF file on disk isn't every line changed", async () => {
    await open("a\nb\n", {
      files_write: () => { throw { kind: "changedOnDisk", message: "changed", sha256: "cc", mtime: 5, size: 3 }; },
      files_read: () => file("a\r\nb\r\n"),
    });
    type("a\nc\n");
    expect(await saveBuffer(key, { sudo: false })).toBe("conflict");
    expect(await loadDiskText(key)).toBe("a\nb\n");
  });
});

describe("a save on this Mac and Admin's two scopes", () => {
  it("node scope unlocked, Mac scope lapsed behind the UI's back: it asks once, then saves", async () => {
    let tries = 0;
    await open("a\n", {
      files_write: (a) => {
        if (++tries === 1) throw { kind: "failed", message: NEEDS_AUTH };
        return saved(a.content);
      },
      admin_authenticate: () => ({ ...MAC_LAPSED, authenticated: true, macExpiresAtMs: Date.now() + 300_000 }),
      admin_state: () => MAC_LAPSED,
    });
    type("b\n");
    expect(await saveBuffer(key, { sudo: false })).toBe("saved");
    expect(mockCalls.filter((c) => c.cmd === "admin_authenticate").map((c) => c.args.action), "once, for this Mac").toEqual(["files_mac"]);
    expect(written()).toEqual(["b\n", "b\n"]);
    expect(getBuffer(key)!.error).toBeNull();
  });

  it("a cancelled prompt leaves the changes in the editor and no error", async () => {
    await open("a\n", {
      files_write: () => { throw { kind: "failed", message: NEEDS_AUTH }; },
      admin_authenticate: () => { throw { kind: "cancelled", message: "Admin mode stays locked." }; },
    });
    type("b\n");
    expect(await saveBuffer(key, { sudo: false })).toBe("failed");
    const b = getBuffer(key)!;
    expect([b.error, b.saving, b.dirty, written().length]).toEqual([null, false, true, 1]);
  });

  it("it asks no more than once: refused again, the refusal shows in words", async () => {
    await open("a\n", {
      files_write: () => { throw { kind: "failed", message: NEEDS_AUTH }; },
      admin_authenticate: () => ({ ...MAC_LAPSED, authenticated: true, macExpiresAtMs: Date.now() + 300_000 }),
    });
    type("b\n");
    expect(await saveBuffer(key, { sudo: false })).toBe("failed");
    expect(mockCalls.filter((c) => c.cmd === "admin_authenticate")).toHaveLength(1);
    expect(getBuffer(key)!.error).toBe("Confirm it's you with Touch ID or your Mac password to change settings on this Mac.");
  });

  it("a save never shows raw JSON", async () => {
    for (const refusal of [
      { kind: "lookOnly", message: "SparkBox is in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date." },
      { kind: "fenced", message: "The fence doesn't allow this change.", detail: "cell2" },
      { kind: "failed", message: "disk full" },
    ]) {
      closeBuffer(key);
      await open("a\n", { files_write: () => { throw { kind: "failed", message: JSON.stringify(refusal) }; } });
      type("b\n");
      expect(await saveBuffer(key, { sudo: false })).toBe("failed");
      expect(getBuffer(key)!.error).toBe(refusal.message);
    }
  });

  it("a plain Failed reads as it is", async () => {
    await open("a\n", { files_write: () => { throw { kind: "failed", message: "Permission denied" }; } });
    type("b\n");
    expect(await saveBuffer(key, { sudo: false })).toBe("failed");
    expect(getBuffer(key)!.error).toBe("Permission denied");
  });

  it("a node's file asks for node scope, never Touch ID for the Mac", async () => {
    const node = bufferKey("cell2", "/etc/x");
    closeBuffer(node);
    mockTauri({ files_read: () => file("a\n"), files_backups: () => [], files_write: (a) => saved(a.content), admin_authenticate: vi.fn() });
    _resetAdminForTests(MAC_LAPSED);
    ensureBuffer("cell2", "/etc/x");
    await flush();
    setBufferState(node, EditorState.create({ doc: "b\n" }), true);
    expect(await saveBuffer(node, { sudo: false })).toBe("saved");
    expect(mockCalls.filter((c) => c.cmd.startsWith("admin_"))).toEqual([]);
    closeBuffer(node);
  });
});
