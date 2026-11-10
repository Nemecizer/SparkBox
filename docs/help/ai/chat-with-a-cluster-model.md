---
id: chat-with-a-cluster-model
title: Chat with a cluster model
type: howto
area: ai
summary: Talk to a model running on your nodes, in SparkBox's Chat window.
keywords: [chat, talk, ask, conversation, prompt, cluster model, try a model, first question]
verified: false
related: [chat, tell-if-a-model-is-working, load-a-model-on-one-node, tokens-and-context]
---

# Chat with a cluster model

To talk to a model on your own nodes, open it from its card in **Endpoints** and type a question.

## Before you start

- You need a model that's serving. If you don't have one, [load a model on one node](help:load-a-model-on-one-node) first.
- Its card in **Endpoints** must say **Ready**. [Tell whether a model is working](help:tell-if-a-model-is-working) explains that word.
- This Mac must be able to reach it. You need to be at home or on the [VPN](help:connect-the-vpn). This Mac can't use a model loaded with **Listen on** set to **Fabric only**. It can't use an Ollama that listens on its node only, either.
- Look-only mode must be off, because Chat doesn't send while it's on.
- You don't need Admin mode, and a chat with your own model costs nothing per message.
- To practice first, switch to [Demo data](help:switch-between-demo-and-live). The simulated models work the same way.

## Steps

1. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) You see one card for each model server.
2. Find a card that says **Ready**, with no "Reachable from" line on it.
3. On that card, click **Chat with this**. The **Chat** window opens on a new chat. The model's name shows at the left of the toolbar.
4. Click the box at the bottom that says "Message (Enter to send, Shift+Enter for a new line)".
5. Type a question, such as `What is the capital of France?`
6. Press <kbd>Return</kbd>. Your question moves up, and you see "Waiting for the first token…".
7. Wait for the words to appear. "Waiting for the first token…" stays on screen until the model starts to write.
8. To stop a reply partway, press <kbd>Esc</kbd> or click the square at the right of the box. Whatever arrived so far stays.
9. To go on, type your next message. The model sees the whole conversation so far.

You can also start from **Chat** itself. In the sidebar, under **AI**, click **Chat**. Click the button at the left of the toolbar, then click a model under **Cluster**.


## What you should see

- The reply appears word by word, under a line with the model's name and where it runs, such as "llama-3.3-70b · vLLM on cell1 :8000".
- A line under the reply shows how it went, such as "0.4 s to first token · 52.0 tok/s · 300 tokens · 6.2 s total · 12 in · finished". **finished** means the model ended the reply itself.
- The conversation appears in the list at the left. It's saved on this Mac.
- In **Endpoints**, the card's **Generate** number rises while the reply is arriving.

## Why it works

When a model turns **Ready**, SparkBox adds it to the list Chat can use. Your message goes straight from this Mac to the head node, the node that answers requests. The model has no memory between requests, so each time you send, SparkBox sends the whole conversation you can see. That's why a long chat gets slower, and why it has to fit in the model's [context](help:tokens-and-context). Nothing goes outside your network, and away from home the request travels through your VPN.

## Limits

- A November 7 two-node Qwen retest preserved the partial answer and its **interrupted** label after closing and reopening Chat. Fresh Chat and clean unload passed. Home LAN access remained available; a complete remote outage is still untested. Reconnection does not resume or resend the old answer.
- Chat handles text only. You can't attach files or pictures.
- You can't limit how long a reply can be.
- Chat can't use tools, such as searching the web.
- A reply that sends nothing for 2 minutes counts as stopped.

## If something goes wrong

- **Chat with this is dimmed.** Move the pointer over it to see why. "Not ready" means the model isn't **Ready** yet. "Reachable from cell3 only" or "Not reachable from this Mac" means this Mac can't reach it.
- **The model is dimmed in the list.** It's still starting, or it stopped. The reason shows beside it, such as **Loading 42%**.
- **"vLLM on cell1 :8000 isn't answering right now. A message may fail until it's back."** It stopped being Ready in the last 2 minutes. Check its card in **Endpoints**.
- **"The model stopped responding (is cell2 still up?)"** Nothing came back for 2 minutes. A Spark that runs out of unified memory can freeze rather than fail. Check that the sidebar shows the node's GPU use, such as **GPU 28%**, not a red word. If it shows **Offline**, follow [Recover a frozen node](help:recover-a-frozen-node). Otherwise, read [Tell whether a model is working](help:tell-if-a-model-is-working).
- **"The connection closed before the reply finished (is cell2 still up?)"** The connection ended partway. Chat keeps the partial answer and marks it **interrupted**, including in saved history. Check **Endpoints**; once Ready, use **Regenerate** or **New chat** to ask again.
- **"The reply is visible but could not be saved."** Click **Copy** before leaving the conversation. The warning includes the save error; an unsaved answer will not be there when you reopen it.
- **"Couldn't reach vLLM on cell1 :8000 …"** This Mac can't get to the node. Check that you're at home or on the [VPN](help:connect-the-vpn).
- **Send is dimmed, and its tooltip reads "Look-only · turn off in Settings › Security & privacy".** [Turn off look-only mode](help:use-look-only-mode), then send again. What you typed stays in the box.
- **The reply stops with "ran out of context".** The conversation no longer fits in the model's context. Click **New chat**.
- **The reply stops with "hit the length limit".** The model reached the limit on how long one reply can be. Send `Continue` to get the rest.

[Chat, Providers and Server messages](help:errors-ai) lists more messages.

## Next steps

- [Tell whether a model is working](help:tell-if-a-model-is-working), to read its speed
- [Benchmark a model](help:benchmark-a-model), to measure it properly
- [Run SparkBox as a server](help:run-sparkbox-as-a-server), to use it from your other apps
- [Chat](help:chat), to see everything in the window
