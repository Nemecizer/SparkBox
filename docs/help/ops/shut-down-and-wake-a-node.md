---
id: shut-down-and-wake-a-node
title: Shut down and wake a node
type: howto
area: ops
summary: Turn a node off from SparkBox, and turn it back on with Wake-on-LAN or its power button.
keywords: [shut down, turn off, power off, wake, wake on lan, power on]
verified: false
related: [restart-a-node, node-window, turn-the-whole-cluster-off-and-on]
---

# Shut down and wake a node

To turn a node off, click **Shut down…** in its window. To turn it back on, click **Wake**, or press its power button.

## Before you start

- **Shut down…** needs the node to be online.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, shutting down and waking are simulated. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- A shut-down node runs nothing. Models it serves stop until it's back on.

### Know how you'll turn it back on

**Wake** uses **Wake-on-LAN**: a special network message that tells a switched-off computer to start. It can only work when all of these are true:

- **The node's network port listens for it.** To see the setting, open [Settings › Nodes](open:settings?section=nodes). Each node shows **Wake: On**, **Wake: Off** or **Wake: Unknown · needs sudo**. Unknown is normal on DGX OS, because only root can read the setting. It doesn't mean off, and it doesn't prove on.
- **SparkBox knows the node's network address.** It learns this while the node is on. A node that has never connected to SparkBox can't be woken.
- **Something on your home network can send the message.** That's another node that's online, or this Mac when it's on the same home network. A VPN connection from this Mac doesn't count, but a node that's still on can send it for you.
- **The node has power.** Wake-on-LAN can't help a node that's unplugged or lost its power.

If any of these fails, someone must press the node's power button.

> [!DANGER]
> **If Wake can't reach it, only someone at the machine can turn the node back on.** When you're away, never shut down every node: with none left on, nothing can send the message. If **Wake** hasn't been tried on a node, don't rely on it when nobody can get to the machine.

## Steps

### Shut down a node

1. In the sidebar, under **Nodes**, click the node, such as `cell3`. The node's window opens.
2. In the toolbar, click **Shut down…**, the power symbol with a line through it. If the window is narrow, it's under the **More** button (**…**) at the end of the toolbar.
3. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. A dialog named **Shut down cell3?** opens. Read the list. It names what stops with the node. If it can't promise that **Wake** will work, it says "You may not be able to turn cell3 back on remotely: SparkBox can't tell whether Wake-on-LAN is on."
5. Under **When**, choose **Now**, **In 1 minute** or **In 5 minutes**. You can call off a later time, but not **Now** once it's sent.
6. Under "Type cell3 to confirm", type the node's name. A shutdown always asks for this.
7. Click **Shut down**.
8. Watch the task "Shut down cell3" in the status bar. When it ends, the node shows **Off (planned)** in the sidebar.

To call off a scheduled shutdown, click **Cancel** on its task before its time comes. A shutdown set for **Now** can't be canceled once it's sent.

You can also start from the sidebar: right-click the node and choose **Shut down…**. On that route, SparkBox asks to unlock after you click **Shut down** in the dialog, not before.

### Wake a node

1. In the sidebar, under **Nodes**, click the node. The node's window opens.
2. In the toolbar, click **Wake**. It appears on any node that isn't online. You can also right-click the node in the sidebar and choose **Wake**. SparkBox asks nothing first, because waking breaks nothing.
3. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. Watch the task "Wake cell3". It sends the message, then waits up to 5 minutes for the node to come online.
5. When it ends, the sidebar shows the node's GPU use, such as **GPU 28%**.

If the node doesn't come on, go to it and press its power button once.

### Turn off the whole cluster

SparkBox has no single button for this, and it's for when you're at home. Unload your models, shut down each node, then press each power button to start again and check the sidebar. [Turn the whole cluster off and on](help:turn-the-whole-cluster-off-and-on) gives every step, and what to do after a power cut.

## What you should see

After a shutdown, the task says "cell3 is off (offline 9s after the shutdown was sent). It stays off until someone presses its power button (or Wake-on-LAN)." The node stays **Off (planned)**, and its alerts stay quiet until it's woken.

After a wake, the first step reads "Sent from cell1", and the task says "cell3 is on after 1m40s". SparkBox then waits for any models the node serves to answer.

If you press the power button instead, SparkBox notices when the node answers, and the sidebar shows its GPU use again, such as **GPU 28%**.

## Why it works

To shut down, SparkBox marks the node as planned, so nothing goes to your phone, and tells the node to power off, as root. A shutdown set for later is scheduled on the node, so it happens even if SparkBox quits. It has no checks like a restart has, because nothing boots until the node is woken. To wake a node, SparkBox sends a **magic packet**, a short message with the node's network address repeated, to everything on your home network. The node's network port hears it, and starts the machine.

## If something goes wrong

- **"cell3 didn't come online within 5 minutes. Wake-on-LAN may be off on its LAN port, or the packet didn't reach it."** Press its power button. Then check its **Wake** setting in [Settings › Nodes](open:settings?section=nodes).
- **"SparkBox doesn't know cell3's MAC address yet. It learns it while the node is on; wake it by its power button this time."** A MAC address is the fixed network address of a port. Press the power button. SparkBox learns the address once the node connects.
- **"No node is online to send the wake packet, and this Mac isn't on cell3's LAN. Wake another node first, or use its power button."** Connect this Mac to your home network, or press a power button at the machine.
- **"You won't be able to turn cell3 back on remotely: Wake-on-LAN is off on its LAN port."** The node's port doesn't listen for the message. Only its power button can start it. To change that, follow [Turn on Wake-on-LAN](help:check-your-nodes#turn-on-wake-on-lan).
- **"The shutdown didn't happen: cell3 kept answering for a minute. Check it with Fleet Command or a terminal."** The node didn't go down. Try again, or look at it in [Fleet Command](help:run-a-command-on-every-node).
- **"cell3 is already on."** It's running. There's nothing to wake.

For the other messages, see [Update and restart messages](help:errors-updates-and-power).

## Next steps

- [Turn the whole cluster off and on](help:turn-the-whole-cluster-off-and-on)
- [Restart a node](help:restart-a-node)
- [Check that your nodes are ready](help:check-your-nodes)
- [Reach your cluster with the VPN](help:connect-the-vpn)
