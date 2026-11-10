---
id: copy-files
title: Copy files between your Mac and a node
type: howto
area: tools
summary: Move files between This Mac and a node in the two-pane Files window, and choose what happens to name clashes.
keywords: [upload, download file, transfer, file transfer, sftp, scp, download to my mac, copy to my mac, move files, drag and drop, name clash, conflict]
verified: false
related: [files, delete-and-restore-files, copy-a-model-to-other-nodes]
---

# Copy files between your Mac and a node

To copy files between this Mac and a node, open both places in the **Files** window and copy from one pane to the other.

## Before you start

- A copy onto a node needs [Admin mode](help:admin-mode).
- A copy onto this Mac needs Admin mode with Touch ID or your Mac password. There's one exception: a copy from a node into your `~/Downloads` folder, under names that aren't taken yet, needs no unlock.
- A move also needs the unlock for the place it removes files from.
- In look-only mode, every copy is held. [Use look-only mode](help:use-look-only-mode) explains how to turn it off.
- With Demo data, the nodes are simulated. You can try a copy onto a Demo node, but a copy from a Demo node to this Mac is refused.
- The place you're copying to needs enough free space. The bottom of each pane shows how much it has.
- To spread a model to your other nodes, don't use **Files**. [Copy a model to other nodes](help:copy-a-model-to-other-nodes) uses the fast fabric.

## Steps

### Copy a file

