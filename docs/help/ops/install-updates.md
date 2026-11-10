---
id: install-updates
title: Install updates on a node
type: howto
area: updates
summary: Upgrade one node safely, follow it, and deal with services and restarts afterward.
keywords: [upgrade, install, apt, security fixes, update node, update the cluster, update all nodes, update every node, which updates, undo an update, roll back, downgrade]
verified: false
related: [simulate-an-update, pin-the-gpu-stack, restart-a-node]
---

# Install updates on a node

Click **Upgrade** beside a node to preview and install its available routine packages. Check the result before continuing to the next node.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data the upgrade is simulated. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- Choose the node to try first. Pick one that serves no model: [Endpoints](open:endpoints) shows what runs where. If every node serves a model, choose a time when you can do without it.
- The node needs to reach the internet, and it needs disk space for the packages. A **package** is one piece of installable software.
- To see the newest fixes, follow [Check for updates now](help:check-for-updates) first.
- For a big change, [capture evidence](help:capture-evidence) first, so you can compare the node before and after.

> [!DANGER]
> **An upgrade may have no way back.** A package's own scripts may restart its service, and SparkBox has no undo for any of it. SparkBox saves a list of every package version first, so you can see what changed, but it doesn't keep the old packages. Going back by hand works only while apt still offers the old version, and Ubuntu often stops offering it once a newer fix is out. If a node must not change, stop here. [Go back on one package](#go-back-on-one-package) says what you can try.

For `cell2`'s saved 1024 update with newer protected packages pending, use [Continue `cell2` to kernel 1025](help:updates#continue-cell2-to-kernel-1025). This separate review preserves the original checkpoint and requires healthy startup before installation.

## Steps

If you're not sure what to install, install the security fixes. It's the smallest useful change, and it needs no setup. SparkBox leaves the **GPU stack** out of it: the kernel, NVIDIA, CUDA, Docker and network software a node needs to run models.

| Route | Use it when | What it needs |
|---|---|---|
| [Upgrade from the node’s row](#upgrade-from-the-nodes-row) | You want the available routine updates on that node | Review the automatic dry run |
| [Install the security fixes](#install-the-security-fixes) | You're not sure, or the sidebar shows a badge such as `3 security` | Nothing extra |
| [Choose the packages yourself](#choose-the-packages-yourself) | You want one or two named updates, such as a fix for a program you use | A dry run of those packages, with no GPU-stack package among them |
| [Upgrade everything pending](#upgrade-everything-pending) | You want to bring a node fully up to date at a quiet time | A pin on the node's GPU stack, then a dry run |

> [!NOTE]
> A new **Updates** window names its first three buttons and shows the rest as icons. In a narrower window every button is an icon. The steps below name each icon, and [Updates](help:updates#the-toolbar) lists them all.

### Install the reviewed protected updates

1. Unload models on the chosen node and stop its other containers and GPU jobs. For the direct 1025 plan, choose `cell1`, `cell3` or `cell4`; do not repeat `cell2`'s completed update. Healthy startup and the reviewed persistent recovery-boot setting must already be present; a refusal needs separate diagnosis and repair. Keep a current data backup and local keyboard/display access for boot recovery.
2. Open **Protected system updates…** in [Updates](open:updates), choose the node and click **Review system updates**. Unlock Admin if asked: reading the protected boot-menu file requires it. This reads a plan without installing.
3. Review the 16 upgrades and eight new kernel packages for the direct 1019-to-1025 path. The plan must include NVIDIA's `1.0-2` boot fix. A different plan is refused. Read [Protected system updates](help:updates#protected-system-updates) for the checks and recovery limits.
4. Above the package list, click **Update cell1 with proposed updates…**. The button names your chosen node. Unlock Admin mode if asked, and type the node's name in the confirmation. Install only if the review and recovery arrangements are acceptable.
5. Follow **View package task**. Wait for installation, package and boot checks to succeed. If it fails or loses the node, inspect the task; do not restart or begin another update.
6. Use **Restart…** beside that node in the table. Wait for it to reconnect, then click **Check system update** in the protected update panel. Unlock Admin again if needed to read the protected boot-menu file.
7. Continue to another node only after the check says that all reviewed versions and the new boot passed, and a model serving test succeeds. If Linux still says **starting**, wait and check again. A persistent warning needs diagnosis before continuing.

### Upgrade from the node's row

1. Open [Updates](open:updates). In the nodes table, click **Upgrade** beside the node you want to update. This button keeps its label even in a narrow window.
2. Unlock Admin mode if asked. SparkBox reads that node's package status again and previews the available packages that are neither held nor part of the GPU stack. It does not reuse another node's selection or dry run.
3. Review the exact versions and the list left pending. Click **Upgrade** to install, or **Cancel** to leave the node unchanged. If the plan is refused, read the reason; no installation starts.
4. Continue at [Follow it](#follow-it), then read the final package check under [After it ends](#after-it-ends).

If the button is dimmed, move the pointer over it for the reason. The node may be offline, already upgrading, have no updates, or have only held or GPU-stack packages left. Use **Check all nodes** first to refresh what the package repositories offer.

### Install the security fixes

1. In the sidebar, under **Tools**, click **Updates**. If you don't see it, scroll the sidebar or type in **Filter**. [Open Updates](open:updates) The **Updates** window opens.
2. In the table at the top, click the node you chose, such as `cell2`. Its pending packages appear below.
3. In the toolbar, click **Upgrade security only**, the shield with a checkmark. If it's dimmed, this node has no security fixes SparkBox may install, so go to [Do the other nodes](#do-the-other-nodes).

> [!NOTE]
> A dimmed **Upgrade security only** means no routine security fixes are eligible. Protected security updates can still need review. You can leave its other updates until a quiet time, as [A good routine](help:packages-and-updates#a-good-routine) explains, or install one you want with [Choose the packages yourself](#choose-the-packages-yourself).

4. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
5. Wait, for up to 2 minutes. SparkBox does a **dry run**, which means **apt**, the program that installs packages, plans the upgrade without doing it. A dialog named **Install 2 security fixes on cell2?** then opens, with your node and count.
6. Read the dialog. It lists the exact versions it installs, and says what SparkBox does around the upgrade.
7. Click **Install**.

### Follow it

8. Watch the panel named **Upgrading cell2** at the lower right. A bar and a line of words show the stage: "Downloading packages", "Unpacking 3 of 7 · libbrotli1", "Setting up", then "Finishing". Below them, apt's own output scrolls past.
9. Wait for it to end. You can use other windows while you wait. The upgrade is built to keep running on the node if SparkBox closes. That hasn't been proven on your cluster, so keep SparkBox open until it ends.
10. To stop while it's still downloading, click **Stop before it installs**. It stops apt at once, and nothing has been installed yet. If that doesn't work, a notice in the panel says why and the button stays, so you can click it again. If the link to the node drops, SparkBox tries again when the node is back. Once apt starts installing, this button reads **Can't stop safely now**, and the upgrade runs to its end.

### After it ends

11. Check that the panel title now reads **Upgraded cell2**, then read the package check below it. **No package updates remain** means a fresh check found none. If updates remain, SparkBox names them, including held and GPU-stack packages. If the check failed, the remaining updates are **unknown**; do not treat the node as up to date.
12. Read **Still using old libraries (needrestart)**. It lists running services that still use the old version of something. Each one has a **Restart** button. Click it for each service you want to restart now. SparkBox asks first if the restart could affect something.
13. Look at the **Restart** column for the node. If it says **needed**, or the panel says "A newer kernel is installed", the node needs a restart to finish. In the toolbar, click **Restart node…**, the curved arrow, then follow [Restart a node](help:restart-a-node).

### Do the other nodes

Repeat the steps one node at a time, or review a saved plan with [Update all nodes one by one](help:updates#update-all-nodes-one-by-one).

14. Check the first node before you go on. The sidebar should show its GPU use, such as **GPU 28%**. If it serves a model, [check that the model answers](help:tell-if-a-model-is-working).
15. In the **Updates** table, click the next node, and go back to step 3.
16. When every node is done, in the sidebar, under **Tools**, click **Fleet Command**. [Open Fleet Command](open:fleet)
17. Click **Versions**, at the left of the toolbar. Review the **Matching**, **Mismatched** and **Unknown** rows. Known differences are named; missing readings stay unknown and cannot confirm agreement.
18. Back in **Updates**, compare **Pending** and **Security** across the rows. If one node shows more than the others, click it to see what's left.

To compare package by package, run the **Updates** snippet in [Fleet Command](help:run-a-command-on-every-node). Nodes with the same list share one card, and a node with a different list has a **differs** badge.

### Choose the packages yourself

Use this route for a named update. Leave out anything with a badge such as **kernel**, **NVIDIA**, **Docker** or **network**: those belong to the GPU stack, and SparkBox refuses them.

1. In the **Updates** window, click the node.
2. In the package list, click a package. To select more, hold <kbd>⌘</kbd> and click.
3. In the toolbar, click **Simulate**, the flask. The **Dry run** panel opens. Read it, and check that it has no "Upgrade refused:" line. [Simulate an update](help:simulate-an-update) explains the panel.
4. In the toolbar, click **Upgrade 3**, the box with a checkmark. The number is how many packages you selected. If it's dimmed, move the pointer over it to see why.
5. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
6. A dialog named **Upgrade 3 packages on cell2?** opens, with your node and count. Read it.
7. Click **Upgrade**.
8. Continue at [Follow it](#follow-it).

### Upgrade everything pending

Use this route to bring one node fully up to date. The GPU stack must be held still first, so that the upgrade can't move it. Without that, the dry run says "Upgrade refused: Upgrade everything needs the GPU stack pinned first".

1. Follow [Pin the GPU stack](help:pin-the-gpu-stack) for this node.
2. In the **Updates** window, click the node. Make sure no package is selected.
3. In the toolbar, click **Simulate**, the flask. The **Dry run** panel opens. Read it, and check that it has no "Upgrade refused:" line.
4. In the toolbar, click **Upgrade node**, the box with a checkmark. If it's dimmed, move the pointer over it to see why.
5. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
6. A dialog named **Upgrade 7 packages on cell2?** opens, with your node and count. Read it.
7. Click **Upgrade**.
8. Continue at [Follow it](#follow-it).

## What you should see

The **Pending** count drops, and for 10 seconds after the upgrade ends the status bar names the task. [Tasks](open:tasks) keeps the result, such as "3 packages installed on cell2". If another window is in front when it ends, a message says the same. If a restart or service still needs attention, the result says so, for example "1 service still uses old libraries: nginx.service".

The panel also shows **Package list before it:** with the path of the saved list on the node. A copy is in `~/Library/Logs/SparkBox/evidence` on this Mac.

## Why it works

Right before it starts, SparkBox runs the same dry run again. If apt's plan has changed since you looked, it refuses and nothing runs, so what you read is what you get. While apt runs, the node's alerts are **Planned**, so nothing goes to your phone. When it ends, SparkBox looks for broken or half-installed packages, lists services still on old files, and reads the pending package list again. Nothing restarts by itself, except what a package's own scripts restart.

## Limits

- **Following an upgrade.** The upgrade runs on the node, in a background unit of its own, and writes to a log in `/var/log/sparkbox`. It's built so that apt keeps going if the connection drops or SparkBox quits, and SparkBox picks up the log when it's back. That hasn't been proven on your cluster.
- **Several nodes.** Use **Update all nodes one by one** in [Software Updates](help:updates#update-all-nodes-one-by-one) for a saved, canary-first routine plan. Review its preview and choose any restarts explicitly.
- **Routine Upgrade leaves the GPU stack unchanged.** Kernel, NVIDIA, CUDA, Docker and network packages need a separate supported review under **Protected system updates…**. NVIDIA control libraries such as `libxnvctrl0` also stay pending. See [What's coming](help:whats-coming).
- **The final check covers apt packages.** Held, protected or deferred packages can remain. It reads the current repository lists and does not install a second round automatically. Firmware, snaps and Ollama have separate status.
- **Nothing is removed.** A plan that would remove a package is refused.
- **DGX OS updates, firmware, snaps and Ollama are shown, not installed.** To see DGX OS updates, click **Open DGX Dashboard** in the toolbar, the square with an arrow. Look only. Don't install from that page: it skips SparkBox's GPU-stack guard and its restart checks, and a failed update can stop the node starting. See [Open the DGX Dashboard](help:open-the-dgx-dashboard).

## If something goes wrong

- **"Can't install the security fixes on cell2"** The dialog gives the reason after "Upgrade refused:". A fix would move or remove something SparkBox doesn't allow on one node, so nothing ran. Leave that fix pending. [Simulate an update](help:simulate-an-update#if-something-goes-wrong) explains each reason.
- **"Upgrade refused: Upgrade everything needs the GPU stack pinned first: …"** Follow [Pin the GPU stack](help:pin-the-gpu-stack) for this node, or use [Install the security fixes](#install-the-security-fixes) instead.
- **"Run the dry run first: SparkBox installs exactly what it showed."** There's no dry run for what's selected now. Click **Simulate**, the flask.
- **"That dry run was for another selection. Run it again."** You changed the selection after the dry run. Click **Simulate** again.
- **"apt's plan for cell2 changed since the dry run you saw: libfoo now 1.3 (was 1.2). Look at the new dry run and start again."** Something changed between your dry run and the start, so nothing ran. Click **Simulate** and read the new plan.
- **"An upgrade is already running on cell2."** Only one upgrade runs on a node at a time. Wait for it to end.
- **"cell2 is restarting; upgrade it once it's back."** Wait until the sidebar shows the node's GPU use again, such as **GPU 28%**.
- **"apt-get failed on cell2: E: …"** Apt stopped with an error. Its reason follows "E:". Open the task in [Tasks](open:tasks) to read the log.
- **"The upgrade finished, but dpkg reports packages that aren't fully installed on cell2. Run sudo dpkg --configure -a there."** Don't restart the node. Read the list of packages in [Tasks](open:tasks). If it names an NVIDIA or kernel package, stop and [get help first](help:errors-updates-and-power#the-upgrade-finished-but-dpkg-reports-packages-that-arent-fully-installed-on-cell2-run-sudo-dpkg-configure-a-there). Otherwise, open a [terminal](help:open-a-terminal) on the node and run that command.
- **"Lost cell2 while it was upgrading"** SparkBox lost the connection for more than 30 minutes. Apt may still be running on the node. SparkBox follows it again when the node is back, and after a restart of SparkBox. Don't restart the node until it's finished.

For the other messages, see [Update and restart messages](help:errors-updates-and-power).

### Go back on one package

Try this only when you know an upgrade has broken something. It happens outside SparkBox, so SparkBox's GPU-stack guard and its rule against removing packages don't apply. You decide what apt does. These steps have run only on a test system, with stand-in packages.

> [!DANGER]
> **Going back can remove software or stop a node starting, and you may not be able to reverse it.** Never do this for a GPU-stack package: kernel, NVIDIA, CUDA, Docker or network software must match across your nodes. If you don't know what a package does, stop here.

1. Open a [terminal](help:open-a-terminal) on the node.
2. Type `apt-cache policy libbrotli1`, with your package's name, then press <kbd>Return</kbd>. This only reads. It lists every version apt offers, and `***` marks the one installed.
3. Find the old version. The panel's **Package list before it:** line names a file. Type `grep libbrotli1` and that file's path, then press <kbd>Return</kbd>. The old version is the second word on the line.
4. Look for that version in the list from step 2. If it isn't there, stop. Apt can't go back, so leave the package as it is.
5. Type `sudo apt-get install libbrotli1=1.1.0-2build2`, with your package's name and old version, then press <kbd>Return</kbd>. If sudo asks for a password, type the cluster password. Nothing shows as you type.
6. Read apt's plan, and don't answer yet. "The following packages will be DOWNGRADED:" should list your package.
7. If the plan also says "The following packages will be REMOVED:", or it names any kernel, NVIDIA, CUDA, Docker or network package, type `n` and press <kbd>Return</kbd>. Apt says "Abort." and nothing changes. Stop here.
8. If the plan is only what you expected, type `y` and press <kbd>Return</kbd>.
9. Run `apt-cache policy libbrotli1` again. **Installed:** now shows the old version.
10. In the **Updates** window, wait about a minute. SparkBox sees the change, and the package shows as pending again. To keep it at this version, select it and click **Hold**.

If step 5 prints "E: Version '1.1.0-2build2' for 'libbrotli1' was not found", apt no longer offers that version, and nothing changed. There's no way back with apt, so leave the package as it is.

## Next steps

- [Restart a node](help:restart-a-node)
- [Restart, disable or enable a service](help:restart-a-service)
- [Find out what SparkBox did](help:find-what-sparkbox-did)
