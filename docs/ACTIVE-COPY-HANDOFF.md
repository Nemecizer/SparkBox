# Bounded active-copy attempt and source record fix

Base and live-tested candidate: `d8a432b43887e49dc90325bdb91e43a3601b2ab0`. Branch: `codex/p3-active-copy-cancel`. The previous [chain cancellation and reconnection handoff](CHAIN-RECOVERY-HANDOFF.md) is preserved. The owner authorized a bounded live cancellation test during active copying, preservation of copies/history/safeguards, cleanup verification, offline fixes, documentation, commit and a separate signed candidate without launching or publishing it.

## Outcome and change

**Active-copy cancellation remains unproven.** Two ordinary Library copies completed before cancellation could be confirmed. No target cancellation was dispatched. Both finished copies and all records were kept. These attempts do not test the download → verify → copy chain's cancellation path.

Inspection found one product defect: an ordinary copy's source share spec omitted the initiating SparkBox instance, although its target spec included it. New source records now retain that same instance. A fake-node regression checks the actual source and target start payloads and normal source cleanup. Existing records are not rewritten. This is a durable provenance fix, not a new cancellation authorization gate.

Help explains how to cancel one target, preserve partial files, and distinguish a target's outcome from a source share marked canceled during ordinary cleanup. Verification flags remain unchanged. The final candidate includes the offline fix and Help; it is a separate clean-commit build and remains unopened. The live test used the earlier d8a432b candidate.

## Live observations

All four nodes were online. Normal owner Admin authentication preceded operations. Each ordinary Library plan copied the same existing verified 75-file, 30,889,968,423-byte model from one source to a separately checked empty destination. No download, tool installation, package update, node restart, inference, network/VPN/firewall change or credential change occurred. Native preflight checked destination space, installed rsync, fabric and normal admission. The final review required 50.2 GiB, including the full transfer plus 5% and 20 GiB. Only the intended target was selected each time.

1. The first destination completed before cancellation. The private observer initially saw a non-atomic transition: the footer counted the two new jobs while the table still exposed only older rows. It refused. A later observation already showed the copy finishing. Both records and the completed target were preserved.
2. The second destination was observed **copying at 19%**, while its source was **serving**. The observer selected the exact new target row. It then failed to parse the normal footer changing from “20 jobs” to “20 jobs · 1 selected”. This was a private test-harness defect, not missing application history. It refused before pressing Cancel or confirming cancellation. The target completed, and the source stopped normally. No third transfer was attempted.

The target records' owner matched the reviewed existing owner. Both old-candidate source records had an empty instance. The first strict inspection retained that failure; a separate exact-fingerprint metadata probe confirmed the persisted instance value was empty. Later checks report functional integrity and cleanup separately from this still-failed source metadata check. No failed ownership check was converted into a pass, and no application safeguard was relaxed.

## Verification and preservation

The first inspection passed all 124 target checks and 41 of 42 source checks; only the source record identity check failed. The exact-fingerprint follow-up isolated the empty instance. The second inspection passed all 43 source functional checks and all 111 target checks, while separately retaining **sourceInstanceMatchesOwner = false**. Both targets contain the same 75 payload hashes and byte counts as the unchanged source, plus their own verified manifest. These are completed-copy results, not cancellation passes.

Recorded workers and child PIDs were absent; no live members remained in their process groups, and the derived source listener ports and per-job secret/server folders were clear. No missing child identity was substituted with a weaker cleanup proof. The first inspection preserved nine earlier source records and five earlier target records; the second preserved ten earlier source records, including the first new share, and the one existing record on its target. Four previously scoped model folders on the source and first target, and the second target's existing small-model copy, retained their recorded payload/marker bytes and modes. Unknown model payloads and hidden data outside the declared scope were not broadly read. The first destination was fully hashed after its own transfer; the second transfer targeted only the other destination.

The final UI showed all **20 jobs** and three verified copies of the tested model. SparkBox was left **Read-only**, with all four nodes online. All **60 selected saved configuration/history files** and **eight prior archives** retained their bytes and modes. Normal activity/task/monitoring records were retained; these changing logs and UI layout are outside the immutable-file comparison.

Raw paths, process identities, job IDs and evidence remain private. No secrets, Keychain contents, process arguments or environments are exported. Inspections used normal Admin-gated Fleet commands with **as root** off; they read reviewed job metadata, declared model payloads and bounded process/listener facts, and sent no signals.

## Safeguards and limits

