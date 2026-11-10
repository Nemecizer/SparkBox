---
id: leases-and-identity
title: Controller lease and identity pin
type: concept
area: safety
summary: "Two checks before any change: one SparkBox at a time starts changes on a node, and the node must be the machine it was."
keywords: [lease, identity, pin, re-pin, machine-id]
verified: null
related: [settings-fence, the-fence, ssh-sudo-and-host-keys]
---

# Controller lease and identity pin

## In short

SparkBox makes two checks before it changes anything on a node. The **controller lease** makes sure only one copy of SparkBox starts changes on a node at a time. The **identity pin** makes sure the machine answering is the one you expect. Both run by themselves, and you only hear about them if one refuses a change.

## How it works

### Controller lease

Two copies of SparkBox can reach the same node: the SparkBox app and the [test build](help:the-fence). The lease stops them working on one node at the same moment.

Before a change, SparkBox claims the node. Each node keeps a small record, `~/.sparkbox/controller.json`, saying which copy holds the claim, on which Mac, and when it last checked in. SparkBox renews its claim every 15 seconds while Admin mode is unlocked. The SparkBox app also renews it while its own work runs on that node, such as a download, a load or an upgrade, so a lock doesn't free a busy node. This holds while SparkBox is running and the Mac is awake. If SparkBox quits or the Mac sleeps, work on the node keeps going with no lease behind it. The test build still needs a fence before it can change that node.

If the other copy checked in less than 45 seconds ago, SparkBox refuses your change. The message reads like this: "SparkBox (dev) is controlling cell2 right now." Here `dev` means the test build, and `release` means the SparkBox app. After 45 seconds with no check-in, the claim counts as stale and either copy may take it.

The lease is SparkBox's own record-keeping, not a change to your node's setup. SparkBox gives it back when it quits. When Admin mode locks with no work of SparkBox's running on the node, or when look-only mode turns on, SparkBox stops renewing the claim, and the node frees it after 45 seconds. Look-only mode never writes the lease.

If a node can't answer the claim, the change is refused. SparkBox doesn't guess.

The same care applies to containers. Each copy of SparkBox labels the model containers it starts, and it refuses to stop or remove the other copy's.

### Identity pin

The identity pin is SparkBox's record of which machine each node is. It's different from the [host key](help:ssh-sudo-and-host-keys#host-keys), which proves you reached a machine you trusted. The pin checks that it's the right one of your machines.

SparkBox makes the pin from a node's first data after you trust its host key. It records:

- The node's **hostname**, its own name for itself.
- The **MAC address** of its LAN port, the hardware address of the network socket.
- Its **machine-id**, a long number in `/etc/machine-id` that is meant to be unique to each installation.

Before every change, SparkBox compares what the node says now with its pin. If they differ, SparkBox refuses the change. The message reads like this: "cell2 answers as cell3-example: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence." Reading still works, so you can look at the node and work out what happened.

For example, say the gateway gives `cell2`'s address to `cell3` after a power cut. SparkBox reaches the address and the host key is fine, because those two nodes share one. Without the pin, a restart meant for `cell2` would restart `cell3`. With the pin, SparkBox sees the wrong hostname and refuses.

### Your nodes share a machine-id

`cell1`, `cell2` and `cell3` have the same machine-id, because they were installed from one cloned image. `gx10-0000` has its own. A number that several machines share can't tell them apart. So SparkBox leaves it out of those three pins, and records only the hostname and LAN MAC. Settings shows this as "machine-id is shared by cell1, cell2 and cell3; pinned hostname and LAN MAC". That's expected, and those two facts are enough to catch a mix-up.

### Accepting a new identity

If you moved or rebuilt a node, its identity really has changed. You tell SparkBox with **Re-pin identity…** in **Settings › Fence**. You type the node's name and confirm with Touch ID or your Mac password. Trusting a changed host key also clears the pin, and SparkBox makes a new one when the node next connects.

## Why it matters in SparkBox

Most days you won't notice either check. You see them in [Settings › Fence](help:settings-fence), which lists who holds each node's lease and how each node is pinned. Both checks work the same in the SparkBox app and the test build, even though only the test build needs a fence.

The pin and the lease apply to Live data only. Demo has no real machines to mix up.

If a change is refused, read the message before you act:

| Message starts with | What happened | What to do |
|---|---|---|
| "SparkBox (dev) is controlling" | The other copy is working on the node | Wait, or quit the other copy. The claim frees 45 seconds after its last check-in |
| "cell2 answers as" | A different machine answered | Find out why. Re-pin only if you moved or rebuilt the node |
| "SparkBox hasn't pinned which machine" | The node has no pin yet | Wait for its first data, or re-pin it |

[Admin, look-only and safety messages](help:errors-admin-and-safety) has the full list.

## Related

- [Settings: Fence](help:settings-fence)
- [The Fence](help:the-fence)
- [SSH, sudo and host keys](help:ssh-sudo-and-host-keys)
- [Trust a node's host key](help:trust-a-host-key)
