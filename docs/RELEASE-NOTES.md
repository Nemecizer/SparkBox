# SparkBox — development candidate after v0.1.0

## Connection diagnostics and chain recovery records

Failed SSH connection attempts now record elapsed time. Normally ending control connections record their authenticated duration. Error categories, retries and encryption checks are unchanged; diagnostics do not claim to repair the network.

Download → verify → copy sources retain their chain metadata beside the pinned download plan. Source and target records stay associated after completion and reopening. Older chain sources with missing participant metadata cannot fall back to ordinary Resume. The UI explains the missing plan; no existing records or model files are rewritten. See [the handoff](HANDOFF.md) for the bounded live outcome and verification.

## Live Library cache inspection increment

Missing backing files in a Hugging Face cache snapshot now appear as an incomplete copy with a per-node missing-file count. Other models on that node remain visible. Present-byte totals are labeled, incomplete snapshots cannot be Load/copy sources, and permission failures retain their strict behavior. Scanning repairs or deletes nothing. See [the handoff](HANDOFF.md) for validation and limits.

This local development candidate starts from published v0.1.0, commit `3121d96`. The changes below are not published or notarized. Use only the exact reviewed artifact for any separately authorized testing. See [developer handoff](HANDOFF.md) and [quick start](START_HERE.md).

## Library reliability (bounded P3.4 round)

Library now reports each node's scan independently. A failed or offline node no longer discards successful results. Folders from its last successful scan stay visible as **last known**, with the time and any error; nodes without a successful scan show **unknown**. A successful empty scan clears old copies. Observations are memory-only and bound to the model root and node connection configuration; they do not add saved inventory or credentials.

Live transfer markers supply percentages and separate waiting/verification phases. Transferred bytes alone never mark a model finished. Stale observations cannot enable Load, Copy or Delete. Copy and Delete retain fresh strict backend scans; Load retains fresh prerequisite checks. Scan errors and malformed replies fail closed, and delayed refreshes cannot replace newer results.

Offline fixtures cover partial failures, unavailable nodes, recovery, changed configuration, race ordering, progress and action refusal. Bounded live acceptance now covers repeated scans on four nodes, incomplete cache copies, last-known observations during connection loss and automatic recovery. A small two-node chain now also passed waiting/copy progress, verified completion and persistence after an app restart. Large transfers, interrupted-chain recovery and other model actions remain separate acceptance. HF cache revision grouping, destination free-space columns and reuse of scans within 60 seconds remain separate P3.4 follow-ups.

## Reviewed download → verify → copy chains (P3.2)

**Then copy to** adds explicit targets to a new download. The final review shows exact destinations, fabric addresses, disks, source tool and immutable files. Targets are armed first; the source downloads, verifies and shares only the reviewed file set. Copies verify fresh source hashes and retain individual records. No model is loaded.

Every destination must be empty. Installed rsync, sufficient disk and a common IPv4 fabric subnet are required. Paths, route, tool and immutable files are rechecked at start. Jobs persist on the nodes, continue after application disconnection and expire within 24 hours. Reopening follows records without dispatching replacement work. Node reboot recovery is explicit; chain rows cannot use ordinary Resume. **Cancel chain…** verifies ownership and the saved participant plan before each cancellation, retaining files and records.

Offline detached-worker, real loopback rsync and failure tests cover this round. A bounded live ARM64 test passed: seven files totaling 6,477,074 bytes downloaded to one node using an already installed image, then copied over the fabric to one empty target. All 53 independent file/plan/cleanup checks passed, and the two completed chain records plus eight older records remained after an app restart and Refresh. This does not establish managed installation, cancellation, node-reboot recovery or large-transfer acceptance. No credentials, saved network state or native defaults are added.

## Capacity-based download-node suggestions (P3.2)

In Models → Hugging Face, expand **Suggest a download node**, choose the potential load nodes to compare, and read their destination capacity. SparkBox suggests the qualifying node with the most space after the full inspected transfer plus 5% and 20 GiB; stable node ID breaks ties. **Use** explicitly applies the suggestion. The existing selected target never changes because of a probe or failed connection.

