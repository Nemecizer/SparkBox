---
id: capture-evidence
title: Capture evidence
type: howto
area: ops
summary: Record the state of every connected node before and after a big change, and compare the two.
keywords: [evidence, before, after, snapshot, compare, diff]
verified: false
related: [settings-diagnostics, collect-diagnostics]
---

# Capture evidence

To record your nodes before and after a big change, click **Capture evidence** in **Settings › Diagnostics**, once each time. SparkBox writes down what's different.

## Before you start

- It works with **Live** data only.
- You don't need Admin mode. It only reads, so it also works in look-only mode.
- At least one node must be connected. A node that isn't connected is left out and named in the result.
- It saves files on this Mac only. Nothing on the nodes changes.
- Validate the results on your own equipment before relying on them.

## Steps

1. At the bottom of the sidebar, click **Settings**, then click **Diagnostics**. [Open Diagnostics settings](open:settings?section=diagnostics) The **Diagnostics** page opens.
2. Find the **Evidence** group.
3. Click the **Group** box. It starts with `general`. Press <kbd>⌘A</kbd> to select that, then type a short name for this piece of work, such as `november-update`. Use the same name for the captures you want to compare.
4. In **Label**, type what this capture marks, such as `before-update`.
5. Click **Capture evidence**. The button reads **Capturing…** while SparkBox reads each node. That can take a few minutes.
6. Read the **Saved** row. It shows the folder and a summary, such as "4 nodes".
7. Make your change, such as [installing updates](help:install-updates).
8. Come back, keep the same **Group**, and change **Label** to something like `after-update`.
9. Click **Capture evidence** again.
10. Click **Show in Finder** on the **Saved** row. Finder opens the new capture's folder.
11. Open the file that starts `diff-from-`. It lists what changed between the two captures.


## What you should see

A new folder in `~/Library/Logs/SparkBox/evidence`, named with the date and **Group**. Inside it is one folder for each capture, named with the time and **Label**.

Each capture holds a folder for each node, with one text file for each thing it read. It also has `manifest.json`, a record of what ran, and `activity.jsonl`, the activity log since the last capture. A copy of the SSH log is there if one exists.

The second capture adds `diff-from-` and the first capture's name. If nothing changed, that file says "No differences."

The last five captures are listed under the **Evidence** group, each with **Show in Finder**.

## Why it works

SparkBox runs a fixed list of read-only commands on every connected node at once, and gives each command 20 seconds. They cover containers, images, failed services, disk, models, apt, installed packages, the GPU, addresses, listening ports, memory and `/dev/shm`. It compares each file with the last finished capture that has the same **Group**, on any day. So one name keeps a piece of work together, and a new name starts fresh. Everything passes through the same filter as the activity log, so anything that looks like a secret is masked.

## If something goes wrong

- **"Available with Live data"** The button works with **Live** data only. See [Switch between Demo and Live](help:switch-between-demo-and-live).
- **"No node is connected, so there's nothing to capture."** Check your nodes in the sidebar. See [Connection and login messages](help:errors-connection).
- **The summary says "3 nodes · cell3 not connected".** The capture worked without that node. Reconnect it, and capture again if you need it.
- **The summary says "2 steps failed".** Some commands didn't work, often because a node doesn't have that tool or folder. The node's text file for each step shows why. The rest of the capture is fine.
- **There's no `diff-from-` file.** This is the first capture with that **Group**, so there's nothing to compare with.

## Next steps

- [Settings: Diagnostics](help:settings-diagnostics)
- [Collect diagnostics](help:collect-diagnostics)
- [Install updates on a node](help:install-updates)
