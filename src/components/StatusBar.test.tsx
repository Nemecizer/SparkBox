// The status bar while SparkBox keeps this Mac awake for an orchestration (P1.12), and the node
// chips' tooltips without the VPN window's connect time (P1.7).
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { baseSnapshot } from "../dev/mockBackend";
import { _resetAwakeForTests, AWAKE_EVENT, awakeLine, type AwakeHold } from "../lib/awake";
import { resetStore, startClusterStore } from "../lib/store";
import { emitMock, mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { StatusBar } from "./StatusBar";

const hold = (what: string, task: string | null = null): AwakeHold => ({ what, sinceMs: 1_000, task });

async function show() {
  resetStore();
  mockTauri({
    get_snapshot: () => structuredClone(baseSnapshot),
    // What the VPN window's table holds: it isn't quoted in the node chips any more.
    vpn_get: () => ({ settings: { mode: "auto", profile: null, forwards: [] }, status: { state: "direct", forwards: [] }, log: [], reach: [{ id: "cell1", name: "cell1", addr: "192.168.50.51:22", via: "direct", ok: true, rttMs: 142, error: null, checkedMs: 0 }], rates: [] }),
  });
  _resetAwakeForTests();
  await startClusterStore();
  await act(async () => {
    render(<WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}><StatusBar /></WindowManagerProvider>);
  });
  // The awake store starts listening when the bar mounts.
  await act(async () => new Promise((r) => setTimeout(r, 0)));
}

async function send(holds: AwakeHold[]) {
  await act(async () => {
    await emitMock(AWAKE_EVENT, { holds });
  });
}

afterEach(() => {
  vi.useRealTimers();
});

describe("keeping this Mac awake (P1.12)", () => {
  it("words one, two or more holds", () => {
    expect(awakeLine([])).toBeNull();
    expect(awakeLine([hold("Restart cell2")])).toBe("Keeping this Mac awake while Restart cell2 runs");
    expect(awakeLine([hold("Restart cell2"), hold("Fleet: sudo reboot")])).toBe("Keeping this Mac awake while Restart cell2 and Fleet: sudo reboot run");
    expect(awakeLine([hold("Restart cell2"), hold("Restart cell3"), hold("Fleet: uptime")])).toBe("Keeping this Mac awake while Restart cell2 and 2 more run");
  });

  it("shows the line while a guard is held and drops it when the last one goes", async () => {
    await show();
    expect(screen.queryByRole("button", { name: /Keeping this Mac awake/ })).toBeNull();
    await send([hold("Restart cell2", "t1")]);
    const chip = screen.getByRole("button", { name: "Keeping this Mac awake while Restart cell2 runs" });
    expect(chip.textContent).toBe("Keeping this Mac awake while Restart cell2 runs");
    expect(chip.title).toContain("macOS still sleeps if you choose Sleep");
    // It opens the task it's for.
    const opened: unknown[] = [];
    const onOpen = (e: Event) => opened.push((e as CustomEvent).detail);
    window.addEventListener("sparkbox:open-window", onOpen);
    fireEvent.click(chip);
    window.removeEventListener("sparkbox:open-window", onOpen);
    expect(opened).toEqual([{ kind: "tasks", params: {} }]);
    await send([]);
    expect(screen.queryByRole("button", { name: /Keeping this Mac awake/ })).toBeNull();
  });

  it("drops the line when the app stops saying so (a missed last event)", async () => {
    await show();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await send([hold("Fleet: apt-get -y upgrade")]);
    expect(screen.getByRole("button", { name: /while Fleet: apt-get -y upgrade runs/ })).toBeInTheDocument();
    // The heartbeat every 5 s keeps it.
    await act(async () => vi.advanceTimersByTime(10_000));
    await send([hold("Fleet: apt-get -y upgrade")]);
    await act(async () => vi.advanceTimersByTime(10_000));
    expect(screen.getByRole("button", { name: /while Fleet: apt-get -y upgrade runs/ })).toBeInTheDocument();
    await act(async () => vi.advanceTimersByTime(6_000));
    expect(screen.queryByRole("button", { name: /Keeping this Mac awake/ })).toBeNull();
  });
});

describe("node chips (P1.7)", () => {
  it("quote SparkBox's own round trip and no connect time", async () => {
    await show();
    const cell1 = screen.getByRole("button", { name: /^cell1 — / });
    expect(cell1.title).toMatch(/^cell1 — Online · 10 ms: round trip of SparkBox's last command over SSH\.$/m);
    expect(cell1.title).not.toMatch(/Connecting takes|142/);
  });
});
