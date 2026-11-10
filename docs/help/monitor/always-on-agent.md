---
id: always-on-agent
title: The always-on agent
type: concept
area: alerts
summary: A small service on each node that keeps watching and can reach your phone when your Mac is closed.
keywords: [agent, 24/7, service, paging, while away, leader, heartbeat]
verified: null
related: [set-up-always-on-monitoring, settings-notifications, get-alerts-on-your-phone]
---

# The always-on agent

## In short

SparkBox watches your cluster only while it's running on this Mac. The **always-on agent** is a small service you can install on each node. It keeps watching while your Mac sleeps, and it can send alerts to your phone by itself.

## How it works

### What it does

An agent runs on its node all the time, as a background service called `sparkbox-agent`. Every 2 seconds it reads the node with the same code SparkBox uses. Then it judges those readings against the same alert rules as this Mac, using the same logic to decide what to send.

It also:

- Keeps 180 days of history on the node.
- Checks each model its node serves, once a minute, with a one-token request. A **token** is a small piece of text.
- Watches the other nodes' agents, so that a node that goes away gets reported.
- Serves readings for Prometheus, a monitoring system, if you turn that on.

An agent only reads. It never changes anything on the node.

### Who sends a page

To **page** means to send an alert to your phone. For each alert, one side sends it:

- A node's agent pages for its own node while it's healthy and runs the current settings. **Healthy** means its heartbeat, the signal it sends to say it's alive, is under 60 seconds old.
- This Mac sends everything else, while SparkBox is running.

An agent with old settings may still page too. So you may sometimes get the same page twice, but you won't get none. SparkBox chooses a possible duplicate over silence.

For example, say you close your laptop and `cell3`'s disk fills up. The agent on `cell3` sees it, judges it against your rule, and sends the page through your ntfy channel. If `cell3` loses power, its agent can't say so. The other agents notice that it has gone, and one of them, the **leader**, pages you.

### When SparkBox opens again

SparkBox collects what the agents recorded. Alerts that fired meanwhile appear in **Alerts › History**, marked **While away**. Gaps in your charts fill in from the agents' history.

### What it can't cover

If the whole house loses power or internet, no agent can reach you. That's what a **dead-man's switch** is for: an outside service that pages you when the agents' regular pings stop. [Set up always-on monitoring](help:set-up-always-on-monitoring) shows how to add one.

### Secrets

A channel marked to send from this Mac only stays on this Mac. A channel you copy to the agents has its secrets on every node. There they're readable by the `sparkbox-agent` service, root and anyone in the docker group, which includes your login user. So give the agents credentials that can only send. [Who can read a channel's secrets](help:get-alerts-on-your-phone#who-can-read-a-channels-secrets) has the details.

## Why it matters in SparkBox

The badge in the top bar tells you how far you can trust the watch:

| Badge | What it means |
|---|---|
| **This Mac only** | No agents. Alerts reach you only while SparkBox runs on this Mac |
| **Not paging** | Agents are watching, but no phone channel can reach you from the nodes |
| **24/7 on 3 of 4** | One agent isn't answering, so that node is watched only while this Mac is awake |
| **24/7 · settings not on cell2** | An agent has old settings |
| **24/7 · paging failed** | A page failed in the last hour |
| **24/7 · untested** | No test page has gone out since your last change |
| **24/7 · no dead-man's switch** | All is well, but a power cut would go unnoticed |
| **24/7** | Proven: every agent is healthy and current, a test page got through from every node, and a dead-man's switch is set |

A rule marked "This Mac only" in the [reference](help:alert-rules-reference) isn't judged by the agents. The agents also aren't told about [maintenance mode](help:use-maintenance-mode), so they may still page during it.

## Related

- [Set up always-on monitoring](help:set-up-always-on-monitoring)
- [Get alerts on your phone](help:get-alerts-on-your-phone)
- [Settings: Notifications](help:settings-notifications)
- [Alerts and incidents](help:alerts-and-incidents)
