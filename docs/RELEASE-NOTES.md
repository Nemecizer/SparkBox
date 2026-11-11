# SparkBox v0.1.1 — Preview

This Apple Silicon macOS preview updates v0.1.0 with reviewed downloads, download-to-copy chains, Library reliability improvements and clearer connection diagnostics. Existing user settings and Keychain credentials stay outside the app bundle and are retained when updating. Fresh installations still have no configured nodes, switches or VPN profile.

## Changes since v0.1.0

- **Pinned download review:** select a branch, tag or full revision, inspect the resolved commit and file list, and download exactly that plan. Changed or incomplete metadata refuses instead of silently switching versions.
- **Capacity suggestions:** compare reviewed destination space and explicitly choose the suggested download node. Suggestions do not start work or silently change your selection.
- **Download → verify → copy:** select copy targets during download review. Each destination must be empty and pass space, fabric, tool and ownership checks. Jobs retain their chain details through normal application quit/reopen. Cancel chain retains completed and partial files and reports cleanup problems.
- **Library reliability:** scan failures are isolated by node. Last-known copies and incomplete Hugging Face cache snapshots remain visible with honest status; stale or incomplete observations cannot enable unsafe model actions.
- **Copy provenance:** new ordinary copy source records retain the initiating instance, matching their target records. Existing records are preserved.
- **Diagnostics and Help:** connection failures include elapsed time, and Help explains download review, chains, incomplete copies and cancellation outcomes.
- **Developer tooling:** an offline cancellation replay controller and synthetic tests exercise observation and confirmation guards without controlling an application or connecting to nodes.

## Verification and limits

Bounded maintainer tests passed small pinned download/copy chains, early chain cancellation, following existing jobs after normal app quit/reopen, ordinary active-copy cancellation and continuation of a retained partial copy. Source/target hashes and reviewed cleanup were checked. A later active-chain cancellation attempt completed before an eligible cancellation window; **cancellation during active chain copying remains unproven**.

Managed tool installation, ordinary download Resume, network/VPN loss, Mac sleep, node-reboot recovery and multi-target cancellation still need separate acceptance. A full 24-hour monitoring acceptance remains outstanding. These tests do not certify every Mac, cluster or model. Protected package plans remain narrowly pinned workflows, not a general bypass for kernel or driver protection. See [the current handoff](HANDOFF.md) for test evidence and qualifications, and [the preserved development notes](DEVELOPMENT-NOTES-0.1.1.md) for earlier checkpoints.

## Download and update

Use the signed, notarized app ZIP attached to the **v0.1.1** GitHub release, and compare its SHA-256 with the attached checksum file. The release also provides source archives for its exact tag. Apple Silicon is supported; no Intel/universal binary is provided.

Quit your existing SparkBox yourself before replacing the app in Applications. Replacing the application bundle retains your existing settings; do not delete Application Support or Keychain entries to update. A new user starts with an empty configuration and the normal authentication and identity checks. Demo examples are synthetic and are not saved network settings.

Packaging verifies the Developer ID signature, notarization ticket, Gatekeeper assessment and archive contents without launching SparkBox. This release preparation does not repeat a fresh-account launch test or establish new live cluster acceptance. Existing v0.1.0 assets remain unchanged.
