// The Load page for a vLLM load: the image picked per load from what the chosen nodes have (by
// local id), the share sized to the model or at 0.75, and "Before it starts" from model_preflight, with the
// page-cache drop offered only when vLLM would refuse to start without it. Load is held, with
// the reason, while the image or the folder is missing. Demo reads no node and says so. LoadedTab
// shows what the last unload found, and Forget says when it unloads first. Edit & reload counts
// the load it replaces as gone: its port and its share come back at the unload. Forget is off in
// look-only, with look-only's words. Below those: the recipe row, the split, the set of nodes in
// "Before it starts", Remove leftovers, the Expert field and the words for stages and failures.
// Then what a real cluster does to the page: a node that can't be read, a read or a removal that
// fails, a tick that belongs to the nodes it named, numbers that can't be sent, and renamed nodes.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { baseSnapshot, defaultHandlers } from "../../dev/mockBackend";
import { MOCKS } from "../../dev/mocks";
import { _resetAdminForTests, actionWords, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP } from "../../lib/admin";
import { usually } from "../../lib/knownPorts";
import { inNames, type DeployPlan, type Deployment, type DeploySpec, type LibraryEntry, type NodePlan, type Preflight, type PreflightSet, type RecipeView, type TopologyOffer } from "../../lib/modelsApi";
import type { AdminState, NodeImages } from "../../lib/opsApi";
import * as session from "../../lib/session";
import { demoDiskPreflight } from "../../dev/mocks/disk";
import { resetStore, startClusterStore } from "../../lib/store";
import { ingestTask } from "../../lib/tasks";
import type { ClusterSnapshot, ModelSettings, PreviewStep } from "../../lib/types";
import { mockCalls, mockTauri, type MockHandlers } from "../../test/tauri";
import { WindowManagerProvider } from "../../wm/WindowManager";
import type { Draft } from "./HubTab";
import { BeforeItStarts, DEFAULT_IMAGE, FIRST_LOAD_SHARE, LoadPage, MIN_SHARE, defaultImage, draftKey, expertRefusal, expertText, firstOffer, imageChoices, imagePresence, offerWords, parseExpertArgs, recipeFill, shareWords, splitPart, verdictLine, wasRead, wholeProblem } from "./LoadPage";
import { failureLog, LoadedTab, stageText } from "./LoadedTab";
import { PrepareTab } from "./PrepareTab";
import { ModelsSection } from "../settings/ModelsSection";

// A page reads the nodes 250 ms after each change, and these tests make several changes.
vi.setConfig({ testTimeout: 30_000 });

const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const GIB = 2 ** 30;
const GEMMA = "/home/sparkuser/.cache/huggingface/hub/models--google--gemma-3-27b-it/snapshots/005ad3404e59d6023443cb575daa05336842228a";
const NVCR = "nvcr.io/nvidia/vllm:26.05-py3";
const ID_5A = "sha256:5a0f8b914da56ea2ae2dbe569e4219a14605ce798e7eff12ae6db63328adc4f1";

const COPY = { path: GEMMA, sizeBytes: 54_904_369_444, verified: false };
const library: LibraryEntry[] = [
  {
    key: GEMMA, inUse: null, repo: "google/gemma-3-27b-it", revision: "", format: "safetensors", sizeBytes: 54_904_369_444, root: "/home/sparkuser/.cache/huggingface/hub",
    copies: { cell2: COPY },
  },
];
/** gemma on cells 1 to 3 and the renamed fourth node, as a several-node load sees it. */
const everywhere: LibraryEntry[] = [{ ...library[0], copies: { cell1: COPY, cell2: COPY, cell3: COPY, "gx10-0000": COPY } }];

// The owner's saved image (the old default), which cell2 doesn't have.
const settings = { modelRoot: "~/models", image: NVCR, advisorProvider: "anthropic", advisorModel: "x", sglangImage: "lmsysorg/sglang:spark" } as ModelSettings;

/** cell2's images as images_list reads them (docker image ls, newest first). */
const listing = (node: string): NodeImages => ({
  node,
  images: [
    { node, id: "c0ffee123456", repository: "eugr/spark-vllm-b12x", tag: "nightly-20261015", size: 2e10, createdMs: null, dangling: false, usedBy: [], running: 0 },
    { node, id: "5a0f8b914da5", repository: "vllm/vllm-openai", tag: "glm53-flash-arm64-cu130", size: 2.2e10, createdMs: null, dangling: false, usedBy: ["glm53"], running: 0 },
    { node, id: "deadbeef0000", repository: "traefik", tag: "v3.6.1", size: 2e8, createdMs: null, dangling: false, usedBy: [], running: 0 },
  ] as NodeImages["images"],
  total: 4.3e10,
  reclaimable: null,
  error: null,
});
const images: NodeImages[] = [listing("cell2")];

/** cell2 on Oct 30: 121.7 GiB in all, 114.3 free, 116.9 available, 3 cached. */
function preflight(spec: DeploySpec, over: Partial<Preflight> = {}): Preflight {
  const on = spec.imageRef === DEFAULT_IMAGE;
  return {
    node: "cell2", demo: false,
    mem: { total: 127_598_748 * 1024, free: 119_861_600 * 1024, available: 122_601_504 * 1024, cached: 3_196_204 * 1024 },
    needFree: Math.floor(127_598_748 * 1024 * spec.gpuMemoryUtilization) + 4 * GIB, dropNeeded: false,
    earlyoom: { active: true, state: "active", config: "EARLYOOM_ARGS=\"-r 3600 -n\"" },
    imageRef: spec.imageRef ?? "", imageId: on ? ID_5A : null, modelPresent: true, portFree: true, portOwner: null, shm: [],
    caches: [{ path: "~/.cache/vllm", exists: true, owner: "sparkuser" }, { path: "~/.cache/flashinfer", exists: false, owner: null }, { path: "~/.triton", exists: false, owner: null }],
    problems: on ? [] : [`cell2 doesn't have ${spec.imageRef}. Pull it first (Fleet: docker pull ${spec.imageRef}), or pick an image that's there.`],
    replacing: null, freed: 0, store: "overlayfs", leftovers: [], downloads: [], rayLeftovers: [], ollamaResident: [],
    fabric: [{ iface: "enp1s0f1np1", ip: "192.168.100.154", prefix: 24, speedMbps: 200000, rdma: "rocep1s0f1", mtu: 9000, operstate: "up", rdmaState: "4: ACTIVE", rdmaPhys: "5: LinkUp" }],
    cutOff: false,
    ...over,
  };
}

/** A set around one node's read, its problems the set's: model_preflight's answer. */
function setOf(nodes: Preflight[], over: Partial<PreflightSet> = {}): PreflightSet {
  const problems = nodes.flatMap((p) => p.problems);
  return { nodes, verdict: problems.length ? "blocked" : "ready", problems, imageId: ID_5A, checkedAtMs: Date.now() - 2 * 60_000, ...over };
}

const recipeOf = (over: Partial<RecipeView>): RecipeView => ({
  id: "gemma-3-27b-it", title: "Gemma 3 27B", image: DEFAULT_IMAGE, topologies: ["single", "tp2", "pp3", "tp4"],
  defaults: { share: null, context: 32768, seqs: null }, toolParser: null, reasoningParser: null, note: null, placeholder: false, fits: [], verifiedOn: [], ...over,
});
const GEMMA_RECIPE = recipeOf({});
const GENERIC = recipeOf({ id: "generic", title: "Generic vLLM", topologies: ["single", "tp2", "tp3", "tp4", "pp2", "pp3", "pp4", "tp2pp2"], defaults: { share: null, context: null, seqs: null } });

const TP3 = "Tensor parallel 3 won't work for this model: 32 heads don't divide by 3. Use pipeline parallel 3.";
const NOT_LISTED = "The Gemma 3 27B recipe doesn't list this split. Pick Generic vLLM to try it.";
/** gemma's offers per node count, as recipe.rs gives them under its recipe (recipe.rs gemmas_offers_follow_its_recipe_and_its_config): 32 heads and 16 KV heads refuse TP=3, and the recipe lists single, tp2, pp3 and tp4. */
function gemmaOffers(nodes: number): TopologyOffer[] {
  const o = (id: string, tp: number, pp: number): TopologyOffer => ({ id, tp, pp, nodes, ok: true, reason: null, advanced: false, unlisted: false });
  const unlisted = (id: string, tp: number, pp: number, advanced = false): TopologyOffer => ({ ...o(id, tp, pp), ok: false, reason: NOT_LISTED, advanced, unlisted: true });
  if (nodes === 3) return [{ ...o("tp3", 3, 1), ok: false, reason: TP3 }, o("pp3", 1, 3)];
  if (nodes === 4) return [o("tp4", 4, 1), unlisted("pp4", 1, 4), unlisted("tp2pp2", 2, 2, true)];
  if (nodes === 2) return [o("tp2", 2, 1), unlisted("pp2", 1, 2)];
  return [o("single", 1, 1)];
}

const CLUSTER_NODES = [
  { id: "cell1", name: "cell1", online: true }, { id: "cell2", name: "cell2", online: true },
  { id: "cell3", name: "cell3", online: true }, { id: "gx10-0000", name: "cell4", online: true },
];

function snapshot(demo: boolean): ClusterSnapshot {
  const s = structuredClone(baseSnapshot) as ClusterSnapshot;
  s.tsMs = Date.now();
  s.demo = demo;
  return s;
}

const tick = () => act(async () => new Promise((res) => setTimeout(res, 50)));

interface ShowOpts {
  demo?: boolean;
  pf?: (spec: DeploySpec) => Preflight;
  /** The whole answer, for what one node's read can't say. */
  set?: (spec: DeploySpec) => PreflightSet;
  draft?: Draft;
  deployments?: Deployment[];
  library?: LibraryEntry[];
  nodes?: { id: string; name: string; online: boolean }[];
  images?: NodeImages[];
  recipe?: (spec: DeploySpec) => RecipeView | null;
  offers?: (nodes: number) => TopologyOffer[];
  /** What deploy_plan answers beyond the empty plan that fits everywhere. */
  plan?: (spec: DeploySpec) => Partial<DeployPlan>;
  preview?: (spec: DeploySpec) => PreviewStep[];
  admin?: AdminState;
  handlers?: MockHandlers;
}

/** The last page rendered, for a test that hands it a new draft. */
let lastRender: ReturnType<typeof render> | null = null;

async function show(opts: ShowOpts = {}) {
  resetStore();
  const admin = opts.admin ?? UNLOCKED;
  const handlers: MockHandlers = {
    get_snapshot: () => snapshot(opts.demo ?? false),
    admin_state: () => admin,
    images_list: () => opts.images ?? images,
    model_preflight: ({ spec }: { spec: DeploySpec }) => (opts.set ? opts.set(spec) : setOf([opts.pf ? opts.pf(spec) : preflight(spec)], opts.demo ? { verdict: "demo" } : {})),
    deploy_plan: ({ spec }: { spec: DeploySpec }) => ({
      nodes: [], level: "green", needs: { weightBytes: 54e9, arch: {}, kvBytesPerToken: null }, portTakenBy: null, suggestedPort: 8000, host: "192.168.50.154",
      url: "http://192.168.50.154:8000/v1", keyPreview: "sbk-••••", maxContext: 131072, suggestedShare: null, suggestedContext: null,
      recipe: opts.recipe ? opts.recipe(spec) : null, topologies: opts.offers ? opts.offers(spec.nodes.length) : [], ...opts.plan?.(spec),
    }),
    deploy_preview: (args: { spec: DeploySpec }) => (opts.preview ? opts.preview(args.spec) : []),
    endpoints_list: () => [],
    ...opts.handlers,
  };
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, handlers));
  _resetAdminForTests(admin);
  await startClusterStore();
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(
      <WindowManagerProvider sizeOf={() => [1200, 900]} isKnown={() => true}>
        <LoadPage draft={opts.draft ?? { key: GEMMA }} library={opts.library ?? library} nodes={opts.nodes ?? [{ id: "cell2", name: "cell2", online: true }]} deployments={opts.deployments ?? []} settings={settings} onClose={() => {}} onStarted={() => {}} onShared={() => {}} />
      </WindowManagerProvider>,
    );
  });
  for (let i = 0; i < 30 && !mockCalls.some((c) => c.cmd === "model_preflight"); i++) await tick();
  await tick();
  lastRender = r;
  return r.container;
}

const imageSelect = () => screen.getByRole("combobox", { name: "Image" }) as HTMLSelectElement;
const blocked = (root: HTMLElement) => root.querySelector(".ai-blocked")?.textContent ?? null;

afterEach(() => vi.restoreAllMocks());

it("shows inventory warnings as advisory without blocking a ready load", async () => {
  await show({ plan: () => ({ inventoryWarnings: ["cell2: expected IPv4 192.168.100.154, reported 192.168.100.99."] }) });
  expect(screen.getByText("Inventory expectations need review.")).toBeInTheDocument();
  expect(screen.getByText(/cell2: expected IPv4/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Review in Settings" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Load on 1 node" })).toBeEnabled();
});

describe("the image a load starts", () => {
  it("is offered with its presence on the chosen node, by local id", () => {
    const choices = imageChoices(images, ["cell2"], NVCR);
    expect(choices.map((c) => `${c.ref} — ${imagePresence(c)}`)).toEqual([
      "nvcr.io/nvidia/vllm:26.05-py3 — not on cell2",
      "vllm/vllm-openai:glm53-flash-arm64-cu130 — on cell2 · sha256:5a0f8b91…",
      "eugr/spark-vllm-b12x:nightly-20261015 — on cell2 · sha256:c0ffee12…",
    ]);
    // The saved image when it's there; else SparkBox's default (not docker's newest nightly).
    expect(defaultImage(choices, NVCR)).toBe(DEFAULT_IMAGE);
    // With the new default saved, NVIDIA's image is still offered: the fallback for garbage answers.
    expect(imageChoices(images, ["cell2"], DEFAULT_IMAGE).map((c) => c.ref)).toEqual([DEFAULT_IMAGE, NVCR, "eugr/spark-vllm-b12x:nightly-20261015"]);
    expect(defaultImage(imageChoices(images, ["cell2"], DEFAULT_IMAGE), DEFAULT_IMAGE)).toBe(DEFAULT_IMAGE);
    const noDefault = [{ ...images[0], images: images[0].images.filter((i) => i.repository !== "vllm/vllm-openai") }];
    expect(defaultImage(imageChoices(noDefault, ["cell2"], NVCR), NVCR)).toBe("eugr/spark-vllm-b12x:nightly-20261015");
    expect(defaultImage(imageChoices([], ["cell2"], NVCR), NVCR)).toBe(DEFAULT_IMAGE);
  });

  it("says where it is on every chosen node, by name, and defaults to one they all have", () => {
    const all = [listing("cell1"), listing("gx10-0000")];
    const name = (n: string) => (n === "gx10-0000" ? "cell4" : n);
    const [saved, def] = imageChoices(all, ["cell1", "gx10-0000"], NVCR);
    expect(imagePresence(saved, name)).toBe("not on cell1, cell4");
    expect(imagePresence(def, name)).toBe("on cell1, cell4 · sha256:5a0f8b91…");

    // One node lacks it, and two ids for one tag read as two parts.
    const short = [listing("cell1"), { ...listing("cell2"), images: [] }];
    expect(imagePresence(imageChoices(short, ["cell1", "cell2"], undefined)[0])).toBe("on cell1 · sha256:5a0f8b91…; not on cell2");
    expect(defaultImage(imageChoices(short, ["cell1", "cell2"], undefined), undefined)).toBe(DEFAULT_IMAGE);
    const other = [listing("cell1"), { ...listing("cell2"), images: listing("cell2").images.map((i) => ({ ...i, id: "abcdef012345" })) }];
    expect(imagePresence(imageChoices(other, ["cell1", "cell2"], undefined)[0])).toBe("on cell1 · sha256:5a0f8b91…; on cell2 · sha256:abcdef01…");

    // A saved image only the head has isn't the default for both, when another is on both.
    const NIGHTLY = "eugr/spark-vllm-b12x:nightly-20261015";
    const headOnly = [listing("cell1"), { ...listing("cell2"), images: listing("cell2").images.filter((i) => i.tag !== "nightly-20261015") }];
    expect(defaultImage(imageChoices(headOnly, ["cell1", "cell2"], NIGHTLY), NIGHTLY)).toBe(DEFAULT_IMAGE);
  });
});

