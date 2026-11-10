// This Mac's box on the Topology: the VPN state word is for the VPN window, not for the Mac.
import { describe, expect, it } from "vitest";
import type { VpnState, VpnStatus } from "../lib/types";
import { macWords } from "./Topology";

const vpn = (state: VpnState, latencyMs: number | null = null) => ({ state, latencyMs }) as VpnStatus;

describe("macWords", () => {
  it("reads On this network whenever the Mac isn't going through the VPN", () => {
    for (const s of [null, vpn("off"), vpn("direct"), vpn("connecting")]) expect(macWords(s), s?.state).toBe("On this network");
  });
  it("names the VPN when the Mac goes through it", () => {
    expect(macWords(vpn("connected"))).toBe("VPN");
    expect(macWords(vpn("connected", 22.4))).toBe("VPN · 22 ms");
  });
  it("says when the VPN has a problem", () => {
    expect(macWords(vpn("error"))).toBe("Problem");
  });
});
