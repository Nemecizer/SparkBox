---
id: pin-the-gpu-stack
title: Pin the GPU stack
type: howto
area: updates
summary: Hold a node's GPU software at its versions, so that you can upgrade everything else, and release it later.
keywords: [pin, hold, gpu stack, driver, nvidia driver, cuda version, kernel, "don't update"]
verified: false
related: [packages-and-updates, install-updates]
---

# Pin the GPU stack

To upgrade everything else on a node while its GPU software stays exactly as it is, pin the GPU stack in the **Updates** window.

## Before you start

- You need this only for the [Upgrade everything pending](help:install-updates#upgrade-everything-pending) route. Security fixes for held packages stay pending while the pin is on.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data the button says "Available with Live data". [Unlock Admin mode](help:unlock-admin-mode) shows how.
- The **GPU stack** is the kernel, NVIDIA, CUDA, Docker and network software a node needs to run models. [Packages, updates and the GPU stack](help:packages-and-updates) explains why it needs care.
- You pin one node at a time. A pin takes effect at once. It installs nothing, restarts nothing, and takes moments.
- You can undo it at any time with **Release the GPU-stack pin…**.

## Steps

### Pin a node

1. In the sidebar, under **Tools**, click **Updates**. [Open Updates](open:updates) The **Updates** window opens.
2. In the table at the top, click the node, such as `cell2`.
3. In the toolbar, click **Pin the GPU stack…**, the second pin icon. **Hold** is the first one. In a narrow window the toolbar shows icons without names. This button may then be under the **More** button (**…**) at the end of the toolbar. [Updates](help:updates#the-toolbar) lists every icon.
4. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A dialog named **Pin the GPU stack on cell2?** opens. It lists every package the pin holds, read from the node.
5. Read the list, then click **Pin**. The task "Pin the GPU stack on cell2" runs.
6. Look at the **Held** column for the node. It now lists the held packages, and in the package list they carry a **held** badge.
7. Repeat for every other node, so that they all hold the same packages. Until you do, a notice at the top of the window says "Hold drift: cell3 holds different packages from the other nodes."

### Release a pin

1. In the **Updates** window, click the node.
2. In the toolbar, click **Release the GPU-stack pin…**, the second crossed-out pin.
3. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. A dialog named **Release the GPU-stack pin on cell2?** opens. It lists what it releases.
5. Click **Release**.

## What you should see

**Held** lists every package, and for 10 seconds the status bar names the task. [Tasks](open:tasks) keeps the result, such as "Pinned 30 GPU-stack packages on cell2". If another window is in front when it ends, a message says the same. With the pin in place on a node, a dry run of everything no longer stops at "Upgrade everything needs the GPU stack pinned first".

## Why it works

SparkBox reads every installed package, picks out the GPU stack, and tells apt to hold those packages at their versions. A held package stays at its version. Apt skips it, and SparkBox lists it under **Held back** in a dry run.

The task's log lists every package, and a copy goes in the evidence folder on this Mac, in `~/Library/Logs/SparkBox/evidence`. Releasing takes the hold off the GPU-stack packages that are held.

## Limits

- `fwupd`, the firmware updater, isn't held. The dialog says "fwupd isn't held, and SparkBox doesn't upgrade it either yet." See [What's coming](help:whats-coming). If `fwupd` has an update waiting, a dry run of everything is still refused, so select the packages you want instead.
- A pin stops upgrades, so held packages stay pending and keep counting in **Pending** and **Needs care**. Security fixes for them stay pending too.
- **Hold** and **Release** in the toolbar work on the packages you select. The pin works on the whole GPU stack.

## If something goes wrong

- **"No GPU-stack packages are installed on cell2."** SparkBox found nothing to hold. Check that you selected the right node.
- **"Nothing of the GPU stack is held on cell2."** There's nothing to release on that node.
- **"apt-mark failed on cell2: …"** Apt couldn't change the holds. Its reason follows the colon. Open the task in [Tasks](open:tasks) to read the log.
- **The button is dimmed.** Move the pointer over it. It needs a selected node, **Live** data, and no other update work in progress.

## Next steps

- [Install updates on a node](help:install-updates)
- [Simulate an update](help:simulate-an-update)
