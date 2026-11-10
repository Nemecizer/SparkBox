---
id: delete-and-restore-files
title: Delete and restore files
type: howto
area: tools
summary: Move files to the Trash, put them back, and bring back an earlier version of a file.
keywords: [trash, delete, undo, restore, previous versions, put back, recover, free space]
verified: false
related: [files, editor, edit-a-file]
---

# Delete and restore files

To remove a file safely, move it to the Trash, where you can get it back. To bring back an older version of a text file you edited, use **Previous versions**.

## Before you start

- Changing files needs [Admin mode](help:admin-mode). SparkBox asks for Touch ID or your Mac password with Live data, and for any change on this Mac, even with Demo data.
- In look-only mode, nothing can be deleted.
- With Demo data, files on nodes are simulated, but files on **This Mac** are real, even in Demo.
- To free disk space on a node, deleting a model usually frees far more. See [Delete a model from a node](help:delete-a-model).

## Steps

### Move a file to the Trash

1. In the sidebar, under **Tools**, click **Files**. [Open Files](open:files) The **Files** window opens with two panes.
2. In either pane, choose the place at the top left and open the file's folder.
3. Click the file. To choose several, hold <kbd>⌘</kbd> and click each one.
4. In the toolbar, click **Move to Trash**, or press <kbd>⌫</kbd>.
5. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. The file leaves the list.

A message says what moved, such as "Moved “notes.txt” to the Trash on cell1", with an **Undo** button.

### Put it back in the first 30 seconds

1. Click **Undo** in the message at the bottom right of the desk. You can also press <kbd>⌘Z</kbd>, as long as the cursor isn't in a text field or a terminal. The file returns to its folder.

### Put it back later

After 30 seconds, the **Undo** button is gone, but the file is still in the Trash.

#### On this Mac

1. Open the Trash in the Dock.
2. Right-click the file and choose **Put Back**. Finder returns it to its folder.

#### On a node

1. In **Files**, show the node in both panes.
2. In the left pane, open the folder where you want the file to go.
3. In the right pane, click the path and type `~/.local/share/Trash/files`, then press <kbd>Return</kbd>. The pane lists what's in that node's Trash.
4. Click the file, then click **Move to cell1** in the toolbar. The file moves to the folder in the left pane.

A file that was on a different disk from your home folder isn't there. Look in the hidden `.sparkbox-trash/files` folder beside where it was. To see hidden folders, click **Hidden files** in the toolbar.

### Delete a file for good

> [!DANGER]
> **Delete immediately can't be undone.** The file doesn't go to the Trash, and there's no way to get it back. Stop if you aren't sure, and use **Move to Trash** instead.

1. Select the file in **Files**.
2. Hold <kbd>⌥</kbd>. The **Move to Trash** button changes to **Delete immediately**.
3. Click **Delete immediately**, or press <kbd>⌥⌫</kbd>. The dialog lists what will go, such as "Delete “notes.txt” immediately?".
4. Check the list, then click **Delete**.

### Bring back an earlier version

SparkBox keeps the last five versions of a text file each time you save it in the Editor.

1. Open the file in the Editor. In **Files**, double-click it.
2. In the Editor's toolbar, click **Previous versions**. A menu lists the versions, newest first, with the date and size. The first one says "(before the last save)".
3. Click the version you want. The Editor shows it, and a message says "Previous version loaded". Nothing has changed on disk yet.
4. Read it. If it's what you wanted, press <kbd>⌘S</kbd>. The file on disk becomes that version, and the version it replaced is kept too.

To drop the old version instead, click **Revert** in the toolbar. The Editor goes back to the file on disk.

## What you should see

- After **Move to Trash**, the file has gone from the list, and the message with **Undo** appears.
- After **Undo**, the file is back where it was, and the pane refreshes.
- After restoring a version and saving, the bottom right of the Editor says **Saved** with the time.

## Why it works

**Move to Trash** doesn't erase anything. On this Mac it uses the Mac's own Trash, so Finder's **Put Back** works. On a node it uses the standard Linux Trash in `~/.local/share/Trash`, in your home folder. **Delete immediately** erases the file at once. Previous versions are copies SparkBox makes right before each save in the Editor, kept in a `backups` folder of its own.

## Limits

- **Undo** lasts 30 seconds, and it goes away if you quit SparkBox.
- SparkBox doesn't empty the Trash on a node. Trashed files still use disk space there until you delete them for good.
- Previous versions exist only for files you saved in SparkBox's Editor. A file changed in **Terminal** or by a program has none.
- SparkBox won't delete system folders, anything directly inside `/`, or your home folder, on this Mac or on a node.

## If something goes wrong

- **"SparkBox won't delete /etc."** That folder is protected. SparkBox refuses it on purpose, and no setting changes that.
- **"Moved to the Trash" with "Finder took it; use Put Back in the Trash to restore it."** macOS handled the move itself, so SparkBox can't offer **Undo**. Use **Put Back** in Finder.
- **"/home/sparkuser/notes.txt already exists, so the trashed copy was left in the Trash."** Something new has taken the old name. Rename or move it, then try again.
- **"… is no longer in the Trash."** The Trash has been emptied, so the file is gone.
- **"SparkBox doesn't know where that item went; use Finder's Put Back."** Open the Trash in Finder and use **Put Back**.
- **"No previous versions yet".** You haven't saved this file in the Editor yet.

More messages are in [Files and Terminal messages](help:errors-files-and-terminal).

## Next steps

- [Files](help:files), to see every part of the window
- [Edit a file on a node](help:edit-a-file), to see how saves are kept
- [Check disk space on a node](help:check-disk-space), to see what's using the space
