// The Dock icon: a progress bar for long jobs and a badge count, through the
// window APIs (no-ops in a plain browser).
import { getCurrentWindow, ProgressBarStatus } from "@tauri-apps/api/window";
import { inTauri } from "./api";

let lastProgress: number | null | undefined;
let lastBadge: number | null | undefined;

/** 0..1 shows a bar under the Dock icon; null hides it. */
export function setDockProgress(frac: number | null) {
  const v = frac === null ? null : Math.round(Math.max(0, Math.min(1, frac)) * 100);
  if (!inTauri || v === lastProgress) return;
  lastProgress = v;
  getCurrentWindow()
    .setProgressBar(v === null ? { status: ProgressBarStatus.None } : { status: ProgressBarStatus.Normal, progress: v })
    .catch(() => {});
}

/** A number on the Dock icon; null or 0 removes it. */
export function setDockBadge(n: number | null) {
  const v = n && n > 0 ? Math.floor(n) : null;
  if (!inTauri || v === lastBadge) return;
  lastBadge = v;
  getCurrentWindow().setBadgeCount(v ?? undefined).catch(() => {});
}
