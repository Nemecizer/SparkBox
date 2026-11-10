---
id: load-a-model-on-several-nodes
title: "Load a model on two, three or four nodes"
type: howto
area: models
summary: "Split a model across nodes: pick its recipe, nodes, split and image, pass Before it starts, and wait for Ready."
keywords: ["multi-node", "two nodes", "three nodes", "four nodes", "cluster load", "several nodes", "all nodes", "across the cluster", "across nodes", "run a model across the cluster", "split a model", "too big for one spark", "too large", "too big", "my model is too large", "model too big", "workers first"]
verified: false
related: [splitting-a-model, copy-a-model-to-other-nodes, fix-a-load-that-failed, use-a-recipe-for-a-load, clear-what-an-earlier-load-left-behind]
---

# Load a model on two, three or four nodes

To run a model too big for one Spark, select several nodes on the **Load** page and pick a split the model can take.

> [!WARNING]
> **A small Qwen model has served on two and four nodes on your cluster.** The four-node test had an Admin-mode pause and a switch management-login alert whose cause remains unknown. Other models, larger loads and different splits still need their own checks. Treat a new configuration as an experiment, and use one node if it serves the model well. If a start fails, SparkBox removes what it started and keeps the logs. To stop a load yourself, [unload it](help:unload-a-model).

[Running one model on several nodes](help:splitting-a-model) explains how the parts fit together.

## Before you start

- You've read the model's fit table. See [Find a model that fits](help:find-a-model-that-fits).
- The model is on **every** node you'll use, at the same path, with the same files. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes).
- Every node you'll use has the same image. An **image** is the recipe a container starts from. SparkBox doesn't fetch it during a load. To fetch one, see **To fetch an image** in [Load a model on one node](help:load-a-model-on-one-node#if-something-goes-wrong).
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the load is simulated.
- None of those nodes has another SparkBox load or an Ollama model in memory. Loads on other nodes are fine: two loads run side by side when they share no node.
- The fabric works on every node you'll use. [Prepare the cluster for models](help:prepare-the-cluster) shows each node's fabric details.
- It may take a while. The first time a model starts, each node prepares code for its GB10 chip. The small two-node Qwen test took about 2 minutes 23 seconds; larger models can take much longer. SparkBox gives up after 90 minutes.
- **Keep SparkBox open and this Mac awake until the model reads Ready.** SparkBox doesn't keep this Mac awake during a load, and it starts the nodes itself. [Keep this Mac awake during long work](help:keep-this-mac-awake) shows how. See also [If this Mac sleeps or SparkBox closes](#if-this-mac-sleeps-or-sparkbox-closes).

## Steps

### Set up the load

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=library) The **Models** window opens on the **Library** tab.
2. Click your model's row, and check that each node you'll use has a mark in its column.
3. Click **Load…**. The **Load** page opens, usually with one node selected.
4. Under **Model**, read **Recipe**. It names the settings SparkBox picked for this model, and says if they can't load it. See [Use a recipe for a load](help:use-a-recipe-for-a-load).
5. Under **Nodes**, select each node you'll use. The button changes to **Load on 3 nodes**, for example. A note reads "Each model, image and split needs its own checks. A successful load on one layout doesn't prove another." Click **What's proven** for the recorded live results and their limits. If a selected node has no copy, another note says so.

> [!NOTE]
> The first node in your node list becomes the **head node**, which answers requests, whatever order you select them in. The others are **workers**, which join it over the fabric. With Admin mode unlocked, the note about a missing copy offers **Copy it there**.

6. Under **Split and limits**, read **Split**. This menu lists the splits for that many nodes, and SparkBox picks the first one this model can take. **Tensor × pipeline parallel** shows the same two numbers. To change the split, use the table below, with the **Split** column of the model's fit table.

