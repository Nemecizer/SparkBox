---
id: connect-an-app
title: Connect an app to SparkBox
type: howto
area: ai
summary: Point Claude Code, Codex or your own code at the Server, using the snippets it gives you.
keywords: [claude code, codex, sdk, curl, connect, base url, snippet, python, environment variable, use from an app, use my cluster, hidden key, shell history]
verified: false
related: [server, create-an-api-key, endpoints-and-apis, whats-coming, give-a-model-a-short-name, use-a-model-from-another-computer]
---

# Connect an app to SparkBox

To use your models from another app, put your key in a Terminal variable, then paste the text the **Server** gives you.

## Before you start

- The Server must be running. [Run SparkBox as a server](help:run-sparkbox-as-a-server) shows how.
- You need a SparkBox key for this app. [Create an API key for an app](help:create-an-api-key) shows how.
- The app must be on this Mac, because the Server answers this Mac only. For an app on another computer, follow [Use a model from another computer](help:use-a-model-from-another-computer).
- Claude Code and Codex must already be installed. SparkBox doesn't install or start them. To install one, follow [Claude Code's quickstart](https://code.claude.com/docs/en/quickstart) or the [Codex CLI page](https://learn.chatgpt.com/docs/codex/cli). [What's coming](help:whats-coming#coding-apps) says what's planned.
- With a cloud model, everything the app sends costs money. An app such as Claude Code sends a lot, and SparkBox shows no total.

> [!WARNING]
> **Keep your key out of Terminal commands and files.** Terminal saves the commands you run in a history file, and a file can get copied or shared. Anyone who has the key can use your models and spend money on your cloud accounts. The steps below hold the key in one Terminal window, so it's never saved. If a key does end up somewhere it shouldn't, [revoke it](help:create-an-api-key#revoke-a-key). That cuts it off at once, and you can then make a new one.

## Steps

### Give Terminal your key

A **variable** is a named value that a Terminal window keeps in memory. It passes that value to the programs you start from that window. Here, a variable named `SPARKBOX_API_KEY` holds your key until you close the window.

1. Open the macOS Terminal app, in **Applications › Utilities**. Don't use SparkBox's own **Terminal** window: its shells run on your nodes, and the Server answers this Mac only.
2. In the box below, click **Copy**.

   ```sh
   printf 'SparkBox key: '; read -rs SPARKBOX_API_KEY; echo; export SPARKBOX_API_KEY
   ```

3. Click in the Terminal app window, paste the line and press <kbd>Return</kbd>. It shows `SparkBox key:` and waits.
4. Copy your key. If you've only now created it, click **Copy** in the box under **Access keys**. Otherwise, copy it from your password manager.
5. Click in the Terminal app window, paste the key and press <kbd>Return</kbd>. Nothing appears as you paste. That's how this line keeps the key hidden.
6. To check it, click **Copy** in the box below.

   ```sh
   echo ${#SPARKBOX_API_KEY}
   ```

7. Click in the Terminal app window, paste the line and press <kbd>Return</kbd>. It prints `44`, the number of characters in a SparkBox key, and not the key itself. If it prints another number, go back to step 2.

Leave this window open, because the parts below use it. Each new Terminal app window starts with no key, so you repeat this part there.

SparkBox shows a key only once. To use it again later, keep it in a password manager, such as the **Passwords** app on this Mac. If you didn't keep it, [revoke the old key](help:create-an-api-key#revoke-a-key) and create a new one.

### Copy the text

1. In the sidebar, under **AI**, click **Server**. [Open Server](open:server) The **Server** window opens.
2. Under **Connect your apps**, click the button next to **Model**. A list of models opens.
3. Click the model you want. Pick one that says **Ready**, or a cloud model. If you've [given it a short name](help:give-a-model-a-short-name), pick that name in the **or an alias…** list instead.
4. Click your app: **Claude Code**, **Codex**, **OpenAI SDK**, **Anthropic SDK** or **curl**. The text below changes.
5. Click **Copy**.


The text never holds your key. It names the variable `SPARKBOX_API_KEY`, and your app reads the key from there.

Go to the part for your app: [Claude Code](#claude-code), [Codex](#codex), [OpenAI SDK or Anthropic SDK](#openai-sdk-or-anthropic-sdk), or [curl](#curl). You don't need the others.

### Claude Code

1. Click in the Terminal app window where you entered your key.
2. Paste the text and press <kbd>Return</kbd>. The `export` lines tell Claude Code where to go, and the last line, `claude`, starts it.

The Terminal app may show `zsh: command not found: #` twice. You can ignore that. It comes from the two comment lines in the text, and the other lines still run.

Claude Code also asks for other models for small side tasks. The lines that end in `OPUS_MODEL`, `SONNET_MODEL` and `HAIKU_MODEL` send those to your model too. Without them, those requests would try to reach Anthropic directly.

A cluster model needs **Tool calling** turned on when it's loaded, under **Capabilities** on the Load page. It also needs a large **Max context**.

### Codex

Codex reads your key from the variable by itself, so its settings file needs no key.

1. In the box below, click **Copy**.

   ```sh
   mkdir -p ~/.codex && touch ~/.codex/config.toml && open -e ~/.codex/config.toml
   ```

2. Click in the Terminal app window where you entered your key, paste the line and press <kbd>Return</kbd>. This makes Codex's settings file, `~/.codex/config.toml`, if it isn't there. The file opens in TextEdit.
3. Go back to the **Server** window and click **Copy** again. Copying that line replaced the text on your clipboard.
4. Click in the TextEdit window and paste the text. If the file already has settings in it, paste at the end. Then move the `model` and `model_provider` lines to the very top, and delete any older lines with those names.
5. Press <kbd>⌘S</kbd> to save the file. It holds the Server's address and your model, not your key.
6. Click in the same Terminal app window, type `codex` and press <kbd>Return</kbd>. Codex starts.

Codex uses the Responses API. OpenAI models and vLLM support it directly. Other engines may not, and Claude models don't.

### OpenAI SDK or Anthropic SDK

An SDK is a code library for calling an API. These two are for Python.

1. Open TextEdit, in **Applications**. If it shows a list of files, click **New Document**.
2. Choose **Format › Make Plain Text**.
3. Paste the text.
4. Press <kbd>⌘S</kbd>, and save the file as `hello.py` in your home folder. If TextEdit asks which extension to use, choose `.py`. The file holds no key: the line with `api_key=` reads it from the variable.
5. Click in the Terminal app window where you entered your key. Install the library by typing `pip3 install openai` or `pip3 install anthropic`, then press <kbd>Return</kbd>. If it's already installed, this changes nothing.
6. Type `python3 hello.py` and press <kbd>Return</kbd>. It asks the model to say hello and prints the reply.

### curl

`curl` is a tool for sending web requests. It comes with macOS. To check that the Server works, **Send test requests** needs no key and no Terminal. Use this tab to see the whole reply, as an app would.

> [!WARNING]
> **As copied, this text shows your key while it runs.** While `curl` runs, this Mac's list of running programs shows its command, key included. Steps 3 and 4 change one line, so your key stays off that list. If you run the text unchanged on a Mac that other people use, [revoke the key](help:create-an-api-key#revoke-a-key) and make a new one.

1. Open TextEdit, in **Applications**. If it shows a list of files, click **New Document**.
2. Press <kbd>⌘V</kbd>. The text from the **curl** tab appears.
3. In the box below, click **Copy**.

   ```sh
   -H @<(printf 'Authorization: Bearer %s\n' "$SPARKBOX_API_KEY") \
   ```

4. In TextEdit, find the line that contains `Authorization`. With the mouse, select it from the `-H` at its start to the `\` at its end, and press <kbd>⌘V</kbd>. The new line replaces it. The text now looks like this, with your own address and model.

   ```sh
   curl http://127.0.0.1:8484/v1/chat/completions \
     -H @<(printf 'Authorization: Bearer %s\n' "$SPARKBOX_API_KEY") \
     -H "Content-Type: application/json" \
     -d '{"model": "cell1-vllm-8000/llama-3.3-70b", "messages": [{"role": "user", "content": "Hello"}]}'
   ```

5. Press <kbd>⌘A</kbd>, then <kbd>⌘C</kbd>. That copies all the text.
6. Click in the Terminal app window where you entered your key.
7. Press <kbd>⌘V</kbd>, then press <kbd>Return</kbd>. The reply prints as one block of text, called JSON. The model's answer is in it, under `content`.

`printf` is built into the terminal, so it passes your key to `curl` without showing it.

## What you should see

- The app answers.
- In the **Server** window, the **Requests** table has a new row. **App** shows your key's name, **Model** shows the model, and **Status** shows `200`.

## Why it works

Each app has a setting for where to send its requests. The text you pasted points it at `http://127.0.0.1:8484` instead of a company's address, and tells it to read your SparkBox key from the variable. The app can't tell the difference, because the Server speaks the same API. The model name tells the Server where to send each request. Your key stays out of sight because `read -s` takes what you paste without showing it or saving it, and the history file records only the name `$SPARKBOX_API_KEY`.

## Limits

Not every model takes every API. This table shows what the Server does with each.

| Model from | Chat Completions | Responses | Messages | Completions, Embeddings |
|---|---|---|---|---|
| Anthropic | Yes, text only | No | Yes | No |
| OpenAI | Yes | Yes | No | Yes |
| Your cluster | Yes | If the engine serves it | If the engine serves it | If the engine serves it |

For a model loaded in SparkBox, its card in **Endpoints** has a line that says which APIs its engine serves, such as "Native API (from /openapi.json): Messages ✓ · count_tokens ✓ · Responses ✗".

Other limits:

- Apps on other computers can't use the Server. They can reach a cluster model directly: see [Use a model from another computer](help:use-a-model-from-another-computer).
- An app that sends tools to Claude must use Messages. Through Chat Completions, it gets "Tool calls to Claude through the Chat Completions API aren't supported yet; use the Anthropic Messages API (/v1/messages) instead."
- Through Chat Completions, Claude ignores `temperature`.
- One request can be up to 64 MiB.

## If something goes wrong

- **The app says it can't connect.** The Server isn't running, or the app uses the wrong port. Check the toolbar in the **Server** window. Look-only mode also stops the Server.
- **"curl: (3) URL rejected: Malformed input to a URL function", after the reply.** The line you changed in [step 4 of the curl part](#curl) has joined the next one, and the reply above the message is good. To run the text again, click before `-H "Content-Type` in TextEdit and press <kbd>Return</kbd>.
- **"zsh: command not found: claude" or "zsh: command not found: codex".** The app isn't installed, or this window opened before it was. Install it, open a new Terminal app window, and [give it your key](#give-terminal-your-key) again.
- **"KeyError: 'SPARKBOX_API_KEY'".** You ran `hello.py` in a window that doesn't have your key. Run it in the window where you entered the key.
- **"Missing or unknown SparkBox key. Create one in SparkBox › AI › Server."** The key is wrong, revoked or not reaching the app. The **Requests** table shows the attempt as **(no key)** or **(wrong key)**, with status `401`. Each new Terminal app window starts with no key, so [give it your key](#give-terminal-your-key) again and paste the text in that window. If that doesn't help, [create a new key](help:create-an-api-key).
- **"No provider serves model 'provider/model'. Use '\<provider\>/\<model\>' from GET /v1/models, or add an alias in SparkBox › AI › Server."** The text had no real model in it, because none was picked. `provider/model` is only a stand-in, and a note under the text says "Pick a model above". Pick a model under **Connect your apps** and copy the text again.
- **"No provider serves model 'local'…"** The model behind that alias has gone, such as after an unload. In the **Server** window, point the alias at another model, as [Give a model a short name](help:give-a-model-a-short-name) shows.
- **"Claude can't be used through the OpenAI Responses API yet. Use /v1/messages or /v1/chat/completions."** Codex can't use a Claude model. Pick an OpenAI or cluster model.
- **"OpenAI models can't be used through the Anthropic Messages API yet. Use /v1/chat/completions or /v1/responses."** Claude Code can't use an OpenAI model. Pick a Claude or cluster model.
- **Claude Code can't use its tools with a cluster model.** The model wasn't loaded with **Tool calling** on. [Restart a model or change its settings](help:restart-or-change-a-model).

[Chat, Providers and Server messages](help:errors-ai) lists more messages, including the ones for a request the Server refuses.

## Next steps

- [Server](help:server), to see everything in the window
- [Endpoints, ports and API keys](help:endpoints-and-apis)
- [Create an API key for an app](help:create-an-api-key), to give your next app its own key
- [Give a model a short name](help:give-a-model-a-short-name), to switch models later without changing your app
