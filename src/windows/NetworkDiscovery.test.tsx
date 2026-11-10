import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { _resetDataSourceForTests, noteSource } from "../lib/dataSource";
import { handlers } from "../dev/mocks/discovery";
import type { Inventory } from "../lib/types";
import { mockTauri, type MockHandlers } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { NetworkDiscovery } from "./NetworkDiscovery";
const auth = vi.hoisted(() => ({ allowed: true, calls: [] as string[] }));
vi.mock("../lib/admin", async (original) => ({ ...await original<typeof import("../lib/admin")>(), withAdmin: async (fn: () => Promise<unknown>, opts: { scope: string; action: string }) => { auth.calls.push(`${opts.scope}:${opts.action}`); if (!auth.allowed) throw new Error("Authentication cancelled"); return fn(); } }));
const inv: Inventory = { nodes: [{ id: "gx10-0000", name: "cell4", host: "gx10-0000.local", ip: "192.168.50.181", user: "owner", port: 22, model: "ASUS" }], switches: [] };
const use = vi.fn();
function show(more: MockHandlers = {}) {
  mockTauri({ ...handlers, get_snapshot: () => null, ...more });
  return render(<WindowManagerProvider sizeOf={() => [900,700]} isKnown={() => true}><NetworkDiscovery inventory={inv} user="owner" onUse={use} /></WindowManagerProvider>);
}
async function click(name: string) { await act(async () => { fireEvent.click(await screen.findByRole("button", { name })); }); }
async function survey() { await click("Find Sparks on my network…"); await click("Survey this /24"); }
beforeEach(() => { _resetDataSourceForTests(); auth.allowed = true; auth.calls = []; use.mockReset(); });
it("does not scan on open; unidentified hosts cannot be selected; all network actions request Mac authentication", async () => {
  const start = vi.fn(handlers.discovery_start); show({ discovery_start: start });
  await click("Find Sparks on my network…"); expect(start).not.toHaveBeenCalled();
  await click("Survey this /24");
  expect(start).toHaveBeenCalledWith(expect.objectContaining({ expectedDemo: true }));
  expect(screen.getByRole("checkbox", { name: "Include 192.168.50.51" })).toBeDisabled();
  await click("Verify 192.168.50.70"); expect(screen.getByText(/generic SSH host/)).toBeInTheDocument();
  expect(screen.getByRole("checkbox", { name: "Include 192.168.50.70" })).toBeDisabled();
  expect(auth.calls).toEqual(["mac:discovery_start", "mac:discovery_verify"]);
});
it("reviews topology and unknown cabling, preserves cell4, and copies only to the draft after fresh validation", async () => {
  const save = vi.fn(), propose = vi.fn(handlers.discovery_propose); show({ save_inventory: save, discovery_propose: propose });
  await survey(); await click("Verify 192.168.50.181");
  fireEvent.click(screen.getByRole("checkbox", { name: "Include 192.168.50.181" }));
  await click("Preview topology"); expect(screen.getByText(/cell4: no LLDP neighbors/)).toBeInTheDocument();
  expect(screen.getByText("192.168.100.0/24")).toBeInTheDocument(); expect(use).not.toHaveBeenCalled();
  await click("Use proposal in draft");
  expect(propose).toHaveBeenCalledTimes(2); expect(save).not.toHaveBeenCalled();
  expect(use.mock.calls[0][0].nodes[0]).toMatchObject({ id: "gx10-0000", name: "cell4" });
});
it("cancelled Mac authentication never begins the survey", async () => {
  auth.allowed = false; const start = vi.fn(); show({ discovery_start: start }); await survey(); expect(start).not.toHaveBeenCalled();
});
it("discards a late start when the source changes and cancels the returned run", async () => {
  let resolve!: (r: unknown) => void;
  const cancel = vi.fn(); show({ discovery_start: () => new Promise((r) => { resolve = r; }), discovery_cancel: cancel });
  await survey(); await act(async () => noteSource("live"));
  await act(async () => resolve(await handlers.discovery_start({})));
  await waitFor(() => expect(cancel).toHaveBeenCalled());
  expect(screen.queryByText(/253\/253/)).toBeNull(); expect(use).not.toHaveBeenCalled();
});
it("closing a pending verification discards the reply and stops discovery", async () => {
  let resolve!: (r: unknown) => void; const cancel = vi.fn(handlers.discovery_cancel);
  show({ discovery_verify: () => new Promise((r) => { resolve = r; }), discovery_cancel: cancel });
  await survey(); await click("Verify 192.168.50.51"); await click("Close discovery");
  await act(async () => resolve({})); expect(cancel).toHaveBeenCalled(); expect(use).not.toHaveBeenCalled();
  expect(screen.queryByText("Proposed topology")).toBeNull();
});
it("a changed inventory draft invalidates a preview even when its reply arrives late", async () => {
  let resolve!: (r: unknown) => void; let result: unknown;
  const r = show({ discovery_propose: (args) => { result = handlers.discovery_propose(args); return new Promise((done) => { resolve = done; }); } });
  await survey(); await click("Verify 192.168.50.181"); fireEvent.click(screen.getByRole("checkbox", { name: "Include 192.168.50.181" }));
  await click("Preview topology");
  r.rerender(<WindowManagerProvider sizeOf={() => [900,700]} isKnown={() => true}><NetworkDiscovery inventory={{ ...inv, nodes: [] }} user="owner" onUse={use} /></WindowManagerProvider>);
  await act(async () => resolve(result)); expect(screen.queryByText("Proposed topology")).toBeNull(); expect(use).not.toHaveBeenCalled();
});

it("shows a structured verification refusal as readable text", async () => {
  show({ discovery_verify: () => { throw { kind: "failed", message: "Selected interface changed. Survey again." }; } });
  await survey(); await click("Verify 192.168.50.51");
  expect(screen.getByText("Selected interface changed. Survey again.")).toBeInTheDocument();
});
