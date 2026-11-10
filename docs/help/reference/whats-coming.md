---
id: whats-coming
title: What's coming
type: reference
area: reference
summary: "What SparkBox can't do yet and is planned: routing between replicas, choosing a download node, app launchers."
keywords: [planned, roadmap, not built, coming soon, future, claude code launcher, codex, resume, rolling updates, tensorrt, llama.cpp, nim, replicas, image pull, glm]
verified: null
related: [whats-proven, settings-general, load-a-model-on-several-nodes]
---

# What's coming

This page lists what SparkBox can't do yet and is planned. None of it is in the app today, so no How-To covers it. It gives no dates.

To see the planned launchers in the sidebar, turn on **Show upcoming features** in [Settings: General](help:settings-general). They appear under **Coming soon**.

## Models

| What's coming | What it will do | What happens today |
|---|---|---|
| Routing requests between replicas | Spread requests between independent copies of one model | The Load page groups independent single-node or TP2 replicas. Each has its own endpoint and key. Spreading requests between them remains future work. See [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes) |
| `GLM-5.3-Flash` | Load the FP4-Local version of this model with its own image | Its recipe can't load it. It reads "Needs the FP4-Local checkpoint and its image, see What's coming". See [Use a recipe for a load](help:use-a-recipe-for-a-load) |
| Chain recovery after a node reboot | Review and resume an interrupted chain as one plan | **Then copy to** runs an approved download → verify → copy chain. Reopening follows existing jobs. After node reboot, cancel remaining members, recover the source and copy unfinished targets separately |
| Downloads proven to survive this Mac | Keep going while this Mac sleeps or SparkBox is closed, proven on your cluster, with a **Resume** button | A download goes to one node. It hasn't been proven to survive the Mac. **Resume download…** in **Jobs** reuses a stopped download's saved revision and files; this recovery has passed offline tests only. Older jobs need **Download again** from **Hugging Face**, which can choose a newer revision |
| Delete that frees the Hugging Face cache | Free a cached model's space from **Delete…** | **Delete…** removes the links and says the space stays used. [Delete a model from a node](help:delete-a-model#free-space-from-the-hugging-face-cache) shows how to free it |
| Copying from the Hugging Face cache | Copy a cached model to other nodes | A model in the cache can't be copied |
| Saved model layouts | Switch between sets of loads in one action | You load and unload each model yourself |

## Coding apps

| What's coming | What it will do | What happens today |
|---|---|---|
| Launch Claude Code | Open Claude Code in a project folder with one click, using a model on your cluster | There's no launcher. You can point an app at the **Server** yourself. See [Connect an app to SparkBox](help:connect-an-app) |
| Launch Codex | The same for Codex | The same |

## Updates

| What's coming | What it will do | What happens today |
|---|---|---|
| GPU-stack updates across the cluster | Move the kernel, NVIDIA driver, Docker and related packages together on every node | SparkBox refuses to move them on one node. See [Pin the GPU stack](help:pin-the-gpu-stack) |
| Firmware updates | Install firmware with you at the node | SparkBox shows whether firmware is waiting. It doesn't install it |

## More engines

An **engine** is the program that serves a model. These launchers open a page that says what they will do.

| Launcher | What it will do |
|---|---|
| **TensorRT-LLM** | Serve a model with TensorRT-LLM's `trtllm-serve`, on one node or several, with live progress for its build step and the commands shown per node before launch |
| **llama.cpp** | Run `llama-server` for GGUF models on one node, or spread layers across nodes. You pick a GGUF, the context size, the GPU layers and the port |
| **NIM** | Run NVIDIA NIM containers. Its NGC API key stays in the Keychain, and you pick an image and a node |

**SGLang** under **Coming soon** is ready now. It opens **Models › Load** with SGLang picked as the engine.

## Also planned

- Changing a node's network settings with an automatic way back if the connection drops
- A disk cleanup assistant
- Changing the shared cluster password on every node at once
- Images and tools in **Chat**
- The **Server** on your LAN, for other computers
- SparkBox updating itself

## How firm these plans are

Everything on this page is a plan, and the order can change. To see what's been proven so far, read [What's proven on your cluster](help:whats-proven).
