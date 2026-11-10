---
id: memory-on-a-spark
title: "How memory works on a Spark"
type: concept
area: models
summary: "Each Spark has one pool of memory for CPU and GPU. Why that decides what fits, and why running out is dangerous."
keywords: ["unified memory", "gpu memory", "kv cache", "page cache", "out of memory", "oom"]
verified: null
related: [choose-load-settings, find-a-model-that-fits]
---

# How memory works on a Spark

## In short

Each Spark has one pool of **unified memory** that its CPU and GPU share. vLLM reserves memory when it starts, before requests arrive. If a Spark runs out, it can freeze instead of failing cleanly, so SparkBox estimates each node's use before anything starts.

## How it works

### One pool for everything

A desktop PC keeps separate memory for its CPU and its graphics card. A Spark doesn't. The system, your files in memory and the model all draw from the same pool. On `cell2`, that pool reads 121.7 GiB.

So a Spark has more room for a model than most graphics cards, but anything else using memory takes room from the model.

### What a load takes

When vLLM starts, its **GPU memory share** sets a memory budget. It allocates room for the model and KV cache before requests arrive. A share of 0.75 budgets 75% of the node's memory. On `cell2`, that's 91.3 GiB. The measured allocation can differ from that budget.

Three things fill the share:

- **Weights.** The model itself. On several nodes, each node holds its part.
- **Runtime.** Working memory for the engine, which SparkBox counts as 6 GiB, plus room for a vision encoder if the model can read images.
- **KV cache.** Everything left over, however little the model needs.

The budget can greatly exceed the model's weights. With a **Max context** of `32768`, the plan estimates that `Qwen/Qwen3-1.7B` needs 13.3 GiB. A share of 0.75 budgets 91.3 GiB of `cell1`, with an estimated 81.5 GiB for KV cache.

vLLM also uses host memory outside that budget. SparkBox allows about 8.8 GiB for it in the plan. This is an estimate, not a measurement of every engine image.

### How a load gets its share

SparkBox sizes the share to the model, so a small model doesn't shut everything else out. It works out the least share that holds three things on every chosen node: the weights, the runtime, and the KV cache for four full-length requests. It adds 10% and rounds up to the next 0.05. If you set **Requests at once**, it counts that many requests.

Here's what that gave on your idle nodes on November 1, 2026, with a **Max context** of `32768`:

| Model | Nodes | Share | What it holds |
|---|---|---|---|
| `Qwen/Qwen3-1.7B` | `cell1` | 0.25 | About 5 full-length requests |
| `Qwen/Qwen3.8-27B-FP8` | `cell1` | 0.45 | About 8 full-length requests |
| `google/gemma-3-27b-it` | `cell2`, `cell3` | 0.60 | About 5 full-length requests |
| `google/gemma-3-27b-it` | `cell2` | 0.75 | About 2 full-length requests |

The share stays between two limits:

- **At least 0.20**, about 24 GiB. The runtime figure is an estimate, and a small model has little room to spare. A lower share saves nothing, because a node holds one load at a time.
- **At most 0.75.** A model that needs more for four requests stays at 0.75, as `gemma-3-27b-it` does on one node.

SparkBox also keeps the share under the 90% line when the model still fits there. If it can't tell how big the KV cache is, or how long a request can be, the load stays at 0.75.

### The KV cache

The **KV cache** is the memory a model uses to keep track of the text so far. More tokens occupy more of that space. The reserved size limits how much **context** you can have. [Tokens and context](help:tokens-and-context) shows the numbers.

The cache percentage in **Endpoints** tells you how much of that reserved space requests occupy. It can return to zero while **GPU allocations** stays high: the engine keeps its reserved room for the next request. Unload the model to release that room.

### Comparing the memory readings

**GPU allocations** adds the GPU memory reported for each process. It is part of the node's unified memory, so don't add it to **Used** again. **Available** shows what Linux estimates can still be used without swapping.

In **Containers**, **Memory** comes from Linux's accounting for that container. **GPU memory** comes from the driver. Neither figure alone is the node's total memory use.

On November 7, 2026, a short `Qwen/Qwen3-0.6B` test on `cell1` used a 0.20 share and 4,096 context. GPU allocations stayed at about 23.2 GiB, container memory at 4.6 GiB and available node memory at 88.9 GiB. The engine reported a 21.22 GiB KV pool; its occupied share returned to zero after generation. Unload restored available memory to 116.6 GiB. These figures describe that load, not every model or image.