1. In the sidebar, under **Tools**, click **Files**. [Open Files](open:files) The window opens with **This Mac** in the left pane and your first node in the right pane.
2. In the right pane, click the list at the top left and choose the node you want, such as `cell2`. The pane shows that node's home folder.
3. In the pane you're copying to, open the destination folder. Double-click a folder to open it, or click a shortcut under the path, such as **Downloads** or **models**.
4. In the other pane, open the folder that holds your file.
5. Click the file. To choose several, hold <kbd>⌘</kbd> and click each one. The bottom of the pane shows how many you selected and their size.
6. In the toolbar, click **Copy to cell2**. The button always names the other pane. <kbd>F5</kbd> does the same.
7. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
8. If a name is already taken, a dialog opens. See [Choose what happens to a name clash](#choose-what-happens-to-a-name-clash).
9. Watch the **Transfers** list under the panes. It shows the amount copied, the speed and the time left.

### Other ways to copy

- **Drag** selected items from one pane to the other. Drop them on a folder to copy them into it.
- **Drag from Finder** into a pane. That always copies.
- Press <kbd>⌘C</kbd> in one pane, click in the other pane, and press <kbd>⌘V</kbd>. The copy lands in the folder that pane is showing.
- To **move** instead of copy, click **Move to cell2**, press <kbd>F6</kbd>, or hold <kbd>⌥</kbd> while you drag.
- To copy from **one node to another**, show a node in each pane and copy as usual. The data passes through this Mac, and **Transfers** says "via this Mac".

### Choose what happens to a name clash

If something with the same name is already there, the dialog lists each item with its size and date on both sides. **Newer** marks the more recent one.

> [!DANGER]
> **Replace, and Merge for files with the same name, remove the old copy for good.** It goes only after the new one has arrived, but it doesn't go to the Trash, and you can't undo it. **Replace if newer** does the same to files older than yours. **Merge** is already chosen for a folder, so look at the **Action** before you click **Continue**. **Keep both** is the safe choice: compare the two, then delete the one you don't want.

1. Read the row. **Incoming** is what you're copying, and **There now** is what's already there.
2. In **Action**, choose what to do with it. If there's more than one item, **Apply to all** sets them all at once. **Merge** works only on folders, so with **Apply to all** it sets any file to **Replace**.
3. Click **Continue**. To call the copy off, click **Cancel copy**.

| Action | What happens | Offered for |
|---|---|---|
| **Keep both** | Copies yours alongside as "name 2". This is the choice for a file unless you change it | Files and folders |
| **Merge** | Adds to the folder that's there. Files with the same name are replaced. This is the choice for a folder unless you change it | Two folders |
| **Replace** | Swaps the old item out whole. A replaced folder loses any files the new one doesn't have | Files and folders |
| **Replace if newer** | Replaces only the files older than yours | Files and folders |
| **Skip** | Leaves the old item alone and doesn't copy this one | Files and folders |

## What you should see

While it runs, the **Transfers** row shows a bar and a line such as "1.2 GiB of 4.0 GiB · 312 of 1,024 items · 410 MB/s · 7s left".

When it ends, the row shows a checkmark and a line such as "Copied notes.txt (4.1 KiB) in 3s." The destination pane refreshes, and the item appears there. The same copy is listed in [Tasks](help:tasks).

## Why it works

SparkBox checks sizes, free space and names already taken, then sends everything over SSH into a hidden staging folder at the destination. Nothing is put in place until both ends report success and every file's size matches. So a copy that fails or stops leaves the destination as it was. A move removes the originals only after that check. A copy into `~/Downloads` needs no unlock, because macOS marks each file as downloaded and asks before anything in it runs.

## Limits

- At most two copies run at once on each route, such as this Mac to `cell1`. Others wait with "Queued (two at a time per route)".
- On this Mac, copies can land only in your home folder or on a volume in `/Volumes`.
- On this Mac, SparkBox never copies into login items, shell startup files or `~/.ssh`. This check doesn't apply on a node.
- SparkBox refuses to copy or move a path that may hold keys or passwords, such as `~/.ssh` or `~/.aws`, on this Mac or on a node. On this Mac that includes a folder with one inside, such as your whole home folder. **Copy to cell2** and **Move to cell2** are dimmed for it, with no Touch ID prompt, because the copy would be refused whatever you unlock.
- Closing **Files** doesn't stop a copy. It asks first, then the copy goes on, and **Tasks** shows it.
- Copies aren't resumed. If a copy stops, **Retry** starts it again from the beginning.

## If something goes wrong

- **"Not enough space on cell1: this needs 12.0 GiB and 3.4 GiB is free."** Nothing was copied. Free up space, or choose another folder. [Check disk space on a node](help:check-disk-space) shows how.
- **"Demo nodes are simulated: their files can't be copied to this Mac."** Demo nodes have no real files. This message applies only with Demo data.
- **"Copy into a folder in your home folder or on a volume in /Volumes."** That folder on this Mac isn't allowed. Choose one in your home folder.
- **"SparkBox doesn't copy into login items, shell startup files or ~/.ssh."** Those places decide what runs on your Mac, so SparkBox never writes there.
- **"SparkBox won't copy a folder that may hold keys or credentials."** The item is a key or password file, or holds one. It appears as a message named **Not copied** or **Not moved**, and as the tooltip on the dimmed button. Choose something else.
- **"SparkBox won't measure, preview or list versions of a path that may hold keys or credentials."** The same check, when SparkBox reads the item. Choose something else.
- **"notes.txt can't be copied into itself."** The destination is inside the item you're copying. Choose another folder.
- **"… arrived as 1.0 GiB, expected 1.2 GiB (it may have changed while it was copied); nothing was put in place."** The file changed during the copy, for example because something was still writing it. Wait for that to finish, then click **Retry**.
- **"Left a 1.2 GiB staging folder on cell2".** The node went away before SparkBox could tidy up. SparkBox removes the folder by itself once the node answers and [Admin mode](help:admin-mode) is unlocked. To do it now, click **Remove**. The folder takes up space on the node until then, and it doesn't affect your files.
- **"Stop copying models.tar?"** You clicked **Stop** on a copy over 1 GiB. Stopping removes what had arrived and changes nothing at the destination.
- **"Demo copies don't carry the contents of files on your Mac."** You opened a file that you copied from this Mac to a Demo node. Demo keeps its name and size but never reads what's in it.
- **You see only one pane.** The window is narrower than 720 points, so **Files** shows one pane at a time. Use the switcher at the top to change panes, or make the window wider.

More messages are in [Files and Terminal messages](help:errors-files-and-terminal).

## Next steps

- [Files](help:files), to see every part of the window
- [Delete and restore files](help:delete-and-restore-files), to use the Trash and Undo
- [Edit a file on a node](help:edit-a-file), to change a text file without copying it to this Mac
