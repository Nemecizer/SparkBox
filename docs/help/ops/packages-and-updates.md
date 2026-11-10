---
id: packages-and-updates
title: Packages, updates and the GPU stack
type: concept
area: updates
summary: How software on a node is updated, and why the GPU stack needs more care than the rest.
keywords: [apt, package, gpu stack, hold, phasing, kernel]
verified: null
related: [updates, pin-the-gpu-stack, simulate-an-update]
---

# Packages, updates and the GPU stack

## In short

The software on a node comes in **packages**, and **apt** is the program that installs them. Most packages can be updated safely one node at a time. The **GPU stack** can't, because your nodes need matching versions of it to run models together.

## How it works

### Packages and apt

A **package** is one piece of installable software with a version number. Your nodes run DGX OS, which is built on Ubuntu, so they use Ubuntu's tool, **apt**. Apt keeps a local list of what's available from a set of **archives**, online stores of packages. `apt-get update` refreshes that list. It installs nothing. Upgrading is a separate step.

A **dry run** is apt planning an upgrade without doing it. It lists what it would install, add, remove and keep back.

A package can be **held**, which tells apt to leave it at its version. The hold stays until someone releases it.

### What the GPU stack is

The GPU stack is the software between your models and your hardware:

- the **kernel**, the core of the operating system
- the **NVIDIA driver**, which runs the GPU
- **CUDA**, NVIDIA's platform for GPU programs
- **Docker**, which runs the engines
- the **network stack** for the ConnectX-7 fabric
- the DGX OS updater and firmware tool

NVIDIA control libraries such as `libxnvctrl0` belong to this group too. They stay pending with the other NVIDIA packages.

These parts depend on each other. The kernel and the driver must fit, or a node can come up without its GPU, or not come up at all. And a model spread over several nodes needs the same driver, CUDA and engine image on all of them. If one node moves ahead, the set stops matching.

So SparkBox treats the GPU stack differently. A dry run that would move it on one node is refused, and so is a selection that names it. The 590 NVIDIA driver is never installed, because it has a known fault on your GB10 chips.

### Pinning

To upgrade everything else on a node, you first **pin** the GPU stack, which holds every one of its packages at once. Apt can then upgrade the rest, and the stack can't move, even as a dependency of something else.

### After an upgrade

A program that's already running keeps using the old version of a library, a shared piece of code. It gets the new one only when it restarts. **needrestart** lists these programs. Some updates need the whole node to restart: a new kernel is used only after the next start.

### Phased updates

Ubuntu releases some updates a little at a time, to a share of computers first, in case something's wrong. Apt calls this **phasing**. A package can be available but not yet offered to your node. It stays pending, and it isn't installed this time.

## Why it matters in SparkBox

Everything you do in [Updates](help:updates) follows from this:

- **Needs care** counts GPU-stack packages.
- [Simulate an update](help:simulate-an-update) shows the plan, and SparkBox installs only that plan.
- [Pin the GPU stack](help:pin-the-gpu-stack) makes it safe to upgrade the rest.
- [Install updates on a node](help:install-updates) works on one node at a time.

### A good routine

Look at [Updates](help:updates) about once a month, and sooner when the sidebar shows a badge such as `3 security`. Take security fixes when they appear, with **Upgrade security only**. It's the smallest useful change, it needs no pin, and it leaves the GPU stack alone. Try one node first, ideally one that serves no model, and check it before you do the others. The other nodes then get the same fixes, one at a time. To compare their reported versions, open **Versions** in [Fleet Command](help:fleet-command#versions). **Matching** means the comparable readings agree; **Unknown** means agreement could not be established.

Pick packages by hand when you want one named update. Pin the GPU stack only when you want to bring a node fully up to date, and do it at a quiet time. [Install updates on a node](help:install-updates) has the steps for each.

### The GPU stack stays put

SparkBox has no way yet to update the GPU stack across the whole cluster in step, so it says it can't upgrade those packages yet. See [What's coming](help:whats-coming). Until then, leave the stack where it is. Checks before a restart are there for the same reason: [Restart a node](help:restart-a-node) won't restart a node whose kernel and driver don't fit.

## Related

- [See what updates are waiting](help:see-pending-updates)
- [Pin the GPU stack](help:pin-the-gpu-stack)
- [Install updates on a node](help:install-updates)
- [Updates](help:updates)
