// Open editor buffers, outside React: CodeMirror's state (the text and its undo history) and
// what the file looked like on disk when it was read. Minimizing or re-rendering the editor
// window loses nothing; only closing it (after the close guard) drops the buffer. In the buffer
// text always has LF line endings, as CodeMirror's does; a save puts the file's own back.
import { useSyncExternalStore } from "react";
import type { EditorState } from "@codemirror/state";
import { ADMIN_REFUSED, withAdmin } from "./admin";
import { commandError } from "./api";
import { errorText, filesApi, isSaveError, MAC, permissionRefusal, type Backup, type PermissionRefusal, type TextFile } from "./filesApi";

export type BufferStatus = "loading" | "ready" | "large" | "error" | "permission";
export type Eol = "LF" | "CRLF" | "CR" | "Mixed";

const BOM = "\uFEFF";

/** The line ending `text` uses; none at all reads as LF. */
export function eolOf(text: string): Eol {
  const count = (re: RegExp) => text.match(re)?.length ?? 0;
  const found: Eol[] = [];
  if (count(/(?<!\r)\n/g)) found.push("LF");
  if (count(/\r\n/g)) found.push("CRLF");
  if (count(/\r(?!\n)/g)) found.push("CR");
  return found.length > 1 ? "Mixed" : found[0] ?? "LF";
}

const toLf = (text: string) => text.replace(/\r\n?/g, "\n");
const withEol = (text: string, eol: Eol) => (eol === "CRLF" ? text.replace(/\n/g, "\r\n") : eol === "CR" ? text.replace(/\n/g, "\r") : text);

/** The status line's encoding and line endings. Mixed endings save as LF, and the tooltip says so. */
export function encodingWords(b: Pick<Buffer, "eol" | "bom">): { text: string; title?: string } {
  const enc = b.bom ? "UTF-8 with BOM" : "UTF-8";
  return b.eol === "Mixed" ? { text: `${enc} · Mixed line endings`, title: "Saving makes them all LF" } : { text: `${enc} · ${b.eol}` };
}

export interface Conflict {
  /** What's on disk now (null sha: the file was deleted). */
  sha256: string | null;
  mtime: number | null;
  size: number | null;
  /** Its text, once read for Show diff. */
  diskText?: string;
}

export interface Buffer {
  key: string;
  place: string;
  path: string;
  status: BufferStatus;
  /** The file as last read or saved. */
  file: TextFile | null;
  /** The line ending of the file on disk (or the previous version loaded): a save writes this one. */
  eol: Eol;
  /** The file starts with a byte order mark (it stays in the text, so a save keeps it). */
  bom: boolean;
  /** The text on disk the buffer is based on, with LF line endings. */
  baseText: string;
  /** The text when there's no editor state yet (just read, or restored from a backup). */
  text: string;
  /** CodeMirror's state, kept by the editor so undo survives a remount. */
  state: EditorState | null;
  dirty: boolean;
  /** Bumps when the document is replaced from outside (read, reload, restore): the editor rebuilds. */
  generation: number;
  saving: boolean;
  error: string | null;
  conflict: Conflict | null;
  /** The node won't let the cluster user write it; Save with sudo can. */
  needsSudo: boolean;
  savedAtMs: number | null;
  backups: Backup[];
  /** Only root can read it (status "permission"): who owns it and its mode, for the explanation. */
  permission?: PermissionRefusal | null;
  /** Read as root with Read with sudo…: saving it goes through Save with sudo. */
  readAsRoot?: boolean;
}

const buffers = new Map<string, Buffer>();
const listeners = new Set<() => void>();
let version = 0;

function publish() {
  version++;
  listeners.forEach((l) => l());
}

function put(b: Buffer) {
  buffers.set(b.key, b);
  publish();
}

function update(key: string, patch: Partial<Buffer>) {
  const b = buffers.get(key);
  if (b) put({ ...b, ...patch });
}

