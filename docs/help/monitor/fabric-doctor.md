---
id: fabric-doctor
title: "Fabric: Doctor tab"
type: feature
area: network
summary: Runs MTU checks, jumbo pings, and bandwidth and latency tests between pairs of nodes.
keywords: [doctor, perftest, ib_write_bw, baseline, bandwidth, latency, jumbo]
verified: false
related: [test-the-fabric, fabric, fabric-and-rdma]
---

# Fabric: Doctor tab

## What it's for

The **Doctor** tab in **Fabric** measures how the fabric really performs, by sending test traffic between pairs of nodes. The **Matrix** tab reports what's happening. This tab actively tests. Use it after you change a cable or an optic, before you trust a several-node load, or when a link seems slow.

## Where to find it

- **Fabric window:** click the **Doctor** tab, or **Test fabric…** in the toolbar.
- **Command palette:** press <kbd>⌘K</kbd>, then type `Test fabric`.
- **Link:** [Open the Doctor tab](open:fabric?tab=doctor)

## What's on the screen


### The toolbar

**Test fabric** starts a run, and it needs Admin mode. During a run it becomes **Stop**. **Clear baselines…** forgets the saved speeds.

### Nodes

Select the nodes to test. Every online node is selected at first. An offline node can't be selected, and reads "· offline".

### Checks

The first four checks are selected at first. **All-reduce (nccl-tests)** isn't.

#### MTU audit

Checks that every fabric interface is at 9000, and that its switch port allows at least that. MTU is the largest packet size.

#### Jumbo-frame ping

Sends a 9000-byte packet that may not be split, between every pair on each half.

#### Bandwidth (ib_write_bw)

Writes over RDMA, the fast direct path, for 5 s with 4 parallel streams, one pair of nodes at a time. A GB10 half delivers about 92 Gb/s.

#### Latency (ib_send_lat)

Times small messages. It's about 2 µs between Sparks through the switch.

#### All-reduce (nccl-tests)

Runs a test across the chosen nodes with `mpirun`, and reports bus bandwidth. It needs nccl-tests built on the nodes.

### Expected per half

What you expect one half to deliver, in Gb/s. The default is `92`. A pair fails below 85% of this figure.

### Baselines

A **baseline** is a pair's speed, saved from the first run where every pair passes. Later runs compare against it, and a pair below 85% of its baseline gets a warning, even if it still clears the expected speed. That shows a link getting worse.

**Clear baselines…** forgets them all. The next run where every pair passes becomes the new baseline. Do this after you change hardware on purpose.

### Results

The heading shows what's running, then **Everything passed**, a count of findings, or **Stopped**. Beside it are the start time, **Demo** if it ran on Demo data, and "saved as the baseline" if it was.

A yellow notice appears if perftest is missing on a node, with **Install…**. That node's pairs are skipped. [What it changes](#what-it-changes) says what **Install…** does to the node. A list below names each finding, worst first, such as "cell2→cell3 half 2: 61 Gb/s, 34% below its baseline of 93 Gb/s".

**MTU audit** lists each interface with its **MTU**, its **Switch port**, that port's **Port L2MTU**, and a **Result**.

**Half 1** and **Half 2** are grids. Each row sends and each column receives. Each cell shows the jumbo ping, such as "ping ok · 0.12 ms". Below it are the pair's speed and latency, such as "92.1 Gb/s · 2.01 µs". Speed and latency are measured once for each pair, from the first node to the second, so the same figures show both ways.

| Word in a cell | What it means |
|---|---|
| **ping ok** | The large packet got through |
| **fails: MTU** | The large packet didn't. Small packets may still work |
| **unreachable** | No reply on that half |
| **measuring…** | The test is running |
| **skipped** | The test couldn't run, such as when perftest is missing |

Red marks a failure and amber a warning. Move the pointer over a cell for the reason.

### Both halves

A node's two fabric halves are separate paths into the same 200G port. Together they carry what NCCL gets between two Sparks, and NCCL is what a several-node model uses. This table adds them up for each pair.

| Column | What it shows |
|---|---|
| **Pair** | The two nodes |
| **Half 1**, **Half 2** | Each half's speed |
| **Together** | The two added up |
| **Expected** | Twice **Expected per half** |
| **Result** | **Pass**, or what went wrong |

**All-reduce** appears below if you ran that check. It's a final test that moves data the way a real several-node model does.

## What it changes

| Action | What it changes | Needs | How to undo |
|---|---|---|---|
| **Test fabric** | Runs test programs on the nodes, then stops them. Each command goes in the **Activity log**. Saves a baseline on this Mac after the first run where all pass | Admin mode | Click **Stop**. A test server left running stops by itself after 30 seconds |
| **Install…** | Runs `sudo apt-get install -y perftest` on the node. apt also adds any packages perftest needs, and all of them stay after the test. perftest needs `libibverbs1`, which is part of the [GPU stack](help:glossary#gpu-stack), and apt upgrades it if the node's copy is too old. SparkBox doesn't check this first, although its dialog says "nothing else changes". [If perftest is missing](help:test-the-fabric#if-perftest-is-missing) shows how to check | Admin mode | [Remove perftest](help:test-the-fabric#remove-perftest) with a command. That removes perftest alone: packages that came with it stay, and an upgrade isn't reversed |
| **Clear baselines…** | Forgets the saved speeds on this Mac | Admin mode, Touch ID or your Mac password | Run the test again |

> [!WARNING]
> **The bandwidth test fills the link.** It slows anything else using the fabric while it runs. Run it when nothing important is serving. To undo it, click **Stop**. The test itself leaves nothing on the nodes.

## Related How-Tos

- [Test the fabric](help:test-the-fabric)
- [Check the fabric](help:check-the-fabric)
- [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes)
