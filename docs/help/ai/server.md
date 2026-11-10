---
id: server
title: Server
type: feature
area: ai
summary: A local API on this Mac that gives apps one address for your cluster models and cloud models.
keywords: [server, access keys, aliases, origins, requests, test, api, port 8484, connect your apps, sparkbox key]
verified: false
related: [run-sparkbox-as-a-server, create-an-api-key, connect-an-app, endpoints-and-apis, providers]
---

# Server

## What it's for

The **Server** gives the apps on this Mac one address for every model SparkBox can reach: your cluster, Claude, OpenAI, and anything else in [Providers](help:providers).

SparkBox's own [Chat](help:chat) doesn't need it. You need the Server only for another program on this Mac, such as Claude Code, Codex or a script of your own.

An app needs three things to use a model: an address, a key and a model name. Without the Server, each model has its own address and its own key. With it, your apps use one address, and each app has its own SparkBox key. If you stop trusting an app, you revoke its key, and nothing else changes.

The Server speaks the OpenAI Chat Completions, Responses, Completions and Embeddings APIs, which Codex and most apps use. It also speaks the Anthropic Messages API, which Claude Code uses. [Endpoints, ports and API keys](help:endpoints-and-apis) explains those words.

It listens on this Mac only, at `127.0.0.1`. Other computers can't reach it, and SparkBox has no setting to change that. To reach a cluster model from another computer, see [Use a model from another computer](help:use-a-model-from-another-computer).

## Where to find it

- **Sidebar:** under **AI**, click **Server**.
- **Command palette:** press <kbd>⌘K</kbd>, type `server`, and press <kbd>Return</kbd>.
- **Status bar:** while it's running, click **Server**.
- **Link:** [Open Server](open:server)

To start or stop it without opening the window, click the menu bar icon, then click **Start Server** or **Stop Server**. That works only if you confirmed with Touch ID or your Mac password in the last 5 minutes. Otherwise SparkBox refuses, and you can use **Start** or **Stop** in the window instead.

## What's on the screen


### Status and Start

The toolbar shows **Stopped**, or "Running at `http://127.0.0.1:8484`". Its button is **Start** or **Stop**.

While it runs, the status bar has a **Server** item with its port, such as **:8484**. The Server keeps running when you close the SparkBox window to the menu bar. It stops when you quit SparkBox.

### Notices

Two notices can appear under the toolbar.

**In look-only mode:** "Look-only holds inference: turning it on stops the API server, the server doesn't start until it's off, and Chat doesn't send. These settings and keys can't change meanwhile."

**With no key required:** "Anyone on this Mac can use your API keys. The server doesn't require a SparkBox key, so any app or script running here can send requests to Claude, OpenAI and your cluster through it. Turn “Require a SparkBox key” back on below."

### Settings

| Field | What it does | Default |
|---|---|---|
| **Port** | The port the Server listens on, from `1024` to `65535`. Change it only if another app uses `8484`. If you change it while the Server runs, it restarts on the new port | `8484` |
| **Require a SparkBox key** | Every request must carry a SparkBox key, as `Authorization: Bearer …` or `x-api-key` | On |
| **Start with SparkBox** | Starts the Server each time SparkBox opens. It stays stopped while look-only mode is on | Off |
| **Allowed web origins** | Web pages that may call the Server, one per line, such as `http://localhost:3000`. Give the scheme, host and port, with no path. Leave it empty unless a web app you trust needs it | Empty |

These changes wait for **Save settings**, at the bottom right. **Revert** puts back what was saved. The line beside them says "Unsaved changes" or "Fix the aliases to save".

> [!WARNING]
> **Turning off Require a SparkBox key opens your cloud accounts to everything on this Mac.** Any app or script here can then spend your money through the Server. To undo this, turn it back on and click **Save settings**.

The Server also checks every request, and refuses it with a 403 error unless all of these are true:

- It names `127.0.0.1` or `localhost` as its host. This stops a web page from reaching the Server by a trick with web addresses.
- It carries no web origin you haven't allowed. A web page always says where it came from.
- A POST request sends JSON.

A refused request shows in **Requests** as **refused**, with the reason.

### Access keys

The table lists each app's key.

| Column | What it shows |
|---|---|
| **App** | The name you gave the key |
| **Key** | Its first 10 characters. SparkBox shows the whole key only once |
| **Created** | The date you made it |
| Trash icon | Revokes the key. The app stops working until it gets a new one |

Below the table are a name field and **Create key**. [Create an API key for an app](help:create-an-api-key) has the steps.

### Connect your apps

This part gives you text to copy into an app.

| Item | What it does |
|---|---|
| **Model** | The model the text uses. Cluster models come first, with their state |
| **or an alias…** | Uses one of your aliases instead. It appears once you have an alias |
| **Claude Code**, **Codex**, **OpenAI SDK**, **Anthropic SDK**, **curl** | Which app to show the text for |
| **Copy** | Copies the text |

The text contains the Server's address and your model. It never holds your key: it names the variable `SPARKBOX_API_KEY`, as the note below the text says. A **variable** is a named value that a Terminal window keeps in memory.

