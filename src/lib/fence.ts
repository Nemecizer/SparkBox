// The write fence, the controller lease and the identity pins (src-tauri/src/live/exit.rs): what
// Settings › Fence and the fence banner show. Every Live change on a node leaves through one exit,
// which refuses it while the other SparkBox holds the node's lease or when the machine answering
// isn't the one pinned. The fence is the test build's: its changes also need an open fence, which
// only the release build opens (Touch ID). The release build's own changes need Admin mode, not a
// fence. Demo ignores fences.
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri, invoke } from "./api";

/** The fence class ids (fence_class.rs), grouped as PLAN §6.3 lists them for P1. */
export const OWNER_CLASSES = [
  "sudo-probe", "root-helper", "service", "container-owner", "fleet", "shell", "power-one", "apt-refresh", "apt-one",
  "apt-hold", "wifi", "model-load-one", "model-load", "model-unload", "drop-caches", "rdp-repair",
] as const;
export const AUTONOMY_CLASSES = ["container-test", "process-test", "files-test"] as const;
export type FenceClass = (typeof OWNER_CLASSES)[number] | (typeof AUTONOMY_CLASSES)[number] | "any";

/** Classes whose changes name items an `only` list can narrow: units, containers, packages. */
export const ONLY_CLASSES: Partial<Record<FenceClass, string>> = {
  service: "Units",
  "container-owner": "Containers",
  "container-test": "Containers",
  "apt-one": "Packages",
  "apt-hold": "Packages",
};

export interface FenceRecord {
  v: number;
  id: string;
  instance: "release" | "dev";
  nodes: string[];
  classes: FenceClass[];
  only: Partial<Record<FenceClass, string[]>>;
  createdMs: number;
  expiresMs: number;
  mac: string;
  keyId: string;
  note: string;
  sig: string;
}

export interface FenceView {
  state: "none" | "valid" | "expired" | "failed" | "untrusted";
  record: FenceRecord | null;
  /** The fence is for this build. In the release build that's one from before it stopped needing them. */
  mine: boolean;
  /** "cell2 · 6 classes · 2 h 41 min" */
  summary: string | null;
  remainingMs: number;
  keyKind: "keychain" | "secureEnclave";
  keyId: string | null;
  trustedKeyId: string | null;
  instance: "release" | "dev";
  /** The release build: it opens fences, for the test build. */
  canOpen: boolean;
  /** The dev build trusts the release build's key by hand once it has its own settings. */
  canTrust: boolean;
  why: string | null;
}

/** What Settings › Fence opens: always for the test build (its id is "dev"). */
export interface FenceSpec {
  nodes: string[];
  classes: FenceClass[];
  only: Partial<Record<FenceClass, string[]>>;
  minutes: number;
  instance: "dev";
  note: string;
}

export interface LeaseView {
  node: string;
  heldBy: "release" | "dev" | null;
  tag: string | null;
  mac: string | null;
  ageS: number | null;
  mine: boolean;
  error: string | null;
}

export interface IdentityPin {
  hostname: string;
  lanMac: string | null;
  machineId?: string;
  clonedMachineId: boolean;
  hostKey: string | null;
  pinnedMs: number;
}

export interface IdentityView {
  node: string;
  name: string;
  pin: IdentityPin | null;
  now: { hostname: string; lanMac: string | null; machineId: string | null } | null;
  matches: boolean | null;
  note: string | null;
}

export const fenceApi = {
  state: () => invoke<FenceView>("fence_state"),
  open: (spec: FenceSpec) => invoke<FenceView>("fence_open", { spec }),
  close: () => invoke<FenceView>("fence_close"),
  trustKey: () => invoke<FenceView>("fence_trust_key"),
  leases: () => invoke<LeaseView[]>("lease_state"),
  identity: () => invoke<IdentityView[]>("node_identity"),
  repin: (node: string, typedName: string) => invoke<IdentityPin>("node_identity_repin", { node, typedName }),
};

/**
 * The presets Settings › Fence offers. A fence is for the test build only: the release build's
 * own changes need Admin mode, so its Session A and Session B presets went with its fence.
 */
export function presets(nodes: string[]): { label: string; spec: FenceSpec }[] {
  return [
    {
      label: "Agent tests on cell2 (test build)",
      spec: { nodes: nodes.filter((n) => n === "cell2"), classes: ["container-test", "process-test", "files-test"], only: {}, minutes: 480, instance: "dev", note: "Agent tests on cell2" },
    },
  ];
}

/** "2 h 41 min", "35 min", "under a minute" (fence.rs `left`). */
export function fmtLeft(ms: number): string {
  const min = Math.floor(Math.max(0, ms) / 60_000);
  const h = Math.floor(min / 60);
  const m = min % 60;
  if (!h && !m) return "under a minute";
  if (!h) return `${m} min`;
  return m ? `${h} h ${m} min` : `${h} h`;
}

/** "cell2 · 6 classes · 3 h": what the Touch ID sheet names. */
export function specSummary(s: FenceSpec): string {
  const n = s.classes.length;
  return `${s.nodes.join(", ")} · ${n} class${n === 1 ? "" : "es"} · ${fmtLeft(s.minutes * 60_000)}`;
}

/** The fence's state, kept current: on `fence://changed`, and every 30 s for the time left. */
export function useFence(enabled = true): { view: FenceView | null; refresh: () => void } {
  const [view, setView] = useState<FenceView | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!inTauri || !enabled) return;
    let alive = true;
    fenceApi.state().then((v) => alive && setView(v), () => {});
    const off = listen<FenceView>("fence://changed", (e) => alive && setView(e.payload));
    const t = window.setInterval(() => fenceApi.state().then((v) => alive && setView(v), () => {}), 30_000);
    return () => {
      alive = false;
      window.clearInterval(t);
      void off.then((f) => f()).catch(() => {});
    };
  }, [enabled, tick]);
  return { view, refresh: () => setTick((n) => n + 1) };
}
