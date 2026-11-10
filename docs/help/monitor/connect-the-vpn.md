---
id: connect-the-vpn
title: Reach your cluster with the VPN
type: howto
area: network
summary: Import a WireGuard or AmneziaWG profile, choose when the tunnel is used, and connect from away.
keywords: [vpn, wireguard, amneziawg, away from home, outside my house, outside the house, remote, tunnel, profile, firewalla]
verified: false
related: [vpn, connect-my-sparks, keychain-and-secrets]
---

# Reach your cluster with the VPN

To reach your cluster when you're away from home, import your VPN profile into SparkBox, and let **Auto** decide when to use it.

## Before you start

- You need a **WireGuard** or **AmneziaWG** profile from your Firewalla Gold. WireGuard and AmneziaWG are VPN protocols: ways to send private traffic across the internet. SparkBox doesn't support AmneziaWG 2.0 settings.
- Be ready to unlock Admin mode, with look-only mode off. VPN settings are on this Mac, so each change asks for Touch ID or your Mac password, with **Demo** data too.
- You need the Gold's public address, if the profile doesn't have one.
- A **tunnel** is the private path between this Mac and your home network.

> [!NOTE]
> Demo data changes the cluster only. The tunnel, its keys and the exposed ports are real, also in Demo, as the window says with **Your real VPN, also in Demo**.

## Steps

### Get the profile from your Firewalla

1. In the Firewalla app, open the **VPN Server** page.
2. Turn on **WireGuard** or **AmneziaWG**, if it isn't on already.
3. Add a new client for this Mac. The app may call it a device. Firewalla makes a profile for it. Each client works on one device at a time, so don't reuse the one on your phone.
4. Open the new client, and send its profile to this Mac. Use a direct way, such as AirDrop from an iPhone. Don't send it by email or in a chat, because the file holds a private key.
5. Save the `.conf` file on this Mac.

Treat this file like a password. You put it away after you import it.

### Import it

6. In the sidebar, under **Network**, click **VPN**. [Open VPN](open:vpn) The **VPN** window opens.
7. Click **Import a profile…** in the note at the bottom. The **Profile** section opens.
8. Click **Choose .conf…**, and select the file. Selecting a replacement clears the old address.
9. Leave **Firewalla address** empty to use the server address in the file. If the file has none, enter the Gold’s public IP address or hostname. Type an override only if you need one.
10. Click **Import**.
11. SparkBox asks for Touch ID or your Mac password. Confirm it. The section now reads **Profile** with the protocol and date.

### Put the file away

SparkBox keeps no copy of the file, and it doesn't need it again: the private key is now in the Keychain. The `.conf` file is still an unencrypted copy of that key. Anyone who has it can connect to your home network through your Firewalla.

Do one of these now:

- Store the file in a password manager that can hold files.
- Move it into an encrypted disk image. **Disk Utility** on your Mac can make one.
- Delete it, then empty the Trash. If you need a profile later, get a new one from the Firewalla app.

### Choose when to use it

12. Scroll to **When to use the tunnel**.
13. Leave it on **Auto**. SparkBox uses the tunnel only when it can't reach the cluster directly.
14. If this Mac stays at home most of the time, turn on **Direct at home**. Then a cable or Wi-Fi drop at home doesn't start the tunnel.

### Connect

With **Auto**, you don't need to do anything: leave home, and SparkBox starts the tunnel by itself.

15. To test it now, click **Connect** in the toolbar. This uses the tunnel even if the cluster answers directly.
16. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
17. Watch the status card. It reads **Connecting**, then **Connected**, with **Handshake** a few seconds ago.
18. Check **Reaching the cluster**. Each node reads **Through the tunnel**, and **Logged in** or **Port answers**.
19. Click **Follow mode** to go back to **Auto**.


## What you should see

- **At home:** the card reads **Direct**, with "Direct on home LAN", and "Tunnel not in use: the cluster answers directly".
- **Away:** the card reads **Connected**, with "Tunnel via 203.0.•••.••" and the traffic each way.
- **In the status bar:** the VPN item shows the same state. Click it to open this window.

## Why it works

SparkBox tries the cluster directly first, whenever the network changes, when the Mac wakes, and every 20 seconds. If a node or the Gold answers, it stays direct. If none answers and this Mac isn't on the nodes' network, it starts the tunnel. The tunnel belongs to SparkBox alone, and it doesn't change your Mac's network. Nothing else on this Mac goes through it, except the ports you list under **Expose to this Mac**.

## Limits

- A November 7 retest disconnected the tunnel for about 34 seconds during a two-node Qwen reply. Reconnecting kept the same containers without a restart. The partial answer and its **interrupted** label survived reopening Chat; fresh Chat and clean unload passed. The earlier November 6 reply loss remains recorded. At home, nodes can fall back to the direct LAN; this test does not prove recovery from a complete outage while away. Use **Follow mode** after a temporary **Connect** test to restore the saved mode.
- The tunnel carries SparkBox's own traffic. To reach a page on a node from your browser while away, add it under [Expose to this Mac](help:vpn#expose-to-this-mac).
- SparkBox uses one profile at a time.

## If something goes wrong

- **"No answer from the Firewalla. Check its address, that the VPN server is on, and that this network allows UDP to that port. Still trying…"** The tunnel can't reach the Gold. Check the address under **Profile**, and try another network. Some networks block VPNs.
- **"The tunnel is up, but no node answered through it. Check AllowedIPs in the profile."** The tunnel works, but the profile doesn't route to your nodes. Export a new profile from the Firewalla app.
- **"This profile uses AmneziaWG 2.0 settings (S3/S4), which SparkBox's tunnel engine doesn't support yet. Firewalla's own profiles use AmneziaWG 1.5."** Export a profile from the Firewalla app, or use WireGuard.
- **"Enter the Firewalla's public IP address."** The file has no address, so type it in **Firewalla address**.
- **A matching local subnet keeps Auto direct.** Turn off **Direct at home** when away, or choose **Connect**. A remote network can use the same addresses as home.

See [VPN, fabric and switch messages](help:errors-network) for more.

### WireGuard works, but SparkBox has no handshake

Keep WireGuard disconnected while testing SparkBox with the same profile. Import that working file again, leave **Firewalla address** empty, and choose **Connect**. Check the status after about 20 seconds:

- **No UDP reply arrived:** compare the displayed server address and port with WireGuard. Check whether the network permits UDP to that server.
- **macOS could not send UDP traffic:** check the Mac’s network and the app’s network permissions.
- **UDP arrived, but no authenticated handshake completed:** confirm that the same current profile was imported, including any preshared key.
- **Connected, but nodes unavailable:** the handshake worked. Check the node reachability table and the profile’s **AllowedIPs**.

The connection log and status contain no private keys. Record the status and packet counts when reporting a failed attempt. Local tests do not establish that the home VPN server is reachable from this network.

## Next steps

- [VPN](help:vpn), to see every part of the window
- [Connect my Sparks](help:connect-my-sparks)
- [Where your passwords and keys go](help:keychain-and-secrets)
