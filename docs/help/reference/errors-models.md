---
id: errors-models
title: Model messages
type: troubleshooting
area: troubleshooting
summary: Messages from Hugging Face, downloads, copies, the memory plan, the checks before a load, loads and readiness.
keywords: [won't start, out of memory, gated, no space left, garbage, 17+25, port in use, download failed, memory plan, red, unload, ready, garbled, gibberish, tensor parallel, heads, jumbo, leftovers, exit 78]
verified: null
related: [fix-a-load-that-failed, download-a-model, choose-load-settings, memory-on-a-spark, tell-if-a-model-is-working]
---

# Model messages

This page covers messages from finding, downloading, copying, loading, checking and unloading models, in that order. Each uses a real node name, so yours may show another. See [Fix a load that failed](help:fix-a-load-that-failed) for a walk-through of a failed load.

## Finding and checking a model

### "google/gemma-3-27b-it is gated or private. Accept its license on huggingface.co and add a Hugging Face token in Settings › Models."

**What it means.** Hugging Face won't give out this model until you accept its terms and SparkBox shows a token.

**What to do.** Follow [Get a Hugging Face token](help:get-a-hugging-face-token). Accept the license on the model's page with the same account.

### "google/gemma-3-27b-it wasn't found on Hugging Face."

**What it means.** Hugging Face has no model with that name, or you can't see it.

**What to do.** Check the spelling. A model ID has the form `org/name`.

### "Hugging Face is rate-limiting requests; try again in a minute."

**What it means.** Hugging Face has had too many requests from you.

**What to do.** Wait a minute. A saved token gives you a higher limit.

### "Couldn't reach Hugging Face: …"

**What it means.** This Mac couldn't reach the website.

**What to do.** Check this Mac's internet connection. "Hugging Face returned 503 for google/gemma-3-27b-it." means the site had its own problem, so try again later.

### "“gemma” isn't a Hugging Face model id like org/name."

**What it means.** What you typed isn't in the form of a model ID.

**What to do.** Type the full ID, such as `google/gemma-3-27b-it`.

### "MLX weights run on Apple silicon, not on a GB10. Look for the same model as safetensors, GGUF or NVFP4."

**What it means.** This copy of the model is made for Apple chips, and your Sparks use NVIDIA chips.

**What to do.** Search for the same model in another format. See [Model formats and quantization](help:formats-and-quantization).

### "Claude declined to analyze this model."

**What it means.** The AI feasibility check, **Check with AI**, got no answer from its model. You may also see:

- "The analysis ran out of room; try again."
- "The analysis reply didn't match the expected format"

**What to do.** Try again. The fit table works without it.

### "Pick Anthropic or OpenAI for the feasibility check."

**What it means.** **Check with AI** can use only those two.

**What to do.** Pick one in [Models settings](open:settings?section=models), and [add its key](help:add-a-cloud-provider).

## Downloading

### "Not enough disk space on …"

**What it means.** The named destination cannot fit the full transfer plus 5% and 20 GiB. Partial files do not reduce the requirement. The message shows available and required space.

**What to do.** Free space on that filesystem, or choose another download node, then review again. A resume stays on its original node. SparkBox keeps old job records and partial files; it never deletes them to make room.

### "Cannot check destination disk …"

**What it means.** The destination or its nearest existing parent could not supply a usable reading. The filesystem may be read-only, inaccessible or behind a broken link. Unknown file sizes also prevent a start.

**What to do.** Reconnect the node, inspect the named folder and refresh the model details. SparkBox refuses rather than guessing.

### "No space left on device on …"

**What it means.** A filesystem filled after the check. The message names the node and reports current space at the model destination; the failing write may have used a different tool or cache filesystem.

**What to do.** Check those disks and free space, then review a new attempt. Partial files are retained. The check does not reserve space or account for every quota or inode limit.

A running download shows its current step in **Models › Jobs**. If it fails, **Details** gives the reason. See [Follow a download](help:follow-a-download).

### "google/gemma-3-27b-it has no weight files SparkBox can serve."

**What it means.** The model's page has no files that vLLM can read.

**What to do.** Look for another copy of the model.

### "No nodes in the inventory."

**What it means.** SparkBox has no node to download to.

**What to do.** Add your nodes. See [Connect your cluster](help:connect-your-cluster).

### "hf download failed (exit 1): …"

**What it means.** The download tool stopped with an error. The end of the message is its last line.

**What to do.** Read that last line, then try again.

- `401` or `403` means the model is gated. See [Get a Hugging Face token](help:get-a-hugging-face-token).
- "No space left on device" means the node's disk is full. See [Check disk space on a node](help:check-disk-space), and [delete a model](help:delete-a-model) to make room.
- Network errors usually pass. Start the download again. It picks up from the files already on the node.

### "couldn't pull nvcr.io/nvidia/vllm:26.05-py3: …"

**What it means.** This is a historical download error. New attempts ask you to choose a tool and never fetch an image automatically.

**What to do.** Choose a tool in the download dialog. See [Download a model](help:download-a-model).

### "The download tool changed. Review the tool choice again; no fallback was started."

**What it means.** The tool or image no longer matches the choice you reviewed. Nothing else was selected automatically.

**What to do.** Open the download or resume action again and review the current choices.

### "The selected tool could not run hf version. Choose another tool; no fallback was started."

**What it means.** The chosen command or image could not run its version check. The model download did not start.

**What to do.** Review another tool. Installation is a separate choice and needs its own confirmation.

### "The uv checksum did not match. Nothing was installed."

**What it means.** The installer did not match SparkBox's pinned checksum. SparkBox refused to execute it.

**What to do.** Leave the refusal in place and report it. Do not disable the check.

### "Download incomplete: …"

**What it means.** The download finished, but files were missing, the wrong size, or failed their checksum. Wording such as "config.json is 100 bytes, expected 200" or "model.safetensors has the wrong checksum" tells you which.

**What to do.** Start the download again. See [Start again](help:follow-a-download#start-again). If it fails the same way, [delete the model folder](help:delete-a-model) and download it fresh.

### "A download of Qwen/Qwen3.8-27B-FP8 is already running on cell1. Follow it in Jobs."

**What it means.** A job is already filling that folder, so SparkBox won't start a second one.

**What to do.** Follow it on the [Jobs tab](open:models?tab=jobs). If you want to stop it, see [Cancel a download](help:follow-a-download#cancel-a-download).

### "The job stopped unexpectedly (node restarted?)."

**What it means.** The job's process on the node disappeared, usually because the node restarted. For a download the message goes on: "Its container may still be running: Resume download stops it first." The container runs apart from the job, so it can outlive it. For a copy it goes on: "Its copy is still running: a new copy stops it first."

**What to do.** Start the job again. The **Library** marks the model **◐ incomplete**. See [Start again](help:follow-a-download#start-again).

### "An earlier download is still writing to this folder: container sparkbox-dl-dl-5e1f00aa is running."

**What it means.** A new job found the last job's container still running, and couldn't stop it. Either the other SparkBox started it, which this one leaves alone, or Docker didn't answer. The new job stopped before it touched the folder, so two downloads don't write to one folder.

**What to do.** Look on the node's **Containers** tab. Stop it from the SparkBox that started it, or wait for it to end, then start again.

### "Docker didn't answer, so SparkBox can't tell whether an earlier download is still writing to this folder."

**What it means.** A new job on a folder with a stopped download asked Docker for that download's container, and Docker didn't answer in time. The new job stopped before it touched the folder, so two downloads don't write to one folder.

**What to do.** Check the Docker service on that node in [Services](open:services), then start again.

### "Docker didn't answer, so SparkBox couldn't look for a vLLM image on this node."

**What it means.** This is an older message from image discovery. New attempts show a note if installed images cannot be read; host tools may still be available.

**What to do.** Check the Docker service on that node in [Services](open:services), then start again.

### "Canceled, but its container may still be running: Resume download stops it first."

**What it means.** The job has stopped, but SparkBox couldn't confirm that its container stopped too. It appears in **Jobs**, and in a message named **Its container may still be running**. "Canceled. Its container may still be running: Resume download stops it first." and "Canceled. Its copy is still running: a new copy stops it first." say the same of a job that had already lost its process.

**What to do.** Look on the node's **Containers** tab. **Resume download** stops the container before it starts again. See [Start again](help:follow-a-download#start-again).

## Copying

### "No node has a finished copy of that model."

**What it means.** SparkBox can't find a source for the copy. A copy marked **◐ incomplete** can't be one.

**What to do.** Click **Find models** in [Library](open:models), or finish the download first. See [Start again](help:follow-a-download#start-again).

### "A copy to cell2 is already running. Follow it in Jobs."

**What it means.** A job is already filling that node's folder.

**What to do.** Wait for it on the [Jobs tab](open:models?tab=jobs). If it stops partway, copy again.

### "Pick at least one node that doesn't have it yet."

**What it means.** Every node you picked already has the model.

**What to do.** Pick another node, or nothing more is needed.

### "Copying a Hugging Face cache model isn't supported yet."

**What it means.** This model's folder is in the Hugging Face cache, and that folder holds links to files outside it. A copy can't follow them.

**What to do.** You can't copy this model yet. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes#limits).

### "cell1 has no fabric address; run Prepare cluster in Models."

**What it means.** Copies travel over the fabric, and this node has no fabric address.

**What to do.** Open [Prepare cluster](open:models?tab=prepare) and click **Check nodes**. If **Fabric (ConnectX-7)** reads **✗ no address**, SparkBox can't set one. See [Network settings SparkBox can't change](help:errors-network#network-settings-sparkbox-cant-change).

### "copy failed (rsync exit 12): …"

**What it means.** The copy stopped with an error. "Copy incomplete: …" means it ended, but some files didn't match. "rsync server stopped: …" means the sending side stopped.

**What to do.** Start the copy again from **Copy to nodes…**. The node reads "· incomplete" and is already selected, and the copy picks up from what arrived. If the disk on the target is full, free some space first.

### "An earlier copy may still be writing to /home/sparkuser/models/google/gemma-3-27b-it."

**What it means.** An earlier copy ended suddenly, and its copy tool may still be running on that node. SparkBox tried to stop it and couldn't confirm that it had. The new copy stopped before it touched the folder, so two copies don't write to one folder. **Delete** says the same and leaves the folder, while a copy that lost its job is still writing to it.

**What to do.** Look for `rsync` on that node in [Workloads](open:workloads), and wait for it to end, or stop it. See [See and stop processes](help:see-and-stop-processes). Then copy again. A restart of the node also clears it.

### "That model isn't in the library any more."

**What it means.** The folder has gone since the list was last read.

**What to do.** Click **Find models** in [Library](open:models) to read it again.

### "That model is loaded; unload it first."

**What it means.** You can't delete a model that's being served.

**What to do.** Follow [Unload a model](help:unload-a-model), then delete it.

### "Still running on cell2. Follow it in Jobs."

**What it means.** A download or copy is still writing to that folder on the node, so SparkBox didn't delete anything. It appears in a message named **Couldn't delete it**.

**What to do.** Wait for the job to end, or cancel it in the **Jobs** tab, then delete the model.

### "refusing to delete …: a download or copy is still writing to it. Follow it in Jobs."

**What it means.** SparkBox's helper on the node found a job still writing to the folder, and left the folder alone.

**What to do.** Wait for the job to end, or cancel it in the **Jobs** tab, then delete the model.

## The Load page

The words beside **Load on 1 node**, at the top of the page, name the first thing that stops a load.

### "Pick a model"

**What it means.** **Folder** is empty. You may also see:

- "Pick at least one node"
- "Pick a node."

**What to do.** Pick a model folder, then a node.

### "Copy the model to cell3 first"

**What it means.** You picked a node that doesn't have the model. "cell3 doesn't have ~/models/google/gemma-3-27b-it yet. Copy it to that node first (Library › Copy to nodes…)." is the same message from the node's check.

**What to do.** Use **Copy to nodes…**. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes).

### "cell2 already serves a model"

**What it means.** A node serves one load at a time. "cell2 is already serving another model; unload it first." is the same.

**What to do.** Unload the other model, or pick another node. See [Unload a model](help:unload-a-model).

### "gemma is already loading or serving."

**What it means.** That name is in use by a load that's running.

**What to do.** Unload it, or give the new load another **Name in the API**.

### "Port 8000 is taken"

**What it means.** Something else is listening on that port. The longer form names it: "Port 8000 is already in use on cell2 (python3). Pick another port."

**What to do.** Change **Port** on the Load page, or find what's using it in [Workloads](open:workloads), under **Listening ports**.

### "vllm/vllm-openai:latest isn't on cell2: pull it first (Fleet: docker pull vllm/vllm-openai:latest)"

**What it means.** The node doesn't have the engine image the load asks for. "cell2 doesn't have vllm/vllm-openai:latest. Pull it first (Fleet: docker pull vllm/vllm-openai:latest), or pick an image that's there." is the same.

**What to do.** Pick an image the node has under **Image**. Or fetch this one: see **To fetch an image** in [Load a model on one node](help:load-a-model-on-one-node#if-something-goes-wrong). In [Fleet Command](open:fleet), set **Time limit** to `1800` first. An image is about 20 GiB, and the default limit of 60 seconds stops the pull partway. Fetching one changes the node and uses disk, and it needs Admin mode.

### "The memory plan is red"

**What it means.** The load doesn't fit, and it would risk the node. The row in **Memory plan** gives the reason. The next four entries explain each one.

**What to do.** Read the row, then lower the settings it names. See [Choose load settings](help:choose-load-settings).

> [!DANGER]
> **A Spark that runs out of unified memory can freeze instead of failing cleanly, and it may need someone at the machine.** **Override memory guardrail** lets a red load start. It's for rare cases when you know why the plan is wrong. Don't use it to make a load fit. If a node does freeze, follow [Recover a frozen node](help:recover-a-frozen-node).

### "Won't start: vLLM reserves 91.3 GiB (0.75 share) at startup; 84.2 GiB is free."

**What it means.** Other things hold too much of the node's memory for the engine to take its share. With **SGLang**, the message names SGLang.

**What to do.** Lower **GPU memory share**, or stop what's using memory. Unload other models, and look at [Workloads](open:workloads).

### "Won't fit: needs 118.0 GiB, but a 0.75 share gives vLLM 91.3 GiB. Lower the context or requests at once, or use more nodes."

**What it means.** The model, plus the memory for its conversations, is larger than the share.

**What to do.** Lower **Max context** or **Requests at once**, raise **GPU memory share** a little, or split the model across nodes. Try **KV cache type** `fp8`, which takes half the memory.

### "Won't start safely: with this load cell2 would use 96% of its memory, and a Spark that runs out hangs."

**What it means.** The load fits on paper, but it leaves too little for the system.

**What to do.** Lower **GPU memory share** or stop what else is running.

### "Tight: runs above the 90% alert line (92% of cell2's memory)."

**What it means.** This is amber, not red. The load can start with little room left, and anything else that grows can run the node out. Alerts check available memory and pressure after loading. SparkBox asks you to confirm. A node that runs out of memory can freeze, and then only someone at the machine can restart it. A load with **Keep running** on starts again after that restart, and it can freeze the node again.

**What to do.** Click the button SparkBox offers, or lower **GPU memory share** or **Max context**. Go on only if someone can reach the node, and then watch the node's memory. While the node still answers, [unloading the model](help:unload-a-model) gives the memory back. If a node does freeze, follow [Recover a frozen node](help:recover-a-frozen-node).

### "cell2 has 84.2 GiB free; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Tick 'Drop the page cache first' to let SparkBox free it (sudo; the kernel refills it)."

**What it means.** Much of the node's memory holds recently read files, called the page cache. vLLM's startup check counts that as used and refuses to start. "Not enough free memory: tick the page-cache drop or lower the share" is the short form.

**What to do.** Turn on **Drop the page cache first**. It's harmless, and the cache refills by itself. If it's still short, the message ends "Lower the share or free memory."

### "Tensor parallel (2) × pipeline parallel (1) must equal the number of nodes (4): one GPU per Spark."

**What it means.** The two numbers multiplied must equal the nodes you picked.

**What to do.** Change the split, or the nodes. See [Running one model on several nodes](help:splitting-a-model).

### "The Gemma 3 27B recipe doesn't list this split. Pick Generic vLLM to try it."

**What it means.** The model could take this split, but its recipe leaves it out.

**What to do.** Pick a split from the **Split** menu. Or choose **Generic vLLM** under **Recipe**. No record stands behind that split. See [Use a recipe for a load](help:use-a-recipe-for-a-load).

### "Needs the FP4-Local checkpoint and its image, see What's coming"

**What it means.** This model's recipe can't load it yet, and no other recipe can stand in for it. Changing **KV cache type** doesn't help.

**What to do.** Load another model. See [What's coming](help:whats-coming).

### "Tensor parallel 3 won't work for this model: 32 heads don't divide by 3. Use pipeline parallel 3."

**What it means.** Tensor parallel gives each node an equal part of the model's attention heads, the parallel parts inside each layer. This model's heads don't divide by that number. The last sentence appears when pipeline parallel can take the same nodes.

**What to do.** Set **Tensor × pipeline parallel** to the split it names, `1` × `3` here, or pick another number of nodes. See [Tensor parallel](help:splitting-a-model#tensor-parallel).

### "Tensor parallel 4 won't work for this model: 6 KV heads don't divide by 4, and 4 isn't a multiple of them."

**What it means.** Some models share memory between attention heads. Those sets are called KV heads, and they must divide evenly too.

**What to do.** Pick another split. The **Tensor × pipeline parallel** field offers what this model can take.

### "Pipeline parallel 4 won't work: this model has 2 layers."

**What it means.** Pipeline parallel gives each node its own run of layers, so it can't use more nodes than the model has layers.

**What to do.** Use fewer nodes, or tensor parallel.

### "SGLang loads split by tensor parallel across nodes: set pipeline parallel to 1."

**What it means.** SGLang doesn't support the pipeline split.

**What to do.** Set **Pipeline parallel** to `1`.

### "SGLang only takes an API key on its command line, where anyone on the node could read it. Turn off “Require an API key” (and listen on the fabric, or use SparkBox Server in front)."

**What it means.** SparkBox won't put a secret on a command line, so an SGLang load can't have a key.

**What to do.** Turn off **Require an API key**. Then choose **Fabric only** under **Listen on**, or use the [Server](help:server).

### "The served model name may use letters, digits and . _ - / : only."

**What it means.** **Name in the API** has a character SparkBox won't pass on.

**What to do.** Use only those characters.

### "Use a port of 1024 or above."

**What it means.** Lower ports are reserved for the system.

**What to do.** Change **Port**.

### "GPU memory utilization must be between 0.2 and 0.95."

**What it means.** **GPU memory share** is out of range.

**What to do.** Enter a number in that range. `0.75` means 75% of the node's memory.

### "Requests at once must be at least 1."

**What it means.** The field needs a positive number.

**What to do.** Enter one, or empty the field to let the engine decide.

### "Max context must be a whole number from 1 to 1,000,000,000."

**What it means.** A number on the Load page can't be used as typed, so **Load** stays off and the last memory plan stays on screen. Three others read the same way: "Requests at once must be a whole number from 1 to 10000.", "Port must be a whole number from 1024 to 65535." and "GPU memory share must be a number from 0.2 to 0.95."

**What to do.** Fix the number. Leave **Max context** or **Requests at once** empty to use the default.

### "Couldn't work out the plan: …"

**What it means.** SparkBox couldn't make a memory plan from these settings. The end of the message says why. "Working out the plan…" only means it's still reading.

**What to do.** Read the end of the message, and change the setting it names. If it names a node, check that the node is online.

### "--middleware loads code or opens files, so SparkBox won't pass it on."

**What it means.** **Expert: raw vLLM args** refuses options that could run code or read other files. Other messages explain the other limits:

- "--port is set by SparkBox from the fields above; remove it from the raw vLLM args."
- "“foo” isn't an option (options start with --)."
- "“--foo bar=1” has a space in its name: put the value in its own element."
- "An option can't be empty or hold a line break."
- "--foo needs a value."
- "--speculative-config takes JSON, and this isn't: …"

**What to do.** Remove or correct the option. Use the field on the page where one exists. Put one option on each line, with a space before its value.

### "--trust-remote-code runs Python code from the model's repository. Tick “Allow remote code” to opt in; it can't be added as an Expert option."

**What it means.** Running a repository's own code is a choice with its own switch.

**What to do.** Turn on **Allow remote code**, if you trust the repository. See [Allow remote code](help:models-load-page#allow-remote-code).

### "Don't put an API key in the options: SparkBox gives vLLM its key from a file only it can read."

**What it means.** A key on a command line can be read by anyone on the node.

**What to do.** Remove it, and turn on **Require an API key**.

## Before it starts

**Before it starts** on the **Load** page reads every chosen node as a set. These messages name what stops a load. SparkBox reads again when you click the button, and nothing starts until they've all cleared.

### "Different Docker image stores on cell1 and cell2; pull the same tag on each node."

**What it means.** Docker keeps images in one of several kinds of store. An image's local id can't be compared between two kinds, so SparkBox can't tell that these nodes would run the same thing.

**What to do.** Fetch the same image name on each node. To fetch one, see **To fetch an image** in [Load a model on one node](help:load-a-model-on-one-node#if-something-goes-wrong).

### "Couldn't read Docker's image store on cell2, so its image can't be compared."

**What it means.** On a load across nodes, SparkBox compares the image on each node. To do that it has to know which kind of store each node uses, and Docker on this node didn't say.

**What to do.** Check that Docker is running on the node, in [Services](open:services), then click **Check again**.

### "vllm/vllm-openai:glm53-flash-arm64-cu130 isn't the same image on every node: cell1 has 5a0f8b91, cell2 has 7c3e1d02. Pull the same tag on each node."

**What it means.** The name is the same, but the image behind it differs. Nodes of one load have to run the same code.

**What to do.** Fetch the name again on each node, then check that **Image** shows one short id for all of them. Or pick an image that matches.

### "cell2 has gemma4:latest in memory in Ollama, and unified memory is shared. Unload it there first."

**What it means.** Ollama on this node holds a model in memory. The GPU and the rest of the node share one memory, so vLLM wouldn't get what the plan expects.

**What to do.** Unload it in the [Ollama](open:ollama) window, or pick another node. See [Use Ollama](help:use-ollama).

### "Port 29501, where a several-node load's ranks meet, is already in use on cell2 (python3). Free it first."

**What it means.** The nodes of a load meet on this port, and something else holds it.

**What to do.** Find it in [Workloads](open:workloads), under **Listening ports**, and stop it.

### "cell2 has no fabric address (ConnectX-7 with an IPv4 address)."

**What it means.** A load across nodes runs on the fabric, and this node reports no address on it.

**What to do.** See [Check the fabric](help:check-the-fabric).

### "Couldn't list the model's files on cell2: …"

**What it means.** SparkBox compares the model's files across the nodes, and this node didn't answer.

**What to do.** Check that the node is online, then click **Check again**.

### "cell2 has containers left from an earlier load (sparkbox-m1a2b3-r1). Remove them first."

**What it means.** This build of SparkBox started these containers, and no running load owns them.

**What to do.** Click **Remove leftovers…**. See [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind). It checks whose they are first, and it refuses in the same words as an unload if another SparkBox started them or a node doesn't answer. See [Unloading and restarting](#unloading-and-restarting).

### "dl-5e1f00aa is a download on cell2, not something a load left. Stop it in Models › Jobs."

**What it means.** A download's container is running, and SparkBox won't remove it as a leftover. A running download doesn't stop a load. It shows as **Download running** under **Before it starts**, and it goes on beside the load.

**What to do.** Leave it, or stop it in [Jobs](open:models?tab=jobs).

### "Nothing of m1a2b3 from this SparkBox is on cell2; nothing was stopped."

**What it means.** The containers have gone since the page read the node, or this SparkBox never started them.

**What to do.** Click **Check again**.

### "Couldn't remove what was left on cell2."

**What it means.** The removal ended without finishing. SparkBox read the nodes again, so the rows show what's still there. The message may give Docker's own reason in place of these words.

**What to do.** Read the rows, then click **Remove leftovers…** again. If the containers stay, remove them in [Containers](open:containers).

### "cell2 has a Ray worker still looking for its head (glm53). SparkBox didn't start it; remove it in Containers."

**What it means.** A container from another start is trying to join a Ray head. Ray is a program that links nodes. SparkBox's own loads don't use it. This container could take the GPU's memory when it finds its head.

**What to do.** If it's yours, remove it in [Containers](help:manage-containers). SparkBox never removes it for you.

### "The jumbo-frame path from cell2 to cell1 failed on enP2p1s0f1np1: the model's traffic can't cross the fabric."

**What it means.** When you click the load button, each worker sends a large test packet to the head node on each half of the fabric. This one didn't arrive, so nothing started. A **jumbo frame** is a packet of about 9000 bytes, and a load across nodes needs them on both halves.

**Before it starts** can show this before you click anything, with no packet sent: one of the two nodes has an address on a half where the other has none, so that half has nothing to test. It names the interface with no partner.

**What to do.** Check the fabric. See [Check the fabric](help:check-the-fabric) and [Fabric MTU](help:alert-rules-reference#fabric-mtu).

### "The model's files differ on cell3: 2 files differ, first model-00002-of-00004.safetensors."

**What it means.** The chosen nodes don't hold the same copy of the model. SparkBox compares file names and sizes across the nodes.

**What to do.** Copy the model to that node again. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes).

## Starting

### "cell2: SparkBox can't use Docker (is sparkuser in the docker group?)"

**What it means.** Docker refused your user on that node.

**What to do.** See ["Installed, but this user can't use it"](help:errors-connection#installed-but-this-user-cant-use-it).

### "cell2: couldn't ask Docker about vllm/vllm-openai:latest: …"

**What it means.** Docker on the node gave an error when SparkBox read the image.

**What to do.** Check that Docker is running on the node, in [Services](open:services).

### "cell2: docker run failed: …"

**What it means.** Docker couldn't start the container. The end of the message is Docker's reason, such as "docker: Error response from daemon: …". If it's empty, the connection dropped. On a load across nodes, the head node never starts after a worker fails this way, and SparkBox removes the workers it started. That node's saved log starts "docker run said:".

**What to do.** Read Docker's reason, then try again.

### "cell2: docker run didn't answer within 60 s. Check Containers on cell2."

**What it means.** The command to start the container didn't finish in time. The node may be busy, or Docker may be stuck. The head node doesn't start after a worker fails this way, and SparkBox removes what it started.

**What to do.** Check [Containers](open:containers) on that node, and the Docker service in [Services](open:services). Then load again.

### "vLLM stopped on cell2 while it was starting, and Docker started it again (once). A start that stops isn't retried. Last log lines: …"

**What it means.** A load with **Keep running** on tells Docker to restart it, and Docker did that before the load had ever been **Ready**. A start that stops isn't retried, so SparkBox saved the log and removed the container. "(3 times)" means Docker had already tried three times.

**What to do.** Read the log lines, then see [Fix a load that failed](help:fix-a-load-that-failed). Leave **Keep running** off until the model has worked.

### "The worker (rank 1) on cell2 stopped while the load was starting (exit 1)."

**What it means.** One node of a load across nodes ended while the others waited. The load can't go on without it, so SparkBox saved each node's log and removed the containers. "its container is gone" in place of the exit code means Docker no longer has it.

**What to do.** Read the log on the **Loaded** tab, then see [Fix a load that failed](help:fix-a-load-that-failed).

### "vLLM answered without its API key, so it was stopped rather than left open."

**What it means.** SparkBox asked the model for its list with no key, and it answered. So it would have served anyone on your network. SparkBox failed the load and removed it.

**What to do.** Load it again. If it happens again, keep **API key** on and read the log.

### "sparkbox: the API key file is missing or empty, so vLLM was not started."

**What it means.** SparkBox's launcher reads the load's key before vLLM starts. The key file wasn't there, or it was empty, so it stopped with exit code 78 instead of serving the model with no key. This line appears in the load's log.

**What to do.** Load the model again, which makes a new key. If it happens again, check that the node's disk isn't full. See [Fix a load that failed](help:fix-a-load-that-failed#the-launcher-refused-to-start-vllm-exit-78).

### "A load can't take --config, so no files were written."

**What it means.** The node checks a load's options once more before it writes the launch files, and this one loads code or opens files. Other lines end the same way, such as "A load runs vllm serve and nothing else, so no files were written.", "A load can't set NCCL_NET_PLUGIN, so no files were written." and "A worker takes no API key, so no files were written." Nothing started on that node.

**What to do.** Remove the option from **Expert: raw vLLM args** and load again. If you didn't set one, send the line and the load's settings, because the Mac should have refused this first.

### "Couldn't read cell2's memory (/proc/meminfo)."

**What it means.** SparkBox's check before a load got no figures.

**What to do.** Check that the node is online, and try again.

### "Couldn't finish reading cell2: it stopped at the ports, and nothing from there on was checked. Check again."

**What it means.** The node stopped answering partway through **Before it starts**. Whatever the check hadn't reached isn't known, so the load stays blocked, and that node's block shows nothing but "Couldn't read cell2". The sentence names where it stopped, such as the image, the containers or Ollama.

**What to do.** Check that the node is online, then click **Check again**.

### "vLLM stopped on cell2 (exit 1). Last log lines: …"

**What it means.** The engine started, then stopped. What's left depends on when it stopped.

- **While the load was starting:** SparkBox saved each node's log to this Mac and removed the containers, so there's no **Failed** row. The load's note on the **Loaded** tab gives the cause.
- **After it had read Ready:** the containers stay, and **Models › Loaded** has a **Failed** row for them.

**What to do.**

- **While the load was starting:** read the note, then read **vLLM log** under the table.
- **After it had read Ready:** click **Explain…** on the **Failed** row, then see [Why a load failed](#why-a-load-failed).

[Fix a load that failed](help:fix-a-load-that-failed) walks you through both.

### "vLLM didn't become ready within 90 minutes."

**What it means.** The load took longer than SparkBox allows, so SparkBox stopped waiting and marked it failed. A first load prepares code for the GB10 and can be slow, but not this slow. SparkBox saves each node's log to this Mac, then removes the containers. A node out of reach counts too: past the limit, a node that stays away for 10 minutes fails the load, and that node keeps its containers.

**What to do.**

1. Check that the node is online. If it's stopped answering, it may be out of memory, and someone may need to restart it at the machine.
2. On the **Loaded** tab, read **vLLM log** under the table. For the whole log, see [Where the logs are](help:fix-a-load-that-failed#where-the-logs-are).
3. If the load's note names nodes that still hold containers, click **Forget…** on the note. SparkBox unloads them first.
4. Load it again with a lower **Max context**.

### "cell2: couldn't drop the page cache: …"

**What it means.** This load has **Drop the page cache first** on, and the node refused the drop. The end of the message is the node's reason. No container started.

**What to do.** Read the reason, then try again. If it stays, lower **GPU memory share** until the node needs no drop.

### "Couldn't read the image's id on cell1, cell2."

**What it means.** SparkBox starts a load from the image's local id, and these nodes' check didn't give one. No container started.

**What to do.** Check that Docker is running on those nodes, in [Services](open:services), then load again.

### "It isn't using the RDMA fabric: its message queue is on 192.168.50.51, which isn't a fabric address."

**What it means.** The nodes of a load should send all of their traffic over the fabric. SparkBox reads the head node's log while the model loads, and the log says some of it went another way. "It isn't using the RDMA fabric: NCCL is on sockets, with no RDMA device." means the same for the traffic between the nodes' GPUs. You see these on the **Loaded** tab and in the load's message. The load keeps going and can read **Ready**, but it can run slowly, and it isn't running as designed.

**What to do.**

1. On the **Loaded** tab, click **Copy** on the **vLLM log**. Unloading removes that log, and it's what to send if you ask for help.
2. Follow [Unload a model](help:unload-a-model).
3. Follow [Check the fabric](help:check-the-fabric), then load it again.

### "Couldn't save the API key in the Keychain: …"

**What it means.** macOS refused to store the load's new key.

**What to do.** Unlock the Keychain, or allow SparkBox when macOS asks, and try again.

## Is it answering?

SparkBox says **Ready** only when a model really answers. While it's checking, the card reads "Up; checking it answers…" with one of these reasons. Most clear by themselves.

### "It doesn't answer /health yet (no answer)."

**What it means.** The engine is still starting. "It doesn't answer /health yet (HTTP 503)." means the same.

**What to do.** Wait. SparkBox asks again.

### "It answers /health, but /v1/models doesn't list gemma."

**What it means.** The engine runs, but not under the name you gave.

**What to do.** Wait a moment. If it stays, check **Name in the API**.

### "It answers /health, but /v1/models refused SparkBox's key (HTTP 401)."

**What it means.** The engine asked for a key, and rejected the one SparkBox has.

**What to do.** Unload it and load it again. SparkBox makes a new key for each load.

### "The test token failed: …"

**What it means.** The model is up, but it didn't produce one token of text. The reason follows, such as "it refused SparkBox's key" or "it serves neither completions nor chat".

**What to do.** Read the reason. SparkBox tries again at the next check.

### "It answers, but wrongly: asked 17+25, it said '…'. This is the GB10 garbage-output bug; try the fallback image (nvcr.io/nvidia/vllm:26.05-py3)."

**What it means.** The model loaded and replied with nonsense. On a GB10, some engine images do this. SparkBox marks the load **Failed**, so that your apps never use it. It saves each node's log to this Mac and removes the containers.

**What to do.**

1. SparkBox has already removed its containers and kept the logs. On the load's note on the **Loaded** tab, click **Forget**. If the button reads **Forget…**, containers are still on a node, and SparkBox unloads them first.
2. Pick another image under **Image** on the Load page.
3. Load it again.

### "Asked 17+25, it gave no answer within 512 tokens; it was still thinking ('…'). It's asked again at the next check."

**What it means.** Some models think at length before they answer. SparkBox gave this one room, and it hadn't finished.

**What to do.** Wait. SparkBox never calls a model wrong for thinking.

### "Look-only mode holds the test token; it's checked once look-only ends."

**What it means.** A test question is a use of the model, and look-only mode stops that.

**What to do.** Turn off look-only mode.

## Why a load failed

**Explain…** on a **Failed** row reads the container's log and names the likely cause. These are the titles it shows. Each comes with a suggested fix.

### "No attention kernel for a bf16 KV cache on GB10"

**What it means.** A large model uses sparse MLA attention, a memory-saving method. This image has no compatible attention kernel for the model and cache type. Your own run of `GLM-5.3-Flash` failed this way.

**What to do.** Check the model's recipe for a compatible checkpoint, image and cache type. Changing **KV cache type** alone may not fix it. SparkBox refuses `GLM-5.3-Flash` at any cache type. See [What's coming](help:whats-coming).

### "Likely ran out of GPU memory while starting"

**What it means.** The engine asked for more memory than was free. On a Spark, the GPU shares memory with everything else.

**What to do.** Lower **GPU memory share** or **Max context**, or free memory on the node.

### "The nodes likely couldn't reach each other (NCCL)"

**What it means.** The nodes of a load didn't finish linking up over the fabric.

**What to do.** Check the fabric in [Fabric](open:fabric). Start the nodes together, and check that each one uses the same head address and port.

### "Its port was likely already taken"

**What it means.** Another program was already listening on the port.

**What to do.** Find it in [Workloads](open:workloads) under **Listening ports**, or pick another port.

### "Hugging Face likely refused the download (gated or no token)"

**What it means.** The model is gated, and the node has no token with access.

**What to do.** Accept its terms, then download it in **Models**. See [Get a Hugging Face token](help:get-a-hugging-face-token).

### "The disk likely filled up"

**What it means.** A write failed because the disk has no space.

**What to do.** See [Check disk space on a node](help:check-disk-space).

### "Weights likely missing while offline (HF_HUB_OFFLINE)"

**What it means.** A file the model needs wasn't in the folder, and loads don't reach the internet.

**What to do.** Copy the full model folder to every node, from the [Library](open:models).

### "The Ray cluster likely didn't form in time"

**What it means.** This is for a server SparkBox didn't start. A worker didn't join in time. Ray is a program that links nodes, and SparkBox's own loads don't use it.

**What to do.** Check that every node is online, and that the fabric is healthy.

### "Its quantization likely isn't supported on this GPU"

**What it means.** The weights use a compact format this image can't run on a GB10.

**What to do.** Use a copy made for this chip, in NVFP4 or FP8, or a newer image.

### "vLLM's engine didn't start; the first error is quoted"

**What it means.** The log names no known cause. "Its log names no error." appears when it doesn't have that either.

**What to do.** Read the quoted lines. Then read the log around them, in any of these places:

- On the **Loaded** tab, **vLLM log** under the table shows the last lines of a load you started.
- In [Endpoints](open:endpoints), **Logs** on the model's card opens the whole log in **Terminal**.
- On the [Dashboard](open:dashboard), **Show log** on the failed load's card does the same. That card appears only while nothing is serving.

## Unloading and restarting

### "Couldn't check gemma's containers on cell2; nothing was stopped."

**What it means.** SparkBox won't unload until it knows whose containers they are, and the node didn't answer.

**What to do.** Check that the node is online, then try again.

### "gemma's containers on cell2 are managed by the other SparkBox; nothing was stopped."

**What it means.** Another copy of SparkBox started them.

**What to do.** Unload it from that copy. See [The other SparkBox](help:errors-admin-and-safety#the-other-sparkbox).

### "gemma's containers on cell2 have no SparkBox instance label, so whose they are can't be told; nothing was stopped. Remove them in Containers."

**What it means.** They were started by an older SparkBox, or by hand.

**What to do.** Check them in [Containers](open:containers), and remove them there if they're yours.

### "Couldn't reach cell2 to stop its containers; they may still be running."

**What it means.** The node didn't answer during the unload.

**What to do.** When the node is back, click **Unload…** again.

### "Its containers on cell2 are still there; it isn't marked stopped."

**What it means.** Docker didn't remove them, so SparkBox won't say they've gone. The message may add what Docker said.

**What to do.** Click **Unload…** again. If they stay, remove them in [Containers](open:containers).

### "Unloaded, but something didn't come back; see the report."

**What it means.** The model stopped, and a check afterward found something amiss, such as "memory not back after 60 s", "port 8000 still taken", or leftovers in `/dev/shm`.

**What to do.** Read the report in [Tasks](open:tasks). Memory usually comes back soon. If it doesn't, restart the node.

### "No containers of gemma from this SparkBox were running; it's marked stopped"

**What it means.** There was nothing to stop.

**What to do.** Nothing.

### "cell2 needs its page cache dropped before this load can start again, and Keep running can only do that while Admin mode is unlocked."

**What it means.** **Keep running** tried to restart the load, and too little memory is free on that node. Without a scoped standing grant, dropping the page cache needs Admin mode. Current builds name the missing grant in this message; see [Keep running page-cache permission](help:settings-security#keep-running-page-cache-permission).

**What to do.** Unlock Admin mode, then click **Restart…** on the model's card.

### "cell2 has 84.2 GiB free once this load stops; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Click Restart and choose 'Restart and allow cache drop' to permit it for this attempt (sudo; the kernel refills it). Nothing was stopped."

**What it means.** **Restart…** checks the nodes before it stops anything. The load wouldn't come back, so it's still running. This attempt had no permission to drop the page cache. Other refusals before a stop also end "Nothing was stopped." If the check that follows the stop finds the same thing, the message has no "once this load stops" and no "Nothing was stopped.", because the load has stopped.

**What to do.** Click **Restart…** and choose **Restart and allow cache drop**. That allows a drop if needed for this attempt and keeps the API key. **Retry** in **Tasks** offers the same choice; permission is never carried over from an earlier attempt. See [Restart a model or change its settings](help:restart-or-change-a-model).

### "It was unloaded meanwhile; nothing was started."

**What it means.** A restart was on its way when you unloaded the model.

**What to do.** Nothing. Load it again if you want it.

### "Its API key is gone from the Keychain; load it again instead."

**What it means.** A restart needs the load's key, and it's no longer saved.

**What to do.** Load the model again from the Load page.

### "vLLM isn't running on cell2 any more."

**What it means.** The engine has stopped since you asked.

**What to do.** Load it again.

### "Unknown model deployment."

**What it means.** SparkBox no longer has a record of this load.

**What to do.** Close the window, and open **Models › Loaded** again.

### "SparkBox started this one: unload it instead, so its workers stop too."

**What it means.** You chose **Stop containers…** on a load SparkBox started. "SparkBox loaded this one: restart it from its card instead." is the same, for **Restart…**.

**What to do.** Use **Unload…** or **Restart…** on its card.

### "That endpoint isn't there any more."

**What it means.** The server has gone since the list was last read.

**What to do.** Use **Refresh**, then look again.

### "Only a server that runs in containers SparkBox didn't start can be restarted here."

**What it means.** The server isn't in a container. "ollama on cell1 isn't a container: restart it in Services or Workloads." says where to go.

**What to do.** Use [Services](open:services) or [Workloads](open:workloads).

## Benchmarks and Ollama

### "Benchmarks need a completions API that takes token-id prompts; Ollama has none."

**What it means.** The benchmark sends exact token counts, and this engine can't take them.

**What to do.** Benchmark a vLLM or SGLang load.

### "It isn't ready yet."

**What it means.** You can only benchmark a model that's **Ready**.

**What to do.** Wait for **Ready**.

### "“gemma” isn't an Ollama model name like qwen3:8b."

**What it means.** The name isn't in Ollama's form.

**What to do.** Copy the name from the Ollama library, including its tag.

### "Ollama couldn't pull qwen3:8b: …"

**What it means.** Ollama gave an error. "Stopped; the partial download stays in Ollama's cache." means you stopped it, and that it can continue later.

**What to do.** Read the reason. Check the node's internet connection and free disk. See [Use Ollama](help:use-ollama).

### "That Ollama server isn't running any more."

**What it means.** It has stopped since the list was read.

**What to do.** Start it in [Services](open:services).