describe("an image listing that didn't come back", () => {
  const failed: NodeImages[] = [listing("cell1"), { ...listing("cell2"), images: [], error: "docker didn't answer" }];

  it("says not read, not absent, and doesn't count the node as having it", () => {
    const [c] = imageChoices(failed, ["cell1", "cell2"], undefined);
    expect(imagePresence(c)).toBe("on cell1 · sha256:5a0f8b91…; not read on cell2");
    // Nothing back yet, or the call itself failed.
    expect(imagePresence(imageChoices(null, ["cell1"], undefined)[0])).toBe("not read on cell1");
    expect(imagePresence(imageChoices([], ["cell1"], undefined)[0])).toBe("not read on cell1");
    // A node left out of the answer isn't known to lack it either.
    expect(imagePresence(imageChoices([listing("cell1")], ["cell1", "cell2"], undefined)[0])).toBe("on cell1 · sha256:5a0f8b91…; not read on cell2");
    // Read, and it isn't there: that's a fact. An image on one and unread on another isn't on both.
    const empty = [{ ...listing("cell1"), images: [] }, { ...listing("cell2"), images: [], error: "no" }];
    expect(imagePresence(imageChoices(empty, ["cell1", "cell2"], undefined)[0])).toBe("not on cell1; not read on cell2");
    expect(defaultImage(imageChoices(failed, ["cell1", "cell2"], undefined), undefined)).toBe(DEFAULT_IMAGE);
  });

  it("uses the node's name, and the menu agrees with what holds Load", async () => {
    const nodes = [{ id: "cell1", name: "cell1", online: true }, { id: "cell2", name: "renamed", online: true }];
    const root = await show({ nodes, images: failed, library: [{ ...library[0], copies: { cell1: COPY, cell2: COPY } }], set: (spec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))) });
    await settle();
    await pickNodes(...nodes.filter((n) => !nodeBox(n.name).checked).map((n) => n.name));
    expect([...imageSelect().options].map((o) => o.textContent)).toContain("vllm/vllm-openai:glm53-flash-arm64-cu130 — on cell1 · sha256:5a0f8b91…; not read on renamed");
    // Unread isn't "isn't on": Load isn't held by a listing that failed.
    expect(blocked(root)).toBeNull();
  });
});

describe("image selection across the actual selected nodes", () => {
  it("blocks an image available only on the head even when preflight is otherwise ready", async () => {
    const nodes = [{ id: "cell1", name: "cell1", online: true }, { id: "cell2", name: "cell2", online: true }];
    const root = await show({ nodes, library: [{ ...library[0], copies: { cell1: COPY, cell2: COPY } }],
      images: [listing("cell1"), { ...listing("cell2"), images: [] }],
      set: (spec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))) });
    await pickNodes(...nodes.filter((n) => !nodeBox(n.name).checked).map((n) => n.name));
    await settle();
    expect(imageSelect().selectedOptions[0].textContent).toContain("not on cell2");
    expect(blocked(root)).toContain("isn't on cell2");
    expect(screen.getByRole("button", { name: "Load on 2 nodes" })).toBeDisabled();
    expect(mockCalls.some((c) => c.cmd === "model_deploy")).toBe(false);
  });
});

