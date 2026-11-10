---
id: get-a-gpu-driver-answering
title: Get a node's GPU driver answering again
type: howto
area: ops
summary: "Record what's wrong, restart the node from SparkBox so that its checks run first, then test the GPU."
keywords: [gpu driver, nvidia-smi, gpu not answering, gpu not responding, gpu stopped working, driver broken, if the gpu driver doesn't answer]
verified: false
related: [restart-a-node, check-your-nodes, collect-diagnostics, capture-evidence, recover-a-frozen-node, errors-connection]
---

# Get a node's GPU driver answering again

To get a node's GPU driver answering again, record what's wrong, then restart the node from SparkBox so that its checks run first.

## Before you start

- A **GPU driver** is the software that lets a node use its GPU. You need this page when the driver stops answering. You see one of two things: the sidebar shows **GPU —** for the node in place of a percentage, or the **GPU driver** check reads "nvidia-smi didn't answer". `nvidia-smi` is the tool that reports on the GPU.
- Don't load a model on this node until the GPU answers. A node whose driver doesn't answer can't run one.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode.
- The node must be online, so that SparkBox can tell it to restart. If it shows **Offline**, follow [Recover a frozen node](help:recover-a-frozen-node) instead.
- A restart stops everything on the node, including any model it serves. SparkBox says "usually 2–3 minutes" for the node to come back.
- Make sure someone can go to the node with a [screen and keyboard](help:trust-a-host-key#run-a-command-at-the-node). A node that doesn't come back can't be fixed from this Mac.

The cause may be small, but this is the most delicate part of a Spark, so go slowly and follow the steps in order.

> [!DANGER]
> **Installing or removing NVIDIA packages by hand can leave a node unable to start.** A package of this kind stopped machines starting in September 2026, and the only way back is at the machine. [Install updates on a node](help:install-updates) can't reinstall a driver, and there's no How-To for it.

## Steps

1. In the sidebar, under **Nodes**, click the node, such as `cell2`. The node's window opens.
2. Click the **GPU** tab. If **Utilization** shows a percentage, the driver is answering again, so go to step 7. If it shows **—**, read the reason under it, such as "nvidia-smi didn't answer in 10 s".
3. Keep a record of what's wrong, because a restart can clear it. Follow [Run the Doctor report](help:collect-diagnostics#run-the-doctor-report). It only reads.
4. Follow [Capture evidence](help:capture-evidence). It only reads too.
5. Follow [Restart a node](help:restart-a-node). Restart it only from SparkBox, not with a command or the power button. SparkBox first checks whether the node can start with its GPU, and refuses if it can't. If it refuses, stop here and see [If something goes wrong](#if-something-goes-wrong).
6. Wait until the sidebar shows the node's GPU use, such as **GPU 0%**. 0% is normal when no model is running.
7. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Nodes settings](open:settings?section=nodes)
8. In the list on the left, click **Nodes**.
9. In the node's row, click **Test**. macOS asks you to confirm with Touch ID or your Mac password. Confirm it.
10. Read **GPU driver** in the **Test of cell2** group, under the table. Then click **Done**.

## What you should see

The sidebar shows the node's GPU use, such as **GPU 0%**. In the **Test of cell2** group, **GPU driver** reads **Passed**, with the GPU's name and driver version beside it, such as "NVIDIA GB10, driver 580.178.04". The node can run a model again.

## Why it works

A restart loads the GPU driver afresh, so a fault that began while the node was running can clear. SparkBox checks first because the driver has to fit the **kernel**, the core of the operating system, that the node starts next. If they don't fit, the node may not start, so SparkBox refuses before it sends anything. The report and the evidence come first because a restart can clear the signs of what went wrong.

## Limits

- SparkBox can't repair a driver. It can restart the node and read what's there, and that's all.

## If something goes wrong

- **A dialog named cell2 can't restart now opens.** Stop here, and leave the node running. One of SparkBox's checks found that the node might not start. Don't look for a way around it. [If a check refuses the restart](help:restart-a-node#if-a-check-refuses-the-restart) lists each check. Keep the report from step 3, and ask NVIDIA or your Spark's maker for help.
- **"cell2 hasn't come back after 5 minutes. It may be stuck in firmware or waiting at a console."** Someone needs to go to the machine and look at its screen. If it's frozen, follow [Recover a frozen node](help:recover-a-frozen-node).
- **The sidebar still shows GPU —, or GPU driver still reads "nvidia-smi didn't answer".** Stop here. Don't try the updates in the DGX Dashboard: [Open the DGX Dashboard](help:open-the-dgx-dashboard) says why. Keep the report from step 3, and ask NVIDIA or your Spark's maker for help.

> [!DANGER]
> **NVIDIA's last resort erases the node.** [System Recovery](https://docs.nvidia.com/dgx/dgx-spark/system-recovery.html) returns a Spark to its factory state and erases its internal SSD, with every model and setting on it. It can't be undone. Get advice before you start it.

## Next steps

- [Check that your nodes are ready](help:check-your-nodes), to test every node
- [Load a model on one node](help:load-a-model-on-one-node), to put the node back to work
- [Pin the GPU stack](help:pin-the-gpu-stack), to keep the driver from changing
