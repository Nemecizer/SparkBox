// Keyboard or pointer: which one the user last used. <html data-modality> carries it,
// and shell.css hides focus rings unless it's "keyboard", the way macOS draws them only
// for keyboard focus. Text fields keep theirs (that's where typing goes). The app starts
// in pointer mode, so a window focused at launch never shows a ring.

export type Modality = "keyboard" | "pointer";

/** Keys that move focus or act on it; typing letters into a field doesn't count. */
const NAV_KEYS = new Set([
  "Tab", "Enter", " ", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "F6", "F10", "ContextMenu",
]);

/** Whether a key press means the user is driving with the keyboard. */
export function isKeyboardNavigation(e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey">): boolean {
  if (e.metaKey || e.ctrlKey || e.altKey) return false;
  return NAV_KEYS.has(e.key);
}

export function getModality(root: HTMLElement = document.documentElement): Modality {
  return root.dataset.modality === "keyboard" ? "keyboard" : "pointer";
}

/** Tracks the last input on `root` (default <html>). Returns a function that stops it. */
export function installModality(root: HTMLElement = document.documentElement): () => void {
  const set = (m: Modality) => {
    if (root.dataset.modality !== m) root.dataset.modality = m;
  };
  set("pointer");
  const onKey = (e: KeyboardEvent) => {
    if (isKeyboardNavigation(e)) set("keyboard");
  };
  const onPointer = () => set("pointer");
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("pointerdown", onPointer, true);
  return () => {
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("pointerdown", onPointer, true);
    delete root.dataset.modality;
  };
}
