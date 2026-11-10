import { useEffect, useSyncExternalStore, useState } from "react";
import { FormGroup, FormRow, HelpTip, Notice, Switch } from "../../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { api } from "../../lib/api";
import type { Prefs } from "../../lib/types";

// ---- App preferences (prefs.json on the Rust side), shared by several sections.

let prefs: Prefs | null = null;
let prefsError: string | null = null;
let loading = false;
const prefListeners = new Set<() => void>();
const emitPrefs = () => prefListeners.forEach((l) => l());

function loadPrefs() {
  if (loading) return;
  loading = true;
  api.prefsGet().then(
    (p) => {
      prefs = p;
      prefsError = null;
      emitPrefs();
    },
    (e) => {
      prefsError = String(e);
      emitPrefs();
    },
  ).finally(() => {
    loading = false;
  });
}

/**
 * Saves `patch` over the app's current prefs and publishes what the app stored (it may normalise
 * values). Only the patch is sent (prefs_patch merges it under one lock), so a data source or
 * model change made elsewhere (the palette, the menu bar) isn't undone by a stale copy.
 */
export async function savePrefs(patch: Partial<Prefs>): Promise<void> {
  if (prefs) prefs = { ...prefs, ...patch };
  emitPrefs();
  try {
    prefs = await api.prefsPatch(patch);
    prefsError = null;
  } catch (e) {
    prefsError = String(e);
    loadPrefs();
  }
  emitPrefs();
}

/** Publishes prefs the backend returned from a change made elsewhere (e.g. data_source_set). */
export function setPrefs(next: Prefs) {
  prefs = next;
  prefsError = null;
  emitPrefs();
}

export function usePrefs(): { prefs: Prefs | null; error: string | null } {
  const p = useSyncExternalStore(
    (l) => {
      prefListeners.add(l);
      if (!prefs) loadPrefs();
      return () => prefListeners.delete(l);
    },
    () => prefs,
  );
  const error = useSyncExternalStore((l) => {
    prefListeners.add(l);
    return () => prefListeners.delete(l);
  }, () => prefsError);
  return { prefs: p, error };
}

// ---- Interface-only switches kept in this Mac's localStorage.

const UPCOMING_KEY = "sparkbox.ui.showUpcoming";
const flagListeners = new Set<() => void>();

function readUpcoming(): boolean {
  try {
    return localStorage.getItem(UPCOMING_KEY) === "1";
  } catch {
    return false;
  }
}

export function setShowUpcoming(on: boolean) {
  try {
    if (on) localStorage.setItem(UPCOMING_KEY, "1");
    else localStorage.removeItem(UPCOMING_KEY);
  } catch {
    // Stays as it was; nothing else depends on it.
  }
  flagListeners.forEach((l) => l());
}

/** Whether the sidebar lists launchers that aren't built yet (off by default). */
export function useShowUpcoming(): boolean {
  return useSyncExternalStore((l) => {
    flagListeners.add(l);
    return () => flagListeners.delete(l);
  }, readUpcoming);
}

export function GeneralSection() {
  const { prefs: p, error } = usePrefs();
  const upcoming = useShowUpcoming();
  const [atLogin, setAtLogin] = useState<boolean | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  useEffect(() => {
    api.autostartGet().then(setAtLogin, () => setAtLogin(null));
  }, []);

  // Opening at login changes this Mac, so it asks for Touch ID or the password (and is logged).
  const toggleLogin = async (on: boolean) => {
    try {
      setAtLogin(await withAdmin(() => api.autostartSet(on), { scope: "mac", action: "autostart_set" }));
      setMsg(null);
    } catch (e) {
      if (e !== ADMIN_REFUSED) setMsg(`Couldn't change Start at login: ${(e as { message?: string })?.message ?? String(e)}`);
    }
  };

  return (
    <>
      {(error || msg) && <Notice tone="error" onDismiss={() => setMsg(null)}>{msg ?? error}</Notice>}
      <FormGroup
        title="Menu bar"
        footnote="The menu bar icon shows each node's status, what it's serving and its GPU load. Quit with ⌘Q or “Quit SparkBox” in that menu."
      >
        <FormRow label={<>Keep running in the menu bar <HelpTip topic="desk.general.menu-bar" /></>} htmlFor="pref-menubar" hint="When the window is closed">
          <Switch
            id="pref-menubar"
            label="Keep running in the menu bar"
            checked={!!p?.keepInMenuBar}
            disabled={!p}
            onChange={(on) => p && savePrefs({ keepInMenuBar: on })}
          />
        </FormRow>
        <FormRow label={<>Start at login <HelpTip topic="desk.general.login" /></>} htmlFor="pref-login" hint="Opens in the menu bar">
          <Switch id="pref-login" label="Start at login" checked={!!atLogin} disabled={atLogin === null} onChange={toggleLogin} />
        </FormRow>
      </FormGroup>
      <FormGroup
        title="Features"
        footnote="Adds a Coming soon group to the sidebar listing the inference launchers that are planned but not built yet, each with what it will do."
      >
        <FormRow label={<>Show upcoming features <HelpTip topic="desk.general.upcoming" /></>} htmlFor="pref-upcoming">
          <Switch id="pref-upcoming" label="Show upcoming features" checked={upcoming} onChange={setShowUpcoming} />
        </FormRow>
      </FormGroup>
    </>
  );
}
