# P3.2 download → verify → copy chain handoff

Base: completed capacity suggestions `e11350c35954192161c8c49c1830138d7974e7d1`, following P3.8 `a4ebb2a` and published v0.1.0 `3121d96`. Branch: `codex/p3.2-download-copy-chain`. One offline coding round; no native launch, cluster work, notarization or publication. Previous round: [capacity handoff](P3.2-CAPACITY-HANDOFF.md).

## Implemented

- Hugging Face adds **Then copy to** with explicit connected targets. Tool choice is followed by a read-only chain review and final **Start download and copies** consent (or separate tool-install consent). Source/revision changes clear copy choices. Existing copies are excluded. A disconnected selected target stays selected until the user explicitly removes it or reconnects; no silent fallback is made.
- The backend re-fetches the immutable inspected manifest, validates distinct inventory IDs and checks every destination. A new chain requires empty, non-symlink folders, rsync already installed, full file sizes plus 5% and 20 GiB free on each filesystem, and distinct IPv4 fabric addresses on one subnet. No LAN fallback, firewall edit or rsync install is performed.
- Start repeats manifest/tool/route/path/disk checks. Changed reviewed paths or fabric addresses refuse. The node repeats empty-folder and disk admission under its writer lock. Files that appeared after review are preserved and refused.
- Target pull jobs are armed first; the source starts last. Each node stores the same non-secret chain ID, immutable revision, participant IDs, destinations and deadline. Tokens/passwords go only through existing protected stdin and temporary secret paths, never the saved chain metadata. The Hub token is removed before source sharing.
- The source download verifies publisher sizes and available publisher hashes before opening its temporary server. Its fresh SHA-256 copy manifest must match the exact chain/repository/revision/file list/sizes and available publisher hashes. Targets validate the same binding and exact copied file set before marking a copy verified.
- All phases run within existing detached node jobs. Mac reopening follows saved jobs and discovers node records; it starts no replacement jobs. Each chain worker expires within 24 hours from setup. The source also closes after acknowledgments or 30 minutes without activity. Failed copy acknowledgments or incomplete expiry fail the chain source instead of reporting success. Existing standalone copy behavior is retained.
- **Cancel chain…** re-reads each member, checks this instance and the exact common plan, then uses the existing guarded cancel path. Terminal records and their original failure messages remain unchanged. Reported orphan processes require explicit recovery review rather than a false cleanup success. Missing/disconnected/changed-ownership jobs are not replaced; per-node errors are reported. Setup failure attempts the same guarded cancellation for all planned members, including an uncertain source start. Reconnect and review all member jobs after interrupted setup.
- Jobs shows chain identity, source/targets, deadline, fixed commit and explicit recovery guidance. Ordinary Resume excludes chain rows so it cannot silently omit their approved copies. Reboots do not auto-resume. Cancel remaining members, recover an incomplete source with a separately reviewed download, then use Library copy for unfinished targets.
- Native and browser Demo simulate the phases, preserve verified copies and report failures. Browser fixtures include `chain=source-fail` and `chain=copy-fail`. Help/tips explain consent, verification, expiry, cancellation and recovery.

## Safeguards and privacy

Admin/Touch ID, Look only, verified SSH identity, controller leases, instance ownership, exact tool/install consent, disk admission and debug fences remain in place. Only `model_chain_review` and the exact `chain-preflight` helper operation are read-only. Start/cancel stay guarded. Fresh native installs remain empty and locked. No saved settings, credentials, private endpoints or records are included or removed. Evidence and the private identifier deny list remain outside public source.

## Verification

- Final TypeScript and UI verification passed: 3,744 tests across 128 files, including release Help, explicit consent, failure/cancellation, verified Demo copies and removing a disconnected target without a silent plan change.
- Ten new Python tests cover detached fixture downloads and real loopback rsync, exact manifest binding, verification failure, changed destinations, expiry, secret cleanup and clearing the source’s stale download rate while sharing.
- Fake-node tests cover read-only review, source-last dispatch, stale routes/manifests, missing rsync, invalid node choices, lost start acknowledgment and guarded rollback, foreign/changed plans, malformed records, Admin/Look only and debug-fence refusal. A separate regression blocks download-only Resume for chain records.
- Light/Dark browser Demo confirmed final review, one source/two target jobs, completion and recovery controls. The long review scrolls to visible Start/Cancel controls. Temporary browser profiles and server were closed.
- Library Clippy completed with 46 existing warnings, unchanged. An additional `--all-targets` experiment found the pre-existing `clippy::unused_io_amount` error at `src-tauri/src/discovery/tests.rs:401`; that file is unchanged from the base. No new chain-module diagnostics appeared. The documented `cargo clippy --lib` check passed.
- Source privacy audit: 2,471 tracked files, zero findings against the external private-identifier deny list. Published app/source ZIPs, both prior candidate ZIPs, private repository and baseline checkouts match their recorded hashes/heads.
- Full native suite: 1,878 passed, zero failures, 36 explicitly ignored hardware/fixture cases. The full invocation passed without retries. After final record-preservation and display corrections, all seven affected chain/recovery tests and all 30 release-compiled safeguards passed again.
- Python helpers: 525 passed, 3 platform-specific skips (528 total), including the final chain/helper edits. Go and all 13 distribution/release-tool tests passed. The complete `scripts/check.sh` invocation passed; final UI, Help, affected native and Clippy checks also passed as recorded above.

## Candidate

After a clean commit, `scripts/public-release.py build` invokes `scripts/build-candidate.sh` and creates `../SparkBox-builds/2026-11-10-p3.2-download-copy-chain/` relative to this checkout. The app is Developer ID signed. Provenance records the exact commit and ZIP checksum. This round does not launch, notarize or publish it.

## Owner inspection and limits

Use the synthetic browser-only procedure in [START_HERE.md](START_HERE.md). Real ARM64/Hub/fabric acceptance remains pending. A fresh chain requires empty destinations and a shared IPv4 fabric subnet; it cannot adopt partial/existing copies or automatically recover a node reboot. Disk probes reserve no space. Source verification uses publisher hashes only when available; target verification uses fresh source hashes. Targets whose source failed before sharing wait until canceled or expired. A disconnected target cannot be canceled immediately; reconnect to cancel it or wait for its deadline. If setup loses contact before every job is acknowledged, review all selected nodes before starting anything else.

Recommended next round: when cluster access is available and explicitly authorized, use one small public model on one source and one empty target. Review exact files/destinations, start the chain, verify source-before-copy ordering, close/reopen SparkBox after setup, confirm both records and hashes, and verify listener/secret cleanup. Follow with a separately approved canceled-chain case. Do not infer live acceptance from offline fixtures or update the published release incidentally. Stop after this candidate.
