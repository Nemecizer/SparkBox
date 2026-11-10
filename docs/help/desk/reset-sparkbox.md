---
id: reset-sparkbox
title: Reset SparkBox
type: howto
area: settings
summary: Start clean by moving SparkBox's folder aside, and know what that keeps and loses.
keywords: [reset, start over, factory, clean, reinstall, defaults, fresh start]
verified: null
related: [back-up-sparkbox, restore-sparkbox, files-and-folders, connect-your-cluster]
---

# Reset SparkBox

To start clean, quit SparkBox and move its folder aside. You can move the folder back if you change your mind.

## Before you start

- Move folders aside, and don't delete anything as a first step. Moving is what lets you go back.
- A smaller fix may be enough:
  - For Terminal's look, click **Restore defaults** in [Settings › Terminal](help:settings-terminal).
  - For a table, right-click its header and click **Reset columns**.
  - For a window that shows an error, close it and open it again.
- Make a [backup](help:back-up-sparkbox) first if you have the room.
- It takes a few minutes, plus the time to [connect your cluster](help:connect-your-cluster) again.

### What a reset loses and keeps

| Area | After a reset |
|---|---|
| Nodes and switches | Back to the four built-in nodes. Switch and gateway addresses are empty |
| Data source | Back to **Demo** |
| Trusted host keys and identity pins | Gone. You trust each node again |
| Security settings | Back to the defaults: Admin mode locks after 15 minutes, idle shells after 60 minutes, and look-only mode is off |
| Providers, the Server, the VPN and notifications | Their settings are gone. Their saved keys stay in the Keychain |
| Activity log, tasks, alert history, chart history and chats | Not shown. They're in the folder you moved |
| SparkBox's record of loads and benchmarks | Not shown. Models that are running keep running |
| Passwords and keys in the Keychain | **Kept** |
| Everything on your nodes | **Kept**: models, containers, always-on agents, helpers and files |
| Your logs and evidence captures | **Kept** |

Window layouts, appearance and favorites are kept unless you also move the `WebKit` folder, as the steps show.

## Steps

### Quit SparkBox

Closing the window isn't enough, since SparkBox keeps running in the menu bar. If it's still running when you move the folder, it goes on writing files, and the reset won't be clean.

Quitting ends your terminal sessions and stops the **Server**. Models on your nodes keep running. Wait for any copy in **Files** to finish first.

1. Click the SparkBox icon in the macOS menu bar. Its menu opens.
2. Click **Quit SparkBox**. The icon leaves the menu bar.

### Move the folder aside

3. In Finder, press <kbd>⇧⌘G</kbd>. The **Go to Folder** box opens.
4. Type `~/Library/Application Support`, then press <kbd>Return</kbd>. The `~` means your home folder, and Finder shows the folder.
5. Click the folder `com.nemecj.sparkbox` once, then press <kbd>Return</kbd>. Its name becomes editable.
6. Change its name to `com.nemecj.sparkbox.before-reset`, then press <kbd>Return</kbd>.

To reset layouts and appearance too:

7. Press <kbd>⇧⌘G</kbd>, type `~/Library/WebKit`, then press <kbd>Return</kbd>.
8. Rename `com.nemecj.sparkbox` to `com.nemecj.sparkbox.before-reset` in the same way.

Leave the folder `SparkBox Fence` alone. It applies to the test build only.

### Start SparkBox clean

9. Open SparkBox from your **Applications** folder. It makes a new, empty folder and starts with **Demo** data.
10. To use your own cluster again, follow [Connect your cluster](help:connect-your-cluster). Your cluster password is still in the Keychain, so you won't need to type it again. You do need to [trust each node's host key](help:trust-a-host-key) again.

### Go back to how it was

If you change your mind:

1. Quit SparkBox.
2. In `~/Library/Application Support`, rename the new `com.nemecj.sparkbox` to `com.nemecj.sparkbox.after-reset`.
3. Rename `com.nemecj.sparkbox.before-reset` back to `com.nemecj.sparkbox`.
4. If you moved the `WebKit` folder, do the same in `~/Library/WebKit`.
5. Open SparkBox. Everything is as it was before the reset.

### Remove the old folder

Keep the old folder until you're sure you don't need it.

> [!DANGER]
> **Deleting the old folder removes your activity log, chats and history for good.** Nothing in SparkBox can bring them back. Stop if you might want them, and keep the folder or copy it to a backup first.

1. In Finder, move `com.nemecj.sparkbox.before-reset` to the Trash.

## What you should see

The top bar says **Demo**, and the sidebar lists `cell1`, `cell2`, `cell3` and `gx10-0000`. The activity log is empty, or has only a few new entries.

If you also moved the `WebKit` folder, **Welcome to SparkBox** opens, and your windows are gone from the desk.

## Why it works

SparkBox reads its folder when it starts. If the folder isn't there, it uses its built-in defaults and makes a new one. Your old folder is untouched under its new name, which is why you can go back.

Passwords are in the Keychain and not in this folder, so a reset leaves them alone. SparkBox's nodes don't depend on the folder either, so anything running on them keeps going.

## Limits

- SparkBox has no reset button, so you do this by hand in Finder.
- A reset doesn't remove anything from your nodes. Always-on agents keep running and keep sending alerts with their last settings.
- A reset doesn't clear saved passwords and keys. To remove them, follow [Remove every secret](help:keychain-and-secrets#remove-every-secret), before you reset.
- After a reset, SparkBox has no record of the loads it started. The models keep running on their nodes, and you can still see their containers in [Containers](help:containers).

## If something goes wrong

- **SparkBox still shows your old settings.** It was still running when you opened it, or you renamed the wrong folder. Quit it from the menu bar icon and check the folder's name.
- **A node shows Error, and its tooltip says "Host key not trusted yet".** This is expected after a reset. See [Trust a node's host key](help:trust-a-host-key).
- **"Can't reach the SparkBox core: …"** The app's core didn't answer. Click **Retry**. If it comes back, quit SparkBox and go back to your old folder, as the steps show.
- **"Couldn't read inventory.json. SparkBox started with defaults and kept the old file beside it."** You may have changed a file by hand. Click **Show in Finder** to see it.

## Next steps

- [Connect your cluster](help:connect-your-cluster), to get back to Live data
- [Back up SparkBox](help:back-up-sparkbox), to keep a copy for next time
- [Where SparkBox keeps things](help:files-and-folders), to see what was in the folder
