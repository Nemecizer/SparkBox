---
id: windows-and-layouts
title: Windows and layouts
type: feature
area: desk
summary: "How SparkBox's windows behave: tiling, snapping, closing with Undo, saved layouts and separate windows."
keywords: [windows, tile, dock, minimize, undo, snap, cascade, restore layout, overview, close others, fill desktop]
verified: null
related: [arrange-your-windows, keyboard-shortcuts, sparkbox-window]
---

# Windows and layouts

## What it's for

Your tools open as windows on the **desk**, the middle of the SparkBox window. You can move them, stack them and fit them together, so that you see what you need at once. Nothing is lost by accident: a closed window can come back, and a minimized one keeps working.

## Where to find it

- **Sidebar:** click a row to open its window.
- **Start page:** click a tile.
- **Other windows:** click a link inside another window.
- **Top bar:** **Tile**, **Windows** and **Close all** are here.
- **Menu bar:** the **View** and **Window** menus have the same commands.
- **Command palette:** the [command palette](help:command-palette) runs any of them by name.

There's one copy of each window. Opening a window that's already open brings it to the front. There are as many node windows as nodes, though, and an Editor window for each file.


## What's on the screen

### Title bar

| Part | What it does |
|---|---|
| Red, yellow and green circles | Close, minimize and zoom, like a Mac window |
| Title | The window's name. A dot before it means unsaved changes |
| **i** icon | A Help tip with a link to this window's guide |

Each action has a matching way to do it with the mouse:

| To | Do this |
|---|---|
| Move a window | Drag its title bar |
| Resize it | Drag any edge or corner |
| Fill the desk | Double-click its title bar, click the green circle, or choose **View › Fill Desktop** |
| See more choices | Right-click its title bar |

The right-click menu has **Minimize**, **Maximize** or **Restore**, **Snap left**, **Snap right**, **Open in separate window**, **Close others** and **Close**.

SparkBox keeps every title bar in reach. If the main window shrinks, windows move back onto the desk and narrow as far as their minimum size. When it grows again, they go back to where you had them.

**View › WinBox-Style Window Controls** swaps the circles for flat buttons at the right of each title bar.

### Snapping

Drag a window by its title bar to an edge of the desk, and a tinted outline shows where it will land. Release to snap it.

| Drag to | It fills |
|---|---|
| The left or right edge | That half |
| A corner, within about 64 points of it | That quarter |
| The top edge | The whole desk |

Hold <kbd>⌥</kbd> while you drag to drop a window freely. Dragging a snapped window away gives it back its own size.

### Tile

**Tile** fits every window on the desk with small gaps and no overlaps. It picks the rows or columns that give each window the most room. Each window has a smallest useful size. If they can't all fit, the ones you used longest ago go to the dock, and a message offers **Undo**.

| Choice | What it does |
|---|---|
| **Tile all** | Every window that fits. If some don't, it says so: "Tile all — fits 4 of 6 here" |
| **Hide sidebar and tile all (fits 6)** | Appears when hiding the sidebar would fit more |
| **Side by side** | The two windows you used last. If they don't fit across, they stack top and bottom |
| **Tile 4** | The four windows you used last |

If you resize the main window, tiled windows tile again, as long as you haven't moved one by hand.

### Cascade

**Cascade** stacks windows like cards, each a step down and to the right, so that every title bar shows. A window that would reach the edge starts a new stack.

### Restore layout

**Restore layout** undoes **Tile** and **Cascade**. Each window goes back to where you put it by hand, and windows that went to the dock come back. If tiling hid the sidebar, it shows again. Tile and Cascade never replace the positions you chose, which is why this always works.

### Minimize and the dock

Minimize sends a window to the **dock**, the row at the bottom of the desk. The dock appears only when something is in it. It isn't the Mac's Dock.

A minimized window keeps running. A terminal stays connected, and an editor keeps your text. Click its name in the dock to bring it back. Click the **×** beside the name to close it, or right-click the name for **Restore** and **Close**.

### Show all windows

Press <kbd>F3</kbd> to see every window at once, minimized ones too, scaled down in a grid. Click one to bring it forward, or use the arrow keys and <kbd>Return</kbd>. <kbd>Esc</kbd> goes back.

### Moving between windows

- <kbd>⌘\`</kbd> goes to the next window, and <kbd>⇧⌘\`</kbd> to the one before.
- The **Windows** menu in the top bar lists every open window, with a checkmark beside the front one.
- **Bring all to front** brings every minimized window back.

### Closing, Undo and reopening

Closing a window is safe. A message says "Closed “Containers”" with an **Undo** button for 10 seconds. Close several in a row, and the message reads "3 windows closed". <kbd>⌘Z</kbd> does the same, as long as the cursor isn't in a text field or a terminal.

After that:

- <kbd>⇧⌘T</kbd> reopens what you closed last, and SparkBox remembers your last 10 closes.
- The start page lists **Recently closed** windows and offers **Restore last layout**.

SparkBox asks first if closing would lose something, with a dialog such as "Close “Terminal”?" that says what's at stake. **Close anyway** goes ahead. In **Close all**, one dialog lists everything that would be lost.

The **Terminal** window is a special case. While it has more than one tab, closing it closes the tab in front, not the window.

If you press <kbd>⌘W</kbd> with no windows open, SparkBox closes its main window and keeps running in the menu bar.

### Saved layouts

A **layout** is a named set of windows and where they sit, including which tab each one shows.

- To save one, click **Windows** in the top bar, then **Save layout…**. Names can be up to 40 characters.
- To open one, click **Windows**, then **Layouts**, then its name. Your open windows close, with **Undo**, and the layout's windows open.
- To delete one, click **Windows**, then **Layouts**, then **Delete a layout**.

**Layouts** also appears in the **Window** menu in the macOS menu bar.

### Separate windows

A **separate window** moves a window off the desk and into a normal Mac window of its own. Use it for a second screen. Press <kbd>⇧⌘O</kbd>, or right-click a title bar and click **Open in separate window**.

It shows the same live data. It has a small bar at the top with its title, a Help tip and **Return to desk**. It has no top bar, sidebar or status bar. Links in it open on the desk.

A separate window starts fresh. So a window with unsaved text or an open session stays on the desk until you've saved or finished.

### What comes back when you start SparkBox

Your windows return to where you put them, with the tabs they showed. A window that was tiled returns to its own place, and one that was minimized by **Tile** returns to the desk. Terminal tabs don't reconnect by themselves. SparkBox offers to reopen them, and nothing connects until you choose.

## What it changes

Everything on this page changes only how windows sit on this Mac. Nothing here touches your nodes, and it all works in look-only mode.

Layouts and window positions are kept in SparkBox's own window storage on this Mac. Delete a layout from the **Windows** menu. Your window positions change whenever you move a window.

## Related How-Tos

- [Arrange your windows](help:arrange-your-windows)
- [Back up SparkBox](help:back-up-sparkbox)
