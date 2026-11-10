---
id: change-alert-rules
title: Change alert rules
type: howto
area: alerts
summary: Change when an alert fires, for all nodes or one, and see what would fire before you save.
keywords: [thresholds, rules, tune alerts, too many alerts, lines]
verified: false
related: [settings-alerts, alert-rules-reference, respond-to-an-alert]
---

# Change alert rules

To change when an alert fires, edit its rule in **Settings › Alerts**, check **Would fire now**, and save.

## Before you start

- Be ready to unlock Admin mode, with look-only mode off. To save, SparkBox asks for Touch ID or your Mac password, because a rule decides what can reach you.
- Rules apply to both **Demo** and **Live**.
- Read what the rule does first, in the [Alert rules reference](help:alert-rules-reference). This guide uses the disk rule as its example.

> [!WARNING]
> **A rule set too loose can hide a real problem.** Turning off a rule, or raising a line too far, means you won't hear about it. A Spark that runs out of unified memory can freeze, so think twice before you loosen **Unified memory**. If a node does freeze, follow [Recover a frozen node](help:recover-a-frozen-node). To undo a rule change, change the line back, or use **Reset to defaults…**.

## Steps

### Change a line for every node

1. Open **Settings**, then click **Alerts**. [Open Alerts settings](open:settings?section=alerts) The rules appear, grouped by area.
2. Find the rule. For this example, go to **Disk** and find **Root disk full**.
3. Click the **Warning** field and type the new line, such as `80`. Press <kbd>Return</kbd>. The toolbar button now reads **Save 1 change**.
4. Look at the rule's second line. If anything would fire with this line, it appears after **Would fire now:**, such as "cell3".
5. To change how long the condition must last, click the field after **For** and type a time, such as `2 min`.

### Change a line for one node

1. On the same row, click **Per node…**. A small panel opens, with one row for each node.
2. In the node's row, type its own **Warning** or **Critical** line. Leave a field empty to use the rule's line.
3. To turn the rule off for this node alone, turn off its **On** switch.
4. Click **Done**. The button now reads **Per node… (1)**.

### Save

1. Check **Would fire now** at the top. It counts what these rules raise right now, so a big number means a line may be too tight.
2. Click **Save 1 change**.
3. SparkBox asks for Touch ID or your Mac password. Confirm it. A message reads "Saved Root disk full" and "Node agents get the new rules too."


## What you should see

- The toolbar button reads **Saved**.
- With **Live** data, the new lines take effect at once. For example, a disk over the new line raises its alert.
- If you use always-on monitoring, each agent gets the rules. **Settings › Notifications** shows when they're all on the current settings.

## Why it works

Every line and time SparkBox uses is stored as data, so you can change them without changing the app. **Would fire now** runs your edited rules against the latest readings without saving them. That way you see the effect before it can page you. SparkBox keeps your edits when it updates, and adds any new rules.

## Limits

- Rules marked with a dash have no line to move. SparkBox decides them by its own logic, and only their **For** time and switch can change.
- A rule's clear line isn't shown as a field. The rule's entry in the [Alert rules reference](help:alert-rules-reference) gives it.
- The rules marked "This Mac only" in the [reference](help:alert-rules-reference) aren't judged by the node agents.

## If something goes wrong

- **"Not reported by any node" under a rule.** No node has this reading now, so the rule can't fire. For example, a rule about a model has nothing to judge until one is loaded.
- **Too many alerts after a change.** Click **Revert** if you haven't saved. If you have, click **Reset to defaults…**, then **Save**.
- **A node agent shows old settings.** Open [Notifications settings](open:settings?section=notifications) and click **Push again**. See [Alert delivery and agent messages](help:errors-alerts-and-agent).

## Next steps

- [Get alerts on your phone](help:get-alerts-on-your-phone)
- [Respond to an alert](help:respond-to-an-alert)
- [Alert rules reference](help:alert-rules-reference)
