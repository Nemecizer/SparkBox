---
id: read-a-root-only-file
title: Read a file only root can read
type: howto
area: tools
summary: Open a protected file with Read with sudo, which uses SparkBox's own checked root helper.
keywords: [sudo, root, permission denied, read with sudo, protected, netplan]
verified: false
related: [edit-a-file, ssh-sudo-and-host-keys, editor]
---

# Read a file only root can read

To read a file that only root can open, open it in the **Editor** and click **Read with sudo…**.

## Before you start

**Root** is the account that can do anything on a node. Some files are readable by root alone, such as the network settings in `/etc/netplan`. **Sudo** lets your own account run one command as root.

- Be ready to unlock Admin mode, with look-only mode off. With Live data, SparkBox asks for Touch ID or your Mac password. [Use look-only mode](help:use-look-only-mode) explains how to turn look-only mode off.
- The [cluster password](help:save-the-cluster-password) must be saved, because sudo asks for it.
- This works only on nodes. Files on this Mac open as you.
- With Demo data, you can try it on a simulated file, such as `/etc/netplan/50-cloud-init.yaml` on `cell1`. It needs no password.

Reading doesn't change the file. Validate the results on your own equipment before relying on them.

## Steps

1. In the sidebar, under **Tools**, click **Files**. [Open Files](open:files) The **Files** window opens with two panes.
2. In the right pane, click the list at the top left and choose your node, such as `cell2`.
3. Click the **netplan** shortcut under the path, or open the folder that holds your file.
4. Double-click the file. The Editor opens and says who can read it, such as "Only root can read 40-cx7.yaml (root, 600)". The word in brackets is the file's owner, and 600 means that only the owner can read or write it.
5. Click **Read with sudo…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
6. Wait while the button says **Reading…**. The file's text appears in the Editor.


## What you should see

The file's text fills the Editor. A bar above it says "40-cx7.yaml belongs to root. Saving uses sudo, keeps its owner and permissions, and shows you the changes first."

You're only reading. To change the file, follow [Edit a file on a node](help:edit-a-file). Saving it goes through **Save with sudo**.

## Why it works

Your usual account can't open the file, so **Read with sudo…** runs SparkBox's small **root helper** as root. The helper lives at `/usr/local/lib/sparkbox/sparkjob.py`, in a folder only root can change. Before each use, SparkBox checks its fingerprint, and puts its own copy there if it's missing or differs. So your first read on a node leaves that one file on the node. The cluster password goes to sudo privately, never on a command line.

## If something goes wrong

- **The Read with sudo… button is dimmed.** Look-only mode is on, or the file is on this Mac. Files on this Mac don't need it.
- **"Admin mode stays locked".** Touch ID or your Mac password didn't go through. Try again.
- **sudo refuses the password.** The saved cluster password is out of date. [Save the cluster password](help:save-the-cluster-password) shows how to update it, and [Admin, look-only and safety messages](help:errors-admin-and-safety) lists the exact wording.
- **"The copy of sparkjob.py arrived damaged; nothing was replaced."** The helper didn't reach the node intact. Nothing changed. Try again, and check the node's network if it happens twice.
- **The file is over 2 MB.** It opens read-only, showing the first and last 64 KB.

More messages are in [Files and Terminal messages](help:errors-files-and-terminal).

## Next steps

- [Edit a file on a node](help:edit-a-file), to save a change to the file
- [SSH, sudo and host keys](help:ssh-sudo-and-host-keys), to learn what sudo is and how SparkBox uses it
- [Find out what SparkBox did](help:find-what-sparkbox-did), to see the read in the activity log
