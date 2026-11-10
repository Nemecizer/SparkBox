---
id: add-or-change-a-node
title: Add or change a node
type: howto
area: start
summary: Change a node's name, address or order in the inventory, or add a new one, and test it before saving.
keywords: [add node, inventory, ip address, hostname]
verified: false
related: [settings-nodes, check-your-nodes, trust-a-host-key]
---

# Add or change a node

To change the list of nodes SparkBox manages, edit the table in **Settings › Nodes**.

## Before you start

- Look-only mode must be off, or the table is read-only.
- Saving is a change on this Mac, so SparkBox asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes.
- Demo and Live share this list. A change you make in Demo applies to Live too.
- For a new node, you need its IP address on your LAN. It must use the same user and password as your other nodes. SparkBox has one account for all of them.
- Editing this list changes only SparkBox's records. It doesn't change anything on the node.

## Steps

### Change a node's details

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Nodes settings](open:settings?section=nodes)
2. In the list on the left, click **Nodes**. The table shows one row for each node.
3. Click the field you want to change, and type. A bar appears at the bottom of the section, with "Unsaved changes". [Settings: Nodes](help:settings-nodes#inventory) explains each field.
4. Click **Test** in that row. SparkBox tries the values you typed, without saving them. Confirm with Touch ID or your Mac password. Read the results under the table, then click **Done**.
5. In the bar, click **Apply & reconnect**. With Demo data, the button reads **Save**.
6. Confirm with Touch ID or your Mac password. The bar goes away, and the word "Saved." appears.

SparkBox reconnects only the nodes whose settings changed. If you change a node's address or SSH port, you need to [trust its host key](help:trust-a-host-key) again. SparkBox keeps each key by address and port.

### Add a node

1. Under the table, click **Add node**. A new row appears at the bottom, with a name such as `spark5`, port `22` and the same **Model** as the row above it.
2. In **Name**, type the name you want to see in SparkBox.
3. In **Hostname**, type the node's name on your network, such as `cell5.local`. If you don't know it, type its IP address here too. This field can't be empty.
4. In **IP address**, type the node's IP address. SparkBox connects to this address when it's filled in.
5. Check **Model**. Choose your node's model from the list, or choose **Other…** and type it.
6. Click **Test**, and confirm with Touch ID or your Mac password. A new node stops at **Host key**, because you haven't trusted it yet.
7. Click **Trust…** and follow [Trust a node's host key](help:trust-a-host-key). The checks run again. The **sudo** check says "Checked once the node is saved". That's normal.
8. Click **Done**.
9. In the bar, click **Apply & reconnect**, and confirm with Touch ID or your Mac password.
10. Click **Test** on the new row again, to run the sudo check.

### Change the order

1. In a node's row, click the up or down arrow.
2. In the bar, click **Apply & reconnect** or **Save**, and confirm.

The order here is the order everywhere: the sidebar, the Dashboard and the charts.

### Remove a node

1. In the node's row, click the trash icon. The row goes, and "Removed gx10-0000. Revert to bring it back." appears under the table.
2. In the bar, click **Apply & reconnect** or **Save**, and confirm.

Removing a node from this list doesn't touch the node, and any models on it keep running. SparkBox only stops watching it. To undo, add it back, or click **Revert** before you save. The last node in the list can't be removed.

## What you should see

The sidebar shows the new name or order right after you save. With Live data, a changed node shows **Connecting**, then its GPU use, such as **GPU 28%**.

## Why it works

SparkBox keeps this list in a file on this Mac, called the **inventory**. **Settings › Cluster login**, **Settings › Nodes** and **Settings › Switches & gateway** all edit the same draft, and one bar saves them together. **Revert** throws away all three sets of edits.

**Test** uses what's in the row, so you can try an address before you commit to it. Nothing is saved until you click **Apply & reconnect** or **Save**.

## Limits

- All nodes share one account. You can't set a different user for one node here. If you need that, use a `Host` block in `~/.ssh/config`, as explained in [Settings: Cluster login](help:settings-cluster-login#sshconfig).
- To change a node's real hostname or IP address, you have to change it on the node or in your gateway. This list only records what they are.

## If something goes wrong

The bar says "1 field needs fixing before saving." and a list under the table says why:

- **"Give the node a name."** The **Name** box is empty.
- **"Another node is already called cell1."** Names must be different.
- **"Enter the node's hostname or IP address."** The **Hostname** box is empty. It can hold an IP address.
- **"“cell 5” isn't a hostname or IPv4 address."** Hostnames can't have spaces.
- **"“999.1.1.1” isn't an IPv4 address (like 192.168.50.11)."** Each of the four numbers must be 255 or lower.
- **"SSH port must be between 1 and 65535."** Type a whole number. The usual value is `22`.
- **"Set the cluster user in Cluster login."** The **User** box in **Settings › Cluster login** is empty.
- **"Couldn't save".** The message gives the reason.
- **The Test results show a failure.** See [Check that your nodes are ready](help:check-your-nodes).

## Next steps

- [Check that your nodes are ready](help:check-your-nodes)
- [Trust a node's host key](help:trust-a-host-key)
- [Settings: Nodes](help:settings-nodes)
