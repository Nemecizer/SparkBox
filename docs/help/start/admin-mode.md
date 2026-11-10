---
id: admin-mode
title: Read-only, Admin mode and Look only
type: concept
area: safety
summary: A change you ask for needs Admin mode, which locks by itself. Look-only mode goes further and stops SparkBox's own work.
keywords: [admin, locked, padlock, permission, touch id, fingerprint, read only, read-only, button disabled, button greyed out, button grayed out, greyed out, grayed out, cant change anything, wont let me, cant click, nothing happens]
verified: null
related: [unlock-admin-mode, use-look-only-mode, settings-security, keychain-and-secrets]
---

# Read-only, Admin mode and Look only

## In short

SparkBox starts **Read-only**: it reads your cluster, and it won't make a change you ask for. To change something, you unlock **Admin mode** with Touch ID or your Mac password, and it locks again by itself. Work you've already started can finish while it's locked. **Look only** goes further: SparkBox starts no change of its own, and nothing can be unlocked, until you turn it off.

## How it works

### The three states

The padlock in the top bar always tells you which state you're in.

| The padlock says | What it means |
|---|---|
| **Read-only** | Everything you can read stays open, but a change you ask for needs Admin mode. A button that changes something shows a small lock, and clicking it asks you to unlock first. [Some of SparkBox's own work goes on](#what-sparkbox-still-does-while-locked) |
| **Admin · 14:32** | Admin mode is unlocked. The clock shows how long until it locks by itself |
| **Look only** | SparkBox starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date, and it stops its own work too. Change buttons are dimmed |

The rule is enforced by the core of the app, not only by its buttons. A locked SparkBox refuses a change you ask for even if something tries to send it directly.

### Two kinds of change

SparkBox treats changes in two ways, because they carry different risks.

**Changes on your cluster** touch a node, a switch or a model. Examples are restarting a service, loading a model and installing updates. These need Admin mode unlocked.

- With **Live** data, unlocking asks for Touch ID or your Mac password.
- With **Demo** data, unlocking asks for nothing, because the nodes are simulated.

**Changes on this Mac** touch what's kept on your computer. Examples are the node list, the data source and the VPN. The Server, providers, where alerts go, and files in the Mac side of [Files](help:files) count too. These need Admin mode unlocked, and they also need Touch ID or your Mac password in the last 5 minutes. That's true with Demo data too, because these changes are real even when the cluster is simulated. Nothing extends the 5 minutes.

Alert rules are a change on this Mac, with Demo data too. Silences depend on how wide they are. A silence of one alert or one incident for an hour or a day needs only Admin mode. Some silences also need Touch ID or your Mac password with Live data: a wider silence, one that lasts until the alert clears, and one of a week or longer.

**Keychain changes** are stricter still. Saving or removing a password or key you type asks for Touch ID or your Mac password every time, even if you confirmed a minute ago. SparkBox's own keys don't ask: it makes a load's API key when you load the model, and removes it when you unload.

### An example

You click the padlock at 9:00 and confirm with Touch ID.

- **9:02:** you restart a service and change a node's address. Neither asks again. Your confirmation is under 5 minutes old.
- **9:06:** you change the VPN settings. Your confirmation is now over 5 minutes old, so SparkBox asks for Touch ID again.
- **9:08:** you restart another service. It doesn't ask, because Admin mode is still unlocked.
- **9:09:** you save a new password. It asks, because Keychain changes always ask.
- **9:24:** you've made no change for 15 minutes. Admin mode locks by itself.

### When Admin mode locks

Admin mode locks by itself in these cases:

- You make no change for the time set in **Lock after**: 5 or 15 minutes, or 1, 2, 4, 8 or 24 hours. The default is 15 minutes, and each change restarts the clock.
- The Mac goes to sleep, or its screen locks.
- You close the window to the menu bar.
- SparkBox quits.
- You switch from Demo to Live, because a Demo unlock never asked who you are.
- You turn on look-only mode.

A minute before an idle lock, a message appears: "Admin mode locks in 1 min". Click **Keep unlocked** to restart the clock. After any lock that you didn't ask for, a message says why it happened.

Locking doesn't stop work that has already started. A download, a load or an upgrade keeps running and isn't refused partway. Here's what a lock does stop:

- **SSH tunnels** close on every lock, including the tunnel to the DGX Dashboard.
- **Open shells** in Terminal stop taking keystrokes with Live data on every lock. The session stays open: click **Authenticate…** in the tab to type again. With Demo data, a shell keeps working through an idle lock and when you close the window to the menu bar. [Locked shells](help:terminal#locked-shells) has the full list.
- **Queued work**, such as a restart waiting its turn, waits if you lock on purpose, the screen locks, the Mac sleeps or SparkBox quits. An idle lock doesn't delay it.

### What SparkBox still does while locked

**Read-only** stops new changes from you. It doesn't freeze your cluster. SparkBox finishes and looks after work you've already asked for, and some of that work changes a node. Every change it makes here goes in the [activity log](help:activity-log).

| What | With Admin mode locked | With look-only mode on |
|---|---|---|
| Work already running | A download, a copy, a load or an upgrade keeps going | Keeps going where it runs by itself on a node. Tasks on this Mac stop before their next change |
| **Keep running** on several nodes | If the load goes down, SparkBox stops its containers and starts them again in order. It waits 30 seconds, then 2, 5 and 10 minutes, and gives up after 5 restarts in 30 minutes, or 5 in a row that don't reach **Ready**. Locking on purpose delays the next try but doesn't cancel it | Held |
| **Keep running** on one node | Docker restarts the container by itself on the node, even with SparkBox closed | Keeps going, because SparkBox isn't involved |
| The end of a copy | SparkBox stops the temporary file server on the source node | Waits until look-only mode ends. The server stops by itself after 12 hours |
| Alerts you acknowledged, and the end of planned work (a restart, a shutdown or an upgrade) | SparkBox tells the [always-on agents](help:always-on-agent), and tries again until each one has it | Waits until look-only mode ends |
| Queued work, such as a restart waiting its turn | Goes ahead after an idle lock, or after you close the window. It waits if you lock on purpose, the screen locks, the Mac sleeps or SparkBox quits | Waits |
| Using a model | **Chat** and apps connected to the [Server](help:server) keep sending requests. That uses a model but changes nothing on the node | Stops |

Only [look-only mode](help:use-look-only-mode) stops this work. It can't stop what's already running by itself on a node, or Docker's own restarts, because stopping them would be a change.

Two jobs don't go on while Admin mode is locked. They wait until it's unlocked:

- **Alert settings.** SparkBox sends settings that changed, and tries a failed send again, only while Admin mode is unlocked, because they copy your secrets to the nodes.
- **A folder left by a failed copy.** A staging folder is a hidden folder where a copy's files wait until they've all arrived. A copy that loses its node can leave one behind. SparkBox removes it once the node answers and Admin mode is unlocked, along with any other idle staging folders beside it. Nobody is there to confirm that delete, so it waits. It also waits in look-only mode.

### Look only

Look-only mode is for when nothing must change: you're looking around, showing someone the app, or leaving your Mac with someone else.

- Turning it on takes effect at once and asks for nothing. It only takes rights away.
- Turning it off asks for Touch ID or your Mac password.
- It stays on when you quit and reopen SparkBox.
- SparkBox refuses every command that isn't a read, before it runs, and it never asks for Touch ID.

Look-only mode also stops model use. The Server stops, Chat doesn't send, and test requests don't run. Running tasks that change something stop before their next change. Work already running by itself on a node keeps going there, because stopping it would be a change. An upgrade and a loading model are two examples.

There's one small exception. SparkBox's helper scripts live in `~/.sparkbox` on each node, and SparkBox keeps them up to date by checksum, even in look-only mode. In look-only mode SparkBox only refreshes these files and runs their reading commands.

## Why it matters in SparkBox

You meet these states every time you use SparkBox:

- To change something, click the button you want. If Admin mode is locked, SparkBox offers to unlock it. [Unlock Admin mode](help:unlock-admin-mode) shows every way in.
- To stop every change SparkBox would start, use [look-only mode](help:use-look-only-mode).
- To choose when Admin mode locks, open [Settings › Security & privacy](help:settings-security).

The SparkBox app needs Admin mode and nothing else for its changes. It doesn't need a [fence](help:the-fence), whatever you're doing: downloads, loads, updates and restarts included.

Unlocking doesn't turn off SparkBox's other checks. Before any change on a node, it still confirms the [node's identity](help:leases-and-identity#identity-pin) and the [controller lease](help:leases-and-identity#controller-lease), and it writes the change to the [activity log](help:activity-log).

## Related

- [Unlock Admin mode](help:unlock-admin-mode)
- [Use look-only mode](help:use-look-only-mode)
- [Settings: Security & privacy](help:settings-security)
- [Where your passwords and keys go](help:keychain-and-secrets)
- [Admin, look-only and safety messages](help:errors-admin-and-safety)
