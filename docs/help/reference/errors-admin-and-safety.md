---
id: errors-admin-and-safety
title: Admin, look-only and safety messages
type: troubleshooting
area: troubleshooting
summary: Messages about locks, look-only mode, the other SparkBox, node identity and the Fence.
keywords: [admin locked, look only, refused, fence, controlling, touch id, confirm it's you, pinned, other sparkbox, read-only]
verified: null
related: [admin-mode, the-fence, leases-and-identity, unlock-admin-mode, use-look-only-mode]
---

# Admin, look-only and safety messages

This page covers the messages SparkBox shows when it holds back a change. Most mean a safety feature worked as designed. The cluster hasn't been harmed, and nothing has been changed.

Every refused change is one **Refused** line in the [Activity log](help:activity-log).

## Admin mode

Admin mode is the state in which SparkBox may make changes. You unlock it with Touch ID or your Mac password. See [Admin mode](help:admin-mode).

### "Admin mode is locked"

**What it means.** You tried a change while the padlock reads **Read-only**. The body reads "Unlock it to make changes to the cluster." Nothing was changed. The full message is "Admin mode is locked. Unlock it in the top bar to make changes."

**What to do.** Click **Unlock**, then confirm with Touch ID or your Mac password. Then repeat what you were doing. See [Unlock Admin mode](help:unlock-admin-mode).

### "Confirm it's you"

**What it means.** This change is on this Mac itself, such as a setting, a saved password or a file. Those always need Touch ID or your Mac password, even with Demo data. The body reads "Changes on this Mac need Touch ID or your Mac password, even with Demo data."

**What to do.** Click **Confirm**, then confirm with Touch ID or your Mac password.

### "Confirm it's you with Touch ID or your Mac password: this changes a saved password or key."

**What it means.** Saving or removing a secret always asks again, even if you confirmed a moment ago.

**What to do.** Confirm it. If you didn't start this, cancel.

### "No changes for 15 minutes, so SparkBox is read-only again."

**What it means.** Admin mode locked itself, because you made no changes for your **Lock after** time. It also locks when the screen locks, when the Mac sleeps, and when you close the window. The Activity log gives the reason, such as "The screen was locked".

**What to do.** Nothing, unless you want to make another change. Then unlock it again. Work that's already running, such as a download or an upgrade, keeps going. To change the time, see [Settings: Security & privacy](help:settings-security).

### "Admin mode stays locked."

**What it means.** You canceled the prompt. With a reason after it, the prompt failed, and the reason says why.

**What to do.** Try again if you meant to go on.

### "Admin mode stays locked. That wasn't the right password or fingerprint."

**What it means.** macOS didn't accept what you gave.

**What to do.** Try again. Use your Mac login password, not the cluster password.

### "Admin mode stays locked. macOS didn't confirm it's you."

**What it means.** macOS ended the check without giving a reason.

**What to do.** Try again.

### "Admin mode stays locked. No answer from macOS; try again."

**What it means.** The Touch ID prompt stayed open for 3 minutes with no answer.

**What to do.** Try again. If the Mac's screen is locked, unlock it first.

### "This Mac can't confirm it's you: …"

**What it means.** macOS can't ask you right now. The end of the message gives its reason.

**What to do.** Check that your Mac has a login password. If Touch ID is unavailable, your Mac password still works.

### "Live data is on: unlocking now asks for Touch ID or your Mac password."

**What it means.** You switched to Live data while Admin mode was unlocked in Demo. With Demo data, unlocking needs no password. With Live data, it does.

**What to do.** Nothing. The next unlock asks you to confirm.

### "Auto-lock can be 5 or 15 minutes, or 1, 2, 4, 8 or 24 hours."

**What it means.** That's the full range of **Lock after**. You may also see:

- "The activity log can be kept for 30, 90 or 365 days."
- "Idle shells can lock after 15 minutes, 60 minutes, 4 hours or never."

**What to do.** Pick one of the choices in [Settings: Security & privacy](help:settings-security).

## Look-only mode

In look-only mode, SparkBox starts no change on the cluster or this Mac, and only turning the mode off asks for Touch ID or your Mac password. The padlock reads **Look only**. See [Use look-only mode](help:use-look-only-mode).

### "SparkBox is in look-only mode. It starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date."

**What it means.** Look-only mode is on, so SparkBox refused the change. It appears under the title **Look only**. "SparkBox can't restart a node in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date." is the same, for one action. Both mean that SparkBox won't start a change. Work already running by itself on a node keeps going.

**What to do.** If you want to make changes, turn it off in [Security settings](open:settings?section=security). That asks for Touch ID or your Mac password.

### "Look-only · turn off in Settings › Security & privacy"

**What it means.** This is the tooltip on a dimmed button while look-only mode is on.

**What to do.** Turn it off there, or leave it on and look around safely.

### "SPARKBOX_LOOK_ONLY=1 turned it on for this run: quit SparkBox and start it without it."

**What it means.** Look-only mode was set when SparkBox started, so the switch in Settings can't turn it off. You see this after clicking the **Look only** padlock, and on the commands that would unlock Admin mode.

**What to do.** Quit SparkBox and start it without that setting.

### "SPARKBOX_LOOK_ONLY=1 set look-only mode for this run. Quit and start SparkBox without it."

**What it means.** Look-only mode was set when SparkBox started, so a switch in Settings can't turn it off.

**What to do.** Quit SparkBox and open it the normal way, from the Applications folder or the Dock.

### "Look-only mode stays on."

**What it means.** You tried to turn it off and didn't confirm.

**What to do.** Try again if you meant to.

### "Look-only turned on: stopped before its next step"

**What it means.** A running task paused for good when look-only mode came on. You may also see:

- "Look-only turned on: its next change is refused", which is the same
- "Look-only turned on: what it started on cell2 keeps running; SparkBox can't stop it in look-only", which means work already on the node keeps going

**What to do.** Turn off look-only mode, then run the task again if you still want it.

### "Look-only mode holds the test token; it's checked once look-only ends."

**What it means.** A load finished starting, but SparkBox can't send it a test question in look-only mode. The check waits.

**What to do.** Turn off look-only mode. The check then runs by itself.

## Demo data

### "Available with Live data."

**What it means.** This action needs your real nodes, and you're using Demo data.

**What to do.** Click **Switch to Live…**. See [Switch between Demo and Live](help:switch-between-demo-and-live).

## The other SparkBox

Two copies of SparkBox can run on this Mac: the SparkBox app, and a test build. Only one may change a node at a time. In these messages, `dev` is the test build, and `release` is the SparkBox app.

### "SparkBox (dev) is controlling cell2 right now."

**What it means.** The test build holds the [controller lease](help:leases-and-identity) on this node, so your change was refused. It appears under the title **The other SparkBox is in charge**. The lease ends about 45 seconds after the other copy stops renewing it.

**What to do.** Wait for it to finish, or close it, then try again. **Open Settings › Fence** shows who holds each node.

### "glm53 on cell2 is managed by the other SparkBox (dev)."

**What it means.** That container was started by the other copy, so this one won't touch it. You may also see:

- "This unload on cell2 would remove the other SparkBox's (dev) containers."
- "This load on cell2 would start a container labeled as the other SparkBox's (dev)."

**What to do.** Change it from the copy that started it.

### "Couldn't check which SparkBox controls cell2: …"

**What it means.** SparkBox asks a node who controls it before each change, and the node didn't answer. The change was refused to be safe.

**What to do.** Check that the node is online, then try again.

## Node identity

SparkBox records which machine each node is, and refuses changes if another machine answers. See [Controller lease and identity pin](help:leases-and-identity).

### "cell2 answers as cell3-example: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence."

**What it means.** The machine at that address isn't the one SparkBox recorded. This can happen if two nodes swapped addresses. It appears under the title **Not the machine SparkBox pinned**. Nothing was changed.

**What to do.** Check that each node has the right address in [Nodes settings](open:settings?section=nodes). If you moved or rebuilt the node on purpose, click **Open Settings › Fence**, then **Re-pin identity…**.

### "SparkBox hasn't pinned which machine cell2 is yet, so changes are refused. Wait for its first data, or re-pin it in Settings › Fence."

**What it means.** The node connected a moment ago.

**What to do.** Wait for it to show data, usually within a few seconds, and try again.

### "cell2 didn't say which machine it is, so changes are refused."

**What it means.** SparkBox asked the node and got no clear answer. "cell2 isn't connected, or didn't say which machine it is." is the same.

**What to do.** Check that the node is online, then try again.

### "Type cell2 to re-pin it."

**What it means.** Re-pinning asks you to type the node's name, so that it can't happen by accident.

**What to do.** Type the name exactly.

## Other refusals

### "Background checks only read; SparkBox didn't run this one."

**What it means.** A background check tried to do something other than read. SparkBox stopped it.

**What to do.** Nothing was changed. Report it, using [Collect diagnostics](help:collect-diagnostics).

### "That didn't work"

**What it means.** This is the general title for an error with no more specific one. With an action, it reads "Couldn't restart docker on cell2". The sentence below it gives the reason.

**What to do.** Search this Help for a few words of that sentence. [Tasks](open:tasks) and the [Activity log](open:activity) show the node's own output.

## The Fence: test build only

The Fence lets the test build, a separate copy of SparkBox used for testing, change chosen nodes for a limited time. **The SparkBox app doesn't use one.** Its changes need Admin mode, so you won't see these messages there. If you do see one, you're in the test build.

Fence messages appear under the title **Outside the fence**, with the line "The test build's changes need an open fence that names the node and the kind of change." and a button, **Open Settings › Fence**. See [The Fence](help:the-fence).

### "No fence is open, so changes on the cluster are refused. This test build needs one: open it in the release SparkBox (Settings › Fence)."

**What it means.** The test build has no permission to change anything right now.

**What to do.** Test build only. In the SparkBox app, open **Settings › Fence** and fill in **Open a fence**. It asks for Touch ID or your Mac password. See [Settings: Fence](help:settings-fence).

### "The fence expired at 14:05."

**What it means.** The fence ran out of time. Fences last 8 hours at most.

**What to do.** Test build only. Open a new one in the SparkBox app, if the test build still needs it.

### "cell2 isn't in the open fence."

**What it means.** A fence is open, but it doesn't name this node.

**What to do.** Test build only. Close it and open one that names the node.

### "The fence doesn't allow this change."

**What it means.** The command isn't one of the kinds of change a fence can name, so no fence covers it.

**What to do.** Test build only. No fence can allow this. Make the change in the SparkBox app, with Admin mode.

### "The fence doesn't allow service changes."

**What it means.** A fence is open, but it doesn't allow this kind of change. The words differ by kind, such as "package upgrades" or "restarts and shutdowns".

**What to do.** Test build only. Open a fence with that class turned on.

### "The fence doesn't allow service changes to docker.service: it names only nginx.service."

**What it means.** The fence allows the class, but only for the items it lists.

**What to do.** Test build only. Open a fence that lists this item, or leave the list empty.

### "This build doesn't trust the fence key yet, so changes on the cluster are refused."

**What it means.** The test build doesn't yet trust the key that signed the fence.

**What to do.** Test build only. Open **Settings › Fence** in the test build, and use its button to trust the key.

### "The fence record failed its check: open a new one."

**What it means.** The fence file changed after it was signed, so it's ignored. In the SparkBox app, a banner says "The fence record failed its check · the test build's changes are refused".

**What to do.** Test build only. Open a new fence in the SparkBox app. If you didn't edit the file, find out what did.

### "This fence is for the release build."

**What it means.** The open fence was made for another build.

**What to do.** Test build only. Close it and open a new one.

### "cell2 isn't in this build's open fence, so this build can't put it in maintenance."

**What it means.** Maintenance mode in the test build needs the node in the fence.

**What to do.** Test build only. Open a fence that names the node.

### "SPARKBOX_ADMIN_NOAUTH needs an open fence in Live."

**What it means.** The test build was started with Admin mode unlocked for scripts, and that counts for nothing without a fence.

**What to do.** Test build only. Open a fence in the SparkBox app.

### "Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts."

**What it means.** The Fence key, identity pins and changed host keys move only after a real Touch ID. A scripted unlock can't do it.

**What to do.** Test build only. Unlock Admin mode by hand.

### "Open fences in the release SparkBox."

**What it means.** Only the SparkBox app opens fences. The test build can't open its own.

**What to do.** Test build only. Use the SparkBox app. "There's no fence key yet: open a fence in the release SparkBox first." means the same.

### "A fence is for the test build: the release build's changes need Admin mode, not a fence."

**What it means.** A fence was asked for the SparkBox app, which doesn't need one.

**What to do.** Test build only. Unlock Admin mode for your own changes.
