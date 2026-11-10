---
id: editor
title: Editor
type: feature
area: tools
summary: A text editor for files on your nodes and this Mac, with safe saving, diffs and previous versions.
keywords: [editor, save, diff, previous versions, find and replace, go to line, yaml, json, save with sudo, revert]
verified: false
related: [edit-a-file, read-a-root-only-file, delete-and-restore-files]
---

# Editor

## What it's for

The **Editor** opens one text file, on this Mac or on a node, in a window of its own. Use it for configuration files, scripts and notes. It's careful with your files: it shows you what will change, it won't overwrite a change someone else made, and it keeps the last five versions.

## Where to find it

- **Files window:** double-click a text file.
- **Files toolbar:** select a file and click **Edit**.
- **Right-click:** right-click a file and click **Edit**.
- **New file:** click **New file**, which makes an empty file and opens it.

The Editor opens beside **Files**. If there's no room, **Files** takes the left half of the desk and the Editor takes the right half. When the Editor closes, **Files** goes back to its own size.

There's one Editor window for each file. The title shows the file and the place, such as "hosts — cell1", with a dot in front while you have unsaved changes.

## What's on the screen


### Toolbar

| Button | What it does |
|---|---|
| **Save** or **Save with sudo** | Saves your changes. <kbd>⌘S</kbd> does the same. **Save with sudo** appears for a file that needs root |
| **Revert** | Throws away unsaved changes and reads the file again |
| **Reload** | Reads the file from disk again, when you have no unsaved changes |
| **Find** | Opens find and replace |
| **Wrap** | Wraps long lines to the window. SparkBox remembers your choice |
| **Previous versions** | Lists earlier versions of the file. See [Previous versions](#previous-versions) |

On the right, the toolbar shows where the file is, such as "cell1 · /etc/hosts". In a narrow window, some buttons lose their labels or move into **More**.

### Read-only

While Admin mode is locked, you can read the file but not type in it. A bar says **Read-only · Unlock to edit**. Click **Unlock…** to edit it. In look-only mode, the bar says **Read-only · look-only mode**, and there's no button.

A file over 2 MB always opens read-only. A note gives its size, and the Editor shows the first and last 64 KB, with a line saying how much is left out.

### The text

Lines are numbered, and the Editor colors text for the kind of file: **YAML**, **JSON**, **Python**, **Shell**, **INI**, **TOML** or **Plain text**. It picks the kind from the file's name.

In YAML and JSON, a mistake is underlined as you type, with a mark in the margin. That helps with `netplan` files, which a node refuses if they don't parse.

### Find and replace

| Keys | What they do |
|---|---|
| <kbd>⌘F</kbd> | Find |
| <kbd>⌥⌘F</kbd> | Find and replace |
| <kbd>⌘L</kbd> or <kbd>⌃G</kbd> | Go to a line |

### Status line

The line at the bottom shows, from left to right:

| Part | Meaning |
|---|---|
| "Ln 3, Col 14" | Where the cursor is |
| **YAML** | The kind of file. Click it to choose another |
| "UTF-8 · LF" | The text format and the file's line endings: **LF**, **CRLF**, **CR** or **Mixed line endings**. "UTF-8 with BOM" means the file starts with a byte-order mark. Saving keeps the file's line endings. With **Mixed line endings**, saving makes them all LF |
| `-rw-r--r-- root:root` | The file's permissions, then its owner and group |
| **No changes**, **Edited**, **Saving…**, **Saved at 2:14 PM** or **Read-only** | Where your work stands |

### Saving with sudo

A file that belongs to root, or that your account can't write, shows a bar: "hosts belongs to root. Saving uses sudo, keeps its owner and permissions, and shows you the changes first."

Sudo runs one command as root, using the saved cluster password. The file keeps its owner and permissions.

### Review changes

Before a save with sudo, or a save of YAML or JSON that doesn't parse, the **Review changes** dialog shows what will change.

- Lines marked **−** are on disk now, and lines marked **+** are your version.
- The top of the dialog counts the lines added and removed.
- If the text doesn't parse, a warning says so. The button then says **Save anyway**, and **Keep editing** is the safe choice.
- Under the changes, a line says how SparkBox saves the file.

### If the file changed while you worked

If the file on disk changed after you opened it, a save stops, and a bar says so: "hosts changed on disk after you opened it (modified Today at 2:14 PM). Your changes aren't saved yet."

| Button | What it does |
|---|---|
| **Reload** | Takes the version on disk. Your changes are lost |
| **Show diff** | Shows what's on disk beside your version, with **Keep editing**, **Reload theirs** and **Overwrite with mine** |
| **Overwrite** | Saves your version over the other one |
| **Keep editing** | Hides the bar |

If the file was deleted, the bar says so, and there's no **Show diff**.

### Previous versions

**Previous versions** lists the versions SparkBox kept, newest first, with the date and size. The first says "(before the last save)".

Click one to load it. It replaces the file only when you save, so you can read it first. **Revert** cancels it. SparkBox keeps the last five versions of each file you save here. Saving a previous version keeps the one it replaces.

A file changed outside SparkBox, or never saved here, has none yet: the menu says "No previous versions yet".

### Closing with unsaved changes

Closing asks **Do you want to save the changes to hosts?**, with **Don't Save**, **Cancel** and **Save**. **Don't Save** loses your changes.

Minimizing the Editor keeps your text and your undo history. <kbd>⌘Z</kbd> in the text undoes your last edit.

### A file only root can read

If the node won't let your account read the file, the Editor says who can, such as "Only root can read 40-cx7.yaml (root, 600)". **Read with sudo…** reads it as root. See [Read a file only root can read](help:read-a-root-only-file).

## What it changes

| Action | What it changes | What it needs | How to undo it |
|---|---|---|---|
| Opening and reading | Nothing | Nothing | |
| Saving a file on a node | That file | Admin mode | **Previous versions** |
| Saving with sudo | That file, as root | Admin mode and the saved cluster password | **Previous versions** |
| Saving a file on this Mac | That file | Admin mode with Touch ID or your Mac password in the last 5 minutes, even when it's unlocked for your nodes | **Previous versions** |

A save is all or nothing. SparkBox writes the new text beside the file, checks it, and swaps it in. A `sudoers` file is also checked with `visudo`. If anything fails, the original stays as it was.

On a node, previous versions are in `~/.sparkbox/backups`. For this Mac, they're in `backups` in SparkBox's folder. Every save is in the [activity log](help:activity-log).

## Related How-Tos

- [Edit a file on a node](help:edit-a-file)
- [Read a file only root can read](help:read-a-root-only-file)
- [Delete and restore files](help:delete-and-restore-files)

![Editor, using synthetic Demo data](img/public-editor.jpg)
