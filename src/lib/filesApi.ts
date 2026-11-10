// Tauri wrappers and types for the Files window and the editor (src-tauri/src/files).
// Saves carry the SHA-256 of the text that was opened, so a file changed meanwhile comes
// back as a typed "changedOnDisk" error instead of being overwritten.
import { Channel } from "@tauri-apps/api/core";
import { invoke } from "./api";
import type { FileEntry as BaseEntry } from "./types";

export const MAC = "mac";

export interface FileEntry extends BaseEntry {
  owner?: string | null;
}

export interface Listing {
  place: string;
  path: string;
  entries: FileEntry[];
  /** Free and total bytes of the filesystem the folder is on. */
  free?: number | null;
  capacity?: number | null;
}

export interface TextFile {
  path: string;
  content: string;
  size: number;
  mtime: number;
  sha256: string;
  mode: string;
  /** "user:group" */
  owner: string;
  /** The cluster user can save it without sudo. */
  writable: boolean;
  /** Over 2 MB: content is empty and head / tail hold the first and last 64 KB. */
  large: boolean;
  head: string | null;
  tail: string | null;
}

export interface Saved {
  path: string;
  sha256: string;
  mtime: number;
  size: number;
  backup: string | null;
}

export type SaveError =
  | { kind: "changedOnDisk"; message: string; sha256: string | null; mtime: number | null; size: number | null }
  | { kind: "needsSudo"; message: string }
  | { kind: "failed"; message: string };

export function isSaveError(e: unknown): e is SaveError {
  return !!e && typeof e === "object" && "kind" in e && ["changedOnDisk", "needsSudo", "failed"].includes((e as SaveError).kind);
}

export interface Backup {
  path: string;
  mtime: number;
  size: number;
}

export interface Trashed {
  path: string;
  /** Where it went; null when only Finder knows (use Put Back). */
  trashed: string | null;
}

export interface Usage {
  path: string;
  bytes: number;
}

export type Policy = "replace" | "merge" | "skip" | "keepBoth" | "replaceIfNewer";

export interface TransferSpec {
  from: string;
  /** The folder the items are in. */
  dir: string;
  names: string[];
  to: string;
  toDir: string;
  op: "copy" | "move";
  /** For names already taken at the destination. */
  policies: Record<string, Policy>;
}

export interface Side {
  kind: string;
  bytes: number;
  entries: number;
  mtime: number;
  tarBytes: number;
}

export interface Conflict {
  name: string;
  source: Side;
  dest: Side;
}

export interface Preflight {
  bytes: number;
  entries: number;
  conflicts: Conflict[];
  free: number | null;
  route: "local" | "direct" | "viaMac";
}

export interface Placed {
  name: string;
  action: "copied" | "replaced" | "merged" | "keptBoth" | "skipped";
  as: string;
}

export type TransferEvent =
  | { kind: "progress"; done: number; total: number; files: number; filesTotal: number; current: string | null }
  | { kind: "done"; bytes: number; files: number; placed: Placed[] }
  | { kind: "failed"; message: string }
  | { kind: "cancelled" }
  /** The staging folder couldn't be removed (the node went away); retried with it back and Admin unlocked. */
  | { kind: "leftover"; place: string; path: string; bytes: number | null };

/** A staging folder a transfer couldn't remove yet. */
export interface Leftover {
  place: string;
  dir: string;
  nonce: string;
  path: string;
  bytes: number | null;
  why: string;
}

export const filesApi = {
  list: (place: string, path: string) => invoke<Listing>("files_list", { place, path }),
  read: (place: string, path: string) => invoke<TextFile>("files_read", { place, path }),
  /** Reads a file only root can read, as root (node scope: Admin mode). */
  readSudo: (place: string, path: string) => invoke<TextFile>("files_read_sudo", { place, path }),
  /** Rejects with a SaveError. `expectSha256` null overwrites whatever is there. */
  write: (place: string, path: string, content: string, sudo: boolean, expectSha256: string | null) =>
    invoke<Saved>("files_write", { place, path, content, sudo, expectSha256 }),
  backups: (place: string, path: string) => invoke<Backup[]>("files_backups", { place, path }),
  mkdir: (place: string, dir: string, name: string) => invoke<void>("files_mkdir", { place, dir, name }),
  /** An empty file, never over anything already there. Resolves to its path. */
  newFile: (place: string, dir: string, name: string) => invoke<string>("files_new_file", { place, dir, name }),
  rename: (place: string, path: string, newName: string) => invoke<void>("files_rename", { place, path, newName }),
  trash: (place: string, paths: string[]) => invoke<Trashed[]>("files_trash", { place, paths }),
  untrash: (place: string, items: Trashed[]) => invoke<void>("files_untrash", { place, items }),
  /** Deletes for good. */
  remove: (place: string, paths: string[]) => invoke<void>("files_delete", { place, paths }),
  chmod: (place: string, path: string, mode: number) => invoke<void>("files_chmod", { place, path, mode }),
  du: (place: string, paths: string[]) => invoke<Usage[]>("files_du", { place, paths }),
  preflight: (spec: TransferSpec) => invoke<Preflight>("files_preflight", { spec }),
  /** Starts a transfer; progress and the result arrive through `onEvent`. Resolves to its id. */
  transfer: (spec: TransferSpec, onEvent: (e: TransferEvent) => void) => {
    const channel = new Channel<TransferEvent>();
    channel.onmessage = onEvent;
    return invoke<string>("files_transfer", { spec, onEvent: channel });
  },
  cancel: (id: string) => invoke<void>("files_cancel", { id }),
  leftovers: () => invoke<Leftover[]>("files_leftovers"),
  removeLeftover: (place: string, path: string) => invoke<void>("files_leftover_remove", { place, path }),
};

/** A read refused because only root (or the owner) can read the file (sparkjob read's permission reply). */
export interface PermissionRefusal {
  path: string;
  /** Octal, e.g. "600". */
  mode: string;
  owner: string;
  /** "Only root can read 40-cx7.yaml (root, 600). Reading it needs sudo, which needs Admin." */
  message: string;
}

/** The permission refusal in a files_read error, or null for any other error. */
export function permissionRefusal(e: unknown): PermissionRefusal | null {
  const err = e as { kind?: unknown; message?: unknown; detail?: unknown } | null;
  if (!err || typeof err !== "object" || err.kind !== "permission") return null;
  let facts: { path?: string; mode?: string; owner?: string } = {};
  try {
    facts = typeof err.detail === "string" ? JSON.parse(err.detail) : {};
  } catch {
    // The sentence alone still explains it.
  }
  return { path: facts.path ?? "", mode: facts.mode ?? "", owner: facts.owner ?? "", message: String(err.message ?? "") };
}

/** The text of an error from a files command (a SaveError, an Error or a plain string). */
export function errorText(e: unknown): string {
  if (isSaveError(e)) return e.message;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message).replace(/^Error: /, "");
  return String(e).replace(/^Error: /, "");
}
