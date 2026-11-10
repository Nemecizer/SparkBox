// How Files lays out its two panes: side by side when there's room, one at a time with a
// switcher ("This Mac | cell1") below 720 px, so the window still works at tile size (460×340).

/** Narrower than this, the window shows one pane and a switcher. */
export const SINGLE_PANE_BELOW = 720;

export type FilesLayout = "split" | "single";

export function filesLayout(width: number): FilesLayout {
  return width > 0 && width < SINGLE_PANE_BELOW ? "single" : "split";
}

/** The switcher's two labels: each pane's place, with its folder when both show the same place. */
export function switcherLabels(places: [string, string], paths: [string, string]): [string, string] {
  if (places[0] !== places[1]) return places;
  const tail = (p: string) => p.replace(/\/+$/, "").split("/").pop() || p || "~";
  return [`${places[0]} · ${tail(paths[0])}`, `${places[1]} · ${tail(paths[1])}`];
}
