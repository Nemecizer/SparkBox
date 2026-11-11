# SparkBox development quick start

Continue the connection-diagnosis and small-chain round from `f7c9fbf` on `codex/p3-connection-chain`. Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md) and [security safeguards](../SECURITY.md). The prior [live Library handoff](LIBRARY-LIVE-HANDOFF.md) remains available.

## Connection review

Start Read-only. Compare SparkBox's actual SSH target and path with any independent reachability check. A route error happens before login; a rejected SSH packet belongs to an established or negotiating secure connection. Neither message alone establishes a credential or filter problem. Preserve host keys, credentials, network permissions and encryption checks.

The SSH log records failed-attempt `ms` and normally ending control connections' `connected_ms`. These are attempt/authenticated-connection durations, not node uptime. Use timestamps and reasons together. Help: [Connection and login messages](help/reference/errors-connection.md) and [Collect diagnostics](help/ops/collect-diagnostics.md).

## Bounded live chain

Only proceed with stable connections. In Models → Hugging Face, inspect one small public model at a full fixed commit and review Files to download. Select one Download node and one Then copy to target. Both destinations must be empty; installed rsync, sufficient destination space and a common fabric subnet are required.

Use the normal Admin/Touch ID and tool-consent dialogs. Review exact destinations, disk results, files, tool and temporary source port before Start download and copies. Do not install tools, change firewall rules or switch networks to bypass a refusal. Record every job ID and chain ID.

Verify both jobs finish, the source waits for target verification, and the same records remain after reopening. Compare exact files, sizes and hashes, then check that temporary listeners, processes and per-job secrets are gone. Keep finished models and job records. Recovery/cancellation needs the original ownership and participant checks.

## Offline checks and build

Run `scripts/check.sh` from this checkout. It covers TypeScript, UI/Help, native debug/release safeguards, Python helpers and Go. New regressions cover retained source chain metadata and connection timing without changing failure categories or logging passwords. Browser Demo remains available with `PORT=1533 npm run dev:mock`; it contacts no nodes.

Build from a clean commit with `scripts/public-release.py build` and a Developer ID Application identity. This invokes `scripts/build-candidate.sh` and creates a separate candidate. Keep existing builds, the private repository, published release and saved records intact. Do not publish, push or submit for notarization incidentally. Final live outcomes and candidate provenance belong in the handoff.
