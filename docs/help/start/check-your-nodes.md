---
id: check-your-nodes
title: Check that your nodes are ready
type: howto
area: start
summary: "Run the connection checks on every node: SSH, host key, login, sudo, Python, Docker, the GPU driver and the fabric."
keywords: [check, probe, sudo check, test connection, wake-on-lan, turn on wake-on-lan, ping, ping a node, reachable, is my node up]
verified: false
related: [connect-my-sparks, trust-a-host-key, errors-connection, settings-nodes, get-a-gpu-driver-answering]
---

# Check that your nodes are ready

To find out whether SparkBox can use your nodes, run its connection checks.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the answers come from the simulator and tell you nothing about your real nodes. [Switch between Demo and Live](help:switch-between-demo-and-live) shows how.
- [Save the cluster password](help:save-the-cluster-password) first.
- The checks use your saved password, so SparkBox asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes.
- The checks change nothing on your nodes. The sudo check runs `sudo true`, a command that does nothing, to prove the password works. Each check is written to the activity log.

## Steps

### Check every node

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Nodes settings](open:settings?section=nodes)
2. In the list on the left, click **Nodes**.
3. Under the table, click **Connect my Sparks…**. The wizard opens on its **Account** step.
4. In the list of steps on the left, click **Checks**.
5. Click **Check all nodes**. If you've checked before in this window, the button reads **Check again**.
6. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
7. Wait while each node shows **Checking…**. All the nodes run at the same time, and each one fills in its own list.
8. Read the line beside the button. "Nothing failed" means every node passed.

### Check one node

1. In **Settings › Nodes**, find the node's row and click **Test**. macOS asks you to confirm with Touch ID or your Mac password. Confirm it.
2. Read the results in the **Test of cell2** group, under the table.
3. Click **Done**.

**Test** uses the values in the table, even if you haven't saved them. That makes it a safe way to try a new address.

## What you should see

Each node shows 11 checks. Here's what each one proves, and the words you see when it passes:

| Check | What it proves | When it passes |
|---|---|---|
| **Find the node** | SparkBox knows where the node is | "Uses its address, 192.168.50.51" |
| **Reach SSH** | The node answers on the network | "192.168.50.51:22 answers" |
| **Host key** | It's the machine you trusted | "Matches the key SparkBox trusts" |
| **Log in** | The account works | "Logged in as sparkuser" |
| **Login method** | How it logged in | "Password (your shared cluster password, from the Keychain)", or the SSH key it used |
| **sudo** | SparkBox can run commands as root | "Works with the cluster password" |
| **Python 3** | The read-only helper can run | The version, such as "Python 3.12.3" |
| **Docker** | Models can run in containers | The version, such as "Docker 29.6.2" |
| **GPU driver** | The GPU answers | The GPU name and driver version |
| **Fabric addresses** | Both halves of the fast network have addresses | Two addresses, one on `192.168.100.x` and one on `192.168.101.x` |
| **Wake-on-LAN** | The node can be woken over the network | See the note below |

In the wizard, a checkmark means passed, a warning sign needs a look, a cross failed, and a dashed circle wasn't checked. In **Settings › Nodes**, each row shows a colored dot with a word.

**Wake-on-LAN** isn't something SparkBox needs. On your nodes it normally reads "Not readable without sudo. NetworkManager leaves it to the driver default." That's not a failure, but it doesn't say whether the setting is on. To use **Wake**, see [Turn on Wake-on-LAN](#turn-on-wake-on-lan).

**Test** ends with one summary line. "Everything SparkBox needs works on this node." means all is well. "Works, with 1 thing to look at." means a warning needs reading, but SparkBox can use the node.

## Why it works

Each check depends on the one before it. If SparkBox can't reach a node, it can't test the login, so the later checks wait and say why. These checks make their own connection, so they work even if a node isn't online. They never trust a host key by themselves, because that's a separate step. Run these checks on your own configured nodes.

## If something goes wrong

Start with the first check that didn't pass. Messages use `cell2` and its address as examples.

Where you run a fix depends on how far the checks got:

- **Before **Log in** has a checkmark,** SparkBox can't reach the node, so you do the fix at the machine. [Run a command at the node](help:trust-a-host-key#run-a-command-at-the-node) shows how.
- **Once the node is online,** you can stay at this Mac. [Open a terminal on a node](help:open-a-terminal) on that node, or use [Fleet Command](help:run-a-command-on-every-node).

