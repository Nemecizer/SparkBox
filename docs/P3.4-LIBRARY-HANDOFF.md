# P3.4 Library reliability handoff

Base: completed P3.2 download → verify → copy chain `5f732f84a46814ff65e54a745b16425b25e64ca6`, following capacity suggestions `e11350c`, P3.8 `a4ebb2a` and published v0.1.0 `3121d96`. Branch: `codex/p3.4-library-reliability`. One bounded offline round; no native launch, cluster work, notarization or publication. Previous round: [P3.2 chain handoff](P3.2-CHAIN-HANDOFF.md).

## Implemented

- Library uses a new read-only `library_report` command. Each inventory node reports success, offline status or a scan error independently. One failure cannot discard the successful nodes. No probe is dispatched to nodes reported offline.
- A successful per-node scan is kept in memory. Failed/offline scans retain its copies as **last known**, with the last successful time; a never-scanned node shows **unknown**, not a no-copy dash. A valid empty scan clears old copies. Nothing is persisted or bundled for this cache.
- Observations are bound to the node's full configuration, its Live connection generation and the model-root setting. Removed/replaced nodes, a changed root and Demo/Live changes discard affected observations. Older concurrent requests cannot restore a discarded cache. The UI rejects late refreshes and invalidates freshness during reconnection until a new scan succeeds.
- The node helper reports unreadable roots/subtrees/files as errors, not successful empty or undersized scans. The backend rejects malformed, duplicate or inconsistent scan rows. A missing optional root remains a valid empty search location; a root that is a file or a broken symbolic link fails. Relative search roots are normalized to absolute paths; an empty root fails instead of scanning the home directory.
- Live partial markers supply download/copy percentages and explicit waiting/verification phases. Progress is accepted only from a live marker-bound job. Even all transferred bytes do not make a copy finished. While Library is visible and Jobs knows a transfer is running, scans refresh about every ten seconds after the previous scan finishes. Stale progress is never displayed as current.
- Last-known copies remain visible in Library/details and are excluded by the shared finished-copy helper used by Load and Hub. Unknown Hub copies require another Library scan. Only current connected targets can be used in Library Copy/Delete. A selected target that goes offline stays selected until explicitly removed or rescanned; it blocks dispatch. Changed observations or revision metadata during confirmation require another review, even if a reconnect or later scan restores identical values. Long error messages scroll without hiding the model table.
- The existing strict `library()` method remains uncached and is used by Copy, disk preflight and Delete. Delete now also checks distinct connected inventory targets and fresh presence on all selected nodes before removing anything. Load keeps its existing fresh prerequisite checks. Read-only observations cannot authorize mutations.
- Native/browser Demo display unfinished jobs without making them finished action sources. Browser fixtures `library=scan-failure` and `library=progress` cover mixed failures, unknown nodes, percentages and verification. Help and tips describe the behavior, limits and recovery steps.

## Safeguards and privacy

Admin/Touch ID, Look only, verified SSH identity, controller leases, instance ownership, exact download-tool/install consent, immutable file review, disk admission, protected updates and debug fences remain in place. `library_report` is explicitly read-only; no new mutation command or automatic repair is added. Fresh native installs remain empty and locked. No credentials, saved settings, network profiles or existing records are included, reset or removed. Private audit input and raw evidence stay outside public source.

## Verification

- Final TypeScript and UI checks passed: 3,756 tests across 129 files, including all 1,942 release Help checks. The final scroll-only style change also passed all 258 color/contrast checks and Light/Dark browser inspection.
- The initial full native run completed 1,883 tests successfully, with 36 existing ignored hardware/fixture cases and one failure: an older copy-disk fixture omitted the root field now required from a valid scan. The real helper includes that field. The fixture now supplies it; no disk-admission or no-job-start assertion was removed or relaxed.
- All 14 affected native Library/disk tests passed on the final rerun, including the corrected fixture, read-only scans, cache isolation, empty-scan recovery and strict action refusal. Binary and documentation targets also passed (zero tests). All 30 release-compiled safeguard tests passed again against the final helper.
- The full Python helper run passed 529 tests with 3 platform-specific skips (532 total). The final relative/empty-root correction then passed all 50 tests in every helper class exercising scan, partial folders and orphan writers; this includes the additional relative/empty-root regression. Go and all 13 distribution/release-tool tests passed.
- The full `scripts/check.sh` invocation reached every stage. Its sole failure was the incomplete native fixture described above; the corrected affected tests and final UI/helper checks were rerun successfully rather than reporting that first invocation as clean.
- A confirmation round-trip regression was reproduced before its fix: stale data, or a changed revision, returning to the original values could revive a pending confirmation. Monotonic change tracking now expires that confirmation, and both regression cases pass. No cached observation can authorize a backend mutation.
- Light/Dark browser Demo verified per-node failures, last-known copies, unknown cells, percentages, verification and disabled Load for unfinished models. A narrower-window check confirmed long errors scroll while the table remains usable. The disposable browser and local server were closed.
- Final library Clippy passed with the same 46 existing warnings. No warning was added. Source privacy audit: 2,476 tracked files and zero findings against the external private-identifier deny list. The changed source also passed an offline TruffleHog scan with credential verification and update checks disabled.
- Published app/source ZIPs, the three previous signed candidate ZIPs, private repository and baseline source checkouts remain at their recorded hashes and clean heads. Raw evidence stays outside public source.

## Candidate

After a clean commit, `scripts/public-release.py build` invokes `scripts/build-candidate.sh` and creates `../SparkBox-builds/2026-11-10-p3.4-library-reliability/` relative to this checkout. The app is Developer ID signed. Provenance records the exact commit and ZIP checksum. This round does not launch, notarize or publish it.

## Owner inspection and limits

Use the synthetic browser-only steps in [START_HERE.md](START_HERE.md). Last-known observations last only for this app process; there is no recovery-copy database or automatic file repair. Scan times are observations, not a guarantee that files cannot change afterward. A report waits for all online scans, each with the existing 120-second timeout. Action scans remain deliberately strict: a failed online node can still block Copy/Delete even though the Library continues to display other nodes. Load still performs its own prerequisites.

This completes the bounded reliability portion of P3.4, not every P3.4 roadmap detail. HF cache snapshot grouping/revision presentation, destination free-space columns and reuse of scans within 60 seconds remain unfinished. Actual HF blob reclamation remains separate P3.6 work. Real node scan, interruption and reconnection acceptance is pending; offline fixtures do not certify Linux/systemd, ARM64, Hub or fabric behavior.

Recommended next offline round: bounded P3.4 Hugging Face cache snapshot/revision presentation, preserving cache links and blobs and keeping all mutations behind fresh checks. When cluster access returns and live testing is separately authorized, verify one read-only Library scan, a temporary unavailable node, retention and reconnection, then a fresh scan before separately approved model actions. Stop after this candidate; no incidental release update.
