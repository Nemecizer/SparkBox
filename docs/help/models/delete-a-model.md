---
id: delete-a-model
title: "Delete a model from a node"
type: howto
area: models
summary: "Remove a model's files from one or more nodes to free disk space."
keywords: ["delete", "remove", "free disk", "space", "get rid of", "get rid of a model"]
verified: false
related: [models-library, unload-a-model, check-disk-space]
---

# Delete a model from a node

To free disk space, remove a model's folder from one or more nodes on the **Library** tab.

## Before you start

- The model isn't loaded. **Delete…** stays dimmed while it's serving. See [Unload a model](help:unload-a-model).
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the delete is simulated.
- Check where the model lives. Look at the **Folder** column on the **Library** tab. This matters, as the warning below explains.

> [!DANGER]
> **Deleting can't be undone.** There's no Trash for this: the folder is gone from that node. To use the model there again, you have to download or copy it. If this is the only copy on your cluster, you'll have to download it from Hugging Face again, which takes time and internet data. Check the node columns first.

> [!WARNING]
> **Deleting a model from the Hugging Face cache doesn't free its disk space.** If the **Folder** starts `~/.cache/huggingface/hub/models--`, SparkBox removes only a folder of links, and its dialog says so. The real files stay on disk, in a nearby `blobs` folder, and the model leaves the **Library** while still taking up the space. To free the space, follow [Free space from the Hugging Face cache](#free-space-from-the-hugging-face-cache) instead. A folder under `~/models` is fine: deleting it frees its whole size.

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=library) The **Library** tab shows every model folder on your nodes.
2. Click the model's row. Check **Size** and the **Folder** column.
3. Click **Delete…** in the toolbar. If you don't see it, click **More** at the right of the toolbar. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A strip appears under the toolbar, reading **Delete from:**, with a box for each node.
4. Select each node to delete it from. Nodes without a copy can't be selected, and neither can a node where a copy or download is running.
5. Click **Delete…** in the strip. A dialog asks **Delete … from cell2?** and gives the folder and its size. For a model in the Hugging Face cache, the dialog is named like **Remove google/gemma-3-27b-it's links from cell2?** and says it frees almost no disk space. Stop here and use the section below if you want the space back.
6. If you selected more than one node, type the model's name to confirm.
7. Click **Delete**, or **Remove links** for a model in the Hugging Face cache. A message reads "Deleted … from cell2", or "Removed the links to … from cell2" with "Its 60.8 GiB are still on disk."

## Free space from the Hugging Face cache

A model in the Hugging Face cache lives in a folder such as `models--google--gemma-3-27b-it`. To free its space, delete that whole folder with the **Files** window.

> [!DANGER]
> **Files can't tell whether a model is loaded, and this delete can't be undone.** Deleting a model's folder while it's serving can break that model, whoever started it. Do steps 1 and 2 first, on every node you'll delete from. If you need the model again, you'll have to download it again.

1. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) Look for a card that shows this model on that node. If there is one, [unload it](help:unload-a-model) first, or stop here.
2. In the sidebar, under **Workloads**, click **Containers**. [Open Containers](open:containers) Look through that node's rows for this model's name, including containers SparkBox didn't start. If you find one, stop it first. See [Manage containers](help:manage-containers).
3. In the sidebar, under **Tools**, click **Files**. [Open Files](open:files) The **Files** window opens with two panes.
4. In one pane, choose the node at the top left.
5. Click **Hidden files** in the toolbar, so folders that start with a dot appear.
6. Click the path, type `~/.cache/huggingface/hub`, and press <kbd>Return</kbd>.
7. Click the model's folder, the one that starts `models--`.
8. Hold <kbd>⌥</kbd>, so **Move to Trash** changes to **Delete immediately**, and click it. Moving the folder to the Trash doesn't free the space. Check the list in the dialog, then click **Delete**. See [Delete and restore files](help:delete-and-restore-files).
9. Repeat from step 1 for each node that has the model.
10. In **Models**, on the **Library** tab, click **Find models**. The model's row updates.

## What you should see

The model's mark is gone from each node you chose. If no node has it, its row leaves the **Library**. For a folder under `~/models`, the **Free** figure on the node's **Disks** tab goes up. See [Check disk space on a node](help:check-disk-space).

## Why it works

A folder under `~/models` holds the real files, so deleting it frees all of its space. In the Hugging Face cache, the folder SparkBox lists holds only links to a shared `blobs` folder. That's why **Size** shows the full size, but deleting that folder frees almost nothing. Deleting the whole `models--` folder removes the links and the files together. SparkBox refuses a model that's serving, and anything that isn't a model folder inside your model folder or the Hugging Face cache.

## Limits

- **Delete…** can't free the space of a model in the Hugging Face cache. Use the steps above.
- You can't delete an Ollama model here. See [Use Ollama](help:use-ollama).
- A model's parent folder, such as `~/models/Qwen`, stays behind. It's empty and takes no space.

## If something goes wrong

- **Delete… is dimmed.** It's being served: "It's being served; unload it first". Or SparkBox is in look-only mode. Unload it first, or see [Use look-only mode](help:use-look-only-mode).
- **"That model is loaded; unload it first."** A load is starting or serving from this folder. See [Unload a model](help:unload-a-model).
- **"That model isn't in the library any more."** The folder has gone since the list was read. Click **Find models**.
- **"refusing to delete …: not a model folder inside …"** The folder doesn't look like a model, so SparkBox won't remove it.
- **The disk didn't get any freer.** The folder was probably in the Hugging Face cache. Follow [Free space from the Hugging Face cache](#free-space-from-the-hugging-face-cache). The model no longer shows in the **Library**, but its `models--` folder is still there.

## Next steps

- [Check disk space on a node](help:check-disk-space)
- [Find a model that fits](help:find-a-model-that-fits), to choose what to run next
- [Models: Library tab](help:models-library)
