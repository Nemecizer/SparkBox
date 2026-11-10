// Downloads and copies, which run on the nodes themselves.
import { useState } from "react";
import { Ban, Play, RefreshCw } from "lucide-react";
import { Table, type Column } from "../../components/Table";
import { HelpTip, Meter, Toolbar, ToolButton } from "../../components/ui";
import { api } from "../../lib/api";
import { isAdminUnlocked, useAdmin } from "../../lib/admin";
import { fmtBytes, fmtDuration } from "../../lib/format";
import { confirmAction } from "../../lib/session";
import { toast } from "../../lib/toasts";
import type { Job } from "../../lib/types";
import { chooseDownloadTool, downloadToolConsent } from "./DownloadToolChoice";
import { checkTransferDisk, DiskPreflight } from "./DiskPreflight";
import { Ago, errText, locked } from "./shared";

/** sparkjob's state, as a word (`cancelled` stays on the wire). */
export function jobWord(word: string): string {
  return word === "cancelled" ? "canceled" : word;
}

/** A message from an older sparkjob said it the other way. */
function jobMessage(message: string): string {
  return message.toLowerCase() === "cancelled" ? "Canceled" : message;
}

/** What canceling each kind of job leaves behind. */
export const CANCEL_WORDS: Record<Job["kind"], string> = {
  download: "The partial files stay on disk and the Library marks the model Incomplete. Select the stopped job and use Resume download… to reuse its saved revision and files. Older jobs without a saved plan need a newly reviewed download from Hugging Face.",
  pull: "The partial files stay on disk and the Library marks this copy Incomplete. Copy to nodes… picks up where it stopped.",
  serve: "This stops the temporary file server. A copy still reading from it fails.",
};

export function JobsTab({ jobs, refresh }: { jobs: Job[]; refresh: () => void }) {
  const admin = useAdmin();
  const [selected, setSelected] = useState<string | null>(null);
  const [resuming, setResuming] = useState(false);
  const job = jobs.find((j) => `${j.node}/${j.id}` === selected) ?? null;
  const canResume = job?.kind === "download" && !!job.resume && (job.state === "failed" || job.state === "cancelled");

  const columns: Column<Job>[] = [
    { key: "label", label: "Job", sort: (j) => j.label, priority: 1, render: (j) => <span className="strong">{j.label || j.id}</span> },
    { key: "node", label: "Node", sort: (j) => j.node, render: (j) => j.node },
    {
      key: "state", label: "State", sort: (j) => j.state,
      render: (j) => (
        <span className="status" title={jobMessage(j.message)}>
          <span className={`dot ${j.state === "done" ? "online" : j.state === "running" ? "connecting" : j.state === "failed" ? "error" : "offline"}`} />
          {j.state === "running" ? j.step : jobWord(j.state)}
        </span>
      ),
    },
    {
      key: "progress", label: "Progress", width: 220,
      render: (j) => (j.bytesTotal > 0 && j.kind !== "serve"
        ? <Meter pct={(100 * j.bytesDone) / j.bytesTotal} label={`${fmtBytes(j.bytesDone)} of ${fmtBytes(j.bytesTotal)}`} />
        : null),
    },
    {
      key: "rate", label: "Speed", align: "right", priority: 2,
      render: (j) => (j.state === "running" && j.rateBps > 0
        ? <span className="num">{fmtBytes(j.rateBps)}/s</span>
        : ""),
    },
    {
      key: "left", label: "Time left", align: "right", priority: 2,
      render: (j) => (j.state === "running" && j.rateBps > 0 && j.bytesTotal > j.bytesDone
        ? <span className="num">{fmtDuration(Math.max(1, (j.bytesTotal - j.bytesDone) / j.rateBps))}</span>
        : ""),
    },
    { key: "tool", label: "Download tool", priority: 3, title: (j) => [j.toolUsed, j.toolVersion].filter(Boolean).join(" · "), render: (j) => j.toolUsed ? `${j.toolUsed}${j.toolVersion ? ` · ${j.toolVersion}` : ""}` : "—" },
    { key: "started", label: "Started", align: "right", sort: (j) => j.startedAt, render: (j) => <Ago ts={j.startedAt} /> },
    { key: "message", label: "Details", priority: 3, title: (j) => jobMessage(j.message), render: (j) => <span className={j.state === "failed" ? "error-text" : "muted"}>{jobMessage(j.message)}</span> },
  ];

  const cancel = async () => {
    if (!job) return;
    const ok = await confirmAction({ title: `Cancel “${job.label || job.id}”?`, body: CANCEL_WORDS[job.kind], confirmLabel: "Cancel job", cancelLabel: "Keep going", danger: true });
    if (!ok) return;
    try {
      await api.jobCancel(job.node, job.id);
    } catch (e) {
      // The job has ended, but docker didn't say its container stopped: the words say which.
      const words = errText(e);
      toast({ kind: "error", title: words.startsWith("Canceled,") ? "Its container may still be running" : "Couldn't cancel it", body: words });
    }
    refresh();
  };

  const resume = async () => {
    if (!job?.resume || !canResume || resuming || !isAdminUnlocked()) return;
    setResuming(true);
    try {
      const plan = job.resume;
      const review = await chooseDownloadTool(job.node);
      if (!review) return;
      const checks = await checkTransferDisk({ kind: "resume", node: job.node, id: job.id, expected: plan });
      if (!checks) return;
      const installing = review.tool.kind === "install";
      if (!await confirmAction({ title: `Resume ${plan.repo} on ${job.node}?`,
        body: <div className="col gap-md"><DiskPreflight checks={checks} /><p>{`${downloadToolConsent(review)} Use the saved revision ${plan.revision}, original file list and folder ${plan.dest} on ${job.node}. This starts a new attempt and keeps the old job record. Partial files are reused where possible and checked before the model is ready. The saved plan and folder ownership are checked again; a changed or busy folder is refused.`}</p></div>,
        confirmLabel: installing ? "Install tool and resume" : "Resume download", danger: installing })) return;
      review.installConsent = installing;
      const next = await api.jobResumeDownload(job.node, job.id, plan, review);
      setSelected(`${next.node}/${next.id}`);
    } catch (e) {
      toast({ kind: "error", title: "Couldn't resume the download", body: errText(e) });
    } finally {
      setResuming(false);
      refresh();
    }
  };

  return (
    <div className="col fill">
      <Toolbar label="Jobs">
        <ToolButton icon={RefreshCw} label="Refresh" onClick={refresh} />
        <ToolButton icon={Play} label={resuming ? "Starting…" : "Resume download…"} locked={!admin} disabled={!canResume || resuming}
          title={!admin ? locked(admin) : canResume ? "Reuse this download's original revision and files" : "Select a stopped download with a saved plan from this SparkBox"} onClick={resume} />
        <ToolButton icon={Ban} label="Cancel" locked={!admin} disabled={job?.state !== "running"} title={locked(admin)} onClick={cancel} />
      </Toolbar>
      <Table columns={columns} rows={jobs} rowKey={(j) => `${j.node}/${j.id}`} selected={selected} onSelect={setSelected}
        tableId="models.jobs" itemNoun={["job", "jobs"]} empty="No downloads or copies yet."
        footer={<HelpTip topic="models.jobs.about" label="About jobs" />} />
    </div>
  );
}
