---
id: ports-reference
title: Ports and addresses
type: reference
area: reference
summary: The network ports and address ranges your cluster and SparkBox use.
keywords: [port, 8000, 11434, 11000, 8484, 9469, ip address, 192.168, lan, fabric, mtu, firewall, what port does sparkbox use, which ports, sparkbox ports]
verified: null
related: [endpoints-and-apis, your-cluster, fabric-and-rdma]
---

# Ports and addresses

This page lists the network addresses your cluster uses and the ports SparkBox and your models use.

An **IP address** says which computer. A **port** is a numbered door on a computer that a program answers on, so one computer can run several programs. In `192.168.50.154:8000`, the address is `cell2` and the door is port `8000`.

## Your networks

| Network | Addresses | Speed | What it's for |
|---|---|---|---|
| The LAN | `192.168.50.x` | The nodes plug into a 2.5G switch, which connects to the 10 GbE Firewalla Switch X. Each Spark's LAN port runs at 1G | Your ordinary home network. SparkBox reaches the nodes here. So do your apps, when a model listens on the LAN |
| The fabric, half 1 | `192.168.100.x` | One 200G cable, shared by both halves | The fast network between nodes, through the CRS804 DDQ. Models and copies use it |
| The fabric, half 2 | `192.168.101.x` | The same cable | The second path over the same cable |

Your Mac isn't on the fabric. Anything that listens only on the fabric can't be reached from this Mac.

Each node's fabric port reaches the node through two separate paths, called halves, and each half has its own address. The fabric uses jumbo frames, large network packets of 9000 bytes, so its MTU, the largest packet size, is `9000`. [The fabric and RDMA](help:fabric-and-rdma) explains why.

## Your nodes

| Node | Model | LAN address |
|---|---|---|
| `cell1` | MSI EdgeXpert | `192.168.50.51` |
| `cell2` | MSI EdgeXpert | `192.168.50.154` |
| `cell3` | MSI EdgeXpert | `192.168.50.98` |
| `gx10-0000` | ASUS Ascent GX10 | `192.168.50.181` |

SparkBox connects to this address. If a node has no address saved, it uses the node's name, such as `cell2-example.local`. [Add or change a node](help:add-or-change-a-node) shows where to change them.

## Ports on a node

**Reach** says who can get to the port. **This node only** means nothing outside the node can reach it, so SparkBox opens a tunnel for you where a page needs one.

### Always there

| Port | Service | Reach |
|---|---|---|
| `22` | SSH, the secure connection SparkBox uses for everything on a node | The LAN |
| `11000` | DGX Dashboard, NVIDIA's own page | This node only. See [Open the DGX Dashboard](help:open-the-dgx-dashboard) |
| `11434` | Ollama | Depends on its settings. Ollama often listens on this node only |
| `53` | DNS, handled by `systemd-resolved` | This node only |
| `25` | Mail, handled by Postfix | Depends on its settings |
| `631` | Printing, handled by CUPS | Depends on its settings |
| `3389`, `3350` | Remote desktop, handled by xrdp | Depends on its settings |

### Model engines

| Port | Service | Notes |
|---|---|---|
| `8000` | vLLM's API | SparkBox starts here and offers the next free port if `8000` is taken. Ports from `1024` up are allowed |
| `30000` | SGLang's API | The default port SparkBox looks for on servers it didn't start |
| `8080` | llama.cpp, or nginx | The default port SparkBox looks for on these servers |
| `29501` | vLLM's link between nodes, shown in **Workloads** as "vLLM multi-node rendezvous" | On the fabric, on the head node, for a load on several nodes. SparkBox checks that it's free on every node before a load |
| `5000` | SGLang's link between nodes | On the fabric |
| `29500` | PyTorch's link between nodes | Used by loads started outside SparkBox |
| `6379` | Ray, a program that links nodes, on servers SparkBox didn't start | SparkBox's own loads don't use it. SparkBox skips this port when it picks a free one |
| `8265` | Ray's own dashboard, on servers SparkBox didn't start | |

### SparkBox's own

| Port | Service | Notes |
|---|---|---|
| `8730` to `8929` | The short-lived file server behind a copy | On the fabric, only while a copy runs. It has a password, and only the target nodes may connect |
| `9469` | The always-on agent, talking to its peers | Only if you installed the agent |
| `9470` | The agent's metrics page, for Prometheus | Off unless you turn it on. You can change it in **Settings › Notifications** |
| `18515` | The fabric speed test in the **Doctor** tab in **Fabric** | Only while a test runs |

The **Listening ports** view in **Workloads** shows what really listens on each node right now. **Local only** there means an unknown port that only its own node can reach. See [Workloads](help:workloads).

## Listen on

**Listen on**, on the Load page, decides which network a load answers on. The address on the page changes to match.

| Choice | Address | Who can reach it |
|---|---|---|
| **LAN** | The head node's LAN address, such as `http://192.168.50.154:8000/v1` | This Mac and your network |
| **Fabric only** | The head node's fabric address | Only the other Sparks. **Chat** and the **Server** can't reach it |
| **All interfaces** | Every address on the node | Everything that can reach the node, the fabric included |

## Ports on this Mac

| Port | Service | Notes |
|---|---|---|
| `8484` | The SparkBox **Server** | It answers on `127.0.0.1` only, so other computers can't reach it. You can change the port in **Server** |
| A random port on `127.0.0.1` | A tunnel to a page on a node | It opens when you open the page, and closes after 15 minutes without use, when Admin mode locks, and when SparkBox quits |

**Expose to this Mac**, in the **VPN** window, can add ports of your own choosing. See [VPN](help:vpn).

## Other connections SparkBox makes

| Port | To | What for |
|---|---|---|
| `443`, then `80` | Your switches and gateway | Checking that they answer, and the CRS804's REST interface, the way programs talk to it |
| UDP `9` | A node's LAN port | The Wake-on-LAN packet |
| UDP `51820` | The Firewalla Gold | The VPN, unless your profile names another port |
| `443` | `huggingface.co` | Searching and reading model details from this Mac |
| `443` | Your MSP domain, under `firewalla.net` | Reading the Switch X's ports, if you [connected Firewalla's cloud](help:connect-firewalla-cloud) |

A node makes its own connection to Hugging Face for a download.

## If a port is taken

- On the Load page, the message reads "Port 8000 is already in use on cell2 (python3). Pick another port." See [Model messages](help:errors-models).
- In **Server**, it reads "Port 8484 is already used by another app. Pick a different port." See [Chat, Providers and Server messages](help:errors-ai).
- To see who has the port, open **Workloads** and choose **Listening ports**.
