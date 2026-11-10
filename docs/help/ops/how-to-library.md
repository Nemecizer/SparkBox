---
id: how-to-library
title: How To library
type: feature
area: tools
summary: Find NVIDIA playbooks, follow guided steps, and keep your own reviewed instructions on this Mac.
keywords: [how to, skills, playbooks, nvidia, guides, instructions, learning]
verified: false
related: [use-and-update-a-playbook, load-a-model-on-one-node, admin-mode]
---

# How To library

## What it's for

**How To** brings NVIDIA's playbooks into SparkBox. Each current guide has a plain-language walkthrough, expected results and points to check before starting. The full original instructions are available beside it.

A skill is a guide with the SparkBox tools that can help you carry it out. Some steps can be automated. Others need an account, physical equipment or a reviewed installation.

The library includes 136 guide folders and 293 text documents. Of those guides, 69 appear in NVIDIA's current index at the bundled revision. Older versions, publishing examples and guides for other hardware remain available as references.

## Where to find it

- **Sidebar:** under **Tools**, click **How To**.
- **Menu bar:** choose **Go › How To**.
- **Command palette:** press <kbd>⌘K</kbd>, type `playbooks`, and press <kbd>Return</kbd>.
- **Link:** [Open How To](open:howto)

## What's on the screen


### Library and search

Search by task, model or topic. The first filter starts at **Current Spark guides**. Choose **All current guides**, **Entire library**, **Other hardware and references** or **Favorites** to widen or narrow the list. The second filter selects a topic.

The star beside a guide saves it as a favorite. A hardware label tells you whether the guide applies to Spark, Station or another platform. A Station procedure is not a Spark procedure, even when both use NVIDIA GPUs.

### Walkthrough

Read the warning or preparation note first. **Steps** gives the route through the task. **What you should see** explains how to judge its result. **If something goes wrong** tells you where to start checking.

Select a step's checkbox to record your own progress. Write results in **Notes for this guide**, then click **Save progress**. These marks record what you reviewed; they do not certify that a hardware test passed. Keep passwords, API keys and private data out of notes.

**Full instruction outline** lists NVIDIA's detailed steps. Click **Read detailed NVIDIA instructions** to see their contents.

### SparkBox actions

Select the nodes you want to inspect, then click **Check selected nodes**. This reads Docker, Python, free model-storage space, the configured image and fabric facts. A failed read stays incomplete. It does not run a GPU test, install software or prove that the full playbook works.

Thirteen guides offer **Prepare image…**. A container image is the packaged software a workload runs inside. Choose the reviewed image and enter its estimated unpacked size. The review names the image and selected nodes before any download begins.

Image preparation uses Admin mode and the existing node jobs. It checks space and records the downloaded image's identity. It does not start a container, load a model or install host packages. A node with an unfinished protected system update is held back.

Other buttons open the named SparkBox tool. Opening **Models and downloads**, for example, opens **Models › Load**. Use that tool's own controls, reviews and health checks to continue. Source text and AI replies cannot add actions to these buttons.

### NVIDIA source

Choose a **Bundled document** to read the original instructions or license notices. **Read a section** narrows a long document to one section. The bundled revision and text digest identify the source you are reading.

Images, scripts and notebooks are linked at that source revision. They need internet access to open. SparkBox does not download or execute them while you read. A copied command still needs review for the intended node, paths and software versions.

A fetched source appears above the bundled original. Fetching it leaves your local edition unchanged. You can compare both texts before updating your own instructions.

### My edition

**Edit local edition** opens your instructions as editable text. **Compare with saved text** shows the saved and proposed texts side by side. **Save local edition** keeps a new edition; **Discard draft** removes only unsaved text.


Choose **AI provider** and **Model** yourself. **Section to revise** selects the local text to change. **NVIDIA reference section** selects its evidence. Enter what you want improved, then click **Review AI request…**.

The review names the provider and its address before sending. A configured local model is supported. A cloud provider receives the selected text and may charge for use. Notes, node facts and credentials are not added to the request.

Each selected section can contain up to 24,000 characters. Larger documents need smaller sections; SparkBox does not silently shorten them. A canceled, cut-off or failed reply cannot become the proposed edition. **Cancel AI** stops the request.

Read the finished draft before saving. AI can miss requirements or write incorrect commands. It cannot run its instructions or save them by itself. **Saved history** keeps earlier editions; **Restore as a new draft** lets you review an older text without deleting newer history.

### Source updates

**Check NVIDIA for updates** reads the public NVIDIA repository when you request it. There is no background polling. SparkBox records one fixed revision and compares its guide folders with the bundle.

The list shows added, changed and removed guides. A script or other asset change also marks its guide as changed. **Fetch source for review** downloads that guide's text and checks its identity against the recorded revision. It does not install anything.

New guides enter as references without execution actions. Updating their text, including with AI, does not change that rule.

### Saved-copy conflicts

Two windows cannot silently replace each other's work. If saving reports a changed guide, choose **Reload saved copy**. This keeps your open edition draft but reloads saved notes and checklist selections. Compare the draft with the reloaded text before saving again.

## What it changes

Reading the library changes no node. Notes, favorites, checklists, fetched source and editions are stored on this Mac. Demo and Live have separate copies. Demo source checks, AI replies and node actions are simulated.

Image preparation downloads to the selected nodes after its review. Unlock Admin mode when asked. A fresh Touch ID or Mac-password confirmation may also be required. Follow or cancel downloads in **Models › Jobs**. Keep an image that another workload needs; removing an image is a separate decision.

The bundled text remains unchanged. Local storage keeps up to 50 editions and 20 fetched source revisions per guide, with a 32 MiB library limit. Reaching a limit refuses the new save instead of deleting history. Keep a backup of SparkBox's application data before reorganizing it. A damaged or newer-format library is preserved and refused rather than replaced with an empty one.

NVIDIA's original instructions can include changes to networks, drivers, packages or services. For this cluster, use SparkBox's own reviewed maintenance and ownership controls. In particular, finish an outstanding protected-update checkpoint before starting new work on that node.

## Related How-Tos

- [Use and update a playbook](help:use-and-update-a-playbook)
- [Load a model on one node](help:load-a-model-on-one-node)
- [Admin mode](help:admin-mode)

![How To library, using synthetic Demo data](img/public-how-to-library.jpg)
