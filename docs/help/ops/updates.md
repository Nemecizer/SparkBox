---
id: updates
title: Updates
type: feature
area: updates
summary: Pending packages on each node, with Simulate, Upgrade, Hold and Pin the GPU stack.
keywords: [updates, apt, held, needrestart, dgx os, firmware, update sparkbox, new version of sparkbox, sparkbox version, latest version]
verified: false
related: [see-pending-updates, install-updates, pin-the-gpu-stack]
---

# Updates

## What it's for

**Updates** shows what software can be updated on each node, and lets you upgrade one node at a time. Use it to see what's waiting, to preview an upgrade, and to install it safely.

Routine upgrades passed on all four nodes: 135 packages installed, with clean package checks. Node `cell2` also completed its protected 1024 update and the reviewed 1025 continuation after boot-setting and Plymouth repairs. Its 17-package continuation installed successfully, and it reconnected automatically 54 seconds after restarting. Every restart and protected-update health check passed, including all 32 installed or retained package versions; no apt updates remained. Gemma then passed readiness, answered in Chat and unloaded cleanly.

Node `cell1` completed the direct plan after reviewed Plymouth startup and recovery-boot repairs. All 16 upgrades and eight new packages installed, with no apt updates remaining. It reconnected automatically 53 seconds after the restart was sent. Every restart and protected-update check passed on kernel 1025, with the 580 driver and earlier recovery kernels retained. Qwen then passed readiness, answered in Chat and unloaded cleanly.

Node `cell3` also completed its own guarded repairs, all 24 packages and one restart. It reconnected automatically in 54 seconds. Every restart and protected-update health check passed on kernel 1025, with no apt updates remaining. Gemma passed readiness, answered a fresh Chat question and unloaded cleanly. The complete update histories for `cell1` and `cell2` remain available.

Node `cell4` then passed its own guarded startup, recovery and local hostname repairs, all 24 packages, one restart and automatic reconnection in 53 seconds. Every restart and protected-update health check passed, and Qwen answered a fresh Chat question. All four reviewed updates are now complete, with no apt packages pending at that check. Completed history remains available; do not repeat these installers.

[Packages, updates and the GPU stack](help:packages-and-updates) explains the ideas behind this window.

This window updates your nodes, not SparkBox. SparkBox doesn't update itself, and it doesn't check for new versions. A new version arrives as a new copy of the app, which replaces the one on this Mac. To see which version you have, choose **SparkBox › About SparkBox**.

## Where to find it

- **Sidebar:** under **Tools**, click **Updates**. With **Live** data, a number or a badge such as `3 security` shows there.
- **Command palette:** press <kbd>⌘K</kbd> and type `updates`.
- **Link:** [Open Updates](open:updates)

## What's on the screen


### Protected system updates


Expand **Protected system updates…** for the reviewed November 2026 package sets. For `cell1`, `cell3` and `cell4`, the direct plan takes kernel 1019 to `7.0.0-1025-nvidia`: 16 upgrades and eight new packages, with no removals. The original `cell2` path remains 13 upgrades and eight new packages for kernel 1024; its saved continuation is described below. It requires NVIDIA's `nvidia-spark-initcall-bl` version `1.0-2`, which addresses a reported display-related reboot loop. The running 580 GPU driver stays unchanged. Other reported kernel issues remain a reason to test one idle node first.

**Review system updates** reads the exact plan and safety checks without installing. Review and **Check system update** need Admin mode because the node protects its boot-menu file; SparkBox reads it through the normal authenticated connection without changing its permissions. **Update cell1 with proposed updates…** appears above the reviewed package list, with your chosen node's name. It opens the installation confirmation, where you type that name; review alone installs nothing. Every package, including new dependencies, has an exact version. A changed plan, package hold, active container, GPU job, broken package database or insufficient space refuses installation. Have a current data backup and local keyboard/display access before proceeding.

The previous kernel and its boot menu entry must be present. SparkBox keeps them; it does not remove old kernels, release holds, change the driver branch or flash device firmware. Recovery means choosing the previous kernel in GRUB, the boot menu, using local access. It does not undo every package or restore your data.

