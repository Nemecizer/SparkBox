---
id: recover-a-frozen-node
title: Recover a frozen node
type: howto
area: ops
summary: Get a node that has stopped answering running again from its power button, then find out why it froze.
keywords: [frozen, freeze, froze, hung, hang, stuck, node frozen, node is stuck, node hung, node not responding, spark frozen, spark froze, power button, hold power button, force off, power cycle, hard reset, out of memory, ran out of memory]
verified: false
related: [restart-a-node, find-out-why-a-node-restarted, memory-on-a-spark, choose-load-settings]
---

# Recover a frozen node

To get a frozen node running again, go to it and hold its power button until it turns off, then press the button once.

## Before you start

- A node is **frozen** when it's still on but has stopped answering. A Spark that runs out of unified memory, the one memory pool its CPU and GPU share, can freeze rather than fail cleanly.
- SparkBox can't fix this from where you are. A restart and a shutdown both need the node to answer, and **Wake** only starts a node that's off. Someone has to go to the machine.
- You don't need Admin mode for the power button. Unloading a model afterward needs it. See [Unlock Admin mode](help:unlock-admin-mode).
- The node usually takes 2–3 minutes to come back. Checking it afterward needs **Live** data.
- Check [Tasks](open:tasks) for an upgrade on this node. If one is running, read [If something goes wrong](#if-something-goes-wrong) first.

> [!DANGER]
> **Holding the power button stops everything on the node at once, and that can't be undone.** Models it serves stop, a model spread across nodes stops working, and anything the node hadn't saved is lost. NVIDIA warns that powering down during an update can cause system damage. Use the button only when the node doesn't answer in any other way.

## Steps

### Make sure the node is frozen

1. Look at the sidebar, under **Nodes**. A frozen node shows **Offline**, and the others still show their GPU use, such as **GPU 28%**. If every node shows **Offline**, the node probably isn't the problem. Stop here and see [Nothing connects](help:troubleshooting#nothing-connects).
2. In the status bar, move the pointer over the node. The tooltip gives the reason, such as "No SSH reply". [Connection and login messages](help:errors-connection) lists each reason.
3. Wait 5 minutes. SparkBox keeps retrying, so a short network drop clears by itself. A node that restarted on its own is usually back in 2–3 minutes.
4. Go to the node. Check that its power adapter and network cable are in, at both ends. If a cable had come out, plug it in. SparkBox reconnects by itself, so stop here.

### Turn the node off and on

5. Find the power button. It's on the back of the node.
6. Hold the button down for 5 seconds, then let go. You may see no change at the machine. NVIDIA's own Spark has no light that shows whether it's on.
7. Wait 5 seconds.
8. Press the button once, and let go. The node starts.

### Check that it came back

9. In SparkBox, watch the node in the sidebar. You don't need to do anything, because SparkBox keeps trying to reach it. It shows **Connecting**, then its GPU use, such as **GPU 28%**.
10. Click the node. The node's window opens. The top line reads like "up 3 min · restarted on its own 6:08 PM · no clean shutdown".
11. Click the **System** tab, and scroll to **Boots**.
12. Read **How it ended** in the row below **Running now**. It should say **Ended without a shutdown (power loss, hang or watchdog)**. That's what a freeze and a forced stop look like, so it doesn't mean a new problem.
13. If that row has a line under the table, such as "Last lines of the boot that ended Oct 29 at 6:08 PM", click it. The lines nearest the end matter most.

### Stop it happening again

> [!WARNING]
> **A model with Keep running starts again by itself, with the settings it had.** If those settings froze the node, it can freeze again. To stop it, unload the model as soon as the node is back. Unloading can be undone: load the model again when its settings are lower.

14. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) You see one card for each model server.
15. Find the model that was running on this node. If its card reads **Loading** or **Ready**, it has started again by itself. Click **Unload…**, then follow [Unload a model](help:unload-a-model).
16. To see whether memory was the cause, open [Alerts](open:alerts) and click **History**. An alert about memory right before the node froze points to it. No alert doesn't rule it out, because memory can fill in moments.
17. Before you load that model again, lower **GPU memory share** or **Max context**. [Choose load settings](help:choose-load-settings) explains both. Don't use **Override memory guardrail** to make a load fit.

