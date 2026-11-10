---
id: give-a-model-a-short-name
title: Give a model a short name
type: howto
area: ai
summary: Make an alias in the Server, so that you can switch models later without changing your app.
keywords: [alias, aliases, add alias, short name, nickname, model name, rename a model, switch models, change model, swap model, change the model my app uses, local]
verified: false
related: [server, connect-an-app, run-sparkbox-as-a-server]
---

# Give a model a short name

To switch models later without changing your app, give the model a short name in the **Server** window.

An **alias** is a short name for a model, such as `local`. Your app asks for `local`, and the Server sends the request to whichever model that name stands for today.

## Before you start

- Your app must reach your models through the Server. [Connect an app to SparkBox](help:connect-an-app) shows how. Chat doesn't use aliases, and neither does an app that [talks straight to a node](help:use-a-model-from-another-computer).
- The model must be available now: a **Ready** model on your cluster, or a cloud model.
- Be ready to unlock Admin mode, with look-only mode off. Saving an alias is a change on this Mac, so it needs Touch ID or your Mac password in the last 5 minutes. While look-only mode is on, **Add alias** and **Save settings** are dimmed.

## Steps

### Make the alias

1. In the sidebar, under **AI**, click **Server**. [Open Server](open:server) The **Server** window opens.
2. Scroll to **Aliases**, and click **Add alias**. A new row appears. The first row you add is already named `local`.
3. In **Alias**, keep `local` or type another short name. Use letters, digits and `.` `_` `:` `-` only.
4. In **Model**, pick the model it stands for. The list shows only models you can use now.
5. At the bottom right, click **Save settings**. SparkBox asks for Touch ID or your Mac password if your last confirmation is more than 5 minutes old. A message says "Server settings saved".

### Use it in your app

6. Scroll up to **Connect your apps**. Open the list that says **or an alias…**, and pick your alias. The text below now uses that name.
7. Click **Copy**, and follow [Connect an app to SparkBox](help:connect-an-app#copy-the-text) for your app.

If your app is already connected, you can instead change its model setting to the alias. For example, in code that uses the OpenAI SDK, write `model="local"`.

### Switch to another model later

1. In the **Server** window, under **Aliases**, find your alias's row.
2. In **Model**, pick the new model.
3. At the bottom right, click **Save settings**. A message says "Server settings saved".

You change nothing in the app. Its next request goes to the new model.

## What you should see

- The row under **Aliases** has no red message.
- The text under **Connect your apps** shows your alias where the full model name was.
- After your app sends a request, the **Requests** table has a new row. **Model** shows your alias, and **Provider** shows who answered.

## Why it works

Your app sends the alias in place of a model's name. The Server looks it up on every request, and passes the request on to the model it stands for. That's why a change works from the next request, with nothing to restart. The alias lives in the Server's settings on this Mac, so only apps that use the Server can use it.

## Limits

- **An alias stands for one load.** After you unload a model, its alias stops working, even if you load the same model again, because a new load counts as a new provider. Point the alias at the new load. **Restart…** keeps the alias working, and so does **Edit & reload…** if you keep the **Name in the API**.
- **Another model may not suit your app.** Claude Code, for example, needs tool calling and a large **Max context**. The alias changes where requests go, not what the model can do.
- **A saved alias keeps its row when its model goes.** The row warns that the model isn't offered, and you can still save other Server settings. A new or changed alias must point at a model that's offered now.

## If something goes wrong

| Message | What it means | What to do |
|---|---|---|
| "Give it a name" | The **Alias** field is empty | Type a name |
| "Letters, digits and . _ : - only" | The name has a character that isn't allowed, such as a space or `/` | Change the name |
| "Used twice" | Two rows have the same name | Change one of them |
| "Pick a model" | **Model** is empty | Pick a model |
| "Unknown target: no provider offers this model now" | You added or changed this alias, and its model isn't offered | Pick another model |
| "cell2-vllm-8000/gemma isn't offered right now" | This saved alias points at a model that has gone, such as after an unload. It's a warning, and **Save settings** still works | Pick another model, or load that model again |
| "Fix the aliases to save" | A row above has one of these problems | Fix that row, or remove it with the trash icon |

- **The or an alias… list isn't there.** It appears once an alias is saved. Click **Save settings** first.
- **"No provider serves model 'local'. Use '\<provider\>/\<model\>' from GET /v1/models, or add an alias in SparkBox › AI › Server."** The app asked for an alias that doesn't exist, or the model behind it has gone. Check the spelling, or point the alias at another model.

[Chat, Providers and Server messages](help:errors-ai) lists more messages.

## Next steps

- [Connect an app to SparkBox](help:connect-an-app)
- [Server](help:server#aliases), to see everything in the window
- [Run SparkBox as a server](help:run-sparkbox-as-a-server), to check a model with a test request
