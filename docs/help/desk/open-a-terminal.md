---
id: open-a-terminal
title: Open a terminal on a node
type: howto
area: tools
summary: Get a command prompt on a node, inside a container, or follow a log, from a tab in Terminal.
keywords: [ssh, shell, command line, console, terminal, logs, journal, container shell, command prompt, sudo, password prompt, password not showing, nothing appears when typing, can't see password, run as root, broadcast, same command on several nodes]
verified: false
related: [terminal, settings-terminal, unlock-admin-mode, ssh-sudo-and-host-keys, run-a-command-on-every-node]
---

# Open a terminal on a node

To run commands on a node, open a shell tab in the **Terminal** window. You can also open a shell inside a container, or follow a log.

## Before you start

A **shell** is a command prompt: you type a command, and the node runs it and prints the result. SparkBox opens it over SSH, the secure connection it uses for everything on a node.

- To open a shell, be ready to unlock Admin mode, with look-only mode off. A shell can change anything on the node. With Demo data, unlocking needs no password.
- Logs don't need Admin mode, because they only read.
- In look-only mode you can follow logs, but shells stay off. [Use look-only mode](help:use-look-only-mode) explains how to turn it off.
- With Live data, the node must be online: the sidebar shows its GPU use, such as **GPU 28%**. A node that's offline can't open a shell.
- With Demo data, the shell is simulated. Type `help` in it to see what it can do. Nothing you type reaches a real node.
- To run a command with [sudo](help:glossary#sudo), you need to know the cluster password. SparkBox keeps it in the Keychain, but it doesn't type it into a shell for you.

## Steps

### Open a shell on a node

1. In the sidebar, under **Tools**, click **Terminal**. [Open Terminal](open:terminal) The **Terminal** window opens. If it has no tabs yet, you see **Open a terminal** and a list of your nodes.
2. In the list, click the node you want, such as `cell1`. If Admin mode is locked, the row says **needs Admin**, and the **Unlock Admin mode?** dialog opens.
3. Click **Unlock…**, then confirm with Touch ID or your Mac password. With Demo data, there's nothing to confirm.
4. Wait a moment. A new tab opens with the node's name on it, and a prompt such as `sparkuser@cell1:~$` appears.
5. Type `uptime`, then press <kbd>Return</kbd>. The node prints how long it has been running.


To add a tab when others are open, press <kbd>⌘T</kbd>. The same list opens over the window, with a filter at the top.

### Run a command with sudo

Some commands need **root**, the account on a node that can do anything. **sudo** is a command that runs something as root, after it checks your password. Many fixes in Help start with `sudo`.

Root can change or delete anything, so read the whole command before you press <kbd>Return</kbd>. Nothing appears on screen while you type the password. That's normal, so keep typing.

1. Open a shell on the node, as in the steps above. You see a prompt such as `sparkuser@cell1:~$`.
2. Type `sudo whoami`, then press <kbd>Return</kbd>. This command changes nothing. The line `[sudo] password for sparkuser:` appears.
3. Type the cluster password. No letters, dots or stars appear, but the node receives each key.
4. Press <kbd>Return</kbd>. The node prints `root`, and the prompt comes back.

Type the password only at that line. Never add it to the command itself, because the shell saves your commands in its history.

sudo may remember you for a while in that shell. If a later command runs without asking, nothing is wrong.

With Demo data, sudo never asks for a password, so you can't practice this part there. Commands that the simulated shell knows, such as `sudo uptime`, print their answer straight away.

### Open a shell inside a container

A **container** is a program running in its own sealed box on a node. A model engine usually runs in one. A shell inside a container runs as root in there, so take care with what you type.

1. In the **Terminal** toolbar, click the small arrow next to **New shell**. A menu lists your nodes.
2. Point to the node, then point to **Container shell**. A list of its running containers opens.
3. Click the container's name. If Admin mode is locked, confirm the unlock as in the steps above. A tab opens with a prompt inside the container.

### Follow a log

A log tab shows what a program prints as it happens. It only reads, so it can't change anything.

1. In the **Terminal** toolbar, click the small arrow next to **New shell**.
2. Point to the node.
3. To follow a container, point to **Logs**, then click the container. To follow a system service, point to **Journal**, then click the service. The **journal** is the system's own log, kept for each service.
4. Read the new tab. A bar above the text shows **Following · read-only**. New lines appear as they're written.
5. To stop new lines while you read, click **Pause**. Click **Follow** to catch up.

### Send the same keys to several shells

**Broadcast** sends what you type to several shells at once. For one command on several nodes, [Run a command on every node](help:run-a-command-on-every-node) is the better tool. It checks the command first and shows each node's result in a table.

Don't use **Broadcast** for `sudo`. If one shell isn't waiting for a password, what you type lands on its command line and stays in its history. To run a command as root on several nodes, use **as root** in [Fleet Command](help:fleet-command), where SparkBox gives the password privately.

> [!DANGER]
> **A command sent with Broadcast runs in every selected shell, and you can't undo it or call it back.** One that's safe on one node can do harm on four. Stop if you aren't sure what it does on each node.

1. Open a shell on each node you want to reach, as in the steps above. Each shell has its own tab.
2. In the **Terminal** toolbar, click **Broadcast**. A checkbox appears on each shell tab, and the toolbar says how many shells receive, such as "Broadcasting to 3".
3. To leave a shell out, deselect the checkbox on its tab. The count in the toolbar goes down by one.
4. Click a selected tab. An orange line around the text shows that this tab is receiving.
5. Type your command, but don't press <kbd>Return</kbd> yet. The same keys go to every selected shell.
6. Read the command, then press <kbd>Return</kbd>. It runs in every selected shell.
7. In the toolbar, click **Broadcast** again. The checkboxes and the count go away, and each tab takes its own typing again.

### Open a shell from somewhere else

You don't have to start in **Terminal**:

- Press <kbd>⌘T</kbd> anywhere. SparkBox opens a shell on the node whose window is in front, or on the first node online.
- Right-click a node in the sidebar, and click **Terminal**.
- In **Files**, right-click a folder on a node and click **Open terminal here**. The shell starts in that folder.
- In **Containers**, select a container and press <kbd>⌘T</kbd> for a shell in it, or <kbd>⌘L</kbd> for its logs.
- Click the menu bar icon, point to a node, and click **Terminal**.

## What you should see

The tab shows a green dot and the node's name. Move the pointer over the tab to see what it is, the account it uses, and how long it has been connected, such as `Shell on cell1 · sparkuser@192.168.50.51 · 20 ms · connected 12s`.

Your command's output appears under the prompt. After `sudo whoami`, that output is `root`. In a log tab, the bar shows **Following · read-only** and the newest lines arrive at the bottom.

With **Broadcast** on, the toolbar shows a count such as "Broadcasting to 3", and each selected shell shows what you typed and its own result.

## Why it works

Each tab is its own SSH session, logged in with the method you set up in [Settings › Cluster login](help:settings-cluster-login). What you type goes straight to the node, and the node does the work. The activity log records that a tab opened and closed, but never what you typed. sudo's password line belongs to the node, and it hides what you type from anyone watching your screen. **Broadcast** passes your keys to each selected session, so each node runs the command for itself.

## Limits

- A shell is one session. If the connection drops, whatever was running in it stops too. Downloads and copies started from **Models** run by themselves on the node, so they don't have this limit.
- The **Terminal** window closes every session when it closes. A minimized window keeps them.
- When SparkBox quits, every session ends. Next time, it offers to reopen your tabs. Nothing connects until you choose.

## If something goes wrong

- **"Sorry, try again."** sudo didn't accept the password. This message comes from the node. Type the cluster password again, then press <kbd>Return</kbd>.
- **"sudo: 3 incorrect password attempts"** sudo stopped after three wrong tries, and the command didn't run. Run the command again.
- **"sudo: command not found (demo mode: try `help`)"** The simulated shell has no answer for that command. This message appears only with Demo data, and nothing is wrong.
- **"Locked — authenticate to keep typing."** The shell stopped taking keystrokes, but its session is still open. Click **Authenticate…**. [Locked shells](help:terminal#locked-shells) explains why it happens.
- **"This shell opens once Admin mode is unlocked."** The tab is waiting for you. Click **Unlock…** in the banner.
- **"Connection lost. Reconnecting in 4 s (attempt 3)".** The link to the node dropped. SparkBox tries up to 12 times, waiting longer each time. Click **Reconnect now** to try at once. The new session starts fresh, so anything that was running is gone.
- **"Connection lost. Reconnecting this shell needs Admin mode."** Admin mode locked while the connection was down. Click **Unlock…**.
- **"[Stopped trying to reconnect. Press Return or use Reconnect]"** SparkBox gave up after 12 tries. Check that the node is online, then press <kbd>Return</kbd> in the tab.
- **"[Press Return to try again]"** The tab couldn't connect, and the line above it says why. [Connection and login messages](help:errors-connection) lists what each reason means.
- **"cell3 is offline".** The node can't be reached, so its menu is off. [A node shows Offline](help:troubleshooting#a-node-shows-offline) shows what to check.
- **"No containers are running".** The node has no running container, so there's nothing to open a shell in.
- **"Look-only · turn off in Settings › Security & privacy".** Look-only mode is on, so shells are off. Logs still work.

More messages are in [Files and Terminal messages](help:errors-files-and-terminal).

## Next steps

- [Terminal](help:terminal), to see every part of the window
- [Settings: Terminal](help:settings-terminal), to change the font, the theme and when idle shells lock
- [Run a command on every node](help:run-a-command-on-every-node), to run the same command on many nodes
