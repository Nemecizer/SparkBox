// Locked and look-only states shown truthfully (P1.43), against the browser preview's backend:
// every registered window's toolbar rendered locked and in look-only mode, Fleet's chip with Run
// and "as root" off, the Settings forms read-only, and the palette's look-only footer.
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CommandPalette } from "../components/CommandPalette";
import { ContextMenuHost } from "../components/ContextMenuHost";
import { defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP } from "../lib/admin";
import type { AdminState } from "../lib/opsApi";
import { registerCommands } from "../lib/commands";
import { openPanel } from "../lib/session";
import { resetStore, startClusterStore } from "../lib/store";
import { FleetCommand } from "../windows/FleetCommand";
import { REGISTRY, WindowBody } from "../windows/registry";
import { Settings } from "../windows/Settings";
import { WindowManagerProvider } from "../wm/WindowManager";
import { extendMock, mockCalls, mockTauri } from "./tauri";

vi.mock("uplot", () => {
  class FakePlot {
    series: { show: boolean }[] = [];
    setData() {}
    setSeries() {}
    setSize() {}
    setSelect() {}
    destroy() {}
    static sync() {
      return { sub() {}, unsub() {}, pub() {} };
    }
  }
  return { default: FakePlot };
});

const LOCKED: AdminState = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const LOOK_ONLY: AdminState = { ...LOCKED, lookOnly: true };
/** Params for the kinds that can't open without them. */
const PARAMS: Record<string, Record<string, string>> = {
  node: { id: "cell1" }, switch: { id: "crs804" }, inspect: { node: "cell1", id: "x" }, editor: { place: "cell1", path: "/home/sparkuser/notes.txt" }, planned: { key: "x" },
};

/** The preview's whole backend, with the admin state the test wants. */
async function backend(admin: AdminState) {
  resetStore();
  mockTauri(defaultHandlers());
  extendMock(Object.assign({}, ...MOCKS));
  extendMock({ admin_state: () => admin });
  _resetAdminForTests(admin);
  await startClusterStore();
}

async function show(ui: React.ReactElement) {
  let r: ReturnType<typeof render> | undefined;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1200, 800]} isKnown={() => true}>{ui}</WindowManagerProvider>);
  });
  // Let the window's first reads land.
  await act(async () => {
    await new Promise((res) => setTimeout(res, 120));
  });
  return r!;
}

interface Marked {
  name: string;
  lock: string | null;
  disabled: boolean;
  title: string | null;
}

/** Every toolbar action marked as needing Admin (`data-lock`), with how it reads. */
function marked(root: HTMLElement): Marked[] {
  return [...root.querySelectorAll<HTMLButtonElement>("[role=toolbar] [data-lock]")].map((b) => ({
    name: (b.getAttribute("aria-label") ?? "").replace(/ — (needs Admin|look-only)$/, ""),
    lock: b.getAttribute("data-lock"),
    disabled: b.disabled,
    title: b.getAttribute("title"),
  }));
}

/** Every title and aria-label in the window, and its text. */
function words(root: HTMLElement): string[] {
  const attrs = [...root.querySelectorAll("[title], [aria-label]")].flatMap((el) => [el.getAttribute("title") ?? "", el.getAttribute("aria-label") ?? ""]);
  return [...attrs, root.textContent ?? ""].filter(Boolean);
}

