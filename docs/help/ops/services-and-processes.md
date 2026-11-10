---
id: services-and-processes
title: Services and processes
type: concept
area: ops
summary: A process is a running program. A service is one the system keeps running for you.
keywords: [systemd, process, service, daemon, unit]
verified: null
related: [services, workloads]
---

# Services and processes

## In short

A **process** is one program that's running right now. A **service** is a program the node starts and keeps running in the background, with no one logged in. A service runs as one or more processes.

## How it works

Everything that runs on a node is a process. Each one has a number, its **PID**, and an owner, the account it runs as. When a process ends, it's gone, and its number can later go to something else.

**systemd** is the part of the node that starts services and looks after them. It's the first program to start when a node boots, so it's process 1. Each service has a small settings file called a **unit**, which says what to run and when. That's why service names end in `.service`. Other kinds of unit do other jobs: a name that ends in `.mount` attaches files to a folder, and one that ends in `.timer` starts something on a schedule.

Two settings often get mixed up:

- **Running or stopped** is about now. **Start** and **Stop…** change that.
- **Enabled or disabled** is about the next boot. **Enable** and **Disable…** change that.

A service can be stopped and enabled, or running and disabled. A third state, **masked**, blocks a service, so nothing can start it.

If a service's process stops by surprise, systemd may start it again, depending on the service. So stopping the process often doesn't last. Stop the service instead.

systemd saves what each service prints in the **journal**, the node's log. That's the first place to look when one fails. A **failed** service stopped with an error, or didn't start. It stays marked failed until it starts again or someone clears the mark.

Not every inactive service is a problem. Some start from a timer and finish in moments. SparkBox shows those as **idle**.

## Why it matters in SparkBox

The nodes depend on services. Docker is one. SSH, which SparkBox uses to reach a node, is another. A service that fails can raise an [alert](help:alerts-and-incidents), and it shows in the sidebar as a badge such as `1 failed`. That badge counts every failed unit, not only services. A unit such as `snap-thunderbird-1295.mount` has no row in the **Services** window, so [Alerts](open:alerts) is where to find it. [Failed units that aren't services](help:services#failed-units-that-arent-services) says how to read why it failed and how to clear it.

Stopping the wrong service can cut you off from a node, so SparkBox makes you type a name for those.

In SparkBox you'll meet these ideas in:

- [Services](help:services), every service on every node, with its journal.
- [Workloads](help:workloads), every process, with what it belongs to in **Runs in**.

## Related

- [Restart, disable or enable a service](help:restart-a-service)
- [See and stop processes](help:see-and-stop-processes)
