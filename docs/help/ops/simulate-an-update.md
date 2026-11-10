---
id: simulate-an-update
title: Simulate an update
type: howto
area: updates
summary: See exactly what an upgrade would change, without changing anything.
keywords: [simulate, dry run, what would change, preview]
verified: false
related: [install-updates, updates]
---

# Simulate an update

To see exactly what an upgrade would change on a node, click **Simulate** in the **Updates** window. Nothing on the node changes.

SparkBox has shown a real upgrade plan for `cell2`, and nothing was installed. It hasn't run an upgrade on your cluster yet.

## Before you start

- You don't need Admin mode, and it works in look-only mode. A simulation only reads.
- With **Demo** data it simulates the simulated packages.
- A simulation is also called a **dry run**. You need one before you can upgrade with **Live** data, because SparkBox installs exactly what the dry run showed.
- SparkBox gives a dry run up to 2 minutes.

## Steps

1. In the sidebar, under **Tools**, click **Updates**. [Open Updates](open:updates) The **Updates** window opens.
2. In the table at the top, click the node you want, such as `cell2`. Its pending packages appear below.
3. Choose what to simulate. To cover everything pending on the node, select no packages. To cover only some, select them in the list. Hold <kbd>⌘</kbd> and click to select more than one.
4. In the toolbar, click **Simulate**, the flask. In a narrow window the toolbar shows icons without names: [Updates](help:updates#the-toolbar) lists them all. The toolbar dims for a moment. A panel named **Dry run on cell2** opens at the lower right, with a **nothing changed** badge.
5. Read the four numbers: **Upgrades**, **New**, **Removed** and **Held back**. **New** is packages the upgrade would add. **Held back** is packages apt leaves alone.
6. Read **Upgrade installs exactly this**. It lists each package with its old and new version.
7. Look for a red line that starts "Upgrade refused:". If there is one, the plan can't run as it stands. See [If something goes wrong](#if-something-goes-wrong).


## What you should see

The **Dry run on cell2** panel with its **nothing changed** badge. It also shows the exact command the upgrade would run as root, and the dry run's own command. To read apt's full reply, open **apt-get output** at the bottom of the panel.

If you change your selection, the panel says "This dry run was for everything; Simulate again for what's picked now." Click **Simulate** again.

## Why it works

SparkBox asks apt for the same upgrade it would really run, and tells it to plan without doing. It asks as the cluster user, not as root, so it can't change anything.

Apt works out which versions it would install and which other packages it would need. SparkBox shows that plan and judges it against its own safety rules. If you then click **Upgrade**, SparkBox installs that plan and nothing else.

## If something goes wrong

- **"Upgrade refused: Upgrade everything needs the GPU stack pinned first: docker-ce, nvidia-driver-580-open aren't held on cell2."** To upgrade everything at once, the GPU stack must be held still first. Follow [Pin the GPU stack](help:pin-the-gpu-stack), or select only the packages you want and simulate again.
- **"Upgrade refused: SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia."** The plan would move part of the GPU stack, the kernel, NVIDIA, CUDA, Docker and network software. Routine Upgrade keeps these packages unchanged. The separate [Protected system updates](help:updates#protected-system-updates) review supports one exact package set. It does not override this routine guard.
- **"Upgrade refused: 590 deadlocks CUDA graph capture on GB10: nvidia-driver-590-open."** The 590 NVIDIA driver has a known fault on your GB10 chips, so SparkBox never installs it.
- **"Upgrade refused: This plan removes libnvidia-compute-580 (a 580 package)."** The plan would remove part of the NVIDIA driver in use. SparkBox refuses it.
- **"Upgrade refused: This plan removes libfoo. SparkBox's one-node upgrades only upgrade; run it in a terminal if you mean it."** Upgrades in SparkBox never remove a package. Leave that package out, or ask yourself whether you need this upgrade at all.
- **"apt-get -s failed on cell2: …"** Apt itself couldn't plan. Its reason follows the colon. Fix that first, then simulate again.

## Next steps

- [Install updates on a node](help:install-updates)
- [Pin the GPU stack](help:pin-the-gpu-stack)
- [Packages, updates and the GPU stack](help:packages-and-updates)