describe("Load page, one node with vLLM", () => {
  it("picks the image on cell2, stays at the first-load share with no sized one, and shows what the node has", async () => {
    const root = await show();
    const select = imageSelect();
    expect([...select.options].map((o) => o.textContent)).toEqual([
      "nvcr.io/nvidia/vllm:26.05-py3 — not on cell2",
      "vllm/vllm-openai:glm53-flash-arm64-cu130 — on cell2 · sha256:5a0f8b91…",
      "eugr/spark-vllm-b12x:nightly-20261015 — on cell2 · sha256:c0ffee12…",
    ]);
    expect(select.value).toBe(DEFAULT_IMAGE);
    expect((root.querySelector("input[type=number][step='0.05']") as HTMLInputElement).value).toBe("0.75");
    // What the pre-flight and the preview were asked about.
    const asked = mockCalls.filter((c) => c.cmd === "model_preflight").map((c) => c.args.spec as DeploySpec);
    expect(asked.at(-1)).toMatchObject({ imageRef: DEFAULT_IMAGE, image: DEFAULT_IMAGE, nodes: ["cell2"], gpuMemoryUtilization: 0.75, dropCaches: false });
    expect(mockCalls.filter((c) => c.cmd === "deploy_preview").at(-1)?.args.spec).toMatchObject({ imageRef: DEFAULT_IMAGE });
    // "Before it starts".
    expect(root).toHaveTextContent("Before it starts");
    expect(root).toHaveTextContent("121.7 GiB in all · 114.3 GiB free · 116.9 GiB available · 3.0 GiB cached");
    expect(root).toHaveTextContent("95.3 GiB free at share 0.75 (4 GiB of it headroom) · enough is free");
    expect(root).toHaveTextContent('active · EARLYOOM_ARGS="-r 3600 -n"');
    expect(root).toHaveTextContent("present on cell2");
    expect(root).toHaveTextContent("8000 free on cell2");
    expect(root).toHaveTextContent("~/.cache/vllm (sparkuser) · ~/.cache/flashinfer (missing) · ~/.triton (missing)");
    expect(screen.queryByLabelText(/Drop the page cache first/)).toBeNull();
    expect(root).toHaveTextContent("Ready to load on cell2.");
    expect(root).toHaveTextContent("Checked 2 min ago");
    expect(blocked(root)).toBeNull();
  });

  it("holds Load, with the reason, when the image picked isn't on the node", async () => {
    const root = await show();
    await act(async () => {
      fireEvent.change(imageSelect(), { target: { value: NVCR } });
    });
    for (let i = 0; i < 10; i++) await tick();
    expect(blocked(root)).toBe(`${NVCR} isn't on cell2: pull it first (Fleet: docker pull ${NVCR}), or pick an image that's there`);
    expect(screen.getByRole("button", { name: "Load on 1 node" })).toBeDisabled();
    expect(root).toHaveTextContent(`cell2 doesn't have ${NVCR}. Pull it first`);
  });

  it("holds Load when the folder isn't on the node", async () => {
    const root = await show({ pf: (spec) => preflight(spec, { modelPresent: false, problems: [`cell2 doesn't have ${GEMMA} yet. Copy it to that node first (Library › Copy to nodes…).`] }) });
    expect(blocked(root)).toBe(`cell2 doesn't have ${GEMMA} yet. Copy it to that node first (Library › Copy to nodes…).`);
    expect(root).toHaveTextContent("not on cell2");
  });

  it("lists only folders with a finished copy", async () => {
    const partial: LibraryEntry = { ...library[0], key: "/home/sparkuser/models/org/cut", repo: "org/cut", copies: { cell2: { path: "/home/sparkuser/models/org/cut", sizeBytes: 1e9, verified: false, partial: "stopped" } } };
    const root = await show({ draft: { key: "" }, library: [partial, library[0]] });
    const folder = root.querySelector("select.wide") as HTMLSelectElement;
    expect([...folder.options].map((o) => o.textContent)).toEqual(["google/gemma-3-27b-it — 51.1 GiB"]);
  });

  it("holds Load when the folder it was asked for isn't complete, and says where", async () => {
    const partial: LibraryEntry = { ...library[0], repo: "org/cut", copies: { cell2: { path: GEMMA, sizeBytes: 1e9, verified: false, partial: "stopped" } } };
    const root = await show({ library: [partial] });
    const says = "org/cut isn't complete on cell2. Finish it first.";
    expect(blocked(root)).toBe(says);
    expect(screen.getAllByText(says)).toHaveLength(2);
    expect((screen.getByRole("button", { name: "Load on 1 node" }) as HTMLButtonElement).disabled).toBe(true);
    expect(root.querySelector("select.wide")?.textContent).toContain("org/cut");
    expect(screen.queryByText(/doesn't have this model yet/)).toBeNull();
    expect(root.textContent).toContain("incomplete");
  });

  it("counts a copy left partway as no copy for a node, when another node has a finished one", async () => {
    const two: LibraryEntry = { ...library[0], copies: { cell2: { path: GEMMA, sizeBytes: 1e9, verified: false, partial: "stopped" }, cell1: { path: GEMMA, sizeBytes: 1e9, verified: true } } };
    const root = await show({ library: [two], draft: { key: GEMMA, spec: undefined } });
    expect(blocked(root)).toBe("Copy the model to cell2 first");
    expect(screen.getByText(/cell2 doesn't have this model yet/)).toBeTruthy();
  });

  it.each(["low", "cancel", "approve"])("reviews destination space before Copy it there: %s", async (choice) => {
    const key = "/models/fixture/model";
    const model = { ...library[0], key, repo: "fixture/model", copies: { cell1: { ...COPY, path: key, verified: true } } };
    const request = { kind: "copy" as const, key, targets: ["cell2"] };
    const checks = demoDiskPreflight(request);
    if (choice === "low") checks[0].ok = false;
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(choice === "approve");
    vi.spyOn(session, "alertInfo").mockResolvedValue(undefined);
    await show({ library: [model], draft: { key }, handlers: { model_disk_preflight: () => checks, model_share: () => [] } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Copy it there" })); });
    expect(mockCalls.find((c) => c.cmd === "model_disk_preflight")?.args).toEqual({ request });
    expect(confirm).toHaveBeenCalledTimes(choice === "low" ? 0 : 1);
    expect(mockCalls.filter((c) => c.cmd === "model_share")).toHaveLength(choice === "approve" ? 1 : 0);
    expect(mockCalls.some((c) => c.cmd === "deploy_start")).toBe(false);
  });

  it("offers the page-cache drop only when vLLM would refuse to start, and sends consent only then", async () => {
    const short = (spec: DeploySpec) => preflight(spec, { mem: { total: 127_598_748 * 1024, free: 43_201_000 * 1024, available: 122_601_504 * 1024, cached: 75 * GIB }, dropNeeded: true });
    const root = await show({ pf: short });
    expect(root).toHaveTextContent("95.3 GiB free at share 0.75 (4 GiB of it headroom) · cell2 has 41.2 GiB free");
    const drop = screen.getByLabelText("Drop the page cache first on cell2 (sudo; the kernel refills it)") as HTMLInputElement;
    expect(drop.checked).toBe(false);
    expect(blocked(root)).toBe("Not enough free memory: tick the page-cache drop or lower the share");
    await act(async () => {
      fireEvent.click(drop);
    });
    await tick();
    expect(blocked(root)).toBeNull();
    // The preview (asked 250 ms after a change) carries the consent.
    const lastPreview = () => mockCalls.filter((c) => c.cmd === "deploy_preview").at(-1)?.args.spec as DeploySpec | undefined;
    for (let i = 0; i < 20 && !lastPreview()?.dropCaches; i++) await tick();
    expect(lastPreview()).toMatchObject({ dropCaches: true });
  });

  it("in Demo reads nothing and says so", async () => {
    const root = await show({ demo: true, pf: (spec) => ({ ...preflight(spec), demo: true, mem: null, problems: [] }) });
    expect(root).toHaveTextContent("Available with Live data: SparkBox reads each node's memory, earlyoom, the image, the folder, the ports, /dev/shm, the cache folders, the fabric and what an earlier load left before a load.");
    expect(root).not.toHaveTextContent("Ready to load");
    expect(blocked(root)).toBeNull();
  });

  it("in Edit & reload counts the load it replaces as gone: its own port and its share", async () => {
    // gemma serves on cell2 at 0.75 on port 8000: 23.0 GiB free, its listener on the port.
    const was: DeploySpec = { id: "d1", servedName: "gemma-3-27b-it", modelPath: GEMMA, nodes: ["cell2"], tensorParallel: 1, pipelineParallel: 1, expertParallel: false, maxModelLen: 32768, gpuMemoryUtilization: 0.75, port: 8000, image: DEFAULT_IMAGE, imageRef: DEFAULT_IMAGE, extraArgs: [] };
    const serving: Deployment = { spec: was, endpoint: "http://192.168.50.154:8000/v1", state: "ready", stage: null, message: "Serving", logTail: "", startedAt: Date.now() / 1000 - 600, providerId: "vllm-d1", demo: false };
    const mem = { total: 127_598_748 * 1024, free: 24_162_539 * 1024, available: 31_000_000 * 1024, cached: 3_196_204 * 1024 };
    const reload = (freed: number, dropNeeded: boolean) => (spec: DeploySpec) => preflight(spec, {
      mem, freed, dropNeeded, replacing: "gemma-3-27b-it", portFree: false, portOwner: "gemma-3-27b-it, the load being replaced; free once it's unloaded",
    });
    const share = (s: number) => Math.floor(127_598_748 * 1024 * s);
    const root = await show({ draft: { key: GEMMA, spec: was, replaces: "d1" }, deployments: [serving], pf: reload(share(0.75), false) });
    expect(mockCalls.filter((c) => c.cmd === "model_preflight").at(-1)?.args.spec).toMatchObject({ id: "d1", port: 8000 });
    expect(root).toHaveTextContent("95.3 GiB free at share 0.75 (4 GiB of it headroom) · enough is free once gemma-3-27b-it is unloaded");
    expect(root).toHaveTextContent("8000 taken on cell2 (gemma-3-27b-it, the load being replaced; free once it's unloaded)");
    expect(blocked(root)).toBeNull();
    expect(screen.getByRole("button", { name: "Reload" })).toBeEnabled();

    // Its share at 0.5 doesn't free enough: the drop is offered, counted from what comes back.
    const short = await show({ draft: { key: GEMMA, spec: was, replaces: "d1" }, deployments: [serving], pf: reload(share(0.5), true) });
    expect(short).toHaveTextContent("· cell2 has 83.9 GiB free once gemma-3-27b-it is unloaded");
    expect(blocked(short)).toBe("Not enough free memory: tick the page-cache drop or lower the share");
  });

  it("says what it's doing while the node is read", () => {
    render(<BeforeItStarts set={null} problem={null} share={0.75} port={8000} drop={false} onDrop={() => {}} />);
    expect(screen.getByText("Reading the nodes…")).toBeInTheDocument();
  });
});

/** The node's tick box on the page, by the name it shows. */
const nodeBox = (name: string) => [...document.querySelectorAll(".node-pick")].find((l) => l.textContent?.startsWith(name) || l.textContent?.includes(` ${name}`))?.querySelector("input") as HTMLInputElement;

async function settle() {
  for (let i = 0; i < 12; i++) await tick();
}

/** Ticks nodes the way a click does, waiting for the plan the page asks for after each. */
async function pickNodes(...names: string[]) {
  for (const n of names) {
    await act(async () => {
      fireEvent.click(nodeBox(n));
    });
    await settle();
  }
}

const lastSpec = (cmd: string) => mockCalls.filter((c) => c.cmd === cmd).at(-1)?.args.spec as DeploySpec | undefined;
const numberField = (label: string) => screen.getByRole("spinbutton", { name: label }) as HTMLInputElement;
const shareField = (root: HTMLElement) => root.querySelector("input[type=number][step='0.05']") as HTMLInputElement;

describe("the recipe row", () => {
  it("shows the recipe picked for the folder and that nothing has verified it yet", async () => {
    const root = await show({ recipe: () => GEMMA_RECIPE });
    await settle();
    expect(root).toHaveTextContent("Gemma 3 27B");
    expect(root).toHaveTextContent("Verified on: nothing yet");
    expect(screen.queryByRole("combobox", { name: "Recipe" })).toBeNull();
  });

  it("gives its records once a load has passed readiness", async () => {
    const at = new Date(2026, 10, 2, 12).getTime();
    const root = await show({ recipe: () => recipeOf({ verifiedOn: [{ imageId: ID_5A, topology: "single", nodeCount: 1, atMs: at }, { imageId: ID_5A, topology: "tp2", nodeCount: 2, atMs: at }] }) });
    await settle();
    const day = new Date(at).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
    expect(root).toHaveTextContent(`Verified on: single on 1 node, 5a0f8b91…, ${day}; tp2 on 2 nodes, 5a0f8b91…, ${day}`);
    expect(root).not.toHaveTextContent("nothing yet");
  });

  it("fills share, context and parsers from its defaults", async () => {
    const qwen = recipeOf({ id: "qwen3-1.7b", title: "Qwen3 1.7B", defaults: { share: 0.7, context: 16384, seqs: 4 }, toolParser: "hermes", reasoningParser: "qwen3" });
    const root = await show({ recipe: () => qwen });
    await settle();
    expect(shareField(root).value).toBe("0.7");
    expect(lastSpec("deploy_preview")).toMatchObject({ gpuMemoryUtilization: 0.7, maxModelLen: 16384, maxNumSeqs: 4, toolCallParser: "hermes", reasoningParser: "qwen3" });
  });

  it("leaves alone what the owner touched, and offers a menu when more than one fits", async () => {
    const tight = recipeOf({ id: "gemma-tight", title: "Gemma, tight", defaults: { share: 0.6, context: 16384, seqs: null } });
    const fits = [GEMMA_RECIPE, tight, GENERIC].map(({ id, title }) => ({ id, title }));
    const root = await show({ recipe: (spec) => ({ ...(spec.recipe === "gemma-tight" ? tight : GEMMA_RECIPE), fits }) });
    await settle();
    const menu = screen.getByRole("combobox", { name: "Recipe" }) as HTMLSelectElement;
    expect([...menu.options].map((o) => o.textContent)).toEqual(["Gemma 3 27B", "Gemma, tight", "Generic vLLM"]);
    expect(lastSpec("deploy_preview")).toMatchObject({ maxModelLen: 32768, gpuMemoryUtilization: 0.75 });
    const context = root.querySelector("input.narrow-wide") as HTMLInputElement;
    await act(async () => {
      fireEvent.change(context, { target: { value: "8192" } });
    });
    await act(async () => {
      fireEvent.change(menu, { target: { value: "gemma-tight" } });
    });
    await settle();
    await settle();
    // The share follows the recipe; the context stays what the owner typed.
    expect(lastSpec("deploy_preview")).toMatchObject({ recipe: "gemma-tight", gpuMemoryUtilization: 0.6, maxModelLen: 8192 });
  });

  it("keeps a saved load's settings in Edit & reload", async () => {
    const was: DeploySpec = { id: "d1", servedName: "gemma-3-27b-it", modelPath: GEMMA, nodes: ["cell2"], tensorParallel: 1, pipelineParallel: 1, expertParallel: false, maxModelLen: 8192, gpuMemoryUtilization: 0.5, port: 8001, image: DEFAULT_IMAGE, imageRef: DEFAULT_IMAGE, extraArgs: [], recipe: "gemma-3-27b-it" };
    await show({ draft: { key: GEMMA, spec: was, replaces: "d1" }, recipe: () => GEMMA_RECIPE });
    await settle();
    expect(lastSpec("deploy_preview")).toMatchObject({ recipe: "gemma-3-27b-it", gpuMemoryUtilization: 0.5, maxModelLen: 8192 });
  });

  it("shows a placeholder's note and holds Load", async () => {
    const note = "Needs the FP4-Local checkpoint and its image, see What's coming";
    const root = await show({ recipe: () => recipeOf({ id: "glm-5.3-flash", title: "GLM-5.3 Flash", note, placeholder: true, defaults: { share: null, context: null, seqs: null } }) });
    await settle();
    expect(screen.getAllByText(note).length).toBeGreaterThan(0);
    expect(blocked(root)).toBe(note);
    expect(screen.getByRole("button", { name: "Load on 1 node" })).toBeDisabled();
  });

  it("gives a placeholder's note as the whole reason: no split is refused beside it", async () => {
    const note = "Needs the FP4-Local checkpoint and its image, see What's coming";
    const held = recipeOf({ id: "glm-5.3-flash", title: "GLM-5.3 Flash", note, placeholder: true, topologies: ["single"], defaults: { share: null, context: null, seqs: null } });
    // What recipe.rs offers a placeholder's folder, and the sentence an older offer carried: neither can be followed.
    for (const reason of [note, "The GLM-5.3 Flash recipe doesn't list this split. Pick Generic vLLM to try it."]) {
      const root = await show({ recipe: () => held, offers: (nodes) => [{ id: "single", tp: 1, pp: 1, nodes, ok: false, reason, advanced: false, unlisted: false }] });
      await settle();
      expect(screen.queryByRole("combobox", { name: "Recipe" })).toBeNull();
      expect(root).not.toHaveTextContent("doesn't list this split");
      expect(root).not.toHaveTextContent("Pick Generic vLLM");
      expect(root).not.toHaveTextContent("(won't work)");
      expect(root).not.toHaveTextContent("(not in this recipe)");
      expect([...(screen.getByRole("combobox", { name: "Split" }) as HTMLSelectElement).options].map((o) => o.textContent)).toEqual(["1 × 1: one node"]);
      expect(blocked(root)).toBe(note);
      cleanup();
    }
  });

  it("says a placeholder's note once, though the preview refuses with it too", async () => {
    const note = "Needs the FP4-Local checkpoint and its image, see What's coming";
    const held = recipeOf({ id: "glm-5.3-flash", title: "GLM-5.3 Flash", note, placeholder: true, topologies: [], defaults: { share: null, context: null, seqs: null } });
    const root = await show({ recipe: () => held, offers: (nodes) => [{ id: "single", tp: 1, pp: 1, nodes, ok: false, reason: note, advanced: false, unlisted: false }], handlers: { deploy_preview: () => { throw note; } } });
    await settle();
    expect(blocked(root)).toBe(note);
    expect([...root.querySelectorAll(".notice")].filter((n) => n.textContent?.includes(note))).toHaveLength(1);
  });

  it("leaves the share to the plan unless it has one of its own", () => {
    expect(recipeFill(recipeOf({ defaults: { share: 0.6, context: null, seqs: null } }), new Set()).gpuMem).toBe("0.6");
    expect(recipeFill(recipeOf({ defaults: { share: 0.6, context: null, seqs: null } }), new Set(["gpuMem"])).gpuMem).toBeUndefined();
  });

  it("leaves the model family's parsers to the generic recipe, whose parsers are empty", () => {
    expect(recipeFill(GENERIC, new Set())).toEqual({});
    // A named recipe's empty parser means off.
    expect(recipeFill(GEMMA_RECIPE, new Set())).toEqual({ maxLen: "32768", tools: false, reasoning: "" });
    expect(recipeFill(GEMMA_RECIPE, new Set(["gpuMem", "maxLen", "tools", "reasoning"]))).toEqual({});
  });

});

/** Qwen3-1.7B on cell2 at 32768, as deploy_plan answers it for the share it's asked about (real cell1, Nov 1). */
function qwenBar(spec: DeploySpec, over: Partial<NodePlan> = {}): NodePlan {
  const total = 121.7 * GIB;
  const budget = total * spec.gpuMemoryUtilization;
  return {
    node: "cell2", weights: 3.8 * GIB, kv: 3.5 * GIB, overhead: 6 * GIB, need: 13.3 * GIB, budget, free: budget, available: total - 4 * GIB, total, holders: [], level: "green", verdict: "Fits",
    inUse: 5.4 * GIB, hostRuntime: 9.4e9, afterPct: 37, maxShare: 0.95, safeShare: 0.75, fitsSafe: true, safeContext: null, reason: null, ...over,
  };
}
const qwenRequests = (spec: DeploySpec) => Math.floor((121.7 * GIB * spec.gpuMemoryUtilization - 9.8 * GIB) / (3.5 * GIB));

describe("a share sized to the model", () => {
  const sized = (spec: DeploySpec) => ({ nodes: [qwenBar(spec)], sizedShare: 0.25, fullRequests: qwenRequests(spec) });
  /** What cell1 read on Nov 1: page cache leaves 62 GiB free, under the 95.3 that 0.75 needs. */
  const cached = (spec: DeploySpec) => {
    const p = preflight(spec, { mem: { total: 127_598_748 * 1024, free: 62 * GIB, available: 116 * GIB, cached: 53.8 * GIB } });
    return { ...p, dropNeeded: 62 * GIB < p.needFree };
  };
  const set = async (field: HTMLElement, value: string) => {
    await act(async () => {
      fireEvent.change(field, { target: { value } });
    });
    await settle();
  };
  const context = (root: HTMLElement) => root.querySelector("input.narrow-wide") as HTMLInputElement;
  const requests = () => screen.getAllByPlaceholderText("auto")[0] as HTMLInputElement;
  const back = () => screen.queryByRole("button", { name: /^Use .* — (sized to this model|the recipe's share|the usual start)$/ });

  it("opens on it, asks the nodes for that much, and offers no page-cache drop", async () => {
    const root = await show({ plan: sized, pf: cached });
    await settle();
    expect(shareField(root).value).toBe("0.25");
    expect(lastSpec("deploy_preview")).toMatchObject({ gpuMemoryUtilization: 0.25 });
    expect(lastSpec("model_preflight")).toMatchObject({ gpuMemoryUtilization: 0.25, dropCaches: false });
    expect(root).toHaveTextContent("34.4 GiB free at share 0.25 (4 GiB of it headroom) · enough is free");
    expect(screen.queryByLabelText(/Drop the page cache first/)).toBeNull();
    expect(blocked(root)).toBeNull();
    expect(root).toHaveTextContent("Sized to this model.");
    expect(back()).toBeNull();
  });

  it("is what a load sends when Load is pressed", async () => {
    const deploy = vi.fn(() => null);
    const root = await show({ plan: sized, pf: cached, handlers: { deploy } });
    await settle();
    await act(async () => {
      fireEvent.click(root.querySelector(".btn.primary") as HTMLButtonElement);
    });
    await settle();
    expect(deploy).toHaveBeenCalledWith(expect.objectContaining({ spec: expect.objectContaining({ gpuMemoryUtilization: 0.25, dropCaches: false }) }));
  });

  it("follows Max context, Requests at once, the KV cache type and the nodes", async () => {
    const answer = (s: DeploySpec) => ({
      nodes: [qwenBar(s)],
      fullRequests: 5,
      sizedShare: s.nodes.length > 1 ? 0.4 : s.kvCacheDtype === "fp8" ? 0.2 : s.maxNumSeqs ? 0.35 : s.maxModelLen === 65536 ? 0.3 : 0.25,
    });
    const root = await show({ plan: answer, nodes: CLUSTER_NODES, library: everywhere, images: CLUSTER_NODES.map((n) => listing(n.id)), offers: gemmaOffers, set: (spec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))) });
    await settle();
    expect(shareField(root).value).toBe("0.25");
    await set(context(root), "65536");
    expect(shareField(root).value).toBe("0.3");
    await set(context(root), "32768");
    expect(shareField(root).value).toBe("0.25");
    await set(requests(), "8");
    expect(shareField(root).value).toBe("0.35");
    await set(requests(), "");
    await act(async () => {
      fireEvent.change(screen.getByRole("combobox", { name: "KV cache type" }), { target: { value: "fp8" } });
    });
    await settle();
    expect(shareField(root).value).toBe("0.2");
    await act(async () => {
      fireEvent.change(screen.getByRole("combobox", { name: "KV cache type" }), { target: { value: "auto" } });
    });
    await settle();
    expect(shareField(root).value).toBe("0.25");
    await pickNodes(CLUSTER_NODES.find((n) => !nodeBox(n.name).checked)!.name);
    expect(shareField(root).value).toBe("0.4");
    expect(lastSpec("model_preflight")).toMatchObject({ gpuMemoryUtilization: 0.4 });
  });

  it("follows the split, which divides the KV cache", async () => {
    const answer = (s: DeploySpec) => ({ nodes: [qwenBar(s)], sizedShare: s.pipelineParallel > 1 ? 0.45 : 0.4 });
    const root = await show({ plan: answer, nodes: CLUSTER_NODES, library: everywhere, images: CLUSTER_NODES.map((n) => listing(n.id)), offers: gemmaOffers });
    await settle();
    await pickNodes(CLUSTER_NODES.find((n) => !nodeBox(n.name).checked)!.name);
    expect(shareField(root).value).toBe("0.4");
    await act(async () => {
      fireEvent.change(numberField("Tensor parallel"), { target: { value: "1" } });
      fireEvent.change(numberField("Pipeline parallel"), { target: { value: "2" } });
    });
    await settle();
    expect(shareField(root).value).toBe("0.45");
  });

  it("never overrides a share typed here, and offers the way back", async () => {
    const answer = (s: DeploySpec) => ({ nodes: [qwenBar(s)], fullRequests: qwenRequests(s), sizedShare: s.maxModelLen === 65536 ? 0.3 : 0.25 });
    const root = await show({ plan: answer });
    await settle();
    await set(shareField(root), "0.6");
    expect(lastSpec("deploy_preview")).toMatchObject({ gpuMemoryUtilization: 0.6 });
    await set(context(root), "65536");
    await settle();
    expect(shareField(root).value).toBe("0.6");
    expect(lastSpec("model_preflight")).toMatchObject({ gpuMemoryUtilization: 0.6 });
    expect(root).not.toHaveTextContent("Sized to this model.");
    // The way back shows the share it goes to, as it is now.
    expect(back()).toHaveTextContent("Use 0.30 — sized to this model");
    await act(async () => {
      fireEvent.click(back()!);
    });
    await settle();
    expect(shareField(root).value).toBe("0.3");
    expect(back()).toBeNull();
    expect(root).toHaveTextContent("Sized to this model.");
    // And it follows again.
    await set(context(root), "32768");
    expect(shareField(root).value).toBe("0.25");
  });

  it("stays typed even at the sized value: the choice stands when the plan moves", async () => {
    const answer = (s: DeploySpec) => ({ nodes: [qwenBar(s)], sizedShare: s.maxModelLen === 65536 ? 0.3 : 0.25 });
    const root = await show({ plan: answer });
    await settle();
    await set(shareField(root), "0.5");
    await set(shareField(root), "0.25");
    expect(back()).toBeNull();
    await set(context(root), "65536");
    expect(shareField(root).value).toBe("0.25");
    expect(back()).toHaveTextContent("Use 0.30 — sized to this model");
  });

  it("keeps a saved load's share in Edit & reload, and offers the sized one", async () => {
    const was: DeploySpec = { id: "d1", servedName: "gemma-3-27b-it", modelPath: GEMMA, nodes: ["cell2"], tensorParallel: 1, pipelineParallel: 1, expertParallel: false, maxModelLen: 8192, gpuMemoryUtilization: 0.75, port: 8001, image: DEFAULT_IMAGE, imageRef: DEFAULT_IMAGE, extraArgs: [] };
    const root = await show({ draft: { key: GEMMA, spec: was, replaces: "d1" }, plan: sized });
    await settle();
    expect(shareField(root).value).toBe("0.75");
    expect(lastSpec("deploy_preview")).toMatchObject({ gpuMemoryUtilization: 0.75 });
    expect(back()).toHaveTextContent("Use 0.25 — sized to this model");
  });

  it("keeps the Advisor's share, from its options or its recommendation", async () => {
    const root = await show({ draft: { key: GEMMA, allowed: [["--gpu-memory-utilization", "0.6"]] }, plan: sized });
    await settle();
    expect(shareField(root).value).toBe("0.6");
    cleanup();
    const rec = { nodes: 1, parallelism: "single" as const, tensorParallelSize: 1, pipelineParallelSize: 1, expertParallel: false, maxModelLen: 32768, gpuMemoryUtilization: 0.5, extraVllmArgs: [], why: "w" };
    const again = await show({ draft: { key: GEMMA, rec }, plan: sized });
    await settle();
    expect(shareField(again).value).toBe("0.5");
    expect(back()).toHaveTextContent("Use 0.25 — sized to this model");
  });

  it("gives way to a share a recipe has a reason for", async () => {
    const own = recipeOf({ defaults: { share: 0.6, context: 32768, seqs: null } });
    const root = await show({ plan: sized, recipe: () => own });
    await settle();
    expect(shareField(root).value).toBe("0.6");
    expect(root).not.toHaveTextContent("Sized to this model.");
    await set(shareField(root), "0.5");
    expect(back()).toHaveTextContent("Use 0.60 — the recipe's share");
  });

  it("goes back to the first-load share for a model the plan can't size, after one it could", async () => {
    const BIG = "/home/sparkuser/models/big";
    const two: LibraryEntry[] = [...library, { ...library[0], key: BIG, repo: "acme/big", copies: { cell2: { ...COPY, path: BIG } } }];
    const answer = (s: DeploySpec) => ({ nodes: [qwenBar(s)], sizedShare: s.modelPath === BIG ? null : 0.25 });
    const root = await show({ plan: answer, library: two });
    await settle();
    expect(shareField(root).value).toBe("0.25");
    await act(async () => {
      fireEvent.change(screen.getAllByRole("combobox").find((c) => (c as HTMLSelectElement).value === GEMMA)!, { target: { value: BIG } });
    });
    await settle();
    expect(shareField(root).value).toBe(String(FIRST_LOAD_SHARE));
    expect(lastSpec("deploy_preview")).toMatchObject({ modelPath: BIG, gpuMemoryUtilization: 0.75 });
    expect(root).not.toHaveTextContent("Sized to this model.");
    expect(back()).toBeNull();
    // A value typed here has a way back to it too.
    await set(shareField(root), "0.6");
    expect(back()).toHaveTextContent("Use 0.75 — the usual start");
  });

  it("leaves SGLang at the usual share: the sizing arithmetic is vLLM's", async () => {
    const root = await show({ draft: { key: GEMMA, engine: "sglang" }, plan: sized });
    await settle();
    expect(shareField(root).value).toBe("0.75");
    expect(root).toHaveTextContent("SparkBox sizes the share for vLLM only, so SGLang starts at the usual share");
    expect(lastSpec("deploy_preview")).toMatchObject({ engine: "sglang", gpuMemoryUtilization: 0.75 });
    expect(back()).toBeNull();
    await set(shareField(root), "0.5");
    expect(back()).toHaveTextContent("Use 0.75 — the usual start");
  });

  it("switched to SGLang mid-page keeps the usual share, and a recipe's own share comes back on vLLM", async () => {
    const own = recipeOf({ defaults: { share: 0.6, context: 32768, seqs: null } });
    const root = await show({ plan: (s) => ({ nodes: [qwenBar(s)], sizedShare: 0.25 }), recipe: (s) => (s.engine === "sglang" ? null : own) });
    await settle();
    expect(shareField(root).value).toBe("0.6");
    await act(async () => { fireEvent.click(screen.getByRole("radio", { name: "SGLang" })); });
    await settle();
    expect(shareField(root).value).toBe("0.75");
    expect(lastSpec("deploy_preview")).toMatchObject({ engine: "sglang", gpuMemoryUtilization: 0.75 });
    await act(async () => { fireEvent.click(screen.getByRole("radio", { name: "vLLM" })); });
    await settle();
    expect(shareField(root).value).toBe("0.6");
  });

  it("a share the plan offers is a choice: it stays, and has the way back", async () => {
    // Amber at 0.75, and 0.7 keeps it under the line: the plan's own button sets the share.
    const tight = (s: DeploySpec) => ({ level: "amber" as const, nodes: [qwenBar(s, { level: "amber", verdict: "Tight", afterPct: 91, reason: "Tight: runs above the 90% alert line (91% of cell2's memory)." })], suggestedShare: s.gpuMemoryUtilization > 0.7 ? 0.7 : null, suggestedContext: null });
    const root = await show({ plan: tight });
    await settle();
    expect(shareField(root).value).toBe("0.75");
    const offer = screen.getByRole("button", { name: "Use 0.70 — fits and stays under 90%" });
    await act(async () => { fireEvent.click(offer); });
    await settle();
    expect(shareField(root).value).toBe("0.70");
    expect(lastSpec("deploy_preview")).toMatchObject({ gpuMemoryUtilization: 0.7 });
    expect(back()).toHaveTextContent("Use 0.75 — the usual start");
    await set(context(root), "65536");
    expect(shareField(root).value).toBe("0.70");
    // The shorter-context button is a choice of the share too.
    cleanup();
    const long = (s: DeploySpec) => ({ level: "red" as const, nodes: [qwenBar(s, { level: "red", verdict: "Won't fit", fitsSafe: false, safeContext: 98304, reason: "Won't fit." })], suggestedShare: null, suggestedContext: s.gpuMemoryUtilization > 0.4 ? [0.4, 98304] as [number, number] : null });
    const again = await show({ plan: long });
    await settle();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: /^Use 0\.40 and a .* context — fits and stays under 90%$/ })); });
    await settle();
    expect(shareField(again).value).toBe("0.40");
    expect(context(again).value).toBe("98304");
    expect(back()).toHaveTextContent("Use 0.75 — the usual start");
  });

  it("the way back also drops the Advisor's highlight", async () => {
    const root = await show({ draft: { key: GEMMA, allowed: [["--gpu-memory-utilization", "0.6"]] }, plan: sized });
    await settle();
    expect(shareField(root).className).toContain("ai-filled");
    await act(async () => { fireEvent.click(back()!); });
    await settle();
    expect(shareField(root).value).toBe("0.25");
    expect(shareField(root).className).not.toContain("ai-filled");
  });

  it("a model too big to size opens at the usual share, and the plan still judges it and offers what fits", async () => {
    const tight = (s: DeploySpec) => ({ level: s.gpuMemoryUtilization > 0.7 ? "amber" as const : "green" as const, nodes: [qwenBar(s, s.gpuMemoryUtilization > 0.7 ? { level: "amber", verdict: "Tight", afterPct: 91, reason: "Tight: runs above the 90% alert line (91% of cell2's memory)." } : {})], sizedShare: null, suggestedShare: s.gpuMemoryUtilization > 0.7 ? 0.7 : null });
    const root = await show({ plan: tight });
    await settle();
    expect(shareField(root).value).toBe("0.75");
    expect(root.querySelector(".ai-plan")).toHaveTextContent("Tight");
    expect(screen.getByRole("button", { name: "Use 0.70 — fits and stays under 90%" })).toBeTruthy();
    expect(root).not.toHaveTextContent("Sized to this model.");
  });

  it("takes the floor, and refuses what's below it", async () => {
    expect(MIN_SHARE).toBe(0.2);
    const root = await show({ plan: sized });
    await settle();
    await set(shareField(root), "0.2");
    expect(blocked(root)).toBeNull();
    await set(shareField(root), "0.19");
    expect(blocked(root)).toBe("GPU memory share must be a number from 0.2 to 0.95.");
    expect(shareField(root).min).toBe("0.2");
  });

  it("says what the model needs, what the share reserves and that the KV cache gets the rest", async () => {
    const root = await show({ plan: sized, pf: cached });
    await settle();
    expect(root.querySelector(".ai-plan")).toHaveTextContent("Needs 13.3 GiB; this share reserves 30.4 GiB, and the KV cache gets the rest, about 5 full-length requests.");
    expect(root.querySelector(".seg-kv")?.getAttribute("title")).toBe("KV cache: everything left in the share, 20.6 GiB");
  });

  it("words it for one request, and for a KV cache it can't size", () => {
    expect(shareWords({ need: 13.3 * GIB, budget: 24.3 * GIB, kv: 3.5 * GIB }, 1)).toBe("Needs 13.3 GiB; this share reserves 24.3 GiB, and the KV cache gets the rest, about 1 full-length request.");
    expect(shareWords({ need: 9.8 * GIB, budget: 91.3 * GIB, kv: 0 }, null)).toBe("Needs at least 9.8 GiB; this share reserves 91.3 GiB, and the KV cache gets the rest.");
  });

  it("leaves the sentence out when the model doesn't fit: the plan says that instead", async () => {
    const short = (s: DeploySpec) => ({ level: "red" as const, nodes: [qwenBar(s, { need: 100 * GIB, level: "red", verdict: "Won't fit", reason: "Won't fit." })], sizedShare: null, fullRequests: 0 });
    const root = await show({ plan: short });
    await settle();
    expect(root.querySelector(".ai-plan")).toHaveTextContent("needs 100 GiB, more than its 91.3 GiB share");
    expect(root.querySelector(".ai-plan")).not.toHaveTextContent("gets the rest");
  });
});

/** What the review threw at the share (its attacks A to H), kept as the page's contract. */
describe("a share that follows the plan, under pressure", () => {
  const BIG = "/home/sparkuser/models/big";
  const two: LibraryEntry[] = [...library, { ...library[0], key: BIG, repo: "acme/big", sizeBytes: 60e9, copies: { cell2: { ...COPY, path: BIG } } }];
  const base = (s: DeploySpec) => ({ nodes: [qwenBar(s)], sizedShare: 0.25, fullRequests: 5 });
  const folder = () => screen.getAllByRole("combobox").find((c) => [...(c as HTMLSelectElement).options].some((o) => o.value === BIG)) as HTMLSelectElement;
  const planOf = (_spec: DeploySpec, over: Partial<DeployPlan>): DeployPlan => ({
    nodes: [], level: "green", needs: { weightBytes: 54e9, arch: {} as DeployPlan["needs"]["arch"], kvBytesPerToken: null }, portTakenBy: null, suggestedPort: 8000, host: "192.168.50.154",
    url: "http://192.168.50.154:8000/v1", keyPreview: "sbk-••••", maxContext: 131072, suggestedShare: null, suggestedContext: null, sizedShare: null, fullRequests: null, recipe: null, topologies: [], ...over,
  });
  const set = async (field: HTMLElement, value: string) => {
    await act(async () => { fireEvent.change(field, { target: { value } }); });
    await settle();
  };
  const calls = (cmd: string) => mockCalls.filter((c) => c.cmd === cmd).length;
  const button = (root: HTMLElement) => root.querySelector(".btn.primary") as HTMLButtonElement;
  const back = () => screen.queryByRole("button", { name: /^Use .* — (sized to this model|the recipe's share|the usual start)$/ });
  /** cell1 on Nov 1: the page cache leaves 62 GiB free, under the 95.3 GiB a 0.75 share needs. */
  const cached = (spec: DeploySpec) => {
    const p = preflight(spec, { mem: { total: 127_598_748 * 1024, free: 62 * GIB, available: 116 * GIB, cached: 53.8 * GIB } });
    return { ...p, dropNeeded: 62 * GIB < p.needFree };
  };

  it("A: shows no share, reads no node and holds Load until the plan for this model arrives", async () => {
    const deploy = vi.fn(() => null);
    const deploy_plan = ({ spec }: { spec: DeploySpec }) => (spec.modelPath === BIG ? new Promise(() => {}) : planOf(spec, base(spec)));
    const root = await show({ library: two, handlers: { deploy_plan, deploy } });
    await settle();
    expect(shareField(root).value).toBe("0.25");
    const reads = calls("model_preflight");
    await set(folder(), BIG);
    expect(shareField(root).value).toBe("");
    expect(root).toHaveTextContent("Waiting for the memory plan to size it");
    expect(root).toHaveTextContent("Waiting for the memory plan: the nodes are read once the share is known.");
    expect(root).toHaveTextContent("Working out the plan…");
    expect(root).not.toHaveTextContent("Needs ");
    expect(blocked(root)).toBe("Working out the memory plan…");
    expect(button(root).disabled).toBe(true);
    expect(calls("model_preflight")).toBe(reads);
    await act(async () => { fireEvent.click(button(root)); });
    await settle();
    expect(deploy).not.toHaveBeenCalled();
    // A plan that never comes back leaves it held, with the reason.
    cleanup();
    const failing = await show({ handlers: { deploy_plan: () => { throw "no"; } } });
    await settle();
    expect(blocked(failing)).toBe("Couldn't work out the plan: no");
    expect(shareField(failing).value).toBe("");
  });

  it("B: a share typed before the first plan lands stays", async () => {
    let release: () => void = () => {};
    let first = true;
    const deploy_plan = ({ spec }: { spec: DeploySpec }) => {
      if (first) {
        first = false;
        return new Promise((res) => { release = () => res(planOf(spec, base(spec))); });
      }
      return planOf(spec, base(spec));
    };
    const root = await show({ handlers: { deploy_plan } });
    await set(shareField(root), "0.6");
    release();
    await settle();
    expect(shareField(root).value).toBe("0.6");
    expect(lastSpec("model_preflight")).toMatchObject({ gpuMemoryUtilization: 0.6 });
  });

  it("C: Edit & reload arriving on a page already open on that folder opens with the saved values", async () => {
    const was: DeploySpec = { id: "d1", servedName: "gemma-3-27b-it", modelPath: GEMMA, nodes: ["cell2"], tensorParallel: 1, pipelineParallel: 1, expertParallel: false, maxModelLen: 8192, gpuMemoryUtilization: 0.6, port: 8001, image: DEFAULT_IMAGE, imageRef: DEFAULT_IMAGE, extraArgs: [] };
    const root = await show({ plan: base });
    await settle();
    expect(shareField(root).value).toBe("0.25");
    await act(async () => {
      lastRender!.rerender(
        <WindowManagerProvider sizeOf={() => [1200, 900]} isKnown={() => true}>
          <LoadPage draft={{ key: GEMMA, spec: was, replaces: "d1" }} library={library} nodes={[{ id: "cell2", name: "cell2", online: true }]} deployments={[]} settings={settings} onClose={() => {}} onStarted={() => {}} onShared={() => {}} />
        </WindowManagerProvider>,
      );
    });
    await settle();
    expect(root.querySelector(".toolbar-text.strong")).toHaveTextContent("Edit gemma-3-27b-it and reload");
    expect(shareField(root).value).toBe("0.6");
    expect((root.querySelector("input.narrow-wide") as HTMLInputElement).value).toBe("8192");
    expect(lastSpec("deploy_preview")).toMatchObject({ id: "d1", gpuMemoryUtilization: 0.6, maxModelLen: 8192, port: 8001 });
    expect(back()).toHaveTextContent("Use 0.25 — sized to this model");
    // The drafts that are different pages, and the ones that aren't.
    expect(draftKey({ key: GEMMA })).toBe(draftKey({ key: GEMMA }));
    expect(draftKey({ key: GEMMA })).not.toBe(draftKey({ key: GEMMA, spec: was, replaces: "d1" }));
    expect(draftKey({ key: GEMMA })).not.toBe(draftKey({ key: GEMMA, engine: "sglang" }));
  });

  it("D: the nodes are read once, at the share the load will use, so no drop is offered and withdrawn", async () => {
    const deploy = vi.fn(() => null);
    let release: () => void = () => {};
    let hold = true;
    const deploy_plan = ({ spec }: { spec: DeploySpec }) => (hold ? new Promise((res) => { release = () => { hold = false; res(planOf(spec, base(spec))); }; }) : planOf(spec, base(spec)));
    const root = await show({ pf: cached, handlers: { deploy_plan, deploy } });
    await settle();
    expect(calls("model_preflight")).toBe(0);
    expect(screen.queryByLabelText(/Drop the page cache first/)).toBeNull();
    release();
    await settle();
    await settle();
    expect(shareField(root).value).toBe("0.25");
    expect(mockCalls.filter((c) => c.cmd === "model_preflight").map((c) => (c.args.spec as DeploySpec).gpuMemoryUtilization)).toEqual([0.25]);
    expect(screen.queryByLabelText(/Drop the page cache first/)).toBeNull();
    expect(blocked(root)).toBeNull();
    await act(async () => { fireEvent.click(button(root)); });
    await settle();
    expect(deploy).toHaveBeenCalledWith(expect.objectContaining({ spec: expect.objectContaining({ gpuMemoryUtilization: 0.25, dropCaches: false }) }));
  });

  it("E: a plan for the model before can't set the share of the one chosen now", async () => {
    const answers: Record<string, () => void> = {};
    const deploy_plan = ({ spec }: { spec: DeploySpec }) => new Promise((res) => {
      answers[`${spec.modelPath}|${spec.gpuMemoryUtilization}`] = () => res(planOf(spec, { nodes: [qwenBar(spec)], sizedShare: spec.modelPath === BIG ? 0.6 : 0.25 }));
    });
    const root = await show({ library: two, handlers: { deploy_plan } });
    await settle();
    const first = Object.keys(answers)[0];
    expect(first.startsWith(GEMMA)).toBe(true);
    await set(folder(), BIG);
    answers[first]();
    await settle();
    expect(shareField(root).value).toBe("");
    expect(root.querySelector(".ai-plan")).toBeNull();
    const big = Object.keys(answers).find((k) => k.startsWith(BIG))!;
    answers[big]();
    await settle();
    expect(shareField(root).value).toBe("0.6");
    // The bars wait for the plan at that share, then show it.
    const at = Object.keys(answers).find((k) => k === `${BIG}|0.6`)!;
    expect(root.querySelector(".ai-plan")).toBeNull();
    answers[at]();
    await settle();
    expect(root.querySelector(".ai-plan")).not.toBeNull();
  });

  it("F: a plan that flips its sized share on every read settles in one extra round", async () => {
    let i = 0;
    const deploy_plan = ({ spec }: { spec: DeploySpec }) => planOf(spec, { nodes: [qwenBar(spec)], sizedShare: i++ % 2 ? 0.45 : 0.4 });
    const root = await show({ handlers: { deploy_plan } });
    for (let n = 0; n < 16; n++) await settle();
    expect(shareField(root).value).toBe("0.4");
    // The plan to learn the share, the plan at it, and once more after the nodes were read.
    expect(calls("deploy_plan")).toBeLessThanOrEqual(3);
    expect(calls("model_preflight")).toBe(1);
  });

  it("H: a share typed for one model stays on another, and offers that model's way back", async () => {
    const root = await show({ library: two, plan: (s) => ({ nodes: [qwenBar(s)], sizedShare: s.modelPath === BIG ? 0.6 : 0.25 }) });
    await settle();
    await set(shareField(root), "0.3");
    await set(folder(), BIG);
    expect(shareField(root).value).toBe("0.3");
    expect(lastSpec("deploy_preview")).toMatchObject({ modelPath: BIG, gpuMemoryUtilization: 0.3 });
    expect(back()).toHaveTextContent("Use 0.60 — sized to this model");
  });

  it("a change to the limits re-plans at the last share first, then once at the new one", async () => {
    const root = await show({ plan: (s) => ({ nodes: [qwenBar(s)], sizedShare: s.maxModelLen === 65536 ? 0.3 : 0.25 }) });
    await settle();
    const before = calls("deploy_plan");
    await set(root.querySelector("input.narrow-wide") as HTMLInputElement, "65536");
    await settle();
    expect(shareField(root).value).toBe("0.3");
    const asked = mockCalls.filter((c) => c.cmd === "deploy_plan").slice(before).map((c) => (c.args.spec as DeploySpec).gpuMemoryUtilization);
    expect(asked[0]).toBe(0.25);
    expect(asked.at(-1)).toBe(0.3);
    expect(asked.length).toBeLessThanOrEqual(3);
  });
});

describe("the split", () => {
  const several = { nodes: CLUSTER_NODES, library: everywhere, offers: gemmaOffers, images: CLUSTER_NODES.map((n) => listing(n.id)), set: (spec: DeploySpec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))) };

  it("opens on 1 × 3 at three nodes, where TP=3 is invalid", async () => {
    const root = await show(several);
    await settle();
    const left = CLUSTER_NODES.filter((n) => !nodeBox(n.name).checked).map((n) => n.name).slice(0, 2);
    await pickNodes(...left);
    await settle();
    expect(numberField("Tensor parallel").value).toBe("1");
    expect(numberField("Pipeline parallel").value).toBe("3");
    expect(lastSpec("deploy_preview")).toMatchObject({ tensorParallel: 1, pipelineParallel: 3 });
    expect((screen.getByRole("combobox", { name: "Split" }) as HTMLSelectElement).value).toBe("pp3");
    expect(root).not.toHaveTextContent("heads don't divide by 3");
    expect(blocked(root)).toBeNull();
  });

  it("refuses TP=3 with Rust's sentence and holds Load, from the menu and from the number fields", async () => {
    const root = await show(several);
    await settle();
    const left = CLUSTER_NODES.filter((n) => !nodeBox(n.name).checked).map((n) => n.name).slice(0, 2);
    await pickNodes(...left);
    await settle();
    await act(async () => {
      fireEvent.change(screen.getByRole("combobox", { name: "Split" }), { target: { value: "tp3" } });
    });
    await settle();
    expect(root).toHaveTextContent(TP3);
    expect(blocked(root)).toBe(TP3);
    expect(screen.getByRole("button", { name: "Load on 3 nodes" })).toBeDisabled();
    // Back to the valid one with the number fields, which stay.
    await act(async () => {
      fireEvent.change(numberField("Tensor parallel"), { target: { value: "1" } });
      fireEvent.change(numberField("Pipeline parallel"), { target: { value: "3" } });
    });
    await settle();
    expect(blocked(root)).toBeNull();
    await act(async () => {
      fireEvent.change(numberField("Tensor parallel"), { target: { value: "3" } });
      fireEvent.change(numberField("Pipeline parallel"), { target: { value: "1" } });
    });
    await settle();
    expect(blocked(root)).toBe(TP3);
  });

  it("picks the split again when the nodes change, even after one was chosen by hand", async () => {
    const root = await show(several);
    await settle();
    const left = CLUSTER_NODES.filter((n) => !nodeBox(n.name).checked).map((n) => n.name);
    await pickNodes(left[0]);
    await act(async () => {
      fireEvent.change(numberField("Pipeline parallel"), { target: { value: "2" } });
    });
    await settle();
    await pickNodes(left[1]);
    await settle();
    expect(numberField("Tensor parallel").value).toBe("1");
    expect(numberField("Pipeline parallel").value).toBe("3");
    expect(blocked(root)).toBeNull();
  });

  it("offers TP=4 first on four nodes and marks 2 × 2 advanced", async () => {
    await show(several);
    await settle();
    const left = CLUSTER_NODES.filter((n) => !nodeBox(n.name).checked).map((n) => n.name);
    await pickNodes(...left);
    await settle();
    expect(numberField("Tensor parallel").value).toBe("4");
    expect(numberField("Pipeline parallel").value).toBe("1");
    expect([...(screen.getByRole("combobox", { name: "Split" }) as HTMLSelectElement).options].map((o) => o.textContent)).toEqual([
      "4 × 1: tensor parallel 4", "1 × 4: pipeline parallel 4 (not in this recipe)", "2 × 2: tensor and pipeline parallel, advanced (not in this recipe)",
    ]);
  });

  it("doesn't block on a warning offer", async () => {
    const warn = "This model's config doesn't say how many attention heads it has, so SparkBox can't check this split. vLLM should refuse a bad one when it starts, and SparkBox cleans up.";
    const root = await show({ ...several, offers: (nodes) => [{ id: "tp2", tp: nodes, pp: 1, nodes, ok: true, reason: warn, advanced: false, unlisted: false }] });
    await settle();
    expect(screen.getByText(warn)).toBeInTheDocument();
    expect(blocked(root)).toBeNull();
  });

  it("words the offers, and takes the first one that works for this many nodes", () => {
    const offers = gemmaOffers(3);
    expect(offers.map((o) => offerWords(o))).toEqual(["3 × 1: tensor parallel 3 (won't work)", "1 × 3: pipeline parallel 3"]);
    expect(offerWords(gemmaOffers(1)[0])).toBe("1 × 1: one node");
    expect(firstOffer(offers, 3)?.id).toBe("pp3");
    // A plan for two nodes while three are ticked offers nothing yet.
    expect(firstOffer(offers, 2)).toBeNull();
    expect(firstOffer([offers[0]], 3)).toBeNull();
    expect(offerWords(gemmaOffers(4)[2])).toBe("2 × 2: tensor and pipeline parallel, advanced (not in this recipe)");
  });

  it("words a split from why it's refused: the model can't, or this recipe doesn't list it", () => {
    // The same split is fine under Generic vLLM, so "won't work" would be false.
    expect(offerWords(gemmaOffers(2)[1])).toBe("1 × 2: pipeline parallel 2 (not in this recipe)");
    expect(offerWords({ ...gemmaOffers(2)[1], ok: true, reason: null, unlisted: false })).toBe("1 × 2: pipeline parallel 2");
    expect(offerWords(gemmaOffers(3)[0])).toBe("3 × 1: tensor parallel 3 (won't work)");
    // A recipe that can't load at all: no word for any split.
    expect(offerWords(gemmaOffers(3)[0], true)).toBe("3 × 1: tensor parallel 3");
    expect(offerWords(gemmaOffers(2)[1], true)).toBe("1 × 2: pipeline parallel 2");
  });

  it("keeps a recipe's sentence for a split it doesn't list, which says where to try it", async () => {
    const root = await show(several);
    await settle();
    await pickNodes(CLUSTER_NODES.find((n) => !nodeBox(n.name).checked)!.name);
    await act(async () => {
      fireEvent.change(screen.getByRole("combobox", { name: "Split" }), { target: { value: "pp2" } });
    });
    await settle();
    expect(blocked(root)).toBe(NOT_LISTED);
    expect(screen.getAllByRole("alert").map((a) => a.textContent)).toContain(NOT_LISTED);
  });
});

describe("Before it starts, for the set", () => {
  const two = { nodes: CLUSTER_NODES, library: everywhere, images: CLUSTER_NODES.map((n) => listing(n.id)), draft: { key: GEMMA, spec: undefined, rec: undefined } as Draft };
  const spec2 = { nodes: ["cell1", "gx10-0000"], imageRef: DEFAULT_IMAGE, gpuMemoryUtilization: 0.75 } as DeploySpec;
  const LEFT = "gx10-0000 has containers left from an earlier load (sparkbox-old-r1). Remove them first.";
  const LEFT_SHOWN = "cell4 has containers left from an earlier load (sparkbox-old-r1). Remove them first.";

  it("gives one verdict, then a block per node with its role and its current name", async () => {
    const set = (spec: DeploySpec) => setOf(spec.nodes.map((node) => preflight(spec, { node })));
    const root = await show({ ...two, set, offers: gemmaOffers, recipe: () => GEMMA_RECIPE });
    await settle();
    await pickNodes(...CLUSTER_NODES.filter((n) => nodeBox(n.name).checked).map((n) => n.name));
    await pickNodes("cell1", "cell4");
    await settle();
    expect(lastSpec("model_preflight")).toMatchObject({ nodes: ["cell1", "gx10-0000"], imageRef: DEFAULT_IMAGE, image: DEFAULT_IMAGE });
    expect(root).toHaveTextContent("Ready to load on cell1 and cell4.");
    expect(root).toHaveTextContent("cell1 · head, serves the API");
    expect(root).toHaveTextContent("cell4 · worker, rank 1");
    expect(root).toHaveTextContent("8000 and 29501 free on cell1");
    expect(root).toHaveTextContent("29501 free on cell4");
    expect(root).not.toHaveTextContent("8000 and 29501 free on cell4");
    expect(root).toHaveTextContent("enp1s0f1np1 192.168.100.154 · 200 Gb/s · MTU 9000 · RDMA ACTIVE");
    expect(root).toHaveTextContent("Tested when you press Load: each worker sends jumbo frames to the head on each half.");
    expect(root).toHaveTextContent("Each model, image and split needs its own checks.");
    expect(within(root).getByRole("button", { name: "What's proven" })).toBeEnabled();
    expect(root).not.toHaveTextContent("haven't run on your cluster yet");
    // The id never shows: ids go to Rust, the page shows names.
    expect(root).not.toHaveTextContent("gx10-0000");
  });

  it("names a blocked set's problem, holds Load and keeps every node's reads", async () => {
    const set = (spec: DeploySpec) => setOf(spec.nodes.map((node, i) => preflight(spec, i ? { node, leftovers: [{ id: "abc", name: "sparkbox-old-r1", deployment: "old", instance: "dev", status: "Exited (1) 2 hours ago", ours: true }], problems: [LEFT] } : { node })));
    const root = await show({ ...two, set });
    await settle();
    await pickNodes(...CLUSTER_NODES.filter((n) => nodeBox(n.name).checked).map((n) => n.name));
    await pickNodes("cell1", "cell4");
    await settle();
    expect(root).toHaveTextContent("Not ready to load on cell1 and cell4: 1 problem to fix first.");
    expect(screen.getAllByText(LEFT_SHOWN)).toHaveLength(2); // the notice, and the toolbar's reason
    expect(blocked(root)).toBe(LEFT_SHOWN);
    expect(root).not.toHaveTextContent("gx10-0000");
    expect(screen.getByRole("button", { name: "Load on 2 nodes" })).toBeDisabled();
    expect(root).toHaveTextContent("sparkbox-old-r1 (Exited (1) 2 hours ago)");
    expect(root).toHaveTextContent("cell1 · head, serves the API");
  });

  /** What Rust sends for a node it couldn't read: Preflight::default with its node, image ref and the problem. */
  const unread = (node: string, imageRef: string): Preflight => ({
    node, demo: false, mem: null, needFree: 0, dropNeeded: false, earlyoom: null, imageRef, imageId: null, modelPresent: false, portFree: false, portOwner: null,
    shm: [], caches: [], problems: [`Couldn't read ${node}: connection timed out`], replacing: null, freed: 0, store: null, leftovers: [], downloads: [], rayLeftovers: [], ollamaResident: [], fabric: [], cutOff: true,
  });

  it("says a node it couldn't read is unread, and prints none of the empty answer as a reading", async () => {
    const set = (spec: DeploySpec) => setOf(spec.nodes.map((node) => (node === "gx10-0000" ? unread(node, spec.imageRef ?? "") : preflight(spec, { node }))));
    const root = await show({ ...two, set });
    await settle();
    await pickNodes(...CLUSTER_NODES.filter((n) => nodeBox(n.name).checked).map((n) => n.name));
    await pickNodes("cell1", "cell4");
    await settle();
    const block = [...root.querySelectorAll(".ai-step")].find((b) => b.textContent?.startsWith("cell4"))!;
    expect(block.textContent).toBe("cell4 · worker, rank 1Couldn't read cell4");
    expect(root).toHaveTextContent("Not ready to load on cell1 and cell4: 1 problem to fix first.");
    expect(blocked(root)).toBe("Couldn't read cell4: connection timed out");
    for (const made of ["(null)", "not on cell4", "Taken on cell4", "nothing in memory on cell4", "0.0 GiB free", "no address"]) expect(root).not.toHaveTextContent(made);
    // The node that was read keeps its rows.
    expect(root).toHaveTextContent("8000 and 29501 free on cell1");
  });

  it("treats a read cut off partway as unread: what it has isn't the whole node", () => {
    const part = preflight(spec2, { node: "cell2", cutOff: true, problems: ["Couldn't finish reading cell2: it stopped partway."] });
    const { container } = render(<BeforeItStarts set={setOf([part])} problem={null} share={0.75} port={8000} drop={false} onDrop={() => {}} />);
    expect(container.querySelector(".ai-step")?.textContent).toBe("cell2Couldn't read cell2");
  });

  it("keeps the rows of a read that finished with a section missing", () => {
    expect(wasRead({ mem: null, caches: [] })).toBe(false);
    expect(wasRead({ mem: null, caches: [{ path: "~/.cache/vllm", exists: true, owner: "sparkuser" }] })).toBe(true);
    const part = preflight(spec2, { node: "cell1", mem: null });
    const { container } = render(<BeforeItStarts set={setOf([part])} problem={null} share={0.75} port={8000} drop={false} onDrop={() => {}} />);
    expect(container).toHaveTextContent("Memory");
    expect(container).toHaveTextContent("Couldn't read it");
    expect(container).toHaveTextContent("present on cell1");
  });

  it("carries a refusal only the set can make", () => {
    const refused = "vllm/vllm-openai:glm53-flash-arm64-cu130 isn't the same image on every node: cell1 has 5a0f8b91, cell2 has abcdef01. Pull the same tag on each node.";
    const nodes = [preflight(spec2, { node: "cell1" }), preflight(spec2, { node: "cell2" })];
    render(<BeforeItStarts set={setOf(nodes, { verdict: "blocked", problems: [refused] })} problem={null} share={0.75} port={8000} drop={false} onDrop={() => {}} />);
    expect(screen.getByText(refused)).toBeInTheDocument();
    expect(verdictLine(setOf(nodes, { verdict: "blocked", problems: [refused] }))).toBe("Not ready to load on cell1 and cell2: 1 problem to fix first.");
  });

  it("offers one page-cache drop for the nodes that need it, by name, and sends one consent", async () => {
    const mem = { total: 127_598_748 * 1024, free: 43_201_000 * 1024, available: 122_601_504 * 1024, cached: 75 * GIB };
    const set = (spec: DeploySpec) => setOf(spec.nodes.map((node) => preflight(spec, node === "gx10-0000" ? { node, mem, dropNeeded: true } : { node })));
    const root = await show({ ...two, set });
    await settle();
    await pickNodes(...CLUSTER_NODES.filter((n) => nodeBox(n.name).checked).map((n) => n.name));
    await pickNodes("cell1", "cell4");
    await settle();
    const drop = screen.getByLabelText("Drop the page cache first on cell4 (sudo; the kernel refills it)") as HTMLInputElement;
    expect(blocked(root)).toBe("Not enough free memory: tick the page-cache drop or lower the share");
    await act(async () => {
      fireEvent.click(drop);
    });
    await settle();
    expect(blocked(root)).toBeNull();
    expect(lastSpec("deploy_preview")).toMatchObject({ dropCaches: true });
    // Memory short with no consent is in the verdict, so the tick reads the nodes again.
    expect(lastSpec("model_preflight")).toMatchObject({ dropCaches: true });
  });

  it("sends no consent once no chosen node needs the drop", async () => {
    const mem = { total: 127_598_748 * 1024, free: 43_201_000 * 1024, available: 122_601_504 * 1024, cached: 75 * GIB };
    const set = (spec: DeploySpec) => setOf(spec.nodes.map((node) => preflight(spec, node === "cell3" ? { node, mem, dropNeeded: true } : { node })));
    await show({ ...two, set });
    await settle();
    await pickNodes(...CLUSTER_NODES.filter((n) => nodeBox(n.name).checked).map((n) => n.name));
    await pickNodes("cell3");
    await settle();
    await act(async () => {
      fireEvent.click(screen.getByLabelText("Drop the page cache first on cell3 (sudo; the kernel refills it)"));
    });
    await settle();
    expect(lastSpec("deploy_preview")).toMatchObject({ dropCaches: true });
    await pickNodes("cell3", "cell1");
    await settle();
    expect(lastSpec("model_preflight")).toMatchObject({ nodes: ["cell1"], dropCaches: false });
    expect(lastSpec("deploy_preview")).toMatchObject({ nodes: ["cell1"], dropCaches: false });
  });

  it("drops a slower read of nodes no longer chosen", async () => {
    let answer: ((s: PreflightSet) => void) | null = null;
    const root = await show({ ...two, draft: { key: GEMMA }, set: (spec) => (spec.nodes.includes("cell3") ? new Promise((res) => { answer = res; }) : setOf(spec.nodes.map((node) => preflight(spec, { node })))) as unknown as PreflightSet });
    await settle();
    await pickNodes(...CLUSTER_NODES.filter((n) => nodeBox(n.name).checked).map((n) => n.name));
    await pickNodes("cell3");
    await settle();
    await pickNodes("cell3", "cell1");
    await settle();
    expect(root).toHaveTextContent("Ready to load on cell1.");
    await act(async () => {
      answer?.(setOf([preflight(lastSpec("model_preflight")!, { node: "cell3" })]));
    });
    await settle();
    expect(root).toHaveTextContent("Ready to load on cell1.");
  });

  it("in look-only still reads the nodes, and Load stays locked with look-only's words", async () => {
    const locked = { ...UNLOCKED, unlocked: false, lookOnly: true };
    const root = await show({ admin: locked });
    await settle();
    expect(root).toHaveTextContent("Ready to load on cell2.");
    const load = screen.getByRole("button", { name: "Load on 1 node" });
    expect(load).toBeDisabled();
    expect(load).toHaveAttribute("title", LOOK_ONLY_TIP);
    expect(screen.queryByRole("textbox", { name: "Expert: raw vLLM args" })).toBeNull();
    // Nothing on the page asked to change anything.
    expect(mockCalls.some((c) => /^(deploy|deployment_|model_share)/.test(c.cmd) && !["deploy_plan", "deploy_preview"].includes(c.cmd))).toBe(false);
  });
});

describe("a pre-flight that fails", () => {
  it("offers Check again, whose read clears it", async () => {
    let fail = true;
    const root = await show({ set: (spec) => {
      if (fail) throw "Couldn't reach the nodes";
      return setOf([preflight(spec)]);
    } });
    await settle();
    expect(screen.getByText("Couldn't reach the nodes")).toBeInTheDocument();
    fail = false;
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    });
    expect(screen.queryByText("Couldn't reach the nodes")).toBeNull();
    await settle();
    expect(root).toHaveTextContent("Ready to load on cell2.");
  });

  it("keeps the last read beside a read that failed, with its age", async () => {
    let fail = false;
    const root = await show({ set: (spec) => {
      if (fail) throw "Couldn't reach the nodes";
      return setOf([preflight(spec)]);
    } });
    await settle();
    fail = true;
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    });
    await settle();
    expect(screen.getByText("Couldn't reach the nodes")).toBeInTheDocument();
    expect(root).toHaveTextContent("Checked 2 min ago");
    expect(screen.getByRole("button", { name: "Check again" })).toBeInTheDocument();
  });
});

