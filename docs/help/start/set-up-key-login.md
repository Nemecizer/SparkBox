---
id: set-up-key-login
title: Set up key login
type: howto
area: safety
summary: Log in to your nodes with an SSH key instead of the password, and optionally turn password login off.
keywords: [ssh key, 1password, secretive, ssh agent, passwordless]
verified: false
related: [settings-cluster-login, ssh-sudo-and-host-keys, keychain-and-secrets]
---

# Set up key login

To log in to your nodes with an SSH key instead of the shared password, let SparkBox add your key to them.

## Before you start

- This is optional. The shared password is a normal way to log in, and you can keep using it.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. Your nodes must be online. In Demo, this only changes the simulator.
- If SparkBox creates a new key for you, it also needs Touch ID or your Mac password.
- Your key stays on this Mac. Only its public half goes to the nodes.
- sudo still uses the cluster password, so keep it saved. SparkBox needs it for service changes, updates and restarts.

An **SSH key** is a pair of files. The private file stays on this Mac, and the public file, ending in `.pub`, goes on each node. A node lets you in if you can prove you hold the private file.

## Steps

### Add your key

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Cluster login settings](open:settings?section=login)
2. In the list on the left, click **Cluster login**.
3. Under the **Each node** group, click **Set up key login…**. A **Set up key login** group appears below it.
4. In **Key**, check the path. The default is `~/.ssh/id_ed25519`. If you already have a key there, leave it.
5. If you don't have a key, select **Create it (ed25519) if it doesn't exist**. SparkBox creates one with no passphrase, so anyone who can read that file can use it. It stays in your Mac account's `~/.ssh` folder.
6. Under **Nodes**, select the nodes you want. Nodes that log in with the password are selected for you.
7. Click **Set up on 4 nodes**. The button shows how many you selected. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
8. Read the result for each node, under the form. "Key login works" means SparkBox added the key and logged in with it.

### Turn password login off

This step is separate, and most people don't need it. It stops your nodes accepting SSH passwords at all, from any computer.

> [!DANGER]
> **If you lose your key with password login off, you can't log in over SSH.** The only way back is a keyboard and screen plugged into the node. Keep a backup of the key in a password manager or on an encrypted disk, never in a plain folder. sudo still uses the password.

1. Follow **Add your key** above, and check that it says "Key login works".
2. In the **Set up key login** group, select **Then turn it off where the key works**. A caution appears.
3. Click **Set up on 4 nodes**.
4. If you chose all your nodes, a dialog named **Turn password login off on all 4 nodes?** opens. Type `4 nodes` in the box, then click **Turn it off**.
5. Read the result for each node. "Key login works; password login is off" means it's done.

SparkBox turns password login off only where the key works, and it confirms this with the node's SSH service.

### Turn password login back on

Try one node first, so that a mistake can't reach them all. You need Admin mode, and your key must still work.

1. In the sidebar, under **Tools**, click **Fleet Command**. [Open Fleet Command](open:fleet) The window opens on its **Run** view.
2. In the row of nodes after **on**, click the other nodes to leave one selected, such as `cell2`.
3. Click the command box, and type this command:

   ```
   rm /etc/ssh/sshd_config.d/00-sparkbox-keys-only.conf && systemctl reload ssh
   ```

4. Select **as root**. The `$` at the start of the box changes to `#`. The badge beside the box reads **cuts access**, because this command reloads the service you log in through. That's expected here.
5. Click **Run**. A red dialog opens and shows the command. Type the node's name, such as `cell2`, then click **Run** in the dialog. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
6. Read the output. No error means the file is gone and the SSH service has reloaded.
7. To check, replace the command with `sshd -T | grep -i passwordauthentication`. Select **as root** again, because it turns itself off after each run. Click **Run**, then click **Run** in the dialog. The node prints `passwordauthentication yes`. That's the node's SSH service saying it accepts passwords again.
8. Repeat from step 2 for each other node.

If you've lost your key, SparkBox can't reach the node. [Go to the node](help:trust-a-host-key#run-a-command-at-the-node) and run this command there:

```
sudo rm /etc/ssh/sshd_config.d/00-sparkbox-keys-only.conf && sudo systemctl reload ssh
```

## What you should see

In **Settings › Cluster login**, the **Each node** group shows **SSH key** for each node, with the key's path. The line under the group changes to "Every node logs in with an SSH key; sudo still uses the cluster password."

The **Key file** box now shows your key, unless it already held another. SparkBox fills it in so that it keeps trying this key.

## Why it works

SparkBox adds your public key to `~/.ssh/authorized_keys` on each node, once, and leaves anything else in that file alone. It then logs in using that key and nothing else, and a node counts as done only if that works. To turn password login off, SparkBox writes one small settings file on the node, `/etc/ssh/sshd_config.d/00-sparkbox-keys-only.conf`. It checks the result with `sshd -t` and `sshd -T`, and removes the file if either check fails. Only then does it reload the SSH service.

## If something goes wrong

- **"Couldn't read ~/.ssh/id_ed25519.pub".** There's no public key at that path. Select **Create it (ed25519) if it doesn't exist**, or correct the path in **Key**.
- **"The key was added, but logging in with it failed".** The key is on the node, but the login didn't work. The message gives the reason. Your password login still works.
- **"sshd rejected the change; nothing was changed".** The node refused the new setting. Password login stays on.
- **"sshd still allows passwords (another setting wins); nothing was changed".** Another settings file on the node takes priority. Password login stays on.
- **"Pick at least one node."** No node is selected.
- **A node isn't connected.** SparkBox adds the key over the node's own connection, so the node must be online.

## Next steps

- [Settings: Cluster login](help:settings-cluster-login)
- [SSH, sudo and host keys](help:ssh-sudo-and-host-keys)
- [Where your passwords and keys go](help:keychain-and-secrets)
