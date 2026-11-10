---
id: ollama
title: Ollama
type: feature
area: models
summary: "Ollama's models on a node: what's installed, what's in memory, Pull and Delete."
keywords: [ollama, pull, in memory, delete model, unloads, gguf, quantization]
verified: false
related: [use-ollama, engines, endpoints]
---

# Ollama

## What it's for

**Ollama** is an engine: a program that serves models. It's the simplest of the three engines on a Spark. It keeps its own small library of models, and reads one into memory when a request asks for it. It lets the model go after 5 idle minutes, by default.

The **Ollama** window manages one Ollama server on one node. It shows what's installed and what's in memory. It lets you pull a new model and delete one you no longer need.

To pull means to fetch a model from [ollama.com](https://ollama.com/library) to the node. SparkBox doesn't install Ollama itself. This window works with an Ollama that's already running.


## Where to find it

- **Sidebar:** under **Workloads**, click **Ollama**.
- **Endpoints window:** on an Ollama card, click **Models…**. The window opens on that server.
- **Command palette:** press <kbd>⌘K</kbd>, type `ollama`, and press <kbd>Return</kbd>.
- **Link:** [Open Ollama](open:ollama)

If no node runs Ollama, the window says "No Ollama server found". SparkBox looks in every node's containers and services, and a new server appears here within a few seconds.

## What's on the screen

### Toolbar

| Item | What it does |
|---|---|
| Server list | Picks which Ollama to manage, such as **cell3 :11434**. It shows only when there are two or more |
| **Refresh** | Reads the list now. It also refreshes by itself every 10 seconds |
| **Chat with this** | Starts a new chat with the model you selected. It's off in three cases: no model is selected, the model is an embedding model, or this Mac can't use this Ollama |
| **Delete…** | Deletes the model you selected from the node's disk |
| Status line | Where you are and what's there, such as "cell3 :11434 · Ollama 0.21.0 · 1 model, 8.9 GiB · nothing in memory" |

On a narrow window, **Delete…** moves into the **More** menu.

### Pull

The row under the toolbar fetches a new model.

| Item | What it does |
|---|---|
| "Model to pull, e.g. qwen3:8b" | Type a name in this field. The part after the colon picks a size or variant. A list offers common names as you type |
| **Pull** | Starts the download on the node. If Admin mode is locked, the **Unlock Admin mode?** dialog opens |

While a pull runs, a row under the field shows the task, a progress bar and the amount fetched. The same task is in **Tasks**, where you can cancel it.

### Model table

| Column | What it means |
|---|---|
| **Model** | The model's name and tag. Use this name in the API |
| **In memory** | How much memory it's using now, and when it will unload, such as "13.6 GiB unloads in 4 min". A dash means it's only on disk |
| **Parameters** | How many learned numbers the model has, such as "8.0B" for 8 billion. A bigger number usually means a more capable model that needs more memory |
| **Quantization** | How tightly the model is packed, such as `Q4_K_M`. [Model formats and quantization](help:formats-and-quantization) explains this |
| **Family** | The model family it belongs to, such as `gemma4` |
| **On disk** | How much space it takes on the node |
| **Pulled** | When it was fetched |

Models in memory sort to the top. Click a heading to sort by that column. Double-click a row to start a chat, if this Mac can use this Ollama.

Some models, such as `nomic-embed-text`, turn text into numbers for search and can't chat. SparkBox doesn't offer **Chat with this** for those.

### What Idle means

When nothing is in memory, the status line ends with "nothing in memory". The card in **Endpoints** says **Idle** and "Nothing in memory". This is Ollama at rest, not a fault, and it raises no alert.

The next request reads the model back into memory, so the first reply takes longer than the ones after it.

## What it changes

Viewing this window changes nothing. **Refresh** only reads. Everything here reaches the node over SSH, the secure connection SparkBox uses for every node. So it works even when Ollama listens on that node only.

| Action | What it changes | What it needs | How to undo it |
|---|---|---|---|
| **Pull** | Fetches the model to the node's disk | Admin mode, and an internet connection on the node | Delete the model |
| **Delete…** | Removes the model from the node's disk. Apps asking for it get an error until it's pulled again | Admin mode | Pull it again |
| **Chat with this** | Starts a chat. Ollama may read the model into memory | Nothing, unless look-only mode is on | It unloads by itself |

In look-only mode, **Pull** and **Delete…** are off. A pull that's already running keeps going, because Ollama does the download on the node.

## Related How-Tos

- [Use Ollama](help:use-ollama)
- [Check disk space on a node](help:check-disk-space)
- [Chat with a cluster model](help:chat-with-a-cluster-model)

![Ollama, using synthetic Demo data](img/public-ollama.jpg)
