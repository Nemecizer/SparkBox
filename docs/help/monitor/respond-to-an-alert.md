---
id: respond-to-an-alert
title: Respond to an alert
type: howto
area: alerts
summary: Find out what an alert means, then acknowledge it, silence it or fix it.
keywords: [alert, acknowledge, silence, warning, critical, mute, what do i do]
verified: false
related: [alerts-window, alerts-and-incidents, alert-rules-reference]
---

# Respond to an alert

To deal with an alert, find out what it means, then fix it, or tell SparkBox you know about it.

## Before you start

- You can read alerts in any mode.
- To acknowledge or silence one, you need [Admin mode](help:unlock-admin-mode). With **Live** data, SparkBox asks for Touch ID or your Mac password when it unlocks. With **Demo** data, a short silence needs no password.
- Look-only mode lets you read alerts but not change them.
- An **alert** is one problem SparkBox found. An **incident** is a group of alerts with one cause. A broken cable can raise several alerts, and they show as one incident.

## Steps

### Find out what it means

1. In the sidebar, under **Overview**, click **Alerts**. [Open Alerts](open:alerts) The **Alerts** window opens on its **Active** tab.
2. Find the alert under **Firing**. **Affects** says what's wrong, and **What** says how. **Since** says how long it's been going.
3. If the row has a small arrow, click it to see the other alerts in the incident.
4. Double-click the row. The window for what's affected opens, such as the node's **Disks** tab for a full disk.
5. Look up the rule in the [Alert rules reference](help:alert-rules-reference). It says what the alert means and what to do.

### Fix it, if you can

1. Follow the advice for that rule. For example, free up space for a full disk, or check the cable for a link that's down.
2. Wait. When the condition goes away, the alert moves to **Recovering** with a countdown, then clears.

### If you can't fix it yet, acknowledge it

Acknowledge an alert when you've seen it and you're dealing with it.

1. Click the row to select it.
2. In the toolbar, click **Acknowledge**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A message confirms it, and the alert moves to **Acknowledged**.

It stops counting and stops repeating. If it gets worse, it comes back by itself.

### If you expect it, silence it

Silence an alert when you know about it and don't want to hear about it for a while.

1. Click the row to select it.
2. In the toolbar, click **Silence ▾**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A menu lists how long: **1 hour**, **1 day**, **1 week…**, **Until it clears…** and **Indefinitely…**.
3. Click a length. The **Silence** dialog opens.
4. To silence the same rule on every node, choose the second option under **What**. Leave it on **This incident** to silence only this one.
5. Type a **Reason**. It's optional for one alert for an hour or a day, and required for anything wider or longer. The reason goes in the activity log, for whoever reads it later.
6. Click **Silence**. With **Live** data, a week or longer, until it clears, or every node also asks for Touch ID or your Mac password.

> [!WARNING]
> **A silenced alert can't reach you.** It isn't counted and isn't sent to your phone, even if the problem gets worse. To undo it, open the **Silenced** tab, select the silence and click **End silence**.


## What you should see

- **Acknowledged:** the row moves to the **Acknowledged** group, and the count on the sidebar falls by one.
- **Silenced:** the row leaves **Active** and appears on the **Silenced** tab, with when it ends and what's firing under it.
- **Fixed:** the row reads **Recovering · clears in 1:34**, then moves to **History**.

## Why it works

SparkBox tells apart three things: what's wrong now, what you know about, and what you've chosen not to hear. Only what's wrong and not yet known counts in the sidebar, the top bar and the **Dashboard**. So a count of zero means nothing new needs you. [Alerts and incidents](help:alerts-and-incidents) explains the difference.

## If something goes wrong

- **"Say why: this silence needs a reason."** A silence that long or that wide needs a reason. Type one in **Reason**.
- **"Couldn't acknowledge it" or "Couldn't end the silence".** The message says why. See [Alert delivery and agent messages](help:errors-alerts-and-agent).
- **The alert came back after you acknowledged it.** It got worse, for example from a warning to critical. That's on purpose.
- **Your phone still gets pages during maintenance.** The node agents aren't told about maintenance mode, so they may still page. See [Use maintenance mode](help:use-maintenance-mode).

## Next steps

- [Change alert rules](help:change-alert-rules), to quiet an alert that fires too often
- [Get alerts on your phone](help:get-alerts-on-your-phone)
- [Find out why a node restarted](help:find-out-why-a-node-restarted)
