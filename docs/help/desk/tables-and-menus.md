---
id: tables-and-menus
title: Lists, tables and right-click menus
type: feature
area: desk
summary: "Every list in SparkBox works the same way: sort, filter, choose columns, select rows and right-click."
keywords: [table, columns, sort, right click, context menu, filter, select, messages, undo]
verified: null
related: [windows-and-layouts, keyboard-shortcuts]
---

# Lists, tables and right-click menus

## What it's for

SparkBox shows most things as lists, such as containers, processes, files and alerts. They all work the same way, so what you learn in one window works in all of them. This page also covers the right-click menus and the short messages you see at the bottom of the desk.

## Where to find it

- **Any table:** any window with rows and column headers, such as [Containers](help:containers), [Workloads](help:workloads) or [Files](help:files).

## What's on the screen

### Column headers

| To | Do this |
|---|---|
| Sort | Click a header. Click it again to reverse the order. An arrow shows the sorted column |
| Resize a column | Drag the right edge of its header |
| Fit columns to the window again | Double-click that edge |
| Hide or show columns | Right-click any header, then select the columns you want. **Reset columns** brings back the defaults |


Sorting is natural, so `cell2` comes before `cell10`. Numbers sort by size. In **Files**, folders stay at the top whichever way you sort.

Each list remembers its sort, column widths and hidden columns on this Mac.

### Narrow windows

A table fits itself to its window, so you rarely scroll sideways. When the window gets narrower:

1. Long columns, such as paths and commands, shrink first and end in **…**.
2. Column headers change to a short form, where they have one.
3. The least important columns hide, one at a time.
4. A node's name may shorten, such as `c1` for `cell1`.

The **Columns** menu marks a column hidden this way with **(no room)**. Widen the window to see it again. Move the pointer over a shortened cell to read all of it.

### Selecting rows

| To | Do this |
|---|---|
| Select a row | Click it |
| Open it | Double-click it, or press <kbd>Return</kbd> |
| Select several, one by one | Hold <kbd>⌘</kbd> and click each row. Some lists have checkboxes instead |
| Select a range | Click the first row, hold <kbd>⇧</kbd> and click the last |
| Select all | Press <kbd>⌘A</kbd> |
| Move up and down | Press <kbd>↑</kbd> <kbd>↓</kbd>, <kbd>Page Up</kbd> <kbd>Page Down</kbd>, <kbd>Home</kbd> or <kbd>End</kbd> |
| Extend the selection | Hold <kbd>⇧</kbd> with those keys |
| Turn the row on or off | Press <kbd>Space</kbd>, in a list that allows several |
| Jump to a row | Type the first letters of its name |

Some lists allow one row at a time. The bottom of a list may show a count, such as "6 items · 1 selected".

### Finding a row

Windows with long lists have a filter field in their toolbar. Press <kbd>⌘F</kbd> to put the cursor in it. In [Terminal](help:terminal#find) and the [Editor](help:editor#find-and-replace), <kbd>⌘F</kbd> searches the text instead.

### Right-click menus

Right-click a row, a node or a title bar for what you can do with it. The same node has the same menu everywhere. To open a menu from the keyboard, press <kbd>⇧F10</kbd>.

Inside a menu:

| To | Do this |
|---|---|
| Move through it | <kbd>↑</kbd> <kbd>↓</kbd>, <kbd>Home</kbd> or <kbd>End</kbd> |
| Run an item | <kbd>Return</kbd> or <kbd>Space</kbd> |
| Open or leave a submenu | <kbd>→</kbd> or <kbd>←</kbd> |
| Jump to an item | Type its first letters |
| Close it | <kbd>Esc</kbd> |

An item may tell you more:

- **Dimmed, with a reason under it.** It can't run now, and the reason says why, such as "No containers are running".
- **A padlock at the right.** It needs Admin mode. Clicking it asks you to unlock first. In look-only mode it's dimmed.
- **A key at the right.** That's its keyboard shortcut.
- **Red text.** It removes or stops something.

### Messages

Short messages appear at the bottom right of the desk, above the status bar.

| Kind | How long it stays |
|---|---|
| Success or note | 4 seconds, unless it says otherwise. Moving the pointer over it keeps it |
| Error | Until you close it with the **×** |
| Progress | Until the work ends |

A message may have a button, such as **Undo**, **Open** or **Retry**. SparkBox skips a success message when the window in front already shows the result. Up to three show at once. **2 more** opens the rest.

### Undo

Some actions offer **Undo** in their message, such as closing a window (10 seconds) or moving a file to the Trash (30 seconds). <kbd>⌘Z</kbd> does the same, as long as the cursor isn't in a text field, the Editor or a terminal. Undo works for the most recent action first.

## What it changes

Sorting, selecting and arranging columns change only how the list looks on this Mac. Nothing here touches your nodes. A menu item that makes a change says so, and it asks for Admin mode if it needs it.

## Related How-Tos

- [Arrange your windows](help:arrange-your-windows)
- [See and stop processes](help:see-and-stop-processes)
- [Manage containers](help:manage-containers)
