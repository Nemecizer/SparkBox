---
id: restart-a-service
title: Restart, disable or enable a service
type: howto
area: ops
summary: Control a background service on a node, and read its status and log.
keywords: [service, systemd, restart service, journal, failed unit, read logs, service log]
verified: false
related: [services, services-and-processes]
---

# Restart, disable or enable a service

To restart a service, select it in the **Services** window and click **Restart…**. To control whether it starts at boot, use **Enable** and **Disable…**.

## Before you start

- A **service** is a program the node keeps running in the background. [Services and processes](help:services-and-processes) explains the idea.
- Reading a service's status and log needs nothing, and it works in look-only mode.
- To change a service, use **Live** data with look-only mode off, and be ready to unlock Admin mode. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- With **Demo** data the services are simulated.
- Validate the results on your own equipment before relying on them.

> [!DANGER]
> **Stopping or disabling `ssh` or the network can cut you off from the node, and SparkBox can't undo that.** Only someone at the machine can fix it. SparkBox makes you type a name for these, and it says what it would break. Stop if nobody can get to the node.

## Steps

### Find the service and read its log

1. In the sidebar, under **Workloads**, click **Services**. [Open Services](open:services) The **Services** window opens with one row for each service and one column for each node. A checkmark means it's running.
2. To find a service, type part of its name in **Filter services**, such as `ollama`.
3. Click the node's cell in that service's row. A panel at the right, or below the table in a narrower window, shows its status, under **On cell2**.
4. Read **State**, **Since** and **Result**. If it has failed, **Error** shows the line that says why.
5. Read **Journal**, the service's own log. The panel scrolls to the line that matters.


### Restart a service

6. Under **Apply to**, check which nodes are selected. To add a node, click its name.
7. In the toolbar, click **Restart…**. In a narrower window the toolbar shows icons, and **Restart…** is the curved arrow. [Services](help:services#the-toolbar) lists every icon.
8. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
9. A dialog named **Restart ollama on cell2?** opens. It says what happens.
10. If you see "Type cell2 to confirm", type what it asks.
11. Click **Restart**.

### Stop it from starting at boot, or start it at boot again

12. In the toolbar, click **Disable…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
13. Confirm in the dialog. The service keeps running until it's stopped, but it won't start at the next boot.
14. To undo that, click **Enable**. Once Admin mode is unlocked, SparkBox asks nothing more, because this changes only what happens at the next boot.

## What you should see

For 10 seconds the status bar names the task, such as "Restart ollama.service on cell2". If another window is in front, a message says "Restarted", "Disabled at boot" or "Enabled at boot". [Tasks](open:tasks) keeps the result.

After a restart, the table still shows a checkmark. **Since** shows a time of a moment ago, and new lines in the journal are highlighted. After **Disable…** or **Enable**, each cell shows `enabled` or `disabled` when you look at one node, chosen in the **All nodes** list.

## Why it works

Services are run by **systemd**, the part of the node that starts programs and keeps them running. SparkBox asks systemd to restart, enable or disable that service as root, using the cluster password from the Keychain. Enable and disable change whether it starts at boot. They don't start or stop it now.

On several nodes, the action runs on one node at a time, and stops at the first failure.

## If something goes wrong

- **A button is dimmed.** Move the pointer over it. It gives the reason, such as "It's masked: unmask it first", "Already off at boot" or "Pick a node under Apply to".
- **"systemctl restart ollama.service failed on cell2: …"** systemd's own reason follows the colon. The journal usually says more.
- **The service fails again right after a restart.** Read the **Error** row and the journal. A restart doesn't fix a wrong setting or a missing file.
- **"Stopped at cell2: …"** It worked on the nodes before `cell2`, and it wasn't tried on the rest.
- **"Couldn't read services on cell3 (…)."** The node can't be read. Check that it's online.

## Next steps

- [Services](help:services), to see every part of the window
- [Respond to an alert](help:respond-to-an-alert)
