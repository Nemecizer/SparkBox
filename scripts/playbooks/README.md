# Maintaining the bundled playbook library

The shipping app reads committed `resources/playbooks/catalog.json` and
`documents.json`. Building or opening How To never downloads NVIDIA content.
The app's explicit source checker is separate, in `src-tauri/src/howto/source.rs`.
It saves reference text only; it cannot revise this compiled action catalog.

`curation.tsv` contains the human-reviewed adaptations for the 69 guides linked
by NVIDIA's root index at revision
`30883150541743d580c9c347700c6ac99fc396d2`. Each row records category, purpose,
steps, expected result and cautions. Keep tabs as field separators and ` / ` as
the step separator. Legacy aliases help classify references but do not inherit
current instructions or executable actions.

The offline importer consumes a reviewed staging folder:

```sh
python3 scripts/playbooks/import-library.py /path/to/review-cache
```

Its inputs are `head.json` (the pinned GitHub commit), `tree.json` (the complete GitHub recursive tree at the pinned
revision), `documents.json` (a list of path/git_blob/sha256/bytes records),
`source/` (the original UTF-8 files at those paths), and `review-cards.json`
(the per-guide overview and step outline extracted during review). The November
6 cache was `/private/tmp/sparkbox-nvidia-playbooks-review`; it is disposable
research evidence, not a dependency for normal builds or tests. The importer
verifies every document's hashes before producing the bundle and coverage ledger.
It performs no network requests and executes no downloaded code.

For a later bundled refresh, obtain a complete tree and root index from the
official repository at one fixed commit, stage only documentation/licenses,
review every added/changed guide's hardware/prerequisites/workflow, and update
the pinned constants, curation and expected coverage counts together. Preserve
original text, licenses and modification notices. Review each image literal and
action separately in code; a successful documentation import is not permission
to add an executable operation. Supporting scripts/notebooks need their own
ownership, cancellation, rollback and hardware acceptance before automation.

Run the How To Rust/UI tests, the full required checks, release Help checks and
the candidate build procedure. The continuation report and coverage ledger
separate guided procedures, existing-tool shortcuts, image preparation and
unproven live behavior. Keep that distinction in future reports.
