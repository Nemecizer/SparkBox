---
id: models-load-page
title: "Models: the Load page"
type: feature
area: models
summary: "Every option for starting a model: recipe, nodes, split, memory plan, limits, access, the checks and what will run."
keywords: ["load page", "recipe", "memory plan", "before it starts", "what will run", "settings", "gpu memory share", "max context", "kv cache type", "listen on", "api key", "keep running", "remote code", "image", "tool calling", "expert", "raw vllm args", "leftovers"]
verified: false
related: [choose-load-settings, load-a-model-on-one-node, load-a-model-on-several-nodes, use-a-recipe-for-a-load, clear-what-an-earlier-load-left-behind]
---

# Models: the Load page

## What it's for

The **Load** page is where you start a model. You choose the model, the nodes and the limits, and SparkBox shows where each node will end up before anything runs. Nothing changes on a node until you click the button at the top.

The page looks long, but most loads need only three or four choices. [Load a model on one node](help:load-a-model-on-one-node) takes you through them, and [Choose load settings](help:choose-load-settings) helps you decide. This page explains every field.

## Where to find it

- **Library tab:** select a model and click **Load…**, or double-click its row.
- **Hugging Face tab:** select a model that's on a node and click **Load…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. In look-only mode this button is dimmed: use the **Library** tab.
- **Loaded tab:** click **Load a model…**.
- **Endpoints window:** click **Load a model…**, or **Edit & reload…** on a card.
- **Sidebar:** right-click **Models** and choose **Load a model…**.
- **Command palette:** press <kbd>⌘K</kbd> and choose **Load model…**.
- **Link:** [Open Load](open:models?tab=load)

If you open it without choosing a model, SparkBox picks the smallest one that fits a free node. If none does, **Folder** starts empty.

## What's on the screen

### Toolbar

- **Back** returns to the tabs without changing anything.
- The heading reads **Load a model with vLLM**, **Load a model with SGLang**, or **Edit … and reload**.
- A message appears when something stops the load, such as "The memory plan is red".
- **Load on 1 node** starts the load. The number follows the nodes you select. It's dimmed while a message is showing. When you're editing a load, it reads **Reload**.

### Model

- **Engine** is the program that runs the model: **vLLM** or **SGLang**. Keep **vLLM** unless you have a reason to change it. See [Engines: vLLM, SGLang and Ollama](help:engines).
- **Folder** is the model to load, from your **Library**. It lists only folders with a finished copy, so a download or copy that stopped partway isn't there. If you open the page on one from the **Library**, or a saved draft names one, the page says "Qwen/Qwen3.8-27B-FP8 isn't complete on cell2. Finish it first." and **Load on 1 node** stays off. [Start again](help:follow-a-download#start-again) shows how to finish it.
- **Recipe** is SparkBox's saved description of how to load this kind of model. It appears for vLLM. The row names the recipe it picked for the folder, or **Generic vLLM**, and says what it's been verified on: "Verified on: nothing yet", or a list of what has reached **Ready**. It's a menu when more than one fits. The recipe's values fill in **Max context**, **Requests at once**, **Tool calling** and **Reasoning parser**, unless you've changed that field. A recipe that can't load its model shows why, and the load button stays dimmed. See [Use a recipe for a load](help:use-a-recipe-for-a-load).
- **Name in the API** is the name apps use in their requests. SparkBox suggests the folder's name in lower case. Use letters, digits and `. _ - / :` only.

### Nodes

There's one box for each node, with a dot for its status. Beside each node, you see **has copy** or **no copy**, or **incomplete**, **copying** or **downloading** for an unfinished one, and what it's serving now. A node with a GPU fault can't be selected.

The first selected node in your node list is the head node, and it's the one apps talk to. The order you select them in doesn't matter. SparkBox selects the freest node with a copy for you.

- If a selected node has no copy, a note says so. When Admin mode is unlocked, the note offers **Copy it there**.
- If a selected node already has a SparkBox load, a note tells you to unload it first.
- With two or more nodes selected, a note reads "Each model, image and split needs its own checks. A successful load on one layout doesn't prove another." **What's proven** opens the recorded cluster results and their limits in Help.

