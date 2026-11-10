---
id: restart-or-change-a-model
title: "Restart a model or change its settings"
type: howto
area: models
summary: "Restart a loaded model, or edit its settings and load it again."
keywords: ["restart model", "edit and reload", "reload", "change settings"]
verified: false
related: [choose-load-settings, unload-a-model]
---

# Restart a model or change its settings

To get a stuck model going again, click **Restart…** on its card. To change how it runs, click **Edit & reload…**.

**Restart…** stops the model and starts it again with the same settings and the same API key. **Edit & reload…** unloads it and loads it again with new settings and a new API key.

## Before you start

- The model was loaded by SparkBox. For a server SparkBox didn't start, **Restart…** restarts its containers, and **Edit & reload…** isn't offered.
- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the restart is simulated.
- **The model goes offline.** Apps using it get errors until it's **Ready** again, which can take as long as a load.
- **Keep SparkBox open and this Mac awake until the model reads Ready.** SparkBox runs the restart from this Mac. [Keep this Mac awake during long work](help:keep-this-mac-awake) shows how.

## Steps

### Restart with the same settings

1. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) You see one card for each model server.
2. On your model's card, click **Restart…**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
3. Read the dialog, **Restart …?** It says what will stop.
4. Click **Restart**. For a model on several nodes, the button reads **Restart … in order**. For a SparkBox vLLM load, choose **Restart and allow cache drop** instead to permit a page-cache drop if this attempt needs one.
5. Wait. The card reads **Paused** while the model stops and starts, then **Loading**, then **Ready**. [Tasks](open:tasks) shows each step and ends when the model answers.

The page cache is file data Linux keeps in memory. Dropping it uses sudo; no files are deleted, and Linux refills it. **Restart and allow cache drop** keeps the API key and adds permission for this attempt only. **Retry** in **Tasks** asks again. Ordinary **Restart** still uses any permission saved when the model was loaded.

You can also right-click the model's row on the **Loaded** tab, or use the command palette: press <kbd>⌘K</kbd> and type `restart`.

If SparkBox refuses the first stop, the restart task fails but the load keeps its previous state. Its containers keep running. Read the task's error before trying again. A failure after the stop may leave the load **Failed**.

### Change its settings

1. On your model's card in **Endpoints**, click **Edit & reload…**. The **Models** window opens the **Load** page, with the heading **Edit … and reload** and the load's current settings filled in.
2. Change what you need. The **Memory plan** treats the old load as gone, because its memory comes back first. **GPU memory share** stays as this load had it, with a button beside it if SparkBox would size it differently. See [Choose load settings](help:choose-load-settings).
3. Click **Reload**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. Read the dialog, **Reload … with these settings?** It warns that the model goes offline, and that apps using it get errors meanwhile.
5. Click **Reload**. SparkBox unloads the model and checks what came back, then loads it again.
6. Wait for **Ready**, as for any load.

## What you should see

- After **Restart…**: the card reads **Ready**. The task in **Tasks** ends with a line such as "Verified: gemma-3-27b-it answered after 2m 10s".
- After **Edit & reload…**: the card reads **Ready**, and the numbers on the card match your new settings.

## Why it works

A restart isn't done until the model answers. SparkBox waits for a fresh test token, sent after the restart began, so an old answer can't count. A model on several nodes restarts in order: the head node stops, then the workers, SparkBox checks the nodes again, the workers start, and the head node starts last. **Edit & reload…** is a real unload and a real load, which is why it makes a new API key. Chat and the Server pick up the new key by themselves.

## Limits

- For a vLLM load, **Restart…** checks the nodes before it stops anything, so a restart that couldn't come back leaves the model running. The task shows this as "Check the nodes first".
- Without saved permission or **Restart and allow cache drop**, a memory shortfall refuses the restart with "Nothing was stopped." SparkBox reads memory again after stopping the model. A shortfall or refused drop then leaves the model down; read the task's error before retrying.
- The extra permission expires when the attempt ends, even if it fails. It does not change **Keep running** or future restarts. Bulk container restarts use only the load's saved permission. SGLang and servers SparkBox didn't start do not offer this choice.
- **Edit & reload…** changes the API key. Any app you set up by hand with the old key needs the new one. Apps that go through the Server keep working.
- SparkBox waits up to 90 minutes for a restart to answer, the same as for a load. See [Tell whether a model is working](help:tell-if-a-model-is-working).
- For a server SparkBox didn't start, the order differs: the workers stop, the head node stops and starts, then the workers start and join it. SparkBox can't tell how that server was started, so it keeps the order a server that uses Ray expects. It waits 25 minutes for that server to answer.

## If something goes wrong

- **"… is already restarting: Tasks follows it."** A restart is under way. Wait for it.
- **"Its API key is gone from the Keychain; load it again instead."** SparkBox can't restart with the same key. Use **Edit & reload…**.
- **"… is down: …"** in **Tasks**. The model didn't come back. The rest of the message says why. See [Fix a load that failed](help:fix-a-load-that-failed).
- **"… didn't come back"**, with a restart button in the message. A restart didn't finish well. Click the button to try again, or read the log first.
- **The restart fails after the model stopped.** SparkBox saves the logs and removes what it started, the same as for a failed load. See [Fix a load that failed](help:fix-a-load-that-failed).
- **The load fails after Reload.** The old load is already gone at this point. Go to [Fix a load that failed](help:fix-a-load-that-failed), change the settings, and load again.

## Next steps

- [Tell whether a model is working](help:tell-if-a-model-is-working)
- [Choose load settings](help:choose-load-settings)
- [Unload a model](help:unload-a-model)