Admin/Touch ID, Look only, pinned SSH/node identity, controller leases and the debug write fence remain intact. Existing container ownership and boot/PID/start-time checks are unchanged. Ordinary job cancellation does not independently compare the record's instance; the metadata fix does not claim otherwise. Source watcher recovery still uses this installation's saved share list.

Completed-copy checks prove matching source/target payloads and reviewed cleanup. They do not prove cancellation under load, retained partial reuse, lost-network/VPN recovery, Mac sleep, node reboot, multi-target cancellation or general throughput. The earlier intermittent connection cause remains unresolved. A separate disarmed private watcher now accepts the exact selected-row suffix while retaining strict row counts and action guards; all 42 offline tests pass, including actual UI fixtures. It has not run live and retains the old target only for replay. Any future test needs a separately reviewed empty target. These fixtures improve test orchestration, not product acceptance.

## Offline verification

- The new fake-node regression first failed specifically because the source start spec had no instance. It passed after the one-line fix, checking source/target payloads and normal one-time source cleanup.
- TypeScript and **3,758 UI tests across 129 files** passed before the Help edits; no UI runtime code changed. After the final Help edits, **2,145 Help tests across 10 files** passed with release validation enabled. Production web assets built successfully. Existing large-chunk and ineffective dynamic-import warnings remain.
- All **22 targeted ordinary-copy/cancellation Python tests** and **13 release/audit-tool tests** passed. Full Python and Go suites were not rerun because their source did not change; the unchanged Go helper built successfully. Earlier full results remain in the preserved predecessor handoffs.
- Native validation was interrupted by the owner after **1,376 explicit passes and 36 explicit skips**, without a final summary. The temporary Cargo target subsequently disappeared. A fresh offline harness rebuilt the unchanged source in a separate persistent cache. Exact enumeration proved that those 1,412 terminal entries and the **511 remaining tests** were a disjoint complete set of all **1,923 tests**. Only the uncovered partition was resumed; the incomplete test ran again.
- The resumed partition finished with **510 passes and one failure**. An existing single-node serving/restart/unload fixture retained one synthetic shared-memory entry and failed its strict empty-memory assertion. Its unload report correctly recorded a cleanup warning. The same unchanged fixture then passed in isolation after compilation finished, including all pre-teardown cleanup assertions and a clean report with zero new shared-memory entries. The original failure remains recorded, and its cause is unproven. Thus every non-skipped test has a passing result across the recorded runs, but this is **not a clean, uninterrupted full-suite pass**. No timeout, assertion or product health check was relaxed.
- All **31 release-compiled checks** passed: the 30 existing admission/exit safeguard cases and the new ordinary-copy source/target instance regression.
- The 36 skips are explicit opt-in staging, external notification, real Mac Trash, fixture-generation and history-measurement/compatibility cases. Exact names and coverage hashes are retained in private evidence. Initial missing-helper, loopback sandbox and draft-fixture capture errors were corrected and are not counted as passes.
- Native binary and documentation targets completed successfully with zero tests. **Clippy passed with 46 warnings**, unchanged from the recorded baseline.
- The source publication guard passed **2,480 tracked files with zero findings**. App/archive privacy and signing verification follow the final clean commit. The private selected-footer watcher passed all **42 offline tests**; it remains disarmed and has not been used live.

## Delivery and next round

Use `scripts/public-release.py build` with Developer ID Application signing after the final clean commit. Verify provenance, source/resource equality, app and extracted archive signatures, private identifiers and offline secret scans. Signing is not notarization. Preserve previous apps, the published v0.1.0, private source and Claude-backup. Do not launch or publish this final candidate.

Owner inspection after separately authorizing the candidate launch:

1. Open **Models → Jobs**, wait for all nodes and click **Refresh**. Confirm the 20 retained jobs, including two completed target copies and their stopped source shares. A canceled source share does not mean its target was canceled.
2. Open **Library** and click **Find models**. Confirm the test model is verified on the source and both new destinations. Keep these completed folders and records.
3. Review the updated copy Help. Do not delete a completed destination to create another test slot.

The next live round should use a separately reviewed empty destination and a watcher that recognizes both unselected and selected Jobs footers, then verify actual cancellation during copying and retained partial cleanup. It should also verify new source instance metadata with the corrected candidate. Preserve the completed copies from this round. For offline work, the next bounded P3.4 option remains HF cache revision grouping; destination free-space columns and recent-scan reuse are separate work. Stop after delivery of this round.
