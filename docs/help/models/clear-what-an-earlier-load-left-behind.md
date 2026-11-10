---
id: clear-what-an-earlier-load-left-behind
title: "Clear what an earlier load left behind"
type: howto
area: models
summary: "Remove containers an earlier load left on a node, so that a new load can start there."
keywords: ["leftovers", "remove leftovers", "left behind", "containers left", "earlier load", "old containers", "clean up after a load", "clean up a node", "ray worker", "blocked by a container"]
verified: false
related: [load-a-model-on-one-node, load-a-model-on-several-nodes, fix-a-load-that-failed, unload-a-model, manage-containers]
---

# Clear what an earlier load left behind

To free a node for a new load, remove the containers an earlier load left on it from **Before it starts** on the **Load** page.

A load can leave containers behind if a node was out of reach while it failed, or look-only mode held the cleanup. SparkBox won't start a new load beside them, because they can hold memory, a port or both.

## Before you start

- To read what's left, you don't need Admin mode. Reading also works in look-only mode.
- To remove anything, use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, this reads "Available with Live data".
- If the load is still listed on the **Loaded** tab or has a card in **Endpoints**, [unload it](help:unload-a-model) instead. Unload does the same job and checks that the memory came back.
- Removing leftovers doesn't touch the model's files or the image.

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=library) The **Models** window opens on the **Library** tab.
2. Click the model you want to load, then click **Load…**. The **Load** page opens.
3. Under **Nodes**, select the node or nodes you want.
4. Scroll to **Before it starts**. A red note reads like "cell2 has containers left from an earlier load (sparkbox-m1a2b3-r1). Remove them first."
5. Find the node's block. **Left by an earlier load** lists each container and its status.
6. Click **Remove leftovers…**. A dialog named like **Remove what an earlier load left on cell2?** opens.
7. Read the dialog. It names the node and each container.
8. Click **Remove leftovers**. If Admin mode is locked, macOS then asks for Touch ID or your Mac password. Confirm it. SparkBox stops and removes those containers, with the launch files and key it made for them. [Tasks](open:tasks) shows the work, as "Remove what earlier loads left on cell2".
9. Wait for **Before it starts** to read the nodes again. To read them now, click **Check again**.

### What SparkBox won't remove

Some things block a load and can't be removed here. Each has its own message.

| Message | What to do |
|---|---|
| "cell2 has a Ray worker still looking for its head (glm53). SparkBox didn't start it; remove it in Containers." | [Remove it in Containers](help:manage-containers) if it's yours |
| "cell2 has gemma4:latest in memory in Ollama, and unified memory is shared. Unload it there first." | Unload it in the [Ollama](open:ollama) window. See [Use Ollama](help:use-ollama) |

**Left by an earlier load** also lists the other SparkBox build's containers, with "Another build's containers are shown, never removed". Remove those from the build that started them. Containers with no SparkBox label aren't listed at all.

## What you should see

The node's **Left by an earlier load** row reads "nothing", and the red note has gone. In [Containers](open:containers), the names are gone too.

## Why it works

SparkBox labels every container it starts with the load's id and with which build started it. It treats a container as left over only if it has this build's label and no active load owns it. So **Remove leftovers…** can't reach a running load, another build's containers or anything SparkBox didn't start. It removes them by id, and only on the nodes it names.

## If something goes wrong

- **Remove leftovers… is dimmed.** SparkBox is in look-only mode. See [Use look-only mode](help:use-look-only-mode).
- **"gemma is loading or serving: unload it instead."** That load came back to life, or started, since SparkBox last read the node. [Unload it](help:unload-a-model).
- **The task fails with a line that starts "cell2:".** That node refused or stopped answering, and the rest of the line says why. Check that it's online, then try again. If the containers stay, remove them in [Containers](help:manage-containers).
- **Memory stays used after the containers have gone.** Look at `/dev/shm`. See [Remove what's left in /dev/shm](help:unload-a-model#remove-whats-left-in-devshm).

For other messages, see [Model messages](help:errors-models).

## Next steps

- [Load a model on one node](help:load-a-model-on-one-node)
- [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes)
- [Fix a load that failed](help:fix-a-load-that-failed)
