import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { alertError, alertInfo, confirmAction, confirmChoice, promptText, singleFlight } from "../lib/session";
import { ConfirmHost } from "./ConfirmHost";

function app() {
  return render(
    <>
      <div className="app"><button>behind</button></div>
      <ConfirmHost />
    </>,
  );
}

describe("dialogs", () => {
  it("queues requests and settles every one in order", async () => {
    app();
    let a!: Promise<boolean>;
    let b!: Promise<boolean>;
    act(() => {
      a = confirmAction({ title: "Restart vllm-head?", body: "Requests in flight fail.", confirmLabel: "Restart" });
      b = confirmAction({ title: "Stop ollama?", confirmLabel: "Stop" });
    });
    expect(screen.getAllByRole("alertdialog")).toHaveLength(1);
    expect(screen.getByRole("alertdialog")).toHaveAccessibleName("Restart vllm-head?");
    expect(screen.getByRole("alertdialog")).toHaveAccessibleDescription("Requests in flight fail.");
    fireEvent.click(screen.getByRole("button", { name: "Restart" }));
    await act(async () => expect(await a).toBe(true));
    expect(screen.getByRole("alertdialog")).toHaveAccessibleName("Stop ollama?");
    fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
    await act(async () => expect(await b).toBe(false));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("defaults to the action for normal asks and to Cancel for destructive ones", async () => {
    app();
    let p!: Promise<boolean>;
    act(() => {
      p = confirmAction({ title: "Download 141 GB?", confirmLabel: "Download" });
    });
    expect(screen.getByRole("button", { name: "Download" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "Download" }));
    await act(async () => void (await p));
    act(() => {
      p = confirmAction({ title: "Remove the container?", confirmLabel: "Remove", danger: true });
    });
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    // Clicking outside a destructive ask does nothing.
    fireEvent.pointerDown(document.querySelector(".sb-overlay")!);
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await act(async () => expect(await p).toBe(false));
  });

  it("makes the app inert behind a dialog and gives focus back after", async () => {
    app();
    const behind = screen.getByText("behind");
    behind.focus();
    let p!: Promise<void>;
    act(() => {
      p = alertInfo({ title: "DGX Dashboard isn't available in Demo", body: "Switch to Live to open it." });
    });
    expect(document.querySelector(".app")).toHaveAttribute("inert");
    // Information: a single OK.
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getAllByRole("button").map((b) => b.textContent)).toEqual(["OK"]);
    fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
    await act(async () => {
      await p;
      await new Promise((r) => requestAnimationFrame(() => r(null)));
    });
    expect(document.querySelector(".app")).not.toHaveAttribute("inert");
    expect(behind).toHaveFocus();
  });

  it("keeps a typed confirmation disabled until the name matches", async () => {
    app();
    let p!: Promise<boolean>;
    act(() => {
      p = confirmAction({ title: "Shut down cell3?", confirmLabel: "Shut down", danger: true, requireTyping: "cell3" });
    });
    expect(screen.getByText(/to confirm/)).toHaveTextContent("Type cell3 to confirm");
    const field = screen.getByRole("textbox");
    expect(field).toHaveFocus();
    const go = screen.getByRole("button", { name: "Shut down" });
    expect(go).toBeDisabled();
    fireEvent.change(field, { target: { value: "cell" } });
    expect(go).toBeDisabled();
    fireEvent.keyDown(field, { key: "Enter" });
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    fireEvent.change(field, { target: { value: "cell3" } });
    expect(go).toBeEnabled();
    fireEvent.keyDown(field, { key: "Enter" });
    await act(async () => expect(await p).toBe(true));
  });

  it("shows error details behind a disclosure, with actions", async () => {
    app();
    const openSettings = vi.fn();
    let p!: Promise<void>;
    act(() => {
      p = alertError({
        title: "Couldn't reach cell2",
        body: "SSH timed out after 10 s.",
        details: "ssh: connect to host 192.168.50.12 port 22: Operation timed out",
        actions: [{ label: "Open Settings", run: openSettings }],
      });
    });
    expect(screen.getByText("Details")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Copy/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    await act(async () => void (await p));
    expect(openSettings).toHaveBeenCalledOnce();
  });

  it("asks for text and validates it", async () => {
    app();
    let p!: Promise<string | null>;
    act(() => {
      p = promptText({ title: "Save layout", label: "Name", confirmLabel: "Save", validate: (v) => (v === "taken" ? "That name is in use." : null) });
    });
    const field = screen.getByRole("textbox");
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    fireEvent.change(field, { target: { value: "taken" } });
    expect(screen.getByText("That name is in use.")).toBeInTheDocument();
    fireEvent.change(field, { target: { value: "  Bring-up  " } });
    fireEvent.click(save);
    await act(async () => expect(await p).toBe("Bring-up"));
  });

  it("shows one dialog for the same request asked three times, and settles them together", async () => {
    app();
    let ps: Promise<boolean>[] = [];
    act(() => {
      ps = [1, 2, 3].map(() => confirmAction({ title: "Restart open-webui on cell1?", confirmLabel: "Restart", dedupeKey: "restart:cell1:4c1e" }));
    });
    expect(ps[1]).toBe(ps[0]);
    expect(ps[2]).toBe(ps[0]);
    expect(screen.getAllByRole("alertdialog")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Restart" }));
    await act(async () => expect(await Promise.all(ps)).toEqual([true, true, true]));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // Once settled, the same request asks again.
    let again!: Promise<boolean>;
    act(() => {
      again = confirmAction({ title: "Restart open-webui on cell1?", confirmLabel: "Restart", dedupeKey: "restart:cell1:4c1e" });
    });
    expect(again).not.toBe(ps[0]);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await act(async () => expect(await again).toBe(false));
  });

  it("runs an action once however often it's asked for while it's under way (a triple-click)", async () => {
    app();
    const run = vi.fn(async () => "task-1");
    const click = () => singleFlight("restart:cell1:4c1e", async () => {
      if (!(await confirmAction({ title: "Restart open-webui on cell1?", confirmLabel: "Restart", dedupeKey: "restart:cell1:4c1e" }))) return null;
      return run();
    });
    let ps: Promise<string | null>[] = [];
    act(() => {
      ps = [click(), click(), click()];
    });
    expect(screen.getAllByRole("alertdialog")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Restart" }));
    await act(async () => expect(await Promise.all(ps)).toEqual(["task-1", "task-1", "task-1"]));
    expect(run).toHaveBeenCalledOnce();
  });

  it("offers two ways to go ahead, stacked with the default on top and focused", async () => {
    app();
    let p!: Promise<"confirm" | "alternative" | null>;
    act(() => {
      p = confirmChoice({ title: "Restart 3 containers on cell1–cell3?", confirmLabel: "Restart llama-3.3-70b in order", alternativeLabel: "Restart 3 containers anyway" });
    });
    const dialog = screen.getByRole("alertdialog");
    const buttons = within(dialog).getAllByRole("button").map((b) => b.textContent);
    expect(buttons).toEqual(["Restart llama-3.3-70b in order", "Restart 3 containers anyway", "Cancel"]);
    expect(within(dialog).getByRole("button", { name: "Restart llama-3.3-70b in order" })).toHaveFocus();
    fireEvent.click(within(dialog).getByRole("button", { name: "Restart 3 containers anyway" }));
    await act(async () => expect(await p).toBe("alternative"));
    act(() => {
      p = confirmChoice({ title: "Again?", confirmLabel: "Default", alternativeLabel: "Other" });
    });
    fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
    await act(async () => expect(await p).toBeNull());
  });
});
