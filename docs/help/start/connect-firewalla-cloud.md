---
id: connect-firewalla-cloud
title: Connect Firewalla's cloud
type: howto
area: network
summary: Give SparkBox your Firewalla MSP domain and token, so that the Switch X's ports show real speeds and counters.
keywords: [firewalla, msp, cloud, token, api token, domain, switch x, link speed, poe, msp lite]
verified: false
related: [see-whats-plugged-into-the-switch-x, firewalla-view, settings-switches, keychain-and-secrets]
---

# Connect Firewalla's cloud

To show the Switch X's real link speeds and counters, give SparkBox your Firewalla MSP domain and an API token.

This is optional. The port panel works without it, from LLDP and your own labels.

## Before you start

- You need a Firewalla **MSP** account. MSP is Firewalla's web service for looking after its devices, at [firewalla.net](https://firewalla.net).
- Be ready to unlock Admin mode, with look-only mode off. Saving the domain and saving the token each ask for Touch ID or your Mac password.
- **Test connection** needs **Live** data, with look-only mode off. In Demo, nothing is sent to Firewalla.
- SparkBox finds the Switch X in Firewalla's answer by its address, or by the MAC address your nodes hear. A **MAC address** is a network card's fixed ID. To give it an address, see [Add your switches and gateway](help:add-switches-and-gateway).

> [!WARNING]
> **None of this has run on your Switch X yet.** Firewalla doesn't document these calls. What SparkBox knows about them comes from a community project, [firewalla-snmp-proxy](https://github.com/kdesch5000/firewalla-snmp-proxy), written against the Switch SE. It's unconfirmed on the Switch X, and it's unconfirmed on the free MSP Lite plan. If it doesn't work, the panel says so in plain words and keeps working from LLDP and your labels. To stop, remove the token.

## Steps

### Get a token

The path in step 2 comes from that project's notes, not from Firewalla, so your account may show something else.

1. In your web browser, open [firewalla.net](https://firewalla.net) and log in, or sign up. Your **MSP domain** is the address your account opens at. It looks like `dn-abc123.firewalla.net`.
2. Open your account settings, then **API tokens**.
3. Create a token, then copy it. Treat it like a password: the notes say it can read your whole Firewalla account.

### Give them to SparkBox

4. Press <kbd>⌘,</kbd>, then click **Switches & gateway**. [Open Switches settings](open:settings?section=switches)
5. Under **Firewalla cloud (MSP)**, in **MSP domain**, type your domain, then click **Save**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
6. Click in **API token**, paste the token, then click **Save**. SparkBox asks for Touch ID or your Mac password. The box then shows "Saved in Keychain".
7. With **Live** data, click **Test connection**.


## What you should see

The test reads "Connected. Firewalla reports 12 ports on Firewalla Switch X, 3 with a link.", with your switch's name and numbers.

In the Switch X's window, the line under the legend reads **Firewalla cloud: read just now**. Ports with a link now glow in the color of their speed, and their details show traffic, errors and power over Ethernet.

## Why it works

Firewalla's switches report their ports to Firewalla's cloud, and the token lets SparkBox read that report. It asks only for the list of your boxes and for the box's topology, and those are the only two requests the app can make, so it can't change anything. It sends the token only over HTTPS to an address that ends in `.firewalla.net`, and keeps it only in the Keychain. It asks at most once in 15 minutes, because Firewalla limits how often an account can ask and the limit isn't published.

## Limits

- The cloud has no list of devices by port. It tells you a port has a link and how fast, not what's on it. Your labels still do that.
- SparkBox asks at most every 15 minutes, and the cloud's own counters move about every 5 minutes. This isn't a live view. A reading can be much older after a failure, and the panel says how old it is.
- SparkBox reads with **Live** data only, and only while a domain is saved.
- In look-only mode, SparkBox doesn't ask at all, and **Test connection** is refused. Nothing that carries your token leaves this Mac.
- **Test connection** runs at most once in 5 minutes. The limit isn't published and lockouts last hours, so clicking it can't spend your quota.
- If Firewalla says to stop, SparkBox waits at least an hour, then longer each time. Others have seen these waits last over 6 hours. Restarting SparkBox doesn't shorten them, and **Test connection** waits too.

## If something goes wrong

- **"Firewalla refused the token. Check it in Settings › Switches & gateway."** Copy the token again and save it. SparkBox tries again within 5 minutes of the last try.
- **"Firewalla answered with a web page, not data. The MSP domain may be wrong, or this account may not have this API."** Check the domain against the address your browser shows. If it's right, this plan may not offer the API.
- **"SparkBox doesn't recognize Firewalla's answer (…), so it shows nothing from the cloud."** The Switch X may report differently from the Switch SE. The panel keeps working from LLDP and your labels.
- **"Firewalla lists 1 switch, and none has the address 192.168.50.238 or a MAC LLDP heard from this switch."** The address in **Settings › Switches & gateway** isn't the one Firewalla has for it.
- **"Firewalla's cloud asked SparkBox to wait: too many requests on this account. SparkBox tries again at 14:05."** Leave it alone. It recovers by itself.
- **"Not tested: SparkBox asked Firewalla in the last 5 minutes. Firewalla limits how often it can ask, so try again after that."** Wait, then click **Test connection** again.

[VPN, fabric and switch messages](help:errors-network) lists the rest.

## Next steps

- [See what's plugged into the Switch X](help:see-whats-plugged-into-the-switch-x)
- [Where your passwords and keys go](help:keychain-and-secrets), to see where the token is kept
