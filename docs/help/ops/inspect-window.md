---
id: inspect-window
title: Inspect
type: feature
area: ops
summary: "A container's full details: image, state, ports, mounts and labels, with secret-looking values hidden."
keywords: [inspect, docker inspect, mounts, labels]
verified: false
related: [containers, manage-containers]
---

# Inspect

## What it's for

**Inspect** shows everything Docker knows about one container: what it runs, how it started, what it can reach, and how it's set up. Use it to answer "what exactly is this container?", or to check a setting when something isn't working.

## Where to find it

- **Containers window:** select a container and press <kbd>Return</kbd> or <kbd>⌘I</kbd>. Or right-click a row and choose **Inspect**.
- **Workloads window:** right-click a process in a container and choose **Inspect its container**.
- **Command palette:** press <kbd>⌘K</kbd> and type `inspect` with the container's name.

The window's title is the container's name and its node, such as "vllm-head on cell1".

## What's on the screen


### The toolbar

- **Summary** shows the details in plain rows. This is what opens first.
- **Raw JSON** shows everything Docker reported, as text.
- **Copy JSON** copies it as shown. Hidden values stay hidden.
- **Refresh** reads the container again.

### Summary

| Row | What it shows |
|---|---|
| **Name** | The container's name |
| **ID** | The start of its ID |
| **Image** | The image it started from |
| **State** | Its state, with its exit code and any out-of-memory kill. Its health check follows if it has one |
| **Started** | When it started, and how long ago |
| **Finished** | When it stopped, for a container that isn't running |
| **Created** | When it was created |
| **Restarts** | How many times Docker has restarted it |
| **Command** | The command it runs |
| **Network** | Its network mode, with its shared-memory settings |
| **GPUs** | "all (--gpus)" if it can use the GPU, or "none" |
| **Restart** | Its **restart policy**: when Docker starts it again by itself. "no" means never |
| **Ports** | Which ports it opens. "host network (all of the node's ports)" means it uses the node's own network |

### Mounts

Folders on the node that the container can see. **Source on the node** is the folder on the node, **Inside the container** is where it appears, and **Mode** is **read-write** or **read-only**.

### Environment

The settings the container started with, one per line. SparkBox hides any value that looks like a secret, such as `HF_TOKEN` or `API_KEY`, and shows "•••• hidden". The heading says how many.

**Reveal** shows one hidden value. It needs Admin mode, and it's logged in the [Activity log](help:activity-log). The value shows only while this window is open, and it's never saved.

A value over 200 characters starts folded, with **Show all** beside it.

### Labels

Short notes attached to the container. SparkBox's own start with `sparkbox.`.

## What it changes

Nothing on the node. It needs no Admin mode and works in look-only mode, except **Reveal**, which needs Admin mode and is off in look-only mode.

## Related How-Tos

- [Manage containers](help:manage-containers)
- [Fix a load that failed](help:fix-a-load-that-failed)

![Inspect, using synthetic Demo data](img/public-inspect-window.jpg)
