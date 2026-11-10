# P3.2 download-capacity handoff

Base: completed P3.8, `a4ebb2a9bfeeadb5497c67776b35f5f00fbacd6d`, following public v0.1.0 `3121d96`. Branch: `codex/p3.2-capacity-suggestions`. This is one offline coding round. The published release, private repository, existing settings and older builds remain unchanged. Previous implementation and validation history: [P3.8 handoff](P3.8-HANDOFF.md).

## Implemented

- Models → Hugging Face adds **Suggest a download node**. The user chooses the potential load members to compare; connected nodes without finished or running copies are initially included. This comparison scope does not change the Load draft or create a copy plan.
- The new read-only `model_download_capacity` command validates distinct inventory IDs, re-fetches the inspected immutable revision once and checks its full file manifest. It probes the actual destination filesystems, with at most four probes in flight. It does not use cached Dashboard disk readings or send node capacity to an AI provider.
- Each node returns either a disk result or its own failure. Unknown, disconnected and insufficient disks cannot win. Among qualifying nodes, maximum available bytes minus the full transfer × 1.05 (rounded up) plus 20 GiB wins. Stable node ID breaks ties. Fullness warnings remain visible and do not silently disqualify a disk with sufficient space.
- Comparison alone never changes the persisted download target. **Use <node>** applies the suggestion explicitly. Manual choices survive comparison, failures, node reordering, disconnection and removal. Jobs Resume retains its original node and plan.
- Results are ephemeral, expire one minute from request start and are invalidated by model/revision, comparison scope, connection or relevant copy-state changes. Late superseded results are ignored. Typing an uninspected revision blocks comparison as well as download. Expired results cannot be applied even if a timer is delayed.
- Download still performs its own tool choice, destination review, final confirmation and node-side disk admission. No capacity result is an authorization, reservation, tool install, model folder creation or job start.
- Help and the target tip explain comparison, explicit application, expiry and limits. Synthetic browser Demo adds `disk=capacity`; `low` and `unknown` retain refusal cases.

## Safeguards and privacy

Admin/Touch ID, Look only, verified SSH identity, controller leases, instance ownership, exact tool/install consent, immutable file review, protected updates and debug fences are unchanged. Read-only classification is explicit and subject to the existing command audit. Native fresh installs still have empty inventory, no VPN profile/forwards and locked Admin. No saved records, private credentials or live endpoints are added, migrated or removed. Raw evidence and the private identifier deny list stay outside public source.

## Verification

- TypeScript and all 3,733 UI tests passed (127 files). Release Help plus affected Hub tests: 1,980 passed. Light/Dark browser Demo confirmed capacity ranking, explicit application, low/unreadable refusal and cleanup of its temporary browser/server.
- New native ranking and fake-node tests passed, including stable tie ordering, subset isolation, offline/unreadable nodes, changed-manifest refusal, invalid identities and no tool/job starts.
- Clippy completed with the existing 46 warnings, unchanged; none point to the new capacity module.
- Source distribution audit with the external private-identifier deny list: 2,466 tracked files, zero findings.
- Full native suite: 1,875 passed, zero failures, 36 explicitly ignored hardware/fixture cases. Binary and documentation targets completed with no test cases. The final full invocation passed without retries.
- All 30 release-compiled safeguard tests passed. Python helpers: 515 passed, 3 platform-specific skips (518 total). Go passed. Distribution and public-release-tool tests: 13 passed. The complete `scripts/check.sh` invocation passed.

No native app, real node, VPN, model download, paid AI call or publication is part of this work. Raw logs remain outside public source.

## Candidate

After a clean commit, `scripts/public-release.py build` invokes `scripts/build-candidate.sh` and creates `../SparkBox-builds/2026-11-10-p3.2-capacity-suggestions/` relative to the source checkout. The app is Developer ID signed; provenance records its exact source and ZIP hash. This round does not launch, notarize or publish it.

## Owner inspection

Use the browser-only procedure in [START_HERE.md](START_HERE.md). Compare the synthetic disks, verify that the current Download node remains unchanged until **Use cell2**, then check low/unknown disk cases. Exclude cell2 and compare again to confirm it cannot be suggested. Change Revision without inspecting to confirm comparison is dimmed. Wait one minute to confirm results expire. Repeat in Dark.

Any native acceptance needs separate authorization. With configured connected nodes, inspect a small public model and compare only the intended potential load members. Check destination paths, reserve arithmetic and warnings. Confirm the selected target does not change until **Use**, then cancel tool/download review and verify no job starts. A separate approved download can verify disk rechecks and final files. Do not infer live acceptance from Demo.

## Limits and next round

This implements capacity-based suggestions, with explicit application instead of silently replacing a default. It does not prove GPU fit, bandwidth, fabric suitability, cache/tool capacity, quotas or inode headroom. Disk readings reserve no space. Comparison candidates come from the user's selection here; no pre-download Load draft is inferred. Existing finished copies should be used or copied through Library. P3.2's durable **Then copy to** download → verify → serve → pulls chain remains unfinished.

Recommended next offline round: design and implement that bounded durable copy chain with exact saved destinations, ownership checks, explicit consent, cancellation and restart recovery. Native acceptance of the download changes and the full P1.38 monitoring soak remain separate live work. Stop after this round's candidate verification.
