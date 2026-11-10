// Shared bits of the switch windows: port states, optics thresholds, the per-window
// history ring the sparklines and the port drawer draw from.
import { useEffect, useRef, useState } from "react";
import type { SwitchPort, SwitchSnapshot } from "../../lib/types";

/** Receive power below these is marginal (amber) or failing (red), for 100G/200G SR optics. */
export const RX_WARN_DBM = -7;
export const RX_CRIT_DBM = -10;

export type PortState = "up" | "down" | "disabled" | "errors";

export function opticTone(dbm: number | null | undefined): "good" | "warn" | "crit" | null {
  if (dbm == null) return null;
  return dbm < RX_CRIT_DBM ? "crit" : dbm < RX_WARN_DBM ? "warn" : "good";
}

export function capacityBps(p: SwitchPort): number {
  return (p.capacityMbps || (p.name.startsWith("qsfp") ? 200_000 : 1_000)) * 1e6;
}

/** Busiest direction as a share of the link, 0–100. */
export function loadPct(p: SwitchPort): number {
  return Math.min(100, (Math.max(p.rxBps, p.txBps) / capacityBps(p)) * 100);
}

export function errorTotal(p: SwitchPort): number {
  return p.rxErrors + p.txErrors + (p.fcsErrors ?? 0);
}

export function portState(p: SwitchPort, rising: boolean): PortState {
  if (p.disabled) return "disabled";
  if (!p.running) return "down";
  return rising ? "errors" : "up";
}

export const STATE_WORD: Record<PortState, string> = { up: "Up", down: "No link", disabled: "Disabled", errors: "Errors rising" };

/** "1-1" from "qsfp56-dd-1-1"; "ether1" stays. */
export function shortPort(name: string): string {
  const m = /^qsfp56-dd-(\d+)(?:-(\d+))?$/.exec(name);
  return m ? (m[2] ? `${m[1]}-${m[2]}` : m[1]) : name;
}

export function fmtDbm(v: number | null | undefined): string {
  return v == null ? "—" : `${v.toFixed(1)} dBm`;
}

export interface PortSample {
  ts: number;
  rx: number;
  tx: number;
  errors: number;
}

const KEEP_S = 300;

/**
 * The last five minutes of every port of one switch, recorded while the window is
 * open (seconds, oldest first). `startedAt` is when recording began, for the charts'
 * "History starts" marker. `rising` says which ports' error counters went up in the last minute.
 */
export function usePortRing(sw: SwitchSnapshot | undefined, tsMs: number | undefined) {
  const ring = useRef(new Map<string, PortSample[]>());
  const startedAt = useRef(Date.now() / 1000);
  const [version, setVersion] = useState(0);
  useEffect(() => {
    if (!sw || !tsMs) return;
    const ts = tsMs / 1000;
    for (const p of sw.ports) {
      const list = ring.current.get(p.name) ?? [];
      if (list.length && ts - list[list.length - 1].ts < 0.5) continue;
      list.push({ ts, rx: p.rxBps, tx: p.txBps, errors: errorTotal(p) });
      while (list.length && ts - list[0].ts > KEEP_S) list.shift();
      ring.current.set(p.name, list);
    }
    setVersion((v) => v + 1);
  }, [sw, tsMs]);

  const samples = (port: string, seconds = KEEP_S): PortSample[] => {
    const list = ring.current.get(port) ?? [];
    const now = list.length ? list[list.length - 1].ts : 0;
    return list.filter((s) => now - s.ts <= seconds);
  };
  const rising = (port: string): boolean => {
    const list = samples(port, 60);
    return list.length > 1 && list[list.length - 1].errors > list[0].errors;
  };
  return { samples, rising, startedAt: startedAt.current, version };
}
