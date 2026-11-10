---
id: services
title: Services
type: feature
area: ops
summary: Background services on all nodes side by side, failed ones first, with status and journal.
keywords: [services, systemd, mask, journal, read logs, service log]
verified: false
related: [restart-a-service, services-and-processes]
---

# Services

## What it's for

**Services** shows the background services on all your nodes, side by side. Use it to spot a failed service, read why it failed, and start, stop, restart, enable or disable it.

A **service** is a program the node keeps running in the background, run by **systemd**. [Services and processes](help:services-and-processes) explains the idea. Validate the results on your own equipment before relying on them.

## Where to find it

- **Sidebar:** under **Workloads**, click **Services**. A badge such as `2 failed` shows there when a **systemd unit** has failed. It can count units this window doesn't list. See [Failed units that aren't services](#failed-units-that-arent-services). Or right-click a node and choose **Services**.
- **Command palette:** press <kbd>⌘K</kbd> and type the name of a failed service or unit, for **Restart** and **Show journal of**.
- **Link:** [Open Services](open:services)

## What's on the screen


### The toolbar

The toolbar shows names when the window is wide enough. In a narrower window it shows icons, in the order below, so this table lists each icon.

| Control | Icon | What it does |
|---|---|---|
| **All nodes** | A menu | Shows every node side by side, or one node |
| **Start** | A triangle | Acts on the selected service, on the nodes under **Apply to** |
| **Stop…** | A square in a circle | The same |
| **Restart…** | A curved arrow | The same |
| **Enable** | A power symbol | See [Enable and Disable](#enable-and-disable) |
| **Disable…** | A power symbol with a line through it | See [Enable and Disable](#enable-and-disable) |
| **More** | A small arrow | See [More](#more) |
| **Active** | A word | Services running on some node, or in the last 5 minutes |
| **Failed** | A word | Only failed services, with a count. Failed units of other kinds aren't listed. See [Failed units that aren't services](#failed-units-that-arent-services) |
| **All** | A word | Every installed service |
| **Filter services** | A text box | Narrows the list by name or description |

A dimmed button explains why when you move the pointer over it.

### The table

One row for each service, and one column for each node. Failed services come first, and the window opens on the first one.

| In a cell | Meaning |
|---|---|
| Checkmark | Running. Running services stay quiet on purpose |
| **idle** | Inactive because nothing needs it now: a timer starts it, it runs once, or its start condition isn't met. This is normal |
| **failed** | It stopped with an error. The color follows the alert: amber for a warning, red if the rule is set to critical |
| **masked** | Blocked. Nothing can start it until it's unmasked |
| **inactive** | Stopped. Amber if an enabled service that should run has stopped |
| **activating**, **deactivating**, **reloading** | In between: starting, stopping or reloading. These are neutral, not faults. `plymouth-quit-wait` stays **activating** on a Spark from boot on, which is normal, so it often tops the list |
| A dash | Not installed on that node |

Click a cell to see that service on that node. Right-click a row for the same actions as the toolbar. With one node chosen in **All nodes**, each cell also says whether it's `enabled` at boot.

Move the pointer over a cell to see its full state. A banner above the table says when services have failed, with **Show failed**.

### Failed units that aren't services

The sidebar's badge counts every failed **systemd unit** on every node, and not every unit is a service. Some are other kinds, such as `snap-thunderbird-1295.mount`, which attaches a snap's files to a folder. This window lists only services, so it can show nothing failed while the badge reads `1 failed`.

[Alerts](open:alerts) names the unit, such as "snap-thunderbird-1295.mount failed on gx10-0000". To read why it failed, press <kbd>⌘K</kbd>, type part of the unit's name, and choose **Show journal of**, such as **Show journal of snap-thunderbird-1295.mount on gx10-0000**. **Services** opens on that unit with its status and journal, though it still has no row in the table.

To clear the mark once you know why, open a [terminal](help:open-a-terminal) on that node and run `sudo systemctl reset-failed snap-thunderbird-1295.mount`, with the unit's name. That clears the mark and the badge. It doesn't start or fix anything.

### The status panel

Select a service, and a panel opens at the right, or below the table in a narrower window.

- **Apply to** picks the nodes an action runs on. See [Several nodes](#several-nodes).
- **State**, **Since** and **Main PID** say what it's doing and since when.
- **Result** says how it last ended. **Error** shows the line that says why it failed.
- **Memory**, **CPU time** and **Tasks** are what it's using.
- **Unit file** is where its settings live on the node.
- **systemctl status**, at the bottom, opens the raw status text.

### Journal

The **journal** is the node's log of what each service printed. This shows up to the last 25 lines. **error** and **warning** lines are marked. The panel scrolls to the line that matters: for a failed service, the first error since it last started.

Lines since your last action are highlighted. **Wrap** folds long lines instead of scrolling sideways. It's on when there's an error to read. To follow the journal as it grows, choose **More › Open journal in Terminal**.

### Enable and Disable

**Enable** makes a service start when the node boots. **Disable…** stops that. Neither one starts or stops it now: it keeps running until it's stopped.

### More

| Item | What it does |
|---|---|
| **Reload** | Asks a running service to read its settings again, without stopping |
| **Reset failed** | Clears a service's failed mark. It starts nothing |
| **Mask…** | Blocks a service, so nothing can start it, by hand or from another service |
| **Unmask** | Removes the block. It doesn't start the service |
| **Reload unit files** | Makes systemd read its settings files again. No service starts, stops or restarts. It needs **Live** data |
| **Open journal in Terminal** | Follows the journal in a Terminal tab |

### Several nodes

Under **Apply to**, click a node's name to add it or take it off. Actions run on one node at a time, in the order you added them, and stop at the first node where they fail. The nodes after it aren't tried.

## What it changes

Reading status and journals changes nothing. SparkBox reads each node every 10 seconds while the window is open, and the list of installed services every 5 minutes. These reads aren't in the activity log.

Every action below runs as root, needs Admin mode, and isn't available in look-only mode:

| Action | Asks first | Undo |
|---|---|---|
| **Start** | No | **Stop…** |
| **Stop…** | Yes, it says what stops | **Start** |
| **Restart…** | Yes | None needed |
| **Enable** | No | **Disable…** |
| **Disable…** | Yes | **Enable** |
| **Reload**, **Reset failed**, **Reload unit files** | No | None needed |
| **Mask…**, **Unmask** | Yes, type a name | The other one |

Some actions make you type a name, because you could lose the node or a model:

- stopping, disabling or masking `ssh`
- restarting, stopping or disabling the network services
- stopping or restarting Docker on a node that serves a model
- masking or unmasking any service

On one node you type the node's name. On several you type the service's short name.

## Related How-Tos

- [Restart, disable or enable a service](help:restart-a-service)
- [Respond to an alert](help:respond-to-an-alert)

![Services, using synthetic Demo data](img/public-services.jpg)