**Inventory expectations need review.** appears when fresh fabric readings differ from your [saved expectations](help:settings-nodes#fabric-and-load-role). It also warns if the selected head has **Worker preferred** saved as its role. Click **Review in Settings** to review the fields. Missing readings are unknown; they don't confirm a match. These warnings are advisory: loads still use fresh node checks. After saving expectations, click **Check again** under **Before it starts** to refresh the plan.

### Memory plan


There's one bar for each node, showing where it will end up once the engine has reserved its share. From the left:

| Part of the bar | What it is |
|---|---|
| **in use** | What the node is using now, including anything else serving |
| **vLLM host runtime** | The engine's own process, outside the share |
| **weights** | This node's part of the model |
| **runtime** | Working memory for the engine, and any vision encoder |
| **KV cache** | Everything left in the share, however little the model needs |
| **free** | What's left over |

A dashed line marks 90% usage, a conservative planning limit. While serving, alerts account for the GPU reservation and still check available memory and pressure. Red shows anything past the end of the node's memory.

A line under the bar compares what the model needs with what the share reserves, such as "Needs 13.3 GiB; this share reserves 30.4 GiB, and the KV cache gets the rest, about 5 full-length requests." The first figure counts one full-length request, or as many as **Requests at once** says. The last one is how many the whole load can hold at this share. It doesn't appear when the model needs more than the share gives.

The word on the bar is the verdict: **Fits**, **Tight**, **Won't start safely**, **Won't fit** or **Won't start**. A line in color gives the reason. See [How memory works on a Spark](help:memory-on-a-spark).


When the plan doesn't read **Fits**, SparkBox may offer a button:

- **Use 0.60 — fits and stays under 90%** sets a lower share.
- **Use 0.40 and a 96k context — fits and stays under 90%** sets a lower share and a shorter context.

If no value works, it says "No share fits this model under the 90% line here: lower the max context or requests at once, or add nodes."

#### Override memory guardrail

This box appears only when the plan is red. It lets you load anyway, once you type the node's name.

> [!DANGER]
> **A Spark that runs out of unified memory can freeze, and then only someone at the machine can restart it.** Whatever else that node is doing stops too. Don't use this to get past a red plan. Lower the share or the context instead. Use it only if you understand why the plan is red, and you can reach the node. If a node does freeze, follow [Recover a frozen node](help:recover-a-frozen-node).

### Split and limits

#### Split

The splits for the number of nodes you selected, such as **3 × 1: tensor parallel 3** and **1 × 3: pipeline parallel 3**. **2 × 2** on four nodes is marked advanced. A split this model or its recipe can't take reads "(won't work)". When you select nodes, SparkBox picks the first split that works. It doesn't appear with SGLang.

The menu also offers independent single-node replicas on selected nodes, and two TP2 replicas on four nodes when the recipe allows them. Each replica has its own endpoint, key and unload boundary, with a shared served name and group label. Losing one replica does not stop another. Request distribution between replicas is not implemented yet.

#### Tensor × pipeline parallel

How the model is split across nodes. Tensor parallel × pipeline parallel × replicas must equal the number of nodes. With SGLang, this row reads **Tensor parallel** and it's set for you. See [Running one model on several nodes](help:splitting-a-model).

These two numbers follow the **Split** menu, and you can also type your own. A split the model can't take shows the reason in a note, such as "Tensor parallel 3 won't work for this model: 32 heads don't divide by 3. Use pipeline parallel 3.", and the load button stays dimmed. If the model's config file doesn't say how many heads it has, a note says SparkBox can't check the split, and the load can go ahead: vLLM should refuse a bad split when it starts, and SparkBox cleans up.

#### Expert parallel

Spreads a mixture-of-experts model across nodes. Leave it off for any other model.

#### Max context

How long a conversation this load can hold, in tokens, counting the prompt and the reply. The hint beside the field gives the model's own limit, such as "The model allows 262,144".

