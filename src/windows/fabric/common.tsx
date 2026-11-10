// Pieces the Fabric window, its drawer and the node's RoCE panel share: the verdict
// word, the counter chips and the short-term traffic ring the sparklines draw from.
import { useEffect, useRef, useState } from "react";
import type { CablingIssue, CounterReading, FabricState, IfaceHealth, VerdictKind, VerdictLevel } from "../../lib/fabric";
import { AdvisoryGlyph, StateGlyph, type GlyphState } from "../../components/ui";
import { fmtRelative } from "../../lib/format";
import { useHistory, type HistoryResult, type NetMetric } from "../../lib/historyApi";

/** A verdict in the one severity mapping (§2.3): ok a quiet dot, informative a neutral ring. */
export function verdictGlyph(level: VerdictLevel): GlyphState {
  return level === "crit" ? "crit" : level === "warn" ? "warn" : level === "info" ? "affected" : "ok";
}

/** The verdict's glyph with its word for screen readers; PFC off (known configuration) is the grey ⓘ advisory. */
export function VerdictMark({ level, title, kind }: { level: VerdictLevel; title: string; kind?: VerdictKind }) {
  if (kind === "notLossless" && level === "info") return <AdvisoryGlyph title={title} />;
  return <StateGlyph state={verdictGlyph(level)} label={title} />;
}

/** A rate for a chip: "0", "12/s", "4.1k/s", "50k/s". */
export function fmtPerS(v: number | null | undefined): string {
  if (v == null) return "—";
  if (v === 0) return "0";
  if (v >= 10_000) return `${(v / 1000).toFixed(0)}k/s`;
  if (v >= 1000) return `${(v / 1000).toFixed(1)}k/s`;
  if (v >= 10) return `${v.toFixed(0)}/s`;
  return `${v.toFixed(1)}/s`;
}

/** A cumulative counter: "41,213", "1.2M". */
/** A counter's total in the drawer's narrow column: "984", "10.7k", "1.2M" (the exact figure is its tooltip). */
export function fmtTotal(v: number | null | undefined): string {
  if (v == null) return "—";
  if (v >= 1_000_000) return `${(v / 1e6).toFixed(1)}M`;
  if (v >= 10_000) return `${(v / 1e3).toFixed(1)}k`;
  return v.toLocaleString();
}

export interface Chip {
  key: string;
  label: string;
  perS: number | null;
  /** Coloured only while it moves: loss, PHY and drops red; pause amber (red past 10k/s); ECN informative. */
  tone: "crit" | "warn" | "info" | null;
  title: string;
  /** Events in the last hour and when the counter last moved (from history), for "CRC +412 · 12 min ago". */
  hour?: { count: number; lastMs: number | null };
}

const sumRate = (cs: CounterReading[], pred: (c: CounterReading) => boolean): number | null => {
  const xs = cs.filter(pred).map((c) => c.perS).filter((v): v is number => v != null);
  return xs.length ? xs.reduce((a, b) => a + b, 0) : null;
};

/** The five chips a fabric cell shows. */
export function chips(i: IfaceHealth): Chip[] {
  const c = i.counters;
  const loss = sumRate(c, (x) => x.group === "loss" && ["outOfSequence", "packetSeqErr", "localAckTimeoutErr", "impliedNakSeqErr"].includes(x.key));
  const phy = sumRate(c, (x) => x.group === "phy" && x.key !== "rxIcrcErr");
  const drop = sumRate(c, (x) => x.group === "drop");
  const ecn = sumRate(c, (x) => x.key === "ecnMarked");
  const pause = sumRate(c, (x) => x.key === "txPause" || x.key === "rxPause");
  const t = (v: number | null, moving: Chip["tone"]): Chip["tone"] => (v != null && v > 0 ? moving : null);
  return [
    { key: "loss", label: "Retransmits", perS: loss, tone: t(loss, "crit"), title: "What RoCE resent, per second: out of sequence, sequence errors, ACK timeouts and implied NAKs together" },
    { key: "phy", label: "PHY errors", perS: phy, tone: t(phy, "crit"), title: "CRC and symbol errors together, per second" },
    { key: "drop", label: "Drops", perS: drop, tone: t(drop, "crit"), title: "Discards and out-of-buffer drops per second" },
    { key: "ecn", label: "ECN", perS: ecn, tone: t(ecn, "info"), title: "ECN-marked packets per second (DCQCN at work)" },
    { key: "pause", label: "Pause", perS: pause, tone: pause != null && pause > 10_000 ? "crit" : t(pause, "warn"), title: "PFC pause frames sent and received per second" },
  ];
}

/** History series behind each chip (`net:<node>/<iface>`, events per second). */
export const CHIP_METRICS: Record<string, NetMetric[]> = {
  loss: ["oos", "seqErr", "timeouts"],
  phy: ["crc"],
  drop: ["discards"],
  pause: ["pauseRx", "pauseTx"],
  ecn: ["ecn"],
};
const ALL_CHIP_METRICS = [...new Set(Object.values(CHIP_METRICS).flat())];

/**
 * Per chip, how many events the last hour of history holds and when the counter last moved
 * (rates × bucket length, summed). The live reading is added by `quietChips`.
 */
