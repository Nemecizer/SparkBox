---
id: sparkbox-window
title: The SparkBox window
type: feature
area: desk
summary: "A tour of the main window: sidebar, top bar, the desk where windows open, and the status bar."
keywords: [sidebar, main window, launcher, welcome, start, desk, banner]
verified: null
related: [top-bar-and-status-bar, windows-and-layouts, command-palette]
---

# The SparkBox window

## What it's for

SparkBox has one main window, and your tools open as smaller windows inside it. That lets you keep a node's charts, a terminal and a log side by side, the way you'd spread papers on a desk.

This page is a tour of the main window. Everything you do in SparkBox starts from one of its four parts.

## Where to find it

- **At start:** it opens when you start SparkBox.
- **Menu bar icon:** if you closed it, click the SparkBox icon in the macOS menu bar, then click **Open SparkBox**. Closing the window doesn't quit SparkBox. [The menu bar icon](help:menu-bar-icon) explains why.

## What's on the screen


| Part | Where | What it does |
|---|---|---|
| [Sidebar](#sidebar) | Left | Opens every window |
| [Top bar](#top-bar) | Top | Shows the cluster's state, Admin mode and the window tools |
| [Desk](#desk) | Center | Holds your open windows |
| [Status bar](#status-bar) | Bottom | Shows each device, tasks, alerts and how fresh the data is |

### Sidebar

The sidebar lists everything you can open. Click a row to open that window, or to bring it forward if it's already open.

| Group | Rows |
|---|---|
| **Overview** | **Dashboard**, **Topology**, **Cluster metrics**, **Alerts** |
| **Nodes** | One row for each node, such as `cell1` |
| **Workloads** | **Workloads**, **Images**, **Containers**, **Services**, **Models**, **Endpoints**, **Ollama** |
| **AI** | **Chat**, **Server**, **Providers** |
| **Network** | **VPN**, one row for each switch and the gateway, and **Fabric** |
| **Tools** | **Files**, **Terminal**, **Fleet Command**, **Activity log**, **Updates**, **Tasks** |
| **Coming soon** | Planned features. It shows only when [Show upcoming features](help:settings-general#features) is on |

**Help** and **Settings** stay at the bottom of the sidebar. The list above them scrolls, and its scrollbar shows only while the pointer is over the sidebar. At the default window size, **Updates** and **Tasks**, the last rows of **Tools**, start out of sight. If you don't see a row, scroll the sidebar or type in **Filter**.

Rows tell you things without being opened:

- **On the right of a row.** A node shows its GPU use, such as "GPU 27%", or a word such as **Offline** in red. A node that's in trouble moves to the top of **Nodes**, so it can't scroll out of sight.
- **Badges.** **Alerts** shows how many [incidents](help:alerts-and-incidents) count now. Related alerts count once, and known issues, the ones you've silenced or acknowledged, aren't counted. **Services** shows "1 failed" when a service has failed. **Containers** shows how many exited with an error in the last 24 hours. With Live data, **Updates** shows how many security updates wait.
- **A small dot.** A window is open for that row, but it's behind another one.
- **A small shape on the icon.** It's the node's state. [The top bar and status bar](help:top-bar-and-status-bar#status-bar) shows what each shape means.

You can also:

- Type in **Filter**, at the top, to narrow the list. It matches more than the labels, so `docker` finds **Containers**. Press <kbd>Return</kbd> to open the first match, or <kbd>Esc</kbd> to clear the filter.
- Move the pointer over a group's name to show a small arrow, and click it to fold that group.
- Right-click a row for its menu. A node's menu has **Open**, **Terminal**, **Files**, **Containers**, **Services**, **Restart…**, **Shut down…** and ways to copy its address.
- Press <kbd>↑</kbd> and <kbd>↓</kbd> to move between rows once one has focus.
- Press <kbd>⌃⌘S</kbd> to hide or show the whole sidebar.

### Top bar

The top bar shows whether you're on **Demo** or **Live** data and whether SparkBox can change anything. It also sums up the cluster in one line. On the right are **Search**, **Tile**, **Windows** and **Close all**. [The top bar and status bar](help:top-bar-and-status-bar) explains each item.

### Desk

The **desk** is the area in the middle, where your windows open. [Windows and layouts](help:windows-and-layouts) explains how they behave.

When no windows are open, the desk shows a start page:

- A line with the cluster's state, such as "4/4 nodes online · 4 GPUs serving · 1 warning · VPN off".
- Six tiles for the main windows, with their keys, <kbd>⌘1</kbd> to <kbd>⌘6</kbd>.
- **Restore last layout**, which brings back the windows you last had open.
- **Recently closed**, with a button for each window you closed. **+2** lists the ones that don't fit.
- Four reminders of useful keys.


SparkBox shows short messages at the bottom right of the desk. Up to three show at once, and **2 more** opens the rest. A success or a note goes away after 4 seconds. An error stays until you close it. Moving the pointer over a message keeps it on screen.

Some problems show as a banner across the top of the desk:

| Banner | What it means |
|---|---|
| "Can't reach the SparkBox core: …" | The part of SparkBox that does the work hasn't answered. Click **Retry** |
| "Couldn't read inventory.json. SparkBox started with defaults and kept the old file beside it." | A settings file couldn't be read. [Restore SparkBox from a backup](help:restore-sparkbox#if-something-goes-wrong) explains what to do |
| A fence banner | Applies to the test build only. See [The Fence](help:the-fence) |

### Status bar

The status bar runs along the bottom. It has a chip for each node, switch and gateway, then tasks, alerts and how old the data is. Click a chip to open that device. [The top bar and status bar](help:top-bar-and-status-bar#status-bar) explains each one.

### Info icons

A small circled **i** beside a title or a field opens a **Help tip**: a short explanation with links to the full guide. Every window has one in its title bar. [Find your way around Help](help:using-help) explains how Help works.

### Welcome

**Welcome to SparkBox** opens the first time you start SparkBox. It offers **Explore the demo**, **Connect my Sparks**, **Getting Started** and **Keyboard shortcuts**. With Live data, **Explore the demo** reads **Look around**. It opens the same two windows, **Dashboard** and **Topology**, side by side. **Not now** closes the screen. To see it again, choose **Help › Show Welcome** in the macOS menu bar.

### Zoom

**View › Zoom In** and **View › Zoom Out** make everything in the window bigger or smaller, from 80% to 160%. **View › Actual Size** goes back to 100%. To change only the text, use **Text size** in [Settings: Appearance](help:settings-appearance).

## What it changes

The main window changes nothing on your nodes by itself. The commands you run from it can, and each one says so.

SparkBox remembers these on this Mac: the window's size and place, whether the sidebar is hidden, which groups are folded, and the zoom. You can go back at any time by changing them again.

## Related How-Tos

- [Take a tour of the Demo](help:tour-the-demo)
- [Arrange your windows](help:arrange-your-windows)
- [Connect your cluster](help:connect-your-cluster)
- [Read the Dashboard](help:read-the-dashboard)
