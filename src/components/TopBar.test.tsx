import { describe, expect, it, vi } from "vitest";
import type { AgentStatus, WatchState } from "../lib/agent";
import { chipTone } from "./StatusBar";
import { countdown, modelsServing, scenarioBadgeText, topSummary } from "./TopBar";

// Admin mode is open, so the badge's actions run straight away.
vi.mock("../lib/admin", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/admin")>()),
  confirmUnlock: vi.fn(async () => true),
  requestAdmin: vi.fn(async () => true),
  withAdmin: vi.fn(async (fn: () => Promise<unknown>) => fn()),
}));

const agent = (node: string, beatAgoMs: number | null): AgentStatus => ({
  node, name: node, installed: true, state: "running", version: "1", lastBeatMs: beatAgoMs === null ? null : 1_000_000 - beatAgoMs,
  historyFromMs: null, firing: 0, lastDeliveryMs: null, lastDeliveryOk: null, uptimeS: null, metricsPort: null, error: null, simulated: true,
});
const watch = (agents: AgentStatus[], mode: WatchState["mode"]): WatchState => ({ mode, agents, deliveryOwner: agents.length ? "agents" : "mac" });

describe("top bar", () => {
  it("counts Admin time down from the expiry it was given, never a stale first value", () => {
    expect(countdown(1_000_000 + 442_000, 1_000_000)).toBe("7:22");
    expect(countdown(1_000_000 + 59_400, 1_000_000)).toBe("0:59");
    expect(countdown(1_000_000 - 5000, 1_000_000)).toBe("0:00");
    expect(countdown(1_000_000 + 3_599_000, 1_000_000)).toBe("59:59");
    expect(countdown(1_000_000 + 3_600_000, 1_000_000)).toBe("1:00:00");
    expect(countdown(1_000_000 + 86_400_000, 1_000_000)).toBe("24:00:00");
    expect(countdown(1_000_000 + 86_399_000, 1_000_000)).toBe("23:59:59");
  });

  it("in look-only the pill says what the mode does and doesn't promise more", async () => {
    const { act, render, screen } = await import("@testing-library/react");
    const { _resetAdminForTests } = await import("../lib/admin");
    const { mockTauri } = await import("../test/tauri");
    const { WindowManagerProvider } = await import("../wm/WindowManager");
    const { TopBar } = await import("./TopBar");
    mockTauri({ get_snapshot: () => null });
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: true });
    await act(async () => {
      render(<WindowManagerProvider sizeOf={() => [800, 600]} isKnown={() => true}><TopBar /></WindowManagerProvider>);
    });
    expect(screen.getByRole("button", { name: "Look only" })).toHaveAttribute("title", "Look only: SparkBox starts no change on the cluster or this Mac in this run. Click for why.");
    _resetAdminForTests();
  });

  it("clicking the look-only pill opens the sheet with the real way out, not a 'not available' toast", async () => {
    const { act, fireEvent, render, screen } = await import("@testing-library/react");
    const { _resetAdminForTests, lookOnlyTip } = await import("../lib/admin");
    const { registerCommands } = await import("../lib/commands");
    const { getToasts } = await import("../lib/toasts");
    const { mockTauri } = await import("../test/tauri");
    const { WindowManagerProvider } = await import("../wm/WindowManager");
    const { ConfirmHost } = await import("./ConfirmHost");
    const { TopBar } = await import("./TopBar");
    mockTauri({ get_snapshot: () => null });
    const locked = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true };
    _resetAdminForTests({ ...locked, lookOnly: true, lookOnlyForced: true });
    // App's padlock command: in look-only it refuses, with a toast.
    const unlock = vi.fn();
    const off = registerCommands([{ id: "cluster:admin", title: "Unlock Admin mode…", group: "Cluster", when: () => lookOnlyTip(), run: unlock }]);
    try {
      await act(async () => {
        render(<WindowManagerProvider sizeOf={() => [800, 600]} isKnown={() => true}><TopBar /><ConfirmHost /></WindowManagerProvider>);
      });
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Look only" }));
      });
      const sheet = screen.getByRole("alertdialog");
      expect(sheet).toHaveAccessibleName("Look only");
      expect(sheet).toHaveTextContent("SPARKBOX_LOOK_ONLY=1 turned it on for this run: quit SparkBox and start it without it.");
      expect(sheet).not.toHaveTextContent("Settings");
      expect(getToasts().filter((t) => /not available/.test(t.title))).toEqual([]);
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "OK" }));
      });
      // With look-only off, the padlock is the unlock command again, and no sheet opens.
      unlock.mockClear();
      off();
      registerCommands([{ id: "cluster:admin", title: "Unlock Admin mode…", group: "Cluster", run: unlock }]);
      await act(async () => {
        _resetAdminForTests(locked);
      });
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Read-only" }));
      });
      expect(unlock).toHaveBeenCalledTimes(1);
      expect(screen.queryByRole("alertdialog")).toBeNull();
    } finally {
      _resetAdminForTests();
    }
  });

  it("the padlock command's refusal names the way out that works in this run", async () => {
    const { _resetAdminForTests, LOOK_ONLY_TIP, lookOnlyTip } = await import("../lib/admin");
    const locked = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: true };
    _resetAdminForTests(locked);
    expect(lookOnlyTip()).toBe(LOOK_ONLY_TIP);
    _resetAdminForTests({ ...locked, lookOnlyForced: true });
    expect(lookOnlyTip()).toBe("Look-only · SPARKBOX_LOOK_ONLY=1 turned it on for this run: quit SparkBox and start it without it.");
    _resetAdminForTests();
  });

  it("shows a scenario's phase and when it moves on", () => {
    expect(scenarioBadgeText("Node offline", { phase: "fault", label: "Node offline · fault", nextChangeS: 42 })).toBe("Node offline · fault · recovers in 0:42");
    expect(scenarioBadgeText("Node offline", { phase: "recovering", label: "Node offline · recovering", nextChangeS: 75 })).toBe("Node offline · recovering · healthy in 1:15");
    expect(scenarioBadgeText("Disk filling", { phase: "fault", label: "Disk filling · fault", nextChangeS: null })).toBe("Disk filling · fault");
    expect(scenarioBadgeText("Disk filling", null)).toBe("Disk filling");
  });

  it("shows the watch badge with its reasons as a tooltip, and its action pushes the settings again", async () => {
    const { act, fireEvent, render, screen } = await import("@testing-library/react");
    const { baseSnapshot } = await import("../dev/mockBackend");
    const { resetStore, startClusterStore } = await import("../lib/store");
    const { applyWatchState } = await import("../lib/agent");
    const { mockCalls, mockTauri } = await import("../test/tauri");
    const { WindowManagerProvider } = await import("../wm/WindowManager");
    const { TopBar } = await import("./TopBar");
    const now = Date.now();
    const agents = ["cell1", "cell2", "cell3", "gx10-0000"].map((n) => ({ ...agent(n, 5000), lastBeatMs: now - 5000, configCurrent: n !== "cell3" }));
    const drifted: WatchState = {
      ...watch(agents, "agents"),
      channels: [{ id: "ntfy", name: "ntfy", kind: "ntfy", enabled: true, remote: true, macOnly: false }] as WatchState["channels"],
      drift: [{ node: "cell3", name: "cell3", reason: "has older settings" }],
    };
    resetStore();
    mockTauri({ get_snapshot: () => structuredClone(baseSnapshot), agent_push_config: () => ({ ...drifted, drift: [] }) });
    await startClusterStore();
    act(() => applyWatchState(drifted));
    render(<WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}><TopBar /></WindowManagerProvider>);
    const label = await screen.findByRole("button", { name: /24\/7 · settings not on cell3/ });
    expect(label.closest(".tb-watch")).toHaveClass("warn");
    expect(label).toHaveAttribute("title", expect.stringMatching(/^cell3 has old alert settings \(has older settings\)\. SparkBox pages for cell3 itself while this Mac is awake\./));
    const { getToasts } = await import("../lib/toasts");
    await act(async () => {
      // A double click is one push (pushAgain is single-flight), and the bar posts no toast of its own.
      fireEvent.click(screen.getByRole("button", { name: "Push again" }));
      fireEvent.click(screen.getByRole("button", { name: "Push again" }));
    });
    expect(mockCalls.filter((c) => c.cmd === "agent_push_config")).toHaveLength(1);
    expect(getToasts().filter((t) => /Pushed the alert settings to the agents/.test(t.title))).toHaveLength(0);
    // With nothing installed it's this Mac only, and Set up… opens Settings.
    act(() => applyWatchState(watch([], "mac-only")));
    expect(await screen.findByRole("button", { name: "This Mac only" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Set up…" })).toBeInTheDocument();
  });
});

