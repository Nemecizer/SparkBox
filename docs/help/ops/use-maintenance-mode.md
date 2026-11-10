---
id: use-maintenance-mode
title: Use maintenance mode
type: howto
area: ops
summary: Tell SparkBox you're working on a node, so that its alerts don't page anyone.
keywords: [maintenance, planned work, quiet, stop alerts]
verified: false
related: [node-window, alerts-and-incidents]
---

# Use maintenance mode

To stop a node's alerts from paging anyone while you work on it, click **Maintenance** in the node's window. To **page** means to send an alert to your phone.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data the button says "Available with Live data". [Unlock Admin mode](help:unlock-admin-mode) shows how.
- **Until I end it** is a change on this Mac too. It also asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes.
- Nothing on the node changes. Maintenance mode only tells SparkBox how to treat the node's alerts.
- If you've installed the [always-on agent](help:always-on-agent) on your nodes, they aren't told about maintenance mode, and they may still page. The dialog says so.
- When you restart or shut down a node from SparkBox, you don't need this. SparkBox plans that work by itself.

> [!WARNING]
> **Until I end it has no end.** The node's alerts stay quiet until you click **End maintenance**, even after SparkBox restarts. If you forget, a real fault on that node pages nobody. Use **For 1 hour** unless you need longer. To undo, click **End maintenance**.

## Steps

### Start maintenance mode

1. In the sidebar, under **Nodes**, click the node, such as `cell2`. The node's window opens.
2. In the toolbar, click **Maintenance**, the wrench. If the window is narrow, it's under the **More** button (**…**) at the end of the toolbar.
3. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. A dialog named **Put cell2 in maintenance?** opens. Click **For 1 hour** or **Until I end it**.
5. If SparkBox asks, confirm with Touch ID or your Mac password. This can happen for **Until I end it**, which also changes this Mac.
6. Look at the top of the node's window. A notice reads like "cell2 in maintenance for 1 h · until 11:42 · its alerts are planned and page nobody".

### End maintenance mode

1. In the node's window, click **End maintenance**, in the toolbar where **Maintenance** was.
2. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
3. A message says "Maintenance ended on cell2", and "Its alerts count again."

**For 1 hour** ends by itself when the time is up.

You can also start and end it from the [command palette](help:command-palette). Press <kbd>⌘K</kbd> and type `maintenance`.

## What you should see

The status bar shows **In maintenance** beside the node, and the notice stays at the top of its window. In [Alerts](open:alerts), its alerts show as **Planned**, such as "cell2 in maintenance (you, 10:42)". They aren't counted, and nobody gets paged.

## Why it works

SparkBox keeps running its alert rules on the node, and it keeps showing what they find. It only marks those alerts **Planned**. The maintenance period is saved on this Mac, so it's still there if SparkBox quits and opens again. It ends when you end it, or at its time, whatever the node does. When it ends, alerts that are still true are raised again, so you see what's still wrong.

## Limits

Maintenance mode doesn't end when the node comes back from a restart. It stays on until you end it or its time runs out.

## If something goes wrong

- **The button is dimmed and says "SparkBox's own restart window is open: its alerts are planned already".** SparkBox is restarting or shutting down that node, and it's already quiet. Wait for that to finish.
- **"SparkBox is in the middle of its own planned work there (restarting); its alerts are already Planned until it's back."** The same, from the [command palette](help:command-palette).
- **"It isn't in maintenance."** There's nothing to end.
- **"That's SparkBox's own window for a restart; it ends by itself once the node is back."** You can't end that one. It closes when the node is back.
- **Your phone still gets a page.** The always-on agent on the node sends its own alerts. See [Set up always-on monitoring](help:set-up-always-on-monitoring).

## Next steps

- [Respond to an alert](help:respond-to-an-alert)
- [Alerts and incidents](help:alerts-and-incidents)
- [Restart a node](help:restart-a-node)
