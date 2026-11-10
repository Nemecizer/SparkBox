---
id: tour-the-demo
title: Take a tour of the Demo
type: tutorial
area: start
summary: Explore a simulated copy of your cluster, where nothing you do can reach a real node.
keywords: [demo, practice, try, simulated]
verified: null
related: [demo-and-live, sparkbox-window, your-first-model, settings-demo-scenarios]
---

# Take a tour of the Demo

## What you'll do

You'll walk through SparkBox on simulated data. You'll read the cluster's health, open a node, unlock Admin mode, and watch SparkBox handle a fault. It takes about 15 minutes, and you don't need your nodes.

## Before you start

This tour uses the browser Demo's synthetic four-node inventory. The native app starts with no nodes or switches and does not add examples automatically. Native Demo uses only your saved inventory: if it is empty, explore the empty windows first or configure your own nodes with **Connect my Sparks** while staying in Demo. Do not save the example addresses as real connections.

- The top bar should read **Demo**. If it reads **Live**, follow [Switch between Demo and Live](help:switch-between-demo-and-live) first.
- The padlock shouldn't read **Look only**. If it does, follow [Use look-only mode](help:use-look-only-mode) to turn it off.

> [!NOTE]
> Demo simulates your cluster, not your Mac. What you do to a node here stays in the simulator. A password you save or a node you remove in **Settings** is real, so this tour stays away from those.

## Step 1: Open the Demo


1. Open SparkBox. If the **Welcome to SparkBox** screen appears, click **Explore the demo**. The **Dashboard** and **Topology** windows open side by side.
2. If you don't see that screen, go to the sidebar on the left. Under **Overview**, click **Dashboard**. [Open Dashboard](open:dashboard)
3. Look at the top of the window. This is the **top bar**. The **Demo** label says the data is simulated, and the padlock reads **Read-only**, so a change needs Admin mode first.
4. Look at the left of the window. This is the **sidebar**. It lists everything SparkBox can show you, grouped under **Overview**, **Nodes**, **Workloads**, **AI**, **Network** and **Tools**.

## Step 2: Read the cluster's health

1. In the **Dashboard** window, find the table of nodes. In the browser Demo, it has one row for each of `cell1`, `cell2`, `cell3` and `gx10-0000`.
2. Read across a row. The **Status** column says how the node is doing, and the next columns show how busy it is and how much memory it's using. The numbers move, as they would on real nodes.
3. Look at `cell1`. Its **Status** reads "1 unit failed", and a line above the table reads "1 warning". The Demo keeps one small fault so that you have something to look at.
4. Find the **Serving** line under the table. It names what's running now: `llama-3.3-70b` reads **Ready**, and so does Ollama on `gx10-0000`. Ollama is a second way to run models.
5. Click **Show charts**, below that line. The tiles and charts that open sum up the whole cluster. If the window is wide, they're already showing, so scroll down to them.

[Read the Dashboard](help:read-the-dashboard) explains each column.

## Step 3: Open a node

1. In the sidebar, under **Nodes**, click `cell2`. The window for `cell2` opens on its **Overview** tab.
2. Click the **GPU** tab. You see the GPU's load and the processes using it.
3. Click the **Disks** tab. You see how full each disk is.
4. Press <kbd>⌘W</kbd> to close the window.

Every window sits on the **desk**, the area in the middle of SparkBox. You can move and resize them, and [Windows and layouts](help:windows-and-layouts) shows how.

## Step 4: Make a change

In Demo, you can practice changes with nothing at risk.

1. Click the padlock in the top bar. It reads **Admin**, with a clock counting down. With Demo data, it doesn't ask for Touch ID.
2. In the sidebar, under **Nodes**, click `gx10-0000`. Its window opens.
3. In the window's toolbar, click **Restart…**. A dialog named **Restart gx10-0000?** lists what goes down with the node.
4. Read the list, then click **Restart**. If the dialog asks you to type the node's name, type `gx10-0000` first. The node shows **Restarting** in the sidebar.
5. Wait. When the simulated restart ends, the sidebar shows the node's GPU use again, such as **GPU 28%**.
6. In the sidebar, under **Tools**, click **Activity log**. The restart is in the list.
7. Click the padlock. It reads **Read-only** again.

On your real cluster, the same click asks for Touch ID or your Mac password, and the node really restarts.

## Step 5: Watch a fault

Demo can act out problems, so you can see what they look like before you meet a real one.

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Demo scenarios](open:settings?section=scenarios)
2. In the list on the left, click **Demo scenarios**.
3. Click **Node offline**. A label in the top bar shows that the scenario is playing, and when it next changes.
4. Watch the sidebar. Within a few seconds, `cell3` shows **Offline** and moves to the top of **Nodes**. It stays that way for about a minute, then recovers. The scenario repeats every 2 minutes.
5. Wait about 30 seconds. An alert takes that long to appear after a node goes offline.
6. In the sidebar, under **Overview**, click **Alerts**. A row for `cell3` reads "Offline · No SSH reply in 10 s". If you don't see it yet, wait a few more seconds. Any other row was there before the scenario started.
7. Go back to **Settings**, and click **Back to everyday serving**. The cluster returns to normal.

[Settings: Demo scenarios](help:settings-demo-scenarios) lists all the scenarios.

## What you learned

- The top bar tells you where you are: **Demo** or **Live**, and **Read-only** or **Admin**.
- The sidebar leads to everything, and each node has its own window.
- Changes need Admin mode, and SparkBox records each one in the activity log.
- Demo can act out faults, so you can practice before something goes wrong for real.

## Next steps

- [Rehearse running a model in Demo](help:your-first-model), to follow the whole path with nothing at risk
- [Connect your cluster](help:connect-your-cluster), to move on to your real nodes
- [The SparkBox window](help:sparkbox-window)
