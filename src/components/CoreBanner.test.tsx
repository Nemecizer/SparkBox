import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mockCalls, mockTauri } from "../test/tauri";
import { SettingsFileBanner } from "./CoreBanner";

describe("unreadable settings files", () => {
  it("names the file, shows where the old copy went and can be dismissed", async () => {
    mockTauri({
      persistence_warnings: () => [
        { file: "inventory.json", path: "/data/inventory.json", backup: "/data/inventory.json.corrupt-1730000000", error: "expected value at line 1 column 1", tsMs: 1 },
      ],
    });
    render(<SettingsFileBanner />);
    expect(await screen.findByText(/Couldn't read inventory\.json\. SparkBox started with defaults/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show in Finder" }));
    await waitFor(() => expect(mockCalls.find((c) => c.cmd === "plugin:opener|reveal_item_in_dir")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("says nothing when every file was read", async () => {
    mockTauri({ persistence_warnings: () => [] });
    render(<SettingsFileBanner />);
    await waitFor(() => expect(mockCalls.some((c) => c.cmd === "persistence_warnings")).toBe(true));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("shows later history failures without claiming defaults, and keeps dismissal across polls", async () => {
    vi.useFakeTimers();
    try {
      let error = "";
      mockTauri({ persistence_warnings: () => error ? [{ file: "deployments.json", path: "/data/deployments.json",
        backup: "/data/deployment-history", error, tsMs: Date.now() }] : [] });
      const view = render(<SettingsFileBanner />);
      await act(async () => {});
      expect(screen.queryByRole("status")).toBeNull();
      error = "Another writer changed the saved list";
      await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
      expect(screen.getByRole("status")).toHaveTextContent("Existing files were kept");
      expect(screen.getByRole("status")).not.toHaveTextContent("started with defaults");
      fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
      await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
      expect(screen.queryByRole("status")).toBeNull();
      error = "A recovery copy cannot be saved";
      await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
      expect(screen.getByRole("status")).toHaveTextContent("New loads and Forget");
      view.unmount();
    } finally { vi.useRealTimers(); }
  });
});
