---
id: save-the-cluster-password
title: Save the cluster password
type: howto
area: safety
summary: Put the one password your nodes share into the Keychain, so that SparkBox can log in and use sudo.
keywords: [password, keychain, login, sudo, change password, change my password, new password]
verified: false
related: [keychain-and-secrets, settings-cluster-login, connect-your-cluster, check-your-nodes]
---

# Save the cluster password

To let SparkBox log in to your nodes, save the password they share in the macOS Keychain.

## Before you start

- You need the password for the user `sparkuser` on your nodes. It's the password you chose when you set up your Sparks. All four nodes share it.
- Look-only mode must be off.
- You need Touch ID or your Mac password. Saving a password asks every time, in Demo too.
- It takes less than a minute.

> [!NOTE]
> Type the password only into SparkBox's own password boxes. It goes straight to the Keychain and never comes back to the screen. Never put it in a file, a note or a command.

## Steps

1. Press <kbd>⌘,</kbd>. The **Settings** window opens. [Open Cluster login settings](open:settings?section=login)
2. In the list on the left, click **Cluster login**.
3. Under **Account on every node**, check the **User** box. It should read `sparkuser`.
4. Click the **Password** box and type the password. The box shows dots as you type.
5. Click **Save**. macOS asks you to confirm with Touch ID or your Mac password. Confirm it.
6. Read the line beside the box. It says "Saved to Keychain.", and the box now reads "Saved in Keychain".

macOS may also ask whether SparkBox can use its Keychain item, the first time it reads it. Allow it.

## What you should see

The **Password** box reads "Saved in Keychain" and stays empty, and **Remove** becomes available. SparkBox never shows the password again. To replace it, type a new one and click **Save**.

SparkBox doesn't test the password when you save it. To test it, follow [Check that your nodes are ready](help:check-your-nodes).

## Why it works

SparkBox keeps the password in the Keychain, the encrypted store built into macOS. It reads it from there when it needs to log in to a node or answer sudo. Saving a new password also clears any earlier sudo rejection, so sudo gets a fresh try.

[Where your passwords and keys go](help:keychain-and-secrets) lists everything SparkBox keeps there.

## If something goes wrong

- **Save stays dimmed.** The **Password** box is empty, or look-only mode is on.
- **"The password for `sparkuser@192.168.50.51` was rejected. Check it in Settings."** The node refused the password at login. Type it again carefully and click **Save**.
- **"The saved cluster password was rejected by sudo on cell2. Save the correct password in Settings."** The node accepted the login, but sudo refused the password. SparkBox stops trying it on that node. Save the correct password.
- **"The saved cluster password was rejected by sudo on cell1 and cell2, so SparkBox has stopped using it everywhere. Save the correct password in Settings."** Two nodes refused it, so SparkBox stopped using it to keep your account from being locked. Save the correct password.
- **"Couldn't read the Keychain"** followed by a reason. macOS refused access. Click **Retry**. If it keeps happening, open **Keychain Access**: press <kbd>⌘Space</kbd>, type `Keychain Access`, then press <kbd>Return</kbd>. Then check that your login keychain is unlocked.
- **You changed the password on your nodes.** SparkBox doesn't know yet. Save the new one here.

### If you forgot the password

SparkBox never shows the password, and it can't reset it for you. What you do next depends on whether it's still saved.

- **SparkBox still logs in.** Nothing is wrong, so you can leave it. Don't click **Remove**, because that deletes the copy.
- **You need to read it.** Open **Keychain Access**: press <kbd>⌘Space</kbd>, type `Keychain Access`, then press <kbd>Return</kbd>. Search for `com.nemecj.sparkbox`, and open the item whose account is `vault`. Select **Show password**, and confirm with your Mac password. Every SparkBox secret appears together as text, and the cluster password is the value after `"ssh"`. Close the window as soon as you've read it.
- **It isn't saved anywhere.** SparkBox needs this password to log in, so it can't change it. The reset happens at each node, with a [screen and keyboard](help:trust-a-host-key#run-a-command-at-the-node) plugged in, following NVIDIA's DGX OS documentation. Then save the new password here.

## Next steps

- [Check that your nodes are ready](help:check-your-nodes)
- [Connect your cluster](help:connect-your-cluster)
- [Set up key login](help:set-up-key-login)