export function hourStats(h: HistoryResult | null | undefined): Record<string, { count: number; lastMs: number | null }> {
  const out: Record<string, { count: number; lastMs: number | null }> = {};
  const step = (h?.stepMs ?? 0) / 1000;
  for (const [chip, metrics] of Object.entries(CHIP_METRICS)) {
    let count = 0;
    let lastMs: number | null = null;
    for (const m of metrics) {
      const xs = h?.series[m] ?? [];
      xs.forEach((v, i) => {
        if (v == null || !(v > 0)) return;
        count += v * step;
        const t = (h!.ts[i] ?? 0) + h!.stepMs;
        if (lastMs == null || t > lastMs) lastMs = t;
      });
    }
    out[chip] = { count: Math.round(count), lastMs };
  }
  return out;
}

/** The last hour of an interface's counters, for its chips (one small query a minute). */
export function useHourStats(i: IfaceHealth) {
  const h = useHistory(`net:${i.node}/${i.iface}`, ALL_CHIP_METRICS, 3600, true, 60);
  return hourStats(h);
}

/**
 * The chips a cell shows (§C10): only for counters that moved in the last hour. Moving now:
 * in its tone with its rate. Moved earlier in the hour: grey, "CRC +412 · 12 min ago". ECN is
 * DCQCN doing its job, so it's never a chip (its rate reads in the quiet line and the drawer);
 * loss and PHY also count the server's own last-hour tally (lossLastHour, lastLossMs).
 */
export function quietChips(i: IfaceHealth, hour: Record<string, { count: number; lastMs: number | null }>, now: number = Date.now()): Chip[] {
  return chips(i)
    .filter((c) => c.key !== "ecn")
    .map((c) => {
      const h = hour[c.key] ?? { count: 0, lastMs: null };
      const fromServer = (c.key === "loss" || c.key === "phy") && i.lastLossMs != null && now - i.lastLossMs < 3_600_000;
      const lastMs = Math.max(h.lastMs ?? 0, fromServer ? i.lastLossMs! : 0) || null;
      const count = Math.max(h.count, fromServer && c.key === "loss" ? i.lossLastHour : 0);
      return { ...c, hour: { count, lastMs } };
    })
    .filter((c) => (c.perS != null && c.perS > 0) || (c.hour!.lastMs != null && now - c.hour!.lastMs < 3_600_000));
}

/** "ECN 12/s" for the quiet line while DCQCN marks packets; null when it doesn't. */
export function ecnNote(i: IfaceHealth): string | null {
  const e = chips(i).find((c) => c.key === "ecn");
  return e && e.perS != null && e.perS > 0 ? `ECN ${fmtPerS(e.perS)}` : null;
}

/**
 * One line of chips with a reserved height that never wraps; with none, the quiet line "No
 * errors in 1 h". Counters stay in the drawer and tooltips.
 */
export function ChipRow({ items, label, quiet, now = Date.now() }: { items: Chip[]; label: string; quiet?: string | null; now?: number }) {
  if (!items.length) {
    return <span className="fab-chips fab-chips-quiet" aria-label={label}>No errors in 1 h{quiet ? ` · ${quiet}` : ""}</span>;
  }
  return (
    <span className="fab-chips" role="list" aria-label={label}>
      {items.map((c) => {
        const moving = c.perS != null && c.perS > 0;
        const ago = c.hour?.lastMs ? fmtRelative(c.hour.lastMs, now) : "";
        return (
          <span key={c.key} role="listitem" className={`fab-chip${moving && c.tone ? ` tone-${c.tone}` : " past"}`} title={`${c.title}${c.hour ? ` · ${c.hour.count.toLocaleString()} in the last hour` : ""}`}>
            <span className="fab-chip-k">{c.label}</span>
            <span className="fab-chip-v num">{moving ? fmtPerS(c.perS) : `+${(c.hour?.count ?? 0).toLocaleString()}${ago ? ` · ${ago}` : ""}`}</span>
          </span>
        );
      })}
    </span>
  );
}

/** Cabling messages in reading order: by node, half 1 before half 2. */
export function sortCabling(list: CablingIssue[], nodeOrder: string[]): CablingIssue[] {
  const at = (id: string) => {
    const i = nodeOrder.indexOf(id);
    return i < 0 ? nodeOrder.length : i;
  };
  return [...list].sort((a, b) => at(a.node) - at(b.node) || a.half - b.half);
}

const KEEP_S = 180;

/** The last three minutes of each interface's fabric bytes (IP + RDMA), recorded while the window is open. */
export function useFabricRing(state: FabricState | null) {
  const ring = useRef(new Map<string, { ts: number; v: number }[]>());
  const [version, setVersion] = useState(0);
  useEffect(() => {
    if (!state) return;
    const ts = state.tsMs / 1000;
    for (const i of state.ifaces) {
      const key = `${i.node}/${i.iface}`;
      const list = ring.current.get(key) ?? [];
      if (list.length && ts - list[list.length - 1].ts < 0.5) continue;
      list.push({ ts, v: i.up ? i.rxBps + i.txBps + (i.rdmaRxBps ?? 0) + (i.rdmaTxBps ?? 0) : NaN });
      while (list.length && ts - list[0].ts > KEEP_S) list.shift();
      ring.current.set(key, list);
    }
    setVersion((v) => v + 1);
  }, [state]);
  return { version, values: (key: string) => (ring.current.get(key) ?? []).map((s) => (Number.isNaN(s.v) ? null : s.v)) };
}
