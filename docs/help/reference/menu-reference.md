---
id: menu-reference
title: Menu bar reference
type: reference
area: reference
summary: Every item in SparkBox's macOS menus and what it does.
keywords: [menu, file, edit, view, go, cluster, window, help menu]
verified: null
related: [keyboard-shortcuts, menu-bar-icon, command-palette]
---

# Menu bar reference

This page lists every item in SparkBox's menus at the top of the screen, in menu order, with what each one does. An item with a dimmed name can't run right now. For example, a command that acts on a window is gray when no window is open.

Most commands act on the window in front on the desk. When a [separate window](help:windows-and-layouts) is in front, the desk comes forward first, except for **Close Window** and **Minimize Window**, which act on the separate window.

## SparkBox

| Item | Keys | What it does |
|---|---|---|
| **About SparkBox** | | Shows the version |
| **Settings…** | <kbd>⌘,</kbd> | Opens [Settings](help:settings-window) |
| **Services** | | The standard macOS Services menu |
| **Hide SparkBox** | <kbd>⌘H</kbd> | Hides the app |
| **Hide Others** | <kbd>⌥⌘H</kbd> | Hides other apps |
| **Show All** | | Shows every app |
| **Quit SparkBox** | <kbd>⌘Q</kbd> | Quits SparkBox |

## File

| Item | Keys | What it does |
|---|---|---|
| **New Terminal Tab** | <kbd>⌘T</kbd> | Opens a new tab in **Terminal** |
| **Open Files** | | Opens [Files](help:files) |
| **Close Window** | <kbd>⌘W</kbd> | Closes the window in front. With none open, it closes the SparkBox window. SparkBox stays in the menu bar if **Keep running in the menu bar** is on, and quits if it's off |
| **Close All Windows** | <kbd>⌥⌘W</kbd> | Closes every window on the desk |
| **Reopen Closed Window** | <kbd>⇧⌘T</kbd> | Brings back the last window you closed |

## Edit

The first six items are the standard Mac commands. They work in text fields, the Editor and terminals.

| Item | Keys | What it does |
|---|---|---|
| **Undo** | <kbd>⌘Z</kbd> | Undoes the last edit |
| **Redo** | <kbd>⇧⌘Z</kbd> | Redoes it |
| **Cut** | <kbd>⌘X</kbd> | Cuts the selection |
| **Copy** | <kbd>⌘C</kbd> | Copies the selection |
| **Paste** | <kbd>⌘V</kbd> | Pastes |
| **Select All** | <kbd>⌘A</kbd> | Selects everything |
| **Find…** | <kbd>⌘F</kbd> | Goes to the find or filter field of the window in front |

## View

| Item | Keys | What it does |
|---|---|---|
| **Command Palette…** | <kbd>⌘K</kbd> | Opens the [command palette](help:command-palette) |
| **Toggle Sidebar** | <kbd>⌃⌘S</kbd> | Hides or shows the sidebar |
| **Tile All** | <kbd>⌥⌘T</kbd> | Arranges the windows side by side, with no overlap. What doesn't fit goes to the dock at the bottom of the desk |
| **Hide Sidebar and Tile All** | | Hides the sidebar and gives its room to the tile |
| **Tile Two Side by Side** | | Puts two windows side by side. It needs at least two windows open |
| **Tile Four** | | Puts four windows in a grid. It needs at least two windows open |
| **Cascade** | <kbd>⌥⌘C</kbd> | Stacks the windows, each a little lower than the last |
| **Restore Layout** | | Puts tiled and cascaded windows back where you had them. It's dimmed until you've tiled or cascaded |
| **Snap Left** | <kbd>⌃⌥←</kbd> | Snaps the window to the left half of the desk |
| **Snap Right** | <kbd>⌃⌥→</kbd> | Snaps the window to the right half of the desk |
| **Fill Desktop** | <kbd>⌃⌥↑</kbd> | Fills the desk with the window |
| **Restore Size** | <kbd>⌃⌥↓</kbd> | Puts the window back to its size |
| **Show All Windows** | <kbd>F3</kbd> | Shows every window at once, small, so that you can pick one |
| **Open in Separate Window** | <kbd>⇧⌘O</kbd> | Moves the window out of the desk into a Mac window of its own |
| **Refresh** | <kbd>⌘R</kbd> | Refreshes the window in front |
| **Actual Size** | <kbd>⌘0</kbd> | Sets the zoom back to normal |
| **Zoom In** | <kbd>⌘+</kbd> | Makes everything larger |
| **Zoom Out** | <kbd>⌘−</kbd> | Makes everything smaller |
| **WinBox-Style Window Controls** | | Switches the buttons in each window's title bar to the WinBox style. A checkmark shows it's on |
| **Enter Full Screen** | <kbd>⌃⌘F</kbd> | Takes SparkBox full screen |

Two keys differ from what you see in the menu:

- **Zoom In** is the key marked `=` and `+`, pressed without Shift. The menu shows it as <kbd>⌘=</kbd>.
- **Show All Windows** shows no key in the menu, but <kbd>F3</kbd> works, except while a Terminal tab has the keyboard.

