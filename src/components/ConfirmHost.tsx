import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { ChevronRight, Copy, Info, OctagonAlert, TriangleAlert } from "lucide-react";
import { useDialogQueue, type DialogRequest } from "../lib/session";
import { windowElement } from "../wm/WindowManager";
import "./shell.css";

/**
 * Shows queued dialogs. App-wide ones are modal over everything (the app goes
 * inert behind them); ones asked for with `winId` are sheets on that window and
 * only block it. Each target shows one dialog at a time, in order.
 */
export function ConfirmHost() {
  const queue = useDialogQueue();
  const sheets = new Map<string, { req: DialogRequest; host: HTMLElement }>();
  let modal: DialogRequest | null = null;
  for (const r of queue) {
    const host = r.winId ? windowElement(r.winId) : null;
    if (host && host.style.display !== "none") {
      if (!sheets.has(r.winId!)) sheets.set(r.winId!, { req: r, host });
    } else if (!modal) {
      modal = r;
    }
  }
  return (
    <>
      {[...sheets.values()].map(({ req, host }) => createPortal(<Dialog key={req.id} req={req} mode="sheet" host={host} />, host))}
      {modal && createPortal(<Dialog key={modal.id} req={modal} mode="modal" host={null} />, document.body)}
    </>
  );
}

// Several dialogs can make the same element inert; count them.
const inertCount = new WeakMap<Element, number>();
function holdInert(el: Element | null | undefined): () => void {
  if (!el) return () => {};
  inertCount.set(el, (inertCount.get(el) ?? 0) + 1);
  el.setAttribute("inert", "");
  return () => {
    const n = (inertCount.get(el) ?? 1) - 1;
    inertCount.set(el, n);
    if (n <= 0) el.removeAttribute("inert");
  };
}

const FOCUSABLE = "button:not(:disabled), input:not(:disabled), textarea:not(:disabled), summary, [href], [tabindex]:not([tabindex='-1'])";

