import { afterEach, describe, expect, it } from "vitest";
import type { HfSizing } from "../../lib/modelsApi";
import { _inFlight, _setFetcher, AT_ONCE, noteSize, requestSize, sizeOf } from "./hfSizes";

const sizing = (id: string, weightBytes: number): HfSizing => ({ id, weightBytes, weightsFormat: "safetensors", minNodes: 1, mlx: false, gguf: false });

describe("Hugging Face list sizes", () => {
  afterEach(() => _setFetcher(() => Promise.reject(new Error("unset"))));

  it("reads at most four repos at a time and keeps each answer", async () => {
    const pending = new Map<string, (s: HfSizing) => void>();
    let peak = 0;
    _setFetcher((repo) => new Promise((resolve) => {
      pending.set(repo, resolve);
      peak = Math.max(peak, _inFlight());
    }));
    for (let i = 0; i < 10; i++) requestSize(`org/m${i}`);
    expect(pending.size).toBe(AT_ONCE);
    for (let round = 0; round < 3; round++) {
      for (const [repo, resolve] of [...pending]) {
        pending.delete(repo);
        resolve(sizing(repo, 1e9));
      }
      await new Promise((r) => setTimeout(r, 0));
    }
    expect(peak).toBe(AT_ONCE);
    expect(sizeOf("org/m9")?.sizing?.weightBytes).toBe(1e9);
    // Asked again: the kept answer, no new request.
    requestSize("org/m3");
    expect(pending.size).toBe(0);
  });

  it("shows the detail's number once the detail has read the files", () => {
    _setFetcher(() => new Promise(() => {}));
    noteSize(sizing("mlx-community/gpt-oss-120b-4bit", 65_770_000_000));
    expect(sizeOf("mlx-community/gpt-oss-120b-4bit")?.sizing?.weightBytes).toBe(65_770_000_000);
  });

  it("remembers a failure so the row can fall back to a marked estimate", async () => {
    _setFetcher(() => Promise.reject(new Error("Hugging Face is rate-limiting requests; try again in a minute.")));
    requestSize("org/gated");
    await new Promise((r) => setTimeout(r, 0));
    expect(sizeOf("org/gated")?.error).toMatch(/rate-limiting/);
  });
});