- **Default:** empty, which means the model's own maximum.
- **Effect on memory:** more context needs a bigger KV cache, the memory the model uses to track the text so far.
- **Choose:** a value you'll use, not the biggest one possible. `32768` suits most chats. Some models allow a million tokens, and an empty field plans for all of them.

#### GPU memory share

How much of each node's memory the engine reserves when it starts. `0.25` means 25% of the node's memory.

- **Default:** sized to the model, and the hint beside the field reads "Sized to this model." It's the least share that holds the model and four full-length requests with 10% to spare, in steps of 0.05, from `0.2` to `0.75`. It changes when you change **Max context**, **Requests at once**, **KV cache type**, the split or the nodes. [How a load gets its share](help:memory-on-a-spark#how-a-load-gets-its-share) shows the sums.
- **When SparkBox can't size it:** `0.75`. That happens when the model needs more than that, or when SparkBox can't tell how big the KV cache is or how long a request can be. The plan still judges that share, and still offers a lower one that fits.
- **With SGLang:** `0.75`. SparkBox sizes the share for vLLM only, because SGLang works out its memory differently.
- **While the plan is on its way:** the field is empty and reads "Waiting for the memory plan to size it". The nodes aren't read and the load button waits until the plan for this model and these nodes arrives, so a share worked out for another model is never used.
- **Allowed:** `0.2` to `0.95`.
- **Effect:** the engine takes the whole share at the start, whether it needs it or not. Whatever's left over inside the share becomes KV cache.
- **Choose:** the default. Go lower if the **Memory plan** says so. Going higher gives more cache and less safety.

A value you type stays, whatever else you change. So does a saved load's share in **Edit & reload…**, and a share from the AI advisor. When yours differs from what SparkBox would use, a button beside the field goes back to it, such as **Use 0.25 — sized to this model**. With no sized share, the button reads **Use 0.75 — the usual start**.

The fit table uses 0.8, so it can show more room than a load gets.

### Capabilities

#### Tool calling

Lets a model ask an app to run a tool, such as a file search, and use the result. The list beside the switch picks the **parser**, the code that reads the model's tool requests. SparkBox marks the one for this model family with "(this model family)".

- **Default:** off.
- **Choose:** on, if an app you use needs it, with the marked parser.

#### Reasoning parser

Some models think out loud before they answer. This setting separates the thinking from the answer, so apps can show them apart.

- **Default:** the one for this model family, or **None**.
- **Choose:** keep the default.

#### Prefix caching

Reuses the KV cache when requests start the same way, such as a shared system prompt or the earlier part of a chat. The model then doesn't have to read that part again.

- **Default:** on.
- **Choose:** leave it on.

#### KV cache type

How precisely the KV cache is stored.

