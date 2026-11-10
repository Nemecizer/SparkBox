// One word for a node's link (P1.10): an SSH timeout reads Offline in the sidebar, the status
// bar and Alerts; Error only for a node that answered and turned SparkBox away.
import { describe, expect, it } from "vitest";
import { nodeGlyphWord, nodeText, notConfirmed } from "../components/StatusBar";
import { linkWord } from "./nodeStatus";
import type { MacReach, NodeSnapshot } from "./types";

const node = (patch: Partial<NodeSnapshot>): NodeSnapshot => ({ id: "cell1", name: "cell1", host: "192.168.50.51", status: "online", nets: [], ...patch }) as NodeSnapshot;

describe("linkWord", () => {
  it("reads Offline for a node SparkBox can't reach, retrying included", () => {
    for (const errorKind of ["timeout", "unreachable", "refused", "dns"] as const) {
      for (const status of ["error", "offline", "connecting"] as const) {
        expect(linkWord(node({ status, errorKind, error: "No SSH reply in 10 s" }))).toBe("Offline");
      }
    }
    // An older backend gives only the text.
    expect(linkWord(node({ status: "error", error: "SSH timeout: no reply from 192.168.50.51 in 10 s." }))).toBe("Offline");
  });

  it("keeps Error for a host key or a login, and Connecting for a first try", () => {
    for (const errorKind of ["hostKeyUnknown", "hostKeyChanged", "authFailed"] as const) {
      expect(linkWord(node({ status: "connecting", errorKind, error: "x" }))).toBe("Error");
      expect(linkWord(node({ status: "error", errorKind, error: "x" }))).toBe("Error");
    }
    expect(linkWord(node({ status: "connecting" }))).toBe("Connecting");
    expect(linkWord(node({ status: "online" }))).toBe("Online");
  });

  it("is what the status bar and its glyph say", () => {
    const timedOut = node({ status: "error", errorKind: "timeout", error: "No SSH reply in 10 s" });
    expect(nodeText(timedOut)).toBe("Offline");
    expect(nodeGlyphWord(timedOut, "offline")).toBe("Offline");
    expect(nodeText(node({ status: "error", errorKind: "authFailed", error: "rejected" }))).toBe("Error");
    expect(nodeGlyphWord(node({ status: "online" }), "ok")).toBe("Online");
  });

  it("says a node isn't confirmed down while this Mac can't reach the cluster", () => {
    const reach: MacReach = { state: "lanBetween", sinceMs: 0, title: "This Mac can't reach any node; the LAN between them?", detail: "" };
    const timedOut = node({ status: "error", errorKind: "timeout", error: "No SSH reply in 10 s" });
    expect(notConfirmed(timedOut, { macReach: reach })).toBe("not confirmed down: this Mac can't reach any node; the LAN between them?");
    expect(notConfirmed(timedOut, { macReach: { ...reach, state: "ok" } })).toBeNull();
    expect(notConfirmed(node({ status: "online" }), { macReach: reach })).toBeNull();
  });
});
