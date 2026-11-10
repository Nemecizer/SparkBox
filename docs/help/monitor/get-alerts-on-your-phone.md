---
id: get-alerts-on-your-phone
title: Get alerts on your phone
type: howto
area: alerts
summary: Add ntfy, Pushover, a webhook or email as a channel, and send a test message.
keywords: [phone, ntfy, pushover, email, webhook, slack, page, discord, smtp]
verified: false
related: [settings-notifications, set-up-always-on-monitoring, get-notifications-on-this-mac]
---

# Get alerts on your phone

To get alerts on your phone, add a channel in **Settings › Notifications** and send yourself a test.

## Before you start

- A **channel** is one place alerts go. Choose one: [ntfy](https://ntfy.sh) and [Pushover](https://pushover.net) are phone apps, a webhook posts to a web address such as Slack, and email uses your own mail account. To **page** means to send an alert to one of these.
- If you're not sure, start with ntfy. It needs only a phone app and a topic name, with no account on the public server.
- You need an account or app for the channel you choose. Those services are separate from SparkBox, and they have their own terms and prices.
- Be ready to unlock Admin mode, with look-only mode off. To add a channel, SparkBox asks for Touch ID or your Mac password, with **Demo** data too.
- A new channel sends from this Mac only. It reaches you only while SparkBox runs on this Mac. To get pages while this Mac is closed, finish here, then read [Set up always-on monitoring](help:set-up-always-on-monitoring).

### Who can read a channel's secrets

A channel's secrets are its tokens, keys, passwords and web addresses. SparkBox keeps them in the Keychain on this Mac, and never shows them again.

> [!WARNING]
> **A channel you copy to the node agents has its secrets on every node.** There they're readable by the `sparkbox-agent` service, root and anyone in the docker group, which includes your login user. To limit the risk, give the agents credentials that can only send: a publish-only ntfy token, an app password for email, a dedicated Pushover application. To undo it, remove the channel, and SparkBox deletes its secrets from the nodes with the next push.

**Send from this Mac only (not copied to the nodes)** is on for a new channel, so nothing leaves this Mac until you turn it off.

## Steps

Open the channel list first:

1. Open **Settings**, then click **Notifications**. [Open Notifications settings](open:settings?section=notifications)
2. Scroll to **Where alerts go**. The buttons at the end of the list add a channel: **ntfy**, **Pushover**, **Webhook** and **Email**.

Then follow the part for your channel.

### ntfy

1. On your phone, install the ntfy app and subscribe to a topic. Choose a long, hard-to-guess name, because on the public server anyone who knows a topic can read it.
2. In SparkBox, click **ntfy**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A form opens.
3. Leave **Server** as `https://ntfy.sh`, or type your own server's address.
4. In **Topic**, type the same topic name.
5. If your server needs one, type an **Access token**. It goes to the Keychain.
6. Click **Add channel**. Confirm with Touch ID or your Mac password if asked.

Critical alerts arrive at urgent priority, which can break through Do Not Disturb on the phone.

### Pushover

1. On [Pushover](https://pushover.net), create an application, and copy its token. Copy your user key from your account page.
2. In SparkBox, click **Pushover**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A form opens.
3. Paste them into **App token** and **User key**. They go to the Keychain.
4. To reach one phone only, type its name in **Device**. Leave it empty for all your devices.
5. Click **Add channel**. Confirm with Touch ID or your Mac password if asked.

### Webhook

1. In the service you use, create an incoming webhook, and copy its address. In Slack, this is an Incoming Webhook.
2. In SparkBox, click **Webhook**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A form opens.
3. For Slack, Mattermost or Discord's `/slack` address, turn on **Slack-compatible**.
4. Paste the address into **URL**. It's a secret too, so it goes to the Keychain, and SparkBox shows only its host name afterward.
5. If your receiver checks signatures, type a **Signing secret**. SparkBox then signs each message.
6. Click **Add channel**. Confirm with Touch ID or your Mac password if asked.

### Email

1. In your mail account, create an app password for SparkBox. Many mail services offer one, and it's safer than your main password.
2. In SparkBox, click **Email**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A form opens.
3. Fill in **SMTP server**, such as `smtp.gmail.com`, and **Security**. **STARTTLS** sets the port to 587 and **SSL** sets it to 465.
4. Fill in **User**, **From** and **To**. **To** can list several addresses, separated by commas.
5. In **Password**, type the app password. It goes to the Keychain.
6. Click **Add channel**. Confirm with Touch ID or your Mac password if asked.

### Send a test

1. On the new channel's row, click **Test**. SparkBox sends a real message at once, even with **Demo** data.
2. Check your phone. SparkBox says "Test sent to Phone (ntfy)", with the channel's name.

## What you should see

- The test message arrives on your phone.
- The channel's row reads something like "This Mac only · Sent 20 s ago (test) in 0.4 s".
- From now on, alerts go to the channel as set under **When to notify**. To send only critical alerts here, click **Edit** and turn on **Critical only**.

## Why it works

SparkBox sends each alert itself, over the internet, to the service you chose. If a send fails for a reason that may pass, SparkBox tries again after 30 seconds, 2 minutes and 10 minutes. A critical alert then keeps trying every 30 minutes for up to 24 hours. Each attempt appears in **Alerts › Deliveries**. With **Demo** data, alerts are recorded as "Demo — not sent", and only **Test** sends for real.

## Limits

- Without [always-on monitoring](help:set-up-always-on-monitoring), a channel works only while SparkBox runs on this Mac. If the Mac sleeps, nothing pages you.
- The node agents aren't told about [maintenance mode](help:use-maintenance-mode), so they may still page during it.
- In the command palette, **Send test notification** tests every enabled channel, from this Mac.

## If something goes wrong

- **"Failed: HTTP 401 — check the token".** The token or key is wrong. Click **Edit** and type it again. Pushover says "check the app token and user key".
- **"Couldn't reach ntfy.sh: no answer in 15 s".** This Mac can't reach the service. Check your internet connection.
- **"SMTP 535 — check the user and password".** The mail server refused the login. Use an app password.
- **"Use https for ntfy.example.com: plain http would send the token and every alert unencrypted."** Use an `https://` address. Plain `http://` works only on your home network.
- **"Phone (ntfy): enter the ntfy topic."** A required field is empty. The message names it.
- **"Changing the server clears the saved token. Enter it again."** You pointed the channel somewhere else, so SparkBox won't send the old secret there.

[Alert delivery and agent messages](help:errors-alerts-and-agent) lists more.

## Next steps

- [Set up always-on monitoring](help:set-up-always-on-monitoring)
- [Respond to an alert](help:respond-to-an-alert)
- [Settings: Notifications](help:settings-notifications)
