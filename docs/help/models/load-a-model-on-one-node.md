---
id: load-a-model-on-one-node
title: "Load a model on one node"
type: howto
area: models
summary: "Start a model on a single Spark with vLLM or SGLang, read the memory plan, and wait for Ready."
keywords: ["load", "run a model", "run", "start", "start a model", "start model", "serve", "deploy", "sglang", "how do i run a model"]
verified: false
related: [choose-load-settings, tell-if-a-model-is-working, fix-a-load-that-failed, tokens-and-context, keep-this-mac-awake]
---

# Load a model on one node

To run a model, load it on one Spark from the **Load** page, then wait for **Ready**.

## Before you start

- If this is your first load, you've run [Prepare the cluster for models](help:prepare-the-cluster). To rehearse a first load safely, follow [Rehearse running a model in Demo](help:your-first-model). For one guided load on a real node, follow [Run your first real model](help:run-your-first-real-model).
- The model is in your **Library**, with a copy on the node you'll use. If it isn't, [download it](help:download-a-model), and [copy it](help:copy-a-model-to-other-nodes) if you want it on another node.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the load is simulated.
- No other SparkBox load is on that node, and it has no Ollama model in memory. A node takes one load at a time.
- It may take a while. The first time a model starts, the engine prepares its code for the GB10 chip and keeps the result on the node. For a model the size of `gemma-3-27b-it` with a **Max context** of `32768`, plan on up to 25 minutes. Nobody has timed this on your cluster yet. SparkBox gives up after 90 minutes.
- Later loads of the same model should be faster, because that work is already done.
- **Keep SparkBox open and this Mac awake until the model reads Ready.** SparkBox doesn't keep this Mac awake during a load. [Keep this Mac awake during long work](help:keep-this-mac-awake) shows how. After **Ready**, the model doesn't need this Mac. See [If this Mac sleeps or SparkBox closes](#if-this-mac-sleeps-or-sparkbox-closes).

## Steps

### Open the Load page

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=library) The **Models** window opens on the **Library** tab.
2. Click your model's row.
3. Click **Load…** in the toolbar. The **Load** page opens, with the heading **Load a model with vLLM**. Double-clicking the row does the same thing.

### Pick the node

4. Under **Nodes**, select the node you want and deselect the others. Each node says **has copy** or **no copy**, and what it's serving now.

Under **Model**, **Recipe** names the settings SparkBox picked for this model. It fills in **Max context** and other fields, so some may differ from the values below. A recipe's note says if it can't load this model. See [Use a recipe for a load](help:use-a-recipe-for-a-load).

### Set the limits

