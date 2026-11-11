---
id: follow-a-download
title: "Follow a download"
type: howto
area: models
summary: "Watch a download's progress in Jobs and Tasks, stop it if you need to, and know when it's finished."
keywords: ["progress", "jobs", "how long", "cancel download", "is my download working", "stuck download", "resume download", "incomplete", "pick up a download", "start again"]
verified: false
related: [models-jobs, tasks-and-jobs, download-a-model]
---

# Follow a download

To see how a download is going, watch its row on the **Jobs** tab, where you can also cancel it and tell when it's finished.

A **job** is long work that runs by itself on a node. Downloads and copies are both jobs.

For a **Then copy to** chain, inspect every node’s row in Jobs. Use **Cancel chain…** before recovery; ordinary Resume is disabled for these rows. [Download, then copy to other nodes](help:download-a-model#download-then-copy-to-other-nodes) explains the empty-folder requirement, deadline and recovery steps.

## Before you start

- You've started a download or a copy. See [Download a model](help:download-a-model).
- Watching needs no Admin mode. It also works in look-only mode.
- **Cancel**, **Cancel chain…** and **Resume download…** need Admin mode, and are unavailable in look-only mode.
- Keep SparkBox open until every chain job appears, and keep this Mac awake while watching it. A bounded chain test followed the same jobs after normal quit/reopen. Mac sleep and network loss still need live tests. [Keep this Mac awake during long work](help:keep-this-mac-awake) shows how.

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Jobs](open:models?tab=jobs) The **Models** window opens.
2. Click the **Jobs** tab. A small number on the tab shows how many jobs are running. The table lists every download and copy, newest first.


3. Find your row by its **Job** name, such as `Download Qwen/Qwen3.8-27B-FP8`, and its **Node**.
4. Read the row, using the table below. The tab refreshes by itself every few seconds.
5. Wait until **State** reads **done**. You can use the rest of SparkBox while you wait.
6. Click the **Library** tab. The model is listed, with **✓ verified** under the node's column.

| Column | What it shows |
|---|---|
| **State** | What the job is doing now: **starting**, **installing the approved download tool**, **checking the selected hf tool**, **downloading** or **verifying**. When it ends: **done**, **failed** or **canceled** |
| **Progress** | A bar with the size so far, such as "12.0 GiB of 60.8 GiB". It can stay empty while the tool is installed or checked |
| **Speed** | How fast it's going now |
| **Time left** | An estimate from the current speed |
| **Download tool** | The chosen tool and reported version. **—** means no tool was recorded, as in older jobs |
| **Started** | How long ago it began |
| **Details** | The reason, if the job failed |

> [!NOTE]
> The bar fills twice. After the download reaches 100%, **State** changes to **verifying** and the bar starts again from zero, while the node checks every large file. This is normal, and the bar goes faster this time.

To follow the same work from anywhere in SparkBox, open [Tasks](open:tasks). The task is named `Download … to cell1`.

## Cancel a download

1. On the **Jobs** tab, click the row.
2. Click **Cancel** in the toolbar. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
3. A dialog asks to confirm. It explains that partial files stay on disk and a saved download plan can be resumed from Jobs. Click **Cancel job** to stop, or **Keep going** to let it run.

Stopping takes several seconds, because SparkBox stops the download's processes and, when used, its owned container. The row reads **canceled**, and the **Library** marks the model **◐ incomplete**. Canceling a separate download and using ordinary Resume have run only on test systems.

For a **Then copy to** chain, select its row and click **Cancel chain…**. Review the nodes, then click **Cancel chain**, or **Keep going** to leave it running. Inspect every member's result; files and records stay on the nodes. A live test passed cancellation and cleanup during tool checking, before file sharing began. Cancellation during file sharing still needs a live test.

## Start again

A download that was canceled, or that stopped partway, leaves its files on the node. The **Library** marks the model **◐ incomplete**, and the **Load** page won't offer it.

