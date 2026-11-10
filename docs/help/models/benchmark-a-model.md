---
id: benchmark-a-model
title: "Benchmark a model"
type: howto
area: models
summary: "Measure a loaded model's speed and delay at different loads, and save the results."
keywords: ["benchmark", "speed", "performance", "tokens per second", "ttft"]
verified: false
related: [models-benchmarks, tell-if-a-model-is-working]
---

# Benchmark a model

To measure a model's speed, run a sweep on the **Benchmarks** tab, which tests it at several levels of load and saves the results.

A **sweep** sends the model test requests of an exact length, first one at a time, then more and more at once.

## Before you start

- A model is **Ready**. See [Tell whether a model is working](help:tell-if-a-model-is-working).
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the results are simulated.
- A sweep keeps the model busy for minutes, so SparkBox treats it as a change.
- Nothing else should be using the model. Other requests change the result, and the sweep slows them down.
- The time depends on the model's speed. The **Chat** profile at the default levels sends 32 requests of 256 tokens each, plus one warm-up.

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Benchmarks](open:models?tab=bench) The **Models** window opens.
2. Click the **Benchmarks** tab.
3. In **Endpoint**, choose your model. The list shows each model that's ready, such as "llama-3.3-70b — cell1 :8000 (external)".
4. Under **Profile**, choose the length of the test. **Chat** is a good start.
5. Under **Requests at once**, select the levels to test. Each level you select adds time. `1`, `2`, `4` and `8` are selected at first.
6. Click **Run benchmark**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A bar shows progress, with text such as "4 at once (8 requests)".
7. Wait for the sweep to end. To stop early, click **Stop**. Levels that finished are kept and saved. Stopping needs no Admin mode.
8. Read the chart and the table on the right.
9. To save the numbers, click **Save CSV** or **Save JSON** above the table. The file goes in your **Downloads** folder, and a message shows its path.

The profiles:

| Profile | Tokens in → out | Good for |
|---|---|---|
| **Chat** | 512 → 256 | A short prompt and a medium answer |
| **RAG** | 4,096 → 256 | Documents in, a short answer |
| **Long** | 32,768 → 512 | Long context. The load's **Max context** must allow it |
| **Custom** | You choose | Matching your own use |

## What you should see

The right side shows a chart with a line for the run, and a table with one row for each level. The run also appears under **Runs** on the left.

**The table:**

| Column | What it means |
|---|---|
| **At once** | How many requests ran together |
| **Done** | How many finished. **(2 failed)** in red means some didn't |
| **Req/s** | Requests finished each second |
| **Output tok/s** | Tokens written each second, all requests together |
| **First token** | Time to first token, or **TTFT**: how long before each reply starts |
| **Per token** | The time to write each token after the first |
| **Between tokens** | Every gap between tokens as they arrive |

The time columns show three numbers, such as "266 / 274 / 279 ms". They're **p50**, **p90** and **p99**. Half of requests were at or under p50, 90% at or under p90, and 99% at or under p99. p50 is the typical case. p99 is close to the worst.

**The chart:** each point is one level. Across is total speed, and up is the time per token. Down and to the right is better. As you add requests, total speed rises and each reply slows. Where the line turns sharply upward, the model has reached its limit.

To compare runs, select up to 4 under **Runs**. Each gets its own color.

A bounded live test on November 6, 2026 used `Qwen/Qwen3-0.6B` across `cell1` and `cell2`, with tensor parallel 2. Ten **Custom** runs used 512 prompt tokens, 3,072 output tokens and only **1** under **Requests at once**. All 40 measured replies passed during a 15-minute window, at about 164–166 output tokens per second. Fresh Chat and clean unload passed afterward. Other profiles, higher concurrency and file export still need their own live checks.

A November 7 single-node check used the same Qwen model on `cell1`, with 4,096 context and 0.20 memory share. After 30 minutes loaded idle, 16 **Custom** batches ran within a 30-minute window. All 64 measured replies passed, one request at a time. Fifteen batches used 512 prompt and 3,072 output tokens; the last used 1,024 output tokens to finish within the window. There were short gaps between batches. No alerts, swap activity or restarts occurred, and available memory stayed near 88.7–88.9 GiB. Fresh Chat and normal unload passed. This does not test higher concurrency or low-memory recovery.

On November 8, the same model passed 30 minutes loaded idle and 30 minutes of repeated work across `cell1` and `cell2`, with tensor parallel 2. All 80 measured replies passed across 20 **Custom** batches: 512 prompt tokens, 3,072 output tokens and one request at a time. Speed was about 165–168 output tokens per second. There were short gaps between batches. No alerts, OOM kills or restarts occurred. Cell2 read one 4 KiB page from its existing swap; neither node wrote new pages to swap. Fresh Chat and clean unload passed on both nodes. This does not establish continuous saturation or pressure recovery.

A later four-node test used the same model and settings with tensor parallel 4. Thirty minutes loaded idle passed. The 30-minute workload allocation was completed in two parts after a Mac screen lock relocked Admin mode.

Work resumed only after normal unlock and fresh health checks. All 88 measured replies passed across 22 **Custom** batches, at about 185–202 output tokens per second. Available memory stayed above 86 GiB on every node, with no memory alerts, swap-out, OOM kills or restarts.

A critical switch management-login alert appeared, cleared and returned while waiting to restore notifications; its cause remains unknown. Fresh Chat and normal unload passed, followed by all 19 health and cleanup checks per node. This was neither an uninterrupted nor an alert-free test, and it does not establish a speed advantage over other splits.

## Why it works

SparkBox's test requests use exact token counts, so each run is the same size. It tells the engine to write the full reply and not stop early, so the output length is exact too. One warm-up request runs first, so a slow first request doesn't spoil the numbers.

These are real measurements of your model, sent from this Mac, so the times include the network between this Mac and the node.

## Limits

- Benchmarks are built for vLLM. The **Benchmark…** button on the **Loaded** tab and the **Benchmark** command in the command palette work only with vLLM. The list on the **Benchmarks** tab also offers SGLang, NIM and TensorRT-LLM endpoints.
- Ollama and llama.cpp can't be tested, because they don't take this kind of request.
- Results stay with the data source they came from. Demo runs don't show with **Live** data.
- The test prompts are made of random tokens, not real text. Treat the numbers as a comparison between setups.

## If something goes wrong

- **"No vLLM or SGLang endpoint is ready."** No model that can be tested is **Ready**. Load one, and wait for **Ready**.
- **Run benchmark is dimmed.** You've selected no levels, or no model is ready.
- **(2 failed) in the Done column.** Some requests didn't finish. The prompt may be longer than the load's **Max context**, or memory may have run short at that level. Try a shorter profile or fewer requests at once.
- **"Pick between 1 and 1,000,000 prompt tokens and 1 to 32,768 output tokens."** Change the **Custom** numbers.
- **"It isn't ready yet."** The model stopped being **Ready** before the sweep began. See [Tell whether a model is working](help:tell-if-a-model-is-working).
- **"Couldn't save it"** The file couldn't be written to **Downloads**. Check that the folder exists and that your disk has space.

## Next steps

- [Models: Benchmarks tab](help:models-benchmarks)
- [Choose load settings](help:choose-load-settings), to try another setup and compare
- [Tokens and context](help:tokens-and-context)
