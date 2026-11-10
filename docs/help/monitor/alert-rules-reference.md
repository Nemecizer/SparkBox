---
id: alert-rules-reference
title: Alert rules reference
type: reference
area: alerts
summary: Every alert SparkBox can raise, what it watches, its default lines, and what to do about it.
keywords: [alert list, defaults, rules, thresholds, what does this alert mean]
verified: null
related: [settings-alerts, change-alert-rules, alerts-and-incidents, respond-to-an-alert]
---

# Alert rules reference

This page lists the 48 rules SparkBox ships with, what each one watches, its default lines, and what to do when it fires. You can change most of these in [Settings: Alerts](help:settings-alerts). The groups match that screen, with one exception: **RoCE** has its own group here, and sits under **Fabric** in Settings.

## How to read this page

- **Lines** are the values that raise an alert. A **warning** needs attention soon. A **critical** alert means something is wrong now.
- **For** is how long the condition must hold before the alert fires. "At once" means no wait.
- **Clears** is what ends the alert. "Below 88%" means it stays raised until the reading falls that far, so a wobbling reading doesn't alert twice. A time after that is how long it must stay gone.
- **Paged** means the alert can go to your phone. "Not paged" means you see it in the **Alerts** window only. Each rule's area also has its own switch in [Settings: Notifications](help:settings-notifications).
- **This Mac only** means SparkBox judges the rule on this Mac. The always-on agents don't, so it needs SparkBox to be running.
- The **kernel** is the core of a node's operating system. Its log records faults that programs can't see.

## Nodes

Whether each node answers, and its memory, clock, kernel and agent. Turned on or off for notifications by the **Node status** switch.

### Node offline

- **What it watches:** The node doesn't answer SparkBox, or is in error.
- **Default lines:** Critical. For 30 s. Clears at once. A quick reconnect isn't an alert.
- **What to do:** Check the node's power and cable. See [Connection and login messages](help:errors-connection). If this Mac can't reach anything, it reads "Offline · not confirmed down" and isn't paged.

### Not back after a restart

- **What it watches:** A restart SparkBox started has run past its expected time.
- **Default lines:** Critical. For 2 min past the expected time. While a restart runs on time, its alerts are **Planned**.
- **What to do:** Open the node's window. If it stays away, it may need someone at the machine.

### Not updating

- **What it watches:** The node is online, but no fresh numbers are arriving.
- **Default lines:** Warning. For 5 s.
- **What to do:** Wait a little. If it lasts, see [Connection and login messages](help:errors-connection).

### This Mac can't reach the cluster

- **What it watches:** This Mac lost its network, its gateway or VPN has a problem, or no node answers over the LAN.
- **Default lines:** Warning. At once. Clears after 10 s. Not paged. This Mac only.
- **What to do:** Check this Mac's network. Nodes it can't reach aren't confirmed down, so you get this one warning, not one for each node.

### Node needs you

- **What it watches:** The node answers but SparkBox can't log in: its host key isn't trusted yet, or it refused the saved password.
- **Default lines:** Information. At once. Not paged. This Mac only.
- **What to do:** It isn't an outage. [Trust a node's host key](help:trust-a-host-key) or [save the cluster password](help:save-the-cluster-password).

### Unified memory

- **What it watches:** Memory in use outside a known GPU reservation, as a share of the node's total. SparkBox uses the larger of the measured GPU allocation and its running vLLM loads' recorded shares, without counting both. Older loads and other engines need a measured allocation.
- **Default lines:** Warning at 90% for 60 s. Critical at 95% for 2 s. Clears below 88%. If **Available memory** is off or its reading is missing, this checks total usage instead.
- **What to do:** Unload a model or stop a container. A Spark that runs out of unified memory can freeze, so act on this one quickly.

### Available memory

- **What it watches:** Memory Linux can give to programs. The engine's reserved pool is already in use; it is never added back to this number.
- **Default lines:** Warning below 4 GiB. Critical below 2 GiB. The condition must last 60 s before the first alert. An active warning escalates when it crosses the critical line. Clears above 5 GiB for 30 s.
- **What to do:** Unload a model or stop other work. A high percentage used by an engine can be expected, but low available memory still needs attention.

