import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { ContextMenuHost } from "../components/ContextMenuHost";
import { closeContextMenu } from "../lib/contextMenu";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Sidebar } from "../components/Sidebar";
import { baseSnapshot } from "../dev/mockBackend";
import { _resetExitsForTests, ackExit } from "../lib/exits";
import { applyHealth } from "../lib/health";
import { resetStore, startClusterStore } from "../lib/store";
import type { ClusterSnapshot } from "../lib/types";
import { isKnown, sizeOf } from "../windows/registry";
import { WindowManagerProvider } from "../wm/WindowManager";
import { mockTauri } from "./tauri";

// The real registry imports every window; the sidebar only asks which kinds exist.
vi.mock("../windows/registry", () => {
  const known = new Set(["dashboard", "topology", "node", "workloads", "containers", "services", "models", "chat", "server", "providers", "vpn", "switch", "files", "terminal", "fleet", "activity", "settings", "planned", "tasks"]);
  return { isKnown: (k: string) => known.has(k), sizeOf: () => [600, 400] as [number, number] };
});

async function renderSidebar(edit: (s: ClusterSnapshot) => void = () => {}) {
  resetStore();
  const snap = structuredClone(baseSnapshot);
  edit(snap);
  mockTauri({
    get_snapshot: () => snap,
    vpn_get: () => ({ settings: { mode: "auto", profile: null, forwards: [] }, status: { state: "direct", forwards: [] } }),
  });
  await startClusterStore();
  return render(
    <WindowManagerProvider sizeOf={sizeOf} isKnown={isKnown}>
      <Sidebar />
    </WindowManagerProvider>,
  );
}

