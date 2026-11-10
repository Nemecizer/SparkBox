---
id: containers
title: Containers
type: feature
area: ops
summary: Every Docker container on every node, with state, exit reason and actions.
keywords: [docker, exited, deployments, test container]
verified: false
related: [manage-containers, containers-and-images]
---

# Containers

## What it's for

**Containers** lists every Docker container on every node, running or stopped. Use it to see how your containers are doing, read why one stopped, and start, stop, restart or remove them.

A **container** is a program running in its own sealed box. [Containers and images](help:containers-and-images) explains the idea.

## Where to find it

- **Sidebar:** under **Workloads**, click **Containers**. Or right-click a node and choose **Containers**.
- **Menu bar:** choose **Go › Containers**.
- **Keyboard:** press <kbd>⌘3</kbd>.
- **Command palette:** press <kbd>⌘K</kbd> and type a container's name, for commands such as **Logs of** and **Restart**.
- **Link:** [Open Containers](open:containers)

## What's on the screen


### The toolbar

A new **Containers** window shows its buttons as icons without names. **Start** is a triangle, **Stop…** a square in a circle and **Restart…** a curved arrow. To see the main names, double-click the title bar to widen the window.

**Remove…** stays under the **More** button (**…**) at the far end of the toolbar, unless the window is very wide. Two buttons are called **More**. The small arrow after **Restart…** holds **Force-stop…**, **Pause…** and **Resume**. The **…** at the far end holds **Remove…** and anything else that doesn't fit.