5. Under **Split and limits**, type a number in **Max context**. For a first load, `32768` is a good choice. This is how long a conversation the model can hold, in [tokens](help:glossary#token), small pieces of text about three quarters of a word long. If you leave it empty, the load uses the model's full maximum, which can take far more memory.
6. Leave **GPU memory share** as SparkBox set it. SparkBox sizes it to the model, so it changes when you change **Max context**. `0.25` means 25% of the node's memory, and the most a new load starts at is `0.75`. Leave **Requests at once** empty.
7. If you used **KV fp8** in the fit table, find **KV cache type** under **Capabilities** and click **fp8 (half the memory)**. The choice in the fit table doesn't carry over to this page. Otherwise, leave it at **Model's own**.

### Read the memory plan


> [!DANGER]
> **Above the dashed 90% line, a node can run out of memory and freeze, and then only someone at the machine can restart it.** The plan is an estimate, and other programs on the node can grow. A load with **Keep running** on starts again after that restart, and it can freeze the node again. Stay under the line unless someone can reach the node. While the node still answers, [unloading the model](help:unload-a-model) gives the memory back. If a node does freeze, follow [Recover a frozen node](help:recover-a-frozen-node).

8. Under **Memory plan**, read the bar. Its percentage is higher than the share in step 6, because it adds the engine's own process and what the node was already using. The dashed 90% line is the one that matters. The line under the bar compares what the model needs with what the share reserves. Read the word on the bar:
   - **Fits**: go on to step 10.
   - **Tight**: it starts with little room left. Alerts check available memory and pressure after loading. Click the button SparkBox offers, such as **Use 0.70 — fits and stays under 90%**, or lower the settings. Go on only if someone can reach the node.
   - **Won't fit**, **Won't start** or **Won't start safely**: don't load yet. Go to step 9.
9. If the plan is red, click the button SparkBox offers, if there is one. It sets a share or a context that fits. If there's none, lower **Max context**, or free the node's memory. Then read the bar again.

### Check access and the node

10. Under **Access**, leave **Listen on** at **LAN** and **API key** on. Apps on your network can then reach the model, and only with its key. Chat and the Server use the key for you. [Use a model from another computer](help:use-a-model-from-another-computer) shows how to give an app that key.
11. Also under **Access**, leave **Keep running** off for a model you're still trying. With it on, a load that crashes keeps restarting until you unload it. Once a model has worked and you want it available all the time, turn it on. Docker, the program that runs containers, then starts the model again after a crash or a node restart, with or without SparkBox open.

Leave every other field as it is. [Choose load settings](help:choose-load-settings) says when to change one.

12. Scroll to **Before it starts**. SparkBox has read the node. Look for three things: **What vLLM needs** ends "enough is free", **Model folder** reads **present on** the node, and **Port** reads **free on** the node. Only a red notice here stops a load. The **earlyoom** row and a cache folder marked "(missing)" never stop one. [Before it starts](help:models-load-page#before-it-starts) explains every row. If it says an earlier load left containers on the node, see [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind).
13. If you see **Drop the page cache first**, select it. vLLM can't start until that memory is free. The drop is safe: the node refills its cache as it's needed. See [the page cache](help:memory-on-a-spark#the-page-cache).

### Start it

14. Look at the top of the page. If a message appears beside the button, it says what's stopping the load. Fix that first. The table under **If something goes wrong** lists each message.
15. Click **Load on 1 node**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
16. If the plan is amber, a dialog asks **Load above the 90% memory alert line?** Click **Cancel** and change the settings, which is the safer choice. Click **Load** only if someone can reach the node.
17. The window moves to the **Loaded** tab, and a message reads "Loading …". Your model has a row there.

### Wait for Ready

18. Watch the **State** column. It reads **Loading**, with a percentage while the weights load. You can use the rest of SparkBox while you wait.
19. To see which stage it's at, click **Open in Endpoints**. The model's card names the stage, as in the table below.

On the **Loaded** tab, the **vLLM log** under the table shows what the engine is printing.

To stop a load before it's **Ready**, click **Unload…** on its card. Quitting SparkBox doesn't stop it: the engine goes on starting on the node, and keeps its memory, until you unload it.

| The card says | What's happening |
|---|---|
| "Checking the nodes…" or "Starting vLLM…" | SparkBox is checking the node and starting the container |
| "Loading weights 42%" | The engine is reading the weights into memory |
| "Compiling: FlashInfer kernels" | The engine is preparing its code for the GB10. The words after the colon name which code. A first start can spend a long time here |
| "Capturing CUDA graphs" | The engine is preparing its code for the GPU |
| "Up; checking it answers…" | The engine is up, and SparkBox is testing it |
| **Ready** | The model answered |

> [!NOTE]
> **A slow first start stays Loading.** After a long quiet spell, the line under the model's state adds "no new progress for 25 min (limit 90 min)". The load may still be working, and SparkBox keeps waiting up to the 90-minute limit. Before you unload it, read the **vLLM log**. If new lines are appearing, leave it alone.

## What you should see

The **State** column reads **Ready**. On its card in the [Endpoints](open:endpoints) window, you see a line such as:

"Ready: /health 200, gemma-3-27b-it listed, a test token in 0.4 s, 17+25 → 42 (first run of this recipe and image)"

The model is now in **Chat** and in the Server. To try it, click **Chat with this** on the card.

## Why it works

SparkBox writes the load's settings to launch files on the node, then starts the container from an image already there, by the image's local id. A small launcher reads those files and starts the engine with no shell in the path, so what runs is exactly what you saw on the page. The model folder is mounted read-only, which means the engine can read the files but can't change them. **Ready** means the engine reports healthy, lists your model and writes a test token. The first time a model runs on an image, SparkBox also asks "What is 17+25?", because a GB10, the NVIDIA chip in each Spark, can run a broken image that looks fine and answers nonsense.

## If this Mac sleeps or SparkBox closes

The model runs on the node, not on this Mac. SparkBox starts it, tests it and watches it. So the answer depends on how far the load has gone.

- **After Ready, nothing stops.** You can quit SparkBox, or let this Mac sleep. The model keeps serving until you unload it or the node restarts, and apps that use the node's address keep working. Chat and the Server run in SparkBox on this Mac, so they stop while it's closed or asleep. If you only close the window to the menu bar, the Server keeps running.
- **Before Ready, keep SparkBox open.** Only SparkBox can run the test that makes a model **Ready**. If SparkBox closes, the engine goes on starting on the node, and SparkBox picks up the load the next time it opens with **Live** data. Until then, the model isn't in Chat or the Server.
- **If this Mac sleeps before Ready,** SparkBox loses touch with the node, and picks the load up when it can reach the node again. Past the 90-minute limit, a node away for 10 minutes fails the load, even though the engine may still be starting. See [Fix a load that failed](help:fix-a-load-that-failed).
- **When Admin mode locks, the load keeps going.** Admin mode locks after 15 minutes idle by default, when the screen locks, and when this Mac sleeps. That doesn't stop a load that's already running. You unlock again only to make the next change.

## Use SGLang instead

SGLang is a second engine. To use it, set **Engine** to **SGLang** before step 4. The heading changes to **Load a model with SGLang**. The steps are the same, with these differences:

- **There's no recipe.** The **Recipe** row is for vLLM.
- **There's no API key.** SGLang takes a key only on its command line, where anyone on the node could read it, so SparkBox doesn't set one. The page warns: "Without a key, anything on your network can use this model." To limit who can reach it, set **Listen on** to **Fabric only**, though Chat and the Server can't reach it then.
- **There's no Before it starts group.** SparkBox doesn't read the node first, and it can't offer the page-cache drop.
- **The image comes from Settings.** The first time, SparkBox fetches it. See [Settings: Models](help:settings-models).
- **There's no Expert: raw vLLM args field.**
- **GPU memory share starts at `0.75`.** SparkBox sizes the share for vLLM only. SGLang works out its memory differently, so its share stays at the usual start until you type one.

## Limits

- One SparkBox load at a time on each node.
- A node runs vLLM from an image it already has. A one-node vLLM load doesn't fetch one.
- SGLang loads have no API key.

## If something goes wrong

These messages appear beside the **Load on 1 node** button:

| Message | What to do |
|---|---|
| "Pick a model" | Choose a model in **Folder** |
| "Pick at least one node" | Select a node under **Nodes** |
| "Copy the model to cell2 first" | Click **Copy it there** under **Nodes**. That button appears only when Admin mode is unlocked. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes) |
| "cell2 already serves a model" | Pick another node, or [unload](help:unload-a-model) what's there |
| "cell2 has gemma4:latest in memory in Ollama, and unified memory is shared. Unload it there first." | Unload it in the [Ollama](open:ollama) window, or pick another node. See [Use Ollama](help:use-ollama) |
| "cell2 has containers left from an earlier load (sparkbox-m1a2b3). Remove them first." | See [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind) |
| "Port 8000 is taken" | Click the **Use …** link beside **Port** |
| "vllm/vllm-openai:… isn't on cell2: pull it first (Fleet: docker pull …), or pick an image that's there" | Pick another **Image**, or fetch this one, as described below |
| "Not enough free memory: tick the page-cache drop or lower the share" | Select **Drop the page cache first**, or lower **GPU memory share** |
| "The memory plan is red" | Lower **Max context** or **GPU memory share**, or use the button SparkBox offers |

**To fetch an image**, open [Fleet Command](open:fleet) and pick only that node. Leave **as root** off, set **Time limit** to `1800`, and run `docker pull` with the image's name. It's about 20 GiB. See [Run a command on every node](help:run-a-command-on-every-node). Then come back and pick the image.

These messages appear after you click the button:

- **"cell2 has 41.2 GiB free; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Tick 'Drop the page cache first' to let SparkBox free it (sudo; the kernel refills it)."** Select the box and try again. If the message then says "after dropping the page cache", lower the share or free memory.
- **"Port 8000 is already in use on cell2 (…). Pick another port."** Something took the port after the page read it. Pick another port.
- **"cell2 is already serving another model; unload it first."** Unload it, then load again.
- **"vLLM isn't running on cell2 any more."** SparkBox closed during a load, and when it opened again the engine had stopped. Load the model again.

If the load starts and then fails, see [Fix a load that failed](help:fix-a-load-that-failed). For more messages, see [Model messages](help:errors-models).

## Next steps

- [Tell whether a model is working](help:tell-if-a-model-is-working)
- [Chat with a cluster model](help:chat-with-a-cluster-model)
- [Common questions about models](help:model-questions)
- [Unload a model](help:unload-a-model), to stop it when you've finished
