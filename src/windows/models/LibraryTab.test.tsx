import type { ComponentProps } from "react";
// The Library's copies, in the words of C-P: what each cell says, which nodes Copy to nodes… offers
// and which it leaves alone, and what Delete says about a folder of Hugging Face cache links.
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { baseSnapshot, defaultHandlers } from "../../dev/mockBackend";
import { MOCKS } from "../../dev/mocks";
import { _resetAdminForTests } from "../../lib/admin";
import { finishedCopy, finishedNodes, type LibraryEntry } from "../../lib/modelsApi";
import type { AdminState } from "../../lib/opsApi";
import * as session from "../../lib/session";
import { demoDiskPreflight } from "../../dev/mocks/disk";
import { resetStore, startClusterStore } from "../../lib/store";
import * as toasts from "../../lib/toasts";
import type { ClusterSnapshot } from "../../lib/types";
import { mockCalls, mockTauri } from "../../test/tauri";
import { WindowManagerProvider } from "../../wm/WindowManager";
import { deleteWords, isHfCache, LibraryTab } from "./LibraryTab";

const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const nodes = ["cell1", "cell2", "cell3", "gx10-0000"].map((id) => ({ id, name: id, online: true }));
const KEY = "/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8";
const CACHE = "/home/sparkuser/.cache/huggingface/hub/models--google--gemma-3-27b-it/snapshots/005ad3404e59d6023443cb575daa05336842228a";

type Copies = LibraryEntry["copies"];
const SIZE = 30.9 * 2 ** 30;
const copy = (over: Partial<Copies[string]> = {}): Copies[string] => ({ path: KEY, sizeBytes: SIZE, verified: false, ...over });
const entry = (copies: Copies, key = KEY, repo = "Qwen/Qwen3.8-27B-FP8"): LibraryEntry => ({
  key, inUse: null, repo, revision: "abc", format: "safetensors", sizeBytes: SIZE, root: "/home/sparkuser/models", copies,
});

async function show(library: LibraryEntry[], onRefresh = () => {}, extra: Record<string, () => unknown> = {}, options: Partial<ComponentProps<typeof LibraryTab>> = {}) {
  resetStore();
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, {
    get_snapshot: () => ({ ...(structuredClone(baseSnapshot) as ClusterSnapshot), tsMs: Date.now(), demo: false }),
    admin_state: () => UNLOCKED,
    endpoints_list: () => [],
    library_detail: () => ({ architectures: [], modelType: null, quantization: null, weightBytes: 0, kvBytesPerToken: null, arch: {} }),
    model_delete: () => undefined,
    model_share: () => [{ id: "share-1", node: "cell3" }],
  }, extra));
  _resetAdminForTests(UNLOCKED);
  await startClusterStore();
  const draw = (over: Partial<ComponentProps<typeof LibraryTab>> = {}) => (
    <WindowManagerProvider sizeOf={() => [1200, 900]} isKnown={() => true}>
      <LibraryTab library={library} error={null} scanning={false} nodes={nodes} onRefresh={onRefresh} onLoad={() => {}} onJobs={() => {}} onShowLoaded={() => {}} {...options} {...over} />
    </WindowManagerProvider>
  );
  let view!: ReturnType<typeof render>;
  await act(async () => { view = render(draw()); });
  return { rerender: (over: Partial<ComponentProps<typeof LibraryTab>>) => view.rerender(draw(over)) };
}

const select = async (name: string) => {
  await act(async () => {
    fireEvent.mouseDown(screen.getByText(name));
  });
};
const press = async (name: string) => {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
};
const cell = (text: string) => screen.getByText(text);

afterEach(() => vi.restoreAllMocks());

describe("finishedCopy", () => {
  it("is the copy when it has no partial, else nothing", () => {
    const e = entry({ cell1: copy({ verified: true }), cell2: copy({ partial: "stopped" }), cell3: copy({ partial: "copying" }), "gx10-0000": copy() });
    expect(finishedCopy(e, "cell1")).toBe(e.copies.cell1);
    expect(finishedCopy(e, "cell2")).toBeUndefined();
    expect(finishedCopy(e, "cell3")).toBeUndefined();
    expect(finishedCopy(e, "nowhere")).toBeUndefined();
    expect(finishedNodes(e)).toEqual(["cell1", "gx10-0000"]);
  });
});

