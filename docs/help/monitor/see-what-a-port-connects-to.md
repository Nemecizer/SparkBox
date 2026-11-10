---
id: see-what-a-port-connects-to
title: See what a port connects to
type: howto
area: network
summary: Use LLDP to see which switch port each node is plugged into.
keywords: [lldp, neighbors, cable, which port, plugged in, mac address]
verified: false
related: [switch-window, node-window, check-the-fabric]
---

# See what a port connects to

To find out which switch port a node is plugged into, read the node's **Network** tab, or the **Neighbors** tab of the switch.

## Before you start

- You don't need Admin mode. Everything here only reads, so it also works in look-only mode.
- To see your own cables, use **Live** data. LLDP works without any setup. The switch's own tables need [your switches added](help:add-switches-and-gateway) first.
- Validate the results on your own equipment before relying on them.
- **LLDP** is a way for network devices to announce themselves to whatever they're plugged into. DGX OS sends it, so each Spark can name itself to the switch.

## Steps

### From a node

1. In the sidebar, under **Nodes**, click the node, such as `cell1`. The node's window opens.
2. Click the **Network** tab.
3. Find **Port 2**. It's the node's fabric port, its link to the fast 200G network. The right end of its heading names the switch port, such as "Switch port qsfp56-dd-2-1 (LLDP)".
4. If you've set up the switch in SparkBox, that name is a link, such as **qsfp56-dd-2-1 on CRS804 DDQ**. Click it. The switch's window opens with the port selected.
5. Scroll to **LAN and Wi-Fi**. The line under the table says what the node hears on your home network, such as "enP7s7 · Neighbors: cell1, cell2 and gx10-0000 (via switch)".

### From the switch

1. In the sidebar, under **Network**, click **CRS804 DDQ**. [Open CRS804 DDQ](open:switch?id=crs804) The switch's window opens.
2. Look at the drawing of the switch. The name under each port is the node cabled to it.
3. Click the **Neighbors** tab. **Bridge hosts** lists the addresses the switch learned behind each port. **Belongs to** names the node, or says "Not a node".
4. Look at **Neighbors**, below it. **Identity** is the name each device announced, and **Seen by** says how.
5. To see a port in full, double-click its row. The **Ports** tab opens on it.

## What you should see

Each node's fabric cable appears on one switch port. The node column of the **Ports** tab, the drawing and the node's **Network** tab all agree.

A node's LLDP line reads in one of these ways:

| Line | What it means |
|---|---|
| **Switch port qsfp56-dd-2-1 (LLDP)** | The switch announced itself, and named the port |
| **No LLDP from the switch (cell2 and cell3 answer through it)** | The switch doesn't announce itself on this port. The nodes you hear are on the far side of it |
| **No LLDP from the switch (only this node's other half answers)** | The node hears only its own announcements come back |
| **Neighbors: cell2 and cell3 (via switch)** | On the LAN, other nodes announce through the switch between you |
| **Switch port unknown (Firewalla Switch X Gi0/2 is heard by other nodes too)** | Every node hears that port, so it can't be the far end of this cable. [See what's plugged into the Switch X](help:see-whats-plugged-into-the-switch-x) shows what to do about it |

SparkBox reads these tables from the switch every 15 seconds, and the tab refreshes every 10.

## Why it works

SparkBox doesn't trust labels. It looks first in the switch's table of the network cards behind each port. A **MAC address** is a network card's fixed ID, so a match proves which node it is. Next come LLDP announcements, then the comment on the port. Validate the results on your own equipment before relying on them.

## If something goes wrong

- **"Switch port unknown" in the Fabric window.** SparkBox hasn't matched this half to a port, and LLDP has heard nothing on it. Check that the switch is set up in [Settings](open:settings?section=switches), and that it answers. If LLDP heard something, the cell shows that line instead, the same one the node's **Network** tab shows.
- **"No hosts learned" or "No neighbors announced".** The switch has no entries yet. The cable may be out, or no traffic has crossed it.
- **"Expected 2-1" in amber.** The cable isn't where SparkBox learned it. See [Check the fabric](help:check-the-fabric).
- **The Neighbors tab shows an error.** SparkBox can't read the switch. See [VPN, fabric and switch messages](help:errors-network).

## Next steps

- [Manage a switch port](help:manage-a-switch-port)
- [Check the fabric](help:check-the-fabric)
- [Switch window: CRS804 DDQ](help:switch-window)
