---
id: ssh-sudo-and-host-keys
title: SSH, sudo and host keys
type: concept
area: safety
summary: How SparkBox logs in to a node, runs commands as root, and makes sure it's talking to the right machine.
keywords: [ssh, sudo, root, fingerprint, login]
verified: null
related: [trust-a-host-key, save-the-cluster-password, set-up-key-login, leases-and-identity]
---

# SSH, sudo and host keys

## In short

SparkBox logs in to each node over **SSH**, a secure connection between computers. When a job needs full control of a node, it uses **sudo** to run that job as **root**, the account with full control. A **host key** proves that the machine answering is the one you trusted.

## How it works

### SSH: logging in to a node

SSH gives SparkBox a private, encrypted connection to a node, over your LAN or through the VPN. Like any login, it needs an account and a proof that you own it. Enter your own node account during setup. `sparkuser` in Help is a synthetic example.

SparkBox tries three proofs, in this order, and stops at the first that works:

- First, keys held by an **SSH agent**, a program that keeps your keys and signs for you. The agent might be macOS's own, 1Password or Secretive.
- Next, a key file on this Mac, chosen in **Settings › Cluster login** or named in `~/.ssh/config`.
- Last, the **cluster password**, the one password your nodes share, kept in the Keychain.

An **SSH key** is a pair of matching files. The private one stays on this Mac, and the public one goes on the node. Key login is optional. Logging in with the shared password is a normal choice, and SparkBox treats it as one. [Set up key login](help:set-up-key-login) shows how to switch.

### sudo: running a command as root

Your account can do everyday work on a node. Some jobs need more, such as restarting a service or installing an update. Those need **root**, the account that can do anything on a node. **sudo** is the standard tool that runs one command as root, after it checks your password.

SparkBox always gives sudo the cluster password, even if you log in with a key. It gives the password only when sudo itself asks, and only through sudo's own prompt, over the encrypted connection. The password never appears on a command line, where other programs could see it.

SparkBox also protects you from a wrong password. If sudo rejects the saved password on a node, SparkBox stops trying it there. If two nodes reject it, SparkBox stops using it everywhere, until you save a new one. This keeps a wrong password from locking your account.

### Host keys

Every node has its own **host key**, a long number that identifies it. When SparkBox connects, the node proves it holds that key. SparkBox shows a short form called a fingerprint, such as `SHA256:example…`.

This protects your password. If another machine pretended to be your node, SparkBox would see the wrong key and refuse to connect, before it sent anything.

The first time SparkBox meets a node, it can't know the key is right. So it stops and asks you. You compare the fingerprint with the one on the node and click **Trust…**. SparkBox then records that key for that address.

From then on:

- If the key matches, SparkBox connects without asking.
- If the key changes, SparkBox refuses and warns you. That's expected after you reinstall a node. If you didn't, something may be intercepting the connection.

If you already trusted a node with your Mac's own `ssh` command, SparkBox accepts that exact key and records it without asking. That's the same decision, made earlier.

A recorded key belongs to an address and a port. If a node gets a new address, SparkBox asks you to trust it again.

### An example

Say you reinstall DGX OS on `gx10-0000`. The installer makes a new host key. The next time SparkBox connects, the key doesn't match, so the node shows **Error** and SparkBox offers **Review host key…**. You check that the new fingerprint matches the node, type the node's name and confirm. SparkBox records the new key and reconnects. Had you not reinstalled anything, that same warning would be your signal to stop.

### Your nodes' shared host key

If nodes were installed from a cloned image, they may present the same host key. The Demo deliberately includes this synthetic scenario. It is not an assumption about your equipment.

- **Compare each device independently.** A matching shared key cannot distinguish the machines that hold it. Verify the intended address and fingerprint at each node's trusted console before trusting it.
- **SparkBox shows shared keys.** Review the Trust dialog, **Settings › Nodes** and the node's **System** tab. Trust is recorded separately for each address and port.
- **Identity checks remain required.** The [identity pin](help:leases-and-identity#identity-pin) adds checks before changes. It does not make a shared SSH key unique or replace independent verification.
- **Use unique host keys for distinct machines.** Regenerating a node's host keys is a separate administrative change. Verify any new fingerprint before updating trust; SparkBox does not rotate keys automatically.

## Why it matters in SparkBox

You meet these ideas when you connect your cluster. The checks in [Connect my Sparks](help:connect-my-sparks) test SSH, the host key, the login and sudo on every node. [Check that your nodes are ready](help:check-your-nodes) explains each result.

After that, SSH works unseen. You'll think about it again in three cases:

- A node shows **Error** with "Host key not trusted yet" or "Host key changed". Follow [Trust a node's host key](help:trust-a-host-key).
- A node shows "Login failed". Check the [cluster password](help:save-the-cluster-password).
- You change the password on your nodes. Save the new one in SparkBox too.

## Related

- [Trust a node's host key](help:trust-a-host-key)
- [Save the cluster password](help:save-the-cluster-password)
- [Set up key login](help:set-up-key-login)
- [Controller lease and identity pin](help:leases-and-identity)
- [Connection and login messages](help:errors-connection)
