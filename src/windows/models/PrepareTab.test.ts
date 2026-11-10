// What Prepare says about the vLLM image: a load starts what's on the node and fetches nothing,
// and SparkBox adds nothing to it. The fabric halves and the GPU seen from inside the image come
// from the node's own check.
import { afterEach, describe, expect, it } from "vitest";
import { fabricState, gpuWord, imageWord, loadChecks } from "./PrepareTab";

describe("the image column", () => {
  it("says what a load will find", () => {
    expect(imageWord({ imagePresent: true })).toBe("on this node");
    expect(imageWord({ imagePresent: false })).toBe("not on this node; a load won't fetch it");
  });
});

describe("the fabric halves", () => {
  it("reads MTU and RDMA state as sysfs writes them, and says when it has neither", () => {
    expect(fabricState({ mtu: 9000, rdmaState: "4: ACTIVE" })).toBe("MTU 9000 · RDMA ACTIVE");
    expect(fabricState({ mtu: 1500, rdmaState: "1: DOWN" })).toBe("MTU 1500 · RDMA DOWN");
    expect(fabricState({ mtu: 0, rdmaState: "" })).toBe("MTU ? · RDMA unknown");
  });
});

describe("the GPU seen from inside the image", () => {
  it("tells no image, no GPU and a GPU apart", () => {
    expect(gpuWord({ gpuInContainer: null, imagePresent: false })).toEqual({ ok: null, text: "not checked: the image isn't on this node" });
    expect(gpuWord({ gpuInContainer: null, imagePresent: true })).toEqual({ ok: null, text: "not checked" });
    expect(gpuWord({ gpuInContainer: "", imagePresent: true })).toEqual({ ok: false, text: "a container sees no GPU" });
    expect(gpuWord({ gpuInContainer: "GPU 0: NVIDIA Tegra NVIDIA GB10B (UUID: GPU-1)", imagePresent: true })).toEqual({ ok: true, text: "GPU 0: NVIDIA Tegra NVIDIA GB10B (UUID: GPU-1)" });
  });
});

describe("the saved result", () => {
  afterEach(() => localStorage.clear());

  it("comes back with its time, and a broken one reads as none", () => {
    expect(loadChecks()).toBeNull();
    localStorage.setItem("sparkbox.prepare.v1:demo", JSON.stringify({ atMs: 1234, checks: [{ node: "cell1" }] }));
    localStorage.setItem("sparkbox.prepare.v1:live", JSON.stringify({ atMs: 1234, checks: [{ node: "cell1" }] }));
    expect(loadChecks()).toMatchObject({ atMs: 1234, checks: [{ node: "cell1" }] });
    localStorage.setItem("sparkbox.prepare.v1:demo", "{");
    localStorage.setItem("sparkbox.prepare.v1:live", "{");
    expect(loadChecks()).toBeNull();
  });
});
