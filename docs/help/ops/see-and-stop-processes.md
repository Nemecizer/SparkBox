---
id: see-and-stop-processes
title: See and stop processes
type: howto
area: ops
summary: Find what's using a node's CPU, GPU or memory, and stop it if you need to.
keywords: [process, kill, stop, "what's using memory", gpu memory, top]
verified: false
related: [workloads, services-and-processes]
---

# See and stop processes

To find what's using a node's CPU, GPU or memory, open **Workloads** and sort its list. To stop a process, select it and click **Stop**.

## Before you start

- A **process** is one running program. [Services and processes](help:services-and-processes) explains how they fit together.
- Looking needs nothing: no Admin mode, and it works in look-only mode.
- To stop a process, use **Live** data with look-only mode off, and be ready to unlock Admin mode. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- With **Demo** data the processes are simulated.

## Steps

### Find a process

1. In the sidebar, under **Workloads**, click **Workloads**. [Open Workloads](open:workloads) The **Workloads** window opens on **Processes**, sorted by **CPU**.
2. To look at one node, choose it in the **All nodes** list in the toolbar.
3. Click the column heading **CPU**, **Memory** or **GPU memory** to sort by it. Click it again to reverse the order.
4. To see only programs that use the GPU, choose **Using the GPU** in the list that starts **Busiest + GPU + listening**.
5. To find a process by name, type part of it in **Filter**.
6. Click the process's row. A bar above the list shows its name, number and node. The list holds still while a row is selected, so it can't move under your pointer.


### Stop a process

7. Look at **Runs in** for the process. If it names a container, stop the container in [Containers](open:containers) instead. If it names a service, stop the service in [Services](open:services): the system may start that process again.

> [!DANGER]
> **A stopped process loses whatever it hadn't saved, and that can't be undone.** Stopping a model's server also takes that model offline until you start it again. If you don't know what a process does, stop here.

8. In the toolbar, click **Stop**. In a narrower window the toolbar shows icons, and **Stop** is the square in a circle.
9. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
10. A dialog named **Send TERM to python3 (2211) on cell2?** opens. TERM is the polite request to shut down cleanly. **PID** is the number a node gives each process, here `2211`.
11. Click **Stop** in the dialog.
12. Wait up to 10 seconds. SparkBox watches the process and tells you what happened.

If the process doesn't stop, the message offers **Force stop**. That sends KILL, which ends the process at once, with no chance to clean up. Use it only after **Stop** has failed.

## What you should see

A message such as "python3 (2211) exited after 1.4 s". The row stays in the list for a minute, dimmed, with "exited · last seen 12 s ago", then goes.

## Why it works

Every 5 seconds, while the window is open, SparkBox asks each node for its process list. Stopping sends a **signal**, a short message the system delivers to a program. A program can act on TERM and tidy up, but KILL can't be caught, so the program ends at once. Before it sends a signal, SparkBox compares the process's start time, because a node can reuse a number after a process exits. If the process belongs to root, SparkBox sends the signal with sudo, using the cluster password from the Keychain.

## Limits

- The list shows up to the 200 busiest processes on each node, by CPU and then memory. Model servers, GPU users, programs on a port and system services are always included.
- To stop a process for good, stop what started it: its container or service.

## If something goes wrong

- **"python3 (2211) is still running"** It didn't exit within 10 seconds. Click **Force stop** in the message, or check whether a service keeps starting it.
- **"Process already exited (its PID is gone or now belongs to another program)."** It stopped by itself, or a new program has its number. Nothing was sent to the new one.
- **"SparkBox won't signal process 1."** Process 1 is the system itself. Stopping it would bring the node down.
- **The row says "unknown · cell3 offline".** The node isn't reporting, so this is its last-known list. Stop is off until the node is back.
- **"sudo on cell2 didn't accept the saved cluster password."** See [Connection and login messages](help:errors-connection#sudo-on-cell2-didnt-accept-the-saved-cluster-password).

## Next steps

- [Workloads](help:workloads), to see every part of the window
- [Manage containers](help:manage-containers)
- [Restart, disable or enable a service](help:restart-a-service)
