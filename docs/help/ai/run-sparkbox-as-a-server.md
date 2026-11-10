---
id: run-sparkbox-as-a-server
title: Run SparkBox as a server
type: howto
area: ai
summary: Turn on the Server, so that apps on this Mac can use your models through one address.
keywords: [server, api, local, openai compatible, start server, port 8484, test, 127.0.0.1, do i need the server]
verified: false
related: [server, create-an-api-key, connect-an-app, endpoints-and-apis, use-a-model-from-another-computer]
---

# Run SparkBox as a server

To let apps on this Mac use your models, start the **Server** and check it with a test request.

SparkBox's own Chat doesn't need the Server. You need it only for another program on this Mac, such as Claude Code, Codex or a script of your own.

## Before you start

- Starting and stopping the Server are changes on this Mac. They need [Admin mode](help:unlock-admin-mode), with Touch ID or your Mac password in the last 5 minutes. You don't have to unlock it first: SparkBox asks when you click **Start**.
- Look-only mode must be off. The Server doesn't run while it's on.
- You need a model for the test: a **Ready** model on your cluster, or a [cloud provider](help:add-a-cloud-provider). With Demo data, you can use the `echo` model.
- With a cloud model, the test costs a small amount.

The Server listens on this Mac only. To reach a cluster model from another computer, follow [Use a model from another computer](help:use-a-model-from-another-computer) instead.

## Steps

1. In the sidebar, under **AI**, click **Server**. [Open Server](open:server) The **Server** window opens. At the left of its toolbar it says **Stopped**.
2. Under **Settings**, look at **Port**. Leave it at `8484` unless you know another app uses that port. Leave **Require a SparkBox key** turned on.
3. In the toolbar, click **Start**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. The toolbar now says "Running at `http://127.0.0.1:8484`", and the button changes to **Stop**.
4. Under **Connect your apps**, click the button next to **Model**. A list of models opens.
5. Click a model. Cluster models are at the top, with their state. Pick one that says **Ready**.
6. Under **Test the server**, click **Send test requests**. The button says **Testing…** while it works. A table with three rows appears.
7. Read the **Result** column. At least one row should say **Works**, with a reply such as "OK" at the right. See [What you should see](#what-you-should-see) for which rows to expect.
8. To have the Server start whenever SparkBox opens, turn on **Start with SparkBox** under **Settings**. Then click **Save settings** at the bottom right. SparkBox asks for Touch ID or your Mac password if your last confirmation is more than 5 minutes old.

To stop the Server, click **Stop** in the toolbar. Apps using it lose their connection. You can also click the menu bar icon, then **Stop Server**. That works only if you confirmed with Touch ID or your Mac password in the last 5 minutes.

## What you should see

- The toolbar says "Running at `http://127.0.0.1:8484`".
- The status bar has a **Server** item with **:8484**. Click it to come back to this window.
- The test table shows which APIs work with the model you chose:

| Model | `chat/completions` | `responses` | `messages` |
|---|---|---|---|
| `echo`, in Demo | **Works** | **Works** | **Works** |
| A Claude model | **Works** | **Failed (400)** | **Works** |
| An OpenAI model | **Works** | **Works** | **Failed (400)** |
| A cluster model | **Works** | Depends on the engine | Depends on the engine |

- The **Requests** table at the bottom lists the three test requests, under the name "SparkBox self-test".

A **Failed (400)** in the table above is expected. It's a limit of that model's API, not a fault. For a cluster model, a **Failed** row means that engine doesn't serve that API. Check the "Native API" line on its card in **Endpoints**.

## Why it works

SparkBox opens a port on this Mac, at `127.0.0.1`, which only programs on this Mac can reach. When an app sends a request there, the Server checks its key and finds which provider offers that model. It then passes the request on with the real key from the [Keychain](help:keychain-and-secrets), and sends the reply back as it arrives.

So an app never sees your cloud keys or a load's key, and every app uses one address. The test uses a private key that SparkBox holds in memory, so it works before you've created any keys.

## Limits

- The Server answers this Mac only. SparkBox has no setting to open it to other computers.
- It stops when you quit SparkBox. It keeps running if you close the window to the menu bar.
- With Live data, it doesn't offer the demo model.
- It can't reach an engine that this Mac can't reach, such as an Ollama that listens on its node only.

## If something goes wrong

- **"Port 8484 is already used by another app. Pick a different port."** Type another port in **Port**, such as `8585`. Click **Save settings**, then **Start**. Any app you've already set up must use the new port too.
- **"Pick a port between 1024 and 65535."** The port is out of range.
- **The Start button is dimmed.** Look-only mode is on. [Turn it off](help:use-look-only-mode).
- **SparkBox asks again, although Admin mode is on.** Your last confirmation is more than 5 minutes old. When the **Unlock Admin mode?** dialog opens, click **Unlock…**, then confirm with Touch ID or your Mac password.
- **The menu bar icon's Start Server or Stop Server doesn't work, and SparkBox comes to the front.** Your last confirmation is more than 5 minutes old, and the menu bar icon can't ask for a new one. A notification may say "Unlock Admin mode in the top bar to start the API server." Use **Start** or **Stop** in the **Server** window instead.
- **"Start the server first."** The Server isn't running, so the test can't go anywhere. Click **Start**.
- **Send test requests is dimmed.** No model is chosen, or look-only mode is on. Pick a model under **Connect your apps**.
- **A row in Requests reads (no key) or (wrong key), with status `401`.** An app sent a missing or unknown key. See [Create an API key for an app](help:create-an-api-key).
- **A row says Failed (404), with "No provider serves model …".** The model has gone since you picked it. Pick another one.
- **A row says Failed (502), with "Couldn't reach …".** The Server can't get to that provider. For a cluster model, check its card in **Endpoints**, and check that you're at home or on the [VPN](help:connect-the-vpn).
- **A row says Failed (no answer).** No answer came within 2 minutes. Check the model's card in **Endpoints**.

[Chat, Providers and Server messages](help:errors-ai) lists more messages.

## Next steps

- [Create an API key for an app](help:create-an-api-key)
- [Connect an app to SparkBox](help:connect-an-app)
- [Server](help:server), to see everything in the window
