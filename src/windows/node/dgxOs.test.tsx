// DGX OS from /etc/dgx-release (P1.22), as the collector reports it in the System tab.
// Fleet comparison coverage lives in FleetVersions.test.tsx and versions.rs.
import { act, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { baseSnapshot, defaultHandlers } from "../../dev/mockBackend";
import { MOCKS } from "../../dev/mocks";
import { resetStore, startClusterStore } from "../../lib/store";
import type { ClusterSnapshot, NodeSnapshot } from "../../lib/types";
import { mockTauri } from "../../test/tauri";
import { WindowManagerProvider } from "../../wm/WindowManager";
import { dgxRows, SystemTab } from "./SystemTab";

/** As collector.py read cell1's file (the 7.2.3 image, then OTA 7.5.0 and 7.6.0). */
const cell1 = { hostname: "cell1-example", model: "MS-C931", os: "NVIDIA DGX Spark (Ubuntu 24.04.5 LTS)", kernel: "7.0.0-1019-nvidia", driver: "580.178.04", cuda: "13.0", uptimeS: 1, dgxBuild: "7.2.3", dgxOta: "7.6.0", dgxOtaDate: "Sun Oct  4 11:41:51 EDT 2026" };

const text = (rows: ReturnType<typeof dgxRows>) => rows.map(([k, v]) => `${k}: ${render(<>{v}</>).container.textContent}`);

describe("DGX OS", () => {
  it("shows the build and the last OTA with its date", () => {
    expect(text(dgxRows(cell1, false))).toEqual(["DGX OS build: 7.2.3", "DGX OS OTA: 7.6.0 · installed Sun Oct 4 11:41:51 EDT 2026"]);
    expect(text(dgxRows({ ...cell1, dgxOtaDate: null }, false))[1]).toBe("DGX OS OTA: 7.6.0");
  });

  it("says what's missing in Live and in Demo", () => {
    const none = { ...cell1, dgxBuild: undefined, dgxOta: undefined, dgxOtaDate: undefined };
    expect(text(dgxRows(none, false))).toEqual(["DGX OS build: Not reported", "DGX OS OTA: Not reported"]);
    expect(text(dgxRows(none, true))).toEqual(["DGX OS build: Available with Live data", "DGX OS OTA: Available with Live data"]);
  });

  it("is in the System tab's Software rows, Live and Demo", async () => {
    for (const demo of [false, true]) {
      const snap = structuredClone(baseSnapshot) as ClusterSnapshot;
      snap.demo = demo;
      const node = { ...snap.nodes[0], system: demo ? { ...snap.nodes[0].system, dgxBuild: null, dgxOta: null } : { ...snap.nodes[0].system, ...cell1 } } as NodeSnapshot;
      mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => structuredClone(snap) }));
      resetStore();
      await startClusterStore();
      let container!: HTMLElement;
      await act(async () => {
        container = render(<WindowManagerProvider sizeOf={() => [1180, 760]} isKnown={() => true}><SystemTab node={node} /></WindowManagerProvider>).container;
      });
      const row = (k: string) => [...container.querySelectorAll(".kv > div")].find((d) => d.querySelector("dt")?.textContent === k)?.querySelector("dd")?.textContent;
      expect([row("DGX OS build"), row("DGX OS OTA")]).toEqual(demo ? ["Available with Live data", "Available with Live data"] : ["7.2.3", "7.6.0 · installed Sun Oct 4 11:41:51 EDT 2026"]);
      container.remove();
    }
  });

});
