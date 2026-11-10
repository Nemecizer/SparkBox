---
id: manage-containers
title: Manage containers
type: howto
area: ops
summary: See the containers on your nodes and start, stop, restart or remove them, and read their logs.
keywords: [docker, container, logs, read logs, container log, stop container, remove]
verified: false
related: [containers, containers-and-images, check-docker-images]
---

# Manage containers

To see, control or remove the containers on your nodes, use the **Containers** window. A **container** is a program running in its own sealed box. [Containers and images](help:containers-and-images) explains the idea.

## Before you start

- Looking and reading logs need nothing: no Admin mode, and they work in look-only mode.
- To start, stop, restart or remove a container, use **Live** data with look-only mode off, and be ready to unlock Admin mode. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- With **Demo** data the containers are simulated.
- To stop or restart a model, prefer [Models](open:models). [Unload a model](help:unload-a-model) and [Restart a model or change its settings](help:restart-or-change-a-model) stop its parts in the right order, and check that it answers.

## Steps

### See your containers

1. In the sidebar, under **Workloads**, click **Containers**. [Open Containers](open:containers) The **Containers** window opens with one row for each container, on every node.
2. To look at one node, choose it in the **All nodes** list in the toolbar.
3. To hide stopped containers, choose **Running only** in the **All states** list.
4. Read **State** and **Exit reason**. **running** is working. For a stopped container, **Exit reason** says how it ended, such as "Exited 1 after 54 s".


### Read a container's log

5. Click the container's row.
6. Press <kbd>⌘L</kbd>, or right-click the row and choose **Logs**. A [Terminal](open:terminal) tab opens and follows the log as it grows.

### Start, stop or restart a container

7. Click the container's row. To select more than one, hold <kbd>⌘</kbd> and click.
8. In the toolbar, click **Start**, **Stop…** or **Restart…**. A new **Containers** window shows these as icons without names: a triangle, a square in a circle and a curved arrow. To see the names, double-click the title bar to widen the window.
9. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
10. **Start** runs at once unless it affects something. If no dialog opens, go to step 13. **Stop…** and **Restart…** always open a dialog that says what goes down.
11. Read the dialog. If the container is part of a model, it says what happens to that model.
12. Click the button at the bottom of the dialog. If it offers a restart "in order", prefer that. Restarting a several-node model one container at a time breaks it: its workers exit when the head restarts.
13. Watch **State** change in the list.

### Remove a stopped container

A container must be stopped before you can remove it.

14. Click the stopped container's row.
15. In the toolbar, click the **More** button (**…**) at the far end, then click **Remove…**. If the window is very wide, **Remove…** is on the toolbar itself, with a trash can.
16. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
17. A dialog opens and says "The containers are deleted; their images stay."

> [!DANGER]
> **Also remove anonymous volumes deletes that data for good.** A **volume** is storage a container keeps outside itself, and SparkBox can't get it back. Leave this box off unless you know the data isn't needed.

18. Leave **Also remove anonymous volumes** off.
19. If you see "Type cell2 to confirm", type the node's name. SparkBox asks for this when the stakes are high.
20. Click **Remove**.

## What you should see

The row's **State** changes, and for 10 seconds the status bar names the task. If another window is in front, a message gives the result, such as "vllm-head restarted". [Tasks](open:tasks) keeps it. Several containers at once show as one task, such as "3 of 3 restarted". If the containers serve a model, the task waits for it to answer again.

## Why it works

SparkBox runs `docker` commands on the node for you, the same as you'd type in a terminal, and records them in the [activity log](help:find-what-sparkbox-did). For several containers it uses one task and runs them in order. For a server SparkBox didn't start, workers stop before their head, and the head starts before its workers. SparkBox's own vLLM loads go the other way: the head stops first, and starts after its workers. If a command fails on one container, it keeps going with the rest and tells you at the end.

## If something goes wrong

- **A button is dimmed.** Move the pointer over it. It names the reason, such as "Already running", "Stop it first" or "cell3 is offline".
- **A row says "managed by the other SparkBox".** The test build started this container. Each build leaves the other's containers alone, so you can read it but not act on it.
- **"docker stop failed on cell2: …"** Docker's own reason follows the colon. The container's log usually says more.
- **Exit reason says "Out of memory (137)".** The node ran out of memory and killed the container. See [How memory works on a Spark](help:memory-on-a-spark).
- **The sidebar shows "1 exited".** A container ended badly in the last 24 hours. To stop counting it, right-click its row and choose **Hide this exit**.

## Next steps

- [Containers](help:containers), to see every part of the window
- [Check Docker images](help:check-docker-images)
- [Fix a load that failed](help:fix-a-load-that-failed)
