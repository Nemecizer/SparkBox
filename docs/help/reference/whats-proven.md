---
id: whats-proven
title: What's proven on your cluster
type: reference
area: reference
summary: How to distinguish offline checks from verification on your own hardware.
keywords: [proven, tested, verified, not yet, test systems, look-only, real cluster, tried, tried for real, tested for real, proven on my cluster]
verified: null
related: [whats-coming, using-help, use-look-only-mode]
---

# What's proven on your cluster

A new installation has not tested your cluster. SparkBox starts in **Demo**, with no saved nodes or switches. The browser Demo, Help pictures, node names and addresses are synthetic examples. They are not saved connection settings or reports from your devices.

## What the checks mean

| Check | What it establishes |
|---|---|
| Offline tests | Behavior against fixtures, simulated nodes and local test servers |
| Demo | How the interface behaves with simulated data; no proof about your devices |
| Build and signature checks | That the application compiles and its signed package can be verified |
| Your own Live checks | Results for the devices, software, workload and time you tested |

Help's verification labels are not a record of checks on your hardware. A page without a warning is not a promise that its actions have passed on your cluster. No private installation history is included in this distribution.

## On test systems only

Offline checks cover parsing, safety gates, package-plan refusal, download consent and disk preflight, saved histories and connection recovery against fake nodes. They cannot prove GPU compatibility, performance, real network recovery, long-running reliability or the effect of a package change on your devices.

Protected updates require an exact supported plan and all prerequisites. A plan that worked with one software version is not permission to apply a different kernel or driver update. Never bypass a failed health check.

## Verify your own setup

1. Follow [Connect your cluster](help:connect-your-cluster). Enter your own addresses and account, and compare every node's SSH host key independently.
2. Start with monitoring in [look-only mode](help:use-look-only-mode). Check that each node is the device you intended and that the readings agree with it.
3. Review the prerequisites and proposed action before unlocking Admin. Try a change on one node first.
4. For model serving, verify a fresh Chat reply, then unload normally and review the cleanup report.
5. For updates, verify reconnection, package state, boot and service health before proceeding to another node.

Keep your own records privately. Redact credentials, addresses and device identifiers before sharing a report.

## Related

- [What's coming](help:whats-coming)
- [Find your way around Help](help:using-help)
- [Use look-only mode](help:use-look-only-mode)
