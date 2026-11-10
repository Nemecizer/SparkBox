# SparkBox Help: style guide

This guide is the contract for every word in SparkBox Help: articles, tips, captions, and the text in Help menus. Writers follow it, critics judge against it, and the help lint enforces the parts a machine can check. If this guide and a habit disagree, this guide wins. If this guide and the running app disagree, the app wins and you report the gap.

Companion files: `manifest.json` (every article, the table of contents, the Help menu) and `INFO-ICONS.md` (where every info icon goes).

---

## 1. Who you are writing for

The reader is a capable adult who has never run a server or an AI model. They own four NVIDIA DGX Spark computers and want to use them with confidence. They can use a Mac well. They don't know what a container, a port, sudo, a token or a KV cache is until you tell them.

The promise of every How-To: **the reader succeeds, and knows why it worked.** The promise of every feature guide: **nothing on that screen stays a mystery.**

Measure yourself against Apple's Mac User Guide (short, calm, exact), Tailscale's knowledge base (prerequisites, steps, how to verify, limitations), Stripe's docs (the reader's goal comes first, and every example works), Docker's docs (concepts beside tasks) and the Proxmox admin guide (complete and honest about limits).

The documentation follows the Diátaxis split. Keep each article to its one job:

| Type | The reader wants to | Your job |
|---|---|---|
| `tutorial` | learn by doing, safely | Lead them by the hand to one success |
| `howto` | get one real task done | Give the shortest reliable path |
| `feature` | understand a screen | Explain every part of it |
| `concept` | understand an idea | Explain it, with no steps |
| `reference` | look up a fact | Be complete and scannable |
| `troubleshooting` | fix something | Match the symptom, give the fix |

A How-To doesn't teach the theory; it links to the concept. A concept doesn't list clicks; it links to the How-To.

---

## 2. The truth rule

**Describe only what the app does today.** Read the code behind the article and open the real screen. Never document from `PLAN.md`, from memory, or from what a feature ought to do.

- Every UI name, default, number and message comes from the code or the running app. Quote messages exactly, character for character, including the app's own punctuation.
- Use synthetic examples in public Help. Do not include private acceptance logs, device identities or owner-specific network details.
- Never write a How-To for something that isn't in the app. Phases P2 to P8 of `PLAN.md` are not built. `whats-coming` is the only article that may mention them.
- Say what the app can't do yet plainly, without apology: "SparkBox downloads to the first node in your list. You can't choose another node yet." Never "unfortunately", never "currently only", never a promise about when.
- If the app contradicts itself (a dialog says one thing and the code does another), write what really happens and report it under `app_defects`. Don't fix app bugs, and don't paper over them.

### The `verified` field

Every article's frontmatter has `verified`, copied from the manifest. Don't change it.

| Value | Meaning | What the reader sees |
|---|---|---|
| `true` | Reserved for independently documented validation; never a claim about a new user's cluster | Nothing extra |
| `false` | It has run only on test systems (fake nodes and the staging container) | One calm line at the top: "Built and tested on test systems. Not yet proven on your cluster." |
| `null` | It doesn't touch the cluster: an idea, a lookup page, or something that happens only on this Mac | Nothing extra |

Don't write that line yourself; the Help window adds it. Don't soften it, repeat it or apologize for it. When the manifest gives an article a `proof` note, work that fact into the body in one plain sentence, where it helps. For example, in `node-window`: "Disk free and boot history have been checked on your cluster. Restart and Shut down have not."

---

## 3. Voice and tone

- **Second person, active voice, present tense.** "SparkBox stops the model" and "You see the card turn gray", not "the model will be stopped".
- **Calm and exact.** You are a knowledgeable friend sitting beside the reader. No marketing, no enthusiasm, no jokes, no exclamation marks.
- **Respectful.** The reader is new to this, not slow. Never say or imply that something is trivial.
- **Direct.** Start with what the reader wants. Lead a sentence with the goal: "To stop a model, click **Unload…**", not "Click **Unload…** to stop a model."
- **Honest about risk.** Say what could go wrong before the step that risks it. Don't frighten; say how likely, how bad and how to undo it.
- **Contractions are fine:** you'll, don't, it's, can't. SparkBox's own text uses them.

---

## 4. Reading level and sentences

