---
id: fabric-and-rdma
title: The fabric and RDMA
type: concept
area: network
summary: Why your nodes have a second, much faster network, and what RDMA, RoCE and NCCL mean in plain words.
keywords: [rdma, roce, nccl, pfc, mtu, connectx, 200g, lossless, ecn, halves]
verified: null
related: [fabric, check-the-fabric, splitting-a-model, your-cluster]
---

# The fabric and RDMA

## In short

Your nodes have two networks. The **LAN** is your ordinary home network, which SparkBox uses to talk to them. The **fabric** is a second, much faster network that only the nodes use. It lets GPUs on different nodes share work as if they sat side by side, which is how one large model runs across several Sparks.

## How it works

![Two networks: the LAN joins this Mac and the nodes, and the fabric joins only the nodes](img/diagram-lan-and-fabric.svg)

### The hardware

Each Spark has a ConnectX-7 network card with two ports. Port 2 runs a cable to the CRS804 DDQ switch at 200G. That's 200 gigabits per second, about 200 times the 1G link each node has on your LAN.

The port connects to the rest of the Spark through two separate paths, called **halves**. Each half has its own address: half 1 on `192.168.100.x` and half 2 on `192.168.101.x`. They share one cable. One half delivers about 92 Gb/s, so a pair of nodes can reach about twice that using both.

### RDMA

Normally, sending data means the operating system copies it several times on the way out and on the way in. That takes time.

**RDMA** (remote direct memory access) skips those copies. One node's network card writes straight into the other node's memory. It's like handing a colleague a page directly, not posting it through the office mail. **RoCE** (RDMA over Converged Ethernet) is the way RDMA runs on an Ethernet network like the fabric.

**NCCL** is NVIDIA's library that a model uses to share work between GPUs. When RDMA can't be used, NCCL quietly falls back to TCP, the ordinary way of sending data, which costs most of the speed. That's why SparkBox checks which path each model really uses.

### Why lost packets matter

Data travels in small pieces called **packets**. RDMA is fast partly because it assumes nothing gets lost. When a packet is lost, RoCE has to go back and resend, which stalls the work. A fabric that loses nothing is called **lossless**.

Three things help keep it that way:

- **PFC** (priority flow control): when a buffer is nearly full, the receiver sends a pause frame that asks the sender to stop for a moment.
- **ECN** (explicit congestion notification): the switch marks packets when a queue is building, so senders slow down before anything is lost. A trickle of ECN under load is normal.
- **Jumbo frames:** the fabric sends packets of up to 9000 bytes, called an MTU of 9000. Every node and switch port must agree, or large packets fail.

RoCE still works without PFC, by resending what it loses. SparkBox calls this **PFC protection unverified** and shows it as a gray note, not a fault. Your cluster runs this way.

## Why it matters in SparkBox

You need the fabric when one model runs across two or more nodes. SparkBox also copies model files between nodes over it, so a copy needs it too. A model that runs on one node doesn't need the fabric to answer you.

The [Fabric](help:fabric) window gives each half a verdict. The words mean:

| Verdict | What it means |
|---|---|
| **Node-side PFC protected** | Nothing lost. This is what you want |
| **Congested** | Traffic is heavy and senders are being slowed, but nothing is lost |
| **PFC protection unverified** | RoCE resends what it loses. A known setup, not a fault |
| **Lost packets earlier** | Something was lost in the last hour, but not now |
| **RoCE is retransmitting** | Packets are being lost and resent now |
| **PHY errors** | Signal errors, which usually point at a dirty optic or a bad cable. PHY is the physical layer: the cable and light |
| **Pause storm** | Pause frames have run for a long time, which stalls the link |
| **Dropping packets** | A receive buffer is overflowing |
| **Link down** | The cable has no connection |

An **optic** is the small module at each end of a fiber cable. It turns data into light. Dirty or failing optics cause most fabric trouble, so SparkBox shows how much light each one receives.

PFC must cover the priority carrying traffic. SparkBox requires complete per-priority readings and a single active priority before calling the node side protected. Idle, missing or mixed traffic stays unknown. Global pause frames are separate from PFC, and node readings do not prove switch QoS.

The proposed configuration needs all three parts checked together: node trust **dscp** with PFC on priority 3; RoCE ToS 106 (`NCCL_IB_TC=106`); and switch DSCP 26 mapped to priority 3 with PFC. SparkBox explains this but applies no fabric settings.

## Related

- [Fabric](help:fabric)
- [Check the fabric](help:check-the-fabric)
- [Test the fabric](help:test-the-fabric)
- [Running one model on several nodes](help:splitting-a-model)
- [Your cluster at a glance](help:your-cluster)