describe("top bar summary", () => {
  const text = (s: ReturnType<typeof topSummary>) => s.parts.map((p) => p.text).join(" · ");

  it("counts models apart from the GPUs serving them, and incidents", () => {
    const s = topSummary({ online: 4, total: 4, models: ["llama-3.3-70b", "qwen3:32b"], gpus: 4, crit: 1, warn: 2 });
    expect(text(s)).toBe("4/4 online · 2 models · 4 GPUs serving · 1 critical · 2 warnings");
    expect(s.parts.find((p) => p.text === "1 critical")?.tone).toBe("crit");
    expect(s.parts.find((p) => p.text === "2 warnings")?.tone).toBe("warn");
    expect(s.tooltip).toMatch(/Serving llama-3.3-70b, qwen3:32b on 4 GPUs/);
  });

  it("puts recovering and known issues in the tooltip only", () => {
    const s = topSummary({ online: 4, total: 4, models: ["llama-3.3-70b"], gpus: 3, crit: 0, warn: 0, recovering: 1, known: 2 });
    expect(text(s)).toBe("4/4 online · 1 model · 3 GPUs serving");
    expect(s.tooltip).toMatch(/1 recovering, not counted/);
    expect(s.tooltip).toMatch(/2 known issues \(silenced or acknowledged\), not counted/);
  });

  it("says when nothing serves, and an offline node in red", () => {
    const s = topSummary({ online: 3, total: 4, models: [], gpus: 0, crit: 1, warn: 0 });
    expect(text(s)).toBe("3/4 online · no model serving · 1 critical");
    expect(s.parts[0].tone).toBe("crit");
    // A node SparkBox is restarting is planned work: counted as not online, never tinted.
    const planned = topSummary({ online: 3, total: 4, planned: 1, models: [], gpus: 0, crit: 0, warn: 1 });
    expect(text(planned)).toBe("3/4 online · no model serving · 1 warning");
    expect(planned.parts[0].tone).toBeUndefined();
    expect(planned.tooltip).toContain("3 of 4 nodes online (1 restarting or shut down by SparkBox)");
  });

  it("counts a model once however many endpoints serve it, and only Ready ones that answered lately", () => {
    const now = 1_000_000;
    const e = (models: string[], health: string, lastOkMs: number | null) => ({ models, health, nodes: ["cell1"], lastOkMs });
    expect(modelsServing([
      e(["llama-3.3-70b"], "ready", now - 5000),
      e(["llama-3.3-70b"], "ready", now - 9000),
      e(["qwen3:32b"], "down", now - 1000),
      e(["gpt-oss-120b"], "ready", now - 120_000),
    ], now)).toEqual(["llama-3.3-70b"]);
    // Ollama: what's loaded, not everything installed.
    expect(modelsServing([{ ...e(["gpt-oss:20b", "qwen3:32b", "llama3.2"], "ready", now), engine: "ollama", ollama: [{ name: "qwen3:32b" }] }], now)).toEqual(["qwen3:32b"]);
    // Other engines report an empty Ollama list: their own models count.
    expect(modelsServing([{ ...e(["llama-3.3-70b"], "ready", now), engine: "vllm", ollama: [] }], now)).toEqual(["llama-3.3-70b"]);
  });
});

