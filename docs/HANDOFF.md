# Connection diagnosis and small-chain acceptance

Base: `f7c9fbfe78a4b66190f2d2402c67d2987e6bf602`, following the [live Library round](LIBRARY-LIVE-HANDOFF.md). Branch: `codex/p3-connection-chain`. Implementation and live-tested app: `c446650155f03357a47490eb47007bd29b77058d`. The owner authorized connection diagnosis, one small download → verify → copy test if stable, documentation, checks, commit and a separate signed candidate. Publication and notarization are outside this round.

## Changes

Failed SSH connection attempts now record elapsed milliseconds, like successful attempts. Normally ending control connections also record their authenticated duration. Failure categories, retry timing, interruption handling, credentials and protocol checks are unchanged. These fields improve diagnosis; they do not establish or repair a network cause.

An offline regression reproduced a separate chain bug: saving the source download plan replaced its chain metadata. The helper now preserves both the exact download plan/hash and the chain's participant plan. Tests cover source/target records, finished/reopened records, disk checks, ownership fields, no credentials in saved status, and no destination writes during parent setup. Older `dc-` source records with missing chain metadata are refused by ordinary Resume at UI, native and node boundaries. The UI explains the missing participant plan and limited single-job cancellation; it never reconstructs or rewrites records.

Help explains the connection evidence, chain records, legacy recovery restriction and refresh after reopening. Download Help and release notes now distinguish the successful small live chain from untested installation, interruption and larger-transfer cases.

## Connection evidence and limits

Direct route errors occurred before SSH login on fresh sockets. Actual app targets matched saved inventory, without ProxyJump or a VPN path. Initial failures recovered automatically. Separate established sessions later rejected SSH packet framing and reconnected. The installed SSH dependency matched its locked archive. Neither error established a bad credential, network-filter cause or a reason to alter encryption, packet limits, rekey or trust.

Two separate 12-minute read-only observations passed: 148 TCP reachability checks each, 296 total, with no new SSH connection errors. One observation used the preceding candidate and one used the new candidate. The new candidate also stayed connected through the small transfer and reconnected after a normal application restart. This bounds the observed behavior; the intermittent root cause remains unresolved. Preserve logs if it recurs and compare actual targets, attempt timings and session durations before changing anything.

## Bounded live acceptance

The public repository `hf-internal-testing/tiny-random-LlamaForCausalLM` was pinned to `9fb191250dd56d0ba7ec9785a025ed29c03d5998`. Seven files totaled 6,477,074 bytes on each participant. One source downloaded using an already installed ARM64 vLLM image whose `hf` reported 1.30.0; one target copied over their existing common fabric subnet. No tool installation, image pull or firewall change was needed.

Normal owner Admin authentication and the explicit tool/plan dialogs were used. Both destinations were absent, installed rsync was confirmed, and both disks passed the full-transfer + 5% + 20 GiB admission check. The review included the fixed commit, files, exact destinations, image identity, fabric route and temporary source port. The source was observed serving verified files while the target copied; both jobs then finished.

Independent read-only checks passed on both nodes: **53 checks total**. They recomputed all seven SHA-256 hashes and matched them between nodes, checked publisher hashes where supplied, exact visible file sets, source/target manifests, identical chain metadata, the source's retained pinned plan/hash and completion acknowledgment. No partial marker, owned job/child process group, reviewed port listener, owned download container or per-job secret/server directory remained. All six preexisting job records on the participants retained their exact bytes and modes; only the two reviewed records were added. Completed model files and records were kept.

After normal quit/reopen, Admin was Read-only and all four nodes reconnected. Library showed both new copies verified alongside existing models. Jobs → Refresh showed the two completed chain records with the same participant plan and all eight older records. The active-following list emptied normally after terminal completion; it is not the persistent node job store. All 60 selected saved configuration/history files remained byte-for-byte and mode-for-mode unchanged. Task/activity records from the authorized test were retained. Ordinary job listing with Look only off can settle old dead records, so the node preservation baseline was captured before the new chain rather than claiming no earlier passive settlement ever occurred.

This proves one small successful existing-image chain and reopening finished records. It does **not** establish large-model throughput, cancellation, lost-network behavior during a chain, node-reboot recovery, managed tool installation or continued work while the Mac sleeps. No model was loaded and no package update or node restart was performed.

## Verification and candidate

The required checks were completed in stages rather than claiming a pass for an interrupted full invocation:

- TypeScript and all 3,758 UI tests across 129 files passed. After the final Help edits, all 1,942 release Help checks passed again.
- The native debug suite covered all 1,922 compiled test names exactly once in eight nonoverlapping batches: 1,886 passed, 36 intentionally ignored, zero failures. The binary test harness and doctests each completed successfully with zero tests. These harnesses did not launch the application.
- Python helpers ran 539 tests: 536 passed and three platform-specific cases skipped. Go tests, the VPN helper build and all 13 release/audit-tool tests passed.
- All 30 production-compiled safeguard tests passed. Clippy completed with the same 46 existing warnings; this round did not increase them.

Two earlier native invocations were interrupted before a final result; neither is counted as a full pass. The first batch runner also inherited system Python 3.9.6 and LibreSSL 3.3.6 instead of the required toolchain. Four fixtures refused an unavailable Python API and one correctly rejected a generated X.509 v1 certificate. All eight batches then passed with the installed Homebrew Python/OpenSSL, without changing product code or assertions. Building instructions now make the tool-path requirement explicit. The private evidence retains both the original failures and the corrected full coverage.

The final delivery procedure is to commit this report, build a new separate Developer ID-signed candidate, then audit the app and extracted ZIP without launching it. The accompanying `PROVENANCE.json` and private round handoff record the delivered commit, archive hash and final signature/archive/privacy checks. Runtime source remains the live-tested `c446650`; subsequent edits are documentation and Help. Do not confuse the final unopened delivery with the earlier candidate used for the authorized live test.

Raw logs, actual network identifiers, node paths, job IDs and preservation hashes remain outside this public tree. Developer ID signing does not imply notarization. Keep the published release, previous candidates, private repository and Claude-backup intact.

## Owner inspection and next round

1. In Models → Jobs, wait for the nodes to connect and click Refresh. Check that both completed rows have the same chain ID and source/target names, alongside older records. Do not start another chain into the finished folders.
2. In Library, confirm the small model is verified on both participants. Finished files were intentionally preserved.
3. If a connection fails again, use Help → Collect diagnostics and retain timestamps and SSH reasons. Do not reset credentials or host keys to investigate a route or packet-framing error.

The next recommended live round is bounded chain cancellation/interruption acceptance with separately reviewed empty destinations, followed by cleanup and record-preservation checks. Do not reuse or delete these finished model folders as an incidental test setup. Managed tool installation and node-reboot recovery need their own rounds. Offline P3.4 follow-ups remain HF cache revision grouping, destination free-space columns and reuse of recent scans.
