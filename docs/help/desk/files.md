---
id: files
title: Files
type: feature
area: tools
summary: Two side-by-side panes, each showing This Mac or a node, for browsing, copying and managing files.
keywords: [files, transfers, trash, favourites, favorites, panes, hidden files, permissions, get info, calculate size]
verified: false
related: [copy-files, delete-and-restore-files, edit-a-file, files-and-folders]
---

# Files

## What it's for

**Files** shows two places side by side, so that you can see this Mac and a node together. Use it to look around a node, copy files in either direction, open a text file to edit, and tidy up.

For models, use [Models](help:models-window) instead. Its copies run over the fast fabric and check every file.

## Where to find it

- **Sidebar:** under **Tools**, click **Files**.
- **Menu bar:** choose **Go › Files**.
- **Keyboard:** press <kbd>⌘7</kbd>.
- **Command palette:** press <kbd>⌘K</kbd>, type `files`, and press <kbd>Return</kbd>.
- **Any node:** right-click a node anywhere, and click **Files**.
- **Menu bar icon:** click it, point to a node, and click **Files**.
- **Link:** [Open Files](open:files)

The window opens with **This Mac** on the left and your first node on the right.

## What's on the screen


### Panes

Each pane shows one place. The pane you clicked last is the active one, and the toolbar acts on it. <kbd>Tab</kbd> moves to the other pane.

Below 720 points wide, **Files** shows one pane at a time with a switcher above it. The arrow beside the switcher chooses the place.

### Location bar

| Control | What it does |
|---|---|
| Place list | Chooses **This Mac** or a node. A node that's offline says so, and the dot beside the list shows its state |
| Back and forward arrows | Go through the folders you visited. <kbd>⌘[</kbd> and <kbd>⌘]</kbd> do the same |
| Up arrow | Opens the folder above. <kbd>⌘↑</kbd> does the same |
| Path | Each folder is a button. `~` means your home folder. Click the last one, or the empty space after it, to type a path. If the path doesn't fit, **…** holds the middle folders |
| Star | Adds this folder to the shortcuts, or removes it |
| Refresh | Reads the folder again. <kbd>⌘R</kbd> refreshes both panes |

### Favorites

The row under the path holds shortcuts to folders. A node starts with **Home**, **models**, **huggingface**, **netplan** and **log**. This Mac starts with **Home** and **Downloads**. Right-click one for **Remove from favorites** and **Reset favorites**.

Each place has its own favorites, kept separately for Demo and Live data.

### The list

| Column | What it shows |
|---|---|
| **Name** | Folders come first. An **alias** is a link to something elsewhere |
| **Size** | A file's size. A folder shows **—** until you use **Calculate size** |
| **Modified** | When it last changed |
| **Permissions** | Who may read, write and run it, such as `-rw-r--r--` |
| **Owner** | The account that owns it |

In a narrow window, the last columns hide. [Lists, tables and right-click menus](help:tables-and-menus) explains how lists work.

To open a folder, double-click it. Double-clicking a text file opens it in the [Editor](help:editor). Double-clicking a model file, an archive or an image shows its details instead.

The bottom of each pane counts the items and says how many are hidden and selected, such as "3 items (6 hidden)" or "2 items · 1 selected (187 B)". On the right it shows the free space, such as "2.6 TiB free of 3.7 TiB".

### Toolbar

The buttons act on what's selected in the active pane.

| Button | What it does | Key |
|---|---|---|
| **Copy to cell1** | Copies into the folder showing in the other pane. Its name follows that pane | <kbd>F5</kbd> |
| **Move to cell1** | Moves there. The originals go after the copy is checked | <kbd>F6</kbd> |
| **New folder** | Makes a folder here | <kbd>⇧⌘N</kbd> |
| **New file** | Makes an empty file and opens it in the Editor. It isn't available on Demo nodes | |
| **Rename** | Renames the selected item | <kbd>F2</kbd> |
| **Edit** | Opens the file in the Editor | |
| **Move to Trash** | See [Delete and restore files](help:delete-and-restore-files). Hold <kbd>⌥</kbd> to change it to **Delete immediately** | <kbd>⌫</kbd> |
| **Terminal here** | Opens a shell in this folder, on a node | |
| **Hidden files** | Shows files whose names start with a dot | |
| **Swap** | Swaps the two panes | |

In a narrow window, some buttons lose their labels or move into **More**.

### Right-click menu

Right-click an item for **Open**, **Edit**, **Copy to**, **Move to**, **Duplicate**, **Rename…**, **Permissions…**, **Get info**, **Calculate size**, **Copy path**, **Open terminal here** and **Move to Trash**. Hold <kbd>⌥</kbd> as you right-click to get **Delete immediately…** instead.

