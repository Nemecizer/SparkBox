import { describe, expect, it } from "vitest";
import { statsLine } from "./stats";

describe("statsLine", () => {
  it("shows a speed only when the backend measured one", () => {
    expect(statsLine({ ttftMs: 420, totalMs: 1900, inputTokens: 12, outputTokens: 70, tokensPerSec: 46.3 }, "end_turn"))
      .toBe("0.4 s to first token · 46.3 tok/s · 70 tokens · 1.9 s total · 12 in · finished");
    // A burst: no rate from the backend, so no rate on screen.
    expect(statsLine({ ttftMs: 2000, totalMs: 2010, inputTokens: null, outputTokens: 70, tokensPerSec: null }, "stop"))
      .toBe("2.0 s to first token · 70 tokens in 2.0 s · finished");
  });

  it("says why a reply ended", () => {
    expect(statsLine({ ttftMs: null, totalMs: 300, inputTokens: null, outputTokens: null, tokensPerSec: null }, "max_tokens")).toBe("0.3 s · hit the length limit");
    expect(statsLine(null, "stopped")).toBe("stopped by you");
  });
});
