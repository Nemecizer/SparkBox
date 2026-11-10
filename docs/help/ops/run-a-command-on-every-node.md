---
id: run-a-command-on-every-node
title: Run a command on every node
type: howto
area: ops
summary: Send one command to some or all nodes with Fleet Command, and compare the answers side by side.
keywords: [fleet, all nodes, broadcast command, as root, snippet, same thing on every node, same command on every node]
verified: false
related: [fleet-command, open-a-terminal]
---

# Run a command on every node

To run one command on several nodes and compare the answers, type it in **Fleet Command** and click **Run**.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. Every run needs it, even one that only reads. With **Demo** data the answers are simulated. [Unlock Admin mode](help:unlock-admin-mode) shows how.
- A **command** is an instruction typed for the node's shell, the program that reads commands, the same as in a [terminal](help:open-a-terminal).
- Validate the results on your own equipment before relying on them.

> [!WARNING]
> **Never type a password, token or key into a command.** It reaches the node's shell, and the command is saved in the activity log. SparkBox hides what looks like a secret, but it can't catch everything. If a command needs root, use **as root**: SparkBox takes the cluster password from the Keychain and passes it safely. If you've typed a secret by mistake, change that password or token.

## Steps

### Run a command that only reads

1. In the sidebar, under **Tools**, click **Fleet Command**. [Open Fleet Command](open:fleet) The **Fleet Command** window opens on its **Run** view.
2. Look at the row of nodes in the toolbar, after **on**. Every node that's online starts selected. Click a node to leave it out.


3. Click the command box. It may already hold a command: a new window starts with `uptime` there.
4. Press <kbd>⌘A</kbd> to select everything in the box, then type `uptime`. A badge beside it reads **read-only**, which means every part of the command only looks.
5. Click **Run**, or press <kbd>⌘↩</kbd>.
6. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
7. Read the output. It streams in from each node. When they're done, nodes that printed the same thing share one card. A node that answered differently has a **differs** badge.

To save typing, click a snippet under the box, such as **GPU** or **Disk**. It puts a ready-made command in the box.

### Run a command as root

**Root** is the account that can do anything on a node, and a mistake as root can damage the node. Use **as root** only when a command needs it, and try it on one node first.

1. Click the command box, press <kbd>⌘A</kbd> to select what's there, then type `id`. This command only prints who it's running as.
2. Click **as root**. The `$` at the start of the box changes to `#`, and the badge reads **runs as root**.
3. To try it safely, leave one node selected, such as `cell2`.
4. Click **Run**. A dialog named **Run on 1 node as root?** opens. On several nodes, you must type the count, such as `4 nodes`, to confirm.
5. Click **Run** in the dialog.
6. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
7. Read the output. It starts `uid=0(root)`. **as root** turns itself off after the run.

## What you should see

Each node's card has a badge: **exit 0** means the command worked. The bar above the cards shows the count, such as "4/4 ok". Each node's button in the toolbar shows how long it took.

`id` on its own counts as **read-only**. With **as root** it counts as a root command, so it asks first.

## Why it works

SparkBox reads each command first, sorts it by risk, and asks as carefully as that risk needs. A command it can't read counts as destructive. A destructive command runs on the first node alone, then asks before it goes on. The node's own `timeout` command enforces the **Time limit**, so a stuck command stops even if this Mac goes away. [Fleet Command](help:fleet-command#run) lists every class and what SparkBox asks for each.

## If something goes wrong

- **A card says "exit 1" or another number.** The command ran and reported a problem. Read its output in the card. To run it again on those nodes only, click **Re-run on failed (1)**.
- **A card says "stopped at the time limit".** The command ran longer than **Time limit**, 60 seconds unless you change it. Raise the limit, up to 3600 seconds, and run it again.
- **A card says "couldn't run".** SparkBox couldn't start the command on that node. The card has the reason. Check that the node is online.
- **A card has a "cut at 1 MiB" badge.** The command printed more than 1 MiB. SparkBox keeps the start.
- **"Pick at least one node to run on."** Every node is left out. Click a node in the toolbar.
- **"Run the restart on its own: SparkBox checks the kernel the node boots next just before it restarts."** Your command restarts a node and does something else too. Run the restart by itself, because the checks must be the last thing to touch the node.
- **"sudo on cell2 didn't accept the saved cluster password."** See [Connection and login messages](help:errors-connection#sudo-on-cell2-didnt-accept-the-saved-cluster-password).

To stop a run, click **Cancel all**.

## Next steps

- [Fleet Command](help:fleet-command), to see every part of the window
- [Open a terminal on a node](help:open-a-terminal), to work on one node
- [Find out what SparkBox did](help:find-what-sparkbox-did)