These are targets; the editor critic measures them.

- How-Tos, tutorials and tips: about US grade 7 to 8. Feature guides and concepts: at most grade 9. Reference and error pages may name technical terms but still explain them.
- Average sentence: 15 words or fewer. Over 25 words, split it.
- Paragraphs: one to three sentences. One idea each.
- One idea per sentence. Avoid chains of "which", "and" and semicolons.
- Prefer everyday words: *use* not *utilize*, *start* not *initiate*, *about* not *approximately*, *need* not *require*, *show* not *display*, *stop* not *terminate*, *so* not *in order to*.
- Don't start a paragraph with "Note that". Don't write "It's worth noting", "Please", or "As mentioned above".
- No idioms, cultural references or metaphors that need explaining. One plain comparison is welcome when it teaches: "Think of the KV cache as the model's short-term memory for this conversation."

### Banned words

The lint fails on these outside code spans, code blocks and quoted UI text:

`simply`, `just`, `easy`, `easily`, `obviously`, `clearly`, `basically`, `actually`, `of course`, `seamless`, `seamlessly`, `powerful`, `robust`, `leverage`, `utilize`, `etc.`, `and so on`, `in order to`, `please`, `unfortunately`.

Rewrite the sentence instead of hunting for a synonym. "Just click Save" becomes "Click **Save**."

### Explaining terms

Explain every technical term the first time an article uses it, or link it to the glossary with `[term](help:glossary#term)`. Don't assume the reader has read another article. Explain the idea in the fewest words, then use the term:

> A **token** is a small piece of text, about three quarters of a word.

Don't stack jargon: "the RDMA verbs layer" helps nobody. Say what it does for the reader.

---

## 5. Article structure

Every article is one Markdown file at the path the manifest gives. It starts with this frontmatter, then one `#` title that matches the manifest. The Help window builds the title line, the verified line, "On this page" and related articles from it, so don't write those by hand.

```yaml
---
id: download-a-model
title: Download a model
type: howto
area: models
summary: Fetch a model from Hugging Face to your first node and check every file.
keywords: [download, hugging face, get a model, fetch]
verified: true
related: [find-a-model-that-fits, follow-a-download]
---
```

Copy `id`, `title`, `type`, `area`, `summary`, `verified` and `related` from the manifest. You may add `keywords`. You may add `related` ids; never remove one. Heading levels: `##` for sections, `###` inside them. Never skip a level. Headings use sentence case.

The lint checks the required headings below. Use these exact words, in this order.

### `howto`

The first line after the title is the goal in one sentence.

| Heading | What goes in it |
|---|---|
| `## Before you start` | What must be true first: mode, Admin, a saved password, other How-Tos, how long it takes if known, what it costs (disk, memory, money). A list. |
| `## Steps` | Numbered steps (section 6). Use `###` sub-parts only when a task truly has stages. |
| `## What you should see` | How the reader knows it worked: the exact words, state or number on screen. |
| `## Why it works` | Two to five sentences on why the reader can trust the result. No tables or lists. Detail belongs in the feature guide or the concept, with a link. The lint counts the sentences. |
| `## If something goes wrong` | Real messages in quotes, what each means, what to do. Link to the error page. |
| `## Next steps` | One to four links to what a reader usually does next, each a bullet with the link alone or the link and `, to …`: `- [Restore SparkBox from a backup](help:restore-sparkbox), to put a backup in place`. No sentences, no end period. The lint checks the form. |

An optional `## Limits` goes after `## Why it works` when the app can't do something a reader would expect.

### `tutorial`

| Heading | What goes in it |
|---|---|
| `## What you'll do` | The outcome, and about how long it takes |
| `## Before you start` | What's needed. Prefer Demo, so that nothing is at risk |
| `## Step 1: …`, `## Step 2: …` | Stages, each with numbered steps and what the reader sees |
| `## What you learned` | The ideas, in three to five bullets |
| `## Next steps` | Links |

### `feature`

