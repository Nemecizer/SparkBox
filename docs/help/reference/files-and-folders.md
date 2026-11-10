---
id: files-and-folders
title: Where SparkBox keeps things
type: reference
area: reference
summary: Every folder and file SparkBox keeps on this Mac and on your nodes.
keywords: [folders, logs, application support, ~/.sparkbox, where, settings file, sparkbox.log, ssh.log, evidence, backup, exports, downloads folder, login item]
verified: null
related: [back-up-sparkbox, keychain-and-secrets, reset-sparkbox, capture-evidence]
---

# Where SparkBox keeps things

This page lists every folder and file SparkBox keeps on this Mac and on your nodes. You don't need to touch any of them. Use it when you back up, tidy a disk, or want to know what SparkBox left behind.

> [!WARNING]
> **Editing these files by hand can stop SparkBox reading them.** Change settings in SparkBox instead. If a settings file can't be read, SparkBox sets it aside and starts with defaults, so you can put the old file back.

## On this Mac

### Passwords and keys

Passwords and keys aren't in any file. They live in the macOS Keychain, in one item named `com.nemecj.sparkbox`, account `vault`. That covers the cluster password, switch passwords, the Hugging Face token, provider API keys, Server keys and the VPN's keys. See [Where your passwords and keys go](help:keychain-and-secrets).

### Settings

These are in `~/Library/Application Support/com.nemecj.sparkbox/`.

| File | What it holds |
|---|---|
| `inventory.json` | Your nodes and switches |
| `prefs.json` | **Keep running in the menu bar**, the data source, **Settings › Models** and **Screenshot-safe** |
| `security.json` | Admin mode's lock time, idle shell lock, and how long the activity log is kept |
| `ssh-login.json` | How SparkBox logs in to the nodes |
| `known_hosts.json` | The host keys you trusted |
| `node_pins.json` | The identity pin of each node |
| `switch_certs.json` | The switch certificates you trusted |
| `firewalla_ports.json` | Your labels for the Switch X's ports |
| `firewalla_msp.json` | The MSP domain, when SparkBox may next ask Firewalla's cloud, and its last answer. Never the token |
| `vpn.json` | The VPN's settings, without its keys |
| `providers.json` | Model providers, without their keys |
| `server.json` | The Server's settings, without its keys |
| `notify.json` | Notification settings and channels, without their secrets |
| `rules.json` | Alert rules you changed |
| `alert-silences.live.json`, `alert-silences.demo.json` | Your silences, one file per data source |
| `agents.json` | The always-on agent's settings |
| `fabric_cabling.json` | The cabling you accepted |
| `fabric_baselines.json` | Saved fabric speeds |
| `deployments.json` | SparkBox's model loads and their latest unload reports |
| `deployments.lock` | Coordinates history saves between SparkBox copies; it holds no deployment data |
| `preflight.json` | The last **Before it starts** reading for each set of nodes, with its time |
| `checks.json` | The last **Check nodes** results |
| `recipes-verified.json` | What each recipe has reached **Ready** on |
| `failed-loads/` | The logs SparkBox saved from each failed load: one folder for each failure, with one file for each node, `r0.log` for the head node. Only your Mac account can read them |
| `upgrades.json` | Upgrades SparkBox is following |
| `fence-trust.json` | The Fence key this app trusts |
| `.window-state.json` | The SparkBox window's size and place |

Appearance and Terminal settings, window layouts and other view choices are kept in SparkBox's own window storage, `~/Library/WebKit/com.nemecj.sparkbox`, not in a file here.

For most settings files, a read failure produces a banner such as "Couldn't read inventory.json. SparkBox started with defaults and kept the old file beside it." The old file is renamed `inventory.json.corrupt-<time>`, and **Show in Finder** takes you there.

Deployment history has stricter handling. An unreadable `deployments.json` stays untouched, and writes to it are refused. A list changed by another app session is also protected from replacement. The banner reports the problem even when it happens after startup. Review and preserve both the current file and its recovery copies before reopening; reopening does not restore missing history.

### History and records

These are in the same folder.

| File or folder | What it holds | How long |
|---|---|---|
| `deployment-history/` | Recovery copies of deployment records, including failure and unload reports. Copies are kept before removal or replacement, and when ended records change. File names identify their contents. They never become active loads automatically | No automatic deletion; ordinary serving polls do not add copies |
| `activity.jsonl`, `activity.<number>.jsonl` | The [Activity log](help:activity-log). A new file starts each day, or at 10 MB, and the older file moves to a numbered name | 30, 90 or 365 days, as set in **Settings › Security & privacy** |
| `tasks.jsonl` | Recent tasks | The last 200 |
| `alerts-live.jsonl`, `alerts-demo.jsonl` | The alert log, one file per data source | The last 1,000 events each |
| `deliveries.jsonl` | What happened to each alert message | |
| `history/` | Readings behind the charts | Up to a month, within 500 MB |
| `chats/` | Your **Chat** conversations | Until you delete them |
| `backups/` | Earlier versions of files you edited on this Mac | The newest five of each file |
| `benchmarks.json` | Your [benchmark](help:benchmark-a-model) results | All of them |
| `alerts-before-r3.jsonl` | An alert log from an early version of SparkBox, set aside. Only that version made one, and SparkBox no longer reads it | Until you delete it |

