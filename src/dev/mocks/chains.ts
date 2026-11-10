import type { ChainCancel, ChainReview, DownloadChain, DownloadSelection, DownloadToolReview, Job, LibraryEntry } from "../../lib/types";
import { snapshotAt } from "../mockBackend";
import { demoDiskPreflight } from "./disk";
import { demoDownloadTools } from "./jobs";
const groups: { review: ChainReview; jobs: Job[]; sourceVerified: boolean; fault: string | null }[] = [];
export function reviewDemoChain(selection: DownloadSelection, source: string, targets: string[]): ChainReview {
  const ids = [source, ...targets];
  if (!targets.length || new Set(ids).size !== ids.length || ids.some((id) => !snapshotAt(0).nodes.some((n) => n.id === id && n.status === "online"))) throw new Error("Choose distinct connected chain nodes.");
  const checks = ids.map((node) => demoDiskPreflight({ kind: "download", selection, node })[0]);
  if (checks.some((c) => !c.ok)) throw new Error("Not enough destination space; no chain was started.");
  return { selection: structuredClone(selection), source, targets: [...targets], checks,
    nodes: checks.map((c, i) => ({ node: c.node, dest: c.dest, address: `192.0.2.${i + 1}` })), port: 8730 };
}
export function startDemoChain(expected: ChainReview, tool: DownloadToolReview): Job[] {
  const fresh = reviewDemoChain(expected.selection, expected.source, expected.targets);
  if (JSON.stringify(fresh.nodes) !== JSON.stringify(expected.nodes)) throw new Error("The reviewed chain destinations changed.");
  if (tool.node !== expected.source || !demoDownloadTools(tool.node).options.some((t) => JSON.stringify(t) === JSON.stringify(tool.tool)) || (tool.tool.kind === "install" && !tool.installConsent)) throw new Error("Review the source tool and installation consent.");
  if (groups.some((g) => g.jobs.some((j) => fresh.nodes.some((n) => n.node === j.node && n.dest === g.review.nodes.find((n) => n.node === j.node)?.dest)))) throw new Error("Demo chain destinations already contain files or reserved jobs. Start a fresh Demo to repeat.");
  const now = Math.floor(Date.now() / 1000), id = `dc-demo-${groups.length + 1}`;
  const chain: DownloadChain = { id, source: expected.source, repo: expected.selection.repo, revision: expected.selection.revision, expiresAt: now + 86400,
    members: fresh.nodes.map((n, i) => ({ node: n.node, dest: n.dest, id: `${id}-${i}` })) };
  const jobs: Job[] = chain.members.map((m, i) => ({ id: m.id, node: m.node, chain, kind: i ? "pull" : "download", state: "running", step: i ? "waiting" : "downloading", label: `${i ? "Wait, then copy" : "Download, verify and share"} ${chain.repo}`, message: "Demo: simulated chain", startedAt: now, updatedAt: now, bytesDone: 0,
    bytesTotal: expected.selection.files.reduce((n, f) => n + (f.size ?? 0), 0), rateBps: 0, resume: null, toolUsed: i ? undefined : tool.tool.label }));
  groups.push({ review: expected, jobs, sourceVerified: false, fault: new URLSearchParams(location.search).get("chain") });
  return structuredClone(jobs);
}
export function demoChainJobs(): Job[] {
  for (const group of groups) {
    const seconds = Date.now() / 1000 - group.jobs[0].startedAt;
    if (seconds >= 8 && group.fault !== "source-fail" && group.jobs[0].state === "running") group.sourceVerified = true;
    for (const [i, j] of group.jobs.entries()) {
      if (j.state !== "running") continue;
      if (seconds >= 5 && group.fault === "source-fail") { j.state = "failed"; j.message = "Demo: source verification failed; no files were copied"; continue; }
      if (seconds >= 14 && group.fault === "copy-fail" && i < 2) { j.state = "failed"; j.message = "Demo: one copy failed; verified source retained"; continue; }
      j.step = i ? seconds < 8 ? "waiting" : seconds < 16 ? "copying" : "verifying" : seconds < 6 ? "downloading" : seconds < 8 ? "verifying" : "serving";
      j.bytesDone = Math.floor(j.bytesTotal * Math.min(1, Math.max(0, i ? (seconds - 8) / 10 : seconds / 8)));
      if (seconds >= 20) { j.state = "done"; j.step = "done"; }
    }
  }
  return structuredClone(groups.flatMap((g) => g.jobs));
}
export function cancelDemoChain(node: string, id: string): ChainCancel[] {
  const group = groups.find((g) => g.jobs.some((j) => j.node === node && j.id === id));
  if (!group) throw new Error("Refresh Jobs; the Demo chain was not found.");
  return group.jobs.map((j) => { if (j.state === "running") { j.state = "cancelled"; j.message = "Canceled; files and records retained"; } return { node: j.node, id: j.id, error: null }; });
}

export function demoChainLibrary(observations = false): LibraryEntry[] {
  demoChainJobs();
  return groups.map((g) => ({ key: g.review.nodes[0].dest, repo: g.review.selection.repo, revision: g.review.selection.revision,
    format: "safetensors", sizeBytes: g.jobs[0].bytesTotal, root: "/home/demo/models", inUse: null,
    copies: Object.fromEntries(g.jobs.filter((j, i) => observations || j.state === "done" || (i === 0 && g.sourceVerified)).map((j) => {
      const verified = j.state === "done" || (j.kind === "download" && g.sourceVerified);
      const partial = j.state === "running" ? j.kind === "pull" ? "copying" as const : "downloading" as const : "stopped" as const;
      return [j.node, {
        path: g.review.nodes.find((n) => n.node === j.node)!.dest, sizeBytes: verified ? j.bytesTotal : j.bytesDone, verified,
        check: verified ? j.kind === "pull" ? "source-sha256" : "sha256" : "",
        ...(verified ? {} : { partial, progress: j.state === "running" ? {
          step: j.step === "verifying" || j.step === "waiting" ? j.step : j.kind === "pull" ? "copying" as const : "downloading" as const,
          bytesDone: j.bytesDone, bytesTotal: j.bytesTotal,
        } : null }),
      }];
    })),
  }));
}
