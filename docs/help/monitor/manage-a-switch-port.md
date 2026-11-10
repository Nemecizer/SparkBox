---
id: manage-a-switch-port
title: Manage a switch port
type: howto
area: network
summary: Enable, disable or bounce a port on the CRS804, with the safeguards that bring it back if something goes wrong.
keywords: [port, bounce, disable, enable, crs804, revert, keep disabled, identify, reset counters]
verified: false
related: [switch-window, see-what-a-port-connects-to, check-the-fabric]
---

# Manage a switch port

To restart a misbehaving link, or turn a port off for a while, use the toolbar in the CRS804's window.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, these buttons act on a simulated switch, so nothing can go wrong.
- You need the switch [set up in SparkBox](help:add-switches-and-gateway).
- These steps work on the CRS804 only. The Firewalla devices are managed in the Firewalla app.

> [!DANGER]
> **A wrong port can cut a node off the fabric, or cut SparkBox off the switch.** Disabling a node's port stops its 200G link, and any model running across nodes stops serving and has to reload. If that port carries SparkBox's own connection to the switch, SparkBox loses the switch until the port comes back. Read the node name on the port before you act. The safeguards below bring a port back by themselves, but a port you choose to keep disabled stays down until you enable it.

## Steps

### Choose the port

1. In the sidebar, under **Network**, click **CRS804 DDQ**. [Open CRS804 DDQ](open:switch?id=crs804) The switch's window opens.
2. Click the port in the drawing, or its row on the **Ports** tab. Check the **Node** column, so you know what's on it.
3. To be sure which cable it is, click **Identify** in the toolbar. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. That port's light blinks for 10 seconds.


### Bounce a port

A **bounce** takes a port down for 3 seconds and brings it back. It's the usual first step for a link that misbehaves.

1. In the toolbar, click **Bounce…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A dialog says what happens and lists any model that will stop serving.
2. Click **Bounce port**. A message reads "1-2 bounced; the switch brings it back in 3 s".

### Disable a port

1. In the toolbar, click **Disable…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A dialog says what will lose its link, and lists any model it stops.
2. Click **Disable port**. A banner counts down: "qsfp56-dd-1-2 is disabled. The switch re-enables it in 60 s unless you keep it disabled."
3. Decide before the countdown ends:
   - To let it come back, do nothing. The switch re-enables the port by itself after 60 seconds.
   - To keep it off, click **Keep disabled** in the banner. A message reads "1-2 stays disabled".

### Enable a port

1. Select the disabled port.
2. In the toolbar, click **Enable**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A message reads "1-2 enabled".

### A port that carries SparkBox's connection

The switch's window marks such a port **SparkBox's path**, with a padlock. Your connection to the switch runs through it, so SparkBox guards it.

1. Click **Disable…** or **Bounce…** as usual. The dialog says this port carries SparkBox's connection.
2. Type the exact phrase it asks for, such as `disable 1-1` or `bounce 1-1`.
3. Click the button. SparkBox loses the switch until the port comes back.

A disable on this port can't be kept. The switch always brings it back after 60 seconds.

## What you should see

- **During a bounce:** the port reads **No link** for a few seconds, then **Up**.
- **While disabled:** the port reads **Disabled**, its drawing is hatched, and the countdown banner shows.
- **After it comes back:** the banner goes, and the port reads **Up**.

## Why it works

The safeguards run on the switch itself, not on this Mac, so they work even if SparkBox loses its connection. Before a disable, SparkBox leaves a scheduled job on the switch that re-enables the port 60 seconds later and then removes itself. If that job can't be scheduled, nothing is disabled. **Keep disabled** removes the job, and only once the switch still answers. A bounce is one script on the switch: down, wait 3 seconds, up, remove itself.

## Limits

- **Reset counters** starts a port's errors, drops, pause frames and link-downs from zero. The old totals are gone, and that can't be undone.
- **Comment…** saves a short note on the port. SparkBox matches it to a node when it can't tell another way.
- Each change goes in the [Activity log](help:activity-log).

## If something goes wrong

- **"Couldn't leave a revert on the switch, so qsfp56-dd-1-2 wasn't disabled:" followed by a reason.** SparkBox won't disable a port without its safeguard. Nothing changed. Fix the reason, then try again.
- **"The switch refused".** The message says why. See [VPN, fabric and switch messages](help:errors-network).
- **"qsfp56-dd-1-2 carries SparkBox's connection to the switch, so it can't stay disabled."** That port can't be kept off. Wait for it to come back.
- **"The revert stays".** SparkBox couldn't confirm the switch still answers, so it left the safeguard in place. The port comes back by itself.
- **A disabled port doesn't come back.** You may have kept it disabled. Select it and click **Enable**. If SparkBox can't reach the switch at all, use WinBox or WebFig, MikroTik's own tools, or the switch itself.

## Next steps

- [Check the fabric](help:check-the-fabric), to confirm the link is healthy
- [Test the fabric](help:test-the-fabric)
- [Switch window: CRS804 DDQ](help:switch-window)
