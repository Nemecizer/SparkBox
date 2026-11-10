// The Switch X's panel by hand and keyboard, and its detail panel's sources and label.
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { _resetAdminForTests } from "../../lib/admin";
import type { MspStatus, PortLabel } from "../../lib/firewalla";
import type { ClusterSnapshot, LldpNeighbor, NodeSnapshot, SwitchSnapshot } from "../../lib/types";
import { mockTauri } from "../../test/tauri";
import { WindowManagerProvider } from "../../wm/WindowManager";
import { buildPorts, type XPort } from "./switchX";
import { SwitchXDrawer } from "./SwitchXDrawer";
import { SwitchXLegend, SwitchXPanel } from "./SwitchXPanel";

const NOW = 1_800_000_000_000;
const sw = { id: "firewalla", name: "Firewalla Switch X", kind: "firewalla" } as SwitchSnapshot;
const off: MspStatus = { configured: false, domain: null, state: "off", message: null, lastOkMs: null, lastTryMs: null, nextPollMs: null, sw: null, held: false };
const SWITCH: LldpNeighbor = { system: "Firewalla-Switch-X", port: "Gi0/2", chassisId: "20:6d:31:be:00:15", portId: "Gi0/2" };
const CRS: LldpNeighbor = { system: "CRS804", port: "ether2", chassisId: "38:32:7a:79:9b:9e", portId: "ether2" };
const node = (name: string, peers: LldpNeighbor[]) => ({
  id: name, name, status: "online", lastSeenMs: NOW - 1000, system: { hostname: name },
  nets: [{ iface: "enP7s7", role: "lan", up: true, speedMbps: 1000, rxBps: 1e6, txBps: 2e6, rxErrors: 1, txErrors: 0, mac: `aa:${name}`, carrierChanges: 5, lldpPeers: peers }],
}) as unknown as NodeSnapshot;
const cluster = { tsMs: NOW, nodes: [node("cell1", [SWITCH, CRS]), node("cell2", [SWITCH, CRS])], switches: [sw] } as unknown as ClusterSnapshot;
const portsWith = (labels: Record<string, PortLabel> = {}) => buildPorts(cluster, sw, { labels, msp: off }, NOW).ports;

function Pick({ ports, onSelect }: { ports: XPort[]; onSelect: (n: number) => void }) {
  const [on, set] = useState<number | null>(null);
  return <SwitchXPanel name={sw.name} ports={ports} selected={on} onSelect={(n) => { set(n); onSelect(n); }} />;
}

function drawer(p: XPort, onClose = () => {}) {
  mockTauri({});
  return render(
    <WindowManagerProvider sizeOf={() => [1200, 800]} isKnown={() => true}>
      <SwitchXDrawer p={p} switchId="firewalla" cluster={cluster} msp={off} onClose={onClose} onSaved={() => {}} />
    </WindowManagerProvider>,
  );
}

describe("the Switch X panel", () => {
  it("offers all 12 ports as buttons, by click, Return and Space", () => {
    const seen: number[] = [];
    render(<Pick ports={portsWith()} onSelect={(n) => seen.push(n)} />);
    const buttons = screen.getAllByRole("button");
    expect(buttons).toHaveLength(12);
    expect(buttons.every((b) => b.getAttribute("tabindex") === "0")).toBe(true);
    fireEvent.click(buttons[1]);
    fireEvent.keyDown(buttons[8], { key: "Enter" });
    fireEvent.keyDown(buttons[11], { key: " " });
    fireEvent.keyDown(buttons[3], { key: "a" });
    expect(seen).toEqual([2, 9, 12]);
    expect(buttons[11].getAttribute("aria-pressed")).toBe("true");
    expect(buttons[1].getAttribute("aria-label")).toBe("Port 2: connected, speed unknown, 3 heard through it. The nodes behind it link at 1 Gb/s");
  });

  it("writes no speed it doesn't know, and a mark for the neutral light", () => {
    const { container } = render(<Pick ports={portsWith()} onSelect={() => {}} />);
    const speeds = [...container.querySelectorAll(".sx-speed")].map((t) => t.textContent).filter(Boolean);
    expect(speeds).toEqual(["?"]);
    expect(container.querySelector('[data-port="2"]')?.classList.contains("sx-lunknown")).toBe(true);
    expect(container.querySelector('[data-port="3"]')?.classList.contains("sx-loff")).toBe(true);
  });

  it("explains every light in words", () => {
    render(<SwitchXLegend />);
    const list = screen.getByRole("list", { name: "What the lights mean" });
    expect(within(list).getAllByRole("listitem").map((i) => i.textContent)).toEqual(["10G", "5G", "2.5G", "1G", "100M", "Connected, speed unknown", "No link or empty"]);
  });
});

