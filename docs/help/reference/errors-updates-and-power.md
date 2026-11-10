---
id: errors-updates-and-power
title: Update and restart messages
type: troubleshooting
area: troubleshooting
summary: Messages from Updates, the checks before a restart, and shutting down and waking nodes.
keywords: [gpu stack, upgrade refused, can't restart, wake packet, dpkg, apt, kernel, initrd, nvidia module, didn't come back, services, containers]
verified: null
related: [install-updates, restart-a-node, simulate-an-update, pin-the-gpu-stack, shut-down-and-wake-a-node]
---

# Update and restart messages

This page covers messages from **Updates**, from the checks SparkBox runs before a restart, and from shutting down and waking nodes. It ends with messages from **Services**, **Containers** and **Fleet Command**.

When SparkBox refuses an upgrade, the task adds "Refused before anything ran as root; nothing changed on the node."

## Checking for updates

### "Couldn't refresh the package lists on cell2"

**What it means.** `apt-get update` failed on the node. The task lists each warning and error from apt.

**What to do.** Read the lines in [Tasks](open:tasks). They usually name a website that couldn't be reached. Then click **Check all nodes** again.

### "cell2: the package lists are locked (apt-daily may be refreshing them); trying again in 30 s"

**What it means.** The node is refreshing its own lists, and only one program can do that at a time.

**What to do.** Nothing. SparkBox waits and tries again.

### "The connection to cell2 dropped before apt-get update finished; the lists may be only partly refreshed"

**What it means.** The link dropped during the refresh.

**What to do.** Check that the node is online, then click **Check all nodes** again.

### "The update check on cell2 gave an unexpected reply: …"

**What it means.** SparkBox couldn't read the node's answer.

**What to do.** Try again. If it repeats, [collect diagnostics](help:collect-diagnostics).

### "The update check hasn't started yet."

**What it means.** SparkBox opened a moment ago.

**What to do.** Wait a minute, and look again.

### "Couldn't ask GitHub for Ollama's latest release: …"

**What it means.** This check needs the internet. Your other update counts aren't affected.

**What to do.** Check this Mac's internet connection. It will try again by itself.

## Dry runs and upgrades

A **dry run** asks apt what an upgrade would do, without doing it. SparkBox then installs exactly that plan.

### "The generated GRUB entries do not all include NVIDIA's boot fix for the new kernel. Do not restart; review the package task."

**What it means.** The system packages may already be installed, but at least one startup choice is missing NVIDIA's required boot parameter. Recovery startup is checked too. This can happen when the vendor settings apply the fix only to normal startup.

**What to do.** Keep the node online and leave its system-update checkpoint intact. The boot settings need a reviewed repair with a saved copy of the previous configuration; keep the old kernel and recovery choices. Don't repeat the package installation or remove recovery entries to clear this message. After the repair, SparkBox must pass its existing checks before **Restart…**. Then wait for automatic reconnection and run **Check system update**. See [Protected system updates](help:updates#protected-system-updates).

### "A reviewed system package still has an update pending. Refresh and review it before continuing."

**What it means.** A package in the reviewed system set still appears in the pending list. A newer version may have arrived after installation, even when the installed versions and startup checks passed.

**What to do.** Read the pending versions in **Updates**. Keep the checkpoint and the next node blocked until the newer plan has been reviewed. The fixed November 2026 plan does not authorize newer kernels automatically.

### "Run the dry run first: SparkBox installs exactly what it showed."

**What it means.** An upgrade needs a plan you've seen.

**What to do.** Click **Simulate** first. See [Simulate an update](help:simulate-an-update).

### "That dry run was for another selection. Run it again."

**What it means.** You changed which packages are selected after the dry run, and its plan doesn't cover all of them.

**What to do.** Click **Simulate** again, read the new plan, then upgrade.

### "apt's plan for cell2 changed since the dry run you saw (apt-get -s: …). Look at the new dry run and start again."

**What it means.** New versions arrived after your dry run. SparkBox won't install anything you haven't seen.

**What to do.** Click **Simulate** again, read the new plan, then upgrade.

