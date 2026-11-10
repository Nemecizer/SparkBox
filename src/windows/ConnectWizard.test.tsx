// The wizard's words: what sudo is for, what the Done step can honestly say, and the unlock sheet
// for the node checks, which change nothing.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { _resetAdminForTests } from "../lib/admin";
import type { SwitchTestResult } from "../lib/netApi";
import type { ConfirmOptions } from "../lib/session";
import type { Inventory, Prefs } from "../lib/types";
import { mockTauri, type MockHandlers } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { ConnectWizard } from "./ConnectWizard";

const asked: ConfirmOptions[] = [];
vi.mock("../lib/session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/session")>()),
  confirmAction: vi.fn(async (o: ConfirmOptions) => (asked.push(o), true)),
}));

const inventory: Inventory = { nodes: [{ id: "cell2", name: "cell2", host: "cell2.local", ip: "192.168.50.154", model: "MSI EdgeXpert", port: 22, user: "sparkuser" }], switches: [] };
const pass = [{ id: "tcp", label: "Reach SSH", status: "ok", detail: "Answers", hint: null }];

async function show(dataSource: "demo" | "live", more: MockHandlers = {}) {
  mockTauri({
    get_inventory: () => inventory, credential_status: () => ({}), prefs_get: () => ({ dataSource }) as Prefs, get_snapshot: () => null, vpn_get: () => ({ status: { state: "off" } }),
    node_probe: () => pass, save_inventory: () => null, admin_authenticate: () => ({ unlocked: true, authenticated: true, expiresAtMs: Date.now() + 60_000, macExpiresAtMs: Date.now() + 60_000, idleMinutes: 15, requiresAuth: true }),
    admin_state: () => ({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true }),
    ...more,
  });
  await act(async () => {
    render(<WindowManagerProvider sizeOf={() => [800, 600]} isKnown={() => true}><ConnectWizard /></WindowManagerProvider>);
  });
}

async function runChecksThenDone() {
  await act(async () => {
    fireEvent.click(await screen.findByRole("button", { name: /Checks/ }));
  });
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Check all nodes" }));
  });
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: /Done/ }));
  });
}

beforeEach(() => {
  asked.length = 0;
  _resetAdminForTests();
});

describe("the wizard", () => {
  it("starts a new public installation with no guessed account or nodes", async () => {
    await show("demo", { get_inventory: () => ({ nodes: [], switches: [] }) });
    expect(screen.getByLabelText("User")).toHaveValue("");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: /Nodes/ })); });
    expect(screen.queryAllByLabelText("Address")).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Add node" })).toBeEnabled();
  });

  it("says when SparkBox runs sudo: a change, a protected read, or the Doctor", async () => {
    await show("demo");
    expect(await screen.findByText(/sudo always uses the password\. SparkBox runs sudo when you make a change, read a protected file with sudo, or run the Doctor\./)).toBeInTheDocument();
  });

  it("in Demo, Done says nothing real was tested", async () => {
    await show("demo");
    await runChecksThenDone();
    expect(screen.getByText("The simulated checks passed; nothing real was tested. Switch to Live to check your nodes. Demo stays one click away in Settings › Data source.")).toBeInTheDocument();
    expect(screen.queryByText(/Every node passed/)).toBeNull();
  });

  it("with Live data, Done keeps Every node passed", async () => {
    await show("live");
    await runChecksThenDone();
    expect(screen.getByText(/^Every node passed\./)).toBeInTheDocument();
  });

  it("the unlock sheet says the checks test settings, not that they change something", async () => {
    await show("live");
    await runChecksThenDone();
    expect(String(asked[0]?.body)).toContain("To test a node's connection settings, SparkBox needs Admin mode.");
  });

  describe("Trust… on the Switches step", () => {
    const failure: SwitchTestResult = { ok: false, board: null, version: null, fingerprint: "AB:CD:EF", errorKind: "tls-untrusted", message: "A certificate SparkBox hasn't trusted yet." };
    const test = async (result: SwitchTestResult) => {
      inventory.switches = [{ id: "crs804", name: "CRS804", kind: "routerOs", host: "192.168.50.2", user: "sparkbox" }];
      await show("live", { switch_test: () => result });
      await act(async () => {
        fireEvent.click(await screen.findByRole("button", { name: /Switches/ }));
      });
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Test" }));
      });
      inventory.switches = [];
    };

    it("is there for a certificate SparkBox doesn't know", async () => {
      await test(failure);
      expect(screen.getByRole("button", { name: "Trust…" })).toBeInTheDocument();
    });

    it("isn't there when the test failed past the certificate, even with a fingerprint", async () => {
      await test({ ...failure, errorKind: "auth", message: "CRS804 refused user \"sparkbox\" (401). Check the REST password." });
      expect(screen.getByText(/Check the REST password\./)).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Trust…" })).toBeNull();
    });
  });
});