### Swap-in activity

- **What it watches:** Memory read back from swap, per second. Swap is space on a drive used to hold memory. A steady stream of reads can slow model requests. Already occupied swap alone does not raise this alert.
- **Default lines:** Warning above 1 MiB/s for 60 s. No critical line by default. Clears below 0.1 MiB/s for 30 s.
- **What to do:** Unload a model or stop other work if it keeps happening. The first reading, a reset counter or a gap longer than 15 s is unknown. Missing readings cannot confirm a pending alert or clear one that has fired.

### Memory pressure

- **What it watches:** How much of the last 10 seconds every running task spent stalled on memory. It rises before the system starts stopping programs.
- **Default lines:** Warning above 5%. Critical above 20%. For 30 s. Clears below 3%, then 30 s.
- **What to do:** Free memory, as for **Unified memory**.

Available memory, swap-in activity and memory pressure need the updated always-on agent, version 1.3.0. This Mac keeps their notifications while an older agent is installed; pushing settings alone does not upgrade it. See [Settings: Notifications](help:settings-notifications) to reinstall the agent when you are ready.

### Clock offset

- **What it watches:** How far the node's clock is from network time.
- **Default lines:** Warning beyond ±100 ms. Critical beyond ±1000 ms. For 60 s. Clears after 60 s.
- **What to do:** Check the node's time service. Several-node models and log times go wrong when clocks drift apart.

### Clock not synchronized

- **What it watches:** The node says its clock isn't in step with network time.
- **Default lines:** Warning. For 10 min. Clears after 60 s.
- **What to do:** Check the node's network and time service in [Services](open:services).

### Out-of-memory kill

- **What it watches:** The kernel stopped a program to free memory.
- **Default lines:** Warning, or critical when it stopped a program that serves a model (vLLM, Ray, Ollama). At once. Lasts 30 min after the latest one.
- **What to do:** See which program it was in **Alerts › Events**. Load less, or use a smaller model.

### Hung task

- **What it watches:** A program stayed stuck for more than 2 minutes.
- **Default lines:** Warning. At once. Lasts 30 min after the latest one.
- **What to do:** Look at **Alerts › Events**. Often it's a slow or failing disk.

### Agent stopped reporting

- **What it watches:** The node answers SparkBox, but its always-on agent's heartbeat is over a minute old.
- **Default lines:** Warning. For 60 s. Clears after 30 s.
- **What to do:** SparkBox still watches the node while it's open, but nothing pages you while it's closed. Reinstall the agent from [Settings: Notifications](help:settings-notifications).

### Agent settings out of date

- **What it watches:** The agent runs other settings than this Mac sent it.
- **Default lines:** Warning. For 60 s.
- **What to do:** Click **Push again** in [Settings: Notifications](help:settings-notifications).

### Restarted on its own

- **What it watches:** The node's boot changed, and SparkBox hadn't restarted it.
- **Default lines:** Warning for an hour. Critical at a second restart within an hour, or two nodes within 5 minutes. This Mac only.
- **What to do:** [Find out why a node restarted](help:find-out-why-a-node-restarted).

### Security updates pending

- **What it watches:** Security updates are waiting on the node.
- **Default lines:** Information at first, not paged. A warning once they've waited 7 days. This Mac only.
- **What to do:** [Install updates on a node](help:install-updates).

### Restart pending

- **What it watches:** The node has needed a restart to finish installing updates for 3 days.
- **Default lines:** Warning. This Mac only.
- **What to do:** [Restart a node](help:restart-a-node) when it suits you.

### Package lists stale

- **What it watches:** The node's list of available updates is over 48 h old, so what SparkBox shows may be out of date.
- **Default lines:** Warning. This Mac only.
- **What to do:** [Check for updates now](help:check-for-updates). If it comes back, the node's daily update timer may not be running.

