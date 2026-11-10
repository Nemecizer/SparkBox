// Activity log › Everything shows a background poll as one row (C11, P1.47): the rows of an
// operation whose source is "poll" fold into "Check updates · 4 nodes · ok" (or "· 1 failed"),
// with its commands in the detail. Changes doesn't list polls; other operations still open into
// their commands.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import type { ActivityEntry } from "../lib/opsApi";
import { resetStore, startClusterStore } from "../lib/store";
import { mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { ActivityLog, groupEntries, groupSummary } from "./ActivityLog";

const NOW = Date.now();
const NODES = ["cell1", "cell2", "cell3", "gx10-0000"];

function entry(over: Partial<ActivityEntry>): ActivityEntry {
  return {
    id: "a", tsMs: NOW - 60_000, updatedMs: NOW - 60_000, node: "cell1", kind: "command", command: "apt list --upgradable", code: 0, ms: 140,
    changes: false, error: null, opId: null, title: "", source: "system", actor: "system", escalated: false, state: "ok",
    stdoutTail: null, stderrTail: null, simulated: false, class: "probe", demo: false, ...over,
  };
}

/** One update check's four probe rows, as audit::with_op_id("poll-…", source "poll") records them. */
function poll(op: string, at: number, failed?: string): ActivityEntry[] {
  return NODES.map((node, i) => entry({
    id: `${op}-${node}`, opId: op, node, tsMs: at + i, updatedMs: at + i, title: "Check updates", source: "poll",
    ...(node === failed ? { state: "failed", code: 100, error: "E: Could not get lock /var/lib/dpkg/lock-frontend" } : {}),
  }));
}

async function show(entries: ActivityEntry[]) {
  resetStore();
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { activity_log: () => entries, activity_log_since: () => [] }));
  await startClusterStore();
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1200, 800]} isKnown={() => true}><ActivityLog params={{}} /></WindowManagerProvider>);
  });
  for (let i = 0; i < 6; i++) await act(async () => new Promise((res) => setTimeout(res, 20)));
  await act(async () => {
    fireEvent.click(screen.getByRole("radio", { name: /Everything/ }));
  });
  return r.container;
}

const rows = (root: HTMLElement) => [...root.querySelectorAll<HTMLElement>("tbody tr[data-key]")];
/** A row's What cell as it reads: its title, then its summary ("Check updates · 4 nodes · ok"). */
const what = (r: HTMLElement) => [r.querySelector(".ops-what-title, .ops-what-cmd")?.textContent, r.querySelector(".ops-what-summary")?.textContent].filter(Boolean).join(" ");

describe("Activity log › a poll is one row", () => {
  it("folds a poll's four probe rows into one, its commands in the detail", async () => {
    const root = await show(poll("poll-1", NOW - 60_000));
    const shown = rows(root);
    expect(shown.length).toBe(1);
    expect(what(shown[0])).toBe("Check updates · 4 nodes · ok");
    expect(shown[0].querySelector(".ops-disclosure"), "no disclosure: it doesn't open into four rows").toBeNull();
    await act(async () => {
      fireEvent.mouseDown(shown[0].querySelector("td")!, { button: 0 });
    });
    expect(root.textContent).toContain("Commands · 4");
  });

  it("says how many nodes failed", async () => {
    const root = await show(poll("poll-2", NOW - 60_000, "cell3"));
    expect(rows(root).map(what)).toEqual(["Check updates · 4 nodes · 1 failed"]);
  });

  it("is one row per poll, out of Changes, and leaves other operations alone", async () => {
    const restart = [
      entry({ id: "r1", opId: "op-r", tsMs: NOW - 30_000, node: "cell1", kind: "command", command: "docker restart abc", title: "Restart 2 containers", source: "ui", actor: "ui", class: "change", changes: true }),
      entry({ id: "r2", opId: "op-r", tsMs: NOW - 30_000, node: "cell2", kind: "command", command: "docker restart def", title: "Restart 2 containers", source: "ui", actor: "ui", class: "change", changes: true }),
    ];
    const root = await show([...poll("poll-3", NOW - 120_000), ...poll("poll-4", NOW - 60_000), ...restart]);
    expect(rows(root).length).toBe(3);
    // The restart still opens into its two commands.
    expect(rows(root).map(what)).toEqual(["Restart 2 containers · 2/2 ok", "Check updates · 4 nodes · ok", "Check updates · 4 nodes · ok"]);
    const head = rows(root).find((r) => r.textContent?.includes("Restart 2 containers"))!;
    await act(async () => {
      fireEvent.click(head.querySelector(".ops-disclosure")!);
    });
    expect(rows(root).length).toBe(4);
    // Changes: the restart only.
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: /Changes/ }));
    });
    expect(rows(root).filter((r) => !r.classList.contains("ops-child-row")).map(what)).toEqual(["Restart 2 containers · 2/2 ok"]);
  });

  it("counts nodes, not commands, and says a poll still running is running", () => {
    const twice = [...poll("poll-5", NOW), entry({ id: "poll-5-cell1-b", opId: "poll-5", node: "cell1", title: "Check updates", source: "poll", state: "failed", tsMs: NOW + 9 })];
    expect(groupSummary(groupEntries(twice)[0])).toBe("4 nodes · 1 failed");
    const going = poll("poll-6", NOW).map((e, i) => (i === 0 ? { ...e, state: "running" as const } : e));
    expect(groupSummary(groupEntries(going)[0])).toBe("4 nodes · running");
    expect(groupSummary(groupEntries(poll("poll-7", NOW).slice(0, 1))[0])).toBe("1 node · ok");
  });
});
