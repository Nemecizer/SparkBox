---
id: splitting-a-model
title: "Running one model on several nodes"
type: concept
area: models
summary: "How a model too big for one Spark is split across nodes, what tensor and pipeline parallel mean, and how nodes start."
keywords: ["tensor parallel", "pipeline parallel", "multi-node", "expert parallel", "head node", "worker", "rank", "attention heads", "kv heads", "across the cluster", "all nodes", "split a model", "why does tensor parallel 3 fail"]
verified: null
related: [load-a-model-on-several-nodes, fabric-and-rdma]
---

# Running one model on several nodes

## In short

A model too big for one Spark can be split across two, three or four nodes, with one GPU on each. **Tensor parallel** divides the work inside each layer, and **pipeline parallel** gives each node its own run of layers. The two numbers multiplied must equal the number of nodes.

## How it works

### Layers and attention heads

A model is a stack of layers. A token passes through every layer in order. Inside each layer, the work divides into parallel parts called **attention heads**. They have nothing to do with the head node, described later. Those two facts give you two ways to split a model.

![Tensor parallel gives every node a slice of every layer. Pipeline parallel gives each node its own run of layers](img/diagram-tensor-pipeline.svg)

### Tensor parallel

With tensor parallel, every node holds a slice of every layer. All the nodes work on each token together, and they exchange results over the fabric, the fast 200G network between your nodes.

- **It can be faster for one reply.** More nodes share each token's work, so the **Decode t/s** column in the fit table rises.
- **It's heavy on the fabric.** The nodes talk to each other many times for every token.
- **It needs an even split.** The attention heads must divide evenly by the number of nodes.

The last point decides what's possible. `gemma-3-27b-it` has 32 attention heads, which divide by 2 and 4 but not by 3. So tensor parallel works on 2 or 4 nodes, and not on 3. `GLM-5.3-Flash` has 64 attention heads, and the same is true of it. Many models have a head count like these, a power of 2, and a power of 2 never divides by 3. That's why tensor parallel 3 fails so often.

Some models share one set of memory between several attention heads. Those sets are called **KV heads**. Their number must divide by the number of nodes too, or the number of nodes must be a multiple of it. `gemma-3-27b-it` has 16 KV heads, so 2 and 4 pass.

SparkBox checks both rules against the model's own config file. If a split can't work, the **Load** page says why and the button stays off, so you never wait on a load that can't start.

### Pipeline parallel

With pipeline parallel, each node holds its own run of layers. A token passes from node to node, like an assembly line.

- **It fits more.** Each node holds fewer weights, the same as with tensor parallel.
- **It's lighter on the fabric.** Nodes pass results along the line, once per stage.
- **One reply gets no faster.** A token still has to go through every layer. The fit table says **· no speedup** for this reason.
- **It works when attention heads don't divide evenly.** The fit table shows this as **PP 3 only**.
- **It needs enough layers.** The number of nodes can't be more than the model's layers, which is rarely a limit.

### Mixing the two

The **Tensor × pipeline parallel** field on the **Load** page takes both numbers. Their product must equal the number of nodes, one GPU per Spark.

| Nodes | Choices |
|---|---|
| 2 | 2 × 1 or 1 × 2 |
| 3 | 3 × 1 or 1 × 3 |
| 4 | 4 × 1, 1 × 4 or 2 × 2 |

The **Split** menu on the **Load** page lists these, and picks the first one this model can take. It marks 2 × 2 as advanced. A model's recipe can list fewer. The fit table shows only the plain tensor or pipeline choice for each row, so it doesn't show 2 × 2.

### Expert parallel

Some models are a **mixture of experts**, shown as **MoE**. They hold many small sub-networks, called experts, and use only a few for each token. For example, a model may have 288 experts and use 8 for each token. **Expert parallel** spreads the experts across nodes. It applies only to these models.

The speed of a mixture of experts depends on which experts each token uses, so the fit table's speed column reads **varies**.

### How memory divides

Weights divide evenly between nodes. The KV cache depends on the model. Tensor parallel usually divides it too, but a model with MLA attention keeps one cache that's copied to every node. More nodes then free room by shrinking the weights, not by sharing the cache. The fit table does this math for you.

### Sparse MLA on a GB10

**Sparse MLA** is one kind of attention. A **GPU kernel** is a small piece of code that does this work on the GPU, separate from the node's operating-system kernel. SparkBox blocks automatic or bfloat16 KV for sparse MLA on GB10. Choosing fp8 alone does not establish a supported kernel for the model and image. A fit estimate cannot prove compatibility.

`GLM-5.3-Flash` is one of these models, but fp8 isn't enough for it. SparkBox refuses to load it at any **KV cache type**. See [What's coming](help:whats-coming).

### How the nodes start together

Several nodes have to start together and find each other. vLLM does this by itself, with the nodes meeting over the fabric.

Each node in a load has a number, its **rank**:

- The first selected node in your node list is the **head node**, which answers requests. It's rank 0, and it's the node apps talk to.
- The other nodes are **workers**. They're ranks 1, 2 and 3. They have no API, and they do their part of the work for the head node.

A rank belongs to the load, not to the node. Rank 1 of one load can be a different node from rank 1 of the next. SparkBox reads each node's fabric address from the node itself, so renaming a node doesn't change a load.

SparkBox starts the workers first, all at once, and they wait. The head node starts last, then every rank meets on the head node's fabric address on port `29501`. All of their traffic stays on the fabric. If any worker can't start, the head node never starts, and SparkBox removes what it did start.

Because the parts depend on each other, one worker that stops takes the model down. SparkBox restarts a several-node load in order: the head node stops, then the workers. Then it checks the nodes again, the workers start, and the head node starts last.

SGLang does this its own way. The head node is rank 0, the first in the group, and the other nodes meet it on its fabric address on port `5000`.

## Why it matters in SparkBox

Use the fewest nodes that serve a model well. A model that fits on one Spark has no network between its parts and is the simplest to run.

A small `Qwen/Qwen3-0.6B` load has passed on `cell1` and `cell2` with tensor parallel 2, including Chat and traffic over both RDMA fabric halves. It does not prove a speedup, larger models or other splits. Unload returned memory but needed a separate guarded cleanup for one small shared-memory file. Your earlier real 4-node load, `GLM-5.3-Flash`, came from a launch script on the nodes, and it failed. For what to pick and what's known so far, read [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes).

## Related

- [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes)
- [Copy a model to other nodes](help:copy-a-model-to-other-nodes)
- [Find a model that fits](help:find-a-model-that-fits)
- [How memory works on a Spark](help:memory-on-a-spark)
- [The fabric and RDMA](help:fabric-and-rdma)