Right-click empty space for **New folder**, **New file**, **Paste**, **Open terminal here**, **Copy path** and **Show hidden files**.

- **Duplicate** makes a copy beside the original, named "name 2". <kbd>⌘D</kbd> does the same.
- **Permissions…** changes who may read, write and run an item. It takes three or four digits: 644 is `rw-r--r--`, 755 is `rwxr-xr-x`, and 600 is for the owner only.
- **Calculate size** adds up what's in a folder. It can take a while on a big one.
- **Copy path** copies the item's full path.

### Get info

Select an item and press <kbd>Space</kbd>, or <kbd>⌘I</kbd>, to see its kind, size, path, date, permissions and owner. <kbd>Space</kbd> or <kbd>Esc</kbd> closes it.

### Transfers

The **Transfers** list sits under the panes once you've copied something. While copies run it's open. When nothing runs, it folds to one line, such as "Nothing running · 3 finished, 1 failed". Click **Transfers** to open it.

Each row shows what's going where and the route: "over SSH", "on the node", "on this Mac", or "via this Mac" for a copy between two nodes.

| State | What it means |
|---|---|
| "Checking sizes and names…" | SparkBox is measuring it first |
| "Waiting: some names are already taken" | It needs your choice. See [name clashes](help:copy-files#choose-what-happens-to-a-name-clash) |
| "Queued (two at a time per route)" | Two copies are already running on this route |
| A progress bar | Amount, items, speed and time left, and the file in flight |
| A checkmark, a warning or a stop mark | It finished, failed or was stopped. The row says what happened |

| Button | What it does |
|---|---|
| **Stop** | Stops it and removes what had arrived, from the hidden `.sparkbox-incoming-` folder it copies into. Nothing at the destination changes. Over 1 GiB, it asks first |
| **Retry** | Runs a failed or stopped copy again from the start, checking the destination afresh |
| **×** | Removes the row |
| **Clear history** | Removes every finished row |

SparkBox keeps the last 50 finished copies, separately for Demo and Live data. Each copy also shows in [Tasks](help:tasks). A copy keeps going if you close or minimize **Files**.

### Secret files

Some paths hold keys or passwords, such as `~/.ssh`, `~/.aws`, `~/.config/gh`, `~/.kube` and a saved Hugging Face token. To open or read one, on this Mac or a node, SparkBox needs Admin mode with Touch ID or your Mac password. It won't measure, copy or move them, and **Copy to cell1** and **Move to cell1** are dimmed for them. It also won't copy a folder on this Mac that has one inside.

### Demo data

Demo nodes have simulated files, and **This Mac** is always your real Mac, even with Demo data.

- You can copy onto a Demo node. It keeps only the name and size, never the contents.
- You can't copy from a Demo node to this Mac.
- **New file** isn't available on Demo nodes.

> [!WARNING]
> **Files on This Mac are real, even with Demo data.** A rename or a move to the Trash in that pane changes your Mac. Files in the Trash can be put back with **Undo** or from Finder.

## What it changes

| Action | What it changes | What it needs | How to undo it |
|---|---|---|---|
| Browsing, **Get info**, **Calculate size** | Nothing | Nothing | |
| Opening a secret file | Nothing | Touch ID or your Mac password | |
| Copy, move, new folder, new file, rename, permissions on a node | That node | Admin mode | Copy or rename back |
| The same on this Mac | This Mac | Admin mode with Touch ID or your Mac password | Copy or rename back |
| A copy from a node into `~/Downloads`, under new names | Adds files to this Mac, marked as downloaded | Nothing | Delete them |
| **Move to Trash** | Moves the item to that place's Trash | As above | **Undo**, for 30 seconds. Then see [Delete and restore files](help:delete-and-restore-files) |
| **Delete immediately** | Erases the item | As above, and a confirmation | It can't be undone |

Replacing an item or merging a folder while you copy is also a change at the destination. The old copy doesn't go to the Trash, so you can't undo it. [Choose what happens to a name clash](help:copy-files#choose-what-happens-to-a-name-clash) explains your choices. SparkBox asks for the unlock it needs once you've chosen. Every change is in the [activity log](help:activity-log). In look-only mode, you can browse and read, and SparkBox refuses any change you ask for.

SparkBox refuses to delete system folders, anything directly inside `/`, and your home folder.

## Related How-Tos

- [Copy files between your Mac and a node](help:copy-files)
- [Delete and restore files](help:delete-and-restore-files)
- [Edit a file on a node](help:edit-a-file)
- [Read a file only root can read](help:read-a-root-only-file)

![Files, using synthetic Demo data](img/public-files.jpg)