Readings use the pinned inspected manifest, expire after one minute and are discarded on revision, scope or connection changes. Individual disk failures remain visible. Finished and running copies are excluded from the UI comparison. The comparison requires no Admin unlock and starts no jobs. Download still requires tool consent, its own disk review and the existing safeguards. The node-side start repeats admission. Read-only capacity comparison passed on two live nodes for one small public model; transfer-start disk admission and one small two-node chain also passed in this round.

## Inspected downloads (P3.8)

New downloads and destination-disk reviews require the repository, full commit and complete inspected file list. SparkBox re-fetches that immutable commit and compares names, sizes and available publisher checksums before starting; any mismatch refuses instead of switching revisions. A moved branch does not change an existing review.

The Hugging Face tab adds branch/tag selection, an explicit revision field, the full resolved commit and an expandable file list. Failed inspections discard old details; late responses cannot replace a newer selection. Configuration access, missing-file and parsing errors are visible, and prevent download/AI advice and misleading fit estimates. An unreadable or inconsistent shard index refuses without a fallback manifest.

Admin, Touch ID, Look only, SSH identity, leases, ownership, exact tool consent, disk checks, saved-plan Resume and debug fences are retained. No credentials, saved connections or node defaults are added. All new behavior is tested offline. Live immutable Hub inspection, read-only node capacity review and the bounded ARM64 download/copy described above passed. Larger or interrupted downloads still need their own live acceptance.

## Included

- macOS desktop monitoring, alerts, node history, topology and local /24 discovery.
- Model library, explicit download-tool and target selection, saved-plan resume, destination-disk checks, copy review, serving, Chat and deployment history.
- Admin-gated routine updates, narrowly pinned protected-update checkpoints, explicit restarts and reconnection checks.
- Terminal, files, RDP inspection/repair, VPN helper, and bundled NVIDIA How To documentation.
- Offline Demo, unit/fake-node tests, build scripts, lockfiles and dependency notices.

## Public-source preparation changes

New installations have an empty inventory and no guessed SSH account. A missing or corrupt inventory cannot restore sample endpoints. Existing saved inventories retain their normal load/preservation behavior. The browser Demo uses a synthetic four-node example; native Demo uses the nodes you explicitly configure.

Owner-specific account names, LAN addresses, hostnames, captured device-specific MAC bytes, physical serial numbers, IPv6 interface identifiers, SSH host-key fingerprints and captured boot identifiers are substituted in examples and fixtures. The public application identifier is retained for compatibility. Private Git history, handoffs, maintenance records, saved state, credential profiles, native test orchestration and old builds are excluded. Public Help uses general onboarding instead of the maintainer's acceptance chronology. Fake SSH servers generate fresh in-memory keys per test process; no private-key fixtures are distributed.

The old raster Help pictures are omitted. A packaging guard and an isolated native startup regression check protect the empty-install contract. Required guide pictures are generated from the sanitized browser Demo in Light and Dark. Public Help does not claim native acceptance of this export. Original assertions for authentication, identity, ownership, plan integrity, cancellation and cleanup remain; fixture encodings/checksums are updated when their synthetic content changes.

## Limits

This is not general hardware certification or approval for unattended changes. The maintainer previously tested specific GB10 configurations; those results do not cover this public export on another person's Mac or cluster. A full 24-hour monitoring acceptance and several Linux/systemd, serving, recovery and download paths remain outstanding. Platform-specific test skips are reported with the preparation evidence.

Disk-space checks are not reservations. Parallel jobs, quotas, inode limits and separate tool/cache filesystems can still cause a later failure. Inspected download revision/file-list pinning is implemented offline; live acceptance remains pending.

Protected update plans match exact historical package versions and prerequisites. They are not a general way to bypass routine kernel/driver protection. Discovery proposes an inventory for review; it does not silently enroll machines or establish cabling from hostnames.

Apple Silicon is the current build target. An Intel/universal binary is not provided. Public installation from a fresh downloaded, quarantined, notarized artifact is a separately authorized acceptance step. The original public-source preparation did not launch SparkBox or operate a cluster. Subsequent bounded live development acceptance is recorded above and in the handoff.

## Before publication

Install a valid Developer ID Application identity, configure a Keychain notarization profile, build from the exact reviewed source commit, submit explicitly, staple and verify. Then perform the owner installation review. Publish only after that review and an explicit publishing request. See RELEASING.md.
