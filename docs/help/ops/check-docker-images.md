---
id: check-docker-images
title: Check Docker images
type: howto
area: ops
summary: See which images are on each node, how much disk they use, and which are unused.
keywords: [docker image, disk, unused, dangling]
verified: false
related: [images, containers-and-images, check-disk-space]
---

# Check Docker images

To see which Docker images are on each node and how much disk they use, open the **Images** window. An **image** is the recipe a container starts from. [Containers and images](help:containers-and-images) explains the idea.

## Before you start

- You don't need Admin mode. Everything here only reads, so it also works in look-only mode.
- With **Demo** data the images are simulated.
- It takes less than a minute.

## Steps

1. In the sidebar, under **Workloads**, click **Images**. [Open Images](open:images) The **Images** window opens and asks each node for its list. The list is sorted by **Size**, biggest first.
2. Read the lines above the table. There's one for each node, such as "gx10-0000: 6 images · 85.6 GiB · 2 unused for 186 days · 3 never used here".
3. To look at one node, click its line. Click it again to go back to all nodes.
4. To see what you could clear out, click **Unused** in the toolbar. It lists images no container has run for 30 days, or ever on that node.
5. To see leftovers, click **Dangling**. These are untagged images left behind by updates and builds.
6. Move the pointer over a row's **Used by** cell to see which containers use that image.
7. Read the line under the table. It gives the total, and may say "Docker could free 21.4 GiB".

## What you should see

One row for each image on each node, with its **Size**, **Created** date, **Last used** and **Used by**. **Last used** says **running now** if a container runs it, or **never here** if no container on that node has used it.

## Why it works

Each node tells SparkBox its images and how much disk they use, and that request only reads. SparkBox joins those answers with the node's containers to work out what uses each image.

Sizes are Docker's own. Two images can share parts, and Docker counts a shared part in both. So the **Size** column can add up to more than the node's total.

## Limits

- SparkBox doesn't delete images. To free the space, open a [terminal](help:open-a-terminal) on the node and run `docker image rm` with the image's name. Docker refuses if a container still needs it.
- The list reads once, when the window opens. To read it again, click **Refresh**.

## If something goes wrong

- **A node's line shows an error instead of its images.** The node is offline, or Docker refused. Check that the sidebar shows its GPU use, such as **GPU 28%**. If the node is online, click **Refresh**. The window reads once, and it may have opened before the node connected.
- **"No node answered with its images."** No node could be read. See [Connection and login messages](help:errors-connection).
- **Unused shows an image you use.** Models and other tools can start a container from an image at any time. "Unused" only means no container on that node has run it lately. Check before you delete it.

## Next steps

- [Check disk space on a node](help:check-disk-space)
- [Images](help:images), to see every part of the window
- [Manage containers](help:manage-containers)
