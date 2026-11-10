// Files: two panes, each This Mac or a node, in the manner of ForkLift and Transmit.
// Multi-select tables (folders first, natural sort), keyboard control (Tab between panes,
// Return / ⌘↓ to open, ⌘↑ up, ⌘[ ⌘] back and forward, F5 copy, F6 move, F2 rename, ⌫ to the
// Trash, Space for info, ⌘C / ⌘V across panes), context menus, drag and drop between panes
// and from Finder, a transfer queue with conflict choices, and an editor window for text.
// Every change needs Admin mode; Move to Trash can be undone with ⌘Z.
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  ArrowLeftRight, ChevronDown, ClipboardCopy, Copy, CopyPlus, Eye, EyeOff, FilePen, FilePlus, FolderOpen, FolderPlus, Info, KeyRound, MoveRight, Pencil,
  Calculator, SquareTerminal, Trash, Trash2,
} from "lucide-react";
import { HelpTip, Segmented, Spacer, Toolbar, ToolButton, ToolSeparator } from "../components/ui";
import { confirmUnlock, unlockedFor, useAdminState } from "../lib/admin";

/**
 * A change item's lock in a menu. In look-only mode it's `locked`, which the menu disables
 * (ContextMenuHost). Locked otherwise, it carries only a tag: its own ensureAdmin asks for the
 * scope it needs (Touch ID for this Mac), so the menu mustn't ask first.
 */
const changeLock = (lookOnly: boolean | undefined, needsUnlock: boolean): { locked?: boolean; hint?: string } =>
  lookOnly ? { locked: true } : needsUnlock ? { hint: "Needs Admin mode (it asks to unlock)" } : {};
import { inTauri } from "../lib/api";
import { showContextMenu, type MenuItem } from "../lib/contextMenu";
import { errorText, filesApi, MAC, type FileEntry, type Listing, type Policy, type TransferSpec } from "../lib/filesApi";
import { promptText, confirmAction } from "../lib/session";
import { useCluster } from "../lib/store";
import { toast } from "../lib/toasts";
import { onTransferLanded, startGatedTransfer, transferGate, transferGuardReason, transferLocked, useTransfers } from "../lib/transfers";
import { pushUndo } from "../lib/undo";
import { useCloseGuard, useViewLink, useWindowEvent, useWindowId, useWinState, useWMActions, useWMState, winId, type Params } from "../wm/WindowManager";
import { ConflictSheet } from "./files/ConflictSheet";
import { hitTest, makeGhost, type DragGhost, type DropTarget } from "./files/drag";
import { InfoPopover } from "./files/InfoPopover";
import { filesLayout, switcherLabels, type FilesLayout } from "./files/layout";
import { Pane, type PlaceInfo } from "./files/Pane";
import { isFolder, join, octal, parentOf, validName } from "./files/paths";
import { getFileClip, setFileClip, setSizes, sizeKey, useSizes } from "./files/stores";
import { TransferQueue } from "./files/TransferQueue";
import { connectTo } from "./terminal/QuickConnect";
import "./files/files.css";

// Other windows (a node's Files button) ask for a node to show in the right pane.
let requested: string | null = null;
const requestListeners = new Set<() => void>();
export function showFilesOn(node: string) {
  requested = node;
  requestListeners.forEach((l) => l());
}

interface Loc {
  place: string;
  path: string;
  back: { place: string; path: string }[];
  fwd: { place: string; path: string }[];
}

interface View {
  listing: Listing | null;
  loading: boolean;
  error: string | null;
}

const EMPTY_VIEW: View = { listing: null, loading: false, error: null };
const BINARY = /\.(safetensors|bin|gguf|pt|pth|onnx|tar|gz|tgz|zst|xz|zip|7z|so|o|a|dylib|png|jpe?g|gif|webp|pdf|iso|img|deb|whl)$/i;
const HISTORY = 50;

function initialLocs(params: Params, firstNode: string): Loc[] {
  const blank = (place: string, path: string): Loc => ({ place, path, back: [], fwd: [] });
  if (params.place && params.place !== MAC) return [blank(MAC, "~"), blank(params.place, params.path ?? "~")];
  if (params.place === MAC) return [blank(MAC, params.path ?? "~"), blank(firstNode, "~")];
  return [blank(MAC, "~"), blank(requested ?? firstNode, "~")];
}

