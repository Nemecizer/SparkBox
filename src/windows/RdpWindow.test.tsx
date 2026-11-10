import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { mockCalls, mockTauri } from "../test/tauri";
import { RdpWindow, rdpStatus, type RdpView } from "./RdpWindow";
const state = vi.hoisted(() => ({ demo: true, admin: true, confirm: true, asked: null as unknown }));
vi.mock("../lib/store", () => ({ useCluster: () => ({ demo: state.demo, nodes: [{ id: "gx10-0000", name: "cell4" }] }) }));
vi.mock("../lib/admin", () => ({ requestAdmin: async () => state.admin }));
vi.mock("../lib/session", async (o) => ({ ...await o<object>(), confirmAction: async (a: unknown) => { state.asked = a; return state.confirm; } }));
vi.mock("../components/ui", async (o) => ({ ...await o<object>(), HelpTip: () => null }));
const fixture = (): RdpView => ({ node: "gx10-0000", address: "192.0.2.4:3390", demo: state.demo,
  mac: { label: "RDP from this Mac", ok: true, detail: "TLS passed; desktop login not tested" },
  report: { port: 3390, profile: "gnome", user: "owner", fingerprint: "ab".repeat(32), digest: "d".repeat(64), backup: null,
    checks: [{ label: "Software display", ok: false, detail: "Configure Xorg" }], changes: ["Configure Xorg", "Restart xrdp"], blockers: [] } });
beforeEach(() => { state.demo = true; state.admin = true; state.confirm = true; state.asked = null; mockTauri({ rdp_check: fixture, rdp_repair: fixture }); });
const click = async (label: string) => act(async () => { fireEvent.click(screen.getByRole("button", { name: label })); });
it("checks without repairing and distinguishes a TLS check from desktop login", async () => {
  render(<RdpWindow />); await click("Check all nodes");
  expect(mockCalls.filter((c) => c.cmd === "rdp_check")).toHaveLength(1);
  expect(mockCalls.some((c) => c.cmd === "rdp_repair")).toBe(false);
  expect(screen.getByText(/Desktop login is not tested\./)).toBeInTheDocument();
  expect(screen.getByText("192.0.2.4:3390")).toBeInTheDocument();
  expect(rdpStatus(fixture())).toBe("Needs attention");
  const v = fixture(); v.report.checks = []; v.mac.ok = null;
  expect(rdpStatus(v)).toBe("Check incomplete");
});
it("requires Admin and a typed display-name confirmation but sends the stable node id", async () => {
  render(<RdpWindow />); state.admin = false; await click("Check all nodes");
  expect(mockCalls.some((c) => c.cmd === "rdp_check")).toBe(false);
  state.admin = true; await click("Check cell4"); state.confirm = false; await click("Review repair…");
  expect(mockCalls.some((c) => c.cmd === "rdp_repair")).toBe(false);
  expect(state.asked).toMatchObject({ requireTyping: "cell4", danger: true });
  state.confirm = true; await click("Review repair…");
  expect(mockCalls.find((c) => c.cmd === "rdp_repair")?.args).toEqual({ node: "gx10-0000", profile: "keep", digest: "d".repeat(64) });
});
it("blocks active sessions and preserves the reason", async () => {
  mockTauri({ rdp_check: () => { const v = fixture(); v.report.blockers = ["Log out of every RDP desktop before repair"]; return v; } });
  render(<RdpWindow />); await click("Check all nodes");
  expect(screen.getByRole("button", { name: "Review repair…" })).toBeDisabled();
  expect(screen.getByText("Log out of every RDP desktop before repair")).toBeInTheDocument();
});
it("drops results from the old data source and invalidates reviews when the desktop choice changes", async () => {
  let done!: (v: RdpView) => void;
  mockTauri({ rdp_check: () => new Promise((resolve) => { done = resolve; }) });
  const view = render(<RdpWindow />); await click("Check all nodes");
  const old = fixture(); state.demo = false; view.rerender(<RdpWindow />);
  await act(async () => done(old));
  expect(screen.queryByRole("button", { name: "Review repair…" })).not.toBeInTheDocument();
  mockTauri({ rdp_check: fixture }); await click("Check all nodes");
  fireEvent.change(screen.getByLabelText("Desktop for repair"), { target: { value: "xfce" } });
  expect(screen.queryByRole("button", { name: "Review repair…" })).not.toBeInTheDocument();
});
it("failed repair removes the previous result and keeps rollback instructions visible", async () => {
  mockTauri({ rdp_check: fixture, rdp_repair: () => { throw new Error("Verification failed; previous configuration restored. Backup: /var/backups/fixture"); } });
  render(<RdpWindow />); await click("Check all nodes"); await click("Review repair…");
  expect(screen.getByText(/Verification failed; previous configuration restored/)).toBeInTheDocument();
  expect(screen.queryByText("192.0.2.4:3390")).not.toBeInTheDocument();
});
