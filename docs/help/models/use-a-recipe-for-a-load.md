---
id: use-a-recipe-for-a-load
title: "Use a recipe for a load"
type: howto
area: models
summary: "See the recipe SparkBox picked for a model, what it fills in, and what it's been verified on."
keywords: ["recipe", "recipes", "load recipe", "built-in recipe", "generic recipe", "defaults for a model", "verified on", "which settings does this model need"]
verified: false
related: [load-a-model-on-one-node, load-a-model-on-several-nodes, models-load-page, settings-models]
---

# Use a recipe for a load

To start a load from settings that suit the model, let the **Load** page pick its recipe, then check what the recipe filled in.

A **recipe** here is SparkBox's saved description of how to load one kind of model. It's different from a Docker image: a recipe says which splits the model can take, which values to start from, and which options vLLM needs.

## Before you start

- You've found the model in your **Library**. See [Download a model](help:download-a-model).
- You don't need Admin mode. Everything here only reads, so it also works in look-only mode. Nothing changes on a node until you click the button at the top of the **Load** page.
- The **Expert: raw vLLM args** field appears only with Admin mode unlocked. Most loads leave it empty.

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=library) The **Models** window opens on the **Library** tab.
2. Click your model's row.
3. Click **Load…** in the toolbar. The **Load** page opens.
4. Under **Model**, read **Recipe**. SparkBox matches the model's name against its recipes and shows the best one. A model with no recipe of its own gets **Generic vLLM**.
5. A model with its own recipe has a menu here, with **Generic vLLM** as the second choice. Leave it on the model's own recipe. To try a split that recipe doesn't list, choose **Generic vLLM**.
6. Read the recipe's note, if it has one. A note in amber means the recipe can't load this model, and says why.
7. Read the line under **Recipe**. "Verified on: nothing yet" means no load of this recipe has reached **Ready** on your cluster.
8. Check what the recipe filled in: **Max context**, **Requests at once**, **Tool calling** and **Reasoning parser**. **Generic vLLM** leaves them for you. No recipe sets **GPU memory share**: SparkBox sizes it to the model.
9. Change any of them if you need to. A field you've changed stays as you set it, even if you pick another recipe.
10. Go on with [Load a model on one node](help:load-a-model-on-one-node) or [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes).

## Import a launch script

Open **Import a launch script as a recipe…** on the Load page. Choose **Script node** and **Script folder**, then click **Read and preview…**. Confirm **Read and preview** to read `config.env` and `inner.sh`. SparkBox parses literal assignments and one `vllm serve` command. It never sources or executes either script, and omits secret options.

Read the draft and every warning. Shell arrays and options SparkBox controls are omitted. A path inside a container must be replaced by the matching host folder from the Library. Click **Use in this Load form** to replace the recipe, image, split, limits and parsers. This starts nothing. Choose the correct nodes and host folder, review the normal checks, then use the Load button. Unknown shell expressions require manual translation. Importing establishes no image or model compatibility.

## What you should see

**Recipe** names the model's recipe, or **Generic vLLM**, and the fields below it show the recipe's values. Once a load of this recipe reaches **Ready**, the line under it lists the split, the number of nodes, the image and the date, such as "Verified on: single on 1 node, 5a0f8b91…, 2 Nov 2026". [Settings: Models](help:settings-models#recipes) lists every recipe the same way.

## Why it works

A recipe holds what SparkBox knows about a model, so that you don't have to work it out each time. SparkBox saves what the recipe gave a load, so a restart builds the same files. It never fills in a secret: no recipe can hold an API key or a token. **Verified on** is written only when a load really passes its checks, so an empty list means nobody has tried it yet.

## Limits

- Built-in recipes cannot be edited here. An imported recipe belongs to this Load form and the load you start from it; it does not replace a built-in recipe.
- `GLM-5.3-Flash` has a recipe that can't load. It reads "Needs the FP4-Local checkpoint and its image, see What's coming", and **Generic vLLM** can't load it either. See [What's coming](help:whats-coming).
- A built-in recipe doesn't choose the image on the page. An imported draft supplies its recorded image; review it under **Image**.
- Recipes apply to vLLM. An SGLang load doesn't use one.

## If something goes wrong

- **The load button stays dimmed, and the recipe's note says why.** This recipe can't load the model. Follow what the note says.
- **"The Gemma 3 27B recipe doesn't list this split. Pick Generic vLLM to try it."** The recipe leaves this split out. Pick a split the **Split** menu offers. Or choose **Generic vLLM** under **Recipe**, knowing that split has no record behind it.
- **"There's no recipe called gemma-3-27b-it."** A saved load names a recipe this SparkBox doesn't have. Choose another under **Recipe**.
- **A value you expected isn't filled in.** You changed that field earlier, or the recipe doesn't set it. Type the value yourself.
- **An option in Expert: raw vLLM args is refused.** The sentence beside it says why. See [Expert: raw vLLM args](help:models-load-page#expert-raw-vllm-args).

## Next steps

- [Load a model on one node](help:load-a-model-on-one-node)
- [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes)
- [Choose load settings](help:choose-load-settings)
- [Settings: Models](help:settings-models), to see every recipe
