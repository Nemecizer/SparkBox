---
id: providers
title: Providers
type: feature
area: ai
summary: "Where SparkBox gets models from: Anthropic, OpenAI, your cluster, and other servers you add."
keywords: [providers, api key, default model, scan cluster, lm studio, plain http, chat models only, add engine]
verified: false
related: [add-a-cloud-provider, chat, keychain-and-secrets, server]
---

# Providers

## What it's for

A **provider** is a source of models. **Chat** and the **Server** offer whatever the providers in this window offer.

There are four kinds:

- **Anthropic (Claude)** and **OpenAI**: cloud companies. You need an API key from each, and you pay for what you use.
- **Your cluster**: engines running on your nodes. They appear by themselves.
- **Other servers**: anything that speaks the OpenAI API, such as LM Studio on another computer. You add these by hand.
- **SparkBox demo**: the simulated model `echo`. Only with Demo data.

Use this window to save a cloud key, to choose which models are offered, and to add a server SparkBox can't find by itself.

## Where to find it

- **Sidebar:** under **AI**, click **Providers**.
- **Command palette:** press <kbd>⌘K</kbd>, type `providers`, and press <kbd>Return</kbd>.
- **Chat window:** on the notice about a missing key, click **Open Providers**.
- **Link:** [Open Providers](open:providers)

## What's on the screen


### Toolbar

| Item | What it does |
|---|---|
| **Scan cluster** | Looks on each node for an engine on its usual port. See below |
| "API keys are stored in the macOS Keychain." | A reminder of where keys go. [Where your passwords and keys go](help:keychain-and-secrets) has more |

You rarely need **Scan cluster**, because engines on your cluster show up by themselves. It's there for a case when SparkBox hasn't found one.

It asks each node from this Mac, on ports `8000`, `30000`, `11434` and `8080`. Anything that answers is saved as a provider. It needs Admin mode, and you must be at home or on the VPN. If it finds nothing, it says "No engines answered. You need to be home or connected through the VPN, with an engine running on its usual port."

### A provider's card

Each card has the same parts, from the top:

| Part | What it does |
|---|---|
| Name and label | The label says what kind it is: **Anthropic API**, **OpenAI API**, **OpenAI-compatible** or **Built in**. With Demo data, a simulated cluster engine says **Simulated engine**. **Found on the cluster** means it's listed only while it's serving |
| Prefix | The name it has in the [Server](help:server), such as `anthropic/…`. Apps put this before a model's name |
| **Enabled** | Turn it off to hide this provider's models from **Chat** and the **Server**. Its key and settings stay |
| Trash icon | Removes a server you added, and deletes its key from the Keychain |
| Address | Where SparkBox sends requests. It's fixed. To change a server's address, remove it and add it again |
| **API key** | See below. Cluster engines have no key field, because SparkBox handles those keys itself |
| **Default model** | The model **Chat** starts with when it picks this provider. A request to the Server must name its own model |
| **Refresh list** | Asks the provider for its models again. Use it when a company adds a model |

### API key

Paste a key into the field and click **Save key**. SparkBox stores it in the Keychain, then fetches the list of models. The field then says "•••••••• (saved)". SparkBox never shows the key again.

**Remove key** deletes it from the Keychain. That doesn't cancel the key at the company. To do that, use the company's website.

Saving or removing a key asks for Touch ID or your Mac password every time.

The field is optional for a server you added. It says "Optional".

### The model list

The bottom of each card lists the provider's models, with a line such as "3 of 4 offered in Chat and the Server". The first offered model, or the one you chose, has a **default** badge.

| Item | What it does |
|---|---|
| **Chat models only** | Hides models that can't hold a conversation: embedding, audio, image, video and moderation models. These show as "not chat". Turn it off if an app needs one through the Server |
| A model's checkbox | Turn it off to stop offering that one model |
| **Filter** | Appears on a list of more than 10 models |

Cloud companies list a lot of models. This list is how you keep **Chat** to the ones you use.

### On the cluster now

These cards are engines SparkBox found on your nodes that this Mac can reach. They need no setup.

A card stays here while its endpoint is **Ready**. If it stops answering, it stays listed for 2 minutes, marked as not answering in **Chat**. If you unload it, it goes at once.

What you change on these cards is remembered if the engine comes back: **Enabled**, **Default model** and the model list.

A model you loaded in SparkBox appears higher up, with your saved providers, until you unload it. An engine that listens on its node only, or on the fabric only, isn't here, because this Mac can't reach it.

### Add another server

The card at the bottom, **Add an engine**, adds a server SparkBox doesn't find by itself. It takes any server that speaks the OpenAI API: vLLM, SGLang, TensorRT-LLM, llama.cpp, NIM, Ollama or LM Studio. The server can be on this Mac, on your network, or on the internet.

| Field | What to enter |
|---|---|
| **Name** | A name you'll recognize, such as `LM Studio on my laptop` |
| **Address** | The server's address, ending in `/v1`, such as `http://192.168.50.20:1234/v1` |
| **Allow plain HTTP on my network** | Appears for a plain `http://` address. See below |
| **API key** | Only if the server asks for one |

Click **Add**. SparkBox asks for Touch ID or your Mac password, adds the server and fetches its models. A message says "Added LM Studio on my laptop with 3 models", with your own count.

The new server appears in **Endpoints** as **Added by hand**, and in the **Cluster** group of the **Chat** model picker.

**Plain HTTP.** A plain `http://` address sends your key and your prompts unencrypted. SparkBox accepts it in these cases only:

| Address | Allowed? |
|---|---|
| `https://` anywhere | Yes |
| `http://` on this Mac (`localhost` or `127.0.0.1`) | Yes |
| `http://` to one of your nodes | Yes |
| `http://` to a private address, such as `192.168.x` or `10.x` | Only with **Allow plain HTTP on my network** turned on |
| `http://` to any other address | No |

Turn that switch on only for a computer on your own network that you trust. If the address doesn't pass, SparkBox says why:

- "192.168.50.20 is on a private network, but plain http sends the API key unencrypted. Tick “Allow plain HTTP on my network” if it's on yours, or use https."
- "Use https for 203.0.113.9: plain http would send the API key and every prompt unencrypted."

Other messages from this form:

- "Give the endpoint a name and an address." One of the two fields is empty.
- "Added, but couldn't list its models: …" The server was added, but it didn't answer. Check that it's running, then click **Refresh list** on its card.

## What it changes

Everything here is a change on this Mac. Nothing changes on your nodes or at any company.

| Action | What it needs | How to undo it |
|---|---|---|
| **Save key**, **Remove key** | Admin mode, and Touch ID or your Mac password every time | Save the key again |
| **Enabled**, **Default model**, **Chat models only**, a model's checkbox | Admin mode, and Touch ID or your Mac password in the last 5 minutes | Change it back |
| **Add** | The same | Click the trash icon on its card |
| Trash icon | The same, and a confirmation. Its key is deleted too | Add it again, with its key |
| **Scan cluster** | Admin mode | Click the trash icon on what it added |
| **Refresh list** | Nothing | Nothing to undo |

If Admin mode is locked, **Scan cluster** shows a small lock, and clicking it asks you to unlock first. [Unlock Admin mode](help:unlock-admin-mode) shows how.

In look-only mode, these actions change nothing.

## Related How-Tos

- [Add a cloud provider](help:add-a-cloud-provider)
- [Chat with a cloud model](help:chat-with-a-cloud-model)
- [Run SparkBox as a server](help:run-sparkbox-as-a-server)

![Providers, using synthetic Demo data](img/public-providers.jpg)
