---
id: create-an-api-key
title: Create an API key for an app
type: howto
area: ai
summary: Give an app its own SparkBox key, and revoke it when you no longer need it.
keywords: [api key, access key, revoke, token, sbx, sparkbox key, lost key]
verified: false
related: [server, connect-an-app, keychain-and-secrets]
---

# Create an API key for an app

To let an app use the Server, give it its own SparkBox key, so that you can revoke that key alone later.

## Before you start

- Creating or revoking a key changes the Keychain, so SparkBox asks for Touch ID or your Mac password every time. If [Admin mode](help:unlock-admin-mode) is locked, SparkBox unlocks it when you click the button.
- Look-only mode must be off.
- The Server doesn't have to be running.
- A SparkBox key isn't the same as an API key from Anthropic or OpenAI. It works only with the Server on this Mac. [Endpoints, ports and API keys](help:endpoints-and-apis) explains the three kinds.

## Steps

### Create a key

1. In the sidebar, under **AI**, click **Server**. [Open Server](open:server) The **Server** window opens.
2. Find **Access keys**. The table lists the keys you already have.
3. Below the table, click the name field. It starts with `Claude Code` in it. Replace that with the name of the app that will use this key. Use one key for each app, so that you can revoke them one at a time.
4. Click **Create key**. SparkBox asks for Touch ID or your Mac password. Confirm it. A box appears under the field, with the whole key in it and the words "copy this key now; SparkBox won't show it again."
5. Click **Copy** in that box. The key is now on your clipboard.
6. If you'll use this key again, paste it into a password manager, such as the **Passwords** app on this Mac. Don't keep it in a note or a text file.
7. Give the key to your app straight away. [Connect an app to SparkBox](help:connect-an-app#give-terminal-your-key) shows how to do that without saving it in a file or in Terminal's history.

Each key needs its own name. A name that's already in use is refused, so a new key can't replace an old one by accident.

### Revoke a key

1. In the **Server** window, find the key's row under **Access keys**.
2. Click the trash icon at the end of the row. A dialog says "Revoke the key “Claude Code”?" and "Apps using it stop working until they get a new key."
3. Click **Revoke**. SparkBox asks for Touch ID or your Mac password. Confirm it. A message says "Revoked “Claude Code”", and the row goes.
4. Delete the key from anywhere you kept it, such as a password manager. It no longer works, so there's no reason to keep it.

## What you should see

The **Access keys** table has a row with the name you typed, the first 10 characters of the key, and today's date. SparkBox never shows the whole key again.

The text under **Connect your apps** never holds your key. It names the variable `SPARKBOX_API_KEY`, and [Connect an app to SparkBox](help:connect-an-app) shows how to put your key there.

## Why it works

A SparkBox key is `sbx-` and then 40 random characters, kept in the [Keychain](help:keychain-and-secrets), not in a file. Each request to the Server carries a key, and the Server looks for a match. It uses the key's name to label the request in **Requests**, and it doesn't record the key, even a wrong one. Because each app has its own key, revoking one affects that app alone. Your cloud keys never reach the app at all.

## If something goes wrong

- **"Give the key a name, e.g. the app that will use it."** The name field is empty. Type a name.
- **"A key named “Claude Code” already exists. Revoke it first, or use another name."** Nothing changed, and the old key still works. To replace that key, [revoke it](#revoke-a-key) first, then create the new one.
- **You lost a key.** SparkBox can't show it again. [Revoke the old key](#revoke-a-key), create a new one with the same name, then [give it to the app](help:connect-an-app#give-terminal-your-key).
- **The app says "Missing or unknown SparkBox key. Create one in SparkBox › AI › Server."** The key is wrong, revoked or not reaching the app. In the **Server** window, a row in **Requests** reads **(no key)** or **(wrong key)**, with status `401`. Each new Terminal app window starts with no key, so [give it your key](help:connect-an-app#give-terminal-your-key) again. If you no longer have the key, create a new one.
- **The Create key button is dimmed.** Look-only mode is on, and keys can't change. [Turn it off](help:use-look-only-mode).

## Next steps

- [Connect an app to SparkBox](help:connect-an-app)
- [Run SparkBox as a server](help:run-sparkbox-as-a-server)
- [Where your passwords and keys go](help:keychain-and-secrets)
