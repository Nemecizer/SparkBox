// Path helpers for the Files window and the editor (POSIX paths on the Mac and the nodes).
import type { FileEntry } from "../../lib/filesApi";

export function join(dir: string, name: string): string {
  return `${dir.replace(/\/+$/, "")}/${name}`;
}

export function parentOf(path: string): string {
  const p = path.replace(/\/+$/, "");
  const i = p.lastIndexOf("/");
  return i <= 0 ? "/" : p.slice(0, i);
}

export function baseName(path: string): string {
  const p = path.replace(/\/+$/, "");
  return p.slice(p.lastIndexOf("/") + 1) || "/";
}

/** "/home/u/models" → [{name:"/", path:"/"}, {name:"home", path:"/home"}, …]. */
export function segments(path: string): { name: string; path: string }[] {
  const parts = path.split("/").filter(Boolean);
  const out = [{ name: "/", path: "/" }];
  let acc = "";
  for (const p of parts) {
    acc += `/${p}`;
    out.push({ name: p, path: acc });
  }
  return out;
}

/** "~/x" with the home folder shown as "~". */
export function tildify(path: string, home: string | null): string {
  if (!home) return path;
  if (path === home) return "~";
  return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
}

/** "rwxr-xr-x" (or with a type letter in front) → "755". */
export function octal(mode: string): string {
  const bits = mode.length === 10 ? mode.slice(1) : mode;
  let n = 0;
  for (let i = 0; i < 9; i++) if (bits[i] && bits[i] !== "-") n |= 0o400 >> i;
  return n.toString(8).padStart(3, "0");
}

export const isFolder = (e: Pick<FileEntry, "kind" | "linkDir">) => e.kind === "dir" || e.linkDir;

/** A name for a copy that doesn't clash, Finder style: "notes 2.txt", "models 2". */
export function freeName(taken: ReadonlySet<string>, name: string, dir: boolean): string {
  const dot = dir ? -1 : name.lastIndexOf(".");
  const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
  for (let n = 2; ; n++) {
    const c = `${stem} ${n}${ext}`;
    if (!taken.has(c)) return c;
  }
}

export function validName(name: string): string | null {
  if (!name.trim()) return "Enter a name.";
  if (name === "." || name === "..") return "That name is reserved.";
  if (name.includes("/")) return "Names can't contain “/”.";
  if (name.includes("\0")) return "That name has a character files can't use.";
  return null;
}
