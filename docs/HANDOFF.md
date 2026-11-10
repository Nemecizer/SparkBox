# P3.8 inspected-download handoff

Base: public v0.1.0, `3121d964b7fdfe87b8b0219011c3e4ebedd2ccaa`. Branch: `codex/p3.8-reviewed-download`. This is one offline coding round; the published release and private development history remain separate. No node, native app, VPN or paid AI operation is part of this round.

## Implemented

- New download and disk-preflight IPC require a `DownloadSelection`: canonical repository, full lowercase 40-character commit, and the complete reviewed names/sizes/available SHA-256 values. Missing fields, moving references, unsafe paths, glob patterns, duplicate files and unknown/invalid sizes refuse.
- Details resolve a selected branch/tag/revision once. Config, shard index and card use the returned immutable commit. A missing/substituted commit, different repository, or inconsistent/unreadable shard index refuses without fallback.
- Disk review and start re-read that exact commit and compare the manifest as a set. Order alone is immaterial; additions, removals, renames, size/hash changes or a different commit refuse before job or installation start. The accepted manifest drives disk admission and the existing pinned node job. Branch movement does not silently update a reviewed download.
- The Hugging Face tab offers a branch/tag list, Refresh branches, a Revision field and Inspect revision, full Commit text and Files to download. Typed changes disable download/advice until inspected; a failed inspection clears old details; delayed earlier responses are ignored. Branch-list failure leaves manual inspection available.
- `configError` preserves access/status, connection and invalid-JSON failures. Config HTTP 401/403/404 is visible instead of blank KV estimates. Download and AI advice refuse unreadable config. AI advice requests the inspected commit too; no AI call was made in this round.
- Help and its revision tip describe actual controls, mismatch refusal, access errors and retries. Browser Demo uses synthetic metadata and preserves the selected revision in its simulated job. Load still uses the existing Library copy; advice from a different inspected revision is not applied to it.

## Safeguards and compatibility

Admin/Touch ID, Look only, stable node identity, SSH verification, controller leases, ownership, tool fingerprint and installation consent, disk rechecks and debug fences are retained. Jobs Resume continues using its original saved plan and fresh Keychain token; no existing jobs, records or settings are migrated or removed. Node helper commands and secret delivery paths are unchanged.

Fresh installations still have empty native inventory, no VPN profile/forwards, locked Admin and no enabled automatic connections. Public privacy fixtures and ephemeral test keys are retained. Raw test/build evidence and private identifier deny lists stay outside this tree. Public Help examples remain synthetic.

## Verification

- TypeScript passed; final UI suite: 3,722 passed. Release Help and affected UI tests: 2,009 passed in the combined targeted run. Light/Dark browser Demo inspection passed and its temporary server/profile were cleaned up.
- Native library: 1,873 executed tests passed across the full run and unchanged isolated retries; 36 explicit fixture/hardware cases remain ignored. The first full run found the missing read-only classification for `hf_refs`; that was added with its metadata-only reason and the audit passed. A second overlapping full run had two existing fake-node timing failures (cleanup observation and PID startup); both passed alone, unchanged. This is not a claim that one final full run was entirely green.
- All 30 release-compiled safeguard tests passed. Binary and documentation targets completed with no test cases.
- New local HTTP/manifest tests and fake-node no-start checks passed, including changed files/commits, HTTP 401/403/404, malformed index, unsafe patterns, tool consent, fence/Look-only refusal and fresh-install preservation.
- Python node helpers: 515 passed, 3 platform-specific skips (518 total). Go passed. Clippy: 46 existing warnings, unchanged. Distribution/release-tool tests: 13 passed. Source distribution audit with the external private-identifier deny list found zero issues in 2,462 tracked files. Published v0.1.0 source/app ZIP hashes and both baseline checkouts remained unchanged.

Linux/systemd and real Hub/node acceptance are separate. Raw logs stay outside this public tree.

The candidate is built only after a clean commit with `scripts/public-release.py build`, which invokes `scripts/build-candidate.sh`. Output: `../SparkBox-builds/2026-11-10-p3.8-reviewed-download/` relative to the checkout. Its `PROVENANCE.json` gives the source commit, signing identity, file manifest and ZIP hash. This round does not submit for notarization, launch or publish.

## Owner inspection

1. Use the offline browser steps in [START_HERE.md](START_HERE.md). Check Light and Dark, the full commit and file list, and the synthetic tag selection.
2. Type a different revision without inspecting: download/advice must be dimmed. Inspect it, and confirm the new resolved commit. The `hf=config-error` scenario must show a readable refusal and no fit table.
3. In Demo select a node without a finished copy, review its tool, disk, commit and file count, then cancel. No job should be created. Demo confirmation can create only a simulated job. Cancel that job before it finishes, select **Resume download…**, choose its tool, and read the saved commit in the confirmation; cancel that confirmation. It must match the inspected commit. Jobs Resume retains the original plan.
4. A separately authorized native/live round should choose a small public model not already installed at the selected destination, inspect and record its full commit/files, cancel once, then approve the exact download. Verify the job and final manifest match, all files verify, and no unexpected tool, replacement node or copy begins. Review cleanup through normal ownership safeguards. Repeat gated-denial behavior without changing credentials for convenience. Do not infer real download success from browser Demo.

## Limits and next round

Selecting a revision does not overwrite a finished Library copy. This adds no automatic copy chain, capacity-based default node, cache deletion, throughput tuning or unattended acceptance. Disk readings are not reservations and do not guarantee separate cache/tool space. The stored manifest compares publisher metadata; actual downloaded bytes still use the existing size and available publisher-checksum verification. No missing publisher checksum is invented.

Recommended next offline round: the remaining P3.2 capacity-based default download-node suggestion using destination-disk preflight, while preserving an explicit user selection and excluding durable copy chaining from that round. Native acceptance of P3.1/P3.3/P3.5/P3.8 and the full P1.38 24-hour monitoring soak remain separately authorized live work. Stop after the signed candidate is verified.

## API reference

The revision and refs request shapes were checked against the official [Hugging Face Hub client](https://github.com/huggingface/huggingface_hub/blob/main/src/huggingface_hub/hf_api.py) and [repository revision documentation](https://huggingface.co/docs/huggingface_hub/guides/repository). Only documentation was consulted online; no live model, node, credential or inference validation was performed.
