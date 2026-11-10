---
id: alerts-window
title: Alerts
type: feature
area: alerts
summary: What's wrong now, what's silenced, what happened before, kernel events, and where each alert was sent.
keywords: [alerts, firing, history, events, deliveries, silenced, incidents, xid]
verified: false
related: [respond-to-an-alert, alerts-and-incidents, alert-rules-reference]
---

# Alerts

## What it's for

The **Alerts** window lists what's wrong now, what you've chosen to quiet, what happened before, and where each alert was sent. Open it when a count appears in the sidebar or the top bar, or when you want the history of a problem.

## Where to find it

- **Sidebar:** under **Overview**, click **Alerts**. A badge shows how many incidents count now.
- **Dashboard:** click the heading of the amber or red box, or the **All clear** line.
- **Command palette:** press <kbd>⌘K</kbd>, then type `Open Alerts`. **Show silenced alerts** goes straight to the **Silenced** tab.
- **Link:** [Open Alerts](open:alerts)

## What's on the screen


The window has five tabs.

### Active

Everything happening now, in groups. Each group's ⓘ says what it means.

| Group | What it means |
|---|---|
| **Firing** | Something's wrong. These count in every badge, and are sent as set in [Settings: Notifications](help:settings-notifications) |
| **Recovering** | Nothing is wrong any more. Each one clears when its clear time has passed. Not counted |
| **Planned** | Work SparkBox started, such as a restart. Counted nowhere and sent to nobody. It clears after the check that follows |
| **Acknowledged** | Known issues. No repeats and not counted, until one gets worse |
| **Information** | Shown here only. Never counted or sent |

Each row is an incident: alerts that share one cause. A **+2** beside the text means two more alerts are in it. Click the small arrow, or select the row and press <kbd>→</kbd>, to see them.

| Column | What it shows |
|---|---|
| The mark | How severe it is |
| **Affects** | What's wrong, such as `cell2 half 2` or `Root disk on gx10-0000` |
| **What** | What's wrong with it, with figures under it where they help |
| **State** | **Firing**, **Flapping · raised 3 times in 10 min**, **Recovering · clears in 1:34**, **Planned**, **Acknowledged** or **Information** |
| **Since** | How long it's been going |

**Flapping** means an alert keeps coming back. SparkBox holds it for 10 minutes after it clears, so it doesn't page you again and again.

The toolbar acts on the selected rows. You can select more than one.

| Button | What it does |
|---|---|
| **Acknowledge** | Marks the alert as known. It needs Admin mode |
| **Silence ▾** | Quiets one incident or alert for a time you choose. It needs Admin mode |
| **Unsilence** | Ends a silence on the selected row. It needs Admin mode |
| **Open** | Opens what the alert is about. Double-clicking a row does the same |

At the right, **Last change** says when an alert last changed. **1 silenced** opens the **Silenced** tab. Right-click a row for the same actions.

When nothing's wrong, the tab reads "All systems healthy", with **Show history**.

### Silenced

Every silence in effect: what's **Silenced**, **Until** when, and what it covers **Now**, such as "1 alert firing" or "Nothing firing". A **Demo** badge marks a silence set in Demo, which Live alerts never see.

Select a row and click **End silence** to end it early. That also ends any other silence on the same alerts. Double-click a row to open what it covers.

When a timed silence ends while its alert still fires, the alert counts again and the log says "Silence ended". "Until it clears" ends after 7 days at most. Demo and Live keep separate silences.

### History

Every alert event, newest first. SparkBox keeps the last 1,000 for each data source.

Use the toolbar to filter: **All**, **Raised**, **Cleared** or **While away (3)**, then **Any severity**, **Critical** or **Warning**. **Filter alerts** searches the text.

| Column | What it shows |
|---|---|
| **Time** | When it happened |
| **Event** | **Raised**, **Became critical**, **Eased to warning**, **Cleared**, **Acknowledged**, **Silenced**, **Unsilenced** or **Silence ended** |
| **Severity** | **Critical**, **Warning** or **Info** |
| **Affects** | What it's about. Click to open it |
| **Message** | What happened, with the reason for a silence or a clear |

**While away** marks what an [always-on agent](help:always-on-agent) recorded while SparkBox was closed. A **Demo log** badge means you're reading Demo's own log. Live keeps a separate one.

### Events

Low-level problems from each node's kernel log. The kernel is the core of the operating system. The tab lists:

| Kind | What it means |
|---|---|
| **GPU Xid** | The GPU reported an error, with a code |
| **ConnectX-7** | The fast network card's driver reported a fault |
| **Fabric link** | A cable or link changed state. This is information, and never raises an alert |
| **Out of memory** | The system stopped a program to free memory |
| **Hung task** | A program stayed stuck for more than 2 minutes |
| **Boot** | The node started |

Filter by **Any severity**, **Critical**, **Warning** or **Info**. Link changes within 15 minutes of each other, on any node, share one row. Click its arrow to see them.

Select a row, and a panel on the right explains it: **What it means**, **What to do**, and the raw line from the log. Double-click to open the node.

A node's **Show in Alerts** link opens this tab for that node alone, with **Show all nodes** to widen it.

### Deliveries

One row for each alert event and channel, with what became of it, grouped by day. A **channel** is one place alerts go, such as your phone. SparkBox keeps the last 500.

| Column | What it shows |
|---|---|
| **Time** | When it was sent or decided |
| **Outcome** | **Sent**, **Failed**, **Demo**, **Grouped**, **Duplicate**, **Held**, **Silenced**, **Planned**, **Eased**, **Flapping** or **Not paged**. Move the pointer over it for the full sentence |
| **Channel** | Which channel |
| **Sent by** | **This Mac**, or the agent on a node |
| **Message** | What it said, with **(test)** for a test |
| **Result** | When it went out, why it failed, or why it wasn't sent |

**Reached or tried a channel** hides what was never meant to go out. **Show not paged (12)** shows everything. **Channels…** opens [Settings: Notifications](help:settings-notifications).

A failure that may pass is tried again after 30 s, 2 min and 10 min. A critical alert then keeps trying every 30 minutes for up to 24 hours. A clear goes out once it has stayed clear for 10 minutes, and a node back online goes out at once. In Demo nothing leaves this Mac: alerts are recorded as "Demo — not sent", and only a **Test** button sends for real.

## What it changes

| Action | What it changes | Needs | How to undo |
|---|---|---|---|
| **Acknowledge** | This Mac's alert state, and the node agents stop repeating it | Admin mode | Can't be undone by hand. It ends when the alert clears or gets worse |
| **Silence**, for one alert or incident, for 1 hour or 1 day | This Mac's alert state, and the node agents get it too | Admin mode | **End silence** |
| **Silence**, wider or longer, with **Live** data | The same | Admin mode, Touch ID or your Mac password, and a reason | **End silence** |
| **End silence**, **Unsilence** | Ends the silence | Admin mode | Silence it again |

The **History**, **Events** and **Deliveries** tabs only read. Every acknowledge and silence goes in the [Activity log](help:activity-log).

## Related How-Tos

- [Respond to an alert](help:respond-to-an-alert)
- [Change alert rules](help:change-alert-rules)
- [Get alerts on your phone](help:get-alerts-on-your-phone)
- [Find out why a node restarted](help:find-out-why-a-node-restarted)

![Alerts, using synthetic Demo data](img/public-alerts-window.jpg)
