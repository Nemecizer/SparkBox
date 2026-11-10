---
id: errors-alerts-and-agent
title: Alert delivery and agent messages
type: troubleshooting
area: troubleshooting
summary: Messages from notification channels, test pages and the always-on agent.
keywords: [push failed, test failed, agent, email, tls, ntfy, pushover, webhook, smtp, heartbeat, deliveries, silence, reason]
verified: null
related: [set-up-always-on-monitoring, get-alerts-on-your-phone, respond-to-an-alert, always-on-agent, settings-notifications]
---

# Alert delivery and agent messages

This page covers messages from notification channels, test pages, silences and the always-on agent. A **channel** is one place alerts go. To **page** is to send an alert to your phone.

The **Deliveries** tab in [Alerts](open:alerts) records what happened to each message.

## Setting up a channel

### "Give the channel a name."

**What it means.** The name field is empty.

**What to do.** Type a name.

### "Phone: enter the ntfy topic."

**What it means.** ntfy needs a topic, the private channel name your phone follows. Other channels have their own forms of this message:

- "Pager: Pushover needs the app token and your user key."
- "Team: enter the webhook URL."
- "Heartbeat: enter the check's URL."

**What to do.** Fill in the field. See [Get alerts on your phone](help:get-alerts-on-your-phone).

### "Mail: email needs the SMTP server, a From address and a To address."

**What it means.** The form is missing one of those.

**What to do.** Fill them in. `Mail: "sparkuser" isn't an email address.` means an address isn't complete.

### "Mail: choose STARTTLS or SSL; SparkBox won't send a password unencrypted."

**What it means.** Without encryption, your mail password would cross the network in plain text.

**What to do.** Choose **STARTTLS** or **SSL**, as your mail provider says.

### "Use https for ntfy.example.com: plain http would send the token and every alert unencrypted."

**What it means.** SparkBox won't send secrets by plain HTTP to a public address.

**What to do.** Use an `https` address.

### "ntfy" isn't a URL.

**What it means.** The address isn't complete. `"ftp://x" has to be an http or https URL.` is the same.

**What to do.** Type the full address.

### "Heartbeat: the dead-man's switch is pinged by the node agents, so it can't be this Mac only."

**What it means.** The dead-man's switch is a check that alerts you when messages stop. It works only from the nodes.

**What to do.** Turn off **Send from this Mac only (not copied to the nodes)** for this channel.

## Test messages and deliveries

### "HTTP 401 — check the token"

**What it means.** The service refused your secret. The word differs by channel, such as "check the app token and user key" for Pushover.

**What to do.** Save the secret again in [Notification settings](open:settings?section=notifications).

### "HTTP 404 — check the address"

**What it means.** That address doesn't exist.

**What to do.** Check it for mistakes.

### "HTTP 413 — the message was too large"

**What it means.** The service won't take a message this long.

**What to do.** Nothing, usually. Report it if it repeats.

### "HTTP 429 — rate limited; it will retry"

**What it means.** The service asked SparkBox to slow down. It tries again by itself. "HTTP 503 — the server had a problem; it will retry" is the same.

**What to do.** Wait.

### "Couldn't reach ntfy.sh: couldn't connect"

**What it means.** This Mac or node can't reach the service. "Couldn't reach ntfy.sh: no answer in 15 s" is the same. It retries by itself.

**What to do.** Check the internet connection of whichever one sent it. If it says **Sent by the agent on cell1**, that's the node.

### "SMTP 535 — check the user and password"

**What it means.** Your mail server refused the login. Many mail services need an app password, not your normal one.

**What to do.** Save the right password in the channel. "SMTP 550 — the server refused the address" means a From or To address was refused. "SMTP 421 — the server is busy; it will retry" retries by itself.

### "TLS with smtp.example.com failed — check STARTTLS or SSL and the port"

**What it means.** The secure connection to the mail server failed, usually from a mismatch between the choice and the port.

**What to do.** Use **STARTTLS** with port `587`, or **SSL** with port `465`.

### "Couldn't reach smtp.example.com: no answer in 20 s"

**What it means.** The mail server didn't reply.

**What to do.** Check the server's name and port.

## Silences

### "Say why: a silence for a week or longer needs a reason."

**What it means.** A long silence can hide a real problem, so SparkBox asks you to write down why. You may also see:

- "Say why: a silence until it clears needs a reason."
- "Say why: a silence of more than one alert needs a reason."

**What to do.** Type a short reason. It's kept with the silence.

### "Keep the reason under 200 characters."

**What it means.** The reason is too long.

**What to do.** Shorten it.

### "A silence has to name an alert or an incident."