describe("every registered toolbar, locked and in look-only mode", () => {
  it("locked: needs Admin (click to unlock); look-only: disabled, how to turn it off, and no promise of an unlock", async () => {
    let total = 0;
    const kinds = Object.keys(REGISTRY);
    for (const kind of kinds) {
      await backend(LOCKED);
      const locked = await show(<WindowBody kind={kind} params={PARAMS[kind] ?? {}} />);
      const before = marked(locked.container);
      for (const b of before) {
        expect(b.lock, `${kind}: ${b.name}`).toBe("locked");
        expect(b.title?.endsWith(NEEDS_ADMIN_TIP), `${kind}: ${b.name} says "${b.title}"`).toBe(true);
      }
      locked.unmount();

      await backend(LOOK_ONLY);
      const looking = await show(<WindowBody kind={kind} params={PARAMS[kind] ?? {}} />);
      const after = marked(looking.container);
      expect(after.map((b) => b.name), `${kind}: the same actions are marked`).toEqual(before.map((b) => b.name));
      for (const b of after) {
        expect([b.lock, b.disabled, b.title], `${kind}: ${b.name}`).toEqual(["lookOnly", true, LOOK_ONLY_TIP]);
      }
      if (after.length) expect(within(looking.container).getAllByRole("button", { name: /^Look only/ }).length, `${kind}: the Look only chip`).toBeGreaterThan(0);
      // Help's pages are about everything, Admin mode included: its words promise nothing.
      if (kind !== "help") expect(words(looking.container).filter((w) => /click to unlock|unlock/i.test(w)), `${kind}: nothing in look-only promises an unlock`).toEqual([]);
      looking.unmount();
      total += after.length;
    }
    // 29 when this was written: node, Containers, Services, Workloads, Updates, Files, Server, VPN,
    // Models, Alerts, Ollama, Fabric. (Fleet's Run sits beside its box, not in the toolbar: below.)
    expect(total).toBeGreaterThanOrEqual(25);
  }, 120_000);
});

/** Change actions by their label (P1.43): none may be enabled in look-only mode, wherever it sits. */
const CHANGES = [
  /^Terminal here/, /^Open terminal here/, /^New shell/, /^Shell$/, /^Create key/, /^Add alias/, /^Save settings/, /^Revoke /, /^Move to Trash/,
  /^Delete immediately/, /^Rename/, /^Permissions…/, /^Duplicate/, /^New folder/, /^Paste$/, /^Copy to /, /^Move to /, /^Read with sudo…/,
];

/** The enabled controls and menu items in `root` whose label is a change action. */
function enabledChanges(root: ParentNode): string[] {
  const label = (el: Element) => (el.getAttribute("aria-label") ?? el.querySelector(".ctx-label")?.childNodes[0]?.textContent ?? el.textContent ?? "").trim();
  const off = (el: HTMLElement) => el.matches(":disabled") || el.getAttribute("aria-disabled") === "true";
  return [...root.querySelectorAll<HTMLElement>("button, [role=menuitem], [role=switch]")]
    .filter((el) => !off(el))
    .map(label)
    .filter((name) => CHANGES.some((re) => re.test(name)));
}

/** Every registered window under `admin`: the change actions it has enabled, its toolbar's More menu included. */
async function sweep(admin: AdminState): Promise<string[]> {
  const found: string[] = [];
  for (const kind of Object.keys(REGISTRY)) {
    await backend(admin);
    const r = await show(<><WindowBody kind={kind} params={PARAMS[kind] ?? {}} /><ContextMenuHost /></>);
    found.push(...enabledChanges(r.container).map((n) => `${kind}: ${n}`));
    for (const more of within(r.container).queryAllByRole("button", { name: /^More/ })) {
      await act(async () => fireEvent.click(more));
      found.push(...enabledChanges(document.body.querySelector("[role=menu]") ?? document.createElement("div")).map((n) => `${kind} › More: ${n}`));
      await act(async () => fireEvent.keyDown(document.body.querySelector("[role=menu]") ?? document.body, { key: "Escape" }));
    }
    r.unmount();
  }
  return found;
}

