---
id: settings-general
title: "Settings: General"
type: feature
area: settings
summary: Keep SparkBox in the menu bar, start it at login, and show upcoming features.
keywords: [login, startup, upcoming, coming soon, menu bar, quit, close window, automatically, start automatically, open automatically, auto start, launch at login, update sparkbox, sparkbox version, new version of sparkbox]
verified: null
related: [back-up-sparkbox, restore-sparkbox, reset-sparkbox, menu-bar-icon, settings-window]
---

# Settings: General

## What it's for

**General** decides what happens when you close the SparkBox window, and whether SparkBox starts when you log in. It also decides whether the sidebar lists features that aren't built yet.

There's no setting here for updating SparkBox. SparkBox doesn't update itself, and it doesn't check for new versions. A new version arrives as a new copy of the app, which replaces the one on this Mac. [Updates](help:updates) is for the software on your nodes.

## Where to find it

- **Sidebar:** click **Settings**, then click **General**.
- **Menu bar:** choose **SparkBox › Settings…**, then click **General**.
- **Keyboard:** press <kbd>⌘,</kbd>, then click **General**.
- **Command palette:** press <kbd>⌘K</kbd>, type `general`, and choose **Settings › General**.
- **Link:** [Open General settings](open:settings?section=general)


## What's on the screen

### Menu bar

| Setting | What it does | Default |
|---|---|---|
| **Keep running in the menu bar** | On: closing the window keeps SparkBox running in the menu bar. It goes on watching the cluster, and any **Server** or VPN it's running stays up. Off: closing the window quits SparkBox | On |
| **Start at login** | On: SparkBox starts when you log in to this Mac, with no window, in the menu bar. Off: you start it yourself | Off |

Closing the window always locks Admin mode, whichever you choose.

To quit SparkBox when it's running in the menu bar, press <kbd>⌘Q</kbd> or choose **Quit SparkBox** from [the menu bar icon](help:menu-bar-icon).

### Features

| Setting | What it does | Default |
|---|---|---|
| **Show upcoming features** | Adds a **Coming soon** group to the sidebar and the command palette | Off |

**Coming soon** lists planned engines that aren't built yet: **TensorRT-LLM**, **llama.cpp** and **NIM**. Each one opens a window that says what it will do, and nothing in that group can run. **SGLang** is listed too, and it leads to **Models › Load**, where you can pick it today. [What's coming](help:whats-coming) lists the plans.

## What it changes

| Setting | What it changes | What it needs | How to undo it |
|---|---|---|---|
| **Keep running in the menu bar** | SparkBox's own settings file on this Mac | Nothing | Turn it back |
| **Start at login** | Adds or removes a login item on this Mac | Touch ID or your Mac password. The change is in the [activity log](help:activity-log) | Turn it back |
| **Show upcoming features** | SparkBox's window storage on this Mac | Nothing | Turn it back |

None of these touches your nodes. If **Start at login** can't be changed, a message says "Couldn't change Start at login:" with the reason.

## Related How-Tos

- [Back up SparkBox](help:back-up-sparkbox)
- [Restore SparkBox from a backup](help:restore-sparkbox)
- [Reset SparkBox](help:reset-sparkbox)
- [Set up always-on monitoring](help:set-up-always-on-monitoring), so that alerts reach you when this Mac is off
