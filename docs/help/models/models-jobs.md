---
id: models-jobs
title: "Models: Jobs tab"
type: feature
area: models
summary: "Downloads and copies running on your nodes, with progress, Cancel and Resume download."
keywords: ["jobs", "progress", "cancel", "resume", "retry"]
verified: false
related: [follow-a-download, tasks-and-jobs]
---

# Models: Jobs tab

## What it's for

The **Jobs** tab shows downloads and copies, which run on the nodes themselves. A **job** is long work that SparkBox hands to a small helper on a node. Use this tab to see how a job is going, whether it finished, and why it failed.

Jobs are built to keep going if SparkBox quits or the VPN drops. That hasn't been proven on your cluster, so keep SparkBox open and this Mac awake until a job ends. [Keep this Mac awake during long work](help:keep-this-mac-awake) shows how.

## Where to find it

- **Sidebar:** under **Workloads**, click **Models**, then click the **Jobs** tab. The number on the tab counts running jobs.
- **After a download or copy:** SparkBox moves you here.
- **Link:** [Open Jobs](open:models?tab=jobs)


## What's on the screen

### Toolbar

- **Refresh** reads the jobs again. The tab also refreshes by itself every few seconds. After reopening SparkBox, wait for the nodes to connect and click **Refresh** before treating an empty list as no saved jobs.
- **Resume download…** starts a new attempt from a stopped download’s saved plan. It keeps the original node, folder, revision and file list. It is dimmed for running jobs, finished downloads, copies, chain rows and older jobs without a saved plan.
- **Cancel chain…** appears for a chain row. It stops this SparkBox’s remaining members, including the file server, after confirmation. Finished files, records and earlier failure messages remain. Disconnected or changed-ownership members show an error and need review. Reported leftover processes also need recovery review; cancellation does not claim they were cleaned.
- **Cancel** stops the selected job. It's dimmed unless a running job is selected.

### Job table

The table lists jobs from every online node, newest first.

| Column | What it shows |
|---|---|
| **Job** | What it is, such as `Download Qwen/Qwen3.8-27B-FP8` |
| **Chain** | The chain ID, source and copy targets; **—** for separate jobs |
| **Node** | The node doing the work |
| **State** | The step while it runs, then **done**, **failed** or **canceled** |
| **Progress** | A bar with the size so far |
| **Speed** | How fast it's going now |
| **Time left** | An estimate from the current speed |
| **Download tool** | The chosen tool and reported version; **—** for jobs without a recorded tool |
| **Started** | How long ago it began |
| **Details** | The reason, if it failed |

In a narrow window, some columns hide. If a node is offline, its jobs don't appear until it's back.

### Kinds of job

| Name in the Job column | What it is | State while it runs |
|---|---|---|
| `Download …` | A download from Hugging Face | **installing the approved download tool**, **checking the selected hf tool**, **downloading**, **verifying** |
| `Copy … to cell2` | One node's copy over the fabric | **copying**, **verifying** |
| `Share … from cell1` | The temporary file server for a copy. It has no progress bar | **serving** |

A temporary share closes after its copies report completion. Older shares can end as **canceled** when SparkBox stops them. Do not stop a file server while a copy still needs it.

A chain source moves through **downloading**, **verifying** and **serving**. Its target rows move through **waiting**, **copying** and **verifying**. The source reports **done** only after every target reports successful verification. Select a chain row to see its fixed commit, deadline and recovery guidance. Read every target result too.

Both the download row and its copy rows keep the same chain ID after they finish. The download row also keeps its fixed download plan. Reopening Jobs follows existing records and starts no replacement chain. Chains expire within 24 hours; the file server also stops after 30 minutes without activity. A node reboot does not restart its work. Cancel remaining jobs before recovering an incomplete source with a new reviewed download, then copy unfinished targets from the Library. See [Download, then copy to other nodes](help:download-a-model#download-then-copy-to-other-nodes).

An older chain source may show **Chain details unavailable** if its participant plan was not saved. **Resume download…** stays unavailable. SparkBox does not guess the missing nodes or rewrite the record. Inspect Jobs on the other nodes before recovery. **Cancel**, when available for that older row, stops only the selected owned job; it does not cancel the whole chain.

Finished jobs stay on the list, and there's no way to clear them here.

## What it changes

- **Reading** works in look-only mode, and there it changes nothing. With look-only mode off, listing a node's jobs also marks a job that stopped unexpectedly as **failed** on the node, and removes the secret files left by any job that's over.
- **Resume download…** needs Admin mode and is unavailable in look-only mode. Choose a tool, then review the node, folder and revision. Click **Resume download**, or give separate installation consent with **Install tool and resume**. Canceling either dialog starts nothing. SparkBox checks the saved plan and folder again before writing. Another SparkBox’s job, a busy folder or a folder claimed by a newer attempt is refused. The old record stays, and the new attempt appears as a separate row. Partial files may be reused, but must pass verification before loading. This recovery has been tested offline only.
- **Cancel** stops the job on its node. It needs Admin mode, and it's off in look-only mode. A dialog asks first: **Cancel job** or **Keep going**, and it says what stopping this kind of job does:
  - A `Download` row: The partial files stay on disk and the Library marks the model Incomplete. Resume uses its saved revision and files; an older job without a saved plan needs a newly reviewed download. See [Start again](help:follow-a-download#start-again).
  - A `Copy` row: "The partial files stay on disk and the Library marks this copy Incomplete. Copy to nodes… picks up where it stopped." See [Copy a model to other nodes](help:copy-a-model-to-other-nodes#if-something-goes-wrong).
  - A `Share` row: "This stops the temporary file server. A copy still reading from it fails."

## Related How-Tos

- [Follow a download](help:follow-a-download)
- [Download a model](help:download-a-model)
- [Copy a model to other nodes](help:copy-a-model-to-other-nodes)
