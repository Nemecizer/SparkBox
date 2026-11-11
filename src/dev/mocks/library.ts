import type { LibraryEntry, LibraryReport } from "../../lib/modelsApi";
import { snapshotAt } from "../mockBackend";

/** Browser-only synthetic observations. No saved app inventory or node access. */
export function demoLibraryReport(entries: LibraryEntry[], scenario = new URLSearchParams(location.search).get("library")): LibraryReport {
  const report: LibraryReport = { entries: structuredClone(entries), nodes: snapshotAt(Date.now()).nodes.map((n) => ({
    node: n.id, state: n.status === "online" ? "ready" : "offline", checkedAtMs: n.status === "online" ? Date.now() : null, error: null,
  })) };
  if (scenario === "scan-failure") {
    const entry = report.entries.find((e) => Object.keys(e.copies).length);
    if (entry) {
      const copy = Object.values(entry.copies)[0];
      entry.copies.cell2 = { ...copy };
      entry.copies.cell3 = { ...copy };
    }
    report.nodes = report.nodes.map((n) => n.node === "cell2" ? { ...n, state: "error", checkedAtMs: Date.now() - 300_000, error: "Demo: scan permission denied. Existing files were kept." }
      : n.node === "cell3" ? { ...n, state: "offline", checkedAtMs: Date.now() - 300_000 }
      : n.node === "gx10-0000" ? { ...n, state: "offline", checkedAtMs: null } : n);
    report.entries.forEach((e) => { delete e.copies["gx10-0000"]; });
  }
  if (scenario === "progress") {
    const key = "/home/demo/models/example/library-progress";
    report.entries.push({ key, root: "/home/demo/models", repo: "example/library-progress", revision: "demo", format: "safetensors", sizeBytes: 100, inUse: null,
      copies: {
        cell1: { path: key, sizeBytes: 37, verified: false, partial: "downloading", progress: { step: "downloading", bytesDone: 37, bytesTotal: 100 } },
        cell2: { path: key, sizeBytes: 100, verified: false, partial: "copying", progress: { step: "verifying", bytesDone: 100, bytesTotal: 100 } },
        cell3: { path: key, sizeBytes: 60, verified: false, partial: "stopped" },
      } });
  }
  if (scenario === "missing-cache") {
    const key = "/home/demo/.cache/huggingface/hub/models--example--damaged/snapshots/" + "a".repeat(40);
    report.entries.push({ key, root: "/home/demo/.cache/huggingface/hub", repo: "example/damaged", revision: "", format: "safetensors", sizeBytes: 1024, inUse: null,
      copies: { cell2: { path: key, sizeBytes: 1024, verified: false, partial: "stopped", missingCacheFiles: 2 } } });
  }
  return report;
}
