// Small stores the Files window shares across its panes and windows: favourite folders per
// place (seeded with the useful ones), folder sizes worked out with Calculate size, and the
// in-app clipboard for ⌘C / ⌘V between panes.
import { useSyncExternalStore } from "react";
import { currentSource, migrateLegacy, onSourceChange, scopedKey } from "../../lib/dataSource";
import { MAC } from "../../lib/filesApi";

function store<T>(initial: T) {
  let value = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => value,
    set(v: T) {
      value = v;
      listeners.forEach((l) => l());
    },
    subscribe(l: () => void) {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
}

// ---- favourites

// Per data source (scopedKey): a Demo node's folders aren't the real node's.
const FAV_KEY = "sparkbox.files.favorites";
const SEED_MAC = ["~", "~/Downloads"];
const SEED_NODE = ["~", "~/models", "~/.cache/huggingface", "/etc/netplan", "/var/log"];

// Favourites saved before they were kept per source were made in Demo.
migrateLegacy(FAV_KEY);

function loadFavs(source = currentSource()): Record<string, string[]> {
  try {
    const v = JSON.parse(localStorage.getItem(scopedKey(FAV_KEY, source)) ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

const favs = store<Record<string, string[]>>(loadFavs());
onSourceChange((next) => favs.set(loadFavs(next)));

function saveFavs(next: Record<string, string[]>) {
  favs.set(next);
  try {
    localStorage.setItem(scopedKey(FAV_KEY), JSON.stringify(next));
  } catch {
    // Favourites still work for this session.
  }
}

export function favoritesFor(all: Record<string, string[]>, place: string): string[] {
  return all[place] ?? (place === MAC ? SEED_MAC : SEED_NODE);
}

export function useFavorites(place: string): string[] {
  const all = useSyncExternalStore(favs.subscribe, favs.get);
  return favoritesFor(all, place);
}

export function addFavorite(place: string, path: string) {
  const list = favoritesFor(favs.get(), place);
  if (!list.includes(path)) saveFavs({ ...favs.get(), [place]: [...list, path] });
}

export function removeFavorite(place: string, path: string) {
  saveFavs({ ...favs.get(), [place]: favoritesFor(favs.get(), place).filter((p) => p !== path) });
}

export function resetFavorites(place: string) {
  const next = { ...favs.get() };
  delete next[place];
  saveFavs(next);
}

// ---- folder sizes (Calculate size)

const sizes = store<Map<string, number | "busy">>(new Map());

export const sizeKey = (place: string, path: string) => `${place}:${path}`;

export function useSizes(): Map<string, number | "busy"> {
  return useSyncExternalStore(sizes.subscribe, sizes.get);
}

export function setSizes(entries: [string, number | "busy" | null][]) {
  const next = new Map(sizes.get());
  for (const [k, v] of entries) {
    if (v === null) next.delete(k);
    else next.set(k, v);
  }
  sizes.set(next);
}

// ---- clipboard (⌘C in one pane, ⌘V in the other)

export interface FileClip {
  place: string;
  dir: string;
  names: string[];
}

const clip = store<FileClip | null>(null);

export function setFileClip(c: FileClip | null) {
  clip.set(c);
}

export function getFileClip(): FileClip | null {
  return clip.get();
}

export function useFileClip(): FileClip | null {
  return useSyncExternalStore(clip.subscribe, clip.get);
}
