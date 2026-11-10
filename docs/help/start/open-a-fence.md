---
id: open-a-fence
title: Let the test build make changes for a limited time
type: howto
area: safety
summary: Open a fence so that the test build can change chosen nodes for a limited time. Your own work never needs one.
keywords: [fence, open a fence, close fence, test build, change window, t1, t2]
verified: false
related: [the-fence, settings-fence, leases-and-identity, unlock-admin-mode]
---

# Let the test build make changes for a limited time

To let the test build change chosen nodes for a limited time, open a fence in **Settings › Fence**.

## Before you start

- You need this only if you use the **test build**, a separate copy of SparkBox used for testing. Your own work in the SparkBox app never needs a fence: its changes need Admin mode and nothing else.
- Use the SparkBox app with **Live** data. The test build can't open a fence, and with **Demo** data the section reads "Available with Live data".
- Look-only mode must be off.
- You need Touch ID or your Mac password. Opening a fence asks every time.
- Decide three things first: which nodes, which [classes of change](help:settings-fence#classes-of-change), and how many hours. Allow as little as the work needs.

> [!WARNING]
> **`fleet` and `shell` let the test build run anything, as root if it asks.** Select them only for the node you mean, for as short a time as you can. If you allow more than you meant to, click **Close fence** in the same section, and the test build's changes are refused again.

## Steps

### Open a fence

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Fence settings](open:settings?section=fence)
2. In the list on the left, click **Fence**. **State** reads **No fence**, and the **Open a fence** group is below it.
3. Under **Nodes**, select each node the test build may change.
4. Under **With you present** and **Agents alone**, select each class it may use. To fill in the form for the usual test work, click **Agent tests on cell2 (test build)** under **Presets** instead. That chooses `cell2`, the three **Agents alone** classes and 8 hours.
5. If you selected a class that can be narrowed, an **Only** row appears for it, such as **Only (service)**. To limit the class, type names separated by commas, such as `earlyoom.service`. Leave it empty to allow any.
6. In **Hours**, type how long the fence stays open. The form starts at 3, and the most is 8.
7. In **Note**, type a reminder of what this fence is for. You can leave it empty.
8. Click **Open fence…**. A dialog sums up the fence, such as "Open a fence: cell2 · 3 classes · 8 h".
9. Read it, then click **Open fence…** in the dialog. macOS asks you to confirm, and its prompt names the nodes, the classes and the hours. It says `dev build`, which is the test build. Use Touch ID or your Mac password. If Admin mode was locked, this also unlocks it.
10. Look for the message "Fence open".

### Close it when you're done

A fence ends by itself when its time is up. To end it sooner:

1. In **Settings › Fence**, click **Close fence**. A dialog named **Close the fence?** opens.
2. Click **Close fence**. **State** reads **No fence**, and the banner goes away.

Closing is always allowed, even in look-only mode, because it only takes rights away. To change an open fence, close it and open a new one.

## What you should see

In **Settings › Fence**, **State** reads **Open**, with the time left and "for the test build". New rows show its **Nodes**, **Classes** and **Opened** time.

A banner sits under the top bar for as long as the fence is open, such as "Fence open for the test build: cell2 · 3 classes · 7 h 59 min left". Its **Settings › Fence** button brings you back here.

The fence changes nothing in your own app. Your changes still need Admin mode, as they did before.

## Why it works

The SparkBox app signs the fence with a private key kept in its Keychain item. Your confirmation covers this exact fence, so it can't be used to open a different one. The test build checks the signature, then checks the fence before every change. A change outside the fence is refused: another node, another class, an item the **Only** row doesn't list, or a fence that has run out. Opening and closing a fence are written to the [activity log](help:activity-log).

## If something goes wrong

- **There's no Open a fence group.** You're in the test build, or you're using Demo data. In the test build, the section says "This test build needs a fence for every change: open it in the release SparkBox." Open the fence in the SparkBox app.
- **Open fence… stays dimmed.** You haven't selected a node and a class, **Hours** is empty, or look-only mode is on.
- **"An `only` item has characters a unit, container or package name doesn't."** Check the **Only** row for a typing mistake. Names are separated by commas.
- **"The fence doesn't allow service changes."** The test build tried something the fence doesn't cover. Close the fence and open one with that class. The words change with the class.
- **"cell2 isn't in the open fence."** Close the fence and open one that names that node.
- **"This build doesn't trust the fence key yet"** appears in the test build. In that copy's **Settings › Fence**, click **Trust this Mac's fence key**, then confirm with Touch ID or your Mac password.
- **"The fence record failed its check · the test build's changes are refused".** The record was edited, or an old one was put back. Open a new fence. If you didn't edit it, find out what did.
- **"No fence is open, so changes on the cluster are refused."** This message comes from the test build. It never comes from your SparkBox app.

[Admin, look-only and safety messages](help:errors-admin-and-safety) lists the rest.

## Next steps

- [The Fence](help:the-fence)
- [Settings: Fence](help:settings-fence)
- [Find out what SparkBox did](help:find-what-sparkbox-did)
