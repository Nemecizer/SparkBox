---
id: restore-sparkbox
title: Restore SparkBox from a backup
type: howto
area: settings
summary: Put a backup of SparkBox's folders back in place, on this Mac or on a new one.
keywords: [restore, restore backup, new mac, move to a new mac, put settings back, get settings back, recover settings]
verified: null
related: [back-up-sparkbox, reset-sparkbox, files-and-folders, keychain-and-secrets]
---

# Restore SparkBox from a backup

To put SparkBox back as it was, quit it and copy the folders from your backup into place with Finder.

## Before you start

- You need a backup made with [Back up SparkBox](help:back-up-sparkbox).
- A restore replaces what SparkBox has on this Mac. Anything newer than the backup, such as later chats and activity, moves aside with the old folder. The steps keep that folder, so you can go back.
- A restore doesn't touch your nodes. Models and containers stay as they are.
- A restore doesn't bring back passwords and keys, because a backup doesn't hold them. On this Mac they stay in the Keychain. On a new Mac you save them again, so check first that you [can get each one back](help:back-up-sparkbox#check-you-can-get-your-passwords-back).
- SparkBox must be in your **Applications** folder.
- It takes a few minutes.

## Steps

### Quit SparkBox

Closing the window isn't enough, since SparkBox keeps running in the menu bar. If it's still running, it goes on writing to the folder you're about to replace. On a new Mac where SparkBox has never run, skip this stage.

Quitting ends your terminal sessions, stops the **Server**, and stops SparkBox watching the cluster from this Mac. Models on your nodes keep running. Wait for any copy in **Files** to finish first.

1. Click the SparkBox icon in the macOS menu bar. Its menu opens.
2. Click **Quit SparkBox**. The icon leaves the menu bar.

### Put the settings folder back

The `~` in these paths means your home folder. Your **Library** folder is hidden in Finder, so go there by typing its path.

3. In Finder, press <kbd>⇧⌘G</kbd>. The **Go to Folder** box opens.
4. Type `~/Library/Application Support`, then press <kbd>Return</kbd>. Finder shows that folder.
5. If you see a folder named `com.nemecj.sparkbox`, click it once, then press <kbd>Return</kbd>. Its name becomes editable.
6. Change its name to `com.nemecj.sparkbox.old`, then press <kbd>Return</kbd>. This keeps it, in case you want it back.
7. Press <kbd>⌘N</kbd> to open a second Finder window, and open your backup's `Application Support` folder in it.
8. Hold <kbd>⌥</kbd> and drag `com.nemecj.sparkbox` from the backup into the first window. Finder copies it and leaves the backup as it was.

### Put the layouts folder back

Skip this stage if your backup has no `WebKit` folder.

9. In the first Finder window, press <kbd>⇧⌘G</kbd>, type `~/Library/WebKit`, then press <kbd>Return</kbd>.
10. If you see `com.nemecj.sparkbox`, rename it to `com.nemecj.sparkbox.old` in the same way.
11. In the second window, open your backup's `WebKit` folder.
12. Hold <kbd>⌥</kbd> and drag `com.nemecj.sparkbox` from the backup into the first window.

### Open SparkBox

13. Open SparkBox from your **Applications** folder.
14. On a new Mac, save your passwords again. [The list in Back up SparkBox](help:back-up-sparkbox#check-you-can-get-your-passwords-back) says where each one goes.

## What you should see

The top bar shows the data source you were using when you made the backup. In the sidebar, under **Tools**, click **Activity log**. [Open Activity log](open:activity) Your older entries are there.

Check these two, and not the sidebar's node list. A SparkBox that found no folder also lists `cell1`, `cell2`, `cell3` and `gx10-0000`. It starts with **Demo** data, and its activity log is empty or has only a few new entries.

## Why it works

SparkBox reads its folder when it starts, so it picks up the restored copy as its own. The copy holds your nodes, trusted host keys, settings and history, as they were when you made the backup.

Your passwords were never in the backup. They live only in the Keychain, which is why a new Mac needs them saved again.

## Limits

- SparkBox has no restore or import feature, so you put the folders back by hand in Finder.
- The `WebKit` folder belongs to the Mac's web engine. Its contents may not carry over to another Mac. If they don't, you lose only layouts and view choices, and you can set those again.
- The `.old` folders stay until you remove them. Keep them until you're sure you don't need them.

## If something goes wrong

- **SparkBox opens with Demo data, and your older activity is missing.** It didn't find the restored folder. Quit it, then check that the folder is in `~/Library/Application Support` and is named exactly `com.nemecj.sparkbox`.
- **"Couldn't read inventory.json. SparkBox started with defaults and kept the old file beside it."** SparkBox couldn't read that file, so it renamed it `inventory.json.corrupt-` and a time. Click **Show in Finder** to see it. Quit SparkBox, then copy `inventory.json` from your backup's `com.nemecj.sparkbox` folder into that folder. If Finder asks, click **Replace**.
- **macOS asks whether SparkBox may use your Keychain.** After a restore or on a new Mac, macOS may ask. Click **Always Allow** if you recognize the request.
- **Live data shows every node as Error after a restore.** The cluster password isn't saved on this Mac yet. [Save the cluster password](help:save-the-cluster-password) shows how.
- **You want to go back to how it was.** Quit SparkBox. In `~/Library/Application Support`, rename `com.nemecj.sparkbox` to `com.nemecj.sparkbox.restored`, then rename `com.nemecj.sparkbox.old` back to `com.nemecj.sparkbox`. Do the same in `~/Library/WebKit` if you restored that folder too.

## Next steps

- [Save the cluster password](help:save-the-cluster-password), to save it on a new Mac
- [Check that your nodes are ready](help:check-your-nodes), to prove the connection works
- [Where SparkBox keeps things](help:files-and-folders), to see every file in these folders
