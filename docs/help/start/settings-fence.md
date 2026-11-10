---
id: settings-fence
title: "Settings: Fence"
type: feature
area: settings
summary: "Only the test build uses a fence. Your SparkBox app needs Admin mode and nothing else."
keywords: [fence, lease, identity, re-pin, test build, t1, t2, classes of change]
verified: false
related: [open-a-fence, unlock-admin-mode, trust-a-host-key, the-fence, leases-and-identity, admin-mode]
---

# Settings: Fence

## What it's for

Only the **test build**, a separate copy of SparkBox used for testing, uses a fence. Your SparkBox app needs Admin mode and nothing else, whatever you're doing. If you don't use the test build, you only need this section if a node's identity changes.

This section does three jobs. It lets you allow the test build to make changes on chosen nodes for a limited time. It shows which copy of SparkBox holds each node's controller lease. And it shows which machine each node is pinned as.

[The Fence](help:the-fence) explains the idea, and [Controller lease and identity pin](help:leases-and-identity) explains the two checks.

## Where to find it

- **Settings:** press <kbd>⌘,</kbd>, then click **Fence**.
- **Fence banner:** while a fence is open, click **Settings › Fence** on the banner under the top bar.
- **Link:** [Open Fence settings](open:settings?section=fence)

This section works with Live data only. With Demo data, it says "Available with Live data".

## What's on the screen


### Fence

#### State

What the fence is doing now:

| State | What it means |
|---|---|
| **No fence** | None is open. The test build can't change anything on the cluster |
| **Open** | A fence is open. The row shows the time left and who it's for |
| **Expired** | Its time ran out. The test build's changes are refused |
| **Failed its check: the test build's changes are refused until a new fence is opened** | The record was edited, or an old one was put back. The hint says why |
| **This build doesn't trust the fence key yet** | Shown in the test build only, until it trusts the key |

None of these states affects your own changes in the SparkBox app.

You may see "for this build, which doesn't need one" beside **Open**. That's a fence from before the app stopped needing them. It does nothing, and you can close it.

While a fence exists, more rows appear: **Nodes**, **Classes**, any **Only** lists, and **Opened**, with its start and end times.

#### Key and Trusted here

A fence is signed, so that the test build can tell it's real.

- **Key** is the short id of the signing key. The SparkBox app makes this key the first time it opens a fence, and keeps it in its own Keychain item.
- **Trusted here** is the key this copy of SparkBox accepts fences from. The hint says which build you're in.

#### Close fence

**Close fence** ends a fence early and asks you to confirm. It's always allowed, even in look-only mode, because it only takes rights away.

In the test build, this group also offers **Trust this Mac's fence key**, which asks for Touch ID or your Mac password. It tells that copy which key to trust. A test build can't open a fence: it says "This test build needs a fence for every change: open it in the release SparkBox."

### Open a fence

This group appears in the SparkBox app only, and it always opens a fence for the test build. The line under it says the same: "This app doesn't need a fence: its changes need Admin mode." [Let the test build make changes for a limited time](help:open-a-fence) gives the steps. The rows appear in this order.

#### Presets

Buttons that fill in the form. **Agent tests on cell2 (test build)** chooses `cell2`, the three **Agents alone** classes and 8 hours, and fills **Note** with "Agent tests on cell2". Here "agent tests" means the test build's automated tests.

#### Nodes

The nodes the fence covers. Select each node the test build may change.

#### Classes of change

Each class allows one kind of change. Anything you don't select is refused. The classes come in two lists.

**With you present** is marked "First runs, sudo, power, apt, network". Its 16 classes cover first runs of sudo, power, package and network changes, which you'd want to watch:

