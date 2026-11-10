---
id: fix-a-load-that-failed
title: "Fix a load that failed"
type: howto
area: models
summary: "Find out why a load failed, read the logs SparkBox kept, and what to try next for each common cause."
keywords: ["failed", "error", "crash", "explain", "out of memory", "exit 137", "exit 78", "rollback", "failed-loads", "api key file missing", "garbage output", "didnt start", "didn't start", "wont start", "won't start", "model wont start", "the model didnt start", "model didnt start", "too large", "too big", "my model is too large", "garbled", "gibberish"]
verified: false
related: [errors-models, models-loaded, memory-on-a-spark, clear-what-an-earlier-load-left-behind]
---

# Fix a load that failed

To fix a load that failed, read why on the **Loaded** tab, change what caused it, and load again.

When a load you started fails while it's starting, SparkBox cleans up. It saves each node's log to this Mac, removes the containers from every node, and keeps the cause. It never tries a first start again by itself. See [What SparkBox does when a start fails](#what-sparkbox-does-when-a-start-fails).

## Before you start

- Reading why a load failed needs no Admin mode, and it also works in look-only mode. SparkBox only reads the containers' state and logs.
- To change a setting and load again, use **Live** data with look-only mode off, and be ready to unlock Admin mode.
- SparkBox shows a failure like this for 7 days. It finds failed runs whoever started them, including a run from a script on the nodes.
- **Check first that it really failed.** A card that reads **Loading** with "no new progress for 25 min (limit 90 min)" may still be working. See [Slow is not failed](#slow-is-not-failed).

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Loaded](open:models?tab=loaded) The **Models** window opens.
2. Click the **Loaded** tab. A load you started has a red note under the table, with its cause. A run that still has its containers has a row with a red **Failed** label and a line such as "Exited 1 · 2 d ago". That's a run SparkBox didn't start, or a load that failed after it read **Ready**.


3. For a load you started, read its note. It gives the cause, and the **vLLM log** under the table shows each node's last lines.
4. For a **Failed** row, click **Explain…**. A dialog, **Why … failed**, opens. It's read-only. Read it from the top: how it exited, the **Likely cause**, a few lines from the log with the decisive one marked, and a suggested fix. Then click **Done**.
5. To read more, open the log SparkBox saved. See [Where the logs are](#where-the-logs-are).
6. Find the likely cause in the table below, and make the change it suggests.
7. Remove the failed load. On its note on the **Loaded** tab, click **Forget**. If its containers are still on a node, SparkBox unloads them first. A run SparkBox didn't start has no note: its containers stay until you [remove them](help:manage-containers).
8. Load the model again. See [Load a model on one node](help:load-a-model-on-one-node).

If the load has a card, you can click **Edit & reload…** instead of steps 7 and 8.

### What SparkBox does when a start fails

If a container won't start, a container stops while the load is starting, the 90-minute limit passes, or the model answers wrongly, SparkBox does this for a vLLM load, in order:

1. It reads each node's state and the last 600 lines of its log, removes any secrets from them, and saves them to this Mac.
2. It unloads the model from every node, head node first, and checks `/dev/shm`.
3. It marks the load **Failed**, with the cause and each node's last 40 lines.

On a load across nodes, a worker that fails to start means the head node never starts. A first start that stops isn't tried again. A load that has been **Ready** and then crashes isn't a failed start: it keeps its containers, and it goes to **Keep running** if that's on. An SGLang load keeps its containers too: [unload it](help:unload-a-model).

> [!NOTE]
> **Sometimes SparkBox can't clean up.** If look-only mode is on, or a node is out of reach, the containers stay. The load's note reads like "Left on cell2: Forget unloads them first." Click **Forget…** on the note, which unloads the model first. See also [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind).

### Read failures and next actions

A temporary log-read failure retries when the head node has fresh online reports, after 30 seconds, 2 minutes and 10 minutes. **Try again** requests another read. A removed container cannot supply its old log. Successful reads stay cached for that container exit.

**Explain…** keeps one dialog per failed run. Its full decisive line remains available by moving the pointer over the shortened line. **Show log** reuses the same node/container log tab. **Copy summary** copies the displayed explanation. **Open script read-only** appears only when a script path is known; it selects the reported line in the editor and cannot save changes.

### Where the logs are

The load's note on the **Loaded** tab gives the path, after "Logs on this Mac:". SparkBox saves one file for each node, in `~/Library/Application Support/com.nemecj.sparkbox/failed-loads/`. Each failure has its own folder, named with the load's id and a time, and each file is named for its rank: `r0.log` for the head node, `r1.log` for the first worker. A one-node load has only `r0.log`. Only your Mac account can read them, and SparkBox takes known secrets out of each log before it saves it. They stay after you click **Forget**.

The **vLLM log** under the table shows each node's last 40 lines, under a line such as "── rank 1, worker on cell2, exit 1 ──".

### Common causes

| Likely cause, as SparkBox says it | What it means | What to do |
|---|---|---|
| "No attention kernel for a bf16 KV cache on GB10" | This image has no compatible attention kernel for the model and cache type | Check the model's recipe for a compatible checkpoint, image and cache type. Changing **KV cache type** alone may not fix it. SparkBox refuses `GLM-5.3-Flash` at any cache type. See [What's coming](help:whats-coming) |
| "Likely ran out of GPU memory while starting" | The engine asked for more than the node had | Lower **GPU memory share** or **Max context**, or free memory on the node |
| "Its port was likely already taken" | Something else is using the port | Pick another **Port**, or stop what holds it. **Workloads** shows it |
| "The disk likely filled up" | A write failed for lack of space | [Check disk space on a node](help:check-disk-space), free some, and load again |
| "Weights likely missing while offline (HF_HUB_OFFLINE)" | A file the model needs isn't in its folder | The folder is incomplete. [Delete it](help:delete-a-model), then download or copy it again |
| "Hugging Face likely refused the download (gated or no token)" | The model is gated | See [Get a Hugging Face token](help:get-a-hugging-face-token) |
| "Its quantization likely isn't supported on this GPU" | The image has no GPU kernel, the small piece of GPU code that does this work, for this format on a GB10 | Use a version made for this chip, such as NVFP4 or FP8, or a newer image. See [Model formats and quantization](help:formats-and-quantization) |
| "The nodes likely couldn't reach each other (NCCL)" | Nodes in a several-node load didn't connect | [Check the fabric](help:check-the-fabric), then load again |
| "The Ray cluster likely didn't form in time" | This one is for a server SparkBox didn't start. A worker didn't join the head node | Check that each worker is online and the fabric is healthy |
| "vLLM's engine didn't start; the first error is quoted" | The log names no cause SparkBox knows | Read the quoted lines. They're the best clue |

Each one is SparkBox's best reading of the log, which is why it says "likely". **Explain…** shows these words. A load's note quotes the log, so it may not use them. A suggested fix may mention a launch script. That wording is for runs started by hand: for a load from SparkBox, make the change on the **Load** page.

### The model answered 17+25 wrongly

The load fails with a message such as "It answers, but wrongly: asked 17+25, it said '4 2 2 2'. This is the GB10 garbage-output bug; try the fallback image (nvcr.io/nvidia/vllm:26.05-py3)."

The engine runs, but this image writes nonsense on a GB10. To use the fallback image:

1. SparkBox has already removed the load's containers and kept the logs. On its note on the **Loaded** tab, click **Forget**. If Admin mode is locked, macOS asks for Touch ID or your Mac password. Confirm it. If the button reads **Forget…**, containers are still on a node, and SparkBox unloads them first.
2. Open [Fleet Command](open:fleet) and pick only that node. Leave **as root** off, and set **Time limit** to `1800`.
3. Run `docker pull nvcr.io/nvidia/vllm:26.05-py3`. It's about 20 GiB. See [Run a command on every node](help:run-a-command-on-every-node).
4. Open the **Load** page for the model again. In **Image**, choose `nvcr.io/nvidia/vllm:26.05-py3`. It now reads "on cell2 · sha256:…".
5. Click **Load on 1 node**. A new image means a new test, so SparkBox asks 17+25 again.

### The launcher refused to start vLLM (exit 78)

A load with an API key needs the key file on its node. If that file is missing or empty, the launcher stops before vLLM starts, so the model can't run open with no key. The log reads "sparkbox: the API key file is missing or empty, so vLLM was not started.", and the container exits with code 78.

Load the model again: SparkBox makes a new key. If it happens again, check that the node's disk isn't full. See [Check disk space on a node](help:check-disk-space).

### The load ran out of memory

If a container exits with code 137, the node's operating system stopped it, usually for lack of memory. SparkBox says "Head ran out of memory (exit 137)" or similar. Lower **GPU memory share** or **Max context**. If the node froze, SparkBox can't restart it, because it can't reach it. Follow [Recover a frozen node](help:recover-a-frozen-node).

### Slow is not failed

The first time a model starts, the engine prepares its code for the GB10 chip and keeps the result on the node. That can take a long time with little to show. The card may read "Compiling: FlashInfer kernels", and any new line in the log counts as progress. After a long quiet spell, the card still reads **Loading**, and the line under it adds "no new progress for 25 min (limit 90 min)". That's a note, not the end. The load keeps going, and SparkBox waits up to the 90-minute limit. Only then does the load fail.

Before you unload it, look at the **vLLM log** on the **Loaded** tab. If lines are still arriving, wait. If the log hasn't moved for a long time and the GPU is idle on the [Dashboard](open:dashboard), unload it and look for a cause.

## What you should see

After your change, the load reaches **Ready**. If it fails again, its note on the **Loaded** tab shows the new cause. Fixing one problem often uncovers the next.

## Why it works

When a run fails, SparkBox reads the containers' state and the last 600 lines of each log. It finds the first real error, skipping the long chain of messages that follow it, and compares it with causes it knows. **Explain…** only reads, and the logs of a load you started are saved before anything is removed.

## If something goes wrong

- **"Its log names no known cause; the first error is below."** SparkBox doesn't recognize this failure. Read the quoted line, and search for it online.
- **"… stopped on cell2 (exit 1). Last log lines: …"** The engine exited while it started. The lines come from its log.
- **"vLLM didn't become ready within 90 minutes."** SparkBox stopped waiting, saved the logs and removed the containers. During the original launch, a node out of reach can reach this limit after 10 minutes away, and that node keeps its containers. After SparkBox reopens, recovery waits for fresh reports before judging a saved load. If the node froze, SparkBox can't restart it, because it can't reach it: follow [Recover a frozen node](help:recover-a-frozen-node). Otherwise read the log and load again with a lower **Max context**. See [the full fix](help:errors-models#vllm-didnt-become-ready-within-90-minutes).
- **"cell2: docker run failed: …"** Docker refused to start the container. Read the rest of the message. On several nodes, the head node never started.
- **"cell2: docker run didn't finish: the connection dropped before it answered. Check Containers on cell2, then unload or try again."** SparkBox lost the node before Docker answered, so the container may or may not have started. Look in [Containers](open:containers), then unload the model or load it again.
- **"cell2: SparkBox can't use Docker (is sparkuser in the docker group?)"** Your login on that node isn't allowed to use Docker. [Fix Docker access](help:prepare-the-cluster#fix-docker-access) has the command and where to run it.
- **There's no Failed row, but the model is gone.** A load you started that fails while it's starting has a note on the **Loaded** tab instead, because SparkBox removed its containers. Read the note and the **vLLM log**. If there's no note, you clicked **Forget**, or the run started outside SparkBox and failed more than 7 days ago. For a run like that, look in [Containers](open:containers) for its stopped containers.

For more messages, see [Model messages](help:errors-models).

## After SparkBox quits or loses its connection

SparkBox saves loads it started and the last observed members of discovered deployment groups on this Mac. Reopening restores those records. **Unknown** means it needs fresh reports; a saved success or exit is historical evidence.

The **Loaded** tab explains what happens next:

- **Recovering saved load:** SparkBox checks ownership and every expected node, then resumes watching the existing containers. It does not repeat the launch. An unreachable node keeps it waiting, even after a long absence.
- **Interrupted startup:** a fresh report confirmed a missing or stopped rank. Review the remaining containers. **Resume missing ranks…** can finish a first startup only when the existing ranks still run with the saved image and ownership. It never restarts an existing rank. A previously serving load needs Restart in order or Unload.
- **Owner review needed:** ownership changed or cannot be confirmed. Review the containers and their owner before changing them. SparkBox does not adopt another app’s containers.

A pending recovery blocks another load from taking the same nodes. A failed interrupted load is not restarted by **Keep running**. Unload and Restart keep their usual ownership checks and Admin requirements. Demo observations do not replace saved Live records. Discovered groups are retired when fresh reports confirm that all their containers have been removed; the saved observation cache keeps up to 256 groups.

## Next steps

- [Model messages](help:errors-models)
- [How memory works on a Spark](help:memory-on-a-spark)
- [Choose load settings](help:choose-load-settings)
- [Models: Loaded tab](help:models-loaded)
