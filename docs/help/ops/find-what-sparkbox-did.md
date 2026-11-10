---
id: find-what-sparkbox-did
title: Find out what SparkBox did
type: howto
area: ops
summary: Look up any command SparkBox ran, when and why, and export the log.
keywords: [audit, history, log, what happened, export]
verified: false
related: [activity-log, settings-security]
---

# Find out what SparkBox did

To see every command SparkBox ran on a node and every change it made on this Mac, open the **Activity log**.

## Before you start

- You don't need Admin mode. Looking works in look-only mode. Exporting doesn't, because it writes a file.
- With **Demo** data, each row is the command SparkBox would have run. Demo and Live have separate logs.
- SparkBox keeps entries for 90 days unless you change **Keep entries for** in [Settings › Security & privacy](open:settings?section=security).

## Steps

1. In the sidebar, under **Tools**, click **Activity log**. [Open Activity log](open:activity) The **Activity log** window opens on **Changes**, for the last 7 days.
2. To see reads and SparkBox's own checks too, click **Everything**.
3. Choose how far back to look: **Today**, **7 days** or **All**.
4. To look at one node, choose it in the **All nodes** list. To look at changes on this Mac, choose **This Mac**.
5. To find something, type part of it in **Filter commands**, such as `restart` or a service's name.
6. To see only what went wrong, turn on **Failed only**.
7. Click a row. A panel at the right shows when it ran, where, who asked, how it ended, the exact command and its output.
8. If a row has an arrow at its left, click the arrow to see the commands inside it. One action, such as restarting 3 containers, is one row with one command each.


### Export the log

9. In the toolbar, click **Export**.
10. Choose **Export as CSV…** or **Export as JSON…**. SparkBox saves the period you're looking at to your Downloads folder, and shows it in Finder.

## What you should see

Each row gives **Time**, **Node**, **What**, **Result**, how long it **Took** and its **Source**. **Result** is **ok**, **ok · warning**, **failed**, **didn't finish** or **stopped**. **Source** says who asked: **You**, **Menu bar** or **SparkBox**.

The exported file is called `SparkBox-activity-` with the date and time.

## Why it works

SparkBox writes each command down before it sends it, and again when it ends. So a command that never reported back still shows, as **didn't finish**, and it may or may not have run on the node. The log is a file in SparkBox's data folder, readable only by your Mac account. SparkBox's own passwords and keys never appear. Anything else that looks like a secret is replaced with `‹redacted›` first, but SparkBox can't catch everything, so never type a secret into a command.

## If something goes wrong

- **"Nothing changed on the cluster or this Mac in the last 7 days."** Nothing changed in that period. Click **Everything**, or **All**.
- **A row says "didn't finish".** SparkBox quit or lost the connection before the command reported back. Check the node to see whether it ran.
- **An old entry is missing.** It may be older than **Keep entries for**. The window also loads only the newest entries, so use **Export** with **All** to search further back.
- **Export is refused.** It writes to Downloads, so it's held in look-only mode. [Use look-only mode](help:use-look-only-mode) shows how to turn that off.

## Next steps

- [Activity log](help:activity-log), to see every part of the window
- [Capture evidence](help:capture-evidence)
- [Tasks](help:tasks)
