---
id: command-palette
title: The command palette
type: feature
area: desk
summary: Press ⌘K, type a few letters, and go anywhere or run almost anything.
keywords: [search, cmd k, find, quick, palette, command]
verified: null
related: [keyboard-shortcuts, sparkbox-window, using-help]
---

# The command palette

## What it's for

The command palette is the fastest way around SparkBox. Press one key, type a few letters of what you want, and press <kbd>Return</kbd>. It finds windows, nodes, containers, services, settings and commands, and it searches Help too.

Use it when you know what you want but not where it lives.

## Where to find it

- **Keyboard:** press <kbd>⌘K</kbd> anywhere. It works even while you're typing in a terminal.
- **Top bar:** click **Search**.
- **Menu bar:** choose **View › Command Palette…**.

## What's on the screen


### The search field

Type here. The list below updates as you type. <kbd>Esc</kbd> clears what you typed, and a second <kbd>Esc</kbd> closes the palette. Clicking outside it closes it too.

### The list

With nothing typed, the palette starts with **Recent**, the five commands you ran last, then lists everything else in groups.

| Group | What's in it |
|---|---|
| **Open windows** | Your open windows, to switch to one |
| **Windows** | Every window you can open |
| **Nodes** | Each node's actions: open, Terminal, Files, DGX Dashboard, restart and shut down |
| **Containers**, **Services**, **Models** | Actions on what's running, including anything that failed |
| **Switches** | Your switches and gateway |
| **Window** | Arranging: tile, snap, cascade, save a layout |
| **Cluster** | Lock or unlock Admin mode, and switch between Demo and Live data |
| **Settings** | Each section of Settings |
| **Appearance** | Theme, text size, reduced motion and contrast |
| **App** | Zoom, the sidebar, keyboard shortcuts and other commands |
| **Help** | Every Help article, once you type something |

Other groups appear for what's open or available, such as **Terminal**, **Fleet**, **Alerts**, **Network** and, with Demo data, **Demo** for scenarios. **Recent** is kept separately for Demo and Live data, so a command you tried in Demo isn't offered first on your real cluster.

### Rows

Each row has a title, sometimes a line of detail, and its key if it has one.

- **Dimmed, with a reason.** The command can't run now, and the row says why, such as "No window is open".
- **A padlock.** The command needs Admin mode, and it asks you to unlock first.
- **Look-only mode.** The bottom of the palette says "Look-only: nothing runs", and commands that make changes are dimmed.

### How it matches

You don't need to type whole words:

| You type | It finds |
|---|---|
| Parts of words: `rest cel3` | **Restart cell3…** Each word you type must start a word in the title |
| A word with one typo: `termnal` | **Terminal**. It forgives one slip in words of four letters or more, but never a wrong number |
| Another word for the same thing: `power off` | **Shut down…**. `log` finds **Logs** too |
| First letters: `toc` | **Terminal on cell1** |
| A related word: `preferences` | **Settings** |

The best match comes first. Commands from the window in front, and ones you ran lately, move up a little.

### Search Help

Whenever you've typed something, **Search Help for “…”** is the last row. If you type a question, such as `how do I run a model`, it comes first. Choose it to search the full text of every Help article.

### Keys

| Key | What it does |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> | Move through the list. <kbd>⌃N</kbd> and <kbd>⌃P</kbd> do the same |
| <kbd>Page Up</kbd> <kbd>Page Down</kbd> | Move 8 rows |
| <kbd>Return</kbd> | Run the highlighted command |
| <kbd>Esc</kbd> | Clear the field, then close |

## What it changes

The palette changes nothing by itself. It runs commands, and each one behaves as it does from its own button: a command that restarts a node still shows its confirmation.

## Related How-Tos

- [Arrange your windows](help:arrange-your-windows)
- [Open a terminal on a node](help:open-a-terminal)
- [Unlock Admin mode](help:unlock-admin-mode)
