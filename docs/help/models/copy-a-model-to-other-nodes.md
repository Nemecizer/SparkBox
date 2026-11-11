---
id: copy-a-model-to-other-nodes
title: "Copy a model to other nodes"
type: howto
area: models
summary: "Send a model from one node to the others over the fast fabric, ready to load on several nodes."
keywords: ["copy", "share", "fabric", "rsync", "other nodes", "incomplete copy", "retry a copy"]
verified: false
related: [models-library, load-a-model-on-several-nodes, keep-this-mac-awake]
---

# Copy a model to other nodes

To use a model on another node, copy it there over the fabric from a node that already has it.

The **fabric** is the fast 200G network between your nodes. Each node you load a model on needs its own copy.

For a **Then copy to** chain, inspect every node’s row in Jobs. Use **Cancel chain…** before recovery; ordinary Resume is disabled for these rows. [Download, then copy to other nodes](help:download-a-model#download-then-copy-to-other-nodes) explains the empty-folder requirement, deadline and recovery steps.

## Before you start

- The model is in your **Library** on at least one node, in your model folder. A model in the Hugging Face cache can't be copied yet.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the copy is simulated.
- **Each target needs room for the full copy plus 5% and 20 GiB.** The review shows each destination filesystem, available space and required space. A partial copy earns no space discount. A warning above 85% projected fullness asks you to consider freeing space first.
- Every node involved is online and has a fabric address. [Prepare the cluster for models](help:prepare-the-cluster) checks this.
- Copies and their completion acknowledgments run on the nodes. The source closes when every expected target finishes, or after 30 minutes without copy activity. Mac disconnection does not cancel a copy. This behavior has local fixture coverage; survival through real cluster interruptions still needs acceptance.

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=library) The **Library** tab shows every model folder on your nodes, with a column for each node.
2. Click the model's row.


3. Click **Copy to nodes…** in the toolbar. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A strip appears under the toolbar, reading **Copy to:**, with a box for each node. Nodes without the model are selected, and so is a node with an unfinished copy, which reads "· incomplete". Nodes with a finished copy read "· has it".
4. Deselect any node you don't want it on.
5. Click **Copy over the fabric**. Read **Destination disk** for every target, then click **Copy over the fabric** in the confirmation to approve. A low or unknown reading stops the attempt; no source or target job starts if the initial check refuses. Cancel starts nothing. After approval, a message reads "Copying …" and names the nodes, and the window moves to the **Jobs** tab.
6. Find one row for each target, such as `Copy Qwen/Qwen3.8-27B-FP8 to cell2`. There's also one row named `Share … from cell1`, whose **State** reads **serving**. Leave that row alone: it's the source of the copy, and canceling it stops every copy.
7. Wait until each `Copy` row reads **done**. See [Follow a download](help:follow-a-download) for what each column shows. While a copy runs, the **Library** reads **◐ copying** under that node, and you can't load the model there.
8. Click the **Library** tab. The model's row now reads **✓ verified** under each target node.

You can also start a copy from the **Load** page. If you select a node that has no copy, a note says so. With Admin mode unlocked, it offers **Copy it there**. This opens the same destination check and a separate confirmation; it does not load the model.

## What you should see

Each `Copy` row reads **done**, and the **Library** row reads **✓ verified** under each target. The source closes after the targets acknowledge completion. A source stopped by SparkBox's cleanup may read **canceled**; inspect the target jobs for their actual outcomes.

A live copy of `Qwen/Qwen3-0.6B` from `cell1` to `cell4` passed source and target hash checks after repairing the older temporary server's group-setting error. The temporary server then closed and its private transfer files were removed. A fresh copy to `cell2` then passed on its first attempt in about five seconds with the corrected app, without a repair or extra privileges. All seven files matched their source hashes, and the temporary server, private transfer files and listening port were gone afterward. Copy interruption and recovery still need separate live tests.

## Stop one copy

1. Open **Jobs** and select the target row named `Copy … to …`.
2. Click **Cancel**, check the target in the confirmation, then click **Cancel job**. Choose **Keep going** to leave it running.
3. Wait for that target to read **canceled**. Partial files stay on disk, and the Library marks the target **◐ incomplete**. Keep the files and job record for a later reviewed retry.

A copy may finish while you are deciding. If the target reads **done** and the Library shows **✓ verified**, keep the finished copy; that does not prove cancellation worked. The source `Share …` row can read **canceled** during normal cleanup even when every target finished. Read the target outcome before deciding what happened. Leave the source row alone while another target still needs it.

