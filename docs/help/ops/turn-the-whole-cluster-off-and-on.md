---
id: turn-the-whole-cluster-off-and-on
title: Turn the whole cluster off and on
type: howto
area: ops
summary: Shut down all your nodes in a safe order, start them again, and bring the cluster back after a power cut.
keywords: [whole cluster, all nodes, every node, turn everything off, turn everything on, shut everything down, start the cluster, power cut, power outage, blackout, move the cluster, moving the sparks]
verified: false
related: [shut-down-and-wake-a-node, unload-a-model, recover-a-frozen-node, find-out-why-a-node-restarted]
---

# Turn the whole cluster off and on

To turn the whole cluster off, unload your models and shut down each node in turn. To turn it back on, press each node's power button and check the sidebar.

Use this before you move the equipment, or before planned work on your home's power. If the power has already gone, go to [After a power cut](#after-a-power-cut).

## Before you start

- SparkBox has no single button for this. You shut down one node at a time, and each one asks you to confirm.
- Be at home. You need to reach every node's power button to start it again.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, shutting down is simulated. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- Everything running on the nodes stops, including any download or copy. Your model files stay on disk, so there's nothing to download again.
- If you set up a [dead-man's switch](help:set-up-always-on-monitoring#add-a-dead-mans-switch), it pages you when the nodes go quiet. To **page** means to send an alert to your phone. That page is expected here.

> [!DANGER]
> **With every node off, someone may have to go to each machine to start it.** **Wake** then depends on this Mac being on your home network, and it hasn't been tried on your cluster. Never turn off the whole cluster when you're away.

## Steps

### Stop what's running

1. In the sidebar, under **Tools**, click **Tasks**. If you don't see it, scroll the sidebar or type in **Filter**. [Open Tasks](open:tasks) The **Tasks** window opens.
2. Click **Running**. If a task is listed, wait for it to end. SparkBox won't shut down a node that's installing packages.
3. In the sidebar, under **Workloads**, click **Models**. [Open Jobs](open:models?tab=jobs) The **Models** window opens.
4. Click the **Jobs** tab. If a download or copy is running, wait for it to end, or select it and click **Cancel**.
5. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) You see one card for each model server.
6. On each card that has **Unload…**, click it, then follow [Unload a model](help:unload-a-model). A card with no **Unload…** needs nothing here, because its server stops with its node.

### Shut down each node

Leave `cell1` until last. It's the first node in your list, and SparkBox uses that node to check its route to the cluster.

7. In the sidebar, under **Nodes**, click `gx10-0000`. The node's window opens.
8. In the toolbar, click **Shut down…**, the power symbol with a line through it. If the window is narrow, it's under the **More** button (**…**) at the end of the toolbar.
9. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
10. A dialog named **Shut down gx10-0000?** opens. Read the list. It names what stops with the node. A warning that you may not be able to turn it back on remotely is fine here, because you'll use its power button.
11. Under **When**, leave **Now** selected.
12. Under "Type gx10-0000 to confirm", type the node's name.
13. Click **Shut down**.
14. Wait until the sidebar shows **Off (planned)** for the node.
15. Repeat steps 7 to 14 for `cell3`, then `cell2`, then `cell1`.
16. If you need the power off, turn off your switches and gateway at the devices, or unplug them. SparkBox can't do this. Your gateway also runs your home internet, so leave it on unless you need it off.

### Turn the cluster back on

> [!TIP]
> **Wake** can replace the power button when the nodes have power and this Mac is on your home network. Once one node is on, that node sends the message to the others. [Wake a node](help:shut-down-and-wake-a-node#wake-a-node) shows how. It has run only on test systems, so be ready to use the button.

17. If you turned off your gateway or switches, turn them on first. SparkBox reaches your nodes through them.
18. In SparkBox, look in the sidebar, under **Network**. Wait until each device says **Online**. A device you haven't set up says **Not set up**.
19. Go to each node and press its power button once. It's on the back of the node. If you unplugged a node, it starts when you plug it in, so leave its button alone.
20. Watch the sidebar, under **Nodes**. A node shows **Off (planned)** until it answers, then its GPU use, such as **GPU 28%**. A node usually takes 2–3 minutes to come back.
21. In the sidebar, under **Network**, click **Fabric**. [Open Fabric](open:fabric) Read the band at the top. [Check the fabric](help:check-the-fabric) explains what it says.
22. Load your models again. [Load a model on one node](help:load-a-model-on-one-node) shows how.

### After a power cut

A power cut stops every node at once, with no warning. Start here when the power is back.

1. Check that your gateway and switches have power. SparkBox can't reach any node until they do.
2. Open SparkBox. Every node reads **Offline** until the power and the network are back. [Alerts](open:alerts) may say "This Mac lost the home network". That clears by itself when the network returns.
3. In the sidebar, under **Network**, wait until each device says **Online**. A device you haven't set up says **Not set up**. If one stays **Offline**, go to [Nothing connects](help:troubleshooting#nothing-connects).
4. Under **Nodes**, wait 5 minutes. A node that starts by itself shows **Connecting**, then its GPU use, such as **GPU 28%**.
5. Go to each node that still reads **Offline**, and press its power button once. It's on the back of the node.
6. Wait up to 5 more minutes. If a node still reads **Offline**, follow [Recover a frozen node](help:recover-a-frozen-node).
7. In the sidebar, under **Overview**, click **Alerts**. [Open Alerts](open:alerts) If SparkBox was open during the cut, a critical alert reads like "cell3 restarted on its own at 6:08 PM · cell1, cell2, gx10-0000 too". That's expected.
8. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) A model comes back only if you turned on **Keep running** when you loaded it. To run any other model, [load it again](help:load-a-model-on-one-node).
9. In the sidebar, under **Network**, click **Fabric**. [Open Fabric](open:fabric) Read the band at the top. [Check the fabric](help:check-the-fabric) explains what it says.

