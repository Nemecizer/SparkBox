---
id: alerts-and-incidents
title: Alerts and incidents
type: concept
area: alerts
summary: How a reading becomes an alert, how related alerts become one incident, and what acknowledge and silence do.
keywords: [alert, incident, severity, silence, acknowledge, planned, flapping, recovering, page]
verified: null
related: [alerts-window, respond-to-an-alert, alert-rules-reference, always-on-agent]
---

# Alerts and incidents

## In short

An **alert** is one problem SparkBox has found and confirmed, such as a full disk. An **incident** is a group of alerts with one cause, counted as one thing. Every count in SparkBox counts incidents, so one broken cable reads as one problem, not five.

## How it works

### From a reading to an alert

SparkBox reads your nodes every second and judges each reading against its [alert rules](help:alert-rules-reference). An alert moves through these stages:

| Stage | What it means |
|---|---|
| **Confirming** | The condition appeared, but it hasn't held long enough. A rule's **For** time decides how long. A one-second blip never becomes an alert |
| **Firing** | The condition held. The alert counts, and is sent to you as your settings say |
| **Recovering** | The condition went away. The alert waits out its clear time, so a problem that comes and goes doesn't alert twice |
| **Cleared** | It stayed gone. The alert moves to **History** |

Some rules also have a gap between raising and clearing. Available memory warns below 4 GiB, but stays raised until more than 5 GiB is available for 30 seconds. So a reading that moves up and down around the warning line does not alert over and over.

### Warning and critical

Each alert has a severity. A **warning** means something needs attention soon. A **critical** alert means something is wrong now and may stop work. Information is shown in **Alerts** only, and is never counted or sent.

### From alerts to an incident

Problems rarely come alone. Suppose an optic, the small module at the end of a fiber cable, starts to fail on `cell2`. SparkBox sees rising errors on the node, errors on the switch port, and weak light on the optic. Those are three alerts with one cause, so they become one incident: "cell2 half 2". You get one page and one line in the count.

A node going away works the same way. Its fabric links, its switch port and any model running on it join one incident under that node.

### Flapping

Some problems come and go. An alert raised again within 10 minutes of clearing, twice in 30 minutes, is **flapping**. It reads "Flapping · raised 3 times in 10 min". It stays one incident, clears only after 10 quiet minutes, and doesn't page you each time.

### Acknowledge and silence

Both tell SparkBox you know about an alert, but they do different things.

| | **Acknowledge** | **Silence** |
|---|---|---|
| **Means** | I've seen this and I'm on it | I know about this, and I don't want to hear about it for now |
| **Lasts** | Until it clears | 1 hour, 1 day, 1 week, until it clears (7 days at most), or indefinitely |
| **If it gets worse** | It comes back and counts again | It stays quiet |
| **Counted** | No | No |
| **Repeats** | Stop | Stop |
| **Needs a reason** | No | For anything wider than one alert, or longer than a day |

Both make an alert a **known issue**. The **Dashboard** shows known issues as one quiet line, such as "2 known issues".

### Planned work

When SparkBox restarts or shuts down a node, the alerts that follow are expected. They show as **Planned**, counted nowhere and sent to nobody. If the restart runs well past its expected time, they count again and "Didn't come back after the restart SparkBox started" goes critical. You can mark your own work the same way with [maintenance mode](help:use-maintenance-mode).

### What reaches you

To **page** means to send an alert to your phone. One decision applies to every alert event:

- Critical alerts always page, even in quiet hours.
- Warnings wait during quiet hours, or if you chose critical alerts only.
- A warning in an incident paged in the last 30 minutes joins that incident's next message.
- The same alert raised again within 5 minutes at the same severity is a duplicate, and isn't sent again.
- A clear goes out once it has stayed clear for 10 minutes. A node back online goes out at once.
- An unacknowledged critical alert repeats "Still firing" as often as you set. The default is every hour.

Every event's outcome appears in **Alerts › Deliveries**, so a page that wasn't sent says why.

## Why it matters in SparkBox

Colors and counts mean the same thing everywhere. A node that's amber in the sidebar is amber on the **Dashboard**, in **Topology** and in **Alerts**, because SparkBox judges each reading once, and every window shows that one result.

A count of zero means nothing new needs you. It doesn't mean nothing is wrong: check for "known issues" if you've silenced anything.

## Related

- [Respond to an alert](help:respond-to-an-alert)
- [Alerts](help:alerts-window)
- [Alert rules reference](help:alert-rules-reference)
- [Change alert rules](help:change-alert-rules)
- [The always-on agent](help:always-on-agent)
