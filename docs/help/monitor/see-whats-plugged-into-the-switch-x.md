---
id: see-whats-plugged-into-the-switch-x
title: See what's plugged into the Switch X
type: howto
area: network
summary: Read the Switch X's port panel, and label each port with what's plugged into it.
keywords: [firewalla, switch x, port, plugged in, connected, lldp, label, port speed, what is connected]
verified: false
related: [firewalla-view, connect-firewalla-cloud, see-what-a-port-connects-to]
---

# See what's plugged into the Switch X

To see what's connected to the Firewalla Switch X, open its window, click a port and read what SparkBox knows. Then tell it what you know.

## Before you start

- Reading the panel needs no setup, and no Admin mode. It also works in look-only mode.
- The panel needs no address for the Switch X, as long as your nodes hear it. An address also shows whether the switch answers. [Add your switches and gateway](help:add-switches-and-gateway) shows how.
- To save a label, look-only mode must be off. Be ready to unlock Admin mode.
- With **Demo** data, you see sample ports and sample labels. A label you save in Demo stays in Demo.
- Have the switch in view, or know how it's cabled. Only you can see which cable goes where.

## Steps

1. In the sidebar, under **Network**, click **Firewalla Switch X**. [Open Firewalla Switch X](open:switch?id=firewalla) The window opens with a drawing of the switch's front.
2. Look for a lit port. Without Firewalla's cloud, a port heard over LLDP is gray, with **?** under it.
3. Click that port. A panel opens on the right, with **LLDP port id** near the top. Your cluster's LAN shows `Gi0/2`.
4. Read **Heard through this port**. It lists the nodes and other devices your nodes hear on that part of the LAN.
5. Click the port that you know a device is plugged into. On the right, under **Your label**, open **What's plugged in**.
6. Choose a node, **CRS804**, **Firewalla Gold** or **This Mac**. For anything else, choose **Something else…** and type its name, such as `Printer`. If nothing's plugged in, choose **Empty**.
7. Click **Save label**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. The name appears under the port.
8. Repeat for each port you know. To take a label off, choose **Not labeled**, then click **Save label**.


## What you should see

A port you labeled with something other than a node shows its name, a gray light, and the line "SparkBox has no live data for this device."

A port you labeled with a node shows that node's link state, speed, traffic, errors and link flaps, with a button to open its window. The port lights in the color of the node's speed only when the evidence agrees: LLDP places that node on this port, nothing else is heard through it, and no other port has its label. So a node alone on a port, on a 1 Gb/s link, shows **1G**.

Otherwise the port stays gray, and a yellow note says why, such as "4 other devices are heard through this port, so cell1's link speed may be to something else." or "LLDP places cell1 on port 2, not this one, so its link speed isn't used here. That doesn't prove the label is wrong." On your LAN, every node hears the same port today, so a port you label with a node stays gray for now. The label is still saved, and the node's own figures still show.

## Why it works

LLDP lets the switch tell your nodes its name and the name of a port, and your nodes tell SparkBox what they heard. But LLDP hears a whole stretch of the LAN, so it can't say what's on one cable. Your labels fill that gap, because you can see the cables. When a label names a node, SparkBox can show that node's own view of its link, and it uses that speed for the port only if LLDP agrees. Each fact shows its source, so you always know which ones are measured and which are your own.

## Limits

- All your nodes hear the same port name. They plug into a 2.5G switch, which connects to the Switch X, so one port carries all four. SparkBox can't see that switch. It lists them all under that port and never guesses another port for them.
- `Gi0/2` reads as port 2, but it isn't confirmed that this name matches the number on the front.
- SparkBox can't show real speeds on ports it has no data for. [Connect Firewalla's cloud](help:connect-firewalla-cloud) adds them, and it still doesn't list devices by port.
- Nothing here changes the switch. Changes to a port are made in the Firewalla app.

## If something goes wrong

- **No port is lit.** No node hears the switch. Check that the nodes are online. If they are, the switch may have stopped sending LLDP.
- **"LLDP names a port SparkBox can't place".** The switch sent a port name SparkBox can't turn into a number. The message gives the name.
- **The Save label button is dimmed, and its tooltip says Look-only.** Turn off look-only mode in **Settings › Security & privacy**.
- **"You marked this port empty, but LLDP hears the switch through it."** The label and LLDP disagree. Check the label.

[VPN, fabric and switch messages](help:errors-network) lists the rest.

## Next steps

- [Connect Firewalla's cloud](help:connect-firewalla-cloud), to add real link speeds
- [Switch window: Firewalla devices](help:firewalla-view)
- [See what a port connects to](help:see-what-a-port-connects-to)