describe("no change action is enabled in look-only mode", () => {
  it("in any registered window, its toolbar's More menu included", async () => {
    expect(await sweep(LOOK_ONLY)).toEqual([]);
    // The sweep sees them: locked, they're enabled (a click asks to unlock).
    const locked = await sweep(LOCKED);
    for (const seen of ["files: Terminal here — needs Admin", "terminal: New shell", "server: Create key", "server: Add alias"]) {
      expect(locked, seen).toContain(seen);
    }
  }, 240_000);

  it("in Files' menus on a node: a row's and the folder's", async () => {
    await backend(LOOK_ONLY);
    const r = await show(<><WindowBody kind="files" params={{ place: "cell1", path: "~" }} /><ContextMenuHost /></>);
    const pane = r.container.querySelector<HTMLElement>("section.sb-pane[data-pane='1']")!;
    const row = await within(pane).findAllByRole("row").then((rows) => rows.find((x) => x.hasAttribute("data-key"))!);
    await act(async () => fireEvent.contextMenu(row));
    let menu = await screen.findByRole("menu");
    const items = within(menu).getAllByRole("menuitem").map((el) => el.textContent ?? "");
    for (const name of ["Copy to This Mac", "Move to This Mac", "Duplicate", "Rename…", "Permissions…", "Open terminal here", "Move to Trash"]) {
      expect(items.some((t) => t.startsWith(name)), `${name} is listed: ${items.join(" | ")}`).toBe(true);
    }
    expect(enabledChanges(menu)).toEqual([]);
    expect(menu.textContent).not.toMatch(/unlock/i);
    await act(async () => fireEvent.keyDown(menu, { key: "Escape" }));

    await act(async () => fireEvent.contextMenu(pane));
    menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((el) => el.textContent ?? "").some((t) => t.startsWith("New folder"))).toBe(true);
    expect(enabledChanges(menu)).toEqual([]);
    const terminal = within(r.container).getByRole("button", { name: "Terminal here — look-only" });
    expect(terminal).toBeDisabled();
    expect(terminal).toHaveAttribute("title", LOOK_ONLY_TIP);
  });

  it("in the Server window: its settings, keys and aliases are read-only, and it says what look-only holds", async () => {
    await backend(LOOK_ONLY);
    const r = await show(<WindowBody kind="server" params={{}} />);
    expect(await screen.findByText(/^Look-only holds inference: turning it on stops the API server/)).toBeInTheDocument();
    const fields = [...r.container.querySelectorAll<HTMLElement>(".settings-fieldset input, .settings-fieldset textarea, .settings-fieldset select, .settings-fieldset button")];
    expect(fields.length).toBeGreaterThan(5);
    expect(fields.filter((el) => !el.matches(":disabled")).map((el) => el.getAttribute("aria-label") ?? el.textContent)).toEqual([]);
    expect(screen.getByRole("button", { name: "Save settings" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "App name" })).toBeDisabled();
  });

  it("in the editor of a root-only file: Read with sudo… is off and says how to turn look-only off", async () => {
    await backend(LOOK_ONLY);
    await show(<WindowBody kind="editor" params={{ place: "cell1", path: "/etc/netplan/40-cx7.yaml" }} />);
    const read = await screen.findByRole("button", { name: /Read with sudo…/ });
    expect(read).toBeDisabled();
    expect(read).toHaveAttribute("title", LOOK_ONLY_TIP);
  });

  it("locked but not look-only, they stay enabled and say they need Admin", async () => {
    await backend(LOCKED);
    await show(<WindowBody kind="editor" params={{ place: "cell1", path: "/etc/netplan/40-cx7.yaml" }} />);
    const read = await screen.findByRole("button", { name: /Read with sudo…/ });
    expect(read).toBeEnabled();
    expect(read.getAttribute("title")).toMatch(new RegExp(`${NEEDS_ADMIN_TIP.replace(/[()]/g, "\\$&")}$`));
  });
});

describe("a window opened before the first snapshot", () => {
  it("keeps its hooks in order when the cluster arrives (Workloads, Fleet, Updates, Settings)", async () => {
    for (const kind of ["workloads", "fleet", "updates", "settings", "containers", "services"]) {
      resetStore();
      mockTauri(defaultHandlers());
      extendMock(Object.assign({}, ...MOCKS));
      _resetAdminForTests(LOOK_ONLY);
      const r = await show(<WindowBody kind={kind} params={{}} />);
      await act(async () => {
        await startClusterStore();
        await new Promise((res) => setTimeout(res, 120));
      });
      expect(r.container.textContent, kind).not.toContain("This window hit an error");
      expect(r.container.textContent, kind).not.toContain("Rendered more hooks");
      r.unmount();
    }
  });
});

