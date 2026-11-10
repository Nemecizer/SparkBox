// The VPN window (P1.7): the "Direct at home" switch, saved through vpn_set_mode, and the
// tunnel's settings and counters marked "not in use" while the cluster is reached directly.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { inventoryFixture, prefsFixture } from "../dev/mockBackend";
import type { VpnMode, VpnState, VpnView } from "../lib/types";
import { mockCalls, mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { VpnWindow } from "./VpnWindow";

// The Mac scope is open, so the switch's change goes straight to the backend.
vi.mock("../lib/admin", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/admin")>()),
  withAdmin: vi.fn(async (fn: () => Promise<unknown>) => fn()),
}));

function view(state: VpnState, mode: VpnMode = "auto", homePin = false): VpnView {
  const tunnel = state === "connected";
  return {
    settings: {
      mode,
      homePin,
      profile: {
        protocol: "wireguard", endpointHost: "203.0.113.24", endpointPort: 51820, addresses: ["10.189.171.3/32"], dns: ["10.189.171.1"], mtu: 1412,
        peerPublicKey: "pk", hasPresharedKey: false, allowedIps: ["192.168.50.0/24"], keepalive: 25, obfuscation: {}, importedAt: 1_792_675_200,
      },
      forwards: [],
    } as VpnView["settings"],
    status: {
      state, protocol: tunnel ? "wireguard" : null, endpoint: tunnel ? "203.0.113.24:51820" : null, handshakeAgeSec: tunnel ? 12 : null,
      rxBytes: tunnel ? 2_000_000 : 0, txBytes: tunnel ? 1_000_000 : 0, latencyMs: 5.6, message: null, forwards: [],
    },
    log: [],
    reach: [],
    rates: [],
  } as VpnView;
}

async function show(first: VpnView, handlers: Record<string, (a: any) => unknown> = {}) {
  mockTauri({ vpn_get: () => structuredClone(first), prefs_get: () => structuredClone(prefsFixture), get_inventory: () => structuredClone(inventoryFixture), ...handlers });
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [900, 700]} isKnown={() => true}><VpnWindow /></WindowManagerProvider>);
  });
  return r.container;
}

const fact = (root: HTMLElement, name: string) => [...root.querySelectorAll(".vpn-facts div")].find((d) => d.querySelector("dt")?.textContent === name)!.querySelector("dd")!.textContent;

describe("VPN window", () => {
  it("uses the replacement profile endpoint instead of silently retaining the old one", async () => {
    const root = await show(view("direct"), { vpn_import: () => view("connecting") });
    const input = root.querySelector<HTMLInputElement>('input[type="file"]')!;
    await act(async () => { fireEvent.change(input, { target: { files: [{ name: "working.conf", text: async () => "profile-with-current-endpoint" }] } }); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Import replacement" })); });
    expect(mockCalls.find((c) => c.cmd === "vpn_import")?.args).toEqual({ confText: "profile-with-current-endpoint", endpointHost: "" });
  });

  it("marks the tunnel's settings and counters not in use while Direct", async () => {
    const root = await show(view("direct"));
    expect(fact(root, "Protocol")).toBe("WireGuard · not in use");
    expect(fact(root, "MTU")).toBe("1412 · not in use");
    expect(fact(root, "DNS")).toBe("10.189.171.1 · not in use");
    expect(fact(root, "Handshake")).toBe("Not in use");
    expect(fact(root, "Received / sent")).toBe("Not in use");
    expect(fact(root, "Connect time")).toBe("5.6 ms");
    expect(root.querySelector(".vpn-no-traffic")?.textContent).toBe("Tunnel not in use: the cluster answers directly");
  });

  it("doesn't say the cluster answers when no node did", async () => {
    // Auto stays Direct with every node silent: on their network, or Direct at home with no network.
    for (const message of [
      "No node answers, and this Mac (192.168.50.229) is on their network, so the tunnel wouldn't help. They may be off or restarting.",
      "This Mac has no network right now. Direct at home is on, so SparkBox waits for the network instead of starting the tunnel.",
    ]) {
      const v = view("direct");
      v.status = { ...v.status, latencyMs: null, message };
      const root = await show(v);
      expect(root.querySelector(".vpn-no-traffic")?.textContent).toBe(`Tunnel not in use. ${message}`);
      expect(fact(root, "Connect time")).toBe("—");
      cleanup();
    }
  });

  it("shows them as they are while the tunnel is up", async () => {
    const root = await show(view("connected"));
    expect(fact(root, "Protocol")).toBe("WireGuard");
    expect(fact(root, "MTU")).toBe("1412");
    expect(fact(root, "Handshake")).toBe("12 s ago");
    expect(fact(root, "Received / sent")).not.toMatch(/not in use/i);
    expect(root.querySelector(".vpn-no-traffic")).toBeNull();
  });

  it("turns Direct at home on through vpn_set_mode, the mode unchanged", async () => {
    await show(view("direct"), { vpn_set_mode: ({ mode, homePin }: { mode: VpnMode; homePin?: boolean }) => view("direct", mode, !!homePin) });
    const pin = screen.getByRole("switch", { name: "Direct at home" });
    expect(pin.getAttribute("aria-checked")).toBe("false");
    expect(pin).not.toBeDisabled();
    await act(async () => {
      fireEvent.click(pin);
    });
    expect(mockCalls.filter((c) => c.cmd === "vpn_set_mode")).toEqual([{ cmd: "vpn_set_mode", args: { mode: "auto", homePin: true } }]);
    expect(screen.getByRole("switch", { name: "Direct at home" }).getAttribute("aria-checked")).toBe("true");
    // Its help says what it does.
    expect(within(document.body).getByRole("switch", { name: "Direct at home" }).title).toMatch(/Leave this off when away/);
  });

  it("is greyed outside Auto, where nothing decides by itself", async () => {
    await show(view("connected", "always", true));
    const pin = screen.getByRole("switch", { name: "Direct at home" });
    expect(pin).toBeDisabled();
    expect(pin.getAttribute("aria-checked")).toBe("true");
    expect(pin.title).toBe("Only Auto decides by itself when to use the tunnel.");
  });
});