| Nodes | The menu reads | What you get |
|---|---|---|
| 2 | **2 × 1: tensor parallel 2** | One reply can run up to twice as fast |
| 2 | **1 × 2: pipeline parallel 2** | It fits, with no speedup for one reply |
| 3 | **3 × 1: tensor parallel 3** | One reply can run up to three times as fast. It works only if the model's attention heads divide by 3, and many models' don't |
| 3 | **1 × 3: pipeline parallel 3** | It fits, with no speedup for one reply. SparkBox picks this when the model can't take 3 × 1 |
| 4 | **4 × 1: tensor parallel 4** | One reply can run up to four times as fast |
| 4 | **1 × 4: pipeline parallel 4** | It fits, with no speedup for one reply |
| 4 | **2 × 2: tensor and pipeline parallel, advanced** | Two pipeline stages of two nodes each |


The menu marks a split this model can't take with "(won't work)". **Attention heads** are the parallel parts inside each of the model's layers, and they have nothing to do with the head node. If you pick a split that can't divide them evenly, a note says why, such as "Tensor parallel 3 won't work for this model: 32 heads don't divide by 3. Use pipeline parallel 3." **Load on …** stays dimmed until you pick another split.

A recipe can list fewer splits. The menu marks one it leaves out with "(not in this recipe)", because the model can still take it, and the note reads like "The Gemma 3 27B recipe doesn't list this split. Pick Generic vLLM to try it."

