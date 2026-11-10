---
id: your-first-model
title: "Rehearse running a model in Demo"
type: tutorial
area: start
summary: "Follow the whole path once, in Demo: find a model, download it, load it, chat with it, and unload it."
keywords: ["first model", "tutorial", "beginner", "start here", "rehearse", "rehearsal", "practice", "practice in demo", "try it in demo", "demo walkthrough", "nothing at risk"]
verified: null
related: [models-in-plain-words, download-a-model, load-a-model-on-one-node, connect-your-cluster, run-your-first-real-model]
---

# Rehearse running a model in Demo

## What you'll do

You'll follow the whole path once in **Demo**, where nothing touches your real nodes: find a model, download it, load it, chat with it and unload it. It takes about 10 minutes.

The search is real, because it goes to the Hugging Face website. The download, the load and the chat are simulated, and the screens work the same way as they do with **Live** data. At the end, a table shows what's different for real.

## Before you start

- **Use Demo data.** The top bar should read **Demo**. If it reads **Live**, see [Switch between Demo and Live](help:switch-between-demo-and-live).
- **Turn look-only mode off.** In look-only mode, buttons that change anything stay dimmed, even in Demo. If a toolbar shows **Look only**, see [Use look-only mode](help:use-look-only-mode).
- **This Mac needs internet access** for the search.

You don't need a password or a Hugging Face account for this.

## Step 1: Clear the cluster

Demo normally opens with a model already serving, so its nodes have no memory to spare. A scenario clears them for you. A **scenario** is a story the simulator plays.

1. At the bottom of the sidebar, click **Settings**. [Open Demo scenarios](open:settings?section=scenarios) The **Settings** window opens.
2. In the list on the left, click **Demo scenarios**.
3. Click **Your cluster at rest (Oct 29)**. It's first in the list, and it now reads **Active**.
4. Wait up to a minute. The top bar shows the label **Your cluster at rest (Oct 29)**.
5. Move the pointer over **4/4 online** in the top bar. The tooltip includes "No model is answering". Until the scenario settles, it may still name a model. If your window is wide, the bar itself also reads **no model serving**.

## Step 2: Find a model

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=hf) The **Models** window opens.
2. Click the **Hugging Face** tab. On the left you see **Recommended for GB10**.
3. Click `Qwen/Qwen3.8-27B-FP8`. After a moment the right side shows its facts and its fit table.
4. Read the line that starts **Downloads**. It says about 28.8 GiB, which is what a node has to store.
5. Read the first row of the table, where **Nodes** is `1`. **Fits now** reads **Yes**, so one Spark can run this model. If you can't see **Fits now**, drag the divider left or scroll the table sideways.

You've done what you'd do before any real download: you've checked that the model fits.

## Step 3: Download it

1. In **Download node**, choose `cell1` for this tutorial. Click **Download to cell1**, the hard drive with a down arrow. The toolbar here often shows icons without names, so move the pointer over an icon to read its name. The **Unlock Admin mode?** dialog opens, and it says that with Demo data it needs no password.
2. Click **Unlock…**. In **Choose a download tool on cell1**, choose the existing `hf` tool, then click **Continue**. This choice is simulated and installs nothing.
3. The final dialog asks **Download Qwen/Qwen3.8-27B-FP8 to cell1?** and repeats the size. Click **Download**. The window moves to the **Jobs** tab, with a new row for your download.
4. Watch the **State** and **Progress** columns. **State** goes from **downloading** to **verifying**, then **done**. This takes about 30 seconds.

**Admin mode** is what lets SparkBox change things. It locks again by itself after a time. With **Live** data, it asks for Touch ID or your Mac password.

## Step 4: Load it

