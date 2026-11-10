---
id: check-the-fabric
title: Check the fabric
type: howto
area: network
summary: Read the Fabric window's verdict, see each link's state, and accept a cabling change.
keywords: [fabric, roce, lossless, cabling, 200g, verdict, pfc]
verified: false
related: [fabric, fabric-and-rdma, test-the-fabric]
---

# Check the fabric

To see whether the fast network between your nodes is healthy, read the verdict at the top of the **Fabric** window.

## Before you start

- You don't need Admin mode to read the window. Accepting a cabling change needs Admin mode and Touch ID or your Mac password.
- With **Demo** data you see a simulated fabric. To see yours, use **Live** data. [Switch between Demo and Live](help:switch-between-demo-and-live) shows how.
- If the words are new to you, read [The fabric and RDMA](help:fabric-and-rdma) first. The **fabric** is the fast 200G network between nodes.

## Steps

1. In the sidebar, under **Network**, click **Fabric**. [Open Fabric](open:fabric) The **Fabric** window opens on its **Matrix** tab.
2. Read the band at the top. A quiet line such as "Fabric healthy · node-side PFC protected · 200G per node · NCCL on RoCE" means all is well. A gray note that starts "Fabric PFC needs verification." means protection is unproven, explained below. An amber or red band names the problem and says what to do.
3. Look at the matrix. It has one row for each node and one column for each half of the fabric port. Each cell gives a verdict, such as **Node-side PFC protected**, with the link speed and MTU.
4. Find any cell that isn't **Node-side PFC protected** or **PFC protection unverified**, or that shows a chip such as **Retransmits**. A chip means that counter moved in the last hour.
5. Click the cell. A panel opens on the right. **Suggested action** says what to do, and the table lists every counter.
6. Below the matrix, read **NCCL transport**, if it's there. It appears only for a model served across nodes, and says how much of its traffic used the fast path.
7. Read **Cabling**, if it's there. It appears only when a node's cable is on a different switch port than SparkBox learned.

### If you moved a cable on purpose

1. In the toolbar, click **Accept current cabling…**. If you don't see it, click **More** at the right of the toolbar.
2. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
3. A dialog named **Accept the current cabling?** lists the moves. Read them, then click **Accept cabling**. The warning and the **Cabling** section go away.

If you didn't mean to move the cable, move it back instead.


## What you should see

- A quiet or gray band at the top, not an amber or red one.
- The same verdict in every cell, with **200G**, **MTU 9000** and "No errors in 1 h".
- In the sidebar, **Fabric** shows no warning.

The verdict in each cell depends on one setting. **PFC** pauses senders before they lose packets. With it on, cells read **Node-side PFC protected**, as they do with **Demo** data. On your cluster PFC is off, so with **Live** data they read **PFC protection unverified**, the sidebar says **PFC protection unverified**, and the gray note says "Fabric PFC needs verification."

**PFC protection unverified** isn't a fault. RoCE resends what it loses, and the note says how much that has cost so far. SparkBox doesn't change this setting.

## Why it works

SparkBox reads each interface's RoCE and error counters, and turns them into rates. It gives each half a verdict from those rates, and remembers an hour of losses. So a problem that passed still shows, as **Lost packets earlier**. The first time SparkBox sees every cable on a switch port, it records them as the cabling to expect. It then warns you if one moves.

## If something goes wrong

- **"RoCE is retransmitting" or "PHY errors".** Packets are being lost. Follow **Suggested action**. Usually that means cleaning or reseating the optic or cable. [Switch window: CRS804 DDQ](help:switch-window#optics) shows its light levels.
- **"Link down".** The cable has no connection. Check both ends. If the node is offline, see [A node shows Offline](help:troubleshooting#a-node-shows-offline).
- **"Pause storm".** A node keeps asking the switch to stop. Look for a stuck process on that node.
- **"NCCL is using TCP sockets".** A model isn't using the fast path, so it runs far slower. The lines below it name the setting to look at.
- **"No fabric interfaces".** No node reports a fabric address yet. Wait for the nodes to report, or check that the cable is in.
- **A cell reads "Not cabled".** That half has no address, which is normal if it isn't in use.

## Next steps

- [Test the fabric](help:test-the-fabric), to measure real speed
- [See what a port connects to](help:see-what-a-port-connects-to)
- [Fabric](help:fabric), to see every part of the window
