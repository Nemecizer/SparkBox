---
id: find-out-why-a-node-restarted
title: Find out why a node restarted
type: howto
area: monitor
summary: Use a node's boot history to see when it restarted and how the last run ended.
keywords: [boot history, crash, restarted, panic, why did it reboot, journal, power loss]
verified: false
related: [node-window, alerts-window, respond-to-an-alert]
---

# Find out why a node restarted

To learn when a node restarted and how its last run ended, read the **Boots** table on its **System** tab.

## Before you start

- You need **Live** data. Demo has no boot history, and the section reads "Available with Live data." [Switch between Demo and Live](help:switch-between-demo-and-live) shows how to change.
- You don't need Admin mode. SparkBox only reads, so this also works in look-only mode.
- The node must be online for SparkBox to read its record. A node that's still offline can't tell you yet.
- A **boot** is one run of a node, from the moment it starts until it stops.

## Steps

1. In the sidebar, under **Nodes**, click the node, such as `cell3`. The node's window opens. If SparkBox didn't start the restart, and it was in the last day, the top line says so: "up 12 min · restarted on its own 6:08 PM · no clean shutdown". After a clean shutdown it reads "restarted outside SparkBox 6:08 PM (clean shutdown)".
2. Click the **System** tab. It lists facts about the node.
3. Scroll to **Boots**, at the bottom. The newest boot is at the top, and the one reading **Running now** is the current one.
4. Read **How it ended** for the row below the current one. That's the run that ended in the restart. The next section explains the words.
5. Compare **Ended** with the time you expected. Note the time.
6. If a row has a matching line under the table, such as "Last lines of the boot that ended Oct 29 at 6:08 PM", click it. The last lines the node wrote before it stopped appear. The lines nearest the end matter most.
7. To see kernel problems from around that time, look at **Watching**, above **Boots**. If **Kernel events** shows a count, click **Show in Alerts**. The **Alerts** window opens on **Events** for this node.

## What you should see

The **How it ended** column uses these words:

| Words | What they mean |
|---|---|
| **Running now** | This is the current boot |
| **Clean shutdown** | The node shut down normally. Someone or something asked it to, such as you or an update |
| **Clean shutdown (SparkBox restarted it)** | The same, and SparkBox started the restart |
| **Ended without a shutdown (power loss, hang or watchdog)** | The node stopped with no shutdown. It lost power, froze, or a watchdog restarted it. A watchdog is a safety timer that restarts a frozen computer |
| **Not known: the journal no longer has it** | The node has since discarded its record of that boot |
| **Not read: more than 14 days ago** | SparkBox reads only the last 14 days |
| **Not read from the journal yet** | SparkBox hasn't read this boot yet. Wait a moment |

A row with no clean shutdown is shaded, so it stands out.

The **Seen** column says how SparkBox knows about the boot. **Live** means it saw the change happen. **Live · SparkBox restarted it** means the same, and SparkBox started the restart. **Found in the journal** means it found the boot in the node's record. **Running when SparkBox connected** means the boot had already started.

## Why it works

A GB10 has no separate management chip that records what happens to it, so the node's own journal is the only record of a restart. The **journal** is the log the operating system keeps. SparkBox reads it as your normal user, so it needs no Admin mode, when it connects and whenever a node's boot changes. A boot counts as clean when the journal shows the system reached its restart or shutdown stage near the end. Validate the results on your own equipment before relying on them.

## Limits

- The journal says how a run ended, not always why. **Ended without a shutdown** can mean a power cut, a freeze or a watchdog, and SparkBox can't tell which.
- SparkBox reads at most 10 earlier boots from the last 14 days, and keeps 30 boots for each node.

## If something goes wrong

- **"The journal couldn't be read:" followed by a reason.** SparkBox tries again by itself after 5 minutes. To try now, click **Read the journal again**. The node must be online.
- **"This node's journal keeps only the current boot."** The node doesn't keep its journal across restarts, so earlier boots can't be read.
- **"No boots known yet."** SparkBox hasn't read this node's record yet. Wait a moment, or click **Read the journal again**.
- **An alert says "Restarted on its own".** This warning lasts an hour after the boot. It turns critical if the same node restarts again within an hour, or if two nodes restart within 5 minutes of each other. Two nodes at once often points at power or the network.

## Next steps

- [Respond to an alert](help:respond-to-an-alert)
- [Check the fabric](help:check-the-fabric), to check the node's links after the restart
- [Capture evidence](help:capture-evidence), to keep a record of what you found
- [A node's window](help:node-window)
