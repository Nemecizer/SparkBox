import { describe, expect, it } from "vitest";
import { alertCounts, alertScope, alertShort, alertShownAs, entityKey, flapWords, fmtClock, fmtFor, needsReason, silenceLine, summarize, type Alert, type HealthState, type Silence } from "./health";
import { dayWord, fmtRange } from "../windows/AlertsWindow";

const base: Alert = {
  id: "disk-full:disk:gx10-0000:/", rule: "disk-full", category: "disk", severity: "warn", state: "firing",
  entity: { kind: "disk", id: "/", node: "gx10-0000", switch: null, label: "Root disk on gx10-0000" },
  message: "Root disk 85% full", summary: "gx10-0000 disk 85%", detail: null, value: 85,
  sinceMs: 0, firingSinceMs: 0, resolvedMs: null, ackedMs: null, mutedUntilMs: null, counted: true,
};

function state(p: Partial<HealthState["summary"]>): HealthState {
  return { tsMs: 1, alerts: [], summary: { warn: 0, crit: 0, pending: 0, muted: 0, ...p }, nodes: {}, switches: {}, lastAlertMs: null };
}

describe("health summary", () => {
  it("counts incidents and keeps recovering, planned and known issues apart", () => {
    const s = summarize(state({
      crit: 1, warn: 2, recovering: 1, planned: 1, alerts: 4,
      known: [{ alertId: "a", incidentId: "i", title: "gx10-0000 disk 85%", severity: "warn", why: "silenced", untilMs: 5, reason: "expanding disk Friday" }],
      pendingAlerts: [{ alertId: "p", title: "cell3 offline", severity: "crit", sinceMs: 1, firesAtMs: 31 }],
    }));
    expect([s.crit, s.warn, s.recovering, s.planned, s.known, s.alerts]).toEqual([1, 2, 1, 1, 1, 4]);
    expect(s.knownIssues[0].reason).toBe("expanding disk Friday");
    expect(s.pending.map((p) => p.title)).toEqual(["cell3 offline"]);
  });

  it("reads an older engine's summary without the new fields", () => {
    const s = summarize(state({ crit: 0, warn: 1, muted: 2 }));
    expect([s.known, s.recovering, s.pending.length, s.alerts]).toEqual([2, 0, 0, 1]);
  });

  it("never counts what's acknowledged, silenced, recovering or planned", () => {
    expect(alertCounts(base)).toBe(true);
    expect(alertCounts({ ...base, counted: false, ackedMs: 1 })).toBe(false);
    // An older engine: firing and not muted counts.
    const old = { ...base, counted: undefined };
    expect(alertCounts(old, 10)).toBe(true);
    expect(alertCounts({ ...old, mutedUntilMs: 20 }, 10)).toBe(false);
    expect(alertShownAs({ ...base, planned: "cell3 restarting (SparkBox, 10:42)" })).toBe("planned");
    expect(alertShownAs({ ...base, silenceId: "s" })).toBe("silenced");
    expect(alertShownAs({ ...base, recoversAtMs: 9 })).toBe("recovering");
    expect(alertShownAs({ ...base, ackedMs: 9 })).toBe("acknowledged");
    expect(alertShownAs({ ...base, state: "pending" })).toBe("pending");
    expect(alertShownAs(base)).toBe("firing");
  });

  it("words the Firing column and the recovering countdown", () => {
    expect(fmtFor(42_000)).toBe("42 s");
    expect(fmtFor(3 * 60_000)).toBe("3 min");
    expect(fmtFor(125 * 60_000)).toBe("2 h 5 min");
    expect(fmtFor(2 * 3600_000)).toBe("2 h");
    expect(fmtFor(30 * 3600_000)).toBe("1 day");
    expect(fmtClock(94_000)).toBe("1:34");
    expect(fmtClock(-5)).toBe("0:00");
  });

  it("says what a silence is and when it ends", () => {
    const s: Silence = { id: "s", matcher: { rule: "disk-full" }, until: { kind: "never" }, reason: "expanding disk Friday", createdMs: 0, createdBy: "Demo", source: "demo", label: "Root disk on gx10-0000", duration: "never" };
    expect(silenceLine(s)).toBe("Silenced indefinitely · expanding disk Friday");
    expect(silenceLine({ ...s, until: { kind: "resolved" }, reason: "" })).toBe("Silenced until it clears");
    const at = new Date(2026, 9, 29, 10, 42).getTime();
    expect(silenceLine({ ...s, until: { kind: "at", ms: at }, reason: "" }, at - 3600_000)).toMatch(/^Silenced until 10:42/);
    expect(needsReason("1w") && needsReason("never") && !needsReason("1h") && !needsReason("resolved")).toBe(true);
    expect(entityKey(base)).toBe("disk:gx10-0000:/");
  });
});

describe("r4 unlocks", () => {
  it("acknowledging, ending a silence and short silences are node scope; a week or longer in Live is Mac scope", () => {
    expect(alertScope("alert_ack", false)).toBe("node");
    expect(alertScope("alert_unsilence", false)).toBe("node");
    for (const d of ["1h", "1d", "resolved"] as const) expect(alertScope("alert_silence", false, d)).toBe("node");
    expect(alertScope("alert_silence", false, "1w")).toBe("mac");
    expect(alertScope("alert_silence", false, "never")).toBe("mac");
    // Demo asks only for the Demo unlock.
    expect(alertScope("alert_silence", true, "never")).toBe("node");
  });
});

describe("r4 words", () => {
  it("says how an alert flaps, and its short label", () => {
    const now = Date.UTC(2026, 9, 29, 9, 30);
    expect(flapWords({ count: 4, sinceMs: now - 30 * 60_000 }, now)).toBe("Flapping · 4 times in 30 min");
    expect(flapWords({ count: 2, sinceMs: now - 20_000 }, now)).toBe("Flapping · 2 times in 1 min");
    expect(alertShort({ ...base, short: "Disk full" })).toBe("Disk full");
    // An older backend without short labels: the summary without the node, at most 24 characters.
    expect(alertShort(base)).toBe("disk 85%");
    expect(alertShort({ ...base, summary: "gx10-0000 a very long summary that would not fit" }).length).toBeLessThanOrEqual(24);
  });

  it("prints a recabling's span and the Deliveries day headers", () => {
    const d = (h: number, m: number, day = 26) => new Date(2026, 9, day, h, m).getTime();
    expect(fmtRange(d(14, 16), d(14, 29))).toMatch(/^Oct 26, 2:16\u2013?–?2:29\s?PM$/);
    expect(fmtRange(d(17, 8), d(17, 8))).toMatch(/^Oct 26, 5:08\s?PM$/);
    expect(fmtRange(d(23, 50), d(0, 10, 27))).toMatch(/^Oct 26, 11:50\s?PM – Oct 27, 12:10\s?AM$/);
    const now = d(9, 30, 29);
    expect(dayWord(d(6, 49, 29), now)).toBe("Today");
    expect(dayWord(d(22, 0, 28), now)).toBe("Yesterday");
    expect(dayWord(d(14, 16, 26), now)).toMatch(/Monday, Oct 26/);
  });
});