describe("the page-cache tick is for the nodes it names", () => {
  const mem = { total: 127_598_748 * 1024, free: 43_201_000 * 1024, available: 122_601_504 * 1024, cached: 75 * GIB };
  const two = [{ id: "cell1", name: "cell1", online: true }, { id: "cell3", name: "cell3", online: true }];
  const opts = { nodes: two, library: [{ ...library[0], copies: { cell1: COPY, cell3: COPY } }], images: two.map((n) => listing(n.id)), draft: { key: GEMMA } as Draft };
  const tickFor = (name: string) => screen.getByLabelText(`Drop the page cache first on ${name} (sudo; the kernel refills it)`) as HTMLInputElement;

  async function tickedOnCell1(set: (spec: DeploySpec) => PreflightSet) {
    const root = await show({ ...opts, set });
    await settle();
    if (!nodeBox("cell1").checked) await pickNodes("cell1");
    if (nodeBox("cell3").checked) await pickNodes("cell3");
    await act(async () => {
      fireEvent.click(tickFor("cell1"));
    });
    await settle();
    expect(lastSpec("deploy_preview")).toMatchObject({ nodes: ["cell1"], dropCaches: true });
    return root;
  }

  it("asks again when the node is swapped for another", async () => {
    const root = await tickedOnCell1((spec) => setOf(spec.nodes.map((node) => preflight(spec, { node, mem, dropNeeded: true }))));
    await act(async () => {
      fireEvent.click(nodeBox("cell1"));
      fireEvent.click(nodeBox("cell3"));
    });
    await settle();
    expect(tickFor("cell3").checked).toBe(false);
    expect(lastSpec("model_preflight")).toMatchObject({ nodes: ["cell3"], dropCaches: false });
    expect(lastSpec("deploy_preview")).toMatchObject({ nodes: ["cell3"], dropCaches: false });
    expect(blocked(root)).toBe("Not enough free memory: tick the page-cache drop or lower the share");
  });

  it("asks again when a node is added, or the swap is undone", async () => {
    const short = (spec: DeploySpec) => setOf(spec.nodes.map((node) => preflight(spec, node === "cell1" ? { node, mem, dropNeeded: true } : { node })));
    await tickedOnCell1(short);
    await pickNodes("cell3");
    expect(tickFor("cell1").checked).toBe(false);
    expect(lastSpec("deploy_preview")).toMatchObject({ dropCaches: false });
    await act(async () => {
      fireEvent.click(tickFor("cell1"));
    });
    await settle();
    expect(lastSpec("deploy_preview")).toMatchObject({ dropCaches: true });
    await pickNodes("cell3");
    expect(tickFor("cell1").checked).toBe(false);
  });

  it("asks again when another chosen node comes up short, and keeps a tick through a re-read of the same nodes", async () => {
    // At a share above 0.6 cell3 is short as well as cell1.
    const set = (spec: DeploySpec) => setOf(spec.nodes.map((node) => preflight(spec, node === "cell1" || spec.gpuMemoryUtilization > 0.6 ? { node, mem, dropNeeded: true } : { node })));
    const root = await show({ ...opts, set, draft: { key: GEMMA, spec: { id: "d1", servedName: "g", modelPath: GEMMA, nodes: ["cell1", "cell3"], tensorParallel: 2, pipelineParallel: 1, expertParallel: false, maxModelLen: null, gpuMemoryUtilization: 0.5, port: 8000, image: DEFAULT_IMAGE, imageRef: DEFAULT_IMAGE, extraArgs: [] } } });
    await settle();
    await act(async () => {
      fireEvent.click(tickFor("cell1"));
    });
    await settle();
    expect(lastSpec("deploy_preview")).toMatchObject({ dropCaches: true });
    // Check again reads the same nodes: the tick stands.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    });
    await settle();
    expect(tickFor("cell1").checked).toBe(true);
    await act(async () => {
      fireEvent.change(shareField(root), { target: { value: "0.7" } });
    });
    await settle();
    expect(tickFor("cell1 and cell3").checked).toBe(false);
    expect(lastSpec("deploy_preview")).toMatchObject({ gpuMemoryUtilization: 0.7, dropCaches: false });
  });
});