| Class | What it allows |
|---|---|
| `sudo-probe` | Commands that run as root but only read, such as a sudo check or reading a root-only file |
| `root-helper` | Installing SparkBox's root helper, the copy of its script that it runs as root |
| `service` | Start, stop, restart, reload, enable or disable a service, and reload unit files |
| `container-owner` | Start, stop, restart, pause, kill or remove a container that isn't a test container or part of a load |
| `fleet` | Any command typed into [Fleet Command](help:fleet-command) |
| `shell` | Shells in [Terminal](help:terminal), and SSH tunnels to a page on a node |
| `power-one` | Restart or shut down a node, cancel a shutdown, and send a Wake-on-LAN packet |
| `apt-refresh` | Refresh the package lists with `apt-get update` |
| `apt-one` | Upgrade named packages, and cancel an upgrade |
| `apt-hold` | Hold a package at its version, or release it |
| `wifi` | Turn a node's Wi-Fi on or off |
| `model-load-one` | Start a model on one node |
| `model-load` | Start a model on several nodes. It also covers `model-load-one`, so one fence can do both |
| `model-unload` | Stop a load and remove its containers |
| `drop-caches` | Drop the page cache |
| `rdp-repair` | Apply a reviewed xrdp configuration repair and verify encrypted remote desktop access |

**Agents alone** is marked "Repeats agents may run by themselves". Here "agents" means the test build's automated tests. Its 3 classes cover test work that the test build repeats by itself:

| Class | What it allows |
|---|---|
| `container-test` | Run, stop and remove test containers, named `sparkbox-test-…`, with no root access |
| `process-test` | Send a signal to a process, or change its priority |
| `files-test` | Create, rename, save, copy and delete files and change their permissions, inside `~/sparkbox-test` only |

> [!WARNING]
> **`fleet` and `shell` are the widest classes.** They let the test build run commands you can't list in advance, as root if it asks. Allow them only on the node you mean, for as short a time as you can. Close the fence as soon as you're done.

#### Only

**Only (service)** and similar rows appear when you select a class that can be narrowed: `service`, `container-owner`, `container-test`, `apt-one` or `apt-hold`. Type the service, container or package names, separated by commas. Empty allows any.

#### Hours

How long the fence stays open. The form starts at 3, and a fence lasts at most 8 hours. It ends by itself when the time is up.

#### Note

A reminder to yourself, shown in the **Opened** row.

#### Open fence…

Opens the fence. It's dimmed until you've chosen a node and a class.

You see a dialog that sums up the fence. Click **Open fence…**. macOS then asks for Touch ID or your Mac password, and its prompt names the nodes, classes and hours. A message confirms "Fence open".

### Controller lease

One row for each node, refreshed every 15 seconds. It shows **Free**, or who holds the lease. A held lease shows the build, its version tag, the Mac and how long ago it checked in. Here `release` is the SparkBox app and `dev` is the test build, and "This build" means the copy you're looking at. A row can also show why the lease couldn't be read.

### Identity

One row for each node. It shows the machine the node is pinned as: its hostname, LAN MAC and, where it's unique, its machine-id. For example, `gx10-0000` reads "gx10-0000 · 30:c5:99:79:4d:07 · machine-id d103edc9…". A row reads "Not pinned yet (pinned at its first data after its host key is trusted)" until SparkBox has its first data.

The hint "machine-id is shared by cell1, cell2 and cell3; pinned hostname and LAN MAC" is expected on your cluster. It's there because those three nodes came from one cloned image.

If a node answers as another machine, the row turns red, and the hint says "Answers as cell3-example · …: changes refused".

**Re-pin identity…** accepts the machine answering now as that node. Use it only after you moved or rebuilt the node. If you didn't, find out why it changed.

## What it changes

| Control | What it needs | What it changes | How to undo |
|---|---|---|---|
| **Open fence…** | Touch ID or your Mac password, each time | Lets the test build make those changes | **Close fence** |
| **Close fence** | Your confirmation | Ends the fence, and the test build's changes are refused again | Open a new one |
| **Re-pin identity…** | The node's name typed, and Touch ID or your Mac password, each time | Changes SparkBox's record on this Mac. Nothing changes on the node | Re-pin again when the right machine answers |
| **Trust this Mac's fence key** | Test build only. Touch ID or your Mac password, each time | The key the test build accepts | Trust a new key |

Opening, closing and re-pinning are written to the activity log. In look-only mode, **Open fence…** and **Re-pin identity…** are dimmed, and **Close fence** still works.

## Related How-Tos

- [Let the test build make changes for a limited time](help:open-a-fence)
- [Trust a node's host key](help:trust-a-host-key)
- [Unlock Admin mode](help:unlock-admin-mode)
- [Find out what SparkBox did](help:find-what-sparkbox-did)

![Settings: Fence, using synthetic Demo data](img/public-settings-fence.jpg)
