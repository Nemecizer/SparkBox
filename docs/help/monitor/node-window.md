---
id: node-window
title: A node's window
type: feature
area: monitor
summary: "Everything about one node: Overview, CPU, Memory, GPU, Disks, Network and System, with its actions."
keywords: [node, cpu, gpu, memory, disks, network, system, uptime, boots, gpu usage, gpu utilization, cpu usage, memory usage, how busy]
verified: false
related: [check-disk-space, find-out-why-a-node-restarted, restart-a-node, use-maintenance-mode, read-the-charts]
---

# A node's window

## What it's for

A node's window shows everything SparkBox knows about one of your configured computers. Open it when the **Dashboard** points at a node, or when you want to know what a node is doing and why.

Compare the readings with your own devices before relying on them.

## Where to find it

- **Sidebar:** under **Nodes**, click the node, such as `cell1`.
- **Dashboard:** double-click a node's row, or click a reading to go straight to its tab.
- **Topology:** click a node's box.
- **Command palette:** press <kbd>⌘K</kbd>, then type the node's name.
- **Link:** [Open `cell1`](open:node?id=cell1)

## What's on the screen


### The toolbar

On the left, the toolbar shows the node's state, hostname, model and uptime. If SparkBox didn't start a restart in the last day, this line says so, such as "restarted on its own 6:08 PM · no clean shutdown". After a clean shutdown it reads "restarted outside SparkBox 6:08 PM (clean shutdown)". **1 note** appears when SparkBox has something worth knowing that isn't a problem. Click it to read the note.

| Button | What it does |
|---|---|
| **Terminal** | Opens a shell on the node. Needs Admin mode. See [Open a terminal on a node](help:open-a-terminal) |
| **Files** | Shows the node's files beside this Mac's. See [Copy files between your Mac and a node](help:copy-files) |
| **DGX Dashboard** | Opens NVIDIA's dashboard in your browser, through a private tunnel. Needs **Live** data and Admin mode. Use it to look: an update from that page skips SparkBox's checks. See [Open the DGX Dashboard](help:open-the-dgx-dashboard) |
| **Maintenance** | Marks the node as in maintenance, so its alerts are **Planned**. Nothing runs on the node. Needs **Live** data and Admin mode. See [Use maintenance mode](help:use-maintenance-mode) |
| **Restart…** | Restarts the node. See [Restart a node](help:restart-a-node) |
| **Shut down…** | Turns the node off. See [Shut down and wake a node](help:shut-down-and-wake-a-node) |
| **Wake** | Shows when the node isn't online. It sends a Wake-on-LAN signal |

Narrow windows move some buttons under **More**.

Below the toolbar, a banner appears when something's wrong. It gives the node's offline reason and the action that would help, or its alerts with an **Alerts** button. It also notes any model it serves that isn't **Ready**.

### The range bar

The **Overview**, **CPU**, **Memory**, **GPU** and **Disks** tabs share a bar above their charts. Choose **5m**, **15m**, **1h**, **6h**, **24h** or **7d**. **Live** means the charts are following the newest readings. Drag across a chart to zoom every chart on the tab, and the bar reads **Paused** with **Back to live**. [Read the charts](help:read-the-charts) has more.

### Overview

Six tiles give the node's vital signs, with charts of the same things below.

| Tile | What it shows |
|---|---|
| **CPU** | How busy the processor is, its load and its temperature |
| **GPU** | How busy the GPU is, its power and temperature |
| **Unified memory** | Memory in use out of the total, and how much is on the GPU. A Spark has one memory pool that the CPU and GPU share |
| **Root disk** | How full the main disk is, and how much is free |
| **Fabric ↓ / ↑** | Data received and sent on the fast 200G network, IP and RDMA together |
| **Uptime** | How long since the node started. It reads **Restart needed for updates** when an update is waiting for a restart |

The charts are **CPU and GPU**, **Unified memory**, **GPU power**, **GPU temperature**, **Fabric** and **Root disk**. On **Fabric**, RDMA is the fast direct path and TCP is ordinary network traffic; [The fabric and RDMA](help:fabric-and-rdma) explains both. Solid lines are data received and dashed lines are data sent.

### CPU

**All cores** shows how busy the processor is overall. **Load average** shows how much work was waiting over the last 1, 5 and 15 minutes. **CPU temperature** is the chip's heat.

Below the chart, each core is a colored square with its share of the work. The darker or brighter the color, the busier the core, as the scale at the bottom shows. **Performance cores · Cortex-X925** are the fast cores. **Efficiency cores · Cortex-A725** are the economical ones.

### Memory

| Tile | What it shows |
|---|---|
| **Used** | Memory in use, and its share of the total. Amber at 90%, red at 95% |
| **Available** | Free memory plus cache the system can give back |
| **GPU allocations** | Memory that programs have reserved for the GPU |
| **Swap** | Disk space used as spare memory, or **None** if it isn't set up |

**How the pool is split** draws the memory as one bar: **GPU allocations**, **Other used**, **Page cache** and **Free**. The page cache holds recently read files, and the system gives it up when something else needs the room. **Free** is the empty part of the bar.

