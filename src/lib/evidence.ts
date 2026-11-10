// Evidence capture (src-tauri/src/evidence.rs): what every connected node looks like at one moment
// (containers, images, failed units, disk, models, apt, packages, GPU, addresses, listening ports,
// memory, /dev/shm), filed in ~/Library/Logs/SparkBox/evidence/<date>-<phase>/<time>-<label>/ with
// a diff against the capture before it, the activity log and the SSH log. Reads only, so it runs in
// look-only mode too; Live only.
import { invoke } from "./api";

/** One capture folder, as evidence_capture returns it and evidence_list lists it. */
export interface EvidenceFolder {
  path: string;
  phase: string;
  label: string;
  startedMs: number;
  /** Nodes captured. */
  nodes: number;
  /** Steps that didn't exit 0 (a missing folder, a command the node lacks, a lost connection). */
  failedSteps: number;
  notConnected: string[];
  /** diff-from-<previous>.txt, when the phase had a capture before this one (any day). */
  diff?: string | null;
  /** What couldn't be filed (the diff, the activity log, a file), or that it didn't finish. */
  problems: string[];
}

/** Captures every connected node now. Rejects with "Available with Live data." in Demo. */
export function captureEvidence(phase: string, label: string): Promise<EvidenceFolder> {
  return invoke<EvidenceFolder>("evidence_capture", { phase, label });
}

/** The last 20 captures, newest first. */
export function listEvidence(): Promise<EvidenceFolder[]> {
  return invoke<EvidenceFolder[]>("evidence_list");
}

/** "4 nodes · 3 steps failed · cell3 not connected · 1 problem": what the button's result says. */
export function evidenceSummary(f: EvidenceFolder): string {
  const parts = [f.nodes === 1 ? "1 node" : `${f.nodes} nodes`];
  if (f.failedSteps) parts.push(f.failedSteps === 1 ? "1 step failed" : `${f.failedSteps} steps failed`);
  if (f.notConnected.length) parts.push(`${f.notConnected.join(", ")} not connected`);
  const problems = f.problems?.length ?? 0;
  if (problems) parts.push(problems === 1 ? "1 problem" : `${problems} problems`);
  return parts.join(" · ");
}
