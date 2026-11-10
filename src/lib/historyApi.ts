// Long-range history from the Rust store (src-tauri/src/history): 1 s for the
// last hour, then 10 s, 1 min and 15 min buckets kept for a day, a week and a
// month, with min/avg/max per bucket. Missing time comes back as null.
//
// Entities: "node:<id>", "endpoint:<id>", "container:<node>/<name>",
// "port:<switch>/<port>" and "net:<node>/<iface>" (RoCE rates). Time a node
// agent recorded while SparkBox was closed comes back in `agentRanges`.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { inTauri } from "./api";

export type HistoryMetric =
  | "cpu" | "gpu" | "memUsed" | "gpuAlloc" | "power" | "gpuTemp"
  | "fabricRx" | "fabricTx" | "rdmaRx" | "rdmaTx" | "lanRx" | "lanTx" | "diskRead" | "diskWrite" | "memCache" | "smClock";

/** "endpoint:<id>": tokens/s, latency in ms, requests, KV cache %, preemptions. */
export type EndpointMetric = "genTps" | "promptTps" | "ttftP50" | "ttftP95" | "itlP50" | "running" | "waiting" | "kvPct" | "preemptions" | "canaryMs";
/** "container:<node>/<name>": CPU % and memory bytes. */
export type ContainerMetric = "cpu" | "mem";
/** "port:<switch>/<port>": bits/s, errors per second, weakest lane's receive power (dBm). */
export type PortMetric = "rxBps" | "txBps" | "errors" | "rxPowerDbm";
/** "net:<node>/<iface>": RoCE counters as events per second. */
export type NetMetric = "ecn" | "cnpSent" | "cnpHandled" | "oos" | "seqErr" | "timeouts" | "pauseRx" | "pauseTx" | "discards" | "crc";
export type AnyMetric = HistoryMetric | EndpointMetric | ContainerMetric | PortMetric | NetMetric;

export interface HistoryResult {
  /** Bucket start times, unix ms. */
  ts: number[];
  /** Averages per bucket; null where nothing was recorded. */
  series: Record<string, (number | null)[]>;
  /** Lowest and highest reading in each bucket (absent for the 1 s tier). */
  min?: Record<string, (number | null)[]>;
  max?: Record<string, (number | null)[]>;
  /** "1s", "10s", "1m" or "15m". */
  tier: string;
  stepMs: number;
  /** The first time with any data in this tier, or null when it's empty. */
  startsAtMs: number | null;
  /** Stretches [from, to] (unix ms) filled from a node agent's own history: real data, no gap band. */
  agentRanges?: [number, number][];
}

/** entity is "node:<id>", "endpoint:<id>", "container:<node>/<name>", "port:<switch>/<port>" or "net:<node>/<iface>". */
export function queryHistory(entity: string, metrics: AnyMetric[], fromMs: number, toMs: number, maxPoints: number): Promise<HistoryResult> {
  return invoke<HistoryResult>("history_query", { entity, metrics, fromMs, toMs, maxPoints });
}

/**
 * History for the last `rangeS` seconds, refreshed as time moves on (every 1/60 of the
 * range, at least every 10 s) and at once when the store says this entity's history grew
 * (`history://filled`: an agent's recording arrived). Returns null while loading or when
 * `enabled` is false.
 */
export function useHistory(entity: string | null, metrics: AnyMetric[], rangeS: number, enabled = true, maxPoints = 720): HistoryResult | null {
  const [result, setResult] = useState<HistoryResult | null>(null);
  const key = metrics.join(",");
  useEffect(() => {
    if (!enabled || !entity || !inTauri) {
      setResult(null);
      return;
    }
    let alive = true;
    const load = () => {
      const now = Date.now();
      queryHistory(entity, key.split(",") as AnyMetric[], now - rangeS * 1000, now, maxPoints)
        .then((r) => alive && setResult(r))
        .catch(() => alive && setResult(null));
    };
    load();
    const every = Math.max(10_000, (rangeS * 1000) / 60);
    const t = window.setInterval(load, every);
    const filled = listen<{ entities: string[] }>("history://filled", (e) => {
      if (e.payload?.entities?.includes(entity)) load();
    }).catch(() => () => {});
    return () => {
      alive = false;
      window.clearInterval(t);
      void filled.then((unlisten) => unlisten());
    };
  }, [entity, key, rangeS, enabled, maxPoints]);
  return result;
}