## What you should see

The sidebar shows the node's GPU use again, such as **GPU 28%**. **Boots** has a new row reading **Running now**. In [Alerts](open:alerts), a warning says "cell3 restarted on its own at 6:08 PM". That's expected after you force a node off. A second restart within an hour turns it critical.

A model doesn't come back unless you turned on **Keep running** when you loaded it. Your model files stay on disk, so there's nothing to download again.

## Why it works

[NVIDIA's instructions](https://forums.developer.nvidia.com/t/dgx-spark-gb10-faq/347344) for a Spark that doesn't respond are to hold the power button for 5 seconds, wait 5 seconds, then press and release it. SparkBox can't do this for you, because everything it sends has to reach the node's software, and frozen software doesn't answer. The node starts with empty memory, so whatever filled it has gone. SparkBox then reads the node's own journal, which has no record of a shutdown, and that's why the boot reads **Ended without a shutdown**.

## Limits

- **You have to be at the machine.** A Spark has no separate management chip that could turn it off from afar, and SparkBox has no way to do it.
- **The hold time is NVIDIA's.** NVIDIA wrote these instructions for its own DGX Spark. Your nodes are MSI and ASUS models of it. [MSI's guide](https://download-2.msi.com/archive/mnu_exe/ipc/C931v1.1_English.pdf) puts the button on the back too, and gives no hold time.
- **SparkBox can't say why a node froze.** **Ended without a shutdown** can mean a power cut, a freeze or a watchdog, a safety timer that restarts a frozen computer. Memory is a likely cause on a Spark, but it isn't the only one.

## If something goes wrong

- **Holding the button for 5 seconds does nothing, or the node doesn't come back within 5 minutes.** Do steps 6 to 8 once more. If that doesn't help, unplug the power adapter from the wall, wait a minute, then plug it back in. A Spark starts as soon as it gets power, so don't press the button afterward.
- **It still doesn't come back.** Plug a screen into the node's HDMI socket and a keyboard into a USB-C socket. A standard USB keyboard needs a USB-C adapter. They show what the node is waiting for. If the screen stays black, contact the company you bought it from.
- **The node was installing updates when it froze.** Don't use the button yet. Apt, the program that installs packages, may still be running. Wait until the **Updates** panel reads "Lost cell2 while it was upgrading", which takes 30 minutes, then use the button only if the node still doesn't answer. When the node is back, SparkBox checks the packages and says what it found. If it says to run `sudo dpkg --configure -a`, read the list of packages first. If it names an NVIDIA or kernel package, stop and [get help first](help:errors-updates-and-power#the-upgrade-finished-but-dpkg-reports-packages-that-arent-fully-installed-on-cell2-run-sudo-dpkg-configure-a-there). Otherwise, open a [terminal](help:open-a-terminal) on the node and run that.
- **The node freezes again soon after.** Leave any model unloaded and watch the node. If it freezes with nothing heavy running, memory probably isn't the cause. Make a [Doctor report](help:collect-diagnostics) once the node is back, and contact the company you bought it from.
- **A load was starting when the node froze.** SparkBox waits for the node. If the load passes its 90-minute limit first, it reads "vLLM didn't become ready within 90 minutes." Once the node is back, see [Fix a load that failed](help:fix-a-load-that-failed).
- **The node shows Offline, but its screen shows a login prompt.** The node isn't frozen, so don't use the button. The problem is its network. See [A node shows Offline](help:troubleshooting#a-node-shows-offline).

## Next steps

- [Find out why a node restarted](help:find-out-why-a-node-restarted)
- [How memory works on a Spark](help:memory-on-a-spark)
- [Choose load settings](help:choose-load-settings)
- [Load a model on one node](help:load-a-model-on-one-node)
