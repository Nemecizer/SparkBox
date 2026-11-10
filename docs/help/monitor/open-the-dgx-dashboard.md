---
id: open-the-dgx-dashboard
title: Open the DGX Dashboard
type: howto
area: monitor
summary: Open NVIDIA's own dashboard for a node in your browser, through a private SSH tunnel.
keywords: [dgx dashboard, nvidia, tunnel, port 11000, browser]
verified: false
related: [node-window, settings-security]
---

# Open the DGX Dashboard

To see NVIDIA's own dashboard for a node, open it from the node's window, and it appears in your browser.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. The **DGX Dashboard** runs on the node, so the button is off in Demo. [Switch between Demo and Live](help:switch-between-demo-and-live) shows how to change.
- A page on a node can do what a shell there does, so SparkBox treats it as a change.
- The node must be online. In the sidebar, it shows its GPU use, such as **GPU 28%**.

The **DGX Dashboard** is a web page that comes with DGX OS. It listens only on the node itself, on port `11000`, so your Mac can't reach it directly. SparkBox builds a private path to it for you.

> [!DANGER]
> **An update from this page can stop a node from starting, and it can't be undone.** [NVIDIA says](https://build.nvidia.com/spark/dgx-dashboard/instructions) its system updates change software and [firmware](help:glossary#firmware), then restart the node. They skip SparkBox's guard on the [GPU stack](help:glossary#gpu-stack) and its [checks before a restart](help:restart-a-node#if-a-check-refuses-the-restart). If the node doesn't start, someone has to fix it at the machine. NVIDIA's only way back is a system backup or recovery media. Use this page to look. Don't install updates or restart the node from it.

## Steps

1. In the sidebar, under **Nodes**, click the node, such as `cell1`. The node's window opens.
2. In the toolbar, click **DGX Dashboard**. If you don't see it, make the window wider, or look under **More** in the toolbar.
3. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
4. Wait a moment. Your default browser opens NVIDIA's dashboard for that node.

To open it from anywhere, press <kbd>⌘K</kbd> and type `DGX Dashboard on cell1`.

## What you should see

Your browser shows the **DGX Dashboard**, at an address that starts with `http://127.0.0.1:`. That address is your own Mac: the page reaches the node through the tunnel. The **Activity log** records "Opened the DGX Dashboard of cell1".

## Why it works

SparkBox opens an SSH tunnel: a private path through the secure connection it already has to the node. It opens one only to a port the node says it's listening on, with a random token, so another program or a web page can't use it. The tunnel closes by itself after 15 minutes without use, when Admin mode relocks, and when SparkBox quits. Validate the results on your own equipment before relying on them. To close it sooner, open [Security settings](open:settings?section=security) and click **Close** beside it.

## If something goes wrong

- **The button is dimmed.** You're on Demo data, or the node isn't online. Move the pointer over the button to see which.
- **"SparkBox hasn't had cell1's listening ports yet; try again once it shows data."** The node connected moments ago. Wait a few seconds, then try again.
- **"cell1 isn't listening on port 11000, so there's no page to open."** The dashboard isn't running on that node. Check its service in [Services](open:services). Then [Restart, disable or enable a service](help:restart-a-service) shows how to start it.
- **The page stops working.** The tunnel closed. Click **DGX Dashboard** again. A new tunnel may use a different local port.
- **You think a node was updated from this page.** Open [Fleet Command](open:fleet), and click **Versions** in the toolbar. Compare **DGX OS (OTA)**, **Kernel** and **NVIDIA driver** across your nodes. A version that isn't the same as most nodes shows **differs**. A model spread over several nodes needs the same driver on all of them. The **Activity log** records that you opened the page, not what you did on it.

## Next steps

- [Settings: Security & privacy](help:settings-security), to see open tunnels
- [Find out what SparkBox did](help:find-what-sparkbox-did), to see the tunnel in the log
- [A node's window](help:node-window)