export function FilesWindow({ params = {} }: { params?: Params }) {
  const wm = useWMActions();
  const myId = useWindowId();
  const cluster = useCluster();
  const adminNow = useAdminState();
  const admin = adminNow.unlocked;
  const transfers = useTransfers();
  const sizes = useSizes();
  const nodes = cluster?.nodes ?? [];
  const firstNode = nodes[0]?.id ?? "cell1";

  const places: PlaceInfo[] = useMemo(
    () => [{ id: MAC, name: "This Mac", status: "online" as const }, ...nodes.map((n) => ({ id: n.id, name: n.name, status: n.status }))],
    [nodes.map((n) => `${n.id}:${n.name}:${n.status}`).join("|")],
  );
  const placeName = useCallback((p: string) => places.find((x) => x.id === p)?.name ?? p, [places]);

  const [locs, setLocs] = useWinState<Loc[]>("panes", initialLocs(params, firstNode));
  const [active, setActive] = useWinState<0 | 1>("active", 1);
  const [hidden, setHidden] = useWinState("hidden", false);
  const [sel, setSel] = useWinState<[string[], string[]]>("selection", [[], []]);
  const [views, setViews] = useState<[View, View]>([EMPTY_VIEW, EMPTY_VIEW]);
  const [homes, setHomes] = useState<Record<string, string>>({});
  const [info, setInfo] = useState<{ pane: 0 | 1; name: string; rect: DOMRect | null } | null>(null);
  const [drop, setDrop] = useState<{ pane: number; folder: string | null } | null>(null);
  const [alt, setAlt] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  // One pane at a time when the window is narrow (tiled), with a switcher.
  const [layout, setLayout] = useState<FilesLayout>("split");
  useEffect(() => {
    const el = root.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(([e]) => setLayout(filesLayout(e.contentRect.width)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const locsRef = useRef(locs);
  locsRef.current = locs;
  const viewsRef = useRef(views);
  viewsRef.current = views;

  const other = (i: 0 | 1): 0 | 1 => (i === 0 ? 1 : 0);
  const dirOf = (i: 0 | 1) => viewsRef.current[i].listing?.path ?? locsRef.current[i].path;
  const selOf = (i: 0 | 1): string[] => {
    const rows = viewsRef.current[i].listing?.entries ?? [];
    const s = new Set(sel[i]);
    return rows.filter((e) => s.has(e.name)).map((e) => e.name);
  };
  const entryOf = (i: 0 | 1, name: string) => viewsRef.current[i].listing?.entries.find((e) => e.name === name) ?? null;
  const setSelFor = (i: 0 | 1, names: string[]) => setSel((s) => (i === 0 ? [names, s[1]] : [s[0], names]));
  const patchView = (i: 0 | 1, p: Partial<View>) => setViews((v) => (i === 0 ? [{ ...v[0], ...p }, v[1]] : [v[0], { ...v[1], ...p }]));

  // ---- loading and navigation

  const seq = useRef([0, 0]);
  const load = useCallback(async (i: 0 | 1, place: string, path: string, select?: string) => {
    const n = ++seq.current[i];
    patchView(i, { loading: true });
    try {
      const listing = await filesApi.list(place, path);
      if (seq.current[i] !== n) return;
      if (path === "~" || path === "") setHomes((h) => (h[place] === listing.path ? h : { ...h, [place]: listing.path }));
      setViews((v) => {
        const keep = v[i].listing?.place === place && v[i].listing?.path === listing.path;
        const next: View = { listing, loading: false, error: keep ? v[i].error : null };
        return i === 0 ? [next, v[1]] : [v[0], next];
      });
      setLocs((ls) => ls.map((l, k) => (k === i ? { ...l, place, path: listing.path } : l)));
      if (select !== undefined) setSelFor(i, [select]);
    } catch (e) {
      if (seq.current[i] !== n) return;
      patchView(i, { loading: false, error: errorText(e) });
    }
  }, []);

  const go = (i: 0 | 1, place: string, path: string, select?: string) => {
    const cur = locsRef.current[i];
    const here = { place: cur.place, path: dirOf(i) };
    setLocs((ls) => ls.map((l, k) => (k === i ? { ...l, back: [...l.back, here].slice(-HISTORY), fwd: [] } : l)));
    setSelFor(i, []);
    patchView(i, { error: null });
    void load(i, place, path, select);
  };
  const back = (i: 0 | 1) => {
    const cur = locsRef.current[i];
    const to = cur.back[cur.back.length - 1];
    if (!to) return;
    setLocs((ls) => ls.map((l, k) => (k === i ? { ...l, back: l.back.slice(0, -1), fwd: [{ place: l.place, path: dirOf(i) }, ...l.fwd] } : l)));
    patchView(i, { error: null });
    void load(i, to.place, to.path);
  };
  const forward = (i: 0 | 1) => {
    const cur = locsRef.current[i];
    const to = cur.fwd[0];
    if (!to) return;
    setLocs((ls) => ls.map((l, k) => (k === i ? { ...l, fwd: l.fwd.slice(1), back: [...l.back, { place: l.place, path: dirOf(i) }] } : l)));
    patchView(i, { error: null });
    void load(i, to.place, to.path);
  };
  const up = (i: 0 | 1) => {
    const d = dirOf(i);
    if (d === "/") return;
    go(i, locsRef.current[i].place, parentOf(d), d.split("/").pop());
  };
  const refresh = (i: 0 | 1) => void load(i, locsRef.current[i].place, dirOf(i));

  useEffect(() => {
    void load(0, locs[0].place, locs[0].path);
    void load(1, locs[1].place, locs[1].path);
    // The active pane's list takes the keyboard when the window opens.
    requestAnimationFrame(() => root.current?.querySelector<HTMLElement>(`[data-pane="${active}"] .table-scroll`)?.focus({ preventScroll: true }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // A node's Files button: show that node on the right.
  useEffect(() => {
    const onReq = () => {
      if (!requested) return;
      const node = requested;
      requested = null;
      setActive(1);
      go(1, node, "~");
    };
    requestListeners.add(onReq);
    onReq();
    return () => {
      requestListeners.delete(onReq);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The same from a link with a place (the sidebar's or Dashboard's Files item) while the window is open.
  useViewLink(params, () => {
    if (!params.place) return;
    const pane = params.place === MAC ? 0 : 1;
    setActive(pane);
    go(pane, params.place, params.path ?? "~");
  });

  // A transfer that lands where a pane is looking refreshes it.
  useEffect(() => onTransferLanded((t) => {
    for (const i of [0, 1] as const) {
      const v = viewsRef.current[i].listing;
      if (!v) continue;
      const atDest = v.place === t.spec.to && (v.path === t.spec.toDir || t.spec.toDir.startsWith(`${v.path}/`) || v.path.startsWith(`${t.spec.toDir}/`));
      const atSource = t.spec.op === "move" && v.place === t.spec.from && v.path === t.spec.dir;
      if (atDest || atSource) refresh(i);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }), []);

  useWindowEvent("refresh", () => {
    refresh(0);
    refresh(1);
  });
  useCloseGuard(() => {
    const r = transferGuardReason();
    return r ? `${r}. It keeps going in the background; the Tasks window shows it.` : null;
  });

  // ⌥ turns Move to Trash into Delete immediately, as in Finder.
  useEffect(() => {
    const on = (e: KeyboardEvent) => setAlt(e.altKey);
    const off = () => setAlt(false);
    window.addEventListener("keydown", on);
    window.addEventListener("keyup", on);
    window.addEventListener("blur", off);
    return () => {
      window.removeEventListener("keydown", on);
      window.removeEventListener("keyup", on);
      window.removeEventListener("blur", off);
    };
  }, []);

  // ---- actions

  // Changes on this Mac need the Mac scope (Touch ID even in Demo); changes on a node, node scope.
  // The "Unlock Admin mode?" sheet first, in Demo too (confirmUnlock), then Touch ID where it applies.
  const ensureAdmin = async (what: string, place: string) => {
    const scope = place === MAC ? "mac" : "node";
    return unlockedFor(scope) || (await confirmUnlock(what, scope, place === MAC ? "files_mac" : undefined));
  };
  const fail = (title: string, e: unknown) => toast({ kind: "error", title, body: errorText(e) });

  // "copy node-launch.sh to Documents": the unlock sheet's words for a transfer.
  const transferWords = (spec: TransferSpec) => {
    const what = spec.names.length === 1 ? spec.names[0] : `${spec.names.length} items`;
    const dir = spec.toDir.replace(/\/+$/, "");
    const home = dir === "~" || (!!homes[MAC] && dir === homes[MAC].replace(/\/+$/, ""));
    const folder = home ? "your home folder" : dir.split("/").pop() || spec.toDir;
    return `${spec.op} ${what} to ${spec.to === MAC ? folder : placeName(spec.to)}`;
  };

  // The toolbar, ⌘V and drag and drop all go through transferGate: onto this Mac needs Mac
  // scope, except a copy into ~/Downloads (it arrives quarantined); onto a node, node scope.
  // Replacing or merging on the Mac is asked for once the conflicts are decided (transfers.ts).
  const startTransfer = (spec: TransferSpec) =>
    startGatedTransfer(spec, { fromName: placeName(spec.from), toName: placeName(spec.to), origin: myId }, transferWords(spec), homes[MAC]);

  const copyToOther = (i: 0 | 1, op: "copy" | "move", names = selOf(i)) => {
    if (!names.length) return;
    const o = other(i);
    void startTransfer({ from: locs[i].place, dir: dirOf(i), names, to: locs[o].place, toDir: dirOf(o), op, policies: {} });
  };

  const duplicate = (i: 0 | 1, names = selOf(i)) => {
    if (!names.length) return;
    const policies: Record<string, Policy> = Object.fromEntries(names.map((n) => [n, "keepBoth" as Policy]));
    void startTransfer({ from: locs[i].place, dir: dirOf(i), names, to: locs[i].place, toDir: dirOf(i), op: "copy", policies });
  };

  const trash = async (i: 0 | 1, names = selOf(i)) => {
    if (!names.length) return;
    const place = locs[i].place;
    const what = names.length === 1 ? `“${names[0]}”` : `${names.length} items`;
    if (!(await ensureAdmin(`move ${what} to the Trash`, place))) return;
    try {
      const items = await filesApi.trash(place, names.map((n) => join(dirOf(i), n)));
      refresh(i);
      const known = items.filter((t) => t.trashed);
      if (known.length) {
        pushUndo({
          label: `Moved ${what} to the Trash${place === MAC ? "" : ` on ${placeName(place)}`}`,
          undo: async () => {
            await filesApi.untrash(place, known);
            refresh(i);
          },
          ttlMs: 30_000,
        });
      }
      if (known.length < items.length) toast({ kind: "info", title: "Moved to the Trash", body: "Finder took it; use Put Back in the Trash to restore it." });
    } catch (e) {
      fail(`Couldn't move ${what} to the Trash`, e);
    }
  };

  const deleteNow = async (i: 0 | 1, names = selOf(i)) => {
    if (!names.length) return;
    const place = locs[i].place;
    const what = names.length === 1 ? `“${names[0]}”` : `${names.length} items`;
    if (!(await ensureAdmin(`delete ${what}`, place))) return;
    const ok = await confirmAction({
      title: `Delete ${what} immediately?`,
      body: `${names.length === 1 ? "It" : "They"} won't go to the Trash on ${placeName(place)}, and you can't undo this.`,
      details: names.map((n) => join(dirOf(i), n)).join("\n"),
      confirmLabel: "Delete",
      danger: true,
      winId: myId,
    });
    if (!ok) return;
    try {
      await filesApi.remove(place, names.map((n) => join(dirOf(i), n)));
      refresh(i);
    } catch (e) {
      fail(`Couldn't delete ${what}`, e);
    }
  };

  const rename = async (i: 0 | 1, name = selOf(i)[0]) => {
    if (!name) return;
    if (!(await ensureAdmin(`rename “${name}”`, locs[i].place))) return;
    const taken = new Set(viewsRef.current[i].listing?.entries.map((e) => e.name));
    const next = await promptText({
      title: `Rename “${name}”`,
      label: "New name",
      initial: name,
      confirmLabel: "Rename",
      validate: (v) => validName(v) ?? (v !== name && taken.has(v) ? `“${v}” is already in this folder.` : null),
      winId: myId,
    });
    if (next === null || next === name) return;
    try {
      await filesApi.rename(locs[i].place, join(dirOf(i), name), next);
      await load(i, locs[i].place, dirOf(i), next);
    } catch (e) {
      fail(`Couldn't rename “${name}”`, e);
    }
  };

  const newFolder = async (i: 0 | 1) => {
    if (!(await ensureAdmin("create a folder", locs[i].place))) return;
    const taken = new Set(viewsRef.current[i].listing?.entries.map((e) => e.name));
    let suggestion = "untitled folder";
    for (let n = 2; taken.has(suggestion); n++) suggestion = `untitled folder ${n}`;
    const name = await promptText({
      title: `New folder in ${dirOf(i).split("/").pop() || "/"}`,
      label: "Name",
      initial: suggestion,
      confirmLabel: "Create",
      validate: (v) => validName(v) ?? (taken.has(v) ? `“${v}” is already in this folder.` : null),
      winId: myId,
    });
    if (!name) return;
    try {
      await filesApi.mkdir(locs[i].place, dirOf(i), name);
      await load(i, locs[i].place, dirOf(i), name);
    } catch (e) {
      fail("Couldn't create the folder", e);
    }
  };

  /** Demo nodes have no New file (it has no simulation); this Mac's files are real in Demo too. */
  const newFileOff = (i: 0 | 1) => (cluster?.demo && locs[i].place !== MAC ? "Available with Live data" : false);

  const newFile = async (i: 0 | 1) => {
    const place = locs[i].place;
    if (newFileOff(i) || !(await ensureAdmin("create a file", place))) return;
    const taken = new Set(viewsRef.current[i].listing?.entries.map((e) => e.name));
    let suggestion = "untitled.txt";
    for (let n = 2; taken.has(suggestion); n++) suggestion = `untitled ${n}.txt`;
    const name = await promptText({
      title: `New file in ${dirOf(i).split("/").pop() || "/"}`,
      label: "Name",
      initial: suggestion,
      confirmLabel: "Create",
      validate: (v) => validName(v) ?? (taken.has(v) ? `“${v}” is already in this folder.` : null),
      winId: myId,
    });
    if (!name) return;
    try {
      const path = await filesApi.newFile(place, dirOf(i), name);
      await load(i, place, dirOf(i), name);
      openEditor(place, path);
    } catch (e) {
      fail("Couldn't create the file", e);
    }
  };

  const chmod = async (i: 0 | 1, name = selOf(i)[0]) => {
    const e = name ? entryOf(i, name) : null;
    if (!e) return;
    if (!(await ensureAdmin(`change the permissions of “${name}”`, locs[i].place))) return;
    const value = await promptText({
      title: `Permissions for “${name}”`,
      body: `Now ${e.mode} (${octal(e.mode)}). For example 644 is rw-r--r--, 755 is rwxr-xr-x, 600 is for the owner only.`,
      label: "Octal mode",
      initial: octal(e.mode),
      confirmLabel: "Change",
      validate: (v) => (/^[0-7]{3,4}$/.test(v.trim()) ? null : "Use three or four octal digits, e.g. 644 or 755."),
      winId: myId,
    });
    if (value === null) return;
    try {
      await filesApi.chmod(locs[i].place, join(dirOf(i), name!), parseInt(value.trim(), 8));
      refresh(i);
    } catch (err) {
      fail(`Couldn't change the permissions of “${name}”`, err);
    }
  };

  const calcSize = async (i: 0 | 1, names = selOf(i).filter((n) => { const e = entryOf(i, n); return e && isFolder(e) && e.kind !== "link"; })) => {
    if (!names.length) return;
    const place = locs[i].place;
    const paths = names.map((n) => join(dirOf(i), n));
    setSizes(paths.map((p) => [sizeKey(place, p), "busy"]));
    try {
      const usage = await filesApi.du(place, paths);
      setSizes(usage.map((u) => [sizeKey(place, u.path), u.bytes]));
    } catch (e) {
      setSizes(paths.map((p) => [sizeKey(place, p), null]));
      fail("Couldn't calculate the size", e);
    }
  };

  const copyPath = (i: 0 | 1, names = selOf(i)) => {
    const text = (names.length ? names.map((n) => join(dirOf(i), n)) : [dirOf(i)]).join("\n");
    navigator.clipboard?.writeText(text).then(
      () => toast({ kind: "success", title: names.length > 1 ? `Copied ${names.length} paths` : "Copied the path", body: text.split("\n")[0], durationMs: 2500 }),
      () => {},
    );
  };

  const terminalHere = (i: 0 | 1, dir = dirOf(i)) => {
    if (locs[i].place === MAC) return;
    void connectTo({ node: locs[i].place, target: { kind: "shell", cwd: dir } });
  };

  // Files' own frame from before an editor was snapped beside it, put back when that editor closes.
  const beside = useRef<{ editor: string; frame: { x: number; y: number; w: number; h: number } } | null>(null);
  const openEditor = (place: string, path: string) => {
    const p = { place, path };
    const id = winId("editor", p);
    const existed = wm.getWindows().some((w) => w.id === id);
    wm.open("editor", p);
    if (existed) return;
    // Beside the panes: to the right if there's room, else Files on the left half and the editor on the right.
    const me = wm.getWindows().find((w) => w.id === myId);
    const b = wm.getBounds();
    if (!me) return;
    const right = me.x + me.w;
    if (b.w - right >= 520) wm.move(id, { x: right + 8, y: me.y, w: Math.min(900, b.w - right - 8), h: me.h });
    else {
      beside.current = beside.current ?? { editor: id, frame: { x: me.x, y: me.y, w: me.w, h: me.h } };
      wm.snap(myId, "left");
      wm.snap(id, "right");
    }
  };
  // When that editor closes, Files goes back to its earlier frame, and with it two panes.
  const openIds = useWMState().windows.map((w) => w.id).join("|");
  useEffect(() => {
    const b = beside.current;
    if (!b || openIds.split("|").includes(b.editor)) return;
    beside.current = null;
    wm.move(myId, b.frame);
  }, [openIds, myId, wm]);

  const open = (i: 0 | 1, e: FileEntry) => {
    if (isFolder(e)) return go(i, locs[i].place, join(dirOf(i), e.name));
    if (BINARY.test(e.name)) return showInfo(i, e.name);
    openEditor(locs[i].place, join(dirOf(i), e.name));
  };

  const showInfo = (i: 0 | 1, name: string) => {
    const row = root.current?.querySelector<HTMLElement>(`[data-pane="${i}"] tr[data-key="${CSS.escape(name)}"]`);
    setInfo({ pane: i, name, rect: row?.getBoundingClientRect() ?? null });
  };

  // ---- keyboard

  const onKey = (i: 0 | 1) => (e: ReactKeyboardEvent) => {
    if ((e.target as HTMLElement).closest("input, select, textarea")) return;
    const mod = e.metaKey || e.ctrlKey;
    const names = selOf(i);
    const stop = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    if (e.key === "Tab" && !mod && !e.altKey) {
      stop();
      const o = other(i);
      setActive(o);
      root.current?.querySelector<HTMLElement>(`[data-pane="${o}"] .table-scroll`)?.focus();
      return;
    }
    if ((e.key === " " && !mod) || (mod && e.key.toLowerCase() === "i")) {
      stop();
      if (info) setInfo(null);
      else if (names[0]) showInfo(i, names[0]);
      return;
    }
    if (mod && e.key === "ArrowUp") return (stop(), up(i));
    if (mod && e.key === "ArrowDown") {
      stop();
      const ent = names[0] ? entryOf(i, names[0]) : null;
      if (ent) open(i, ent);
      return;
    }
    if (mod && e.code === "BracketLeft" && !e.shiftKey) return (stop(), back(i));
    if (mod && e.code === "BracketRight" && !e.shiftKey) return (stop(), forward(i));
    if (e.key === "F5") return (stop(), copyToOther(i, "copy"));
    if (e.key === "F6") return (stop(), copyToOther(i, "move"));
    if (e.key === "F2") return (stop(), void rename(i));
    if ((e.key === "Backspace" || e.key === "Delete") && names.length) {
      stop();
      void (e.altKey ? deleteNow(i) : trash(i));
      return;
    }
    if (mod && e.shiftKey && e.key.toLowerCase() === "n") return (stop(), void newFolder(i));
    if (mod && e.key.toLowerCase() === "d" && !e.shiftKey) return (stop(), duplicate(i));
  };

  // ⌘C / ⌘V: the Edit menu's Copy and Paste reach the focused pane as clipboard events.
  const onCopy = (i: 0 | 1) => (e: ClipboardEvent) => {
    if ((e.target as HTMLElement).closest?.("input, textarea")) return;
    const names = selOf(i);
    if (!names.length) return;
    e.preventDefault();
    e.clipboardData?.setData("text/plain", names.map((n) => join(dirOf(i), n)).join("\n"));
    setFileClip({ place: locs[i].place, dir: dirOf(i), names });
    toast({ kind: "info", title: `Copied ${names.length === 1 ? `“${names[0]}”` : `${names.length} items`}`, body: "Paste (⌘V) in a folder to copy it there.", durationMs: 2500 });
  };
  const onPaste = (i: 0 | 1) => (e: ClipboardEvent) => {
    if ((e.target as HTMLElement).closest?.("input, textarea")) return;
    const clip = getFileClip();
    if (!clip) return;
    e.preventDefault();
    const here = { place: locs[i].place, dir: dirOf(i) };
    const same = clip.place === here.place && clip.dir === here.dir;
    const policies: Record<string, Policy> = same ? Object.fromEntries(clip.names.map((n) => [n, "keepBoth" as Policy])) : {};
    void startTransfer({ from: clip.place, dir: clip.dir, names: clip.names, to: here.place, toDir: here.dir, op: "copy", policies });
  };

  // ---- context menus

  /** The menu's lock for Copy/Move to the other pane: a tag only when transferGate says it needs an unlock; in look-only, every copy is held. */
  const gateLock = (i: 0 | 1, op: "copy" | "move", names: string[]) => {
    const o = other(i);
    const g = transferGate({ from: locs[i].place, dir: dirOf(i), names, to: locs[o].place, toDir: dirOf(o), op, policies: {} }, homes[MAC]);
    return g.refused ? { disabled: g.refused } : changeLock(adminNow.lookOnly, transferLocked(g));
  };

  const rowMenu = (i: 0 | 1, e: FileEntry | null, ev: ReactMouseEvent, selected: Set<string>) => {
    const o = other(i);
    const place = locs[i].place;
    const names = e ? [...selected].filter((n) => entryOf(i, n)) : [];
    const one = names.length === 1 ? entryOf(i, names[0]) : null;
    const lock = changeLock(adminNow.lookOnly, !admin);
    // A shell needs node scope, which the menu's own unlock asks for.
    const shell = { locked: !admin };
    const toOther = placeName(locs[o].place);
    const folders = names.filter((n) => { const x = entryOf(i, n); return x && isFolder(x) && x.kind !== "link"; });
    const items: MenuItem[] = !e
      ? [
          { label: "New folder", icon: FolderPlus, shortcut: "⇧⌘N", ...lock, run: () => newFolder(i) },
          { label: "New file", icon: FilePlus, ...lock, disabled: newFileOff(i), run: () => newFile(i) },
          { label: "Paste", icon: ClipboardCopy, shortcut: "⌘V", disabled: getFileClip() ? false : "Copy something first (⌘C)", ...changeLock(adminNow.lookOnly, false), run: () => {
            const clip = getFileClip();
            if (clip) void startTransfer({ from: clip.place, dir: clip.dir, names: clip.names, to: place, toDir: dirOf(i), op: "copy", policies: {} });
          } },
          { separator: true },
          { label: "Open terminal here", icon: SquareTerminal, disabled: place === MAC ? "Only on nodes" : false, ...shell, run: () => terminalHere(i) },
          { label: "Copy path", icon: Copy, run: () => copyPath(i, []) },
          { label: hidden ? "Hide hidden files" : "Show hidden files", icon: hidden ? EyeOff : Eye, run: () => setHidden(!hidden) },
        ]
      : [
          { label: "Open", icon: FolderOpen, shortcut: "↩", disabled: names.length !== 1, run: () => one && open(i, one) },
          { label: "Edit", icon: FilePen, disabled: !one || isFolder(one) ? true : false, run: () => one && openEditor(place, join(dirOf(i), one.name)) },
          { separator: true },
          { label: `Copy to ${toOther}`, icon: Copy, shortcut: "F5", ...gateLock(i, "copy", names), run: () => copyToOther(i, "copy", names) },
          { label: `Move to ${toOther}`, icon: MoveRight, shortcut: "F6", ...gateLock(i, "move", names), run: () => copyToOther(i, "move", names) },
          { label: "Duplicate", icon: CopyPlus, shortcut: "⌘D", ...lock, run: () => duplicate(i, names) },
          { label: "Rename…", icon: Pencil, shortcut: "F2", disabled: names.length !== 1, ...lock, run: () => rename(i, names[0]) },
          { label: "Permissions…", icon: KeyRound, disabled: names.length !== 1, ...lock, run: () => chmod(i, names[0]) },
          { separator: true },
          { label: "Get info", icon: Info, shortcut: "Space", disabled: names.length !== 1, run: () => showInfo(i, names[0]) },
          { label: "Calculate size", icon: Calculator, disabled: folders.length ? false : "Only for folders", run: () => calcSize(i, folders) },
          { label: names.length > 1 ? "Copy paths" : "Copy path", icon: Copy, run: () => copyPath(i, names) },
          { label: "Open terminal here", icon: SquareTerminal, disabled: place === MAC ? "Only on nodes" : one && !isFolder(one) ? "Choose a folder" : false, ...shell,
            run: () => terminalHere(i, one && isFolder(one) ? join(dirOf(i), one.name) : dirOf(i)) },
          { separator: true },
          ev.altKey
            ? { label: "Delete immediately…", icon: Trash, shortcut: "⌥⌫", danger: true, ...lock, run: () => deleteNow(i, names) }
            : { label: "Move to Trash", icon: Trash2, shortcut: "⌫", danger: true, ...lock, run: () => trash(i, names) },
        ];
    showContextMenu(ev, items);
  };

  // ---- drag and drop between panes (pointer events)

  const targetLabel = (t: DropTarget) => {
    const name = placeName(locsRef.current[t.pane as 0 | 1].place);
    return t.folder ? `“${t.folder}”` : t.pane === active ? "this folder" : name;
  };

  const onDragStart = (i: 0 | 1) => (names: string[], start: PointerEvent, collapseTo: string | null) => {
    if (!names.length || !root.current) return;
    const x0 = start.clientX;
    const y0 = start.clientY;
    let ghost: DragGhost | null = null;
    let target: DropTarget | null = null;
    let move = start.altKey;
    const valid = (t: DropTarget | null): DropTarget | null => {
      if (!t) return null;
      if (t.pane === i && (t.folder === null || names.includes(t.folder))) return null; // its own folder
      return t;
    };
    const update = (x: number, y: number) => {
      target = valid(hitTest(root.current!, x, y));
      setDrop(target ? { pane: target.pane, folder: target.folder } : null);
      ghost?.setLabel(move, target ? targetLabel(target) : null);
    };
    let last = { x: x0, y: y0 };
    const onMove = (e: PointerEvent) => {
      last = { x: e.clientX, y: e.clientY };
      if (!ghost) {
        if (Math.hypot(e.clientX - x0, e.clientY - y0) < 5) return;
        const first = entryOf(i, names[0]);
        ghost = makeGhost(names[0], names.length, !!first && isFolder(first));
      }
      move = e.altKey;
      ghost.move(e.clientX, e.clientY);
      update(e.clientX, e.clientY);
    };
    const onKeyChange = (e: KeyboardEvent) => {
      if (e.key === "Escape" && e.type === "keydown") {
        e.preventDefault();
        e.stopPropagation();
        finish(false);
        return;
      }
      if (e.key === "Alt" && ghost) {
        move = e.type === "keydown";
        update(last.x, last.y);
      }
    };
    const finish = (commit: boolean) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("keydown", onKeyChange, true);
      window.removeEventListener("keyup", onKeyChange, true);
      const dragged = !!ghost;
      ghost?.remove();
      setDrop(null);
      if (!dragged) {
        if (collapseTo) setSelFor(i, [collapseTo]);
        return;
      }
      if (!commit || !target) return;
      const t = target as DropTarget;
      const to = t.pane as 0 | 1;
      const toDir = t.folder ? join(dirOf(to), t.folder) : dirOf(to);
      void startTransfer({ from: locsRef.current[i].place, dir: dirOf(i), names, to: locsRef.current[to].place, toDir, op: move ? "move" : "copy", policies: {} });
    };
    const onUp = () => finish(true);
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("keydown", onKeyChange, true);
    window.addEventListener("keyup", onKeyChange, true);
  };

  // ---- drops from Finder

  const finderDrop = useRef<(paths: string[], t: DropTarget) => void>(() => {});
  finderDrop.current = (paths, t) => {
    const to = t.pane as 0 | 1;
    const toDir = t.folder ? join(dirOf(to), t.folder) : dirOf(to);
    const groups = new Map<string, string[]>();
    for (const p of paths) {
      const dir = parentOf(p);
      groups.set(dir, [...(groups.get(dir) ?? []), p.slice(dir === "/" ? 1 : dir.length + 1)]);
    }
    for (const [dir, names] of groups) void startTransfer({ from: MAC, dir, names, to: locsRef.current[to].place, toDir, op: "copy", policies: {} });
  };
  useEffect(() => {
    if (!inTauri) return;
    let unlisten: (() => void) | null = null;
    let alive = true;
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === "leave") return setDrop(null);
        const dpr = window.devicePixelRatio || 1;
        const t = root.current ? hitTest(root.current, p.position.x / dpr, p.position.y / dpr) : null;
        if (p.type === "drop") {
          setDrop(null);
          if (t && p.paths.length) finderDrop.current(p.paths, t);
          return;
        }
        setDrop(t ? { pane: t.pane, folder: t.folder } : null);
      })
      .then((u) => {
        if (alive) unlisten = u;
        else u();
      })
      .catch(() => {});
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  // ---- render

  const i = active;
  const o = other(i);
  const names = selOf(i);
  const one = names.length === 1 ? entryOf(i, names[0]) : null;
  const toOther = placeName(locs[o].place);
  // The Copy and Move buttons are locked only when their own transfer needs an unlock.
  const specFor = (op: "copy" | "move"): TransferSpec => ({ from: locs[i].place, dir: dirOf(i), names, to: locs[o].place, toDir: dirOf(o), op, policies: {} });
  const copyGate = transferGate(specFor("copy"), homes[MAC]);
  const moveGate = transferGate(specFor("move"), homes[MAC]);
  const gateProps = (op: "copy" | "move") => {
    const g = op === "copy" ? copyGate : moveGate;
    // Look-only holds every copy, Downloads included (files_transfer is refused before it runs).
    const locked = names.length > 0 && !g.refused && (!!adminNow.lookOnly || transferLocked(g, adminNow));
    return { locked, refused: g.refused, scope: g.needs.includes("mac") ? ("mac" as const) : ("node" as const), action: names.length ? transferWords(specFor(op)) : undefined, gateTitle: locked ? ` · ${g.reason}` : "" };
  };
  const copyBtn = gateProps("copy");
  const moveBtn = gateProps("move");
  const decision = transfers.find((t) => t.state === "needsDecision" && (t.origin === myId || !t.origin));
  const infoEntry = info ? entryOf(info.pane, info.name) : null;

  return (
    <div className="sb-files col fill" ref={root}>
      <Toolbar label="Files">
        <ToolButton icon={Copy} label={`Copy to ${toOther}`} disabled={!names.length || !!copyBtn.refused} locked={copyBtn.locked} scope={copyBtn.scope} action={copyBtn.action}
          title={copyBtn.refused ?? `Copy the selection into ${dirOf(o)} on ${toOther} (F5)${copyBtn.gateTitle}`} onClick={() => copyToOther(i, "copy")} />
        <ToolButton icon={MoveRight} label={`Move to ${toOther}`} priority={2} disabled={!names.length || !!moveBtn.refused} locked={moveBtn.locked} scope={moveBtn.scope} action={moveBtn.action}
          title={moveBtn.refused ?? `Move the selection into ${dirOf(o)} on ${toOther} (F6)${moveBtn.gateTitle}`} onClick={() => copyToOther(i, "move")} />
        <ToolSeparator />
        <ToolButton icon={FolderPlus} label="New folder" priority={2} locked={!admin} title="New folder (⇧⌘N)" onClick={() => void newFolder(i)} />
        <ToolButton icon={FilePlus} label="New file" priority={2} locked={!admin} disabled={!!newFileOff(i)} title={newFileOff(i) || "New file, opened in an editor"} onClick={() => void newFile(i)} />
        <ToolButton icon={Pencil} label="Rename" priority={3} disabled={names.length !== 1} locked={!admin && names.length === 1} title="Rename (F2)" onClick={() => void rename(i)} />
        <ToolButton icon={FilePen} label="Edit" disabled={!one || isFolder(one)} title="Open in an editor window next to these panes" onClick={() => one && openEditor(locs[i].place, join(dirOf(i), one.name))} />
        <ToolSeparator />
        {alt ? (
          <ToolButton icon={Trash} label="Delete immediately" danger disabled={!names.length} locked={!admin && names.length > 0}
            title="Delete for good, skipping the Trash (⌥⌫)" onClick={() => void deleteNow(i)} />
        ) : (
          <ToolButton icon={Trash2} label="Move to Trash" disabled={!names.length} locked={!admin && names.length > 0}
            title="Move to the Trash; ⌘Z puts it back. Hold ⌥ to delete immediately" onClick={() => void trash(i)} />
        )}
        <HelpTip topic="delete-and-restore-files" />
        <Spacer />
        <ToolButton icon={SquareTerminal} label="Terminal here" priority={3} disabled={locs[i].place === MAC} locked={!admin} action="terminal_open"
          title="Open a shell in this folder" onClick={() => terminalHere(i)} />
        <ToolButton icon={hidden ? EyeOff : Eye} label="Hidden files" active={hidden} priority={3} title={hidden ? "Hide files whose names start with a dot" : "Show files whose names start with a dot"}
          onClick={() => setHidden(!hidden)} />
        <HelpTip topic="desk.files.hidden" />
        <ToolButton icon={ArrowLeftRight} label="Swap" priority={3} title="Swap the two panes" onClick={() => {
          setLocs([locs[1], locs[0]]);
          setViews([views[1], views[0]]);
          setSel([sel[1], sel[0]]);
        }} />
      </Toolbar>
      {layout === "single" && (
        // One-pane mode: the switcher is the place picker. A click on a pane shows it; a click
        // on the one already shown (or its ▾) picks the place it shows.
        <div className="sb-pane-switch">
          <Segmented<string>
            label="Pane"
            value={String(active)}
            onChange={(v) => setActive(v === "1" ? 1 : 0)}
            options={switcherLabels([placeName(locs[0].place), placeName(locs[1].place)], [locs[0].path, locs[1].path]).map((label, k) => ({ value: String(k), label }))}
          />
          <button type="button" className="tool icon-only sb-pane-switch-place" aria-label={`Show another place in this pane (now ${placeName(locs[active].place)})`}
            title="Show another place in this pane"
            onClick={(e) => showContextMenu(e, places.map((p) => ({
              label: `${p.name}${p.id !== MAC && p.status !== "online" ? ` (${p.status})` : ""}`,
              checked: p.id === locs[active].place,
              run: () => go(active, p.id, "~"),
            })))}>
            <ChevronDown size={14} aria-hidden />
          </button>
        </div>
      )}
      <div className={`sb-panes${layout === "single" ? " single" : ""}`}>
        {([0, 1] as const).filter((k) => layout === "split" || k === active).map((k) => (
          <Pane
            key={k}
            index={k}
            active={active === k}
            place={locs[k].place}
            places={places}
            path={locs[k].path}
            listing={views[k].listing}
            loading={views[k].loading}
            error={views[k].error}
            hidden={hidden}
            selection={new Set(sel[k])}
            home={homes[locs[k].place] ?? null}
            canBack={locs[k].back.length > 0}
            canForward={locs[k].fwd.length > 0}
            dropTarget={drop && drop.pane === k ? { folder: drop.folder } : null}
            onFocus={() => active !== k && setActive(k)}
            onGo={(p) => go(k, locs[k].place, p)}
            onPlace={(p) => go(k, p, "~")}
            onBack={() => back(k)}
            onForward={() => forward(k)}
            onUp={() => up(k)}
            onRefresh={() => refresh(k)}
            onSelection={(s) => setSelFor(k, [...s])}
            onOpen={(e) => open(k, e)}
            onContextMenu={(e, ev, s) => rowMenu(k, e, ev, s)}
            onKeyDownCapture={onKey(k)}
            onDragStart={onDragStart(k)}
            onDismissError={() => patchView(k, { error: null })}
            onCopyEvent={onCopy(k)}
            onPasteEvent={onPaste(k)}
            placePicker={layout === "split"}
          />
        ))}
      </div>
      <TransferQueue />
      {decision && <ConflictSheet key={decision.id} t={decision} />}
      {info && infoEntry && (
        <InfoPopover
          entry={infoEntry}
          path={join(dirOf(info.pane), infoEntry.name)}
          placeName={placeName(locs[info.pane].place)}
          size={sizes.get(sizeKey(locs[info.pane].place, join(dirOf(info.pane), infoEntry.name)))}
          anchor={info.rect}
          onClose={() => setInfo(null)}
          onCalculate={() => void calcSize(info.pane, [infoEntry.name])}
        />
      )}
    </div>
  );
}