describe("a Library row", () => {
  it("explains missing cache files and blocks Load and use as a copy source", async () => {
    const onLoad = vi.fn();
    await show([entry({ cell2: copy({ path: CACHE, sizeBytes: 1024, partial: "stopped", missingCacheFiles: 2 }) }, CACHE, "example/cache")], undefined, {}, { onLoad });
    expect(cell("◐ incomplete (2 missing)").title).toContain("Size counts only files still present");
    await select("example/cache");
    expect(screen.getByText(/2 missing cache files\. 1\.0 KiB in files still present on this node/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Load…" })).toBeDisabled();
    await act(async () => { fireEvent.doubleClick(screen.getAllByText("example/cache")[0]); });
    expect(onLoad).not.toHaveBeenCalled();
    await press("Copy to nodes…");
    expect(screen.getByRole("button", { name: "Copy over the fabric" })).toBeDisabled();
    expect(mockCalls.some((c) => c.cmd === "model_share")).toBe(false);
  });
  it("says what each copy is, with C-P's tooltips", async () => {
    await show([
      entry({
        cell1: copy({ verified: true, check: "sha256" }),
        cell2: copy({ verified: true, check: "sizes" }),
        cell3: copy({ partial: "stopped" }),
        "gx10-0000": copy({ partial: "copying" }),
      }),
      entry({ cell1: copy({ partial: "downloading" }), cell2: copy() }, "/home/sparkuser/models/org/other", "org/other"),
      // A record from before `checked` existed.
      entry({ cell1: copy({ verified: true }) }, "/home/sparkuser/models/org/older", "org/older"),
    ]);
    expect(cell("✓ sizes match").title).toBe("Copied over the fabric: every file's size matches the source. Checksums weren't compared.");
    expect(cell("◐ incomplete").title).toBe("A download or copy stopped partway. Copy to nodes… picks up where it stopped, and a new download should too.");
    expect(cell("◐ copying").title).toBe("Still running. Follow it in Jobs.");
    expect(cell("◐ downloading").title).toBe("Still running. Follow it in Jobs.");
    expect(cell("● found").title).toBe("Found on disk; SparkBox didn't download it, so it hasn't checked it");
    const verified = screen.getAllByText("✓ verified");
    expect(verified).toHaveLength(2);
    for (const v of verified) expect(v.title).toBe("Checked against Hugging Face's sizes and checksums");
  });
});

describe("Copy to nodes…", () => {
  it.each(["low", "cancel"])("starts no copy when one target is short or the review is declined: %s", async (mode) => {
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    vi.spyOn(session, "alertInfo").mockResolvedValue(undefined);
    const targets = ["cell2", "cell3", "gx10-0000"];
    const checks = demoDiskPreflight({ kind: "copy", key: KEY, targets });
    if (mode === "low") checks[1].ok = false;
    await show([entry({ cell1: copy({ verified: true }) })], undefined, { model_disk_preflight: () => checks });
    await select("Qwen/Qwen3.8-27B-FP8"); await press("Copy to nodes…"); await press("Copy over the fabric");
    expect(mockCalls.find((c) => c.cmd === "model_disk_preflight")?.args).toEqual({ request: { kind: "copy", key: KEY, targets } });
    expect(confirm).toHaveBeenCalledTimes(mode === "low" ? 0 : 1);
    expect(mockCalls.some((c) => c.cmd === "model_share")).toBe(false);
  });

  it("starts with a stopped copy selected, and leaves a finished or running one alone", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await show([entry({ cell1: copy({ verified: true }), cell2: copy({ partial: "stopped" }), cell3: copy({ partial: "copying" }) })]);
    await select("Qwen/Qwen3.8-27B-FP8");
    await press("Copy to nodes…");
    const box = (name: string) => screen.getByRole("checkbox", { name: new RegExp(`^${name}`) }) as HTMLInputElement;
    expect((box("cell1").closest("label") as HTMLElement).textContent).toBe("cell1 · has it");
    expect(box("cell1")).toMatchObject({ disabled: true, checked: false });
    expect((box("cell2").closest("label") as HTMLElement).textContent).toBe("cell2 · incomplete");
    expect(box("cell2")).toMatchObject({ disabled: false, checked: true });
    expect((box("cell3").closest("label") as HTMLElement).textContent).toBe("cell3 · copying");
    expect(box("cell3")).toMatchObject({ disabled: true, checked: false });
    expect(box("gx10-0000")).toMatchObject({ disabled: false, checked: true });
    await press("Copy over the fabric");
    expect(mockCalls.find((c) => c.cmd === "model_share")?.args).toMatchObject({ key: KEY, targets: ["cell2", "gx10-0000"] });
  });

  it("reads a running download the same way", async () => {
    await show([entry({ cell1: copy({ partial: "downloading" }) })]);
    await select("Qwen/Qwen3.8-27B-FP8");
    await press("Copy to nodes…");
    const label = screen.getByRole("checkbox", { name: /^cell1/ }).closest("label") as HTMLElement;
    expect(label.textContent).toBe("cell1 · downloading");
    expect(within(label).getByRole("checkbox")).toHaveProperty("disabled", true);
  });
});

describe("Delete", () => {
  const body = (key: string) => `This removes the folder of links at ${key}. It frees almost no disk space: the model's 30.9 GiB stay in the Hugging Face cache's blobs folder, and the model leaves this list. Help's "Delete a model" shows how to free the space.`;

  it("knows a Hugging Face cache folder by its path", () => {
    expect(isHfCache(CACHE)).toBe(true);
    expect(isHfCache(KEY)).toBe(false);
    expect(isHfCache("/home/sparkuser/models/models--org--name")).toBe(false);
  });

  it("says a cache folder's links go and its bytes stay", async () => {
    const e = entry({ cell2: copy({ path: CACHE }) }, CACHE, "google/gemma-3-27b-it");
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const toast = vi.spyOn(toasts, "toast");
    const refresh = vi.fn();
    await show([e], refresh);
    await select("google/gemma-3-27b-it");
    await press("Delete…");
    await act(async () => {
      fireEvent.click(screen.getByRole("checkbox", { name: /^cell2/ }));
    });
    const strip = screen.getByText("Delete from:").parentElement as HTMLElement;
    await act(async () => {
      fireEvent.click(within(strip).getByRole("button", { name: "Delete…" }));
    });
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({
      title: "Remove google/gemma-3-27b-it's links from cell2?",
      body: body(CACHE),
      confirmLabel: "Remove links",
      danger: true,
    }));
    expect(mockCalls.find((c) => c.cmd === "model_delete")?.args).toMatchObject({ key: CACHE, nodes: ["cell2"] });
    expect(toast).toHaveBeenCalledWith(expect.objectContaining({ title: "Removed the links to google/gemma-3-27b-it from cell2", body: "Its 30.9 GiB are still on disk." }));
    expect(refresh).toHaveBeenCalled();
  });

  it("keeps its words for a folder of real files", () => {
    const words = deleteWords(entry({ cell1: copy() }), ["cell1", "cell2"]);
    expect(words).toEqual({
      title: "Delete Qwen/Qwen3.8-27B-FP8 from cell1, cell2?",
      body: `This removes ${KEY} (30.9 GiB) from those nodes. You'd need to download or copy it again to use it there.`,
      confirmLabel: "Delete",
      doneTitle: "Deleted Qwen/Qwen3.8-27B-FP8 from cell1, cell2",
      doneBody: undefined,
    });
    expect(deleteWords(entry({ cell1: copy() }), ["cell1"]).body).toContain("from that node.");
  });

  it("leaves a copy that's still running alone", async () => {
    await show([entry({ cell1: copy({ partial: "copying" }), cell2: copy({ partial: "stopped" }) })]);
    await select("Qwen/Qwen3.8-27B-FP8");
    await press("Delete…");
    expect(screen.getByRole("checkbox", { name: /^cell1/ })).toHaveProperty("disabled", true);
    expect(screen.getByRole("checkbox", { name: /^cell2/ })).toHaveProperty("disabled", false);
  });
});


