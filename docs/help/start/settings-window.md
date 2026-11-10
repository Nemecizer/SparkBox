---
id: settings-window
title: Settings
type: feature
area: settings
summary: All of SparkBox's settings in one window, with a search box and 14 sections.
keywords: [settings, preferences, options]
verified: null
related: [save-the-cluster-password, add-or-change-a-node, switch-between-demo-and-live, settings-general, settings-security, settings-cluster-login, settings-data-source]
---

# Settings

## What it's for

**Settings** holds every choice SparkBox lets you make: how it looks, how it logs in to your nodes, where alerts go, and how it stays safe. You use it when you first connect your cluster, and whenever you want SparkBox to behave differently.

## Where to find it

- **Keyboard:** press <kbd>⌘,</kbd>.
- **Menu bar:** choose **SparkBox › Settings…**.
- **Command palette:** press <kbd>⌘K</kbd> and type `settings`, or the name of a section such as `notifications`.
- **Menu bar icon:** click it, then click **Settings…**.


## What's on the screen

### Search

The box at the top left filters the list as you type. It also matches words inside a section: `password` finds **Cluster login**, and `lock` finds **Security & privacy**. Press <kbd>Return</kbd> to open the first match. If nothing matches, the window says so.

### The section list

The list on the left has 14 sections. Click one to open it. You can also use the <kbd>↑</kbd> and <kbd>↓</kbd> keys, with <kbd>Home</kbd> and <kbd>End</kbd> for the first and last. **Settings** opens where you last left it.

| Section | What it's for |
|---|---|
| [General](help:settings-general) | How SparkBox starts, and whether it stays in the menu bar |
| [Appearance](help:settings-appearance) | Theme, text size, motion and contrast |
| [Cluster login](help:settings-cluster-login) | The account on your nodes, and how SparkBox logs in |
| [Nodes](help:settings-nodes) | The list of your nodes |
| [Switches & gateway](help:settings-switches) | Addresses and login for your network devices |
| [Data source](help:settings-data-source) | Demo or Live |
| [Demo scenarios](help:settings-demo-scenarios) | Scripted faults to practice on, in Demo |
| [Notifications](help:settings-notifications) | Where alerts go, and always-on monitoring |
| [Alerts](help:settings-alerts) | The rules that raise alerts |
| [Terminal](help:settings-terminal) | How the terminal looks and behaves |
| [Models](help:settings-models) | Model folders, engine images and your Hugging Face token |
| [Security & privacy](help:settings-security) | Look-only mode, Admin mode, tunnels and the activity log |
| [Fence](help:settings-fence) | What the test build may change. You don't need it for your own work |
| [Diagnostics](help:settings-diagnostics) | Reports to help find problems, and host keys |

### The section's page

The right side shows the section you chose. Beside its name, an info icon opens a Help tip about that section. Many fields inside have their own info icon too.

### The save bar

**Cluster login**, **Nodes** and **Switches & gateway** edit one shared list. When you change something in any of them, a bar appears with **Revert** and **Save**. With Live data, the save button reads **Apply & reconnect**. One click saves your edits in all three sections.

Most other sections save as soon as you change them.

## What it changes

Opening **Settings** changes nothing. What each field changes is explained on that section's own page.

Most changes in **Settings** are changes on this Mac. They ask for Touch ID or your Mac password, unless you confirmed in the last 5 minutes. A section's changes are written to the [activity log](help:activity-log).

In look-only mode, forms are read-only, so you can read a setting but not change it. Three sections stay usable:

- **Security & privacy**, so that you can turn look-only mode off. The rest of that section is read-only.
- **Fence**, because closing a fence only takes rights away.
- **Diagnostics**, because capturing evidence only reads.

## Related How-Tos

- [Save the cluster password](help:save-the-cluster-password)
- [Add or change a node](help:add-or-change-a-node)
- [Switch between Demo and Live](help:switch-between-demo-and-live)
- [Use look-only mode](help:use-look-only-mode)

![Settings, using synthetic Demo data](img/public-settings-window.jpg)
