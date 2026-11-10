---
id: fabric
title: Fabric
type: feature
area: network
summary: The health of the 200G network between nodes, link by link, with NCCL checks and cabling.
keywords: [fabric, matrix, history, nccl, roce, pfc, counters, cabling]
verified: false
related: [check-the-fabric, fabric-doctor, fabric-and-rdma]
---

# Fabric

## What it's for

The **Fabric** window shows the health of the fast 200G network between your nodes, link by link. It answers three questions: is the fabric losing anything, is each several-node model really using the fast path, and is every cable where it was? [The fabric and RDMA](help:fabric-and-rdma) explains the ideas.

## Where to find it

- **Sidebar:** under **Network**, click **Fabric**. The row shows a warning when a link has a problem, and **PFC protection unverified** when the evidence is incomplete.
- **Command palette:** press <kbd>⌘K</kbd>, then type `Open Fabric`.
- **A node's window:** on the **Network** tab, click the arrow beside a half.
- **Link:** [Open Fabric](open:fabric)

## What's on the screen


### The toolbar and verdict band

**Test fabric…** opens the **Doctor** tab. **Accept current cabling…** is available when a cable has moved. It needs Admin mode, and Touch ID or your Mac password.

The band below sums up the fabric.

| Band | What it means |
|---|---|
| A quiet line, such as "Fabric healthy · node-side PFC protected · 200G per node · NCCL on RoCE" | All is well. The ⓘ gives the evidence |
| A gray note, "Fabric PFC needs verification." | The traffic priority is unknown or lacks PFC protection. Read the evidence: an idle port cannot prove which priority RoCE uses. SparkBox changes no settings |
| An amber or red band | It names the problem, gives the evidence and says what to do |

### Matrix

The tab has one row for each node and one column for each half of Port 2. **Port 2 · half 1** is on `192.168.100.x`, and **Port 2 · half 2** is on `192.168.101.x`.

Each cell shows:

- The verdict, such as **Node-side PFC protected** or **PFC protection unverified**, with the link speed and MTU. MTU is the largest packet size. It turns red if it isn't 9000.
- Traffic received and sent, with **RDMA** and the share that used the fast direct path.
- A small chart of the last 3 minutes.
- Chips for counters that moved in the last hour, such as **Retransmits**, **PHY errors**, **Drops** or **Pause**. A colored chip with a rate is moving now. A gray one, such as "+412 · 12 min ago", moved earlier. With no chips the cell reads "No errors in 1 h".
- The switch port, such as **Switch 1-2 · by MAC**, which says how SparkBox found it. **Expected 2-1** in amber means the cable moved. With no port matched, the cell shows what LLDP heard on that half, or **Switch port unknown**.

**Not cabled** means that half has no address.

Click a cell to open its panel on the right. Click it again, or the **×**, to close it.

### A cell's panel

The panel shows the verdict in words and a **Suggested action** if there's something to do. Below that, it lists **Link**, **MTU**, **Received**, **Sent**, **Switch port**, **Cabling** and **Loss, last hour**.

The counter table lists every counter the node reports.

| Column | What it shows |
|---|---|
| **Counter** | The counter's name. Its ⓘ says what a rise means |
| **Since reset** | Its total since the driver last reset it, usually at the node's start |
| **Now** | Its rate. It turns red when a loss, error or drop counter is moving |
| **1 h** | A small chart of the last hour |

### NCCL transport

This section appears below the matrix when a model runs across nodes. NCCL is the library a model uses to share work between GPUs. Each line shows the model and its nodes. It says how much of the model's fabric traffic used RDMA while it was generating, such as "NCCL over RoCE: 98% of fabric bytes are RDMA".

NCCL quietly falls back to TCP sockets when RDMA can't be used, which costs most of the bandwidth. Under half on RDMA reads "NCCL is using TCP sockets". Any setting that turns RDMA off, or points it the wrong way, is listed under the model.

### Cabling

This section appears when a cable has moved. SparkBox learned which switch port each node's fabric cable is in the first time it saw all of them. A line such as "cell2 half 1 is on 2-1; expected 1-2" means it moved. A node on another port still links, but port comments and per-port history no longer match it.

### Doctor

Tests real speed and latency between nodes. See [Fabric: Doctor tab](help:fabric-doctor).

### History

The last hour of one interface's counters, as rates. Choose the interface under **Interface**. It starts on the first one with a problem.

| Chart | What it shows |
|---|---|
| **Retransmits** | **Out of sequence**, **Sequence errors** and **ACK timeouts**: packets lost and sent again |
| **PHY errors and drops** | **CRC errors** and **Discards**: corrupted or dropped packets |
| **Congestion** | **ECN marked**, **CNPs sent** and **CNPs handled**: senders being slowed. A trickle under heavy traffic is normal |
| **Pause frames** | **Sent** and **Received**, with a line at 10,000 per second marked **Storm 10k/s** |

SparkBox keeps this history while it runs, so it appears after the first minute. [Read the charts](help:read-the-charts) shows how to use them.

## What it changes

The **Matrix** and **History** tabs only read.

| Action | What it changes | Needs | How to undo |
|---|---|---|---|
| **Accept current cabling…** | Records today's cabling on this Mac | Admin mode, Touch ID or your Mac password | Move the cables back, and accept again |
| **Test fabric** (Doctor) | Runs tests on the nodes. The test itself leaves nothing on them. **Install…**, for a missing tool, does: see [What it changes](help:fabric-doctor#what-it-changes) | Admin mode | Click **Stop** |

## Related How-Tos

- [Check the fabric](help:check-the-fabric)
- [Test the fabric](help:test-the-fabric)
- [See what a port connects to](help:see-what-a-port-connects-to)
- [Manage a switch port](help:manage-a-switch-port)

![Fabric, using synthetic Demo data](img/public-fabric.jpg)
