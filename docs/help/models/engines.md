---
id: engines
title: "Engines: vLLM, SGLang and Ollama"
type: concept
area: models
summary: "An engine is the program that runs a model. How SparkBox's three engines differ and which to choose."
keywords: ["vllm", "sglang", "ollama", "engine", "which one"]
verified: null
related: [load-a-model-on-one-node, use-ollama]
---

# Engines: vLLM, SGLang and Ollama

## In short

An **engine** is the program that runs a model and answers requests. SparkBox starts two engines from the **Load** page, vLLM and SGLang, and it works with Ollama through its own window. Start with vLLM: it's the default, and it has the most checks in SparkBox.

## How it works

Every engine does the same basic job. It reads a model into memory, then waits for requests on a **port**, a numbered door on a computer that a program answers on. All three answer in the OpenAI style, the common format apps use to talk to a model.

### vLLM

vLLM is SparkBox's main engine. SparkBox starts it in a **container**, a program running in its own sealed box. An **image** is the recipe a container starts from, and Docker is the program that runs containers. A load runs an image already on each node. On several nodes, vLLM links them by itself over the fabric.

Only vLLM has all of these:

- The **Recipe** row and the **Before it starts** check on the **Load** page.
- A new API key for each load. An API key is a password that apps send with each request.
- The **Expert: raw vLLM args** field.

The image SparkBox starts first is `vllm/vllm-openai:glm53-flash-arm64-cu130`. If an image gives garbage answers on a GB10, SparkBox tells you and names a fallback: `nvcr.io/nvidia/vllm:26.05-py3`. [Fix a load that failed](help:fix-a-load-that-failed) shows what to do.

### SGLang

SGLang is a second engine that serves the same kind of API. You pick it with the **Engine** switch on the **Load** page. It uses the same model folders as vLLM, and its image comes from **Settings › Models**. The first time a node needs that image, SparkBox fetches it. The default is `lmsysorg/sglang:spark`.

SGLang differs from vLLM in three ways that matter:

- **No API key.** SGLang takes a key only on its command line, where anyone on the node could read it. So SparkBox's SGLang loads have none. Anything on your network can use the model, unless you choose **Fabric only**.
- **Tensor parallel only.** On several nodes, SGLang splits the model by tensor parallel across every node you pick.
- **Fewer checks.** It has no **Recipe**, no **Before it starts** group and no **Expert: raw vLLM args**. Its **GPU memory share** isn't sized to the model: it starts at `0.75`.

### Ollama

Ollama keeps its own store of models and fetches them by name, such as `gemma4:latest`. It uses a smaller file format called GGUF. SparkBox doesn't load Ollama models from the **Load** page, and it can't copy them between nodes. You pull, list and delete them in the **Ollama** window. The **Library** tab shows them in a read-only list. See [Use Ollama](help:use-ollama).

### Other engines SparkBox finds

SparkBox also notices engines it didn't start, such as llama.cpp, TensorRT-LLM and NIM. They appear in the **Endpoints** window and on the **Loaded** tab, marked **External**. SparkBox can't start these for you.

### Side by side

| | vLLM | SGLang | Ollama |
|---|---|---|---|
| Where you start it | **Load** page | **Load** page, **Engine** set to **SGLang** | It runs by itself on a node |
| Model files | A folder in your **Library** | A folder in your **Library** | Its own store, pulled by name |
| Several nodes | Yes, tensor or pipeline parallel | Yes, tensor parallel only | No |
| API key | A new key for each load, on by default | None | Not set by SparkBox |
| Check before load | **Before it starts**, on any number of nodes | Memory plan only | None |
| **Keep running** | Yes | Yes | Not set by SparkBox |

## Why it matters in SparkBox

The **Engine** switch is the first choice on the **Load** page. Leave it on **vLLM** unless you have a reason to change it. A reason might be a model card that names SGLang, or a model that won't start on vLLM.

Choose Ollama when you want a small model running quickly, and you don't need several nodes.

SparkBox's own vLLM and SGLang loads have run only on test systems so far. See [What's proven on your cluster](help:whats-proven).

## Related

- [Load a model on one node](help:load-a-model-on-one-node)
- [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes)
- [Choose load settings](help:choose-load-settings)
- [Use Ollama](help:use-ollama)
- [Model formats and quantization](help:formats-and-quantization)
