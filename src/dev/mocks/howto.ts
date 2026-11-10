import type { MockHandlers } from "../../test/tauri";
import { LIBRARY, emptyLocal, type LocalGuide, type Document, type SourceCheck, type Save } from "../../howto/library";
import { preparedImageJobs } from "./jobs";
import type { Job } from "../../lib/types";

const key = (demo: boolean) => `sparkbox:mock-howto:${demo ? "demo" : "live"}`;
type Saved = { guides: Record<string, LocalGuide>; check: SourceCheck | null };
function load(demo: boolean): Saved { const data = localStorage.getItem(key(demo)); return data ? JSON.parse(data) as Saved : { guides: {}, check: null }; }
function save(demo: boolean, data: Saved) { localStorage.setItem(key(demo), JSON.stringify(data)); }
async function docs(): Promise<Record<string, Document>> { return (await import("../../../resources/playbooks/documents.json")).default; }
export const handlers: MockHandlers = {
  howto_document: async ({ path }) => { const d = (await docs())[path]; if (!d) throw new Error("Document not bundled."); return d; },
  howto_saved: ({ demo }) => Object.fromEntries(Object.entries(load(demo).guides).map(([id, g]) => [id, { favorite: g.favorite, editions: g.editions.length, completed: g.completed.length, title: id }])),
  howto_read: ({ demo, id }) => load(demo).guides[id] ?? emptyLocal(),
  howto_save: ({ demo, request: r }: { demo: boolean; request: Save }) => {
    const data = load(demo); const g = data.guides[r.id] ?? emptyLocal();
    if (r.expected !== g.version) throw new Error("This guide changed in another window. Reload it before saving; your draft has not replaced anything.");
    g.version++; g.favorite = r.favorite; g.notes = r.notes; g.completed = r.completed;
    if (r.edition) g.editions.push({ ...r.edition, atMs: Date.now(), sha256: "demo-text-digest" });
    data.guides[r.id] = g; save(demo, data); return g;
  },
  howto_last_check: ({ demo }) => load(demo).check,
  howto_check_source: ({ demo }) => {
    const data = load(demo); data.check = { revision: "d".repeat(40), checkedAtMs: Date.now(), demo: true, total: LIBRARY.guides.length, changes: LIBRARY.guides.map((g) => ({ id: g.id, status: g.id === "playbook-vllm" ? "changed" : "unchanged", treeSha: g.treeSha, readmeBlob: "demo", published: g.published })) };
    save(demo, data); return data.check;
  },
  howto_fetch_source: async ({ demo, id, revision }) => {
    const data = load(demo); if (data.check?.revision !== revision) throw new Error("Check NVIDIA again before fetching.");
    const guide = LIBRARY.guides.find((g) => g.id === id); if (!guide) throw new Error("Unknown Demo guide.");
    const source = (await docs())[guide.readme]; const g = data.guides[id] ?? emptyLocal();
    if (!g.sources.some((s) => s.revision === revision)) g.sources.push({ revision, treeSha: guide.treeSha, atMs: Date.now(), document: { ...source, text: source.text + "\n\n## Demo source update\n\nThis simulated revision adds a reminder to verify the endpoint before starting another workload. No NVIDIA request was made.\n" } });
    data.guides[id] = g; save(demo, data); return g;
  },
  howto_check_nodes: ({ nodes }) => nodes.map((node: string) => ({ node, error: null, imageRef: "nvcr.io/nvidia/vllm:26.09-py3", home: "/home/demo", rootPath: "/home/demo/models", rootExists: true, freeBytes: 1750 * 1024 ** 3, docker: true, rsync: true, python: "3.12.3", imagePresent: true, vllmVersion: "", gpuInContainer: null, fabric: [{ iface: "enp1s0f0np0", ip: "192.168.100.10", rdma: "mlx5_0" }], checkedAtMs: Date.now() })),
  howto_prepare_image: ({ id, image, nodes, estimatedBytes }) => {
    if (!LIBRARY.guides.find((g) => g.id === id)?.images.includes(image)) throw new Error("That image is not a reviewed action for this guide.");
    const now = Math.floor(Date.now() / 1000);
    const jobs: Job[] = nodes.map((node: string) => ({ id: `demo-howto-${crypto.randomUUID()}`, node, kind: "pull", state: "done", step: "done", label: `Prepare ${image}`, message: "Demo: image preparation simulated; no download was made.", bytesDone: estimatedBytes, bytesTotal: estimatedBytes, rateBps: 0, startedAt: now, updatedAt: now }));
    preparedImageJobs.unshift(...jobs);
    return jobs;
  },
};
