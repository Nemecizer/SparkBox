// What Admin mode's own messages say. The flows behind them are in safety.test.ts.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mockTauri } from "../test/tauri";
import { _resetAdminForTests, actionWords, adminState, LOOK_ONLY_TEXT } from "./admin";
import { dismissToast, getToasts } from "./toasts";

beforeEach(() => mockTauri({ admin_state: () => adminState() }));
afterEach(() => {
  vi.useRealTimers();
  getToasts().forEach((t) => dismissToast(t.id));
  _resetAdminForTests();
});

describe("the warning before Admin relocks", () => {
  it("says what really happens to open shells", () => {
    vi.useFakeTimers();
    _resetAdminForTests({ unlocked: true, expiresAtMs: Date.now() + 30_000, idleMinutes: 15, requiresAuth: true });
    adminState();
    vi.advanceTimersByTime(1100);
    expect(getToasts().map((t) => [t.title, t.body])).toEqual([
      ["Admin mode locks in 1 min", "With Live data, open shells stop taking keystrokes when it locks. They stay connected."],
    ]);
  });
});

describe("the words", () => {
  it("look-only names the one thing that still changes", () => {
    expect(LOOK_ONLY_TEXT).toBe("SparkBox is in look-only mode. It starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date.");
  });

  it("the wizard's checks and the cluster scan aren't called a change", () => {
    expect(actionWords("node_probe")).toBe("test a node's connection settings");
    expect(actionWords("providers_scan")).toBe("look for model servers on your nodes");
    expect(actionWords("switch_trust")).toBe("trust this switch's certificate");
  });
});