1. On the **Jobs** tab, select the latest **canceled** or **failed** download for that folder.
2. Click **Resume download…**. If Admin mode is locked, click **Unlock…** and confirm with Touch ID or your Mac password. Look-only mode must be off.
3. Choose the download tool for this attempt, then click **Continue**. Choosing installation does not grant permission yet.
4. Review the original node, folder, full revision and chosen tool. **Destination disk** must have room for the full saved file list plus 5% and 20 GiB, even when partial files are present. Read any fullness warning. Click **Resume download**, or **Install tool and resume** if you approve the displayed installation. The old row stays in Jobs. Canceling either dialog starts nothing.
5. Follow the new row until it reads **done**, then check **✓ verified** in the Library.

If the disk check refuses, no new attempt starts and the old record and partial files stay intact. Free space on the original node and review again. The check repeats before the node takes ownership of the folder; it does not reserve space.

New downloads save a plan without your token. Resume obtains the current token from Keychain and checks the saved plan and folder before starting. It refuses a changed plan, another SparkBox’s job or a folder now owned by a newer attempt. If refused, refresh Jobs and the Library and read the latest row’s details.

An older job without a saved plan cannot use this button. Open the model on **Hugging Face**, review its details and select that job's original node in **Download node** and choose **Download again**. That action uses the commit and file list you inspect now; it does not recover the old plan. Check **Commit** and **Files to download** before approving. Recovery and node-restart detection have been tested offline only.

To start from the beginning instead, delete the folder first. On the **Library** tab, select the model, click **Delete…**, select the node and click **Delete…** again. [Delete a model from a node](help:delete-a-model) has the details.

## What you should see

- A running job has a moving bar and a speed.
- A finished job reads **done**, and **Tasks** says "Finished on cell1".
- Finished jobs stay on the list. They do no harm.

## Why it works

SparkBox gives the job to a small helper on the node, and the helper runs apart from SparkBox's connection. It writes its progress to a file on the node, and the **Jobs** tab reads those files from every online node. Progress is the size of the folder on disk, so a resumed job starts from what's already there. If SparkBox closes during a job, it looks for that job the next time it runs with **Live** data. It then follows the job in **Tasks**, with "(picked up again)" after the name; this appeared in the bounded chain quit/reopen test.

## Limits

- A task follows a job for 24 hours. After that, it ends with "dl-5e1f00aa was still running on cell1 after a day; the Jobs tab follows it." The job isn't stopped, and the **Jobs** tab keeps showing it.
- You can't pause a job.
- You can't clear finished jobs from the list.

## If something goes wrong

- **"No downloads or copies yet."** No job has run on any online node. If you've started one, check that the node is online. The **Jobs** tab lists only nodes it can reach.
- **The bar hasn't moved for a long time.** Look at **State**. **installing the approved download tool** and **verifying** can each take a while. If **State** reads **downloading** and **Speed** is empty, check the node's internet connection.
- **"The job stopped unexpectedly (node restarted?). Its container may still be running: Resume download stops it first."** The job's program ended without finishing, and its container may be writing still. See [Start again](#start-again). A copy's row has only the first sentence.
- **"An earlier download is still writing to this folder: container sparkbox-dl-dl-5e1f00aa is running."** Starting again found a container SparkBox couldn't stop, or one the other SparkBox started. Find it on the node's **Containers** tab, and start again once it has stopped.
- **"dl-5e1f00aa isn't listed on cell1 any more."** The job's record is gone from the node. Look in the **Library** to see what arrived, then [start again](#start-again) if you need to.
- **A row named `Share … from cell1` reads canceled.** That's a copy's temporary file server, and this is how it ends. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes).

## Next steps

- [Copy a model to other nodes](help:copy-a-model-to-other-nodes)
- [Load a model on one node](help:load-a-model-on-one-node)
- [Tasks and jobs](help:tasks-and-jobs), to see how the two differ
