# Cancellation acceptance and offline replay checkpoint

This checkpoint follows application commit `92dfacf` on `codex/p3-active-copy-cancel`. It commits the portable offline replay controller, synthetic regression fixtures and the results of subsequent bounded acceptance rounds. Application runtime and safeguards are unchanged. The [preceding source-identity fix and original test report](ACTIVE-COPY-HANDOFF.md) and [earlier chain recovery report](CHAIN-RECOVERY-HANDOFF.md) are preserved as historical records.

## Established results

Subsequent authorized testing with the signed `92dfacf` candidate established ordinary Library cancellation during active copying and continuation of the retained partial folder. Cancellation records showed positive incomplete copy progress, nonempty matching source/target instance metadata and reviewed process/listener/temporary-secret cleanup. The later continuation matched every one of the 75 payload hashes; the 39 already complete retained files kept their inode, modification time and size. All 200 final checks in that continuation passed. Exact network bytes saved were not measured. These are ordinary-copy results, not chain cancellation or Jobs Resume acceptance.

A separate bounded download → verify → copy cancellation attempt completed before cancellation. The eight-file download and copy finished with matching hashes, and all 116 final node checks passed, including ownership, cleanup and preservation of previous copies and records. No cancellation confirmation or replacement attempt occurred. **Active-chain cancellation remains unproven.**

Offline replay examined all 80 saved observations. The observed copying interval was approximately 2.6 seconds, with target progress remaining at 0% before verification and completion. The node reports folder size every two seconds, Jobs refreshes every 2.5 seconds, and the accessible percentage is rounded. Sampling can miss a usable intermediate interval; the evidence does not establish a product defect or exact network throughput.

## Committed offline tool

`scripts/cancel_replay.py` emits proposed decisions from saved Accessibility-shaped snapshots. It contains no UI, process, network, authentication or node adapter and cannot operate SparkBox. It preselects an exact chain, checks the selected pinned revision, retains positive incomplete copying progress and normal Admin/dialog requirements, stops on either member becoming terminal, and latches closed before proposing one confirmation. A 15-minute bound, two-second maximum read, and 1.5-second active-observation budget reject stale decisions. The active budget includes capture time and cannot be extended by waiting for the modal.

The committed tests use a reduced synthetic phase/timing trace and generated minimal UI rows. Repository names, revisions, job identifiers and selected details are synthetic. Raw application captures, saved state, real identities and private handoff records are excluded. The private original replay remains preserved separately.

See [the replay guide](CANCELLATION-REPLAY.md) for offline use, limits and prerequisites for a separately authorized live attempt. A replay decision or simulated confirmation is not live acceptance or authorization.

## Verification and preservation

The private offline controller passed 22 tests, including the original capture. The portable version has 22 synthetic tests covering the same control paths, including progress boundaries, exact identity/revision, early selection, terminal members, completion races, ambiguous rows, changed dialogs, disabled controls, relocking, stale reads and deadlines. The tests forbid process creation and network sockets. The suite is included in `scripts/check.sh`.

During sanitization, two tests initially reused the synthetic valid revision as their deliberately wrong revision. They failed as expected for that mistaken fixture; the invalid revision is now distinct and the assertions are unchanged. This was a fixture conversion issue, not a product failure. The original private replay had separately exposed an invalid repository-component case, fixed before its final passing run.

All 141 previous private-round files, 60 selected saved settings/history files and nine prior archives were preserved at the offline checkpoint. No node or live UI was accessed by the offline round or this commit preparation. Last live evidence showed 26 retained Jobs and complete cleanup; that state is not freshly asserted by an offline run. Public source retains no connection defaults or private artifacts. Application Help behavior is unchanged, so no new UI captures or app launch are required.

All 35 script tests passed: 22 replay cases and 13 distribution/release-tool cases. Source privacy audit and diff checks cover this tooling/documentation commit. Application runtime did not change; the full app suite, native build and signed candidate were not repeated. Historical native-suite limitations in the preceding handoff remain recorded rather than being replaced with a new full-suite claim.

## Next round and delivery

The existing signed application remains the `92dfacf` build. This later source commit contains test tooling and documentation; it does not imply a newly signed binary or alter prior build provenance. No build, launch, push, notarization or publication is performed by wrapping up this checkpoint.

A future active-chain attempt needs separate authorization, fresh empty destinations and a reviewed artifact with enough expected copy time for multiple reporting cycles. Preserve completed copies. Integrate the tested policy with a default-disarmed normal-UI adapter and validate it offline before dispatch. Retain the positive-progress and exact-dialog gates; do not manufacture a window by throttling traffic, adding production delays or relaxing safeguards. Start at most one chain, then independently verify exact ownership, positive incomplete cancellation progress and all cleanup/preservation facts. If completion wins, preserve it and report unproven.

Managed tool installation, ordinary download Resume, network/VPN loss, sleep, reboot and multi-target cancellation remain separate acceptance. The earlier intermittent connection cause remains unresolved. Offline P3.4 HF cache revision grouping remains a separate next coding option. Stop after this checkpoint.
