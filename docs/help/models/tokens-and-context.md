---
id: tokens-and-context
title: "Tokens and context"
type: concept
area: models
summary: "Models read and write in tokens. Context is how many they can hold at once, and it costs memory."
keywords: ["token", "context", "max context", "how long", "tok/s"]
verified: null
related: [choose-load-settings, memory-on-a-spark]
---

# Tokens and context

## In short

Models read and write in **tokens**. A token is a small piece of text, about three quarters of a word. **Context** is how many tokens a model can hold at once, counting what you send and what it replies. Every token of context costs memory.

## How it works

### Tokens

A model never sees words. Its tokenizer cuts text into tokens first. Common short words are often one token, and long or rare words become several. As a rough guide, 1,000 tokens is about 750 words.

### Context

Each request carries a whole conversation: your first message, every reply and your newest message. Together with the reply it's about to write, all of that has to fit in the context.

When a conversation outgrows the context, the request fails or the app has to drop older messages. So **Max context** on the **Load** page is the longest conversation this load can hold.

A model also has its own limit, which comes with it. SparkBox shows it as **Model's max context** in the fit table. More memory can't take a model past that limit.

### Why context costs memory

As a model reads, it stores a little for every token in its **KV cache**. How much depends on the model's design:

| Model | KV cache per token | At 32,768 tokens | At the model's own maximum |
|---|---|---|---|
| `Qwen/Qwen3.8-27B-FP8` | 64 KiB | 2 GiB | 16 GiB at 262,144 tokens |
| `meta-llama/Llama-3.3-70B-Instruct` | 400 KiB | 12.5 GiB | 50 GiB at 131,072 tokens |

The KV cache has to hold every request that runs at the same time. Two full-length requests take twice the memory of one. That's what **Requests at once** on the **Load** page sets. An **fp8** KV cache takes half the memory per token, for a small cost in accuracy.

A model that's split across nodes may divide this memory between them. See [Running one model on several nodes](help:splitting-a-model).

### Speed

**Tokens per second** (**tok/s**) is how fast a model writes. It's the speed you notice as a reply streams in. A token is about three quarters of a word, so 8 tok/s is about 6 words a second.

**Time to first token** (**TTFT**) is how long you wait before the reply starts. The model has to read your whole conversation first, so a long conversation means a longer wait, even on a fast model.

The fit table's **Decode t/s** column is a ceiling for one reply, set by how fast the Spark's memory can move data. Real speed is lower. To measure it, [benchmark the model](help:benchmark-a-model). To make a model faster, see [A model is slow](help:troubleshooting#a-model-is-slow).

## Why it matters in SparkBox

You choose context when you load a model. A sensible way to choose:

- **For a chat**, 32,768 tokens is plenty for a long conversation. It's about 24,000 words.
- **For reading long documents**, you need room for the whole document as well as the reply.
- **If the model has a huge maximum**, set **Max context** lower. Leaving it empty uses the model's full maximum, and the memory plan counts all of it.

If the **Memory plan** is red, lowering **Max context** is usually the first thing to try. To set it, see [Max context](help:choose-load-settings#max-context).

## Related

- [Choose load settings](help:choose-load-settings)
- [How memory works on a Spark](help:memory-on-a-spark)
- [Find a model that fits](help:find-a-model-that-fits)
- [Benchmark a model](help:benchmark-a-model)
