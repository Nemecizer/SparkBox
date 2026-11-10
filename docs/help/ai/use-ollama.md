---
id: use-ollama
title: Use Ollama
type: howto
area: models
summary: Pull a model with Ollama, chat with it if your Mac can reach it, and delete it when you're done.
keywords: [ollama, pull, gguf, small model, idle, reachable, 127.0.0.1, gemma]
verified: false
related: [ollama, engines, chat, endpoints-and-apis]
---

# Use Ollama

To try a smaller model without a full load, pull it with Ollama, chat with it if this Mac can reach it, then delete it.

## Before you start

- You need a node with Ollama running. **Ollama** is an [engine](help:engines): a program that serves models. On your cluster it runs on `cell1` and `cell3`. SparkBox doesn't install it.
- Be ready to unlock Admin mode, with look-only mode off. Pull and Delete change a node.
- A pull uses the node's disk. `llama3.2:3b` takes about 1.9 GiB and `qwen3:32b` about 18.8 GiB. [Check disk space on a node](help:check-disk-space) if you aren't sure there's room.
- The node needs the internet, because the model comes straight from ollama.com to the node.
- Keep SparkBox open until the pull ends. How long it takes depends on the model's size and your connection.

> [!NOTE]
> **Chat depends on where Ollama listens.** On your cluster, Ollama on `cell1` and `cell3` listens on the node only, so this Mac can't reach it. You can still pull, list and delete models there, but you can't chat with them in SparkBox. With Demo data, the simulated Ollama on `gx10-0000` does answer this Mac, so Chat works there. Step 3 tells you which case you have.

## Steps

### Find your Ollama

1. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) The **Endpoints** window opens, with one card for each model server.
2. Find a card marked **Ollama** at its top left, such as **Ollama on cell3**. To hide the others, click **Ollama** at the right of the toolbar.
3. Read the card. If it says "Reachable from cell3 only (127.0.0.1:11434). Chat and the API server on this Mac can't use it.", Ollama answers on that node only. Skip the chat stage below.
4. On the card, click **Models…**. The **Ollama** window opens. The line at the right of its toolbar names the node and says what's there, such as "cell3 :11434 · Ollama 0.21.0 · 1 model, 8.9 GiB · nothing in memory".

Next time, you can go straight there: in the sidebar, under **Workloads**, click **Ollama**. With two or more servers, pick one in the list at the left of the toolbar.

### Pull a model

1. Click the field that says "Model to pull, e.g. qwen3:8b".
2. Type a model's name, such as `llama3.2:3b`. A list offers common names as you type. [Ollama's library](https://ollama.com/library) has the rest. The part after the colon picks a size or variant.
3. Click **Pull**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A row with a progress bar appears under the field.
4. Wait for the bar to fill. It shows how much has arrived, such as "1.2 of 2.0 GB". You can use other windows meanwhile. To stop the pull, open **Tasks** and cancel it there.
5. Look for your model in the table. It appears when the pull ends, and **Tasks** says "llama3.2:3b is ready on cell3".

### Chat with it

Follow this stage only if the card had no "Reachable from" line.

1. In the table, click the model's row.
2. In the toolbar, click **Chat with this**. **Chat** opens on a new chat with that model.
3. Click the box at the bottom, type a question, and press <kbd>Return</kbd>. The first reply takes longer, because Ollama has to read the model into memory.
4. Go back to the **Ollama** window. The model's **In memory** column now shows its size, and when it will unload.

### Delete a model

> [!WARNING]
> **Delete removes the model from the node.** Any app that asks for it gets an error. To undo this, pull it again, which downloads the whole model.

1. In the table, click the model's row.
2. In the toolbar, click **Delete…**. If you don't see it, click **More** first. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A dialog says how much space this frees, such as "Frees 1.9 GiB on cell3."
3. Click **Delete**. A message says "Deleted llama3.2:3b", and the row goes.

## What you should see

- After a pull, the model has a row with its **On disk** size and its **Pulled** date.
- While a model is in memory, its card in **Endpoints** says **Ready**, and it has an **In memory** line.
- A few minutes after the last request, Ollama lets the model go. The card then says **Idle** and "Nothing in memory", and the **Ollama** window's status line ends with "nothing in memory". This is normal.

## Why it works

For **Pull**, **Delete** and the model list, SparkBox connects to the node over SSH, the secure connection it uses for everything on a node. It then asks Ollama from inside the node, on `127.0.0.1`, so these work even when Ollama listens on the node only. The download itself goes from ollama.com to the node, not through this Mac. Chat sends requests from this Mac, so it needs an address this Mac can reach. [Endpoints, ports and API keys](help:endpoints-and-apis) explains why.

## Limits

- An Ollama that listens on its node only can't be used by Chat or the Server. SparkBox has no setting that changes where Ollama listens.
- To try a model on such a node, use a terminal there. [Open a terminal on a node](help:open-a-terminal), and run `ollama run llama3.2:3b`. Type `/bye` to leave.
- SparkBox can't choose how long Ollama keeps a model in memory.
- The speed numbers on an Ollama card count only the requests SparkBox sent or relayed.
- Ollama serves a model from one node. It doesn't split a model across nodes.

## If something goes wrong

- **"No Ollama server found".** No node is running Ollama, or the node is offline. SparkBox can't install it for you. Check that the sidebar shows the node's GPU use, such as **GPU 28%**.
- **"“My model” isn't an Ollama model name like qwen3:8b."** The name has a character Ollama doesn't allow, such as a space. Check it against [Ollama's library](https://ollama.com/library).
- **"Ollama couldn't pull llama9:3b: pull model manifest: file does not exist".** Ollama's library has no model by that name. Check the spelling and the part after the colon.
- **"The pull stopped (exit 7)."** The connection to Ollama broke. Check that the node is online, then pull the same name again. Ollama keeps what it already fetched.
- **"Stopped; the partial download stays in Ollama's cache."** Someone canceled the pull. Pull the same name to continue. To free the space, delete the model once it's complete.
- **"That Ollama server isn't running any more."** It stopped after you opened the window. Check its card in **Endpoints**.
- **Chat with this is dimmed.** Move the pointer over the button in **Endpoints** to see why:
  - "Reachable from cell3 only": this Mac can't reach this Ollama. See [Limits](#limits).
  - "Nothing in memory to chat with": this Ollama can be reached, but SparkBox offers it to Chat only while a model is in memory. Click **Copy curl** on its card, paste the command into the Terminal app on this Mac, and press <kbd>Return</kbd>. That one request loads the model. **Chat with this** turns on within about 2 minutes.
- **The first reply is slow.** Ollama is reading the model into memory. Later replies are faster until it unloads.

[Chat, Providers and Server messages](help:errors-ai) lists more messages.

## Next steps

- [Chat with a cluster model](help:chat-with-a-cluster-model)
- [Load a model on one node](help:load-a-model-on-one-node), to serve many requests from a model that stays in memory
- [Engines: vLLM, SGLang and Ollama](help:engines)
