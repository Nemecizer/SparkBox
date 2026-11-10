---
id: what-is-sparkbox
title: What SparkBox is
type: concept
area: start
summary: SparkBox is a Mac app that shows you and helps you run your DGX Spark computers from one place.
keywords: [about, overview, what does it do, internet, need the internet, without the internet, account]
verified: null
related: [your-cluster, getting-started, demo-and-live, admin-mode]
---

# What SparkBox is

## In short

SparkBox is a Mac app for your NVIDIA DGX Spark computers. It shows you how they're doing, and it lets you run AI models on them without typing commands. You do it all from one window on this Mac.

## How it works

SparkBox runs on this Mac. Your four computers, called **nodes**, stay where they are. SparkBox reaches each node over **SSH**, the secure connection computers use to log in to each other.

On each node, SparkBox runs a small helper called the **collector**. The collector only reads. Once a second it sends back figures such as GPU load, memory use and temperature, and SparkBox draws them for you.

When you ask for a change, such as restarting a service, SparkBox sends the command over the same secure connection. It writes every command to the [activity log](help:activity-log) on this Mac, so you can always see what it did.

SparkBox has no cloud service of its own, and you don't need an account. It uses the internet only for things you ask for, such as a Hugging Face download or a chat with a cloud model.

### What you can do with it

| Goal | What SparkBox gives you | Where to find it |
|---|---|---|
| See how the cluster is doing | Live figures, charts and alerts | **Dashboard**, **Topology**, **Alerts** |
| Run AI models | Find, download, copy, load and test models | **Models**, **Endpoints**, **Ollama** |
| Use those models | Chat with them, and share them with other apps | **Chat**, **Server**, **Providers** |
| Look after your nodes | Updates, services, containers and commands | **Updates**, **Services**, **Containers**, **Fleet Command** |
| Work on one node | A terminal, and a file browser | **Terminal**, **Files** |
| Check the network | The fast network, your switches and the VPN | **Fabric**, the switch windows, **VPN** |

### An example

Say you want to run a model on `cell1`. You pick the model on the **Hugging Face** tab of **Models** and choose `cell1` in **Download node**. You click **Load…**, and SparkBox shows whether it will fit in memory. It then starts the model and tells you when it's ready to answer. You didn't need a Docker command, a port number or a memory setting. SparkBox shows you all of them if you want to see them.

## Why it matters in SparkBox

Two things make SparkBox safe to explore, and you meet both on your first day:

- **It starts with a simulated cluster.** A new SparkBox shows **Demo** data: a built-in copy of your cluster where nothing is real. [Demo and Live](help:demo-and-live) explains this.
- **It waits for your say-so.** The padlock in the top bar reads **Read-only**, and SparkBox won't make a change you ask for until you unlock Admin mode with Touch ID or your Mac password. [Read-only, Admin mode and Look only](help:admin-mode) explains this.

To start, follow [Getting started with SparkBox](help:getting-started). To see what isn't built yet, read [What's coming](help:whats-coming).

## Related

- [Getting started with SparkBox](help:getting-started)
- [Your cluster at a glance](help:your-cluster)
- [Demo and Live](help:demo-and-live)
- [Read-only, Admin mode and Look only](help:admin-mode)
