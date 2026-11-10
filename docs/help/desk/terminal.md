---
id: terminal
title: Terminal
type: feature
area: tools
summary: Tabs of real command-line sessions on your nodes, with broadcast, search and read-only log tabs.
keywords: [shell, ssh, broadcast, logs, journal, locked, authenticate, reconnect, scrollback]
verified: false
related: [open-a-terminal, settings-terminal, admin-mode, fleet-command]
---

# Terminal

## What it's for

**Terminal** gives you command-line sessions on your nodes, each in its own tab. Use it to run commands, look around a node, or watch a log as it's written.

A tab is one of four things:

| Tab | What it is | Needs Admin mode |
|---|---|---|
| **Shell** | A command prompt on a node | Yes |
| **Container shell** | A command prompt inside a container, as root there | Yes |
| **Container logs** | What a container prints, read-only | No |
| **Journal** | A service's log from the system journal, read-only | No |

For the same command on many nodes, [Fleet Command](help:fleet-command) is usually better. It shows each node's result in a table.

## Where to find it

- **Sidebar:** under **Tools**, click **Terminal**.
- **Menu bar:** choose **Go › Terminal**.
- **Keyboard:** press <kbd>⌘6</kbd>. Press <kbd>⌘T</kbd> for a new shell, on the node in front or the first node online.
- **Command palette:** press <kbd>⌘K</kbd>, type `terminal`, and press <kbd>Return</kbd>.
- **Any node:** right-click a node anywhere, and click **Terminal**.
- **Files:** right-click a folder on a node, and click **Open terminal here**.
- **Menu bar icon:** click it, point to a node, and click **Terminal**.
- **Link:** [Open Terminal](open:terminal)

## What's on the screen

### Open a terminal

With no tabs open, the window lists your nodes with their address and delay, then your **Recent sessions**. Click a row to connect. **needs Admin** marks a row that asks you to unlock first.

If you had tabs open when SparkBox last quit, a line offers **Reopen** and **Not now**. A message says the same soon after SparkBox starts. Nothing connects until you choose.

When tabs are open, <kbd>⌘T</kbd> shows the same list over the window. Type to filter it, use <kbd>↑</kbd> <kbd>↓</kbd> to choose, and press <kbd>Return</kbd> to connect.

### Toolbar

