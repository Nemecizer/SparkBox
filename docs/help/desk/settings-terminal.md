---
id: settings-terminal
title: "Settings: Terminal"
type: feature
area: settings
summary: How long idle shells stay open, and how the terminal looks and behaves.
keywords: [font, theme, scrollback, option key, idle, lock idle shells, cursor, copy on select, meta]
verified: false
related: [terminal, open-a-terminal]
---

# Settings: Terminal

## What it's for

These settings control when an idle shell locks, and how [Terminal](help:terminal) looks and behaves. A **shell** is a command prompt on a node. Changes to the look apply to every open tab right away.

## Where to find it

- **Sidebar:** click **Settings**, then click **Terminal**.
- **Terminal window:** in the toolbar, click **Terminal settings**.
- **Menu bar:** choose **SparkBox › Settings…**, then click **Terminal**.
- **Keyboard:** press <kbd>⌘,</kbd>, then click **Terminal**.
- **Command palette:** press <kbd>⌘K</kbd>, type `settings terminal`, and choose **Settings › Terminal**.
- **Link:** [Open Terminal settings](open:settings?section=terminal)


## What's on the screen

### Shells

| Setting | Choices | Default |
|---|---|---|
| **Lock idle shells after** | **15 min**, **60 min**, **4 h**, and a fourth choice | **60 min** |

If nothing is typed in a shell for this long, it stops taking keystrokes. A minute before, the tab warns you, and typing anything resets the time. A locked shell keeps its session. Click **Authenticate…** in the tab to type again.

The fourth choice depends on your data source:

| Data source | Fourth choice | What it does |
|---|---|---|
| Live | **8 h** | A Live shell locks after 8 hours at most, so that's the longest you can set |
| Demo | **Never** | Demo shells don't lock for being idle |

This isn't the only thing that locks a shell. It also locks when you lock Admin mode, when the screen locks, and when the Mac sleeps. With Live data, it locks whenever Admin mode does, including when Admin mode locks itself. [Locked shells](help:terminal#locked-shells) has the full list.

### Preview

A sample session in your theme, font, size and cursor. It updates as you change them.

### Look

| Setting | What it does | Default |
|---|---|---|
| **Theme** | The terminal's colors: **Follow app appearance**, **SparkBox Dark**, **SparkBox Light**, **Solarized Dark** or **One Dark** | **Follow app appearance** |
| **Font** | Any fixed-width font installed on this Mac, from those SparkBox knows | **SF Mono** |
| **Size** | The text size, from 9 to 24 points | 12 |
| **Line height** | The space between lines, from 1.00 to 1.40 | 1.15 |

### Cursor and scrollback

| Setting | What it does | Default |
|---|---|---|
| **Cursor** | **Block**, **Bar** or **Underline** | **Block** |
| **Blinking cursor** | Makes the cursor blink | On |
| **Scrollback** | How many past lines each tab keeps, so that you can scroll back and search them with <kbd>⌘F</kbd>: 1,000 to 100,000 | 10,000 lines |
| **Copy on select** | Selecting text copies it at once, so you don't need <kbd>⌘C</kbd> | Off |

More scrollback uses more of this Mac's memory, for each tab.

### Keyboard

| Setting | Choices | Default |
|---|---|---|
| **Option key as Meta** | **Normal**, **Left ⌥**, **Right ⌥**, **Both** | **Normal** |

**Normal** lets the Option key type characters, as it does everywhere on a Mac. **Meta** makes it send a special code instead, which many command-line programs use. For example, <kbd>⌥B</kbd> and <kbd>⌥F</kbd> move back and forward a word, and <kbd>⌥.</kbd> brings back the last argument.

If your keyboard types `@`, `|`, `{ }` or `[ ]` with Option, keep one Option key **Normal**.

A line at the bottom of the **Terminal** window shows which you've chosen.

### Restore defaults

**Restore defaults** resets the look, cursor, scrollback, **Copy on select** and the Option key. It doesn't change **Lock idle shells after**.

## What it changes

| Setting | What it changes | What it needs | How to undo it |
|---|---|---|---|
| **Lock idle shells after** | How long a shell stays unlocked on this Mac | Touch ID or your Mac password. The change is in the [activity log](help:activity-log) | Choose the old value |
| Everything else | How Terminal looks and behaves on this Mac | Nothing | **Restore defaults**, or choose the old value |

Nothing here changes a node.

## Related How-Tos

- [Open a terminal on a node](help:open-a-terminal)
- [Unlock Admin mode](help:unlock-admin-mode)