**GPU allocations** is part of unified memory use, not extra memory to add to **Used**. A model can keep its GPU reservation while no request is running. The cache percentage in **Endpoints** measures occupied cache space, so it can be zero at the same time. See [Comparing the memory readings](help:memory-on-a-spark#comparing-the-memory-readings).

The line below the bar reports pressure, such as "Pressure: memory 2% · IO 14% of the last 10 s waiting". **Pressure: none** means nothing had to wait for memory or the disk.

> [!DANGER]
> **A Spark that runs out of unified memory can freeze, and then only someone at the machine can restart it.** Watch this tab during a load. If the bar nears full, unload a model or stop a container. See [How memory works on a Spark](help:memory-on-a-spark). If the node stops answering, follow [Recover a frozen node](help:recover-a-frozen-node).

### GPU

The top line says whether the GPU is slowing itself down. **Not throttled** is normal. **Idle** means clocks are low because nothing is running. **Throttled:** followed by a reason means it's holding back, usually because of heat or power. A red **Xid error** badge means the GPU reported a fault; see the [GPU rules](help:alert-rules-reference#gpu).

The tiles show **Utilization**, **SM clock** (how fast the GPU's core runs), **Power** and **Temperature**. The charts show the same over time.

A clock below its maximum does not by itself mean throttling. SparkBox also checks the driver's reported reason. Power and heat warning rules remain unchanged.

**Processes on the GPU** lists each program using the GPU: its **PID** (the system's number for it), **Process**, **Container** and **GPU memory**. Nothing here means nothing is using the GPU.

A GB10 doesn't report its GPU memory the usual way, because the CPU and GPU share one pool. SparkBox adds up each process's share instead, so these numbers can differ from what NVIDIA's own tools show.

### Disks

One row for each disk: **Mount** (where it appears in the file system), **Device**, **Type**, **Size**, **Free**, **Used**, **Read** and **Write**. A row turns amber at 85% and red at 95%. **Root disk throughput** charts how fast the main disk is being read and written.

[Check disk space on a node](help:check-disk-space) shows what to do with these numbers.

### Network

**Fabric health** gives a verdict for each half of the node's fabric port, such as **Lossless**. It adds the half's traffic, its switch port, and any counters that moved in the last hour. The arrow opens that half in [Fabric](help:fabric).

**Port 1** and **Port 2** are the two ConnectX-7 ports, the node's fast network cards. Each cabled port has two interfaces, called halves, that share one 200G cable. The header says **200G cable** or **Not cabled**, and names the switch port. Click that name to open the port in the switch's window.

**LAN and Wi-Fi** lists the ordinary network connections. Under it, a line says what the node hears on each one through LLDP, a way for network devices to announce themselves. [See what a port connects to](help:see-what-a-port-connects-to) explains those lines.

| Column | What it shows |
|---|---|
| **Interface** | The network connection's name on the node |
| **Link** | The speed, or **No link** or **Down** |
| **Address** | Its IP address |
| **MTU** | The largest packet it sends. The fabric uses 9000. It turns amber if a fabric link differs |
| **Receive**, **Transmit** | Traffic now |
| **RDMA share** | The share of its traffic that used the fast direct path |
| **Last minute** | A small chart of traffic since you opened the tab |
| **Errors · drops** | Totals since the link came up. They turn amber while they're rising |
| **RDMA device** | The name of the fast path's device |

A note with a gray mark, such as "cell3 is on your LAN twice.", is worth knowing but isn't a fault. SparkBox doesn't change it.

### System

**Machine** lists the **Hostname**, **Address**, **Model**, **BIOS**, **Uptime** and whether a restart is needed.

**Software** lists the **Operating system**, **DGX OS build**, **DGX OS OTA** (the last over-the-air update), **Kernel**, **NVIDIA driver**, **CUDA**, **ConnectX-7 firmware** and **Docker**.

**Time and connection** lists:

- **Clock (NTP):** whether the node's clock is in step with the network's time. It warns when the clock is more than 100 ms off.
- **Login:** how SparkBox logged in.
- **Host key:** the key that proves this node is the same node. A note appears if another node shares it, which happens with cloned disk images.
- **SSH round trip:** how long SparkBox's last command took.
- **Last reading:** when SparkBox last heard from the node.

#### Watching

**Kernel events** counts problems from the node's kernel log in the last 24 hours, with **Show in Alerts** to see them. **Agent** says whether the [always-on agent](help:always-on-agent) is installed here. It reads "Not installed · SparkBox watches this node only while it's open" if it isn't.

#### Boots

**Boots** lists each run of the node and how it ended. Read [Find out why a node restarted](help:find-out-why-a-node-restarted). In Demo it reads "Available with Live data."

## What it changes

The tabs only read. They change nothing and need no Admin mode.

The toolbar's actions change things, and each one asks first:

| Action | What it changes | How to undo |
|---|---|---|
| **Restart…** | Restarts the node. Anything running on it stops | Models set to keep running come back by themselves |
| **Shut down…** | Turns the node off | Click **Wake**, or press the node's power button |
| **Maintenance** | Marks the node on this Mac only. Nothing changes on the node | Click **End maintenance** |
| **DGX Dashboard** | Opens a tunnel | It closes by itself, or click **Close** in Settings › Security & privacy |

## Related How-Tos

- [Check disk space on a node](help:check-disk-space)
- [Find out why a node restarted](help:find-out-why-a-node-restarted)
- [Open the DGX Dashboard](help:open-the-dgx-dashboard)
- [Restart a node](help:restart-a-node)
- [Use maintenance mode](help:use-maintenance-mode)
- [Read the charts](help:read-the-charts)

![A node's window, using synthetic Demo data](img/public-node-window.jpg)
