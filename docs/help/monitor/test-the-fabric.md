---
id: test-the-fabric
title: Test the fabric
type: howto
area: network
summary: Run the Doctor tab in Fabric to measure real speed and latency between nodes.
keywords: [perftest, bandwidth, speed test, jumbo, mtu, doctor, latency, ib_write_bw]
verified: false
related: [fabric-doctor, check-the-fabric, fabric-and-rdma]
---

# Test the fabric

To measure how fast your fabric really is, run the **Doctor** tab in **Fabric**.

## Before you start

- Use **Live** data with look-only mode off, and be ready to unlock Admin mode. The test runs commands on your nodes. With **Demo** data, it runs against the simulator, so it's a safe way to learn the screen.
- At least two nodes must be online. An online node shows its GPU use in the sidebar, such as **GPU 28%**.
- The speed tests need a tool called perftest on each node. If it's missing, SparkBox offers to install it. That changes the node, so read [If perftest is missing](#if-perftest-is-missing) first.
- The test takes minutes, not seconds. Each bandwidth test runs for 5 seconds, one pair of nodes at a time, on each half.

> [!WARNING]
> **The bandwidth test fills the link.** It sends as much data as it can, which slows anything else using the fabric, such as a model that spans several nodes. Run it when nothing important is serving. To undo it, click **Stop**. The test itself leaves nothing on the nodes.

## Steps

