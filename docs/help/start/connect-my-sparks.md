---
id: connect-my-sparks
title: Connect my Sparks
type: feature
area: start
summary: "The setup wizard: account, nodes, checks, VPN, switches, then Live data."
keywords: [wizard, setup, onboarding, discovery, subnet, topology, find sparks]
verified: false
related: [connect-your-cluster, check-your-nodes, trust-a-host-key, add-switches-and-gateway]
---

# Connect my Sparks

## What it's for

**Connect my Sparks** is a wizard that takes you from a new SparkBox to one that shows your real nodes. It asks for the account your nodes share, lists your nodes, checks each one, and offers the VPN and your switches. It ends by switching you to Live data.

You can run it as often as you like. Each run starts from what's already saved. A new installation has no saved nodes or switches; enter your own account and addresses. Example names and addresses in Help are synthetic. For a guided first run, follow [Connect your cluster](help:connect-your-cluster).

## Where to find it

- **Welcome screen:** click **Connect my Sparks**. To see that screen again, choose **Help › Show Welcome** in the menu bar.
- **Settings:** in **Nodes**, click **Connect my Sparks…** under the table.
- **Command palette:** press <kbd>⌘K</kbd> and type `connect`.
- **Link:** [Open Nodes settings](open:settings?section=nodes)

## What's on the screen

The list on the left names the six steps, and a checkmark shows the ones behind you. You can click any step at any time. **Back** and **Next** at the bottom move one step, and the bottom also shows "Step 1 of 6". The window's title names the step you're on.


### Step 1: Account

The heading is **The account SparkBox logs in with**.

- **User** is the account on your nodes, `sparkuser`.
- **Password** is the cluster password. Click **Save to Keychain** to save it. A green **Saved** label appears, and the hint reads "Saved in the Keychain. Type a new one to replace it."

SparkBox logs in with your SSH keys first, and this password last. sudo always uses the password. SparkBox runs sudo when you make a change, read a protected file with sudo, or run the Doctor.

### Step 2: Nodes

The heading is **Your Sparks**. The table has one row for each node, with its **Name**, **Address (IP)**, **Hostname** and **SSH port**. The trash icon removes a row, and **Add node** adds one.

Enter each node's LAN address, or use **Find Sparks on my network…**. Names ending in `.local` work at home, but not through the VPN.

**Next** saves the list, and then starts the checks if you haven't run them yet.

### Find Sparks on your network

Open **Find Sparks on my network…** in **Nodes**. Choose **Local network**, then click **Survey this /24**. A /24 is one group of up to 254 local addresses. SparkBox surveys only the group containing this Mac's selected Ethernet or Wi-Fi address. With multiple networks, choose one explicitly. It excludes VPNs, virtual bridges, public addresses and smaller networks.

The survey checks SSH and web services on ports 22, 80 and 443. It sends no credentials, follows no web redirects, and stops within two minutes. **Stop discovery** cancels it. Missing replies do not prove that a device is absent. Custom SSH ports need manual entry.

For devices you recognize, click **Verify** one at a time. Compare any unknown SSH key on the device before **Trust…**. A changed key needs a separate review. Verification uses the **Account** user, your configured SSH keys and saved cluster password, so trust only your devices. It reads the GPU, machine identity, physical network addresses and any reported neighbors. It installs nothing, runs no sudo and changes no configuration.

Only a device reporting a GB10 GPU through trusted SSH can be selected as a Spark. A hostname, open SSH port or RouterOS web page alone cannot confirm its identity. Python 3, `ip` and `nvidia-smi` must be available; missing tools or identity fields leave the device unconfirmed.

Verification also needs the physical network adapter's MAC address, read from the node's system files. If that address is missing, the device stays unconfirmed even when its GPU and IP address are known.

Select up to four verified Sparks, then click **Preview topology**. The proposal groups observed network addresses and shows **Reported neighbors (LLDP)**, the neighbor information exchanged by network devices. Reports naming the same node or one of its own adapters are excluded. Switches can forward other reports, so a neighbor report does not prove a direct cable. No LLDP report means cabling is unknown. Shared network addresses do not prove a physical cable. Switch hints never add a switch automatically.


**Use proposal in draft** copies reviewed nodes to the table. Saved IDs, names, fabric expectations, unselected nodes and switches stay intact. Identity conflicts stop the proposal; discovery cannot replace an identity pin. If one Spark answers on both Ethernet and Wi-Fi, select only one address. The existing **Next**, **Checks** and **Done** actions save the draft with Mac authentication. Closing the wizard before saving discards it.