Follow **View package task** until installation and package checks pass. Then use **Restart…** beside that node. SparkBox checks the new kernel, its GPU module, the generated NVIDIA boot fix and the retained recovery kernel before restarting. After reconnection, click **Check system update**.

The boot fix must appear in recovery startup choices too. If that check refuses, keep the node online and preserve its checkpoint while the boot settings are repaired. A successful installation alone does not permit a restart. If newer reviewed packages appear afterward, verification keeps the next node blocked until the new plan is reviewed. See [Update and restart messages](help:errors-updates-and-power).

Verification reads all 24 installed versions for the direct plan, or 21 for the original plan, the package database, a new boot identity, kernel, NVIDIA boot option, GPU driver, both fabric links, Docker and Linux startup. A missing answer or a startup state such as **starting** keeps verification pending. Check again after startup settles. If it remains pending, resolve the reported issue before continuing.

A repaired startup can pass the health checks while the full system-update check still waits for newer packages. The `cell2` continuation below handles the exact reviewed 1025 versions. Other versions need a new compatibility review; SparkBox refuses a changed plan.

A saved checkpoint prevents another protected update and model loads on the reserved node until verification passes. Closing SparkBox preserves that checkpoint. It never repeats an installation or restart automatically. An interrupted review that sent no installer can be cleared with **Discard interrupted review** after reopening SparkBox. An interrupted or failed installation can require manual package repair; do not delete the checkpoint to bypass it. Demo has a separate simulated checkpoint.

### Prepare `cell1`, `cell3` and `cell4`

This direct plan passed live update and restart checks on `cell1`, `cell3` and `cell4`. It combines the previously reviewed package versions and archives; SparkBox must read each node's actual plan again before it can install. It accepts only these three saved node identities, Ubuntu 24.04 arm64, kernel 1019 and driver `580.178.04`. A renamed display label does not change a node's identity. Do not repeat an update already marked verified.

Before any installation, Linux startup must report **running**, with no failed services. GPU, both fabric links and Docker must be healthy, and the node must be idle. A Plymouth startup blocker must be diagnosed and repaired first. The persistent recovery-boot setting must also match the reviewed setting that carries NVIDIA's normal boot option into recovery choices when the boot menu is regenerated. If it is missing or changed, SparkBox stops and names the required repair. This review does not install that setting, repair Plymouth, mask services or clear failed checks. The earlier repairs on `cell2` do not prove that the other nodes are ready.

The 1019 kernel, its matching GPU module, its startup files and both normal and recovery menu entries must remain available. After installation, both the 1019 and 1025 menu entries must contain NVIDIA's boot fix before Restart can proceed. This direct path does not install or require kernel 1024. A local recovery boot has not been tested; keep keyboard/display access and a current backup.

Review one node, confirm its named Update button, follow the package task, restart separately and use **Check system update**. Every package and health check must pass before the next protected node can begin. Test model serving after verification, then unload the test model before continuing. A healthy restart alone does not finish the protected-update check.

**Completed system updates** keeps earlier verified records when another update starts, including `cell2`'s original and continuation package-task buttons. A refusal or interrupted review preserves that history. An installer that may have started keeps its reservation; never remove it to continue.

### Continue `cell2` to kernel 1025


**Review 1025 continuation** appears beside `cell2`'s saved original update. It reads the node again before offering nine upgrades and eight new packages for kernel `7.0.0-1025-nvidia`. The original packages must be fully installed, kernel 1024 must have started successfully on a new boot, and every existing health check must pass. No model, container or GPU job may be running. The original checkpoint, package task and versions stay recorded.

The review accepts only the observed Ubuntu archive and exact package versions. It keeps driver `580.178.04`, the NVIDIA boot fix, and both kernels 1019 and 1024 for local recovery. It does not change package holds or allow routine **Upgrade** to install protected packages. Have a current data backup and local keyboard/display access: a package failure or failed startup may need repair at the node.

Choose **Update cell2 with proposed updates…** above the package list, complete Admin confirmation if requested, and type `cell2`'s displayed name. SparkBox checks the plan and healthy baseline again before sending the installer. **Close review** or canceling the confirmation installs nothing. A refusal before the installer starts restores the original reservation; it does not release the node for another update or a model load.

