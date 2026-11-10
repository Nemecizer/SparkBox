---
id: settings-models
title: "Settings: Models"
type: feature
area: settings
summary: "The model folder, engine images, your Hugging Face token, the load recipes, and the AI advisor."
keywords: ["model folder", "image", "token", "advisor", "recipes", "verified on"]
verified: false
related: [get-a-hugging-face-token, models-window, use-a-recipe-for-a-load]
---

# Settings: Models

## What it's for

This section sets where models live on your nodes, which engine images SparkBox uses, your Hugging Face access token, and which AI model answers **Check with AI**. Most people change only the access token.

## Where to find it

- **Settings:** press <kbd>⌘,</kbd>, then click **Models** in the list.
- **Models window:** click **Settings** at the right of the tabs.
- **Link:** [Open Models settings](open:settings?section=models)


## What's on the screen

### Where models live

**Model folder** is the same folder on every node. The default is `~/models`, where `~` means the cluster login's home folder. Downloads go to `<model folder>/<org>/<name>`. **Find models** looks here and in the Hugging Face cache, `~/.cache/huggingface/hub`.

Changing this doesn't move anything. Models already in the old folder stay there, and SparkBox stops listing them.

### Engines

- **vLLM image** is an image a vLLM load can pick if the node has it. Downloads ask you to choose their tool separately. The default is `vllm/vllm-openai:glm53-flash-arm64-cu130`.
- **SGLang image** is what SGLang loads use as it is. The default is `lmsysorg/sglang:spark`.

An **image** is the recipe a container starts from. Docker starts the engine from it. Under each field, **On the cluster:** lists other images your nodes' containers use. Clicking one fills in the field.

Leave these alone unless you know why you're changing them. A vLLM load can pick a different image on the **Load** page, without changing this setting.

### Hugging Face

**Access token** holds your Hugging Face access token, which gated models need. **Save** puts it in the Keychain, and **Remove** takes it out. Each asks for Touch ID or your Mac password right away, and doesn't wait for the **Save** button at the bottom. The field reads "Saved in Keychain" or "Not saved". SparkBox never shows it again.

See [Get a Hugging Face token](help:get-a-hugging-face-token).

### Recipes

A list of the load recipes built into SparkBox, and what each one has been verified on. A **recipe** is SparkBox's saved description of how to load one kind of model. "Verified on: nothing yet" means no load of it has reached **Ready** on your cluster. A recipe's note appears under its name. You can read this list, and you can't change it. See [Use a recipe for a load](help:use-a-recipe-for-a-load).

### AI feasibility check

**Check with AI** on the **Hugging Face** tab uses these settings.

- **Provider** is Anthropic or OpenAI. A provider without a key reads "(no key)".
- **Model** is which of its models answers. The default provider is Anthropic.

If the provider has no key, a note reads "Add your Anthropic (Claude) API key first.", with **Open Providers**. See [Add a cloud provider](help:add-a-cloud-provider).

Each check sends the model's facts: size, design, the fit table, your nodes' names and memory, what's serving, and the start of its model card. It doesn't send IP addresses or keys. Your provider may charge for it.

### Save and Revert

**Save** keeps your changes, and asks for Touch ID or your Mac password. **Revert** appears when you've changed something, and puts the fields back.

## What it changes

Everything here is stored on this Mac, and no node changes until you use it. A new image is used from the next load or download. A model folder change takes effect at the next **Find models**.

- **Save** and the access token's **Save** and **Remove** need Touch ID or your Mac password, even with **Demo** data.
- In look-only mode, this section is read-only.
- To undo a change, set the old value and click **Save**. To restore a default, type the value shown above.

## Related How-Tos

- [Get a Hugging Face token](help:get-a-hugging-face-token)
- [Download a model](help:download-a-model)
- [Use a recipe for a load](help:use-a-recipe-for-a-load)
- [Find a model that fits](help:find-a-model-that-fits)
- [Add a cloud provider](help:add-a-cloud-provider)
