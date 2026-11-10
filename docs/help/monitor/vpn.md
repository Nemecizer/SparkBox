---
id: vpn
title: VPN
type: feature
area: network
summary: SparkBox's own tunnel to your Firewalla Gold, for reaching the cluster when you're away from home.
keywords: [vpn, tunnel, wireguard, expose, direct at home, amneziawg, firewalla gold]
verified: false
related: [connect-the-vpn, keychain-and-secrets, firewalla-view]
---

# VPN

## What it's for

The **VPN** window is SparkBox's own tunnel to your Firewalla Gold, for reaching the cluster when you're away from home. A **tunnel** is a private path across the internet. It belongs to SparkBox alone: nothing else on this Mac goes through it, except ports you choose to expose.

Demo data changes the cluster only. The tunnel, its keys and the exposed ports are real, also in Demo.

## Where to find it

- **Sidebar:** under **Network**, click **VPN**.
- **Status bar:** click the VPN item.
- **Topology:** click **This Mac**.
- **Link:** [Open VPN](open:vpn)

## What's on the screen


### The toolbar

The toolbar shows the state and, while the tunnel is on, the protocol.

| Button | What it does |
|---|---|
| **Connect** | Uses the tunnel now, even if the cluster is reachable directly |
| **Disconnect** | Stops the tunnel until you connect again |
| **Follow mode** | Appears after **Connect** or **Disconnect**. Goes back to your chosen mode |
| **Reconnect** | Starts the tunnel again. In look-only mode, you can still click it, but SparkBox refuses it and says why |
| **Screenshot-safe** | Masks public addresses and DNS in this window, so a screenshot doesn't give them away |

### The status card

The large word is the state.

| State | What it means |
|---|---|
| **Direct** | SparkBox reaches the cluster without the tunnel |
| **Connecting** | The tunnel is starting |
| **Connected** | The tunnel is up |
| **Off** | The tunnel isn't running, and SparkBox isn't trying to start it |
| **Problem** | Something's wrong, and the card says what |

**Connected by you** or **Disconnected by you** means you overrode the mode. The line below shows the path, such as "Tunnel via 203.0.•••.•• · up 2h 14m". The ⓘ says what the tunnel is doing and what network this Mac is on. The eye button shows the full address.

| Fact | What it shows |
|---|---|
| **Handshake** | When the tunnel last confirmed the Gold is there. It renews about every 2 minutes |
| **Connect time** | The fastest connection to a node in the last round of checks |
| **Protocol** | **WireGuard** or **AmneziaWG** |
| **MTU** | The largest packet in the tunnel, or **Automatic** |
| **DNS** | The name servers from the profile |
| **Received / sent** | Total data through the tunnel |

While the tunnel is on, two small charts show traffic each way.

#When no handshake completes, the status reports UDP packets sent and received, plus send errors. These counts describe attempts to reach the VPN server, not successful login. A received packet alone does not prove a valid handshake. After a handshake, SparkBox retries SSH through the tunnel. An attempt with no handshake is retried with a fresh tunnel after about two minutes.

## Reaching the cluster

Every node's SSH port and the Firewalla Gold, tested directly and, when the tunnel is up, through it. Auto mode uses these answers to decide whether the tunnel is needed. These are real checks, also in Demo.

| Column | What it shows |
|---|---|
| **Device** | The node or the Gold |
| **Address** | The address tested |
| **Path** | **Direct** or **Through the tunnel** |
| **Connect time** | How long a connection took. The status bar's figure is different: it's how long SparkBox's last command took |
| **Answers** | **Logged in**, **Port answers**, **No answer** or **Not set up** |
| **Checked** | When |

### When to use the tunnel

| Mode | What it does |
|---|---|
| **Auto** | Uses the tunnel only when the cluster isn't reachable directly. SparkBox checks every node and the Gold at once, and again whenever the network changes |
| **Always** | Always goes through the tunnel, even at home |
| **Off** | Never starts the tunnel |

When a node answers directly, **Auto** stays direct. Keep WireGuard or AmneziaWG disconnected when testing SparkBox’s own tunnel with the same profile. **Connect** and **Always** force the built-in tunnel.

**Direct at home** is a switch for **Auto** only. When no node answers, it keeps Auto direct if this Mac has a matching local subnet or no network at all. Leave it off when away: another network may use the same addresses as home. **Connect** still starts the tunnel.

### Expose to this Mac

Normally only SparkBox can use the tunnel. These are local ports that relay to your home network while the tunnel is up. A browser or another app on this Mac can then reach those addresses too. They're off at home, where the addresses answer directly.

| Column | What it shows |
|---|---|
| **On** | Turns the port on or off |
| **Label** | A name for it |
| **Local port** | The port on this Mac. Below 1024 gets a warning, because macOS usually uses those |
| **Home address** | Where it goes, such as `192.168.50.51:3000` |
| **Status** | **Listening** with **Open** and **Copy URL**, **Waiting for the tunnel**, **Not needed (direct)** or the error |

**Add from cluster…** lists ports your nodes are listening on. **Add port** adds an empty row. **Save ports** saves your changes, and **Revert** throws them away.

> [!WARNING]
> **An exposed port is open to any program on this Mac.** Expose only what you need, while you need it. To undo it, deselect **On** or remove the row, then click **Save ports**.

### Connection log

The last 50 events, newest first: state changes, network changes, your actions and messages from the tunnel. Each line says why, such as "Network changed to Wi-Fi Cafe · gateway 10.0.0.1; checking the path again."

### Profile

Click **Profile** to open this section. **Firewalla address** is the Gold's public address. **Save address** appears if you change it. **Choose .conf…** or **Replace .conf…** picks a file, and **Import** or **Import replacement** brings it in.

Below that, it lists the **Protocol**, **Tunnel address**, **Routes through the tunnel**, **Keepalive** and **Keys**. **Remove profile** deletes it.

SparkBox keeps no copy of the file. The private key goes into the Keychain, and the rest into SparkBox's settings. The file itself stays an unencrypted copy of the key, so [put it away](help:connect-the-vpn#put-the-file-away) after you import it.

## What it changes

The tunnel doesn't change your Mac's network settings. All of these change this Mac only.

| Action | What it changes | Needs | How to undo |
|---|---|---|---|
| **Connect**, **Disconnect**, **Follow mode** | Whether the tunnel runs | Admin mode, Touch ID or your Mac password | **Follow mode** |
| **Reconnect** | Restarts the tunnel | Look-only mode off | |
| **Auto**, **Always**, **Off**, **Direct at home** | When the tunnel runs | Admin mode, Touch ID or your Mac password | Choose again |
| **Save ports** | Which local ports listen | Admin mode, Touch ID or your Mac password | Change them and save again |
| **Import** | Saves the key in the Keychain and the rest in settings | Admin mode, Touch ID or your Mac password | **Remove profile** |
| **Remove profile** | Deletes SparkBox's copy of the keys from the Keychain | Admin mode, Touch ID or your Mac password | Import the `.conf` file again, or a new profile from the Firewalla app |
| **Screenshot-safe** | Masks addresses in this window | Nothing | Click it again |

## Related How-Tos

- [Reach your cluster with the VPN](help:connect-the-vpn)
- [Connect my Sparks](help:connect-my-sparks)
- [Switch between Demo and Live](help:switch-between-demo-and-live)

![VPN, using synthetic Demo data](img/public-vpn.jpg)