**What it means.** Nothing was selected.

**What to do.** Select an alert first.

## The always-on agent

The always-on agent is SparkBox's small service on a node. See [The always-on agent](help:always-on-agent).

### "No node has the agent installed yet."

**What it means.** There's no agent to test or push to.

**What to do.** Follow [Set up always-on monitoring](help:set-up-always-on-monitoring).

### "Add a phone channel first: the agents have nothing to send a test to."

**What it means.** The agents can only use channels copied to them.

**What to do.** Add one. See [Get alerts on your phone](help:get-alerts-on-your-phone).

### "A test page went out less than a minute ago. Wait a moment and try again."

**What it means.** SparkBox limits test pages, so that your phone isn't flooded.

**What to do.** Wait a minute.

### "The service didn't report a heartbeat on cell2. See `journalctl -u sparkbox-agent` there."

**What it means.** The install ran, but the agent isn't reporting.

**What to do.** Run `journalctl -u sparkbox-agent -n 50` on that node in [Fleet Command](open:fleet), and read the last lines.

### "systemd reports the service failed; it restarts it every 5 s"

**What it means.** The agent keeps crashing, and systemd, the part of Ubuntu that watches services, keeps restarting it.

**What to do.**

1. Run `journalctl -u sparkbox-agent -n 50` on that node in [Fleet Command](open:fleet), and read the last lines.
2. Open [Notification settings](open:settings?section=notifications). Under **Always-on monitoring**, click **Reinstall…** in that node's row.
3. If it still fails, click **Remove…**, then **Install…**. **Remove…** also deletes the agent's history on that node.

### "the install script failed"

**What it means.** A step of the install failed on the node. "What SparkBox sent arrived incomplete; nothing was installed." and "A file SparkBox sent arrived damaged; nothing was installed." mean the link caused it.

**What to do.** Check that sudo works, with [Check that your nodes are ready](help:check-your-nodes). Then try again.

### "The agent on cell2 didn't answer. Is it installed and running?"

**What it means.** A test page couldn't start on that node. "The agent didn't answer in 20 s. Is sparkbox-agent running?" is the same.

**What to do.** Check the node's row under **Always-on monitoring** in [Notification settings](open:settings?section=notifications).

### "The agent didn't send any test."

**What it means.** The agent ran, but it had nothing to send. You may also see:

- "The agent didn't report a result."
- "The agent's answer wasn't readable."

**What to do.** Click **Push again**, then send the test again.

### "2 of 4 failed"

**What it means.** Part of a test page from every node failed. "4 of 4 delivered" means all arrived.

**What to do.** Read which node and channel failed. Fix that one first.

### "has older settings than this Mac"

**What it means.** You changed alert settings, and the agent hasn't got them yet. You may also see:

- "set up before this version of SparkBox: push the settings", which is the same
- "agent 1.2 doesn't report its settings: reinstall it", which means it's too old

**What to do.** Click **Push again**, or **Install…** for an old agent.

### "push failed: …"

**What it means.** Sending settings to the agent failed. While Admin mode is unlocked, SparkBox retries by itself when the node is back, and later as well.

**What to do.** Check that the node is online, then click **Push again**.

### "A push to cell2 is already running."

**What it means.** One push runs at a time.

**What to do.** Wait for it.

### "The always-on agent stopped reporting · last heartbeat 12 min ago"

**What it means.** This alert means the agent went quiet, while the node may be fine.

**What to do.** Check its row in [Notification settings](open:settings?section=notifications). If it's stopped, run `journalctl -u sparkbox-agent -n 50` on that node in [Fleet Command](open:fleet), and read the last lines.

### "the agent's last delivery failed"

**What it means.** The agent couldn't send its most recent message. "3 message(s) waiting in its outbox since 10:42" means it's holding them to send later.

**What to do.** Check that node's internet connection, and the channel.

### "maintenance note for cell2 not sent: …"

**What it means.** The agent wasn't told about planned work, so it may page you.

**What to do.** Ignore that alert. The note is sent again when the node can be reached.

### "no request key yet: push the alert settings first"

**What it means.** The agents need settings from SparkBox before they accept requests from it.

**What to do.** Click **Push again**.

### "Use two different ports from 1024 up."

**What it means.** The metrics port and the agent's own port must differ, and be `1024` or higher.

**What to do.** Pick another **Port**.

### "agents.json wasn't saved: …"

**What it means.** SparkBox couldn't write its record on this Mac.

**What to do.** Check this Mac's free disk.

### "Demo — the agent on cell2 is simulated, so nothing was sent."

**What it means.** You're using Demo data.

**What to do.** Switch to Live data for a real test.