### "SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia"

**What it means.** The **GPU stack** is the kernel, NVIDIA driver, CUDA, Docker and network software, which must match across the cluster. SparkBox won't move them on one node, and the cluster-wide way to update them isn't built yet. "SparkBox can't upgrade GPU-stack packages yet: this plan removes docker-ce" is the same refusal for a plan that would remove one.

**What to do.** Pick other packages, or use **Upgrade security only**, which skips the GPU stack. To hold them still, see [Pin the GPU stack](help:pin-the-gpu-stack). See also [What's coming](help:whats-coming).

### "Upgrade everything needs the GPU stack pinned first: docker-ce, nvidia-driver-580-open aren't held on cell2"

**What it means.** An all-package upgrade would move the GPU stack unless it's pinned first.

**What to do.** Click **Pin the GPU stack…** for that node, then try again. See [Pin the GPU stack](help:pin-the-gpu-stack).

### "590 deadlocks CUDA graph capture on GB10: nvidia-driver-590-open"

**What it means.** The plan would install the 590 NVIDIA driver, which can freeze GPU work on your chip. SparkBox refuses it.

**What to do.** Don't install it. Pin the GPU stack so that it stays where it is.

### "This plan removes libnvidia-compute-580 (a 580 package)"

**What it means.** The plan would remove part of your working NVIDIA driver.

**What to do.** Don't run it. Check what else changed in the dry run.

### "This plan removes libfoo-doc. SparkBox's one-node upgrades only upgrade; run it in a terminal if you mean it."

**What it means.** SparkBox upgrades never remove packages.

**What to do.** Choose other packages. Removing a package can stop other software working, and SparkBox's checks don't apply in **Terminal**. Remove it yourself only if you know what the package does, and never a GPU-stack package.

### "An upgrade is already running on cell2."

**What it means.** Each node runs one upgrade at a time.

**What to do.** Wait for it to finish, in [Tasks](open:tasks).

### "cell2 is restarting; upgrade it once it's back."

**What it means.** The node is in planned work. You may also see:

- "cell2 has a restart scheduled; upgrade it once it's back."
- "cell2 is waking up; upgrade it once it's back."
- "cell2 is shutting down."
- "cell2 has a shutdown scheduled."

**What to do.** Wait, then upgrade.

### "cell2 is already up to date"

**What it means.** There's nothing to install. "No security fixes SparkBox may install on cell2 (it can't upgrade GPU-stack ones yet)" means there are none it may install now.

**What to do.** Nothing.

### "Can't install the security fixes on cell2"

**What it means.** **Upgrade security only** didn't start. "Nothing to install." means apt has no fixes to install, though the window's list showed some. That list can be out of date, for example after an upgrade from a terminal, so SparkBox reads the node again. "Upgrade refused: …" gives the reason apt's plan can't run.

**What to do.** With "Nothing to install.", nothing. Otherwise, follow the reason.

### "Couldn't start the upgrade on cell2: …"

**What it means.** The upgrade didn't begin, and apt didn't run. The reason follows.

**What to do.** Read the reason, then try again.

### "Couldn't start the upgrade on cell2: the start command's reply was lost, and no upgrade is running there."

**What it means.** The link dropped at the moment of starting. SparkBox looked, and found nothing running.

**What to do.** Try again.

## While an upgrade runs

### "Lost the connection to cell2 (…); the upgrade keeps running there. Waiting to reconnect…"

**What it means.** The upgrade runs on the node by itself, so the link dropping doesn't stop it.

**What to do.** Wait. SparkBox follows it again when the node is back.

### "SparkBox lost cell2 while it was upgrading; apt may still be running there. It looks again when cell2 is back."

**What it means.** The link to the node has been down for a while. The upgrade runs on the node by itself, so it may still be going. SparkBox keeps it as **lost** and picks it up again, even after SparkBox restarts.

**What to do.** Don't start another upgrade on this node. Restart and shut down are refused until it's clear.

### "Can't stop safely now: dpkg is installing packages. It will finish on its own."

