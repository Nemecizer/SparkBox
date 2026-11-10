---
id: switch-window
title: "Switch window: CRS804 DDQ"
type: feature
area: network
summary: The CRS804's ports, optics, neighbors and live traffic, with actions for each port.
keywords: [crs804, ports, optics, neighbors, routeros, drawing, lanes, dbm]
verified: false
related: [manage-a-switch-port, see-what-a-port-connects-to, add-switches-and-gateway, check-the-fabric]
---

# Switch window: CRS804 DDQ

## What it's for

This window shows the CRS804 DDQ, the MikroTik switch that carries your 200G fabric. Use it to see which node is on which port, how busy each port is and how healthy each optic is. You can also act on one port. An **optic** is the small module at the end of a fiber cable.

On your cluster, each node has reported which switch port it's plugged into. This window's own readings have run only on test systems.

## Where to find it

- **Sidebar:** under **Network**, click **CRS804 DDQ**.
- **Topology:** click the switch, or a fabric line to open its port.
- **A node's window:** on the **Network** tab, click the port's name.
- **Command palette:** press <kbd>⌘K</kbd>, then type a port, such as `port 2-1`.
- **Link:** [Open CRS804 DDQ](open:switch?id=crs804)

If the switch has no address yet, the window reads "Not set up". [Add your switches and gateway](help:add-switches-and-gateway) shows how to set it up.

## What's on the screen


### The toolbar

The toolbar acts on the selected port.

| Button | What it does |
|---|---|
| **Enable** | Turns a disabled port back on |
| **Disable…** | Turns the port off. The switch turns it back on after 60 seconds, unless you keep it off |
| **Bounce…** | Takes the port down for 3 seconds, then back up |
| **Reset counters** | Starts the port's counters again from zero |
| **Identify** | Blinks the port's light for 10 seconds |
| **Comment…** | Saves a short note on the port |
| **Test connection** | Checks that SparkBox can reach the switch. If the switch shows a certificate SparkBox doesn't trust, or a changed one, the result offers **Trust…** |
| **Details** | Shows or hides the port's panel |

All but **Test connection** and **Details** need Admin mode. So does **Trust…**: with Admin mode locked, the result reads "Unlock Admin mode to trust it." In look-only mode, you can still click **Test connection**, but SparkBox refuses it and says why.

### The strip

Below the toolbar: the switch's state and response time, its model, RouterOS version, uptime and address. RouterOS is the switch's operating system. A warning or error appears below it, and a countdown banner appears while a port is waiting to come back.

### The drawing

