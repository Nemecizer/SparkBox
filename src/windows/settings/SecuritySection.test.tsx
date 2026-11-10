import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ADMIN_REFUSED, _resetAdminForTests, withAdmin } from "../../lib/admin";
import type { SecuritySettings } from "../../lib/opsApi";
import { mockTauri } from "../../test/tauri";
import { SecuritySection } from "./SecuritySection";

vi.mock("../../lib/admin", async (original) => ({
  ...await original<typeof import("../../lib/admin")>(),
  withAdmin: vi.fn(async (fn: () => Promise<unknown>) => fn()),
}));

const defaults: SecuritySettings = { idleMinutes: 15, retentionDays: 365, shellIdleMinutes: 240 };
const choices = [[5, "5 minutes"], [15, "15 minutes"], [60, "1 hour"], [120, "2 hours"], [240, "4 hours"], [480, "8 hours"], [1440, "24 hours"]] as const;
let saved: SecuritySettings;
const set = vi.fn(({ settings }: { settings: SecuritySettings }) => (saved = settings));

beforeEach(() => {
  saved = { ...defaults };
  set.mockClear();
  vi.mocked(withAdmin).mockClear();
  _resetAdminForTests();
  mockTauri({ security_get: () => saved, security_set: set, tunnels_list: () => [], drop_cache_grant_get: () => null });
});
afterEach(() => _resetAdminForTests());

async function editor() {
  const view = render(<SecuritySection />);
  const select = screen.getByRole("combobox", { name: "Lock after" });
  await waitFor(() => expect(select).toBeEnabled());
  return { ...view, select };
}

describe("Admin idle duration", () => {
  it.each(choices)("saves %i minutes through Mac authentication and reads it back on reopening", async (minutes) => {
    const { select, unmount } = await editor();
    expect(within(select).getAllByRole("option").map((option) => option.textContent)).toEqual(choices.map(([, label]) => label));
    expect(select).toHaveValue("15");
    fireEvent.change(select, { target: { value: String(minutes) } });
    await waitFor(() => expect(set).toHaveBeenCalledWith({ settings: { ...defaults, idleMinutes: minutes } }));
    expect(withAdmin).toHaveBeenCalledWith(expect.any(Function), { scope: "mac", action: "security_set" });
    unmount();
    const reopened = await editor();
    expect(reopened.select).toHaveValue(String(minutes));
  });

  it.each([ADMIN_REFUSED, new Error("Could not save security settings")])("restores the saved choice after a refused or failed save", async (error) => {
    vi.mocked(withAdmin).mockRejectedValueOnce(error);
    const { select } = await editor();
    fireEvent.change(select, { target: { value: "1440" } });
    await waitFor(() => expect(select).toHaveValue("15"));
    expect(set).not.toHaveBeenCalled();
    expect(select).toBeEnabled();
    if (error instanceof Error) expect(screen.getByText(error.message)).toBeInTheDocument();
  });

  it("waits for authentication before accepting another choice", async () => {
    let finish!: (value: SecuritySettings) => void;
    vi.mocked(withAdmin).mockImplementationOnce(() => new Promise<SecuritySettings>((resolve) => { finish = resolve; }) as never);
    const { select } = await editor();
    fireEvent.change(select, { target: { value: "480" } });
    expect(select).toBeDisabled();
    await act(async () => finish({ ...defaults, idleMinutes: 480 }));
    expect(select).toBeEnabled();
    expect(select).toHaveValue("480");
  });

  it("disables the duration in look-only mode", async () => {
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: true });
    await act(async () => { render(<SecuritySection />); });
    expect(screen.getByRole("combobox", { name: "Lock after" })).toBeDisabled();
    expect(set).not.toHaveBeenCalled();
  });

  it("starts the countdown from the time of a new unlock", async () => {
    let now = 1_000_000;
    const clock = vi.spyOn(Date, "now").mockImplementation(() => now);
    try {
      await editor();
      now += 10_000;
      await act(async () => _resetAdminForTests({ unlocked: true, expiresAtMs: now + 86_400_000, idleMinutes: 1440, requiresAuth: true }));
      expect(screen.getByText("Locks in 24:00:00 unless you make a change")).toBeInTheDocument();
    } finally {
      clock.mockRestore();
    }
  });
});
