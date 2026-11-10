---
id: tell-if-a-model-is-working
title: "Tell whether a model is working"
type: howto
area: models
summary: "Read a model's state, its test answer and its speed, and tell Ready from Loading, Down and Stopped."
keywords: ["ready", "working", "down", "stalled", "compiling", "17+25", "is it running", "is it working", "is my model working", "not responding", "model not answering", "test"]
verified: false
related: [endpoints, chat-with-a-cluster-model, fix-a-load-that-failed]
---

# Tell whether a model is working

To find out whether a model is working, read its state and the lines on its card in **Endpoints**, then ask it a question.

## Before you start

- You have a model loading or loaded. See [Load a model on one node](help:load-a-model-on-one-node).
- Reading needs no Admin mode, and it also works in look-only mode.
- With **Demo** data, you see simulated models.
- Asking a question needs look-only mode off, and this Mac has to be able to reach the model.
- You can check at any time. Once a model reads **Ready**, it keeps running on the node whether SparkBox is open or not. See [Common questions about models](help:model-questions).

## Steps

1. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) You see one card for each model server.
2. Find your model's card by its name. The word beside the name is its state.


3. Read the state, using the table below.
4. If it reads **Ready**, read the lines under the name. The next section says what each one means.
5. Click **Chat with this**, type a short question and press <kbd>Return</kbd>. A sensible answer is the best proof there is.

| State | What it means | What to do |
|---|---|---|
| **Ready** | It answers, and it wrote a token recently | Use it |
| **Loading**, or **Loading 42%** | It's starting. The line under the name says which stage, such as "Compiling: FlashInfer kernels" | Wait, or **Unload…** to stop it |
| **Unknown** | A node is unavailable or has no current report. The model may still be running | Restore the connection and wait for fresh checks |
| **Down** | It should be serving and it isn't. A red line says why | See [Fix a load that failed](help:fix-a-load-that-failed) |
| **Stopped** | It ended, and it isn't meant to be running | Load it again if you want it |

For the other states, and the numbers on the card, see [Endpoints](help:endpoints).

## Read the proof lines

A card for a model SparkBox loaded can have up to three lines of proof.

**What it did most recently.** One of these:

- "Answered 12 s ago · 1 token in 84 ms". SparkBox sends a test token, a request for one token, about once a minute.
- "Generating · live traffic". Real requests are coming in, and the model is writing tokens, so no test is needed.

**What the first check found.** For example:

"Ready: /health 200, gemma-3-27b-it listed, a test token in 0.4 s, 17+25 → 42 (first run of this recipe and image)"

That line says four things passed: the engine reported healthy, it listed your model, it wrote a test token, and it answered a test question correctly. On a later load with the same settings, the line ends "(17+25 was checked on an earlier load of this recipe and image)".

**Which request styles it speaks.** For example, "Native API (from /openapi.json): Messages ✓ · count_tokens ✓ · Responses ✗". This lists what the engine can do by itself. A **✗** here isn't a fault. Chat works either way.

## What you should see

A healthy model reads **Ready**, with a recent "Answered" or "Generating" line, and it gives a sensible answer in **Chat**. The **Generate** number on the card shows how fast it's writing, in tokens per second, while it's busy. It reads low or zero when nothing is being asked.

## Why it works

SparkBox doesn't trust a model because its program is running. An engine can start, report healthy and still write nonsense. So a load must pass four checks before it first reads **Ready**. Its `/health` address answers, its model list names your model, it writes one token, and it answers 17+25 correctly. After that, a model stays **Ready** only while it keeps answering and has written a token in the last 2 minutes.

## If something goes wrong

- **"Up; checking it answers… (…)"** The engine has started, and one of the checks hasn't passed yet. SparkBox keeps asking. The words in brackets give the reason.
- **"Up; checking it answers… (Look-only mode holds the test token; it's checked once look-only ends.)"** Look-only mode doesn't let SparkBox send a test. Turn it off and the check goes on.
- **"It answers, but wrongly: asked 17+25, it said '4 2 2 2'. This is the GB10 garbage-output bug; try the fallback image (nvcr.io/nvidia/vllm:26.05-py3)."** The load has failed. SparkBox has already removed its containers and kept the logs. See [The model answered 17+25 wrongly](help:fix-a-load-that-failed#the-model-answered-1725-wrongly).
- **"Asked 17+25, it gave no answer within 512 tokens; it was still thinking ('…'). It's asked again at the next check."** A model that thinks at length used up its allowance. SparkBox doesn't count this as wrong.
- **Loading, with "Compiling: …".** The engine is preparing its code for the GB10. A first start can spend a long time here. Any new line in the log counts as progress, and **Keep running** leaves it alone.
- **Loading, with "no new progress for 25 min (limit 90 min)".** The load may still be working. SparkBox keeps waiting up to the limit. Read the log before you do anything. See [Slow is not failed](help:fix-a-load-that-failed#slow-is-not-failed).
- **Down, with "Stalled: 3 requests running, no tokens for 60 s".** The engine may still answer `/health`, but it's stopped writing. Requests are running or waiting, and no token has come out for 60 seconds. It clears by itself if tokens start to flow again. If not, read its log, then restart it. See [Restart a model or change its settings](help:restart-or-change-a-model).
- **Down, with "Head exited (code 1) 2 min ago · worker on cell3 stopped first (exit 137) · tensor-parallel rank 2 of 2".** A head node exits when it loses a worker, so the worker named here is the likely cause. "stopped with it" in place of "stopped first" means both stopped in the same second, and Docker's clock can't say which went first. A head node that exits cleanly after a worker crashed reads **Down** too, not **Stopped**. Read the worker's log. See [Fix a load that failed](help:fix-a-load-that-failed).
- **Down, with "Didn't become ready in 20 min".** This appears on a server SparkBox didn't start, when it stops making progress. Compile lines in its log count as progress. Read that server's log. While a load SparkBox started is starting, it stays **Loading** instead.
- **Chat with this is dimmed.** The model isn't **Ready**, or this Mac can't reach it. A **Fabric only** load can't be reached from this Mac. See [Chat with a cluster model](help:chat-with-a-cluster-model).
- **Ready, but the answers are poor.** The model is working, and that's its quality. Try a larger or less quantized version. See [Model formats and quantization](help:formats-and-quantization).

## Next steps

- [Chat with a cluster model](help:chat-with-a-cluster-model)
- [Benchmark a model](help:benchmark-a-model), to measure its speed
- [Fix a load that failed](help:fix-a-load-that-failed)
- [Endpoints](help:endpoints), to see everything on the card