describe("numbers the page can't send", () => {
  /** A spec Rust's types would refuse at the invoke boundary, in the words of serde. */
  const unsendable = (s: DeploySpec) => [s.tensorParallel, s.pipelineParallel, s.port, s.maxModelLen ?? 1, s.maxNumSeqs ?? 1].some((n) => !Number.isInteger(n) || n < 0) || s.port > 65535 || !Number.isFinite(s.gpuMemoryUtilization);
  const sent = () => mockCalls.filter((c) => ["deploy_plan", "deploy_preview", "model_preflight"].includes(c.cmd)).map((c) => c.args.spec as DeploySpec);
  const context = (root: HTMLElement) => root.querySelector("input.narrow-wide") as HTMLInputElement;
  const set = async (field: HTMLInputElement, value: string) => {
    await act(async () => {
      fireEvent.change(field, { target: { value } });
    });
    await settle();
  };

  it("checks a whole-number field by its own limits", () => {
    expect(wholeProblem("Max context", "", 1, 100)).toBeNull();
    expect(wholeProblem("Max context", " 32 ", 1, 100)).toBeNull();
    expect(wholeProblem("Max context", "1e2", 1, 100)).toBeNull();
    for (const bad of ["-1", "0", "1.5", "101", "1e999"]) expect(wholeProblem("Max context", bad, 1, 100)).toBe("Max context must be a whole number from 1 to 100.");
    expect(splitPart("1.5")).toBe(1);
    expect(splitPart("2.9")).toBe(2);
    expect(splitPart("")).toBe(1);
    expect(splitPart("-3")).toBe(1);
    expect(splitPart("1e30")).toBe(1024);
  });

  it("gives Max context's limits in words, holds Load, and sends nothing", async () => {
    const root = await show({ offers: gemmaOffers, recipe: () => GEMMA_RECIPE });
    await settle();
    const before = sent().length;
    const says = "Max context must be a whole number from 1 to 1,000,000,000.";
    for (const bad of ["-1", "1.5", "0"]) {
      await set(context(root), bad);
      expect(blocked(root)).toBe(says);
      expect(screen.getByRole("button", { name: "Load on 1 node" })).toBeDisabled();
    }
    expect(screen.getAllByRole("alert").map((a) => a.textContent)).toContain(says);
    expect(sent().slice(before).filter(unsendable)).toEqual([]);
    expect(root).not.toHaveTextContent("expected u64");
    // The plan isn't said to want nodes, and the split stays to choose from.
    expect(root).not.toHaveTextContent("Pick the nodes");
    expect(screen.getByRole("combobox", { name: "Split" })).toBeInTheDocument();
    await set(context(root), "8192");
    expect(blocked(root)).toBeNull();
    expect(lastSpec("deploy_preview")).toMatchObject({ maxModelLen: 8192 });
  });

  it("gives Requests at once and the share theirs", async () => {
    const root = await show();
    await settle();
    const seqs = screen.getAllByPlaceholderText("auto")[0] as HTMLInputElement;
    await set(seqs, "1.5");
    expect(blocked(root)).toBe("Requests at once must be a whole number from 1 to 10000.");
    await set(seqs, "");
    expect(blocked(root)).toBeNull();
    for (const bad of ["", "2", "0.1", "1e999"]) {
      await set(shareField(root), bad);
      expect(blocked(root)).toBe("GPU memory share must be a number from 0.2 to 0.95.");
    }
    expect(sent().filter(unsendable)).toEqual([]);
  });

  it("gives the port's range beside it, and never reads the nodes for a port that can't be", async () => {
    const root = await show();
    await settle();
    const field = screen.getByRole("spinbutton", { name: "Port" }) as HTMLInputElement;
    const says = "Port must be a whole number from 1024 to 65535.";
    for (const bad of ["70000", "1.5", "80", ""]) {
      await set(field, bad);
      expect(blocked(root)).toBe(says);
      expect(field).toHaveAttribute("aria-invalid", "true");
      expect(root).not.toHaveTextContent("free on cell2");
      expect(root).not.toHaveTextContent("Reading the nodes");
      expect(screen.queryByRole("button", { name: "Check again" })).toBeNull();
    }
    expect(sent().filter(unsendable)).toEqual([]);
    await set(field, "8001");
    expect(blocked(root)).toBeNull();
    expect(lastSpec("model_preflight")).toMatchObject({ port: 8001 });
    expect(root).toHaveTextContent("Ready to load on cell2.");
  });

  it("reads the nodes once a wrong number is fixed, if the nodes changed meanwhile", async () => {
    const nodes = [{ id: "cell1", name: "cell1", online: true }, { id: "cell2", name: "cell2", online: true }];
    const root = await show({ nodes, library: [{ ...library[0], copies: { cell1: COPY, cell2: COPY } }], images: nodes.map((n) => listing(n.id)), set: (spec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))) });
    await settle();
    await set(context(root), "-1");
    await pickNodes(nodes.find((n) => !nodeBox(n.name).checked)!.name);
    await set(context(root), "8192");
    expect(lastSpec("model_preflight")?.nodes).toHaveLength(2);
    expect(root).toHaveTextContent("Ready to load on cell1 and cell2.");
  });

  it("takes a whole number from the split's own fields", async () => {
    await show({ ...{ nodes: CLUSTER_NODES, library: everywhere, offers: gemmaOffers, images: CLUSTER_NODES.map((n) => listing(n.id)), set: (spec: DeploySpec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))) } });
    await settle();
    await set(numberField("Tensor parallel"), "1.5");
    expect(numberField("Tensor parallel").value).toBe("1");
    await set(numberField("Pipeline parallel"), "1e30");
    expect(sent().filter(unsendable)).toEqual([]);
    expect(lastSpec("deploy_preview")?.pipelineParallel).toBe(1024);
  });

  it("says a plan that failed failed, with the nodes picked", async () => {
    const root = await show({ handlers: { deploy_plan: () => { throw "Tensor parallel (2) × pipeline parallel (1) must equal the number of nodes (1): one GPU per Spark."; } } });
    await settle();
    expect(root).toHaveTextContent("Couldn't work out the plan: Tensor parallel (2) × pipeline parallel (1) must equal the number of nodes (1): one GPU per Spark.");
    expect(root).not.toHaveTextContent("Pick the nodes");
  });

  it("says the plan needs nodes when none are picked, and a folder when none is", async () => {
    const none = await show();
    await settle();
    await pickNodes("cell2");
    expect(none).toHaveTextContent("Pick the nodes to see the plan.");
    const empty = await show({ draft: { key: "" }, library: [] });
    expect(empty).toHaveTextContent("Choose a model folder to see where it fits.");
  });
});

