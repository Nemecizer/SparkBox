---
id: errors-connection
title: Connection and login messages
type: troubleshooting
area: troubleshooting
summary: What SSH, password, sudo, host-key and reachability messages mean, and what to do.
keywords: [offline, ssh timeout, password rejected, no reply, can't connect, sudo, host key, login failed, reconnecting, collector]
verified: null
related: [save-the-cluster-password, trust-a-host-key, check-your-nodes, ssh-sudo-and-host-keys, errors-network]
---

# Connection and login messages

This page covers messages about reaching a node, logging in, sudo and host keys. Messages name your real nodes, so yours may show another name or address.

SSH is the secure connection SparkBox uses for everything on a node. An error often appears under a title that names what you were doing, such as "Couldn't restart cell2", or "That didn't work".

When a message has more to say, it has a **Details** button, which shows the node's own words, such as its error output and exit code.

## Reaching a node

### "cell2 isn't connected. SparkBox keeps reconnecting; check that the node is on and on your network."

**What it means.** SparkBox has no connection to the node right now, so it can't do what you asked. It's already retrying.

**What to do.** Check that the sidebar shows the node's GPU use, such as **GPU 28%**, then try again. If it stays **Offline**, see [A node shows Offline](help:troubleshooting#a-node-shows-offline). When the VPN is in use or failing, a **Reconnect VPN** button appears with this message. At home it doesn't show.

### "cell3 isn't connected."

**What it means.** SparkBox has no connection to the node right now, so it can't do what you asked. It's already retrying.

**What to do.** Check that the sidebar shows the node's GPU use, such as **GPU 28%**, then try again. If it stays **Offline**, see [A node shows Offline](help:troubleshooting#a-node-shows-offline).

### "SSH timeout: no reply from 192.168.50.98:22 in 10 s. The node may be off or restarting, or this Mac isn't on its network."

**What it means.** This Mac tried to reach the node and heard nothing back. In a status bar tooltip, this reads **No SSH reply**.

**What to do.**

1. Look at the node. Is it on?
2. If it's on, check its network cable.
3. If you're away from home, [connect the VPN](help:connect-the-vpn).
4. If every node shows this, see [Nothing connects](help:troubleshooting#nothing-connects).

### "Couldn't reach 192.168.50.98:22: Connection refused"

**What it means.** The node answered, but nothing accepted the SSH connection. The status bar reads **SSH refused the connection**. The last part of this message comes from macOS, so its wording can differ. A route or name-lookup failure is a different problem; it does not show that the node answered.

**What to do.** If the node restarted a moment ago, wait a minute: SSH starts late. If it stays this way, someone at the node can run `sudo systemctl enable --now ssh`.

### "Couldn't reach 192.168.50.98:22: No route to host (os error 65)"

**What it means.** This Mac could not open a connection to the node. SSH login had not started. This message does not mean the password was rejected.

**What to do.** Wait for SparkBox's automatic retry, then check the network and the mode shown in **VPN**. If another app reaches the same address and port, inspect SparkBox's access in **System Settings › Privacy & Security › Local Network**. Network-filter logs may also help explain an app-specific failure. Record the time and [collect diagnostics](help:collect-diagnostics) if it repeats. Keep saved credentials and host keys unless a separate login or identity error calls for review.

### The connection log reports "Bad packet size"

**What it means.** SSH rejected an invalid packet and closed that connection. The number describes rejected connection data, not a model's file size. This message alone does not identify the cause.

**What to do.** Let SparkBox reconnect, then [collect diagnostics](help:collect-diagnostics) if it happens again. Wait for a stable connection before starting new work. Keep packet checks, encryption and host-key checks enabled.

### "SSH handshake with 192.168.50.98:22 timed out"

**What it means.** The node accepted the connection, but the secure setup didn't finish. It may be very busy, or still starting. The message adds "(through the VPN)" when the VPN carried it.

**What to do.** Wait a minute. SparkBox retries by itself. If it goes on, a node that's out of memory can behave this way, so check it at the machine.

### "SSH handshake with 192.168.50.98:22 failed: …"

**What it means.** The secure setup failed. The end of the message gives the reason.

**What to do.** If you reinstalled the node, check its [host key](help:trust-a-host-key). Otherwise, retry, then [collect diagnostics](help:collect-diagnostics).

### "192.168.50.98:22 didn't answer through the VPN within 10 s"

**What it means.** The VPN is carrying the connection, and the node didn't reply through it.

**What to do.** Open the [VPN](open:vpn) window. If the VPN shows **Connected**, check that the node is on. See [VPN, fabric and switch messages](help:errors-network).

### "Connection closed; reconnecting…"

**What it means.** The link to the node dropped. SparkBox is connecting again by itself. You may also see:

- "No reply from cell2 in 10 s; reconnecting…"
- "The network path changed; reconnecting…"

**What to do.** Nothing, unless it keeps happening. `~/Library/Logs/SparkBox/ssh.log` records each drop.

### "SSH channels busy on cell2"

**What it means.** SparkBox allows only a few commands at a time over one connection, and they're all in use. The node isn't lost.

**What to do.** Wait for the busy work to finish, then try again.

### "`docker ps` took longer than 30s"

**What it means.** A command ran past its time limit, so SparkBox stopped waiting. The message names the command and the limit.

**What to do.** Try again. If it happens often, the node may be overloaded. Check its memory and load in its [window](help:node-window).

### "The collector on cell2 stopped 3 times in 5 minutes (…); SparkBox tries again in a minute."

**What it means.** The collector is the small read-only helper that sends each node's readings. It keeps stopping. You may also see:

- "The collector on cell2 stalled 3 times in 5 minutes; reconnecting…", which is the same problem
- "cell2 sent a collector frame SparkBox couldn't read", which means its reading was damaged

**What to do.** Wait for the retry. If it goes on, [collect diagnostics](help:collect-diagnostics). Readings from that node stay frozen meanwhile.

### "cell3 is on your LAN twice."

**What it means.** Advice, not a fault. The node's Wi-Fi and its cable are on the same network, so replies can leave over Wi-Fi, which is slower and less steady. A gray ⓘ marks it in the node's **Network** tab. SparkBox doesn't change this.

**What to do.** If the node doesn't need Wi-Fi, run `nmcli radio wifi off` on it, in **Terminal**. Do this only while its cable is in. If you lose the connection, someone at the node can run `nmcli radio wifi on` to undo it.

### "Interrupted: the connection was reset (the login settings changed)"

**What it means.** The connection closed while something was running. You'll see this after you change login settings or a node's settings, or if a node is removed. The end of the message gives the reason.

**What to do.** Run it again. Long jobs such as downloads keep going by themselves on the node.

## Logging in

### "cell2 didn't accept the password. Check the cluster password in Settings."

**What it means.** The node refused the saved password. The full message reads "The password for sparkuser\@192.168.50.154:22 was rejected. Check it in Settings." The status bar reads **Login failed**.

**What to do.** Click **Open Settings**, then [save the cluster password](help:save-the-cluster-password) again. All your nodes share this password. If only one node refuses it, that node's password may have been changed.

### "Save the cluster password first; SparkBox uses it for SSH and sudo."

**What it means.** No cluster password is saved yet, so SparkBox can't log in, or can't run a command as root. "Save the cluster password in Settings first." means the same.

**What to do.** Click **Open Settings**, then follow [Save the cluster password](help:save-the-cluster-password).

### "No SSH key was found and no cluster password is saved. Save the cluster password, or set up key login, in Settings › Cluster login."

**What it means.** SparkBox has no way to log in.

**What to do.** Follow [Save the cluster password](help:save-the-cluster-password) or [Set up key login](help:set-up-key-login).

### "sparkuser\@192.168.50.154:22 didn't accept ~/.ssh/id_ed25519 or the password. Check Settings › Cluster login."

**What it means.** SparkBox tried your SSH keys and then the password, and the node refused them all. "sparkuser\@192.168.50.154:22 didn't accept ~/.ssh/id_ed25519. Add the key to the node's authorized_keys or save the cluster password in Settings." is the same, with no password to try.

**What to do.** Check the user name in [Cluster login](open:settings?section=login), then save the password again.

### "The connection to 192.168.50.154:22 closed during the login."

**What it means.** The connection dropped during login. Your password wasn't rejected. "192.168.50.154:22 ended the login: …" gives the node's own reason.

**What to do.** Try again. If the node says "Too many authentication failures", SparkBox offered too many keys. Turn off the extra ways to log in, in [Settings: Cluster login](help:settings-cluster-login).

## sudo

sudo runs a command as root, the account that can change anything on a node. It uses the cluster password.

### "sudo on cell2 didn't accept the saved cluster password."

**What it means.** The password worked for logging in, or SparkBox used a key, but sudo refused it. The Live form is "The saved cluster password was rejected by sudo on cell2. Save the correct password in Settings."

**What to do.** Click **Fix password**, then save the right password. Until you do, sudo stays off for that node, so that repeated tries don't lock the account.

### "The saved cluster password was rejected by sudo on cell1 and cell2, so SparkBox has stopped using it everywhere. Save the correct password in Settings."

**What it means.** Two or more nodes refused the password, so SparkBox stopped trying it on any node. This protects your account.

**What to do.** [Save the cluster password](help:save-the-cluster-password) again, with the right password.

### "The cluster user can't use sudo on cell2."

**What it means.** User `sparkuser` isn't allowed to use sudo on that node.

**What to do.** Someone with root on the node needs to add the user to the `sudo` group. Then run the checks in [Check that your nodes are ready](help:check-your-nodes).

## Host keys

A **host key** proves that a node is the same node as last time. SparkBox asks you to trust it once.

### "SparkBox hasn't connected to 192.168.50.154:22 before. Its host key is SHA256:…. Compare it with the node (ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub) and trust it in Connect or Settings › Nodes."

**What it means.** This is a first connection, so SparkBox doesn't know the node's key. The status bar reads **Host key not trusted yet**, and the node shows **Error**.

**What to do.** Follow [Trust a node's host key](help:trust-a-host-key).

### "The host key for 192.168.50.154:22 changed (was SHA256:…, now SHA256:…). If you reinstalled the node, use Settings › Diagnostics › Forget host keys; otherwise something may be intercepting the connection."

**What it means.** The node showed a different key from the one you trusted. This happens after a reinstall. If you didn't reinstall it, another device may be answering at that address.

**What to do.** Don't trust the new key until you know why it changed. If you reinstalled the node, follow [Trust a node's host key](help:trust-a-host-key). If you didn't, compare the key at the machine first.

### "cell3's host key changed. Type cell3 to trust the new key."

**What it means.** SparkBox asks you to type the node's name, so that a changed key can't be accepted by accident.

**What to do.** Type the name, exactly, in the dialog. Do this only if you know why the key changed.

### "cell1 and cell2 present the same key (a cloned image). Trusting it for cell3 is your choice."

**What it means.** Several nodes show one host key. That happens when they were made from one disk image. It's true of `cell1`, `cell2` and `cell3`, so the key alone can't tell them apart.

**What to do.** It's safe to trust if you recognize the node. SparkBox uses a separate [identity pin](help:leases-and-identity) to tell your nodes apart.

### "known_hosts.json couldn't be read; the original is kept at …; use Settings › Diagnostics › Forget host keys to start over."

**What it means.** The file of keys you trusted is damaged. SparkBox kept a copy.

**What to do.** Open [Diagnostics](open:settings?section=diagnostics) and use **Forget host keys…**. You then trust each node again. See [Trust a node's host key](help:trust-a-host-key).

### "That doesn't look like an SSH host key fingerprint (SHA256:…)."

**What it means.** The key text you gave isn't in the expected form.

**What to do.** Copy the full fingerprint, starting with `SHA256:`.

## The check of a node

These appear in **Connect my Sparks** and in **Test** in **Settings › Nodes**. See [Check that your nodes are ready](help:check-your-nodes).

### "Everything SparkBox needs works on this node."

**What it means.** Every check passed.

**What to do.** Nothing.

### "Works, with 1 thing to look at."

**What it means.** The node works, and one check has advice for you.

**What to do.** Read the row with the warning. It says what to do.

### "Not finished: trust the host key to run the other 3 checks."

**What it means.** The checks stopped at the host key, because SparkBox won't log in to a node you haven't trusted.

**What to do.** Click **Trust…** on that row after you compare the key. The checks can then go on.

### "2 checks failed."

**What it means.** Two checks failed. The message adds how many others couldn't run as a result.

**What to do.** Each failed row says what's wrong and what to do. Fix the first one, then run the check again.

### "cell2-example.local didn't resolve within 4 s"

**What it means.** SparkBox couldn't turn the node's name into an address. The hint reads "Enter the node's IP address: .local names don't resolve over the VPN."

**What to do.** Fill in the node's address. See [Add or change a node](help:add-or-change-a-node).

### "No reply from 192.168.50.154 in 5 s"

**What it means.** The node didn't answer on its SSH port. The hint reads "Away from home? Import the VPN profile first. At home, check the cable and the IP."

**What to do.** Do what the hint says. Then check that the node is on.

### "Not checked: SparkBox is in look-only mode"

**What it means.** The sudo check changes nothing, but look-only mode stops anything that runs as root. The hint reads "The sudo check runs in your own session: look-only off and Admin unlocked."

**What to do.** [Turn off look-only mode](help:use-look-only-mode), unlock Admin mode, and run the check again.

### "Not checked: no open fence allows sudo checks on cell2"

**What it means.** This message appears only in the test build, which needs a Fence for this check. The SparkBox app never shows it.

**What to do.** In the test build, follow [The Fence](help:the-fence). In the SparkBox app, there's nothing to do.

### "No cluster password saved"

**What it means.** The sudo check has no password to use. The hint reads "Services, power and updates need sudo: save the password in Settings › Cluster login."

**What to do.** Follow [Save the cluster password](help:save-the-cluster-password).

### "Checked once the node is saved"

**What it means.** The sudo check runs only on saved nodes. The hint reads "Sudo is checked on saved nodes only; test the node again from Settings › Nodes."

**What to do.** Save the node, then click **Test** in [Nodes settings](open:settings?section=nodes).

### "Use the same password for SSH and sudo, or add the user to the sudo group."

**What it means.** This hint follows a failed sudo check. Your account on a node has one password, used for both logging in and sudo. So one of two things is wrong. The saved cluster password may not be that account's password, which can happen when SparkBox logs in with an SSH key, because the login then doesn't test the password. Or the account may not be in the `sudo` group, the list of users allowed to use sudo.

**What to do.**

1. Follow [Save the cluster password](help:save-the-cluster-password), with the password for that account.
2. Run the check again.
3. If it still fails, see ["The cluster user can't use sudo on cell2."](#the-cluster-user-cant-use-sudo-on-cell2)

### "Installed, but this user can't use it"

**What it means.** Docker is on the node, but your user isn't allowed to use it. The hint reads "Run sudo usermod -aG docker sparkuser and log in again."

**What to do.** Follow [Fix Docker access](help:prepare-the-cluster#fix-docker-access). The fix lets your account do what root can on that node, and it says how to undo that. SparkBox's own connection needs a new login too, so quit SparkBox and open it again before you load a model.

### "Docker isn't available"

**What it means.** Docker didn't answer on this node, so models can't load there. This usually means its service has stopped, not that Docker has gone. The hint reads "Don't reinstall it. Check the docker service in Services. See Help: Docker isn't available."

**What to do.** Don't reinstall Docker, and don't run `apt` for it. Docker is part of the GPU stack, which must match across your nodes, and a reinstall can move it to a newer version on this node alone. That can stop any model on the node, and it may have no way back.

1. In the sidebar, under **Workloads**, click **Services**. [Open Services](open:services)
2. In **Filter services**, type `docker`.
3. Click this node's cell in the `docker` row. Read **State** and **Error**, then read **Journal**. They say why it stopped.
4. If `docker` has stopped or failed, follow [Restart, disable or enable a service](help:restart-a-service). A restart of Docker has run only on test systems. It stops every container on the node, including any model it serves, and the dialog lists them. Containers with a restart policy come back by themselves. vLLM containers stay stopped, so load those models again.
5. Run the check again. A checkmark beside **Docker** means you're done.

If the node has no `docker` row, or the service fails again, stop here. [Run the Doctor report](help:collect-diagnostics) and keep it, because it records what's wrong. Then ask NVIDIA or your Spark's maker for help.

### "nvidia-smi didn't answer"

**What it means.** The tool that talks to the GPU didn't work, so the driver may be broken. The hint reads "Don't load a model on this node. SparkBox can't repair a driver. See Help: If the GPU driver doesn't answer."

**What to do.** Don't load a model on this node until it's fixed. SparkBox can't repair a driver, and installing NVIDIA packages by hand can leave a node unable to start. Follow [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering).

### "No address on 192.168.100.x or 192.168.101.x"

**What it means.** The node's fabric port has no address. "Only", followed by one address, means one of its two halves has none. The hint names the two interfaces and their ranges. A node with no fabric address can't join a load across nodes or take part in copies. It can still serve a one-node load.

**What to do.** SparkBox can't set a fabric address. Follow [Network settings SparkBox can't change](help:errors-network#network-settings-sparkbox-cant-change).

### "The connection to cell2 dropped during this check. Run it again."

**What it means.** The link dropped in the middle of the check, so its results aren't complete. Later rows read "Not checked: the connection dropped. Run the check again."

**What to do.** Run it again. If it repeats, `~/Library/Logs/SparkBox/ssh.log` says when and why it dropped.

### "Off on enP7s7 (Wake-on: d)"

**What it means.** Wake-on-LAN is off on that port, so **Wake** won't reach this node. The hint gives one command and one file edit that turn it on and keep it on after a restart.

**What to do.** SparkBox doesn't need Wake-on-LAN, so you can leave this. To use **Wake**, follow [Turn on Wake-on-LAN](help:check-your-nodes#turn-on-wake-on-lan).

## Status words

The status bar and sidebar use short words for the same problems. [What the status words mean](help:status-words) lists them all.
