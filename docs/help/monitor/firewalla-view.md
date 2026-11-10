---
id: firewalla-view
title: "Switch window: Firewalla devices"
type: feature
area: network
summary: The Firewalla Switch X's ports, and reachability, latency, loss and uptime for the Switch X and Firewalla Gold.
keywords: [firewalla, gateway, latency, uptime, switch x, gold, loss, outage, ports, lldp, port speed, port panel]
verified: false
related: [see-whats-plugged-into-the-switch-x, connect-firewalla-cloud, add-switches-and-gateway, connect-the-vpn, switch-window]
---

# Switch window: Firewalla devices

## What it's for

This window watches your two Firewalla devices: the **Firewalla Switch X**, your 10 GbE LAN switch, and the **Firewalla Gold**, your gateway. Use it to see whether they answer, how quickly, and how each node's LAN link looks. For the Switch X, it also shows its ports and what's known about each one.

Firewalla doesn't publish a local way for other apps to read or change these devices. So SparkBox puts together what it can learn from outside, and says where each fact came from. To change a port, use the Firewalla app.

## Where to find it

- **Sidebar:** under **Network**, click **Firewalla Switch X** or **Firewalla Gold**.
- **Topology:** click the box.
- **Links:** [Open Firewalla Switch X](open:switch?id=firewalla), [Open Firewalla Gold](open:switch?id=gold)

If a device has no address yet, the window reads "Not set up". [Add your switches and gateway](help:add-switches-and-gateway) shows how to set it up. If your nodes hear the Switch X, its window opens anyway and shows its ports, with a note that it has no address.



## What's on the screen

### The toolbar

**Probe now** checks the device at once, and says how it went, such as "Firewalla Gold answered in 0.8 ms.". In look-only mode, you can still click it, but SparkBox refuses it and says why. **Open Firewalla web UI** opens `my.firewalla.com`, the web version of the Firewalla app, in your browser.

### The strip

The device's state and response time, its name, and its address.

### The port panel

The Switch X only. This is a drawing of the switch's front, from Firewalla's own product photo: ports **1** to **8** are RJ45 sockets, and ports **9** to **12** are SFP+ cages. The socket at the far left, the two lights and the two buttons are there to orient you. Firewalla doesn't say what that socket is for, and SparkBox can't read any of them.

A port with a link glows. Its color shows its speed, and the speed is written under it, so color is never the only cue. The name under that says what's connected.

| You see | What it means |
|---|---|
| **10G**, **5G**, **2.5G**, **1G** or **100M**, in its own color | The link's speed. It comes from Firewalla's cloud, or from one node's own link. For a node, you must have labeled this port with it, LLDP must place it on this port, and nothing else may be heard through the port |
| **?**, in gray | Something's connected, but the speed isn't known |
| Dark, with nothing written | No link, or marked empty |

SparkBox never colors a port from a guess. A port known only from LLDP stays gray, even when the nodes behind it report their own speed. Under the panel, a legend says what each color means.

Click a port, or press <kbd>Tab</kbd> to reach it and <kbd>Return</kbd> to open it, to see its details.

### Where the facts come from

The line under the legend names the three sources. Each one adds to the others.

| Source | What it gives | Setup |
|---|---|---|
| **LLDP** | The way network devices tell each other their names. The switch sends it, and your nodes report what they hear | None |
| **Your labels** | What you say is plugged into each port | None |
| **Firewalla cloud** | Link, speed, traffic, errors and power over Ethernet for each port, from Firewalla's **MSP** service, its web service for looking after its devices | [Connect Firewalla's cloud](help:connect-firewalla-cloud) |

SparkBox asks the cloud at most every 15 minutes, so a reading is usually no older than that. It can be much older after a failure, or while Firewalla has told SparkBox to wait. The line says how old it is. A reading more than an hour old stops lighting ports, and so does one from a poll that says it may not be this switch's, or that says the switch is offline. The port's details say so.

