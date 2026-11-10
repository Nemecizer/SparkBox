import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import cases from "../../../src-tauri/src/inventory/fixtures/validation.json";
import { baseSnapshot, inventoryFixture } from "../../dev/mockBackend";
import { fabricErrors, inventoryRequiresReconnect, reportedFabric } from "../../lib/fabricInventory";
import type { NodeSnapshot } from "../../lib/types";
import { FabricSection } from "./FabricSection";
import { validateInventory } from "./NodesSection";

function Editor({ snapshots = baseSnapshot.nodes }: { snapshots?: NodeSnapshot[] }) {
  const [nodes, setNodes] = useState(structuredClone(inventoryFixture.nodes));
  return <>
    <FabricSection nodes={nodes} snapshots={snapshots} editNode={(i, patch) => setNodes((all) => all.map((n, j) => i === j ? { ...n, ...patch } : n))} />
    <button onClick={() => setNodes(structuredClone(inventoryFixture.nodes))}>Revert</button>
  </>;
}

describe("owner-confirmed fabric", () => {
  it.each(cases)("validates shared case: $name", ({ fabric, valid }) => {
    expect(Object.keys(fabricErrors(fabric)).length === 0).toBe(valid);
    const inventory = structuredClone(inventoryFixture);
    inventory.nodes[0].fabric = fabric;
    expect(Object.keys(validateInventory(inventory)).length === 0).toBe(valid);
  });

  it("copies reported facts only on request and supports edits, roles, clear and revert", () => {
    render(<Editor />);
    expect(screen.queryByLabelText("cell1 fabric 1 Interface")).toBeNull();
    fireEvent.change(screen.getByLabelText("Load role"), { target: { value: "worker" } });
    fireEvent.click(screen.getByRole("button", { name: "Use reported fabric" }));
    expect(screen.getByLabelText("Load role")).toHaveValue("worker");
    expect(screen.getByLabelText("cell1 fabric 1 Interface")).toHaveValue(reportedFabric(baseSnapshot.nodes[0])!.filter((l) => l.ip)[0].iface);
    fireEvent.change(screen.getByLabelText("cell1 fabric 1 Fabric IPv4"), { target: { value: "invalid" } });
    expect(screen.getByLabelText("cell1 fabric 1 Fabric IPv4")).toHaveAttribute("aria-invalid", "true");
    fireEvent.click(screen.getByRole("button", { name: "Revert" }));
    expect(screen.queryByLabelText("cell1 fabric 1 Interface")).toBeNull();
    expect(screen.getByLabelText("Load role")).toHaveValue("none");
    fireEvent.click(screen.getByRole("button", { name: "Use reported fabric" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove fabric interface 1" }));
    expect(screen.queryByLabelText("cell1 fabric 2 Interface")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Clear expectations" }));
    expect(screen.queryByLabelText("cell1 fabric 1 Interface")).toBeNull();
  });

  it.each(["stale", "offline", "absent", "no fabric", "bad IP"])("never seeds %s observations", (kind) => {
    const snapshots = structuredClone(baseSnapshot.nodes);
    if (kind === "stale") snapshots[0].stale = true;
    if (kind === "offline") snapshots[0].status = "offline";
    if (kind === "absent") snapshots.splice(0, 1);
    if (kind === "no fabric") snapshots[0].nets = [];
    if (kind === "bad IP") snapshots[0].nets.filter((n) => n.role === "fabric").forEach((n) => { n.ip = null; });
    render(<Editor snapshots={snapshots} />);
    expect(screen.getByRole("button", { name: "Use reported fabric" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Add interface" }));
    expect(screen.getByLabelText("cell1 fabric 1 Fabric IPv4")).toHaveValue("");
    expect(screen.getByLabelText("cell1 fabric 1 Fabric IPv4")).toHaveAttribute("aria-invalid", "true");
  });

  it("keeps each node's draft separate and reconnects only for preexisting inventory fields", () => {
    render(<Editor />);
    fireEvent.change(screen.getByLabelText("Load role"), { target: { value: "head" } });
    fireEvent.change(screen.getByLabelText("Fabric node"), { target: { value: "gx10-0000" } });
    expect(screen.getByLabelText("Load role")).toHaveValue("none");
    fireEvent.change(screen.getByLabelText("Fabric node"), { target: { value: "cell1" } });
    expect(screen.getByLabelText("Load role")).toHaveValue("head");
    const next = structuredClone(inventoryFixture);
    next.nodes[0].fabric = { links: reportedFabric(baseSnapshot.nodes[0])!, headCandidate: true };
    expect(inventoryRequiresReconnect(next, inventoryFixture)).toBe(false);
    next.nodes[0].ip = "10.0.0.99";
    expect(inventoryRequiresReconnect(next, inventoryFixture)).toBe(true);
  });
});
