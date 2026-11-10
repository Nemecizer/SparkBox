---
id: glossary
title: Glossary
type: reference
area: reference
summary: The technical words you meet in SparkBox, explained in plain language.
keywords: [definition, meaning, what does it mean, terms, dictionary, word, jargon]
verified: null
related: [using-help, status-words, models-in-plain-words]
---

# Glossary

This page explains the technical words you meet in SparkBox, in alphabetical order. Status words such as **Online** and **Ready** have a page of their own: [What the status words mean](help:status-words).

## Numbers and symbols

### 17+25 check

A one-line test. SparkBox asks a newly loaded model "What is 17+25? Reply with the number only." and expects `42`. A model that loads but answers nonsense fails here, instead of failing later in your app. See [Tell whether a model is working](help:tell-if-a-model-is-working).

### /dev/shm

A folder on a node that lives in memory, not on disk. Programs use it to pass large data to each other. SparkBox checks that it's empty before a load, because something left there holds memory the model needs.

### ~ (tilde)

Short for your [home folder](#home-folder) on a node. `~/models` means the `models` folder inside it.

## A

### ACK timeouts

A fabric counter: a sender waited for a reply that didn't come, and sent again. A few under load are normal.

### Acknowledge

To tell SparkBox you've seen an alert. It stops repeating and stops counting, until it gets worse. See [Alerts and incidents](help:alerts-and-incidents).

### Activity log

SparkBox's record of what it did: each command, on which node, and how it ended. See [Activity log](help:activity-log).

### Admin mode

The state in which SparkBox may make changes. You unlock it with Touch ID or your Mac password, and it locks by itself after a while. The padlock in the top bar shows it. See [Admin mode](help:admin-mode).

### Agent

See [always-on agent](#always-on-agent).

### Alert

One condition SparkBox noticed because a rule says it matters, such as a disk that's 85% full. Related alerts are grouped into an [incident](#incident).

### Alias

A short name for a model in the Server, such as `local`. Your app asks for that name, and the Server sends the request to whichever model it stands for today. See [Give a model a short name](help:give-a-model-a-short-name). In **Files**, an alias is a link to something elsewhere.

### Allow remote code

A load option for models that need their own Python code to run. Turn it on only for a model you trust, because that code runs inside the model's container. See [Choose load settings](help:choose-load-settings).

### All-reduce

A way for several GPUs to combine their results. A model split across nodes does this constantly, so the fabric test measures it.

### Always-on agent

SparkBox's small service on a node. It keeps watching and can reach your phone while this Mac is asleep or SparkBox is closed. See [The always-on agent](help:always-on-agent).

### AmneziaWG

A variant of [WireGuard](#wireguard) that's harder for networks to block. The Firewalla Gold can offer either one.

### API

The way one program talks to another. A model's API is how apps send it questions and get answers. See [Endpoints, ports and API keys](help:endpoints-and-apis).

### API key

A long secret that proves an app may use a model or service. SparkBox keeps yours in the Keychain on this Mac. A load's key also sits on its node while the load runs. See [Where your passwords and keys go](help:keychain-and-secrets).

### apt

The tool that installs and updates software on Ubuntu, the system your nodes run. See [Packages, updates and the GPU stack](help:packages-and-updates).

### Attention heads

Parts of a model that each focus on different words. Tensor parallel needs them to divide evenly among nodes. If they don't, the fit table shows **PP** instead of **TP**, and the **Load** page refuses the split.

## B

### Baseline

A speed you saved from an earlier fabric test. Later tests compare with it.

### Before it starts

The part of the Load page that shows what SparkBox read on each chosen node: its memory, the image, the folder, the port, the fabric and more. It gives one verdict for all of them. It's filled in with Live data.

### Benchmark

A measured speed test of a model. SparkBox runs one as a [sweep](#sweep) and saves the results. See [Benchmark a model](help:benchmark-a-model).

### bf16

A way of storing numbers, using 16 bits each. It's the usual precision for a model's KV cache. See [fp8](#fp8) for the smaller choice.

### Boot

One start of a node, from power-on or a restart. SparkBox tells boots apart by an ID that changes each time.

### Bounce

To turn a switch port off and on again, as a quick reset. See [Manage a switch port](help:manage-a-switch-port).

### Broadcast

A Terminal option that sends what you type to several shells at once.

### Bus bandwidth

The speed the fabric reaches while GPUs exchange data in the all-reduce test, in Gb/s.

## C

### Cache folders

Folders on a node where an engine keeps the code it prepares for the GB10, so that the next load of a model starts faster.

### Canary

A first test on one node before the rest. In **Fleet Command**, **Canary first** runs on one node, and the others run only if it succeeds.

### Carrier

The signal that says a network link is physically connected. "No carrier" points at a cable, an optic, or a device that's off.

### Certificate

A small file a device shows to prove who it is over a secure web connection. SparkBox asks you to trust your CRS804 DDQ's certificate once, and tells you if it changes. See [Add your switches and gateway](help:add-switches-and-gateway).

### Channel

One place alerts go: this Mac, ntfy, Pushover, a webhook or email. See [Settings: Notifications](help:settings-notifications).

### Checksum

A short code worked out from a file's contents. Change one byte of the file and the code changes, so a matching checksum shows the file arrived whole. SparkBox uses checksums to check downloads and its own helper scripts.

### Cluster

Your four nodes, with their switches and gateway. See [Your cluster at a glance](help:your-cluster).

### Cluster password

The one password for user `sparkuser` on every node. It's used to log in and for sudo. SparkBox keeps it only in the Keychain. See [Save the cluster password](help:save-the-cluster-password).

### CNP

Short for congestion notification packet. A node sends one to ask a sender to slow down. A few are normal under load.

### Collector

A small read-only helper at `~/.sparkbox/collector.py` on each node. It sends SparkBox the readings you see.

### Command palette

The search box that runs any command. Press <kbd>⌘K</kbd>. See [The command palette](help:command-palette).

### Compile

To turn a program into code a chip can run. An engine does this for the GB10 the first time it starts a model, which is why a first start is slow. The compiled work stays in the [cache folders](#cache-folders) on the node, so that the next load of that model starts faster.

### Connecting

A node state: SparkBox is trying to connect. See [What the status words mean](help:status-words).

### ConnectX-7

The NVIDIA network card in each Spark that connects it to the fabric.

### Container

A program running in its own sealed box. The box keeps that program apart from the rest of the node, and it can be removed cleanly. Your model engines run in containers. See [Containers and images](help:containers-and-images).

### Context

How long a conversation the model can hold, in tokens. **Max context** on the Load page sets the limit. See [Tokens and context](help:tokens-and-context).

### Controller lease

The claim one SparkBox holds on a node while it makes a change, so that two copies never change a node at once. See [Controller lease and identity pin](help:leases-and-identity).

### Copy

To send a model from one node to others over the fabric. The button is **Copy to nodes…**. See [Copy a model to other nodes](help:copy-a-model-to-other-nodes).

### CPU

The chip that does a computer's general work, such as running the system and its programs. A Spark's CPU shares [unified memory](#unified-memory) with its [GPU](#gpu).

### CQE errors

Short for completion queue entry errors. They mean the network card reported a failed fast transfer. Any that keep rising are worth checking.

### Critical

The more serious of the two alert severities. It needs attention now.

### CUDA

NVIDIA's software for running programs on a GPU. Model engines depend on it.

## D

### DCQCN

The method that slows senders on the fabric when ECN marks arrive. It's normal during heavy traffic between nodes.

### Dead-man's switch

A check that alerts you when messages stop arriving. A node's agent regularly contacts a web address you choose. If the contact stops, that service tells you, even if your whole cluster is off.

### Decode t/s

A column in the fit table: the most tokens per second one reply can reach, set by memory bandwidth. Real speed is lower. See [Find a model that fits](help:find-a-model-that-fits).

### Demo

Simulated data. SparkBox shows a pretend cluster, and nothing you do reaches your real nodes. See [Demo and Live](help:demo-and-live).

### Desk

The area where SparkBox's own windows sit. See [Windows and layouts](help:windows-and-layouts).

### DGX Dashboard

NVIDIA's own web page on each node. SparkBox opens it through a tunnel. See [Open the DGX Dashboard](help:open-the-dgx-dashboard).

### DGX OS

The system on your nodes: NVIDIA's version of Ubuntu.

### DGX Spark

NVIDIA's small AI computer, sold under brand names including MSI EdgeXpert and ASUS Ascent GX10. You own four. Help also calls one a Spark or a [node](#node).

### Discards

Packets dropped by a network card. **Discards (PHY)**, **Transmit discards** and **Out of buffer** each count a different place.

### DKMS

A way of rebuilding a driver for each new kernel on the node itself. On your Sparks, DKMS NVIDIA packages have caused a failed restart, so SparkBox checks for them first.

### Docker

The program that runs [containers](#container) on your nodes.

### Doctor (Fabric tab)

The fabric test. **Doctor** is a tab in the **Fabric** window. It isn't the Doctor report. See [Fabric: Doctor tab](help:fabric-doctor).

### Doctor report

A text report from **Settings › Diagnostics**, for when you need to look into a problem. It isn't the fabric test. See [Collect diagnostics](help:collect-diagnostics).

### Down

An endpoint state: it should be answering and isn't. See [What the status words mean](help:status-words).

### Download

To fetch a model from Hugging Face to a node. See [Download a model](help:download-a-model).

### dpkg

The lower-level tool under [apt](#apt) that unpacks and sets up each package.

### Driver

The software that lets a computer use a piece of hardware, such as a GPU or a network card. The **NVIDIA driver** runs the GPU on each node. It has to fit the [kernel](#kernel), so it's part of the [GPU stack](#gpu-stack).

### Drop the page cache first

An option on the Load page. It tells the node to empty its page cache before a load, when vLLM would otherwise refuse to start. It needs sudo. The cache refills by itself.

### Dry run

A rehearsal. SparkBox asks apt what an upgrade would do, without doing it. See [Simulate an update](help:simulate-an-update).

### Duplicate requests

A fabric counter: the same data arrived twice, because a sender repeated it.

## E

### earlyoom

A small program on your nodes that stops the biggest process when memory runs very low, to keep the node from freezing.

### ECN

Short for explicit congestion notification. Busy network hardware marks packets to warn of congestion before it has to drop them.

### Endpoint

A running model server you can talk to. See [Endpoints](help:endpoints).

### Engine

The program that runs a model and answers requests. SparkBox loads models with **vLLM** or **SGLang**, and also works with **Ollama**. See [Engines: vLLM, SGLang and Ollama](help:engines).

### Error

A node state: the node answered and turned SparkBox away. See [What the status words mean](help:status-words).

### Evidence

A saved record of each node's state, made with **Capture evidence**, so that you can compare before and after. See [Capture evidence](help:capture-evidence).

### Expert parallel

A load option for mixture-of-experts models. It spreads the model's experts across nodes.

### Expert: raw vLLM args

A field on the Load page for vLLM options that have no field of their own, one per line. It appears only with Admin mode unlocked. It can't hold a key, load code or open files.

## F

### Fabric

The fast 200G network between nodes, through the CRS804 DDQ. This Mac isn't on it. See [The fabric and RDMA](help:fabric-and-rdma).

### Fence

A time-limited permission that lets the test build change chosen nodes. The SparkBox app doesn't need one. See [The Fence](help:the-fence).

### Firewalla

The brand of your LAN switch, the Firewalla Switch X, and your gateway, the Firewalla Gold.

### Firmware

Software built into hardware, such as a node's board or network card. SparkBox can show whether an update is waiting.

### Fit table

The table on the **Hugging Face** tab that shows, for each number of nodes, whether a model fits and how much room is left for conversations. See [Find a model that fits](help:find-a-model-that-fits).

### Fits now

A column in the fit table. It says whether enough nodes have the room right now, counting what other models already hold.

### Fleet Command

The window that runs one command on several nodes. See [Fleet Command](help:fleet-command).

### fp16

A way of storing numbers, using 16 bits each. It takes the same memory as [bf16](#bf16), and twice as much as [fp8](#fp8).

### fp8

A smaller way of storing numbers, using 8 bits each. A KV cache in fp8 takes half the memory, with a little less accuracy.

### Frozen

A node that's still on but has stopped answering. A Spark that runs out of unified memory can freeze instead of failing cleanly. See [Recover a frozen node](help:recover-a-frozen-node).

## G

### Gated

A model on Hugging Face that you must ask to use. Accept its license on the website, then give SparkBox a token. See [Get a Hugging Face token](help:get-a-hugging-face-token).

### Gateway

Your Firewalla Gold: the device between your home network and the internet.

### GB10

The NVIDIA chip in each Spark. Its CPU and GPU share one pool of memory.

### Generate

A number on an endpoint's card: tokens the model is writing per second, across all requests.

### GGUF

A file format for models, used by Ollama and llama.cpp. See [Model formats and quantization](help:formats-and-quantization).

### GiB

Gibibyte, the unit SparkBox uses for memory and disk space. 1 GiB is about 1.07 GB, so you can treat the two as about the same when you plan disk space. 1 TiB is 1,024 GiB.

### GPU

The chip that does a model's heavy work. Each Spark has one.

### GPU memory share

How much of each node's memory the engine reserves when it starts. `0.75` means 75%. See [How memory works on a Spark](help:memory-on-a-spark).

### GPU stack

The parts that must match for the GPU to work: the kernel, the NVIDIA driver, CUDA, Docker, the network software for the fabric, and the DGX OS updater and firmware tool. SparkBox guards them during updates. See [Pin the GPU stack](help:pin-the-gpu-stack).

## H

### Half

One of the two paths through a node's fabric port. Half 1 uses `192.168.100.x`, and half 2 uses `192.168.101.x`.

### Head node

The first node of a load, which answers requests. It's rank 0, and on a load across nodes it starts last. See [Running one model on several nodes](help:splitting-a-model).

### Help tip

The small box that opens from an info icon: a short explanation and links. See [Find your way around Help](help:using-help).

### Hold

An instruction to apt: don't update this package. **Pin the GPU stack** uses holds.

### Home folder

Your own folder on a node, `/home/sparkuser`. Paths write it as `~`, so `~/models` is the `models` folder inside it.

### Host key

The key that proves a node is the same node as last time. SparkBox asks you to trust it once. See [SSH, sudo and host keys](help:ssh-sudo-and-host-keys).

### Hostname

The name a node goes by on your network, such as `cell1-example.local`. SparkBox connects by [IP address](#ip-address) when it has one, and by hostname otherwise. The [identity pin](#identity-pin) checks the hostname too.

### Hugging Face

The website where most AI models are published. See [huggingface.co](https://huggingface.co).

## I

### Identity pin

SparkBox's record of which machine a node is, by its name, ID and network address. If another machine answers at that address, SparkBox refuses changes. See [Controller lease and identity pin](help:leases-and-identity).

### Image

The recipe a container starts from. See [Containers and images](help:containers-and-images).

### Implied NAKs

A fabric counter: a node worked out that data was lost, and asked for it again.

### In maintenance

A node state: you put it in [maintenance mode](#maintenance-mode).

### Incident

Related alerts counted as one. If a fabric cable fails, you see one incident, not ten alerts. See [Alerts and incidents](help:alerts-and-incidents).

### Incomplete

A model folder that a download or copy left partway. The Library shows **◐ incomplete**, and SparkBox won't load it until the download or copy finishes. See [Start again](help:follow-a-download#start-again).

### Info icon

The small ⓘ beside a label. Click it for a Help tip.

### initrd

A small file a node needs at the very start of a restart. A kernel without one won't start. SparkBox checks for it first.

### Inventory

The list of nodes SparkBox manages. You change it in [Settings: Nodes](help:settings-nodes).

### IP address

The number that says which computer, such as `192.168.50.154`. See [Ports and addresses](help:ports-reference).

### ITL

Short for inter-token latency: the time between one token and the next as a model writes.

## J

### Job

Long work running by itself on a node, such as a download or a copy. It's listed in **Models › Jobs**. See [Tasks and jobs](help:tasks-and-jobs).

### Journal

A node's system log, kept by [systemd](#systemd).

### Jumbo frame

A larger network packet, 9000 bytes here. The fabric uses them for speed. A device on the path that doesn't allow them breaks large transfers.

### Jump host

A machine you pass through to reach another. SparkBox follows one `ProxyJump` line from `~/.ssh/config`. See [Settings: Cluster login](help:settings-cluster-login).

## K

### Keep running

A load option that restarts a model after a crash. See [Choose load settings](help:choose-load-settings).

### Kernel

The core of the system on a node. An update to it needs a restart. It isn't the same as a [GPU kernel](#kernel-gpu).

### Kernel (GPU)

A small piece of code that does one kind of work on the GPU. On a GB10, an engine may have no GPU kernel for a model's format or kind of attention, and then the model can't start. SparkBox's messages shorten this to "kernel", as in "No kernel on GB10".

### Keychain

The macOS vault for passwords. SparkBox keeps your secrets there on this Mac. A few must also reach a node for a job, and [Where your passwords and keys go](help:keychain-and-secrets) lists each one.

### KV cache

The memory a model uses to keep track of the text so far. Think of it as the model's short-term memory for this conversation. Longer conversations and more requests at once need more of it. See [How memory works on a Spark](help:memory-on-a-spark).

### KV cache type

How numbers in the KV cache are stored: **bf16**, or **fp8** for half the memory.

### KV heads

Sets of memory some models share between [attention heads](#attention-heads). Tensor parallel needs them to divide evenly among nodes too.

### KV room

A column in the fit table: what's left of the engine's share for the KV cache, after the weights.

## L

### LAN

Your ordinary home network. See [Ports and addresses](help:ports-reference).

### Latency

The delay before something happens. On the fabric, it's how long a small message takes to reach another node, about 2 µs between Sparks. For a model, see [TTFT](#ttft).

### Launcher

A small program SparkBox keeps on each node, `~/.sparkbox/launch.py`. Inside a load's container it reads the load's settings and starts vLLM, with no shell in the path.

### Layer

One level of a stack, in a model or in an image. A [model](#model) is a stack of layers, and a token passes through every layer in order. A Docker [image](#image) is built in layers too, and images that share a layer store it once.

### Leftovers

Containers an earlier load left on a node, when no running load owns them. They stop a new load until they're gone. See [Clear what an earlier load left behind](help:clear-what-an-earlier-load-left-behind).

### Library

The tab in **Models** that lists model folders on your nodes. See [Models: Library tab](help:models-library).

### Linux

A family of operating systems, the main software that runs a computer. Your nodes run [DGX OS](#dgx-os), which is NVIDIA's version of Ubuntu 24.04 Linux.

### Listen on

Which network a load answers on: **LAN**, **Fabric only** or **All interfaces**. See [Ports and addresses](help:ports-reference#listen-on).

### Live

Your real cluster, reached over SSH. See [Demo and Live](help:demo-and-live).

### LLDP

The way network devices tell each other their names. It's how SparkBox knows what's at the other end of a cable.

### Load

To start serving a model. "A load" is one running start. See [Load a model on one node](help:load-a-model-on-one-node).

### Loading

An endpoint state: it's starting and reading its weights.

### Look only

A mode in which SparkBox starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date. Work already running by itself on a node keeps going. Only turning the mode off asks for Touch ID or your Mac password. The padlock shows **Look only**. See [Use look-only mode](help:use-look-only-mode).

### Lossless

A fabric setting in which traffic isn't dropped under load. Yours needs [PFC](#pfc) for that.

## M

### MAC address

A network card's fixed ID. **Wake** needs a node's, so SparkBox learns it while the node is on.

### Maintenance mode

A planned period for a node. Its alerts page nobody until it ends. See [Use maintenance mode](help:use-maintenance-mode).

### Mask

A strong way to stop a service: nothing can start it, by hand or otherwise, until you unmask it. See [Services](help:services).

### Max context

How long a conversation a load can hold, in tokens. See [Tokens and context](help:tokens-and-context).

### Memory plan

The bars on the Load page that show how a load would fill each node's memory. See [Choose load settings](help:choose-load-settings).

### Menu bar

The macOS bar at the top of the screen. See [Menu bar reference](help:menu-reference).

### Menu bar icon

SparkBox's icon at the right of the menu bar. See [The menu bar icon](help:menu-bar-icon).

### MikroTik

The maker of your fabric switch, the CRS804 DDQ.

### Mixture of experts

A model design with many smaller parts called experts, of which only a few work on each token. Speed varies, so the fit table says **varies**.

### MLX

A model format for Apple chips. It doesn't run on a GB10. See [Model formats and quantization](help:formats-and-quantization).

### Model

A trained AI: a very large set of learned numbers, stored as files, that lets a computer read and write text. See [Models, engines, memory and context in plain words](help:models-in-plain-words).

### Model folder

A model's folder on a node, usually under `~/models`.

### Mount

To make a disk or a folder appear at a place in a file system. A node's main disk is mounted at `/`, so its row in the **Disks** tab reads `/` under **Mount**. A folder on a node can also be mounted into a [container](#container), which is how an [engine](#engine) reaches a model's files.

### MSP

Firewalla's web service for looking after its devices, at firewalla.net. SparkBox can read the Switch X's ports from it. See [Connect Firewalla's cloud](help:connect-firewalla-cloud).

### MTU

The largest network packet a link carries. The fabric uses `9000`.

## N

### Name in the API

The name apps put in `model` to reach a load.

### NCCL

NVIDIA's library that lets GPUs on different nodes work together. It uses RDMA over the fabric.

### needrestart

A tool that lists services still using old copies of software after an update.

### Node

One of the computers you configure. `cell1`, `cell2`, `cell3` and `gx10-0000` are synthetic Demo names.

### Not updating

The same state as [Stale](#stale), in a tooltip.

### ntfy

A service that sends messages to an app on your phone. You can use it as a [channel](#channel). See [Get alerts on your phone](help:get-alerts-on-your-phone).

### NVFP4

A compact model format from NVIDIA that keeps weights in very few bits. See [Model formats and quantization](help:formats-and-quantization).

## O

### Off (planned)

A node state: you shut it down from SparkBox.

### Offline

A node state: SparkBox can't reach it. See [A node shows Offline](help:troubleshooting#a-node-shows-offline).

### Ollama

A simpler engine that already runs on some of your nodes. See [Ollama](help:ollama).

### Online

Connected and sending readings: the sidebar shows the node's GPU use, such as **GPU 28%**, and the top bar counts it, as in **4/4 online**. Help writes this word in lower case, because a node's row never shows it.

### Optic

The small plug-in part at each end of a fiber cable that turns light into signals.

### OTA

Short for over the air: an update delivered over the network, such as a DGX OS update.

### Out of buffer

A fabric counter: data arrived with no room to put it.

### Out of sequence

Data that arrived in the wrong order on the fabric. It usually means something was lost and sent again.

### Override memory guardrail

A checkbox that lets a load start even though its memory plan is red. A Spark that runs out of unified memory can freeze, and may need someone at the machine. See [Choose load settings](help:choose-load-settings).

## P

### p50, p95

Ways of reading many timings. p50 is the middle one: half were faster. p95 is what 95 out of 100 beat, so it shows the slow cases.

### Package

One piece of software that apt installs.

### Packet

One small piece of data sent across a network. A large transfer is split into many packets, and the [MTU](#mtu) sets the largest size a link carries.

### Page

To send an alert to your phone. The settings call this **Paging**.

### Page cache

Memory the node uses to remember recently read files. It's released when programs need it, but vLLM's startup check counts it as used.

### Passphrase

A password that protects a key file. Most keys don't have one. SparkBox keeps yours in the Keychain. See [Settings: Cluster login](help:settings-cluster-login).

### Pause frames

Messages that tell a sender to stop for a moment. Many in a row point at a stuck receiver.

### perftest

A set of tools that measure RDMA speed. The **Doctor** tab in **Fabric** uses them.

### PFC

Short for priority flow control. It uses pause frames to avoid losing data on a chosen kind of traffic. SparkBox reports whether it's on, and doesn't change it.

### Phasing

Ubuntu's way of releasing an update to a few computers at a time. **Deferred by phasing** means apt held it back for now.

### PHY

The physical layer: the optic, the cable and the signal. PHY errors point at those.

### PID

Process ID: the number a node gives each running program.

### Pin

To hold packages at their current version. See [Pin the GPU stack](help:pin-the-gpu-stack).

### Pipeline parallel

A way to split a model: each node runs a slice of its layers, one after another. It adds room but not speed for one reply. See [Running one model on several nodes](help:splitting-a-model).

### Planned

A state for work you started. Planned alerts count nowhere and page nobody.

### Port

A numbered door on a computer that a program answers on. See [Ports and addresses](help:ports-reference). A switch or network card has ports too: the sockets its cables plug into.

### Prefill

The stage where a model reads your prompt before it starts to answer. The **Prompt** number on an endpoint's card measures it.

### Prefix caching

Reusing the KV cache when prompts start the same way, such as a shared system prompt.

### Process

One running program on a node. See [Services and processes](help:services-and-processes).

### Prometheus

A popular program that collects numbers over time. The always-on agent can offer its numbers to one.

### Prompt

The text you send a model. On an endpoint's card, **Prompt** is how fast it reads prompts, in tokens per second.

### Provider

A place Chat can reach models: a cluster endpoint, or a cloud service such as Anthropic or OpenAI. See [Providers](help:providers).

### Pull

To fetch an Ollama model or a Docker image. For Hugging Face models, SparkBox says [download](#download).

### Pushover

A paid phone app for alerts. You can use it as a [channel](#channel).

## Q

### Quantization

Storing a model's numbers in fewer bits. The model gets smaller and faster, with a little less accuracy. See [Model formats and quantization](help:formats-and-quantization).

### Quiet hours

Times when alerts wait instead of reaching your phone. See [Settings: Notifications](help:settings-notifications).

## R

### Rank

A node's number within one load. The [head node](#head-node) is rank 0, and the [workers](#worker) are ranks 1, 2 and 3. A rank belongs to the load, not to the node, so the same node can have another rank in the next load. A message such as "tensor-parallel rank 3 of 4" counts from 1, so it means the container whose name ends in `-r2`.

### Ray

A program that links nodes so that they can work on one job. SparkBox's own loads don't use it. Servers SparkBox didn't start may, and **Demo** data still shows it.

### Ray head

The copy of [Ray](#ray) on a server's head node. The other nodes join it.

### RDMA

Short for remote direct memory access. One computer writes straight into another's memory, with very little delay. See [The fabric and RDMA](help:fabric-and-rdma).

### Read-only

The padlock word when Admin mode is locked. SparkBox won't make a change you ask for until you unlock, but it finishes and looks after work already started. See [Read-only, Admin mode and Look only](help:admin-mode).

### Ready

An endpoint state: it answers. See [What the status words mean](help:status-words).

### Reasoning parser

A load option that separates a model's thinking from its answer.

### Recipe

SparkBox's saved description of how to load one kind of model: which splits suit it, which values to start from, and which options vLLM needs. It isn't a Docker [image](#image). See [Use a recipe for a load](help:use-a-recipe-for-a-load).

### Re-pin

To accept the machine answering now as this node, after you moved or rebuilt it. The button is **Re-pin identity…**.

### Repository

A model's page of files on Hugging Face, named like `org/name`.

### Request

One question sent to a model. A long chat sends the whole conversation each time.

### Requests at once

How many full-length requests the KV cache must hold together. It changes the memory plan.

### Restart

To turn a node off and on again. The button is **Restart…**. The menu bar icon's item is named **Reboot…**. See [Restart a node](help:restart-a-node).

### Restarting

A node state: you restarted it from SparkBox, and it's on its way back.

### Retransmit

Sending data again because it was lost. A little is normal. A lot slows a model down.

### RNR NAK retries

A fabric counter: the receiver wasn't ready, so the sender waited and tried again.

### RoCE

Short for RDMA over Converged Ethernet: RDMA running on an ordinary Ethernet network, such as your fabric.

### Root

The account on a node that can do anything. See [SSH, sudo and host keys](help:ssh-sudo-and-host-keys).

### RouterOS

The system that runs on your MikroTik switch.

### Running and waiting

A number on an endpoint's card. **Running** is requests being served. **Waiting** is requests queued for a place.

## S

### safetensors

The usual file format for model weights on Hugging Face. vLLM reads it.

### Scenario

A problem the simulated cluster acts out with Demo data, such as a node going offline. You see the same alerts and messages as on a real cluster, with nothing at risk. See [Settings: Demo scenarios](help:settings-demo-scenarios).

### Separate window

A SparkBox window moved off the desk with **Open in Separate Window**.

### Sequence errors

A fabric counter: data arrived out of order, and had to be sent again. See [Out of sequence](#out-of-sequence).

### Serve

What an engine does with a loaded model: it answers requests.

### Server

The SparkBox feature that gives apps on this Mac one address for all your models. See [Server](help:server).

### Service

A program a node keeps running in the background, such as Docker or SSH. See [Services and processes](help:services-and-processes).

### Shell

The command line you get when you log in to a node.

### Shut down

To turn a node off. It stays off until someone presses its power button, or until Wake-on-LAN works. See [Shut down and wake a node](help:shut-down-and-wake-a-node).

### Sidebar

The list on the left of the SparkBox window.

### Signal

A short message the system delivers to a program, such as a request to stop. See [See and stop processes](help:see-and-stop-processes).

### Silence

To stop hearing about an alert for a while. **Unsilence** ends it early. See [Respond to an alert](help:respond-to-an-alert).

### Snap

A kind of software package on Ubuntu, kept up to date apart from apt.

### Spark

One of your computers, thought of as a product. See [DGX Spark](#dgx-spark).

### SparkBox app

Your normal copy of SparkBox. Help says "the release app" only to tell it apart from the [test build](#test-build).

### Sparse MLA

A memory-saving way some large models handle attention. On a GB10, the one vLLM [GPU kernel](#kernel-gpu) for it needs an fp8 KV cache.

### Split

A column in the fit table: how a model would divide across nodes. **TP 2** is tensor parallel across 2 nodes. **PP 3 only** means pipeline parallel is the only way.

### SSH

The secure connection SparkBox uses for everything on a node. See [SSH, sudo and host keys](help:ssh-sudo-and-host-keys).

### SSH agent

A program that keeps your keys and signs logins for you. It might be macOS's own, 1Password or Secretive. See [SSH, sudo and host keys](help:ssh-sudo-and-host-keys).

### SSH key

A pair of matching files that lets you log in without a password. The private file stays on this Mac, and the public file goes on each node. See [Set up key login](help:set-up-key-login).

### Stale

A node state: connected, but no new reading for over 3 seconds.

### Status bar

The bar at the bottom of the SparkBox window. See [The top bar and status bar](help:top-bar-and-status-bar).

### Stopped

An endpoint state: it isn't running.

### sudo

A command that runs something as root. It asks for the cluster password. SparkBox sends the password privately, never on a command line. See [SSH, sudo and host keys](help:ssh-sudo-and-host-keys).

### Sweep

One [benchmark](#benchmark) run. It sends test requests of an exact length, first one at a time, then more and more at once. See [Benchmark a model](help:benchmark-a-model).

### Switch

A box that connects network cables. Yours are the CRS804 DDQ, for the fabric, and the Firewalla Switch X, for the LAN.

### Symbol errors

A fabric counter: bad signal on a link. It usually points at an optic or a cable.

### System prompt

A standing instruction for the model, such as "Answer in one short paragraph." Click **System** in [Chat](help:chat) to set one.

### systemd

The part of Ubuntu that starts and watches services.

## T

### Task

Anything SparkBox is doing, listed in **Tasks**. See [Tasks and jobs](help:tasks-and-jobs).

### Tensor parallel

A way to split a model: every layer is divided across nodes, which work on each step together. It adds room and speed. See [Running one model on several nodes](help:splitting-a-model).

### Tensor × pipeline parallel

The field on the Load page that sets the split. The two numbers multiplied must equal the number of nodes. **TP** is tensor parallel, and **PP** is pipeline parallel.

### Terminal

The window for command-line access to a node. See [Terminal](help:terminal).

### Test build

A separate copy of SparkBox used for testing. Unlike the SparkBox app, it can change a node only inside an open fence. See [The Fence](help:the-fence).

### This Mac

The Mac running SparkBox.

### Token

A small piece of text, about three quarters of a word. Models read and write in tokens. See [Tokens and context](help:tokens-and-context).

### Token (Firewalla)

A long secret that lets SparkBox read your Firewalla account. It isn't the kind of token a model reads. See [Connect Firewalla's cloud](help:connect-firewalla-cloud).

### Token (Hugging Face)

A long secret that proves who you are to Hugging Face. It's a kind of [API key](#api-key). See [Get a Hugging Face token](help:get-a-hugging-face-token).

### Tokens per second

How fast a model reads or writes, shown as **tok/s**.

### Tool calling

Lets a model ask an app to run a tool, such as reading a file. Coding apps depend on it.

### Tool-call parser

The load setting that reads a model's tool requests. Each model family has its own.

### Top bar

The bar at the top of the SparkBox window.

### Touch ID or your Mac password

How you confirm it's you for a change.

### TTFT

Short for time to first token: how long you wait before the reply starts.

### Tunnel

A private path through an existing connection. SparkBox uses SSH tunnels to open a page that only listens on a node. The VPN is a tunnel too.

## U

### Unified memory

The one memory pool a Spark's CPU and GPU share. Each Spark has about 121.7 GiB. See [How memory works on a Spark](help:memory-on-a-spark).

### Unit

Something systemd manages, each with a small settings file. Most are services, such as `ollama.service`. Others do other jobs, such as a `.mount` or a `.timer`.

### Unload

To stop serving a model. It stays on disk. See [Unload a model](help:unload-a-model).

### Unlock and lock

What you do with [Admin mode](#admin-mode). "Admin mode relocks" means it locked by itself.

### Unsilence

To end a [silence](#silence) early.

## V

### Verified

A copy of a model checked against Hugging Face's checksums or its source copy's SHA-256 manifest. The Library shows **✓ verified**, and its tip names the source of verification. Older copies checked only by size still show **✓ sizes match**.

### Vision encoder

The part of a model that reads images. It adds to the memory the model needs.

### vLLM

The main engine SparkBox uses to serve models. See [Engines: vLLM, SGLang and Ollama](help:engines).

### Volume

Storage a container keeps outside itself, so that it outlives the container. On this Mac, a volume is a disk that appears in `/Volumes`.

### VPN

A private tunnel to your home network, for when you're away. SparkBox's connects to the Firewalla Gold. See [VPN](help:vpn).

## W

### Wake

To turn on a node that's off, over the network.

### Wake-on-LAN

The feature that lets a special network packet turn a node on. It must be turned on at the node. See [Shut down and wake a node](help:shut-down-and-wake-a-node).

### Warning

The less serious of the two alert severities. It needs attention soon.

### Webhook

A web address that receives a message. Many services can use one to start something, such as a post in a chat room.

### Weights

The model's files: the numbers it learned. They're most of its size.

### What vLLM needs

A row on the Load page: how much memory must be free for vLLM to start. Its startup check counts the page cache as used.

### What will run

The part of the Load page that lists the exact commands a load runs on each node, before you start it.

### Window

One SparkBox window on the desk.

### WireGuard

The standard behind your VPN.

### Worker

Any node of a load other than the [head node](#head-node). Workers start first and wait for it.