| Control | What it does |
|---|---|
| **Start** | Starts the selected stopped containers |
| **Stop…** | Stops the selected running containers, after it asks |
| **Restart…** | Restarts the selected containers, after it asks |
| **Remove…** | Deletes the selected stopped containers. Their images stay |
| **More**, the small arrow | **Force-stop…**, **Pause…** and **Resume** |
| **Images…** | Opens the [Images](help:images) window |
| **Run test container** | See [Run test container](#run-test-container) |
| **List** and **Deployments** | See [List and Deployments](#list-and-deployments) |
| **All states** | Shows all containers, or **Running only** |
| **All nodes** | Limits the list to one node |
| **Filter** | Narrows the list by name, image, project or node |

With several containers selected, the buttons show how many they'd act on, such as **Stop 3…**. A dimmed button explains why when you move the pointer over it.

### The list

A narrow window hides some columns, so make it wider to see them all.

| Column | What it shows |
|---|---|
| **Name** | The container's name. In a deployment, its part: head, worker or rank. A SparkBox load on several nodes names its containers by rank, such as `sparkbox-m1a2b3-r1`, never by node |
| **Node** | Which node it's on |
| **Image** | The image it started from |
| **State** | **running**, **exited**, **created**, **restarting**, **paused** or **dead** |
| **Health** | The container's own health check, if it has one: **healthy**, **starting** or **unhealthy** |
| **Exit reason** | How it stopped. See [Exit reason](#exit-reason) |
| **Restarts** | How many times Docker has restarted it. Over 3 turns red |
| **CPU**, **Memory**, **GPU memory** | What it's using now, for running containers |
| **Ports** | Which ports it opens on the node |
| **Started** | When it started, or when it exited if it's stopped |

Select a single container, and a line at the bottom shows its short ID and command. A node that goes offline keeps its last-known rows, dimmed, with "unknown · cell3 offline" until it's back. A stale report reads "unknown · cell3 not reporting". Move the pointer over the state for its last observation time, when known. Old exit reasons do not appear as current failures while the node is unavailable.

**Deployments** keeps expected members in rank order when nodes stop reporting. Its state reads **Unknown**, and its running count lists unknown members separately. A fresh report can confirm that a container stopped or was removed. Saved SparkBox loads and discovered groups keep their expected members across app restarts. These records stay historical until fresh reports arrive. Discovered groups are retired once fresh reports confirm all their containers were removed.

With a row selected, press <kbd>⌘L</kbd> for **Logs**, <kbd>⌘T</kbd> for **Shell** or <kbd>⌘I</kbd> for **Inspect**. <kbd>Return</kbd> or a double-click also inspects. Right-click a row for every action, plus **Copy ID** and **Copy name**.

**CPU** comes from the container's CPU time; 100% means one fully busy core. **Memory** is Linux's accounting for the container, while **GPU memory** adds its processes' driver-reported allocations. They can differ greatly on a Spark. Use the node's **Memory** tab to judge remaining room; don't add either figure to the node's **Used** total. [How memory works on a Spark](help:memory-on-a-spark#comparing-the-memory-readings) explains the difference.

### List and Deployments

**List** shows one row for each container. **Deployments** groups them. A group is one model's containers, or one Compose project, a set of containers started together. A group's line says what it serves and how it's doing, such as "glm53 · 4 nodes · ranks 0–3 · Failed".

Click a group's row to select all its containers. SparkBox also groups containers it didn't start, when they look like one launch. Actions on a group run in dependency order: for those, workers stop before their head, and the head starts before its workers. SparkBox's own vLLM loads go the other way: the head stops first, and starts after its workers. To restart a SparkBox load, use the restart it offers, which stops the whole load, then starts the workers first and the head node last, and checks it answers.

### Exit reason

This says how a stopped container ended:

| Words | Meaning |
|---|---|
| "Exited 0 after 3 min" | It finished normally |
| "Exited 1 after 54 s" | It stopped with an error. Any number but 0 is an error |
| "Stopped (143)" | Something asked it to stop |
| "Interrupted (130)" | It was interrupted |
| "Killed (137) after 3 min" | It was killed by force |
| "Out of memory (137) after 3 min" | The node ran out of memory and killed it |
| "Never started" | It was created and never run |
| "Exited 1 · see deployment" | It's part of a failed deployment, which has the details |

An exit with an error, or an out-of-memory kill, counts for 24 hours. It's red in this list, and it counts in the sidebar and on the Dashboard. After that it's history: it stays in the list but stops counting.

To stop counting one sooner, right-click its row and choose **Hide this exit**. If the same container crashes again, the new exit counts.

### Run test container

This starts `sparkbox-test-busybox`, SparkBox's own small test container, on one node. It sleeps for an hour and does nothing else, as a harmless way to check that Docker works there.

It needs **Live** data, Admin mode and one node chosen in the **All nodes** list. Docker downloads `busybox:1.36` the first time. Use **Stop…** and **Remove…** to clear it away. Only one can exist on a node: "A test container is already on cell2; remove it first."

### Containers you can read but not change

A row that says "managed by the other SparkBox" was started by the test build. Each build labels its own containers and leaves the other's alone. You can read these, and **Stop…** and the other actions stay off.

## What it changes

Looking, logs and **Inspect** change nothing and need no Admin mode. These change the node, and need Admin mode:

| Action | What it changes | Undo |
|---|---|---|
| **Start**, **Resume** | Starts the container | **Stop…** |
| **Stop…**, **Force-stop…** | Ends the program in the container. Anything it served goes down | **Start**. What it held only in memory is gone |
| **Restart…** | Stops and starts it. A model must load again | None needed |
| **Pause…** | Freezes the container | **Resume** |
| **Remove…** | Deletes the container | None. Start a new one from its image |
| **Also remove anonymous volumes** | Deletes the data in its volumes too | None. The data is gone for good |
| **Shell** | Whatever you type inside the container | Depends what you do |

If the containers serve a several-node model, **Restart…** says what that does to the model. It offers to restart the whole model in order instead.

## Related How-Tos

- [Manage containers](help:manage-containers)
- [Check Docker images](help:check-docker-images)
- [Fix a load that failed](help:fix-a-load-that-failed)

![Containers, using synthetic Demo data](img/public-containers.jpg)
