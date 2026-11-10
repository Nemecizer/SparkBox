// What the Hugging Face tab offers for a repo that's partly on the cluster (C-P): "On cell1" and
// Load need a finished copy; one left partway on the head node can be resumed, and one still
// running can't be started twice.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { baseSnapshot, defaultHandlers } from "../../dev/mockBackend";
import { MOCKS } from "../../dev/mocks";
import { _resetAdminForTests } from "../../lib/admin";
import type { LibraryEntry, ModelDetails } from "../../lib/modelsApi";
import type { AdminState } from "../../lib/opsApi";
import * as session from "../../lib/session";
import { demoDiskPreflight } from "../../dev/mocks/disk";
import { resetStore, startClusterStore } from "../../lib/store";
import type { ClusterSnapshot } from "../../lib/types";
import { emitMock, mockCalls, mockTauri } from "../../test/tauri";
import { WindowManagerProvider } from "../../wm/WindowManager";
import { copiesText, hubPresence, HubTab, libraryMatch } from "./HubTab";

const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const REPO = "Qwen/Qwen3.8-27B-FP8";
const KEY = `/home/sparkuser/models/${REPO}`;

type Copies = LibraryEntry["copies"];
const copy = (over: Partial<Copies[string]> = {}): Copies[string] => ({ path: KEY, sizeBytes: 30.9e9, verified: false, ...over });
const entry = (copies: Copies, key = KEY): LibraryEntry => ({ key, inUse: null, repo: REPO, revision: "abc", format: "safetensors", sizeBytes: 30.9e9, root: "/home/sparkuser/models", copies });

const details = {
  model: {
    id: REPO, revision: "a".repeat(40), gated: false, license: null, pipeline: null, library: null, params: null, dtypes: {}, weightBytes: 30e9, downloadBytes: 30.9e9, repoBytes: 30.9e9,
    skipped: [], weightsFormat: "safetensors", quantization: null, architectures: [], modelType: null, config: {}, visionConfig: null, files: [{ path: "model.safetensors", size: 30.9e9, sha256: "c".repeat(64) }], readmeExcerpt: "", mlx: false,
  },
  arch: {}, kvBytesPerToken: null, fit: [], fitFp8: [], nodeMemory: 128e9, nodesAvailable: 4,
} as unknown as ModelDetails;

const selection = { repo: REPO, revision: details.model.revision, files: details.model.files };

async function show(library: LibraryEntry[], onLoad = vi.fn(), extra: Record<string, (...args: any[]) => unknown> = {}, state = UNLOCKED) {
  resetStore();
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, {
    get_snapshot: () => ({ ...(structuredClone(baseSnapshot) as ClusterSnapshot), tsMs: Date.now(), demo: false }),
    admin_state: () => state,
    hf_model: () => details,
    model_download: () => ({ id: "dl-1", node: "cell1" }),
  }, extra));
  _resetAdminForTests(state);
  await startClusterStore();
  await act(async () => {
    render(
      <WindowManagerProvider sizeOf={() => [1200, 900]} isKnown={() => true}>
        <HubTab library={library} onLoad={onLoad} onDownloading={() => {}} />
      </WindowManagerProvider>,
    );
  });
  await act(async () => {
    fireEvent.click(screen.getByText(REPO));
  });
  await act(async () => new Promise((res) => setTimeout(res, 20)));
  return onLoad;
}

const button = (name: string | RegExp) => screen.queryByRole("button", { name });
const head = () => (baseSnapshot as ClusterSnapshot).nodes[0].id;

afterEach(() => vi.restoreAllMocks());

