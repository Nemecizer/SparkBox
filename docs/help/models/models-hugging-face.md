---
id: models-hugging-face
title: "Models: Hugging Face tab"
type: feature
area: models
summary: "Search for models, see the fit table for your Sparks, and start a download."
keywords: ["hugging face", "search", "recommended", "fit table", "gated", "mlx", "check with ai", "advisor", "kv room", "decode"]
verified: false
related: [find-a-model-that-fits, download-a-model]
---

# Models: Hugging Face tab

## What it's for

The **Hugging Face** tab is where you look for a new model. You search the [Hugging Face](https://huggingface.co) website, see how much memory a model needs and how many Sparks it takes, and start a download. Everything you read here comes straight from the website to this Mac, so it works with **Demo** data too.

## Where to find it

- **Sidebar:** under **Workloads**, click **Models**, then click the **Hugging Face** tab.
- **Link:** [Open Hugging Face](open:models?tab=hf)

## What's on the screen

The tab has a list on the left and details on the right. Drag the divider between them to change their widths.


### Search

Type in the box and press <kbd>Return</kbd>, or click **Search**. You can type part of a name, paste `org/name`, or paste a `huggingface.co` link. A name or a link also opens that model's details straight away. SparkBox shows the 40 most-downloaded matches.

### Recommended for GB10

Before you search, the left side lists four starting points, each with its reason. Click one to see its details. If this is your first download, start with `Qwen/Qwen3.8-27B-FP8`: it fits on one Spark, so it needs no splitting.

These four suit the chip in your Sparks. That doesn't mean they've run on your cluster. [If you're not sure which to pick](help:find-a-model-that-fits#if-youre-not-sure-which-to-pick) says what each reason means in plain words, and which one to leave for later.

### Search results

If results cover several kinds of task, buttons above the table filter them, such as **text-generation**. **All tasks** clears the filter.

| Column | What it shows |
|---|---|
| **Model** | The name, with who published it under it. **MLX · Apple only** marks files that don't run on a GB10 |
| **Task** | The publisher's label for what the model does. Models for chat are usually `text-generation` or `image-text-to-text`. `any-to-any` is a broad label that often includes chat. **—** means no label |
| **Weights** | The size of the files the engine serves. `…` means SparkBox is reading them. **≈** marks an estimate. **GGUF** means only files for Ollama or llama.cpp |
| **Fits on** | The fewest Sparks that can hold the weights, such as **1 Spark**. **· fp8** means it needs an fp8 KV cache. **more than 4** means it won't fit |
| **Downloads**, **Likes** | How popular it is on Hugging Face |
| **Updated** | When the repository last changed |

In a narrow window, some columns hide. Widen the window to bring them back.

**Fits on** counts only the weights. A model also needs memory for context, so open it and read the fit table. [Choose from search results](help:find-a-model-that-fits#choose-from-search-results) says which rows to skip, and how to tell an original from a re-upload.

### Model details

Click a row, and the right side shows:

- **The name**, which links to its page on Hugging Face. **gated** means you must accept a license first. See [Get a Hugging Face token](help:get-a-hugging-face-token). Its license name comes next.
- **A facts line**, such as "28B parameters · 28.7 GiB of safetensors weights · fp8". **MoE** marks a mixture of experts, and "sparse MLA attention" marks a model whose GPU kernel support needs checking for the selected image and KV type. Choosing fp8 alone does not establish compatibility.
- **A download line**, such as "Downloads 60.8 GiB of the repo's 182 GiB: leaves out original/ (…)". This is what the node will store.

### Revision and files

**Branch or tag** lists the versions Hugging Face reports. Each choice shows its type and the start of its commit, the fixed version it points to. Selecting one inspects that commit. **Refresh branches** reads the list again without changing the inspected model.

**Revision** also accepts a branch name, tag or full commit. Click **Inspect revision** after typing. Download and AI advice stay dimmed while your typed choice is unreviewed. A failed inspection removes the old details so they cannot be used by mistake.

**Commit** shows the full version that was read. Expand **Files to download** to see every name, size and available SHA-256 checksum. Disk checks and new downloads use this same commit and list. SparkBox refuses if the rechecked repository, commit, names, sizes or checksums differ; inspect again to make a new choice. A branch moving later does not change an already inspected commit.

If **config.json** cannot be read, the message replaces the fit table. HTTP 401 or 403 means access was refused; check the license and token. HTTP 404 means the configuration is missing at that revision. Retry with **Inspect revision** after fixing access or choosing another version. Download and AI advice stay unavailable while the configuration is unreadable.

### Model actions

- **Check with AI** asks your AI provider for an opinion. See [Ask the AI advisor](#ask-the-ai-advisor).
- **Download node** selects where the next download goes. See [Download node](#download-node).
- **Download to cell1**, the hard drive with a down arrow, names the selected node. These buttons often show as icons without names: move the pointer over one to read its name. A finished copy on that node hides its Download button. If that node has an incomplete copy, you see **Download again to cell1**, which uses the commit and files you inspected here. Use **Resume download…** on **Jobs** for the saved revision and file list. While that node's folder is being filled, you see "Downloading on cell1" or "Copying on cell1" and no Download button. A finished copy on a different node still lets you use **Load…**.
- **Load…** opens the [Load page](help:models-load-page). It's dimmed until a finished copy of the model is on a node. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. In look-only mode this button is dimmed: use **Load…** on the **Library** tab.

MLX repositories have none of these. A repository with no weights SparkBox can serve keeps these buttons, with **Download to cell1** and **Load…** dimmed. In both cases a note explains why.

Choosing a revision here affects new downloads. It does not replace a finished Library copy. **Load…** uses that saved copy and leaves out AI advice for a different revision.

### Download node

Choose a connected node from **Download node** before starting. The first connected node is selected initially; use **Suggest a download node** to compare capacity. The button and final review name your choice. Changing the selection starts no download or tool check.

Expand **Suggest a download node**. Select the nodes you are considering for the load, then click **Compare destination capacity**. Connected nodes without finished or running copies start selected. These choices limit the comparison; they do not set load members or copy files.

Each result names the destination, available space and required reserve. The suggested node has the most space left after the full transfer plus 5% and 20 GiB. Equal results use node ID order. Unknown, disconnected or short disks cannot win. Fullness warnings remain visible; a warning does not erase sufficient capacity.

To change **Download node**, click **Use cell2**, or the name shown. Reading capacity never changes your selection by itself. Results expire after one minute; changing the revision, comparison nodes or connections requires another comparison. This reads only, works in look-only mode and needs no Admin unlock. It does not install a tool, create a model folder or start a job.

SparkBox remembers the selected node for this window. Reordering or renaming nodes does not change the target. If it disconnects or is removed from inventory, downloading is dimmed until you reconnect it or choose another node. SparkBox never sends the download to a replacement by itself.

After tool choice, **Destination disk** shows the selected node's actual destination filesystem, available space and required space. The full transfer plus 5% and 20 GiB must fit; above 85% projected fullness, read the warning before approving. Unknown or insufficient space stops the attempt. SparkBox checks again at start, but does not reserve space. Expand **Then copy to** to choose connected nodes with no existing copy. This adds an explicit download → verify → copy chain to the final review. Empty folders, disk space, `rsync` and a shared fabric subnet are checked on every node. See [Download, then copy to other nodes](help:download-a-model#download-then-copy-to-other-nodes) for consent, progress and recovery. You can also use **Copy to nodes…** in the Library afterward. **Resume download…** in Jobs always uses that job's original node; this picker does not move partial files.

### Download tool choice

Each new download and **Download again** asks you to choose the tool on the selected node. The list can show an existing `hf` command, a verified SparkBox installation, a new installation, or installed images. The selected location or exact image ID is shown.

**Continue** moves to the final download review. Installing needs **Install tool and download** there. Canceling either dialog starts nothing. An unsuitable or changed tool stops the attempt; SparkBox does not try another or pull an image. [Download a model](help:download-a-model#if-you-choose-installation) explains the installation and cleanup limits.

### Fit table

The heading says how many Sparks you have, how much memory each one has, and the share it assumes: for example, "Fit on 4 Sparks (122 GiB memory each, vLLM share 0.8)". There's one row for each number of nodes. If you can't see the last columns, drag the divider left or scroll the table sideways.

A dimmed row doesn't fit. Otherwise, read across:

| Column | What it tells you |
|---|---|
| **Nodes** | How many Sparks the model is split across |
| **Weights** | How much of the model each node holds |
| **KV room** | What's left for the KV cache on each node, the memory the model uses to track the text so far. **none** means nothing is left |
| **Split** | How it can be split. **single node** needs no split. **TP 4** means tensor parallel across 4 nodes. **PP 3 only** means only pipeline parallel works. **can't split 3 ways** means neither works. **✗ doesn't fit** means there's too little memory. See [Running one model on several nodes](help:splitting-a-model) |
| **Context** | The longest single request, in tokens. **· model max** means memory isn't the limit: the model itself stops there |
| **Requests** | How many requests of the model's full length fit together. **≈0.6** in amber means less than one, so set **Max context** lower when you load it |
| **Decode t/s** | A ceiling on writing speed for one reply, in tokens per second. Real speed is lower. **· no speedup** means more nodes don't help one reply. **varies** means speed depends on the model's design |
| **Fits now** | Whether enough nodes have room right now: **Yes**, **No**, a count such as **2 free**, or **unknown**. Move the pointer over it to see which nodes |

Above the table, **Model's max context** is the model's own limit. **Requests** beside it says what "full-length" means for this model.

Beside the title, **KV bf16** and **KV fp8** change the table. **KV fp8** halves the memory each token of context takes, for a small cost in accuracy.

Under the table you may see a line such as "Seen on your cluster: Oct 26, TP 4, KV bf16, exit 1". It means this model already failed on your nodes, with those settings.

> [!NOTE]
> Treat the table as a guide. When it disagrees with the **Memory plan** on the **Load** page, trust the plan. The table assumes a share of 0.8 of each node's memory, while a load's share is sized to the model and starts at 0.75 at most. **Fits now** counts only what the weights need, while a load reserves its whole share at the start. So **Fits now** can read **Yes** on a busy node where the **Memory plan** reads **Won't start**.

The **KV fp8** choice stays on this table. To load with it, set [KV cache type](help:models-load-page#kv-cache-type) on the **Load** page as well.

### Ask the AI advisor

The advisor is optional. It reads the same facts and gives an opinion on how well the model will serve, how to split it, and what to watch for. It can be wrong, so treat it as a second opinion. The fit table's numbers are worked out exactly. The advisor's judgment isn't.

To use it, select a model and click **Check with AI**. The button reads **Analyzing…** until the answer arrives. A card then appears below the table with:

- A verdict: **Serves well**, **Workable**, **Not recommended** or **Won't fit**.
- **Recommended**: how many nodes, the split, the context and the share.
- **Options it fills in** and **AI also suggested (not applied)**. SparkBox applies only options from a short list. It shows the rest for you to judge.
- **Why**, **Expected speed**, **vLLM support**, **Alternatives**, **Risks** and sometimes **Consider**, a better version of the model.

If the model is already in your Library, click **Load…** while the card is showing. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. The **Load** page opens with the advice filled in and highlighted. The card goes away when you leave the **Hugging Face** tab. If you want the advice on the **Load** page after a download, check again first.

SparkBox sends the advisor the model's facts, the fit table, your nodes' names and memory, what's serving, and the start of the model card. It doesn't send IP addresses or keys. Choose the provider and model in [Settings › Models](open:settings?section=models).

## What it changes

- **Searching and reading** change nothing, and they work in look-only mode.
- **Check with AI** sends the model's facts, your nodes' names and memory, and what's serving, to your AI provider. It needs an API key, and your provider may charge for it. It doesn't work in look-only mode, because the advisor model may be on your cluster. The button stays on, and clicking it shows an error.
- **Download to cell1** adds files to the selected node. It needs Admin mode, and asks you to choose a tool. Installation needs the separate **Install tool and download** confirmation; no image is fetched automatically. To undo it, [delete the model](help:delete-a-model).
- **Load…** only opens the **Load** page, and nothing changes on a node until you press that page's button. On this tab it still asks for Admin mode first, which the **Library** tab's **Load…** doesn't.

## Related How-Tos

- [Find a model that fits](help:find-a-model-that-fits)
- [Download a model](help:download-a-model)
- [Get a Hugging Face token](help:get-a-hugging-face-token)

## Last-known Library copies

If the selected node's Library scan failed or it went offline, the Hub shows **Last-known copy** instead of treating the folder as finished or offering to overwrite it. Reconnect and press **Find models** in [Library](help:models-library#scan-results). A successful fresh scan restores the appropriate Load or Download choice.
