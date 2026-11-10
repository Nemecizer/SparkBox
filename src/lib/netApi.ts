// Tauri wrappers for the demo world, switches, probes, onboarding and persistence
// (src-tauri/src/demo, routeros, probe.rs, onboard.rs, util.rs).
import { invoke } from "@tauri-apps/api/core";
import type { NodeConfig, Service, VpnView } from "./types";

/** A scripted fault the simulator can play (Settings › Demo scenarios). */
export interface ScenarioInfo {
  id: string;
  title: string;
  description: string;
  /** Seconds before it plays again; 0 while it holds steady. */
  periodS: number;
  /** What it touches, e.g. ["cell3", "crs804"]. */
  affects: string[];
}

/** A settings file SparkBox couldn't read and set aside (for the banner). */
export interface PersistenceWarning {
  file: string;
  path?: string;
  /** Where the unreadable original now is. */
  backup: string;
  error: string;
  tsMs: number;
}

export type SwitchErrorKind =
  | "not-configured"
  | "tls-untrusted"
  | "tls-changed"
  | "auth"
  | "rest-disabled"
  | "timeout"
  | "unreachable"
  | "other";

/** What "Test connection" found for a switch. */
export interface SwitchTestResult {
  ok: boolean;
  board: string | null;
  version: string | null;
  /** SHA-256 of the switch's TLS certificate, for trusting it. */
  fingerprint: string | null;
  errorKind: SwitchErrorKind | null;
  message: string;
}

/** The certificate a failed test offers to trust. Only a certificate failure has one: past it, a wrong password isn't fixed by trusting anything. */
export function certToTrust(r: SwitchTestResult): string | null {
  return !r.ok && (r.errorKind === "tls-untrusted" || r.errorKind === "tls-changed") ? r.fingerprint : null;
}

export interface ProbeSample {
  tsMs: number;
  /** Round trip in ms, or null for a lost probe. */
  ms: number | null;
}

export interface Outage {
  startMs: number;
  /** null while it's still down. */
  endMs: number | null;
  reason: string;
}

/** Reachability of a device SparkBox can only probe (the Firewalla Switch X and Gold). */
export interface ProbeHistory {
  switchId: string;
  /** What's probed, e.g. "192.168.50.3:443". */
  target: string;
  /** Oldest first, one every 5 s. */
  samples: ProbeSample[];
  lossPct: number | null;
  /** Share of the last 24 hours it answered. */
  uptimePct: number | null;
  lastOutage: Outage | null;
}

export type CheckStatus = "ok" | "warn" | "fail" | "skip";

/** One line of the Connect wizard's checklist for a node. */
export interface CheckStep {
  id: "resolve" | "tcp" | "hostkey" | "auth" | "method" | "sudo" | "python" | "docker" | "gpu" | "fabric" | "wol";
  label: string;
  status: CheckStatus;
  detail: string;
  /** What to do about a failure. */
  hint: string | null;
  /** For an unknown host key: the fingerprint to compare and trust. */
  fingerprint?: string | null;
  /** "address:port" the fingerprint belongs to. */
  address?: string | null;
  /** Skipped because an earlier step stopped the run (not a check that couldn't tell). */
  blocked?: boolean;
}

export const netApi = {
  demoScenariosList: () => invoke<ScenarioInfo[]>("demo_scenarios_list"),
  /** null (or "default") for the everyday world. Resolves to the scenario now playing. */
  demoScenarioSet: (id: string | null) => invoke<string | null>("demo_scenario_set", { id }),

  persistenceWarnings: () => invoke<PersistenceWarning[]>("persistence_warnings"),

  switchTest: (switchId: string) => invoke<SwitchTestResult>("switch_test", { switchId }),
  /** Pins the switch's certificate (after the user compared the fingerprint). */
  switchTrust: (switchId: string, fingerprint: string) => invoke<SwitchTestResult>("switch_trust", { switchId, fingerprint }),
  switchPortComment: (switchId: string, port: string, comment: string) => invoke<void>("switch_port_comment", { switchId, port, comment }),
  switchPortResetCounters: (switchId: string, port: string) => invoke<void>("switch_port_reset", { switchId, port }),
  /** Blinks the port's LED for a few seconds, to find it in the rack. */
  switchPortBlink: (switchId: string, port: string) => invoke<void>("switch_port_blink", { switchId, port }),
  switchProbeHistory: (switchId: string) => invoke<ProbeHistory>("switch_probe_history", { switchId }),

  /** Connect or disconnect the VPN now, whatever the mode; null follows the mode again. Not saved. */
  vpnOverride: (action: "connect" | "disconnect" | null) => invoke<VpnView>("vpn_override", { action }),

  /** Runs the Connect wizard's checks against one node (nothing is saved). */
  nodeProbe: (node: NodeConfig) => invoke<CheckStep[]>("node_probe", { node }),
  /** Pins a node's SSH host key after the user compared the fingerprint. */
  /** A changed key also needs the node's name typed (`typedName`). */
  knownHostsTrust: (address: string, fingerprint: string, typedName?: string) => invoke<void>("known_hosts_trust", { address, fingerprint, typedName: typedName ?? null }),
  /** Every loaded service on a node from one `systemctl show`, plus the unit files when `files` (read-only). */
  servicesPoll: (node: string, files: boolean) => invoke<ServicesPoll>("services_poll", { node, files }),
};

/** One poll of a node's services (src-tauri/src/ops/services.rs). */
export interface ServicesPoll {
  units: Service[];
  /** [unit file, state] when asked for. */
  files: [string, string][] | null;
}
