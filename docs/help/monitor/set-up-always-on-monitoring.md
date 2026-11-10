---
id: set-up-always-on-monitoring
title: Set up always-on monitoring
type: howto
area: alerts
summary: Install SparkBox's agent on your nodes so that they keep watching while your Mac is closed.
keywords: [agent, 24/7, dead-man's switch, mac asleep, paging, install, test page]
verified: false
related: [always-on-agent, get-alerts-on-your-phone, settings-notifications]
---

# Set up always-on monitoring

To keep watching your cluster while your Mac is closed, install SparkBox's agent on each node and prove it with a test page.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. Your nodes must be online. With **Demo** data the agents are simulated, which is a safe way to look around first.
- You need a phone channel. [Get alerts on your phone](help:get-alerts-on-your-phone) shows how to add one.
- Each node needs Python 3.10 or newer. DGX OS has it.

The **agent** is a small service that runs on each node. [The always-on agent](help:always-on-agent) explains what it does.

> [!WARNING]
> **What this changes on each node.** It adds a service called `sparkbox-agent` in `/opt/sparkbox-agent`, running as its own user. It keeps 180 days of history on the node's disk. The service is in the docker group, which can do what root can, because the container rules need to read Docker. To undo it, click **Remove…**, which deletes the service, its settings, the copied secrets and its history.

## Steps

### Install the agents

1. Open **Settings**, then click **Notifications**. [Open Notifications settings](open:settings?section=notifications)
2. Scroll to **Always-on monitoring**. Each node has a row that reads **Not installed**.
3. Click **Install on all nodes…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. A dialog named **Install the always-on agent on 4 nodes?** says what will happen and lists any secrets that will be copied. Read it, then click **Install**. A message says "Progress is in Tasks."
5. Wait. Each row changes to something like "agent 1.2.0 · running · heartbeat 20 s ago". The heading then reads "4 of 4 watching".

### Let the agents use your phone channel

1. Scroll up to **Where alerts go**, and click **Edit** on your phone channel. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
2. Read the note about who can read the channel's secrets on the nodes. [Who can read a channel's secrets](help:get-alerts-on-your-phone#who-can-read-a-channels-secrets) explains it.
3. Turn off **Send from this Mac only (not copied to the nodes)**.
4. Click **Save**. Confirm with Touch ID or your Mac password if asked. SparkBox copies the channel to each agent.

### Send a test page

1. Scroll to the top of the section, to **Paging**. It lists what's still missing.
2. Click **Send a test page from every node…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A dialog says how many messages this sends: one for each node on each phone channel.
3. Click **Send test pages**.
4. Check your phone. A message arrives from each node, and a table under **Always-on monitoring** shows how long each took, such as "Delivered in 0.6 s".

### Add a dead-man's switch

A **dead-man's switch** covers what the agents can't: the whole house losing power or internet. It's a check at an outside service that pages you when it stops hearing from SparkBox.

1. Create a check at a service such as [healthchecks.io](https://healthchecks.io), and set it to expect a ping every minute or so. Copy its ping address.
2. In SparkBox, under **Paging**, click **Set up…** beside "No dead-man's switch". If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A form opens.
3. Paste the address into **Ping URL**. It goes to the Keychain.
4. Click **Add**. Confirm with Touch ID or your Mac password if asked.

The address must use `https://` unless it's on your home network.


## What you should see

- The badge in the top bar reads **24/7**.
- Every line under **Paging** has a checkmark.
- On the **Dashboard**, the top band adds **watched 24/7** when all is clear.
- A node's **System** tab reads "Watching", with its version and last heartbeat.

The badge shows what's still missing until it's all proven. It may read **This Mac only**, **Not paging**, **24/7 · untested**, **24/7 on 3 of 4** or **24/7 · no dead-man's switch**.

## Why it works

Each agent reads its own node, judges the same alert rules as this Mac, and sends pages itself, so your Mac can sleep. Agents also watch each other, so they can report a node that goes away. SparkBox checks that each agent reports the settings it was sent, and sends pages for that node itself until it does. "24/7" appears only once a test page has gone out from every node since your last change. That's the only proof that a page can reach you.

## Limits

- The agents aren't told about [maintenance mode](help:use-maintenance-mode), so they may still page during it.
- Some rules are judged on this Mac only. The [reference](help:alert-rules-reference) marks them.
- Switches and the gateway are watched only while this Mac is awake, apart from the fabric switch's management address, which one agent checks. **Paging** lists what isn't watched.

## If something goes wrong

- **A row reads "agent … · settings out of date (push failed 10:42: sudo refused)".** The node refused the update. Check the [cluster password](help:save-the-cluster-password), then click **Push again**. SparkBox also retries by itself while Admin mode is unlocked.
- **"Not answering" or "Not heard from".** The agent stopped. Click **Reinstall…** on its row.
- **"The last test page didn't reach Phone (ntfy) from cell2".** That node can't reach the service. Check the node's internet access. The table's cell has the reason.
- **"Not tested since the settings changed".** You changed a channel. Send a test page again.

See [Alert delivery and agent messages](help:errors-alerts-and-agent) for more.

## Next steps

- [The always-on agent](help:always-on-agent)
- [Respond to an alert](help:respond-to-an-alert)
- [Change alert rules](help:change-alert-rules)
