---
id: connect-your-cluster
title: Connect your cluster
type: tutorial
area: start
summary: Use Connect my Sparks to save your password, check every node and switch to Live data.
keywords: [first launch, setup, wizard, live]
verified: false
related: [connect-my-sparks, save-the-cluster-password, check-your-nodes, trust-a-host-key, switch-between-demo-and-live]
---

# Connect your cluster

## What you'll do

You'll connect SparkBox to the nodes you configure. At the end, the top bar reads **Live** and the number of online nodes. Each node in the sidebar shows its GPU use, such as **GPU 0%**, with no red word beside it. SparkBox will also have checked that it can log in and use sudo on each one. Most steps take a minute or two. Allow more time if you compare host keys at each node.

Nothing in this tutorial changes how your nodes run. SparkBox only logs in, reads, and keeps its own small helper files in `~/.sparkbox`.

## Before you start

- Your nodes are on and connected to your LAN, and this Mac is on the same network.
- You know your node account name and its password.
- You have Touch ID or your Mac password ready. SparkBox asks for it several times, because these are changes on this Mac.
- Look-only mode is off. If the padlock in the top bar reads **Look only**, follow [Use look-only mode](help:use-look-only-mode) first.
- To compare host keys in Step 5, use a trusted local console on each node. A screen and keyboard connected to that device work. Compare every device independently; do not assume matching keys prove distinct identities.

## Step 1: Save the cluster password

The password lets SparkBox log in to your nodes. It goes into the macOS Keychain and nowhere else.

1. Open the wizard. On the **Welcome to SparkBox** screen, click **Connect my Sparks**. If you don't see that screen, press <kbd>⌘,</kbd>, click **Nodes**, then click **Connect my Sparks…** under the table. The wizard opens with the heading **The account SparkBox logs in with**.
2. Enter the account name used on your nodes in **User**.
3. Click **Password** and type the cluster password.
4. Click **Save to Keychain**. macOS asks you to confirm. Use Touch ID or your Mac password.
5. Look for the green **Saved** label beside the box.
6. Click **Next**. You see the heading **Your Sparks**.

## Step 2: Check your node list

A new installation starts with no saved nodes or switches. Add your own addresses with **Add node**. The names and addresses elsewhere in Help are examples. Do not copy them into your network settings. In Live data, **Find Sparks on my network…** can survey your local /24 and propose verified nodes and reported neighbors; in Demo it uses fixtures. [Find Sparks on your network](help:connect-my-sparks#find-sparks-on-your-network) explains verification and review before saving.

1. Add each node and enter its name, address and account. Check these values against your own devices.

2. Leave **SSH port** at `22`.
3. Click **Next**. SparkBox saves the list. It may ask you to confirm with Touch ID or your Mac password. You see the heading **Checking each node**, and the checks start.

## Step 3: Switch to Live

Look at the top bar. If it already reads **Live**, skip to [Step 4](#step-4-run-the-real-checks).

If it reads **Demo**, the checks you see are simulated, and the page says "With Demo data these answers come from the simulator." They can't tell you about your real nodes. To run the real ones, switch to Live first.

1. Click **Next**. You see the **VPN** step. It's optional, and you can set it up later.
2. Click **Next**. You see the **Switches** step. It's optional too.
3. Click **Next**. You see the heading **Ready**, with "The simulated checks passed; nothing real was tested. Switch to Live to check your nodes."
4. Click **Switch to Live data**. Confirm with Touch ID or your Mac password if macOS asks. The **Dashboard** opens.
5. Look at the top bar. It now reads **Live**, and the padlock reads **Read-only**.

A message says "Admin mode locked", because moving to Live always locks it. Your nodes may show **Error** in the sidebar, with "Host key not trusted yet". That's expected, and the next steps fix it.

## Step 4: Run the real checks

Now SparkBox talks to your real nodes. These checks, including the sudo check, run on the devices you configured.

1. Press <kbd>⌘,</kbd>, click **Nodes**, then click **Connect my Sparks…**. The wizard comes to the front.
2. In the list of steps on the left, click **Checks**.
3. Click **Check again**. If the button reads **Check all nodes**, click that. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. Wait while each node shows **Checking…**.

The first time, each node stops at **Host key**, with "Not trusted yet (first connection)". If you've already connected to a node with your Mac's own `ssh` command, that node goes straight through.

## Step 5: Trust each node's host key

A host key proves a node is the machine you think it is. SparkBox won't send your password until you've trusted it.

> [!WARNING]
> **Trusting the wrong machine sends it your cluster password.** That password is also your sudo password. Compare the key before you click **Trust**. If you trusted a key by mistake, change the password on your nodes, then [save the new one](help:save-the-cluster-password).

Comparing is your decision: SparkBox can't do it for you, and **Trust** works without it. On your own home network, a wrong machine is unlikely. If you skip the comparison, you're relying on your network holding only what you think it holds. [If you can't compare today](help:trust-a-host-key#if-you-cant-compare-today) explains the choice.

Compare each node at its own trusted console. A cloned system can share a host key with another device; that does not prove that the address belongs to the intended node. [SSH, sudo and host keys](help:ssh-sudo-and-host-keys#your-nodes-shared-host-key) explains shared keys.

1. Beside the node, click **Trust…**. Leave the dialog showing its **Address** and **Key** open.
2. At that node's local console, log in using your own node account.
3. Type this command, then press <kbd>Return</kbd>:

   ```
   ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
   ```

4. Compare the full `SHA256:` fingerprint with the dialog. Every character must match. If they differ, click **Cancel** and read [Trust a node's host key](help:trust-a-host-key#if-something-goes-wrong).
5. Click **Trust**. Confirm with Touch ID or your Mac password if asked.
6. Wait for the node's checks to run again, then repeat at every other node.

## Step 6: Read the results

1. Read each node's list. You want a checkmark beside **Log in**, **sudo**, **Python 3**, **Docker**, **GPU driver** and **Fabric addresses**.
2. Look at **Wake-on-LAN**. If it reads "Not readable without sudo. NetworkManager leaves it to the driver default." with a dashed circle, SparkBox can still connect without Wake-on-LAN.
3. Read the line beside **Check again**. "Nothing failed" means you're done.
4. If a check failed, look it up in [Check that your nodes are ready](help:check-your-nodes) and fix it, then click **Check again**.
5. In the list of steps, click **Done**.
6. Click **Save and stay on Live**. The **Dashboard** opens. The top bar reads the number of online nodes, and each node in the sidebar shows its GPU use, such as **GPU 0%**, with no red word beside it. 0% is normal when no model is running.

## What you learned

- The cluster password lives in the Keychain, and each password change asks for Touch ID or your Mac password.
- In Demo, the checks are simulated. Real checks need Live.
- A host key proves which machine you reached, and you decide whether to trust it.
- Moving to Live locks Admin mode, so that a Demo habit can't reach a real node.
- You can run the wizard again at any time from **Settings › Nodes**.

## Next steps

- [Run your first real model](help:run-your-first-real-model)
- [Read the Dashboard](help:read-the-dashboard)
- [Add your switches and gateway](help:add-switches-and-gateway)
- [Reach your cluster with the VPN](help:connect-the-vpn), to use SparkBox away from home
