---
id: restart-a-node
title: Restart a node
type: howto
area: ops
summary: "Restart a node safely: see what goes down with it, pass the checks, and confirm it came back."
keywords: [restart, reboot, node, kernel, restart safely, without breaking anything]
verified: false
related: [shut-down-and-wake-a-node, use-maintenance-mode, node-window]
---

# Restart a node

To restart a node, click **Restart…** in its window or its Updates row, read what goes down with it, and confirm.

## Before you start

- The node must be online, so that SparkBox can tell it to restart. In the sidebar, it shows its GPU use, such as **GPU 28%**. If it has stopped answering, follow [Recover a frozen node](help:recover-a-frozen-node) instead.
- A restart stops everything on the node. Models it serves stop answering. Its containers stop, and only those with a restart policy come back by themselves. SparkBox says "usually 2–3 minutes" for the node to come back.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data the restart is simulated, and the checks before it say "Available with Live data". [Unlock Admin mode](help:unlock-admin-mode) shows how.

> [!DANGER]
> **A node that doesn't start again stays down until someone goes to it.** SparkBox checks first, but it can't fix a node that stops during startup. Restart only when someone can reach the node.

## Steps

1. In the sidebar, under **Nodes**, click the node, such as `cell2`. The node's window opens.
2. In the toolbar, click **Restart…**, the curved arrow. If the window is narrow, it's under the **More** button (**…**) at the end of the toolbar.
3. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. Wait a few seconds. With **Live** data, a message says "Checking cell2 before the restart…". Then a dialog named **Restart cell2?** opens.
5. Read the list. It names the models, endpoints, containers and open terminal tabs that go down with the node, and what brings each one back.
6. Read **Checks before the restart**. If a check fails, you get a different dialog. See [If a check refuses the restart](#if-a-check-refuses-the-restart).
7. Under **When**, choose **Now**, **In 1 minute** or **In 5 minutes**.
8. If the dialog offers a checkbox such as "Then restart llama-3.3-70b in order once cell2 is back", leave it on. It restarts that model afterward.
9. If you see "Type cell2 to confirm", type the node's name. SparkBox asks for this when the stakes are high: you're away on the VPN, the node is part of a model on several nodes, or SparkBox reaches the cluster through it.
10. Click **Restart**.
11. Watch the task "Restart cell2" in the status bar. The node shows **Restarting** in the sidebar. When it's back, the sidebar shows its GPU use again, such as **GPU 28%**. To follow each step, click the task, then **All tasks**, and select it.


In **Updates**, each node has a **Restart…** button. It runs the same checks and confirmation, then opens **Tasks** on that restart. The row offers **View progress** while it runs and a result link afterward. SparkBox refreshes the package and restart status after the task succeeds.

You can also start from the sidebar: right-click the node and choose **Restart…**. On that route, SparkBox asks to unlock after you click **Restart** in the dialog, not before.

To call off a scheduled restart, click **Cancel** on its task before its time comes. A restart set for **Now** can be canceled only until SparkBox has sent it.

## What you should see

For 10 seconds after the restart ends, the status bar names the task. [Tasks](open:tasks) keeps the result, such as "cell2 is back after 2m14s (offline 8s after the restart was sent, back on a new boot 2m06s later)". If another window is in front when it ends, a message says the same. If models were serving on the node, SparkBox waits for them too, and the step reads "Waiting for llama-3.3-70b to answer…".

SparkBox reconnects automatically using the saved address and login. You do not need to reconnect manually or unlock Admin mode again for monitoring. A fresh boot id proves the restart even when the brief offline interval falls between readings.

After reconnection, **Tasks** records checks of the kernel, GPU driver, both fabric links, Docker and system services. A warning means the node returned but a check needs attention. Reconnecting does not by itself reload a model; the confirmation explains any model follow-up.

Live restarts of `cell1`, `cell2`, `cell3` and `cell4` passed every check after their boot-setting and Plymouth startup repairs. Their latest restarts reached kernel 1025 and reconnected automatically in 53, 54, 54 and 53 seconds, respectively. GPU, driver, both fabric links, Docker and Linux startup were healthy. The idle Ollama service on `cell3` also answered its model-list check; nothing was loaded in it for an inference test. This proves those tested nodes and configurations. Scheduled restarts, interrupted connections and physical recovery startup still need their own checks.

## Why it works

Before it sends anything, SparkBox reads which kernel the node boots next and checks that it can start with its GPU. The kernel is the core of the operating system. These checks only read, and they run again right before the restart is sent. SparkBox marks the node as planned work, so nothing goes to your phone, then sends the restart as root. It counts the restart as done only when the node comes back with a new boot id, a number each node gets fresh at every start.

## Limits

- Each **Restart…** button targets one node. Wait for its result before restarting another. **Updates** also has a separate rolling maintenance plan; see [Updates](help:updates).
- SparkBox checks a scheduled restart when you schedule it. With **In 5 minutes**, it checks again a minute before its time. It doesn't check when the restart fires.
- Keep SparkBox open to follow the task and run its checks. If you quit, the node still restarts and SparkBox reconnects when reopened, but the interrupted task does not resume or prove its post-restart checks. A scheduled restart also loses its second check while SparkBox is closed.
- A node that's installing updates can't be restarted until they finish.

## If something goes wrong

- **"cell2 restarted, but systemd state is starting"** The node answered on a new boot, but Linux had not finished starting its services. Wait and inspect the node before restarting another. The task keeps its original warning; **Retry…** starts another restart, not another health check. See [Update and restart messages](help:errors-updates-and-power).
- **"cell2 hasn't come back after 5 minutes. It may be stuck in firmware or waiting at a console. SparkBox keeps trying to reconnect; it won't send another restart."** The task times out, but monitoring keeps trying to reconnect. A later connection does not change that task to success. Someone may need to go to the machine. Look at its screen. If it's frozen, follow [Recover a frozen node](help:recover-a-frozen-node). Its alerts count again once the expected time has passed.
- **"The restart didn't happen: cell2 kept answering for a minute. Check it with Fleet Command or a terminal."** The node didn't go down. Try `uptime` in [Fleet Command](help:run-a-command-on-every-node), then try again.
- **"cell2 answers again on the boot it had before (its boot id didn't change), so it didn't restart."** The node never restarted. Nothing was lost. Try again.
- **"cell2 is installing packages (Upgrade libbrotli1 on cell2); restart it when that finishes."** Wait for the upgrade to end in [Updates](open:updates).
- **"Wait for fresh readings and a boot id from cell2 before restarting it. Nothing was sent."** Wait for monitoring to resume before trying again.
- **"cell2 isn't connected, so it can't be told to restart."** The node is offline. See [Connection and login messages](help:errors-connection).
- **The task says "Restarted, but llama-3.3-70b didn't come back: …"** The node is fine, and the model isn't answering. See [Fix a load that failed](help:fix-a-load-that-failed).

For the other messages, see [Update and restart messages](help:errors-updates-and-power).

### If a check refuses the restart

The dialog is named **cell2 can't restart now**, and it has no **Restart** button. The restart is refused before anything is sent. These are the checks that can refuse:

| Check | What it looks for | Message |
|---|---|---|
| **Kernel that boots next** | SparkBox can tell which kernel starts next | "Can't tell which kernel boots next: /boot/vmlinuz isn't there." |
| **Its initrd** | The small start-up file the kernel needs is there and isn't empty | "No initrd for 7.0.0-1019-nvidia: the September 2026 panic." |
| **Its NVIDIA module** | The GPU driver part is built for that kernel | "No NVIDIA module for 7.0.0-1019-nvidia: it would boot without its GPU." |
| **NVIDIA userspace (libnvidia-compute-580)** | The other half of the GPU driver is installed | "libnvidia-compute-580 isn't installed, so SparkBox can't tell which driver the GPU programs expect." |
| **Module and userspace match** | Both halves are the same version | "7.0.0-1019-nvidia would boot driver 580.95.05 against userspace 580.178.04." |
| **No DKMS NVIDIA packages** | No package that rebuilds the driver at install time | "nvidia-dkms-580-open is installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic." |

"The September 2026 panic" is a real fault: in September 2026 some kernels were left without an initrd, and those machines stopped at startup. A DKMS NVIDIA package is what caused it.

Lines marked **(recorded)**, such as **GRUB_DEFAULT**, are notes. They never block a restart.

> [!DANGER]
> **Don't get around a refused check.** Typing `sudo reboot` in a terminal skips these checks, and the node may not start again. If that happens, only someone at the machine can recover it. Leave the node running. Don't install, remove or change NVIDIA or kernel packages by hand. Make a [Doctor report](help:collect-diagnostics), which records what the node says, then ask NVIDIA or your Spark's maker for help. Fleet Command runs the same checks.

## Next steps

- [Find out why a node restarted](help:find-out-why-a-node-restarted)
- [Recover a frozen node](help:recover-a-frozen-node)
- [Shut down and wake a node](help:shut-down-and-wake-a-node)
- [Use maintenance mode](help:use-maintenance-mode)
