---
id: models-library
title: "Models: Library tab"
type: feature
area: models
summary: "Every model folder on your nodes: its size, which nodes have it, and whether each copy is verified."
keywords: ["library", "verified", "found", "incomplete", "sizes match", "copies", "find models", "cant find my model", "can't find my model", "find my model", "where is my model"]
verified: false
related: [copy-a-model-to-other-nodes, delete-a-model, load-a-model-on-one-node]
---

# Models: Library tab

## What it's for

The **Library** tab lists model folders found on your nodes, with a separate scan result for each node. It shows each model's size, which nodes have it, and whether each copy has been checked. Use it to see what you have, copy a model to more nodes, start a load, or delete a model to free disk space.

## Where to find it

- **Sidebar:** under **Workloads**, click **Models**. The window opens on this tab.
- **Link:** [Open Library](open:models?tab=library)

## What's on the screen


### Toolbar

- **Find models** looks on every online node, in your model folder and in the Hugging Face cache (`~/.cache/huggingface/hub`). It reads **Looking…** while it runs. The list also refreshes when you open the window, change the model folder or node connection, and when a download or copy starts or ends. While this tab is open and Jobs reports a running transfer, it checks again about every ten seconds after the last scan finishes.
- **Load…** opens the [Load page](help:models-load-page) for the selected model. Double-clicking a row does the same thing. A model with no current finished copy has its Load button dimmed.
- **Copy to nodes…** opens a strip for copying the selected model. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes).
- **Delete…** opens a strip for deleting the selected model. It's dimmed while the model is serving. See [Delete a model from a node](help:delete-a-model). In a narrow window, it may be under **More**.

The label **Read-only** at the left of the toolbar means Admin mode is locked. **Look only** means look-only mode is on. Click it to find out how to change it.

### Scan results

A line above the table shows each node's result and the time of its last successful scan:

- **Scanned** means the node answered. A missing copy on this node is shown as **—**.
- **Scan failed** includes the error. Other nodes' successful results still appear.
- **Offline** means SparkBox could not scan that node. **Not scanned** means it has no current result yet.
- **showing last-known copies** means these folders were seen earlier in this app session. Their current contents and verification state are unknown. A node with no successful scan reads **copies unknown**.

Reconnect the node, resolve the reported problem and press **Find models**. A successful scan replaces its old results, even if it now finds no models. Relative folders are reported as absolute paths. An empty folder setting or a broken root link reports an error; correct the folder in **Settings › Models** and scan again. Long errors scroll above the table. Reading the list repairs or deletes nothing. Last-known observations are held in memory only: quitting SparkBox clears them. Changing the model root, replacing a node's connection configuration or switching between Demo and Live also discards the affected observations.

### The model table

| Column | What it shows |
|---|---|
| **Model** | The Hugging Face name, or the folder's name if SparkBox can't tell. **in use** means it's serving now: click it to go to the **Loaded** tab |
| **Size** | The largest observed copy of the folder across nodes, counting what its links point to |
| **Format** | `safetensors` or `gguf` |
| **cell1**, **cell2**… | One column for each node, showing that node's copy |
| **Folder** | Where it lives. The full path is in the tip when you move the pointer over it |

Each node's column shows one of these marks. Move the pointer over a mark to read what it means.

