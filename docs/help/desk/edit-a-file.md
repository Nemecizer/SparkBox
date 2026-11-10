---
id: edit-a-file
title: Edit a file on a node
type: howto
area: tools
summary: Open a text file on a node, change it, check the difference and save it safely.
keywords: [edit, editor, config file, yaml, netplan, save with sudo, diff]
verified: false
related: [editor, read-a-root-only-file, delete-and-restore-files]
---

# Edit a file on a node

To change a text file on a node, open it in the **Editor**, make your change, review it and save.

## Before you start

- Reading a file needs nothing. Changing it needs [Admin mode](help:admin-mode).
- A file that belongs to root, the account that can do anything on a node, is saved with **sudo**. Sudo runs one command as root, and it uses the cluster password saved in your Keychain.
- The Editor works on text files up to 2 MB, in UTF-8, the usual text format. A bigger file opens read-only.
- A file on this Mac has its own unlock. Saving one asks for Touch ID or your Mac password if you haven't confirmed in the last 5 minutes, even when Admin mode is unlocked for your nodes.
- With Demo data, the nodes are simulated, so a save changes only the simulated file. **New file** isn't available on Demo nodes.
- In look-only mode, nothing can be edited.

> [!DANGER]
> **A wrong setting in a system file can cut a node off.** Files such as those in `/etc/netplan` decide how a node reaches the network. A mistake may not show until the next restart, and you may then have to fix it at the machine itself. Change one thing at a time, and read the review before you save.

## Steps

1. In the sidebar, under **Tools**, click **Files**. [Open Files](open:files) The **Files** window opens with two panes.
2. In the right pane, click the list at the top left and choose your node, such as `cell1`.
3. Open the file's folder. Double-click folders, or click a shortcut such as **netplan**, or click the path and type it.
4. Double-click the file. The Editor opens beside **Files**, with the file's name and node in its title, such as "hosts — cell1".
5. If you see **Read-only · Unlock to edit**, click **Unlock…** in that bar. The **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
6. Make your change. A dot appears in the window's title, and the bottom right of the window says **Edited**.
7. Press <kbd>⌘S</kbd>, or click **Save** in the toolbar. For a root-owned file, the button says **Save with sudo**.
8. If the **Review changes** dialog opens, read it. Lines marked with − are on disk now. Lines marked with + are your version.
9. To go ahead, click **Save** or **Save with sudo**. To go back to the file, click **Cancel**.

SparkBox shows the review for every file it saves with sudo, and for YAML or JSON that doesn't parse. Other saves happen at once.


## What you should see

A message says "Saved hosts" and names the node. For a root-owned file it adds "with sudo, owner and permissions kept".

The dot leaves the title, and the bottom right of the window says **Saved** with the time. The file on the node now has your text. Its owner and permissions haven't changed.

## Why it works

SparkBox doesn't write over the file directly. It writes the new text to a temporary file beside it, checks that file, then swaps it into place in one step. If anything goes wrong, the original is untouched. SparkBox also checks that nobody changed the file while you were typing, so it can't overwrite their work. It keeps the last five versions you replaced, and [Delete and restore files](help:delete-and-restore-files#bring-back-an-earlier-version) shows how to bring one back.

## Limits

- Files over 2 MB open read-only, showing the first and last 64 KB.
- The Editor doesn't open binary files. Double-clicking a model file or an archive shows its details instead.
- The Editor reads UTF-8 only. A file in another text format doesn't open.
- Saving keeps the file's line endings, and the status line shows them, such as "UTF-8 · CRLF". A file that reads "UTF-8 · Mixed line endings" saves with all LF endings, as its tooltip says.

## If something goes wrong

- **"hosts changed on disk after you opened it (modified Today at 2:14 PM). Your changes aren't saved yet."** Someone or something changed the file. Click **Show diff** to compare, **Reload** to take the version on disk and lose your changes, or **Overwrite** to save yours over it. **Keep editing** hides the bar.
- **"hosts was deleted on cell1 after you opened it."** The file is gone from the node. Click **Overwrite** to save your text as a new file, or **Reload** to give up.
- **"This YAML doesn't parse (1 error, underlined in the editor)."** Your text has a mistake. Click **Keep editing** and fix the underlined line. For a file in `/etc/netplan`, the dialog also warns: "netplan won't apply it, and the node could lose these links at the next boot." **Save anyway** saves it as it is, so use it only if you know why it doesn't parse.
- **"The cluster user can't write /etc/hosts on cell1. Save it with sudo instead."** The file needs root. SparkBox shows the **Review changes** dialog so that you can save with sudo.
- **"sudo couldn't save /etc/hosts on cell1: …"** Sudo refused or failed, and the end of the message says why. A wrong saved password is the usual cause. [Save the cluster password](help:save-the-cluster-password) shows how to update it.
- **"Only root can read 40-cx7.yaml (root, 600)"** You need to read it with sudo first. See [Read a file only root can read](help:read-a-root-only-file).
- **"/home/sparkuser/model.bin looks like a binary file"** or **"… isn't UTF-8 text"**. The Editor can't show that file. Use **Terminal** if you need to look inside it.

You can close the Editor with unsaved changes safely. **Do you want to save the changes to hosts?** opens, with **Don't Save**, **Cancel** and **Save**.

More messages are in [Files and Terminal messages](help:errors-files-and-terminal).

## Next steps

- [Editor](help:editor), to see every part of the window, including find and replace
- [Delete and restore files](help:delete-and-restore-files), to bring back an earlier version
- [Find out what SparkBox did](help:find-what-sparkbox-did), to see the save in the activity log
