---
id: unload-a-model
title: "Unload a model"
type: howto
area: models
summary: "Stop a model and get its memory back, then check that nothing is left behind."
keywords: ["unload", "stop model", "stop a model", "stop a load", "cancel a load", "cancel loading", "stop loading", "free memory", "shut down model", "turn off a model", "get rid of", "get rid of a model"]
verified: false
related: [delete-a-model, tell-if-a-model-is-working]
---

# Unload a model

To stop a model and get its memory back, click **Unload…** on its card, then read the report to check that nothing was left behind.

Unloading doesn't touch the model's files. They stay in your **Library**, ready to load again.

## Before you start

- The model was loaded by SparkBox. For a server SparkBox didn't start, the card offers **Stop containers…** instead. See [Endpoints](help:endpoints).
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the unload is simulated.
- **Apps using this model stop working.** Check that nobody's using it.
- To stop a load before it's **Ready**, follow these same steps. **Unload…** works at any stage. Quitting SparkBox doesn't stop a load.
- The engine gets up to 30 seconds to stop, and SparkBox then waits up to 60 seconds for the memory to come back.

## Steps

1. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) You see one card for each model server.


2. On your model's card, click **Unload…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
3. Read the dialog, **Unload …?** It names the nodes, and says that apps using the address stop working.
4. Click **Unload**. A message reads "Unloading …".
5. Open [Tasks](open:tasks) to watch it. The task has two steps: "Stopping (docker stop -t 30, then docker rm)", then "Checking memory, port and /dev/shm". On several nodes, SparkBox stops the head node first, then the workers.
6. When the task ends, read its summary. The next section shows what you want to see.

You can also right-click the model's row on the **Loaded** tab, or use the command palette: press <kbd>⌘K</kbd> and type `unload`.

## What you should see

The task ends with a summary such as:

"Unloaded from cell2 · memory back (116.5 GiB available, baseline 116.7) · port 8000 closed · /dev/shm: nothing new"

That says three things came back as they were before the load:

- **Memory.** The node's available memory is within 2 GiB of what it was.
- **The port.** Nothing is listening on it any more.
- **`/dev/shm`.** This area of shared memory has nothing new in it.

If you stopped a load before its container started, the summary begins "No containers of … from this SparkBox were running; it's marked stopped". That's normal.

The model's card leaves **Endpoints**, and the model leaves Chat's list and the Server. On the **Loaded** tab, a note says it "was unloaded", with the same summary. To clear that note, click **Forget**, then confirm **Forget note**. SparkBox keeps a recovery copy on this Mac; it never restores that copy as an active load. Read it in **Models › History**. You can also open an older report file there without importing it. See [Saved model history](help:models-window#history).

New vLLM loads keep their launcher alive while the engine's child processes finish cleanup. Already-running loads retain their original launcher. If a report names leftover files or processes, review it before making any separate cleanup change. SparkBox does not automatically remove arbitrary shared-memory files. Old warnings stay unchanged; a later clean result does not rewrite them.

## Why it works

SparkBox tags every container it starts with the load's id. Unload stops exactly those containers, on every node, head node first, so another load on other nodes keeps running. Within the same 30-second stop limit, the launcher lets the engine and its cleanup workers finish; SparkBox then checks shared memory instead of deleting unknown files. SparkBox also removes the load's launch files from the node, and its API key from the node and the Keychain, so a later load gets a new key. It leaves vLLM's cache folders in place, because they make the next load faster.

## Limits

- This change applies to new vLLM loads. It does not change a running container or the separate SGLang launcher.
- A forced stop, crashed cleanup worker or shutdown that takes too long can still leave files. The report names them, and the existing warning remains. There is no automatic removal by filename or size.

## If something goes wrong

- **The task warns that deployment history could not be saved.** Cleanup and its health checks still run. The report stays visible for this session, but the main saved list may be older. Keep a copy of the report and use the storage banner's **Show in Finder** to preserve the files for review. A recovery copy is kept when storage allows. Do not replace the current list with an old copy or assume reopening will repair it.
- **The task ends with a warning: "Unloaded, but something didn't come back; see the report."** The model is stopped, and something didn't return. Read the summary:
  - **"memory not back after 60 s (…)"** Something else may have taken memory, or the engine is slow to let go. Look again in a minute on the [Dashboard](open:dashboard).
  - **"port 8000 still taken (…)"** Another program is using the port. See [Workloads](help:workloads).
  - **"/dev/shm: new since the load: …; removing them needs sudo and your OK (not done)"** The engine left shared memory behind, and the report gives its name and size. It's harmless, but it keeps that memory in use until it's gone, so you can leave it until you need the memory. The safest way to clear it is to [restart the node](help:restart-a-node) when nothing else needs that node. To remove it by hand, see [Remove what's left in /dev/shm](#remove-whats-left-in-devshm).
- **"Couldn't check …'s containers on cell2; nothing was stopped."** SparkBox couldn't ask the node. Check that it's online, then try again.
- **"Couldn't reach cell2 to stop its containers; they may still be running."** The node stopped answering partway. When it's back, unload again.
- **"Its containers on cell2 are still there; it isn't marked stopped."** Something held on to the container. Try again. If it's still there, see [Manage containers](help:manage-containers).
- **"…'s containers on cell2 are managed by the other SparkBox; nothing was stopped."** The test build started this load, not this app. Unload it from there.
- **"…'s containers on cell2 have no SparkBox instance label, so whose they are can't be told; nothing was stopped. Remove them in Containers."** An older SparkBox started them. See [Manage containers](help:manage-containers).
- **Unload… is dimmed.** SparkBox is in look-only mode. See [Use look-only mode](help:use-look-only-mode).

### Remove what's left in /dev/shm

> [!DANGER]
> **This delete can't be undone, and a wrong name can break a running model.** Another model on the same node can be using a file in `/dev/shm`. Do step 1 first. If you're not sure, leave the file alone and restart the node when it's free.

1. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) Check that no other card runs on that node. If one does, stop here.
2. Open a [terminal](help:open-a-terminal) on that node.
3. Type `sudo rm /dev/shm/<name>`, with the exact name from the report in place of `<name>`, without the angle brackets or the size. For `psm_5f3c2a91 (2.0 GiB)`, you'd run `sudo rm /dev/shm/psm_5f3c2a91`.
4. Press <kbd>Return</kbd>. sudo asks for your cluster password, and [nothing appears as you type it](help:open-a-terminal#run-a-command-with-sudo).

## Next steps

- [Delete a model from a node](help:delete-a-model), to free its disk space too
- [Load a model on one node](help:load-a-model-on-one-node), to load another
- [Check disk space on a node](help:check-disk-space)
