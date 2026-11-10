---
id: read-the-dashboard
title: Read the Dashboard
type: howto
area: monitor
summary: Learn what each part of the Dashboard tells you, and spot a problem in a few seconds.
keywords: [dashboard, overview, health, what does this mean, status, 100 percent, is that bad, is that normal, what is normal]
verified: false
related: [dashboard, status-words, respond-to-an-alert, node-window]
---

# Read the Dashboard

To see at a glance whether your cluster is healthy, read the **Dashboard** from the top down.

## Before you start

- You don't need Admin mode. The **Dashboard** only reads, so it also works in look-only mode.
- With **Demo** data you see simulated nodes. To see your own, use **Live** data. [Switch between Demo and Live](help:switch-between-demo-and-live) shows how.
- It takes less than a minute.

## Steps

1. In the sidebar, under **Overview**, click **Dashboard**. [Open Dashboard](open:dashboard) The **Dashboard** window opens.
2. Read the band at the very top. A quiet line that starts with **All clear** means nothing needs you, such as "All clear · 4 nodes · nothing serving". An amber or red box means something does, and its heading counts the problems, such as "1 critical · 2 warnings".
3. If you see the amber or red box, click a line in it. The window for that problem opens. Click the box's heading instead to open [Alerts](open:alerts).
4. Look at the **Node** and **Status** columns of the table. The worst nodes sort to the top, and each healthy node says **Healthy**. A node with a problem also gets a second line that explains it in a sentence.
5. Read across a node's row. **CPU** and **GPU** show how busy each one is. **Memory** and **Disk** show how full each one is, as a bar. A bar turns amber or red as it nears full.
6. To look closer at one reading, click it. For example, click a node's **Disk** bar, and that node's window opens on its **Disks** tab.
7. Look at **Serving**, under the table. It shows a card for each model that's serving, or says "Nothing is serving." The **Capacity** card beside it shows how much memory is free on each node.
8. Read the four tiles, then the charts below them. They show the last 15 minutes, so you can see whether something changed recently.


## What you should see

On a healthy cluster:

- The top band reads **All clear**, followed by how many nodes report and what's serving.
- Every node's status reads **Healthy**.
- No bar in **Memory** or **Disk** is amber or red.

If your window is narrow, you see only the top band, the table and a one-line **Serving** summary. Click **Show charts** to see the rest, or make the window wider. Columns also step aside in a narrow window, and come back when you widen it.

## Why it works

SparkBox reads each node over SSH, the secure connection it uses for everything on a node, and refreshes the numbers every second. SparkBox judges those readings against your [alert rules](help:settings-alerts), so the colors here match the **Alerts** window, the sidebar and the top bar. The band at the top counts incidents, not single alerts: related alerts that share one cause count as one. Validate the results on your own equipment before relying on them.

## If something goes wrong

- **The band says "Checking health…".** SparkBox hasn't judged the first readings yet. Wait a few seconds.
- **A row says "Offline" or "No SSH reply".** SparkBox can't reach that node. Its numbers read as a dash, because SparkBox won't show old readings as if they were new. See [A node shows Offline](help:troubleshooting#a-node-shows-offline).
- **A row says "Not updating".** The node answers, but no fresh numbers have arrived. The row shows the last numbers it received. If this lasts, see [Connection and login messages](help:errors-connection).
- **A line reads "· confirming…".** SparkBox has seen something, but it hasn't held long enough to count. Wait and see. It either becomes an alert or goes away.
- **The band shows "2 known issues".** These are alerts you acknowledged or silenced. They don't count, but you can click the line to see them in [Alerts](open:alerts).

## Next steps

- [Respond to an alert](help:respond-to-an-alert)
- [Read the charts](help:read-the-charts)
- [Dashboard](help:dashboard), to see every column and tile
- [What the status words mean](help:status-words)