7. If the model is marked **MoE**, a [mixture of experts](help:splitting-a-model#expert-parallel), turn on **Expert parallel**. Leave it off for any other model.
8. If the recipe hasn't filled in **Max context**, type a number, such as `32768`.
9. If the fit table reports no supported GPU kernel, stop and check the model and image. Automatic or bfloat16 KV is blocked for sparse MLA on GB10; fp8 alone does not prove compatibility. See [Sparse MLA on a GB10](help:splitting-a-model#sparse-mla-on-a-gb10).
10. Under **Access**, pick the image in **Image**. Each line says which chosen nodes have it, such as "on cell1, cell2 · sha256:5a0f8b91…; not on cell3". Pick an image with one short id and no "not on". "not read on cell3" means SparkBox couldn't list that node's images, so it doesn't know what's there.

### Check the nodes

11. Under **Memory plan**, read the bar for each node. Every node has its own. They differ, because each node has its own load already. **GPU memory share** is one number for all of them, sized to the node that needs the most, and it changes with the nodes and the split.
12. If any bar is red, use the button SparkBox offers, or lower **Max context** or **GPU memory share**.
13. Scroll to **Before it starts**. SparkBox reads every chosen node as a set. The top line is one verdict for all of them, such as "Ready to load on cell1, cell2 and cell3." Beside it, "Checked 2 min ago" says how old the reading is, and **Check again** reads the nodes again. Below is a block for each node, with its rank.
14. If the verdict reads "Not ready to load on …", read the red notes under it. Each one says what's wrong. [Model messages](help:errors-models#before-it-starts) lists them. For containers an earlier load left, follow [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind).
15. If you see **Drop the page cache first**, select it. It names the nodes that need it. The drop is safe: each node refills its cache as it's needed. See [the page cache](help:memory-on-a-spark#the-page-cache).
16. Under **What will run**, read the steps. They run from the top: the checks, the fabric test, then the launch files and the start for each node, workers first and the head node last.

### Start it

17. Click the **Load on …** button at the top. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
18. SparkBox reads every node again, and tests the fabric: each worker sends jumbo frames, large test packets, to the head node on each half. A problem stops the load here, with nothing started.
19. The window moves to the **Loaded** tab. Click **Open in Endpoints** to watch the card. The table below says what each stage means.
20. Wait for **Ready**. SparkBox allows up to 90 minutes.

| The card says | What's happening |
|---|---|
| "Checking the nodes…" or "Starting vLLM…" | SparkBox is writing each node's launch files and starting its container |
| "2 of 4 nodes up" | That many containers are running. The workers start first, the head node last, and then they link up over the fabric |
| "Loading weights 42%" | Each node is reading its part of the model into memory |
| "Compiling: FlashInfer kernels" | The engine is preparing its code for the GB10. A first start can spend a long time here |
| "Capturing CUDA graphs" | The engine is preparing its code for the GPU |
| "Up; checking it answers…" | The engine is up, and SparkBox is testing it |
| **Ready** | The model answered |

To stop a load before it's **Ready**, click **Unload…** on its card. Quitting SparkBox doesn't stop it: containers that have started keep running, and keep their memory, until you unload them.

## What you should see

**State** reads **Ready**, and the **Nodes** column lists each node, head node first. The card in [Endpoints](open:endpoints) shows the split, such as **TP 4**. The model answers on the head node's address. In the [activity log](help:find-what-sparkbox-did), each node has a row for its launch files, then a row such as "Copy launch.py to cell2" if its launcher needed replacing.

Verify a fresh Chat reply and review memory, alerts and network behavior on every participating node. After the test, unload normally and review the cleanup report. An earlier test on different equipment is not proof of stability on your setup. See [Unload a model](help:unload-a-model).

## Why it works

SparkBox starts one container on each node, from one image picked by its local id, and points each container at its own fabric addresses. A small launcher starts vLLM with no shell in the path, so what you read under **What will run** is what runs. The workers start first and wait, and the head node starts last. Before **Ready**, SparkBox runs the same checks as for one node, including the 17+25 test. Only the head node has the API key.

## If a start fails

If a node's container won't start, a container stops while the load is starting, the load runs out of time, or the model answers 17+25 wrongly, SparkBox cleans up:

1. It saves the end of each node's log to this Mac.
2. It unloads the model from every node, head node first, and checks `/dev/shm`.
3. It marks the load **Failed**, with the cause and each node's last lines.

If a worker fails to start, the head node never starts. SparkBox never retries a first start by itself. See [Fix a load that failed](help:fix-a-load-that-failed).

If look-only mode is on, or a node is out of reach, SparkBox can't remove everything. The load's note on the **Loaded** tab names the nodes that still hold containers. To remove them, click **Forget…** on that note, which unloads the model first.

## If this Mac sleeps or SparkBox closes

The running model doesn't need this Mac, but starting one across nodes does. This differs from a one-node load.

- **Until every node's container has started,** SparkBox is doing the work over its connections to the nodes. That's while the card reads "Checking the nodes…" or "Starting vLLM…". The load can't finish without them.
- **If SparkBox closes then,** it checks every expected node when it opens again. Unreachable nodes wait for fresh reports. Missing or stopped workers show **Cleanup needed**; SparkBox does not start replacements or automatically remove the remaining workers. Review the containers and choose **Unload** before loading again. See [After SparkBox quits or loses its connection](help:fix-a-load-that-failed#after-sparkbox-quits-or-loses-its-connection).
- **If this Mac sleeps then, with SparkBox open,** the start fails when the connection drops. The load's note on the **Loaded** tab gives the reason. It names any node that still holds containers, because SparkBox may not reach a node to remove them. Click **Forget…** on the note, or check [Containers](open:containers). Then load the model again.
- **Once the head node has started,** the load goes on by itself on the nodes. SparkBox picks it up the next time it opens with **Live** data, checks every rank, and watches readiness until the model reads **Ready**.
- **After Ready,** you can quit SparkBox, or let this Mac sleep. The model keeps serving on its nodes. Chat and the Server run in SparkBox on this Mac, so they stop while it's closed or asleep.
- **Keep running is different.** It needs SparkBox open, as the limits below say.
- **When Admin mode locks, the load keeps going.** Admin mode locks after 15 minutes idle by default, when the screen locks, and when this Mac sleeps. That doesn't stop a load that's already running.

## Limits

The revised vLLM shutdown passed a fresh `Qwen/Qwen3-0.6B` test across `cell1` and `cell2` on November 6. Both nodes returned memory and left no new shared-memory files after normal **Unload**, without manual cleanup. The earlier warning remains in the history; see [What's proven on your cluster](help:whats-proven).

- **The two- and four-node Qwen tests cover only that model, image and those splits.** Three-node loads, pipeline splits, larger models, full connection outages, higher concurrency, continuous saturation and longer runs still need live tests. The four-node interruption and management-login alert still need review. Your earlier real 4-node load, `GLM-5.3-Flash`, failed: it exited with code 1 on all 4 nodes after 54 seconds. That run came from a launch script on the nodes, and it used a bf16 KV cache, which this model can't use on a GB10.
- **A load doesn't fetch images, or share them between nodes.** Each node needs the image already. See [What's coming](help:whats-coming).
- **SparkBox can't group copies of a model.** You can load a model on two separate sets of nodes as two loads, but SparkBox can't put them under one name or spread requests between them yet. These have run only on test systems. See [What's coming](help:whats-coming).
- **Keep running** works only while SparkBox is open on this Mac. It restarts the load in order after 30 seconds, then 2, 5 and 10 minutes, and gives up after 5 restarts in 30 minutes, or 5 in a row that don't reach **Ready**. It doesn't restart a load that has never been **Ready**, one that's still starting or compiling, or one whose start gave the wrong answer or ran out of time. Its card reads "Keep running · It failed to start: won't restart it". A restart that fails after the load had been **Ready** counts as one of the 5, and SparkBox tries again. It needs no Touch ID, so it acts with Admin mode locked. The one exception is a node that needs its page cache dropped: that restart fails, and says so, unless Admin mode is unlocked. If this Mac is off, nothing restarts the load.
- If one worker stops, the whole model goes down. See [Restart a model or change its settings](help:restart-or-change-a-model).
- **Containers on several nodes run with `--privileged`.** If you turn on [Allow remote code](help:models-load-page#allow-remote-code), the model's own Python runs as root with every device on the node, so only allow it for a repository you trust.

## If something goes wrong

- **"Tensor parallel 3 won't work for this model: 32 heads don't divide by 3. Use pipeline parallel 3."** Set **Tensor × pipeline parallel** to `1` × `3`. See [Running one model on several nodes](help:splitting-a-model#tensor-parallel).
- **"Copy the model to cell3 first"** A node has no copy. Click **Copy it there**. That button appears only when Admin mode is unlocked.
- **"Inventory expectations need review."** Compare the warning with **Settings › Nodes › Fabric and load role**. Review old expectations or correct the node configuration separately. A role preference never changes which selected node becomes the head.

- **"cell3 has no fabric address (ConnectX-7 with an IPv4 address)."** See [Check the fabric](help:check-the-fabric).
- **"cell3 has containers left from an earlier load (sparkbox-m1a2b3-r1). Remove them first."** See [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind).
- **"The jumbo-frame path from cell2 to cell1 failed on enP2p1s0f1np1: the model's traffic can't cross the fabric."** Nothing started. See [Check the fabric](help:check-the-fabric).
- **"The worker (rank 1) on cell2 stopped while the load was starting (exit 1)."** That node's container ended, so SparkBox removed the rest and kept the logs. See [Fix a load that failed](help:fix-a-load-that-failed).
- **The card stays on "4 of 4 nodes up".** Every container is running, but the nodes haven't linked up. Read the **vLLM log** on the **Loaded** tab. If it stops moving, unload the model and [check the fabric](help:check-the-fabric).
- **"It isn't using the RDMA fabric: its message queue is on 192.168.50.51, which isn't a fabric address."** Part of the load's traffic isn't on the fabric. You see this on the **Loaded** tab and in the load's message. The load keeps going and can read **Ready**, but it can run slowly, and it isn't running as designed. Click **Copy** on the **vLLM log** first, because unloading removes that log and it's what to send if you ask for help. Then [unload the model](help:unload-a-model) and [check the fabric](help:check-the-fabric).
- **"No attention kernel for a bf16 KV cache on GB10"** This is what stopped your `GLM-5.3-Flash` run. SparkBox now refuses that model at any **KV cache type**, with "Needs the FP4-Local checkpoint and its image, see What's coming". See [What's coming](help:whats-coming). For another sparse MLA model, check support in the selected image before loading; fp8 alone is not proof.
- **The card stays Loading, with "no new progress for 25 min (limit 90 min)".** The load may still be working, and SparkBox keeps waiting up to the limit. Read the log before you act. See [Slow is not failed](help:fix-a-load-that-failed#slow-is-not-failed).

For the others, see [Model messages](help:errors-models).

## Next steps

- [Tell whether a model is working](help:tell-if-a-model-is-working)
- [Fix a load that failed](help:fix-a-load-that-failed)
- [Running one model on several nodes](help:splitting-a-model)
- [What's proven on your cluster](help:whats-proven)
