# SparkBox development quick start

Continue on `codex/p3-active-copy-cancel`, following live-tested base `d8a432b`. Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md) and [security safeguards](../SECURITY.md). The previous [chain cancellation and reconnection handoff](CHAIN-RECOVERY-HANDOFF.md) is preserved.

## What this round established

Two bounded ordinary Library copies of a 28.8 GiB model completed and were preserved. No target cancellation was dispatched. The private observer refused a transition snapshot on the first attempt, then failed to recognize the normal selected-row footer on the second. **Active-copy cancellation remains unproven**; the second attempt did observe copying at 19% before row selection. No third transfer was started.

The live inspection found missing instance metadata on ordinary source-share records. The offline fix adds the initiating instance to new source specs, matching targets; historical records remain untouched. It does not add a new cancellation authorization gate. Help explains target cancellation and the difference between completed targets and source shares canceled during cleanup. All existing safeguards and privacy defaults remain.

See the handoff for final file/cleanup checks, preservation scope, offline test results and evidence limits. The delivery candidate is separate, Developer ID signed, and unopened. Do not publish, push or submit for notarization incidentally.

## Next bounded work

A separately authorized live round should confirm new source metadata and actual cancellation during copying, using a reviewed empty destination and a watcher tested against both Jobs footer formats. Keep this round's completed copies and all history. Do not delete data, install tools, reset host keys or change networks/firewall rules to bypass a refusal.

Active chain cancellation, partial-copy reuse, managed download-tool installation, ordinary Resume, network/VPN loss, Mac sleep and node-reboot recovery remain separate acceptance. The earlier intermittent connection cause remains unresolved.

For offline P3.4 work, choose one coherent scope: HF cache revision grouping is next; destination free-space columns and reuse of scans within 60 seconds follow separately. Do not restart historical workflows.

## Checks and build

The handoff records partitioned native coverage after an interruption, including one retained synthetic shared-memory fixture failure and its successful unchanged isolated retry. Do not describe it as a clean uninterrupted suite or a proven timing cause.

Use the installed Homebrew toolchain and local dependencies. Run relevant stages in `scripts/check.sh` and documented Clippy/production checks; fake-node tests use one worker by default. Browser Demo (`PORT=1533 npm run dev:mock`) contacts no nodes. A failed or interrupted invocation is not a pass.

Build from a clean commit with `scripts/public-release.py build` and Developer ID Application signing. It invokes `scripts/build-candidate.sh`, preserves existing builds and does not launch the output. Verify signed archive provenance and privacy before delivery. Keep raw live evidence private.
