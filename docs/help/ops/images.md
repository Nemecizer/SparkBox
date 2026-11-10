---
id: images
title: Images
type: feature
area: ops
summary: A read-only list of the Docker images on each node, their sizes and what uses them.
keywords: [images, unused, dangling]
verified: false
related: [check-docker-images]
---

# Images

## What it's for

**Images** lists the Docker images on each node. Use it to see what's taking disk space and what hasn't run for months. It only reads: nothing here changes a node.

An **image** is the recipe a container starts from. [Containers and images](help:containers-and-images) explains the idea.

## Where to find it

- **Sidebar:** under **Workloads**, click **Images**.
- **Containers window:** in the toolbar, click **Images…**. [Containers](help:containers) explains that window.
- **Command palette:** press <kbd>⌘K</kbd> and type `images`.
- **Link:** [Open Images](open:images)

## What's on the screen


### Show

The first choice in the toolbar filters the list:

- **All** lists every image.
- **Unused** lists images no container has run for 30 days, or ever on that node.
- **Dangling** lists untagged images left behind by pulls and builds. A **tag** is an image's version name, and a dangling image has lost its name. A **pull** is a download from an image registry.

**All nodes** limits the list to one node. **Filter images** narrows it by name, ID or container. **Refresh** reads every node again. The window reads once when it opens.

### One line for each node

Above the table, each node has a summary, such as "gx10-0000: 6 images · 85.6 GiB · 2 unused for 186 days · 3 never used here". Click a line to show only that node. Click it again to go back.

### The list

| Column | What it shows |
|---|---|
| **Image** | Its name and tag, or `<none>` and its ID if it's dangling |
| **Node** | Which node it's on |
| **Size** | Its size on disk |
| **Created** | When the image was built |
| **Last used** | **running now**, how long ago its last container stopped, or **never here** |
| **Used by** | The containers made from it, running or not |
| **Dangling** | Marks a dangling image |

**Last used** is dimmed when an image counts as unused.

The line under the table gives the number of images and the total size. It may add what Docker says it could free, such as "Docker could free 21.4 GiB". That's what images no container uses would give back.

### What the numbers mean

Sizes are Docker's. Two images can share layers, the parts an image is built from, and Docker counts a shared layer in both. So the **Size** column can add up to more than Docker's total.

"Unused" only looks at containers on that node. A model load can start a container from any image at any time, so check before you delete one.

## What it changes

Nothing. Each node answers `docker image ls` and `docker system df`, which only read. It needs no Admin mode, and it works in look-only mode.

SparkBox has no button to delete an image. [Check Docker images](help:check-docker-images) says how to do that in a terminal.

## Related How-Tos

- [Check Docker images](help:check-docker-images)
- [Check disk space on a node](help:check-disk-space)

![Images, using synthetic Demo data](img/public-images.jpg)