**What it means.** You pressed **Cancel** after apt began to install. Stopping now could leave packages half-configured.

**What to do.** Let it finish. If apt then fails, the task reads **Failed**, because the install did run.

### "Couldn't stop it: …"

**What it means.** You pressed **Cancel**, or **Stop before it installs**, and apt didn't stop. The rest of the message is the reason. Apt keeps running. If the link to the node dropped, SparkBox tries again when the node is back, up to three tries in all.

**What to do.** Press it again while the button is there. Once it reads **Can't stop safely now**, let the upgrade finish.

### "Stopped before anything was installed on cell2"

**What it means.** **Cancel** worked. Nothing changed.

**What to do.** Nothing.

### "Look-only refused Cancel; the upgrade keeps running"

**What it means.** Look-only mode came on, so SparkBox can't send **Cancel**.

**What to do.** Turn off look-only mode if you still want to cancel.

### "No upgrade is running there."

**What it means.** It finished, or never started.

**What to do.** Check [Updates](open:updates).

### "The upgrade finished, but dpkg reports packages that aren't fully installed on cell2. Run sudo dpkg --configure -a there."

**What it means.** Some packages stopped partway, and the node isn't fully updated.

**What to do.** Don't restart yet. Read the list of packages in [Tasks](open:tasks), then do one of these:

- **It names an NVIDIA or kernel package.** Stop here. Don't install, remove or change those packages by hand. Make a [Doctor report](help:collect-diagnostics), then ask NVIDIA or your Spark's maker for help.
- **It doesn't.** Open a [terminal](help:open-a-terminal) on the node and run `sudo dpkg --configure -a`. Use a terminal, not **Fleet Command**. Its default **Time limit** of 60 seconds can stop the command partway and leave the same problem. To check the result, run `sudo dpkg --audit`. No output means it's clean.

"… and sudo dpkg --configure -a if it lists packages" is a variation of this message.

### "The upgrade finished, but SparkBox couldn't check the package database on cell2. Run sudo dpkg --audit there."

**What it means.** The upgrade ended, but the follow-up check didn't run.

**What to do.** In [Fleet Command](open:fleet), run `dpkg --audit` on that node, as root. No output means it's clean.

### "The upgrade finished, but apt-get check reports broken dependencies on cell2: …"

**What it means.** Packages on the node don't agree with each other. The message names the first problem.

**What to do.** Don't restart the node, and don't try to repair it with apt by hand, because a repair can remove packages. Make a [Doctor report](help:collect-diagnostics) and [capture evidence](help:capture-evidence), then ask NVIDIA or your Spark's maker for help.

### "Restart it to finish (kernel or driver)"

**What it means.** The upgrade worked, and a new kernel or driver takes effect at the next restart. This ends a line such as "2 packages installed on cell2. Restart it to finish (kernel or driver)".

**What to do.** Follow [Restart a node](help:restart-a-node) when it suits you.

### "1 service still uses old libraries: nginx.service"

**What it means.** A running service is using the old copy of updated software.

**What to do.** Click **Restart** beside it, or see [Restart, disable or enable a service](help:restart-a-service).

## The checks before a restart

SparkBox runs these checks before every restart. They protect against a node that wouldn't come back. Each message ends "Not restarted." The node hasn't been touched.

> [!DANGER]
> **Don't get around a refused check.** A terminal skips these checks, and a node that fails one may not start again. If that happens, only someone at the machine can recover it. Leave the node running. Don't install, remove or change NVIDIA or kernel packages by hand. Make a [Doctor report](help:collect-diagnostics), then ask NVIDIA or your Spark's maker for help.

### "No initrd for 7.0.0-1020-nvidia: the September 2026 panic. Not restarted."

**What it means.** The kernel the node would start has no initrd, a file it needs at startup. This is what stopped a node from starting in September 2026.

**What to do.** Don't restart. Don't install, remove or change NVIDIA or kernel packages by hand. Leave the node running, make a [Doctor report](help:collect-diagnostics) and [capture evidence](help:capture-evidence), then ask NVIDIA or your Spark's maker for help. See [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering#if-something-goes-wrong).