describe("status bar chips", () => {
  it("keep healthy devices quiet and colour only problems", () => {
    expect(chipTone("online")).toBe("ok");
    expect(chipTone("connecting")).toBe("ok");
    expect(chipTone("unconfigured")).toBe("ok");
    expect(chipTone("warn")).toBe("warn");
    expect(chipTone("stale")).toBe("warn");
    expect(chipTone("crit")).toBe("crit");
    expect(chipTone("offline")).toBe("crit");
    expect(chipTone("error")).toBe("crit");
  });
});

describe("status bar fit", () => {
  it("drops latencies, then names, to stay on one line, and never hides a device", async () => {
    const { render, screen, act } = await import("@testing-library/react");
    const { baseSnapshot } = await import("../dev/mockBackend");
    const { resetStore, startClusterStore } = await import("../lib/store");
    const { mockTauri } = await import("../test/tauri");
    const { WindowManagerProvider } = await import("../wm/WindowManager");
    const { StatusBar } = await import("./StatusBar");
    resetStore();
    mockTauri({ get_snapshot: () => structuredClone(baseSnapshot) });
    await startClusterStore();
    // A 1000 px bar: full chips need 1500, names alone 1100, dots fit.
    const width = Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth")!;
    const scroll = Object.getOwnPropertyDescriptor(Element.prototype, "scrollWidth")!;
    Object.defineProperty(Element.prototype, "clientWidth", { configurable: true, get() { return this.classList.contains("statusbar") ? 1000 : 0; } });
    Object.defineProperty(Element.prototype, "scrollWidth", {
      configurable: true,
      get() {
        if (!this.classList.contains("statusbar")) return 0;
        return this.querySelector(".sb-dev-extra") ? 1500 : this.querySelector(".sb-dev-name") ? 1100 : 900;
      },
    });
    try {
      await act(async () => {
        render(<WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}><StatusBar /></WindowManagerProvider>);
      });
      const chips = document.querySelectorAll(".sb-dev");
      expect(chips.length).toBe(baseSnapshot.nodes.length + baseSnapshot.switches.length);
      expect(document.querySelector(".sb-dev-extra")).toBeNull();
      expect(document.querySelector(".sb-dev-name")).toBeNull();
      // Each dot still says what it is.
      expect(screen.getByRole("button", { name: /^cell1 — / })).toBeInTheDocument();
    } finally {
      Object.defineProperty(Element.prototype, "clientWidth", width);
      Object.defineProperty(Element.prototype, "scrollWidth", scroll);
    }
  });
});