[Connect an app to SparkBox](help:connect-an-app) shows how to put your key in that variable without saving it in a file or in Terminal's history. As copied, the **curl** text shows your key in the list of running programs while it runs, so [that How-To](help:connect-an-app#curl) changes one line first. SparkBox doesn't install or start these apps. [What's coming](help:whats-coming#coding-apps) says what's planned.

### Test the server

**Send test requests** sends "Reply with the single word OK." to the Server in three API formats, the way an outside app would, using the model above. It works only while the Server is running, and it doesn't need a key.

| Column | What it shows |
|---|---|
| **API** | `chat/completions`, `responses` or `messages` |
| **Result** | **Works**, or **Failed** with its error code, such as **Failed (400)** |
| **Time** | How long that request took |
| **Reply or error** | What the model said, or what went wrong |

A **Failed** row doesn't always mean a fault. Not every model takes every API. For example, a Claude model fails on `responses`, and an OpenAI model fails on `messages`. See [Which model takes which API](help:connect-an-app#limits).

With a cloud model, these requests cost a small amount.

### Aliases

A model's full name has the form `<provider>/<model>`, such as `anthropic/claude-opus-5-5` or `cell1-vllm-8000/llama-3.3-70b`. Apps can ask for the full list at `/v1/models`.

An **alias** is a short name for a model, such as `local`. Apps can use it instead of the full `<provider>/<model>` name. If you later point the alias at another model, your apps keep working with no change. [Give a model a short name](help:give-a-model-a-short-name) shows how.

| Item | What it does |
|---|---|
| **Alias** | The short name. Letters, digits and `.` `_` `:` `-` only |
| **Model** | The model it stands for. The list shows only models you can use now |
| Trash icon | Removes the alias |
| **Add alias** | Adds a row. The first one is named `local`, and it points at the model chosen above |

Aliases wait for **Save settings**. A problem shows under the row:

| Message | What it means |
|---|---|
| "Give it a name" | The **Alias** field is empty |
| "Letters, digits and . _ : - only" | The name has a character that isn't allowed |
| "Used twice" | Two rows have the same name |
| "Pick a model" | **Model** is empty |
| "Unknown target: no provider offers this model now" | You added or changed this alias, and its model isn't offered |
| "cell2-vllm-8000/gemma isn't offered right now" | This saved alias points at a model that has gone, such as after an unload. It's a warning |

A saved alias whose model has gone doesn't stop you saving other settings. Any other problem in the list does, until you fix that row or remove it.

### Requests

This table lists what the Server has handled, newest first.

| Column | What it shows |
|---|---|
| **Time** | When it arrived |
| **App** | The name of the key used. **refused** means a check above stopped it. **(no key)** and **(wrong key)** mean a request came with a missing or unknown key. "SparkBox self-test" is **Send test requests**. "(no key required)" means **Require a SparkBox key** is off |
| **API** | Which API it called |
| **Model** | The model it asked for |
| **Provider** | Who answered |
| **Status** | The result code, with any error beside it. Under `400` means it worked. `401` means a missing or unknown key, with "Missing or unknown SparkBox key" beside it. `403` means a check above refused it. `404` means no provider has that model. `502` means the provider couldn't be reached. `503` means look-only mode held it. Other codes come from the provider |
| **First byte** | How long the reply took to start |
| **Total** | How long the whole reply took |
| **Tokens in / out** | Tokens sent and tokens received |
| **tok/s** | Speed. It shows only for replies long enough to measure |

SparkBox keeps the last 300 requests until it quits. It doesn't record what was asked or answered.

A request with a missing or wrong key appears here with status `401`, and it never holds any part of the key that was sent.

## What it changes

The Server changes nothing on your nodes. Everything in this window is a change on this Mac.

| Action | What it changes | What it needs | How to undo it |
|---|---|---|---|
| **Start**, **Stop** | Opens or closes the port on this Mac. **Stop** cuts off apps using it | Admin mode, and Touch ID or your Mac password in the last 5 minutes | Do the opposite |
| **Save settings** | Saves the port, key rule, startup rule, web origins and aliases | The same | Change it back and save |
| **Create key** | Adds a key to the Keychain. A name that's already in use is refused | Admin mode, and Touch ID or your Mac password every time | Revoke it |
| Revoke | Deletes the key. Apps using it stop working | The same, and a confirmation | Create a new key, and give it to the app |

SparkBox also records **Start**, **Stop**, saves, and created and revoked keys in the [Activity log](help:activity-log), but never the key itself.

**Requests to cloud models cost money.** Every app using the Server spends on your account, and SparkBox doesn't show a total or set a limit.

In look-only mode, the Server stops and these settings can't change. Any request that still arrives gets this reply: "SparkBox is in look-only mode: the API server doesn't send requests to models. Turn it off in SparkBox › Settings › Security & privacy."

## Related How-Tos

- [Run SparkBox as a server](help:run-sparkbox-as-a-server)
- [Create an API key for an app](help:create-an-api-key)
- [Connect an app to SparkBox](help:connect-an-app)
- [Give a model a short name](help:give-a-model-a-short-name)
- [Use a model from another computer](help:use-a-model-from-another-computer), for apps that aren't on this Mac

![Server, using synthetic Demo data](img/public-server.jpg)