describe("answers that come back out of order", () => {
  it("drops a slower plan for the nodes chosen before", async () => {
    const nodes = [{ id: "cell1", name: "cell1", online: true }, { id: "cell2", name: "cell2", online: true }];
    const slowed = new Set<number>();
    let release: () => void = () => {};
    const root = await show({
      nodes, library: [{ ...library[0], copies: { cell1: COPY, cell2: COPY } }], images: nodes.map((n) => listing(n.id)), set: (spec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))),
      handlers: {
        deploy_plan: async ({ spec }: { spec: DeploySpec }) => {
          // The first read of the single-node plan after the second node is ticked takes longer than the next.
          if (spec.nodes.length === 1 && slowed.has(1)) await new Promise<void>((res) => { release = res; });
          return { nodes: [], level: "green", needs: { weightBytes: 54e9, arch: {}, kvBytesPerToken: null }, portTakenBy: null, suggestedPort: 8000, host: "h", url: "u", keyPreview: null, maxContext: 1, suggestedShare: null, suggestedContext: null, recipe: null, topologies: gemmaOffers(spec.nodes.length) };
        },
      },
    });
    await settle();
    slowed.add(1);
    // Asked again about one node, then two: the two-node answer arrives first.
    await act(async () => {
      fireEvent.change(root.querySelector("input.narrow-wide")!, { target: { value: "8192" } });
    });
    for (let i = 0; i < 40 && !mockCalls.some((c) => c.cmd === "deploy_plan" && (c.args.spec as DeploySpec).maxModelLen === 8192); i++) await tick();
    await pickNodes(nodes.find((n) => !nodeBox(n.name).checked)!.name);
    expect((screen.getByRole("combobox", { name: "Split" }) as HTMLSelectElement).value).toBe("tp2");
    await act(async () => {
      release();
    });
    await settle();
    expect((screen.getByRole("combobox", { name: "Split" }) as HTMLSelectElement).value).toBe("tp2");
  });
});

describe("a renamed node", () => {
  const renamed = [{ id: "cell1", name: "cell1", online: true }, { id: "gx10-0000", name: "cell4", online: true }];
  const amber = { node: "gx10-0000", level: "amber", reason: "Tight: gx10-0000 reaches 92%", afterPct: 92, verdict: "Tight", inUse: 1, weights: 1, overhead: 1, kv: 1, need: 1, budget: 2, total: 10, holders: [] };
  const planWith = (nodes: unknown[], level: string) => ({ nodes, level, needs: { weightBytes: 54e9, arch: {}, kvBytesPerToken: null }, portTakenBy: null, suggestedPort: 8000, host: "h", url: "u", keyPreview: null, maxContext: 1, suggestedShare: null, suggestedContext: null, recipe: null, topologies: [] });

  it("reads by whole ids in one pass, so swapped names stay apart", () => {
    const swapped = [{ id: "cell1", name: "cell2" }, { id: "cell2", name: "cell1" }];
    expect(inNames("From cell1 to cell2.", swapped)).toBe("From cell2 to cell1.");
    expect(inNames("gx10-0000's memory, then gx10-0000.", [{ id: "gx10-0000", name: "cell4" }])).toBe("cell4's memory, then cell4.");
    expect(inNames("cell10 and sparkbox-cell1-r0 and cell1", [{ id: "cell1", name: "alpha" }])).toBe("cell10 and sparkbox-cell1-r0 and alpha");
    expect(inNames("node.1 and nodex1", [{ id: "node.1", name: "n" }])).toBe("n and nodex1");
    expect(inNames("cell1", [{ id: "cell1", name: "cell1" }])).toBe("cell1");
  });

  it("is named in What will run, the plan's reason and the amber dialog", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const root = await show({
      nodes: renamed, library: [{ ...library[0], copies: { cell1: COPY, "gx10-0000": COPY } }], images: renamed.map((n) => listing(n.id)), set: (spec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))),
      preview: () => [{ node: "gx10-0000", title: "Test the jumbo-frame path from gx10-0000 to cell1 (stops here if it fails)", command: "x" }],
      handlers: { deploy_plan: () => planWith([amber], "amber") },
    });
    await settle();
    expect(root).toHaveTextContent("Test the jumbo-frame path from cell4 to cell1");
    expect(root).toHaveTextContent("Tight: cell4 reaches 92%");
    expect(root).not.toHaveTextContent("gx10-0000");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^Load on/ }));
    });
    await settle();
    expect(asked.mock.calls[0][0].body).toContain("cell4 would use 92% of its memory");
    expect(asked.mock.calls[0][0].body).not.toContain("gx10-0000");
  });

  it("is named in the red dialog and in a refusal from Rust", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const red = { ...amber, level: "red", reason: "Won't fit: gx10-0000 has too little", verdict: "Won't fit", fitsSafe: true };
    const root = await show({
      nodes: renamed, library: [{ ...library[0], copies: { cell1: COPY, "gx10-0000": COPY } }], images: renamed.map((n) => listing(n.id)), set: (spec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))),
      handlers: { deploy_plan: () => planWith([red], "red") },
    });
    await settle();
    await act(async () => {
      fireEvent.click(root.querySelector(".ai-override input")!);
    });
    await settle();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^Load on/ }));
    });
    await settle();
    expect(asked.mock.calls[0][0].body).toContain("Won't fit: cell4 has too little");

    const refused = await show({ nodes: [renamed[1]], library: [{ ...library[0], copies: { "gx10-0000": COPY } }], images: [listing("gx10-0000")], pf: (spec) => preflight(spec, { node: "gx10-0000" }), preview: () => { throw "gx10-0000 can't take this."; } });
    await settle();
    expect(blocked(refused)).toBe("cell4 can't take this.");
    expect(refused).not.toHaveTextContent("gx10-0000");
  });
});