A fix that starts with `sudo` asks for the cluster password, and nothing appears as you type it. [Run a command with sudo](help:open-a-terminal#run-a-command-with-sudo) shows what to expect.

**Find the node**

- **"cell2-example.local didn't resolve".** Your Mac can't find that name. Add the node's address in **Settings › Nodes**. Names ending in `.local` don't work through the VPN.

**Reach SSH**

- **"No reply from 192.168.50.154:22 in 5 s".** Nothing answered. At home, check that the node is on, then check the cable and the address. Away from home, [connect the VPN](help:connect-the-vpn).
- **A message with "Connection refused".** The node is on, but SSH isn't running. [Go to the node](help:trust-a-host-key#run-a-command-at-the-node) and run `sudo systemctl enable --now ssh`.
- **"The connection to cell2 dropped during this check. Run it again."** The node went away partway. Run the check again. If it keeps happening, the reason is in `~/Library/Logs/SparkBox/`.

**Host key**

- **"Not trusted yet (first connection)" and "Not finished: trust the host key to run the other 8 checks."** This is normal the first time. Follow [Trust a node's host key](help:trust-a-host-key).
- **A message that says the key changed.** Don't trust it unless you reinstalled the node. Follow [Trust a node's host key](help:trust-a-host-key#if-a-nodes-key-changed).

**Log in**

- **"The password for `sparkuser@192.168.50.154` was rejected. Check it in Settings."** [Save the cluster password](help:save-the-cluster-password).
- **"No SSH key was found and no cluster password is saved."** Save the password, or [set up key login](help:set-up-key-login).

**sudo**

- **"No cluster password saved".** SparkBox logged in with a key, but sudo always needs the password. [Save it](help:save-the-cluster-password).
- **"Checked once the node is saved".** This node isn't in your saved list yet. Save it, then test it again.
- **"Not checked: SparkBox is in look-only mode".** [Turn look-only mode off](help:use-look-only-mode), then check again.
- **A message that names a fence.** That comes from the test build, never from the SparkBox app. See [The Fence](help:the-fence).

**Python 3, Docker, GPU driver**

- **"python3 isn't installed".** Without Python, the node can't come online in SparkBox. [Go to the node](help:trust-a-host-key#run-a-command-at-the-node) and run `sudo apt install -y python3`.
- **"Installed, but this user can't use it".** The fix lets your account do what root can on that node, which SparkBox needs to run models. [Open a terminal on a node](help:open-a-terminal) on the node and run `sudo usermod -aG docker sparkuser`. Then click **Check again**. The check logs in afresh, so it sees the change. SparkBox's own connection to the node doesn't, so quit SparkBox with <kbd>⌘Q</kbd> and open it again before you load a model. To undo the change, run `sudo gpasswd -d sparkuser docker` the same way.
- **"Docker isn't available".** Docker didn't answer on this node, so models can't load there. Don't reinstall it. Follow [Docker isn't available](help:errors-connection#docker-isnt-available), which starts in **Services**.
- **"nvidia-smi didn't answer".** The node's GPU driver didn't respond, so don't load a model on this node. SparkBox has no way to repair a driver. Follow [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering).

**Fabric addresses**

- **"No address on 192.168.100.x or 192.168.101.x", or "Only" and one address.** The fast network isn't set up on this node. Several-node loads need both halves. See [Check the fabric](help:check-the-fabric).

**Wake-on-LAN**

- **"Off on enP7s7 (Wake-on: d)", or "Off on enP7s7 (NetworkManager)".** SparkBox doesn't need Wake-on-LAN, so you can leave this. To use **Wake**, follow [Turn on Wake-on-LAN](#turn-on-wake-on-lan).
- **"enP7s7 can't wake on a magic packet".** This node's network hardware can't do it, so **Wake** won't work here. Use the node's power button.

[Connection and login messages](help:errors-connection) lists the rest.

### Turn on Wake-on-LAN

You need this only to use **Wake**. When the setting is off, a hint under the **Wake-on-LAN** row gives the fix: one command to run on the node, and one line to add to its netplan file, so that the setting survives a restart. A **netplan file** holds a node's network settings. This fix hasn't been tried on your nodes.

> [!DANGER]
> **A wrong setting in a netplan file can cut a node off.** The mistake may not show until the next restart, and you may then have to fix it at the machine itself. If you're not sure, leave this setting alone, because SparkBox doesn't need it.

1. [Open a terminal on a node](help:open-a-terminal) on that node.
2. Run `sudo ethtool enP7s7`, and find the line that starts with `Wake-on:`. If it contains `g`, the setting is on, so stop here. If it reads `d`, it's off. If the hint names something other than `enP7s7`, use that name in these commands.
3. Run `sudo ethtool -s enP7s7 wol g`. This turns the setting on until the node next restarts.
4. Follow [Edit a file on a node](help:edit-a-file), and read the warning at its start. Open the file in `/etc/netplan` that lists `enP7s7`. If no file lists it, stop here: the setting stays on until the next restart, and you can repeat step 3 after each one.
5. Under `enP7s7`, add `wakeonlan: true`, with the same indent as the lines beside it, then save the file. The node reads this file when it next starts, and the result looks like this:

   ```yaml
   ethernets:
     enP7s7:
       dhcp4: true
       optional: true
       wakeonlan: true
   ```

6. Run `sudo ethtool enP7s7` again. The `Wake-on:` line now contains `g`.

To undo this, remove the line from the file, then run `sudo ethtool -s enP7s7 wol d`.

### If the GPU driver doesn't answer

Follow [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering).

## Next steps

- [Trust a node's host key](help:trust-a-host-key)
- [Read the Dashboard](help:read-the-dashboard)
- [Run your first real model](help:run-your-first-real-model)