describe("per-node scan reliability", () => {
  it("shows last-known and unknown copies without claiming they are ready or absent", async () => {
    const onLoad = vi.fn();
    await show([entry({ cell1: copy({ verified: true, stale: true }) })], undefined, {}, {
      onLoad, statuses: nodes.map((n) => ({ node: n.id, state: "offline", checkedAtMs: n.id === "cell1" ? 1000 : null, error: null })),
    });
    expect(screen.getByText("◷ last known")).toBeVisible();
    expect(screen.getAllByText("unknown")).toHaveLength(3);
    expect(screen.queryByText("✓ verified")).toBeNull();
    await select("Qwen/Qwen3.8-27B-FP8");
    expect(screen.getByRole("button", { name: "Load…" })).toBeDisabled();
    await act(async () => { fireEvent.doubleClick(screen.getAllByText("Qwen/Qwen3.8-27B-FP8")[0]); });
    expect(onLoad).not.toHaveBeenCalled();
    await press("Copy to nodes…");
    expect(screen.getByRole("button", { name: "Copy over the fabric" })).toBeDisabled();
    expect(screen.getAllByRole("checkbox").every((box) => (box as HTMLInputElement).disabled)).toBe(true);
  });
  it("does not select failed or offline targets and requires fresh scans after connectivity changes", async () => {
    const lib = [entry({ cell1: copy({ verified: true }), cell2: copy({ verified: true, stale: true }) })];
    const statuses = nodes.map((n) => ({ node: n.id, state: n.id === "cell2" ? "error" as const : "ready" as const, checkedAtMs: 1000, error: n.id === "cell2" ? "fixture denied" : null }));
    const view = await show(lib, undefined, {}, { statuses });
    await select("Qwen/Qwen3.8-27B-FP8"); await press("Copy to nodes…");
    expect(screen.getByRole("checkbox", { name: /^cell2/ })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: /^cell3/ })).toBeChecked();
    await act(async () => view.rerender({ nodes: nodes.map((n) => ({ ...n, online: n.id !== "cell3" })) }));
    expect(screen.getByRole("button", { name: "Copy over the fabric" })).toBeDisabled();
    // The choice is never silently dropped; the owner can explicitly remove it.
    expect(screen.getByRole("checkbox", { name: /^cell3/ })).toBeChecked();
    fireEvent.click(screen.getByRole("checkbox", { name: /^cell3/ }));
    expect(screen.getByRole("button", { name: "Copy over the fabric" })).toBeEnabled();
  });
  it("shows percentages and verification separately, with no finished copy at 100 percent", async () => {
    await show([entry({
      cell1: copy({ partial: "downloading", progress: { step: "downloading", bytesDone: 37, bytesTotal: 100 } }),
      cell2: copy({ partial: "copying", progress: { step: "verifying", bytesDone: 100, bytesTotal: 100 } }),
      cell3: copy({ partial: "copying", progress: { step: "waiting", bytesDone: 0, bytesTotal: 100 } }),
    })]);
    expect(screen.getByText("◐ downloading 37%")).toBeVisible();
    expect(screen.getByText("◐ verifying")).toBeVisible(); expect(screen.getByText("◐ waiting")).toBeVisible();
    await select("Qwen/Qwen3.8-27B-FP8"); expect(screen.getByRole("button", { name: "Load…" })).toBeDisabled();
  });
  it.each(["copy", "revision"])("invalidates deletion consent after %s changes even if a fresh scan restores identical values", async (field) => {
    let accept!: (value: boolean) => void;
    vi.spyOn(session, "confirmAction").mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    const toast = vi.spyOn(toasts, "toast");
    const view = await show([entry({ cell1: copy({ verified: true }) })]);
    await select("Qwen/Qwen3.8-27B-FP8"); await press("Delete…");
    fireEvent.click(screen.getByRole("checkbox", { name: /^cell1/ }));
    await act(async () => { fireEvent.click(screen.getAllByRole("button", { name: "Delete…" })[1]); });
    const changed = entry({ cell1: copy({ verified: true, stale: field === "copy" ? true : undefined }) });
    if (field === "revision") changed.revision = "newer-revision";
    await act(async () => view.rerender({ library: [changed] }));
    await act(async () => view.rerender({ library: [entry({ cell1: copy({ verified: true }) })] }));
    await act(async () => { accept(true); });
    expect(mockCalls.some((c) => c.cmd === "model_delete")).toBe(false);
    expect(toast).toHaveBeenCalledWith(expect.objectContaining({ body: expect.stringContaining("Library changed during review") }));
  });
  it("uses an honest empty state when a node has never been scanned", async () => {
    await show([], undefined, {}, { statuses: [{ node: "cell1", state: "offline", checkedAtMs: null, error: null }] });
    expect(screen.getByText(/No confirmed models to show/)).toBeVisible();
    expect(screen.queryByText(/^No models found/)).toBeNull();
  });
});