export const bufferKey = (place: string, path: string) => `${place}:${path}`;
export const isDirty = (b: Pick<Buffer, "dirty" | "status">) => b.status === "ready" && b.dirty;
/** The buffer's current text. */
export const bufferText = (b: Pick<Buffer, "state" | "text">) => (b.state ? b.state.doc.toString() : b.text);

function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useBuffer(key: string): Buffer | null {
  useSyncExternalStore(subscribe, () => version);
  return buffers.get(key) ?? null;
}

export function getBuffer(key: string): Buffer | null {
  return buffers.get(key) ?? null;
}

export function dirtyBuffers(): Buffer[] {
  return [...buffers.values()].filter(isDirty);
}

function fromFile(b: Buffer, f: TextFile): Buffer {
  // A large file is read-only and only its head is here; a cut may fall inside a CRLF.
  const raw = f.large ? (f.head ?? "").replace(/\r$/, "") : f.content;
  const text = toLf(f.content);
  return {
    ...b, file: f, status: f.large ? "large" : "ready", eol: eolOf(raw), bom: raw.startsWith(BOM), baseText: text, text, state: null, dirty: false,
    generation: b.generation + 1, error: null, conflict: null, needsSudo: !f.writable,
  };
}

/** Opens (or returns the already open) buffer for a file, reading it the first time. */
export function ensureBuffer(place: string, path: string): Buffer {
  const key = bufferKey(place, path);
  const have = buffers.get(key);
  if (have && have.status !== "error" && have.status !== "permission") return have;
  const b: Buffer = {
    key, place, path, status: "loading", file: null, eol: "LF", bom: false, baseText: "", text: "", state: null, dirty: false, generation: have?.generation ?? 0,
    saving: false, error: null, conflict: null, needsSudo: false, savedAtMs: null, backups: [],
  };
  put(b);
  void load(key);
  return b;
}

async function load(key: string) {
  const b = buffers.get(key);
  if (!b) return;
  try {
    const f = await filesApi.read(b.place, b.path);
    const cur = buffers.get(key);
    if (cur) put(fromFile(cur, f));
    void refreshBackups(key);
  } catch (e) {
    const permission = permissionRefusal(e);
    update(key, permission ? { status: "permission", permission, error: null } : { status: "error", error: errorText(e) });
  }
}

/** Read with sudo…: reads a root-only file as root (the caller has unlocked node scope). */
export async function readBufferAsRoot(key: string): Promise<boolean> {
  const b = buffers.get(key);
  if (!b) return false;
  try {
    const f = await filesApi.readSudo(b.place, b.path);
    const cur = buffers.get(key);
    if (cur) put({ ...fromFile(cur, f), permission: null, readAsRoot: true, needsSudo: true });
    return true;
  } catch (e) {
    update(key, { error: errorText(e) });
    return false;
  }
}

/** Reads the file again, dropping unsaved changes (Reload after a conflict, or Revert). */
export async function reloadBuffer(key: string): Promise<void> {
  const b = buffers.get(key);
  if (!b) return;
  try {
    const f = await (b.readAsRoot ? filesApi.readSudo(b.place, b.path) : filesApi.read(b.place, b.path));
    const cur = buffers.get(key);
    if (cur) put({ ...fromFile(cur, f), needsSudo: cur.readAsRoot || !f.writable });
  } catch (e) {
    update(key, { error: errorText(e) });
  }
}

/** The editor's latest state. Listeners hear about it only when the dirty flag flips. */
export function setBufferState(key: string, state: EditorState, dirty: boolean) {
  const b = buffers.get(key);
  if (!b) return;
  buffers.set(key, { ...b, state, dirty });
  if (b.dirty !== dirty) publish();
}

/** Starts from another text as unsaved changes (Restore previous version). */
export function replaceBufferText(key: string, text: string) {
  const b = buffers.get(key);
  if (!b) return;
  const lf = toLf(text);
  put({ ...b, text: lf, eol: eolOf(text), bom: text.startsWith(BOM), state: null, dirty: lf !== b.baseText, generation: b.generation + 1 });
}

