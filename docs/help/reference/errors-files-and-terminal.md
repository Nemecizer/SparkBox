---
id: errors-files-and-terminal
title: Files and Terminal messages
type: troubleshooting
area: troubleshooting
summary: Messages from copying, editing and deleting files, and from terminal sessions.
keywords: [permission, locked shell, can't write, won't delete, sudo save, binary file, changed on disk, tar, trash, authenticate]
verified: null
related: [copy-files, edit-a-file, open-a-terminal, read-a-root-only-file, delete-and-restore-files]
---

# Files and Terminal messages

This page covers messages from **Files**, the **Editor** and **Terminal**.

Files on a node need Admin mode to change. Files on this Mac need Touch ID or your Mac password. See [Admin mode](help:admin-mode).

## Reading a file

### "Only root can read shadow (root, -rw-r-----). Reading it needs sudo, which needs Admin."

**What it means.** Your user can't read this file. The brackets give its owner and permissions.

**What to do.** Follow [Read a file only root can read](help:read-a-root-only-file).

### "Permission denied"

**What it means.** The system refused your user. It's the same for reading a file, listing a folder and changing a file.

**What to do.** To read, use **Read with sudo…**. To save, see ["The cluster user can't write /etc/hosts on cell2. Save it with sudo instead."](#the-cluster-user-cant-write-etchosts-on-cell2-save-it-with-sudo-instead)

### "/etc/hosts: No such file or directory"

**What it means.** The file has gone, or the name is wrong.

**What to do.** Go back to the folder, and refresh it.

### "model.safetensors looks like a binary file"

**What it means.** The **Editor** opens text only. You may also see:

- "… isn't UTF-8 text", which means the same
- "… is a folder", which means you opened a folder

**What to do.** Use **Get info**, or copy the file to this Mac and open it in another app.

### "Files on this Mac open as you; there's nothing to read with sudo."

**What it means.** **Read with sudo…** is for files on a node.

**What to do.** Open the file the normal way.

### "SparkBox won't copy a folder that may hold keys or credentials."

**What it means.** The item is a key or password file, or a folder with one inside. SparkBox refuses it before it asks for anything, so you see no Touch ID prompt. It appears as a message named **Not copied** or **Not moved**, and as the tooltip on the dimmed button.

**What to do.** Choose something else. See [Copy files between your Mac and a node](help:copy-files).

### "SparkBox won't measure, preview or list versions of a path that may hold keys or credentials."

**What it means.** The path is one of the places on this Mac where keys are kept, such as `~/.ssh` or the Keychains folder.

**What to do.** Leave it alone, or use Finder if you need it.

## Saving a file

### "/etc/hosts on cell2 changed on disk after you opened it."

**What it means.** Something else changed the file while you were editing. SparkBox won't overwrite that change.

**What to do.** Copy your edits somewhere safe, close the file, open it again, and put your edits back.

### "The cluster user can't write /etc/hosts on cell2. Save it with sudo instead."

**What it means.** Your user isn't allowed to change this file. "/etc/hosts belongs to someone else; save it with sudo to keep its owner." is the same.

**What to do.** Use the save with sudo. SparkBox shows your changes first and keeps the file's owner and permissions. See [Edit a file on a node](help:edit-a-file).

### "The new text arrived incomplete (1024 bytes), so notes.txt wasn't changed."

**What it means.** The connection dropped during the save. The file on the node is as it was.

**What to do.** Save again.

### "visudo rejected the new /etc/sudoers, so it wasn't saved: …"

**What it means.** The file has a mistake, and SparkBox checked it before saving. A broken sudoers file can stop sudo from working.

**What to do.** Fix the line the message names, and save again.

### "couldn't stage the file on cell2: …"

**What it means.** A step of the save failed. The file wasn't changed.

**What to do.** Check that the node has free disk, then save again.

### "There's already something called notes.txt there."

**What it means.** A file or folder already has that name. "/home/sparkuser/notes.txt already exists." is the same.

**What to do.** Pick another name.

### "“a/b” isn't a usable file name."

**What it means.** The name has a character a file name can't have.

**What to do.** Use another name.

### "Can't create notes.txt: the folder /home/sparkuser/new doesn't exist."

**What it means.** The folder has gone.

**What to do.** Go to a folder that exists.

### "Permissions are three or four octal digits, e.g. 644."

**What it means.** The value isn't in the right form.

**What to do.** Type three or four digits. `644` lets you read and write, and others read.

## Copying

### "Choose a folder first."

**What it means.** The copy has nowhere to go. "Nothing to copy." means nothing is selected.

**What to do.** Open the target folder in the other pane, and select what you want to copy.

### "models can't be copied into itself."

**What it means.** The target is inside the folder you're copying.

**What to do.** Pick another target.

### "3 of 10 items didn't arrive on cell2 (first: config.json); nothing was put in place."

**What it means.** SparkBox checks a copy before it puts it in place. This one wasn't complete, so the target stays as it was. You may also see:

- "… arrived as a different kind of item"
- "… arrived as 100 bytes, expected 200 (it may have changed while it was copied)"
- "4 arrived, 5 were sent"

**What to do.** Copy again. If a file was being written, wait for that to finish.

### "tar failed on cell2: …"

**What it means.** The tool that packs files for a copy gave an error. You may also see:

- "unpacking on cell2 failed: …", which comes from the receiving side
- "The receiving side (cell2) closed the connection", which means the link dropped
- "The sending side (cell2) ended without reporting success", which means the link dropped

**What to do.** Read the reason. Check free disk on the target, then copy again.

### "notes.txt is no longer in /home/sparkuser."

**What it means.** The file moved or was deleted after you picked it.

**What to do.** Refresh the folder.

### "SparkBox doesn't copy into login items, shell startup files or ~/.ssh."

**What it means.** Those places can run code or grant access, so SparkBox won't put files there. This protects this Mac.

**What to do.** "Copy into a folder in your home folder or on a volume in /Volumes."

### "Demo nodes are simulated: their files can't be copied to this Mac."

**What it means.** You're using Demo data. "Demo copies don't carry the contents of files on your Mac." is the other direction.

**What to do.** Switch to Live data for real copies.

## Deleting

### "SparkBox won't delete /home."

**What it means.** That's a system folder, or the home folder itself, so SparkBox refuses. "SparkBox won't delete … : it isn't an absolute path." is a safety check too.

**What to do.** Delete the items inside it instead.

### "SparkBox won't put ~/.local itself in the Trash; delete what's in it instead."

**What it means.** That folder holds the Trash itself.

**What to do.** Do what the message says.

### "notes.txt is already in the Trash; Delete removes it for good."

**What it means.** You're looking at the Trash.

**What to do.** Use **Delete immediately…** only if you're sure. It can't be undone.

### "notes.txt already exists, so the trashed copy was left in the Trash."

**What it means.** You put something back, and a file with that name has appeared.

**What to do.** Rename the new file, and put the trashed one back again. See [Delete and restore files](help:delete-and-restore-files).

### "SparkBox can't tell where notes.txt came from."

**What it means.** The Trash has no record of where it was. On this Mac, the same problem reads "SparkBox doesn't know where that item went; use Finder's Put Back."

**What to do.** Move it back by hand.

### "SparkBox won't change notes.txt: ~/sparkbox-test is a link."

**What it means.** This message concerns the test build's practice folder.

**What to do.** Nothing, in the SparkBox app.

## Terminal

### "Locked — authenticate to keep typing. Keystrokes typed while locked aren't sent."

**What it means.** A shell on a node can change that node, so shells lock with Admin mode. Nothing you type reaches the node until you confirm. The session itself stays open.

**What to do.** Click **Authenticate…**, then confirm with Touch ID or your Mac password.

### "Admin mode locked while this shell was connecting. Authenticate to start typing."

**What it means.** Admin mode locked while this shell was opening. A shell on a node can change that node, so shells lock with Admin mode. Nothing you type reaches the node until you confirm.

**What to do.** Click **Authenticate…**, then confirm with Touch ID or your Mac password.

### "Locked: nothing typed for 60 min."

**What it means.** The shell went idle for your idle shell time. The message shows the time you set. A minute earlier, it warned: "This shell locks in 60 s: nothing typed for 59 min. Type anything to keep it open."

**What to do.** Click **Authenticate…**. To change the time, see [Terminal settings](help:settings-terminal#shells).

### "This tab has closed."

**What it means.** The session ended, and you typed in the tab.

**What to do.** Open a new tab with <kbd>⌘T</kbd>.

### "The connection to the node closed."

**What it means.** The link dropped.

**What to do.** Check that the node is online, then open a new tab. Anything that was running in that shell has usually stopped.

### "This tab is read-only."

**What it means.** A log tab shows output only.

**What to do.** Open a shell tab to type.

### "The terminal is busy; try again in a moment."

**What it means.** The tab is catching up with a lot of output.

**What to do.** Wait a moment.

### "That container or service name has characters SparkBox won't pass to a shell."

**What it means.** SparkBox won't build a command with those characters.

**What to do.** Check the name.
