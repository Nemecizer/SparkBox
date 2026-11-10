import { describe, expect, it } from "vitest";
import { parsePopout, popoutLabel, popoutUrl } from "./popout";

describe("pop-out routes", () => {
  it("round-trips a window's kind and params through the address", () => {
    const url = popoutUrl("node", { id: "cell3", tab: "gpu" });
    expect(url.startsWith("index.html?popout=node&params=")).toBe(true);
    expect(parsePopout(url.slice(url.indexOf("?")))).toEqual({ kind: "node", params: { id: "cell3", tab: "gpu" } });
    expect(parsePopout("?popout=dashboard")).toEqual({ kind: "dashboard", params: {} });
    expect(parsePopout(new URL(`http://x/${popoutUrl("cluster-metrics")}`).search)).toEqual({ kind: "cluster-metrics", params: {} });
  });

  it("is the main window without one, and refuses a malformed one", () => {
    expect(parsePopout("")).toBeNull();
    expect(parsePopout("?foo=1")).toBeNull();
    expect(parsePopout("?popout=../etc")).toBeNull();
    expect(parsePopout("?popout=Node")).toBeNull();
    expect(parsePopout("?popout=node&params=not-json")).toBeNull();
    expect(parsePopout("?popout=node&params=[1,2]")).toBeNull();
    // Only string params are kept.
    expect(parsePopout(`?popout=node&params=${encodeURIComponent(JSON.stringify({ id: "cell1", n: 3 }))}`)).toEqual({ kind: "node", params: { id: "cell1" } });
  });

  it("labels windows pop-<kind>-<n>, taking the first free number", () => {
    expect(popoutLabel("dashboard", ["main"])).toBe("pop-dashboard-1");
    expect(popoutLabel("dashboard", ["main", "pop-dashboard-1", "pop-dashboard-3"])).toBe("pop-dashboard-2");
    expect(popoutLabel("cluster-metrics", [])).toBe("pop-cluster-metrics-1");
  });
});
