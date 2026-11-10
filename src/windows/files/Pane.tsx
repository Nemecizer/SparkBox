import { useMemo, useRef, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { ArrowLeft, ArrowRight, ArrowUp, File, FileCode2, FileText, Folder, FolderOpen, Link2, RefreshCw, Star, X } from "lucide-react";
import { LoadingState, StatusDot } from "../../components/ui";
import { Table, type Column } from "../../components/Table";
import { MAC, type FileEntry, type Listing } from "../../lib/filesApi";
import { fmtBytes, fmtDateTime } from "../../lib/format";
import type { LinkStatus } from "../../lib/types";
import { Breadcrumbs } from "./Breadcrumbs";
import { isFolder, join, tildify } from "./paths";
import { addFavorite, removeFavorite, resetFavorites, sizeKey, useFavorites, useSizes } from "./stores";
import { showContextMenu } from "../../lib/contextMenu";

export interface PlaceInfo {
  id: string;
  name: string;
  status: LinkStatus;
}

const CODE_EXT = /\.(ya?ml|json|py|sh|toml|conf|ini|service|cfg|env)$/i;
const TEXT_EXT = /\.(txt|md|log|csv)$/i;

function FileIcon({ e }: { e: FileEntry }) {
  if (e.kind === "link") return e.linkDir ? <Folder size={14} className="ico-dir" /> : <File size={14} className="ico-file" />;
  if (e.kind === "dir") return <Folder size={14} className="ico-dir" />;
  if (CODE_EXT.test(e.name)) return <FileCode2 size={14} className="ico-file" />;
  if (TEXT_EXT.test(e.name)) return <FileText size={14} className="ico-file" />;
  return <File size={14} className="ico-file" />;
}

export function Pane({
  index, active, place, places, path, listing, loading, error, hidden, selection, home, canBack, canForward, dropTarget,
  onFocus, onGo, onPlace, onBack, onForward, onUp, onRefresh, onSelection, onOpen, onContextMenu, onKeyDownCapture, onDragStart,
  onDismissError, onCopyEvent, onPasteEvent, placePicker = true,
}: {
  index: 0 | 1;
  /** False in one-pane mode, where the pane switcher above is the place picker. */
  placePicker?: boolean;
  active: boolean;
  place: string;
  places: PlaceInfo[];
  path: string;
  listing: Listing | null;
  loading: boolean;
  error: string | null;
  hidden: boolean;
  selection: ReadonlySet<string>;
  home: string | null;
  canBack: boolean;
  canForward: boolean;
  /** Highlighted while something is dragged over this pane: null for the pane itself, else a folder row. */
  dropTarget: { folder: string | null } | null;
  onFocus: () => void;
  onGo: (path: string) => void;
  onPlace: (place: string) => void;
  onBack: () => void;
  onForward: () => void;
  onUp: () => void;
  onRefresh: () => void;
  onSelection: (names: Set<string>) => void;
  onOpen: (e: FileEntry) => void;
  onContextMenu: (e: FileEntry | null, ev: ReactMouseEvent, selected: Set<string>) => void;
  onKeyDownCapture: (e: ReactKeyboardEvent) => void;
  onDragStart: (names: string[], e: PointerEvent, collapseTo: string | null) => void;
  onDismissError: () => void;
  onCopyEvent: (e: ClipboardEvent) => void;
  onPasteEvent: (e: ClipboardEvent) => void;
}) {
  const sizes = useSizes();
  const favs = useFavorites(place);
  const placeInfo = places.find((p) => p.id === place);
  const current = listing?.path ?? path;
  const preserve = useRef(false);

  const rows = useMemo(
    () => (listing?.entries ?? []).filter((e) => hidden || !e.name.startsWith(".")),
    [listing, hidden],
  );

  const sizeOf = (e: FileEntry) => (listing ? sizes.get(sizeKey(place, join(listing.path, e.name))) : undefined);
  const columns: Column<FileEntry>[] = [
    {
      key: "name",
      label: "Name",
      minWidth: 140,
      sort: (e) => e.name,
      title: (e) => (e.target ? `${e.name} → ${e.target}` : e.name),
      render: (e) => (
        <span className="sb-file-name" data-folder={isFolder(e) ? "" : undefined}>
          <FileIcon e={e} />
          <span className="sb-file-label">{e.name}</span>
          {e.kind === "link" && <span className="sb-alias" title={`Alias to ${e.target ?? "?"}`}><Link2 size={10} />alias</span>}
        </span>
      ),
    },
    {
      key: "size",
      label: "Size",
      align: "right",
      width: 92,
      // Folders by their calculated size, or −1 while it's unknown.
      sort: (e) => (e.kind === "link" ? -1 : isFolder(e) ? (typeof sizeOf(e) === "number" ? (sizeOf(e) as number) : -1) : e.size),
      render: (e) => {
        if (e.kind === "link") return <span className="muted">—</span>;
        if (!isFolder(e)) return <span className="num">{fmtBytes(e.size)}</span>;
        const s = sizeOf(e);
        return s === "busy" ? <span className="muted">Calculating…</span> : typeof s === "number" ? <span className="num">{fmtBytes(s)}</span> : <span className="muted">—</span>;
      },
    },
    { key: "mtime", label: "Modified", width: 150, priority: 2, sort: (e) => e.mtime, render: (e) => <span className="num">{fmtDateTime(e.mtime * 1000)}</span> },
    { key: "mode", label: "Permissions", width: 96, priority: 3, render: (e) => <span className="mono">{e.mode}</span> },
    { key: "owner", label: "Owner", width: 80, priority: 3, sort: (e) => e.owner ?? "", render: (e) => e.owner ?? "—" },
  ];

  // Footer: "12 items · 3 selected (4.1 GiB) · 2.4 TiB free".
  const selected = rows.filter((e) => selection.has(e.name));
  let selBytes = 0;
  let unknownDirs = 0;
  for (const e of selected) {
    if (e.kind === "link") continue;
    if (isFolder(e)) {
      const s = sizeOf(e);
      if (typeof s === "number") selBytes += s;
      else unknownDirs++;
    } else selBytes += e.size;
  }
  const hiddenCount = (listing?.entries.length ?? 0) - rows.length;
  const favorite = favs.includes(tildify(current, home)) || favs.includes(current);

  const onPointerDownCapture = (e: ReactPointerEvent) => {
    onFocus();
    if (e.button !== 0 || e.shiftKey || e.metaKey || e.ctrlKey) return;
    const row = (e.target as HTMLElement).closest<HTMLElement>("tr[data-key]");
    const key = row?.dataset.key;
    if (!key) return;
    // Pressing on an already selected item keeps the selection (so several can be dragged);
    // a click without a drag narrows it to that item, as in Finder.
    preserve.current = selection.has(key) && selection.size > 1;
    const names = selection.has(key) ? rows.filter((r) => selection.has(r.name)).map((r) => r.name) : [key];
    onDragStart(names, e.nativeEvent, preserve.current ? key : null);
  };

  const isDropPane = dropTarget && dropTarget.folder === null;
  return (
    <section
      className={`sb-pane${active ? " active" : ""}${isDropPane ? " drop" : ""}`}
      data-pane={index}
      aria-label={`${placeInfo?.name ?? place} pane`}
      onPointerDownCapture={onPointerDownCapture}
      onMouseDownCapture={(e) => {
        if (preserve.current) e.stopPropagation();
        preserve.current = false;
      }}
      onKeyDownCapture={onKeyDownCapture}
      onCopy={(e) => onCopyEvent(e.nativeEvent)}
      onPaste={(e) => onPasteEvent(e.nativeEvent)}
      onContextMenu={(e) => {
        if (!(e.target as HTMLElement).closest("tr[data-key], input, select, button")) onContextMenu(null, e, new Set());
      }}
    >
      <div className="sb-pane-bar">
        {placePicker && (
          <select className="sb-pane-place" value={place} onChange={(e) => onPlace(e.target.value)} aria-label="Location">
            {places.map((p) => <option key={p.id} value={p.id}>{p.name}{p.id !== MAC && p.status !== "online" ? ` (${p.status})` : ""}</option>)}
          </select>
        )}
        {placePicker && place !== MAC && placeInfo && <StatusDot status={placeInfo.status} />}
        <button type="button" className="tool icon-only sb-pane-hist" title="Back (⌘[)" aria-label="Back" disabled={!canBack} onClick={onBack}><ArrowLeft size={14} /></button>
        <button type="button" className="tool icon-only sb-pane-hist" title="Forward (⌘])" aria-label="Forward" disabled={!canForward} onClick={onForward}><ArrowRight size={14} /></button>
        <button type="button" className="tool icon-only" title="Enclosing folder (⌘↑)" aria-label="Enclosing folder" disabled={current === "/"} onClick={onUp}><ArrowUp size={14} /></button>
        <Breadcrumbs
          path={current}
          home={home}
          onGo={onGo}
          favorite={favorite}
          onToggleFavorite={() => (favorite ? removeFavorite(place, favs.find((f) => f === current || f === tildify(current, home))!) : addFavorite(place, tildify(current, home)))}
        />
        <button type="button" className="tool icon-only" title="Refresh (⌘R)" aria-label="Refresh" onClick={onRefresh}><RefreshCw size={14} className={loading ? "spin" : undefined} /></button>
      </div>
      <div className="sb-pane-favs" role="toolbar" aria-label="Favorites">
        {favs.map((f) => {
          const on = f === current || f === tildify(current, home);
          return (
            <button key={f} type="button" className={`sb-fav${on ? " on" : ""}`} onClick={() => onGo(f)} title={f}
              onContextMenu={(e) => showContextMenu(e, [
                { label: "Open", icon: FolderOpen, run: () => onGo(f) },
                { label: "Remove from favorites", icon: Star, run: () => removeFavorite(place, f) },
                { separator: true },
                { label: "Reset favorites", run: () => resetFavorites(place) },
              ])}>
              {f === "~" ? <Star size={11} /> : <FolderOpen size={11} />}
              {f === "~" ? "Home" : f.replace(/^~\//, "").split("/").pop() || f}
            </button>
          );
        })}
      </div>
      <div className="sb-pane-body">
        {loading && listing && <div className="sb-pane-busy" role="progressbar" aria-label="Loading" />}
        {error && (
          <div className="sb-pane-error" role="alert">
            <span>{error}</span>
            <button type="button" className="notice-close" onClick={onDismissError} aria-label="Dismiss" title="Dismiss"><X size={12} /></button>
          </div>
        )}
        {place !== MAC && placeInfo && placeInfo.status !== "online" && !listing && !loading && (
          <div className="sb-pane-offline muted">{placeInfo.name} is {placeInfo.status === "connecting" ? "connecting" : "offline"}. Its files show once it's back.</div>
        )}
        <Table
          columns={columns}
          rows={rows}
          rowKey={(e) => e.name}
          selectionMode="multi"
          selection={selection}
          onSelectionChange={(keys) => onSelection(keys)}
          onActivate={onOpen}
          onRowContextMenu={(row, ev, sel) => onContextMenu(row, ev, sel)}
          tableId={`files.${index}`}
          pinFirst={isFolder}
          rowClass={(e) => [dropTarget?.folder === e.name && "sb-drop-row", e.name.startsWith(".") && "sb-dot-row"].filter(Boolean).join(" ") || undefined}
          ariaLabel={`Files in ${current}`}
          empty={!listing && loading ? <LoadingState rows={8} label="Loading folder" /> : error && !listing ? "" : hiddenCount > 0 ? `Only hidden files here (${hiddenCount})` : "Empty folder"}
          initialSort={{ key: "name", dir: 1 }}
        />
      </div>
      <div className="sb-pane-foot" role="status">
        <span>
          {rows.length.toLocaleString()} item{rows.length === 1 ? "" : "s"}
          {hiddenCount > 0 && !hidden ? ` (${hiddenCount} hidden)` : ""}
          {selected.length > 0 && ` · ${selected.length} selected (${fmtBytes(selBytes)}${unknownDirs ? ` + ${unknownDirs} folder${unknownDirs === 1 ? "" : "s"}` : ""})`}
        </span>
        <span className="spacer" />
        {listing?.free != null && <span>{fmtBytes(listing.free, { decimals: 1 })} free{listing.capacity ? ` of ${fmtBytes(listing.capacity, { decimals: 1 })}` : ""}</span>}
      </div>
    </section>
  );
}
