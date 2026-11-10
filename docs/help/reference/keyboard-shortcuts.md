---
id: keyboard-shortcuts
title: Keyboard shortcuts
type: reference
area: reference
summary: The keyboard shortcuts in SparkBox, grouped by what they act on.
keywords: [shortcuts, keys, keyboard, hotkeys, command, option, control]
verified: null
related: [command-palette, menu-reference, tables-and-menus]
---

# Keyboard shortcuts

This page lists the keyboard shortcuts in SparkBox, grouped by what they act on. To see the main ones inside the app, press <kbd>⌘/</kbd>.

The keys use the Mac's symbols: ⌘ is Command, ⌥ is Option, ⇧ is Shift, ⌃ is Control.

A shortcut that has a menu item works even while a Terminal tab has the keyboard. Keys with no modifier, such as <kbd>Return</kbd> or a letter, never take over from a text field.

> [!NOTE]
> Some keys change meaning in the window in front. For example, <kbd>⌘T</kbd> opens a new Terminal tab, but in **Containers** it opens a shell in the selected container. When a window changes a key, the <kbd>⌘/</kbd> dialog lists that at the top, under the window's name.

## Windows on the desk

| Shortcut | What it does | Menu |
|---|---|---|
| <kbd>⌘W</kbd> | Close the window in front. With no window open, it closes the SparkBox window | **File › Close Window** |
| <kbd>⌥⌘W</kbd> | Close all windows | **File › Close All Windows** |
| <kbd>⇧⌘T</kbd> | Reopen the last window you closed | **File › Reopen Closed Window** |
| <kbd>⌘M</kbd> | Minimize the window in front | **Window › Minimize Window** |
| <kbd>⌥⌘M</kbd> | Minimize SparkBox itself | **Window › Minimize SparkBox** |
| <kbd>⌘\`</kbd> | Go to the next window | **Window › Next Window** |
| <kbd>⇧⌘\`</kbd> | Go to the previous window | **Window › Previous Window** |
| <kbd>⌥⌘T</kbd> | Tile all windows | **View › Tile All** |
| <kbd>⌥⌘C</kbd> | Cascade the windows | **View › Cascade** |
| <kbd>⌃⌥←</kbd> | Snap the window to the left half | **View › Snap Left** |
| <kbd>⌃⌥→</kbd> | Snap the window to the right half | **View › Snap Right** |
| <kbd>⌃⌥↑</kbd> | Fill the desk with the window | **View › Fill Desktop** |
| <kbd>⌃⌥↓</kbd> | Put the window back to its size | **View › Restore Size** |
| <kbd>⇧⌘O</kbd> | Move the window out into a separate window | **View › Open in Separate Window** |
| <kbd>F3</kbd> | Show all windows at once | **View › Show All Windows** |
| <kbd>⌘Z</kbd> | Undo closing a window, or a move to the Trash. Outside text fields and terminals only | |
| <kbd>⌥</kbd> drag | Drop a window freely. A plain drag to an edge snaps it | |

While **Show All Windows** is open, the arrow keys or <kbd>Tab</kbd> move between windows. <kbd>Return</kbd> or <kbd>Space</kbd> opens the one you picked, and <kbd>Esc</kbd> closes the view.

## Open a window

| Shortcut | Opens | Menu |
|---|---|---|
| <kbd>⌘1</kbd> | **Dashboard** | **Go › Dashboard** |
| <kbd>⌘2</kbd> | **Topology** | **Go › Topology** |
| <kbd>⌘3</kbd> | **Containers** | **Go › Containers** |
| <kbd>⌘4</kbd> | **Workloads** | **Go › Workloads** |
| <kbd>⌘5</kbd> | **Models** | **Go › Models** |
| <kbd>⌘6</kbd> | **Terminal** | **Go › Terminal** |
| <kbd>⌘7</kbd> | **Files** | **Go › Files** |
| <kbd>⌘8</kbd> | **Activity log** | **Go › Activity Log** |
| <kbd>⌘9</kbd> | **Alerts** | **Go › Alerts** |
| <kbd>⇧⌘J</kbd> | **Tasks** | **Cluster › Tasks** |
| <kbd>⌘,</kbd> | **Settings** | **SparkBox › Settings…** |
| <kbd>⌘T</kbd> | A new tab in **Terminal** | **File › New Terminal Tab** |

## The app

| Shortcut | What it does | Menu |
|---|---|---|
| <kbd>⌘K</kbd> | Open the command palette | **View › Command Palette…** |
| <kbd>⌃⌘S</kbd> | Hide or show the sidebar | **View › Toggle Sidebar** |
| <kbd>⌘F</kbd> | Find in the window in front | **Edit › Find…** |
| <kbd>⌘R</kbd> | Refresh the window in front | **View › Refresh** |
| <kbd>⌘0</kbd> | Actual size | **View › Actual Size** |
| <kbd>⌘+</kbd> | Zoom in | **View › Zoom In** |
| <kbd>⌘−</kbd> | Zoom out | **View › Zoom Out** |
| <kbd>⇧⌘L</kbd> | Unlock or lock Admin mode | **Cluster › Unlock Admin Mode…** |
| <kbd>⌘/</kbd> | Show the keyboard shortcuts | **Help › Show Keyboard Shortcuts** |
| <kbd>⌘?</kbd> | Open SparkBox Help | **Help › SparkBox Help** |
| <kbd>⌃⌘F</kbd> | Enter or leave full screen | **View › Enter Full Screen** |
| <kbd>⌘H</kbd> | Hide SparkBox | **SparkBox › Hide SparkBox** |
| <kbd>⌥⌘H</kbd> | Hide other apps | **SparkBox › Hide Others** |
| <kbd>⌘Q</kbd> | Quit SparkBox | **SparkBox › Quit SparkBox** |

The menu item for <kbd>⇧⌘L</kbd> reads **Lock Admin Mode** while Admin mode is unlocked.

Two of these keys look different in the menu. For <kbd>⌘+</kbd>, press the key marked `=` and `+`, without Shift. The menu shows it as <kbd>⌘=</kbd>. The menu shows <kbd>⌘?</kbd> as <kbd>⇧⌘/</kbd>, which is the same keys.

## Text

These are the standard Mac commands in the **Edit** menu. They work in text fields, the Editor and terminals.

| Shortcut | What it does |
|---|---|
| <kbd>⌘Z</kbd> <kbd>⇧⌘Z</kbd> | Undo and redo |
| <kbd>⌘X</kbd> <kbd>⌘C</kbd> <kbd>⌘V</kbd> | Cut, copy and paste |
| <kbd>⌘A</kbd> | Select all |

## Sidebar

| Shortcut | What it does |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> | Move between rows, once a row has the keyboard |
| <kbd>Return</kbd> in **Filter** | Open the first row |
| <kbd>↓</kbd> in **Filter** | Move to the first row |
| <kbd>Esc</kbd> in **Filter** | Clear the filter |

## Lists and tables

These work in any list once a row has the keyboard.

| Shortcut | What it does |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> | Move to the row above or below |
| <kbd>Page Up</kbd> <kbd>Page Down</kbd> | Move a page at a time |
| <kbd>Home</kbd> <kbd>End</kbd> | Go to the first or last row |
| <kbd>⇧</kbd> with any of those keys | Extend the selection |
| <kbd>⌘A</kbd> | Select every row, in lists that allow several |
| <kbd>Space</kbd> | Select the row. In lists that allow several, it selects or deselects that row |
| Type some letters | Jump to the row that starts with them |
| <kbd>Return</kbd> | Open the selected row: its status, details or Inspect window |
| <kbd>⇧F10</kbd> | Open the right-click menu of the selected item |
| <kbd>Esc</kbd> | Close a menu, a dialog or the command palette |

## Containers

| Shortcut | What it does |
|---|---|
| <kbd>⌘L</kbd> | Show the logs of the selected container |
| <kbd>⌘T</kbd> | Open a shell in the selected container |
| <kbd>⌘I</kbd> | Inspect the selected container |
| <kbd>⌦</kbd> or <kbd>⌘⌫</kbd> | Ask to remove the selected containers. <kbd>⌦</kbd> is Forward Delete: on a laptop, press <kbd>fn</kbd> and <kbd>⌫</kbd> together |

## Alerts

These work on a selected row that groups other rows.

| Shortcut | What it does |
|---|---|
| <kbd>→</kbd> | Show the rows grouped under it |
| <kbd>←</kbd> | Hide them |

## Fleet Command

These work in the command box.

| Shortcut | What it does |
|---|---|
| <kbd>⌘Return</kbd> | Run the command |
| <kbd>↑</kbd> on the first line | Bring back the previous command |
| <kbd>↓</kbd> on the last line | Go forward to a newer command |

## Chat

These work in the message box.

| Shortcut | What it does |
|---|---|
| <kbd>Return</kbd> | Send the message |
| <kbd>⇧Return</kbd> | Start a new line |
| <kbd>Esc</kbd> | Stop the reply while it's arriving |

While you edit an earlier message, <kbd>Return</kbd> sends it and <kbd>Esc</kbd> cancels the edit.

## Command palette

| Shortcut | What it does |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd>, or <kbd>⌃P</kbd> <kbd>⌃N</kbd> | Move between results |
| <kbd>Return</kbd> | Run the selected command |
| <kbd>Esc</kbd> | Close the palette |

## Terminal

These work while a Terminal tab has the keyboard. <kbd>⌘K</kbd>, <kbd>⌘+</kbd>, <kbd>⌘−</kbd> and <kbd>⌘W</kbd> keep their app-wide meaning.

| Shortcut | What it does |
|---|---|
| <kbd>⌘T</kbd> | Open a new tab |
| <kbd>⌃Tab</kbd> or <kbd>⇧⌘]</kbd> | Go to the next tab |
| <kbd>⌃⇧Tab</kbd> or <kbd>⇧⌘[</kbd> | Go to the previous tab |
| <kbd>⌘F</kbd> | Find in the scrollback |
| <kbd>⌘G</kbd> | Find the next match |
| <kbd>⇧⌘G</kbd> | Find the previous match |
| <kbd>⌥⌘K</kbd> | Clear the scrollback |

In the find box, <kbd>Return</kbd> finds the next match, <kbd>⇧Return</kbd> finds the previous one, and <kbd>Esc</kbd> closes the box.

**Settings › Terminal** can make an Option key act as Meta, the key that shell programs use for moves such as "back one word". You can choose the left key, the right key or both. See [Settings: Terminal](help:settings-terminal).

## Files

These work while a pane in **Files** has the keyboard, and not while you type in a field.

| Shortcut | What it does |
|---|---|
| <kbd>Tab</kbd> | Move to the other pane |
| <kbd>Return</kbd> or <kbd>⌘↓</kbd> | Open the selected item |
| <kbd>⌘↑</kbd> | Go up to the parent folder |
| <kbd>⌘[</kbd> <kbd>⌘]</kbd> | Go back or forward |
| <kbd>Space</kbd> or <kbd>⌘I</kbd> | Show or hide **Get info** |
| <kbd>F5</kbd> | Copy to the other pane |
| <kbd>F6</kbd> | Move to the other pane |
| <kbd>F2</kbd> | Rename |
| <kbd>⌘C</kbd> <kbd>⌘V</kbd> | Copy and paste |
| <kbd>⌘D</kbd> | Duplicate |
| <kbd>⇧⌘N</kbd> | New folder |
| <kbd>⌫</kbd> or <kbd>Delete</kbd> | Move to Trash |
| <kbd>⌥⌫</kbd> | Delete immediately, with a confirmation. This can't be undone |
| <kbd>⌥</kbd> while you drag | Move instead of copy |

Changing keys need Admin mode. See [Copy files between your Mac and a node](help:copy-files).

## Editor

| Shortcut | What it does |
|---|---|
| <kbd>⌘S</kbd> | Save. When saving as root, SparkBox shows the changes first |
| <kbd>⌘F</kbd> | Find |
| <kbd>⌥⌘F</kbd> | Find and replace |
| <kbd>⌘L</kbd> or <kbd>⌃G</kbd> | Go to a line |
| <kbd>⌘Z</kbd> <kbd>⇧⌘Z</kbd> | Undo and redo |
| <kbd>Tab</kbd> | Indent |

The Editor uses <kbd>⌘L</kbd> for Go to Line, which is why Admin mode's shortcut is <kbd>⇧⌘L</kbd>.

## Help

| Shortcut | What it does |
|---|---|
| <kbd>⌘F</kbd> | Go to **Search Help** |
| <kbd>⌘[</kbd> <kbd>⌘]</kbd> | Back and Forward |
| <kbd>↑</kbd> <kbd>↓</kbd> <kbd>←</kbd> <kbd>→</kbd> | Move and fold in the table of contents |
| <kbd>Esc</kbd> | Close a Help tip |