describe("reviewed revision and files", () => {
  it("passes the same full commit and manifest to disk review and start", async () => {
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await show([]);
    expect(screen.getByText(`Commit ${selection.revision}`)).toBeTruthy();
    expect(screen.getByText("Files to download (1)")).toBeTruthy();
    await act(async () => { fireEvent.click(button(`Download to ${head()}`) as HTMLElement); });
    expect(mockCalls.find((c) => c.cmd === "model_disk_preflight")?.args).toEqual({ request: { kind: "download", node: head(), selection } });
    expect(mockCalls.find((c) => c.cmd === "model_download")?.args).toMatchObject({ selection });
    const text = render(<>{confirm.mock.calls[1][0].body}</>).container.textContent;
    expect(text).toContain(selection.revision);
    expect(text).toContain("Any mismatch stops the download");
  });

  it("selects a branch snapshot by full commit and does not use main for the new download", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const commit = "b".repeat(40);
    const changed = { ...details, model: { ...details.model, revision: commit, files: [{ path: "new.safetensors", size: 123, sha256: null }] } };
    await show([], vi.fn(), {
      hf_refs: () => [{ name: "release/test", kind: "branch", revision: commit }],
      hf_model: ({ revision }) => revision === commit ? changed : details,
    });
    await act(async () => { fireEvent.change(screen.getByLabelText("Branch or tag"), { target: { value: "0" } }); });
    expect(mockCalls.filter((c) => c.cmd === "hf_model").at(-1)?.args).toEqual({ repo: REPO, revision: commit });
    expect(screen.getByText(`Commit ${commit}`)).toBeTruthy();
    await act(async () => { fireEvent.click(button(`Download to ${head()}`) as HTMLElement); });
    expect(mockCalls.find((c) => c.cmd === "model_download")?.args).toMatchObject({ selection: { repo: REPO, revision: commit, files: changed.model.files } });
  });

  it("a typed revision needs inspection and a failed inspection cannot leave an old download available", async () => {
    await show([], vi.fn(), { hf_model: ({ revision }) => { if (revision) throw new Error("Revision missing (HTTP 404)"); return details; } });
    await act(async () => { fireEvent.change(screen.getByLabelText("Revision"), { target: { value: "missing" } }); });
    expect((button(`Download to ${head()}`) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => { fireEvent.click(button("Inspect revision") as HTMLElement); });
    expect(screen.getByText("Revision missing (HTTP 404)")).toBeTruthy();
    expect(button(`Download to ${head()}`)).toBeNull();
    expect(screen.queryByText(`Commit ${selection.revision}`)).toBeNull();
  });

  it.each([401, 403, 404])("shows config HTTP %s instead of a blank fit table and blocks download and AI", async (status) => {
    const configError = `config.json (HTTP ${status}): review access to this revision.`;
    await show([], vi.fn(), { hf_model: () => ({ ...details, model: { ...details.model, configError } }) });
    expect(screen.getByText(configError)).toBeTruthy();
    expect(screen.queryByText(/Fit on 4/)).toBeNull();
    expect((button(`Download to ${head()}`) as HTMLButtonElement).disabled).toBe(true);
    expect((button("Check with AI") as HTMLButtonElement).disabled).toBe(true);
    expect(mockCalls.some((c) => c.cmd === "model_download_tools")).toBe(false);
  });

  it("reports a branch-list failure but still allows an exact revision inspection", async () => {
    await show([], vi.fn(), { hf_refs: () => { throw new Error("Access refused"); } });
    expect(screen.getByText(/Branches and tags: Access refused/)).toBeTruthy();
    await act(async () => { fireEvent.change(screen.getByLabelText("Revision"), { target: { value: selection.revision } }); });
    await act(async () => { fireEvent.click(button("Inspect revision") as HTMLElement); });
    expect(mockCalls.filter((c) => c.cmd === "hf_model").at(-1)?.args).toEqual({ repo: REPO, revision: selection.revision });
  });

  it("ignores a late response for a previously selected repository", async () => {
    let finish: (value: ModelDetails) => void = () => {};
    let first = true;
    await show([], vi.fn(), { hf_model: ({ repo }) => {
      if (first) { first = false; return new Promise<ModelDetails>((resolve) => { finish = resolve; }); }
      return { ...details, model: { ...details.model, id: repo, revision: "b".repeat(40) } };
    } });
    await act(async () => { fireEvent.click(screen.getByText("openai/gpt-oss-120b")); });
    await act(async () => { finish(details); });
    expect(screen.getByText(`Commit ${"b".repeat(40)}`)).toBeTruthy();
    expect(screen.queryByText(`Commit ${selection.revision}`)).toBeNull();
  });

  it("a rejected manifest starts nothing and retains the reviewed details for a deliberate refresh", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await show([], vi.fn(), { model_disk_preflight: () => { throw new Error("The reviewed revision or file list no longer matches Hugging Face."); } });
    await act(async () => { fireEvent.click(button(`Download to ${head()}`) as HTMLElement); });
    expect(screen.getByText(/no longer matches Hugging Face/)).toBeTruthy();
    expect(mockCalls.some((c) => c.cmd === "model_download")).toBe(false);
    expect(screen.getByText(`Commit ${selection.revision}`)).toBeTruthy();
  });

  it("does not apply advice for the inspected revision to a different saved Library copy", async () => {
    const advice = { analysis: { recommended: { nodes: 1, maxModelLen: 8192, gpuMemoryUtilization: 0.75 }, verdict: "good", summary: "Fixture", engineSupport: {}, alternatives: [], risks: [], betterVariant: {} }, notApplied: [], allowedArgs: [] };
    const onLoad = await show([entry({ cell1: copy() })], vi.fn(), { models_analyze: () => advice });
    await act(async () => { fireEvent.click(button("Check with AI") as HTMLElement); });
    expect(mockCalls.find((c) => c.cmd === "models_analyze")?.args).toEqual({ repo: REPO, revision: selection.revision });
    expect(screen.getByText(/Load uses the saved Library copy at revision abc/)).toBeTruthy();
    await act(async () => { fireEvent.click(button("Load…") as HTMLElement); });
    expect(onLoad).toHaveBeenCalledWith(expect.objectContaining({ key: KEY, rec: undefined, allowed: undefined }));
  });
});