describe("Fleet in look-only mode", () => {
  beforeEach(() => backend(LOOK_ONLY));

  it("shows the Look only chip; Run and as root are off with sudo reboot typed, Parallel and four nodes", async () => {
    const r = await show(<FleetCommand />);
    const box = screen.getByRole("textbox", { name: "Command" });
    fireEvent.change(box, { target: { value: "sudo reboot" } });
    expect(screen.getByRole("radio", { name: "Parallel" })).toBeChecked();
    expect(screen.getAllByRole("button", { pressed: true }).filter((b) => b.classList.contains("fleet-node")).map((b) => b.textContent)).toEqual(["cell1", "cell2", "cell3", "gx10-0000"]);
    expect(screen.getByRole("button", { name: /^Look only/ })).toHaveTextContent("Look only");
    const run = screen.getByRole("button", { name: "Run — look-only" });
    expect(run).toBeDisabled();
    expect(run).toHaveAttribute("title", LOOK_ONLY_TIP);
    expect(screen.getByRole("checkbox", { name: "as root" })).toBeDisabled();
    // ⌘↩ in the box runs nothing either.
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter", metaKey: true });
    });
    expect(mockCalls.filter((c) => c.cmd.startsWith("fleet_"))).toEqual([]);
    expect(words(r.container).filter((w) => /unlock/i.test(w))).toEqual([]);
  });

  it("locked, Run stays enabled and says it needs Admin", async () => {
    await backend(LOCKED);
    await show(<FleetCommand />);
    const run = screen.getByRole("button", { name: "Run — needs Admin" });
    expect(run).toBeEnabled();
    expect(run).toHaveAttribute("title", `Run (⌘↩) · ${NEEDS_ADMIN_TIP}`);
    expect(screen.getByRole("checkbox", { name: "as root" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Read-only. Unlock Admin mode" })).toHaveTextContent("Read-only");
  });
});

describe("Settings in look-only mode", () => {
  beforeEach(() => backend(LOOK_ONLY));

  it("the forms are read-only, but not the look-only switch, Diagnostics or the fence", async () => {
    const r = await show(<Settings params={{ section: "nodes" }} />);
    const pane = r.container.querySelector(".settings-pane-body fieldset") as HTMLFieldSetElement;
    expect(pane.disabled).toBe(true);
    expect(await screen.findByRole("textbox", { name: "cell2 IP address" })).toBeDisabled();
    r.unmount();

    const sec = await show(<Settings params={{ section: "security" }} />);
    expect((sec.container.querySelector(".settings-pane-body > fieldset") as HTMLFieldSetElement).disabled).toBe(false);
    expect(screen.getByRole("switch", { name: "Look-only mode" })).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "Lock after" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "CSV…" })).toBeDisabled();
    sec.unmount();

    const diag = await show(<Settings params={{ section: "diagnostics" }} />);
    expect((diag.container.querySelector(".settings-pane-body > fieldset") as HTMLFieldSetElement).disabled).toBe(false);
    diag.unmount();
  });

  it("locked but not look-only, the forms take edits", async () => {
    await backend(LOCKED);
    const r = await show(<Settings params={{ section: "nodes" }} />);
    expect((r.container.querySelector(".settings-pane-body fieldset") as HTMLFieldSetElement).disabled).toBe(false);
    expect(await screen.findByRole("textbox", { name: "cell2 IP address" })).toBeEnabled();
  });
});

