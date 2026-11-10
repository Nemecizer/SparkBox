---
id: models-benchmarks
title: "Models: Benchmarks tab"
type: feature
area: models
summary: "Run a speed sweep against a ready vLLM model and compare runs."
keywords: ["benchmark", "p50", "p95", "sweep", "csv"]
verified: false
related: [benchmark-a-model]
---

# Models: Benchmarks tab

## What it's for

The **Benchmarks** tab measures how fast a loaded model really is. It runs a **sweep**, sending test requests of an exact length at several levels of load, and saves the results so you can compare setups. The fit table tells you what might be possible. This tab tells you what you got.

## Where to find it

- **Sidebar:** under **Workloads**, click **Models**, then click the **Benchmarks** tab.
- **Loaded tab:** click **Benchmark…**.
- **Command palette:** press <kbd>⌘K</kbd> and type `benchmark`.
- **Link:** [Open Benchmarks](open:models?tab=bench)


## What's on the screen

The left side sets up a sweep and lists past runs. The right side shows the results.

### Run a sweep

- **Endpoint** picks the model. It lists each **Ready** endpoint that can be tested, such as "llama-3.3-70b — cell1 :8000 (external)". If there's none, it reads "No vLLM or SGLang endpoint is ready."
- **Profile** sets the length of each request: **Chat** (512 → 256 tokens), **RAG** (4,096 → 256), **Long** (32,768 → 512) or **Custom**. RAG means feeding a model documents to answer from.
- **Tokens in → out** appears with **Custom**. Set your own lengths: up to 1,000,000 tokens in, and up to 32,768 out.
- **Requests at once** sets the levels: `1`, `2`, `4`, `8`, `16` and `32`. Each selected level runs twice that many requests, and at least 4.
- **Run benchmark** starts the sweep. While it runs, it becomes **Stop**, with a bar and the current level.

The **About the sweep** tip shows the exact numbers for your choices. To keep lengths exact, SparkBox sends each prompt as token numbers, not as text, and tells the engine not to end a reply early. Each request also has a different prompt, so prefix caching can't shorten the test.

### Runs

Every finished sweep is saved here, newest first, with its model, split and lengths, such as "llama-3.3-70b PP 3 · 512→256". Click a run to show its table. Select up to 4 to compare them in the chart. If no run is selected, the right side reads **No run picked**.

### Chart

Each line is one run, and each point on it is one level. Across is **Output tokens per second, all requests**. Up is **ms per output token (p50)**. Down and to the right is better. Move the pointer over a point for its numbers.

### Results table

A heading names the run and its nodes. **Save CSV** and **Save JSON** write the run to your **Downloads** folder.

| Column | What it means |
|---|---|
| **At once** | How many requests ran together |
| **Done** | How many finished, with any failures in red |
| **Req/s** | Requests finished each second |
| **Output tok/s** | Tokens written each second, all requests together |
| **First token** | Time to first token (TTFT) |
| **Per token** | Time for each token after the first |
| **Between tokens** | Every gap between tokens as they arrive |

The last three columns show p50, p90 and p99. Half of requests were at or under p50. p99 is close to the worst. [Benchmark a model](help:benchmark-a-model) explains how to read them.

## What it changes

- **Reading saved runs** changes nothing, and it works in look-only mode.
- **Run benchmark** needs Admin mode, because it keeps the model busy for minutes. It sends requests and changes no settings. Other apps get slower replies while it runs. **Stop** ends it, and doesn't need Admin mode.
- **Save CSV** and **Save JSON** write a file on this Mac.

Runs are kept on this Mac, separately for **Demo** and **Live** data. You can't delete a run from this tab.

## Related How-Tos

- [Benchmark a model](help:benchmark-a-model)
- [Choose load settings](help:choose-load-settings)
