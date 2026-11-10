---
id: demo-and-live
title: Demo and Live
type: concept
area: start
summary: Demo is a simulated cluster where nothing is real. Live is your real Sparks over SSH.
keywords: [demo, live, simulator, fake, real]
verified: null
related: [switch-between-demo-and-live, settings-data-source, tour-the-demo]
---

# Demo and Live

## In short

SparkBox has two data sources. **Demo** is a simulated copy of your cluster, built into the app. **Live** is your real nodes, reached over SSH. A label in the top bar always shows which one you're looking at, and only Live can touch a real node.

## How it works

### Demo

Demo is a simulator. The native app simulates the inventory you configure; on a fresh installation that list is empty. The browser Demo uses a synthetic four-node example. No example inventory is copied into a native installation. The numbers move, the charts draw and the alerts fire, but nothing connects to any node.

When you act in Demo, the simulation changes and your nodes don't. If you restart `cell2` in Demo, the simulated `cell2` goes down and comes back. The real one never hears about it. You can also play scripted faults, such as a link failure, from [Settings › Demo scenarios](help:settings-demo-scenarios).

### Live

Live is the real thing. SparkBox logs in to each node over SSH, directly at home and through the VPN when you're away. It starts a small read-only helper on each node, `~/.sparkbox/collector.py`, which sends a reading once a second. What you do in Live happens on your nodes.

### What differs

| | Demo | Live |
|---|---|---|
| Nodes, containers and models | Simulated | Real |
| Unlocking Admin mode | Asks for nothing | Asks for Touch ID or your Mac password |
| Changes on this Mac | Ask for Touch ID or your Mac password | The same |
| Tasks, chats and terminal sessions | Its own | Its own |
| **Demo scenarios** | Available | Not available |
| **Fence**, **Capture evidence** and **Run doctor** | "Available with Live data" | Available |

### What's real in both

Demo simulates your cluster, not your Mac. These things are real, whichever you use:

- **Your Keychain.** A password you save in Demo really is saved.
- **Your node list.** Demo and Live share one list. If you remove a node in Demo, it's gone from Live too.
- **Your settings.** Changes in **Settings** apply to both.
- **The internet.** A Hugging Face search reaches the real website, and a chat with a cloud model reaches the real provider.

That's why changes on this Mac ask for Touch ID or your Mac password, even in Demo.

### An example

Say you want to practice restarting a node. In Demo, you click the padlock and Admin mode unlocks at once. You restart `cell3` and watch it go down and come back. Now you know what to expect. In Live, the same click asks for Touch ID, shows the same dialog, and restarts the real `cell3`.

## Why it matters in SparkBox

A new SparkBox starts in Demo, so you can learn it with nothing at risk. [Take a tour of the Demo](help:tour-the-demo) shows you around.

Before you do anything important, glance at the top bar. A gray **Demo** label means nothing you do reaches a node. A **Live** label means it does.

One thing to watch for: the checks in [Connect my Sparks](help:connect-my-sparks) are simulated while you're in Demo, and the wizard says so. To check your real nodes, switch to Live first.

Switching asks for Touch ID or your Mac password. When you move to Live, Admin mode also locks, because a Demo unlock never asked who you are. [Switch between Demo and Live](help:switch-between-demo-and-live) has the steps.

## Related

- [Switch between Demo and Live](help:switch-between-demo-and-live)
- [Take a tour of the Demo](help:tour-the-demo)
- [Settings: Data source](help:settings-data-source)
- [Settings: Demo scenarios](help:settings-demo-scenarios)