### The page cache

Linux keeps recently read files in memory, in case they're needed again. This is the **page cache**. It's useful, and Linux gives the memory back when a program asks for it.

vLLM's startup check doesn't see it that way. It counts the page cache as used, so it looks at **free** memory, not **available** memory:

- **Free** memory is empty.
- **Available** memory is free memory plus what Linux can give back, mostly the page cache.

So a node can have plenty of available memory, and vLLM can still refuse to start. Before a load, vLLM needs its share plus 4 GiB to be free. On `cell2` at 0.75, that's 95.3 GiB. At 0.25, it's 34.4 GiB, so a small model rarely needs more memory freed.

For a vLLM load, SparkBox reads these numbers in **Before it starts**, for each chosen node. If too little is free, it offers **Drop the page cache first**. That clears the page cache with sudo, which means running one command as root, the account that can change anything on the node. No data is lost, and Linux refills the cache as files are read again.

### Why running out is dangerous

Most computers slow down when memory runs short. A Spark that runs out of unified memory can freeze. It stops answering, SparkBox can't reach it, and it may need someone at the machine to restart it. [Recover a frozen node](help:recover-a-frozen-node) has the steps.

That's why SparkBox plans before it loads. Some nodes also run **earlyoom**, a small service that stops a process when memory runs very low. SparkBox shows whether it's running, but it doesn't rely on it.

### Alerts while a model runs

A large GPU reservation is expected. SparkBox excludes the larger of its running vLLM loads' recorded shares and the measured GPU allocation from the percentage alert. It does not count that memory as available. Older loads and other engines need a measured GPU allocation; without one, the percentage alert stays conservative.

**Available memory** warns below 4 GiB and turns critical below 2 GiB, after the condition lasts 60 seconds. **Memory pressure**, **Swap-in activity** and out-of-memory kills still raise alerts. Missing readings do not prove recovery. These thresholds have offline tests; they have not been tuned with a long model run on your cluster.

### The memory plan

The **Memory plan** on the **Load** page adds up where each node will end up: what's in use now, vLLM's own process, and the whole share.


Here's an example for `cell2`, with 5 GiB already in use, at a share of 0.75:

| Part | Size |
|---|---|
| In use now | 5.0 GiB |
| vLLM's own process | 8.8 GiB |
| The share | 91.3 GiB |
| Total | 105.1 GiB, or 86% of the node |

That's under the line, so it reads **Fits**. The same node at 0.80 reaches 91%, which is over the line. This is why a new load never starts above 0.75.

| Word on the bar | Color | What it means |
|---|---|---|
| **Fits** | Plain | The node stays under 90% |
| **Tight** | Amber | The node ends up from 90% to under 95%. It starts with little room left. Lower the settings, or go on only if someone can reach the node |
| **Won't start safely** | Red | The node would reach 95% or more |
| **Won't fit** | Red | The model needs more than the share gives it |
| **Won't start** | Red | The share is more than the node has to give right now |

## Why it matters in SparkBox

You meet these ideas in three places:

- **The fit table** on the **Hugging Face** tab shows what could fit. It uses a share of 0.8, the figure in NVIDIA's guide for several Sparks.
- **The Memory plan** on the **Load** page shows what will happen on your nodes right now, at the share on the page. When the two disagree, trust the Memory plan.
- **Memory alerts** distinguish an expected GPU reservation from low available memory, time spent waiting on memory, and sustained reads from swap. The bars still show total usage. See [Alert rules reference](help:alert-rules-reference#available-memory).

If the plan is red, lower **GPU memory share** or **Max context**. SparkBox often offers a button with a value that fits. **Override memory guardrail** exists, but it risks the freeze described above. Treat it as a last resort, and only when you can reach the node if it freezes.

## Related

- [Choose load settings](help:choose-load-settings)
- [Find a model that fits](help:find-a-model-that-fits)
- [Tokens and context](help:tokens-and-context)
- [Running one model on several nodes](help:splitting-a-model)
- [Fix a load that failed](help:fix-a-load-that-failed)

![How memory works on a Spark, using synthetic Demo data](img/public-memory-on-a-spark.jpg)
