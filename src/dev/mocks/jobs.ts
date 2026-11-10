import type { DownloadToolReview, DownloadTools, Job } from "../../lib/types";
import type { DownloadResume } from "../../lib/types";
import { snapshotAt } from "../mockBackend";
import { requireDemoDisk } from "./disk";

/** Jobs created by explicit browser-Demo actions; shared with the Models window. */
export const preparedImageJobs: Job[] = [];

const recoveryJobs: Job[] = [{ id: "demo-dl-stopped", node: "cell1", kind: "download", state: "cancelled", step: "downloading", message: "Canceled; partial files retained", label: "Download Qwen/Qwen3-0.6B",
  bytesDone: 500_000_000, bytesTotal: 1_200_000_000, rateBps: 0, startedAt: Math.floor(Date.now() / 1000) - 600, updatedAt: Math.floor(Date.now() / 1000) - 580,
  resume: { repo: "Qwen/Qwen3-0.6B", revision: "a".repeat(40), dest: "/home/spark/models/Qwen/Qwen3-0.6B", planHash: "demo-original" } }];

export function downloadRecoveryJobs(): Job[] {
  const now = Math.floor(Date.now() / 1000);
  for (const j of recoveryJobs) if (j.state === "running") {
    j.bytesDone = Math.min(j.bytesTotal, 500_000_000 + (now - j.startedAt) * 100_000_000);
    j.updatedAt = now;
    j.step = j.bytesDone >= j.bytesTotal ? "done" : "downloading";
    if (j.step === "done") { j.state = "done"; j.rateBps = 0; }
  }
  return structuredClone(recoveryJobs);
}

export function resumeDemoDownload({ node, id, expected, review }: { node: string; id: string; expected: DownloadResume; review: DownloadToolReview }): Job {
  checkDemoTool(review, node);
  requireDemoDisk({ kind: "resume", node, id, expected });
  const index = recoveryJobs.findIndex((j) => j.id === id && j.node === node);
  const old = recoveryJobs[index];
  if (!old || !["failed", "cancelled"].includes(old.state) || JSON.stringify(old.resume) !== JSON.stringify(expected)
    || recoveryJobs.slice(index + 1).some((j) => j.node === node && j.resume?.dest === expected.dest)) throw new Error("Refresh Jobs and select the latest stopped Demo download.");
  const next: Job = { ...structuredClone(old), id: `demo-dl-${recoveryJobs.length}`, state: "running", step: "downloading", message: "Simulated download; no network access", label: `Resume ${expected.repo}`, startedAt: Math.floor(Date.now() / 1000), rateBps: 100_000_000 };
  next.resume = { ...expected, planHash: next.id };
  next.toolUsed = review.tool.label;
  next.toolVersion = "Demo";
  recoveryJobs.push(next);
  return structuredClone(next);
}

export function cancelDemoDownload(id: string): void {
  const job = recoveryJobs.find((j) => j.id === id && j.state === "running");
  if (job) { job.state = "cancelled"; job.message = "Canceled; partial files retained"; job.rateBps = 0; }
}

export function demoDownloadTools(node: string): DownloadTools {
  return { node, options: [
    { kind: "host", path: "/home/demo/.local/bin/hf", fingerprint: "a".repeat(64), label: "Existing hf on this node" },
    { kind: "install", path: "/home/demo/.sparkbox/venv", fingerprint: "uv-0.13.7-hf-2.4.0-xet-1.8.0", label: "Install SparkBox hf 2.4.0 + hf-xet 1.8.0" },
    { kind: "image", path: `sha256:${"b".repeat(64)}`, fingerprint: `sha256:${"b".repeat(64)}`, label: "vllm:demo" },
  ], notes: ["Demo: tool checks, installation and downloads are simulated."] };
}

function checkDemoTool(review: DownloadToolReview, node: string): void {
  if (!snapshotAt(0).nodes.some((n) => n.id === node && n.status === "online")) throw new Error("Choose a connected inventory node for this Demo download.");
  if (!review || review.node !== node || !demoDownloadTools(node).options.some((tool) => JSON.stringify(tool) === JSON.stringify(review.tool))
    || (review.tool.kind === "install" && !review.installConsent)) throw new Error("Review the Demo tool and confirm any installation for this attempt.");
}

export function startDemoDownload({ repo, review }: { repo: string; review: DownloadToolReview }): Job {
  checkDemoTool(review, review?.node ?? "");
  requireDemoDisk({ kind: "download", node: review.node, repo });
  const now = Math.floor(Date.now() / 1000);
  const job: Job = { id: `demo-dl-${recoveryJobs.length}`, node: review.node, kind: "download", state: "running", step: "downloading", message: "Demo: approved tool and download are simulated", label: `Download ${repo}`,
    bytesDone: 0, bytesTotal: 1_200_000_000, rateBps: 100_000_000, startedAt: now, updatedAt: now,
    toolUsed: review.tool.label, toolVersion: "Demo", resume: { repo, revision: "a".repeat(40), dest: `/home/demo/models/${repo}`, planHash: `demo-${now}` } };
  recoveryJobs.push(job);
  return structuredClone(job);
}