In look-only mode, SparkBox doesn't ask the cloud at all, so nothing that carries your token leaves this Mac. The line reads **Firewalla cloud: held in look-only mode**. **Test connection** is held too.

### A port's details

Clicking a port opens a panel on the right. Press <kbd>Esc</kbd> to close it. Every fact has a small line under it that says where it came from and how old it is.

| Part | What it shows |
|---|---|
| **Link**, **Speed** | Whether the port has a link, and how fast |
| **Behind it** | What the nodes on this port report about their own links. This is a remark, not the port's speed: a node's speed is its link to whatever is next to it |
| **LLDP port id** | The name the switch sent, such as `Gi0/2`. SparkBox reads that as port 2, but it isn't confirmed that this always matches the number on the front |
| **What's connected** | Your label. A label for something other than a node reads "SparkBox has no live data for this device." |
| **Heard through this port** | Each device your nodes hear on the LAN behind this port, its own port name, and which nodes heard it |
| **Traffic and errors** | The cloud's totals, and the labeled node's own traffic, errors and link flaps. **Open** with the node's name goes to its window |
| **Port details** | Whether it's the link up to the Gold, power over Ethernet, the SFP+ module and spanning tree, when the cloud reports them |
| **Your label** | Choose what's plugged in, then click **Save label** |

"Heard through this port" doesn't mean plugged into it. If several nodes hear the same switch port, there may be another switch between them and the Switch X, or the Switch X may send the same port name on every port. SparkBox can't tell which, so it lists them all and never guesses a port for each one.

A yellow note appears when sources disagree, such as "You marked this port empty, but LLDP hears the switch through it." or "LLDP places cell1 on port 2, not this one, so its link speed isn't used here. That doesn't prove the label is wrong."

### The Gold's drawing

The Firewalla Gold only. A gray drawing of the device's ports, labeled **Managed in the Firewalla app**. It's there to orient you. The ports aren't live, and you can't click them.

### The tiles

| Tile | What it shows |
|---|---|
| **Latency** | How long the last check took, from a connection to port 443, every 5 s |
| **Loss** | Share of checks with no answer, over the last 5 minutes |
| **Uptime** | Share of the last 24 hours it answered |
| **Last outage** | When it last stopped answering, how long for, and why. **None seen** means none since SparkBox started watching. Three missed checks in a row count as an outage |
| **VPN round trip** | The Gold only. How long the tunnel takes, or why it's not in use, such as "Not needed: on the home network" |

### Latency

A chart of the last 60 checks. Gaps are lost checks. One slow check doesn't flatten the rest: it sits on the top edge with ▲.

### LAN health, from each node's side

Each node's own LAN link, as that node reports it. It's the link from the node to whatever is next to it, which may not be a Switch X port.

| Column | What it shows |
|---|---|
| **Node** | The node |
| **Link** | **Up** or **Down**. **No data** means the node isn't reporting |
| **Interface** | The network connection's name on the node |
| **Address** | Its IP address |
| **Speed** | The link speed, such as **1 Gb/s** |
| **Duplex** | **Full** for links of 1 Gb/s or more |
| **↓ / ↑** | Traffic now |
| **Errors**, **Drops** | Totals |
| **Flaps** | How often the link dropped since the node started |

## What it changes

The switch and the Gold: nothing. This window only reads. Changes to a port are made in the Firewalla app. SparkBox sends each device a connection attempt, and needs no password or Admin mode for that.

**Save label** changes one file on this Mac. It's refused in look-only mode. It asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes. Each save is written to the activity log. To undo it, choose **Not labeled** and save.

## Related How-Tos

- [See what's plugged into the Switch X](help:see-whats-plugged-into-the-switch-x)
- [Connect Firewalla's cloud](help:connect-firewalla-cloud)
- [Add your switches and gateway](help:add-switches-and-gateway)
- [Reach your cluster with the VPN](help:connect-the-vpn)
- [Check that your nodes are ready](help:check-your-nodes)
