---
id: settings-cluster-login
title: "Settings: Cluster login"
type: feature
area: settings
summary: "The account SparkBox uses on every node and how it logs in: SSH agent, key file or password."
keywords: [password, ssh, login, agent, key file]
verified: false
related: [save-the-cluster-password, set-up-key-login, ssh-sudo-and-host-keys, keychain-and-secrets]
---

# Settings: Cluster login

## What it's for

This section sets the account SparkBox uses on your nodes, and how it proves who it is. You need it once, to save the cluster password. Come back if your password changes, or if you want to log in with an SSH key.

## Where to find it

- **Settings:** press <kbd>⌘,</kbd>, then click **Cluster login**.
- **Command palette:** press <kbd>⌘K</kbd> and type `cluster login`.
- **Node menu:** when a node shows "Login failed", its menu offers **Open Cluster login**.
- **Link:** [Open Cluster login settings](open:settings?section=login)


## What's on the screen

### Account on every node

All your Sparks share one account, as NVIDIA's setup creates them.

- **User** is the account's name, `sparkuser`. Changing it changes it for every node. If nodes have different users, a hint says saving sets this one on all of them.
- **Password** is the cluster password. It's kept only in the macOS Keychain. **Save** stores it, and **Remove** deletes it. The box shows "Saved in Keychain" or "Not saved", and never the password.

SSH uses this password when no key is accepted. sudo always uses it, even when you log in with a key. It gets the password on its standard input, after its own prompt, and never on a command line.

### How SparkBox logs in

This group sets the order of proofs SparkBox tries: keys from an SSH agent, then key files, then the cluster password. A **Save** button with **Revert** appears when you change something. Nodes reconnect when you save.

### SSH agent

An **SSH agent** is a program that holds your keys and signs logins for you.

| Choice | What SparkBox uses |
|---|---|
| **System** | The agent your Mac's Terminal uses. This is the default |
| **1Password** | 1Password's SSH agent |
| **Secretive** | The Secretive app's agent |
| **Off** | No agent. Key files and the password only |

### Key file

The path of a private SSH key on this Mac, such as `~/.ssh/id_ed25519`. SparkBox tries it after the agent's keys. The file must exist when you save: otherwise SparkBox says "There's no key file at" that path.

### Key passphrase

A **passphrase** is a password that protects a key file. Most keys don't have one. If yours does, save the key file first. Then type the passphrase here and click **Save**. It goes to the Keychain.

### ~/.ssh/config

**Use it for host names, ports, users, keys and jump hosts** is on by default. This file is where your Mac's own `ssh` command keeps its settings. SparkBox follows these lines from it: `HostName`, `User`, `Port`, `IdentityFile`, `IdentitiesOnly` and `ProxyJump` (one hop). A **jump host** is a machine you pass through to reach another. `Match` blocks are ignored.

A `Host` block written for a node wins over that node's row in **Settings › Nodes**, as it does when you type `ssh` in Terminal.

### Password

**Fall back to the cluster password when no key is accepted** is on by default. Turn it off, and a node that accepts none of your keys can't connect. sudo uses the password either way. The checks in [Connect my Sparks](help:connect-my-sparks) and **Test** obey this box too, so a check fails where a real connection would. The check's sudo step still uses the password.

### Each node

One row for each node, refreshed every 5 seconds. It shows **SSH key (agent)**, **SSH key** or **Password**, with the details beside it. A node that hasn't connected shows what SparkBox will try. A node reached through a jump host says so.

Logging in with the shared password is a normal choice, so a plain lock marks it, not a warning. The line under the group says which applies to you, and has a **Set up key login…** link.

### Set up key login

This group appears when you click **Set up key login…**.

- **Key** is the key to use. The default is `~/.ssh/id_ed25519`. Its `.pub` file goes into each node's `~/.ssh/authorized_keys`.
- **New key** creates an ed25519 key at that path if there isn't one.
- **Nodes** chooses which nodes to set up.
- **Password login** turns SSH password login off where the key works. Read the warning in [Set up key login](help:set-up-key-login#turn-password-login-off) first.
- **Close** hides the group. **Set up on 4 nodes** starts the work.

## What it changes

| What you change | What it needs | What it changes | How to undo |
|---|---|---|---|
| **User** | Save bar | SparkBox's records on this Mac | Type the old name and save |
| **Password** | Touch ID or your Mac password, every time | The Keychain | Save the old value, or click **Remove** |
| **Key passphrase** | Touch ID or your Mac password, every time | The Keychain | Save the old value. This field has no **Remove**: see [Remove every secret](help:keychain-and-secrets#remove-every-secret) |
| **SSH agent**, **Key file**, **~/.ssh/config**, **Password** | Touch ID or your Mac password, unless you confirmed in the last 5 minutes | SparkBox's settings on this Mac. Nodes reconnect | Change it back and save |
| **Set up key login** | Admin mode | Adds your key to each node's `~/.ssh/authorized_keys` | Remove that line from the file |
| **Then turn it off where the key works** | Admin mode, and sudo on the node | Adds a settings file for the node's SSH service | [Turn password login back on](help:set-up-key-login#turn-password-login-back-on) |

Each change is written to the activity log. The password itself never is.

## Related How-Tos

- [Save the cluster password](help:save-the-cluster-password)
- [Set up key login](help:set-up-key-login)
- [Check that your nodes are ready](help:check-your-nodes)
