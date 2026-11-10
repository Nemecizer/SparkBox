---
id: errors-network
title: VPN, fabric and switch messages
type: troubleshooting
area: troubleshooting
summary: Messages from the VPN, the fabric tests and the CRS804.
keywords: [401, switch login, username, allowed address, vpn error, pfc, nccl, rest password, certificate, fabric, perftest, mtu, handshake, this mac can't reach the cluster, firewalla, switch port]
verified: null
related: [connect-the-vpn, test-the-fabric, manage-a-switch-port, fabric-and-rdma, add-switches-and-gateway]
---

# VPN, fabric and switch messages

This page covers messages about how this Mac reaches your cluster, and from the VPN, the fabric and your switches.

## This Mac can't reach the cluster

SparkBox watches its own path to the nodes. When the path fails, it raises one warning, not a page about each node. These words appear in **Alerts** and in node tooltips, as "not confirmed down".

### "This Mac just woke up"

**What it means.** The body reads "It woke up 20 s ago; the nodes get a minute to reconnect before anything is raised about them."

**What to do.** Wait a minute.

### "This Mac lost the home network"

**What it means.** This Mac has no working path. While the VPN is in use, the title reads "This Mac lost its network". The body gives the reason: "It has no network route, so no node can be reached. Check its Wi-Fi or cable." or "Its router (…) doesn't answer, so no node can be reached." The router here is your gateway, the Firewalla Gold.

**What to do.** Check this Mac's Wi-Fi or cable. Then check that the Firewalla Gold is on.

### "This Mac can't reach the cluster: VPN problem"

**What it means.** The VPN has an error, and no node answers through it. The body gives the VPN's reason.

**What to do.** Open [VPN](open:vpn), and read the reason. Then find it under [VPN](help:errors-network#vpn) on this page.

### "This Mac can't reach any node; the LAN between them?"

**What it means.** Your gateway answers, but no node does. The body adds whether the Firewalla Switch X answers. "The nodes aren't confirmed down." means they may be fine, with the fault between you and them. "Switch X not set up, so SparkBox can't probe it." means it has no address yet.

**What to do.** Check the Firewalla Switch X: its power and its cables. See [Add your switches and gateway](help:add-switches-and-gateway).

### "Only cell3's Wi-Fi answers: wired LAN switch?"

**What it means.** The node answers on its Wi-Fi address, but not on its cable. So the node is on, and the wired path is the likely fault.

**What to do.** Check the Firewalla Switch X and its cables.

## VPN

The VPN is a private tunnel from this Mac to your home network, through the Firewalla Gold. See [Reach your cluster with the VPN](help:connect-the-vpn).

### "No VPN profile imported."

**What it means.** SparkBox has no profile to connect with.

**What to do.** Follow [Reach your cluster with the VPN](help:connect-the-vpn).

### "The VPN private key is missing from the Keychain; import the profile again."

**What it means.** The profile's settings are here, but its key has gone.

**What to do.** Import the profile again.

### "Disconnected by you. Choose Connect, or Auto mode, to use the tunnel again."

**What it means.** You chose **Disconnect**, and SparkBox is respecting that.

**What to do.** Choose **Connect** when you want the tunnel.

### "VPN is turned off."

**What it means.** The VPN is off in its mode setting.

**What to do.** Change the mode in [VPN](open:vpn), if you want it on.

### "No answer from the Firewalla. Check its address, that the VPN server is on, and that this network allows UDP to that port. Still trying…"

**What it means.** The tunnel can't finish its first handshake, the opening exchange. Some networks block this traffic.

**What to do.**

1. Check the address in [VPN](open:vpn).
2. In the Firewalla app, check that the VPN server is on.
3. Try another network, such as a phone hotspot, to rule out a block.

### "No handshake yet"

**What it means.** SparkBox’s tunnel has not authenticated the server. The status includes UDP packets sent, received and send errors. Received packets alone do not prove a working tunnel.

**What to do.** Follow [WireGuard works, but SparkBox has no handshake](help:connect-the-vpn#wireguard-works-but-sparkbox-has-no-handshake). Reimport the working profile, keep the separate WireGuard app disconnected, and record the status if it still fails.

### "The tunnel is up, but no node answered through it. Check AllowedIPs in the profile."

**What it means.** The tunnel works, but the profile doesn't route your nodes' addresses. `AllowedIPs` is the profile's list of addresses to send through the tunnel.

**What to do.** Make a new profile in the Firewalla app that covers `192.168.50.0/24`, and import it.

### "Direct at home is on and this Mac has a matching local subnet."

**What it means.** No node answered directly, but Direct at home keeps Auto from starting the tunnel on a matching local network. This does not prove you are at home.

**What to do.** When away, turn off **Direct at home** or choose **Connect**. Another network may use the same addresses.

### "This Mac has no network right now. Direct at home is on, so SparkBox waits for the network instead of starting the tunnel."

**What it means.** With **Direct at home** on, SparkBox won't start a tunnel while this Mac has no network.

**What to do.** Reconnect this Mac.

### "The cluster is reachable directly (cell2 answered in 2 ms): home network, or another VPN app is connected."

**What it means.** A note, not a fault. No tunnel is needed.

**What to do.** Nothing.

### "Handshake went stale; reconnecting…"

**What it means.** The tunnel stopped hearing from the Firewalla, and is starting again.

**What to do.** Wait.

### "The tunnel helper stopped; retrying shortly."

**What it means.** The small program behind the tunnel stopped. "Tunnel error: …" gives its reason.

**What to do.** Wait. If it repeats, quit and reopen SparkBox.

### "Port 8080 is already used by another app on this Mac."

**What it means.** A port under **Expose to this Mac** is taken.

**What to do.** Pick another local port.

### "Local port 8080 is used twice."

**What it means.** Two rows share a port. You may also see:

- `"Grafana" needs a local port from 1 to 65535.`
- `"Grafana" needs a home address like 192.168.50.51:3000.`

**What to do.** Fix the row.

### "Enter the Firewalla's public IP address."

**What it means.** The address field is empty.

**What to do.** Fill it in, from the Firewalla app.

### "This profile uses AmneziaWG 2.0 settings (S3/S4), which SparkBox's tunnel engine doesn't support yet. Firewalla's own profiles use AmneziaWG 1.5."

**What it means.** The file comes from a newer version than SparkBox reads. "This profile uses AmneziaWG 2.0 header ranges (H1–H4 as a range), which SparkBox's tunnel engine doesn't support yet." is the same.

**What to do.** Export a profile from the Firewalla app.

### "The profile has no PrivateKey in [Interface]."

**What it means.** The file isn't a complete profile. You may also see:

- "The profile has no Address in [Interface]."
- "The profile has no [Peer] PublicKey."
- "line 7: expected `Key = Value`"
- "The profile has more than one [Peer]; SparkBox expects a single Firewalla peer.", which means it has too much

**What to do.** Export a fresh profile from the Firewalla app.

## The fabric

The fabric is the fast 200G network between your nodes. See [The fabric and RDMA](help:fabric-and-rdma).

### "Fabric healthy"

**What it means.** All is well. **Fabric up · PFC off · 8 fabric ports** means the links are up, and SparkBox is noting the setting below.

**What to do.** Nothing.

### "The fabric isn't lossless."

**What it means.** PFC, a setting that protects fabric traffic from loss, is off. The fabric can lose packets under load, and then resends them. Things work, a little slower. It's advice, not a fault.

**What to do.** Nothing is required. SparkBox doesn't change this setting.

### "cell2 half 2 is on 1-2; expected 1-1"

**What it means.** The cabling differs from what you accepted. The verdict reads **Cabling changed**. The hint reads "Move the cables back, or accept the current cabling if it was on purpose."

**What to do.** Do what the hint says. See [Fabric](help:fabric).

### "No fabric data"

**What it means.** The body reads "No node reports a fabric interface yet."

**What to do.** Wait for the nodes to report. Check that they're online.

### "cell2 half 1 has no carrier."

**What it means.** The link isn't connected at that port. "Check the optic or cable between cell2's port 2 and switch port 1-1." suggests where to look.

**What to do.** Reseat the cable and the optic at both ends. Check the CRS804's power.

### "RoCE is retransmitting"

**What it means.** The body reads "Packets are being lost on this path: look for PHY errors on the switch port, a pause storm elsewhere, or PFC not enabled for RoCE's priority." Lost data is being sent again, which slows things down.

**What to do.** Run a test in the **Doctor** tab in **Fabric**. If one link stands out, check its optic and cable.

### "PHY errors"

**What it means.** PHY is the physical layer: the optic, the cable and the signal. Errors there point at one of those. The hint names the place, such as "Check the optic or cable between cell2's port 2 and switch port 1-1."

**What to do.** Reseat the cable and the optic at both ends. If the errors go on, replace them.

### "Pause storm"

**What it means.** A pause frame tells a sender to stop for a moment, and a stream of them is holding traffic back. If this port sends them, the hint reads "cell2 can't drain its receive queue and is stopping the whole priority. Look for a stuck process or out-of-buffer drops on cell2." If it receives them, it reads "Something downstream is stopping this port. Find the node that sends the pause frames."

**What to do.** Look for a stuck process on the node that sends them.

### "Dropping packets"

**What it means.** The receive buffer overflows. "With PFC on RoCE's priority this shouldn't happen; check mlnx_qos."

**What to do.** See [The fabric and RDMA](help:fabric-and-rdma).

### "Lost packets earlier"

**What it means.** Past losses, in the last hour. The problem isn't happening now.

**What to do.** Watch it for now.

### "NCCL is using TCP sockets (RDMA 0%)"

**What it means.** A running model isn't using the fast path. NCCL is NVIDIA's library that lets GPUs on different nodes work together. Related lines are "NCCL_IB_DISABLE=1 on cell2: NCCL won't use RDMA at all.", "NCCL_IB_HCA isn't set on cell2, so NCCL picks the RDMA devices itself (it may pick the uncabled port)." and "NCCL_SOCKET_IFNAME=enP7s7 on cell2 is the LAN, not the fabric."

**What to do.** These settings come from how the model was started. Correct them in its launch script.

### "Not generating yet: the RDMA share is measured while it serves"

**What it means.** A note. The figure appears when the model is busy.

**What to do.** Nothing.

### "Choose at least two nodes: the tests run between pairs."

**What it means.** A fabric test needs two nodes. "Choose at least one check." means no test is picked.

**What to do.** Pick nodes and a check on the **Doctor** tab.

### "The fabric test is already running (task …)."

**What it means.** One test runs at a time.

**What to do.** Follow it in [Tasks](open:tasks).

### "cell2 isn't connected; it was left out."

**What it means.** The node wasn't online when the test started.

**What to do.** Run the test again when it's back.

### "cell2 can't send 9000-byte frames (MTU 1500 on the path)"

**What it means.** A device or setting on the path won't carry large packets, so large transfers will fail. You may also see:

- "Small packets reach cell3 but 9000-byte frames don't", which is the same problem
- "MTU 1500; the fabric uses 9000", which names the node's interface
- "Switch port 1-2 allows 1500, less than 9000", which names the switch port

**What to do.** SparkBox can't set an MTU, the largest packet size a link carries. The **MTU audit** table on the **Doctor** tab in **Fabric** shows what differs. Follow [Network settings SparkBox can't change](#network-settings-sparkbox-cant-change).

### "A fabric link is below 200 Gb/s. On the CRS804, set those ports to 200G-baseCR4 if auto-negotiation came up short."

**What it means.** A link came up slower than it should. Auto-negotiation is how the two ends of a cable agree on a speed. This message appears on the **Prepare cluster** tab in **Models**.

**What to do.** These steps break the link for a moment, so first [unload](help:unload-a-model) any model running across nodes.

1. Reseat the cable and the optic at both ends.
2. Follow [Bounce a port](help:manage-a-switch-port#bounce-a-port). It's the usual first step for a link that misbehaves.
3. Click **Check nodes** on the **Prepare cluster** tab.
4. If the link is still slow, SparkBox can't set a port's speed. Follow [Network settings SparkBox can't change](#network-settings-sparkbox-cant-change).

### "A node has no 192.168.100.x address. SparkBox uses whatever ConnectX-7 address it has, but every node should be on the same subnet."

**What it means.** One node's fabric address is missing, or it's in a different range from the others. A **subnet** is a range of addresses that can reach each other directly. This message appears on the **Prepare cluster** tab in **Models**.

**What to do.** SparkBox can't set a fabric address. Follow [Network settings SparkBox can't change](#network-settings-sparkbox-cant-change).

### "No reply from cell3 on half 2"

**What it means.** The two nodes can't reach each other on that half.

**What to do.** Check the links in [Fabric](open:fabric).

### "perftest isn't installed on cell2: its pairs were skipped."

**What it means.** The speed test needs a tool the node doesn't have.

**What to do.** Click **Install…** on the notice in the **Doctor** tab. It needs Admin mode.

### "Timed out after 30 s (is port 18515 blocked, or the peer's RDMA device down?)"

**What it means.** The speed test couldn't start between two nodes.

**What to do.** Check that the fabric port on the other node is up. See [Ports and addresses](help:ports-reference).

### "61 Gb/s, below 85% of the expected 92 Gb/s"

**What it means.** The link works, but it's slow. "74 Gb/s, 20% below its baseline of 92 Gb/s" compares with your saved speed.

**What to do.** Run the test again when nothing else is running. If it stays low, check the optic.

### "nccl-tests isn't installed on cell1: build it (github.com/NVIDIA/nccl-tests, make MPI=1) and set the paths"

**What it means.** The all-reduce test needs a tool that isn't on the node. You may also see:

- "All-reduce needs two or more nodes"
- "Every node needs an addressed fabric half 1"

**What to do.** The other tests work without it.

### "1 wrong values: results don't match"

**What it means.** Data changed in transit during the all-reduce test.

**What to do.** Take this seriously. Run it again, and suspect a cable, an optic or a node's memory.

## Network settings SparkBox can't change

Some messages ask for a change to a fabric address, an MTU or a link speed. SparkBox has no control for any of these. A node keeps them in its own netplan files, in `/etc/netplan`. The CRS804 keeps them in RouterOS, the system that runs on it.

> [!DANGER]
> **A wrong network setting can cut a node off, and then only someone at the machine can fix it.** A mistake may not show until the next restart. SparkBox can't undo it, because it reaches the node over the network. Don't change these settings yourself unless you've done it before.

To get one of these fixed:

1. Leave the settings as they are. The node stays online on the LAN, and it can still serve a one-node load.
2. Make a [Doctor report](help:collect-diagnostics) and [capture evidence](help:capture-evidence). Both only read.
3. Ask NVIDIA, your Spark's maker, or someone who has set up Linux networking to make the change. Give them the message and the report. A node that works shows what the settings should be, and [Read a file only root can read](help:read-a-root-only-file) shows how to look at them without changing anything.
4. Before anyone starts, make sure someone can get to the node's screen and keyboard.

The hint for a missing address says "as in NVIDIA's guide". NVIDIA's [ConnectX-7 Networking](https://docs.nvidia.com/dgx/dgx-spark/spark-clustering.html) page explains the ports and their names, and it's safe to read. Don't run the setup script from its [manual setup page](https://build.nvidia.com/spark/connect-multiple-sparks/advance) on your nodes. That script assigns `10.20.x` addresses, and the page gives MTU `1500` for a layout with a switch. Your cluster uses `192.168.100.x`, `192.168.101.x` and MTU `9000`.

## Switches

### "CRS804 DDQ has no address yet. Add it in Settings › Switches & gateway."

**What it means.** The switch isn't set up. It shows **Not set up**.

**What to do.** Follow [Add your switches and gateway](help:add-switches-and-gateway).

### "Save the REST password for CRS804 DDQ in Settings › Switches & gateway."

**What it means.** REST is the way SparkBox talks to the switch. It has no password for it yet.

**What to do.** Fill in **REST password** in [Switches & gateway](open:settings?section=switches).

### Switch login refused (401)

**What it means.** The message starts with the switch's address and `refused user "sparkbox" (401).` The account name is the saved **REST user**. The switch rejected that login; this does not establish a fabric failure or prove the password is wrong. A refusal during a sensor or port-table read also leaves the switch in error.

**What to do.**

1. Open [Switches & gateway](open:settings?section=switches). Check the address, **REST user** and whether **REST password** says "Saved in Keychain". Keep the saved password while testing.
2. Click **Test connection** once. It makes a read immediately. Automatic monitoring waits 30 seconds after each 401 before trying again, while keeping the error and alert visible.
3. If the refusal returns, record its time, the displayed RouterOS version when connected, and whether **VPN** says **Direct** or uses the tunnel. Have the switch administrator compare that time with its login log and review this account's allowed client addresses and REST access policies. [MikroTik's REST guide](https://manual.mikrotik.com/docs/developer-guides/rest-api/) explains these checks.
4. Keep the existing certificate trust and credentials unless that review identifies a specific correction. A successful test checks one request; watch the normal switch status to see whether the failure returns.

### "CRS804 DDQ didn't answer on port 443 or 80."

**What it means.** The switch didn't reply.

**What to do.** Check the address, and that the switch is on.

### "Firewalla Switch X is managed in the Firewalla app."

**What it means.** SparkBox only watches Firewalla devices. They have no local interface it can use.

**What to do.** Use the Firewalla app.

### "SparkBox has no live data for this device."

**What it means.** You labeled a Switch X port with something other than a node. SparkBox shows your label, and nothing else is known about that device.

**What to do.** Nothing. To see a device's link, label the port with a node if it is one. See [See what's plugged into the Switch X](help:see-whats-plugged-into-the-switch-x).

### "4 other devices are heard through this port, so cell1's link speed may be to something else."

**What it means.** You labeled a port with a node, but LLDP hears more than that node through it. So there may be another switch between them, and the node's speed may belong to that link. The port stays gray.

**What to do.** Check where the node's cable goes. If the label is wrong, change it.

### "LLDP places cell1 on port 2, not this one, so its link speed isn't used here. That doesn't prove the label is wrong."

**What it means.** You labeled a port with a node, but the node hears the switch through another port. Your label stays, and SparkBox doesn't color the port from that node's speed.

**What to do.** Check where the node's cable goes. All your nodes may hear one port if another switch sits between them and the Switch X, so the label may still be right.

### "cell1 doesn't hear this switch over LLDP, so nothing shows its link is on this port."

**What it means.** The node hears nothing from the Switch X, so nothing ties its link to this port. Your label stays, and the port stays gray.

**What to do.** Nothing, if you're sure of the cable. To see the port's real speed, [Connect Firewalla's cloud](help:connect-firewalla-cloud).

### "cell1 is labeled on more than one port, so SparkBox can't tell which one its link speed belongs to."

**What it means.** A node has one LAN cable, and two ports name it.

**What to do.** Fix one of the labels in the port's details.

### "You marked this port empty, but LLDP hears the switch through it."

**What it means.** Your label and what your nodes hear disagree. The same note says "but Firewalla reports a link." when it's the cloud that disagrees. The port stays lit.

**What to do.** Check the port, then change the label if it's out of date.

### "Firewalla reports no link here, but you labeled it Printer."

**What it means.** The cloud says the port has no link, and your label says something's plugged in. The switch's own report beats a label, so the port is dark, unless LLDP hears the switch through it now. The device may be off, or its cable may be out.

**What to do.** Check the device and its cable.

### "Firewalla reported no link here, but LLDP hears the switch through this port now."

**What it means.** The cloud's reading is older than what LLDP hears now. The port stays lit in gray, whatever its label says, and a node's speed doesn't color it.

**What to do.** Wait for the next reading.

### "cell1 isn't reporting, so its link can't be read."

**What it means.** The node labeled on this port isn't sending readings.

**What to do.** Check the node. See [Troubleshooting](help:troubleshooting).

### "LLDP names a port SparkBox can't place: Gi0/77 (cell3)."

**What it means.** The switch sent a port name SparkBox can't turn into a number from 1 to 12.

**What to do.** Nothing breaks, and that node isn't placed on the panel. Report the name it shows.

### "This switch has no address yet, so SparkBox can't check that it answers. The ports below come from what your nodes hear."

**What it means.** Your nodes hear the Switch X over LLDP, so its panel works. With no address, SparkBox can't check that it answers, so the tiles stay empty.

**What to do.** To fill them, add the address in [Switches & gateway](open:settings?section=switches). Your Firewalla app lists it.

### "Save the MSP domain first."

**What it means.** **Test connection** has nowhere to ask.

**What to do.** Fill in **MSP domain** in [Switches & gateway](open:settings?section=switches). See [Connect Firewalla's cloud](help:connect-firewalla-cloud).

### "That isn't an MSP domain. It looks like dn-abc123.firewalla.net."

**What it means.** SparkBox sends the token only to an address that ends in `.firewalla.net`, so it refused this one. "The MSP domain must use https." means the same for an address that starts with `http://`.

**What to do.** Copy the address your MSP account opens at.

### "Save the MSP token in Settings › Switches & gateway."

**What it means.** A domain is saved, but no token is.

**What to do.** Fill in **API token** in [Switches & gateway](open:settings?section=switches).

### "Firewalla refused the token. Check it in Settings › Switches & gateway."

**What it means.** Firewalla answered 401 or 403. SparkBox waits an hour before it asks again, or until you save a new token.

**What to do.** Copy the token from your MSP account and save it again.

### "Firewalla answered with a web page, not data. The MSP domain may be wrong, or this account may not have this API."

**What it means.** On this service, an address it doesn't know answers with its web page, not an error.

**What to do.** Check the domain against the address your browser shows. If it's right, your MSP plan may not offer the API.

### "Firewalla's cloud asked SparkBox to wait: too many requests on this account. SparkBox tries again at 14:05."

**What it means.** Firewalla answered 429 or 503. SparkBox stops asking for at least an hour, and longer each time, up to a day, or longer if Firewalla says so. Others have seen these waits last over 6 hours.

**What to do.** Leave it alone. It recovers by itself. Asking again, or restarting SparkBox, doesn't help, so **Test connection** waits too. Ports go back to gray once the last reading is an hour old.

### "SparkBox doesn't recognize Firewalla's answer (it has no list of devices), so it shows nothing from the cloud."

**What it means.** The answer isn't the shape SparkBox knows, which comes from a Switch SE. The words in brackets say what was missing. SparkBox shows nothing rather than guess.

**What to do.** The panel keeps working from LLDP and your labels. Report the message.

### "Firewalla lists 1 switch, and none has the address 192.168.50.238 or a MAC LLDP heard from this switch."

**What it means.** SparkBox knows the switch by its address and its MAC address, a network card's fixed ID. Neither matches, so it won't show that switch's ports as this one's. "Firewalla lists no switch on this box." and "This token sees no Firewalla boxes." are close cousins.

**What to do.** Check the **Address** of the Firewalla Switch X in [Switches & gateway](open:settings?section=switches), and that the switch shows in your MSP account.

### "Firewalla's cloud answered with an error (HTTP 500)."

**What it means.** Firewalla had a problem. SparkBox tries again in 15 minutes.

**What to do.** Nothing. "Firewalla's cloud didn't answer within 20 s." and "Couldn't reach Firewalla's cloud" mean the same, from this Mac's side: check its internet connection.

### "Not tested: SparkBox asked Firewalla in the last 5 minutes. Firewalla limits how often it can ask, so try again after that."

**What it means.** **Test connection** runs at most once in 5 minutes, counting SparkBox's own polls. The limit isn't published and lockouts last hours, so a click can't spend it.

**What to do.** Wait, then click it again.

### "The saved MSP token can't be sent: it holds a line break or another character a request can't carry."

**What it means.** The token has something in it that a web request can't hold, usually from copying it with a line break. Nothing was sent.

**What to do.** Copy the token again, and save it in [Switches & gateway](open:settings?section=switches).

### "The Firewalla settings changed while SparkBox was asking, so it set that answer aside."

**What it means.** You saved a new domain or token while a request was on its way, so its answer is for the old settings.

**What to do.** Nothing. SparkBox asks again within 5 minutes of the last try.

### "Demo data: nothing was sent to Firewalla. Switch to Live data to test the real one."

**What it means.** You're in Demo. The ports you see there are samples.

**What to do.** [Switch between Demo and Live](help:switch-between-demo-and-live).

### "That isn't a SHA-256 certificate fingerprint (64 hex digits)."

**What it means.** The value isn't in the right form.

**What to do.** Copy the full fingerprint.

### "CRS804 DDQ has no port 9-9."

**What it means.** That port doesn't exist on the switch. "" isn't a port name." is the same.

**What to do.** Pick a port from the list.

### "1-1 carries SparkBox's connection to the switch. … To disable it anyway, type "disable 1-1"; the switch undoes it by itself."

**What it means.** Turning this port off would cut SparkBox off from the switch. So SparkBox asks you to type a phrase, and leaves a timed undo on the switch. **Bounce…** asks for the same with "bounce 1-1".

**What to do.** Don't go on unless you're sure. If you do, type the phrase exactly. The port comes back by itself.

### "Couldn't leave a revert on the switch, so 1-2 wasn't disabled: …"

**What it means.** SparkBox only turns off a port when the switch has agreed to undo it. That failed, so nothing changed.

**What to do.** Try again.

### "1-1 carries SparkBox's connection to the switch, so it can't stay disabled."

**What it means.** SparkBox won't keep that port off.

**What to do.** Nothing. It comes back by itself.

### "1-2 disabled; it comes back in 60 s unless you keep it disabled"

**What it means.** A safety feature. If SparkBox loses the switch, the port returns by itself.

**What to do.** If you meant to keep it off, click **Keep disabled**. See [Manage a switch port](help:manage-a-switch-port).

### "Demo data: the simulated switch answers. Switch to Live data to test the real one."

**What it means.** The test ran against the Demo switch.

**What to do.** Switch to Live data for a real test.
