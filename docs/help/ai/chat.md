---
id: chat
title: Chat
type: feature
area: ai
summary: Talk to any model SparkBox can reach, with saved conversations, a system prompt and speed readouts.
keywords: [chat, system prompt, temperature, effort, regenerate, conversation, thinking, model picker, stop]
verified: false
related: [chat-with-a-cluster-model, chat-with-a-cloud-model, providers, tokens-and-context]
---

# Chat

## What it's for

**Chat** is where you talk to a model. It works with a model on your cluster, with a cloud model from Anthropic or OpenAI, and with any other server you've added in [Providers](help:providers). With Demo data, it also has a simulated model, `echo`, that needs no setup.

Use it to try a model you've loaded, to compare two models on the same question, or to get an answer without leaving SparkBox.

Chat keeps your conversations, and every version of them. If you edit a question or ask for another reply, the earlier one stays, and you can switch back to it.

## Where to find it

- **Sidebar:** under **AI**, click **Chat**.
- **Menu bar icon:** click it, then click **Chat…**.
- **Command palette:** press <kbd>⌘K</kbd>, type `chat`, and press <kbd>Return</kbd>. To pick a Ready model by name, type `chat with`.
- **Endpoints window:** click **Chat with this** on a card. That starts a new chat with that model.
- **Link:** [Open Chat](open:chat)

## What's on the screen

The toolbar runs across the top. Your saved conversations are on the left. The conversation you're in fills the rest, with the message box at the bottom.

### Model picker

The button at the left of the toolbar shows the model in use and where it runs. Click it to open a list of every model SparkBox can reach.


| Group | What's in it |
|---|---|
| **Recent** | The last models you picked with this data source |
| **Cluster** | Models on your nodes, and servers you added by hand. Each shows its state and, for some, its context, such as "32k ctx" |
| **Anthropic** | Claude models. "no key" means you need to [add an API key](help:add-a-cloud-provider) first |
| **OpenAI** | GPT models. They appear once you've added a key |
| **Demo** | The `echo` model. Only with Demo data |

Type in **Search models** to narrow the list. Press <kbd>↓</kbd> and <kbd>↑</kbd> to move, <kbd>Return</kbd> to pick and <kbd>Esc</kbd> to close.

A dimmed model can't be picked right now. Beside it you see why, such as **Loading 42%**, **Idle** or **Not answering**. A model that stopped answering stays listed for 2 minutes, in case it comes back.

You can change the model partway through a conversation. Your next message goes to the new model, along with the conversation so far. Each reply shows which model wrote it.

### Effort

This choice appears for Claude and for OpenAI's reasoning models. It sets how hard the model thinks before it answers.

| Choice | Offered for |
|---|---|
| **Default effort** | Both. Uses the provider's own default |
| **low effort**, **medium effort**, **high effort** | Both |
| **xhigh effort**, **max effort** | Claude |

Higher effort can give a better answer on a hard question. It takes longer and, because it uses more tokens, it costs more.

### Temperature

This field, marked **temp**, appears for models on your cluster and other models that accept it. It sets how much the model varies its wording, from `0` to `2`. Low is steady and repeatable. High is more varied, and can drift. Leave it empty to use the model's default.

Models that take an effort don't take a temperature. Some older Claude models take neither, so you see neither control.

### System

Click **System** to open a box above the conversation. A **system prompt** is a standing instruction for the model, such as "Answer in one short paragraph." The button stays highlighted while a prompt is set.

- Before the first reply, the box sets the prompt for the whole conversation.
- After the first reply, a change applies from your next message. Earlier replies keep the prompt they had. A line in the conversation marks the place: "System prompt changed from here: …", or "System prompt removed from here".

### New chat

**New chat** starts a blank conversation with the same model. It's off while a reply is arriving.

### Conversation list

Each saved conversation shows its title, its model and when you last used it. The title is the start of your first message. Click one to open it. SparkBox goes back to the model and settings it last used.

To delete one, move the pointer over it and click the trash icon. A dialog asks you to confirm.

The list belongs to the data source. Chats you start with Demo data don't appear with Live data, and the other way round.

### The conversation

While a reply arrives, you first see "Waiting for the first token…". Then the words appear as the model writes them.

Each reply has these parts:

