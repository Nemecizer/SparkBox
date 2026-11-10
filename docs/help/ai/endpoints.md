---
id: endpoints
title: Endpoints
type: feature
area: models
summary: Every model server on your cluster, whoever started it, with live speed, health and actions.
keywords: [endpoints, inference, cards, ttft, kv cache, history, ready, down, idle, itl, test token]
verified: false
related: [tell-if-a-model-is-working, endpoints-and-apis, unload-a-model, ollama, use-ollama, chat-with-a-cluster-model, use-a-model-from-another-computer]
---

# Endpoints

## What it's for

An **endpoint** is a running model server you can talk to. The **Endpoints** window shows every one on your cluster, whoever started it: models you loaded in SparkBox, and servers it found running by themselves.

Use it to answer three questions. What's serving? Is it working? How fast is it?

SparkBox looks for vLLM, SGLang, TensorRT-LLM, NIM, llama.cpp and Ollama on every node. You don't have to tell it where they are.


## Where to find it

- **Sidebar:** under **Workloads**, click **Endpoints**.
- **Command palette:** press <kbd>⌘K</kbd>, type `endpoints`, and press <kbd>Return</kbd>.
- **Dashboard:** under **Serving**, click a model's card. **Endpoints** opens on it.
- **Models window:** on the **Loaded** tab, click **Open in Endpoints** on a row.
- **Link:** [Open Endpoints](open:endpoints)

## What's on the screen

### Toolbar

| Item | What it does |
|---|---|
| **Check now** | Asks every server for its models now. Otherwise SparkBox asks every 15 seconds, and reads the speed numbers every 3 seconds. An Ollama with nothing in memory is asked once a minute |
| **Load a model…** | Opens the Load page in **Models** |
| **All**, **SparkBox**, **External** | Which cards to show. **SparkBox** means models loaded here. **External** means everything else |
| An engine's name, such as **Ollama** or **vLLM** | Shows that engine only. There's one choice for each engine that's running |
| **Filter by model or node** | Hides cards that don't match what you type. It looks at the name, the address, the engine and the nodes |

### The tiles

Three tiles add up the whole cluster.

| Tile | What it shows |
|---|---|
| **Ready** | How many endpoints are Ready, out of all of them, such as "2 of 2". The line below says "all answering", or counts the rest, such as "1 down · 2 idle". It turns red if one is **Down** |
| **Generating** | Tokens per second from every Ready endpoint together. A **token** is a small piece of text, about three quarters of a word |
| **Requests** | Requests being answered now. The line below says how many are waiting. It turns amber if any are |

### The top line of a card

Reading from the left:

