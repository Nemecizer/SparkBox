import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import crs804 from "../assets/hardware/crs804.json";

export interface HardwareProfile {
  manufacturer: string; model: string; summary: string; sourceUrl: string;
  groups: { connector: string; count: number; maxGbps: number; evidence: string }[];
}
export interface HardwareEntry {
  identity: string; profile: HardwareProfile | null; checkedAtMs: number;
  provider: string; model: string; error: string | null;
}
export interface HardwareView {
  enabled: boolean; demo: boolean;
  devices: { id: string; name: string; identity: string; builtin: boolean; entry: HardwareEntry | null }[];
}
export const crs804Profile: HardwareProfile = crs804;
export const isCrs804 = (board: string | null | undefined) => ["crs8044ddq", "crs8044ddqhrm", "crs804ddq"].includes((board ?? "").toLowerCase().replace(/[^a-z0-9]/g, ""));
export function useHardwareProfiles(source: string) {
  const [view, setView] = useState<HardwareView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let stopped = false;
    let busy = false;
    setView(null);
    const read = async () => {
      if (busy) return;
      busy = true;
      try {
        const next = await invoke<HardwareView | null>("hardware_profiles");
        if (!stopped) { setView(next); setError(null); }
      } catch { if (!stopped) setError("Hardware profiles could not be read."); }
      finally { busy = false; }
    };
    void read();
    const t = window.setInterval(read, 5000);
    return () => { stopped = true; window.clearInterval(t); };
  }, [source]);
  return { view, error };
}
