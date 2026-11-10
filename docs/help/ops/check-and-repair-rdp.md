---
id: check-and-repair-rdp
title: Check and repair RDP
type: howto
area: tools
summary: Check a node's remote desktop connection, review a repair, then test your desktop login.
keywords: [rdp, remote desktop, repair, xrdp, gnome, xfce]
verified: false
related: [rdp, open-a-terminal, admin-mode]
---

# Check and repair RDP

Check encrypted remote desktop access and repair supported settings on one node.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode with Touch ID or your Mac password. **Demo** simulates the same controls.
- Connect the node through its saved trusted SSH connection. Finish any protected system update on it before repair.
- Install xrdp, xorgxrdp, the software renderer and your chosen desktop through your normal package process. SparkBox reports missing prerequisites.
- Have an RDP client and your Linux login password. Your Mac needs a LAN or system VPN route to the node.
- Save your desktop work and log out of every RDP session before repair. Repair restarts the RDP services and may enable access on a different port. Its review shows that port; the old configuration is backed up privately on the node.

## Steps

1. In the sidebar under **Tools**, click **RDP**. The window lists your nodes.
2. Click **Check all nodes**, or **Check cell2** to check that node. Unlock Admin mode when asked. Read both the node-side and Mac-side connection results.
3. If the desktop needs repair, leave **Desktop for repair** at **Keep existing desktop**. To select an installed desktop explicitly, choose **Ubuntu GNOME** or **XFCE**, then check again.
4. On the node you want to fix, click **Review repair…**. Read the port, desktop, certificate and service changes. Type that node's name and click **Repair RDP**. SparkBox checks that the review is still current, keeps a backup and verifies the repaired service.
5. Read the new results and keep the backup path for recovery. In your RDP client, open the displayed **PC name**. Compare the displayed certificate identity if asked, select **Xorg** and log in using your Linux password.
6. Check that the desktop appears and an application opens. Log out, reconnect and confirm that a new desktop session also works.

## What you should see

The individual connection checks say **Passed**, and the node can say **Encrypted connection checked**. The RDP client must also show a usable desktop after login. SparkBox does not claim that last check on your behalf.

## Why it works

SparkBox reads the settings through trusted SSH and checks the answering RDP certificate against that reading. The repair checks for changed files and existing desktop sessions before it writes. It keeps the previous configuration so a failed service check can restore it.

## If something goes wrong

**Check incomplete** means that a read or required prerequisite was unavailable. Read the message, resolve the missing package, account or route requirement, then check again. **Review repair…** stays dimmed while a blocking condition remains.

If a session is still present, use that desktop's **Log Out** command; closing the RDP client leaves it running. If repair reports changed settings, check again before approving a fresh review. If a protected system update is pending, finish its verification first.

If the node-side test passes but the Mac-side test fails, check your LAN or system VPN route and firewall rules. Repair does not open the firewall. SparkBox's built-in VPN does not route your separate RDP client.

If repair fails, read whether rollback completed. Keep its backup on the node, compare any later edits and follow the [RDP recovery guidance](help:rdp#what-it-changes). Do not repeat repairs blindly after an interrupted operation.

## Next steps

- [RDP](help:rdp)
- [Open a terminal on a node](help:open-a-terminal)
