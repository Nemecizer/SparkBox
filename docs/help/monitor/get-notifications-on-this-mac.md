---
id: get-notifications-on-this-mac
title: Get notifications on this Mac
type: howto
area: alerts
summary: Turn on macOS notifications, and choose what reaches you and when, including quiet hours.
keywords: [notifications, quiet hours, banner, mac alert, do not disturb, notification center, at night, overnight, night, alerts at night]
verified: false
related: [settings-notifications, get-alerts-on-your-phone, respond-to-an-alert]
---

# Get notifications on this Mac

To see alerts as macOS notifications, turn on **This Mac** in **Settings › Notifications**, then choose what reaches you.

## Before you start

- SparkBox must be running, and this Mac awake. To get alerts while it's asleep, read [Get alerts on your phone](help:get-alerts-on-your-phone) and [Set up always-on monitoring](help:set-up-always-on-monitoring).
- Be ready to unlock Admin mode, with look-only mode off. To change a setting, SparkBox asks for Touch ID or your Mac password, because these settings decide what can reach you.
- **Test** needs no Admin mode, but it's off in look-only mode.
- With **Demo** data, a simulated alert can show a notification on this Mac too, because nothing leaves this Mac. **Test** shows a real one.

**This Mac** is on by default, so you may only need to test it.

## Steps

### Check that it works

1. Open **Settings**, then click **Notifications**. [Open Notifications settings](open:settings?section=notifications) The first group under **Where alerts go** is **This Mac**.
2. Check that its switch, **Show notifications on this Mac**, is on. If it's off, click it and confirm with Touch ID or your Mac password.
3. On the **This Mac** row, click **Test**. SparkBox says "Test sent to This Mac".
4. If macOS asks whether SparkBox may send notifications, allow it. It asks only the first time.
5. Look at the top right of your screen. A sample notification appears.

### Choose what reaches you

1. Scroll to **When to notify**.
2. To get only the most serious alerts, turn on **Critical alerts only**. Warnings then wait in the **Alerts** window.
3. To hear when a problem ends, leave **When an alert clears** on.
4. Under **Repeat critical alerts**, choose how often a critical alert reminds you: **15 min**, **30 min**, **1 h**, **4 h** or **Never**. The default is **1 h**.
5. Turn off any area you don't want to hear about: **Node status**, **Disk**, **GPU**, **Fabric & switches**, **Containers, services & models** or **Tasks**.

### Set quiet hours

1. In the same group, turn on the switch at the right of **Quiet hours**. The two time fields become active.
2. Type the start and end times. The defaults are 22:00 to 07:00.

During quiet hours only critical alerts and their clears go out. Warnings wait. When the hours end, the warnings still firing arrive as one summary, such as "While quiet: 2 warnings still firing".

## What you should see

- A notification appears when an alert fires. Click it, and SparkBox opens what it's about.
- If SparkBox comes to the front some other way soon after, it offers "Last notification · Open".
- Under **Quiet hours**, the hint shows your hours and time zone, such as "22:00 to 07:00 · America/New_York".

## Why it works

One set of rules decides what goes out, and it's the same for this Mac and for your phone. Critical alerts always go out, and warnings wait for quiet hours to end. SparkBox groups related alerts, so one problem doesn't send a burst of messages. Each decision is written down in **Alerts › Deliveries**, so a notification that didn't appear says why. Quiet hours follow the time zone this Mac had when you saved them, and a note with **Use** offers the new zone if you travel.

## If something goes wrong

- **Test says it was sent, but nothing appears.** macOS may be blocking SparkBox. Open **System Settings › Notifications**, select **SparkBox**, and turn on **Allow Notifications**. Check **Focus** too, because a Focus can hide notifications.
- **A switch doesn't change.** You canceled Touch ID, or look-only mode is on. See [Admin, look-only and safety messages](help:errors-admin-and-safety).
- **Test is dimmed.** Look-only mode is on, and it turns off everything in this section. [Use look-only mode](help:use-look-only-mode) shows how to turn it off.
- **"Wait a few seconds before testing that channel again."** Each channel accepts one test every 10 seconds.
- **An alert didn't notify you.** Open [Alerts](open:alerts), click **Deliveries**, then click **Show not paged**. The **Outcome** says why.

## Next steps

- [Get alerts on your phone](help:get-alerts-on-your-phone)
- [Respond to an alert](help:respond-to-an-alert)
- [Settings: Notifications](help:settings-notifications)
