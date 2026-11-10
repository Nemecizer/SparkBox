---
id: switch-between-demo-and-live
title: Switch between Demo and Live
type: howto
area: start
summary: "Change what SparkBox shows: the simulated cluster, or your real one."
keywords: [demo, live, data source, change mode]
verified: false
related: [demo-and-live, settings-data-source, save-the-cluster-password]
---

# Switch between Demo and Live

To change what SparkBox shows, choose **Demo** or **Live** in **Settings › Data source**.

## Before you start

- Look-only mode must be off.
- Switching is a change on this Mac. SparkBox asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes.
- To use Live, [save the cluster password](help:save-the-cluster-password) first. Without it, SparkBox can't log in to your nodes.
- Switching doesn't change your nodes. A model loaded on a node keeps running while SparkBox shows Demo.

## Steps

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Data source settings](open:settings?section=data)
2. In the list on the left, click **Data source**. You see two cards, **Demo** and **Live**. The one in use is marked.
3. Click the card you want. SparkBox asks for Touch ID or your Mac password. Confirm it.
4. Look at the top bar. The label beside the padlock reads **Demo** or **Live**.

You can also switch from the menu bar. Choose **Cluster › Data Source**, then **Demo (Simulated Cluster)** or **Live (Your Sparks over SSH)**. In the command palette, press <kbd>⌘K</kbd> and type `use live data` or `use demo data`.

## What you should see

**When you switch to Live,** each node in the sidebar shows **Connecting**, then its GPU use, such as **GPU 28%**, as it answers. The first time, a node may show **Error** with "Host key not trusted yet". That's normal: follow [Trust a node's host key](help:trust-a-host-key).

Admin mode also locks. A message appears: "Admin mode locked", with "Live data is on: unlocking now asks for Touch ID or your Mac password."

**When you switch to Demo,** the simulated nodes appear at once, all online.

Each side keeps its own tasks, chats and terminal sessions. Switching shows the other side's, and switching back brings yours back.

## Why it works

The data source decides where SparkBox's numbers come from, and where its commands go. In Demo, both stay inside the app. In Live, SparkBox logs in to each node over SSH and starts its read-only helper there.

A Demo unlock never asks who you are, so moving to Live locks Admin mode. That way, a click you meant for the simulator can't reach a real node.

## If something goes wrong

- **A group named Needed for Live appears, with "SparkBox can't log in to the nodes until the cluster password is in the Keychain."** Type the password in that group, then click **Save**.
- **A node shows Error with "Host key not trusted yet".** Follow [Trust a node's host key](help:trust-a-host-key).
- **A node shows Error with "Login failed".** The saved password or key was refused. Check [Settings › Cluster login](help:save-the-cluster-password).
- **A node shows Offline.** SparkBox can't reach it. Check that it's on and on your LAN. If you're away from home, [connect the VPN](help:connect-the-vpn).
- **"Couldn't switch to Live".** The message gives the reason. If it says look-only mode is on, [turn it off](help:use-look-only-mode) first.

[Connection and login messages](help:errors-connection) lists the rest.

## Next steps

- [Check that your nodes are ready](help:check-your-nodes)
- [Unlock Admin mode](help:unlock-admin-mode)
- [Read the Dashboard](help:read-the-dashboard)