[Windows and layouts](help:windows-and-layouts) explains tiling, snapping and layouts.

## Go

| Item | Keys | What it does |
|---|---|---|
| **Dashboard** | <kbd>⌘1</kbd> | Opens [Dashboard](help:dashboard) |
| **Topology** | <kbd>⌘2</kbd> | Opens [Topology](help:topology) |
| **Containers** | <kbd>⌘3</kbd> | Opens [Containers](help:containers) |
| **Workloads** | <kbd>⌘4</kbd> | Opens [Workloads](help:workloads) |
| **Models** | <kbd>⌘5</kbd> | Opens [Models](help:models-window) |
| **Terminal** | <kbd>⌘6</kbd> | Opens [Terminal](help:terminal) |
| **Files** | <kbd>⌘7</kbd> | Opens [Files](help:files) |
| **Activity Log** | <kbd>⌘8</kbd> | Opens the [Activity log](help:activity-log) |
| **Alerts** | <kbd>⌘9</kbd> | Opens [Alerts](help:alerts-window) |
| **Nodes** | | A submenu with one item per node. Choose one to open its [window](help:node-window) |

## Cluster

| Item | Keys | What it does |
|---|---|---|
| **Unlock Admin Mode…** | <kbd>⇧⌘L</kbd> | Unlocks [Admin mode](help:admin-mode). While it's unlocked, this item reads **Lock Admin Mode** and locks it |
| **Data Source › Demo (Simulated Cluster)** | | Shows simulated data. A checkmark shows which source is in use |
| **Data Source › Live (Your Sparks over SSH)** | | Shows your real cluster. Changing the source asks for Touch ID or your Mac password. See [Switch between Demo and Live](help:switch-between-demo-and-live) |
| **Demo Scenarios** | | A submenu of simulated faults to practice on. It works only with Demo data. See [Settings: Demo scenarios](help:settings-demo-scenarios) |
| **Tasks** | <kbd>⇧⌘J</kbd> | Opens [Tasks](help:tasks) |

## Window

| Item | Keys | What it does |
|---|---|---|
| **Minimize Window** | <kbd>⌘M</kbd> | Minimizes the window in front |
| **Minimize SparkBox** | <kbd>⌥⌘M</kbd> | Minimizes SparkBox itself |
| **Zoom** | | The standard Mac zoom for the SparkBox window |
| **Next Window** | <kbd>⌘\`</kbd> | Goes to the next window on the desk |
| **Previous Window** | <kbd>⇧⌘\`</kbd> | Goes to the previous one |
| **Save Layout…** | | Saves where your windows are, under a name |
| **Layouts** | | A submenu of your saved layouts. Choose one to open it |
| The list of windows | | One item per open window. A checkmark marks the one in front, and "(minimized)" marks a minimized one. It reads **No Windows Open** when the desk is empty |
| **Bring All to Front** | | Restores minimized windows |

## Help

| Item | Keys | What it does |
|---|---|---|
| **SparkBox Help** | <kbd>⌘?</kbd> | Opens the Help window, where you last were |
| **Getting Started** | | A submenu of the first things to read |
| **How-To Guides** | | The main How-Tos, then a submenu for each topic, then **All How-To Guides…** |
| **Feature Guide** | | A submenu for each area, with a guide to each window and every Settings section, then **All Feature Guides…** |
| **Concepts** | | A submenu for each group of ideas, then **All Concepts…** |
| **Reference** | | The pages to look things up on, then **All Reference Pages…** |
| **Troubleshooting** | | [Troubleshooting](help:troubleshooting) and the message pages for each area, then **All Troubleshooting Pages…** |
| **Glossary** | | Opens the [Glossary](help:glossary) |
| **Keyboard Shortcuts** | | Opens [the keyboard shortcuts article](help:keyboard-shortcuts) |
| **What's Proven on Your Cluster** | | Opens [What's proven on your cluster](help:whats-proven) |
| **What's Coming** | | Opens [What's coming](help:whats-coming) |
| **Show Welcome** | | Shows the Welcome screen again |
| **Show Keyboard Shortcuts** | <kbd>⌘/</kbd> | Opens the **Keyboard shortcuts** dialog |
| **Open Diagnostics…** | | Opens **Settings › Diagnostics**. Nothing runs until you press its button there. See [Collect diagnostics](help:collect-diagnostics) |
| **Show Activity Log** | | Opens the [Activity log](help:activity-log) |

The menu shows **SparkBox Help** as <kbd>⇧⌘/</kbd>. Those are the same keys as <kbd>⌘?</kbd>.

Every article is in this menu. macOS adds a search field at the top of the **Help** menu, and it finds menu items by name, so you can type a word such as `fence` to find every article with it in the title. An item that ends in **…** opens a list of that section's articles in the Help window.

**SparkBox Help** and the items that open an article bring the SparkBox window to the front, because Help opens inside it. While the SparkBox window is closed to the menu bar, these menus aren't there. To open the window, use the [menu bar icon](help:menu-bar-icon).

## The menu bar icon

SparkBox also has an icon at the right of the menu bar, with its own short menu. [The menu bar icon](help:menu-bar-icon) describes it.
