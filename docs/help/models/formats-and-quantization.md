---
id: formats-and-quantization
title: "Model formats and quantization"
type: concept
area: models
summary: "Why one model comes in many sizes, and what safetensors, GGUF, MLX, FP8 and NVFP4 mean for your Sparks."
keywords: ["safetensors", "gguf", "mlx", "fp8", "nvfp4", "quantization", "bf16"]
verified: null
related: [find-a-model-that-fits, models-hugging-face]
---

# Model formats and quantization

## In short

One model often comes in many versions. A **format** is how its files are packed, and your Sparks can use some formats and not others. **Quantization** stores each number with fewer bits, so the model takes less memory and loses a little quality.

## How it works

### Formats

A format decides which programs can read a model. SparkBox looks at each repository's files and shows you what it found.

#### safetensors

This is the main format for vLLM and SGLang. Large models come as several numbered files, called shards. When a repository has safetensors files, these are what SparkBox downloads. The **Format** column in the **Library** reads `safetensors`.

#### .bin

An older format for the same kind of weights. SparkBox downloads `.bin` files only when a repository has no safetensors.

#### GGUF

The format used by Ollama and llama.cpp. vLLM and SGLang don't serve it, so SparkBox can't size or download a GGUF-only repository. The **Weights** column reads **GGUF**. To run one of these models, use [Ollama](help:use-ollama).

#### MLX

MLX is Apple's format for Apple silicon. It doesn't run on a GB10, the NVIDIA chip in your Sparks. SparkBox marks these repositories **MLX · Apple only** and offers no fit table, download or load. It says:

> "MLX weights run on Apple silicon, not on a GB10. Look for the same model as safetensors, GGUF or NVFP4."

Search for the model's name without the word MLX, and avoid the `mlx-community` organization. Its models are converted for Apple silicon.

### Why a repository is bigger than its download

Many repositories hold the same weights more than once, such as an `original/` folder or a copy for Apple's Metal. SparkBox downloads only the weights the engine serves, plus the small config and tokenizer files.

For example, `openai/gpt-oss-120b` is 182 GiB as a repository, but SparkBox downloads about 61 GiB. The details on the **Hugging Face** tab say what's left out, such as "Downloads 60.8 GiB of the repo's 182 GiB".

### Quantization

Each weight is a number, and a number can be stored with more or fewer bits. **Quantization** uses fewer. The model shrinks, and it usually gets a little less accurate. The main levels you'll see:

| Name in a model | Bits per weight | Notes |
|---|---|---|
| **bf16** | 16 | Full size. SparkBox's Library shows "none (bf16)" when a model isn't quantized |
| **FP8** | 8 | Half the size of bf16. The GB10 has hardware for it |
| **NVFP4** | 4 | NVIDIA's 4-bit format, made for chips like the GB10 |
| **MXFP4** | 4 | Another 4-bit format, used by `openai/gpt-oss-120b` |
| **W4A16**, **AWQ**, **GPTQ** | Usually 4 | General methods. A GB10 build of an engine may have no GPU kernel for them |

The effect is large. A 70-billion-weight model in bf16 is 131 GiB and needs two Sparks. A 27-billion-weight model in FP8 is 28.8 GiB and fits on one, with room for long context.

### The KV cache has its own type

Quantization of the weights is separate from the **KV cache**, the memory a model uses while it runs. Its type is a setting on the **Load** page, and fp8 halves the memory it takes per token. A few models need it. See [KV cache type](help:models-load-page#kv-cache-type).

## Why it matters in SparkBox

When you search, look for names that end in `FP8` or `NVFP4`. These versions make the best use of a Spark's memory. The **Recommended for GB10** list on the **Hugging Face** tab starts you with one of each kind.

If a load fails because the GB10 can't run a model's quantization, the likely cause reads "Its quantization likely isn't supported on this GPU". The fix is a version made for this chip, usually NVFP4 or FP8, or a newer engine image.

The **Library** tab shows each folder's **Format** and **Quantization**, so you can check what you have.

## Related

- [Find a model that fits](help:find-a-model-that-fits)
- [Models: Hugging Face tab](help:models-hugging-face)
- [Tokens and context](help:tokens-and-context)
- [Use Ollama](help:use-ollama)
- [Fix a load that failed](help:fix-a-load-that-failed)