| Heading | What goes in it |
|---|---|
| `## What it's for` | What this is and when you'd use it, in plain words |
| `## Where to find it` | Every way in, each a bullet that starts with where you are: `- **Sidebar:** under **Tools**, click **Updates**.` Use **Sidebar:**, **Menu bar:**, **Keyboard:**, **Command palette:**, **Menu bar icon:**, **Top bar:**, **Status bar:**, **Settings:** or the name of a window. The `open:` link is the last bullet: `- **Link:** [Open Updates](open:updates)`. The lint checks the form |
| `## What's on the screen` | A tour of every area, tab, column, button and field, in the order they appear. Use `###` per tab or area, so that info icons can link to them |
| `## What it changes` | What's read-only, what changes the cluster or this Mac, what it needs (Admin, Touch ID) and how to undo each change |
| `## Related How-Tos` | Links |

### `concept`

| Heading | What goes in it |
|---|---|
| `## In short` | The idea in two or three sentences. A reader who stops here has it |
| `## How it works` | The explanation, with one plain example |
| `## Why it matters in SparkBox` | Where the reader meets this, and what to do with it |
| `## Related` | Links |

Concepts have no numbered steps.

### `reference`

An opening sentence that says what the page lists, then tables or definition lists. Reference pages have no required headings, but every row is complete: name exactly as on screen, what it means, and the default where there is one.

### `troubleshooting`

Message pages open with one sentence on what the page covers. Each message is one entry:

```markdown
### "No fence is open, so changes on the cluster are refused."

**What it means.** One or two sentences in plain words.

**What to do.** The fix, as short steps with links. Say which build or mode the message applies to if it isn't all of them.
```

Use the message as SparkBox shows it, in quotes. Where a message contains a variable, write it the way a reader sees it, with a real example name: "sudo on cell2 didn't accept the saved cluster password." Keep entries in the order a reader is most likely to meet them. Symptom entries ("A node shows Offline") use the same two bold labels, with the symptom as the heading.

---

## 6. How to write a step

A step is **one action**. It says **where**, then **what to do**, then **what you should see**.

Good:

> 1. In the sidebar, under **Workloads**, click **Models**. The **Models** window opens on the **Library** tab.
> 2. Click the **Hugging Face** tab. On the left you see **Recommended for GB10**.

Bad:

> 1. Go to Models and open Hugging Face and search for a model.

Rules:

