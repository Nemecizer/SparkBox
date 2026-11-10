---
id: run-your-first-real-model
title: "Run your first real model"
type: tutorial
area: models
summary: "Load a real model on one of your nodes, check that it answers, chat with it, and unload it."
keywords: ["first real model", "real load", "run a model", "start a model", "get started with models", "gemma", "walkthrough", "step by step"]
verified: false
related: [your-first-model, prepare-the-cluster, load-a-model-on-one-node, tell-if-a-model-is-working, unload-a-model]
---

# Run your first real model

## What you'll do

You'll run a real model on a real node, ask it a question, and stop it again. The steps use `google/gemma-3-27b-it` on a node named `cell2` as a synthetic example. Substitute a model you are licensed to use and a node you configured. No models or node settings are included with SparkBox.

Allow time for model preparation and loading; the duration depends on your setup.

The preflight must confirm that your selected node meets this load's requirements. If a screen differs from what you read here, each stage says where to go.

The examples below need validation on your own hardware and software. [What's proven on your cluster](help:whats-proven) explains the limits of offline checks.

To practice first, follow [Rehearse running a model in Demo](help:your-first-model), where nothing is at risk.

## Before you start

- **Connect SparkBox to your nodes.** If the top bar doesn't read **Live** with your selected node online, follow [Connect your cluster](help:connect-your-cluster) first.
- **Use Live data with look-only mode off, and be ready to unlock Admin mode.** The top bar shouldn't read **Look only**. See [Switch between Demo and Live](help:switch-between-demo-and-live) and [Use look-only mode](help:use-look-only-mode).
- **Have Touch ID or your Mac password ready.** Admin mode lets SparkBox make changes. It may lock while you wait. That doesn't stop the model.
- **Be at home, or on the VPN.** Chat has to reach `cell2`. See [Reach your cluster with the VPN](help:connect-the-vpn).
- **Keep SparkBox open and this Mac awake until the model reads Ready.** SparkBox doesn't keep this Mac awake during a load. [Keep this Mac awake during long work](help:keep-this-mac-awake) shows how.

Some models require a provider account, license acceptance or a token. Check those requirements before downloading.

> [!WARNING]
> **This load reserves 75% of the memory on `cell2` until you unload it.** Anything else that needs that memory can slow down or fail. To undo it at any point, even before the model is ready, follow Step 7, which gives all of it back. Your other nodes don't change.

## Step 1: Check your nodes

First, check that `cell2` can run a model. This takes about a minute.

1. In the sidebar, under **Workloads**, click **Models**. [Open Prepare cluster](open:models?tab=prepare) The **Models** window opens.
2. Click the **Prepare cluster** tab.
3. Click **Check nodes** in the toolbar. This starts short test containers on each node, which remove themselves. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. The button reads **Checking…**.
4. Wait for the table. It has one row for each node.
5. Read the `cell2` row. **Docker** should read **✓ yes**. **vLLM image** should read **✓ on this node**.

If that row shows **✗**, apart from **✗ will be created**, or **not on this node; a load won't fetch it**, stop here. [Read the results](help:prepare-the-cluster#read-the-results) says how to fix it.

## Step 2: Look in your Library

Always look here before you download. You may already have a model.

1. Click the **Library** tab. It has one row for each model on your nodes, and one column for each node.
2. Find `google/gemma-3-27b-it`. It's about 51 GiB.
3. Read its **cell2** column. It should read **● found**. That means the folder is on the node, but SparkBox didn't download it, so it hasn't checked its files. That's expected for this model.

If the row isn't there, or **cell2** reads **—**, click **Find models** and wait. If it's still missing, use another model: [download](help:download-a-model) `Qwen/Qwen3.8-27B-FP8`, which goes to `cell1`. Then follow the rest of this page with that model and `cell1`.

## Step 3: Set up the load

Nothing on the node changes in this stage.

1. Click the `google/gemma-3-27b-it` row. A panel on the right shows its details.
2. Click **Load…** in the toolbar. The **Load** page opens, with the heading **Load a model with vLLM**.
3. Under **Nodes**, make sure `cell2` is selected and no other node is. `cell2` reads **has copy**. Under **Model**, **Recipe** reads **Gemma 3 27B**. That's SparkBox's saved description of how to load this model, and it fills in the next field.
4. Under **Split and limits**, check that **Max context** reads `32768`. If it's empty, click it and type `32768`. This is how long a conversation the model can hold, in tokens. A **token** is a small piece of text, about three quarters of a word, so `32768` is about 24,000 words.
5. Check that **GPU memory share** reads `0.75`. That reserves 75% of the node's memory for the model. SparkBox sizes the share to the model, and this one is big enough to stay at the limit for a new load. With `Qwen/Qwen3.8-27B-FP8` on `cell1`, it reads `0.45`.
6. Under **Memory plan**, read the bar for `cell2`. It should read **Fits**, with about "86% of its memory once loaded". That's more than the 75% in step 5, because it adds the engine's own process and what the node was already using. With `Qwen/Qwen3.8-27B-FP8`, it's about 57%. The dashed 90% line is the one that matters.
7. Under **Access**, leave **Listen on** at **LAN** and **API key** on. The line under **Listen on** reads "Apps reach it at `http://192.168.50.154:8000/v1`".
8. Check that **Image** starts with `vllm/vllm-openai:glm53-flash-arm64-cu130` and says "on cell2". For Qwen on `cell1`, choose `nvcr.io/nvidia/vllm:26.05-py3`, the image used in its successful loads. An **image** is the recipe a container starts from.
9. Under **Before it starts**, read the top line. It should say "Ready to load on cell2." Then check three rows. **What vLLM needs** ends "enough is free". **Model folder** reads "present on cell2". **Port** reads "8000 free on cell2".
10. If **Drop the page cache first** appears, select it. vLLM can't start until that memory is free. The drop is safe, because the node refills its cache as it's needed.

Leave every other field as it is. [Choose load settings](help:choose-load-settings) says when to change one.


If the bar doesn't read **Fits**, or a red notice appears, don't start yet. See [Read the memory plan](help:load-a-model-on-one-node#read-the-memory-plan). [Choose load settings](help:choose-load-settings) explains these choices.

## Step 4: Start the load

This is the step that changes `cell2`.

1. Scroll to the top of the page. There should be no message beside **Load on 1 node**. If there is, it says what's stopping the load. [If something goes wrong](help:load-a-model-on-one-node#if-something-goes-wrong) lists each message.
2. Click **Load on 1 node**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
3. The window moves to the **Loaded** tab, and a message reads "Loading gemma-3-27b-it". The model's row reads **Loading**.

SparkBox has now started vLLM, the engine that runs the model, in a **container**, a program running in its own sealed box on `cell2`.

## Step 5: Wait for Ready

**Ready** means the model has answered a test question, not only that a program is running.

1. On the model's row, click **Open in Endpoints**. The **Endpoints** window opens, with the model's card. The line under its name names the stage.
2. Wait. You can use the rest of SparkBox meanwhile. The first time a model starts, the engine prepares its code for the GB10, the chip in each Spark, and keeps the result on the node. So the card can stay on one stage for a long time.

| The card says | What's happening |
|---|---|
| "Checking the nodes…" or "Starting vLLM…" | SparkBox is checking the node and starting the container |
| "Loading weights 42%" | The engine is reading the model into memory |
| "Compiling: FlashInfer kernels" | The engine is preparing its code for the GB10. Expect the longest wait here or at the next line |
| "Capturing CUDA graphs" | The engine is preparing its code for the GPU |
| "Up; checking it answers…" | The engine is up, and SparkBox is testing it |
| **Ready** | The model answered |

3. When the card reads **Ready**, read the line under it. It should look like this:

"Ready: /health 200, gemma-3-27b-it listed, a test token in 0.4 s, 17+25 → 42 (first run of this recipe and image)"

That line says four things passed. The engine reported healthy, it listed your model, it wrote a test token, and it answered "What is 17+25?" correctly.

> [!NOTE]
> **A slow first start stays Loading.** After a long quiet spell, the line under the card's state adds "no new progress for 25 min (limit 90 min)". The load may still be working, and SparkBox keeps waiting up to the 90-minute limit. Before you do anything, read the **vLLM log** on the **Loaded** tab. If new lines are appearing, leave it alone.

To stop the load before it reads **Ready**, click **Unload…** on its card, as in Step 7. Quitting SparkBox doesn't stop it: the engine goes on starting on `cell2`, and keeps its memory, until you unload it.

If the model answers 17+25 wrongly, or the load fails, see [Fix a load that failed](help:fix-a-load-that-failed). Your files are safe, and you can load again.

## Step 6: Ask it a question

1. On the model's card, click **Chat with this**. The **Chat** window opens, with the model's name at the left of the toolbar.
2. Click the box at the bottom, type `Explain what a GPU does, in three sentences.`, and press <kbd>Return</kbd>. You see "Waiting for the first token…", and then the reply appears word by word.
3. Read the line under the reply. It shows how long the model took to start, and how fast it wrote, in tokens per second.

The reply came from `cell2`. It went straight from your node to this Mac, and nothing left your network.

## Step 7: Unload it

A loaded model holds its memory until you unload it, even when nobody is using it. This tutorial unloads it so that `cell2` ends as it started. A model you use every day can stay loaded: see [Can I leave a model loaded all the time?](help:model-questions#can-i-leave-a-model-loaded-all-the-time)

1. Go back to the **Endpoints** window and find the model's card.
2. Click **Unload…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
3. Read the dialog, **Unload gemma-3-27b-it?** Click **Unload**. A message reads "Unloading gemma-3-27b-it".
4. Open [Tasks](open:tasks) and wait for the task to end. Its summary should look like this:

"Unloaded from cell2 · memory back (116.5 GiB available, baseline 116.7) · port 8000 closed · /dev/shm: nothing new"

The model's files stay on `cell2`, so you can load it again at any time. Later loads should be faster, because the engine's first-start work is already done. To lock Admin mode now, click the padlock in the top bar. Otherwise it locks by itself.

## What you learned

- Check the **Library** first, because a model you already have needs no download.
- **Check nodes** tells you whether a node can run a model.
- **Max context** and **GPU memory share** decide how much memory a load takes, and the **Memory plan** shows the result before anything starts.
- **Ready** means the model has answered, and that's when it appears in **Chat**.
- Unloading gives the memory back and leaves the files on disk.

## Next steps

- [Find a model that fits](help:find-a-model-that-fits), to choose your next model
- [Download a model](help:download-a-model)
- [Tell whether a model is working](help:tell-if-a-model-is-working)
- [Common questions about models](help:model-questions)
