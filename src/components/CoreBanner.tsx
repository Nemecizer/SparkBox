import { useEffect, useState, useSyncExternalStore } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FileWarning, PlugZap, RotateCcw } from "lucide-react";
import { inTauri } from "../lib/api";
import { netApi, type PersistenceWarning } from "../lib/netApi";
import { startServerStore } from "../lib/server";
import { startClusterStore, useCluster } from "../lib/store";
import { startVpnStore } from "../lib/vpn";
import "./shell.css";

// Starting the data stores from the core. A failure is kept (not thrown into the
// void), and the banner below offers Retry, which re-runs only what failed.

const STARTERS: Record<string, () => Promise<void>> = {
  cluster: startClusterStore,
  vpn: startVpnStore,
  server: startServerStore,
};

const failed = new Map<string, string>();
const listeners = new Set<() => void>();
let version = 0;

function changed() {
  version++;
  listeners.forEach((l) => l());
}

async function start(name: string) {
  try {
    await STARTERS[name]();
    failed.delete(name);
  } catch (e) {
    failed.set(name, String(e).replace(/^Error: /, ""));
  }
  changed();
}

/** Starts every store (main.tsx). */
export function startCore() {
  for (const name of Object.keys(STARTERS)) void start(name);
}

/** Starts again only the stores that failed. */
export function retryCore() {
  const names = failed.size ? [...failed.keys()] : Object.keys(STARTERS);
  for (const name of names) void start(name);
}

function useCoreErrors(): Map<string, string> {
  useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => version,
  );
  return failed;
}

const WAIT_MS = 10_000;

/** "Can't reach the SparkBox core" when no snapshot has arrived within 10 s, or a store failed to start. */
export function CoreBanner() {
  const cluster = useCluster();
  const errors = useCoreErrors();
  const [late, setLate] = useState(false);
  const [tries, setTries] = useState(0);

  useEffect(() => {
    if (cluster) return;
    setLate(false);
    const t = window.setTimeout(() => setLate(true), WAIT_MS);
    return () => window.clearTimeout(t);
  }, [!!cluster, tries]);

  if (!inTauri) return null;
  const clusterError = errors.get("cluster");
  const otherErrors = [...errors.entries()].filter(([k]) => k !== "cluster");
  const stuck = !cluster && (late || !!clusterError);
  if (!stuck && !otherErrors.length) return null;

  const reason = clusterError
    ?? (stuck ? "no data has arrived for 10 seconds" : otherErrors.map(([k, v]) => `${k === "vpn" ? "VPN" : "Server"} status: ${v}`).join("; "));
  return (
    <div className="core-banner" role="alert">
      <PlugZap size={14} aria-hidden />
      <span>Can't reach the SparkBox core: {reason}</span>
      <button
        className="core-banner-btn"
        onClick={() => {
          retryCore();
          setTries((n) => n + 1);
        }}
      >
        <RotateCcw size={12} aria-hidden /> Retry
      </button>
    </div>
  );
}

/**
 * Storage failures at startup or while running. A guarded deployment store leaves the
 * original in place; other stores can quarantine unreadable settings and use defaults.
 */
export function SettingsFileBanner() {
  const [warnings, setWarnings] = useState<PersistenceWarning[]>([]);
  const [dismissed, setDismissed] = useState("");

  useEffect(() => {
    if (!inTauri) return;
    let active = true;
    const poll = () => { void netApi.persistenceWarnings().then((next) => { if (active) setWarnings(next); }, () => {}); };
    poll();
    const timer = setInterval(poll, 5000);
    return () => { active = false; clearInterval(timer); };
  }, []);

  const signature = warnings.map((w) => `${w.path ?? w.file}:${w.error}`).join("\n");
  if (!warnings.length || signature === dismissed) return null;
  const files = [...new Set(warnings.map((w) => w.file))];
  const named = files.length === 1 ? files[0] : `${files.slice(0, -1).join(", ")} and ${files[files.length - 1]}`;
  const history = warnings.some((w) => w.file === "deployments.json");
  return (
    <div className="core-banner warn" role="status">
      <FileWarning size={14} aria-hidden />
      <span title={warnings.map((w) => `${w.file}: ${w.error}`).join("\n")}>
        {history ? `Couldn't save or read ${named}. Existing files were kept. New loads and Forget need a successful history save. Review the files before reopening SparkBox.` :
          `Couldn't read ${named}. SparkBox started with defaults and kept the old ${files.length === 1 ? "file" : "files"} beside ${files.length === 1 ? "it" : "them"}.`}
      </span>
      <button className="core-banner-btn" onClick={() => void revealItemInDir(warnings[0].backup).catch(() => {})}>
        Show in Finder
      </button>
      <button className="core-banner-btn" onClick={() => setDismissed(signature)}>
        Dismiss
      </button>
    </div>
  );
}
