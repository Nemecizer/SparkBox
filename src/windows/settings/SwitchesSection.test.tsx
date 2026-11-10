// Settings › Switches & gateway: a certificate the test doesn't know can be trusted from here, as
// the wizard's Switches step does, and the footnotes say what SparkBox really does.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { _resetAdminForTests, LOOK_ONLY_TIP } from "../../lib/admin";
import type { AdminState } from "../../lib/opsApi";
import type { SwitchTestResult } from "../../lib/netApi";
import type { ConfirmOptions } from "../../lib/session";
import type { Inventory } from "../../lib/types";
import { mockCalls, mockTauri, type MockHandlers } from "../../test/tauri";
import { SwitchesSection } from "./SwitchesSection";

const asked: ConfirmOptions[] = [];
let answer = true;
vi.mock("../../lib/session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/session")>()),
  confirmAction: vi.fn(async (o: ConfirmOptions) => (asked.push(o), answer)),
}));

const inventory: Inventory = {
  nodes: [],
  switches: [
    { id: "crs804", name: "CRS804", kind: "routerOs", host: "192.168.50.2", user: "sparkbox" },
    { id: "switchx", name: "Switch X", kind: "firewalla", host: "192.168.50.3", user: null },
    { id: "gold", name: "Firewalla Gold", kind: "gateway", host: "192.168.50.1", user: null },
  ],
};
const FP = "AB:CD:EF";
const unknownCert: SwitchTestResult = { ok: false, board: null, version: null, fingerprint: FP, errorKind: "tls-untrusted", message: "This certificate isn't trusted yet: compare it, then choose Trust." };
const trusted: SwitchTestResult = { ok: true, board: "CRS804-4DDQ", version: "7.20", fingerprint: null, errorKind: null, message: "Connected" };
const LOCKED: AdminState = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true };
const MAC: AdminState = { unlocked: true, authenticated: true, expiresAtMs: Date.now() + 600_000, macExpiresAtMs: Date.now() + 300_000, idleMinutes: 15, requiresAuth: true };

async function show(admin: AdminState, more: MockHandlers = {}) {
  mockTauri({
    get_inventory: () => inventory, credential_status: () => ({}), firewalla_msp_settings: () => ({ domain: null }), switch_test: () => unknownCert, switch_trust: () => trusted,
    prefs_get: () => ({ dataSource: "live", hardwareAutoLookup: false }), hardware_profiles: () => ({ enabled: false, demo: false, devices: [] }),
    admin_state: () => admin, admin_authenticate: () => MAC, ...more,
  });
  _resetAdminForTests(admin);
  await act(async () => {
    render(<SwitchesSection />);
  });
}

async function test() {
  await act(async () => {
    fireEvent.click(screen.getAllByRole("button", { name: "Test connection" })[0]);
  });
}

beforeEach(() => {
  asked.length = 0;
  answer = true;
});

