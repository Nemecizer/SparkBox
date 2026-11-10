---
id: workloads
title: Workloads
type: feature
area: ops
summary: The processes and listening ports on every node, and what each belongs to.
keywords: [processes, ports, priority, nice, signal, pid]
verified: false
related: [see-and-stop-processes, open-the-dgx-dashboard]
---

# Workloads

## What it's for

**Workloads** shows what's running on your nodes. Use it to find what's using a node's CPU, GPU or memory, or to stop a process. It also shows which programs answer on which ports.

A **process** is one running program. A **port** is a numbered door on a computer that a program answers on. [Services and processes](help:services-and-processes) explains the first idea.

## Where to find it

- **Sidebar:** under **Workloads**, click **Workloads**.
- **Menu bar:** choose **Go › Workloads**.
- **Keyboard:** press <kbd>⌘4</kbd>.
- **Command palette:** press <kbd>⌘K</kbd> and type `workloads`.
- **Link:** [Open Workloads](open:workloads)

## What's on the screen


The toolbar's first choice switches between two views: **Processes** and **Listening ports**. In both, the **All nodes** list limits the view to one node, and **Filter** narrows it by text.

### Processes

One row for each process, sorted by **CPU**. A narrow window hides some columns, so make it wider to see them all. The list refreshes every 5 seconds. It holds still while your pointer is over it or a row is selected, so that nothing moves under your click.

The list shows up to the 200 busiest processes on each node, by CPU and then memory. Model servers, GPU users, programs on a port and system services are always included.

| Column | What it shows |
|---|---|
| **Node** | Which node it's on |
| **PID** | The number the node gave this process. PID means process ID |
| **Process** | Its name, with a tag if it's SparkBox's own or a kernel thread. Move the pointer over it for the whole command |
| **Command** | The whole command line. Values that look like secrets are masked |
| **User** | The account it runs as |
| **State** | **running**, **sleeping**, **waiting on disk**, **paused**, **idle** or **zombie**. A zombie has ended, but it's still listed |
| **CPU** | Its share of one CPU core. 200% means two whole cores |
| **Memory** | How much unified memory it holds |
| **GPU memory** | How much memory the GPU holds for it |
| **Runs in** | Its container or service |
| **Ports** | The ports it listens on |
| **Threads** | How many threads it runs. You can hide this column |
| **Nice** | Its priority number. See [Priority](#priority) |

A dimmed row means one of three things. "unknown · cell3 offline" means the node isn't reporting, and this is its last-known list. "exited · last seen 12 s ago" means a process you selected has gone; it stays for a minute so you can read it. "not among the 200 busiest · last seen 12 s ago" means a process you selected dropped out of the list. It may still be running.

The toolbar shows names when the window is wide enough. A new **Workloads** window names **Stop**, **Signal** and **View**. **Logs** and **Shell** are icons, a page and a square with a prompt, and **Force stop** is under the **More** button (**…**) at the end of the toolbar. In a narrower window every button is an icon, and **Stop** is a square in a circle.

The toolbar on this view has:

- **Stop**, **Force stop** and **Signal**, which act on the selected process. See [Signals](#signals).
- **Logs**, which opens the log of the process's container or service in a Terminal tab.
- **Shell**, which opens a shell inside the process's container. It needs Admin mode.
- The list that starts **Busiest + GPU + listening**. It can narrow the list to **Using the GPU** or **Listening on a port**.
- **View**, which can also show SparkBox's own processes and kernel threads. Both are hidden unless you ask.

Press <kbd>Return</kbd> or double-click a row to open its details: its container's [Inspect](help:inspect-window) window, its service in [Services](help:services), or its full command line here. Right-click a row for every action, including **Copy PID**, **Copy command line** and **Reveal command line**.

### Runs in

This column says what a process belongs to: a container, with a box icon, or a service, with a service icon. If the container's name is unavailable, its short ID appears instead. A dash means neither.

It matters when you stop something. A process in a container stops with its container. A process in a service may be started again by the system, so stop the service instead.

### Signals

A **signal** is a short message the system delivers to a program. **Stop** sends TERM. **Force stop** sends KILL. **Signal** offers them all:

| Choice | What it does |
|---|---|
| **TERM — stop cleanly** | Asks it to shut down cleanly |
| **INT — interrupt (Ctrl-C)** | Interrupts it, like <kbd>⌃C</kbd> in a terminal |
| **HUP — reload** | Usually makes a background program reload its settings |
| **KILL — force stop** | Ends it at once, with no chance to clean up |
| **STOP — pause** | Pauses it until it gets CONT |
| **CONT — resume** | Resumes a paused process |

Each one asks first, except CONT. Root's processes are signalled with sudo. SparkBox first checks that the process started at the same time as the one you selected, so a reused number is never hit.

After a stop, SparkBox watches for 10 seconds and tells you whether the process went.

### Priority

The bar above the list shows the selected process, with its **Priority (nice)**. This number decides who gets the CPU when a node is busy. −20 is the highest priority, and 19 is the lowest.

Use **−** and **+**, or type a number, then click **Apply**. These controls stay dimmed until you [unlock Admin mode](help:unlock-admin-mode). They show no lock, and a click on them does nothing. Raising the priority, which means a lower number, needs sudo. **Details** opens the process's user, state, threads, ports and full command line.

### Listening ports

One row for each open TCP port. The same port on IPv4 and IPv6 is one row.

| Column | What it shows |
|---|---|
| **Node** | Which node it's on |
| **Port** | The port number |
| **Listening on** | The address it listens on |
| **Reachable** | Who could reach it: **This node only**, **LAN**, **Fabric** or **All interfaces** |
| **Usually** | What that port number usually means |
| **Owner** | The program that owns it |

Only root can see who owns every port, so SparkBox works it out from the process list. "nginx (likely)" means an nginx process runs on that node and 8080 is its usual port. Move the pointer over it to see how it matched.

**Reveal with sudo** asks each node, as root, for the real owners. It needs Admin mode, and it changes nothing.

**Open in browser** opens the selected port in your browser through an SSH tunnel, a private path through your connection to the node. That works even when the port listens only on the node itself. You can also press <kbd>Return</kbd> or double-click a row. It works for any listed port but SSH's and DNS's. With **Demo** data nothing opens: SparkBox says what it would do with **Live** data.

## What it changes

Looking changes nothing, and it needs no Admin mode.

| Action | Changes | Needs | Undo |
|---|---|---|---|
| **Stop**, **Force stop**, **Signal** | Stops or pauses a process on the node | Admin mode | None. Start the program again. CONT undoes STOP |
| **Apply** priority | The process's priority, until it ends | Admin mode, and sudo to raise it | Apply the old number |
| **Reveal with sudo** | Nothing | Admin mode | None needed |
| **Open in browser** | Opens a tunnel from this Mac | Admin mode | It closes when Admin mode locks |
| **Shell** | Whatever you type there | Admin mode | Depends what you do |
| **Reveal command line** | Nothing. It copies the full command line | Admin mode | None needed |

## Related How-Tos

- [See and stop processes](help:see-and-stop-processes)
- [Open the DGX Dashboard](help:open-the-dgx-dashboard)
- [Manage containers](help:manage-containers)

![Workloads, using synthetic Demo data](img/public-workloads.jpg)
