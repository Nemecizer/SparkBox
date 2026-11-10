---
id: settings-data-source
title: "Settings: Data source"
type: feature
area: settings
summary: Choose Demo or Live. Each keeps its own tasks, chats and terminal sessions.
keywords: [demo, live, data]
verified: false
related: [demo-and-live, switch-between-demo-and-live, save-the-cluster-password]
---

# Settings: Data source

## What it's for

This section chooses what SparkBox shows: a simulated cluster, or your real one. Use **Demo** to learn and practice. Use **Live** to work on your nodes. [Demo and Live](help:demo-and-live) explains the difference.

## Where to find it

- **Settings:** press <kbd>⌘,</kbd>, then click **Data source**.
- **Menu bar:** choose **Cluster › Data Source**, to switch without opening this section.
- **Command palette:** press <kbd>⌘K</kbd> and type `use live data` or `use demo data`.
- **Link:** [Open Data source settings](open:settings?section=data)


## What's on the screen

### Demo and Live cards

The two cards work like radio buttons. Click one to choose it.

- **Demo**: "A simulated twin of your four Sparks: the same hostnames, hardware and software, serving a model by default. Nothing connects anywhere; actions change only the simulation."
- **Live**: "Your Sparks over SSH (directly at home, through the VPN when away). Each runs a small read-only collector, ~/.sparkbox/collector.py."

A new SparkBox starts on **Demo**.

### The line under the cards

"Demo and Live keep their own tasks, chats and terminal sessions: switching shows the other side's."

### Needed for Live

This group appears when **Live** is chosen and no cluster password is saved. SparkBox can't log in to your nodes without one, unless you've [set up key login](help:set-up-key-login). Type the password in **Cluster password** and click **Save**. Once it's saved, a line says "The cluster password is saved. Connection details are in Nodes and Cluster login."

## What it changes

Your choice changes where SparkBox's numbers come from and where its commands go. It's a change on this Mac, so it asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes. It's saved, so SparkBox opens on the same choice next time.

Moving from Demo to Live also locks Admin mode, because a Demo unlock never asked who you are.

Switching changes nothing on your nodes. Work that runs by itself on a node, such as a loaded model, keeps running while SparkBox shows Demo. To undo, click the other card.

## Related How-Tos

- [Switch between Demo and Live](help:switch-between-demo-and-live)
- [Save the cluster password](help:save-the-cluster-password)
- [Connect your cluster](help:connect-your-cluster)