describe("Sidebar", () => {
  beforeEach(() => {
    localStorage.clear();
    _resetExitsForTests();
  });

  it("groups the menu into source-list sections with live rows and badges", async () => {
    await renderSidebar();
    const nav = screen.getByRole("navigation", { name: "Main menu" });
    for (const s of ["Overview", "Nodes", "Workloads", "AI", "Network", "Tools"]) expect(within(nav).getByRole("group", { name: s })).toBeInTheDocument();
    expect(within(nav).queryByRole("group", { name: "Coming soon" })).toBeNull();

    const cell1 = within(nav).getByRole("button", { name: /cell1/ });
    expect(cell1).toHaveTextContent(/GPU \d+%/);
    expect(within(cell1).getByRole("img")).toHaveAccessibleName(/Healthy|Online|Warning/);
    // The cluster as found (Oct 29): no failed units, so no Services badge.
    expect(within(nav).getByRole("button", { name: /Services/ })).not.toHaveTextContent("failed");
    // The glm53 run exited two days before the snapshot: history, not a badge.
    expect(within(nav).getByRole("button", { name: /Containers/ })).not.toHaveTextContent("exited");
    // A switch with no address set says so, quietly.
    expect(within(nav).getByRole("button", { name: /CRS804 DDQ/ })).not.toHaveTextContent("Online");
    expect(within(nav).getByRole("button", { name: /Settings/ })).toHaveTextContent("⌘,");
  });

  it("collapses a section from its chevron and remembers it", async () => {
    const user = userEvent.setup();
    const { unmount } = await renderSidebar();
    await user.click(screen.getByRole("button", { name: "Hide AI" }));
    expect(screen.queryByRole("button", { name: /Chat/ })).toBeNull();
    expect(JSON.parse(localStorage.getItem("sparkbox.sidebar.collapsed")!).ai).toBe(true);
    unmount();
    await renderSidebar();
    expect(screen.queryByRole("button", { name: /Chat/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Show AI" })).toHaveAttribute("aria-expanded", "false");
  });

  it("filters rows, finds hidden ones like vLLM, and clears with Escape", async () => {
    const user = userEvent.setup();
    await renderSidebar();
    expect(screen.queryByRole("button", { name: /vLLM/ })).toBeNull();
    const filter = screen.getByRole("searchbox", { name: "Filter the menu" });
    await user.type(filter, "vllm");
    expect(screen.getByRole("button", { name: /vLLM/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Dashboard/ })).toBeNull();
    await user.keyboard("{Escape}");
    expect(filter).toHaveValue("");
    expect(screen.getByRole("button", { name: /Dashboard/ })).toBeInTheDocument();
  });

  it("marks the window in front as current", async () => {
    const user = userEvent.setup();
    await renderSidebar();
    const dash = screen.getByRole("button", { name: /Dashboard/ });
    expect(dash).not.toHaveAttribute("aria-current");
    await act(async () => {
      await user.click(dash);
    });
    expect(screen.getByRole("button", { name: /Dashboard/ })).toHaveAttribute("aria-current", "page");
  });

  it("badges an exit from the last 24 hours until it's hidden", async () => {
    // gx10-0000's glm53 rank exits with code 1 ten minutes before the snapshot.
    const recent = (snap: ClusterSnapshot) => {
      const c = snap.nodes.find((n) => n.id === "gx10-0000")!.containers.find((x) => x.name === "glm53")!;
      c.finishedAt = Math.round(snap.tsMs / 1000) - 600;
    };
    const { unmount } = await renderSidebar(recent);
    const containers = screen.getByRole("button", { name: /Containers/ });
    expect(containers).toHaveTextContent("1 exited");
    expect(within(containers).getByText("1 exited")).toHaveAttribute("title", expect.stringMatching(/glm53/));
    unmount();
    const snap = structuredClone(baseSnapshot);
    recent(snap);
    const c = snap.nodes.find((n) => n.id === "gx10-0000")!.containers.find((x) => x.name === "glm53")!;
    ackExit("gx10-0000", c.id, c.finishedAt);
    await renderSidebar(recent);
    expect(screen.getByRole("button", { name: /Containers/ })).not.toHaveTextContent("exited");
  });

  it("pins a node that's down to the top of Nodes, in red", async () => {
    await renderSidebar((snap) => {
      const n = snap.nodes.find((x) => x.id === "cell3")!;
      n.status = "offline";
      n.error = "SSH timeout";
    });
    const nodes = within(screen.getByRole("group", { name: "Nodes" })).getAllByRole("button").filter((b) => b.classList.contains("src-row"));
    expect(nodes[0]).toHaveTextContent("cell3");
    const word = within(nodes[0]).getByText("Offline");
    expect(word).toHaveClass("crit-text");
    // The loudest glyph: a filled red circle with a slash.
    expect(nodes[0].querySelector("svg.sglyph-offline")).toHaveAccessibleName("Offline");
    // Healthy nodes stay quiet: no badge, no red.
    expect(nodes[1].querySelector(".src-badge, .crit-text")).toBeNull();
    // Its menu offers Wake; a node that's on doesn't.
    const { unmount } = render(<ContextMenuHost />);
    const labels = () => screen.getAllByRole("menuitem").map((m) => m.textContent ?? "");
    fireEvent.contextMenu(nodes[0]);
    // The one node menu (contextMenu.nodeMenu), as the Dashboard, Topology and the status bar have it.
    expect(labels().slice(0, 8)).toEqual([
      "Open", "Terminalcell3 isn't connected", "Files", "Containers", "Services", "Restart…cell3 isn't connected", "Shut down…cell3 isn't connected", "Wake",
    ]);
    act(() => closeContextMenu(false));
    fireEvent.contextMenu(nodes[1]);
    expect(labels()).not.toContain("Wake");
    act(() => closeContextMenu(false));
    unmount();
  });

  it("the Models row's menu says Load a model…", async () => {
    await renderSidebar();
    const { unmount } = render(<ContextMenuHost />);
    fireEvent.contextMenu(within(screen.getByRole("navigation", { name: "Main menu" })).getByRole("button", { name: /^Models/ }));
    expect(screen.getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Open Models", "Load a model…"]);
    act(() => closeContextMenu(false));
    unmount();
  });

  it("shows the alert total tinted by the worst, with the breakdown in its tooltip", async () => {
    await renderSidebar();
    act(() => applyHealth({ tsMs: Date.now(), alerts: [], summary: { crit: 1, warn: 2, pending: 0, muted: 0 }, nodes: {}, switches: {}, lastAlertMs: null }));
    const badge = await screen.findByText("3", { selector: ".src-badge" });
    expect(badge).toHaveClass("crit");
    expect(badge).toHaveAttribute("title", "1 critical · 2 warnings");
  });

  it("calls a device by its one name everywhere, cut with an ellipsis when it must be, with the full name in the tooltip", async () => {
    await renderSidebar();
    const row = screen.getByRole("button", { name: /Firewalla Switch X/ });
    expect(row.querySelector(".src-label")).toHaveTextContent(/^Firewalla Switch X$/);
    expect(row.getAttribute("title")).toMatch(/^Firewalla Switch X/);
  });

  it("colours a node by the node alone: a model it serves being down is a note, not red", async () => {
    await renderSidebar();
    const alert = (id: string, kind: "endpoint" | "disk", node: string, severity: "warn" | "crit") => ({
      id, rule: kind === "endpoint" ? "endpoint-down" : "disk-full", category: "node" as const, severity, state: "firing" as const,
      entity: { kind, id: kind === "endpoint" ? "cell1-vllm-8000" : "/", node, switch: null, label: id },
      message: id, summary: id, detail: null, value: null, sinceMs: 0, firingSinceMs: 0, resolvedMs: null, ackedMs: null, mutedUntilMs: null,
    });
    act(() => applyHealth({
      tsMs: Date.now() + 1,
      alerts: [alert("llama-3.3-70b is down", "endpoint", "cell1", "crit"), alert("gx10-0000 disk 91%", "disk", "gx10-0000", "warn")],
      summary: { crit: 1, warn: 1, pending: 0, muted: 0 },
      nodes: {
        cell1: { level: "crit", issues: [{ alertId: "llama-3.3-70b is down", severity: "crit", summary: "llama-3.3-70b is down", label: "" }] },
        "gx10-0000": { level: "warn", issues: [{ alertId: "gx10-0000 disk 91%", severity: "warn", summary: "gx10-0000 disk 91%", label: "" }] },
      },
      switches: {},
      lastAlertMs: null,
    }));
    const row = (name: string) => [...document.querySelectorAll<HTMLElement>(".src-row")].find((b) => b.querySelector(".src-label")?.textContent === name)!;
    const cell1 = row("cell1");
    expect(cell1.querySelector(".sglyph-crit, .sglyph-warn, .sglyph-offline")).toBeNull();
    expect(cell1.getAttribute("title")).not.toMatch(/Critical/);
    // The disk is gx10's own: amber triangle.
    const gx10 = row("gx10-0000");
    expect(gx10.querySelector("svg.sglyph-warn")).toHaveAccessibleName("Warning");
    expect(gx10.getAttribute("title")).toMatch(/Warning: gx10-0000 disk 91%/);
  });
});
