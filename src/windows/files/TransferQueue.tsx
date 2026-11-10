import { useState } from "react";
import { ArrowRight, Ban, ChevronDown, ChevronUp, CircleAlert, CircleCheck, Clock, LoaderCircle, RotateCw, X } from "lucide-react";
import { HelpTip, Meter } from "../../components/ui";
import { fmtByteRate, fmtBytes, fmtDuration, GiB } from "../../lib/format";
import { MAC } from "../../lib/filesApi";
import { confirmAction } from "../../lib/session";
import { useCluster } from "../../lib/store";
import { useDataSource } from "../../lib/dataSource";
import {
  canRetry, clearFinishedTransfers, idleSummary, isActive, isFinished, removeTransfer, retryTransfer, routeLabel, stopTransfer, useTransfers, type Transfer,
} from "../../lib/transfers";

function StateIcon({ t }: { t: Transfer }) {
  switch (t.state) {
    case "running":
    case "checking":
      return <LoaderCircle size={13} className="spin sb-tq-ico running" aria-label={t.state === "running" ? "Running" : "Checking"} />;
    case "queued":
    case "needsDecision":
      return <Clock size={13} className="sb-tq-ico waiting" aria-label={t.state === "queued" ? "Queued" : "Waiting for your choice"} />;
    case "done":
      return <CircleCheck size={13} className="sb-tq-ico done" aria-label="Done" />;
    case "failed":
      return <CircleAlert size={13} className="sb-tq-ico failed" aria-label="Failed" />;
    default:
      return <Ban size={13} className="sb-tq-ico cancelled" aria-label="Stopped" />;
  }
}

/** "1.2 of 4.0 GiB · 312 of 1,024 items · 410 MB/s · 7 s left" */
function detail(t: Transfer): string {
  if (t.state === "checking") return "Checking sizes and names…";
  if (t.state === "needsDecision") return "Waiting: some names are already taken";
  if (t.state === "queued") return `Queued (two at a time per route) · ${fmtBytes(t.total)}`;
  const parts = [`${fmtBytes(t.done)} of ${fmtBytes(t.total)}`];
  if (t.filesTotal) parts.push(`${t.files.toLocaleString()} of ${t.filesTotal.toLocaleString()} items`);
  if (t.rateBps) parts.push(fmtByteRate(t.rateBps));
  if (t.etaS !== null && t.etaS !== undefined && t.rateBps) parts.push(t.etaS < 1 ? "finishing" : `${fmtDuration(t.etaS)} left`);
  return parts.join(" · ");
}

async function stop(t: Transfer) {
  if (t.state === "running" && t.total > GiB) {
    const ok = await confirmAction({
      title: `Stop copying ${t.what}?`,
      body: `${fmtBytes(t.done)} of ${fmtBytes(t.total)} has arrived. Stopping removes it from ${t.toName}; nothing there is changed.`,
      confirmLabel: "Stop",
      danger: true,
    });
    if (!ok) return;
  }
  stopTransfer(t.id);
}

/**
 * The transfer queue under the panes: what's running, waiting and done, newest first, for the
 * data source in use only. While nothing runs it's one line (the history opens on a click),
 * so idle Files keeps its room for the panes.
 */
export function TransferQueue() {
  const all = useTransfers();
  const source = useDataSource();
  // Demo nodes' files never reach this Mac, so a retry of one would only be refused again; and
  // a transfer from the other data source is never retried here (it would act on other nodes).
  const demo = useCluster()?.demo ?? false;
  const retryable = (t: Transfer) => canRetry(t, source) && !(demo && t.spec.to === MAC && t.spec.from !== MAC);
  const [opened, setOpen] = useState<boolean | null>(null);
  const mine = all.filter((t) => (t.source ?? source) === source);
  if (!mine.length) return null;
  const active = mine.filter(isActive);
  const running = active.filter((t) => t.state === "running").length;
  const waiting = active.length - running;
  const finished = mine.filter(isFinished);
  const idle = idleSummary(mine);
  // Open while something's running or waiting; one line when idle, until you open it.
  const open = opened ?? !idle;
  const shown = open ? [...active, ...finished.slice(0, 8)] : active;

  return (
    <section className="sb-tq" aria-label="Transfers">
      <div className="sb-tq-head">
        <button type="button" className="sb-tq-toggle" onClick={() => setOpen(!open)} aria-expanded={open}>
          {open ? <ChevronDown size={12} /> : <ChevronUp size={12} />}
          Transfers
        </button>
        <HelpTip topic="desk.files.transfers" />
        <span className="muted">
          {idle && !open ? idle.replace(/^No transfers running · /, "Nothing running · ") : (
            <>
              {running ? `${running} running` : "Nothing running"}
              {waiting ? ` · ${waiting} waiting` : ""}
              {finished.length ? ` · ${finished.length} in history` : ""}
            </>
          )}
        </span>
        <span className="spacer" />
        {finished.length > 0 && <button type="button" className="btn small" onClick={clearFinishedTransfers}>Clear history</button>}
      </div>
      {shown.length > 0 && (
        <ul className="sb-tq-list">
          {shown.map((t) => (
            <li key={t.id} className={`sb-tq-row ${t.state}`}>
              <StateIcon t={t} />
              <div className="sb-tq-main">
                <div className="sb-tq-line">
                  <span className="sb-tq-what" title={t.spec.names.join(", ")}>{t.spec.op === "move" ? "Move" : "Copy"} {t.what}</span>
                  <span className="sb-tq-route">
                    {t.fromName} <ArrowRight size={11} aria-label="to" /> {t.toName}
                    <span className="muted"> · {routeLabel(t)}</span>
                  </span>
                </div>
                {t.state === "running" && (
                  <Meter pct={t.total ? (100 * t.done) / t.total : 0} label={detail(t)} ariaLabel={`${t.what}: ${detail(t)}`} />
                )}
                {t.state !== "running" && (
                  <div className={`sb-tq-detail${t.state === "failed" ? " failed" : ""}`} title={t.message ?? undefined}>
                    {isFinished(t) ? t.message : detail(t)}
                  </div>
                )}
                {t.state === "running" && t.current && <div className="sb-tq-current mono" title={t.current}>{t.current}</div>}
              </div>
              <div className="sb-tq-actions">
                {isActive(t) && (
                  <button type="button" className="btn small" onClick={() => void stop(t)} title="Stop; what had arrived is removed">
                    <Ban size={12} />Stop
                  </button>
                )}
                {retryable(t) && (
                  <button type="button" className="btn small" onClick={() => retryTransfer(t.id)} title="Run it again (checking the destination afresh)">
                    <RotateCw size={12} />Retry
                  </button>
                )}
                {isFinished(t) && (
                  <button type="button" className="sb-tq-x" onClick={() => removeTransfer(t.id)} title="Remove from the list" aria-label="Remove from the list">
                    <X size={12} />
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
