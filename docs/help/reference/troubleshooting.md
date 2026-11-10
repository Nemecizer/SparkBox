---
id: troubleshooting
title: Troubleshooting
type: troubleshooting
area: troubleshooting
summary: Start here when something isn't working. Find your symptom and go to the fix.
keywords: [problem, broken, not working, fix, help, error, stuck, dimmed, can't, won't, greyed out, slow, loading forever, disk full, blank, blank window, empty window, white screen, nothing works, something is wrong]
verified: null
related: [errors-connection, errors-models, collect-diagnostics, errors-admin-and-safety, status-words]
---

# Troubleshooting

Start here when something isn't working. Find what you see, then follow the fix. If a message has a **What does this mean?** button, click it, and Help searches for that message. A **Details** button shows the node's own words. For any other message, search Help for a few of its words. Most messages are listed, and similar ones share an entry.

## Connection problems

### A node shows Offline

**What it means.** SparkBox can't reach the node. It keeps retrying by itself, so a short drop clears without you. A red **Offline** is different from the gray **Off (planned)** and **Restarting**, which you caused yourself.

**What to do.**

1. Move the pointer over the node in the status bar. The tooltip gives the reason, such as **No SSH reply**.
2. Look at the node. Is it on, and is its network cable in?
3. If you're away from home, open [VPN](open:vpn) and check that it's connected.
4. If only this node is affected and it doesn't answer, it may have frozen. Follow [Recover a frozen node](help:recover-a-frozen-node).
5. If every node is affected, see [Nothing connects](#nothing-connects).

[Connection and login messages](help:errors-connection) lists each reason in full.

### A node shows Error

**What it means.** The node answered and turned SparkBox away. The first time, this usually means its host key isn't trusted yet. SparkBox asks you to trust each node's key once, so this is normal while you connect a node for the first time.

**What to do.** Move the pointer over the node in the status bar, then match the reason in the tooltip.

| Tooltip | Fix |
|---|---|
| **Host key not trusted yet** | [Trust a node's host key](help:trust-a-host-key) |
| **Host key changed** | Don't trust the new key until you know why it changed. See [If a node's key changed](help:trust-a-host-key#if-a-nodes-key-changed) |
| **Login failed** | [Save the cluster password](help:save-the-cluster-password) |
| Anything else | Look up its words in [Connection and login messages](help:errors-connection) |

### Nothing connects

**What it means.** Every node shows **Offline** or **Connecting**. Your nodes rarely fail together, so look first at what they share: this Mac's connection, your switches and gateway, and your home's power.

**What to do.**

1. Look at the status bar. If it says **Demo · 1 s**, you're looking at simulated data. See [Switch between Demo and Live](help:switch-between-demo-and-live).
2. Check that your switches and gateway have power. After a power cut, every node reads **Offline** until the power and the network are back. See [After a power cut](help:turn-the-whole-cluster-off-and-on#after-a-power-cut).
3. Check this Mac's Wi-Fi or cable.
4. If you're away from home, [connect the VPN](help:connect-the-vpn).
5. Open [Alerts](open:alerts). A line such as "This Mac can't reach any node; the LAN between them?" names the likely place.
6. Check that the cluster password is saved. See [Save the cluster password](help:save-the-cluster-password).
7. Go through [This Mac can't reach the cluster](help:errors-network#this-mac-cant-reach-the-cluster).

### The numbers stopped moving

**What it means.** A node, or the whole status bar, reads **Stale** with a time, such as **Stale 8 s**. What's on screen is the last reading SparkBox received, over 3 seconds ago.

**What to do.** Wait a few seconds. One stale node usually means a busy node, or a short network drop. If it stays stale, see [A node shows Offline](#a-node-shows-offline). If **Waiting for data…** stays on, see [A banner says "Can't reach the SparkBox core"](#a-banner-says-cant-reach-the-sparkbox-core).

## Buttons, locks and Touch ID

### A button is dimmed

**What it means.** The action can't run right now. Move the pointer over the button, and the tooltip says why.

**What to do.** Match the tooltip.

| Tooltip | Why | Fix |
|---|---|---|
| "Look-only · turn off in Settings › Security & privacy" | Look-only mode is on | [Use look-only mode](help:use-look-only-mode) |
| "Available with Live data" | You're using Demo data | [Switch between Demo and Live](help:switch-between-demo-and-live) |
| None | Nothing is selected, or the action doesn't apply | Select a row, or check that the node is online |

A button that needs Admin mode isn't dimmed. It shows a small lock, and a click asks you to unlock. The one exception is the priority controls in [Workloads](help:workloads#priority), which stay dimmed until you unlock. Menu items dim when no window is open that they can act on.

### SparkBox asks for Touch ID

**What it means.** Unlocking [Admin mode](help:admin-mode) asks for Touch ID or your Mac password. So does any change on this Mac itself: a setting, a saved password, a file, even with Demo data and with Admin mode on. This protects you if someone else sits down at your Mac. Changes on this Mac stay allowed for 5 minutes after you confirm, except for saved passwords and keys, which ask each time.

**What to do.** If you started the action, confirm it. Use your Mac password, not the cluster password. If you didn't start anything, click **Cancel**, then read the [Activity log](help:activity-log).

macOS may also ask once to let SparkBox use the Keychain. That's normal. Allow it.

### Admin mode keeps locking

**What it means.** Admin mode locks itself after your **Lock after** time with no changes, and when the screen locks, the Mac sleeps, or you close the window. This is on purpose. Running work, such as a download or an upgrade, keeps going.

**What to do.** To change the time, see [Settings: Security & privacy](help:settings-security). You can pick 5 or 15 minutes, or 1, 2, 4, 8 or 24 hours.

### A change was refused

**What it means.** SparkBox stopped a change before it reached the node. The [Activity log](help:activity-log) shows it as **Refused**, with the reason.

**What to do.** Read the reason, then see [Admin, look-only and safety messages](help:errors-admin-and-safety). The usual reasons are look-only mode, a locked Admin mode, and another copy of SparkBox.

### The padlock says Look only

**What it means.** Look-only mode is on, so SparkBox starts no change. Work already running by itself on a node keeps going. The mode stays on across launches.

**What to do.** Turn it off in [Security settings](open:settings?section=security), or see [Use look-only mode](help:use-look-only-mode).

## Models

### A model won't load

**What it means.** The Load page won't start, or a load started and failed.

**What to do.**

1. If **Load on 1 node** is off, read the words beside it. They name the first problem, such as "The memory plan is red".
2. If the load started and failed, open **Models › Loaded**. Read the load's note under the table. If it has a **Failed** row, click **Explain…**.
3. Follow [Fix a load that failed](help:fix-a-load-that-failed).
4. Look up the exact words in [Model messages](help:errors-models).

Several-node loads have run only on test systems. A one-node load is the safer first try. See [What's proven on your cluster](help:whats-proven).

### A model has been loading for a long time, or reads Down

**What it means.** The first time a model starts, the engine prepares its code for the GB10 chip, and that can take a long time with little to show. **Loading** with "no new progress for 25 min (limit 90 min)" doesn't mean it failed. It means nothing has changed for a long time. The load keeps going, and SparkBox waits up to the 90-minute limit.

**What to do.**

1. In the sidebar, under **Workloads**, click **Models**, then click the **Loaded** tab. [Open Loaded](open:models?tab=loaded)
2. Read **vLLM log** under the table. If new lines are appearing, leave it alone.
3. If the log hasn't moved for a long time, follow [Slow is not failed](help:fix-a-load-that-failed#slow-is-not-failed).

**Unknown**, with a reason such as **Pipeline stage 3 of 3 on cell3 is offline**, means SparkBox cannot check the model. Restore the connection and wait for fresh checks. **Down** means current evidence shows a failure. See [Tell whether a model is working](help:tell-if-a-model-is-working).

### A node's GPU doesn't answer

**What it means.** The sidebar shows **GPU —** for the node, or the **GPU driver** check reads "nvidia-smi didn't answer". SparkBox can't read the GPU, so the node can't run a model.

**What to do.** Don't load a model on this node, and don't install or remove NVIDIA packages by hand, because that can leave it unable to start. Follow [Get a node's GPU driver answering again](help:get-a-gpu-driver-answering).

### A model writes nonsense

**What it means.** The model loaded and replied with gibberish. On a GB10, some engine images do this. SparkBox tests for it with the [17+25 check](help:glossary#1725-check), and fails the load.

**What to do.** SparkBox has already removed its containers. Pick another image, and load it again. See ["It answers, but wrongly"](help:errors-models#is-it-answering).

### A model is slow

**What it means.** Some of this is normal. A large model on one Spark may write only a few words a second. Speed is measured in tokens per second, shown as **tok/s**. A token is about three quarters of a word, so 8 tok/s is about 6 words a second.

The fit table's **Decode t/s** column shows the most one reply can reach, and real speed is lower. If yours is close to that number, this model can't go much faster with this setup.

**What to do.** Try these in order.

1. If a reply is slow to start, the conversation may be long. A model reads the whole conversation before each reply, so click **New chat** in [Chat](open:chat).
2. Open the model's card in [Endpoints](open:endpoints), and look at **Running · waiting** and **KV cache**. If requests are waiting and **KV cache** is near 100%, it's short of memory for its conversations. Send fewer requests at once.
3. If it's still too slow for you, pick a smaller model. See [Find a model that fits](help:find-a-model-that-fits).
4. Use a tensor parallel split, shown as **TP** in the fit table's **Split** column. It raises the ceiling, while pipeline parallel doesn't. Several-node loads have run only on test systems, so try the other steps first. See [Running one model on several nodes](help:splitting-a-model).

To measure real speed, see [Benchmark a model](help:benchmark-a-model).

### A download or copy stopped

**What it means.** Jobs run on the node. If a job failed, it says why in **Models › Jobs**, under **Details**.

**What to do.** Read **Details**, then see [Downloading](help:errors-models#downloading) or [Copying](help:errors-models#copying). "The job stopped unexpectedly (node restarted?)." means it needs starting again.

### Chat doesn't list my model

**What it means.** Chat lists models that are **Ready** and that this Mac can reach. A model that listens only on its node, or only on the fabric, can't be reached from here.

**What to do.** Check its card in [Endpoints](open:endpoints). If it says "Chat and the API server on this Mac can't use it", load it again with **Listen on** set to **LAN**. See [Chat, Providers and Server messages](help:errors-ai).

### My app can't reach a model

**What it means.** Your app may be using the wrong address, a wrong key, or a model that isn't **Ready**.

**What to do.** Check [Endpoints, ports and API keys](help:endpoints-and-apis) and [Connect an app to SparkBox](help:connect-an-app). Run the test in [Server](open:server).

## Disks

### A disk is nearly full

**What it means.** A node's root disk, its main disk, is filling up. SparkBox warns at 85% and calls it critical at 95%. A full root disk stops a node from working normally. Models take the most room, and one can be more than 100 GiB.

**What to do.**

1. Follow [Check disk space on a node](help:check-disk-space) to see what's using the space.
2. Follow [Delete a model from a node](help:delete-a-model). It frees the most space.
3. Follow [Check Docker images](help:check-docker-images) to find images you don't use.

Don't start a download until there's room for it.

## Updates and power

### An update was refused

**What it means.** SparkBox checks each upgrade before it runs. A refusal means nothing changed on the node. The usual reason is the GPU stack, which it won't move on one node.

**What to do.** Read the reason. See [Update and restart messages](help:errors-updates-and-power).

### SparkBox won't restart a node

**What it means.** A check before the restart failed, and the message ends "Not restarted." SparkBox is protecting you from a node that wouldn't come back.

**What to do.** Don't get around it. See [The checks before a restart](help:errors-updates-and-power#the-checks-before-a-restart).

### A node didn't come back after a restart

**What it means.** SparkBox waited 5 minutes. The node may be slow, stuck in firmware, or waiting at a console.

**What to do.** Give it a few more minutes. If it still doesn't answer, follow [Recover a frozen node](help:recover-a-frozen-node). See also ["cell2 hasn't come back after 5 minutes"](help:errors-updates-and-power#restarting-and-shutting-down).

### Wake doesn't work

**What it means.** The wake packet didn't start the node, or SparkBox couldn't send it. Wake-on-LAN has run only on test systems.

**What to do.** Press the node's power button. See [Waking a node](help:errors-updates-and-power#waking-a-node).

## Alerts

### An alert keeps coming back

**What it means.** The state pill reads **Flapping**. The problem clears and returns.

**What to do.** Fix the cause. While you do, you can [silence it](help:respond-to-an-alert). If the rule is too sensitive for you, see [Change alert rules](help:change-alert-rules).

### My phone didn't get an alert

**What it means.** The alert may have been held, or sending failed.

**What to do.** Open **Alerts › Deliveries**, and find the row. **Held** with quiet hours means it waited. **Failed** gives a reason. See [What the status words mean](help:status-words#deliveries) and [Alert delivery and agent messages](help:errors-alerts-and-agent).

## The app itself

### A window shows "This window hit an error"

**What it means.** One window crashed. The rest of SparkBox is fine, and nothing on your nodes changed.

**What to do.** Click **Reload window**. If it happens again, click **Copy details** and keep the text. SparkBox also writes it to `~/Library/Logs/SparkBox/sparkbox.log`.

### A window is blank

**What it means.** The window hasn't drawn its contents. Nothing on your nodes changed.

**What to do.** Close the window and open it again. If it's still blank, quit SparkBox and open it again. Work running on your nodes isn't affected.

### A banner says "Can't reach the SparkBox core"

**What it means.** The part of SparkBox that does the work isn't answering the window. The banner adds a reason, such as "no data has arrived for 10 seconds".

**What to do.** Click **Retry**. If it comes back, quit SparkBox and open it again. Work running on your nodes isn't affected.

### A banner says "Couldn't read inventory.json"

**What it means.** A settings file was damaged. SparkBox started with defaults, and kept the old file. It may name another file.

**What to do.** Click **Show in Finder**. See [Where SparkBox keeps things](help:files-and-folders#settings). If your nodes are missing, [connect your cluster](help:connect-your-cluster) again, or restore the file from a backup.

### A banner says deployment history could not be saved or read

**What it means.** The deployment list is unreadable, changed outside this session, or could not be saved. SparkBox keeps the existing files instead of replacing them with an empty list. New loads and Forget require a successful save. An unload can still finish cleanup and show a warning about saving its report.

**What to do.** Keep the visible report and click **Show in Finder**. Preserve `deployments.json` and the `deployment-history` folder for review. Use one SparkBox app at a time. Do not restore an old list over the current file: its saved running states and ownership may be stale. Missing records from before recovery copies existed cannot be reconstructed automatically. See [Where SparkBox keeps things](help:files-and-folders#history-and-records).

### The SparkBox window has gone

**What it means.** With **Keep running in the menu bar** on, closing the window leaves SparkBox running. With it off, closing the window quits SparkBox.

**What to do.** Click SparkBox's icon in the menu bar, then **Open SparkBox**. If there's no icon, open SparkBox from the Applications folder.

### A window on the desk has gone

**What it means.** It's minimized, closed, or hidden behind another window.

**What to do.** Look in the **Window** menu. Press <kbd>F3</kbd> to see every window. Press <kbd>⇧⌘T</kbd> to reopen the last one you closed.

### I can't find a feature

**What it means.** It may be in another window, or not built yet.

**What to do.** Press <kbd>⌘K</kbd> and type what you want. Check [What's coming](help:whats-coming).

## Messages by area

| Where the message came from | Page |
|---|---|
| A node, SSH, a password, sudo, a host key | [Connection and login messages](help:errors-connection) |
| Admin mode, look-only mode, the other SparkBox, the Fence | [Admin, look-only and safety messages](help:errors-admin-and-safety) |
| Hugging Face, downloads, copies, loads, unloads | [Model messages](help:errors-models) |
| Updates, restarts, waking, Services, Containers, Fleet | [Update and restart messages](help:errors-updates-and-power) |
| Files, the Editor, Terminal | [Files and Terminal messages](help:errors-files-and-terminal) |
| Chat, Providers, the Server | [Chat, Providers and Server messages](help:errors-ai) |
| The VPN, the fabric, the switches | [VPN, fabric and switch messages](help:errors-network) |
| Notifications, the always-on agent | [Alert delivery and agent messages](help:errors-alerts-and-agent) |

## Find out more about a problem

- **Activity log.** Every command SparkBox ran, and how it ended. See [Find out what SparkBox did](help:find-what-sparkbox-did).
- **Tasks.** The steps of anything SparkBox did, and where it stopped. See [Tasks](help:tasks).
- **Evidence.** A saved record of every node's state, to compare before and after. See [Capture evidence](help:capture-evidence).
- **Doctor report.** A full text report for you to keep or share. See [Collect diagnostics](help:collect-diagnostics).
