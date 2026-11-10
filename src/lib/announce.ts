// Screen reader announcements: two visually hidden live regions, one polite
// (waits for a pause) and one assertive (interrupts, for errors).

export type Politeness = "polite" | "assertive";

const regions: Partial<Record<Politeness, HTMLElement>> = {};

function region(p: Politeness): HTMLElement | null {
  if (typeof document === "undefined") return null;
  const existing = regions[p];
  if (existing?.isConnected) return existing;
  const el = document.createElement("div");
  el.className = "sr-only";
  el.setAttribute("aria-live", p);
  el.setAttribute("aria-atomic", "true");
  el.setAttribute("role", p === "assertive" ? "alert" : "status");
  el.dataset.announce = p;
  document.body.appendChild(el);
  regions[p] = el;
  return el;
}

/** Reads `text` out to VoiceOver. Repeating the same text announces it again. */
export function announce(text: string, politeness: Politeness = "polite") {
  const el = region(politeness);
  if (!el || !text) return;
  // Clearing first and setting on the next tick makes a repeat count as a change.
  el.textContent = "";
  window.setTimeout(() => {
    el.textContent = text;
  }, 30);
}
