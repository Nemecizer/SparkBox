// Scenario node-offline on the Dashboard (§2.2, C8, C9, C12): one incident counted, cell3 with
// the loud offline glyph and its state across the empty metric cells, cell1 and cell2 quiet
// with the model's note, and the lines that don't count (pending, recovering, known issues).
import { act, render, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { baseSnapshot, defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import type { Alert, HealthState } from "../lib/health";
import { resetStore, startClusterStore } from "../lib/store";
import type { ClusterSnapshot } from "../lib/types";
import { Dashboard, nodeState } from "../windows/Dashboard";
import type { Endpoint } from "../lib/endpoints";
import { WindowManagerProvider } from "../wm/WindowManager";
import { mockTauri } from "./tauri";

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

const NOW = Date.now();

function snapshot(): ClusterSnapshot {
  const s = structuredClone(baseSnapshot);
  s.tsMs = NOW;
  const n = s.nodes.find((x) => x.id === "cell3")!;
  n.status = "offline";
  n.error = "SSH timeout: no reply from cell3.local:22 in 15 s";
  n.lastSeenMs = NOW - 35_000;
  return s;
}

function alert(p: Partial<Alert> & { id: string; rule: string; kind: Alert["entity"]["kind"]; node: string; severity: Alert["severity"] }): Alert {
  const { kind, node, ...rest } = p;
  return {
    category: "node", state: "firing", entity: { kind, id: kind === "node" ? node : p.id, node, switch: null, label: node },
    message: p.id, summary: p.id, detail: null, value: null, sinceMs: NOW - 40_000, firingSinceMs: NOW - 30_000, resolvedMs: null, ackedMs: null, mutedUntilMs: null,
    ...rest,
  };
}

const alerts: Alert[] = [
  alert({ id: "node-offline:node:cell3", rule: "node-offline", kind: "node", node: "cell3", severity: "crit", summary: "cell3 offline", incidentId: "node:cell3" }),
  alert({ id: "endpoint-down:cell1-vllm-8000", rule: "endpoint-down", kind: "endpoint", node: "cell1", severity: "crit", summary: "llama-3.3-70b is down", incidentId: "node:cell3" }),
  alert({ id: "disk-full:gx10", rule: "disk-full", kind: "disk", node: "gx10-0000", severity: "warn", summary: "gx10-0000 disk 85%", silenceId: "s1", incidentId: "disk:gx10" }),
  alert({ id: "unit-failed:cell1", rule: "unit-failed", kind: "service", node: "cell1", severity: "warn", summary: "nvidia-dgx-telemetry failed on cell1", ackedMs: NOW - 5000, incidentId: "service:cell1" }),
];

const health: HealthState = {
  tsMs: NOW,
  alerts,
  summary: {
    warn: 0, crit: 1, pending: 1, muted: 2, recovering: 0, planned: 0, alerts: 2,
    known: [
      { alertId: "disk-full:gx10", incidentId: "disk:gx10", title: "gx10-0000 disk 85%", severity: "warn", why: "silenced", untilMs: null, reason: "expanding disk Friday" },
      { alertId: "unit-failed:cell1", incidentId: "service:cell1", title: "nvidia-dgx-telemetry failed on cell1", severity: "warn", why: "acknowledged", untilMs: null, reason: null },
    ],
    pendingAlerts: [{ alertId: "gpu-temp:cell2", title: "cell2 GPU 86 °C", severity: "warn", sinceMs: NOW - 20_000, firesAtMs: NOW + 40_000 }],
  },
  nodes: {},
  switches: {},
  lastAlertMs: NOW,
  incidents: [{
    id: "node:cell3", title: "cell3 offline", severity: "crit", state: "firing", children: ["node-offline:node:cell3", "endpoint-down:cell1-vllm-8000"],
    cause: null, sinceMs: NOW - 40_000, affects: "cell3", headline: "cell3 offline", counted: true, recoversAtMs: null, planned: null,
  }],
};

const llama = {
  id: "cell1-vllm-8000", engine: "vllm", models: ["llama-3.3-70b"], nodes: ["cell1"], expectedNodes: ["cell1", "cell2", "cell3"], headNode: "cell1", port: 8000,
  url: "http://cell1:8000/v1", managedBy: "external", health: "down", healthReason: "Pipeline stage 3 of 3 on cell3 is offline", containerIds: [],
};

describe("node-offline on the Dashboard", () => {
  const backend = () => mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, {
    get_snapshot: () => snapshot(),
    health_get: () => health,
    endpoints_list: () => [llama],
  }));
  beforeAll(async () => {
    resetStore();
    backend();
    await startClusterStore();
  });
  beforeEach(backend);

  it("counts one incident, keeps the healthy members quiet with the model's note, and shows the quiet lines", async () => {
    let container!: HTMLElement;
    await act(async () => {
      container = render(<WindowManagerProvider sizeOf={() => [1180, 760]} isKnown={() => true}><Dashboard /></WindowManagerProvider>).container;
    });
    for (let i = 0; i < 20 && !container.querySelector(".mon-issues"); i++) await act(async () => new Promise((r) => setTimeout(r, 50)));
    await act(async () => new Promise((r) => setTimeout(r, 100)));

    const box = container.querySelector(".mon-issues")!;
    expect(box.classList.contains("state-crit")).toBe(true);
    expect(box.querySelector(".state-word")?.textContent).toBe("1 critical");
    const lines = [...box.querySelectorAll(".mon-issue-text")].map((e) => e.textContent);
    expect(lines[0]).toMatch(/^cell3 offline · for \d+s$/);
    expect(lines).toContain("cell2 GPU 86 °C · confirming…");
    expect(within(box as HTMLElement).getByText("2 known issues")).toBeTruthy();
    // The incident's line carries the loud offline glyph; its two alerts are in the tooltip.
    const first = box.querySelector(".mon-issue")!;
    expect(first.querySelector(".sglyph-offline")).not.toBeNull();
    expect(first.getAttribute("title")).toMatch(/2 related alerts/);

    const row = (id: string) => container.querySelector(`tr[data-key="${id}"]:not(.row-detail)`) as HTMLTableRowElement;
    // cell3: offline, first, red rule, the state spans its empty metric cells, the error on line 2.
    expect(row("cell3").classList.contains("row-off")).toBe(true);
    expect(row("cell3").querySelector(".sglyph-offline")).not.toBeNull();
    const status = row("cell3").querySelector("td.cell-message") as HTMLTableCellElement;
    expect(status.colSpan).toBeGreaterThan(1);
    // One clock and one span format for the band and the row: time since the last reply.
    expect(status.textContent).toMatch(/^Offline · 3\ds · No SSH reply$/);
    const detail = container.querySelector('tr.row-detail[data-key="cell3"]') as HTMLElement;
    expect(detail.querySelector(".mon-sentence")?.textContent).toMatch(/^SSH timeout/);
    // The model's note is its own chip on line 2, not glued to the sentence.
    expect(detail.querySelector(".mon-note-chip")?.textContent).toBe("Serving llama-3.3-70b · down");
    // cell1 and cell2: neutral rows with the note, the state word in its tone; never painted.
    for (const id of ["cell1", "cell2"]) {
      const r = row(id);
      expect(r.className).not.toMatch(/row-(crit|off|warn)/);
      expect(r.querySelector(".sglyph-affected")).not.toBeNull();
      expect(r.querySelector(".mon-note")?.textContent).toBe("Serving llama-3.3-70b · down");
      expect(r.querySelector(".mon-note .tone-text-crit")?.textContent).toBe("down");
    }
    // gx10-0000's disk warning is silenced: a known issue, no amber on its row, its disk muted.
    expect(row("gx10-0000").className).not.toMatch(/row-warn/);
    // The rows sort problems first.
    const order = [...container.querySelectorAll("tbody tr:not(.row-detail)")].map((r) => r.getAttribute("data-key"));
    expect(order[0]).toBe("cell3");
  });

  it("keeps the model's note on a member that has a problem of its own", () => {
    const snap = snapshot();
    const unit = { ...alerts[3], ackedMs: null, message: "nvidia-dgx-telemetry.service failed" };
    const st = nodeState(snap.nodes.find((n) => n.id === "cell1")!, { ...health, alerts: [unit] }, snap, [llama as unknown as Endpoint], NOW);
    expect(st.short).toBe("1 unit failed");
    expect(st.tone).toBe("warn");
    // Line 2 is the message only; the time is a muted suffix and the model's note its own chip.
    expect(st.sentence).toBe("nvidia-dgx-telemetry.service failed");
    expect(st.suffix).toMatch(/^for \d+s$/);
    expect(st.note?.text).toBe("Serving llama-3.3-70b · down");
  });
});

