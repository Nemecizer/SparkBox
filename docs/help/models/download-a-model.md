---
id: download-a-model
title: "Download a model"
type: howto
area: models
summary: "Fetch a model from Hugging Face to the node you choose, then check every file."
keywords: ["download", "hugging face", "get model", "get a model", "fetch a model", "install a model", "add a model", "how long", "disk space", "save a model"]
verified: false
related: [follow-a-download, copy-a-model-to-other-nodes, find-a-model-that-fits, keep-this-mac-awake]
---

# Download a model

To put a model on your cluster, download it from Hugging Face to the node you choose.

An earlier public `Qwen/Qwen3-0.6B` download passed on `cell1`: seven files, a pinned revision and publisher checksums. Target selection, capacity suggestions, tool choice, disk checks, revision review, installation and recovery have passed offline tests only.

## Before you start

- You've chosen a model. See [Find a model that fits](help:find-a-model-that-fits). Look in your **Library** first: you may already have it.
- If this is your first download, you've run [Prepare the cluster for models](help:prepare-the-cluster), which checks Docker and free disk on every node.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the download is simulated and nothing reaches a node. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- For a gated model, you've saved a Hugging Face access token and accepted its license. See [Get a Hugging Face token](help:get-a-hugging-face-token).
- **Choose a connected node in Download node.** The first connected node is selected initially. Use **Suggest a download node** to compare disks before choosing. Choose **Then copy to** to include empty destination nodes in this download, or [copy it](help:copy-a-model-to-other-nodes) afterward.
- **The destination needs room for the full download plus 5% and 20 GiB.** SparkBox reads that filesystem before asking you to approve the download.
- **You have time.** How long it takes depends on your internet speed and the model's size. Models can be more than 100 [GiB](help:glossary#gib). This uses your internet allowance, if you have one.
- **Admin mode may lock while you wait.** That doesn't stop the download.

> [!WARNING]
> **Keep SparkBox open and this Mac awake until the download ends.** The job runs on the node and is built to keep going by itself, but that has never been proven on your cluster. SparkBox doesn't keep this Mac awake for you. [Keep this Mac awake during long work](help:keep-this-mac-awake) shows how. If SparkBox closes or this Mac sleeps anyway, open SparkBox and look at the **Jobs** tab. See [Follow a download](help:follow-a-download).

## Steps

### Find the size

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=hf) The **Models** window opens.
2. Click the **Hugging Face** tab.
3. Click a model under **Recommended for GB10**, or search for one and click its row. The right side shows the model's facts. In **Download node**, choose the node that will receive it, such as `cell2`.
4. In **Branch or tag**, select the version you want, or leave `main`. You can also type a branch, tag or full commit in **Revision**, then click **Inspect revision**.
5. Read **Commit** and expand **Files to download**. A commit names one fixed version. Check the file names and sizes.
6. Read the line that starts with **Downloads**, such as "Downloads 60.8 GiB of the repo's 182 GiB". This is what the node needs to store. SparkBox leaves out copies the engine doesn't serve.


### Compare nodes, if needed

To compare before downloading, expand **Suggest a download node**. Select the nodes you are considering for the load, then click **Compare destination capacity**. Read each result. The suggestion has the most space left after the required reserve. Click **Use cell2**, or the name shown, to change **Download node**.

This comparison reads only and needs no Admin unlock. It never changes your choice by itself. Finished or running copies are excluded. Failed checks remain visible; if no node qualifies, choose nothing and resolve the space or connection problem. Results expire after one minute. Comparing does not reserve space or set load members.

### Check the disk and start

7. Click **Download to cell2**, the hard drive with a down arrow. If Admin mode is locked, click **Unlock…**, then confirm with Touch ID or your Mac password.
8. In **Choose a download tool on cell2**, select a tool and read its location or image ID. Click **Continue**. This does not install or download.
9. Read **Destination disk** in the final dialog. It names the node, destination, available space and required space. SparkBox reads the filesystem containing that folder, or its nearest existing parent if the folder does not exist yet. It does not rely on the Dashboard's `/` disk reading.
10. If there is enough room, review the tool and file details. Click **Download** for an existing tool, or **Install tool and download** for the displayed installation. **Cancel** changes nothing. A refusal shows the problem and offers no start button.
11. Follow the new row on **Jobs**. **Download tool** shows the tool and version after its check.