describe("Allow remote code", () => {
  const SAYS = "On several nodes the containers run with --privileged, so that Python runs as root with every device on the node.";
  const opts = { nodes: CLUSTER_NODES, library: everywhere, images: CLUSTER_NODES.map((n) => listing(n.id)), offers: gemmaOffers, set: (spec: DeploySpec) => setOf(spec.nodes.map((node) => preflight(spec, { node }))) };

  it("says what --privileged means on several nodes, and only there", async () => {
    const root = await show(opts);
    await settle();
    expect(root).not.toHaveTextContent(SAYS);
    await pickNodes(CLUSTER_NODES.find((n) => !nodeBox(n.name).checked)!.name);
    expect(root).toHaveTextContent(SAYS);
    await act(async () => {
      fireEvent.click(screen.getByRole("switch", { name: "Allow remote code" }));
    });
    await settle();
    expect(root).toHaveTextContent("with access to every device on those nodes, as root.");
    expect(root).not.toHaveTextContent("with access to the GPU and the model folder");
  });

  it("keeps one node's words", async () => {
    const root = await show(opts);
    await settle();
    await act(async () => {
      fireEvent.click(screen.getByRole("switch", { name: "Allow remote code" }));
    });
    await settle();
    expect(root).toHaveTextContent("with access to the GPU and the model folder.");
  });

  it("says nothing of it for SGLang, which never runs privileged", async () => {
    const root = await show({ ...opts, draft: { key: GEMMA, engine: "sglang" } });
    await settle();
    await pickNodes(CLUSTER_NODES.find((n) => !nodeBox(n.name).checked)!.name);
    expect(root).not.toHaveTextContent(SAYS);
  });
});

describe("pressing Load", () => {
  it("sends the split, the image and the recipe it showed, and no key", async () => {
    const started = vi.fn(() => ({}));
    const root = await show({ recipe: () => GEMMA_RECIPE, offers: gemmaOffers, handlers: { deploy: started } });
    await settle();
    expect(blocked(root)).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Load on 1 node" }));
    });
    await settle();
    expect(mockCalls.filter((c) => c.cmd === "deploy").map((c) => c.args.spec)).toEqual([
      expect.objectContaining({ nodes: ["cell2"], tensorParallel: 1, pipelineParallel: 1, image: DEFAULT_IMAGE, imageRef: DEFAULT_IMAGE, gpuMemoryUtilization: 0.75, maxModelLen: 32768, dropCaches: false, extraArgs: [], engine: "vllm" }),
    ]);
  });

  it("asks for the head's name by its current name when the plan is red", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const redPlan = { node: "gx10-0000", level: "red", reason: "Won't fit", fitsSafe: true, weights: 1, kv: 1, overhead: 1, need: 2, budget: 1, free: 1, available: 1, total: 4, holders: [], verdict: "Won't fit", inUse: 0, afterPct: 99, maxShare: 0.9, safeShare: 0.5, safeContext: null };
    const root = await show({
      nodes: [{ id: "gx10-0000", name: "cell4", online: true }], images: [listing("gx10-0000")], library: [{ ...library[0], copies: { "gx10-0000": COPY } }], pf: (spec) => preflight(spec, { node: "gx10-0000" }),
      handlers: { deploy_plan: () => ({ nodes: [redPlan], level: "red", needs: { weightBytes: 54e9, arch: {}, kvBytesPerToken: null }, portTakenBy: null, suggestedPort: 8000, host: "h", url: "u", keyPreview: null, maxContext: null, suggestedShare: null, suggestedContext: null, recipe: null, topologies: [] }) },
    });
    await settle();
    await act(async () => {
      fireEvent.click(root.querySelector(".ai-override input")!);
    });
    await settle();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Load on 1 node" }));
    });
    await settle();
    expect(asked).toHaveBeenCalledWith(expect.objectContaining({ requireTyping: "cell4" }));
    expect(root).not.toHaveTextContent("gx10-0000");
  });
});

describe("the Prepare tab", () => {
  const check = {
    node: "cell1", error: null, home: "/home/sparkuser", rootPath: "/home/sparkuser/models", rootExists: true, freeBytes: 2e12, docker: true, rsync: true, python: "3.12", imagePresent: true,
    vllmVersion: "0.28.1", gpuInContainer: "GPU 0: NVIDIA Tegra NVIDIA GB10B (UUID: GPU-1)",
    fabric: [
      { iface: "enp1s0f1np1", ip: "192.168.100.1", prefix: 24, speedMbps: 200000, rdma: "rocep1s0f1", mtu: 9000, operstate: "up", rdmaState: "4: ACTIVE", rdmaPhys: "5: LinkUp" },
      { iface: "enP2p1s0f1np1", ip: "192.168.101.1", prefix: 24, speedMbps: 200000, rdma: "roceP2p1s0f1", mtu: 1500, operstate: "up", rdmaState: "1: DOWN", rdmaPhys: "3: Disabled" },
    ],
  };
  afterEach(() => localStorage.clear());

  async function prepare(handlers: MockHandlers = {}) {
    resetStore();
    mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => snapshot(false), admin_state: () => UNLOCKED, ...handlers }));
    _resetAdminForTests(UNLOCKED);
    await startClusterStore();
    let r!: ReturnType<typeof render>;
    await act(async () => {
      r = render(<WindowManagerProvider sizeOf={() => [1600, 900]} isKnown={() => true}><PrepareTab /></WindowManagerProvider>);
    });
    await tick();
    return r;
  }

  it("shows the last result after a restart, with its age, its fabric halves and the GPU", async () => {
    for (const k of ["demo", "live"]) localStorage.setItem(`sparkbox.prepare.v1:${k}`, JSON.stringify({ atMs: Date.now() - 3 * 60_000, checks: [check] }));
    const { container } = await prepare();
    expect(container).toHaveTextContent("Checked 3 min ago");
    expect(container).toHaveTextContent("MTU 9000 · RDMA ACTIVE");
    expect(container).toHaveTextContent("MTU 1500 · RDMA DOWN");
    expect(container).toHaveTextContent("GPU 0: NVIDIA Tegra NVIDIA GB10B");
    expect(container).toHaveTextContent("on this node");
    expect(mockCalls.some((c) => c.cmd === "cluster_check")).toBe(false);
  });

  it("says a node's name today, in the table and in its error, from a result saved under its id", async () => {
    const s = snapshot(false);
    s.nodes.find((n) => n.id === "cell1")!.name = "renamed";
    const failed = { ...check, error: "cell1 didn't answer" };
    for (const k of ["demo", "live"]) localStorage.setItem(`sparkbox.prepare.v1:${k}`, JSON.stringify({ atMs: Date.now() - 3 * 60_000, checks: [failed] }));
    const { container } = await prepare({ get_snapshot: () => s });
    expect(container).toHaveTextContent("renamed: renamed didn't answer");
    expect(container.querySelector("td .strong")).toHaveTextContent("renamed");
    expect(container).not.toHaveTextContent("cell1");
  });

  it("keeps a new result for next time, and an empty page says what the check does", async () => {
    const first = await prepare({ cluster_check: () => [check] });
    expect(first.container).toHaveTextContent("Check nodes starts short containers on each node to read the vLLM version and see the GPU from inside the image (needs Admin).");
    await act(async () => {
      fireEvent.click(screen.getAllByRole("button", { name: /^Check nodes/ })[0]);
    });
    await settle();
    expect(first.container).toHaveTextContent("Checked just now");
    first.unmount();
    const again = await prepare();
    expect(again.container).toHaveTextContent("MTU 9000 · RDMA ACTIVE");
  });
});

describe("Remove leftovers", () => {
  const left = (over = {}) => ({ id: "abc123", name: "sparkbox-old-r1", deployment: "old", instance: "dev", status: "Exited (137) 3 hours ago", ours: true, ...over });
  const pf = (spec: DeploySpec) => preflight(spec, { leftovers: [left()], problems: ["cell2 has containers left from an earlier load (sparkbox-old-r1). Remove them first."] });
  const done = { id: "t-clean", kind: "op", title: "Remove leftovers", nodes: ["cell2"], source: "ui", state: "ok", cancellable: false } as unknown as Parameters<typeof ingestTask>[0];

  it("asks by name, then removes by deployment on that node and reads the nodes again", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await show({ pf, handlers: { deployment_clean_leftovers: () => { ingestTask(done); return "t-clean"; } } });
    const reads = () => mockCalls.filter((c) => c.cmd === "model_preflight").length;
    const before = reads();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove leftovers…" }));
    });
    await settle();
    expect(asked).toHaveBeenCalledWith(expect.objectContaining({ title: "Remove what an earlier load left on cell2?", details: "sparkbox-old-r1", confirmLabel: "Remove leftovers", danger: true }));
    expect(mockCalls.filter((c) => c.cmd === "deployment_clean_leftovers").map((c) => c.args)).toEqual([{ nodes: ["cell2"], ids: ["old"] }]);
    expect(reads()).toBeGreaterThan(before);
  });

  it("names only this build's leftovers and sends only their ids, with another build's on the same node", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const mine = left({ name: "sparkbox-mine-r1", deployment: "mine" });
    const theirs = left({ name: "sparkbox-theirs-r1", deployment: "theirs", instance: "release", ours: false });
    await show({ pf: (spec) => preflight(spec, { leftovers: [theirs, mine] }), handlers: { deployment_clean_leftovers: () => { ingestTask(done); return "t-clean"; } } });
    const button = screen.getByRole("button", { name: "Remove leftovers…" });
    expect(button).toHaveAttribute("title", "Remove sparkbox-mine-r1");
    await act(async () => {
      fireEvent.click(button);
    });
    await settle();
    expect(asked.mock.calls.map((c) => c[0].details)).toEqual(["sparkbox-mine-r1"]);
    expect(mockCalls.filter((c) => c.cmd === "deployment_clean_leftovers").map((c) => c.args)).toEqual([{ nodes: ["cell2"], ids: ["mine"] }]);
  });

  it("keeps the blocks, says what failed and can read the nodes again", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const root = await show({ pf, handlers: { deployment_clean_leftovers: () => { throw "cell2: the fence said no"; } } });
    const reads = () => mockCalls.filter((c) => c.cmd === "model_preflight").length;
    const before = reads();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove leftovers…" }));
    });
    await settle();
    expect(screen.getByText("cell2: the fence said no")).toBeInTheDocument();
    expect(root).toHaveTextContent("Not ready to load on cell2: 1 problem to fix first.");
    expect(root).toHaveTextContent("sparkbox-old-r1 (Exited (137) 3 hours ago)");
    // Part of it may be gone: the nodes were read again, and the failure outlived that read.
    expect(reads()).toBeGreaterThan(before);
    const again = reads();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    });
    await settle();
    expect(reads()).toBeGreaterThan(again);
    expect(screen.queryByText("cell2: the fence said no")).toBeNull();
  });

  it("says when the removal's task ends in failure, which arrives with no error to catch", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const failed = { ...done, id: "t-fail", state: "failed", error: "cell2: docker didn't answer" } as typeof done;
    await show({ pf, handlers: { deployment_clean_leftovers: () => { ingestTask(failed); return "t-fail"; } } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove leftovers…" }));
    });
    await settle();
    expect(screen.getByText("cell2: docker didn't answer")).toBeInTheDocument();
  });

  it("does nothing when the confirm is cancelled", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await show({ pf, handlers: { deployment_clean_leftovers: () => "t-clean" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove leftovers…" }));
    });
    await settle();
    expect(mockCalls.some((c) => c.cmd === "deployment_clean_leftovers")).toBe(false);
  });

  it("shows the other build's leftovers and offers nothing for them", async () => {
    const root = await show({ pf: (spec) => preflight(spec, { leftovers: [left({ instance: "release", name: "sparkbox-theirs", ours: false })] }) });
    expect(root).toHaveTextContent("sparkbox-theirs (Exited (137) 3 hours ago)");
    expect(root).toHaveTextContent("Another build's containers are shown, never removed");
    expect(screen.queryByRole("button", { name: "Remove leftovers…" })).toBeNull();
  });

  it("shows a running download as what it is, and offers nothing for it", async () => {
    const root = await show({ pf: (spec) => preflight(spec, { downloads: [left({ name: "sparkbox-dl-job1", deployment: "job1", status: "Up 2 minutes" })] }) });
    expect(root).toHaveTextContent("Download running");
    expect(root).toHaveTextContent("sparkbox-dl-job1 (Up 2 minutes)");
    expect(screen.getByText("nothing")).toBeInTheDocument(); // the leftovers' row
    expect(screen.queryByRole("button", { name: "Remove leftovers…" })).toBeNull();
  });

  it("is off in look-only and asks to unlock when locked", async () => {
    await show({ pf, admin: { ...UNLOCKED, unlocked: false, lookOnly: true } });
    const off = screen.getByRole("button", { name: "Remove leftovers…" });
    expect(off).toBeDisabled();
    expect(off).toHaveAttribute("title", LOOK_ONLY_TIP);
    await show({ pf, admin: { ...UNLOCKED, unlocked: false } });
    const locked = screen.getAllByRole("button", { name: "Remove leftovers…" }).at(-1)!;
    expect(locked).toBeEnabled();
    expect(locked).toHaveAttribute("title", `Remove sparkbox-old-r1 · ${NEEDS_ADMIN_TIP}`);
  });
});

