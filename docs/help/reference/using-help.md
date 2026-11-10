---
id: using-help
title: Find your way around Help
type: reference
area: reference
summary: "Find what you need: info icons, search, the table of contents, and what the labels on a page mean."
keywords: [help, search, info icon, table of contents, tip, guide, manual]
verified: null
related: [getting-started, glossary, troubleshooting, keyboard-shortcuts]
---

# Find your way around Help

This page lists every way to reach an answer in SparkBox Help, and explains what you see on a Help page.

Help is built into SparkBox. It works with no network and no connection to your nodes. Links to outside websites open in your browser.

## Ways to open Help

| Way in | Where | What opens |
|---|---|---|
| **SparkBox Help** | **Help** menu in the menu bar, or <kbd>⌘?</kbd> | The Help window, where you last were, or its home page |
| An article name | **Help** menu. Every article is in it, under **Getting Started**, **How-To Guides**, **Feature Guide**, **Concepts**, **Reference** or **Troubleshooting** | That article |
| A whole section | **All How-To Guides…** and the like, at the end of those submenus | That section's list of articles |
| Info icon | The small ⓘ beside a label | A Help tip (below) |
| Title bar info button | The ⓘ in any window's title bar | A Help tip about that window |
| Settings heading | The ⓘ beside any Settings section's heading | A Help tip about that section |
| Command palette | Press <kbd>⌘K</kbd>, then type a task such as `download` | Articles appear in the **Help** group. Press <kbd>Return</kbd> to open one |

## Info icons

An info icon is the small ⓘ beside a label. Click it to open a **Help tip**: a short explanation, up to three How-To links, and **Read the full guide**.

- To go deeper, click **Read the full guide**. Help opens at the right heading.
- To close a tip, press <kbd>Esc</kbd> or click anywhere else.
- Clicking an icon does nothing else. It doesn't sort a column, press a button or open a tile.
- Icons stay readable in look-only mode and in read-only forms.


A few places have no icon, on purpose. Sidebar rows and the items in the top bar and status bar have tooltips, and a click opens the right window. Confirm dialogs stay clear so that you can focus on the decision. [The SparkBox window](help:sparkbox-window) and [The top bar and status bar](help:top-bar-and-status-bar) explain those.

## The Help window

Help is a normal SparkBox window. You can move it, resize it, snap it beside the screen you're asking about, or open it in a separate window. In a separate window, <kbd>⌘?</kbd> opens Help in the main SparkBox window.

| Part | Where | What it does |
|---|---|---|
| **Back** and **Forward** | Toolbar | Move through the pages you've read. <kbd>⌘[</kbd> and <kbd>⌘]</kbd> do the same |
| **Help home** | Toolbar | Goes to the home page |
| Breadcrumb | Toolbar | Shows where the page sits. Click a part to go up a level |
| **Search Help** | Toolbar, right | Finds articles as you type |
| **Contents** | Toolbar, in a narrow window | Shows the table of contents when there's no room to keep it open |
| Table of contents | Left | Every article, in folding groups. Use the arrow keys to move and fold |
| The page | Middle | The article you're reading |
| **On this page** | Right, or at the top in a narrow window | The headings of the article. The current one is marked as you scroll |
| **Related articles** | End of the article | What to read next |
| **Previous** and **Next** | End of the article | The articles on either side in the table of contents |


## The contents

The table of contents has seven parts, in the order a new owner usually needs them.

| Part | Use it to | Example |
|---|---|---|
| Getting Started | Learn by doing, from the first launch to your first model | [Getting started with SparkBox](help:getting-started) |
| How-To Guides | Get one real task done, step by step | [Download a model](help:download-a-model) |
| Feature Guide | Learn what every part of a window does | [Settings: Fence](help:settings-fence) |
| Concepts | Understand an idea | [Admin mode](help:admin-mode) |
| Reference | Look up a fact | [Keyboard shortcuts](help:keyboard-shortcuts) |
| Troubleshooting | Fix something | [Troubleshooting](help:troubleshooting) |
| Glossary | Look up a word | [Glossary](help:glossary) |

## Search

Type in **Search Help**. Results appear as you type, best match first.

- **Write it your way.** "How do I load a model" works, because common words are ignored. "Downloading" finds "download". One typo is forgiven.
- **Use your own words.** "Reboot" finds **Restart**, "RAM" finds memory, and "mute" finds **Silence**.
- **Search for a message.** Type a few words of a message SparkBox shows. Most messages are listed in the Troubleshooting pages, and similar ones share an entry.
- **Use the keyboard.** <kbd>⌘F</kbd> jumps to the search field. <kbd>↓</kbd> moves into the results, and <kbd>Return</kbd> opens the first one.

Each result shows where the article sits and a line of text with your words marked. Open a result and the words stay marked in the article. To clear the marks, empty the search field.

If no article has all your words, Help says "None has all those words, so these have some of them." If nothing matches, Help says so and may offer a nearby word.

## What's on a page

### The label above the title

Each article has one job, and the small label above its title says which.

| Label | What the page gives you |
|---|---|
| **Tutorial** | A guided first run, with Demo data where possible |
| **How-To** | The shortest reliable path through one task |
| **Feature guide** | A tour of one window or Settings section |
| **Concept** | An explanation of an idea, with no steps |
| **Reference** | Facts to look up |
| **Troubleshooting** | Symptoms and messages, each with what to do |

### The line about test systems

Some pages show this line under the title: "Built and tested on test systems. Not yet proven on your cluster."

It means the feature works on fake nodes and on a test copy of your nodes' system. It hasn't yet run on your real Sparks. Pages with no such line are not evidence of validation on your devices. No feature is preverified on a new user's cluster. [What's proven on your cluster](help:whats-proven) has the full list.

### Callouts

| Callout | What it means |
|---|---|
| **Note** | Something useful that isn't a risk |
| **Tip** | A faster or better way |
| **Warning** | A problem you can recover from. It says how to undo it |
| **Danger** | A loss you can't undo, or a node that may need someone at the machine. Stop and read it first |

Warnings and dangers always come before the step they guard.

### Text styles

| Style | Meaning | Example |
|---|---|---|
| Bold | A word on screen, exactly as it appears | **Load…**, **Settings › Fence** |
| `Code` | Something you type, a command, a file path, a folder or a port | `~/models`, port `8000` |
| Key | A key to press | <kbd>⌘K</kbd> |
| "Quotes" | A message SparkBox shows, word for word | "Pick at least one node." |

### Links and buttons

| Looks like | What it does |
|---|---|
| A plain link | Moves to another page in Help |
| A word with a dotted underline | Comes from the [Glossary](help:glossary). Click it to read what it means, beside the word, without leaving the page. Press <kbd>Esc</kbd> or click away to close it. A word is marked the first time a page uses it |
| A picture | Click it to see it at full size. Press <kbd>Esc</kbd> or click again to close it |
| A small button such as [Open Models](open:models) | Opens that SparkBox window, leaving Help open |
| **Copy**, on a block of code | Copies the code |

## If you can't find it

1. Search for a few words of the message, or the words on the screen.
2. Open [Troubleshooting](help:troubleshooting), which starts from what you see.
3. Check the [Glossary](help:glossary) for a word you don't know.
4. Check [What's coming](help:whats-coming). The feature may not be built yet.