Two bounded live attempts copied a 28.8 GiB model successfully before cancellation could be confirmed. Their finished copies were retained. Active-copy cancellation and reuse of its partial files still need live acceptance.

## Why it works

SparkBox picks the source: the first node in your list whose copy reads **✓ verified** or **✓ sizes match**, or else a node with any finished copy. On that node it starts a temporary, read-only file server that takes a one-time password and accepts only fabric addresses. Each target pulls the files with `rsync`, a standard tool for copying folders, to the same path, because a load needs the same path on every node. Before sharing, the source hashes its files with SHA-256; each target compares those hashes before writing its own verified record. The Library tip distinguishes a copy verified against its source from a download verified against Hugging Face.

## Limits

- You can't choose the source node.
- All targets are checked again before the source starts. Each target also checks space against the source's fresh file list before taking ownership of its folder. Space is not reserved; another job can fill it later. A later failure may leave a failed job and partial files, which SparkBox keeps. The review and admission checks passed offline tests and two bounded live copies. Disk exhaustion during a transfer still needs a separate test.
- A target checks for rsync, source reachability and free space of at least 1.05 times the copy size plus 20 GiB. It waits for source hashing and tries a failed transfer up to three more times.
- Hashes establish that the target matches this source snapshot. They do not prove that the source matches its publisher. Root hidden metadata is excluded. Directory symlinks and links outside the supported source folder are refused; use a normal model folder for copying.
- A model in the Hugging Face cache, whose **Folder** starts `~/.cache/huggingface/hub/models--`, can't be copied yet. That folder holds links to files outside it. On a test system, `rsync` refused them and nothing arrived, so SparkBox doesn't try.
- The source uses a private password and accepts the selected fabric subnet. It reports firewall status but never changes firewall rules.

## If something goes wrong

- **The source says "serving", but the target keeps waiting for its file list.** The source may have failed to accept the transfer. An older SparkBox copy server tried to change Linux groups without root permission. The corrected version keeps the existing user and groups; it needs no extra privileges or firewall change. Stop the target job before retrying with the corrected app. Other connection failures can produce the same waiting message.
- **"… has no fabric address; run Prepare cluster in Models."** The source node has no address on the fabric. See [Prepare the cluster for models](help:prepare-the-cluster) and [Check the fabric](help:check-the-fabric).
- **"Pick at least one node that doesn't have it yet."** Every node you selected already has this model.
- **"No node has a finished copy of that model."** Every copy is unfinished, so there's nothing to copy from. Finish the download first. See [Start again](help:follow-a-download#start-again).
- **"A copy to cell2 is already running. Follow it in Jobs."** Wait for that job to end.
- **"Copying a Hugging Face cache model isn't supported yet."** This model's **Folder** is in the Hugging Face cache, so it can't be copied. See [Limits](#limits).
- **"That model isn't in the library any more."** The folder has gone since the list was read. Click **Find models** and look again.
- **"No space left on device"** in the **Details** column, or in the message **Couldn't start the copy**. A filesystem on that node filled up. Read its current destination space, inspect the destination and any separate cache disk, then free space and review the copy again. See [Check disk space on a node](help:check-disk-space).
- **"copy failed (rsync exit …): …"** The copy stopped partway. Check that both nodes are online and that the fabric is healthy, then copy again as described below. The partial files stay on the target, and the **Library** reads **◐ incomplete** there.
- **"Copy incomplete: …"** A file has the wrong size or SHA-256 hash. Copy again as described below.

**To copy again after a failure:**

1. On the **Library** tab, click the model's row. The target node's column reads **◐ incomplete**.
2. Click **Copy to nodes…**. That node reads "· incomplete" and is already selected.
3. Click **Copy over the fabric**, read the destination review, then approve **Copy over the fabric** again. The copy picks up where it stopped, and the mark changes to **✓ verified** when it's done.

For more messages, see [Model messages](help:errors-models).

## Next steps

- [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes)
- [Load a model on one node](help:load-a-model-on-one-node)
- [Models: Library tab](help:models-library)

## If a node could not be scanned

In Library, **last known** and **unknown** are not current copies. Reconnect the node and press **Find models** before selecting it. A failed scan leaves other nodes' results visible and preserves the last-known observation in this app session. SparkBox reads the folders again before Copy or Delete; an unavailable or changed copy blocks the action. See [Library scan results](help:models-library#scan-results).
