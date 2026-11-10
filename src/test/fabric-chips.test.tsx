// The quiet Fabric matrix (§C10, M14): a healthy cell is one line, and a counter chip shows
// only while its counter moved in the last hour: in tone while moving, grey after.
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { IfaceHealth } from "../lib/fabric";
import type { HistoryResult } from "../lib/historyApi";
import { ChipRow, hourStats, quietChips, sortCabling } from "../windows/fabric/common";

const NOW = 1_800_000_000_000;

function iface(rates: Partial<Record<string, number>> = {}, p: Partial<IfaceHealth> = {}): IfaceHealth {
  const c = (key: string, group: IfaceHealth["counters"][number]["group"]) => ({ key, label: key, total: 0, perS: rates[key] ?? 0, group });
  return {
    node: "cell2", nodeName: "cell2", iface: "enP2p1s0f1np1", half: 1, up: true, speedMbps: 200_000, mtu: 9000, ip: "192.168.101.12",
    rdmaDev: "roceP2p1s0f1", rxBps: 1e6, txBps: 3e7, rdmaRxBps: 0, rdmaTxBps: 0, rdmaShare: 0.98, switchPort: "qsfp56-dd-1-2", switchId: "crs804",
    mappedBy: "mac", cablingOk: true, expectedPort: null, verdict: { kind: "lossless", level: "ok", title: "Lossless", detail: "", action: null } as IfaceHealth["verdict"],
    counters: [
      c("outOfSequence", "loss"), c("packetSeqErr", "loss"), c("rxCrcErrorsPhy", "phy"), c("rxDiscardsPhy", "drop"),
      c("ecnMarked", "congestion"), c("txPause", "pause"), c("rxPause", "pause"),
    ],
    lossLastHour: 0, lastLossMs: null,
    ...p,
  };
}

/** An hour of 1-minute buckets where `metric` ran at `perS` during minute `at` (minutes before NOW). */
function hour(metric: string, perS: number, at: number): HistoryResult {
  const ts = Array.from({ length: 60 }, (_, i) => NOW - (60 - i) * 60_000);
  return { ts, series: { [metric]: ts.map((t) => (t === NOW - at * 60_000 ? perS : 0)) }, tier: "1m", stepMs: 60_000, startsAtMs: ts[0] };
}

describe("fabric chips", () => {
  it("a healthy interface renders zero chips and one quiet line", () => {
    const i = iface({ ecnMarked: 2 });
    expect(quietChips(i, hourStats(null), NOW)).toEqual([]);
    const { container } = render(<ChipRow items={quietChips(i, hourStats(null), NOW)} quiet="ECN 2.0/s" label="Counters" />);
    expect(container.querySelectorAll(".fab-chip")).toHaveLength(0);
    expect(container.textContent).toBe("No errors in 1 h · ECN 2.0/s");
  });

  it("a counter moving now is a chip in its tone; ECN never is", () => {
    const chips = quietChips(iface({ rxCrcErrorsPhy: 17, ecnMarked: 900 }), hourStats(null), NOW);
    expect(chips.map((c) => [c.label, c.tone])).toEqual([["PHY errors", "crit"]]);
  });

  it("chips are named for what they sum: Retransmits, PHY errors", () => {
    const chips = quietChips(iface({ outOfSequence: 100, packetSeqErr: 34, rxCrcErrorsPhy: 54 }), hourStats(null), NOW);
    const { container } = render(<ChipRow items={chips} label="Counters" now={NOW} />);
    expect([...container.querySelectorAll(".fab-chip")].map((c) => c.textContent)).toEqual(["Retransmits134/s", "PHY errors54/s"]);
  });

  it("a counter that moved earlier in the hour is grey: \"PHY errors +412 · 12 min ago\"", () => {
    // 412 errors in one minute, ending 12 minutes ago.
    const h = hourStats(hour("crc", 412 / 60, 13));
    const chips = quietChips(iface(), h, NOW);
    expect(chips.map((c) => c.label)).toEqual(["PHY errors"]);
    const { container } = render(<ChipRow items={chips} label="Counters" now={NOW} />);
    const chip = container.querySelector(".fab-chip")!;
    expect(chip.classList.contains("past")).toBe(true);
    expect(chip.textContent).toBe("PHY errors+412 · 12 min ago");
    // Over an hour ago: gone.
    expect(quietChips(iface(), hourStats(hour("crc", 5, 61)), NOW)).toEqual([]);
  });

  it("orders cabling messages by node, half 1 before half 2", () => {
    const m = (node: string, half: 0 | 1) => ({ node, iface: `${node}-${half}`, half, switchId: "crs804", port: "p", expectedPort: "q", message: `${node} half ${half + 1}` });
    expect(sortCabling([m("cell2", 1), m("cell1", 1), m("cell2", 0), m("cell1", 0)], ["cell1", "cell2"]).map((c) => c.message)).toEqual(["cell1 half 1", "cell1 half 2", "cell2 half 1", "cell2 half 2"]);
  });
});
