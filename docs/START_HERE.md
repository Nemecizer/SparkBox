# SparkBox development quick start

Continue on `codex/p3-connection-chain`, following base `f7c9fbf`. Implementation and live-tested app are `c446650`; the handoff records the final verification and candidate. Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md) and [security safeguards](../SECURITY.md). The prior [live Library handoff](LIBRARY-LIVE-HANDOFF.md) is preserved.

## What passed live

Two separate 12-minute connection observations passed. A small public-model download → verify → copy chain then passed on one source and one target using an already installed image. All 53 independent file/plan/cleanup checks passed. Seven matching files remain on each node, the original records are preserved, and reopening showed both completed chain rows plus eight older jobs after Refresh. SparkBox was left Read-only with all four nodes connected.

Connection timing now helps diagnose failures, but the earlier intermittent route and SSH-framing causes are unresolved. Preserve credentials, SSH identity checks, network permissions and filters. A clean bounded observation does not prove those errors are permanently fixed.

## Next bounded work

The next live acceptance should exercise chain cancellation/interruption with a separately reviewed empty destination. Use normal Admin/Touch ID, explicit tool consent, exact immutable files, fresh disk admission, common fabric, leases and ownership checks. Preserve completed models and history. Do not install tools, reset host keys, switch networks or change firewall rules to bypass a refusal. Managed installation, larger transfers and node-reboot recovery remain separate acceptance.

For offline work, remaining P3.4 tasks include HF cache revision grouping, destination free-space columns and reuse of scans within 60 seconds. Choose one coherent scope; do not restart historical workflows or publish incidentally.

## Checks and build

Run the required stages in `scripts/check.sh`: TypeScript, UI/Help, native debug/release safeguards, Python and Go, plus documented Clippy and production-build checks. Native tests use fake nodes and one worker by default. If an execution host terminates long sessions, enumerate the exact compiled test names and run bounded, nonoverlapping batches with the existing isolated runner; verify that every test is accounted for. An interrupted run is not a pass. Browser Demo is available with `PORT=1533 npm run dev:mock` and contacts no nodes.

Build from a clean commit with `scripts/public-release.py build` and Developer ID Application signing. It invokes `scripts/build-candidate.sh` and creates a separate candidate without launching it. Keep the private repository, prior apps, saved settings and published release intact. Do not publish, push or submit for notarization without a separate request. Keep raw live evidence private.
