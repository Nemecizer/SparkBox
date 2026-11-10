---
id: settings-nodes
title: "Settings: Nodes"
type: feature
area: settings
summary: "The list of your nodes: name, hostname, IP address, model and SSH port, with a test for each."
keywords: [inventory, hostname, ip fallback, ssh port, wake]
verified: false
related: [add-or-change-a-node, connect-my-sparks, check-your-nodes, trust-a-host-key]
---

# Settings: Nodes

## What it's for

This section holds the **inventory**, the list of nodes SparkBox manages. Use it to change a node's address, add a node, change the order, or test a node's connection.

## Where to find it

- **Settings:** press <kbd>⌘,</kbd>, then click **Nodes**.
- **Command palette:** press <kbd>⌘K</kbd> and type `nodes`.
- **Link:** [Open Nodes settings](open:settings?section=nodes)


## What's on the screen

### Inventory

The table has one row for each node. The order here is the order everywhere: the sidebar, the Dashboard and the charts.

### Name

What SparkBox calls the node, such as `cell1`. Names must be different from each other.

### Hostname

The node's name on your network, such as `cell1-example.local`. SparkBox uses it to connect only when **IP address** is empty. Names ending in `.local` work at home, but not through the VPN. This field can't be empty. It can hold an IP address.

### IP address

The node's numeric address on your LAN, such as `192.168.50.51`. This address comes first: when it's filled in, SparkBox connects to it and doesn't use the hostname. Keep it filled in, so that SparkBox can reach your nodes through the VPN.

### MAC and Wake

Two read-only lines sit under each hostname.

- **MAC** is the hardware address of the node's LAN port, such as "MAC fc:9d:05:9b:e9:14". The node's read-only helper reports it, and **Wake** sends its packet to this address. It reads "MAC not reported yet" until the node has been reached.
- **Wake** says whether the node can be woken over the network: **Wake: On**, **Wake: Off** or **Wake: Unknown · needs sudo**. On your nodes it normally reads **Wake: Unknown · needs sudo**. That doesn't mean off. It means SparkBox can't read the setting without root.

### Model

The hardware, such as **MSI EdgeXpert** or **ASUS Ascent GX10**. Choose from the list, or choose **Other…** and type a name.

### SSH port

The network port where the node listens for SSH. The normal value is `22`.

### Row buttons

From left to right:

- **Trust host key…** or **Review host key…** appears when a node is waiting for your decision. See [Trust a node's host key](help:trust-a-host-key).
- **Test** connects with the values in the row, without saving them, and runs the [connection checks](help:check-your-nodes). The results appear in a **Test of cell1** group under the table.
- The up and down arrows move the node in the list.
- The trash icon removes the node from the list. It's dimmed when only one node is left.

### Below the table

- **Add node** adds a row.
- **Connect my Sparks…** opens the [setup wizard](help:connect-my-sparks).
- A line such as "cell1, cell2 and cell3 share one host key, SHA256:example… (a cloned image) · trusted by you" appears when nodes share a key. [SSH, sudo and host keys](help:ssh-sudo-and-host-keys#your-nodes-shared-host-key) explains why yours do.

### Fabric and load role

The **fabric** is the fast network between your nodes. Select a **Node** to record what its connections should be. **Reported fabric** shows fresh readings; stale or offline readings show **Unknown**. The fields below are your saved expectations.


- **Use reported fabric** copies fresh, valid readings for ports with IPv4 addresses. Unaddressed spare ports stay out of the fields. Review the values, then click **Save** to confirm them. It is dimmed when those readings are unavailable.
- **Interface** is the node's network port name, such as `enp1s0f1np1`.
- **Fabric IPv4** is that port's address, such as `192.168.100.51`. It is separate from the LAN address used for login.
- **RDMA device (HCA)** names the fast network adapter, such as `rocep1s0f1`. Leave it blank if you don't know it.
- **Add interface** adds an empty row. **Remove** removes that expected interface.
- **Load role** can be **No preference**, **Head candidate** or **Worker preferred**. The head answers requests; workers help run the model. This preference doesn't change the node order or grant permission to load.
- **Clear expectations** removes the selected node's fabric fields and role preference from your draft. Click **Save** to keep that change, or **Revert** to discard it.

On the [Load page](help:models-load-page#nodes), saved expectations are compared with fresh readings for loads that use the fabric. Differences produce an advisory warning. A node marked **Worker preferred** also warns if selected as the head of a several-node load. Missing readings remain unknown; they don't count as a match.

### The save bar

A bar appears when you've edited something. With Live data it reads "Unsaved changes. Applying reconnects to the nodes." and its button is **Apply & reconnect**. When only fabric expectations or role preferences changed, the button is **Save** and connections stay open. With Demo data, the button is also **Save**. **Revert** throws your edits away. If a field has a problem, the bar says how many, and a list under the table says what's wrong.

## What it changes

The inventory is a file on this Mac. Saving it is a change on this Mac, so it asks for Touch ID or your Mac password, unless you confirmed in the last 5 minutes. The change is written to the activity log.

- Nothing here changes a node. Removing a node only stops SparkBox watching it.
- With Live data, saving login details or changing the node list reconnects the nodes. Saving only fabric expectations or role preferences keeps connections open.
- Fabric expectations never configure a node. Loads still use addresses and interfaces from fresh node checks.
- Changing an address or port means trusting that node's host key again.
- Demo and Live share this list.
- **Test** saves nothing, but it does log in to the node, so it asks for Touch ID or your Mac password too.

To undo a saved change, type the old value and save again.

## Related How-Tos

- [Add or change a node](help:add-or-change-a-node)
- [Check that your nodes are ready](help:check-your-nodes)
- [Trust a node's host key](help:trust-a-host-key)
- [Shut down and wake a node](help:shut-down-and-wake-a-node)
