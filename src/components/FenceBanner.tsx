import { useEffect, useState } from "react";
import { ShieldAlert, ShieldCheck } from "lucide-react";
import { inTauri } from "../lib/api";
import { useDataSource } from "../lib/dataSource";
import { fmtLeft, useFence } from "../lib/fence";
import { openWindow } from "../wm/WindowManager";
import "./shell.css";

const openFence = () => openWindow("settings", { section: "fence" });

/**
 * The write fence, under the top bar (Live only): open, with what it allows and how long it has
 * left; failed its check, with the test build's changes refused; or a key the test build doesn't
 * trust yet. The release build's own changes need no fence, so its banner is only about what the
 * test build may do. No fence shows nothing.
 */
export function FenceBanner() {
  const live = useDataSource() === "live";
  const { view } = useFence(live);
  const [now, setNow] = useState(() => Date.now());
  const [at, setAt] = useState(() => Date.now());
  useEffect(() => setAt(Date.now()), [view]);
  useEffect(() => {
    if (view?.state !== "valid") return;
    const t = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(t);
  }, [view?.state]);

  if (!inTauri || !live || !view) return null;
  if (view.state === "failed") {
    return (
      <div className="core-banner" role="alert">
        <ShieldAlert size={14} aria-hidden />
        <span title={view.why ?? undefined}>The fence record failed its check · {view.canOpen ? "the test build's changes" : "changes on the cluster"} are refused</span>
        <button className="core-banner-btn" onClick={openFence}>Open a new fence</button>
      </div>
    );
  }
  if (view.state === "untrusted") {
    // The test build's trust: nothing the release build does depends on it.
    if (view.canOpen) return null;
    return (
      <div className="core-banner warn" role="status">
        <ShieldAlert size={14} aria-hidden />
        <span title={view.why ?? undefined}>This build doesn't trust the fence key yet</span>
        <button className="core-banner-btn" onClick={openFence}>Settings › Fence</button>
      </div>
    );
  }
  if (view.state !== "valid" || !view.record) return null;
  // A fence for the release build itself, from before it stopped needing them, allows nothing.
  if (view.canOpen && view.mine) return null;
  const n = view.record.classes.length;
  const what = `${view.record.nodes.join(", ")} · ${n} class${n === 1 ? "" : "es"} · ${fmtLeft(view.remainingMs - (now - at))} left`;
  return (
    <div className="core-banner warn" role="status">
      <ShieldCheck size={14} aria-hidden />
      <span>{view.mine ? `Fence open: ${what}` : `Fence open for the ${view.record.instance === "dev" ? "test" : "release"} build: ${what}`}</span>
      <button className="core-banner-btn" onClick={openFence}>Settings › Fence</button>
    </div>
  );
}
