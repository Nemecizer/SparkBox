---
id: collect-diagnostics
title: Collect diagnostics
type: howto
area: ops
summary: Run the Doctor report or export SparkBox's own diagnostics when something isn't working.
keywords: [doctor, report, diagnostics, support, logs, send logs, send logs to support, share logs]
verified: false
related: [settings-diagnostics, troubleshooting]
---

# Collect diagnostics

To record what your nodes and SparkBox report when something isn't working, make a Doctor report or export diagnostics.

The **Doctor report** asks your nodes what they see. **Export diagnostics** describes SparkBox itself.

## Before you start

- **Doctor report** works with **Live** data only, and not in look-only mode, because it runs checks on every node. It takes about ten seconds.
- **Export diagnostics** works with Demo or Live data. It doesn't work in look-only mode, because it writes a file to Downloads.
- Neither one needs Admin mode. If Admin mode is unlocked, the **Doctor report** also reads the node's SSH settings with sudo.
- SparkBox's own passwords and keys never go in either file. Anything else in **Export diagnostics** that looks like a secret is replaced with `‹redacted›`, but SparkBox can't catch everything. Both files hold your node names, addresses and settings, so skim them before you share them.
- The **Doctor report** isn't the **Doctor** tab in **Fabric**, which tests the fabric. [Fabric: Doctor tab](help:fabric-doctor) covers that one.

## Steps

### Run the Doctor report

1. At the bottom of the sidebar, click **Settings**, then click **Diagnostics**. [Open Diagnostics settings](open:settings?section=diagnostics) You can also choose **Open Diagnostics…** from the **Help** menu.
2. In the **Doctor report** group, click **Run doctor**. The button reads **Collecting…**.
3. Wait for **Saved to** to appear, with the path of the file.
4. Click **Show in Finder**. Finder opens your Downloads folder with the report selected.

### Export diagnostics

1. At the bottom of the sidebar, click **Settings**, then click **Security & privacy**. [Open Security settings](open:settings?section=security)
2. In the **Diagnostics** group, click **Export…**. SparkBox saves the file to your Downloads folder and shows it in Finder.

## What you should see

- A file called `SparkBox-doctor-` with a number. It has one part for each node: its OS, `nvidia-smi`, memory, disks, network, Docker, some SSH settings, apt, `dgx-release` and the last reading SparkBox received.
- A file called `SparkBox-diagnostics-` with the date and time. It has SparkBox's version, settings, inventory, each node's status, recent tasks and activity, and its own log.

SparkBox has no support desk, so these files are for you, or for whoever is helping you.

## Why it works

The **Doctor report** logs in to each node and runs a fixed list of read-only commands. Problems can then be fixed from what the node really says. **Export diagnostics** gathers what SparkBox knows on this Mac and replaces anything that looks like a secret. SparkBox's passwords and keys live only in the Keychain, so they can't end up in either file. A secret you typed into a command is different: it's in your recent activity, and the export hides it only if it looks like one.

## If something goes wrong

- **Run doctor is dimmed, with "Available with Live data: it reads the real nodes".** Switch to **Live** data. See [Switch between Demo and Live](help:switch-between-demo-and-live).
- **Run doctor is dimmed, with "Not in look-only mode: it runs checks on every node".** Turn off look-only mode. See [Use look-only mode](help:use-look-only-mode).
- **The report says a node couldn't be reached.** That's useful to know. Look at the node's section, then see [Connection and login messages](help:errors-connection).

## Next steps

- [Troubleshooting](help:troubleshooting)
- [Capture evidence](help:capture-evidence)
