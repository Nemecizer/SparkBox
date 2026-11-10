---
id: tasks
title: Tasks
type: feature
area: ops
summary: Everything SparkBox is doing or has done, with steps, logs and Cancel.
keywords: [tasks, progress, cancel, running]
verified: false
related: [find-what-sparkbox-did, follow-a-download, tasks-and-jobs]
---

# Tasks

## What it's for

**Tasks** lists what SparkBox is doing and has done: restarts, service and container actions, Fleet runs, upgrades and more. Use it to follow something long, to read why something failed, or to run it again.

A **task** is one thing SparkBox is doing. [Tasks and jobs](help:tasks-and-jobs) explains how tasks differ from jobs.

## Where to find it

- **Sidebar:** under **Tools**, click **Tasks**. If you don't see it, scroll the sidebar or type in **Filter**.
- **Menu bar:** choose **Cluster › Tasks**.
- **Keyboard:** press <kbd>⇧⌘J</kbd>.
- **Status bar:** click the task indicator, then click **All tasks**.
- **Failed task:** on a failed task's message, click **Details**.
- **Link:** [Open Tasks](open:tasks)

## What's on the screen


### The status bar

While tasks run, the status bar shows a spinner with the task's name, or "2 running". For 10 seconds after a task ends, it shows how it ended. When nothing is running, it shows nothing.

Click it to open a small list of what's running, with progress, time left and **Cancel**, and the 20 most recent tasks. **Open** goes to one of them in the **Tasks** window.

### The list

**All**, **Running** and **Failed** filter the list. **Running** and **Failed** show a count when it isn't zero. **Filter tasks** narrows it by name, node or error. **Cancel** stops the selected task. With **Demo** data, a **Demo** badge reminds you these ran on the simulator.

| Column | What it shows |
|---|---|
| **State** | **Running**, **Done**, **Done with a warning**, **Failed** or **Stopped** |
| **Task** | What it was |
| **Nodes** | Where it ran |
| **Started** | How long ago |
| **Took** | How long it ran |
| **Outcome** | What it found, or why it failed |

**Done with a warning** means the task finished, but something didn't come back as it should. A model that doesn't answer after a restart is one example. Failed rows are red, and rows with a warning are amber.

### A task's details

Click a row, and a panel opens at the right. **Nodes** and **Outcome** leave the list to make room.

- **Cancel** stops a running task. See [Cancel](#cancel).
- **Retry…** starts a finished task again. See [Retry](#retry).
- **Commands** opens the [activity log](help:activity-log) on this task's commands alone.
- **Started**, **Took**, **Nodes** and **Started from** say when, where and from which window.
- A progress bar shows while it runs, with its speed and time left when known.
- The error, warning or summary follows.
- **Steps** lists each step with how long it took. A step can be running, done, failed or skipped.
- **Log** shows the task's output, up to the last 400 lines.

### Cancel

Cancel stops a task at its next safe point. What that means depends on the task:

- A restart set for now can be called off until it's sent. A scheduled restart or shutdown can be called off until its time.
- A Fleet run stops, and the nodes it hasn't reached are skipped.
- An upgrade can be stopped only until apt starts installing.
- A wake can't be stopped. The button is dimmed, with "This task can't be stopped part-way".

Canceling needs Admin mode, as starting did. A task that only reads, such as a benchmark, needs none.

### Retry

**Retry…** never replays a task blindly. It reopens the same question, with the same targets: a restart reopens its dialog, and asks for the node's name when the stakes are high. A shutdown always asks for the name. Containers and services show what they'd affect, and a Fleet run goes back through **Fleet Command**.

An upgrade goes back to the **Updates** window, with that node selected, where you need a new dry run.

## What it changes

The window itself changes nothing, and it needs no Admin mode. **Cancel** and **Retry…** act on the task, and they need Admin mode.

SparkBox keeps the last 200 tasks on this Mac, so they're still there after a restart of SparkBox. Titles are saved with anything that looks like a secret hidden. Demo and Live keep separate lists: you never see a simulated "Shut down cell3" while you're looking at the real cluster.

## Related How-Tos

- [Find out what SparkBox did](help:find-what-sparkbox-did)
- [Follow a download](help:follow-a-download)

![Tasks, using synthetic Demo data](img/public-tasks.jpg)
