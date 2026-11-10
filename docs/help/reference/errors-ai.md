---
id: errors-ai
title: Chat, Providers and Server messages
type: troubleshooting
area: troubleshooting
summary: Messages from chatting with models, saving provider keys, and running the Server.
keywords: [port in use, origin refused, api key, alias, chat, provider, server, rate limited, not answering]
verified: null
related: [chat, server, providers, add-a-cloud-provider, run-sparkbox-as-a-server]
---

# Chat, Providers and Server messages

This page covers messages from **Chat**, **Providers** and the **Server**. A **provider** is a place Chat can reach models: a model on your cluster, or a cloud service such as Anthropic or OpenAI.

## Chat

### "Add your Anthropic API key under AI › Providers."

**What it means.** You chose a cloud model, and SparkBox has no key for it. You may also see:

- "Add your OpenAI API key under AI › Providers."
- "Add your Anthropic (Claude) API key to chat with it.", above the message box

**What to do.** Follow [Add a cloud provider](help:add-a-cloud-provider).

### "vLLM on cell2 :8000 isn't answering right now. A message may fail until it's back."

**What it means.** The model stopped answering in the last 2 minutes. The message names the engine, the node and the port. SparkBox keeps the model in the picker, marked **Not answering**, in case it returns. After 2 minutes, it leaves the picker.

**What to do.** Check it in [Endpoints](open:endpoints). If it's **Down**, see [Model messages](help:errors-models).

### "The model stopped responding (is cell2 still up?)"

**What it means.** Nothing came from the model for a while, so SparkBox ended the reply with what arrived. For a cloud model, it reads "The model stopped responding: nothing came from Anthropic (Claude) for 300 s."

**What to do.** Check that the node is online. A model can stop if the node ran out of memory.

### "The connection closed before the reply finished (is cell2 still up?)"

**What it means.** The link dropped during the reply. Chat keeps partial output and marks it **interrupted** in saved history. For a cloud model, the message reads "The connection to Anthropic (Claude) closed before the reply finished."

**What to do.** Wait until the connection is back and the model is **Ready**. Use **Regenerate** or **New chat** to ask again. Reconnection does not resume or resend the old answer. The November 7 two-node Qwen VPN retest preserved the partial reply after reopening Chat. Other models and interruption types still need their own tests.

### "The reply is visible but could not be saved."

**What it means.** Chat kept the answer on screen, but could not write its conversation file. The message includes the save error, alongside any connection warning.

**What to do.** Click **Copy** before leaving the conversation. Check free space and access to SparkBox's data folder. Do not assume the answer is saved; reopening cannot recover it.

### "The reply ended without a result. Check the connection before asking again."

**What it means.** Chat did not receive the final reply record. Any text already on screen is kept and marked **interrupted**.

**What to do.** Check the connection and wait for **Ready**. Ask again yourself; SparkBox does not resend automatically.

### "Couldn't reach Anthropic (Claude) (https://api.anthropic.com/v1/messages): …"

**What it means.** This Mac couldn't reach that address. The message names the provider and the address SparkBox tried. For a model on your cluster, it starts "Couldn't reach vLLM on cell2 :8000".

**What to do.** For a cloud provider, check your internet connection. For a cluster model, check that it's **Ready**.

### "Anthropic (Claude) returned 401: …. Check the API key."

**What it means.** The provider refused your key.

**What to do.** Save the key again in [Providers](open:providers).

### "Anthropic (Claude) returned 429: …. Rate limited; try again shortly."

**What it means.** You've sent too much, too quickly, for your account.

**What to do.** Wait a minute, and try again.

### "Anthropic (Claude) returned 404: …. The model or endpoint wasn't found."

**What it means.** That model name doesn't exist at this provider.

**What to do.** Pick another model. Check the provider's address in [Providers](open:providers).

### "Unknown provider"

**What it means.** The provider has been removed since this chat was made.

**What to do.** Pick another model.

### "Claude declined this request."

**What it means.** Claude chose not to answer. Under the reply, you also see "Claude declined this request, so nothing from this turn is sent back next time."

**What to do.** Reword your message, or try another model.

### "OpenAI doesn't give this account reasoning summaries, so this reply comes without one."

**What it means.** A note, not an error. The reply itself is fine.

**What to do.** Nothing.

### "Earlier reasoning (…) was left out: it came from another model."

**What it means.** A note. When you change models partway through a chat, the old model's reasoning stays out of the new request.

**What to do.** Nothing.

### "That conversation was made with Live data."

**What it means.** Chats made with Live data and with Demo data are kept apart.

**What to do.** Switch back to that data source to open it.

## The picker doesn't list a model

### "Reachable from cell3 only (127.0.0.1:11434). Chat and the API server on this Mac can't use it."

**What it means.** The server listens only on its own node, so this Mac can't reach it.

**What to do.** To change that, change where the server listens on the node. A load started in SparkBox with **Listen on** set to **LAN** can be used.

### "Reachable from the other nodes over the fabric only. Chat and the API server on this Mac can't use it."

**What it means.** This Mac isn't on the fabric.

