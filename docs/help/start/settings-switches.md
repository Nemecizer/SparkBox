---
id: settings-switches
title: "Settings: Switches & gateway"
type: feature
area: settings
summary: Addresses and login for the CRS804 DDQ, the Firewalla Switch X and the Firewalla Gold, and the optional Firewalla cloud.
keywords: [crs804, firewalla, rest, routeros, msp, firewalla cloud]
verified: false
related: [add-switches-and-gateway, switch-window, firewalla-view, connect-firewalla-cloud]
---

# Settings: Switches & gateway

## What it's for

This section tells SparkBox where your network devices are, how to log in to the one that needs it, and how to reach Firewalla's cloud if you use it. Use it to change an address or a password after you've set things up. For a first-time setup, follow [Add your switches and gateway](help:add-switches-and-gateway).

## Where to find it

- **Settings:** press <kbd>⌘,</kbd>, then click **Switches & gateway**.
- **Link:** [Open Switches settings](open:settings?section=switches)


## What's on the screen

The section has one group for each device: **CRS804 DDQ**, **Firewalla Switch X** and **Firewalla Gold**. A line under each group says what SparkBox does with that device. **Equipment profiles** controls automatic specification lookup. A last group, **Firewalla cloud (MSP)**, is optional.

### Kind

What sort of device this is. You can't change it.

| Kind | Device | What SparkBox does |
|---|---|---|
| **MikroTik RouterOS switch** | CRS804 DDQ | Reads ports, optics and counters through the RouterOS REST API |
| **Firewalla switch** | Firewalla Switch X | Checks that it answers on TCP 443, then 80, and reads its ports from LLDP, your labels and, if you set it up, Firewalla's cloud. Firewalla switches have no local API for port figures |
| **Gateway** | Firewalla Gold | Checks that it answers on TCP 443, then 80, and times the reply |

### Address

The device's IP address or hostname on your LAN. Empty means SparkBox doesn't watch the device, and the row says "Empty means not monitored".

### REST user

Shown for the CRS804 DDQ only. This is the account SparkBox uses on the switch. The field suggests `sparkbox`. The user must be in a group with the `read` and `rest-api` policies. To change ports from SparkBox, the group also needs `write` and `test`.

### REST password

Shown for the CRS804 DDQ only. This is that user's password. It goes straight to the Keychain. **Save** stores it, and **Remove** deletes it. The box shows "Saved in Keychain" or "Not saved", and never the password.

### Connection

**Test connection** tries the saved settings, so save your edits first. A dot and a word show the result: **Connected** with the board and RouterOS version, or **Failed** with the reason.

After a login refusal (401), automatic checks wait 30 seconds before trying again. The error and its alert stay visible. **Test connection** makes one read right away using the saved account; changing the saved user or password also lets the next check try the new account. See [Switch login refused](help:errors-network#switch-login-refused-401) if the error returns.

If the switch presents a certificate SparkBox hasn't seen, the result has a **Trust…** button. It opens the same dialog as the **Switches** step of [Connect my Sparks](help:connect-my-sparks#step-5-switches), and then tests again.

In Demo, the test says "Demo data: the simulated switch answers. Switch to Live data to test the real one."

### Equipment profiles

**Automatic AI lookup** is off by default. Turn it on to let SparkBox look up newly observed equipment models with the provider and model selected under **Settings › Models › AI feasibility check**. Changing this setting needs Touch ID or your Mac password. Provider requests may incur charges.


SparkBox watches board models reported by monitored switches and system models reported by online nodes in your inventory. It sends the model string to Anthropic or OpenAI, then fetches a public manufacturer page and asks the AI to extract the physical connector counts and rated speeds. It does not send addresses, passwords, hostnames or the cluster topology. It makes at most two AI requests per model and one lookup at a time. A saved result is reused for 30 days; a failure waits 24 hours before retrying. Turning the switch off stops pending lookup work. Demo and look-only runs make no lookup requests.

The list shows each device's model, physical-port groups, manufacturer link and whether the profile is bundled or AI-derived. AI profiles show the provider, model and date. The CRS804 profile is bundled and always has four fabric sockets. Unsupported sources, unreadable pages and replies without matching evidence show a reason and leave the physical layout unresolved. Supported documentation sites are MikroTik, NVIDIA, MSI, ASUS, Firewalla, Ubiquiti, NETGEAR and TP-Link; a page must be readable HTML or text, without a redirect.

A profile describes rated hardware capacity. It does not overwrite measured link speeds, MAC/LLDP cabling, your inventory expectations or load admission checks. It never changes a device's configuration. A device still needs to be in the inventory and report its model. In **Connect my Sparks › Nodes**, **Find Sparks on my network…** can survey a local /24 and propose verified nodes and reported neighbors. Web-page hints do not add switches automatically. See [Connect my Sparks](help:connect-my-sparks#find-sparks-on-your-network).

### Firewalla cloud (MSP)

Optional. With a Firewalla **MSP** login, SparkBox reads the Switch X's real link speeds and counters from Firewalla's cloud. [Connect Firewalla's cloud](help:connect-firewalla-cloud) shows how to get what these rows need.

| Row | What it does |
|---|---|
| **MSP domain** | The address your MSP account opens at, such as `dn-abc123.firewalla.net`. Empty means SparkBox doesn't use the cloud. Only an address that ends in `.firewalla.net` is accepted, and **Save** asks for Touch ID or your Mac password every time, because this says where the token goes |
| **API token** | The key that lets SparkBox read your account. It goes straight to the Keychain. **Save** stores it, and **Remove** deletes it, along with what SparkBox read with it. The box shows "Saved in Keychain" or "Not saved", and never the token |
| **Connection** | **Test connection** asks Firewalla once with what you saved. It needs **Live** data, and in look-only mode it's refused. It can run once in 5 minutes, and not at all while Firewalla has told SparkBox to wait |

SparkBox reads at most every 15 minutes, only with **Live** data, only once a domain is saved, and never in look-only mode, so nothing that carries your token leaves this Mac then. It uses two read-only requests and has no way to send any other. Firewalla doesn't document these requests. A community project uses them on the Switch SE, and they aren't confirmed on the Switch X.

### The save bar

A bar appears when you've edited an address or user. Click **Save**, or **Apply & reconnect** with Live data. **Revert** throws your edits away. The bar also covers unsaved edits in **Cluster login** and **Nodes**.

## What it changes

These settings live on this Mac. Saving an address asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes. Saving or removing the REST password, the MSP domain or the API token asks every time. Each change is written to the activity log. The log names the change, and never shows a password or token.

Nothing here changes the devices themselves. To stop watching a device, clear its address and save. To undo, type it back.

## Related How-Tos

- [Add your switches and gateway](help:add-switches-and-gateway)
- [Connect Firewalla's cloud](help:connect-firewalla-cloud)
- [See what a port connects to](help:see-what-a-port-connects-to)
- [Manage a switch port](help:manage-a-switch-port)
