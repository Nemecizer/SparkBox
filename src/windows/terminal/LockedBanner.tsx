// What a shell tab shows while it can't take keystrokes (the owner locked, or nobody typed for
// its idle time), and the minute's warning before an idle lock. A keystroke typed while
// locked isn't sent; the banner flashes and says so instead of dropping it silently.
import { Clock, Lock } from "lucide-react";
import { HelpTip } from "../../components/ui";
import { authenticateTab, REFUSED_INPUT, type TermTab } from "../../lib/terminals";

/** How long after a refused keystroke the banner keeps the longer sentence. */
const REFUSED_SHOWN_MS = 8000;

export function LockedBanner({ tab, now = Date.now() }: { tab: Pick<TermTab, "key" | "lockReason" | "refusedAtMs">; now?: number }) {
  const refused = tab.refusedAtMs !== null && now - tab.refusedAtMs < REFUSED_SHOWN_MS;
  return (
    // Keyed by the refusal, so each refused keystroke restarts the flash.
    <div key={tab.refusedAtMs ?? 0} className={`sb-term-overlay${refused ? " refused" : ""}`} role={refused ? "alert" : "status"}>
      <Lock size={14} aria-hidden />
      <span className="sb-term-overlay-text">
        <span>{refused ? REFUSED_INPUT : "Locked — authenticate to keep typing."}</span>
        {tab.lockReason && <span className="sb-term-overlay-why">{tab.lockReason}</span>}
      </span>
      <HelpTip topic="desk.terminal.locked" />
      <button type="button" className="btn small primary" onClick={() => void authenticateTab(tab.key)}>Authenticate…</button>
    </div>
  );
}

export function IdleWarning({ text }: { text: string }) {
  return (
    <div className="sb-term-banner idle" role="status">
      <Clock size={13} aria-hidden />
      <span>{text}</span>
    </div>
  );
}
