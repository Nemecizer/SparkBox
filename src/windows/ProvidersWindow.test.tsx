// Scan cluster saves what it finds, so it follows the lock rule: off in look-only, and locked it
// asks to unlock, then scans.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests, adminState, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP } from "../lib/admin";
import type { AdminState } from "../lib/opsApi";
import * as session from "../lib/session";
import { mockCalls, mockTauri } from "../test/tauri";
import { ProvidersWindow } from "./ProvidersWindow";

const LOCKED: AdminState = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };

async function show(providersScan: () => unknown = () => []) {
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, {
    admin_state: () => adminState(),
    providers_list: () => [],
    providers_scan: providersScan,
    admin_unlock: () => ({ ...LOCKED, unlocked: true }),
  }));
  await act(async () => {
    render(<ProvidersWindow />);
  });
  return screen.getByRole("button", { name: /^Scan cluster/ });
}

afterEach(() => {
  _resetAdminForTests();
  vi.restoreAllMocks();
});

describe("Scan cluster", () => {
  it("is off in look-only, with look-only's words", async () => {
    _resetAdminForTests({ ...LOCKED, lookOnly: true });
    const scan = await show();
    expect(scan).toBeDisabled();
    expect(scan).toHaveAttribute("title", LOOK_ONLY_TIP);
    fireEvent.click(scan);
    expect(mockCalls.some((c) => c.cmd === "providers_scan")).toBe(false);
  });

  it("asks to unlock when locked, under its own action, then scans", async () => {
    _resetAdminForTests(LOCKED);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const scan = await show();
    expect(scan).toBeEnabled();
    expect(scan.getAttribute("title")).toContain(NEEDS_ADMIN_TIP);
    expect(scan.querySelector(".tool-lock")).not.toBeNull();
    await act(async () => {
      fireEvent.click(scan);
      await new Promise((res) => setTimeout(res, 20));
    });
    expect(asked).toHaveBeenCalledOnce();
    expect(mockCalls.find((c) => c.cmd === "admin_unlock")?.args).toMatchObject({ reason: "providers_scan" });
    expect(mockCalls.some((c) => c.cmd === "providers_scan")).toBe(true);
  });

  it("stays locked, and scans nothing, when the unlock is turned down", async () => {
    _resetAdminForTests(LOCKED);
    vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const scan = await show();
    await act(async () => {
      fireEvent.click(scan);
      await new Promise((res) => setTimeout(res, 20));
    });
    expect(mockCalls.some((c) => c.cmd === "providers_scan")).toBe(false);
  });

  it("tries again when Admin relocked behind the button", async () => {
    _resetAdminForTests({ ...LOCKED, unlocked: true });
    let refused = false;
    const scan = await show(() => {
      if (refused) return [];
      refused = true;
      throw { kind: "locked", message: "Admin mode is locked." };
    });
    await act(async () => {
      fireEvent.click(scan);
      await new Promise((res) => setTimeout(res, 20));
    });
    expect(mockCalls.filter((c) => c.cmd === "providers_scan")).toHaveLength(2);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("scans at once when Admin is unlocked", async () => {
    _resetAdminForTests({ ...LOCKED, unlocked: true });
    const scan = await show();
    expect(scan).not.toHaveAttribute("data-lock");
    await act(async () => {
      fireEvent.click(scan);
      await new Promise((res) => setTimeout(res, 20));
    });
    expect(mockCalls.some((c) => c.cmd === "providers_scan")).toBe(true);
  });
});