Results expire after ten minutes. A changed data source or local interface stops discovery; refresh by closing and reopening discovery, then survey again. Live surveys have a 30-second minimum interval. In Demo, every answer comes from fixtures and no network traffic or real login occurs.

### Step 3: Checks

The heading is **Checking each node**. SparkBox connects to each node and runs 11 checks, listed in [Check that your nodes are ready](help:check-your-nodes).

> [!NOTE]
> With Demo data these answers come from the simulator, and the page says so. They tell you nothing about your real nodes. To check your real nodes, switch to Live first.

**Check all nodes** runs the checks. Afterward the button reads **Check again**. Each node gets a box with its name and address, showing **Checking…** and then its results. Each result has an icon. A checkmark means passed, a warning sign needs a look, a cross failed, and a dashed circle wasn't checked.

Where a host key needs your decision, the key appears with **Trust…**, or **Review…** if it changed. [Trust a node's host key](help:trust-a-host-key) explains what to do.

Run these checks on your own configured nodes.

### Step 4: VPN

The heading is **Reaching the cluster from anywhere**. At home, SparkBox talks to your nodes directly. Away from home, it can use its own tunnel to your Firewalla Gold.

**Right now** shows the tunnel's state. **Open VPN to import a profile…** opens the [VPN](help:vpn) window. This step is optional: skip it if you only use SparkBox at home.

### Step 5: Switches

The heading is **Switches and the gateway**. There's one group for each device, with an **Address** and a **Test** button. The CRS804 DDQ also has **REST user** and **REST password**. Leave an address empty to skip that device.

**Test** saves the list, tries the device and shows the result in the group. If the switch presents a certificate SparkBox hasn't seen, the result has a **Trust…** button. You can also trust a certificate from **Test connection** in the [Switch window: CRS804 DDQ](help:switch-window), or in **Settings › Switches & gateway**. [Add your switches and gateway](help:add-switches-and-gateway) has the details.

### Step 6: Done

The heading is **Ready**, or **Almost there** if a check failed. A line says what the checks found. With Live data it reads "Every node passed…" when they all did. With Demo data it reads "The simulated checks passed; nothing real was tested. Switch to Live to check your nodes. Demo stays one click away in Settings › Data source."

| If you're on | Main button | Other button |
|---|---|---|
| Demo | **Switch to Live data** | **Stay in Demo** |
| Live | **Save and stay on Live** | **Switch back to Demo** |

Both buttons save the list and open the **Dashboard**. With a failed check, Live still works for the nodes that passed. The others show why they can't connect.

## What it changes

| Step | What changes | What it needs | How to undo |
|---|---|---|---|
| **Account** | Saves the password in the Keychain | Touch ID or your Mac password, every time | Change it in [Settings › Cluster login](help:settings-cluster-login) |
| **Nodes**, and **Test** in **Switches** | Saves the node and switch lists on this Mac | Touch ID or your Mac password, unless you confirmed in the last 5 minutes | Change them in **Settings › Nodes** or **Switches & gateway** |
| **Find Sparks** | Keeps a temporary survey and copies reviewed nodes into the draft; trust records keys on this Mac | Touch ID or your Mac password for survey, verification and trust | Close discovery; discard an unsaved draft, or edit saved nodes in Settings |
| **Checks** | Nothing on your nodes. The checks only read, and the sudo check runs `sudo true`, which does nothing | Touch ID or your Mac password, unless you confirmed in the last 5 minutes | Nothing to undo |
| **Trust…** | Records a host key on this Mac | The same | Forget it in **Settings › Diagnostics** |
| **Done** | Switches the data source | The same | Switch back in **Settings › Data source** |

Your nodes don't change in any step, apart from SparkBox's own small helper files in `~/.sparkbox`, which it keeps there in Live.

## Related How-Tos

- [Connect your cluster](help:connect-your-cluster)
- [Check that your nodes are ready](help:check-your-nodes)
- [Trust a node's host key](help:trust-a-host-key)
- [Add your switches and gateway](help:add-switches-and-gateway)
- [Reach your cluster with the VPN](help:connect-the-vpn)

![Connect my Sparks, using synthetic Demo data](img/public-connect-my-sparks.jpg)
