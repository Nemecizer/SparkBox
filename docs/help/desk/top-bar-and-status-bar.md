---
id: top-bar-and-status-bar
title: The top bar and status bar
type: feature
area: desk
summary: What every word, chip and count in the top and bottom bars means, and what clicking it does.
keywords: [top bar, status bar, 24/7, padlock, chips, stale, look only, read-only, freshness]
verified: null
related: [status-words, admin-mode, alerts-and-incidents, tasks-and-jobs]
---

# The top bar and status bar

## What it's for

The two bars tell you how the cluster is doing without opening a window. The top bar answers "Can SparkBox change anything, and is the cluster well?" The status bar answers "How is each device, and is this data fresh?"

Almost everything in them can be clicked, and moving the pointer over an item shows the details.

## Where to find it

- **Top bar:** it runs along the top of the SparkBox window.
- **Status bar:** it runs along the bottom.

Both are always there. A [separate window](help:windows-and-layouts#separate-windows) has neither.

## What's on the screen

### Top bar


From left to right:

| Item | What it shows | Click it to |
|---|---|---|
| **Live** or **Demo** | Where the numbers come from: your real nodes, or the built-in simulator. See [Demo and Live](help:demo-and-live) | It isn't a button. Change it in [Settings › Data source](help:settings-data-source) |
| Scenario pill | With Demo data, the [scenario](help:settings-demo-scenarios) that's playing, such as "Node offline · fault · recovers in 0:59" | Open **Settings › Demo scenarios** |
| Padlock | Whether SparkBox can make changes. See [the padlock](#the-padlock) | Unlock or lock Admin mode |
| Summary | The cluster in one line. See [the summary](#the-summary) | Open **Alerts** if anything needs attention, otherwise the **Dashboard** |
| **1 failed deployment** | A model load failed in the last week. It isn't counted as a problem, since nothing is down because of it | Open **Models** on the **Loaded** tab |
| Watch badge | Who alerts you when this Mac is asleep. See [the watch badge](#the-watch-badge) | Open **Settings › Notifications** |
| **Search** | The [command palette](help:command-palette) | Open it. <kbd>⌘K</kbd> does the same |
| **Tile** | Arranges your windows. The arrow beside it has more choices | Tile all windows. <kbd>⌥⌘T</kbd> does the same |
| **Windows** | How many windows are open | List them, arrange them, and save or open a layout |
| **Close all** | Nothing until you click it. It's dimmed with no windows open | Close every window. **Undo** brings them back. <kbd>⌥⌘W</kbd> does the same |

Empty space in the top bar works like any Mac title bar: drag it to move the window, and double-click it to zoom.

When the window is narrow, the top bar folds in steps so that everything still fits. First the watch badge's action goes. Then the summary shortens to the online count and any problems, and the watch badge keeps only its icon. Then **Tile**, **Windows** and **Close all** lose their labels. Last, those three fold into one **…** menu.

### The padlock

| It says | What it means |
|---|---|
| **Read-only** | SparkBox won't make a change you ask for, but it finishes and looks after work already started. Click it, or press <kbd>⇧⌘L</kbd>, to unlock Admin mode |
| **Admin · 14:32** | Changes are allowed. The clock shows how long until Admin mode locks itself. Click it to lock now |
| **Look only** | SparkBox starts no change of its own, and nothing can be unlocked until you turn this off. Work already running by itself on a node keeps going. Click it to see how to turn it off |

[What SparkBox still does while locked](help:admin-mode#what-sparkbox-still-does-while-locked) lists the work that goes on in each state. [Look only](help:admin-mode#look-only) names the one small exception: SparkBox keeps its helper scripts on each node up to date. [Admin mode](help:admin-mode) and [Use look-only mode](help:use-look-only-mode) explain these in full.

### The summary

The summary reads like this: "4/4 online · 3 models · 4 GPUs serving · 1 critical · 2 warnings".

| Part | Meaning |
|---|---|
| "4/4 online" | Nodes that answer, out of all your nodes. It turns red when one is missing, unless SparkBox restarted it or shut it down itself |
| "3 models" | Models answering requests in the last minute |
| "4 GPUs serving" | GPUs those models run on |
| "no model serving" | Nothing is answering |
| "1 critical · 2 warnings" | Problems that need you. Related alerts count once, as one **incident**. See [Alerts and incidents](help:alerts-and-incidents) |

Recovering problems and ones you've silenced or acknowledged aren't counted. They're listed when you move the pointer over the summary.

When the top bar runs out of room, the summary keeps only the online count and any problems, so "no model serving" leaves the bar. This can happen at the default window size when a scenario label is showing. The tooltip still says it: "No model is answering", or the models that are serving.

### The watch badge

This badge says who would alert your phone if this Mac were asleep or closed. A **page** is an alert sent to your phone, and an **agent** is SparkBox's small always-on service on a node.

| Badge | Meaning | Button beside it |
|---|---|---|
| **This Mac only** | No agents yet. Alerts reach you only while SparkBox runs on this Mac | **Set up…** |
| **Not paging** | Agents are watching, but no phone channel sends from them | **Set up…** |
| **24/7 on 3 of 4** | One agent has stopped answering | **Details…** |
| **Agents not answering** | No agent is answering | **Details…** |
| **24/7 · settings not on cell2** | That node's agent has old alert settings | **Push again** |
| **24/7 · paging failed** | The last page didn't get through | **Details…** |
| **24/7 · untested** | No test page has proved the path to your phone | **Send a test page…** |
| **24/7 · no dead-man's switch** | Nothing would page you if the power or the internet went | **Set up…** |
| **24/7** | Agents are watching and a test page got through | None |

[The always-on agent](help:always-on-agent) explains these. With Demo data, the agents and the phone channel are simulated.

### Status bar


From left to right:

| Item | Example | Click it to |
|---|---|---|
| Node chips | `cell1` 22 ms | Open the node's window. Right-click for its menu |
| Switch and gateway chips | `CRS804 DDQ` 1.0 ms | Open the switch, or **Settings › Switches & gateway** if it says **Not set up** |
| **VPN** | **Off**, or the protocol and delay | Open **VPN** |
| **Server** | `:8484`, its port, while it's running | Open **Server** |
| Tasks | "2 running" while tasks run | Open a drawer with the tasks. See [Tasks](help:tasks) |
| Keeping awake | "Keeping this Mac awake while Restart cell2 runs" | Open that task |
| Alerts | **Healthy**, or counts of critical alerts and warnings | Open **Alerts** |
| Freshness | **Demo · 1 s** or **Live · 1 s** | It isn't a button |

**A node chip** has a shape, the node's name, and how it's doing:

| After the name | Meaning |
|---|---|
| A time, such as `22 ms` | The node is online. This is the round trip of SparkBox's last command over SSH |
| **Stale 8 s** | The connection is up, but no new readings have come for that long |
| **In maintenance** | You've put the node in [maintenance mode](help:use-maintenance-mode) |
| **Restarting**, **Shutting down**, **Off (planned)**, **Waking** | SparkBox is doing this on purpose |
| **Offline** | SparkBox can't reach the node |
| **Error** | The node answered but turned SparkBox away, for example over a host key or a login |

| Shape | Meaning |
|---|---|
| Filled circle | Online and well |
| Triangle | Warning |
| Diamond | Critical |
| Circle with a slash | Offline |
| Wrench | Planned work, such as a restart SparkBox started or maintenance mode |

A node's shape is about the node itself. A model that's down on a node doesn't color it. That's noted in the chip's tooltip instead. [What the status words mean](help:status-words) lists every state.

**A switch chip** shows a time, **Not set up**, **Connecting** or **Unreachable**.

**Freshness** pulses while data keeps arriving, and it says **Demo · 1 s** or **Live · 1 s**. If no new data arrives for more than 3 seconds, it turns to **Stale 8 s**, with the age. What you see then is the last thing SparkBox received. **Waiting for data…** means nothing has arrived yet.

When the window is narrow, chips drop their times first, then their names, leaving only the shape. The tooltip always has the details.

## What it changes

The bars only show things, with these exceptions:

| Action | What it changes | How to undo it |
|---|---|---|
| Padlock | Unlocks or locks Admin mode | Click it again |
| **Close all** | Closes your windows | **Undo** in the message, for 10 seconds |
| **Push again** | Sends your current alert settings to every agent | Change the settings and push again |
| **Send a test page…** | Asks first, then sends a test alert to your phone | Nothing to undo |

## Related How-Tos

- [Unlock Admin mode](help:unlock-admin-mode)
- [Respond to an alert](help:respond-to-an-alert)
- [Set up always-on monitoring](help:set-up-always-on-monitoring)
- [Read the Dashboard](help:read-the-dashboard)
