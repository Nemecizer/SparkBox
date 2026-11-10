---
id: use-and-update-a-playbook
title: Use and update a playbook
type: howto
area: tools
summary: Read a guide, compare NVIDIA changes, and save a reviewed local edition with optional AI help.
keywords: [how to, skills, playbooks, nvidia, ai, update, local edition]
verified: null
related: [how-to-library, providers, chat]
---

# Use and update a playbook

Choose a guide, compare its source and keep your own reviewed instructions.

## Before you start

- Reading bundled guides and saving notes work offline.
- Use **Demo** to practice. Its source check and AI draft are simulated.
- For a real source check, use **Live** data and an internet connection. This reads public NVIDIA text; it does not change a node.
- For real AI help, configure a provider and an available model in **Providers**. The request review names its destination. A cloud provider receives the selected text and may charge for use.
- Keep credentials and private information out of the text you send to AI.

## Steps

### Choose and read a guide

1. In the sidebar under **Tools**, click **How To**. The library opens with **Current Spark guides** selected.
2. In the search field, type a task such as `vllm`. Click the guide you want. Read its platform label and preparation note.
3. Click **NVIDIA source**. Read the prerequisites and hardware limits. For a long document, use **Read a section** to select one part.
4. Click **Walkthrough**. Read its steps and expected result. Write a short note in **Notes for this guide**, then click **Save progress**. Your note is saved on this Mac.

### Compare source changes

5. Click **Check NVIDIA for updates**. Wait for the comparison in **Source updates**. It names the revision and lists changed guide folders.
6. Beside a changed guide, click **Fetch source for review**. Read the fetched text in **NVIDIA source** and compare it with the bundled original. Your notes and local edition remain in place.

If the guide has not changed, continue with its bundled source. A change to a supporting script can mark a guide as changed even when its main text is the same.

### Review a local edition

7. Click **My edition**, then **Edit local edition**. The text becomes editable. Make the changes you want, or continue to the optional AI step.
8. To use AI, choose **AI provider**, **Model**, **Section to revise** and **NVIDIA reference section**. Enter your request in **What should AI improve?**, then click **Review AI request…**. Read the destination and click **Create draft** only if you want to send those sections.
9. Wait for the draft. Read **Saved text** beside **Proposed text**. Check commands, required versions, hardware limits, expected results and cleanup. Edit any inaccurate text before continuing.
10. Click **Save local edition**. The new edition appears in **Saved history**. Close and reopen **How To** to confirm your edition is still there.

## What you should see

**Walkthrough** shows **Your local edition** after you save one. **Saved history** lists its author and source revision. The bundled original remains available in **NVIDIA source**.

Nothing runs on a node when you fetch, edit or save instructions. Use the separate SparkBox action controls when you are ready to perform a supported task.

## Why it works

The original source, local instructions and executable controls are separate. A source check records a fixed revision. Local saves keep previous editions and refuse a conflicting save from another window. AI output remains a draft until you save it.

## If something goes wrong

If a source check fails, keep using the offline library. A network or GitHub rate-limit error leaves your saved guides intact. Try again later; repeated clicks do not make a rate limit clear faster.

If a section is too large, select a smaller heading in each section field. SparkBox refuses oversized requests instead of dropping part of the document. Work through the remaining sections separately.

If AI stops early or fails, the saved edition stays unchanged. Retry with a smaller section or another available model. A provider must be enabled and answering to appear in the list.

If saving reports that another window changed the guide, choose **Reload saved copy**. Your open edition draft stays available, while notes and checklist selections reload. Compare before saving again.

To recover an earlier edition, open it in **Saved history** and click **Restore as a new draft**. Review and save it as another edition. Newer history stays intact.

## Next steps

- [How To library](help:how-to-library)
- [Providers](help:providers)
- [Chat](help:chat)
