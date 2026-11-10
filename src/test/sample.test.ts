import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { describe, expect, it } from "vitest";
import { fmtBits, fmtBytes, fmtDuration } from "../lib/format";
import { emitMock, mockCalls, mockTauri } from "./tauri";

describe("test infrastructure", () => {
  it("formats like the rest of the app", () => {
    expect(fmtBytes(128 * 1024 ** 3)).toBe("128 GiB");
    expect(fmtBits(19_100_000_000)).toBe("19.1 Gb/s");
    expect(fmtDuration(3 * 3600 + 120)).toBe("3h 2m");
  });

  it("routes invoke to mock handlers and records the call", async () => {
    mockTauri({ get_inventory: () => ({ nodes: [], switches: [] }) });
    await expect(invoke("get_inventory")).resolves.toEqual({ nodes: [], switches: [] });
    expect(mockCalls.map((c) => c.cmd)).toEqual(["get_inventory"]);
    await expect(invoke("nope")).rejects.toThrow(/No mock for "nope"/);
  });

  it("delivers mocked events to listen()", async () => {
    mockTauri({});
    const got: unknown[] = [];
    await listen<number>("cluster://tick", (e) => got.push(e.payload));
    await emitMock("cluster://tick", 7);
    expect(got).toEqual([7]);
  });
});
