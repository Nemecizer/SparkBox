---
id: getting-started
title: Getting started with SparkBox
type: tutorial
area: start
summary: Learn SparkBox in Demo, rehearse running a model, connect your real nodes, then run a real model.
keywords: [start, begin, first time, new, new user, beginner, im new]
verified: null
related: [tour-the-demo, connect-your-cluster, your-first-model, run-your-first-real-model, what-is-sparkbox, using-help, download-a-model, load-a-model-on-one-node, chat-with-a-cluster-model]
---

# Getting started with SparkBox

A new installation has no saved nodes or switches. Add your own nodes in **Settings › Nodes** before following the native Demo exercises. Demo uses that list without connecting to the nodes. The four-node pictures use synthetic example data; their addresses and accounts are not your network settings.

## What you'll do

You'll learn SparkBox safely, connect it to your real nodes, and run your first real model. There are five steps. Steps 1 and 2 happen in **Demo**, a simulated copy of your cluster, so nothing you do there can reach a real node. Steps 3 to 5 use your real nodes.

This page is the map, and each step links to its full guide. You can stop after any step and come back later.

> [!NOTE]
> **Already on Live?** If the top bar reads **Live**, you've already connected your real nodes, so go to [Step 5](#step-5-run-a-real-model). To rehearse first, [switch to Demo](help:switch-between-demo-and-live), then start at Step 1.

## Before you start

- SparkBox is installed on this Mac.
- For steps 1 and 2, you need nothing else, apart from internet access for the model search in step 2.
- For steps 3 to 5, you need your configured nodes turned on and connected to your LAN, and your own node account and password.
- If you're new to these ideas, read [What SparkBox is](help:what-is-sparkbox) first. It takes a couple of minutes.

## Step 1: Look around in the Demo

A new native installation starts in **Demo** with an empty node list. The browser Demo supplies the synthetic four-node examples used in this tour. Native Demo simulates only the inventory you configure. Nothing you do here can reach a real node, so it's a safe place to learn.

1. Open SparkBox. The **Welcome to SparkBox** screen appears.
2. Click **Explore the demo**. The **Dashboard** and **Topology** windows open side by side.
3. Look at the top bar. A **Demo** label sits beside the padlock, and the padlock reads **Read-only**.
4. If your Demo has nodes, in the sidebar under **Nodes**, click one of them (`cell1` in the browser example). The node's own window opens, with its figures moving.

To go further, follow [Take a tour of the Demo](help:tour-the-demo). If the Welcome screen doesn't appear, choose **Help › Show Welcome** in the menu bar.

## Step 2: Rehearse running a model

A **model** is a trained AI: a very large set of learned numbers, stored as files, that lets a computer read and write text. To run one, SparkBox downloads its files to a node, then starts it in a **container**, a program running in its own sealed box.

This step is a rehearsal. The search for a model is real. The download, the load and the chat are simulated, so nothing reaches your nodes and the answer you read isn't real. If the top bar reads **Live**, [switch to Demo](help:switch-between-demo-and-live) first, and switch back to **Live** when you finish.

1. Follow [Rehearse running a model in Demo](help:your-first-model) from start to finish. It takes about 10 minutes.
2. At the end, you ask the simulated model a question in **Chat**, read its answer, and unload it.

## Step 3: Connect your cluster

Connecting has three parts: you save the password your nodes share, you switch to **Live** data, and you check each node. A wizard called **Connect my Sparks** guides you.

1. Follow [Connect your cluster](help:connect-your-cluster) from start to finish.
2. Look at the top bar. It now reads **Live** and **4/4 online**. Each node in the sidebar shows its GPU use, such as **GPU 0%**. 0% is normal when no model is running.

You're now looking at your real cluster, and from here on your actions reach your real nodes. It's still safe to look around: SparkBox won't make a change you ask for until you unlock Admin mode.

## Step 4: Learn how to allow changes

Every change you ask for needs **Admin mode**, which you unlock yourself.

1. Find the padlock in the top bar. It reads **Read-only**.
2. Click it. macOS asks you to confirm. Use Touch ID or your Mac password.
3. Look at the padlock. It reads **Admin**, with a clock counting down.
4. Click the padlock again. It reads **Read-only** again.

You can now unlock whenever you need to. Admin mode locks by itself after 15 minutes without a change. [Unlock Admin mode](help:unlock-admin-mode) has the details.

## Step 5: Run a real model

This is the path you rehearsed, now on your real nodes. A loaded model uses the node's memory until you unload it. [What's different for real](help:your-first-model#whats-different-for-real) lists the rest.

1. Follow [Run your first real model](help:run-your-first-real-model). Choose a model available on your own selected node; the names in that guide are examples.

To run a model you don't have yet, follow these in order. The download takes up disk space on the node. How long it takes depends on your internet speed and the model's size.

1. If this is your first model, check your nodes first. Follow [Prepare the cluster for models](help:prepare-the-cluster).
2. Choose a model. Follow [Find a model that fits](help:find-a-model-that-fits).
3. Fetch its files. Follow [Download a model](help:download-a-model).
4. Start it. Follow [Load a model on one node](help:load-a-model-on-one-node).
5. Ask it a question. Follow [Chat with a cluster model](help:chat-with-a-cluster-model). This answer comes from your own node.
6. When you're done, free its memory. Follow [Unload a model](help:unload-a-model).

[What's proven on your cluster](help:whats-proven) explains the difference between offline checks and validation on your own devices.

## What you learned

- Demo is safe to explore, and only Live reaches your real nodes.
- The top bar always shows whether you're in Demo or Live, and whether changes are allowed.
- A change you ask for needs Admin mode, which you unlock with Touch ID or your Mac password.
- **Connect my Sparks** sets up your cluster, and you can run it again at any time.
- A model's path is the same in Demo and on your real nodes: find, download, load, check and unload.

## Next steps

- [Read the Dashboard](help:read-the-dashboard), to see how your cluster is doing
- [Respond to an alert](help:respond-to-an-alert), to act when something needs your attention
- [Install updates on a node](help:install-updates), to keep your nodes current
- [Find your way around Help](help:using-help), to learn the info icons, search and contents