- **Default:** **Model's own**. The menu also preserves explicit **bfloat16**, **fp8_e4m3** and **fp8_e5m2** choices from imported settings.
- **fp8 (half the memory):** takes half the memory per token, for a small cost in accuracy.
- **Choose:** the default, unless you need more context in the same memory. Sparse MLA with automatic or bfloat16 KV is blocked on GB10. Choosing fp8 alone does not establish a working kernel. **Sparse MLA** is one kind of attention. See [Sparse MLA on a GB10](help:splitting-a-model#sparse-mla-on-a-gb10).

The **KV fp8** choice in the fit table doesn't carry over to this page. Set this field yourself.

#### Requests at once

How many full-length requests the KV cache must hold together.

- **Default:** empty. The engine decides, the memory plan counts one, and a share sized to the model leaves room for four.
- **Effect:** the engine serves at most this many at a time, and the plan reserves KV cache for all of them at full length. A share sized to the model holds this many instead of four.
- **Choose:** empty, unless several people or apps will send long requests at the same time.

### Parser choices from the image

**Read parsers from selected image…** needs Admin mode because it starts short test containers to read `vllm serve --help`. Choices are shared by every selected node and tied to the image's local ID. A known unsupported parser blocks loading. Missing results remain unknown; they are not proof of support. After moving an image tag, read the choices again.

### Access

#### Port

The **port**, a numbered door on the node, that apps use to reach this model.

- **Default:** the first free port from `8000` on the head node. SparkBox shows **free on cell2**, or says what's using the port and offers another.
- **Choose:** the default, unless an app expects a particular number. Use `1024` or above.

#### Listen on

Which network can reach this model.

| Choice | Who can reach it |
|---|---|
| **LAN** | This Mac and your home network. This is the default |
| **Fabric only** | Only the other Sparks. Chat and the Server can't use it |
| **All interfaces** | Every network the node has, the fabric included |

The hint under the setting shows the address apps will use, such as "Apps reach it at `http://192.168.50.154:8000/v1`".

#### API key

A password for this load. Apps must send it with every request.

- **Default:** on. SparkBox makes a new key for each load and keeps it in the Keychain.
- **Effect of on:** Chat and the Server use the key for you. Other apps need it, or they can go through the Server. See [Connect an app to SparkBox](help:connect-an-app) and [Use a model from another computer](help:use-a-model-from-another-computer).
- **Effect of off:** "Without a key, anything on your network can use this model."
- **SGLang:** always off, because SGLang takes a key only on its command line, where anyone on the node could read it.
- **Choose:** on.

SparkBox never puts the key in a command. It reaches the node a safe way, in a file only its owner can read, and it's removed when you unload.

#### Keep running

Starts the model again if it stops. It works differently depending on the number of nodes:

| | One node | Several nodes |
|---|---|---|
| Who restarts it | Docker, on the node | SparkBox, on this Mac |
| Needs SparkBox open | No | Yes |
| Needs Admin mode | No | No, unless a node needs its page cache dropped and has no explicit standing grant |
| After a node restart | Docker starts it again | SparkBox restarts it in order, when the node is back |
| Timing | Docker decides | After 30 seconds, then 2, 5 and 10 minutes |
| A load that fails while starting | SparkBox removes it, if it's open. If it's closed, Docker keeps trying | SparkBox doesn't restart it, unless the load has been **Ready** before and this start only stopped. The wrong answer or the time limit ends the restarts even then. A load still starting or compiling is left alone |
| When it stops trying | Docker keeps trying | After 5 restarts in 30 minutes, or 5 in a row that don't reach **Ready** |

For several nodes, SparkBox acts by itself, with no Touch ID or Mac password. After SparkBox quits and opens again, it first checks the saved load against fresh node reports. Pending recovery or an incomplete load holds automatic restarts; see [After SparkBox quits or loses its connection](help:fix-a-load-that-failed#after-sparkbox-quits-or-loses-its-connection). Locking only delays an otherwise eligible restart. If you lock Admin mode, the screen locks or this Mac sleeps while a restart is waiting, the card reads "Held · locked". SparkBox puts that restart off by one wait, then tries again with Admin mode still locked. An idle lock doesn't cause this delay.

- **Default:** off.
- **Choose:** on, for a model you want available all the time.

> [!WARNING]
> **A model that keeps crashing keeps restarting.** With **Keep running** on one node, Docker restarts a load that crashes, again and again. If a load misbehaves, unload it, which stops the restarts for good. On several nodes, unloading does the same, and no restart happens while look-only mode is on.

#### Allow remote code

Some models need their own Python code to run. This setting allows that code to run inside the container, with access to the GPU and the model folder. On several nodes the containers run with `--privileged`, so that code runs as root with every device on the node. A red warning names the model and the nodes when it's on.

- **Default:** off.
- **Choose:** on only if the model needs it, and only for a repository you trust.

> [!DANGER]
> **Remote code is a program you're choosing to run on your node.** It comes from the model's repository. SparkBox can't check what it does, and the container has access to the GPU and the model folder. Turn this on only for an organization you trust. SparkBox never turns it on by itself, and the AI advisor can't set it.

#### Expert: raw vLLM args

Options for vLLM that have no field on this page. This field appears only for vLLM, with Admin mode unlocked. While Admin mode is locked, a saved option shows as plain text, with "Unlock Admin to change them".

- **Default:** empty. A recipe's own options don't show here.
- **Form:** one option per line. The first space splits the option from its value, so a value can hold spaces and quotes, such as `--speculative-config {"method": "mtp"}`. Each line goes to vLLM as it stands, with no shell.
- **Rules:** an option must start with `-`. SparkBox refuses options a field on this page already sets, and anything that holds a key, loads code or opens files. Options that take JSON must hold valid JSON. A refused option shows the reason.
- **Choose:** leave it empty, unless a model card or the AI advisor names an option.

#### Image

The recipe the engine's container starts from.

- **vLLM:** a list of images. Each says which chosen nodes have it, such as "on cell2 · sha256:5a0f8b91…" or "on cell1, cell2 · sha256:5a0f8b91…; not on cell3". Every chosen node needs the image, with the same id. SparkBox starts it by its local id, so all nodes run the same thing.
- **SGLang:** the image from **Settings › Models**, shown as plain text.
- **Default:** your saved image if every chosen node has it, then `vllm/vllm-openai:glm53-flash-arm64-cu130`, then the first vLLM image they all have.
- **Choose:** the default. If a model gives garbage answers, pick the fallback, `nvcr.io/nvidia/vllm:26.05-py3`. See [Fix a load that failed](help:fix-a-load-that-failed).

#### Advice from Check with AI

If you came from **Check with AI**, a note at the top gives the advisor's reason, and the fields it filled in are highlighted. **AI also suggested (not applied)** lists what SparkBox didn't apply. Changing a field removes its highlight.

### Before it starts

This group appears for a vLLM load on any number of nodes. SparkBox reads every chosen node as a set, so you see the facts before you commit. With **Demo** data it reads "Available with Live data", and no node is read. In look-only mode it still reads the nodes, and the load button stays dimmed.

The top line is one verdict for all the chosen nodes: "Ready to load on cell1 and cell2.", or "Not ready to load on cell1 and cell2: 1 problem to fix first." Beside it, "Checked 2 min ago" says how old the reading is, and **Check again** reads the nodes again. Red notes under it name each problem.

On several nodes, **Version consistency** compares the same fields as [Fleet › Versions](help:fleet-command#versions), using fresh reads for this load. **Compare version details** opens each node's values. **Mismatched** and **Unknown** are advisory; they do not add a new load block. Existing image-identity and Docker-store checks still stop unsafe combinations.

Here, **Serving image ID** and **NCCL (declared)** refer to the image selected on Load. NCCL comes from that image's `NCCL_VERSION` declaration, without starting a test container. If the image changes between reads, the declaration becomes unknown. **Collector build** reads the installed collector source; Fleet reports the source loaded by its running collector. In Demo, unavailable preflight versions stay unknown.

**Drop the page cache first** comes next, when vLLM would otherwise refuse to start. One choice covers every node that needs it, and it names them. Then **Fabric path**, on several nodes, says what's tested when you click the load button: "Tested when you press Load: each worker sends jumbo frames to the head on each half." Reading this page sends nothing across the fabric.

Each node then has a block, headed with its name. On several nodes, the heading also gives its part, such as "head, serves the API" or "worker, rank 1". If SparkBox couldn't read a node, or couldn't finish reading it, its block says only "Couldn't read cell2", and a red note says why.

| Row | What it shows |
|---|---|
| **Memory** | The node's total, free, available and cached memory, such as "121.7 GiB in all · 114.8 GiB free · 116.7 GiB available · 2.3 GiB cached" |
| **What vLLM needs** | The free memory it needs at this share, and whether the node has it. It's the share plus 4 GiB |
| **earlyoom** | Whether this memory-protection service is running, and its settings. It's shown for information and never stops a load |
| **Image** | The image's local id on this node, or **not on cell2**. Its hint names the node's Docker image store |
| **Model folder** | **present on cell2**, or **not on cell2**. A folder that's still marked incomplete counts as not there |
| **Port** or **Ports** | Whether the port is free. On several nodes, the row also checks port `29501`, which the nodes use to meet, such as "8000 and 29501 free on cell2" |
| **/dev/shm** | What's in shared memory now. **empty** is normal. SparkBox compares it after an unload |
| **Cache folders** | Three folders where vLLM keeps the code it prepares for the GB10, with their owners. A missing one is made at the start |
| **Fabric** | What the node reports for each half of the fabric, such as "enp1s0f1np1 192.168.100.1 · 200 Gb/s · MTU 9000 · RDMA ACTIVE". RDMA is how the nodes write straight into each other's memory |
| **Ollama** | Whether Ollama holds a model in memory on this node |
| **Left by an earlier load** | Containers an earlier load left here, or "nothing". **Remove leftovers…** removes this build's own. See [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind) |
| **Download running** | A download's container on this node. It's a job, not a leftover, so it doesn't stop a load. It goes on beside the load, and you stop it in [Jobs](open:models?tab=jobs) |

Across the set, SparkBox also compares each node's image and model files, and it looks for a Ray worker some other start left behind. [Model messages](help:errors-models#before-it-starts) lists each problem. The **earlyoom** row, a non-empty **/dev/shm** and a missing cache folder never stop a load.

SparkBox reads all of this again when you click the button, and a problem stops the load before anything changes. On several nodes it then tests the fabric from each worker to the head node, on each half.

### What will run

These are the exact steps SparkBox will run, node by node, in order. You don't need to read them, but they're here so nothing is hidden.

1. **Check each node.** This is the same reading as **Before it starts**.
2. **Drop the page cache**, if you selected it.
3. **Test the jumbo-frame path** from each worker to the head node, on several nodes.
4. **Write the launch files**, then **Start**, for each node: the workers, then the head node.

The launch files hold vLLM's options and settings, shown as JSON with the key masked as `sbk-••••••••`. A small launcher reads them inside the container and starts vLLM, so no shell reads your options. Containers carry the load's id, so **Unload** removes exactly these. The API key reaches the node on stdin, a private input channel, so it's never in a command or the container's settings. Only the head node gets it.

For several nodes, the steps may show `<fabric-iface>` until SparkBox has read the nodes' fabric. See [Prepare the cluster for models](help:prepare-the-cluster).

A saved, explicit [page-cache grant](help:settings-security#keep-running-page-cache-permission) can let Keep running perform the conditional cache step while Admin is idle-locked. It starts off disabled and does not bypass memory checks or an explicit owner hold.

## What it changes

- **Filling in the page** changes nothing, and needs no Admin mode. Only the way in from the **Hugging Face** tab asks you to unlock first. **Before it starts** only reads the node.
- **Load on 1 node** writes launch files to the node, starts its container and reserves memory. It needs Admin mode, which asks for Touch ID or your Mac password. To undo it, [unload the model](help:unload-a-model).
- **Load on 3 nodes**, or any other number, does the same on each node: the workers first, the head node last. If it fails partway, SparkBox removes what it started and keeps the logs on this Mac.
- **Remove leftovers…** stops and removes the named containers. It needs Admin mode. It can't be undone, and it doesn't touch model files.
- **Drop the page cache first** runs one command with sudo. Nothing is lost, and the node refills the cache.
- **Reload** unloads the model, then loads it again, with a new API key.

A model that reads **Ready** runs on its nodes, so it keeps running if SparkBox closes or this Mac sleeps. Until then, keep SparkBox open. See [Common questions about models](help:model-questions).

In look-only mode, you can open the page from the **Library** tab and read all of it, and the button stays dimmed.


## Related How-Tos

- [Load a model on one node](help:load-a-model-on-one-node)
- [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes)
- [Use a recipe for a load](help:use-a-recipe-for-a-load)
- [Choose load settings](help:choose-load-settings)
- [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind)
- [Fix a load that failed](help:fix-a-load-that-failed)

## Last-known copies

A Library copy marked **last known** is not a current source for loading or copying. Reconnect its node and press **Find models** in [Library](help:models-library#scan-results). The Load page still performs its own fresh prerequisite checks before starting a model.
