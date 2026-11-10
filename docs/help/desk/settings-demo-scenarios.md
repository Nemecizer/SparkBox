---
id: settings-demo-scenarios
title: "Settings: Demo scenarios"
type: feature
area: settings
summary: Make the simulated cluster act out a failure, so that you can practice before it happens for real.
keywords: [scenario, simulate, failure, practice, demo, fault, drill]
verified: null
related: [demo-and-live, tour-the-demo, respond-to-an-alert]
---

# Settings: Demo scenarios

## What it's for

A **scenario** makes the simulated cluster act out a problem, such as a node going offline or a disk filling up. You see the same alerts, colors and messages you'd see on a real cluster, with nothing at risk. Use scenarios to learn what trouble looks like, and to practice your response.

Scenarios run only with Demo data. Your real cluster is never touched.

## Where to find it

- **Sidebar:** click **Settings**, then click **Demo scenarios**.
- **Menu bar:** choose **Cluster › Demo Scenarios**.
- **Command palette:** press <kbd>⌘K</kbd> and type `demo scenario`.
- **Link:** [Open Demo scenarios](open:settings?section=scenarios)

With Live data, this page says "Scenarios play only in the simulator. Switch to Demo in Data source to try them; your real cluster is never touched."

## What's on the screen


### Scenario

One row for each scenario. Click a row to play it, or use <kbd>↑</kbd> <kbd>↓</kbd>. One plays at a time. In look-only mode the rows don't respond. See [What it changes](#what-it-changes).

Each row shows:

| Part | Meaning |
|---|---|
| Title and description | What happens |
| **Active**, **Fault**, **Recovering** or **Healthy** | On the row that's playing, where it is in its cycle |
| "next in 0:42" | Time until the next stage |
| "Every 2 min" | How often it plays again. A row without this stays as it is until you change it |
| Name tags | What it touches, such as `cell3` or `crs804` |

Many scenarios break something, hold it, let it recover, and then play again.

While a scenario plays, the top bar shows a pill such as "Node offline · fault · recovers in 0:59". Click it to come back here.

### Back to everyday serving

This button returns the simulator to the everyday world.

### The scenarios

**Starting points:**

| Scenario | What you see |
|---|---|
| **Your cluster at rest (Oct 29)** | Your four nodes as SparkBox found them on Oct 29: nothing serving, a past run that exited, and Ollama on two nodes |
| **Everyday serving** | Models serving across the cluster, and one failed service on `cell1`. This is the default |
| **All healthy** | Everything up and nothing failed |

**Nodes:**

| Scenario | What you see |
|---|---|
| **Node offline** | `cell3` freezes for a minute, then recovers |
| **Slow node** | `cell2` takes about 2 seconds to answer, then recovers |
| **Collector stalled** | `cell1`'s numbers stop for 30 seconds, then resume |
| **Disk filling** | `gx10-0000`'s disk fills to 97%, then a log is cleaned up |
| **Thermal throttling** | `cell1`'s GPU runs hot and slows down, then cools |
| **Out of memory** | Ollama on `gx10-0000` uses too much memory, and the system kills it |
| **GPU Xid error** | `gx10-0000`'s GPU drops off the system. Its readings stop, and the alert stays open until the node restarts |
| **Sudo password rejected** | Everything that needs sudo fails, as if the saved password were out of date |

**Models and containers:**

| Scenario | What you see |
|---|---|
| **Crash loop** | `vllm-worker` on `cell2` keeps crashing, then stays up |
| **Model out of memory** | A new load runs out of GPU memory |
| **Gated model refused** | Hugging Face refuses a gated model |
| **Ray worker can't join** | A several-node load gives up waiting for a worker |
| **Copy fills a disk** | A fabric copy stops when the destination disk is full |
| **Endpoint slow** | A model answers, but slowly |
| **Endpoint stalled** | A model stops producing text, and it goes **Down** |

**The fabric and switches:**

| Scenario | What you see |
|---|---|
| **Fabric link flapping** | `cell2`'s 200G link keeps dropping for 2 minutes |
| **Degraded optic** | One lane on a CRS804 port fades, and errors appear |
| **Switch port down** | A CRS804 port goes down for a minute |
| **Fabric congestion** | Heavy traffic between `cell1`, `cell2` and `cell3` slows senders down. Nothing is dropped |
| **RoCE packet loss** | One of `cell2`'s fabric links starts losing packets, and the link drops once |
| **PFC pause storm** | `gx10-0000` floods its link with pause signals for 90 seconds, stalling other traffic |
| **Fabric lost** | `cell2` loses both fabric links for two minutes while it stays online on the LAN |
| **Cables swapped** | `cell2`'s and `cell3`'s fabric cables are in each other's ports |
| **MTU mismatch** | One of `gx10-0000`'s fabric links has a smaller packet size than the rest, so large packets to it fail |

[The fabric and RDMA](help:fabric-and-rdma) explains these terms.

**Always-on monitoring:**

| Scenario | What you see |
|---|---|
| **Agent offline** | The agent on `cell2` stops checking in, so 24/7 monitoring of that node lapses |
| **Agent settings out of date** | The agent on `cell3` missed the last settings push, so **24/7** turns amber |

## What it changes

Scenarios change only the simulator, so they need no unlock. Nothing touches your nodes. The **VPN** stays real, since it's a real part of this Mac.

In look-only mode this page is read-only. Its rows don't respond, though they look the same as usual, and **Back to everyday serving** is dimmed. To use this page, [turn look-only mode off](help:use-look-only-mode).

The command palette and **Cluster › Demo Scenarios** still play scenarios in look-only mode, because a scenario changes only the simulator. To end one there, press <kbd>⌘K</kbd>, type `back to everyday`, and press <kbd>Return</kbd>.

A scenario keeps playing until you choose another one or click **Back to everyday serving**. The menu bar's **Cluster › Demo Scenarios** does the same.

## Related How-Tos

- [Take a tour of the Demo](help:tour-the-demo)
- [Respond to an alert](help:respond-to-an-alert)
- [Switch between Demo and Live](help:switch-between-demo-and-live)
