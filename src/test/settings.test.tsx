import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { inventoryFixture, prefsFixture } from "../dev/mockBackend";
import { _resetAdminForTests } from "../lib/admin";
import type { Inventory } from "../lib/types";
import { Settings } from "../windows/Settings";
import { mockCalls, mockTauri } from "./tauri";

function backend() {
  let inventory: Inventory = structuredClone(inventoryFixture);
  // Each test starts locked: an unlock from the one before doesn't carry over.
  _resetAdminForTests();
  mockTauri({
    get_inventory: () => structuredClone(inventory),
    save_inventory: ({ inventory: next }: { inventory: Inventory }) => {
      inventory = next;
      return null;
    },
    prefs_get: () => structuredClone(prefsFixture),
    credential_status: ({ accounts }: { accounts: string[] }) => Object.fromEntries(accounts.map((a) => [a, true])),
    demo_scenarios_list: () => [],
    "plugin:autostart|is_enabled": () => false,
    autostart_get: () => false,
    autostart_set: ({ enabled }: { enabled: boolean }) => enabled,
    admin_state: () => ({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true }),
    admin_authenticate: () => ({ unlocked: true, authenticated: true, expiresAtMs: Date.now() + 600_000, idleMinutes: 15, requiresAuth: true }),
    data_source_set: ({ source }: { source: "demo" | "live" }) => ({ ...structuredClone(prefsFixture), dataSource: source }),
  });
  return () => inventory;
}

