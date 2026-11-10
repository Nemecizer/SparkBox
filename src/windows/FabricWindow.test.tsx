// The Fabric matrix: with no switch port mapped, a cell says what LLDP heard on that half, as the
// node's RoCE panel does, before it gives up with "Switch port unknown".
import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import type { FabricState, IfaceHealth } from "../lib/fabric";
import { resetStore, startClusterStore } from "../lib/store";
import type { ClusterSnapshot } from "../lib/types";
import { mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { FabricWindow } from "./FabricWindow";

const IFACE = "enp1s0f1np1";
const iface = (node: string, extra: Partial<IfaceHealth> = {}): IfaceHealth => ({
  node, nodeName: node, iface: IFACE, half: 0, up: true, speedMbps: 200_000, mtu: 9000, rxBps: 0, txBps: 0, rdmaShare: null, switchPort: null, mappedBy: null, cablingOk: null, expectedPort: null,
  verdict: { level: "ok", title: "Lossless", detail: "", action: null }, counters: [], lossLastHour: 0, ...extra,
}) as unknown as IfaceHealth;
let fabric: FabricState;

vi.mock("../lib/fabric", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/fabric")>()),
  useFabricHealth: () => fabric,
}));

async function show(lldp: unknown) {
  resetStore();
  const snap = structuredClone(baseSnapshot) as ClusterSnapshot;
  snap.tsMs = Date.now();
  const n = snap.nodes[0];
  n.nets = [{ iface: IFACE, role: "fabric", up: true, speedMbps: 200_000, rxBps: 0, txBps: 0, ip: "192.168.100.51", ...(lldp as object) }] as ClusterSnapshot["nodes"][0]["nets"];
  fabric = { ifaces: [iface(n.id)], verdict: { level: "ok", title: "ok", detail: "" }, summary: "ok", transport: [], cabling: [] } as unknown as FabricState;
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => snap }));
  await startClusterStore();
  await act(async () => {
    render(<WindowManagerProvider sizeOf={() => [1000, 700]} isKnown={() => true}><FabricWindow params={{}} /></WindowManagerProvider>);
  });
}

describe("a Fabric cell with no switch port mapped", () => {
  it("uses the LLDP line", async () => {
    await show({ lldp: { system: "crs804", port: "qsfp56-dd-2-1", portId: null } });
    expect(screen.getByRole("gridcell", { name: /Lossless/ })).toHaveTextContent(/Switch port qsfp56-dd-2-1 \(LLDP\)/);
    expect(screen.queryByText("Switch port unknown")).toBeNull();
  });

  it("says Switch port unknown only when LLDP heard nothing", async () => {
    await show({});
    expect(screen.getByRole("gridcell", { name: /Lossless/ })).toHaveTextContent("Switch port unknown");
  });
});
