---
id: settings-security
title: "Settings: Security & privacy"
type: feature
area: settings
summary: Look-only mode, when Admin mode locks, open SSH tunnels, how long the activity log is kept, and exports.
keywords: [admin, lock after, retention, tunnels, privacy]
verified: false
related: [admin-mode, unlock-admin-mode, use-look-only-mode, find-what-sparkbox-did, keychain-and-secrets]
---

# Settings: Security & privacy

## What it's for

This section controls how careful SparkBox is. Use it to turn look-only mode on or off, and to choose when Admin mode locks. You can also close an SSH tunnel, choose how long the activity log is kept, and export records. [Read-only, Admin mode and Look only](help:admin-mode) explains the ideas behind it.

## Where to find it

- **Settings:** press <kbd>⌘,</kbd>, then click **Security & privacy**.
- **Command palette:** press <kbd>⌘K</kbd> and type `security`.
- **Link:** [Open Security settings](open:settings?section=security)


## What's on the screen

### Look only

**Look-only mode** is a switch. While it's on, SparkBox makes no changes to your cluster or this Mac. The one exception is its own helper scripts in `~/.sparkbox` on each node, which it keeps up to date. It refuses every command that isn't a read, before it runs, and it never asks for Touch ID.

- Turning it on takes effect at once, and locks Admin mode. It asks for nothing.
- Turning it off asks for Touch ID or your Mac password.
- It stays on when you quit and reopen SparkBox.

If SparkBox was started with `SPARKBOX_LOOK_ONLY=1`, the switch is dimmed and its hint says "Set by SPARKBOX_LOOK_ONLY for this run". SparkBox then stays look-only until it quits.

While look-only mode is on, this switch is the only control in this section that works. [Use look-only mode](help:use-look-only-mode) lists what stays and what stops.

### Admin mode

**Right now** shows the current state, with a button:

| State | Button | What it means |
|---|---|---|
| **Locked** | **Unlock…** | A change you ask for waits until you unlock. [Some of SparkBox's own work goes on](help:admin-mode#what-sparkbox-still-does-while-locked) |
| **Unlocked · confirmed it's you** | **Lock now** | You confirmed in the last 5 minutes, so changes on this Mac are allowed too |
| **Unlocked · nodes only** | **Lock now** | Changes on the cluster are allowed. Changes on this Mac will ask first |
| **Look only** | None | Nothing can be unlocked |

While unlocked, the hint counts down: "Locks in 12:40 unless you make a change".

#### Lock after

Choose **5 minutes**, **15 minutes**, **1 hour**, **2 hours**, **4 hours**, **8 hours** or **24 hours** from the menu. The default is **15 minutes**. This is time without a change to the cluster; each change restarts the clock.

The choice is saved for future sessions. A shorter choice applies at once, measured from your last change. A longer choice applies on your next change or unlock. The countdown shows hours when at least an hour remains, such as **23:59:59**. This choice does not extend the separate five-minute confirmation for changes on this Mac.

Admin mode also locks when the Mac sleeps or its screen locks, when you close the window to the menu bar, and when SparkBox quits. When it locks, SSH tunnels close. With Live data, open shells stop taking keystrokes on every lock, and the session stays open until you authenticate. With Demo data, they keep working through an idle lock and when the window closes. Shells also stop after their own idle time, set in [Settings › Terminal](help:settings-terminal).

#### Changes on this Mac

This row tells you whether changes on this Mac are allowed now. These changes cover the node list, the data source, the VPN, the Server, providers and alert delivery. They also cover files in the Mac side of **Files**, except copies into Downloads. Each confirmation with Touch ID or your Mac password allows them for 5 minutes, and nothing extends that. While it's running, the hint counts down: "Confirmed: 3:12 left". Saving or removing a password or key you type asks every time.

#### Asking who you are

This row tells you when SparkBox asks for Touch ID or your Mac password. With Live data, every unlock asks. With Demo data, unlocking for the simulated nodes asks nothing, but changes on this Mac still ask.

### Keep running: page-cache permission

This permission starts off disabled. **Grant with Touch ID…** asks for a fresh confirmation and records the current inventory nodes for this SparkBox instance in the Keychain. It allows only the conditional `sync` and page-cache drop needed by a saved vLLM load's Keep running restart. A fresh memory check must show the need. Every use is audited.

The grant survives an idle Admin lock and app restarts. It bypasses no look-only setting, explicit owner hold, identity check, ownership check or controller lease. New inventory nodes need a new grant. **Revoke with Touch ID…** removes permission for future attempts; it does not undo a cache drop already admitted. Demo cannot grant it. See [Restart a model or change its settings](help:restart-or-change-a-model).

### SSH tunnels

Some pages only listen on the node itself, such as the [DGX Dashboard](help:open-the-dgx-dashboard). SparkBox opens these in your browser through SSH, at a local address with a one-time key. Each tunnel has its own key, and the page itself never sees it.

This group lists the tunnels open now, or **None**. Each row shows the page and the node, its local address, when it opened, and when it was last used. **Close** ends it.

Tunnels close after 15 minutes without use, whenever Admin mode locks, and when SparkBox quits. Closing your browser tab doesn't close a tunnel. Close it here.

### Activity log

SparkBox writes every command it runs on a node to the [activity log](help:activity-log), before it's sent and again when it ends. It records the whole command, with up to 64 KB of its output. Changes on this Mac are listed too, with the old and new value.

Only your user account can read the log. sudo passwords and keys travel on standard input, not on the command line. Anything that looks like a password, token or key is replaced with ‹redacted› before it's written.

#### Keep entries for

How long the log is kept: **30 days**, **90 days** or **1 year**. The default is **90 days**. The log starts a new file each day, or at 10 MB. A file whose newest entry is older than your choice is deleted when SparkBox starts and whenever a new file starts. There's no limit on how many files that keeps.

#### Export

**CSV…** and **JSON…** save the entries for the data source you're using to your Downloads folder. A message appears with **Show in Finder**.

### Diagnostics

**Export…** saves one text file to Downloads. It holds SparkBox's version, settings, inventory, recent tasks and activity, and its own log from `~/Library/Logs/SparkBox`. Use it when something isn't working. Passwords and keys are never in it.

This export covers SparkBox on this Mac. To collect what your nodes report, use **Run doctor** in [Settings › Diagnostics](help:settings-diagnostics).

## What it changes

| Control | What it needs | What it changes | How to undo |
|---|---|---|---|
| **Look-only mode**, on | Nothing | Locks Admin mode and stops every change | Turn it off |
| **Look-only mode**, off | Touch ID or your Mac password | Allows Admin mode again | Turn it on |
| **Lock after**, **Keep entries for** | Touch ID or your Mac password, unless you confirmed in the last 5 minutes | A setting on this Mac | Choose the old value |
| **Lock now** | Nothing | Locks Admin mode | Unlock it |
| **Close** on a tunnel | Nothing | Ends the tunnel | Open the page again |
| **CSV…**, **JSON…**, **Export…** | Look-only mode off | Adds a file to Downloads | Delete the file |

A shorter **Keep entries for** deletes old log files the next time SparkBox checks, and they can't be brought back.

Each change here is written to the activity log.

## Related How-Tos

- [Unlock Admin mode](help:unlock-admin-mode)
- [Use look-only mode](help:use-look-only-mode)
- [Find out what SparkBox did](help:find-what-sparkbox-did)
- [Collect diagnostics](help:collect-diagnostics)
