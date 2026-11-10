---
id: add-a-cloud-provider
title: Add a cloud provider
type: howto
area: ai
summary: Save an Anthropic or OpenAI API key, so that their models appear in Chat and the Server.
keywords: [anthropic, openai, claude, gpt, api key, cloud, keychain, get a key, openai key, anthropic key, claude key]
verified: false
related: [providers, chat-with-a-cloud-model, keychain-and-secrets]
---

# Add a cloud provider

To use Claude or GPT from SparkBox, save your API key from Anthropic or OpenAI in the **Providers** window.

## Before you start

- You need an API account with Anthropic or OpenAI. It's separate from a Claude or ChatGPT subscription, and it's billed by what you use.
- You need an API key from that account. An **API key** is a long secret that lets SparkBox use your account. You can make one on [Anthropic's key page](https://platform.claude.com/settings/keys) or [OpenAI's key page](https://platform.openai.com/api-keys).
- Saving a key asks for Touch ID or your Mac password every time. If [Admin mode](help:unlock-admin-mode) is locked, SparkBox unlocks it when you click **Save key**. Look-only mode must be off.
- This Mac needs the internet.

> [!NOTE]
> Treat an API key like a password. Anyone who has it can spend money on your account. Paste it only into the **API key** field in SparkBox, and leave it out of chats and notes.

## Steps

1. In the sidebar, under **AI**, click **Providers**. [Open Providers](open:providers) The window opens with a card for **Anthropic (Claude)** and a card for **OpenAI**.
2. Find the card for your provider.
3. Click the **API key** field on that card. It says "Paste your key".
4. Paste your key. The field shows dots, not the key.
5. Click **Save key**. SparkBox asks for Touch ID or your Mac password. Confirm it.
6. Wait a moment while SparkBox fetches the list of models. A message says "Key saved to the Keychain. Anthropic (Claude): 12 models listed", with your own count. The key field now says "•••••••• (saved)".
7. Check the list of models at the bottom of the card. Each one with its box turned on appears in **Chat** and the **Server**.
8. To change which model Chat starts with, pick one in **Default model**. SparkBox may ask for Touch ID or your Mac password again.


## What you should see

- The key field says "•••••••• (saved)".
- The card has **Remove key** beside **Save key**.
- The line above the model list says how many are offered, such as "12 of 12 offered in Chat and the Server".
- In **Chat**, the model picker lists your models under **Anthropic** or **OpenAI**, without "no key".

## Why it works

SparkBox keeps the key in the [macOS Keychain](help:keychain-and-secrets), and never in a file. It sends the key only to that company, and only when you send a request. After saving, it asks the company for its list of models, which also proves the key works.

Apps that use the [Server](help:server) never see this key. They get a SparkBox key, and SparkBox adds your real one when it passes their request on.

## Limits

- SparkBox doesn't show prices, or what you've spent. Check that on the company's own website.
- A card holds one key. To use a second account, you'd have to replace the key.

## If something goes wrong

- **"Anthropic (Claude) returned 401: invalid x-api-key. Check the API key."** The company refused the key. SparkBox saved it anyway, so paste a correct key and click **Save key** again. If you copied it from the website, check that nothing was cut off.
- **"Couldn't reach OpenAI: …"** This Mac can't get to that company. Check your internet connection.
- **"OpenAI returned 429: …"** That's the company limiting requests. Wait a little, then click **Refresh list**. A new account may also need billing set up on the company's website.
- **"No models listed yet. Refresh the list after adding the key."** The list hasn't arrived. Click **Refresh list**.
- **The Save key button is dimmed.** The field is empty.
- **A model you expected is missing.** Click **Refresh list**. If it's still missing, check that **Chat models only** isn't hiding it.

To stop using a key, click **Remove key** on the card. SparkBox asks for Touch ID or your Mac password, and deletes the key from the Keychain. That doesn't cancel the key itself. To do that, delete it on the company's website.

[Chat, Providers and Server messages](help:errors-ai) lists more messages.

## Next steps

- [Chat with a cloud model](help:chat-with-a-cloud-model)
- [Run SparkBox as a server](help:run-sparkbox-as-a-server)
- [Providers](help:providers), to see everything in the window
