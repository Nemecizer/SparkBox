---
id: check-for-updates
title: Check for updates now
type: howto
area: updates
summary: Refresh the package lists on your nodes, so that the counts are up to the minute.
keywords: [check all nodes, apt update, refresh]
verified: false
related: [see-pending-updates, simulate-an-update]
---

# Check for updates now

To get up-to-the-minute update counts, click **Check all nodes** in the **Updates** window.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data the check is simulated. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- The check needs Admin mode because it runs as **root**, the account that can do anything on a node, through **sudo**, the command that runs something as root.
- It only refreshes each node's package lists, the lists of available versions. It installs nothing, and it restarts nothing.
- Each node needs to reach the internet. If another update tool is using the lists on a node, SparkBox waits for it, so a check can take some minutes.

## Steps

1. In the sidebar, under **Tools**, click **Updates**. [Open Updates](open:updates) The **Updates** window opens.
2. In the toolbar, click **Check all nodes**, the two curved arrows. In a narrow window the toolbar shows icons without names: [Updates](help:updates#the-toolbar) lists them all.
3. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. Wait for the check to finish. The toolbar buttons are dimmed while it runs, and the status bar shows the task "Check for updates on 4 nodes". You can keep working in other windows.
5. When the buttons come back, look at the **Pending** and **Checked** columns. They show the new results.
6. Click a node's row. The panel at the lower right says how that node's check went.

To stop a check partway, click the task in the status bar, then click **Cancel**. Lists that were already refreshed stay refreshed.

## What you should see

For 10 seconds after the check ends, the status bar names the task. [Tasks](open:tasks) keeps the result, "Package lists are up to date". If another window is in front when it ends, a message says the same. The panel at the lower right says "Last check for updates 1 min ago: ok, no warnings." The **Checked** column shows a time of a few seconds or minutes ago.

The **Firmware** column may change too. Reading firmware updates needs root on some nodes, and this check reads them that way.

## Why it works

A node keeps a local copy of the list of software it could install. SparkBox tells apt on every node, as root and all at once, to download a fresh copy, then reads the counts again. If something else holds the lists, such as the node's own daily update job, apt waits up to 5 minutes. SparkBox then tries again after 15, 30 and 60 seconds. It keeps every warning and error apt prints, so you can read them afterward.

## If something goes wrong

- **"Couldn't refresh the package lists on cell2"** The check failed on that node. Open [Tasks](open:tasks), select the task and read its steps. The failed step has apt's own words.
- **"apt printed warnings on cell2 (1); they're in the log"** The check worked, but apt had something to say. A warning about one source usually doesn't stop the others. The node's lower right panel lists the warning.
- **"The connection to cell2 dropped before apt-get update finished; the lists may be only partly refreshed"** Check that the node is online, then run the check again.
- **"sudo on cell2 didn't accept the saved cluster password."** See [Connection and login messages](help:errors-connection#sudo-on-cell2-didnt-accept-the-saved-cluster-password).

For the other messages, see [Update and restart messages](help:errors-updates-and-power).

## Next steps

- [See what updates are waiting](help:see-pending-updates)
- [Simulate an update](help:simulate-an-update)
