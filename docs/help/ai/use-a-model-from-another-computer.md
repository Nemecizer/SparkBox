---
id: use-a-model-from-another-computer
title: Use a model from another computer
type: howto
area: ai
summary: Reach a model on your cluster from a laptop, phone or other device on your network, with its address, name and key.
keywords: [another computer, other computer, different computer, second computer, laptop, phone, tablet, use from my phone, use from my laptop, use from another computer, use a model on my network, reach a model from my network, lan, load key, sbk key, find the api key, get the api key, where is the api key, vllm_api_key, copy curl, base url, openai compatible]
verified: false
related: [endpoints, endpoints-and-apis, restart-or-change-a-model, keychain-and-secrets, connect-the-vpn, connect-an-app]
---

# Use a model from another computer

To use a model on your cluster from another computer, give an app on that computer the model's address, its name and its key.

This works from a laptop or a desktop computer, and from a phone or tablet app that has these settings. The app talks straight to the node, so it doesn't use the **Server**, which answers this Mac only.

## Before you start

- **The model is loaded.** Its card in **Endpoints** says **Ready**. [Load a model on one node](help:load-a-model-on-one-node) shows how.
- **The model listens on your home network.** Its **Listen on** setting is **LAN**, the usual choice, or **All interfaces**. A **Fabric only** load can't be reached from another computer. To change that, use [Edit & reload…](help:restart-or-change-a-model#change-its-settings).
- **The other computer is on your home network.** Away from home, it needs its own VPN client from your Firewalla, as in [Get the profile from your Firewalla](help:connect-the-vpn#get-the-profile-from-your-firewalla). SparkBox's **VPN** window can't help, because that tunnel belongs to SparkBox on this Mac.
- **The app lets you set its own server address.** It may call this a base URL, a custom provider or an OpenAI-compatible server. An app with no such setting can't connect.
- **You can open a shell on a node.** To read the key, use **Live** data with look-only mode off, and be ready to unlock Admin mode. With Demo data, the shell is simulated, and there's no key to read.

> [!WARNING]
> **This connection isn't encrypted, and the key opens the model to anyone who has it.** The address starts with `http://`, so your questions, the answers and the key cross the network as plain text. Use it only on your own home network or a VPN, and don't open the model's port to the internet. If the key gets out, **Edit & reload…** replaces it with a new one.

## Steps

### Find the address and the name

1. In the sidebar, under **Workloads**, click **Endpoints**. [Open Endpoints](open:endpoints) You see one card for each model server.
2. Find your model's card, and check that it says **Ready**.



3. On the card, click **Copy curl**. A message says "curl command copied".
4. Open TextEdit, in **Applications**. If it shows a list of files, click **New Document**.
5. Paste the text. It looks like this, with your own values. It holds no key.

   ```sh
   # This load needs its API key (Keychain item llm:vllm-m1a2b3), or call it through SparkBox Server.
   curl http://192.168.50.154:8000/v1/chat/completions \
     -H "Content-Type: application/json" \
     -H "Authorization: Bearer $VLLM_API_KEY" \
     -d '{"model": "gemma-3-27b-it", "messages": [{"role": "user", "content": "Hello"}]}'
   ```

6. Find these three things in the text:

   | What | Where it is | In this example |
   |---|---|---|
   | The load's id | On the first line, after `vllm-` | `m1a2b3` |
   | The address | On the second line, from `http://` up to and including `/v1` | `http://192.168.50.154:8000/v1` |
   | The **Name in the API** | On the last line, after `"model":` | `gemma-3-27b-it` |

> [!NOTE]
> The first line says "Keychain item", but don't look for it in the Keychain Access app. SparkBox keeps all its secrets together in one item, so opening that item puts every password on your screen at once. The steps below read this one key from the node instead.

### Get the key

Skip this part if the text has no line that starts with `#`. That means SparkBox didn't load this model, so it has no load key.

7. In the sidebar, under **Tools**, click **Terminal**. [Open Terminal](open:terminal) The **Terminal** window opens. If it already has tabs, press <kbd>⌘T</kbd> to see the list of nodes.
8. Click the model's **head node**: the node that answers requests, and the first one named on its card. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A tab opens with a prompt such as `sparkuser@cell2:~$`.
9. Type this line, with your load's id in place of `m1a2b3`, and press <kbd>Return</kbd>. It only reads, so it changes nothing on the node.

   ```sh
   cat ~/.sparkbox/keys/m1a2b3; echo
   ```

   The node prints the key: `sbk-` and then 32 letters and digits.

10. Select the whole key with the mouse, and press <kbd>⌘C</kbd>.
11. Paste the key into a password manager that the other computer can also open, such as the **Passwords** app on Apple devices that share an Apple Account. If you have none, you can read the key from the screen and type it on the other computer. Keep it out of email, chats and ordinary files.
12. Click the tab's **×**. The key stays in that tab's text until you close it.

### Test it from the other computer

This part is for a Mac or a Linux computer. On a Windows PC, a phone or a tablet, go to step 21. If the model has no key, skip steps 14, 15, 18 and 19.

The text runs `curl`, a tool for sending web requests.

> [!WARNING]
> **As copied, this text can show the key to other people on that computer.** While `curl` runs, the computer's list of running programs shows its command, key included. Steps 14 and 15 change one line, so the key stays off that list. If you run the text unchanged on a computer that others use, replace the key with **Edit & reload…**.

13. In TextEdit, delete the first line, the one that starts with `#`.
14. In the box below, click **Copy**.

    ```sh
    -H @<(printf 'Authorization: Bearer %s\n' "$VLLM_API_KEY") \
    ```

15. In TextEdit, find the line that contains `Authorization`. With the mouse, select it from the `-H` at its start to the `\` at its end, and press <kbd>⌘V</kbd>. The new line replaces it. The text now looks like this, with your own values.

    ```sh
    curl http://192.168.50.154:8000/v1/chat/completions \
      -H "Content-Type: application/json" \
      -H @<(printf 'Authorization: Bearer %s\n' "$VLLM_API_KEY") \
      -d '{"model": "gemma-3-27b-it", "messages": [{"role": "user", "content": "Hello"}]}'
    ```

16. Get the text to the other computer. It holds no key, so any way is safe, such as Notes or an email to yourself.
17. On the other computer, open its terminal app. On a Mac, that's Terminal, in **Applications › Utilities**.
18. Type this line and press <kbd>Return</kbd>. The cursor moves to the next line and waits.

    ```sh
    read -rs VLLM_API_KEY; export VLLM_API_KEY
    ```

19. Paste the key and press <kbd>Return</kbd>. Nothing appears as you paste. That keeps the key off the screen and out of the terminal's history. The prompt comes back.
20. Open the text you sent yourself in step 16, copy it, paste it in the terminal and press <kbd>Return</kbd>. When the model has answered, the reply prints as one block of text, called JSON. The model's answer is in it, under `content`.

### Set up your app

21. On the other computer, open your app's settings, and find where it sets up a model server. If it asks what kind, choose OpenAI or OpenAI-compatible.
22. In the field for the address, paste the address from step 6. Apps give this field different names, such as base URL, API base or server URL.
23. In the field for the key, paste the key. The key now stays in that app's settings. If the model has no key, leave the field empty. If the app won't accept that, type any short word, because a model with no key ignores it.
24. In the field for the model, type the **Name in the API**, exactly as it is in step 6.
25. Save the settings, and send a short message, such as `Hello`.

## What you should see

- The app answers, using your model.
- During a long answer, the model's card in **Endpoints** shows a request under **Running · waiting**, and **Generate** rises above 0.
- The **Requests** table in **Server** stays as it was. That's expected, because these requests don't go through the Server.

## Why it works

A model that listens on the **LAN** answers at its head node's own address, so the other computer talks straight to the node. That's why the Server plays no part, and why the model keeps answering while this Mac sleeps. The engine checks the key on every request, and it answers only to the **Name in the API**, so the app has to send both. The key is on the head node because the engine reads it from that file when it starts, and only your account can read that file. In the test, `printf` is built into the terminal, so it passes the key to `curl` without showing it.

## Limits

- **Each model has its own address and key.** To use two models, set up both in the app.
- **Cloud models and aliases stay on this Mac.** They go through the Server, which answers this Mac only. SparkBox has no setting to open it to other computers.
- **A SparkBox key doesn't work here.** A key that starts with `sbx-` is for the Server. A model takes only its own key, which starts with `sbk-`.
- **SparkBox has no button that shows a load's key.** Reading it on the node, as above, is the way to get it.
- **Restart… keeps the key and the address.** **Edit & reload…** makes a new key, so repeat [Get the key](#get-the-key) and paste the new one into the app. Changing the node or the port also changes the address. **Unload…** deletes the key.
- **A card marked External has no load key.** If that server wants a key, it's the one set when the server was started. SparkBox can't show it.

## If something goes wrong

- **"No such file or directory", after the `cat` line.** Compare the id you typed with the first line of the text. If it matches, the model has no key: it's an SGLang load, or **API key** was off when it was loaded. Skip steps 14, 15, 18 and 19.
- **"curl: (7) Failed to connect to 192.168.50.154 port 8000 after 3 ms: Couldn't connect to server", or the app says it can't connect.** The other computer can't reach the node. Check that it's on your home network and not a guest network. Then compare the address with the card.
- **"curl: (3) URL rejected: Malformed input to a URL function", after the reply.** The line you changed in step 15 has joined the next one, and the reply above the message is good. To run the text again, click before `-d` and press <kbd>Return</kbd>.
- **`{"error":"Unauthorized"}`, or the app reports error 401.** The key is missing or wrong. Each new terminal window starts with no key, so repeat steps 18 and 19. If the model was reloaded, [get the new key](#get-the-key).
- **The reply says the model doesn't exist, or the app reports error 404.** The model's name must match the **Name in the API** exactly. Check step 6 again.
- **It worked, and then it stopped.** Open the model's card. If it doesn't say **Ready**, see [Tell whether a model is working](help:tell-if-a-model-is-working). If it does, the key has probably changed, so [get the new key](#get-the-key).
- **The card says "Reachable from cell2 only".** The server listens only on that node, so no other computer can reach it, and this Mac can't either.

## Next steps

- [Endpoints, ports and API keys](help:endpoints-and-apis), to see what each part of the address means
- [Restart a model or change its settings](help:restart-or-change-a-model)
- [Tell whether a model is working](help:tell-if-a-model-is-working)
- [Connect an app to SparkBox](help:connect-an-app), to use apps on this Mac
