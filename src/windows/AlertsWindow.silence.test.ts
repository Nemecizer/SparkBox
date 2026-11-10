// P1.45: what the Silence sheet asks for. One alert or one incident for an hour or a day is node
// scope; a rule everywhere, until it clears (at most 7 days), or a week or longer needs Mac scope
// with Live data and a reason (health/silence.rs needs_mac).
import { describe, expect, it } from "vitest";
import { needsMac, silenceNote } from "./AlertsWindow";

const one = { rule: "node-offline", entity: "node::cell3" };
const incident = { incident: "node:cell3" };
const ruleWide = { rule: "node-offline" };

describe("the Silence sheet", () => {
  it("asks for Mac scope and a reason only beyond one alert for an hour or a day", () => {
    for (const m of [one, incident]) {
      expect(needsMac(m, "1h")).toBe(false);
      expect(needsMac(m, "1d")).toBe(false);
      for (const d of ["resolved", "1w", "never"] as const) expect(needsMac(m, d)).toBe(true);
    }
    for (const d of ["1h", "1d", "1w", "resolved", "never"] as const) expect(needsMac(ruleWide, d)).toBe(true);
    expect(needsMac({ entity: "node::cell3" }, "1h")).toBe(true);
  });

  it("says until it clears ends after 7 days at most, and what a rule-wide silence needs", () => {
    expect(silenceNote(incident, "cell3", "resolved", false)).toBe("Until it clears (at most 7 days): it ends when the alert clears, or after 7 days if it doesn't. It needs Mac scope (Touch ID) and a reason.");
    expect(silenceNote(ruleWide, "every node-offline alert", "1h", false)).toBe("Every node-offline alert, on every node: a silence that wide needs Mac scope (Touch ID) and a reason.");
    expect(silenceNote(ruleWide, "every node-offline alert", "resolved", false)).toBe("Every node-offline alert, on every node, until it clears (at most 7 days): a silence that wide needs Mac scope (Touch ID) and a reason.");
    // Demo silences are node scope: only the reason.
    expect(silenceNote(ruleWide, "every node-offline alert", "1h", true)).toBe("Every node-offline alert, on every node: a silence that wide needs a reason.");
    expect(silenceNote(one, "cell3", "1h", false)).toBeNull();
  });
});
