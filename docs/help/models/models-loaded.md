---
id: models-loaded
title: "Models: Loaded tab"
type: feature
area: models
summary: "What's serving right now, loads that failed or stopped, and the engine's log."
keywords: ["loaded", "serving", "failed", "forget", "log"]
verified: false
related: [restart-or-change-a-model, unload-a-model, fix-a-load-that-failed, tell-if-a-model-is-working, endpoints]
---

# Models: Loaded tab

## What it's for

The **Loaded** tab shows what's serving on your cluster right now, in one short list. It also shows recent failures, loads that ended, and the engine's log. For the full details of a model, use its card in [Endpoints](help:endpoints).

## Where to find it

- **Sidebar:** under **Workloads**, click **Models**, then click the **Loaded** tab. The number on the tab counts models serving or loading.
- **After a load:** SparkBox moves you here when you click **Load on 1 node**.
- **Library tab:** click **in use** beside a model.
- **Dashboard:** click a failed run's notice.
- **Link:** [Open Loaded](open:models?tab=loaded)

## What's on the screen


### Toolbar

- **Benchmark…** opens the **Benchmarks** tab. It's dimmed until a vLLM model is **Ready**.
- **Load a model…** opens the [Load page](help:models-load-page).

### Serving now

There's one row for each model server, whoever started it. SparkBox finds servers on your nodes by itself.

| Column | What it shows |
|---|---|
| **Model** | The name apps use. For Ollama, the server and the models in memory |
| **Engine** | **vLLM**, **SGLang**, **Ollama** or another engine |
| **Nodes** | The nodes it runs on, head node first |
| **State** | **Ready**, **Loading**, **Down** and others, with a reason where there is one. See [Tell whether a model is working](help:tell-if-a-model-is-working) |
| **Generate** | Tokens per second while it's writing. **—** when it isn't **Ready** |

**Open in Endpoints** opens that model's card. Double-clicking the row does the same thing. Right-click a row for its other actions, such as **Chat with this**, **Restart…** and **Unload…**.

If nothing is running, the table says so, and suggests how to start something.

### Failed row

If a run's containers failed in the last 7 days, it has a row with a red **Failed** label and how it ended, such as "Exited 1 · 2 d ago". SparkBox finds these whoever started the run. Only the most recent failure gets a row. A vLLM load that fails while it's starting has no **Failed** row, because SparkBox removes its containers. Its note below gives the cause. A load that fails after it read **Ready** keeps its containers, so it has this row.

**Explain…** opens a read-only dialog with the likely cause, the decisive lines from the log and a suggested fix. See [Fix a load that failed](help:fix-a-load-that-failed).

### Fabric warning

A load across nodes that's starting or serving has an amber note here if its log says its traffic left the fabric, such as "gemma: It isn't using the RDMA fabric: its message queue is on 192.168.50.51, which isn't a fabric address." The load keeps going. [Model messages](help:errors-models) says what to do.

### Failed and stopped loads

Under the table, each load of yours that failed or was unloaded has a note. Its age counts from the failure or unload, when that time was recorded. A failed load's note gives the cause, and "Logs on this Mac:" with the folder that holds each node's log. If SparkBox couldn't clean up on a node, it adds "Left on cell2: Forget unloads them first." An unloaded load's note gives the unload report, starting "Last unload:". See [Unload a model](help:unload-a-model).

**Forget** asks before taking the note off the list. Choose **Forget note** to confirm, or **Cancel** to keep it. SparkBox keeps a recovery copy of the saved deployment and unload report on this Mac. If the load's containers are still on a node, the button reads **Forget…**. Confirm **Unload and forget** to unload them first. Demo removes only the simulated note.

Recovery copies stay in the `deployment-history` folder beside `deployments.json`. Open the **History** tab to read them, or use **Open report file…** there to inspect older unload reports. See [Saved model history](help:models-window#history). They are records for review, not active loads. SparkBox never restores them automatically. A successful Forget of a live note also appears in the Activity log. See [Where SparkBox keeps things](help:files-and-folders#history-and-records).


If saved history changes outside this SparkBox session, SparkBox refuses to overwrite it. A storage banner appears, and new loads and Forget need a successful save. Unload can still clean up an owned model; its task warns if the report could not be saved. Keep the report visible and use **Show in Finder** to preserve the files for review. Do not copy an old saved load over the current list.

### vLLM log

When a load is starting, serving or failed, the last lines of its engine log appear under the notes. The heading reads **vLLM log** or **SGLang log**, followed by the model and its stage, such as "Compiling: FlashInfer kernels". For a failed load, it shows each node's last lines under a line such as "── rank 1, worker on cell2, exit 1 ──". If several loads have logs, a list picks which one to show.

For the whole log, click **Logs** on the model's card in **Endpoints**.

### Replica groups and maintenance

Replicas appear as independent endpoints under their shared group. **Unload group…** reviews the group and unloads each replica through its usual ownership checks; a failure is reported without undoing earlier unloads.

An interrupted first startup can offer **Resume missing ranks…**. Existing ranks must still be running with the saved image and ownership. Resume starts only absent ranks. It refuses loads that already served, stopped ranks, changed ownership or missing credentials. Use Restart in order or Unload for those cases.

**Paused for rolling maintenance** retains the load's settings, key and node claims. Use **Software Updates** to resume the saved plan or restore its models. Ordinary restart and Forget are held while it is paused. Unload remains an explicit way to stop keeping the load.

## What it changes

- **The list and the log** only read. They work in look-only mode.
- **Explain…** only reads the run's containers and log, and it changes nothing.
- **Forget** changes SparkBox's list. It needs Admin mode, and it's off in look-only mode. If containers remain, it unloads them, which stops them. It doesn't delete model files.
- **Right-click actions** such as **Restart…** and **Unload…** change the cluster, and they ask first.

This tab shows what's on your nodes. A model listed here keeps running if SparkBox closes. See [Common questions about models](help:model-questions).

## Related How-Tos

- [Unload a model](help:unload-a-model)
- [Fix a load that failed](help:fix-a-load-that-failed)
- [Tell whether a model is working](help:tell-if-a-model-is-working)
- [Restart a model or change its settings](help:restart-or-change-a-model)

After reopening, recovery notes explain which saved loads are waiting for fresh reports, need cleanup, or need an owner’s review. See [Recover a saved load](help:fix-a-load-that-failed#after-sparkbox-quits-or-loses-its-connection).

### Optional speculative counters

vLLM images that publish speculative-decoding counters also expose their cumulative draft, draft-token and accepted-token counts in the endpoint metrics. Missing counters stay unknown; SparkBox does not turn speculative decoding on. These counters are retained for inspection, without a new chart or alert. Real MTP performance and acceptance still need a serving test.
