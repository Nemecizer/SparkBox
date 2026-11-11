---
id: tasks-and-jobs
title: Tasks and jobs
type: concept
area: ops
summary: A task is anything SparkBox is doing. A job is long work running by itself on a node.
keywords: [task, job, background, keeps running]
verified: null
related: [tasks, models-jobs]
---

# Tasks and jobs

## In short

A **task** is anything SparkBox is doing, listed in **Tasks**. A **job** is long work running by itself on a node, such as a download or a copy, listed in **Models › Jobs**. SparkBox runs a task. A node runs a job.

## How it works

A task lives in SparkBox on this Mac. When you restart a node or run a Fleet command, SparkBox starts a task. It sends the commands, watches what happens, records each step, and tells you how it ended. A task ends as **Done**, **Done with a warning**, **Failed** or **Stopped**, and SparkBox keeps the last 200.

A job lives on the node. SparkBox hands the work to a small helper it keeps there, `~/.sparkbox/sparkjob.py`, and the helper runs it apart from SparkBox's connection. The job's progress is saved on the node, and SparkBox reads it to show you. SparkBox can show a task that follows a job. The persistent job record stays on the node and appears in the [Jobs tab](help:models-jobs).

An upgrade is a mix of the two. It's a task in SparkBox, and apt runs on the node in a background unit of its own. It's built to keep going if SparkBox disconnects or quits, and SparkBox follows it again when it's back. That hasn't been proven on your cluster, so keep SparkBox open until it ends. It shows in **Tasks** and in the **Updates** window, not in **Jobs**.

Think of a task as washing by hand, and a job as the washing machine: it does the work itself, and you read its display.

## Why it matters in SparkBox

The difference tells you where to look, and what to expect.

- To follow a restart, a Fleet run or an upgrade, look in [Tasks](help:tasks) or at the status bar.
- To follow a download or a copy, look in the [Jobs tab](help:models-jobs).
- To stop a task, use **Cancel** in **Tasks**. To stop a separate job, use **Cancel** in **Jobs**; for a chain, use **Cancel chain…**. Files and records stay on the nodes. Chain rows cannot use ordinary Resume. See [Follow a download](help:follow-a-download) before recovery.

A bounded two-node chain kept its job IDs through normal SparkBox quit/reopen, without replacement work. That does not test Mac sleep, network loss or node reboot. Keep SparkBox open until every chain job appears, and keep this Mac awake while watching it. See [Keep this Mac awake during long work](help:keep-this-mac-awake). See [What's coming](help:whats-coming).

## Related

- [Tasks](help:tasks)
- [Models: Jobs tab](help:models-jobs)
- [Follow a download](help:follow-a-download)