function Dialog({ req, mode, host }: { req: DialogRequest; mode: "modal" | "sheet"; host: HTMLElement | null }) {
  const ref = useRef<HTMLDivElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const detailsRef = useRef<HTMLDivElement>(null);
  const [typed, setTyped] = useState("");
  const [value, setValue] = useState(req.prompt?.initial ?? "");
  const [copied, setCopied] = useState(false);
  const titleId = `dlg-title-${req.id}`;
  const bodyId = `dlg-body-${req.id}`;

  const isAsk = req.kind === "confirm" || req.kind === "prompt";
  const typingOk = !req.requireTyping || typed.trim() === req.requireTyping;
  const invalid = req.kind === "prompt" ? (value.trim() ? (req.prompt?.validate?.(value.trim()) ?? null) : "") : null;
  const canConfirm = typingOk && invalid === null;

  const cancel = () => req.settle(false);
  const confirm = () => {
    if (!canConfirm) return;
    req.settle(true, req.kind === "prompt" ? value.trim() : undefined);
  };

  // Everything behind a modal goes inert; a sheet only blocks its own window's content.
  useLayoutEffect(() => {
    const target = mode === "modal" ? document.querySelector(".app") : host?.querySelector(":scope > .win-body");
    return holdInert(target);
  }, [mode, host]);

  // Initial focus: the field to type in, else Cancel for destructive asks (Return can't fire them), else the default button.
  useEffect(() => {
    const first = req.requireTyping || req.kind === "prompt" ? inputRef.current : req.danger && isAsk ? cancelRef.current : confirmRef.current;
    first?.focus({ preventScroll: true });
    if (req.kind === "prompt") inputRef.current?.select();
    const opener = req.opener as HTMLElement | null;
    return () => {
      // Back to whatever had focus before, once the next frame has settled.
      window.requestAnimationFrame(() => {
        if (opener?.isConnected && !opener.closest("[inert]")) opener.focus?.({ preventScroll: true });
      });
    };
  }, []);

  // Esc cancels a modal even when focus has wandered off it.
  useEffect(() => {
    if (mode !== "modal") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        cancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [mode]);

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      cancel();
    } else if (e.key === "Tab") {
      // Keep focus inside the dialog.
      const items = [...(ref.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
      if (!items.length) return;
      const i = items.indexOf(document.activeElement as HTMLElement);
      const next = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : (i === items.length - 1 ? 0 : i + 1);
      e.preventDefault();
      items[next].focus();
    } else if (e.key === "Enter" && e.target instanceof HTMLInputElement) {
      e.preventDefault();
      confirm();
    }
  };

  const copyDetails = async () => {
    const text = typeof req.details === "string" ? req.details : (detailsRef.current?.innerText ?? "");
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  const Icon = req.kind === "error" ? OctagonAlert : req.danger ? TriangleAlert : Info;
  const tone = req.kind === "error" || req.danger ? "crit" : "info";

  const card = (
    <div
      ref={ref}
      className={`sb-dialog ${mode}`}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={req.body ? bodyId : undefined}
      onKeyDown={onKeyDown}
    >
      <Icon className={`sb-dialog-icon ${tone}`} size={32} strokeWidth={1.75} aria-hidden />
      <div className="sb-dialog-main">
        <h2 id={titleId} className="sb-dialog-title">{req.title}</h2>
        {req.body && <div id={bodyId} className="sb-dialog-body">{req.body}</div>}
        {req.requireTyping && (
          <label className="sb-dialog-field">
            <span>Type <b className="mono">{req.requireTyping}</b> to confirm</span>
            <input
              ref={inputRef}
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              aria-invalid={typed.length > 0 && !typingOk}
            />
          </label>
        )}
        {req.prompt && (
          <label className="sb-dialog-field">
            <span>{req.prompt.label}</span>
            <input
              ref={inputRef}
              value={value}
              placeholder={req.prompt.placeholder}
              onChange={(e) => setValue(e.target.value)}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={!!invalid}
            />
            {invalid && <span className="sb-dialog-error">{invalid}</span>}
          </label>
        )}
        {req.details && (
          <details className="sb-dialog-details">
            <summary><ChevronRight size={12} className="sb-disclosure" aria-hidden />Details</summary>
            <div className="sb-dialog-details-box">
              <div ref={detailsRef} className="sb-dialog-details-text">{req.details}</div>
              <button className="btn sb-dialog-copy" onClick={copyDetails}>
                <Copy size={12} aria-hidden /> {copied ? "Copied" : "Copy"}
              </button>
            </div>
          </details>
        )}
        {req.alternative ? (
          // Two ways to go ahead, with long labels: stacked full width, the default on top, Cancel last.
          <div className="sb-dialog-actions stacked">
            <button ref={confirmRef} className={`btn ${req.danger ? "danger" : "primary"}`} disabled={!canConfirm} onClick={confirm}>{req.confirmLabel}</button>
            <button className="btn" disabled={!typingOk} onClick={() => typingOk && req.settle(true, "alternative")}>{req.alternative.label}</button>
            <button ref={cancelRef} className="btn" onClick={cancel}>{req.cancelLabel ?? "Cancel"}</button>
          </div>
        ) : (
          <div className="sb-dialog-actions">
            {req.kind === "error" && req.actions?.map((a) => (
              <button
                key={a.label}
                className="btn"
                onClick={() => {
                  req.settle(true);
                  void a.run();
                }}
              >
                {a.label}
              </button>
            ))}
            <span className="spacer" />
            {isAsk && (
              <button ref={cancelRef} className="btn" onClick={cancel}>{req.cancelLabel ?? "Cancel"}</button>
            )}
            <button
              ref={confirmRef}
              className={`btn ${req.danger ? "danger" : "primary"}`}
              disabled={!canConfirm}
              onClick={confirm}
            >
              {req.confirmLabel}
            </button>
          </div>
        )}
      </div>
    </div>
  );

  if (mode === "sheet") {
    return (
      <div className="sb-sheet-scrim" data-shell-overlay onPointerDown={(e) => e.target === e.currentTarget && !req.danger && isAsk && cancel()}>
        {card}
      </div>
    );
  }
  return (
    <div
      className="sb-overlay"
      data-shell-overlay
      // Destructive asks ignore clicks outside: only a button press decides them.
      onPointerDown={(e) => e.target === e.currentTarget && !req.danger && isAsk && cancel()}
    >
      {card}
    </div>
  );
}
