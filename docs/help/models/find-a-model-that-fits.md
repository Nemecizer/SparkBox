---
id: find-a-model-that-fits
title: "Find a model that fits"
type: howto
area: models
summary: "Search Hugging Face, read the fit table, and optionally ask an AI advisor whether a model will run well."
keywords: ["search", "search results", "which result", "any-to-any", "text-generation", "image-text-to-text", "re-upload", "fit table", "how big", "which model", "what model should i use", "what model", "recommend", "best model", "first model", "pick a model", "choose a model", "check with ai", "recommended", "too large", "too big", "my model is too large", "model too big", "wont fit", "will it fit", "how many sparks do i need", "how many nodes do i need", "which models can i run", "what models can i run"]
verified: false
related: [models-hugging-face, models-in-plain-words, download-a-model]
---

# Find a model that fits

To choose a model your Sparks can run, search Hugging Face in the **Models** window and read its fit table.

## Before you start

- If the words are new to you, read [Models, engines, memory and context in plain words](help:models-in-plain-words) first.
- This Mac needs internet access. The search goes straight from this Mac to Hugging Face.
- You don't need Admin mode. Searching and reading the fit table only read, so they also work in look-only mode and with **Demo** data.
- With **Demo** data the search is real, but the fit table's memory figures and **Fits now** column come from the simulated nodes.
- For a gated model, one whose owner asks you to accept a license, you need a Hugging Face access token. See [Get a Hugging Face token](help:get-a-hugging-face-token).
- **Check with AI** is optional. It needs an Anthropic or OpenAI API key, and your provider may charge for each check. See [Add a cloud provider](help:add-a-cloud-provider). It doesn't work in look-only mode.

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Models](open:models?tab=hf) The **Models** window opens.
2. Click the **Hugging Face** tab. On the left you see **Recommended for GB10**, four starting points for the chip in your Sparks. On the right you see **Will it fit?**
3. Click a recommended model to look at it, or go to step 4 to search. If you're not sure, click `Qwen/Qwen3.8-27B-FP8`, and see [If you're not sure which to pick](#if-youre-not-sure-which-to-pick). If you click one, skip to step 6.
4. Click the search box, type part of a model's name, such as `gemma`, and press <kbd>Return</kbd>. You can also paste a name like `org/name`, or a `huggingface.co` link. The list changes to your results, sorted by how often each model is downloaded.
5. Read each row's **Weights** and **Fits on**. These show `…` for a moment while SparkBox reads the model's files. **Fits on** gives the fewest Sparks that can hold the weights, such as "1 Spark" or "2 Sparks". [Choose from search results](#choose-from-search-results) says which rows to skip.
6. Click a model. The right side reads "Reading … from Hugging Face…", then shows its facts and its fit table.
7. Read the fit table. There's one row for each number of nodes. Find the row with the fewest **Nodes** that isn't dimmed. A dimmed row doesn't fit.
8. Check that row. **Context** should read `32768` or more, which suits most chats. If **Requests** is below 1, that's fine, as long as you set **Max context** when you load. **Fits now** should read **Yes**. If you can't see **Fits now**, drag the divider left or scroll the table sideways. [Fit table](help:models-hugging-face#fit-table) explains every column.
9. If the table reports no supported kernel on GB10, read its reason before proceeding. Changing **KV fp8** alone does not prove that the image can run this model.

> [!NOTE]
> That message means a supported [GPU kernel](help:glossary#kernel-gpu), a small piece of GPU code, has not been established for this model and image. Stock fp8 sparse MLA also fails for GLM-5.3. Its supported checkpoint and image remain future work.

10. For a second opinion, click **Check with AI**. This is optional, and it can be wrong. See [Ask the AI advisor](help:models-hugging-face#ask-the-ai-advisor).


## Choose from search results

A search can return 40 rows, and most won't suit your Sparks. Anyone can publish on Hugging Face, so the list mixes original models with copies, conversions and files for other kinds of computer. Go through the list with these four checks.

- **Skip rows your Sparks can't serve.** **MLX · Apple only** marks files for Apple silicon. **GGUF** under **Weights** means only files for Ollama or llama.cpp. **—** under **Weights** means no files the engine can serve.
- **Prefer the organization that made the model, or NVIDIA.** The name under each model says who published it. `google` made Gemma, so `google/gemma-4-E4B-it` is an original. `nvidia` publishes versions prepared for the chip in your Sparks, such as `nvidia/Gemma-4-26B-A4B-NVFP4`.
- **Treat an unknown account's re-upload with care.** A **re-upload** is a copy of a model, often changed, that someone else publishes. Words such as `Uncensored` in a name are a sign of one. SparkBox can't tell you what was changed. Never turn on **Allow remote code** for one, because that runs the account's own program on your node.
- **Check the task.** The **Task** column is the publisher's label for what the model does. Use this table to read it.

| **Task** reads | What the model does | Can you chat with it? |
|---|---|---|
| `text-generation` | Reads text and writes text | Yes |
| `image-text-to-text` | Reads text and pictures, and writes text | Yes. **Chat** sends only text, and that works |
| `any-to-any` | Takes and gives several kinds of content, such as text, pictures and sound | Often. It's a broad label, so read the model's page. Google's own `google/gemma-4-E4B-it` uses it |
| **—** | The publisher didn't label it | Read the model's page before you choose it |
| Anything else, such as `text-to-speech` | A different job, such as speech | No. Choose another row |

To show one task, click its button above the table, such as **text-generation**. **All tasks** shows every row again. To read a model's page, click the row, then click the model's name on the right. Its page opens in your web browser.

When only a few rows are left, go back to step 6 under [Steps](#steps) and read each one's fit table. Choose the smallest model that does what you need.

## If you're not sure which to pick

For a first download, pick `Qwen/Qwen3.8-27B-FP8`. It fits on one Spark, so nothing has to be split across nodes, and it leaves room for long conversations.

In general, a bigger model gives better answers, but it answers more slowly and takes more memory. The number in a name is its size: `27B` means 27 billion learned numbers. Start small, and move up if the answers aren't good enough.

**Recommended for GB10** gives each model one line of reason. Here's what those lines mean:

| Model | Its reason | In plain words | When to try it |
|---|---|---|---|
| `Qwen/Qwen3.8-27B-FP8` | "FP8 on one Spark, with room for long context" | A 28.8 GiB download, stored in a compact form. One Spark runs it, with memory to spare | Your first download |
| `openai/gpt-oss-120b` | "MXFP4 mixture of experts: one Spark, fast" | A 60.8 GiB download, also on one Spark. It's a larger model that uses only part of itself for each token, which keeps it quick | When you want more capable answers |
| `meta-llama/Llama-3.3-70B-Instruct` | "bf16 dense: two or more Sparks, slow per stream" | A 131 GiB download that's too big for one Spark. It's stored at full size, and each reply is slow. It's gated, so you need an access token to read its fit table | Only after a several-node load has worked for you |
| `nvidia/GLM-5.3-Flash-NVFP4` | "NVFP4 mixture of experts across 3–4 Sparks" | Split across three or four Sparks. It's already on all four of your nodes, so there's nothing to download | Not yet. See the note below |

> [!NOTE]
> **GLM-5.3-Flash is the model family that failed on your cluster.** Your one real 4-node load, `GLM-5.3-Flash`, stopped after 54 seconds, and a method that uses 3 to 4 nodes has never worked on your cluster. SparkBox now refuses to load this model. Its recipe reads "Needs the FP4-Local checkpoint and its image, see What's coming". See [What's coming](help:whats-coming).

Words such as FP8 and bf16 describe how a model's numbers are stored. You don't need them to choose. [Model formats and quantization](help:formats-and-quantization) explains them.

The fit estimate uses the Load page's host-runtime reserve, safe GPU share and model architecture. Sliding-window layers reserve only their window of KV cache; full-attention layers reserve the requested context. **Fits now** also requires enough room for the whole GPU reservation. Estimates do not establish image compatibility.

## What you should see

You now know how many Sparks a model takes, how long a context each choice allows, and whether your nodes have room today. A good choice has a row that isn't dimmed, with `32768` or more under **Context** and **Yes** under **Fits now**. **Requests** can be below 1, as long as you set **Max context** when you load.

Treat the table as a guide. When it disagrees with the **Memory plan** on the **Load** page, trust the plan.

## Why it works

SparkBox reads the repository's file list and its config file, and counts only the weights the engine serves. It then works out what each number of nodes leaves for the KV cache, from the model's own design. These are exact sums, so the same model always gets the same answer. **Fits now** adds what your nodes are using at this moment. See [How memory works on a Spark](help:memory-on-a-spark) and [Tokens and context](help:tokens-and-context).

## If something goes wrong

- **"Couldn't reach Hugging Face: …"** This Mac can't reach the website. Check its internet connection and try again.
- **"Hugging Face is rate-limiting requests; try again in a minute."** You've made many requests in a short time. Wait a minute. A saved access token can raise your limit.
- **"… is gated or private. Accept its license on huggingface.co and add a Hugging Face token in Settings › Models."** See [Get a Hugging Face token](help:get-a-hugging-face-token).
- **"… wasn't found on Hugging Face."** Check the spelling of the name. Names are in the form `org/name`.
- **Every row reads ✗ doesn't fit, on a gated model.** Without your Hugging Face access token, SparkBox can't read the model's design, so this says nothing about whether it fits. Accept the license, save your access token, and click the model again. See [Get a Hugging Face token](help:get-a-hugging-face-token).
- **A size starts with ≈.** SparkBox couldn't read the model's files, so this is an estimate from the website's figures. Click the row to try again.
- **Weights reads GGUF.** The repository has only files for Ollama or llama.cpp. SparkBox can't size or download them. Look for the same model as safetensors, or [use Ollama](help:use-ollama).
- **A row shows MLX · Apple only.** These files run on Apple silicon, not on a GB10. See [MLX](help:formats-and-quantization#mlx).
- **"Add your Anthropic (Claude) API key under AI › Providers."** Check with AI has no key to use. With OpenAI as the provider, it reads "Add your OpenAI API key under AI › Providers." See [Add a cloud provider](help:add-a-cloud-provider).
- **"SparkBox is in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date."** You clicked **Check with AI** in look-only mode. It doesn't work there, because the advisor model may be on your cluster. See [Use look-only mode](help:use-look-only-mode).

## Next steps

- [Download a model](help:download-a-model)
- [Prepare the cluster for models](help:prepare-the-cluster), to get ready for your first load
- [Get a Hugging Face token](help:get-a-hugging-face-token), to download a gated model
- [Models: Hugging Face tab](help:models-hugging-face), to see every part of this screen