Follow **View package task**. After the package task passes, use **Restart…** separately, wait for automatic reconnection, and choose **Check system update**. Verification now checks 32 exact installed or retained package versions, both recovery kernels, the 1025 boot and its NVIDIA option, GPU, fabric links, Docker, Linux startup and remaining reviewed updates. Missing evidence, a partial installation, an unchanged boot, **starting**, a failed service or a newer pending version keeps the reservation active.

**View original package task** opens the earlier 1024 task. Its historical result is preserved. If SparkBox closes before installation could start, **Discard interrupted review** restores that original reservation. If an installer may have started, discard and another install are refused; follow the saved task and repair or verify the node. SparkBox never repeats the installation or restart automatically. After verification passes, use the separate direct review above for the remaining nodes; this continuation applies only to `cell2`'s existing checkpoint. Do not repeat a completed continuation.

### The toolbar

The toolbar shows names when the window is wide enough. A new **Updates** window names its first three buttons and shows the rest as icons, in the order below, so this table lists each icon. The other names stay hidden on most screens, even in a window that fills the desk. In a narrower window every button is an icon, and in a very narrow one the last buttons move under the **More** button (**…**) at the end of the toolbar.

| Control | Icon | What it does | Needs Admin mode |
|---|---|---|---|
| **Check all nodes** | Two curved arrows | Fetches new package lists on every node. See [Check for updates now](help:check-for-updates) | Yes |
| **Simulate** | A flask | A dry run of what's selected, or of everything. See [Simulate an update](help:simulate-an-update) | No |
| **Upgrade node** | A box with a checkmark | Installs the plan the dry run showed. With packages selected it reads **Upgrade 3** | Yes |
| **Upgrade security only** | A shield with a checkmark | See [Upgrade security only](#upgrade-security-only) | Yes |
| **Hold** | The first pin | See [Hold and Release](#hold-and-release) | Yes |
| **Release** | The first crossed-out pin | See [Hold and Release](#hold-and-release) | Yes |
| **Pin the GPU stack…** | The second pin | Holds the whole GPU stack on this node. See [Pin the GPU stack](help:pin-the-gpu-stack). Live data only | Yes |
| **Release the GPU-stack pin…** | The second crossed-out pin | Releases that hold. Live data only | Yes |
| **Restart node…** | A curved arrow | Shows only when the node needs a restart. See [Restart a node](help:restart-a-node) | Yes |
| **Open DGX Dashboard** | A square with an arrow | Opens NVIDIA's own dashboard for the node, which also shows DGX OS updates. Look only. Don't install from that page: it skips SparkBox's GPU-stack guard and its restart checks, and a failed update can stop the node starting. See [Open the DGX Dashboard](help:open-the-dgx-dashboard) | Yes |
| **Filter packages** | A text box | Narrows the package list by name or archive | No |

Move the pointer over a button to see what it does. That text doesn't always include the button's name. On a dimmed button, it says why the button is off.

A notice at the top warns of drift, when nodes differ. "NVIDIA driver drift: …" means nodes run different drivers, and "Multi-node models need the same driver everywhere." "Hold drift: …" means nodes hold different packages.

### The nodes table

To restart a node, use **Restart…** in its row. SparkBox reconnects automatically using the saved connection, then checks its new boot, GPU, fabric and services. The task opens so you can follow it; return to Updates for refreshed package and restart status. A restart stops work on that node and cannot be undone once sent. See [Restart a node](help:restart-a-node) before confirming.

One row for each node. A red row couldn't be read, and an amber row is waiting for a restart. Move the pointer over a cell for more. A narrow window hides some columns, so make it wider to see them all.

| Column | What it shows |
|---|---|
| **Node** | The node |
| **Upgrade** | Preview and install the available routine packages on this row’s node. Refreshes its status and makes a new dry run before asking you to confirm. Held and GPU-stack packages stay unchanged. A dimmed button explains why it cannot start |
| **Pending** | How many packages can be upgraded. "couldn't read" means the check failed |
| **Security** | How many of those are security fixes, from an archive whose name ends in `-security` |
| **Needs care** | How many belong to the GPU stack: the kernel, NVIDIA, CUDA, Docker, network packages and the DGX OS updater. SparkBox won't move these on one node |
| **NVIDIA driver** | The driver it runs, any update, and a **differs** badge if it isn't the same as most nodes |
| **Kernel** | The kernel it runs |
| **Restart** | **Restart…** opens the checks and confirmation for this row’s node. **needed** names a pending restart; **unknown** means no package reading. **View progress** and **View restart result** open its task |
| **DGX OS** | The last DGX OS update applied, and "update pending" if one is waiting. SparkBox doesn't install these |
| **Firmware** | How many firmware updates wait, as fwupd reports them, or **none**. **needs sudo** means it can only be read as root, and **Check all nodes** reads it that way |
| **Ollama** | The version installed and the latest one, or "couldn't check". SparkBox asks GitHub for the latest from this Mac, never from a node |
| **Snaps** | How many snaps have an update waiting. A snap is a self-contained Ubuntu app package |
| **Auto-updates** | Whether the node's automatic security updates are **on** or **off** |
| **Held** | The packages held at their version |
| **Checked** | When SparkBox last checked the node |

With **Demo** data, **DGX OS**, **Firmware**, **Ollama** and **Snaps** say "Available with Live data". The **Pending** and package figures are simulated.

SparkBox reads these numbers in the background, whether or not this window is open. Every 60 seconds it reads a few small time stamps on each connected node. It runs the full check when one of those changes, when a node reconnects, and at least every 6 hours. It saves the results on this Mac, so after a restart of SparkBox you see the last results with their age.

### Engine image updates

The **Engine images** column and sidebar count show newer tags or moved tags for supported vLLM repositories. SparkBox checks public registries from this Mac. A moved tag requires a comparable saved registry digest; a local image ID alone cannot establish it. Newer Docker Hub tags must offer linux/arm64. Missing data, rate limits and unrecognized tag formats stay unknown. Successful checks are cached for 6 hours, failures for 30 minutes. Checking never pulls or changes an image.

### Update all nodes one by one

Open **Update all nodes one by one…**. Choose a **Canary**, the first node to try. The default is `cell2` when selected, otherwise the node with the fewest dependent models. Select **Restart when done** only for nodes you want restarted; no restart is selected by default. **Preview routine updates** shows the order and exact package versions without changing a node.

Review **Start reviewed plan…** and confirm. SparkBox rechecks the package plan before admission and stops at the first failure. Routine updates keep models serving. Before a selected restart, it stops every member of affected owned loads, retains their settings and keys, checks the next boot, restarts the node and checks its new boot, kernel, GB10, driver, fabric links, Docker and systemd. A spanning load is restored after its last selected member. An external server that SparkBox cannot restore blocks that restart plan.

Follow the parent and child tasks in **Tasks**. Cancel stops before the next step; an admitted installer is left to finish. A saved plan waits for explicit **Resume…** after SparkBox reopens. It follows an existing package job and never blindly repeats a saved reboot. Review a failed child before ending that plan and making a new preview. **Restore stopped models and end plan…** waits for admitted work to finish, restores models still kept by the plan, and abandons remaining steps. Models the owner unloaded are never relaunched. Restart nodes stay reserved until the plan ends. These flows have local tests; real package and reboot acceptance remains pending.

### The package list

Select a node, and its pending packages show below the table, GPU-stack ones first, then security fixes. Select packages to upgrade, hold or simulate only those. With none selected, **Simulate** and **Upgrade node** cover everything pending.

| Column | What it shows |
|---|---|
| **Package** | Its name, with badges: a kind such as **kernel**, **NVIDIA**, **Docker** or **network**; **security**; **held** |
| **From → to** | The version installed and the new one |
| **From archive** | Where it comes from, such as `noble-updates` |

A held package is dimmed. A node with nothing pending says "cell2 is up to date", with when its lists were checked.

### The panel at the lower right

This panel shows one of three things.

**With nothing running,** it says how the last **Check all nodes** went on that node, and what's waiting for a restart.

**After Simulate,** it shows **Dry run on cell2** with a **nothing changed** badge:

- "Upgrade refused:" and the reason, if the plan can't run.
- **Upgrade installs exactly this**, the plan with each version. An empty plan reads "Nothing to install.", and **Upgrade node** is dimmed.
- **The upgrade runs, as root**, the exact command.
- **Dry run that showed it**, the command that made this plan. Its `-s` tells apt to plan without doing.
- **Upgrades**, **New**, **Removed** and **Held back**, the four counts.
- **Deferred by phasing (not installed this time)**, packages Ubuntu is holding back this time. See [Phased updates](help:packages-and-updates#phased-updates).
- **apt-get output**, apt's whole reply.

**During an upgrade,** it shows **Upgrading cell2**, with a progress bar and the stage, such as "Unpacking 3 of 7 · libbrotli1". Below that are the name of the background unit, the path of its log on the node, and apt's output as it arrives.

**Stop before it installs** stops apt at once, while it's still downloading and nothing has been installed. If that doesn't work, a notice in the panel says why and the button stays, so you can click it again. If the link to the node drops, SparkBox tries again when the node is back. Once apt starts installing, the button reads **Can't stop safely now**, because stopping would leave packages half-configured.

If SparkBox loses the node, the title reads "Lost cell2 while it was upgrading". Apt may still be running there, and SparkBox follows it again when the node is back.

After an upgrade, it shows the package list saved before it and the result of a fresh package check. **No package updates remain** appears only when that check succeeds and finds none. Otherwise it lists the remaining packages, or says the remaining updates are unknown if the check could not complete. Held and GPU-stack packages can still be pending. This check covers apt packages using the current repository lists; it does not update firmware, snaps or Ollama. SparkBox runs `dpkg --audit` and `apt-get check`, which look for broken or half-installed packages, and `needrestart`, which lists services still on old files.

### Still using old libraries

After an upgrade, `needrestart` lists running services that still use the old version of something. They keep working, but they get the fix only after a restart. Each one has a **Restart** button. "No services are left on old libraries." means there's nothing to do. "A newer kernel is installed (7.0.0-1020-nvidia); it runs after the next restart." means the node needs a restart.

This panel keeps the result from when installation ended. After you restart, use the node's current **Restart** column and its restart task to check the new boot. For a protected update, also require **Check system update** to pass; an old sentence in the installation result is not a request to restart again.

### Upgrade security only

This takes the pending security fixes that aren't GPU-stack or held. It runs its own dry run, then asks before it installs. It needs no pin. If there's nothing it may install, the button is dimmed and says "No security fixes SparkBox may install here".

### Hold and Release

**Hold** tells apt to leave the selected packages at their version. **Release** lets them move again. These work on the packages you select, one node at a time. They install nothing. They run `apt-mark hold` and `apt-mark unhold` as root. To hold the whole GPU stack, use **Pin the GPU stack…**. It reads every installed package with `dpkg-query`, then runs `apt-mark hold` on the GPU-stack ones.

## What it changes

Reading the counts, **Simulate** and **Filter packages** change nothing, and they work in look-only mode.

| Action | Changes on the node | Undo |
|---|---|---|
| **Check all nodes** | Refreshes the package lists | None needed |
| **Hold**, **Release** | Marks packages | The other one |
| **Pin the GPU stack…** | Marks the GPU stack as held | **Release the GPU-stack pin…** |
| **Upgrade** beside a node, **Upgrade node**, **Upgrade security only** | Installs new versions, runs as root, and may restart a service | There may be no way back. [Go back on one package](help:install-updates#go-back-on-one-package) says when you can try |
| **Restart** beside a service | Restarts that service | None needed |

An upgrade first saves every package's version to `/var/log/sparkbox` on the node, and to the evidence folder on this Mac. While it runs, the node's alerts are **Planned**. That lasts 30 minutes from the start, and SparkBox extends it each time it sees apt still running. If SparkBox loses the node, its alerts count again 30 minutes after it last saw apt running. It removes nothing, keeps config files you changed, and installs only what you were shown.

The reviewed plan upgrades routine packages one node at a time. GPU-stack updates remain unavailable; see [What's coming](help:whats-coming).

## Related How-Tos

- [See what updates are waiting](help:see-pending-updates)
- [Check for updates now](help:check-for-updates)
- [Simulate an update](help:simulate-an-update)
- [Install updates on a node](help:install-updates)
- [Pin the GPU stack](help:pin-the-gpu-stack)

![Updates, using synthetic Demo data](img/public-updates.jpg)