| Part | Example | What it means |
|---|---|---|
| Engine | **vLLM** | The program serving the model |
| Name | **llama-3.3-70b** | The name apps use. An Ollama card says **Ollama on cell3**, because its models come and go |
| State | **Ready** | See [States](#states) |
| Nodes | **cell1 · cell2 · cell3** | The nodes it runs on, head node first. The **head node** answers requests. A node it's missing is struck through in red. Move the pointer over a node's name for its role |
| Split and port | **PP 3 · :8000** | How the model is split across nodes, and its port |
| Context | **32k context** | The longest request it accepts, in tokens |
| Who started it | **SparkBox** | **SparkBox** if you loaded it here. **External** if SparkBox found it running. **Added by hand** if you added it in [Providers](help:providers#add-another-server) |

### States

| State | What it means | What to do |
|---|---|---|
| **Ready** | It's proven to be working. See below | Use it |
| **Loading**, or **Loading 42%** | It's starting. The line under the title says what it's doing, such as "2 of 4 nodes up", "Loading weights 42%", "Compiling: FlashInfer kernels", "Capturing CUDA graphs" or "Checking that it generates…" | Wait. A load SparkBox started stays **Loading** up to its 90-minute limit. After a long quiet spell, the line adds "no new progress for 25 min (limit 90 min)" |
| **Unknown** | A node is unavailable or has no current report. Its last successful check is history | Reconnect and wait for fresh checks |
| **Down** | It should be serving and isn't. One red line says why | Read the line, then see [Model messages](help:errors-models) |
| **Stopped** | It exited cleanly, such as "Stopped 2 h ago" | Load it again if you want it |
| **Idle** | An Ollama that answers, with nothing in memory. The line says "Nothing in memory" | Nothing. This is how Ollama rests |
| **Paused** | It's waiting for planned work on one of its nodes. The line says what work | Wait for the work to end |
| **Not found** | SparkBox can't find it any more, and has no evidence that it broke. The line says "Not found since 14:05" | Check the node. If it's gone for good, click **Forget** |

**Ready** means more than "a port answers". SparkBox calls an endpoint Ready only when all of these are true:

- Every node it needs is online and reporting.
- Its containers are running.
- It answered in the last minute.
- A token came out of it in the last 2 minutes.

Unavailable nodes keep their places in the model's node list. **Unknown** uses a neutral state and hides live numbers. **Last successful check** shows when it last responded, when known. SparkBox checks again when the nodes return. Unknown alone does not trigger a **Keep running** restart.

SparkBox saves discovered deployment membership and the node lists of loads it started. After reopening, those records remain historical until fresh reports arrive. A saved load awaiting recovery reads Unknown; reopening does not launch replacements. See [Recover a saved load](help:fix-a-load-that-failed#after-sparkbox-quits-or-loses-its-connection).

The token comes from one of two places. If real requests are making its token count rise, that's proof enough. If not, SparkBox sends a **test token**: a request for one token, once a minute. It allows 30 seconds for an answer. Two failures in a row turn the endpoint **Down**.

A model SparkBox loaded must also pass a fuller check before it first turns Ready. See [Tell whether a model is working](help:tell-if-a-model-is-working).

There are three cases where SparkBox can't send a test token, so it goes by the answer alone:

- A server that wants an API key SparkBox doesn't have. The card says "Not tested: it wants an API key SparkBox doesn't have".
- Look-only mode. The card says "Not tested: look-only mode holds test tokens".
- A server you added by hand. It's Ready when it answers.

SparkBox doesn't test an Ollama with nothing in memory, so a test never wakes a model.

### The lines under the title

Not every card has all of these.

| Line | What it tells you |
|---|---|
| A reason, such as "cell3 is offline" | Why it isn't Ready. It's red when the state is **Down** |
| "Answered 12 s ago · 1 token in 84 ms" | The last test token came back. The time is from request to reply |
| "Generating · live traffic" or "Generated 25 s ago · live traffic" | Real requests are the proof, so no test token was needed |
| "Slow: 1 token took 12.4 s" | It's Ready, but one token took more than 10 seconds |
| "No tokens for 20 s · 1 request running" | A request is running and nothing is coming out. If this lasts 60 seconds, it turns **Down** with "Stalled: 1 request running, no tokens for 60 s". Requests that are only waiting count too, and then it reads "Stalled: 2 requests waiting, no tokens for 60 s". A `/health` answer doesn't clear it |
| "Not checked since 14:02: SSH channels busy on cell1" | SparkBox couldn't get a free connection to the node to check it. The state is the last one it knew |
| "Keep running · restarted 2 times in 30 min" | **Keep running** is on for this load, and SparkBox has restarted it |
| "Keep running · It failed to start: won't restart it" | **Keep running** is on, but this start failed, so SparkBox leaves it for you. Fix the cause, then click **Restart…**, or unload the model. See [Fix a load that failed](help:fix-a-load-that-failed) |
| "Ready: /health 200, … 17+25 → 42" | What the check proved when this load started. Only models loaded in SparkBox have this line |
| "Native API (from /openapi.json): Messages ✓ · count_tokens ✓ · Responses ✗" | Which of the Anthropic and newer OpenAI APIs this engine serves itself. See [Connect an app to SparkBox](help:connect-an-app#limits) |

### The address

Under those lines is the endpoint's address, with a button to copy it. **LAN only** beside it means it listens on your home network, so apps on other computers there can reach it. [Endpoints, ports and API keys](help:endpoints-and-apis) explains each part.

Below the address, a card may say that this Mac can't reach it. For example: "Reachable from cell3 only (127.0.0.1:11434). Chat and the API server on this Mac can't use it." The address on that card works only from that node.

### The six numbers

Cards show the same six numbers, whatever the engine. A dash (—) means the number doesn't apply, or the endpoint isn't Ready. A small chart appears under a number once it starts to move.

An **Idle** card shows no numbers, because nothing is in memory.

For **Down** with a stall, the last **Running · waiting** numbers stay in gray, because those stuck requests explain the stall.

#### Generate

How fast the model writes, in tokens per second, for all requests together.

For Ollama and other engines with no numbers of their own, SparkBox counts only the requests it sent or relayed. Requests that went straight to the engine aren't in it.

#### Prompt

How fast the model reads what it's given, in tokens per second. It's above zero only while a request is being read, so 0 between requests is normal.

#### Running · waiting

Two numbers: requests being answered now, and requests in the queue. A request waits for a free slot or for room in the KV cache. Below them, "0.25 req/s done" shows how many finish each second.

Some waiting during a burst is normal. A queue that doesn't go down means apps are asking faster than the model can answer.

#### KV cache

How much of the KV cache is in use. The **KV cache** is the memory the model uses to keep track of the text so far. Think of it as short-term memory for each conversation.

Near 100%, new requests wait. Below it, "42% prefix hits" shows how often the model could reuse text it had already read. Ollama and llama.cpp don't report a KV cache, so their cards show a dash.

This percentage measures occupied cache space, not the share of all node memory. It can fall to zero while the engine keeps its GPU reservation for later requests. See [Comparing the memory readings](help:memory-on-a-spark#comparing-the-memory-readings).

#### TTFT p50 · p95

**TTFT** is time to first token: how long a request waits before the first word comes back. It's measured over the last minute. A longer wait usually means a long prompt or a queue.

- **p50** is the typical wait. Half of the requests were faster.
- **p95** is a slow one. 95 of every 100 requests were faster.

Some engines report only a range, such as "250–500 ms". That means the true value fell somewhere in that range.

#### ITL p50

**ITL** is inter-token latency: the time between one token and the next, for the typical request. Below it, "1.7 tok/s a stream" turns this into the speed one reader sees. **Generate** is the total across all requests, so the two differ when several run at once.

### In memory and On disk

An Ollama card has two more lines.

- **In memory** lists the models Ollama holds now, with their size and when each unloads, such as "unloads in 4 min".
- **On disk** lists the models it has stored, or says "No models on disk".

### Buttons

| Button | Shows on | What it does |
|---|---|---|
| **Chat with this** | Every card | Starts a new chat with this model. It's on only when the card is **Ready** and this Mac can reach it. Move the pointer over it to see why it's off |
| **Copy curl** | Every card | Copies a command that sends one test question. `curl` is a tool for sending web requests from a terminal. See below |
| **Logs** | Every card | Opens the server's log in **Terminal**. This only reads. It's off for a service SparkBox can't name |
| **Models…** | Ollama | Opens the [Ollama](help:ollama) window |
| **History** | Every card | Opens [History](#history) in this window |
| **Edit & reload…** | Models loaded in SparkBox | Opens the Load page with its settings, so you can change them and load it again |
| **Restart…** | Models loaded in SparkBox, and servers in containers | Stops it, starts it, and checks that it answers |
| **Unload…** | Models loaded in SparkBox | Stops its containers on every node and frees the memory |
| **Stop containers…** | Servers SparkBox found in containers | Stops the containers behind it |
| **Forget** | Servers SparkBox found that have stopped or vanished | Takes the card off this list. It comes back if the server starts again |

For a vLLM load SparkBox started, the restart dialog also offers **Restart and allow cache drop**. It permits a page-cache drop if needed for that attempt, keeps the API key, and does not change future restart permission. See [Restart a model or change its settings](help:restart-or-change-a-model).

**Restart…**, **Unload…**, **Forget** and **Stop containers…** change something. With Admin mode locked, each shows a small lock, and clicking it asks you to unlock first. In look-only mode, they're dimmed.

**Copy curl** has two limits. On a card that says "Reachable from … only", the command works only in a terminal on that node. Its first line says so, as in `# Run this on cell2: it listens on 127.0.0.1 there.`, and so does the message. On a model loaded in SparkBox, the command says `$VLLM_API_KEY`, because SparkBox doesn't copy the key itself. A comment at the top gives the load's id. [Use a model from another computer](help:use-a-model-from-another-computer) shows how to read the key with that id. As copied, the command shows the key in the list of running programs while it runs, so [its test](help:use-a-model-from-another-computer#test-it-from-the-other-computer) changes one line first. An app on this Mac can skip all of that by using the [Server](help:server).

### The right-click menu

Right-click a card for **Chat with this**, **Copy URL**, **Copy curl**, **Show logs** and **History**. The menu also has whichever of **Restart…**, **Edit & reload…**, **Unload…**, **Stop containers…** and **Forget** the card offers. Those that change something show the same small lock.

The small cards on the **Dashboard** have the same menu.

### History

**History** shows one endpoint over time, so you can answer questions like "What was the wait last night, and was the fabric busy then?"

It replaces the cards. To go back, click **Endpoints** at the left of its toolbar.


| Item | What it does |
|---|---|
| **1 h**, **24 h**, **7 d**, **14 d** | How far back to look. It opens on the shortest range that holds all of the endpoint's history |
| **All cluster events** | Marks every alert and change on the cluster on the top chart. When it's off, the chart marks only those on this endpoint and its nodes |

| Chart | What it shows |
|---|---|
| **Generation** | Tokens generated per second, all requests |
| **Time to first token** | The p50 and p95 wait for each minute |
| **RoCE retransmits** | Out-of-sequence packets per second on the fabric of each node. RoCE is how nodes share data over the fabric. A rise here during a slow spell points at the network |
| **GPU utilization** | How busy each node's GPU was |
| **Test token** | How long each test token took |
| **Waiting requests** | Requests queued for a slot or KV-cache room |
| **KV cache** | Share in use. Ollama and llama.cpp have no such chart |
| **Preemptions** | Requests pushed out of the KV cache to make room. Ollama and llama.cpp have no such chart |

All the charts share one time line. Move the pointer over one, and the others follow.

The endpoint's own numbers come from the engine. For Ollama, they come from the requests SparkBox relayed. The GPU and RoCE charts come from the endpoint's nodes.

If [the always-on agent](help:always-on-agent) is installed, it keeps recording while this Mac sleeps, and its numbers fill those hours. Without it, there's a gap.

You may see these lines in place of a chart:

- "No serving numbers recorded for llama-3.3-70b in this range yet: they start once it's Ready."
- "Its nodes report no RoCE counters, so there's no retransmit lane."
- "No test-token times yet: nothing is in memory to test."

## What it changes

Looking at this window changes nothing. **Check now**, **Copy curl**, **Logs** and **History** only read.

SparkBox's test tokens are small requests to your own models. They cost nothing, and they aren't sent while real traffic proves the model works.

| Action | What it changes | What it needs | How to undo it |
|---|---|---|---|
| **Unload…** | Stops the model on every node. Apps using it stop working, and it leaves Chat and the Server | Admin mode | Load it again |
| **Restart…** | Takes the model offline until it answers again | Admin mode | It comes back by itself. If not, see [Fix a load that failed](help:fix-a-load-that-failed) |
| **Stop containers…** | Stops containers SparkBox didn't start. On a model across nodes, the workers stop serving too. You must type the node's name to confirm | Admin mode | Start them again in [Containers](help:containers) |
| **Forget** | Takes a stopped server off the list. Nothing on the node changes | Admin mode | It's listed again if it starts |
| **Edit & reload…** | Stops the model and loads it with new settings | Admin mode | Edit it again |

In look-only mode, these actions are off or show a notice, and they send nothing.

## Related How-Tos

- [Tell whether a model is working](help:tell-if-a-model-is-working)
- [Load a model on one node](help:load-a-model-on-one-node)
- [Restart a model or change its settings](help:restart-or-change-a-model)
- [Unload a model](help:unload-a-model)
- [Chat with a cluster model](help:chat-with-a-cluster-model)
- [Use Ollama](help:use-ollama)
- [Use a model from another computer](help:use-a-model-from-another-computer)

![Endpoints, using synthetic Demo data](img/public-endpoints.jpg)