describe("the Dashboard at rest (C3)", () => {
  // The cluster as found on Oct 29: nothing serving, two idle Ollamas, glm53 failed on 4 nodes two days ago.
  const ollama = (node: string) => ({
    id: `${node}-ollama-11434`, engine: "ollama", models: [], nodes: [node], headNode: node, port: 11434, url: `http://${node}:11434`,
    managedBy: "external", health: "idle", healthReason: "Idle · nothing in memory", containerIds: [], reach: "node-only", onDisk: node === "cell3" ? ["gemma4"] : [],
  });
  const glm = {
    id: "glm53", label: "GLM-5.3-Flash", served: "glm-5.3-flash", engine: "vllm", image: "vllm/vllm-openai:glm53-flash-arm64-cu130", model: null,
    nodes: ["cell1", "cell2", "cell3", "gx10-0000"], state: "failed", inferred: true, how: "same container name on 4 nodes",
    members: ["cell1", "cell2", "cell3", "gx10-0000"].map((node, rank) => ({ node, container: "glm53", id: `glm53-${node}`, rank, role: rank ? "worker" : "head", state: "exited", exitCode: 1, oom: false, startedMs: NOW - 2 * 86_400_000, finishedMs: NOW - 2 * 86_400_000 + 300_000 })),
    startedMs: NOW - 2 * 86_400_000, finishedMs: NOW - 2 * 86_400_000 + 300_000, ranS: 300, exitCode: 1, postmortem: null, demo: false,
  };
  it("says nothing is serving, shows the failed deployment and the idle servers, and no endpoint card", async () => {
    // Fresh stores, so they read this world rather than the scenario above.
    vi.resetModules();
    const { mockTauri: mock } = await import("./tauri");
    const { defaultHandlers: handlers } = await import("../dev/mockBackend");
    const { MOCKS: mocks } = await import("../dev/mocks");
    const store = await import("../lib/store");
    const { Dashboard: Dash } = await import("../windows/Dashboard");
    const { WindowManagerProvider: WM } = await import("../wm/WindowManager");
    mock(Object.assign({}, handlers(), ...mocks, {
      get_snapshot: () => ({ ...structuredClone(baseSnapshot), tsMs: NOW }),
      health_get: () => ({ ...health, alerts: [], incidents: [], summary: { warn: 0, crit: 0, pending: 0, muted: 0 } }),
      endpoints_list: () => [ollama("cell1"), ollama("cell3")],
      cluster_deployments_list: () => [glm],
    }));
    store.resetStore();
    await store.startClusterStore();
    let container!: HTMLElement;
    await act(async () => {
      container = render(<WM sizeOf={() => [1180, 760]} isKnown={() => true}><Dash /></WM>).container;
    });
    for (let i = 0; i < 20 && !container.querySelector(".mon-serving-none"); i++) await act(async () => new Promise((r) => setTimeout(r, 50)));
    await act(async () => new Promise((r) => setTimeout(r, 150)));
    const serving = container.querySelector('[aria-label="Serving"]') as HTMLElement;
    expect(within(serving).getByText("Nothing is serving.")).toBeTruthy();
    // No card for a server with nothing in memory.
    expect(serving.querySelector(".mon-card.state-crit, .ep-card")).toBeNull();
    expect(serving.textContent).toMatch(/Ollama on cell1 and cell3/);
    expect(serving.textContent).toMatch(/idle, nothing in memory/);
    // The failed run, neutral, with its Failed pill.
    expect(serving.textContent).toMatch(/GLM-5.3-Flash/);
    expect(within(serving).getByText("Failed")).toBeTruthy();
    // Idle servers put no note on their nodes.
    expect(container.textContent).not.toMatch(/Serving cell\d-ollama/);
    // Idle CPUs and GPUs draw no trend that would read as an underline; the numbers keep their place.
    const { RowSpark } = await import("../windows/Dashboard");
    const idle = render(<RowSpark values={[1, 1, 2, 1, 0.5, 1]} color="var(--m-cpu)" />).container;
    expect(idle.querySelector(".mon-spark-blank")).not.toBeNull();
    expect(idle.querySelector("svg")).toBeNull();
    const busy = render(<RowSpark values={[1, 30, 64, 60, 12, 3]} color="var(--m-gpu)" />).container;
    expect(busy.querySelector("svg")).not.toBeNull();
  });
});
