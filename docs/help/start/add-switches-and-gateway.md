---
id: add-switches-and-gateway
title: Add your switches and gateway
type: howto
area: network
summary: Tell SparkBox the addresses of your CRS804, Firewalla Switch X and Firewalla Gold so that it can watch them.
keywords: [crs804, firewalla, routeros, rest, switch]
verified: false
related: [settings-switches, switch-window, firewalla-view, connect-my-sparks]
---

# Add your switches and gateway

To let SparkBox watch your network devices, give it their addresses in the **Switches** step of **Connect my Sparks**.

## Before you start

- Use **Live** data if you can. With **Demo** data, the test only answers for the simulated switch.
- Look-only mode must be off.
- You need the LAN address of each device. Your Firewalla app lists them.
- Saving a password asks for Touch ID or your Mac password every time. Saving an address asks unless you confirmed in the last 5 minutes.
- All three devices are optional. Leave an address empty to skip that device.
- For the `CRS804 DDQ`, you first have to set up the switch. The steps are below. The Firewalla devices need no setup.

## Steps

### Set up the CRS804 DDQ

SparkBox reads the CRS804 DDQ through its **REST API**, a way for apps to ask a switch for its figures over a secure web connection. You do these three steps on the switch, in MikroTik's WinBox app or the switch's web page. They need RouterOS 7.1 or later. [MikroTik's REST API guide](https://help.mikrotik.com/docs/spaces/ROS/pages/47579162/REST+API) has the details.

1. Under **System › Certificates**, make sure the switch has a certificate. If it doesn't, create one and sign it.
2. Under **IP › Services**, turn on `www-ssl`, and choose that certificate.
3. Under **System › Users**, create a user for SparkBox, such as `sparkbox`, in a group with the `read` and `rest-api` policies. Give it its own password.

That's enough for SparkBox to read the switch. To change ports from SparkBox, the group also needs the `write` and `test` policies. See [Manage a switch port](help:manage-a-switch-port).

### Enter the addresses

4. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Nodes settings](open:settings?section=nodes)
5. In the list on the left, click **Nodes**, then click **Connect my Sparks…** under the table. The wizard opens.
6. In the list of steps on the left, click **Switches**. You see one group each for the **CRS804 DDQ**, the **Firewalla Switch X** and the **Firewalla Gold**.
7. Under **CRS804 DDQ**, type its address in **Address**.
8. In **REST user**, type the user you created, such as `sparkbox`.
9. In **REST password**, type its password, then click **Save to Keychain**. Confirm with Touch ID or your Mac password. "Saved in the Keychain." appears under the box.
10. Click **Test**. SparkBox saves the list, then tries the switch. Confirm with Touch ID or your Mac password if it asks.
11. If the message says the switch "presented a certificate SparkBox hasn't trusted yet", compare the SHA-256 value with **System › Certificates** on the switch. If they match, click **Trust…**, then confirm with Touch ID or your Mac password.
12. Check the message. It starts with "Connected to", then names the switch, the user and the RouterOS version.
13. Under **Firewalla Switch X**, type its address, then click **Test**. Do the same under **Firewalla Gold**.

To change these details later, use [Settings › Switches & gateway](help:settings-switches). If the certificate changes later, trust the new one here in the wizard, from **Test connection** in the [Switch window: CRS804 DDQ](help:switch-window), or from **Test connection** in **Settings › Switches & gateway**.

## What you should see

Each **Test** shows a green message. For a Firewalla device, it says the device answered, and how many milliseconds that took.

In the sidebar, under **Network**, each device now shows its state. Click one to open its window. The CRS804 DDQ's window has its ports, optics and neighbors. A Firewalla device's window shows how quickly it answers and how the LAN looks from each node. The Switch X's window also shows its ports.

## Why it works

The CRS804 DDQ offers its figures over the REST API, so SparkBox logs in and reads them. The first time, it shows you the switch's certificate, the same way it shows you a node's host key. Once you trust it, SparkBox refuses any other. Firewalla devices have no local API, so SparkBox checks that they answer on port 443 or 80, and how fast, with no password. It learns about the Switch X's ports from what your nodes hear.

## If something goes wrong

- **"192.168.50.2 refused user "sparkbox" (401)."** The password is wrong, or the user's group doesn't have the `read` and `rest-api` policies. Save the password again, or fix the group on the switch.
- **"192.168.50.2 doesn't offer the REST API".** `www-ssl` is off or has no certificate. Follow **Set up the CRS804 DDQ** above.
- **"Save the REST password for CRS804 DDQ in Settings › Switches & gateway."** Save the password first.
- **"No answer from 192.168.50.2 within 5 s."** The address may be wrong, or the switch is off. Check it. If you're away from home, [connect the VPN](help:connect-the-vpn).
- **"Couldn't reach 192.168.50.2".** The message gives the reason.
- **A message that says the certificate changed.** If you replaced the certificate on the switch, click **Trust…**. If you didn't, stop: something may be intercepting the connection.
- **"Firewalla Gold didn't answer on port 443 or 80."** Check the address, and that the device is on.
- **"Demo data: the simulated switch answers. Switch to Live data to test the real one."** You're in Demo, so this test proves nothing about your switch.

[VPN, fabric and switch messages](help:errors-network) lists the rest.

## Next steps

- [Switch window: CRS804 DDQ](help:switch-window)
- [See what's plugged into the Switch X](help:see-whats-plugged-into-the-switch-x)
- [See what a port connects to](help:see-what-a-port-connects-to)
- [Connect your cluster](help:connect-your-cluster)
