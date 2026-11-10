---
id: settings-notifications
title: "Settings: Notifications"
type: feature
area: settings
summary: Where alerts go, when they're sent, always-on monitoring on the nodes, and the Prometheus metrics endpoint.
keywords: [notifications, channels, quiet hours, prometheus, paging, dead-man's switch, agents, at night, overnight, night, do not disturb, alerts at night]
verified: false
related: [get-notifications-on-this-mac, get-alerts-on-your-phone, set-up-always-on-monitoring]
---

# Settings: Notifications

## What it's for

**Settings › Notifications** decides how alerts reach you. Use it to choose where alerts go, when they're sent, and whether your nodes keep watching while your Mac is closed.

## Where to find it

- **Settings:** open **Settings**, then click **Notifications**.
- **Alerts:** on the **Deliveries** tab, click **Channels…**.
- **Top bar:** click the watch badge, such as **This Mac only**.
- **Link:** [Open Notifications settings](open:settings?section=notifications)

## What's on the screen


### Paging

To **page** means to send an alert to your phone. This checklist shows what the top bar's badge rests on. Each line has a checkmark or a warning mark, and a button where one helps.

| Line | What it checks |
|---|---|
| **No node agents** | With no agents, alerts reach you only while SparkBox runs on this Mac |
| **Phone channel** | A channel the agents can use |
| **Test page** | A test page went out from every node after the last change. **Send a test page from every node…** sends one |
| **agents answering** | Appears when an agent isn't answering |
| **agents on current settings** | Each agent runs what this Mac sent it. **Push again** sends the settings again |
| **Dead-man's switch** | An outside check that pages you if the pings stop. **Set up…** adds one |

### Where alerts go

Each row is a **channel**, one place alerts go. [What reaches you](help:alerts-and-incidents#what-reaches-you) explains how SparkBox decides what to send, and [Who sends a page](help:always-on-agent#who-sends-a-page) explains which side sends it.

**This Mac** is Notification Center on this Mac. Its switch reads **Show notifications on this Mac**. macOS asks for permission the first time there's something to say.

Each other channel's row shows its name, its kind and where it sends from. It also shows how its last delivery went, such as "This Mac only · Sent 2 min ago".

| Control | What it does |
|---|---|
| **Test** | Sends a real message now, even with **Demo** data. It's off in look-only mode. If agents can use the channel, it asks whether to send from this Mac or from a node's agent |
| **Edit** | Opens the channel's form |
| **Remove** | Deletes the channel. Its secrets leave the Keychain and, with the next push, the nodes |
| The switch | Turns the channel on or off |

The buttons below the list add a channel: **ntfy**, **Pushover**, **Webhook**, **Email** and, until you have one, **Dead-man's switch**.

A channel's form has these fields. Secrets go to the Keychain, and a saved one shows as "Saved in the Keychain · type to replace".

| Channel | Fields |
|---|---|
| Every channel | **Name**, **Critical only** (send only critical alerts here) and **This Mac only** |
| **ntfy** | **Server**, **Topic**, **Access token** |
| **Pushover** | **Device**, **App token**, **User key** |
| **Webhook** | **Slack-compatible**, **URL**, **Signing secret** |
| **Email** | **SMTP server**, **Port**, **Security** (**STARTTLS** or **SSL**), **User**, **From**, **To**, **Password** |
| **Dead-man's switch** | **Name**, **Ping URL** |

The switch **Send from this Mac only (not copied to the nodes)** is on for a new channel. With it on, the channel's secrets stay in this Mac's Keychain, and it reaches you only while SparkBox runs here.

> [!WARNING]
> **A channel you copy to the node agents has its secrets on every node.** There they're readable by the `sparkbox-agent` service, root and anyone in the docker group, which includes your login user. To limit the risk, give the agents credentials that can only send: a publish-only ntfy token, an app password for email, a dedicated Pushover application. To undo it, remove the channel, and SparkBox deletes its secrets from the nodes with the next push.

If you change where a channel points, such as its server, SparkBox clears its saved secret and asks you to type it again. It won't send an old secret somewhere new.

### When to notify

| Setting | Default | What it does |
|---|---|---|
| **Critical alerts only** | Off | Warnings wait in the **Alerts** window |
| **When an alert clears** | On | Says when a problem ends, such as "cell3 is back online" |
| **Repeat critical alerts** | **1 h** | Sends "Still firing" this often until the alert is acknowledged or silenced. **Never** turns it off |
| **Node status** | On | Offline, back online, not updating, memory, clock, out-of-memory kills |
| **Disk** | On | Root disk full, or filling up within a day |
| **GPU** | On | Running hot, throttling, Xid errors |
| **Fabric & switches** | On | Links down or flapping, errors, MTU, weak optics, RoCE counters |
| **Containers, services & models** | On | Crashes, crash loops, failed units, a model down or slow |
| **Tasks** | On | Long tasks that finish while SparkBox is in the background. This Mac only |
| **Quiet hours** | Off, 22:00 to 07:00 | Only critical alerts and their clears go out. Warnings wait, then arrive as one summary |

Quiet hours follow one time zone, taken from this Mac when you save them. If this Mac moves to another zone, a note offers **Use** and the new zone.

### Always-on monitoring

An agent on each node keeps watching while SparkBox is closed. The heading shows how many are watching, such as "4 of 4 watching". [The always-on agent](help:always-on-agent) explains it.

Each node's row reads like "agent 1.2.0 · running · history 12 days · heartbeat 20 s ago".

| Button | What it does |
|---|---|
| **Install…** | Installs the agent on one node |
| **Install on all nodes…** | Installs it on every node, or **Install on 2 more nodes…** on those without one |
| **Reinstall…** | Sends the current agent, rules and channels again, and restarts it |
| **Remove…** | Deletes the service, its settings, the copied secrets and its history from the node |
| **Push again** | Appears when an agent's settings are out of date |
| **Send a test page from every node…** | Sends one test from each node through each phone channel. The table below shows the result |

### Expose metrics for Prometheus

Prometheus is a monitoring system that collects readings from a web address. With this on, each agent serves `/metrics` on its LAN address. The readings cover CPU, GPU, memory, disks, network, RoCE counters and firing alerts, each labeled with its node. It's off by default.

**Port** sets the port. **Bearer token** sets the password that scrapers must send. Without a token, only the node itself can read `/metrics`, and anything else is refused. The address reads like `http://<node>:<port>/metrics`.

## What it changes

| Action | What it changes | Needs | How to undo |
|---|---|---|---|
| Add, edit or remove a channel | This Mac's settings and Keychain | Admin mode, Touch ID or your Mac password | Change it back. After **Remove**, add the channel again and type its secrets again, because they've left the Keychain |
| Change a **When to notify** setting | This Mac's settings. Agents get it with the next push | Admin mode, Touch ID or your Mac password | Change it back |
| **Test** from this Mac | Sends one real message | Look-only mode off | |
| **Test** from a node's agent | Sends one real message from that node | Admin mode | |
| **Install…** | Adds a service on the node | Admin mode. Also Touch ID or your Mac password if it copies your secrets | **Remove…** |
| **Remove…** | Deletes the agent, its history and the copied secrets. It can't be undone, but you can install it again with no history | Admin mode | **Install…** |
| **Push again** | Sends settings to the agents | Admin mode. Also Touch ID or your Mac password if it copies your secrets | |
| **Expose metrics** | Changes what each agent serves | Admin mode, Touch ID or your Mac password | Turn it off |

With **Demo** data, the agents are simulated and nothing leaves this Mac, except a **Test**. Notifications on this Mac still appear. Each change goes in the [Activity log](help:activity-log).

## Related How-Tos

- [Get notifications on this Mac](help:get-notifications-on-this-mac)
- [Get alerts on your phone](help:get-alerts-on-your-phone)
- [Set up always-on monitoring](help:set-up-always-on-monitoring)
- [Respond to an alert](help:respond-to-an-alert)
