---
id: see-pending-updates
title: See what updates are waiting
type: howto
area: updates
summary: Read the update counts in the sidebar and the Updates window without changing anything.
keywords: [updates, pending, security, how many]
verified: false
related: [updates, check-for-updates, packages-and-updates]
---

# See what updates are waiting

To see which software updates are waiting, read the **Updates** row in the sidebar, or open the **Updates** window.

## Before you start

- You don't need Admin mode. Everything here only reads, so it also works in look-only mode.
- The number in the sidebar appears with **Live** data only. With **Demo** data the sidebar shows no number, and the **Updates** window lists simulated packages. [Switch between Demo and Live](help:switch-between-demo-and-live) shows how to change.
- It takes less than a minute.

## Steps

1. In the sidebar, find **Updates** under **Tools**. Beside it you see the number of pending packages on all your nodes together. If any are security fixes, you see an amber badge such as `3 security` instead. A **package** is one piece of installable software, and a pending package has a newer version available.
2. Move the pointer over **Updates**. A tooltip gives one line for each node, with what's waiting there.
3. Click **Updates**. [Open Updates](open:updates) The **Updates** window opens. The table at the top has one row for each node.
4. Read the row for a node. **Pending** is how many packages can be upgraded. **Security** is how many of those are security fixes. **Needs care** is how many belong to the GPU stack, the software a node needs to use its GPU and its fast network. **Restart** says **needed** if the node is waiting for a restart.
5. Click a node's row. The list below it shows that node's pending packages, with the version each one would move from and to. Badges mark **security** and **held** packages, and what kind of GPU-stack package each one is, such as **kernel** or **NVIDIA**.
6. To find one package, type part of its name in **Filter packages**, at the right of the toolbar.


The [Updates](help:updates#the-nodes-table) guide explains every column.

## What you should see

Each node's row shows its counts and, under **Checked**, when SparkBox last checked it. A node with nothing pending says "cell2 is up to date" below the table, with "Package lists checked 2 h ago."

## Why it works

SparkBox reads these numbers in the background, whether or not the window is open. It asks apt, the program that installs software on a node, what could be upgraded. This only reads, and it doesn't use sudo. Run these checks on your own configured nodes. The check reports what the node already knows, and [Check for updates now](help:check-for-updates) fetches new lists.

## If something goes wrong

- **The sidebar shows no number.** You're on **Demo** data, no node has been checked yet, or nothing is pending. Open the **Updates** window to see which.
- **A node's Pending column says "couldn't read".** Move the pointer over those words to see why. Check that the sidebar shows the node's GPU use, such as **GPU 28%**. If it doesn't, see [Connection and login messages](help:errors-connection).
- **"Not in the update check yet."** The node hasn't been checked since it connected. A check starts within seconds of a node connecting. Wait a minute and look again.
- **The numbers look out of date.** Look at the **Checked** column, then follow [Check for updates now](help:check-for-updates).

## Next steps

- [Check for updates now](help:check-for-updates)
- [Simulate an update](help:simulate-an-update)
- [Packages, updates and the GPU stack](help:packages-and-updates)
