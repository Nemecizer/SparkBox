import { useEffect, useState } from "react";
import { CircleCheck, Info, LoaderCircle, OctagonAlert, X } from "lucide-react";
import { dismissToast, pauseToast, resumeToast, useToasts, visibleToasts, type Toast, type ToastKind } from "../lib/toasts";
import "./shell.css";

const ICON: Record<ToastKind, typeof Info> = { success: CircleCheck, error: OctagonAlert, info: Info, progress: LoaderCircle };
const KIND_LABEL: Record<ToastKind, string> = { success: "Done", error: "Error", info: "Note", progress: "In progress" };

/**
 * Toasts at the bottom right of the desktop, above the status bar (and the dock when it
 * shows), the newest at the bottom. Rendered inside the desktop so they never cover the
 * top bar or a window's toolbar. Three at most; "N more" opens the rest.
 */
export function Toasts() {
  const toasts = useToasts();
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    if (toasts.length <= 3) setExpanded(false);
  }, [toasts.length]);
  if (!toasts.length) return null;
  const { shown, more } = visibleToasts(toasts, expanded);
  return (
    <div className={`toasts${expanded ? " expanded" : ""}`} aria-label="Notifications" data-shell-overlay>
      {more > 0 && (
        <button type="button" className="toast-more" onClick={() => setExpanded(true)} aria-label={`Show ${more} more notification${more === 1 ? "" : "s"}`}>
          {more} more
        </button>
      )}
      {expanded && toasts.length > 3 && (
        <button type="button" className="toast-more" onClick={() => setExpanded(false)}>Show fewer</button>
      )}
      {shown.map((t) => <ToastCard key={t.id} t={t} />)}
    </div>
  );
}

function ToastCard({ t }: { t: Toast }) {
  const Icon = ICON[t.kind];
  const run = async (fn: () => unknown) => {
    dismissToast(t.id);
    try {
      await fn();
    } catch (e) {
      console.error(e);
    }
  };
  return (
    <div
      className={`toast toast-${t.kind}`}
      role={t.kind === "error" ? "alert" : "status"}
      onPointerEnter={() => pauseToast(t.id)}
      onPointerLeave={() => resumeToast(t.id)}
      onFocus={() => pauseToast(t.id)}
      onBlur={() => resumeToast(t.id)}
    >
      <Icon className={`toast-icon${t.kind === "progress" ? " spin" : ""}`} size={16} aria-label={KIND_LABEL[t.kind]} />
      <div className="toast-main">
        <div className="toast-title">{t.title}</div>
        {t.body && <div className="toast-body">{t.body}</div>}
        {t.kind === "progress" && (
          <div
            className={`toast-bar${t.progress == null ? " indeterminate" : ""}`}
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={t.progress == null ? undefined : Math.round(t.progress * 100)}
          >
            <i style={t.progress == null ? undefined : { width: `${Math.max(0, Math.min(1, t.progress)) * 100}%` }} />
          </div>
        )}
        {(t.undo || t.actions?.length) && (
          <div className="toast-actions">
            {t.undo && <button className="toast-btn strong" onClick={() => run(t.undo!)}>Undo</button>}
            {t.actions?.map((a) => <button key={a.label} className="toast-btn" onClick={() => run(a.run)}>{a.label}</button>)}
          </div>
        )}
      </div>
      <button className="toast-close" aria-label="Dismiss" title="Dismiss" onClick={() => dismissToast(t.id)}>
        <X size={12} aria-hidden />
      </button>
    </div>
  );
}