describe("a port's details", () => {
  it("shows the raw LLDP id, everything heard, and where each fact came from", () => {
    drawer(portsWith()[1]);
    expect(screen.getByText("Gi0/2")).toBeTruthy();
    expect(screen.getByText(/isn't confirmed/)).toBeTruthy();
    expect(screen.getByText("The nodes behind it link at 1 Gb/s")).toBeTruthy();
    const rows = within(screen.getByRole("table", { name: "Heard through this port" })).getAllByRole("row").slice(1).map((r) => r.textContent);
    expect(rows).toEqual(["cell1—cell1", "cell2—cell2", "CRS804ether2All"]);
    expect(screen.getAllByText(/^LLDP/).length).toBeGreaterThan(3);
    expect(screen.getByText(/never changes the switch/)).toBeTruthy();
  });

  it("says a label for anything but a node has no live data", () => {
    drawer(portsWith({ 4: { kind: "text", text: "Printer", setAtMs: NOW } })[3]);
    expect(screen.getByText("Printer")).toBeTruthy();
    expect(screen.getByText("SparkBox has no live data for this device.")).toBeTruthy();
    expect(screen.queryByText(/^Open /)).toBeNull();
  });

  it("gives a labeled node its own link figures and a way to its window", () => {
    const p = portsWith({ 3: { kind: "node", node: "cell2", setAtMs: NOW } })[2];
    drawer(p);
    expect(screen.getByText("Open cell2")).toBeTruthy();
    expect(screen.getByText("Up, 1 Gb/s")).toBeTruthy();
    expect(screen.getByText("1 in, 0 out")).toBeTruthy();
    // LLDP puts cell2 on port 2, so its NIC isn't this port's speed: the link is its report, the speed nobody's.
    expect(screen.getAllByText(/cell2's own report/).length).toBe(5);
    expect(screen.getByText("LLDP places cell2 on port 2, not this one, so its link speed isn't used here. That doesn't prove the label is wrong.")).toBeTruthy();
    expect(screen.getByText("Link flaps")).toBeTruthy();
    expect(screen.getByText("2")).toBeTruthy();
    expect(screen.queryByText("SparkBox has no live data for this device.")).toBeNull();
  });

  it("closes on Esc", () => {
    const close = vi.fn();
    drawer(portsWith()[1], close);
    fireEvent.keyDown(screen.getByRole("complementary", { name: "Port 2 details" }), { key: "Escape" });
    expect(close).toHaveBeenCalledOnce();
  });

  it("won't save a label in look-only mode", () => {
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: false, lookOnly: true });
    drawer(portsWith()[1]);
    fireEvent.change(screen.getByLabelText("What's plugged in"), { target: { value: "gold" } });
    expect((screen.getByRole("button", { name: "Save label" }) as HTMLButtonElement).disabled).toBe(true);
    _resetAdminForTests();
  });

  it("asks for the words of a free-text label", () => {
    _resetAdminForTests();
    drawer(portsWith()[1]);
    expect((screen.getByRole("button", { name: "Save label" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("What's plugged in"), { target: { value: "text" } });
    expect(screen.getByLabelText("What's plugged in, in your words")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Save label" }) as HTMLButtonElement).disabled).toBe(false);
  });
});