| Button | What it does |
|---|---|
| **New shell** | Opens a shell on the node you used last. The arrow beside it lists every node, with **Shell**, **Container shell**, **Logs** and **Journal** |
| **Broadcast** | Sends what you type to several shells. See [Broadcast](#broadcast) |
| **Find** | Searches the tab's text. <kbd>⌘F</kbd> does the same |
| **Clear** | Clears the tab's text. <kbd>⌥⌘K</kbd> does the same |
| **Reconnect** | Opens a closed tab's session again |
| **Terminal settings** | Opens [Settings › Terminal](help:settings-terminal) |

In a narrow window, some buttons move into **More**.

### Tabs

Each tab shows an icon for its kind, a dot for its connection, and a name.

| You see | It means |
|---|---|
| Green dot | Connected |
| Pulsing amber ring | Connecting or reconnecting |
| Empty ring | Closed |
| Padlock | Locked. See [Locked shells](#locked-shells) |
| Bell | The shell rang its bell while you were on another tab |
| Small dot at the end | New output while you were on another tab |

To work with tabs:

- Click to switch. <kbd>⇧⌘[</kbd>, <kbd>⇧⌘]</kbd> and <kbd>⌃Tab</kbd> move between them.
- Double-click a name to rename the tab. Leave it empty to go back to the name the shell sets.
- Drag a tab to move it.
- Click its **×**, press <kbd>⌘W</kbd>, or middle-click it to close it.
- Move the pointer over it to see what it is, the account it uses and how long it has been connected.

If a command printed something in the last few seconds, closing that tab asks first: "It's running a command (output in the last few seconds). Closing the tab ends it."

A line at the bottom of the window lists the main keys, and whether the Option key types characters or acts as Meta.

### The terminal area

This is the session itself. Select text with the mouse, and press <kbd>⌘C</kbd> to copy and <kbd>⌘V</kbd> to paste. Click a web address to open it in your browser. Right-clicking selects a word and opens the usual **Copy** and **Paste** menu.

A command that starts with [sudo](help:glossary#sudo) asks for the cluster password in the shell, and nothing appears as you type it. SparkBox doesn't type it for you. [Run a command with sudo](help:open-a-terminal#run-a-command-with-sudo) shows what to expect.

With Demo data, the shell is simulated. It knows common commands such as `ls`, `cd`, `df -h`, `nvidia-smi` and `docker ps`. Type `help` for a list. Files you see in it are the same simulated files that **Files** shows.

### Find

<kbd>⌘F</kbd> opens a bar over the text. Type in **Find in scrollback**, and every match is marked with a count such as "3 of 12".

| Control | What it does |
|---|---|
| **Aa** | Matches capitals exactly |
| `.*` | Treats what you type as a regular expression, a pattern language |
| Up and down arrows | Previous and next match. <kbd>⇧Return</kbd> and <kbd>Return</kbd> do the same, and so do <kbd>⇧⌘G</kbd> and <kbd>⌘G</kbd> |
| **×** | Closes the bar. <kbd>Esc</kbd> does the same |

### Log tabs

A log tab has its own bar above the text:


| Control | What it does |
|---|---|
| **Pause** | Holds new lines so that you can read. The bar counts what's waiting, such as "Paused · 1.2 MiB waiting". **Follow** shows them and keeps going |
| **Wrap** | Wraps long lines. **No wrap** keeps each on one line, and you scroll sideways |
| **From** | How far back to start: **Last lines**, **10 minutes**, **1 hour**, **6 hours** or **24 hours**. A journal also offers **This boot**. Changing it restarts the tab |
| **Highlight…** | Marks every line containing what you type, as new lines arrive |
| **Follow again** | Appears when the stream has ended but the container still runs |
| **Save to Downloads** | Saves the tab's text to a new file in your Downloads folder |

**Last lines** means the last 500 lines of a container, or the last 300 of a journal.

On the right, the bar shows the state: **Following · read-only**, **Connecting…**, **Paused**, **End of log** or **Stopped**. A log tab runs `docker logs` for a container and `journalctl` for a service, with no input. So <kbd>⌃C</kbd> can't stop anything in it.

If a tab can't keep up, or you pause for a long time, SparkBox keeps the newest 2 MiB and tells you what it dropped: "… 3.2 MiB skipped while paused …".

When a stream ends, the last line says why, such as "— End of log · glm53 exited 2 days ago —" or "— The log stream ended · vllm-head is still running —".

### Broadcast

**Broadcast** sends what you type to several shells at once. [Send the same keys to several shells](help:open-a-terminal#send-the-same-keys-to-several-shells) has the steps.

> [!DANGER]
> **A command sent with Broadcast runs in every selected shell, and you can't undo it or call it back.** One that's safe on one node can do harm on four. Stop if you aren't sure what it does on each node, and turn **Broadcast** off when you finish.

While it's on, you see:

| You see | It means |
|---|---|
| A checkbox on each shell tab | A selected shell receives what you type. Deselect it to leave that shell out |
| "Broadcasting to 3" in the toolbar | How many shells receive right now |
| An orange line around the text | The tab in front is receiving |

What you type in a deselected tab goes to that tab only. Log tabs, locked shells and tabs that aren't connected never receive. If you paste more than one line, SparkBox asks first: "Paste 5 lines into 3 shells?". The activity log notes that broadcast started and which shells it reached, but never what you typed.

### Locked shells

A shell can stop taking keystrokes while its session stays open. A banner says **Locked — authenticate to keep typing.**, with the reason under it, and the tab shows a padlock.

Anything already running keeps running, and its output keeps appearing. What you type while it's locked isn't sent, and the banner says so. Click **Authenticate…** to type again. With Live data, that always asks for Touch ID or your Mac password. The session is the same one, so nothing is lost.

| What happens | With Live data | With Demo data |
|---|---|---|
| You lock Admin mode | Locks | Locks |
| The screen locks, or the Mac sleeps | Locks | Locks |
| Admin mode locks itself because nothing changed for its time limit | Locks | Stays open |
| You close the SparkBox window to the menu bar | Locks | Stays open |
| Nothing is typed for the time in [Lock idle shells after](help:settings-terminal#shells) | Locks, after 8 hours at most | Locks, unless set to **Never** |

So with Live data, a shell locks whenever Admin mode does. The reason is that your account can use Docker, which can do anything on the node. Typing in a shell doesn't count as a change, so it doesn't keep Admin mode unlocked.

A minute before Admin mode locks itself, a message says "Admin mode locks in 1 min" and "With Live data, open shells stop taking keystrokes when it locks. They stay connected." To type again, you confirm with Touch ID or your Mac password. Click **Keep unlocked** in that message to give yourself more time.

A shell in a container runs as root in there, so it locks with Admin mode even with Demo data.

A minute before an idle lock, a banner says "This shell locks in 60 s: nothing typed for 59 min. Type anything to keep it open."

### When the connection drops

If the node goes away, the tab shows "Connection lost. Reconnecting in 4 s (attempt 3)". SparkBox tries up to 12 times, waiting a little longer each time, up to 30 seconds. **Reconnect now** tries at once.

A reconnected shell is a new session, marked **[Reconnected]**. Whatever ran in the old one has stopped, but the old text stays on screen.

When a shell ends by itself, such as after you type `exit`, the tab says "[exited with code 0. Press Return to reconnect]". A shell needs Admin mode to reconnect, and the banner has **Unlock…** if it's locked.

## What it changes

| Action | What it changes | Can you undo it? |
|---|---|---|
| Opening a log tab | Nothing. It only reads | Nothing to undo |
| Typing in a shell | Whatever your commands change on the node | Only as far as the command itself allows |
| Typing with **Broadcast** on | The same, on every selected shell's node | Only as far as the command itself allows, on each node |
| **Save to Downloads** | Makes a new file on this Mac | Delete the file |
| **Clear** | Removes the text from the tab only | No. The node isn't affected |

A shell runs as your account, `sparkuser`, on the node. A shell in a container runs as root in there. Admin mode is required to open either one, and SparkBox doesn't check what you type.

The activity log has one entry for each tab, from open to close, and never your keystrokes. **Terminal** closes every session when its window closes. Minimizing it keeps them.

## Related How-Tos

- [Open a terminal on a node](help:open-a-terminal)
- [Run a command on every node](help:run-a-command-on-every-node)
- [Unlock Admin mode](help:unlock-admin-mode)

![Terminal, using synthetic Demo data](img/public-terminal.jpg)