The drawing shows **four physical QSFP56-DD sockets** and two RJ45 management sockets. Your four nodes occupy all four fabric sockets, with 200 Gb/s links. Each fabric socket is rated for up to 400 Gb/s; the speed below it is the observed link speed. [MikroTik's specification](https://mikrotik.com/product/crs804_ddq) gives the rated limits.

RouterOS can report breakout interfaces such as `qsfp56-dd-1-1` and `qsfp56-dd-1-2`. These share physical socket 1; they do not add another socket to the drawing. All reported interfaces remain in the Ports table. The drawing follows the observed MAC or LLDP mapping, so it does not assume that `cell2` is on socket 2. Click a socket to select an active interface; use the table to select another breakout interface.

A board that SparkBox cannot identify gets no invented CRS804 layout. [Equipment profiles](help:settings-switches#equipment-profiles) can look up another model and add a labeled physical-port diagram and its manufacturer source. The generic diagram does not claim a cable mapping.

| What you see | What it means |
|---|---|
| A filled port | **Up** |
| An open, dark slot | **No link** |
| Hatching | **Disabled** |
| A red edge and **!** | **Errors rising** |
| A thin bar | Traffic, from 1% of the link |
| The light above a port | Green when fine. Amber or red for changed cabling, weak light or errors |

The small lights on the left show **PWR**, **PSU1**, **PSU2** and **FAN**. Move the pointer over a port for its details. Click one to select it.

### Overview

Tiles for the switch itself: **CPU**, **Temperature**, **Power supply 1**, **Power supply 2**, each fan in RPM, **Uptime**, **RouterOS**, **Board** and **Ports up**. They turn amber or red when something's wrong. CPU warns above 80% and temperature above 75 °C.

### Ports

One row for each reported interface, including breakout interfaces. Click a row to select it, or double-click to open its panel.

| Column | What it shows |
|---|---|
| **Port** | The port's name, such as `qsfp56-dd-1-2` |
| **Node** | The node cabled to it, or the port's comment. **SparkBox's path** with a padlock means your connection to the switch runs through it. **expected cell2** in amber means the cable moved |
| **State** | **Up** with its speed, **No link**, **Disabled** or **Errors rising** |
| **↓ Receive**, **↑ Transmit** | Traffic now |
| **Load** | Share of the link in use. The bar turns amber at 70% and red at 90% |
| **Last 60 s** | A small chart of traffic |
| **Errors** | Total errors. Red while rising |
| **Flaps** | Link-downs since the counters were reset |
| **MTU** | The largest packet size the port allows |
| **Worst RX** | The light level of the weakest lane, marked ▼ when low |
| **Temp** | The optic's temperature |

### Optics

One row for each fiber port, with the light received on each lane. A fiber port carries data on several **lanes**, and one weak lane can cause errors.

Light is measured in dBm. For these SR optics, amber is below −7 dBm (marginal) and red is below −10 dBm (failing). If a lane is low, clean or reseat the fiber. Ports without a link show no readings.

The tab also shows each optic's **TX** power, **Temp** and **Module** name. A **!** on the tab means a lane is low.

### Neighbors

**Bridge hosts** lists the network cards the switch learned behind each port. **Belongs to** names the node, and **Learned** says **Dynamic**, **Static** or **Switch's own**. **Neighbors** lists devices that announced themselves, with their **Identity**, **Address**, **Platform** and **Seen by**.

A port's node comes from the MAC first, then LLDP, then its comment. Double-click a row to open the port. [See what a port connects to](help:see-what-a-port-connects-to) explains these tables.

### Port details

The panel on the right shows one port.

- **↓ Receive** and **↑ Transmit:** the last 5 minutes.
- **Found by:** how SparkBox matched the node.
- **Last hour:** from saved history.
- **Switch says / node says:** the same cable from both ends. What the switch receives is what the node sent, so those rows should nearly match.
- **Counters:** errors, drops, pause frames, link-downs and when the link last came up.
- **Optics:** a bar for each lane, marked at −7 and −10 dBm, with the transmit power, module temperature, vendor and part number.

## What it changes

Everything shown is read-only until you use the toolbar. The actions change the real switch, except with **Demo** data, where they act on a simulator.

| Action | What it changes | Needs | How to undo |
|---|---|---|---|
| **Disable…** | Turns the port off. A node on it loses its fabric link | Admin mode | Comes back by itself after 60 seconds, or click **Enable**. After **Keep disabled**, click **Enable** |
| **Bounce…** | Port down for 3 seconds | Admin mode | Comes back by itself |
| **Reset counters** | Clears the port's totals | Admin mode | Can't be undone |
| **Identify** | Blinks a light | Admin mode | Stops by itself |
| **Comment…** | Saves a note on the switch | Admin mode | Edit it again |
| **Trust…** | Saves the switch's certificate on this Mac | Admin mode, Touch ID or your Mac password | No button removes it. If the certificate changes, **Test connection** shows the new one, and **Trust…** replaces the old one |

Each change goes in the [Activity log](help:activity-log).

## Related How-Tos

- [Manage a switch port](help:manage-a-switch-port)
- [See what a port connects to](help:see-what-a-port-connects-to)
- [Add your switches and gateway](help:add-switches-and-gateway)
- [Check the fabric](help:check-the-fabric)

![Switch window: CRS804 DDQ, using synthetic Demo data](img/public-switch-window.jpg)
