import { describe, expect, it } from "vitest";
import { cloneAdvisory } from "../windows/node/SystemTab";
import { restartedWords } from "../windows/NodeDetail";
import { endedWords, seenWords, type Boot } from "./boots";

const boot = (p: Partial<Boot>): Boot => ({ bootId: "9cabf4f7e39d4c62b9d620e269501402", startMs: 1, source: "journal", ...p });

describe("System › Boots in words", () => {
  it("says how each boot ended and how SparkBox knows of it", () => {
    expect(endedWords(boot({ ended: "unclean" }), false)).toBe("Ended without a shutdown (power loss, hang or watchdog)");
    expect(endedWords(boot({ ended: "clean" }), false)).toBe("Clean shutdown");
    expect(endedWords(boot({ ended: "clean" }), false, boot({ planned: true, source: "live" }))).toBe("Clean shutdown (SparkBox restarted it)");
    expect(endedWords(boot({ ended: "unknown" }), false)).toBe("Not known: the journal no longer has it");
    expect(endedWords(boot({ ended: null, endMs: Date.now() - 86_400_000 }), false)).toBe("Not read from the journal yet");
    expect(endedWords(boot({ ended: null, endMs: Date.now() - 20 * 86_400_000 }), false), "SparkBox reads 14 days back").toBe("Not read: more than 14 days ago");
    expect(endedWords(boot({}), true)).toBe("Running now");
    expect([seenWords(boot({ source: "journal" })), seenWords(boot({ source: "live" })), seenWords(boot({ source: "first" }))]).toEqual(["Found in the journal", "Live", "Running when SparkBox connected"]);
  });

  it("names the restart in the node header: today by its time, before that with its date", () => {
    const now = new Date(2026, 9, 29, 19, 4).getTime();
    const at = (h: number, m: number) => new Date(2026, 9, 29, h, m).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    // cell3 on Oct 29: the 5:45 PM boot followed one that ended without a shutdown, the 6:08 PM boot a clean one.
    expect(restartedWords(new Date(2026, 9, 29, 17, 45).getTime(), "unclean", now)).toBe(`restarted on its own ${at(17, 45)} · no clean shutdown`);
    expect(restartedWords(new Date(2026, 9, 29, 18, 8).getTime(), "clean", now)).toBe(`restarted outside SparkBox ${at(18, 8)} (clean shutdown)`);
    // Not read from the journal yet, or the journal no longer has it: only what SparkBox knows.
    expect(restartedWords(new Date(2026, 9, 29, 18, 8).getTime(), null, now)).toBe(`restarted outside SparkBox ${at(18, 8)}`);
    expect(restartedWords(new Date(2026, 9, 29, 18, 8).getTime(), "unknown", now)).toBe(`restarted outside SparkBox ${at(18, 8)}`);
    expect(restartedWords(new Date(2026, 9, 28, 23, 50).getTime(), "unclean", now)).toMatch(/^restarted on its own Oct 28, .* · no clean shutdown$/);
  });
});

describe("the cloned host key advisory (P1.8)", () => {
  it("names the nodes that share cell2's key and the ones to regenerate", () => {
    expect(cloneAdvisory("cell2", ["cell1", "cell3"])).toBe(
      "Shared with cell1, cell3 (cloned image): any node holding this key can pass as the others. Fix: regenerate /etc/ssh/ssh_host_* on cell2 and cell3, then re-trust them (your call; not planned).",
    );
    // From cell1's side the same two are regenerated: the first by name keeps its key.
    expect(cloneAdvisory("cell1", ["cell2", "cell3"])).toContain("on cell2 and cell3, then");
    expect(cloneAdvisory("gx10-0000", [])).toBeNull();
  });
});
