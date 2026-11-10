---
id: prepare-the-cluster
title: "Prepare the cluster for models"
type: howto
area: models
summary: "Check Docker, disk space, the engine image, the GPU and the fabric on every node before your first load."
keywords: ["prepare", "check nodes", "ready", "first load", "before my first load", "docker group", "can sparkbox use docker"]
verified: false
related: [models-prepare-cluster, load-a-model-on-one-node]
---

# Prepare the cluster for models

To check that every node can run a model, run **Check nodes** on the **Prepare cluster** tab before your first load.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. With **Demo** data, the table shows simulated results.
- The check needs Admin mode because it starts short containers on each node. A **container** is a program running in its own sealed box. SparkBox removes these when they end.
- Your nodes are online: each one shows its GPU use in the sidebar, such as **GPU 28%**.
- It takes about a minute the first time.

## Steps

1. In the sidebar, under **Workloads**, click **Models**. [Open Prepare cluster](open:models?tab=prepare) The **Models** window opens.
2. Click the **Prepare cluster** tab.
3. Click **Check nodes** in the toolbar. The button in the middle of the tab does the same thing. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. The button reads **Checking…** while it runs.
4. Read the table when it appears. There's one row for each node. The next section says what each column should show, and what to do if it doesn't.
5. Fix anything marked **✗**, then click **Check nodes** again. **✗ will be created** is the one exception, and it needs no fix.

## Read the results

| Column | Good result | If it isn't |
|---|---|---|
| **Docker** | **✓ yes** | **✗ no** means SparkBox can't run containers on the node. Docker is the program that runs them. See [Fix Docker access](#fix-docker-access) |
| **Model folder** | **✓ exists** | **✗ will be created** needs no fix, because the first download makes the folder |
| **Free disk** | More than the model you plan to download | Free some space. See [Check disk space on a node](help:check-disk-space) |
| **vLLM image** | **✓ on this node** | **not on this node; a load won't fetch it** means a load stops and asks you to fetch the image first. Downloads never fetch an image automatically; they ask you to choose a tool. See [Load a model on one node](help:load-a-model-on-one-node#if-something-goes-wrong) |
| **vLLM** | A version number | **—** means the image isn't on the node yet, so there's no version to read |
| **Fabric (ConnectX-7)** | Addresses, each at **200 Gb/s** | **✗ no address** means the node can't join a several-node load. **link down** means the cable or port isn't working. See [Check the fabric](help:check-the-fabric) |
| **MTU and RDMA** | **MTU 9000 · RDMA ACTIVE**, once for each half | Any other MTU can stop a several-node load, because its traffic needs large packets. Any other RDMA word means that half can't carry fast traffic. See [Check the fabric](help:check-the-fabric) |
| **GPU in container** | **✓**, with the GPU's name | **✗ a container sees no GPU** means the check completed but listed no GPU. **not checked: the image isn't on this node** clears once the node has the image. **not checked** alone means this check didn't run, failed, or ran past its 60-second limit. Check Docker and the image, then click **Check nodes** again |
| **rsync** | **✓ yes** | **✗ no** means this node can't take part in copies |

> [!NOTE]
> The results stay after you leave the tab, and after SparkBox quits. The toolbar says how old they are, such as "Checked 2 min ago". They're what the nodes said then, so click **Check nodes** again after you change anything. The **Load** page uses the fabric addresses to show its commands.

### Fix Docker access

If **Docker** reads **✗ no**, your login on that node probably isn't in the `docker` group, the list of users allowed to use Docker. The message "cell2: SparkBox can't use Docker (is sparkuser in the docker group?)" means the same thing.

> [!WARNING]
> **Anyone in the `docker` group can do what root can on that node.** This fix lets your account do that, which SparkBox needs to run models. It adds only your own login, `sparkuser`. To undo it, run `sudo gpasswd -d sparkuser docker` the same way, then quit SparkBox and open it again. SparkBox can't run models on that node after that.

1. Open a [terminal](help:open-a-terminal) on that node. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
2. Type `sudo usermod -aG docker sparkuser` and press <kbd>Return</kbd>.
3. When the node asks, type your cluster password and press <kbd>Return</kbd>. Nothing appears as you type it.
4. Quit SparkBox with <kbd>⌘Q</kbd> and open it again. The change reaches only new logins, so SparkBox has to log in to the node again.
5. Open the **Prepare cluster** tab in the **Models** window, and click **Check nodes**. **Docker** now reads **✓ yes**.

If it still reads **✗ no**, read the message above the table. See [Connection and login messages](help:errors-connection).

## What you should see

Every row has no **✗**, apart from **✗ will be created**. If any node shows a version under **vLLM**, the toolbar also says "Started short containers on each node to read the vLLM version and see the GPU; they're removed when they end."

## Why it works

SparkBox's helper on each node looks at Docker, the disk that holds your model folder, the vLLM image from [Settings: Models](help:settings-models), and the fabric links. To read the vLLM version and see the GPU from inside the image, it starts short test containers from the image, then removes them. Nothing else on the node changes.

## If something goes wrong

- **A row reads "not checked", with a message above the table.** SparkBox couldn't finish the check on that node. Read the message, and check that the node is online. See [Connection and login messages](help:errors-connection).
- **"A fabric link is below 200 Gb/s. On the CRS804, set those ports to 200G-baseCR4 if auto-negotiation came up short."** A cable or port came up at a lower speed. See [VPN, fabric and switch messages](help:errors-network#a-fabric-link-is-below-200-gbs-on-the-crs804-set-those-ports-to-200g-basecr4-if-auto-negotiation-came-up-short).
- **"A node has no 192.168.100.x address. SparkBox uses whatever ConnectX-7 address it has, but every node should be on the same subnet."** One node's fabric address is missing or on a different network. See [VPN, fabric and switch messages](help:errors-network#a-node-has-no-192168100x-address-sparkbox-uses-whatever-connectx-7-address-it-has-but-every-node-should-be-on-the-same-subnet).
- **Check nodes is dimmed.** SparkBox is in look-only mode. See [Use look-only mode](help:use-look-only-mode).

## Next steps

- [Find a model that fits](help:find-a-model-that-fits)
- [Load a model on one node](help:load-a-model-on-one-node)
- [Models: Prepare cluster tab](help:models-prepare-cluster)