## Disk

The node's root disk, which is its main disk. Turned on or off for notifications by the **Disk** switch.

### Root disk full

- **What it watches:** How full the root disk is, judged on the whole percent SparkBox shows.
- **Default lines:** Warning at 85%. Critical at 95%. At once. Clears below 83%.
- **What to do:** [Check disk space on a node](help:check-disk-space), then [delete a model](help:delete-a-model) or [remove Docker images](help:check-docker-images).

### Root disk filling

- **What it watches:** How soon a steady fill would fill the disk. It needs a rate the disk held for most of a stretch at least a sixth as long as the time it predicts, and the disk must still be filling. So 10 minutes can warn about the next hour, but a 24-hour estimate takes 4 hours to appear. A stretch of an hour or less must also have used a quarter of the free space.
- **What doesn't count:** A download or copy that's over within about an hour, on a disk with room to spare. One that keeps going for hours does count, because the disk can't tell it from any other fill.
- **Writers that pause:** An hourly writer can raise and clear this warning again and again while the disk is still filling. A clear doesn't always mean the fill has stopped, so check the disk before you rely on it. Writers that pause for 40 minutes or more do this.
- **Default lines:** Warning under 24 h. At once. Clears above 30 h. It also clears after 5 minutes without a steady fill. That usually takes about 35 minutes after writing stops, sooner for a fast fill, and longer after a last burst of writing (44 minutes seen).
- **After a restart or sleep:** SparkBox keeps an open warning while the disk is still gaining, because it has lost the history it needs to measure the fill. If the disk has stopped gaining, the warning clears.
- **What to do:** See what's writing to the disk. A long download can be the cause. Check [Tasks](open:tasks) and **Models › Jobs**.

## GPU

The node's graphics processor. Turned on or off for notifications by the **GPU** switch.

### GPU temperature

- **What it watches:** How hot the GPU is.
- **Default lines:** Warning at 85 °C. For 2 min. Clears below 82 °C, then 30 s. No critical line by default, although the charts draw one at 90 °C.
- **What to do:** Check the node's airflow and what's around it.

### GPU throttling

- **What it watches:** The GPU is slowing itself down. Hardware reasons always count. Software limits count only while the GPU is busy: 50% or more and below 90% of its top clock, or at 80% of a reported power limit. Idle and application clocks never count.
- **Default lines:** Warning. For 60 s. Clears after 2 min.
- **What to do:** Look at the node's **GPU** tab for the reason. Heat and power are the usual causes.

### GPU Xid error

- **What it watches:** The GPU reported a numbered fault in the kernel log.
- **Default lines:** Severity depends on the code. At once. Fatal codes (48, 79, 94, 95, 119, 120, 154) stay until the node restarts. Others clear an hour after the last one.
- **What to do:** Select it in **Alerts › Events** to read what it means. A fatal one needs a [restart](help:restart-a-node).

### GPU not answering

- **What it watches:** NVIDIA's tools stopped answering.
- **Default lines:** Critical. For 30 s.
- **What to do:** The node's models are **Down** until the GPU is back. It usually needs a [restart](help:restart-a-node).

## Fabric

The fast 200G network, its switch and its cables. Turned on or off for notifications by the **Fabric & switches** switch.

### Fabric link down

- **What it watches:** A fabric interface with an address has lost its link. An uncabled half stays quiet.
- **Default lines:** Critical. For 3 s. Clears after 30 s.
- **What to do:** Check the cable and optic. See [Check the fabric](help:check-the-fabric).

### Link flapping

- **What it watches:** A link went down and came back.
- **Default lines:** Warning at any flap in 5 min. At once.
- **What to do:** Reseat the cable. If it continues, [bounce the port](help:manage-a-switch-port).

### Interface errors

- **What it watches:** Error counters are rising on a network connection.
- **Default lines:** Warning at any rise in 2 min. At once.
- **What to do:** Check the cable and optic.

### Fabric MTU