1. In the sidebar, under **Network**, click **Fabric**. [Open the Doctor tab](open:fabric?tab=doctor) The **Fabric** window opens.
2. Click the **Doctor** tab. You see **Nodes**, **Checks** and **Expected per half**.
3. Under **Nodes**, leave every online node selected, or select the ones you want to test. You need at least two.
4. Under **Checks**, leave the first four selected: **MTU audit**, **Jumbo-frame ping**, **Bandwidth (ib_write_bw)** and **Latency (ib_send_lat)**.
5. Leave **Expected per half** at `92` Gb/s. That's what one half of a GB10's fabric port delivers.
6. Click **Test fabric**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
7. Wait. The report's heading shows what's running, such as "cell1 → cell2, half 1", and each cell fills in as it finishes. To stop early, click **Stop**.
8. If a yellow notice says perftest is missing, follow [If perftest is missing](#if-perftest-is-missing).


### If perftest is missing

The notice reads like "perftest isn't installed on cell2: ib_write_bw and ib_send_lat are missing, so its pairs were skipped." The rest of the test runs without that node. To include it, install perftest on it.

> [!WARNING]
> **Installing perftest changes the node, and the change stays after the test.** apt, the tool that installs software on a node, adds perftest and any packages it needs. perftest needs `libibverbs1`, which is part of the [GPU stack](help:glossary#gpu-stack), and apt upgrades it if the node's copy is too old. SparkBox doesn't check this first, although its dialog says "nothing else changes". To avoid a surprise, do steps 1 to 6 before you click **Install…**. To undo the install, follow [Remove perftest](#remove-perftest).

With **Demo** data, go straight to step 7. Steps 1 to 6 have no simulated answer, and nothing real changes.

1. In the sidebar, under **Tools**, click **Fleet Command**. [Open Fleet Command](open:fleet) The **Fleet Command** window opens.
2. In the toolbar, after **on**, click the other nodes to leave them out. Only the node in the notice stays selected.
3. Click the command box, press <kbd>⌘A</kbd> to select what's there, then type `apt-get -s install perftest`. The badge reads **read-only**. `-s` tells apt to plan without changing anything.
4. Click **Run**. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
5. Read the node's card. Each line that starts `Inst` is a package apt would add or upgrade. Each line that starts `Remv` is one it would remove.
6. Look for two things. A line that starts `Remv` means apt would remove something. Square brackets right after a package's name, as in `Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 …)`, mean apt would upgrade a package that's already there. If you see either, stop here and leave perftest off this node. Otherwise, go on.
7. In the **Fabric** window, on the **Doctor** tab, click **Install…** on the notice. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password. A dialog named **Install perftest on cell2?** opens.
8. Click **Install**. This changes the node. A task named **Install perftest** starts.
9. Open [Tasks](open:tasks), and wait until that task's **State** reads **Done**.
10. Go back to the **Doctor** tab, and click **Test fabric** again. That node's pairs now run.

### Remove perftest

SparkBox has no button for this, so you use a command as root. **Root** is the account that can do anything on a node.

1. In the sidebar, under **Tools**, click **Fleet Command**. [Open Fleet Command](open:fleet) The **Fleet Command** window opens.
2. In the toolbar, after **on**, click the other nodes to leave them out. Only the node you want to change stays selected.
3. Click the command box, press <kbd>⌘A</kbd> to select what's there, then type `apt-get remove -y perftest`. `-y` answers yes for you, because a Fleet command can't stop to ask.
4. Click **as root**. The `$` at the start of the box changes to `#`, and the badge reads **runs as root**.
5. Click **Run**. A dialog named **Run on 1 node as root?** opens.
6. Click **Run** in the dialog. If Admin mode is locked, the **Unlock Admin mode?** dialog opens. Click **Unlock…**, then confirm with Touch ID or your Mac password.
7. Read the node's card. **exit 0** means perftest is gone.

This removes perftest alone. Packages that came with it stay, and it doesn't reverse anything apt upgraded. That's why the check in [If perftest is missing](#if-perftest-is-missing) comes first.

## What you should see

The heading reads **Everything passed**, or a count of findings. Under it:

- **MTU audit:** every row reads **OK**, with MTU 9000.
- **Half 1** and **Half 2:** a grid of nodes. Each cell reads something like "ping ok · 0.12 ms" and "92.1 Gb/s · 2.01 µs". Latency is about 2 µs between Sparks through the switch.
- **Both halves:** each pair's two halves added together. **Together** should be near **Expected**, about 184 Gb/s.

The first run where every pair passes is saved as your **baseline**, and the heading adds "saved as the baseline". Later runs compare against it.

## Why it works

The checks go from simple to demanding, so a failure points at its cause. **MTU audit** and **Jumbo-frame ping** prove the whole path carries large packets. **Bandwidth** and **Latency** then measure RDMA, the fast direct path that models use. A pair fails below 85% of **Expected per half**. It gets a warning below 85% of its own baseline, which can show a link getting worse while it still passes.

## Limits

- **All-reduce (nccl-tests)** is off by default. It needs nccl-tests built on the nodes, which SparkBox doesn't install.
- Each pair is measured once, in one direction, from the first node to the second.
- Only one fabric test can run at a time.
- **Install…** doesn't do the dry run or the GPU-stack check that updates do. SparkBox has no button to remove perftest. [If perftest is missing](#if-perftest-is-missing) and [Remove perftest](#remove-perftest) do both by hand.

## If something goes wrong

- **"Choose at least two nodes: the tests run between pairs."** Select another node.
- **"fails: MTU".** The ping couldn't get a large packet through. If small packets work, something on the path allows less than 9000. Check the **MTU audit** table for the node or switch port that differs.
- **"unreachable".** Nothing came back on that half. Check that half's link in [Fabric](open:fabric).
- **"61 Gb/s, below 85% of the expected 92 Gb/s".** The link is slower than it should be. Look for errors in [Check the fabric](help:check-the-fabric).
- **"Timed out after 30 s (is port 18515 blocked, or the peer's RDMA device down?)".** The speed test couldn't start between the pair. Check that both nodes show the half as up.
- **"The fabric test is already running".** Wait for it to end, or open [Tasks](open:tasks).
- **"E: Unable to locate package perftest".** apt doesn't know this package on that node. Run [Check for updates now](help:check-for-updates) to refresh its list, then try again. If you still see it, leave that node out.
- **"apt-get finished but ib_write_bw still isn't on the PATH".** The install ended, but SparkBox can't find the tool. Open the task in [Tasks](open:tasks), and read its log.

## Next steps

- [Fabric: Doctor tab](help:fabric-doctor)
- [Check the fabric](help:check-the-fabric)
- [Load a model on two, three or four nodes](help:load-a-model-on-several-nodes)
