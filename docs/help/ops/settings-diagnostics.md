---
id: settings-diagnostics
title: "Settings: Diagnostics"
type: feature
area: settings
summary: The Doctor report, evidence captures, and SparkBox's list of trusted host keys.
keywords: [doctor, evidence, host keys, known hosts]
verified: false
related: [capture-evidence, collect-diagnostics]
---

# Settings: Diagnostics

## What it's for

**Settings › Diagnostics** collects facts about your cluster. Use it to get a report when something isn't working, or to record your nodes before and after a big change. You can also reset SparkBox's trusted host keys after you reinstall a node.

Validate the results on your own equipment before relying on them. The Doctor report has not been run on your cluster.

## Where to find it

- **Sidebar:** at the bottom, click **Settings**, then click **Diagnostics**.
- **Keyboard:** press <kbd>⌘,</kbd>, then click **Diagnostics**.
- **Menu bar:** choose **Help › Open Diagnostics…**.
- **Command palette:** press <kbd>⌘K</kbd>, type `Open Diagnostics`, and press <kbd>Return</kbd>.
- **Link:** [Open Diagnostics settings](open:settings?section=diagnostics)

## What's on the screen


### Doctor report

**Run doctor** collects what each node reports into a text file in your Downloads folder, so problems can be fixed from real output. It takes about ten seconds. When it's done, **Saved to** shows the path, and **Show in Finder** shows the file.

The report covers each node's OS, `nvidia-smi`, memory, disks, network and Docker. It also has SSH's limits, the machine ID, Docker's image store, `GRUB_DEFAULT`, the firewall, apt, `dgx-release` and the last reading SparkBox received. With Admin mode unlocked, it also reads the SSH server's own settings with sudo.

The report contains no passwords, because it runs a fixed list of commands that don't print them. Skim it before you share it, because it has your node names and addresses.

It needs **Live** data, because it reads the real nodes. It's off in look-only mode, because it runs checks on every node.

This is not the **Doctor** tab in **Fabric**, which tests the fabric.

### Evidence

**Capture evidence** reads each connected node and saves what it finds, with a list of what's different from the capture before it. The files go in `~/Library/Logs/SparkBox/evidence`.

| Field | What it's for |
|---|---|
| **Group** | A short name for one piece of work. Captures in the same group are compared with each other. It starts as `general`. Type your own, such as `november-update` |
| **Label** | What this capture marks, such as `before-update` or `after-reboot`. It becomes part of the folder name |
| **Capture** | **Capture evidence** starts it. While it runs, the hint reads "Reading every node; this can take a few minutes" |

After a capture, **Saved** shows the folder, with **Show in Finder**. Its summary reads like "4 nodes · 3 steps failed · cell3 not connected". **Problems** lists anything it couldn't save. The last five captures are listed below.

It reads containers, images, failed services, disk, models, apt, installed packages, the GPU, addresses, listening ports, memory and `/dev/shm`. Anything that looks like a secret is replaced with `‹redacted›`, but SparkBox can't catch everything. It needs **Live** data, and it works in look-only mode because it only reads.

### SSH host keys

A **host key** proves a node is the same node as last time. SparkBox connects to a node only when you've trusted its key, or when this Mac's own `~/.ssh/known_hosts` file already holds that exact key. It refuses a node whose key changes.

**Forget host keys…** forgets every key SparkBox saved. Nothing is trusted by itself afterward. On its next connection, each node waits for you to trust it again, unless `~/.ssh/known_hosts` already holds that exact key. Do this only after you reinstall a node. [Trust a node's host key](help:trust-a-host-key) is the better way to handle one node.

## What it changes

| Action | What it changes | Needs | Undo |
|---|---|---|---|
| **Run doctor** | Writes a file in Downloads. Nothing changes on the nodes | Live data, not look-only | Delete the file |
| **Capture evidence** | Writes files under `~/Library/Logs/SparkBox/evidence`. Nothing changes on the nodes | Live data | Delete the folder |
| **Forget host keys…** | Forgets every saved host key on this Mac | Admin mode, with Touch ID or your Mac password | None. Trust each node again, which saves its key |

## Related How-Tos

- [Capture evidence](help:capture-evidence)
- [Collect diagnostics](help:collect-diagnostics)
- [Trust a node's host key](help:trust-a-host-key)
