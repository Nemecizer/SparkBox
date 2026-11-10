---
id: arrange-your-windows
title: Arrange your windows
type: howto
area: desk
summary: Tile, snap and cascade windows, save a layout, and move a window to a second screen.
keywords: [tile, snap, layout, second screen, cascade, organize, side by side, separate window]
verified: null
related: [windows-and-layouts, keyboard-shortcuts]
---

# Arrange your windows

To see several windows at once, tile or snap them on the desk, save a layout, or move a window to a second screen.

## Before you start

- The **desk** is the area of the SparkBox window where windows open.
- This works with Demo or Live data, and in look-only mode. It changes where windows sit on this Mac and nothing on your nodes.
- Moving a window to a second screen needs a second screen connected to this Mac.
- It takes a minute or two.

## Steps

### Tile your windows

Tiling sets windows side by side so that none overlaps another.

1. In the sidebar, under **Overview**, click **Dashboard**. [Open Dashboard](open:dashboard) The **Dashboard** window opens on the desk.
2. In the sidebar, under **Overview**, click **Topology**. The **Topology** window opens on top of it.
3. In the top bar, click **Tile**. The two windows move side by side, and each one fills half of the desk.
4. To put them back, click the small arrow next to **Tile**, then click **Restore layout**. Each window returns to where you had it.


The same arrow offers other arrangements:

| Choice | What it does |
|---|---|
| **Tile all** | Fits every open window on the desk. <kbd>⌥⌘T</kbd> does the same |
| **Side by side** | Tiles the two windows you used last |
| **Tile 4** | Tiles the four windows you used last |
| **Cascade** | Stacks the windows like cards, each one a step down and to the right. <kbd>⌥⌘C</kbd> does the same |
| **Restore layout** | Undoes any of these |

With **Side by side** and **Tile 4**, the other windows go to the dock. The **dock** is the row of minimized windows at the bottom of the desk. It isn't the Mac's Dock. **Restore layout** brings them back.

### Snap one window

Snapping sends one window to a half or a quarter of the desk.

1. Drag the window by its title bar to the left edge of the desk. A tinted outline shows the space it will fill.
2. Release the mouse button. The window fills the left half of the desk.

| Drag the window to | It fills |
|---|---|
| The left or right edge | That half of the desk |
| A corner | That quarter of the desk |
| The top edge | The whole desk |

To drop a window near an edge without snapping it, hold <kbd>⌥</kbd> while you drag. To snap from the keyboard, press <kbd>⌃⌥←</kbd> or <kbd>⌃⌥→</kbd>. <kbd>⌃⌥↑</kbd> fills the desk, and <kbd>⌃⌥↓</kbd> restores the window's size.

### Save a layout

A **layout** is a named set of windows and where they sit. Saving one lets you come back to it in one click.

1. Open the windows you want and arrange them.
2. In the top bar, click **Windows**. A menu lists your open windows and the arranging commands.
3. Click **Save layout…**. The **Save layout** dialog opens.
4. In **Name**, type a name, such as `Model bring-up`. Use fewer than 41 characters.
5. Click **Save**. A message says "Saved layout “Model bring-up”".

### Open a saved layout

1. In the top bar, click **Windows**.
2. Point to **Layouts**, then click the layout's name. SparkBox closes the open windows and opens the layout's windows where you saved them.

If you change your mind, click **Undo** in the message at the bottom right of the desk within 10 seconds. It brings back the windows that closed.

**Layouts** appears in the menu once you've saved at least one. To remove a layout, click **Windows**, then **Layouts**, then **Delete a layout**, then its name.

### Move a window to a second screen

A **separate window** is a SparkBox window that has left the desk and become a normal Mac window. You can drag it to any screen.

1. Click the window you want to move, so that it's in front.
2. Press <kbd>⇧⌘O</kbd>. You can also right-click its title bar and click **Open in separate window**. The window leaves the desk and opens as its own Mac window.
3. Drag the new window by its title bar to the second screen.
4. When you're done, click **Return to desk** at the top right of that window. It goes back to the desk.

## What you should see

- After **Tile**, no window covers another, and there are small gaps between them.
- After you snap a window, it fills exactly the half or quarter that the outline showed.
- After you save a layout, its name is listed under **Windows › Layouts** and under **Window › Layouts** in the macOS menu bar.
- A separate window has its own title bar and a **Return to desk** button.

## Why it works

SparkBox remembers where you put each window yourself. Tiling, cascading and snapping only arrange windows for now, which is why **Restore layout** can always put them back. When you close SparkBox and open it again, windows return to the places you chose, never to a tiled position. Layouts and window positions are kept on this Mac.

## Limits

- A layout saves windows on the desk. Separate windows aren't part of it.
- A layout brings back windows, not what ran in them. Terminal tabs in a layout's **Terminal** window don't reconnect by themselves. [Open a terminal on a node](help:open-a-terminal) explains how to reopen them.
- A window opens one copy at a time. Opening a window that's already open brings it to the front.

## If something goes wrong

- **The menu says "Tile all — fits 4 of 6 here".** The desk is too small to fit every window at a usable size. If you tile anyway, the windows that don't fit go to the dock. The menu may also offer **Hide sidebar and tile all (fits 6)**, which gives the windows the sidebar's room. **Restore layout** shows the sidebar again.
- **"Tiled 4 windows · 2 minimized".** Tile sent 2 windows to the dock because they didn't fit. Click **Undo** in the message, or click a window's name in the dock to bring it back.
- **"Stacked top and bottom: too narrow to sit side by side".** The desk is too narrow for two windows across, so **Side by side** stacked them. Click **Undo** in the message, or make the SparkBox window wider and try again.
- **"Needs two windows".** **Side by side** and **Tile 4** need at least two windows open and not minimized.
- **"Nothing has been tiled or cascaded".** **Restore layout** has nothing to undo yet.
- **"Open “Terminal” in a separate window?"** The window holds something that a move would lose, such as an open session or unsaved text. A separate window starts fresh, so SparkBox doesn't move it. Finish or save your work, then try again.
- **"Couldn't open a separate window".** macOS refused to make the new window. The message gives the reason. The window stays on the desk.
- **A window seems lost.** In the top bar, click **Windows** and click its name. If it's minimized, it's in the dock.

## Next steps

- [Windows and layouts](help:windows-and-layouts), to see every part of a window
- [Keyboard shortcuts](help:keyboard-shortcuts), to see every key for arranging windows
- [The command palette](help:command-palette), to run any of these commands by name