- **What it watches:** A fabric port's largest packet size isn't the expected one, or differs from its peers.
- **Default lines:** Warning. Expects 9000. For 30 s. Clears after 30 s.
- **What to do:** Set the node's MTU to 9000. Check the result with [Test the fabric](help:test-the-fabric).

### ConnectX-7 driver error

- **What it watches:** The fast network card's driver reported a fault: a module unplugged, a command timeout, a health problem or a link down.
- **Default lines:** Warning. At once. Lasts 30 min after the latest one.
- **What to do:** Select it in **Alerts › Events** to read what it means.

### Switch unreachable

- **What it watches:** A switch or the gateway doesn't answer.
- **Default lines:** Critical. For 10 s. Clears after 30 s. If this Mac can't reach anything, it reads "Unreachable · not confirmed down" and isn't paged.
- **What to do:** Check the device's power and network cable.

### Switch power, fans and temperature

- **What it watches:** The CRS804's own health sensors.
- **Default lines:** A failed power supply is critical. A fan under 500 RPM or a sensor over 85 °C warns. For 30 s. Clears after 60 s.
- **What to do:** Open the [switch's window](open:switch?id=crs804) on its **Overview** tab. A failed supply means the switch runs on its other one.

### Switch port down

- **What it watches:** A port cabled to a node has lost its link.
- **Default lines:** Critical. For 3 s. Clears after 30 s.
- **What to do:** Check the cable. This joins the node's incident if the node has gone too.

### Switch port errors

- **What it watches:** Receive, transmit and frame-check errors together are rising on a port.
- **Default lines:** Warning at any rise in 2 min. At once.
- **What to do:** Clean or reseat the optic, or change the cable.

### Optic receive power

- **What it watches:** How much light reaches the weakest lane of an optic.
- **Default lines:** Warning below −7 dBm for 5 s. Critical below −10 dBm. Clears above −6.5 dBm, then 15 s.
- **What to do:** Clean or reseat the fiber. See the **Optics** tab in the [switch's window](help:switch-window#optics).

### NCCL on TCP sockets

- **What it watches:** A several-node model moves under half of its fabric bytes over RDMA while generating, or its NCCL settings turn RDMA off or point it the wrong way.
- **Default lines:** Warning. For 60 s. Clears after 2 min.
- **What to do:** See the details in [Fabric](help:fabric#nccl-transport). NCCL is the library a model uses to share work between GPUs.

### Fabric cabling changed

- **What it watches:** A node is on a different switch port than SparkBox learned. A swap still links, but breaks port comments and per-port history.
- **Default lines:** Warning. For 30 s. Clears after 2 min.
- **What to do:** Move the cables back, or [accept the current cabling](help:check-the-fabric) if you meant it.

## RoCE

RoCE is how RDMA, the fast direct path, runs over the fabric. This rule watches each fabric interface, and raises one alert for each one, however many counters move. Turned on or off for notifications by the **Fabric & switches** switch.

### RoCE on a fabric interface

- **What it watches:** The interface's RoCE and physical-layer counters.
- **Default lines:** The lines are per counter, below. Retransmits and drops must average past their line for a minute. Any CRC or symbol error counts at once. Clears once nothing has grown for 30 s.
- **What to do:** Read the alert's words. "Check the optic or cable" comes first when CRC or symbol errors are rising. See [Check the fabric](help:check-the-fabric).

In Settings this rule has a **13 counters** button. Each line is a rate per second, and each has its own switch.

| Counter | Warning above | Critical above | What it means when it rises |
|---|---|---|---|
| **Out of sequence** | 1/s | 100/s | Packets arrived out of order because earlier ones were lost |
| **Sequence errors** | 1/s | 100/s | The receiver saw an unexpected packet, and the sender went back |
| **ACK timeouts** | 1/s | 20/s | This side waited for a reply that never came, then sent again |
| **Implied NAKs** | 1/s | 100/s | A reply skipped ahead, which implies a lost packet |
| **CRC errors** | 0/s | 10/s | Frames arrived corrupted. Almost always a dirty or failing optic, or a bad cable |
| **Symbol errors (PHY)** | 0/s | 10/s | The hardware couldn't decode the signal |
| **Symbol errors (port)** | 0/s | 10/s | The same trouble, as the port counts it |
| **Receive errors** | 0/s | 10/s | The port received packets with errors and dropped them |
| **Discards (PHY)** | 1/s | None | The network card dropped packets because its buffer was full |
| **Out of buffer** | 1/s | None | Packets dropped because a program hadn't made room in time |
| **Transmit discards** | 1/s | None | Packets the port couldn't send |
| **Pause sent** | 10,000/s | None | This node asked the switch to stop for a moment. Bursts are normal |
| **Pause received** | 10,000/s | None | The switch asked this node to stop for a moment |

## Containers & services

Docker containers and the node's background services. Turned on or off for notifications by the **Containers, services & models** switch.

### Crash loop

- **What it watches:** Docker keeps restarting a container.
- **Default lines:** Warning at one restart in 10 min. Critical at two. It stays critical through the seconds it runs between restarts, and eases only after 5 minutes under the line. It ends once the container has run a minute without restarting, or its model answers again, then clears after 5 min.
- **What to do:** Read its log in [Containers](open:containers).

### Container exited with an error

- **What it watches:** A container stopped with an error code. Exits older than 24 h are history.
- **Default lines:** Warning, or critical when the system stopped it for memory. At once.
- **What to do:** See [Manage containers](help:manage-containers).

### systemd unit failed

- **What it watches:** A systemd unit on the node failed. It's usually a service, but it can be another kind, such as a `.mount`.
- **Default lines:** Warning. At once.
- **What to do:** See [Restart, disable or enable a service](help:restart-a-service). A unit that isn't a service isn't in the **Services** window: see [Failed units that aren't services](help:services#failed-units-that-arent-services).

## Serving

Models that are loaded. Turned on or off for notifications by the **Containers, services & models** switch.

### Model down

- **What it watches:** A model should be serving, but a node, a container or its answer to a check is missing.
- **Default lines:** Critical. For 30 s. Clears the moment it answers again.
- **What to do:** See [Fix a load that failed](help:fix-a-load-that-failed).

### Time to first token

- **What it watches:** How long requests wait for their first word: the 95th percentile, so 95% are faster.
- **Default lines:** Warning above 2000 ms. For 5 min. Clears after 60 s.
- **What to do:** The model is busy. Reduce the load on it.

### Requests waiting

- **What it watches:** Requests queued because the model is full.
- **Default lines:** Warning above 0. For 60 s. Clears after 30 s.
- **What to do:** Fewer requests at once, or a higher **Requests at once** setting. See [Choose load settings](help:choose-load-settings).

### KV cache

- **What it watches:** How full the memory is that the model uses to keep track of the text so far.
- **Default lines:** Warning above 90%. For 2 min. Clears below 85%, then 60 s.
- **What to do:** Allow more room, or fewer long requests at once. See [Choose load settings](help:choose-load-settings).

### Preemptions

- **What it watches:** The model paused running requests to make room.
- **Default lines:** Warning at any in 5 min. At once.
- **What to do:** The KV cache is too small for the work. See [Choose load settings](help:choose-load-settings).

### Corrupted requests

- **What it watches:** Requests that came back with broken numbers, from the engine's own counter. vLLM keeps this counter only when it runs with `VLLM_COMPUTE_NANS_IN_LOGITS` set, so a load without that setting never raises this alert.
- **Default lines:** Warning at any in 5 min. At once.
- **What to do:** The model is writing nonsense. Unload it and try the fallback image. See [The model answered 17+25 wrongly](help:fix-a-load-that-failed#the-model-answered-1725-wrongly).

### Loaded but idle

- **What it watches:** A model has held memory for 24 h with no requests.
- **Default lines:** Information. Not paged.
- **What to do:** [Unload it](help:unload-a-model) if you don't need it.