You can read recovery copies in **Models › History**. **Open report file…** also reads older unload-report files without importing them. See [Saved model history](help:models-window#history).

### What SparkBox remembers between runs

These are in the same folder. They let SparkBox continue where it left off after it quits.

| File | What it holds |
|---|---|
| `alerts-active.live.json`, `alerts-active.demo.json` | The alerts open now, one file per data source |
| `gate.json`, `gate-demo.json` | Which alerts have gone out, and messages still waiting to go. `gate.json` is for Live data, and `gate-demo.json` is for Demo |
| `maintenance.live.json`, `maintenance.demo.json` | Maintenance windows that are open, one file per data source |
| `boots.json` | The boot each node was last on, so that SparkBox can tell when a node restarted without being asked |
| `updates-cache.json` | Each node's last update check |
| `wol-macs.json` | Each node's LAN network card address, which **Wake** needs once the node is off |
| `jobs-followed.json` | Downloads and copies a task is following, so that SparkBox follows them again after it restarts |
| `jobs-shares.json` | The temporary file server behind each copy between nodes, so that SparkBox can stop it even after it restarts |
| `sanity.json` | The result of the [17+25 check](help:glossary#1725-check) for each set of load settings, so that a check that passed doesn't run again |
| `protocols.json` | Which API routes each engine image offers |

### Logs

These are in `~/Library/Logs/SparkBox/`. SparkBox redacts secrets before it writes them.

| File or folder | What it holds | How long |
|---|---|---|
| `sparkbox.log` | Problems in the app itself, such as a window that crashed or a task that failed | Rotated at 5 MB, with three files kept |
| `ssh.log` | Every SSH connect, drop, reconnect and refusal | Rotated at 5 MB, with three files kept |
| `evidence/` | Your [evidence captures](help:capture-evidence) | Until you delete them |

Each capture is a folder named `<date>-<group>/<time>-<label>/`, from the **Group** and **Label** you typed. It holds one folder per node, with a text file for each step. Beside those are `manifest.json`, a comparison with the capture before it, the activity since then, and a copy of the SSH log.

An upgrade and [Pin the GPU stack](help:pin-the-gpu-stack) also file a package list under `evidence/`, in a folder named `<date>-updates`.

### Downloads

SparkBox saves these to `~/Downloads`. Each name carries a date, a time or a number, so a new file sits beside the older ones. The one exception is a benchmark: exporting the same run in the same format again replaces its file, with the same results.

| File | Where it comes from |
|---|---|
| `SparkBox-doctor-<number>.txt` | The [Doctor report](help:collect-diagnostics), from **Run doctor** in **Settings › Diagnostics** |
| `SparkBox-diagnostics-<date and time>.txt` | **Export diagnostics**, in **Settings › Security & privacy** |
| `SparkBox-activity-<date and time>.csv` or `.json` | An export from the [Activity log](help:activity-log), or from **Settings › Security & privacy** |
| `SparkBox-bench-<model>-<number>.csv` or `.json` | A benchmark you exported |
| `<tab name> <date> <time>.log` | A Terminal log tab you saved |

### Start at login

`~/Library/LaunchAgents/SparkBox.plist` exists only while **Start at login** is on. It's the item that opens SparkBox when you log in to this Mac. To remove it, turn the setting off in **Settings › General**.

### The Fence

`~/Library/Application Support/SparkBox Fence/` belongs to the [Fence](help:the-fence), which applies to the test build only. The signing key is in the Keychain, in an item of its own.

| File | What it holds |
|---|---|
| `fence.json` | The open fence |
| `fence-key.pub` | The public key that checks it |
| `fence-closed.json` | Which fence closed last, and when |

Your main folder also holds `fence-seen.json`, the newest fence accepted and the last one closed, and `fence-failed.json`, a fence that failed its check. Together they stop an old fence file from working again if someone puts it back.

### The test build

The test build keeps its own folder, `com.nemecj.sparkbox.dev`, its own Keychain item, an SSH log named `ssh-dev.log`, and `.window-state.dev.json` for its window. The first time it runs, it copies your node list, host keys, login method and identity pins, and notes that in `imported-from-release.json`. Your own SparkBox never reads any of these, so you can ignore them.

## On your nodes

### In your home folder

SparkBox keeps its helpers in `~/.sparkbox/` for user `sparkuser` on each node.

| File or folder | What it holds |
|---|---|
| `collector.py` | The read-only helper that reports each node's readings |
| `roce.py` | Reads the fabric's counters for the collector |
| `sparkjob.py` | Runs long jobs and file changes |
| `launch.py` | The small launcher that starts vLLM inside a load's container, with no shell in the path |
| `deploy/` | One folder for each load, with the options and settings the launcher reads. Only your account can read it. Unloading removes it |
| `jobs/` | One status file for each download or copy, which is how SparkBox reads a job's progress |
| `keys/` | A load's API key, in a file only your account and root can read. Unloading removes it |
| `backups/` | The previous five versions of files you edited |
| `tmp/` | Text on its way to a file you're saving |
| `rsyncd/` | One folder for each copy between nodes, with its temporary file server's settings and one-time password. The folder goes when the job ends. If the job was killed or the node restarted, it goes the next time SparkBox lists jobs on that node |
| `job-secrets/` | One folder for a download that uses a Hugging Face token, holding the token in a file only your account can read. It goes when the job ends, the same way as `rsyncd/` |
| `controller.json`, `controller.lock` | The [controller lease](help:leases-and-identity): which SparkBox is making changes. The lock file stops two copies of SparkBox writing it at once |

In look-only mode, SparkBox starts no change on a node, with one exception: it keeps these helper scripts up to date, by checksum. In that mode it runs only their reading commands. With look-only mode off, listing a node's jobs also marks a job that stopped unexpectedly as **failed** in `jobs/`, and removes the folders in `job-secrets/` and `rsyncd/` left by any job that's over.

[Set up key login](help:set-up-key-login) also adds your public key to `~/.ssh/authorized_keys`, once, and leaves the rest of that file alone.

### Models

| Folder | What it holds |
|---|---|
| `~/models/<org>/<name>/` | A model SparkBox downloaded. You can change `~/models` in **Settings › Models** |
| `.sparkbox.json`, in a model folder | The record that lets SparkBox mark a copy **✓ verified** or **✓ sizes match** |
| `.sparkbox.partial`, in a model folder | A marker that a download or copy is still filling this folder, or stopped partway. It goes once the files are checked |
| `~/.cache/huggingface/hub/` | Models fetched by other tools. SparkBox lists these too |
| `~/.cache/vllm`, `~/.cache/flashinfer`, `~/.triton` | Code prepared during earlier loads, so that the next load of a model starts faster |

### While a copy or a save runs

You may see a hidden item beside the place SparkBox is writing to, named like `.sparkbox-incoming-…` or `.notes.txt.sparkbox-…`. It's the new data on its way, and SparkBox puts it in place only when it's complete. If a copy or a save is cut off, SparkBox clears the leftover the next time it copies into that folder or saves that file, once the leftover has sat for 10 minutes.

### In the Trash

**Move to Trash** on a node uses the standard Trash folder, `~/.local/share/Trash/`. An item on another disk goes to a folder named `.sparkbox-trash` next to it. See [Delete and restore files](help:delete-and-restore-files).

### Owned by root

Root is the account on a node that can change anything. These appear only after you use the feature, and each needs Admin mode.

| Path | What it holds | When it appears |
|---|---|---|
| `/usr/local/lib/sparkbox/` | SparkBox's root helper, checked by checksum before each use | The first time you read or save a file with sudo |
| `/var/log/sparkbox/` | An upgrade's log (`upgrade-<id>.log`) and the package list saved before it started (`dpkg-before-<id>.txt`) | Your first upgrade |
| `/etc/ssh/sshd_config.d/00-sparkbox-keys-only.conf` | The setting that turns password login off | When you [turn password login off](help:set-up-key-login#turn-password-login-off) |

### The always-on agent

These exist only on nodes where you [installed the agent](help:set-up-always-on-monitoring). **Remove…** deletes them all.

| Path | What it holds |
|---|---|
| `/opt/sparkbox-agent/` | The agent's program files |
| `/etc/sparkbox-agent/` | Its settings. `notify.json` holds channel secrets, load API keys, the metrics token and the agents' request key. It belongs to root, and the agent's service reads it. Anyone in the docker group, including your own account, can also reach it. [Who can read a channel's secrets](help:get-alerts-on-your-phone#who-can-read-a-channels-secrets) has the details |
| `/etc/systemd/system/sparkbox-agent.service` | The service that keeps it running |
| `/var/lib/sparkbox-agent/` | Its history, its waiting messages and its state |

## What's safe to delete

| To free | Do this |
|---|---|
| A model | Use [Delete a model from a node](help:delete-a-model), not Finder or `rm`. SparkBox checks that nothing is using it |
| A Docker image | Use [Check Docker images](help:check-docker-images) |
| Old evidence | Delete folders under `~/Library/Logs/SparkBox/evidence/` in Finder |
| Everything on this Mac | Use [Reset SparkBox](help:reset-sparkbox) |
