---
id: containers-and-images
title: Containers and images
type: concept
area: ops
summary: A container is a program in its own sealed box. An image is the recipe it starts from. Why models run this way.
keywords: [docker, container, image, what is]
verified: null
related: [containers, images, engines]
---

# Containers and images

## In short

A **container** is a program running in its own sealed box. An **image** is the recipe a container starts from. One image can start many containers, and removing a container leaves its image behind.

## How it works

An image holds the program, the libraries it needs and its settings, all at known versions, so it starts the same way on any node.

**Docker** is the program on each node that does this. It downloads images, starts containers from them, and keeps them apart from each other and from the node.

A running container sees only its own files, unless it's given more. Three things connect it to the node:

- A **mount** shows a folder from the node inside the container. Your model files reach an engine this way.
- A **port** is a numbered door on a computer that a program answers on. Docker can connect one of the node's ports to a program inside the container.
- A **volume** is storage that outlives the container.

Anything else a container writes inside itself is gone when you remove it. Stopping keeps it. Removing doesn't.

A container has a **restart policy**, which says whether Docker starts it again by itself. With `no`, it stays stopped until someone starts it.

Images take disk space. An image is built in **layers**, and images that share a layer store it once. An image can also be left behind with no name after a newer one replaces it. That's a **dangling** image.

A tag is an image's version name. In `nvcr.io/nvidia/vllm:26.05-py3`, the part after the colon is the tag.

## Why it matters in SparkBox

A model you load with vLLM or SGLang runs in a container, started from the engine's image. The model's weights, its files, stay in a folder on the node, mounted into the container. So you can remove the container and keep the model, then load it again later.

Docker's power has a cost: a container can be given full access to a node. SparkBox treats such a command like one run as root.

In SparkBox you'll meet these ideas in:

- [Containers](help:containers), every container on every node.
- [Images](help:images), every image and what uses it.
- [Inspect](help:inspect-window), one container's mounts, ports and settings.
- [Models](help:models-window), which starts and stops the containers for you.

## Related

- [Manage containers](help:manage-containers)
- [Check Docker images](help:check-docker-images)
- [Engines: vLLM, SGLang and Ollama](help:engines)
