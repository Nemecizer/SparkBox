// Entry point of mock.html: the real UI against the simulated backend, in any
// browser. URL parameters:
//   open=dashboard,node:id=cell1,settings:section=appearance   windows to open (kind or kind:k=v&k2=v2)
//   theme=light|dark|black   appearance for this page (saved like the Settings choice)
//   contrast=more, motion=reduce, text=0.92|1.12|1.25   the other Appearance settings
//   reset=1              forget every saved sparkbox.* setting first
//   still=1              no ticking: numbers stay put (for pixel-stable screenshots)
//   chat=interrupted     first simulated reply ends early, with late progress delivery
//   fencePreview=1       offline release Fence layout for Help pictures; all calls stay mocked
//   welcome=1            show the first-run Welcome even when windows are opened
import { extendMock, mockTauri } from "../test/tauri";
import { defaultHandlers, startTicker } from "./mockBackend";

const q = new URLSearchParams(location.search);

function resetStorage() {
  try {
    for (const k of Object.keys(localStorage)) if (k.startsWith("sparkbox.")) localStorage.removeItem(k);
  } catch {
    // No storage: nothing saved to reset.
  }
}

/** Saves Appearance choices from the URL where theme.ts reads them on load. */
function applyAppearance() {
  const patch: Record<string, string | number> = {};
  const theme = q.get("theme");
  if (theme === "light" || theme === "dark" || theme === "black") patch.theme = theme;
  if (q.get("contrast") === "more") patch.contrast = "more";
  if (q.get("motion") === "reduce") patch.motion = "reduce";
  const text = Number(q.get("text"));
  if ([0.92, 1.12, 1.25].includes(text)) patch.textScale = text;
  if (!Object.keys(patch).length) return;
  try {
    const a = JSON.parse(localStorage.getItem("sparkbox.appearance") ?? "{}");
    localStorage.setItem("sparkbox.appearance", JSON.stringify({ ...a, ...patch }));
  } catch {
    // Storage is off: the theme attribute below still applies.
  }
  if (patch.theme) document.documentElement.setAttribute("data-theme", String(patch.theme));
}

function parseOpen(spec: string): { kind: string; params: Record<string, string> }[] {
  return spec.split(",").map((s) => s.trim()).filter(Boolean).map((s) => {
    const [kind, rest = ""] = s.split(/:(.*)/s);
    const params = Object.fromEntries(rest.split("&").filter(Boolean).map((kv) => {
      const [k, v = ""] = kv.split("=");
      return [decodeURIComponent(k), decodeURIComponent(v)];
    }));
    return { kind, params };
  });
}

async function seedWindows(spec: string) {
  const { sizeOf } = await import("../windows/registry");
  const { winId } = await import("../wm/WindowManager");
  const deskW = window.innerWidth - 212;
  const deskH = window.innerHeight - 62;
  const wins = parseOpen(spec).map(({ kind, params }, i) => {
    const [w, h] = sizeOf(kind);
    const ww = Math.min(w, deskW - 24 - i * 28);
    const hh = Math.min(h, deskH - 24 - i * 28);
    return { id: winId(kind, params), kind, params, x: 12 + i * 28, y: 12 + i * 28, w: ww, h: hh, z: i + 1, minimized: false, maximized: false };
  });
  try {
    localStorage.setItem("sparkbox.windows.v1", JSON.stringify(wins));
    if (q.get("welcome") !== "1") localStorage.setItem("sparkbox.welcomed", "mock");
  } catch {
    // The page still opens, with whatever layout the shell picks.
  }
}

async function boot() {
  if (q.get("reset") === "1") resetStorage();
  applyAppearance();
  // The bridge goes in before any package's mocks load: one that imports app code (lib/tasks)
  // would otherwise read `inTauri` as false for good, and the preview would show no data.
  const fencePreview = q.get("fencePreview") === "1";
  mockTauri(defaultHandlers(!fencePreview), { events: true });
  const { MOCKS } = await import("./mocks");
  extendMock(Object.assign({}, ...MOCKS));
  if (fencePreview) extendMock({
    fence_state: () => ({ state: "none", record: null, mine: false, summary: null, remainingMs: 0, keyKind: "keychain", keyId: "0123456789abcdef", trustedKeyId: "0123456789abcdef", instance: "release", canOpen: true, canTrust: false, why: null }),
    lease_state: () => [], node_identity: () => [],
    fence_open: () => { throw new Error("This is a picture fixture; it cannot open a fence."); },
  });
  const open = q.get("open");
  if (open) await seedWindows(open);
  if (q.get("still") !== "1") startTicker(1000, !fencePreview);
  await import("../main");
}

void boot();
