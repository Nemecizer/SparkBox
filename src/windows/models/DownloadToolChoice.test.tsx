import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { demoDownloadTools } from "../../dev/mocks/jobs";
import { api } from "../../lib/api";
import * as session from "../../lib/session";
import { DownloadToolChoice, chooseDownloadTool, downloadToolConsent } from "./DownloadToolChoice";

afterEach(() => vi.restoreAllMocks());

it("shows the exact chosen path or image ID and distinguishes installation", () => {
  const inventory = demoDownloadTools("cell1"); const choice = vi.fn();
  render(<DownloadToolChoice inventory={inventory} onChoice={choice} />);
  expect((screen.getByLabelText("Selected tool identity") as HTMLInputElement).value).toBe(inventory.options[0].path);
  fireEvent.change(screen.getByLabelText("Download tool"), { target: { value: "1" } });
  expect(choice).toHaveBeenLastCalledWith(inventory.options[1]);
  expect(screen.getByText(/separate installation confirmation/)).toBeTruthy();
  fireEvent.change(screen.getByLabelText("Download tool"), { target: { value: "2" } });
  expect(choice).toHaveBeenLastCalledWith(inventory.options[2]);
  expect((screen.getByLabelText("Selected tool identity") as HTMLInputElement).value).toBe(inventory.options[2].path);
  expect(screen.getByText(/image must already be on this node/)).toBeTruthy();
});

it("selection alone never grants installation consent", async () => {
  const inventory = demoDownloadTools("cell2"); inventory.options = [inventory.options[1]];
  vi.spyOn(api, "modelDownloadTools").mockResolvedValue(inventory);
  vi.spyOn(session, "confirmAction").mockResolvedValue(true);
  expect(await chooseDownloadTool("cell2")).toEqual({ node: "cell2", tool: inventory.options[0], installConsent: false });
  expect(downloadToolConsent({ node: "cell2", tool: inventory.options[0], installConsent: false })).toContain("GitHub and packages from PyPI");
});

it("declining selection returns without a write", async () => {
  vi.spyOn(api, "modelDownloadTools").mockResolvedValue(demoDownloadTools("cell1"));
  vi.spyOn(session, "confirmAction").mockResolvedValue(false);
  const start = vi.spyOn(api, "modelDownload");
  expect(await chooseDownloadTool("cell1")).toBeNull();
  expect(start).not.toHaveBeenCalled();
});

it("no usable tool reports the discovery reason instead of falling back", async () => {
  vi.spyOn(api, "modelDownloadTools").mockResolvedValue({ node: "cell1", options: [], notes: ["Tool folder cannot be replaced."] });
  const confirm = vi.spyOn(session, "confirmAction");
  await expect(chooseDownloadTool("cell1")).rejects.toThrow("Tool folder cannot be replaced.");
  expect(confirm).not.toHaveBeenCalled();
});
