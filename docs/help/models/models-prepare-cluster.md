---
id: models-prepare-cluster
title: "Models: Prepare cluster tab"
type: feature
area: models
summary: "A table showing whether each node has what it needs to load a model, kept with its age."
keywords: ["prepare", "docker", "image", "free disk", "rsync", "mtu", "rdma", "gpu in container"]
verified: false
related: [prepare-the-cluster]
---

# Models: Prepare cluster tab

## What it's for

The **Prepare cluster** tab checks that every node has what it needs to run a model: Docker, disk space, the vLLM image, a GPU a container can see, and working fabric links. Use it before your first load, and after you change a node's software or cabling.

## Where to find it

- **Sidebar:** under **Workloads**, click **Models**, then click the **Prepare cluster** tab.
- **Link:** [Open Prepare cluster](open:models?tab=prepare)


## What's on the screen

### Toolbar

**Check nodes** runs the check on every node, and reads **Checking…** while it runs. Before the first check, the middle of the tab has the same button, **Check nodes (needs Admin)…** when Admin mode is locked. The first check takes about a minute.

After a check, the toolbar says how old the results are, such as "Checked 2 min ago". If any node shows a version under **vLLM**, it also says "Started short containers on each node to read the vLLM version and see the GPU; they're removed when they end."

### The table

There's one row for each node.

| Column | What it shows |
|---|---|
| **Node** | The node's name |
| **Docker** | **✓ yes** if SparkBox can use Docker. **not checked** if the check failed on this node |
| **Model folder** | **✓ exists**, or **✗ will be created**. This ✗ needs no fix, because the first download makes the folder. Move the pointer over it for the path |
| **Free disk** | Free space on the disk that holds the model folder |
| **vLLM image** | **✓ on this node**, or **not on this node; a load won't fetch it**. [Prepare the cluster for models](help:prepare-the-cluster) explains what each one means for a load |
| **vLLM** | The engine's version, or **—** |
| **Fabric (ConnectX-7)** | Each fast network address and its speed, such as `192.168.100.1/24 @ 200 Gb/s`. A slow link shows in amber. **link down** or **✗ no address** needs fixing |
| **MTU and RDMA** | For each half of the fabric, its largest packet size and its RDMA state, such as **MTU 9000 · RDMA ACTIVE**. Move the pointer over a line for the interface and its link state |
| **GPU in container** | What a short test container sees of the GPU: **✓** with the GPU's name, **✗ a container sees no GPU** after a successful empty check, or **not checked** if the check didn't run or failed. A missing image has its own reason |
| **rsync** | **✓ yes** if the node has the copy tool |

In a narrow window, **GPU in container**, **vLLM** and **rsync** hide first.

The results stay when you leave the tab and when SparkBox quits, so that you can compare them later. They show what the nodes said at that time. SparkBox keeps them separately for **Demo** and **Live** data.

### Notes above the table

- A red note names any node where the check failed, and why.
- "A fabric link is below 200 Gb/s…" means a link came up too slow.
- "A node has no 192.168.100.x address…" means a node's fabric address is on another network.

### Prepare an engine image

Enter a tagged image under **Image tag**. Choose **Pull from its registry on each target**, or choose a source node to copy from over the fabric. Select the targets and review **Prepare image…**. Registry pulls use anonymous access to public images. Supply an unpacked size estimate; each pull reserves twice that estimate plus 20 GiB before it starts. A larger image can still exceed the estimate.

Copies export the source's linux/arm64 image, compare SHA-256 on the target, load it, check its local image ID and set the requested tag. Docker image stores must match. Each target needs twice the larger of the image or archive size, plus 20 GiB. Follow progress in **Jobs**. Normal completion and cancellation remove temporary archives; a forced process kill can leave an archive for manual cleanup. No serving container is restarted. Demo simulates progress without adding an installed image.

### earlyoom process preference

Choose a node and enter exact **Recorded comm names** from its serving processes. Linux limits these names to 15 characters; examples include `VLLM::EngineCor` and `VLLM::Worker_TP`. Do not use a broad name such as `python3`.

**Preview earlyoom edit** reads the file and shows the proposed contents. **Save and restart earlyoom…** asks for Admin mode and confirmation, preserves the existing options and avoid expression, saves a backup and restarts the service. A file changed since preview must be read again. If restarting fails, SparkBox attempts to restore the previous contents without overwriting a later edit; review the reported service state. Complex shell configuration needs manual review. An avoid rule is a preference, not a guarantee against memory kills. Demo previews only.

## What it changes

**Check nodes** starts short test containers on each node to read the vLLM version and see the GPU, so it needs Admin mode. They remove themselves. The check changes nothing else, and there's nothing to undo. If a node doesn't have the image, there's nothing to start, so that node skips this step.

In look-only mode, the check doesn't run.

Image jobs and saving the earlyoom rule need Admin mode. An image tag can be changed back to the previous image if it is still installed. The earlyoom backup is available through Files; restoring it and restarting the service also need Admin mode. Neither action is available in look-only mode.

## Related How-Tos

- [Prepare the cluster for models](help:prepare-the-cluster)
- [Check the fabric](help:check-the-fabric)
- [Check disk space on a node](help:check-disk-space)
