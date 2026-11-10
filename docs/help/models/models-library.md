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

The **Library** tab lists every model folder on your nodes. It shows each model's size, which nodes have it, and whether each copy has been checked. Use it to see what you have, copy a model to more nodes, start a load, or delete a model to free disk space.

## Where to find it

- **Sidebar:** under **Workloads**, click **Models**. The window opens on this tab.
- **Link:** [Open Library](open:models?tab=library)

## What's on the screen


### Toolbar

- **Find models** looks on every online node, in your model folder and in the Hugging Face cache (`~/.cache/huggingface/hub`). It reads **Looking…** while it runs. The list also refreshes when you open the window, and when a download or copy starts or ends.
- **Load…** opens the [Load page](help:models-load-page) for the selected model. Double-clicking a row does the same thing. A model with no finished copy can't load, and that page says so.
- **Copy to nodes…** opens a strip for copying the selected model. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes).
- **Delete…** opens a strip for deleting the selected model. It's dimmed while the model is serving. See [Delete a model from a node](help:delete-a-model). In a narrow window, it may be under **More**.

The label **Read-only** at the left of the toolbar means Admin mode is locked. **Look only** means look-only mode is on. Click it to find out how to change it.

### The model table

| Column | What it shows |
|---|---|
| **Model** | The Hugging Face name, or the folder's name if SparkBox can't tell. **in use** means it's serving now: click it to go to the **Loaded** tab |
| **Size** | The folder's size, counting what its links point to |
| **Format** | `safetensors` or `gguf` |
| **cell1**, **cell2**… | One column for each node, showing that node's copy |
| **Folder** | Where it lives. The full path is in the tip when you move the pointer over it |

Each node's column shows one of these marks. Move the pointer over a mark to read what it means.

| Mark | What it means |
|---|---|
| **✓ verified** | Checked against Hugging Face's checksums or a source copy's SHA-256 manifest; the tip names which |
| **✓ sizes match** | Copied over the fabric. Every file's size matches the source. Checksums weren't compared |
| **◐ incomplete** | A download or copy stopped partway. **Copy to nodes…** picks up where it stopped, and a new download should too. You can't load it until then |
| **◐ downloading**, **◐ copying** | Still running. Follow it on the [Jobs tab](help:models-jobs) |
| **● found** | A folder is on disk with no record from SparkBox. It's usually a model you put there by hand. SparkBox hasn't checked it |
| **—** | This node has no copy |

A folder counts as a model when it holds a `config.json` file and at least one weights file. A download or copy that stopped partway also shows, with **◐ incomplete**, even before it has any weights.

### Copy and delete strip

When you click **Copy to nodes…** or **Delete…**, a strip appears under the toolbar with a box for each node.

- **Copy to:** nodes without the model are selected. A node with an **incomplete** copy reads "· incomplete" and is selected too, so the copy picks up where it stopped. Nodes that have a finished copy read "· has it" and can't be selected. So can't a node where a copy or download is running, which reads "· copying" or "· downloading". **Copy over the fabric** starts the copy. SparkBox leaves out copies the engine doesn't serve: `original/`, `metal/` and `.pth` files.
- **Delete from:** only nodes with a copy can be selected, and not one where a copy or download is running. **Delete…** asks you to confirm.
- **Cancel** closes the strip.

### Details

Click a row, and a panel opens on the right:

| Field | What it shows |
|---|---|
| **Size** | The folder's size |
| **Copies** | Which nodes have it, and where. An unfinished copy says so, such as "cell2 (incomplete)" |
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
