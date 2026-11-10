// Dragging files between panes with pointer events (WKWebView's HTML5 drag and drop belongs
// to Tauri's Finder drop handling): a ghost with a count follows the pointer, ⌥ switches copy
// to move, Esc cancels, and the pane or folder row under the pointer is the target.

export interface DropTarget {
  /** The Files window's own element the target is in. */
  root: HTMLElement;
  pane: number;
  /** A folder row's name, when over one (the items go into it). */
  folder: string | null;
}

/** What's under a point: a pane of this Files window, and a folder row in it. */
export function hitTest(root: HTMLElement, x: number, y: number): DropTarget | null {
  const el = document.elementFromPoint(x, y) as HTMLElement | null;
  if (!el || !root.contains(el)) return null;
  const pane = el.closest<HTMLElement>("[data-pane]");
  if (!pane || !root.contains(pane)) return null;
  const row = el.closest<HTMLElement>("tr[data-key]");
  const folder = row && row.querySelector("[data-folder]") ? (row.dataset.key ?? null) : null;
  return { root, pane: Number(pane.dataset.pane), folder };
}

export interface DragGhost {
  move(x: number, y: number): void;
  setLabel(move: boolean, target: string | null): void;
  remove(): void;
}

// Lucide's folder and file outlines, drawn in the ghost (it lives outside React).
const FOLDER_SVG = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/></svg>';
const FILE_SVG = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/></svg>';

export function makeGhost(first: string, count: number, folder: boolean): DragGhost {
  const el = document.createElement("div");
  el.className = "sb-files-ghost";
  el.setAttribute("aria-hidden", "true");
  el.innerHTML = `<span class="sb-files-ghost-icon${folder ? " folder" : ""}">${folder ? FOLDER_SVG : FILE_SVG}</span><span class="sb-files-ghost-name"></span>${count > 1 ? `<span class="sb-files-ghost-count">${count}</span>` : ""}<span class="sb-files-ghost-op"></span>`;
  (el.querySelector(".sb-files-ghost-name") as HTMLElement).textContent = first;
  document.body.appendChild(el);
  document.body.classList.add("sb-files-dragging");
  return {
    move(x, y) {
      el.style.transform = `translate(${x + 14}px, ${y + 10}px)`;
    },
    setLabel(move, target) {
      const op = el.querySelector(".sb-files-ghost-op") as HTMLElement;
      op.textContent = target ? `${move ? "Move" : "Copy"} to ${target}` : "Drop on a folder or the other pane";
      el.classList.toggle("move", move);
      el.classList.toggle("nowhere", !target);
    },
    remove() {
      el.remove();
      document.body.classList.remove("sb-files-dragging");
    },
  };
}
