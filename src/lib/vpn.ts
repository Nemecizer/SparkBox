// Live VPN status for the status bar and the VPN window.
import { useSyncExternalStore } from "react";
import { api, inTauri, invoke } from "./api";
import type { LinkStatus, VpnMode, VpnSettings, VpnState, VpnStatus, VpnView } from "./types";

let status: VpnStatus | null = null;
const listeners = new Set<() => void>();

function publish(s: VpnStatus) {
  status = s;
  listeners.forEach((l) => l());
}

export async function startVpnStore() {
  if (!inTauri) return;
  await api.onVpnStatus(publish);
  const view = await api.vpnGet();
  if (!status) publish(view.status);
}

/** For callers that just changed settings and got a fresh view back. */
export const setVpnStatus = publish;

/** vpn.json's "Direct at home" pin (src-tauri/src/vpn/mod.rs `home_pin`), beside the settings types.ts mirrors. */
export type VpnSettingsWithPin = VpnSettings & { homePin?: boolean };

export const homePinOf = (s: VpnSettings): boolean => !!(s as VpnSettingsWithPin).homePin;

/**
 * Turns "Direct at home" on or off. It goes with vpn_set_mode (the mode sent as it is), so it
 * has that command's gate and Touch ID prompt: "change when the VPN connects".
 */
export const vpnSetHomePin = (mode: VpnMode, on: boolean) => invoke<VpnView>("vpn_set_mode", { mode, homePin: on });

/** The VPN's status now, for code outside React (null until it's known). */
export const getVpnStatus = (): VpnStatus | null => status;

export function useVpnStatus(): VpnStatus | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => status,
  );
}

export const VPN_LABEL: Record<VpnState, string> = {
  off: "Off",
  direct: "Direct",
  connecting: "Connecting",
  connected: "Connected",
  error: "Problem",
};

export const VPN_DOT: Record<VpnState, LinkStatus> = {
  off: "offline",
  direct: "online",
  connecting: "connecting",
  connected: "online",
  error: "error",
};

export function protocolName(p: string | null | undefined): string {
  return p === "amneziawg" ? "AmneziaWG" : p === "wireguard" ? "WireGuard" : "—";
}

/** Hides most of a public address: "203.0.•••.••", "2001:db8:…", "home•••.example.net". */
export function maskAddress(addr: string | null | undefined): string {
  if (!addr) return "—";
  const [host, port] = splitHostPort(addr);
  let masked: string;
  if (/^\d+\.\d+\.\d+\.\d+$/.test(host)) {
    const p = host.split(".");
    masked = `${p[0]}.${p[1]}.•••.••`;
  } else if (host.includes(":")) {
    masked = `${host.split(":").slice(0, 2).join(":")}:…`;
  } else {
    const [first, ...rest] = host.split(".");
    masked = `${first.slice(0, 4)}•••${rest.length ? `.${rest.slice(-2).join(".")}` : ""}`;
  }
  return port ? `${masked}:${port}` : masked;
}

function splitHostPort(addr: string): [string, string | null] {
  const v6 = /^\[(.+)\]:(\d+)$/.exec(addr);
  if (v6) return [v6[1], v6[2]];
  const m = /^([^:]+):(\d+)$/.exec(addr);
  return m ? [m[1], m[2]] : [addr, null];
}

/** "2h 14m", "45s", "3d 2h". */
export function upFor(sinceMs: number | null | undefined, now = Date.now()): string | null {
  if (!sinceMs) return null;
  const s = Math.max(0, Math.round((now - sinceMs) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
}

/** The path line: "Tunnel via 203.0.•••.•• · up 2h 14m" or "Direct on home LAN · up 3h 2m". */
export function pathLine(s: VpnStatus, reveal: boolean, now = Date.now()): string {
  const up = upFor(s.upSinceMs, now);
  const tail = up ? ` · up ${up}` : "";
  switch (s.state) {
    case "connected":
      return `Tunnel via ${reveal ? s.endpoint ?? "the Firewalla" : maskAddress(s.endpoint)}${tail}`;
    case "direct":
      return `Direct on home LAN${tail}`;
    case "connecting":
      return "Connecting to the Firewalla…";
    case "error":
      return "Not connected";
    default:
      return s.override === "disconnect" ? "Disconnected by you" : "Off";
  }
}
