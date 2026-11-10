---
id: status-words
title: What the status words mean
type: reference
area: reference
summary: Every status word and color in SparkBox, for nodes, models, switches, tasks and the VPN.
keywords: [online, offline, stale, ready, down, colors, icons, dots, healthy, connecting, error, firing, recovering, not set up, shapes]
verified: null
related: [top-bar-and-status-bar, read-the-dashboard, alerts-and-incidents, tasks-and-jobs, troubleshooting]
---

# What the status words mean

This page lists every status word SparkBox shows, what it means, and what to do about it. It's grouped by where you see the word, because a few words, such as **Stopped** and **Held**, mean something different in each place.

SparkBox never relies on color alone. Each state also has its own shape and a word, so you can read it in any light or with any color vision. Move the pointer over a dot or chip to see the details in a tooltip.

## How to read the colors

| Color | Means | Words |
|---|---|---|
| Amber | Look at this soon | **Warning**, **Stale**, **Connecting**, **Waking** |
| Red | Something is wrong now | **Critical**, **Offline**, **Error**, **Down**, **Failed** |
| Neutral gray | Known, planned or fine | Everything else |

Healthy things stay quiet on purpose, so anything colored stands out.

## Dots and shapes

| Shape | Word |
|---|---|
| Filled circle | **Online** |
| Pulsing ring | **Connecting** |
| Hollow ring | **Offline** |
| Filled circle with ✕ | **Error** |
| Half-filled amber circle | **Stale** |
| Dashed ring | **Not set up** |
| Triangle | **Warning** |
| Diamond | **Critical** |

The sidebar and the status bar use a second set of small marks for a node's own state.

| Mark | Word | Meaning |
|---|---|---|
| Small neutral dot | **Healthy** or **Online** | Nothing to report |
| Amber triangle | **Warning** | The node has a warning alert |
| Red diamond | **Critical** | The node has a critical alert |
| Red circle with a slash | **Offline**, **Error** or **Off** | SparkBox can't reach it. This is the loudest mark |
| Hollow ring | **Affected** | Fine in itself, but part of something that isn't, such as one node of a model that's down |
| Gray clock | **Recovering** | The problem has stopped, and the alert is waiting to clear |
| Gray wrench | **Planned**, **Restarting (SparkBox)** or **In maintenance** | Work you started |
| Gray bell with a slash | **Silenced** | You chose not to hear about this |
| Gray ⓘ | | Worth knowing, not a problem, and never counted. For example, a node that's on your LAN twice |

A node's color comes from the node and its own alerts. A model that's down doesn't turn its node red.

## Nodes

### Connection

