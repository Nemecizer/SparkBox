---
id: topology
title: Topology
type: feature
area: monitor
summary: A live map of your nodes, switches and links, colored by health, GPU load, temperature or memory.
keywords: [topology, map, diagram, cabling, links]
verified: false
related: [check-the-fabric, see-what-a-port-connects-to, connect-the-vpn, your-cluster, fabric, switch-window]
---

# Topology

## What it's for

**Topology** is a live map of your cluster. It draws the internet, your gateway, both switches, the four nodes and this Mac, with the cables between them. Use it to see how everything connects, and which link is carrying traffic or having trouble.

## Where to find it

- **Sidebar:** under **Overview**, click **Topology**.
- **Dashboard:** click the **Fabric ↓ / ↑** tile.
- **Command palette:** press <kbd>⌘K</kbd>, then type `Topology`.
- **Link:** [Open Topology](open:topology)

## What's on the screen


### The diagram

From top to bottom, the diagram shows **Internet**, the gateway (Firewalla Gold), the LAN switch (Firewalla Switch X), your nodes, and the fabric switch (CRS804 DDQ). **This Mac** sits beside the LAN switch, or beside **Internet** when you're connected through the VPN.

Each box shows its state:

- **A node** shows its GPU load and temperature, its memory, and the model it's serving, or **Idle**. A node that isn't reporting shows why, such as **Offline · 2 min**. A problem tints the top of the box amber or red.
- **A switch or the gateway** shows what it does and how quickly it answered, or **Unreachable**, or **Not set up**.
- **This Mac** shows how it reaches the cluster: **On this network** when it isn't going through the VPN, **VPN · 18 ms** when it is, or an error if the VPN failed.

### The lines

| Line | What it means |
|---|---|
| **Fabric** | The fast 200G network. Each cable is two lines, one for data sent and one for data received. A line gets thicker as its share of the 200G link grows. Moving dashes mean traffic is flowing |
| **LAN** | Your ordinary home network, drawn thin and dashed. The chip on it shows traffic now |
| **Management** | The fabric switch's own link to your LAN, labeled **ether1 · management** |
| **VPN tunnel** | Appears when this Mac reaches the cluster through the VPN |

The chip on a fabric line shows traffic and its share of the link. **Link down** means the cable isn't working. **No reading** means nobody can say, because the node isn't reporting and its switch port isn't known. **(switch)** means the number comes from the switch, because the node isn't reporting.

Fabric lines stay gray while the link is lossless, congested or has PFC off, which is a known setup that [Fabric](help:fabric) explains. They turn amber when the link drops packets or a cable isn't on its usual port. They turn red when the link is losing packets, has physical errors or is down.

Move the pointer over a box or a line for details. A fabric line shows its traffic each way, its interface names and its switch port. It also shows errors since the node started, and the weakest lane's light level.

Click a node or a switch to open its window. Click a fabric line to open its port, or **This Mac** to open **VPN**. Right-click a node for its menu.

### Color nodes by

This choice in the toolbar tints each node's box:

| Choice | What the tint shows |
|---|---|
| **Health** | No tint. Problems show as an amber or red band anyway |
| **GPU load** | How busy the GPU is |
| **GPU temp** | How hot the GPU is |
| **Memory** | How full unified memory is |

With a tint on, the reading appears in the box's corner, such as **GPU 62%**.

### Fit

**Fit** tightens the spacing so the diagram fits the window. The boxes and text never shrink.

### The list view

If the diagram can't fit at full size, or the window is smaller than 640 × 420 points, SparkBox shows the same facts as a list. The toolbar says "List view: the diagram doesn't fit this window". The list has two groups: the LAN, then the fabric, with one row for each device or link.

## What it changes

**Topology** changes nothing. It only reads, and needs no Admin mode. A node's right-click menu offers actions that ask first, such as **Restart…**.

## Related How-Tos

- [Check the fabric](help:check-the-fabric)
- [See what a port connects to](help:see-what-a-port-connects-to)
- [Reach your cluster with the VPN](help:connect-the-vpn)
- [Read the Dashboard](help:read-the-dashboard)

![Topology, using synthetic Demo data](img/public-topology.jpg)
