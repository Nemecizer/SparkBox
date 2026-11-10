// One node's chart data for a time range: the in-memory hour at 1 s for short
// ranges, the Rust history store (min/avg/max buckets) from 6 hours up.
import { useMemo } from "react";
import { usesStore } from "../../lib/chartScale";
import { useHistory, type HistoryMetric } from "../../lib/historyApi";
import { chartWindow, getHistory, historyStart, type SeriesKey } from "../../lib/store";

export interface NodeSeries {
  /** Unix seconds. */
  ts: number[];
  get(key: SeriesKey): (number | null)[];
  band(key: SeriesKey): { min: (number | null)[]; max: (number | null)[] } | undefined;
  /** The x window, [now − range, now] (collapsed to the data when history is truly missing). */
  range: [number, number];
  /** When recorded history begins, if that's inside the window. */
  startsAt: number | null;
  /**
   * When this source's history begins (historyStart()): the minutes before this run's first
   * reading read "No data recorded", as on the Dashboard, instead of a window cut to this run.
   */
  recordedFrom: number | null;
  loading: boolean;
}

const EMPTY: (number | null)[] = [];

export function useNodeSeries(nodeId: string, rangeS: number, keys: SeriesKey[], nowS: number): NodeSeries {
  const stored = usesStore(rangeS);
  const hist = useHistory(`node:${nodeId}`, keys as HistoryMetric[], rangeS, stored);
  const live = stored ? undefined : getHistory(nodeId, rangeS);
  return useMemo(() => {
    if (stored) {
      const ts = hist?.ts.map((t) => t / 1000) ?? [];
      const to = nowS;
      const from = to - rangeS;
      const starts = hist?.startsAtMs != null ? hist.startsAtMs / 1000 : null;
      return {
        ts,
        get: (k) => hist?.series[k] ?? EMPTY,
        band: (k) => (hist?.min?.[k] && hist?.max?.[k] ? { min: hist.min[k], max: hist.max[k] } : undefined),
        range: [from, to],
        startsAt: starts != null && starts > from ? starts : null,
        recordedFrom: starts,
        loading: !hist,
      };
    }
    const ts = live?.ts ?? [];
    const to = ts.length ? ts[ts.length - 1] : nowS;
    const first = ts.findIndex((_, i) => keys.some((k) => live?.[k][i] != null));
    const recordedFrom = historyStart();
    const w = chartWindow(first >= 0 ? ts.slice(first) : [], to, rangeS, recordedFrom);
    return {
      ts,
      get: (k) => live?.[k] ?? EMPTY,
      band: () => undefined,
      range: w.range,
      startsAt: w.startsAt,
      recordedFrom,
      loading: false,
    };
    // `live` is a new object only when a new second was recorded.
  }, [stored, hist, live, rangeS, nowS, keys.join(",")]);
}

/** Element-wise a − b, keeping gaps. */
export function minus(a: (number | null)[], b: (number | null)[]): (number | null)[] {
  return a.map((v, i) => (v == null || b[i] == null ? v : Math.max(0, v - (b[i] as number))));
}
