---
id: menu-bar-icon
title: The menu bar icon
type: feature
area: desk
summary: SparkBox's icon at the top of your screen shows the cluster at a glance and keeps running when the window closes.
keywords: [menu bar, tray, close window, keep running, dock badge, quit, status item, app disappeared, window gone]
verified: null
related: [settings-general, top-bar-and-status-bar]
---

# The menu bar icon

## What it's for

SparkBox puts an icon at the right of the macOS menu bar, the bar at the very top of your screen. It shows how the cluster is doing while the SparkBox window is closed, and it gives you quick actions without opening the window.

It's also how SparkBox keeps working in the background. Closing the window doesn't quit SparkBox. It keeps watching the cluster, and any **Server** or VPN it's running keeps running.

## Where to find it

- **Menu bar:** look at the right end of the macOS menu bar for the SparkBox icon, with a short text beside it. Click it to open its menu.

It's there whenever SparkBox is running.

## What's on the screen

### The icon and its text

The text beside the icon reads like "4/4 · 27% · 2 warnings":

| Part | Meaning |
|---|---|
| "4/4" | Nodes online, out of all your nodes |
| "27%" | The average GPU use across the nodes online. A GPU that gave no reading is left out. **GPU —** means none gave one |
| "2 warnings" or "1 critical" | Active alerts. If any are critical, only those are named |

With no node online, the text reads "0/4 online".

| The icon looks | It means |
|---|---|
| Normal | Every node is online and nothing is wrong |
| With a small triangle in its corner | A node is offline, or there's an alert |
| Faded | No node is online |

### The menu

From top to bottom:

| Item | What it does |
|---|---|
| **SparkBox — live** or **SparkBox — demo data** | Says which data you're seeing. You can't click it |
| **Alerts: all healthy** or **Alerts: 1 critical, 2 warnings…** | Opens **Alerts** |
| A line for each node | Shows the node's state, and opens [its submenu](#a-nodes-submenu) |
| **Models: …** | Lists the models SparkBox loaded and their state, such as "qwen3.8-27b on cell1+cell2 ready", or "none loaded by SparkBox". Opens **Models** |
| **VPN: …** | The VPN's state. You can't click it |
| **Connect VPN** or **Disconnect VPN** | Connects or disconnects it |
| **Server: …** | Whether the **Server** is running, and at what address. You can't click it |
| **Start Server** or **Stop Server** | Starts or stops it |
| **Data Source** | **Demo** or **Live** |
| **Lock Admin Mode (12 min left)** or **Admin Mode: locked — unlock in SparkBox…** | Locks Admin mode now. While it's locked, this item brings the window forward so that you can unlock it there |
| **Open SparkBox** | Brings the window back |
| **Chat…** | Opens **Chat** |
| **Settings…** | Opens **Settings** |
| **About SparkBox** | Shows the version |
| **Quit SparkBox** | Quits SparkBox |

### A node's line

Each node's line sums it up, and its small shape shows the same state as the status bar:

| Line | Meaning |
|---|---|
| "cell1 — vllm-head · GPU 73% · 99/120 GiB · 65 W" | What it's serving, GPU use, memory used and total, and GPU power |
| "cell1 — no model loaded · 12/120 GiB" | Nothing is using its GPU |
| "cell2 — connecting…" | SparkBox is connecting |
| "cell3 — not updating (8 s)" | Connected, but no new readings for that long |
| "cell3 — no SSH reply" | The reason it's offline. Other reasons include "login failed", "host key not trusted yet" and "restarting" |

| Shape | Meaning |
|---|---|
| Filled green circle | Online and serving |
| Small gray circle | Online and idle |
| Amber triangle | Warning |
| Red diamond | Critical |
| Half-filled amber ring | Not updating |
| Amber ring | Connecting |
| Gray ring | Offline |

### A node's submenu

Point to a node's line for its submenu:

| Item | What it does |
|---|---|
| **Open cell1** | Opens the node's window |
| **Terminal** | Opens a shell tab on it |
| **Files** | Opens **Files** on it |
| **DGX Dashboard** | Opens the node's DGX Dashboard |
| **Copy IP (192.168.50.51)** | Copies its address |
| **Reboot…** | Opens the node's window and asks you to confirm a restart. It's the same as **Restart…** everywhere else |

### The Dock icon

While the window is open, SparkBox also has a Dock icon. A red number on it counts critical alerts, and a bar on it shows the progress of long work. Hiding the window to the menu bar removes the Dock icon as well.

### Closing and quitting

| You do this | What happens |
|---|---|
| Close the SparkBox window | With [Keep running in the menu bar](help:settings-general#menu-bar) on, the window and the Dock icon go away, and SparkBox keeps running. Admin mode locks. With it off, closing the window quits SparkBox |
| Choose **Open SparkBox** | The window and the Dock icon come back |
| Choose **Quit SparkBox**, or press <kbd>⌘Q</kbd> | SparkBox stops. Admin mode locks, terminal sessions end, and the **Server** stops. Models on your nodes keep running |

## What it changes

Showing the icon changes nothing. These menu items do:

| Item | What it changes |
|---|---|
| **Connect VPN**, **Disconnect VPN** | The VPN on this Mac |
| **Start Server**, **Stop Server** | The **Server** on this Mac |
| **Demo**, **Live** | Which data SparkBox shows |
| **Lock Admin Mode** | Locks Admin mode. It needs nothing |
| **Reboot…** | Restarts the node, after you confirm. It needs Admin mode |

The VPN, **Server** and **Data Source** items change this Mac. They need Admin mode, confirmed with Touch ID or your Mac password in the last 5 minutes. The menu doesn't ask for it. If it's needed, SparkBox opens its window, and a notification says what to do, such as "Unlock Admin mode in the top bar to connect the VPN." Unlock it, then choose the item again.

Each one is listed in the [activity log](help:activity-log) as coming from the menu bar. To undo one, choose the opposite item.

## Related How-Tos

- [Restart a node](help:restart-a-node)
- [Reach your cluster with the VPN](help:connect-the-vpn)
- [Run SparkBox as a server](help:run-sparkbox-as-a-server)
- [Switch between Demo and Live](help:switch-between-demo-and-live)