**What to do.** Load it again, with **Listen on** set to **LAN**. See [Ports and addresses](help:ports-reference#listen-on).

## Providers

### "“myserver” isn't a URL like https://host/v1."

**What it means.** The address isn't in the right form.

**What to do.** Type the full address, with `https://` and `/v1`.

### "192.168.50.154 is on a private network, but plain http sends the API key unencrypted. Tick “Allow plain HTTP on my network” if it's on yours, or use https."

**What it means.** The address uses plain HTTP, so anyone on the network could read the key.

**What to do.** If it's your own network, turn on **Allow plain HTTP on my network**. Otherwise, use `https`.

### "Use https for api.example.com: plain http would send the API key and every prompt unencrypted."

**What it means.** SparkBox never allows plain HTTP to a public address.

**What to do.** Use `https`.

### "ftp:// isn't supported: use https:// (or http:// on this Mac or your own network)."

**What it means.** Only web addresses work.

**What to do.** Use `https://`.

### "Provider ids may only use letters, digits, '-' and '_'."

**What it means.** The name has another character.

**What to do.** Use only those.

### "Couldn't reach Local vLLM: …"

**What it means.** SparkBox couldn't get the model list from that server. "Unexpected reply from Local vLLM: …" means something answered, but not as a model server.

**What to do.** Check the address, and that the server is running.

## The Server

### "SparkBox is in look-only mode: the API server doesn't send requests to models. Turn it off in SparkBox › Settings › Security & privacy."

**What it means.** The Server held its requests, because look-only mode is on. It also stops if it was running.

**What to do.** Turn off look-only mode in [Security settings](open:settings?section=security), then start the Server.

### "Port 8484 is already used by another app. Pick a different port."

**What it means.** Another program on this Mac has that port. "Couldn't start on port 8484: …" gives another reason.

**What to do.** Change **Port** in [Server](open:server).

### "Start the server first."

**What it means.** The test needs a running Server.

**What to do.** Click **Start**.

### "Give the key a name, e.g. the app that will use it."

**What it means.** The name field is empty.

**What to do.** Type a name.

### "A key named “Claude Code” already exists. Revoke it first, or use another name."

**What it means.** Each key needs its own name, so SparkBox won't replace an old key with a new one. Nothing changed, and the old key still works.

**What to do.** Use another name, or [revoke the old key](help:create-an-api-key#revoke-a-key) first.

### "Missing or unknown SparkBox key. Create one in SparkBox › AI › Server."

**What it means.** An app connected with no key, or a wrong one. The **Requests** table shows the attempt as **(no key)** or **(wrong key)**, with status `401`.

**What to do.** Follow [Create an API key for an app](help:create-an-api-key).

### "No provider serves model 'gemma'. Use '\<provider\>/\<model\>' from GET /v1/models, or add an alias in SparkBox › AI › Server."

**What it means.** The app asked for a name no provider offers.

**What to do.** Use a name from the list the Server gives, or add an alias. See [Connect an app to SparkBox](help:connect-an-app) and [Give a model a short name](help:give-a-model-a-short-name).

### "The alias “gemma” points at local/gemma, which no provider offers. Pick a model from the list."

**What it means.** An alias is a nickname for a model, and this one points at nothing.

**What to do.** Pick another target.

### "The alias “my/model” may use letters, digits and . _ : - only (no /)."

**What it means.** The name has a character aliases can't use.

**What to do.** Use another name.

### "“example.com” isn't an origin like http\://localhost:3000 (scheme, host and port only)."

**What it means.** **Allowed web origins** needs the form `http://localhost:3000`.

**What to do.** Enter it in that form.

### "Origin https://example.com refused: web pages can't use SparkBox Server unless you allow their origin."

**What it means.** A web page tried to use the Server. This protects you from any website reaching your models.

**What to do.** If you trust that page, add it to **Allowed web origins**.

### "Host “example.com” refused: SparkBox Server only answers 127.0.0.1:8484 or localhost:8484."

**What it means.** The Server answers only on this Mac.

**What to do.** Use one of those two addresses in your app.

### "Content-Type “text/plain” refused: send JSON (application/json)."

**What it means.** The app sent the wrong kind of request. You may also see:

- "Request body isn't valid JSON: …"
- "The request has no model."

**What to do.** Fix the request in your app.

### "Add your Anthropic API key in SparkBox › AI › Providers."

**What it means.** The request goes to a cloud model with no key.

**What to do.** Follow [Add a cloud provider](help:add-a-cloud-provider).

### "Claude can't be used through the OpenAI Responses API yet. Use /v1/messages or /v1/chat/completions."

**What it means.** Each cloud model works through certain routes. You may also see:

- "Claude has no /v1/embeddings API. Use /v1/messages or /v1/chat/completions."
- "OpenAI models can't be used through the Anthropic Messages API yet. Use /v1/chat/completions or /v1/responses."
- "Tool calls to Claude through the Chat Completions API aren't supported yet; use the Anthropic Messages API (/v1/messages) instead."

**What to do.** Set your app to the route the message names.

### "The demo model doesn't make embeddings."

**What it means.** The simulated model can't do this.

**What to do.** Use a real model.

## The AI check for a model

**Check with AI** on the Hugging Face tab uses a provider too. See [Model messages](help:errors-models#finding-and-checking-a-model).
