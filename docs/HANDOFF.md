# Connection diagnosis and small-chain acceptance

Base: `f7c9fbfe78a4b66190f2d2402c67d2987e6bf602`, following the [live Library round](LIBRARY-LIVE-HANDOFF.md). Branch: `codex/p3-connection-chain`. The owner authorized connection diagnosis, a small download → verify → copy test only if stable, documentation, checks, commit and a separate signed candidate. Publication and notarization are outside this round.

## Bounded changes

Failed SSH connection attempts now record elapsed milliseconds, like successful attempts. Normally ending control connections also record their authenticated duration. Failure categories, retry timing, interruption handling, credentials and protocol checks are unchanged. These fields improve diagnosis; they do not establish or repair a network cause.

An offline regression reproduced a separate chain bug: saving the source download plan replaced its chain metadata. The helper now preserves both the exact download plan/hash and the chain's participant plan. Tests cover source/target records, finished/reopened records, disk checks, ownership fields, no credentials in saved status, and no destination writes during parent setup. Existing records are preserved. Older `dc-` source records with missing chain metadata are refused by ordinary Resume at UI, native and node boundaries. The UI explains the missing participant plan and limited single-job cancellation; it never reconstructs or rewrites records.

## Connection evidence and acceptance boundary

Direct route errors occur before SSH login, on fresh sockets. Current actual SSH targets match saved inventory, without ProxyJump or a VPN path. Initial route failures recovered automatically. Separate established sessions later reported rejected SSH packet framing and reconnected. The dependency's installed sources match its locked archive; no cause justifies changing encryption, packet limits, rekey, trust or credentials. Diagnostics do not establish that a network filter caused either error.

A bounded observation compares node TCP reachability with new SSH log events. A live chain must wait for stable connections and pass the existing tool, empty-folder, exact-file, disk and fabric checks. No refusal can be bypassed. Final observations, test totals and candidate provenance will be recorded after completion. Raw evidence remains outside this public tree.

## Safeguards

Admin/Touch ID, Look only, SSH identity, controller leases, instance ownership, exact plans and debug fences remain active. No host-key reset, credential replacement, network permission or filter-rule change is part of this round. Model weights and job records stay intact; transfer cleanup means temporary listeners/processes/secrets, not deleting completed models.