describe("top bar fit (C4)", () => {
  /**
   * The bar's width as WebKit lays it out at text size 12.5 px (widths measured in the app at
   * 1440 px): what each control takes at the bar's current fit level. jsdom has no layout, so
   * the test lays it out the same way the CSS does.
   */
  function barWidth(bar: Element): number {
    const fit = Number(bar.getAttribute("data-fit") ?? 0);
    const text = (sel: string) => (bar.querySelector(sel)?.textContent ?? "").length * 6.6;
    const has = (sel: string) => !!bar.querySelector(sel);
    const lead = 84 + 26 + (has(".badge.live") ? 44 : 0) + (has(".tb-pill") ? 52 : 0) + 104;
    const summary = has(".topbar-summary") ? text(".topbar-summary") + 20 : 0;
    const watch = has(".tb-watch") ? (fit >= 2 ? 26 : text(".tb-watch-label") + 26) + (has(".tb-link") ? text(".tb-link") + 10 : 0) : 0;
    const search = fit >= 3 ? 66 : 182;
    const tools = has(".tb-fold") ? 32 : fit >= 3 ? 32 + 20 + 32 + 22 + 32 : 74 + 20 + 116 + 92;
    return lead + summary + watch + search + 9 + tools + 12;
  }

  for (const width of [560, 800, 1000, 1440]) {
    it(`keeps every control inside a ${width} px window, folding in order`, async () => {
      const { act, render } = await import("@testing-library/react");
      const { baseSnapshot } = await import("../dev/mockBackend");
      const { resetStore, startClusterStore } = await import("../lib/store");
      const { applyWatchState } = await import("../lib/agent");
      const { mockTauri } = await import("../test/tauri");
      const { WindowManagerProvider } = await import("../wm/WindowManager");
      const { TopBar } = await import("./TopBar");
      resetStore();
      mockTauri({ get_snapshot: () => structuredClone(baseSnapshot) });
      await startClusterStore();
      // A badge with an action, the longest the bar carries.
      act(() => applyWatchState({ ...watch([], "mac-only"), channels: [] } as WatchState));
      const cw = Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth")!;
      const sw = Object.getOwnPropertyDescriptor(Element.prototype, "scrollWidth")!;
      Object.defineProperty(Element.prototype, "clientWidth", { configurable: true, get() { return this.classList.contains("topbar") ? width : 0; } });
      Object.defineProperty(Element.prototype, "scrollWidth", { configurable: true, get() { return this.classList.contains("topbar") ? barWidth(this) : 0; } });
      try {
        await act(async () => {
          render(<WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}><TopBar /></WindowManagerProvider>);
        });
        const bar = document.querySelector(".topbar")!;
        const fit = Number(bar.getAttribute("data-fit") ?? 0);
        // Every control's right edge is inside the window: the laid-out bar fits.
        expect(barWidth(bar), `fit ${fit}`).toBeLessThanOrEqual(width);
        // The padlock and the counts never give way.
        expect(bar.querySelector(".admin-pill")).not.toBeNull();
        expect(bar.querySelector(".topbar-summary")?.textContent).toMatch(/online/);
        if (width >= 1440) expect(fit).toBe(0);
        if (width <= 1000) expect(bar.querySelector(".tb-link")).toBeNull();
        if (width <= 800) expect(fit).toBeGreaterThanOrEqual(2);
        if (width <= 560) {
          // Tile, Windows and Close all are one "…" menu.
          expect(bar.querySelector(".tb-fold")).not.toBeNull();
          expect(bar.querySelector(".windows-menu-btn")).toBeNull();
        }
      } finally {
        Object.defineProperty(Element.prototype, "clientWidth", cw);
        Object.defineProperty(Element.prototype, "scrollWidth", sw);
      }
    });
  }
});
