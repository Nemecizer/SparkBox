import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { _resetAdminForTests, LOOK_ONLY_TIP } from "../../lib/admin";
import { _resetDataSourceForTests } from "../../lib/dataSource";
import type { AdminState } from "../../lib/opsApi";
import type { EvidenceFolder } from "../../lib/evidence";
import { mockCalls, mockTauri } from "../../test/tauri";
import { DiagnosticsSection } from "./DiagnosticsSection";

vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: vi.fn(async () => {}) }));
// Confirmations are answered by a stub that records what it was asked (and cancels).
const asked: string[] = [];
vi.mock("../../lib/session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/session")>()),
  confirmAction: vi.fn(async (o: { title: string }) => (asked.push(o.title), false)),
}));

const folder = (n: number, extra: Partial<EvidenceFolder> = {}): EvidenceFolder => ({
  path: `/Users/me/Library/Logs/SparkBox/evidence/2026-10-29-p1/10150${n}-step-${n}`,
  phase: "p1", label: `step-${n}`, startedMs: 1_793_300_000_000 + n * 1000, nodes: 4, failedSteps: 0, notConnected: [], diff: null, problems: [], ...extra,
});

describe("Settings › Diagnostics › Evidence", () => {
  it("says it needs Live data in Demo, and asks the backend nothing", async () => {
    _resetDataSourceForTests("demo");
    mockTauri({});
    render(<DiagnosticsSection />);
    expect(screen.getByText("Available with Live data")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Capture evidence" })).toBeDisabled();
    expect(mockCalls.filter((c) => c.cmd.startsWith("evidence_"))).toHaveLength(0);
    // The doctor would log in to the real nodes: Live data only.
    expect(screen.getByText("Available with Live data: it reads the real nodes")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run doctor" })).toBeDisabled();
  });

  it("captures with the group and label given, then shows where it went and the last five", async () => {
    _resetDataSourceForTests("live");
    const listed = [1, 2, 3, 4, 5, 6].map((n) => folder(n));
    const made = folder(7, { failedSteps: 3, notConnected: ["cell3"], diff: "/x/diff-from-101506-step-6.txt" });
    mockTauri({ evidence_list: () => listed, evidence_capture: () => made });
    render(<DiagnosticsSection />);
    expect(await screen.findByText("p1 · step-5")).toBeInTheDocument();
    expect(screen.getByLabelText("Group")).toHaveValue("general");
    expect(screen.getByText("Captures in the same group are compared with each other")).toBeInTheDocument();
    expect(screen.queryByText("p1 · step-6"), "five at most").toBeNull();
    fireEvent.change(screen.getByLabelText("Label"), { target: { value: "before update" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Capture evidence" }));
    });
    expect(mockCalls.find((c) => c.cmd === "evidence_capture")?.args).toEqual({ phase: "general", label: "before update" });
    expect(screen.getByText(made.path)).toBeInTheDocument();
    expect(screen.getByText("4 nodes · 3 steps failed · cell3 not connected")).toBeInTheDocument();
    const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
    fireEvent.click(screen.getAllByRole("button", { name: "Show in Finder" })[0]);
    expect(revealItemInDir).toHaveBeenCalledWith(made.path);
    _resetDataSourceForTests("demo");
  });

  it("shows what a capture couldn't file, and counts it in the list", async () => {
    _resetDataSourceForTests("live");
    const made = folder(8, { problems: ["No activity log: export failed", "Couldn't copy ssh.log: disk full"] });
    let listed: EvidenceFolder[] = [];
    mockTauri({ evidence_list: () => listed, evidence_capture: () => ((listed = [made]), made) });
    render(<DiagnosticsSection />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Capture evidence" }));
    });
    expect(screen.getByText("No activity log: export failed · Couldn't copy ssh.log: disk full")).toBeInTheDocument();
    expect(screen.getAllByText("4 nodes · 2 problems")).toHaveLength(1);
    expect(await screen.findByText(/· 4 nodes · 2 problems$/)).toBeInTheDocument();
    _resetDataSourceForTests("demo");
  });

  it("an empty group is the default group", async () => {
    _resetDataSourceForTests("live");
    mockTauri({ evidence_list: () => [], evidence_capture: () => folder(1) });
    render(<DiagnosticsSection />);
    fireEvent.change(screen.getByLabelText("Group"), { target: { value: "  " } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Capture evidence" }));
    });
    expect(mockCalls.find((c) => c.cmd === "evidence_capture")?.args.phase).toBe("general");
    _resetDataSourceForTests("demo");
  });

  it("says a capture can take a few minutes while it runs", async () => {
    _resetDataSourceForTests("live");
    let finish!: (f: EvidenceFolder) => void;
    mockTauri({ evidence_list: () => [], evidence_capture: () => new Promise((r) => { finish = r; }) });
    render(<DiagnosticsSection />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Capture evidence" }));
    });
    expect(screen.getByText("Reading every node; this can take a few minutes")).toBeInTheDocument();
    await act(async () => {
      finish(folder(9));
    });
    _resetDataSourceForTests("demo");
  });

  it("shows why a capture failed", async () => {
    _resetDataSourceForTests("live");
    mockTauri({ evidence_list: () => [], evidence_capture: () => { throw new Error("No node is connected, so there's nothing to capture."); } });
    render(<DiagnosticsSection />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Capture evidence" }));
    });
    expect(screen.getByText("No node is connected, so there's nothing to capture.")).toBeInTheDocument();
    _resetDataSourceForTests("demo");
  });
});

describe("Settings › Diagnostics › SSH host keys", () => {
  const LOCKED: AdminState = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };

  it("look-only, Forget host keys is off and says why, so it never asks then refuses", () => {
    _resetAdminForTests({ ...LOCKED, lookOnly: true });
    mockTauri({});
    render(<DiagnosticsSection />);
    const forget = screen.getByRole("button", { name: "Forget host keys…" });
    expect(forget).toBeDisabled();
    expect(forget).toHaveAttribute("title", LOOK_ONLY_TIP);
    asked.length = 0;
    fireEvent.click(forget);
    expect(asked, "no confirmation to accept and then refuse").toEqual([]);
    expect(mockCalls.filter((c) => c.cmd === "known_hosts_forget")).toHaveLength(0);
    _resetAdminForTests();
  });

  it("locked, it stays enabled with no look-only tooltip and asks first", async () => {
    _resetAdminForTests(LOCKED);
    mockTauri({});
    render(<DiagnosticsSection />);
    const forget = screen.getByRole("button", { name: "Forget host keys…" });
    expect(forget).toBeEnabled();
    expect(forget).not.toHaveAttribute("title");
    asked.length = 0;
    await act(async () => {
      fireEvent.click(forget);
    });
    expect(asked).toEqual(["Forget the pinned SSH host keys?"]);
    expect(mockCalls.filter((c) => c.cmd === "known_hosts_forget"), "cancelled: nothing runs").toHaveLength(0);
    _resetAdminForTests();
  });
});
