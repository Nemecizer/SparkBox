# SparkBox

SparkBox is a macOS desktop application for monitoring and administering a small NVIDIA DGX Spark / GB10 cluster. It combines node health, model downloads and serving, Chat, package maintenance, Terminal, RDP checks, network discovery and an offline NVIDIA playbook library.

This is a **prerelease** prepared for public distribution. It targets Apple Silicon Macs. No model weights, node credentials, VPN profiles or maintainer cluster configuration are included.

## Start here

- [Build from source](docs/BUILDING.md)
- [Prepare a signed, notarized release](docs/RELEASING.md)
- [What is tested and what remains](docs/RELEASE-NOTES.md)
- [Security and safeguards](SECURITY.md)
- [Distribution privacy and first launch](docs/DISTRIBUTION-PRIVACY.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)

A new native installation starts in Demo with an empty inventory. Use **Connect my Sparks** to enter your account and nodes. The browser Demo supplies a synthetic four-node example for an offline tour; it connects to no cluster. Help is included in the application. Example addresses and accounts are not configuration for your network.

Admin, Touch ID or Mac-password confirmation, Look only, SSH identity checks, controller ownership and debug write fences are enforced. A successful build or Demo tour is not authorization to change a node. Protected package plans are narrowly pinned historical workflows, not general permission to install arbitrary kernel or driver updates.

## Offline browser tour

```sh
npm ci
PORT=1533 npm run dev:mock
```

Open `http://localhost:1533/mock.html?reset=1&still=1&theme=light&open=dashboard`. Stop the server with Control-C afterward. This is simulated data, not a health check of your devices.

## Source and license

This export starts a new history. It includes application sources, Rust/Python/Go helpers, tests, fixtures, build scripts, lockfiles, Help text, diagrams and upstream notices. Generated dependencies, app state, private development records and prior binaries are excluded. Old raster Help screenshots were omitted because they contained maintainer-specific details. Required window-guide pictures are regenerated from the sanitized browser Demo; Help instructions and diagrams remain available.

The application identifier `com.nemecj.sparkbox` is retained for signing, Keychain and saved-data compatibility. It is a public application identifier, not a credential. Fixture hostnames, user accounts, addresses and captured device-specific MAC bytes are substituted consistently; measurements and validation assertions remain.

Original SparkBox code is available under the MIT license selected by its copyright owner. See [license status](docs/LICENSE-STATUS.md). Third-party components retain their own licenses. No GitHub repository, tag or release has been published by this preparation.