export type SaveResult = "saved" | "conflict" | "needsSudo" | "failed";

/**
 * A gate refusal reaches a save as a Failed whose message is the refusal's JSON (the backend's
 * `From<String> for SaveError`): this gives the refusal itself, and any other error as it is.
 */
function refusalIn(e: unknown): unknown {
  return isSaveError(e) && e.kind === "failed" ? commandError(e.message) : e;
}

/**
 * Saves the buffer, in the file's own line ending (Mixed saves as LF). Normally it only replaces
 * the version it was read from; `overwrite` saves over whatever is there now (after a conflict).
 * When the backend says Mac scope has lapsed, it asks once and tries again.
 */
export async function saveBuffer(key: string, opts: { sudo: boolean; overwrite?: boolean }): Promise<SaveResult> {
  const b = buffers.get(key);
  if (!b || !b.file || b.saving) return "failed";
  const text = bufferText(b);
  const eol = b.eol === "Mixed" ? "LF" : b.eol;
  const write = () => filesApi.write(b.place, b.path, withEol(text, eol), opts.sudo, opts.overwrite ? null : b.file!.sha256).catch((e) => {
    throw refusalIn(e);
  });
  const mac = b.place === MAC;
  update(key, { saving: true, error: null });
  try {
    const saved = await withAdmin(write, { scope: mac ? "mac" : "node", action: mac ? "files_mac" : "files_write" });
    const cur = buffers.get(key)!;
    const still = bufferText(cur) === text;
    put({
      ...cur, saving: false, eol, bom: text.startsWith(BOM), baseText: text, dirty: !still, conflict: null, savedAtMs: Date.now(),
      file: { ...cur.file!, sha256: saved.sha256, mtime: saved.mtime, size: saved.size },
    });
    void refreshBackups(key);
    return "saved";
  } catch (e) {
    if (e === ADMIN_REFUSED) {
      update(key, { saving: false });
      return "failed";
    }
    if (isSaveError(e) && e.kind === "changedOnDisk") {
      update(key, { saving: false, conflict: { sha256: e.sha256, mtime: e.mtime, size: e.size } });
      return "conflict";
    }
    if (isSaveError(e) && e.kind === "needsSudo") {
      update(key, { saving: false, needsSudo: true });
      return "needsSudo";
    }
    update(key, { saving: false, error: errorText(e) });
    return "failed";
  }
}

/** Reads what's on disk now, for Show diff after a conflict. */
export async function loadDiskText(key: string): Promise<string | null> {
  const b = buffers.get(key);
  if (!b?.conflict) return null;
  if (b.conflict.sha256 === null) return "";
  try {
    const f = await (b.readAsRoot ? filesApi.readSudo(b.place, b.path) : filesApi.read(b.place, b.path));
    const cur = buffers.get(key);
    const text = toLf(f.content);
    if (cur?.conflict) update(key, { conflict: { ...cur.conflict, diskText: text } });
    return text;
  } catch (e) {
    update(key, { error: errorText(e) });
    return null;
  }
}

export async function refreshBackups(key: string): Promise<void> {
  const b = buffers.get(key);
  if (!b) return;
  try {
    const backups = await filesApi.backups(b.place, b.path);
    update(key, { backups });
  } catch {
    // No previous versions to offer.
  }
}

/** Loads a previous version into the buffer as unsaved changes; saving makes it current. */
export async function restoreBackup(key: string, backup: Backup): Promise<void> {
  const b = buffers.get(key);
  if (!b) return;
  try {
    const f = await filesApi.read(b.place, backup.path);
    replaceBufferText(key, f.content);
  } catch (e) {
    update(key, { error: errorText(e) });
  }
}

export function clearBufferError(key: string) {
  update(key, { error: null });
}

export function dismissConflict(key: string) {
  update(key, { conflict: null });
}

/** Forgets a buffer (its window closed). */
export function closeBuffer(key: string) {
  if (buffers.delete(key)) publish();
}
