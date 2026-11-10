// The Firewalla Switch X's panel (src-tauri/src/firewalla): the owner's labels and what the
// cloud (MSP) API last said. What LLDP hears comes with the cluster snapshot.
import { invoke } from "@tauri-apps/api/core";

/** The token's name in the Keychain vault. */
export const MSP_TOKEN_ACCOUNT = "firewalla:msp";

export type LabelKind = "node" | "crs804" | "gold" | "mac" | "text" | "empty";

/** What the owner says is plugged into a port. */
export interface PortLabel {
  kind: LabelKind;
  /** The node's id, for kind "node". */
  node?: string | null;
  /** The words, for kind "text". */
  text?: string | null;
  setAtMs?: number;
}

export interface MspSfp {
  present: boolean | null;
  connector: string | null;
  bitRate: string | null;
  vendor: string | null;
}

/** One port as Firewalla's cloud reports it. Null means Firewalla didn't say, never zero. */
export interface MspPort {
  port: number;
  linkUp: boolean | null;
  linkSpeedMbps: number | null;
  rxBytes: number | null;
  txBytes: number | null;
  rxErrors: number | null;
  txErrors: number | null;
  rxDiscards: number | null;
  txDiscards: number | null;
  /** When the counters last started from zero (Unix seconds). */
  statsSinceS: number | null;
  poeStatus: string | null;
  poeMode: string | null;
  poeWatts: number | null;
  stpState: string | null;
  stpRole: string | null;
  portType: string | null;
  sfp: MspSfp | null;
  uplink: boolean;
}

export interface MspSwitch {
  mac: string | null;
  name: string | null;
  ip: string | null;
  model: string | null;
  version: string | null;
  online: boolean | null;
  ports: MspPort[];
  /** Port entries SparkBox couldn't place on the panel. */
  skipped: number;
}

export type MspState = "off" | "waiting" | "ok" | "no-token" | "keychain" | "rate-limited" | "auth" | "not-json" | "shape" | "not-listed" | "http" | "unreachable";

export interface MspStatus {
  /** An MSP domain is saved. */
  configured: boolean;
  domain: string | null;
  state: MspState;
  message: string | null;
  lastOkMs: number | null;
  lastTryMs: number | null;
  nextPollMs: number | null;
  /** The last good answer: it stays through a failure that says nothing against it. */
  sw: MspSwitch | null;
  /** Look-only mode is holding the poll. */
  held?: boolean;
}

export interface PortsView {
  /** Labels by port number. */
  labels: Record<string, PortLabel>;
  msp: MspStatus;
}

export interface MspTestResult {
  ok: boolean;
  kind: string | null;
  message: string;
}

export const firewallaApi = {
  view: (switchId: string) => invoke<PortsView>("firewalla_view", { switchId }),
  /** Sets a port's label, or (null) takes it off. */
  labelSet: (switchId: string, port: number, label: PortLabel | null) => invoke<void>("firewalla_label_set", { switchId, port, label }),
  mspSettings: () => invoke<{ domain: string | null }>("firewalla_msp_settings"),
  mspSetDomain: (domain: string) => invoke<{ domain: string | null }>("firewalla_msp_set_domain", { domain }),
  mspTest: () => invoke<MspTestResult>("firewalla_msp_test"),
};
