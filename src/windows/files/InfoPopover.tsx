import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { File, Folder, Link2 } from "lucide-react";
import type { FileEntry } from "../../lib/filesApi";
import { fmtBytes, fmtDateTime } from "../../lib/format";
import { isFolder, octal } from "./paths";

/** Space (or ⌘I) on a selected item: what it is, how big, its permissions, owner and date. */
export function InfoPopover({ entry, path, placeName, size, anchor, onClose, onCalculate }: {
  entry: FileEntry;
  path: string;
  placeName: string;
  /** A folder's size from Calculate size, "busy" while it runs. */
  size: number | "busy" | undefined;
  anchor: DOMRect | null;
  onClose: () => void;
  onCalculate: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const a = anchor ?? new DOMRect(window.innerWidth / 2 - 140, 120, 280, 0);
    let top = a.bottom + 6;
    if (top + r.height > window.innerHeight - 8) top = Math.max(8, a.top - r.height - 6);
    const left = Math.max(8, Math.min(a.left + 24, window.innerWidth - r.width - 8));
    setPos({ top, left });
  }, [anchor]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === " ") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onDown, true);
    };
  }, [onClose]);

  const folder = isFolder(entry);
  const Icon = entry.kind === "link" ? Link2 : folder ? Folder : File;
  const kind = entry.kind === "link" ? `Alias to a ${entry.linkDir ? "folder" : "file"}` : folder ? "Folder" : "File";
  return (
    <div ref={ref} className="sb-info" role="dialog" aria-label={`Info for ${entry.name}`} style={pos ? { top: pos.top, left: pos.left } : { visibility: "hidden" }}>
      <div className="sb-info-head">
        <Icon size={28} className={folder ? "ico-dir" : "ico-file"} />
        <div>
          <div className="sb-info-name">{entry.name}</div>
          <div className="muted">{kind} · {placeName}</div>
        </div>
      </div>
      <dl className="sb-info-grid">
        <dt>Size</dt>
        <dd className="num">
          {entry.kind === "link"
            ? "—"
            : folder
              ? size === "busy" ? "Calculating…" : size !== undefined ? `${fmtBytes(size)} (${size.toLocaleString()} bytes)` : <button type="button" className="btn small" onClick={onCalculate}>Calculate</button>
              : `${fmtBytes(entry.size)} (${entry.size.toLocaleString()} bytes)`}
        </dd>
        <dt>Where</dt>
        <dd className="mono sb-info-path">{path}</dd>
        {entry.target && (
          <>
            <dt>Points to</dt>
            <dd className="mono sb-info-path">{entry.target}</dd>
          </>
        )}
        <dt>Modified</dt>
        <dd>{fmtDateTime(entry.mtime * 1000)}</dd>
        <dt>Permissions</dt>
        <dd><span className="mono">{entry.mode}</span> <span className="muted">({octal(entry.mode)})</span></dd>
        <dt>Owner</dt>
        <dd>{entry.owner ?? "—"}</dd>
      </dl>
      <div className="sb-info-foot muted">Space or Esc to close</div>
    </div>
  );
}