- Write a window's first step so that a reader who starts cold can follow it. Prefer the sidebar. Name the sidebar group the first time: "under **Workloads**".
- Use an `open:` link beside the words where it saves the reader a search: `[Open Models](open:models)`. It never replaces the written path.
- Use verbs for what the hand does: *click*, *double-click*, *right-click*, *press*, *type*, *select*, *deselect*, *turn on*, *turn off*, *drag*, *move the pointer over*. Never "hit", "tap", "hover" or "navigate to". Section 9 gives the wording for checkboxes, dialogs and logins.
- Name fields and buttons exactly, in bold, including their ellipsis: **Copy to nodes…**.
- If a step asks for Touch ID with Admin mode already unlocked, say so in that step: "SparkBox asks for Touch ID or your Mac password. Confirm it."
- A step that may need an unlock uses the words in [the table below](#one-event-one-sentence).
- If a step can't happen in Demo or in Look only, say so in `## Before you start`.
- Show typed input as code: "In **Name in the API**, type `gemma`."
- Every step that changes something states what changes. A step that risks harm has its warning **before** it.
- Say what the reader should do while they wait, and how they'll know it's over.
- Number one task in one list. If a list runs past 10 steps, break it into `###` stages. Stages of one task continue the numbering, so "step 9" can mean only one step. Sections that are separate tasks or alternatives each restart at 1, and a reference to a step in another section names that section. The lint checks that numbers run in order and that each "step 9" exists.
- Branches stay short: "If you see X, do Y. Otherwise, go to step 5."

### One event, one sentence

The same event reads the same everywhere, in these words. Add what's special about the article after them. The lint refuses the old wordings.

| When | Write |
|---|---|
| A step that may need an unlock | "If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password." |
| `## Before you start`: a change on a node | "Use **Live** data with look-only mode off, and be ready to unlock Admin mode." |
| `## Before you start`: such a change is one part of the article | "To stop a process, use **Live** data with look-only mode off, and be ready to unlock Admin mode." |
| `## Before you start`: a change that works with either data source | "Be ready to unlock Admin mode, with look-only mode off." |
| `## Before you start`: no Admin mode, but it doesn't work in look-only mode | "Look-only mode must be off." |
| `## Before you start`: it only reads | "You don't need Admin mode. Everything here only reads, so it also works in look-only mode." |

---

## 7. Callouts

Use GitHub-style callouts. There are four, and they mean different things.

```markdown
> [!NOTE]
> Something useful that isn't a risk.

> [!TIP]
> A faster or better way.

> [!WARNING]
> **What could go wrong.** What to do to avoid it. How to undo it.

> [!DANGER]
> **What you could lose or break, and that it can't be undone.** When to stop.
```

- **WARNING** is for a problem you can recover from. It always says how to undo it. If there's no undo, it's a DANGER.
- **DANGER** is for data loss, a node that may not come back, or a cut-off you can only fix at the machine.
- Warnings and dangers come **before** the step they guard.
- No more than two callouts per screenful. If everything is a warning, nothing is.
- Don't put steps inside a callout.

---

## 8. Writing names and terms

### Formatting

| What | How | Example |
|---|---|---|
| UI text: buttons, tabs, fields, windows, menu items, states | **Bold**, exactly as on screen, with its ellipsis | **Load…**, **GPU memory share**, **Ready** |
| A path through menus or Settings | Bold, joined with `›` | **Settings › Security & privacy**, **Cluster › Data Source** |
| Things you type; commands, file paths, folders, ports and code | `Code` | `~/models`, `nvidia-smi`, port `8000` |
| Keys | `<kbd>` | <kbd>⌘K</kbd>, <kbd>Esc</kbd>, <kbd>Return</kbd> |
| A message quoted from the app | Double quotes, exact | "Port 8000 is taken" |
| A new term being defined | Bold, once | A **node** is one of your configured computers. |
| Model and repository names | Code | `Qwen/Qwen3.8-27B-FP8` |

A quoted message outside a code span needs `\<` and `\>` around a word in angle brackets, as in `'\<provider\>/\<model\>'`, and `&#96;` for a backtick. Otherwise Markdown reads them as a tag or a code span, and the reader loses the words. The lint checks the page.

Use the Mac's key symbols: ⌘ ⌥ ⇧ ⌃. Write keys in the Mac's modifier order: ⌃⌥⇧⌘.

### Examples

Use the real cluster's names: `cell1`, `cell2`, `cell3`, `gx10-0000`, the `CRS804 DDQ`, `Firewalla Switch X`, `Firewalla Gold`. A node's name is an example the reader can follow, never a fact about the node: the reader can rename it, so never say a name is fixed or that anything depends on it. Use real model names from the app. Never show a real password, token or key; show a fake one only if it's obviously fake, such as `hf_xxxxxxxx`, and only in a screenshot or field description, never in a command.

### Numbers and units

- Numerals for all measurements: `15 minutes`, `121.7 GiB`.
- Write "your nodes" for the cluster as a whole in prose. Use numerals for counts in results and on screen, such as "Passed on all 4 nodes" and **Set up on 4 nodes**.
- Any other count in a sentence: words from one to nine, numerals from 10. "One or two sentences", "five tabs", "14 sections".
- Memory and disk in binary units, as the app shows them: **GiB**, **MiB**, **TiB**. The app's fabric and network speeds use **Gb/s** and **Mb/s**. The fabric is 200G, as the app writes it.
- Times as the app shows them when quoting; otherwise "15 minutes", "2 hours".
- Percent with no space: `75%`. GPU memory share is a fraction on screen: `0.75`. Explain it: "0.75 means 75% of the node's memory."

### Links

| Kind | Form |
|---|---|
| Another article | `[Load a model on one node](help:load-a-model-on-one-node)` |
| A heading in an article | `[GPU memory share](help:choose-load-settings#gpu-memory-share)` |
| A SparkBox window | `[Open Models](open:models)`, `[Open Load](open:models?tab=load)`, `[Open Fence settings](open:settings?section=fence)` |
| A website | `[Hugging Face](https://huggingface.co)`, a page you've confirmed exists |

Heading anchors are the heading in lower case, with spaces as hyphens and punctuation removed. The lint resolves every `help:`, `open:` and image link.

### Images

Add an entry to `docs/help/shots.json` and reference the image as ``. The caption says what the reader should notice. Use images for screens where a beginner gets lost, not for every step. Don't capture images yourself. Images show Demo data and no secrets. One exception: Demo has no Fence screen, and the test build can't draw the SparkBox app's, so the `settings-fence` recipe gives the real screen made-up values, with no real key, address or MAC.

### Diagrams

A concept that's hard to picture gets a diagram. A screenshot shows what the app looks like. A diagram shows how an idea works, so it belongs on a concept page, near the words it explains.

- Add the drawing to `tools/agents/docs-diagrams.mjs`, and a `shots.json` entry with `"source": "diagram"` and a caption. Reference it as `![Short caption](img/<id>.svg)`. Run the script: it writes `img/<id>.svg` and `img/<id>-dark.svg`, so each diagram has a light and a dark version.
- The words in a diagram follow this guide. Use names from section 9 and real numbers from the cluster.
- A diagram adds to the text and never replaces it. A reader who can't see it must still get the idea from the page, and the caption says what it shows.
- Keep it plain: boxes, lines and short labels, with no decoration. A line of the LAN is dashed and a line of the fabric is thick, as they are in **Topology**.

---

## 9. Terminology

One thing has one name everywhere. Names come from the UI (`src/lib/vocabulary.ts` is the source for many of them). The left column is what you write. The right column lists what you never write, except where a real label differs, which the note says.

### The cluster

| Write | Meaning | Don't write |
|---|---|---|
| **node** | One of the four computers. The Nodes sidebar group and most of the app use this | machine, server (for a node), box, host (except "host key", "hostname") |
| **Spark** | The computer as a product, or in a UI name such as **Connect my Sparks**, **Fits on** "2 Sparks". Use it when the hardware matters | DGX box |
| **GB10** | The NVIDIA chip in each Spark. Use only where the hardware decides something | |
| **cluster** | The four nodes with their switches and gateway | farm, rack |
| **the fabric** | The fast 200G network between nodes (window: **Fabric**) | backplane, interconnect |
| **the LAN** | Your ordinary home network (the nodes' 2.5G switch and the 10 GbE Switch X). The **Listen on** option says **LAN** | home net |
| **gateway** | The Firewalla Gold, per **Switches & gateway** | router (except in quoted messages) |
| **switch** | The CRS804 DDQ or Firewalla Switch X | |
| **this Mac** | The Mac running SparkBox. The UI's words | your computer, the host, local machine |
| **unified memory** | The one memory pool the CPU and GPU share | RAM, VRAM |
| **Restart** | Restarting a node. Button: **Restart…** | reboot. The only exception is the menu bar icon's item, **Reboot…**, named exactly |
| **freeze**, **frozen** | A node that's still on but has stopped answering | hang, hung. They stay only where the app says them: the label **Ended without a shutdown (power loss, hang or watchdog)**, the alert **Hung task**, and quoted messages |
| **Shut down** / **Wake** | Buttons **Shut down…** and **Wake** | power off, boot up |
| **Offline**, **Connecting**, **Stale**, **Not updating**, **Error**, **Restarting**, **Off (planned)**, **In maintenance** | Node states, as the app shows them. `status-words` lists them all | down (for a node), dead, unreachable (except in quoted messages) |
| online | A node that's connected and sending readings. Lower case and not bold, because a node's row never shows the word. The sidebar shows its GPU use, such as **GPU 28%**, and the top bar counts it, as in **4/4 online**. To tell the reader what to look for, write "the sidebar shows the node's GPU use, such as **GPU 28%**". Keep **Online** only where it's on screen: a switch's row, the status bar when there's no time to show, and tooltips | **Online** (for a node's sidebar row) |

### Core definitions

Each core term has one definition. Use these words, or a shorter form of them, whenever an article explains the term. Never give a term a second comparison, and never use one comparison for two terms. The glossary uses the same words.

| Term | Definition |
|---|---|
| **model** | A trained AI: a very large set of learned numbers, stored as files, that lets a computer read and write text |
| **engine** | The program that runs a model and answers requests |
| **container** | A program running in its own sealed box |
| **image** | The recipe a container starts from |
| **Docker** | The program that runs containers |
| **port** | A numbered door on a computer that a program answers on. A switch or network card also has ports: the sockets its cables plug into |
| **endpoint** | A running model server you can talk to |
| **token** | A small piece of text, about three quarters of a word |
| **process** | One running program |
| **service** | A program a node keeps running in the background |
| **root** | The account on a node that can do anything |
| **sudo** | A command that runs something as root |
| **SSH** | The secure connection SparkBox uses for everything on a node |
| **the fabric** | The fast 200G network between nodes |

Help links the first use of each term in `terms.json` to the glossary when an article renders, unless the article defines it on the spot. So you don't need to link these by hand. You do still explain a term that a step depends on, in the step itself.

### Modes and safety

| Write | Meaning | Don't write |
|---|---|---|
| **Demo** / **Live** | The two data sources | simulator mode, production |
| **Read-only** | The padlock word when Admin mode is locked | locked mode, safe mode |
| **Admin mode** | Unlocked for changes. Capital A, lower-case m | admin, sudo mode, elevated |
| **unlock** / **lock** Admin mode | The verbs. "Admin mode relocks" for the automatic lock | enable, log in |
| **Touch ID or your Mac password** | The confirmation. Keep both, unless the UI quotes one | authenticate, biometrics |
| **Look only** | The padlock word and the Settings group | |
| **look-only mode** | The same mode in a sentence. Lower case except at the start | read-only mode, safe mode |
| **Mac scope** | Avoid. Write "changes on this Mac" | |
| **cluster password** | The one password for user `sparkuser`, kept in the Keychain. UI: **Cluster password** | SSH password, root password |
| **Keychain** | The macOS Keychain | keyring |
| **sudo** | "Run as root". Explain it the first time | superuser |
| **root** | The all-powerful account on a node. UI: **as root** | admin user |
| **host key** | The key that proves a node is the same node. UI: **Trust…**, **Forget host keys…** | fingerprint (except when you show one) |
| **Fence** | Capital F for the feature and the Settings section; "a fence" for an instance | write gate, allowlist |
| **SparkBox app** | The owner's normal app. Say "the release app" only when you must tell it apart from the test build | production build, the prod app |
| **test build** | The debug build that agents and tests use. The Fence screen's own label may differ; quote it exactly where you name it | dev build (outside quoted labels), debug app |
| **controller lease** | The claim one SparkBox holds on a node while it makes a change | lock, mutex |
| **identity pin**, **re-pin** | SparkBox's record of which machine a node is. UI: **Re-pin identity…** | |

### Models

| Write | Meaning | Don't write |
|---|---|---|
| **model** | The trained AI you run | AI, LLM (except where explained once) |
| **weights** | The model's files, its learned numbers | the binary |
| **engine** | The program that serves a model: **vLLM**, **SGLang**, **Ollama** | runtime, backend, server (for an engine) |
| **Library** | The tab listing model folders on your nodes | repository |
| **model folder** | A model's folder on a node. UI: **Folder**, **Model folder** | path |
| **download** | Fetch from Hugging Face to a node | pull (except for Ollama and Docker, where the UI says **Pull**) |
| **copy** | Fan out to other nodes. UI: **Copy to nodes…** | share, sync (the code's `share` is not a user word) |
| **load** / **unload** | Start or stop serving a model. "A load" is one running start. UI: **Load…**, **Unload…** | deploy, launch, spin up. "Deployment" only in the labels **Deployments** and **1 failed deployment** |
| **serve** | What an engine does with a loaded model | host |
| **head node** | The first node of a load, which answers requests | master |
| **worker** | Another node in a load | slave |
| **endpoint** | A running model server you can talk to. Window: **Endpoints** | instance, service (for a model) |
| **Ready**, **Loading**, **Down**, **Stopped** | Endpoint states, from `vocabulary.ts` | healthy (for an endpoint), up |
| **Name in the API** | What apps put in `model` | served name, alias (aliases are a Server feature) |
| **GPU memory share** | The share of each node's memory the engine reserves | gpu_memory_utilization |
| **Max context**, **context** | How long a conversation the model can hold, in tokens | context window, max model length |
| **Requests at once** | Full-length requests the KV cache must hold together | concurrency, batch size |
| **KV cache** | The memory the model uses to keep track of the text so far. Explain it the first time | attention cache |
| **Memory plan** | The bars on the Load page | fit preview |
| **Listen on** | Which network a load answers on | bind address |
| **tokens per second**, **tok/s** | Speed. Spell it out first | TPS |
| **TTFT** | Time to first token. Spell it out first | latency (alone) |
| **Tensor × pipeline parallel** | The split, as the field names it. **TP** and **PP** only with the field named | sharding |

### Work and windows

| Write | Meaning | Don't write |
|---|---|---|
| **task** | Anything SparkBox is doing, listed in **Tasks** | operation, op |
| **job** | Long work running by itself on a node (a download, a copy), listed in **Models › Jobs** | background process |
| **sidebar** | The list on the left | left panel, nav |
| **top bar** / **status bar** | The bars at the top and bottom of the SparkBox window | header, footer |
| **desk** | The area where SparkBox's own windows sit. The menu item **Fill Desktop** keeps its label | workspace, canvas, desktop (it means the Mac's) |
| **window** | One inner window on the desk | panel, pane (Files has panes), tab (unless it is one) |
| **the Server** | SparkBox's own address for apps on this Mac. **Server** alone is the window | API server, SparkBox Server. Both stay in quoted messages and code |
| **separate window** | A window moved out with **Open in Separate Window** | popout |
| **menu bar** | The macOS menu bar at the top of the screen | menubar |
| **menu bar icon** | SparkBox's icon at the right of it | tray, status item |
| **command palette** | <kbd>⌘K</kbd> | quick open, launcher, spotlight |
| **toast** | Avoid. Write "a message appears" | notification (reserved for macOS and phone alerts) |
| **sheet** | Avoid. Write "a dialog" | modal, popup |
| **Help tip** | The popover from an info icon | tooltip (a tooltip is what appears on hover) |

### Alerts

| Write | Meaning | Don't write |
|---|---|---|
| **alert** | One condition raised by a rule | alarm, issue, problem count |
| **critical**, **warning** | The two severities, in lower case in a sentence. Counts read "1 critical · 2 warnings" | issues, errors (for alerts), severe |
| **incident** | Related alerts counted as one | event (the **Events** tab is something else) |
| **Acknowledge**, **Silence**, **Unsilence** | The actions | mute, snooze, dismiss |
| **Planned** | An alert during work you started | expected |
| **channel** | One place alerts go: this Mac, ntfy, Pushover, webhook, email | destination, integration |
| **page** (verb) | To send an alert to your phone. Explain it the first time; the UI uses **Paging** | ping (the dead-man's switch pings) |
| **always-on agent**, **agent** | SparkBox's small service on a node. UI group: **Always-on monitoring** | daemon, watcher |
| **collector** | The read-only helper at `~/.sparkbox/collector.py` | |
| **Doctor report** | **Settings › Diagnostics**: a text report | diagnostics dump |
| **Doctor** (Fabric tab) | The fabric test. Always say "the **Doctor** tab in **Fabric**" so it can't be confused with the Doctor report | |
| **evidence** | **Capture evidence** in **Settings › Diagnostics** | snapshot |

### Everyday actions

| Write | For | Don't write |
|---|---|---|
| **select**, **deselect** the checkbox | A checkbox. For a list of nodes: "select the node". A switch is **turn on** and **turn off**. The UI's own "Tick" stays in quoted messages | tick, check, uncheck, clear |
| **checkmark** | The ✓ mark | tick, check mark |
| **partway** | Before the end: "stopped partway" | part-way |
| **several-node** | Of a load or model on two or more nodes: "a several-node load" | multi-node (except in quoted messages). Keep `multi-node` in `keywords`, where a reader may type it |
| **keeps going** | Work that goes on by itself: "When Admin mode locks, the load keeps going." | carries on |
| **Move the pointer over** | Showing a tooltip | hover, hold or leave the pointer |
| a dialog named **…** | Naming a dialog: "A dialog named **Restart cell2?** opens." For a name that changes, "a dialog named like **…**" | titled "…", a quoted name |
| **log in**, **login** | The verb, and the noun or adjective. The UI's words: **Log in**, **Login method**, **Cluster login** | sign in, sign-in (except quoted UI text) |
| **dimmed** | A button or menu item you can't use now | grayed out, disabled |
| **gray** | The color | grey |

### Spelling

SparkBox, vLLM, SGLang, Ollama, Hugging Face, Touch ID, DGX Spark, DGX OS, DGX Dashboard, ConnectX-7, MikroTik, RouterOS, Firewalla, WireGuard, AmneziaWG, NVIDIA, Docker, Keychain, macOS, GiB, Wi-Fi, Wake-on-LAN. Use American spelling in Help prose: *gray*, *canceled*, *afterward*.

---

## 10. Writing about Admin mode and the Fence

This changed on 2026-10-30, and old text is wrong. **The SparkBox app needs only Admin mode with Touch ID or your Mac password for changes. It has no fence.** That applies to everything it does, including downloads, loads, updates and restarts.

- Never tell the reader to open a fence to do something in the SparkBox app. Never list a fence in `## Before you start`.
- The Fence is for the test build only: the build agents and tests use. In the SparkBox app, **Settings › Fence** is the place where the owner lets the test build make changes on chosen nodes for a limited time.
- Fence errors in `errors-admin-and-safety` are marked as test-build messages. Say so in **What to do**.
- Look-only mode, the identity pin, the controller lease and the activity log apply to both builds.
- Read `src-tauri/src/live/exit.rs`, `src/lib/fence.ts` and `src/windows/settings/FenceSection.tsx` as they stand when you write. The foundation change lands before you start. README and the acceptance playbooks may describe the older design.
- Don't mention agents, rounds, sessions, phases or commits anywhere in Help. The reader owns the app; those words mean nothing to them. Write "test build", not "agents".

---

## 11. Safety and secrets

Help teaches habits. These rules are absolute.

- **Never teach anyone to put a secret on a command line, in a file, in a shell history, or in a chat.** Passwords and keys go in the Keychain through SparkBox's own fields. The Server's text names the variable `SPARKBOX_API_KEY` and never holds a key. The one standard exception is a key the reader must paste into a third-party app's own setting; say where it ends up and how to revoke it.
- Before any step that changes a node, say what changes, what goes offline, and how to undo it. If it can't be undone, use a DANGER callout.
- State the real consequence of a mistake. A Spark that runs out of unified memory can freeze instead of failing cleanly. A node may need someone at the machine. Say so.
- Don't teach reaching around SparkBox's guardrails: overrides such as **Override memory guardrail** are documented with their risk, and never as the normal path.
- Every security statement must be true of the code: where a secret lives, who can read it, what's logged. Check before you write. For example, a channel copied to node agents puts its secrets on every node, and the app says so.
- Secrets never appear in images, examples or quoted logs.

---

## 12. Tips

An info icon opens a Help tip: a title, a short plain explanation, up to three How-To links, and **Read the full guide**. A tip is either an article's `summary` or a named entry in `docs/help/tips.json`.

```json
"models.load.gpu-share": {
  "title": "GPU memory share",
  "body": "How much of each node's memory the engine reserves when it starts. 0.75 means 75%. Higher leaves more room for the model and less for everything else.",
  "article": "choose-load-settings#gpu-memory-share",
  "howto": ["load-a-model-on-one-node"]
}
```

- Body: one to three sentences, at most 60 words. Say what it is and what to do with it. Tips obey the same language rules as articles.
- A summary doubles as a tip, so write it as a sentence that makes sense alone. At most 120 characters. Summaries are in the manifest; if you find one that's wrong, fix it there and in your frontmatter.
- Every How-To and tutorial is offered by at least one icon, and every window's guide offers a How-To. A tip shows only its first three, from `howto` or the article's `related`, so put the likeliest first. The lint checks both.
- `tips.json` and `shots.json` are shared by every writer. Add only entries under your package's prefix, with small edits. Never rewrite either file. If an edit fails because the file changed, read it again and retry.

Prefixes: `start.`, `desk.`, `mon.`, `models.`, `ai.`, `ops.`, `ref.`.

---

## 13. Checklist before you finish an article

1. Is every UI name, number and message checked against the code or the running app?
2. Does the title promise what the page delivers?
3. Could a person who has never seen SparkBox follow every step cold, from wherever they start?
4. Does each step say where, what, and what they see next?
5. Is every warning before its step, and does it say how to undo?
6. Is every term explained or linked?
7. Is anything here not in the app today? Remove it.
8. Are the required headings present, in order?
9. Are the words from the terminology table, and none from the banned list?
10. Do `npx vitest run src/help` and every link pass?