| Word | Meaning | What to do |
|---|---|---|
| Online: the sidebar shows GPU use, such as **GPU 28%**, and the top bar counts it, as in **4/4 online** | SparkBox is connected and receiving readings. The tooltip reads **Healthy** when there are no alerts | Nothing |
| **GPU —** in the sidebar | Connected, but the node's GPU can't be read | Open the node's **GPU** tab to see why. To fix it, see [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering) |
| **Connecting** | SparkBox is trying to connect | Wait a moment |
| **Offline** | SparkBox can't reach the node. It keeps retrying | See [A node shows Offline](help:troubleshooting#a-node-shows-offline) |
| **Error** | The node answered and turned SparkBox away, usually over a login or host key, or it failed in a way SparkBox can't name | Read the tooltip, then see [Connection and login messages](help:errors-connection) |
| **Stale** with a time, such as **Stale 8 s** | Connected, but no new reading for over 3 seconds. The numbers on screen are the last ones received | Wait a few seconds. If it stays, see [The numbers stopped moving](help:troubleshooting#the-numbers-stopped-moving) |
| **Not updating** | Connected, but no new reading for over 3 seconds. This is the tooltip's word for **Stale** | Wait a few seconds. If it stays, see [The numbers stopped moving](help:troubleshooting#the-numbers-stopped-moving) |

While a node is healthy, its entry in the status bar shows a time, such as **22 ms**. That's the round trip of SparkBox's last command over SSH, the secure connection it uses to reach a node.

### Why it's offline

The tooltip or the node's box gives a reason.

| Reason | Meaning |
|---|---|
| **No SSH reply** | The node didn't answer in time |
| **SSH refused the connection** | The node is on, but its SSH service refused |
| **Unreachable on the LAN** | This Mac has no route to the node |
| **Hostname not found** | The node's name didn't turn into an address |
| **Host key not trusted yet** | First connection. See [Trust a node's host key](help:trust-a-host-key) |
| **Host key changed** | The node's identity key isn't the one SparkBox saved. See [Trust a node's host key](help:trust-a-host-key) |
| **Login failed** | The node refused the login |
| "not confirmed down" | This Mac's own path is the problem, so SparkBox can't say the node is down |

### Planned work

These never count as a fault, and they page nobody.

| Word | Meaning |
|---|---|
| **Restarting** | You restarted it from SparkBox. It's on its way back |
| **Shutting down** | You shut it down from SparkBox |
| **Off (planned)** | It's off because you shut it down. **Shut down from SparkBox** is the same state in a tooltip |
| **Waking** | SparkBox sent a Wake-on-LAN packet and is waiting for it |
| **In maintenance** | You put it in [maintenance mode](help:use-maintenance-mode). Its alerts page nobody |

## The top bar

| Text | Meaning |
|---|---|
| **4/4 online** | Nodes reachable, of all your nodes. It turns red when a node is missing with no planned reason |
| **2 models** and **4 GPUs serving** | Models answering now, and the GPUs they use |
| **no model serving** | No model has answered lately |
| **1 critical · 2 warnings** | Incidents now. Related alerts count once |
| **Demo** | You're looking at simulated data |

### Who watches while this Mac is away

A label in the top bar says whether your alerts still reach you while this Mac sleeps. See [Set up always-on monitoring](help:set-up-always-on-monitoring).

| Word | Meaning |
|---|---|
| **This Mac only** | No agent is installed. Alerts reach you only while SparkBox runs on this Mac |
| **Not paging** | Agents are watching, but no phone channel is set up, or every one sends from this Mac only |
| **24/7** | Agents are watching, and they can reach your phone |
| **24/7 on 3 of 4** | Only some agents answer. The rest are watched only while this Mac is awake |
| **Agents not answering** | No agent answers |
| **24/7 · settings not on cell2** | That node's agent has old alert settings. **Push again** fixes it |
| **24/7 · paging failed** | A message didn't get through. **Details…** says why |
| **24/7 · untested** | No test message has gone out since the settings changed. **Send a test page…** does that |

### The padlock

| Word | Meaning |
|---|---|
| **Read-only** | Admin mode is locked. You can look at everything, and a click on a change asks to unlock. |
| **Admin** | Admin mode is unlocked for changes |
| **Look only** | Look-only mode is on. SparkBox starts no change, and it asks for Touch ID or your Mac password only to turn the mode off |

A toolbar may show a small **Read-only** or **Look only** label of its own. **Needs Touch ID** there means Admin mode is on, but these tools change this Mac, so each one asks you to confirm it's you. See [Admin mode](help:admin-mode).

## The status bar

| Text | Meaning |
|---|---|
| **Live · 1 s** or **Demo · 1 s** | The data source, updating every second. The dot beats while new readings arrive |
| **Stale 8 s** | No new readings for 8 seconds. What's shown is the last thing received |
| **Waiting for data…** | Nothing has arrived yet |
| **Healthy** | No active alerts |
| A diamond or triangle with a number | That many critical or warning incidents. Click to open **Alerts** |
| **Keeping this Mac awake while Restart cell2 runs** | SparkBox is holding off sleep during long work. macOS still sleeps if you choose Sleep |
| **2 running** | That many tasks are running. For 10 seconds after a task ends, the status bar says how it ended |

## Switches and the gateway

| Word | Meaning |
|---|---|
| A time, such as **1.2 ms** | It answered in that time |
| **Online** | It's reachable |
| **Connecting** | SparkBox is trying to reach it |
| **Unreachable** | It didn't answer |
| **Not set up** | It has no address in [Settings › Switches & gateway](help:settings-switches) yet |

### Switch ports

| Word | Meaning |
|---|---|
| **Up** | The port has a link |
| **No link** | Nothing is connected, or the other end is off |
| **Disabled** | The port is turned off on the switch |
| **Errors rising** | The port is up, and its error counters are climbing. Suspect the cable or optic |

## Models and endpoints

An **endpoint** is a running model server you can talk to.

| Word | Meaning | What to do |
|---|---|---|
| **Ready** | It answers. For a load SparkBox started, it also gave a real reply to a test | Use it |
| **Loading** or **Loading 42%** | It's starting and reading its weights. The line under it says which stage | Wait. The first load of a model can take a long time |
| **Compiling: FlashInfer kernels**, or another name after the colon | A stage of **Loading**. The engine is preparing its code for the GB10 | Wait. A first start can spend a long time here, and **Keep running** leaves it alone |
| **2 of 4 nodes up** | A stage of **Loading**. That many nodes of a load are running so far | Wait. If the count stops, read the load's message |
| **Unknown**, with a reason such as **Pipeline stage 3 of 3 on cell3 is offline** | SparkBox cannot check a node the model needs. This does not prove the model failed | Reconnect and wait for fresh checks |
| **Down** with a reason, such as **Head exited (code 1)** | Current evidence shows it cannot serve | See [Model messages](help:errors-models) |
| **Down** with **Stalled: 3 requests running, no tokens for 60 s** | It answers `/health`, but it's stopped writing while requests run or wait | Read its log, then restart it. See [Tell whether a model is working](help:tell-if-a-model-is-working) |
| **Stopped** | It isn't running | Load it again if you want it |
| **Idle** | An Ollama server with nothing in memory. This is its resting state, not a fault | Nothing |
| **Paused** | It's on a node in planned work | Wait for the work to end |
| **Not found** | It has gone, and SparkBox has no evidence of why | Use **Forget** if you no longer want it listed |
| **Held · look-only** | Look-only mode is holding a **Keep running** restart of a several-node load. Nothing restarts while it's on | Leave it on to keep the model stopped. If you turn it off, SparkBox waits, then restarts the model by itself |
| **Held · locked** | You locked Admin mode, the screen locked or this Mac slept while a **Keep running** restart was waiting, so that restart was put off by one wait. SparkBox then tries again by itself, even with Admin mode locked | Nothing, if you want the model back. To stop the restarts, unload the model or turn on look-only mode |

### Loads

| Word | Meaning |
|---|---|
| **Running** | Every container is up |
| **Partly running** | Only some of its containers are up |
| **Stopped** | None of its containers are up, and that's expected |
| **Failed** | It stopped with an error. See [Fix a load that failed](help:fix-a-load-that-failed) |

### Models on disk

| Mark | Meaning |
|---|---|
| **✓ verified** | The copy was checked against Hugging Face's sizes and checksums |
| **✓ sizes match** | Copied over the fabric. Every file's size matches the source, and checksums weren't compared |
| **◐ incomplete** | A download or copy stopped partway. **Copy to nodes…** picks up where it stopped, and a new download should too |
| **◐ downloading**, **◐ copying** | Still running. Follow it in **Models › Jobs** |
| **● found** | The files are on disk with no record from SparkBox, so it hasn't checked them |

### Download and copy jobs

In **Models › Jobs**, a running job shows its current step, such as "checking the selected hf tool".

| Word | Meaning |
|---|---|
| **done** | It finished |
| **failed** | It stopped with an error. **Details** gives the reason |
| **canceled** | You stopped it |

## Alerts

| Word | Meaning |
|---|---|
| **Critical** | Needs attention now |
| **Warning** | Needs attention soon |
| **Info** | For your information. Never counted or sent |
| **Firing** | The problem is happening now. It counts in every badge, and it's sent as set in **Settings › Notifications** |
| **Confirming** | SparkBox has seen the condition and is checking that it lasts |
| **Recovering · clears in 1:34** | Nothing is wrong any more. The alert clears when the time runs out. Not counted |
| **Planned** | It comes from work you started. Not counted, and paged to nobody |
| **Acknowledged** | You've seen it. No repeats, and not counted, until it gets worse |
| **Silenced** | You chose not to hear about it for a while. Not counted |
| **Information** | Shown in **Alerts** only |
| **Flapping** | It keeps coming back, such as **Flapping · raised 3 times in 10 min** |

A count with nothing in it reads **No alerts**. **Silenced until Tue 10:42**, **Silenced until it clears** and **Silenced indefinitely** say how long a silence lasts. **Known issue** in a tooltip means an alert you silenced or acknowledged. [Alerts and incidents](help:alerts-and-incidents) explains the whole system.

### Deliveries

The **Deliveries** tab in **Alerts** says what happened to each message.

| Word | Meaning |
|---|---|
| **Sent** | It went out |
| **Grouped** | It was listed with its incident |
| **Duplicate** | It was the same as one already sent |
| **Held** | It waited, for example during quiet hours |
| **Silenced** | A silence stopped it |
| **Planned** | It came from planned work, so nobody was paged |
| **Eased** | It dropped to a warning, so nobody was paged |
| **Not paged** | Your settings didn't send this one |
| **Flapping** | It keeps coming back, so it wasn't sent again |
| **Muted** | It was muted for this run |
| **Failed** | Sending failed. See [Alert delivery and agent messages](help:errors-alerts-and-agent) |
| **Demo** | It came from simulated data, so it wasn't sent |

## Tasks

| Word | Meaning |
|---|---|
| **Running** | It's working now |
| **Done** | It finished |
| **Done with a warning** | It finished, and something needs a look |
| **Failed** | It stopped with an error |
| **Stopped** | You or SparkBox stopped it |

## VPN

| Word | Meaning |
|---|---|
| **Off** | No tunnel. In the status bar this is a hollow ring |
| **Direct** | No tunnel is needed, because the nodes answer directly |
| **Connecting** | The tunnel is starting |
| **Connected** | The tunnel is up |
| **Problem** | The tunnel has an error. The tooltip gives it. See [VPN, fabric and switch messages](help:errors-network) |

## Fleet Command badges

Beside the command you type, **Fleet Command** shows how risky SparkBox judges it to be. [Fleet Command](help:fleet-command#run) says what SparkBox asks for each one.

| Badge | Meaning |
|---|---|
| **read-only** | Every part of it only reads |
| **changes things** | It isn't on SparkBox's list of read-only commands, so it may change something |
| **runs as root** | It runs with root's power |
| **destructive** | It can destroy data or take nodes down |
| **takes nodes down** | It restarts or shuts down the nodes |
| **cuts access** | SparkBox may lose its connection to these nodes |

## GPU

A node's **GPU** tab says whether the GPU is being held back.

| Word | Meaning |
|---|---|
| **Not throttled** | It's running as it should |
| **Idle (P8) · clocks are low because nothing is running** | It's resting, and that's normal |
| **Throttled: SW power cap** | It's busy and held below its top speed. The words after the colon give the reason |
| **Not readable** | SparkBox couldn't read its state |

## Meters

A bar turns amber at its warning level and red at its critical level, judged on the value as printed. Levels come from the alert rules. For example, a disk warns at 85% and is critical at 95%. See [Alert rules reference](help:alert-rules-reference).

## Time words

Windows round times a little differently, so one moment can read differently in two windows.

| Text | Meaning |
|---|---|
| **just now** | Less than a minute ago. The exact limit depends on the window |
| **12 s ago**, **4 min ago**, **3 h ago** | About that long ago |
| **yesterday** | One to two days ago. Some windows write **1 d ago** or **1 day ago** instead |
| **2 d ago**, **5 days ago** | About that many days ago |
