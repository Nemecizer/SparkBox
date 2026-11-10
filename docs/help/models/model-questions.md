---
id: model-questions
title: "Common questions about models"
type: reference
area: models
summary: "Short answers to what people ask first: which model, what it costs, and what happens if this Mac sleeps."
keywords: ["faq", "common questions", "questions", "leave a model loaded", "leave a model running", "keep a model running", "always on", "does it need sparkbox open", "close my laptop", "mac sleeps", "quit sparkbox", "does my model stop", "cost", "how many models", "unload or delete", "privacy", "how big a model", "biggest model", "lid", "close the lid", "keep my mac awake", "two models", "two models at once", "more than one model", "several models", "first load", "first start", "why is the first load slow", "private", "is my data private"]
verified: false
related: [run-your-first-real-model, find-a-model-that-fits, load-a-model-on-one-node, tell-if-a-model-is-working]
---

# Common questions about models

This page answers the questions people ask first about running models with SparkBox. Each answer links to the full steps.

## Choosing a model

### Which model should I start with?

Look in your **Library** first, because a model you already have needs no download. `google/gemma-3-27b-it` is on `cell2`, and [Run your first real model](help:run-your-first-real-model) takes you through it.

For a first download, pick `Qwen/Qwen3.8-27B-FP8`. It fits on one Spark. [Find a model that fits](help:find-a-model-that-fits) explains the others.

### Does running a model cost money?

A model on your own nodes costs nothing per message. Downloading from Hugging Face is free, though a large download uses your internet allowance if you have one.

**Check with AI** and chats with a cloud model use your own Anthropic or OpenAI account, and your provider may charge for them.

### How big a model can one Spark run?

A model of about 60 GiB fits: `openai/gpt-oss-120b`, at 60.8 GiB, runs on one Spark. `meta-llama/Llama-3.3-70B-Instruct`, at 131 GiB, doesn't.

Each Spark has about 121.7 GiB of memory, and a load reserves a share of it sized to the model, up to 75%. A model's weights have to fit inside that share, with room left for the conversation. The fit table works this out for each model. See [How memory works on a Spark](help:memory-on-a-spark).

## Keeping a model running

### Do I have to keep SparkBox open while a model runs?

Not once it reads **Ready**. The model runs on the node, and SparkBox only starts it, tests it and watches it. If you quit SparkBox, the model keeps serving, and apps that use the node's address keep working.

**Chat** and the Server run in SparkBox on this Mac, so they stop when you quit. If you only close the window to the menu bar, SparkBox keeps running.

### Can I leave a model loaded all the time?

Yes. A model you use every day can stay loaded.

It costs two things, even when nobody is using it. It keeps the memory it reserved, which is 75% of the node's memory by default. That node also can't take another SparkBox load until you unload this one. After 24 hours with no requests, SparkBox lists a **Loaded but idle** alert. That alert is for information only, and it never pages you.

| If this happens | The model |
|---|---|
| You quit SparkBox, or this Mac sleeps | Keeps serving. Apps that use the node's address keep working. **Chat** and the Server stop until SparkBox is back |
| Admin mode locks | Keeps serving |
| It crashes, or the node restarts | Stops, and stays stopped, unless you turned on **Keep running** |

To have a model come back by itself, turn on [Keep running](help:models-load-page#keep-running) when you load it. On several nodes, that needs SparkBox open on this Mac. For a model that's already loaded, click **Edit & reload…** on its card, turn on **Keep running** and click **Reload**. That stops the model, loads it again and makes a new API key. See [Restart a model or change its settings](help:restart-or-change-a-model).

Unload a model when you want its memory back for something else. Its files stay on disk.

### What happens if I close my laptop, or this Mac sleeps?

**Closing a MacBook's lid puts it to sleep, even when it's plugged in,** unless it's connected to an external display, keyboard and mouse. SparkBox doesn't keep this Mac awake during a download, a copy or a load. [Keep this Mac awake during long work](help:keep-this-mac-awake) shows how to set that up.

What happens next depends on what's running.

| What's running | If this Mac sleeps or SparkBox closes |
|---|---|
| A download or a copy | The job runs on the node and is built to keep going, but that hasn't been proven on your cluster. Keep this Mac awake until it ends |
| A load on one node, before **Ready** | The engine goes on starting on the node. SparkBox has to be open to test it, so it can't read **Ready** until you open SparkBox again. A sleep that runs past the 90-minute limit can show the load as failed, with "vLLM didn't become ready within 90 minutes." |
| A load on several nodes, before **Ready** | SparkBox starts each node itself, and the head node goes last. If it's interrupted before the head node has started, the load can't finish. See [If this Mac sleeps or SparkBox closes](help:load-a-model-on-several-nodes#if-this-mac-sleeps-or-sparkbox-closes) |
| A model that reads **Ready** | It keeps serving. **Keep running** on several nodes needs SparkBox open, so nothing restarts that model until SparkBox is back |

### What happens when Admin mode locks?

Nothing that's already running stops. Admin mode locks after 15 minutes idle by default, when the screen locks, when this Mac sleeps and when SparkBox quits. Downloads, copies and loads keep going. You unlock it again only to make the next change.

### What if a node freezes?

A Spark that runs out of unified memory can stop answering, and SparkBox can't restart it from where you are. Someone has to go to the machine. [Recover a frozen node](help:recover-a-frozen-node) has the steps, and says what to change before you load the model again.

### Does a model come back after a node restarts?

Only if you turned on [Keep running](help:models-load-page#keep-running) when you loaded it. Otherwise, load it again. Your files stay on disk, so there's nothing to download.

## Memory and disk

### How many models can I run at once?

One SparkBox load on each node. So you can run up to four models that each fit on one Spark, or fewer if one model spans several nodes.

### What's the difference between unload and delete?

[Unload](help:unload-a-model) stops the model and gives its memory back. The files stay, so you can load it again. [Delete](help:delete-a-model) removes the files from a node and gives the disk space back. You'd have to download or copy the model to use it there again.

### Why does the first start take so long?

The first time a model starts, the engine prepares its code for the GB10 chip and keeps the result on the node. That can take a long time with little to show. Later loads of the same model should be faster, because that work is already done. SparkBox waits up to 90 minutes. See [Fix a load that failed](help:fix-a-load-that-failed#slow-is-not-failed).

## Privacy

### Does what I type leave my network?

Not with a model on your own nodes. Your message goes straight from this Mac to the node, and through your VPN if you're away from home. With a cloud model, it goes to that provider. See [Chat with a cluster model](help:chat-with-a-cluster-model).