describe("Settings window", () => {
  it("saves a role preference through Mac authentication and reverts draft expectations", async () => {
    const saved = backend();
    const user = userEvent.setup();
    render(<Settings params={{ section: "nodes" }} />);
    const role = await screen.findByLabelText("Load role");
    await user.selectOptions(role, "worker");
    await user.click(screen.getByRole("button", { name: "Revert" }));
    expect(role).toHaveValue("none");
    expect(mockCalls.some((c) => c.cmd === "save_inventory")).toBe(false);
    await user.selectOptions(role, "head");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saved().nodes[0].fabric?.headCandidate).toBe(true));
    const auth = mockCalls.findIndex((c) => c.cmd === "admin_authenticate");
    expect(auth).toBeGreaterThanOrEqual(0);
    expect(mockCalls[auth].args).toMatchObject({ action: "save_inventory" });
    expect(auth).toBeLessThan(mockCalls.findIndex((c) => c.cmd === "save_inventory"));
    await user.click(screen.getByRole("button", { name: "Clear expectations" }));
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saved().nodes[0].fabric).toBeNull());
  });
  it("lists sections in order, opens a deep link and remembers the last section", async () => {
    backend();
    const user = userEvent.setup();
    const { unmount } = render(<Settings params={{ section: "appearance" }} />);
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    const labels = within(nav).getAllByRole("button").map((b) => b.textContent);
    expect(labels.slice(0, 3)).toEqual(["General", "Appearance", "Cluster login"]);
    expect(labels.at(-1)).toBe("Diagnostics");
    expect(within(nav).getByRole("button", { name: "Appearance" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { level: 2, name: "Appearance" })).toBeInTheDocument();
    await user.click(within(nav).getByRole("button", { name: "Diagnostics" }));
    unmount();
    render(<Settings params={{}} />);
    expect(screen.getByRole("heading", { level: 2, name: "Diagnostics" })).toBeInTheDocument();
  });

  it("searches labels and keywords", async () => {
    backend();
    const user = userEvent.setup();
    render(<Settings params={{}} />);
    await user.type(screen.getByRole("searchbox", { name: "Search settings" }), "password");
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    expect(within(nav).getAllByRole("button").map((b) => b.textContent)).toEqual(["Cluster login"]);
    await user.keyboard("{Enter}");
    expect(screen.getByRole("heading", { level: 2, name: "Cluster login" })).toBeInTheDocument();
    expect(screen.getByLabelText("User")).toHaveValue("sparkuser");
    expect(screen.getByLabelText("Password")).toHaveAttribute("type", "password");
  });

  it("validates node addresses and saves through one bar that appears only with changes", async () => {
    const saved = backend();
    const user = userEvent.setup();
    render(<Settings params={{ section: "nodes" }} />);
    const ip = await screen.findByRole("textbox", { name: "cell2 IP address" });
    expect(screen.queryByRole("region", { name: "Unsaved changes" })).toBeNull();
    await user.clear(ip);
    await user.type(ip, "192.168.50.300");
    expect(ip).toHaveAttribute("aria-invalid", "true");
    const bar = screen.getByRole("region", { name: "Unsaved changes" });
    expect(within(bar).getByRole("button", { name: "Save" })).toBeDisabled();
    expect(bar).toHaveTextContent("1 field needs fixing");
    await user.clear(ip);
    await user.type(ip, "192.168.50.154");
    await user.clear(screen.getByRole("textbox", { name: "cell2 SSH port" }));
    await user.type(screen.getByRole("textbox", { name: "cell2 SSH port" }), "2222");
    await user.click(screen.getByRole("button", { name: "Move cell2 up" }));
    await user.click(within(screen.getByRole("region", { name: "Unsaved changes" })).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saved().nodes.map((n) => n.id)).toEqual(["cell2", "cell1", "cell3", "gx10-0000"]));
    expect(saved().nodes[0].port).toBe(2222);
    // The inventory is Mac-side state: Touch ID or the password first.
    const auth = mockCalls.findIndex((c) => c.cmd === "admin_authenticate");
    expect(auth).toBeGreaterThanOrEqual(0);
    expect(mockCalls[auth].args).toMatchObject({ action: "save_inventory" });
    expect(auth).toBeLessThan(mockCalls.findIndex((c) => c.cmd === "save_inventory"));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Unsaved changes" })).toBeNull());
    // Put the shared draft back for the other tests.
    await user.click(screen.getByRole("button", { name: "Move cell1 up" }));
    await act(async () => {
      within(screen.getByRole("region", { name: "Unsaved changes" })).getByRole("button", { name: "Revert" }).click();
    });
  });

  it("asks for Touch ID or the password before switching the data source (Mac scope)", async () => {
    backend();
    const user = userEvent.setup();
    render(<Settings params={{ section: "data" }} />);
    const live = await screen.findByRole("radio", { name: /Live/ });
    await user.click(live);
    await waitFor(() => expect(mockCalls.some((c) => c.cmd === "data_source_set")).toBe(true));
    const auth = mockCalls.findIndex((c) => c.cmd === "admin_authenticate");
    const set = mockCalls.findIndex((c) => c.cmd === "data_source_set");
    expect(auth).toBeGreaterThanOrEqual(0);
    expect(mockCalls[auth].args).toMatchObject({ action: "data_source_set" });
    expect(auth).toBeLessThan(set);
    expect(mockCalls[set].args).toEqual({ source: "live" });
    await waitFor(() => expect(screen.getByRole("radio", { name: /Live/ })).toHaveAttribute("aria-checked", "true"));
    // Put Demo back for the other tests.
    await user.click(screen.getByRole("radio", { name: /Demo/ }));
  });

  it("turns Start at login on through the Mac gate and autostart_set", async () => {
    backend();
    const user = userEvent.setup();
    render(<Settings params={{ section: "general" }} />);
    const sw = await screen.findByRole("switch", { name: "Start at login" });
    await waitFor(() => expect(sw).toBeEnabled());
    await user.click(sw);
    await waitFor(() => expect(mockCalls.some((c) => c.cmd === "autostart_set")).toBe(true));
    expect(mockCalls.find((c) => c.cmd === "autostart_set")!.args).toEqual({ enabled: true });
    expect(mockCalls.some((c) => c.cmd.startsWith("plugin:autostart|enable"))).toBe(false);
    await waitFor(() => expect(sw).toHaveAttribute("aria-checked", "true"));
  });

  it("offers GB10 models in a list and a labelled Test per node", async () => {
    backend();
    render(<Settings params={{ section: "nodes" }} />);
    const model = await screen.findByRole("combobox", { name: "cell1 model" });
    const options = within(model).getAllByRole("option").map((o) => o.textContent);
    expect(options).toEqual(expect.arrayContaining(["MSI EdgeXpert", "ASUS Ascent GX10", "NVIDIA DGX Spark", "Dell Pro Max GB10", "Lenovo ThinkStation PGX", "HP ZGX Nano", "Gigabyte AI TOP ATOM", "Other…"]));
    expect(screen.getByRole("button", { name: "Test cell1" })).toHaveTextContent("Test");
  });
});

