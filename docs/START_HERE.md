# SparkBox development quick start

Continue on `codex/p3-chain-recovery`, following reviewed base `5a0062e`. Runtime source is unchanged; this round records bounded live acceptance and updates Help. Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md) and [security safeguards](../SECURITY.md). The previous [connection and small-chain handoff](CONNECTION-CHAIN-HANDOFF.md) is preserved.

## What passed live

An initial eight-file chain finished before cancellation and was preserved. A separate chain passed early cancellation during source tool checking and target waiting. A third chain continued through normal SparkBox quit/reopen, retained the same job IDs and completed with matching hashes. Independent inspections passed 86, 94 and 120 checks respectively. Jobs showed all 16 old and new records; Library showed the completed copies verified and the canceled source incomplete. The app was left Read-only with all four nodes online.

The original completed copies, canceled marker, prior records, 60 selected saved configuration/history files and seven prior archives were preserved. Normal Admin/Touch ID, exact plans, explicit installed-image consent, disk admission, ownership, leases and debug fences remain intact. No package update, node restart, tool installation, model load, network change or credential change was performed.

## Limits and next bounded work

Early cancellation does not test an active file server. Normal quit/reopen does not test network loss, VPN interruption, Mac sleep or node reboot. No pre-quit node PID checkpoint was captured. The handoff explains the narrow waiting-target process-group proof and all other evidence limits. The earlier intermittent connection cause remains unresolved.

The next live acceptance should exercise cancellation during active sharing/copying with a separately reviewed empty destination. Preserve all finished and canceled folders and records. Do not install tools, reset host keys, switch networks or change firewall rules to bypass a refusal. Managed installation, ordinary Resume, larger transfers and node-reboot recovery remain separate acceptance.

For offline work, remaining P3.4 tasks include HF cache revision grouping, destination free-space columns and reuse of scans within 60 seconds. Choose one coherent scope; do not restart historical workflows or publish incidentally.

## Checks and build

This documentation-only round passed TypeScript, 3,758 UI tests, 2,145 final Help checks, 10 download-chain helper tests and 13 release/audit-tool tests. The previous full native/Python/Go/release-safeguard results apply to unchanged runtime and are not claimed as rerun. Use the installed Homebrew toolchain and local dependencies; do not count an interrupted or failed invocation as a pass.

For runtime changes, run the relevant required stages in `scripts/check.sh` and documented Clippy/production checks. Native fixtures use fake nodes and one worker by default. Browser Demo (`PORT=1533 npm run dev:mock`) contacts no nodes.

Build from a clean commit with `scripts/public-release.py build` and Developer ID Application signing. It invokes `scripts/build-candidate.sh` and creates a separate candidate without launching it. Keep private source, prior apps, saved settings and published release intact. Do not publish, push or submit for notarization without a separate request. Keep raw live evidence private.