| Mark | What it means |
|---|---|
| **✓ verified** | Checked against Hugging Face's checksums or a source copy's SHA-256 manifest; the tip names which |
| **✓ sizes match** | Copied over the fabric. Every file's size matches the source. Checksums weren't compared |
| **◐ incomplete** | A download or copy stopped partway. **Copy to nodes…** picks up where it stopped, and a new download should too. You can't load it until then |
| **◐ incomplete (2 missing)** | Two links in a Hugging Face cache snapshot point to missing files. Other models stay visible. This snapshot cannot be loaded or used as a copy source |
| **◐ downloading 37%**, **◐ copying 37%** | The marker's live job reported this transfer progress at the last scan; a percentage appears only when the total is known |
| **◐ verifying**, **◐ waiting** | The job is checking files or waiting for its source. Even 100% transferred is not a finished copy. Follow it on the [Jobs tab](help:models-jobs) |
| **◷ last known** | A folder was seen in an earlier successful scan. It cannot be used as a current copy until the node is scanned again |
| **unknown** | This node has no current scan for the folder. It does not mean the folder is absent |
| **● found** | A folder is on disk with no record from SparkBox. It's usually a model you put there by hand. SparkBox hasn't checked it |
| **—** | This node answered its last scan and no copy was found |

A folder counts as a model when it holds a `config.json` file and at least one weights file. A download or copy that stopped partway also shows, with **◐ incomplete**, even before it has any weights.

For a snapshot with missing cache files, select the row to see each affected node's missing-file count and present-byte total. These bytes do not estimate the full download or the space a deletion would free. The shared **Size** column may come from a healthy copy on another node. Restore the files through the tool that manages that cache, then press **Find models** again. Scanning does not repair links, download missing files or remove anything. Permission failures, unreadable folders and missing files outside cache snapshots still report **Scan failed**.

### Copy and delete strip

When you click **Copy to nodes…** or **Delete…**, a strip appears under the toolbar with a box for each node. Only connected nodes with a successful current scan can be used. A last-known copy is never a finished source or a deletion target. If a selected node becomes unavailable, the action is blocked; reconnect and scan again or explicitly uncheck it.

- **Copy to:** nodes without the model are selected. A node with an **incomplete** copy reads "· incomplete" and is selected too, so the copy picks up where it stopped. Nodes that have a finished copy read "· has it" and can't be selected. So can't a node where a copy or download is running, which reads "· copying" or "· downloading". **Copy over the fabric** starts the copy. SparkBox leaves out copies the engine doesn't serve: `original/`, `metal/` and `.pth` files.
- **Delete from:** only nodes with a copy can be selected, and not one where a copy or download is running. **Delete…** asks you to confirm.
- **Cancel** closes the strip.

Copy and Delete repeat a fresh scan before changing files; they never use last-known observations to authorize changes. If Library changes during confirmation, review the action again. Loading still uses its separate fresh prerequisite checks.

### Details

Click a row, and a panel opens on the right:

| Field | What it shows |
|---|---|
| **Size** | The folder's size |
| **Copies** | Which nodes have it, and where. An unfinished or last-known copy says so, such as "cell2 (incomplete)" or "cell2 (last known)" |
| **Format** | The file format |
| **Quantization** | How its numbers are stored, such as `NVFP4`, or "none (bf16)". See [Model formats and quantization](help:formats-and-quantization) |
| **Config** | What its `config.json` says: the design, layers, experts and longest context |
| **In memory** | Whether it's serving now. Click the link to go to the **Loaded** tab |
| **Last used by** | The last load SparkBox saw for this folder, with its state and date |

### Ollama

If a node runs Ollama, a list under the table shows its models, with the node, size and whether they're in memory. This list is read-only, because Ollama keeps its own store. To change it, use the [Ollama](help:ollama) window.

## What it changes

- **Reading the list** changes nothing, and it works in look-only mode.
- **Copy to nodes…** adds files to other nodes. It needs Admin mode. To undo it, delete that copy.
- **Delete…** removes files from nodes. It needs Admin mode, and it can't be undone: you'd have to download or copy the model again. A model in the Hugging Face cache doesn't free its space this way. See [Delete a model from a node](help:delete-a-model).
- **Load…** opens a page. Nothing changes until you press its button.

## Related How-Tos

- [Copy a model to other nodes](help:copy-a-model-to-other-nodes)
- [Delete a model from a node](help:delete-a-model)
- [Load a model on one node](help:load-a-model-on-one-node)
- [Download a model](help:download-a-model)
