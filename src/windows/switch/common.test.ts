import { describe, expect, it } from "vitest";
import { maskAddress, pathLine, upFor } from "../../lib/vpn";
import type { SwitchPort, VpnStatus } from "../../lib/types";
import { errorTotal, loadPct, opticTone, portState, shortPort } from "./common";

const port = (p: Partial<SwitchPort>): SwitchPort => ({
  name: "qsfp56-dd-1-2", comment: "cell2", running: true, disabled: false, speed: "200G", rxBps: 0, txBps: 0,
  rxErrors: 0, txErrors: 0, moduleTempC: null, rxPowerDbm: null, capacityMbps: 200_000, ...p,
});

describe("switch ports", () => {
  it("grades optics at −7 and −10 dBm", () => {
    expect(opticTone(-2.1)).toBe("good");
    expect(opticTone(-7.5)).toBe("warn");
    expect(opticTone(-10.2)).toBe("crit");
    expect(opticTone(null)).toBeNull();
  });
  it("shows state by priority: disabled, no link, rising errors, up", () => {
    expect(portState(port({ disabled: true, running: false }), true)).toBe("disabled");
    expect(portState(port({ running: false }), true)).toBe("down");
    expect(portState(port({}), true)).toBe("errors");
    expect(portState(port({}), false)).toBe("up");
  });
  it("measures load against the link and counts every kind of error", () => {
    expect(loadPct(port({ rxBps: 50e9, txBps: 20e9 }))).toBe(25);
    expect(loadPct(port({ rxBps: 900e9 }))).toBe(100);
    expect(errorTotal(port({ rxErrors: 2, txErrors: 1, fcsErrors: 4 }))).toBe(7);
    expect(shortPort("qsfp56-dd-3-2")).toBe("3-2");
    expect(shortPort("ether1")).toBe("ether1");
  });
});

describe("VPN privacy", () => {
  it("masks public addresses", () => {
    expect(maskAddress("203.0.113.17:51820")).toBe("203.0.•••.••:51820");
    expect(maskAddress("203.0.113.17")).toBe("203.0.•••.••");
    expect(maskAddress("[2001:db8:1::5]:51820")).toBe("2001:db8:…:51820");
    expect(maskAddress("home.example.net")).toBe("home•••.example.net");
    expect(maskAddress(null)).toBe("—");
  });
  it("words the path, masked unless revealed", () => {
    const now = 1_800_000_000_000;
    const s = { state: "connected", endpoint: "203.0.113.17:51820", upSinceMs: now - (2 * 3600 + 14 * 60) * 1000 } as VpnStatus;
    expect(pathLine(s, false, now)).toBe("Tunnel via 203.0.•••.••:51820 · up 2h 14m");
    expect(pathLine(s, true, now)).toBe("Tunnel via 203.0.113.17:51820 · up 2h 14m");
    expect(pathLine({ ...s, state: "direct", upSinceMs: now - 45_000 }, false, now)).toBe("Direct on home LAN · up 45s");
    expect(pathLine({ ...s, state: "off", override: "disconnect" }, false, now)).toBe("Disconnected by you");
    expect(upFor(now - 3 * 86_400_000, now)).toBe("3d 0h");
  });
});
