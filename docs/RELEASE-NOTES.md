# SparkBox — development candidate after v0.1.0

This local development candidate starts from published v0.1.0, commit `3121d96`. The changes below are not published or notarized. Use only the exact reviewed artifact for any separately authorized testing. See [developer handoff](HANDOFF.md) and [quick start](START_HERE.md).

## Inspected downloads (P3.8)

New downloads and destination-disk reviews require the repository, full commit and complete inspected file list. SparkBox re-fetches that immutable commit and compares names, sizes and available publisher checksums before starting; any mismatch refuses instead of switching revisions. A moved branch does not change an existing review.

The Hugging Face tab adds branch/tag selection, an explicit revision field, the full resolved commit and an expandable file list. Failed inspections discard old details; late responses cannot replace a newer selection. Configuration access, missing-file and parsing errors are visible, and prevent download/AI advice and misleading fit estimates. An unreadable or inconsistent shard index refuses without a fallback manifest.

Admin, Touch ID, Look only, SSH identity, leases, ownership, exact tool consent, disk checks, saved-plan Resume and debug fences are retained. No credentials, saved connections or node defaults are added. All new behavior is tested offline; real Hub and ARM64-node acceptance remain separate.

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

Apple Silicon is the current build target. An Intel/universal binary is not provided. Public installation from a fresh downloaded, quarantined, notarized artifact is a separately authorized acceptance step. The preparation does not launch SparkBox or operate a cluster.

## Before publication

Install a valid Developer ID Application identity, configure a Keychain notarization profile, build from the exact reviewed source commit, submit explicitly, staple and verify. Then perform the owner installation review. Publish only after that review and an explicit publishing request. See RELEASING.md.