### "No NVIDIA module for 7.0.0-1020-nvidia: it would boot without its GPU. Not restarted."

**What it means.** The kernel has no matching NVIDIA driver.

**What to do.** Don't restart. Don't install, remove or change NVIDIA or kernel packages by hand. Leave the node running, make a [Doctor report](help:collect-diagnostics) and [capture evidence](help:capture-evidence), then ask NVIDIA or your Spark's maker for help. See [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering#if-something-goes-wrong).

### "7.0.0-1020-nvidia would boot driver 590.0 against userspace 580.178.04. Not restarted."

**What it means.** The kernel's driver and the driver's other half wouldn't match, so GPU programs would fail.

**What to do.** Don't restart. Don't install, remove or change NVIDIA or kernel packages by hand. Leave the node running, make a [Doctor report](help:collect-diagnostics) and [capture evidence](help:capture-evidence), then ask NVIDIA or your Spark's maker for help. See [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering#if-something-goes-wrong).

### "libnvidia-compute-580 isn't fully installed (dpkg status iU): the driver's userspace may be broken. Not restarted."

**What it means.** Part of the driver is half installed. You may also see:

- "libnvidia-compute-580 is removed (only its config is left): the driver's userspace isn't installed. Not restarted."
- "libnvidia-compute-580 isn't installed, so SparkBox can't tell which driver the GPU programs expect. Not restarted."

**What to do.** Don't restart. Don't install, remove or change NVIDIA or kernel packages by hand. Leave the node running, make a [Doctor report](help:collect-diagnostics) and [capture evidence](help:capture-evidence), then ask NVIDIA or your Spark's maker for help. See [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering#if-something-goes-wrong).

To add to your report, run `dpkg --audit` on that node in [Fleet Command](open:fleet), as root. It only reads. Copy what it lists, and don't act on the list yourself.

### "nvidia-dkms-580-open is installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted."

**What it means.** A package that's known to cause trouble is on the node. Your Sparks use prebuilt NVIDIA modules, not DKMS ones.