### Download, then copy to other nodes

Before clicking **Download to cell2**, expand **Then copy to** and select the other nodes. A **chain** is this one approved sequence: download, verify, then copy. Changing the download node or inspected version clears the copy choices. If a selected target disconnects, remove its checkmark or reconnect it; SparkBox never drops it from the plan by itself.

Every chosen node needs an empty destination folder, enough space and an installed `rsync` copy tool. Existing or incomplete copies are refused without deleting them. All nodes need distinct IPv4 addresses on the same fabric subnet, the network used between nodes. SparkBox does not install `rsync`, use a different network or change firewall rules.

After choosing a download tool, review every destination and disk result. Check the source, copy targets, fixed commit and temporary source port. Click **Start download and copies**, or **Install tool and start chain** if you approved tool installation. Canceling starts nothing. SparkBox rechecks the files, tool, paths, fabric and disks at start.

Keep SparkBox open until every job appears in **Jobs**. The targets wait on their nodes. The source downloads and checks publisher sizes and any available publisher checksums. Only then does it share the files. Each target checks fresh source SHA-256 hashes, fingerprints that detect changed file contents. A fresh source hash is not a publisher checksum when none was supplied.

The approved jobs can then continue without the Mac. Each has a deadline of at most 24 hours from setup. The temporary file server closes after all targets report completion, cancellation, expiry or 30 minutes without activity. A failed copy or missing completion report makes the source chain fail too. This sequence stores files; it does not load the model.

Read every member's result in **Jobs**. The download and copy rows keep the same chain ID, including after reopening SparkBox. Select any chain row for its deadline, fixed commit and recovery instructions. **Cancel chain…** stops this SparkBox's remaining jobs and file server; it keeps files and records. An unreachable node needs cancellation after reconnection, or its deadline must expire.

If the source fails before sharing, targets keep waiting until canceled or expired. If setup is interrupted, reconnect and inspect every selected node's jobs. Some targets may be waiting even if the source never started. Reopening SparkBox follows records; it never starts replacement work. A rebooted node needs explicit recovery. Cancel remaining chain jobs first. Recover an incomplete source with a separately reviewed download, then use Library → **Copy to nodes…** for unfinished targets. Ordinary **Resume download…** is disabled for chain rows so it cannot discard the approved copy plan.

### Destination disk

The required space is the full transfer size, plus 5%, plus 20 GiB. Partial files do not reduce this requirement: they may need replacing. SparkBox uses the space available to the node's user, excluding blocks reserved for root. Unknown file sizes or disk readings, and read-only filesystems, stop the attempt before it starts.

A warning appears if the disk is already more than 85% full or would become more than 85% full after the transfer. You can continue if the required space is still available. Read the warning first and consider another destination node or freeing space.


SparkBox checks again when you approve and on the node before it takes ownership of the folder. This is a fresh reading, not a reservation: another job can still fill the disk later. Tool installations and caches may use another filesystem; check those disks separately when needed. Nothing is deleted to make room.

### If you choose installation

SparkBox offers **Install SparkBox hf 2.4.0 + hf-xet 1.8.0** on ARM64 Linux with Python 3.10 or newer already installed. It downloads a checksum-verified `uv` installer from GitHub, then the named packages and their dependencies from PyPI. These are download tools, not the model.

The tools stay under `~/.sparkbox/bin` and `~/.sparkbox/venv` on that node. No system packages or Python are installed, and existing tool folders are not replaced. An ordinary cancellation or error removes the new staging folder; an abrupt shutdown may leave files for inspection. A completed tool installation remains available even if the model download later fails.

If an existing tool folder cannot be verified, SparkBox leaves it alone. Choose a different tool or have the folder inspected. There is no repair or uninstall button.

The download now runs on the node. [Follow a download](help:follow-a-download) explains each column while you wait.

## What you should see

On the **Jobs** tab, the row's **State** goes from "starting" through "downloading" to "verifying", then **done**. Then the **Library** tab lists the model, with **✓ verified** under the selected node's column.

