---
id: use-look-only-mode
title: Use look-only mode
type: howto
area: safety
summary: Stop SparkBox starting any change while you look around, or while someone else is using your Mac.
keywords: [look only, read only, read-only, safe, lock everything, button disabled, button greyed out, button grayed out, greyed out, grayed out, cant change anything, wont let me, cant click, nothing happens, by accident, by mistake, accidental]
verified: false
related: [admin-mode, settings-security, unlock-admin-mode]
---

# Use look-only mode

To stop SparkBox starting any change while you look around, turn on look-only mode in **Settings**.

## Before you start

- You don't need Admin mode to turn it on. It only takes rights away, so it asks for nothing.
- To turn it off again, you need Touch ID or your Mac password.
- It takes a few seconds.
- Look-only mode stops more than changes. Chat doesn't send, and the Server stops, so any app using it loses its connection. Plan for that if someone is relying on a model.

## Steps

### Turn it on

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Security settings](open:settings?section=security)
2. In the list on the left, click **Security & privacy**.
3. Under **Look only**, turn on **Look-only mode**. It takes effect at once. A message appears: "Look-only mode is on".
4. Look at the padlock in the top bar. It reads **Look only**.

### Turn it off

1. Open **Settings › Security & privacy** again.
2. Under **Look only**, turn off **Look-only mode**. SparkBox asks for Touch ID or your Mac password. Confirm it. A message appears: "Look-only mode is off", with "Admin mode can be unlocked again."
3. Look at the padlock. It reads **Read-only**. Admin mode stays locked until you unlock it.

## What you should see

While look-only mode is on:

- The padlock reads **Look only**.
- Change buttons are dimmed. Their tooltip reads "Look-only · turn off in Settings › Security & privacy".
- Most **Settings** forms are read-only. **Security & privacy**, **Fence** and **Diagnostics** stay open, so that you can turn this switch off.

Here's what stays and what stops:

| Still works | Stops |
|---|---|
| The Dashboard, charts, alerts and every monitoring window | Every change on the cluster or this Mac |
| Reading files in **Files** | Terminal shells |
| Following logs | Chat, and the Server |
| **Capture evidence** | **Run doctor**, and **Test connection** on a switch |
| | Exports to Downloads, and saving or deleting chats |

There's one small exception. SparkBox keeps its helper scripts in `~/.sparkbox` on each node up to date, by checksum, even in look-only mode. Work already running by itself on a node keeps going too.

SparkBox still lists each node's jobs in this mode, and a job that stopped unexpectedly reads **failed**, but the node's files stay as they are. Its status and any secret files it left wait until look-only mode is off.

## Why it works

SparkBox checks look-only mode before it runs any command, and it refuses everything that isn't a read. It never reaches the point of asking for Touch ID. A second check sits where commands leave for your nodes, so background work is held too. Work already running by itself on a node, such as an upgrade or a loading model, keeps going there. Look-only mode is saved, so it stays on if you quit and reopen SparkBox.

## If something goes wrong

- **A dialog named Look only appears, with "SparkBox can't … in look-only mode".** You tried something that look-only mode holds. Click **OK**, or turn the mode off.
- **The switch goes back on after you turn it off.** You canceled the Touch ID prompt, so look-only mode stays on. Turn the switch off again and confirm.
- **The switch is dimmed, and its hint says "Set by SPARKBOX_LOOK_ONLY for this run".** SparkBox was started from a terminal with that setting. Quit SparkBox and open it the normal way. You can't turn this off in **Settings**.
- **An upgrade or model keeps running after you turned it on.** That's expected. It runs on the node by itself, and stopping it is a change. Turn look-only mode off if you need to stop it.

## Next steps

- [Read-only, Admin mode and Look only](help:admin-mode)
- [Unlock Admin mode](help:unlock-admin-mode)
- [Settings: Security & privacy](help:settings-security)
