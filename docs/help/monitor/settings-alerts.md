---
id: settings-alerts
title: "Settings: Alerts"
type: feature
area: settings
summary: Every alert rule, with its warning and critical lines, how long it must hold, and per-node overrides.
keywords: [rules, thresholds, would fire now, per node, reset]
verified: false
related: [change-alert-rules, alert-rules-reference]
---

# Settings: Alerts

## What it's for

**Settings › Alerts** is where you decide when SparkBox raises an alert. Each rule says what to watch, the value that counts as a problem, and how long it must last. Use it when an alert fires too often, or when you want an earlier warning.

## Where to find it

- **Settings:** open **Settings**, then click **Alerts**. To open Settings, press <kbd>⌘,</kbd> or use the **SparkBox** menu.
- **Command palette:** press <kbd>⌘K</kbd>, type `Alerts`, then choose **Settings › Alerts**.
- **Link:** [Open Alerts settings](open:settings?section=alerts)

## What's on the screen


### The toolbar

| Item | What it does |
|---|---|
| **Saved** or **Save 2 changes** | Saves your edits. It reads **Saved** when there's nothing to save |
| **Revert** | Throws away edits you haven't saved |
| **Would fire now: 3** | How many alerts these rules raise right now. See below |
| **Reset to defaults…** | Puts every rule back as SparkBox ships it. Nothing is saved until you click **Save** |

### Would fire now

**Would fire now** checks your edits against the cluster as it is this moment, before you save anything. The figure at the top counts every alert, at once or after its **For** time. It updates a moment after each edit, and every 5 seconds.

Under a rule, the same words list what would fire, such as "cell2 · in 2 min". Gray text such as "Not reported by any node" means no current reading exists for the rule, so it can't fire. **Per node…** shows which nodes.

### The areas

The rules appear in groups: **Nodes**, **Disk**, **GPU**, **Fabric**, **Containers & services** and **Serving**. The [Alert rules reference](help:alert-rules-reference) describes every rule. RoCE, the fast path over the fabric, is one rule under **Fabric**.

### A rule's row

| Part | What it does |
|---|---|
| The switch | Turns the rule on or off |
| The rule's name, with ⓘ | The ⓘ says what the rule watches and what to do. **Read the full guide** opens the rule's entry in the [Alert rules reference](help:alert-rules-reference), with its lines and when it clears |
| **For** | How long the condition must hold before the alert fires. Type a time such as `60 s`, `2 min` or `1 h`. Zero reads **at once** |
| **Warning**, **Critical** | The lines. Leave a field empty for no line at that level. A rule with a dash has no line to move: SparkBox decides it another way, and only its **For** time can change |
| **Per node…** | Different lines for one node, below |

Lines are judged on what the app shows, so a disk reading 85% is at the 85% line. A changed row is marked until you save.

### Per node

**Per node…** opens a small panel with one row for each node.

- **Warning** and **Critical** set that node's own lines. Leave them empty to use the rule's lines.
- **On** turns the rule on or off for that node alone.
- "Not reported by this node" means the node has no such reading.
- **Clear all** removes the node differences. **Done** closes the panel.

The button then reads **Per node… (1)**, with how many nodes differ.

### Counters

**RoCE on a fabric interface** watches 13 counters at once. Click **13 counters** to open them. Each has its own switch and its own lines, in events per second.

## What it changes

Changes apply to this Mac and to the node agents. Editing changes nothing until you save.

| Action | What it changes | Needs | How to undo |
|---|---|---|---|
| **Save** | Saves the rules on this Mac, sends them to any always-on agents, and records "Alert rules changed" in the **Activity log** | Admin mode, Touch ID or your Mac password | Change them back, or use **Reset to defaults…** and save |
| **Revert** | Throws away unsaved edits | Nothing | |
| **Reset to defaults…** | Fills in the shipped rules, including per-node overrides | Nothing until you save | **Revert** |

The same rules apply to **Demo** and **Live**.

## Related How-Tos

- [Change alert rules](help:change-alert-rules)
- [Respond to an alert](help:respond-to-an-alert)
- [Set up always-on monitoring](help:set-up-always-on-monitoring)