1. Click the **Library** tab. `Qwen/Qwen3.8-27B-FP8` is listed, with **✓ verified** under **cell1**.
2. Click its row, then click **Load…** in the toolbar. The **Load** page opens, with the heading **Load a model with vLLM**.
3. Under **Nodes**, check that **cell1** is selected and no other node is.
4. Find **Memory plan**. The bar for `cell1` reads **Fits**, and the line under its name says how full the node will be.
5. Scroll to **Max context**. It reads `32768`, which the model's recipe filled in. If it's empty, type `32768`. This is how long a conversation the model can hold, in tokens. A **token** is a small piece of text, about three quarters of a word.
6. Scroll down to **Before it starts**. In Demo it reads "Available with Live data". With **Live** data, this group reads the node itself.
7. Scroll back to the top. Click **Load on 1 node**. The window moves to the **Loaded** tab, and a message reads "Loading qwen3.8-27b-fp8".
8. Find your model's row, and watch the **State** column. It reads **Loading** with a percentage, then **Ready**. This takes about a minute.

The other rows are left over from the scenario: **Not found**, **Idle** and one red **Failed**. They're part of the simulated history, and you didn't cause them.

**Ready** means more than "started". With **Live** data, SparkBox only says **Ready** after the model has answered a test question.

## Step 5: Chat with it

1. On the row for your model, click **Open in Endpoints**. The **Endpoints** window opens, with a card for your model.
2. On that card, click **Chat with this**. The **Chat** window opens.
3. Click the box at the bottom, type `What is a GPU?`, and press <kbd>Return</kbd>. A reply streams in.

In Demo the reply is simulated, and it says so. With **Live** data, the real model writes it.

## Step 6: Unload it

A loaded model holds its memory until you unload it.

1. Go back to the **Endpoints** window and find your model's card.
2. Click **Unload…**. A dialog asks **Unload qwen3.8-27b-fp8?** and says what will stop.
3. Click **Unload**. Its card leaves the window.

The model's files stay in the **Library**. You can load it again at any time.

## Step 7: Tidy up

1. Go back to **Settings › Demo scenarios**.
2. Click **Back to everyday serving**. Demo returns to its normal state. Everyday serving has its own copy of this model on `cell1`, so it stays in the **Library**.
3. To lock Admin mode now, click the padlock in the top bar. Otherwise it locks by itself.
4. If the top bar read **Live** when you started, [switch back to Live](help:switch-between-demo-and-live). The top bar reads **Live** again.

## What's different for real

| In this tutorial | With Live data |
|---|---|
| Nothing changes on a node | Everything happens on your real nodes |
| No password | Touch ID or your Mac password to unlock Admin mode |
| The download takes about 30 seconds | It depends on your internet speed and the model's size. Keep SparkBox open and [this Mac awake](help:keep-this-mac-awake) until it ends |
| **Before it starts** reads nothing | It reads the node's memory, the image, the port and more, and it can stop the load |
| The load takes about a minute | The first time a model starts, the engine prepares its code for the GB10 chip and keeps the result on the node. That can take much longer. See [Load a model on one node](help:load-a-model-on-one-node) |
| The chat reply is simulated | A real reply, from the real model |
| Unload finishes at once | SparkBox stops the containers and checks that the memory, the port and `/dev/shm` came back |

To do all of this on your real nodes, first follow [Connect your cluster](help:connect-your-cluster). Then follow [Run your first real model](help:run-your-first-real-model). It checks your nodes, loads a model you already have on `cell2`, confirms that it answers, chats with it and unloads it. It needs no download.

## What you learned

- A model's path is find, download, load, check and unload.
- The fit table tells you whether a model will fit before you spend any time on it.
- A download goes to the node you choose, and it's a job you can watch.
- The **Memory plan** shows where a node will end up before anything starts.
- **Ready** means the model answers, and unloading gives its memory back while its files stay on disk.

## Next steps

- [Connect your cluster](help:connect-your-cluster), to move on to your real nodes
- [Run your first real model](help:run-your-first-real-model)
- [Find a model that fits](help:find-a-model-that-fits)
- [Models, engines, memory and context in plain words](help:models-in-plain-words)
