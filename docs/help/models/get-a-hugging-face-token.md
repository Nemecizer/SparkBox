---
id: get-a-hugging-face-token
title: "Get a Hugging Face token"
type: howto
area: models
summary: "Create a free access token and save it in SparkBox, so that you can download gated models."
keywords: ["token", "hugging face", "gated", "license", "401", "needs permission", "permission to download", "access to a model"]
verified: false
related: [settings-models, download-a-model]
---

# Get a Hugging Face token

To download a gated model, create a free access token on Hugging Face and save it in SparkBox.

An **access token** is a long password that lets an app act for your Hugging Face account. A **gated** model is one whose owner asks you to accept a license before you can download it. Many popular models are gated. SparkBox marks them **gated** on the **Hugging Face** tab.

## Before you start

- You need a Hugging Face account. It's free, and you can create one at [huggingface.co](https://huggingface.co). This takes a few minutes.
- Saving the token changes something on this Mac, so SparkBox asks for Touch ID or your Mac password, even with **Demo** data.
- Look-only mode must be off. See [Use look-only mode](help:use-look-only-mode).
- The token costs nothing.

## Steps

### Create the token

1. In your web browser, log in to [Hugging Face](https://huggingface.co).
2. Go to [your Access Tokens page](https://huggingface.co/settings/tokens).
3. Click **New token**. The form may say **Create new token**, because the website's labels change from time to time.
4. For the role or token type, choose **Read**. SparkBox only needs to read models, and a read token can't change your account's repositories.
5. Type a name you'll recognize, such as `SparkBox`, and create the token.
6. Copy the token. It starts with `hf_`. Treat it like a password: don't paste it into a chat, a file or a command.

### Save it in SparkBox

7. In SparkBox, open **Settings › Models**. [Open Models settings](open:settings?section=models) The **Hugging Face** group has an **Access token** field.
8. Click the field and paste the token.
9. Click **Save**. SparkBox asks for Touch ID or your Mac password. Confirm it. The message "Saved to Keychain." appears, and the field then reads "Saved in Keychain".

### Accept a gated model's license

10. In your web browser, open the model's page on Hugging Face. On the **Hugging Face** tab in SparkBox, you can click the model's name to open it.
11. Read the license and accept it. Some owners approve each request by hand, so access may not be immediate.

## What you should see

The **Access token** field reads "Saved in Keychain". Back on the **Hugging Face** tab, click a gated model. Its facts and fit table appear, and **Download to cell1** is ready to use.

## Why it works

SparkBox keeps the token in the macOS Keychain, not in a file, and sends it to Hugging Face when this Mac searches or reads a model. For a download, SparkBox passes the token to the node for that job only, over the secure connection. The job keeps it in a file only your account can read, out of Docker's records and the job's own, and removes that file when the job ends. [Where your passwords and keys go](help:keychain-and-secrets) has the details. The license is tied to your Hugging Face account and the token proves which account is asking, so you need both.

## If something goes wrong

- **"… is gated or private. Accept its license on huggingface.co and add a Hugging Face token in Settings › Models."** Hugging Face refused the request. Check that you accepted the license with the same account that created the token, and that the token is saved.
- **The download job fails with "hf download failed …" and a 401 or 403 in its details.** The token doesn't have access to this model. Check the license, then see [Follow a download](help:follow-a-download) to start again.
- **You pasted the token somewhere public.** Open [your Access Tokens page](https://huggingface.co/settings/tokens), click **Manage** beside the token, and delete or refresh it. Then save a new one in SparkBox.
- **You want to take the token off this Mac.** In **Settings › Models**, click **Remove** beside the field. SparkBox asks for Touch ID or your Mac password.

## Next steps

- [Download a model](help:download-a-model)
- [Find a model that fits](help:find-a-model-that-fits)
- [Where your passwords and keys go](help:keychain-and-secrets), to see how SparkBox stores what you give it
