---
id: chat-with-a-cloud-model
title: Chat with a cloud model
type: howto
area: ai
summary: Talk to Claude or GPT from SparkBox. Your messages go to that company, and you pay for what you use.
keywords: [claude, gpt, cloud, cost, money, privacy, effort, anthropic, openai]
verified: false
related: [add-a-cloud-provider, chat, tokens-and-context]
---

# Chat with a cloud model

To talk to Claude or GPT from SparkBox, pick the model in **Chat** and send your message.

## Before you start

- You need an API key saved for that company. [Add a cloud provider](help:add-a-cloud-provider) shows how.
- Look-only mode must be off, because Chat doesn't send while it's on.
- You don't need Admin mode.
- This Mac needs the internet.

> [!WARNING]
> **Each message goes to that company and costs money.** The whole conversation so far goes with it, so you pay for those tokens again every turn. You can't recall a message, so leave passwords, keys and anything private out of it. To stay on your own hardware at no cost per message, use a [cluster model](help:chat-with-a-cluster-model) instead.

SparkBox doesn't show what a message costs, or what you've spent so far. The company's website has prices and your usage. The stats line under each reply gives you the token counts, so you can work out the cost.

## Steps

1. In the sidebar, under **AI**, click **Chat**. [Open Chat](open:chat) The **Chat** window opens.
2. Click the button at the left of the toolbar. A list of models opens.
3. Under **Anthropic** or **OpenAI**, click a model, such as `claude-opus-5-5`. The list closes, and the button shows your choice. If the model says "no key", go back and [add your key](help:add-a-cloud-provider).
4. To give the model more or less time to think, open the effort choice in the toolbar and pick one. **Default effort** is a good start. Higher effort uses more tokens, and so costs more.
5. Click the box at the bottom that says "Message (Enter to send, Shift+Enter for a new line)".
6. Type your message. This is the last point before it leaves this Mac.
7. Press <kbd>Return</kbd>. You see "Waiting for the first token…". A section marked **Thinking** may open while the model reasons.
8. Read the reply as it arrives. To stop it partway, press <kbd>Esc</kbd> or click the square. You still pay for what was generated.

## What you should see

- The reply appears word by word, under a line such as "claude-opus-5-5 · Anthropic (Claude)".
- A line under the reply shows how it went, such as "1.2 s to first token · 61.0 tok/s · 412 tokens · 7.9 s total · 96 in · finished". **in** is what you sent, and **tokens** is what came back. Those are the two numbers you pay for.
- The conversation appears in the list at the left. It's saved on this Mac.

## Why it works

SparkBox sends your request straight from this Mac to the company over an encrypted connection, with your key from the [Keychain](help:keychain-and-secrets). Your nodes play no part. The model keeps no memory between requests, so each time you send, SparkBox sends the conversation on screen and your system prompt. That's why a long chat costs more per message than a short one. **Regenerate** and editing a question each make a new request, and each one costs money too.

## Limits

- SparkBox doesn't show prices, totals or spending limits. To set a spending limit, use the company's website.
- You can't limit how long a reply can be. A Claude reply can run to 64,000 tokens.
- Chat handles text only. You can't attach files or pictures.
- Chat can't use tools, such as searching the web.
- A reply that sends nothing for 5 minutes counts as stopped.
- What a company keeps, and for how long, is set by its own terms. SparkBox can't change that.

## If something goes wrong

- **"Add your Anthropic (Claude) API key to chat with it."** No key is saved. Click **Open Providers**, then follow [Add a cloud provider](help:add-a-cloud-provider).
- **"Anthropic (Claude) returned 401: invalid x-api-key. Check the API key."** The company refused the key. Save a correct one in [Providers](help:providers).
- **"OpenAI returned 429: … Rate limited; try again shortly."** You've sent too much in a short time, or your account has run out of credit. Wait a little, then send again. If it goes on, check your account on the company's website.
- **"Anthropic (Claude) returned 404: … The model or endpoint wasn't found."** Your account can't use that model, or it's been retired. Pick another one.
- **"The model stopped responding: nothing came from Anthropic (Claude) for 300 s."** Nothing came back for 5 minutes. Click **Regenerate**, or use the pencil on your question to send it again.
- **"The connection to Anthropic (Claude) closed before the reply finished."** The connection broke. Whatever arrived is kept. Click **Regenerate** to ask again.
- **"Claude declined this request, so nothing from this turn is sent back next time."** The model refused. That turn won't be part of what's sent next time. Rephrase your message, or start a new chat.
- **"OpenAI doesn't give this account reasoning summaries, so this reply comes without one."** Your account doesn't include the **Thinking** section for that model. The reply itself is complete.
- **"Earlier reasoning (…) was left out: it came from another model."** You switched models. Claude keeps its own reasoning, not another model's. The conversation itself still goes across.
- **Send is dimmed, and its tooltip reads "Look-only · turn off in Settings › Security & privacy".** [Turn off look-only mode](help:use-look-only-mode), then send again. What you typed stays in the box.

[Chat, Providers and Server messages](help:errors-ai) lists more messages.

## Next steps

- [Chat with a cluster model](help:chat-with-a-cluster-model), to compare it with your own hardware
- [Run SparkBox as a server](help:run-sparkbox-as-a-server), to use cloud models from your other apps
- [Chat](help:chat), to see everything in the window
