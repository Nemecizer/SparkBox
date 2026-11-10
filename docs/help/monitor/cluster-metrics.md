---
id: cluster-metrics
title: Cluster metrics
type: feature
area: monitor
summary: Charts for every node and model side by side, with a GPU heatmap. Open it from the command palette.
keywords: [charts, heatmap, metrics, history, compare nodes]
verified: false
related: [read-the-charts, dashboard, command-palette]
---

# Cluster metrics

## What it's for

**Cluster metrics** puts every node's charts side by side. Use it to compare nodes, such as finding the one GPU that runs hottest. It also looks further back than the **Dashboard**'s 15 minutes.

## Where to find it

- **Sidebar:** under **Overview**, click **Cluster metrics**.
- **Command palette:** press <kbd>⌘K</kbd>, type `Cluster metrics`, and press <kbd>Return</kbd>.
- **Link:** [Open Cluster metrics](open:cluster-metrics)

## What's on the screen


### The toolbar

Choose a time range: **5m**, **15m**, **1h**, **6h**, **24h** or **7d**. The window opens on **1h**. Beside it, choose **Charts** or **GPU heatmap**.

If you drag across a chart to zoom, the toolbar shows **Paused** and **Back to live**.

### Charts

Each chart measures one thing, with one row for each node, so you can read down the rows and compare. A node that isn't reporting is hatched.

| Chart | What it shows |
|---|---|
| **GPU utilization** | How busy each GPU is |
| **CPU** | How busy each processor is |
| **Unified memory used** | Memory in use, in GiB |
| **GPU power** | Power in watts |
| **GPU temperature** | Heat, with a line at 85 °C where a warning starts |
| **Fabric received** | Data received on the fast 200G network. The number at the right is the average of the last 10 seconds |
| **Fabric sent** | Data sent on the same network, measured the same way |
| **Root disk writes** | How fast each main disk is being written |

Moving the pointer over one chart marks the same moment on all of them. [Read the charts](help:read-the-charts) shows how to zoom and copy the data.

### Serving

When a model is **Ready** or **Loading**, a **Serving** section appears below the charts, with one line for each model on these four charts:

- **Generation:** how fast it writes, in tokens per second. A **token** is a small piece of text, about three quarters of a word.
- **Time to first token, p95:** how long a request waits for its first word. The p95 figure means 95% of requests were faster than this.
- **Requests waiting:** requests queued because the model is full.
- **KV cache:** how full the memory it uses to keep track of the text so far is.

### GPU heatmap

The heatmap shows GPU use as a grid, with one row for each node and 120 cells across. Each cell is the average for that stretch of time, and the scale at the top shows what the colors mean. A hatched cell had no readings. Point at a cell to read its node, time and value. It's a quick way to see when each GPU was busy over a long range.

## What it changes

**Cluster metrics** changes nothing. It only reads, and needs no Admin mode.

## Related How-Tos

- [Read the charts](help:read-the-charts)
- [Read the Dashboard](help:read-the-dashboard)
- [Tell whether a model is working](help:tell-if-a-model-is-working)

![Cluster metrics, using synthetic Demo data](img/public-cluster-metrics.jpg)