describe("turning look-only on", () => {
  it("the toast says how many running tasks it stopped", async () => {
    const { lookOnlyOnBody } = await import("../windows/settings/SecuritySection");
    const base = "SparkBox is in look-only mode. It starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date.";
    expect(lookOnlyOnBody({ stopped: 0, stopping: 0, onNode: 0 })).toBe(base);
    expect(lookOnlyOnBody({ stopped: 2, stopping: 1, onNode: 0 })).toBe(`${base} 2 tasks stopped before their next step. 1 task can't be stopped part-way; look-only refuses its next change.`);
    expect(lookOnlyOnBody({ stopped: 1, stopping: 0 })).toBe(`${base} 1 task stopped before its next step.`);
    // An upgrade apt runs in its own unit isn't counted as stopped: it keeps running on the node.
    expect(lookOnlyOnBody({ stopped: 0, stopping: 0, onNode: 1 })).toBe(
      `${base} 1 task started work that keeps running on its node (an upgrade, a model loading); SparkBox can't stop it in look-only.`,
    );
  });
});

describe("Keep running held", () => {
  it("the endpoint card says Held · look-only, and Held · locked with the fresh back-off", async () => {
    const { EndpointCard } = await import("../windows/models/EndpointCard");
    const e = (held: string | null, nextRetryMs: number | null) => ({
      id: "cell1-vllm-8000", engine: "vllm", models: ["qwen3"], nodes: ["cell1", "cell2"], headNode: "cell1", port: 8000, url: "http://192.168.50.51:8000/v1",
      managedBy: "sparkbox", health: "down", containerIds: [], containerNames: [], expectedNodes: ["cell1", "cell2"], missingNodes: [],
      supervisor: { enabled: true, state: nextRetryMs ? "restarting" : "watching", restarts: 0, nextRetryMs, held },
    }) as unknown as import("../lib/endpoints").Endpoint;
    await backend(LOOK_ONLY);
    const r = await show(<EndpointCard endpoint={e("Held · look-only", null)} variant="full" />);
    expect(r.container.querySelector(".ai-card-sup")).toHaveTextContent("Keep running · Held · look-only");
    r.unmount();
    const l = await show(<EndpointCard endpoint={e("Held · locked", Date.now() + 120_000)} variant="full" />);
    expect(l.container.querySelector(".ai-card-sup")?.textContent).toMatch(/^Keep running · Held · locked · restarting in [12]:\d\d$/);
  });
});

describe("the palette", () => {
  const scenario = () =>
    registerCommands([
      { id: "demo:scenario:node-offline", title: "Demo scenario: Node offline", group: "Demo", run: () => {} },
      { id: "test:restart-cell3", title: "Restart cell3…", group: "Nodes", needsAdmin: true, run: () => {} },
    ]);

  it("in look-only mode says nothing runs, and its Admin rows can't be run", async () => {
    await backend(LOOK_ONLY);
    const off = scenario();
    try {
      await show(<CommandPalette />);
      act(() => openPanel("palette"));
      const palette = await screen.findByRole("dialog", { name: "Command palette" });
      expect(palette.querySelector(".palette-foot")).toHaveTextContent("Look-only: nothing runs");
      expect(palette.textContent).not.toMatch(/unlock/i);
      const row = within(palette).getByRole("option", { name: /Restart cell3/ });
      expect(row).toHaveAttribute("aria-disabled", "true");
      expect(row).toHaveTextContent(LOOK_ONLY_TIP);
    } finally {
      off();
    }
  });

  it("with Live data lists no Demo scenario commands; with Demo data it does", async () => {
    await backend(LOCKED);
    const off = scenario();
    try {
      const r = await show(<CommandPalette />);
      act(() => openPanel("palette"));
      let palette = await screen.findByRole("dialog", { name: "Command palette" });
      expect(palette.textContent).not.toContain("Demo scenario");
      expect(palette.querySelector(".palette-foot")).toHaveTextContent("asks to unlock Admin mode first");
      r.unmount();

      // The same palette over the simulator's snapshot.
      const { snapshotAt } = await import("../dev/mockBackend");
      resetStore();
      extendMock({ get_snapshot: () => ({ ...snapshotAt(0), demo: true }) });
      await startClusterStore();
      await show(<CommandPalette />);
      act(() => openPanel("palette"));
      palette = await screen.findByRole("dialog", { name: "Command palette" });
      expect(palette.textContent).toContain("Demo scenario: Node offline");
    } finally {
      off();
    }
  });
});
