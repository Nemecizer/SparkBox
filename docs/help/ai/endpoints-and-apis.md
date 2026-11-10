---
id: endpoints-and-apis
title: Endpoints, ports and API keys
type: concept
area: ai
summary: "How apps talk to a model: an address, a port, a key, and the OpenAI and Anthropic ways of asking."
keywords: [api, endpoint, port, openai, anthropic, url, 127.0.0.1, localhost, base url]
verified: null
related: [server, endpoints, connect-an-app, keychain-and-secrets, providers, use-a-model-from-another-computer]
---

# Endpoints, ports and API keys

## In short

An **endpoint** is a running model server you can talk to. An app reaches it through an address, which includes a port, and it often has to show a key. It then asks in one of two common formats: OpenAI's or Anthropic's.

Those four things decide nearly everything about connecting an app: where the model is, who can reach it, who may use it, and how to ask.

## How it works

### The address

An app finds an endpoint by its address, also called its URL. Here's a real one from the **Endpoints** window:

```
http://192.168.50.51:8000/v1
```

| Part | Example | What it means |
|---|---|---|
| Scheme | `http://` | How the two ends talk. `https://` is encrypted. `http://` isn't |
| Host | `192.168.50.51` | Which computer. This one is `cell1` on your home network |
| Port | `8000` | Which program on that computer |
| Path | `/v1` | Where the API starts. Apps add to it, such as `/v1/chat/completions` |

### The port

A **port** is a numbered door on a computer that a program answers on. One computer can run many programs, and each one listens at its own door. These are the usual ones:

| Port | What usually listens there |
|---|---|
| `8000` | vLLM, TensorRT-LLM or NIM |
| `30000` | SGLang |
| `11434` | Ollama |
| `8080` | llama.cpp |
| `8484` | The Server, on this Mac |

### Who can reach it

The same port can be open to everyone on your network, or to nobody but the computer itself. That depends on which address the program listens on.

| It listens on | Who can reach it | Where you see this |
|---|---|---|
| A LAN address, such as `192.168.50.51` | Anything on your home network, or on a VPN into it | A normal card in **Endpoints**. **LAN only** beside the address |
| All addresses | The same, and the other nodes over the fabric | A normal card in **Endpoints** |
| A fabric address, such as `192.168.100.2` | Only the other nodes | The card says "Reachable from the other nodes over the fabric only. Chat and the API server on this Mac can't use it." |
| `127.0.0.1` | Only that same computer | The card says "Reachable from cell3 only (127.0.0.1:11434). Chat and the API server on this Mac can't use it." |

`127.0.0.1`, also called `localhost`, always means "this same computer". So the same address means different things in different places. On a card for `cell3`, it means `cell3`. In the **Server** window, it means this Mac.

### The key

An **API key** is a long secret that proves a request is allowed. You meet three kinds in SparkBox, and they don't work in place of each other.

| Kind | Looks like | Where it comes from | What it unlocks |
|---|---|---|---|
| Cloud provider key | Set by Anthropic or OpenAI | Your account with that company. You save it in [Providers](help:providers) | That company's models. Using it costs money |
| Load key | `sbk-` and then random characters | SparkBox makes one when a load has **API key** turned on | That one model on your cluster |
| SparkBox key | `sbx-` and then random characters | You make one for each app in [Server](help:server) | The Server on this Mac, and only that |

All three stay in the [Keychain](help:keychain-and-secrets). A load's key is also on its head node, in a file only your account can read, because the engine needs it there. An app that goes through the Server only ever holds a SparkBox key. SparkBox adds the real key when it passes the request on.

### The API

An **API** is the agreed way for one program to ask another for something. It says which address to use and how to lay out the request. For models, two styles matter:

- **OpenAI style.** Most apps and engines use it. **Chat Completions** is for conversations, and **Responses** is the newer form that Codex uses. **Completions** is for plain text. **Embeddings** turn text into numbers for search.
- **Anthropic style.** The **Messages** API, which Claude Code uses.

Here's the core of a request in the OpenAI style. It asks one model one question:

```json
{
  "model": "llama-3.3-70b",
  "messages": [{ "role": "user", "content": "Hello" }]
}
```

The `model` line is the **Name in the API**: the name the engine knows the model by. Through the Server, a model's name also says who provides it, as `<provider>/<model>`, such as `cell1-vllm-8000/llama-3.3-70b` or `anthropic/claude-opus-5-5`.

### Two ways to reach a cluster model

![Two ways from an app to a model: straight to the port on the head node, or through the Server on this Mac](img/diagram-app-to-head-node.svg)

Either way, a request ends at the model's port on the **head node**, the first node of a load, which answers requests. A **worker** is another node in a load. It works with the head node over the fabric, and apps don't talk to it.

| | Straight to the endpoint | Through the Server |
|---|---|---|
| Address | The one on its card, such as `http://192.168.50.51:8000/v1` | `http://127.0.0.1:8484` |
| Who can use it | Anything on your network, with the load's key if it has one | Apps on this Mac, with a SparkBox key |
| Models | That one model | Every model SparkBox can reach, in one list |
| Who sees the requests | Nobody in SparkBox | The **Requests** list in **Server** |

[Use a model from another computer](help:use-a-model-from-another-computer) takes the first way. [Connect an app to SparkBox](help:connect-an-app) takes the second.

## Why it matters in SparkBox

- **Endpoints** shows every model server's address. A line on the card says if this Mac can't reach it.
- **Chat** sends from this Mac. It can use an endpoint only if this Mac can reach it. That's why an Ollama listening on `127.0.0.1` can't be used in Chat.
- **Server** gives your apps one address for everything. Most apps need three things from it: a base URL, a key and a model name. It hands you all three to copy.
- **Listen on**, when you load a model, picks who can reach it. **Fabric only** takes it away from this Mac.

## Related

- [Run SparkBox as a server](help:run-sparkbox-as-a-server)
- [Connect an app to SparkBox](help:connect-an-app)
- [Use a model from another computer](help:use-a-model-from-another-computer)
- [Create an API key for an app](help:create-an-api-key)
- [Tokens and context](help:tokens-and-context)
- [Where your passwords and keys go](help:keychain-and-secrets)