**What to do.** Don't restart, and don't remove this package yourself, because that can take the driver with it. Don't install or change other NVIDIA or kernel packages by hand either. Leave the node running, make a [Doctor report](help:collect-diagnostics) and [capture evidence](help:capture-evidence), then ask NVIDIA or your Spark's maker for help. See [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering#if-something-goes-wrong).

### "Can't tell which kernel boots next: …. Not restarted."

**What it means.** SparkBox couldn't read the link to the next kernel.

**What to do.** Try again. If it repeats, stop. Don't restart. Don't install, remove or change NVIDIA or kernel packages by hand. Leave the node running, make a [Doctor report](help:collect-diagnostics) and [capture evidence](help:capture-evidence), then ask NVIDIA or your Spark's maker for help. See [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering#if-something-goes-wrong).

### "The checks didn't finish (nothing came back for …). Not restarted."

**What it means.** The answer was cut off.

**What to do.** Try again.

### "Couldn't run the checks before the restart on cell2: …. Not restarted."

**What it means.** The checks themselves didn't run.

**What to do.** Check that the node is online, then try again.

### "Called off during the checks before the restart of cell2. Not restarted."

**What it means.** You stopped it.

**What to do.** Nothing.

## Restarting and shutting down

### "cell2 restarted, but systemd state is starting"

**What it means.** SparkBox reconnected and confirmed a new boot. Linux's service manager still reported startup in progress when the checks ran. This does not mean every check passed.

**What to do.** Leave the other nodes running while you inspect this one. In its [Terminal](open:terminal), `systemctl is-system-running`, `systemctl list-jobs --no-pager` and `systemctl --failed --no-legend --plain` only read its state. If startup keeps waiting, inspect its screen and [collect diagnostics](help:collect-diagnostics). The task keeps its original warning even if startup later finishes. **Retry…** starts another restart; it does not repeat only the checks.

If the waiting service is `plymouth-quit-wait.service`, the boot splash may not have handed the screen to the login manager. A running login manager alone does not prove that handoff finished. Repair the handoff with a reviewed configuration and a recovery copy; don't mask the waiting service, clear failed states or accept **starting** as healthy. Starting `plymouth-quit.service` can stop the login manager on systems where those services conflict. After a repair, verify a fresh restart and every health check before continuing to another node.

### "Wait for fresh readings and a boot id from cell2 before restarting it. Nothing was sent."

**What it means.** SparkBox cannot establish the boot to compare after a restart. No restart command was sent.

**What to do.** Wait for fresh monitoring readings, then try again. If they do not resume, see [Connection and login messages](help:errors-connection).

### "cell2 isn't connected, so it can't be told to restart."

**What it means.** SparkBox reaches a node over SSH, and it isn't connected.

**What to do.** See [A node shows Offline](help:troubleshooting#a-node-shows-offline). If the node has frozen, someone must restart it at the machine.

### "cell2 is installing packages (Upgrade cell2); restart it when that finishes."

**What it means.** Restarting now could break the update. "cell2 may still be installing packages (…): SparkBox lost it while it was upgrading, and follows it again once it's back; restart it when that finishes." is the same when SparkBox has lost sight of it.

**What to do.** Wait for it to finish, in [Tasks](open:tasks).

### "cell2 is already restarting."

**What it means.** One power command runs at a time. You may also see:

- "cell2 is already shutting down."
- "cell2 already has a restart scheduled."
- "cell2 already has a shutdown scheduled."
- "cell2 is being checked before a restart."
- "cell2 is already starting up."

**What to do.** Wait for it to finish.

### "Schedule it within the next hour."

**What it means.** A delay can be at most an hour.

**What to do.** Pick a shorter delay.

### "Couldn't call off the restart: cell2 still restarts at 14:05 (…)"

**What it means.** You tried to cancel a scheduled restart, and the node didn't accept it. Its checks may also have failed a minute before it: "Its checks failed a minute before it (…), but SparkBox couldn't call off the restart of cell2 (…), so it stayed due at 14:05".

**What to do.** In [Fleet Command](open:fleet), run `shutdown -c` on that node, as root.

### "The restart didn't happen: cell2 kept answering for a minute. Check it with Fleet Command or a terminal."

**What it means.** The node accepted the command but didn't go down.

**What to do.** Check the node's state, then try again.

### "cell2 hasn't come back after 5 minutes. It may be stuck in firmware or waiting at a console. SparkBox keeps trying to reconnect; it won't send another restart."

**What it means.** The task timed out. SparkBox keeps trying the saved connection without sending another restart. A later connection does not turn the timed-out task into a success.

**What to do.** Give it a few more minutes. If it stays away, go to the machine. A screen and keyboard will show what it's waiting for. Hold the power button only as a last resort: follow [Recover a frozen node](help:recover-a-frozen-node).

### "cell2 answers again on the boot it had before (its boot id didn't change), so it didn't restart."

**What it means.** The node never restarted. Something stopped it.

**What to do.** Look at [the node's boots](help:find-out-why-a-node-restarted), then try again.

### "cell2 answers again, but SparkBox can't tell it restarted: it reports no boot id."

**What it means.** The node is back, but SparkBox can't prove the restart happened.

**What to do.** Check its uptime in the node's **System** tab.

### "cell2 refused to restart: …"

**What it means.** The node answered with an error. The reason follows.

**What to do.** Read the reason. It usually concerns sudo. See [sudo](help:errors-connection#sudo).

### "cell2's agent didn't get the maintenance note: it may page (…)."

**What it means.** The always-on agent didn't hear about the planned work, so your phone may get an alert. This doesn't affect the restart.

**What to do.** Ignore that alert, or push the agents' settings again. See [Alert delivery and agent messages](help:errors-alerts-and-agent).

## Waking a node

Wake-on-LAN uses a special network packet to turn a node on. See [Shut down and wake a node](help:shut-down-and-wake-a-node).

### "SparkBox doesn't know cell2's MAC address yet. It learns it while the node is on; wake it by its power button this time."

**What it means.** The packet needs the node's network card address. SparkBox hasn't seen it yet.

**What to do.** Press the node's power button this once.

### "No node is online to send the wake packet, and this Mac isn't on cell2's LAN. Wake another node first, or use its power button."

**What it means.** The packet must start on the same network as the node. Nothing on that network can send it.

**What to do.** Get home, or start another node first.

### "cell2 didn't come online within 5 minutes. Wake-on-LAN may be off on its LAN port, or the packet didn't reach it."

**What it means.** The packet went out, and the node didn't start.

**What to do.** Press its power button, then follow [Turn on Wake-on-LAN](help:check-your-nodes#turn-on-wake-on-lan) for next time.

### "cell2 is already on."

**What it means.** There's nothing to wake.

**What to do.** Nothing.

## Services, containers and Fleet

### "That service name has characters SparkBox won't pass to a shell."

**What it means.** The name has characters that could change what a command does.

**What to do.** Check the name. "That container or service name has characters SparkBox won't pass to a shell." is the same.

### "…: the cluster user can't use Docker (…)."

**What it means.** Your user isn't allowed to use Docker on the node.

**What to do.** See ["Installed, but this user can't use it"](help:errors-connection#installed-but-this-user-cant-use-it).

### "glm53 is running; stop it before removing it."

**What it means.** SparkBox won't remove a running container.

**What to do.** Stop it first. See [Manage containers](help:manage-containers).

### "A test container is already on cell2; remove it first."

**What it means.** One test container runs at a time.

**What to do.** Remove it in [Containers](open:containers).

### "Process already exited (its PID is gone or now belongs to another program)."

**What it means.** The process had already ended, so SparkBox sent nothing. It checks first, so that it never stops the wrong program.

**What to do.** Nothing.

### "SparkBox won't signal process 1."

**What it means.** Process 1 is the system itself. "SparkBox won't change process 1." is the same.

**What to do.** Use [Restart a node](help:restart-a-node).

### "Priority (nice) goes from -20 (highest) to 19 (lowest)."

**What it means.** The number you typed is out of range.

**What to do.** Enter a number in that range.

### "Type a command first."

**What it means.** The command field in **Fleet Command** is empty. "Pick at least one node." is the same kind of reminder.

**What to do.** Type a command, and pick nodes.

### "Run the restart on its own: SparkBox checks the kernel the node boots next just before it restarts."

**What it means.** You mixed a restart with other commands.

**What to do.** Run the restart by itself, or use **Restart…**.

### "The canary failed, so the other nodes weren't touched"

**What it means.** **Canary first** tries one node. It failed, so the rest were spared.

**What to do.** Read that node's output. Fix the problem, then run it again.

### "cell2 didn't come back within 6 minutes, so the other nodes weren't touched."

**What it means.** A Fleet restart runs one node at a time, and stops if one doesn't return. You may also see:

- "cell2 didn't go down within 2 minutes, so the other nodes weren't touched."
- "SparkBox can't log in to cell2 over SSH any more (…), so the other nodes weren't touched."

**What to do.** Look at that node first. See [A node didn't come back after a restart](help:troubleshooting#a-node-didnt-come-back-after-a-restart).

### "Took longer than the time limit; stopped"

**What it means.** The command reached the **Time limit** you set.

**What to do.** Raise the limit if it needs longer.

### "Pick up to 24 hours, or until you end it."

**What it means.** Maintenance mode can last 24 hours at most, or until you end it.

**What to do.** Pick a shorter time.

### "“Restart cell2” can't be stopped part-way."

**What it means.** This task can't be canceled safely.

**What to do.** Let it finish.

### "SparkBox quit while this was running."

**What it means.** The task was running when SparkBox closed. "SparkBox quit while following this; it may still be running on cell2." means the work may go on at the node.

**What to do.** Check the node, or run it again.

### "Stopped unexpectedly."

**What it means.** A task ended with no result.

**What to do.** Try again. See [Collect diagnostics](help:collect-diagnostics).
