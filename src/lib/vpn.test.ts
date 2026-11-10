import { describe, expect, it } from "vitest";
import { latencyTitle } from "../components/StatusBar";
import { mockCalls, mockTauri } from "../test/tauri";
import type { NodeSnapshot, VpnSettings } from "./types";
import { homePinOf, vpnSetHomePin } from "./vpn";

describe("one latency (N13, P1.7)", () => {
  it("quotes only SparkBox's own round trip, never a connect time that isn't the node's", () => {
    const cell2 = { id: "cell2", status: "online", latencyMs: 22 } as NodeSnapshot;
    expect(latencyTitle(cell2)).toBe("22 ms: round trip of SparkBox's last command over SSH.");
    expect(latencyTitle({ ...cell2, latencyMs: null } as NodeSnapshot)).toBeNull();
    expect(latencyTitle({ ...cell2, status: "offline" } as NodeSnapshot)).toBeNull();
  });
});

describe("Direct at home (P1.7)", () => {
  it("is read from the settings, off when an older vpn.json has none", () => {
    const base: VpnSettings = { mode: "auto", profile: null, forwards: [] };
    expect(homePinOf(base)).toBe(false);
    expect(homePinOf({ ...base, homePin: true } as VpnSettings)).toBe(true);
  });

  it("is set through vpn_set_mode with the mode unchanged (its gate and its prompt)", async () => {
    const view = { settings: { mode: "auto", profile: null, forwards: [], homePin: true } };
    mockTauri({ vpn_set_mode: () => view });
    await expect(vpnSetHomePin("auto", true)).resolves.toEqual(view);
    expect(mockCalls).toEqual([{ cmd: "vpn_set_mode", args: { mode: "auto", homePin: true } }]);
  });
});
