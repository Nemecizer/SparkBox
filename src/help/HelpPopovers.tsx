// The two things that open over a Help page: a term's definition, beside the word, and a picture
// at full size. Both close with Esc or a click away, and give focus back to what opened them.
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { Icon } from "../components/Icon";

/** Esc closes, and nothing behind hears it. */
function useEscape(onClose: () => void) {
  const latest = useRef(onClose);
  latest.current = onClose;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      latest.current();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
}

/** A definition beside its word. Rendered inside the page, so its links reach the page's handler. */
export function TermPopover({ term, html, from, onClose, children }: { term: string; html: string; from: HTMLElement; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  useEscape(onClose);

  useLayoutEffect(() => {
    const pop = ref.current;
    if (!pop) return;
    const b = from.getBoundingClientRect();
    const { offsetWidth: w, offsetHeight: h } = pop;
    const left = Math.max(8, Math.min(b.left, window.innerWidth - w - 8));
    const below = b.bottom + 6;
    const top = below + h > window.innerHeight - 8 && b.top - h - 6 > 8 ? b.top - h - 6 : below;
    setPos({ left, top: Math.max(8, Math.min(top, window.innerHeight - h - 8)) });
  }, [from, html]);

  useEffect(() => {
    if (pos) ref.current?.focus({ preventScroll: true });
  }, [!!pos, html]);

  useEffect(() => {
    const away = (e: Event) => {
      if (!(e.target instanceof Node) || ref.current?.contains(e.target) || from.contains(e.target)) return;
      onClose();
    };
    window.addEventListener("pointerdown", away, true);
    return () => window.removeEventListener("pointerdown", away, true);
  }, [from, onClose]);

  useEffect(() => () => {
    if (from.isConnected) from.focus({ preventScroll: true });
  }, [from]);

  return createPortal(
    <div
      ref={ref}
      className="infotip-pop help-term-pop"
      role="dialog"
      aria-label={term}
      tabIndex={-1}
      style={pos ? { left: pos.left, top: pos.top } : { visibility: "hidden", left: 0, top: 0 }}
    >
      <p className="helptip-title">{term}</p>
      <div className="help-term-text" dangerouslySetInnerHTML={{ __html: html }} />
      {children}
    </div>,
    document.body,
  );
}

/** A picture at full size, over the whole window. */
export function PictureView({ src, caption, from, onClose }: { src: string; caption: string; from: HTMLElement; onClose: () => void }) {
  const close = useRef<HTMLButtonElement>(null);
  useEscape(onClose);
  useEffect(() => {
    // Everything behind is out of reach while it's open. Close is the only control, so Tab stays there.
    const behind = document.getElementById("root");
    behind?.setAttribute("inert", "");
    const stay = (e: KeyboardEvent) => {
      if (e.key !== "Tab") return;
      e.preventDefault();
      close.current?.focus({ preventScroll: true });
    };
    window.addEventListener("keydown", stay, true);
    close.current?.focus({ preventScroll: true });
    return () => {
      window.removeEventListener("keydown", stay, true);
      behind?.removeAttribute("inert");
      if (from.isConnected) from.focus({ preventScroll: true });
    };
  }, [from]);
  return createPortal(
    <div className="help-zoom" role="dialog" aria-modal="true" aria-label={caption} onClick={onClose}>
      <button ref={close} type="button" className="icon-btn help-zoom-close" aria-label="Close" title="Close (Esc)" onClick={onClose}>
        <Icon icon={X} size={16} />
      </button>
      <img src={src} alt={caption} />
      <p>{caption}</p>
    </div>,
    document.body,
  );
}