describe("Trust… in Settings", () => {
  it("shows when the test hands back a fingerprint, pins it through Mac scope and shows the new result", async () => {
    await show(LOCKED);
    await test();
    expect(screen.getByRole("status")).toHaveTextContent("then choose Trust.");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Trust… — needs Admin" }));
    });
    expect(asked.map((a) => a.title)).toEqual(["Unlock Admin mode?"]);
    expect(String(asked[0].body)).toContain("To trust this switch's certificate, SparkBox needs Admin mode.");
    expect(mockCalls.find((c) => c.cmd === "admin_authenticate")?.args.action).toBe("switch_trust");
    expect(mockCalls.find((c) => c.cmd === "switch_trust")?.args).toEqual({ switchId: "crs804", fingerprint: FP });
    expect(await screen.findByText(/CRS804-4DDQ · RouterOS 7\.20/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Trust…/ }), "nothing left to trust").toBeNull();
  });

  it("cancelling the sheet pins nothing", async () => {
    answer = false;
    await show(LOCKED);
    await test();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Trust… — needs Admin" }));
    });
    expect(mockCalls.some((c) => c.cmd === "switch_trust")).toBe(false);
    expect(screen.getByRole("button", { name: "Trust… — needs Admin" })).toBeInTheDocument();
  });

  it("unlocked for the Mac, it asks nothing more", async () => {
    await show(MAC);
    await test();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Trust…" }));
    });
    expect(asked).toEqual([]);
    expect(mockCalls.some((c) => c.cmd === "admin_authenticate")).toBe(false);
    expect(mockCalls.some((c) => c.cmd === "switch_trust")).toBe(true);
  });

  it("a failed test with no fingerprint offers nothing to trust", async () => {
    await show(LOCKED, { switch_test: () => ({ ...unknownCert, fingerprint: null, message: "No answer" }) });
    await test();
    expect(screen.queryByRole("button", { name: /^Trust…/ })).toBeNull();
  });

  it("a failure past the certificate offers nothing to trust, even with a fingerprint", async () => {
    const wrongPassword: SwitchTestResult = { ...unknownCert, errorKind: "auth", message: "CRS804 refused user \"sparkbox\" (401). Check the REST password." };
    await show(LOCKED, { switch_test: () => wrongPassword });
    await test();
    expect(screen.getByRole("status")).toHaveTextContent("Check the REST password.");
    expect(screen.queryByRole("button", { name: /^Trust…/ })).toBeNull();
  });

  it("a changed certificate can be trusted too", async () => {
    await show(LOCKED, { switch_test: () => ({ ...unknownCert, errorKind: "tls-changed", message: "The certificate changed." }) });
    await test();
    expect(screen.getByRole("button", { name: "Trust… — needs Admin" })).toBeInTheDocument();
  });

  it("in look-only it's off, and says why", async () => {
    await show({ ...LOCKED, lookOnly: true });
    await test();
    const b = screen.getByRole("button", { name: "Trust… — look-only" });
    expect(b).toBeDisabled();
    expect(b).toHaveAttribute("title", LOOK_ONLY_TIP);
  });
});

describe("the footnotes", () => {
  it("say what SparkBox checks and what the RouterOS user needs", async () => {
    await show(LOCKED);
    expect(screen.getByText(/create a user for SparkBox: the read and rest-api policies are enough to watch it, and port actions also need write and test\. RouterOS 7 serves REST at/)).toBeInTheDocument();
    expect(screen.getByText(/SparkBox checks that the switch answers on TCP 443, then 80, and reads its ports from LLDP/)).toBeInTheDocument();
    expect(screen.getByText(/The router and VPN endpoint\. SparkBox checks that it answers on TCP 443, then 80, and times the reply\./)).toBeInTheDocument();
    expect(screen.queryByText(/pings|read-only user/)).toBeNull();
  });
});


describe("automatic equipment lookup consent", () => {
  it("canceling Mac authentication sends no preference change", async () => {
    await show(LOCKED, { admin_authenticate: () => { throw { kind: "cancelled", message: "Owner canceled Touch ID" }; } });
    await act(async () => { fireEvent.click(screen.getByRole("switch", { name: "Automatic AI hardware lookup" })); });
    expect(mockCalls.find((c) => c.cmd === "admin_authenticate")?.args.action).toBe("prefs_set");
    expect(mockCalls.some((c) => c.cmd === "prefs_patch")).toBe(false);
    expect(screen.getByRole("switch", { name: "Automatic AI hardware lookup" })).toHaveAttribute("aria-checked", "false");
  });
  it("saves only the opt-in preference through Mac scope, never a lookup request", async () => {
    await show(MAC, { prefs_patch: ({ patch }) => ({ dataSource: "live", ...patch }) });
    await act(async () => { fireEvent.click(screen.getByRole("switch", { name: "Automatic AI hardware lookup" })); });
    expect(mockCalls.find((c) => c.cmd === "prefs_patch")?.args).toEqual({ patch: { hardwareAutoLookup: true } });
    expect(screen.getByRole("switch", { name: "Automatic AI hardware lookup" })).toHaveAttribute("aria-checked", "true");
    expect(mockCalls.some((c) => /chat_send|model_analyze/.test(c.cmd))).toBe(false);
  });
});