describe("hubPresence", () => {
  it("reads only the selected download node", () => {
    expect(hubPresence(undefined, "cell1")).toEqual({ kind: "none" });
    expect(hubPresence(entry({ cell1: copy({ verified: true }) }), "cell1")).toEqual({ kind: "finished" });
    expect(hubPresence(entry({ cell1: copy() }), "cell1")).toEqual({ kind: "finished" });
    expect(hubPresence(entry({ cell1: copy({ partial: "stopped" }), cell2: copy({ verified: true }) }), "cell1")).toEqual({ kind: "incomplete", node: "cell1" });
    expect(hubPresence(entry({ cell1: copy({ verified: true }) }), "cell2")).toEqual({ kind: "none" });
    expect(hubPresence(entry({ cell1: copy({ partial: "stopped" }) }), "cell1")).toEqual({ kind: "incomplete", node: "cell1" });
    expect(hubPresence(entry({ cell1: copy({ partial: "downloading" }) }), "cell1")).toEqual({ kind: "running", node: "cell1", partial: "downloading" });
    // Left partway on another node, with nothing finished: a download to the head is what's left.
    expect(hubPresence(entry({ cell2: copy({ partial: "stopped" }) }), "cell1")).toEqual({ kind: "none" });
  });

  it("keeps matching a folder, and prefers a finished one", () => {
    const partial = entry({ cell1: copy({ partial: "stopped" }) });
    const whole = entry({ cell2: copy({ verified: true }) }, "/home/sparkuser/.cache/huggingface/hub/models--Qwen--Qwen3.8-27B-FP8/snapshots/abc");
    expect(libraryMatch([partial], REPO)).toBe(partial);
    expect(libraryMatch([partial, whole], REPO)).toBe(whole);
    expect(libraryMatch([partial, whole], REPO, "cell1")).toBe(partial);
    expect(libraryMatch([partial, whole], REPO, "cell2")).toBe(whole);
  });

  it("counts only finished copies in where it says the model is", () => {
    expect(copiesText(entry({ cell1: copy({ verified: true }), cell2: copy({ partial: "stopped" }), cell3: copy({ partial: "copying" }) }), 4)).toBe(`On cell1 (~/models/${REPO})`);
  });
});

