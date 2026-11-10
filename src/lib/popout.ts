// Pop-out windows: any inner window can move out of the desk into a native macOS
// window of its own (labelled "pop-<kind>-<n>", loading index.html?popout=<kind>&params=…),
// for a second screen or Mission Control. The pop-out runs the same stores off the
// same events. What it opens (a node from the Dashboard, say) opens on the desk, and
// "Return to desk" puts it back there.
import { emit, listen } from "@tauri-apps/api/event";
import { getAllWebviewWindows, WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Command } from "./commands";

export interface PopoutRoute {
  kind: string;
  params: Record<string, string>;
}

/** Asks the desk to open a window: from a pop-out's links, or Return to desk. */
export const POPOUT_OPEN = "popout://open";

const KIND = /^[a-z][a-z0-9-]{0,40}$/;

/** The page address for a pop-out: "index.html?popout=dashboard&params=%7B%7D". */
export function popoutUrl(kind: string, params: Record<string, string> = {}): string {
  const q = new URLSearchParams({ popout: kind, params: JSON.stringify(params) });
  return `index.html?${q.toString()}`;
}

/** The route a page was opened with, or null for the main window (or a malformed address). */
export function parsePopout(search: string): PopoutRoute | null {
  const q = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const kind = q.get("popout");
  if (!kind || !KIND.test(kind)) return null;
  let params: Record<string, string> = {};
  const raw = q.get("params");
  if (raw) {
    try {
      const v = JSON.parse(raw) as unknown;
      if (!v || typeof v !== "object" || Array.isArray(v)) return null;
      params = Object.fromEntries(Object.entries(v as Record<string, unknown>).filter(([, x]) => typeof x === "string")) as Record<string, string>;
    } catch {
      return null;
    }
  }
  return { kind, params };
}

/** The first free label for a kind: "pop-dashboard-1", then "-2"… */
export function popoutLabel(kind: string, taken: string[]): string {
  const base = `pop-${kind.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  let n = 1;
  while (taken.includes(`${base}-${n}`)) n++;
  return `${base}-${n}`;
}

/** Opens a pop-out window for `kind`; resolves once macOS has made it, rejects if it couldn't. */
export async function openPopout(route: PopoutRoute, opts: { title: string; size: [number, number]; minSize: [number, number] }): Promise<string> {
  const taken = (await getAllWebviewWindows()).map((w) => w.label);
  const label = popoutLabel(route.kind, taken);
  const win = new WebviewWindow(label, {
    url: popoutUrl(route.kind, route.params),
    title: opts.title,
    width: opts.size[0],
    height: opts.size[1],
    minWidth: opts.minSize[0],
    minHeight: opts.minSize[1],
  });
  await new Promise<void>((resolve, reject) => {
    void win.once("tauri://created", () => resolve());
    void win.once<string>("tauri://error", (e) => reject(new Error(String(e.payload))));
  });
  return label;
}

/** From a pop-out: open something on the desk (and bring the desk forward). */
export function openOnDesk(route: PopoutRoute): Promise<void> {
  return emit(POPOUT_OPEN, route);
}

/** The command behind ⌘? in a pop-out, which has no window manager of its own to open Help in. */
export const helpOnDesk: Command = { id: "help:open", title: "SparkBox Help", group: "App", run: () => openOnDesk({ kind: "help", params: {} }) };

/** On the desk: follows pop-outs' requests. Returns a function that stops. */
export function onDeskRequests(open: (route: PopoutRoute) => void): () => void {
  let stop: (() => void) | undefined;
  let alive = true;
  listen<PopoutRoute>(POPOUT_OPEN, (e) => {
    if (e.payload?.kind && KIND.test(e.payload.kind)) open({ kind: e.payload.kind, params: e.payload.params ?? {} });
  }).then((u) => (alive ? (stop = u) : u()), () => {});
  return () => {
    alive = false;
    stop?.();
  };
}
