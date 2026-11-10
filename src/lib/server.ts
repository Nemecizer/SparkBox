// Live server status for the status bar and the Server window.
import { useSyncExternalStore } from "react";
import { api, inTauri } from "./api";
import type { ServerView } from "./modelsApi";

let view: ServerView | null = null;
const listeners = new Set<() => void>();

export function setServerView(v: ServerView) {
  view = v;
  listeners.forEach((l) => l());
}

export async function startServerStore() {
  if (!inTauri) return;
  await api.onServerStatus((v) => setServerView(v as ServerView));
  const v = (await api.serverGet()) as ServerView;
  if (!view) setServerView(v);
}

export function useServerView(): ServerView | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => view,
  );
}