describe("download target", () => {
  const picker = () => screen.getByRole("combobox", { name: "Download node" }) as HTMLSelectElement;
  const choose = async (node: string) => act(async () => { fireEvent.change(picker(), { target: { value: node } }); });
  const snapshot = () => ({ ...structuredClone(baseSnapshot), tsMs: Date.now() + 1000, demo: false }) as ClusterSnapshot;

  it("sends the selected stable ID through tool discovery and consent while displaying its name", async () => {
    const s = snapshot(); const fourth = s.nodes[3]; fourth.name = "Office Spark";
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await show([], vi.fn(), { get_snapshot: () => s });
    await choose(fourth.id);
    await act(async () => { fireEvent.click(button("Download to Office Spark") as HTMLElement); });
    expect(mockCalls.find((c) => c.cmd === "model_download_tools")?.args).toEqual({ node: fourth.id });
    expect(mockCalls.find((c) => c.cmd === "model_download")?.args).toMatchObject({ selection, review: { node: fourth.id, installConsent: false } });
    expect(confirm.mock.calls[1][0].title).toBe(`Download ${REPO} to Office Spark?`);
  });

  it("keeps Load available while a different selected node is incomplete or empty", async () => {
    const partial = entry({ cell2: copy({ partial: "stopped" }) }, "/other/partial");
    await show([entry({ cell1: copy({ verified: true }) }), partial]);
    await choose("cell2");
    expect(screen.getByText("Incomplete on cell2")).toBeTruthy();
    expect(button("Download again to cell2")).toBeTruthy();
    expect((button("Load…") as HTMLButtonElement).disabled).toBe(false);
    await choose("cell3");
    expect(button("Download to cell3")).toBeTruthy();
    expect(screen.queryByText("Incomplete on cell2")).toBeNull();
  });

  it("does not offer a second download into a selected node's running copy", async () => {
    await show([entry({ cell1: copy(), cell2: copy({ partial: "downloading" }) })]);
    await choose("cell2");
    expect(screen.getByText("Downloading on cell2")).toBeTruthy();
    expect(button("Download to cell2")).toBeNull();
    expect((button("Load…") as HTMLButtonElement).disabled).toBe(false);
  });

  it("keeps the chosen ID on reorder, disconnection and removal instead of picking another node", async () => {
    await show([]); await choose("cell2");
    const s = snapshot(); s.nodes.reverse();
    await act(async () => { await emitMock("cluster://snapshot", s); });
    expect(picker().value).toBe("cell2");
    s.nodes.find((n) => n.id === "cell2")!.status = "offline"; s.tsMs++;
    await act(async () => { await emitMock("cluster://snapshot", structuredClone(s)); });
    expect(picker().value).toBe("cell2");
    expect((button("Download to cell2") as HTMLButtonElement).disabled).toBe(true);
    s.nodes = s.nodes.filter((n) => n.id !== "cell2"); s.tsMs++;
    await act(async () => { await emitMock("cluster://snapshot", structuredClone(s)); });
    expect(picker().value).toBe("cell2");
    expect(screen.getByRole("option", { name: "cell2 — removed from inventory" })).toBeTruthy();
    expect((button("Download to selected node") as HTMLButtonElement).disabled).toBe(true);
    expect(mockCalls.some((c) => c.cmd === "model_download_tools" || c.cmd === "model_download")).toBe(false);
  });

  it.each([false, true])("initial selection needs a connected node; all offline: %s", async (allOffline) => {
    const s = snapshot(); s.nodes[0].status = "offline";
    if (allOffline) s.nodes.forEach((n) => { n.status = "offline"; });
    await show([], vi.fn(), { get_snapshot: () => s });
    expect(picker().value).toBe(allOffline ? "" : s.nodes[1].id);
    expect((screen.getByRole("option", { name: `${s.nodes[0].name} — not connected` }) as HTMLOptionElement).disabled).toBe(true);
    if (allOffline) expect((button("Download to selected node") as HTMLButtonElement).disabled).toBe(true);
  });

  it("changing a target and declining tool review starts nothing", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await show([]); await choose("cell3");
    expect(mockCalls.some((c) => c.cmd === "model_download_tools")).toBe(false);
    await act(async () => { fireEvent.click(button("Download to cell3") as HTMLElement); });
    expect(mockCalls.find((c) => c.cmd === "model_download_tools")?.args).toEqual({ node: "cell3" });
    expect(mockCalls.some((c) => c.cmd === "model_download")).toBe(false);
  });
});

describe("the Hugging Face tab", () => {
  it("shows where a finished copy is and offers Load", async () => {
    const onLoad = await show([entry({ [head()]: copy({ verified: true }) })]);
    expect(screen.getByText(`On ${head()} (~/models/${REPO})`)).toBeTruthy();
    expect(button(/^(Download|Resume)/)).toBeNull();
    const load = button("Load…") as HTMLButtonElement;
    expect(load.disabled).toBe(false);
    await act(async () => {
      fireEvent.click(load);
    });
    expect(onLoad).toHaveBeenCalledWith(expect.objectContaining({ key: KEY }));
  });

  it("offers a newly reviewed download for a copy left partway, and no Load", async () => {
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await show([entry({ [head()]: copy({ partial: "stopped" }) })]);
    expect(screen.getByText(`Incomplete on ${head()}`)).toBeTruthy();
    expect(screen.queryByText(new RegExp(`^On ${head()}`))).toBeNull();
    expect((button("Load…") as HTMLButtonElement).disabled).toBe(true);
    await act(async () => {
      fireEvent.click(button(`Download again to ${head()}`) as HTMLElement);
    });
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: `Download ${REPO} again to ${head()}?`, confirmLabel: "Download again" }));
    expect(render(<>{confirm.mock.calls[1][0].body}</>).container.textContent).toContain("For the original revision and files, use Resume download… in Jobs.");
    expect(mockCalls.find((c) => c.cmd === "model_download")?.args).toMatchObject({ selection });
  });

  it("says a running download is running, with no button to start another", async () => {
    await show([entry({ [head()]: copy({ partial: "downloading" }) })]);
    expect(screen.getByText(`Downloading on ${head()}`)).toBeTruthy();
    expect(button(/^(Download|Resume)\b/)).toBeNull();
    expect((button("Load…") as HTMLButtonElement).disabled).toBe(true);
  });

  it("offers a plain download when nothing is on the cluster", async () => {
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValueOnce(true).mockResolvedValue(false);
    await show([]);
    await act(async () => {
      fireEvent.click(button(`Download to ${head()}`) as HTMLElement);
    });
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: `Download ${REPO} to ${head()}?`, confirmLabel: "Download" }));
  });
});

