---
id: models-in-plain-words
title: "Models, engines, memory and context in plain words"
type: concept
area: models
summary: "What a model is, what runs it, what it needs from a Spark, and the few words you need to pick one."
keywords: ["what is a model", "ai", "llm", "beginner", "explain", "where do models come from", "where models come from"]
verified: null
related: [engines, memory-on-a-spark, tokens-and-context, find-a-model-that-fits]
---

# Models, engines, memory and context in plain words

## In short

A **model** is a trained AI: a very large set of learned numbers, stored as files, that lets a computer read and write text. An **engine** is the program that runs it. A running model sits in a Spark's memory, so memory decides what fits, and **context** decides how long a conversation can be.

## How it works

### A model is a set of files

A model's learned numbers are called its **weights**. They fill most of a model folder. The rest is small: a config file that describes the model, and a tokenizer that turns words into numbers.

Many model names include a size. The `27B` in `Qwen/Qwen3.8-27B-FP8` means 27 billion learned numbers, also called parameters. More parameters usually mean a more capable model that needs more memory.

### Hugging Face is where models live

[Hugging Face](https://huggingface.co) is a public website that stores models. Each model has its own **repository**, a page of files named like `org/name`. SparkBox searches it, works out what a model needs from your Sparks, and downloads the files to a node.

### An engine runs the model

Model files do nothing by themselves. An **engine** reads them, holds them in memory and answers requests. SparkBox starts each engine inside a **container**, a program running in its own sealed box. [Engines: vLLM, SGLang and Ollama](help:engines) compares them.

### Loading puts the model in memory

A **load** is one running start of a model. While it's loaded, the model holds a large part of the node's memory, even when nobody is asking it anything. When you're done, you **unload** it and the memory comes back. The files stay on disk.

A model on disk costs disk space. A loaded model costs memory. These are two separate costs, and you free them in two separate steps.

### Memory decides what fits

Each Spark has one pool of **unified memory**, shared by its CPU and its GPU. A load takes a fixed share of that pool when it starts. A model too big for one Spark can be split across several. [How memory works on a Spark](help:memory-on-a-spark) explains the numbers.

### Context decides how long a conversation can be

Models read and write in **tokens**, small pieces of text. A token is about three quarters of a word. **Context** is how many tokens a model can hold at once, counting your words and its reply. More context takes more memory. [Tokens and context](help:tokens-and-context) shows how much.

### An endpoint is how you reach a model

A running model answers at an address on your network. That address, with the model behind it, is an **endpoint**. Chat, the Server and other apps send requests to it. [Endpoints, ports and API keys](help:endpoints-and-apis) explains how apps connect.

## Why it matters in SparkBox

Everything you do with a model follows one path, and each step has a place in the **Models** window.

| Step | What happens | Where you do it |
|---|---|---|
| Prepare | You check that your nodes can run a model, before your first load | **Prepare cluster** tab |
| Find | You pick a model that fits your Sparks, or one you already have | **Library** and **Hugging Face** tabs |
| Download | Its files arrive on the node you choose | **Hugging Face** tab, then **Jobs** tab |
| Copy | The files go to other nodes, if you want them there | **Library** tab |
| Load | An engine starts the model | **Load** page |
| Check | You confirm that it answers | **Loaded** tab and the **Endpoints** window |
| Unload | The engine stops and the memory comes back | Its card in **Endpoints** |
| Delete | The files leave a node and the disk space comes back | **Library** tab |

To try the whole path with nothing at risk, follow [Rehearse running a model in Demo](help:your-first-model). To do it for real, follow [Run your first real model](help:run-your-first-real-model).

## Related

- [Rehearse running a model in Demo](help:your-first-model)
- [Run your first real model](help:run-your-first-real-model)
- [Common questions about models](help:model-questions)
- [Engines: vLLM, SGLang and Ollama](help:engines)
- [How memory works on a Spark](help:memory-on-a-spark)
- [Tokens and context](help:tokens-and-context)
- [Model formats and quantization](help:formats-and-quantization)
- [Find a model that fits](help:find-a-model-that-fits)
