import { describe, expect, it } from "vitest";
import { endpointStateLabel, issueSummary, plural, POWER, windowTitle, worstSeverity } from "./vocabulary";

describe("vocabulary", () => {
  it("says Restart… and Shut down… (they ask first) and Wake (it doesn't)", () => {
    expect(POWER).toEqual({ restart: "Restart…", shutdown: "Shut down…", wake: "Wake" });
  });

  it("counts alerts critical first, and never says issues", () => {
    expect(issueSummary(1, 2)).toBe("1 critical · 2 warnings");
    expect(issueSummary(0, 1)).toBe("1 warning");
    expect(issueSummary(3, 0)).toBe("3 critical");
    expect(issueSummary(0, 0)).toBe("No alerts");
    expect(worstSeverity(0, 2)).toBe("warn");
    expect(worstSeverity(1, 2)).toBe("crit");
    expect(worstSeverity(0, 0)).toBeNull();
  });

  it("labels model states: Ready, Loading 42%, Down · reason, Stopped", () => {
    expect(endpointStateLabel("ready")).toBe("Ready");
    expect(endpointStateLabel("loading", { pct: 41.6 })).toBe("Loading 42%");
    expect(endpointStateLabel("down", { reason: "Pipeline stage 3 of 3 on cell3 is offline" })).toBe("Down · Pipeline stage 3 of 3 on cell3 is offline");
    expect(endpointStateLabel("stopped")).toBe("Stopped");
  });

  it("titles windows the way the sidebar labels them", () => {
    expect(["containers", "workloads", "services", "vpn", "server", "providers", "updates", "activity", "fleet"].map(windowTitle))
      .toEqual(["Containers", "Workloads", "Services", "VPN", "Server", "Providers", "Updates", "Activity log", "Fleet Command"]);
    expect(plural(1, "window")).toBe("1 window");
    expect(plural(3, "window")).toBe("3 windows");
  });
});