it.each([true, false])("managed installation needs a second concrete consent: %s", async (accept) => {
  const inventory = { node: head(), options: [{ kind: "install", path: "/home/demo/.sparkbox/venv", fingerprint: "pinned-bundle", label: "Install SparkBox hf" }], notes: [] };
  const confirm = vi.spyOn(session, "confirmAction").mockResolvedValueOnce(true).mockResolvedValueOnce(accept);
  await show([], vi.fn(), { model_download_tools: () => inventory });
  await act(async () => { fireEvent.click(button(`Download to ${head()}`) as HTMLElement); });
  expect(confirm.mock.calls[1][0]).toMatchObject({ confirmLabel: "Install tool and download", danger: true });
  expect(render(<>{confirm.mock.calls[1][0].body}</>).container.textContent).toContain("Existing tool folders are not replaced");
  const start = mockCalls.find((c) => c.cmd === "model_download");
  if (accept) expect(start?.args).toEqual({ selection, review: { node: head(), tool: inventory.options[0], installConsent: true } });
  else expect(start).toBeUndefined();
});


it("continues to tool review after the normal Admin unlock without another click", async () => {
  const confirm = vi.spyOn(session, "confirmAction").mockResolvedValueOnce(true).mockResolvedValueOnce(true).mockResolvedValueOnce(false);
  await show([], vi.fn(), { admin_unlock: () => UNLOCKED }, { ...UNLOCKED, unlocked: false });
  await act(async () => { fireEvent.click(button(`Download to ${head()} — needs Admin`) as HTMLElement); });
  expect(confirm.mock.calls.map(([options]) => options.confirmLabel)).toEqual(["Unlock…", "Continue", "Download"]);
  expect(mockCalls.some((call) => call.cmd === "model_download_tools")).toBe(true);
  expect(mockCalls.some((call) => call.cmd === "model_download")).toBe(false);
});

it.each(["low", "unknown", "changed"])("refuses a %s destination without retrying or choosing another node", async (mode) => {
  const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
  const alert = vi.spyOn(session, "alertInfo").mockResolvedValue(undefined);
  const checks = demoDiskPreflight({ kind: "download", node: head(), selection });
  if (mode === "low") checks[0] = { ...checks[0], availableBytes: 1, ok: false };
  await show([], vi.fn(), {
    model_disk_preflight: () => { if (mode === "unknown") throw new Error("Cannot read destination disk"); return checks; },
    model_download: () => { throw new Error("Not enough disk space now"); },
  });
  await act(async () => { fireEvent.click(button(`Download to ${head()}`) as HTMLElement); });
  expect(mockCalls.filter((c) => c.cmd === "model_download")).toHaveLength(mode === "changed" ? 1 : 0);
  expect(mockCalls.find((c) => c.cmd === "model_disk_preflight")?.args).toEqual({ request: { kind: "download", node: head(), selection } });
  expect(confirm).toHaveBeenCalledTimes(mode === "changed" ? 2 : 1);
  if (mode === "low") expect(alert).toHaveBeenCalledWith(expect.objectContaining({ title: "Not enough destination space" }));
  else expect(screen.getByText(mode === "unknown" ? "Cannot read destination disk" : "Not enough disk space now")).toBeTruthy();
});

it("shows the full disk warning and reserve before a download can be approved", async () => {
  const checks = demoDiskPreflight({ kind: "download", node: head(), selection });
  checks[0].warning = "This disk could be 88% full after the transfer.";
  const confirm = vi.spyOn(session, "confirmAction").mockResolvedValueOnce(true).mockResolvedValueOnce(false);
  await show([], vi.fn(), { model_disk_preflight: () => checks });
  await act(async () => { fireEvent.click(button(`Download to ${head()}`) as HTMLElement); });
  const review = render(<>{confirm.mock.calls[1][0].body}</>).container.textContent;
  expect(review).toContain("88% full"); expect(review).toContain("full transfer plus 5% and 20 GiB");
  expect(review).toContain(checks[0].dest); expect(review).toContain("available"); expect(review).toContain("required");
  expect(mockCalls.some((c) => c.cmd === "model_download")).toBe(false);
});