## What you should see

With every node off, the sidebar shows **Off (planned)** for each node, and the top bar reads **0/4 online**. [Tasks](open:tasks) keeps each result, such as "cell3 is off (offline 9s after the shutdown was sent). It stays off until someone presses its power button (or Wake-on-LAN)."

With the cluster back on, the sidebar shows each node's GPU use, such as **GPU 28%**, and the top bar reads **4/4 online**. In a node's **System** tab, under **Boots**, the boot before reads **Clean shutdown** or **Clean shutdown (SparkBox restarted it)**.

After a power cut, you see the same sidebar and top bar. **Boots** instead reads **Ended without a shutdown (power loss, hang or watchdog)**.

## Why it works

Unloading first stops each model cleanly, so **Keep running** can't start it again when the nodes come back. A shutdown from SparkBox is planned work, so its alerts stay quiet. A node's disk keeps your model files, so they're still there afterward. A power cut gives no warning, so SparkBox treats it as a fault: it raises alerts, and each node's record shows no clean shutdown.

## Limits

- **No single button.** SparkBox shuts down one node at a time. It can't turn the whole cluster off or on in order.
- **Your switches and gateway are out of its reach.** SparkBox can't turn their power off or on.
- **No warning of a power cut.** SparkBox can't shut your nodes down for you when the power fails.
- **What a node does when the power returns isn't certain.** [NVIDIA says](https://forums.developer.nvidia.com/t/dgx-spark-gb10-faq/347344) its Spark turns on as soon as you plug it in. It doesn't say what happens after a power cut, and your nodes are MSI and ASUS models of it. So the steps check each node, and you press the button only on a node that stays **Offline**.

## If something goes wrong

- **"cell3 is installing packages (Upgrade libbrotli1 on cell3); shut it down when that finishes."** Wait for the upgrade to end in [Updates](open:updates), then try again.
- **"cell3 isn't connected, so it can't be told to shut down."** The node is already off, or SparkBox can't reach it. See [A node shows Offline](help:troubleshooting#a-node-shows-offline).
- **"The shutdown didn't happen: cell3 kept answering for a minute. Check it with Fleet Command or a terminal."** The node stayed on. Try again, or look at it in [Fleet Command](help:run-a-command-on-every-node).
- **Every node stays Offline when the power is back.** Go to [Nothing connects](help:troubleshooting#nothing-connects).
- **A node stays Offline after you press its button.** Follow [Recover a frozen node](help:recover-a-frozen-node).
- **"cell2 answers as cell3-example: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence."** After a power cut, your gateway may give a node another node's address. Check each node's address in [Nodes settings](open:settings?section=nodes). [Controller lease and identity pin](help:leases-and-identity) explains why SparkBox refuses.
- **"The job stopped unexpectedly (node restarted?)."** A download or copy was running when the node went off. See [Start again](help:follow-a-download#start-again).
- **An upgrade was running when the power went.** When the node is back, SparkBox checks the packages and says what it found. If it says to run `sudo dpkg --configure -a`, read the list of packages first. If it names an NVIDIA or kernel package, stop and [get help first](help:errors-updates-and-power#the-upgrade-finished-but-dpkg-reports-packages-that-arent-fully-installed-on-cell2-run-sudo-dpkg-configure-a-there). Otherwise, open a [terminal](help:open-a-terminal) on the node and run that.

## Next steps

- [Load a model on one node](help:load-a-model-on-one-node)
- [Check the fabric](help:check-the-fabric)
- [Find out why a node restarted](help:find-out-why-a-node-restarted)
- [Shut down and wake a node](help:shut-down-and-wake-a-node)
