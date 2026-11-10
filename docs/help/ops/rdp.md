---
id: rdp
title: RDP
type: feature
area: tools
summary: Check encrypted remote desktop access and review repairs to a node's RDP settings.
keywords: [rdp, xrdp, desktop, gnome, xfce, certificate, remote desktop]
verified: false
related: [check-and-repair-rdp, open-a-terminal, admin-mode]
---

# RDP

## What it's for

**RDP** checks whether a node's remote desktop service answers securely. It can repair an installed xrdp server's settings for Ubuntu GNOME or XFCE, the two supported desktops.

The connection check does not log in or open a desktop. You finish that check in your RDP client with your Linux password.

## Where to find it

- **Sidebar:** under **Tools**, click **RDP**, directly below **Terminal**.
- **Menu bar:** choose **Go › RDP**.
- **Command palette:** press <kbd>⌘K</kbd>, type `rdp`, and press <kbd>Return</kbd>.
- **Link:** [Open RDP](open:rdp)

## What's on the screen


### Checks and desktop choice

**Check all nodes** reads each node in turn. **Check cell2**, or the corresponding node button, checks one node. Both ask for Admin mode because certificate access and some settings require root.

**Desktop for repair** starts at **Keep existing desktop**. It recognizes the launchers used by the supported setup scripts. Select **Ubuntu GNOME** or **XFCE** to request that installed desktop explicitly. Changing this choice clears the previous review.

### Each node

The node shows its **PC name**, **Username**, desktop and individual results. **Passed** means that specific check succeeded. **Needs attention** identifies a failed check; **Not verified** records a limit or information that needs your review. A failed read stays **Check incomplete**.

The node checks software, service state, existing desktop sessions, TLS certificate access, software display settings and Linux password status. It lists UFW firewall information without changing rules. An encrypted connection is tested on the node and separately from this Mac. Each test checks that the answering certificate matches the public certificate read through trusted SSH.

**Review repair…** shows the exact proposed settings and asks you to type the node's display name. A missing prerequisite, masked service or existing desktop session blocks repair. Disconnected desktops count too: save your work and log out before repairing.

The certificate's **SHA-256** value lets you compare the identity shown by your RDP client. A new certificate changes this value. Review any certificate warning in that client yourself.

### Connection limits

**Encrypted connection checked** does not establish that your password works or that a desktop starts. Open the displayed address in an RDP client, select **Xorg** and log in to finish testing. Changing the desktop launcher affects your next new session; closing the client window does not log out of the old desktop.

SparkBox's built-in VPN carries SparkBox's own traffic. It does not give another app a network route. Your RDP client needs a direct LAN connection or a system VPN that reaches the node. A node-side pass with a Mac-side failure points to the route or firewall as well as the listener.

## What it changes

Checks make no RDP configuration changes. Checks and repairs need Admin mode and look-only mode off. Repairs require Ubuntu 24.04 on an arm64 node, an installed xrdp server and the selected desktop. Missing packages, Linux password setup and firewall rules remain owner actions; SparkBox explains the missing prerequisite.

Repair preserves a usable port from 3389 through 3399. If another service owns it, the review selects a free port in that range. It configures TLS 1.2 or 1.3, the chosen launcher and an xrdp software display. It creates a dedicated certificate only when the current certificate or key access is unusable. It then enables xrdp at startup, restarts its two services and checks encrypted RDP again. GNOME Remote Desktop, GPU drivers, packages and other services are not changed. The node is not restarted.

A private folder under `/var/backups/` keeps previous files and a manifest of their permissions and service states. Its path appears after repair or in the failure message. It can contain private certificate material; keep it on the node. A failed repair restores files only while they still match SparkBox's edits, then restores the prior service state. A concurrent edit or new desktop session can require manual recovery instead.

To undo a completed repair, first log out of RDP desktops. In [Terminal](help:open-a-terminal), use an administrator to restore the recorded files, ownership, permissions and service states from that backup's manifest. The original locations are recorded there; do not copy the whole backup into one directory. Do not restore over later owner edits without comparing them. Recheck RDP afterward.

## Related How-Tos

- [Check and repair RDP](help:check-and-repair-rdp)
- [Open a terminal on a node](help:open-a-terminal)

![RDP, using synthetic Demo data](img/public-rdp.jpg)
