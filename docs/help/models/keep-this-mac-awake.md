---
id: keep-this-mac-awake
title: "Keep this Mac awake during long work"
type: howto
area: models
summary: "Stop this Mac sleeping during a download, a copy or a load, and see what closing the lid does."
keywords: ["keep awake", "keep my mac awake", "stay awake", "stop my mac sleeping", "mac goes to sleep", "mac sleeps", "prevent sleep", "sleep settings", "energy settings", "battery settings", "close the lid", "close my laptop", "closed lid", "lid closed", "clamshell", "laptop", "asleep", "computer sleeps", "computer goes to sleep", "fell asleep"]
verified: null
related: [download-a-model, load-a-model-on-one-node, copy-a-model-to-other-nodes, model-questions]
---

# Keep this Mac awake during long work

To stop this Mac sleeping during a download, a copy or a load, turn on one macOS setting, keep it plugged in, and leave the lid open.

## Before you start

- **You need this before long work that SparkBox runs from this Mac.** SparkBox doesn't keep this Mac awake during a download, a copy or a load. A load can take up to 90 minutes, and a download's time depends on your internet speed.
- **This Mac must be plugged in.** The setting on a MacBook works only on the power adapter. A MacBook on battery can still sleep.
- **This changes a setting on this Mac, and nothing on your nodes.** You don't need Admin mode, and SparkBox doesn't have to be open. This Mac may use more power while the setting is on.
- These are the names in macOS 27. Another version of macOS may word them differently.

## Steps

### On a MacBook

1. Plug this Mac into power.
2. Click the Apple menu at the left of the macOS menu bar, then click **System Settings**. The **System Settings** window opens.
3. In the list on the left, click **Battery**. You may need to scroll down.
4. Click **Options…**. A dialog opens.
5. Turn on **Prevent automatic sleeping on power adapter when the display is off**.
6. Click **Done**.
7. Leave the lid open until the work ends. [What closing the lid does](#what-closing-the-lid-does) explains why.

### On a desktop Mac

1. Click the Apple menu at the left of the macOS menu bar, then click **System Settings**. The **System Settings** window opens.
2. In the list on the left, click **Energy**. You may need to scroll down.
3. Turn on **Prevent automatic sleeping when the display is off**.

## What you should see

The setting stays on after you close **System Settings**. The display can still turn off, and the screen can still lock. This Mac keeps running behind them, so when you come back, the **Progress** column on the **Jobs** tab has moved on.

Admin mode locks when the screen locks. That doesn't stop work that's already running. You unlock it again only to make the next change.

To undo this, turn the same setting off when the work ends.

## Why it works

SparkBox reaches your nodes over connections that stop when this Mac sleeps. macOS puts a Mac to sleep after it has sat idle for a while. This setting turns off that automatic sleep, so the display can go dark while this Mac keeps working.

## What closing the lid does

**Closing a MacBook's lid puts it to sleep, even when it's plugged in and this setting is on.** The setting stops only automatic sleep.

There's one exception. A MacBook keeps running with its lid closed if it's connected to power, an external display, and an external keyboard and mouse. See [Apple's page on external displays](https://support.apple.com/en-us/102501).

A desktop Mac has no lid, so none of this applies to it.

## When SparkBox keeps this Mac awake by itself

With **Live** data, SparkBox holds off automatic sleep while a node restarts or shuts down, and during a Fleet Command run that may last more than a minute or that changes nodes one at a time. The status bar then says so, as in "Keeping this Mac awake while Restart cell2 runs". Closing the lid still sleeps this Mac. For the rest, including a download, a copy and a load, use the setting on this page.

## If something goes wrong

- **This Mac slept anyway.** Wake it, then open SparkBox. What happens next depends on the work:
  - **A download or a copy:** look at the **Jobs** tab in [Models](open:models?tab=jobs). See [Follow a download](help:follow-a-download).
  - **A load on one node:** see [If this Mac sleeps or SparkBox closes](help:load-a-model-on-one-node#if-this-mac-sleeps-or-sparkbox-closes).
  - **A load on several nodes:** it can't finish if it was still starting its nodes. See [If this Mac sleeps or SparkBox closes](help:load-a-model-on-several-nodes#if-this-mac-sleeps-or-sparkbox-closes).
- **The list has no Battery.** This Mac is a desktop. Follow [On a desktop Mac](#on-a-desktop-mac).
- **The list has Battery, but there's no Options… button.** Your macOS may use other names. Click the search box at the top of **System Settings** and type `sleep`.

## Next steps

- [Download a model](help:download-a-model)
- [Load a model on one node](help:load-a-model-on-one-node)
- [Common questions about models](help:model-questions)
