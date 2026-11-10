import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ChevronRight, Ellipsis, Folder, Star } from "lucide-react";
import { showContextMenu } from "../../lib/contextMenu";
import { segments } from "./paths";

/**
 * The folder path as clickable segments. When they don't fit, the middle ones fold into a
 * "…" menu. Clicking the last segment (or the empty space after it) edits the path as text.
 */
export function Breadcrumbs({ path, home, onGo, favorite, onToggleFavorite }: {
  path: string;
  home: string | null;
  onGo: (path: string) => void;
  favorite: boolean;
  onToggleFavorite: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [typed, setTyped] = useState(path);
  const [fold, setFold] = useState(0);
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const all = segments(path);
  // The home folder reads "~" (one segment) instead of /home/sparkuser.
  const homeAt = home ? all.findIndex((s) => s.path === home) : -1;
  const segs = homeAt > 0 ? [{ name: "~", path: home! }, ...all.slice(homeAt + 1)] : all;

  useEffect(() => {
    setTyped(path);
    setFold(0);
  }, [path]);
  useEffect(() => {
    if (editing) input.current?.select();
  }, [editing]);

  // Fold more of the middle until the crumbs fit.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el || editing) return;
    if (el.scrollWidth > el.clientWidth + 1 && fold < segs.length - 2) setFold((f) => f + 1);
  });
  useEffect(() => {
    const el = box.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setFold(0));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  if (editing) {
    return (
      <input
        ref={input}
        className="sb-crumbs-input mono"
        value={typed}
        aria-label="Folder path"
        spellCheck={false}
        onChange={(e) => setTyped(e.target.value)}
        onBlur={() => setEditing(false)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            setEditing(false);
            if (typed.trim()) onGo(typed.trim());
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            setEditing(false);
          }
          e.stopPropagation();
        }}
      />
    );
  }

  const first = segs.slice(0, 1);
  const hidden = fold > 0 ? segs.slice(1, 1 + fold) : [];
  const rest = segs.slice(1 + fold);
  const crumb = (s: { name: string; path: string }, last: boolean) => (
    <button
      key={s.path}
      type="button"
      className={`sb-crumb${last ? " last" : ""}`}
      title={last ? "Click to type a path" : s.path}
      onClick={() => (last ? setEditing(true) : onGo(s.path))}
    >
      {s.name}
    </button>
  );

  return (
    <div className={`sb-crumbs${fold >= segs.length - 2 ? " tight" : ""}`} ref={box} onDoubleClick={(e) => { if (e.target === e.currentTarget) setEditing(true); }}>
      {first.map((s) => crumb(s, segs.length === 1))}
      {hidden.length > 0 && (
        <>
          <ChevronRight size={11} className="sb-crumb-sep" aria-hidden />
          <button
            type="button"
            className="sb-crumb more"
            aria-label="More folders"
            title="More folders"
            onClick={(e) => showContextMenu(e, hidden.map((s) => ({ label: s.name, icon: Folder, run: () => onGo(s.path) })))}
          >
            <Ellipsis size={12} />
          </button>
        </>
      )}
      {rest.map((s, i) => (
        <span key={s.path} className="sb-crumb-wrap">
          <ChevronRight size={11} className="sb-crumb-sep" aria-hidden />
          {crumb(s, i === rest.length - 1)}
        </span>
      ))}
      <button type="button" className="sb-crumbs-fill" aria-label="Type a path" tabIndex={-1} onClick={() => setEditing(true)} />
      <button
        type="button"
        className={`sb-crumb-star${favorite ? " on" : ""}`}
        title={favorite ? "Remove from favorites" : "Add this folder to favorites"}
        aria-pressed={favorite}
        aria-label="Favorite"
        onClick={onToggleFavorite}
      >
        <Star size={12} />
      </button>
    </div>
  );
}
