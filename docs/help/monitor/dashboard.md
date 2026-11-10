---
id: dashboard
title: Dashboard
type: feature
area: monitor
summary: "Your whole cluster on one page: node health, what's serving, capacity and activity."
keywords: [dashboard, overview, serving, capacity, tiles, node table, gpu usage, gpu utilization, how busy]
verified: false
related: [read-the-dashboard, node-window, status-words, top-bar-and-status-bar]
---

# Dashboard

## What it's for

The **Dashboard** is your whole cluster on one page. Use it first, each time you sit down, to see whether anything needs you. It shows each node's health and load, what's serving, how much room is left and how busy things were in the last 15 minutes.

Validate the results on your own equipment before relying on them.

## Where to find it

- **Sidebar:** under **Overview**, click **Dashboard**.
- **Command palette:** press <kbd>⌘K</kbd>, then type `Dashboard`.
- **Link:** [Open Dashboard](open:dashboard)

## What's on the screen


The window runs from the most urgent thing at the top to the least urgent at the bottom. Below 640 points wide, it shows the band, the table and one **Serving** line, with a **Show charts** button for the rest.

### The status band

The band at the very top says how things stand in one line.

| What you see | What it means |
|---|---|
| **All clear** · 4 nodes · nothing serving | Nothing counts as a problem. The line says how many nodes report and what's serving. It adds **watched 24/7** when [always-on monitoring](help:always-on-agent) is fully proven |
| **3 of 4 nodes reporting** | One node isn't sending readings |
| **1 critical · 2 warnings** | An amber or red box. It counts incidents, and lists up to four, each with how long it has lasted. Click a line to open what it's about. Click the heading to open **Alerts** |
| **confirming…** | Seen, but it hasn't held long enough to count |
| **Recovering · clears in 1:34** | The condition has gone. The alert ends when it has stayed gone that long |
| **Planned** | Work SparkBox started, such as a restart. It's never paged |
| **2 known issues** | Alerts you acknowledged or silenced. They don't count and don't color their node. Click to open **Alerts** |
| **Checking health…** | SparkBox hasn't judged the first readings yet |

[Alerts and incidents](help:alerts-and-incidents) explains these states.

### The node table

One row for each node. The worst nodes sort to the top, and a node that's offline or critical stays pinned there. Click a column heading to sort by it.

| Column | What it shows | Click to open |
|---|---|---|
| **Node** | The node's name and its state mark | The node's window |
| **Status** | A short state: **Healthy**, **Connecting**, **Offline · 35 s**, **Not updating**, or the problem itself, such as **Root disk 85% full**. A problem adds a line below with the full sentence | |
| **CPU** | How busy the processor is, with a small trend when it has moved | **CPU** tab |
| **GPU** | How busy the graphics processor is, with a small trend. A dash means the GPU can't be read | **GPU** tab |
| **Memory** | Unified memory in use, as a bar. Unified memory is the one pool the CPU and GPU share. Amber at 90%, red at 95% | **Memory** tab |
| **Power · temp** | GPU power in watts and temperature. It turns amber at 85 °C | |
| **Disk** | How full the root disk is, and how much is free. Amber at 85%, red at 95% | **Disks** tab |
| **Fabric ↓/↑ Mb/s** | Data received and sent on the fast 200G network, in megabits per second | **Network** tab |
| **LAN ↓/↑ Mb/s** | Data received and sent on your ordinary home network | **Network** tab |
| **Containers** | Containers running out of all containers, such as `3/5`, and how many exited with an error | **Containers** for that node |

A gray bar with a bell-slash mark means the alert behind it is a known issue. It's silenced or acknowledged, so SparkBox draws it quietly.

When a node isn't reporting, its state reads across the empty columns. If one action would help, such as trusting a host key, it appears as a link in the same place.

Double-click a row to open the node. Right-click a row for the same menu as the sidebar: **Open**, **Terminal**, **Files**, **Containers**, **Services**, **Restart…**, **Shut down…**, **Copy IP address** and **Copy hostname**.

### Serving

**Serving** shows every endpoint on the cluster, with a card for each one that's serving. An **endpoint** is a running model server you can talk to. A model is **Ready** only when every node it spans is online, its containers run and it answered a check in the last minute. [Endpoints](help:endpoints) explains the card.

With nothing serving, the card reads "Nothing is serving.", names any idle servers, and offers **Load a model…**. If a load failed in the last week, a card shows that too.

### Capacity

**Capacity** shows the unified memory free on each node, which is room for another model. **Most free** names the node with the most. A node whose GPU can't be read never gets that label, however much memory it has free. Click a row to open that node's **Memory** tab.

A model that needs more than the largest free figure won't load on one node. [Find a model that fits](help:find-a-model-that-fits) shows how to check.

### The four tiles

| Tile | What it shows |
|---|---|
| **GPUs serving** | Nodes whose GPU is running a **Ready** model, out of all nodes. Click to open **Endpoints** |
| **Unified memory** | Memory in use across the cluster, out of the total, with the percent used |
| **GPU power** | Total GPU power in watts. It leaves out any GPU that can't be read, and says so, such as "avg 24 W (3 of 4)" |
| **Fabric ↓ / ↑** | All fabric traffic received and sent, and the share of the full fabric in use. Click to open **Topology** |

### The charts

The four charts cover the last 15 minutes. [Read the charts](help:read-the-charts) shows how to zoom, hide a line and copy the data.

- **GPU utilization:** one row for each node.
- **Unified memory:** one row for each node, as a share of its memory, with a line at 90%.
- **Fabric throughput:** data sent by every node. The line is the average of each 5 seconds. The faint band runs from the slowest to the busiest second in them. **RDMA**, drawn dashed over the total, is the part that used the fabric's fast direct path, which [The fabric and RDMA](help:fabric-and-rdma) explains. Short, tall bursts sit on the top edge with ▲.
- **GPU power:** the total for the GPUs that report.

A band reading "No data recorded" marks missing readings. A gap can come from closing SparkBox, Mac sleep or reconnecting; the band does not identify the cause.

## What it changes

The **Dashboard** changes nothing. It only reads, so it works in look-only mode and needs no Admin mode. Actions in a row's right-click menu, such as **Restart…**, ask first and need Admin mode.

## Related How-Tos

- [Read the Dashboard](help:read-the-dashboard)
- [Read the charts](help:read-the-charts)
- [Check disk space on a node](help:check-disk-space)
- [Respond to an alert](help:respond-to-an-alert)

![Dashboard, using synthetic Demo data](img/public-dashboard.jpg)
