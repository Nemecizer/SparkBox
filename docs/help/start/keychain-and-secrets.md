---
id: keychain-and-secrets
title: Where your passwords and keys go
type: concept
area: safety
summary: SparkBox keeps your passwords and keys in the macOS Keychain, out of its files, logs and command lines.
keywords: [password, keychain, secret, security, privacy, token, api key, where are my passwords, remove all passwords, delete my secrets, pass on this mac, keychain access]
verified: null
related: [save-the-cluster-password, settings-security, ssh-sudo-and-host-keys]
---

# Where your passwords and keys go

## In short

SparkBox keeps your passwords and keys in the macOS **Keychain**, the encrypted store built into your Mac. They don't go into SparkBox's settings files, its logs or any command line. When a secret must reach a node, SparkBox sends it over the encrypted connection, and this page lists each case. A load's API key then goes further: it crosses your network unencrypted with each request to the model.

## How it works

### What's in the Keychain

SparkBox keeps these secrets together in one Keychain item, named `com.nemecj.sparkbox`:

| Secret | Where you set it | How to remove it |
|---|---|---|
| The cluster password, for SSH and sudo | [Settings › Cluster login](help:settings-cluster-login) | **Remove** beside its box |
| An SSH key's passphrase, if your key has one | **Settings › Cluster login** | There's no button. See [Remove every secret](#remove-every-secret) |
| The CRS804 DDQ's REST password | [Settings › Switches & gateway](help:settings-switches) | **Remove** beside its box |
| The Hugging Face token | [Settings › Models](help:settings-models) | **Remove** beside its box |
| The Firewalla MSP token | [Settings › Switches & gateway](help:settings-switches) | **Remove** beside its box |
| The VPN's private key and preshared key | The profile you import in [VPN](help:vpn) | **Remove profile** |
| Cloud provider API keys | [Providers](help:providers) | **Remove key** |
| API keys for your loads | [Models](help:models-load-page) | SparkBox removes each one when you unload the model |
| API keys for apps that use the Server | [Server](help:server) | The trash icon beside the key, which revokes it |
| Alert channel secrets | [Settings › Notifications](help:settings-notifications) | **Remove** on the channel's row |
| The metrics token | [Settings › Notifications](help:settings-notifications) | **Remove token** |
| The request key for the always-on agents | SparkBox makes it the first time you install an agent | There's no button. See [Remove every secret](#remove-every-secret) |

The key that signs a fence is kept apart, in its own Keychain item, `com.nemecj.sparkbox.fence`. SparkBox makes it the first time you [open a fence](help:open-a-fence). The test build keeps its secrets in a third item, `com.nemecj.sparkbox.dev`.

A password box in SparkBox writes straight to the Keychain. The secret never comes back to the screen: the box only shows whether one is saved.

Saving or removing a secret you type asks for Touch ID or your Mac password, every time. SparkBox's own keys don't ask: it makes a load's API key when you load the model, and removes it when you unload. You may also see a macOS prompt the first time SparkBox reads the Keychain, and sometimes after an update. That's macOS asking whether SparkBox may use its own item.

### While SparkBox is running

SparkBox reads the secrets from the Keychain when it needs them. It keeps a copy in memory, wipes that copy after 10 minutes without use, and reads the Keychain again the next time.

### What isn't stored

- **SparkBox's settings files** hold no secrets. The node list, for example, holds names and addresses, not the password.
- **An SSH key file** isn't in the Keychain. It stays in `~/.ssh`, the usual place, and only its passphrase goes in the Keychain.
- **The activity log** records commands, not secrets. Before an entry is written, anything that looks like a password, token or key is replaced with ‹redacted›.
- **Diagnostics exports** contain no passwords or keys. Skim a report before you share it anyway, because it does hold addresses and settings.

### When a secret leaves this Mac

Some jobs need a secret on a node. In each case it reaches the node over the encrypted connection, and never on a command line. A load's API key is then used again with each request, as its row says.

| Secret | What happens on the node | When it's gone |
|---|---|---|
| Cluster password, for sudo | Given to sudo's own prompt. Nothing is kept | At once |
| Cluster password, for login | Used to log in, if no key works. Nothing is kept | At once |
| Hugging Face token | Given to the download job, which writes it to a file only your account can read, in `~/.sparkbox/job-secrets/`. The download's container reads that one file, so the token stays out of Docker's records, the job's records and its log. While the download runs, your account and root can read the file, and so can anyone in the `docker` group, because that group can reach any file on the node | When the job ends. If the job was killed or the node restarted, SparkBox removes the file the next time it lists jobs on that node |
| One-time copy password | SparkBox makes a new random password for each [copy](help:copy-a-model-to-other-nodes). The node sending the files keeps it in a file only your account can read, in `~/.sparkbox/rsyncd/` | When the copy's temporary file server stops, after 12 hours at most. The job removes the file as it ends. If the job was killed or the node restarted, SparkBox removes the file the next time it lists jobs on that node |
| A load's API key | Written to `~/.sparkbox/keys/` on the node, in a file only your account can read. Chat, the Server and benchmarks send it to the node with each request, over plain `http` on your home network, or inside the VPN tunnel when you're away. Keep the model's port off the internet | When you unload the model |
| Alert channel secrets, load API keys, the metrics token and the agents' request key | Copied to `/etc/sparkbox-agent/notify.json` on each node with an [always-on agent](help:always-on-agent), so that the agent can alert you. The file belongs to root, but anyone in the `docker` group can reach it, and that includes your own account | When you remove the agent |

Offline tests check that the Hugging Face token uses the protected file path and is kept out of Docker's records. Verify the complete download workflow on your own node before relying on it.

You choose some of these. A load has an API key only if you ask for one. The last row applies only if you install the agents. A channel set to **Send from this Mac only (not copied to the nodes)** stays out of it. [Settings: Notifications](help:settings-notifications) explains the alert channels.

If a node crashes during a download, the token's file stays on that node until SparkBox next lists jobs there with look-only mode off. If that worries you, revoke the token on Hugging Face and save a new one in [Settings › Models](help:settings-models).

One more secret leaves this Mac when you ask it to. To connect another app to SparkBox, you paste a key into that app's own settings. The key then lives in that app. If you stop using the app, revoke the key in [Server](help:server).

The metrics token also crosses your network, but SparkBox doesn't send it. If you set up Prometheus, a monitoring system, it sends the token to each agent over plain `http`. Use it only on your home network.

### Remove every secret

SparkBox has no button that removes all your secrets, and [Reset SparkBox](help:reset-sparkbox) leaves them in the Keychain. If you're passing this Mac on, or you think someone has seen your secrets, work through these three places in this order. Your nodes come first, because SparkBox needs the cluster password to reach them.

- **On your nodes.** Follow [Unload a model](help:unload-a-model) for each model, which removes its API key from the node. In [Settings › Notifications](help:settings-notifications#always-on-monitoring), use **Remove…** for each [always-on agent](help:always-on-agent), which deletes the secrets copied to it.
- **On this Mac.** Quit SparkBox from its menu bar icon, because a running SparkBox keeps a copy in memory. Open **Keychain Access**: press <kbd>⌘Space</kbd>, type `Keychain Access`, then press <kbd>Return</kbd>. Search for `com.nemecj.sparkbox` and delete each item it lists. That removes everything in the table above, the key that signs a fence, and the test build's item if you have one. SparkBox still opens afterward, and you save what it needs again. An SSH key file isn't in the Keychain, so if you set up key login, delete it from `~/.ssh` yourself.
- **At their source.** Deleting a key from this Mac doesn't cancel it. Revoke your Hugging Face token on Hugging Face, and each cloud API key with its provider. If someone may have seen the cluster password, change it on your nodes.

### An example

Say you save the cluster password, then restart a service. SparkBox reads the password from the Keychain into memory and connects to the node. sudo asks for the password with a one-time prompt, and SparkBox answers it over the encrypted connection. The activity log records the restart command, not the password. Ten minutes after its last use, SparkBox wipes its memory copy.

## Why it matters in SparkBox

You never need to put a password in a file, a script or a chat with SparkBox. Every secret has its own field in SparkBox. If an article or a person tells you otherwise, stop and check.

If you think a password has been seen by someone else, change it on the nodes first. Then save the new one in [Settings › Cluster login](help:save-the-cluster-password). A rejected password also clears here: saving a new one gives sudo a fresh start.

To remove one secret, use the control listed in [What's in the Keychain](#whats-in-the-keychain). To remove them all, follow [Remove every secret](#remove-every-secret). [Reset SparkBox](help:reset-sparkbox) leaves them in the Keychain.

## Related

- [Save the cluster password](help:save-the-cluster-password)
- [SSH, sudo and host keys](help:ssh-sudo-and-host-keys)
- [Settings: Security & privacy](help:settings-security)
- [Where SparkBox keeps things](help:files-and-folders)
