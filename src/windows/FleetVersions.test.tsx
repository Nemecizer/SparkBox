// The shared Rust report is the only source of verdicts; old/partial data stays visible.
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { baseSnapshot } from "../dev/mockBackend";
import { copyText } from "../lib/markdown";
import type { VersionReport } from "../lib/types";
import { VersionComparison, versionTable } from "../components/VersionComparison";
import { Versions } from "./FleetCommand";

vi.mock("../lib/markdown", async (importOriginal) => ({ ...await importOriginal<typeof import("../lib/markdown")>(), copyText: vi.fn() }));
const nodes = baseSnapshot.nodes;
const row = (label: string) => screen.getByText(label, { exact: true }).closest("tr")!;
const sample = (): VersionReport => structuredClone(baseSnapshot.versions!);

describe("Versions", () => {
  it("never claims agreement while an older backend has no comparison", () => {
    render(<Versions nodes={nodes} />);
    expect(screen.getByRole("status")).toHaveTextContent("Version consistency: Unknown");
    expect(screen.queryByRole("table")).toBeNull();
  });
  it("keeps unknown rows visible, including in Demo", () => {
    render(<Versions nodes={nodes} report={sample()} demo />);
    expect(screen.getByText(/Demo readings are simulated/)).toBeVisible();
    expect(within(row("DGX OS (OTA)")).getAllByText("Unknown")).toHaveLength(5);
    expect(within(row("Kernel")).getByText("Matching")).toBeVisible();
    expect(screen.getByRole("status")).toHaveTextContent("Unknown");
    expect(screen.queryByText(/Every node runs the same/)).toBeNull();
  });
  it("shows a two-node disagreement without inventing a majority or claiming all match", () => {
    const report: VersionReport = { state: "mismatched", rows: [{ key: "driver", label: "NVIDIA driver", state: "mismatched", note: null,
      values: [{ node: nodes[1].id, value: "580.1", reason: null }, { node: nodes[0].id, value: "580.2", reason: null }] }] };
    render(<Versions nodes={nodes.slice(0, 2)} report={report} />);
    const cells = within(row("NVIDIA driver")).getAllByRole("cell");
    expect(cells.slice(2).map((c) => c.textContent)).toEqual(["580.2", "580.1"]);
    expect(screen.getByRole("status")).toHaveTextContent("Mismatched");
    expect(screen.queryByText(/All reported values match/)).toBeNull();
  });
  it("copies full IDs, unknown reasons and comparability notes with the displayed names", () => {
    const id = `sha256:${"a".repeat(64)}`;
    const report: VersionReport = { state: "unknown", rows: [{ key: "imageId", label: "Serving image ID", state: "unknown", note: "Stores are not comparable.",
      values: [{ node: nodes[0].id, value: id, reason: null }, { node: nodes[1].id, value: null, reason: "Stale readings" }] }] };
    const named = nodes.slice(0, 2).map((n, i) => ({ ...n, name: `Renamed ${i + 1}` }));
    render(<Versions nodes={named} report={report} />);
    expect(screen.getByTitle(id)).toHaveTextContent(`${id.slice(0, 19)}…`);
    expect(screen.getByText("Stale readings")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Copy as table" }));
    expect(copyText).toHaveBeenCalledWith(versionTable(report, named));
    const copied = versionTable(report, named);
    expect(copied).toContain(id); expect(copied).toContain("Unknown: Stale readings");
    expect(copied).toContain("Stores are not comparable."); expect(copied).toContain("Renamed 2");
  });
  it("preflight exposes the selected-image scope and expands the same report", () => {
    render(<VersionComparison report={sample()} nodes={nodes} load />);
    expect(screen.getByText(/Selected image; NCCL is its declared version/)).toHaveTextContent("advisory");
    expect(screen.getByText("Compare version details").closest("details")).not.toHaveAttribute("open");
    fireEvent.click(screen.getByText("Compare version details"));
    expect(screen.getByRole("table", { name: "Versions by node" })).toBeVisible();
  });
});
