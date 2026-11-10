---
id: the-fence
title: The Fence
type: concept
area: safety
summary: The Fence limits what the test build may change. The SparkBox app itself doesn't need one.
keywords: [fence, test build, write fence, change window]
verified: null
related: [open-a-fence, settings-fence, admin-mode, leases-and-identity]
---

# The Fence

## In short

The Fence limits what the **test build** may change on your nodes. The test build is a separate copy of SparkBox used for testing new work. The SparkBox app you use every day doesn't need a fence: its changes need only Admin mode. If you never run the test build, you can leave **Settings › Fence** alone.

## How it works

### Two builds, two rules

There are two copies of SparkBox, and they follow different rules for changes on your nodes:

| | SparkBox app | Test build |
|---|---|---|
| What it is | The app you use | A separate copy used for testing |
| What a change on a node needs | Admin mode, confirmed with Touch ID or your Mac password | An open fence that allows that change |
| Can it open a fence? | Yes, for the test build | No, it can only read one |

Which rule applies is fixed when the app is built. Nothing can move it while SparkBox runs, whether a setting, a file or a command, so the SparkBox app can't start needing a fence by accident. The same goes the other way: the test build can't start working without one.

### What a fence says

A fence is a short, signed record. It names:

- **The nodes** the test build may change.
- **The classes of change** it may make, such as `service` for service changes or `apt-one` for package upgrades.
- **Which items**, if you want to narrow a class to named services, containers or packages.
- **How long it lasts**, up to 8 hours.

While a fence is open, the test build checks it before every change. A change outside it is refused: another node, another class, an item the list doesn't name, or a fence that has run out. Reading is never fenced, so the test build can still look at everything.

### Who opens one

Only the SparkBox app opens a fence, with Touch ID or your Mac password. The confirmation names the nodes, the classes and the hours, so you can see what you're agreeing to. The app signs the record with a private key kept in its own Keychain item. The test build checks the signature with the key it trusts.

If the record is edited by hand, the check fails and every change from the test build is refused until you open a new fence. Putting an old record back doesn't work either, and a closed fence stays closed.

### What it can't do

A fence catches mistakes and edits to the fence file. It doesn't stop someone who changes this check in the test build's own source code. Nor does it stop a program that can write SparkBox's own settings folders on this Mac.

### An example

Say you want the test build to try its service changes on `cell2` only. In the SparkBox app, you open a fence that names `cell2`, the class `service` and 3 hours. For the next 3 hours the test build may restart services on `cell2`. A change on `cell3`, or a package upgrade on `cell2`, is refused. After 3 hours, everything is refused again.

## Why it matters in SparkBox

You meet the Fence in two places:

- [Settings › Fence](help:settings-fence) shows whether a fence is open, and lets you close one. [Let the test build make changes for a limited time](help:open-a-fence) shows how to open one.
- A banner under the top bar appears while a fence is open for the test build. It looks like this: "Fence open for the test build: cell2 · 3 classes · 2 h 41 min left".

You never need a fence for your own work. Downloads, loads, updates and restarts in the SparkBox app need Admin mode and nothing else. If you see "No fence is open, so changes on the cluster are refused.", you're using the test build.

Some checks apply to both builds, with or without a fence:

- [Look-only mode](help:admin-mode#look-only) refuses every change.
- The [identity pin](help:leases-and-identity#identity-pin) refuses a change if the node isn't the machine it was.
- The [controller lease](help:leases-and-identity#controller-lease) stops two copies of SparkBox acting on one node at once.
- The [activity log](help:activity-log) records every command SparkBox runs. It doesn't record what you type in a shell, or what you do on a page you open through a tunnel.

## Related

- [Settings: Fence](help:settings-fence)
- [Read-only, Admin mode and Look only](help:admin-mode)
- [Controller lease and identity pin](help:leases-and-identity)
- [Admin, look-only and safety messages](help:errors-admin-and-safety)