| Part | What it does |
|---|---|
| Model line | The model that wrote it, and its provider |
| **Thinking** | Appears when the model shares its reasoning. Click it to open or close it. It opens by itself until the reply starts |
| Stats line | How the reply went. See below |
| **Copy** | Copies the reply as Markdown. A code block in a reply also has its own copy button |
| **Regenerate** | Asks the same question again. It's on the last reply only. The current reply stays as another version |

Each question you asked has a pencil icon. Click it to change the question and send it again. Press <kbd>Return</kbd> to send, or <kbd>Esc</kbd> to cancel. The original stays as another version.

Where there are several versions, you see "‹ 2 / 3 ›". Click the arrows to move between them. Only the version on screen goes to the model with your next message.

### The stats line

The line under a reply reads like this: "0.4 s to first token · 52.0 tok/s · 300 tokens · 6.2 s total · 12 in · finished".

| Part | What it means |
|---|---|
| **to first token** | How long the reply took to start |
| **tok/s** | Tokens per second. This appears only when the reply was long enough to measure. A short reply reads "70 tokens in 1.6 s" |
| **tokens** | How long the reply was. A **token** is about three quarters of a word |
| **total** | How long the whole reply took |
| **in** | How many tokens were sent in, including the conversation so far |
| Last word | How the reply ended |

| Last word | What it means |
|---|---|
| **finished** | The model ended the reply itself |
| **hit the length limit** | The reply reached the longest length allowed, and was cut off |
| **ran out of context** | The conversation no longer fits in the model's context. Start a new chat |
| **stopped by you** | You stopped it |
| **declined** | The model refused the request |
| **wants to use a tool** | The model asked for a tool. Chat has no tools, so the reply ends there |

### The message box

Type in the box at the bottom. It grows to 8 lines, then scrolls.

| To do this | Do this |
|---|---|
| Send | Press <kbd>Return</kbd>, or click the arrow |
| Start a new line | Press <kbd>⇧Return</kbd> |
| Stop a reply | Press <kbd>Esc</kbd>, or click the square. Whatever arrived so far is kept |

The arrow is off until you've typed something and a model is chosen. For a cloud model with no key, the box says "Add an API key first, or pick another model". A notice above it offers **Open Providers**.

SparkBox asks before it closes the window if a reply is still arriving, or if you've typed a message and not sent it.

When a connection drops or a model reports an error, Chat keeps the text that arrived and marks the reply **interrupted**. A reply that goes quiet is marked **stalled**. The status stays with the saved reply when you reopen the conversation.

Wait for the model to be **Ready**, then choose **Regenerate** or start a **New chat**. SparkBox does not resend or resume the old answer on its own. If no text arrived, it shows the error without adding an empty answer.

If saving fails, the reply remains visible with a separate warning. Click **Copy** before leaving the conversation; reopening cannot recover an unsaved reply.

A November 7 live retest kept the partial two-node Qwen reply after a brief VPN interruption. Closing Chat, reopening it and selecting the saved conversation preserved the answer and its **interrupted** label. A fresh Chat and clean unload also passed. The earlier November 6 loss remains recorded. See [What's proven on your cluster](help:whats-proven).

## What it changes

Chat changes nothing on your nodes. It needs no Admin mode, and it asks for no Touch ID.

| What | What happens | How to undo it |
|---|---|---|
| Sending to a cluster model | The question goes from this Mac to that node. An Ollama may read the model into memory | Nothing to undo |
| Sending to a cloud model | The whole conversation goes to that company, and you pay for it. See [Chat with a cloud model](help:chat-with-a-cloud-model) | You can't recall a message |
| Saving | Each conversation is a file on this Mac. It's plain text, not in the Keychain, so don't put passwords or keys in a chat. [Where SparkBox keeps things](help:files-and-folders) has the folder | Delete the conversation |
| Deleting a conversation | Removes its file and every version in it | You can't undo this |

In look-only mode, Chat doesn't send. **Send** is dimmed, <kbd>Return</kbd> does nothing, and what you typed stays in the box. Asking again and editing a message are dimmed too. You can still read saved conversations.

## Related How-Tos

- [Chat with a cluster model](help:chat-with-a-cluster-model)
- [Chat with a cloud model](help:chat-with-a-cloud-model)
- [Add a cloud provider](help:add-a-cloud-provider)
- [Use Ollama](help:use-ollama)

![Chat, using synthetic Demo data](img/public-chat.jpg)
