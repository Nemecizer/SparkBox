---
id: your-cluster
title: Your cluster at a glance
type: concept
area: start
summary: "What each part of your cluster does: four nodes, a fast fabric, a home network, a gateway and this Mac."
keywords: [hardware, nodes, network, what do I have, cluster, what is my cluster]
verified: null
related: [fabric-and-rdma, what-is-sparkbox, memory-on-a-spark]
---

# Your cluster at a glance

## In short

Your **cluster** is four computers, called **nodes**, joined by two networks. A very fast network, **the fabric**, carries data between the nodes. Your ordinary home network, **the LAN**, connects them to this Mac. A **gateway** connects it all to the internet.

## How it works


### The four nodes

A node is one computer in the cluster. You have four, and each one is a **Spark**: a small NVIDIA system built around the GB10 chip.

| Node | Hardware | LAN address |
|---|---|---|
| `cell1` | MSI EdgeXpert | `192.168.50.51` |
| `cell2` | MSI EdgeXpert | `192.168.50.154` |
| `cell3` | MSI EdgeXpert | `192.168.50.98` |
| `gx10-0000` | ASUS Ascent GX10 | `192.168.50.181` |

The four are the same inside, whichever company built them. Each runs DGX OS, NVIDIA's version of Ubuntu 24.04 Linux. All four share one user account, `sparkuser`, and one password.

NVIDIA sells each node with 128 GB of **unified memory**, one pool that the processor and the GPU share. The node itself reports 121.7 GiB, and that's the figure you see in SparkBox. A model has to fit in this memory to run. [How memory works on a Spark](help:memory-on-a-spark) explains why that matters.

### The fabric

The fabric is the fast network between your nodes. Each node has an NVIDIA ConnectX-7 network card, cabled at 200G to the `CRS804 DDQ`, a MikroTik switch. A **switch** is a box that passes network traffic between the cables plugged into it.

When a model is too big for one node, its parts talk to each other over the fabric. SparkBox also uses it to copy model files between nodes.

Each node's cable appears as two network connections, so each node has two fabric addresses: one on `192.168.100.x` and one on `192.168.101.x`. SparkBox calls these the two halves of the fabric.

### The LAN

The LAN is your ordinary home network. Each node plugs into the `Firewalla Switch X`. This Mac reaches the nodes over the LAN, at the `192.168.50.x` addresses in the table above. Every reading and every command in SparkBox travels this way.

### The gateway

The `Firewalla Gold` is your gateway, the box that joins your home to the internet. It's also where SparkBox's VPN connects when you're away from home. A **VPN** is a private tunnel across the internet.

SparkBox can read the CRS804 DDQ's ports in detail. Firewalla devices are managed in the Firewalla app, so SparkBox only checks that they answer.

### This Mac

This Mac runs SparkBox and nothing else in the cluster. Models run on the nodes, not here.

### An example

Say you chat with a model that runs on `cell1`, `cell2` and `cell3`. Your message leaves this Mac over the LAN and reaches `cell1`, the first node of that model. The three nodes then work on the answer together, passing data over the fabric. The answer comes back to this Mac over the LAN. You see only the text, but both networks did their part.

## Why it matters in SparkBox

The sidebar follows this layout. The **Nodes** group lists your configured nodes, and the **Network** group lists the VPN and your switches. To see everything on one page, open [Topology](open:topology).

Knowing which network does what helps when something goes wrong:

- If the LAN has a problem, SparkBox loses sight of a node, and the node shows **Offline**.
- If the fabric has a problem, the node stays online, but a model split across nodes can fail. [Check the fabric](help:check-the-fabric) shows how to look.

## Related

- [What SparkBox is](help:what-is-sparkbox)
- [The fabric and RDMA](help:fabric-and-rdma)
- [How memory works on a Spark](help:memory-on-a-spark)
- [Topology](help:topology)
