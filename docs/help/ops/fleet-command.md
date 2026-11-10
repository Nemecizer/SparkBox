---
id: fleet-command
title: Fleet Command
type: feature
area: ops
summary: Run a command on several nodes at once, and compare versions across the cluster.
keywords: [fleet, versions, snippets, canary, as root]
verified: false
related: [run-a-command-on-every-node]
---

# Fleet Command

## What it's for

**Fleet Command** runs one command on several nodes and shows the answers together, so differences stand out. Its **Versions** view compares what each node runs. Validate the results on your own equipment before relying on them.

## Where to find it

- **Sidebar:** under **Tools**, click **Fleet Command**.
- **Command palette:** press <kbd>⌘K</kbd> and type a snippet's name, such as `GPU`, for **Run GPU snippet on all nodes**.
- **Link:** [Open Fleet Command](open:fleet)

## What's on the screen


The toolbar's first choice switches between **Run** and **Versions**.

### Run

**Nodes.** After **on**, one button for each node. Those that are online start selected. Click one to leave it out. During a run, each button shows how that node is doing.

**How it runs.** See [How it runs](#how-it-runs).

**The command box.** Type a command. The start of the box shows `$`, or `#` when it runs as root. <kbd>↑</kbd> and <kbd>↓</kbd> bring back recent commands.

**The badge.** As you type, a badge says what SparkBox makes of the command. That class decides how carefully it asks. It fails safe: a command it can't read counts as destructive. Move the pointer over the badge for the reason.

| Badge | What it means | What SparkBox asks |
|---|---|---|
| **read-only** | Every part only looks | Nothing. It runs at once |
| **changes things** | It isn't on the list of read-only commands, so it may change something | A dialog that shows what's on the nodes |
| **runs as root** | It runs as root, with **as root** or `sudo` or through Docker | The same dialog. On several nodes, type the count, such as `4 nodes` |
| **destructive** | It can destroy data or take a node down | A red dialog. Type the node's name, or the count on several nodes |
| **cuts access** | It's destructive, and SparkBox may not get back in: it stops SSH, the network or the firewall, or changes login files | The red dialog, and SparkBox logs in again after each node |
| **takes nodes down** | It restarts or shuts down the node | The red dialog, and SparkBox follows each node away and back |

A tool that usually only looks reads as **changes things** when one of its options could write, such as `blkid -c <file>`, `apt-cache gencaches` or `tree -o <file>`. So does one with a word the shell fills in, such as `find . $(cat list)`, because SparkBox can't see what it will be. Tools with no writing option, such as `cat` and `ls`, stay read-only.

A destructive command never runs on several nodes at once. It runs on the first node alone, then asks "Continue on 3 more?". The rest go one at a time, and stop at the first failure.

**Run** starts it, or press <kbd>⌘↩</kbd>. Every run needs Admin mode. While it runs, this button becomes **Cancel all**. **Save** keeps the command as a snippet.

### as root

Runs the command as **root**, the account that can do anything on a node. SparkBox takes the cluster password from the Keychain and passes it to `sudo` privately. Never type a password into a command.

**as root** turns itself off after each run. To keep it on, click the pin beside it.

### Time limit

The longest the command may run, in seconds, from 1 to 3600. The default is 60. The node enforces it, so the command stops there even if this Mac goes away. SparkBox sends a stop request at the limit, and forces it 5 seconds later.

### How it runs

- **Parallel** runs on every node at once.
- **One at a time** goes node by node, in order.
- **Canary first** runs on the first node alone, and on the rest only if that worked. The first node is the canary, the one that tests the way.

SparkBox overrides this choice for risky commands. A destructive command, or one that cuts access, always goes one node at a time and stops at the first failure. So does a root Docker command on several nodes.

### Stop if a node fails

With **One at a time**, the run stops at the first node that fails, and the rest are skipped. It's off for **Parallel**, where every node starts together.

### Snippets

Saved commands, under the box. Click one to put it in the box, then click **Run**. A run from a snippet carries the snippet's name in [Tasks](help:tasks).

| Snippet | What it shows |
|---|---|
| **GPU** | Each GPU's name, driver, load, temperature and power |
| **Uptime** | How long each node has been on |
| **Disk** | How full the disks are |
| **Memory** | How much memory is free |
| **Containers** | The running Docker containers |
| **Updates** | The packages that can be upgraded |
| **Kernel** | The kernel version |
| **Fabric links** | Each fabric port's state and speed |

**Save** adds your own, named after the command's first three words. They're kept on this Mac. The **×** on a saved snippet forgets it, and you can undo that.

**History** at the right lists your last 50 runs, with the nodes and how many worked. **Re-run** runs one again, through the same questions. **Re-run on failed (1)** runs it only on the nodes that failed. Demo and Live keep separate histories.

### The results

Output streams in as it arrives. When nodes finish, those that printed exactly the same thing share a card. A card that isn't the most common answer has a **differs** badge. Each card's badge says how the command ended:

| Badge | Meaning |
|---|---|
| **exit 0** | It worked |
| **exit 1**, or another number | It ran and reported a problem |
| **couldn't run** | SparkBox couldn't start it there |
| **stopped at the time limit** | It ran past **Time limit** |
| **skipped** | It wasn't run on this node, and the card says why |
| **canceled** | You stopped it |

Output past 1 MiB is cut, and the card says **cut at 1 MiB**. SparkBox keeps the start. Text that looks like a secret is hidden.

The bar above the cards has:

- **Re-run on failed (1)**, after a run with failures.
- **Table**, which shows output as a table when every node prints comma-separated values, as the **GPU** snippet does.
- **Wrap**, which folds long lines.
- **Copy all**, which copies every node's output as Markdown.
- **Each node**, which gives every node its own card, even when they match.

### Versions

Compare the nodes' reported versions without running a Fleet command. Each row and the whole table have a verdict:

| Verdict | What it means |
|---|---|
| **Matching** | At least two comparable nodes reported the same value, with none missing |
| **Mismatched** | At least two known values differ, even if other nodes have unknown readings |
| **Unknown** | A reading is missing or stale, a node is offline, or the values cannot be compared |

A difference between two nodes is enough to show **Mismatched**. Unknown rows stay in the table. These verdicts describe agreement; matching versions alone do not prove that a model will work.


The table includes **DGX OS (OTA)**, **Kernel**, **NVIDIA driver**, **CUDA (driver maximum)**, **NVIDIA container toolkit**, **Collector build**, **Docker image store**, **Serving image ID** and **NCCL (declared)**. It also shows **OS**, **BIOS**, **ConnectX-7 firmware**, **Docker** and **Clock synced**.

**CUDA (driver maximum)** is the CUDA version the driver reports supporting. It does not identify the CUDA libraries inside a model's image. **Collector build** is a short fingerprint of the collector's loaded source. **BIOS** compares nodes of the same hardware model; a model with one node has no peer.

**Serving image ID** uses the full identity from running vLLM or SGLang containers. An image tag alone cannot establish a match. No running serving container, several different serving images, or an unreadable identity gives **Unknown**. Image IDs can be compared only when the nodes report the same Docker image store.

**NCCL (declared)** reads the container's `NCCL_VERSION` declaration. Missing or conflicting declarations give **Unknown**. SparkBox does not start a container to find the library version, and a declaration does not prove which library a process loaded.

**Copy as table** copies full values, verdicts and unknown reasons as Markdown. Move the pointer over a shortened image ID to see it in full. Demo readings are simulated.

## What it changes

**Versions** changes nothing, and it needs no Admin mode. A run changes whatever the command changes. SparkBox can't undo a command for you, so the undo depends on the command.

| Tier | What SparkBox does before it runs |
|---|---|
| Read | Runs at once |
| Change, or as root | Shows what's on the nodes. As root on several nodes, type the count, such as `4 nodes` |
| Root through Docker, on several nodes | The same, and the first node runs alone, then the rest one at a time |
| Destructive | A red banner, and you type the node's name, or the count. On several nodes, the first runs alone, then it asks "Continue on 3 more?" |
| Cuts access | The same, and SparkBox logs in to each node again before the next. It stops at the first it can't reach |
| Takes nodes down | The same, and the checks before a restart run on each node. SparkBox follows each one away and back, and its alerts page nobody |

A command SparkBox can't read counts as destructive. It says why: "SparkBox can't read this command's quoting, so it treats it as destructive." or "The command it runs comes from a substitution or a variable, so SparkBox can't tell what it is; it treats it as destructive."

A typed restart is built but hasn't run on your cluster. [Restart a node](help:restart-a-node) is the better way, and buttons for it appear under the box when you type one.

Every run is a [task](help:tasks), and it's in the [activity log](help:activity-log). Stopping a run needs Admin mode too.

## Related How-Tos

- [Run a command on every node](help:run-a-command-on-every-node)
- [Open a terminal on a node](help:open-a-terminal)

![Fleet Command, using synthetic Demo data](img/public-fleet-command.jpg)
