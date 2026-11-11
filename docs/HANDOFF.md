# Live Library cache inspection handoff

Base: `4879869ce553dda8b9d27db00642558b5c5cfced`, following the [offline Library reliability round](P3.4-LIBRARY-HANDOFF.md). Branch: `codex/p3-live-download`. The owner authorized development and live testing after returning to the cluster network. This round follows the first live Library failure rather than starting several roadmap phases.

## Finding and change

The signed baseline candidate initially reported unreachable nodes. All four subsequently reconnected automatically without changing credentials, identity trust, VPN mode or node settings. A read-only Library scan then succeeded on one node but failed on the other three at missing files in old Hugging Face cache snapshots. Successful results remained visible, as intended by the previous round, but those failures hid otherwise usable models on the affected nodes.

A synthetic dangling snapshot link reproduced the same FileNotFoundError before the fix. Scans now distinguish that known incomplete snapshot from an unreadable node search:

- In a `models--…/snapshots/<revision>` folder, a missing link target is counted only when the link itself still exists. The snapshot stays incomplete and loses any older verified badge. No link, manifest, blob or model file is changed.
- Library displays **incomplete (2 missing)** and a per-node explanation in the selected row's details. Size counts present files only; it is not an estimate of the complete download or reclaimable space.
- Active download/copy markers retain their running state and protection against competing writes. A missing-file observation cannot become a finished Load/copy source. Native parsing rejects contradictory metadata.
- Permission errors, unreadable subtrees, broken search-root links, ordinary model-folder failures and a regular file disappearing during the scan still fail. This is not a relaxed error catch or a repair operation.
- A later scan can observe restored links. Without a verification record it remains **found**, not newly verified. Full cache inventory, revision grouping, blob reclamation and automatic repair remain outside this change.

## Safeguards and privacy

Admin, Touch ID, Look only, SSH identity checks, leases, instance ownership, protected update plans and debug fences are unchanged. No new mutation command is added. The native app remains locked during the read-only acceptance. No model load, download, copy, package update, reboot or cache deletion is part of this fix. Notifications and VPN settings remain unchanged. Raw live evidence stays outside the public tree; public fixtures contain synthetic accounts and paths.

## Verification

The initial live scan failure and read-only download-capacity review are recorded privately. A seven-file public test model was inspected at an immutable revision; both compared destination filesystems passed, and the capacity suggestion chose the node with more space after the existing reserve. No download was started. This establishes bounded read-only review behavior, not download/chain acceptance.

Focused offline helper tests cover missing cache weights/config links, ordinary failures, permission errors, concurrent regular-file removal, old manifest preservation, later rescanning and active-writer protection. UI tests cover the explanation, disabled Load and refusal to dispatch a copy from an incomplete-only source. Native tests reject inconsistent scan metadata and exclude an incomplete snapshot from source selection. Light and Dark browser Demo inspections show the per-node explanation without obscuring the Library. The temporary preview was closed.

TypeScript and all 3,757 UI tests across 129 files passed. All 13 focused helper tests and 13 release/distribution-tool tests passed. Source privacy audit covered 2,477 tracked files with zero private-identifier findings; the changed-source offline secret scan found no credentials. All 60 selected saved-state/history file hashes and modes still match. The remaining native/helper checks and corrected-candidate live acceptance will be recorded after completion. The signed baseline and older builds remain intact. Publication and notarization are not authorized by this round.

## Follow-up

After this bounded fix passes live scans, the next recommended acceptance is one small public-model download → verify → copy chain on one source and one empty target. It needs normal Admin authentication and explicit in-app tool/plan review. Verify the exact files, immutable revision, disk admission, source verification before sharing, target hashes, persisted jobs after reopening and temporary listener/secret cleanup. Cancellation and managed tool installation need their own bounded acceptance; no successful read-only review proves them.
