---
id: choose-load-settings
title: "Choose load settings"
type: howto
area: models
summary: "Decide a load's context, its memory share and who can reach it, and when to change anything else."
keywords: ["gpu memory share", "max context", "listen on", "api key", "port", "keep running", "which settings", "how much context", "what should i pick"]
verified: false
related: [models-load-page, memory-on-a-spark, tokens-and-context]
---

# Choose load settings

To set up a load, choose three things: its context, its memory share, and who can reach it.

## Before you start

- You have the **Load** page open. See [Load a model on one node](help:load-a-model-on-one-node).
- You can change every setting without Admin mode. Nothing happens until you click **Load on 1 node**, so you're free to experiment.
- The **Memory plan** and the **What will run** commands update as you type.
- SparkBox fills in some fields from the model's recipe. See [Use a recipe for a load](help:use-a-recipe-for-a-load).
- For what every field does, see [Models: the Load page](help:models-load-page).

## Steps

Most loads need only the first three stages.

### Max context

1. Under **Split and limits**, click **Max context**. If the recipe hasn't filled it in, type `32768`. That suits most chats. It's about 24,000 words, counting your words and the model's reply.
2. Go higher only if you'll paste in long documents. More context takes more memory, so read the **Memory plan** after you change it. Don't leave the field empty: an empty field plans for the model's full maximum, which can be a million tokens.

### GPU memory share

3. Leave **GPU memory share** as SparkBox set it. SparkBox works out the least share that holds the model and four full-length requests at this **Max context**, with 10% to spare. The hint beside the field reads "Sized to this model."
4. Read the word on the **Memory plan** bar. If it reads **Fits**, go on.
5. If the plan is amber or red, click the button SparkBox offers, or lower **Max context**. Don't raise the share to make a model fit: a higher share gives more KV cache and less safety. The **KV cache** is the memory the model uses to track the text so far.
6. To reserve a different share, type it in the field. SparkBox then leaves your value alone, whatever else you change. To go back, click the button beside the field, such as **Use 0.25 — sized to this model**.

### Who can reach it

7. Under **Access**, choose **Listen on**. Keep **LAN** for this Mac, Chat and other apps on your network.
8. Leave **API key** on. SparkBox makes a new key for this load, and Chat and the Server use it for you.

### Anything else

9. Turn on **Tool calling** only if an app you use needs it.
10. Turn on **Keep running** if the model should start again after a crash. See [Keep running](help:models-load-page#keep-running) for how it differs between one node and several.
11. Leave everything else alone, unless this table gives you a reason to change it.

| Setting | Change it when |
|---|---|
| [KV cache type](help:models-load-page#kv-cache-type) | You used **KV fp8** in the fit table, or you need more context in the same memory. Set it to **fp8 (half the memory)**. The choice in the fit table doesn't carry over |
| [Requests at once](help:models-load-page#requests-at-once) | Several people or apps will send long requests at the same time |
| [Allow remote code](help:models-load-page#allow-remote-code) | The model's page says it needs this, and you trust the organization that published it. It runs their program on your node |
| [Image](help:models-load-page#image) | The model gives garbage answers. See [Fix a load that failed](help:fix-a-load-that-failed) |
| [Expert: raw vLLM args](help:models-load-page#expert-raw-vllm-args) | The model's page or the AI advisor names an option. This field appears only with Admin mode unlocked |
| [Override memory guardrail](help:models-load-page#override-memory-guardrail) | Almost never. Lower the share or the context instead |

## What you should see

The **Memory plan** reads **Fits** on every node, and no message appears beside the **Load on 1 node** button. Under **What will run**, a note sums up the load, such as "28.8 GiB of weights · KV cache 64 KiB per token · 32k context".

## Why it works

Each setting becomes part of the launch files under **What will run**. SparkBox checks the same rules again when you click the load button, so the page and the load always agree. The memory plan does the sums from the model's own config file and what the node is using now.

## If something goes wrong

- **"GPU memory utilization must be between 0.2 and 0.95."** Type a value in that range.
- **"Use a port of 1024 or above."** Lower ports are reserved.
- **"The served model name may use letters, digits and . _ - / : only."** Change **Name in the API**.
- **"--max-model-len is set by SparkBox from the fields above; remove it from the raw vLLM args."** Use the field on this page instead.
- **"--trust-remote-code runs Python code from the model's repository. Tick “Allow remote code” to opt in; it can't be added as an Expert option."** Use the switch, if you trust the repository.

For more messages, see [Model messages](help:errors-models).

## Next steps

- [Load a model on one node](help:load-a-model-on-one-node)
- [How memory works on a Spark](help:memory-on-a-spark)
- [Tokens and context](help:tokens-and-context)
- [Models: the Load page](help:models-load-page), to tour the whole page
