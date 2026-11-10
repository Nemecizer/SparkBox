---
id: check-disk-space
title: Check disk space on a node
type: howto
area: monitor
summary: See how full each node's disks are and find what's using the space.
keywords: [disk, storage, full, free space, df, disk left, space left, how much space, hard drive, hard drive full, drive full, disk full, out of space, out of disk space]
verified: false
related: [node-window, delete-a-model, check-docker-images]
---

# Check disk space on a node

To see how full a node's disks are, open the node's window and look at its **Disks** tab.

## Before you start

- You don't need Admin mode. Everything here only reads, so it also works in look-only mode.
- With **Demo** data you see the simulated disks. To see your own, use **Live** data. [Switch between Demo and Live](help:switch-between-demo-and-live) shows how.
- It takes less than a minute.

## Steps

1. In the sidebar, under **Overview**, click **Dashboard**. [Open Dashboard](open:dashboard) The **Dashboard** window opens with one row for each node.
2. Find the **Disk** column. It shows each node's root disk as a bar, with the percent used and how much is free, such as "30% · 2.6 TiB free". The root disk is the main disk, the one that holds the system. If you don't see the column, make the window wider.
3. Click the bar for the node you want to check, such as `cell1`. The node's window opens on its **Disks** tab.
4. Read the table. It has one row for each disk. **Free** is the space you can still use. **Used** shows the same bar with the amount used.
5. To find what's using the space, check the two things that usually take the most room:
   - Models. In the sidebar, under **Workloads**, click **Models**. The **Size** column on the **Library** tab shows how big each model is.
   - Docker images. Open the [Images](open:images) window. The **Size** column shows how big each one is.


## What you should see

The **Disks** tab shows one row for each disk, with its **Size**, **Free** and **Used**. A row changes color when the disk is nearly full: at 85% it shows a warning, and at 95% it shows as critical. Under the table, the **Root disk throughput** chart shows how fast the root disk is being read and written.

![The node overview, using synthetic Demo data](img/public-node-window.jpg)

## Why it works

SparkBox reads each node's disk figures over SSH, the secure connection it uses for everything on a node. It reports them the way the node's own `df` command does. **Free** is the space a normal user can still write to, and **Used** leaves out the small share the system keeps for itself. Validate the results on your own equipment before relying on them.

## If something goes wrong

- **"No disks reported."** The node hasn't sent its disk figures. Check that the sidebar shows the node's GPU use, such as **GPU 28%**, then look again. If it shows **Offline**, see [A node shows Offline](help:troubleshooting#a-node-shows-offline).
- **A disk shows as critical.** Free up space before you download anything. A full root disk stops a node from working normally. [Delete a model from a node](help:delete-a-model) frees the most space.

## Next steps

- [Delete a model from a node](help:delete-a-model)
- [Check Docker images](help:check-docker-images)
- [Change alert rules](help:change-alert-rules), to choose when a full disk warns you
