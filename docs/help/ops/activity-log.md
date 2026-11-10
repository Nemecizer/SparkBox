---
id: activity-log
title: Activity log
type: feature
area: ops
summary: A record of every command SparkBox ran on a node and every change on this Mac.
keywords: [audit, changes, everything, export, refused]
verified: false
related: [find-what-sparkbox-did, settings-security]
---

# Activity log

## What it's for

The **Activity log** is a record of every command SparkBox runs on a node and every change it makes on this Mac. Use it to answer "what did SparkBox do, when, and did it work?".

## Where to find it

- **Sidebar:** under **Tools**, click **Activity log**.
- **Menu bar:** choose **Go › Activity Log** or **Help › Show Activity Log**.
- **Keyboard:** press <kbd>⌘8</kbd>.
- **Tasks window:** select a task and click **Commands**. The log opens on that task's commands alone, with **Show everything** to go back. [Tasks](help:tasks) explains that window.
- **Link:** [Open Activity log](open:activity)

## What's on the screen


### Changes and Everything

- **Changes** lists what changed a node or this Mac, Admin sessions and root shells. An **Admin session** is one unlock of Admin mode, from unlock to lock.
- **Everything** adds reads, shells, log views and SparkBox's own checks.

### The toolbar

| Control | What it does |
|---|---|
| **Failed only** | Shows only what failed or didn't finish |
| **Today**, **7 days**, **All** | How far back to look |
| **All nodes** | Limits the log to one node, or **This Mac** |
| **Filter commands** | Finds text in titles, commands, nodes and errors |
| **Export** | **Export as CSV…** or **Export as JSON…**. Saves the period shown to Downloads and shows it in Finder |
| **Show log file** | Shows the log file in Finder |

### The list

One row for each operation, newest first. An **operation** is one thing you asked for, such as restarting 3 containers, with all the commands it took.

| Column | What it shows |
|---|---|
| **Time** | How long ago. Move the pointer over it for the exact time |
| **Node** | Where it ran, such as "cell1–cell3" or "This Mac" |
| **What** | What it was, with a count such as "3/3 ok". Tags mark **destructive** commands and **root** |
| **Result** | **ok**, **ok · warning**, **failed**, **didn't finish**, **stopped** or **running** |
| **Took** | How long it ran |
| **Source** | Who asked: **You**, **You (palette)**, **Menu bar** or **SparkBox** |

An arrow at the left of a row opens its commands. Press <kbd>Return</kbd> to do the same. A background check is one row, such as "Check updates · 4 nodes · ok", and it doesn't open.

### The detail panel

Click a row, and a panel opens at the right with **When**, **Where**, **Who**, **Exit code**, **Took**, **Kind** and **Operation**. Below those are the **Command**, its **Output** and any **Errors**. **Open task** goes to the matching task. An exit code is the number a command reports when it ends: 0 means it worked.

**Kind** says what SparkBox makes of the entry:

| Kind | Meaning |
|---|---|
| "Only reads" | It looked at something |
| "Changes something" | It may have changed something on a node |
| "Can destroy data or take a node down" | The same class as Fleet's **destructive** |
| "Change on this Mac" | It changed a setting, a password or similar here |
| "SparkBox's own check" | A background read |
| "Shell session", "Root shell" | A terminal, from open to close. Your keystrokes are never recorded |
| "Admin mode" | An unlock, a lock or a confirmation |

### How it's kept

Every command is written down before it's sent and again when it ends. One that never reported back shows as **didn't finish**: SparkBox quit or lost the connection, so it may or may not have run on the node.

Commands are kept whole, with up to 64 KB of output for each. The log is a file in SparkBox's data folder, readable only by your Mac account. It starts a new file each day, or at 10 MB. Files older than **Keep entries for** in [Settings › Security & privacy](help:settings-security) are deleted. The default is 90 days.

SparkBox's own passwords and keys never appear. Sudo passwords and keys are sent privately, never on a command line. Anything else that looks like a secret is replaced with `‹redacted›`, but SparkBox can't catch everything, so never type a secret into a command.

The window shows the newest 5000 rows. **Export** reads the files on disk, so it covers the whole period you chose.

### Demo entries

With **Demo** data, a **Demo** mark shows in the footer. Nothing ran on a node: each row is the command SparkBox would have run. Demo and Live have separate logs.

## What it changes

Nothing on the cluster. **Export** writes a file to your Downloads folder, so it's held in look-only mode. The window needs no Admin mode. To change how long entries are kept, use [Settings › Security & privacy](help:settings-security).

## Related How-Tos

- [Find out what SparkBox did](help:find-what-sparkbox-did)
- [Capture evidence](help:capture-evidence)

![Activity log, using synthetic Demo data](img/public-activity-log.jpg)