## Why it works

SparkBox pins the download to the full commit and file list you inspected. Before starting, it checks that version again and refuses a different repository, commit, file name, size or checksum. It fetches only the files the engine serves, the same list the size came from. At the end, it checks each file's size, and each large file's **checksum**, a fingerprint of its contents, against the one Hugging Face publishes. The files land in `~/models/<org>/<name>` on the node, and a record of the check gives the **✓ verified** mark.

## Limits

Choosing a revision here affects new downloads. It does not replace a finished Library copy. **Load…** uses that saved copy and leaves out AI advice for a different revision.

- Jobs **Resume download…** stays on its original node. Choosing a different download node does not move partial files.
- The disk check covers the model destination. It does not reserve space, predict quotas or inode limits, or guarantee space on separate tool/cache filesystems.
- A stopped download with a saved plan has **Resume download…** on **Jobs**. Older jobs need a newly reviewed download from **Hugging Face**. See [Start again](help:follow-a-download#start-again).

## If something goes wrong

The **Details** column on the **Jobs** tab shows why a job failed.

- **The reviewed revision or file list no longer matches.** Click **Inspect revision** and review the refreshed commit and files. Start again only if you agree. SparkBox does not switch to a newer version by itself.
- **config.json shows HTTP 401 or 403.** Hugging Face refused access. Accept the model license and check your token in **Settings › Models**, then inspect again. The fit table, download and AI advice stay unavailable until SparkBox can read the configuration.
- **config.json shows HTTP 404.** That configuration file is missing at this commit. Check the selected revision. A missing or unreadable shard index also stops inspection; SparkBox does not guess which files to fetch.
- **Branches and tags cannot be read.** Click **Refresh branches** after checking access. You can still enter a known branch, tag or full commit in **Revision** and inspect it.

- **"Not enough disk space on …"** Free space on the named destination or choose another download node, then review again. Existing jobs and partial files stay intact.
- **"Cannot check destination disk …"** Reconnect the node and check that the destination folder or its parent is accessible. SparkBox will not guess the free space.
- **"No space left on device …"** A filesystem filled after the check. Read the named node and current destination space, then inspect its destination and tool/cache disks. Partial files remain for a reviewed retry.
- **"hf download failed (exit 1): …"** The download tool stopped. Read the rest of the message. "No space left on device" means the disk filled up. A 401 or 403 means Hugging Face refused access: see [Get a Hugging Face token](help:get-a-hugging-face-token).
- **"The download tool changed. Review the tool choice again; no fallback was started."** Open the tool choice again. SparkBox will not use a replacement without a new review.
- **"Download incomplete: …"** A file came down wrong. The message names it, such as "… has the wrong checksum". [Start again](help:follow-a-download#start-again).
- **"The job stopped unexpectedly (node restarted?). Its container may still be running: Resume download stops it first."** The job's program ended without finishing. This usually means the node restarted. [Start again](help:follow-a-download#start-again).
- **You see "On cell1 (~/models/…)".** A finished copy is available on `cell1`. Selecting `cell1` hides its Download button; choosing another node can still offer a new download.
- **You see "Incomplete on cell1" and Download again to cell1.** A download stopped partway. See [Start again](help:follow-a-download#start-again).
- **You see "Downloading on cell1" and no button, or the message "A download of Qwen/Qwen3.8-27B-FP8 is already running on cell1. Follow it in Jobs."** A job is already filling that folder. Follow it on the **Jobs** tab.
- **The selected node is not connected or was removed.** Reconnect it or choose another node in **Download node**. SparkBox does not choose a replacement by itself.
- **Download to cell1 is dimmed despite a connected node.** If the screen says "This repo has no safetensors or .bin weights for vLLM to serve", SparkBox can't download this repository. See [Model formats and quantization](help:formats-and-quantization).
- **"… has no weight files SparkBox can serve."** The same thing, found at the last moment.

For more messages, see [Model messages](help:errors-models).

## Next steps

- [Follow a download](help:follow-a-download)
- [Copy a model to other nodes](help:copy-a-model-to-other-nodes), to load it anywhere else
- [Load a model on one node](help:load-a-model-on-one-node)