describe("Expert: raw vLLM args", () => {
  it("splits a line at its first space, so JSON with spaces stays one value", () => {
    expect(parseExpertArgs('--speculative-config {"method": "ngram", "num_tokens": 4}\n\n  --enforce-eager  \n--block-size 256\n')).toEqual([
      "--speculative-config", '{"method": "ngram", "num_tokens": 4}', "--enforce-eager", "--block-size", "256",
    ]);
    expect(parseExpertArgs("")).toEqual([]);
    expect(parseExpertArgs("   \n  ")).toEqual([]);
    expect(parseExpertArgs("--hf-overrides    {\"a\": 1}")).toEqual(["--hf-overrides", '{"a": 1}']);
  });

  it("writes saved args back line by line, and they read the same", () => {
    const args = ["--enforce-eager", "--quantization", "fp8", "--speculative-config", '{"a": "b c"}', "--seed", "-1", "--limit", "1", "2"];
    expect(expertText(args)).toBe('--enforce-eager\n--quantization fp8\n--speculative-config {"a": "b c"}\n--seed -1\n--limit 1\n2');
    expect(parseExpertArgs(expertText(args))).toEqual(args);
    expect(expertText([])).toBe("");
  });

  it("tells a refused option from other problems", () => {
    expect(expertRefusal("--middleware loads code or opens files, so SparkBox won't pass it on.", "--middleware x")).toContain("--middleware");
    expect(expertRefusal("--block-size needs a value.", "--block_size=")).toContain("--block-size");
    expect(expertRefusal("“foo” isn't an option (options start with --).", "foo bar")).toContain("foo");
    expect(expertRefusal("Don't put an API key in the options: SparkBox gives vLLM its key from a file only it can read.", "--api-key sbk")).toContain("API key");
    expect(expertRefusal("Port 8000 is taken", "--enforce-eager")).toBeNull();
    expect(expertRefusal("“foo” isn't an option (options start with --).", "")).toBeNull();
    expect(expertRefusal(null, "--enforce-eager")).toBeNull();
  });

  it("is there when Admin is unlocked, sends one element per value, and shows Rust's sentence", async () => {
    const refused = "--middleware loads code or opens files, so SparkBox won't pass it on.";
    const root = await show({ preview: (spec) => {
      if (spec.extraArgs.includes("--middleware")) throw refused;
      return [];
    } });
    const field = screen.getByRole("textbox", { name: "Expert: raw vLLM args" });
    await act(async () => {
      fireEvent.change(field, { target: { value: '--speculative-config {"a": 1, "b": 2}\n--enforce-eager' } });
    });
    await settle();
    expect(lastSpec("deploy_preview")?.extraArgs).toEqual(["--speculative-config", '{"a": 1, "b": 2}', "--enforce-eager"]);
    expect(blocked(root)).toBeNull();
    await act(async () => {
      fireEvent.change(field, { target: { value: "--middleware evil.mod" } });
    });
    await settle();
    expect(screen.getAllByText(refused)).toHaveLength(2); // under the field, and the toolbar's reason
    expect(blocked(root)).toBe(refused);
    expect(screen.getByRole("button", { name: "Load on 1 node" })).toBeDisabled();
  });

  it("is hidden while Admin is locked, and what's set still shows", async () => {
    const was: DeploySpec = { id: "d1", servedName: "g", modelPath: GEMMA, nodes: ["cell2"], tensorParallel: 1, pipelineParallel: 1, expertParallel: false, maxModelLen: null, gpuMemoryUtilization: 0.75, port: 8000, image: DEFAULT_IMAGE, imageRef: DEFAULT_IMAGE, extraArgs: ["--block-size", "256"] };
    const root = await show({ admin: { ...UNLOCKED, unlocked: false }, draft: { key: GEMMA, spec: was, replaces: "d1" } });
    expect(screen.queryByRole("textbox", { name: "Expert: raw vLLM args" })).toBeNull();
    expect(root).toHaveTextContent("--block-size 256");
  });
});

describe("what a load says while it starts", () => {
  const starting = (stage: Deployment["stage"]) => ({ state: "starting" as const, stage, message: "Starting" });

  it("counts nodes up, and names what's compiling", () => {
    expect(stageText(starting({ kind: "joining", have: 1, need: 4 }))).toBe("1 of 4 nodes up");
    expect(stageText(starting({ kind: "compiling", what: "FlashInfer kernels" }))).toBe("Compiling: FlashInfer kernels");
    expect(stageText(starting({ kind: "loading", percent: 40 }))).toBe("Loading weights 40%");
    expect(stageText(starting({ kind: "warming" }))).toBe("Capturing CUDA graphs");
    expect(stageText(starting({ kind: "starting" }))).toBe("Starting");
    expect(stageText(starting(null))).toBe("Starting");
    expect(stageText({ state: "failed", stage: { kind: "joining", have: 1, need: 4 }, message: "It stopped." })).toBe("It stopped.");
  });
});

describe("words a several-node load needs elsewhere", () => {
  it("names the rendezvous port in Workloads and the removal on the unlock sheet", () => {
    expect(usually({ port: 29501, addr: "192.168.100.1" })).toBe("vLLM multi-node rendezvous");
    expect(actionWords("deployment_clean_leftovers")).toBe("remove containers an earlier load left on a node");
  });
});

describe("Settings › Models", () => {
  it("lists the recipes read-only, with what each is verified on", async () => {
    const note = "Needs the FP4-Local checkpoint and its image, see What's coming";
    const recipes = [GENERIC, GEMMA_RECIPE, recipeOf({ id: "glm-5.3-flash", title: "GLM-5.3 Flash", note, placeholder: true })];
    mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { recipes_list: () => recipes, get_snapshot: () => snapshot(false) }));
    let r!: ReturnType<typeof render>;
    await act(async () => {
      r = render(<WindowManagerProvider sizeOf={() => [1200, 900]} isKnown={() => true}><ModelsSection /></WindowManagerProvider>);
    });
    await settle();
    expect(r.container).toHaveTextContent("Generic vLLM");
    expect(r.container).toHaveTextContent("Gemma 3 27B");
    expect(r.container).toHaveTextContent(note);
    expect(screen.getAllByText("Verified on: nothing yet")).toHaveLength(3);
  });
});

describe("Loaded tab", () => {
  const stopped: Deployment = {
    spec: { id: "d1", servedName: "gemma-3-27b-it", modelPath: GEMMA, nodes: ["cell2"], tensorParallel: 1, pipelineParallel: 1, expertParallel: false, maxModelLen: 32768, gpuMemoryUtilization: 0.75, port: 8000, image: DEFAULT_IMAGE, extraArgs: [] },
    endpoint: "http://192.168.50.154:8000/v1", state: "stopped", stage: null, message: "x", logTail: "", startedAt: Date.now() / 1000 - 600, providerId: "", demo: false,
    unloadReport: {
      atMs: Date.now(), ok: false, summary: "Unloaded from cell2 · memory back (116.8 GiB available, baseline 116.9) · port 8000 closed · /dev/shm: new since the load: psm_5f3c2a91 (2.0 GiB); removing them needs sudo and your OK (not done)",
      nodes: [{ node: "cell2", stopped: true, memAvailable: 125e9, memBack: true, portClosed: true, newShm: ["psm_5f3c2a91"] }],
    },
  };

  async function loaded(deployments: Deployment[], containerOnCell2: boolean, admin: AdminState = UNLOCKED, cell2Name?: string) {
    resetStore();
    const s = snapshot(false);
    const cell2 = s.nodes.find((n) => n.id === "cell2")!;
    if (cell2Name) cell2.name = cell2Name;
    if (containerOnCell2) {
      cell2.containers = [...cell2.containers, { id: "abc123def456", name: "sparkbox-d1", image: DEFAULT_IMAGE, state: "exited", startedAt: 0, finishedAt: 0, exitCode: 1, cpuPct: 0, mem: 0, gpuMem: 0, ports: [], project: null, command: "sh -c", labels: { "sparkbox.deployment": "d1", "sparkbox.instance": "dev" } }];
    }
    mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { get_snapshot: () => s, admin_state: () => admin, endpoints_list: () => [], cluster_deployments_list: () => [], deployment_forget: () => "t-unload" }));
    _resetAdminForTests(admin);
    await startClusterStore();
    let r!: ReturnType<typeof render>;
    await act(async () => {
      r = render(<WindowManagerProvider sizeOf={() => [1200, 900]} isKnown={() => true}><LoadedTab deployments={deployments} refresh={() => {}} onLoad={() => {}} onBenchmark={() => {}} /></WindowManagerProvider>);
    });
    await tick();
    return r.container;
  }

  it("explains a saved load waiting for fresh reports and one needing cleanup", async () => {
    const message = "Waiting for fresh reports from cell2. Nothing is relaunched.";
    const root = await loaded([{ ...stopped, state: "starting", unloadReport: null, recovery: "waiting", message }], false);
    expect(root).toHaveTextContent("Recovering saved load");
    expect(root).toHaveTextContent(message);
    cleanup();
    const cleanupRoot = await loaded([{ ...stopped, state: "failed", unloadReport: null, recovery: "cleanup", message: "Review its remaining containers, then choose Unload." }], false);
    expect(cleanupRoot).toHaveTextContent("Interrupted startup");
    expect(screen.getAllByText("Review its remaining containers, then choose Unload.")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Forget" })).toBeEnabled();
  });

  it("shows what the last unload found", async () => {
    const root = await loaded([stopped], false);
    expect(root).toHaveTextContent("gemma-3-27b-it on cell2 was unloaded");
    expect(root.querySelector(".notice time")).toHaveAttribute("dateTime", new Date(stopped.unloadReport!.atMs).toISOString());
    expect(root).toHaveTextContent("Last unload: Unloaded from cell2 · memory back (116.8 GiB available, baseline 116.9) · port 8000 closed · /dev/shm: new since the load: psm_5f3c2a91 (2.0 GiB); removing them needs sudo and your OK (not done)");
    expect(root.querySelector(".notice.warn")).not.toBeNull();
  });

  it("confirms removal of a stopped history note and Cancel preserves it", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await loaded([stopped], false);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Forget" })); });
    expect(asked).toHaveBeenCalledWith(expect.objectContaining({ confirmLabel: "Forget note", body: expect.stringContaining("keeps a recovery copy") }));
    expect(mockCalls.some((c) => c.cmd === "deployment_forget")).toBe(false);
    expect(screen.getByText(/Last unload:/)).toBeInTheDocument();
    cleanup();
    await loaded([{ ...stopped, demo: true }], false);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Forget" })); });
    expect(asked.mock.calls.at(-1)![0].body).toBe("This removes the simulated note. Saved live history stays unchanged.");
  });

  it("Forget of a load whose containers are still there says it unloads them first", async () => {
    const failed: Deployment = { ...stopped, state: "failed", unloadReport: null, message: "It answers, but wrongly. Unload removes it." };
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await loaded([failed], true);
    const forget = screen.getByRole("button", { name: "Forget…" });
    expect(forget).toHaveAttribute("title", "Unload its containers on cell2, then take it off this list");
    await act(async () => {
      fireEvent.click(forget);
    });
    for (let i = 0; i < 10 && !mockCalls.some((c) => c.cmd === "deployment_forget"); i++) await tick();
    expect(asked).toHaveBeenCalledWith(expect.objectContaining({ title: "Forget gemma-3-27b-it?", body: expect.stringContaining("Its containers are still on cell2: this unloads them first.") }));
    expect(asked.mock.calls[0][0].body).toContain("keeps a recovery copy");
    expect(mockCalls.filter((c) => c.cmd === "deployment_forget").map((c) => c.args)).toEqual([{ id: "d1" }]);
  });

  const failure: Deployment = {
    ...stopped, state: "failed", unloadReport: null, message: "Rank 1 stopped while starting.",
    spec: { ...stopped.spec, nodes: ["cell1", "gx10-0000"] },
    logTail: "the two tails, labelled",
    failure: {
      atMs: Date.now(), cause: "Rank 1 on gx10-0000 stopped while starting.", logDir: "/Users/me/Library/Application Support/com.nemecj.sparkbox.dev/failed-loads/d1-1",
      leftOn: ["gx10-0000"],
      ranks: [
        { node: "cell1", rank: 0, role: "head", exit: null, tail: "waiting for rank 1" },
        { node: "gx10-0000", rank: 1, role: "worker", exit: 1, tail: "out of memory" },
      ],
    },
  };

  it("shows why a start failed, where it's left, and each rank's last lines", async () => {
    const root = await loaded([failure], false);
    expect(root).toHaveTextContent("Rank 1 on gx10-0000 stopped while starting.");
    expect(root.querySelector(".notice.error time")).toHaveAttribute("dateTime", new Date(failure.failure!.atMs).toISOString());
    expect(root).toHaveTextContent("Left on cell4: Forget unloads them first.".replace("cell4", baseSnapshot.nodes.find((n) => n.id === "gx10-0000")!.name));
    expect(root).not.toHaveTextContent("Unload removes");
    expect(root).toHaveTextContent("Logs on this Mac:");
    expect(root).toHaveTextContent("failed-loads/d1-1");
    expect(root).toHaveTextContent("── rank 1, worker on");
    expect(root).toHaveTextContent("out of memory");
    expect(root).toHaveTextContent("waiting for rank 1");
    expect(root).not.toHaveTextContent("the two tails, labelled");
  });

  it("says when a running load isn't using the RDMA fabric", async () => {
    const said = "It isn't using the RDMA fabric: its message queue is on 192.168.50.51.";
    const root = await loaded([{ ...stopped, state: "ready", unloadReport: null, transportWarning: said }], false);
    expect(root).toHaveTextContent(`gemma-3-27b-it: ${said}`);
  });

  it("labels each rank's tail with the node's name and its exit, and falls back to the load's tail", () => {
    const name = (n: string) => (n === "gx10-0000" ? "cell4" : n);
    expect(failureLog(failure, name)).toBe("── rank 0, head on cell1 ──\nwaiting for rank 1\n\n── rank 1, worker on cell4, exit 1 ──\nout of memory");
    expect(failureLog({ logTail: "plain tail", failure: null })).toBe("plain tail");
    expect(failureLog({ logTail: "plain tail", failure: { ...failure.failure!, ranks: [] } })).toBe("plain tail");
  });

  it("says a renamed node's name in its note, its tooltip and the Forget dialog", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const failed: Deployment = { ...stopped, state: "failed", unloadReport: null, message: "It answers, but wrongly." };
    const root = await loaded([failed], true, UNLOCKED, "renamed");
    expect(root).toHaveTextContent("gemma-3-27b-it on renamed failed");
    const forget = screen.getByRole("button", { name: "Forget…" });
    expect(forget).toHaveAttribute("title", "Unload its containers on renamed, then take it off this list");
    await act(async () => {
      fireEvent.click(forget);
    });
    for (let i = 0; i < 10 && !asked.mock.calls.length; i++) await tick();
    expect(asked.mock.calls[0][0].body).toContain("Its containers are still on renamed: this unloads them first.");
  });

  it("says a renamed node's name in Rust's sentences too", async () => {
    const said: Deployment = { ...failure, spec: { ...failure.spec, nodes: ["cell2"] }, failure: { ...failure.failure!, cause: "Rank 1 on cell2 stopped while starting.", leftOn: ["cell2"] } };
    const root = await loaded([said, { ...stopped, spec: { ...stopped.spec, id: "d2" } }], false, UNLOCKED, "renamed");
    expect(root).toHaveTextContent("Rank 1 on renamed stopped while starting.");
    expect(root).toHaveTextContent("Left on renamed: Forget unloads them first.");
    expect(root).toHaveTextContent("Last unload: Unloaded from renamed");
    expect(root).not.toHaveTextContent("cell2");
  });

  it("Forget of a load with nothing left requires confirmation before removing the note", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await loaded([stopped], false);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Forget" }));
    });
    for (let i = 0; i < 10 && !mockCalls.some((c) => c.cmd === "deployment_forget"); i++) await tick();
    expect(asked).toHaveBeenCalledWith(expect.objectContaining({ confirmLabel: "Forget note" }));
    expect(mockCalls.some((c) => c.cmd === "deployment_forget")).toBe(true);
  });

  it("Forget is off in look-only, with look-only's words, and asks to unlock when locked", async () => {
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await loaded([stopped], true, { ...UNLOCKED, unlocked: false, lookOnly: true });
    const forget = screen.getByRole("button", { name: "Forget…" });
    expect(forget).toBeDisabled();
    expect(forget).toHaveAttribute("title", LOOK_ONLY_TIP);
    await act(async () => {
      fireEvent.click(forget);
    });
    await tick();
    expect(asked).not.toHaveBeenCalled();
    expect(mockCalls.some((c) => c.cmd === "deployment_forget")).toBe(false);

    await loaded([stopped], false, { ...UNLOCKED, unlocked: false });
    const locked = screen.getAllByRole("button", { name: "Forget" }).at(-1)!;
    expect(locked).toBeEnabled();
    expect(locked).toHaveAttribute("title", "Take it off this list · needs Admin (click to unlock)");
  });
});

// Version advice uses the same report as Fleet and never replaces the readiness verdict.
it("shows version advice on multi-node preflight, including Demo, and keeps single-node UI unchanged", () => {
  const spec = { imageRef: DEFAULT_IMAGE, gpuMemoryUtilization: 0.75 } as DeploySpec;
  const versions = { state: "mismatched" as const, rows: [{ key: "driver", label: "NVIDIA driver", state: "mismatched" as const, note: null,
    values: [{ node: "cell1", value: "580.1", reason: null }, { node: "gx10-0000", value: "580.2", reason: null }] }] };
  const nodes = ["cell1", "gx10-0000"].map((node) => preflight(spec, { node }));
  for (const verdict of ["ready", "demo"] as const) {
    render(<BeforeItStarts set={setOf(nodes, { verdict, versions })} problem={null} share={0.75} port={8000} drop={false} onDrop={() => {}} nameOf={(id) => id === "gx10-0000" ? "cell4" : id} />);
    expect(screen.getByText("Version consistency: Mismatched")).toBeVisible();
    expect(screen.getByText(/Selected image; NCCL is its declared version/)).toHaveTextContent("advisory");
    if (verdict === "ready") expect(screen.getByText("Ready to load on cell1 and cell4.")).toBeVisible();
    fireEvent.click(screen.getByText("Compare version details"));
    expect(screen.getByRole("columnheader", { name: "cell4" })).toBeVisible();
    cleanup();
  }
  render(<BeforeItStarts set={setOf(nodes.slice(0, 1), { versions })} problem={null} share={0.75} port={8000} drop={false} onDrop={() => {}} />);
  expect(screen.queryByText(/Version consistency/)).toBeNull();
});
