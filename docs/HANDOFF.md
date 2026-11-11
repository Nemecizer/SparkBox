# Bounded chain cancellation and reconnection acceptance

Base and live-tested candidate: `5a0062e1d2422bcc6de1139472e97bca6692a920`. Branch: `codex/p3-chain-recovery`. The previous [connection and small-chain handoff](CONNECTION-CHAIN-HANDOFF.md) is preserved. The owner authorized bounded live cancellation and reconnection, preservation of copies/history/safeguards, cleanup checks, documentation, testing, commit and a separate signed candidate. Publication and notarization are outside this round.

## Result and changes

Early chain cancellation and normal application quit/reopen passed their bounded live checks. No runtime defect was found, and runtime source is unchanged from the reviewed base. Help now explains the observed recovery behavior, distinguishes early cancellation from active file sharing, and explains that Tasks can follow persistent node jobs. Stale statements that all live immutable-download acceptance remained pending are corrected. Article verification flags remain unchanged.

The delivered candidate contains these documentation and Help changes. It is built from the final clean commit and remains unopened. The live test used the prior reviewed candidate, not this final delivery. The accompanying provenance and private handoff identify the exact delivery commit, archive hash and signing checks.

## Live evidence

All three plans used normal owner Admin authentication, explicit consent to an already installed download image, immutable repository commits and eight inspected files. Both destinations had to be absent. Native admission checked the full transfer plus 5% and 20 GiB, installed rsync, the common fabric, temporary port, exact tool identity, ownership and leases. No tools were installed, images pulled, nodes restarted or network/firewall/credential settings changed. No model was loaded.

1. **Completed before cancellation.** `HuggingFaceTB/SmolLM2-135M-Instruct`, revision `12fd25f77366fa6b3b4b768ec3050bf629380bac`, finished before the cancellation confirmation. The late dialog was dismissed with **Keep going**; cancellation was never dispatched. Both completed copies remain. All **86 independent checks** passed, including eight matching payload hashes, pinned plans, cleanup and preservation. Each copy contains 272,437,573 payload bytes. This attempt is not cancellation acceptance.
2. **Early cancellation.** `HuggingFaceTB/SmolLM2-360M-Instruct`, revision `a10cc1512eabd3dde888204e902eca88bddb4951`, was canceled through **Cancel chain…** while the source checked its selected tool and the target waited. Both records ended canceled. All **94 independent checks** passed. The source retains its partial marker, and the target never claimed a model folder. The source worker, recorded tool-check child, owned container and process group were absent; no reviewed listener or per-job secret/server directory remained. Completed models and prior records retained their exact declared bytes and modes.
3. **Active normal quit/reopen.** `HuggingFaceTB/SmolLM2-360M`, revision `f8027fd0eaeea54caa13c31d31b9fdc459c38b49`, had both member rows active immediately before normal Cmd-Q: source checking its tool, target waiting. The app process ended normally without force. Reopening the same reviewed app relocked Admin, connected all four nodes and followed the same chain IDs. Both records reached done; a task marked **picked up again** was observed. Jobs showed all **16 old and new records**, without replacement jobs. After normal fresh owner authentication, all **120 independent node checks** passed. All eight fresh payload hashes matched between nodes, totaling 727,051,810 bytes per copy. Plans, acknowledgments, manifests, cleanup and all 12 preceding participant records passed; earlier completed models and canceled artifacts were unchanged.

Library showed the original small model and both newly completed models verified on both participants. The canceled model remained incomplete on its source and absent on its target. The app was left **Read-only**, with all four nodes online. All **60 selected saved configuration/history files** and **seven prior archives** retained their bytes and modes. Normal task/activity records from the authorized work were retained. Keychain contents were not read or exported.

## Evidence limits

The first strict cancellation inspection kept the waiting target's missing child PID as unknown. It did not equate missing identity with absence. A separate inspection used a narrowly reviewed proof for this exact waiting state: matching boot and recorded parent identity, absent parent and inherited process group, an unclaimed destination, and all other resource checks clear. The child field remains unknown with explicit group coverage in private evidence. The readiness and failure-acknowledgment subprocesses inherit that group. No application safeguard or health check was changed.

No pre-quit node PID/start checkpoint was captured. Reconnection acceptance uses the two active UI rows immediately before normal quit, actual Mac process termination, the same recovered chain IDs, pinned final records, no extra jobs and independent final file/cleanup checks. Do not claim an unrecorded PID-continuity proof. The canceled source directory's earlier mode was not captured; its declared marker/file bytes and modes were preserved. Completed-directory mode checks remained strict. Hidden files outside the declared model/marker scope were not broadly read.

This does not establish cancellation during active serving/copying, lost-network or VPN behavior during a chain, Mac sleep, node-reboot recovery, managed tool installation, ordinary Resume or large-model throughput. The earlier intermittent connection cause remains unresolved; this round did not alter connection handling or credentials.

## Offline verification and delivery

- All 10 download-chain helper tests passed using fake nodes and loopback fixtures.
- TypeScript and all **3,758 UI tests across 129 files** passed. After the Help edits, all **2,145 Help tests across 10 files** passed with release validation enabled.
- All **13 release/audit-tool tests** passed. Private UI-watch helpers also passed 20 cancellation and 13 normal-quit tests; these are orchestration checks, not product acceptance.
- The new worktree initially lacked dependencies. Two initial npx attempts tried unavailable registry access and failed; neither is counted as a pass. Package and lock files exactly matched the reviewed previous worktree. Dependencies were cloned locally into ignored node_modules, and the successful checks used those local binaries without network access. Original failure logs are preserved.
- The prior full native, Python, Go, release-safeguard and Clippy results remain recorded in the previous handoff. They were not rerun in full because runtime source did not change. The same 46-warning Clippy baseline is preserved; no runtime changes were introduced to affect it.

Commit the completed documentation, then use the normal Developer ID Application build from a clean tree. Verify exact source provenance, signed app and extracted ZIP, file permissions, resources, private-identifier absence and offline secret scans. The private delivery record contains final results; do not confuse Developer ID signing with notarization. Preserve prior apps, published v0.1.0, private source and Claude-backup. Do not launch or publish the final candidate.

## Owner inspection and next round

1. In **Models → Jobs**, wait for connection and click **Refresh**. Confirm all 16 records remain: the two new finished chains and one canceled chain, alongside older records. Each chain's source and target must share its chain ID.
2. In **Library**, confirm `SmolLM2-135M-Instruct` and `SmolLM2-360M` are verified on both participants. `SmolLM2-360M-Instruct` should be incomplete only on the canceled source. Keep these files and records; do not start another chain into their existing folders.
3. Leave the new signed candidate unopened until its separate test is authorized. Its Help and handoff contain this bounded acceptance and the remaining limits.

The next live acceptance should target cancellation during active file sharing or copying, with a separately reviewed empty destination and the same preservation/cleanup checks. Do not reuse or delete these retained folders as incidental setup. For offline development, the next bounded P3.4 option remains HF cache revision grouping; destination free-space columns and recent-scan reuse are separate follow-ups. Stop after this round rather than starting another phase.
