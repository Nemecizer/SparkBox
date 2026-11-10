// The restart confirmation's checks before the restart (P1.26, ops/prereboot.rs): with Live data
// the confirm lists them; when one blocks there is no Restart button, only the reasons, and
// node_power is never asked. Demo and a shutdown ask nothing new. The backend is mockTauri;
// dialogs are answered by stubs that record what they were asked.
import { isValidElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockCalls, mockTauri, type MockHandlers } from "../test/tauri";
import type { ConfirmOptions } from "./session";
import type { ClusterSnapshot, NodeSnapshot } from "./types";
import type { PreReboot } from "./power";

const asked: ConfirmOptions[] = [];
const notices: { title: string; body?: ReactNode }[] = [];
vi.mock("./session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./session")>()),
  confirmAction: vi.fn(async (opts: ConfirmOptions) => {
    asked.push(opts);
    return true;
  }),
  alertInfo: vi.fn(async (opts: { title: string; body?: ReactNode }) => {
    notices.push(opts);
  }),
}));

let snapshot: ClusterSnapshot | null = null;
vi.mock("./store", async (importOriginal) => ({ ...(await importOriginal<typeof import("./store")>()), getCluster: () => snapshot }));

const { _resetAdminForTests, adminState } = await import("./admin");
const { requestPower, checkItems } = await import("./power");

const node = (id: string): NodeSnapshot => ({
  id, name: id, host: `${id}.local`, status: "online", latencyMs: 1, error: null,
  cpu: { totalPct: 0, perCore: [], load: [0, 0, 0], tempC: 40 },
  mem: { total: 1, used: 0, available: 1, cached: 0, gpuAlloc: 0, swapTotal: 0, swapUsed: 0 },
  gpu: { utilPct: 0, tempC: 40, powerW: 10, smClockMhz: 0, procs: [] },
  disks: [], nets: [], system: { hostname: id, model: "", os: "", kernel: "7.0.0-1019-nvidia", driver: "580.178.04", cuda: "", uptimeS: 100 },
  containers: [], processes: [], listeners: [],
});

const cluster = (demo: boolean): ClusterSnapshot => ({ tsMs: 0, demo, switches: [], nodes: ["cell1", "cell2", "cell3", "gx10-0000"].map(node) });

const check = (id: string, label: string, state: "ok" | "fail" | "info", detail: string) => ({ id, label, state, detail });
const PASSED: PreReboot = {
  node: "cell2", kernel: "7.0.0-1019-nvidia", ok: true, blocking: [],
  checks: [
    check("kernel", "Kernel that boots next", "ok", "7.0.0-1019-nvidia (where /boot/vmlinuz points)"),
    check("initrd", "Its initrd", "ok", "/boot/initrd.img-7.0.0-1019-nvidia is there"),
    check("grub", "GRUB_DEFAULT", "info", "GRUB_DEFAULT=0 (a grub-reboot rollback needs GRUB_DEFAULT=saved)"),
  ],
};
const BLOCKED: PreReboot = {
  ...PASSED, ok: false,
  blocking: ["No initrd for 7.0.0-1019-nvidia: the September 2026 panic. Not restarted."],
  checks: [PASSED.checks[0], check("initrd", "Its initrd", "fail", "/boot/initrd.img-7.0.0-1019-nvidia is missing or empty"), PASSED.checks[2]],
};

/** A dialog's body as the user reads it. */
const text = (body: ReactNode | undefined) => (isValidElement(body) ? renderToStaticMarkup(body).replace(/<[^>]+>/g, " ") : String(body ?? ""))
  .replace(/&#x27;/g, "'").replace(/&quot;/g, '"').replace(/&amp;/g, "&").replace(/\s+/g, " ");

function backend(over: MockHandlers = {}) {
  mockTauri({
    vpn_get: () => ({ status: { state: "direct" } }),
    node_power: () => "t-power",
    node_wol_status: ({ node }: { node: string }) => ({ node, enabled: true, routes: [] }),
    node_prereboot_check: () => PASSED,
    admin_unlock: () => ({ unlocked: true, expiresAtMs: Date.now() + 900_000, idleMinutes: 15, requiresAuth: false, authenticated: false }),
    admin_state: () => adminState(),
    ...over,
  });
}

const calls = (cmd: string) => mockCalls.filter((c) => c.cmd === cmd);

beforeEach(() => {
  asked.length = 0;
  notices.length = 0;
  snapshot = cluster(false);
  _resetAdminForTests();
  backend();
});

describe("Restart… with Live data shows the checks before the restart", () => {
  it("passing: the confirm lists each check, then the restart starts", async () => {
    const id = await requestPower("cell2", "reboot");
    expect(id).toBe("t-power");
    expect(calls("node_prereboot_check").map((c) => c.args)).toEqual([{ node: "cell2" }]);
    expect(asked).toHaveLength(1);
    const body = text(asked[0].body);
    expect(body).toContain("Checks before the restart: passed");
    expect(body).toContain("Its initrd: /boot/initrd.img-7.0.0-1019-nvidia is there");
    expect(body).toContain("GRUB_DEFAULT (recorded): GRUB_DEFAULT=0");
    expect(asked[0].confirmLabel).toBe("Restart");
    expect(calls("node_power").map((c) => c.args)).toEqual([{ node: "cell2", action: "reboot", delayMin: 0 }]);
  });

  it("blocked: no Restart button, the reasons instead, and node_power is never asked", async () => {
    backend({ node_prereboot_check: () => BLOCKED });
    const id = await requestPower("cell2", "reboot");
    expect(id).toBeNull();
    expect(asked).toHaveLength(0);
    expect(notices).toHaveLength(1);
    expect(notices[0].title).toBe("cell2 can't restart now");
    const body = text(notices[0].body);
    expect(body).toContain("SparkBox won't restart cell2: No initrd for 7.0.0-1019-nvidia: the September 2026 panic.");
    expect(body).not.toContain("Not restarted");
    expect(body).toContain("Its initrd: /boot/initrd.img-7.0.0-1019-nvidia is missing or empty");
    expect(calls("node_power")).toHaveLength(0);
  });

  it("checks that couldn't run don't stop the confirm: the restart's own checks decide", async () => {
    backend({ node_prereboot_check: () => { throw { kind: "unreachable", message: "cell2 isn't connected.", node: "cell2" }; } });
    await requestPower("cell2", "reboot");
    expect(asked).toHaveLength(1);
    expect(text(asked[0].body)).toContain("The checks before the restart couldn't run: cell2 isn't connected. SparkBox runs them again before it sends the restart, and doesn't restart the node if they fail.");
    expect(calls("node_power")).toHaveLength(1);
  });

  it("the list marks what blocks", () => {
    expect(checkItems(BLOCKED).map((i) => i.tone)).toEqual(["info", "crit", "info"]);
  });
});

describe("what doesn't change", () => {
  it("Demo asks for no checks and says they need Live data", async () => {
    snapshot = cluster(true);
    await requestPower("cell2", "reboot");
    expect(calls("node_prereboot_check")).toHaveLength(0);
    expect(asked).toHaveLength(1);
    expect(text(asked[0].body)).toContain("Checks before the restart: Available with Live data");
    expect(calls("node_power")).toHaveLength(1);
  });

  it("a shutdown isn't checked", async () => {
    await requestPower("cell2", "poweroff");
    expect(calls("node_prereboot_check")).toHaveLength(0);
    expect(asked).toHaveLength(1);
  });
});
