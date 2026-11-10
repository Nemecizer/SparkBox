---
id: back-up-sparkbox
title: Back up SparkBox
type: howto
area: settings
summary: Keep a copy of SparkBox's settings by copying its folders, and check you can get your passwords back.
keywords: [backup, save settings, move to a new mac, export, copy settings, keep a copy]
verified: null
related: [restore-sparkbox, reset-sparkbox, files-and-folders, keychain-and-secrets]
---

# Back up SparkBox

To keep a copy of SparkBox's settings, quit it and copy its folders to a safe place with Finder.

## Before you start

- You need a disk or folder to copy to, outside SparkBox's own folders.
- A backup covers this Mac only. It doesn't include anything on your nodes, such as models or containers.
- A backup doesn't include your passwords and keys. They stay in the Keychain. See [Check you can get your passwords back](#check-you-can-get-your-passwords-back).
- The copy holds your cluster's addresses, your activity log and your **Chat** conversations. Keep it somewhere private.
- It takes a few minutes.

## Steps

### Quit SparkBox

Copy only while SparkBox is closed, because it writes to these folders all the time. Closing the window isn't enough, since SparkBox keeps running in the menu bar.

Quitting ends your terminal sessions, stops the **Server**, and stops SparkBox watching the cluster from this Mac. Models on your nodes keep running. Wait for any copy in **Files** to finish first.

1. Click the SparkBox icon in the macOS menu bar. Its menu opens.
2. Click **Quit SparkBox**. The icon leaves the menu bar.

### Make a place for the backup

3. In Finder, open the disk or folder you're backing up to.
4. Make a new folder, and name it with today's date, such as `SparkBox backup 2026-10-31`.
5. Inside it, make two folders named `Application Support` and `WebKit`. The two SparkBox folders you're about to copy share one name, so each needs its own place.

### Copy the folders

| Folder on this Mac | What it holds | Copy it? |
|---|---|---|
| `~/Library/Application Support/com.nemecj.sparkbox` | Your nodes and switches, trusted host keys, settings, activity log, tasks, chats, alert history, chart history, and the versions of files you edited on this Mac | Yes. This is the one that matters |
| `~/Library/WebKit/com.nemecj.sparkbox` | Your window layouts, appearance, Terminal settings, table columns and **Files** favorites | Yes, if you want to keep them |
| `~/Library/Logs/SparkBox` | SparkBox's logs and your evidence captures | Only if you want to keep them |
| `~/Library/Application Support/SparkBox Fence` | The Fence, which applies to the test build only | No |

The `~` means your home folder. Your **Library** folder is hidden in Finder, so go there by typing its path.

6. In Finder, press <kbd>⇧⌘G</kbd>. The **Go to Folder** box opens.
7. Type `~/Library/Application Support`, then press <kbd>Return</kbd>. Finder shows that folder.
8. Hold <kbd>⌥</kbd> and drag `com.nemecj.sparkbox` into the backup's `Application Support` folder. Finder copies it and leaves the original where it is.
9. Press <kbd>⇧⌘G</kbd> again, type `~/Library/WebKit`, then press <kbd>Return</kbd>.
10. Hold <kbd>⌥</kbd> and drag `com.nemecj.sparkbox` into the backup's `WebKit` folder.

### Open SparkBox again

11. Open SparkBox from your **Applications** folder. It opens as you left it.

### Check you can get your passwords back

Passwords and keys live only in the macOS [Keychain](help:keychain-and-secrets), never in these folders. On this Mac they stay where they are. A new Mac won't get them from this backup, so make sure you can get each one again.

| What | Where you save it in SparkBox | Where it comes from |
|---|---|---|
| **Cluster password** | **Settings › Cluster login** | The password of user `sparkuser` on your nodes |
| Key passphrase, if you use one | **Settings › Cluster login** | You set it when you made the key |
| **REST password** | **Settings › Switches & gateway** | The account on your CRS804 DDQ |
| Hugging Face token | **Settings › Models** | Your Hugging Face account. You can make a new one |
| API keys | **Providers** | Each provider's website. You can make new ones |
| VPN keys | **VPN**, by importing the profile | Your Firewalla app |
| Notification secrets | **Settings › Notifications** | ntfy, Pushover, your webhook or your email service |
| Server access keys | **Server** | SparkBox makes them. Make new ones, and put them in your apps again |

SparkBox also keeps a few keys it made itself, such as the API key of a load. To replace one, load the model again.

> [!NOTE]
> Keep passwords in a password manager, not in a file, a note or a chat. If one ends up somewhere it shouldn't, make a new one from the list above.

## What you should see

Your backup folder holds `Application Support/com.nemecj.sparkbox` and `WebKit/com.nemecj.sparkbox`, and Finder shows their sizes. SparkBox opens as you left it, and its originals are unchanged.

## Why it works

SparkBox keeps what it knows about your cluster as plain files in one folder on this Mac. Copying that folder while SparkBox is closed gives you a complete, consistent copy. Window layouts and appearance live in the storage of the web view that draws SparkBox's windows, which macOS keeps in the `WebKit` folder.

Passwords are left out on purpose. They live only in the Keychain, so a stray copy of the folder gives nobody a way into your cluster.

## Limits

- SparkBox has no backup, export or import feature, so you make the copy by hand in Finder.
- SparkBox doesn't back itself up on a schedule. Time Machine may include these folders, but SparkBox doesn't check.
- The `WebKit` folder belongs to the Mac's web engine. Its contents may not carry over to another Mac. If they don't, you lose only layouts and view choices, and you can set those again.
- A folder ending in `.dev` belongs to the test build. You don't need it.

## If something goes wrong

- **You can't find the Library folder.** It's hidden in Finder. Use <kbd>⇧⌘G</kbd> and type the whole path.
- **You don't see `com.nemecj.sparkbox`.** Finder is showing another folder. Press <kbd>⇧⌘G</kbd> and type the whole path again, with the space in `Application Support`.

## Next steps

- [Restore SparkBox from a backup](help:restore-sparkbox), to put a backup in place
- [Where SparkBox keeps things](help:files-and-folders), to see every file in these folders
- [Reset SparkBox](help:reset-sparkbox), to start clean, which works best after a backup
- [Where your passwords and keys go](help:keychain-and-secrets), to see how passwords are kept
