---
id: models-window
title: "Models"
type: feature
area: models
summary: "Where you find, download, copy, load, benchmark and check models."
keywords: ["models", "tabs", "history", "recovery copies", "older reports", "unload history"]
verified: false
related: [your-first-model, run-your-first-real-model, find-a-model-that-fits, download-a-model, load-a-model-on-one-node, models-library, models-hugging-face, models-load-page]
---

# Models

## What it's for

**Models** is where you do everything with a model's files and its loads. You find a model on Hugging Face, download it, copy it between nodes, load it, measure it and check on it. It works on the four nodes of your cluster from one window.

New to models? Start with [Rehearse running a model in Demo](help:your-first-model), then [Run your first real model](help:run-your-first-real-model), or read [Models, engines, memory and context in plain words](help:models-in-plain-words).

## Where to find it

- **Sidebar:** under **Workloads**, click **Models**. Right-click it for **Open Models** and **Load a model…**, which opens the **Load** page.
- **Command palette:** press <kbd>⌘K</kbd> and type `models`. **Load model…** opens the **Load** page.
- **Other windows:** **Load a model…** in the **Endpoints** window, and links from the Dashboard.
- **Link:** [Open Models](open:models)

## What's on the screen


### Tabs

The window has seven tabs, in the order you'd usually use them. A number on a tab shows what's there now.

| Tab | What it's for | The number shows |
|---|---|---|
| [Library](help:models-library) | The model folders on your nodes. Copy, delete and start a load | Model folders |
| [Hugging Face](help:models-hugging-face) | Search, read the fit table and download | |
| [Loaded](help:models-loaded) | What's serving now, what failed and the engine's log | Models serving or loading |
| [History](help:models-window#history) | Saved recovery copies and older unload reports on this Mac | |
| [Benchmarks](help:models-benchmarks) | Measure a model's speed | |
| [Jobs](help:models-jobs) | Downloads and copies running on your nodes | Jobs running |
| [Prepare cluster](help:models-prepare-cluster) | Check that every node can run a model | |

### History

[Open History](open:models?tab=history)

**History** reads saved copies on this Mac. You can use it with Admin mode locked or in look-only mode. It makes no connection to a node.


- **Refresh copies** reads the `deployment-history` folder again. Choose a file in **Source**. Files are listed by filename, not by date. The same load can appear in several copies; each keeps its own version.
- **Open report file…** opens a JSON file you choose on this Mac. It accepts saved deployment lists and older unload-report lists. The file stays open only in this view. **Close file**, leaving this tab or switching between Live and Demo clears it from the view. It does not delete or import the file.
- **File details** shows the file's SHA-256 checksum and byte count. A checksum identifies the exact contents. Recovery filenames must match their contents. An opened file has no verified original location or completeness.
- **Find** searches load IDs, models, nodes, saved states and report text. **Unload warnings only** shows reports whose saved result was a warning. The count shows visible records out of the file's total: **1 of 1 record** or **1 of 10 records**.
- Each record shows its saved state, nodes and recorded times. **Recorded node checks** shows the original container, memory, port and shared-memory results. An older report may lack a model name, load settings and start time; these stay marked as missing.

**Saved state: ready** is a past observation. **Current state: not checked** means this view has not checked the nodes. A recorded pass or warning keeps its original meaning, even after later repairs. Use [Loaded](help:models-loaded) or [Endpoints](help:endpoints) for current serving information.

A file warning explains any records that could not be read. SparkBox does not repair, delete, merge or restore them. The viewer accepts up to 4 MiB and 256 records per file. The recovery folder has a 128-entry limit and a 16 MiB total reading limit. If a folder exceeds a limit, open an individual report instead. An altered recovery copy is not shown as verified; opening it separately shows only its present contents.

Demo shows made-up recovery and older-report samples. It never reads your saved copies, and **Open report file…** is dimmed. Choose Live to view your local files; this viewer still makes no node connection.


A later check confirmed **1 record**, **1 of 1 record** and **1 of 10 records** in the Mac app.

### Load page

The [Load page](help:models-load-page) isn't a tab. It opens over the tabs when you click **Load…** or **Load a model…**, and **Back** returns you to them.

### Settings link

**Settings**, at the right of the tabs, opens [Settings: Models](help:settings-models). That's where you set the model folder, the engine images, your Hugging Face access token and the AI advisor.

## What it changes

| You do | What changes | What it needs | How to undo it |
|---|---|---|---|
| View **History** or open an older report | Nothing; reads local files only | Nothing | |
| Search, read a fit table, read the **Library** | Nothing | Nothing | |
| **Check with AI** | Sends a model's facts to your AI provider, who may charge for it | An API key | |
| **Download to cell1** | Adds files to the selected download node | Admin mode | [Delete a model from a node](help:delete-a-model) |
| **Copy to nodes…** | Adds files to other nodes | Admin mode | [Delete the copy](help:delete-a-model) |
| **Delete…** | Removes files from a node | Admin mode | Download or copy it again |
| **Load on 1 node** | Starts containers and reserves memory | Admin mode | [Unload a model](help:unload-a-model) |
| **Check nodes** | Starts short test containers, which remove themselves | Admin mode | Nothing to undo |
| **Run benchmark** | Keeps a model busy for minutes | Admin mode | **Stop** |

Admin mode asks for Touch ID or your Mac password with **Live** data. In look-only mode, all of these changes are off.

## Related How-Tos

- [Rehearse running a model in Demo](help:your-first-model)
- [Run your first real model](help:run-your-first-real-model)
- [Find a model that fits](help:find-a-model-that-fits)
- [Download a model](help:download-a-model)
- [Load a model on one node](help:load-a-model-on-one-node)
- [Tell whether a model is working](help:tell-if-a-model-is-working)
- [Unload a model](help:unload-a-model)

After reopening, the Loaded tab shows recovery notes while SparkBox checks saved loads. See [After SparkBox quits or loses its connection](help:fix-a-load-that-failed#after-sparkbox-quits-or-loses-its-connection) for waiting, cleanup and ownership decisions.

![Models, using synthetic Demo data](img/public-models-window.jpg)
