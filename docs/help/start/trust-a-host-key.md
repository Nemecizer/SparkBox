---
id: trust-a-host-key
title: Trust a node's host key
type: howto
area: safety
summary: Confirm that a node is the machine you expect the first time you connect, or after you rebuild it.
keywords: [host key, fingerprint, trust, review, changed key]
verified: false
related: [ssh-sudo-and-host-keys, check-your-nodes, settings-nodes, leases-and-identity]
---

# Trust a node's host key

To let SparkBox connect to a node for the first time, or after you rebuild it, compare its host key and trust it.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, there are no real keys to trust.
- Trusting a key is a change on this Mac. SparkBox asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes.
- To compare the key, you need a screen and a keyboard plugged into the node. The screen needs an HDMI input and an HDMI cable. The keyboard needs a USB-C plug, or a USB-C adapter if it has a standard USB plug, because a Spark has no standard USB sockets. SparkBox can't reach a node it doesn't trust yet, so this is the one job its [Terminal](help:open-a-terminal) can't do for you.
- Comparing is your decision: SparkBox can't do it for you, and **Trust** works without it. If you can't compare today, see [If you can't compare today](#if-you-cant-compare-today).
- A **host key** proves a node is the machine you think it is. [SSH, sudo and host keys](help:ssh-sudo-and-host-keys) explains the idea.

> [!WARNING]
> **Trusting the wrong machine sends it your cluster password.** That password is also your sudo password. Compare the key before you click **Trust**. If you trusted a key by mistake, change the password on your nodes, then [save the new one](help:save-the-cluster-password).

## Steps

### Run a command at the node

A **command** is a line of text that tells the node what to do. At the node means sitting at the machine itself, with its own screen and keyboard. You don't need any experience for this.

1. At the back of the node, plug the screen into the HDMI socket and the keyboard into a free USB-C socket. One USB-C socket holds the power cable, so leave that one alone. Turn the screen on.
2. Log in using your own node account and its password. `sparkuser` below is a synthetic example. Nothing may appear on the screen as you type the password. That's normal. Press <kbd>Return</kbd> when you've typed it.
3. If you see a desktop, press the Windows key, or <kbd>⌘</kbd> on an Apple keyboard. Type `terminal`, then press <kbd>Return</kbd>. A window opens with a command prompt. If you see only lines of text, you're already at the prompt.
4. Find the prompt, a line that ends in `$`, such as `sparkuser@cell1-example:~$`.
5. Type the command exactly as it's written, then press <kbd>Return</kbd>. If it starts with `sudo`, it asks for the cluster password. Nothing appears as you type that either.
6. Read what the command prints. To finish, type `exit` and press <kbd>Return</kbd>. If the screen is still logged in, log out.

### Trust a new node

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Nodes settings](open:settings?section=nodes)
2. In the list on the left, click **Nodes**. A node waiting for you shows **Trust host key…** in its row. The same button appears in the node's own window, on its **Dashboard** row and in its right-click menu.
3. Click **Trust host key…**. A dialog named **Trust cell1's host key?** opens. It shows the node's **Address** and **Key**. Leave it open.
4. Go to the node and [run this command](#run-a-command-at-the-node):

   ```
   ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
   ```

   It prints a line with a `SHA256:` fingerprint.
5. Compare that fingerprint with the **Key** in the dialog. Every character must match. If they differ, click **Cancel** and stop: see [If something goes wrong](#if-something-goes-wrong).
6. Click **Trust**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
7. Read the message that appears: "Trusted cell1's host key", with "SparkBox connects to it on its next try, within 30 s."
8. Repeat for each node that's waiting.

If multiple nodes present the same key, compare each device independently at its trusted console. A shared key does not distinguish those machines. [Shared host keys](help:ssh-sudo-and-host-keys#your-nodes-shared-host-key) explains the limitation.

If you're using [Connect my Sparks](help:connect-my-sparks), the **Checks** step has its own **Trust…** button beside each key. It opens the same dialog, and it runs that node's checks again afterward.

### If you can't compare today

You can finish setting up without comparing, and compare the next time you're at the node. Here's what you're deciding:

- **How likely is a wrong machine?** On your own home network, with only your own devices on it, it's unlikely. A wrong machine would have to be on your LAN and answering at the node's address. A mix-up is more likely, such as a mistyped address.
- **What does skipping it cost?** You rely on your network holding only what you think it holds. If it doesn't, the machine that answered receives your cluster password.

On a network you don't control, don't skip it: wait until you can compare. If you're on your own home network and you accept the risk, do this:

1. Click **Trust**, and confirm with Touch ID or your Mac password.
2. When the node is connected, open its window from the sidebar, under **Nodes**, and click the **System** tab.
3. Find the **Host key** row. Move the pointer over the short key to see all of it. That's the key you trusted.
4. The next time you're at the node, [run the command](#run-a-command-at-the-node) from **Trust a new node** and compare the two.
5. If they differ, treat your cluster password as known to someone else. At each node, [run the command](#run-a-command-at-the-node) `passwd`, and follow its questions to set a new password. Nodes using the cluster password must share it. Then [save the new one](help:save-the-cluster-password).

### If a node's key changed

A node's key changes when you reinstall it. If you didn't reinstall or rebuild the node, don't trust the new key.

> [!WARNING]
> **A changed key you didn't expect can mean another machine is answering in the node's place.** Click **Cancel** and find out why before you go on. Nothing is sent to the node while you do.

1. In **Settings › Nodes**, click **Review host key…** in the node's row. A dialog named **cell2's host key changed** opens.
2. Go to the node and [run the command](#run-a-command-at-the-node) from **Trust a new node**.
3. Compare the fingerprints. If they differ, click **Cancel** and stop.
4. In the box, type the node's name, such as `cell2`.
5. Click **Trust new key**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.

SparkBox replaces the key and writes both the old and the new key to the activity log. It also clears the node's [identity pin](help:leases-and-identity#identity-pin), and makes a new one when the node next connects.

### Start over on every node

Use this only if you reinstalled several nodes.

1. In **Settings**, click **Diagnostics**.
2. Under **SSH host keys**, click **Forget host keys…**, then click **Forget keys**.
3. Confirm with Touch ID or your Mac password.
4. Follow **Trust a new node** above for each node.

Forgetting doesn't trust anything by itself. Each node asks to be trusted again, unless your Mac's own `~/.ssh/known_hosts` file already has its key.

## What you should see

Within 30 seconds, the sidebar shows the node's GPU use, such as **GPU 0%**, in place of the red word. 0% is normal when no model is running. In **Settings › Nodes**, the **Trust host key…** button goes away. If several nodes share a key, a line under the table says so: "cell1, cell2 and cell3 share one host key, SHA256:example… (a cloned image) · trusted by you".

## Why it works

SparkBox now holds the key for that address and port. On every later connection, it checks the node's key before it sends anything. If the key ever differs, it refuses to connect.

If you've already accepted a node's key with your Mac's own `ssh` command, SparkBox accepts that exact key without asking.

## If something goes wrong

- **The fingerprints don't match, and you didn't change the node.** Don't trust the key. Check that you're looking at the right node and the right address. If they're right, something may be intercepting the connection: look at what's on your network before you go on.
- **"The host key for 192.168.50.154:22 changed".** The full message gives the old and new keys. Follow **If a node's key changed** above.
- **"cell2's host key changed. Type cell2 to trust the new key."** The name you typed didn't match. Type it again, exactly.
- **"known_hosts.json couldn't be read".** SparkBox's list of keys is damaged, so it refuses every connection to be safe. Follow **Start over on every node** above.
- **"Couldn't trust the host key".** The message gives the reason. If it's about look-only mode, [turn that off](help:use-look-only-mode).
- **The node still isn't online after 30 seconds.** The key may be fine, and the next check may be failing. Follow [Check that your nodes are ready](help:check-your-nodes).

## Next steps

- [Check that your nodes are ready](help:check-your-nodes)
- [SSH, sudo and host keys](help:ssh-sudo-and-host-keys)
- [Controller lease and identity pin](help:leases-and-identity)
